//! Filling polygons that may be concave (L shapes, arrows, synoptic
//! outlines), which egui's `convex_polygon` fills wrongly.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use eframe::egui::{self, Color32, Mesh, Pos2, Shape, Stroke};

/// Outlines with more points than this are filled as a fan, as before:
/// ear clipping is quadratic or worse in the number of points.
const MAX_POINTS: usize = 1000;

/// Fill the polygon `screen`, the points `display` placed on the screen.
/// A convex one is filled by egui, with anti-aliased edges; a concave one
/// is cut into triangles, worked out once for its display points (moving
/// or zooming it does not change them) and kept in egui's memory.
pub fn fill(painter: &egui::Painter, display: &[(f64, f64)], screen: Vec<Pos2>, color: Color32) {
    if screen.len() < 3 || color.a() == 0 {
        return;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (x, y) in display {
        x.to_bits().hash(&mut hasher);
        y.to_bits().hash(&mut hasher);
    }
    let id = egui::Id::new(("bobview-polygon", hasher.finish()));
    let ctx = painter.ctx();
    let cached: Option<Arc<Option<Vec<u32>>>> = ctx.memory_mut(|m| m.data.get_temp(id));
    let triangles = cached.unwrap_or_else(|| {
        let pts: Vec<Pos2> = display
            .iter()
            .map(|(x, y)| Pos2::new(*x as f32, *y as f32))
            .collect();
        let t = Arc::new(triangulate(&pts));
        ctx.memory_mut(|m| m.data.insert_temp(id, t.clone()));
        t
    });
    match triangles.as_ref() {
        Some(indices) => {
            let mut mesh = Mesh::default();
            for p in &screen {
                mesh.colored_vertex(*p, color);
            }
            mesh.indices = indices.clone();
            painter.add(Shape::mesh(mesh));
        }
        _ => {
            painter.add(Shape::convex_polygon(screen, color, Stroke::NONE));
        }
    }
}

/// Twice the signed area of `pts`: positive when they go anticlockwise in
/// y-up coordinates (clockwise on a y-down screen).
fn area2(pts: &[Pos2]) -> f32 {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum()
}

fn cross(o: Pos2, a: Pos2, b: Pos2) -> f32 {
    (a - o).x * (b - o).y - (a - o).y * (b - o).x
}

/// Whether `p` is inside or on the triangle `a b c`, which turns the way
/// `sign` says.
fn in_triangle(p: Pos2, a: Pos2, b: Pos2, c: Pos2, sign: f32) -> bool {
    cross(a, b, p) * sign >= 0.0 && cross(b, c, p) * sign >= 0.0 && cross(c, a, p) * sign >= 0.0
}

/// The triangles filling the outline `pts`, three indices into it each, by
/// ear clipping; `None` for a convex outline (egui fills those itself) or
/// one too big to cut. An outline that crosses itself runs out of ears at
/// some point, and what is left is filled as a fan.
pub fn triangulate(pts: &[Pos2]) -> Option<Vec<u32>> {
    let n = pts.len();
    if !(4..=MAX_POINTS).contains(&n) {
        return None;
    }
    let area = area2(pts);
    if area == 0.0 {
        return None;
    }
    let sign = area.signum();
    // Convex: every corner turns the same way as the whole outline.
    let turns = |i: usize| cross(pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]) * sign;
    if (0..n).all(|i| turns(i) >= 0.0) {
        return None;
    }

    let mut left: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(3 * (n - 2));
    while left.len() > 3 {
        let m = left.len();
        let mut cut = None;
        for i in 0..m {
            let (a, b, c) = (left[(i + m - 1) % m], left[i], left[(i + 1) % m]);
            let turn = cross(pts[a], pts[b], pts[c]) * sign;
            if turn == 0.0 {
                // A point on a straight edge (or repeated) adds nothing.
                cut = Some((i, None));
                break;
            }
            if turn < 0.0 {
                continue;
            }
            // An ear: no other point of the outline in it.
            let blocked = left.iter().any(|&k| {
                k != a
                    && k != b
                    && k != c
                    && pts[k] != pts[a]
                    && pts[k] != pts[b]
                    && pts[k] != pts[c]
                    && in_triangle(pts[k], pts[a], pts[b], pts[c], sign)
            });
            if !blocked {
                cut = Some((i, Some([a, b, c])));
                break;
            }
        }
        match cut {
            Some((i, tri)) => {
                out.extend(tri.into_iter().flatten().map(|k| k as u32));
                left.remove(i);
            }
            None => {
                // No ear: the outline crosses itself. Fan out the rest.
                for k in 1..left.len() - 1 {
                    out.extend([left[0], left[k], left[k + 1]].map(|k| k as u32));
                }
                return Some(out);
            }
        }
    }
    out.extend(left.iter().map(|&k| k as u32));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(p: &[(f32, f32)]) -> Vec<Pos2> {
        p.iter().map(|&(x, y)| Pos2::new(x, y)).collect()
    }

    /// The area the triangles cover, which for a correct cut is the
    /// polygon's own.
    fn covered(p: &[Pos2], tris: &[u32]) -> f32 {
        tris.chunks(3)
            .map(|t| cross(p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]).abs() / 2.0)
            .sum()
    }

    /// Whether `q` is inside any of the triangles.
    fn filled(p: &[Pos2], tris: &[u32], q: Pos2) -> bool {
        tris.chunks(3).any(|t| {
            let (a, b, c) = (p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]);
            in_triangle(q, a, b, c, cross(a, b, c).signum())
        })
    }

    #[test]
    fn convex_polygons_are_left_to_egui() {
        assert_eq!(
            triangulate(&pts(&[(0., 0.), (10., 0.), (10., 10.), (0., 10.)])),
            None
        );
        assert_eq!(triangulate(&pts(&[(0., 0.), (10., 0.), (5., 10.)])), None);
    }

    #[test]
    fn an_l_shape_is_filled_but_not_its_notch() {
        // An L, both ways round: a convex fill would cover the notch.
        let l = pts(&[
            (0., 0.),
            (10., 0.),
            (10., 4.),
            (4., 4.),
            (4., 10.),
            (0., 10.),
        ]);
        for outline in [l.clone(), l.into_iter().rev().collect()] {
            let tris = triangulate(&outline).expect("concave");
            assert_eq!(tris.len(), 3 * (outline.len() - 2));
            assert_eq!(covered(&outline, &tris), 64.0);
            assert!(filled(&outline, &tris, Pos2::new(2.0, 8.0)));
            assert!(filled(&outline, &tris, Pos2::new(8.0, 2.0)));
            assert!(!filled(&outline, &tris, Pos2::new(8.0, 8.0)), "the notch");
        }
    }

    #[test]
    fn an_arrow_is_filled_but_not_beside_its_shaft() {
        let arrow = pts(&[
            (0., 3.),
            (6., 3.),
            (6., 0.),
            (10., 5.),
            (6., 10.),
            (6., 7.),
            (0., 7.),
        ]);
        let tris = triangulate(&arrow).expect("concave");
        // Shaft 6 x 4, head 4 x 10 / 2.
        assert_eq!(covered(&arrow, &tris), 44.0);
        assert!(!filled(&arrow, &tris, Pos2::new(3.0, 1.0)));
        assert!(!filled(&arrow, &tris, Pos2::new(3.0, 9.0)));
        assert!(filled(&arrow, &tris, Pos2::new(8.0, 5.0)));
    }

    #[test]
    fn straight_runs_and_crossed_outlines_still_fill() {
        // A point in the middle of an edge.
        let u = pts(&[
            (0., 0.),
            (3., 0.),
            (3., 8.),
            (6., 8.),
            (6., 0.),
            (9., 0.),
            (9., 10.),
            (4., 10.),
            (0., 10.),
        ]);
        let tris = triangulate(&u).expect("concave");
        assert_eq!(covered(&u, &tris), 90.0 - 24.0);
        // A bow tie crosses itself: it gets some fill and no panic.
        let bow = pts(&[(0., 0.), (10., 10.), (10., 0.), (0., 10.), (-1., 5.)]);
        assert!(triangulate(&bow).is_some_and(|t| t.len() % 3 == 0));
    }
}
