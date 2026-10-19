//! MCP `designos_query_xpath` (`query_nodes`) streaming query handler and schema (E15-17 / #602).

use designos_doc::query::{run, QueryFormat, QueryOptions, DEFAULT_LIMIT, DEFAULT_MAX_SCANNED, MAX_LIMIT, MAX_MAX_SCANNED, MIN_MAX_SCANNED};
use designos_doc::{Document, NodeId};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryXpathToolArgs {
    #[serde(alias = "query")]
    pub selector: String,
    #[serde(default)]
    pub page_id: Option<u64>,
    #[serde(default, alias = "within")]
    pub node_id: Option<u64>,
    #[serde(default)]
    pub node_ids: Option<Vec<u64>>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default, alias = "budget", alias = "node_budget")]
    pub max_scanned: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default, alias = "verbosity", alias = "projection")]
    pub format: Option<String>,
    #[serde(default)]
    pub with_problems: bool,
}

/// Returns the JSON schema properties and required fields for `designos_query_xpath` (`E15-17` §5).
pub fn tool_schema(id_schema: &Value) -> (Value, Vec<&'static str>) {
    let props = json!({
        "selector": {
            "type": "string",
            "minLength": 1,
            "maxLength": 2048,
            "description": "XPath-subset selector. Examples: //TEXT[has-problem('text-truncated')], //FRAME[@name='Card']//*[has-hardcoded('fills/0')], //*[@inProgress]."
        },
        "page_id": id_schema,
        "node_id": id_schema,
        "limit": {
            "type": "integer",
            "minimum": 1,
            "maximum": MAX_LIMIT,
            "default": DEFAULT_LIMIT
        },
        "max_scanned": {
            "type": "integer",
            "minimum": MIN_MAX_SCANNED,
            "maximum": MAX_MAX_SCANNED,
            "default": DEFAULT_MAX_SCANNED
        },
        "cursor": {
            "type": "string",
            "description": "Opaque continuation cursor returned by the previous page."
        },
        "format": {
            "type": "string",
            "enum": ["rows", "objects"],
            "default": "rows"
        },
        "with_problems": {
            "type": "boolean",
            "default": false
        }
    });
    (props, vec!["selector"])
}

/// Executes `designos_query_xpath` against `doc` and `active_page_id`.
pub fn execute_query_xpath(doc: &Document, active_page_id: Option<NodeId>, raw_args: Value) -> Result<Value, String> {
    let args: QueryXpathToolArgs = serde_json::from_value(raw_args).map_err(|e| format!("INVALID_SELECTOR: {e}"))?;

    if args.selector.trim().is_empty() || args.selector.len() > 2048 {
        return Err("INVALID_SELECTOR: selector must be 1..=2048 characters (path: /selector, corrected_example: //FRAME[@name='Card'], safe_to_retry: true)"
            .to_string());
    }

    let explicit_within = args.node_id.or_else(|| args.node_ids.as_ref().and_then(|v| v.first().copied()));

    if args.page_id.is_some() && explicit_within.is_some() {
        return Err("EXACTLY_ONE_SCOPE: provide at most one of page_id or node_id (safe_to_retry: true)".to_string());
    }

    let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(format!("INVALID_LIMIT: limit must be between 1 and {MAX_LIMIT} (safe_to_retry: true)"));
    }

    let max_scanned = args.max_scanned.unwrap_or(DEFAULT_MAX_SCANNED);
    if !(1..=MAX_MAX_SCANNED).contains(&max_scanned) {
        return Err(format!("INVALID_BUDGET: max_scanned must be between {MIN_MAX_SCANNED} and {MAX_MAX_SCANNED} (safe_to_retry: true)"));
    }

    let format = QueryFormat::parse(args.format.as_deref()).map_err(|e| e.to_string())?;

    let resolved_page = if explicit_within.is_some() {
        None
    } else if let Some(pid) = args.page_id {
        if pid == 0 {
            return Err("NODE_NOT_FOUND: page_id must be >= 1".to_string());
        }
        Some(NodeId(pid))
    } else {
        active_page_id.or_else(|| doc.pages.first().copied())
    };

    let within = match explicit_within {
        Some(0) => return Err("NODE_NOT_FOUND: node_id must be >= 1".to_string()),
        Some(id) => Some(NodeId(id)),
        None => None,
    };

    let page = run(
        doc,
        &QueryOptions {
            selector: args.selector,
            page: resolved_page,
            within,
            limit: Some(limit),
            max_scanned: Some(max_scanned),
            cursor: args.cursor,
            format,
            with_problems: args.with_problems,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;

    Ok(json!({
        "columns": page.columns,
        "rows": page.rows,
        "count": page.count,
        "scanned": page.scanned,
        "scanned_total": page.scanned_total,
        "scope_size": page.scope_size,
        "remaining": page.remaining,
        "done": page.done,
        "complete": page.complete,
        "cursor": page.cursor,
    }))
}
