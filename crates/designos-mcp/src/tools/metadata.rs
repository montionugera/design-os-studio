//! MCP tool `designos_get_metadata` (`T7.2`): sparse XML node hierarchy for agent context.

use crate::engine::Engine;
use designos_doc::NodeId;
use designos_export::{metadata_xml, structural_lint_counts, MetadataOptions};
use serde::Deserialize;
use serde_json::{json, Value};

fn default_depth() -> i64 {
    3
}
fn default_max_nodes() -> i64 {
    1_000
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataArgs {
    #[serde(default)]
    pub node_id: Option<u64>,
    #[serde(default)]
    pub page_id: Option<u64>,
    #[serde(default = "default_depth")]
    pub depth: i64,
    #[serde(default = "default_max_nodes")]
    pub max_nodes: i64,
    #[serde(default = "default_true")]
    pub include_hidden: bool,
    #[serde(default)]
    pub lint: bool,
}

/// JSON property schema for `designos_get_metadata`.
pub fn properties_schema() -> Value {
    json!({
        "node_id": {"type": "integer", "minimum": 1},
        "page_id": {"type": "integer", "minimum": 1},
        "depth": {"type": "integer", "minimum": 1, "maximum": 10, "default": 3},
        "max_nodes": {"type": "integer", "minimum": 1, "maximum": 5000, "default": 1000},
        "include_hidden": {"type": "boolean", "default": true},
        "lint": {"type": "boolean", "default": false}
    })
}

/// Executes `designos_get_metadata` against `engine`.
pub fn call_get_metadata(engine: &mut Engine, args: Value) -> Result<Value, String> {
    let a: MetadataArgs = serde_json::from_value(args).map_err(|e| format!("Invalid arguments: {e}"))?;

    if a.node_id.is_some() && a.page_id.is_some() {
        return Err("EXACTLY_ONE_SCOPE: specify at most one of node_id or page_id".into());
    }
    if a.node_id == Some(0) || a.page_id == Some(0) {
        return Err("VALUE_OUT_OF_RANGE: node_id and page_id must be >= 1".into());
    }
    if !(1..=10).contains(&a.depth) {
        return Err(format!("VALUE_OUT_OF_RANGE: depth must be in 1..=10, got {}", a.depth));
    }
    if !(1..=5_000).contains(&a.max_nodes) {
        return Err(format!("VALUE_OUT_OF_RANGE: max_nodes must be in 1..=5000, got {}", a.max_nodes));
    }

    let root = if let Some(nid) = a.node_id {
        let id = NodeId(nid);
        if engine.doc.get(id).is_none() {
            return Err(format!("NODE_NOT_FOUND: node {nid} not found"));
        }
        id
    } else if let Some(pid) = a.page_id {
        let id = NodeId(pid);
        if engine.doc.get(id).is_none() {
            return Err(format!("NODE_NOT_FOUND: page {pid} not found"));
        }
        id
    } else {
        engine.resolve_default_page().map_err(|e| format!("NODE_NOT_FOUND: {e}"))?
    };

    engine.settle_layout();

    let rev = (engine.doc.runtime_id(), engine.doc.mutation_epoch());
    if a.lint {
        let mut counts = structural_lint_counts(&engine.doc, root);
        if let Some((cached_rev, existing)) = &engine.lint_cache {
            if *cached_rev == rev {
                for (&k, &v) in existing {
                    counts.entry(k).or_insert(v);
                }
            }
        }
        engine.lint_cache = Some((rev, counts));
    }

    let cached_map = engine.lint_cache.as_ref().and_then(|(cached_rev, map)| (*cached_rev == rev).then_some(map));
    let lint_ran = a.lint || cached_map.is_some();

    let opts = MetadataOptions {
        depth: a.depth as u8,
        max_nodes: a.max_nodes as usize,
        include_hidden: a.include_hidden,
        scoped: false,
        in_progress: Default::default(),
    };

    let meta = match cached_map {
        Some(map) => {
            let lookup = |id: NodeId| map.get(&id).copied();
            metadata_xml(&engine.doc, root, &opts, Some(&lookup))
        }
        None => metadata_xml(&engine.doc, root, &opts, None),
    };

    let mut out = json!({
        "root_id": meta.root_id,
        "xml": meta.xml,
        "node_count": meta.node_count,
        "truncated": meta.truncated,
        "remaining_estimate": meta.remaining_estimate,
        "hidden_omitted": meta.hidden_omitted,
        "lint_ran": lint_ran,
    });
    if meta.scoped {
        out["scoped"] = json!(true);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use designos_doc::{Node, NodeKind, Rect, TextAutoResize, TextStyle};
    use quick_xml::events::Event;
    use quick_xml::Reader;

    fn assert_valid_xml(xml: &str) -> usize {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().check_end_names = true;
        let mut count = 0usize;
        let mut open_depth = 0usize;
        loop {
            match reader.read_event().expect("XML must parse without error") {
                Event::Start(_) => {
                    count += 1;
                    open_depth += 1;
                }
                Event::Empty(_) => {
                    count += 1;
                }
                Event::End(_) => {
                    assert!(open_depth > 0, "unmatched closing tag");
                    open_depth -= 1;
                }
                Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(open_depth, 0, "all opening tags must be closed");
        count
    }

    #[test]
    fn xml_parses() {
        let mut e = Engine::default();
        let page = e.resolve_default_page().unwrap();
        let fid = e.doc.alloc_id();
        let mut f = Node::frame(fid, Rect::new(0.0, 0.0, 400.0, 300.0));
        f.name = "A & B <\"quote\">\nline2\u{0000}null".to_string();
        e.doc.insert(page, f).unwrap();

        let tid = e.doc.alloc_id();
        let mut t = Node::text(tid, Rect::new(10.0, 10.0, 200.0, 30.0), "Text with & < \" \n and \u{0000} control", TextStyle::default());
        t.name = "Label <1> & \"2\"".to_string();
        e.doc.insert(fid, t).unwrap();

        let res = e.call("designos_get_metadata", json!({})).unwrap();
        let xml = res["xml"].as_str().unwrap();
        assert!(!xml.contains('\u{0000}'), "NUL must be replaced");
        let parsed_elements = assert_valid_xml(xml);
        assert_eq!(parsed_elements, 3);
        assert_eq!(res["node_count"], 3);
    }

    #[test]
    fn count_equals_subtree_to_depth() {
        let mut e = Engine::default();
        let page = e.resolve_default_page().unwrap();
        let root_frame = e.doc.alloc_id();
        e.doc.insert(page, Node::frame(root_frame, Rect::new(0.0, 0.0, 1000.0, 1000.0))).unwrap();

        // Build 3 levels x 4 children under `root_frame`: 1 + 4 + 16 + 64 = 85 nodes
        let mut level1 = Vec::new();
        for i in 0..4 {
            let id1 = e.doc.alloc_id();
            let mut n1 = Node::frame(id1, Rect::new(i as f32 * 100.0, 0.0, 80.0, 80.0));
            n1.name = format!("L1-{i}");
            e.doc.insert(root_frame, n1).unwrap();
            level1.push(id1);
        }
        let mut level2 = Vec::new();
        for &p1 in &level1 {
            for j in 0..4 {
                let id2 = e.doc.alloc_id();
                let mut n2 = Node::frame(id2, Rect::new(j as f32 * 20.0, 0.0, 16.0, 16.0));
                n2.name = format!("L2-{j}");
                e.doc.insert(p1, n2).unwrap();
                level2.push(id2);
            }
        }
        for &p2 in &level2 {
            for k in 0..4 {
                let id3 = e.doc.alloc_id();
                let mut n3 = Node::rectangle(id3, Rect::new(k as f32 * 4.0, 0.0, 3.0, 3.0));
                n3.name = format!("L3-{k}");
                e.doc.insert(p2, n3).unwrap();
            }
        }

        let res_d2 = e.call("designos_get_metadata", json!({"node_id": root_frame.0, "depth": 2})).unwrap();
        assert_eq!(res_d2["node_count"], 21);
        let xml_d2 = res_d2["xml"].as_str().unwrap();
        assert_eq!(assert_valid_xml(xml_d2), 21);
        assert!(xml_d2.contains("children=\"4\" more=\"depth\""));

        let res_d3 = e.call("designos_get_metadata", json!({"node_id": root_frame.0, "depth": 3})).unwrap();
        assert_eq!(res_d3["node_count"], 85);
        let xml_d3 = res_d3["xml"].as_str().unwrap();
        assert_eq!(assert_valid_xml(xml_d3), 85);
        assert!(!xml_d3.contains("more=\"depth\""));
    }

    #[test]
    fn hidden_flagged() {
        let mut e = Engine::default();
        let page = e.resolve_default_page().unwrap();
        let frame = e.doc.alloc_id();
        e.doc.insert(page, Node::frame(frame, Rect::new(0.0, 0.0, 200.0, 200.0))).unwrap();

        let vis = e.doc.alloc_id();
        e.doc.insert(frame, Node::rectangle(vis, Rect::new(0.0, 0.0, 50.0, 50.0))).unwrap();

        let hid = e.doc.alloc_id();
        let mut hnode = Node::rectangle(hid, Rect::new(60.0, 0.0, 50.0, 50.0));
        hnode.name = "SecretLayer".to_string();
        hnode.visible = false;
        e.doc.insert(frame, hnode).unwrap();

        let with_hidden = e.call("designos_get_metadata", json!({"node_id": frame.0, "include_hidden": true})).unwrap();
        let xml_inc = with_hidden["xml"].as_str().unwrap();
        assert!(xml_inc.contains("hidden=\"true\""));
        assert_eq!(with_hidden["hidden_omitted"], 0);
        assert_eq!(with_hidden["node_count"], 3);

        let without_hidden = e.call("designos_get_metadata", json!({"node_id": frame.0, "include_hidden": false})).unwrap();
        let xml_omitted = without_hidden["xml"].as_str().unwrap();
        assert!(!xml_omitted.contains("SecretLayer"));
        assert!(!xml_omitted.contains("hidden=\"true\""));
        assert_eq!(without_hidden["hidden_omitted"], 1);
        assert_eq!(without_hidden["node_count"], 2);
    }

    #[test]
    fn budget_truncates() {
        let mut e = Engine::default();
        let page = e.resolve_default_page().unwrap();
        let root = e.doc.alloc_id();
        e.doc.insert(page, Node::frame(root, Rect::new(0.0, 0.0, 1000.0, 1000.0))).unwrap();

        // 1 root + 199 children = 200 nodes in subtree
        for i in 0..199 {
            let cid = e.doc.alloc_id();
            let mut r = Node::rectangle(cid, Rect::new((i % 10) as f32 * 20.0, (i / 10) as f32 * 20.0, 16.0, 16.0));
            r.name = format!("Item-{i}");
            e.doc.insert(root, r).unwrap();
        }

        let res = e.call("designos_get_metadata", json!({"node_id": root.0, "max_nodes": 50})).unwrap();
        assert_eq!(res["node_count"], 50);
        assert_eq!(res["truncated"], true);
        assert!(res["remaining_estimate"].as_u64().unwrap() > 0);
        let xml = res["xml"].as_str().unwrap();
        assert!(xml.contains("more=\"budget\""));
        assert_eq!(assert_valid_xml(xml), 50);
    }

    #[test]
    fn clipped_text_issue_count() {
        let mut e = Engine::default();
        let page = e.resolve_default_page().unwrap();
        let frame = e.doc.alloc_id();
        e.doc.insert(page, Node::frame(frame, Rect::new(0.0, 0.0, 400.0, 200.0))).unwrap();

        let tid = e.doc.alloc_id();
        let style = TextStyle { font_size: 18.0, ..Default::default() };
        let mut t = Node::text(tid, Rect::new(16.0, 16.0, 40.0, 20.0), "Complete your order right now before items sell out", style);
        if let NodeKind::Text(tp) = &mut t.kind {
            tp.auto_resize = TextAutoResize::None;
        }
        t.name = "Title".to_string();
        e.doc.insert(frame, t).unwrap();

        let res = e.call("designos_get_metadata", json!({"node_id": frame.0, "lint": true})).unwrap();
        assert_eq!(res["lint_ran"], true);
        let xml = res["xml"].as_str().unwrap();
        assert!(
            xml.contains(&format!("id=\"{}\" name=\"Title\"", tid.0)) && xml.contains("issues=\"1\""),
            "expected issues=\"1\" on clipped text node, got:\n{xml}"
        );
    }

    #[test]
    fn bounds_are_page_space() {
        let mut e = Engine::default();
        let page = e.resolve_default_page().unwrap();
        let parent = e.doc.alloc_id();
        let mut pf = Node::frame(parent, Rect::new(100.0, 200.0, 300.0, 300.0));
        pf.name = "Parent".to_string();
        e.doc.insert(page, pf).unwrap();

        let child = e.doc.alloc_id();
        let mut cr = Node::rectangle(child, Rect::new(10.0, 20.0, 50.0, 40.0));
        cr.name = "Child".to_string();
        e.doc.insert(parent, cr).unwrap();

        let res = e.call("designos_get_metadata", json!({"node_id": parent.0})).unwrap();
        let xml = res["xml"].as_str().unwrap();
        assert!(
            xml.contains("name=\"Child\" x=\"110\" y=\"220\" width=\"50\" height=\"40\""),
            "expected page-space bounds x=\"110\" y=\"220\" on child, got:\n{xml}"
        );
    }

    #[test]
    fn scope_and_range_errors() {
        let mut e = Engine::default();
        let err_both = e.call("designos_get_metadata", json!({"node_id": 1, "page_id": 1})).unwrap_err();
        assert!(err_both.contains("EXACTLY_ONE_SCOPE"), "{err_both}");

        let err_depth = e.call("designos_get_metadata", json!({"depth": 11})).unwrap_err();
        assert!(err_depth.contains("VALUE_OUT_OF_RANGE"), "{err_depth}");

        let err_max = e.call("designos_get_metadata", json!({"max_nodes": 5001})).unwrap_err();
        assert!(err_max.contains("VALUE_OUT_OF_RANGE"), "{err_max}");

        let err_missing = e.call("designos_get_metadata", json!({"node_id": 99999})).unwrap_err();
        assert!(err_missing.contains("NODE_NOT_FOUND"), "{err_missing}");
    }
}
