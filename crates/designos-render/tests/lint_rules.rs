use designos_doc::lint::{lint, lint_subtree, preset_rule_ids, resolve_rule, Budget, Finding, LintConfig, LintPreset, NoLintEnv};
use designos_doc::{AutoLayout, Color, Document, FrameProps, LayoutDirection, Node, NodeId, NodeKind, Paint, Rect, Sizing, TextAutoResize, TextStyle};
use designos_render::RenderLintEnv;
use std::time::Duration;

fn new_doc_with_page(name: &str) -> (Document, NodeId) {
    let mut doc = Document::new(name);
    let page = doc.add_page("Page 1");
    (doc, page)
}

fn add_frame(doc: &mut Document, parent: NodeId, name: &str, r: Rect, fp: FrameProps) -> NodeId {
    let id = doc.alloc_id();
    let mut n = Node::frame(id, r);
    n.name = name.into();
    n.kind = NodeKind::Frame(fp);
    doc.insert(parent, n).unwrap();
    id
}

fn add_rect(doc: &mut Document, parent: NodeId, name: &str, r: Rect) -> NodeId {
    let id = doc.alloc_id();
    let mut n = Node::rectangle(id, r);
    n.name = name.into();
    n.fills = vec![Paint::solid(Color::BLACK)];
    doc.insert(parent, n).unwrap();
    id
}

fn add_text(doc: &mut Document, parent: NodeId, name: &str, r: Rect, content: &str, style: TextStyle, auto_resize: TextAutoResize) -> NodeId {
    let id = doc.alloc_id();
    let mut n = Node::text(id, r, content, style);
    n.name = name.into();
    if let NodeKind::Text(tp) = &mut n.kind {
        tp.auto_resize = auto_resize;
    }
    doc.insert(parent, n).unwrap();
    id
}

fn only_rule_config(rule: &str) -> LintConfig {
    LintConfig::from_rule_list(&[rule.to_string()]).expect("valid rule")
}

fn lint_single_rule(doc: &Document, rule: &str) -> Vec<Finding> {
    let cfg = only_rule_config(rule);
    lint(doc, &cfg, &RenderLintEnv).findings
}

#[test]
fn fail_and_clean_overflow_parent() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Overflow");
    let frame = add_frame(&mut fail_doc, page, "Parent", Rect::new(0.0, 0.0, 100.0, 100.0), FrameProps { clips_content: false, ..Default::default() });
    let child = add_rect(&mut fail_doc, frame, "OverflowChild", Rect::new(20.0, 10.0, 120.0, 50.0));
    let findings = lint_single_rule(&fail_doc, "overflow-parent");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "overflow-parent");
    assert_eq!(findings[0].node, child);
    assert_eq!(findings[0].bounds, Rect::new(20.0, 10.0, 120.0, 50.0));
    assert!(findings[0].suggested_fix.is_none());

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Overflow");
    let cframe = add_frame(&mut clean_doc, cpage, "Parent", Rect::new(0.0, 0.0, 100.0, 100.0), FrameProps { clips_content: false, ..Default::default() });
    add_rect(&mut clean_doc, cframe, "InsideChild", Rect::new(10.0, 10.0, 80.0, 50.0));
    assert!(lint_single_rule(&clean_doc, "overflow-parent").is_empty());
}

#[test]
fn fail_and_clean_clipped_partial() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Clipped Partial");
    let frame = add_frame(&mut fail_doc, page, "ClipFrame", Rect::new(0.0, 0.0, 100.0, 100.0), FrameProps { clips_content: true, ..Default::default() });
    let child = add_rect(&mut fail_doc, frame, "HalfClipped", Rect::new(50.0, 10.0, 100.0, 40.0));
    let findings = lint_single_rule(&fail_doc, "clipped-partial");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "clipped-partial");
    assert_eq!(findings[0].node, child);
    assert_eq!(findings[0].bounds, Rect::new(50.0, 10.0, 100.0, 40.0));
    assert!(findings[0].suggested_fix.is_none());

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Clipped Partial");
    let cframe = add_frame(&mut clean_doc, cpage, "ClipFrame", Rect::new(0.0, 0.0, 100.0, 100.0), FrameProps { clips_content: true, ..Default::default() });
    add_rect(&mut clean_doc, cframe, "Inside", Rect::new(10.0, 10.0, 50.0, 40.0));
    assert!(lint_single_rule(&clean_doc, "clipped-partial").is_empty());
}

#[test]
fn fail_and_clean_clipped_full() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Clipped Full");
    let frame = add_frame(&mut fail_doc, page, "ClipFrame", Rect::new(0.0, 0.0, 100.0, 100.0), FrameProps { clips_content: true, ..Default::default() });
    let child = add_rect(&mut fail_doc, frame, "FullyOutside", Rect::new(150.0, 10.0, 40.0, 40.0));
    let findings = lint_single_rule(&fail_doc, "clipped-full");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "clipped-full");
    assert_eq!(findings[0].node, child);
    assert_eq!(findings[0].bounds, Rect::new(150.0, 10.0, 40.0, 40.0));
    assert!(findings[0].suggested_fix.is_none());

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Clipped Full");
    let cframe = add_frame(&mut clean_doc, cpage, "ClipFrame", Rect::new(0.0, 0.0, 100.0, 100.0), FrameProps { clips_content: true, ..Default::default() });
    add_rect(&mut clean_doc, cframe, "Inside", Rect::new(10.0, 10.0, 40.0, 40.0));
    assert!(lint_single_rule(&clean_doc, "clipped-full").is_empty());
}

#[test]
fn fail_and_clean_sibling_overlap() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Sibling Overlap");
    let frame = add_frame(&mut fail_doc, page, "FreeformParent", Rect::new(0.0, 0.0, 200.0, 200.0), FrameProps::default());
    add_text(&mut fail_doc, frame, "TitleA", Rect::new(10.0, 10.0, 80.0, 24.0), "Hello World", TextStyle::default(), TextAutoResize::None);
    let _b = add_text(&mut fail_doc, frame, "TitleB", Rect::new(30.0, 12.0, 80.0, 24.0), "Overlapping Text", TextStyle::default(), TextAutoResize::None);
    let findings = lint_single_rule(&fail_doc, "sibling-overlap");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "sibling-overlap");
    assert_eq!(findings[0].node, frame);
    assert_eq!(findings[0].bounds, Rect::new(0.0, 0.0, 200.0, 200.0));
    assert!(findings[0].suggested_fix.is_none());

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Sibling Overlap");
    let cframe = add_frame(&mut clean_doc, cpage, "FreeformParent", Rect::new(0.0, 0.0, 200.0, 200.0), FrameProps::default());
    add_text(&mut clean_doc, cframe, "TitleA", Rect::new(10.0, 10.0, 80.0, 24.0), "Hello World", TextStyle::default(), TextAutoResize::None);
    add_text(&mut clean_doc, cframe, "TitleB", Rect::new(10.0, 50.0, 80.0, 24.0), "Separated Text", TextStyle::default(), TextAutoResize::None);
    assert!(lint_single_rule(&clean_doc, "sibling-overlap").is_empty());
}

#[test]
fn fail_and_clean_text_truncated() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Text Truncated");
    let tid = add_text(
        &mut fail_doc,
        page,
        "BioText",
        Rect::new(0.0, 0.0, 80.0, 20.0),
        "Save changes to your profile settings right now before leaving",
        TextStyle { font_size: 16.0, ..Default::default() },
        TextAutoResize::None,
    );
    let findings = lint_single_rule(&fail_doc, "text-truncated");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "text-truncated");
    assert_eq!(findings[0].node, tid);
    assert_eq!(findings[0].bounds, Rect::new(0.0, 0.0, 80.0, 20.0));
    let fix = findings[0].suggested_fix.as_ref().expect("text-truncated suggests HEIGHT fix");
    assert_eq!(fix.property, "textAutoResize");
    assert_eq!(fix.to, serde_json::json!("HEIGHT"));

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Text Truncated");
    add_text(
        &mut clean_doc,
        cpage,
        "BioText",
        Rect::new(0.0, 0.0, 300.0, 60.0),
        "Short text",
        TextStyle { font_size: 16.0, ..Default::default() },
        TextAutoResize::None,
    );
    assert!(lint_single_rule(&clean_doc, "text-truncated").is_empty());
}

#[test]
fn fail_and_clean_text_unexpected_wrap() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Text Unexpected Wrap");
    let tid = add_text(
        &mut fail_doc,
        page,
        "ButtonLabel",
        Rect::new(0.0, 0.0, 50.0, 48.0),
        "Save changes",
        TextStyle { font_size: 16.0, ..Default::default() },
        TextAutoResize::Height,
    );
    let findings = lint_single_rule(&fail_doc, "text-unexpected-wrap");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "text-unexpected-wrap");
    assert_eq!(findings[0].node, tid);
    assert_eq!(findings[0].bounds, Rect::new(0.0, 0.0, 50.0, 48.0));
    let fix = findings[0].suggested_fix.as_ref().expect("text-unexpected-wrap suggests WIDTH_AND_HEIGHT fix");
    assert_eq!(fix.property, "textAutoResize");
    assert_eq!(fix.to, serde_json::json!("WIDTH_AND_HEIGHT"));

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Text Unexpected Wrap");
    add_text(
        &mut clean_doc,
        cpage,
        "ButtonLabel",
        Rect::new(0.0, 0.0, 240.0, 28.0),
        "Save changes",
        TextStyle { font_size: 16.0, ..Default::default() },
        TextAutoResize::Height,
    );
    assert!(lint_single_rule(&clean_doc, "text-unexpected-wrap").is_empty());
}

#[test]
fn fail_and_clean_collapsed_size() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Collapsed Size");
    let nid = add_frame(&mut fail_doc, page, "ZeroHeightFrame", Rect::new(10.0, 20.0, 120.0, 0.0), FrameProps::default());
    let findings = lint_single_rule(&fail_doc, "collapsed-size");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "collapsed-size");
    assert_eq!(findings[0].node, nid);
    assert_eq!(findings[0].bounds, Rect::new(10.0, 20.0, 120.0, 0.0));
    assert!(findings[0].suggested_fix.is_none());

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Collapsed Size");
    add_frame(&mut clean_doc, cpage, "NormalFrame", Rect::new(10.0, 20.0, 120.0, 40.0), FrameProps::default());
    assert!(lint_single_rule(&clean_doc, "collapsed-size").is_empty());
}

#[test]
fn fail_and_clean_sizing_conflict() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Sizing Conflict");
    let parent = add_frame(
        &mut fail_doc,
        page,
        "HugParent",
        Rect::new(0.0, 0.0, 200.0, 80.0),
        FrameProps {
            auto_layout: Some(AutoLayout {
                direction: LayoutDirection::Horizontal,
                primary_sizing: Sizing::Hug,
                counter_sizing: Sizing::Fixed,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let cid = fail_doc.alloc_id();
    let mut child = Node::rectangle(cid, Rect::new(0.0, 0.0, 80.0, 40.0));
    child.name = "FillChild".into();
    child.fills = vec![Paint::solid(Color::BLACK)];
    child.layout_child.horizontal = Sizing::Fill;
    fail_doc.insert(parent, child).unwrap();

    let findings = lint_single_rule(&fail_doc, "sizing-conflict");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "sizing-conflict");
    assert_eq!(findings[0].node, cid);
    assert_eq!(findings[0].bounds, Rect::new(0.0, 0.0, 80.0, 40.0));
    let fix = findings[0].suggested_fix.as_ref().expect("sizing-conflict suggests FIXED fix");
    assert_eq!(fix.property, "horizontalSizing");
    assert_eq!(fix.to, serde_json::json!("Fixed"));

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Sizing Conflict");
    let cparent = add_frame(
        &mut clean_doc,
        cpage,
        "FixedParent",
        Rect::new(0.0, 0.0, 200.0, 80.0),
        FrameProps {
            auto_layout: Some(AutoLayout {
                direction: LayoutDirection::Horizontal,
                primary_sizing: Sizing::Fixed,
                counter_sizing: Sizing::Fixed,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let ccid = clean_doc.alloc_id();
    let mut cchild = Node::rectangle(ccid, Rect::new(0.0, 0.0, 80.0, 40.0));
    cchild.name = "FillChild".into();
    cchild.fills = vec![Paint::solid(Color::BLACK)];
    cchild.layout_child.horizontal = Sizing::Fill;
    clean_doc.insert(cparent, cchild).unwrap();
    assert!(lint_single_rule(&clean_doc, "sizing-conflict").is_empty());
}

#[test]
fn fail_and_clean_text_missing_fill() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Text Missing Fill");
    let tid = fail_doc.alloc_id();
    let mut tnode = Node::text(tid, Rect::new(5.0, 5.0, 100.0, 24.0), "Invisible label", TextStyle { fills: Vec::new(), ..Default::default() });
    tnode.name = "InvisibleText".into();
    tnode.fills.clear();
    fail_doc.insert(page, tnode).unwrap();

    let findings = lint_single_rule(&fail_doc, "text-missing-fill");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "text-missing-fill");
    assert_eq!(findings[0].node, tid);
    assert_eq!(findings[0].bounds, Rect::new(5.0, 5.0, 100.0, 24.0));
    assert!(findings[0].suggested_fix.is_none());

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Text Missing Fill");
    add_text(&mut clean_doc, cpage, "VisibleText", Rect::new(5.0, 5.0, 100.0, 24.0), "Visible label", TextStyle::default(), TextAutoResize::WidthAndHeight);
    assert!(lint_single_rule(&clean_doc, "text-missing-fill").is_empty());
}

#[test]
fn fail_and_clean_absolute_in_auto_layout() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Absolute In Auto Layout");
    let parent = add_frame(
        &mut fail_doc,
        page,
        "AutoFrame",
        Rect::new(0.0, 0.0, 200.0, 100.0),
        FrameProps { auto_layout: Some(AutoLayout::default()), ..Default::default() },
    );
    let cid = fail_doc.alloc_id();
    let mut child = Node::rectangle(cid, Rect::new(10.0, 10.0, 40.0, 40.0));
    child.name = "PinnedBox".into();
    child.fills = vec![Paint::solid(Color::BLACK)];
    child.layout_child.absolute = true;
    fail_doc.insert(parent, child).unwrap();

    let findings = lint_single_rule(&fail_doc, "absolute-in-auto-layout");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "absolute-in-auto-layout");
    assert_eq!(findings[0].node, cid);
    assert_eq!(findings[0].bounds, Rect::new(10.0, 10.0, 40.0, 40.0));
    assert!(findings[0].suggested_fix.is_none());

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Absolute In Auto Layout");
    let cparent = add_frame(
        &mut clean_doc,
        cpage,
        "AutoFrame",
        Rect::new(0.0, 0.0, 200.0, 100.0),
        FrameProps { auto_layout: Some(AutoLayout::default()), ..Default::default() },
    );
    let bcid = clean_doc.alloc_id();
    let mut badge = Node::rectangle(bcid, Rect::new(10.0, 10.0, 20.0, 20.0));
    badge.name = "FlowChild".into();
    badge.fills = vec![Paint::solid(Color::BLACK)];
    badge.layout_child.absolute = false;
    clean_doc.insert(cparent, badge).unwrap();
    assert!(lint_single_rule(&clean_doc, "absolute-in-auto-layout").is_empty());
}

#[test]
fn fail_and_clean_placeholder_text() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Placeholder Text");
    let tid = add_text(
        &mut fail_doc,
        page,
        "BodyCopy",
        Rect::new(0.0, 0.0, 200.0, 40.0),
        "Lorem ipsum dolor sit amet",
        TextStyle::default(),
        TextAutoResize::WidthAndHeight,
    );
    let findings = lint_single_rule(&fail_doc, "placeholder-text");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "placeholder-text");
    assert_eq!(findings[0].node, tid);
    assert_eq!(findings[0].bounds, Rect::new(0.0, 0.0, 200.0, 40.0));
    assert!(findings[0].suggested_fix.is_none());

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Placeholder Text");
    add_text(
        &mut clean_doc,
        cpage,
        "BodyCopy",
        Rect::new(0.0, 0.0, 200.0, 40.0),
        "Account security settings",
        TextStyle::default(),
        TextAutoResize::WidthAndHeight,
    );
    assert!(lint_single_rule(&clean_doc, "placeholder-text").is_empty());
}

#[test]
fn fail_and_clean_detached_instance() {
    let (mut fail_doc, page) = new_doc_with_page("Fail Detached Instance");
    let comp_id = fail_doc.alloc_id();
    let mut comp = Node::frame(comp_id, Rect::new(0.0, 0.0, 120.0, 40.0));
    comp.name = "PrimaryButton".into();
    comp.kind = NodeKind::Component(FrameProps::default());
    comp.source_id = Some("1:10".into());
    fail_doc.insert(page, comp).unwrap();

    let did = fail_doc.alloc_id();
    let mut detached = Node::frame(did, Rect::new(0.0, 60.0, 120.0, 40.0));
    detached.name = "PrimaryButtonCopy".into();
    detached.detached_symbol_id = Some("1:10".into());
    fail_doc.insert(page, detached).unwrap();

    let findings = lint_single_rule(&fail_doc, "detached-instance");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, "detached-instance");
    assert_eq!(findings[0].node, did);
    assert_eq!(findings[0].bounds, Rect::new(0.0, 60.0, 120.0, 40.0));
    let fix = findings[0].suggested_fix.as_ref().expect("detached-instance suggests reconnect fix when structure matches");
    assert_eq!(fix.kind, "reconnect");
    assert_eq!(fix.to, serde_json::json!(comp_id.0));

    let (mut clean_doc, cpage) = new_doc_with_page("Clean Detached Instance");
    add_frame(&mut clean_doc, cpage, "PlainFrame", Rect::new(0.0, 0.0, 120.0, 40.0), FrameProps::default());
    assert!(lint_single_rule(&clean_doc, "detached-instance").is_empty());
}

#[test]
fn cosmic_text_measurement_catches_real_overflow() {
    let (mut doc, page) = new_doc_with_page("Cosmic Text Overflow");
    let tid = add_text(
        &mut doc,
        page,
        "I18nHeader",
        Rect::new(0.0, 0.0, 60.0, 20.0),
        "Internationalization",
        TextStyle { font_family: "Inter".into(), font_size: 14.0, ..Default::default() },
        TextAutoResize::None,
    );

    let cfg = LintConfig::from_preset(LintPreset::Agent);
    let report = lint(&doc, &cfg, &RenderLintEnv);
    assert!(report.skipped_rules.is_empty());
    assert!(
        report.findings.iter().any(|f| f.rule == "text-truncated" && f.node == tid),
        "expected text-truncated on Internationalization in 60x20 box, got: {:?}",
        report.findings
    );
    assert!(
        report.findings.iter().any(|f| f.rule == "text-unexpected-wrap" && f.node == tid),
        "expected text-unexpected-wrap on Internationalization in 60x20 box, got: {:?}",
        report.findings
    );

    let headless_report = lint(&doc, &cfg, &NoLintEnv);
    let skipped_ids: Vec<&str> = headless_report.skipped_rules.iter().map(|s| s.rule.as_str()).collect();
    assert!(skipped_ids.contains(&"text-truncated"));
    assert!(skipped_ids.contains(&"text-unexpected-wrap"));
    assert!(headless_report.skipped_rules.iter().all(|s| s.reason == "no_text_engine"));
}

#[test]
fn presets_name_only_registered_rules() {
    for preset in [LintPreset::Agent, LintPreset::CheckDesigns, LintPreset::Recommended, LintPreset::Strict] {
        for id in preset_rule_ids(preset) {
            assert!(resolve_rule(id).is_some(), "preset {} names unregistered rule {id}", preset.as_str());
        }
    }
}

#[test]
fn ignored_never_reappears() {
    let (mut doc, page) = new_doc_with_page("Ignore Persistence");
    let frame = add_frame(&mut doc, page, "Parent", Rect::new(0.0, 0.0, 100.0, 100.0), FrameProps { clips_content: false, ..Default::default() });
    let child = add_rect(&mut doc, frame, "OverflowRect", Rect::new(20.0, 20.0, 120.0, 50.0));

    let cfg = only_rule_config("overflow-parent");
    assert_eq!(lint(&doc, &cfg, &RenderLintEnv).findings.len(), 1);

    doc.ignore_lint("overflow-parent", child);
    assert!(lint(&doc, &cfg, &RenderLintEnv).findings.is_empty());

    // 1. Native JSON round-trip
    let json_text = doc.to_json().expect("serialize native JSON");
    let from_json = Document::from_json(&json_text).expect("deserialize native JSON");
    assert!(lint(&from_json, &cfg, &RenderLintEnv).findings.is_empty());

    // 2. .fig archive round-trip
    let fig_bytes = designos_fig::export_fig(&doc).expect("export .fig");
    let from_fig = designos_fig::import_fig(&fig_bytes).expect("import .fig");
    assert!(lint(&from_fig, &cfg, &RenderLintEnv).findings.is_empty(), "expected overflow-parent finding to remain ignored after .fig round-trip");
}

#[test]
fn large_fixture_off_thread_budget() {
    let (mut doc, page) = new_doc_with_page("Large 2000-Node Fixture");
    let container = add_frame(&mut doc, page, "RootContainer", Rect::new(0.0, 0.0, 2000.0, 2000.0), FrameProps { clips_content: false, ..Default::default() });
    for i in 0..2_000 {
        let x = (i % 50) as f32 * 36.0;
        let y = (i / 50) as f32 * 36.0;
        let w = if i % 200 == 0 { 2500.0 } else { 24.0 };
        add_rect(&mut doc, container, &format!("Item_{i}"), Rect::new(x, y, w, 24.0));
    }

    let full_cfg = LintConfig::from_preset(LintPreset::Agent);
    let unlimited = lint(&doc, &full_cfg, &RenderLintEnv);
    assert!(unlimited.complete);
    assert!(unlimited.cursor.is_none());

    let mut sliced_findings = Vec::new();
    let mut cursor: Option<String> = None;
    let mut slices = 0usize;
    loop {
        slices += 1;
        let mut budget = Budget { max_nodes: 400, max_duration: Some(Duration::from_millis(4)), cursor: cursor.clone(), shallow_nodes: Default::default() };
        let step_report = lint_subtree(&doc, &doc.pages, &full_cfg, &RenderLintEnv, &mut budget);
        sliced_findings.extend(step_report.findings);
        if step_report.complete {
            assert!(step_report.cursor.is_none());
            break;
        }
        assert!(step_report.cursor.is_some(), "incomplete slice must return cursor");
        cursor = step_report.cursor;
        assert!(slices <= 100, "expected completion within bounded slices");
    }
    assert!(slices >= 2, "expected 2000-node run to span multiple slices");
    sliced_findings.sort_by_key(|f| designos_doc::lint::rules::severity_rank(f.severity));
    assert_eq!(sliced_findings, unlimited.findings);
}
