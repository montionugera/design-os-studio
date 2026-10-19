//! `LintEnv` implementation backed by `cosmic-text` shaping and the CPU renderer.

use crate::{render_page, resolve_font, text_geometry, Viewport};
use designos_doc::lint::{Backdrop, LintEnv, TextMetrics};
use designos_doc::{Color, Document, Node, NodeId, NodeKind, PaintKind, Rect, TextAutoResize, TextProps, Vec2};

/// Real text-measurement and backdrop-sampling environment for [`designos_doc::lint`].
#[derive(Debug, Default, Clone, Copy)]
pub struct RenderLintEnv;

impl LintEnv for RenderLintEnv {
    fn has_text_engine(&self) -> bool {
        true
    }

    fn text_metrics(&self, _node: &Node, props: &TextProps, size: Vec2) -> Option<TextMetrics> {
        let geom = text_geometry(props, size);
        let line_count = geom.lines.len().max(1);
        let content_height = geom.height;
        let widest_line = geom.lines.iter().map(|l| l.width).fold(geom.width, f32::max);

        let mut breaks_inside_word = Vec::new();
        for pair in geom.lines.windows(2) {
            let (prev, next) = (&pair[0], &pair[1]);
            if prev.para == next.para && prev.end == next.start && next.start > 0 && next.start < props.content.len() {
                let before_alnum = props.content[..next.start].chars().next_back().is_some_and(char::is_alphanumeric);
                let after_alnum = props.content[next.start..].chars().next().is_some_and(char::is_alphanumeric);
                if before_alnum && after_alnum {
                    breaks_inside_word.push(next.start);
                }
            }
        }

        let max_by_height = if matches!(props.auto_resize, TextAutoResize::None | TextAutoResize::Truncate) && size.y > 0.0 {
            geom.lines.iter().filter(|l| l.top + l.height <= size.y + 0.5).count().max(1)
        } else {
            line_count
        };

        let visible_lines = match props.max_lines {
            Some(max_l) if props.auto_resize == TextAutoResize::Truncate => line_count.min(max_l.max(1) as usize).min(max_by_height),
            _ => line_count.min(max_by_height),
        };

        let substituted_family = if !props.style.font_family.is_empty() {
            resolve_font(&props.style.font_family, props.style.font_weight, props.style.italic)
                .and_then(|resolved| (!resolved.family.eq_ignore_ascii_case(&props.style.font_family)).then_some(resolved.family))
        } else {
            None
        };

        Some(TextMetrics { line_count, visible_lines, content_height, widest_line, breaks_inside_word, substituted_family })
    }

    fn backdrop(&self, doc: &Document, node: NodeId, rect: Rect) -> Option<Backdrop> {
        let page_id = doc.page_of(node)?;
        if rect.w > 0.0 && rect.h > 0.0 && doc.nodes.len() <= 512 {
            if let Some(mut pm) = tiny_skia::Pixmap::new(3, 3) {
                let mut temp = doc.clone();
                if let Some(n) = temp.get_mut(node) {
                    n.visible = false;
                }
                let cx = rect.x + rect.w * 0.5;
                let cy = rect.y + rect.h * 0.5;
                let vp = Viewport { offset: Vec2::new(cx - 1.0, cy - 1.0), zoom: 1.0, dpr: 1.0 };
                render_page(&temp, page_id, &vp, &mut pm);
                let pixels = pm.pixels();
                let center = pixels[4].demultiply();
                let c = Color {
                    r: f32::from(center.red()) / 255.0,
                    g: f32::from(center.green()) / 255.0,
                    b: f32::from(center.blue()) / 255.0,
                    a: f32::from(center.alpha()) / 255.0,
                };
                let uniform = pixels.iter().all(|px| {
                    let u = px.demultiply();
                    u.red().abs_diff(center.red()) <= 2 && u.green().abs_diff(center.green()) <= 2 && u.blue().abs_diff(center.blue()) <= 2
                });
                return Some(Backdrop { color: c, uniform });
            }
        }
        ancestor_backdrop(doc, node)
    }
}

fn ancestor_backdrop(doc: &Document, id: NodeId) -> Option<Backdrop> {
    let mut cur = doc.get(id).and_then(|n| n.parent);
    while let Some(pid) = cur {
        let Some(p) = doc.get(pid) else { break };
        if let NodeKind::Page { background } = p.kind {
            return Some(Backdrop { color: background, uniform: true });
        }
        if let Some(fill) = p.fills.iter().rev().find(|f| f.visible && f.opacity >= 0.5) {
            if let PaintKind::Solid(c) = fill.kind {
                if c.a >= 0.5 {
                    return Some(Backdrop { color: c, uniform: true });
                }
            }
        }
        cur = p.parent;
    }
    None
}
