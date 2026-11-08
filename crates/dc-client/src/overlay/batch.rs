//! The overlay's 2D batch: a list of instanced quads (solid fills, glyphs, textured images) drawn in order over
//! the finished frame, and the texture atlas they read from.
//!
//! The atlas keeps every image and glyph sheet the overlay has drawn in a few RGBA8 pages: an image is uploaded
//! the first time it is drawn and again only when its content changes (a new `Arc<Tex>` under the same name);
//! a font's glyphs are packed into one sheet the first time any of its text is drawn. A frame's draw list is then
//! a few kilobytes of quad instances; nothing is rasterised on the CPU per frame.
//!
//! Each quad reproduces, per pixel, what the CPU compositor ([`crate::hud::compose::compose`], [`super::text`]) writes:
//! the same covered pixel ranges, the same glyph placement and coverage arithmetic, the same bilinear filter over
//! texel centres clamped to the source rectangle, the same tint. The quads blend straight alpha "over" one after
//! the other directly into the frame, which is the same sum as composing them into a transparent image first and
//! blending that image once; only the 8-bit rounding of the intermediate image is gone (differences of an LSB
//! or two where translucent items overlap). Drawing each 2D primitive blended straight into the frame is also how
//! the 2009 engine draws its HUD. Engineering only; no 2009 rule is decided here.
// SPEC: RENDER_2009.md §12.1 (2D blend mode SRCALPHA / INVSRCALPHA, linear filter), §12.2 / §12.3 (each 2D
// primitive is drawn with blending on, in the current 2D mode, into the frame)

use super::text::GlyphAtlas;
use dc_render::GpuContext;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

/// What a quad draws.
pub const KIND_FILL: u32 = 0;
pub const KIND_GLYPH: u32 = 1;
pub const KIND_IMAGE: u32 = 2;
/// Flag in `d[0]`: the image has a texture (else it is solid white times the tint).
pub const HAS_TEXTURE: u32 = 0x100;

/// One instance. `rect` is the covered pixel rectangle `[x0, y0, x1, y1)` on the canvas (whole pixels, already
/// clipped to it); the other fields depend on the kind:
/// - fill: `colour` = straight RGBA, 0..255.
/// - glyph: `colour` = RGB 0..255 and the alpha the coverage is scaled by (0..255); `a` = the bitmap's top-left
///   pixel on the canvas and its top-left texel in the atlas.
/// - image: `colour` = tint 0..1; `a` = destination rectangle `[x0, y0, x1, y1]` before the turn; `b` = pivot,
///   cos, sin; `c` = source size (u, v extent) and the clamp limits `max_u`, `max_v`; `d` = kind | flags, atlas
///   texel origin x, y, last texel `lim_x | lim_y << 16`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Quad {
    pub rect: [f32; 4],
    pub colour: [f32; 4],
    pub a: [f32; 4],
    pub b: [f32; 4],
    pub c: [f32; 4],
    pub d: [u32; 4],
    /// Images: the second row of the inverse turn (`[m10, m11, 0, 0]`; the first row rides in `b`).
    pub e: [f32; 4],
}

pub const QUAD_BYTES: u64 = std::mem::size_of::<Quad>() as u64;

/// A frame's quads in draw order, with the atlas page of each.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuadList {
    pub quads: Vec<Quad>,
    pub pages: Vec<u16>,
    /// The canvas the rectangles are clipped to.
    pub canvas: (u32, u32),
    /// CLI-1915: a pixel rectangle `[x0, y0, x1, y1)` the quads pushed while it is set are clipped to as well (the
    /// menus' 2D band on a surface wider than 16:9).
    pub limit: Option<[i32; 4]>,
}

impl QuadList {
    pub fn clear(&mut self, canvas: (u32, u32)) {
        self.quads.clear();
        self.pages.clear();
        self.canvas = canvas;
        self.limit = None;
    }

    /// Clips `r` (whole pixels) to the canvas; `None` when nothing is left.
    fn clip(&self, r: [i32; 4]) -> Option<[f32; 4]> {
        let (w, h) = (self.canvas.0 as i32, self.canvas.1 as i32);
        let (mut x0, mut y0, mut x1, mut y1) = (r[0].max(0), r[1].max(0), r[2].min(w), r[3].min(h));
        if let Some(l) = self.limit {
            (x0, y0, x1, y1) = (x0.max(l[0]), y0.max(l[1]), x1.min(l[2]), y1.min(l[3]));
        }
        (x0 < x1 && y0 < y1).then_some([x0 as f32, y0 as f32, x1 as f32, y1 as f32])
    }

    fn push(&mut self, q: Quad, page: u16) {
        self.quads.push(q);
        self.pages.push(page);
    }

    /// A solid rectangle of straight colour `c` (0..255) over pixels `[x0, x1) x [y0, y1)`.
    pub fn fill(&mut self, r: [i32; 4], c: [f32; 4]) {
        if c[3] <= 0.0 {
            return;
        }
        if let Some(rect) = self.clip(r) {
            let page = self.pages.last().copied().unwrap_or(0);
            self.push(
                Quad {
                    rect,
                    colour: c,
                    a: [0.0; 4],
                    b: [0.0; 4],
                    c: [0.0; 4],
                    d: [KIND_FILL, 0, 0, 0],
                    e: [0.0; 4],
                },
                page,
            );
        }
    }

    /// The glyphs of `text` (as [`GlyphAtlas::draw_rgba`] places them), colour `rgb`, coverage scaled by `alpha`.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        sheet: &GlyphSheet,
        atlas: &GlyphAtlas,
        x: f32,
        baseline: f32,
        text: &str,
        rgb: [u8; 3],
        alpha: u8,
    ) {
        let colour = [rgb[0] as f32, rgb[1] as f32, rgb[2] as f32, alpha as f32];
        atlas.layout(x, baseline, text, |i, bm, ox, oy| {
            let Some(rect) = self.clip([ox, oy, ox + bm.width as i32, oy + bm.height as i32])
            else {
                return;
            };
            let (tx, ty) = sheet.at[i];
            self.push(
                Quad {
                    rect,
                    colour,
                    a: [ox as f32, oy as f32, tx as f32, ty as f32],
                    b: [0.0; 4],
                    c: [0.0; 4],
                    d: [KIND_GLYPH, 0, 0, 0],
                    e: [0.0; 4],
                },
                sheet.page,
            );
        });
    }

    /// A textured (or, with `region = None`, white) image quad; `px` is the covered pixel range and `turn` the
    /// compositor's `[pivot x, pivot y, m00, m01, m10, m11]` (see `hud::compose::Prim::Image`).
    #[allow(clippy::too_many_arguments)]
    pub fn image(
        &mut self,
        px: [i32; 4],
        dst: [f32; 4],
        turn: [f32; 6],
        src: [f32; 2],
        tex_size: Option<(u32, u32)>,
        region: Option<Region>,
        tint: [f32; 4],
    ) {
        let Some(rect) = self.clip(px) else {
            return;
        };
        let (mut flags, mut c, mut d1, mut d2, mut lim, mut page) =
            (KIND_IMAGE, [src[0], src[1], 0.0, 0.0], 0, 0, 0, 0);
        if let (Some(r), Some((tw, th))) = (region, tex_size) {
            let (max_u, max_v) = (src[0].min(tw as f32), src[1].min(th as f32));
            // the compositor's last texel: (max_u as u32).min(width).max(1) - 1
            let lim_x = (max_u as u32).min(tw).max(1) - 1;
            let lim_y = (max_v as u32).min(th).max(1) - 1;
            flags |= HAS_TEXTURE;
            c = [src[0], src[1], max_u, max_v];
            d1 = r.x;
            d2 = r.y;
            lim = lim_x | (lim_y << 16);
            page = r.page;
        }
        self.push(
            Quad {
                rect,
                colour: tint,
                a: dst,
                b: [turn[0], turn[1], turn[2], turn[3]],
                c,
                d: [flags, d1, d2, lim],
                e: [turn[4], turn[5], 0.0, 0.0],
            },
            page,
        );
    }

    /// Consecutive runs of quads on the same page: (page, first, count).
    pub fn runs(&self) -> Vec<(u16, u32, u32)> {
        let mut out: Vec<(u16, u32, u32)> = Vec::new();
        for (i, &p) in self.pages.iter().enumerate() {
            let kind = self.quads[i].d[0] & 0xff;
            match out.last_mut() {
                // fills read no texture: they join whatever run they follow
                Some(last) if last.0 == p || kind == KIND_FILL => last.2 += 1,
                _ => out.push((p, i as u32, 1)),
            }
        }
        out
    }
}

/// A rectangle of texels in an atlas page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub page: u16,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The glyphs of one [`GlyphAtlas`] in the atlas: the top-left texel of each glyph (index order).
#[derive(Debug, Clone)]
pub struct GlyphSheet {
    pub page: u16,
    pub at: Vec<(u32, u32)>,
}

/// Shelf packer: rows filled left to right, a new row below when the current one is full.
#[derive(Debug, Clone)]
pub struct Shelves {
    pub w: u32,
    pub h: u32,
    x: u32,
    y: u32,
    row: u32,
}

impl Shelves {
    pub fn new(w: u32, h: u32) -> Shelves {
        Shelves {
            w,
            h,
            x: 0,
            y: 0,
            row: 0,
        }
    }

    pub fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > self.w || h > self.h {
            return None;
        }
        if self.x + w > self.w {
            self.y += self.row;
            self.x = 0;
            self.row = 0;
        }
        if self.y + h > self.h {
            return None;
        }
        let at = (self.x, self.y);
        self.x += w;
        self.row = self.row.max(h);
        Some(at)
    }

    pub fn clear(&mut self) {
        (self.x, self.y, self.row) = (0, 0, 0);
    }
}

/// Lays the glyph bitmaps of `atlas` out in a sheet `width` texels wide: (sheet size, top-left of each glyph).
pub fn glyph_sheet_layout(atlas: &GlyphAtlas, width: u32) -> ((u32, u32), Vec<(u32, u32)>) {
    let mut sh = Shelves::new(width, u32::MAX);
    let mut at = Vec::with_capacity(atlas.bitmaps().len());
    let mut size = (1, 1);
    for bm in atlas.bitmaps() {
        // a 1-texel gutter keeps neighbours apart (texels are read whole, never filtered across)
        let (w, h) = (bm.width.min(width - 1) + 1, bm.height + 1);
        let p = sh.alloc(w, h).unwrap_or((0, 0));
        size = (size.0.max(p.0 + w), size.1.max(p.1 + h));
        at.push(p);
    }
    (size, at)
}

/// The atlas is full and could not grow: the caller clears it and builds the frame again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasFull;

/// Largest number of pages before the atlas is cleared and refilled from what the next frame draws.
pub const MAX_PAGES: usize = 4;
/// Side of a regular page (texels); larger images get a page of their own size.
pub const PAGE_SIDE: u32 = 2048;
/// Width of a glyph sheet before it wraps.
const SHEET_WIDTH: u32 = 512;

struct Page {
    texture: wgpu::Texture,
    group: wgpu::BindGroup,
    shelves: Shelves,
}

struct ImageEntry {
    region: Region,
    /// The texture last uploaded under this name (the weak count keeps its allocation, so the address is not
    /// reused while the entry lives).
    ident: Weak<super::super::hud::Tex>,
}

/// The overlay atlas: RGBA8 pages with their bind groups.
pub struct Atlas {
    layout: wgpu::BindGroupLayout,
    pages: Vec<Page>,
    images: HashMap<String, ImageEntry>,
    glyphs: HashMap<u64, GlyphSheet>,
    max_dim: u32,
    /// Texel bytes uploaded since the last call of [`Atlas::take_uploaded`].
    uploaded: u64,
    pub resets: u64,
}

impl Atlas {
    pub fn new(ctx: &GpuContext, layout: wgpu::BindGroupLayout) -> Atlas {
        Atlas {
            layout,
            pages: Vec::new(),
            images: HashMap::new(),
            glyphs: HashMap::new(),
            max_dim: ctx.device.limits().max_texture_dimension_2d,
            uploaded: 0,
            resets: 0,
        }
    }

    pub fn page_group(&self, page: u16) -> Option<&wgpu::BindGroup> {
        self.pages.get(page as usize).map(|p| &p.group)
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn take_uploaded(&mut self) -> u64 {
        std::mem::take(&mut self.uploaded)
    }

    /// Forgets every entry (the pages stay allocated and are refilled).
    pub fn clear(&mut self) {
        self.images.clear();
        self.glyphs.clear();
        for p in &mut self.pages {
            p.shelves.clear();
        }
        self.resets += 1;
    }

    fn add_page(&mut self, ctx: &GpuContext, w: u32, h: u32) -> Result<u16, AtlasFull> {
        if self.pages.len() >= MAX_PAGES || w > self.max_dim || h > self.max_dim {
            return Err(AtlasFull);
        }
        let side = |n: u32| n.max(PAGE_SIDE).next_power_of_two().min(self.max_dim);
        let (pw, ph) = (side(w), side(h));
        let texture = dc_render::perf::texture(
            &ctx.device,
            &wgpu::TextureDescriptor {
                label: Some("overlay atlas page"),
                size: wgpu::Extent3d {
                    width: pw,
                    height: ph,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
        );
        let view = texture.create_view(&Default::default());
        let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay atlas page"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        self.pages.push(Page {
            texture,
            group,
            shelves: Shelves::new(pw, ph),
        });
        Ok((self.pages.len() - 1) as u16)
    }

    fn alloc(&mut self, ctx: &GpuContext, w: u32, h: u32) -> Result<Region, AtlasFull> {
        for (i, p) in self.pages.iter_mut().enumerate() {
            if let Some((x, y)) = p.shelves.alloc(w, h) {
                return Ok(Region {
                    page: i as u16,
                    x,
                    y,
                    w,
                    h,
                });
            }
        }
        let page = self.add_page(ctx, w, h)?;
        let (x, y) = self.pages[page as usize]
            .shelves
            .alloc(w, h)
            .ok_or(AtlasFull)?;
        Ok(Region { page, x, y, w, h })
    }

    fn upload(&mut self, ctx: &GpuContext, r: Region, rgba: &[u8]) {
        if r.w == 0 || r.h == 0 {
            return;
        }
        dc_render::perf::write_texture(
            &ctx.queue,
            wgpu::TexelCopyTextureInfo {
                texture: &self.pages[r.page as usize].texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: r.x,
                    y: r.y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &rgba[..(4 * r.w * r.h) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * r.w),
                rows_per_image: Some(r.h),
            },
            wgpu::Extent3d {
                width: r.w,
                height: r.h,
                depth_or_array_layers: 1,
            },
        );
        self.uploaded += (4 * r.w * r.h) as u64;
    }

    /// The region of the texture drawn under `name`, uploading it when it is new or its content changed.
    pub fn image(
        &mut self,
        ctx: &GpuContext,
        name: &str,
        tex: &Arc<super::super::hud::Tex>,
    ) -> Result<Region, AtlasFull> {
        let (w, h) = (tex.width.max(1), tex.height.max(1));
        if tex.rgba.len() < (4 * tex.width * tex.height) as usize
            || tex.width == 0
            || tex.height == 0
        {
            // nothing usable to upload: a 1x1 transparent texel
            let r = match self.images.get(name) {
                Some(e) => e.region,
                None => self.alloc(ctx, 1, 1)?,
            };
            self.upload(ctx, Region { w: 1, h: 1, ..r }, &[0, 0, 0, 0]);
            self.images.insert(
                name.to_string(),
                ImageEntry {
                    region: Region { w: 1, h: 1, ..r },
                    ident: Arc::downgrade(tex),
                },
            );
            return Ok(Region { w: 1, h: 1, ..r });
        }
        if let Some(e) = self.images.get(name) {
            if Weak::as_ptr(&e.ident) == Arc::as_ptr(tex) {
                return Ok(e.region);
            }
        }
        // new, or changed content: in place when the size is the same
        let region = match self.images.get(name) {
            Some(e) if e.region.w == w && e.region.h == h => e.region,
            _ => self.alloc(ctx, w, h)?,
        };
        self.upload(ctx, region, &tex.rgba);
        self.images.insert(
            name.to_string(),
            ImageEntry {
                region,
                ident: Arc::downgrade(tex),
            },
        );
        Ok(region)
    }

    /// The glyph sheet of `atlas`, packed and uploaded the first time.
    pub fn glyphs(
        &mut self,
        ctx: &GpuContext,
        atlas: &GlyphAtlas,
    ) -> Result<GlyphSheet, AtlasFull> {
        if let Some(s) = self.glyphs.get(&atlas.id()) {
            return Ok(s.clone());
        }
        let ((sw, sh), at) = glyph_sheet_layout(atlas, SHEET_WIDTH);
        let mut rgba = vec![0u8; (4 * sw * sh) as usize];
        for (bm, &(x, y)) in atlas.bitmaps().zip(&at) {
            for row in 0..bm.height {
                for col in 0..bm.width.min(sw.saturating_sub(x)) {
                    let o = (4 * ((y + row) * sw + x + col)) as usize;
                    rgba[o..o + 3].copy_from_slice(&[255, 255, 255]);
                    rgba[o + 3] = bm.alpha[(row * bm.width + col) as usize];
                }
            }
        }
        let r = self.alloc(ctx, sw, sh)?;
        self.upload(ctx, r, &rgba);
        let sheet = GlyphSheet {
            page: r.page,
            at: at.iter().map(|&(x, y)| (r.x + x, r.y + y)).collect(),
        };
        self.glyphs.insert(atlas.id(), sheet.clone());
        Ok(sheet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelves_fill_rows_then_refuse() {
        let mut s = Shelves::new(10, 6);
        assert_eq!(s.alloc(6, 3), Some((0, 0)));
        assert_eq!(s.alloc(4, 2), Some((6, 0)));
        assert_eq!(s.alloc(5, 3), Some((0, 3)), "a new row below the tallest");
        assert_eq!(s.alloc(6, 1), None, "past the bottom");
        assert_eq!(s.alloc(11, 1), None, "wider than the page");
        s.clear();
        assert_eq!(s.alloc(10, 6), Some((0, 0)));
    }

    #[test]
    fn quads_clip_to_the_canvas_and_runs_follow_pages() {
        let mut l = QuadList::default();
        l.clear((100, 50));
        l.fill([-5, -5, 10, 10], [1.0, 2.0, 3.0, 255.0]);
        l.fill([90, 40, 120, 60], [1.0, 2.0, 3.0, 255.0]);
        l.fill([200, 0, 210, 10], [1.0, 2.0, 3.0, 255.0]);
        l.fill([0, 0, 10, 10], [1.0, 2.0, 3.0, 0.0]);
        assert_eq!(l.quads.len(), 2, "outside and invisible fills are dropped");
        assert_eq!(l.quads[0].rect, [0.0, 0.0, 10.0, 10.0]);
        assert_eq!(l.quads[1].rect, [90.0, 40.0, 100.0, 50.0]);
        let tex = Region {
            page: 1,
            x: 0,
            y: 0,
            w: 4,
            h: 4,
        };
        l.image(
            [0, 0, 4, 4],
            [0.0, 0.0, 4.0, 4.0],
            [0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
            [4.0, 4.0],
            Some((4, 4)),
            Some(tex),
            [1.0; 4],
        );
        l.fill([0, 0, 1, 1], [1.0, 1.0, 1.0, 255.0]);
        l.image(
            [0, 0, 4, 4],
            [0.0, 0.0, 4.0, 4.0],
            [0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
            [4.0, 4.0],
            None,
            None,
            [1.0; 4],
        );
        assert_eq!(l.runs(), vec![(0, 0, 2), (1, 2, 2), (0, 4, 1)]);
        assert_eq!(l.quads[2].d[3], 3 | (3 << 16), "last texel 3, 3");
        assert_eq!(l.quads[2].d[0], KIND_IMAGE | HAS_TEXTURE);
        assert_eq!(l.quads[4].d[0], KIND_IMAGE, "no texture: white");
    }
}
