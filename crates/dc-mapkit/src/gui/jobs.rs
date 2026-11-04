//! Worker threads of the editor: a build after every change (debounced by the app), export, the software
//! renders, play, the ROM texture listing with thumbnails and the ROM texture index. Each job is one thread that
//! sends one [`Done`] back and asks egui to repaint.
// DC-DEFAULT: MAP-62 (the scene is rebuilt through build::build on a worker thread on every change)

use crate::build::{self, Built};
use crate::diag::Diagnostics;
use crate::export::{self, ExportOptions, Exported};
use crate::play::{self, PlayOptions, PlayReport};
use crate::project::{write_atomic, Heights, Project, BUILD_DIR};
use crate::render_sw::{render_report, RenderOptions, Scene, View};
use crate::romlist::{self, RomEntry, RomKind, RomQuery};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

/// A finished job.
pub enum Done {
    Built {
        result: Result<Built, Diagnostics>,
        ms: u128,
        written: Option<PathBuf>,
        /// The document revision the build started from.
        rev: u64,
    },
    Exported(Result<Exported, Diagnostics>),
    Rendered(Result<Vec<PathBuf>, Diagnostics>),
    /// Boxed: the report is several times the size of the other results (clippy `large_enum_variant`).
    Played(Box<Result<PlayReport, Diagnostics>>),
    RomList(Result<Vec<RomEntry>, Diagnostics>),
    Index(Result<Arc<dc_render::TextureIndex>, String>),
}

pub struct Jobs {
    tx: Sender<Done>,
    rx: Receiver<Done>,
    repaint: egui::Context,
}

impl Jobs {
    pub fn new(repaint: egui::Context) -> Jobs {
        let (tx, rx) = channel();
        Jobs { tx, rx, repaint }
    }

    /// Everything that finished since the last poll.
    pub fn poll(&self) -> Vec<Done> {
        self.rx.try_iter().collect()
    }

    fn run(&self, f: impl FnOnce() -> Done + Send + 'static) {
        let tx = self.tx.clone();
        let repaint = self.repaint.clone();
        std::thread::spawn(move || {
            let done = f();
            let _ = tx.send(done);
            repaint.request_repaint();
        });
    }

    /// Build the world; `write` also writes `build/<key>.dat` (the Build button; the automatic rebuilds do not).
    pub fn build(&self, dir: PathBuf, project: Project, heights: Heights, write: bool, rev: u64) {
        self.run(move || {
            let t = Instant::now();
            let result = build::build(
                &project,
                &heights,
                &build::Assets::new(&dir),
                &project.build,
            );
            let mut written = None;
            if write {
                if let Ok(b) = &result {
                    let out = dir.join(BUILD_DIR).join(format!("{}.dat", project.key));
                    if write_atomic(&out, &b.dat).is_ok() {
                        written = Some(out);
                    }
                }
            }
            Done::Built {
                result,
                ms: t.elapsed().as_millis(),
                written,
                rev,
            }
        });
    }

    pub fn export(&self, dir: PathBuf, project: Project, heights: Heights, zip: bool) {
        self.run(move || {
            let built = match build::build(
                &project,
                &heights,
                &build::Assets::new(&dir),
                &project.build,
            ) {
                Ok(b) => b,
                Err(d) => return Done::Exported(Err(d)),
            };
            if built.diagnostics.has_errors() {
                return Done::Exported(Err(built.diagnostics));
            }
            let opts = ExportOptions {
                zip,
                no_rom_paths: false,
                project_dir: Some(dir.clone()),
                force: false,
            };
            Done::Exported(export::export(&project, &built, &dir.join(BUILD_DIR), opts))
        });
    }

    /// The `top` and `iso` software renders to `build/render/`.
    pub fn render(&self, dir: PathBuf, project: Project, heights: Heights) {
        self.run(move || {
            let level = match build::level_of(&project, &heights, &build::Assets::new(&dir)) {
                Ok(l) => l,
                Err(d) => return Done::Rendered(Err(d)),
            };
            let opts = RenderOptions {
                labels: true,
                ..Default::default()
            };
            let mut scene = Scene::from_level(&level, &opts);
            scene.add_project_objects(&project);
            let mut out = Vec::new();
            for view in [View::Top, View::Iso] {
                let r = match render_report(&scene, view.clone(), (1024, 768), &opts) {
                    Ok(r) => r,
                    Err(d) => return Done::Rendered(Err(d)),
                };
                let path = dir
                    .join(BUILD_DIR)
                    .join("render")
                    .join(format!("{}.png", view.slug()));
                if let Err(e) = write_atomic(&path, &r.image.png()) {
                    return Done::Rendered(Err(Diagnostics::one(crate::diag::Diagnostic::new(
                        crate::diag::Code::ProjectIo,
                        path.display().to_string(),
                        e.to_string(),
                    ))));
                }
                out.push(path);
            }
            Done::Rendered(Ok(out))
        });
    }

    pub fn play(&self, dir: PathBuf, rom: Option<PathBuf>) {
        self.run(move || {
            Done::Played(Box::new(play::play(
                &dir,
                &PlayOptions {
                    rom,
                    binaries: None,
                    headless: false,
                    seconds: None,
                    keep_profile: false,
                    modded: false,
                },
            )))
        });
    }

    /// The ROM's textures with thumbnails under `thumbs` (decoded lazily into that cache).
    pub fn rom_list(&self, rom: PathBuf, thumbs: Option<PathBuf>) {
        self.run(move || {
            Done::RomList(romlist::list(
                &rom,
                &RomQuery {
                    kind: RomKind::Textures,
                    filter: None,
                    thumbs,
                },
            ))
        });
    }

    pub fn open_index(&self, rom: PathBuf) {
        self.run(move || {
            Done::Index(
                dc_render::TextureIndex::open(&rom)
                    .map(Arc::new)
                    .map_err(|e| e.to_string()),
            )
        });
    }
}
