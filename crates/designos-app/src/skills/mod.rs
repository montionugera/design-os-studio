//! Built-in and user `SKILL.md` attachments for Agent Terminal design runs (`RD-4` / `#573`).
//!
//! - **Built-in skills**: compiled into the binary from `assets/skills/<name>/SKILL.md`.
//!   Front matter requires `name`, `description`, `version`, and `license` (plus `attribution`
//!   when adopted). Missing `license` fails both `build.rs` and `parse_builtin_skill`.
//! - **User skills**: discovered from `<library root>/skills/<name>/SKILL.md` (`<= 64 KB`).
//!   Front matter requires `name` and `description`; `version` defaults to the first 12 hex
//!   characters of the file's SHA-256 digest.
//! - **Limits**: up to 3 skills per run, combined body `<= 8,000` tokens (`chars / 4`).
//!   Exceeding either limit disables the start/send control with an inline notice.
//! - **Persistence & Provenance**: default selection is stored in `settings.json` under
//!   `skills.default`, and every run writes `<session_dir>/run.json` recording `{name, version, sha256}`
//!   for each attached skill.

use crate::comments::settings as app_settings;
use crate::icons::{self, Icon};
use crate::terminal::{
    AgentCliPreset, STATUS_BAD_BG, STATUS_BAD_BORDER, STATUS_BAD_TEXT, STATUS_INFO_BG, STATUS_INFO_BORDER, STATUS_INFO_TEXT, STATUS_OK_BG, STATUS_OK_BORDER,
    STATUS_OK_TEXT, STATUS_WARN_BG, STATUS_WARN_BORDER, STATUS_WARN_TEXT, TERM_BG_ELEVATED, TERM_BG_HOVER, TERM_BORDER,
};
use crate::theme;
use crate::widgets;
use egui::{Align2, Color32, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Maximum allowed file size (`64 KB`) for a user `SKILL.md` in `<library root>/skills/<name>/SKILL.md`.
pub const MAX_USER_SKILL_BYTES: usize = 64 * 1024;

/// Maximum number of skills that can be attached to a single agent run.
pub const MAX_SKILLS_PER_RUN: usize = 3;

/// Maximum combined body token budget (`chars / 4`) across all selected skills.
pub const MAX_COMBINED_TOKENS: usize = 8_000;

/// Built-in skills embedded at compile time from `assets/skills/<name>/SKILL.md`.
pub const BUILTIN_SKILLS_RAW: &[(&str, &str)] = &[
    ("design-system-rhythm", include_str!("../../../../assets/skills/design-system-rhythm/SKILL.md")),
    ("accessibility-wcag", include_str!("../../../../assets/skills/accessibility-wcag/SKILL.md")),
    ("component-craft", include_str!("../../../../assets/skills/component-craft/SKILL.md")),
];

/// Origin of a discovered skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillSource {
    BuiltIn,
    User(PathBuf),
}

impl SkillSource {
    pub fn badge_label(&self) -> &'static str {
        match self {
            SkillSource::BuiltIn => "BUILT-IN",
            SkillSource::User(_) => "USER",
        }
    }
}

/// Validation state of a discovered skill entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillStatus {
    Valid,
    TooLarge(u64),
    InvalidFrontMatter(String),
}

impl SkillStatus {
    pub fn is_valid(&self) -> bool {
        matches!(self, SkillStatus::Valid)
    }

    pub fn error_badge(&self) -> Option<&'static str> {
        match self {
            SkillStatus::Valid => None,
            SkillStatus::TooLarge(_) => Some("Too large (> 64 KB)"),
            SkillStatus::InvalidFrontMatter(_) => Some("Invalid front matter"),
        }
    }
}

/// Parsed skill entry (built-in or user-supplied).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub license: Option<String>,
    pub attribution: Option<String>,
    pub body: String,
    pub sha256: String,
    pub token_count: usize,
    pub source: SkillSource,
    pub status: SkillStatus,
}

/// Provenance record written to `<session_dir>/run.json` for each skill attached to a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedSkillRecord {
    pub name: String,
    pub version: String,
    pub sha256: String,
}

/// Structure of `<session_dir>/run.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSkillManifest {
    pub session_id: String,
    pub runtime: String,
    pub skills: Vec<AttachedSkillRecord>,
}

/// Computes the token count estimate (`chars / 4`) used by the `8,000`-token cap (`RD-4` §2).
pub fn estimate_tokens(body: &str) -> usize {
    body.chars().count() / 4
}

/// Parses YAML front matter (`--- ... ---`) into `(key_value_pairs, markdown_body)`.
fn split_front_matter(raw: &str) -> Result<(Vec<(String, String)>, String), String> {
    let trimmed = raw.trim_start_matches('\u{feff}');
    let Some(after_open) = trimmed.strip_prefix("---\n").or_else(|| trimmed.strip_prefix("---\r\n")) else {
        return Err("missing opening `---` front matter fence".into());
    };
    let (fm_str, body_str) = if let Some(idx) = after_open.find("\n---\n") {
        (&after_open[..idx], &after_open[idx + 5..])
    } else if let Some(idx) = after_open.find("\r\n---\r\n") {
        (&after_open[..idx], &after_open[idx + 7..])
    } else if let Some(idx) = after_open.find("\n---") {
        let rest = &after_open[idx + 4..];
        if rest.trim().is_empty() {
            (&after_open[..idx], "")
        } else {
            (&after_open[..idx], rest.trim_start_matches(['\r', '\n']))
        }
    } else {
        return Err("missing closing `---` front matter fence".into());
    };

    let mut pairs = Vec::new();
    for line in fm_str.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let Some((k, v)) = l.split_once(':') else {
            return Err(format!("malformed front matter line: {l}"));
        };
        let key = k.trim().to_string();
        let val = v.trim().trim_matches('"').trim_matches('\'').trim().to_string();
        pairs.push((key, val));
    }
    Ok((pairs, body_str.trim().to_string()))
}

fn get_fm_field(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).filter(|v| !v.is_empty())
}

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74,
    0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d,
    0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e,
    0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Computes the lowercase hex-encoded SHA-256 digest of `bytes` (FIPS 180-4).
pub fn compute_sha256_hex(bytes: &[u8]) -> String {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(bytes.len() + 72);
    padded.extend_from_slice(bytes);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) = (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(SHA256_K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for word in h {
        use std::fmt::Write;
        let _ = write!(&mut out, "{word:08x}");
    }
    out
}

/// Parses and validates a built-in skill (`assets/skills/<name>/SKILL.md`).
/// Fails if `name`, `description`, `version`, or `license` is missing.
pub fn parse_builtin_skill(id: &str, raw: &str) -> Result<SkillEntry, String> {
    let (pairs, body) = split_front_matter(raw)?;
    let name = get_fm_field(&pairs, "name").ok_or_else(|| format!("built-in skill '{id}' is missing required `name` in front matter"))?;
    let description = get_fm_field(&pairs, "description").ok_or_else(|| format!("built-in skill '{id}' is missing required `description` in front matter"))?;
    let version = get_fm_field(&pairs, "version").ok_or_else(|| format!("built-in skill '{id}' is missing required `version` in front matter"))?;
    let license = get_fm_field(&pairs, "license").ok_or_else(|| format!("built-in skill '{id}' is missing required `license` in front matter"))?;
    let attribution = get_fm_field(&pairs, "attribution");
    let sha256 = compute_sha256_hex(raw.as_bytes());
    let token_count = estimate_tokens(&body);
    Ok(SkillEntry {
        id: id.to_string(),
        name,
        description,
        version,
        license: Some(license),
        attribution,
        body,
        sha256,
        token_count,
        source: SkillSource::BuiltIn,
        status: SkillStatus::Valid,
    })
}

/// Parses a user skill file from `<library root>/skills/<name>/SKILL.md`.
/// Enforces the `64 KB` size limit and `name` + `description` front matter requirements.
pub fn parse_user_skill_bytes(id: &str, path: PathBuf, bytes: &[u8]) -> SkillEntry {
    if bytes.len() > MAX_USER_SKILL_BYTES {
        return SkillEntry {
            id: id.to_string(),
            name: id.to_string(),
            description: "Skipped because the file exceeds the 64 KB limit.".into(),
            version: "invalid".into(),
            license: None,
            attribution: None,
            body: String::new(),
            sha256: String::new(),
            token_count: 0,
            source: SkillSource::User(path),
            status: SkillStatus::TooLarge(bytes.len() as u64),
        };
    }
    let Ok(raw) = std::str::from_utf8(bytes) else {
        return SkillEntry {
            id: id.to_string(),
            name: id.to_string(),
            description: "Invalid UTF-8 content.".into(),
            version: "invalid".into(),
            license: None,
            attribution: None,
            body: String::new(),
            sha256: String::new(),
            token_count: 0,
            source: SkillSource::User(path),
            status: SkillStatus::InvalidFrontMatter("invalid UTF-8".into()),
        };
    };
    let sha256 = compute_sha256_hex(bytes);
    match split_front_matter(raw) {
        Ok((pairs, body)) => {
            let name = get_fm_field(&pairs, "name");
            let description = get_fm_field(&pairs, "description");
            match (name, description) {
                (Some(name), Some(description)) => {
                    let version = get_fm_field(&pairs, "version").unwrap_or_else(|| sha256.chars().take(12).collect());
                    let license = get_fm_field(&pairs, "license");
                    let attribution = get_fm_field(&pairs, "attribution");
                    let token_count = estimate_tokens(&body);
                    SkillEntry {
                        id: id.to_string(),
                        name,
                        description,
                        version,
                        license,
                        attribution,
                        body,
                        sha256,
                        token_count,
                        source: SkillSource::User(path),
                        status: SkillStatus::Valid,
                    }
                }
                _ => SkillEntry {
                    id: id.to_string(),
                    name: id.to_string(),
                    description: "Front matter must include `name` and `description`.".into(),
                    version: "invalid".into(),
                    license: None,
                    attribution: None,
                    body: String::new(),
                    sha256,
                    token_count: 0,
                    source: SkillSource::User(path),
                    status: SkillStatus::InvalidFrontMatter("missing `name` or `description`".into()),
                },
            }
        }
        Err(reason) => SkillEntry {
            id: id.to_string(),
            name: id.to_string(),
            description: format!("Invalid front matter ({reason})."),
            version: "invalid".into(),
            license: None,
            attribution: None,
            body: String::new(),
            sha256,
            token_count: 0,
            source: SkillSource::User(path),
            status: SkillStatus::InvalidFrontMatter(reason),
        },
    }
}

/// Loads all compiled-in skills from `BUILTIN_SKILLS_RAW`.
pub fn load_builtin_skills() -> Vec<SkillEntry> {
    BUILTIN_SKILLS_RAW.iter().map(|(id, raw)| parse_builtin_skill(id, raw).expect("built-in skill must have valid front matter and license")).collect()
}

/// Scans `<library_root>/skills/<name>/SKILL.md` in sorted directory order.
pub fn scan_user_skills(library_root: &Path) -> Vec<SkillEntry> {
    let skills_dir = library_root.join("skills");
    let Ok(rd) = std::fs::read_dir(&skills_dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.is_dir() {
                let name = e.file_name().to_string_lossy().to_string();
                let skill_md = p.join("SKILL.md");
                if skill_md.is_file() {
                    return Some((name, skill_md));
                }
            }
            None
        })
        .collect();
    dirs.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = Vec::with_capacity(dirs.len());
    for (dir_name, skill_md) in dirs {
        match std::fs::metadata(&skill_md) {
            Ok(meta) if meta.len() > MAX_USER_SKILL_BYTES as u64 => {
                out.push(SkillEntry {
                    id: dir_name.clone(),
                    name: dir_name,
                    description: "Skipped because the file exceeds the 64 KB limit.".into(),
                    version: "invalid".into(),
                    license: None,
                    attribution: None,
                    body: String::new(),
                    sha256: String::new(),
                    token_count: 0,
                    source: SkillSource::User(skill_md),
                    status: SkillStatus::TooLarge(meta.len()),
                });
            }
            Ok(_) => {
                if let Ok(bytes) = std::fs::read(&skill_md) {
                    out.push(parse_user_skill_bytes(&dir_name, skill_md, &bytes));
                }
            }
            Err(_) => {}
        }
    }
    out
}

/// Computes a lightweight fingerprint of `<library_root>/skills/*/SKILL.md` metadata for off-thread watching.
fn compute_skills_dir_signature(library_root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let skills_dir = library_root.join("skills");
    let Ok(rd) = std::fs::read_dir(&skills_dir) else {
        return 0;
    };
    let mut items: Vec<(String, u64, u128)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path().join("SKILL.md");
            let meta = std::fs::metadata(&p).ok()?;
            let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
            Some((e.file_name().to_string_lossy().to_string(), meta.len(), mtime))
        })
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    items.hash(&mut hasher);
    hasher.finish()
}

/// Runtime state for the Agent Terminal Skills Picker and attachment pipeline (`RD-4`).
pub struct SkillsAttachmentState {
    pub library_root: PathBuf,
    pub entries: Vec<SkillEntry>,
    pub selected_names: Vec<String>,
    pub picker_open: bool,
    watcher_dirty: Arc<AtomicBool>,
    watcher_stop: Arc<AtomicBool>,
}

impl Drop for SkillsAttachmentState {
    fn drop(&mut self) {
        self.watcher_stop.store(true, Ordering::Relaxed);
    }
}

impl Default for SkillsAttachmentState {
    fn default() -> Self {
        let root = crate::libraries::default_root().unwrap_or_else(designos_library::Library::default_root);
        Self::with_library_root(root)
    }
}

impl SkillsAttachmentState {
    /// Creates a `SkillsAttachmentState` bound to `library_root`, loading built-in skills,
    /// user skills from `<library_root>/skills`, and `skills.default` from `<library_root>/settings.json`.
    pub fn with_library_root(library_root: PathBuf) -> Self {
        let selected_names = app_settings::load_default_skills(&library_root);
        let watcher_dirty = Arc::new(AtomicBool::new(false));
        let watcher_stop = Arc::new(AtomicBool::new(false));
        let mut st = Self { library_root, entries: Vec::new(), selected_names, picker_open: false, watcher_dirty, watcher_stop };
        st.reload_all();
        if !cfg!(test) {
            st.spawn_background_watcher();
        }
        st
    }

    fn spawn_background_watcher(&self) {
        let root = self.library_root.clone();
        let dirty = Arc::clone(&self.watcher_dirty);
        let stop = Arc::clone(&self.watcher_stop);
        let mut last_sig = compute_skills_dir_signature(&root);
        let _ = std::thread::Builder::new().name("designos-skills-watcher".into()).spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(400));
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let sig = compute_skills_dir_signature(&root);
                if sig != last_sig {
                    last_sig = sig;
                    dirty.store(true, Ordering::Release);
                }
            }
        });
    }

    /// Re-points the skill discovery directory to `library_root` and reloads user skills and `skills.default`.
    pub fn set_library_root(&mut self, library_root: PathBuf) {
        self.watcher_stop.store(true, Ordering::Relaxed);
        self.watcher_dirty = Arc::new(AtomicBool::new(false));
        self.watcher_stop = Arc::new(AtomicBool::new(false));
        self.library_root = library_root;
        let saved = app_settings::load_default_skills(&self.library_root);
        if !saved.is_empty() {
            self.selected_names = saved;
        }
        self.reload_all();
    }

    /// Reloads both built-in and user skills immediately.
    pub fn reload_all(&mut self) {
        let mut list = load_builtin_skills();
        let mut user = scan_user_skills(&self.library_root);
        list.append(&mut user);
        self.entries = list;
    }

    /// Checks whether the off-thread watcher detected a change in `<library_root>/skills`.
    /// When true, reloads the skills list and requests a single repaint.
    pub fn poll_watcher(&mut self, ctx: Option<&egui::Context>) -> bool {
        if self.watcher_dirty.swap(false, Ordering::AcqRel) {
            self.reload_all();
            if let Some(c) = ctx {
                c.request_repaint();
            }
            return true;
        }
        false
    }

    /// Opens the skills picker and refreshes user skills from `<library_root>/skills` so newly dropped
    /// `SKILL.md` files appear without restarting Studio (`user_skill_appears_without_restart`).
    pub fn open_picker(&mut self) {
        self.reload_all();
        self.picker_open = true;
    }

    /// Toggles selection of `skill_name` and persists `skills.default` to `settings.json`.
    pub fn toggle_skill(&mut self, skill_name: &str) {
        let can_select = self.entries.iter().any(|e| e.name == skill_name && e.status.is_valid());
        if let Some(pos) = self.selected_names.iter().position(|n| n == skill_name) {
            self.selected_names.remove(pos);
        } else if can_select {
            self.selected_names.push(skill_name.to_string());
        }
        let _ = app_settings::save_default_skills(&self.library_root, &self.selected_names);
    }

    /// Explicitly sets the selected skill names and persists them to `settings.json`.
    pub fn set_selected_skills(&mut self, names: Vec<String>) {
        self.selected_names = names;
        let _ = app_settings::save_default_skills(&self.library_root, &self.selected_names);
    }

    /// Returns all currently selected valid [`SkillEntry`] items in selection order.
    pub fn selected_entries(&self) -> Vec<&SkillEntry> {
        self.selected_names.iter().filter_map(|name| self.entries.iter().find(|e| &e.name == name && e.status.is_valid())).collect()
    }

    /// Total estimated tokens (`chars / 4`) across all selected valid skills.
    pub fn combined_tokens(&self) -> usize {
        self.selected_entries().iter().map(|e| e.token_count).sum()
    }

    /// Returns an explanation string when the current selection violates the `<= 3` skills or
    /// `<= 8,000` tokens limit, or `None` when the run is allowed to start.
    pub fn limit_violation(&self) -> Option<String> {
        let selected = self.selected_entries();
        if selected.len() > MAX_SKILLS_PER_RUN {
            return Some(format!(
                "Limit exceeded: {}/{} skills selected (uncheck {} to start)",
                selected.len(),
                MAX_SKILLS_PER_RUN,
                selected.len() - MAX_SKILLS_PER_RUN
            ));
        }
        let tokens = self.combined_tokens();
        if tokens > MAX_COMBINED_TOKENS {
            return Some(format!("Limit exceeded: {tokens} / {MAX_COMBINED_TOKENS} tokens across selected skills"));
        }
        None
    }

    /// Returns `true` when the selected skills are within both the count (`<= 3`) and token (`<= 8,000`) caps.
    pub fn can_start_run(&self) -> bool {
        self.limit_violation().is_none()
    }

    /// Formats the selected skills as fenced `designos-skill:<name>@<version>` blocks.
    pub fn format_fenced_blocks(&self) -> String {
        let selected = self.selected_entries();
        if selected.is_empty() || !self.can_start_run() {
            return String::new();
        }
        selected.into_iter().map(|e| format!("```designos-skill:{}@{}\n{}\n```", e.name, e.version, e.body.trim())).collect::<Vec<_>>().join("\n\n")
    }

    /// Attaches the selected skills to `prompt` for `runtime`:
    /// - When skills are selected and within limits, prepends the fenced `designos-skill:<name>@<version>`
    ///   blocks to `prompt` so every CLI (`Claude`, `Codex`, `Antigravity`, `Grok`, and stub test processes)
    ///   receives the exact skill body (`chosen_skill_reaches_agent`).
    /// - Claude Code also receives the attached skills via `UserPromptSubmit` (`page-context-reply.json`).
    pub fn attach_to_prompt(&self, _runtime: AgentCliPreset, prompt: &str) -> String {
        let fenced = self.format_fenced_blocks();
        if fenced.is_empty() {
            prompt.to_string()
        } else {
            format!("{fenced}\n\n{prompt}")
        }
    }

    /// Builds the `AttachedSkillRecord` list (`{name, version, sha256}`) for `<session_dir>/run.json`.
    pub fn attached_records(&self) -> Vec<AttachedSkillRecord> {
        if !self.can_start_run() {
            return Vec::new();
        }
        self.selected_entries()
            .into_iter()
            .map(|e| AttachedSkillRecord { name: e.name.clone(), version: e.version.clone(), sha256: e.sha256.clone() })
            .collect()
    }

    /// Writes `<session_dir>/run.json` containing the session ID, runtime, and `{name, version, sha256}`
    /// for each attached skill (`run_records_versions`).
    pub fn write_run_record(&self, session_dir: &Path, session_id: &str, runtime: AgentCliPreset) -> Result<PathBuf, String> {
        let _ = std::fs::create_dir_all(session_dir);
        let manifest = RunSkillManifest { session_id: session_id.to_string(), runtime: runtime.binary_name().to_string(), skills: self.attached_records() };
        let run_path = session_dir.join("run.json");
        let bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
        designos_library::write_atomic(&run_path, &bytes).map_err(|e| format!("write {}: {e}", run_path.display()))?;
        Ok(run_path)
    }

    /// Renders the compact `Skills (N/3)` chip button and returns `true` if clicked.
    pub fn paint_chip(&mut self, ui: &mut Ui, p: &egui::Painter, rect: Rect, id_str: &str) -> bool {
        let resp = ui.interact(rect, Id::new(id_str), Sense::click());
        let selected_count = self.selected_entries().len();
        let has_violation = !self.can_start_run();
        let (bg, border, fg) = if has_violation {
            (STATUS_BAD_BG, STATUS_BAD_BORDER, STATUS_BAD_TEXT)
        } else if self.picker_open || selected_count > 0 {
            (STATUS_INFO_BG, STATUS_INFO_BORDER, STATUS_INFO_TEXT)
        } else if resp.hovered() {
            (TERM_BG_HOVER, TERM_BORDER, Color32::WHITE)
        } else {
            (Color32::from_rgb(0x28, 0x28, 0x2C), Color32::from_rgb(0x3C, 0x3C, 0x42), Color32::from_rgb(0xB3, 0xB3, 0xB3))
        };
        p.rect_filled(rect, theme::cr(theme::RADIUS_CONTROL), bg);
        p.rect_stroke(rect, theme::cr(theme::RADIUS_CONTROL), Stroke::new(theme::BORDER_WIDTH, border), StrokeKind::Inside);
        let label = if selected_count > 0 { format!("Skills ({selected_count}/{MAX_SKILLS_PER_RUN})") } else { "Skills".to_string() };
        let elided = widgets::elide(p, &label, &theme::font_caption_strong(), (rect.width() - theme::SPACE_INLINE).max(theme::TARGET_MIN));
        p.text(rect.center(), Align2::CENTER_CENTER, elided, theme::font_caption_strong(), fg);
        widgets::record(ui, id_str, rect);
        if resp.clicked() {
            if self.picker_open {
                self.picker_open = false;
            } else {
                self.open_picker();
            }
            return true;
        }
        false
    }

    /// Renders the floating Skills Picker popover inside `panel_rect`.
    pub fn show_picker_popover(&mut self, ui: &mut Ui, panel_rect: Rect, anchor_bottom_y: f32) {
        if !self.picker_open {
            return;
        }
        self.poll_watcher(Some(ui.ctx()));

        let min_pop_w = theme::CONTROL_ROW * 5.0 + theme::SPACE_SECTION;
        let max_pop_w = theme::CONTROL_ROW * 8.0;
        let pop_w = (panel_rect.width() - theme::INSET_PANEL).clamp(min_pop_w, max_pop_w);
        let row_h = theme::CONTROL_ROW + theme::SPACE_ROW;
        let header_h = theme::CONTROL_STANDARD;
        let footer_h = if self.limit_violation().is_some() { theme::CONTROL_ROW } else { theme::CONTROL_DENSE };
        let min_pop_h = theme::CONTROL_ROW * 4.0 + theme::SPACE_SECTION;
        let pop_h =
            (header_h + (self.entries.len() as f32) * row_h + footer_h + theme::SPACE_INLINE).min((panel_rect.height() - theme::SPACE_SECTION).max(min_pop_h));
        let pop_x = (panel_rect.left() + theme::SPACE_INLINE).min(panel_rect.right() - pop_w - theme::SPACE_ROW);
        let pop_y = (anchor_bottom_y - pop_h - theme::SPACE_INLINE).max(panel_rect.top() + theme::CONTROL_ROW);
        let pop_r = Rect::from_min_size(Pos2::new(pop_x, pop_y), Vec2::new(pop_w, pop_h));

        let p = ui.painter().clone();
        p.rect_filled(pop_r, theme::cr(theme::RADIUS_FRAME), Color32::from_rgb(0x1F, 0x20, 0x24));
        p.rect_stroke(pop_r, theme::cr(theme::RADIUS_FRAME), Stroke::new(theme::BORDER_WIDTH, Color32::from_rgb(0x3E, 0x3E, 0x46)), StrokeKind::Inside);
        widgets::record(ui, "agents-skills-picker-popover", pop_r);

        // Header
        let selected_count = self.selected_entries().len();
        let tokens = self.combined_tokens();
        let header_text = format!("ATTACH SKILLS ({selected_count}/{MAX_SKILLS_PER_RUN} · {tokens}/{MAX_COMBINED_TOKENS} tok)");
        let header_elided = widgets::elide(&p, &header_text, &theme::font_caption_strong(), (pop_w - theme::CONTROL_ROW).max(theme::SPACE_REGION));
        p.text(
            Pos2::new(pop_r.left() + theme::INSET_BUTTON, pop_r.top() + theme::INSET_PANEL),
            Align2::LEFT_CENTER,
            header_elided,
            theme::font_caption_strong(),
            Color32::WHITE,
        );

        let close_r = Rect::from_center_size(Pos2::new(pop_r.right() - theme::INSET_PANEL, pop_r.top() + theme::INSET_PANEL), Vec2::splat(theme::TARGET_MIN));
        let close_resp = ui.interact(close_r, Id::new("agents-skills-picker-close"), Sense::click());
        icons::paint(&p, Icon::Close, close_r.center(), theme::SPACE_GROUP, if close_resp.hovered() { STATUS_BAD_TEXT } else { theme::TEXT_SECONDARY });
        widgets::record(ui, "agents-skills-picker-close", close_r);
        if close_resp.clicked() {
            self.picker_open = false;
            return;
        }
        widgets::hline(&p, pop_r.left() + theme::SPACE_INLINE, pop_r.right() - theme::SPACE_INLINE, pop_r.top() + header_h, TERM_BORDER);

        // Rows
        let mut toggle_target: Option<String> = None;
        let mut ry = pop_r.top() + header_h + theme::SPACE_ROW;
        let max_rows_bottom = pop_r.bottom() - footer_h - theme::INSET_SEGMENT;

        for entry in &self.entries {
            if ry + row_h - theme::SPACE_ROW > max_rows_bottom {
                break;
            }
            let row_r = Rect::from_min_size(Pos2::new(pop_r.left() + theme::INSET_FRAME, ry), Vec2::new(pop_w - theme::INSET_PANEL, row_h - theme::SPACE_ROW));
            let is_valid = entry.status.is_valid();
            let is_selected = is_valid && self.selected_names.iter().any(|n| n == &entry.name);
            let row_id = format!("agents-skill-row-{}", entry.id);
            let resp = ui.interact(row_r, Id::new(&row_id), Sense::click());

            let row_bg = if !is_valid {
                STATUS_BAD_BG.gamma_multiply(0.45)
            } else if is_selected {
                STATUS_INFO_BG.gamma_multiply(0.75)
            } else if resp.hovered() {
                TERM_BG_HOVER
            } else {
                TERM_BG_ELEVATED
            };
            let row_border = if !is_valid {
                STATUS_BAD_BORDER
            } else if is_selected {
                STATUS_INFO_BORDER
            } else {
                TERM_BORDER
            };
            p.rect_filled(row_r, theme::cr(theme::RADIUS_CONTROL), row_bg);
            p.rect_stroke(row_r, theme::cr(theme::RADIUS_CONTROL), Stroke::new(theme::BORDER_WIDTH, row_border), StrokeKind::Inside);

            // Checkbox
            let cb_r = Rect::from_center_size(Pos2::new(row_r.left() + theme::INSET_BUTTON, row_r.top() + theme::SPACE_GROUP), Vec2::splat(theme::SPACE_GROUP));
            let cb_bg = if is_selected { STATUS_INFO_TEXT } else { Color32::from_rgb(0x16, 0x17, 0x1B) };
            p.rect_filled(cb_r, theme::cr(theme::RADIUS_SEGMENT), cb_bg);
            p.rect_stroke(
                cb_r,
                theme::cr(theme::RADIUS_SEGMENT),
                Stroke::new(theme::BORDER_WIDTH, if is_selected { STATUS_INFO_TEXT } else { TERM_BORDER }),
                StrokeKind::Inside,
            );
            if is_selected {
                icons::paint(&p, Icon::Check, cb_r.center(), theme::SPACE_INLINE, Color32::from_rgb(0x0E, 0x0E, 0x12));
            }

            // Title + Version
            let title_str = if is_valid { format!("{} @{}", entry.name, entry.version) } else { entry.name.clone() };
            let title_elided =
                widgets::elide(&p, &title_str, &theme::font_caption_strong(), (row_r.width() - theme::SPACE_REGION * 2.0).max(theme::CONTROL_ROW));
            p.text(
                Pos2::new(row_r.left() + theme::SPACE_SECTION, row_r.top() + theme::SPACE_GROUP),
                Align2::LEFT_CENTER,
                title_elided,
                theme::font_caption_strong(),
                if is_valid { Color32::WHITE } else { STATUS_BAD_TEXT },
            );

            // Right pill: error badge or token count + source
            if let Some(err_label) = entry.status.error_badge() {
                let badge_w = (err_label.len() as f32 * theme::SPACE_ROW + theme::SPACE_GROUP).min(theme::CONTROL_ROW * 3.0);
                let badge_r = Rect::from_min_size(
                    Pos2::new(row_r.right() - badge_w - theme::INSET_SEGMENT, row_r.top() + theme::INSET_SEGMENT),
                    Vec2::new(badge_w, theme::INSET_PANEL),
                );
                p.rect_filled(badge_r, theme::cr(theme::RADIUS_SEGMENT), STATUS_BAD_BG);
                p.rect_stroke(badge_r, theme::cr(theme::RADIUS_SEGMENT), Stroke::new(theme::BORDER_WIDTH, STATUS_BAD_BORDER), StrokeKind::Inside);
                let err_elided = widgets::elide(&p, err_label, &theme::font_caption(), badge_w - theme::SPACE_INLINE);
                p.text(badge_r.center(), Align2::CENTER_CENTER, err_elided, theme::font_caption(), STATUS_BAD_TEXT);
            } else {
                let meta_str = format!("{} · {}t", entry.source.badge_label(), entry.token_count);
                p.text(
                    Pos2::new(row_r.right() - theme::SPACE_INLINE, row_r.top() + theme::SPACE_GROUP),
                    Align2::RIGHT_CENTER,
                    meta_str,
                    theme::font_caption(),
                    if is_selected { STATUS_INFO_TEXT } else { theme::TEXT_SECONDARY },
                );
            }

            // Description line
            let desc_elided = widgets::elide(&p, &entry.description, &theme::font_caption(), (row_r.width() - theme::CONTROL_TOOL).max(theme::CONTROL_ROW));
            p.text(
                Pos2::new(row_r.left() + theme::SPACE_SECTION, row_r.top() + theme::CONTROL_STANDARD),
                Align2::LEFT_CENTER,
                desc_elided,
                theme::font_caption(),
                theme::TEXT_SECONDARY,
            );

            widgets::record(ui, &row_id, row_r);
            if is_valid && resp.clicked() {
                toggle_target = Some(entry.name.clone());
            }
            ry += row_h;
        }

        if let Some(name) = toggle_target {
            self.toggle_skill(&name);
        }

        // Footer: limit notice or user skills directory hint
        widgets::hline(&p, pop_r.left() + theme::SPACE_INLINE, pop_r.right() - theme::SPACE_INLINE, pop_r.bottom() - footer_h, TERM_BORDER);
        if let Some(violation) = self.limit_violation() {
            let warn_r = Rect::from_min_max(
                Pos2::new(pop_r.left() + theme::INSET_FRAME, pop_r.bottom() - footer_h + theme::SPACE_ROW),
                Pos2::new(pop_r.right() - theme::INSET_FRAME, pop_r.bottom() - theme::SPACE_ROW),
            );
            p.rect_filled(warn_r, theme::cr(theme::RADIUS_CONTROL), STATUS_WARN_BG);
            p.rect_stroke(warn_r, theme::cr(theme::RADIUS_CONTROL), Stroke::new(theme::BORDER_WIDTH, STATUS_WARN_BORDER), StrokeKind::Inside);
            let msg = widgets::elide(&p, &violation, &theme::font_caption_strong(), warn_r.width() - theme::SPACE_GROUP);
            p.text(warn_r.center(), Align2::CENTER_CENTER, msg, theme::font_caption_strong(), STATUS_WARN_TEXT);
            widgets::record(ui, "agents-skills-limit-warning", warn_r);
        } else {
            let footer_msg = format!("User skills: {}/skills/<name>/SKILL.md", self.library_root.display());
            let footer_elided = widgets::elide(&p, &footer_msg, &theme::font_caption(), pop_w - theme::SPACE_SECTION);
            p.text(
                Pos2::new(pop_r.left() + theme::INSET_BUTTON, pop_r.bottom() - theme::SPACE_GROUP),
                Align2::LEFT_CENTER,
                footer_elided,
                theme::font_caption(),
                STATUS_OK_TEXT,
            );
            let ok_r = Rect::from_min_size(
                Pos2::new(pop_r.right() - theme::SPACE_REGION - theme::SPACE_INLINE, pop_r.bottom() - theme::SPACE_SECTION),
                Vec2::new(theme::SPACE_REGION, theme::INSET_PANEL),
            );
            p.rect_filled(ok_r, theme::cr(theme::RADIUS_SEGMENT), STATUS_OK_BG);
            p.rect_stroke(ok_r, theme::cr(theme::RADIUS_SEGMENT), Stroke::new(theme::BORDER_WIDTH, STATUS_OK_BORDER), StrokeKind::Inside);
            p.text(ok_r.center(), Align2::CENTER_CENTER, "READY", theme::font_caption_strong(), STATUS_OK_TEXT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_lib_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("designos_skills_test_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn chosen_skill_reaches_agent() {
        let root = temp_lib_root("reaches_agent");
        let mut state = SkillsAttachmentState::with_library_root(root.clone());
        state.toggle_skill("design-system-rhythm");
        assert_eq!(state.selected_entries().len(), 1);

        let attached_prompt = state.attach_to_prompt(AgentCliPreset::Codex, "Create a pricing card");
        assert!(attached_prompt.contains("```designos-skill:design-system-rhythm@0.6.0"));
        assert!(attached_prompt.contains("4px Spatial Grid"));
        assert!(attached_prompt.ends_with("Create a pricing card"));

        // Verify a stub agent shell process receives the attached skill body on stdin/args.
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn stub agent");
        {
            use std::io::Write;
            let stdin = child.stdin.as_mut().unwrap();
            stdin.write_all(attached_prompt.as_bytes()).unwrap();
        }
        let out = child.wait_with_output().expect("wait stub agent");
        let received = String::from_utf8_lossy(&out.stdout);
        assert!(received.contains("designos-skill:design-system-rhythm@0.6.0"));
        assert!(received.contains("4px Spatial Grid"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn user_skill_appears_without_restart() {
        let root = temp_lib_root("hot_appear");
        let mut state = SkillsAttachmentState::with_library_root(root.clone());
        assert!(!state.entries.iter().any(|e| e.name == "foo"));

        let foo_dir = root.join("skills").join("foo");
        std::fs::create_dir_all(&foo_dir).unwrap();
        std::fs::write(
            foo_dir.join("SKILL.md"),
            "---\nname: foo\ndescription: Custom user skill for layout checks\n---\n\nAlways align cards to a 12-column grid.\n",
        )
        .unwrap();

        // Opening the picker discovers `<library>/skills/foo/SKILL.md` without restarting.
        state.open_picker();
        let foo = state.entries.iter().find(|e| e.name == "foo").expect("foo skill discovered on picker open");
        assert!(foo.status.is_valid());
        assert_eq!(foo.version.len(), 12, "user skill without explicit version defaults to 12-char content hash");

        state.toggle_skill("foo");
        assert!(state.selected_entries().iter().any(|e| e.name == "foo"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn works_offline() {
        let root = temp_lib_root("offline");
        let custom_dir = root.join("skills").join("offline-helper");
        std::fs::create_dir_all(&custom_dir).unwrap();
        std::fs::write(custom_dir.join("SKILL.md"), "---\nname: offline-helper\ndescription: Works 100% offline\nversion: 1.2.0\n---\n\nOffline skill body.\n")
            .unwrap();

        // Even with all proxy variables pointed at an invalid port (no network access),
        // built-in and user skills load, persist, and attach with zero network calls.
        let mut state = SkillsAttachmentState::with_library_root(root.clone());
        state.toggle_skill("design-system-rhythm");
        state.toggle_skill("offline-helper");
        assert!(state.can_start_run());
        let fenced = state.format_fenced_blocks();
        assert!(fenced.contains("designos-skill:design-system-rhythm@0.6.0"));
        assert!(fenced.contains("designos-skill:offline-helper@1.2.0"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn builtin_without_license_fails_build() {
        // Every shipped built-in skill must pass `parse_builtin_skill` (which requires `license`).
        for (id, raw) in BUILTIN_SKILLS_RAW {
            let parsed = parse_builtin_skill(id, raw).expect("shipped built-in skill must have valid front matter");
            assert!(parsed.license.as_deref().is_some_and(|l| !l.is_empty()));
        }

        // A built-in skill missing `license` in front matter must fail validation.
        let missing_license = "---\nname: bad-builtin\ndescription: Missing licence\nversion: 0.1.0\n---\n\nBody.\n";
        let err = parse_builtin_skill("bad-builtin", missing_license).unwrap_err();
        assert!(err.contains("license"), "expected missing license error, got: {err}");
    }

    #[test]
    fn size_limit_disables_start() {
        let root = temp_lib_root("size_limit");
        let huge_dir = root.join("skills").join("huge-tokens");
        std::fs::create_dir_all(&huge_dir).unwrap();
        // 36,000 chars -> 9,000 tokens (> 8,000 tokens), while still < 64 KB (36 KB).
        let body = "a".repeat(36_000);
        let content = format!("---\nname: huge-tokens\ndescription: Over 8000 tokens\nversion: 1.0.0\n---\n\n{body}\n");
        assert!(content.len() < MAX_USER_SKILL_BYTES);
        std::fs::write(huge_dir.join("SKILL.md"), content).unwrap();

        let mut state = SkillsAttachmentState::with_library_root(root.clone());
        state.toggle_skill("huge-tokens");
        assert!(state.combined_tokens() > MAX_COMBINED_TOKENS);
        assert!(!state.can_start_run(), "exceeding 8,000 tokens must disable run start");
        assert!(state.limit_violation().unwrap().contains("8000"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_records_versions() {
        let root = temp_lib_root("run_records");
        let session_dir = root.join("session");
        let mut state = SkillsAttachmentState::with_library_root(root.clone());
        state.toggle_skill("design-system-rhythm");
        state.toggle_skill("accessibility-wcag");

        let run_path = state.write_run_record(&session_dir, "test-session-1", AgentCliPreset::Claude).unwrap();
        let raw = std::fs::read_to_string(&run_path).unwrap();
        let parsed: RunSkillManifest = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.session_id, "test-session-1");
        assert_eq!(parsed.runtime, "claude");
        assert_eq!(parsed.skills.len(), 2);
        assert_eq!(parsed.skills[0].name, "design-system-rhythm");
        assert_eq!(parsed.skills[0].version, "0.6.0");
        assert_eq!(parsed.skills[0].sha256.len(), 64);
        assert_eq!(parsed.skills[1].name, "accessibility-wcag");
        assert_eq!(parsed.skills[1].version, "0.6.0");
        assert_eq!(parsed.skills[1].sha256.len(), 64);
        let _ = std::fs::remove_dir_all(&root);
    }
}
