use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{json, Value};

#[path = "../src/bin/gen-tool-docs.rs"]
mod gen_tool_docs;

use gen_tool_docs::{
    build_tool_snapshots, render_parity_markdown, render_tools_markdown_block, render_tools_snapshot_json, replace_marked_block, repo_root, ParityFile,
    DOC_TARGETS,
};

#[test]
fn tool_list_matches_snapshot() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let snapshot_path = crate_dir.join("tests/snapshots/tools.json");
    let on_disk = fs::read_to_string(&snapshot_path)
        .unwrap_or_else(|e| panic!("missing {} ({e}); run `cargo run -p designos-mcp --bin gen-tool-docs`", snapshot_path.display()));

    let snapshots = build_tool_snapshots();
    assert_eq!(snapshots.len(), 24, "expected 24 declared tools (4 session + 20 engine)");
    let expected = render_tools_snapshot_json(&snapshots);
    assert_eq!(on_disk, expected, "tests/snapshots/tools.json is out of date; run `cargo run -p designos-mcp --bin gen-tool-docs`");
}

#[test]
fn every_dispatched_tool_is_declared() {
    let declared_server: BTreeSet<String> = designos_mcp::server::tools().into_iter().map(|t| t.name.as_ref().to_string()).collect();
    let declared_engine: BTreeSet<String> = designos_mcp::engine::tools().into_iter().map(|t| t["name"].as_str().expect("tool name").to_string()).collect();
    let aliases: BTreeSet<&str> = designos_mcp::engine::ALIASES.iter().copied().collect();
    let dispatched: BTreeSet<&str> = designos_mcp::engine::DISPATCHED.iter().copied().collect();

    for &name in designos_mcp::engine::DISPATCHED {
        assert!(
            declared_server.contains(name) || aliases.contains(name),
            "dispatched tool `{name}` is neither declared in server::tools() nor listed in engine::ALIASES"
        );
        let mut engine = designos_mcp::engine::Engine::default();
        let res = engine.call(name, json!({}));
        assert_ne!(
            res.as_ref().err().map(String::as_str),
            Some(format!("Unknown tool: {name}").as_str()),
            "DISPATCHED entry `{name}` is not handled by Engine::call"
        );
    }

    for name in &declared_engine {
        assert!(dispatched.contains(name.as_str()), "declared engine tool `{name}` is missing from engine::DISPATCHED");
    }

    for &alias in designos_mcp::engine::ALIASES {
        assert!(!alias.starts_with("designos_"), "alias `{alias}` must not be an undocumented `designos_*` tool");
        assert!(dispatched.contains(alias), "alias `{alias}` must be present in engine::DISPATCHED");
    }
}

#[test]
fn docs_blocks_are_current() {
    let root = repo_root();
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let snapshots = build_tool_snapshots();
    let block = render_tools_markdown_block(&snapshots);

    for &rel in DOC_TARGETS {
        let path = root.join(rel);
        let on_disk = fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let expected = replace_marked_block(&on_disk, &block).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(on_disk, expected, "{rel} tool block is out of date; run `cargo run -p designos-mcp --bin gen-tool-docs`");
    }

    let parity_raw = fs::read_to_string(crate_dir.join("parity/figma-mcp-parity.json")).expect("read parity/figma-mcp-parity.json");
    let parity: ParityFile = serde_json::from_str(&parity_raw).expect("parse parity/figma-mcp-parity.json");
    let expected_parity_md = render_parity_markdown(&snapshots, &parity);
    let on_disk_parity_md =
        fs::read_to_string(root.join("docs/mcp-parity.md")).expect("read docs/mcp-parity.md; run `cargo run -p designos-mcp --bin gen-tool-docs`");
    assert_eq!(on_disk_parity_md, expected_parity_md, "docs/mcp-parity.md is out of date; run `cargo run -p designos-mcp --bin gen-tool-docs`");
}

fn collect_known_roadmap_tasks(root: &Path) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();

    let specs_dir = root.join("docs/specs/v0.6");
    if let Ok(entries) = fs::read_dir(&specs_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Some((prefix, _)) = stem.split_once('-') {
                        if prefix.starts_with('T') || prefix.starts_with('E') {
                            ids.insert(prefix.to_string());
                        }
                    }
                }
                if let Ok(text) = fs::read_to_string(&path) {
                    for token in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-') {
                        if (token.starts_with("T7.") || token.starts_with("T8.") || token.starts_with("E9-")) && token.len() >= 4 {
                            ids.insert(token.trim_end_matches('.').to_string());
                        }
                    }
                }
            }
        }
    }

    let plans_dir = root.join("plans/261017-epics-full-scope");
    if let Ok(entries) = fs::read_dir(&plans_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(raw) = fs::read_to_string(&path) {
                    if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                        if let Some(tasks) = val.get("tasks").and_then(Value::as_array) {
                            for task in tasks {
                                if let Some(id) = task.get("id").and_then(Value::as_str) {
                                    ids.insert(id.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    ids
}

#[test]
fn parity_rows_valid() {
    let root = repo_root();
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let parity_raw = fs::read_to_string(crate_dir.join("parity/figma-mcp-parity.json")).expect("read parity/figma-mcp-parity.json");
    let parity: ParityFile = serde_json::from_str(&parity_raw).expect("parse parity/figma-mcp-parity.json");

    assert_eq!(parity.core_tools, 32);
    assert_eq!(parity.weave_tools, 9);
    assert_eq!(parity.rows.len(), parity.core_tools + parity.weave_tools, "parity matrix must list all 32 core + 9 Weave tools");

    let declared_tools: BTreeSet<String> = designos_mcp::server::tools().into_iter().map(|t| t.name.as_ref().to_string()).collect();
    let known_tasks = collect_known_roadmap_tasks(&root);
    let mut seen_figma_tools = BTreeSet::new();

    for row in &parity.rows {
        assert!(seen_figma_tools.insert(row.figma_tool.clone()), "duplicate figma_tool `{}` in parity matrix", row.figma_tool);

        match row.status.as_str() {
            "implemented" => {
                let studio_tool = row.studio_tool.as_deref().unwrap_or_else(|| panic!("implemented row `{}` must specify `studio_tool`", row.figma_tool));
                assert!(declared_tools.contains(studio_tool), "implemented row `{}` references unknown studio_tool `{studio_tool}`", row.figma_tool);
            }
            "task" => {
                let task_id = row.task_id.as_deref().unwrap_or_else(|| panic!("task row `{}` must specify `task_id`", row.figma_tool));
                assert!(
                    task_id.starts_with("T7.") || task_id.starts_with("T8.") || task_id.starts_with("E9-"),
                    "task row `{}` has unexpected task_id prefix `{task_id}`",
                    row.figma_tool
                );
                assert!(
                    known_tasks.contains(task_id),
                    "task row `{}` references task_id `{task_id}` not found in docs/specs/v0.6/ or plans/261017-epics-full-scope/",
                    row.figma_tool
                );
                let implied_studio_tool = format!("designos_{}", row.figma_tool);
                assert!(
                    !declared_tools.contains(&implied_studio_tool),
                    "task row `{}` still says `task` ({task_id}) even though `{implied_studio_tool}` is now declared in server::tools()",
                    row.figma_tool
                );
                if let Some(studio_tool) = row.studio_tool.as_deref() {
                    assert!(
                        !declared_tools.contains(studio_tool),
                        "task row `{}` still says `task` ({task_id}) even though `{studio_tool}` is now declared in server::tools()",
                        row.figma_tool
                    );
                }
            }
            "excluded" => {
                let reason = row.reason.as_deref().unwrap_or_else(|| panic!("excluded row `{}` must specify `reason`", row.figma_tool));
                assert!(
                    reason.contains("R4.13") || reason.contains("R4.14"),
                    "excluded row `{}` reason `{reason}` must cite owner ruling R4.13 or R4.14",
                    row.figma_tool
                );
            }
            other => panic!("row `{}` has invalid status `{other}`; expected implemented | task | excluded", row.figma_tool),
        }
    }
}

#[test]
fn stdio_count_follows_snapshot() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let stdio_src = fs::read_to_string(crate_dir.join("tests/stdio.rs")).expect("read tests/stdio.rs");
    assert!(stdio_src.contains("tests/snapshots/tools.json"), "tests/stdio.rs must read tests/snapshots/tools.json");
    assert!(!stdio_src.contains("assert_eq!(tools.len(), 22)"), "tests/stdio.rs must not hardcode tool count 22");

    let snapshot_raw = fs::read_to_string(crate_dir.join("tests/snapshots/tools.json")).expect("read tests/snapshots/tools.json");
    let snapshot: Vec<Value> = serde_json::from_str(&snapshot_raw).expect("parse tools.json");
    assert_eq!(designos_mcp::server::tools().len(), snapshot.len());
}
