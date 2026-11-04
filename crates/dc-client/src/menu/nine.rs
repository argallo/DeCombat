//! The 2009 frame control's look: a texture drawn in nine parts over a rectangle. The corners keep a quarter of the
//! texture each at the texture's own size, the edges stretch along their side, the middle over the rest.
// EVIDENCE: cshell+0x343860 (the frame control built from a texture, a width and a height; mode 0 = nine parts),
// cshell+0x343520 (texel splits 0, 0.25, 0.75, 1 on both axes, three parts per row), cshell+0x342d10 (screen
// splits x, x + c, x + w - c, x + w with c = texture width x scale x 0.25, the same on y)

use super::catalog::Catalog;
use crate::hud::compose::part_of;
use crate::hud::items::DrawItem;

/// Draws `tex` in nine parts over `dst` (x, y, w, h) with `tint`; nothing when the texture is missing. Returns the
/// number of items pushed.
pub fn draw(
    cat: &mut Catalog,
    tex: &str,
    dst: [i32; 4],
    tint: [u8; 4],
    out: &mut Vec<DrawItem>,
) -> usize {
    let Some(size) = cat.tex_size(tex) else {
        return 0;
    };
    draw_art(tex, size, dst, tint, out)
}

/// [`draw`] for art that fills only the top-left `art` (w, h) texels of its texture (a texture padded to a power of
/// two): the quarters are taken of the art, so the visible frame covers `dst` exactly.
pub fn draw_art(
    tex: &str,
    art: (u32, u32),
    dst: [i32; 4],
    tint: [u8; 4],
    out: &mut Vec<DrawItem>,
) -> usize {
    let (tw, th) = art;
    let n = out.len();
    let (cw, ch) = (tw / 4, th / 4);
    // texel spans and screen spans of the three columns / rows
    let tx = [(0, cw), (cw, tw - 2 * cw), (tw - cw, cw)];
    let ty = [(0, ch), (ch, th - 2 * ch), (th - ch, ch)];
    let [x, y, w, h] = dst;
    let (cw, ch) = (cw as i32, ch as i32);
    let sx = [(x, cw), (x + cw, w - 2 * cw), (x + w - cw, cw)];
    let sy = [(y, ch), (y + ch, h - 2 * ch), (y + h - ch, ch)];
    for r in 0..3 {
        for c in 0..3 {
            let (src, at) = ((tx[c], ty[r]), (sx[c], sy[r]));
            if src.0 .1 == 0 || src.1 .1 == 0 || at.0 .1 <= 0 || at.1 .1 <= 0 {
                continue;
            }
            out.push(DrawItem::Image {
                tex: part_of(tex, [src.0 .0, src.1 .0, src.0 .1, src.1 .1]),
                src: [src.0 .1 as f32, src.1 .1 as f32],
                dst: [
                    at.0 .0 as f32,
                    at.1 .0 as f32,
                    at.0 .1 as f32,
                    at.1 .1 as f32,
                ],
                tint,
                turn: None,
            });
        }
    }
    out.len() - n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI-1623 layout fix: art padded inside a larger texture takes its quarters of the art, so the right column
    /// samples the art's right edge and the visible frame reaches the rectangle's right edge.
    #[test]
    fn padded_art_takes_its_quarters_of_the_art() {
        let mut out = Vec::new();
        assert_eq!(draw_art("frame.dtx", (478, 258), [100, 50, 800, 600], [255; 4], &mut out), 9);
        let DrawItem::Image { tex, dst, .. } = &out[2] else {
            panic!("an image")
        };
        assert_eq!(tex, &part_of("frame.dtx", [478 - 119, 0, 119, 64]));
        assert_eq!(dst[0] + dst[2], 900.0);
    }
}
