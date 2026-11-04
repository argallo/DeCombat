//! The menu's textures decoded ahead on a worker thread, so a screen's first frame (and a button's first hover
//! image) finds them in the catalogue's cache instead of reading and decoding them from the ROM archives on the
//! frame that needs them.
//!
//! The names come from the ROM layout pages of the menu's screens (panels, button state images, caption images,
//! check images, tiled backgrounds) and from the shell's code literals ([`super::catalog::CodePaths`]). The worker
//! opens its own ROM texture source; the menu takes what arrived once per frame ([`Prefetch::drain`]). A texture the
//! menu needed before it arrived was already loaded on the spot and is kept: the prefetched copy is then dropped,
//! so the overlay atlas never sees the same name twice. Pictures do not change: the same source decodes the same
//! texture either way.
// DC-DEFAULT: REN-51 (proposed) menu textures decoded ahead on a worker thread at menu start

use super::catalog::Catalog;
use super::pages;
use super::sheet::{Look, Sheet, Tiles};
use crate::hud::compose::{HudAssets, RomTextureSource, Tex, TextureSource};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};

/// Textures decoded on a worker thread.
pub struct Prefetch {
    rx: Receiver<(String, Option<Tex>)>,
    /// Names asked for, and textures taken into the cache so far (logs, tests).
    pub asked: usize,
    pub taken: usize,
    pub done: bool,
}

impl Prefetch {
    /// Starts decoding `names` from the ROM at `rom` (`None` when there is nothing to do or no thread).
    pub fn start(rom: &Path, names: Vec<String>) -> Option<Prefetch> {
        if names.is_empty() {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let rom = rom.to_path_buf();
        let asked = names.len();
        std::thread::Builder::new()
            .name("dc-client menu textures".into())
            .spawn(move || {
                let Ok(mut src) = RomTextureSource::open(&rom) else {
                    return;
                };
                for n in names {
                    let t = src.load(&n);
                    if tx.send((n, t)).is_err() {
                        return;
                    }
                }
            })
            .ok()?;
        Some(Prefetch {
            rx,
            asked,
            taken: 0,
            done: false,
        })
    }

    /// Puts the textures that arrived into `assets` (names it already holds stay as they are). Returns how many
    /// were taken.
    pub fn drain(&mut self, assets: &mut HudAssets) -> usize {
        let mut n = 0;
        loop {
            match self.rx.try_recv() {
                Ok((name, Some(t))) => {
                    if assets.generated(&name).is_none() {
                        assets.put(&name, t);
                        n += 1;
                    }
                }
                Ok((_, None)) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.done = true;
                    break;
                }
            }
        }
        self.taken += n;
        n
    }
}

/// The pages the menu's screens build their sheets from (the ROM holds more: screens the menu does not have).
pub const SCREEN_PAGES: [dc_ui_data::PageTag; 10] = [
    pages::COMMON,
    pages::LOBBY_COMMON,
    pages::LOGIN,
    pages::SERVERS,
    pages::CHANNELS,
    pages::ROOM_CREATE,
    pages::WAITING,
    pages::RESULT,
    pages::ROOM_LIST,
    pages::CHAT,
];

/// Every texture name the menu's screen pages and code literals can draw, without duplicates, in a stable order.
pub fn menu_textures(cat: &Catalog) -> Vec<String> {
    let mut out = BTreeSet::new();
    let tiles = |t: &Tiles, out: &mut BTreeSet<String>| {
        for r in 0..t.rows {
            for c in 0..t.cols {
                out.insert(t.name(r, c));
            }
        }
    };
    for tag in SCREEN_PAGES {
        let sheet = Sheet::build(&cat.pages, &[tag], cat);
        if let Some(t) = &sheet.tiles {
            tiles(t, &mut out);
        }
        for p in &sheet.parts {
            match &p.look {
                Look::Panel { tex, .. } => {
                    out.insert(tex.clone());
                }
                Look::Button { prefix, frame, .. } => {
                    if !prefix.is_empty() {
                        for l in ["A", "B", "C", "D"] {
                            out.insert(format!("{prefix}{l}.dtx"));
                        }
                    }
                    if let Some((t, _)) = frame {
                        out.insert(t.clone());
                    }
                }
                Look::Check { on, off } => {
                    out.insert(on.clone());
                    out.insert(off.clone());
                }
                _ => {}
            }
        }
    }
    let p = &cat.paths;
    // the code's tiled backgrounds are 3 rows of 4 (the tiler calls of the login, server select and result screens)
    for prefix in [&p.login_back, &p.server_back, &p.result_back]
        .into_iter()
        .flatten()
    {
        if let Some(t) = Tiles::new([0, 0], 3, 4, prefix) {
            tiles(&t, &mut out);
        }
    }
    if let Some(prefix) = &p.group_button {
        for l in ["A", "B", "C", "D"] {
            out.insert(format!("{prefix}{l}.dtx"));
        }
    }
    let singles = p.state_images.iter().chain(p.popup_buttons.iter()).chain([
        &p.state_ok,
        &p.state_no,
        &p.popup,
        &p.warning,
        &p.ok_icon,
    ]);
    for s in singles.flatten() {
        out.insert(s.clone());
    }
    out.remove("");
    out.into_iter().collect()
}
