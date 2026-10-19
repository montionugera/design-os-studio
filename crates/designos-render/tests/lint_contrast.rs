//! Integration tests for E15-10 (`contrast` rule: analytic fast path + pixel-sampled backdrop).

use designos_doc::lint::{lint_subtree, Backdrop, Budget, LintConfig, LintEnv, LintPreset, Severity, TextMetrics};
use designos_doc::{
    Color, ContrastLevel, Document, GradientStop, Node, NodeId, NodeKind, Paint, PaintKind, Rect, TextAutoResize, TextProps, TextStyle, Variable,
    VariableCollection, VariableKind, VariableMode, VariableValue, Vec2,
};
use designos_render::RenderLintEnv;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingEnv {
    inner: RenderLintEnv,
    backdrop_calls: AtomicUsize,
}

impl CountingEnv {
    fn new() -> Self {
        Self { inner: RenderLintEnv, backdrop_calls: AtomicUsize::new(0) }
    }

    fn calls(&self) -> usize {
        self.backdrop_calls.load(Ordering::SeqCst)
    }
}

impl LintEnv for CountingEnv {
    fn has_text_engine(&self) -> bool {
        self.inner.has_text_engine()
    }

    fn text_metrics(&self, node: &Node, props: &TextProps, size: Vec2) -> Option<TextMetrics> {
        self.inner.text_metrics(node, props, size)
    }

    fn backdrop(&self, doc: &Document, node: NodeId, rect: Rect) -> Option<Backdrop> {
        self.backdrop_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.backdrop(doc, node, rect)
    }
}

fn make_text_node(id: NodeId, rect: Rect, name: &str, content: &str, color: Color, font_size: f32, font_weight: u16) -> Node {
    let mut style = TextStyle { font_size, font_weight, fills: vec![Paint::solid(color)], ..Default::default() };
    style.font_family = "Inter".to_string();
    let mut n = Node::text(id, rect, content, style);
    n.name = name.to_string();
    if let NodeKind::Text(tp) = &mut n.kind {
        tp.auto_resize = TextAutoResize::None;
    }
    n
}

#[test]
fn test_1_analytic_fast_path_solid_on_solid_does_not_call_backdrop() {
    let mut doc = Document::new("analytic");
    let page = doc.add_page("Page 1");

    let fid = doc.alloc_id();
    let mut frame = Node::frame(fid, Rect::new(0.0, 0.0, 200.0, 80.0));
    frame.name = "Card".to_string();
    frame.fills = vec![Paint::solid(Color::WHITE)];
    doc.insert(page, frame).unwrap();

    // #777777 on #FFFFFF is 4.48:1 (< 4.5:1 AA normal text).
    let tid = doc.alloc_id();
    let text = make_text_node(tid, Rect::new(16.0, 16.0, 120.0, 24.0), "Body Copy", "Hello world", Color::rgb(0x77, 0x77, 0x77), 16.0, 400);
    doc.insert(fid, text).unwrap();

    let env = CountingEnv::new();
    let cfg = LintConfig::from_preset(LintPreset::Recommended);
    let report = lint_subtree(&doc, &[page], &cfg, &env, &mut Budget::unlimited());

    assert_eq!(env.calls(), 0, "analytic fast path must not invoke LintEnv::backdrop");
    let finding = report.findings.iter().find(|f| f.rule == "contrast" && f.node == tid).expect("expected contrast finding");
    assert_eq!(finding.method, Some("analytic"));
    assert_eq!(finding.level, Some("AA"));
    assert_eq!(finding.large, Some(false));
    assert_eq!(finding.required_ratio, Some(4.5));
    assert!((finding.measured_ratio.unwrap() - 4.48).abs() <= 0.02);
    assert_eq!(finding.foreground_hex.as_deref(), Some("#777777"));
    assert_eq!(finding.backdrop_hex.as_deref(), Some("#FFFFFF"));
}

#[test]
fn test_2_semi_transparent_sibling_uses_pixel_path() {
    let mut doc = Document::new("semi-transparent-sibling");
    let page = doc.add_page("Page 1");

    let fid = doc.alloc_id();
    let mut frame = Node::frame(fid, Rect::new(0.0, 0.0, 200.0, 80.0));
    frame.name = "Hero".to_string();
    frame.fills = vec![Paint::solid(Color::WHITE)];
    doc.insert(page, frame).unwrap();

    // 50% black sibling over white background -> resolves to #808080 (~3.95:1 against #FFFFFF text).
    let sid = doc.alloc_id();
    let mut scrim = Node::rectangle(sid, Rect::new(0.0, 0.0, 200.0, 80.0));
    scrim.name = "Dimmer".to_string();
    scrim.fills = vec![Paint::solid(Color { r: 0.0, g: 0.0, b: 0.0, a: 0.5 })];
    doc.insert(fid, scrim).unwrap();

    let tid = doc.alloc_id();
    let text = make_text_node(tid, Rect::new(16.0, 16.0, 140.0, 32.0), "Headline", "Hello world", Color::WHITE, 16.0, 400);
    doc.insert(fid, text).unwrap();

    let env = CountingEnv::new();
    let cfg = LintConfig::from_preset(LintPreset::Recommended);
    let report = lint_subtree(&doc, &[page], &cfg, &env, &mut Budget::unlimited());

    assert!(env.calls() >= 1, "semi-transparent sibling must trigger pixel path");
    let finding = report.findings.iter().find(|f| f.rule == "contrast" && f.node == tid).expect("expected contrast finding");
    assert_eq!(finding.method, Some("pixels"));
    assert!(finding.measured_ratio.unwrap() < 4.5, "expected < 4.5 ratio, got {:?}", finding.measured_ratio);
    assert!(finding.measured_ratio.unwrap() > 3.5, "expected ~3.95 ratio, got {:?}", finding.measured_ratio);
}

#[test]
fn test_3_linear_gradient_backdrop_flags_failing_half() {
    let mut doc = Document::new("gradient-backdrop");
    let page = doc.add_page("Page 1");

    let fid = doc.alloc_id();
    let mut frame = Node::frame(fid, Rect::new(0.0, 0.0, 240.0, 60.0));
    frame.name = "Banner".to_string();
    frame.fills = vec![Paint {
        visible: true,
        opacity: 1.0,
        blend: designos_doc::BlendMode::Normal,
        kind: PaintKind::LinearGradient {
            handles: [Vec2::new(0.0, 0.5), Vec2::new(1.0, 0.5), Vec2::new(0.0, 1.0)],
            stops: vec![GradientStop { pos: 0.0, color: Color::BLACK }, GradientStop { pos: 1.0, color: Color::WHITE }],
        },
    }];
    doc.insert(page, frame).unwrap();

    // White text spanning across both halves of the horizontal black->white gradient.
    let tid = doc.alloc_id();
    let text = make_text_node(tid, Rect::new(10.0, 12.0, 220.0, 36.0), "Banner Title", "WWWWWWWWWWWWWWWWWWWW", Color::WHITE, 20.0, 700);
    doc.insert(fid, text).unwrap();

    let env = CountingEnv::new();
    let cfg = LintConfig::from_preset(LintPreset::Recommended);
    let report = lint_subtree(&doc, &[page], &cfg, &env, &mut Budget::unlimited());

    assert!(env.calls() >= 1);
    let finding = report.findings.iter().find(|f| f.rule == "contrast" && f.node == tid).expect("expected contrast finding on gradient");
    assert_eq!(finding.method, Some("pixels"));
    assert!(finding.measured_ratio.unwrap() < 3.0, "expected worst ratio < 3.0 on light side, got {:?}", finding.measured_ratio);
    let sample = finding.worst_sample.expect("expected worst_sample coordinates");
    assert!(sample.x > 100.0, "expected worst sample on right (light) half of gradient, got x={}", sample.x);
}

#[test]
fn test_4_covering_sibling_emits_covered_by_message() {
    let mut doc = Document::new("covered-text");
    let page = doc.add_page("Page 1");

    let fid = doc.alloc_id();
    let mut frame = Node::frame(fid, Rect::new(0.0, 0.0, 200.0, 80.0));
    frame.name = "Container".to_string();
    frame.fills = vec![Paint::solid(Color::WHITE)];
    doc.insert(page, frame).unwrap();

    let tid = doc.alloc_id();
    let text = make_text_node(tid, Rect::new(20.0, 20.0, 120.0, 30.0), "Hidden Label", "Hello world", Color::BLACK, 16.0, 400);
    doc.insert(fid, text).unwrap();

    // Opaque sibling painted AFTER the text node that completely covers it.
    let cid = doc.alloc_id();
    let mut cover = Node::rectangle(cid, Rect::new(0.0, 0.0, 200.0, 80.0));
    cover.name = "Modal Scrim".to_string();
    cover.fills = vec![Paint::solid(Color::WHITE)];
    doc.insert(fid, cover).unwrap();

    let cfg = LintConfig::from_preset(LintPreset::Recommended);
    let report = lint_subtree(&doc, &[page], &cfg, &RenderLintEnv, &mut Budget::unlimited());

    let finding = report.findings.iter().find(|f| f.rule == "contrast" && f.node == tid).expect("expected covered text finding");
    assert_eq!(finding.message, "text is covered by Modal Scrim");
}

#[test]
fn test_5_large_text_thresholds_24px_and_18_66px_bold() {
    let mut doc = Document::new("large-text");
    let page = doc.add_page("Page 1");

    let fid = doc.alloc_id();
    let mut frame = Node::frame(fid, Rect::new(0.0, 0.0, 300.0, 200.0));
    frame.fills = vec![Paint::solid(Color::WHITE)];
    doc.insert(page, frame).unwrap();

    // #949494 on #FFFFFF has contrast ~3.03:1 (passes 3.0:1 large text, fails 4.5:1 normal text).
    let c = Color::rgb(0x94, 0x94, 0x94);
    let t_small = doc.alloc_id();
    doc.insert(fid, make_text_node(t_small, Rect::new(10.0, 10.0, 150.0, 24.0), "Small 16", "Normal text", c, 16.0, 400)).unwrap();

    let t_large_24 = doc.alloc_id();
    doc.insert(fid, make_text_node(t_large_24, Rect::new(10.0, 50.0, 150.0, 32.0), "Large 24", "Large text", c, 24.0, 400)).unwrap();

    let t_bold_19 = doc.alloc_id();
    doc.insert(fid, make_text_node(t_bold_19, Rect::new(10.0, 100.0, 150.0, 28.0), "Bold 19", "Bold large", c, 19.0, 700)).unwrap();

    let cfg = LintConfig::from_preset(LintPreset::Recommended);
    let report = lint_subtree(&doc, &[page], &cfg, &RenderLintEnv, &mut Budget::unlimited());

    let contrast_nodes: Vec<NodeId> = report.findings.iter().filter(|f| f.rule == "contrast").map(|f| f.node).collect();
    assert_eq!(contrast_nodes, vec![t_small], "only normal 16px text should fail AA at 3.03:1");
}

#[test]
fn test_6_wcag_aaa_mode_and_warning_severity_when_aa_passes() {
    let mut doc = Document::new("aaa-mode");
    let page = doc.add_page("Page 1");

    let fid = doc.alloc_id();
    let mut frame = Node::frame(fid, Rect::new(0.0, 0.0, 300.0, 120.0));
    frame.fills = vec![Paint::solid(Color::WHITE)];
    doc.insert(page, frame).unwrap();

    // #767676 on #FFFFFF is 4.54:1 -> passes AA (4.5:1), fails AAA (7.0:1).
    let tid = doc.alloc_id();
    doc.insert(fid, make_text_node(tid, Rect::new(10.0, 10.0, 150.0, 24.0), "AA-only text", "Passes AA only", Color::rgb(0x76, 0x76, 0x76), 16.0, 400))
        .unwrap();

    // Under AA: 0 findings.
    let cfg_aa = LintConfig::from_preset(LintPreset::Recommended);
    let rep_aa = lint_subtree(&doc, &[page], &cfg_aa, &RenderLintEnv, &mut Budget::unlimited());
    assert!(rep_aa.findings.iter().all(|f| f.node != tid));

    // Under AAA: 1 finding with level == "AAA", required_ratio == 7.0, and Warning severity (since AA passes).
    let mut cfg_aaa = LintConfig::from_preset(LintPreset::Recommended);
    cfg_aaa.contrast_level = ContrastLevel::AAA;
    let rep_aaa = lint_subtree(&doc, &[page], &cfg_aaa, &RenderLintEnv, &mut Budget::unlimited());
    let finding = rep_aaa.findings.iter().find(|f| f.rule == "contrast" && f.node == tid).expect("expected AAA finding");
    assert_eq!(finding.level, Some("AAA"));
    assert_eq!(finding.required_ratio, Some(7.0));
    assert_eq!(finding.severity, Severity::Warning);
}

#[test]
fn test_7_suggest_passing_variable_pair_by_cie76_delta_e() {
    let mut doc = Document::new("var-suggestion");
    doc.variable_collections.insert(
        "c1".to_string(),
        VariableCollection {
            id: "c1".to_string(),
            name: "Tokens".to_string(),
            modes: vec![VariableMode { id: "light".to_string(), name: "Light".to_string() }],
            default_mode: "light".to_string(),
            variables: vec!["v_muted".to_string(), "v_secondary".to_string(), "v_primary".to_string()],
            key: None,
            source_library_key: None,
        },
    );
    for (id, name, hex) in [
        ("v_muted", "text/muted", Color::rgb(0x99, 0x99, 0x99)),
        ("v_secondary", "text/secondary", Color::rgb(0x55, 0x55, 0x55)),
        ("v_primary", "text/primary", Color::rgb(0x11, 0x11, 0x11)),
    ] {
        doc.variables.insert(
            id.to_string(),
            Variable {
                id: id.to_string(),
                name: name.to_string(),
                collection: "c1".to_string(),
                kind: VariableKind::Color,
                values: BTreeMap::from([("light".to_string(), VariableValue::Color(hex))]),
                description: String::new(),
                scopes: vec![],
                hidden_from_publishing: false,
                key: None,
                code_syntax: BTreeMap::new(),
            },
        );
    }

    let page = doc.add_page("Page 1");
    let fid = doc.alloc_id();
    let mut frame = Node::frame(fid, Rect::new(0.0, 0.0, 200.0, 80.0));
    frame.fills = vec![Paint::solid(Color::WHITE)];
    doc.insert(page, frame).unwrap();

    let tid = doc.alloc_id();
    let mut text = make_text_node(tid, Rect::new(10.0, 10.0, 120.0, 24.0), "Muted Caption", "Caption", Color::rgb(0x99, 0x99, 0x99), 14.0, 400);
    text.variables.bindings.insert("fills/0".to_string(), "v_muted".to_string());
    doc.insert(fid, text).unwrap();

    let cfg = LintConfig::from_preset(LintPreset::Recommended);
    let report = lint_subtree(&doc, &[page], &cfg, &RenderLintEnv, &mut Budget::unlimited());
    let finding = report.findings.iter().find(|f| f.rule == "contrast" && f.node == tid).expect("expected contrast finding");
    let fix = finding.suggested_fix.as_ref().expect("expected suggested_fix from variable collection");
    assert_eq!(fix.kind, "bind");
    assert_eq!(fix.property, "fills/0");
    assert_eq!(fix.from, json!("#999999"));
    assert_eq!(fix.to, json!("var:text/secondary"));
}
