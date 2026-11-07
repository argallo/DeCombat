//! CPU compositor of the HUD display list: ROM textures (DTX through dc-render's archive index and decoder) and
//! ROM fonts (system font records through dc-ui-data, rasterised by our TrueType reader) drawn into one
//! straight-alpha RGBA image the size of the target, which the overlay pass then blends over the world.
//!
//! The 2009 2D path draws in alpha mode with linear filtering (RENDER_2009 §12.1, `bilinear_2d` default 1); the
//! sampler here is bilinear over the source texels, clamped to the source rectangle.
// SPEC: RENDER_2009.md §12.1 (2D blend mode 0 SRCALPHA / INVSRCALPHA, linear filter), §12.4 (ROM TrueType fonts)
// SPEC: FMT_CRES_UI.md §5 step 5 (per-axis scale from 1024 x 768), §7.1 (system font records)

use super::items::DrawItem;
use crate::overlay::text::{GlyphAtlas, RgbaImage};
use crate::overlay::ttf::Font;
use dc_ui_data::{FontResolver, SystemFonts, VirtualScaler};
use std::collections::HashMap;
use std::sync::Arc;

/// A decoded texture (RGBA8, top row first).
#[derive(Debug, Clone)]
pub struct Tex {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Where textures come from.
pub trait TextureSource {
    fn load(&mut self, path: &str) -> Option<Tex>;
}

/// Textures of the user's ROM: any archive entry by its full path (`<archive stem>\..\name.dtx`).
pub struct RomTextureSource {
    index: dc_render::TextureIndex,
}

impl RomTextureSource {
    pub fn open(rom: &std::path::Path) -> Result<Self, String> {
        Ok(RomTextureSource {
            index: dc_render::TextureIndex::open(rom).map_err(|e| e.to_string())?,
        })
    }
}

impl RomTextureSource {
    /// CLI-1612: the installed mod layer first (a `@mod:` path or an override), then the ROM; a `@mod:` path never
    /// reaches the ROM.
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(b) = crate::modassets::read_layered(path) {
            return Some(b);
        }
        if crate::modassets::is_mod_path(path) {
            return None;
        }
        self.index.read(path.as_bytes()).ok()?
    }
}

/// The first level of a DTX file as RGBA.
pub fn decode_tex(bytes: &[u8]) -> Option<Tex> {
    let dtx = dc_tex::DtxFile::parse(bytes).ok()?;
    let img = dc_render::decode_dtx(&dtx)?;
    Some(Tex {
        width: img.width,
        height: img.height,
        rgba: img.levels.into_iter().next()?,
    })
}

impl TextureSource for RomTextureSource {
    fn load(&mut self, path: &str) -> Option<Tex> {
        // VAR-3: a variant's shop picture, painted here from the ROM's picture and the pack's pattern
        if let Some((pattern, picture)) = crate::camo::parse_art(path) {
            return crate::camo::paint_picture(&self.load(picture)?, &self.load(pattern)?);
        }
        let mut bytes = self.read(path)?;
        // a sprite file (the scope pictures) draws its first frame
        // SPEC: FMT_SPR_LTC_FX.md §2.2 (the frame list); DC-DEFAULT: CLI-615
        if dc_render::is_sprite_name(path.as_bytes()) {
            let s = dc_fx_data::Sprite::parse(&bytes).ok()?;
            bytes = self.read(s.frames.first()?)?;
        }
        decode_tex(&bytes)
    }
}

/// No textures (the fallback layout draws text and boxes only).
pub struct NoTextures;

impl TextureSource for NoTextures {
    /// No ROM: only a mod's own files (CLI-1612).
    fn load(&mut self, path: &str) -> Option<Tex> {
        decode_tex(&crate::modassets::read_layered(path)?)
    }
}

/// The fonts of the HUD: one atlas per system font index, at the record's height scaled to the target.
pub struct HudFonts {
    /// The client's default font (the overlay font's file) for records without a usable file of their own.
    default_bytes: Option<(Vec<u8>, usize)>,
    default_px: f32,
    records: Option<(SystemFonts, FontResolver)>,
    atlases: HashMap<(i32, i32), Option<Arc<GlyphAtlas>>>,
    /// The same fonts at 1024 x 768 (what a wrap measures with, CLI-540).
    virtual_atlases: HashMap<(i32, i32), Option<Arc<GlyphAtlas>>>,
    /// Vertical scale virtual -> target.
    scale: f32,
}

/// CLI-1267 the whole em (pixels per em) GDI draws `font` at for a requested cell height of `cell` pixels (a
/// positive `lfHeight`): the largest em whose Windows ascent and descent, each scaled and rounded, add up to no more
/// than the cell (hhea ascender / descender without an OS/2 table).
// PUBLIC: GDI LOGFONT (positive height = cell height), TEXTMETRIC tmHeight = tmAscent + tmDescent
// OBSERVED: label-place-r14/cpp/ (reference client, Tahoma Bold from the hdmx widths of the drawn
// text: cell 14 -> em 12 and cell 13 -> em 11 at 1024 x 768; cell 26 -> em 21 and cell 24 -> em 20 at 2560 x 1440)
pub fn cell_em(font: &Font, cell: f32) -> f32 {
    let (up, down) = if font.win_ascent as u32 + font.win_descent as u32 > 0 {
        (font.win_ascent as f32, font.win_descent as f32)
    } else {
        (font.ascender as f32, -(font.descender as f32))
    };
    cell_em_of(up, down, font.units_per_em as f32, cell)
}

/// The em of a cell of `cell` pixels by the face's Windows ascent + descent (hhea without an OS/2 table), rounded.
// PUBLIC: GDI LOGFONT (positive height = cell height); OpenType OS/2 usWinAscent / usWinDescent, hhea fallback
pub fn rounded_em(font: &Font, cell: f32) -> f32 {
    let win = font.win_ascent as f32 + font.win_descent as f32;
    let span = if win > 0.0 {
        win
    } else {
        (font.ascender as f32 - font.descender as f32).max(1.0)
    };
    (cell * font.units_per_em as f32 / span).round().max(1.0)
}

/// [`cell_em`] from the face's ascent and descent (both positive) in `units` per em.
pub fn cell_em_of(up: f32, down: f32, units: f32, cell: f32) -> f32 {
    let units = units.max(1.0);
    let fits = |em: f32| (up * em / units).round() + (down * em / units).round() <= cell;
    // the em grows with the cell, so the first em past the cell ends the search
    (2..=1024)
        .map(|e| e as f32)
        .take_while(|&e| fits(e))
        .last()
        .unwrap_or(1.0)
}

/// CLI-1267 the screen cell height of a text of `cell` virtual pixels at vertical scale `scale`: the product
/// truncated toward zero (the font is requested at that whole height).
// EVIDENCE: cshell+0x2eb340 (the text control's scale: its base char size times the vertical scale through the
// truncating float-to-int cshell+0x3a5f00, kept as the char size), cshell+0x2ec5b0 (a char size that differs from
// the font's request height requests the font anew at that height)
pub fn scaled_cell(cell: f32, scale: f32) -> f32 {
    // a hair over the product, so 768-based scales that land on a whole number never truncate one below it
    (cell * scale + 1e-3).trunc()
}

/// CLI-1267 the screen pixel a text's top-left corner lands on: the virtual place times the scale, truncated.
// EVIDENCE: cshell+0x2ec240 (the text control's screen place: base x times the horizontal scale and base y times
// the vertical scale, each through the truncating cshell+0x3a5f00)
pub fn scaled_place(v: f32, scale: f32) -> f32 {
    (v * scale + 1e-3).trunc()
}

impl HudFonts {
    pub fn new(
        default_bytes: Option<(Vec<u8>, usize)>,
        default_px: f32,
        records: Option<(SystemFonts, FontResolver)>,
        scale: f32,
    ) -> Self {
        HudFonts {
            default_bytes,
            default_px,
            records,
            atlases: HashMap::new(),
            virtual_atlases: HashMap::new(),
            scale,
        }
    }

    fn make(&self, index: i32, size: Option<i32>, scale: f32) -> Option<GlyphAtlas> {
        let spec = self
            .records
            .as_ref()
            .and_then(|(f, _)| (index >= 0).then(|| f.get(index as usize, true)).flatten());
        let cell = size
            .map(|s| s as f32)
            .or_else(|| spec.map(|s| s.size_px() as f32))
            .filter(|h| *h > 0.0)
            .unwrap_or(self.default_px);
        let own = spec.zip(self.records.as_ref()).and_then(|(s, (_, r))| {
            let found = r.resolve(s)?;
            let bytes = r.read(&found).ok()?;
            let face = dc_ui_data::fonts::font_file_info(&bytes)
                .and_then(|info| {
                    info.face_offsets.iter().position(|&o| {
                        dc_ui_data::fonts::face_names(&bytes, o)
                            .iter()
                            .any(|n| n.eq_ignore_ascii_case(&s.face))
                    })
                })
                .unwrap_or(0);
            Font::parse(bytes, face).ok()
        });
        let font = own.or_else(|| {
            let (b, face) = self.default_bytes.clone()?;
            Font::parse(b, face).ok()
        })?;
        // GDI rasterises a TrueType face at whole pixels per em (the cell's em rounded): Tahoma cell 14 -> 12 at
        // 1024 x 768
        // PUBLIC: GDI TrueType rasteriser (integer ppem); OBSERVED shop-guns/cpp/ (CLI-724)
        let scale = scale.max(f32::EPSILON);
        let base_em = rounded_em(&font, cell.max(4.0));
        // CLI-1267: on another target the font is requested anew at the screen cell height (the size times the
        // vertical scale, truncated), drawn at the largest whole em that fits that cell and with that em's own
        // advances: Tahoma 14 at 2560 x 1440 is cell 26, em 21 (was 12 x 1.875 = 22.5 with the 12 em's advances
        // stretched); 1024 x 768 keeps the CLI-724 em (the two agree for Tahoma 9..19, not checked for other faces)
        let em = if (scale - 1.0).abs() < 1e-6 {
            base_em
        } else {
            cell_em(&font, scaled_cell(cell, scale).max(4.0))
        };
        // DC-DEFAULT: CLI-54 (proposed) HUD text is scaled with the vertical virtual scale like the positions
        // the face's gasp choice (monochrome or grey) follows the 1024 x 768 em, so the text looks the same at
        // every target size
        // DC-DEFAULT: CLI-780 gasp decided at the base em
        Some(GlyphAtlas::gdi(&font, em, base_em as u16, em as u16))
    }

    /// A new vertical scale (target resized): atlases are rebuilt on use.
    pub fn set_scale(&mut self, scale: f32) {
        if scale != self.scale {
            self.scale = scale;
            self.atlases.clear();
        }
    }

    /// The atlas of `index` at the record's height (built once).
    pub fn atlas(&mut self, index: i32) -> Option<Arc<GlyphAtlas>> {
        self.atlas_sized(index, None)
    }

    /// The atlas of `index` at `size` (a cell height in virtual pixels) or the record's height.
    pub fn atlas_sized(&mut self, index: i32, size: Option<i32>) -> Option<Arc<GlyphAtlas>> {
        let key = (index, size.unwrap_or(-1));
        if !self.atlases.contains_key(&key) {
            let a = self.make(index, size, self.scale).map(Arc::new);
            self.atlases.insert(key, a);
        }
        self.atlases.get(&key).cloned().flatten()
    }

    /// The atlas of `index` at `size` as drawn at 1024 x 768, whatever the target: its widths are virtual pixels.
    // DC-DEFAULT: CLI-540 a wrap breaks a text where it breaks at 1024 x 768 at every target size
    pub fn atlas_virtual(&mut self, index: i32, size: Option<i32>) -> Option<Arc<GlyphAtlas>> {
        let key = (index, size.unwrap_or(-1));
        if !self.virtual_atlases.contains_key(&key) {
            let a = self.make(index, size, 1.0).map(Arc::new);
            self.virtual_atlases.insert(key, a);
        }
        self.virtual_atlases.get(&key).cloned().flatten()
    }
}

/// Textures and fonts with their caches.
pub struct HudAssets {
    pub textures: Box<dyn TextureSource>,
    cache: HashMap<String, Option<Arc<Tex>>>,
    pub fonts: HudFonts,
    /// Texture names that resolved to nothing (logged once).
    pub missing: Vec<String>,
}

impl HudAssets {
    pub fn new(textures: Box<dyn TextureSource>, fonts: HudFonts) -> Self {
        HudAssets {
            textures,
            cache: HashMap::new(),
            fonts,
            missing: Vec::new(),
        }
    }

    pub fn texture(&mut self, path: &str) -> Option<Arc<Tex>> {
        if path.is_empty() {
            return None;
        }
        if !self.cache.contains_key(path) {
            let t = match part_name(path) {
                // CLI-1847: the fall vignette's texture is made here, not read from the ROM
                None if path == super::fallborder::TEXTURE => {
                    Some(Arc::new(super::fallborder::texture()))
                }
                // CRATE-7: the case reel's effect textures are made here
                None if path.starts_with("@dc:reel_") => crate::menu::reelfx::texture_arc(path),
                // CLI-1893: our own flag pictures (the ROM catalogue puts them in the ROM flags' shape at load)
                None if path.starts_with("@dc:flag_") => {
                    crate::menu::ownflag::texture(path, crate::menu::ownflag::OWN_SHAPE)
                        .map(Arc::new)
                }
                Some((base, r)) => self.texture(base).map(|b| Arc::new(crop(&b, r))),
                None => self.textures.load(path).map(Arc::new),
            };
            if t.is_none() {
                self.missing.push(path.to_string());
            }
            self.cache.insert(path.to_string(), t);
        }
        self.cache.get(path).cloned().flatten()
    }
}

/// The name of the texels `r` (x, y, w, h) of texture `base`: [`HudAssets::texture`] serves it as a texture of its
/// own (the menu draws a texture's nine parts this way, as the draw item has no texel offset).
pub fn part_of(base: &str, r: [u32; 4]) -> String {
    format!("{base}#{},{},{},{}", r[0], r[1], r[2], r[3])
}

fn part_name(path: &str) -> Option<(&str, [u32; 4])> {
    let (base, r) = path.rsplit_once('#')?;
    let mut v = r.split(',').map(|n| n.parse::<u32>().ok());
    let r = [v.next()??, v.next()??, v.next()??, v.next()??];
    Some((base, r))
}

/// Texels `r` of `t` (clamped to the texture), at least 1 x 1.
fn crop(t: &Tex, r: [u32; 4]) -> Tex {
    let x0 = r[0].min(t.width.saturating_sub(1));
    let y0 = r[1].min(t.height.saturating_sub(1));
    let w = r[2].min(t.width - x0).max(1);
    let h = r[3].min(t.height - y0).max(1);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in y0..y0 + h {
        let o = ((y * t.width + x0) * 4) as usize;
        match t.rgba.get(o..o + (w * 4) as usize) {
            Some(row) => rgba.extend_from_slice(row),
            None => rgba.extend(std::iter::repeat_n(0, (w * 4) as usize)),
        }
    }
    Tex {
        width: w,
        height: h,
        rgba,
    }
}

// ---- client-radar: textures composed by the HUD itself (the radar's map layer and view disc)
impl HudAssets {
    /// Stores a texture the HUD composed itself under `name` (not an archive path); [`HudAssets::texture`] returns
    /// it from then on.
    pub fn put(&mut self, name: &str, tex: Tex) {
        self.cache.insert(name.to_string(), Some(Arc::new(tex)));
    }

    /// Drops a composed texture (shop-model: a replaced character frame).
    pub fn forget(&mut self, name: &str) {
        self.cache.remove(name);
    }

    /// A texture already in the cache (a composed one, or a loaded one), without loading.
    pub fn generated(&self, name: &str) -> Option<Arc<Tex>> {
        self.cache.get(name).cloned().flatten()
    }
}

fn sample(t: &Tex, u: f32, v: f32, max_u: f32, max_v: f32) -> [f32; 4] {
    // bilinear over texel centres, clamped to [0, max) of the source rectangle
    let u = (u - 0.5).clamp(0.0, (max_u - 1.0).max(0.0));
    let v = (v - 0.5).clamp(0.0, (max_v - 1.0).max(0.0));
    let (x0, y0) = (u.floor() as u32, v.floor() as u32);
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let lim_x = (max_u as u32).min(t.width).max(1) - 1;
    let lim_y = (max_v as u32).min(t.height).max(1) - 1;
    let (x1, y1) = ((x0 + 1).min(lim_x), (y0 + 1).min(lim_y));
    let (x0, y0) = (x0.min(lim_x), y0.min(lim_y));
    let px = |x: u32, y: u32| {
        let o = ((y * t.width + x) * 4) as usize;
        [
            t.rgba[o] as f32,
            t.rgba[o + 1] as f32,
            t.rgba[o + 2] as f32,
            t.rgba[o + 3] as f32,
        ]
    };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    let mut out = [0.0; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bot = c[i] + (d[i] - c[i]) * fx;
        out[i] = top + (bot - top) * fy;
    }
    out
}

/// Blends straight-alpha `src` (0..255 floats) over the pixel.
fn over(img: &mut RgbaImage, x: i32, y: i32, src: [f32; 4]) {
    if x < 0 || y < 0 || x as u32 >= img.width || y as u32 >= img.height || src[3] <= 0.0 {
        return;
    }
    let o = ((y as u32 * img.width + x as u32) * 4) as usize;
    let sa = (src[3] / 255.0).min(1.0);
    let da = img.rgba[o + 3] as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    for (dst, &s) in img.rgba[o..o + 3].iter_mut().zip(&src[..3]) {
        let d = *dst as f32;
        let v = if oa > 0.0 {
            (s * sa + d * da * (1.0 - sa)) / oa
        } else {
            0.0
        };
        *dst = (v + 0.5).clamp(0.0, 255.0) as u8;
    }
    img.rgba[o + 3] = (oa * 255.0 + 0.5) as u8;
}

/// One display-list item resolved to target pixels: the single geometry both the CPU compositor ([`compose`] /
/// [`raster`]) and the GPU overlay ([`crate::overlay::hud_quads`]) draw from, so the two cannot drift apart.
/// Rectangles are half-open `[x0, y0, x1, y1)` and not clipped to the target.
#[derive(Clone)]
pub enum Prim<'a> {
    /// `colour` (straight alpha, 0..255) blended once over every covered pixel.
    Fill { rect: [i32; 4], colour: [f32; 4] },
    /// A text run with its pen at `x` on `baseline` (the glyph placement of [`GlyphAtlas::layout`]).
    Text {
        atlas: Arc<GlyphAtlas>,
        x: f32,
        baseline: f32,
        text: &'a str,
        colour: [u8; 4],
    },
    /// A texture (white without one) on the unturned rectangle `rect` (`[x0, y0, x1, y1]`, target pixels),
    /// turned by `turn` (`[pivot x, pivot y, m00, m01, m10, m11]`: a target pixel `q` samples the unturned
    /// rectangle at `pivot + M (q - pivot)`), over `src` source texels and multiplied by `tint` (0..1); `bbox` is
    /// the pixel range its turned corners touch.
    Image {
        name: &'a str,
        tex: Option<Arc<Tex>>,
        bbox: [i32; 4],
        rect: [f32; 4],
        turn: [f32; 6],
        src: [f32; 2],
        tint: [f32; 4],
    },
}

/// Resolves the display list on a `width` x `height` target: virtual-to-target scaling and rounding, the aim
/// mark's covered ranges, the cross's pixel walk, image corners and turns, text pen positions. Items whose font or
/// texture is missing, and empty images, resolve to nothing.
pub fn resolve<'a>(
    items: &'a [DrawItem],
    assets: &mut HudAssets,
    width: u32,
    height: u32,
) -> Vec<Prim<'a>> {
    let sc = VirtualScaler::new(width, height);
    let (sx, sy) = (sc.sx, sc.sy);
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        match it {
            DrawItem::Fill { rect, colour } => {
                let x0 = (rect[0] * sx).round() as i32;
                let y0 = (rect[1] * sy).round() as i32;
                let x1 = ((rect[0] + rect[2]) * sx).round() as i32;
                let y1 = ((rect[1] + rect[3]) * sy).round() as i32;
                out.push(Prim::Fill {
                    rect: [x0, y0, x1, y1],
                    colour: colour.map(|v| v as f32),
                });
            }
            DrawItem::AimMark(v) => {
                // a pixel is covered when its index lies in [x, x + w) (the 2009 2D quads carry their own -0.5)
                // EVIDENCE: cshell+0x845f0
                let c = v.colour.map(|x| x as f32);
                // use-items-sweep: the shape of a reticle item (CLI-1878)
                for rect in crate::spread::aim_mark_fills(width, height, v) {
                    out.push(Prim::Fill { rect, colour: c });
                }
            }
            DrawItem::Crosshair(c) => c.resolve(width, height, sx, sy, &mut out),
            DrawItem::Text {
                style,
                pos,
                text,
                right,
            } => {
                let Some(atlas) = assets.fonts.atlas_sized(style.font, style.size) else {
                    continue;
                };
                let w = atlas.measure(text);
                let mut x = pos[0] * sx;
                if *right {
                    x -= w;
                }
                let baseline = pos[1] * sy + atlas.ascent;
                out.push(Prim::Text {
                    atlas,
                    x,
                    baseline,
                    text,
                    colour: style.colour,
                });
            }
            DrawItem::Label {
                style,
                pos,
                text,
                anchor,
            } => {
                let Some(atlas) = assets.fonts.atlas_sized(style.font, style.size) else {
                    continue;
                };
                let w = atlas.measure(text);
                // CLI-1267: a left / top aligned text starts on the truncated screen place (cshell+0x2ec240)
                let at = |v: f32, s: f32, a: f32| if a == 0.0 { scaled_place(v, s) } else { v * s };
                let x = at(pos[0], sx, anchor[0]) - anchor[0] * w;
                // centred by the cell (ascent + descent) as GDI's DT_VCENTER does, not the line pitch
                // PUBLIC: GDI DrawText DT_VCENTER, TEXTMETRIC tmHeight (CLI-780)
                let top = at(pos[1], sy, anchor[1]) - anchor[1] * atlas.cell_height;
                let baseline = top + atlas.ascent;
                out.push(Prim::Text {
                    atlas,
                    x,
                    baseline,
                    text,
                    colour: style.colour,
                });
            }
            DrawItem::AimCross {
                bar,
                half_gap,
                colour,
            } => {
                // OBSERVED: capture 20261025_123446_xhair-frozen/driver/rest_1.png (1680 x 1050:
                // centre pixel (839, 524) = (w / 2 - 1, h / 2 - 1); inner ends 7 pixels out on both axes; horizontal
                // bars 11 pixels, vertical bars 13 pixels: gap and horizontal bars follow the vertical factor, the
                // vertical bars the horizontal factor)
                let (cx, cy) = (width as i32 / 2 - 1, height as i32 / 2 - 1);
                let gap = (half_gap * sy).round() as i32;
                let hbar = (bar * sy).round() as i32;
                let vbar = (bar * sx).round() as i32;
                let c = colour.map(|v| v as f32);
                // one pixel per blend, in walk order (a pixel reached twice is blended twice)
                let mut dot = |x: i32, y: i32| {
                    out.push(Prim::Fill {
                        rect: [x, y, x + 1, y + 1],
                        colour: c,
                    })
                };
                for d in gap..gap + hbar {
                    dot(cx - d, cy);
                    dot(cx + d, cy);
                }
                for d in gap..gap + vbar {
                    dot(cx, cy - d);
                    dot(cx, cy + d);
                }
            }
            DrawItem::Image {
                tex,
                src,
                dst,
                tint,
                turn,
            } => {
                let t = assets.texture(tex);
                if t.is_none() && !tex.is_empty() {
                    continue;
                }
                if dst[2] <= 0.0 || dst[3] <= 0.0 {
                    continue;
                }
                // destination corners in target pixels, turned about the pivot when asked: the turn happens in
                // virtual units and the turned shape is then scaled per axis (2009's view disc: the corner at
                // angle t is (centre + R cos t, centre + R sin t) x (sx, sy)), so on a non-4:3 target a turned
                // disc stretches exactly like the unturned ring image around it instead of tilting into an oval
                // EVIDENCE: cshell+0xb96a0..0xb96ce (radar view disc corners); scratchpad shot 3024 x 1964
                // (the disc square turned in target pixels sat as an oval askew of the ring)
                // DC-DEFAULT: CLI-145 (proposed, amended) turned images turn in virtual units, then scale per axis
                let (fwd, inv, pivot) = match turn {
                    Some((p, a)) => {
                        let (c, s) = (a.cos(), a.sin());
                        // S R S^-1 (forward) and S R^-1 S^-1 (inverse) with S = diag(sx, sy), k = sx / sy
                        let k = sx / sy;
                        ([c, -k * s, s / k, c], [c, k * s, -s / k, c], [p[0] * sx, p[1] * sy])
                    }
                    None => ([1.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0], [0.0, 0.0]),
                };
                let rot = |x: f32, y: f32| -> [f32; 2] {
                    let (dx, dy) = (x - pivot[0], y - pivot[1]);
                    [
                        pivot[0] + dx * fwd[0] + dy * fwd[1],
                        pivot[1] + dx * fwd[2] + dy * fwd[3],
                    ]
                };
                // the top-left corner lands on a whole target pixel (truncated), the size keeps the scale
                // OBSERVED: capture 20261025_123446_xhair-frozen/driver/rest_1.png (1680 x 1050:
                // with unsnapped corners every element sat 0.3 .. 1 pixel right of / below the 2009 client's)
                // DC-DEFAULT: CLI-73 (proposed) image corners truncated to whole target pixels
                let (x0, y0) = ((dst[0] * sx).floor(), (dst[1] * sy).floor());
                let (mut x1, mut y1) = (x0 + dst[2] * sx, y0 + dst[3] * sy);
                // CLI-911: the parts of a nine-part frame (cropped textures, `#` in the name) end on a whole pixel, so a
                // part's truncated start never leaves a one-pixel gap to the part before it at a fractional scale
                // DC-DEFAULT: CLI-911 (proposed) a part's far edges round up
                if tex.contains('#') {
                    x1 = x1.ceil();
                    y1 = y1.ceil();
                }
                let corners = [rot(x0, y0), rot(x1, y0), rot(x0, y1), rot(x1, y1)];
                let minx = corners.iter().map(|c| c[0]).fold(f32::MAX, f32::min);
                let maxx = corners.iter().map(|c| c[0]).fold(f32::MIN, f32::max);
                let miny = corners.iter().map(|c| c[1]).fold(f32::MAX, f32::min);
                let maxy = corners.iter().map(|c| c[1]).fold(f32::MIN, f32::max);
                out.push(Prim::Image {
                    name: tex,
                    tex: t,
                    bbox: [
                        minx.floor() as i32,
                        miny.floor() as i32,
                        maxx.ceil() as i32,
                        maxy.ceil() as i32,
                    ],
                    rect: [x0, y0, x1, y1],
                    turn: [pivot[0], pivot[1], inv[0], inv[1], inv[2], inv[3]],
                    src: *src,
                    tint: tint.map(|v| v as f32 / 255.0),
                });
            }
        }
    }
    out
}

/// Draws resolved primitives into a transparent `width` x `height` image (the CPU reference raster).
pub fn raster(prims: &[Prim<'_>], width: u32, height: u32) -> RgbaImage {
    let mut img = RgbaImage::new(width, height, [0, 0, 0, 0]);
    let (w, h) = (width as i32, height as i32);
    for p in prims {
        match p {
            Prim::Fill { rect, colour } => {
                for y in rect[1].max(0)..rect[3].min(h) {
                    for x in rect[0].max(0)..rect[2].min(w) {
                        over(&mut img, x, y, *colour);
                    }
                }
            }
            Prim::Text {
                atlas,
                x,
                baseline,
                text,
                colour,
            } => atlas.draw_rgba(&mut img, *x, *baseline, text, *colour),
            Prim::Image {
                tex,
                bbox,
                rect,
                turn,
                src,
                tint,
                ..
            } => {
                let [x0, y0, x1, y1] = *rect;
                let [pvx, pvy, m00, m01, m10, m11] = *turn;
                for py in bbox[1].max(0)..bbox[3].min(h) {
                    for px in bbox[0].max(0)..bbox[2].min(w) {
                        // inverse turn of the pixel centre
                        let (cx, cy) = (px as f32 + 0.5 - pvx, py as f32 + 0.5 - pvy);
                        let (ux, uy) = (pvx + cx * m00 + cy * m01, pvy + cx * m10 + cy * m11);
                        if ux < x0 || ux >= x1 || uy < y0 || uy >= y1 {
                            continue;
                        }
                        let s = match tex {
                            Some(t) => {
                                let u = (ux - x0) / (x1 - x0) * src[0];
                                let v = (uy - y0) / (y1 - y0) * src[1];
                                sample(
                                    t,
                                    u,
                                    v,
                                    src[0].min(t.width as f32),
                                    src[1].min(t.height as f32),
                                )
                            }
                            None => [255.0; 4],
                        };
                        over(
                            &mut img,
                            px,
                            py,
                            [
                                s[0] * tint[0],
                                s[1] * tint[1],
                                s[2] * tint[2],
                                s[3] * tint[3],
                            ],
                        );
                    }
                }
            }
        }
    }
    img
}

/// Draws the display list into a transparent `width` x `height` image ([`resolve`], then [`raster`]).
pub fn compose(items: &[DrawItem], assets: &mut HudAssets, width: u32, height: u32) -> RgbaImage {
    raster(&resolve(items, assets, width, height), width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::layout::TextStyle;

    struct Solid;
    impl TextureSource for Solid {
        fn load(&mut self, path: &str) -> Option<Tex> {
            (path == "red").then(|| Tex {
                width: 4,
                height: 4,
                rgba: [200, 10, 10, 255].repeat(16),
            })
        }
    }

    #[test]
    fn scaled_text_cell_em_and_place_follow_the_2009_truncation() {
        // CLI-1267 with Tahoma's Windows metrics (ascent 2049, descent 423, 2048 units per em)
        let em = |cell: f32| cell_em_of(2049.0, 423.0, 2048.0, cell);
        // scale 1.0 (1024 x 768): unchanged cells and ems, places on whole virtual pixels
        assert_eq!(scaled_cell(14.0, 1.0), 14.0);
        assert_eq!((em(14.0), em(13.0)), (12.0, 11.0));
        assert_eq!((scaled_place(491.0, 1.0), scaled_place(128.0, 1.0)), (491.0, 128.0));
        // 2560 x 1440: vertical 1.875, horizontal 2.5
        assert_eq!(scaled_cell(14.0, 1440.0 / 768.0), 26.0, "26.25 truncated");
        assert_eq!(scaled_cell(13.0, 1440.0 / 768.0), 24.0, "24.375 truncated");
        assert_eq!((em(26.0), em(24.0)), (21.0, 20.0), "not 12 x 1.875 / 11 x 1.875");
        assert_eq!(scaled_cell(14.0, 2.5), 35.0);
        assert_eq!(em(35.0), 29.0);
        assert_eq!(scaled_place(491.0, 2.5), 1227.0, "1227.5 truncated");
        assert_eq!(scaled_place(128.0, 1440.0 / 768.0), 240.0);
        assert_eq!(scaled_place(129.0, 1440.0 / 768.0), 241.0, "241.875 truncated");
        // a product that lands on a whole number stays on it
        assert_eq!(scaled_cell(16.0, 1080.0 / 768.0), 22.0, "22.5 truncated");
        assert_eq!(scaled_place(256.0, 1080.0 / 768.0), 360.0);
    }

    #[test]
    fn a_top_left_label_starts_on_the_truncated_screen_place() {
        let label = || DrawItem::Label {
            style: TextStyle {
                font: -1,
                colour: [255; 4],
                size: Some(14),
            },
            pos: [491.0, 129.0],
            text: "AB".into(),
            anchor: [0.0, 0.0],
        };
        for (w, h, x, top) in [(1024, 768, 491.0, 129.0), (2560, 1440, 1227.0, 241.0)] {
            let fonts = HudFonts::new(Some((crate::testkit::synth_font(), 0)), 16.0, None, h as f32 / 768.0);
            let mut a = HudAssets::new(Box::new(Solid), fonts);
            let items = [label()];
            let prims = resolve(&items, &mut a, w, h);
            let Some(Prim::Text { atlas, x: px, baseline, .. }) = prims.first() else {
                panic!("a text prim at {w} x {h}");
            };
            assert_eq!((*px, baseline - atlas.ascent), (x, top), "{w} x {h}");
        }
    }

    #[test]
    fn stacked_frame_parts_leave_no_gap_at_a_fractional_scale() {
        // CLI-911: 1024 x 768 shown at 1024 x 1440 (scale 1.875 down the screen): part A ends at y 2.25, part B starts
        // at whole pixel 3; row 2 used to show what lay under the frame
        let fonts = HudFonts::new(Some((crate::testkit::synth_font(), 0)), 16.0, None, 1.0);
        let mut a = HudAssets::new(Box::new(Solid), fonts);
        let part = |y: f32| DrawItem::Image {
            tex: part_of("red", [0, 0, 4, 4]),
            src: [4.0, 4.0],
            dst: [0.0, y, 20.0, 1.2],
            tint: [255; 4],
            turn: None,
        };
        let img = compose(&[part(0.4), part(1.6)], &mut a, 1024, 1440);
        for y in 0..6 {
            assert_eq!(img.pixel(5, y)[3], 255, "row {y} is covered");
        }
    }

    #[test]
    fn images_scale_turn_and_text_draws() {
        let fonts = HudFonts::new(Some((crate::testkit::synth_font(), 0)), 16.0, None, 1.0);
        let mut a = HudAssets::new(Box::new(Solid), fonts);
        let items = vec![
            DrawItem::Image {
                tex: "red".into(),
                src: [4.0, 4.0],
                dst: [0.0, 0.0, 512.0, 384.0],
                tint: [255; 4],
                turn: None,
            },
            DrawItem::Image {
                tex: "missing".into(),
                src: [4.0, 4.0],
                dst: [600.0, 0.0, 100.0, 100.0],
                tint: [255; 4],
                turn: None,
            },
            // a bar right of the centre turned by a quarter turn: it ends up below the centre
            DrawItem::Image {
                tex: String::new(),
                src: [1.0, 1.0],
                dst: [612.0, 374.0, 100.0, 20.0],
                tint: [0, 0, 255, 255],
                turn: Some(([512.0, 384.0], std::f32::consts::FRAC_PI_2)),
            },
            DrawItem::Text {
                style: TextStyle {
                    font: -1,
                    colour: [0, 255, 0, 255],
                    size: None,
                },
                pos: [700.0, 600.0],
                text: "AB".into(),
                right: false,
            },
        ];
        let img = compose(&items, &mut a, 512, 384);
        assert_eq!(
            img.pixel(10, 10),
            [200, 10, 10, 255],
            "half scale: 256 x 192"
        );
        assert_eq!(img.pixel(300, 10)[3], 0, "outside the first image");
        assert_eq!(img.pixel(320, 10)[3], 0, "missing texture draws nothing");
        assert_eq!(a.missing, vec!["missing".to_string()]);
        // quarter turn: (612..712, 374..394) about (512, 384) -> x 502..522, y 484..584 virtual = (251..261, 242..292)
        assert_eq!(img.pixel(256, 260), [0, 0, 255, 255]);
        assert_eq!(
            img.pixel(330, 192)[3],
            0,
            "nothing left at the unturned place"
        );
        let green = (0..512 * 384)
            .filter(|i| {
                let p = img.pixel(i % 512, i / 512);
                p[1] > 200 && p[0] < 50
            })
            .count();
        assert!(green > 10, "text pixels {green}");
    }

    /// CLI-145 (amended): the turn happens in virtual units and the turned shape is scaled per axis, so on a
    /// 2:1 target (x scale 1, y scale 2/3) a bar turned below the centre lands at 2/3 of its virtual drop, where
    /// a turn in target pixels would have put it a full 100..200 pixels down.
    #[test]
    fn turned_images_turn_in_virtual_units_then_scale_per_axis() {
        let fonts = HudFonts::new(None, 16.0, None, 1.0);
        let mut a = HudAssets::new(Box::new(Solid), fonts);
        let items = vec![DrawItem::Image {
            tex: String::new(),
            src: [1.0, 1.0],
            dst: [612.0, 374.0, 100.0, 20.0],
            tint: [0, 0, 255, 255],
            turn: Some(([512.0, 384.0], std::f32::consts::FRAC_PI_2)),
        }];
        let img = compose(&items, &mut a, 1024, 512);
        // virtual: x 502..522, y 484..584 -> target x 502..522, y 322.7..389.3
        assert_eq!(img.pixel(512, 330), [0, 0, 255, 255], "top of the scaled drop");
        assert_eq!(img.pixel(512, 385), [0, 0, 255, 255], "bottom of the scaled drop");
        assert_eq!(img.pixel(512, 318)[3], 0, "above it");
        assert_eq!(
            img.pixel(512, 440)[3],
            0,
            "a turn in target pixels would have reached 356..456"
        );
        assert_eq!(img.pixel(495, 350)[3], 0, "the width keeps the x scale");
        assert_eq!(img.pixel(530, 350)[3], 0, "the width keeps the x scale");
    }
}
