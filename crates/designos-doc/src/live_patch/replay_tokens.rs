//! Editor side: replay variable and shared-style deltas on the full document, and check
//! variable / style references on replayed node edits so missing tokens do not leave dangling links.

use super::{LivePatchReport, LivePatchState};
use crate::{Command, Document, Node, Style, StyleMeta, Variable, VariableCollection};
use std::collections::BTreeMap;

/// Merges the agent's variable and collection changes into the editor's own maps by id.
///
/// When `is_spec_seed` is true (a patch that also inserts nodes, such as `apply_spec` seeding
/// preset tokens on a copy that may have predated the user's edits), only ids the editor lacks
/// are added. When `is_spec_undo` is true (a patch that also removes nodes), only ids introduced
/// by this session's replays are removed. Direct variable edits (`!is_spec_seed`) write by id
/// (last-write-wins) so variable edits made on the scoped canvas merge back without duplicates.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_variables(
    doc: &Document,
    collections: &[VariableCollection],
    variables: &[Variable],
    removed_collections: &[String],
    removed_variables: &[String],
    is_spec_seed: bool,
    is_spec_undo: bool,
    state: &mut LivePatchState,
    report: &mut LivePatchReport,
) -> Option<Command> {
    let skipped_before = report.skipped.len();
    let mut merged_vars = doc.variables.clone();
    let mut merged_colls = doc.variable_collections.clone();
    for id in removed_variables {
        if state.introduced_variables.remove(id) || !is_spec_undo {
            merged_vars.remove(id);
        }
    }
    for id in removed_collections {
        if state.introduced_collections.remove(id) || !is_spec_undo {
            merged_colls.remove(id);
        }
    }
    for v in variables {
        if v.id.is_empty() {
            report.skipped.push("a variable without an id was not replayed".to_string());
        } else if !merged_vars.contains_key(&v.id) {
            merged_vars.insert(v.id.clone(), v.clone());
            state.introduced_variables.insert(v.id.clone());
        } else if !is_spec_seed {
            merged_vars.insert(v.id.clone(), v.clone());
        }
    }
    for c in collections {
        if c.id.is_empty() {
            report.skipped.push("a variable collection without an id was not replayed".to_string());
        } else if !merged_colls.contains_key(&c.id) {
            merged_colls.insert(c.id.clone(), c.clone());
            state.introduced_collections.insert(c.id.clone());
        } else if !is_spec_seed {
            merged_colls.insert(c.id.clone(), c.clone());
        }
    }
    if merged_vars == doc.variables && merged_colls == doc.variable_collections {
        if report.skipped.len() == skipped_before {
            report.duplicates += 1;
            report.tokens_duplicates += 1;
        }
        return None;
    }
    report.tokens_applied += 1;
    Some(Command::SetVariables { variables: merged_vars, collections: merged_colls })
}

/// Merges the agent's shared-style and metadata changes into the editor's own maps by style key.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_styles(
    doc: &Document,
    styles_set: &BTreeMap<String, Style>,
    styles_removed: &[String],
    meta_set: &BTreeMap<String, StyleMeta>,
    meta_removed: &[String],
    is_spec_seed: bool,
    is_spec_undo: bool,
    state: &mut LivePatchState,
    report: &mut LivePatchReport,
) -> Option<Command> {
    let skipped_before = report.skipped.len();
    let mut merged_styles = doc.styles.clone();
    let mut merged_meta = doc.style_meta.clone();
    for id in styles_removed {
        if state.introduced_styles.remove(id) || !is_spec_undo {
            merged_styles.remove(id);
        }
    }
    for id in meta_removed {
        if !is_spec_undo || !doc.styles.contains_key(id) || !merged_styles.contains_key(id) {
            merged_meta.remove(id);
        }
    }
    for (id, style) in styles_set {
        if id.is_empty() {
            report.skipped.push("a style without an id was not replayed".to_string());
        } else if !merged_styles.contains_key(id) {
            merged_styles.insert(id.clone(), style.clone());
            state.introduced_styles.insert(id.clone());
        } else if !is_spec_seed {
            merged_styles.insert(id.clone(), style.clone());
        }
    }
    for (id, meta) in meta_set {
        if id.is_empty() {
            report.skipped.push("style metadata without an id was not replayed".to_string());
        } else if !merged_meta.contains_key(id) || !is_spec_seed {
            merged_meta.insert(id.clone(), meta.clone());
        }
    }
    if merged_styles == doc.styles && merged_meta == doc.style_meta {
        if report.skipped.len() == skipped_before {
            report.duplicates += 1;
            report.tokens_duplicates += 1;
        }
        return None;
    }
    report.tokens_applied += 1;
    Some(Command::SetStyles { styles: merged_styles, style_meta: merged_meta })
}

/// Drops any newly added/changed variable binding or shared-style reference on `merged` whose
/// target variable or style does not exist in `doc`, restoring `cur`'s entry and recording the
/// reason in `report.skipped`. Returns the number of skipped links.
pub(super) fn filter_missing_token_links_on_update(doc: &Document, cur: &Node, merged: &mut Node, report: &mut LivePatchReport) -> usize {
    let mut skipped = 0;
    let new_bindings: Vec<(String, String)> =
        merged.variables.bindings.iter().filter(|(k, v)| cur.variables.bindings.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), v.clone())).collect();
    for (prop, var_id) in new_bindings {
        if !doc.variables.contains_key(&var_id) {
            report.skipped.push(format!("update of node {}: variable '{var_id}' not found", cur.id.0));
            match cur.variables.bindings.get(&prop) {
                Some(prev) => {
                    merged.variables.bindings.insert(prop, prev.clone());
                }
                None => {
                    merged.variables.bindings.remove(&prop);
                }
            }
            skipped += 1;
        }
    }
    let new_modes: Vec<(String, String)> =
        merged.variables.modes.iter().filter(|(k, v)| cur.variables.modes.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), v.clone())).collect();
    for (coll_id, _) in new_modes {
        if !doc.variable_collections.contains_key(&coll_id) {
            report.skipped.push(format!("update of node {}: variable collection '{coll_id}' not found", cur.id.0));
            match cur.variables.modes.get(&coll_id) {
                Some(prev) => {
                    merged.variables.modes.insert(coll_id, prev.clone());
                }
                None => {
                    merged.variables.modes.remove(&coll_id);
                }
            }
            skipped += 1;
        }
    }
    let new_styles: Vec<(String, String)> =
        merged.style_refs.iter().filter(|(k, v)| cur.style_refs.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), v.clone())).collect();
    for (prop, style_id) in new_styles {
        if !doc.styles.contains_key(&style_id) {
            report.skipped.push(format!("update of node {}: style '{style_id}' not found", cur.id.0));
            match cur.style_refs.get(&prop) {
                Some(prev) => {
                    merged.style_refs.insert(prop, prev.clone());
                }
                None => {
                    merged.style_refs.remove(&prop);
                }
            }
            skipped += 1;
        }
    }
    skipped
}

/// Strips any variable binding or shared-style reference on inserted `nodes` whose target does
/// not exist in `doc`, recording the reason in `report.skipped`.
pub(super) fn filter_missing_token_links_on_insert(doc: &Document, nodes: &mut [Node], report: &mut LivePatchReport) {
    for n in nodes {
        let missing_vars: Vec<(String, String)> =
            n.variables.bindings.iter().filter(|(_, v)| !doc.variables.contains_key(*v)).map(|(k, v)| (k.clone(), v.clone())).collect();
        for (prop, var_id) in missing_vars {
            report.skipped.push(format!("insert of node {}: variable '{var_id}' not found", n.id.0));
            n.variables.bindings.remove(&prop);
        }
        let missing_colls: Vec<String> = n.variables.modes.keys().filter(|c| !doc.variable_collections.contains_key(*c)).cloned().collect();
        for coll_id in missing_colls {
            report.skipped.push(format!("insert of node {}: variable collection '{coll_id}' not found", n.id.0));
            n.variables.modes.remove(&coll_id);
        }
        let missing_styles: Vec<(String, String)> =
            n.style_refs.iter().filter(|(_, s)| !doc.styles.contains_key(*s)).map(|(k, s)| (k.clone(), s.clone())).collect();
        for (prop, style_id) in missing_styles {
            report.skipped.push(format!("insert of node {}: style '{style_id}' not found", n.id.0));
            n.style_refs.remove(&prop);
        }
    }
}
