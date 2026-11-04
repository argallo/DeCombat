//! Readers for the 2009 UI data of the user's client folder, written from `rust/specs/FMT_CRES_UI.md`.
//!
//! - [`UiStrings`]: localized text of `Game/CRes.dll` (string-table resources) by numeric id, with the
//!   `FormatMessage` insert grammar; cursor resources enumerated.
//! - [`LayoutTable`]: the layout table (base constants, per-screen sections with fallback to screen 0,
//!   system font records, HUD set with fallback to set 0, lights, interface FX, popup boxes).
//! - [`ScreenPages`]: the positional front-end widget layout (20 pages, 10 widget kinds).
//! - [`SystemFonts`] / [`FontSpec`] / [`FontResolver`]: font requests and the font files of the archives.
//! - [`HudKeys`] / [`HudView`]: the keys of the in-match HUD elements and their values in the HUD layout set.
//! - [`UiScreen`] / [`PageTag`]: the client's screen and page tables by index; [`VirtualScaler`] maps the
//!   1024 x 768 virtual coordinates to a surface.
//!
//! ROM model: no string, name or value of the client is in this crate. Table names, section names, keys
//! and tags are read from the user's own `CShell.dll` by rva and accepted only when they hash to the pinned
//! digest ([`UiNames`]); the tables are read and decrypted through `dc-rez`.

mod error;
pub mod activate;
pub mod fonts;
pub mod hud;
pub mod layout;
pub mod names;
pub mod pages;
pub mod pe_res;
pub mod scale;
pub mod screens;
pub mod strings;

#[cfg(test)]
mod rom_tests;

pub use error::UiDataError;
pub use fonts::{FontResolver, FontSpec, ResolvedFont, SystemFonts};
pub use hud::{HudKey, HudKeys, HudLiteral, HudSection, HudView};
pub use layout::{FontRecord, FxRecord, LayoutTable, LightRecord, ScreenKey, SectionView, Value};
pub use names::{UiNames, UiSection};
pub use pages::{IssueKind, LineIssue, Page, ScreenPages, Widget, WidgetKind, WidgetRecord};
pub use scale::{VirtualScaler, VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
pub use screens::{PageTag, UiScreen};
pub use strings::{FmtArg, UiStrings};

use dc_rez::RezVfs;
use std::path::{Path, PathBuf};

/// Finds `Game/<name>` of the client folder with an ASCII case-insensitive match (archive names in the
/// table paths do not always match the file's case on disk).
pub fn find_game_file(rom: &Path, name: &str) -> Option<PathBuf> {
    let dir = rom.join("Game");
    let exact = dir.join(name);
    if exact.is_file() {
        return Some(exact);
    }
    std::fs::read_dir(&dir).ok()?.flatten().find_map(|e| {
        let n = e.file_name();
        n.to_str()
            .filter(|s| s.eq_ignore_ascii_case(name))
            .map(|_| e.path())
    })
}

/// Decrypted bytes of the table at `path` (`folder\entry`, folder = archive stem). The text tables are the
/// encrypted entries; `RezVfs` decrypts them with the key read from the user's binary.
pub(crate) fn read_table_bytes(
    vfs: &RezVfs,
    path: &str,
    what: &'static str,
) -> Result<Vec<u8>, UiDataError> {
    let (rez, entry) = UiNames::split_table_path(path).ok_or(UiDataError::MissingEntry { what })?;
    let rez_name = find_game_file(vfs.rom_dir(), &rez)
        .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .unwrap_or(rez);
    let mut bytes = vfs
        .entry(&rez_name, &entry)?
        .ok_or(UiDataError::MissingEntry { what })?;
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    Ok(bytes)
}

/// Like [`read_table_bytes`], decoded as Latin-1.
pub(crate) fn read_table_text(
    vfs: &RezVfs,
    path: &str,
    what: &'static str,
) -> Result<String, UiDataError> {
    Ok(read_table_bytes(vfs, path, what)?
        .iter()
        .map(|&b| b as char)
        .collect())
}

/// Everything at once, in the client's load order (strings, layout table, screen pages, fonts).
/// SPEC: FMT_CRES_UI.md §5 ("Rust load order")
#[derive(Debug)]
pub struct UiData {
    pub strings: UiStrings,
    pub layout: LayoutTable,
    pub pages: ScreenPages,
    pub fonts: SystemFonts,
}

impl UiData {
    /// Loads the UI data of the client folder `rom`.
    pub fn load(rom: &Path) -> Result<Self, UiDataError> {
        let strings = UiStrings::from_rom(rom)?;
        let names = UiNames::from_rom(rom)?;
        let vfs = RezVfs::new(rom);
        let layout = LayoutTable::load(&vfs, names.clone())?;
        let pages = ScreenPages::load(&vfs, &names)?;
        let fonts = SystemFonts::from_layout(&layout);
        Ok(UiData {
            strings,
            layout,
            pages,
            fonts,
        })
    }
}
