//! The menu screens on the GPU: the menu's display list drawn as overlay quads straight into the frame, the way
//! the 2009 front end draws its controls (each control a textured quad blended SRCALPHA / INVSRCALPHA in screen
//! space, every frame), instead of one screen-sized image composed on the CPU and uploaded after every input.
//!
//! - Textures and glyph sheets go into the overlay atlas once ([`crate::overlay::batch`]); decoded textures stay
//!   in the catalogue's cache ([`crate::hud::HudAssets`]), so no ROM read or decode happens after a screen's first
//!   frame.
//! - The quads are rebuilt only when the menu asks for a redraw AND its display list differs from the drawn one (a
//!   pointer move that changes no hover state costs the list only); an unchanged list uploads nothing.
//! - [`Menu::image`] stays the CPU reference (the headless PNGs and the tests draw with it); the two agree to the
//!   8-bit rounding of the CPU's intermediate image (`tests/menu_paint.rs`).
// SPEC: RENDER_2009.md §12.1 (2D blend mode 0 SRCALPHA / INVSRCALPHA, linear filter), §12.3 (draw-primitive
// interface: screen-space textured quads for HUD and menus)
// EVIDENCE: engine+0x27300 (the 2D draw object's batch: screen-space vertices with rhw 1.0, vertex layout 0x144,
// staged in blocks of at most 125 vertices and drawn each call: no cached screen image)
// DC-DEFAULT: REN-50 (proposed) menu screens as per-item GPU quads, rebuilt only when the display list changes; the
// CPU image stays the reference for PNGs and tests (at most 3 per channel apart)

use super::Menu;
use crate::hud::items::DrawItem;
use crate::overlay::OverlayRenderer;
use crate::prof::{Laps, Stage};
use dc_render::GpuContext;

/// Draws a [`Menu`] with the overlay's batched quads.
pub struct MenuPainter {
    layer: OverlayRenderer,
    /// The display list and target size the quads were last built from.
    drawn: Option<(Vec<DrawItem>, (u32, u32))>,
    /// Quad rebuilds so far (tests, logs).
    pub builds: u64,
    /// gpu-lost-menu: the window device's generation the GPU objects were made on ([`crate::gpulost::device_generation`]).
    gen: u64,
    format: wgpu::TextureFormat,
}

impl MenuPainter {
    pub fn new(ctx: &GpuContext, format: wgpu::TextureFormat) -> MenuPainter {
        MenuPainter {
            layer: OverlayRenderer::new(ctx, format),
            drawn: None,
            builds: 0,
            gen: crate::gpulost::device_generation(),
            format,
        }
    }

    /// gpu-lost-menu: the painter for the window handed back by a match (`ctx`, `format`: the window's device and
    /// surface format now). When the match's recovery replaced the device since this painter was made (or the format
    /// changed), its objects belong to the old device: they are put away without a drop ([`crate::gpulost::retire`],
    /// the lost device's objects must not run wgpu-core's drop paths) and a new painter is made on `ctx`; it builds
    /// its quads and atlas at the menu's next frame. Otherwise the painter is kept as it is. Once per match return.
    // ORIGINAL: our own device-recovery bookkeeping (gpu-lost-menu); no 2009 behaviour
    pub fn for_device(self, ctx: &GpuContext, format: wgpu::TextureFormat) -> MenuPainter {
        if self.gen == crate::gpulost::device_generation() && self.format == format {
            return self;
        }
        crate::gpulost::retire(self);
        println!("dc-client menu: the GPU device was replaced in the match: the menu's painter is made again");
        MenuPainter::new(ctx, format)
    }

    /// The window device's generation this painter was made on.
    pub fn device_gen(&self) -> u64 {
        self.gen
    }

    /// Brings the quads up to date with the menu (nothing when it needs no redraw or its display list is
    /// unchanged). The display list books to the profiler's `hud_compose` stage, the quad build and upload to
    /// `hud_upload`. Returns true when the quads were rebuilt.
    pub fn prepare(&mut self, ctx: &GpuContext, menu: &mut Menu, laps: &mut Laps) -> bool {
        if !menu.needs_redraw() {
            return false;
        }
        let items = menu.frame_items();
        let size = menu.cat.size();
        laps.lap(Stage::HudCompose);
        if self
            .drawn
            .as_ref()
            .is_some_and(|(d, s)| *s == size && *d == items)
        {
            return false;
        }
        // CLI-1915: the screen clipped to the 2D band, the cursor over the whole window
        let free = super::cursor::trailing(&items);
        self.layer
            .update_menu_items(ctx, &items, &mut menu.cat.assets, size, free);
        self.drawn = Some((items, size));
        self.builds += 1;
        laps.lap(Stage::HudUpload);
        true
    }

    /// Records the menu's frame into `target` (`size` pixels): a black clear, then the quads.
    pub fn encode(
        &self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("menu clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        self.layer.encode(ctx, encoder, target, size, (0, 0));
    }

    /// Bytes the last rebuild uploaded: (atlas texels, quad instances).
    pub fn last_upload(&self) -> (u64, u64) {
        self.layer.last_upload()
    }

    /// The display list the quads were last built from (with the backdrop; tests compose it on the CPU).
    pub fn drawn_items(&self) -> Option<&[DrawItem]> {
        self.drawn.as_ref().map(|(d, _)| d.as_slice())
    }

    /// Quads drawn per frame.
    pub fn quad_count(&self) -> usize {
        self.layer.quad_count()
    }
}
