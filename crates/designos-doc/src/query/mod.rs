//! Streaming document query visitor (`designos_query_xpath`) with paging,
//! node budget, compact rows/objects, and solver-backed linter predicates (E15-17 / #602).

use crate::lint::{canonical_rule_id, lint_subtree, resolve_rule, Budget as LintBudget, LintConfig, LintEnv, LintPreset, NoLintEnv, RuleNeeds};
use crate::openpencil::{eval_predicate_expr, layout_mode_str, matches_node_test, parse_xpath, xpath_type_name, XPathAxis, XPathStep};
use crate::{Document, Node, NodeId, NodeKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

pub const DEFAULT_LIMIT: usize = 100;
pub const MAX_LIMIT: usize = 500;
pub const DEFAULT_MAX_SCANNED: usize = 50_000;
pub const MIN_MAX_SCANNED: usize = 1_000;
pub const MAX_MAX_SCANNED: usize = 200_000;

pub const ROW_COLUMNS: [&str; 9] = ["id", "name", "type", "x", "y", "w", "h", "problems", "in_progress"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryFormat {
    #[default]
    Rows,
    Objects,
}

impl QueryFormat {
    pub fn parse(raw: Option<&str>) -> Result<Self, QueryError> {
        match raw.unwrap_or("rows").trim().to_ascii_lowercase().as_str() {
            "" | "rows" | "compact" => Ok(Self::Rows),
            "objects" | "full" | "summary" => Ok(Self::Objects),
            other => Err(QueryError::InvalidSelector {
                message: format!("unknown format '{other}'; expected 'rows' or 'objects'"),
                corrected_example: "//FRAME[@name='Card']".to_string(),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    #[default]
    Compact,
    Summary,
    Full,
}

#[derive(Clone, Debug, Default)]
pub struct QueryOptions {
    pub selector: String,
    pub page: Option<NodeId>,
    pub within: Option<NodeId>,
    pub limit: Option<usize>,
    pub max_scanned: Option<usize>,
    pub cursor: Option<String>,
    pub format: QueryFormat,
    pub with_problems: bool,
    pub projection: Projection,
    pub fields: Option<Vec<String>>,
    pub max_depth: Option<u32>,
    pub node_budget: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryPage {
    pub columns: Vec<String>,
    pub rows: Vec<Value>,
    pub matches: Vec<Value>,
    pub count: usize,
    pub returned: usize,
    pub scanned: usize,
    pub scanned_total: usize,
    pub scope_size: usize,
    pub remaining: usize,
    pub done: bool,
    pub complete: bool,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub document_epoch: u64,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryError {
    InvalidSelector { message: String, corrected_example: String },
    InvalidCursor { message: String },
    CursorStale { cursor_epoch: u64, document_epoch: u64 },
    UnknownRule { rule: String },
    RuleUnavailable { rule: String, reason: String },
    NodeNotFound { id: NodeId },
    ExactlyOneScope,
}

impl QueryError {
    pub fn code(&self) -> &'static str {
        match self {
            QueryError::InvalidSelector { .. } => "INVALID_SELECTOR",
            QueryError::InvalidCursor { .. } => "INVALID_CURSOR",
            QueryError::CursorStale { .. } => "CURSOR_STALE",
            QueryError::UnknownRule { .. } => "UNKNOWN_RULE",
            QueryError::RuleUnavailable { .. } => "RULE_UNAVAILABLE",
            QueryError::NodeNotFound { .. } => "NODE_NOT_FOUND",
            QueryError::ExactlyOneScope => "EXACTLY_ONE_SCOPE",
        }
    }
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::InvalidSelector {
                message,
                corrected_example,
            } => write!(
                f,
                "INVALID_SELECTOR: {message} (path: /selector, corrected_example: {corrected_example}, safe_to_retry: true)"
            ),
            QueryError::InvalidCursor { message } => {
                write!(f, "INVALID_CURSOR: {message} (safe_to_retry: true)")
            }
            QueryError::CursorStale {
                cursor_epoch,
                document_epoch,
            } => write!(
                f,
                "CURSOR_STALE: cursor revision {cursor_epoch} does not match document revision {document_epoch}; restart query without cursor (safe_to_retry: true)"
            ),
            QueryError::UnknownRule { rule } => write!(
                f,
                "UNKNOWN_RULE: unknown lint rule '{rule}' in has-problem() predicate (safe_to_retry: true)"
            ),
            QueryError::RuleUnavailable { rule, reason } => write!(
                f,
                "RULE_UNAVAILABLE: lint rule '{rule}' cannot be evaluated ({reason}) (safe_to_retry: false)"
            ),
            QueryError::NodeNotFound { id } => write!(
                f,
                "NODE_NOT_FOUND: scope node {} does not exist (safe_to_retry: true)",
                id.0
            ),
            QueryError::ExactlyOneScope => write!(
                f,
                "EXACTLY_ONE_SCOPE: provide at most one of page_id or node_id (safe_to_retry: true)"
            ),
        }
    }
}

impl std::error::Error for QueryError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct CursorToken {
    v: u8,
    doc_revision: u64,
    scope_root: Option<u64>,
    expr_hash: u64,
    scope_size: usize,
    scanned_total: usize,
    stack: Vec<(u64, usize, u64)>,
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

const B64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64url_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() * 4).div_ceil(3));
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(B64_URL[((n >> 18) & 0x3f) as usize] as char);
        out.push(B64_URL[((n >> 12) & 0x3f) as usize] as char);
        out.push(B64_URL[((n >> 6) & 0x3f) as usize] as char);
        out.push(B64_URL[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(B64_URL[((n >> 18) & 0x3f) as usize] as char);
        out.push(B64_URL[((n >> 12) & 0x3f) as usize] as char);
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(B64_URL[((n >> 18) & 0x3f) as usize] as char);
        out.push(B64_URL[((n >> 12) & 0x3f) as usize] as char);
        out.push(B64_URL[((n >> 6) & 0x3f) as usize] as char);
    }
    out
}

fn b64url_decode(raw: &str) -> Result<Vec<u8>, String> {
    let s = raw.trim().trim_end_matches('=');
    let mut vals = Vec::with_capacity(s.len());
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return Err(format!("invalid base64 byte {b:#x}")),
        };
        vals.push(v);
    }
    if vals.len() % 4 == 1 {
        return Err("invalid base64 length".to_string());
    }
    let mut out = Vec::with_capacity(vals.len() * 3 / 4);
    let mut i = 0;
    while i + 4 <= vals.len() {
        let n = ((vals[i] as u32) << 18) | ((vals[i + 1] as u32) << 12) | ((vals[i + 2] as u32) << 6) | (vals[i + 3] as u32);
        out.push(((n >> 16) & 0xff) as u8);
        out.push(((n >> 8) & 0xff) as u8);
        out.push((n & 0xff) as u8);
        i += 4;
    }
    let rem = vals.len() - i;
    if rem == 2 {
        let n = ((vals[i] as u32) << 18) | ((vals[i + 1] as u32) << 12);
        out.push(((n >> 16) & 0xff) as u8);
    } else if rem == 3 {
        let n = ((vals[i] as u32) << 18) | ((vals[i + 1] as u32) << 12) | ((vals[i + 2] as u32) << 6);
        out.push(((n >> 16) & 0xff) as u8);
        out.push(((n >> 8) & 0xff) as u8);
    }
    Ok(out)
}

fn encode_cursor(token: &CursorToken) -> String {
    let json_bytes = serde_json::to_vec(token).unwrap_or_default();
    b64url_encode(&json_bytes)
}

fn decode_cursor(raw: &str) -> Result<CursorToken, QueryError> {
    let bytes = b64url_decode(raw).map_err(|e| QueryError::InvalidCursor { message: format!("base64 decode failed: {e}") })?;
    let tok: CursorToken = serde_json::from_slice(&bytes).map_err(|e| QueryError::InvalidCursor { message: format!("cursor JSON invalid: {e}") })?;
    if tok.v != 1 {
        return Err(QueryError::InvalidCursor { message: format!("unsupported cursor version {}", tok.v) });
    }
    Ok(tok)
}

fn unquote_arg(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('\'') && t.ends_with('\'')) || (t.starts_with('"') && t.ends_with('"')) {
        if t.len() >= 2 {
            return t[1..t.len() - 1].to_string();
        }
    }
    t.to_string()
}

/// Validates steps and checks `has-problem` rules and environment availability upfront.
fn validate_steps(steps: &[XPathStep], env: Option<&dyn LintEnv>) -> Result<(), QueryError> {
    if steps.is_empty() || steps.len() > 63 {
        return Err(QueryError::InvalidSelector {
            message: "selector must have between 1 and 63 steps".to_string(),
            corrected_example: "//TEXT[has-problem('text-truncated')]".to_string(),
        });
    }
    let dummy_doc = Document::new("validate");
    let dummy_node = Node::frame(NodeId(1), crate::Rect::new(0.0, 0.0, 10.0, 10.0));
    for step in steps {
        for ch in step.node_test.chars() {
            if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '*') {
                return Err(QueryError::InvalidSelector {
                    message: format!("invalid character '{ch}' in node test '{}'", step.node_test),
                    corrected_example: "//FRAME[@name='Card']".to_string(),
                });
            }
        }
        for pred in &step.predicates {
            validate_predicate(pred, &dummy_doc, &dummy_node, env)?;
        }
    }
    Ok(())
}

fn validate_predicate(pred: &str, dummy_doc: &Document, dummy_node: &Node, env: Option<&dyn LintEnv>) -> Result<(), QueryError> {
    let s = pred.trim();
    if s.is_empty() {
        return Err(QueryError::InvalidSelector {
            message: "empty predicate '[]' is not valid".to_string(),
            corrected_example: "//FRAME[@name='Card']".to_string(),
        });
    }
    let mut rest = s;
    while let Some(pos) = rest.find("has-problem(") {
        let after = &rest[pos + "has-problem(".len()..];
        let Some(end) = after.find(')') else {
            return Err(QueryError::InvalidSelector {
                message: "unclosed has-problem(...) call".to_string(),
                corrected_example: "//TEXT[has-problem('text-truncated')]".to_string(),
            });
        };
        let rule_raw = unquote_arg(&after[..end]);
        let Some(canonical) = canonical_rule_id(&rule_raw) else {
            return Err(QueryError::UnknownRule { rule: rule_raw });
        };
        if let Some(spec) = resolve_rule(canonical) {
            if spec.needs == RuleNeeds::Text {
                let has_text = env.is_some_and(|e| e.has_text_engine());
                if !has_text {
                    return Err(QueryError::RuleUnavailable { rule: canonical.to_string(), reason: "no_text_engine".to_string() });
                }
            }
        }
        rest = &after[end + 1..];
    }

    if s.contains("has-problem(") || s.contains("is-bound(") || s.contains("has-hardcoded(") || s.contains("@problems") || s.contains("@inProgress") {
        return Ok(());
    }

    eval_predicate_expr(dummy_doc, dummy_node, s)
        .map_err(|message| QueryError::InvalidSelector { message, corrected_example: "//FRAME[@name='Card']".to_string() })?;
    Ok(())
}

fn is_property_bound(node: &Node, prop_key: &str) -> bool {
    let key = prop_key.trim();
    if key == "*" {
        return !node.variables.bindings.is_empty();
    }
    if node.variables.bindings.contains_key(key) {
        return true;
    }
    match key {
        "fills/0" | "fill" | "fills" => {
            node.variables.bindings.contains_key("fills/0") || node.variables.bindings.contains_key("fill") || node.variables.bindings.contains_key("fills")
        }
        "strokes/0" | "stroke" | "strokes" => {
            node.variables.bindings.contains_key("strokes/0")
                || node.variables.bindings.contains_key("stroke")
                || node.variables.bindings.contains_key("strokes")
        }
        _ => false,
    }
}

fn has_hardcoded_property(doc: &Document, node: &Node, prop_key: &str) -> bool {
    let key = prop_key.trim();
    let check_fill = key == "*" || key == "fills/0" || key == "fill" || key == "fills";
    let check_stroke = key == "*" || key == "strokes/0" || key == "stroke" || key == "strokes";
    let check_spacing = key == "*" || key == "itemSpacing" || key == "gap";
    let check_radius = key == "*" || key == "cornerRadius";

    if check_fill && node.fills.iter().any(|f| f.visible && matches!(f.kind, crate::PaintKind::Solid(_))) && !is_property_bound(node, "fills/0") {
        return true;
    }
    if check_stroke && node.stroke.paints.iter().any(|s| s.visible && matches!(s.kind, crate::PaintKind::Solid(_))) && !is_property_bound(node, "strokes/0") {
        return true;
    }
    if check_spacing {
        if let Some(al) = node.kind.frame_props().and_then(|fp| fp.auto_layout.as_ref()) {
            if al.item_spacing > 0.0 && !is_property_bound(node, "itemSpacing") {
                return true;
            }
        }
    }
    if check_radius && node.corner_radii[0] > 0.0 && !is_property_bound(node, "cornerRadius") {
        return true;
    }
    let _ = doc;
    false
}

fn compute_node_problems(
    doc: &Document,
    node: &Node,
    rule_filter: Option<&str>,
    env: Option<&dyn LintEnv>,
    extra_units: &mut usize,
) -> Result<Vec<String>, QueryError> {
    let no_env = NoLintEnv;
    let env_ref: &dyn LintEnv = env.unwrap_or(&no_env);

    let cfg = if let Some(rule_raw) = rule_filter {
        let Some(canonical) = canonical_rule_id(rule_raw) else {
            return Err(QueryError::UnknownRule { rule: rule_raw.to_string() });
        };
        if let Some(spec) = resolve_rule(canonical) {
            if spec.needs == RuleNeeds::Text {
                if !env_ref.has_text_engine() {
                    return Err(QueryError::RuleUnavailable { rule: canonical.to_string(), reason: "no_text_engine".to_string() });
                }
                *extra_units += 20;
            }
            if spec.needs == RuleNeeds::Pixels {
                *extra_units += 200;
            }
        }
        LintConfig::from_rule_list(&[canonical.to_string()]).unwrap_or_else(|_| LintConfig::from_preset(LintPreset::Agent))
    } else {
        if node.kind.any_text().is_some() && env_ref.has_text_engine() {
            *extra_units += 20;
        }
        LintConfig::from_preset(LintPreset::Agent)
    };

    let root = doc.page_of(node.id).or(node.parent).unwrap_or(node.id);
    let mut budget = LintBudget::unlimited();
    let report = lint_subtree(doc, &[root], &cfg, env_ref, &mut budget);
    let mut rules: Vec<String> = report.findings.into_iter().filter(|f| f.node == node.id).map(|f| f.rule.to_string()).collect();
    rules.sort();
    rules.dedup();
    Ok(rules)
}

fn eval_single_predicate(
    doc: &Document,
    node: &Node,
    expr: &str,
    env: Option<&dyn LintEnv>,
    extra_units: &mut usize,
    cached_problems: &mut Option<Vec<String>>,
) -> Result<bool, QueryError> {
    let s = expr.trim();
    if s.is_empty() {
        return Ok(true);
    }

    if let Ok(pos) = s.parse::<usize>() {
        if pos == 0 {
            return Ok(false);
        }
        let ty = xpath_type_name(&node.kind);
        if let Some(parent) = node.parent.and_then(|p| doc.get(p)) {
            let mut count = 0usize;
            for &sib_id in &parent.children {
                *extra_units += 1;
                if let Some(sib) = doc.get(sib_id) {
                    if xpath_type_name(&sib.kind) == ty {
                        count += 1;
                        if sib_id == node.id {
                            return Ok(count == pos);
                        }
                    }
                }
            }
            return Ok(false);
        }
        return Ok(pos == 1);
    }

    if let Some(rest) = s.strip_prefix("not(").and_then(|r| r.strip_suffix(')')) {
        return Ok(!eval_single_predicate(doc, node, rest, env, extra_units, cached_problems)?);
    }

    if let Some(inner) = s.strip_prefix("has-problem(").and_then(|r| r.strip_suffix(')')) {
        let rule_name = unquote_arg(inner);
        let Some(canonical) = canonical_rule_id(&rule_name) else {
            return Err(QueryError::UnknownRule { rule: rule_name });
        };
        let problems = compute_node_problems(doc, node, Some(canonical), env, extra_units)?;
        return Ok(problems.iter().any(|r| r == canonical));
    }

    if let Some(inner) = s.strip_prefix("is-bound(").and_then(|r| r.strip_suffix(')')) {
        let prop = unquote_arg(inner);
        return Ok(is_property_bound(node, &prop));
    }

    if let Some(inner) = s.strip_prefix("has-hardcoded(").and_then(|r| r.strip_suffix(')')) {
        let prop = unquote_arg(inner);
        return Ok(has_hardcoded_property(doc, node, &prop));
    }

    if s == "@inProgress" || s == "@in_progress" {
        return Ok(node_in_progress(node));
    }

    if s.contains("@problems") {
        let probs = if let Some(ref p) = cached_problems {
            p.clone()
        } else {
            let p = compute_node_problems(doc, node, None, env, extra_units)?;
            *cached_problems = Some(p.clone());
            p
        };
        let count = probs.len() as f32;
        if s == "@problems" {
            return Ok(count > 0.0);
        }
        for op in ["<=", ">=", "!=", "=", "<", ">"] {
            if let Some((lhs, rhs)) = s.split_once(op) {
                if lhs.trim() == "@problems" {
                    if let Ok(rhs_n) = rhs.trim().parse::<f32>() {
                        return Ok(match op {
                            "=" => (count - rhs_n).abs() < 1e-4,
                            "!=" => (count - rhs_n).abs() >= 1e-4,
                            "<" => count < rhs_n,
                            "<=" => count <= rhs_n,
                            ">" => count > rhs_n,
                            ">=" => count >= rhs_n,
                            _ => false,
                        });
                    }
                }
            }
        }
    }

    eval_predicate_expr(doc, node, s).map_err(|message| QueryError::InvalidSelector { message, corrected_example: "//FRAME[@name='Card']".to_string() })
}

fn node_in_progress(_node: &Node) -> bool {
    false
}

fn step_matches_node(
    doc: &Document,
    node: &Node,
    step: &XPathStep,
    env: Option<&dyn LintEnv>,
    extra_units: &mut usize,
    cached_problems: &mut Option<Vec<String>>,
) -> Result<bool, QueryError> {
    if !matches_node_test(&node.kind, &step.node_test) {
        return Ok(false);
    }
    for pred in &step.predicates {
        if !eval_single_predicate(doc, node, pred, env, extra_units, cached_problems)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn round2(v: f32) -> f64 {
    ((v as f64) * 100.0).round() / 100.0
}

fn build_row(
    doc: &Document,
    node: &Node,
    format: QueryFormat,
    with_problems: bool,
    env: Option<&dyn LintEnv>,
    extra_units: &mut usize,
    cached_problems: Option<Vec<String>>,
) -> Result<Value, QueryError> {
    let problems_val: Value = if let Some(probs) = cached_problems {
        json!(probs)
    } else if with_problems {
        let probs = compute_node_problems(doc, node, None, env, extra_units)?;
        json!(probs)
    } else {
        Value::Null
    };

    let x = round2(node.transform.tx);
    let y = round2(node.transform.ty);
    let w = round2(node.size.x);
    let h = round2(node.size.y);
    let ty = xpath_type_name(&node.kind);
    let in_prog = node_in_progress(node);

    Ok(match format {
        QueryFormat::Rows => json!([node.id.0, node.name, ty, x, y, w, h, problems_val, in_prog,]),
        QueryFormat::Objects => json!({
            "id": node.id.0,
            "name": node.name,
            "type": ty,
            "x": x,
            "y": y,
            "w": w,
            "h": h,
            "bounds": [x, y, w, h],
            "visible": node.visible,
            "child_count": node.children.len(),
            "layout_mode": layout_mode_str(node),
            "problems": problems_val,
            "in_progress": in_prog,
        }),
    })
}

fn count_scope_nodes(doc: &Document, roots: &[NodeId], max_depth: Option<u32>) -> usize {
    let mut count = 0usize;
    let mut stack: Vec<(NodeId, u32)> = roots.iter().rev().map(|&r| (r, 0)).collect();
    let mut seen = HashSet::new();
    while let Some((nid, depth)) = stack.pop() {
        if !seen.insert(nid) {
            continue;
        }
        let Some(node) = doc.nodes.get(&nid) else {
            continue;
        };
        if depth >= 1 {
            count += 1;
        }
        if max_depth.is_some_and(|max_d| depth >= max_d) {
            continue;
        }
        for &cid in node.children.iter().rev() {
            stack.push((cid, depth + 1));
        }
    }
    count
}

#[derive(Clone, Debug)]
struct StackFrame {
    node_id: NodeId,
    next_child_idx: usize,
    matched_mask: u64,
    ancestor_mask: u64,
    depth: u32,
}

fn default_columns() -> Vec<String> {
    ROW_COLUMNS.iter().map(|s| (*s).to_string()).collect()
}

/// Runs a streaming pre-order XPath query over `doc` with an NFA over steps,
/// enforcing `limit`, `max_scanned`, and cursor pagination (`E15-17`).
pub fn run(doc: &Document, opts: &QueryOptions) -> Result<QueryPage, QueryError> {
    let approx = crate::lint::ApproxLintEnv;
    run_with_env(doc, opts, Some(&approx))
}

/// Runs a streaming pre-order XPath query over `doc` with an explicit [`LintEnv`]
/// (pass `None` when no text-shaping engine is available so `has-problem` on text rules
/// returns `RULE_UNAVAILABLE` per `E15-17` §8.5).
pub fn run_with_env(doc: &Document, opts: &QueryOptions, env: Option<&dyn LintEnv>) -> Result<QueryPage, QueryError> {
    let start_instant = std::time::Instant::now();
    if opts.page.is_some() && opts.within.is_some() {
        return Err(QueryError::ExactlyOneScope);
    }

    let steps =
        parse_xpath(&opts.selector).map_err(|message| QueryError::InvalidSelector { message, corrected_example: "//FRAME[@name='Card']".to_string() })?;
    validate_steps(&steps, env)?;

    let limit = opts.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let max_scanned = opts.max_scanned.or_else(|| opts.node_budget.map(|b| b as usize)).unwrap_or(DEFAULT_MAX_SCANNED).clamp(1, MAX_MAX_SCANNED);

    let doc_revision = doc.mutation_epoch();
    let expr_hash = fnv1a64(opts.selector.trim().as_bytes());

    if let Some(page_id) = opts.page {
        let Some(pnode) = doc.nodes.get(&page_id) else {
            return Err(QueryError::NodeNotFound { id: page_id });
        };
        if !matches!(pnode.kind, NodeKind::Page { .. }) {
            return Err(QueryError::NodeNotFound { id: page_id });
        }
    }

    if let Some(within_id) = opts.within {
        if !doc.nodes.contains_key(&within_id) {
            return Err(QueryError::NodeNotFound { id: within_id });
        }
    }

    let scope_root: Option<NodeId> = opts.within.or(opts.page);
    let roots: Vec<NodeId> = if let Some(sr) = scope_root {
        vec![sr]
    } else if !doc.pages.is_empty() {
        doc.pages.clone()
    } else {
        doc.nodes.values().filter(|n| n.parent.is_none()).map(|n| n.id).collect()
    };

    let mut stack: Vec<StackFrame> = Vec::new();
    let (scope_size, prev_scanned_total) = if let Some(cursor_str) = &opts.cursor {
        let tok = decode_cursor(cursor_str)?;
        if tok.doc_revision != doc_revision {
            return Err(QueryError::CursorStale { cursor_epoch: tok.doc_revision, document_epoch: doc_revision });
        }
        if tok.expr_hash != expr_hash || tok.scope_root != scope_root.map(|id| id.0) || tok.stack.is_empty() {
            return Err(QueryError::InvalidCursor { message: "cursor parameters do not match query selector or scope".to_string() });
        }
        let mut running_anc = 0u64;
        for (idx, &(nid_raw, next_child_idx, matched_mask)) in tok.stack.iter().enumerate() {
            let nid = NodeId(nid_raw);
            if !doc.nodes.contains_key(&nid) {
                return Err(QueryError::InvalidCursor { message: format!("cursor references missing node {}", nid.0) });
            }
            stack.push(StackFrame { node_id: nid, next_child_idx, matched_mask, ancestor_mask: running_anc, depth: idx as u32 });
            running_anc |= matched_mask;
        }
        (tok.scope_size, tok.scanned_total)
    } else {
        let sz = count_scope_nodes(doc, &roots, opts.max_depth);
        if let Some(&first_root) = roots.first() {
            stack.push(StackFrame { node_id: first_root, next_child_idx: 0, matched_mask: 0, ancestor_mask: 0, depth: 0 });
        }
        (sz, 0usize)
    };

    let mut rows = Vec::new();
    let mut matches_obj = Vec::new();
    let mut scanned_units: usize = 0;
    let mut seen_nodes: HashSet<NodeId> = HashSet::new();
    for frame in &stack {
        seen_nodes.insert(frame.node_id);
    }

    let advance_root = |stack: &mut Vec<StackFrame>, finished_root: NodeId| {
        if let Some(pos) = roots.iter().position(|&r| r == finished_root) {
            if let Some(&next_root) = roots.get(pos + 1) {
                stack.push(StackFrame { node_id: next_root, next_child_idx: 0, matched_mask: 0, ancestor_mask: 0, depth: 0 });
            }
        }
    };

    let settle_stack = |stack: &mut Vec<StackFrame>| {
        while let Some(top) = stack.last() {
            let Some(node) = doc.nodes.get(&top.node_id) else {
                let popped = stack.pop().unwrap();
                if stack.is_empty() {
                    advance_root(stack, popped.node_id);
                }
                continue;
            };
            let at_max_depth = opts.max_depth.is_some_and(|max_d| top.depth >= max_d);
            if top.next_child_idx == 0 {
                break;
            }
            if !at_max_depth && top.next_child_idx <= node.children.len() {
                break;
            }
            let popped = stack.pop().unwrap();
            if stack.is_empty() {
                advance_root(stack, popped.node_id);
            }
        }
    };

    let last_step_bit = 1u64 << (steps.len() - 1);

    loop {
        settle_stack(&mut stack);
        let Some(top_idx) = stack.len().checked_sub(1) else {
            let scanned_total = (prev_scanned_total + scanned_units).min(scope_size.max(prev_scanned_total + scanned_units));
            let remaining = scope_size.saturating_sub(scanned_total);
            let count = rows.len();
            return Ok(QueryPage {
                columns: default_columns(),
                rows,
                matches: matches_obj,
                count,
                returned: count,
                scanned: scanned_units,
                scanned_total,
                scope_size,
                remaining,
                done: true,
                complete: true,
                truncated: false,
                cursor: None,
                next_cursor: None,
                document_epoch: doc_revision,
                elapsed_ms: start_instant.elapsed().as_millis() as u64,
            });
        };

        if stack[top_idx].next_child_idx == 0 {
            let nid = stack[top_idx].node_id;
            let depth = stack[top_idx].depth;
            let parent_matched = if top_idx > 0 { stack[top_idx - 1].matched_mask } else { 0 };
            let ancestor_matched = stack[top_idx].ancestor_mask;

            if depth >= 1 && scanned_units >= max_scanned {
                let scanned_total = prev_scanned_total + scanned_units;
                let remaining = scope_size.saturating_sub(scanned_total);
                let tok = CursorToken {
                    v: 1,
                    doc_revision,
                    scope_root: scope_root.map(|id| id.0),
                    expr_hash,
                    scope_size,
                    scanned_total,
                    stack: stack.iter().map(|f| (f.node_id.0, f.next_child_idx, f.matched_mask)).collect(),
                };
                let cur = encode_cursor(&tok);
                let count = rows.len();
                return Ok(QueryPage {
                    columns: default_columns(),
                    rows,
                    matches: matches_obj,
                    count,
                    returned: count,
                    scanned: scanned_units,
                    scanned_total,
                    scope_size,
                    remaining,
                    done: false,
                    complete: false,
                    truncated: true,
                    cursor: Some(cur.clone()),
                    next_cursor: Some(cur),
                    document_epoch: doc_revision,
                    elapsed_ms: start_instant.elapsed().as_millis() as u64,
                });
            }

            stack[top_idx].next_child_idx = 1;

            let Some(node) = doc.nodes.get(&nid) else {
                continue;
            };

            if depth >= 1 {
                scanned_units += 1;
                let mut extra_units = 0usize;
                let mut cached_problems: Option<Vec<String>> = None;
                let mut my_mask = 0u64;

                for (s_idx, step) in steps.iter().enumerate() {
                    let prev_satisfied = if s_idx == 0 {
                        match step.axis {
                            XPathAxis::Child => depth == 1,
                            XPathAxis::Descendant => true,
                        }
                    } else {
                        let prev_bit = 1u64 << (s_idx - 1);
                        match step.axis {
                            XPathAxis::Child => (parent_matched & prev_bit) != 0,
                            XPathAxis::Descendant => (ancestor_matched & prev_bit) != 0,
                        }
                    };
                    if prev_satisfied && step_matches_node(doc, node, step, env, &mut extra_units, &mut cached_problems)? {
                        my_mask |= 1u64 << s_idx;
                    }
                }

                stack[top_idx].matched_mask = my_mask;
                scanned_units = scanned_units.saturating_add(extra_units);

                if (my_mask & last_step_bit) != 0 {
                    let mut row_extra = 0usize;
                    let row = build_row(doc, node, opts.format, opts.with_problems, env, &mut row_extra, cached_problems.clone())?;
                    let obj = build_row(doc, node, QueryFormat::Objects, opts.with_problems, env, &mut 0, cached_problems)?;
                    scanned_units = scanned_units.saturating_add(row_extra);
                    rows.push(row);
                    matches_obj.push(obj);

                    if rows.len() >= limit || scanned_units >= max_scanned {
                        settle_stack(&mut stack);
                        let has_more = !stack.is_empty();
                        let scanned_total = prev_scanned_total + scanned_units;
                        let remaining = if has_more { scope_size.saturating_sub(scanned_total) } else { 0 };
                        let cur = if has_more {
                            let tok = CursorToken {
                                v: 1,
                                doc_revision,
                                scope_root: scope_root.map(|id| id.0),
                                expr_hash,
                                scope_size,
                                scanned_total,
                                stack: stack.iter().map(|f| (f.node_id.0, f.next_child_idx, f.matched_mask)).collect(),
                            };
                            Some(encode_cursor(&tok))
                        } else {
                            None
                        };
                        let count = rows.len();
                        return Ok(QueryPage {
                            columns: default_columns(),
                            rows,
                            matches: matches_obj,
                            count,
                            returned: count,
                            scanned: scanned_units,
                            scanned_total,
                            scope_size,
                            remaining,
                            done: !has_more,
                            complete: !has_more,
                            truncated: has_more,
                            cursor: cur.clone(),
                            next_cursor: cur,
                            document_epoch: doc_revision,
                            elapsed_ms: start_instant.elapsed().as_millis() as u64,
                        });
                    }
                }
            }
        } else {
            let nid = stack[top_idx].node_id;
            let depth = stack[top_idx].depth;
            let child_anc_mask = stack[top_idx].ancestor_mask | stack[top_idx].matched_mask;
            let child_idx = stack[top_idx].next_child_idx - 1;
            stack[top_idx].next_child_idx += 1;

            let Some(node) = doc.nodes.get(&nid) else {
                continue;
            };
            if let Some(&child_id) = node.children.get(child_idx) {
                if seen_nodes.insert(child_id) {
                    stack.push(StackFrame { node_id: child_id, next_child_idx: 0, matched_mask: 0, ancestor_mask: child_anc_mask, depth: depth + 1 });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{AutoLayout, LayoutDirection};
    use crate::paint::{TextAutoResize, TextStyle};
    use crate::Rect;

    fn build_synthetic_doc(frame_count: usize, children_per_frame: usize) -> (Document, NodeId) {
        let mut doc = Document::new("Large Query Test");
        let page = doc.add_page("Page 1");
        for f_idx in 0..frame_count {
            let fid = doc.alloc_id();
            let mut frame = Node::frame(fid, Rect::new((f_idx as f32) * 120.0, 0.0, 100.0, 100.0));
            frame.name = if f_idx % 3 == 0 { "Card".to_string() } else { format!("Card_{f_idx}") };
            if let NodeKind::Frame(ref mut fp) = frame.kind {
                let mut al = AutoLayout::default();
                al.direction = LayoutDirection::Vertical;
                fp.auto_layout = Some(al);
            }
            doc.insert(page, frame).unwrap();
            for c_idx in 0..children_per_frame {
                let cid = doc.alloc_id();
                let child = if c_idx % 2 == 0 {
                    let mut t = Node::text(cid, Rect::new(0.0, 0.0, 80.0, 20.0), format!("Label {f_idx}:{c_idx}"), TextStyle::default());
                    t.name = if c_idx == 0 { format!("btn_{f_idx}") } else { format!("Text_{f_idx}_{c_idx}") };
                    t
                } else {
                    let mut r = Node::rectangle(cid, Rect::new(0.0, 24.0, 80.0, 20.0));
                    r.name = format!("Rect_{f_idx}_{c_idx}");
                    r
                };
                doc.insert(fid, child).unwrap();
            }
        }
        (doc, page)
    }

    #[test]
    fn paging_returns_each_text_once() {
        let (doc, page_id) = build_synthetic_doc(120, 4);
        let expected_texts = doc.nodes.values().filter(|n| matches!(n.kind, NodeKind::Text(_))).count();
        assert_eq!(expected_texts, 240);

        let mut seen_ids = Vec::new();
        let mut cursor = None;
        loop {
            let res = run(
                &doc,
                &QueryOptions {
                    selector: "//TEXT".to_string(),
                    page: Some(page_id),
                    limit: Some(50),
                    max_scanned: Some(150),
                    cursor: cursor.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
            for row in &res.rows {
                seen_ids.push(row[0].as_u64().unwrap());
            }
            if res.done {
                assert!(res.cursor.is_none());
                assert_eq!(res.remaining, 0);
                break;
            }
            cursor = res.cursor;
            assert!(cursor.is_some());
        }
        assert_eq!(seen_ids.len(), expected_texts);
        let unique: HashSet<u64> = seen_ids.iter().copied().collect();
        assert_eq!(unique.len(), expected_texts);
    }

    #[test]
    fn clipped_text_query_matches_linter() {
        let mut doc = Document::new("Clipped Text Test");
        let page = doc.add_page("Page 1");
        let root = doc.alloc_id();
        doc.insert(page, Node::frame(root, Rect::new(0.0, 0.0, 800.0, 600.0))).unwrap();

        for i in 0..20 {
            let tid = doc.alloc_id();
            let mut style = TextStyle::default();
            style.font_size = 16.0;
            let content = if i < 12 { "This is a very long sentence that definitely overflows a narrow 40x24 fixed text box" } else { "Hi" };
            let mut t = Node::text(tid, Rect::new(0.0, (i as f32) * 28.0, 40.0, 24.0), content, style);
            if let NodeKind::Text(ref mut tp) = t.kind {
                tp.auto_resize = TextAutoResize::None;
            }
            doc.insert(root, t).unwrap();
        }

        let cfg = LintConfig::from_rule_list(&["text-truncated".to_string()]).unwrap();
        let mut budget = LintBudget::unlimited();
        let lint_report = lint_subtree(&doc, &[page], &cfg, &crate::lint::ApproxLintEnv, &mut budget);
        let lint_ids: Vec<u64> = lint_report.findings.iter().map(|f| f.node.0).collect();
        assert_eq!(lint_ids.len(), 12);

        let q_page = run(
            &doc,
            &QueryOptions { selector: "//TEXT[has-problem('text-truncated')]".to_string(), page: Some(page), limit: Some(100), ..Default::default() },
        )
        .unwrap();
        let query_ids: Vec<u64> = q_page.rows.iter().map(|r| r[0].as_u64().unwrap()).collect();
        assert_eq!(query_ids, lint_ids);
    }

    #[test]
    fn no_page_exceeds_budget() {
        let (doc, page_id) = build_synthetic_doc(80, 4);
        let mut cursor = None;
        let mut pages = 0usize;
        loop {
            let res = run(
                &doc,
                &QueryOptions {
                    selector: "//TEXT".to_string(),
                    page: Some(page_id),
                    limit: Some(25),
                    max_scanned: Some(60),
                    cursor: cursor.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
            pages += 1;
            assert!(res.rows.len() <= 25);
            assert!(res.scanned <= 60);
            if res.done {
                break;
            }
            cursor = res.cursor;
        }
        assert!(pages >= 6);
    }

    #[test]
    fn cursor_stale_after_edit() {
        let (mut doc, page_id) = build_synthetic_doc(40, 2);
        let p1 = run(&doc, &QueryOptions { selector: "//FRAME".to_string(), page: Some(page_id), limit: Some(10), ..Default::default() }).unwrap();
        let cursor = p1.cursor.expect("expected continuation cursor");

        let extra = doc.alloc_id();
        doc.insert(page_id, Node::rectangle(extra, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        doc.bump_mutation_epoch();

        let err =
            run(&doc, &QueryOptions { selector: "//FRAME".to_string(), page: Some(page_id), limit: Some(10), cursor: Some(cursor), ..Default::default() })
                .unwrap_err();
        assert!(matches!(err, QueryError::CursorStale { .. }));
    }

    #[test]
    fn legacy_selectors_still_work() {
        let (doc, page_id) = build_synthetic_doc(18, 2);
        for sel in ["//FRAME[@name='Card']", "//*[contains(@name,'btn')]"] {
            let q = run(&doc, &QueryOptions { selector: sel.to_string(), page: Some(page_id), limit: Some(500), ..Default::default() }).unwrap();
            let stream_ids: Vec<u64> = q.rows.iter().map(|r| r[0].as_u64().unwrap()).collect();
            let legacy = crate::openpencil::query_xpath(&doc, Some(page_id), sel, 500).unwrap();
            let legacy_ids: Vec<u64> = legacy.iter().map(|v| v["id"].as_u64().unwrap()).collect();
            assert!(!stream_ids.is_empty());
            assert_eq!(stream_ids, legacy_ids);
        }
    }

    #[test]
    fn rule_unavailable_when_no_text_engine() {
        let (doc, page_id) = build_synthetic_doc(2, 2);
        let err = run_with_env(
            &doc,
            &QueryOptions { selector: "//TEXT[has-problem('text-truncated')]".to_string(), page: Some(page_id), ..Default::default() },
            None,
        )
        .unwrap_err();
        assert!(matches!(err, QueryError::RuleUnavailable { .. }));
    }
}
