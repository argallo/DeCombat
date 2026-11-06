//! Our own flag picture for the countries we add after the ROM's country table (CLI-1873): the United States row has
//! no flag in the ROM, so its flag is drawn here in code, a small picture of our own (13 red and white stripes, a blue
//! canton with white dots), made once at catalogue load and served under a name of its own through the same texture
//! cache the ROM flags come from. The country window, the Character Info panel and the lobby's user rows then draw
//! it exactly as they draw a ROM flag.
//!
//! Shape: the ROM's flag pictures are a texture with the flag in its top left corner; ours is made the same way, the
//! flag at the country window's `FlagSize` inside a texture the size of the ROM's first flag picture, both read from
//! the user's ROM at load (no ROM pixel is used).

use crate::hud::compose::Tex;

/// The name the texture cache serves our United States flag under (nothing in the ROM or a mod answers to it).
pub const USA: &str = "@dc:flag_usa";

/// Our flag's look: the flag's size and the texture's size, in texels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagShape {
    pub flag: [u32; 2],
    pub texture: [u32; 2],
}

/// DC-DEFAULT: CLI-1893 (proposed) without a ROM flag size our own shape: 19 x 10 texels (the 1.9 : 1 of the US
/// flag), the texture just as big.
pub const OWN_SHAPE: FlagShape = FlagShape {
    flag: [19, 10],
    texture: [19, 10],
};

impl FlagShape {
    /// The shape from the ROM's `FlagSize` and the size of the ROM's first flag picture; our own shape for what is
    /// missing. The texture is never smaller than the flag.
    pub fn from_rom(flag_size: Option<[i32; 2]>, rom_texture: Option<(u32, u32)>) -> FlagShape {
        let flag = flag_size
            .filter(|s| s[0] > 0 && s[1] > 0)
            .map_or(OWN_SHAPE.flag, |s| [s[0] as u32, s[1] as u32]);
        let texture = rom_texture
            .filter(|t| t.0 > 0 && t.1 > 0)
            .map_or(flag, |t| [t.0.max(flag[0]), t.1.max(flag[1])]);
        FlagShape { flag, texture }
    }
}

/// The colours: red, white, blue (our picks after the public flag colours), opaque.
// DC-DEFAULT: CLI-1893 (proposed)
pub const RED: [u8; 4] = [178, 34, 52, 255];
pub const WHITE: [u8; 4] = [255, 255, 255, 255];
pub const BLUE: [u8; 4] = [60, 59, 110, 255];

/// The stripes.
pub const STRIPES: u32 = 13;

/// The United States flag of `shape`: 13 stripes, red at the top and the bottom, a blue canton over the top seven
/// stripes and two fifths of the width, white dots on every other texel of the canton's inside; transparent outside
/// the flag.
// DC-DEFAULT: CLI-1893 (proposed) our own drawing
pub fn usa(shape: FlagShape) -> Tex {
    let [fw, fh] = shape.flag;
    let [tw, th] = shape.texture;
    let canton = [(fw * 2).div_ceil(5), (fh * 7).div_ceil(STRIPES)];
    let mut rgba = Vec::with_capacity((tw * th * 4) as usize);
    for y in 0..th {
        for x in 0..tw {
            let px = if x >= fw || y >= fh {
                [0, 0, 0, 0]
            } else if x < canton[0] && y < canton[1] {
                let inside = x > 0 && y > 0 && x + 1 < canton[0] && y + 1 < canton[1];
                if inside && x % 2 == 1 && y % 2 == 1 {
                    WHITE
                } else {
                    BLUE
                }
            } else if (y * STRIPES / fh).is_multiple_of(2) {
                RED
            } else {
                WHITE
            };
            rgba.extend_from_slice(&px);
        }
    }
    Tex {
        width: tw,
        height: th,
        rgba,
    }
}

/// Our flag picture of texture name `name` (none for another name).
pub fn texture(name: &str, shape: FlagShape) -> Option<Tex> {
    (name == USA).then(|| usa(shape))
}

impl super::catalog::Catalog {
    /// Puts our flag pictures into the texture cache in the shape of the ROM's flags: the country window's
    /// `FlagSize` and the size of the ROM country table's first flag picture (row 1); our own shape for what is
    /// missing. Returns the shape.
    // EVIDENCE: cshell+0x120d90 (every flag control is built at CountryChangeWnd / FlagSize), cshell+0x2a9e30 (the
    // country window's flags likewise)
    pub fn load_own_flags(&mut self) -> FlagShape {
        let flag_size = super::catalog::pair(self.section_value(
            &super::lobby::row_keys::FLAG_SECTION,
            &super::lobby::row_keys::FLAG_SIZE,
        ));
        let rom_flag = self.rom.clone().and_then(|rom| {
            super::charinfo::Tables::cached(&rom)
                .countries
                .get(1)
                .map(|c| c.2.clone())
        });
        let rom_texture = rom_flag
            .filter(|f| !f.is_empty())
            .and_then(|f| self.tex_size(&f));
        let shape = FlagShape::from_rom(flag_size, rom_texture);
        self.assets.forget(USA);
        self.assets.put(USA, usa(shape));
        shape
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: &Tex, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * t.width + x) * 4) as usize;
        [t.rgba[i], t.rgba[i + 1], t.rgba[i + 2], t.rgba[i + 3]]
    }

    #[test]
    fn the_flag_has_thirteen_stripes_a_canton_and_a_clear_margin() {
        let shape = FlagShape::from_rom(Some([20, 13]), Some((30, 15)));
        assert_eq!(
            shape,
            FlagShape {
                flag: [20, 13],
                texture: [30, 15]
            }
        );
        let t = usa(shape);
        assert_eq!((t.width, t.height, t.rgba.len()), (30, 15, 30 * 15 * 4));
        // the stripes down the fly: red first and last, 13 changes of colour in all
        let column: Vec<[u8; 4]> = (0..13).map(|y| at(&t, 19, y)).collect();
        assert_eq!((column[0], column[12]), (RED, RED));
        let runs = 1 + column.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(runs, STRIPES as usize);
        assert!(column.iter().all(|c| *c == RED || *c == WHITE));
        // the canton: blue at its corner, with white dots inside, over the top stripes only
        assert_eq!(at(&t, 0, 0), BLUE);
        assert_eq!(at(&t, 1, 1), WHITE);
        assert_eq!(at(&t, 7, 6), BLUE);
        assert_ne!(at(&t, 0, 12), BLUE);
        assert_ne!(at(&t, 8, 0), BLUE);
        // outside the flag: clear, as the ROM's flag pictures
        assert_eq!(at(&t, 20, 0)[3], 0);
        assert_eq!(at(&t, 0, 13)[3], 0);
        assert_eq!(at(&t, 29, 14)[3], 0);
    }

    #[test]
    fn without_rom_sizes_the_flag_takes_our_own_shape() {
        assert_eq!(FlagShape::from_rom(None, None), OWN_SHAPE);
        assert_eq!(FlagShape::from_rom(Some([0, 5]), None), OWN_SHAPE);
        // a texture smaller than the flag grows to hold it
        assert_eq!(
            FlagShape::from_rom(Some([20, 13]), Some((16, 8))).texture,
            [20, 13]
        );
        let t = usa(OWN_SHAPE);
        assert_eq!((t.width, t.height), (19, 10));
        assert!(t.rgba.chunks(4).all(|p| p[3] == 255));
        assert!(texture("flag.dtx", OWN_SHAPE).is_none());
        assert!(texture(USA, OWN_SHAPE).is_some());
    }
}
