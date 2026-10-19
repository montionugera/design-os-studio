//! Agents panel empty-state sample prompts, context selector, attach hint line, and no-CLI
//! install guidance (`#688`).
//!
//! Prompts live in `agent_sample_prompts.toml` (compiled in via `include_str!`). Clicking a
//! sample prompt appends its text to the composer and focuses the input without sending, mutating
//! the document, or writing to any PTY.

use std::sync::OnceLock;

use designos_doc::{Document, NodeId, NodeKind};
use egui::{Align2, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use serde::Deserialize;

use crate::terminal::AgentCliPreset;
use crate::{agent_start_failure, theme, widgets};

pub const SHIPPED_SAMPLE_PROMPTS_TOML: &str = include_str!("agent_sample_prompts.toml");
pub const MAX_PROMPTS_PER_CONTEXT: usize = 6;
pub const MAX_PROMPT_TEXT_CHARS: usize = 300;

/// Document/selection context that selects which starter prompts appear in the Agents empty state (`#688` §3 rule 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum PromptContext {
    /// No page of the document has any child layer.
    EmptyFile,
    /// Selection holds at least one Frame, Component, ComponentSet, Instance, or Section.
    Frame,
    /// Everything else, including an empty selection on a non-empty file or a selection of leaf layers.
    #[default]
    NoSelection,
}

impl PromptContext {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmptyFile => "empty_file",
            Self::Frame => "frame",
            Self::NoSelection => "no_selection",
        }
    }
}

/// Summary of the open document and selection passed to the Agents panel painter (`#688` §5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanelContext {
    pub page_empty: bool,
    pub selection_kind: Option<PromptContext>,
}

impl PanelContext {
    /// Computes the panel summary in O(pages + selection.len()) without walking any layer subtree (`#688` §11).
    pub fn from_doc(doc: &Document, page: NodeId, selection: &[NodeId]) -> Self {
        let page_empty = if doc.pages.is_empty() {
            doc.get(page).map(|n| n.children.is_empty()).unwrap_or(true)
        } else {
            doc.pages.iter().all(|&pid| doc.get(pid).map(|n| n.children.is_empty()).unwrap_or(true))
        };
        let has_frame = selection.iter().any(|&id| {
            doc.get(id).is_some_and(|n| {
                matches!(n.kind, NodeKind::Frame(_) | NodeKind::Component(_) | NodeKind::ComponentSet(_) | NodeKind::Instance { .. } | NodeKind::Section)
            })
        });
        Self { page_empty, selection_kind: if has_frame { Some(PromptContext::Frame) } else { None } }
    }

    /// Resolves the [`PromptContext`] in strict priority order: `EmptyFile` -> `Frame` -> `NoSelection` (`#688` §3 rule 1).
    pub fn resolve(self) -> PromptContext {
        if self.page_empty {
            PromptContext::EmptyFile
        } else if self.selection_kind == Some(PromptContext::Frame) {
            PromptContext::Frame
        } else {
            PromptContext::NoSelection
        }
    }
}

/// Pure context selector over `(doc, page, selection)` (`#688` §3 rule 1, §5).
pub fn prompt_context(doc: &Document, page: NodeId, selection: &[NodeId]) -> PromptContext {
    PanelContext::from_doc(doc, page, selection).resolve()
}

/// One validated starter prompt entry loaded from `agent_sample_prompts.toml` (`#688` §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamplePrompt {
    pub context: PromptContext,
    pub label: String,
    pub text: String,
}

/// Validated starter prompts grouped by [`PromptContext`] in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SamplePromptCatalog {
    pub empty_file: Vec<SamplePrompt>,
    pub frame: Vec<SamplePrompt>,
    pub no_selection: Vec<SamplePrompt>,
}

impl SamplePromptCatalog {
    pub fn for_context(&self, ctx: PromptContext) -> &[SamplePrompt] {
        match ctx {
            PromptContext::EmptyFile => &self.empty_file,
            PromptContext::Frame => &self.frame,
            PromptContext::NoSelection => &self.no_selection,
        }
    }
}

/// Validation failure returned by [`parse_sample_prompts`] (`#688` §3 rule 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptLoadError {
    TomlSyntax(String),
    UnknownContext(String),
    TooManyForContext { context: PromptContext, count: usize },
    EmptyLabel,
    EmptyText,
    TextTooLong { len: usize },
    ForbiddenPathReference(String),
}

#[derive(Deserialize)]
struct RawPromptFile {
    #[serde(default, alias = "prompt")]
    prompts: Vec<RawSamplePrompt>,
}

#[derive(Deserialize)]
struct RawSamplePrompt {
    context: String,
    label: String,
    text: String,
}

fn contains_forbidden_path(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    if lower.contains("plans/") || lower.contains(".md") {
        return true;
    }
    s.split_whitespace().any(|word| {
        let trimmed = word.trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':'));
        trimmed.starts_with('/') || trimmed.starts_with('\\')
    })
}

/// Parses and validates a TOML sample-prompt catalogue (`#688` §3 rule 4).
pub fn parse_sample_prompts(toml_src: &str) -> Result<SamplePromptCatalog, PromptLoadError> {
    let parsed: RawPromptFile = toml::from_str(toml_src).map_err(|e| PromptLoadError::TomlSyntax(e.to_string()))?;
    let mut catalog = SamplePromptCatalog::default();

    for raw in parsed.prompts {
        let context = match raw.context.trim() {
            "empty_file" => PromptContext::EmptyFile,
            "frame" => PromptContext::Frame,
            "no_selection" => PromptContext::NoSelection,
            other => return Err(PromptLoadError::UnknownContext(other.to_string())),
        };
        let label = raw.label.trim().to_string();
        if label.is_empty() {
            return Err(PromptLoadError::EmptyLabel);
        }
        let text = raw.text.trim().to_string();
        if text.is_empty() {
            return Err(PromptLoadError::EmptyText);
        }
        let char_len = text.chars().count();
        if char_len > MAX_PROMPT_TEXT_CHARS {
            return Err(PromptLoadError::TextTooLong { len: char_len });
        }
        if contains_forbidden_path(&label) || contains_forbidden_path(&text) {
            return Err(PromptLoadError::ForbiddenPathReference(text));
        }

        let bucket = match context {
            PromptContext::EmptyFile => &mut catalog.empty_file,
            PromptContext::Frame => &mut catalog.frame,
            PromptContext::NoSelection => &mut catalog.no_selection,
        };
        bucket.push(SamplePrompt { context, label, text });
        if bucket.len() > MAX_PROMPTS_PER_CONTEXT {
            return Err(PromptLoadError::TooManyForContext { context, count: bucket.len() });
        }
    }

    Ok(catalog)
}

/// Returns the shipped sample prompts for `ctx` in file order (`#688` §5).
pub fn sample_prompts(ctx: PromptContext) -> &'static [SamplePrompt] {
    static CATALOG: OnceLock<SamplePromptCatalog> = OnceLock::new();
    let cat = CATALOG.get_or_init(|| parse_sample_prompts(SHIPPED_SAMPLE_PROMPTS_TOML).expect("shipped agent_sample_prompts.toml must be valid"));
    cat.for_context(ctx)
}

/// Appends `prompt_text` into the composer draft (`#688` §3 rule 3, §8 edge cases 4–5):
/// replaces an empty draft, or appends after a single space when the draft already holds text.
pub fn apply_sample_prompt_to_draft(draft: &mut String, prompt_text: &str) {
    let clean = prompt_text.trim();
    if clean.is_empty() {
        return;
    }
    if draft.trim().is_empty() {
        *draft = clean.to_string();
    } else if draft.ends_with(char::is_whitespace) {
        draft.push_str(clean);
    } else {
        draft.push(' ');
        draft.push_str(clean);
    }
}

/// Shipped feature flags that govern the attach-hint line under the sample prompts (`#688` §3 rule 5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AttachHintCapabilities {
    /// True when `#654` (selection chip in composer) is present.
    pub selection_chip: bool,
    /// True when `#655` (`@`-mention layer completion) is present.
    pub at_mentions: bool,
}

impl AttachHintCapabilities {
    /// Neither `#654` nor `#655` is on `main` yet (`#688` §2, §3 rule 5).
    pub const CURRENT: Self = Self { selection_chip: false, at_mentions: false };
}

/// Builds the single attach-hint line under the sample prompts from existing features only (`#688` §3 rule 5).
pub fn attach_hint_line(caps: AttachHintCapabilities) -> Option<String> {
    match (caps.selection_chip, caps.at_mentions) {
        (false, false) => None,
        (true, false) => Some("Click the selection chip to send what is selected.".to_string()),
        (false, true) => Some("Type @ to mention layers.".to_string()),
        (true, true) => Some("Click the selection chip to send what is selected. Type @ to mention layers.".to_string()),
    }
}

/// Renders the `"What's next?"` empty state inside `center_rect` using Studio theme tokens (`#688` §7).
/// Returns `true` when a sample prompt row was clicked so the caller can focus the composer input.
pub fn paint_empty_state(
    ui: &mut Ui,
    center_rect: Rect,
    ctx: PromptContext,
    any_cli_installed: bool,
    input_line: &mut String,
    viewport_focused: &mut bool,
) -> bool {
    let p = ui.painter().clone();
    widgets::record(ui, &format!("agents-empty-context-{}", ctx.as_str()), center_rect);
    let heading_y = center_rect.top() + theme::SPACE_SECTION;
    p.text(Pos2::new(center_rect.center().x, heading_y), Align2::CENTER_CENTER, "What's next?", theme::font_large_strong(), theme::TEXT);

    let row_x = center_rect.left() + theme::INSET_PANEL;
    let row_w = (center_rect.width() - 2.0 * theme::INSET_PANEL).max(theme::TARGET_MIN);
    let mut y = heading_y + theme::SPACE_SECTION;

    if !any_cli_installed {
        let msg_elided = widgets::elide(&p, "No agent CLI found", &theme::font_body_medium(), row_w);
        let msg_rect = Rect::from_min_size(Pos2::new(row_x, y), Vec2::new(row_w, theme::LINE_BODY));
        p.text(Pos2::new(row_x, msg_rect.center().y), Align2::LEFT_CENTER, msg_elided, theme::font_body_medium(), theme::TEXT_SECONDARY);
        widgets::record(ui, "agents-no-cli-line", msg_rect);
        y = msg_rect.bottom() + theme::SPACE_GROUP;

        for (idx, (id, preset)) in [
            ("agents-install-claude", AgentCliPreset::Claude),
            ("agents-install-codex", AgentCliPreset::Codex),
            ("agents-install-agy", AgentCliPreset::Antigravity),
            ("agents-install-grok", AgentCliPreset::Grok),
        ]
        .into_iter()
        .enumerate()
        {
            let btn_rect = Rect::from_min_size(Pos2::new(row_x, y), Vec2::new(row_w, theme::CONTROL_STANDARD));
            let resp = ui.interact(btn_rect, Id::new(id), Sense::click());
            let fill = if resp.hovered() { theme::BG_HOVER } else { theme::BG_SECONDARY };
            p.rect_filled(btn_rect, theme::cr(theme::RADIUS_CONTROL), fill);
            p.rect_stroke(btn_rect, theme::cr(theme::RADIUS_CONTROL), Stroke::new(theme::BORDER_WIDTH, theme::BORDER), StrokeKind::Inside);

            let text_max_w = (row_w - 2.0 * theme::INSET_BUTTON).max(theme::TARGET_MIN);
            let label = format!("Install {}", preset.tab_title());
            let elided = widgets::elide(&p, &label, &theme::font_body_medium(), text_max_w);
            p.text(Pos2::new(btn_rect.left() + theme::INSET_BUTTON, btn_rect.center().y), Align2::LEFT_CENTER, elided, theme::font_body_medium(), theme::TEXT);
            let url = agent_start_failure::install_url(preset);
            let resp = resp.on_hover_text(url);
            widgets::record(ui, id, btn_rect);
            widgets::record(ui, &format!("agents-install-guidance-{idx}"), btn_rect);
            if resp.clicked() {
                let _ = crate::home::open_link(url);
            }
            y += theme::CONTROL_STANDARD + theme::SPACE_ROW;
        }
        return false;
    }

    let mut prompt_clicked = false;
    let prompts = sample_prompts(ctx);
    for (idx, prompt) in prompts.iter().take(MAX_PROMPTS_PER_CONTEXT).enumerate() {
        let row_rect = Rect::from_min_size(Pos2::new(row_x, y), Vec2::new(row_w, theme::CONTROL_ROW));
        let row_id = format!("agents-sample-prompt-{idx}");
        let resp = ui.interact(row_rect, Id::new(&row_id), Sense::click());
        let fill = if resp.hovered() { theme::BG_HOVER } else { theme::BG_SECONDARY };
        p.rect_filled(row_rect, theme::cr(theme::RADIUS_CONTROL), fill);
        p.rect_stroke(row_rect, theme::cr(theme::RADIUS_CONTROL), Stroke::new(theme::BORDER_WIDTH, theme::BORDER), StrokeKind::Inside);

        let text_max_w = (row_w - 2.0 * theme::INSET_BUTTON).max(theme::TARGET_MIN);
        let label_elided = widgets::elide(&p, &prompt.label, &theme::font_body_medium(), text_max_w);
        let body_elided = widgets::elide(&p, &prompt.text, &theme::font_caption(), text_max_w);

        p.text(
            Pos2::new(row_rect.left() + theme::INSET_BUTTON, row_rect.top() + 13.0),
            Align2::LEFT_CENTER,
            label_elided,
            theme::font_body_medium(),
            theme::TEXT,
        );
        p.text(
            Pos2::new(row_rect.left() + theme::INSET_BUTTON, row_rect.top() + 28.0),
            Align2::LEFT_CENTER,
            body_elided,
            theme::font_caption(),
            theme::TEXT_SECONDARY,
        );
        widgets::record(ui, &row_id, row_rect);

        if resp.clicked() {
            apply_sample_prompt_to_draft(input_line, &prompt.text);
            *viewport_focused = false;
            prompt_clicked = true;
        }
        y += theme::CONTROL_ROW + theme::SPACE_ROW;
    }

    if let Some(hint) = attach_hint_line(AttachHintCapabilities::CURRENT) {
        let hint_y = y - theme::SPACE_ROW + theme::SPACE_GROUP;
        let hint_rect = Rect::from_min_size(Pos2::new(row_x, hint_y), Vec2::new(row_w, theme::LINE_CAPTION));
        let hint_elided = widgets::elide(&p, &hint, &theme::font_caption(), row_w);
        p.text(Pos2::new(row_x, hint_rect.center().y), Align2::LEFT_CENTER, hint_elided, theme::font_caption(), theme::TEXT_SECONDARY);
        widgets::record(ui, "agents-empty-attach-hint", hint_rect);
    }

    prompt_clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use designos_doc::{FrameProps, Node, TextProps};

    #[test]
    fn shipped_prompts_load_within_limits() {
        let catalog = parse_sample_prompts(SHIPPED_SAMPLE_PROMPTS_TOML).expect("shipped TOML must parse cleanly");
        for ctx in [PromptContext::EmptyFile, PromptContext::Frame, PromptContext::NoSelection] {
            let list = catalog.for_context(ctx);
            assert!(!list.is_empty(), "context {} must have at least 1 prompt", ctx.as_str());
            assert!(list.len() <= MAX_PROMPTS_PER_CONTEXT, "context {} has {} prompts (max {})", ctx.as_str(), list.len(), MAX_PROMPTS_PER_CONTEXT);
            for item in list {
                assert_eq!(item.context, ctx);
                assert!(!item.label.is_empty());
                assert!(!item.text.is_empty());
                assert!(item.text.chars().count() <= MAX_PROMPT_TEXT_CHARS, "prompt text exceeds 300 chars: {:?}", item.text);
                assert!(!contains_forbidden_path(&item.label), "label contains forbidden path: {:?}", item.label);
                assert!(!contains_forbidden_path(&item.text), "text contains forbidden path: {:?}", item.text);
            }
        }
    }

    #[test]
    fn loader_rejects_bad_entries() {
        let unknown_ctx = r#"
[[prompts]]
context = "unknown_ctx"
label = "Bad"
text = "Do something."
"#;
        assert!(matches!(parse_sample_prompts(unknown_ctx), Err(PromptLoadError::UnknownContext(_))));

        let mut seven = String::new();
        for i in 1..=7 {
            seven.push_str(&format!("[[prompts]]\ncontext = \"frame\"\nlabel = \"P{i}\"\ntext = \"Prompt {i}.\"\n\n"));
        }
        assert!(matches!(parse_sample_prompts(&seven), Err(PromptLoadError::TooManyForContext { context: PromptContext::Frame, count: 7 })));

        let empty_label = r#"
[[prompts]]
context = "frame"
label = "   "
text = "Valid text."
"#;
        assert_eq!(parse_sample_prompts(empty_label), Err(PromptLoadError::EmptyLabel));

        let empty_text = r#"
[[prompts]]
context = "frame"
label = "Valid label"
text = "   "
"#;
        assert_eq!(parse_sample_prompts(empty_text), Err(PromptLoadError::EmptyText));

        let too_long = format!("[[prompts]]\ncontext = \"frame\"\nlabel = \"Long\"\ntext = \"{}\"\n", "a".repeat(MAX_PROMPT_TEXT_CHARS + 1));
        assert!(matches!(parse_sample_prompts(&too_long), Err(PromptLoadError::TextTooLong { len: 301 })));

        for bad_text in [
            "Audit against plans/specs/designos-studio-v2/prd/PRD.md",
            "Read README.md and summarize",
            "Inspect /tmp/designos-live-spec.json",
            r"Inspect \Windows\System32\drivers",
        ] {
            let src = format!("[[prompts]]\ncontext = \"no_selection\"\nlabel = \"Bad\"\ntext = {bad_text:?}\n");
            assert!(
                matches!(parse_sample_prompts(&src), Err(PromptLoadError::ForbiddenPathReference(_))),
                "loader must reject forbidden path reference {bad_text:?}"
            );
        }
    }

    #[test]
    fn context_is_chosen_in_order() {
        let mut doc = Document::new("688 Context Order");
        let page = doc.add_page("Page 1");

        // 1. Empty file (no page has any child layer) -> EmptyFile
        assert_eq!(prompt_context(&doc, page, &[]), PromptContext::EmptyFile);

        // Add a Text node and a Frame node to the page
        let text_id = doc.alloc_id();
        let mut text_node = Node::new(text_id, NodeKind::Text(TextProps { content: "Hello".into(), ..Default::default() }));
        text_node.name = "Heading".into();
        let text_id = doc.insert(page, text_node).expect("insert text");

        let frame_id = doc.alloc_id();
        let mut frame_node = Node::new(frame_id, NodeKind::Frame(FrameProps::default()));
        frame_node.name = "Hero Frame".into();
        frame_node.visible = false;
        frame_node.locked = true;
        let frame_id = doc.insert(page, frame_node).expect("insert frame");

        // 2. Non-empty file, no selection -> NoSelection
        assert_eq!(prompt_context(&doc, page, &[]), PromptContext::NoSelection);

        // 3. Non-empty file, text selection only -> NoSelection
        assert_eq!(prompt_context(&doc, page, &[text_id]), PromptContext::NoSelection);

        // 4. Non-empty file, hidden/locked frame selected (alone or with text) -> Frame
        assert_eq!(prompt_context(&doc, page, &[frame_id]), PromptContext::Frame);
        assert_eq!(prompt_context(&doc, page, &[text_id, frame_id]), PromptContext::Frame);
    }

    #[test]
    fn hint_names_only_existing_features() {
        assert_eq!(attach_hint_line(AttachHintCapabilities::CURRENT), None);
        assert_eq!(attach_hint_line(AttachHintCapabilities { selection_chip: false, at_mentions: false }), None);
        assert_eq!(
            attach_hint_line(AttachHintCapabilities { selection_chip: true, at_mentions: false }).as_deref(),
            Some("Click the selection chip to send what is selected.")
        );
        assert_eq!(attach_hint_line(AttachHintCapabilities { selection_chip: false, at_mentions: true }).as_deref(), Some("Type @ to mention layers."));
        assert_eq!(
            attach_hint_line(AttachHintCapabilities { selection_chip: true, at_mentions: true }).as_deref(),
            Some("Click the selection chip to send what is selected. Type @ to mention layers.")
        );
    }
}
