//! Stop an agent from the panel (`#684` / `AX-684`):
//! - Interrupt key table (`INTERRUPT_KEY_TABLE`), one row per CLI (`Grok` grounded from its guide;
//!   `Claude`, `Codex`, `Antigravity` default `Esc` then `Ctrl+C`).
//! - Per-tab `StopState { sent_at, second_sent, key_table_row }` sequence with a 3-second retry.
//! - Turn close on stop: applies held live patches, folds the turn's contiguous agent undo steps into
//!   one `Command::Batch` labelled `"Agent turn (stopped)"` via `History::fold_since`, clears
//!   `Document::in_progress` flags for the agent, and records `"Stopped by you after <N>s"`.
//! - Secondary header buttons `"Stop"` and `"Stop all"` (when 2+ tabs run) and chrome-only `Esc`.

use crate::app::App;
use crate::terminal::{AgentCliPreset, AgentTabStatus, TerminalAction, TerminalState, TerminalTab};
use crate::{theme, widgets};
use egui::{Align2, Id, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

/// Undo step label applied by [`designos_doc::History::fold_since`] when a turn is stopped (`§3.4`).
pub const STOP_UNDO_LABEL: &str = "Agent turn (stopped)";

/// Delay in milliseconds before sending the second interrupt key (`Ctrl+C`) if still running (`§3.3`).
pub const SECOND_KEY_DELAY_MS: u64 = 3_000;

/// Notice shown when a CLI ignores both interrupt keys (`§8.1`).
pub const STILL_RUNNING_NOTICE: &str = "Still running. Close the tab to end it.";

/// ASCII `ESC` (`0x1b`).
pub const KEY_ESC: u8 = 0x1b;

/// ASCII `ETX` / `Ctrl+C` (`0x03`).
pub const KEY_CTRL_C: u8 = 0x03;

/// One row of the per-CLI interrupt key table (`§3.2`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptKeyRow {
    pub cli: AgentCliPreset,
    pub first_key: u8,
    pub second_key: u8,
    pub source: &'static str,
    pub grounded: bool,
}

/// Canonical interrupt key table (`§3.2`). The keys live only in this table.
pub const INTERRUPT_KEY_TABLE: &[InterruptKeyRow] = &[
    InterruptKeyRow {
        cli: AgentCliPreset::Grok,
        first_key: KEY_CTRL_C,
        second_key: KEY_CTRL_C,
        source: "~/.grok/docs/user-guide/10-hooks.md:478 (Ctrl+C interrupts a turn; Esc never cancels a running turn)",
        grounded: true,
    },
    InterruptKeyRow {
        cli: AgentCliPreset::Claude,
        first_key: KEY_ESC,
        second_key: KEY_CTRL_C,
        source: "issue #684 default (Esc then Ctrl+C)",
        grounded: false,
    },
    InterruptKeyRow { cli: AgentCliPreset::Codex, first_key: KEY_ESC, second_key: KEY_CTRL_C, source: "issue #684 default (Esc then Ctrl+C)", grounded: false },
    InterruptKeyRow {
        cli: AgentCliPreset::Antigravity,
        first_key: KEY_ESC,
        second_key: KEY_CTRL_C,
        source: "issue #684 default (Esc then Ctrl+C)",
        grounded: false,
    },
];

/// Looks up the [`INTERRUPT_KEY_TABLE`] index and row for `cli`. Shell and help tabs have no row (`§3.7`).
pub fn interrupt_row_for(cli: AgentCliPreset) -> Option<(usize, &'static InterruptKeyRow)> {
    INTERRUPT_KEY_TABLE.iter().enumerate().find(|(_, row)| row.cli == cli)
}

/// Per-tab stop sequence state in memory (`§4`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopState {
    /// Timestamp (ms from [`designos_doc::now_ms`]) when the first interrupt key was sent.
    pub sent_at: u64,
    /// True once the 3-second follow-up `Ctrl+C` has been sent (`§3.3`).
    pub second_sent: bool,
    /// Index into [`INTERRUPT_KEY_TABLE`].
    pub key_table_row: usize,
}

impl StopState {
    /// Button label for the Stop control (`"Stop"` before the second key, `"Stopping…"` after, `§3.3` & `§7`).
    pub fn button_label(&self) -> &'static str {
        if self.second_sent {
            "Stopping…"
        } else {
            "Stop"
        }
    }
}

/// Formats the stopped-turn receipt line (`"Stopped by you after 42s"` or `"Stopped by you after 1m 20s"`, `§3.4` & `#683 §3.5`).
pub fn format_stopped_receipt(started_ms: u64, ended_ms: u64) -> String {
    let elapsed_ms = ended_ms.saturating_sub(started_ms);
    let secs = elapsed_ms.div_ceil(1_000).max(1);
    if secs < 60 {
        format!("Stopped by you after {secs}s")
    } else {
        format!("Stopped by you after {}m {}s", secs / 60, secs % 60)
    }
}

/// True when `tab` has a known running turn eligible for Stop (`§3.1`, `§3.7`, `§8.6`).
pub fn is_tab_turn_running(tab: &TerminalTab) -> bool {
    if tab.is_shell_process() || tab.resumed_without_known_turn {
        return false;
    }
    let explicit = tab.agent_status.lock().ok().map(|s| s.clone()).unwrap_or_default();
    match explicit {
        AgentTabStatus::Working(_) => true,
        AgentTabStatus::WaitingForInput(_) | AgentTabStatus::Done | AgentTabStatus::Stopped => false,
        AgentTabStatus::Idle => {
            matches!(tab.spawn_preset, AgentCliPreset::Codex)
                && tab.last_output_at.lock().ok().and_then(|g| *g).is_some_and(|ts| ts.elapsed() < std::time::Duration::from_millis(600))
        }
    }
}

/// Number of tabs in `state` whose turn is currently running (`§3.6`).
pub fn running_tab_count(state: &TerminalState) -> usize {
    state.tabs.iter().filter(|t| is_tab_turn_running(t)).count()
}

/// Starts the stop sequence on `tab` at `now_ms` (`§3.3`).
///
/// Does nothing and sends zero bytes if:
/// - the tab's turn is not currently running (`§3.3`, `§8.2`),
/// - a stop sequence is already in flight on the tab (`§8.7`), or
/// - the tab is a shell process (`§3.7`).
pub fn start_stop_on_tab(tab: &mut TerminalTab, now_ms: u64) -> bool {
    if !is_tab_turn_running(tab) || tab.stop_state.is_some() {
        return false;
    }
    let Some((row_idx, row)) = interrupt_row_for(tab.spawn_preset) else {
        return false;
    };
    if tab.turn_started_at_ms.is_none() {
        tab.turn_started_at_ms = Some(now_ms);
    }
    tab.write_bytes(&[row.first_key]);
    tab.stop_sent_keys.push(row.first_key);
    tab.stop_state = Some(StopState { sent_at: now_ms, second_sent: false, key_table_row: row_idx });
    true
}

/// Starts the stop sequence on the active tab in `state` at `now_ms`.
pub fn start_stop_active(state: &mut TerminalState, now_ms: u64) -> bool {
    let idx = state.active_tab;
    let Some(tab) = state.tabs.get_mut(idx) else {
        return false;
    };
    start_stop_on_tab(tab, now_ms)
}

/// Starts the stop sequence on every running tab in `state` (`"Stop all"`, `§3.6`).
/// Returns the number of tabs where a stop sequence was started.
pub fn stop_all(state: &mut TerminalState, now_ms: u64) -> usize {
    let mut started = 0;
    for tab in &mut state.tabs {
        if start_stop_on_tab(tab, now_ms) {
            started += 1;
        }
    }
    started
}

/// Advances the stop sequence on `tab` at `now_ms` (`§3.3`, `§3.4`, `§3.8`, `§8.1`).
///
/// - If `tab.stop_state` is active and the turn has ended (`!is_tab_turn_running(tab)` or `!tab.is_alive()`),
///   closes the stopped turn, records the receipt `"Stopped by you after <N>s"`, sets status to
///   [`AgentTabStatus::Stopped`], and returns `Some(TerminalAction::CloseStoppedTurn { .. })`.
/// - If the turn is still running at `now_ms >= sent_at + 3_000` and `!second_sent`, sends the second
///   interrupt key (`Ctrl+C`) once and sets `second_sent = true`.
/// - If still running after the second key was sent, sets `tab.still_running_notice` to [`STILL_RUNNING_NOTICE`].
pub fn tick_stop_on_tab(tab: &mut TerminalTab, session_id: &str, now_ms: u64) -> Option<TerminalAction> {
    let alive = tab.is_alive();
    let running = alive && is_tab_turn_running(tab);
    let (sent_at, second_sent, key_table_row) = {
        let st = tab.stop_state.as_ref()?;
        (st.sent_at, st.second_sent, st.key_table_row)
    };

    if !running {
        let started_ms = tab.turn_started_at_ms.take().unwrap_or(sent_at);
        let depth = tab.turn_start_undo_depth;
        tab.stop_state = None;
        tab.still_running_notice = None;
        tab.last_receipt = Some(format_stopped_receipt(started_ms, now_ms));
        if let Ok(mut status) = tab.agent_status.lock() {
            *status = AgentTabStatus::Stopped;
        }
        return Some(TerminalAction::CloseStoppedTurn {
            tab_id: tab.id,
            turn_start_depth: depth,
            run_id: Some(session_id.to_string()),
            agent: Some(tab.spawn_preset.binary_name().to_string()),
        });
    }

    if !second_sent && now_ms.saturating_sub(sent_at) >= SECOND_KEY_DELAY_MS {
        let row = &INTERRUPT_KEY_TABLE[key_table_row];
        tab.write_bytes(&[row.second_key]);
        tab.stop_sent_keys.push(row.second_key);
        if let Some(st) = tab.stop_state.as_mut() {
            st.second_sent = true;
        }
    } else if second_sent && now_ms.saturating_sub(sent_at) > SECOND_KEY_DELAY_MS {
        tab.still_running_notice = Some(STILL_RUNNING_NOTICE.to_string());
    }
    None
}

/// Polls all tabs in `state` for stop sequence transitions at `now_ms`.
pub fn poll_stop_sequences(state: &mut TerminalState, now_ms: u64) -> Vec<TerminalAction> {
    let session_id = state.session_id.clone();
    let mut out = Vec::new();
    for tab in &mut state.tabs {
        if let Some(action) = tick_stop_on_tab(tab, &session_id, now_ms) {
            out.push(action);
        }
    }
    if let Some(active) = state.tabs.get(state.active_tab) {
        if let Some(notice) = &active.still_running_notice {
            state.status_msg = Some(notice.clone());
        } else if let Some(receipt) = &active.last_receipt {
            if matches!(active.agent_status.lock().ok().as_deref(), Some(AgentTabStatus::Stopped)) {
                state.status_msg = Some(receipt.clone());
            }
        }
    }
    out
}

/// Handles an `Escape` key press in the Agents panel (`§3.1`, `§8.5`).
///
/// - When the terminal viewport has focus (`state.viewport_focused`), `Esc` is forwarded unchanged
///   (`0x1b`) to the active tab's PTY (or closes search if `state.search_open`), and NEVER stops the turn.
/// - When panel chrome has focus (`state.chrome_focused && !state.viewport_focused`) and the composer
///   text field is NOT focused (`!composer_focused`), `Esc` triggers [`start_stop_active`].
///
/// Returns `true` if `start_stop_active` was triggered.
pub fn handle_panel_escape(state: &mut TerminalState, composer_focused: bool, now_ms: u64) -> bool {
    if state.viewport_focused {
        if state.search_open {
            state.search_open = false;
        } else if let Some(tab) = state.active_tab_mut() {
            tab.write_bytes(&[KEY_ESC]);
            tab.stop_sent_keys.push(KEY_ESC);
        }
        return false;
    }
    if state.chrome_focused && !composer_focused {
        return start_stop_active(state, now_ms);
    }
    false
}

/// Closes a stopped agent turn on `app` (`§3.4`):
/// 1. Ingests any remaining lines from `doc-patch.jsonl` and applies held live patches (unless the user
///    is mid-edit, in which case the fold is deferred to `apply_pending_live_patches` when the edit ends).
/// 2. Folds the turn's contiguous agent undo steps since `turn_start_depth` into one `Command::Batch`
///    step labelled [`STOP_UNDO_LABEL`].
/// 3. Clears `Document::in_progress` flags owned by the stopped agent run.
pub fn close_stopped_turn_on_app(app: &mut App, turn_start_depth: usize, run_id: Option<&str>, agent: Option<&str>) {
    let (lines, errors) = app.terminal.live_patch.poll();
    for err in errors {
        app.show_toast(err, true);
    }
    if !lines.is_empty() {
        for err in app.terminal.live_patch.ingest(lines) {
            app.show_toast(err, true);
        }
    }
    if crate::live_patch_apply::user_edit_in_progress(app) {
        app.terminal.pending_stop_fold_depth = Some(turn_start_depth);
    } else {
        crate::live_patch_apply::apply_pending_live_patches(app);
        app.hist.fold_since(turn_start_depth, STOP_UNDO_LABEL);
    }
    let mut cleared = app.doc.clear_in_progress_for_run_or_agent(run_id, agent);
    if let Some(a) = agent {
        cleared.extend(app.doc.clear_in_progress_for_agent(a));
    }
    if !cleared.is_empty() {
        app.changed();
    }
}

/// Paints the secondary `"Stop"` button (and `"Stop all"` when 2+ tabs are running) in the panel
/// header strip at the right (`§7`), returning the left edge of the painted control group so the
/// tab strip knows where to stop.
pub fn paint_header_stop_controls(ui: &mut Ui, p: &Painter, state: &mut TerminalState, bar_r: Rect, right_anchor_x: f32) -> f32 {
    let now = designos_doc::now_ms();
    let running_count = running_tab_count(state);
    let active_running = state.tabs.get(state.active_tab).is_some_and(is_tab_turn_running);
    let active_stopping = state.tabs.get(state.active_tab).and_then(|t| t.stop_state.as_ref()).is_some_and(|s| s.second_sent);

    if !active_running && running_count < 2 {
        return right_anchor_x;
    }

    let btn_h = theme::CONTROL_STANDARD;
    let btn_y = bar_r.center().y - btn_h * 0.5;
    let mut cursor_right = right_anchor_x - theme::SPACE_INLINE;

    if active_running {
        let label = state.tabs.get(state.active_tab).and_then(|t| t.stop_state.as_ref()).map_or("Stop", StopState::button_label);
        let btn_w = theme::CONTROL_ROW + theme::SPACE_SECTION;
        let stop_rect = Rect::from_min_size(Pos2::new(cursor_right - btn_w, btn_y), Vec2::new(btn_w, btn_h));
        let resp = ui.interact(stop_rect, Id::new("agents-header-stop-btn"), Sense::click());
        let bg = if !active_stopping && resp.hovered() { theme::BG_HOVER } else { theme::BG_SECONDARY };
        p.rect_filled(stop_rect, theme::cr(theme::RADIUS_CONTROL), bg);
        p.rect_stroke(stop_rect, theme::cr(theme::RADIUS_CONTROL), Stroke::new(theme::BORDER_WIDTH, theme::BORDER), StrokeKind::Inside);

        let sq_side = theme::SPACE_INLINE;
        let sq_center = Pos2::new(stop_rect.left() + theme::INSET_BUTTON, stop_rect.center().y);
        let sq_rect = Rect::from_center_size(sq_center, Vec2::splat(sq_side));
        let fg = if active_stopping { theme::TEXT_SECONDARY } else { theme::TEXT };
        p.rect_filled(sq_rect, theme::cr(theme::RADIUS_MARK), fg);
        p.text(Pos2::new(sq_rect.right() + theme::SPACE_FACT, stop_rect.center().y), Align2::LEFT_CENTER, label, theme::font_caption_medium(), fg);
        widgets::record(ui, "agents-header-stop-btn", stop_rect);
        if !active_stopping && resp.clicked() {
            start_stop_active(state, now);
        }
        cursor_right = stop_rect.left();
        if state.tabs.get(state.active_tab).and_then(|t| t.stop_state.as_ref()).is_some_and(|s| !s.second_sent) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    if running_count >= 2 {
        let btn_w = theme::CONTROL_ROW + theme::SPACE_SECTION + theme::SPACE_INLINE;
        let right_x = if active_running { cursor_right - theme::SPACE_INLINE } else { cursor_right };
        let all_rect = Rect::from_min_size(Pos2::new(right_x - btn_w, btn_y), Vec2::new(btn_w, btn_h));
        let resp = ui.interact(all_rect, Id::new("agents-header-stop-all-btn"), Sense::click());
        let bg = if resp.hovered() { theme::BG_HOVER } else { theme::BG_SECONDARY };
        p.rect_filled(all_rect, theme::cr(theme::RADIUS_CONTROL), bg);
        p.rect_stroke(all_rect, theme::cr(theme::RADIUS_CONTROL), Stroke::new(theme::BORDER_WIDTH, theme::BORDER), StrokeKind::Inside);
        p.text(all_rect.center(), Align2::CENTER_CENTER, "Stop all", theme::font_caption_medium(), theme::TEXT);
        widgets::record(ui, "agents-header-stop-all-btn", all_rect);
        if resp.clicked() {
            stop_all(state, now);
        }
        cursor_right = all_rect.left();
    }

    cursor_right
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Interaction;
    use designos_doc::live_patch::{LiveOp, LivePatch, SiblingHint};
    use designos_doc::{Command, Document, History, InProgress, Node, Rect as DocRect};
    use portable_pty::CommandBuilder;

    fn new_test_app() -> App {
        let mut doc = Document::new("Test");
        doc.add_page("Page 1");
        App::new(doc)
    }

    fn spawn_looping_stub_tab(id: usize, preset: AgentCliPreset, ignore_first_key: bool) -> TerminalTab {
        let reads = if ignore_first_key { 2 } else { 1 };
        let script = format!(
            "python3 -u -c 'import sys, tty\ntty.setraw(sys.stdin.fileno())\nsys.stdout.write(\"READY\\n\")\nsys.stdout.flush()\nfor _ in range({reads}):\n    sys.stdin.buffer.read(1)\nsys.exit(0)\n'"
        );
        let mut cmd = CommandBuilder::new("sh");
        cmd.arg("-c");
        cmd.arg(&script);
        let mut tab = TerminalTab::spawn_test_stub(id, preset, cmd).expect("spawn stub tab");
        let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < ready_deadline {
            if tab.raw_log.lock().ok().is_some_and(|l| l.contains("READY")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if let Ok(mut st) = tab.agent_status.lock() {
            *st = AgentTabStatus::Working(Some("Editing layers…".into()));
        }
        tab.turn_started_at_ms = Some(1_000);
        tab
    }

    #[test]
    fn stop_ends_a_looping_stub_within_three_seconds() {
        designos_doc::set_mock_now_ms(Some(1_000));
        let mut tab = spawn_looping_stub_tab(1, AgentCliPreset::Claude, false);
        assert!(is_tab_turn_running(&tab));

        assert!(start_stop_on_tab(&mut tab, 1_000));
        assert_eq!(tab.stop_sent_keys, vec![KEY_ESC]);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut action = None;
        while std::time::Instant::now() < deadline {
            if let Some(act) = tick_stop_on_tab(&mut tab, "sess-684", 2_000) {
                action = Some(act);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(action.is_some(), "looping stub should exit and close its turn within 3 seconds");
        assert_eq!(tab.agent_status.lock().unwrap().clone(), AgentTabStatus::Stopped);
        assert!(tab.stop_state.is_none());
        designos_doc::set_mock_now_ms(None);
    }

    #[test]
    fn second_key_is_sent_only_if_still_running() {
        // Case A: Stub ignores the first key (Esc) -> gets Ctrl+C at t = 3s, and no third key.
        let mut tab_ignores = spawn_looping_stub_tab(1, AgentCliPreset::Claude, true);
        assert!(start_stop_on_tab(&mut tab_ignores, 1_000));
        assert_eq!(tab_ignores.stop_sent_keys, vec![KEY_ESC]);
        assert_eq!(tab_ignores.stop_state.as_ref().unwrap().button_label(), "Stop");

        // Before 3s: no second key sent.
        assert!(tick_stop_on_tab(&mut tab_ignores, "sess-a", 3_999).is_none());
        assert_eq!(tab_ignores.stop_sent_keys, vec![KEY_ESC]);

        // At t = 1_000 + 3_000 = 4_000: still running -> second key (Ctrl+C) sent once.
        assert!(tick_stop_on_tab(&mut tab_ignores, "sess-a", 4_000).is_none());
        assert_eq!(tab_ignores.stop_sent_keys, vec![KEY_ESC, KEY_CTRL_C]);
        assert_eq!(tab_ignores.stop_state.as_ref().unwrap().button_label(), "Stopping…");

        // Wait briefly for the stub to exit after receiving the second byte.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline && tab_ignores.is_alive() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let closed = tick_stop_on_tab(&mut tab_ignores, "sess-a", 4_500);
        assert!(closed.is_some());
        assert_eq!(tab_ignores.stop_sent_keys.len(), 2, "never sends a third key");

        // Case B: Stub exits on the first key -> gets NO second key at t = 3s.
        let mut tab_exits = spawn_looping_stub_tab(2, AgentCliPreset::Claude, false);
        assert!(start_stop_on_tab(&mut tab_exits, 1_000));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline && tab_exits.is_alive() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let closed_b = tick_stop_on_tab(&mut tab_exits, "sess-b", 2_000);
        assert!(closed_b.is_some());
        // Advancing past 3s sends nothing because the turn already ended.
        assert!(tick_stop_on_tab(&mut tab_exits, "sess-b", 5_000).is_none());
        assert_eq!(tab_exits.stop_sent_keys, vec![KEY_ESC], "stub that exited after first key must not receive a second key");
    }

    #[test]
    fn no_key_is_sent_after_the_turn_ended() {
        let mut tab = spawn_looping_stub_tab(1, AgentCliPreset::Grok, false);
        if let Ok(mut st) = tab.agent_status.lock() {
            *st = AgentTabStatus::Done;
        }
        assert!(!start_stop_on_tab(&mut tab, 1_000), "must not start stop when turn already ended");
        assert!(tab.stop_sent_keys.is_empty(), "zero bytes sent after Done");
        assert!(tick_stop_on_tab(&mut tab, "sess", 5_000).is_none());
        assert!(tab.stop_sent_keys.is_empty());
    }

    #[test]
    fn fold_since_makes_the_tail_one_undo_step() {
        let mut d = Document::new("t");
        let page = d.add_page("P1");
        let mut h = History::default();

        let r0 = d.alloc_id();
        h.exec_user(&mut d, Command::Insert { parent: page, index: 0, nodes: vec![Node::rectangle(r0, DocRect::new(0.0, 0.0, 20.0, 20.0))] }).unwrap();
        let start_depth = h.undo_depth();
        let start_hash = crate::home::doc_hash(&d);

        for i in 0..3 {
            let rid = d.alloc_id();
            h.exec_agent(
                &mut d,
                Command::Insert { parent: page, index: i + 1, nodes: vec![Node::rectangle(rid, DocRect::new((i as f32) * 30.0, 0.0, 20.0, 20.0))] },
            )
            .unwrap();
        }
        assert_eq!(h.undo_depth(), start_depth + 3);

        let folded = h.fold_since(start_depth, STOP_UNDO_LABEL);
        assert_eq!(folded, 3);
        assert_eq!(h.undo_depth(), start_depth + 1);
        assert_eq!(h.undo_label(), Some(STOP_UNDO_LABEL));

        assert!(h.undo(&mut d).unwrap());
        assert_eq!(crate::home::doc_hash(&d), start_hash);
    }

    #[test]
    fn fold_stops_at_a_user_step() {
        let mut d = Document::new("t");
        let page = d.add_page("P1");
        let mut h = History::default();
        let start_depth = h.undo_depth();

        let a1 = d.alloc_id();
        h.exec_agent(&mut d, Command::Insert { parent: page, index: 0, nodes: vec![Node::rectangle(a1, DocRect::new(0.0, 0.0, 10.0, 10.0))] }).unwrap();
        let u1 = d.alloc_id();
        h.exec_user(&mut d, Command::Insert { parent: page, index: 1, nodes: vec![Node::rectangle(u1, DocRect::new(20.0, 0.0, 10.0, 10.0))] }).unwrap();
        let a2 = d.alloc_id();
        h.exec_agent(&mut d, Command::Insert { parent: page, index: 2, nodes: vec![Node::rectangle(a2, DocRect::new(40.0, 0.0, 10.0, 10.0))] }).unwrap();
        let a3 = d.alloc_id();
        h.exec_agent(&mut d, Command::Insert { parent: page, index: 3, nodes: vec![Node::rectangle(a3, DocRect::new(60.0, 0.0, 10.0, 10.0))] }).unwrap();

        let folded = h.fold_since(start_depth, STOP_UNDO_LABEL);
        assert_eq!(folded, 2);
        assert_eq!(h.undo_depth(), 3);

        assert!(h.undo(&mut d).unwrap());
        assert!(d.get(a2).is_none() && d.get(a3).is_none());
        assert!(d.get(u1).is_some() && d.get(a1).is_some());
    }

    #[test]
    fn held_patches_apply_before_the_fold() {
        let mut app = new_test_app();
        let page = app.page;
        let start_depth = app.hist.undo_depth();

        // Simulate user mid-edit (panning/dragging on canvas) while two agent patches are held.
        app.canvas.inter = Interaction::Pan;
        let n1 = Node::rectangle(designos_doc::NodeId(500), DocRect::new(10.0, 10.0, 80.0, 40.0));
        let n2 = Node::rectangle(designos_doc::NodeId(501), DocRect::new(100.0, 10.0, 80.0, 40.0));
        app.terminal.live_patch.pending.push(LivePatch::new(vec![LiveOp::Insert { parent: page, at: SiblingHint::default(), nodes: vec![n1] }]));
        app.terminal.live_patch.pending.push(LivePatch::new(vec![LiveOp::Insert { parent: page, at: SiblingHint::default(), nodes: vec![n2] }]));

        // Stop closes the turn while the user is still mid-edit.
        close_stopped_turn_on_app(&mut app, start_depth, Some("run-held"), Some("claude"));
        assert_eq!(app.terminal.pending_stop_fold_depth, Some(start_depth));

        // User finishes the edit -> held patches apply and fold into one undo step.
        app.canvas.inter = Interaction::None;
        crate::live_patch_apply::apply_pending_live_patches(&mut app);

        assert_eq!(app.doc.get(page).unwrap().children.len(), 2, "both held patches must be in the document");
        assert_eq!(app.hist.undo_depth(), start_depth + 1, "both held patches must fold into one undo step");
        assert_eq!(app.hist.undo_label(), Some(STOP_UNDO_LABEL));
    }

    #[test]
    fn in_progress_flags_clear_on_stop() {
        designos_doc::set_mock_now_ms(Some(10_000));
        let mut app = new_test_app();
        let page = app.page;
        let fid = app.doc.alloc_id();
        let frame = app.doc.insert(page, Node::frame(fid, DocRect::new(0.0, 0.0, 400.0, 300.0))).unwrap();
        app.doc.set_in_progress(
            frame,
            Some(InProgress {
                run_id: Some("sess-stop".into()),
                agent: Some("claude".into()),
                since_ms: 10_000,
                stale_after_s: Some(300),
                status: Some("Building hero...".into()),
            }),
        );
        assert!(app.doc.get_in_progress(frame).is_some());

        let depth = app.hist.undo_depth();
        close_stopped_turn_on_app(&mut app, depth, Some("sess-stop"), Some("claude"));
        assert!(app.doc.get_in_progress(frame).is_none());
        assert!(app.doc.in_progress.is_empty());
        designos_doc::set_mock_now_ms(None);
    }

    #[test]
    fn receipt_says_stopped_by_you() {
        assert_eq!(format_stopped_receipt(1_000, 43_000), "Stopped by you after 42s");

        let mut tab = spawn_looping_stub_tab(1, AgentCliPreset::Claude, false);
        tab.turn_started_at_ms = Some(1_000);
        assert!(start_stop_on_tab(&mut tab, 42_500));
        if let Ok(mut st) = tab.agent_status.lock() {
            *st = AgentTabStatus::Done;
        }
        let action = tick_stop_on_tab(&mut tab, "sess-receipt", 43_000);
        assert!(action.is_some());
        assert_eq!(tab.last_receipt.as_deref(), Some("Stopped by you after 42s"));
    }

    #[test]
    fn esc_in_the_terminal_reaches_the_cli_unchanged() {
        let mut state = TerminalState::default();
        let tab = spawn_looping_stub_tab(1, AgentCliPreset::Grok, true);
        state.tabs.push(tab);
        state.active_tab = 0;
        state.viewport_focused = true;
        state.chrome_focused = false;

        let stopped = handle_panel_escape(&mut state, false, 1_000);
        assert!(!stopped, "Esc inside the terminal must not trigger Stop");
        assert!(state.tabs[0].stop_state.is_none());
        assert_eq!(state.tabs[0].stop_sent_keys, vec![KEY_ESC], "0x1b must be written to the CLI unchanged");
    }

    #[test]
    fn esc_in_the_chrome_stops() {
        let mut state = TerminalState::default();
        let tab = spawn_looping_stub_tab(1, AgentCliPreset::Grok, true);
        state.tabs.push(tab);
        state.active_tab = 0;
        state.viewport_focused = false;
        state.chrome_focused = true;

        // Composer focused (§8.5): Esc leaves the field and does NOT stop.
        assert!(!handle_panel_escape(&mut state, true, 1_000));
        assert!(state.tabs[0].stop_state.is_none());

        // Chrome focused and composer not focused: Esc starts the stop sequence.
        assert!(handle_panel_escape(&mut state, false, 1_000));
        assert!(state.tabs[0].stop_state.is_some());
        assert_eq!(state.tabs[0].stop_sent_keys, vec![KEY_CTRL_C]);
    }

    #[test]
    fn stop_all_stops_every_running_tab() {
        let mut state = TerminalState::default();
        state.tabs.push(spawn_looping_stub_tab(1, AgentCliPreset::Claude, false));
        state.tabs.push(spawn_looping_stub_tab(2, AgentCliPreset::Grok, false));
        assert_eq!(running_tab_count(&state), 2);

        let count = stop_all(&mut state, 1_000);
        assert_eq!(count, 2);
        assert_eq!(state.tabs[0].stop_sent_keys, vec![KEY_ESC]);
        assert_eq!(state.tabs[1].stop_sent_keys, vec![KEY_CTRL_C]);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline && (state.tabs[0].is_alive() || state.tabs[1].is_alive()) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let actions = poll_stop_sequences(&mut state, 2_500);
        assert_eq!(actions.len(), 2);
        assert_eq!(state.tabs[0].agent_status.lock().unwrap().clone(), AgentTabStatus::Stopped);
        assert_eq!(state.tabs[1].agent_status.lock().unwrap().clone(), AgentTabStatus::Stopped);
    }
}
