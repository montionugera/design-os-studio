//! Single-pass O(1)-bounds tree traversal and budget/cursor continuation for [`super::lint_subtree`].

use super::config::LintConfig;
use super::finding::{Finding, LintReport, LintSummary, SkippedRule};
use super::registry::{Category, RuleNeeds, RULES};
use super::rules::{aabb_of_transformed_rect, contrast, detached, evaluate_node, severity_rank, NodeEvalCtx};
use super::LintEnv;
use crate::{Affine, Document, Node, NodeId, NodeKind};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Maximum number of pixel-path contrast renders per `lint_subtree` call before yielding a continuation cursor.
const MAX_PIXEL_EVALS: usize = 2_000;

/// Execution budget for [`lint_subtree`], capping visited nodes and/or wall-clock duration.
#[derive(Clone, Debug)]
pub struct Budget {
    pub max_nodes: usize,
    pub max_duration: Option<Duration>,
    pub cursor: Option<String>,
}

impl Default for Budget {
    fn default() -> Self {
        Self::unlimited()
    }
}

impl Budget {
    pub fn unlimited() -> Self {
        Self { max_nodes: usize::MAX, max_duration: None, cursor: None }
    }

    pub fn with_limits(max_nodes: usize, max_duration: Option<Duration>) -> Self {
        Self { max_nodes, max_duration, cursor: None }
    }
}

#[derive(Clone, Copy, Debug)]
struct StackItem {
    id: NodeId,
    parent_world: Affine,
    depth: usize,
    in_component: bool,
}

/// Computes `(parent_world, depth, in_component)` for an arbitrary starting `node_id` by walking its ancestor chain.
fn ancestor_state(doc: &Document, node_id: NodeId) -> (Affine, usize, bool) {
    let mut chain = Vec::new();
    let mut cur = doc.get(node_id).and_then(|n| n.parent);
    while let Some(pid) = cur {
        let Some(pn) = doc.get(pid) else { break };
        if matches!(pn.kind, NodeKind::Page { .. }) {
            break;
        }
        chain.push(pn);
        cur = pn.parent;
    }
    let depth = chain.len() + 1;
    let mut world = Affine::IDENTITY;
    let mut in_component = false;
    for anc in chain.into_iter().rev() {
        world = world * anc.transform;
        if matches!(anc.kind, NodeKind::Component(_) | NodeKind::ComponentSet(_)) {
            in_component = true;
        }
    }
    (world, depth, in_component)
}

fn push_children(n: &Node, node_world: Affine, depth: usize, in_component: bool, stack: &mut Vec<StackItem>) {
    let child_in_comp = in_component || matches!(n.kind, NodeKind::Component(_) | NodeKind::ComponentSet(_));
    let paint_children = n.paint_children();
    for &cid in paint_children.iter().rev() {
        stack.push(StackItem { id: cid, parent_world: node_world, depth: depth + 1, in_component: child_in_comp });
    }
}

/// Evaluates enabled lint rules over `roots` within `budget`, returning a [`LintReport`].
pub fn lint_subtree(doc: &Document, roots: &[NodeId], config: &LintConfig, env: &dyn LintEnv, budget: &mut Budget) -> LintReport {
    let start = Instant::now();

    let mut skipped_rules = Vec::new();
    if !env.has_text_engine() {
        for r in RULES {
            if r.needs == RuleNeeds::Text && config.is_enabled(r.id) {
                skipped_rules.push(SkippedRule { rule: r.id.to_string(), reason: "no_text_engine".to_string() });
            }
        }
    }

    let components = if config.is_enabled("detached-instance") { detached::Components::new(doc) } else { detached::Components::default() };
    let contrast_bounds = contrast::SubtreeBounds::default();

    let active_cursor = budget.cursor.as_deref().or(config.cursor.as_deref());
    let mut page_offset = 0usize;
    let mut stack: Vec<StackItem> = Vec::new();

    if let Some(cur_str) = active_cursor {
        if let Some(rest) = cur_str.strip_prefix("stack:") {
            for part in rest.split(',').rev() {
                if let Ok(raw_id) = part.trim().parse::<u64>() {
                    let nid = NodeId(raw_id);
                    if doc.get(nid).is_some() {
                        let (parent_world, depth, in_component) = ancestor_state(doc, nid);
                        stack.push(StackItem { id: nid, parent_world, depth, in_component });
                    }
                }
            }
        } else if let Some(rest) = cur_str.strip_prefix("offset:") {
            page_offset = rest.trim().parse::<usize>().unwrap_or(0);
        } else if let Ok(off) = cur_str.trim().parse::<usize>() {
            page_offset = off;
        }
    }

    if stack.is_empty() {
        for &root_id in roots.iter().rev() {
            let Some(root_node) = doc.get(root_id) else {
                continue;
            };
            if matches!(root_node.kind, NodeKind::Page { .. }) {
                for &cid in root_node.children.iter().rev() {
                    stack.push(StackItem { id: cid, parent_world: Affine::IDENTITY, depth: 1, in_component: false });
                }
            } else {
                let (parent_world, depth, in_component) = ancestor_state(doc, root_id);
                stack.push(StackItem { id: root_id, parent_world, depth, in_component });
            }
        }
    }

    let mut raw_findings: Vec<Finding> = Vec::new();
    let mut scanned = 0usize;
    let mut pixel_evals = 0usize;
    let mut complete = true;
    let mut next_cursor: Option<String> = None;
    let loop_start = Instant::now();

    while let Some(item) = stack.pop() {
        let Some(n) = doc.get(item.id) else {
            continue;
        };
        if !n.visible || matches!(n.kind, NodeKind::Page { .. }) {
            continue;
        }

        if scanned >= budget.max_nodes
            || pixel_evals >= MAX_PIXEL_EVALS
            || budget.max_duration.is_some_and(|max_d| scanned >= 32 && loop_start.elapsed() >= max_d)
        {
            stack.push(item);
            complete = false;
            let remaining: Vec<String> = stack.iter().rev().map(|s| s.id.0.to_string()).collect();
            next_cursor = Some(format!("stack:{}", remaining.join(",")));
            break;
        }

        let node_world = item.parent_world * n.transform;
        let page_bounds = aabb_of_transformed_rect(node_world, n.size);
        let is_instance_mirror = n.component.source.is_some() && n.component.overrides.is_empty();

        if !is_instance_mirror || config.include_instances {
            scanned += 1;
            let node_in_comp = item.in_component || matches!(n.kind, NodeKind::Component(_) | NodeKind::ComponentSet(_));
            let ctx = NodeEvalCtx {
                doc,
                node: n,
                depth: item.depth,
                in_component: node_in_comp,
                is_instance_mirror,
                page_bounds,
                config,
                env,
                components: &components,
                contrast_bounds: &contrast_bounds,
            };
            if evaluate_node(&ctx, &mut raw_findings) {
                pixel_evals += 1;
            }
        }

        push_children(n, node_world, item.depth, item.in_component, &mut stack);
    }

    // Filter or mark ignored findings.
    let mut filtered: Vec<Finding> = Vec::with_capacity(raw_findings.len());
    for mut f in raw_findings {
        let is_ignored = doc.is_lint_ignored(f.rule, f.node);
        if is_ignored {
            if config.include_ignored {
                f.ignored = true;
                filtered.push(f);
            }
        } else {
            filtered.push(f);
        }
    }

    // Stable sort by severity (error, warning, info), preserving document paint order within each.
    filtered.sort_by_key(|f| severity_rank(f.severity));

    let total = filtered.len();
    let mut by_severity: BTreeMap<String, usize> = BTreeMap::from([("error".to_string(), 0), ("warning".to_string(), 0), ("info".to_string(), 0)]);
    let mut by_rule: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_category: BTreeMap<String, usize> = Category::ALL.iter().map(|c| (c.as_str().to_string(), 0)).collect();

    for f in &filtered {
        *by_severity.entry(f.severity.as_str().to_string()).or_insert(0) += 1;
        *by_rule.entry(f.rule.to_string()).or_insert(0) += 1;
        *by_category.entry(f.category.as_str().to_string()).or_insert(0) += 1;
    }

    let errors = by_severity.get("error").copied().unwrap_or(0);
    let warnings = by_severity.get("warning").copied().unwrap_or(0);
    let infos = by_severity.get("info").copied().unwrap_or(0);

    let findings = if let Some(limit) = config.limit {
        let start_idx = page_offset.min(total);
        let end_idx = (start_idx + limit).min(total);
        if end_idx < total && next_cursor.is_none() {
            next_cursor = Some(format!("offset:{end_idx}"));
        }
        filtered.into_iter().skip(start_idx).take(end_idx - start_idx).collect()
    } else if page_offset > 0 {
        filtered.into_iter().skip(page_offset.min(total)).collect()
    } else {
        filtered
    };

    let truncated = !complete || findings.len() < total;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    budget.cursor = next_cursor.clone();

    LintReport {
        findings,
        total,
        by_severity: by_severity.clone(),
        by_rule: by_rule.clone(),
        skipped_rules,
        scanned,
        scanned_nodes: scanned,
        complete,
        truncated,
        elapsed_ms,
        cursor: next_cursor,
        summary: LintSummary { total, errors, warnings, infos, by_severity, by_rule, by_category },
    }
}
