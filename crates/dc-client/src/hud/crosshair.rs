//! The drawn crosshair of the `crosshair_*` HUD keys (CLI-1633): vector shapes resolved to
//! antialiased fills on the target's pixels, no texture, so the CPU compositor and the GPU overlay draw the same pixels
//! at any resolution. Its size follows [`crate::spread::AimMarkView`], the numbers the stock aim mark is drawn from.
//!
//! Provenance: ORIGINAL (DeCombat's own mod feature; the numbers it reads are those of [`crate::spread`]).
// DC-DEFAULT: CLI-1633 (proposed, project decision) the five styles and their proportions, see DC_DEFAULTS.md

use super::compose::Prim;
use crate::spread::{mark, AimMarkView};

/// A stroke's width and a dot's radius in virtual pixels (scale `s` = target width / 1024, like the aim mark).
pub const STROKE: f32 = 1.2;
pub const DOT_RADIUS: f32 = 1.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrosshairStyle {
    Cross,
    Circle,
    Dot,
    CrossDot,
    CircleDot,
}

impl CrosshairStyle {
    pub const ALL: [CrosshairStyle; 5] = [
        CrosshairStyle::Cross,
        CrosshairStyle::Circle,
        CrosshairStyle::Dot,
        CrosshairStyle::CrossDot,
        CrosshairStyle::CircleDot,
    ];

    /// The value of the `crosshair_style` key.
    pub fn parse(s: &str) -> Option<CrosshairStyle> {
        Self::ALL.into_iter().find(|x| x.name() == s)
    }

    pub fn name(self) -> &'static str {
        match self {
            CrosshairStyle::Cross => "cross",
            CrosshairStyle::Circle => "circle",
            CrosshairStyle::Dot => "dot",
            CrosshairStyle::CrossDot => "cross_dot",
            CrosshairStyle::CircleDot => "circle_dot",
        }
    }
}

/// How the size follows the aim mark (the `crosshair_recoil` key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoilMode {
    /// The mark's distance and its growth after a shot.
    Dynamic,
    /// The row's rest size.
    Static,
    /// The rest size and the growth after a shot only: moving and the spread that builds up do not show.
    Fire,
}

impl RecoilMode {
    pub fn parse(s: &str) -> Option<RecoilMode> {
        match s {
            "dynamic" => Some(RecoilMode::Dynamic),
            "static" => Some(RecoilMode::Static),
            "fire" => Some(RecoilMode::Fire),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RecoilMode::Dynamic => "dynamic",
            RecoilMode::Static => "static",
            RecoilMode::Fire => "fire",
        }
    }
}

/// The player's shape, the values of the HUD keys (virtual pixels). The default is the look of the first version.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrossLook {
    pub length: f32,
    pub thickness: f32,
    /// Added to the distance of the arms' inner ends and to a ring's radius; below zero the arms cross.
    pub gap: f32,
    /// A centre dot besides what the style holds.
    pub dot: bool,
    /// The black outline's width, 0 for none.
    pub outline: f32,
    /// No top arm.
    pub t_style: bool,
    pub recoil: RecoilMode,
    /// Multiplies what the recoil adds to the rest size.
    pub recoil_scale: f32,
}

impl Default for CrossLook {
    fn default() -> Self {
        CrossLook {
            length: mark::LENGTH,
            thickness: STROKE,
            gap: 0.0,
            dot: false,
            outline: 0.0,
            t_style: false,
            recoil: RecoilMode::Dynamic,
            recoil_scale: 1.0,
        }
    }
}

/// One drawn crosshair: `inner` is the distance of the arms' inner ends from the centre and `length` the arms' length,
/// both in virtual pixels. `centre`: a virtual point (the settings page's preview); `None` is the aim mark's centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawnCrosshair {
    pub style: CrosshairStyle,
    pub inner: f32,
    pub length: f32,
    pub thickness: f32,
    pub dot: bool,
    pub outline: f32,
    pub t_style: bool,
    pub colour: [u8; 4],
    pub centre: Option<[f32; 2]>,
}

impl DrawnCrosshair {
    /// From the aim mark's numbers, the first version's shape. With `recoil` the size follows the mark; without it the
    /// size stays at the row's rest.
    pub fn of_mark(style: CrosshairStyle, v: &AimMarkView, recoil: bool) -> DrawnCrosshair {
        let shape = CrossLook {
            recoil: if recoil { RecoilMode::Dynamic } else { RecoilMode::Static },
            ..CrossLook::default()
        };
        DrawnCrosshair::of_shape(style, v, &shape)
    }

    /// From the aim mark's numbers and the player's shape. Dynamic takes the distance and the growth of 0.1 s after a
    /// shot from the mark (cshell+0x845f0 as read by [`crate::spread::aim_mark_rects`]), Fire the growth only, Static
    /// neither; the recoil scale multiplies what they add to the row's rest.
    pub fn of_shape(style: CrosshairStyle, v: &AimMarkView, shape: &CrossLook) -> DrawnCrosshair {
        let k = match v.since_shot_s.filter(|_| shape.recoil != RecoilMode::Static) {
            Some(t) => (1.0 - (t / mark::FIRING_S).clamp(0.0, 1.0)) * shape.recoil_scale,
            None => 0.0,
        };
        let spread = match shape.recoil {
            RecoilMode::Dynamic => (v.distance - v.rest) * shape.recoil_scale,
            _ => 0.0,
        };
        DrawnCrosshair {
            style,
            inner: v.rest + spread + mark::FIRING_OFFSET * k + shape.gap,
            length: shape.length + mark::FIRING_ADD_LENGTH * k,
            thickness: shape.thickness,
            dot: shape.dot,
            outline: shape.outline,
            t_style: shape.t_style,
            colour: v.colour,
            centre: None,
        }
    }

    /// The ring's radius in virtual pixels: on the middle of the resting arm, so it moves exactly as the arms' inner
    /// ends do and stays a ring at distance 0.
    pub fn ring_radius(&self) -> f32 {
        self.inner + mark::LENGTH / 2.0
    }

    /// Appends the fills on a `width` x `height` target (`sx`, `sy`: the virtual scale, used by `centre` only).
    pub fn resolve(&self, width: u32, height: u32, sx: f32, sy: f32, out: &mut Vec<Prim<'_>>) {
        let s = width as f32 / mark::VIRTUAL_WIDTH;
        let (cx, cy) = match self.centre {
            Some(p) => ((p[0] * sx).round() as i32, (p[1] * sy).round() as i32),
            // the pixel the stock arms cross
            None => ((width / 2) as i32 - 1, (height / 2) as i32 - 1),
        };
        let c = self.colour.map(|v| v as f32);
        let mut p = Painter {
            cx,
            cy,
            w: width as i32,
            h: height as i32,
            colour: [0.0, 0.0, 0.0, c[3]],
            out,
        };
        // never below one device pixel
        let stroke = (self.thickness * s).max(1.0);
        let edge = if self.outline > 0.0 { (self.outline * s).max(1.0) } else { 0.0 };
        let dot_r = (self.thickness * DOT_RADIUS / STROKE * s).max(1.0);
        let has_dot = self.dot
            || matches!(
                self.style,
                CrosshairStyle::Dot | CrosshairStyle::CrossDot | CrosshairStyle::CircleDot
            );
        let ring = matches!(self.style, CrosshairStyle::Circle | CrosshairStyle::CircleDot)
            .then(|| (self.ring_radius() * s).max(0.0));
        let mut arms = [[0.0f32; 4]; 4];
        if matches!(self.style, CrosshairStyle::Cross | CrosshairStyle::CrossDot) {
            let a = self.inner * s + 1.0;
            let b = a + if self.length > 0.0 { (self.length * s).max(1.0) } else { 0.0 };
            let t = stroke / 2.0;
            arms[0] = [-b, -a, -t, t];
            arms[1] = [a, b, -t, t];
            arms[2] = [-t, t, a, b];
            if !self.t_style {
                arms[3] = [-t, t, -b, -a];
            }
        }
        // the outline first, under the colour and only where the colour does not cover a pixel fully: a translucent
        // colour shows the world through it, not black
        if edge > 0.0 {
            let mut wide = arms;
            for r in wide.iter_mut().filter(|r| r[0] < r[1]) {
                *r = [r[0] - edge, r[1] + edge, r[2] - edge, r[3] + edge];
            }
            p.boxes(&wide, Some(&arms));
            if let Some(r) = ring {
                let (o, i) = (r + stroke / 2.0, r - stroke / 2.0);
                p.ring(o + edge, i - edge, |dx, dy| {
                    let d = (((dx * dx + dy * dy) as f32).sqrt(), 0.5);
                    d.0 <= o - d.1 && (i <= 0.0 || d.0 >= i + d.1)
                });
            }
            if has_dot {
                p.ring(dot_r + edge, 0.0, |dx, dy| {
                    ((dx * dx + dy * dy) as f32).sqrt() <= dot_r - 0.5 || fully_in(&wide, dx, dy)
                });
            }
        }
        p.colour = c;
        if let Some(r) = ring {
            p.ring(r + stroke / 2.0, r - stroke / 2.0, |_, _| false);
        }
        p.boxes(&arms, None);
        if has_dot {
            p.ring(dot_r, 0.0, |dx, dy| fully_in(&arms, dx, dy));
        }
    }
}

/// How much of pixel `i` the span `[lo, hi]` covers.
fn cover(lo: f32, hi: f32, i: i32) -> f32 {
    ((i as f32 + 0.5).min(hi) - (i as f32 - 0.5).max(lo)).max(0.0)
}

/// The largest coverage of pixels `(x, y)` by the rectangles `[x0, x1, y0, y1]`.
fn covered(rects: &[[f32; 4]], x: i32, y: i32) -> f32 {
    rects
        .iter()
        .filter(|r| r[0] < r[1])
        .map(|r| cover(r[0], r[1], x) * cover(r[2], r[3], y))
        .fold(0.0, f32::max)
}

fn fully_in(rects: &[[f32; 4]], x: i32, y: i32) -> bool {
    covered(rects, x, y) >= 0.999
}

/// Fills about a centre pixel: pixel `i` covers `[i - 0.5, i + 0.5]` relative to its own index.
struct Painter<'o, 'a> {
    cx: i32,
    cy: i32,
    w: i32,
    h: i32,
    colour: [f32; 4],
    out: &'o mut Vec<Prim<'a>>,
}

impl Painter<'_, '_> {
    /// Pixels `dx0..dx1` of the row `dy` at coverage `cov`, clipped to the target.
    fn span(&mut self, dx0: i32, dx1: i32, dy: i32, dy1: i32, cov: f32) {
        let x0 = (self.cx + dx0).max(0);
        let x1 = (self.cx + dx1).min(self.w);
        let y0 = (self.cy + dy).max(0);
        let y1 = (self.cy + dy1).min(self.h);
        if x0 >= x1 || y0 >= y1 || cov <= 0.002 {
            return;
        }
        let mut c = self.colour;
        c[3] *= cov.min(1.0);
        self.out.push(Prim::Fill {
            rect: [x0, y0, x1, y1],
            colour: c,
        });
    }

    /// The union of rectangles with fractional edges, each pixel blended once at the largest coverage it gets, so arms
    /// that meet or cross show no seam and no double blend on a translucent colour. `over`: rectangles to leave out
    /// where they cover a pixel fully. On the stack: at most (2 x 4 x 4)^2 cells, and only the lit ones become fills.
    fn boxes(&mut self, rects: &[[f32; 4]; 4], over: Option<&[[f32; 4]; 4]>) {
        let (mut xs, mut ys) = ([0i32; 32], [0i32; 32]);
        let mut n = 0;
        for r in rects.iter().chain(over.into_iter().flatten()).filter(|r| r[0] < r[1]) {
            for (k, e) in [r[0], r[1]].into_iter().enumerate() {
                let i = (e + 0.5).floor() as i32;
                xs[n + 2 * k] = i;
                xs[n + 2 * k + 1] = i + 1;
            }
            for (k, e) in [r[2], r[3]].into_iter().enumerate() {
                let i = (e + 0.5).floor() as i32;
                ys[n + 2 * k] = i;
                ys[n + 2 * k + 1] = i + 1;
            }
            n += 4;
        }
        let (xs, ys) = (&mut xs[..n], &mut ys[..n]);
        xs.sort_unstable();
        ys.sort_unstable();
        for yw in ys.windows(2).filter(|w| w[0] < w[1]) {
            for xw in xs.windows(2).filter(|w| w[0] < w[1]) {
                let (x, y) = (xw[0], yw[0]);
                if over.is_some_and(|o| fully_in(o, x, y)) {
                    continue;
                }
                self.span(x, xw[1], y, yw[1], covered(rects, x, y));
            }
        }
    }

    /// The disc of radius `outer` without the disc of radius `inner` (0: a filled disc). Coverage is one pixel wide
    /// across each edge; runs of full coverage become one fill, so a large ring stays at about a few fills per row.
    /// `skip`: pixels (relative to the centre) to leave out; it must be the same at `dx` and `-dx`.
    fn ring(&mut self, outer: f32, inner: f32, skip: impl Fn(i32, i32) -> bool) {
        const EDGE: f32 = 0.71;
        let top = (outer + EDGE).ceil() as i32;
        for dy in -top..=top {
            let y = dy as f32;
            let reach = (outer + EDGE).powi(2) - y * y;
            if reach < 0.0 {
                continue;
            }
            let hole = inner - EDGE;
            let first = if hole > 0.0 && hole * hole > y * y {
                (hole * hole - y * y).sqrt().floor() as i32
            } else {
                0
            };
            let last = reach.sqrt().ceil() as i32;
            let mut run: Option<i32> = None;
            for dx in first..=last + 1 {
                let d = ((dx * dx) as f32 + y * y).sqrt();
                let out_cov = (outer + 0.5 - d).clamp(0.0, 1.0);
                let in_cov = if inner > 0.0 {
                    (d - inner + 0.5).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let cov = if skip(dx, dy) { 0.0 } else { out_cov * in_cov };
                let full = cov >= 0.999 && dx <= last;
                match (full, run) {
                    (true, None) => run = Some(dx),
                    (false, Some(start)) => {
                        self.mirror(start, dx, dy, 1.0);
                        run = None;
                    }
                    _ => {}
                }
                if !full && dx <= last {
                    self.mirror(dx, dx + 1, dy, cov);
                }
            }
        }
    }

    /// Pixels `a..b` (`a >= 0`) right of the centre and their mirror image; pixel 0 only once.
    fn mirror(&mut self, a: i32, b: i32, dy: i32, cov: f32) {
        self.span(a, b, dy, dy + 1, cov);
        let a2 = a.max(1);
        if a2 < b {
            self.span(1 - b, 1 - a2, dy, dy + 1, cov);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::compose::{raster, Prim};

    const BLUE: [u8; 4] = [0x40, 0x80, 0xff, 0xff];

    fn view(distance: f32, since: Option<f32>) -> AimMarkView {
        AimMarkView {
            shape: crate::spread::mark::SHAPE_PLAIN,
            distance,
            since_shot_s: since,
            colour: BLUE,
            rest: 5.0,
        }
    }

    fn lit(c: &DrawnCrosshair, w: u32, h: u32) -> crate::overlay::text::RgbaImage {
        let mut prims: Vec<Prim<'_>> = Vec::new();
        c.resolve(w, h, w as f32 / 1024.0, h as f32 / 768.0, &mut prims);
        raster(&prims, w, h)
    }

    /// The radius (centre to lit pixels) along the row and the column through the centre, right and down.
    fn extents(img: &crate::overlay::text::RgbaImage) -> (i32, i32) {
        let (cx, cy) = (img.width / 2 - 1, img.height / 2 - 1);
        let right = (0..img.width - cx).filter(|d| img.pixel(cx + d, cy)[3] > 128).max();
        let down = (0..img.height - cy).filter(|d| img.pixel(cx, cy + d)[3] > 128).max();
        (right.unwrap_or(0) as i32, down.unwrap_or(0) as i32)
    }

    #[test]
    fn the_ring_is_round_on_a_wide_screen_at_two_sizes() {
        for (w, h) in [(1280u32, 720u32), (2560, 1440), (1280, 960)] {
            let c = DrawnCrosshair::of_mark(CrosshairStyle::Circle, &view(20.0, None), true);
            let img = lit(&c, w, h);
            let (dx, dy) = extents(&img);
            assert_eq!(dx, dy, "{w}x{h}");
            let want = c.ring_radius() * w as f32 / 1024.0;
            assert!((dx as f32 - want).abs() <= 1.5, "{w}x{h}: {dx} vs {want}");
        }
    }

    #[test]
    fn the_sizes_follow_the_width_only() {
        let c = DrawnCrosshair::of_mark(CrosshairStyle::Circle, &view(20.0, None), true);
        let (a, _) = extents(&lit(&c, 1280, 720));
        let (b, _) = extents(&lit(&c, 2560, 1440));
        assert!((b - 2 * a).abs() <= 2, "{a} {b}");
    }

    #[test]
    fn with_recoil_it_moves_as_the_stock_arms_do_and_returns() {
        let at = |v: AimMarkView| DrawnCrosshair::of_mark(CrosshairStyle::Cross, &v, true);
        let (w, h) = (1680, 1050);
        for (d, since) in [(5.0, None), (5.0, Some(0.0)), (5.0, Some(0.05)), (30.0, None)] {
            let v = view(d, since);
            let stock = crate::spread::aim_mark_rects(w, h, &v);
            let s = w as f32 / 1024.0;
            let c = at(v);
            // the right arm's inner end and its length equal the stock arm's (to the stock's half-pixel convention)
            assert!((c.inner * s - (stock[1][0] - (w / 2) as f32)).abs() <= 1.5, "{d} {since:?}");
            assert!((c.length * s - stock[1][2]).abs() < 1e-3, "{d} {since:?}");
        }
        assert_eq!(at(view(5.0, Some(0.2))).inner, 5.0, "the growth ended");
        assert!(at(view(5.0, Some(0.0))).inner > at(view(5.0, Some(0.05))).inner);
        assert!(at(view(40.0, None)).ring_radius() > at(view(5.0, None)).ring_radius());
    }

    #[test]
    fn without_recoil_the_size_stays_at_the_rest() {
        for v in [view(5.0, None), view(60.0, Some(0.0)), view(20.0, Some(0.05))] {
            let c = DrawnCrosshair::of_mark(CrosshairStyle::Circle, &v, false);
            assert_eq!((c.inner, c.length), (5.0, mark::LENGTH));
        }
    }

    #[test]
    fn every_pixel_of_a_large_ring_is_blended_once() {
        // a translucent colour shows any double coverage as a brighter pixel
        let mut c = DrawnCrosshair::of_mark(CrosshairStyle::CircleDot, &view(120.0, None), true);
        c.colour = [255, 255, 255, 128];
        let img = lit(&c, 2560, 1440);
        let max = img.rgba.chunks(4).map(|p| p[3]).max().unwrap();
        assert!(max <= 129, "{max}");
    }

    /// Writes the mod's own screenshot (run with `--ignored` after a look change): five styles, our pixels only.
    #[test]
    #[ignore]
    fn write_the_mod_screenshot() {
        let (w, h) = (640u32, 200u32);
        let mut prims: Vec<Prim<'_>> = vec![Prim::Fill {
            rect: [0, 0, w as i32, h as i32],
            colour: [24.0, 28.0, 32.0, 255.0],
        }];
        let colours = [[0x40, 0x80, 0xff, 0xff], [0xff, 0xff, 0x00, 0xff], [0x00, 0xff, 0x00, 0xff], [0xff, 0x30, 0x30, 0xff], [0xff, 0xff, 0xff, 0xff]];
        for (i, style) in CrosshairStyle::ALL.into_iter().enumerate() {
            let mut v = view(30.0, None);
            v.colour = colours[i];
            let mut c = DrawnCrosshair::of_mark(style, &v, true);
            c.centre = Some([64.0 + 128.0 * i as f32, 100.0]);
            c.resolve(w, h, 1.0, 1.0, &mut prims);
        }
        let img = raster(&prims, w, h);
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/mods/drawn_crosshair/screenshots/main.png");
        image::save_buffer(out, &img.rgba, w, h, image::ColorType::Rgba8).unwrap();
    }

    #[test]
    fn a_ring_takes_few_fills() {
        let c = DrawnCrosshair::of_mark(CrosshairStyle::Circle, &view(120.0, None), true);
        let mut prims: Vec<Prim<'_>> = Vec::new();
        c.resolve(2560, 1440, 2.5, 1.875, &mut prims);
        assert!(prims.len() < 8000, "{}", prims.len());
    }

    fn shaped(style: CrosshairStyle, v: &AimMarkView, shape: CrossLook) -> DrawnCrosshair {
        DrawnCrosshair::of_shape(style, v, &shape)
    }

    /// The first and last lit x along the row `dy` and lit y along the column `dx` (about the centre pixel).
    fn runs(img: &crate::overlay::text::RgbaImage, along_row: bool, across: i32) -> Vec<i32> {
        let (cx, cy) = ((img.width / 2 - 1) as i32, (img.height / 2 - 1) as i32);
        let n = if along_row { img.width as i32 } else { img.height as i32 };
        let c = if along_row { cx } else { cy };
        (0..n)
            .filter(|i| {
                let (x, y) = if along_row { (*i, cy + across) } else { (cx + across, *i) };
                img.pixel(x as u32, y as u32)[3] > 128
            })
            .map(|i| i - c)
            .collect()
    }

    /// Length, thickness and gap land on exact device pixels, about the centre, at 1024 wide (s = 1) and 2048 (s = 2).
    #[test]
    fn thickness_length_and_gap_are_exact_and_symmetric() {
        for (w, h, s) in [(1024u32, 768u32, 1i32), (2048, 1536, 2)] {
            let c = shaped(
                CrosshairStyle::Cross,
                &view(5.0, None),
                CrossLook { length: 9.0, thickness: 3.0, gap: 2.0, ..Default::default() },
            );
            assert_eq!(c.inner, 7.0);
            let img = lit(&c, w, h);
            // the row through the centre: both arms, from inner + 1 px to inner + length + 1 px
            let row = runs(&img, true, 0);
            let col = runs(&img, false, 0);
            assert_eq!(row, col, "{w}: the arms of both axes match");
            // both ends fall on half a pixel, so the fully lit run is one pixel shorter at each
            let (a, b) = (7 * s + 1, 16 * s + 1);
            let want: Vec<i32> = (-(b - 1)..=-(a + 1)).chain(a + 1..=b - 1).collect();
            assert_eq!(row, want, "{w}");
            // the width of an arm: 3 virtual pixels, centred
            let (cx, cy) = (img.width as i32 / 2 - 1, img.height as i32 / 2 - 1);
            let across: Vec<i32> = (-20..20).filter(|d| img.pixel((cx + a + 2) as u32, (cy + d) as u32)[3] > 128).collect();
            let half = (3 * s - 1) / 2;
            assert_eq!((across[0], across[across.len() - 1]), (-half, half), "{w}");
        }
    }

    #[test]
    fn a_thin_line_never_vanishes() {
        for w in [640u32, 800, 1024] {
            let c = shaped(
                CrosshairStyle::Cross,
                &view(5.0, None),
                CrossLook { thickness: 0.2, length: 0.5, ..Default::default() },
            );
            let img = lit(&c, w, w * 3 / 4);
            let cy = w * 3 / 8 - 1;
            let sum: u32 = (w / 2 - 1..w).map(|x| img.pixel(x, cy)[3] as u32).sum();
            assert!((254..=256).contains(&sum), "{w}: one device pixel of arm, {sum}");
        }
    }

    #[test]
    fn a_negative_gap_crosses_the_arms_with_no_double_blend() {
        for gap in [-5.0, -4.5, -4.0, -6.5, -12.0] {
            let mut c = shaped(
                CrosshairStyle::Cross,
                &view(5.0, None),
                CrossLook { gap, thickness: 3.0, length: 7.0, ..Default::default() },
            );
            c.colour = [255, 255, 255, 128];
            let img = lit(&c, 1280, 720);
            let max = img.rgba.chunks(4).map(|p| p[3]).max().unwrap();
            assert!(max <= 129, "gap {gap}: {max}");
            assert_eq!(runs(&img, true, 0), runs(&img, false, 0), "gap {gap}");
        }
        // meeting exactly: the centre is lit and no seam shows along the arms
        let c = shaped(
            CrosshairStyle::Cross,
            &view(5.0, None),
            CrossLook { gap: -7.0, thickness: 1.7, length: 10.0, ..Default::default() },
        );
        let img = lit(&c, 1024, 768);
        assert_eq!(runs(&img, true, 0).windows(2).filter(|w| w[1] != w[0] + 1).count(), 0);
    }

    #[test]
    fn the_t_style_has_no_top_arm_and_the_rest_stays() {
        let shape = CrossLook { t_style: true, ..Default::default() };
        let t = lit(&shaped(CrosshairStyle::Cross, &view(5.0, None), shape), 1280, 720);
        let full = lit(&shaped(CrosshairStyle::Cross, &view(5.0, None), Default::default()), 1280, 720);
        let col = runs(&t, false, 0);
        assert!(col.iter().all(|y| *y > 0) && !col.is_empty(), "{col:?}");
        assert!(runs(&full, false, 0).iter().any(|y| *y < 0));
        assert_eq!(runs(&t, true, 0), runs(&full, true, 0));
    }

    #[test]
    fn the_outline_is_black_around_the_colour_and_keeps_a_translucent_colour_clean() {
        let shape = CrossLook { outline: 2.0, thickness: 3.0, gap: 5.0, length: 9.0, ..Default::default() };
        let mut v = view(5.0, None);
        v.colour = [255, 255, 255, 255];
        let img = lit(&shaped(CrosshairStyle::Cross, &v, shape), 1024, 768);
        let (cx, cy) = (511u32, 383u32);
        // the right arm starts at inner + 1 = 11; 2 black pixels above it past half the thickness
        let at = |dx: i32, dy: i32| img.pixel((cx as i32 + dx) as u32, (cy as i32 + dy) as u32);
        assert_eq!(at(14, 0), [255, 255, 255, 255]);
        assert_eq!(at(14, -3), [0, 0, 0, 255], "above the arm");
        assert_eq!(at(14, 3), [0, 0, 0, 255], "below the arm");
        assert_eq!(at(14, -5)[3], 0, "the outline ends");
        // translucent: the arm's middle holds no black under it
        v.colour = [255, 255, 255, 128];
        let img = lit(&shaped(CrosshairStyle::Cross, &v, shape), 1024, 768);
        let at = |dx: i32, dy: i32| img.pixel((cx as i32 + dx) as u32, (cy as i32 + dy) as u32);
        assert_eq!(at(14, 0)[3], 128);
        assert_eq!(at(14, -3)[3], 128);
        for style in [CrosshairStyle::Circle, CrosshairStyle::CircleDot, CrosshairStyle::CrossDot, CrosshairStyle::Dot] {
            let img = lit(&shaped(style, &v, shape), 1280, 720);
            assert!(img.rgba.chunks(4).any(|p| p[3] > 0 && p[0] < 40), "{style:?} has an outline");
            let max = img.rgba.chunks(4).map(|p| p[3]).max().unwrap();
            assert!(max <= 192, "{style:?}: {max}"); // at most an edge pixel blended over its outline
        }
    }

    #[test]
    fn the_circle_takes_its_radius_from_the_gap_and_its_line_from_the_thickness() {
        let radius = |gap: f32, thickness: f32| {
            let c = shaped(CrosshairStyle::Circle, &view(5.0, None), CrossLook { gap, thickness, ..Default::default() });
            (c.ring_radius(), lit(&c, 1024, 768))
        };
        let (r0, base) = radius(0.0, 3.0);
        assert_eq!(r0, 8.5);
        let (r1, wide) = radius(4.0, 3.0);
        assert_eq!(r1, 12.5);
        assert!(extents(&wide).0 >= extents(&base).0 + 3);
        let count = |img: &crate::overlay::text::RgbaImage| img.rgba.chunks(4).filter(|p| p[3] > 128).count();
        assert!(count(&radius(0.0, 5.0).1) > count(&base));
        // a length change does not move a ring; a negative gap leaves a disc, not nothing
        let c = shaped(CrosshairStyle::Circle, &view(5.0, None), CrossLook { length: 30.0, ..Default::default() });
        assert_eq!(c.ring_radius(), DrawnCrosshair::of_mark(CrosshairStyle::Circle, &view(5.0, None), true).ring_radius());
        assert!(count(&radius(-20.0, 3.0).1) > 0);
    }

    #[test]
    fn the_centre_dot_follows_the_thickness_on_any_style() {
        let count = |t: f32| {
            let shape = CrossLook { dot: true, thickness: t, ..Default::default() };
            let img = lit(&shaped(CrosshairStyle::Cross, &view(30.0, None), shape), 1024, 768);
            assert_eq!(img.pixel(511, 383)[3], 255, "the dot lights the centre");
            let (mut n, mut y) = (0, 383 - 10);
            while y <= 383 + 10 {
                n += (511 - 10..=511 + 10).filter(|x| img.pixel(*x, y)[3] > 128).count();
                y += 1;
            }
            n
        };
        assert!(count(6.0) > count(1.2));
        let off = lit(&shaped(CrosshairStyle::Cross, &view(30.0, None), Default::default()), 1024, 768);
        assert_eq!(off.pixel(511, 383)[3], 0);
    }

    #[test]
    fn the_recoil_modes_and_the_scale() {
        let s = |recoil, recoil_scale| CrossLook { recoil, recoil_scale, ..Default::default() };
        let at = |shape, d, since| shaped(CrosshairStyle::Cross, &view(d, since), shape);
        use RecoilMode::*;
        // dynamic scale 1 is the first version's numbers
        let (d, f) = (s(Dynamic, 1.0), s(Fire, 1.0));
        assert_eq!(at(d, 30.0, Some(0.0)), DrawnCrosshair::of_mark(CrosshairStyle::Cross, &view(30.0, Some(0.0)), true));
        assert_eq!(at(d, 30.0, None).inner, 30.0);
        // the scale multiplies what comes on top of the rest (5)
        assert_eq!(at(s(Dynamic, 2.0), 30.0, None).inner, 55.0);
        assert_eq!(at(s(Dynamic, 0.5), 30.0, None).inner, 17.5);
        assert_eq!(at(s(Dynamic, 2.0), 5.0, Some(0.0)).inner, 5.0 + 8.0);
        assert_eq!(at(s(Dynamic, 2.0), 5.0, Some(0.0)).length, 7.0 + 12.0);
        // scale 0 and static stand still
        assert_eq!(at(s(Dynamic, 0.0), 30.0, Some(0.0)).inner, 5.0);
        assert_eq!(at(s(Static, 2.0), 30.0, Some(0.0)).inner, 5.0);
        assert_eq!(at(s(Static, 2.0), 30.0, Some(0.0)).length, 7.0);
        // on fire only: the shot's kick, not the spread
        assert_eq!(at(f, 30.0, None).inner, 5.0);
        assert_eq!(at(f, 30.0, Some(0.0)).inner, 9.0);
        assert_eq!(at(f, 30.0, Some(0.0)).length, 13.0);
        assert_eq!(at(f, 30.0, Some(0.2)).inner, 5.0);
    }

    /// Writes the sheet of variants (run with `--ignored` and `DC_CROSSHAIR_SHOTS`), scaled 3x with no smoothing.
    #[test]
    #[ignore]
    fn write_the_variant_sheet() {
        let Some(dir) = std::env::var_os("DC_CROSSHAIR_SHOTS") else { return };
        let (cols, cell) = (6u32, 120u32);
        let def = CrossLook::default();
        let green = [0x20, 0xff, 0x40, 0xff];
        let list: Vec<(CrosshairStyle, CrossLook, [u8; 4], f32)> = vec![
            (CrosshairStyle::Circle, def, BLUE, 5.0),
            (CrosshairStyle::Cross, def, green, 5.0),
            (CrosshairStyle::Dot, def, green, 5.0),
            (CrosshairStyle::CrossDot, def, green, 5.0),
            (CrosshairStyle::CircleDot, def, BLUE, 5.0),
            (CrosshairStyle::Cross, def, green, 25.0),
            (CrosshairStyle::Cross, CrossLook { length: 3.0, ..def }, green, 5.0),
            (CrosshairStyle::Cross, CrossLook { length: 20.0, ..def }, green, 5.0),
            (CrosshairStyle::Cross, CrossLook { thickness: 0.5, ..def }, green, 5.0),
            (CrosshairStyle::Cross, CrossLook { thickness: 3.0, ..def }, green, 5.0),
            (CrosshairStyle::Cross, CrossLook { gap: -5.0, thickness: 2.0, ..def }, green, 5.0),
            (CrosshairStyle::Cross, CrossLook { gap: -8.0, thickness: 2.0, ..def }, green, 5.0),
            (CrosshairStyle::Cross, CrossLook { gap: 8.0, ..def }, green, 5.0),
            (CrosshairStyle::Cross, CrossLook { t_style: true, ..def }, green, 5.0),
            (CrosshairStyle::Cross, CrossLook { dot: true, thickness: 2.0, ..def }, green, 5.0),
            (CrosshairStyle::Cross, CrossLook { outline: 1.0, thickness: 2.0, ..def }, [0xff, 0xff, 0xff, 0xff], 5.0),
            (CrosshairStyle::Cross, CrossLook { outline: 2.0, thickness: 3.0, gap: -2.0, ..def }, [0xff, 0xe0, 0x20, 0xff], 5.0),
            (CrosshairStyle::Cross, CrossLook { outline: 1.0, thickness: 3.0, ..def }, [0xff, 0xff, 0xff, 0x70], 5.0),
            (CrosshairStyle::Circle, CrossLook { thickness: 3.0, ..def }, BLUE, 5.0),
            (CrosshairStyle::Circle, CrossLook { gap: 10.0, ..def }, [0xff, 0x30, 0x30, 0xff], 5.0),
            (CrosshairStyle::CircleDot, CrossLook { outline: 1.0, thickness: 2.0, gap: 5.0, ..def }, [0xff, 0xff, 0xff, 0xff], 5.0),
            (CrosshairStyle::Circle, CrossLook { outline: 1.0, thickness: 3.0, gap: 5.0, ..def }, [0xff, 0x30, 0xff, 0x80], 5.0),
            (CrosshairStyle::Dot, CrossLook { thickness: 4.0, outline: 1.0, ..def }, green, 5.0),
            (CrosshairStyle::Cross, CrossLook { t_style: true, outline: 1.0, gap: -2.0, thickness: 2.0, dot: true, ..def }, [0xff, 0x80, 0x00, 0xff], 5.0),
        ];
        let rows = (list.len() as u32).div_ceil(cols);
        let (w, h) = (cols * cell, rows * cell);
        let mut prims: Vec<Prim<'_>> = vec![Prim::Fill {
            rect: [0, 0, w as i32, h as i32],
            colour: [90.0, 96.0, 104.0, 255.0],
        }];
        for (i, (style, shape, colour, d)) in list.iter().enumerate() {
            let mut v = view(*d, None);
            v.colour = *colour;
            let mut c = DrawnCrosshair::of_shape(*style, &v, shape);
            c.centre = Some([512.0, 384.0]);
            // one device pixel per virtual pixel, as at 1024 wide
            let mut one = Vec::new();
            c.resolve(1024, 768, 1.0, 1.0, &mut one);
            let i = i as u32;
            let dx = ((i % cols) * cell + cell / 2) as i32 - 512;
            let dy = ((i / cols) * cell + cell / 2) as i32 - 384;
            for f in one {
                if let Prim::Fill { rect, colour } = f {
                    prims.push(Prim::Fill {
                        rect: [rect[0] + dx, rect[1] + dy, rect[2] + dx, rect[3] + dy],
                        colour,
                    });
                }
            }
        }
        let img = raster(&prims, w, h);
        let mut big = vec![0u8; (w * h * 4 * 4) as usize];
        for y in 0..h * 2 {
            for x in 0..w * 2 {
                let p = img.pixel(x / 2, y / 2);
                let o = ((y * w * 2 + x) * 4) as usize;
                big[o..o + 4].copy_from_slice(&p);
            }
        }
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        image::save_buffer(dir.join("variants.png"), &big, w * 2, h * 2, image::ColorType::Rgba8).unwrap();
    }
}
