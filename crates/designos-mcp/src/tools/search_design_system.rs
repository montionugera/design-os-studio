//! `designos_search_design_system` implementation (`T7.9`, Issue `#272`, extended by `E15`).

use super::library_source::{enabled_snapshots_from_root, resolve_library_root};
use designos_doc::{
    is_slot_container, library_asset_of_component, local_component_roots, local_key, parse_variant_name, property_kind, scope_allows, style_property,
    ComponentPropDef, Document, FrameProps, LibraryAssetKind, LibrarySnapshot, Node, NodeId, NodeKind, OverrideField, PaintKind, PropKind, PropValue,
    SlotPropConfig, Style, StyleMeta, StyleProperty, TextProps, Variable, VariableCollection, VariableKind, VariableValue,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Canonical bindable property keys accepted by `designos_doc::property_kind`.
pub const VALID_PROPERTY_KEYS: &[&str] = &[
    "fills/0",
    "strokes/0",
    "fills/0/opacity",
    "strokes/0/opacity",
    "effects/0/opacity",
    "width",
    "height",
    "topLeftRadius",
    "topRightRadius",
    "bottomLeftRadius",
    "bottomRightRadius",
    "itemSpacing",
    "counterAxisSpacing",
    "paddingLeft",
    "paddingRight",
    "paddingTop",
    "paddingBottom",
    "opacity",
    "fontSize",
    "fontWeight",
    "lineHeight",
    "letterSpacing",
    "strokeWeight",
    "strokeTopWeight",
    "strokeRightWeight",
    "strokeBottomWeight",
    "strokeLeftWeight",
    "visible",
    "characters",
];

fn default_limit() -> usize {
    20
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchDesignSystemArgs {
    pub query: String,
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    #[serde(default)]
    pub property: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemKind {
    ComponentSet,
    Component,
    Style,
    Variable,
}

impl ItemKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ComponentSet => "component_set",
            Self::Component => "component",
            Self::Style => "style",
            Self::Variable => "variable",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::ComponentSet => 0,
            Self::Component => 1,
            Self::Style => 2,
            Self::Variable => 3,
        }
    }

    fn matches_filter(self, filter: &[String]) -> bool {
        filter.iter().any(|k| match k.as_str() {
            "component" => matches!(self, Self::Component | Self::ComponentSet),
            "component_set" => matches!(self, Self::ComponentSet),
            "style" => matches!(self, Self::Style),
            "variable" => matches!(self, Self::Variable),
            _ => false,
        })
    }
}

#[derive(Clone, Debug)]
struct IndexedItem {
    kind: ItemKind,
    name: String,
    description: String,
    is_local: bool,
    ref_str: String,
    /// Lowercase `/`-separated path segments of the name (plus collection name for variables).
    path_segments: Vec<String>,
    /// Lowercase word tokens split on whitespace, `/`, `-`, `_`.
    name_tokens: Vec<String>,
    /// Lowercase full name (`name.to_lowercase()`).
    name_lower: String,
    /// Optional qualified lowercase name (`<collection>/<name>` for variables).
    qualified_lower: Option<String>,
    /// Variable metadata for `property` scope & kind filtering.
    var_meta: Option<VariableFilterMeta>,
    /// Prebuilt instance-ready payload fields (merged with `score` on match).
    payload: Map<String, Value>,
}

#[derive(Clone, Debug)]
struct VariableFilterMeta {
    kind: VariableKind,
    scopes: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SearchIndex {
    items: Vec<IndexedItem>,
    libraries_searched: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IndexCacheKey {
    doc_runtime_id: u64,
    doc_epoch: u64,
    node_count: usize,
    style_count: usize,
    var_count: usize,
    external_count: usize,
    enabled_libs: Vec<(String, u32)>,
    root: PathBuf,
}

type CachedIndex = Option<(IndexCacheKey, Arc<SearchIndex>)>;

fn index_cache() -> &'static Mutex<CachedIndex> {
    static CACHE: OnceLock<Mutex<CachedIndex>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Clears the cached per-session search index.
pub fn clear_search_index_cache() {
    if let Ok(mut guard) = index_cache().lock() {
        *guard = None;
    }
}

/// Executes `designos_search_design_system` against `doc` using the configured library root.
pub fn search_design_system(doc: &Document, raw_args: Value) -> Result<Value, String> {
    let root = resolve_library_root();
    search_design_system_at(doc, raw_args, &root)
}

/// Executes `designos_search_design_system` against `doc` and an explicit library `root`.
pub fn search_design_system_at(doc: &Document, raw_args: Value, root: &Path) -> Result<Value, String> {
    let args: SearchDesignSystemArgs = serde_json::from_value(raw_args).map_err(|e| format!("INVALID_ARGUMENTS: {e}"))?;
    validate_args(&args)?;

    let index = get_or_build_index(doc, root);
    Ok(index.query(&args))
}

fn validate_args(args: &SearchDesignSystemArgs) -> Result<(), String> {
    if args.query.trim().is_empty() || args.query.len() > 256 {
        return Err("INVALID_ARGUMENTS: query must be 1..=256 non-empty characters".into());
    }
    if !(1..=50).contains(&args.limit) {
        return Err("INVALID_ARGUMENTS: limit must be between 1 and 50".into());
    }
    if let Some(kinds) = &args.kinds {
        for k in kinds {
            if !matches!(k.as_str(), "component" | "component_set" | "style" | "variable") {
                return Err(format!("INVALID_ARGUMENTS: unknown kind '{k}' (expected component, style, or variable)"));
            }
        }
    }
    if let Some(prop) = &args.property {
        if property_kind(prop).is_none() {
            return Err(format!("UNKNOWN_PROPERTY_KEY: '{prop}' is not a bindable property key (valid_options: {})", VALID_PROPERTY_KEYS.join(", ")));
        }
    }
    Ok(())
}

fn get_or_build_index(doc: &Document, root: &Path) -> Arc<SearchIndex> {
    let key = IndexCacheKey {
        doc_runtime_id: doc.runtime_id(),
        doc_epoch: doc.mutation_epoch(),
        node_count: doc.nodes.len(),
        style_count: doc.styles.len(),
        var_count: doc.variables.len(),
        external_count: doc.library.external.len(),
        enabled_libs: doc.library.enabled.iter().map(|r| (r.id.clone(), r.version)).collect(),
        root: root.to_path_buf(),
    };
    if let Ok(guard) = index_cache().lock() {
        if let Some((cached_key, cached_idx)) = guard.as_ref() {
            if cached_key == &key {
                return Arc::clone(cached_idx);
            }
        }
    }
    let built = Arc::new(SearchIndex::build(doc, root));
    if let Ok(mut guard) = index_cache().lock() {
        *guard = Some((key, Arc::clone(&built)));
    }
    built
}

impl SearchIndex {
    /// Builds the search index from `doc`'s local assets and all enabled library snapshots in `root`.
    pub fn build(doc: &Document, root: &Path) -> Self {
        let snapshots = enabled_snapshots_from_root(doc, root);
        Self::build_with_snapshots(doc, &snapshots)
    }

    /// Builds the search index from `doc` and an explicit slice of enabled [`LibrarySnapshot`]s.
    pub fn build_with_snapshots(doc: &Document, snapshots: &[LibrarySnapshot]) -> Self {
        let mut items = Vec::new();
        let mut libraries_searched = Vec::with_capacity(1 + doc.library.enabled.len());
        libraries_searched.push("local".to_string());
        for en in &doc.library.enabled {
            if !libraries_searched.contains(&en.id) {
                libraries_searched.push(en.id.clone());
            }
        }

        let imported_local_keys: BTreeSet<&str> = doc.library.external.values().filter_map(|e| e.local_key.as_deref()).collect();
        let local_origin = json!({"type": "local"});

        // 1. Local components and component sets (`local_component_roots` excludes `doc.library_page`).
        for (node_id, name) in local_component_roots(doc) {
            let Some(node) = doc.get(node_id) else { continue };
            let ref_str = format!("node:{}", node_id.0);
            items.push(build_component_item(node, &name, true, local_origin.clone(), ref_str, None, ComponentSource::Local(doc)));
        }

        // 2. Local styles (excluding imported library copies).
        for (style_key, style) in &doc.styles {
            if imported_local_keys.contains(style_key.as_str()) {
                continue;
            }
            let meta = doc.style_meta.get(style_key);
            let name = doc.style_name(style_key).to_string();
            let prop = style_property(doc, style_key);
            items.push(build_style_item(&name, style, meta, prop, true, local_origin.clone()));
        }

        // 3. Local variables (excluding imported library copies).
        for (var_id, var) in &doc.variables {
            if imported_local_keys.contains(var_id.as_str()) || imported_local_keys.contains(var.collection.as_str()) {
                continue;
            }
            if doc.variable_collections.get(&var.collection).is_some_and(|c| c.is_library()) {
                continue;
            }
            items.push(build_variable_item(var, &doc.variables, &doc.variable_collections, snapshots, true, local_origin.clone()));
        }

        // 4. Enabled library snapshots.
        for snap in snapshots {
            let lib_name = if !snap.name.is_empty() {
                snap.name.clone()
            } else {
                doc.library.enabled.iter().find(|r| r.id == snap.library_id).map(|r| r.name.clone()).unwrap_or_else(|| snap.library_id.clone())
            };
            let lib_origin = json!({
                "type": "library",
                "id": snap.library_id,
                "name": lib_name,
            });

            // 4a. Library components & component sets.
            for asset in &snap.assets {
                if !matches!(asset.kind, LibraryAssetKind::Component | LibraryAssetKind::ComponentSet) {
                    continue;
                }
                let Some(root_id) = asset.node else { continue };
                let Some(node) = snap.nodes.get(&root_id) else { continue };
                let local_copy = find_local_copy_of_library_component(doc, &snap.library_id, &asset.key, &asset.name);
                items.push(build_component_item(
                    node,
                    &asset.name,
                    false,
                    lib_origin.clone(),
                    asset.key.clone(),
                    local_copy,
                    ComponentSource::Snapshot(snap, doc),
                ));
            }

            // 4b. Library styles.
            let has_style_assets = snap.assets.iter().any(|a| a.kind == LibraryAssetKind::Style);
            for (style_key, style) in &snap.styles {
                if has_style_assets && !snap.assets.iter().any(|a| a.kind == LibraryAssetKind::Style && strip_prefix_or_self(&a.key, "style:") == style_key) {
                    continue;
                }
                let meta = snap.style_meta.get(style_key);
                let name = meta.map(|m| m.name.as_str()).filter(|n| !n.is_empty()).unwrap_or(style_key).to_string();
                let prop = meta.map(|m| m.property).unwrap_or(match style {
                    Style::Text(_) => StyleProperty::Text,
                    Style::Effect(_) => StyleProperty::Effect,
                    Style::Grid(_) => StyleProperty::Grid,
                    Style::Fill(_) => StyleProperty::Fill,
                });
                items.push(build_style_item(&name, style, meta, prop, false, lib_origin.clone()));
            }

            // 4c. Library variables (hidden_from_publishing variables are excluded per §8.5).
            let has_var_assets = snap.assets.iter().any(|a| a.kind == LibraryAssetKind::Variable);
            for (var_id, var) in &snap.variables {
                if var.hidden_from_publishing {
                    continue;
                }
                if has_var_assets && !snap.assets.iter().any(|a| a.kind == LibraryAssetKind::Variable && strip_prefix_or_self(&a.key, "var:") == var_id) {
                    continue;
                }
                items.push(build_variable_item(var, &snap.variables, &snap.variable_collections, &[], false, lib_origin.clone()));
            }
        }

        Self { items, libraries_searched }
    }

    fn query(&self, args: &SearchDesignSystemArgs) -> Value {
        let q_norm = args.query.trim().to_lowercase();
        let q_tokens = tokenize(&args.query);
        let prop_filter = args.property.as_deref().and_then(|key| property_kind(key).map(|expected_kind| (key, expected_kind)));

        let mut scored: Vec<(u32, &IndexedItem)> = Vec::new();
        for item in &self.items {
            if let Some(kinds) = &args.kinds {
                if !item.kind.matches_filter(kinds) {
                    continue;
                }
            }
            if let Some((prop_key, expected_kind)) = prop_filter {
                if let Some(vm) = &item.var_meta {
                    if vm.kind != expected_kind || !variable_scope_allows_property(&vm.scopes, prop_key) {
                        continue;
                    }
                }
            }
            let s = score_item(item, &q_norm, &q_tokens);
            if s > 0 {
                scored.push((s, item));
            }
        }

        scored.sort_by(|(score_a, item_a), (score_b, item_b)| {
            score_b
                .cmp(score_a)
                .then_with(|| item_a.kind.rank().cmp(&item_b.kind.rank()))
                .then_with(|| item_a.name.cmp(&item_b.name))
                .then_with(|| item_a.ref_str.cmp(&item_b.ref_str))
        });

        let total = scored.len();
        let results: Vec<Value> = scored
            .into_iter()
            .take(args.limit)
            .map(|(score, item)| {
                let mut obj = item.payload.clone();
                obj.insert("score".into(), json!(score));
                Value::Object(obj)
            })
            .collect();

        json!({
            "results": results,
            "total": total,
            "libraries_searched": self.libraries_searched,
        })
    }
}

enum ComponentSource<'a> {
    Local(&'a Document),
    Snapshot(&'a LibrarySnapshot, &'a Document),
}

impl<'a> ComponentSource<'a> {
    fn get_node(&self, id: NodeId) -> Option<&'a Node> {
        match self {
            Self::Local(doc) => doc.get(id),
            Self::Snapshot(snap, _) => snap.nodes.get(&id),
        }
    }

    fn resolve_ref_for_node(&self, id: NodeId) -> Option<String> {
        match self {
            Self::Local(doc) => {
                if let Some((ekey, e)) = library_asset_of_component(doc, id) {
                    let prefix = format!("{}/", e.library_id);
                    return Some(ekey.strip_prefix(&prefix).unwrap_or(ekey).to_string());
                }
                let n = doc.get(id)?;
                if matches!(n.kind, NodeKind::Component(_) | NodeKind::ComponentSet(_)) {
                    Some(format!("node:{}", id.0))
                } else {
                    None
                }
            }
            Self::Snapshot(snap, doc) => {
                if let Some(asset) = snap.assets.iter().find(|a| a.node == Some(id)) {
                    return Some(asset.key.clone());
                }
                if let Some(n) = snap.nodes.get(&id) {
                    if let Some(parent_id) = n.parent {
                        if let Some(asset) = snap.assets.iter().find(|a| a.node == Some(parent_id)) {
                            return Some(asset.key.clone());
                        }
                    }
                    if matches!(n.kind, NodeKind::ComponentSet(_)) {
                        return Some(format!("s{}", id.0));
                    }
                    if matches!(n.kind, NodeKind::Component(_)) {
                        return Some(format!("c{}", id.0));
                    }
                }
                if doc.get(id).is_some() {
                    return Some(format!("node:{}", id.0));
                }
                None
            }
        }
    }
}

fn find_local_copy_of_library_component(doc: &Document, library_id: &str, asset_key: &str, asset_name: &str) -> Option<String> {
    let ns_key = local_key(library_id, asset_key);
    for (k, ext) in &doc.library.external {
        if ext.library_id != library_id || !matches!(ext.kind, LibraryAssetKind::Component | LibraryAssetKind::ComponentSet) {
            continue;
        }
        let Some(nid) = ext.node else { continue };
        let Some(local_node) = doc.get(nid) else { continue };
        if k == &ns_key || k == asset_key || local_node.name == asset_name {
            return Some(format!("node:{}", nid.0));
        }
    }
    None
}

fn build_component_item(
    node: &Node,
    name: &str,
    is_local: bool,
    origin: Value,
    ref_str: String,
    local_copy: Option<String>,
    source: ComponentSource<'_>,
) -> IndexedItem {
    let is_set = matches!(node.kind, NodeKind::ComponentSet(_));
    let kind = if is_set { ItemKind::ComponentSet } else { ItemKind::Component };

    let (variant_props_json, default_variant_map) = extract_variant_schema(node, &source);
    let properties_json = extract_component_properties(node, &source);
    let slots_json = extract_slot_definitions(node, &source);

    let mut example = Map::new();
    example.insert("kind".into(), json!("instance"));
    example.insert("component".into(), json!(ref_str));
    if !default_variant_map.is_empty() {
        example.insert("variant".into(), json!(default_variant_map));
    }

    let mut payload = Map::new();
    payload.insert("kind".into(), json!(kind.as_str()));
    payload.insert("name".into(), json!(name));
    if !node.component.description.is_empty() {
        payload.insert("description".into(), json!(node.component.description));
    }
    payload.insert("origin".into(), origin);
    payload.insert("ref".into(), json!(ref_str));
    if let Some(lc) = local_copy {
        payload.insert("local_copy".into(), json!(lc));
    }
    payload.insert("variant_properties".into(), Value::Object(variant_props_json));
    payload.insert("properties".into(), Value::Array(properties_json));
    payload.insert("slots".into(), Value::Array(slots_json));
    payload.insert("example".into(), Value::Object(example));

    let path_segments = split_path_segments(name);
    let name_tokens = tokenize(name);
    IndexedItem {
        kind,
        name: name.to_string(),
        description: node.component.description.clone(),
        is_local,
        ref_str,
        path_segments,
        name_tokens,
        name_lower: name.trim().to_lowercase(),
        qualified_lower: None,
        var_meta: None,
        payload,
    }
}

fn extract_variant_schema(node: &Node, source: &ComponentSource<'_>) -> (Map<String, Value>, BTreeMap<String, String>) {
    let mut prop_order: Vec<String> = Vec::new();
    let mut options_by_prop: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut default_by_prop: BTreeMap<String, String> = BTreeMap::new();

    // 1. Seed from explicit `PropKind::Variant` definitions on the node.
    for def in &node.component.props {
        if let PropKind::Variant { options } = &def.kind {
            if !prop_order.contains(&def.name) {
                prop_order.push(def.name.clone());
            }
            let entry = options_by_prop.entry(def.name.clone()).or_default();
            for opt in options {
                if !entry.contains(opt) {
                    entry.push(opt.clone());
                }
            }
            if let PropValue::Variant(d) | PropValue::Text(d) = &def.default {
                if !d.is_empty() {
                    default_by_prop.insert(def.name.clone(), d.clone());
                }
            }
        }
    }

    // 2. Inspect child variant components (for a ComponentSet) in child order.
    // Per E15-1 §3.2, the first variant in child order is the set default.
    if matches!(node.kind, NodeKind::ComponentSet(_)) {
        for (idx, &child_id) in node.children.iter().enumerate() {
            let Some(child) = source.get_node(child_id) else { continue };
            if !matches!(child.kind, NodeKind::Component(_)) {
                continue;
            }
            let mut pairs = parse_variant_name(&child.name);
            if pairs.is_empty() && !child.component.variant_props.is_empty() {
                pairs = child.component.variant_props.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            }
            for (k, v) in pairs {
                if !prop_order.contains(&k) {
                    prop_order.push(k.clone());
                }
                let entry = options_by_prop.entry(k.clone()).or_default();
                if !entry.contains(&v) {
                    entry.push(v.clone());
                }
                if idx == 0 {
                    default_by_prop.insert(k, v);
                } else {
                    default_by_prop.entry(k).or_insert(v);
                }
            }
        }
    }

    let mut variant_props_json = Map::new();
    let mut default_map = BTreeMap::new();
    for prop_name in prop_order {
        let opts = options_by_prop.remove(&prop_name).unwrap_or_default();
        let def_val = default_by_prop.remove(&prop_name).or_else(|| opts.first().cloned()).unwrap_or_default();
        if !def_val.is_empty() {
            default_map.insert(prop_name.clone(), def_val.clone());
        }
        variant_props_json.insert(
            prop_name,
            json!({
                "options": opts,
                "default": def_val,
            }),
        );
    }

    (variant_props_json, default_map)
}

fn extract_component_properties(node: &Node, source: &ComponentSource<'_>) -> Vec<Value> {
    let mut all_defs: Vec<&ComponentPropDef> = node.component.props.iter().collect();
    if matches!(node.kind, NodeKind::ComponentSet(_)) {
        for &child_id in &node.children {
            if let Some(child) = source.get_node(child_id) {
                for def in &child.component.props {
                    if !all_defs.iter().any(|d| d.name == def.name) {
                        all_defs.push(def);
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    for def in all_defs {
        match &def.kind {
            PropKind::Boolean => {
                let default_val = match &def.default {
                    PropValue::Bool(b) => json!(b),
                    _ => json!(false),
                };
                out.push(json!({
                    "name": def.name,
                    "type": "boolean",
                    "default": default_val,
                }));
            }
            PropKind::Text => {
                let default_val = match &def.default {
                    PropValue::Text(s) => json!(s),
                    _ => json!(""),
                };
                out.push(json!({
                    "name": def.name,
                    "type": "text",
                    "default": default_val,
                }));
            }
            PropKind::InstanceSwap { preferred } => {
                let default_val = match &def.default {
                    PropValue::Instance(id) => source.resolve_ref_for_node(*id).map(|r| json!(r)).unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                let recommended: Vec<String> = preferred.iter().filter_map(|id| source.resolve_ref_for_node(*id)).collect();
                out.push(json!({
                    "name": def.name,
                    "type": "instance_swap",
                    "default": default_val,
                    "recommended": recommended,
                }));
            }
            PropKind::Variant { .. } | PropKind::Slot => {}
        }
    }
    out
}

fn extract_slot_definitions(node: &Node, source: &ComponentSource<'_>) -> Vec<Value> {
    fn collect_from_holder(holder: &Node, slot_names: &mut Vec<String>, configs: &mut BTreeMap<String, SlotPropConfig>) {
        for def in &holder.component.props {
            if matches!(def.kind, PropKind::Slot) && !slot_names.contains(&def.name) {
                slot_names.push(def.name.clone());
            }
        }
        for (name, cfg) in &holder.component.slot_configs {
            if !slot_names.contains(name) {
                slot_names.push(name.clone());
            }
            configs.entry(name.clone()).or_insert_with(|| cfg.clone());
        }
    }

    let mut slot_names: Vec<String> = Vec::new();
    let mut configs: BTreeMap<String, SlotPropConfig> = BTreeMap::new();

    collect_from_holder(node, &mut slot_names, &mut configs);

    // Also inspect subtree for child variants or slot frames (`component.is_slot`).
    let mut stack: Vec<NodeId> = node.children.iter().rev().copied().collect();
    while let Some(nid) = stack.pop() {
        let Some(child) = source.get_node(nid) else { continue };
        collect_from_holder(child, &mut slot_names, &mut configs);
        if child.component.is_slot {
            let slot_name = child.component.bindings.get(&OverrideField::SlotContent).cloned().unwrap_or_else(|| child.name.clone());
            if !slot_name.is_empty() && !slot_names.contains(&slot_name) {
                slot_names.push(slot_name);
            }
        }
        stack.extend(child.children.iter().rev().copied());
    }

    let mut out = Vec::new();
    for name in slot_names {
        let cfg = configs.remove(&name).unwrap_or_default();
        let mut recommended: Vec<String> = Vec::new();
        for &pid in &cfg.preferred {
            if let Some(r) = source.resolve_ref_for_node(pid) {
                if !recommended.contains(&r) {
                    recommended.push(r);
                }
            }
        }
        out.push(json!({
            "name": name,
            "min_children": cfg.min_children,
            "max_children": cfg.max_children,
            "strict": cfg.allow_preferred_values_only.unwrap_or(false),
            "recommended": recommended,
        }));
    }
    out
}

fn build_style_item(name: &str, style: &Style, meta: Option<&StyleMeta>, prop: StyleProperty, is_local: bool, origin: Value) -> IndexedItem {
    let ref_str = format!("style:{name}");
    let style_type = match prop {
        StyleProperty::Fill => "fill",
        StyleProperty::Stroke => "stroke",
        StyleProperty::Text => "text",
        StyleProperty::Effect => "effect",
        StyleProperty::Grid => "grid",
    };
    let summary = summarize_style(style);
    let example = match prop {
        StyleProperty::Text => json!({"text_style": name}),
        StyleProperty::Effect => json!({"effect_style": name}),
        StyleProperty::Stroke => json!({"stroke": ref_str}),
        StyleProperty::Fill | StyleProperty::Grid => json!({"fill": ref_str}),
    };
    let description = meta.map(|m| m.description.clone()).unwrap_or_default();

    let mut payload = Map::new();
    payload.insert("kind".into(), json!("style"));
    payload.insert("name".into(), json!(name));
    if !description.is_empty() {
        payload.insert("description".into(), json!(description));
    }
    payload.insert("origin".into(), origin);
    payload.insert("ref".into(), json!(ref_str));
    payload.insert("style_type".into(), json!(style_type));
    payload.insert("summary".into(), json!(summary));
    payload.insert("example".into(), example);

    IndexedItem {
        kind: ItemKind::Style,
        name: name.to_string(),
        description,
        is_local,
        ref_str,
        path_segments: split_path_segments(name),
        name_tokens: tokenize(name),
        name_lower: name.trim().to_lowercase(),
        qualified_lower: None,
        var_meta: None,
        payload,
    }
}

fn summarize_style(style: &Style) -> String {
    match style {
        Style::Fill(paints) => {
            if let Some(p) = paints.iter().find(|p| p.visible) {
                match &p.kind {
                    PaintKind::Solid(c) => format!("#{}", c.to_hex()),
                    PaintKind::LinearGradient { .. } => "linear-gradient".into(),
                    PaintKind::RadialGradient { .. } => "radial-gradient".into(),
                    PaintKind::AngularGradient { .. } => "angular-gradient".into(),
                    PaintKind::DiamondGradient { .. } => "diamond-gradient".into(),
                    PaintKind::Image { .. } => "image".into(),
                }
            } else {
                "empty".into()
            }
        }
        Style::Text(ts) => format!("{} {}px/{}", ts.font_family, ts.font_size, ts.font_weight),
        Style::Effect(fx) => format!("{} effect(s)", fx.len()),
        Style::Grid(grids) => format!("{} grid(s)", grids.len()),
    }
}

fn build_variable_item(
    var: &Variable,
    variables: &BTreeMap<String, Variable>,
    collections: &BTreeMap<String, VariableCollection>,
    snapshots: &[LibrarySnapshot],
    is_local: bool,
    origin: Value,
) -> IndexedItem {
    let coll = collections.get(&var.collection);
    let collection_name = coll.map(|c| c.name.clone()).unwrap_or_else(|| var.collection.clone());
    let resolved_kind = resolve_variable_kind(var, variables);
    let ref_str = format_variable_ref(var, &collection_name, variables, collections, snapshots);

    let mut values_map = Map::new();
    if let Some(c) = coll {
        for mode in &c.modes {
            if let Some(val) = resolve_variable_in_mode(var, &mode.id, &c.default_mode, variables, collections) {
                values_map.insert(mode.name.clone(), format_variable_value(&val));
            }
        }
    } else {
        for (mode_id, val) in &var.values {
            values_map.insert(mode_id.clone(), format_variable_value(val));
        }
    }

    let example = variable_example(&ref_str, resolved_kind, &var.scopes);
    let kind_str = match resolved_kind {
        VariableKind::Color => "color",
        VariableKind::Number => "number",
        VariableKind::String => "string",
        VariableKind::Boolean => "boolean",
    };

    let mut payload = Map::new();
    payload.insert("kind".into(), json!("variable"));
    payload.insert("name".into(), json!(var.name));
    payload.insert("collection".into(), json!(collection_name));
    payload.insert("variable_kind".into(), json!(kind_str));
    if !var.description.is_empty() {
        payload.insert("description".into(), json!(var.description));
    }
    payload.insert("origin".into(), origin);
    payload.insert("ref".into(), json!(ref_str));
    payload.insert("values".into(), Value::Object(values_map));
    payload.insert("code_syntax".into(), json!(var.code_syntax));
    payload.insert("example".into(), example);

    let mut path_segments = split_path_segments(&collection_name);
    path_segments.extend(split_path_segments(&var.name));
    let mut name_tokens = tokenize(&collection_name);
    for t in tokenize(&var.name) {
        if !name_tokens.contains(&t) {
            name_tokens.push(t);
        }
    }

    let qualified_lower = Some(format!("{}/{}", collection_name.trim().to_lowercase(), var.name.trim().to_lowercase()));
    let _ = is_slot_container;

    IndexedItem {
        kind: ItemKind::Variable,
        name: var.name.clone(),
        description: var.description.clone(),
        is_local,
        ref_str,
        path_segments,
        name_tokens,
        name_lower: var.name.trim().to_lowercase(),
        qualified_lower,
        var_meta: Some(VariableFilterMeta { kind: resolved_kind, scopes: var.scopes.clone() }),
        payload,
    }
}

fn resolve_variable_kind(var: &Variable, variables: &BTreeMap<String, Variable>) -> VariableKind {
    let mut cur = var;
    for _ in 0..16 {
        if let Some(VariableValue::Alias(next_id)) = cur.values.values().next() {
            if let Some(next_var) = variables.get(next_id) {
                cur = next_var;
                continue;
            }
        }
        return cur.kind;
    }
    var.kind
}

fn resolve_variable_in_mode(
    var: &Variable,
    mode_id: &str,
    default_mode_id: &str,
    variables: &BTreeMap<String, Variable>,
    collections: &BTreeMap<String, VariableCollection>,
) -> Option<VariableValue> {
    let mut cur_var = var;
    let mut cur_mode = mode_id;
    let mut cur_default = default_mode_id;
    for _ in 0..16 {
        let val = cur_var.values.get(cur_mode).or_else(|| cur_var.values.get(cur_default)).or_else(|| cur_var.values.values().next())?;
        match val {
            VariableValue::Alias(next_id) => {
                cur_var = variables.get(next_id)?;
                if let Some(c) = collections.get(&cur_var.collection) {
                    // Match mode by id or by name if crossing collections, falling back to target collection's default_mode.
                    if !cur_var.values.contains_key(cur_mode) {
                        cur_mode = &c.default_mode;
                    }
                    cur_default = &c.default_mode;
                }
            }
            other => return Some(other.clone()),
        }
    }
    None
}

fn format_variable_value(val: &VariableValue) -> Value {
    match val {
        VariableValue::Color(c) => json!(format!("#{}", c.to_hex())),
        VariableValue::Number(n) => json!(n),
        VariableValue::String(s) => json!(s),
        VariableValue::Boolean(b) => json!(b),
        VariableValue::Alias(a) => json!(format!("alias:{a}")),
    }
}

/// Formats `var:<collection>/<name>` or falls back to `var:id:<id>` when the path is ambiguous (`E15-3` §3.1, `T7.9` §3.3).
fn format_variable_ref(
    var: &Variable,
    collection_name: &str,
    variables: &BTreeMap<String, Variable>,
    collections: &BTreeMap<String, VariableCollection>,
    _snapshots: &[LibrarySnapshot],
) -> String {
    let candidate_path = format!("{collection_name}/{}", var.name);
    if is_unambiguous_var_path(&candidate_path, var, variables, collections) {
        format!("var:{candidate_path}")
    } else {
        format!("var:id:{}", var.id)
    }
}

fn is_unambiguous_var_path(path: &str, target: &Variable, variables: &BTreeMap<String, Variable>, collections: &BTreeMap<String, VariableCollection>) -> bool {
    // Find longest collection id or name `c` such that `path` starts with `{c}/`.
    let mut best_len = 0usize;
    let mut best_colls: Vec<&str> = Vec::new();
    for coll in collections.values() {
        for label in [coll.id.as_str(), coll.name.as_str()] {
            if label.is_empty() {
                continue;
            }
            if let Some(rest) = path.strip_prefix(label).and_then(|r| r.strip_prefix('/')) {
                if rest.is_empty() {
                    continue;
                }
                if label.len() > best_len {
                    best_len = label.len();
                    best_colls.clear();
                    best_colls.push(&coll.id);
                } else if label.len() == best_len && !best_colls.contains(&coll.id.as_str()) {
                    best_colls.push(&coll.id);
                }
            }
        }
    }
    if best_colls.len() != 1 || best_colls[0] != target.collection {
        return false;
    }
    let remainder = &path[best_len + 1..];
    let matches_in_coll = variables.values().filter(|v| v.collection == target.collection && v.name == remainder).count();
    matches_in_coll == 1
}

fn variable_example(ref_str: &str, kind: VariableKind, scopes: &[String]) -> Value {
    let has_scope = |target: &str| scopes.iter().any(|s| s.eq_ignore_ascii_case(target));
    match kind {
        VariableKind::Color => {
            if has_scope("STROKE_COLOR")
                && !has_scope("ALL_FILLS")
                && !has_scope("FRAME_FILL")
                && !has_scope("SHAPE_FILL")
                && !has_scope("TEXT_FILL")
                && !has_scope("FILL")
            {
                json!({"stroke": ref_str})
            } else {
                json!({"fill": ref_str})
            }
        }
        VariableKind::Number => {
            if has_scope("CORNER_RADIUS") {
                json!({"corner_radius": ref_str})
            } else if has_scope("GAP") {
                json!({"layout": {"gap": ref_str}})
            } else if has_scope("FONT_SIZE") {
                json!({"font_size": ref_str})
            } else if has_scope("LINE_HEIGHT") {
                json!({"line_height": ref_str})
            } else if has_scope("STROKE_FLOAT") {
                json!({"stroke_weight": ref_str})
            } else if has_scope("OPACITY") || has_scope("COLOR_OPACITY") {
                json!({"opacity": ref_str})
            } else {
                json!({"width": ref_str})
            }
        }
        VariableKind::String => json!({"text": ref_str}),
        VariableKind::Boolean => json!({"visible": ref_str}),
    }
}

fn variable_scope_allows_property(scopes: &[String], prop_key: &str) -> bool {
    scope_allows(scopes, prop_key, &NodeKind::Frame(FrameProps::default()))
        || scope_allows(scopes, prop_key, &NodeKind::Text(TextProps::default()))
        || scope_allows(scopes, prop_key, &NodeKind::Rectangle)
}

fn score_item(item: &IndexedItem, q_norm: &str, q_tokens: &[String]) -> u32 {
    let mut lexical: u32 = 0;

    let exact_name = item.name_lower == q_norm || item.qualified_lower.as_deref() == Some(q_norm);
    let starts_with = item.name_lower.starts_with(q_norm) || item.qualified_lower.as_deref().is_some_and(|ql| ql.starts_with(q_norm));

    if exact_name {
        lexical += 100;
    }
    if starts_with {
        lexical += 60;
    }

    let all_tokens_in_segments = !q_tokens.is_empty()
        && q_tokens
            .iter()
            .all(|qt| item.path_segments.iter().any(|seg| seg.contains(qt.as_str())) || item.name_tokens.iter().any(|nt| nt.contains(qt.as_str())));

    if all_tokens_in_segments && !exact_name {
        lexical += 40;
    }

    // Description token matches: +10 per query token found in description.
    if !item.description.is_empty() {
        let desc_lower = item.description.to_lowercase();
        for qt in q_tokens {
            if desc_lower.contains(qt.as_str()) {
                lexical += 10;
            }
        }
    }

    // Token fuzzy match (Levenshtein <= 1 for tokens of length >= 4) in name: +15.
    if !all_tokens_in_segments {
        let has_fuzzy = q_tokens
            .iter()
            .filter(|qt| qt.chars().count() >= 4)
            .any(|qt| !item.name_tokens.iter().any(|nt| nt.contains(qt.as_str())) && item.name_tokens.iter().any(|nt| levenshtein_at_most_one(qt, nt)));
        if has_fuzzy {
            lexical += 15;
        }
    }

    if lexical == 0 {
        return 0;
    }
    if item.is_local {
        lexical + 5
    } else {
        lexical
    }
}

fn split_path_segments(s: &str) -> Vec<String> {
    s.split('/').map(|seg| seg.trim().to_lowercase()).filter(|seg| !seg.is_empty()).collect()
}

fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || matches!(c, '/' | '-' | '_')).map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).collect()
}

fn strip_prefix_or_self<'a>(s: &'a str, prefix: &str) -> &'a str {
    s.strip_prefix(prefix).unwrap_or(s)
}

/// Returns true when the Levenshtein edit distance between `a` and `b` is `<= 1`.
fn levenshtein_at_most_one(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let (m, n) = (ac.len(), bc.len());
    if m.abs_diff(n) > 1 {
        return false;
    }
    if m == n {
        let diffs = ac.iter().zip(bc.iter()).filter(|(x, y)| x != y).count();
        return diffs <= 1;
    }
    // Length differs by 1: check single insertion/deletion.
    let (shorter, longer) = if m < n { (&ac, &bc) } else { (&bc, &ac) };
    let (mut i, mut j, mut edits) = (0usize, 0usize, 0usize);
    while i < shorter.len() && j < longer.len() {
        if shorter[i] == longer[j] {
            i += 1;
            j += 1;
        } else {
            edits += 1;
            if edits > 1 {
                return false;
            }
            j += 1;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::library_source::clear_snapshot_cache;
    use designos_doc::{create_instance, import_asset, CodeSyntaxPlatform, Color, LibraryRef, Paint, VariableMode, Vec2};
    use designos_library::Library;

    fn make_temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("designos_t7_9_{tag}_{}", designos_library::new_id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build_brand_kit_doc() -> Document {
        let mut doc = Document::new("Brand kit");
        let page = doc.add_page("Components");

        // Button ComponentSet with variants Size={sm,md,lg}, State={default,hover} and Text prop "Label"
        let set_id = doc.alloc_id();
        let mut set_node = Node::new(set_id, NodeKind::ComponentSet(FrameProps::default()));
        set_node.name = "Button".into();
        set_node.component.description = "Primary interactive action".into();
        set_node.component.props = vec![
            ComponentPropDef {
                name: "Size".into(),
                kind: PropKind::Variant { options: vec!["sm".into(), "md".into(), "lg".into()] },
                default: PropValue::Variant("md".into()),
            },
            ComponentPropDef {
                name: "State".into(),
                kind: PropKind::Variant { options: vec!["default".into(), "hover".into()] },
                default: PropValue::Variant("default".into()),
            },
            ComponentPropDef { name: "Label".into(), kind: PropKind::Text, default: PropValue::Text("Button".into()) },
        ];
        doc.insert(page, set_node).unwrap();

        // First variant in child order is Size=md, State=default
        for (size, state) in [("md", "default"), ("sm", "default"), ("lg", "default"), ("md", "hover")] {
            let vid = doc.alloc_id();
            let mut v = Node::new(vid, NodeKind::Component(FrameProps::default()));
            v.name = format!("Size={size}, State={state}");
            v.size = Vec2::new(120.0, 40.0);
            doc.insert(set_id, v).unwrap();
        }

        // Shared text style "Heading L"
        doc.styles.insert("s_heading".into(), Style::Text(designos_doc::TextStyle::default()));
        doc.style_meta.insert(
            "s_heading".into(),
            StyleMeta { name: "Heading L".into(), key: Some("k_heading".into()), description: "Large section heading".into(), property: StyleProperty::Text },
        );

        doc
    }

    #[test]
    fn ranked_with_origin() {
        clear_snapshot_cache();
        clear_search_index_cache();
        let root = make_temp_dir("ranked_origin");
        let lib = Library::open(&root).unwrap();

        let brand_doc = build_brand_kit_doc();
        lib.publish_library("lib-7f3a", &brand_doc, "v1").unwrap();

        let mut host = Document::new("App");
        let page = host.add_page("Page 1");
        host.library.enabled.push(LibraryRef { id: "lib-7f3a".into(), name: "Brand kit".into(), version: 1 });

        // Add a less-specific local component "ButtonGroup" and local Color/Primary variable
        let bg_id = host.alloc_id();
        let mut bg = Node::new(bg_id, NodeKind::Component(FrameProps::default()));
        bg.name = "Button/Toolbar".into();
        host.insert(page, bg).unwrap();

        let out = search_design_system_at(&host, json!({"query": "button"}), &root).unwrap();
        let results = out["results"].as_array().unwrap();
        assert!(!results.is_empty());
        assert_eq!(out["libraries_searched"], json!(["local", "lib-7f3a"]));

        // Exact match "Button" set from Brand kit ranks first with score 160
        let first = &results[0];
        assert_eq!(first["kind"], "component_set");
        assert_eq!(first["name"], "Button");
        assert_eq!(first["score"], 160);
        assert_eq!(first["origin"], json!({"type": "library", "id": "lib-7f3a", "name": "Brand kit"}));
        assert_eq!(
            first["variant_properties"],
            json!({
                "Size": {"options": ["sm", "md", "lg"], "default": "md"},
                "State": {"options": ["default", "hover"], "default": "default"}
            })
        );
        assert_eq!(first["properties"], json!([{"name": "Label", "type": "text", "default": "Button"}]));

        // If a local component also has the exact name "Button", local ranks higher (+5 -> 165)
        let local_btn_id = host.alloc_id();
        let mut local_btn = Node::new(local_btn_id, NodeKind::Component(FrameProps::default()));
        local_btn.name = "Button".into();
        host.insert(page, local_btn).unwrap();

        let out2 = search_design_system_at(&host, json!({"query": "button"}), &root).unwrap();
        let results2 = out2["results"].as_array().unwrap();
        assert_eq!(results2[0]["origin"], json!({"type": "local"}));
        assert_eq!(results2[0]["score"], 165);
        assert_eq!(results2[1]["origin"]["id"], "lib-7f3a");
        assert_eq!(results2[1]["score"], 160);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn disabled_excluded() {
        clear_snapshot_cache();
        clear_search_index_cache();
        let root = make_temp_dir("disabled_excluded");
        let lib = Library::open(&root).unwrap();

        let brand_doc = build_brand_kit_doc();
        lib.publish_library("lib-disabled", &brand_doc, "v1").unwrap();

        // Host does NOT enable `lib-disabled`
        let mut host = Document::new("App");
        host.add_page("Page 1");

        let out = search_design_system_at(&host, json!({"query": "button"}), &root).unwrap();
        assert_eq!(out["results"], json!([]));
        assert_eq!(out["total"], 0);
        assert_eq!(out["libraries_searched"], json!(["local"]));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn result_pastes_into_apply_spec() {
        clear_snapshot_cache();
        clear_search_index_cache();
        let root = make_temp_dir("paste_apply_spec");
        let lib = Library::open(&root).unwrap();

        let brand_doc = build_brand_kit_doc();
        lib.publish_library("lib-7f3a", &brand_doc, "v1").unwrap();

        let mut host = Document::new("App");
        let page = host.add_page("Page 1");
        host.library.enabled.push(LibraryRef { id: "lib-7f3a".into(), name: "Brand kit".into(), version: 1 });

        let out = search_design_system_at(&host, json!({"query": "button"}), &root).unwrap();
        let first = &out["results"].as_array().unwrap()[0];
        let comp_ref = first["ref"].as_str().unwrap();
        let example = &first["example"];

        // Verify the example is a complete, edit-free instance spec fragment
        assert_eq!(
            example,
            &json!({
                "kind": "instance",
                "component": comp_ref,
                "variant": {
                    "Size": "md",
                    "State": "default"
                }
            })
        );

        // Verify that applying/importing the example's `component` ref and `variant` produces
        // an instance linked to the exact default variant (`Size=md, State=default`) without edits.
        let snaps = enabled_snapshots_from_root(&host, &root);
        let snap = &snaps[0];
        let import_cmd = import_asset(&mut host, snap, comp_ref, None);
        import_cmd.apply(&mut host).unwrap();

        let ext_entry = host.library.external.get(&local_key("lib-7f3a", comp_ref)).unwrap();
        let imported_set_id = ext_entry.node.unwrap();
        let requested_variant: BTreeMap<String, String> = serde_json::from_value(example["variant"].clone()).unwrap();
        let target_variant_id = host
            .get(imported_set_id)
            .unwrap()
            .children
            .iter()
            .copied()
            .find(|&cid| {
                let pairs: BTreeMap<String, String> = parse_variant_name(&host.get(cid).unwrap().name).into_iter().collect();
                pairs == requested_variant
            })
            .expect("example variant map must match an existing variant in the imported ComponentSet");

        let inst_cmd = create_instance(&mut host, target_variant_id, page, Vec2::new(0.0, 0.0));
        inst_cmd.apply(&mut host).unwrap();

        let inst_id = *host.get(page).unwrap().children.last().unwrap();
        let inst_node = host.get(inst_id).unwrap();
        assert!(matches!(inst_node.kind, NodeKind::Instance { component: Some(c), .. } if c == target_variant_id));

        // Re-running search after import returns the library asset once with `local_copy: "node:<id>"` (§8.2)
        let out_after = search_design_system_at(&host, json!({"query": "button"}), &root).unwrap();
        let results_after = out_after["results"].as_array().unwrap();
        assert_eq!(results_after.len(), 1);
        assert_eq!(results_after[0]["ref"], comp_ref);
        assert_eq!(results_after[0]["local_copy"], format!("node:{}", imported_set_id.0));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn variable_scope_filter() {
        clear_snapshot_cache();
        clear_search_index_cache();
        let root = make_temp_dir("var_scope");
        let mut doc = Document::new("Tokens");
        doc.add_page("Page 1");

        doc.variable_collections.insert(
            "c_tokens".into(),
            VariableCollection {
                id: "c_tokens".into(),
                name: "Spacing".into(),
                modes: vec![VariableMode { id: "m_light".into(), name: "Light".into() }, VariableMode { id: "m_dark".into(), name: "Dark".into() }],
                default_mode: "m_light".into(),
                variables: vec!["v_gap".into(), "v_radius".into(), "v_color".into(), "v_unscoped".into()],
                key: None,
                source_library_key: None,
            },
        );

        // 1. GAP-compatible number variable
        let mut code_syntax = BTreeMap::new();
        code_syntax.insert(CodeSyntaxPlatform::Web, "var(--spacing-md)".into());
        doc.variables.insert(
            "v_gap".into(),
            Variable {
                id: "v_gap".into(),
                name: "space-md".into(),
                collection: "c_tokens".into(),
                kind: VariableKind::Number,
                values: BTreeMap::from([("m_light".into(), VariableValue::Number(16.0)), ("m_dark".into(), VariableValue::Number(16.0))]),
                description: "Medium spacing token".into(),
                scopes: vec!["GAP".into()],
                hidden_from_publishing: false,
                key: None,
                code_syntax,
            },
        );

        // 2. CORNER_RADIUS-only number variable (incompatible with itemSpacing)
        doc.variables.insert(
            "v_radius".into(),
            Variable {
                id: "v_radius".into(),
                name: "space-radius".into(),
                collection: "c_tokens".into(),
                kind: VariableKind::Number,
                values: BTreeMap::from([("m_light".into(), VariableValue::Number(8.0)), ("m_dark".into(), VariableValue::Number(8.0))]),
                description: String::new(),
                scopes: vec!["CORNER_RADIUS".into()],
                hidden_from_publishing: false,
                key: None,
                code_syntax: Default::default(),
            },
        );

        // 3. Color variable with "space" in name (incompatible with itemSpacing)
        doc.variables.insert(
            "v_color".into(),
            Variable {
                id: "v_color".into(),
                name: "space-bg".into(),
                collection: "c_tokens".into(),
                kind: VariableKind::Color,
                values: BTreeMap::from([
                    ("m_light".into(), VariableValue::Color(Color::from_hex("#0D99FF").unwrap())),
                    ("m_dark".into(), VariableValue::Color(Color::from_hex("#3582FF").unwrap())),
                ]),
                description: String::new(),
                scopes: vec!["ALL_FILLS".into()],
                hidden_from_publishing: false,
                key: None,
                code_syntax: Default::default(),
            },
        );

        // 4. Unscoped number variable (empty scopes = allowed everywhere)
        doc.variables.insert(
            "v_unscoped".into(),
            Variable {
                id: "v_unscoped".into(),
                name: "space-any".into(),
                collection: "c_tokens".into(),
                kind: VariableKind::Number,
                values: BTreeMap::from([("m_light".into(), VariableValue::Number(24.0)), ("m_dark".into(), VariableValue::Number(24.0))]),
                description: String::new(),
                scopes: vec![],
                hidden_from_publishing: false,
                key: None,
                code_syntax: Default::default(),
            },
        );

        let out = search_design_system_at(
            &doc,
            json!({
                "query": "space",
                "kinds": ["variable"],
                "property": "itemSpacing"
            }),
            &root,
        )
        .unwrap();

        let results = out["results"].as_array().unwrap();
        let names: Vec<&str> = results.iter().map(|r| r["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["space-any", "space-md"]);
        assert!(!names.contains(&"space-bg"), "colour variables must be excluded for itemSpacing");
        assert!(!names.contains(&"space-radius"), "CORNER_RADIUS number variables must be excluded for itemSpacing");

        // Verify variable payload fields (ref, values per mode, code_syntax, example)
        let gap_item = results.iter().find(|r| r["name"] == "space-md").unwrap();
        assert_eq!(gap_item["ref"], "var:Spacing/space-md");
        assert_eq!(gap_item["values"], json!({"Light": 16.0, "Dark": 16.0}));
        assert_eq!(gap_item["code_syntax"], json!({"WEB": "var(--spacing-md)"}));
        assert_eq!(gap_item["example"], json!({"layout": {"gap": "var:Spacing/space-md"}}));

        // Verify colour variable formatting when queried with fills/0
        let fill_out = search_design_system_at(
            &doc,
            json!({
                "query": "space-bg",
                "kinds": ["variable"],
                "property": "fills/0"
            }),
            &root,
        )
        .unwrap();
        let fill_res = &fill_out["results"].as_array().unwrap()[0];
        assert_eq!(fill_res["values"], json!({"Light": "#0D99FF", "Dark": "#3582FF"}));
        assert_eq!(fill_res["example"], json!({"fill": "var:Spacing/space-bg"}));

        // Unknown property key returns UNKNOWN_PROPERTY_KEY with valid_options
        let err = search_design_system_at(&doc, json!({"query": "space", "property": "notAProperty"}), &root).unwrap_err();
        assert!(err.starts_with("UNKNOWN_PROPERTY_KEY:"), "{err}");
        assert!(err.contains("itemSpacing"), "{err}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn slot_definitions_included() {
        clear_snapshot_cache();
        clear_search_index_cache();
        let root = make_temp_dir("slots_included");
        let mut doc = Document::new("System");
        let page = doc.add_page("Page 1");

        // Recommended child components
        let badge_id = doc.alloc_id();
        let mut badge = Node::new(badge_id, NodeKind::Component(FrameProps::default()));
        badge.name = "Badge".into();
        doc.insert(page, badge).unwrap();

        let avatar_id = doc.alloc_id();
        let mut avatar = Node::new(avatar_id, NodeKind::Component(FrameProps::default()));
        avatar.name = "Avatar".into();
        doc.insert(page, avatar).unwrap();

        // Card component set with a "Header" slot recommending Badge and Avatar
        let card_set_id = doc.alloc_id();
        let mut card_set = Node::new(card_set_id, NodeKind::ComponentSet(FrameProps::default()));
        card_set.name = "Card".into();
        card_set.component.props.push(ComponentPropDef { name: "Header".into(), kind: PropKind::Slot, default: PropValue::Slot(vec![]) });
        card_set.component.slot_configs.insert(
            "Header".into(),
            SlotPropConfig {
                min_children: Some(1),
                max_children: Some(2),
                allow_preferred_values_only: Some(true),
                preferred: vec![badge_id, avatar_id],
                ..Default::default()
            },
        );
        doc.insert(page, card_set).unwrap();

        let card_var_id = doc.alloc_id();
        let mut card_var = Node::new(card_var_id, NodeKind::Component(FrameProps::default()));
        card_var.name = "Elevation=raised".into();
        doc.insert(card_set_id, card_var).unwrap();

        let out = search_design_system_at(&doc, json!({"query": "card"}), &root).unwrap();
        let results = out["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        let card = &results[0];
        assert_eq!(card["kind"], "component_set");
        assert_eq!(card["ref"], format!("node:{}", card_set_id.0));

        let slots = card["slots"].as_array().unwrap();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0]["name"], "Header");
        assert_eq!(slots[0]["min_children"], 1);
        assert_eq!(slots[0]["max_children"], 2);
        assert_eq!(slots[0]["strict"], true);
        let rec = slots[0]["recommended"].as_array().unwrap();
        assert_eq!(rec, &vec![json!(format!("node:{}", badge_id.0)), json!(format!("node:{}", avatar_id.0))]);

        // Verify every recommended ref resolves to a real Component in `doc`
        for r in rec {
            let id_num: u64 = r.as_str().unwrap().strip_prefix("node:").unwrap().parse().unwrap();
            assert!(matches!(doc.get(NodeId(id_num)).map(|n| &n.kind), Some(NodeKind::Component(_))));
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn edge_cases_hidden_vars_300_variants_description_and_fuzzy() {
        clear_snapshot_cache();
        clear_search_index_cache();
        let root = make_temp_dir("edge_cases");
        let lib = Library::open(&root).unwrap();

        // Library with a hidden-from-publishing variable and a published variable
        let mut lib_doc = Document::new("Tokens Lib");
        lib_doc.add_page("P");
        lib_doc.variable_collections.insert(
            "c1".into(),
            VariableCollection {
                id: "c1".into(),
                name: "Brand".into(),
                modes: vec![VariableMode { id: "m1".into(), name: "Default".into() }],
                default_mode: "m1".into(),
                variables: vec!["v_pub".into(), "v_secret".into()],
                key: None,
                source_library_key: None,
            },
        );
        lib_doc.variables.insert(
            "v_pub".into(),
            Variable {
                id: "v_pub".into(),
                name: "accent-token".into(),
                collection: "c1".into(),
                kind: VariableKind::Color,
                values: BTreeMap::from([("m1".into(), VariableValue::Color(Color::rgb(10, 20, 30)))]),
                description: String::new(),
                scopes: vec![],
                hidden_from_publishing: false,
                key: None,
                code_syntax: Default::default(),
            },
        );
        lib_doc.variables.insert(
            "v_secret".into(),
            Variable {
                id: "v_secret".into(),
                name: "accent-secret".into(),
                collection: "c1".into(),
                kind: VariableKind::Color,
                values: BTreeMap::from([("m1".into(), VariableValue::Color(Color::rgb(40, 50, 60)))]),
                description: String::new(),
                scopes: vec![],
                hidden_from_publishing: true,
                key: None,
                code_syntax: Default::default(),
            },
        );
        lib.publish_library("lib-tokens", &lib_doc, "v1").unwrap();

        let mut host = Document::new("Host");
        let page = host.add_page("Page 1");
        host.library.enabled.push(LibraryRef { id: "lib-tokens".into(), name: "Tokens Lib".into(), version: 1 });

        // Component set with 300 variants (10 sizes x 10 tones x 3 states)
        let mega_id = host.alloc_id();
        let mut mega = Node::new(mega_id, NodeKind::ComponentSet(FrameProps::default()));
        mega.name = "MegaChip".into();
        mega.component.description = "Interactive filter pill tag".into();
        host.insert(page, mega).unwrap();
        for s in 0..10 {
            for t in 0..10 {
                for st in ["rest", "hover", "active"] {
                    let vid = host.alloc_id();
                    let mut v = Node::new(vid, NodeKind::Component(FrameProps::default()));
                    v.name = format!("Size=s{s}, Tone=t{t}, State={st}");
                    host.insert(mega_id, v).unwrap();
                }
            }
        }
        assert_eq!(host.get(mega_id).unwrap().children.len(), 300);

        // Style for testing style output
        host.styles.insert("st_fill".into(), Style::Fill(vec![Paint::solid(Color::from_hex("#FF5500").unwrap())]));
        host.style_meta.insert(
            "st_fill".into(),
            StyleMeta { name: "Brand/Warm".into(), key: None, description: "Warm orange fill".into(), property: StyleProperty::Fill },
        );

        // Edge case 5: Hidden library variable excluded
        let var_out = search_design_system_at(&host, json!({"query": "accent"}), &root).unwrap();
        let var_results = var_out["results"].as_array().unwrap();
        assert_eq!(var_results.len(), 1);
        assert_eq!(var_results[0]["name"], "accent-token");

        // Edge case 3: 300 variants -> only unique option lists returned
        let mega_out = search_design_system_at(&host, json!({"query": "MegaChip"}), &root).unwrap();
        let mega_res = &mega_out["results"].as_array().unwrap()[0];
        assert_eq!(mega_res["variant_properties"]["Size"]["options"].as_array().unwrap().len(), 10);
        assert_eq!(mega_res["variant_properties"]["Tone"]["options"].as_array().unwrap().len(), 10);
        assert_eq!(mega_res["variant_properties"]["State"]["options"].as_array().unwrap().len(), 3);
        assert_eq!(mega_res["variant_properties"]["Size"]["default"], "s0");

        // Edge case 4: Query matching only description
        let desc_out = search_design_system_at(&host, json!({"query": "pill"}), &root).unwrap();
        let desc_results = desc_out["results"].as_array().unwrap();
        assert_eq!(desc_results.len(), 1);
        assert_eq!(desc_results[0]["name"], "MegaChip");
        assert_eq!(desc_results[0]["score"], 15); // +10 description + 5 local

        // Fuzzy match (Levenshtein == 1 on token >= 4 chars: "megachp" -> "megachip")
        let fuzzy_out = search_design_system_at(&host, json!({"query": "megachp"}), &root).unwrap();
        let fuzzy_results = fuzzy_out["results"].as_array().unwrap();
        assert_eq!(fuzzy_results.len(), 1);
        assert_eq!(fuzzy_results[0]["name"], "MegaChip");
        assert_eq!(fuzzy_results[0]["score"], 20); // +15 fuzzy + 5 local

        // Style result check
        let style_out = search_design_system_at(&host, json!({"query": "Warm", "kinds": ["style"]}), &root).unwrap();
        let style_res = &style_out["results"].as_array().unwrap()[0];
        assert_eq!(style_res["ref"], "style:Brand/Warm");
        assert_eq!(style_res["style_type"], "fill");
        assert_eq!(style_res["summary"], "#FF5500");
        assert_eq!(style_res["example"], json!({"fill": "style:Brand/Warm"}));

        let _ = std::fs::remove_dir_all(&root);
    }
}
