//! Panel state: what is being checked, the cached audit report, and the auto-fix report/settings.

use super::{audit_with_libraries, FixRow, LibrarySources, Report, Scope};
use crate::app::App;
use designos_doc::{AutoFixMode, AutoFixSettings, Command, LibraryRef, REMOTE_LIBRARY_PREFIX, SAFE_AUTO_FIX_RULES};
use std::collections::VecDeque;

/// Maximum number of auto-fix reports retained in session memory (`E15-14 §5`).
pub const MAX_AUTO_FIX_HISTORY: usize = 20;

/// Maximum number of rows rendered in the `Last auto-fix` list before a `+N more fixes` footer (`E15-14 §5`).
pub const MAX_AUTO_FIX_ROWS: usize = 200;

/// One auto-fix pass recorded from `doc-patch.jsonl` (applied or report-only).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoFixReport {
    pub label: String,
    pub report_only: bool,
    pub rows: Vec<FixRow>,
}

impl LibrarySources {
    /// Collects pinned snapshots for every enabled library in `app.doc.library.enabled`, plus the
    /// display names of any enabled library whose snapshot could not be read.
    pub(super) fn collect(app: &mut App) -> (Self, Vec<String>) {
        let enabled = app.doc.library.enabled.clone();
        let mut libs = Vec::new();
        let mut unreadable = Vec::new();
        for r in enabled.into_iter().filter(|r| !r.id.starts_with(REMOTE_LIBRARY_PREFIX)) {
            match app.snapshot_of(&r.id, r.version) {
                Some(snap) => libs.push((r, snap)),
                None => unreadable.push(r.name),
            }
        }
        (Self { libs }, unreadable)
    }
}

/// Check designs panel state. The scope is fixed when the panel opens, so selecting a finding's
/// layer does not change what is being listed.
#[derive(Default)]
pub struct CheckDesignsState {
    pub open: bool,
    pub scope: Option<Scope>,
    /// "Selection (3 layers)" or "Page 1", shown under the title.
    pub scope_label: String,
    pub report: Report,
    /// `(document revision, scope, enabled libraries)` the cached `report` was computed for.
    audited: Option<(u64, Scope, Vec<LibraryRef>)>,
    /// Most recent auto-fix report for the active document tab.
    pub last_auto_fix: Option<AutoFixReport>,
    /// Ring buffer of up to [`MAX_AUTO_FIX_HISTORY`] recent auto-fix reports in this session.
    pub history: VecDeque<AutoFixReport>,
    /// User-level default auto-fix settings (`~/.designos/settings.json` `"auto_fix"`).
    pub user_auto_fix: AutoFixSettings,
    /// True when `settings.json` could not be read or parsed; shown as an inline notice in Settings.
    pub settings_unreadable: bool,
    /// Whether the `Settings` disclosure inside the `Auto-fix` section is expanded.
    pub settings_open: bool,
    /// Scroll the Check designs panel to the `Auto-fix` section on the next frame (set by `Review`).
    pub scroll_to_auto_fix: bool,
}

impl CheckDesignsState {
    /// Forces the next frame to audit again.
    pub fn invalidate(&mut self) {
        self.audited = None;
    }

    /// Records a new auto-fix report into `last_auto_fix` and the 20-entry `history` ring buffer.
    pub fn record_auto_fix(&mut self, label: impl Into<String>, report_only: bool, rows: Vec<FixRow>) {
        let entry = AutoFixReport { label: label.into(), report_only, rows };
        self.last_auto_fix = Some(entry.clone());
        self.history.push_back(entry);
        while self.history.len() > MAX_AUTO_FIX_HISTORY {
            self.history.pop_front();
        }
    }
}

impl App {
    /// Opens the panel on the selected layers, or on the whole page when nothing is selected.
    /// Preserves the tab's auto-fix report and loaded user settings.
    pub fn open_check_designs(&mut self) {
        let layers: Vec<_> = self.selection.iter().copied().filter(|id| self.doc.get(*id).is_some()).collect();
        let (scope, label) = if layers.is_empty() {
            let name = self.doc.get(self.page).map(|p| p.name.clone()).unwrap_or_default();
            (Scope::Page(self.page), format!("Page: {name}"))
        } else {
            let label = if layers.len() == 1 { "Selection (1 layer)".to_string() } else { format!("Selection ({} layers)", layers.len()) };
            (Scope::Layers(layers), label)
        };
        self.check_designs.open = true;
        self.check_designs.scope = Some(scope);
        self.check_designs.scope_label = label;
        self.check_designs.report = Report::default();
        self.check_designs.audited = None;
        self.cmdk.open = false;
    }

    /// Opens the Check designs panel and scrolls to the `Auto-fix` section.
    pub fn open_check_designs_auto_fix(&mut self) {
        self.open_check_designs();
        self.check_designs.scroll_to_auto_fix = true;
    }

    /// Effective auto-fix settings for the active file, following the precedence:
    /// `Document::auto_fix` (if `Some`) -> user `settings.json` `"auto_fix"` -> built-in default.
    pub fn effective_auto_fix(&self) -> AutoFixSettings {
        let user = &self.check_designs.user_auto_fix;
        match &self.doc.auto_fix {
            Some(file) => {
                let mut rules = std::collections::BTreeMap::new();
                for &r in &SAFE_AUTO_FIX_RULES {
                    let on = file.rules.get(r).copied().or_else(|| user.rules.get(r).copied()).unwrap_or(true);
                    rules.insert(r.to_string(), on);
                }
                for (k, v) in &file.rules {
                    rules.insert(k.clone(), *v);
                }
                AutoFixSettings { mode: file.mode, rules }
            }
            None => user.clone(),
        }
    }

    /// Sets the per-file (`This file`) auto-fix mode as one undoable document command.
    pub fn set_file_auto_fix_mode(&mut self, mode: AutoFixMode) {
        let mut next = self.doc.auto_fix.clone().unwrap_or_else(|| self.effective_auto_fix());
        next.mode = mode;
        if self.doc.auto_fix.as_ref() == Some(&next) {
            return;
        }
        if self.hist.exec(&mut self.doc, Command::SetAutoFix { settings: Some(next) }).is_ok() {
            self.changed();
        }
    }

    /// Sets a per-file (`This file`) safe-tier rule toggle as one undoable document command.
    pub fn set_file_auto_fix_rule(&mut self, rule: &str, enabled: bool) {
        let mut next = self.doc.auto_fix.clone().unwrap_or_else(|| self.effective_auto_fix());
        if next.rule_enabled(rule) == enabled && self.doc.auto_fix.is_some() {
            return;
        }
        next.set_rule(rule, enabled);
        if self.hist.exec(&mut self.doc, Command::SetAutoFix { settings: Some(next) }).is_ok() {
            self.changed();
        }
    }

    /// Sets the user-level (`My default`) auto-fix mode and persists it to `settings.json`.
    pub fn set_user_auto_fix_mode(&mut self, mode: AutoFixMode) {
        self.check_designs.user_auto_fix.mode = mode;
        self.persist_user_auto_fix();
    }

    /// Sets a user-level (`My default`) safe-tier rule toggle and persists it to `settings.json`.
    pub fn set_user_auto_fix_rule(&mut self, rule: &str, enabled: bool) {
        self.check_designs.user_auto_fix.set_rule(rule, enabled);
        self.persist_user_auto_fix();
    }

    fn persist_user_auto_fix(&mut self) {
        if let Some(lib) = &self.lib {
            match crate::comments::settings::save_auto_fix(lib.root(), &self.check_designs.user_auto_fix) {
                Ok(()) => self.check_designs.settings_unreadable = false,
                Err(e) => self.show_toast(e, true),
            }
        }
        self.terminal.set_auto_fix_user(self.check_designs.user_auto_fix.clone());
        self.terminal.sync_live_file_context(&self.doc, self.page, &self.selection, self.file_path.as_deref());
    }

    /// Re-audits when the document or the scope changed since the cached report. While the user is
    /// mid-gesture (`interacting`) an existing report is kept, so a drag does not audit every frame.
    pub fn refresh_check_designs(&mut self, interacting: bool) {
        // A page scope whose page was deleted would list nothing for ever; follow the page on screen.
        if matches!(self.check_designs.scope, Some(Scope::Page(p)) if self.doc.get(p).is_none()) {
            let name = self.doc.get(self.page).map(|p| p.name.clone()).unwrap_or_default();
            self.check_designs.scope = Some(Scope::Page(self.page));
            self.check_designs.scope_label = format!("Page: {name}");
            self.check_designs.invalidate();
        }
        let Some(scope) = self.check_designs.scope.clone() else { return };
        let key = (self.canvas.tiles.rev(), scope, self.doc.library.enabled.clone());
        if self.check_designs.audited.as_ref() == Some(&key) || (interacting && self.check_designs.audited.is_some()) {
            return;
        }
        let (libs, unreadable) = LibrarySources::collect(self);
        let mut report = audit_with_libraries(&self.doc, &key.1, &libs);
        report.unreadable_libraries = unreadable;
        self.check_designs.report = report;
        self.check_designs.audited = Some(key);
    }
}
