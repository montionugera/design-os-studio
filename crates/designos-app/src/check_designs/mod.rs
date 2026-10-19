//! Check designs: scans layers for design-system drift and offers a one-click fix per finding.
//!
//! Three rules, as in Figma's "Check designs":
//! - **Hard-coded values**: a fill, stroke, text style or effect list holds a raw value that equals an
//!   existing variable or shared style but is not linked to it. The suggested fix links it.
//! - **Contrast**: text fails WCAG 2.x AA against the backdrop its ancestors paint.
//! - **Detached instances**: a frame that was detached from a main component present in this file;
//!   the reconnect fix is offered only while the frame still equals the component.
//!
//! The audit is a pure function of the [`Document`]. Fixes run through [`crate::edit::Tx`], so each
//! fix (and "Fix all") is exactly one undo step. Layers inside an instance mirror their main
//! component and are skipped: the fix belongs on the main component, and the instance follows.

mod contrast;
mod detached;
mod fix;
pub mod fixture;
mod hardcoded;
mod state;
#[cfg(test)]
mod tests;

pub use fixture::FIXTURE;
pub use state::CheckDesignsState;

use designos_doc::{BlendMode, Color, Document, LibraryRef, LibrarySnapshot, Node, NodeId, NodeKind, PaintKind};
use std::sync::Arc;

/// Meaningful layers scanned per audit. A page of several hundred thousand layers must never stall
/// the UI thread, so the audit stops here and the panel says so.
pub const MAX_SCANNED_LAYERS: usize = 20_000;

/// Total visible layers examined per audit, including quiet mirrors inside component instances.
///
/// Measured release-mode `audit_with_libraries` wall times on the synthetic mirror page:
/// - 10,000 visited layers: 0.91 ms (`0.908 ms`)
/// - 50,000 visited layers: 6.19 ms (`6.187 ms`)
/// - 100,000 visited layers: 13.18 ms (`13.184 ms`)
/// - 200,000 visited layers: 26.08 ms (`26.084 ms`, well under the 150 ms UI-thread budget)
/// - 112,474-node synthetic large fixture: 3.71 ms (`3.713 ms`) vs 3.78 ms (`3.777 ms`) pre-change
pub const MAX_VISITED_LAYERS: usize = 200_000;

/// Which scan cap stopped an audit early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Truncation {
    /// Stopped at [`MAX_SCANNED_LAYERS`] meaningful layers.
    Scanned,
    /// Stopped at [`MAX_VISITED_LAYERS`] total examined layers.
    Work,
}

/// The kind of problem a [`Finding`] reports; also the group it is listed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    HardCodedValue,
    Contrast,
    DetachedInstance,
}

impl Rule {
    pub const ALL: [Rule; 3] = [Rule::HardCodedValue, Rule::Contrast, Rule::DetachedInstance];

    /// Group heading in the panel.
    pub fn title(self) -> &'static str {
        match self {
            Rule::HardCodedValue => "Hard-coded values",
            Rule::Contrast => "Contrast",
            Rule::DetachedInstance => "Detached instances",
        }
    }
}

/// The edit that resolves a finding.
#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    /// Bind the property `key` (`fills/0`, `strokes/1`, `itemSpacing`, …) to a variable.
    BindVariable { key: String, variable: String },
    /// Bind several properties at once (uniform corner radius or uniform padding) to a variable.
    BindVariables { keys: Vec<String>, variable: String },
    /// Link the layer to a shared style (fill, stroke, text or effect: the style knows which).
    ApplyStyle { style: String },
    /// Replace the text colour with `color` (alpha is kept).
    SetTextColor { color: Color },
    /// Replace the detached frame with an instance of `component`, keeping its name and size.
    Reconnect { component: NodeId },
}

/// One problem on one layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub node: NodeId,
    pub node_name: String,
    pub rule: Rule,
    /// What is wrong, as shown in the row.
    pub message: String,
    /// The one-click fix, when there is one.
    pub fix: Option<Fix>,
    /// What the fix does, for its tooltip (empty without a fix).
    pub fix_label: String,
}

impl Finding {
    /// Whether this finding suggests an asset from an enabled library.
    pub fn is_from_library(&self) -> bool {
        self.message.contains(" from library \"")
    }
}

/// What an audit looks at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every top-level layer of a page.
    Page(NodeId),
    /// These layers and everything below them.
    Layers(Vec<NodeId>),
}

/// Enabled library snapshots collected from the app cache before an audit.
#[derive(Clone, Debug, Default)]
pub struct LibrarySources {
    pub(super) libs: Vec<(LibraryRef, Arc<LibrarySnapshot>)>,
}

/// Result of one audit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Meaningful layers examined.
    pub scanned: usize,
    /// The scan stopped at [`MAX_SCANNED_LAYERS`] or [`MAX_VISITED_LAYERS`].
    pub truncated: bool,
    /// Which limit stopped the scan, when [`Self::truncated`] is `true`.
    pub truncated_by: Option<Truncation>,
    /// Display names of enabled libraries whose pinned snapshot could not be read.
    pub unreadable_libraries: Vec<String>,
}

impl Report {
    /// Findings of one rule, in document order.
    pub fn of(&self, rule: Rule) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(move |f| f.rule == rule)
    }

    /// Findings that carry a fix.
    pub fn fixable(&self) -> Vec<&Finding> {
        self.findings.iter().filter(|f| f.fix.is_some()).collect()
    }
}

fn has_opaque_fill(n: &Node) -> bool {
    n.fills
        .iter()
        .any(|p| p.visible && matches!(p.blend, BlendMode::Normal | BlendMode::PassThrough) && matches!(p.kind, PaintKind::Solid(c) if c.a * p.opacity >= 1.0))
}

fn same_own_as_source(n: &Node, src: &Node) -> bool {
    n.opacity == src.opacity
        && n.blend == src.blend
        && n.fills == src.fills
        && n.stroke == src.stroke
        && n.effects == src.effects
        && n.corner_radii == src.corner_radii
        && n.style_refs == src.style_refs
        && n.variables == src.variables
        && n.detached_symbol_id == src.detached_symbol_id
        && n.component.detached_symbol_id == src.component.detached_symbol_id
        && match (&n.kind, &src.kind) {
            (NodeKind::Text(a), NodeKind::Text(b)) => a.style == b.style && a.runs == b.runs,
            _ => n.kind.frame_props() == src.kind.frame_props(),
        }
}

/// Whether `n` is an un-overridden mirror layer inside an instance whose ancestors up to the
/// instance root are also un-overridden and fully opaque (`opacity >= 1.0`, normal blend).
fn is_quiet_mirror(doc: &Document, n: &Node) -> bool {
    let Some(src_id) = n.component.source else { return false };
    if !n.component.overrides.is_empty() || !n.variables.modes.is_empty() {
        return false;
    }
    let Some(src) = doc.get(src_id) else { return false };
    if !same_own_as_source(n, src) {
        return false;
    }
    let mut opaque_backdrop = !matches!(n.kind, NodeKind::Text(_)) || has_opaque_fill(n);
    let mut cur = n.parent;
    while let Some(pid) = cur {
        let Some(anc) = doc.get(pid) else { return false };
        if anc.component.source.is_none() {
            break;
        }
        if !anc.component.overrides.is_empty()
            || !anc.variables.modes.is_empty()
            || anc.opacity < 1.0
            || !matches!(anc.blend, BlendMode::Normal | BlendMode::PassThrough)
        {
            return false;
        }
        if let Some(anc_src) = anc.component.source.and_then(|s| doc.get(s)) {
            if anc.fills != anc_src.fills || anc.opacity != anc_src.opacity || anc.blend != anc_src.blend {
                return false;
            }
        }
        opaque_backdrop |= has_opaque_fill(anc);
        cur = anc.parent;
    }
    opaque_backdrop
}

/// Scans `scope` with the document's own variables and styles only (the app always audits with
/// its enabled libraries through [`audit_with_libraries`]). Hidden layers are skipped; layers inside
/// an instance are audited only where their effective properties or backdrop diverge from their
/// main component.
#[cfg(test)]
pub fn audit(doc: &Document, scope: &Scope) -> Report {
    audit_with_libraries(doc, scope, &LibrarySources::default())
}

#[cfg(test)]
pub(super) fn audit_with_cheap_skip(doc: &Document, scope: &Scope, libs: &LibrarySources, cheap_skip: bool) -> Report {
    audit_impl(doc, scope, libs, cheap_skip)
}

/// Scans `scope` and includes variables and styles from enabled `libs` snapshots.
pub fn audit_with_libraries(doc: &Document, scope: &Scope, libs: &LibrarySources) -> Report {
    audit_impl(doc, scope, libs, true)
}

fn audit_impl(doc: &Document, scope: &Scope, libs: &LibrarySources, cheap_skip: bool) -> Report {
    let roots: Vec<NodeId> = match scope {
        Scope::Page(page) => doc.get(*page).map(|p| p.children.clone()).unwrap_or_default(),
        Scope::Layers(ids) => ids.clone(),
    };
    let index = hardcoded::Index::with_libraries(doc, libs);
    let components = detached::Components::new(doc);
    let bounds = contrast::SubtreeBounds::default();
    let mut report = Report::default();
    let mut visited = 0usize;
    let mut stack: Vec<NodeId> = roots.into_iter().rev().collect();
    while let Some(id) = stack.pop() {
        let Some(n) = doc.get(id) else { continue };
        if !n.visible || matches!(n.kind, NodeKind::Page { .. }) {
            continue;
        }
        if visited >= MAX_VISITED_LAYERS {
            report.truncated = true;
            report.truncated_by = Some(Truncation::Work);
            break;
        }
        if cheap_skip && is_quiet_mirror(doc, n) {
            visited += 1;
            stack.extend(n.children.iter().rev().copied());
            continue;
        }
        let always_counted = n.component.source.is_none() || !n.component.overrides.is_empty();
        if always_counted && report.scanned >= MAX_SCANNED_LAYERS {
            report.truncated = true;
            report.truncated_by = Some(Truncation::Scanned);
            break;
        }
        visited += 1;
        let before = report.findings.len();
        hardcoded::check(doc, &index, n, &mut report.findings);
        contrast::check(doc, &bounds, n, &mut report.findings);
        detached::check(doc, &components, n, &mut report.findings);
        if always_counted || report.findings.len() > before {
            if report.scanned >= MAX_SCANNED_LAYERS {
                report.findings.truncate(before);
                report.truncated = true;
                report.truncated_by = Some(Truncation::Scanned);
                break;
            }
            report.scanned += 1;
        }
        stack.extend(n.children.iter().rev().copied());
    }
    report
}
