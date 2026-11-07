//! The menu screens in a window (feature `window`): the same borderless window plan as the game, the menu's display
//! list drawn as overlay quads every frame ([`super::paint::MenuPainter`]), keys / characters / pointer as
//! [`MenuInput`]s. Like the game window the menu is never frame-capped: Immediate presentation unless `--vsync` (on macOS Fifo unless `--no-vsync`, CLI-1575). The menu and the game share one event
//! loop and run on it in turns: menu, loading page, match, menu again (waiting room), until the player quits.
//!
//! One window for the whole run (w2-loading): the menu opens it hidden and shows it after its first frame; at the
//! game start the menu draws the loading page ([`crate::loading`]) in it while the session joins, then hands the
//! window to the match ([`crate::window::run_window_in_host`]), which hands it back at its end. Nothing in between
//! closes or opens a window, so the screen goes lobby -> loading page -> match -> waiting room without an undrawn
//! (white) frame or a moment without a window.
//!
//! Cursor: free on the menu screens; with the ROM's cursor image the menu draws it ([`super::cursor`]) and the
//! system cursor is hidden over the window while it has the focus (visible again when the focus goes); without the
//! image the system cursor stays. The game window takes the cursor as before.

use super::handoff::{game_options, LobbyHandoff};
use super::paint::MenuPainter;
use super::{Menu, MenuInput, MenuKey, MenuOutcome};
use crate::error::ClientError;
use crate::music::MusicCue;
use crate::net::{NetOptions, NetWorld};
use crate::sound::SoundDirector;
use crate::window::{open_host, run_window_in_host, KeyIn, WindowHost, WindowLook, WindowOptions};
// client-fps: the game window's exit report after every match
use crate::net::NetHandle;
use crate::window::exit_report_lines;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::WindowId;

/// The menu's window and its painter (kept across the menu's turns and lent to the match).
struct Live {
    host: WindowHost,
    painter: MenuPainter,
}

/// The loading page's turn: the session that joins, until when the menu waits for its world, and the outcome.
struct Joining {
    handle: NetHandle,
    deadline: Instant,
    timeout: Duration,
    result: Option<Result<String, ClientError>>,
}

struct MenuApp<'a> {
    /// CLI-890: presentation frame pacer
    pacer: crate::pacer::Pacer,
    /// CLI-1596: the front end's frame-rate meter and when its text was last handed to the menu.
    fps: crate::frame::FpsMeter,
    fps_last: Option<Instant>,
    fps_shown: Option<Instant>,
    menu: &'a mut Menu,
    opts: &'a WindowOptions,
    live: Option<Live>,
    error: Option<ClientError>,
    closed: bool,
    /// The turn asked its event loop to end (the desktop's `exit`; on iPadOS the driver reads it, CLI-1604).
    ended: bool,
    mods: ModifiersState,
    clipboard: super::clipboard::SystemClipboard,
    pointer: [f32; 2],
    /// The window has the focus / the pointer is over it (the drawn cursor shows only with both).
    focused: bool,
    inside: bool,
    /// The menu phase's sound director (background music); `None` with `--no-sound`.
    sound: &'a mut Option<SoundDirector>,
    /// w3-options: the options the player confirmed in this run (they replace the command line's for the matches).
    changed: &'a mut Option<crate::settings::Settings>,
    /// fix-hang: the scripted menu player in the window (`--menu-pilot`, hands-free test runs of the menu path).
    pilot: &'a mut Option<super::pilot::Pilot>,
    /// The user's clicks as `click` events (`--events`, gate T2).
    clicks: crate::steps::ClickWatch,
    /// The loading page's turn (`None`: the menu's own turn).
    joining: Option<Joining>,
    /// The part of a wheel turn below one notch (touch pads give fractions).
    wheel_rest: f32,
    /// CLI-1848: frames drawn by the loop when the window system stops delivering redraw events (ms since `epoch`).
    redraw_keep: crate::redrawkeep::RedrawKeeper,
    epoch: Instant,
}

/// The options' volumes and switches in the menu's sound director.
pub fn apply_to_director(s: &crate::settings::Settings, d: &mut SoundDirector) -> bool {
    super::apply_sound(s, d)
}

/// The options in the window options of a match: the volumes, the mouse look and the window size.
pub fn apply_to_options(s: &crate::settings::Settings, o: &mut WindowOptions) {
    o.audio.effects_volume = s.effective_effects();
    o.audio.music_volume = s.effective_music();
    o.look.sensitivity = s.sensitivity as f32;
    o.look.invert_y = s.invert_y;
    // CLI-1280: the resolution setting is the windowed look's size (full screen stays the monitor's rectangle);
    // `--size` keeps its size for the run (CLI-1282)
    if !o.keep_size {
        if let Some(r) = s.resolution {
            o.windowed_size = Some(r);
        }
    }
}

/// One menu frame of the background music: the screen's cue (if it changed), the interface sounds the menu asked
/// for (clicks, countdown, result), and one music step ([`Menu::sound_frame`], pinned by `menu_music_plays_the_cues`).
fn menu_music(menu: &mut Menu, sound: &mut Option<SoundDirector>) {
    menu.sound_frame(sound.as_mut());
}

/// Menu-window reopens after a GPU device loss this process (CLI-460).
static MENU_REOPENS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// The fourth loss ends the menu with an error instead of reopening.
const MAX_MENU_REOPENS: u32 = 3;

/// Menu keys from the physical key (Shift+Tab = back tab).
// DC-DEFAULT: CLI-175 (proposed) menu keys: Enter / keypad Enter, Escape, Tab, Backspace, Space, Up, Down; F2 / F3
// (CLI-1173, the chat target keys of cshell+0x205e0)
pub fn menu_key(code: KeyCode, shift: bool) -> Option<MenuKey> {
    Some(match code {
        KeyCode::Enter | KeyCode::NumpadEnter => MenuKey::Enter,
        KeyCode::Escape => MenuKey::Escape,
        KeyCode::Tab if shift => MenuKey::BackTab,
        KeyCode::Tab => MenuKey::Tab,
        KeyCode::Backspace => MenuKey::Backspace,
        KeyCode::Space => MenuKey::Space,
        KeyCode::ArrowUp => MenuKey::Up,
        KeyCode::ArrowDown => MenuKey::Down,
        KeyCode::F2 => MenuKey::F2,
        KeyCode::F3 => MenuKey::F3,
        _ => return None,
    })
}

impl MenuApp<'_> {
    /// The window options with the options the player confirmed in this run (the look's sizes follow them).
    fn effective_opts(&self) -> WindowOptions {
        let mut o = self.opts.clone();
        if let Some(s) = self.changed.as_ref() {
            apply_to_options(s, &mut o);
        }
        o
    }

    /// CLI-1280: confirmed options change the windowed look's size at once (and a changed look, e.g. after a
    /// reset); the same look at the same size leaves the window where it is.
    fn follow_settings(&mut self) {
        let Some(s) = self.changed.as_ref() else {
            return;
        };
        let want = if s.window == crate::settings::WinStyle::Bordered {
            WindowLook::Bordered
        } else {
            WindowLook::Borderless
        };
        let o = self.effective_opts();
        let Some(l) = &self.live else {
            return;
        };
        let w = &l.host.window;
        let same = crate::window::current_look() == Some(want)
            && crate::window::plan_here(w, want, &o).is_none_or(|p| {
                let now = w.inner_size();
                (now.width, now.height) == (p.width, p.height)
            });
        if !same {
            crate::window::apply_look(w, want, &o);
        }
    }

    fn start(&mut self, el: &ActiveEventLoop) -> Result<Live, ClientError> {
        let host = open_host(el, self.opts)?;
        let painter = MenuPainter::new(&host.ctx, host.config.format);
        let mut live = Live { host, painter };
        self.take_window(&mut live);
        Ok(live)
    }

    /// Opens a new window (hidden), draws the first frame into it and shows it.
    // DC-DEFAULT: CLI-511 (proposed) a new window shows once its first frame was presented
    fn open_window(&mut self, el: &ActiveEventLoop) {
        match self.start(el) {
            Ok(l) => {
                self.live = Some(l);
                self.menu.update();
                let drawn = crate::gpulost::guard(|| self.redraw());
                if let Some(l) = &self.live {
                    l.host.window.set_visible(true);
                }
                if drawn.is_err() {
                    if let Some(l) = &self.live {
                        l.host.window.request_redraw();
                    }
                }
            }
            Err(e) => {
                self.error = Some(e);
                self.end(el);
            }
        }
    }

    /// The window becomes the menu's (a new one, or the one the match handed back): menu size, cursor rule, redraw.
    fn take_window(&mut self, live: &mut Live) {
        let size = live.host.window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        if (w, h) != (live.host.config.width, live.host.config.height) {
            live.host.config.width = w;
            live.host.config.height = h;
            live.host
                .surface
                .configure(&live.host.ctx.device, &live.host.config);
        }
        self.menu
            .set_size(live.host.config.width, live.host.config.height);
        live.host.window.request_redraw();
        self.redraw_keep.arm(self.now_ms());
        // L1: the menu draws the ROM's cursor image; the system cursor is hidden over the window
        // DC-DEFAULT: CLI-480 (proposed) system cursor hidden while the menu window has the focus, shown when it
        // loses it (and the drawn one hidden); the drawn one also hides while the pointer is outside the window
        let own = self.menu.has_cursor_image();
        live.host.window.set_cursor_visible(!own);
        self.menu.set_pointer_shown(true);
        crate::events::emit(
            "cursor",
            serde_json::json!({ "image": if own { "menu-pcx" } else { "os-default" }, "visible": !own, "grabbed": false, "screen": self.menu.screen_name() }),
        );
    }

    /// Ms since the turn began (the redraw keeper's clock).
    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis().min(u64::MAX as u128) as u64
    }

    fn redraw(&mut self) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        // CLI-1848: focused again or a stalled redraw chain: the surface again at the window's real size
        if self.redraw_keep.take_reconfigure() {
            let size = live.host.window.inner_size();
            if size.width > 0 && size.height > 0 {
                let h = &mut live.host;
                h.config.width = size.width;
                h.config.height = size.height;
                h.surface.configure(&h.ctx.device, &h.config);
                self.menu.set_size(size.width, size.height);
            }
        }
        let host = &live.host;
        live.painter
            .prepare(&host.ctx, self.menu, &mut crate::prof::Laps::start());
        let t_acquire = Instant::now();
        let frame = match dc_hangdump::crumb_wait!(
            "frame: acquire the surface texture",
            host.surface.get_current_texture()
        ) {
            Ok(f) => {
                self.redraw_keep.on_acquire(true);
                f
            }
            // CLI-1848: any failure configures the surface again and skips the frame
            // PUBLIC: wgpu 25 docs (SurfaceError)
            Err(e) => {
                if self.redraw_keep.on_acquire(false) {
                    println!("dc-client menu: surface texture not acquired ({e:?}): surface configured again");
                }
                host.surface.configure(&host.ctx.device, &host.config);
                return;
            }
        };
        let acquire_ms = t_acquire.elapsed().as_secs_f64() * 1000.0;
        let view = frame.texture.create_view(&Default::default());
        let mut enc = host
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("menu"),
            });
        let size = (host.config.width, host.config.height);
        live.painter.encode(&host.ctx, &mut enc, &view, size);
        dc_hangdump::crumb_wait!("frame: queue submit", host.ctx.queue.submit([enc.finish()]));
        if let Some(name) = crate::steps::take_shot() {
            if host.readable {
                match crate::steps::read_texture(
                    &host.ctx,
                    &frame.texture,
                    host.config.format,
                    host.config.width,
                    host.config.height,
                ) {
                    Ok(img) => crate::steps::save_shot(&name, &img),
                    Err(e) => crate::steps::shot_failed(&name, &e),
                }
            } else {
                crate::steps::shot_failed(
                    &name,
                    "the surface cannot be read (no --shot-dir or no COPY_SRC)",
                );
            }
        }
        let t_present = Instant::now();
        dc_hangdump::crumb_wait!("frame: present", frame.present());
        #[cfg(target_os = "ios")]
        if std::env::var_os("DC_IOS_INPUT_LOG").is_some() {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 600 == 0 {
                eprintln!(
                    "dc-client ios input: menu frame {n}: acquire {acquire_ms:.2} ms, present {:.2} ms",
                    t_present.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
        // gate T1: the menu's frames feed the watchdog and the heartbeat
        crate::watchdog::frame();
        crate::watchdog::phase(crate::watchdog::Phase::Menu);
    }

    /// One frame: for a delivered redraw event (`from_loop` false) or drawn by the loop when the events stopped
    /// (CLI-1848); the next redraw is asked for either way.
    fn frame_turn(&mut self, el: &ActiveEventLoop, from_loop: bool) {
        // CLI-1595: the frame-rate setting's vertical-sync choice, applied to the surface in place
        if let Some(l) = self.live.as_mut() {
            let fifo = crate::window::wants_fifo(self.opts.vsync, self.opts.no_vsync);
            let want = if !fifo && l.host.immediate_ok {
                wgpu::PresentMode::Immediate
            } else {
                wgpu::PresentMode::Fifo
            };
            if l.host.config.present_mode != want {
                l.host.config.present_mode = want;
                l.host.surface.configure(&l.host.ctx.device, &l.host.config);
            }
        }
        // CLI-890: min(360, monitor refresh) in the front end
        let hz = self
            .live
            .as_ref()
            .and_then(|l| crate::window::monitor_refresh_mhz(&l.host.window));
        self.pacer.wait(crate::pacer::menu_cap(hz));
        // CLI-1596: the "Show FPS" text four times a second
        let now = Instant::now();
        if let Some(last) = self.fps_last {
            self.fps.push((now - last).as_secs_f64());
        }
        self.fps_last = Some(now);
        if crate::settings::show_fps() {
            if self
                .fps_shown
                .is_none_or(|t| now.duration_since(t).as_millis() >= 250)
            {
                self.menu
                    .set_fps_text(Some(format!("{:.0} fps", self.fps.fps())));
                self.fps_shown = Some(now);
            }
        } else {
            self.menu.set_fps_text(None);
            self.fps_shown = None;
        }
        if crate::gpulost::guard(|| self.redraw()).is_err() {
            // CLI-460: the lost device's objects are leaked (dropping them can deadlock in wgpu-core), the
            // old window is hidden and the menu goes on in a new window with a new device
            if let Some(old) = self.live.take() {
                old.host.window.set_visible(false);
                std::mem::forget(old);
            }
            // DC-DEFAULT: CLI-460: give up after MAX_MENU_REOPENS losses per process
            if MENU_REOPENS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
                > MAX_MENU_REOPENS
            {
                eprintln!("dc-client menu: GPU device lost {} times: giving up (the graphics driver keeps resetting)", MAX_MENU_REOPENS + 1);
                self.error = Some(ClientError::Window("GPU device lost repeatedly".into()));
                self.end(el);
                return;
            }
            println!("dc-client menu: GPU device lost: reopening the menu window");
            self.open_window(el);
            return;
        }
        let now = self.now_ms();
        if from_loop {
            self.redraw_keep.on_loop_frame(now);
        } else if let Some(n) = self.redraw_keep.on_redraw_event(now) {
            println!("{}", crate::redrawkeep::note_line("dc-client menu", n));
        }
        if let Some(l) = &self.live {
            l.host.window.request_redraw();
        }
    }

    /// CLI-1848: the loop draws the frame itself when no redraw event came for a while and the window is visible
    /// (no window query while the events come).
    fn keep_frames(&mut self, el: &ActiveEventLoop) {
        // the iPad's driver raises every frame from its tick (CLI-1604)
        if crate::window::IOS || self.ended {
            return;
        }
        let now = self.now_ms();
        if !self.redraw_keep.due(now) {
            return;
        }
        let Some(l) = self.live.as_ref() else {
            return;
        };
        let visible = crate::window::window_visible(&l.host.window);
        let (wake, note) = self.redraw_keep.poll(now, visible);
        if let Some(n) = note {
            println!("{}", crate::redrawkeep::note_line("dc-client menu", n));
        }
        if wake == crate::redrawkeep::Wake::Draw {
            self.frame_turn(el, true);
        }
    }

    /// One key press or release of the menu (winit's `KeyboardInput`, or a key of the iPad's keyboard bridge).
    fn key_in(&mut self, el: &ActiveEventLoop, k: KeyIn) {
        // a step file drives the run; the loading page takes no keys (as the window events above)
        if !k.down || crate::steps::active() || self.joining.is_some() {
            return;
        }
        if let PhysicalKey::Code(code) = k.physical {
            // CLI-1285: Cmd+Q on macOS closes the menu window like the window's close button
            if crate::window::is_mac_quit(code, true, k.repeat, self.mods.super_key()) {
                self.closed = true;
                self.end(el);
                return;
            }
            // w3-options: a key box of the options waits for the key itself (Escape still gives up)
            if self.menu.wants_scan() && code != KeyCode::Escape {
                if let Some(s) = crate::settings::scan_of_code(code) {
                    self.menu.input(MenuInput::Scan(s));
                }
                return;
            }
            // CLI-1280: Alt+Enter switches the look in the menu too (the Resized event follows)
            if crate::window::is_look_toggle(code, true, k.repeat, self.mods.alt_key()) {
                let o = self.effective_opts();
                if let Some(l) = &self.live {
                    crate::window::toggle_and_note(&l.host.window, &o);
                }
                return;
            }
            // CLI-1510..1512: Ctrl (Cmd on macOS and iPadOS) + V / C / X is a clipboard command, never typed characters
            let letter = match code {
                KeyCode::KeyV => Some('v'),
                KeyCode::KeyC => Some('c'),
                KeyCode::KeyX => Some('x'),
                _ => None,
            };
            let platform = letter.and_then(|l| {
                super::clipboard::chord(
                    l,
                    cfg!(any(target_os = "macos", target_os = "ios")),
                    self.mods.control_key(),
                    self.mods.super_key(),
                    self.mods.alt_key(),
                )
            });
            if let Some(c) = platform {
                super::clipboard::run(c, self.menu, &mut self.clipboard);
                return;
            }
            if let Some(mk) = menu_key(code, self.mods.shift_key()) {
                self.menu.input(MenuInput::Key(mk));
                return;
            }
        }
        if let Some(t) = &k.text {
            for c in t.chars().filter(|c| !c.is_control()) {
                self.menu.input(MenuInput::Char(c));
            }
        } else if let Key::Named(NamedKey::Space) = k.logical {
            self.menu.input(MenuInput::Char(' '));
        }
    }

    /// Ends the turn: the event loop's run ends on the desktop; on iPadOS the driver sees [`MenuApp::ended`].
    fn end(&mut self, el: &ActiveEventLoop) {
        self.ended = true;
        crate::window::end_loop(el);
    }

    /// Window pixels to the virtual 1024 x 768 screen.
    fn to_virtual(&self, x: f64, y: f64) -> [f32; 2] {
        let (w, h) = self.live.as_ref().map_or((1024, 768), |l| {
            (l.host.config.width.max(1), l.host.config.height.max(1))
        });
        [
            (x * 1024.0 / w as f64) as f32,
            (y * 768.0 / h as f64) as f32,
        ]
    }
}

impl ApplicationHandler for MenuApp<'_> {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.error.is_some() {
            return;
        }
        // gate T1: the frame watchdog watches the menu window's thread too (a window start counts as loading)
        crate::watchdog::phase(crate::watchdog::Phase::Loading);
        crate::watchdog::arm(
            self.opts
                .sound
                .log
                .as_ref()
                .and_then(|p| p.parent().map(|d| d.to_path_buf())),
        );
        // the window of an earlier turn (the menu's or the match's): drawn again at once, still on screen
        // (CLI-1288: on macOS the turn's exit hid it; it is ordered back before its redraw)
        if let Some(mut l) = self.live.take() {
            crate::window::reshow_carried(&l.host.window);
            self.take_window(&mut l);
            self.live = Some(l);
            return;
        }
        self.open_window(el);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.closed = true;
                self.end(el);
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            // L1 (CLI-480): focus gone -> the system cursor back and the drawn one hidden; focus back -> reversed
            WindowEvent::Focused(on) => {
                self.focused = on;
                if let Some(l) = &self.live {
                    l.host
                        .window
                        .set_cursor_visible(!(on && self.menu.has_cursor_image()));
                    // CLI-1848: focused again (alt-tab back): the surface again and a frame asked for
                    if on {
                        self.redraw_keep.on_shown();
                        l.host.window.request_redraw();
                    }
                }
                self.menu.set_pointer_shown(on && self.inside);
            }
            WindowEvent::CursorLeft { .. } => {
                self.inside = false;
                self.menu.set_pointer_shown(false);
            }
            WindowEvent::CursorEntered { .. } => {
                self.inside = true;
                self.menu.set_pointer_shown(self.focused);
            }
            // a step file drives the run: the user's pointer and keys do not reach the menu (gate T2); the loading
            // page takes no input either
            // EVIDENCE: cshell+0x1bb910 (the loading state's tick reads no input)
            WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::KeyboardInput { .. }
                if crate::steps::active() || self.joining.is_some() => {}
            WindowEvent::MouseWheel { delta, .. } => {
                // notches as the 2009 window hands them on (delta / 120; positive = away from the user)
                // EVIDENCE: cshell+0x1a4520
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 120.0,
                };
                self.wheel_rest += lines;
                let n = self.wheel_rest.trunc();
                if n != 0.0 {
                    self.wheel_rest -= n;
                    self.menu.input(MenuInput::Wheel(self.pointer, n as i32));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = self.to_virtual(position.x, position.y);
                self.inside = true;
                self.menu.set_pointer_shown(self.focused);
                #[cfg(target_os = "ios")]
                if std::env::var_os("DC_IOS_INPUT_LOG").is_some() {
                    eprintln!(
                        "dc-client ios input: menu move {:?} -> virtual {:?} (focused {}, screen {})",
                        position,
                        self.pointer,
                        self.focused,
                        self.menu.screen_name()
                    );
                }
                self.clicks.input(self.menu, MenuInput::Move(self.pointer));
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let p = self.pointer;
                #[cfg(target_os = "ios")]
                if std::env::var_os("DC_IOS_INPUT_LOG").is_some() {
                    eprintln!("dc-client ios input: menu {state:?} at virtual {p:?}");
                }
                self.clicks.input(
                    self.menu,
                    if state == ElementState::Pressed {
                        MenuInput::Press(p)
                    } else {
                        MenuInput::Release(p)
                    },
                );
            }
            WindowEvent::KeyboardInput { event, .. } => self.key_in(el, KeyIn::from(&event)),
            WindowEvent::Resized(size) => {
                if let Some(l) = self.live.as_mut() {
                    if size.width > 0 && size.height > 0 {
                        let h = &mut l.host;
                        h.config.width = size.width;
                        h.config.height = size.height;
                        h.surface.configure(&h.ctx.device, &h.config);
                        self.menu.set_size(size.width, size.height);
                        // CLI-1848: a restored window asks for its frame (the redraw chain may have ended)
                        h.window.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => self.frame_turn(el, false),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        crate::watchdog::pump();
        self.keep_frames(el);
        if let Some(j) = self.joining.as_mut() {
            // the loading page's turn: the load stage the session reached, until its world is announced or it ends
            self.menu.set_load_stage(j.handle.load_stage());
            if j.result.is_none() {
                j.result = j.handle.world_ready();
            }
            if j.result.is_none() && Instant::now() >= j.deadline {
                j.result = Some(Err(ClientError::Net(format!(
                    "no world announced within {:?} ({})",
                    j.timeout,
                    j.handle.phase().label()
                ))));
            }
            let done = j.result.is_some();
            // the lobby connection belongs to the session now: the menu reads no link events until it is back
            menu_music(self.menu, self.sound);
            if done || self.closed || crate::steps::quit_requested() {
                self.end(el);
            }
            return;
        }
        if let Some(p) = self.pilot.as_mut() {
            for i in p.next(self.menu) {
                self.menu.input(i);
            }
        }
        self.menu.pump_clipboard(&mut self.clipboard);
        self.menu.update();
        crate::steps::menu_frame(self.menu);
        if let Some(s) = self.menu.take_settings_change() {
            if let Some(d) = self.sound.as_mut() {
                if apply_to_director(&s, d) {
                    self.menu.recue_music();
                }
            }
            *self.changed = Some(s);
            self.follow_settings();
        }
        menu_music(self.menu, self.sound);
        if crate::steps::quit_requested() {
            self.closed = true;
        }
        if self.menu.quitting() || self.closed {
            self.end(el);
        }
        if self.menu.has_outcome() {
            self.end(el);
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        // the window stays: the caller lends it to the match, or drops it when the menu ends
        // CLI-1288: winit's macOS backend has just closed it (a fade-out to the desktop): ordered back at once, in
        // the same pass, so the next turn (the loading page, the match) draws into a window that never left
        if !self.closed {
            if let Some(l) = &self.live {
                crate::window::reshow_carried(&l.host.window);
            }
        }
        // gate T1: what follows (the wait for the match's server, bounded by its own time-out) is not watched
        crate::watchdog::phase(crate::watchdog::Phase::Idle);
    }
}

/// One turn of the menu on the event loop: the menu's screens (`joining` `None`) or the loading page while the session
/// joins. Hands back the window (`None`: lost to a GPU reset) and the turn's outcome.
struct Turn {
    live: Option<Live>,
    closed: bool,
    joined: Option<Result<String, ClientError>>,
}

impl<'a> MenuApp<'a> {
    /// The handler of one turn (its event loop has not run yet).
    #[allow(clippy::too_many_arguments)]
    fn new(
        menu: &'a mut Menu,
        wopts: &'a WindowOptions,
        live: Option<Live>,
        sound: &'a mut Option<SoundDirector>,
        changed: &'a mut Option<crate::settings::Settings>,
        pilot: &'a mut Option<super::pilot::Pilot>,
        joining: Option<Joining>,
    ) -> MenuApp<'a> {
        MenuApp {
            menu,
            opts: wopts,
            live,
            error: None,
            closed: false,
            ended: false,
            mods: ModifiersState::default(),
            clipboard: super::clipboard::SystemClipboard::new(),
            pointer: [0.0, 0.0],
            focused: true,
            inside: true,
            sound,
            changed,
            pilot,
            clicks: crate::steps::ClickWatch::default(),
            joining,
            wheel_rest: 0.0,
            redraw_keep: Default::default(),
            epoch: Instant::now(),
            pacer: Default::default(),
            fps: Default::default(),
            fps_last: None,
            fps_shown: None,
        }
    }

    /// The turn's outcome once its event loop ended.
    fn into_turn(mut self) -> Result<Turn, ClientError> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        Ok(Turn {
            live: self.live.take(),
            closed: self.closed,
            joined: self.joining.take().and_then(|j| j.result),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn run_turn(
    el: &mut EventLoop<()>,
    menu: &mut Menu,
    wopts: &WindowOptions,
    live: Option<Live>,
    sound: &mut Option<SoundDirector>,
    changed: &mut Option<crate::settings::Settings>,
    pilot: &mut Option<super::pilot::Pilot>,
    joining: Option<Joining>,
) -> Result<Turn, ClientError> {
    let mut app = MenuApp::new(menu, wopts, live, sound, changed, pilot, joining);
    #[cfg(not(target_os = "ios"))]
    {
        use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
        el.run_app_on_demand(&mut app)
            .map_err(|e| ClientError::Window(e.to_string()))?;
    }
    // iPadOS: the one event loop is entered once and never returns (crate::ios drives the turns)
    #[cfg(target_os = "ios")]
    {
        let _ = el;
        app.error = Some(ClientError::Window(
            "the iPad runs the menu through dc_client::ios_main".into(),
        ));
    }
    app.into_turn()
}

/// The sound director of a menu run: one for the menu and its matches, so the music's track and clock carry over
/// (AUDIO-1); `None` with `--no-sound`.
fn open_sound(wopts: &WindowOptions, base: &NetOptions) -> Option<SoundDirector> {
    wopts.sound.enabled.then(|| {
        let rom = base.rom.clone().or_else(|| dc_rom::resolve_rom().ok());
        let mut d = SoundDirector::open(rom.as_deref());
        if let Some(p) = wopts.sound.log.as_deref() {
            if let Err(e) = d.log_to(p) {
                d.note = format!("{}; log not written: {e}", d.note);
            }
        }
        println!("dc-client menu: {}", d.configure(&wopts.audio));
        // CLI-1599: the profile's BGM style (published when the settings were read)
        d.set_bgm_classic(crate::settings::bgm_classic());
        d
    })
}

/// A game the menu started: the session, its handle and the lobby hand-over.
struct Play {
    net: NetWorld,
    handle: NetHandle,
    handoff: LobbyHandoff,
}

/// The game start of `MenuOutcome::Play`: the loading music, the session's start, the loading page; returns the game
/// and the loading turn's wait.
fn begin_play(
    menu: &mut Menu,
    sound: &mut Option<SoundDirector>,
    base: &NetOptions,
    server: std::net::SocketAddr,
    lobby: dc_botclient::lobby::Lobby,
    mode: i32,
) -> (Play, Joining) {
    // the game launch starts the loading track
    // EVIDENCE: cshell+0x165650 -> 0x1630a0 (lobby game start: stop, then the loading track once, no fade; AUDIO_2009.md §8.2 row 2)
    if let Some(s) = sound.as_mut() {
        s.music_cue(MusicCue::Loading);
        s.music_tick();
    }
    let nick = lobby.profile.nick.clone();
    let handoff = LobbyHandoff::new(server, lobby, mode);
    let login = menu.data.user.clone();
    let mut opts = game_options(base, handoff.clone(), &nick, &login);
    // the room's weapon limit goes with the match (CLI-1605)
    opts.arms_limit = u32::try_from(menu.data.room.weapons).unwrap_or(0);
    // LG-274 / CLI-1627: and its Hacks setting (no-recoil)
    opts.room_hacks = menu.data.room.hacks;
    // CLI-1611: and the server set's weapon content
    opts.content = menu.mod_content();
    let net = NetWorld::start(opts);
    let handle = net.handle();
    println!("dc-client menu: joining {server}");
    // the loading page in the same window while the session joins (no blocking wait without a window)
    // EVIDENCE: cshell+0x1630a0 (the interface enters the loading state before the engine connects)
    // DC-DEFAULT: CLI-512 (proposed) one window for the run: the page until the world is announced, then
    // the match takes the window and hands it back
    menu.begin_loading();
    let joining = Joining {
        handle: handle.clone(),
        deadline: Instant::now() + base.load_timeout,
        timeout: base.load_timeout,
        result: None,
    };
    (
        Play {
            net,
            handle,
            handoff,
        },
        joining,
    )
}

/// The window options of a match: the run's, with the options the player confirmed and the server's mods (`mods`,
/// `Menu::mod_recipe`).
fn match_options(
    wopts: &WindowOptions,
    changed: &Option<crate::settings::Settings>,
    mods: Option<crate::mods::HostRecipe>,
    room: &super::link::RoomView,
    reticle: Option<(u16, u32)>,
    prefs: crate::window::MatchPrefs,
) -> WindowOptions {
    let mut mopts = wopts.clone();
    if let Some(s) = changed {
        apply_to_options(s, &mut mopts);
    }
    // CLI-1619: the server's mods for the match (vanilla: none)
    mopts.mods = mods;
    // LG-274 / CLI-1627: the room's Hacks setting when the lobby offered one
    mopts.room_hacks = (room.hacks_offered != 0).then_some(room.hacks);
    // LG-305 / CLI-1639: and a Gun Game room's ladder for the HUD
    mopts.gun_ladder = if room.gun_game {
        room.ladder.clone()
    } else {
        Vec::new()
    };
    // use-items-sweep: and the reticle item in use (CLI-1878)
    mopts.reticle = reticle;
    // CLI-1907: and the settings the options over the match start from
    mopts.prefs = Some(prefs);
    mopts
}

/// The menu's window lent to the match: its host (the match draws in it) and the painter, kept for the return.
fn lend_live(live: Option<Live>) -> (Option<WindowHost>, Option<MenuPainter>) {
    match live {
        Some(l) => (Some(l.host), Some(l.painter)),
        None => (None, None),
    }
}

/// The window back from a match (`None`: lost to a GPU reset).
fn return_live(host: Option<WindowHost>, painter: Option<MenuPainter>) -> Option<Live> {
    match (host, painter) {
        // gpu-lost-menu: a device the match's recovery replaced takes a new painter (the old one's atlas and pipelines
        // are of the lost device: the user's "Texture does not exist" panic on the match result screen)
        (Some(host), Some(painter)) => {
            let painter = painter.for_device(&host.ctx, host.config.format);
            Some(Live { host, painter })
        }
        // CLI-460: the window was lost with the device: its painter's objects are leaked too
        (None, Some(p)) => {
            std::mem::forget(p);
            None
        }
        _ => None,
    }
}

/// The end of a game the menu started (`r`: the match's outcome, or why it never ran): the session's shutdown, the
/// result and the box of the session's end on the menu.
fn finish_play(
    menu: &mut Menu,
    handle: &NetHandle,
    handoff: &LobbyHandoff,
    join_failed: bool,
    r: &Result<(), ClientError>,
) {
    handle.shutdown(Duration::from_secs(5));
    if let Err(e) = r {
        println!("dc-client menu: match ended with {e}");
    }
    let end = if join_failed {
        crate::loading::SessionEnd::JoinFailed
    } else {
        handle.session_end()
    };
    // CLI-1840: a match that did not end normally is a problem to report (asked or sent by the consent)
    super::diagbox::session_ended(menu, end, handle.diag_link());
    let result = handoff.take_result();
    menu.back_from_match(handoff.take_back(), result);
    menu.match_end_box(end);
}

/// The exit report of a match, printed after every match (client-fps).
fn print_exit_report(r: &crate::window::WindowReport) {
    for l in exit_report_lines(r) {
        println!("{l}");
    }
}

/// iPadOS: the menu and its matches on UIKit's one event loop (CLI-1604, `crate::ios`); never returns once the loop
/// runs.
#[cfg(target_os = "ios")]
pub fn run_ipad(menu: Menu, wopts: WindowOptions, base: NetOptions) -> Result<(), ClientError> {
    ipad::run(menu, wopts, base)
}

#[cfg(target_os = "ios")]
mod ipad;

/// Runs the menu and the matches it starts in one window until the player quits; returns the screens shown.
pub fn run_menu_window(
    mut menu: Menu,
    wopts: WindowOptions,
    base: NetOptions,
    mut pilot: Option<super::pilot::Pilot>,
) -> Result<Vec<String>, ClientError> {
    let mut el = crate::window::new_event_loop()?;
    el.set_control_flow(ControlFlow::Poll);
    // one sound director for the menu and its matches: the music's track and clock carry over (AUDIO-1)
    let mut sound = open_sound(&wopts, &base);
    // the run's one window (opened by the first turn)
    let mut live: Option<Live> = None;
    let mut changed: Option<crate::settings::Settings> = None;
    loop {
        let turn = run_turn(
            &mut el,
            &mut menu,
            &wopts,
            live.take(),
            &mut sound,
            &mut changed,
            &mut pilot,
            None,
        )?;
        live = turn.live;
        if turn.closed {
            break;
        }
        match menu.take_outcome() {
            Some(MenuOutcome::Play {
                server,
                lobby,
                mode,
            }) => {
                let (play, joining) =
                    begin_play(&mut menu, &mut sound, &base, server, *lobby, mode);
                let Play {
                    net,
                    handle,
                    handoff,
                } = play;
                let turn = run_turn(
                    &mut el,
                    &mut menu,
                    &wopts,
                    live.take(),
                    &mut sound,
                    &mut changed,
                    &mut pilot,
                    Some(joining),
                )?;
                live = turn.live;
                let quit = turn.closed;
                let joined = turn.joined.unwrap_or_else(|| {
                    Err(ClientError::Net("the client quit while loading".into()))
                });
                // a join that never reached a world is the 2009 time-out case (the box 0xbd0), unless the player quit
                let join_failed = joined.is_err() && !quit;
                let r = match joined {
                    Ok(w) if !quit => {
                        println!("dc-client menu: server announced {w:?}");
                        // the match takes the window over and hands it back (no window swap); the painter stays
                        let (host, painter) = lend_live(live.take());
                        // client-fps: the exit report of every match (it was dropped here before)
                        let reticle = super::colorbox::match_reticle(
                            menu.cat.rom.as_deref(),
                            &menu.data.store,
                        );
                        let prefs = crate::window::MatchPrefs {
                            settings: menu.data.settings.clone(),
                            commands: menu.data.commands.clone(),
                        };
                        let mopts = match_options(
                            &wopts,
                            &changed,
                            menu.mod_recipe(),
                            &menu.data.room,
                            reticle,
                            prefs,
                        );
                        let (r, back, host) =
                            run_window_in_host(&mut el, Box::new(net), mopts, sound.take(), host);
                        sound = back;
                        // CLI-1280: an Alt+Enter in the match is saved now (also when the player quits from it)
                        menu.store_display_note();
                        // CLI-1907: and what the options over the match changed
                        menu.store_match_settings();
                        live = return_live(host, painter);
                        r.map(|r| print_exit_report(&r))
                    }
                    Ok(_) => {
                        drop(net);
                        Err(ClientError::Net("the client quit while loading".into()))
                    }
                    Err(e) => {
                        drop(net);
                        Err(e)
                    }
                };
                finish_play(&mut menu, &handle, &handoff, join_failed, &r);
                if quit || crate::steps::quit_requested() {
                    break;
                }
            }
            Some(MenuOutcome::Quit) | None => break,
        }
    }
    if let Some(d) = sound.as_mut() {
        println!("dc-client menu: {}", d.close_output());
    }
    drop(live);
    Ok(menu.trail.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::link::LinkOptions;
    use crate::menu::popup::{BoxTag, Popup};
    use crate::menu::{Go, MenuOptions};
    use crate::sound::Trigger;

    /// w2-uisound review: the window's per-frame hook plays the menu's interface sounds through the director (the
    /// user's silent lobby: the cues were queued and never handed over). A message box click without a ROM: the
    /// cue has no file, yet the start is decided and logged.
    #[test]
    fn menu_music_plays_the_cues() {
        let mut menu = Menu::new(MenuOptions {
            auth: "127.0.0.1:1".parse().unwrap(),
            user: "HookUser".into(),
            link: LinkOptions::new("w2-uisound-hook"),
            rom: None,
            font: Some((crate::testkit::synth_font(), 0)),
            width: 1024,
            height: 768,
        });
        menu.open(Go::Lobby);
        let mut sound = Some(SoundDirector::open(None));
        menu_music(&mut menu, &mut sound);
        let before = sound.as_ref().unwrap().starts_of("interface").len();
        let b = Popup::ok("x".into(), BoxTag::None);
        let ok = b.buttons()[0];
        let ok = [
            ok[0] as f32 + ok[2] as f32 / 2.0,
            ok[1] as f32 + ok[3] as f32 / 2.0,
        ];
        menu.show_popup(b);
        menu.input(MenuInput::Press(ok));
        menu.input(MenuInput::Release(ok));
        assert!(
            !menu.pending_sounds().is_empty(),
            "the click asked for a sound"
        );
        menu_music(&mut menu, &mut sound);
        assert!(menu.pending_sounds().is_empty(), "the frame took the cues");
        let sd = sound.as_ref().unwrap();
        let starts = sd.starts_of("interface");
        assert_eq!(starts.len(), before + 1, "{:?}", sd.log);
        assert!(matches!(
            starts.last().unwrap().trigger,
            Trigger::Interface { what: "select" }
        ));
        // `--no-sound`: the frame still takes (drops) the cues
        menu.show_popup(Popup::ok("y".into(), BoxTag::None));
        menu.input(MenuInput::Press(ok));
        menu.input(MenuInput::Release(ok));
        let mut none = None;
        menu_music(&mut menu, &mut none);
        assert!(menu.pending_sounds().is_empty());
    }
}
