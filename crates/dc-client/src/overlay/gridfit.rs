//! A small grid fitter for monochrome text: the straight edges of a glyph outline are moved onto pixel boundaries
//! and the strokes between them keep a whole-pixel width, the points between edges follow by linear interpolation.
//!
//! A bytecode interpreter would run the face's own TrueType instructions; DeCombat does not have one. This
//! fitter gives the same kind of result on simple sans faces (stems and bars a whole number of pixels wide, flat
//! tops and the baseline on pixel rows), so monochrome text is crisp instead of having half-covered edges decide
//! its weight.
//!
//! PUBLIC: TrueType scan conversion and grid fitting concepts (pixel-boundary edges, stem widths, interpolation
//! of untouched points between touched ones, as in the TrueType IUP instruction); the clockwise outer contour
//! convention of `glyf` outlines (filled area right of the travel direction, y up).

use super::ttf::Outline;

/// One straight edge across an axis: its coordinate and whether the filled side lies above it (towards larger
/// coordinates).
#[derive(Debug, Clone, Copy)]
struct Edge {
    at: f32,
    fill_above: bool,
}

/// Edges of `axis` (0 = vertical edges, x; 1 = horizontal edges, y): on-curve points with a neighbour at the same
/// coordinate (a straight segment or a curve's flat extreme).
fn edges(contours: &[Vec<(f32, f32, bool)>], axis: usize) -> Vec<Edge> {
    let c = |p: &(f32, f32, bool)| if axis == 0 { p.0 } else { p.1 };
    let o = |p: &(f32, f32, bool)| if axis == 0 { p.1 } else { p.0 };
    let mut out: Vec<Edge> = Vec::new();
    for ct in contours {
        let n = ct.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let p = &ct[i];
            if !p.2 {
                continue;
            }
            let prev = &ct[(i + n - 1) % n];
            let next = &ct[(i + 1) % n];
            let d = if (c(next) - c(p)).abs() < 1e-3 && (o(next) - o(p)).abs() > 1e-3 {
                o(next) - o(p)
            } else if (c(prev) - c(p)).abs() < 1e-3 && (o(p) - o(prev)).abs() > 1e-3 {
                o(p) - o(prev)
            } else {
                continue;
            };
            // filled right of the travel direction (y up): travelling +x fills below, travelling +y fills to +x
            let fill_above = if axis == 0 { d > 0.0 } else { d < 0.0 };
            if !out
                .iter()
                .any(|e| (e.at - c(p)).abs() < 1e-3 && e.fill_above == fill_above)
            {
                out.push(Edge {
                    at: c(p),
                    fill_above,
                });
            }
        }
    }
    out
}

/// Where each edge goes: strokes (an edge with the fill above and the nearest edge with the fill below it, at most
/// `max_stroke` apart) keep a whole width of at least one pixel and are anchored at their outer edge (the one
/// away from `mid`); single edges round to the nearest boundary. A stroke's width rounds after taking off `thin`
/// (horizontal bars of a sans face are drawn thinner than its stems at text sizes).
fn targets(edges: &[Edge], max_stroke: f32, mid: f32, thin: f32) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = Vec::new();
    // closest pairs first: an edge belongs to the stroke it bounds most tightly
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for (li, l) in edges.iter().enumerate().filter(|(_, e)| e.fill_above) {
        for (hi, h) in edges.iter().enumerate().filter(|(_, e)| !e.fill_above) {
            let d = h.at - l.at;
            if d > 0.0 && d <= max_stroke {
                pairs.push((d, li, hi));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut used = vec![false; edges.len()];
    for (d, li, hi) in pairs {
        if used[li] || used[hi] {
            continue;
        }
        used[li] = true;
        used[hi] = true;
        let (low, high) = (edges[li].at, edges[hi].at);
        let w = (d - thin).round().max(1.0);
        if (low + high) * 0.5 >= mid {
            let t = high.round();
            out.push((high, t));
            out.push((low, t - w));
        } else {
            let b = low.round();
            out.push((low, b));
            out.push((high, b + w));
        }
    }
    for (i, e) in edges.iter().enumerate() {
        if !used[i] {
            out.push((e.at, e.at.round()));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.dedup_by(|b, a| (a.0 - b.0).abs() < 1e-3);
    // keep the order of the edges (no contour folds over)
    for i in 1..out.len() {
        if out[i].1 < out[i - 1].1 {
            out[i].1 = out[i - 1].1;
        }
    }
    out
}

/// Maps a coordinate through the sorted (from, to) pairs: exact at the edges, linear between, shifted outside.
fn map(t: &[(f32, f32)], v: f32) -> f32 {
    let Some(first) = t.first() else {
        return v;
    };
    let last = t[t.len() - 1];
    if v <= first.0 {
        return v + first.1 - first.0;
    }
    if v >= last.0 {
        return v + last.1 - last.0;
    }
    let i = t.partition_point(|p| p.0 <= v);
    let (a, b) = (t[i - 1], t[i]);
    if b.0 - a.0 < 1e-6 {
        return a.1;
    }
    a.1 + (v - a.0) * (b.1 - a.1) / (b.0 - a.0)
}

/// The outline scaled to pixels (`scale` pixels per unit, y up) with its edges fitted to the pixel grid; strokes
/// wider than `max_stroke` pixels are treated as two single edges.
pub fn fit(outline: &Outline, scale: f32, max_stroke: f32) -> Outline {
    let mut contours: Vec<Vec<(f32, f32, bool)>> = outline
        .contours
        .iter()
        .map(|c| {
            c.iter()
                .map(|&(x, y, on)| (x * scale, y * scale, on))
                .collect()
        })
        .collect();
    let Some((x0, y0, x1, y1)) = (Outline {
        contours: contours.clone(),
    })
    .bounds() else {
        return Outline { contours };
    };
    for axis in 0..2 {
        let mid = if axis == 0 {
            (x0 + x1) * 0.5
        } else {
            (y0 + y1) * 0.5
        };
        // OBSERVED: shop-guns/cpp/shop_bb00_weapons_c0_p1.png (Tahoma Bold 12 ppem: stems 2.19
        // px drawn 2 wide, the H bar 1.66 px drawn 1 high) (CLI-780)
        let thin = if axis == 0 { 0.0 } else { 0.25 };
        let t = targets(&edges(&contours, axis), max_stroke, mid, thin);
        for p in contours.iter_mut().flatten() {
            if axis == 0 {
                p.0 = map(&t, p.0);
            } else {
                p.1 = map(&t, p.1);
            }
        }
    }
    Outline { contours }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clockwise (y up) rectangle.
    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(f32, f32, bool)> {
        vec![
            (x0, y0, true),
            (x0, y1, true),
            (x1, y1, true),
            (x1, y0, true),
        ]
    }

    #[test]
    fn a_stem_lands_on_whole_pixels_with_a_whole_width() {
        // 0.3..2.4 x 0..8.6: width 2.1 -> 2, height 8.6 -> 9 (the baseline stays)
        let o = Outline {
            contours: vec![rect(0.3, 0.0, 2.4, 8.6)],
        };
        let f = fit(&o, 1.0, 3.0);
        let (x0, y0, x1, y1) = f.bounds().unwrap();
        assert_eq!((x0, x1 - x0), (0.0, 2.0));
        assert_eq!((y0, y1), (0.0, 9.0));
    }

    #[test]
    fn a_thin_bar_keeps_one_pixel() {
        // a bar 0.6 high around y 4.5: one pixel, not none
        let o = Outline {
            contours: vec![rect(0.0, 4.2, 6.0, 4.8)],
        };
        let f = fit(&o, 1.0, 3.0);
        let (_, y0, _, y1) = f.bounds().unwrap();
        assert_eq!(y1 - y0, 1.0);
        assert_eq!(y0.fract(), 0.0);
    }
}
