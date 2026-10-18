//! Variable, style, mode, and deprecated EaseUI token resolution for `apply_spec` / `export_spec` (E15-3).
#![allow(clippy::result_large_err, clippy::type_complexity, clippy::too_many_arguments)]

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Reserved collection ID for the auto-materialized deprecated EaseUI token collection.
pub const EASEUI_DEPRECATED_COLLECTION_ID: &str = "easeui-deprecated";
/// Default mode ID inside the `easeui-deprecated` variable collection.
pub const EASEUI_DEPRECATED_MODE_ID: &str = "default";

/// Canonical EaseUI BrainWave 2.0 colour tokens (`(canonical_var_name, (r, g, b), aliases)`).
pub const EASEUI_CANONICAL_TOKENS: &[(&str, (u8, u8, u8), &[&str])] = &[
    ("bg/canvas", (0x12, 0x12, 0x15), &["$easeui.bg.canvas", "$easeui.bg"]),
    ("bg/panel", (0x18, 0x18, 0x1B), &["$easeui.bg.panel", "$easeui.panel"]),
    ("bg/surface", (0x27, 0x27, 0x2A), &["$easeui.bg.surface", "$easeui.surface"]),
    ("bg/elevated", (0x30, 0x30, 0x36), &["$easeui.bg.elevated", "$easeui.elevated"]),
    ("border", (0x27, 0x27, 0x2A), &["$easeui.border"]),
    ("border/subtle", (0x3F, 0x3F, 0x46), &["$easeui.border.subtle"]),
    ("text/primary", (0xF4, 0xF4, 0xF5), &["$easeui.text.primary", "$easeui.text"]),
    ("text/secondary", (0xA1, 0xA1, 0xAA), &["$easeui.text.secondary"]),
    ("text/muted", (0x71, 0x71, 0x7A), &["$easeui.text.muted"]),
    ("accent/orange", (0xE3, 0x63, 0x23), &["$easeui.accent.orange", "$easeui.brand", "$easeui.primary"]),
    ("accent/blue", (0x35, 0x82, 0xFF), &["$easeui.accent.blue", "$easeui.selection"]),
    ("accent/violet", (0x87, 0x55, 0xE9), &["$easeui.accent.violet", "$easeui.component"]),
    ("accent/emerald", (0x18, 0xB3, 0x68), &["$easeui.accent.emerald", "$easeui.status.success", "$easeui.success"]),
    ("status/warning", (0xF5, 0x9E, 0x0B), &["$easeui.status.warning", "$easeui.warning"]),
    ("status/danger", (0xEF, 0x44, 0x44), &["$easeui.status.danger", "$easeui.danger"]),
    ("light/bg", (0xFA, 0xFA, 0xFA), &["$easeui.light.bg"]),
    ("light/surface", (0xFF, 0xFF, 0xFF), &["$easeui.light.surface"]),
    ("light/border", (0xE4, 0xE4, 0xE7), &["$easeui.light.border"]),
    ("light/text", (0x18, 0x18, 0x1B), &["$easeui.light.text"]),
];

/// A numeric spec field that accepts either a literal `f32` or a `"var:..."` string reference.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumberOrVar {
    Num(f32),
    Ref(String),
}

impl Default for NumberOrVar {
    fn default() -> Self {
        NumberOrVar::Num(0.0)
    }
}

impl From<f32> for NumberOrVar {
    fn from(v: f32) -> Self {
        NumberOrVar::Num(v)
    }
}

impl From<&str> for NumberOrVar {
    fn from(s: &str) -> Self {
        NumberOrVar::Ref(s.to_string())
    }
}

impl From<String> for NumberOrVar {
    fn from(s: String) -> Self {
        NumberOrVar::Ref(s)
    }
}

impl PartialEq<f32> for NumberOrVar {
    fn eq(&self, other: &f32) -> bool {
        matches!(self, NumberOrVar::Num(v) if (*v - *other).abs() < 1e-6)
    }
}

impl NumberOrVar {
    pub fn as_num(&self) -> Option<f32> {
        match self {
            NumberOrVar::Num(v) => Some(*v),
            NumberOrVar::Ref(_) => None,
        }
    }

    pub fn as_ref_str(&self) -> Option<&str> {
        match self {
            NumberOrVar::Num(_) => None,
            NumberOrVar::Ref(s) => Some(s.as_str()),
        }
    }

    pub fn unwrap_or(&self, default: f32) -> f32 {
        self.as_num().unwrap_or(default)
    }

    pub fn is_zero(&self) -> bool {
        matches!(self, NumberOrVar::Num(v) if *v == 0.0)
    }

    pub fn is_one(&self) -> bool {
        matches!(self, NumberOrVar::Num(v) if (*v - 1.0).abs() < 1e-6)
    }
}

/// Non-fatal warning emitted during `apply_spec_with_report`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecWarning {
    pub code: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Structured report returned by [`Document::apply_spec_with_report`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecApplyReport {
    pub root_node_id: NodeId,
    pub bindings_created: usize,
    pub styles_linked: usize,
    pub warnings: Vec<SpecWarning>,
}

/// Structured error envelope produced when a spec variable, style, mode, or colour fails validation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpecBindError {
    pub code: String,
    pub message: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub valid_options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrected_example: Option<serde_json::Value>,
    pub safe_to_retry: bool,
}

impl std::fmt::Display for SpecBindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.valid_options.is_empty() {
            write!(f, "{}: {} (at {})", self.code, self.message, self.path)
        } else {
            write!(f, "{}: {} (at {}; valid_options: {})", self.code, self.message, self.path, self.valid_options.join(", "))
        }
    }
}

impl std::error::Error for SpecBindError {}

/// Outcome of resolving a colour string (`fill`, `stroke`, or `text_color`).
#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedColourRef {
    /// `"none"` or `"transparent"` — clears the paint list.
    NoneOrTransparent,
    /// Raw hex colour `#RRGGBB` / `#RRGGBBAA`.
    Literal(Color),
    /// Bound variable (`var:...` or `$easeui.*` alias) with its resolved colour in the node's mode.
    Variable { var_id: String, color: Color },
    /// Linked shared Fill style (`style:...`) with its paints.
    Style { style_key: String, paints: Vec<Paint> },
}

/// Fast O(variables + styles) lookup index built once per `apply_spec` call.
pub struct SpecBindIndex {
    /// `(collection_name, var_name)` -> list of `(var_id, coll_name)` in collection order.
    by_coll_and_name: HashMap<(String, String), Vec<(String, String)>>,
    /// `var_name` -> list of `(var_id, coll_name)` in collection order.
    by_name: HashMap<String, Vec<(String, String)>>,
    /// Ordered list of `(coll_idx, var_name, full_path, var_id, resolved_kind)` for deterministic Levenshtein ranking.
    all_vars: Vec<IndexedVar>,
    /// `style_name` -> list of `style_key`.
    styles_by_name: HashMap<String, Vec<String>>,
    /// Ordered list of `(style_key, style_name, property)`.
    all_styles: Vec<(String, String, StyleProperty)>,
}

#[derive(Clone, Debug)]
struct IndexedVar {
    coll_idx: usize,
    var_name: String,
    full_path: String,
    var_id: String,
    resolved_kind: VariableKind,
}

impl SpecBindIndex {
    /// Builds the lookup index in a single pass over `doc.variable_collections`, `doc.variables`, and `doc.styles`.
    pub fn build(doc: &Document) -> Self {
        let mut coll_order: HashMap<&str, (usize, &str)> = HashMap::with_capacity(doc.variable_collections.len());
        for (idx, (cid, coll)) in doc.variable_collections.iter().enumerate() {
            if cid != EASEUI_DEPRECATED_COLLECTION_ID {
                coll_order.insert(cid.as_str(), (idx, coll.name.as_str()));
            }
        }
        let next_idx = doc.variable_collections.len();
        if let Some(coll) = doc.variable_collections.get(EASEUI_DEPRECATED_COLLECTION_ID) {
            coll_order.insert(EASEUI_DEPRECATED_COLLECTION_ID, (next_idx, coll.name.as_str()));
        }

        let mut all_vars: Vec<IndexedVar> = Vec::with_capacity(doc.variables.len());
        for (vid, var) in &doc.variables {
            let Some(&(c_idx, c_name)) = coll_order.get(var.collection.as_str()) else {
                continue;
            };
            let resolved_kind = resolve_target_variable(doc, vid).map(|v| v.kind).unwrap_or(var.kind);
            all_vars.push(IndexedVar {
                coll_idx: c_idx,
                var_name: var.name.clone(),
                full_path: format!("{c_name}/{}", var.name),
                var_id: vid.clone(),
                resolved_kind,
            });
        }
        all_vars.sort_by(|a, b| (a.coll_idx, &a.var_name, &a.var_id).cmp(&(b.coll_idx, &b.var_name, &b.var_id)));

        let mut by_coll_and_name: HashMap<(String, String), Vec<(String, String)>> = HashMap::with_capacity(all_vars.len());
        let mut by_name: HashMap<String, Vec<(String, String)>> = HashMap::with_capacity(all_vars.len());
        for iv in &all_vars {
            let c_name = doc.variables.get(&iv.var_id).and_then(|v| doc.variable_collections.get(&v.collection)).map(|c| c.name.clone()).unwrap_or_default();
            by_coll_and_name.entry((c_name.clone(), iv.var_name.clone())).or_default().push((iv.var_id.clone(), c_name.clone()));
            by_name.entry(iv.var_name.clone()).or_default().push((iv.var_id.clone(), c_name));
        }

        let mut all_styles = Vec::with_capacity(doc.styles.len());
        let mut styles_by_name: HashMap<String, Vec<String>> = HashMap::with_capacity(doc.styles.len());
        for key in doc.styles.keys() {
            let name = doc.style_meta.get(key).map(|m| m.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| key.clone());
            let prop = crate::style_property(doc, key);
            styles_by_name.entry(name.clone()).or_default().push(key.clone());
            all_styles.push((key.clone(), name, prop));
        }
        all_styles.sort_by(|a, b| (&a.1, &a.0).cmp(&(&b.1, &b.0)));

        Self { by_coll_and_name, by_name, all_vars, styles_by_name, all_styles }
    }

    /// Registers the 19 `easeui-deprecated` tokens in the index the first time the collection is materialized.
    pub fn ensure_easeui_indexed(&mut self, doc: &Document) {
        let Some(coll) = doc.variable_collections.get(EASEUI_DEPRECATED_COLLECTION_ID) else {
            return;
        };
        let first_vid = format!("{EASEUI_DEPRECATED_COLLECTION_ID}/{}", EASEUI_CANONICAL_TOKENS[0].0);
        if self.all_vars.iter().any(|iv| iv.var_id == first_vid) {
            return;
        }
        let c_idx = doc.variable_collections.len();
        let c_name = coll.name.clone();
        for &(canonical, _, _) in EASEUI_CANONICAL_TOKENS {
            let vid = format!("{EASEUI_DEPRECATED_COLLECTION_ID}/{canonical}");
            let var_name = canonical.to_string();
            self.by_coll_and_name.entry((c_name.clone(), var_name.clone())).or_default().push((vid.clone(), c_name.clone()));
            self.by_name.entry(var_name.clone()).or_default().push((vid.clone(), c_name.clone()));
            self.all_vars.push(IndexedVar {
                coll_idx: c_idx,
                var_name,
                full_path: format!("{c_name}/{canonical}"),
                var_id: vid,
                resolved_kind: VariableKind::Color,
            });
        }
    }

    fn closest_variables(&self, query: &str, limit: usize) -> Vec<String> {
        let mut scored: Vec<(usize, usize, &str, &str)> = self
            .all_vars
            .iter()
            .filter(|iv| !iv.var_id.starts_with("easeui-deprecated/"))
            .map(|iv| {
                let d_full = levenshtein(query, &iv.full_path);
                let d_name = levenshtein(query, &iv.var_name);
                let dist = d_full.min(d_name);
                (dist, iv.coll_idx, iv.var_name.as_str(), iv.full_path.as_str())
            })
            .collect();
        scored.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
        scored.into_iter().take(limit).map(|(_, _, _, full)| full.to_string()).collect()
    }

    fn variables_of_kind(&self, kind: VariableKind, limit: usize) -> Vec<String> {
        self.all_vars
            .iter()
            .filter(|iv| !iv.var_id.starts_with("easeui-deprecated/") && iv.resolved_kind == kind)
            .take(limit)
            .map(|iv| format!("var:{}", iv.full_path))
            .collect()
    }

    fn in_scope_variables(&self, doc: &Document, kind: VariableKind, prop_key: &str, node_kind: &NodeKind, limit: usize) -> Vec<String> {
        self.all_vars
            .iter()
            .filter(|iv| {
                if iv.var_id.starts_with("easeui-deprecated/") || iv.resolved_kind != kind {
                    return false;
                }
                let Some(target) = resolve_target_variable(doc, &iv.var_id) else {
                    return false;
                };
                crate::scope_allows(&target.scopes, prop_key, node_kind)
            })
            .take(limit)
            .map(|iv| format!("var:{}", iv.full_path))
            .collect()
    }

    fn closest_styles(&self, query: &str, expected_prop: Option<StyleProperty>, limit: usize) -> Vec<String> {
        let mut scored: Vec<(usize, &str)> = self
            .all_styles
            .iter()
            .filter(|(_, _, prop)| expected_prop.is_none_or(|ep| *prop == ep))
            .map(|(_, name, _)| (levenshtein(query, name), name.as_str()))
            .collect();
        scored.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        scored.into_iter().take(limit).map(|(_, name)| name.to_string()).collect()
    }
}

/// Standard Levenshtein edit distance on Unicode scalar values.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];
    for (i, &ca) in a_chars.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b_chars.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

/// Follows alias chains up to 16 hops to find the terminal target [`Variable`] for kind and scope checks.
pub fn resolve_target_variable<'a>(doc: &'a Document, var_id: &str) -> Option<&'a Variable> {
    let mut cur_id = var_id;
    for _ in 0..16 {
        let v = doc.variables.get(cur_id)?;
        let coll = doc.variable_collections.get(&v.collection);
        let default_mode = coll.map(|c| c.default_mode.as_str()).unwrap_or("");
        let sample_val = v.values.get(default_mode).or_else(|| v.values.values().next());
        match sample_val {
            Some(VariableValue::Alias(next_id)) if doc.variables.contains_key(next_id) => {
                cur_id = next_id.as_str();
            }
            _ => return Some(v),
        }
    }
    doc.variables.get(var_id)
}

/// Maps any `$easeui.*` token spelling (including short aliases) to its canonical variable name and RGB value.
pub fn lookup_easeui_token(raw: &str) -> Option<(&'static str, Color)> {
    let s = raw.trim();
    for &(canonical, (r, g, b), aliases) in EASEUI_CANONICAL_TOKENS {
        if aliases.contains(&s) {
            return Some((canonical, Color::rgb(r, g, b)));
        }
    }
    None
}

/// Maps an `easeui-*` preset name to its `(fill_canonical_token, stroke_canonical_token)` pair, if any.
pub fn preset_easeui_tokens(preset: &str) -> (Option<(&'static str, &'static str)>, Option<(&'static str, &'static str)>) {
    match preset.trim().to_ascii_lowercase().as_str() {
        "easeui-screen" => (Some(("bg/canvas", "$easeui.bg.canvas")), None),
        "easeui-panel" | "easeui-card" => (Some(("bg/panel", "$easeui.bg.panel")), Some(("border", "$easeui.border"))),
        "easeui-button-primary" => (Some(("accent/orange", "$easeui.accent.orange")), None),
        "easeui-button-secondary" | "easeui-badge" => (Some(("bg/surface", "$easeui.bg.surface")), Some(("border/subtle", "$easeui.border.subtle"))),
        "easeui-input" => (Some(("bg/canvas", "$easeui.bg.canvas")), Some(("border", "$easeui.border"))),
        _ => (None, None),
    }
}

/// Ensures the `easeui-deprecated` collection and all 19 canonical tokens exist in `doc`.
/// If a user collection with a different ID already uses the name `"EaseUI (deprecated)"`,
/// appends `" (2)"` (or higher) so user collections are never overwritten or rebound.
/// Returns the collection's display name.
pub fn ensure_easeui_deprecated_collection(doc: &mut Document) -> String {
    if let Some(existing) = doc.variable_collections.get(EASEUI_DEPRECATED_COLLECTION_ID) {
        return existing.name.clone();
    }
    let base_name = "EaseUI (deprecated)";
    let mut chosen_name = base_name.to_string();
    let name_taken =
        |d: &Document, candidate: &str| -> bool { d.variable_collections.values().any(|c| c.id != EASEUI_DEPRECATED_COLLECTION_ID && c.name == candidate) };
    if name_taken(doc, &chosen_name) {
        let mut suffix = 2usize;
        loop {
            let candidate = format!("{base_name} ({suffix})");
            if !name_taken(doc, &candidate) {
                chosen_name = candidate;
                break;
            }
            suffix += 1;
        }
    }

    let mut var_ids = Vec::with_capacity(EASEUI_CANONICAL_TOKENS.len());
    for &(canonical, (r, g, b), _) in EASEUI_CANONICAL_TOKENS {
        let vid = format!("{EASEUI_DEPRECATED_COLLECTION_ID}/{canonical}");
        var_ids.push(vid.clone());
        let mut values = BTreeMap::new();
        values.insert(EASEUI_DEPRECATED_MODE_ID.to_string(), VariableValue::Color(Color::rgb(r, g, b)));
        doc.variables.insert(
            vid.clone(),
            Variable {
                id: vid,
                name: canonical.to_string(),
                collection: EASEUI_DEPRECATED_COLLECTION_ID.to_string(),
                kind: VariableKind::Color,
                values,
                description: "Deprecated EaseUI BrainWave 2.0 token alias".to_string(),
                scopes: Vec::new(),
                hidden_from_publishing: false,
                key: None,
                code_syntax: BTreeMap::new(),
            },
        );
    }

    doc.variable_collections.insert(
        EASEUI_DEPRECATED_COLLECTION_ID.to_string(),
        VariableCollection {
            id: EASEUI_DEPRECATED_COLLECTION_ID.to_string(),
            name: chosen_name.clone(),
            modes: vec![VariableMode { id: EASEUI_DEPRECATED_MODE_ID.to_string(), name: "Default".to_string() }],
            default_mode: EASEUI_DEPRECATED_MODE_ID.to_string(),
            variables: var_ids,
            key: None,
            source_library_key: None,
        },
    );

    chosen_name
}

/// Resolves a `"var:<path>"` or `"var:id:<id>"` string to a local variable ID, importing from
/// `libraries` if needed.
pub fn resolve_variable_ref(
    doc: &mut Document,
    index: &mut SpecBindIndex,
    libraries: &[LibrarySnapshot],
    raw_ref: &str,
    field_name: &str,
    spec_path: &str,
) -> Result<String, SpecBindError> {
    let body = raw_ref.trim().strip_prefix("var:").unwrap_or(raw_ref.trim());
    if let Some(id_part) = body.strip_prefix("id:") {
        if doc.variables.contains_key(id_part) {
            return Ok(id_part.to_string());
        }
        // Check enabled libraries by raw variable id
        for snap in libraries {
            if !doc.library.enabled.iter().any(|l| l.id == snap.library_id) {
                continue;
            }
            if snap.variables.contains_key(id_part) {
                let asset_key = format!("var:{id_part}");
                let cmd = crate::import_asset(doc, snap, &asset_key, None);
                let _ = cmd.apply(doc);
                *index = SpecBindIndex::build(doc);
                let local_id = crate::local_key(&snap.library_id, id_part);
                if doc.variables.contains_key(&local_id) {
                    return Ok(local_id);
                }
            }
        }
        let valid_options = index.closest_variables(id_part, 5);
        let corrected_example = valid_options.first().map(|first| serde_json::json!({ field_name: format!("var:{first}") }));
        return Err(SpecBindError {
            code: "UNKNOWN_VARIABLE".to_string(),
            message: format!("Variable id '{id_part}' was not found in the document."),
            path: spec_path.to_string(),
            received: Some(raw_ref.to_string()),
            expected: Some("an existing variable reference".to_string()),
            valid_options,
            corrected_example,
            safe_to_retry: true,
        });
    }

    // Rule 1: Split at the first '/': collection name (exact) + variable name (exact, may contain '/')
    if let Some((coll_name, var_name)) = body.split_once('/') {
        if let Some(matches) = index.by_coll_and_name.get(&(coll_name.to_string(), var_name.to_string())) {
            if matches.len() == 1 {
                return Ok(matches[0].0.clone());
            } else if matches.len() > 1 {
                return Err(ambiguous_variable_error(raw_ref, field_name, spec_path, matches));
            }
        }
    }

    // Rule 2: Whole path equals a variable name in local collections
    if let Some(matches) = index.by_name.get(body) {
        if matches.len() == 1 {
            return Ok(matches[0].0.clone());
        } else if matches.len() > 1 {
            return Err(ambiguous_variable_error(raw_ref, field_name, spec_path, matches));
        }
    }

    // Rule 3: Enabled library variables, same two rules
    let mut lib_step1: Vec<(usize, String, String)> = Vec::new();
    let mut lib_step2: Vec<(usize, String, String)> = Vec::new();
    for (s_idx, snap) in libraries.iter().enumerate() {
        if !doc.library.enabled.iter().any(|l| l.id == snap.library_id) {
            continue;
        }
        if let Some((coll_name, var_name)) = body.split_once('/') {
            for (vid, var) in &snap.variables {
                let Some(coll) = snap.variable_collections.get(&var.collection) else {
                    continue;
                };
                if coll.name == coll_name && var.name == var_name {
                    lib_step1.push((s_idx, vid.clone(), coll.name.clone()));
                }
            }
        }
        for (vid, var) in &snap.variables {
            let Some(coll) = snap.variable_collections.get(&var.collection) else {
                continue;
            };
            if var.name == body {
                lib_step2.push((s_idx, vid.clone(), coll.name.clone()));
            }
        }
    }
    let chosen_lib = if !lib_step1.is_empty() {
        Some(lib_step1)
    } else if !lib_step2.is_empty() {
        Some(lib_step2)
    } else {
        None
    };
    if let Some(hits) = chosen_lib {
        if hits.len() > 1 {
            let pairs: Vec<(String, String)> =
                hits.iter().map(|(s_idx, vid, cname)| (crate::local_key(&libraries[*s_idx].library_id, vid), cname.clone())).collect();
            return Err(ambiguous_variable_error(raw_ref, field_name, spec_path, &pairs));
        }
        let (s_idx, vid, _) = &hits[0];
        let snap = &libraries[*s_idx];
        let asset_key = format!("var:{vid}");
        let cmd = crate::import_asset(doc, snap, &asset_key, None);
        let _ = cmd.apply(doc);
        *index = SpecBindIndex::build(doc);
        let local_id = crate::local_key(&snap.library_id, vid);
        return Ok(local_id);
    }

    let valid_options = index.closest_variables(body, 5);
    let corrected_example = valid_options.first().map(|first| serde_json::json!({ field_name: format!("var:{first}") }));
    Err(SpecBindError {
        code: "UNKNOWN_VARIABLE".to_string(),
        message: format!("Unknown variable '{body}'."),
        path: spec_path.to_string(),
        received: Some(raw_ref.to_string()),
        expected: Some("an existing variable path (<collection>/<name>)".to_string()),
        valid_options,
        corrected_example,
        safe_to_retry: true,
    })
}

fn ambiguous_variable_error(raw_ref: &str, field_name: &str, spec_path: &str, matches: &[(String, String)]) -> SpecBindError {
    let valid_options: Vec<String> = matches.iter().map(|(id, coll)| format!("var:id:{id} ({coll})")).collect();
    let first_id = matches.first().map(|(id, _)| format!("var:id:{id}")).unwrap_or_default();
    SpecBindError {
        code: "AMBIGUOUS_VARIABLE".to_string(),
        message: format!("Variable reference '{raw_ref}' matches multiple variables."),
        path: spec_path.to_string(),
        received: Some(raw_ref.to_string()),
        expected: Some("an unambiguous collection/name or var:id:<id>".to_string()),
        valid_options,
        corrected_example: Some(serde_json::json!({ field_name: first_id })),
        safe_to_retry: true,
    }
}

/// Validates that `var_id` can bind to `prop_keys` on `node_kind` (checking kind, applicability, and scope).
pub fn validate_variable_binding_for_keys(
    doc: &Document,
    index: &SpecBindIndex,
    var_id: &str,
    prop_keys: &[&str],
    node_kind: &NodeKind,
    raw_ref: &str,
    field_name: &str,
    spec_path: &str,
) -> Result<(), SpecBindError> {
    let allowed_keys = crate::bindable_keys(node_kind);
    let target_var = resolve_target_variable(doc, var_id).ok_or_else(|| SpecBindError {
        code: "UNKNOWN_VARIABLE".to_string(),
        message: format!("Variable '{var_id}' could not be resolved."),
        path: spec_path.to_string(),
        received: Some(raw_ref.to_string()),
        expected: None,
        valid_options: Vec::new(),
        corrected_example: None,
        safe_to_retry: true,
    })?;
    for &key in prop_keys {
        let expected_kind = crate::property_kind(key).ok_or_else(|| SpecBindError {
            code: "OVERRIDE_FIELD_NOT_APPLICABLE".to_string(),
            message: format!("Unknown bindable property key '{key}'."),
            path: spec_path.to_string(),
            received: Some(raw_ref.to_string()),
            expected: None,
            valid_options: Vec::new(),
            corrected_example: None,
            safe_to_retry: true,
        })?;
        if target_var.kind != expected_kind {
            let valid_options = index.variables_of_kind(expected_kind, 5);
            let corrected_example = valid_options.first().map(|first| serde_json::json!({ field_name: first }));
            return Err(SpecBindError {
                code: "VARIABLE_TYPE_MISMATCH".to_string(),
                message: format!(
                    "Property '{field_name}' ({key}) expects a {:?} variable, but '{}' has kind {:?}.",
                    expected_kind, target_var.name, target_var.kind
                ),
                path: spec_path.to_string(),
                received: Some(format!("{:?}", target_var.kind)),
                expected: Some(format!("{:?}", expected_kind)),
                valid_options,
                corrected_example,
                safe_to_retry: true,
            });
        }
        if !allowed_keys.contains(&key) {
            return Err(SpecBindError {
                code: "OVERRIDE_FIELD_NOT_APPLICABLE".to_string(),
                message: format!("Property '{key}' (from '{field_name}') is not applicable to {} nodes.", node_kind.default_name()),
                path: spec_path.to_string(),
                received: Some(raw_ref.to_string()),
                expected: Some(format!("one of the bindable properties for {}", node_kind.default_name())),
                valid_options: allowed_keys.iter().map(|s| (*s).to_string()).collect(),
                corrected_example: None,
                safe_to_retry: true,
            });
        }
        if !crate::scope_allows(&target_var.scopes, key, node_kind) {
            let valid_options = index.in_scope_variables(doc, expected_kind, key, node_kind, 5);
            let corrected_example = valid_options.first().map(|first| serde_json::json!({ field_name: first }));
            return Err(SpecBindError {
                code: "VARIABLE_SCOPE_MISMATCH".to_string(),
                message: format!(
                    "Variable '{}' has scopes {:?} which do not allow binding to '{key}' on {}.",
                    target_var.name,
                    target_var.scopes,
                    node_kind.default_name()
                ),
                path: spec_path.to_string(),
                received: Some(format!("{:?}", target_var.scopes)),
                expected: Some(format!("scope compatible with '{key}'")),
                valid_options,
                corrected_example,
                safe_to_retry: true,
            });
        }
    }
    Ok(())
}

/// Resolves a style reference by exact `StyleMeta.name` (or style key) and verifies its `StyleProperty`.
pub fn resolve_style_ref(
    doc: &mut Document,
    index: &mut SpecBindIndex,
    libraries: &[LibrarySnapshot],
    raw_ref: &str,
    expected_prop: StyleProperty,
    field_name: &str,
    spec_path: &str,
) -> Result<String, SpecBindError> {
    let name = raw_ref.trim().strip_prefix("style:").unwrap_or(raw_ref.trim());
    if let Some(keys) = index.styles_by_name.get(name).cloned() {
        for k in &keys {
            if crate::style_property(doc, k) == expected_prop {
                return Ok(k.clone());
            }
        }
        // Found a style with that name, but wrong type!
        let actual_prop = crate::style_property(doc, &keys[0]);
        let valid_options = index.closest_styles(name, Some(expected_prop), 5);
        let corrected_example = valid_options.first().map(|first| {
            let val = if field_name == "fill" || field_name == "stroke" { format!("style:{first}") } else { first.clone() };
            serde_json::json!({ field_name: val })
        });
        return Err(SpecBindError {
            code: "STYLE_TYPE_MISMATCH".to_string(),
            message: format!("Style '{name}' is a {:?} style, but '{field_name}' requires a {:?} style.", actual_prop, expected_prop),
            path: spec_path.to_string(),
            received: Some(format!("{:?}", actual_prop)),
            expected: Some(format!("{:?}", expected_prop)),
            valid_options,
            corrected_example,
            safe_to_retry: true,
        });
    }

    // Check enabled libraries
    for snap in libraries {
        if !doc.library.enabled.iter().any(|l| l.id == snap.library_id) {
            continue;
        }
        let mut wrong_type = None;
        for (skey, style) in &snap.styles {
            let sname = snap.style_meta.get(skey).map(|m| m.name.as_str()).filter(|n| !n.is_empty()).unwrap_or(skey.as_str());
            if sname == name {
                let prop = snap.style_meta.get(skey).map(|m| m.property).unwrap_or(match style {
                    Style::Text(_) => StyleProperty::Text,
                    Style::Effect(_) => StyleProperty::Effect,
                    Style::Grid(_) => StyleProperty::Grid,
                    Style::Fill(_) => StyleProperty::Fill,
                });
                if prop == expected_prop {
                    let asset_key = format!("style:{skey}");
                    let (cmd, local_opt) = crate::import_style_key(doc, snap, &asset_key);
                    let _ = cmd.apply(doc);
                    *index = SpecBindIndex::build(doc);
                    if let Some(local_key) = local_opt {
                        return Ok(local_key);
                    }
                } else {
                    wrong_type = Some(prop);
                }
            }
        }
        if let Some(actual_prop) = wrong_type {
            let valid_options = index.closest_styles(name, Some(expected_prop), 5);
            let corrected_example = valid_options.first().map(|first| serde_json::json!({ field_name: first }));
            return Err(SpecBindError {
                code: "STYLE_TYPE_MISMATCH".to_string(),
                message: format!("Library style '{name}' is a {:?} style, but '{field_name}' requires a {:?} style.", actual_prop, expected_prop),
                path: spec_path.to_string(),
                received: Some(format!("{:?}", actual_prop)),
                expected: Some(format!("{:?}", expected_prop)),
                valid_options,
                corrected_example,
                safe_to_retry: true,
            });
        }
    }

    let valid_options = index.closest_styles(name, Some(expected_prop), 5);
    let corrected_example = valid_options.first().map(|first| {
        let val = if field_name == "fill" || field_name == "stroke" { format!("style:{first}") } else { first.clone() };
        serde_json::json!({ field_name: val })
    });
    Err(SpecBindError {
        code: "UNKNOWN_STYLE".to_string(),
        message: format!("Unknown {:?} style '{name}'.", expected_prop),
        path: spec_path.to_string(),
        received: Some(raw_ref.to_string()),
        expected: Some(format!("an existing {:?} style name", expected_prop)),
        valid_options,
        corrected_example,
        safe_to_retry: true,
    })
}

/// Resolves a `mode: { "<collection name>": "<mode name>" }` map into `(collection_id, mode_id)` pairs.
pub fn resolve_mode_map(doc: &Document, mode_spec: &BTreeMap<String, String>, spec_path: &str) -> Result<BTreeMap<String, String>, SpecBindError> {
    let mut resolved = BTreeMap::new();
    for (coll_name, mode_name) in mode_spec {
        let field_path = format!("{spec_path}.mode.{coll_name}");
        let Some(coll) = doc
            .variable_collections
            .values()
            .find(|c| c.id != EASEUI_DEPRECATED_COLLECTION_ID && c.name == *coll_name)
            .or_else(|| doc.variable_collections.values().find(|c| c.name == *coll_name))
        else {
            let valid_options: Vec<String> = doc
                .variable_collections
                .values()
                .filter(|c| c.id != EASEUI_DEPRECATED_COLLECTION_ID)
                .flat_map(|c| c.modes.iter().map(move |m| format!("{}:{}", c.name, m.name)))
                .take(20)
                .collect();
            return Err(SpecBindError {
                code: "UNKNOWN_VARIABLE_MODE".to_string(),
                message: format!("Variable collection '{coll_name}' does not exist."),
                path: field_path,
                received: Some(format!("{coll_name}:{mode_name}")),
                expected: Some("an existing variable collection and mode name".to_string()),
                valid_options,
                corrected_example: None,
                safe_to_retry: true,
            });
        };
        let Some(mode) = coll.modes.iter().find(|m| m.name == *mode_name || m.id == *mode_name) else {
            let valid_options: Vec<String> = coll.modes.iter().map(|m| m.name.clone()).collect();
            let corrected_example = valid_options.first().map(|first| serde_json::json!({ "mode": { coll_name: first } }));
            return Err(SpecBindError {
                code: "UNKNOWN_VARIABLE_MODE".to_string(),
                message: format!("Collection '{coll_name}' has no mode named '{mode_name}'."),
                path: field_path,
                received: Some(mode_name.clone()),
                expected: Some(format!("one of: {}", valid_options.join(", "))),
                valid_options,
                corrected_example,
                safe_to_retry: true,
            });
        };
        resolved.insert(coll.id.clone(), mode.id.clone());
    }
    Ok(resolved)
}

/// Resolves a variable in the effective mode of `node_id` (falling back to the collection's default mode
/// and emitting a `VARIABLE_MODE_FALLBACK` warning if the variable has no value in the effective mode).
pub fn resolve_variable_with_fallback_warning(
    doc: &Document,
    node_id: NodeId,
    var_id: &str,
    spec_path: &str,
    warnings: &mut Vec<SpecWarning>,
) -> Option<VariableValue> {
    if let Some(v) = doc.variables.get(var_id) {
        if let Some(eff_mode) = doc.variable_mode(node_id, &v.collection) {
            if !v.values.contains_key(&eff_mode) {
                warnings.push(SpecWarning {
                    code: "VARIABLE_MODE_FALLBACK".to_string(),
                    path: spec_path.to_string(),
                    token: Some(v.name.clone()),
                    replacement: None,
                    message: Some(format!("Variable '{}' has no value for mode '{}'; fell back to collection default mode.", v.name, eff_mode)),
                });
            }
        }
    }
    doc.resolve_variable(node_id, var_id)
}

/// Resolves a colour reference (`"var:..."`, `"style:..."`, `"$easeui.*"`, `"transparent"`/`"none"`, or `"#RRGGBB[AA]"`).
/// Reused by `apply_spec` and `E15-1` instance overrides.
#[allow(clippy::too_many_arguments)]
pub fn resolve_colour_ref(
    doc: &mut Document,
    index: &mut SpecBindIndex,
    libraries: &[LibrarySnapshot],
    node_id: NodeId,
    node_kind: &NodeKind,
    prop_key: &str,
    field_name: &str,
    raw: &str,
    spec_path: &str,
    warnings: &mut Vec<SpecWarning>,
) -> Result<ResolvedColourRef, SpecBindError> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("none") || trimmed.eq_ignore_ascii_case("transparent") {
        return Ok(ResolvedColourRef::NoneOrTransparent);
    }
    if trimmed.starts_with("var:") {
        let var_id = resolve_variable_ref(doc, index, libraries, trimmed, field_name, spec_path)?;
        validate_variable_binding_for_keys(doc, index, &var_id, &[prop_key], node_kind, trimmed, field_name, spec_path)?;
        let val = resolve_variable_with_fallback_warning(doc, node_id, &var_id, spec_path, warnings);
        let color = match val {
            Some(VariableValue::Color(c)) => c,
            _ => Color::TRANSPARENT,
        };
        return Ok(ResolvedColourRef::Variable { var_id, color });
    }
    if trimmed.starts_with("style:") {
        let style_key = resolve_style_ref(doc, index, libraries, trimmed, StyleProperty::Fill, field_name, spec_path)?;
        let paints = match doc.styles.get(&style_key) {
            Some(Style::Fill(p)) => p.clone(),
            _ => Vec::new(),
        };
        return Ok(ResolvedColourRef::Style { style_key, paints });
    }
    if let Some((canonical, color)) = lookup_easeui_token(trimmed) {
        let coll_name = ensure_easeui_deprecated_collection(doc);
        index.ensure_easeui_indexed(doc);
        let var_id = format!("{EASEUI_DEPRECATED_COLLECTION_ID}/{canonical}");
        warnings.push(SpecWarning {
            code: "DEPRECATED_EASEUI_TOKEN".to_string(),
            path: spec_path.to_string(),
            token: Some(trimmed.to_string()),
            replacement: Some(format!("var:{coll_name}/{canonical}")),
            message: None,
        });
        return Ok(ResolvedColourRef::Variable { var_id, color });
    }
    if let Some(color) = Color::from_hex(trimmed) {
        return Ok(ResolvedColourRef::Literal(color));
    }
    Err(SpecBindError {
        code: "INVALID_COLOR".to_string(),
        message: format!("Invalid colour value '{trimmed}'."),
        path: spec_path.to_string(),
        received: Some(raw.to_string()),
        expected: Some("one of: #RRGGBB, #RRGGBBAA, var:..., style:...".to_string()),
        valid_options: vec!["#RRGGBB".to_string(), "#RRGGBBAA".to_string(), "var:...".to_string(), "style:...".to_string()],
        corrected_example: None,
        safe_to_retry: true,
    })
}

/// Binds a variable ID to `prop_keys` on `node_id` in `doc` and writes the resolved value via [`set_bound_property`].
pub fn apply_variable_binding(
    doc: &mut Document,
    node_id: NodeId,
    prop_keys: &[&str],
    var_id: &str,
    spec_path: &str,
    warnings: &mut Vec<SpecWarning>,
) -> usize {
    let val = resolve_variable_with_fallback_warning(doc, node_id, var_id, spec_path, warnings);
    let Some(node) = doc.get_mut(node_id) else {
        return 0;
    };
    let mut created = 0usize;
    for &key in prop_keys {
        if key == "fills/0" {
            match &mut node.kind {
                NodeKind::Text(tp) => {
                    if tp.style.fills.is_empty() {
                        tp.style.fills.push(Paint::solid(Color::TRANSPARENT));
                    }
                }
                _ => {
                    if node.fills.is_empty() {
                        node.fills.push(Paint::solid(Color::TRANSPARENT));
                    }
                }
            }
        } else if key == "strokes/0" && node.stroke.paints.is_empty() {
            node.stroke.paints.push(Paint::solid(Color::TRANSPARENT));
        }
        node.variables.bindings.insert(key.to_string(), var_id.to_string());
        if let Some(ref resolved) = val {
            crate::set_bound_property(node, key, resolved);
        }
        created += 1;
    }
    created
}

/// Test fixtures for E15-3 (`fixtures::tokens_doc()`).
pub mod fixtures {
    use crate::*;
    use std::collections::BTreeMap;

    /// Builds the canonical E15-3 test document:
    /// - Collection `Color` (modes `Light`, `Dark`) with `Primary` (`#0D99FF` / `#3582FF`)
    /// - Collection `Spacing` (mode `Default`) with `4` = `16.0`
    /// - Text style `Heading L` (`24px` / weight `700`)
    /// - Effect style `Elevation/2`
    pub fn tokens_doc() -> Document {
        let mut doc = Document::new("Tokens Doc");
        doc.add_page("Page 1");

        // Collection: Color (Light, Dark)
        doc.variable_collections.insert(
            "coll_color".to_string(),
            VariableCollection {
                id: "coll_color".to_string(),
                name: "Color".to_string(),
                modes: vec![
                    VariableMode { id: "mode_light".to_string(), name: "Light".to_string() },
                    VariableMode { id: "mode_dark".to_string(), name: "Dark".to_string() },
                ],
                default_mode: "mode_light".to_string(),
                variables: vec!["var_color_primary".to_string()],
                key: None,
                source_library_key: None,
            },
        );
        let mut primary_values = BTreeMap::new();
        primary_values.insert("mode_light".to_string(), VariableValue::Color(Color::rgb(0x0D, 0x99, 0xFF)));
        primary_values.insert("mode_dark".to_string(), VariableValue::Color(Color::rgb(0x35, 0x82, 0xFF)));
        doc.variables.insert(
            "var_color_primary".to_string(),
            Variable {
                id: "var_color_primary".to_string(),
                name: "Primary".to_string(),
                collection: "coll_color".to_string(),
                kind: VariableKind::Color,
                values: primary_values,
                description: String::new(),
                scopes: Vec::new(),
                hidden_from_publishing: false,
                key: None,
                code_syntax: BTreeMap::new(),
            },
        );

        // Collection: Spacing (Default) with "4" = 16
        doc.variable_collections.insert(
            "coll_spacing".to_string(),
            VariableCollection {
                id: "coll_spacing".to_string(),
                name: "Spacing".to_string(),
                modes: vec![VariableMode { id: "mode_default".to_string(), name: "Default".to_string() }],
                default_mode: "mode_default".to_string(),
                variables: vec!["var_spacing_4".to_string()],
                key: None,
                source_library_key: None,
            },
        );
        let mut spacing_values = BTreeMap::new();
        spacing_values.insert("mode_default".to_string(), VariableValue::Number(16.0));
        doc.variables.insert(
            "var_spacing_4".to_string(),
            Variable {
                id: "var_spacing_4".to_string(),
                name: "4".to_string(),
                collection: "coll_spacing".to_string(),
                kind: VariableKind::Number,
                values: spacing_values,
                description: String::new(),
                scopes: Vec::new(),
                hidden_from_publishing: false,
                key: None,
                code_syntax: BTreeMap::new(),
            },
        );

        // Text style: Heading L (24 / 700)
        let mut heading_ts = TextStyle::default();
        heading_ts.font_size = 24.0;
        heading_ts.font_weight = 700;
        doc.styles.insert("style_heading_l".to_string(), Style::Text(heading_ts));
        doc.style_meta.insert(
            "style_heading_l".to_string(),
            StyleMeta { name: "Heading L".to_string(), description: String::new(), property: StyleProperty::Text, key: None },
        );

        // Effect style: Elevation/2
        doc.styles.insert(
            "style_elevation_2".to_string(),
            Style::Effect(vec![Effect::DropShadow {
                color: Color::rgba(0, 0, 0, 64),
                offset: Vec2::new(0.0, 4.0),
                radius: 8.0,
                spread: 0.0,
                visible: true,
                show_behind: false,
            }]),
        );
        doc.style_meta.insert(
            "style_elevation_2".to_string(),
            StyleMeta { name: "Elevation/2".to_string(), description: String::new(), property: StyleProperty::Effect, key: None },
        );

        doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_fill_and_mode_switch_recolours() {
        let mut doc = fixtures::tokens_doc();
        let page = doc.pages[0];
        let spec: NodeSpec = serde_json::from_value(serde_json::json!({
            "kind": "frame",
            "name": "Dark Frame",
            "mode": { "Color": "Dark" },
            "children": [
                {
                    "kind": "rectangle",
                    "name": "Primary Rect",
                    "fill": "var:Color/Primary"
                }
            ]
        }))
        .unwrap();

        let report = doc.apply_spec_with_report(page, &spec).unwrap();
        assert_eq!(report.bindings_created, 1);
        let frame = doc.get(report.root_node_id).unwrap();
        assert_eq!(frame.variables.modes.get("coll_color").map(String::as_str), Some("mode_dark"));

        let rect_id = frame.children[0];
        let rect = doc.get(rect_id).unwrap();
        assert_eq!(rect.variables.bindings.get("fills/0").map(String::as_str), Some("var_color_primary"));
        assert_eq!(rect.fills[0].kind, PaintKind::Solid(Color::rgb(0x35, 0x82, 0xFF)));

        // Switching the ancestor frame mode to Light re-resolves the bound colour to #0D99FF
        doc.get_mut(report.root_node_id).unwrap().variables.modes.insert("coll_color".into(), "mode_light".into());
        assert_eq!(doc.bound_value(rect_id, "fills/0"), Some(VariableValue::Color(Color::rgb(0x0D, 0x99, 0xFF))));
    }

    #[test]
    fn unknown_variable_lists_5_closest() {
        let mut doc = fixtures::tokens_doc();
        // Add 5 more variables to verify the top-5 cap and ordering
        for (i, name) in ["Secondary", "Tertiary", "Accent", "Surface", "Muted"].iter().enumerate() {
            let id = format!("var_extra_{i}");
            let mut vals = BTreeMap::new();
            vals.insert("mode_light".to_string(), VariableValue::Color(Color::WHITE));
            doc.variables.insert(
                id.clone(),
                Variable {
                    id: id.clone(),
                    name: (*name).to_string(),
                    collection: "coll_color".to_string(),
                    kind: VariableKind::Color,
                    values: vals,
                    description: String::new(),
                    scopes: Vec::new(),
                    hidden_from_publishing: false,
                    key: None,
                    code_syntax: BTreeMap::new(),
                },
            );
            doc.variable_collections.get_mut("coll_color").unwrap().variables.push(id);
        }

        let page = doc.pages[0];
        let before_nodes = doc.nodes.len();
        let before_vars = doc.variables.len();
        let spec: NodeSpec = serde_json::from_value(serde_json::json!({
            "kind": "rectangle",
            "fill": "var:Color/Primery"
        }))
        .unwrap();

        let err = doc.apply_spec_with_report(page, &spec).unwrap_err();
        assert_eq!(err.code, "UNKNOWN_VARIABLE");
        assert!(err.valid_options.len() <= 5);
        assert_eq!(err.valid_options.first().map(String::as_str), Some("Color/Primary"));
        assert!(err.safe_to_retry);
        assert_eq!(doc.nodes.len(), before_nodes, "document nodes must be unchanged on error");
        assert_eq!(doc.variables.len(), before_vars, "document variables must be unchanged on error");
    }

    #[test]
    fn number_binding_gap() {
        let mut doc = fixtures::tokens_doc();
        let page = doc.pages[0];
        let spec: NodeSpec = serde_json::from_value(serde_json::json!({
            "kind": "frame",
            "name": "Row Frame",
            "layout": {
                "direction": "row",
                "gap": "var:Spacing/4"
            }
        }))
        .unwrap();

        let report = doc.apply_spec_with_report(page, &spec).unwrap();
        assert_eq!(report.bindings_created, 1);
        let node = doc.get(report.root_node_id).unwrap();
        assert_eq!(node.variables.bindings.get("itemSpacing").map(String::as_str), Some("var_spacing_4"));
        let al = node.kind.frame_props().unwrap().auto_layout.as_ref().unwrap();
        assert_eq!(al.item_spacing, 16.0);
    }

    #[test]
    fn text_style_link() {
        let mut doc = fixtures::tokens_doc();
        let page = doc.pages[0];
        let spec: NodeSpec = serde_json::from_value(serde_json::json!({
            "kind": "text",
            "text": "Dashboard",
            "text_style": "Heading L"
        }))
        .unwrap();

        let report = doc.apply_spec_with_report(page, &spec).unwrap();
        assert_eq!(report.styles_linked, 1);
        let node = doc.get(report.root_node_id).unwrap();
        assert_eq!(node.style_refs.get("text").map(String::as_str), Some("style_heading_l"));
        let NodeKind::Text(tp) = &node.kind else {
            panic!("expected text node");
        };
        assert_eq!(tp.style.font_size, 24.0);
        assert_eq!(tp.style.font_weight, 700);
    }

    #[test]
    fn easeui_alias_bit_identical() {
        // 1. Apply the canonical spec.rs:804 EaseUI spec (using presets & $easeui.* tokens)
        let mut doc_alias = Document::new("Alias Spec Test");
        let page_a = doc_alias.add_page("Screens");
        let spec_alias: NodeSpec = serde_json::from_value(serde_json::json!({
            "name": "IAM · Login Screen",
            "preset": "easeui-screen",
            "dev_status": "ready",
            "annotations": ["Use BrainWave 2.0 dark tokens"],
            "children": [
                {
                    "name": "Login Card",
                    "preset": "easeui-card",
                    "width": 400.0,
                    "height_sizing": "hug",
                    "children": [
                        {
                            "kind": "text",
                            "text": "Sign in to DESIGN:OS",
                            "font_size": 20.0,
                            "font_weight": 600,
                            "text_color": "$easeui.text.primary"
                        },
                        {
                            "name": "Email Input",
                            "preset": "easeui-input",
                            "width_sizing": "fill",
                            "text": "jang@designos.dev",
                            "text_color": "$easeui.text.secondary"
                        },
                        {
                            "name": "Submit Button",
                            "preset": "easeui-button-primary",
                            "width_sizing": "fill",
                            "text": "Continue"
                        }
                    ]
                }
            ]
        }))
        .unwrap();
        let report = doc_alias.apply_spec_with_report(page_a, &spec_alias).unwrap();
        assert!(doc_alias.variable_collections.contains_key(EASEUI_DEPRECATED_COLLECTION_ID), "EaseUI (deprecated) collection must be materialized");
        // 1 (screen fill) + 2 (card fill+stroke) + 1 (title text_color) + 2 (input fill+stroke) + 1 (input text_color) + 1 (button fill) = 8
        assert_eq!(report.warnings.len(), 8);
        assert!(report.warnings.iter().all(|w| w.code == "DEPRECATED_EASEUI_TOKEN"));
        assert!(report.warnings.iter().any(|w| w.replacement.as_deref() == Some("var:EaseUI (deprecated)/bg/panel")));

        // 2. Apply the identical spec with raw literal hex colours (pre-E15-3 golden)
        let mut doc_literal = Document::new("Literal Spec Test");
        let page_l = doc_literal.add_page("Screens");
        let screen_l = doc_literal.apply_spec(page_l, &spec_alias).unwrap();

        // Compare every node's resolved visual state (after apply_bound_variables, which render_node uses)
        let resolved_doc = crate::apply_bound_variables(&doc_alias);
        let mut nodes_a = Vec::new();
        resolved_doc.walk(report.root_node_id, |n, _| {
            let mut clean = n.clone();
            clean.variables = NodeVariables::default();
            nodes_a.push(clean);
        });
        let mut nodes_l = Vec::new();
        doc_literal.walk(screen_l, |n, _| {
            let mut clean = n.clone();
            clean.variables = NodeVariables::default();
            nodes_l.push(clean);
        });
        assert_eq!(
            serde_json::to_vec(&nodes_a).unwrap(),
            serde_json::to_vec(&nodes_l).unwrap(),
            "resolved node hierarchy and paints must be bit-for-bit identical"
        );
        // Also verify every bound node already holds the exact literal RGB in its raw fills/strokes
        let card_id = doc_alias.get(report.root_node_id).unwrap().children[0];
        let card = doc_alias.get(card_id).unwrap();
        assert_eq!(card.fills[0].kind, PaintKind::Solid(Color::rgb(0x18, 0x18, 0x1B)));
        assert_eq!(card.stroke.paints[0].kind, PaintKind::Solid(Color::rgb(0x27, 0x27, 0x2A)));
    }

    #[test]
    fn type_mismatch() {
        let mut doc = fixtures::tokens_doc();
        let page = doc.pages[0];
        let spec: NodeSpec = serde_json::from_value(serde_json::json!({
            "font_size": "var:Color/Primary"
        }))
        .unwrap();

        let err = doc.apply_spec_with_report(page, &spec).unwrap_err();
        assert_eq!(err.code, "VARIABLE_TYPE_MISMATCH");
        assert!(err.valid_options.contains(&"var:Spacing/4".to_string()));
    }

    #[test]
    fn export_apply_keeps_bindings() {
        let mut doc = fixtures::tokens_doc();
        let page = doc.pages[0];
        let spec: NodeSpec = serde_json::from_value(serde_json::json!({
            "kind": "frame",
            "name": "Bound Card",
            "fill": "var:Color/Primary",
            "effect_style": "Elevation/2",
            "mode": { "Color": "Dark" },
            "layout": {
                "direction": "column",
                "gap": "var:Spacing/4",
                "padding": ["var:Spacing/4", 12.0, "var:Spacing/4", 12.0]
            },
            "children": [
                {
                    "kind": "text",
                    "name": "Heading",
                    "text": "Hello Tokens",
                    "text_style": "Heading L",
                    "text_color": "var:Color/Primary"
                }
            ]
        }))
        .unwrap();

        let first = doc.apply_spec_with_report(page, &spec).unwrap();
        let exported = doc.export_spec(first.root_node_id).unwrap();
        assert_eq!(exported.fill.as_deref(), Some("var:Color/Primary"));
        assert_eq!(exported.effect_style.as_deref(), Some("Elevation/2"));
        assert_eq!(exported.mode.get("Color").map(String::as_str), Some("Dark"));
        assert_eq!(exported.layout.as_ref().unwrap().gap, NumberOrVar::Ref("var:Spacing/4".to_string()));
        assert_eq!(exported.children[0].text_style.as_deref(), Some("Heading L"));
        assert_eq!(exported.children[0].text_color.as_deref(), Some("var:Color/Primary"));

        let second = doc.apply_spec_with_report(page, &exported).unwrap();
        let n1 = doc.get(first.root_node_id).unwrap();
        let n2 = doc.get(second.root_node_id).unwrap();
        assert_eq!(n1.variables.bindings, n2.variables.bindings);
        assert_eq!(n1.variables.modes, n2.variables.modes);
        assert_eq!(n1.style_refs, n2.style_refs);

        let c1 = doc.get(n1.children[0]).unwrap();
        let c2 = doc.get(n2.children[0]).unwrap();
        assert_eq!(c1.variables.bindings, c2.variables.bindings);
        assert_eq!(c1.style_refs, c2.style_refs);
    }

    #[test]
    fn edge_cases_scope_detach_and_name_clash() {
        let mut doc = fixtures::tokens_doc();
        let page = doc.pages[0];

        // Edge case 3: corner_radius on a text node -> OVERRIDE_FIELD_NOT_APPLICABLE
        let bad_radius: NodeSpec = serde_json::from_value(serde_json::json!({
            "kind": "text",
            "text": "No radius",
            "corner_radius": "var:Spacing/4"
        }))
        .unwrap();
        let err = doc.apply_spec_with_report(page, &bad_radius).unwrap_err();
        assert_eq!(err.code, "OVERRIDE_FIELD_NOT_APPLICABLE");

        // Explicit font_size detaches text_style with STYLE_DETACHED_BY_SPEC warning
        let detached: NodeSpec = serde_json::from_value(serde_json::json!({
            "kind": "text",
            "text": "Custom Size",
            "text_style": "Heading L",
            "font_size": 18.0
        }))
        .unwrap();
        let rep = doc.apply_spec_with_report(page, &detached).unwrap();
        assert!(rep.warnings.iter().any(|w| w.code == "STYLE_DETACHED_BY_SPEC"));
        let tn = doc.get(rep.root_node_id).unwrap();
        assert!(!tn.style_refs.contains_key("text"));
        if let NodeKind::Text(tp) = &tn.kind {
            assert_eq!(tp.style.font_size, 18.0);
        }

        // Edge case 4: user collection already named "EaseUI (deprecated)" -> engine appends " (2)"
        doc.variable_collections.insert(
            "user_coll".into(),
            VariableCollection {
                id: "user_coll".into(),
                name: "EaseUI (deprecated)".into(),
                modes: vec![VariableMode { id: "m1".into(), name: "Default".into() }],
                default_mode: "m1".into(),
                variables: Vec::new(),
                key: None,
                source_library_key: None,
            },
        );
        let ease_spec: NodeSpec = serde_json::from_value(serde_json::json!({
            "kind": "rectangle",
            "fill": "$easeui.bg.panel"
        }))
        .unwrap();
        let ease_rep = doc.apply_spec_with_report(page, &ease_spec).unwrap();
        assert_eq!(doc.variable_collections.get(EASEUI_DEPRECATED_COLLECTION_ID).unwrap().name, "EaseUI (deprecated) (2)");
        assert_eq!(ease_rep.warnings[0].replacement.as_deref(), Some("var:EaseUI (deprecated) (2)/bg/panel"));
    }

    #[test]
    fn perf_1000_bindings_on_5000_variables_under_budget() {
        let mut doc = Document::new("Perf Doc");
        let page = doc.add_page("Page 1");
        let mut var_ids = Vec::with_capacity(5000);
        for i in 0..5000 {
            let vid = format!("v_{i}");
            var_ids.push(vid.clone());
            let mut vals = BTreeMap::new();
            vals.insert("m0".to_string(), VariableValue::Color(Color::rgb(0x10, 0x20, 0x30)));
            doc.variables.insert(
                vid.clone(),
                Variable {
                    id: vid,
                    name: format!("token/{i}"),
                    collection: "c0".to_string(),
                    kind: VariableKind::Color,
                    values: vals,
                    description: String::new(),
                    scopes: Vec::new(),
                    hidden_from_publishing: false,
                    key: None,
                    code_syntax: BTreeMap::new(),
                },
            );
        }
        doc.variable_collections.insert(
            "c0".to_string(),
            VariableCollection {
                id: "c0".to_string(),
                name: "Tokens".to_string(),
                modes: vec![VariableMode { id: "m0".to_string(), name: "Default".to_string() }],
                default_mode: "m0".to_string(),
                variables: var_ids,
                key: None,
                source_library_key: None,
            },
        );

        let children: Vec<NodeSpec> = (0..500)
            .map(|i| NodeSpec {
                kind: "rectangle".to_string(),
                name: format!("R{i}"),
                fill: Some(format!("var:Tokens/token/{}", i * 2)),
                stroke: Some(format!("var:Tokens/token/{}", i * 2 + 1)),
                ..Default::default()
            })
            .collect();
        let root_spec = NodeSpec { kind: "frame".to_string(), name: "Perf Root".to_string(), children, ..Default::default() };

        let start = std::time::Instant::now();
        let report = doc.apply_spec_with_report(page, &root_spec).unwrap();
        let elapsed = start.elapsed();
        assert_eq!(report.bindings_created, 1000);
        // Budget is < 20ms in release; allow generous 250ms in unoptimized debug builds
        assert!(elapsed.as_millis() < 250, "1,000 bindings on 5,000 variables took {}ms", elapsed.as_millis());
    }
}
