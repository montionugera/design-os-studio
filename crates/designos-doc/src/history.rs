//! Undo/redo. Every edit goes through [`Command`]; applying returns its inverse.

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    /// Insert subtree nodes (root first) under `parent` at `index`.
    Insert {
        parent: NodeId,
        index: usize,
        nodes: Vec<Node>,
    },
    Remove {
        id: NodeId,
    },
    Reparent {
        id: NodeId,
        parent: NodeId,
        index: usize,
    },
    /// Replace a node's own properties (children/parent are preserved).
    Update {
        node: Box<Node>,
    },
    /// Insert a page subtree (page node first) at `index` in `Document::pages`.
    InsertPage {
        index: usize,
        nodes: Vec<Node>,
    },
    RemovePage {
        id: NodeId,
    },
    /// Replace the document's prototype flow list (flow starting points).
    SetFlows {
        flows: Vec<Flow>,
    },
    /// Replace shared styles and their metadata (style create / edit / delete).
    SetStyles {
        styles: BTreeMap<String, Style>,
        style_meta: BTreeMap<String, StyleMeta>,
    },
    /// Replace all comment threads.
    SetComments {
        comments: Vec<CommentThread>,
    },
    /// Replace library bookkeeping (publish info, enabled libraries, asset copies).
    SetLibraryState {
        state: LibraryState,
        library_page: Option<NodeId>,
    },
    /// Replace the guides of one page.
    SetGuides {
        page: NodeId,
        guides: Vec<Guide>,
    },
    /// Replace the prototype device settings.
    SetPrototypeDevice {
        device: PrototypeDevice,
    },
    /// Replace all variables and collections (variable edits).
    SetVariables {
        variables: BTreeMap<String, Variable>,
        collections: BTreeMap<String, VariableCollection>,
    },
    /// Adds embedded image bytes (a hash already present keeps its bytes) and drops the `remove` hashes.
    /// Its inverse removes what was newly added and puts the removed bytes back, so undo/redo of a paste
    /// leaves no orphaned images and loses none.
    SetImages {
        add: BTreeMap<String, Vec<u8>>,
        remove: Vec<String>,
    },
    Batch(Vec<Command>),
}

impl Command {
    /// Applies the command and returns the command that undoes it.
    pub fn apply(self, doc: &mut Document) -> Result<Command, DocError> {
        Ok(match self {
            Command::Insert { parent, index, nodes } => {
                let root = nodes.first().map(|n| n.id).ok_or(DocError::NotFound(parent))?;
                let p = doc.nodes.get_mut(&parent).ok_or(DocError::NotFound(parent))?;
                if !p.kind.can_hold_children() {
                    return Err(DocError::NotAContainer(parent));
                }
                let index = index.min(p.children.len());
                p.children.insert(index, root);
                let many = nodes.len() > 64;
                for mut n in nodes {
                    if n.id == root {
                        n.parent = Some(parent);
                    }
                    doc.nodes.insert(n.id, n);
                }
                doc.bump_ids();
                if many {
                    doc.mark_tree_full_dirty();
                } else {
                    doc.mark_node_dirty(parent);
                    doc.mark_node_dirty(root);
                }
                Command::Remove { id: root }
            }
            Command::Remove { id } => {
                let parent = doc.get(id).ok_or(DocError::NotFound(id))?.parent.ok_or(DocError::NotFound(id))?;
                let index = doc.get(parent).map(|p| p.children.iter().position(|c| *c == id).unwrap_or(0)).unwrap_or(0);
                let nodes = doc.remove(id)?;
                Command::Insert { parent, index, nodes }
            }
            Command::Reparent { id, parent, index } => {
                let n = doc.get(id).ok_or(DocError::NotFound(id))?;
                let old_parent = n.parent.ok_or(DocError::NotFound(id))?;
                let old_index = doc.get(old_parent).and_then(|p| p.children.iter().position(|c| *c == id)).unwrap_or(0);
                doc.reparent(id, parent, index)?;
                Command::Reparent { id, parent: old_parent, index: old_index }
            }
            Command::Update { mut node } => {
                let cur = doc.get_mut(node.id).ok_or(DocError::NotFound(node.id))?;
                node.parent = cur.parent;
                node.children = std::mem::take(&mut cur.children);
                let old = std::mem::replace(cur, *node);
                Command::Update { node: Box::new(Node { children: Vec::new(), ..old }) }
            }
            Command::InsertPage { index, nodes } => {
                let root = nodes.first().ok_or(DocError::NotFound(NodeId(0)))?;
                if !matches!(root.kind, NodeKind::Page { .. }) {
                    return Err(DocError::NotAContainer(root.id));
                }
                let root = root.id;
                for mut n in nodes {
                    if n.id == root {
                        n.parent = None;
                    }
                    doc.nodes.insert(n.id, n);
                }
                let index = index.min(doc.pages.len());
                doc.pages.insert(index, root);
                doc.bump_ids();
                doc.mark_tree_full_dirty();
                Command::RemovePage { id: root }
            }
            Command::RemovePage { id } => {
                let index = doc.pages.iter().position(|p| *p == id).ok_or(DocError::NotFound(id))?;
                let nodes = doc.remove(id)?;
                Command::InsertPage { index, nodes }
            }
            Command::SetFlows { flows } => Command::SetFlows { flows: std::mem::replace(&mut doc.flows, flows) },
            Command::SetStyles { styles, style_meta } => {
                doc.mark_full_dirty();
                Command::SetStyles { styles: std::mem::replace(&mut doc.styles, styles), style_meta: std::mem::replace(&mut doc.style_meta, style_meta) }
            }
            Command::SetComments { comments } => {
                doc.bump_mutation_epoch();
                Command::SetComments { comments: std::mem::replace(&mut doc.comments, comments) }
            }
            Command::SetLibraryState { state, library_page } => Command::SetLibraryState {
                state: std::mem::replace(&mut doc.library, state),
                library_page: std::mem::replace(&mut doc.library_page, library_page),
            },
            Command::SetGuides { page, guides } => {
                let old = if guides.is_empty() { doc.guides.remove(&page) } else { doc.guides.insert(page, guides) };
                Command::SetGuides { page, guides: old.unwrap_or_default() }
            }
            Command::SetPrototypeDevice { device } => Command::SetPrototypeDevice { device: std::mem::replace(&mut doc.prototype_device, device) },
            Command::SetVariables { variables, collections } => {
                doc.mark_full_dirty();
                Command::SetVariables {
                    variables: std::mem::replace(&mut doc.variables, variables),
                    collections: std::mem::replace(&mut doc.variable_collections, collections),
                }
            }
            Command::SetImages { add, remove } => {
                let removed: BTreeMap<String, Vec<u8>> = remove.into_iter().filter_map(|h| doc.images.remove(&h).map(|bytes| (h, bytes))).collect();
                let mut added = Vec::new();
                for (hash, bytes) in add {
                    if let std::collections::btree_map::Entry::Vacant(slot) = doc.images.entry(hash) {
                        added.push(slot.key().clone());
                        slot.insert(bytes);
                    }
                }
                Command::SetImages { add: removed, remove: added }
            }
            Command::Batch(cmds) => {
                let mut inv = Vec::with_capacity(cmds.len());
                for c in cmds {
                    inv.push(c.apply(doc)?);
                }
                inv.reverse();
                Command::Batch(inv)
            }
        })
    }
}

impl Document {
    pub(crate) fn bump_ids(&mut self) {
        if let Some((id, _)) = self.nodes.last_key_value() {
            self.next_id = self.next_id.max(id.0);
        }
    }
}

/// Origin of an undo step for [`History::fold_since`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StepOrigin {
    #[default]
    Unspecified,
    Agent,
    User,
}

/// Metadata attached to a single step on [`History`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UndoStepMeta {
    pub origin: StepOrigin,
    pub label: Option<String>,
}

/// Linear undo stack.
#[derive(Default)]
pub struct History {
    undo: Vec<Command>,
    redo: Vec<Command>,
    undo_meta: Vec<UndoStepMeta>,
    redo_meta: Vec<UndoStepMeta>,
}

impl History {
    pub fn exec(&mut self, doc: &mut Document, cmd: Command) -> Result<(), DocError> {
        let inv = cmd.apply(doc)?;
        self.undo.push(inv);
        self.undo_meta.push(UndoStepMeta::default());
        self.redo.clear();
        self.redo_meta.clear();
        Ok(())
    }

    /// Executes an edit originating from an agent tool call (`live_patch_apply`) so
    /// [`Self::fold_since`] can fold the contiguous agent tail on stop without folding
    /// intervening user edits.
    pub fn exec_agent(&mut self, doc: &mut Document, cmd: Command) -> Result<(), DocError> {
        let inv = cmd.apply(doc)?;
        self.undo.push(inv);
        self.undo_meta.push(UndoStepMeta { origin: StepOrigin::Agent, label: None });
        self.redo.clear();
        self.redo_meta.clear();
        Ok(())
    }

    /// Executes an edit explicitly marked as a user edit.
    pub fn exec_user(&mut self, doc: &mut Document, cmd: Command) -> Result<(), DocError> {
        let inv = cmd.apply(doc)?;
        self.undo.push(inv);
        self.undo_meta.push(UndoStepMeta { origin: StepOrigin::User, label: None });
        self.redo.clear();
        self.redo_meta.clear();
        Ok(())
    }

    /// Marks the most recently pushed undo step as an agent step.
    pub fn mark_last_undo_as_agent(&mut self) {
        self.sync_meta_lengths();
        if let Some(m) = self.undo_meta.last_mut() {
            m.origin = StepOrigin::Agent;
        }
    }

    /// Marks the most recently pushed undo step as a user step.
    pub fn mark_last_undo_as_user(&mut self) {
        self.sync_meta_lengths();
        if let Some(m) = self.undo_meta.last_mut() {
            m.origin = StepOrigin::User;
        }
    }

    /// Label of the step [`Self::undo`] would apply next, if one was set (e.g. `"Agent turn (stopped)"`).
    pub fn undo_label(&self) -> Option<&str> {
        self.undo_meta.get(self.undo.len().checked_sub(1)?).and_then(|m| m.label.as_deref())
    }

    /// Label of the step [`Self::redo`] would apply next, if one was set.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo_meta.get(self.redo.len().checked_sub(1)?).and_then(|m| m.label.as_deref())
    }

    fn sync_meta_lengths(&mut self) {
        self.undo_meta.resize_with(self.undo.len(), UndoStepMeta::default);
        self.redo_meta.resize_with(self.redo.len(), UndoStepMeta::default);
    }

    /// Folds the contiguous agent steps on top of the undo stack since `depth` into one
    /// [`Command::Batch`] step labelled `label`, returning the number of folded steps (`#684`).
    ///
    /// Stops at the first step in `depth..` (walking from the top of the undo stack downward)
    /// that is a user edit. Refuses and returns `0` when the redo stack is not empty or when
    /// no foldable steps exist above `depth`.
    pub fn fold_since(&mut self, depth: usize, label: impl Into<String>) -> usize {
        if !self.redo.is_empty() || depth >= self.undo.len() {
            return 0;
        }
        self.sync_meta_lengths();
        let has_explicit_agent = self.undo_meta[depth..].iter().any(|m| m.origin == StepOrigin::Agent);
        let is_foldable = |m: &UndoStepMeta| {
            if has_explicit_agent {
                m.origin == StepOrigin::Agent
            } else {
                m.origin != StepOrigin::User
            }
        };
        let mut fold_start = self.undo.len();
        while fold_start > depth && is_foldable(&self.undo_meta[fold_start - 1]) {
            fold_start -= 1;
        }
        let folded_count = self.undo.len() - fold_start;
        if folded_count == 0 {
            return 0;
        }
        let mut tail: Vec<Command> = self.undo.drain(fold_start..).collect();
        self.undo_meta.truncate(fold_start);
        // Each step on `self.undo` is an inverse command pushed in execution order (oldest first).
        // `Command::Batch::apply` executes its children front-to-back, so reversing `tail` applies
        // the newest inverse first and the oldest inverse last when undoing the folded batch.
        tail.reverse();
        self.undo.push(Command::Batch(tail));
        self.undo_meta.push(UndoStepMeta { origin: StepOrigin::Agent, label: Some(label.into()) });
        folded_count
    }

    /// Reverts the latest step. Any deferred auto-layout queue from the superseded edit is dropped
    /// before applying the inverse command, and affected auto-layout containers are re-solved from
    /// their restored state so descendants return to the pre-edit layout.
    pub fn undo(&mut self, doc: &mut Document) -> Result<bool, DocError> {
        self.sync_meta_lengths();
        let Some(c) = self.undo.pop() else { return Ok(false) };
        let meta = self.undo_meta.pop().unwrap_or_default();
        let inv = apply_and_relayout(doc, c)?;
        self.redo.push(inv);
        self.redo_meta.push(meta);
        Ok(true)
    }
    /// Re-applies the latest undone step, dropping any superseded queue and re-solving affected
    /// auto-layout containers from their restored state, like [`Self::undo`].
    pub fn redo(&mut self, doc: &mut Document) -> Result<bool, DocError> {
        self.sync_meta_lengths();
        let Some(c) = self.redo.pop() else { return Ok(false) };
        let meta = self.redo_meta.pop().unwrap_or_default();
        let inv = apply_and_relayout(doc, c)?;
        self.undo.push(inv);
        self.undo_meta.push(meta);
        Ok(true)
    }
    /// The command [`Self::undo`] would apply next.
    pub fn peek_undo(&self) -> Option<&Command> {
        self.undo.last()
    }
    /// The command [`Self::redo`] would apply next.
    pub fn peek_redo(&self) -> Option<&Command> {
        self.redo.last()
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    /// Number of steps [`Self::undo`] can take.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }
    /// Forgets undo steps past `depth` without applying them, for edits the caller already
    /// reverted another way (by restoring a snapshot of the document).
    pub fn drop_undo_steps_after(&mut self, depth: usize) {
        self.undo.truncate(depth);
        self.undo_meta.truncate(depth);
    }
    /// The steps [`Self::redo`] can take, oldest first, for [`Self::restore_redo`].
    pub fn redo_steps(&self) -> Vec<Command> {
        self.redo.clone()
    }
    /// Puts back the redo steps an edit cleared, for an edit the caller reverted another way.
    pub fn restore_redo(&mut self, steps: Vec<Command>) {
        let len = steps.len();
        self.redo = steps;
        self.redo_meta.resize_with(len, UndoStepMeta::default);
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    /// Replaces any non-finite numbers stored inside undo and redo commands with their neutral values.
    pub fn neutralize_non_finite(&mut self) -> usize {
        let mut replaced = 0;
        for cmd in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            replaced += cmd.neutralize_non_finite();
        }
        replaced
    }
}

impl Command {
    /// Replaces any non-finite numbers carried by this command with their neutral values.
    pub fn neutralize_non_finite(&mut self) -> usize {
        match self {
            Command::Insert { nodes, .. } | Command::InsertPage { nodes, .. } => nodes.iter_mut().map(finite_json::neutralize_in_place).sum(),
            Command::Update { node } => finite_json::neutralize_in_place(node.as_mut()),
            Command::SetFlows { flows } => flows.iter_mut().map(finite_json::neutralize_in_place).sum(),
            Command::SetStyles { styles, style_meta } => {
                styles.values_mut().map(finite_json::neutralize_in_place).sum::<usize>()
                    + style_meta.values_mut().map(finite_json::neutralize_in_place).sum::<usize>()
            }
            Command::SetComments { comments } => comments.iter_mut().map(finite_json::neutralize_in_place).sum(),
            Command::SetLibraryState { state, .. } => finite_json::neutralize_in_place(state),
            Command::SetGuides { guides, .. } => guides.iter_mut().map(finite_json::neutralize_in_place).sum(),
            Command::SetPrototypeDevice { device } => finite_json::neutralize_in_place(device),
            Command::SetVariables { variables, collections } => {
                variables.values_mut().map(finite_json::neutralize_in_place).sum::<usize>()
                    + collections.values_mut().map(finite_json::neutralize_in_place).sum::<usize>()
            }
            Command::Batch(cmds) => cmds.iter_mut().map(Self::neutralize_non_finite).sum(),
            Command::Remove { .. } | Command::Reparent { .. } | Command::RemovePage { .. } | Command::SetImages { .. } => 0,
        }
    }
}

/// Variable binding keys whose resolved value the layout solver reads: the box, the paddings and
/// spacings, visibility, and the border weights and opacity that decide the border inset.
fn is_layout_binding_key(key: &str) -> bool {
    matches!(
        key,
        "width"
            | "height"
            | "visible"
            | "itemSpacing"
            | "counterAxisSpacing"
            | "paddingTop"
            | "paddingRight"
            | "paddingBottom"
            | "paddingLeft"
            | "strokeWeight"
            | "strokeTopWeight"
            | "strokeRightWeight"
            | "strokeBottomWeight"
            | "strokeLeftWeight"
    ) || (key.starts_with("strokes/") && key.ends_with("/opacity"))
}

/// Whether replacing `old` with `new` can change what the auto-layout solver computes, for the node
/// itself or for the frame that lays it out. Only what the solver reads counts: the size, the
/// child sizing and limits, visibility, the frame's own auto-layout settings, a border that takes
/// space (`bordersTakeSpace`, layout version 0), and variable bindings or modes that feed those.
/// Paint colours, effects and the like never do, and neither does the transform: the solver reads
/// a flow child's size only and writes its position, and absolute children stay where they are.
fn layout_inputs_changed(old: &Node, new: &Node) -> bool {
    let old_al = old.kind.frame_props().and_then(|f| f.auto_layout.as_ref());
    let new_al = new.kind.frame_props().and_then(|f| f.auto_layout.as_ref());
    old.size != new.size
        || old.visible != new.visible
        || old.layout_child != new.layout_child
        || std::mem::discriminant(&old.kind) != std::mem::discriminant(&new.kind)
        || old_al != new_al
        || new_al.is_some_and(|al| al.effective_padding(&old.stroke) != al.effective_padding(&new.stroke))
        || old.variables.modes != new.variables.modes
        || !old.variables.bindings.iter().filter(|(k, _)| is_layout_binding_key(k)).eq(new.variables.bindings.iter().filter(|(k, _)| is_layout_binding_key(k)))
}

fn collect_relayout_seeds(doc: &Document, cmd: &Command, seeds: &mut Vec<NodeId>) {
    match cmd {
        Command::Update { node } => {
            let Some(cur) = doc.get(node.id) else { return };
            if !layout_inputs_changed(cur, node) {
                return;
            }
            let is_auto =
                node.kind.frame_props().and_then(|f| f.auto_layout.as_ref()).is_some() || cur.kind.frame_props().and_then(|f| f.auto_layout.as_ref()).is_some();
            if is_auto {
                seeds.push(node.id);
            }
            if let Some(p) = cur.parent {
                seeds.push(p);
            }
        }
        Command::Insert { parent, nodes, .. } => {
            seeds.push(*parent);
            if let Some(root) = nodes.first() {
                if root.kind.frame_props().and_then(|f| f.auto_layout.as_ref()).is_some() {
                    seeds.push(root.id);
                }
            }
        }
        Command::Remove { id } => {
            if let Some(p) = doc.get(*id).and_then(|n| n.parent) {
                seeds.push(p);
            }
        }
        Command::Reparent { id, parent, .. } => {
            if let Some(old_p) = doc.get(*id).and_then(|n| n.parent) {
                seeds.push(old_p);
            }
            seeds.push(*parent);
        }
        Command::Batch(cmds) => {
            for c in cmds {
                collect_relayout_seeds(doc, c, seeds);
            }
        }
        _ => {}
    }
}

/// Applies `c` and re-solves only the auto-layout frames it changed: each seeded frame is solved on
/// its own and the walk climbs to a parent only while a frame's size changed. Containers below a
/// seed are not walked, so untouched siblings keep their positions. `pending_relayout` is left as
/// it is: a queued subtree is always solved against the tree as it stands when the queue is
/// drained, so it stays valid across the step, and a tail this solve defers joins the queue to be
/// finished a bounded step per frame, like any other edit. A command that fails to apply leaves the
/// document and the queue untouched.
fn apply_and_relayout(doc: &mut Document, c: Command) -> Result<Command, DocError> {
    let mut seeds = Vec::new();
    collect_relayout_seeds(doc, &c, &mut seeds);
    let inv = c.apply(doc)?;
    doc.relayout_seeds_upward(&seeds);
    Ok(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_update_and_remove() {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let id = d.alloc_id();
        d.insert(page, Node::rectangle(id, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        let mut h = History::default();

        let mut n = d.get(id).unwrap().clone();
        n.name = "Renamed".into();
        h.exec(&mut d, Command::Update { node: Box::new(n) }).unwrap();
        assert_eq!(d.get(id).unwrap().name, "Renamed");
        h.undo(&mut d).unwrap();
        assert_eq!(d.get(id).unwrap().name, "Rectangle");

        h.exec(&mut d, Command::Remove { id }).unwrap();
        assert!(d.get(id).is_none());
        h.undo(&mut d).unwrap();
        assert_eq!(d.get(page).unwrap().children, vec![id]);
        h.redo(&mut d).unwrap();
        assert!(d.get(id).is_none());
    }

    /// 70 nested Fill frames under a page, 200 wide; the outermost is the only one that is not Fill,
    /// and a 10 px leaf hugs the right edge of the innermost frame.
    fn nested_fill_frames() -> (Document, Vec<NodeId>, NodeId) {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let mut frames = Vec::new();
        let mut cur = page;
        for i in 0..70 {
            let id = d.alloc_id();
            let mut f = Node::frame(id, Rect::new(0.0, 0.0, 200.0, 40.0));
            if let NodeKind::Frame(fp) = &mut f.kind {
                fp.auto_layout = Some(AutoLayout {
                    direction: LayoutDirection::Horizontal,
                    primary_sizing: Sizing::Fixed,
                    counter_sizing: Sizing::Fixed,
                    primary_align: LayoutAlign::End,
                    ..AutoLayout::default()
                });
            }
            if i > 0 {
                f.layout_child = LayoutChild { horizontal: Sizing::Fill, vertical: Sizing::Fill, ..LayoutChild::default() };
            }
            d.insert(cur, f).unwrap();
            frames.push(id);
            cur = id;
        }
        let leaf = d.alloc_id();
        d.insert(cur, Node::rectangle(leaf, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        d.relayout_subtree(frames[0]);
        d.flush_pending_relayout();
        assert_eq!(d.get(leaf).unwrap().transform.tx, 190.0);
        (d, frames, leaf)
    }

    fn widths(d: &Document, frames: &[NodeId]) -> Vec<f32> {
        frames.iter().map(|f| d.get(*f).unwrap().size.x).collect()
    }

    #[test]
    fn undo_and_redo_never_leave_a_stale_layout_queue_to_write_later() {
        let (mut d, frames, leaf) = nested_fill_frames();
        let mut h = History::default();
        let mut wide = d.get(frames[0]).unwrap().clone();
        wide.size.x = 600.0;
        h.exec(&mut d, Command::Update { node: Box::new(wide) }).unwrap();
        d.relayout_subtree(frames[0]);
        assert!(d.relayout_skipped_count() > 0, "precondition: the nest is deeper than one relayout reaches");

        h.undo(&mut d).unwrap();
        assert_eq!(d.get(frames[0]).unwrap().size.x, 200.0, "the command was undone");
        // The undo re-solves from the restored size within the usual hop budget and queues the
        // rest, like any edit; it does not finish the whole 70-deep chain on the calling thread.
        assert!(d.relayout_skipped_count() > 0, "the tail past the hop budget is queued for later frames, not solved on the undo");
        let hop_budget = crate::layout_apply::MAX_RELAYOUT_RESOLVE_DEPTH;
        assert!(widths(&d, &frames)[..hop_budget].iter().all(|w| *w == 200.0), "the frames within the hop budget are restored at once");
        d.flush_pending_relayout();
        let after_undo = widths(&d, &frames);
        assert!(after_undo.iter().all(|w| *w == 200.0), "draining the queue restores the 200 px layout across all 70 frames: {after_undo:?}");
        assert_eq!(d.get(leaf).unwrap().transform.tx, 190.0, "undo restores the leaf position inside the innermost frame");
        assert_eq!(d.relayout_skipped_count(), 0);

        h.redo(&mut d).unwrap();
        assert!(d.relayout_skipped_count() > 0);
        d.flush_pending_relayout();
        assert!(widths(&d, &frames).iter().all(|w| *w == 600.0), "redo brings back the whole edited layout: {:?}", widths(&d, &frames));
        assert_eq!(d.get(leaf).unwrap().transform.tx, 590.0, "redo places the leaf at the right edge of the 600 px frame");
    }

    #[test]
    fn undo_when_second_step_does_not_resize_queued_chain_still_drains_leaf_to_590() {
        let (mut d, frames, leaf) = nested_fill_frames();
        let mut h = History::default();

        let mut wide = d.get(frames[0]).unwrap().clone();
        wide.size.x = 600.0;
        h.exec(&mut d, Command::Update { node: Box::new(wide) }).unwrap();
        d.relayout_subtree(frames[0]);
        let queued_before = d.pending_relayout().to_vec();
        assert!(!queued_before.is_empty(), "precondition: tail of 70-deep chain is queued");

        assert!(apply_and_relayout(&mut d, Command::Remove { id: NodeId(999_999) }).is_err());
        assert_eq!(d.pending_relayout(), queued_before.as_slice(), "failed apply must not lose pending_relayout");

        let mut painted = d.get(frames[0]).unwrap().clone();
        painted.fills = vec![Paint::solid(Color::WHITE)];
        h.exec(&mut d, Command::Update { node: Box::new(painted) }).unwrap();

        h.undo(&mut d).unwrap();
        assert_eq!(d.pending_relayout(), queued_before.as_slice(), "non-layout undo must preserve pending_relayout");
        d.flush_pending_relayout();
        assert!(widths(&d, &frames).iter().all(|w| *w == 600.0), "undoing the fill step must not drop the queued relayout tail: {:?}", widths(&d, &frames));
        assert_eq!(d.get(leaf).unwrap().transform.tx, 590.0, "leaf reaches 590 once the queue drains");

        h.undo(&mut d).unwrap();
        assert!(d.relayout_skipped_count() > 0, "the undo defers the tail past its hop budget");
        d.flush_pending_relayout();
        assert!(widths(&d, &frames).iter().all(|w| *w == 200.0));
        assert_eq!(d.get(leaf).unwrap().transform.tx, 190.0);

        // Also verify when the second step DOES touch a layout input of the outer frame without
        // resizing the queued Fill chain: undoing that step re-solves the outer frame and keeps the tail.
        let mut wide2 = d.get(frames[0]).unwrap().clone();
        wide2.size.x = 600.0;
        h.exec(&mut d, Command::Update { node: Box::new(wide2) }).unwrap();
        d.relayout_subtree(frames[0]);
        assert!(d.relayout_skipped_count() > 0);

        let mut spaced_outer = d.get(frames[0]).unwrap().clone();
        if let NodeKind::Frame(fp) = &mut spaced_outer.kind {
            fp.auto_layout.as_mut().unwrap().item_spacing = 6.0;
        }
        h.exec(&mut d, Command::Update { node: Box::new(spaced_outer) }).unwrap();

        h.undo(&mut d).unwrap();
        d.flush_pending_relayout();
        assert!(widths(&d, &frames).iter().all(|w| *w == 600.0));
        assert_eq!(d.get(leaf).unwrap().transform.tx, 590.0);
    }

    #[test]
    fn undo_and_redo_of_batch_updating_stack_and_direct_child_resolves_fill_sibling() {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let stack = d.alloc_id();
        let mut stack_node = Node::frame(stack, Rect::new(0.0, 0.0, 200.0, 40.0));
        if let NodeKind::Frame(fp) = &mut stack_node.kind {
            fp.auto_layout = Some(AutoLayout {
                direction: LayoutDirection::Horizontal,
                primary_sizing: Sizing::Fixed,
                counter_sizing: Sizing::Fixed,
                ..AutoLayout::default()
            });
        }
        d.insert(page, stack_node).unwrap();

        let child_b = d.alloc_id();
        d.insert(stack, Node::rectangle(child_b, Rect::new(0.0, 0.0, 40.0, 40.0))).unwrap();

        let child_c = d.alloc_id();
        let mut c_node = Node::rectangle(child_c, Rect::new(0.0, 0.0, 10.0, 40.0));
        c_node.layout_child = LayoutChild { horizontal: Sizing::Fill, vertical: Sizing::Fixed, ..LayoutChild::default() };
        d.insert(stack, c_node).unwrap();

        d.relayout_subtree(stack);
        assert_eq!(d.get(child_c).unwrap().size.x, 160.0);
        assert_eq!(d.get(child_c).unwrap().transform.tx, 40.0);

        let mut h = History::default();
        let mut stack_wide = d.get(stack).unwrap().clone();
        stack_wide.size.x = 500.0;
        let mut b_wide = d.get(child_b).unwrap().clone();
        b_wide.size.x = 80.0;
        h.exec(&mut d, Command::Batch(vec![Command::Update { node: Box::new(stack_wide) }, Command::Update { node: Box::new(b_wide) }])).unwrap();
        d.relayout_subtree(stack);
        assert_eq!(d.get(child_c).unwrap().size.x, 420.0);
        assert_eq!(d.get(child_c).unwrap().transform.tx, 80.0);

        h.undo(&mut d).unwrap();
        assert_eq!(d.get(stack).unwrap().size.x, 200.0);
        assert_eq!(d.get(child_b).unwrap().size.x, 40.0);
        assert_eq!(d.get(child_c).unwrap().size.x, 160.0, "undo must re-solve stack so Fill sibling shrinks back to 160");
        assert_eq!(d.get(child_c).unwrap().transform.tx, 40.0);

        h.redo(&mut d).unwrap();
        assert_eq!(d.get(stack).unwrap().size.x, 500.0);
        assert_eq!(d.get(child_b).unwrap().size.x, 80.0);
        assert_eq!(d.get(child_c).unwrap().size.x, 420.0, "redo must re-solve stack so Fill sibling expands back to 420");
        assert_eq!(d.get(child_c).unwrap().transform.tx, 80.0);
    }

    fn auto_frame(d: &mut Document, parent: NodeId, size: (f32, f32), sizing: Sizing) -> NodeId {
        let id = d.alloc_id();
        let mut f = Node::frame(id, Rect::new(0.0, 0.0, size.0, size.1));
        if let NodeKind::Frame(fp) = &mut f.kind {
            fp.auto_layout = Some(AutoLayout {
                direction: LayoutDirection::Vertical,
                primary_sizing: sizing,
                counter_sizing: sizing,
                layout_version: 1,
                ..AutoLayout::default()
            });
        }
        d.insert(parent, f).unwrap();
        id
    }

    fn visible_stroke(weight: f32, color: Color) -> Stroke {
        Stroke { paints: vec![Paint::solid(color)], weight, align: StrokeAlign::Inside, ..Stroke::default() }
    }

    #[test]
    fn undo_and_redo_of_a_paint_only_stroke_edit_solve_no_frame() {
        let (mut d, frames, leaf) = nested_fill_frames();
        d.get_mut(leaf).unwrap().stroke = visible_stroke(1.0, Color::BLACK);
        d.get_mut(frames[10]).unwrap().stroke = visible_stroke(1.0, Color::BLACK);
        let mut h = History::default();
        for id in [leaf, frames[10]] {
            let mut recoloured = d.get(id).unwrap().clone();
            recoloured.stroke = visible_stroke(1.0, Color::WHITE);
            h.exec(&mut d, Command::Update { node: Box::new(recoloured) }).unwrap();
        }
        let widths_before = widths(&d, &frames);
        crate::layout_apply::reset_solve_calls();
        h.undo(&mut d).unwrap();
        h.undo(&mut d).unwrap();
        h.redo(&mut d).unwrap();
        h.redo(&mut d).unwrap();
        assert_eq!(crate::layout_apply::solve_calls(), 0, "a stroke colour never reaches the layout solver");
        assert_eq!(widths(&d, &frames), widths_before);
        assert_eq!(d.get(leaf).unwrap().transform.tx, 190.0);
    }

    #[test]
    fn undo_and_redo_of_a_move_or_effect_edit_solve_no_frame() {
        let (mut d, _frames, leaf) = nested_fill_frames();
        let mut h = History::default();
        let mut moved = d.get(leaf).unwrap().clone();
        moved.transform.tx = 3.0;
        moved.effects =
            vec![Effect::DropShadow { color: Color::BLACK, offset: Vec2::new(0.0, 1.0), radius: 2.0, spread: 0.0, visible: true, show_behind: false }];
        h.exec(&mut d, Command::Update { node: Box::new(moved) }).unwrap();
        crate::layout_apply::reset_solve_calls();
        h.undo(&mut d).unwrap();
        h.redo(&mut d).unwrap();
        assert_eq!(crate::layout_apply::solve_calls(), 0, "the solver reads neither a flow child's transform nor its effects");
    }

    #[test]
    fn undo_of_a_leaf_resize_solves_only_the_chain_up_to_where_the_size_holds() {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let top = auto_frame(&mut d, page, (400.0, 400.0), Sizing::Fixed);
        let h0 = auto_frame(&mut d, top, (0.0, 0.0), Sizing::Hug);
        let fixed = auto_frame(&mut d, h0, (100.0, 100.0), Sizing::Fixed);
        let h1 = auto_frame(&mut d, fixed, (0.0, 0.0), Sizing::Hug);
        let h2 = auto_frame(&mut d, h1, (0.0, 0.0), Sizing::Hug);
        let leaf = d.alloc_id();
        d.insert(h2, Node::rectangle(leaf, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        let card = auto_frame(&mut d, top, (80.0, 80.0), Sizing::Fixed);
        let card_child = d.alloc_id();
        d.insert(card, Node::rectangle(card_child, Rect::new(0.0, 0.0, 20.0, 20.0))).unwrap();
        d.relayout_subtree(top);
        assert_eq!(d.get(h2).unwrap().size.y, 10.0);
        // A position the solver would overwrite if it ever re-solved the untouched sibling card.
        d.get_mut(card_child).unwrap().transform.tx = 15.0;

        let mut h = History::default();
        let mut tall = d.get(leaf).unwrap().clone();
        tall.size.y = 30.0;
        h.exec(&mut d, Command::Update { node: Box::new(tall) }).unwrap();
        d.relayout_subtree(top);
        assert_eq!((d.get(h2).unwrap().size.y, d.get(h1).unwrap().size.y), (30.0, 30.0));
        d.get_mut(card_child).unwrap().transform.tx = 15.0;

        crate::layout_apply::reset_solve_calls();
        h.undo(&mut d).unwrap();
        assert_eq!(crate::layout_apply::solve_calls(), 3, "h2 and h1 changed size; the fixed frame above them held it, so the climb stops there");
        assert_eq!((d.get(h2).unwrap().size.y, d.get(h1).unwrap().size.y), (10.0, 10.0));
        assert_eq!(d.get(card_child).unwrap().transform.tx, 15.0, "the untouched sibling card is not re-solved");

        crate::layout_apply::reset_solve_calls();
        h.redo(&mut d).unwrap();
        assert_eq!(crate::layout_apply::solve_calls(), 3);
        assert_eq!((d.get(h2).unwrap().size.y, d.get(h1).unwrap().size.y), (30.0, 30.0));
        assert_eq!(d.get(card_child).unwrap().transform.tx, 15.0);
    }

    #[test]
    fn stroke_weight_undo_re_solves_only_when_borders_take_space() {
        for (layout_version, stroke_included, expect_solved) in [(0u8, true, true), (1, true, false), (0, false, false)] {
            let mut d = Document::new("t");
            let page = d.add_page("P");
            let stack = auto_frame(&mut d, page, (100.0, 100.0), Sizing::Fixed);
            if let NodeKind::Frame(fp) = &mut d.get_mut(stack).unwrap().kind {
                let al = fp.auto_layout.as_mut().unwrap();
                al.layout_version = layout_version;
                al.stroke_included = stroke_included;
            }
            d.get_mut(stack).unwrap().stroke = visible_stroke(1.0, Color::BLACK);
            let child = d.alloc_id();
            d.insert(stack, Node::rectangle(child, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
            d.relayout_subtree(stack);
            let at_one = d.get(child).unwrap().transform.tx;

            let mut h = History::default();
            let mut thick = d.get(stack).unwrap().clone();
            thick.stroke = visible_stroke(5.0, Color::BLACK);
            h.exec(&mut d, Command::Update { node: Box::new(thick) }).unwrap();
            d.relayout_subtree(stack);
            let at_five = d.get(child).unwrap().transform.tx;
            assert_eq!(at_five != at_one, expect_solved, "version {layout_version} included {stroke_included}: the solver's reading of the border");

            crate::layout_apply::reset_solve_calls();
            h.undo(&mut d).unwrap();
            assert_eq!(crate::layout_apply::solve_calls() > 0, expect_solved, "version {layout_version} included {stroke_included}");
            if expect_solved {
                assert_eq!(d.get(child).unwrap().transform.tx, at_one, "the child returns to the 1 px border position");
            }
        }
    }

    #[test]
    fn undo_with_a_pending_tail_and_an_unrelated_layout_edit_keeps_the_tail_queued() {
        let (mut d, frames, leaf) = nested_fill_frames();
        let mut h = History::default();
        let mut wide = d.get(frames[0]).unwrap().clone();
        wide.size.x = 600.0;
        h.exec(&mut d, Command::Update { node: Box::new(wide) }).unwrap();
        d.relayout_subtree(frames[0]);
        let queued = d.pending_relayout().to_vec();
        assert!(!queued.is_empty());

        let mut spaced = d.get(frames[1]).unwrap().clone();
        if let NodeKind::Frame(fp) = &mut spaced.kind {
            fp.auto_layout.as_mut().unwrap().item_spacing = 4.0;
        }
        h.exec(&mut d, Command::Update { node: Box::new(spaced) }).unwrap();
        h.undo(&mut d).unwrap();
        assert!(queued.iter().all(|q| d.pending_relayout().contains(q)), "an undo elsewhere must not drop the queued tail");
        d.flush_pending_relayout();
        assert_eq!(d.get(leaf).unwrap().transform.tx, 590.0);
    }

    #[test]
    fn undo_redo_images_add_only_what_was_missing() {
        let mut d = Document::new("t");
        d.images.insert("old".into(), vec![9]);
        let mut h = History::default();
        let add: BTreeMap<String, Vec<u8>> = [("old".to_string(), vec![0]), ("new".to_string(), vec![1, 2])].into();
        h.exec(&mut d, Command::SetImages { add, remove: vec![] }).unwrap();
        assert_eq!(d.images["old"], vec![9], "an existing hash keeps its bytes");
        assert_eq!(d.images["new"], vec![1, 2]);
        h.undo(&mut d).unwrap();
        assert_eq!(d.images.keys().collect::<Vec<_>>(), ["old"], "undo removes only the newly added image");
        assert_eq!(d.images["old"], vec![9]);
        h.redo(&mut d).unwrap();
        assert_eq!(d.images["new"], vec![1, 2]);
        h.exec(&mut d, Command::SetImages { add: BTreeMap::new(), remove: vec!["new".into(), "missing".into()] }).unwrap();
        assert!(!d.images.contains_key("new"));
        h.undo(&mut d).unwrap();
        assert_eq!(d.images["new"], vec![1, 2], "undoing a removal puts the bytes back");
    }

    #[test]
    fn undo_redo_flows() {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let fid = d.alloc_id();
        let f = d.insert(page, Node::frame(fid, Rect::new(0.0, 0.0, 10.0, 10.0))).unwrap();
        let flow = Flow { name: "Flow 1".into(), page, start: f, description: String::new() };
        let mut h = History::default();
        h.exec(&mut d, Command::SetFlows { flows: vec![flow.clone()] }).unwrap();
        assert_eq!(d.flows, vec![flow]);
        h.undo(&mut d).unwrap();
        assert!(d.flows.is_empty());
        h.redo(&mut d).unwrap();
        assert_eq!(d.flows.len(), 1);
    }

    #[test]
    fn undo_redo_pages() {
        let mut d = Document::new("t");
        let p1 = d.add_page("P1");
        let pid = d.alloc_id();
        let mut page = Node::new(pid, NodeKind::Page { background: Color::WHITE });
        page.name = "P2".into();
        let mut h = History::default();
        h.exec(&mut d, Command::InsertPage { index: 0, nodes: vec![page] }).unwrap();
        assert_eq!(d.pages, vec![pid, p1]);
        let rid = d.alloc_id();
        d.insert(pid, Node::rectangle(rid, Rect::new(0.0, 0.0, 1.0, 1.0))).unwrap();

        h.exec(&mut d, Command::RemovePage { id: pid }).unwrap();
        assert_eq!(d.pages, vec![p1]);
        assert!(d.get(rid).is_none());
        h.undo(&mut d).unwrap();
        assert_eq!(d.pages, vec![pid, p1]);
        assert_eq!(d.get(pid).unwrap().children, vec![rid]);
        h.undo(&mut d).unwrap();
        assert_eq!(d.pages, vec![p1]);
    }

    #[test]
    fn fold_since_makes_the_tail_one_undo_step() {
        let doc_hash = |doc: &Document| -> String {
            serde_json::to_string(&(&doc.name, &doc.kind, &doc.nodes, &doc.pages, &doc.styles, &doc.flows, &doc.variables)).unwrap()
        };
        let mut d = Document::new("t");
        let page = d.add_page("P1");
        let mut h = History::default();

        // Pre-turn user edit at depth 1.
        let r0 = d.alloc_id();
        h.exec_user(&mut d, Command::Insert { parent: page, index: 0, nodes: vec![Node::rectangle(r0, Rect::new(0.0, 0.0, 20.0, 20.0))] }).unwrap();
        let start_depth = h.undo_depth();
        assert_eq!(start_depth, 1);
        let start_hash = doc_hash(&d);

        // Three agent steps during the turn: insert r1, update r1 name, insert r2.
        let r1 = d.alloc_id();
        h.exec_agent(&mut d, Command::Insert { parent: page, index: 1, nodes: vec![Node::rectangle(r1, Rect::new(30.0, 0.0, 40.0, 40.0))] }).unwrap();
        let mut r1_updated = d.get(r1).unwrap().clone();
        r1_updated.name = "Agent Card".into();
        h.exec_agent(&mut d, Command::Update { node: Box::new(r1_updated) }).unwrap();
        let r2 = d.alloc_id();
        h.exec_agent(&mut d, Command::Insert { parent: page, index: 2, nodes: vec![Node::rectangle(r2, Rect::new(80.0, 0.0, 40.0, 40.0))] }).unwrap();
        assert_eq!(h.undo_depth(), start_depth + 3);
        let after_turn_hash = doc_hash(&d);

        let folded = h.fold_since(start_depth, "Agent turn (stopped)");
        assert_eq!(folded, 3);
        assert_eq!(h.undo_depth(), start_depth + 1);
        assert_eq!(h.undo_label(), Some("Agent turn (stopped)"));

        // One undo restores the exact document state at turn start.
        assert!(h.undo(&mut d).unwrap());
        assert_eq!(doc_hash(&d), start_hash);
        assert_eq!(h.redo_label(), Some("Agent turn (stopped)"));

        // One redo reapplies all 3 steps together.
        assert!(h.redo(&mut d).unwrap());
        assert_eq!(doc_hash(&d), after_turn_hash);
    }

    #[test]
    fn fold_stops_at_a_user_step() {
        let mut d = Document::new("t");
        let page = d.add_page("P1");
        let mut h = History::default();
        let start_depth = h.undo_depth();

        // Agent step 1 before user edit.
        let a1 = d.alloc_id();
        h.exec_agent(&mut d, Command::Insert { parent: page, index: 0, nodes: vec![Node::rectangle(a1, Rect::new(0.0, 0.0, 10.0, 10.0))] }).unwrap();

        // User edit in between.
        let u1 = d.alloc_id();
        h.exec_user(&mut d, Command::Insert { parent: page, index: 1, nodes: vec![Node::rectangle(u1, Rect::new(20.0, 0.0, 10.0, 10.0))] }).unwrap();

        // Contiguous agent tail (2 steps) after user edit.
        let a2 = d.alloc_id();
        h.exec_agent(&mut d, Command::Insert { parent: page, index: 2, nodes: vec![Node::rectangle(a2, Rect::new(40.0, 0.0, 10.0, 10.0))] }).unwrap();
        let a3 = d.alloc_id();
        h.exec_agent(&mut d, Command::Insert { parent: page, index: 3, nodes: vec![Node::rectangle(a3, Rect::new(60.0, 0.0, 10.0, 10.0))] }).unwrap();
        assert_eq!(h.undo_depth(), 4);

        // Fold stops at the user edit: only the 2 contiguous tail agent steps fold.
        let folded = h.fold_since(start_depth, "Agent turn (stopped)");
        assert_eq!(folded, 2);
        assert_eq!(h.undo_depth(), 3, "a1 (1) + u1 (1) + folded(a2,a3) (1) = 3 steps");

        // Undoing once removes a2 and a3 together while keeping u1 and a1.
        assert!(h.undo(&mut d).unwrap());
        assert!(d.get(a2).is_none() && d.get(a3).is_none());
        assert!(d.get(u1).is_some() && d.get(a1).is_some());

        // Undoing again removes the user step u1 while keeping a1.
        assert!(h.undo(&mut d).unwrap());
        assert!(d.get(u1).is_none() && d.get(a1).is_some());
    }
}

#[cfg(test)]
mod style_tests {
    use crate::*;

    #[test]
    fn set_styles_undo() {
        let mut d = Document::new("t");
        d.add_page("P");
        let mut h = History::default();
        let mut styles = d.styles.clone();
        styles.insert("s1".into(), Style::Fill(vec![Paint::solid(Color::WHITE)]));
        let mut meta = d.style_meta.clone();
        meta.insert("s1".into(), StyleMeta { name: "Brand/White".into(), ..Default::default() });
        h.exec(&mut d, Command::SetStyles { styles, style_meta: meta }).unwrap();
        assert_eq!(d.style_name("s1"), "Brand/White");
        h.undo(&mut d).unwrap();
        assert!(d.styles.is_empty() && d.style_meta.is_empty());
        h.redo(&mut d).unwrap();
        assert_eq!(d.styles.len(), 1);
    }
}
