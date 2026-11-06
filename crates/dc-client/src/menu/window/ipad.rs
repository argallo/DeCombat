//! iPadOS (CLI-1604): the menu run on UIKit's one event loop. The desktop runs the menu's screens, the loading page
//! and every match as turns of re-entrant event loops (`run_app_on_demand`, [`super::run_menu_window`]); UIKit's
//! loop is entered once and never returns, and its `exit` is ignored. The driver here is one application handler
//! with the run's state and a stage (the menu's screens, the loading page, a match): it forwards the loop's events
//! to the stage's own handler ([`MenuApp`], the match's [`MatchTurn`]), reads after every event whether the stage
//! asked its turn to end (where the desktop's loop would have returned) and makes the desktop's step between two
//! turns (game start, the join's result, the match's end). The keyboard and the mouse come from GameController
//! through [`crate::ios::bridge`], fed to the stage once per frame as the winit events of the desktop.
//!
//! The run's state lives as long as the process (a UIKit app never leaves its loop): the menu, the window options,
//! the director, the confirmed options and the pilot are leaked once at the start, so the stages borrow them as the
//! desktop's turns do.
// DC-DEFAULT: CLI-1604 (proposed)
// PUBLIC: winit 0.30 docs (EventLoop::run_app on iOS never returns; ActiveEventLoop::exit is ignored on iOS;
// Resumed / Suspended follow applicationDidBecomeActive / applicationWillResignActive)

use super::*;
use crate::ios::bridge::{Bridge, Synth, View};
use crate::menu::pilot::Pilot;
use crate::settings::Settings;
use crate::window::MatchTurn;
use dc_ios_hid::{Hid, HidEvent};
use std::sync::Arc;
use winit::event::{DeviceEvent, DeviceId};
use winit::window::Window;

/// What the turns borrow (the desktop's `run_menu_window` locals).
struct Parked {
    menu: &'static mut Menu,
    wopts: &'static WindowOptions,
    sound: &'static mut Option<SoundDirector>,
    changed: &'static mut Option<Settings>,
    pilot: &'static mut Option<Pilot>,
}

enum Stage {
    /// Before the first `resumed` (UIKit has not finished launching: no window may be made yet).
    Start(Parked),
    /// A menu turn: the screens (`play` `None`) or the loading page of the game the menu started.
    Menu {
        app: Box<MenuApp<'static>>,
        play: Option<Play>,
    },
    /// A match in the menu's window.
    Match {
        turn: Box<MatchTurn>,
        painter: Option<MenuPainter>,
        parked: Parked,
        handle: NetHandle,
        handoff: LobbyHandoff,
    },
    /// The run failed where the desktop's would have returned its error: nothing more is drawn.
    Stopped,
}

struct Shell {
    base: NetOptions,
    stage: Stage,
    hid: Option<Hid>,
    bridge: Bridge,
    /// Frame and tick counts for the input log.
    redraws: u64,
    ticks: u64,
    /// When winit last delivered a RedrawRequested (its chain, once it runs, paces the frames itself).
    last_redraw: Option<std::time::Instant>,
    /// The pointer lock the driver last asked for.
    locked: Option<bool>,
    /// The display rate the driver last asked for.
    rate: Option<u32>,
    started: std::time::Instant,
    /// The display link firing the last frame was drawn for, and when that frame was drawn.
    last_link: u64,
    last_frame: Option<std::time::Instant>,
    /// When the pointer lock was last asserted again (iPadOS drops it on its own, Escape among the causes).
    last_lock: Option<std::time::Instant>,
    /// Frame time statistics of the input log: sum and maximum (ms) since the last print.
    frame_ms: (f64, f64),
    /// The match's capture state at the last tick (`None` outside a match).
    last_captured: Option<bool>,
}

/// The parts of a menu turn that ended: what it borrowed and its outcome (the desktop's `run_turn` result).
fn park(mut app: MenuApp<'static>) -> (Parked, Result<Turn, ClientError>) {
    let turn = match app.error.take() {
        Some(e) => Err(e),
        None => Ok(Turn {
            live: app.live.take(),
            closed: app.closed,
            joined: app.joining.take().and_then(|j| j.result),
        }),
    };
    let MenuApp {
        menu,
        opts,
        sound,
        changed,
        pilot,
        ..
    } = app;
    (
        Parked {
            menu,
            wopts: opts,
            sound,
            changed,
            pilot,
        },
        turn,
    )
}

/// A new menu turn (the screens, or the loading page with `joining`), started as the desktop's run starts it.
fn menu_turn(
    el: &ActiveEventLoop,
    p: Parked,
    live: Option<Live>,
    joining: Option<Joining>,
    play: Option<Play>,
) -> Stage {
    let mut app = MenuApp::new(p.menu, p.wopts, live, p.sound, p.changed, p.pilot, joining);
    app.resumed(el);
    Stage::Menu {
        app: Box::new(app),
        play,
    }
}

/// The desktop's end of `run_menu_window`: the director's close line and the screens shown; the app process ends
/// (the menu's Quit, as on the desktop).
fn quit(p: Parked) -> Stage {
    if let Some(d) = p.sound.as_mut() {
        println!("dc-client menu: {}", d.close_output());
    }
    println!("dc-client menu: screens {}", p.menu.trail.join(" -> "));
    dc_hangdump::crash::cleanup();
    std::process::exit(0);
}

impl Shell {
    fn window(&self) -> Option<&Arc<Window>> {
        match &self.stage {
            Stage::Menu { app, .. } => app.live.as_ref().map(|l| &l.host.window),
            Stage::Match { turn, .. } => turn.window(),
            _ => None,
        }
    }

    /// The view the bridge maps the pointer into (`None`: no window now).
    fn view(&self) -> Option<View> {
        let w = self.window()?;
        let (s, o) = (w.inner_size(), w.outer_size());
        Some(View {
            surface: (s.width, s.height),
            outer: (o.width, o.height),
            scale: w.scale_factor(),
            captured: matches!(&self.stage, Stage::Match { turn, .. } if turn.captured()),
        })
    }

    fn forward_window(&mut self, el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
        match &mut self.stage {
            Stage::Menu { app, .. } => app.window_event(el, id, ev),
            Stage::Match { turn, .. } => turn.handler().window_event(el, id, ev),
            _ => {}
        }
    }

    /// The bridge's inputs to the stage, each followed by the turn check (a key may end the turn; the next input
    /// goes to the next stage, as winit hands the rest of a frame to the desktop's next run).
    fn deliver(&mut self, el: &ActiveEventLoop, inputs: Vec<Synth>) {
        for s in inputs {
            let Some(id) = self.window().map(|w| w.id()) else {
                if input_log() {
                    eprintln!("dc-client ios input: {s:?} dropped (no window)");
                }
                return;
            };
            if input_log() && !matches!(s, Synth::Motion(..)) {
                let stage = match &self.stage {
                    Stage::Menu { .. } => "menu",
                    Stage::Match { .. } => "match",
                    _ => "other",
                };
                eprintln!("dc-client ios input: deliver {s:?} to {stage}");
            }
            match s {
                Synth::Key(k) => match &mut self.stage {
                    Stage::Menu { app, .. } => app.key_in(el, k),
                    Stage::Match { turn, .. } => turn.key(el, k),
                    _ => {}
                },
                s => {
                    if let Some(ev) = s.device_event() {
                        if let Stage::Match { turn, .. } = &mut self.stage {
                            turn.handler().device_event(el, DeviceId::dummy(), ev);
                        }
                    }
                    if let Some(ev) = s.window_event() {
                        self.forward_window(el, id, ev);
                    }
                }
            }
            self.advance(el);
        }
    }

    /// The step between two turns, as often as turns end (the desktop's `run_menu_window` loop body).
    fn advance(&mut self, el: &ActiveEventLoop) {
        loop {
            let ended = match &self.stage {
                Stage::Menu { app, .. } => app.ended,
                Stage::Match { turn, .. } => turn.ended(),
                _ => false,
            };
            if !ended {
                return;
            }
            let stage = std::mem::replace(&mut self.stage, Stage::Stopped);
            self.stage = match stage {
                Stage::Menu { mut app, play } => {
                    app.exiting(el);
                    let (p, turn) = park(*app);
                    match (turn, play) {
                        (Err(e), play) => {
                            eprintln!("dc-client menu: {e}");
                            drop(play);
                            el.set_control_flow(ControlFlow::Wait);
                            Stage::Stopped
                        }
                        (Ok(t), None) => self.after_menu(el, p, t),
                        (Ok(t), Some(play)) => self.after_loading(el, p, t, play),
                    }
                }
                Stage::Match {
                    turn,
                    painter,
                    parked: p,
                    handle,
                    handoff,
                } => {
                    let (r, back, host) = turn.finish(el);
                    *p.sound = back;
                    // CLI-1280: an Alt+Enter in the match is saved now (also when the player quits from it)
                    p.menu.store_display_note();
                    let live = return_live(host, painter);
                    let r = r.map(|r| print_exit_report(&r));
                    finish_play(p.menu, &handle, &handoff, false, &r);
                    if crate::steps::quit_requested() {
                        quit(p)
                    } else {
                        menu_turn(el, p, live, None, None)
                    }
                }
                s => s,
            };
        }
    }

    /// The menu's screens ended: the player quit, or the menu started a game (the loading page's turn).
    fn after_menu(&mut self, el: &ActiveEventLoop, p: Parked, t: Turn) -> Stage {
        if t.closed {
            return quit(p);
        }
        match p.menu.take_outcome() {
            Some(MenuOutcome::Play {
                server,
                lobby,
                mode,
            }) => {
                let (play, joining) = begin_play(p.menu, p.sound, &self.base, server, *lobby, mode);
                menu_turn(el, p, t.live, Some(joining), Some(play))
            }
            Some(MenuOutcome::Quit) | None => quit(p),
        }
    }

    /// The loading page ended: the match takes the window, or the join failed (the menu again, with its box).
    fn after_loading(&mut self, el: &ActiveEventLoop, p: Parked, t: Turn, play: Play) -> Stage {
        let Play {
            net,
            handle,
            handoff,
        } = play;
        let quit_now = t.closed;
        let joined = t
            .joined
            .unwrap_or_else(|| Err(ClientError::Net("the client quit while loading".into())));
        // a join that never reached a world is the 2009 time-out case (the box 0xbd0), unless the player quit
        let join_failed = joined.is_err() && !quit_now;
        let r = match joined {
            Ok(w) if !quit_now => {
                println!("dc-client menu: server announced {w:?}");
                // the match takes the window over and hands it back (no window swap); the painter stays
                let (host, painter) = lend_live(t.live);
                let reticle = crate::menu::colorbox::match_reticle(
                    p.menu.cat.rom.as_deref(),
                    &p.menu.data.store,
                );
                let mopts = match_options(
                    p.wopts,
                    p.changed,
                    p.menu.mod_recipe(),
                    &p.menu.data.room,
                    reticle,
                );
                let mut turn = Box::new(MatchTurn::new(Box::new(net), mopts, p.sound.take(), host));
                turn.handler().resumed(el);
                return Stage::Match {
                    turn,
                    painter,
                    parked: p,
                    handle,
                    handoff,
                };
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
        finish_play(p.menu, &handle, &handoff, join_failed, &r);
        if quit_now || crate::steps::quit_requested() {
            quit(p)
        } else {
            menu_turn(el, p, t.live, None, None)
        }
    }
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        el.set_control_flow(wake_in(FRAME_FALLBACK));
        match std::mem::replace(&mut self.stage, Stage::Stopped) {
            Stage::Start(p) => {
                crate::ios::set_cursor_size(p.menu.cat.cursor_size());
                self.stage = menu_turn(el, p, None, None, None)
            }
            s => {
                // the app is active again: the stage's own resume (its window is drawn again)
                self.stage = s;
                match &mut self.stage {
                    Stage::Menu { app, .. } => app.resumed(el),
                    Stage::Match { turn, .. } => turn.handler().resumed(el),
                    _ => {}
                }
            }
        }
        // GameController's handlers once the window exists (dc-ios-hid's rule)
        if self.hid.is_none() && self.window().is_some() {
            self.hid = Some(Hid::start());
            eprintln!("dc-client ios: keyboard and mouse input started");
        }
        self.advance(el);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if input_log() {
            match &event {
                WindowEvent::RedrawRequested => {}
                WindowEvent::CursorMoved { .. } => {}
                e => eprintln!("dc-client ios input: window {e:?}"),
            }
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            // the driver draws every frame from its tick (below); winit's own redraw events would draw twice
            self.last_redraw = Some(std::time::Instant::now());
            return;
        }
        if let WindowEvent::Touch(t) = &event {
            // with the pointer locked, iPadOS still reports a mouse click as a touch, at a fixed point: the
            // GameController button is the click then, and the touch would snap the cursor (playtest report
            // 2026-10-31 "clicking always brings the cursor back to an origin point")
            if self.locked == Some(true) {
                return;
            }
            if let Some(v) = self.view() {
                let inputs = self
                    .bridge
                    .touch(t.id, t.phase, t.location.x, t.location.y, &v);
                self.deliver(el, inputs);
            }
            return;
        }
        self.forward_window(el, id, event);
        self.advance(el);
    }

    fn device_event(&mut self, el: &ActiveEventLoop, id: DeviceId, event: DeviceEvent) {
        if let Stage::Match { turn, .. } = &mut self.stage {
            turn.handler().device_event(el, id, event);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.ticks += 1;
        if input_log() && (self.ticks <= 3 || self.ticks % 600 == 0) {
            eprintln!(
                "dc-client ios input: tick {} (redraws {})",
                self.ticks, self.redraws
            );
        }
        let events = self.hid.as_mut().map(|h| h.poll()).unwrap_or_default();
        for e in events {
            match e {
                HidEvent::Keyboard(on) => eprintln!(
                    "dc-client ios: keyboard {}",
                    if on { "connected" } else { "gone" }
                ),
                HidEvent::Mouse(on) => eprintln!(
                    "dc-client ios: mouse {}",
                    if on { "connected" } else { "gone" }
                ),
                e => {
                    if input_log() && !matches!(e, HidEvent::MouseMove { .. }) {
                        eprintln!("dc-client ios input: hid {e:?}");
                    }
                    if let Some(v) = self.view() {
                        let inputs = self.bridge.hid(e, &v);
                        if input_log() && !matches!(e, HidEvent::MouseMove { .. }) {
                            eprintln!("dc-client ios input: -> {inputs:?}");
                        }
                        self.deliver(el, inputs);
                    } else if input_log() {
                        eprintln!("dc-client ios input: dropped (no window yet)");
                    }
                }
            }
        }
        match &mut self.stage {
            Stage::Menu { app, .. } => app.about_to_wait(el),
            Stage::Match { turn, .. } => turn.handler().about_to_wait(el),
            _ => {}
        }
        // winit's iOS backend does not deliver the stages' redraw chain reliably (OBSERVED 2026-10-31 on the
        // iPad: the request made while the window opened was dropped and the chain never started; re-armed, it
        // came back at an uneven rate), so the driver raises the stage's RedrawRequested from every tick and
        // ignores winit's own: the surface's Fifo present paces the loop at the display's rate
        // DC-DEFAULT: CLI-1604 (amended) the iOS driver draws every frame from its tick
        // one frame per display-link firing: a frame per tick blocked the main thread in the vsync wait, the link
        // fired at 45 Hz and iPadOS kept the panel at 60; with the frames following the link the thread is free
        // between them. Without a firing for 50 ms (the link not running) the frame is drawn anyway
        let link = dc_ios_hid::link_ticks();
        // DC_IOS_NO_FRAMES (decombat.toml `debug_frames = false`): no frame at all, to measure the display link alone
        if no_frames() {
            if input_log() && self.ticks % 1200 == 0 {
                eprintln!(
                    "dc-client ios input: no frames; display link firings {} at {:.1} s",
                    link,
                    self.started.elapsed().as_secs_f64()
                );
            }
            return;
        }
        let due = link != self.last_link
            || self
                .last_frame
                .is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(50));
        if !due {
            return;
        }
        self.last_link = link;
        self.last_frame = Some(std::time::Instant::now());
        if let Some(id) = self.window().map(|w| w.id()) {
            self.redraws += 1;
            if input_log() && (self.redraws <= 3 || self.redraws % 600 == 0) {
                eprintln!(
                    "dc-client ios input: frame {} (ticks {}, display link firings {}, {:.1} s; frame avg {:.2} ms max {:.2} ms)",
                    self.redraws,
                    self.ticks,
                    dc_ios_hid::link_ticks(),
                    self.started.elapsed().as_secs_f64(),
                    self.frame_ms.0 / 600.0,
                    self.frame_ms.1
                );
                self.frame_ms = (0.0, 0.0);
            }
            let t0 = std::time::Instant::now();
            self.forward_window(el, id, WindowEvent::RedrawRequested);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            self.frame_ms.0 += ms;
            self.frame_ms.1 = self.frame_ms.1.max(ms);
            self.advance(el);
        }
        // the pointer lock: on for the whole run once the window exists. The menu's ROM cursor and, in a match
        // with the mouse free (the leave box, the report form), the HUD's software cursor (`App::soft_cursor`)
        // follow the raw mouse, as the desktop hides the system cursor; two pointers drifted apart before, and
        // a lock released mid-match showed no pointer at all and came back only with the app (playtest reports
        // 2026-10-31)
        // DC-DEFAULT: CLI-1604 (amended)
        let want = !matches!(self.stage, Stage::Start(_) | Stage::Stopped) && self.window().is_some();
        if self.locked != Some(want) {
            self.locked = Some(want);
            self.last_lock = Some(std::time::Instant::now());
            dc_ios_hid::set_pointer_locked(want);
            // the system pointer hidden over the view whatever the lock's fate (playtest report: the lock
            // left the system pointer visible beside the game's own cursor)
            dc_ios_hid::hide_pointer(want);
            if input_log() {
                eprintln!("dc-client ios input: pointer lock {want}");
            }
        }
        // iPadOS lets the lock go by itself when the match frees the mouse (the Escape: the system pointer came
        // back over the leave box, playtest report) and re-reads the flag only on a change: at every change
        // of the match's capture the flag is toggled off and on again (`UIWindowScene.pointerLockState` reads
        // unlocked at all times here, so it is no guide: OBSERVED 2026-10-31)
        let captured = match &self.stage {
            Stage::Match { turn, .. } => Some(turn.captured()),
            _ => None,
        };
        if want && captured != self.last_captured {
            self.last_captured = captured;
            if captured.is_some() {
                dc_ios_hid::relock();
                if input_log() {
                    eprintln!(
                        "dc-client ios input: capture {captured:?}: pointer lock asked again (UIKit says locked {})",
                        dc_ios_hid::pointer_is_locked()
                    );
                }
            }
        }
        // the display rate: a ProMotion iPad stays at 60 Hz unless the app asks for more; the frame-rate setting's
        // 60 asks for 60, everything else (VSync = the display's rate, unlimited, the higher caps) for 120
        // DC-DEFAULT: CLI-1604 (amended; playtest report "locked to 60fps even when I change to unlimited")
        let hz = if crate::pacer::setting_cap() == 60 { 60 } else { 120 };
        if self.rate != Some(hz) {
            self.rate = Some(hz);
            dc_ios_hid::set_frame_rate(hz);
            if input_log() {
                eprintln!("dc-client ios input: display rate {hz} Hz asked");
            }
        }
        {
        }
        self.advance(el);
        // the main thread sleeps in the run loop between frames (ControlFlow::Wait): the display link's firing wakes
        // it (a frame per firing, above), as do touches; without a firing the fallback wakes it. A thread that spins
        // (Poll) never services the link, so the system never saw a 120 Hz client and kept the panel at 60
        // (OBSERVED 2026-10-31: 0 firings without frames, 60 with the frames blocking in the present)
        // DC-DEFAULT: CLI-1604 (amended) the iOS driver waits in the run loop and draws per display-link firing
        el.set_control_flow(wake_in(FRAME_FALLBACK));
    }

    fn suspended(&mut self, _el: &ActiveEventLoop) {
        // in the background UIKit runs no frames: nothing to watch until the app is active again (the next frame sets
        // the phase back)
        crate::watchdog::phase(crate::watchdog::Phase::Idle);
    }
}

/// Runs the menu and its matches on UIKit's event loop (never returns once the loop runs).
pub(crate) fn run(menu: Menu, wopts: WindowOptions, base: NetOptions) -> Result<(), ClientError> {
    let el = crate::window::new_event_loop()?;
    el.set_control_flow(ControlFlow::Poll);
    // one sound director for the menu and its matches: the music's track and clock carry over (AUDIO-1)
    let sound = open_sound(&wopts, &base);
    let parked = Parked {
        menu: Box::leak(Box::new(menu)),
        wopts: Box::leak(Box::new(wopts)),
        sound: Box::leak(Box::new(sound)),
        changed: Box::leak(Box::new(None)),
        pilot: Box::leak(Box::new(None)),
    };
    let mut shell = Shell {
        base,
        stage: Stage::Start(parked),
        hid: None,
        bridge: Bridge::default(),
        redraws: 0,
        ticks: 0,
        last_redraw: None,
        locked: None,
        rate: None,
        started: std::time::Instant::now(),
        last_link: 0,
        last_frame: None,
        last_lock: None,
        frame_ms: (0.0, 0.0),
        last_captured: None,
    };
    el.run_app(&mut shell)
        .map_err(|e| ClientError::Window(e.to_string()))
}

/// `debug_input = true` in decombat.toml (the entry sets `DC_IOS_INPUT_LOG`): input events on the console.
fn input_log() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("DC_IOS_INPUT_LOG").is_some())
}

/// `debug_frames = false` in decombat.toml (the entry sets `DC_IOS_NO_FRAMES`): the driver draws nothing.
fn no_frames() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("DC_IOS_NO_FRAMES").is_some())
}

/// The longest the driver sleeps without a display-link firing before it draws a frame anyway.
const FRAME_FALLBACK: std::time::Duration = std::time::Duration::from_millis(50);

fn wake_in(d: std::time::Duration) -> ControlFlow {
    ControlFlow::WaitUntil(std::time::Instant::now() + d)
}
