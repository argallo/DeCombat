//! The HUD display list as overlay quads: the GPU counterpart of [`crate::hud::compose::compose`].
//!
//! Both paths draw from the same resolved primitives ([`crate::hud::compose::resolve`]): this module only maps
//! each [`Prim`] to quads with the same values before blending (see [`super::batch`]); `compose` stays the CPU
//! reference (tests compare both, and the headless report's HUD image is still composed by it).
// SPEC: RENDER_2009.md §12.1 (2D blend mode 0 SRCALPHA / INVSRCALPHA, linear filter), §12.4 (ROM TrueType fonts)

use super::batch::{Atlas, AtlasFull, QuadList};
use crate::hud::compose::{resolve, HudAssets, Prim};
use crate::hud::items::DrawItem;
use dc_render::GpuContext;

/// Appends the quads of `items` on a `width` x `height` target to `list` (cleared first), uploading new
/// textures and glyph sheets into `atlas`.
pub fn hud_quads(
    ctx: &GpuContext,
    atlas: &mut Atlas,
    list: &mut QuadList,
    items: &[DrawItem],
    assets: &mut HudAssets,
    width: u32,
    height: u32,
) -> Result<(), AtlasFull> {
    list.clear((width, height));
    push_prims(ctx, atlas, list, items, assets, width, height)
}

/// [`hud_quads`] for a menu screen: the items before the last `free` ones (the cursor) are clipped to the 2D band
/// (CLI-1915: the side bands of a surface wider than 16:9 stay the frame's black), the last ones are not.
#[allow(clippy::too_many_arguments)]
pub fn menu_quads(
    ctx: &GpuContext,
    atlas: &mut Atlas,
    list: &mut QuadList,
    items: &[DrawItem],
    assets: &mut HudAssets,
    width: u32,
    height: u32,
    free: usize,
) -> Result<(), AtlasFull> {
    list.clear((width, height));
    let split = items.len() - free.min(items.len());
    let sc = dc_ui_data::VirtualScaler::new(width, height);
    if sc.banded() {
        let [x, y, w, h] = sc.band().map(|v| v as i32);
        list.limit = Some([x, y, x + w, y + h]);
    }
    push_prims(ctx, atlas, list, &items[..split], assets, width, height)?;
    list.limit = None;
    push_prims(ctx, atlas, list, &items[split..], assets, width, height)
}

/// Appends the quads of `items` to `list` (not cleared).
fn push_prims(
    ctx: &GpuContext,
    atlas: &mut Atlas,
    list: &mut QuadList,
    items: &[DrawItem],
    assets: &mut HudAssets,
    width: u32,
    height: u32,
) -> Result<(), AtlasFull> {
    for p in resolve(items, assets, width, height) {
        match p {
            Prim::Fill { rect, colour } => list.fill(rect, colour),
            Prim::Text {
                atlas: ga,
                x,
                baseline,
                text,
                colour: c,
            } => {
                let sheet = atlas.glyphs(ctx, &ga)?;
                list.text(&sheet, &ga, x, baseline, text, [c[0], c[1], c[2]], c[3]);
            }
            Prim::Image {
                name,
                tex,
                bbox,
                rect,
                turn,
                src,
                tint,
            } => {
                let (region, size) = match &tex {
                    Some(t) => (Some(atlas.image(ctx, name, t)?), Some((t.width, t.height))),
                    None => (None, None),
                };
                list.image(bbox, rect, turn, src, size, region, tint);
            }
        }
    }
    Ok(())
}

/// The debug panel of `lines` as quads (the GPU counterpart of [`super::text::GlyphAtlas::panel`]): the
/// background over the whole panel, then the lines.
pub fn panel_quads(
    ctx: &GpuContext,
    atlas: &mut Atlas,
    list: &mut QuadList,
    font: &super::text::GlyphAtlas,
    lines: &[String],
) -> Result<(), AtlasFull> {
    let (w, h, pad) = font.panel_size(lines);
    list.clear((w, h));
    list.fill(
        [0, 0, w as i32, h as i32],
        super::text::PANEL_BACKGROUND.map(|v| v as f32),
    );
    let sheet = atlas.glyphs(ctx, font)?;
    for (i, l) in lines.iter().enumerate() {
        let baseline = font.panel_baseline(pad, i);
        list.text(&sheet, font, pad, baseline, l, super::text::PANEL_TEXT, 255);
    }
    Ok(())
}
