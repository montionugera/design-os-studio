//! Flexbox, CSS Grid, and vertical block-flow auto-layout inference into
//! [`designos_doc::AutoLayout`] and [`designos_doc::LayoutChild`].

use crate::import::{element_selector_label, AbsoluteFallback, ImportReport};
use crate::paint_map::{parse_css_px, parse_css_px_or_percent};
use crate::schema::Element;
use designos_doc::{AlignSelf, AutoLayout, GridTrack, GridTracks, LayoutAlign, LayoutChild, LayoutDirection, Sizing};
use std::collections::BTreeMap;

/// Returns `true` if `el` is positioned out of normal flow (`position: absolute | fixed`).
pub fn is_out_of_flow(el: &Element) -> bool {
    matches!(el.styles.get("position").map(|s| s.trim().to_ascii_lowercase()).as_deref(), Some("absolute") | Some("fixed"))
}

/// Returns `true` if `overflow` / `overflow-x` / `overflow-y` clips content.
pub fn map_overflow_clips(styles: &BTreeMap<String, String>) -> bool {
    for key in ["overflow", "overflow-x", "overflow-y"] {
        if let Some(v) = styles.get(key) {
            let lower = v.trim().to_ascii_lowercase();
            if lower.contains("hidden") || lower.contains("clip") || lower.contains("scroll") || lower.contains("auto") {
                return true;
            }
        }
    }
    false
}

/// Parses `[top, right, bottom, left]` padding from `styles`.
pub fn parse_padding(styles: &BTreeMap<String, String>, font_size: f32) -> [f32; 4] {
    let p = |k: &str| styles.get(k).and_then(|v| parse_css_px(v, font_size)).unwrap_or(0.0).max(0.0);
    [p("padding-top"), p("padding-right"), p("padding-bottom"), p("padding-left")]
}

/// Parses `[top, right, bottom, left]` border widths from `styles`.
pub fn parse_border_widths(styles: &BTreeMap<String, String>, font_size: f32) -> [f32; 4] {
    let b = |side: &str| -> f32 {
        let st = styles.get(&format!("border-{side}-style")).map(|s| s.trim()).unwrap_or("none");
        if st.eq_ignore_ascii_case("none") || st.eq_ignore_ascii_case("hidden") {
            return 0.0;
        }
        styles.get(&format!("border-{side}-width")).and_then(|v| parse_css_px(v, font_size)).unwrap_or(0.0).max(0.0)
    };
    [b("top"), b("right"), b("bottom"), b("left")]
}

/// Parses `[top, right, bottom, left]` margins from `styles`.
pub fn parse_margins(styles: &BTreeMap<String, String>, font_size: f32) -> [f32; 4] {
    let m = |k: &str| styles.get(k).and_then(|v| parse_css_px(v, font_size)).unwrap_or(0.0);
    [m("margin-top"), m("margin-right"), m("margin-bottom"), m("margin-left")]
}

/// Sorts flow children into visual order when `flex-direction` is `*-reverse` or any child sets CSS `order != 0`.
pub fn sort_flow_children_visually<'a>(parent: &Element, children: Vec<&'a Element>) -> Vec<&'a Element> {
    let flex_dir = parent.styles.get("flex-direction").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default();
    let is_reverse = flex_dir.ends_with("-reverse");
    let has_order = children.iter().any(|c| c.styles.get("order").and_then(|o| o.trim().parse::<i32>().ok()).is_some_and(|v| v != 0));

    if !is_reverse && !has_order {
        return children;
    }

    let mut indexed: Vec<(usize, i32, &'a Element)> = children
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let ord = c.styles.get("order").and_then(|o| o.trim().parse::<i32>().ok()).unwrap_or(0);
            (i, ord, c)
        })
        .collect();

    indexed.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    if is_reverse {
        indexed.reverse();
    }
    indexed.into_iter().map(|(_, _, c)| c).collect()
}

/// Infers an [`AutoLayout`] for `el` from CSS `display: flex | inline-flex`, `display: grid | inline-grid`,
/// or vertical block-flow heuristics (§3.2).
pub fn infer_auto_layout(el: &Element, flow_children: &[&Element], infer_block_flow: bool, report: &mut ImportReport) -> Option<AutoLayout> {
    let display = el.styles.get("display").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_else(|| "block".into());

    if display == "flex" || display == "inline-flex" {
        return infer_flex_layout(el, flow_children, report);
    }
    if display == "grid" || display == "inline-grid" {
        return infer_grid_layout(el, flow_children, report);
    }
    if infer_block_flow {
        return infer_block_flow_layout(el, flow_children);
    }
    None
}

fn infer_flex_layout(el: &Element, flow_children: &[&Element], report: &mut ImportReport) -> Option<AutoLayout> {
    let font_size = el.styles.get("font-size").and_then(|s| parse_css_px(s, 16.0)).unwrap_or(16.0);

    for child in flow_children {
        let margins = parse_margins(&child.styles, font_size);
        if margins.iter().any(|&m| m < -0.5) {
            report.absolute_fallbacks.push(AbsoluteFallback {
                element_id: element_selector_label(el),
                tag: el.tag.clone(),
                reason: format!("child {} has negative margin", element_selector_label(child)),
            });
            return None;
        }
    }

    let flex_dir = el.styles.get("flex-direction").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_else(|| "row".into());
    let direction = if flex_dir.starts_with("column") { LayoutDirection::Vertical } else { LayoutDirection::Horizontal };

    let wrap_raw = el.styles.get("flex-wrap").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_else(|| "nowrap".into());
    let wrap = wrap_raw == "wrap" || wrap_raw == "wrap-reverse";

    if !wrap && flow_children.len() >= 2 && children_overlap_along_axis(flow_children, direction) {
        report.absolute_fallbacks.push(AbsoluteFallback {
            element_id: element_selector_label(el),
            tag: el.tag.clone(),
            reason: "flex children overlap along primary axis".into(),
        });
        return None;
    }

    let row_gap = el
        .styles
        .get("row-gap")
        .and_then(|v| parse_css_px(v, font_size))
        .or_else(|| el.styles.get("gap").and_then(|v| parse_css_px(v.split_whitespace().next().unwrap_or(""), font_size)))
        .unwrap_or(0.0)
        .max(0.0);
    let col_gap = el
        .styles
        .get("column-gap")
        .and_then(|v| parse_css_px(v, font_size))
        .or_else(|| {
            el.styles.get("gap").and_then(|v| {
                let mut parts = v.split_whitespace();
                let first = parts.next()?;
                let second = parts.next().unwrap_or(first);
                parse_css_px(second, font_size)
            })
        })
        .unwrap_or(row_gap)
        .max(0.0);

    let (mut item_spacing, counter_spacing) = match direction {
        LayoutDirection::Horizontal => (col_gap, row_gap),
        LayoutDirection::Vertical | LayoutDirection::Grid => (row_gap, col_gap),
    };

    if item_spacing == 0.0 && flow_children.len() >= 2 {
        if let Some(inferred_gap) = uniform_primary_gap(flow_children, direction) {
            if inferred_gap > 0.0 {
                item_spacing = inferred_gap;
            }
        }
    }

    let padding = parse_padding(&el.styles, font_size);
    let primary_align = map_justify_content(el.styles.get("justify-content").map(String::as_str));
    let counter_align = map_align_items(el.styles.get("align-items").map(String::as_str));
    let (w_sizing, h_sizing) = infer_container_sizing(el);
    let (primary_sizing, counter_sizing) = match direction {
        LayoutDirection::Horizontal => (w_sizing, h_sizing),
        LayoutDirection::Vertical => (h_sizing, w_sizing),
        LayoutDirection::Grid => (w_sizing, h_sizing),
    };

    Some(AutoLayout {
        direction,
        wrap,
        item_spacing,
        counter_spacing,
        padding,
        primary_align,
        counter_align,
        layout_version: 1,
        primary_sizing,
        counter_sizing,
        stroke_included: false,
        reverse_z: false,
        ..Default::default()
    })
}

fn infer_grid_layout(el: &Element, flow_children: &[&Element], report: &mut ImportReport) -> Option<AutoLayout> {
    let font_size = el.styles.get("font-size").and_then(|s| parse_css_px(s, 16.0)).unwrap_or(16.0);

    let raw_cols = el.styles.get("grid-template-columns").map(|s| s.trim()).unwrap_or("");
    let raw_rows = el.styles.get("grid-template-rows").map(|s| s.trim()).unwrap_or("");

    let col_tracks = parse_grid_tracks(raw_cols, font_size);
    let row_tracks = parse_grid_tracks(raw_rows, font_size);

    let ncols = if !col_tracks.is_empty() { col_tracks.len() } else { infer_grid_column_count_from_bboxes(flow_children) };

    if ncols == 0 {
        report.absolute_fallbacks.push(AbsoluteFallback {
            element_id: element_selector_label(el),
            tag: el.tag.clone(),
            reason: "could not determine grid column tracks".into(),
        });
        return None;
    }

    let nrows = if !row_tracks.is_empty() {
        row_tracks.len()
    } else if !flow_children.is_empty() {
        flow_children.len().div_ceil(ncols)
    } else {
        1
    };

    let row_gap = el
        .styles
        .get("row-gap")
        .and_then(|v| parse_css_px(v, font_size))
        .or_else(|| el.styles.get("gap").and_then(|v| parse_css_px(v.split_whitespace().next().unwrap_or(""), font_size)))
        .unwrap_or(0.0)
        .max(0.0);
    let col_gap = el
        .styles
        .get("column-gap")
        .and_then(|v| parse_css_px(v, font_size))
        .or_else(|| {
            el.styles.get("gap").and_then(|v| {
                let mut parts = v.split_whitespace();
                let first = parts.next()?;
                let second = parts.next().unwrap_or(first);
                parse_css_px(second, font_size)
            })
        })
        .unwrap_or(row_gap)
        .max(0.0);

    let padding = parse_padding(&el.styles, font_size);
    let mut grid_col_tracks = GridTracks::default();
    for (i, t) in col_tracks.iter().copied().enumerate() {
        if i < grid_col_tracks.len() {
            grid_col_tracks[i] = t;
        } else {
            grid_col_tracks.push(t);
        }
    }
    let mut grid_row_tracks = GridTracks::default();
    for (i, t) in row_tracks.iter().copied().enumerate() {
        if i < grid_row_tracks.len() {
            grid_row_tracks[i] = t;
        } else {
            grid_row_tracks.push(t);
        }
    }

    let (w_sizing, h_sizing) = infer_container_sizing(el);

    Some(AutoLayout {
        direction: LayoutDirection::Grid,
        wrap: true,
        item_spacing: col_gap,
        counter_spacing: row_gap,
        padding,
        primary_align: map_justify_content(el.styles.get("justify-content").map(String::as_str)),
        counter_align: map_align_items(el.styles.get("align-items").map(String::as_str)),
        layout_version: 1,
        primary_sizing: w_sizing,
        counter_sizing: h_sizing,
        stroke_included: false,
        reverse_z: false,
        grid_columns: ncols as u32,
        grid_rows: nrows as u32,
        grid_col_tracks,
        grid_row_tracks,
        ..Default::default()
    })
}

fn parse_grid_tracks(raw: &str, font_size: f32) -> Vec<GridTrack> {
    if raw.is_empty() || raw.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    let mut tracks = Vec::new();
    for tok in raw.split_whitespace() {
        let lower = tok.to_ascii_lowercase();
        if let Some(fr) = lower.strip_suffix("fr") {
            let v = fr.trim().parse::<f32>().unwrap_or(1.0).max(0.1);
            tracks.push(GridTrack::Flex(v));
        } else if lower == "auto" || lower == "min-content" || lower == "max-content" {
            tracks.push(GridTrack::Hug);
        } else if let Some(px) = parse_css_px(&lower, font_size) {
            if px > 0.0 {
                tracks.push(GridTrack::Fixed(px));
            }
        }
    }
    tracks
}

fn infer_grid_column_count_from_bboxes(children: &[&Element]) -> usize {
    if children.is_empty() {
        return 0;
    }
    let first_y = children[0].border_box.y;
    children.iter().take_while(|c| (c.border_box.y - first_y).abs() <= 4.0).count().max(1)
}

/// Block flow heuristic (§3.2):
/// When `display` is `block` / `flow-root` / `list-item` and the element has >= 2 in-flow children
/// whose bounding boxes stack vertically without overlapping, share a left edge within 1 px, and
/// have vertical gaps that vary by <= 1 px, infer a vertical [`AutoLayout`] frame.
fn infer_block_flow_layout(el: &Element, flow_children: &[&Element]) -> Option<AutoLayout> {
    let display = el.styles.get("display").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_else(|| "block".into());
    if !matches!(display.as_str(), "block" | "flow-root" | "list-item" | "") {
        return None;
    }
    if flow_children.len() < 2 {
        return None;
    }

    let first_x = flow_children[0].border_box.x;
    let mut gaps = Vec::with_capacity(flow_children.len() - 1);
    for (idx, child) in flow_children.iter().enumerate() {
        if (child.border_box.x - first_x).abs() > 1.0 {
            return None;
        }
        if idx > 0 {
            let prev = flow_children[idx - 1];
            let prev_bottom = prev.border_box.y + prev.border_box.height;
            let gap = child.border_box.y - prev_bottom;
            if gap < -0.5 {
                return None;
            }
            gaps.push(gap.max(0.0));
        }
    }

    let min_gap = gaps.iter().copied().fold(f32::INFINITY, f32::min);
    let max_gap = gaps.iter().copied().fold(0.0f32, f32::max);
    if (max_gap - min_gap) > 1.0 {
        return None;
    }
    let avg_gap = gaps.iter().sum::<f32>() / (gaps.len() as f32);

    let font_size = el.styles.get("font-size").and_then(|s| parse_css_px(s, 16.0)).unwrap_or(16.0);
    let padding = parse_padding(&el.styles, font_size);
    let (w_sizing, h_sizing) = infer_container_sizing(el);

    Some(AutoLayout {
        direction: LayoutDirection::Vertical,
        wrap: false,
        item_spacing: avg_gap.round(),
        counter_spacing: 0.0,
        padding,
        primary_align: LayoutAlign::Start,
        counter_align: LayoutAlign::Start,
        layout_version: 1,
        primary_sizing: h_sizing,
        counter_sizing: w_sizing,
        stroke_included: false,
        reverse_z: false,
        ..Default::default()
    })
}

fn children_overlap_along_axis(children: &[&Element], dir: LayoutDirection) -> bool {
    for i in 1..children.len() {
        let prev = children[i - 1];
        let cur = children[i];
        let gap = match dir {
            LayoutDirection::Horizontal | LayoutDirection::Grid => cur.border_box.x - (prev.border_box.x + prev.border_box.width),
            LayoutDirection::Vertical => cur.border_box.y - (prev.border_box.y + prev.border_box.height),
        };
        if gap < -1.0 {
            return true;
        }
    }
    false
}

fn uniform_primary_gap(children: &[&Element], dir: LayoutDirection) -> Option<f32> {
    if children.len() < 2 {
        return None;
    }
    let mut gaps = Vec::with_capacity(children.len() - 1);
    for i in 1..children.len() {
        let prev = children[i - 1];
        let cur = children[i];
        let gap = match dir {
            LayoutDirection::Horizontal | LayoutDirection::Grid => cur.border_box.x - (prev.border_box.x + prev.border_box.width),
            LayoutDirection::Vertical => cur.border_box.y - (prev.border_box.y + prev.border_box.height),
        };
        if gap < -0.5 {
            return None;
        }
        gaps.push(gap.max(0.0));
    }
    let min_g = gaps.iter().copied().fold(f32::INFINITY, f32::min);
    let max_g = gaps.iter().copied().fold(0.0f32, f32::max);
    if (max_g - min_g) <= 1.5 {
        Some((gaps.iter().sum::<f32>() / (gaps.len() as f32)).round())
    } else {
        None
    }
}

pub fn map_justify_content(raw: Option<&str>) -> LayoutAlign {
    let Some(s) = raw else {
        return LayoutAlign::Start;
    };
    match s.trim().to_ascii_lowercase().as_str() {
        "center" => LayoutAlign::Center,
        "flex-end" | "end" | "right" => LayoutAlign::End,
        "space-between" => LayoutAlign::SpaceBetween,
        "space-around" => LayoutAlign::SpaceAround,
        "space-evenly" => LayoutAlign::SpaceEvenly,
        _ => LayoutAlign::Start,
    }
}

pub fn map_align_items(raw: Option<&str>) -> LayoutAlign {
    let Some(s) = raw else {
        return LayoutAlign::Start;
    };
    match s.trim().to_ascii_lowercase().as_str() {
        "center" => LayoutAlign::Center,
        "flex-end" | "end" | "self-end" => LayoutAlign::End,
        "baseline" | "first baseline" | "last baseline" => LayoutAlign::Baseline,
        _ => LayoutAlign::Start,
    }
}

fn infer_container_sizing(el: &Element) -> (Sizing, Sizing) {
    let display = el.styles.get("display").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default();
    let w_sizing = if display.starts_with("inline") { Sizing::Hug } else { Sizing::Fixed };
    let h_sizing = Sizing::Hug;
    (w_sizing, h_sizing)
}

/// Maps child sizing (`Fill` / `Fixed` / `Hug`), `grow`, `align_self`, and min/max constraints
/// for `child` inside `parent` with `parent_al`.
pub fn infer_layout_child(child: &Element, parent: &Element, parent_al: Option<&AutoLayout>) -> LayoutChild {
    let font_size = child.styles.get("font-size").and_then(|s| parse_css_px(s, 16.0)).unwrap_or(16.0);

    let absolute = is_out_of_flow(child);
    let grow = child.styles.get("flex-grow").and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(0.0).max(0.0);

    let align_self_raw = child.styles.get("align-self").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_else(|| "auto".into());
    let align_self = match align_self_raw.as_str() {
        "flex-start" | "start" | "self-start" => AlignSelf::Start,
        "center" => AlignSelf::Center,
        "flex-end" | "end" | "self-end" => AlignSelf::End,
        "stretch" => AlignSelf::Stretch,
        "baseline" => AlignSelf::Baseline,
        _ => AlignSelf::Auto,
    };

    let parent_align_items = parent.styles.get("align-items").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_else(|| "normal".into());

    let parent_pad = parse_padding(&parent.styles, font_size);
    let parent_border = parse_border_widths(&parent.styles, font_size);
    let content_w = if parent.content_box.width > 0.0 {
        parent.content_box.width
    } else {
        (parent.border_box.width - parent_pad[1] - parent_pad[3] - parent_border[1] - parent_border[3]).max(0.0)
    };
    let content_h = if parent.content_box.height > 0.0 {
        parent.content_box.height
    } else {
        (parent.border_box.height - parent_pad[0] - parent_pad[2] - parent_border[0] - parent_border[2]).max(0.0)
    };

    let fills_cross_w = content_w > 0.0 && (child.border_box.width - content_w).abs() <= 2.0;
    let fills_cross_h = content_h > 0.0 && (child.border_box.height - content_h).abs() <= 2.0;

    let (horizontal, vertical) = match parent_al.map(|al| al.direction) {
        Some(LayoutDirection::Horizontal) => {
            let h = if grow > 0.0 { Sizing::Fill } else { Sizing::Fixed };
            let stretches = align_self == AlignSelf::Stretch
                || (align_self == AlignSelf::Auto && matches!(parent_align_items.as_str(), "stretch" | "normal"))
                || child.styles.get("height").is_some_and(|v| v.trim() == "100%");
            let v = if stretches && fills_cross_h { Sizing::Fill } else { Sizing::Fixed };
            (h, v)
        }
        Some(LayoutDirection::Vertical) => {
            let v = if grow > 0.0 { Sizing::Fill } else { Sizing::Fixed };
            let stretches = align_self == AlignSelf::Stretch
                || (align_self == AlignSelf::Auto && matches!(parent_align_items.as_str(), "stretch" | "normal"))
                || child.styles.get("width").is_some_and(|v| v.trim() == "100%");
            let h = if stretches && fills_cross_w { Sizing::Fill } else { Sizing::Fixed };
            (h, v)
        }
        Some(LayoutDirection::Grid) => (if fills_cross_w { Sizing::Fill } else { Sizing::Fixed }, Sizing::Fixed),
        None => (Sizing::Fixed, Sizing::Fixed),
    };

    let min_w = child.styles.get("min-width").and_then(|s| parse_css_px_or_percent(s, content_w, font_size)).filter(|&v| v > 0.0);
    let max_w = child.styles.get("max-width").and_then(|s| parse_css_px_or_percent(s, content_w, font_size)).filter(|&v| v > 0.0 && v < 100_000.0);
    let min_h = child.styles.get("min-height").and_then(|s| parse_css_px_or_percent(s, content_h, font_size)).filter(|&v| v > 0.0);
    let max_h = child.styles.get("max-height").and_then(|s| parse_css_px_or_percent(s, content_h, font_size)).filter(|&v| v > 0.0 && v < 100_000.0);

    LayoutChild { absolute, horizontal, vertical, grow, align_self, min_w, max_w, min_h, max_h, ..Default::default() }
}
