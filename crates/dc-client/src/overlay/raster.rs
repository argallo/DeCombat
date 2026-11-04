//! Outline rasteriser: quadratic TrueType contours flattened to line segments, then filled with the non-zero
//! winding rule on supersampled scanlines with exact horizontal coverage.
//!
//! PUBLIC: TrueType quadratic B-spline contours (consecutive off-curve points imply an on-curve midpoint);
//! the non-zero winding fill rule; scanline polygon filling by sorted edge crossings; TrueType monochrome scan
//! conversion (a pixel is on when its centre lies inside the outline; a span between two centres lights its
//! nearest pixel: dropout control).

use super::ttf::Outline;

/// Vertical samples per pixel row.
const SUBSAMPLES: usize = 5;

/// An 8-bit coverage bitmap positioned relative to the pen: column 0 is at `left` pixels right of the pen,
/// row 0 at `top` pixels below the baseline (negative = above).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub alpha: Vec<u8>,
}

type Pt = (f32, f32);

fn quad_points(out: &mut Vec<Pt>, p0: Pt, c: Pt, p1: Pt) {
    let len = ((c.0 - p0.0).hypot(c.1 - p0.1) + (p1.0 - c.0).hypot(p1.1 - c.1)).max(0.0);
    let n = ((len / 2.0).ceil() as usize).clamp(1, 16);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        out.push((
            u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
            u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
        ));
    }
}

/// One contour (already in pixel space) as a closed polyline.
pub fn flatten(contour: &[(f32, f32, bool)]) -> Vec<Pt> {
    let n = contour.len();
    if n == 0 {
        return Vec::new();
    }
    let mid = |a: (f32, f32, bool), b: (f32, f32, bool)| ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
    // start at an on-curve point; with none, at the implied midpoint of the last and first points
    let (start, order): (Pt, Vec<(f32, f32, bool)>) = match contour.iter().position(|p| p.2) {
        Some(s) => (
            (contour[s].0, contour[s].1),
            (1..=n).map(|i| contour[(s + i) % n]).collect(),
        ),
        None => (mid(contour[n - 1], contour[0]), contour.to_vec()),
    };
    let mut out = vec![start];
    let mut cur = start;
    let mut ctrl: Option<Pt> = None;
    let push = |out: &mut Vec<Pt>, cur: &mut Pt, ctrl: &mut Option<Pt>, q: Pt, on: bool| {
        if on {
            match ctrl.take() {
                Some(c) => quad_points(out, *cur, c, q),
                None => out.push(q),
            }
            *cur = q;
        } else if let Some(c) = *ctrl {
            let m = ((c.0 + q.0) * 0.5, (c.1 + q.1) * 0.5);
            quad_points(out, *cur, c, m);
            *cur = m;
            *ctrl = Some(q);
        } else {
            *ctrl = Some(q);
        }
    };
    let last_is_start = contour.iter().any(|p| p.2);
    for (i, p) in order.iter().enumerate() {
        if last_is_start && i + 1 == order.len() {
            break; // the start point itself closes the contour below
        }
        push(&mut out, &mut cur, &mut ctrl, (p.0, p.1), p.2);
    }
    push(&mut out, &mut cur, &mut ctrl, start, true);
    out
}

fn add_span(row: &mut [f32], xa: f32, xb: f32, w: f32) {
    let width = row.len() as f32;
    let xa = xa.clamp(0.0, width);
    let xb = xb.clamp(0.0, width);
    if xb <= xa {
        return;
    }
    let ia = xa.floor() as usize;
    let ib = xb.floor() as usize;
    if ia == ib {
        if ia < row.len() {
            row[ia] += (xb - xa) * w;
        }
        return;
    }
    row[ia] += (ia as f32 + 1.0 - xa) * w;
    for v in row.iter_mut().take(ib).skip(ia + 1) {
        *v += w;
    }
    if ib < row.len() {
        row[ib] += (xb - ib as f32) * w;
    }
}

/// Fills closed polylines (pixel space, y down) into a `width x height` coverage map with the non-zero rule.
pub fn fill(polys: &[Vec<Pt>], width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut edges: Vec<(Pt, Pt)> = Vec::new();
    for p in polys {
        for s in p.windows(2) {
            if s[0].1 != s[1].1 {
                edges.push((s[0], s[1]));
            }
        }
    }
    let mut out = vec![0u8; w * h];
    let mut row = vec![0f32; w];
    let mut xs: Vec<(f32, i32)> = Vec::new();
    let weight = 1.0 / SUBSAMPLES as f32;
    for y in 0..h {
        row.iter_mut().for_each(|v| *v = 0.0);
        for k in 0..SUBSAMPLES {
            let sy = y as f32 + (k as f32 + 0.5) * weight;
            xs.clear();
            for &(a, b) in &edges {
                let (lo, hi) = if a.1 < b.1 { (a.1, b.1) } else { (b.1, a.1) };
                if sy < lo || sy >= hi {
                    continue;
                }
                let x = a.0 + (sy - a.1) * (b.0 - a.0) / (b.1 - a.1);
                xs.push((x, if b.1 > a.1 { 1 } else { -1 }));
            }
            xs.sort_by(|p, q| p.0.total_cmp(&q.0));
            let mut wind = 0;
            for i in 0..xs.len() {
                let before = wind;
                wind += xs[i].1;
                if before == 0 && wind != 0 {
                    // span starts; find where the winding returns to zero
                    let mut j = i + 1;
                    let mut wj = wind;
                    while j < xs.len() {
                        wj += xs[j].1;
                        if wj == 0 {
                            break;
                        }
                        j += 1;
                    }
                    let end = xs.get(j).map_or(w as f32, |e| e.0);
                    add_span(&mut row, xs[i].0, end, weight);
                }
            }
        }
        for (o, v) in out[y * w..(y + 1) * w].iter_mut().zip(&row) {
            *o = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    out
}

/// Crossings of the horizontal line `sy` with the edges as non-zero spans [xa, xb).
fn spans(edges: &[(Pt, Pt)], sy: f32, xs: &mut Vec<(f32, i32)>, out: &mut Vec<(f32, f32)>) {
    xs.clear();
    out.clear();
    for &(a, b) in edges {
        let (lo, hi) = if a.1 < b.1 { (a.1, b.1) } else { (b.1, a.1) };
        if sy < lo || sy >= hi {
            continue;
        }
        let x = a.0 + (sy - a.1) * (b.0 - a.0) / (b.1 - a.1);
        xs.push((x, if b.1 > a.1 { 1 } else { -1 }));
    }
    xs.sort_by(|p, q| p.0.total_cmp(&q.0));
    let mut wind = 0;
    let mut start = 0.0;
    for &(x, d) in xs.iter() {
        let before = wind;
        wind += d;
        if before == 0 && wind != 0 {
            start = x;
        } else if before != 0 && wind == 0 {
            out.push((start, x));
        }
    }
}

/// Fills closed polylines like [`fill`] in monochrome: 255 where the pixel centre is inside (non-zero rule), 0
/// elsewhere; a span or a vertical run that covers no centre lights its nearest pixel (dropout control).
pub fn fill_mono(polys: &[Vec<Pt>], width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h];
    let mut xs = Vec::new();
    let mut sp = Vec::new();
    let horizontal: Vec<(Pt, Pt)> = polys
        .iter()
        .flat_map(|p| p.windows(2).map(|s| (s[0], s[1])))
        .filter(|(a, b)| a.1 != b.1)
        .collect();
    for y in 0..h {
        spans(&horizontal, y as f32 + 0.5, &mut xs, &mut sp);
        for &(xa, xb) in &sp {
            // centres x + 0.5 in [xa, xb)
            let first = (xa - 0.5).ceil().max(0.0) as usize;
            let last = (xb - 0.5).ceil() as isize - 1;
            if (first as isize) <= last {
                for x in first..=(last as usize).min(w.saturating_sub(1)) {
                    out[y * w + x] = 255;
                }
            } else {
                let x = (((xa + xb) * 0.5).floor().max(0.0) as usize).min(w.saturating_sub(1));
                out[y * w + x] = 255;
            }
        }
    }
    // vertical runs (thin horizontal strokes between two row centres)
    let vertical: Vec<(Pt, Pt)> = polys
        .iter()
        .flat_map(|p| p.windows(2).map(|s| ((s[0].1, s[0].0), (s[1].1, s[1].0))))
        .filter(|(a, b)| a.1 != b.1)
        .collect();
    for x in 0..w {
        spans(&vertical, x as f32 + 0.5, &mut xs, &mut sp);
        for &(ya, yb) in &sp {
            let first = (ya - 0.5).ceil();
            let last = (yb - 0.5).ceil() - 1.0;
            if first > last {
                let y = (((ya + yb) * 0.5).floor().max(0.0) as usize).min(h.saturating_sub(1));
                out[y * w + x] = 255;
            }
        }
    }
    out
}

/// Rasterises an outline (font units, y up) at `scale` pixels per unit.
pub fn rasterize(outline: &Outline, scale: f32) -> Bitmap {
    raster_with(outline, scale, false)
}

/// [`rasterize`] in monochrome: the outline fitted to the pixel grid ([`super::gridfit::fit`], strokes up to a
/// quarter em), then scan converted by pixel centres ([`fill_mono`]).
pub fn rasterize_mono(outline: &Outline, scale: f32, em: f32) -> Bitmap {
    let fitted = super::gridfit::fit(outline, scale, em * 0.25);
    raster_with(&fitted, 1.0, true)
}

fn raster_with(outline: &Outline, scale: f32, mono: bool) -> Bitmap {
    let Some((x0, y0, x1, y1)) = outline.bounds() else {
        return Bitmap::default();
    };
    // pixel space: x right, y down, baseline at y = 0
    let left = (x0 * scale).floor() as i32;
    let right = (x1 * scale).ceil() as i32;
    let top = (-y1 * scale).floor() as i32;
    let bottom = (-y0 * scale).ceil() as i32;
    let width = (right - left).max(0) as u32;
    let height = (bottom - top).max(0) as u32;
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Bitmap::default();
    }
    let polys: Vec<Vec<Pt>> = outline
        .contours
        .iter()
        .map(|c| {
            let px: Vec<(f32, f32, bool)> = c
                .iter()
                .map(|&(x, y, on)| (x * scale - left as f32, -y * scale - top as f32, on))
                .collect();
            flatten(&px)
        })
        .collect();
    Bitmap {
        width,
        height,
        left,
        top,
        alpha: if mono {
            fill_mono(&polys, width, height)
        } else {
            fill(&polys, width, height)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_is_filled_exactly() {
        // 2..6 x 1..5 square, clockwise in y-down
        let sq = vec![vec![
            (2.0, 1.0),
            (6.0, 1.0),
            (6.0, 5.0),
            (2.0, 5.0),
            (2.0, 1.0),
        ]];
        let a = fill(&sq, 8, 6);
        for y in 0..6 {
            for x in 0..8 {
                let inside = (2..6).contains(&x) && (1..5).contains(&y);
                assert_eq!(a[y * 8 + x], if inside { 255 } else { 0 }, "({x}, {y})");
            }
        }
        // half-pixel edge gives half coverage
        let half = vec![vec![
            (0.5, 0.0),
            (2.0, 0.0),
            (2.0, 1.0),
            (0.5, 1.0),
            (0.5, 0.0),
        ]];
        let b = fill(&half, 2, 1);
        assert_eq!(b, vec![128, 255]);
    }

    #[test]
    fn hole_with_opposite_winding_is_empty_and_same_winding_overlap_is_full() {
        let outer = vec![(0.0, 0.0), (6.0, 0.0), (6.0, 6.0), (0.0, 6.0), (0.0, 0.0)];
        let hole = vec![(2.0, 2.0), (2.0, 4.0), (4.0, 4.0), (4.0, 2.0), (2.0, 2.0)];
        let a = fill(&[outer.clone(), hole], 6, 6);
        assert_eq!(a[3 * 6 + 3], 0);
        assert_eq!(a[6 + 1], 255);
        let same = vec![(2.0, 2.0), (4.0, 2.0), (4.0, 4.0), (2.0, 4.0), (2.0, 2.0)];
        let b = fill(&[outer, same], 6, 6);
        assert_eq!(b[3 * 6 + 3], 255, "non-zero rule keeps overlaps filled");
    }

    #[test]
    fn flatten_handles_implied_points_and_all_off_contours() {
        // on, off, off, on: one implied midpoint between the two off points
        let c = [
            (0.0, 0.0, true),
            (0.0, 10.0, false),
            (10.0, 10.0, false),
            (10.0, 0.0, true),
        ];
        let p = flatten(&c);
        assert_eq!(p.first(), Some(&(0.0, 0.0)));
        assert_eq!(p.last(), Some(&(0.0, 0.0)), "closed");
        assert!(
            p.contains(&(5.0, 10.0)),
            "implied on-curve midpoint is on the path: {p:?}"
        );
        // all off-curve: a closed curve inside the control square
        let o = [
            (0.0, 0.0, false),
            (10.0, 0.0, false),
            (10.0, 10.0, false),
            (0.0, 10.0, false),
        ];
        let q = flatten(&o);
        assert_eq!(q.first(), q.last());
        assert!(q
            .iter()
            .all(|p| p.0 >= 0.0 && p.0 <= 10.0 && p.1 >= 0.0 && p.1 <= 10.0));
    }

    #[test]
    fn glyph_of_the_made_up_font() {
        let f = super::super::ttf::Font::parse(crate::testkit::synth_font(), 0).unwrap();
        let o = f.outline(f.glyph_index('A')).unwrap();
        let b = rasterize(&o, 0.02); // 1000 units -> 20 px
        assert_eq!((b.left, b.top), (2, -14));
        assert_eq!((b.width, b.height), (12, 14));
        // the counter (hole) is empty, the frame is full
        let at =
            |x: i32, y: i32| b.alpha[((y - b.top) as u32 * b.width + (x - b.left) as u32) as usize];
        assert_eq!(at(8, -5), 0, "hole");
        assert_eq!(at(3, -2), 255, "frame");
        assert!(rasterize(&Outline::default(), 1.0).alpha.is_empty());
    }
}
