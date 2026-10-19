//! Shared Figma constraint solver for container resize and instance expansion (#691).
//!
//! Implements the measured Figma constraint rules for `Min`, `Max`, `Center`,
//! `Stretch`, and `Scale` across both unrotated and rotated/skewed children,
//! including half-away-from-zero extent rounding and sub-1px sizes.

use crate::{Affine, Constraint, Document, Node, NodeId, Vec2};

/// Minimum axis extent (`2^-12`) below which per-axis scale falls back to `1.0`.
const MIN_EXTENT: f64 = 1.0 / 4096.0;

/// Rounds `x` to the nearest integer, ties rounding half away from zero.
#[inline]
pub fn round_half_away_from_zero(x: f64) -> f64 {
    x.round()
}

#[inline]
fn move_axis_edges(constraint: Constraint, e0: f64, e1: f64, old_p: f64, new_p: f64) -> (f64, f64) {
    match constraint {
        Constraint::Min => (e0, e1),
        Constraint::Max => {
            let delta = round_half_away_from_zero(new_p) - round_half_away_from_zero(old_p);
            (e0 + delta, e1 + delta)
        }
        Constraint::Center => {
            let delta = round_half_away_from_zero(new_p * 0.5) - round_half_away_from_zero(old_p * 0.5);
            (e0 + delta, e1 + delta)
        }
        Constraint::Stretch => {
            let delta = round_half_away_from_zero(new_p) - round_half_away_from_zero(old_p);
            (e0, e1 + delta)
        }
        Constraint::Scale => {
            if old_p.abs() < MIN_EXTENT {
                (e0, e1)
            } else {
                let r = new_p / old_p;
                (e0 * r, e1 * r)
            }
        }
    }
}

/// Computes the new `(transform, size)` of `child` when its parent container
/// resizes from `old` to `new`.
///
/// Returns `None` when the resulting matrix determinant is `0` or non-finite,
/// in which case the caller leaves `child` unchanged.
pub fn constrain_child(child: &Node, old: Vec2, new: Vec2) -> Option<(Affine, Vec2)> {
    let w = child.size.x as f64;
    let h = child.size.y as f64;
    let a = child.transform.a as f64;
    let b = child.transform.b as f64;
    let c = child.transform.c as f64;
    let d = child.transform.d as f64;
    let tx = child.transform.tx as f64;
    let ty = child.transform.ty as f64;

    // 1. Axis-aligned bounds B of child's local (0, 0, w, h) in parent's axes.
    let aw = a * w;
    let ch = c * h;
    let bw = b * w;
    let dh = d * h;

    let bx0 = tx + aw.min(0.0) + ch.min(0.0);
    let bx1 = tx + aw.max(0.0) + ch.max(0.0);
    let by0 = ty + bw.min(0.0) + dh.min(0.0);
    let by1 = ty + bw.max(0.0) + dh.max(0.0);

    // 2. Move B's min and max edges per axis according to the child's constraints.
    let (nbx0, nbx1) = move_axis_edges(child.constraints.horizontal, bx0, bx1, old.x as f64, new.x as f64);
    let (nby0, nby1) = move_axis_edges(child.constraints.vertical, by0, by1, old.y as f64, new.y as f64);

    // 3. Per-axis scale s = new_extent / old_extent (1.0 when old_extent < 2^-12).
    let old_ext_x = bx1 - bx0;
    let new_ext_x = nbx1 - nbx0;
    let sx = if old_ext_x < MIN_EXTENT { 1.0 } else { new_ext_x / old_ext_x };

    let old_ext_y = by1 - by0;
    let new_ext_y = nby1 - nby0;
    let sy = if old_ext_y < MIN_EXTENT { 1.0 } else { new_ext_y / old_ext_y };

    // 4. Apply map p -> B'.min + s * (p - B.min) after child's matrix.
    let ma = sx * a;
    let mc = sx * c;
    let mtx = nbx0 + sx * (tx - bx0);
    let mb = sy * b;
    let md = sy * d;
    let mty = nby0 + sy * (ty - by0);

    let det = ma * md - mb * mc;
    if !det.is_finite() || det.abs() <= 1e-18 {
        return None;
    }

    // 5. Split M' into size and transform (keeping skew in the transform).
    let kx = ma.hypot(mb);
    if !kx.is_finite() || kx <= 1e-18 {
        return None;
    }
    let ky = det.abs() / kx;
    if !ky.is_finite() || ky <= 1e-18 {
        return None;
    }

    let new_size = Vec2::new((w * kx) as f32, (h * ky) as f32);
    let new_transform = Affine { a: (ma / kx) as f32, b: (mb / kx) as f32, c: (mc / ky) as f32, d: (md / ky) as f32, tx: mtx as f32, ty: mty as f32 };
    Some((new_transform, new_size))
}

/// Plans a recursive constraint pass rooted at `parent` when `parent` resizes
/// from `old` to `new`, up to `depth` levels deep.
///
/// Parents are solved before their children; each child uses its immediate
/// parent's new size. In-flow children of auto-layout parents are skipped,
/// while absolute children are solved.
pub fn plan_constraint_pass(doc: &Document, parent: NodeId, old: Vec2, new: Vec2, depth: usize) -> Vec<(NodeId, Affine, Vec2)> {
    let mut out = Vec::new();
    plan_constraint_pass_inner(doc, parent, old, new, depth, &mut out);
    out
}

fn plan_constraint_pass_inner(doc: &Document, parent: NodeId, old: Vec2, new: Vec2, depth: usize, out: &mut Vec<(NodeId, Affine, Vec2)>) {
    if depth == 0 {
        return;
    }
    let Some(p) = doc.get(parent) else {
        return;
    };
    let has_al = p.kind.frame_props().and_then(|f| f.auto_layout.as_ref()).is_some();
    for &c in &p.children {
        let Some(k) = doc.get(c) else { continue };
        if has_al && !k.layout_child.absolute {
            continue;
        }
        let Some((new_transform, new_size)) = constrain_child(k, old, new) else {
            continue;
        };
        let old_child = k.size;
        let size_changed = (new_size.x - old_child.x).abs() > 1e-4 || (new_size.y - old_child.y).abs() > 1e-4;
        let is_container = k.kind.is_container();
        out.push((c, new_transform, new_size));
        if size_changed && is_container {
            plan_constraint_pass_inner(doc, c, old_child, new_size, depth - 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Constraints, NodeKind, Rect};

    #[test]
    fn half_away_from_zero_rounding_table() {
        assert_eq!(round_half_away_from_zero(-0.5), -1.0);
        assert_eq!(round_half_away_from_zero(0.5), 1.0);
        assert_eq!(round_half_away_from_zero(2.5), 3.0);
        assert_eq!(round_half_away_from_zero(-2.5), -3.0);
        assert_eq!(round_half_away_from_zero(0.0), 0.0);
    }

    #[test]
    fn rotated_90_stretch_stretch_gives_100_by_140_without_skew() {
        let mut child = Node::rectangle(NodeId(2), Rect::new(0.0, 0.0, 100.0, 40.0));
        child.transform = Affine { a: 0.0, b: 1.0, c: -1.0, d: 0.0, tx: 60.0, ty: 20.0 };
        child.constraints = Constraints { horizontal: Constraint::Stretch, vertical: Constraint::Stretch };
        let (t, sz) = constrain_child(&child, Vec2::new(200.0, 200.0), Vec2::new(300.0, 200.0)).unwrap();
        assert!((sz.x - 100.0).abs() < 1e-4, "expected w=100, got {}", sz.x);
        assert!((sz.y - 140.0).abs() < 1e-4, "expected h=140, got {}", sz.y);
        // Column dot product a*c + b*d == 0 means no skew.
        let dot = t.a * t.c + t.b * t.d;
        assert!(dot.abs() < 1e-5, "expected no skew, dot={dot}");
        assert!((t.b * t.c - (-1.0)).abs() < 1e-5);
    }

    #[test]
    fn center_rounding_uses_rounded_extents() {
        let mut child = Node::rectangle(NodeId(2), Rect::new(10.0, 10.0, 20.0, 20.0));
        child.constraints = Constraints { horizontal: Constraint::Center, vertical: Constraint::Center };
        // Parent 101 -> 150 moves centred child by r(75) - r(50.5) = 75 - 51 = 24, not 24.5.
        let (t, sz) = constrain_child(&child, Vec2::new(101.0, 101.0), Vec2::new(150.0, 150.0)).unwrap();
        assert_eq!(sz, Vec2::new(20.0, 20.0));
        assert_eq!(t.tx, 34.0);
        assert_eq!(t.ty, 34.0);
    }

    #[test]
    fn sub_one_child_keeps_its_size() {
        // A 0.5 px horizontal divider with Stretch horizontal, Min vertical keeps 0.5 height.
        let mut half_line = Node::rectangle(NodeId(2), Rect::new(0.0, 10.0, 100.0, 0.5));
        half_line.constraints = Constraints { horizontal: Constraint::Stretch, vertical: Constraint::Min };
        let (_, sz) = constrain_child(&half_line, Vec2::new(100.0, 100.0), Vec2::new(200.0, 100.0)).unwrap();
        assert_eq!(sz, Vec2::new(200.0, 0.5));

        // A zero-thickness line stays zero.
        let mut zero_line = Node::new(NodeId(3), NodeKind::Line);
        zero_line.size = Vec2::new(100.0, 0.0);
        zero_line.constraints = Constraints { horizontal: Constraint::Stretch, vertical: Constraint::Min };
        let (_, sz0) = constrain_child(&zero_line, Vec2::new(100.0, 100.0), Vec2::new(250.0, 100.0)).unwrap();
        assert_eq!(sz0, Vec2::new(250.0, 0.0));
    }

    #[test]
    fn rotated_30_stretch_stretch_gives_175_130_by_44_266_with_skew() {
        let mut child = Node::rectangle(NodeId(2), Rect::new(0.0, 0.0, 100.0, 40.0));
        child.transform = Affine::rotate(30.0_f32.to_radians());
        child.constraints = Constraints { horizontal: Constraint::Stretch, vertical: Constraint::Stretch };
        let (t, sz) = constrain_child(&child, Vec2::new(200.0, 200.0), Vec2::new(300.0, 200.0)).unwrap();
        assert!((sz.x - 175.130).abs() < 1e-3, "expected w≈175.130, got {}", sz.x);
        assert!((sz.y - 44.266).abs() < 1e-3, "expected h≈44.266, got {}", sz.y);
        // Non-uniform stretch of a 30-degree rotated rectangle introduces skew (columns not orthogonal).
        let dot = t.a * t.c + t.b * t.d;
        assert!(dot.abs() > 0.1, "expected skew in transform, dot={dot}");
    }

    #[test]
    fn det_zero_leaves_the_child_unchanged() {
        let mut child = Node::rectangle(NodeId(2), Rect::new(10.0, 10.0, 50.0, 50.0));
        child.constraints = Constraints { horizontal: Constraint::Scale, vertical: Constraint::Scale };
        // Parent width collapses to 0 -> det(M') == 0 -> returns None.
        assert!(constrain_child(&child, Vec2::new(100.0, 100.0), Vec2::new(0.0, 100.0)).is_none());
    }

    #[test]
    fn old_extent_under_2_pow_minus_12_scales_by_one() {
        let mut child = Node::rectangle(NodeId(2), Rect::new(10.0, 20.0, 1e-5, 40.0));
        child.constraints = Constraints { horizontal: Constraint::Stretch, vertical: Constraint::Stretch };
        let (t, sz) = constrain_child(&child, Vec2::new(100.0, 100.0), Vec2::new(200.0, 150.0)).unwrap();
        // Horizontal old extent is 1e-5 < 2^-12, so sx = 1.0 (width unchanged), while vertical grows by 50.
        assert!((sz.x - 1e-5).abs() < 1e-9);
        assert!((sz.y - 90.0).abs() < 1e-4);
        assert_eq!(t.tx, 10.0);
        assert_eq!(t.ty, 20.0);
    }

    #[test]
    fn no_local_constraint_copies_remain() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let rest_import = std::fs::read_to_string(root.join("designos-fig/src/rest_import.rs")).unwrap();
        let app_layout = std::fs::read_to_string(root.join("designos-app/src/layout.rs")).unwrap();
        let prod_layout = app_layout.split("#[cfg(test)]").next().unwrap_or(&app_layout);
        assert!(!rest_import.contains("Constraint::Stretch =>"), "rest_import.rs must not hold a local per-axis match on Constraint");
        assert!(!prod_layout.contains("Constraint::Stretch =>"), "layout.rs production code must not hold a local per-axis match on Constraint");
        assert!(rest_import.contains("plan_constraint_pass"), "rest_import.rs must call plan_constraint_pass");
        assert!(prod_layout.contains("plan_constraint_pass"), "layout.rs must call plan_constraint_pass");
    }
}
