//! Glyph atlas (ASCII 32..126 rasterised once) and the composition of text lines into an RGBA panel.

use super::raster::{rasterize, rasterize_mono, Bitmap};
use super::ttf::Font;

/// Straight-alpha RGBA8 image, row-major, top row first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RgbaImage {
    pub fn new(width: u32, height: u32, fill: [u8; 4]) -> RgbaImage {
        RgbaImage {
            width,
            height,
            rgba: fill.repeat((width * height) as usize),
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[o],
            self.rgba[o + 1],
            self.rgba[o + 2],
            self.rgba[o + 3],
        ]
    }

    /// Blends `colour` with coverage `cov` (0..255) over the pixel (straight alpha "over").
    fn blend(&mut self, x: i32, y: i32, colour: [u8; 3], cov: u8) {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height || cov == 0 {
            return;
        }
        let o = ((y as u32 * self.width + x as u32) * 4) as usize;
        let sa = cov as f32 / 255.0;
        let da = self.rgba[o + 3] as f32 / 255.0;
        let oa = sa + da * (1.0 - sa);
        for (dst, &src) in self.rgba[o..o + 3].iter_mut().zip(&colour) {
            let (s, d) = (src as f32, *dst as f32);
            let v = if oa > 0.0 {
                (s * sa + d * da * (1.0 - sa)) / oa
            } else {
                0.0
            };
            *dst = (v + 0.5).clamp(0.0, 255.0) as u8;
        }
        self.rgba[o + 3] = (oa * 255.0 + 0.5) as u8;
    }
}

/// ASCII glyphs of one font at one pixel size.
#[derive(Debug, Clone)]
pub struct GlyphAtlas {
    pub px: f32,
    /// Baseline below the line top (pixels).
    pub ascent: f32,
    pub line_height: f32,
    /// Ascent plus descent (GDI's tmHeight): the height a text is centred by.
    pub cell_height: f32,
    glyphs: Vec<(Bitmap, f32)>,
    /// Glyphs the font had no outline for (drawn as the missing glyph).
    pub missing: u32,
    /// Identity of this rasterisation (clones share it): the GPU overlay keys its glyph sheet by it.
    id: u64,
}

/// Source of [`GlyphAtlas`] identities (never reused within a process).
static NEXT_ATLAS_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Debug panel colours (a DeCombat choice): translucent dark background, light text.
pub const PANEL_BACKGROUND: [u8; 4] = [8, 10, 14, 170];
pub const PANEL_TEXT: [u8; 3] = [235, 240, 220];

const FIRST: u32 = 32;
const LAST: u32 = 126;

impl GlyphAtlas {
    /// Rasterises ASCII 32..126 at `px` pixels per em (anti-aliased).
    pub fn new(font: &Font, px: f32) -> GlyphAtlas {
        Self::build(font, px, false, None, None)
    }

    /// Rasterises ASCII 32..126 at `px` pixels per em the way GDI draws a TrueType face: monochrome where the
    /// face's `gasp` table asks for no grey-scale at `gasp_ppem` (Tahoma: 9..16 ppem), the line's ascent /
    /// descent from the face's `VDMX` record for the drawn ppem when it has one (GDI's tmAscent / tmDescent), the
    /// advances from the face's `hdmx` widths at `advance_ppem` scaled to `px` (GDI's hinted advances: the drawn
    /// em's own, CLI-1267); the glyphs are grid fitted by [`super::gridfit`] (no bytecode interpreter).
    // PUBLIC: OpenType `gasp` (GASP_DOGRAY), `VDMX` and `hdmx` tables; GDI TEXTMETRIC tmAscent / tmDescent
    // OBSERVED: shop-guns/cpp/shop_bb00_weapons_c0_p1.png (font 0x17 description and gauge
    // labels: binary pixels, no grey ramp; Tahoma Bold 12 ppem baseline 12 below the text top) (CLI-780)
    pub fn gdi(font: &Font, px: f32, gasp_ppem: u16, advance_ppem: u16) -> GlyphAtlas {
        let mono = !font.gasp_grayscale(gasp_ppem);
        let ppem = px.round().clamp(1.0, u16::MAX as f32) as u16;
        Self::build(font, px, mono, font.vdmx(ppem), Some(advance_ppem))
    }

    /// `hinted`: advances from the face's `hdmx` widths at that ppem, scaled to `px` (so a text measures the
    /// same at every target size).
    fn build(
        font: &Font,
        px: f32,
        mono: bool,
        device: Option<(i16, i16)>,
        hinted: Option<u16>,
    ) -> GlyphAtlas {
        let scale = px / font.units_per_em as f32;
        let mut missing = 0;
        let glyphs: Vec<(Bitmap, f32)> = (FIRST..=LAST)
            .map(|c| {
                let ch = char::from_u32(c).unwrap_or(' ');
                let g = font.glyph_index(ch);
                if g == 0 && ch != ' ' {
                    missing += 1;
                }
                let bm = font
                    .outline(g)
                    .map(|o| {
                        if mono {
                            rasterize_mono(&o, scale, px)
                        } else {
                            rasterize(&o, scale)
                        }
                    })
                    .unwrap_or_default();
                let adv = hinted
                    .filter(|&p| p > 0)
                    .and_then(|p| Some(font.hdmx_advance(p, g)? as f32 * px / p as f32))
                    .unwrap_or(font.advance(g) as f32 * scale);
                (bm, adv)
            })
            .collect();
        // the line box covers both the font's vertical metrics and the glyphs actually drawn (some faces
        // draw outside their ascender / descender)
        let (ink_up, ink_down) = glyphs
            .iter()
            .filter(|(b, _)| b.height > 0)
            .fold((0i32, 0i32), |(u, d), (b, _)| {
                (u.max(-b.top), d.max(b.top + b.height as i32))
            });
        let (ascent, descent) = match device {
            Some((up, down)) => (up.max(1) as f32, down.max(0) as f32),
            None => (
                (font.ascender as f32 * scale).ceil().max(ink_up as f32),
                (-(font.descender as f32) * scale)
                    .ceil()
                    .max(ink_down as f32),
            ),
        };
        let gap = (font.line_gap.max(0) as f32 * scale).round().max(1.0);
        GlyphAtlas {
            px,
            ascent,
            line_height: ascent + descent + gap,
            cell_height: ascent + descent,
            glyphs,
            missing,
            id: NEXT_ATLAS_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    fn glyph_index(ch: char) -> usize {
        let c = ch as u32;
        let i = if (FIRST..=LAST).contains(&c) {
            c - FIRST
        } else {
            b'?' as u32 - FIRST
        };
        i as usize
    }

    fn glyph(&self, ch: char) -> &(Bitmap, f32) {
        &self.glyphs[Self::glyph_index(ch)]
    }

    /// Identity of this rasterisation (equal for clones, unique otherwise).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The glyph bitmaps in index order (index = code - 32; `?` stands in for codes outside 32..126).
    pub fn bitmaps(&self) -> impl ExactSizeIterator<Item = &Bitmap> {
        self.glyphs.iter().map(|(b, _)| b)
    }

    /// Where [`GlyphAtlas::draw`] / [`GlyphAtlas::draw_rgba`] put each glyph of `text`: `f(index, bitmap, x, y)`
    /// with the bitmap's top-left pixel at (x, y); glyphs without pixels are left out.
    pub fn layout(
        &self,
        x: f32,
        baseline: f32,
        text: &str,
        mut f: impl FnMut(usize, &Bitmap, i32, i32),
    ) {
        let mut pen = x;
        for ch in text.chars() {
            let i = Self::glyph_index(ch);
            let (bm, adv) = &self.glyphs[i];
            let ox = pen.round() as i32 + bm.left;
            let oy = baseline.round() as i32 + bm.top;
            if bm.width > 0 && bm.height > 0 {
                f(i, bm, ox, oy);
            }
            pen += adv;
        }
    }

    /// Size of the [`GlyphAtlas::panel`] of `lines` and its margin.
    pub fn panel_size(&self, lines: &[String]) -> (u32, u32, f32) {
        let pad = (self.px * 0.5).ceil();
        let w = lines.iter().map(|l| self.measure(l)).fold(0.0, f32::max) + 2.0 * pad;
        let h = self.line_height * lines.len() as f32 + 2.0 * pad;
        ((w.ceil() as u32).max(1), (h.ceil() as u32).max(1), pad)
    }

    /// Width of `text` in pixels.
    pub fn measure(&self, text: &str) -> f32 {
        text.chars().map(|c| self.glyph(c).1).sum()
    }

    /// Draws `text` with its baseline at `baseline`, starting at pen position `x`.
    pub fn draw(&self, img: &mut RgbaImage, x: f32, baseline: f32, text: &str, colour: [u8; 3]) {
        self.draw_rgba(
            img,
            x,
            baseline,
            text,
            [colour[0], colour[1], colour[2], 255],
        );
    }

    /// Draws `text` like [`GlyphAtlas::draw`] with a straight-alpha colour (coverage times the colour's alpha).
    pub fn draw_rgba(
        &self,
        img: &mut RgbaImage,
        x: f32,
        baseline: f32,
        text: &str,
        colour: [u8; 4],
    ) {
        let rgb = [colour[0], colour[1], colour[2]];
        self.layout(x, baseline, text, |_, bm, ox, oy| {
            for row in 0..bm.height {
                for col in 0..bm.width {
                    let a =
                        bm.alpha[(row * bm.width + col) as usize] as u32 * colour[3] as u32 / 255;
                    img.blend(ox + col as i32, oy + row as i32, rgb, a as u8);
                }
            }
        });
    }

    /// The overlay panel: `lines` in light text on a translucent dark background with a small margin.
    pub fn panel(&self, lines: &[String]) -> RgbaImage {
        let (w, h, pad) = self.panel_size(lines);
        let mut img = RgbaImage::new(w, h, PANEL_BACKGROUND);
        for (i, l) in lines.iter().enumerate() {
            self.draw(&mut img, pad, self.panel_baseline(pad, i), l, PANEL_TEXT);
        }
        img
    }

    /// Baseline of line `i` of a panel with margin `pad`.
    pub fn panel_baseline(&self, pad: f32, i: usize) -> f32 {
        pad + self.ascent + self.line_height * i as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_has_text_pixels_and_background() {
        let f = Font::parse(crate::testkit::synth_font(), 0).unwrap();
        let atlas = GlyphAtlas::new(&f, 20.0);
        assert_eq!(atlas.missing, 92, "the made-up font has two glyphs");
        assert_eq!(atlas.ascent, 16.0);
        assert_eq!(atlas.line_height, 21.0);
        assert_eq!(atlas.measure("AB"), 16.0 + 9.0);
        let img = atlas.panel(&["AB".to_string(), "A".to_string()]);
        assert_eq!((img.width, img.height), (45, 62));
        assert_eq!(img.pixel(0, 0), [8, 10, 14, 170]);
        // the frame of the first 'A' (pen at 10, baseline at 26): solid text colour, opaque
        let p = img.pixel(10 + 3, 26 - 2);
        assert_eq!(p[3], 255);
        assert!(p[0] > 200, "{p:?}");
        // inside its hole: background
        assert_eq!(img.pixel(10 + 8, 26 - 5), [8, 10, 14, 170]);
    }
}
