//! The window: the guns on the left, the gun in the middle, the camo on the right. Everything acts at once, there is no apply
//! button, and an agent's changes show up the same way.

use super::viewport::Viewport;
use crate::commands;
use crate::design::{Art, Design};
use crate::guns::View;
use crate::live::{self, Listener, Request};
use crate::pack::{self, SaveOptions};
use crate::patterns;
use crate::rom;
use crate::studio::{Orbit, Studio, BACKGROUNDS};
use dc_render::camo::{self, CamoFx, Circle};
use dc_render::GpuContext;
use egui::{Color32, RichText, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const ACCENT: Color32 = Color32::from_rgb(232, 134, 46);
const PANEL: Color32 = Color32::from_rgb(24, 25, 28);
const FAINT: Color32 = Color32::from_rgb(140, 143, 150);
const LEFT_WIDTH: f32 = 236.0;
const RIGHT_WIDTH: f32 = 316.0;
const UNDO_LIMIT: usize = 60;
/// Degrees per point of mouse drag.
const TURN_RATE: f32 = 0.45;
/// How long the art's files go between looks.
const WATCH_EVERY: Duration = Duration::from_millis(250);

/// The tools that change the design, which an undo can take back.
const CHANGING_TOOLS: &[&str] = &[
    "set_pattern",
    "clear_pattern",
    "set_full_texture",
    "set_effect",
    "set_flow",
    "set_keep",
    "open_pack",
];

/// The preset the paint came from, so its colours can still be changed.
#[derive(Clone, PartialEq)]
struct PresetChoice {
    key: &'static str,
    colours: [[u8; 3]; 4],
    seed: u32,
}

/// A note at the bottom of the window.
struct Note {
    text: String,
    error: bool,
    at: Instant,
}

/// What the viewport's mouse last did, for the gun to coast a little after a flick.
#[derive(Default)]
struct Coast {
    yaw: f32,
    pitch: f32,
}

pub struct App {
    render_state: egui_wgpu::RenderState,
    viewport: Viewport,
    gpu: GpuContext,
    studio: Option<Studio>,
    open_error: Option<String>,
    first_gun: Option<String>,
    listener: Option<Listener>,
    last_agent: Option<Instant>,
    home: Option<PathBuf>,
    search: String,
    focus_search: bool,
    list_shows: Option<u32>,
    spin: bool,
    coast: Coast,
    touched: bool,
    dirty: bool,
    undo: Vec<Design>,
    redo: Vec<Design>,
    preset: Option<PresetChoice>,
    keep_radius: f32,
    dragging_circle: Option<usize>,
    name: String,
    author: String,
    note: Option<Note>,
    saved_dir: Option<PathBuf>,
    last_watch: Instant,
    thumbs: HashMap<&'static str, TextureHandle>,
    texture_tex: Option<(u64, TextureHandle)>,
    texture_seen: Option<u64>,
    saved_list: Vec<(String, PathBuf)>,
    pending_shots: Vec<std::sync::mpsc::Sender<Result<commands::Reply, String>>>,
    show_agent: bool,
}

fn style(ctx: &egui::Context) {
    use egui::{FontFamily, FontId, TextStyle};
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(15, 16, 18);
    v.selection.bg_fill = ACCENT.linear_multiply(0.75);
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.hyperlink_color = ACCENT;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(44, 46, 51));
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
    v.widgets.active.corner_radius = egui::CornerRadius::same(6);
    v.widgets.open.corner_radius = egui::CornerRadius::same(6);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(40, 42, 47);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(54, 56, 62);
    // dark whatever the system is set to: a camo is judged on the picture, not against a white panel
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    let mut s = (*ctx.style()).clone();
    s.spacing.item_spacing = Vec2::new(8.0, 6.0);
    s.spacing.button_padding = Vec2::new(10.0, 5.0);
    s.spacing.interact_size.y = 26.0;
    s.spacing.slider_width = 118.0;
    s.text_styles = [
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Heading,
            FontId::new(19.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
    ]
    .into();
    ctx.set_style(s);
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(
        RichText::new(title.to_uppercase())
            .small()
            .color(FAINT)
            .strong(),
    );
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        gpu: GpuContext,
        named_rom: Option<PathBuf>,
        first_gun: Option<String>,
    ) -> App {
        cc.egui_ctx.set_fonts(super::fonts());
        style(&cc.egui_ctx);
        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("eframe runs on wgpu here");
        let viewport = Viewport::new(&render_state);
        let home = live::home_dir();
        let listener = home.as_deref().and_then(|h| {
            let ctx = cc.egui_ctx.clone();
            live::listen(h, move || ctx.request_repaint())
                .map_err(|e| eprintln!("dc-gunkit: no agent link: {e}"))
                .ok()
        });
        let mut app = App {
            render_state,
            viewport,
            gpu,
            studio: None,
            open_error: None,
            first_gun,
            listener,
            last_agent: None,
            home,
            search: String::new(),
            focus_search: false,
            list_shows: None,
            spin: false,
            coast: Coast::default(),
            touched: false,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            preset: None,
            keep_radius: 40.0,
            dragging_circle: None,
            name: String::new(),
            author: String::new(),
            note: None,
            saved_dir: None,
            last_watch: Instant::now(),
            thumbs: HashMap::new(),
            texture_tex: None,
            texture_seen: None,
            saved_list: pack::saved_camos(),
            pending_shots: Vec::new(),
            show_agent: false,
        };
        if let Some(r) = rom::discover(named_rom.as_deref(), app.home.as_deref()) {
            app.open_rom(&r);
        }
        app
    }

    fn open_rom(&mut self, dir: &Path) {
        match Studio::with_context(dir, self.gpu.clone()) {
            Ok(mut s) => {
                if let Some(h) = &self.home {
                    rom::remember(h, dir);
                }
                let wanted = self.first_gun.take();
                let row = wanted
                    .as_deref()
                    .and_then(|w| s.find_gun(w))
                    .or_else(|| s.find_gun("G36E"))
                    .or_else(|| s.guns.first().map(|g| g.row));
                if let Some(r) = row {
                    if let Err(e) = s.select(r) {
                        self.say_error(e);
                    }
                }
                self.studio = Some(s);
                self.open_error = None;
            }
            Err(e) => self.open_error = Some(e),
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.note = Some(Note {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    fn say_error(&mut self, text: impl Into<String>) {
        self.note = Some(Note {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    /// Keeps the design for an undo, before a change.
    fn checkpoint(&mut self) {
        if let Some(s) = &self.studio {
            self.undo.push(s.design.clone());
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }

    fn undo_step(&mut self, back: bool) {
        let Some(s) = self.studio.as_mut() else {
            return;
        };
        let (from, to) = if back {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        if let Some(d) = from.pop() {
            to.push(std::mem::replace(&mut s.design, d));
            self.preset = None;
            self.dirty = true;
        }
    }

    /// The agent's commands, on the window's own thread.
    fn run_requests(&mut self, ctx: &egui::Context) {
        let requests: Vec<Request> = match &self.listener {
            Some(l) => l.requests.try_iter().collect(),
            None => Vec::new(),
        };
        for r in requests {
            self.last_agent = Some(Instant::now());
            if r.tool == "window_screenshot" {
                self.pending_shots.push(r.reply);
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                continue;
            }
            if self.studio.is_none() {
                let _ = r.reply.send(Err(
                    "The studio is waiting for the game folder: ask the user to choose it in the window.".into(),
                ));
                continue;
            }
            if CHANGING_TOOLS.contains(&r.tool.as_str()) {
                self.checkpoint();
                self.preset = None;
            }
            let Some(s) = self.studio.as_mut() else {
                continue;
            };
            let answer = commands::run(s, &r.tool, &r.args);
            if r.tool == "save_pack" {
                self.saved_list = pack::saved_camos();
            }
            if matches!(r.tool.as_str(), "save_pack" | "open_pack") {
                self.name = s.design.name.clone();
            }
            let _ = r.reply.send(answer);
        }
    }

    fn take_screenshots(&mut self, ctx: &egui::Context) {
        let images: Vec<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .chain(i.events.iter())
                .filter_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
                .collect()
        });
        let Some(image) = images.first() else {
            return;
        };
        let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
        let answer =
            commands::png_of(image.size[0] as u32, image.size[1] as u32, rgba).map(|png| {
                commands::Reply {
                    json: json!({"width": image.size[0], "height": image.size[1]}),
                    png: Some(png),
                }
            });
        for reply in self.pending_shots.drain(..) {
            let _ = reply.send(answer.clone());
        }
    }

    fn take_dropped_files(&mut self, ctx: &egui::Context) {
        let files = ctx.input(|i| i.raw.dropped_files.clone());
        for f in files {
            let Some(path) = f.path else {
                continue;
            };
            if path.is_dir() {
                self.open_pack_dir(&path);
            } else {
                self.set_paint_file(&path);
            }
        }
    }

    fn set_paint_file(&mut self, path: &Path) {
        match Art::from_file(path) {
            Ok(art) => {
                self.checkpoint();
                if let Some(s) = self.studio.as_mut() {
                    s.design.paint = Some(art);
                }
                self.preset = None;
                self.dirty = true;
                let name = path.file_name().map(|n| n.to_string_lossy().to_string());
                self.say(format!(
                    "Using {}. Save over the file and the gun updates by itself.",
                    name.unwrap_or_default()
                ));
            }
            Err(e) => self.say_error(e),
        }
    }

    fn open_pack_dir(&mut self, dir: &Path) {
        let opened = pack::open(dir);
        self.take_opened(opened);
    }

    fn take_opened(&mut self, opened: Result<pack::Opened, String>) {
        match opened {
            Ok(o) => {
                self.checkpoint();
                self.preset = None;
                let Some(s) = self.studio.as_mut() else {
                    return;
                };
                let result = commands::apply_opened(s, o);
                let name = s.design.name.clone();
                self.name = name.clone();
                match result {
                    Ok(notes) if notes.is_empty() => self.say(format!("Opened {name}.")),
                    Ok(notes) => self.say(format!("Opened {name}; {}.", notes.join("; "))),
                    Err(e) => self.say_error(e),
                }
            }
            Err(e) => self.say_error(e),
        }
    }

    fn gun_list(&mut self, ui: &mut egui::Ui) {
        let Some(s) = self.studio.as_ref() else {
            return;
        };
        let selected = s.row();
        let wanted = self.search.to_ascii_lowercase();
        let list: Vec<(u32, String, bool)> = s
            .guns
            .iter()
            .filter(|g| wanted.is_empty() || g.name.to_ascii_lowercase().contains(&wanted))
            .map(|g| (g.row, g.name.clone(), g.on_sale))
            .collect();
        let edit = egui::TextEdit::singleline(&mut self.search)
            .hint_text("Search guns")
            .desired_width(f32::INFINITY)
            .margin(Vec2::new(8.0, 6.0));
        let response = ui.add(edit);
        if self.focus_search {
            response.request_focus();
            self.focus_search = false;
        }
        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let mut choose = None;
        if enter {
            choose = list.first().map(|g| g.0);
        }
        // the arrow keys walk the list, when nothing else is being typed in
        if !ui.ctx().wants_keyboard_input() {
            let step: i32 = ui.input(|i| {
                i32::from(i.key_pressed(egui::Key::ArrowDown))
                    - i32::from(i.key_pressed(egui::Key::ArrowUp))
            });
            if step != 0 && !list.is_empty() {
                let at = list.iter().position(|g| Some(g.0) == selected);
                let next = match at {
                    Some(i) => (i as i32 + step).rem_euclid(list.len() as i32) as usize,
                    None => 0,
                };
                choose = Some(list[next].0);
            }
        }
        ui.add_space(2.0);
        let rows = list.len();
        let spacing = ui.spacing().item_spacing.y;
        let height = ui.spacing().interact_size.y - spacing;
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        // a gun chosen another way (an agent, a pack, the arrow keys) comes into view, near the top
        if selected != self.list_shows {
            if let Some(i) = list.iter().position(|g| Some(g.0) == selected) {
                area =
                    area.vertical_scroll_offset(((i as f32 - 3.0).max(0.0)) * (height + spacing));
            }
            self.list_shows = selected;
        }
        area.show_rows(ui, height, rows, |ui, range| {
            for i in range {
                let (row, name, on_sale) = &list[i];
                let on = Some(*row) == selected;
                let text = if *on_sale {
                    RichText::new(name.as_str())
                } else {
                    RichText::new(name.as_str()).color(FAINT)
                };
                let (rect, r) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
                if on {
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(5),
                        ACCENT.linear_multiply(0.28),
                    );
                } else if r.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(5),
                        Color32::from_rgb(44, 46, 51),
                    );
                }
                let galley = egui::WidgetText::from(text).into_galley(
                    ui,
                    Some(egui::TextWrapMode::Truncate),
                    rect.width() - 16.0,
                    egui::TextStyle::Body,
                );
                let pos = rect.left_center() + Vec2::new(8.0, -galley.size().y / 2.0);
                let colour = ui.visuals().text_color();
                ui.painter().galley(pos, galley, colour);
                if r.clicked() {
                    choose = Some(*row);
                }
            }
        });
        if let Some(row) = choose.filter(|r| Some(*r) != selected) {
            if let Some(s) = self.studio.as_mut() {
                if let Err(e) = s.select(row) {
                    self.say_error(e);
                }
            }
        }
    }

    fn viewport_panel(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let dt = ui.input(|i| i.stable_dt).min(0.05);
        let Some(s) = self.studio.as_mut() else {
            return;
        };
        let (touched, moving) =
            mouse_on_gun(ui, &response, rect, &mut s.orbit, &mut self.coast, dt);
        self.touched |= touched;
        if moving {
            ui.ctx().request_repaint();
        }
        if self.spin && !response.dragged() {
            s.orbit.turn(dt * 28.0, 0.0);
            ui.ctx().request_repaint();
        }
        if s.animating() {
            ui.ctx().request_repaint();
        }
        self.viewport.show(ui, rect, s, &self.render_state);

        let cursor = if response.dragged_by(egui::PointerButton::Primary) {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        };
        if response.hovered() {
            ui.ctx().set_cursor_icon(cursor);
        }
        self.viewport_overlay(ui, rect);
    }

    fn viewport_overlay(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let Some(s) = self.studio.as_mut() else {
            return;
        };
        let dark = s.background[0] < 130;
        let ink = if dark { Color32::WHITE } else { Color32::BLACK };
        let soft = if dark {
            Color32::from_white_alpha(150)
        } else {
            Color32::from_black_alpha(150)
        };
        // the gun's name, top left
        let name = s.gun().map(|g| g.name.clone()).unwrap_or_default();
        ui.painter().text(
            rect.left_top() + Vec2::new(18.0, 14.0),
            egui::Align2::LEFT_TOP,
            name,
            egui::FontId::proportional(24.0),
            ink,
        );
        // the controls, along the bottom
        let bar = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.bottom() - 34.0),
            Vec2::new(rect.width().min(660.0), 40.0),
        );
        let mut hands = s.show_hands;
        let mut whole_gun = s.whole_gun;
        let mut view = s.view;
        let mut spin = self.spin;
        let mut background = s.background;
        let mut reset = false;
        let (first, third, has_hands) = (
            s.gun().is_some_and(|g| g.first_person.is_some()),
            s.gun().is_some_and(|g| g.third_person.is_some()),
            s.has_hands(),
        );
        let builder = egui::UiBuilder::new().max_rect(bar);
        ui.scope_builder(builder, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(20, 21, 24, 225))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(10, 5))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().button_padding = Vec2::new(8.0, 3.0);
                        ui.add_enabled_ui(first, |ui| {
                            ui.selectable_value(&mut view, View::FirstPerson, "First person");
                        });
                        ui.add_enabled_ui(third, |ui| {
                            ui.selectable_value(&mut view, View::ThirdPerson, "Third person");
                        });
                        ui.separator();
                        ui.add_enabled(
                            has_hands && view == View::FirstPerson,
                            egui::Checkbox::new(&mut hands, "Hands"),
                        );
                        ui.add_enabled(
                            view == View::FirstPerson && !hands,
                            egui::Checkbox::new(&mut whole_gun, "Whole gun"),
                        )
                        .on_hover_text(
                            "The third-person model's shape with the first-person texture on it. Off shows the first-person model as it is.",
                        );
                        ui.checkbox(&mut spin, "Spin");
                        ui.separator();
                        for (label, colour) in BACKGROUNDS {
                            let c = Color32::from_rgb(colour[0], colour[1], colour[2]);
                            let (r, click) =
                                ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                            ui.painter().circle_filled(r.center(), 8.0, c);
                            let chosen = background == *colour;
                            let line = if chosen {
                                Stroke::new(2.0_f32, ACCENT)
                            } else {
                                Stroke::new(1.0_f32, FAINT)
                            };
                            ui.painter().circle_stroke(r.center(), 8.0, line);
                            if click.on_hover_text(*label).clicked() {
                                background = *colour;
                            }
                        }
                        ui.separator();
                        if ui
                            .small_button("Reset")
                            .on_hover_text("Or double-click the gun")
                            .clicked()
                        {
                            reset = true;
                        }
                    });
                });
        });
        if view != s.view {
            if let Err(e) = s.set_view(view) {
                self.say_error(e);
            }
        }
        let Some(s) = self.studio.as_mut() else {
            return;
        };
        if whole_gun != s.whole_gun {
            if let Err(e) = s.set_whole_gun(whole_gun) {
                self.say_error(e);
            }
        }
        let Some(s) = self.studio.as_mut() else {
            return;
        };
        if hands != s.show_hands {
            if let Err(e) = s.set_hands(hands) {
                self.say_error(e);
            }
        }
        let Some(s) = self.studio.as_mut() else {
            return;
        };
        self.spin = spin;
        s.background = background;
        if reset {
            s.orbit = Default::default();
            self.coast = Coast::default();
        }
        // what to do with the mouse, until it has been done; and what the far side of a first-person gun is
        if s.far_side_is_reflected() {
            ui.painter().text(
                egui::pos2(rect.center().x, bar.top() - 16.0),
                egui::Align2::CENTER_CENTER,
                "This first-person model only has the side the player sees: this side is its reflection. Third person shows the real one.",
                egui::FontId::proportional(13.0),
                soft,
            );
        } else if !self.touched {
            ui.painter().text(
                egui::pos2(rect.center().x, bar.top() - 16.0),
                egui::Align2::CENTER_CENTER,
                "Drag to turn  ·  scroll to zoom  ·  right-drag to move  ·  double-click to reset",
                egui::FontId::proportional(13.0),
                soft,
            );
        }
        // an agent at work
        if self
            .last_agent
            .is_some_and(|t| t.elapsed() < Duration::from_secs(4))
        {
            let at = rect.right_top() + Vec2::new(-18.0, 20.0);
            let label = ui.painter().text(
                at,
                egui::Align2::RIGHT_CENTER,
                "Agent at work",
                egui::FontId::proportional(13.0),
                ACCENT,
            );
            ui.painter()
                .circle_filled(egui::pos2(label.left() - 9.0, at.y), 4.0, ACCENT);
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
    }

    fn thumb(&mut self, ctx: &egui::Context, key: &'static str) -> Option<TextureHandle> {
        if let Some(t) = self.thumbs.get(key) {
            return Some(t.clone());
        }
        let art = patterns::generate(key, None, 1).ok()?;
        let tex = ctx.load_texture(
            format!("preset {key}"),
            egui::ColorImage::from_rgba_unmultiplied(
                [art.width as usize, art.height as usize],
                &art.rgba,
            ),
            TextureOptions::LINEAR,
        );
        self.thumbs.insert(key, tex.clone());
        Some(tex)
    }

    fn pattern_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Pattern");
        let ctx = ui.ctx().clone();
        let mut pick = None;
        let current = self.preset.as_ref().map(|p| p.key);
        egui::Grid::new("presets")
            .num_columns(3)
            .spacing([6.0, 6.0])
            .show(ui, |ui| {
                for (i, (key, label, _)) in patterns::PRESETS.iter().enumerate() {
                    let size = Vec2::new(90.0, 54.0);
                    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
                    if let Some(t) = self.thumb(&ctx, key) {
                        ui.painter().image(
                            t.id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(0.35, 0.21)),
                            Color32::WHITE,
                        );
                    }
                    let on = current == Some(*key);
                    ui.painter().rect_stroke(
                        rect,
                        egui::CornerRadius::same(5),
                        if on {
                            Stroke::new(2.0_f32, ACCENT)
                        } else if r.hovered() {
                            Stroke::new(1.0_f32, Color32::WHITE)
                        } else {
                            Stroke::new(1.0_f32, Color32::from_black_alpha(160))
                        },
                        egui::StrokeKind::Inside,
                    );
                    let bottom = egui::Rect::from_min_max(
                        egui::pos2(rect.left(), rect.bottom() - 18.0),
                        rect.right_bottom(),
                    );
                    ui.painter().rect_filled(
                        bottom,
                        egui::CornerRadius::ZERO,
                        Color32::from_black_alpha(120),
                    );
                    ui.painter().text(
                        bottom.center(),
                        egui::Align2::CENTER_CENTER,
                        *label,
                        egui::FontId::proportional(11.5),
                        Color32::WHITE,
                    );
                    if r.on_hover_text("Click again for another one").clicked() {
                        pick = Some(*key);
                    }
                    if i % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
        if let Some(key) = pick {
            self.choose_preset(key);
        }
        // the colours of the preset in use
        if let Some(mut p) = self.preset.clone() {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Colours").color(FAINT));
                for c in p.colours.iter_mut() {
                    changed |= ui.color_edit_button_srgb(c).changed();
                }
            });
            if changed {
                if let Ok(art) = patterns::generate(p.key, Some(p.colours), p.seed) {
                    if let Some(s) = self.studio.as_mut() {
                        s.design.paint = Some(art);
                    }
                    self.dirty = true;
                }
                self.preset = Some(p);
            }
        }
        ui.horizontal(|ui| {
            if ui
                .button("Import image...")
                .on_hover_text("Or drop a PNG on the window")
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Images", &["png", "dtx"])
                    .pick_file()
                {
                    self.set_paint_file(&path);
                }
            }
            let has = self
                .studio
                .as_ref()
                .is_some_and(|s| s.design.paint.is_some());
            if ui.add_enabled(has, egui::Button::new("Remove")).clicked() {
                self.checkpoint();
                if let Some(s) = self.studio.as_mut() {
                    s.design.paint = None;
                }
                self.preset = None;
                self.dirty = true;
            }
        });
        let source = self
            .studio
            .as_ref()
            .and_then(|s| s.design.paint.as_ref())
            .and_then(|a| a.source.clone());
        if let Some(p) = source {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            ui.label(
                RichText::new(format!("Watching {name}"))
                    .small()
                    .color(FAINT),
            );
        }
    }

    fn choose_preset(&mut self, key: &'static str) {
        let Some(&(_, _, colours)) = patterns::PRESETS.iter().find(|p| p.0 == key) else {
            return;
        };
        let (colours, seed) = match &self.preset {
            Some(p) if p.key == key => (p.colours, p.seed + 1),
            _ => (colours, 1),
        };
        match patterns::generate(key, Some(colours), seed) {
            Ok(art) => {
                self.checkpoint();
                if let Some(s) = self.studio.as_mut() {
                    s.design.paint = Some(art);
                }
                self.preset = Some(PresetChoice { key, colours, seed });
                self.dirty = true;
            }
            Err(e) => self.say_error(e),
        }
    }

    fn effect_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Animation");
        let Some(s) = self.studio.as_ref() else {
            return;
        };
        let fx = s.design.fx;
        let mut on = fx.is_some();
        if ui.checkbox(&mut on, "Moving light").on_hover_text("A pattern drifts over the gun as light. Keep it subtle: it must never give a player away.").changed() {
            self.checkpoint();
            if let Some(s) = self.studio.as_mut() {
                s.design.fx = on.then_some(CamoFx {
                    speed: [0.02, 0.012],
                    gain: [0.0, 0.65],
                    seconds: 0.0,
                });
            }
            self.dirty = true;
        }
        let Some(mut f) = fx.filter(|_| on) else {
            return;
        };
        let was = f;
        let mut touched = false;
        let mut slide = |ui: &mut egui::Ui,
                         value: &mut f32,
                         range: std::ops::RangeInclusive<f32>,
                         label: &str| {
            let r = ui.add(
                egui::Slider::new(value, range)
                    .text(label)
                    .fixed_decimals(3),
            );
            touched |= r.drag_started();
        };
        slide(ui, &mut f.speed[0], -0.2..=0.2, "Drift across");
        slide(ui, &mut f.speed[1], -0.2..=0.2, "Drift down");
        slide(ui, &mut f.gain[0], 0.0..=1.0, "Lowest light");
        slide(ui, &mut f.gain[1], 0.0..=1.0, "Highest light");
        slide(ui, &mut f.seconds, 0.0..=8.0, "Pulse secs");
        if touched {
            self.checkpoint();
        }
        if f != was {
            if let Some(s) = self.studio.as_mut() {
                s.design.fx = Some(camo::limited_fx(f));
            }
            self.dirty = true;
        }
        // which pattern moves
        let mut glow_pick: Option<Option<&'static str>> = None;
        let has_own = self
            .studio
            .as_ref()
            .is_some_and(|s| s.design.glow.is_some());
        egui::ComboBox::from_label("Light pattern")
            .selected_text(if has_own { "Its own" } else { "Same as paint" })
            .show_ui(ui, |ui| {
                if ui.selectable_label(!has_own, "Same as paint").clicked() {
                    glow_pick = Some(None);
                }
                for (key, label, _) in patterns::PRESETS {
                    if ui.selectable_label(false, *label).clicked() {
                        glow_pick = Some(Some(key));
                    }
                }
            });
        if let Some(choice) = glow_pick {
            self.checkpoint();
            let art = choice.and_then(|k| patterns::generate(k, None, 1).ok());
            if let Some(s) = self.studio.as_mut() {
                s.design.glow = art;
            }
            self.dirty = true;
        }
        if ui.small_button("Light pattern from image...").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "dtx"])
                .pick_file()
            {
                match Art::from_file(&path) {
                    Ok(art) => {
                        self.checkpoint();
                        if let Some(s) = self.studio.as_mut() {
                            s.design.glow = Some(art);
                        }
                        self.dirty = true;
                    }
                    Err(e) => self.say_error(e),
                }
            }
        }
    }

    fn texture_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Texture");
        let Some(s) = self.studio.as_ref() else {
            return;
        };
        let Some(tex) = s.shown_texture() else {
            ui.label(RichText::new("This model has no main texture.").color(FAINT));
            return;
        };
        let (Some(row), view, revision) = (s.row(), s.view, s.revision) else {
            return;
        };
        let keep = s.design.keep_of(row, view).to_vec();
        if self.texture_seen != Some(revision) || self.texture_tex.is_none() {
            if let Some(px) = tex.levels.first() {
                let handle = ui.ctx().load_texture(
                    "gun texture",
                    egui::ColorImage::from_rgba_unmultiplied(
                        [tex.width as usize, tex.height as usize],
                        &opaque(px),
                    ),
                    TextureOptions::LINEAR,
                );
                self.texture_tex = Some((revision, handle));
            }
            self.texture_seen = Some(revision);
        }
        let side = RIGHT_WIDTH - 24.0;
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click_and_drag());
        if let Some((_, t)) = &self.texture_tex {
            ui.painter().image(
                t.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let scale = side / tex.width.max(1) as f32;
        let to_screen = |c: &Circle| {
            egui::pos2(
                rect.left() + c.x * scale,
                rect.top() + c.y * tex.height as f32 / tex.width.max(1) as f32 * scale,
            )
        };
        let to_texel = |p: egui::Pos2| {
            (
                (p.x - rect.left()) / scale,
                (p.y - rect.top()) / scale * tex.width as f32 / tex.height.max(1) as f32,
            )
        };
        for (i, c) in keep.iter().enumerate() {
            let on = self.dragging_circle == Some(i);
            ui.painter().circle_stroke(
                to_screen(c),
                c.radius * scale,
                Stroke::new(if on { 2.5_f32 } else { 1.5_f32 }, ACCENT),
            );
        }
        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        let hit = pointer.and_then(|p| {
            keep.iter()
                .position(|c| to_screen(c).distance(p) <= (c.radius * scale).max(6.0))
        });
        let mut list = keep.clone();
        let mut changed = false;
        if response.drag_started_by(egui::PointerButton::Primary) {
            self.dragging_circle = hit;
            if hit.is_some() {
                self.checkpoint();
            }
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            if let (Some(i), Some(p)) = (self.dragging_circle, pointer) {
                let (x, y) = to_texel(p);
                if let Some(c) = list.get_mut(i) {
                    c.x = x;
                    c.y = y;
                    changed = true;
                }
            }
        }
        if response.drag_stopped() {
            self.dragging_circle = None;
        }
        if response.clicked() {
            if let (None, Some(p)) = (hit, pointer) {
                self.checkpoint();
                let (x, y) = to_texel(p);
                list.push(Circle {
                    x,
                    y,
                    radius: self.keep_radius * tex.width as f32 / 1024.0,
                });
                changed = true;
            }
        }
        if response.secondary_clicked() {
            if let Some(i) = hit {
                self.checkpoint();
                list.remove(i);
                changed = true;
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if let (Some(i), true) = (hit, scroll != 0.0) {
                if let Some(c) = list.get_mut(i) {
                    c.radius = (c.radius * (scroll * 0.003).exp()).clamp(1.0, tex.width as f32);
                    changed = true;
                }
            }
        }
        if changed {
            if let Some(s) = self.studio.as_mut() {
                s.design.keep.insert((row, view), list);
            }
            self.dirty = true;
        }
        if let Some(s) = &self.studio {
            let found = s.lenses();
            if !found.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "{} lens{} found and kept clean.",
                        found.len(),
                        if found.len() == 1 { "" } else { "es" }
                    ))
                    .small()
                    .color(ACCENT),
                );
            }
        }
        ui.label(
            RichText::new(format!(
                "{} x {}. Click a part to leave it unpainted, such as a lens. Drag a circle to move it, scroll to size it, right-click to remove it.",
                tex.width, tex.height
            ))
            .small()
            .color(FAINT),
        );
        ui.horizontal(|ui| {
            if ui
                .small_button("Full texture from image...")
                .on_hover_text(
                    "Art drawn to this layout replaces the texture, on this gun and view",
                )
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Images", &["png", "dtx"])
                    .pick_file()
                {
                    match Art::from_file(&path) {
                        Ok(art) => {
                            self.checkpoint();
                            if let Some(s) = self.studio.as_mut() {
                                s.design.full.insert((row, view), art);
                            }
                            self.dirty = true;
                        }
                        Err(e) => self.say_error(e),
                    }
                }
            }
            let has = self
                .studio
                .as_ref()
                .is_some_and(|s| s.design.full.contains_key(&(row, view)));
            if has && ui.small_button("Remove it").clicked() {
                self.checkpoint();
                if let Some(s) = self.studio.as_mut() {
                    s.design.full.remove(&(row, view));
                }
                self.dirty = true;
            }
        });
    }

    fn library_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Open");
        let mut example = None;
        let mut saved = None;
        let mut open_dir = false;
        egui::ComboBox::from_id_salt("open camo")
            .selected_text("A camo...")
            .width(RIGHT_WIDTH - 32.0)
            .show_ui(ui, |ui| {
                ui.label(RichText::new("Examples").small().color(FAINT));
                for (name, ..) in pack::EXAMPLES {
                    if ui.selectable_label(false, *name).clicked() {
                        example = Some(*name);
                    }
                }
                if !self.saved_list.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("Saved").small().color(FAINT));
                    for (name, dir) in &self.saved_list {
                        if ui.selectable_label(false, name).clicked() {
                            saved = Some(dir.clone());
                        }
                    }
                }
                ui.separator();
                if ui.selectable_label(false, "A pack folder...").clicked() {
                    open_dir = true;
                }
            });
        if let Some(name) = example {
            self.take_opened(pack::open_example(name));
        }
        if let Some(dir) = saved {
            self.open_pack_dir(&dir);
        }
        if open_dir {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                self.open_pack_dir(&dir);
            }
        }
    }

    fn save_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Save");
        ui.add(
            egui::TextEdit::singleline(&mut self.name)
                .hint_text("Camo name")
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if ui
                .button(RichText::new("Save pack").strong())
                .on_hover_text("A pack for the gun on show, covering both of its models")
                .clicked()
            {
                self.save();
            }
            if let Some(dir) = self.saved_dir.clone() {
                if ui.small_button("Show folder").clicked() {
                    show_in_files(&dir);
                }
            }
        });
    }

    fn save(&mut self) {
        let opts = SaveOptions {
            name: self.name.clone(),
            author: self.author.clone(),
            ..SaveOptions::default()
        };
        let Some(s) = self.studio.as_mut() else {
            return;
        };
        match s.save_pack(&opts) {
            Ok(saved) => {
                s.design.name = opts.name;
                self.saved_list = pack::saved_camos();
                let text = if saved.warnings.is_empty() {
                    format!(
                        "Saved and checked: row {}, {}.",
                        saved.row,
                        saved.dir.display()
                    )
                } else {
                    format!(
                        "Saved: row {}, {}. {}",
                        saved.row,
                        saved.dir.display(),
                        saved.warnings.join("; ")
                    )
                };
                self.saved_dir = Some(saved.dir);
                self.say(text);
            }
            Err(e) => self.say_error(e),
        }
    }

    fn agent_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let header = RichText::new("CONNECT AN AGENT")
            .small()
            .color(FAINT)
            .strong();
        if ui
            .add(egui::Label::new(header).sense(Sense::click()))
            .clicked()
        {
            self.show_agent = !self.show_agent;
        }
        if !self.show_agent {
            return;
        }
        let exe = std::env::current_exe()
            .ok()
            .map(|p| {
                p.with_file_name(if cfg!(windows) {
                    "dc-gunkit.exe"
                } else {
                    "dc-gunkit"
                })
            })
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "dc-gunkit".to_string());
        let command = format!("claude mcp add dc-gunkit -- \"{exe}\" mcp");
        ui.label(
            RichText::new("An agent changes the gun on this screen as you watch. Run this once:")
                .small(),
        );
        ui.add(egui::Label::new(RichText::new(command.as_str()).monospace().small()).wrap());
        if ui.small_button("Copy").clicked() {
            ui.ctx().copy_text(command);
            self.say("Copied.");
        }
    }

    fn welcome(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.heading("DeCombat GunKit");
                ui.add_space(8.0);
                ui.label("Design camos on the game's own guns. It reads your copy of the game and changes nothing in it.");
                ui.add_space(16.0);
                if ui.button(RichText::new("Choose your Combat Arms folder").strong()).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        if rom::is_rom(&dir) {
                            self.open_rom(&dir);
                        } else {
                            self.open_error = Some("That folder has no game in it. Pick the one with Engine.exe and a Game folder.".into());
                        }
                    }
                }
                if let Some(e) = &self.open_error {
                    ui.add_space(10.0);
                    ui.colored_label(Color32::from_rgb(235, 110, 90), e);
                }
            });
        });
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (undo, redo, find) = ctx.input_mut(|i| {
            use egui::{Key, KeyboardShortcut, Modifiers};
            let redo = i.consume_shortcut(&KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT,
                Key::Z,
            ));
            let undo = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z));
            let find = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F));
            (undo, redo, find)
        });
        if undo {
            self.undo_step(true);
        }
        if redo {
            self.undo_step(false);
        }
        if find {
            self.focus_search = true;
        }
    }
}

/// What the mouse does to the gun: the left button turns it, and a flick keeps it going for a moment; the right or middle button moves
/// it; the wheel or a pinch zooms; a double click puts it back. Returns whether it was touched and whether it is still coasting.
fn mouse_on_gun(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: egui::Rect,
    orbit: &mut Orbit,
    coast: &mut Coast,
    dt: f32,
) -> (bool, bool) {
    let mut touched = false;
    let mut moving = false;
    if response.dragged_by(egui::PointerButton::Primary) {
        let d = response.drag_delta();
        let (yaw, pitch) = (d.x * TURN_RATE, d.y * TURN_RATE);
        orbit.turn(yaw, pitch);
        *coast = Coast { yaw, pitch };
        touched = true;
    } else if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        let d = response.drag_delta();
        let per_point = 2.2 / rect.height().max(1.0) / orbit.zoom;
        orbit.pan[0] += d.x * per_point;
        orbit.pan[1] -= d.y * per_point;
        touched = true;
    } else {
        orbit.turn(coast.yaw, coast.pitch);
        let keep = 0.0005_f32.powf(dt);
        coast.yaw *= keep;
        coast.pitch *= keep;
        if coast.yaw.abs() < 0.01 && coast.pitch.abs() < 0.01 {
            *coast = Coast::default();
        } else {
            moving = true;
        }
    }
    if response.hovered() {
        let (scroll, pinch) = ui.input(|i| (i.raw_scroll_delta.y, i.zoom_delta()));
        if scroll != 0.0 || pinch != 1.0 {
            orbit.zoom_by((scroll * 0.0025).exp() * pinch);
            touched = true;
        }
    }
    if response.double_clicked() {
        *orbit = Orbit::default();
        *coast = Coast::default();
    }
    (touched, moving)
}

/// The picture of a texture as it should look in a panel: its colour, with no see-through holes.
fn opaque(px: &[u8]) -> Vec<u8> {
    let mut out = px.to_vec();
    for p in out.as_chunks_mut::<4>().0 {
        p[3] = 255;
    }
    out
}

fn show_in_files(dir: &Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(dir).spawn();
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.run_requests(ctx);
        self.take_screenshots(ctx);
        if self.studio.is_none() {
            self.welcome(ctx);
            return;
        }
        self.take_dropped_files(ctx);
        self.shortcuts(ctx);
        if self.last_watch.elapsed() >= WATCH_EVERY {
            self.last_watch = Instant::now();
            if let Some(s) = self.studio.as_mut() {
                if let Some(e) = s.watch_files() {
                    self.say_error(e);
                }
            }
        }
        ctx.request_repaint_after(WATCH_EVERY);
        if std::mem::take(&mut self.dirty) {
            if let Some(s) = self.studio.as_mut() {
                if let Err(e) = s.repaint() {
                    self.say_error(e);
                }
            }
        }

        egui::SidePanel::left("guns")
            .exact_width(LEFT_WIDTH)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                self.gun_list(ui);
            });
        egui::SidePanel::right("camo")
            .exact_width(RIGHT_WIDTH)
            .resizable(false)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        self.pattern_section(ui);
                        self.effect_section(ui);
                        self.texture_section(ui);
                        self.library_section(ui);
                        self.save_section(ui);
                        self.agent_section(ui);
                        ui.add_space(8.0);
                    });
            });
        let expired = self
            .note
            .as_ref()
            .is_some_and(|n| n.at.elapsed() > Duration::from_secs(9));
        if expired {
            self.note = None;
        }
        if let Some(n) = &self.note {
            let (text, error) = (n.text.clone(), n.error);
            egui::TopBottomPanel::bottom("note").show(ctx, |ui| {
                let colour = if error {
                    Color32::from_rgb(235, 110, 90)
                } else {
                    ui.visuals().text_color()
                };
                ui.add_space(2.0);
                ui.add(egui::Label::new(RichText::new(text).color(colour)).wrap());
                ui.add_space(2.0);
            });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| self.viewport_panel(ui));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, Event, Modifiers, PointerButton, RawInput, Rect};

    /// Runs one frame of a gun viewport with these input events, through egui's own input handling.
    fn frame(
        ctx: &egui::Context,
        events: Vec<Event>,
        orbit: &mut Orbit,
        coast: &mut Coast,
    ) -> bool {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 600.0))),
            events,
            ..RawInput::default()
        };
        let mut touched = false;
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                let response = ui.allocate_rect(rect, Sense::click_and_drag());
                touched = mouse_on_gun(ui, &response, rect, orbit, coast, 0.016).0;
            });
        });
        touched
    }

    fn button(pos: egui::Pos2, button: PointerButton, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn drag(ctx: &egui::Context, which: PointerButton, orbit: &mut Orbit, coast: &mut Coast) {
        let (a, b) = (pos2(300.0, 300.0), pos2(400.0, 340.0));
        frame(ctx, vec![Event::PointerMoved(a)], orbit, coast);
        frame(ctx, vec![button(a, which, true)], orbit, coast);
        frame(
            ctx,
            vec![Event::PointerMoved(pos2(340.0, 316.0))],
            orbit,
            coast,
        );
        frame(ctx, vec![Event::PointerMoved(b)], orbit, coast);
        frame(ctx, vec![button(b, which, false)], orbit, coast);
    }

    #[test]
    fn dragging_with_the_left_button_turns_the_gun_and_a_flick_coasts_to_a_stop() {
        let ctx = egui::Context::default();
        let (mut orbit, mut coast) = (Orbit::default(), Coast::default());
        let before = orbit;
        drag(&ctx, PointerButton::Primary, &mut orbit, &mut coast);
        assert!(orbit.yaw > before.yaw + 10.0, "{orbit:?}");
        assert!(orbit.pitch > before.pitch + 3.0, "{orbit:?}");
        assert_eq!(orbit.pan, before.pan);
        for _ in 0..600 {
            frame(&ctx, Vec::new(), &mut orbit, &mut coast);
        }
        assert_eq!((coast.yaw, coast.pitch), (0.0, 0.0), "it comes to rest");
    }

    #[test]
    fn dragging_with_the_right_button_moves_the_gun_without_turning_it() {
        let ctx = egui::Context::default();
        let (mut orbit, mut coast) = (Orbit::default(), Coast::default());
        let before = orbit;
        drag(&ctx, PointerButton::Secondary, &mut orbit, &mut coast);
        assert!(orbit.pan[0] > 0.0 && orbit.pan[1] < 0.0, "{orbit:?}");
        assert_eq!((orbit.yaw, orbit.pitch), (before.yaw, before.pitch));
    }

    #[test]
    fn the_wheel_zooms_and_a_double_click_puts_the_gun_back() {
        let ctx = egui::Context::default();
        let (mut orbit, mut coast) = (Orbit::default(), Coast::default());
        let p = pos2(300.0, 300.0);
        frame(&ctx, vec![Event::PointerMoved(p)], &mut orbit, &mut coast);
        frame(&ctx, Vec::new(), &mut orbit, &mut coast);
        let wheel = Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, 120.0),
            modifiers: Modifiers::NONE,
        };
        let touched = frame(&ctx, vec![wheel], &mut orbit, &mut coast);
        assert!(touched && orbit.zoom > 1.0, "{orbit:?}");
        orbit.yaw = 33.0;
        for _ in 0..2 {
            frame(
                &ctx,
                vec![button(p, PointerButton::Primary, true)],
                &mut orbit,
                &mut coast,
            );
            frame(
                &ctx,
                vec![button(p, PointerButton::Primary, false)],
                &mut orbit,
                &mut coast,
            );
        }
        frame(&ctx, Vec::new(), &mut orbit, &mut coast);
        assert_eq!(orbit, Orbit::default());
    }
}
