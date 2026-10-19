//! "Check designs" panel: lists hard-coded values, contrast failures and detached instances, with a
//! one-click Fix per finding and Fix all. Clicking a row selects and frames the layer.

use crate::app::App;
use crate::check_designs::{can_revert_fix_row, AutoFixReport, Finding, FixRow, Report, Rule, MAX_AUTO_FIX_ROWS, MAX_SCANNED_LAYERS};
use crate::icons::{self, Icon};
use crate::terminal::{STATUS_BAD_TEXT, STATUS_INFO_TEXT, STATUS_OK_TEXT, STATUS_WARN_BG, STATUS_WARN_BORDER, STATUS_WARN_TEXT};
use crate::theme;
use crate::widgets;
use designos_doc::{AutoFixMode, AutoFixSettings, NodeId, SAFE_AUTO_FIX_RULES};
use egui::{Align2, Area, Color32, Context, Id, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

const PANEL_W: f32 = 320.0;
const PAD: f32 = theme::SPACE_SECTION;
const HEADER_H: f32 = 44.0;
const SUMMARY_H: f32 = 44.0;
const GROUP_H: f32 = theme::CONTROL_STANDARD;
/// A row is name + a message of at most two lines, so every row has the same height and a long
/// list only lays out the rows in view.
const ROW_H: f32 = 60.0;
const EMPTY_H: f32 = 112.0;
const NOTE_H: f32 = 44.0;
const BTN_H: f32 = theme::CONTROL_DENSE;
const FIX_W: f32 = 44.0;
const FIX_ALL_W: f32 = 64.0;
const ICON_COL: f32 = 24.0;
const MARGIN: f32 = theme::SPACE_SECTION;
const ONE: f32 = theme::BORDER_WIDTH;
const GAP: f32 = theme::SPACE_INLINE;
const FACT_GAP: f32 = theme::SPACE_FACT;
const SEG_INSET: f32 = theme::INSET_SEGMENT;
const ICON_SM: f32 = 12.0;
const CHECK_BOX_SIZE: f32 = 14.0;
const STATUS_PILL_W: f32 = 64.0;
const STATUS_PILL_H: f32 = 16.0;
const STATUS_PILL_OFFSET: f32 = 40.0;
const VALUE_COL_RIGHT_PAD: f32 = 32.0;
const LABEL_MAX_W: f32 = 148.0;
const DIFF_MAX_W: f32 = 108.0;
const SEG_COUNT: f32 = 3.0;
const CHEVRON_OFFSET: f32 = 6.0;
const TITLE_AFTER_CHEVRON: f32 = 16.0;
const CHECK_TEXT_OFFSET: f32 = 22.0;

/// What the user did this frame; applied after drawing so the document is not borrowed while painting.
enum Act {
    Close,
    FixAll,
    Fix(usize),
    Focus(NodeId),
    RevertAutoFix(usize),
    ToggleAutoFixSettings,
    SetFileMode(AutoFixMode),
    SetFileRule(&'static str, bool),
    SetUserMode(AutoFixMode),
    SetUserRule(&'static str, bool),
}

fn rule_icon(rule: Rule) -> (Icon, Color32) {
    match rule {
        Rule::HardCodedValue => (Icon::Variables, STATUS_WARN_TEXT),
        Rule::Contrast => (Icon::Eye, STATUS_BAD_TEXT),
        Rule::DetachedInstance => (Icon::Component, STATUS_INFO_TEXT),
    }
}

/// Width the message wraps at in a row, given whether the row has a Fix button.
fn message_width(has_fix: bool) -> f32 {
    PANEL_W - PAD - ICON_COL - PAD - if has_fix { FIX_W + GAP } else { 0.0 }
}

fn message_job(f: &Finding, color: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple(f.message.clone(), theme::font_body(), color, message_width(f.fix.is_some()));
    job.wrap.max_rows = 2;
    job
}

fn auto_fix_section_height(last: Option<&AutoFixReport>, settings_open: bool, settings_unreadable: bool) -> f32 {
    let rows_count = last.map_or(0, |r| r.rows.len());
    let shown_rows = rows_count.min(MAX_AUTO_FIX_ROWS);
    let rows_h = if rows_count == 0 {
        GROUP_H
    } else {
        let more_h = if rows_count > MAX_AUTO_FIX_ROWS { BTN_H } else { 0.0 };
        (shown_rows as f32) * GROUP_H + more_h
    };
    let rule_rows_h = (SAFE_AUTO_FIX_RULES.len() as f32) * BTN_H;
    let scope_block_h = BTN_H + GROUP_H + rule_rows_h;
    let unreadable_h = if settings_unreadable { BTN_H } else { 0.0 };
    let settings_body_h = if settings_open { unreadable_h + scope_block_h + scope_block_h + GAP } else { 0.0 };
    GROUP_H + BTN_H + rows_h + GROUP_H + settings_body_h
}

fn list_height(report: &Report, auto_fix_h: f32) -> f32 {
    let notes_h = (report.unreadable_libraries.len() as f32) * GROUP_H + if report.truncated { NOTE_H } else { 0.0 };
    let findings_h = if report.findings.is_empty() {
        EMPTY_H + notes_h
    } else {
        let groups = Rule::ALL.iter().filter(|r| report.of(**r).next().is_some()).count() as f32;
        groups * GROUP_H + report.findings.len() as f32 * ROW_H + notes_h
    };
    findings_h + auto_fix_h
}

pub fn show(app: &mut App, ctx: &Context) {
    if !app.check_designs.open {
        return;
    }
    // A drag or a scrub changes the document every frame; the list catches up when the pointer is released.
    let interacting = ctx.input(|i| i.pointer.any_down());
    app.refresh_check_designs(interacting);
    let report = std::mem::take(&mut app.check_designs.report);
    let scope_label = app.check_designs.scope_label.clone();
    let last_auto_fix = app.check_designs.last_auto_fix.clone();
    let settings_open = app.check_designs.settings_open;
    let settings_unreadable = app.check_designs.settings_unreadable;
    let scroll_to_auto_fix = std::mem::take(&mut app.check_designs.scroll_to_auto_fix);
    let effective_af = app.effective_auto_fix();
    let user_af = app.check_designs.user_auto_fix.clone();
    let auto_fix_h = auto_fix_section_height(last_auto_fix.as_ref(), settings_open, settings_unreadable);
    let canvas = app.canvas_rect;
    let max_h = (canvas.height() - 2.0 * MARGIN).max(HEADER_H + SUMMARY_H + 96.0);
    let list_h = list_height(&report, auto_fix_h).min(max_h - HEADER_H - SUMMARY_H);
    let size = Vec2::new(PANEL_W, HEADER_H + SUMMARY_H + list_h);
    let pos = Pos2::new((canvas.right() - PANEL_W - MARGIN).max(canvas.left() + MARGIN), canvas.top() + MARGIN);
    let mut act: Option<Act> = None;

    Area::new(Id::new("designos-check-designs")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(size, Sense::click());
        widgets::record(ui, "check-designs-panel", rect);
        let p = ui.painter().clone();
        widgets::elevation_400(&p, rect, theme::RADIUS_FRAME);
        p.rect_filled(rect, theme::cr(theme::RADIUS_FRAME), theme::BG_MENU);
        p.rect_stroke(rect, theme::cr(theme::RADIUS_FRAME), Stroke::new(1.0, theme::BORDER_MENU), StrokeKind::Inside);

        // Header: title and close.
        p.text(Pos2::new(rect.left() + PAD, rect.top() + HEADER_H / 2.0), Align2::LEFT_CENTER, "Check designs", theme::font_large_strong(), theme::TEXT);
        let close = Rect::from_center_size(Pos2::new(rect.right() - PAD - BTN_H / 2.0, rect.top() + HEADER_H / 2.0), Vec2::splat(BTN_H));
        widgets::record(ui, "check-designs-close", close);
        if widgets::icon_button(ui, close, "check-designs-close", Icon::Close, false, "Close").clicked() {
            act = Some(Act::Close);
        }
        widgets::hline(&p, rect.left(), rect.right(), rect.top() + HEADER_H, theme::BORDER_SUBTLE);

        // Summary: scope, counts, Fix all.
        let top = rect.top() + HEADER_H;
        let fixable = report.fixable().len();
        let counts = if report.findings.is_empty() {
            format!("No issues · {} layers checked", report.scanned)
        } else {
            let n = report.findings.len();
            format!("{n} {} · {} layers checked", if n == 1 { "issue" } else { "issues" }, report.scanned)
        };
        let text_w = PANEL_W - 2.0 * PAD - FIX_ALL_W - GAP;
        let scope_txt = widgets::elide(&p, &scope_label, &theme::font_body_medium(), text_w);
        p.text(Pos2::new(rect.left() + PAD, top + 14.0), Align2::LEFT_CENTER, scope_txt, theme::font_body_medium(), theme::TEXT);
        let counts_txt = widgets::elide(&p, &counts, &theme::font_body(), text_w);
        p.text(Pos2::new(rect.left() + PAD, top + 30.0), Align2::LEFT_CENTER, counts_txt, theme::font_body(), theme::TEXT_SECONDARY);
        let fix_all = Rect::from_min_size(Pos2::new(rect.right() - PAD - FIX_ALL_W, top + (SUMMARY_H - BTN_H) / 2.0), Vec2::new(FIX_ALL_W, BTN_H));
        widgets::record(ui, "check-designs-fix-all", fix_all);
        let resp = ui.interact(fix_all, Id::new("check-designs-fix-all"), if fixable > 0 { Sense::click() } else { Sense::hover() });
        let (bg, fg) = match (fixable > 0, resp.hovered()) {
            (true, true) => (theme::BG_BRAND_HOVER, Color32::WHITE),
            (true, false) => (theme::BG_BRAND, Color32::WHITE),
            (false, _) => (theme::BG_TERTIARY, theme::TEXT_DISABLED),
        };
        p.rect_filled(fix_all, theme::cr(theme::RADIUS_CONTROL), bg);
        p.text(fix_all.center(), Align2::CENTER_CENTER, "Fix all", theme::font_body_medium(), fg);
        if fixable > 0 && resp.on_hover_text(format!("Apply {fixable} fixes as one undo step")).clicked() {
            act = Some(Act::FixAll);
        }
        widgets::hline(&p, rect.left(), rect.right(), top + SUMMARY_H, theme::BORDER_SUBTLE);

        // List.
        let list_rect = Rect::from_min_size(Pos2::new(rect.left(), top + SUMMARY_H), Vec2::new(PANEL_W, list_h));
        ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
            egui::ScrollArea::vertical().id_salt("check-designs-list").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(PANEL_W);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                if let Some(a) =
                    auto_fix_section(ui, &app.doc, last_auto_fix.as_ref(), settings_open, settings_unreadable, &effective_af, &user_af, scroll_to_auto_fix)
                {
                    act = Some(a);
                }
                for lib_name in &report.unreadable_libraries {
                    unreadable_library_note(ui, lib_name);
                }
                if report.findings.is_empty() {
                    empty_state(ui, report.scanned);
                }
                for rule in Rule::ALL {
                    let items: Vec<(usize, &Finding)> = report.findings.iter().enumerate().filter(|(_, f)| f.rule == rule).collect();
                    if items.is_empty() {
                        continue;
                    }
                    group_header(ui, rule, items.len());
                    for (idx, f) in items {
                        if let Some(a) = row(ui, idx, f) {
                            act = Some(a);
                        }
                    }
                }
                if report.truncated {
                    truncation_note(ui);
                }
            });
        });
    });

    match act {
        Some(Act::Close) => app.check_designs.open = false,
        Some(Act::Focus(id)) => {
            if app.doc.get(id).is_some() {
                app.select_only(id);
                app.fit_selection();
            }
        }
        Some(Act::Fix(i)) => {
            if let Some(f) = report.findings.get(i) {
                if app.check_designs_fix(f) {
                    app.show_toast(format!("{} · Cmd+Z to undo", f.fix_label), false);
                } else {
                    app.show_toast("That layer changed since the check. The list is refreshed.", true);
                }
                app.check_designs.invalidate();
            }
        }
        Some(Act::FixAll) => {
            let n = app.check_designs_fix_all(&report.findings);
            app.show_toast(if n == 0 { "Nothing could be fixed".to_string() } else { format!("Fixed {n} in one step · Cmd+Z to undo") }, n == 0);
            app.check_designs.invalidate();
        }
        Some(Act::RevertAutoFix(i)) => {
            if app.revert_auto_fix_row(i) {
                app.show_toast("Reverted auto-fix · Cmd+Z to undo", false);
                app.check_designs.invalidate();
            } else {
                app.show_toast("Changed since the fix", true);
            }
        }
        Some(Act::ToggleAutoFixSettings) => {
            app.check_designs.settings_open = !app.check_designs.settings_open;
            app.check_designs.scroll_to_auto_fix = app.check_designs.settings_open;
        }
        Some(Act::SetFileMode(mode)) => {
            app.set_file_auto_fix_mode(mode);
        }
        Some(Act::SetFileRule(rule, enabled)) => {
            app.set_file_auto_fix_rule(rule, enabled);
        }
        Some(Act::SetUserMode(mode)) => {
            app.set_user_auto_fix_mode(mode);
        }
        Some(Act::SetUserRule(rule, enabled)) => {
            app.set_user_auto_fix_rule(rule, enabled);
        }
        None => {}
    }
    app.check_designs.report = report;
}

fn group_header(ui: &mut Ui, rule: Rule, count: usize) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, GROUP_H), Sense::hover());
    let p = ui.painter();
    let title = rule.title();
    let font = theme::font_body_strong();
    p.text(Pos2::new(rect.left() + PAD, rect.center().y), Align2::LEFT_CENTER, title, font.clone(), theme::TEXT_SECONDARY);
    let tw = widgets::text_width(p, title, &font);
    let badge = Rect::from_center_size(Pos2::new(rect.left() + PAD + tw + 8.0 + 10.0, rect.center().y), Vec2::new(20.0, 16.0));
    p.rect_filled(badge, theme::cr(theme::RADIUS_FRAME), theme::BG_TERTIARY);
    p.text(badge.center(), Align2::CENTER_CENTER, count.to_string(), theme::font_caption_medium(), theme::TEXT);
}

fn row(ui: &mut Ui, idx: usize, f: &Finding) -> Option<Act> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, ROW_H), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    widgets::record(ui, &format!("check-designs-row-{idx}"), rect);
    let row_resp = ui.interact(rect, Id::new(("check-designs-row", idx)), Sense::click());
    let p = ui.painter().clone();
    if row_resp.hovered() {
        p.rect_filled(rect, 0.0, theme::BG_HOVER);
    }
    let (icon, tint) = rule_icon(f.rule);
    icons::paint(&p, icon, Pos2::new(rect.left() + PAD + 8.0, rect.top() + 16.0), 14.0, tint);
    let text_x = rect.left() + PAD + ICON_COL;
    let has_fix = f.fix.is_some();
    let from_lib = f.is_from_library();
    let badge_w = if from_lib { 48.0 } else { 0.0 };
    let name = widgets::elide(&p, &f.node_name, &theme::font_body_medium(), (message_width(has_fix) - badge_w).max(40.0));
    p.text(Pos2::new(text_x, rect.top() + 16.0), Align2::LEFT_CENTER, &name, theme::font_body_medium(), theme::TEXT);
    if from_lib {
        let nw = widgets::text_width(&p, &name, &theme::font_body_medium());
        let badge = Rect::from_min_size(Pos2::new(text_x + nw + 6.0, rect.top() + 8.0), Vec2::new(42.0, 16.0));
        p.rect_filled(badge, theme::cr(theme::RADIUS_MARK), theme::BG_TERTIARY);
        p.text(badge.center(), Align2::CENTER_CENTER, "Library", theme::font_caption(), theme::TEXT_SECONDARY);
    }
    let galley = p.layout_job(message_job(f, theme::TEXT_SECONDARY));
    p.galley(Pos2::new(text_x, rect.top() + 26.0), galley, theme::TEXT_SECONDARY);
    widgets::hline(&p, rect.left() + PAD, rect.right() - PAD, rect.bottom() - 1.0, theme::BORDER_SUBTLE);

    let mut act = row_resp.clicked().then_some(Act::Focus(f.node));
    if has_fix {
        let btn = Rect::from_min_size(Pos2::new(rect.right() - PAD - FIX_W, rect.top() + 8.0), Vec2::new(FIX_W, BTN_H));
        widgets::record(ui, &format!("check-designs-fix-{idx}"), btn);
        let resp = ui.interact(btn, Id::new(("check-designs-fix", idx)), Sense::click());
        p.rect_filled(btn, theme::cr(theme::RADIUS_CONTROL), if resp.hovered() { theme::BG_PRESSED } else { theme::BG_TERTIARY });
        p.text(btn.center(), Align2::CENTER_CENTER, "Fix", theme::font_body_medium(), theme::TEXT);
        if resp.on_hover_text(f.fix_label.clone()).clicked() {
            act = Some(Act::Fix(idx));
        }
    }
    act
}

fn unreadable_library_note(ui: &mut Ui, name: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, 28.0), Sense::hover());
    widgets::record(ui, "check-designs-unreadable-library", rect);
    let p = ui.painter();
    let note = format!("Library \"{name}\" couldn't be read; its suggestions are missing");
    let txt = widgets::elide(p, &note, &theme::font_caption(), PANEL_W - 2.0 * PAD);
    p.text(Pos2::new(rect.left() + PAD, rect.center().y), Align2::LEFT_CENTER, txt, theme::font_caption(), theme::TEXT_SECONDARY);
    widgets::hline(p, rect.left() + PAD, rect.right() - PAD, rect.bottom() - 1.0, theme::BORDER_SUBTLE);
}

fn empty_state(ui: &mut Ui, scanned: usize) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, EMPTY_H), Sense::hover());
    let p = ui.painter();
    icons::paint(p, Icon::Check, Pos2::new(rect.center().x, rect.top() + 28.0), 20.0, STATUS_OK_TEXT);
    p.text(Pos2::new(rect.center().x, rect.top() + 52.0), Align2::CENTER_CENTER, "No issues found", theme::font_body_strong(), theme::TEXT);
    let note = format!("Hard-coded values, contrast and detached instances were checked on {scanned} layers.");
    let galley = p.layout(note, theme::font_body(), theme::TEXT_SECONDARY, PANEL_W - 2.0 * PAD - 24.0);
    p.galley(Pos2::new(rect.center().x - galley.size().x / 2.0, rect.top() + 66.0), galley, theme::TEXT_SECONDARY);
}

fn truncation_note(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, NOTE_H), Sense::hover());
    let p = ui.painter();
    let inner = rect.shrink2(Vec2::new(PAD, 6.0));
    p.rect_filled(inner, theme::cr(theme::RADIUS_CONTROL), STATUS_WARN_BG);
    p.rect_stroke(inner, theme::cr(theme::RADIUS_CONTROL), Stroke::new(1.0, STATUS_WARN_BORDER), StrokeKind::Inside);
    let note = format!("Stopped after {MAX_SCANNED_LAYERS} layers. Select fewer layers to check the rest.");
    let galley = p.layout(note, theme::font_body(), STATUS_WARN_TEXT, inner.width() - theme::SPACE_BLOCK);
    p.galley(Pos2::new(inner.left() + theme::SPACE_INLINE, inner.center().y - galley.size().y / 2.0), galley, STATUS_WARN_TEXT);
}

#[allow(clippy::too_many_arguments)]
fn auto_fix_section(
    ui: &mut Ui,
    doc: &designos_doc::Document,
    last: Option<&AutoFixReport>,
    settings_open: bool,
    settings_unreadable: bool,
    effective: &AutoFixSettings,
    user: &AutoFixSettings,
    scroll_to_section: bool,
) -> Option<Act> {
    let mut act = None;
    auto_fix_header(ui, last, scroll_to_section);
    auto_fix_subheader(ui, last);

    match last {
        None => auto_fix_empty_row(ui, "No auto-fix passes yet"),
        Some(rep) if rep.rows.is_empty() => auto_fix_empty_row(ui, "No issues found"),
        Some(rep) => {
            let shown = rep.rows.len().min(MAX_AUTO_FIX_ROWS);
            let applied = !rep.report_only;
            for (idx, fix_row) in rep.rows.iter().take(shown).enumerate() {
                if let Some(a) = auto_fix_diff_row(ui, doc, idx, fix_row, applied) {
                    act = Some(a);
                }
            }
            if rep.rows.len() > MAX_AUTO_FIX_ROWS {
                let extra = rep.rows.len() - MAX_AUTO_FIX_ROWS;
                auto_fix_more_footer(ui, extra);
            }
        }
    }

    if auto_fix_settings_header(ui, settings_open, effective.mode) {
        act = Some(Act::ToggleAutoFixSettings);
    }

    if settings_open {
        if settings_unreadable {
            auto_fix_unreadable_notice(ui);
        }
        auto_fix_scope_label(ui, "This file");
        if let Some(mode) = auto_fix_mode_segmented(ui, "file", effective.mode, true) {
            act = Some(Act::SetFileMode(mode));
        }
        for rule in SAFE_AUTO_FIX_RULES {
            if let Some(enabled) = auto_fix_rule_checkbox(ui, "file", rule, effective.rule_enabled(rule), true) {
                act = Some(Act::SetFileRule(rule, enabled));
            }
        }

        auto_fix_scope_label(ui, "My default");
        if let Some(mode) = auto_fix_mode_segmented(ui, "user", user.mode, false) {
            act = Some(Act::SetUserMode(mode));
        }
        for rule in SAFE_AUTO_FIX_RULES {
            if let Some(enabled) = auto_fix_rule_checkbox(ui, "user", rule, user.rule_enabled(rule), false) {
                act = Some(Act::SetUserRule(rule, enabled));
            }
        }
    }

    act
}

fn mode_label(mode: AutoFixMode) -> &'static str {
    match mode {
        AutoFixMode::On => "On",
        AutoFixMode::ReportOnly => "Report only",
        AutoFixMode::Off => "Off",
    }
}

fn auto_fix_header(ui: &mut Ui, last: Option<&AutoFixReport>, scroll_to_section: bool) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(PANEL_W, GROUP_H), Sense::hover());
    if scroll_to_section {
        resp.scroll_to_me(Some(egui::Align::TOP));
    }
    widgets::record(ui, "check-designs-auto-fix-header", rect);
    let p = ui.painter();
    widgets::hline(p, rect.left(), rect.right(), rect.top(), theme::BORDER_SUBTLE);
    let title = "Auto-fix";
    let font = theme::font_body_strong();
    p.text(Pos2::new(rect.left() + PAD, rect.center().y), Align2::LEFT_CENTER, title, font.clone(), theme::TEXT_SECONDARY);
    let count = last.map_or(0, |r| r.rows.len());
    let tw = widgets::text_width(p, title, &font);
    let badge_x = rect.left() + PAD + tw + GAP;
    let badge = Rect::from_min_size(Pos2::new(badge_x, rect.center().y - GAP), Vec2::new(ICON_COL, PAD));
    widgets::record(ui, "check-designs-auto-fix-count", badge);
    p.rect_filled(badge, theme::cr(theme::RADIUS_FRAME), theme::BG_TERTIARY);
    p.text(badge.center(), Align2::CENTER_CENTER, count.to_string(), theme::font_caption_medium(), theme::TEXT);
}

fn auto_fix_subheader(ui: &mut Ui, last: Option<&AutoFixReport>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, BTN_H), Sense::hover());
    let p = ui.painter();
    let label = match last {
        Some(rep) if rep.report_only => "Fixes available",
        _ => "Last agent pass",
    };
    p.text(Pos2::new(rect.left() + PAD, rect.center().y), Align2::LEFT_CENTER, label, theme::font_caption_medium(), theme::TEXT_SECONDARY);
}

fn auto_fix_empty_row(ui: &mut Ui, msg: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, GROUP_H), Sense::hover());
    widgets::record(ui, "check-designs-auto-fix-empty", rect);
    let p = ui.painter();
    p.text(Pos2::new(rect.left() + PAD, rect.center().y), Align2::LEFT_CENTER, msg, theme::font_body(), theme::TEXT_SECONDARY);
    widgets::hline(p, rect.left() + PAD, rect.right() - PAD, rect.bottom() - ONE, theme::BORDER_SUBTLE);
}

fn auto_fix_diff_row(ui: &mut Ui, doc: &designos_doc::Document, idx: usize, fix_row: &FixRow, applied: bool) -> Option<Act> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, GROUP_H), Sense::hover());
    widgets::record(ui, &format!("check-designs-auto-fix-row-{idx}"), rect);
    if idx == 0 {
        widgets::record(ui, "check-designs-auto-fix-row", rect);
    }
    let row_resp = ui.interact(rect, Id::new(("check-designs-auto-fix-row", idx)), Sense::click());
    let p = ui.painter().clone();
    if row_resp.hovered() {
        p.rect_filled(rect, 0.0, theme::BG_HOVER);
    }

    let node_name = doc.get(fix_row.node).map_or_else(
        || {
            if fix_row.node_name.is_empty() {
                format!("#{}", fix_row.node.0)
            } else {
                fix_row.node_name.clone()
            }
        },
        |n| n.name.clone(),
    );
    let label = format!("{} · {node_name}", fix_row.rule);
    let elided_label = widgets::elide(&p, &label, &theme::font_caption_medium(), LABEL_MAX_W);
    p.text(Pos2::new(rect.left() + PAD, rect.center().y), Align2::LEFT_CENTER, elided_label, theme::font_caption_medium(), theme::TEXT);

    let before_str = format_fix_value(&fix_row.before);
    let after_str = format_fix_value(&fix_row.after);
    let diff_str = format!("{before_str} → {after_str}");
    let elided_diff = widgets::elide(&p, &diff_str, &theme::font_caption(), DIFF_MAX_W);
    let diff_right = if applied { rect.right() - PAD - VALUE_COL_RIGHT_PAD } else { rect.right() - PAD };
    p.text(Pos2::new(diff_right, rect.center().y), Align2::RIGHT_CENTER, elided_diff, theme::font_caption(), theme::TEXT_SECONDARY);

    let mut act = row_resp.clicked().then_some(Act::Focus(fix_row.node));

    if applied {
        let can_revert = can_revert_fix_row(doc, fix_row);
        let btn = Rect::from_min_size(Pos2::new(rect.right() - PAD - ICON_COL, rect.center().y - ICON_SM), Vec2::new(ICON_COL, ICON_COL));
        widgets::record(ui, &format!("check-designs-auto-fix-revert-{idx}"), btn);
        if idx == 0 {
            widgets::record(ui, "check-designs-auto-fix-revert", btn);
        }
        let btn_resp = ui.interact(btn, Id::new(("check-designs-auto-fix-revert", idx)), Sense::click());
        if btn_resp.hovered() && can_revert {
            p.rect_filled(btn, theme::cr(theme::RADIUS_MARK), theme::BG_TERTIARY);
        }
        let tint = if can_revert { theme::TEXT } else { theme::TEXT_DISABLED };
        icons::paint(&p, Icon::Rotate90, btn.center(), ICON_SM, tint);
        let tip = if can_revert { "Revert this fix" } else { "Changed since the fix" };
        if btn_resp.on_hover_text(tip).clicked() && can_revert {
            act = Some(Act::RevertAutoFix(idx));
        }
    }

    widgets::hline(&p, rect.left() + PAD, rect.right() - PAD, rect.bottom() - ONE, theme::BORDER_SUBTLE);
    act
}

fn auto_fix_more_footer(ui: &mut Ui, extra: usize) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, BTN_H), Sense::hover());
    widgets::record(ui, "check-designs-auto-fix-more", rect);
    let p = ui.painter();
    let msg = format!("+{extra} more");
    p.text(Pos2::new(rect.left() + PAD, rect.center().y), Align2::LEFT_CENTER, msg, theme::font_caption(), theme::TEXT_SECONDARY);
}

fn auto_fix_settings_header(ui: &mut Ui, settings_open: bool, mode: AutoFixMode) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, GROUP_H), Sense::hover());
    widgets::record(ui, "check-designs-auto-fix-settings-toggle", rect);
    let resp = ui.interact(rect, Id::new("check-designs-auto-fix-settings-toggle"), Sense::click());
    let p = ui.painter().clone();
    if resp.hovered() {
        p.rect_filled(rect, 0.0, theme::BG_HOVER);
    }
    let chev = if settings_open { Icon::ChevronDown } else { Icon::ChevronRight };
    icons::paint(&p, chev, Pos2::new(rect.left() + PAD + CHEVRON_OFFSET, rect.center().y), ICON_SM, theme::TEXT_SECONDARY);
    p.text(
        Pos2::new(rect.left() + PAD + TITLE_AFTER_CHEVRON, rect.center().y),
        Align2::LEFT_CENTER,
        "Auto-fix settings",
        theme::font_body_medium(),
        theme::TEXT,
    );
    let pill = Rect::from_center_size(Pos2::new(rect.right() - PAD - STATUS_PILL_OFFSET, rect.center().y), Vec2::new(STATUS_PILL_W, STATUS_PILL_H));
    p.rect_filled(pill, theme::cr(theme::RADIUS_MARK), theme::BG_TERTIARY);
    p.text(pill.center(), Align2::CENTER_CENTER, mode_label(mode), theme::font_caption(), theme::TEXT_SECONDARY);
    resp.clicked()
}

fn auto_fix_unreadable_notice(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, BTN_H), Sense::hover());
    widgets::record(ui, "check-designs-auto-fix-settings-notice", rect);
    let p = ui.painter();
    p.text(
        Pos2::new(rect.left() + PAD, rect.center().y),
        Align2::LEFT_CENTER,
        "Couldn't read settings.json — using defaults.",
        theme::font_caption(),
        STATUS_WARN_TEXT,
    );
}

fn auto_fix_scope_label(ui: &mut Ui, title: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, BTN_H), Sense::hover());
    let p = ui.painter();
    p.text(Pos2::new(rect.left() + PAD, rect.center().y), Align2::LEFT_CENTER, title, theme::font_caption_medium(), theme::TEXT_SECONDARY);
}

fn auto_fix_mode_segmented(ui: &mut Ui, scope: &str, active: AutoFixMode, alias_default_ids: bool) -> Option<AutoFixMode> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, GROUP_H), Sense::hover());
    let track = Rect::from_min_max(Pos2::new(rect.left() + PAD, rect.top() + FACT_GAP), Pos2::new(rect.right() - PAD, rect.bottom() - FACT_GAP));
    ui.painter().rect_filled(track, theme::cr(theme::RADIUS_CONTROL), theme::BG_TERTIARY);
    let inner = track.shrink(SEG_INSET);
    let seg_w = inner.width() / SEG_COUNT;
    let modes = [(AutoFixMode::On, "on"), (AutoFixMode::ReportOnly, "report-only"), (AutoFixMode::Off, "off")];
    let mut chosen = None;
    for (i, (mode, slug)) in modes.iter().enumerate() {
        let x0 = inner.left() + (i as f32) * seg_w;
        let seg_rect = Rect::from_min_size(Pos2::new(x0, inner.top()), Vec2::new(seg_w, inner.height()));
        if paint_mode_segment(ui, scope, slug, *mode, active == *mode, seg_rect, alias_default_ids) {
            chosen = Some(*mode);
        }
    }
    chosen
}

fn paint_mode_segment(ui: &mut Ui, scope: &str, slug: &str, mode: AutoFixMode, is_active: bool, seg_rect: Rect, alias_default_ids: bool) -> bool {
    let id_str = format!("check-designs-auto-fix-{scope}-mode-{slug}");
    widgets::record(ui, &id_str, seg_rect);
    if alias_default_ids {
        widgets::record(ui, &format!("check-designs-auto-fix-mode-{slug}"), seg_rect);
    }
    let resp = ui.interact(seg_rect, Id::new(id_str), Sense::click());
    let p = ui.painter();
    if is_active {
        p.rect_filled(seg_rect, theme::cr(theme::RADIUS_SEGMENT), theme::BG_ELEVATED);
    }
    let fg = if is_active { theme::TEXT } else { theme::TEXT_SECONDARY };
    p.text(seg_rect.center(), Align2::CENTER_CENTER, mode_label(mode), theme::font_caption_medium(), fg);
    resp.clicked() && !is_active
}

fn auto_fix_rule_checkbox(ui: &mut Ui, scope: &str, rule: &'static str, enabled: bool, alias_default_ids: bool) -> Option<bool> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(PANEL_W, BTN_H), Sense::hover());
    let id_str = format!("check-designs-auto-fix-{scope}-rule-{rule}");
    widgets::record(ui, &id_str, rect);
    if alias_default_ids {
        widgets::record(ui, &format!("check-designs-auto-fix-rule-{rule}"), rect);
    }
    let resp = ui.interact(rect, Id::new(id_str), Sense::click());
    let p = ui.painter().clone();
    if resp.hovered() {
        p.rect_filled(rect, 0.0, theme::BG_HOVER);
    }
    let box_rect = Rect::from_center_size(Pos2::new(rect.left() + PAD + CHEVRON_OFFSET, rect.center().y), Vec2::new(CHECK_BOX_SIZE, CHECK_BOX_SIZE));
    let fill = if enabled { theme::BG_BRAND } else { theme::BG_TERTIARY };
    p.rect_filled(box_rect, theme::cr(theme::RADIUS_MARK), fill);
    if enabled {
        icons::paint(&p, Icon::Check, box_rect.center(), ICON_SM, Color32::WHITE);
    }
    p.text(Pos2::new(rect.left() + PAD + CHECK_TEXT_OFFSET, rect.center().y), Align2::LEFT_CENTER, rule, theme::font_caption(), theme::TEXT);
    resp.clicked().then_some(!enabled)
}

fn format_fix_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if (f.fract()).abs() < f64::EPSILON {
                    format!("{}px", f as i64)
                } else {
                    format!("{f:.1}px")
                }
            } else {
                n.to_string()
            }
        }
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "none".to_string(),
        other => other.to_string(),
    }
}
