//! The front end's own mouse cursor (user finding L1): the 2009 shell loads a PCX image from the logo archive once
//! and, while the hardware cursor is off (its default), draws it over the finished screen at the pointer every frame;
//! the system cursor is hidden over the window ([`super::window`]).
//!
//! The image is decoded by [`dc_tex::decode_pcx`], keyed (black texels transparent) and stored in the catalogue's
//! texture cache under its archive path; [`item`] is the display-list item [`super::Menu`] puts last.
// EVIDENCE: cshell+0x1bf010, cshell+0x1ef3a0 (the cursor image loaded once by the path the archive entry carries,
// through the engine's load-surface-from-file entry, engine+0x9b1f0 slot +0x12c; the hardware cursor only when the
// "HardwareCursor" option is > 0 and "DisableHardwareCursor" is 0, both default 0)
// DISASM: cshell+0x1ef530 (each frame without the hardware cursor: the image drawn through engine slot +0x124 with
// its top-left at the shell's pointer position (shell +0x5300 / +0x5304, screen pixels), no offset, no scaling,
// after the 2D frame is begun; nothing is drawn after it)
// SPEC: FMT_SPR_LTC_FX.md §9.4 (the three `.pcx` entries of the logo archive), FMT_PCX.md

use super::catalog::Catalog;
use crate::hud::compose::Tex;
use crate::hud::items::DrawItem;
use dc_ui_data::VirtualScaler;
use std::path::Path;

/// The archive holding the cursor image (the logo archive).
// EVIDENCE: cshell+0x1ef3a0 (the entry path's first directory names it); OBSERVED: the archive's directory lists
// the entry under this path (2026-10-27, eu-test ROM)
pub const CURSOR_REZ: &str = "LOGO.rez";
/// The cursor image's entry path (also its name in the texture cache).
// EVIDENCE: cshell+0x1bf010, cshell+0x1ef3a0 (the literal path both loaders pass)
pub const CURSOR_ENTRY: &str = "LOGO\\CURSOR0.PCX";

/// The hot spot inside the image (texels from its top-left): the image's top-left is the pointer.
// DISASM: cshell+0x1ef530 (the draw call takes the pointer position as the image's top-left, no offset)
pub const HOT_SPOT: [f32; 2] = [0.0, 0.0];

/// The pointer the menu assumes before the first pointer event (headless runs, the dumps): the screen centre.
// DC-DEFAULT: CLI-482 (proposed) no 2009 rule for the pointer at screen entry (the 2009 client draws at the system
// pointer, wherever it is); the virtual screen's centre
pub const DEFAULT_POINTER: [f32; 2] = [512.0, 384.0];

/// The decoded cursor image, black texels made transparent.
// DC-DEFAULT: CLI-481 (proposed) the colour key: texels of exactly (0, 0, 0) are fully transparent, every other
// texel opaque. OBSERVED: the ROM image is 32 x 32, 24-bit, its pure-black texels all lie outside the drawn shape
// (the shape's own dark outline is not pure black); the meaning of the draw call's last argument (0x80000000,
// cshell+0x1ef530) inside the engine was not traced
pub fn keyed(img: dc_tex::PcxImage) -> Tex {
    let mut rgba = img.rgba;
    for px in rgba.as_chunks_mut::<4>().0 {
        if px[..3] == [0, 0, 0] {
            px[3] = 0;
        }
    }
    Tex {
        width: img.width,
        height: img.height,
        rgba,
    }
}

/// Reads and decodes the cursor image from the ROM's logo archive (`None`: no archive, no entry or a bad file).
pub fn load(vfs: &dc_rez::RezVfs) -> Option<Tex> {
    let bytes = vfs.entry(CURSOR_REZ, CURSOR_ENTRY).ok()??;
    match dc_tex::decode_pcx(&bytes) {
        Ok(img) => Some(keyed(img)),
        Err(e) => {
            eprintln!("dc-client menu: cursor image not decoded: {e}");
            None
        }
    }
}

/// [`load`] from the client folder `rom`.
pub fn load_rom(rom: &Path) -> Option<Tex> {
    load(&dc_rez::RezVfs::new(rom))
}

/// The cursor's display-list item at `pointer` (virtual pixels), drawn one texel per target pixel on a `target`
/// sized frame (the 2009 draw does not scale it).
pub fn item(size: [u32; 2], pointer: [f32; 2], target: (u32, u32)) -> DrawItem {
    let sc = VirtualScaler::new(target.0.max(1), target.1.max(1));
    let (w, h) = (size[0] as f32, size[1] as f32);
    DrawItem::Image {
        tex: CURSOR_ENTRY.to_string(),
        src: [w, h],
        dst: [
            pointer[0] - HOT_SPOT[0] / sc.sx,
            pointer[1] - HOT_SPOT[1] / sc.sy,
            w / sc.sx,
            h / sc.sy,
        ],
        tint: [255, 255, 255, 255],
        turn: None,
    }
}

/// The item is the cursor image (the step runner's click probe leaves it out: a pointer move is not an effect).
pub fn is_cursor(item: &DrawItem) -> bool {
    matches!(item, DrawItem::Image { tex, .. } if tex == CURSOR_ENTRY)
}

impl Catalog {
    /// Loads the cursor image into the texture cache (once; a later call does nothing); returns its size.
    pub fn load_cursor(&mut self, vfs: &dc_rez::RezVfs) -> Option<[u32; 2]> {
        if let Some(t) = self.assets.generated(CURSOR_ENTRY) {
            return Some([t.width, t.height]);
        }
        let tex = load(vfs)?;
        let size = [tex.width, tex.height];
        self.assets.put(CURSOR_ENTRY, tex);
        Some(size)
    }

    /// The loaded cursor image's size (`None`: no image, the system cursor stays).
    pub fn cursor_size(&self) -> Option<[u32; 2]> {
        self.assets
            .generated(CURSOR_ENTRY)
            .map(|t| [t.width, t.height])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_is_keyed_out() {
        let t = keyed(dc_tex::PcxImage {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 255, 1, 0, 0, 255],
        });
        assert_eq!(t.rgba, vec![0, 0, 0, 0, 1, 0, 0, 255]);
    }

    #[test]
    fn item_is_one_texel_per_pixel_at_the_pointer() {
        let DrawItem::Image { dst, src, .. } = item([32, 32], [100.0, 50.0], (2048, 1536)) else {
            panic!("image");
        };
        assert_eq!(src, [32.0, 32.0]);
        assert_eq!(dst, [100.0, 50.0, 16.0, 16.0]);
        assert!(is_cursor(&item([32, 32], [0.0, 0.0], (1024, 768))));
    }
}
