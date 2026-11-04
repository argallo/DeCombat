//! The asset browser tab: the project's own PNG textures with thumbnails, and with a ROM the ROM's textures
//! (listed with thumbnails on a worker thread into the profile cache, `romlist::list` with `thumbs`), with a
//! filter box. A click answers the texture to assign.
// DC-DEFAULT: MAP-65 (asset browser and thumbnail cache)

use crate::project::{TextureRef, ASSETS_DIR};
use crate::render_sw::RgbaImage;
use crate::romlist::RomEntry;
use crate::thumbs::THUMB_SIZE;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Thumbnails decoded per frame at most (keeps scrolling smooth).
const LOADS_PER_FRAME: usize = 6;
const ROW: f32 = 72.0;

enum RomState {
    None,
    Loading,
    Ready(Vec<RomEntry>),
    Failed(String),
}

struct OwnTexture {
    rel: String,
    handle: Option<Result<egui::TextureHandle, String>>,
}

pub struct AssetBrowser {
    pub filter: String,
    own: Vec<OwnTexture>,
    rom: RomState,
    rom_handles: HashMap<String, Result<egui::TextureHandle, String>>,
}

/// A PNG file as an egui image of at most [`THUMB_SIZE`] pixels on its longer side.
pub fn thumb_image(path: &Path) -> Result<egui::ColorImage, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let full = RgbaImage {
        width: w,
        height: h,
        data: rgba.into_raw(),
    };
    let small = crate::thumbs::fit(&full, THUMB_SIZE);
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [small.width as usize, small.height as usize],
        &small.data,
    ))
}

fn walk_pngs(dir: &Path, prefix: &str, out: &mut Vec<String>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if p.is_dir() {
            if depth < 4 {
                walk_pngs(&p, &format!("{prefix}{name}/"), out, depth + 1);
            }
        } else if name.to_ascii_lowercase().ends_with(".png") {
            out.push(format!("{prefix}{name}"));
        }
    }
}

impl Default for AssetBrowser {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetBrowser {
    pub fn new() -> AssetBrowser {
        AssetBrowser {
            filter: String::new(),
            own: Vec::new(),
            rom: RomState::None,
            rom_handles: HashMap::new(),
        }
    }

    /// The project's own PNGs under `assets/` (recursive, sorted), project-relative with forward slashes.
    pub fn own_textures(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        walk_pngs(
            &dir.join(ASSETS_DIR),
            &format!("{ASSETS_DIR}/"),
            &mut out,
            0,
        );
        out
    }

    pub fn scan_own(&mut self, dir: &Path) {
        let rels = Self::own_textures(dir);
        let mut old: HashMap<String, Option<Result<egui::TextureHandle, String>>> =
            self.own.drain(..).map(|t| (t.rel, t.handle)).collect();
        self.own = rels
            .into_iter()
            .map(|rel| OwnTexture {
                handle: old.remove(&rel).flatten(),
                rel,
            })
            .collect();
    }

    pub fn rom_loading(&mut self) {
        self.rom = RomState::Loading;
    }

    pub fn rom_cleared(&mut self) {
        self.rom = RomState::None;
        self.rom_handles.clear();
    }

    pub fn set_rom_rows(&mut self, rows: Vec<RomEntry>) {
        self.rom = RomState::Ready(rows);
        self.rom_handles.clear();
    }

    pub fn rom_failed(&mut self, msg: String) {
        self.rom = RomState::Failed(msg);
    }

    pub fn rom_is_listed(&self) -> bool {
        matches!(self.rom, RomState::Ready(_) | RomState::Loading)
    }

    fn thumb_button(
        ui: &mut egui::Ui,
        handle: Option<&Result<egui::TextureHandle, String>>,
    ) -> egui::Response {
        let size = egui::vec2(THUMB_SIZE as f32, THUMB_SIZE as f32);
        match handle {
            Some(Ok(h)) => ui.add(egui::ImageButton::new(
                egui::Image::new(h).fit_to_exact_size(size),
            )),
            Some(Err(e)) => ui
                .add_sized(size, egui::Button::new("?"))
                .on_hover_text(e.clone()),
            None => ui.add_sized(size, egui::Button::new("...")),
        }
    }

    /// Draw the browser; answers the texture clicked, and sets `want_rom` when the user asks for a ROM.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        project_dir: Option<&Path>,
        rom: Option<&Path>,
        want_rom: &mut bool,
    ) -> Option<TextureRef> {
        let mut picked = None;
        let mut loads = 0usize;
        ui.horizontal(|ui| {
            ui.label("filter");
            ui.text_edit_singleline(&mut self.filter);
        });
        let filter = self.filter.trim().to_ascii_lowercase();
        ui.separator();
        ui.strong("own textures");
        ui.small("PNG files under the project's assets/ folder");
        let own_dir = project_dir.map(Path::to_path_buf);
        let own_rows: Vec<usize> = (0..self.own.len())
            .filter(|i| {
                filter.is_empty() || self.own[*i].rel.to_ascii_lowercase().contains(&filter)
            })
            .collect();
        if own_rows.is_empty() {
            ui.small("none yet: put PNGs under assets/ and reopen the project");
        }
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical()
            .id_salt("own textures")
            .max_height(ROW * 3.5)
            .show(ui, |ui| {
                for i in own_rows {
                    if self.own[i].handle.is_none() && loads < LOADS_PER_FRAME {
                        if let Some(d) = &own_dir {
                            let path = crate::project::Project::resolve(d, &self.own[i].rel);
                            let h = thumb_image(&path).map(|img| {
                                ctx.load_texture(
                                    format!("own:{}", self.own[i].rel),
                                    img,
                                    egui::TextureOptions::LINEAR,
                                )
                            });
                            self.own[i].handle = Some(h);
                            loads += 1;
                        }
                    }
                    ui.horizontal(|ui| {
                        let r = Self::thumb_button(ui, self.own[i].handle.as_ref());
                        if r.clicked() {
                            picked = Some(TextureRef::Own(self.own[i].rel.clone()));
                        }
                        ui.label(&self.own[i].rel);
                    });
                }
            });
        ui.separator();
        ui.strong("rom textures");
        match &self.rom {
            RomState::None => {
                if rom.is_none() {
                    ui.small("choose a ROM folder to list its textures");
                    if ui.button("Choose ROM folder").clicked() {
                        *want_rom = true;
                    }
                } else {
                    ui.small("listing...");
                }
            }
            RomState::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.small(
                        "decoding thumbnails into the profile cache (first time takes a while)",
                    );
                });
            }
            RomState::Failed(e) => {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
            RomState::Ready(rows) => {
                let shown: Vec<usize> = (0..rows.len())
                    .filter(|i| {
                        filter.is_empty() || rows[*i].path.to_ascii_lowercase().contains(&filter)
                    })
                    .collect();
                ui.small(format!(
                    "{} of {} (click to assign)",
                    shown.len(),
                    rows.len()
                ));
                let mut new_handles: Vec<(String, Result<egui::TextureHandle, String>)> =
                    Vec::new();
                egui::ScrollArea::vertical()
                    .id_salt("rom textures")
                    .show_rows(ui, ROW, shown.len(), |ui, range| {
                        for k in range {
                            let row = &rows[shown[k]];
                            if !self.rom_handles.contains_key(&row.path)
                                && loads < LOADS_PER_FRAME
                                && !new_handles.iter().any(|(p, _)| *p == row.path)
                            {
                                let h = match &row.thumb {
                                    Some(t) => thumb_image(Path::new(t)).map(|img| {
                                        ctx.load_texture(
                                            format!("rom:{}", row.path),
                                            img,
                                            egui::TextureOptions::LINEAR,
                                        )
                                    }),
                                    None => Err("no thumbnail".into()),
                                };
                                new_handles.push((row.path.clone(), h));
                                loads += 1;
                            }
                            ui.horizontal(|ui| {
                                let handle = self.rom_handles.get(&row.path).or_else(|| {
                                    new_handles
                                        .iter()
                                        .find(|(p, _)| *p == row.path)
                                        .map(|(_, h)| h)
                                });
                                let r = Self::thumb_button(ui, handle);
                                if r.clicked() {
                                    picked = Some(TextureRef::Rom(row.path.clone()));
                                }
                                ui.vertical(|ui| {
                                    ui.label(&row.path);
                                    if let (Some(w), Some(h)) = (row.width, row.height) {
                                        ui.small(format!("{w} x {h}"));
                                    }
                                });
                            });
                        }
                    });
                for (p, h) in new_handles {
                    self.rom_handles.insert(p, h);
                }
            }
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_textures_are_listed_recursively_and_sorted() {
        let base = std::env::temp_dir().join(format!("mapkit-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("assets/sub")).unwrap();
        std::fs::write(base.join("assets/b.png"), b"x").unwrap();
        std::fs::write(base.join("assets/A.PNG"), b"x").unwrap();
        std::fs::write(base.join("assets/sub/c.png"), b"x").unwrap();
        std::fs::write(base.join("assets/not.txt"), b"x").unwrap();
        let rels = AssetBrowser::own_textures(&base);
        assert_eq!(
            rels,
            vec!["assets/A.PNG", "assets/b.png", "assets/sub/c.png"]
        );
        assert!(AssetBrowser::own_textures(&base.join("nothing")).is_empty());
        // a real PNG becomes a thumbnail of at most THUMB_SIZE on the longer side
        let png = crate::template::textures::png("ground", 1);
        std::fs::write(base.join("assets/real.png"), &png).unwrap();
        let img = thumb_image(&base.join("assets/real.png")).unwrap();
        assert!(img.size[0] <= THUMB_SIZE as usize && img.size[1] <= THUMB_SIZE as usize);
        assert!(thumb_image(&base.join("assets/b.png")).is_err());
        let mut b = AssetBrowser::new();
        b.scan_own(&base);
        assert_eq!(b.own.len(), 4);
        let _ = std::fs::remove_dir_all(&base);
    }
}
