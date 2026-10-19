use super::*;
use crate::{Command, Comment, CommentAnchor, CommentThread, Document, History, Node, NodeId, Rect, Style, Vec2};

/// A page with `frames` top-level frames of `kids` rectangles each, one style, one comment.
fn full_doc(frames: usize, kids: usize) -> (Document, NodeId, Vec<NodeId>) {
    let mut d = Document::new("full");
    let page = d.add_page("Page");
    let mut tops = Vec::new();
    for f in 0..frames {
        let fid = d.alloc_id();
        d.insert(page, Node::frame(fid, Rect::new(f as f32 * 500.0, 0.0, 400.0, 300.0))).unwrap();
        for k in 0..kids {
            let kid = d.alloc_id();
            d.insert(fid, Node::rectangle(kid, Rect::new(k as f32 * 10.0, 0.0, 8.0, 8.0))).unwrap();
        }
        tops.push(fid);
    }
    d.styles.insert("S:1".into(), Style::Fill(Vec::new()));
    d.comments.push(thread("t-1", page, "keep"));
    (d, page, tops)
}

fn thread(id: &str, page: NodeId, text: &str) -> CommentThread {
    CommentThread {
        id: id.into(),
        anchor: CommentAnchor { page, node: None, offset: Vec2::new(1.0, 1.0) },
        messages: vec![Comment {
            id: format!("{id}-m"),
            author: "a".into(),
            author_id: None,
            author_color: None,
            text: text.into(),
            created: "2026-10-16T00:00:00Z".into(),
            edited: None,
        }],
        resolved: false,
    }
}

/// What the editor shares for large files: the page and its first `cap` frames, without
/// children, styles or comments.
fn scoped(full: &Document, page: NodeId, cap: usize) -> Document {
    let mut s = Document::new("scoped");
    let mut p = full.get(page).unwrap().clone();
    p.children.truncate(cap);
    for c in &p.children {
        let mut n = full.get(*c).unwrap().clone();
        n.children.clear();
        s.nodes.insert(n.id, n);
    }
    s.nodes.insert(page, p);
    s.pages.push(page);
    s.reserve_ids(NodeId(full.nodes.keys().next_back().unwrap().0));
    s
}

/// Runs `cmd` on the agent copy the way the MCP engine does, collecting the ops.
fn agent_exec(agent: &mut Document, hist: &mut History, ops: &mut Vec<LiveOp>, cmd: Command) {
    ops.extend(live_ops_for_command(agent, &cmd).unwrap());
    hist.exec(agent, cmd).unwrap();
}

fn replay(doc: &mut Document, ops: Vec<LiveOp>, state: &mut LivePatchState) -> LivePatchReport {
    replay_live_patch(doc, &LivePatch::new(ops), state, &mut |d, c| c.apply(d).map(|_| ())).unwrap()
}

#[test]
fn agent_edits_on_a_scoped_copy_replay_without_losing_the_rest() {
    let (mut full, page, tops) = full_doc(6, 5);
    let mut agent = scoped(&full, page, 4);
    let mut hist = History::default();
    let mut ops = Vec::new();

    // Insert a card on top of the page (the agent sees only 4 of 6 frames).
    let card = agent.alloc_id();
    let label = agent.alloc_id();
    let mut card_node = Node::frame(card, Rect::new(0.0, 900.0, 200.0, 100.0));
    card_node.children = vec![label];
    let mut label_node = Node::rectangle(label, Rect::new(0.0, 0.0, 10.0, 10.0));
    label_node.parent = Some(card);
    let index = agent.get(page).unwrap().children.len();
    agent_exec(&mut agent, &mut hist, &mut ops, Command::Insert { parent: page, index, nodes: vec![card_node, label_node] });

    // Rename a frame the agent holds without children.
    let mut renamed = agent.get(tops[1]).unwrap().clone();
    renamed.name = "Renamed".into();
    agent_exec(&mut agent, &mut hist, &mut ops, Command::Update { node: Box::new(renamed) });

    // A layout pass on the partial copy shrinks a frame; it is not a command and must not leak.
    agent.get_mut(tops[2]).unwrap().size = Vec2::new(0.0, 0.0);

    // Add a comment: the agent's copy has none, so its SetComments carries only the new thread.
    agent_exec(&mut agent, &mut hist, &mut ops, Command::SetComments { comments: vec![thread("t-2", page, "new")] });

    let report = replay(&mut full, ops, &mut LivePatchState::default());
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(report.inserted_roots, vec![card]);
    for t in &tops {
        assert_eq!(full.get(*t).unwrap().children.len(), 5, "frame {t:?} lost its children");
    }
    assert_eq!(full.get(tops[2]).unwrap().size, Vec2::new(400.0, 300.0));
    assert_eq!(full.get(tops[1]).unwrap().name, "Renamed");
    assert_eq!(full.get(page).unwrap().children.last(), Some(&card), "the card goes on top, past frames the agent never saw");
    assert_eq!(full.get(card).unwrap().children, vec![label]);
    assert_eq!(full.styles.len(), 1);
    assert_eq!(full.comments.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t-1", "t-2"]);
}

#[test]
fn a_repeated_patch_changes_nothing() {
    let (mut full, page, tops) = full_doc(2, 1);
    let mut agent = full.clone();
    let mut ops = Vec::new();
    let id = agent.alloc_id();
    agent_exec(
        &mut agent,
        &mut History::default(),
        &mut ops,
        Command::Insert { parent: tops[0], index: 0, nodes: vec![Node::rectangle(id, Rect::new(0.0, 0.0, 4.0, 4.0))] },
    );
    let mut state = LivePatchState::default();
    replay(&mut full, ops.clone(), &mut state);
    let again = replay(&mut full, ops, &mut state);
    assert_eq!(again.applied, 0);
    assert_eq!(again.duplicates, 1);
    assert_eq!(full.get(tops[0]).unwrap().children.len(), 2);
    assert_eq!(full.get(page).unwrap().children.len(), 2);
}

#[test]
fn ids_the_editor_reused_meanwhile_get_fresh_ones() {
    let (mut full, page, _) = full_doc(1, 0);
    let mut agent = full.clone();
    // The user draws a layer in the editor; it takes the id the agent will also allocate.
    let taken = full.alloc_id();
    full.insert(page, Node::rectangle(taken, Rect::new(0.0, 0.0, 1.0, 1.0))).unwrap();

    let mut hist = History::default();
    let mut ops = Vec::new();
    let id = agent.alloc_id();
    assert_eq!(id, taken);
    agent_exec(&mut agent, &mut hist, &mut ops, Command::Insert { parent: page, index: 1, nodes: vec![Node::frame(id, Rect::new(0.0, 0.0, 9.0, 9.0))] });
    let mut moved = agent.get(id).unwrap().clone();
    moved.name = "Agent frame".into();
    agent_exec(&mut agent, &mut hist, &mut ops, Command::Update { node: Box::new(moved) });

    let report = replay(&mut full, ops, &mut LivePatchState::default());
    let fresh = report.inserted_roots[0];
    assert_ne!(fresh, taken);
    assert_eq!(full.get(taken).unwrap().size, Vec2::new(1.0, 1.0), "the user's layer is untouched");
    assert_eq!(full.get(fresh).unwrap().name, "Agent frame", "later ops follow the new id");
}

#[test]
fn remapped_ids_stop_applying_once_the_agent_reloads_the_editor_canvas() {
    let (mut full, page, _) = full_doc(1, 0);
    let mut agent = full.clone();
    let taken = full.alloc_id();
    full.insert(page, Node::rectangle(taken, Rect::new(0.0, 0.0, 1.0, 1.0))).unwrap();
    let id = agent.alloc_id();
    assert_eq!(id, taken);
    let mut ops = Vec::new();
    agent_exec(
        &mut agent,
        &mut History::default(),
        &mut ops,
        Command::Insert { parent: page, index: 1, nodes: vec![Node::frame(id, Rect::new(0.0, 0.0, 9.0, 9.0))] },
    );
    let mut state = LivePatchState::default();
    let on = |base: u64, ops: Vec<LiveOp>| LivePatch::new(ops).with_base(Some(base));
    let exec = &mut |d: &mut Document, c: Command| c.apply(d).map(|_| ());
    let fresh = replay_live_patch(&mut full, &on(1, ops), &mut state, exec).unwrap().inserted_roots[0];
    let rename = |name: &str| vec![LiveOp::Update { id: taken, set: [("name".to_string(), serde_json::json!(name))].into_iter().collect(), unset: Vec::new() }];

    // Same canvas: the agent's id still means its own frame.
    replay_live_patch(&mut full, &on(1, rename("agent frame")), &mut state, exec).unwrap();
    assert_eq!(full.get(fresh).unwrap().name, "agent frame");
    // After a reload the agent holds the editor's ids, so the same id is the user's layer.
    replay_live_patch(&mut full, &on(2, rename("user layer")), &mut state, exec).unwrap();
    assert_eq!(full.get(taken).unwrap().name, "user layer");
    assert_eq!(full.get(fresh).unwrap().name, "agent frame");
}

#[test]
fn removes_and_moves_replay_on_the_full_subtree() {
    let (mut full, page, tops) = full_doc(3, 2);
    let mut agent = full.clone();
    let mut hist = History::default();
    let mut ops = Vec::new();
    let kid = agent.get(tops[0]).unwrap().children[0];
    agent_exec(&mut agent, &mut hist, &mut ops, Command::Reparent { id: kid, parent: tops[2], index: 0 });
    agent_exec(&mut agent, &mut hist, &mut ops, Command::Remove { id: tops[1] });
    // Undo of the remove arrives as an insert of the subtree it brought back.
    let undo = hist_peek(&hist);
    ops.extend(live_ops_for_command(&agent, &undo).unwrap());
    hist.undo(&mut agent).unwrap();

    replay(&mut full, ops, &mut LivePatchState::default());
    assert_eq!(full.get(kid).unwrap().parent, Some(tops[2]));
    assert_eq!(full.get(tops[2]).unwrap().children[0], kid);
    assert_eq!(full.get(tops[1]).unwrap().children.len(), 2);
    assert_eq!(full.get(page).unwrap().children, tops);
}

fn hist_peek(h: &History) -> Command {
    h.peek_undo().cloned().expect("undo entry")
}

fn collection(id: &str) -> crate::VariableCollection {
    crate::VariableCollection {
        id: id.into(),
        name: id.into(),
        modes: vec![crate::VariableMode { id: "m".into(), name: "Default".into() }],
        default_mode: "m".into(),
        variables: vec![format!("{id}/v")],
        key: None,
        source_library_key: None,
    }
}

fn variable(id: &str, collection: &str) -> crate::Variable {
    crate::Variable {
        id: id.into(),
        name: "v".into(),
        collection: collection.into(),
        kind: crate::VariableKind::Number,
        values: [("m".to_string(), crate::VariableValue::Number(1.0))].into(),
        description: String::new(),
        scopes: Vec::new(),
        hidden_from_publishing: false,
        key: None,
        code_syntax: Default::default(),
    }
}

#[test]
fn variables_the_agent_adds_merge_into_the_editors_and_undo_removes_only_those() {
    // The editor holds a collection of its own that the agent's scoped copy lacks.
    let (mut full, page, _) = full_doc(1, 0);
    full.variable_collections.insert("user".into(), collection("user"));
    full.variables.insert("user/v".into(), variable("user/v", "user"));
    let user_data = (full.variables.clone(), full.variable_collections.clone());
    let mut agent = scoped(&full, page, 1);
    let mut hist = History::default();
    let mut ops = Vec::new();

    let id = agent.alloc_id();
    let mut vars = agent.variables.clone();
    vars.insert("easeui/v".into(), variable("easeui/v", "easeui"));
    let mut colls = agent.variable_collections.clone();
    colls.insert("easeui".into(), collection("easeui"));
    let batch = Command::Batch(vec![
        Command::SetVariables { variables: vars, collections: colls },
        Command::Insert { parent: page, index: 0, nodes: vec![Node::rectangle(id, Rect::new(0.0, 0.0, 4.0, 4.0))] },
    ]);
    agent_exec(&mut agent, &mut hist, &mut ops, batch);
    assert!(matches!(&ops[0], LiveOp::Variables { collections, variables, removed_collections, removed_variables }
        if collections.len() == 1 && variables.len() == 1 && removed_collections.is_empty() && removed_variables.is_empty()));

    let mut state = LivePatchState::default();
    let report = replay(&mut full, ops.clone(), &mut state);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(report.applied, 2);
    assert!(full.variable_collections.contains_key("easeui") && full.variables.contains_key("easeui/v"));
    assert!(full.variable_collections.contains_key("user") && full.variables.contains_key("user/v"), "the editor's own variables survive");

    // A repeated delivery changes nothing.
    let again = replay(&mut full, ops, &mut state);
    assert_eq!(again.applied, 0);
    assert_eq!(again.duplicates, 2);

    // Undoing on the agent removes exactly what the agent added.
    let undo = hist_peek(&hist);
    let mut undo_ops = live_ops_for_command(&agent, &undo).unwrap();
    hist.undo(&mut agent).unwrap();
    assert!(matches!(undo_ops.iter().find(|o| matches!(o, LiveOp::Variables { .. })),
        Some(LiveOp::Variables { removed_collections, removed_variables, collections, variables })
            if removed_collections == &["easeui".to_string()] && removed_variables == &["easeui/v".to_string()] && collections.is_empty() && variables.is_empty()));
    let report = replay(&mut full, std::mem::take(&mut undo_ops), &mut state);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!((full.variables.clone(), full.variable_collections.clone()), user_data);
}

/// The agent applies a spec that adds `easeui`/`easeui/v` on a scoped copy and then undoes it;
/// returns the ops of both steps.
fn agent_adds_then_undoes_easeui(full: &Document, page: NodeId) -> (Vec<LiveOp>, Vec<LiveOp>) {
    let mut agent = scoped(full, page, 1);
    let mut hist = History::default();
    let mut apply_ops = Vec::new();
    let id = agent.alloc_id();
    let mut vars = agent.variables.clone();
    vars.insert("easeui/v".into(), variable("easeui/v", "easeui"));
    let mut colls = agent.variable_collections.clone();
    colls.insert("easeui".into(), collection("easeui"));
    let batch = Command::Batch(vec![
        Command::SetVariables { variables: vars, collections: colls },
        Command::Insert { parent: page, index: 0, nodes: vec![Node::rectangle(id, Rect::new(0.0, 0.0, 4.0, 4.0))] },
    ]);
    agent_exec(&mut agent, &mut hist, &mut apply_ops, batch);
    let undo_ops = live_ops_for_command(&agent, &hist_peek(&hist)).unwrap();
    (apply_ops, undo_ops)
}

#[test]
fn an_agent_undo_never_removes_variables_the_editor_already_had() {
    // The editor's file already holds the collection (an earlier preset created it) and layers use it.
    let (mut full, page, tops) = full_doc(1, 0);
    full.variable_collections.insert("easeui".into(), collection("easeui"));
    full.variables.insert("easeui/v".into(), variable("easeui/v", "easeui"));
    full.get_mut(tops[0]).unwrap().variables.bindings.insert("fills/0".into(), "easeui/v".into());
    let before = (full.variables.clone(), full.variable_collections.clone());
    let (apply_ops, undo_ops) = agent_adds_then_undoes_easeui(&full, page);

    let mut state = LivePatchState::default();
    replay(&mut full, apply_ops, &mut state);
    replay(&mut full, undo_ops, &mut state);

    assert_eq!((full.variables.clone(), full.variable_collections.clone()), before, "the editor's own collection must survive the agent's undo");
}

#[test]
fn an_agent_apply_never_overwrites_a_variable_the_user_changed() {
    let (mut full, page, _) = full_doc(1, 0);
    full.variable_collections.insert("easeui".into(), collection("easeui"));
    let mut edited = variable("easeui/v", "easeui");
    edited.values.insert("m".into(), crate::VariableValue::Number(99.0));
    full.variables.insert("easeui/v".into(), edited.clone());
    let (apply_ops, _) = agent_adds_then_undoes_easeui(&full, page);

    replay(&mut full, apply_ops, &mut LivePatchState::default());

    assert_eq!(full.variables.get("easeui/v"), Some(&edited), "the user's edited value must stay");
}

#[test]
fn a_variable_op_without_an_id_is_refused() {
    let (mut full, _, _) = full_doc(1, 0);
    let before = full.variables.clone();
    let op = LiveOp::Variables { collections: Vec::new(), variables: vec![variable("", "c")], removed_collections: Vec::new(), removed_variables: Vec::new() };
    let report = replay(&mut full, vec![op], &mut LivePatchState::default());
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(full.variables, before);
}

#[test]
fn edits_the_editor_cannot_replay_are_refused() {
    page_flow_library_commands_are_still_refused();
}

#[test]
fn page_flow_library_commands_are_still_refused() {
    let (doc, page, _) = full_doc(1, 0);
    for (cmd, expected) in [
        (Command::InsertPage { index: 0, nodes: Vec::new() }, "Adding a page"),
        (Command::RemovePage { id: page }, "Removing a page"),
        (Command::SetFlows { flows: Vec::new() }, "Editing prototype flows"),
        (Command::SetLibraryState { state: Default::default(), library_page: None }, "Editing library state"),
        (Command::SetGuides { page, guides: Vec::new() }, "Editing guides"),
        (Command::SetPrototypeDevice { device: Default::default() }, "Editing the prototype device"),
    ] {
        let err = live_ops_for_command(&doc, &cmd).unwrap_err();
        assert!(err.contains(expected), "expected '{expected}' in '{err}'");
    }
}

#[test]
fn a_bind_on_the_scoped_canvas_merges_with_unchanged_ids() {
    let (mut full, page, tops) = full_doc(2, 2);
    full.variable_collections.insert("col:user".into(), collection("col:user"));
    full.variables.insert("col:user/v".into(), variable("col:user/v", "col:user"));
    let mut agent = scoped(&full, page, 2);
    agent.variable_collections = full.variable_collections.clone();
    agent.variables = full.variables.clone();
    let mut hist = History::default();
    let mut ops = Vec::new();

    let mut frame = agent.get(tops[0]).unwrap().clone();
    frame.variables.bindings.insert("fills/0".into(), "col:user/v".into());
    agent_exec(&mut agent, &mut hist, &mut ops, Command::Update { node: Box::new(frame) });

    let report = replay(&mut full, ops, &mut LivePatchState::default());
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(full.get(tops[0]).unwrap().variables.bindings.get("fills/0").map(String::as_str), Some("col:user/v"));
    assert_eq!(full.get(tops[0]).unwrap().children.len(), 2, "frame kept its children");
}

#[test]
fn a_variable_edit_on_the_scoped_canvas_merges_without_duplicates() {
    let (mut full, page, _) = full_doc(1, 0);
    full.variable_collections.insert("col:user".into(), collection("col:user"));
    full.variables.insert("col:user/v".into(), variable("col:user/v", "col:user"));
    let mut agent = scoped(&full, page, 1);
    agent.variable_collections = full.variable_collections.clone();
    agent.variables = full.variables.clone();
    let mut hist = History::default();
    let mut ops = Vec::new();

    let mut vars = agent.variables.clone();
    vars.get_mut("col:user/v").unwrap().values.insert("m".into(), crate::VariableValue::Number(42.0));
    let colls = agent.variable_collections.clone();
    agent_exec(&mut agent, &mut hist, &mut ops, Command::SetVariables { variables: vars, collections: colls });

    let mut state = LivePatchState::default();
    let report = replay(&mut full, ops, &mut state);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(report.tokens_applied, 1);
    assert_eq!(full.variables.len(), 1);
    assert_eq!(full.variables["col:user/v"].values.get("m"), Some(&crate::VariableValue::Number(42.0)));
}

#[test]
fn a_style_edit_on_the_scoped_canvas_merges_without_duplicates() {
    let (mut full, page, _) = full_doc(1, 0);
    full.style_meta.insert("S:1".into(), crate::StyleMeta { name: "Brand/Primary".into(), ..Default::default() });
    let mut agent = scoped(&full, page, 1);
    agent.styles = full.styles.clone();
    agent.style_meta = full.style_meta.clone();
    let mut hist = History::default();
    let mut ops = Vec::new();

    let mut styles = agent.styles.clone();
    styles.insert("S:1".into(), Style::Fill(vec![crate::Paint::solid(crate::Color::rgb(10, 20, 30))]));
    let mut meta = agent.style_meta.clone();
    meta.get_mut("S:1").unwrap().description = "Updated by agent".into();
    agent_exec(&mut agent, &mut hist, &mut ops, Command::SetStyles { styles, style_meta: meta });

    let mut state = LivePatchState::default();
    let report = replay(&mut full, ops, &mut state);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(report.tokens_applied, 1);
    assert_eq!(full.styles.len(), 1);
    assert_eq!(full.styles["S:1"], Style::Fill(vec![crate::Paint::solid(crate::Color::rgb(10, 20, 30))]));
    assert_eq!(full.style_meta["S:1"].description, "Updated by agent");
}

#[test]
fn replaying_a_token_patch_twice_changes_nothing() {
    let (mut full, page, _) = full_doc(1, 0);
    full.variable_collections.insert("col:user".into(), collection("col:user"));
    full.variables.insert("col:user/v".into(), variable("col:user/v", "col:user"));
    let mut agent = scoped(&full, page, 1);
    agent.variable_collections = full.variable_collections.clone();
    agent.variables = full.variables.clone();
    agent.styles = full.styles.clone();
    let mut hist = History::default();
    let mut ops = Vec::new();

    let mut vars = agent.variables.clone();
    vars.get_mut("col:user/v").unwrap().values.insert("m".into(), crate::VariableValue::Number(7.0));
    let mut styles = agent.styles.clone();
    styles.insert("S:2".into(), Style::Fill(Vec::new()));
    let colls = agent.variable_collections.clone();
    let meta = agent.style_meta.clone();
    agent_exec(
        &mut agent,
        &mut hist,
        &mut ops,
        Command::Batch(vec![Command::SetVariables { variables: vars, collections: colls }, Command::SetStyles { styles, style_meta: meta }]),
    );

    let mut state = LivePatchState::default();
    let first = replay(&mut full, ops.clone(), &mut state);
    assert_eq!(first.tokens_applied, 2);
    let snapshot = (full.variables.clone(), full.styles.clone());

    let second = replay(&mut full, ops, &mut state);
    assert_eq!(second.applied, 0);
    assert_eq!(second.duplicates, 2);
    assert_eq!(second.tokens_duplicates, 2);
    assert_eq!((full.variables, full.styles), snapshot);
}

#[test]
fn a_bind_to_a_deleted_variable_is_skipped_with_a_reason() {
    let (mut full, page, tops) = full_doc(1, 0);
    full.variable_collections.insert("col:user".into(), collection("col:user"));
    full.variables.insert("col:user/v".into(), variable("col:user/v", "col:user"));
    let mut agent = scoped(&full, page, 1);
    agent.variable_collections = full.variable_collections.clone();
    agent.variables = full.variables.clone();
    // User deletes the variable in the editor before the agent's bind replays.
    full.variables.remove("col:user/v");

    let mut hist = History::default();
    let mut ops = Vec::new();
    let mut frame = agent.get(tops[0]).unwrap().clone();
    frame.variables.bindings.insert("fills/0".into(), "col:user/v".into());
    agent_exec(&mut agent, &mut hist, &mut ops, Command::Update { node: Box::new(frame) });

    let report = replay(&mut full, ops, &mut LivePatchState::default());
    assert_eq!(report.applied, 0);
    assert_eq!(report.duplicates, 0);
    assert_eq!(report.skipped.len(), 1);
    assert!(report.skipped[0].contains("variable 'col:user/v' not found"), "{:?}", report.skipped);
    assert!(full.get(tops[0]).unwrap().variables.bindings.is_empty(), "no dangling binding may land");
}

#[test]
fn token_ops_are_one_undo_step_with_their_patch() {
    let (mut full, page, tops) = full_doc(1, 0);
    full.variable_collections.insert("col:user".into(), collection("col:user"));
    full.variables.insert("col:user/v".into(), variable("col:user/v", "col:user"));
    let mut agent = scoped(&full, page, 1);
    agent.variable_collections = full.variable_collections.clone();
    agent.variables = full.variables.clone();
    agent.styles = full.styles.clone();
    let mut agent_hist = History::default();
    let mut ops = Vec::new();

    let mut vars = agent.variables.clone();
    vars.get_mut("col:user/v").unwrap().values.insert("m".into(), crate::VariableValue::Number(12.0));
    let mut styles = agent.styles.clone();
    styles.insert("S:new".into(), Style::Fill(Vec::new()));
    let mut frame = agent.get(tops[0]).unwrap().clone();
    frame.name = "Tokenized Frame".into();
    frame.variables.bindings.insert("width".into(), "col:user/v".into());
    frame.style_refs.insert("fill".into(), "S:new".into());
    let colls = agent.variable_collections.clone();
    let meta = agent.style_meta.clone();
    agent_exec(
        &mut agent,
        &mut agent_hist,
        &mut ops,
        Command::Batch(vec![
            Command::SetVariables { variables: vars, collections: colls },
            Command::SetStyles { styles, style_meta: meta },
            Command::Update { node: Box::new(frame) },
        ]),
    );

    let mut state = LivePatchState::default();
    let mut cmds = Vec::new();
    let report = replay_live_patch(&mut full, &LivePatch::new(ops), &mut state, &mut |d, c| {
        cmds.push(c.clone());
        c.apply(d).map(|_| ())
    })
    .unwrap();
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(report.applied, 3);
    assert_eq!(report.tokens_applied, 2);

    // Committing the patch as a single batch creates 1 undo entry.
    let mut full_before = full_doc(1, 0).0;
    full_before.variable_collections.insert("col:user".into(), collection("col:user"));
    full_before.variables.insert("col:user/v".into(), variable("col:user/v", "col:user"));
    let mut editor_hist = History::default();
    editor_hist.exec(&mut full_before, Command::Batch(cmds)).unwrap();
    assert!(editor_hist.can_undo());
    assert!(editor_hist.undo(&mut full_before).unwrap());
    assert!(!editor_hist.can_undo());
    assert_eq!(full_before.variables["col:user/v"].values.get("m"), Some(&crate::VariableValue::Number(1.0)));
    assert!(!full_before.styles.contains_key("S:new"));
}

#[test]
fn old_reader_reports_the_new_op_as_unreadable() {
    #[allow(dead_code)]
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum OldLiveOp {
        Insert { parent: NodeId },
        Update { id: NodeId },
        Reparent { id: NodeId },
        Remove { id: NodeId },
        Comments { removed: Vec<String> },
    }
    #[allow(dead_code)]
    #[derive(serde::Deserialize)]
    struct OldLivePatch {
        ops: Vec<OldLiveOp>,
    }

    let mut styles_set = std::collections::BTreeMap::new();
    styles_set.insert("S:1".to_string(), Style::Fill(Vec::new()));
    let patch = LivePatch::new(vec![LiveOp::Styles { styles_set, styles_removed: Vec::new(), meta_set: Default::default(), meta_removed: Vec::new() }]);
    let line = serde_json::to_vec(&patch).unwrap();
    let old_res = serde_json::from_slice::<OldLivePatch>(&line);
    assert!(old_res.is_err(), "an older reader without the Styles variant must reject the line as unreadable");
}

#[test]
fn sibling_hints_survive_different_views_of_the_parent() {
    let ids: Vec<NodeId> = (1..=6).map(NodeId).collect();
    let seen = &ids[..4];
    assert_eq!(SiblingHint::at(seen, 4).index_in(&ids), 6, "on top of a truncated list stays on top");
    assert_eq!(SiblingHint::at(seen, 0).index_in(&ids), 0);
    assert_eq!(SiblingHint::at(seen, 2).index_in(&ids), 2);
    assert_eq!(SiblingHint { after: Some(NodeId(99)), before: Some(NodeId(3)) }.index_in(&ids), 2);
}

#[test]
fn agent_comments_keep_the_editor_threads_and_follow_their_new_ids() {
    let (mut full, page, _) = full_doc(1, 0);
    let mut agent = full.clone();
    let mut hist = History::default();
    // Meanwhile the user starts `t-2` in the editor, the id the agent is about to use.
    full.comments.push(thread("t-2", page, "user"));

    let mut add = Vec::new();
    let mut with_t2 = agent.comments.clone();
    with_t2.push(thread("t-2", page, "agent"));
    agent_exec(&mut agent, &mut hist, &mut add, Command::SetComments { comments: with_t2 });
    let mut state = LivePatchState::default();
    replay(&mut full, add.clone(), &mut state);
    let texts = |d: &Document, id: &str| d.comments.iter().find(|t| t.id == id).map(|t| t.messages.iter().map(|m| m.text.clone()).collect::<Vec<_>>());
    assert_eq!(texts(&full, "t-1").unwrap(), ["keep"]);
    assert_eq!(texts(&full, "t-2").unwrap(), ["user"], "the user's thread was overwritten");
    assert_eq!(texts(&full, "t-2.2").unwrap(), ["agent"]);
    assert_eq!(replay(&mut full, add, &mut state).duplicates, 1, "a repeated patch adds nothing");
    assert_eq!(full.comments.len(), 3);

    // The agent replies in its `t-2` and resolves it; both land on `t-2.2`. A reply id the user's
    // own message took meanwhile gets a fresh one.
    full.comments.iter_mut().find(|t| t.id == "t-2.2").unwrap().messages.push(Comment {
        id: "t-2-r".into(),
        author: "u".into(),
        author_id: None,
        author_color: None,
        text: "user reply".into(),
        created: "2026-10-16T00:01:00Z".into(),
        edited: None,
    });
    let mut changed = agent.comments.clone();
    let mine = changed.iter_mut().find(|t| t.id == "t-2").unwrap();
    mine.messages.push(Comment {
        id: "t-2-r".into(),
        author: "a".into(),
        author_id: None,
        author_color: None,
        text: "agent reply".into(),
        created: "2026-10-16T00:02:00Z".into(),
        edited: None,
    });
    mine.resolved = true;
    let mut ops = Vec::new();
    agent_exec(&mut agent, &mut hist, &mut ops, Command::SetComments { comments: changed });
    replay(&mut full, ops, &mut state);
    let t = full.comments.iter().find(|t| t.id == "t-2.2").unwrap();
    assert!(t.resolved);
    assert_eq!(t.messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["agent", "user reply", "agent reply"]);
    assert_eq!(t.messages.iter().map(|m| m.id.as_str()).collect::<std::collections::BTreeSet<_>>().len(), 3);
    assert!(!full.comments.iter().find(|t| t.id == "t-2").unwrap().resolved, "the user's thread stays open");
}
