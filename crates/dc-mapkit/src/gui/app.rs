//! The editor window: welcome screen, the viewport with its tools, the inspector / asset browser, the problems
//! panel and the status line. Every edit is a [`Gesture`] turned into one op batch at the gesture's end and
//! applied through `ops::apply` with a history snapshot (MAP-61); the world is rebuilt on a worker thread after
//! every change and uploaded into the viewport (MAP-62).
// DC-DEFAULT: MAP-61, MAP-62, MAP-63, MAP-64, MAP-65

use super::assets::AssetBrowser;
use super::camera::{self, FlyCamera, V};
use super::commands::{self, AssignTarget, BrushTool, Gesture};
use super::jobs::{Done, Jobs};
use super::model::{self, Selection};
use super::pick;
use super::viewport::{TextureSource, Viewport};
use super::welcome;
use crate::build::{self, BuildStats};
use crate::diag::{Diagnostics, Severity};
use crate::geom::Poly;
use crate::history;
use crate::ops::{self, Op, Preset, SolidSpec};
use crate::project::{BrushRole, Heights, Project, PropValue, Vec3f};
use crate::romlist::{self, RomSource};
use crate::template::{self, Template};
use crate::vocab_names as v;
use dc_world::PropKind;
use egui::{Color32, Key, PointerButton, Pos2, Sense, Stroke};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long after the last change the automatic rebuild starts.
const BUILD_DEBOUNCE: Duration = Duration::from_millis(400);
/// Pixel radius of a marker click.
const MARKER_PICK_PX: f32 = 12.0;
/// Pixels of pointer travel between two brush dabs.
const DAB_SPACING_PX: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Place,
    Solid,
    Terrain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SolidChoice {
    Box,
    Wedge,
    Cylinder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RightTab {
    Inspector,
    Assets,
}

/// The open project.
struct Doc {
    dir: PathBuf,
    project: Project,
    heights: Heights,
    selection: Option<Selection>,
    /// The level polygons for picking (terrain, solids, imports), refreshed after every change.
    level_polys: Vec<Poly>,
    /// Grows with every committed change; builds carry the revision they started from.
    rev: u64,
    undo: Vec<u32>,
    redo: Vec<u32>,
    /// `validate` after every change.
    problems: Diagnostics,
    /// The last build's diagnostics (world-level checks).
    world_problems: Diagnostics,
}

enum GestKind {
    DragObject {
        id: String,
        plane_y: f32,
        offset: V,
    },
    DragSolid {
        id: String,
        plane_y: f32,
        offset: V,
    },
    Brush {
        dabs: Vec<[f32; 2]>,
        flatten_height: f32,
        last_px: [f32; 2],
    },
}

/// A gesture in progress: the state before it (restored before the batch is committed) and what it does.
struct Gest {
    base_project: Project,
    base_heights: Heights,
    kind: GestKind,
}

struct Welcome {
    name: String,
    author: String,
    template: Template,
    folder: String,
    folder_edited: bool,
    error: Option<String>,
}

struct Status {
    build: Option<BuildStats>,
    build_ms: Option<u128>,
    last_build: Option<Instant>,
    building: bool,
    message: String,
}

pub struct Editor {
    render_state: egui_wgpu::RenderState,
    viewport: Viewport,
    jobs: Jobs,
    doc: Option<Doc>,
    welcome: Welcome,
    rom: Option<(PathBuf, RomSource)>,
    rom_index: Option<Arc<dc_render::TextureIndex>>,
    rom_index_pending: bool,
    camera: FlyCamera,
    tool: Tool,
    place_class: String,
    solid_choice: SolidChoice,
    solid_size: Vec3f,
    solid_brush: Option<BrushRole>,
    brush_tool: BrushTool,
    brush_radius: f32,
    brush_strength: f32,
    snap_ground: bool,
    right_tab: RightTab,
    assets: AssetBrowser,
    gesture: Option<Gest>,
    dirty_at: Option<Instant>,
    status: Status,
    log: Vec<String>,
    log_open: bool,
    export_zip: bool,
    /// Text fields being edited (field key -> buffer), committed when they lose focus.
    text_edits: HashMap<String, String>,
    /// A terrain generation form.
    gen: GenForm,
    frame_once: bool,
    smoke: Option<Smoke>,
    /// Where the next window screenshot (egui's viewport screenshot event) is written.
    window_shot: Option<PathBuf>,
}

/// The scripted run of `dc-mapkit-gui --smoke <out dir> <project>`: waits for the first build, places a spawn,
/// waits for the rebuild, exports, calls play, writes the viewport read-back and closes. It drives the same
/// actions the buttons do, so a developer can check the editor end to end on a machine without clicking.
struct Smoke {
    out: PathBuf,
    step: u32,
    started: Instant,
}

struct GenForm {
    preset: Preset,
    amplitude: f32,
    base: f32,
    scale: f32,
    octaves: u32,
}

/// Something the UI asked for this frame, applied after the panels are drawn.
enum Action {
    New,
    Open(PathBuf),
    ChooseRom,
    Close,
    Build,
    Validate,
    Export,
    Render,
    Play,
    Undo,
    Redo,
    Save,
    Commit(Gesture),
    Select(Option<Selection>),
    Frame,
    /// Both screenshots into this folder (`None` = `build/render/`).
    Screenshot(Option<PathBuf>),
}

fn colour(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

impl Editor {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        dir: Option<PathBuf>,
        gpu: dc_render::GpuContext,
    ) -> Editor {
        cc.egui_ctx.set_fonts(super::font::definitions());
        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("eframe runs on wgpu (MAPKIT_PLAN.md D7)");
        let viewport = Viewport::new(gpu, &render_state);
        let jobs = Jobs::new(cc.egui_ctx.clone());
        let rom = romlist::discover_rom(None);
        let docs = welcome::documents_dir().unwrap_or_else(|| PathBuf::from("."));
        let mut e = Editor {
            render_state,
            viewport,
            jobs,
            doc: None,
            welcome: Welcome {
                name: "My Map".into(),
                author: String::new(),
                template: Template::Starter,
                folder: welcome::default_folder(&docs, "My Map")
                    .display()
                    .to_string(),
                folder_edited: false,
                error: None,
            },
            rom: None,
            rom_index: None,
            rom_index_pending: false,
            camera: FlyCamera::default(),
            tool: Tool::Select,
            place_class: v::CLASS_PLAYER_SPAWN.into(),
            solid_choice: SolidChoice::Box,
            solid_size: [128.0, 128.0, 128.0],
            solid_brush: None,
            brush_tool: BrushTool::Raise,
            brush_radius: 300.0,
            brush_strength: 24.0,
            snap_ground: true,
            right_tab: RightTab::Inspector,
            assets: AssetBrowser::new(),
            gesture: None,
            dirty_at: None,
            status: Status {
                build: None,
                build_ms: None,
                last_build: None,
                building: false,
                message: "welcome".into(),
            },
            log: Vec::new(),
            log_open: false,
            export_zip: true,
            text_edits: HashMap::new(),
            gen: GenForm {
                preset: Preset::Valley,
                amplitude: 128.0,
                base: 0.0,
                scale: 1024.0,
                octaves: 5,
            },
            frame_once: true,
            smoke: None,
            window_shot: None,
        };
        if let Some(r) = rom {
            e.set_rom(r);
        }
        if let Some(d) = dir {
            if let Err(msg) = e.open(&d) {
                e.welcome.error = Some(msg);
            }
        }
        e
    }

    /// Run the scripted smoke sequence and write its files under `out` (see [`Smoke`]).
    pub fn with_smoke(mut self, out: PathBuf) -> Editor {
        self.smoke = Some(Smoke {
            out,
            step: 0,
            started: Instant::now(),
        });
        self
    }

    fn smoke_step(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(s) = self.smoke.as_mut() else {
            return;
        };
        let elapsed = s.started.elapsed();
        let built = self.viewport.has_world() && !self.status.building && self.dirty_at.is_none();
        let note = |log: &mut Vec<String>, t: &str| log.push(format!("smoke: {t}"));
        match s.step {
            0 if built => {
                note(&mut self.log, "first build done, placing a spawn");
                actions.push(Action::Commit(Gesture::Place {
                    class: v::CLASS_PLAYER_SPAWN.into(),
                    at: [0.0, 0.0, 0.0],
                }));
                s.step = 1;
            }
            1 if built && elapsed > Duration::from_secs(2) => {
                note(&mut self.log, "rebuilt with the spawn, exporting");
                actions.push(Action::Export);
                s.step = 2;
            }
            2 if self
                .log
                .iter()
                .any(|l| l.starts_with("exported") || l.contains("export failed")) =>
            {
                note(&mut self.log, "export answered, calling play");
                actions.push(Action::Play);
                s.step = 3;
            }
            3 if self.log.iter().any(|l| l.starts_with("play:"))
                && self.status.message == "play finished" =>
            {
                note(&mut self.log, "play answered, taking the screenshots");
                actions.push(Action::Screenshot(Some(s.out.clone())));
                s.step = 4;
                s.started = Instant::now();
            }
            4 if elapsed > Duration::from_secs(3) || s.out.join("editor.png").is_file() => {
                let text = self.log.join("\n");
                let _ = std::fs::write(s.out.join("smoke.log"), text);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                s.step = 5;
            }
            _ if elapsed > Duration::from_secs(120) && s.step < 5 => {
                note(&mut self.log, &format!("timed out at step {}", s.step));
                let _ = std::fs::create_dir_all(&s.out);
                let _ = std::fs::write(s.out.join("smoke.log"), self.log.join("\n"));
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                s.step = 5;
            }
            _ => {}
        }
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    // ----------------------------------------------------------------------------------------------------------
    // project

    fn open(&mut self, dir: &Path) -> Result<(), String> {
        let (project, heights) = Project::open(dir).map_err(|d| diag_text(&d))?;
        let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        let mut doc = Doc {
            dir: dir.clone(),
            project,
            heights,
            selection: None,
            level_polys: Vec::new(),
            rev: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            problems: Diagnostics::new(),
            world_problems: Diagnostics::new(),
        };
        refresh_doc(&mut doc);
        self.assets.scan_own(&dir);
        self.doc = Some(doc);
        self.viewport.clear_world();
        self.status = Status {
            build: None,
            build_ms: None,
            last_build: None,
            building: false,
            message: format!("opened {}", dir.display()),
        };
        self.dirty_at = Some(Instant::now() - BUILD_DEBOUNCE);
        self.frame_once = true;
        self.text_edits.clear();
        Ok(())
    }

    fn new_project(&mut self) -> Result<(), String> {
        let dir = PathBuf::from(self.welcome.folder.trim());
        if self.welcome.folder.trim().is_empty() {
            return Err("choose a folder".into());
        }
        if !welcome::folder_is_free(&dir) {
            return Err(format!("{} is not empty", dir.display()));
        }
        let name = self.welcome.name.trim();
        let name = if name.is_empty() { "Untitled" } else { name };
        let made = template::make(self.welcome.template, name, self.welcome.author.trim(), 1);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        crate::cli::write_project(&dir, &made, self.welcome.template).map_err(|d| diag_text(&d))?;
        self.open(&dir)
    }

    fn set_rom(&mut self, rom: (PathBuf, RomSource)) {
        self.rom = Some(rom.clone());
        self.rom_index = None;
        self.rom_index_pending = true;
        self.jobs.open_index(rom.0.clone());
        self.assets.rom_loading();
        let thumbs = romlist::profile_dir().map(|p| p.join("mapkit").join("thumbs"));
        self.jobs.rom_list(rom.0, thumbs);
    }

    fn choose_rom(&mut self) {
        let mut dlg = rfd::FileDialog::new()
            .set_title("Choose the Combat Arms ROM folder (Game/ + Engine.exe)");
        if let Some((r, _)) = &self.rom {
            dlg = dlg.set_directory(r);
        }
        if let Some(dir) = dlg.pick_folder() {
            if !romlist::looks_like_rom(&dir) {
                self.status.message = format!(
                    "{} has no Game/ folder with Engine.exe; using it anyway",
                    dir.display()
                );
            }
            if let Some(profile) = romlist::profile_dir() {
                let _ = std::fs::create_dir_all(&profile);
                match romlist::save_profile_rom(&profile, &dir) {
                    Ok(f) => self.log.push(format!("ROM remembered in {}", f.display())),
                    Err(e) => self.log.push(format!("could not remember the ROM: {e}")),
                }
            }
            self.set_rom((dir, RomSource::Profile));
            self.dirty_at = Some(Instant::now());
        }
    }

    fn textures(&self) -> Option<TextureSource> {
        self.doc
            .as_ref()
            .map(|d| TextureSource::new(d.dir.clone(), self.rom_index.clone()))
    }

    // ----------------------------------------------------------------------------------------------------------
    // changes

    /// Apply a gesture's batch to the project on disk: snapshot, apply, save, refresh.
    fn commit(&mut self, g: Gesture) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        if let Some(gest) = self.gesture.take() {
            doc.project = gest.base_project;
            doc.heights = gest.base_heights;
        }
        let ops = commands::ops_of(&g, &doc.project);
        if ops.is_empty() {
            return;
        }
        let label = commands::label_of(&g);
        match ops::apply(&mut doc.project, &mut doc.heights, &ops) {
            Ok(applied) => {
                if applied.changed.is_empty() {
                    self.status.message = format!("{label}: nothing changed");
                    return;
                }
                match history::snapshot(&doc.dir, &label) {
                    Ok(e) => {
                        doc.undo.push(e.n);
                        doc.redo.clear();
                    }
                    Err(d) => self.log.push(format!("snapshot failed: {}", diag_text(&d))),
                }
                if let Err(d) = doc.project.save_all(&doc.dir, &doc.heights) {
                    self.log.push(format!("save failed: {}", diag_text(&d)));
                }
                for id in applied.applied.iter().flat_map(|a| a.ids.iter()) {
                    if let Gesture::Place { .. } = g {
                        doc.selection = Some(Selection::Object(id.clone()));
                    } else if let Gesture::PlaceSolid { .. } = g {
                        doc.selection = Some(Selection::Solid {
                            id: id.clone(),
                            face: None,
                        });
                    }
                }
                if let Gesture::Delete { .. } = g {
                    doc.selection = None;
                }
                self.status.message = format!(
                    "{label} ({} op{})",
                    ops.len(),
                    if ops.len() == 1 { "" } else { "s" }
                );
                for d in applied.diagnostics.iter() {
                    self.log.push(d.to_string());
                }
                refresh_doc(doc);
                doc.rev += 1;
                self.dirty_at = Some(Instant::now());
                if let Gesture::PutTexture { .. } = g {
                    self.assets.scan_own(&doc.dir);
                }
            }
            Err(d) => {
                self.status.message = format!("{label} refused: {}", diag_text(&d));
                self.log.push(format!("{label} refused: {}", diag_text(&d)));
            }
        }
    }

    /// Reload the project after a history revert.
    fn reload(&mut self) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        match Project::open(&doc.dir) {
            Ok((p, h)) => {
                doc.project = p;
                doc.heights = h;
                if let Some(sel) = &doc.selection {
                    if !doc.project.has_id(sel.id()) {
                        doc.selection = None;
                    }
                }
                refresh_doc(doc);
                doc.rev += 1;
                self.dirty_at = Some(Instant::now());
            }
            Err(d) => self.log.push(format!("reload failed: {}", diag_text(&d))),
        }
    }

    fn undo(&mut self) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let Some(n) = doc.undo.pop() else {
            self.status.message = "nothing to undo".into();
            return;
        };
        let before = history::list(&doc.dir)
            .ok()
            .and_then(|l| l.last().map(|e| e.n))
            .unwrap_or(0);
        match history::revert(&doc.dir, n) {
            Ok(_) => {
                doc.redo.push(before + 1);
                self.status.message = format!("undo (history {n})");
                self.reload();
            }
            Err(d) => {
                doc.undo.clear();
                self.status.message = format!("undo failed: {}", diag_text(&d));
            }
        }
    }

    fn redo(&mut self) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let Some(n) = doc.redo.pop() else {
            self.status.message = "nothing to redo".into();
            return;
        };
        let before = history::list(&doc.dir)
            .ok()
            .and_then(|l| l.last().map(|e| e.n))
            .unwrap_or(0);
        match history::revert(&doc.dir, n) {
            Ok(_) => {
                doc.undo.push(before + 1);
                self.status.message = format!("redo (history {n})");
                self.reload();
            }
            Err(d) => {
                doc.redo.clear();
                self.status.message = format!("redo failed: {}", diag_text(&d));
            }
        }
    }

    fn save(&mut self) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        // every gesture is already on disk; Save takes a labelled checkpoint in the shared history
        match history::snapshot(&doc.dir, "save") {
            Ok(e) => {
                doc.undo.push(e.n);
                self.status.message = format!("saved (history {})", e.n);
            }
            Err(d) => self.status.message = format!("save failed: {}", diag_text(&d)),
        }
    }

    fn start_build(&mut self, write: bool) {
        let Some(doc) = &self.doc else {
            return;
        };
        self.status.building = true;
        self.dirty_at = None;
        self.jobs.build(
            doc.dir.clone(),
            doc.project.clone(),
            doc.heights.clone(),
            write,
            doc.rev,
        );
    }

    fn handle_done(&mut self, done: Done) {
        match done {
            Done::Built {
                result,
                ms,
                written,
                rev: _,
            } => {
                self.status.building = false;
                self.status.build_ms = Some(ms);
                self.status.last_build = Some(Instant::now());
                match result {
                    Ok(b) => {
                        self.status.build = Some(b.stats.clone());
                        if let Some(doc) = self.doc.as_mut() {
                            doc.world_problems = b.diagnostics;
                        }
                        if let Some(mut tex) = self.textures() {
                            match self.viewport.set_world(&b.dat, &mut tex) {
                                Ok(_) => {
                                    if self.frame_once {
                                        self.frame_once = false;
                                        self.frame_world();
                                    }
                                }
                                Err(e) => self.log.push(format!("viewport: {e}")),
                            }
                        }
                        if let Some(w) = written {
                            self.log.push(format!("built {}", w.display()));
                            self.status.message = format!("built {}", w.display());
                        }
                    }
                    Err(d) => {
                        if let Some(doc) = self.doc.as_mut() {
                            doc.world_problems = d;
                        }
                        self.status.message = "build refused: see problems".into();
                    }
                }
            }
            Done::Exported(r) => match r {
                Ok(e) => {
                    self.log.push(format!("exported {}", e.dir.display()));
                    if let Some(z) = &e.zip {
                        self.log.push(format!("zip {}", z.display()));
                    }
                    for d in e.diagnostics.iter() {
                        self.log.push(d.to_string());
                    }
                    self.status.message = format!("exported {}", e.dir.display());
                }
                Err(d) => {
                    for x in d.iter() {
                        self.log.push(x.to_string());
                    }
                    self.status.message = "export failed: see output".into();
                    self.log_open = true;
                }
            },
            Done::Rendered(r) => match r {
                Ok(files) => {
                    for f in &files {
                        self.log.push(format!("rendered {}", f.display()));
                    }
                    self.status.message = format!("rendered {} pictures", files.len());
                }
                Err(d) => {
                    self.log.push(format!("render failed: {}", diag_text(&d)));
                    self.status.message = "render failed".into();
                }
            },
            Done::Played(r) => {
                match *r {
                    Ok(rep) => {
                        self.log
                            .push(format!("play: lobby on port {}", rep.lobby_port));
                        for l in rep.logs {
                            self.log.push(format!("play log {}", l.display()));
                        }
                    }
                    Err(d) => {
                        for x in d.iter() {
                            self.log.push(format!("play: {x}"));
                        }
                    }
                }
                self.status.message = "play finished".into();
                self.log_open = true;
            }
            Done::RomList(r) => match r {
                Ok(rows) => {
                    self.log
                        .push(format!("ROM: {} textures listed", rows.len()));
                    self.assets.set_rom_rows(rows);
                }
                Err(d) => self.assets.rom_failed(diag_text(&d)),
            },
            Done::Index(r) => {
                self.rom_index_pending = false;
                match r {
                    Ok(i) => {
                        self.rom_index = Some(i);
                        self.dirty_at = Some(Instant::now());
                    }
                    Err(e) => self.log.push(format!("ROM textures: {e}")),
                }
            }
        }
    }

    fn frame_world(&mut self) {
        if let Some(doc) = &self.doc {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &doc.level_polys {
                for c in &p.corners {
                    for (a, v) in [c.x, c.y, c.z].iter().enumerate() {
                        lo[a] = lo[a].min(*v);
                        hi[a] = hi[a].max(*v);
                    }
                }
            }
            if lo[0] <= hi[0] {
                // the sky lid is far above the floor: frame the lower half
                hi[1] = lo[1] + (hi[1] - lo[1]) * 0.5;
                self.camera.frame_box(lo, hi);
            }
        }
    }

    // ----------------------------------------------------------------------------------------------------------
    // viewport

    fn viewport_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let (w, h) = (rect.width().max(1.0), rect.height().max(1.0));
        let ctx = ui.ctx().clone();
        let text_focus = ctx.memory(|m| m.focused().is_some());
        let looking = resp.dragged_by(PointerButton::Secondary);
        if looking {
            let d = ctx.input(|i| i.pointer.delta());
            self.camera.look(d.x, d.y);
        }
        if (resp.hovered() || looking) && !text_focus {
            let (fwd, right, up, scroll, dt) = ctx.input(|i| {
                let k = |a: Key, b: Key| (i.key_down(a) as i32 - i.key_down(b) as i32) as f32;
                (
                    k(Key::W, Key::S),
                    k(Key::D, Key::A),
                    k(Key::E, Key::Q),
                    i.raw_scroll_delta.y,
                    i.stable_dt.min(0.1),
                )
            });
            if fwd != 0.0 || right != 0.0 || up != 0.0 {
                self.camera.fly(fwd, right, up, dt);
                ctx.request_repaint();
            }
            if scroll != 0.0 {
                self.camera.scale_speed(scroll / 40.0);
            }
            if ctx.input(|i| i.key_pressed(Key::F)) {
                actions.push(Action::Frame);
            }
        }
        let local = |p: Pos2| [p.x - rect.min.x, p.y - rect.min.y];
        self.viewport
            .draw(ui, rect, &self.camera, &self.render_state);
        // markers and outlines over the frame
        let painter = ui.painter_at(rect);
        let mut marker_px: Vec<([f32; 2], String)> = Vec::new();
        if let Some(doc) = &self.doc {
            for m in model::markers(&doc.project) {
                if let Some((px, depth)) = self.camera.project(m.pos, w, h) {
                    let at = Pos2::new(rect.min.x + px[0], rect.min.y + px[1]);
                    let r = (900.0 / depth.max(1.0)).clamp(3.0, 9.0);
                    let selected = doc.selection.as_ref().is_some_and(|s| s.id() == m.id);
                    painter.circle_filled(at, r, colour(m.colour));
                    painter.circle_stroke(at, r, Stroke::new(1.0_f32, Color32::BLACK));
                    if selected {
                        painter.circle_stroke(at, r + 4.0, Stroke::new(2.0_f32, Color32::WHITE));
                        painter.text(
                            at + egui::vec2(r + 8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &m.id,
                            egui::FontId::proportional(13.0),
                            Color32::WHITE,
                        );
                    }
                    marker_px.push((px, m.id));
                }
            }
            if let Some(Selection::Solid { id, .. }) = &doc.selection {
                if let Some(s) = doc.project.solid(id) {
                    if let Some((lo, hi)) = pick::solid_box(s) {
                        for (a, b) in pick::box_edges(lo, hi) {
                            if let (Some((pa, _)), Some((pb, _))) =
                                (self.camera.project(a, w, h), self.camera.project(b, w, h))
                            {
                                painter.line_segment(
                                    [
                                        Pos2::new(rect.min.x + pa[0], rect.min.y + pa[1]),
                                        Pos2::new(rect.min.x + pb[0], rect.min.y + pb[1]),
                                    ],
                                    Stroke::new(1.5_f32, Color32::from_rgb(255, 230, 80)),
                                );
                            }
                        }
                    }
                }
            }
        }
        // terrain brush cursor
        if self.tool == Tool::Terrain {
            if let (Some(pos), Some(doc)) = (resp.hover_pos(), &self.doc) {
                let p = local(pos);
                let ray = self.camera.unproject(p[0], p[1], w, h);
                if let Some(hit) = pick::pick_polys(&doc.level_polys, &ray) {
                    let n = 32;
                    let mut pts = Vec::with_capacity(n + 1);
                    for i in 0..=n {
                        let a = i as f32 / n as f32 * std::f32::consts::TAU;
                        let q = [
                            hit.point[0] + self.brush_radius * a.cos(),
                            hit.point[1],
                            hit.point[2] + self.brush_radius * a.sin(),
                        ];
                        if let Some((px, _)) = self.camera.project(q, w, h) {
                            pts.push(Pos2::new(rect.min.x + px[0], rect.min.y + px[1]));
                        }
                    }
                    if pts.len() > 2 {
                        painter.add(egui::Shape::line(
                            pts,
                            Stroke::new(1.5_f32, Color32::from_rgb(120, 255, 160)),
                        ));
                    }
                }
            }
        }
        // a hint
        painter.text(
            rect.left_bottom() + egui::vec2(8.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            format!(
                "right-drag look, WASD/QE fly, scroll speed ({:.0}/s), F frame, Delete removes",
                self.camera.speed
            ),
            egui::FontId::proportional(12.0),
            Color32::from_rgba_unmultiplied(255, 255, 255, 170),
        );

        // ---- gestures
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let pointer = resp.interact_pointer_pos().map(local);
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = pointer {
                let ray = self.camera.unproject(p[0], p[1], w, h);
                match self.tool {
                    Tool::Select => {
                        let pxs: Vec<[f32; 2]> = marker_px.iter().map(|(px, _)| *px).collect();
                        if let Some(i) = pick::pick_marker(&pxs, p, MARKER_PICK_PX) {
                            actions.push(Action::Select(Some(Selection::Object(
                                marker_px[i].1.clone(),
                            ))));
                        } else if let Some(hit) = pick::pick_solids(&doc.project.solids, &ray) {
                            actions.push(Action::Select(Some(Selection::Solid {
                                id: hit.id,
                                face: Some(hit.face),
                            })));
                        } else {
                            actions.push(Action::Select(None));
                        }
                    }
                    Tool::Place => {
                        if let Some(hit) = pick::pick_polys(&doc.level_polys, &ray) {
                            actions.push(Action::Commit(Gesture::Place {
                                class: self.place_class.clone(),
                                at: hit.point,
                            }));
                        }
                    }
                    Tool::Solid => {
                        if let Some(hit) = pick::pick_polys(&doc.level_polys, &ray) {
                            let kind = match self.solid_choice {
                                SolidChoice::Box => SolidSpec::Box,
                                SolidChoice::Wedge => SolidSpec::Wedge,
                                SolidChoice::Cylinder => SolidSpec::Cylinder { sides: 8 },
                            };
                            let material = doc
                                .project
                                .materials
                                .iter()
                                .find(|m| !m.flags.sky)
                                .map(|m| m.id.clone())
                                .unwrap_or_else(|| "block".into());
                            actions.push(Action::Commit(Gesture::PlaceSolid {
                                kind,
                                at: hit.point,
                                size: self.solid_size,
                                material,
                                brush: self.solid_brush,
                            }));
                        }
                    }
                    Tool::Terrain => {
                        if let Some(hit) = pick::pick_polys(&doc.level_polys, &ray) {
                            actions.push(Action::Commit(Gesture::Brush {
                                tool: self.brush_tool,
                                dabs: vec![[hit.point[0], hit.point[2]]],
                                radius: self.brush_radius,
                                strength: self.brush_strength,
                                flatten_height: hit.point[1],
                            }));
                        }
                    }
                }
            }
        }
        if resp.drag_started_by(PointerButton::Primary) {
            if let Some(p) = pointer {
                let ray = self.camera.unproject(p[0], p[1], w, h);
                let base = (doc.project.clone(), doc.heights.clone());
                match self.tool {
                    Tool::Select => {
                        let pxs: Vec<[f32; 2]> = marker_px.iter().map(|(px, _)| *px).collect();
                        if let Some(i) = pick::pick_marker(&pxs, p, MARKER_PICK_PX) {
                            let id = marker_px[i].1.clone();
                            if let Some(o) = doc.project.object(&id) {
                                let plane_y = o.pos[1];
                                let hit = ray.hit_plane_y(plane_y).unwrap_or(o.pos);
                                doc.selection = Some(Selection::Object(id.clone()));
                                self.gesture = Some(Gest {
                                    base_project: base.0,
                                    base_heights: base.1,
                                    kind: GestKind::DragObject {
                                        id,
                                        plane_y,
                                        offset: camera::sub(o.pos, hit),
                                    },
                                });
                            }
                        } else if let Some(hit) = pick::pick_solids(&doc.project.solids, &ray) {
                            if let Some(s) = doc.project.solid(&hit.id) {
                                doc.selection = Some(Selection::Solid {
                                    id: hit.id.clone(),
                                    face: Some(hit.face.clone()),
                                });
                                self.gesture = Some(Gest {
                                    base_project: base.0,
                                    base_heights: base.1,
                                    kind: GestKind::DragSolid {
                                        id: hit.id.clone(),
                                        plane_y: hit.point[1],
                                        offset: camera::sub(s.pos, hit.point),
                                    },
                                });
                            }
                        }
                    }
                    Tool::Terrain => {
                        if let Some(hit) = pick::pick_polys(&doc.level_polys, &ray) {
                            let dab = [hit.point[0], hit.point[2]];
                            self.gesture = Some(Gest {
                                base_project: base.0,
                                base_heights: base.1,
                                kind: GestKind::Brush {
                                    dabs: vec![dab],
                                    flatten_height: hit.point[1],
                                    last_px: p,
                                },
                            });
                            preview_dab(
                                doc,
                                self.brush_tool,
                                dab,
                                self.brush_radius,
                                self.brush_strength,
                                hit.point[1],
                            );
                            self.dirty_at = Some(Instant::now());
                        }
                    }
                    _ => {}
                }
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let (Some(p), Some(gest)) = (pointer, self.gesture.as_mut()) {
                let ray = self.camera.unproject(p[0], p[1], w, h);
                match &mut gest.kind {
                    GestKind::DragObject {
                        id,
                        plane_y,
                        offset,
                    } => {
                        if let Some(hit) = ray.hit_plane_y(*plane_y) {
                            let to = camera::add(hit, *offset);
                            doc.project = gest.base_project.clone();
                            if let Some(o) = doc.project.object_mut(id) {
                                o.pos = to;
                            }
                        }
                    }
                    GestKind::DragSolid {
                        id,
                        plane_y,
                        offset,
                    } => {
                        if let Some(hit) = ray.hit_plane_y(*plane_y) {
                            let to = camera::add(hit, *offset);
                            doc.project = gest.base_project.clone();
                            if let Some(s) = doc.project.solid_mut(id) {
                                s.pos = to;
                            }
                            self.dirty_at = Some(Instant::now());
                        }
                    }
                    GestKind::Brush {
                        dabs,
                        flatten_height,
                        last_px,
                    } => {
                        let moved =
                            ((p[0] - last_px[0]).powi(2) + (p[1] - last_px[1]).powi(2)).sqrt();
                        if moved >= DAB_SPACING_PX {
                            if let Some(hit) = pick::pick_polys(&doc.level_polys, &ray) {
                                let dab = [hit.point[0], hit.point[2]];
                                dabs.push(dab);
                                *last_px = p;
                                preview_dab(
                                    doc,
                                    self.brush_tool,
                                    dab,
                                    self.brush_radius,
                                    self.brush_strength,
                                    *flatten_height,
                                );
                                self.dirty_at = Some(Instant::now());
                            }
                        }
                    }
                }
                ctx.request_repaint();
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            if let Some(gest) = &self.gesture {
                let g = match &gest.kind {
                    GestKind::DragObject { id, .. } => {
                        doc.project.object(id).map(|o| Gesture::MoveObject {
                            id: id.clone(),
                            to: o.pos,
                            ground: self.snap_ground,
                        })
                    }
                    GestKind::DragSolid { id, .. } => {
                        doc.project.solid(id).map(|s| Gesture::MoveSolid {
                            id: id.clone(),
                            to: s.pos,
                        })
                    }
                    GestKind::Brush {
                        dabs,
                        flatten_height,
                        ..
                    } => Some(Gesture::Brush {
                        tool: self.brush_tool,
                        dabs: dabs.clone(),
                        radius: self.brush_radius,
                        strength: self.brush_strength,
                        flatten_height: *flatten_height,
                    }),
                };
                match g {
                    Some(g) => actions.push(Action::Commit(g)),
                    None => {
                        // nothing to commit: drop the preview
                        if let Some(gest) = self.gesture.take() {
                            doc.project = gest.base_project;
                            doc.heights = gest.base_heights;
                        }
                    }
                }
            }
        }
        if !text_focus {
            let del = ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace));
            if del {
                if let Some(sel) = &doc.selection {
                    actions.push(Action::Commit(Gesture::Delete {
                        id: sel.id().to_string(),
                    }));
                }
            }
            let (z, shift, cmd) = ctx.input(|i| {
                (
                    i.key_pressed(Key::Z),
                    i.modifiers.shift,
                    i.modifiers.command,
                )
            });
            if z && cmd {
                actions.push(if shift { Action::Redo } else { Action::Undo });
            }
        }
    }

    // ----------------------------------------------------------------------------------------------------------
    // panels

    fn top_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("DeCombat map kit");
            ui.separator();
            if ui
                .button("Close")
                .on_hover_text("back to the welcome screen")
                .clicked()
            {
                actions.push(Action::Close);
            }
            ui.separator();
            let can = self.gesture.is_none();
            if ui.add_enabled(can, egui::Button::new("Undo")).clicked() {
                actions.push(Action::Undo);
            }
            if ui.add_enabled(can, egui::Button::new("Redo")).clicked() {
                actions.push(Action::Redo);
            }
            if ui
                .button("Save")
                .on_hover_text(
                    "every edit is already on disk; this takes a labelled history checkpoint",
                )
                .clicked()
            {
                actions.push(Action::Save);
            }
            ui.separator();
            if ui
                .add_enabled(!self.status.building, egui::Button::new("Build"))
                .on_hover_text("build/<key>.dat plus the world-level checks")
                .clicked()
            {
                actions.push(Action::Build);
            }
            if ui.button("Validate").clicked() {
                actions.push(Action::Validate);
            }
            if ui
                .button("Export")
                .on_hover_text("the mod folder under build/mod/ (and the zip the lobby serves)")
                .clicked()
            {
                actions.push(Action::Export);
            }
            ui.checkbox(&mut self.export_zip, "zip");
            if ui
                .button("Render")
                .on_hover_text("top and iso pictures to build/render/")
                .clicked()
            {
                actions.push(Action::Render);
            }
            if ui
                .button("Play")
                .on_hover_text("Vanilla++ on this machine: lobby with the map on its featured entry + game server + client")
                .clicked()
            {
                actions.push(Action::Play);
            }
            if ui
                .button("Screenshot")
                .on_hover_text(
                    "the viewport frame to build/render/viewport.png and the window to editor.png",
                )
                .clicked()
            {
                actions.push(Action::Screenshot(None));
            }
            ui.separator();
            ui.checkbox(&mut self.log_open, "Output");
        });
    }

    fn tools_panel(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.heading("Tools");
        ui.selectable_value(&mut self.tool, Tool::Select, "Select / move")
            .on_hover_text("click a marker or a solid; drag to move it on its plane");
        ui.selectable_value(&mut self.tool, Tool::Place, "Place object")
            .on_hover_text("click the floor to place one");
        ui.selectable_value(&mut self.tool, Tool::Solid, "Solid")
            .on_hover_text("click the floor to drop a solid standing on it");
        ui.selectable_value(&mut self.tool, Tool::Terrain, "Terrain brush")
            .on_hover_text("click or drag over the terrain");
        ui.separator();
        match self.tool {
            Tool::Select => {
                ui.checkbox(&mut self.snap_ground, "snap moved objects to the floor");
                if ui.button("Frame the map (F)").clicked() {
                    actions.push(Action::Frame);
                }
            }
            Tool::Place => {
                ui.label("class");
                egui::ComboBox::from_id_salt("place class")
                    .selected_text(&self.place_class)
                    .show_ui(ui, |ui| {
                        for c in v::POINT_CLASSES {
                            ui.selectable_value(&mut self.place_class, c.to_string(), *c);
                        }
                    });
                if let Some(spec) = dc_world::vocab::class_named(&self.place_class) {
                    ui.small(spec.consumer);
                }
            }
            Tool::Solid => {
                ui.selectable_value(&mut self.solid_choice, SolidChoice::Box, "box");
                ui.selectable_value(&mut self.solid_choice, SolidChoice::Wedge, "wedge");
                ui.selectable_value(
                    &mut self.solid_choice,
                    SolidChoice::Cylinder,
                    "cylinder (8 sides)",
                );
                ui.label("size");
                ui.horizontal(|ui| {
                    for x in self.solid_size.iter_mut() {
                        ui.add(egui::DragValue::new(x).speed(4.0).range(4.0..=8192.0));
                    }
                });
                ui.label("role");
                egui::ComboBox::from_id_salt("solid role")
                    .selected_text(role_name(self.solid_brush))
                    .show_ui(ui, |ui| {
                        for r in ROLES {
                            ui.selectable_value(&mut self.solid_brush, *r, role_name(*r));
                        }
                    });
            }
            Tool::Terrain => {
                for t in BrushTool::ALL {
                    ui.selectable_value(&mut self.brush_tool, t, t.name());
                }
                ui.add(egui::Slider::new(&mut self.brush_radius, 32.0..=2048.0).text("radius"));
                ui.add(egui::Slider::new(&mut self.brush_strength, 1.0..=256.0).text("strength"));
                ui.small("flatten pulls towards the height under the first dab; smooth's strength = passes");
            }
        }
        ui.separator();
        if let Some(doc) = &self.doc {
            ui.small(format!(
                "{} objects, {} solids, {} materials",
                doc.project.objects.len(),
                doc.project.solids.len(),
                doc.project.materials.len()
            ));
            ui.small(format!(
                "history: {} undo / {} redo",
                doc.undo.len(),
                doc.redo.len()
            ));
        }
    }

    fn right_panel(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.right_tab, RightTab::Inspector, "Inspector");
            ui.selectable_value(&mut self.right_tab, RightTab::Assets, "Assets");
        });
        ui.separator();
        match self.right_tab {
            RightTab::Inspector => {
                egui::ScrollArea::vertical()
                    .id_salt("inspector")
                    .show(ui, |ui| self.inspector(ui, actions));
            }
            RightTab::Assets => {
                let mut want_rom = false;
                let dir = self.doc.as_ref().map(|d| d.dir.clone());
                let rom = self.rom.as_ref().map(|r| r.0.clone());
                let picked = self
                    .assets
                    .ui(ui, dir.as_deref(), rom.as_deref(), &mut want_rom);
                if want_rom {
                    actions.push(Action::ChooseRom);
                }
                if let Some(texture) = picked {
                    let target = match self.doc.as_ref().and_then(|d| d.selection.clone()) {
                        Some(Selection::Solid {
                            id,
                            face: Some(face),
                        }) => AssignTarget::Face { id, face },
                        Some(Selection::Solid { id, face: None }) => AssignTarget::Solid { id },
                        _ => AssignTarget::Terrain { layer: 0 },
                    };
                    actions.push(Action::Commit(Gesture::PutTexture { texture, target }));
                }
                ui.separator();
                ui.small(match self.doc.as_ref().and_then(|d| d.selection.as_ref()) {
                    Some(Selection::Solid { id, face: Some(f) }) => {
                        format!("a click assigns to face {f} of {id}")
                    }
                    Some(Selection::Solid { id, face: None }) => {
                        format!("a click assigns to every face of {id}")
                    }
                    _ => "a click assigns to the terrain (select a solid face to texture it)"
                        .to_string(),
                });
            }
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        match doc.selection.clone() {
            Some(Selection::Object(id)) => {
                if let Some(o) = doc.project.object(&id).cloned() {
                    ui.strong(format!("{} ({})", o.id, o.class));
                    if let Some(spec) = dc_world::vocab::class_named(&o.class) {
                        ui.small(spec.consumer);
                    }
                    if let Some(name) = text_field(
                        ui,
                        &mut self.text_edits,
                        &format!("{}.name", o.id),
                        "name",
                        &o.name,
                    ) {
                        actions.push(Action::Commit(Gesture::Rename {
                            id: o.id.clone(),
                            name,
                        }));
                    }
                    if let Some(pos) = vec3_field(ui, "pos", o.pos, 1.0) {
                        actions.push(Action::Commit(Gesture::SetProp {
                            id: o.id.clone(),
                            prop: "pos".into(),
                            value: PropValue::Vec3(pos),
                        }));
                    }
                    if let Some(rot) = vec3_field(ui, "rot (deg)", o.rot, 1.0) {
                        actions.push(Action::Commit(Gesture::SetProp {
                            id: o.id.clone(),
                            prop: "rot".into(),
                            value: PropValue::Vec3(rot),
                        }));
                    }
                    ui.separator();
                    for row in model::prop_rows(&o) {
                        let label = if row.required {
                            format!("{} *", row.name)
                        } else {
                            row.name.clone()
                        };
                        let key = format!("{}.{}", o.id, row.name);
                        let changed: Option<PropValue> = match (row.kind, row.value.clone()) {
                            (PropKind::Bool, val) => {
                                let mut b = matches!(val, Some(PropValue::Bool(true)));
                                ui.checkbox(&mut b, &label)
                                    .on_hover_text(&row.doc)
                                    .changed()
                                    .then_some(PropValue::Bool(b))
                            }
                            (PropKind::Real | PropKind::Int, val) => {
                                let n = val.and_then(|v| v.as_num()).unwrap_or(0.0);
                                num_field(
                                    ui,
                                    &label,
                                    n,
                                    if row.kind == PropKind::Int { 1.0 } else { 0.1 },
                                )
                                .map(PropValue::Num)
                            }
                            (PropKind::Vec3 | PropKind::Euler, val) => {
                                let x = val.and_then(|v| v.as_vec3()).unwrap_or([0.0; 3]);
                                vec3_field(ui, &label, x, 1.0).map(PropValue::Vec3)
                            }
                            (PropKind::Rgb, val) => {
                                let mut rgb = match val {
                                    Some(PropValue::Rgb { rgb }) => rgb,
                                    Some(PropValue::Vec3(v)) => {
                                        [v[0] as u8, v[1] as u8, v[2] as u8]
                                    }
                                    _ => [255, 255, 255],
                                };
                                let mut out = None;
                                ui.horizontal(|ui| {
                                    ui.label(&label).on_hover_text(&row.doc);
                                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                                        out = Some(PropValue::Rgb { rgb });
                                    }
                                });
                                out
                            }
                            (_, val) => {
                                let cur = val
                                    .and_then(|v| v.as_text().map(str::to_string))
                                    .unwrap_or_default();
                                if row.name == v::PROP_TEAM {
                                    let mut t = cur.clone();
                                    let mut out = None;
                                    ui.horizontal(|ui| {
                                        ui.label(&label).on_hover_text(&row.doc);
                                        for (val, text) in
                                            [("", "any"), ("1", "team 1"), ("2", "team 2")]
                                        {
                                            if ui
                                                .selectable_value(&mut t, val.to_string(), text)
                                                .clicked()
                                                && t != cur
                                            {
                                                out = Some(PropValue::Text(t.clone()));
                                            }
                                        }
                                    });
                                    out
                                } else {
                                    text_field(ui, &mut self.text_edits, &key, &label, &cur)
                                        .map(PropValue::Text)
                                }
                            }
                        };
                        if let Some(value) = changed {
                            actions.push(Action::Commit(Gesture::SetProp {
                                id: o.id.clone(),
                                prop: row.name.clone(),
                                value,
                            }));
                        }
                    }
                    ui.separator();
                    if ui.button("Delete").clicked() {
                        actions.push(Action::Commit(Gesture::Delete { id: o.id.clone() }));
                    }
                } else {
                    ui.label("(gone)");
                }
            }
            Some(Selection::Solid { id, face }) => {
                if let Some(s) = doc.project.solid(&id).cloned() {
                    ui.strong(format!("{} ({})", s.id, solid_kind_name(&s.kind)));
                    if let Some(f) = &face {
                        ui.small(format!(
                            "face {f}: material {}",
                            crate::geom::face_material(&s, f)
                        ));
                    }
                    let mut edit = Gesture::SolidEdit {
                        id: s.id.clone(),
                        pos: None,
                        rot_y_deg: None,
                        size: None,
                        material: None,
                        brush: None,
                        group: None,
                    };
                    let mut any = false;
                    if let Some(p) = vec3_field(ui, "pos (centre)", s.pos, 1.0) {
                        if let Gesture::SolidEdit { pos, .. } = &mut edit {
                            *pos = Some(p);
                        }
                        any = true;
                    }
                    if let Some(sz) = vec3_field(ui, "size", s.size, 1.0) {
                        if let Gesture::SolidEdit { size, .. } = &mut edit {
                            *size = Some([sz[0].max(1.0), sz[1].max(1.0), sz[2].max(1.0)]);
                        }
                        any = true;
                    }
                    if let Some(r) = num_field(ui, "rot y (deg)", s.rot_y_deg as f64, 1.0) {
                        if let Gesture::SolidEdit { rot_y_deg, .. } = &mut edit {
                            *rot_y_deg = Some(r as f32);
                        }
                        any = true;
                    }
                    let mats: Vec<String> =
                        doc.project.materials.iter().map(|m| m.id.clone()).collect();
                    let mut mat = s.material.clone();
                    ui.horizontal(|ui| {
                        ui.label("material");
                        egui::ComboBox::from_id_salt("solid material")
                            .selected_text(&mat)
                            .show_ui(ui, |ui| {
                                for m in &mats {
                                    ui.selectable_value(&mut mat, m.clone(), m);
                                }
                            });
                    });
                    if mat != s.material {
                        if let Gesture::SolidEdit { material, .. } = &mut edit {
                            *material = Some(mat);
                        }
                        any = true;
                    }
                    let mut role = s.brush;
                    ui.horizontal(|ui| {
                        ui.label("role");
                        egui::ComboBox::from_id_salt("solid role edit")
                            .selected_text(role_name(role))
                            .show_ui(ui, |ui| {
                                for r in ROLES {
                                    ui.selectable_value(&mut role, *r, role_name(*r));
                                }
                            });
                    });
                    if role != s.brush {
                        if let Gesture::SolidEdit { brush, .. } = &mut edit {
                            *brush = Some(role);
                        }
                        any = true;
                    }
                    if let Some(g) = text_field(
                        ui,
                        &mut self.text_edits,
                        &format!("{}.group", s.id),
                        "group",
                        s.group.as_deref().unwrap_or(""),
                    ) {
                        if let Gesture::SolidEdit { group, .. } = &mut edit {
                            *group = Some((!g.trim().is_empty()).then(|| g.trim().to_string()));
                        }
                        any = true;
                    }
                    if !s.faces.is_empty() {
                        ui.label("faces with their own material:");
                        for (f, m) in &s.faces {
                            ui.small(format!("{f}: {m}"));
                        }
                    }
                    if any {
                        actions.push(Action::Commit(edit));
                    }
                    ui.separator();
                    if ui.button("Delete").clicked() {
                        actions.push(Action::Commit(Gesture::Delete { id: s.id.clone() }));
                    }
                } else {
                    ui.label("(gone)");
                }
            }
            None => {
                ui.small("nothing selected: click a marker or a solid in the viewport");
            }
        }
        ui.separator();
        egui::CollapsingHeader::new("Terrain")
            .default_open(false)
            .show(ui, |ui| {
                match &doc.project.terrain {
                    Some(t) => ui.small(format!(
                        "{} x {} cells of {} units, origin y {}",
                        t.cells_x, t.cells_z, t.cell_size, t.origin[1]
                    )),
                    None => ui.small("no terrain"),
                };
                ui.horizontal(|ui| {
                    ui.label("preset");
                    for (p, name) in [
                        (Preset::Hills, "hills"),
                        (Preset::Valley, "valley"),
                        (Preset::Ridge, "ridge"),
                        (Preset::Flat, "flat"),
                    ] {
                        ui.selectable_value(&mut self.gen.preset, p, name);
                    }
                });
                ui.add(egui::Slider::new(&mut self.gen.amplitude, 0.0..=1024.0).text("amplitude"));
                ui.add(egui::Slider::new(&mut self.gen.base, -512.0..=512.0).text("base"));
                ui.add(egui::Slider::new(&mut self.gen.scale, 128.0..=4096.0).text("scale"));
                ui.add(egui::Slider::new(&mut self.gen.octaves, 1..=8).text("octaves"));
                if ui
                    .button("Generate terrain")
                    .on_hover_text("replaces the heights (undo brings them back)")
                    .clicked()
                {
                    actions.push(Action::Commit(Gesture::TerrainGen {
                        preset: self.gen.preset,
                        amplitude: self.gen.amplitude,
                        base: self.gen.base,
                        scale: self.gen.scale,
                        octaves: self.gen.octaves,
                        seed: None,
                    }));
                }
            });
        egui::CollapsingHeader::new("World")
            .default_open(false)
            .show(ui, |ui| {
                let s = doc.project.settings.clone();
                let mut op = Op::SettingsSet {
                    bake_shadow: None,
                    bake_lightmap: None,
                    fog_enable: None,
                    fog_near: None,
                    fog_far: None,
                    fog_color: None,
                    far_z: None,
                    sun_dir: None,
                    sun_color: None,
                    sun_bright: None,
                    ambient: None,
                    sky_size: None,
                    sky_material: None,
                    two_sided: None,
                };
                let mut any = false;
                if let Op::SettingsSet {
                    fog_enable,
                    fog_near,
                    fog_far,
                    fog_color,
                    far_z,
                    sun_dir,
                    sun_color,
                    sun_bright,
                    ambient,
                    two_sided,
                    ..
                } = &mut op
                {
                    let mut fe = s.fog_enable;
                    if ui.checkbox(&mut fe, "fog").changed() {
                        *fog_enable = Some(fe);
                        any = true;
                    }
                    if let Some(x) = num_field(ui, "fog near", s.fog_near as f64, 10.0) {
                        *fog_near = Some(x as f32);
                        any = true;
                    }
                    if let Some(x) = num_field(ui, "fog far", s.fog_far as f64, 10.0) {
                        *fog_far = Some(x as f32);
                        any = true;
                    }
                    if let Some(c) = rgb_field(ui, "fog colour", s.fog_color) {
                        *fog_color = Some(c);
                        any = true;
                    }
                    if let Some(x) = num_field(ui, "far plane", s.far_z as f64, 100.0) {
                        *far_z = Some(x as f32);
                        any = true;
                    }
                    if let Some(d) = vec3_field(ui, "sun direction", s.sun_dir, 0.01) {
                        *sun_dir = Some(d);
                        any = true;
                    }
                    if let Some(c) = rgb_field(ui, "sun colour", s.sun_color) {
                        *sun_color = Some(c);
                        any = true;
                    }
                    if let Some(x) = num_field(ui, "sun brightness", s.sun_bright as f64, 0.05) {
                        *sun_bright = Some(x as f32);
                        any = true;
                    }
                    if let Some(c) = rgb_field(ui, "ambient", s.ambient) {
                        *ambient = Some(c);
                        any = true;
                    }
                    // MAP-118
                    let mut ts = s.two_sided;
                    if ui
                        .checkbox(&mut ts, "both faces")
                        .on_hover_text("draw the back of every surface too: turn it on for a map with ROM pieces")
                        .changed()
                    {
                        *two_sided = Some(ts);
                        any = true;
                    }
                }
                if any {
                    actions.push(Action::Commit(Gesture::Settings(Box::new(op))));
                }
            });
        egui::CollapsingHeader::new("Mod metadata")
            .default_open(false)
            .show(ui, |ui| {
                let m = doc.project.mod_meta.clone();
                ui.small(format!(
                    "mod id {} (set by `new`), key {}",
                    m.id, doc.project.key
                ));
                let mut op = Op::MetaSet {
                    name: None,
                    version: None,
                    author: None,
                    description: None,
                    tags: None,
                    homepage: None,
                    modes: None,
                    goals: None,
                    round_seconds: None,
                    players: None,
                };
                let mut any = false;
                if let Op::MetaSet {
                    name,
                    version,
                    author,
                    description,
                    homepage,
                    modes,
                    round_seconds,
                    ..
                } = &mut op
                {
                    if let Some(x) = text_field(
                        ui,
                        &mut self.text_edits,
                        "meta.name",
                        "map name",
                        &doc.project.name,
                    ) {
                        *name = Some(x);
                        any = true;
                    }
                    if let Some(x) =
                        text_field(ui, &mut self.text_edits, "meta.author", "author", &m.author)
                    {
                        *author = Some(x);
                        any = true;
                    }
                    if let Some(x) = text_field(
                        ui,
                        &mut self.text_edits,
                        "meta.version",
                        "version",
                        &m.version,
                    ) {
                        *version = Some(x);
                        any = true;
                    }
                    if let Some(x) = text_field(
                        ui,
                        &mut self.text_edits,
                        "meta.description",
                        "description",
                        &m.description,
                    ) {
                        *description = Some(x);
                        any = true;
                    }
                    if let Some(x) = text_field(
                        ui,
                        &mut self.text_edits,
                        "meta.homepage",
                        "homepage",
                        m.homepage.as_deref().unwrap_or(""),
                    ) {
                        *homepage = Some(x);
                        any = true;
                    }
                    ui.label("modes");
                    let mut list = m.modes.clone();
                    let mut mchanged = false;
                    ui.horizontal_wrapped(|ui| {
                        for mode in v::MODE_NAMES {
                            let mut on = list.iter().any(|x| x == mode);
                            if ui.checkbox(&mut on, *mode).changed() {
                                mchanged = true;
                                if on {
                                    list.push(mode.to_string());
                                } else {
                                    list.retain(|x| x != mode);
                                }
                            }
                        }
                    });
                    if mchanged {
                        *modes = Some(list);
                        any = true;
                    }
                    if let Some(x) = num_field(
                        ui,
                        "round seconds (0 = lobby default)",
                        m.round_seconds.unwrap_or(0) as f64,
                        10.0,
                    ) {
                        *round_seconds = Some(x.max(0.0) as u32);
                        any = true;
                    }
                }
                if any {
                    actions.push(Action::Commit(Gesture::Meta(Box::new(op))));
                }
            });
    }

    fn bottom_panel(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let rom_text = match &self.rom {
            Some((p, s)) => format!(
                "ROM {} ({s:?}){}",
                p.display(),
                if self.rom_index_pending {
                    ", indexing"
                } else {
                    ""
                }
            ),
            None => "no ROM (own textures only; Choose ROM folder for ROM textures and play)"
                .to_string(),
        };
        ui.horizontal_wrapped(|ui| {
            ui.small(&self.status.message);
            ui.separator();
            ui.small(rom_text);
            ui.separator();
            match (&self.status.build, self.status.build_ms) {
                (Some(b), Some(ms)) => ui.small(format!(
                    "build {} polys, {} nodes (depth {}), {} textures, {} bytes in {ms} ms{}",
                    b.polys,
                    b.nodes,
                    b.depth,
                    b.textures,
                    b.bytes,
                    self.status
                        .last_build
                        .map(|t| format!(", {} s ago", t.elapsed().as_secs()))
                        .unwrap_or_default()
                )),
                _ => ui.small("not built yet"),
            };
            if self.status.building {
                ui.spinner();
            }
            if let Some(st) = &self.viewport.stats {
                ui.small(format!("viewport {} triangles", st.triangles_drawn));
            }
        });
        ui.separator();
        let Some(doc) = &self.doc else {
            return;
        };
        let errors =
            doc.problems.count(Severity::Error) + doc.world_problems.count(Severity::Error);
        let warnings =
            doc.problems.count(Severity::Warning) + doc.world_problems.count(Severity::Warning);
        ui.strong(format!("Problems: {errors} errors, {warnings} warnings"));
        egui::ScrollArea::vertical()
            .id_salt("problems")
            .show(ui, |ui| {
                for d in doc.problems.iter().chain(doc.world_problems.iter()) {
                    let c = match d.severity {
                        Severity::Error => Color32::LIGHT_RED,
                        Severity::Warning => Color32::from_rgb(255, 210, 90),
                        _ => Color32::LIGHT_GRAY,
                    };
                    let r = ui.add(
                        egui::Label::new(egui::RichText::new(model::problem_line(d)).color(c))
                            .sense(Sense::click()),
                    );
                    if let Some(fix) = &d.fix {
                        r.clone().on_hover_text(fix);
                    }
                    if r.clicked() {
                        if let Some(sel) = model::selection_of_problem(&doc.project, d) {
                            actions.push(Action::Select(Some(sel)));
                        }
                    }
                }
            });
    }

    fn welcome_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.heading("DeCombat map kit");
            ui.small("the community map builder: a map is a folder you can also edit with `dc-mapkit`, scripts and agents");
        });
        ui.add_space(20.0);
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            ui.strong("New map");
            ui.horizontal(|ui| {
                ui.label("name");
                if ui.text_edit_singleline(&mut self.welcome.name).changed() && !self.welcome.folder_edited {
                    let docs = welcome::documents_dir().unwrap_or_else(|| PathBuf::from("."));
                    self.welcome.folder = welcome::default_folder(&docs, &self.welcome.name).display().to_string();
                }
            });
            ui.horizontal(|ui| {
                ui.label("author");
                ui.text_edit_singleline(&mut self.welcome.author);
            });
            ui.horizontal(|ui| {
                ui.label("template");
                for t in Template::ALL {
                    ui.selectable_value(&mut self.welcome.template, t, t.name());
                }
            });
            ui.small(match self.welcome.template {
                Template::Flat => "a blank canvas: flat floor, walls, sky, every spawn",
                Template::Arena => "flat plus bases and cover: playable in one step",
                Template::Starter => "the arena on valley terrain: the showcase",
            });
            ui.horizontal(|ui| {
                ui.label("folder");
                if ui.text_edit_singleline(&mut self.welcome.folder).changed() {
                    self.welcome.folder_edited = true;
                }
                if ui.button("...").clicked() {
                    if let Some(d) = rfd::FileDialog::new().set_title("Folder for the new map").pick_folder() {
                        self.welcome.folder = d.join(welcome::folder_name(&self.welcome.name)).display().to_string();
                        self.welcome.folder_edited = true;
                    }
                }
            });
            if ui.button("Create").clicked() {
                actions.push(Action::New);
            }
            ui.add_space(12.0);
            ui.strong("Open map");
            if ui.button("Open a map folder...").clicked() {
                if let Some(d) = rfd::FileDialog::new().set_title("Open a map folder (holds map.json)").pick_folder() {
                    actions.push(Action::Open(d));
                }
            }
            if let Some(e) = &self.welcome.error {
                ui.colored_label(Color32::LIGHT_RED, e);
            }
            let ui = &mut cols[1];
            ui.strong("ROM");
            match &self.rom {
                Some((p, s)) => {
                    ui.small(format!("{} (from {s:?})", p.display()));
                }
                None => {
                    ui.colored_label(
                        Color32::from_rgb(255, 210, 90),
                        "No ROM folder: the editor works with your own textures; ROM textures and Play need the Combat Arms folder.",
                    );
                }
            }
            if ui.button("Choose ROM folder...").clicked() {
                actions.push(Action::ChooseRom);
            }
            ui.add_space(12.0);
            ui.strong("Also");
            ui.small("`dc-mapkit new|info|apply|run|build|export|render|play` on the same folder; agents through `dc-mapkit mcp`.");
        });
    }

    fn apply_actions(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::New => {
                    if let Err(e) = self.new_project() {
                        self.welcome.error = Some(e);
                    }
                }
                Action::Open(d) => {
                    if let Err(e) = self.open(&d) {
                        self.welcome.error = Some(e);
                    }
                }
                Action::ChooseRom => self.choose_rom(),
                Action::Close => {
                    self.doc = None;
                    self.gesture = None;
                    self.viewport.clear_world();
                }
                Action::Build => self.start_build(true),
                Action::Validate => {
                    if let Some(doc) = self.doc.as_mut() {
                        refresh_doc(doc);
                        self.status.message = format!(
                            "validated: {} errors, {} warnings",
                            doc.problems.count(Severity::Error),
                            doc.problems.count(Severity::Warning)
                        );
                    }
                }
                Action::Export => {
                    if let Some(doc) = &self.doc {
                        self.status.message = "exporting...".into();
                        self.jobs.export(
                            doc.dir.clone(),
                            doc.project.clone(),
                            doc.heights.clone(),
                            self.export_zip,
                        );
                    }
                }
                Action::Render => {
                    if let Some(doc) = &self.doc {
                        self.status.message = "rendering...".into();
                        self.jobs
                            .render(doc.dir.clone(), doc.project.clone(), doc.heights.clone());
                    }
                }
                Action::Play => {
                    if let Some(doc) = &self.doc {
                        self.status.message =
                            "play: starting the lobby, the game server and the client...".into();
                        self.log.push(
                            "play: starting (the window closes when the client exits)".into(),
                        );
                        self.log_open = true;
                        self.jobs
                            .play(doc.dir.clone(), self.rom.as_ref().map(|r| r.0.clone()));
                    }
                }
                Action::Undo => self.undo(),
                Action::Redo => self.redo(),
                Action::Save => self.save(),
                Action::Commit(g) => self.commit(g),
                Action::Select(s) => {
                    if let Some(doc) = self.doc.as_mut() {
                        doc.selection = s;
                        self.text_edits.clear();
                    }
                }
                Action::Frame => self.frame_world(),
                Action::Screenshot(folder) => {
                    if let Some(doc) = &self.doc {
                        let folder = folder.unwrap_or_else(|| {
                            doc.dir.join(crate::project::BUILD_DIR).join("render")
                        });
                        let _ = std::fs::create_dir_all(&folder);
                        let out = folder.join("viewport.png");
                        match self.viewport.read_back() {
                            Ok(img) => match img.save_png(&out) {
                                Ok(()) => {
                                    self.log.push(format!("wrote {}", out.display()));
                                    self.status.message = format!("screenshot {}", out.display());
                                }
                                Err(e) => self.status.message = format!("screenshot failed: {e}"),
                            },
                            Err(e) => self.status.message = format!("screenshot failed: {e}"),
                        }
                        // the whole window arrives as an egui event next frame (saved in `update`)
                        self.window_shot = Some(folder.join("editor.png"));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                            egui::UserData::default(),
                        ));
                    }
                }
            }
        }
    }

    /// Save a window screenshot egui delivered.
    fn take_window_shots(&mut self, ctx: &egui::Context) {
        let shots: Vec<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
                .collect()
        });
        for img in shots {
            let Some(path) = self.window_shot.take() else {
                break;
            };
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            let r = image::save_buffer(
                &path,
                &rgba,
                img.size[0] as u32,
                img.size[1] as u32,
                image::ColorType::Rgba8,
            );
            match r {
                Ok(()) => self.log.push(format!("wrote {}", path.display())),
                Err(e) => self.log.push(format!("window screenshot failed: {e}")),
            }
        }
    }
}

impl eframe::App for Editor {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for done in self.jobs.poll() {
            self.handle_done(done);
        }
        self.take_window_shots(ctx);
        if let Some(t) = self.dirty_at {
            if self.doc.is_some() && !self.status.building && t.elapsed() >= BUILD_DEBOUNCE {
                self.start_build(false);
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        let mut actions = Vec::new();
        if self.doc.is_none() {
            egui::CentralPanel::default().show(ctx, |ui| self.welcome_ui(ui, &mut actions));
            self.apply_actions(ctx, actions);
            return;
        }
        self.smoke_step(ctx, &mut actions);
        egui::TopBottomPanel::top("top").show(ctx, |ui| self.top_bar(ui, &mut actions));
        egui::TopBottomPanel::bottom("bottom")
            .resizable(true)
            .default_height(150.0)
            .show(ctx, |ui| self.bottom_panel(ui, &mut actions));
        egui::SidePanel::left("tools")
            .resizable(true)
            .default_width(210.0)
            .show(ctx, |ui| self.tools_panel(ui, &mut actions));
        egui::SidePanel::right("right")
            .resizable(true)
            .default_width(340.0)
            .show(ctx, |ui| self.right_panel(ui, &mut actions));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| self.viewport_ui(ui, &mut actions));
        let mut open = self.log_open;
        egui::Window::new("Output")
            .open(&mut open)
            .default_size([520.0, 260.0])
            .show(ctx, |ui| {
                if ui.button("clear").clicked() {
                    self.log.clear();
                }
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in &self.log {
                            ui.monospace(l);
                        }
                    });
            });
        self.log_open = open;
        self.apply_actions(ctx, actions);
    }
}

// --------------------------------------------------------------------------------------------------------------
// helpers

const ROLES: &[Option<BrushRole>] = &[
    None,
    Some(BrushRole::WorldModel),
    Some(BrushRole::KillVolume),
    Some(BrushRole::Ladder),
    Some(BrushRole::Breakable),
    Some(BrushRole::Sky),
    Some(BrushRole::Blocker),
];

fn role_name(r: Option<BrushRole>) -> &'static str {
    match r {
        None => "level geometry",
        Some(BrushRole::WorldModel) => "world model",
        Some(BrushRole::KillVolume) => "kill volume",
        Some(BrushRole::Ladder) => "ladder",
        Some(BrushRole::Breakable) => "breakable",
        Some(BrushRole::Sky) => "sky model",
        Some(BrushRole::Blocker) => "invisible wall",
    }
}

fn solid_kind_name(k: &crate::project::SolidKind) -> &'static str {
    match k {
        crate::project::SolidKind::Box => "box",
        crate::project::SolidKind::Wedge => "wedge",
        crate::project::SolidKind::Cylinder { .. } => "cylinder",
        crate::project::SolidKind::Prism { .. } => "prism",
    }
}

fn diag_text(d: &Diagnostics) -> String {
    d.iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// `validate` and the picking polygons after a change.
fn refresh_doc(doc: &mut Doc) {
    doc.problems = crate::validate::validate(&doc.dir, &doc.project, &doc.heights);
    doc.level_polys = build::level_of(&doc.project, &doc.heights, &build::Assets::new(&doc.dir))
        .map(|l| l.polys)
        .unwrap_or_default();
}

/// Apply one brush dab to the live document (the preview of a stroke in progress).
fn preview_dab(
    doc: &mut Doc,
    tool: BrushTool,
    dab: [f32; 2],
    radius: f32,
    strength: f32,
    flatten_height: f32,
) {
    let g = Gesture::Brush {
        tool,
        dabs: vec![dab],
        radius,
        strength,
        flatten_height,
    };
    let ops = commands::ops_of(&g, &doc.project);
    let _ = ops::apply(&mut doc.project, &mut doc.heights, &ops);
}

/// A number field; answers the value when an edit ends (drag released or focus lost).
fn num_field(ui: &mut egui::Ui, label: &str, value: f64, speed: f64) -> Option<f64> {
    let mut v = value;
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label(label);
        let r = ui.add(egui::DragValue::new(&mut v).speed(speed));
        if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged() && !r.has_focus()))
            && v != value
        {
            out = Some(v);
        }
    });
    out
}

fn vec3_field(ui: &mut egui::Ui, label: &str, value: Vec3f, speed: f64) -> Option<Vec3f> {
    let mut v = value;
    let mut done = false;
    ui.horizontal(|ui| {
        ui.label(label);
        for x in v.iter_mut() {
            let r = ui.add(egui::DragValue::new(x).speed(speed));
            if r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged() && !r.has_focus())
            {
                done = true;
            }
        }
    });
    (done && v != value).then_some(v)
}

fn rgb_field(ui: &mut egui::Ui, label: &str, value: [u8; 3]) -> Option<[u8; 3]> {
    let mut c = value;
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.color_edit_button_srgb(&mut c).changed() && c != value {
            out = Some(c);
        }
    });
    out
}

/// A text field with its own buffer; answers the new text when the field loses focus with a change.
fn text_field(
    ui: &mut egui::Ui,
    buffers: &mut HashMap<String, String>,
    key: &str,
    label: &str,
    current: &str,
) -> Option<String> {
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label(label);
        let buf = buffers
            .entry(key.to_string())
            .or_insert_with(|| current.to_string());
        let r = ui.text_edit_singleline(buf);
        if r.lost_focus() {
            if buf.as_str() != current {
                out = Some(buf.clone());
            }
            buffers.remove(key);
        } else if !r.has_focus() && buf.as_str() != current {
            // the project changed under the field (undo, a script): show the new value
            *buf = current.to_string();
        }
    });
    out
}
