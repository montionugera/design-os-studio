//! Typed, local document operations shared by the MCP transport and future bridges.
use crate::tools::errors::{tool_output_schema, ErrorCode, ToolError};
use designos_doc::live_patch::{live_ops_for_command, LiveOp};
use designos_doc::{Annotation, Command, Comment, CommentAnchor, CommentThread, DevStatus, Document, History, Node, NodeId, NodeKind, NodeSpec, Vec2};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

/// Pre-auto-fix checkpoint saved when an `Auto-fix` undo entry is pushed during a mutating call (`E15-13` §8.4, §10).
#[derive(Clone)]
pub struct AutoFixCheckpoint {
    pub pre_doc: Document,
    pub pre_undo_depth: usize,
    pub post_undo_depth: usize,
    pub pre_revision: u64,
    pub pre_reply: Value,
}

pub struct Engine {
    pub doc: Document,
    pub history: History,
    pub active_page_id: Option<NodeId>,
    /// Edits executed since the last [`Engine::take_live_ops`], recorded while an open editor
    /// mirrors this session (`None` = not recording).
    pub live_ops: Option<Vec<LiveOp>>,
    /// Undo-group label attached to the next [`designos_doc::live_patch::LivePatch`] line when set.
    pub live_patch_label: Option<String>,
    /// Optional RD-3 frame ownership lock: when `Some`, writes outside these root frames are refused.
    pub owned_frames: Option<Vec<NodeId>>,
    /// Per-session options (`designos_configure_session`, E15-12).
    pub session_options: crate::tools::session_options::SessionOptions,
    /// Optional user-level auto-fix settings (`E15-13` §3.4).
    pub user_auto_fix: Option<designos_doc::AutoFixSettings>,
    /// Live ops recorded specifically for the separate `Auto-fix` undo entry of the last mutating call (`E15-13` §3.3).
    pub auto_fix_live_ops: Vec<LiveOp>,
    /// Pre-auto-fix checkpoint of the last mutating call (`E15-13` §8.4, §10).
    pub last_auto_fix_checkpoint: Option<AutoFixCheckpoint>,
    /// Optional test hook for injecting extra findings per auto-fix pass (`E15-13` §10 `two_pass_limit`).
    #[allow(clippy::type_complexity)]
    pub test_lint_hook: Option<std::sync::Arc<dyn Fn(&Document, &[NodeId], usize) -> Vec<designos_doc::lint::Finding> + Send + Sync>>,
    /// When `true` (used by headless tests), disables the text-shaping engine so text rules are
    /// reported in `issues_skipped_rules` (`E15-12` §8.5).
    pub disable_text_lint_engine: bool,
    /// Monotonic in-memory mutation counter used by [`crate::server::Session`] to compute `safe_to_retry`.
    pub revision: u64,
}

impl Default for Engine {
    fn default() -> Self {
        let mut doc = Document::new("Untitled");
        let page = doc.add_page("Page 1");
        Self {
            doc,
            history: History::default(),
            active_page_id: Some(page),
            live_ops: None,
            live_patch_label: None,
            owned_frames: None,
            session_options: crate::tools::session_options::SessionOptions::default(),
            user_auto_fix: None,
            auto_fix_live_ops: Vec::new(),
            last_auto_fix_checkpoint: None,
            test_lint_hook: None,
            disable_text_lint_engine: false,
            revision: 0,
        }
    }
}

impl Engine {
    /// Returns the active [`designos_doc::lint::LintEnv`] for this engine.
    pub fn lint_env(&self) -> &dyn designos_doc::lint::LintEnv {
        if self.disable_text_lint_engine {
            &designos_doc::lint::NoLintEnv
        } else {
            &designos_render::RenderLintEnv
        }
    }

    pub(crate) fn resolved_owned_frames(&self) -> Option<Vec<NodeId>> {
        if let Some(frames) = &self.owned_frames {
            return Some(frames.clone());
        }
        std::env::var("DESIGNOS_OWNED_FRAMES")
            .ok()
            .map(|s| s.split(',').filter_map(|p| p.trim().parse::<u64>().ok()).filter(|&id| id > 0).map(NodeId).collect())
            .filter(|v: &Vec<NodeId>| !v.is_empty())
    }

    pub(crate) fn effective_user_auto_fix(&self) -> Option<designos_doc::AutoFixSettings> {
        if let Some(af) = &self.user_auto_fix {
            return Some(af.clone());
        }
        for var in ["DESIGNOS_LIBRARY", "DESIGNOS_LIBRARY_DIR"] {
            if let Some(p) = std::env::var_os(var).filter(|p| !p.is_empty()) {
                let path = std::path::PathBuf::from(p).join("settings.json");
                if let Ok(raw) = std::fs::read_to_string(path) {
                    if let Some(af) =
                        serde_json::from_str::<Value>(&raw).ok().and_then(|v| v.get("auto_fix").cloned()).and_then(|v| serde_json::from_value(v).ok())
                    {
                        return Some(af);
                    }
                }
                return None;
            }
        }
        #[cfg(not(test))]
        {
            let path = designos_library::Library::default_root().join("settings.json");
            if let Ok(raw) = std::fs::read_to_string(path) {
                if let Some(af) = serde_json::from_str::<Value>(&raw).ok().and_then(|v| v.get("auto_fix").cloned()).and_then(|v| serde_json::from_value(v).ok())
                {
                    return Some(af);
                }
            }
        }
        None
    }

    pub fn resolved_auto_fix_config(&self, doc: &Document) -> designos_doc::lint::ResolvedAutoFixConfig {
        let session_af = self.session_options.auto_fix.to_settings();
        let user_af = self.effective_user_auto_fix();
        designos_doc::lint::ResolvedAutoFixConfig::resolve(Some(&session_af), doc.auto_fix.as_ref(), user_af.as_ref())
    }

    /// Resolves the target page when a tool call omits `parent_id` / `page_id`:
    /// prefers `self.active_page_id` (the user's currently selected page in `design:os studio`)
    /// if valid in `self.doc.pages`, falling back to `self.doc.pages.first()`.
    pub fn resolve_default_page(&self) -> Result<NodeId, ToolError> {
        self.active_page_id
            .filter(|p| self.doc.pages.contains(p))
            .or_else(|| self.doc.pages.first().copied())
            .ok_or_else(|| ToolError::new(ErrorCode::PageNotFound, "/page_id", "Document has no pages"))
    }

    pub(crate) fn collect_library_snapshots(&self, _spec: &NodeSpec) -> Vec<designos_doc::LibrarySnapshot> {
        crate::tools::library_source::load_enabled_snapshots(&self.doc)
    }

    pub(crate) fn check_owned_frames(&self, target: NodeId, path: &str) -> Result<(), ToolError> {
        let Some(frames) = self.resolved_owned_frames() else {
            return Ok(());
        };
        let allowed = frames.iter().any(|&f| f == target || self.doc.is_ancestor(f, target));
        if !allowed {
            let valid: Vec<String> = frames.iter().map(|f| f.0.to_string()).collect();
            return Err(ToolError::new(ErrorCode::OutsideOwnedFrames, path, format!("node {} is outside this agent run's owned frames", target.0))
                .with_valid_options(valid));
        }
        Ok(())
    }

    pub fn call(&mut self, name: &str, args: Value) -> Result<Value, ToolError> {
        self.auto_fix_live_ops.clear();
        self.last_auto_fix_checkpoint = None;
        // Nothing here drains the lazy layout queue between calls, so no tool may start from, read,
        // or serialize a document with deferred layout still in it.
        self.settle_layout();
        match name {
            "designos_get_document" => {
                let _: Empty = parse(args)?;
                Ok(json!({"name":self.doc.name,"kind":self.doc.kind,
                    "active_page_id":self.resolve_default_page().ok().map(|p| p.0),
                    "pages":self.doc.pages.iter().filter_map(|id|self.doc.get(*id)).map(summary).collect::<Vec<_>>(),
                    "node_count":self.doc.nodes.len(),"variable_count":self.doc.variables.len(),
                    "comment_count":self.doc.comments.len(),
                    "style_count":self.doc.styles.len(),
                    "styles":self.doc.styles.iter().map(|(key,values)|{
                        let meta=self.doc.style_meta.get(key);
                        json!({"key":key,"name":self.doc.style_name(key),"property":designos_doc::style_property(&self.doc,key),
                            "description":meta.map(|m|m.description.as_str()).unwrap_or(""),"library_key":meta.and_then(|m|m.key.as_deref()),"values":values})
                    }).collect::<Vec<_>>(),
                    "can_undo":self.history.can_undo(),"can_redo":self.history.can_redo()}))
            }
            "designos_list_nodes" => {
                let a: ListArgs = parse(args)?;
                if a.limit == 0 || a.limit > 500 {
                    return Err(
                        ToolError::new(ErrorCode::ValueOutOfRange, "/limit", "limit must be between 1 and 500").with_corrected_example(json!({ "limit": 100 }))
                    );
                }
                if let Some(id) = a.parent_id {
                    self.node_at(id, "/parent_id")?;
                }
                if let Some(q) = &a.query {
                    bounded_text("query", q, 1024)?;
                }
                let query = a.query.unwrap_or_default().to_lowercase();
                let matching: Vec<_> = self
                    .doc
                    .nodes
                    .values()
                    .filter(|n| a.parent_id.is_none_or(|id| n.parent == Some(NodeId(id))) && n.name.to_lowercase().contains(&query))
                    .collect();
                let nodes: Vec<_> = matching.iter().skip(a.offset).take(a.limit).map(|n| summary(n)).collect();
                let next = a.offset.saturating_add(nodes.len());
                Ok(json!({"nodes":nodes,"total":matching.len(),"offset":a.offset,
                    "next_offset":(next < matching.len()).then_some(next)}))
            }
            "designos_get_node" => {
                let a: NodeArgs = parse(args)?;
                Ok(json!({"node":self.node(a.node_id)?}))
            }
            "designos_get_variables" => {
                let a: VariablesArgs = parse(args)?;
                let effective = if let Some(id) = a.node_id {
                    let n = self.node(id)?;
                    Some(
                        n.variables
                            .bindings
                            .iter()
                            .map(|(property, variable)| {
                                json!({
                                    "property":property,"variable_id":variable,
                                    "mode_id":self.doc.variables.get(variable).and_then(|v|self.doc.variable_mode(n.id,&v.collection)),
                                    "value":self.doc.resolve_variable(n.id,variable),
                                    "code_syntax":self.doc.variables.get(variable).map(|v|&v.code_syntax)
                                })
                            })
                            .collect::<Vec<_>>(),
                    )
                } else {
                    None
                };
                // `code_syntax` is skipped in the native format when empty; tools get a stable shape.
                let mut variables = json!(self.doc.variables);
                for v in variables.as_object_mut().into_iter().flat_map(|m| m.values_mut()).filter_map(|v| v.as_object_mut()) {
                    v.entry("code_syntax").or_insert_with(|| json!({}));
                }
                Ok(json!({"variables":variables,"collections":self.doc.variable_collections,"effective_bindings":effective}))
            }
            "designos_generate_code" => {
                let a: CodeArgs = parse(args)?;
                let id = self.node(a.node_id)?.id;
                let doc = designos_doc::apply_bound_variables(&self.doc);
                let code = match a.language.as_str() {
                    "css" => designos_export::css_for(&doc, id),
                    "tailwind" => designos_export::tailwind_for(&doc, id),
                    "react" => designos_export::react_for(&doc, id),
                    "swiftui" => designos_export::swiftui_for(&doc, id),
                    "uikit" => designos_export::uikit_for(&doc, id),
                    "compose" => designos_export::compose_for(&doc, id),
                    "android-xml" => designos_export::android_xml_for(&doc, id),
                    "svg" => designos_export::export_svg(&doc, id, &Default::default()),
                    _ => {
                        return Err(ToolError::new(
                            ErrorCode::UnknownLanguage,
                            "/language",
                            "language must be css, tailwind, react, swiftui, uikit, compose, android-xml, or svg",
                        )
                        .with_valid_options(vec!["css", "tailwind", "react", "swiftui", "uikit", "compose", "android-xml", "svg"])
                        .with_corrected_example(json!({ "node_id": a.node_id, "language": "react" })));
                    }
                };
                Ok(json!({"node_id":id,"language":a.language,"code":code}))
            }
            "designos_create_node" => self.create(parse(args)?),
            "designos_update_node" => self.update(parse(args)?),
            "designos_delete_node" => {
                let a: DeleteNodeArgs = parse(args)?;
                crate::tools::ledger::validate_idempotency_key_opt(&a.idempotency_key)?;
                let n = self.node(a.node_id)?;
                let Some(former_parent) = n.parent else {
                    return Err(ToolError::new(ErrorCode::PageRootProtected, "/node_id", "page roots cannot be deleted by this tool")
                        .with_corrected_example(json!({ "node_id": 2 })));
                };
                let id = n.id;
                self.check_owned_frames(id, "/node_id")?;
                let baseline =
                    crate::tools::issues::PreCallBaseline::capture(&self.doc, &[former_parent], &self.session_options.issues, a.lint, self.lint_env());
                self.exec(Command::Remove { id })?;
                self.relayout(former_parent);
                let mut out = json!({"node_id":id,"deleted":true});
                crate::tools::issues::finish_mutating_call(self, &[former_parent], a.lint, &baseline, None, &mut out);
                Ok(out)
            }
            "designos_list_comments" => {
                let a: ListCommentsArgs = parse(args)?;
                let list: Vec<_> = self
                    .doc
                    .resolve_comment_targets()
                    .into_iter()
                    .filter(|c| a.resolved.is_none_or(|r| c.resolved == r))
                    .filter(|c| a.page_id.is_none_or(|pid| c.page_id == pid))
                    .filter(|c| a.node_id.is_none_or(|nid| c.target_node_id == Some(nid) || c.target_frame_id == Some(nid)))
                    .collect();
                let total = list.len();
                Ok(json!({"comments": list, "total": total}))
            }
            "designos_add_comment" => {
                let a: AddCommentArgs = parse(args)?;
                crate::tools::ledger::validate_idempotency_key_opt(&a.idempotency_key)?;
                if a.text.trim().is_empty() {
                    return Err(ToolError::new(ErrorCode::ValueOutOfRange, "/text", "text must not be empty")
                        .with_corrected_example(json!({ "text": "Review padding" })));
                }
                bounded_text("text", &a.text, 10_000)?;
                if let Some(author) = &a.author {
                    bounded_text("author", author, 256)?;
                }
                if !a.x.is_finite() || !a.y.is_finite() {
                    return Err(
                        ToolError::new(ErrorCode::ValueOutOfRange, "/x", "x and y must be finite").with_corrected_example(json!({ "x": 0.0, "y": 0.0 }))
                    );
                }
                let page = if let Some(nid) = a.node_id {
                    self.node(nid)?;
                    self.doc
                        .page_of(NodeId(nid))
                        .ok_or_else(|| ToolError::new(ErrorCode::PageNotFound, "/node_id", format!("node {nid} does not belong to a page")))?
                } else if let Some(pid) = a.page_id {
                    let p = NodeId(pid);
                    if !self.doc.pages.contains(&p) {
                        return Err(ToolError::new(ErrorCode::PageNotFound, "/page_id", format!("page {pid} not found")));
                    }
                    p
                } else {
                    self.resolve_default_page()?
                };
                let anchor = CommentAnchor { page, node: a.node_id.map(NodeId), offset: Vec2::new(a.x, a.y) };
                let mut seq = self.doc.comments.len() + 1;
                let thread_id = loop {
                    let candidate = format!("t{seq}");
                    if !self.doc.comments.iter().any(|t| t.id == candidate) {
                        break candidate;
                    }
                    seq += 1;
                };
                let msg_id = format!("{thread_id}-1");
                let mut comments = self.doc.comments.clone();
                comments.push(CommentThread {
                    id: thread_id.clone(),
                    anchor,
                    messages: vec![Comment {
                        id: msg_id,
                        author: a.author.unwrap_or_else(|| "Agent".into()),
                        author_id: None,
                        author_color: None,
                        text: a.text.trim().into(),
                        created: "2026-10-14T23:30:00Z".into(),
                        edited: None,
                    }],
                    resolved: false,
                });
                self.exec(Command::SetComments { comments })?;
                let resolved = self
                    .doc
                    .resolve_comment_targets()
                    .into_iter()
                    .find(|t| t.id == thread_id)
                    .ok_or_else(|| ToolError::new(ErrorCode::InternalError, "", "Failed to resolve comment thread"))?;
                let mut out = serde_json::to_value(&resolved).map_err(|e| ToolError::new(ErrorCode::InternalError, "", e.to_string()))?;
                if let Some(obj) = out.as_object_mut() {
                    obj.insert("thread".into(), serde_json::to_value(&resolved).map_err(|e| ToolError::new(ErrorCode::InternalError, "", e.to_string()))?);
                    obj.insert("changed".into(), json!(true));
                }
                Ok(out)
            }
            "designos_reply_comment" => {
                let a: ReplyCommentArgs = parse(args)?;
                crate::tools::ledger::validate_idempotency_key_opt(&a.idempotency_key)?;
                if a.text.trim().is_empty() {
                    return Err(ToolError::new(ErrorCode::ValueOutOfRange, "/text", "text must not be empty")
                        .with_corrected_example(json!({ "thread_id": a.thread_id, "text": "Addressed" })));
                }
                bounded_text("text", &a.text, 10_000)?;
                if let Some(author) = &a.author {
                    bounded_text("author", author, 256)?;
                }
                let mut comments = self.doc.comments.clone();
                let valid_threads: Vec<String> = comments.iter().map(|t| t.id.clone()).collect();
                let t = comments.iter_mut().find(|t| t.id == a.thread_id).ok_or_else(|| {
                    ToolError::new(ErrorCode::CommentNotFound, "/thread_id", format!("comment thread '{}' not found", a.thread_id))
                        .with_valid_options(valid_threads)
                })?;
                let msg_id = format!("{}-{}", t.id, t.messages.len() + 1);
                t.messages.push(Comment {
                    id: msg_id,
                    author: a.author.unwrap_or_else(|| "Agent".into()),
                    author_id: None,
                    author_color: None,
                    text: a.text.trim().into(),
                    created: "2026-10-14T23:30:00Z".into(),
                    edited: None,
                });
                if let Some(resolved) = a.resolved {
                    t.resolved = resolved;
                }
                self.exec(Command::SetComments { comments })?;
                let resolved_thread = self
                    .doc
                    .resolve_comment_targets()
                    .into_iter()
                    .find(|t| t.id == a.thread_id)
                    .ok_or_else(|| ToolError::new(ErrorCode::InternalError, "", "Failed to resolve comment thread"))?;
                Ok(json!({"thread": resolved_thread, "changed": true}))
            }
            "designos_resolve_comment" => {
                let a: ResolveCommentArgs = parse(args)?;
                crate::tools::ledger::validate_idempotency_key_opt(&a.idempotency_key)?;
                let mut comments = self.doc.comments.clone();
                let valid_threads: Vec<String> = comments.iter().map(|t| t.id.clone()).collect();
                let t = comments.iter_mut().find(|t| t.id == a.thread_id).ok_or_else(|| {
                    ToolError::new(ErrorCode::CommentNotFound, "/thread_id", format!("comment thread '{}' not found", a.thread_id))
                        .with_valid_options(valid_threads)
                })?;
                let changed = t.resolved != a.resolved;
                t.resolved = a.resolved;
                if changed {
                    self.exec(Command::SetComments { comments })?;
                }
                Ok(json!({"thread_id": a.thread_id, "resolved": a.resolved, "changed": changed}))
            }
            "designos_apply_spec" => {
                if let Some(raw_spec) = args.get("spec") {
                    designos_doc::validate_raw_spec_slots(raw_spec).map_err(|e| crate::tools::errors::from_spec_error(e, "/spec"))?;
                }
                let a: ApplySpecArgs = parse_spec(args)?;
                crate::tools::ledger::validate_idempotency_key_opt(&a.idempotency_key)?;
                let parent = match a.parent_id.filter(|&id| id != 0) {
                    Some(id) => {
                        self.node_at(id, "/parent_id")?;
                        NodeId(id)
                    }
                    None => self.resolve_default_page()?,
                };
                self.check_owned_frames(parent, "/parent_id")?;
                if !self.node_at(parent.0, "/parent_id")?.kind.is_container() {
                    return Err(ToolError::new(ErrorCode::NotAContainer, "/parent_id", "parent_id must identify a container")
                        .with_corrected_example(json!({ "parent_id": 1 })));
                }
                let baseline =
                    crate::tools::issues::PreCallBaseline::capture_container(&self.doc, parent, &self.session_options.issues, a.lint, self.lint_env());
                let snapshots = self.collect_library_snapshots(&a.spec);
                let (cmd, report) = designos_doc::apply_spec_command(&mut self.doc, parent, &a.spec, &snapshots)
                    .map_err(|e| crate::tools::errors::from_spec_error(e, "/spec"))?;
                self.exec_apply_spec(cmd)?;
                self.relayout(parent);
                let root_id = report.root_node_id;
                let mut resp = json!({
                    "root_node_id": root_id,
                    "parent_id": parent.0,
                    "node": self.doc.get(root_id),
                    "changed": true,
                    "name_to_id": report.name_to_id,
                    "instances": report.instances,
                    "imported_assets": report.imported_assets,
                    "issues": report.issues,
                    "bindings_created": report.bindings_created,
                    "styles_linked": report.styles_linked,
                    "warnings": report.warnings,
                });
                if !report.stale_library_assets.is_empty() {
                    resp["stale_library_assets"] = json!(report.stale_library_assets);
                }
                crate::tools::issues::finish_mutating_call(self, &[root_id], a.lint, &baseline, Some(root_id), &mut resp);
                Ok(resp)
            }
            "designos_export_spec" => {
                let a: NodeArgs = parse(args)?;
                let spec = self.doc.export_spec(NodeId(a.node_id)).map_err(|e| ToolError::new(ErrorCode::NodeNotFound, "/node_id", e))?;
                Ok(json!({"node_id": a.node_id, "spec": spec}))
            }
            "designos_set_dev_status" => {
                let a: SetDevStatusArgs = parse(args)?;
                crate::tools::ledger::validate_idempotency_key_opt(&a.idempotency_key)?;
                let mut node = self.node(a.node_id)?.clone();
                self.check_owned_frames(node.id, "/node_id")?;
                if let Some(status) = &a.status {
                    node.dev.status = match status.trim().to_ascii_lowercase().as_str() {
                        "ready" | "ready_for_dev" | "readyfordev" => Some(DevStatus::ReadyForDev),
                        "completed" | "done" => Some(DevStatus::Completed),
                        "clear" | "none" | "" => None,
                        other => {
                            return Err(ToolError::new(
                                ErrorCode::InvalidArguments,
                                "/status",
                                format!("status must be 'ready', 'completed', or 'clear' (got '{other}')"),
                            )
                            .with_valid_options(vec!["ready", "completed", "clear", "none"])
                            .with_corrected_example(json!({ "node_id": a.node_id, "status": "ready" })));
                        }
                    };
                }
                if let Some(annotation) = &a.annotation {
                    let trimmed = annotation.trim();
                    if !trimmed.is_empty() {
                        bounded_text("annotation", trimmed, 10_000)?;
                        node.dev.annotations.push(Annotation { text: trimmed.to_string(), properties: Vec::new(), category: None });
                    }
                }
                self.exec(Command::Update { node: Box::new(node) })?;
                Ok(json!({"node_id": a.node_id, "dev": self.doc.get(NodeId(a.node_id)).map(|n| &n.dev), "changed": true}))
            }
            "designos_export_fig_file" | "export_fig_file" => {
                let a: ExportFigArgs = parse(args)?;
                let path = std::path::Path::new(&a.output_path);
                if path.extension().and_then(|x| x.to_str()) != Some("fig") {
                    return Err(ToolError::new(ErrorCode::UnsupportedExtension, "/output_path", "output_path must end in .fig")
                        .with_valid_options(vec![".fig"])
                        .with_corrected_example(json!({ "output_path": "export.fig" })));
                }
                let bytes = designos_fig::export_fig(&self.doc)
                    .map_err(|e| ToolError::new(ErrorCode::IoError, "/output_path", format!("Failed to export .fig: {e}")))?;
                std::fs::write(path, &bytes).map_err(|e| ToolError::new(ErrorCode::IoError, "/output_path", format!("Cannot write .fig file: {e}")))?;
                Ok(json!({"output_path": a.output_path, "path": a.output_path, "bytes": bytes.len(), "saved": true}))
            }
            "designos_query_xpath" | "query_nodes" => {
                let a: QueryXpathArgs = parse(args)?;
                let root = if let Some(pid) = a.page_id {
                    let p = NodeId(pid);
                    if !self.doc.pages.contains(&p) {
                        return Err(ToolError::new(ErrorCode::PageNotFound, "/page_id", format!("page {pid} not found")));
                    }
                    Some(p)
                } else {
                    self.resolve_default_page().ok()
                };
                let selector = a.selector.trim();
                validate_xpath_selector(selector)?;
                let matches: Vec<Value> = query_xpath_nodes(&self.doc, root, selector).into_iter().map(summary).collect();
                let total = matches.len();
                Ok(json!({"selector": a.selector, "page_id": root.map(|r| r.0), "nodes": matches, "total": total}))
            }
            "designos_lint" | "check_design" | "designos_lint_fix" => {
                let a: LintArgs = parse(args)?;
                if (a.page_id.is_some() && (a.node_ids.is_some() || a.node_id.is_some())) || (a.node_id.is_some() && a.node_ids.is_some()) {
                    return Err(ToolError::new(
                        ErrorCode::InvalidArguments,
                        "/page_id",
                        "EXACTLY_ONE_SCOPE: provide at most one of page_id or node_ids (safe_to_retry: true)",
                    ));
                }
                let limit = a.limit.unwrap_or(100);
                if !(1..=500).contains(&limit) {
                    return Err(ToolError::new(ErrorCode::ValueOutOfRange, "/limit", "limit must be between 1 and 500 (safe_to_retry: true)"));
                }
                let roots: Vec<NodeId> = if let Some(pid) = a.page_id {
                    if pid == 0 || self.doc.get(NodeId(pid)).is_none() {
                        return Err(ToolError::new(ErrorCode::NodeNotFound, "/page_id", format!("NODE_NOT_FOUND: page {pid} not found (safe_to_retry: true)")));
                    }
                    vec![NodeId(pid)]
                } else if let Some(nid) = a.node_id {
                    if nid == 0 || self.doc.get(NodeId(nid)).is_none() {
                        return Err(ToolError::new(ErrorCode::NodeNotFound, "/node_id", format!("NODE_NOT_FOUND: node {nid} not found (safe_to_retry: true)")));
                    }
                    vec![NodeId(nid)]
                } else if let Some(ids) = &a.node_ids {
                    if ids.is_empty() || ids.len() > 500 {
                        return Err(ToolError::new(ErrorCode::ValueOutOfRange, "/node_ids", "node_ids must contain 1..=500 IDs (safe_to_retry: true)"));
                    }
                    let mut out = Vec::with_capacity(ids.len());
                    for (idx, &nid) in ids.iter().enumerate() {
                        if nid == 0 || self.doc.get(NodeId(nid)).is_none() {
                            return Err(ToolError::new(
                                ErrorCode::NodeNotFound,
                                format!("/node_ids/{idx}"),
                                format!("NODE_NOT_FOUND: node {nid} not found (safe_to_retry: true)"),
                            ));
                        }
                        out.push(NodeId(nid));
                    }
                    out
                } else {
                    match self.resolve_default_page() {
                        Ok(p) => vec![p],
                        Err(_) => self.doc.pages.clone(),
                    }
                };
                let mut cfg = match a.preset.as_deref() {
                    Some(p) => designos_doc::lint::LintConfig::from_preset_str(p).map_err(|e| match &e {
                        designos_doc::lint::LintConfigError::UnknownPreset { received, valid_options } => {
                            ToolError::new(ErrorCode::InvalidArguments, "/preset", e.to_string())
                                .with_received(json!(received))
                                .with_valid_options(valid_options.iter().copied())
                        }
                        designos_doc::lint::LintConfigError::UnknownRule { received, valid_options } => {
                            ToolError::new(ErrorCode::InvalidArguments, "/rules", e.to_string())
                                .with_received(json!(received))
                                .with_valid_options(valid_options.iter().copied())
                        }
                    })?,
                    None => designos_doc::lint::LintConfig::from_preset(designos_doc::lint::LintPreset::Agent),
                };
                if let Some(rules) = &a.rules {
                    cfg = cfg.with_rules(rules).map_err(|e| match &e {
                        designos_doc::lint::LintConfigError::UnknownPreset { received, valid_options } => {
                            ToolError::new(ErrorCode::InvalidArguments, "/preset", e.to_string())
                                .with_received(json!(received))
                                .with_valid_options(valid_options.iter().copied())
                        }
                        designos_doc::lint::LintConfigError::UnknownRule { received, valid_options } => {
                            ToolError::new(ErrorCode::InvalidArguments, "/rules", e.to_string())
                                .with_received(json!(received))
                                .with_valid_options(valid_options.iter().copied())
                        }
                    })?;
                }
                cfg.include_ignored = a.include_ignored.unwrap_or(false);
                cfg.limit = Some(limit);
                cfg.cursor = a.cursor.clone();

                self.settle_layout();
                if name == "designos_lint_fix" || a.fix == Some(true) {
                    let target_page = a.page_id.map(NodeId).or_else(|| self.resolve_default_page().ok());
                    let (cmds, fix_payload) = designos_doc::lint::lint_fix_commands(&self.doc, target_page, a.rules.as_deref());
                    let fixed_count = fix_payload.get("fixed_count").and_then(Value::as_u64).unwrap_or(0);
                    let changes = fix_payload.get("changes").cloned().unwrap_or_else(|| json!([]));
                    if !cmds.is_empty() {
                        self.exec(Command::Batch(cmds))?;
                        if let Some(p) = target_page {
                            self.relayout(p);
                        } else {
                            self.settle_layout();
                        }
                    }
                    if name == "designos_lint_fix" {
                        return Ok(json!({ "changed": fixed_count > 0, "fixed_count": fixed_count, "changes": changes }));
                    }
                    let mut budget = designos_doc::lint::Budget::unlimited();
                    budget.cursor = a.cursor;
                    let mut val = designos_doc::lint::lint_subtree(&self.doc, &roots, &cfg, &designos_render::RenderLintEnv, &mut budget).to_value();
                    if let Some(obj) = val.as_object_mut() {
                        obj.insert("changed".into(), json!(fixed_count > 0));
                        obj.insert("fixed_count".into(), json!(fixed_count));
                        obj.insert("changes".into(), changes);
                    }
                    return Ok(val);
                }

                let mut budget = designos_doc::lint::Budget::unlimited();
                budget.cursor = a.cursor;
                let report = designos_doc::lint::lint_subtree(&self.doc, &roots, &cfg, &designos_render::RenderLintEnv, &mut budget);
                Ok(report.to_value())
            }
            "designos_apply_batch" => crate::tools::batch::apply(self, args),
            "designos_configure_session" => self.session_options.apply_patch(args),
            "designos_undo" | "designos_redo" => {
                let _: Empty = parse(args)?;
                let undo = name == "designos_undo";
                let next = if undo { self.history.peek_undo() } else { self.history.peek_redo() }.cloned();
                let ops = match &next {
                    Some(command) => self.live_ops_for(command)?,
                    None => Vec::new(),
                };
                let changed = if undo { self.history.undo(&mut self.doc) } else { self.history.redo(&mut self.doc) }
                    .map_err(|e| ToolError::new(ErrorCode::InternalError, "", e.to_string()))?;
                if changed {
                    self.revision = self.revision.wrapping_add(1);
                }
                // Undo and redo defer a deep layout tail to later frames; this engine has no frames.
                self.settle_layout();
                self.record(ops);
                Ok(json!({"changed":changed,"can_undo":self.history.can_undo(),"can_redo":self.history.can_redo()}))
            }
            _ => Err(ToolError::new(ErrorCode::UnknownTool, "", format!("unknown tool: {name}"))),
        }
    }

    /// Re-solves auto layout under `root` and finishes what the pass had to defer. The editor
    /// drains deferred layout a step per frame; this engine has no window, and its document is
    /// read, saved and exported as is.
    pub(crate) fn relayout(&mut self, root: NodeId) {
        self.doc.relayout_subtree(root);
        self.doc.flush_pending_relayout();
    }

    /// Finishes any auto-layout the document still has deferred. Callers that serialize or
    /// export the document directly call this first.
    pub fn settle_layout(&mut self) {
        self.doc.flush_pending_relayout();
    }

    pub(crate) fn node(&self, id: u64) -> Result<&Node, ToolError> {
        self.node_at(id, "/node_id")
    }

    pub(crate) fn node_at(&self, id: u64, path: &str) -> Result<&Node, ToolError> {
        if id == 0 {
            return Err(ToolError::new(ErrorCode::ValueOutOfRange, path, "node IDs must be positive integers").with_corrected_example(json!(1)));
        }
        self.doc.get(NodeId(id)).ok_or_else(|| ToolError::new(ErrorCode::NodeNotFound, path, format!("node {id} not found")))
    }

    pub(crate) fn exec(&mut self, command: Command) -> Result<(), ToolError> {
        let ops = self.live_ops_for(&command)?;
        self.history.exec(&mut self.doc, command).map_err(|e| ToolError::new(ErrorCode::InternalError, "", e.to_string()))?;
        self.revision = self.revision.wrapping_add(1);
        self.record(ops);
        Ok(())
    }

    fn exec_apply_spec(&mut self, command: Command) -> Result<(), ToolError> {
        let ops = if self.live_ops.is_some() {
            let filtered = Self::filter_live_command(&command);
            self.live_ops_for(&filtered)?
        } else {
            Vec::new()
        };
        self.history.exec(&mut self.doc, command).map_err(|e| ToolError::new(ErrorCode::InternalError, "", e.to_string()))?;
        self.revision = self.revision.wrapping_add(1);
        self.record(ops);
        Ok(())
    }

    fn filter_live_command(cmd: &Command) -> Command {
        match cmd {
            Command::SetLibraryState { .. } => Command::Batch(Vec::new()),
            Command::Batch(cmds) => Command::Batch(cmds.iter().map(Self::filter_live_command).collect()),
            other => other.clone(),
        }
    }

    /// The ops `command` will perform, when recording; refuses edits the editor cannot replay.
    pub(crate) fn live_ops_for(&self, command: &Command) -> Result<Vec<LiveOp>, ToolError> {
        if self.live_ops.is_none() {
            return Ok(Vec::new());
        }
        live_ops_for_command(&self.doc, command).map_err(|e| ToolError::new(ErrorCode::InternalError, "", e))
    }

    pub(crate) fn record(&mut self, ops: Vec<LiveOp>) {
        if let Some(log) = &mut self.live_ops {
            log.extend(ops);
        }
    }

    /// Drains the edits recorded since the last call.
    pub fn take_live_ops(&mut self) -> Vec<LiveOp> {
        self.live_ops.as_mut().map(std::mem::take).unwrap_or_default()
    }

    fn create(&mut self, a: CreateArgs) -> Result<Value, ToolError> {
        crate::tools::ledger::validate_idempotency_key_opt(&a.idempotency_key)?;
        let parent_id = match a.parent_id {
            Some(id) => {
                self.node_at(id, "/parent_id")?;
                NodeId(id)
            }
            None => self.resolve_default_page()?,
        };
        self.check_owned_frames(parent_id, "/parent_id")?;
        let baseline = crate::tools::issues::PreCallBaseline::capture_container(&self.doc, parent_id, &self.session_options.issues, a.lint, self.lint_env());
        let (id, index, node) = crate::tools::build_created_node(&mut self.doc, parent_id, None, &a.kind, a.name, a.text, a.x, a.y, a.width, a.height)?;
        self.exec(Command::Insert { parent: parent_id, index, nodes: vec![node] })?;
        if let Some(page_id) = self.doc.page_of(id) {
            self.relayout(page_id);
        }
        let mut out = json!({"node":self.doc.get(id),"parent_id":parent_id.0,"changed":true});
        crate::tools::issues::finish_mutating_call(self, &[id], a.lint, &baseline, Some(id), &mut out);
        Ok(out)
    }

    fn update(&mut self, a: UpdateArgs) -> Result<Value, ToolError> {
        crate::tools::ledger::validate_idempotency_key_opt(&a.idempotency_key)?;
        self.check_owned_frames(NodeId(a.node_id), "/node_id")?;
        let (node, changed) = crate::tools::build_updated_node(
            &self.doc,
            NodeId(a.node_id),
            crate::tools::UpdateFields { name: a.name, text: a.text, x: a.x, y: a.y, width: a.width, height: a.height, visible: a.visible, opacity: a.opacity },
        )?;
        let id = node.id;
        let baseline = crate::tools::issues::PreCallBaseline::capture(&self.doc, &[id], &self.session_options.issues, a.lint, self.lint_env());
        if changed {
            self.exec(Command::Update { node: Box::new(node) })?;
            if let Some(page_id) = self.doc.page_of(id) {
                self.relayout(page_id);
            }
        }
        let mut out = json!({"node":self.doc.get(NodeId(a.node_id)),"changed":changed});
        if changed {
            crate::tools::issues::finish_mutating_call(self, &[id], a.lint, &baseline, Some(id), &mut out);
        } else {
            crate::tools::issues::finish_dry_run_call(self, &self.doc, &[id], a.lint, &baseline, &mut out);
        }
        Ok(out)
    }
}

fn validate_xpath_selector(selector: &str) -> Result<(), ToolError> {
    let mut depth = 0i32;
    for ch in selector.chars() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth < 0 {
                    return Err(ToolError::new(ErrorCode::InvalidSelector, "/selector", format!("invalid XPath selector '{selector}': unmatched ']'"))
                        .with_corrected_example(json!({ "selector": "//FRAME[@name='Card']" })));
                }
            }
            _ => {}
        }
    }
    if depth != 0 || (selector.starts_with('/') && !selector.starts_with("//")) {
        return Err(ToolError::new(ErrorCode::InvalidSelector, "/selector", format!("invalid XPath selector '{selector}'"))
            .with_corrected_example(json!({ "selector": "//FRAME[@name='Card']" })));
    }
    Ok(())
}

fn matches_single_step(doc: &Document, n: &Node, step: &str) -> bool {
    let kind_upper = n.kind.default_name().to_ascii_uppercase();
    if step.is_empty() || step == "*" {
        return true;
    }
    if let Some((tag_part, rest)) = step.split_once('[') {
        let tag_ok = tag_part.is_empty() || tag_part == "*" || kind_upper == tag_part.to_ascii_uppercase();
        if !tag_ok {
            return false;
        }
        let pred_clean = rest.trim_end_matches(']').trim();
        if let Some(val) = pred_clean.strip_prefix("@name=") {
            let unquoted = val.trim().trim_matches('\'').trim_matches('"');
            n.name == unquoted
        } else if pred_clean.contains("contains(@name") {
            let parts: Vec<&str> = pred_clean.split(['\'', '"']).collect();
            parts.get(1).is_some_and(|needle| n.name.to_lowercase().contains(&needle.to_lowercase()))
        } else if let Some(val) = pred_clean.strip_prefix("@component=") {
            let unquoted = val.trim().trim_matches('\'').trim_matches('"');
            designos_doc::instance_component(n).and_then(|cid| doc.get(cid)).is_some_and(|cn| {
                cn.name == unquoted || cn.parent.and_then(|pid| doc.get(pid)).is_some_and(|p| matches!(p.kind, NodeKind::ComponentSet(_)) && p.name == unquoted)
            })
        } else if pred_clean == "@fill" {
            !n.fills.is_empty()
        } else {
            true
        }
    } else {
        kind_upper == step.to_ascii_uppercase() || n.name.to_lowercase().contains(&step.to_lowercase())
    }
}

fn query_xpath_nodes<'a>(doc: &'a Document, root: Option<NodeId>, selector: &str) -> Vec<&'a Node> {
    let mut matches = Vec::new();
    let steps: Vec<&str> = if let Some(rest) = selector.strip_prefix("//") { rest.split("//").filter(|s| !s.is_empty()).collect() } else { Vec::new() };

    let candidate_ids: Vec<NodeId> = if let Some(r) = root {
        let mut ids = Vec::new();
        doc.walk(r, |n, _| ids.push(n.id));
        ids
    } else {
        doc.nodes.keys().copied().collect()
    };

    for nid in candidate_ids {
        let Some(n) = doc.get(nid) else {
            continue;
        };
        let matched = if selector.is_empty() || selector == "//*" || selector == "*" {
            true
        } else if steps.len() >= 2 {
            let last = steps[steps.len() - 1];
            if !matches_single_step(doc, n, last) {
                false
            } else {
                let mut cur_parent = n.parent;
                let mut idx = steps.len() - 1;
                while idx > 0 {
                    let target_step = steps[idx - 1];
                    let mut found = None;
                    while let Some(pid) = cur_parent {
                        if let Some(pnode) = doc.get(pid) {
                            if matches_single_step(doc, pnode, target_step) {
                                found = pnode.parent;
                                break;
                            }
                            cur_parent = pnode.parent;
                        } else {
                            break;
                        }
                    }
                    if found.is_some() || (idx == 1 && cur_parent.is_some()) {
                        cur_parent = found;
                        idx -= 1;
                    } else {
                        break;
                    }
                }
                idx == 0
            }
        } else if let Some(&only) = steps.first() {
            matches_single_step(doc, n, only)
        } else {
            matches_single_step(doc, n, selector)
        };
        if matched {
            matches.push(n);
        }
    }
    matches
}

fn summary(n: &Node) -> Value {
    json!({"node_id":n.id,"name":n.name,"kind":n.kind.default_name(),"parent_id":n.parent,
        "child_count":n.children.len(),"visible":n.visible,"x":n.transform.tx,"y":n.transform.ty,
        "width":n.size.x,"height":n.size.y})
}

pub(crate) fn parse<T: DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| crate::tools::errors::from_serde_error(&e, "", false))
}

pub(crate) fn parse_spec<T: DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| crate::tools::errors::from_serde_error(&e, "", true))
}

use crate::tools::bounded_text;
fn default_limit() -> usize {
    100
}
fn default_size() -> f32 {
    100.0
}
fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeArgs {
    node_id: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteNodeArgs {
    node_id: u64,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    lint: Option<crate::tools::session_options::PerCallLintOverride>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    parent_id: Option<u64>,
    query: Option<String>,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VariablesArgs {
    node_id: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeArgs {
    node_id: u64,
    language: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    #[serde(default)]
    parent_id: Option<u64>,
    kind: String,
    name: Option<String>,
    text: Option<String>,
    #[serde(default)]
    x: f32,
    #[serde(default)]
    y: f32,
    #[serde(default = "default_size")]
    width: f32,
    #[serde(default = "default_size")]
    height: f32,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    lint: Option<crate::tools::session_options::PerCallLintOverride>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    node_id: u64,
    name: Option<String>,
    text: Option<String>,
    x: Option<f32>,
    y: Option<f32>,
    width: Option<f32>,
    height: Option<f32>,
    visible: Option<bool>,
    opacity: Option<f32>,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    lint: Option<crate::tools::session_options::PerCallLintOverride>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListCommentsArgs {
    resolved: Option<bool>,
    page_id: Option<u64>,
    node_id: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddCommentArgs {
    text: String,
    author: Option<String>,
    node_id: Option<u64>,
    page_id: Option<u64>,
    #[serde(default)]
    x: f32,
    #[serde(default)]
    y: f32,
    #[serde(default)]
    idempotency_key: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyCommentArgs {
    thread_id: String,
    text: String,
    author: Option<String>,
    resolved: Option<bool>,
    #[serde(default)]
    idempotency_key: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveCommentArgs {
    thread_id: String,
    #[serde(default = "default_true")]
    resolved: bool,
    #[serde(default)]
    idempotency_key: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplySpecArgs {
    parent_id: Option<u64>,
    spec: NodeSpec,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    lint: Option<crate::tools::session_options::PerCallLintOverride>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetDevStatusArgs {
    node_id: u64,
    status: Option<String>,
    annotation: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExportFigArgs {
    #[serde(alias = "path")]
    pub output_path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryXpathArgs {
    #[serde(default, alias = "query")]
    selector: String,
    #[serde(default)]
    page_id: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LintArgs {
    #[serde(default)]
    page_id: Option<u64>,
    #[serde(default)]
    node_id: Option<u64>,
    #[serde(default)]
    node_ids: Option<Vec<u64>>,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default)]
    rules: Option<Vec<String>>,
    #[serde(default)]
    include_ignored: Option<bool>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    fix: Option<bool>,
}

/// JSON tool descriptors kept beside the argument validators.
pub fn tools() -> Vec<Value> {
    let id = json!({"type":"integer","minimum":1,"description":"DesignOS numeric node ID; obtain from list_nodes/get_document."});
    let coordinate = json!({"type":"number","minimum":-1000000,"maximum":1000000});
    let size = json!({"type":"number","minimum":0,"maximum":1000000});
    let text = json!({"type":"string","maxLength":100000,"description":"Plain text; limited to 100000 UTF-8 bytes."});
    let name = json!({"type":"string","maxLength":1024,"description":"Layer name; limited to 1024 UTF-8 bytes."});
    let lint_opt = json!({"anyOf":[{"type":"boolean"},{"type":"string","enum":["preset","errors_only","off"]}],"description":"Optional per-call lint override (false or 'off' skips the per-call lint pass)."});
    let idempotency_key = json!({
        "type": "string",
        "minLength": 1,
        "maxLength": 128,
        "pattern": "^[A-Za-z0-9._:-]{1,128}$",
        "description": "Optional agent-supplied idempotency key for deduplicating replayed writes within the active session."
    });
    let mut out = Vec::new();
    let mut add =
        |tool: &str, description: &str, properties: Value, required: Vec<&str>, out_props: Value, out_req: &[&str], read_only: bool, idempotent: bool| {
            out.push(json!({"name":format!("designos_{tool}"),"description":description,
            "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
            "outputSchema":tool_output_schema(out_props, out_req),
            "annotations":{"readOnlyHint":read_only,"destructiveHint":!read_only,"idempotentHint":idempotent,"openWorldHint":false}}));
        };
    add(
        "get_document",
        "Summarize the current in-memory document: pages, counts, shared styles (fill, text, effect, grid) with their values, and undo state.",
        json!({}),
        vec![],
        json!({
            "name": {"type": "string"},
            "kind": {"type": "string"},
            "active_page_id": {"type": ["integer", "null"]},
            "pages": {"type": "array"},
            "node_count": {"type": "integer"},
            "variable_count": {"type": "integer"},
            "comment_count": {"type": "integer"},
            "style_count": {"type": "integer"},
            "styles": {"type": "array"},
            "can_undo": {"type": "boolean"},
            "can_redo": {"type": "boolean"}
        }),
        &["name", "pages", "node_count", "can_undo", "can_redo"],
        true,
        true,
    );
    add(
        "list_nodes",
        "List node summaries in numeric ID order. Query matches names case-insensitively; parent_id filters direct children. Use next_offset to paginate.",
        json!({"parent_id":id,"query":{"type":"string","maxLength":1024},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"maximum":500,"default":100}}),
        vec![],
        json!({
            "nodes": {"type": "array"},
            "total": {"type": "integer"},
            "offset": {"type": "integer"},
            "next_offset": {"type": ["integer", "null"]}
        }),
        &["nodes", "total", "offset"],
        true,
        true,
    );
    add(
        "get_node",
        "Read the full native node including geometry, paints, text, bindings, and child IDs.",
        json!({"node_id":id}),
        vec!["node_id"],
        json!({ "node": {"type": "object"} }),
        &["node"],
        true,
        true,
    );
    add(
        "get_variables",
        "Read variable definitions and collections. With node_id, also resolve that node's direct bindings using inherited collection modes.",
        json!({"node_id":id}),
        vec![],
        json!({
            "variables": {"type": "object"},
            "collections": {"type": "object"},
            "effective_bindings": {"type": ["array", "null"]}
        }),
        &["variables", "collections"],
        true,
        true,
    );
    add(
        "generate_code",
        "Generate developer handoff code or SVG for a layer using resolved variable values. Returns source text; writes no files.",
        json!({"node_id":id,"language":{"type":"string","enum":["css","tailwind","react","swiftui","uikit","compose","android-xml","svg"]}}),
        vec!["node_id", "language"],
        json!({
            "node_id": {"type": "integer"},
            "language": {"type": "string"},
            "code": {"type": "string"}
        }),
        &["node_id", "language", "code"],
        true,
        true,
    );
    add(
        "create_node",
        "Create one layer in a container; defaults to x/y=0, width/height=100. Supports undo. Text is only valid for text layers. Changes are in memory until saved.",
        json!({"parent_id":id,"kind":{"type":"string","enum":["frame","rectangle","ellipse","text"]},"name":name,"text":text,"x":coordinate,"y":coordinate,"width":size,"height":size,"idempotency_key":idempotency_key,"lint":lint_opt}),
        vec!["parent_id", "kind"],
        json!({
            "node": {"type": "object"},
            "parent_id": {"type": "integer"},
            "changed": {"type": "boolean"}
        }),
        &["node", "parent_id", "changed"],
        false,
        false,
    );
    add(
        "update_node",
        "Update specified fields of one non-page layer. Coordinates are parent-relative. Replacing text clears text runs and list ranges. Supports undo; changes remain in memory until saved.",
        json!({"node_id":id,"name":name,"text":text,"x":coordinate,"y":coordinate,"width":size,"height":size,"visible":{"type":"boolean"},"opacity":{"type":"number","minimum":0,"maximum":1},"idempotency_key":idempotency_key,"lint":lint_opt}),
        vec!["node_id"],
        json!({
            "node": {"type": "object"},
            "changed": {"type": "boolean"}
        }),
        &["node", "changed"],
        false,
        true,
    );
    add(
        "delete_node",
        "Remove a non-page layer and its entire subtree. Supports undo; changes remain in memory until saved.",
        json!({"node_id":id,"idempotency_key":idempotency_key,"lint":lint_opt}),
        vec!["node_id"],
        json!({
            "node_id": {"type": "integer"},
            "deleted": {"type": "boolean"}
        }),
        &["node_id", "deleted"],
        false,
        false,
    );
    add(
        "list_comments",
        "List canvas comment threads enriched with resolved world coordinates, target node ID/name/kind, and top-level frame ID/name. Filter by resolved state, page_id, or node_id.",
        json!({"resolved":{"type":"boolean"},"page_id":id,"node_id":id}),
        vec![],
        json!({
            "comments": {"type": "array"},
            "total": {"type": "integer"}
        }),
        &["comments", "total"],
        true,
        true,
    );
    add(
        "add_comment",
        "Pin a new comment thread to a node (with local offset x/y) or page coordinates. Supports undo.",
        json!({"text":{"type":"string","minLength":1,"maxLength":10000},"author":{"type":"string","maxLength":256},"node_id":id,"page_id":id,"x":coordinate,"y":coordinate,"idempotency_key":idempotency_key}),
        vec!["text"],
        json!({
            "id": {"type": "string"},
            "thread": {"type": "object"},
            "changed": {"type": "boolean"}
        }),
        &["id", "thread", "changed"],
        false,
        false,
    );
    add(
        "reply_comment",
        "Append a reply message to an existing canvas comment thread and optionally mark it resolved/unresolved. Supports undo.",
        json!({"thread_id":{"type":"string","minLength":1},"text":{"type":"string","minLength":1,"maxLength":10000},"author":{"type":"string","maxLength":256},"resolved":{"type":"boolean"},"idempotency_key":idempotency_key}),
        vec!["thread_id", "text"],
        json!({
            "thread": {"type": "object"},
            "changed": {"type": "boolean"}
        }),
        &["thread", "changed"],
        false,
        false,
    );
    add(
        "resolve_comment",
        "Mark a canvas comment thread resolved (default true) or unresolved (false). Supports undo.",
        json!({"thread_id":{"type":"string","minLength":1},"resolved":{"type":"boolean","default":true},"idempotency_key":idempotency_key}),
        vec!["thread_id"],
        json!({
            "thread_id": {"type": "string"},
            "resolved": {"type": "boolean"},
            "changed": {"type": "boolean"}
        }),
        &["thread_id", "resolved", "changed"],
        false,
        true,
    );
    add(
        "apply_spec",
        concat!(
            "Build a complete UI subtree from a declarative NodeSpec (supports EaseUI BrainWave 2.0 presets like easeui-screen/easeui-card/easeui-button-primary, $easeui.* tokens, and Auto-Layout 5.0 row/column/wrap/hug/fill) in a single atomic undoable step together with the variables it creates. Replies with bindings_created, styles_linked and warnings (for example DEPRECATED_EASEUI_TOKEN).\n\n",
            "Examples:\n",
            "1) Token-bound frame with auto-layout:\n",
            "{\"parent_id\":1,\"spec\":{\"kind\":\"frame\",\"name\":\"Hero\",\"layout\":{\"direction\":\"column\",\"gap\":16,\"padding\":24},\"fill\":\"var:Color/Surface\",\"children\":[{\"kind\":\"text\",\"name\":\"Title\",\"text\":\"Welcome\"}]}}\n",
            "2) Component instance with variant and overrides:\n",
            "{\"parent_id\":1,\"spec\":{\"kind\":\"instance\",\"name\":\"PrimaryCTA\",\"component\":\"Button\",\"variant\":{\"Size\":\"lg\",\"State\":\"default\"},\"overrides\":[{\"path\":\"Label\",\"text\":\"Continue\"}]}}\n",
            "3) Component instance with filled slots:\n",
            "{\"parent_id\":1,\"spec\":{\"kind\":\"instance\",\"name\":\"SettingsCard\",\"component\":\"Card\",\"slots\":{\"header\":[{\"kind\":\"instance\",\"component\":\"CardHeader\"}],\"body\":[{\"kind\":\"instance\",\"component\":\"FormRow\"}],\"footer\":false}}}"
        ),
        json!({"parent_id":id,"spec":{"type":"object","description":"Declarative NodeSpec tree (kind, name, preset, width, height, width_sizing, height_sizing, fill, stroke, corner_radius, text, font_size, layout, dev_status, annotations, children)."},"idempotency_key":idempotency_key,"lint":lint_opt}),
        vec!["spec"],
        json!({
            "root_node_id": {"type": "integer"},
            "parent_id": {"type": "integer"},
            "node": {"type": "object"},
            "changed": {"type": "boolean"},
            "name_to_id": {"type": "object"},
            "instances": {"type": "array"},
            "imported_assets": {"type": "array"},
            "issues": {"type": "array"},
            "bindings_created": {"type": "integer"},
            "styles_linked": {"type": "integer"},
            "warnings": {"type": "array"}
        }),
        &["root_node_id", "parent_id", "node", "changed", "name_to_id", "instances", "issues"],
        false,
        false,
    );
    add(
        "export_spec",
        "Export a node and its descendants as a declarative NodeSpec JSON tree, enriched with Dev Mode status/annotations and any canvas comment threads pinned within the subtree.",
        json!({"node_id":id}),
        vec!["node_id"],
        json!({
            "node_id": {"type": "integer"},
            "spec": {"type": "object"}
        }),
        &["node_id", "spec"],
        true,
        true,
    );
    add(
        "set_dev_status",
        "Set a layer's Dev Mode handoff status ('ready', 'completed', or 'clear') and/or append a developer annotation. Supports undo.",
        json!({"node_id":id,"status":{"type":"string","enum":["ready","completed","clear","none"]},"annotation":{"type":"string","maxLength":10000},"idempotency_key":idempotency_key}),
        vec!["node_id"],
        json!({
            "node_id": {"type": "integer"},
            "dev": {"type": "object"},
            "changed": {"type": "boolean"}
        }),
        &["node_id", "dev", "changed"],
        false,
        false,
    );
    add(
        "export_fig_file",
        "Export the current session document to a Figma binary .fig container (with fig-kiwi header and deflate chunks) inside the workspace.",
        json!({"output_path":{"type":"string","description":"Destination .fig path inside the workspace."}}),
        vec!["output_path"],
        json!({
            "output_path": {"type": "string"},
            "path": {"type": "string"},
            "bytes": {"type": "integer"},
            "saved": {"type": "boolean"}
        }),
        &["output_path", "bytes", "saved"],
        false,
        true,
    );
    add(
        "query_xpath",
        concat!(
            "Query layers in the document using an XPath-like selector (tag, @name, @component, @fill, and //descendant steps).\n\n",
            "Examples:\n",
            "1) Find instances by component name:\n",
            "{\"selector\":\"//INSTANCE[@component='Button']\"}\n",
            "2) Find text descendants inside a named frame:\n",
            "{\"selector\":\"//FRAME[@name='Hero']//TEXT\"}\n",
            "3) Find layers that have a fill:\n",
            "{\"selector\":\"//RECTANGLE[@fill]\",\"page_id\":1}"
        ),
        json!({"selector":{"type":"string","description":"XPath-style selector (e.g. //FRAME[@name='Card']//TEXT)."},"page_id":id}),
        vec!["selector"],
        json!({
            "selector": {"type": "string"},
            "page_id": {"type": ["integer", "null"]},
            "nodes": {"type": "array"},
            "total": {"type": "integer"}
        }),
        &["selector", "nodes", "total"],
        true,
        true,
    );
    add(
        "lint",
        "Run the unified solver-backed geometric, text, token, and structural design linter on a page, specific node subtrees, or the active page.",
        json!({
            "page_id": id,
            "node_ids": {"type": "array", "items": id, "maxItems": 500},
            "preset": {"type": "string", "default": "agent"},
            "rules": {"type": "array", "items": {"type": "string"}},
            "include_ignored": {"type": "boolean", "default": false},
            "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 100},
            "cursor": {"type": "string"}
        }),
        vec![],
        json!({
            "findings": {"type": "array"},
            "total": {"type": "integer"},
            "by_rule": {"type": "object"},
            "by_severity": {"type": "object"},
            "scanned": {"type": "integer"},
            "complete": {"type": "boolean"},
            "next_cursor": {"type": ["string", "null"]},
            "skipped_rules": {"type": "array"}
        }),
        &["findings", "total", "by_rule", "by_severity", "scanned", "complete"],
        true,
        true,
    );
    add(
        "undo",
        "Undo the latest successful edit in this session; returns changed=false if the stack is empty.",
        json!({}),
        vec![],
        json!({
            "changed": {"type": "boolean"},
            "can_undo": {"type": "boolean"},
            "can_redo": {"type": "boolean"}
        }),
        &["changed", "can_undo", "can_redo"],
        false,
        false,
    );
    add(
        "redo",
        "Redo the most recently undone edit in this session; returns changed=false if the stack is empty.",
        json!({}),
        vec![],
        json!({
            "changed": {"type": "boolean"},
            "can_undo": {"type": "boolean"},
            "can_redo": {"type": "boolean"}
        }),
        &["changed", "can_undo", "can_redo"],
        false,
        false,
    );
    out.push(crate::tools::batch::tool_descriptor());
    out.push(crate::tools::session_options::tool_descriptor());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use designos_doc::{NodeKind, Rect};

    #[test]
    fn variable_inspection_and_codegen_resolve_inherited_cross_collection_aliases() {
        use designos_doc::{Variable, VariableCollection, VariableKind, VariableMode, VariableValue};
        let mut e = Engine::default();
        let id = create(&mut e, "rectangle", "Box");
        for collection in ["alias", "size"] {
            e.doc.variable_collections.insert(
                collection.into(),
                VariableCollection {
                    id: collection.into(),
                    name: collection.into(),
                    modes: vec![VariableMode { id: "light".into(), name: "Light".into() }, VariableMode { id: "dark".into(), name: "Dark".into() }],
                    default_mode: "light".into(),
                    variables: vec![],
                    key: None,
                    source_library_key: None,
                },
            );
        }
        for (id, collection, light, dark) in [
            ("width", "size", VariableValue::Number(100.0), VariableValue::Number(240.0)),
            ("bound", "alias", VariableValue::Alias("width".into()), VariableValue::Alias("width".into())),
        ] {
            e.doc.variables.insert(
                id.into(),
                Variable {
                    id: id.into(),
                    name: id.into(),
                    collection: collection.into(),
                    kind: VariableKind::Number,
                    values: [("light".into(), light), ("dark".into(), dark)].into_iter().collect(),
                    description: String::new(),
                    scopes: vec![],
                    hidden_from_publishing: false,
                    key: None,

                    code_syntax: Default::default(),
                },
            );
        }
        e.doc.get_mut(NodeId(1)).unwrap().variables.modes.insert("size".into(), "dark".into());
        e.doc.get_mut(NodeId(id)).unwrap().variables.bindings.insert("width".into(), "bound".into());
        e.doc.variables.get_mut("bound").unwrap().code_syntax.insert(designos_doc::CodeSyntaxPlatform::Web, "var(--size-bound, 240px)".into());
        e.doc.variables.get_mut("bound").unwrap().code_syntax.insert(designos_doc::CodeSyntaxPlatform::Ios, "Size.bound".into());
        let before = e.doc.to_json().unwrap();
        let values = e.call("designos_get_variables", json!({"node_id":id})).unwrap();
        assert_eq!(values["effective_bindings"][0]["value"], json!({"Number":240.0}));
        let syntax = json!({"WEB": "var(--size-bound, 240px)", "iOS": "Size.bound"});
        assert_eq!(values["variables"]["bound"]["code_syntax"], syntax);
        assert_eq!(values["effective_bindings"][0]["code_syntax"], syntax);
        assert_eq!(values["variables"]["width"]["code_syntax"], json!({}));
        let css = e.call("designos_generate_code", json!({"node_id":id,"language":"css"})).unwrap();
        assert!(css["code"].as_str().unwrap().contains("width: var(--size-bound, 240px);"), "{css}");
        let svg = e.call("designos_generate_code", json!({"node_id":id,"language":"svg"})).unwrap();
        assert!(svg["code"].as_str().unwrap().contains("width=\"240\""));
        assert_eq!(e.doc.to_json().unwrap(), before);
    }
    #[test]
    fn get_document_lists_shared_effect_and_grid_styles_and_nodes_carry_their_links() {
        use designos_doc::{Color, Effect, LayoutGrid, Style, StyleMeta, StyleProperty, Vec2};
        let mut e = Engine::default();
        let id = create(&mut e, "frame", "Card");
        assert_eq!(e.call("designos_get_document", json!({})).unwrap()["styles"], json!([]), "no styles yet");
        let shadow =
            Effect::DropShadow { color: Color::rgba(0, 0, 0, 64), offset: Vec2::new(0.0, 4.0), radius: 8.0, spread: 0.0, visible: true, show_behind: false };
        e.doc.styles.insert("Shadow/Card".into(), Style::Effect(vec![shadow]));
        e.doc.styles.insert("Layout/Cols".into(), Style::Grid(vec![LayoutGrid::columns(12, 20.0, 0.0, Color::rgba(255, 0, 0, 26))]));
        for (key, name, property) in [("Shadow/Card", "Shadow/Card", StyleProperty::Effect), ("Layout/Cols", "Layout / 12 col", StyleProperty::Grid)] {
            e.doc.style_meta.insert(key.into(), StyleMeta { name: name.into(), key: None, description: String::new(), property });
        }
        e.doc.get_mut(NodeId(id)).unwrap().style_refs.insert("effect".into(), "Shadow/Card".into());
        let doc = e.call("designos_get_document", json!({})).unwrap();
        assert_eq!(doc["style_count"], 2);
        let styles = doc["styles"].as_array().unwrap();
        let by_key = |k: &str| styles.iter().find(|s| s["key"] == k).unwrap_or_else(|| panic!("style {k} listed"));
        assert_eq!((by_key("Shadow/Card")["property"].as_str(), by_key("Shadow/Card")["name"].as_str()), (Some("Effect"), Some("Shadow/Card")));
        assert_eq!(by_key("Shadow/Card")["values"]["Effect"][0]["DropShadow"]["radius"], 8.0);
        assert_eq!((by_key("Layout/Cols")["property"].as_str(), by_key("Layout/Cols")["name"].as_str()), (Some("Grid"), Some("Layout / 12 col")));
        assert_eq!(by_key("Layout/Cols")["values"]["Grid"][0]["Columns"]["count"], 12);
        let node = e.call("designos_get_node", json!({"node_id":id})).unwrap();
        assert_eq!(node["node"]["style_refs"]["effect"], "Shadow/Card");
    }
    fn create(e: &mut Engine, kind: &str, name: &str) -> u64 {
        e.call("designos_create_node", json!({"parent_id":1,"kind":kind,"name":name})).unwrap()["node"]["id"].as_u64().unwrap()
    }
    #[test]
    fn crud_round_trips_through_undo_and_redo() {
        let mut e = Engine::default();
        let id = create(&mut e, "text", "Title");
        let before = e.doc.to_json().unwrap();
        e.call("designos_update_node", json!({"node_id":id,"text":"Hello","x":17,"visible":false,"opacity":0.5})).unwrap();
        assert_eq!(e.doc.get(NodeId(id)).unwrap().transform.tx, 17.0);
        e.call("designos_undo", json!({})).unwrap();
        assert_eq!(e.doc.to_json().unwrap(), before);
        e.call("designos_redo", json!({})).unwrap();
        assert_eq!(e.doc.get(NodeId(id)).unwrap().kind.any_text().unwrap().content, "Hello");
        e.call("designos_delete_node", json!({"node_id":id})).unwrap();
        assert!(e.doc.get(NodeId(id)).is_none());
        e.call("designos_undo", json!({})).unwrap();
        assert!(e.doc.get(NodeId(id)).is_some());
    }
    #[test]
    fn invalid_mutations_leave_document_and_history_unchanged() {
        let mut e = Engine::default();
        let id = create(&mut e, "rectangle", "Box");
        let before = e.doc.to_json().unwrap();
        for (name, args) in [
            ("designos_create_node", json!({"parent_id":1,"kind":"bad"})),
            ("designos_create_node", json!({"parent_id":id,"kind":"frame"})),
            ("designos_create_node", json!({"parent_id":1,"kind":"text","width":-1})),
            ("designos_update_node", json!({"node_id":id,"name":"changed","text":"invalid"})),
            ("designos_update_node", json!({"node_id":id,"x":1e100})),
            ("designos_update_node", json!({"node_id":id,"opacity":2})),
            ("designos_update_node", json!({"node_id":id,"unknown":true})),
            ("designos_delete_node", json!({"node_id":1})),
            ("designos_delete_node", json!({"node_id":0})),
        ] {
            assert!(e.call(name, args).is_err(), "{name}");
            assert_eq!(e.doc.to_json().unwrap(), before);
        }
        e.call("designos_undo", json!({})).unwrap();
        assert!(!e.history.can_undo());
        assert!(e.doc.get(NodeId(id)).is_none());
    }
    #[test]
    fn name_filter_pagination_and_code_generation() {
        let mut e = Engine::default();
        let id = create(&mut e, "frame", "Card");
        create(&mut e, "rectangle", "card background");
        create(&mut e, "ellipse", "Icon");
        let result = e.call("designos_list_nodes", json!({"parent_id":1,"query":"CARD","limit":1})).unwrap();
        assert_eq!(result["total"], 2);
        assert_eq!(result["nodes"][0]["node_id"], id);
        assert_eq!(result["next_offset"], 1);
        for language in ["css", "tailwind", "react", "swiftui", "uikit", "compose", "android-xml", "svg"] {
            let output = e.call("designos_generate_code", json!({"node_id":id,"language":language})).unwrap();
            assert!(!output["code"].as_str().unwrap().is_empty());
        }
        let uikit = e.call("designos_generate_code", json!({"node_id":id,"language":"uikit"})).unwrap();
        assert!(uikit["code"].as_str().unwrap().starts_with("let card = "), "{uikit}");
        let xml = e.call("designos_generate_code", json!({"node_id":id,"language":"android-xml"})).unwrap();
        assert!(xml["code"].as_str().unwrap().starts_with("<!-- res/layout/card.xml -->"), "{xml}");
        let err = e.call("designos_generate_code", json!({"node_id":id,"language":"vue"})).unwrap_err();
        assert!(err.message.contains("uikit") && err.message.contains("android-xml"), "{err}");
        assert!(e.call("designos_list_nodes", json!({"limit":501})).is_err());
        assert!(e.call("designos_get_node", json!({"node_id":"2"})).is_err());
    }

    #[test]
    fn spec_and_comments_workflow_round_trips() {
        let mut e = Engine::default();
        let applied = e
            .call(
                "designos_apply_spec",
                json!({
                    "parent_id": 1,
                    "spec": {
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
                                        "name": "Submit Button",
                                        "preset": "easeui-button-primary",
                                        "width_sizing": "fill",
                                        "text": "Continue"
                                    }
                                ]
                            }
                        ]
                    }
                }),
            )
            .unwrap();
        let screen_id = applied["root_node_id"].as_u64().unwrap();
        assert_eq!(applied["changed"], true);

        let btn_list = e.call("designos_list_nodes", json!({"query":"Submit Button"})).unwrap();
        let btn_id = btn_list["nodes"][0]["node_id"].as_u64().unwrap();
        assert_eq!(btn_list["nodes"][0]["width"], 360.0);

        let added = e
            .call(
                "designos_add_comment",
                json!({
                    "node_id": btn_id,
                    "author": "Jang",
                    "text": "Make this CTA use $easeui.accent.orange and full width",
                    "x": 12.0,
                    "y": 8.0
                }),
            )
            .unwrap();
        assert_eq!(added["id"], "t1");
        assert_eq!(added["target_node_id"], btn_id);
        assert_eq!(added["target_frame_id"], screen_id);

        let listed = e.call("designos_list_comments", json!({"resolved": false, "node_id": screen_id})).unwrap();
        assert_eq!(listed["total"], 1);

        let replied = e
            .call(
                "designos_reply_comment",
                json!({
                    "thread_id": "t1",
                    "author": "Agent",
                    "text": "Verified: CTA uses easeui-button-primary with Fill width (360px)."
                }),
            )
            .unwrap();
        assert_eq!(replied["thread"]["messages"].as_array().unwrap().len(), 2);

        let resolved = e.call("designos_resolve_comment", json!({"thread_id": "t1", "resolved": true})).unwrap();
        assert_eq!(resolved["resolved"], true);
        assert_eq!(resolved["changed"], true);

        let unresolved_list = e.call("designos_list_comments", json!({"resolved": false})).unwrap();
        assert_eq!(unresolved_list["total"], 0);

        let dev_updated = e
            .call(
                "designos_set_dev_status",
                json!({
                    "node_id": screen_id,
                    "status": "completed",
                    "annotation": "Implemented in EaseUI React"
                }),
            )
            .unwrap();
        assert_eq!(dev_updated["dev"]["status"], "Completed");
        assert_eq!(dev_updated["dev"]["annotations"].as_array().unwrap().len(), 2);

        let exported = e.call("designos_export_spec", json!({"node_id": screen_id})).unwrap();
        assert_eq!(exported["spec"]["name"], "IAM · Login Screen");
        assert_eq!(exported["spec"]["dev_status"], "Completed");
        assert_eq!(exported["spec"]["children"][0]["children"][1]["comments"][0]["id"], "t1");
        assert_eq!(exported["spec"]["children"][0]["children"][1]["comments"][0]["resolved"], true);

        // Undo set_dev_status, resolve_comment, reply_comment, add_comment, and apply_spec
        for _ in 0..5 {
            e.call("designos_undo", json!({})).unwrap();
        }
        assert!(e.doc.get(NodeId(screen_id)).is_none());
        assert!(e.doc.comments.is_empty());
    }

    /// A vertical frame of 4,097 centred rows on the first page, each holding one 20 px rectangle
    /// at x = 40. Widening the frame to 500 moves every rectangle to x = 240, but one relayout
    /// spends its 4,096 re-solves on the first rows and queues the last one for the lazy pass.
    fn rows_frame(e: &mut Engine) -> (NodeId, NodeId) {
        use designos_doc::{AutoLayout, LayoutAlign, LayoutChild, LayoutDirection, Sizing};
        let page = e.resolve_default_page().unwrap();
        let frame = e.doc.alloc_id();
        let mut f = Node::frame(frame, Rect::new(0.0, 0.0, 100.0, 2000.0));
        if let NodeKind::Frame(fp) = &mut f.kind {
            fp.auto_layout = Some(AutoLayout {
                direction: LayoutDirection::Vertical,
                primary_sizing: Sizing::Fixed,
                counter_sizing: Sizing::Fixed,
                ..AutoLayout::default()
            });
        }
        e.doc.insert(page, f).unwrap();
        let mut last = NodeId(0);
        for _ in 0..4_097 {
            let row = e.doc.alloc_id();
            let mut r = Node::frame(row, Rect::new(0.0, 0.0, 100.0, 20.0));
            if let NodeKind::Frame(fp) = &mut r.kind {
                fp.auto_layout = Some(AutoLayout {
                    direction: LayoutDirection::Horizontal,
                    primary_sizing: Sizing::Fixed,
                    counter_sizing: Sizing::Fixed,
                    primary_align: LayoutAlign::Center,
                    ..AutoLayout::default()
                });
            }
            r.layout_child = LayoutChild { horizontal: Sizing::Fill, vertical: Sizing::Fixed, ..LayoutChild::default() };
            e.doc.insert(frame, r).unwrap();
            last = e.doc.alloc_id();
            e.doc.insert(row, Node::rectangle(last, Rect::new(40.0, 0.0, 20.0, 20.0))).unwrap();
        }
        (frame, last)
    }

    fn assert_settled(e: &Engine, last: NodeId, what: &str) {
        assert_eq!(e.doc.relayout_skipped_count(), 0, "{what}: the lazy layout queue must be empty when the call returns");
        assert_eq!(e.doc.get(last).unwrap().transform.tx, 240.0, "{what}: the last row is centred in the widened frame");
    }

    /// Widens the frame behind the engine's back and leaves the last row queued, as an earlier
    /// relayout would have.
    fn queue_the_last_row(e: &mut Engine, frame: NodeId, last: NodeId) {
        e.doc.get_mut(frame).unwrap().size.x = 500.0;
        e.doc.relayout_subtree(frame);
        assert_eq!(e.doc.relayout_skipped_count(), 1, "precondition: the last row is queued");
        assert_eq!(e.doc.get(last).unwrap().transform.tx, 40.0, "precondition: its rectangle is not centred yet");
    }

    #[test]
    fn update_node_finishes_the_layout_it_started() {
        let mut e = Engine::default();
        let (frame, last) = rows_frame(&mut e);
        e.call("designos_update_node", json!({"node_id":frame.0,"width":500})).unwrap();
        assert_settled(&e, last, "update_node");
        let node = e.call("designos_get_node", json!({"node_id":last.0})).unwrap();
        assert_eq!(node["node"]["transform"]["tx"], 240.0, "a reader sees the finished position");
    }

    #[test]
    fn create_node_finishes_the_layout_it_started() {
        let mut e = Engine::default();
        let (frame, last) = rows_frame(&mut e);
        e.doc.get_mut(frame).unwrap().size.x = 500.0;
        e.call("designos_create_node", json!({"parent_id":frame.0,"kind":"rectangle","name":"Extra"})).unwrap();
        assert_settled(&e, last, "create_node");
    }

    #[test]
    fn apply_spec_commits_variables_with_the_nodes_and_reports_them() {
        let mut e = Engine::default();
        let page = e.resolve_default_page().unwrap();
        let applied = e
            .call(
                "designos_apply_spec",
                json!({"parent_id": page.0, "spec": {"name": "Panel", "kind": "frame", "fill": "$easeui.bg.panel", "stroke": "$easeui.border.subtle"}}),
            )
            .unwrap();
        let root = applied["root_node_id"].as_u64().unwrap();

        // The collection the bindings point at is part of the document, not only of a discarded copy.
        let collection = e.doc.variable_collections.get("easeui-deprecated").expect("EaseUI (deprecated) collection");
        assert_eq!(collection.name, "EaseUI (deprecated)");
        assert_eq!(e.doc.get(NodeId(root)).unwrap().variables.bindings.get("fills/0").map(String::as_str), Some("easeui-deprecated/bg/panel"));
        assert!(e.doc.variables.contains_key("easeui-deprecated/bg/panel"));

        // The reply carries the report.
        assert_eq!(applied["bindings_created"], 2, "{applied}");
        assert_eq!(applied["styles_linked"], 0);
        let warnings = applied["warnings"].as_array().unwrap();
        let deprecated: Vec<_> = warnings.iter().filter(|w| w["code"] == "DEPRECATED_EASEUI_TOKEN").collect();
        assert_eq!(deprecated.len(), 2, "{warnings:?}");
        assert_eq!(deprecated[0]["token"], "$easeui.bg.panel");
        assert_eq!(deprecated[0]["replacement"], "var:EaseUI (deprecated)/bg/panel");
        assert_eq!(deprecated[0]["path"], "spec.fill");

        // export_spec keeps the binding, so export then apply keeps the colour linked.
        let exported = e.call("designos_export_spec", json!({"node_id": root})).unwrap();
        assert_eq!(exported["spec"]["fill"], "var:EaseUI (deprecated)/bg/panel", "{exported}");
        let again = e.call("designos_apply_spec", json!({"parent_id": page.0, "spec": exported["spec"]})).unwrap();
        let copy = again["root_node_id"].as_u64().unwrap();
        assert_eq!(e.doc.get(NodeId(copy)).unwrap().variables.bindings.get("fills/0"), e.doc.get(NodeId(root)).unwrap().variables.bindings.get("fills/0"));
        assert_eq!(again["warnings"].as_array().unwrap().iter().filter(|w| w["code"] == "DEPRECATED_EASEUI_TOKEN").count(), 0);

        // The second apply added no data, so one undo removes only its nodes; one more undo removes the
        // first apply's nodes and the collection together, and redo restores both.
        e.call("designos_undo", json!({})).unwrap();
        assert!(e.doc.get(NodeId(copy)).is_none());
        assert!(e.doc.variable_collections.contains_key("easeui-deprecated"));
        e.call("designos_undo", json!({})).unwrap();
        assert!(e.doc.get(NodeId(root)).is_none());
        assert!(!e.doc.variable_collections.contains_key("easeui-deprecated"));
        assert!(e.doc.variables.is_empty());
        e.call("designos_redo", json!({})).unwrap();
        assert!(e.doc.get(NodeId(root)).is_some());
        assert!(e.doc.variable_collections.contains_key("easeui-deprecated"));
        assert!(e.doc.variables.contains_key("easeui-deprecated/bg/panel"));
    }

    #[test]
    fn a_failed_apply_spec_leaves_variables_and_undo_depth_untouched() {
        let mut e = Engine::default();
        let page = e.resolve_default_page().unwrap();
        let depth = e.history.undo_depth();
        let err = e
            .call(
                "designos_apply_spec",
                json!({"parent_id": page.0, "spec": {"kind": "frame", "children": [
                    {"kind": "rectangle", "fill": "$easeui.bg.panel"},
                    {"kind": "rectangle", "fill": "var:Color/Nope"}
                ]}}),
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::UnknownVariable, "{err}");
        assert!(e.doc.variable_collections.is_empty());
        assert!(e.doc.variables.is_empty());
        assert_eq!(e.history.undo_depth(), depth);
    }

    #[test]
    fn apply_spec_finishes_the_layout_it_started() {
        let mut e = Engine::default();
        let (frame, last) = rows_frame(&mut e);
        e.doc.get_mut(frame).unwrap().size.x = 500.0;
        e.call("designos_apply_spec", json!({"parent_id":frame.0,"spec":{"kind":"rectangle","name":"Extra","width":10,"height":10}})).unwrap();
        assert_settled(&e, last, "apply_spec");
    }

    #[test]
    fn readers_that_serialize_or_generate_finish_a_queue_they_find() {
        let dir = std::env::temp_dir().join(format!("designos_engine_settle_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for tool in ["designos_generate_code", "designos_export_fig_file", "designos_export_spec", "designos_lint"] {
            let mut e = Engine::default();
            let (frame, last) = rows_frame(&mut e);
            queue_the_last_row(&mut e, frame, last);
            let out = dir.join("out.fig");
            let args = match tool {
                "designos_generate_code" => json!({"node_id":frame.0,"language":"css"}),
                "designos_export_fig_file" => json!({"output_path":out.to_str().unwrap()}),
                "designos_lint" => json!({"node_ids":[frame.0],"limit":10}),
                _ => json!({"node_id":frame.0}),
            };
            e.call(tool, args).unwrap();
            assert_settled(&e, last, tool);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_lint_returns_full_rule_set() {
        use designos_doc::{
            AutoLayout, Color, FrameProps, LayoutChild, LayoutDirection, Paint, Sizing, TextAutoResize, TextStyle, Variable, VariableCollection, VariableKind,
            VariableMode, VariableValue,
        };
        let mut e = Engine::default();
        let page = e.resolve_default_page().unwrap();

        e.doc.variable_collections.insert(
            "c1".into(),
            VariableCollection {
                id: "c1".into(),
                name: "Tokens".into(),
                modes: vec![VariableMode { id: "default".into(), name: "Default".into() }],
                default_mode: "default".into(),
                variables: vec!["v1".into()],
                key: None,
                source_library_key: None,
            },
        );
        e.doc.variables.insert(
            "v1".into(),
            Variable {
                id: "v1".into(),
                name: "Color/Primary".into(),
                collection: "c1".into(),
                kind: VariableKind::Color,
                values: [("default".to_string(), VariableValue::Color(Color::rgb(12, 34, 56)))].into_iter().collect(),
                description: String::new(),
                scopes: Vec::new(),
                hidden_from_publishing: false,
                key: None,
                code_syntax: Default::default(),
            },
        );

        // 1. overflow-parent: non-clipping frame with overflowing child
        let ov_parent = e.doc.alloc_id();
        let mut ov_f = Node::frame(ov_parent, Rect::new(0.0, 0.0, 100.0, 100.0));
        ov_f.name = "OverflowParent".into();
        ov_f.kind = NodeKind::Frame(FrameProps { clips_content: false, ..Default::default() });
        e.doc.insert(page, ov_f).unwrap();
        let ov_child = e.doc.alloc_id();
        let mut ov_c = Node::rectangle(ov_child, Rect::new(60.0, 0.0, 80.0, 40.0));
        ov_c.name = "OverflowChild".into();
        ov_c.fills = vec![Paint::solid(Color::rgb(12, 34, 56))]; // also hardcoded-color + hardcoded-value
        e.doc.insert(ov_parent, ov_c).unwrap();

        // 2 & 3. clipped-partial and clipped-full inside clipping frame
        let clip_parent = e.doc.alloc_id();
        let mut clip_f = Node::frame(clip_parent, Rect::new(200.0, 0.0, 100.0, 100.0));
        clip_f.name = "ClipParent".into();
        clip_f.kind = NodeKind::Frame(FrameProps { clips_content: true, ..Default::default() });
        e.doc.insert(page, clip_f).unwrap();
        let cp_id = e.doc.alloc_id();
        let mut cp_n = Node::rectangle(cp_id, Rect::new(80.0, 10.0, 40.0, 40.0));
        cp_n.name = "PartialClip".into();
        e.doc.insert(clip_parent, cp_n).unwrap();
        let cf_id = e.doc.alloc_id();
        let mut cf_n = Node::rectangle(cf_id, Rect::new(140.0, 10.0, 20.0, 20.0));
        cf_n.name = "FullClip".into();
        e.doc.insert(clip_parent, cf_n).unwrap();

        // 4, 8, 19, 26, 27. Plain frame with sibling-overlap, missing-auto-layout, misaligned-siblings, sizing-conflict, default-layer-name
        let plain_f = e.doc.alloc_id();
        let mut pf = Node::frame(plain_f, Rect::new(400.0, 0.0, 200.0, 200.0));
        pf.name = "Frame 1".into(); // default-layer-name
        e.doc.insert(page, pf).unwrap();
        let s1 = e.doc.alloc_id();
        let mut s1_n = Node::rectangle(s1, Rect::new(10.0, 10.0, 60.0, 40.0));
        s1_n.name = "SibA".into();
        s1_n.fills = vec![Paint::solid(Color::BLACK)];
        s1_n.layout_child = LayoutChild { horizontal: Sizing::Fill, ..Default::default() }; // sizing-conflict
        e.doc.insert(plain_f, s1_n).unwrap();
        let s2 = e.doc.alloc_id();
        let mut s2_n = Node::rectangle(s2, Rect::new(40.0, 12.0, 60.0, 40.0)); // overlaps s1 + 2px y misalignment
        s2_n.name = "SibB".into();
        s2_n.fills = vec![Paint::solid(Color::BLACK)];
        e.doc.insert(plain_f, s2_n).unwrap();

        // 5, 6, 9, 11, 15, 20, 22, 17. Text rules + low-contrast-text + detached-text-style
        let bg_f = e.doc.alloc_id();
        let mut bg = Node::frame(bg_f, Rect::new(0.0, 240.0, 400.0, 200.0));
        bg.name = "TextHost".into();
        bg.fills = vec![Paint::solid(Color::WHITE)];
        e.doc.insert(page, bg).unwrap();

        // text-truncated + non-scale-font-size (15px) + detached-text-style + low-contrast-text
        let t_trunc = e.doc.alloc_id();
        let mut tt = Node::text(
            t_trunc,
            Rect::new(8.0, 8.0, 80.0, 20.0),
            "Enterprise annual plan subscription",
            TextStyle { font_size: 15.0, fills: vec![Paint::solid(Color::rgb(210, 210, 210))], ..Default::default() },
        );
        tt.name = "PlanTitle".into();
        if let NodeKind::Text(tp) = &mut tt.kind {
            tp.auto_resize = TextAutoResize::None;
        }
        e.doc.insert(bg_f, tt).unwrap();

        // text-unexpected-wrap
        let t_wrap = e.doc.alloc_id();
        let mut tw = Node::text(t_wrap, Rect::new(8.0, 40.0, 60.0, 60.0), "Subscribe now", TextStyle { font_size: 16.0, ..Default::default() });
        tw.name = "CTA".into();
        if let NodeKind::Text(tp) = &mut tw.kind {
            tp.auto_resize = TextAutoResize::Height;
        }
        e.doc.insert(bg_f, tw).unwrap();

        // text-missing-fill + placeholder-text + tiny-text
        let t_ghost = e.doc.alloc_id();
        let mut tg =
            Node::text(t_ghost, Rect::new(8.0, 110.0, 120.0, 20.0), "Lorem ipsum dolor", TextStyle { font_size: 9.0, fills: vec![], ..Default::default() });
        tg.name = "GhostLorem".into();
        tg.fills.clear();
        e.doc.insert(bg_f, tg).unwrap();

        // 7 & 16. collapsed-size + non-4px-grid + inconsistent-radius
        let col_id = e.doc.alloc_id();
        let mut col_n = Node::rectangle(col_id, Rect::new(0.0, 480.0, 0.0, 19.0));
        col_n.name = "CollapsedRect".into();
        col_n.corner_radii = [7.0, 7.0, 7.0, 7.0]; // off-scale radius
        e.doc.insert(page, col_n).unwrap();

        // 10, 14, 18. Auto-layout with >50% absolute children, inconsistent-spacing (11px), unbalanced-padding
        let al_id = e.doc.alloc_id();
        let mut al_n = Node::frame(al_id, Rect::new(100.0, 480.0, 200.0, 120.0));
        al_n.name = "AutoFrame".into();
        al_n.kind = NodeKind::Frame(FrameProps {
            auto_layout: Some(AutoLayout {
                direction: LayoutDirection::Horizontal,
                item_spacing: 11.0,
                padding: [8.0, 24.0, 8.0, 8.0],
                primary_sizing: Sizing::Fixed,
                counter_sizing: Sizing::Fixed,
                ..Default::default()
            }),
            ..Default::default()
        });
        e.doc.insert(page, al_n).unwrap();
        for i in 0..2 {
            let ac = e.doc.alloc_id();
            let mut ac_n = Node::rectangle(ac, Rect::new(0.0, 0.0, 20.0, 20.0));
            ac_n.name = format!("AbsChild{i}");
            ac_n.layout_child = LayoutChild { absolute: true, ..Default::default() };
            e.doc.insert(al_id, ac_n).unwrap();
        }

        // 12. detached-instance
        let det_id = e.doc.alloc_id();
        let mut det_n = Node::frame(det_id, Rect::new(340.0, 480.0, 80.0, 40.0));
        det_n.name = "DetachedCard".into();
        det_n.detached_symbol_id = Some("guid:99:1".into());
        e.doc.insert(page, det_n).unwrap();

        // 14 & 24. empty-container + deeply-nested (depth > 6)
        let mut parent_id = page;
        for d in 1..=7 {
            let nid = e.doc.alloc_id();
            let mut nf = Node::frame(nid, Rect::new(0.0, 0.0, 80.0, 80.0));
            nf.name = format!("Nest{d}");
            if d == 7 {
                nf.fills.clear();
            }
            e.doc.insert(parent_id, nf).unwrap();
            parent_id = nid;
        }

        // 21. small-touch-target (button < 44x44)
        let btn_id = e.doc.alloc_id();
        let mut btn = Node::frame(btn_id, Rect::new(500.0, 480.0, 32.0, 28.0));
        btn.name = "Save Button".into();
        btn.fills = vec![Paint::solid(Color::BLACK)];
        e.doc.insert(page, btn).unwrap();

        let res = e.call("designos_lint", json!({"preset": "strict", "limit": 500})).unwrap();
        assert!(res.get("passed").is_none(), "old two-rule `passed` field must be gone");
        let by_rule = res["by_rule"].as_object().expect("by_rule map");
        for rule_id in designos_doc::lint::all_rule_ids() {
            assert!(
                by_rule.get(rule_id).and_then(Value::as_u64).unwrap_or(0) >= 1,
                "strict preset must report finding for rule '{rule_id}', got by_rule={by_rule:?}"
            );
        }
    }

    #[test]
    fn same_findings_three_callers() {
        use designos_doc::lint::{lint, LintConfig, LintPreset};
        let mut e = Engine::default();
        let page = e.resolve_default_page().unwrap();

        let comp_id = e.doc.alloc_id();
        let mut comp = Node::frame(comp_id, Rect::new(0.0, 0.0, 120.0, 40.0));
        comp.name = "PrimaryButton".into();
        comp.source_id = Some("10:1".into());
        comp.kind = NodeKind::Component(Default::default());
        e.doc.insert(page, comp).unwrap();

        let det_id = e.doc.alloc_id();
        let mut det = Node::frame(det_id, Rect::new(0.0, 80.0, 120.0, 40.0));
        det.name = "DetachedCopy".into();
        det.detached_symbol_id = Some("guid:10:1".into());
        e.doc.insert(page, det).unwrap();

        // Caller 1: MCP designos_lint with preset "check-designs"
        let mcp_res = e.call("designos_lint", json!({"preset": "check-designs"})).unwrap();
        let mcp_findings = mcp_res["findings"].as_array().unwrap();

        // Caller 2: Direct library call with RenderLintEnv and CheckDesigns preset
        let lib_cfg = LintConfig::from_preset(LintPreset::CheckDesigns);
        let lib_report = lint(&e.doc, &lib_cfg, &designos_render::RenderLintEnv);
        let lib_val = lib_report.to_value();
        let lib_findings = lib_val["findings"].as_array().unwrap();

        // Caller 3: Check designs shared detached rule engine
        let comps = designos_doc::lint::rules::detached::Components::new(&e.doc);
        let det_node = e.doc.get(det_id).unwrap();
        let cd_plan = designos_doc::lint::rules::detached::plan(&e.doc, &comps, det_node);
        let cd_equal = match cd_plan {
            designos_doc::lint::rules::detached::DetachedPlan::Compare { component } => {
                assert_eq!(component, comp_id);
                designos_doc::lint::rules::detached::matches_component(&e.doc, det_id, component)
            }
            other => panic!("expected Compare plan, got {other:?}"),
        };
        let (cd_msg, cd_reconnect, _) = designos_doc::lint::rules::detached::compared_outcome(&e.doc, comp_id, cd_equal);
        // Touch comps again so clippy knows it's used.
        assert_eq!(comps.resolve("guid:10:1"), Some(comp_id));

        assert_eq!(mcp_findings, lib_findings, "MCP and direct library call must match identically");
        assert_eq!(mcp_findings.len(), 1);
        assert_eq!(mcp_findings[0]["rule"], "detached-instance");
        assert_eq!(mcp_findings[0]["node_id"], det_id.0);
        assert_eq!(mcp_findings[0]["message"], cd_msg);
        assert_eq!(mcp_findings[0]["suggested_fix"]["to"].as_u64(), cd_reconnect.map(|id| id.0));
    }

    #[test]
    fn mcp_lint_error_contracts_and_history_fix() {
        let mut e = Engine::default();
        let id = create(&mut e, "frame", "Frame 1");

        let err_scope = e.call("designos_lint", json!({"page_id": 1, "node_ids": [id]})).unwrap_err();
        assert!(err_scope.contains("EXACTLY_ONE_SCOPE") && err_scope.contains("safe_to_retry"), "{err_scope}");

        let err_preset = e.call("designos_lint", json!({"preset": "nonexistent-preset"})).unwrap_err();
        assert!(err_preset.contains("UNKNOWN_PRESET") && err_preset.contains("valid_options"), "{err_preset}");

        let err_rule = e.call("designos_lint", json!({"rules": ["nonexistent-rule"]})).unwrap_err();
        assert!(err_rule.contains("UNKNOWN_RULE") && err_rule.contains("valid_options"), "{err_rule}");

        let err_node = e.call("designos_lint", json!({"node_ids": [999_999]})).unwrap_err();
        assert!(err_node.contains("NODE_NOT_FOUND"), "{err_node}");

        // Fix routes through History so a single undo reverts the batch.
        let fixed = e.call("designos_lint", json!({"rules": ["default-layer-name"], "fix": true})).unwrap();
        assert_eq!(fixed["fixed_count"], 1);
        assert_ne!(e.doc.get(NodeId(id)).unwrap().name, "Frame 1");
        e.call("designos_undo", json!({})).unwrap();
        assert_eq!(e.doc.get(NodeId(id)).unwrap().name, "Frame 1");
    }

    #[test]
    fn apply_spec_instance_round_trips_designos_and_fig_and_exports_spec() {
        let mut e = Engine { doc: designos_doc::fixtures::button_library(), ..Engine::default() };
        let page = e.doc.pages[0];
        e.active_page_id = Some(page);

        let res = e
            .call(
                "designos_apply_spec",
                json!({
                    "parent_id": page.0,
                    "spec": {
                        "kind": "instance",
                        "name": "Hero CTA",
                        "component": "Button",
                        "variant": { "Size": "lg", "State": "hover" },
                        "overrides": [
                            { "path": "Label", "text": "Launch" },
                            { "path": "Icon", "swap": "Icon/Arrow" }
                        ]
                    }
                }),
            )
            .unwrap();

        let root_id = NodeId(res["root_node_id"].as_u64().unwrap());
        assert_eq!(res["instances"].as_array().unwrap().len(), 1);
        assert_eq!(res["name_to_id"]["Hero CTA"], root_id.0);

        // .designos JSON round-trip
        let json_str = e.doc.to_json().unwrap();
        let from_json = Document::from_json(&json_str).unwrap();
        let inst_json = from_json.get(root_id).unwrap();
        let comp_json = designos_doc::instance_component(inst_json).unwrap();
        assert_eq!(from_json.get(comp_json).unwrap().name, "Size=lg, State=hover");

        // .fig binary round-trip
        let fig_bytes = designos_fig::export_fig(&e.doc).unwrap();
        let from_fig = designos_fig::import_fig(&fig_bytes).unwrap();
        let fig_inst = from_fig
            .nodes
            .values()
            .find(|n| matches!(n.kind, NodeKind::Instance { .. }) && n.name == "Hero CTA")
            .expect("Hero CTA instance must survive .fig round-trip");
        let fig_comp = designos_doc::instance_component(fig_inst).expect("Hero CTA must remain linked to its variant component after .fig import");
        assert_eq!(from_fig.get(fig_comp).unwrap().name, "Size=lg, State=hover");
        let fig_label_id = designos_doc::resolve_override_path(&from_fig, fig_inst.id, "Label").unwrap();
        assert_eq!(from_fig.get(fig_label_id).unwrap().kind.any_text().unwrap().content, "Launch");

        // designos_export_spec -> designos_apply_spec round-trip
        let exported = e.call("designos_export_spec", json!({ "node_id": root_id.0 })).unwrap();
        assert_eq!(exported["spec"]["kind"], "instance");
        let reapplied = e
            .call(
                "designos_apply_spec",
                json!({
                    "parent_id": page.0,
                    "spec": exported["spec"]
                }),
            )
            .unwrap();
        let root2 = NodeId(reapplied["root_node_id"].as_u64().unwrap());
        assert_eq!(designos_doc::instance_component(e.doc.get(root_id).unwrap()), designos_doc::instance_component(e.doc.get(root2).unwrap()));

        // Undo removes the second instance in one step
        e.call("designos_undo", json!({})).unwrap();
        assert!(e.doc.get(root2).is_none());
        assert!(e.doc.get(root_id).is_some());
    }

    #[test]
    fn apply_spec_slots_issues_and_fig_round_trip() {
        let mut e = Engine { doc: designos_doc::fixtures::card_with_slots(), ..Engine::default() };
        let page = e.doc.pages[0];
        e.active_page_id = Some(page);

        // Passing `"footer": true` fails with structured INVALID_SLOT_VALUE and does not mutate doc/history
        let err_str = e
            .call(
                "designos_apply_spec",
                json!({
                    "parent_id": page.0,
                    "spec": {
                        "kind": "instance",
                        "component": "Card",
                        "slots": { "footer": true }
                    }
                }),
            )
            .unwrap_err();
        assert_eq!(err_str.code, ErrorCode::InvalidSlotValue);
        assert!(!e.history.can_undo());

        // Filling `body` with a recommended FormRow + a non-recommended raw frame succeeds and returns `issues`
        let res = e
            .call(
                "designos_apply_spec",
                json!({
                    "parent_id": page.0,
                    "spec": {
                        "kind": "instance",
                        "name": "SettingsCard",
                        "component": "Card",
                        "slots": {
                            "header": [
                                { "kind": "instance", "name": "Hdr", "component": "CardHeader" }
                            ],
                            "body": [
                                { "kind": "instance", "name": "RowA", "component": "FormRow" },
                                { "kind": "frame", "name": "CustomBanner", "height": 32 }
                            ],
                            "footer": false
                        }
                    }
                }),
            )
            .unwrap();

        let root_id = NodeId(res["root_node_id"].as_u64().unwrap());
        let issues = res["issues"].as_array().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0]["rule"], "slot-non-recommended");
        assert_eq!(issues[0]["severity"], "warning");
        assert_eq!(issues[0]["node_name"], "CustomBanner");

        // .fig binary round-trip preserves filled slot children
        let fig_bytes = designos_fig::export_fig(&e.doc).unwrap();
        let from_fig = designos_fig::import_fig(&fig_bytes).unwrap();
        let fig_card = from_fig
            .nodes
            .values()
            .find(|n| matches!(n.kind, NodeKind::Instance { .. }) && n.name == "SettingsCard")
            .expect("SettingsCard must survive .fig round-trip");
        let fig_body = designos_doc::resolve_override_path(&from_fig, fig_card.id, "body").unwrap();
        assert_eq!(from_fig.get(fig_body).unwrap().children.len(), 2);

        // Undo removes the placed Card and all its slot children in 1 step
        e.call("designos_undo", json!({})).unwrap();
        assert!(e.doc.get(root_id).is_none());
    }
}
