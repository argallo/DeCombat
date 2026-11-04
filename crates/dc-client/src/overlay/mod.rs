//! Debug overlay: lines of text drawn into a small RGBA panel, uploaded and drawn over the world.
//!
//! Text rendering is our own: one ROM TrueType font is located through dc-ui-data's font records and
//! resolver, its ASCII glyphs are rasterised once into a CPU glyph atlas ([`ttf`], [`raster`]), and every
//! frame the lines are composed into an RGBA panel ([`text`]) that [`gpu::OverlayRenderer`] draws with alpha
//! blending after the world passes. Without a usable font the overlay is skipped (the lines still go to the
//! log in headless mode).
// SPEC: FMT_CRES_UI.md §7.1 (system font records), §7.3 (font archive)

pub mod batch;
pub mod gpu;
pub mod gridfit;
pub mod hud_quads;
pub mod raster;
pub mod text;
pub mod ttf;

pub use gpu::OverlayRenderer;
pub use text::{GlyphAtlas, RgbaImage};

use dc_ui_data::{FontResolver, LayoutTable, SystemFonts, UiNames};
use std::path::Path;

/// Overlay text size limits (pixels). A DeCombat choice for a readable debug panel.
pub const MIN_PX: u32 = 12;
pub const MAX_PX: u32 = 20;

/// A font ready for the overlay.
pub struct OverlayFont {
    pub atlas: GlyphAtlas,
    /// Where it came from (record index, archive entry, face), for logs.
    pub source: String,
}

/// Loads the overlay font from the user's ROM: the HUD font record first, then the help font record, then
/// every other record in table order; the first record whose file resolves in the font archive and parses
/// as a `glyf` font wins (in a collection, the face named by the record, else face 0).
// SPEC: FMT_CRES_UI.md §7.1 (records, fallback to slot 0), §7.3 (font archive, collections)
pub fn load_rom_font(rom: &Path) -> Result<OverlayFont, String> {
    let (font, px, source, _) = rom_font_choice(rom)?;
    Ok(OverlayFont {
        atlas: GlyphAtlas::new(&font, px),
        source,
    })
}

/// The bytes of the font file [`load_rom_font`] picks (the HUD uses it for records without a usable file).
pub fn load_rom_font_bytes(rom: &Path) -> Option<(Vec<u8>, usize)> {
    rom_font_choice(rom)
        .ok()
        .map(|(f, _, _, face)| (f.into_data(), face))
}

fn rom_font_choice(rom: &Path) -> Result<(ttf::Font, f32, String, usize), String> {
    let names = UiNames::from_rom(rom).map_err(|e| e.to_string())?;
    let vfs = dc_rez::RezVfs::new(rom);
    let layout = LayoutTable::load(&vfs, names).map_err(|e| e.to_string())?;
    let fonts = SystemFonts::from_layout(&layout);
    let resolver = FontResolver::open(rom).map_err(|e| e.to_string())?;
    let mut order: Vec<usize> = Vec::new();
    for i in [layout.hud_font_index(), layout.help_font_index()]
        .into_iter()
        .flatten()
    {
        if i >= 0 {
            order.push(i as usize);
        }
    }
    order.extend(0..fonts.len());
    let mut tried = Vec::new();
    for i in order {
        let Some(spec) = fonts.get(i, false) else {
            continue;
        };
        let Some(found) = resolver.resolve(spec) else {
            continue;
        };
        let Ok(bytes) = resolver.read(&found) else {
            continue;
        };
        let face = dc_ui_data::fonts::font_file_info(&bytes)
            .and_then(|info| {
                info.face_offsets.iter().position(|&o| {
                    dc_ui_data::fonts::face_names(&bytes, o)
                        .iter()
                        .any(|n| n.eq_ignore_ascii_case(&spec.face))
                })
            })
            .unwrap_or(0);
        match ttf::Font::parse(bytes, face) {
            Ok(font) => {
                let px = spec.size_px().clamp(MIN_PX, MAX_PX);
                let source = format!(
                    "font record {i} ({}), file {}, face {face}, {px} px",
                    spec.face, found.entry
                );
                return Ok((font, px as f32, source, face));
            }
            Err(e) => tried.push(format!("record {i}: {e}")),
        }
    }
    Err(format!(
        "no usable TrueType font record ({})",
        tried.join("; ")
    ))
}

/// A font from raw bytes (tests, user-supplied files).
pub fn font_from_bytes(bytes: Vec<u8>, px: f32) -> Result<OverlayFont, String> {
    let font = ttf::Font::parse(bytes, 0).map_err(|e| e.to_string())?;
    Ok(OverlayFont {
        atlas: GlyphAtlas::new(&font, px),
        source: "font bytes".into(),
    })
}
