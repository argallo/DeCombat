//! Window mode (feature `window`): one borderless window on the primary monitor, a wgpu surface, mouse look
//! and keyboard movement. Never exclusive full screen and never a display-mode change (RENDER_2009.md §2.1
//! DeCombat note: "full screen" is a borderless window).
//!
//! Cursor rule (DeCombat client policy, PROPOSED CLI-4): the cursor is confined to the window and hidden only
//! while the window has focus and mouse look is active; Escape or focus loss releases it, a click in the
//! focused window takes it again, Escape with the cursor already released opens the leave-the-match box (Yes
//! closes the window; m36, [`crate::loading::LeaveBox`]). Every exit path (the leave box, close button, the
//! `--window-secs` deadline) releases the cursor first and leaves through the event loop's normal exit. In the
//! menu's run the match plays in the menu's window and hands it back at its end ([`run_window_in_host`]).

use crate::characters::ModelScene;
use crate::debug_draw::DebugBoxes;
use crate::error::ClientError;
use crate::frame::{apply_view_set, overlay_lines, FpsMeter, ViewSettings};
use crate::headless::box_list;
use crate::headless::shot_aim;
use crate::input::{
    Action, DeviceInput, InputSource, KeyboardMouse, ScriptedInput, ShellKey, ShootTest, WalkTest,
};
use crate::look::{LookParams, ViewAngles};
use crate::overlay::{load_rom_font, OverlayFont, OverlayRenderer};
use crate::player::LocalPlayer;
use crate::update::LocalUpdate;
use crate::world::{ArmsFrame, GpuTarget, LoadedWorld, WorldSource};
use dc_render::{
    AdapterChoice, FrameOptions, FrameTarget, GpuContext, RenderSettings, WorldRenderer,
};
use dc_sim::MovementParams;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{
    DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey, SmolStr};
#[cfg(target_os = "macos")]
use winit::platform::macos::{EventLoopBuilderExtMacOS, WindowExtMacOS};
#[cfg(not(target_os = "ios"))]
use winit::window::CursorGrabMode;
use winit::window::{Window, WindowId, WindowLevel};

/// True on macOS: the window, cursor and quit rules of CLI-1284 / CLI-1285 apply.
pub const MACOS: bool = cfg!(target_os = "macos");

/// True on iPadOS: the window is the screen (no size, place or look of ours), the pointer lock replaces the cursor
/// grab and one event loop runs the whole process (`crate::ios`, CLI-1604).
pub const IOS: bool = cfg!(target_os = "ios");

/// The match renderer's settings: the defaults with the Graphic tab's anti-aliasing count (CLI-1594), lowered to
/// the highest count the adapter supports for the surface format and the depth format (1 = off).
// DC-DEFAULT: CLI-1594 (proposed) the published count, clamped to the device's; taken at the match window's start
pub fn render_settings(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> RenderSettings {
    let want = crate::settings::antialias().max(1);
    let supported = dc_render::supported_msaa(adapter, format);
    let msaa = supported
        .iter()
        .copied()
        .filter(|&n| n <= want)
        .max()
        .unwrap_or(1);
    if msaa != want {
        println!("dc-client: anti-aliasing {want}x is not available on this device; using {msaa}x");
    }
    RenderSettings {
        msaa,
        ..RenderSettings::default()
    }
}

/// Whether the windows present at the display's rate (Fifo): `--vsync` asks for it everywhere, `--no-vsync`
/// refuses it everywhere, else the Graphic tab's frame-rate setting decides (CLI-1595: its VSync entry; the
/// setting's default is VSync on macOS, CLI-1575: Immediate drew 300..500 frames/s on a 120 Hz panel at 55 % of a
/// core with the fans up, Fifo 120 frames/s at 38 %; unlimited elsewhere). Read every frame: a change reconfigures
/// the surface in place.
// DC-DEFAULT: CLI-1575 (proposed; amended 2026-10-31 by CLI-1595: the setting's VSync entry, the macOS default)
pub fn wants_fifo(vsync_flag: bool, no_vsync_flag: bool) -> bool {
    vsync_flag || (!no_vsync_flag && crate::settings::fps_vsync())
}

/// The present mode for `wants_fifo` among `caps`: Immediate when wanted and offered, else Fifo.
pub fn present_mode_for(fifo: bool, caps: &wgpu::SurfaceCapabilities) -> wgpu::PresentMode {
    if !fifo && caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
        wgpu::PresentMode::Immediate
    } else {
        wgpu::PresentMode::Fifo
    }
}

/// Window-mode options.
#[derive(Debug, Clone)]
pub struct WindowOptions {
    /// `--size WxH` without `--windowed` / `--fullscreen`: the full-screen look is this smaller centred borderless
    /// window (physical pixels; the test tools' window). `None`: the full-screen look fills the monitor.
    // DC-DEFAULT: CLI-111 (proposed) the default window fills the chosen monitor exactly
    pub size: Option<(u32, u32)>,
    /// The windowed look's inner size (physical pixels): `--size`, else the profile's resolution, else `None`
    /// ([`BORDERED_SIZE`]); limited to the monitor by [`windowed_size`].
    // DC-DEFAULT: CLI-1280 (proposed)
    pub windowed_size: Option<(u32, u32)>,
    /// `--windowed` / `--fullscreen`: the look the first window opens with (`None`: the options' window mode).
    // DC-DEFAULT: CLI-1280 (proposed)
    pub start_look: Option<WindowLook>,
    /// `--size` was given: the saved resolution does not replace `windowed_size` in this run (CLI-1282).
    pub keep_size: bool,
    /// `--monitor N`: 0 = the primary monitor, 1.. = the other monitors in the system's order.
    pub monitor: usize,
    /// Close by itself after this many seconds (`None`: until the user closes it).
    pub seconds: Option<f32>,
    /// Drive the player with the scripted sequence instead of the keyboard (mouse grab still applies).
    pub scripted: bool,
    /// Drive the player with the repeating walk ([`WalkTest`], `--walk-test`) instead of the keyboard.
    pub walk_test: bool,
    /// Drive the player with the scripted shooter ([`ShootTest`], `--shoot-test`; with `walk_test` it also walks).
    pub shoot_test: bool,
    /// Draw the boxes of remote players also where a model is drawn (`--debug-boxes`).
    pub debug_boxes: bool,
    pub tick_hz: u32,
    pub spawn: usize,
    pub view: ViewSettings,
    pub params: MovementParams,
    pub look: LookParams,
    /// `--vsync`: wait for vertical blank (Fifo) whatever the setting; `--no-vsync`: never (the 2009 engine
    /// presents immediately). Neither: the frame-rate setting (CLI-1595).
    // SPEC: RENDER_2009.md §2.2 (presentation interval IMMEDIATE), §2.6
    pub vsync: bool,
    pub no_vsync: bool,
    pub font_rom: Option<PathBuf>,
    /// Which graphics backends the window may use (`--adapter`): `Backends` restricts the instance, `Auto` and
    /// `Fallback` keep the default set (the surface adapter request falls back to the software adapter itself).
    pub adapter: AdapterChoice,
    /// CLI-1852: `adapter` is only the first attempt (the saved Renderer choice): when no adapter of those backends
    /// can present to the window, the default backends are tried. `--adapter` leaves it false (exclusive).
    pub adapter_first: bool,
    /// The in-match HUD (Tab holds the scoreboard).
    pub hud: crate::hud::HudChoice,
    /// Draw the debug text panel.
    pub debug_panel: bool,
    /// Test runs: also show the scoreboard from `.0` to `.1` seconds after the window opened (`--show-board-at`).
    pub board_at: Option<(f32, f32)>,
    /// The sounds (director on / off, JSONL log of every start).
    pub sound: crate::sound::SoundOptions,
    /// Where the mix goes (`--audio device|null`) and the volumes (`--volume`, `--music-volume`).
    pub audio: crate::sound::AudioConfig,
    /// CLI-1619: the joined server's mod set as a recipe (`None`: vanilla); the match loads its own host from it.
    pub mods: Option<crate::mods::HostRecipe>,
    /// CLI-1916: the field-of-view range the joined server published (modded and curated entries; `None`: vanilla, an
    /// older lobby, no server): the player's Field of View setting applies inside it, `view` stays as it is without.
    pub client_fov: Option<(u32, u32)>,
    /// LG-274: the room's Hacks setting (`dc_wire::roomhacks` bits; `None`: the lobby offered none). A room without
    /// the wallhacks bit gets no `render.entity_visibility` bridge (CLI-1620), as its server culls.
    pub room_hacks: Option<u32>,
    /// LG-305 / CLI-1639: a Gun Game room's ladder (empty: another mode).
    pub gun_ladder: Vec<u16>,
    /// use-items-sweep: the reticle item in use (the mark's shape bits, `0xAARRGGBB`; CLI-1878).
    pub reticle: Option<(u16, u32)>,
    /// CLI-1907: the menu's settings and key table at the match start (the options over the match start from them;
    /// `None`: a match without the menu, the defaults).
    pub prefs: Option<MatchPrefs>,
}

/// CLI-1907: the menu's settings and the 2009 bindable command table, handed to the match for its options box.
#[derive(Debug, Clone, Default)]
pub struct MatchPrefs {
    pub settings: crate::settings::Settings,
    pub commands: crate::settings::CommandTable,
}

/// LG-274 / CLI-1627: whether a match of a room with Hacks setting `hacks` may run the x-ray render bridge: a room
/// without a Hacks choice (`None`) leaves it to the set (CLI-1620), a room with one needs the wallhacks bit.
pub fn room_wallhacks(hacks: Option<u32>) -> bool {
    hacks.is_none_or(|v| v & dc_wire::roomhacks::WALLHACKS != 0)
}

/// What a window run did.
#[derive(Debug, Clone, Default)]
pub struct WindowReport {
    pub adapter: String,
    pub present_mode: String,
    pub world: String,
    pub font: Option<String>,
    pub frames: u64,
    pub seconds: f64,
    pub mean_fps: f64,
    pub min_frame_ms: f64,
    pub max_frame_ms: f64,
    pub ticks: u64,
    pub start: [f32; 3],
    pub end: [f32; 3],
    /// Every grab / release with the time since start (seconds).
    pub cursor_log: Vec<(f64, String)>,
    /// The cursor was released when the loop exited.
    pub released_on_exit: bool,
    pub exit_reason: String,
    /// Server corrections applied, frames with remote players, remote boxes drawn.
    pub corrections: u64,
    pub remote_frames: u64,
    pub boxes_drawn: u64,
    /// Model instances drawn, remote characters drawn with a model, frames with the own weapon model.
    pub models_drawn: u64,
    pub characters_drawn: u64,
    pub view_weapon_frames: u64,
    pub models_note: Option<String>,
    /// The server's player state at the end and the last status lines (network sessions).
    pub life_state: Option<u8>,
    pub hud: Vec<String>,
    /// Where the HUD layout came from, HUD inputs applied, HUD images composed, frames with the scoreboard.
    pub hud_layout: Option<String>,
    pub hud_events: u64,
    pub hud_composed: u64,
    pub board_frames: u64,
    /// The sound output: the open note and the last status line (device statistics or the null backend's time).
    pub audio_out: Vec<String>,
    /// Seconds after the start when the server ended the session (network sessions).
    pub session_over_s: Option<f64>,
    /// The frame-time distribution of the whole run (client-fps: median, p95, p99, worst, hitch counts).
    pub frame_times: crate::frametimes::FrameTimes,
    /// CLI-1576: the display period (ms) the frame clock was paced to (blank-synced presentation), and the raw CPU
    /// frames under half / over one and a half periods.
    pub vsync_period_ms: Option<f64>,
    pub pace_short: u64,
    pub pace_long: u64,
    /// The GPU device was lost and the window left the match (fix-hang, CLI-460).
    pub gpu_reset: bool,
    /// GPU device losses the window recovered from inside the match (gpu-recover).
    pub gpu_recoveries: u64,
}

/// The exit report of a window run, one string per printed line. Every way a game window ends prints it: the
/// window mode's own run and each match the menu starts (client-fps; the menu path dropped it before).
// DC-DEFAULT: CLI-390 (proposed) the exit report, see DC_DEFAULTS.md
pub fn exit_report_lines(r: &WindowReport) -> Vec<String> {
    let mut v = vec![format!(
        "dc-client window: adapter {} ({}), {} frames in {:.2} s = {:.1} fps (frame {:.2}..{:.2} ms), {} ticks",
        r.adapter, r.present_mode, r.frames, r.seconds, r.mean_fps, r.min_frame_ms, r.max_frame_ms, r.ticks
    )];
    v.push(format!("  {}", r.frame_times.line()));
    if let Some(p) = r.vsync_period_ms {
        v.push(format!(
            "  frame clock: paced to the {:.1} Hz blank (CLI-1576); raw CPU frames under half a period {}, over 1.5 periods {}",
            1000.0 / p,
            r.pace_short,
            r.pace_long
        ));
    }
    v.push(format!("  start {:?} end {:?}", r.start, r.end));
    v.push(format!(
        "  HUD {} | inputs {} | composed {} | scoreboard frames {}",
        r.hud_layout.as_deref().unwrap_or("off"),
        r.hud_events,
        r.hud_composed,
        r.board_frames
    ));
    for l in &r.audio_out {
        v.push(format!("  {l}"));
    }
    for (t, what) in &r.cursor_log {
        v.push(format!("  cursor {t:7.3} s: {what}"));
    }
    v.push(format!(
        "  exit: {}; cursor released on exit: {}",
        r.exit_reason, r.released_on_exit
    ));
    v.push(format!(
        "  server: corrections {} remote frames {} boxes drawn {} state {:?}",
        r.corrections, r.remote_frames, r.boxes_drawn, r.life_state
    ));
    v.push(format!(
        "  models: instances drawn {} characters drawn {} own weapon frames {} {}",
        r.models_drawn,
        r.characters_drawn,
        r.view_weapon_frames,
        r.models_note.as_deref().unwrap_or("")
    ));
    for l in &r.hud {
        v.push(format!("  | {l}"));
    }
    v
}

/// Weapon keys: R reloads (the fire button is the left mouse button while the cursor is captured, the wheel steps
/// through the slots, the zoom is the right button or Z); the number keys are class keys ([`shell_keys_for`]) that
/// cycle the weapons of their order class.
// DC-DEFAULT: CLI-36 (amended by w2-scope) weapon key map (the 2009 bindings are ROM configuration, not read)
// EVIDENCE: cshell+0x22e530 (number key k cycles the WeaponOrder class k)
pub fn weapon_input_for(key: KeyCode) -> Option<DeviceInput> {
    // w3-options: the keys come from the player's bindings (settings::bound_input, the defaults without a profile)
    press_input_for(bound_key(key))
}

/// The player's binding of `key` (the published bindings, the defaults before any profile).
pub fn bound_key(key: KeyCode) -> Option<crate::settings::Bound> {
    crate::settings::scan_of_code(key).and_then(crate::settings::bound_input)
}

/// The press-only game input of a bound key: reload, and the use command (the dropped weapon under the player,
/// CLI-675; its key is the Activate binding, CLI-1032).
pub fn press_input_for(bound: Option<crate::settings::Bound>) -> Option<DeviceInput> {
    match bound? {
        crate::settings::Bound::Reload => Some(DeviceInput::Reload),
        crate::settings::Bound::Inspect => Some(DeviceInput::Inspect),
        crate::settings::Bound::Ping => Some(DeviceInput::Ping),
        crate::settings::Bound::Use => Some(DeviceInput::Use),
        _ => None,
    }
}

/// CLI-1032: the held game input of a bound key: the fire command on a keyboard key goes down and up with it.
pub fn held_input_for(bound: Option<crate::settings::Bound>, down: bool) -> Option<DeviceInput> {
    match bound? {
        crate::settings::Bound::Fire => Some(DeviceInput::Fire(down)),
        _ => None,
    }
}

/// CLI-1031: the sensitivity step of `key` (its SensitivityInc / SensitivityDec binding).
pub fn sensitivity_step_for(key: KeyCode) -> Option<i32> {
    match crate::settings::scan_of_code(key).and_then(crate::settings::bound_input)? {
        crate::settings::Bound::Sensitivity(s) => Some(i32::from(s)),
        _ => None,
    }
}

/// Shell-level keys: Enter opens / sends the chat line, Escape cancels it (Escape keeps its cursor / close job),
/// digits 1-9 are class keys 1-9 (the weapon reads them), Page Down / Page Up follow the next /
/// previous mate as an observer, V changes the observer camera, F10 opens the leave box.
// DC-DEFAULT: CLI-411 (staged) shell key map (the 2009 bindings are ROM / profile configuration, not read)
// EVIDENCE: cshell+0x22e530 (class key k), cshell+0x48450 (leave box); the physical keys are ours
pub fn shell_keys_for(key: KeyCode) -> Vec<ShellKey> {
    let digit = |n: u8| vec![ShellKey::ClassKey(n)];
    // CLI-1032: the weapon commands' keys are the class keys 1..6 (PrimaryWeapon .. ObjectiveWeapon); a digit that is
    // bound to another command is that command's
    match bound_key(key) {
        Some(crate::settings::Bound::Slot(n)) => return digit(n),
        Some(_)
            if matches!(
                key,
                KeyCode::Digit1
                    | KeyCode::Digit2
                    | KeyCode::Digit3
                    | KeyCode::Digit4
                    | KeyCode::Digit5
                    | KeyCode::Digit6
            ) =>
        {
            return Vec::new()
        }
        _ => {}
    }
    match key {
        KeyCode::Enter | KeyCode::NumpadEnter => vec![ShellKey::ChatOpen, ShellKey::ChatSend],
        KeyCode::Escape => vec![ShellKey::ChatCancel],
        KeyCode::Digit7 => digit(6),
        KeyCode::Digit8 => digit(7),
        KeyCode::Digit9 => digit(8),
        KeyCode::PageDown => vec![ShellKey::ObserverNext],
        KeyCode::PageUp => vec![ShellKey::ObserverPrev],
        KeyCode::KeyV => vec![ShellKey::ObserverView],
        KeyCode::F10 => vec![ShellKey::LeaveBox],
        _ => Vec::new(),
    }
}

/// The chat input's view of a key: closed, Enter (not Alt+Enter, handled before) opens it for all players and the
/// team chat key for the own team; open, every key press is the box's (Enter sends, Escape closes, Backspace, the
/// key's text). Releases while the box is open are swallowed as well (`None` only when the key is the game's).
// DC-DEFAULT: CLI-1032 (proposed; amends CLI-490) the chat keys are the SendMessage / BroadCastChat / TeamChat bindings
pub fn chat_press_for(
    code: KeyCode,
    down: bool,
    repeat: bool,
    text: Option<&str>,
    open: bool,
) -> Option<crate::hud::chat::ChatPress> {
    use crate::hud::chat::ChatPress as P;
    if !open {
        // CLI-1032: the SendMessage / BroadCastChat keys open the line for all, the TeamChat key for the own team
        return match (bound_key(code), down && !repeat) {
            (Some(crate::settings::Bound::ChatAll), true) => Some(P::Enter),
            (Some(crate::settings::Bound::ChatTeam), true) => Some(P::Team),
            _ => None,
        };
    }
    if !down {
        return None;
    }
    Some(match code {
        KeyCode::Enter | KeyCode::NumpadEnter if !repeat => P::Enter,
        KeyCode::Escape if !repeat => P::Escape,
        KeyCode::Backspace => P::Backspace,
        _ => P::Text(text.unwrap_or_default().to_string()),
    })
}

/// Default key map (PROPOSED client default CLI-1, staged): W / S / A / D, Space jump, Ctrl (either side, by
/// its logical key) or C crouch, Left Shift sprint, Left Alt walk, Caps Lock run lock, Q strafe modifier.
// SPEC: MOVEMENT_2009.md §5.1 (key-to-command bindings are configuration)
pub fn action_for(key: KeyCode, logical: &Key) -> Option<Action> {
    // w3-options: the keys come from the player's bindings (settings::bound_input holds the defaults above until a
    // profile is read); either Ctrl key stays a crouch where it is bound to nothing else
    // DC-DEFAULT: CLI-411 (staged) Z is the zoom key (the right mouse button is the other binding)
    match crate::settings::scan_of_code(key).and_then(crate::settings::bound_input) {
        Some(crate::settings::Bound::Action(a)) => Some(a),
        Some(_) => None,
        None if matches!(logical, Key::Named(NamedKey::Control)) => Some(Action::Crouch),
        None => None,
    }
}

/// The modifier family of a key by its logical key (the right Alt of a layout with AltGr is its own key, not Alt:
/// the platform reports Ctrl and Alt up while it is held).
pub fn modifier_of(logical: &Key) -> Option<crate::input::ModifierKey> {
    use crate::input::ModifierKey as M;
    match logical {
        Key::Named(NamedKey::Control) => Some(M::Ctrl),
        Key::Named(NamedKey::Alt) => Some(M::Alt),
        Key::Named(NamedKey::Shift) => Some(M::Shift),
        Key::Named(NamedKey::Super | NamedKey::Meta) => Some(M::Logo),
        _ => None,
    }
}

/// The game input of a bound key going down / up: the key itself holds the action (an action stays held while any
/// of its keys is down; a modifier key also lets the modifier state release it, CLI-1521).
// DC-DEFAULT: CLI-1520 (proposed) held actions follow their physical keys
pub fn key_input(key: KeyCode, logical: &Key, action: Action, down: bool) -> DeviceInput {
    DeviceInput::Key {
        // the scan code; a key without one shares one id (no unscanned key is bound)
        key: crate::settings::scan_of_code(key).map_or(0x100, u32::from),
        family: modifier_of(logical),
        action,
        down,
    }
}

/// One key press or release as the windows read it: winit's `KeyEvent` (whose platform part cannot be built outside
/// winit), or the same fields made by the iPad's keyboard bridge (`crate::ios`), so both reach one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyIn {
    pub physical: PhysicalKey,
    pub logical: Key,
    /// The text the press types (`None` for a release, as winit reports it).
    pub text: Option<SmolStr>,
    pub down: bool,
    pub repeat: bool,
}

impl From<&KeyEvent> for KeyIn {
    fn from(e: &KeyEvent) -> KeyIn {
        KeyIn {
            physical: e.physical_key,
            logical: e.logical_key.clone(),
            text: e.text.clone(),
            down: e.state == ElementState::Pressed,
            repeat: e.repeat,
        }
    }
}

/// The modifier state the window reports, as the input event that releases the keys whose key-up was lost.
// DC-DEFAULT: CLI-1521 (proposed)
pub fn modifiers_input(m: winit::keyboard::ModifiersState) -> DeviceInput {
    DeviceInput::Modifiers {
        ctrl: m.control_key(),
        alt: m.alt_key(),
        shift: m.shift_key(),
        logo: m.super_key(),
    }
}

/// Where and how large the borderless window is, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowPlan {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Window rectangle for a monitor at `pos` with physical size `mon`: no request fills the monitor exactly
/// (its origin, its full size); a request is clamped to the monitor (and to at least 1x1) and centred.
// DC-DEFAULT: CLI-111 (proposed) borderless window = the monitor's rectangle, `--size` = centred and smaller
pub fn plan_window(requested: Option<(u32, u32)>, pos: (i32, i32), mon: (u32, u32)) -> WindowPlan {
    let mon = (mon.0.max(1), mon.1.max(1));
    let (w, h) = match requested {
        None => mon,
        Some((w, h)) => (w.clamp(1, mon.0), h.clamp(1, mon.1)),
    };
    WindowPlan {
        x: pos.0 + ((mon.0 - w) / 2) as i32,
        y: pos.1 + ((mon.1 - h) / 2) as i32,
        width: w,
        height: h,
    }
}

/// The monitor list in `--monitor` order: the primary first, then every other monitor in the system's order.
// DC-DEFAULT: CLI-111 (proposed) `--monitor N`: 0 = primary, 1.. = the others
pub fn monitor_order<T: PartialEq + Clone>(primary: Option<&T>, all: &[T]) -> Vec<T> {
    let mut v: Vec<T> = primary.cloned().into_iter().collect();
    v.extend(all.iter().filter(|m| Some(*m) != primary).cloned());
    v
}

/// The two looks of the one window: full screen (`Borderless`, the monitor-filling borderless window of CLI-111) and
/// windowed (`Bordered`, a bordered centred window at the windowed size). Alt+Enter switches between them; neither
/// is exclusive full screen and neither changes the display mode.
// DC-DEFAULT: CLI-1280 (proposed; supersedes CLI-220's sizes) Alt+Enter toggles the window look (no 2009 evidence
// for the key; the 2009 options screen chose full screen or windowed, RENDER_2009.md §2.1)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowLook {
    Borderless,
    Bordered,
}

impl WindowLook {
    /// The other look.
    pub fn toggled(self) -> WindowLook {
        match self {
            WindowLook::Borderless => WindowLook::Bordered,
            WindowLook::Bordered => WindowLook::Borderless,
        }
    }
}

/// The windowed look's inner size without `--size` or a profile resolution.
// DC-DEFAULT: CLI-1280 (proposed)
pub const BORDERED_SIZE: (u32, u32) = (1280, 720);

/// The windowed look's default inner size (physical pixels) on a monitor with `scale` physical pixels per point:
/// [`BORDERED_SIZE`] as it is, or, when `points` (macOS), [`BORDERED_SIZE`] in points, so a Retina panel (scale 2)
/// gets a 2560x1440 physical window that looks like 1280x720 on screen, not a quarter-area one.
// DC-DEFAULT: CLI-1284 (proposed)
pub fn default_windowed_size(scale: f64, points: bool) -> (u32, u32) {
    let s = if points && scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (
        (BORDERED_SIZE.0 as f64 * s).round().max(1.0) as u32,
        (BORDERED_SIZE.1 as f64 * s).round().max(1.0) as u32,
    )
}

/// The event loop of a DeCombat window. On macOS without winit's default application menu: its Quit item ends the
/// process through `terminate:` before the client's own close (net leave, report, log flush); Cmd+Q is handled as a
/// key by the windows instead (CLI-1285). The other platforms have no such menu.
// DC-DEFAULT: CLI-1285 (proposed)
// PUBLIC: winit docs (EventLoopBuilderExtMacOS::with_default_menu)
pub fn new_event_loop() -> Result<EventLoop<()>, ClientError> {
    let mut builder = EventLoop::builder();
    #[cfg(target_os = "macos")]
    builder.with_default_menu(false);
    builder
        .build()
        .map_err(|e| ClientError::Window(e.to_string()))
}

/// True for the key press that quits on macOS: Q pressed (not repeated) with a Command key held. Never on the other
/// platforms (their Q with the Windows / Super key is nothing of ours).
// DC-DEFAULT: CLI-1285 (proposed)
pub fn is_mac_quit(code: KeyCode, down: bool, repeat: bool, super_held: bool) -> bool {
    MACOS && super_held && down && !repeat && code == KeyCode::KeyQ
}

/// macOS: puts the window into winit's simple fullscreen (the whole screen, menu bar and Dock hidden, no new Space,
/// no display-mode change) or takes it out again. The other platforms do nothing: their borderless window covers the
/// monitor on its own, while macOS keeps the menu bar and the Dock above a normal-level window. A window that is on
/// no screen (winit asks the window for its screen) is left alone. Returns whether the window changed.
// DC-DEFAULT: CLI-1284 (proposed)
// PUBLIC: winit docs (WindowExtMacOS::set_simple_fullscreen; Window::current_monitor is None off screen)
pub fn mac_simple_fullscreen(window: &Window, on: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        if on && window.current_monitor().is_none() {
            return false;
        }
        window.set_simple_fullscreen(on)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, on);
        false
    }
}

/// A window carried from one event-loop turn to the next (the menu's screens and the match each run
/// `run_app_on_demand` once) is ordered back on screen on macOS: winit closes every window when a run exits (its
/// `notify_windows_of_exit`, winit issue 4135), which hides it, and a hidden window's drawables are throttled to
/// the display rate by the window server. The other platforms keep the window up. A window still hidden on purpose
/// (CLI-511: before its first frame) is never carried, so this is only called on a window that was shown once.
// DC-DEFAULT: CLI-1288 (proposed)
// PUBLIC: winit docs (Window::is_visible, set_visible; macOS NSWindow close orders the window out)
pub fn reshow_carried(window: &Window) -> bool {
    if MACOS && window.is_visible() == Some(false) {
        window.set_visible(true);
        return true;
    }
    false
}

/// macOS: hides or shows the system cursor for the whole application (`NSCursor::hide` / `unhide`, a balanced
/// counter, so the two calls are paired through `Live::captured`). winit's `set_cursor_visible(false)` goes through
/// the view's cursor rects, which AppKit re-evaluates only on a cursor update; with the mouse locked the cursor
/// never moves, no update comes, and the arrow stayed over the match until the cursor was released and taken again
/// (playtest report). Nothing on the other platforms.
// DC-DEFAULT: CLI-1284 (amended)
// PUBLIC: Apple docs (NSCursor hide / unhide balance; main thread), winit 0.30 view.rs resetCursorRects
fn mac_cursor_hidden(hidden: bool) {
    // the AppKit call lives in dc-render (this crate forbids unsafe code); a no-op elsewhere
    dc_render::system_cursor_hidden(hidden);
}

/// macOS: the window's AppKit animations off (`dc_render::window_animations_off` through the raw AppKit handle;
/// this crate forbids unsafe code). Nothing elsewhere.
// DC-DEFAULT: CLI-1288 (amended)
// PUBLIC: winit docs (HasWindowHandle, RawWindowHandle::AppKit)
fn mac_window_animations_off(window: &Window) {
    #[cfg(target_os = "macos")]
    {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = window.window_handle() {
            if let RawWindowHandle::AppKit(a) = h.as_raw() {
                if !dc_render::window_animations_off(a.ns_view.as_ptr()) {
                    println!(
                        "dc-client: macOS window animations not turned off (no AppKit window)"
                    );
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// Whether `look` with `opts` is the macOS simple-fullscreen case: the full-screen look without `--size` (the
/// `--size` test window stays a plain centred window for the gates and the harness, CLI-1282).
fn wants_mac_fullscreen(look: WindowLook, opts: &WindowOptions) -> bool {
    MACOS && look == WindowLook::Borderless && opts.size.is_none()
}

/// The windowed look's inner size on a monitor of physical size `mon`: `requested` (or [`BORDERED_SIZE`]) as it is
/// when it fits nine tenths of each monitor side (the room for the frame and the task bar), else scaled down to fit
/// with its aspect kept; at least 1x1.
// DC-DEFAULT: CLI-1280 (proposed)
pub fn windowed_size(requested: Option<(u32, u32)>, mon: (u32, u32)) -> (u32, u32) {
    let (w, h) = requested.unwrap_or(BORDERED_SIZE);
    let (w, h) = (u64::from(w.max(1)), u64::from(h.max(1)));
    let nine = |n: u32| (u64::from(n.max(1)) * 9 / 10).max(1);
    let room = (nine(mon.0), nine(mon.1));
    let (w, h) = if w <= room.0 && h <= room.1 {
        (w, h)
    } else if room.0 * h <= room.1 * w {
        (room.0, h * room.0 / w)
    } else {
        (w * room.1 / h, room.1)
    };
    (w.max(1) as u32, h.max(1) as u32)
}

/// Window rectangle (inner size, physical pixels) of `look` on the monitor at `pos` with physical size `mon`:
/// full screen (`Borderless`) is [`plan_window`] with `fill` (`None`: the whole monitor, CLI-111; `Some`: the
/// `--size` test window); windowed (`Bordered`) is [`windowed_size`] of `windowed`, centred.
// DC-DEFAULT: CLI-1280 (proposed)
pub fn plan_look(
    look: WindowLook,
    fill: Option<(u32, u32)>,
    windowed: Option<(u32, u32)>,
    pos: (i32, i32),
    mon: (u32, u32),
) -> WindowPlan {
    match look {
        WindowLook::Borderless => plan_window(fill, pos, mon),
        WindowLook::Bordered => plan_window(Some(windowed_size(windowed, mon)), pos, mon),
    }
}

/// The look the first window opens with: the `--windowed` / `--fullscreen` choice, else the options' window mode
/// (`bordered`; full screen without a profile).
// DC-DEFAULT: CLI-1280 (proposed)
pub fn start_look(flag: Option<WindowLook>, bordered: bool) -> WindowLook {
    flag.unwrap_or(if bordered {
        WindowLook::Bordered
    } else {
        WindowLook::Borderless
    })
}

/// The look of the window now (0: no window opened yet, 1: full screen, 2: windowed). One window per process, kept
/// across the menu / match hand-over and a reopened window, so Alt+Enter in either keeps the other's look.
static LOOK_NOW: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The look of the window now (`None` before the first window opened).
pub fn current_look() -> Option<WindowLook> {
    match LOOK_NOW.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Some(WindowLook::Borderless),
        2 => Some(WindowLook::Bordered),
        _ => None,
    }
}

fn set_current_look(look: WindowLook) {
    let v = match look {
        WindowLook::Borderless => 1,
        WindowLook::Bordered => 2,
    };
    LOOK_NOW.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// Puts `window` into `look` on the monitor it is on (the primary without one), through the one window: decorations,
/// inner size, outer position; no full-screen request and no display-mode change. `None` (nothing changed) without
/// monitor information.
// DC-DEFAULT: CLI-1280 (proposed)
// PUBLIC: winit docs (set_decorations, request_inner_size, set_outer_position, current_monitor; sizes and
// positions are physical)
pub fn apply_look(window: &Window, look: WindowLook, opts: &WindowOptions) -> Option<WindowPlan> {
    // CLI-1604: the iPad's window is the screen in every look (winit would move the UIWindow, not resize it)
    if IOS {
        return None;
    }
    let Some(plan) = plan_here(window, look, opts) else {
        println!("dc-client: window look unchanged (no monitor information)");
        return None;
    };
    // CLI-1284: macOS leaves simple fullscreen before a plain window is shaped, and enters it after the full-screen
    // look was shaped (the plain shaping is what it restores when it leaves)
    let mac_full = wants_mac_fullscreen(look, opts);
    if !mac_full {
        mac_simple_fullscreen(window, false);
    }
    window.set_decorations(look == WindowLook::Bordered);
    let _ = window.request_inner_size(PhysicalSize::new(plan.width, plan.height));
    window.set_outer_position(PhysicalPosition::new(plan.x, plan.y));
    if mac_full {
        mac_simple_fullscreen(window, true);
    }
    set_current_look(look);
    println!(
        "dc-client: window look {:?}: {}x{} at {},{}",
        look, plan.width, plan.height, plan.x, plan.y
    );
    Some(plan)
}

/// The plan of `look` on the monitor `window` is on (the primary without one).
pub fn plan_here(window: &Window, look: WindowLook, opts: &WindowOptions) -> Option<WindowPlan> {
    let m = window
        .current_monitor()
        .or_else(|| window.primary_monitor())?;
    let (p, s) = (m.position(), m.size());
    Some(plan_look(
        look,
        opts.size,
        windowed_or_default(opts, m.scale_factor()),
        (p.x, p.y),
        (s.width, s.height),
    ))
}

/// The windowed look's requested size: `--size` / the saved resolution, else the platform default of
/// [`default_windowed_size`] for a monitor with `scale` physical pixels per point.
fn windowed_or_default(opts: &WindowOptions, scale: f64) -> Option<(u32, u32)> {
    opts.windowed_size
        .or_else(|| Some(default_windowed_size(scale, MACOS)))
}

/// Alt+Enter in the menu or the match: the other look, applied and noted for the profile (CLI-1280).
pub fn toggle_and_note(window: &Window, opts: &WindowOptions) -> Option<WindowPlan> {
    let next = current_look().unwrap_or(WindowLook::Borderless).toggled();
    let plan = apply_look(window, next, opts)?;
    crate::settings::note_display(Some(next == WindowLook::Bordered), None);
    Some(plan)
}

/// What the first window of the process does about the saved display values: its look, and whether it is a first
/// launch whose look and monitor size are saved (no saved look or resolution, no command line override).
// DC-DEFAULT: CLI-1280 (proposed)
pub fn first_window(
    flag: Option<WindowLook>,
    keep_size: bool,
    saved: Option<bool>,
    bordered: bool,
) -> (WindowLook, bool) {
    let first_launch = saved == Some(false);
    let look = match flag {
        Some(l) => l,
        None if first_launch => WindowLook::Borderless,
        None => start_look(None, bordered),
    };
    (look, first_launch && flag.is_none() && !keep_size)
}

/// True for the key press that toggles the look: Enter (main or keypad) pressed, not repeated, with an Alt key held.
pub fn is_look_toggle(code: KeyCode, down: bool, repeat: bool, alt_held: bool) -> bool {
    alt_held && down && !repeat && matches!(code, KeyCode::Enter | KeyCode::NumpadEnter)
}

/// Where a key goes while the in-match leave box may be open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveRoute {
    /// The box is closed: the key goes to the match as usual.
    Pass,
    /// A key of the box (Enter / Y = Yes, Escape / N = No).
    Box(crate::loading::LeaveKey),
    /// A key release under the open box: it still ends a held action or the held scoreboard.
    Release,
    /// Any other key under the open box: nothing happens.
    Swallow,
}

/// The leave box's key rule: while it is open it takes every key (CLI-510); only its own keys act and releases
/// still reach the match so no action stays held.
// DC-DEFAULT: CLI-510 (proposed)
pub fn leave_box_route(open: bool, code: KeyCode, down: bool, repeat: bool) -> LeaveRoute {
    use crate::loading::LeaveKey;
    if !open {
        return LeaveRoute::Pass;
    }
    if !down {
        return LeaveRoute::Release;
    }
    if repeat {
        return LeaveRoute::Swallow;
    }
    match code {
        KeyCode::Escape => LeaveRoute::Box(LeaveKey::Escape),
        KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::KeyY => LeaveRoute::Box(LeaveKey::Confirm),
        KeyCode::KeyN => LeaveRoute::Box(LeaveKey::Deny),
        _ => LeaveRoute::Swallow,
    }
}

/// Where a key goes while the in-match report form may be open (CLI-950).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportRoute {
    /// The form is closed: the key goes on as usual.
    Pass,
    /// A key the form takes (Escape cancels, Tab moves between the entries, Backspace).
    Key(crate::menu::MenuKey),
    /// Typed text for the focused entry.
    Text(String),
    /// A key release under the open form: it still ends a held action or the held scoreboard.
    Release,
    /// Any other key under the open form: nothing happens.
    Swallow,
}

/// The report form's key rule: while it is open it takes every key; releases still reach the match.
// DC-DEFAULT: CLI-950 (proposed) like the leave box (CLI-510): the form takes every key, only its own keys act
pub fn report_form_route(
    open: bool,
    code: KeyCode,
    down: bool,
    repeat: bool,
    text: Option<&str>,
) -> ReportRoute {
    use crate::menu::MenuKey;
    if !open {
        return ReportRoute::Pass;
    }
    if !down {
        return ReportRoute::Release;
    }
    match code {
        KeyCode::Escape if !repeat => ReportRoute::Key(MenuKey::Escape),
        KeyCode::Tab if !repeat => ReportRoute::Key(MenuKey::Tab),
        KeyCode::Backspace => ReportRoute::Key(MenuKey::Backspace),
        _ => match text.filter(|t| !t.is_empty() && t.chars().all(|c| !c.is_control())) {
            Some(t) => ReportRoute::Text(t.to_string()),
            None => ReportRoute::Swallow,
        },
    }
}

/// Where a key goes while the options over the match are open (CLI-1907).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionsRoute {
    /// A menu key of the options (Escape, Enter, Up / Down, Tab, Backspace, Space).
    Key(crate::menu::MenuKey),
    /// The key a waiting key box takes (its scan code).
    Scan(u8),
    /// Typed text (the share code's paste goes through the clipboard, not here).
    Text(String),
    /// A key release under the open box: it still ends a held action or the held scoreboard.
    Release,
    /// Any other key under the open box: nothing happens.
    Swallow,
}

/// The options box's key rule over the match: while it is open it takes every key as the menu window would give it
/// to the box; releases still reach the match. `scan`: a key box waits for a key (Escape still gives up).
// DC-DEFAULT: CLI-1907 (proposed) as the leave box and the report form (CLI-510, CLI-950)
pub fn options_route(code: KeyCode, down: bool, repeat: bool, text: Option<&str>, scan: bool) -> OptionsRoute {
    use crate::menu::MenuKey;
    if !down {
        return OptionsRoute::Release;
    }
    if scan && code != KeyCode::Escape {
        if repeat {
            return OptionsRoute::Swallow;
        }
        return crate::settings::scan_of_code(code).map_or(OptionsRoute::Swallow, OptionsRoute::Scan);
    }
    if let Some(k) = crate::menu::window::menu_key(code, false) {
        let once = matches!(k, MenuKey::Escape | MenuKey::Enter | MenuKey::Tab | MenuKey::BackTab);
        if repeat && once {
            return OptionsRoute::Swallow;
        }
        return OptionsRoute::Key(k);
    }
    match text.filter(|t| !t.is_empty() && t.chars().all(|c| !c.is_control())) {
        Some(t) => OptionsRoute::Text(t.to_string()),
        None => OptionsRoute::Swallow,
    }
}

/// An Escape in the match with the box closed: frees a captured cursor; the same press opens the exit box (no
/// cursor-only step first, the 2009 key handler shows the box and the cursor together).
// EVIDENCE: cshell+0x49670 case 0x1b, cshell+0x275290 (shows the box and refreshes the cursor)
// DC-DEFAULT: CLI-920 (proposed) replaces CLI-4's "Escape releases the cursor, Escape again opens the box"
pub fn escape_in_match(
    captured: bool,
    leave: &mut crate::loading::LeaveBox,
) -> (bool, crate::loading::LeaveStep) {
    (captured, leave.key(crate::loading::LeaveKey::Escape))
}

/// The one window of a menu run with its surface and GPU device: the menu draws in it, the loading page too, and
/// the match takes it over and hands it back, so the screen never shows a window swap or an undrawn window between
/// the lobby, the loading page and the match.
// EVIDENCE: cshell+0x1bb0b0 (the 2009 client changes the interface state inside its one window)
pub struct WindowHost {
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub ctx: GpuContext,
    /// The surface can be read back for `shot` steps (`--shot-dir`, gate T2).
    pub readable: bool,
    /// The surface offers Immediate presentation (CLI-1595: the setting may switch to it).
    pub immediate_ok: bool,
}

/// The attributes of a window DeCombat opens: hidden until its first frame (CLI-511), never always-on-top, no
/// border, not resizable, no mode change; at the plan's place when a monitor was chosen.
// DC-DEFAULT: CLI-511 (proposed)
pub fn host_attributes(
    plan: &WindowPlan,
    look: WindowLook,
    placed: bool,
    no_activate: bool,
) -> winit::window::WindowAttributes {
    // never always-on-top, not resizable, no mode change; a border only for the windowed look (CLI-1280)
    let mut attrs = Window::default_attributes()
        .with_title("dc-client")
        .with_decorations(look == WindowLook::Bordered)
        .with_resizable(false)
        .with_visible(false)
        .with_window_level(WindowLevel::Normal);
    // CLI-1604: on iPadOS a UIWindow made without a size fills the screen (a size would shrink it)
    if !IOS {
        attrs = attrs.with_inner_size(PhysicalSize::new(plan.width, plan.height));
    }
    // `--no-activate` (gate T2): the window opens without focus, below the other windows
    if no_activate {
        attrs = attrs
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnBottom);
    }
    if placed && !IOS {
        attrs = attrs.with_position(PhysicalPosition::new(plan.x, plan.y));
    }
    attrs
}

/// Opens the window (hidden: the caller shows it after its first frame was presented, so no undrawn, white window
/// is ever on screen) and its surface, on the monitor and at the size of `opts`.
// DC-DEFAULT: CLI-511 (proposed) a new window stays hidden until its first frame was presented
// PUBLIC: winit docs (WindowAttributes::with_visible, primary_monitor / available_monitors / MonitorHandle::position,
// size are physical pixels; a PhysicalSize / PhysicalPosition request is 1:1 whatever the scale factor)
pub fn open_host(el: &ActiveEventLoop, opts: &WindowOptions) -> Result<WindowHost, ClientError> {
    let monitors = monitor_order(
        el.primary_monitor().as_ref(),
        &el.available_monitors().collect::<Vec<_>>(),
    );
    let chosen = monitors.get(opts.monitor).or_else(|| {
        println!(
            "dc-client: --monitor {} not found ({} monitor(s)), using the primary",
            opts.monitor,
            monitors.len()
        );
        monitors.first()
    });
    // CLI-1280: a window opened again (the menu after a device loss, a match without the menu's window) keeps the
    // look the player switched to; the first takes the flag or the options' window mode
    let (look, note_first) = match current_look() {
        Some(l) => (l, false),
        None => first_window(
            opts.start_look,
            opts.keep_size,
            crate::settings::published_display_saved(),
            crate::settings::published_bordered(),
        ),
    };
    set_current_look(look);
    let plan = match chosen {
        Some(m) => {
            let (p, s) = (m.position(), m.size());
            let plan = plan_look(
                look,
                opts.size,
                windowed_or_default(opts, m.scale_factor()),
                (p.x, p.y),
                (s.width, s.height),
            );
            println!(
                "dc-client: window look {:?}: {}x{} at {},{} (monitor {}x{} at {},{})",
                look, plan.width, plan.height, plan.x, plan.y, s.width, s.height, p.x, p.y
            );
            // CLI-1280: a first launch saves full screen and the monitor's size as the resolution
            if note_first {
                crate::settings::note_display(Some(false), Some((s.width, s.height)));
                println!(
                    "dc-client: first launch: resolution {}x{} saved",
                    s.width, s.height
                );
            }
            plan
        }
        // no monitor information: the requested size, or a 1280x720 fallback, at the system's placement
        None => {
            let (w, h) = match look {
                WindowLook::Borderless => opts.size.unwrap_or(BORDERED_SIZE),
                WindowLook::Bordered => opts.windowed_size.unwrap_or(BORDERED_SIZE),
            };
            WindowPlan {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }
        }
    };
    let attrs = host_attributes(
        &plan,
        look,
        chosen.is_some(),
        crate::steps::config().no_activate,
    );
    let window = Arc::new(
        el.create_window(attrs)
            .map_err(|e| ClientError::Window(e.to_string()))?,
    );
    // CLI-1288: no AppKit fade on close / order-front, so the hand-over between the event-loop turns (winit closes
    // the window at every exit) never shows the desktop
    mac_window_animations_off(&window);
    // CLI-1284: on macOS the full-screen look is winit's simple fullscreen (menu bar and Dock hidden), taken while
    // the window is still hidden (CLI-511), so the first frame shown is already the full-screen one
    if wants_mac_fullscreen(look, opts) && !mac_simple_fullscreen(&window, true) {
        println!("dc-client: macOS simple fullscreen refused (the window is on no screen?)");
    }
    let (surface, ctx) =
        open_gpu(opts.adapter, opts.adapter_first, &window).map_err(ClientError::Gpu)?;
    // CLI-1840: the adapter a diagnostic report names
    crate::diag::note_gpu(&ctx.info);
    // fix-hang: a driver-side device loss unwinds out of wgpu instead of deadlocking the frame (CLI-460)
    crate::gpulost::watch(&ctx.device);
    let caps = surface.get_capabilities(&ctx.adapter);
    // SPEC: RENDER_2009.md §2.1 (DeCombat: Bgra8Unorm window surface, no sRGB conversion)
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| *f == wgpu::TextureFormat::Bgra8Unorm)
        .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
        .or_else(|| caps.formats.first().copied())
        .ok_or_else(|| ClientError::Gpu("surface has no formats".into()))?;
    // DC-DEFAULT: CLI-320 (proposed) Immediate presentation (no frame cap) unless --vsync asks for Fifo; CLI-1575
    // / CLI-1595: the frame-rate setting's VSync entry (`wants_fifo`)
    let immediate_ok = caps.present_modes.contains(&wgpu::PresentMode::Immediate);
    let present_mode = present_mode_for(wants_fifo(opts.vsync, opts.no_vsync), &caps);
    let size = window.inner_size();
    if (size.width, size.height) != (plan.width, plan.height) {
        println!(
            "dc-client: window inner size {}x{} (planned {}x{})",
            size.width, size.height, plan.width, plan.height
        );
    }
    // `shot` steps read the swapchain image back before it is presented (gate T2)
    let readable = crate::steps::config().shot_dir.is_some()
        && caps.usages.contains(wgpu::TextureUsages::COPY_SRC);
    let config = wgpu::SurfaceConfiguration {
        usage: if readable {
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
        } else {
            wgpu::TextureUsages::RENDER_ATTACHMENT
        },
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode,
        // iPadOS / DX12: two frames in flight (dc_render::frame_latency_for, dx12-perf), else one
        desired_maximum_frame_latency: dc_render::frame_latency_for(ctx.info.backend),
        alpha_mode: caps
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto),
        view_formats: vec![],
    };
    surface.configure(&ctx.device, &config);
    Ok(WindowHost {
        window,
        surface,
        config,
        ctx,
        readable,
        immediate_ok,
    })
}

struct Live {
    /// fix-hang watchdog events: the death view and the remote count of the last frame.
    watch_dead: bool,
    watch_remotes: usize,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    /// CLI-1593: the multisampled colour and depth the 3D passes draw into while the renderer's MSAA count is above 1
    /// (made in the frame for the window's size and the count; `None` with MSAA off).
    msaa: Option<dc_render::MsaaTargets>,
    /// CLI-1593: the anti-aliasing count the settings asked for when the renderer was built (before the device
    /// clamp); a different published count rebuilds the renderer at the next frame ([`App::reload_renderer`]).
    msaa_want: u32,
    /// CLI-1595: the surface offers Immediate presentation (the frame-rate setting may switch to and from Fifo).
    immediate_ok: bool,
    ctx: GpuContext,
    renderer: WorldRenderer,
    overlay: OverlayRenderer,
    hud_layer: OverlayRenderer,
    hud: Option<crate::hud::HudDriver>,
    board_held: bool,
    boxes: DebugBoxes,
    models: Option<ModelScene>,
    /// The weapon effects ([`crate::fx`]) and their colour format.
    fx: crate::fx::FxStage,
    fx_format: wgpu::TextureFormat,
    font: Option<OverlayFont>,
    world: LoadedWorld,
    player: LocalPlayer,
    view: ViewAngles,
    /// The own view yaw offset sent as 0x64 aux[1] (CLI-636).
    twist: crate::aim::OwnTwist,
    death_cam: crate::deathcam::DeathCam,
    cam_shot: crate::camshot::CamShot,
    fps: FpsMeter,
    started: Instant,
    last: Instant,
    /// CLI-1576: the frame clock, the previous frame's render time, and the display period when presentation
    /// waits for the blank (looked up once).
    frame_clock: crate::frameclock::FrameClock,
    render_last: Instant,
    vsync_period: Option<Duration>,
    vsync_period_known: bool,
    focused: bool,
    captured: bool,
    /// The surface can be read back for `shot` steps (`--shot-dir`, gate T2).
    readable: bool,
    sound: Option<crate::sound::SoundDirector>,
    /// Seconds after the start when the next sound output status line is printed.
    audio_status_at: f64,
    /// The mover's surface type, held while airborne.
    surface_type: u8,
    /// The 2009 gamma ramp of the finished frame (RENDER_2009.md §2.5).
    gamma: dc_render::GammaStage,
    /// client-fps: the frame-time distribution of the run, and the frame profiler (`--profile` / `DC_PROF`; off:
    /// `prof` is `None` and the lap clock reads no clock). A profile record runs from one redraw to the next, so
    /// the event loop's time between them is its `loop` stage.
    times: crate::frametimes::FrameTimes,
    laps: crate::prof::Laps,
    prof: Option<WindowProf>,
    /// gpu-recover: the HUD layer was rebuilt; the next frame composes and uploads the HUD in full.
    hud_force: bool,
    /// CLI-1571: when the network overlay's lines were last refreshed (HUD ms) and in which mode.
    net_refresh: Option<(u32, u8)>,
    /// CLI-1596: when the "Show FPS" line was last refreshed (HUD ms).
    fps_refresh: Option<u32>,
    /// CLI-1573: the mouse's report rate: motion events counted since `mouse_t0`, and the last rate (per second)
    /// the count gave (user request 2026-10-31: the rate the mouse delivers, on the `/network` overlay).
    mouse_count: u32,
    mouse_t0: Option<Instant>,
    mouse_hz: Option<f32>,
}

/// gpu-recover: the window is rebuilding a lost GPU device (the match goes on, nothing is drawn meanwhile).
struct Recovering {
    since: Instant,
    attempts: u32,
    next_at: Instant,
    last_error: String,
}

/// The window's frame profiler state (client-fps).
struct WindowProf {
    /// Window events handled since the last redraw (perf-g1: the hitch list's `window_events`).
    events: usize,
    rec: crate::prof::Recorder,
    /// A record is open (begun at the last redraw).
    open: bool,
    remotes: usize,
}

/// Takes the cursor for mouse look; `Ok` names the grab. CLI-1284: macOS has no confined grab (winit refuses it), so
/// the lock comes first there; elsewhere the confined grab first, the lock as the fallback (CLI-4).
#[cfg(not(target_os = "ios"))]
fn grab_cursor(w: &Window) -> Result<&'static str, String> {
    let (first, second) = if MACOS {
        (CursorGrabMode::Locked, CursorGrabMode::Confined)
    } else {
        (CursorGrabMode::Confined, CursorGrabMode::Locked)
    };
    let name = |m: CursorGrabMode| match m {
        CursorGrabMode::Confined => "confined",
        _ => "locked",
    };
    match w.set_cursor_grab(first) {
        Ok(()) => Ok(name(first)),
        Err(_) => match w.set_cursor_grab(second) {
            Ok(()) => Ok(name(second)),
            Err(e) => Err(format!("grab refused: {e}")),
        },
    }
}

/// iPadOS: winit has no cursor grab there; the pointer lock (the system pointer hidden, the mouse reporting deltas
/// through GameController) is set by the iOS driver from the stage's state (`menu::window::ipad`: the menu
/// always, the match while it holds the mouse), so the grab itself changes nothing here.
// DC-DEFAULT: CLI-1604 (proposed; amended: the driver owns the lock)
#[cfg(target_os = "ios")]
fn grab_cursor(_w: &Window) -> Result<&'static str, String> {
    Ok("pointer lock (driver)")
}

/// Gives the cursor back; `false` when the system refused the reset.
#[cfg(not(target_os = "ios"))]
fn ungrab_cursor(w: &Window) -> bool {
    w.set_cursor_grab(CursorGrabMode::None).is_ok()
}

#[cfg(target_os = "ios")]
fn ungrab_cursor(_w: &Window) -> bool {
    true
}

/// The display's refresh rate in millihertz: winit's monitor value; on iPadOS the screen's maximum frame rate
/// (winit's iOS monitor reports 60 Hz on a 120 Hz ProMotion panel, which held the frame clock and the menu's pacer
/// at 60; OBSERVED 2026-10-31 on the user's iPad Pro).
// DC-DEFAULT: CLI-1604 (amended)
pub fn monitor_refresh_mhz(w: &Window) -> Option<u32> {
    #[cfg(target_os = "ios")]
    {
        let hz = dc_ios_hid::display_max_hz();
        if hz > 0 {
            return Some(hz * 1000);
        }
    }
    w.current_monitor().and_then(|m| m.refresh_rate_millihertz())
}

/// Ends the current run of the event loop (the window's turn). iPadOS has one event loop for the whole process that
/// never returns (`crate::ios` drives the turns and reads the handlers' end flags instead); winit ignores the
/// request there, so it is not made.
// DC-DEFAULT: CLI-1604 (proposed)
pub(crate) fn end_loop(el: &ActiveEventLoop) {
    #[cfg(not(target_os = "ios"))]
    el.exit();
    #[cfg(target_os = "ios")]
    let _ = el;
}

struct App {
    /// CLI-890: presentation frame pacer
    pacer: crate::pacer::Pacer,
    opts: WindowOptions,
    /// The physical size the window was planned with (restored if a scale-factor change tries to rescale it).
    target: (u32, u32),
    source: Box<dyn WorldSource>,
    input: Box<dyn InputSource>,
    live: Option<Live>,
    /// CLI-1619: the match's own mod host (from `opts.mods`), opened with the first `start` and kept across a GPU
    /// rebuild; dropped with the window run (the mod files are uninstalled then).
    mods: Option<crate::modassets::MatchMods>,
    error: Option<ClientError>,
    report: WindowReport,
    closing: bool,
    /// Whether an Alt key is down (Alt+Enter, CLI-1280; the look itself is [`current_look`]).
    alt_held: bool,
    /// Whether a Command (Super) key is down (Cmd+Q on macOS, CLI-1285).
    super_held: bool,
    /// The cursor was released for a look change: take it again when the new size arrives.
    recapture: bool,
    /// When the cursor grab is asserted again while captured (CLI-1721).
    grab_keep: crate::grabkeep::GrabKeeper,
    /// CLI-1848: frames drawn by the loop when the window system stops delivering redraw events.
    redraw_keep: crate::redrawkeep::RedrawKeeper,
    /// Close the window one second after the server ended the session (the menu's game runs).
    close_on_over: bool,
    /// A sound director handed in by the caller (the menu's, with its music): used instead of opening one, and
    /// handed back open when the window closes, so the music plays on across the menu / match switch.
    carried_sound: Option<crate::sound::SoundDirector>,
    /// The window was given `carried_sound` (it is handed back instead of closed).
    carries_sound: bool,
    /// The events the frame state gives (`--events`, gate T2).
    watch: crate::events::GameWatch,
    /// w2-loading: the menu's window handed in (taken by `start`), and handed back when the match window ends.
    host_in: Option<WindowHost>,
    host_out: Option<WindowHost>,
    /// The window was handed in: hand it back instead of closing it.
    hands_back: bool,
    /// `start` opened a window of its own (hidden until the first frame is presented).
    created_window: bool,
    /// m36: the leave-the-match box ([`crate::loading::LeaveBox`]).
    leave: crate::loading::LeaveBox,
    /// The left button went down over the open leave box (its button fires on the release over it).
    leave_held: bool,
    /// CLI-1012: interface cues of the leave box's clicks, played with the next match frame.
    ui_cues: Vec<crate::sound::SoundCue>,
    /// The pointer in window pixels (the leave box's buttons).
    pointer: [f64; 2],
    /// CLI-950: the in-match report form, open while the exit box's report answer is up.
    report_form: Option<crate::menu::reportdlg::ReportForm>,
    /// CLI-1907: the options over the match (a menu of their own, built at the first Options click and kept for the
    /// match) and whether they are up.
    options_menu: Option<crate::menu::Menu>,
    options_up: bool,
    /// CLI-1907: the clipboard of the Mods tab's share code buttons (opened at their first use).
    clipboard: Option<crate::menu::clipboard::SystemClipboard>,
    /// When the last report went out (a new one within ten seconds shows the notice, cshell+0x273b70).
    last_report: Option<Instant>,
    /// gpu-recover: a lost GPU device is being rebuilt.
    recovering: Option<Recovering>,
}

fn depth_view(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    dc_render::perf::texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("client window depth"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        },
    )
    .create_view(&Default::default())
}

/// Milliseconds since `t0` (the grab keeper's clock).
/// The match surface at the window's client `size`, configured again; the depth buffer, the multisampled targets
/// and the HUD follow a changed size. `false` for a zero size (minimised): nothing changes.
fn fit_surface(l: &mut Live, size: PhysicalSize<u32>) -> bool {
    if size.width == 0 || size.height == 0 {
        return false;
    }
    let changed = (size.width, size.height) != (l.config.width, l.config.height);
    l.config.width = size.width;
    l.config.height = size.height;
    l.surface.configure(&l.ctx.device, &l.config);
    if changed {
        l.depth = depth_view(&l.ctx.device, size.width, size.height);
        // CLI-1593: the frame makes the multisampled targets again at the new size
        l.msaa = None;
        if let Some(h) = l.hud.as_mut() {
            h.hud.set_size(size.width, size.height);
        }
    }
    true
}

/// Whether `window` can show a frame: not minimised and with a non-zero client area (CLI-1848).
// PUBLIC: winit 0.30 docs (Window::is_minimized, Window::inner_size)
pub(crate) fn window_visible(window: &Window) -> bool {
    let s = window.inner_size();
    s.width > 0 && s.height > 0 && window.is_minimized() != Some(true)
}

fn ms_since(t0: Instant) -> u64 {
    t0.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

/// A surface of `window` and a device on an adapter that presents to it. `Backends` restricts the instance to those
/// backends; with `first` (CLI-1852, the saved Renderer choice) a failure there tries the default backends next,
/// so the chosen backend is the first attempt rather than the only one.
fn open_gpu(
    choice: AdapterChoice,
    first: bool,
    window: &Arc<Window>,
) -> Result<(wgpu::Surface<'static>, GpuContext), String> {
    // CLI-1931: the saved renderer's first attempt takes only a hardware adapter (a DX12 instance whose driver
    // wgpu does not expose offers only WARP); the default backends' attempt and an `--adapter` choice may still end
    // on the software adapter
    let attempt = |choice: AdapterChoice,
                   software: bool|
     -> Result<(wgpu::Surface<'static>, GpuContext), String> {
        let mut desc = wgpu::InstanceDescriptor::default();
        if let AdapterChoice::Backends(b) = choice {
            desc.backends = b;
        }
        let instance = wgpu::Instance::new(&desc);
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let ctx = GpuContext::for_surface_with(instance, &surface, software)
            .map_err(|e| e.to_string())?;
        Ok((surface, ctx))
    };
    let fallback_next = first && matches!(choice, AdapterChoice::Backends(_));
    let out = match attempt(choice, !fallback_next) {
        Err(e) if fallback_next => {
            println!("dc-client window: the chosen renderer has no adapter ({e}); trying the default backends");
            attempt(AdapterChoice::Auto, true)
                .map_err(|e2| format!("{e}; then the default backends: {e2}"))
        }
        r => r,
    };
    // CLI-1931: the adapter in the log at once (the run's end line never comes when the run crashes)
    if let Ok((_, ctx)) = &out {
        println!("dc-client window: GPU {}", gpu_line(&ctx.info));
    }
    out
}

/// CLI-1932: the adapter choice of device-loss rebuild attempt `attempt` (1 = the first): the first attempt asks for
/// the window's own choice; later ones, when that choice is the saved renderer (`first`), its other backend (DX12
/// and Vulkan swap; the default backends still follow, `open_gpu`). An `--adapter` choice (`first` false) and the
/// default backends stay as they are.
fn rebuild_choice(choice: AdapterChoice, first: bool, attempt: u32) -> AdapterChoice {
    match choice {
        AdapterChoice::Backends(b) if first && attempt >= 2 => {
            let other = if b == wgpu::Backends::DX12 {
                wgpu::Backends::VULKAN
            } else if b == wgpu::Backends::VULKAN {
                wgpu::Backends::DX12
            } else {
                b
            };
            AdapterChoice::Backends(other)
        }
        c => c,
    }
}

/// CLI-1931: the window's adapter as one log line: name, backend, device type, driver, vendor and device id.
fn gpu_line(info: &wgpu::AdapterInfo) -> String {
    let driver = format!("{} {}", info.driver, info.driver_info);
    format!(
        "{} ({:?}, {:?}); driver {}; vendor 0x{:04x} device 0x{:04x}",
        info.name,
        info.backend,
        info.device_type,
        driver.trim(),
        info.vendor,
        info.device
    )
}

impl App {
    fn log_cursor(&mut self, what: String) {
        let t = self
            .live
            .as_ref()
            .map_or(0.0, |l| l.started.elapsed().as_secs_f64());
        println!("dc-client: {t:7.3} s cursor {what}");
        self.report.cursor_log.push((t, what));
    }

    fn capture(&mut self) {
        // a scripted input (--script, --walk-test, --shoot-test) needs no mouse: the cursor stays free
        // DC-DEFAULT: CLI-4 extension (proposed): no cursor grab while a scripted input drives the player
        if self.opts.scripted
            || self.opts.walk_test
            || self.opts.shoot_test
            || crate::steps::active()
        {
            return;
        }
        let msg = {
            let Some(live) = self.live.as_mut() else {
                return;
            };
            if live.captured || !live.focused {
                return;
            }
            let w = &live.window;
            match grab_cursor(w) {
                Ok(m) => {
                    w.set_cursor_visible(false);
                    mac_cursor_hidden(true);
                    live.captured = true;
                    self.grab_keep.on_capture(ms_since(live.started));
                    format!("{m} + hidden")
                }
                Err(e) => e,
            }
        };
        self.log_cursor(msg);
        self.soft_cursor();
    }

    /// CLI-1721: the grab asserted again while the window is focused and captured (Windows: the system's cursor clip
    /// can be dropped or stale after an alt-tab return; asserting an unchanged clip costs nothing). Elsewhere the
    /// keeper runs but asserts nothing (the macOS lock and the iPad pointer lock hold by themselves).
    fn keep_grab(&mut self) {
        let Some(live) = self.live.as_ref() else {
            return;
        };
        let now = ms_since(live.started);
        if self.grab_keep.retry_capture(now, live.captured, live.focused) {
            if !self.leave.open && !self.form_up() {
                self.capture();
            }
            return;
        }
        if !self.grab_keep.due(now, live.captured, live.focused) || !cfg!(windows) {
            return;
        }
        let w = &live.window;
        match grab_cursor(w) {
            Ok(_) => w.set_cursor_visible(false),
            Err(e) => {
                let msg = format!("kept grab refused: {e}");
                self.log_cursor(msg);
            }
        }
    }

    /// CLI-1604: the iPad has no system pointer to show while the mouse is free (the pointer stays locked for the
    /// whole run there), so the HUD draws the ROM cursor at the pointer while the match is not capturing.
    #[cfg(target_os = "ios")]
    fn soft_cursor(&mut self) {
        let pointer = self.pointer;
        let rom = match &self.opts.hud {
            crate::hud::HudChoice::Rom(p) => Some(p.clone()),
            _ => None,
        };
        let Some(l) = self.live.as_mut() else {
            return;
        };
        let target = (l.config.width.max(1), l.config.height.max(1));
        let captured = l.captured;
        let Some(h) = l.hud.as_mut() else {
            return;
        };
        // the cursor is a keyed PCX of LOGO.rez (the menu's loader), not a texture the HUD's source reads: put
        // into the HUD's assets once; its size comes from the image itself
        let entry = crate::menu::cursor::CURSOR_ENTRY;
        let size = match h.hud.assets.generated(entry) {
            Some(t) => Some([t.width, t.height]),
            None => rom.as_deref().and_then(crate::menu::cursor::load_rom).map(|t| {
                let size = [t.width, t.height];
                h.hud.assets.put(entry, t);
                if std::env::var_os("DC_IOS_INPUT_LOG").is_some() {
                    eprintln!("dc-client ios input: match cursor image {size:?} loaded");
                }
                size
            }),
        };
        if size.is_none() && std::env::var_os("DC_IOS_INPUT_LOG").is_some() {
            eprintln!("dc-client ios input: no cursor image for the match (rom {rom:?})");
        }
        let item = match size {
            Some(size) if !captured => {
                let sc = dc_ui_data::VirtualScaler::new(target.0, target.1);
                let p = sc.to_virtual([pointer[0] as f32, pointer[1] as f32]);
                Some(crate::menu::cursor::item(size, p, target))
            }
            _ => None,
        };
        h.set_cursor(item);
    }

    #[cfg(not(target_os = "ios"))]
    fn soft_cursor(&mut self) {}

    fn release(&mut self, why: &str) {
        let ok = {
            let Some(live) = self.live.as_mut() else {
                return;
            };
            if !live.captured {
                return;
            }
            let ok = ungrab_cursor(&live.window);
            live.window.set_cursor_visible(true);
            mac_cursor_hidden(false);
            live.captured = false;
            self.grab_keep.on_release();
            ok
        };
        self.input.device(DeviceInput::ReleaseAll);
        self.log_cursor(format!(
            "released ({why}){}",
            if ok { "" } else { ", grab reset refused" }
        ));
        self.soft_cursor();
    }

    /// The GPU device was lost (driver reset): the window leaves the match without another call on the lost device.
    /// Its GPU objects (the whole [`Live`] but the sound director) are leaked on purpose: dropping them can run
    /// wgpu-core's lost-device paths that deadlock (crate::gpulost). The window is hidden; the report keeps the
    /// frame counts.
    // DC-DEFAULT: CLI-460, see crate::gpulost
    fn on_gpu_reset(&mut self, el: &ActiveEventLoop) {
        crate::gpulost::left_match();
        crate::prewarm::load_begins();
        self.recovering = None;
        self.release("gpu device lost");
        if let Some(mut live) = self.live.take() {
            let r = &mut self.report;
            if self.carries_sound {
                self.carried_sound = live.sound.take();
            } else if let Some(d) = live.sound.as_mut() {
                r.audio_out.push(d.close_output());
            }
            drop(live.sound.take());
            r.frames = live.fps.frames;
            r.seconds = live.started.elapsed().as_secs_f64();
            r.mean_fps = live.fps.mean_fps();
            r.min_frame_ms = live.fps.min_ms;
            r.max_frame_ms = live.fps.max_ms;
            r.frame_times = live.times.clone();
            r.vsync_period_ms = live.vsync_period.map(|p| p.as_secs_f64() * 1000.0);
            r.pace_short = live.frame_clock.short;
            r.pace_long = live.frame_clock.long;
            r.ticks = live.player.ticks();
            r.end = live.player.state.pos.to_array();
            r.released_on_exit = !live.captured;
            r.gpu_reset = true;
            // the profile of the frames before the loss (gpu-reset): no call on the lost device
            crate::prof::tail_flush();
            if let Some(p) = live.prof.as_mut() {
                let _ = crate::gpulost::guard(|| p.rec.finish_with(None));
            }
            live.window.set_visible(false);
            std::mem::forget(live);
        }
        println!("dc-client: GPU device lost: the window leaves the match");
        self.close(el, "gpu device lost");
    }

    /// A key or click for the leave box: the lobby button asks, Yes leaves the match through the window's normal
    /// exit (the session's leave sequence, CLI-23), the box shows on the HUD while open; closing it takes the
    /// cursor again (the 2009 cursor is shown only while a box is up, cshell+0x1b6650).
    fn leave_key(&mut self, el: &ActiveEventLoop, k: crate::loading::LeaveKey) {
        // CLI-1012: a click on a button of the box plays the select sound, as every menu button does
        if matches!(k, crate::loading::LeaveKey::Click(_)) {
            let cue = self
                .live
                .as_ref()
                .and_then(|l| l.hud.as_ref())
                .and_then(|h| h.hud.exit_box.select_cue());
            self.ui_cues.extend(cue);
        }
        let step = self.leave.key(k);
        self.leave_step(el, step);
    }

    /// The effects of a leave box step (draw, events, leave, cursor).
    fn leave_step(&mut self, el: &ActiveEventLoop, step: crate::loading::LeaveStep) {
        use crate::loading::LeaveStep;
        self.leave_view();
        match step {
            LeaveStep::Opened | LeaveStep::Cancelled | LeaveStep::Asked => {
                crate::events::emit(
                    "leave_box",
                    serde_json::json!({
                        "open": step != LeaveStep::Cancelled,
                        "page": format!("{:?}", self.leave.view.page),
                    }),
                );
            }
            LeaveStep::Leave => self.close(el, "escape"),
            LeaveStep::Report => {
                crate::events::emit("leave_box", serde_json::json!({ "report": true }));
                // CLI-950: the exit box gives way to the report form (cshell+0x48e80 answer 2)
                self.leave = Default::default();
                self.leave_held = false;
                self.leave_view();
                self.report_open();
            }
            LeaveStep::Options => {
                crate::events::emit("leave_box", serde_json::json!({ "options": true }));
                // CLI-1907: the exit box gives way to the options; closing them shows the box again
                self.leave = Default::default();
                self.leave_held = false;
                self.leave_view();
                self.options_open();
            }
            LeaveStep::Look | LeaveStep::None => {}
        }
        if step == LeaveStep::Cancelled {
            self.leave_held = false;
            self.capture();
        }
        // without a HUD there is nothing to draw the box with: the key leaves as before
        if step == LeaveStep::Opened && self.live.as_ref().is_some_and(|l| l.hud.is_none()) {
            self.leave = Default::default();
            self.close(el, "escape");
        }
    }

    /// A form of the menu is up over the match (the report form, the options): it keeps the cursor free.
    fn form_up(&self) -> bool {
        self.report_form.is_some() || self.options_up
    }

    /// CLI-1907: opens the options over the match: the lobby's options box with the settings in effect in the match
    /// now (the hotkeys' sensitivity, the x-ray style) and, CLI-1908, a Mods tab listing the match's mods that have
    /// settings.
    fn options_open(&mut self) {
        let Some(l) = self.live.as_ref() else {
            return;
        };
        let size = (l.config.width, l.config.height);
        let mut s = self
            .opts
            .prefs
            .as_ref()
            .map(|p| p.settings.clone())
            .unwrap_or_default();
        s.sensitivity = self.opts.look.sensitivity.round() as i32;
        s.invert_y = self.opts.look.invert_y;
        s.xray = crate::settings::xray_style();
        let info = {
            let host = self.mods.as_ref().map(|m| m.host.borrow());
            crate::menu::optionsmods::MatchModsInfo::of_host(
                host.as_deref(),
                self.opts.mods.as_ref().map(|r| r.policy),
            )
            .with_fov_range(self.opts.client_fov)
        };
        crate::events::emit(
            "options_form",
            serde_json::json!({ "open": true, "mods": info.mods.len() }),
        );
        match self.options_menu.as_mut() {
            Some(m) => {
                m.set_size(size.0, size.1);
                m.reopen_over_match(Some(s), info);
            }
            None => {
                let rom = self.source.rom();
                let commands = self
                    .opts
                    .prefs
                    .as_ref()
                    .map(|p| p.commands.clone())
                    .unwrap_or_default();
                self.options_menu = Some(crate::menu::Menu::over_match(
                    rom.as_deref(),
                    None,
                    size,
                    s,
                    commands,
                    info,
                ));
            }
        }
        self.options_up = true;
        self.options_overlay();
    }

    /// Hands the options' display list to the HUD (every frame while they are up: the caret, the preview's burst).
    fn options_overlay(&mut self) {
        let items = match self.options_menu.as_mut() {
            Some(m) if self.options_up => m.items(),
            _ => Vec::new(),
        };
        if let Some(h) = self.live.as_mut().and_then(|l| l.hud.as_mut()) {
            h.set_overlay(items);
        }
    }

    /// One input for the open options, then what it changed: the interface sounds, the sound rows heard at once, the
    /// confirmed settings and the mods' values applied to the running match, the box closed.
    fn options_input(&mut self, ev: crate::menu::MenuInput) {
        let Some(m) = self.options_menu.as_mut().filter(|_| self.options_up) else {
            return;
        };
        m.input(ev);
        if m.data.clipboard_out.is_some() || m.data.clipboard_wanted {
            let clip = self
                .clipboard
                .get_or_insert_with(crate::menu::clipboard::SystemClipboard::new);
            m.pump_clipboard(clip);
        }
        let cues = m.take_sound_cues();
        let heard = m.data.sound_live.take();
        let confirmed = m.take_settings_change();
        let now = m.data.settings.clone();
        let open = m.over_match_open();
        self.ui_cues.extend(cues);
        if let Some(d) = self.live.as_mut().and_then(|l| l.sound.as_mut()) {
            if let Some(s) = heard.as_ref().or(confirmed.as_ref()) {
                crate::menu::apply_sound(s, d);
            }
        }
        if let Some(s) = &confirmed {
            self.apply_match_settings(s);
        }
        let mods_changed = self
            .opts
            .mods
            .as_ref()
            .is_some_and(|r| r.settings != now.mods_settings);
        if mods_changed {
            self.reload_match_mods(now.mods_settings.clone());
        }
        let known = self.opts.prefs.as_ref().map(|p| &p.settings);
        if (confirmed.is_some() || mods_changed) && known != Some(&now) {
            crate::settings::note_match_settings(now.clone());
            self.opts.prefs.get_or_insert_with(MatchPrefs::default).settings = now;
        }
        if open {
            self.options_overlay();
        } else {
            self.options_close();
        }
    }

    /// CLI-1907: the options went down (OK, Cancel, the close button, Escape): the exit box shows again.
    fn options_close(&mut self) {
        self.options_up = false;
        if let Some(h) = self.live.as_mut().and_then(|l| l.hud.as_mut()) {
            h.set_overlay(Vec::new());
        }
        crate::events::emit("options_form", serde_json::json!({ "open": false }));
        let step = self.leave.key(crate::loading::LeaveKey::Escape);
        self.leave_view();
        if step == crate::loading::LeaveStep::Opened {
            crate::events::emit(
                "leave_box",
                serde_json::json!({ "open": true, "page": "Exit" }),
            );
        }
        self.leave_pointer();
    }

    /// CLI-1907: the confirmed options in the running match: the mouse look at once (sensitivity, invert Y); the
    /// volumes went with the sound director above. The key bindings, the frame-rate cap, Show FPS, the gamma and the
    /// anti-aliasing were published by the options' OK and the match reads them per frame; the resolution and the
    /// window look follow when the menu takes the window back, the renderer at the next start.
    // DC-DEFAULT: CLI-1907 (proposed), CLI-1916 (proposed)
    fn apply_match_settings(&mut self, s: &crate::settings::Settings) {
        let v = s.sensitivity as f32;
        self.opts.look.sensitivity = v;
        self.input.set_sensitivity(v);
        self.opts.look.invert_y = s.invert_y;
        self.input.set_invert_y(s.invert_y);
        // CLI-1916: the field of view, on a server that allows it: the camera and the scope's normal field read
        // `opts.view` every frame
        self.opts.view = self.opts.view.with_player_fov(s.fov, self.opts.client_fov);
    }

    /// CLI-1908: the match's mods loaded again with the player's new values (the way the next match loads them,
    /// [`crate::mods::HostRecipe::load`]): the mods read `mod.settings` when they load, so the set is loaded again
    /// and its HUD keys and hooks handed to the HUD. Every mod of the set starts its Lua state again.
    // DC-DEFAULT: CLI-1908 (proposed) a change reloads the match's mod set (no per-mod change event)
    fn reload_match_mods(
        &mut self,
        values: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    ) {
        let Some(r) = self.opts.mods.as_mut() else {
            return;
        };
        r.settings = values;
        // the old set leaves the file lookups first: its drop installs nothing, the new one installs itself
        drop(self.mods.take());
        self.mods = crate::modassets::MatchMods::open(self.opts.mods.as_ref());
        let wallhacks = room_wallhacks(self.opts.room_hacks);
        if let Some(h) = self.live.as_mut().and_then(|l| l.hud.as_mut()) {
            match &self.mods {
                Some(m) => m.wire(h),
                None => {
                    h.hud.overrides = Default::default();
                    h.hud.hook = None;
                    h.xray = None;
                }
            }
            if !wallhacks {
                h.xray = None;
            }
        }
    }

    /// CLI-950: opens the in-match report form over the HUD: the lobby report box, the match players in its user
    /// list.
    // EVIDENCE: cshell+0x48e80 (answer 2: cshell+0x273b70(1), then every player of the match but the own one)
    fn report_open(&mut self) {
        let Some(l) = self.live.as_ref() else {
            return;
        };
        let (w, h) = (l.config.width, l.config.height);
        let (me, names) = match l.hud.as_ref() {
            Some(d) => {
                let own = d.state.own_pid;
                let me = own
                    .and_then(|p| d.state.roster.get(&p))
                    .map(|r| r.name.clone())
                    .unwrap_or_default();
                let names = d
                    .state
                    .roster
                    .values()
                    .filter(|r| Some(r.pid) != own && r.pid != 0xff)
                    .map(|r| r.name.clone())
                    .collect::<Vec<_>>();
                (me, names)
            }
            None => (String::new(), Vec::new()),
        };
        let rom = self.source.rom();
        let form = crate::menu::reportdlg::ReportForm::open(
            rom.as_deref(),
            None,
            (w, h),
            &names,
            &me,
            self.last_report,
        );
        crate::events::emit(
            "report_form",
            serde_json::json!({ "open": true, "players": names.len() }),
        );
        self.report_form = Some(form);
        self.report_overlay();
    }

    /// Hands the form's display list to the HUD.
    fn report_overlay(&mut self) {
        let items = self
            .report_form
            .as_mut()
            .map(|f| f.items())
            .unwrap_or_default();
        if let Some(h) = self.live.as_mut().and_then(|l| l.hud.as_mut()) {
            h.set_overlay(items);
        }
    }

    /// The pointer in the virtual 1024 x 768 screen.
    fn pointer_virtual(&self) -> [f32; 2] {
        match self.live.as_ref() {
            Some(l) => {
                dc_ui_data::VirtualScaler::new(l.config.width.max(1), l.config.height.max(1))
                    .to_virtual([self.pointer[0] as f32, self.pointer[1] as f32])
            }
            None => [0.0, 0.0],
        }
    }

    /// One input for the open report form: its sent reports go to the lobby connection, a closed form gives the
    /// cursor back to the match.
    fn report_input(&mut self, ev: crate::menu::MenuInput) {
        use crate::menu::reportdlg::FormStep;
        let Some(form) = self.report_form.as_mut() else {
            return;
        };
        let step = form.input(ev);
        for cmd in form.take_sent() {
            if let crate::menu::link::LinkCmd::Report {
                name,
                text,
                kind,
                detail,
            } = cmd
            {
                crate::events::emit(
                    "report_form",
                    serde_json::json!({ "sent": true, "kind": kind, "detail": detail, "target": name }),
                );
                self.source
                    .lobby_frame(crate::menu::link::report_packet(&name, &text, kind, detail));
            }
        }
        if let Some(t) = form.sent_at() {
            self.last_report = Some(t);
        }
        match step {
            FormStep::Closed => {
                self.report_form = None;
                if let Some(h) = self.live.as_mut().and_then(|l| l.hud.as_mut()) {
                    h.set_overlay(Vec::new());
                }
                crate::events::emit("report_form", serde_json::json!({ "open": false }));
                self.capture();
            }
            FormStep::Redraw => self.report_overlay(),
            FormStep::None => {}
        }
    }

    /// Hands the leave box's drawn state to the HUD.
    fn leave_view(&mut self) {
        let v = self.leave.shown();
        if let Some(h) = self.live.as_mut().and_then(|l| l.hud.as_mut()) {
            h.set_leave_box(v);
        }
    }

    /// The leave box's button under the pointer on the page shown, in the virtual 1024 x 768 screen.
    fn leave_button_at_pointer(&self) -> Option<usize> {
        let l = self.live.as_ref()?;
        let h = l.hud.as_ref()?;
        let sc = dc_ui_data::VirtualScaler::new(l.config.width.max(1), l.config.height.max(1));
        let p = sc.to_virtual([self.pointer[0] as f32, self.pointer[1] as f32]);
        h.hud.exit_box.button_at(self.leave.view.page, p)
    }

    /// The pointer moved or the left button changed over the open box: hover / pressed look.
    fn leave_pointer(&mut self) {
        if !self.leave.open {
            return;
        }
        let over = self.leave_button_at_pointer();
        if self.leave.pointer(over, self.leave_held) == crate::loading::LeaveStep::Look {
            self.leave_view();
        }
    }

    /// gpu-recover: the GPU device was lost. The window rebuilds it inside the match ([`App::try_recover`]); with
    /// `DC_GPU_RECOVER=0` it leaves the match at once (the fix-hang rule).
    // DC-DEFAULT: CLI-460 (amended by gpu-recover), see crate::gpulost
    fn on_gpu_lost(&mut self, el: &ActiveEventLoop) {
        if !crate::gpulost::recover_enabled() || self.live.is_none() {
            self.on_gpu_reset(el);
            return;
        }
        if self.recovering.is_none() {
            crate::watchdog::phase(crate::watchdog::Phase::Recovering);
            println!("dc-client: GPU device lost: rebuilding the device inside the match");
            let now = Instant::now();
            self.recovering = Some(Recovering {
                since: now,
                attempts: 0,
                next_at: now,
                last_error: String::new(),
            });
        }
        self.try_recover(el);
    }

    /// One recovery attempt; on failure the next one follows after [`crate::gpulost::RECOVER_RETRY_MS`], and after
    /// [`crate::gpulost::RECOVER_GIVE_UP_MS`] the window leaves the match.
    fn try_recover(&mut self, el: &ActiveEventLoop) {
        let Some(r) = self.recovering.as_mut() else {
            return;
        };
        r.attempts += 1;
        let attempt = r.attempts;
        let t = Instant::now();
        crate::watchdog::frame();
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.rebuild_gpu()));
        let err = match out {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(p) => Some(if p.is::<crate::gpulost::DeviceLostUnwind>() {
                "the new device was lost too".to_string()
            } else if let Some(m) = p.downcast_ref::<String>() {
                m.clone()
            } else if let Some(m) = p.downcast_ref::<&str>() {
                m.to_string()
            } else {
                "panic".to_string()
            }),
        };
        let Some(r) = self.recovering.as_mut() else {
            return;
        };
        match err {
            None => {
                let ms = r.since.elapsed().as_millis();
                println!(
                    "dc-client: GPU device recovered in {ms} ms (attempt {attempt}, rebuild {} ms); the match goes on",
                    t.elapsed().as_millis()
                );
                self.recovering = None;
                self.report.gpu_recoveries += 1;
                crate::gpulost::recovered();
                crate::watchdog::phase(crate::watchdog::Phase::Play);
                el.set_control_flow(ControlFlow::Poll);
                if let Some(l) = &self.live {
                    self.redraw_keep.arm(ms_since(l.started));
                    l.window.request_redraw();
                }
            }
            Some(e) => {
                println!("dc-client: GPU device recovery attempt {attempt} failed: {e}");
                r.last_error = e;
                if r.since.elapsed() >= Duration::from_millis(crate::gpulost::RECOVER_GIVE_UP_MS) {
                    println!(
                        "dc-client: GPU device recovery failed after {} ms ({} attempts): {}",
                        r.since.elapsed().as_millis(),
                        attempt,
                        r.last_error
                    );
                    self.on_gpu_reset(el);
                } else {
                    r.next_at =
                        Instant::now() + Duration::from_millis(crate::gpulost::RECOVER_RETRY_MS);
                    el.set_control_flow(ControlFlow::WaitUntil(r.next_at));
                }
            }
        }
    }

    /// CLI-1593: builds the match renderer again with `settings` on the live device (a new `RenderSettings::msaa`
    /// needs new pipelines): the world renderer, the debug box pass and the effect pass take the new sample count,
    /// the world's render scene is uploaded again and the models and effects upload again from their CPU caches at
    /// the next frame. The surface, the HUD, the overlay and the gamma stage stay. Called by the frame when the
    /// published anti-aliasing count changed ([`App::apply_published_msaa`]); a GPU error unwinds to the caller's guard.
    fn reload_renderer(&mut self, settings: RenderSettings) -> Result<(), String> {
        let live = self.live.as_mut().ok_or("no match window")?;
        let t0 = Instant::now();
        crate::prewarm::load_begins();
        let format = live.config.format;
        let renderer = WorldRenderer::new(&live.ctx, format, settings);
        let fresh = self
            .source
            .reload_scene(GpuTarget {
                ctx: &live.ctx,
                renderer: &renderer,
            })
            .map_err(|e| e.to_string())?;
        live.world.adopt_scene(fresh);
        live.boxes = DebugBoxes::with_msaa(&live.ctx, format, renderer.msaa());
        live.fx.set_msaa(renderer.msaa());
        live.renderer = renderer;
        // the frame makes the multisampled targets of the new count
        live.msaa = None;
        if let Some(m) = live.models.as_mut() {
            m.gpu_reset();
        }
        // gpu-prewarm: the new renderer's on-demand pipelines before the next frame
        crate::prewarm::match_gpu(
            &live.ctx,
            &live.renderer,
            live.models.as_ref(),
            &mut live.fx,
            live.fx_format,
        );
        println!(
            "dc-client: renderer rebuilt for anti-aliasing {}x in {} ms",
            live.renderer.msaa(),
            t0.elapsed().as_millis()
        );
        Ok(())
    }

    /// CLI-1593: the Graphic tab's anti-aliasing count changed since the renderer was built: the renderer is built
    /// again with [`render_settings`] (the count clamped to the device's) before this frame. Nothing while the device
    /// is being rebuilt or no match window is up; one integer compare per frame otherwise.
    /// CLI-1595: the frame-rate setting's vertical-sync choice, applied to the surface in place when it changed
    /// (the flags `--vsync` / `--no-vsync` pin it); the frame clock (CLI-1576) then reads the new mode.
    fn apply_published_present(&mut self) {
        if self.recovering.is_some() {
            return;
        }
        let fifo = wants_fifo(self.opts.vsync, self.opts.no_vsync);
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let want = if !fifo && live.immediate_ok {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::Fifo
        };
        if live.config.present_mode == want {
            return;
        }
        live.config.present_mode = want;
        live.surface.configure(&live.ctx.device, &live.config);
        live.vsync_period_known = false;
        live.vsync_period = None;
        println!("dc-client: presentation {want:?} (frame-rate setting)");
    }

    fn apply_published_msaa(&mut self) {
        if self.recovering.is_some() {
            return;
        }
        let want = crate::settings::antialias().max(1);
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if live.msaa_want == want {
            return;
        }
        live.msaa_want = want;
        let settings = render_settings(&live.ctx.adapter, live.config.format);
        if live.renderer.msaa() == settings.msaa_count() {
            return;
        }
        if let Err(e) = self.reload_renderer(settings) {
            println!("dc-client: anti-aliasing {want}x not applied: {e}");
        }
    }

    /// Builds the window's whole GPU side again on a new device: instance, surface, adapter, device, queue, the
    /// depth target, every renderer, the world's render scene (from the ROM, with the server's state applied again),
    /// and drops the uploads of the models and the effects (they upload again from the CPU caches). The objects of
    /// the lost device are put away without a drop (crate::gpulost::retire); the old surface IS dropped (its swap
    /// chain must go before the window gets a new one; that runs no wgpu-core device path).
    // DC-DEFAULT: CLI-460 (amended by gpu-recover)
    fn rebuild_gpu(&mut self) -> Result<(), String> {
        let live = self.live.as_mut().ok_or("no match window")?;
        let t0 = Instant::now();
        crate::prewarm::load_begins();
        // CLI-1932: from the second attempt on, the saved renderer's other backend first
        let attempt = self.recovering.as_ref().map_or(1, |r| r.attempts);
        let choice = rebuild_choice(self.opts.adapter, self.opts.adapter_first, attempt);
        if choice != self.opts.adapter {
            println!("dc-client: GPU rebuild attempt {attempt} tries the other renderer first");
        }
        let (surface, ctx) = open_gpu(choice, self.opts.adapter_first, &live.window)?;
        // the new device is never dropped either (a queue of a lost device panics in its drop)
        let ctx = std::mem::ManuallyDrop::new(ctx);
        crate::gpulost::watch_keep_lost(&ctx.device);
        crate::diag::note_gpu(&ctx.info);
        let t_device = t0.elapsed().as_millis();
        let caps = surface.get_capabilities(&ctx.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == live.config.format)
            .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
            .or_else(|| caps.formats.first().copied())
            .ok_or("surface has no formats")?;
        let mut config = live.config.clone();
        config.format = format;
        // CLI-1932: the new device may be on the other backend
        config.desired_maximum_frame_latency = dc_render::frame_latency_for(ctx.info.backend);
        if !caps.present_modes.contains(&config.present_mode) {
            config.present_mode = wgpu::PresentMode::Fifo;
        }
        let size = live.window.inner_size();
        config.width = size.width.max(1);
        config.height = size.height.max(1);
        // the old swap chain goes first (one swap chain per window). A frame the loss interrupted may still hold the
        // old surface's image: wgpu-hal then panics in the drop after the swap chain is gone; that is caught here
        let old = std::mem::replace(&mut live.surface, surface);
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(old))).is_err() {
            println!("dc-client: the lost surface did not drop cleanly (an image still acquired); going on");
        }
        live.surface.configure(&ctx.device, &config);
        let depth = depth_view(&ctx.device, config.width, config.height);
        let renderer = WorldRenderer::new(&ctx, format, render_settings(&ctx.adapter, format));
        let overlay = OverlayRenderer::new(&ctx, format);
        let hud_layer = OverlayRenderer::new(&ctx, format);
        // CLI-1593: the passes into the 3D target follow the renderer's sample count
        let boxes = DebugBoxes::with_msaa(&ctx, format, renderer.msaa());
        let gamma = dc_render::GammaStage::new(&ctx, format, live.gamma.gamma());
        let t_scene = t0.elapsed().as_millis();
        let fresh = self
            .source
            .reload_scene(GpuTarget {
                ctx: &ctx,
                renderer: &renderer,
            })
            .map_err(|e| e.to_string())?;
        println!(
            "dc-client: GPU rebuild: device {t_device} ms, renderers {} ms, world scene {} ms",
            t_scene.saturating_sub(t_device),
            t0.elapsed().as_millis().saturating_sub(t_scene)
        );
        // everything is built: the lost device's objects go
        use crate::gpulost::retire;
        live.world.adopt_scene(fresh);
        retire(std::mem::replace(
            &mut live.ctx,
            std::mem::ManuallyDrop::into_inner(ctx),
        ));
        // gpu-lost-menu: what the menu made on the old device (its painter) is made again when the window goes back
        crate::gpulost::device_replaced();
        retire(std::mem::replace(&mut live.depth, depth));
        retire(live.msaa.take());
        live.msaa_want = crate::settings::antialias().max(1);
        retire(std::mem::replace(&mut live.renderer, renderer));
        retire(std::mem::replace(&mut live.overlay, overlay));
        retire(std::mem::replace(&mut live.hud_layer, hud_layer));
        retire(std::mem::replace(&mut live.boxes, boxes));
        retire(std::mem::replace(&mut live.gamma, gamma));
        if let Some(m) = live.models.as_mut() {
            m.gpu_reset();
        }
        live.fx.gpu_reset();
        live.fx.set_msaa(live.renderer.msaa());
        live.fx_format = format;
        // gpu-prewarm: the new device's on-demand pipelines before the first frame on it
        crate::prewarm::match_gpu(
            &live.ctx,
            &live.renderer,
            live.models.as_ref(),
            &mut live.fx,
            format,
        );
        live.config = config;
        live.hud_force = true;
        if let Some(h) = live.hud.as_mut() {
            h.hud.set_size(live.config.width, live.config.height);
        }
        // the frame begun before the loss is not ended on the new device
        if let Some(p) = live.prof.as_mut() {
            p.open = false;
        }
        Ok(())
    }

    fn close(&mut self, el: &ActiveEventLoop, why: &str) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.release(why);
        self.report.exit_reason = why.to_string();
        end_loop(el);
    }

    /// Alt+Enter: the other window look on the monitor the window is on, through the one window (decorations, inner
    /// size, position): no full-screen request and no display-mode change.
    // DC-DEFAULT: CLI-220 (proposed)
    // PUBLIC: winit docs (set_decorations, request_inner_size, set_outer_position, current_monitor; sizes and
    // positions are physical)
    /// CLI-1031: a sensitivity hotkey: the look turns at the new value at once, the HUD shows the line, the menu
    /// takes the value into the settings and the profile.
    // EVIDENCE: cshell+0x49060 (commands 0x6d / 0x6e: applied, stored, CRes 0x4f6 shown 5000 ms)
    fn step_sensitivity(&mut self, step: i32) {
        let v = crate::settings::step_sensitivity(self.opts.look.sensitivity, step);
        self.opts.look.sensitivity = v;
        self.input.set_sensitivity(v);
        crate::settings::note_sensitivity(v as i32);
        if let Some(h) = self.live.as_mut().and_then(|l| l.hud.as_mut()) {
            h.sensitivity_changed(v as i32);
        }
    }

    /// One frame: for a delivered redraw event (`from_loop` false) or drawn by the loop when the events stopped
    /// (CLI-1848); the next redraw is asked for either way.
    fn frame_turn(&mut self, el: &ActiveEventLoop, from_loop: bool) {
        if self.recovering.is_some() {
            // nothing is drawn while the device is rebuilt (about_to_wait runs the attempts)
            return;
        }
        // CLI-890: presentation cap in a match
        self.pacer.wait(crate::pacer::match_cap());
        let drawn = crate::gpulost::guard(|| self.redraw());
        // a loss met on a loader thread marks it without unwinding the frame
        if drawn.is_err() || crate::gpulost::is_lost() {
            self.on_gpu_lost(el);
            return;
        }
        if let Some(l) = &self.live {
            let now = ms_since(l.started);
            if from_loop {
                self.redraw_keep.on_loop_frame(now);
            } else if let Some(n) = self.redraw_keep.on_redraw_event(now) {
                println!("{}", crate::redrawkeep::note_line("dc-client", n));
            }
        }
        if self.deadline_passed() {
            self.close(el, "deadline");
        } else if let Some(l) = &self.live {
            l.window.request_redraw();
        }
    }

    /// CLI-1848: the loop draws the frame itself when no redraw event came for a while and the window is visible
    /// (no window query while the events come).
    fn keep_frames(&mut self, el: &ActiveEventLoop) {
        // the iPad's driver raises every frame from its tick (CLI-1604)
        if IOS || self.closing || self.recovering.is_some() {
            return;
        }
        let Some(l) = self.live.as_ref() else {
            return;
        };
        let now = ms_since(l.started);
        if !self.redraw_keep.due(now) {
            return;
        }
        let visible = window_visible(&l.window);
        let (wake, note) = self.redraw_keep.poll(now, visible);
        if let Some(n) = note {
            println!("{}", crate::redrawkeep::note_line("dc-client", n));
        }
        if wake == crate::redrawkeep::Wake::Draw {
            self.frame_turn(el, true);
        }
    }

    fn toggle_look(&mut self) {
        let Some(plan) = self
            .live
            .as_ref()
            .and_then(|l| toggle_and_note(&l.window, &self.opts))
        else {
            return;
        };
        self.target = (plan.width, plan.height);
        // the keys held across the change may never see their release; the cursor grab is taken again at the new size
        if self.live.as_ref().is_some_and(|l| l.captured) {
            self.release("window look");
            self.recapture = true;
        } else {
            self.input.device(DeviceInput::ReleaseAll);
        }
    }

    fn start(&mut self, el: &ActiveEventLoop) -> Result<Live, ClientError> {
        crate::prewarm::load_begins();
        // perf-g1: the profiler's load phase starts before the device exists (pipelines, world, tables)
        let mut prof_rec = crate::prof::on().then(crate::prof::Recorder::new);
        if let Some(p) = prof_rec.as_mut() {
            p.load_begin();
        }
        // CLI-1619: the match's mods before anything reads a file (models, sounds, HUD textures)
        if self.mods.is_none() {
            self.mods = crate::modassets::MatchMods::open(self.opts.mods.as_ref());
        }
        // w2-loading: the menu's window is taken over when the menu hands it in (no new window, no window swap)
        let host = match self.host_in.take() {
            Some(h) => {
                self.created_window = false;
                h
            }
            None => {
                self.created_window = true;
                open_host(el, &self.opts)?
            }
        };
        let WindowHost {
            window,
            surface,
            mut config,
            ctx,
            readable,
            immediate_ok,
        } = host;
        let format = config.format;
        let present_mode = config.present_mode;
        // the real inner size is what the surface, the depth buffer and the HUD follow
        let size = window.inner_size();
        self.target = (size.width.max(1), size.height.max(1));
        if (size.width.max(1), size.height.max(1)) != (config.width, config.height) {
            config.width = size.width.max(1);
            config.height = size.height.max(1);
            surface.configure(&ctx.device, &config);
        }
        let depth = depth_view(&ctx.device, config.width, config.height);
        let renderer = WorldRenderer::new(&ctx, format, render_settings(&ctx.adapter, format));
        let overlay = OverlayRenderer::new(&ctx, format);
        let hud_layer = OverlayRenderer::new(&ctx, format);
        // CLI-1593: the passes into the 3D target follow the renderer's sample count
        let boxes = DebugBoxes::with_msaa(&ctx, format, renderer.msaa());
        let world = self.source.load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))?;
        println!("dc-client: {}", world.summary());
        let gamma = dc_render::GammaStage::new(
            &ctx,
            format,
            crate::display::client_gamma(self.source.rom().as_deref()),
        );
        println!("dc-client: gamma {}", gamma.gamma());
        let models = match self.source.rom() {
            Some(rom) => match ModelScene::open(&rom) {
                Ok(mut m) => {
                    // models load on loader threads in the window (DC_ASYNC_LOADS=0: inline)
                    m.set_async_loads(crate::characters::async_loads_from_env(true));
                    Some(m)
                }
                Err(e) => {
                    self.report.models_note = Some(format!("models not loaded: {e}"));
                    None
                }
            },
            None => {
                self.report.models_note = Some("no ROM: remote players as boxes".into());
                None
            }
        };
        let font = match &self.opts.font_rom {
            Some(rom) => match load_rom_font(rom) {
                Ok(f) => {
                    println!("dc-client: overlay {}", f.source);
                    self.report.font = Some(f.source.clone());
                    Some(f)
                }
                Err(e) => {
                    println!("dc-client: overlay font not loaded: {e}");
                    self.report.font = Some(format!("not loaded: {e}"));
                    None
                }
            },
            None => None,
        };
        let hud_font = self
            .opts
            .font_rom
            .as_ref()
            .and_then(|r| crate::overlay::load_rom_font_bytes(r));
        let mut hud =
            crate::hud::HudDriver::open(&self.opts.hud, config.width, config.height, hud_font).map(
                |(mut d, src)| {
                    println!("dc-client: HUD {src}");
                    self.report.hud_layout = Some(src);
                    d.set_reality(self.source.reality());
                    // DC-DEFAULT: CLI-1639 (proposed) the ladder's names from the ROM shop
                    if !self.opts.gun_ladder.is_empty() {
                        d.set_gun_game(crate::hud::gun_ladder_names(
                            self.opts.font_rom.as_deref(),
                            &self.opts.gun_ladder,
                        ));
                    }
                    // DC-DEFAULT: CLI-1878 (proposed) the reticle item in use when the match starts
                    // EVIDENCE: cshell+0x1630a0 (the match entry: the in-use function record of kind 11 -> the mark's
                    // shape by the item's ShapeType and its colour by the record's colour index)
                    if let Some((shape, argb)) = self.opts.reticle {
                        d.set_reticle(shape, argb);
                    }
                    d.set_draw_voice(self.source.draw_voice());
                    // CLI-1570: the network overlay starts as the flag / env / last choice says
                    d.set_net_mode(crate::settings::net_overlay_start());
                    // CLI-1613 / CLI-1614 / CLI-1620: the set's HUD keys, hud.draw and render.entity_visibility
                    if let Some(m) = &self.mods {
                        m.wire(&mut d);
                    }
                    // DC-DEFAULT: CLI-1627 (proposed) a room without wallhacks: no x-ray bridge for this match
                    if !room_wallhacks(self.opts.room_hacks) {
                        d.xray = None;
                    }
                    d
                },
            );
        // client-fps: the radar map layer is composed off the frame thread while the player moves
        if let Some(r) = hud.as_mut().and_then(|h| h.hud.radar.as_mut()) {
            r.compose_in_background();
        }
        let spawn = world.spawn(self.opts.spawn)?;
        let mut player = LocalPlayer::spawn(
            self.opts.params,
            &world.collision,
            &spawn,
            world.stand_dims,
            self.opts.tick_hz,
        );
        player.volumes = world.containers.clone();
        player.state.crouch_dims = world.crouch_dims;
        // sim-sprint-jump: the held weapon's and the body's speed rows from the ROM (MOVEMENT_2009.md §7)
        player.load_speed_rows(self.source.rom().as_deref());
        let start_view = ViewAngles {
            yaw: spawn.yaw,
            pitch: 0.0,
        };
        if self.opts.shoot_test {
            let walk = self.opts.walk_test.then(|| WalkTest::new(start_view));
            self.input = Box::new(ShootTest::new(1.0 / 60.0, walk));
        } else if self.opts.walk_test {
            self.input = Box::new(WalkTest::new(start_view));
        } else if self.opts.scripted {
            self.input = Box::new(ScriptedInput::new(start_view));
        } else if let Some(i) = self.source.scripted_input(start_view) {
            // a source with its own input (the stress scenarios, perf-g1); like the scripted inputs it needs no
            // cursor grab
            self.input = i;
            self.opts.scripted = true;
        }
        // a step file drives the match (gate T2): its inputs, never the OS devices
        if crate::steps::active() {
            self.input = Box::new(crate::steps::StepInput::new(self.opts.look));
        }
        crate::events::screen("game");
        crate::events::emit(
            "cursor",
            serde_json::json!({ "image": "hidden-when-captured", "visible": true, "grabbed": false, "screen": "game" }),
        );
        self.report.adapter = ctx.describe();
        self.report.present_mode = format!("{present_mode:?}");
        // gpu-prewarm: every on-demand pipeline of the match on this (the frame) thread before the first frame
        let mut fx = crate::fx::FxStage::open(self.source.rom().as_deref());
        fx.set_msaa(renderer.msaa());
        crate::prewarm::match_gpu(&ctx, &renderer, models.as_ref(), &mut fx, format);
        if let Some(p) = prof_rec.as_mut() {
            p.adapter = format!("{} {present_mode:?}", ctx.describe());
            p.load_end(Some(&ctx.device));
        }
        self.report.world = world.summary();
        self.report.start = player.state.pos.to_array();
        window.request_redraw();
        let now = Instant::now();
        let mut carried = self.carried_sound.take();
        // fix-first-match-drop: the menu's director, carried into the match, decodes ahead and reads the footstep rigs
        // on its threads too (it ran inline before: every new body model's rig was read on the frame thread when the
        // remote players appeared, 0.6 s each in a debug build, and the frame watchdog ended the process after 8 s)
        // DC-DEFAULT: CLI-1700 (proposed, project decision) no first-sight ROM reads of the sound layer on the frame
        if let Some(d) = carried.as_mut() {
            d.set_async_decode(crate::characters::async_loads_from_env(true));
        }
        let sound = carried.or_else(|| {
            if self.carries_sound || !self.opts.sound.enabled {
                return None;
            }
            let rom = self.source.rom();
            let mut d = crate::sound::SoundDirector::open(rom.as_deref());
            // sound files decode ahead on a thread in the window (perf-hitch; DC_ASYNC_LOADS=0 turns it off)
            d.set_async_decode(crate::characters::async_loads_from_env(true));
            if let Some(p) = self.opts.sound.log.as_deref() {
                let _ = d.log_to(p);
            }
            let note = d.configure(&self.opts.audio);
            println!("{note}");
            self.report.audio_out.push(note);
            Some(d)
        });
        Ok(Live {
            watch_dead: false,
            watch_remotes: 0,
            window,
            surface,
            config,
            depth,
            msaa: None,
            msaa_want: crate::settings::antialias().max(1),
            immediate_ok,
            ctx,
            renderer,
            overlay,
            hud_layer,
            hud,
            board_held: false,
            boxes,
            models,
            fx,
            fx_format: format,
            font,
            view: ViewAngles {
                yaw: spawn.yaw,
                pitch: 0.0,
            },
            twist: crate::aim::OwnTwist::default(),
            world,
            player,
            death_cam: crate::deathcam::DeathCam::default(),
            cam_shot: crate::camshot::CamShot::default(),
            fps: FpsMeter::default(),
            started: now,
            last: now,
            frame_clock: crate::frameclock::FrameClock::default(),
            render_last: now,
            vsync_period: None,
            vsync_period_known: false,
            focused: false,
            captured: false,
            readable,
            sound,
            audio_status_at: self.opts.audio.status_every_s,
            surface_type: 0,
            gamma,
            times: crate::frametimes::FrameTimes::default(),
            laps: crate::prof::Laps::start(),
            prof: prof_rec.map(|mut rec| {
                rec.reserve(600_000);
                rec.size = (size.width, size.height);
                rec.note = format!(
                    "window {} {}x{}",
                    self.report.world, size.width, size.height
                );
                WindowProf {
                    events: 0,
                    rec,
                    open: false,
                    remotes: 0,
                }
            }),
            hud_force: false,
            net_refresh: None,
            fps_refresh: None,
            mouse_count: 0,
            mouse_t0: None,
            mouse_hz: None,
        })
    }

    /// A press for the match's chat box (a key, or a step's `type` / `key enter`): the box opens, edits or sends;
    /// true when the box consumed it (the game does not see it).
    fn chat_press(&mut self, press: crate::hud::chat::ChatPress) -> bool {
        let hud = self.live.as_mut().and_then(|l| l.hud.as_mut());
        let Some(o) = hud.map(|h| h.chat_press(press)) else {
            return false;
        };
        if o.opened {
            self.input.device(DeviceInput::ReleaseAll);
        }
        if let Some((line, mode)) = o.send {
            self.source.chat(&line, mode);
        }
        o.consumed
    }

    fn redraw(&mut self) {
        // CLI-1593: a changed anti-aliasing setting rebuilds the renderer before this frame
        self.apply_published_present();
        self.apply_published_msaa();
        // a step file's chat presses (`type`, `key enter`) reach the chat box as the keys would
        for press in crate::steps::take_chat() {
            self.chat_press(press);
        }
        // the report form's caret blinks: its list is handed over again every frame while it is open
        if self.report_form.is_some() {
            self.report_overlay();
        }
        // CLI-1907: the options' list too (caret, the mod page's preview burst)
        if self.options_up {
            self.options_overlay();
        }
        let Some(live) = self.live.as_mut() else {
            return;
        };
        // CLI-1848: shown / focused again or a stalled redraw chain: the surface again at the window's real size
        if self.redraw_keep.take_reconfigure() {
            let size = live.window.inner_size();
            fit_surface(live, size);
        }
        use crate::prof::Stage as PS;
        crate::watchdog::frame();
        crate::watchdog::phase(crate::watchdog::Phase::Play);
        crate::watchdog::selftest(live.started.elapsed().as_millis() as u64);
        if crate::gpulost::test_due(live.started.elapsed().as_millis() as u64) {
            eprintln!("dc-client: DC_GPU_LOSS_TEST_AT: provoking a GPU reset (test)");
            crate::gpulost::provoke(&live.ctx.device, &live.ctx.queue);
        }
        if crate::gpulost::fault_due(live.started.elapsed().as_millis() as u64) {
            crate::gpulost::simulate_loss();
        }
        let now = Instant::now();
        let dt_raw = now - live.last;
        live.last = now;
        if dt_raw.as_millis() >= 100 {
            crate::watchdog::event(crate::watchdog::EV_FRAME_MS, dt_raw.as_millis() as u64);
        }
        live.fps.push(dt_raw.as_secs_f64());
        live.times.push_ms(dt_raw.as_secs_f64() * 1000.0);
        // CLI-1576: a blank-synced frame is simulated and drawn for its predicted presentation time
        if !live.vsync_period_known {
            live.vsync_period_known = true;
            if live.config.present_mode == wgpu::PresentMode::Fifo {
                live.vsync_period = monitor_refresh_mhz(&live.window)
                    .filter(|&m| m >= 1000)
                    .map(|m| Duration::from_secs_f64(1000.0 / m as f64));
            }
        }
        live.frame_clock.note_raw(dt_raw, live.vsync_period);
        let render_now = live.frame_clock.next(now, live.vsync_period);
        let lead_ms = (crate::frameclock::signed_secs(render_now, now) * 1000.0) as f32;
        let dt = render_now.saturating_duration_since(live.render_last);
        live.render_last = render_now;
        if let Some(p) = live.prof.as_mut() {
            if p.open {
                live.laps.lap(PS::Loop);
                p.rec.end_with(p.remotes, Some(&live.ctx.device));
            }
            p.rec.begin(Some(&live.ctx.device));
            p.open = true;
            live.laps.reset();
            crate::prof::note(
                crate::prof::Note::WindowEvents,
                std::mem::take(&mut p.events),
            );
        }
        self.input.frame(&mut live.view);
        live.laps.lap(PS::Input);
        for c in self.source.poll_corrections() {
            live.player.apply_correction(&live.world.collision, &c);
            self.report.corrections += 1;
            crate::prof::note(crate::prof::Note::Corrections, 1);
        }
        let mut fs = self.source.frame_state_led(lead_ms);
        crate::prof::note(crate::prof::Note::HudEvents, fs.hud_events.len());
        crate::prof::note(crate::prof::Note::FxEvents, fs.fx_events.len());
        crate::prof::note(crate::prof::Note::SoundCues, fs.sound_cues.len());
        if fs.session_over && self.report.session_over_s.is_none() {
            self.report.session_over_s = Some(live.started.elapsed().as_secs_f64());
        }
        // DC-DEFAULT: CLI-230 (proposed) the server's world models; CLI-232 the effect player's call site
        if let Some(o) = &fs.world_objects {
            let fresh = live.world.apply_objects(o);
            // CLI-1369: a body standing on a model that left the collision set falls
            live.player
                .detach_from_models(live.world.world_models.left());
            crate::objects::play_world_effects(&fresh, &mut fs.fx_events);
        }
        // client-hudgaps: the server's sky table (view box, sky objects)
        if let Some(sky) = &fs.sky {
            live.world.apply_sky(sky);
        }
        for (kind, a) in &fs.view_sets {
            apply_view_set(&mut live.view, *kind, *a);
            live.twist.reset(live.view.yaw);
        }
        live.player.follow(&fs);
        if let Some(ls) = fs.life_state {
            if self.report.life_state != Some(ls) {
                crate::watchdog::event(crate::watchdog::EV_LIFE_STATE, ls as u64);
            }
            self.report.life_state = Some(ls);
        }
        if fs.death.is_some() != live.watch_dead {
            live.watch_dead = fs.death.is_some();
            crate::watchdog::event(crate::watchdog::EV_DEAD_VIEW, live.watch_dead as u64);
        }
        if fs.remotes.len() != live.watch_remotes {
            live.watch_remotes = fs.remotes.len();
            crate::watchdog::event(crate::watchdog::EV_REMOTES, live.watch_remotes as u64);
        }
        if !fs.remotes.is_empty() {
            self.report.remote_frames += 1;
        }
        if let Some(p) = live.prof.as_mut() {
            p.remotes = fs.remotes.len();
        }
        live.laps.lap(PS::Net);
        if let Some(h) = live.hud.as_mut() {
            let local_ms = live.started.elapsed().as_millis().min(u32::MAX as u128) as u32;
            let t = live.started.elapsed().as_secs_f32();
            let board =
                live.board_held || self.opts.board_at.is_some_and(|(a, b)| (a..b).contains(&t));
            h.update(
                &fs,
                local_ms,
                live.view.yaw,
                live.player.state.crouch_held,
                board,
            );
            // the stamina bar follows the sprint gauge (CLI-589)
            h.set_stamina(live.player.stamina_level());
            // the kill banners' voice lines play with this frame's sounds
            fs.sound_cues.extend(h.take_sound_cues());
            fs.sound_cues.append(&mut self.ui_cues);
            if board {
                self.report.board_frames += 1;
            }
        }
        live.laps.lap(PS::HudUpdate);
        let eye_now = live.player.eye_target(&live.view);
        self.input
            .aim(eye_now.to_array(), &fs.remotes, fs.own_team, &mut live.view);
        let mut moves: Vec<dc_sim::StepEvents> = Vec::new();
        live.player.advance(
            &live.world.collision,
            dt.as_micros() as u64,
            self.input.as_mut(),
            &mut live.view,
            &mut |r| {
                if r.events.notice_codes().next().is_some() {
                    moves.push(r.events);
                }
            },
        );
        for m in &moves {
            self.source.moved(m);
        }
        live.laps.lap(PS::Sim);
        let weapon_cmd = self.input.weapon();
        // the zoom changes run against the frame's normal horizontal field (cshell+0x1f9100)
        let normal_fov_x = dc_render::fov_pair_2009(
            self.opts.view.fov_y_deg,
            live.config.width,
            live.config.height,
        )
        .0;
        let kick = self.source.arms(&ArmsFrame {
            commands: weapon_cmd,
            aim: shot_aim(&live.player.state, &live.view),
            body: live.player.state.pos.to_array(),
            remotes: &fs.remotes,
            world: &live.world.collision,
            motion: crate::spread::Motion::of(
                &live.player.state,
                &live.player.params,
                live.view.yaw,
                live.view.pitch,
            ),
            loaded: Some(&live.world),
            anims: Some(&fs.anims),
            normal_fov_x,
        });
        // the scope: the mouse turns 1 / (3 z + 1) as fast, a zoomed body walks instead of sprinting
        // SPEC: MOVEMENT_2009.md §13.2 step 2, §7 step 2 (+0xf0 >= 1 or +0xf4 != 0 blocks the sprint speed)
        self.input.set_zoom_level(kick.zoom.level);
        live.player.sprint_blocked = kick.zoom.active;
        crate::headless::apply_recoil(&mut live.view, kick.view_pitch);
        crate::headless::apply_recoil_yaw(&mut live.view, kick.view_yaw);
        live.laps.lap(PS::Arms);
        // the own twist: the body stays put while the standing view turns (CLI-636)
        live.twist.step(live.view.yaw, live.player.state.controls);
        let mut update = LocalUpdate::from_state(
            &live.player.state,
            &live.view,
            live.player.state.clock as f32,
        )
        .with_twist(live.view.yaw, &live.twist);
        if let Some(t) = live.world.surface_under(&live.player.state) {
            live.surface_type = t;
        }
        update.surface = live.surface_type;
        self.source.report(&update);
        live.laps.lap(PS::Net);
        // the step-up smoothing runs per tick inside the player (CLI-650)
        let eye = live.player.eye(&live.view);
        // dead: the death camera replaces the own view (CLI-85)
        let (eye, cam_view) = live
            .death_cam
            .frame(
                fs.death.as_ref(),
                &fs.remotes,
                eye,
                live.player.render_pos(),
                &live.view,
                Some(&live.world.collision),
                dt.as_secs_f32(),
            )
            .unwrap_or((eye, live.view));
        // the death panel shows the followed player (CLI-128)
        if let Some(h) = live.hud.as_mut() {
            h.set_observed(crate::deathcam::observed(live.death_cam.mode(), &fs.remotes));
        }
        // the own camera draws the hit jar's offsets and the recoil's kick-back (crate::camfx)
        let own_view = fs.death.is_none();
        let (eye, cam_view) = if own_view {
            crate::camfx::own_camera(eye, &cam_view, kick.camera, kick.eye_back)
        } else {
            (eye, cam_view)
        };
        live.laps.lap(PS::Sim);
        let fx_ms = fs
            .clock_ms
            .unwrap_or(live.started.elapsed().as_millis().min(u32::MAX as u128) as u32);
        // CLI-1591: the remote shooters' drawn weapon muzzle sockets (as posed last frame) for this frame's records
        live.fx.runner.remote_muzzle.clear();
        if let Some(m) = live.models.as_ref() {
            for e in &fs.fx_events {
                let crate::fx::FxEvent::Record(r) = e else { continue };
                if r.own {
                    continue;
                }
                if let Some((inst, files, _)) = m.remote_weapon_parts(r.shooter) {
                    if let Some(t) = crate::fx::view_muzzle_socket(inst, &files.body) {
                        live.fx.runner.remote_muzzle.insert(r.shooter, t);
                    }
                }
            }
        }
        // CLI-1136 (amended): a flown object with an effect and no model is its effect only, no body
        let bodiless = live.fx.runner.tables.bodiless_thrown(&fs.fx_events);
        if !bodiless.is_empty() {
            fs.thrown.retain(|b| !bodiless.contains(&b.obj));
        }
        let fx_cues = live.fx.frame(fx_ms, &fs, &live.world);
        live.laps.lap(PS::Fx);
        let cues: Vec<crate::sound::SoundCue> =
            fs.sound_cues.iter().cloned().chain(fx_cues).collect();
        // a live world camera replaces the own and the death camera (CLI-240)
        let (eye, cam_view, shot_fov) = match live.cam_shot.frame(fs.cam_shot.as_ref()) {
            Some((e, v, fov)) => (e, v, Some(fov)),
            None => (eye, cam_view, None),
        };
        // w2-fx: the effects' camera shake and model lights on the frame's final eye (crate::fx)
        let eye = live.fx.camera_frame(fx_ms, eye, &live.world);
        if let Some(d) = live.sound.as_mut() {
            let local_ms = live.started.elapsed().as_millis().min(u32::MAX as u128) as u32;
            let st = &live.player.state;
            d.frame(&crate::sound::AudioFrame {
                now_ms: fs.clock_ms.unwrap_or(local_ms),
                eye: eye.to_array(),
                yaw: cam_view.yaw,
                pitch: cam_view.pitch,
                cues: &cues,
                remotes: &fs.remotes,
                own: Some(crate::sound::OwnAudio {
                    look: fs.own_look.as_ref(),
                    obj: fs.own_object,
                    feet: [st.pos.x, st.pos.y - st.dims.y, st.pos.z],
                    surface: update.surface,
                    control: st.controls,
                    on_ground: st.on_ground,
                    airborne: st.jumping,
                    sprinting: st.stamina.mode == dc_sim::GaugeMode::Sprint,
                    alive: fs.life_state.is_none_or(|s| s == 1),
                    jumped: moves.iter().any(|m| m.notice_codes().any(|c| c == 1)),
                    speed: st.vel.len(),
                    life_state: fs.life_state,
                }),
                world: Some(&live.world),
            });
            if crate::events::on() {
                crate::events::sound_poll(&d.mixer());
            }
            let every = self.opts.audio.status_every_s;
            let t = live.started.elapsed().as_secs_f64();
            if every > 0.0 && t >= live.audio_status_at {
                println!("{:7.1} s {}", t, d.output_line());
                live.audio_status_at = t + every;
            }
        }
        live.laps.lap(PS::Sound);
        let (w, h) = (live.config.width, live.config.height);
        let mut camera = crate::camshot::view_settings(
            self.opts.view.with_world_far(live.world.far_z),
            shot_fov,
        )
        .camera(eye, &cam_view, w, h);
        if own_view && shot_fov.is_none() {
            crate::camfx::roll_camera(&mut camera, kick.camera.roll);
            // the scope's field of view (cshell+0x1f9100)
            kick.zoom.apply(&mut camera);
        }
        let mut drawn: Vec<u16> = Vec::new();
        let mut fx_socket = None;
        if let Some(m) = live.models.as_mut() {
            let lights = live.world.model_lights.as_ref();
            let bodies = crate::fx::with_thrown(&fs.remotes, &fs.thrown);
            // client-hudgaps: the team-mate mark
            m.own_team = fs.own_team;
            // CLI-1635: a declared x-ray server sends every player of the map: the ones outside the view cost no pose
            m.view_cull = live
                .hud
                .as_ref()
                .filter(|h| h.xray.is_some())
                .map(|_| crate::characters::ViewCull::of(&camera));
            m.update_remotes_animated(&live.ctx, &live.renderer, &bodies, &fs.anims, lights);
            m.place_remote_attachments(&live.ctx, &live.renderer, &fs.remote_attach);
            m.place_carried(
                &live.ctx,
                &live.renderer,
                &fs.remote_carried,
                &fs.remote_gear,
            );
            live.laps.lap(PS::Models);
            // the dropped weapons ([`crate::drops`]), moved on the source's clock
            let drop_ms = fs
                .clock_ms
                .unwrap_or(live.started.elapsed().as_millis().min(u32::MAX as u128) as u32);
            let cw: &dyn dc_sim::Collider = &live.world.collision;
            m.update_drops(
                &live.ctx,
                &live.renderer,
                &fs.drops,
                drop_ms,
                Some(cw),
                lights,
            );
            // client-destructfx: the effect models (a breaking crate's pieces, a launched drum)
            let fx_list = live.fx.runner.models_at(fx_ms);
            m.update_fx_models(&live.ctx, &live.renderer, &fx_list, lights);
            live.laps.lap(PS::Drops);
            m.set_own_look(fs.own_look.as_ref());
            m.place_view_weapon(
                &live.ctx,
                &live.renderer,
                fs.view_weapon.as_ref(),
                glam::Vec3::new(eye.x, eye.y, eye.z),
                lights,
            );
            m.place_view_attachments(&live.ctx, &live.renderer, &fs.view_attach);
            m.set_view_hidden(fs.view_hidden);
            live.laps.lap(PS::ViewWeapon);
            drawn = m.drawn();
            self.report.models_drawn += m.count() as u64;
            self.report.characters_drawn += m.stats.characters as u64;
            self.report.view_weapon_frames += m.stats.view_weapon as u64;
            fx_socket = live.fx.view_socket(m);
        }
        live.laps.lap(PS::Models);
        self.watch.frame(&fs);
        self.watch.corpses(&fs, &drawn);
        crate::heartbeat::corpses(&fs, &drawn);
        let mut boxes = box_list(&fs.remotes, &drawn, self.opts.debug_boxes);
        // MAP-34: the capture flags without a model, as bars in the team colour
        boxes.extend(crate::world::flag_marker_boxes(&fs.flag_markers));
        // client-tags2: the names stand on the posed name sockets of the characters just posed
        // DC-DEFAULT: CLI-371 (proposed)
        let name_points = live
            .models
            .as_ref()
            .map(|m| m.name_points())
            .unwrap_or_default();
        if let Some(h) = live.hud.as_mut() {
            h.set_cases(&fs.hud_cases, &camera);
            h.set_flags(&fs.hud_flags, &camera);
            h.set_marks_at(
                &fs.remotes,
                &camera,
                Some(&live.world.collision),
                &name_points,
            );
            h.set_radar(&fs.remotes, &camera);
            // CLI-1620: a declared x-ray server's boxes (no bridge: no call)
            h.set_entity_visibility(
                &fs.remotes,
                &camera,
                Some(&live.world.collision),
                fs.own_object.map(|o| (o, fs.own_team.unwrap_or(0))),
            );
            // CLI-1571: the network overlay's lines every 250 ms (at once after a mode change)
            let mode = h.state.net_mode;
            if mode != crate::hud::netview::OFF {
                let now_ms = h.state.now_ms;
                let due = live.net_refresh.is_none_or(|(t, m)| {
                    m != mode || now_ms.wrapping_sub(t) >= crate::hud::netview::REFRESH_MS
                });
                if due {
                    let sample = self
                        .source
                        .net_sample(mode >= crate::hud::netview::PLAYERS);
                    // a mouse silent for two seconds reads as no rate
                    let mouse = live
                        .mouse_t0
                        .filter(|t| t.elapsed() < Duration::from_secs(2))
                        .and(live.mouse_hz);
                    let lines = crate::hud::netview::lines(
                        sample.as_ref(),
                        &h.state,
                        live.fps.fps() as f32,
                        mouse,
                        mode,
                    );
                    h.set_net_lines(lines);
                    live.net_refresh = Some((now_ms, mode));
                }
            } else {
                live.net_refresh = None;
            }
            // CLI-1596: the "Show FPS" line, refreshed four times a second
            if crate::settings::show_fps() {
                let now_ms = h.state.now_ms;
                if live.fps_refresh.is_none_or(|t| {
                    now_ms.wrapping_sub(t) >= crate::hud::netview::REFRESH_MS
                }) {
                    h.set_fps_line(Some(format!("{:.0} fps", live.fps.fps())));
                    live.fps_refresh = Some(now_ms);
                }
            } else if h.state.fps_line.is_some() {
                h.set_fps_line(None);
                live.fps_refresh = None;
            }
            live.laps.lap(PS::HudMarks);
            // the display list as batched quads (overlay::batch), uploaded only when it changed (the steps of
            // `HudDriver::draw_into`, lapped apart: client-fps)
            if let Some(items) = h.items_due(std::mem::take(&mut live.hud_force)) {
                live.laps.lap(PS::HudItems);
                let size = h.hud.size();
                live.hud_layer
                    .update_items(&live.ctx, &items, &mut h.hud.assets, size);
            }
            self.report.hud_events = h.events_applied;
            self.report.hud_composed = h.composed;
        }
        live.laps.lap(PS::HudCompose);
        if let (Some(font), true) = (&live.font, self.opts.debug_panel) {
            let mut lines = overlay_lines(
                &live.world.name,
                &live.player,
                &live.view,
                live.fps.fps(),
                self.input.label(),
            );
            lines.extend(fs.hud.iter().cloned());
            live.overlay.update_panel(&live.ctx, &font.atlas, &lines);
        }
        if !fs.hud.is_empty() {
            self.report.hud = fs.hud.clone();
        }
        live.laps.lap(PS::Panel);
        let frame = match dc_hangdump::crumb_wait!(
            "frame: acquire the surface texture",
            live.surface.get_current_texture()
        ) {
            Ok(f) => {
                self.redraw_keep.on_acquire(true);
                f
            }
            // CLI-1848: Timeout / Outdated / Lost / OutOfMemory / Other: the surface is configured again and the
            // frame skipped (the next one acquires from the new swapchain)
            // PUBLIC: wgpu 25 docs (SurfaceError)
            Err(e) => {
                if self.redraw_keep.on_acquire(false) {
                    println!(
                        "dc-client: surface texture not acquired ({e:?}): surface configured again"
                    );
                }
                live.surface.configure(&live.ctx.device, &live.config);
                live.laps.lap(PS::Acquire);
                return;
            }
        };
        live.laps.lap(PS::Acquire);
        let screen = frame.texture.create_view(&Default::default());
        // SPEC: RENDER_2009.md §2.5 (the frame is drawn into the gamma stage's target and copied to the screen
        // through the ramp; gamma 1 draws straight to the screen)
        // CLI-1140: the options page's gamma slider moves the ramp at once
        if let Some(g) = crate::settings::published_gamma() {
            if (g - live.gamma.gamma()).abs() > 1e-4 {
                live.gamma.set_gamma(&live.ctx, g);
            }
        }
        let gamma_on = live.gamma.active();
        if gamma_on {
            live.gamma.prepare(&live.ctx, (w, h));
        }
        // CLI-1593: the multisampled 3D target of the renderer's sample count (none with MSAA off; made again when
        // the size or the count changed); the effect pass follows the count
        dc_render::MsaaTargets::prepare(
            &mut live.msaa,
            &live.ctx,
            live.config.format,
            (w, h),
            live.renderer.msaa(),
        );
        live.fx.set_msaa(live.renderer.msaa());
        let view = match (gamma_on, live.gamma.view()) {
            (true, Some(v)) => v,
            _ => &screen,
        };
        let mut enc = live
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("client window frame"),
            });
        if let Some(scene) = &live.world.scene {
            let models = live
                .models
                .as_ref()
                .map(|m| m.instances())
                .unwrap_or_default();
            let view_models = live
                .models
                .as_ref()
                .map(|m| m.view_instances())
                .unwrap_or_default();
            // CLI-1593: with MSAA the 3D passes draw into the multisampled target and the LAST of them resolves into
            // `view` (the gamma target or the screen): the effect pass when it has quads, else the box pass when it
            // has boxes, else the world pass; the HUD passes after them draw into the resolved `view`. With MSAA off
            // every pass gets the single-sample target exactly as before.
            let fx_draws = live.fx.prepare(&camera, fx_ms, fx_socket);
            let boxes_draw = !boxes.is_empty();
            // CLI-1624: the chams / skeleton players of a declared x-ray server, each with its posed instances
            let xray: Vec<dc_render::XrayModel<'_>> = match (&live.hud, &live.models) {
                (Some(h), Some(m)) => h
                    .xray_draws
                    .iter()
                    .filter_map(|d| {
                        let instances = m.character_instances(d.entity);
                        (!instances.is_empty()).then_some(dc_render::XrayModel {
                            instances,
                            mode: d.mode,
                            primary: d.primary,
                            secondary: d.secondary,
                        })
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let xray_draw = !xray.is_empty();
            let pass_target = |resolves: bool| match &live.msaa {
                Some(ms) => ms.target(resolves.then_some(view)),
                None => FrameTarget::new(view, &live.depth, (w, h)),
            };
            // SPEC: RENDER_2009.md §4.2 / §4.3 (the own weapon's first-person model with the player-view camera)
            let report = live.renderer.encode_frame_with_view(
                &live.ctx,
                scene,
                &camera,
                pass_target(!fx_draws && !boxes_draw && !xray_draw),
                // SPEC: RENDER_2009.md §5.1 step 3 / §5.3 (sky pass before the world), §7.1 steps 7-9 (translucent and additive
                // world models); DC-DEFAULT: CLI-211 (proposed), see the staged rows (the client draws them like the viewer)
                &FrameOptions::default(),
                &models,
                &view_models,
                &dc_render::PLAYER_VIEW_DEFAULT,
                &mut enc,
            );
            // the frame profiler's command counters (perf-g1: the window books them too)
            crate::prof::count_frame(&report);
            if xray_draw {
                live.renderer.encode_xray(
                    &live.ctx,
                    &camera,
                    pass_target(!fx_draws && !boxes_draw),
                    &xray,
                    &mut enc,
                );
            }
            live.boxes.encode(
                &live.ctx,
                &mut enc,
                pass_target(!fx_draws && boxes_draw),
                &camera,
                &boxes,
            );
            self.report.boxes_drawn += live.boxes.last_drawn as u64;
            live.fx.encode(
                &live.ctx,
                live.fx_format,
                &mut enc,
                pass_target(fx_draws),
                &camera,
                fx_ms,
                fx_socket,
            );
        }
        live.laps.lap(PS::Encode);
        if live.hud.is_some() {
            live.hud_layer
                .encode(&live.ctx, &mut enc, view, (w, h), (0, 0));
        }
        if live.font.is_some() && self.opts.debug_panel {
            live.overlay
                .encode(&live.ctx, &mut enc, view, (w, h), (8, 8));
        }
        if gamma_on {
            live.gamma.encode(&mut enc, &screen);
        }
        // perf-g1: the frame's pass timestamps are resolved in its own encoder (no-op without the profiler's timer)
        dc_render::perf::gpu_frame_end(&mut enc);
        live.laps.lap(PS::HudUpload);
        dc_hangdump::crumb_wait!("frame: queue submit", live.ctx.queue.submit([enc.finish()]));
        dc_render::perf::gpu_frame_submitted();
        live.laps.lap(PS::Submit);
        if let Some(name) = crate::steps::take_shot() {
            if live.readable {
                match crate::steps::read_texture(
                    &live.ctx,
                    &frame.texture,
                    live.config.format,
                    w,
                    h,
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
        dc_hangdump::crumb_wait!("frame: present", frame.present());
        // CLI-1932: enough frames shown: the start finished
        crate::startguard::presented();
        live.laps.lap(PS::Present);
    }

    fn deadline_passed(&self) -> bool {
        match (self.opts.seconds, &self.live) {
            (Some(s), Some(l)) => l.started.elapsed() >= Duration::from_secs_f32(s.max(0.1)),
            _ => false,
        }
    }
}

impl App {
    /// One key press or release of the match (winit's `KeyboardInput`, or a key of the iPad's keyboard bridge).
    fn key_in(&mut self, el: &ActiveEventLoop, k: KeyIn) {
        let PhysicalKey::Code(code) = k.physical else {
            return;
        };
        let down = k.down;
        if matches!(code, KeyCode::AltLeft | KeyCode::AltRight) {
            self.alt_held = down;
        }
        if matches!(code, KeyCode::SuperLeft | KeyCode::SuperRight) {
            self.super_held = down;
        }
        // CLI-1285: Cmd+Q on macOS is the client's own close (there is no Quit menu item)
        if is_mac_quit(code, down, k.repeat, self.super_held) {
            self.close(el, "cmd+q");
            return;
        }
        // m36 / CLI-510: while the leave box is open it takes every key: no look toggle, scoreboard, shell
        // key (chat, class, observer) or game action reaches the match; key releases still do, so nothing
        // stays held behind the box
        // CLI-1907: the open options take every key
        if self.options_up {
            let scan = self.options_menu.as_ref().is_some_and(|m| m.wants_scan());
            match options_route(code, down, k.repeat, k.text.as_deref(), scan) {
                OptionsRoute::Key(mk) => self.options_input(crate::menu::MenuInput::Key(mk)),
                OptionsRoute::Scan(s) => self.options_input(crate::menu::MenuInput::Scan(s)),
                OptionsRoute::Text(t) => {
                    for c in t.chars() {
                        self.options_input(crate::menu::MenuInput::Char(c));
                    }
                }
                OptionsRoute::Release => {
                    if code == KeyCode::Tab {
                        if let Some(l) = self.live.as_mut() {
                            l.board_held = false;
                        }
                    } else if let Some(a) = action_for(code, &k.logical) {
                        self.input.device(key_input(code, &k.logical, a, false));
                    }
                }
                OptionsRoute::Swallow => {}
            }
            return;
        }
        // CLI-950: the open report form takes every key
        match report_form_route(
            self.report_form.is_some(),
            code,
            down,
            k.repeat,
            k.text.as_deref(),
        ) {
            ReportRoute::Pass => {}
            ReportRoute::Key(k) => {
                self.report_input(crate::menu::MenuInput::Key(k));
                return;
            }
            ReportRoute::Text(t) => {
                for c in t.chars() {
                    self.report_input(crate::menu::MenuInput::Char(c));
                }
                return;
            }
            ReportRoute::Release => {
                if code == KeyCode::Tab {
                    if let Some(l) = self.live.as_mut() {
                        l.board_held = false;
                    }
                } else if let Some(a) = action_for(code, &k.logical) {
                    self.input.device(key_input(code, &k.logical, a, false));
                }
                return;
            }
            ReportRoute::Swallow => return,
        }
        match leave_box_route(self.leave.open, code, down, k.repeat) {
            LeaveRoute::Pass => {}
            LeaveRoute::Box(k) => {
                self.leave_key(el, k);
                return;
            }
            LeaveRoute::Release => {
                if code == KeyCode::Tab {
                    if let Some(l) = self.live.as_mut() {
                        l.board_held = false;
                    }
                } else if let Some(a) = action_for(code, &k.logical) {
                    self.input.device(key_input(code, &k.logical, a, false));
                }
                return;
            }
            LeaveRoute::Swallow => return,
        }
        if is_look_toggle(code, down, k.repeat, self.alt_held) {
            self.toggle_look();
            return;
        }
        // DC-DEFAULT: CLI-53 (proposed) Tab holds the scoreboard (the 2009 binding is ROM configuration)
        if code == KeyCode::Tab {
            if let Some(l) = self.live.as_mut() {
                l.board_held = down;
            }
            return;
        }
        // the in-match chat box takes Enter / the team chat key and, while open, every key
        if let Some(press) = self.live.as_ref().and_then(|l| {
            let open = l.hud.as_ref()?.state.chat_input.is_open();
            chat_press_for(code, down, k.repeat, k.text.as_deref(), open)
        }) {
            if self.chat_press(press) {
                return;
            }
        }
        if self
            .live
            .as_ref()
            .and_then(|l| l.hud.as_ref())
            .is_some_and(|h| h.state.chat_input.is_open())
        {
            return;
        }
        if down && !k.repeat {
            for k in shell_keys_for(code) {
                self.input.device(DeviceInput::Shell(k));
            }
        }
        // m36 / CLI-920: one Escape frees the cursor and opens the exit box (cshell+0x49670 case 0x1b)
        if code == KeyCode::Escape && down && !k.repeat {
            let captured = self.live.as_ref().is_some_and(|l| l.captured);
            let (free, step) = escape_in_match(captured, &mut self.leave);
            if free {
                self.release("escape");
            }
            self.leave_step(el, step);
            self.leave_pointer();
            return;
        }
        if let Some(step) = sensitivity_step_for(code) {
            if down && !k.repeat {
                self.step_sensitivity(step);
            }
        } else if let Some(f) = held_input_for(bound_key(code), down) {
            if !(down && k.repeat) {
                self.input.device(f);
            }
        } else if let Some(a) = action_for(code, &k.logical) {
            if !(down && k.repeat) {
                self.input.device(key_input(code, &k.logical, a, down));
            }
        } else if let Some(w) = weapon_input_for(code) {
            if down && !k.repeat {
                self.input.device(w);
            } else if !down && w == DeviceInput::Use {
                // CLI-1054: the end of a hold to plant / defuse
                self.input.device(DeviceInput::UseUp);
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.live.is_some() || self.error.is_some() {
            return;
        }
        // fix-hang / gate T1: the frame watchdog watches this (the event loop's) thread from the load on
        crate::watchdog::phase(crate::watchdog::Phase::Loading);
        crate::watchdog::arm(
            self.opts
                .sound
                .log
                .as_ref()
                .and_then(|p| p.parent().map(|d| d.to_path_buf())),
        );
        match self.start(el) {
            Ok(l) => {
                self.redraw_keep.arm(ms_since(l.started));
                self.live = Some(l);
                // CLI-1288: the menu's window comes back on screen on macOS (hidden by the menu turn's exit)
                if !self.created_window {
                    if let Some(l) = &self.live {
                        reshow_carried(&l.window);
                    }
                }
                // CLI-511: our own new window shows once its first frame is presented (never undrawn / white)
                if self.created_window {
                    if crate::gpulost::guard(|| self.redraw()).is_err() {
                        self.on_gpu_reset(el);
                        return;
                    }
                    if let Some(l) = &self.live {
                        l.window.set_visible(true);
                    }
                }
                let Some(focused) = self.live.as_ref().map(|l| l.window.has_focus()) else {
                    return;
                };
                if focused {
                    if let Some(l) = self.live.as_mut() {
                        l.focused = true;
                    }
                    // CLI-1604: the iPad resumes the stage on scene activity (the pointer lock changing among
                    // them); an open leave box or report form keeps the mouse free there
                    let box_open = cfg!(target_os = "ios") && (self.leave.open || self.form_up());
                    if !box_open {
                        self.capture();
                    }
                }
            }
            Err(e) => {
                self.error = Some(e);
                end_loop(el);
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if let Some(p) = live.prof.as_mut() {
            p.events += 1;
        }
        // a step file drives the run: the user's keys, buttons and wheel do not reach the game (gate T2)
        if crate::steps::active()
            && matches!(
                event,
                WindowEvent::MouseInput { .. }
                    | WindowEvent::MouseWheel { .. }
                    | WindowEvent::KeyboardInput { .. }
            )
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => self.close(el, "close requested"),
            // Minimised (Windows reports it as occluded): both masters read 0 until restored.
            // SPEC: AUDIO_2009.md §2 "Window minimise" (engine+0xdbc70)
            WindowEvent::Occluded(hidden) => {
                if let Some(d) = self.live.as_mut().and_then(|l| l.sound.as_mut()) {
                    d.set_minimised(hidden);
                }
                // CLI-1721: a window shown again takes its grab again soon
                if let Some(l) = self.live.as_ref() {
                    self.grab_keep.on_change(ms_since(l.started));
                    // CLI-1848: shown again: the surface again and a frame asked for
                    if !hidden {
                        self.redraw_keep.on_shown();
                        l.window.request_redraw();
                    }
                }
            }
            WindowEvent::Moved(_) => {
                if let Some(l) = self.live.as_ref() {
                    self.grab_keep.on_change(ms_since(l.started));
                }
            }
            WindowEvent::Focused(f) => {
                if let Some(l) = self.live.as_mut() {
                    l.focused = f;
                    // CLI-1848: focused again (alt-tab back): the surface again and a frame asked for
                    if f {
                        self.redraw_keep.on_shown();
                        l.window.request_redraw();
                    }
                }
                if !f {
                    self.alt_held = false;
                    self.super_held = false;
                }
                // the leave box keeps the cursor free while it is open
                if f && !self.leave.open && !self.form_up() {
                    // CLI-1721: a capture refused while the window is still being restored is tried again
                    if let Some(l) = self.live.as_ref() {
                        self.grab_keep.on_focus(ms_since(l.started));
                    }
                    self.capture();
                } else if !f {
                    self.release("focus lost");
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                // a click captures the cursor; with the cursor captured the button is the fire button
                let captured = self.live.as_ref().is_some_and(|l| l.captured);
                let down = state == ElementState::Pressed;
                // CLI-1907: the options take the left button
                if self.options_up {
                    let p = self.pointer_virtual();
                    self.options_input(if down {
                        crate::menu::MenuInput::Press(p)
                    } else {
                        crate::menu::MenuInput::Release(p)
                    });
                    return;
                }
                // CLI-950: the report form takes the left button
                if self.report_form.is_some() {
                    let p = self.pointer_virtual();
                    self.report_input(if down {
                        crate::menu::MenuInput::Press(p)
                    } else {
                        crate::menu::MenuInput::Release(p)
                    });
                    return;
                }
                // m36 / CLI-920: with the leave box open the left button presses its buttons: pressed look while
                // held, the button acts on the release over it
                if self.leave.open && !captured {
                    let over = self.leave_button_at_pointer();
                    if down {
                        self.leave_held = over.is_some();
                    } else if self.leave_held {
                        self.leave_held = false;
                        if let Some(i) = over.filter(|i| self.leave.view.down == Some(*i)) {
                            self.leave_key(el, crate::loading::LeaveKey::Click(i));
                        }
                    }
                    self.leave_pointer();
                    return;
                }
                // dead: the left button follows the next team mate (CLI-1120)
                if captured && down {
                    if let Some(l) = self.live.as_mut() {
                        if l.death_cam.cycle(1) {
                            return;
                        }
                    }
                }
                if captured {
                    self.input.device(DeviceInput::Fire(down));
                } else if down {
                    self.capture();
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } => {
                // DC-DEFAULT: CLI-411 (staged) the right button holds zoom while the cursor is captured
                let captured = self.live.as_ref().is_some_and(|l| l.captured);
                let down = state == ElementState::Pressed;
                // dead: the right button follows the previous team mate (CLI-1120)
                if captured && down {
                    if let Some(l) = self.live.as_mut() {
                        if l.death_cam.cycle(-1) {
                            return;
                        }
                    }
                }
                if captured || !down {
                    self.input
                        .device(DeviceInput::Action(Action::Zoom, down && captured));
                }
            }
            // DC-DEFAULT: CLI-1950 (proposed) the middle button: a waiting key box of the options takes it as our
            // pseudo key; in the match it is the press of the command bound to it (the team ping by default)
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Middle,
                ..
            } => {
                if self.options_up {
                    if self.options_menu.as_ref().is_some_and(|m| m.wants_scan()) {
                        self.options_input(crate::menu::MenuInput::Scan(
                            crate::settings::KEY_MOUSE_MIDDLE,
                        ));
                    }
                    return;
                }
                let captured = self.live.as_ref().is_some_and(|l| l.captured);
                if captured && self.report_form.is_none() && !self.leave.open {
                    if let Some(d) = press_input_for(crate::settings::bound_input(
                        crate::settings::KEY_MOUSE_MIDDLE,
                    )) {
                        self.input.device(d);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = [position.x, position.y];
                self.soft_cursor();
                if self.report_form.is_some() {
                    let p = self.pointer_virtual();
                    self.report_input(crate::menu::MenuInput::Move(p));
                }
                if self.options_up {
                    let p = self.pointer_virtual();
                    self.options_input(crate::menu::MenuInput::Move(p));
                }
                self.leave_pointer();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.options_up {
                    let steps = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                    };
                    let p = self.pointer_virtual();
                    self.options_input(crate::menu::MenuInput::Wheel(p, steps.signum() as i32));
                    return;
                }
                if self.report_form.is_some() {
                    let steps = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                    };
                    let p = self.pointer_virtual();
                    self.report_input(crate::menu::MenuInput::Wheel(p, steps.signum() as i32));
                    return;
                }
                if self.live.as_ref().is_some_and(|l| l.captured) {
                    let steps = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                    };
                    // dead: the wheel zooms the follow camera (cshell+0x276620)
                    if let Some(l) = self.live.as_mut() {
                        if l.death_cam.wheel(steps) {
                            return;
                        }
                    }
                    // wheel up = previous weapon, down = next
                    let n = -(steps.signum() as i32) * (steps.abs().ceil() as i32).min(4);
                    if n != 0 {
                        self.input.device(DeviceInput::Wheel(n));
                    }
                }
            }
            // CLI-1521: the modifier state releases a held Ctrl / Alt / Shift key whose key-up never arrived
            // (winit sends it before the key event of the same press / release)
            WindowEvent::ModifiersChanged(m) => {
                if !crate::steps::active() {
                    self.input.device(modifiers_input(m.state()));
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.key_in(el, KeyIn::from(&event)),
            // a different scale factor must not rescale the physical-pixel window (winit would keep the
            // logical size); ask for the planned size again
            // PUBLIC: winit docs (ScaleFactorChanged, InnerSizeWriter)
            WindowEvent::ScaleFactorChanged {
                mut inner_size_writer,
                ..
            } => {
                let _ = inner_size_writer
                    .request_inner_size(PhysicalSize::new(self.target.0, self.target.1));
            }
            WindowEvent::Resized(size) => {
                if self.recovering.is_some() {
                    // the rebuild reads the window's size itself
                    return;
                }
                let resized = crate::gpulost::guard(|| {
                    if let Some(l) = self.live.as_mut() {
                        if fit_surface(l, size) {
                            // CLI-1848: a restored window asks for its frame (the redraw chain may have ended)
                            l.window.request_redraw();
                        }
                    }
                });
                if resized.is_err() {
                    self.on_gpu_lost(el);
                    return;
                }
                if self.recapture {
                    self.recapture = false;
                    self.capture();
                }
                // CLI-1721: the clip follows the new client rectangle
                if let Some(l) = self.live.as_ref() {
                    self.grab_keep.on_change(ms_since(l.started));
                }
            }
            WindowEvent::RedrawRequested => self.frame_turn(el, false),
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            // CLI-1573: every motion report counts; the rate over the last second
            if let Some(l) = self.live.as_mut() {
                let now = Instant::now();
                match l.mouse_t0 {
                    Some(t0) if now.duration_since(t0) >= Duration::from_secs(1) => {
                        l.mouse_hz =
                            Some(l.mouse_count as f32 / now.duration_since(t0).as_secs_f32());
                        l.mouse_count = 0;
                        l.mouse_t0 = Some(now);
                    }
                    Some(_) => {}
                    None => l.mouse_t0 = Some(now),
                }
                l.mouse_count += 1;
            }
            if self.live.as_ref().is_some_and(|l| l.captured) && !crate::steps::active() {
                self.input.device(DeviceInput::MouseDelta(delta.0, delta.1));
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        crate::watchdog::pump();
        self.keep_grab();
        self.keep_frames(el);
        if let Some(at) = self.recovering.as_ref().map(|r| r.next_at) {
            if Instant::now() >= at {
                self.try_recover(el);
            } else {
                el.set_control_flow(ControlFlow::WaitUntil(at));
            }
        }
        if self.deadline_passed() {
            self.close(el, "deadline");
        }
        // a `leave` or `quit` step ends the match window (gate T2)
        if self.live.is_some() && crate::steps::end_game() {
            self.close(el, "steps");
        }
        // DC-DEFAULT: CLI-26 (proposed) the menu's game window ends one second after the session ended
        let over = self.report.session_over_s;
        let now = self
            .live
            .as_ref()
            .map(|l| l.started.elapsed().as_secs_f64());
        if self.close_on_over && over.zip(now).is_some_and(|(o, n)| n - o >= 1.0) {
            self.close(el, "session over");
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        // gate T1: the teardown is watched with its own budget (the menu or the end of main takes over)
        crate::watchdog::phase(crate::watchdog::Phase::Teardown);
        self.release("exit");
        crate::prewarm::load_begins();
        crate::events::screen("match_end");
        if let Some(mut live) = self.live.take() {
            let r = &mut self.report;
            // the device stops before the window closes; no callback runs after this (a carried director is handed
            // back open: its owner closes it)
            if self.carries_sound {
                self.carried_sound = live.sound.take();
            } else if let Some(d) = live.sound.as_mut() {
                r.audio_out.push(d.close_output());
                r.audio_out.push(d.bounds_line());
            }
            r.frames = live.fps.frames;
            r.seconds = live.started.elapsed().as_secs_f64();
            r.mean_fps = live.fps.mean_fps();
            r.min_frame_ms = live.fps.min_ms;
            r.max_frame_ms = live.fps.max_ms;
            r.frame_times = live.times.clone();
            r.vsync_period_ms = live.vsync_period.map(|p| p.as_secs_f64() * 1000.0);
            r.pace_short = live.frame_clock.short;
            r.pace_long = live.frame_clock.long;
            if let Some(p) = live.prof.as_mut() {
                if p.open {
                    live.laps.lap(crate::prof::Stage::Loop);
                    p.rec.end_with(p.remotes, Some(&live.ctx.device));
                    p.open = false;
                }
                p.rec.finish_with(Some(&live.ctx.device));
            }
            r.ticks = live.player.ticks();
            r.end = live.player.state.pos.to_array();
            r.released_on_exit = !live.captured;
            // w2-loading: a handed-in window goes back to the menu (it stays on screen with the last frame until the
            // menu draws its next one); our own window closes when `live` drops here, after the cursor was released
            if self.hands_back {
                let Live {
                    window,
                    surface,
                    config,
                    ctx,
                    readable,
                    immediate_ok,
                    ..
                } = live;
                // CLI-1288: ordered back at once on macOS (winit closed it on this exit) for the menu's next turn
                reshow_carried(&window);
                self.host_out = Some(WindowHost {
                    window,
                    surface,
                    config,
                    ctx,
                    readable,
                    immediate_ok,
                });
            }
        }
    }
}

/// Opens the window and runs until it is closed (or the deadline passes).
pub fn run_window(
    source: Box<dyn WorldSource>,
    opts: WindowOptions,
) -> Result<WindowReport, ClientError> {
    let el = new_event_loop()?;
    el.set_control_flow(ControlFlow::Poll);
    let look = opts.look;
    let mut app = App {
        opts,
        target: (0, 0),
        source,
        input: Box::new(KeyboardMouse::new(look)),
        live: None,
        mods: None,
        error: None,
        report: WindowReport::default(),
        closing: false,
        alt_held: false,
        super_held: false,
        pacer: Default::default(),
        recapture: false,
        grab_keep: crate::grabkeep::GrabKeeper::default(),
        redraw_keep: crate::redrawkeep::RedrawKeeper::default(),
        close_on_over: false,
        carried_sound: None,
        carries_sound: false,
        watch: crate::events::GameWatch::default(),
        host_in: None,
        host_out: None,
        hands_back: false,
        created_window: false,
        leave: crate::loading::LeaveBox::default(),
        leave_held: false,
        ui_cues: Vec::new(),
        report_form: None,
        options_menu: None,
        options_up: false,
        clipboard: None,
        last_report: None,
        pointer: [0.0, 0.0],
        recovering: None,
    };
    el.run_app(&mut app)
        .map_err(|e| ClientError::Window(e.to_string()))?;
    if let Some(e) = app.error {
        return Err(e);
    }
    if app.report.frames == 0 {
        return Err(ClientError::Window(
            "window closed before the first frame".into(),
        ));
    }
    Ok(app.report)
}

/// Runs the game window on an event loop the caller owns (the menu screens and the game share one loop and
/// run on it in turns); the window closes one second after the server ended the session.
// PUBLIC: winit docs (EventLoopExtRunOnDemand::run_app_on_demand: the loop may run several times per process)
pub fn run_window_in(
    el: &mut EventLoop<()>,
    source: Box<dyn WorldSource>,
    opts: WindowOptions,
) -> Result<WindowReport, ClientError> {
    run_window_in_with_sound(el, source, opts, None).0
}

/// [`run_window_in`] with the caller's sound director (`Some`: the window plays through it instead of opening its
/// own, and hands it back still open; the music keeps its clock and its track across the switch).
pub fn run_window_in_with_sound(
    el: &mut EventLoop<()>,
    source: Box<dyn WorldSource>,
    opts: WindowOptions,
    sound: Option<crate::sound::SoundDirector>,
) -> (
    Result<WindowReport, ClientError>,
    Option<crate::sound::SoundDirector>,
) {
    let (r, s, _) = run_window_in_host(el, source, opts, sound, None);
    (r, s)
}

/// [`run_window_in_with_sound`] in the caller's window (`host`: the menu's; the match draws in it and hands it back
/// with the director when it ends, `None` back when the window was lost to a GPU reset). Without a host the match
/// opens a window of its own and closes it at the end.
pub fn run_window_in_host(
    el: &mut EventLoop<()>,
    source: Box<dyn WorldSource>,
    opts: WindowOptions,
    sound: Option<crate::sound::SoundDirector>,
    host: Option<WindowHost>,
) -> (
    Result<WindowReport, ClientError>,
    Option<crate::sound::SoundDirector>,
    Option<WindowHost>,
) {
    #[cfg(not(target_os = "ios"))]
    {
        use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
        el.set_control_flow(ControlFlow::Poll);
        let mut app = App::for_host(source, opts, sound, host);
        let ran = el
            .run_app_on_demand(&mut app)
            .map_err(|e| ClientError::Window(e.to_string()));
        app.hand_back(ran)
    }
    // iPadOS: the one event loop never returns; crate::ios drives the match through `MatchTurn`
    #[cfg(target_os = "ios")]
    {
        let _ = (el, source, opts);
        (
            Err(ClientError::Window(
                "a match runs in the iPad driver's event loop".into(),
            )),
            sound,
            host,
        )
    }
}

impl App {
    /// The match window of [`run_window_in_host`] before its event loop runs.
    fn for_host(
        source: Box<dyn WorldSource>,
        opts: WindowOptions,
        sound: Option<crate::sound::SoundDirector>,
        host: Option<WindowHost>,
    ) -> App {
        let look = opts.look;
        App {
            opts,
            target: (0, 0),
            source,
            input: Box::new(KeyboardMouse::new(look)),
            live: None,
            mods: None,
            error: None,
            report: WindowReport::default(),
            closing: false,
            close_on_over: true,
            alt_held: false,
            super_held: false,
            pacer: Default::default(),
            recapture: false,
            grab_keep: crate::grabkeep::GrabKeeper::default(),
            redraw_keep: crate::redrawkeep::RedrawKeeper::default(),
            carries_sound: sound.is_some(),
            carried_sound: sound,
            watch: crate::events::GameWatch::default(),
            hands_back: host.is_some(),
            host_in: host,
            host_out: None,
            created_window: false,
            leave: crate::loading::LeaveBox::default(),
            leave_held: false,
            ui_cues: Vec::new(),
            report_form: None,
            options_menu: None,
            options_up: false,
            clipboard: None,
            last_report: None,
            pointer: [0.0, 0.0],
            recovering: None,
        }
    }

    /// What the match hands back once its event loop ended (`ran`: the loop's own result): the report, the
    /// carried sound director (its world's sounds ended) and the window.
    fn hand_back(
        mut self,
        ran: Result<(), ClientError>,
    ) -> (
        Result<WindowReport, ClientError>,
        Option<crate::sound::SoundDirector>,
        Option<WindowHost>,
    ) {
        // a window that never reached its first frame still holds the director it was given
        let back = self
            .carried_sound
            .take()
            .or_else(|| self.live.take().and_then(|mut l| l.sound.take()))
            .map(|mut d| {
                // CLI-1135 / CLI-1890: every voice of the match ends with it (every leave path passes here), the
                // menu keeps the director and its music
                // EVIDENCE: engine+0x19d00 -> engine+0xd8eb0 (the world teardown stops every sound instance)
                d.end_world();
                d
            });
        // the window back: handed back at the exit, or never taken (the start failed before it)
        let host = self.host_out.take().or_else(|| self.host_in.take());
        if let Err(e) = ran {
            return (Err(e), back, host);
        }
        if let Some(e) = self.error.take() {
            return (Err(e), back, host);
        }
        (Ok(std::mem::take(&mut self.report)), back, host)
    }
}

/// iPadOS: one match on the one event loop of the process (`crate::ios`, CLI-1604). The iPad driver forwards the
/// loop's events to the match's handler, reads [`MatchTurn::ended`] after each (where the desktop's run would have
/// returned) and then hands the window and the director back as [`run_window_in_host`] does.
#[cfg(target_os = "ios")]
pub(crate) struct MatchTurn(App);

#[cfg(target_os = "ios")]
impl MatchTurn {
    /// The match in the menu's window (`host`) with the menu's director (`sound`); the caller sends `resumed` next.
    pub(crate) fn new(
        source: Box<dyn WorldSource>,
        opts: WindowOptions,
        sound: Option<crate::sound::SoundDirector>,
        host: Option<WindowHost>,
    ) -> MatchTurn {
        MatchTurn(App::for_host(source, opts, sound, host))
    }

    pub(crate) fn handler(&mut self) -> &mut impl ApplicationHandler {
        &mut self.0
    }

    /// The match asked its turn to end (closed, failed to start).
    pub(crate) fn ended(&self) -> bool {
        self.0.closing || self.0.error.is_some()
    }

    /// The cursor is taken for mouse look (the pointer lock).
    pub(crate) fn captured(&self) -> bool {
        self.0.live.as_ref().is_some_and(|l| l.captured)
    }

    /// The match's window (`None` before the start or after a GPU loss).
    pub(crate) fn window(&self) -> Option<&Arc<Window>> {
        self.0.live.as_ref().map(|l| &l.window)
    }

    /// A key of the iPad's keyboard bridge, on the path of winit's `KeyboardInput`.
    pub(crate) fn key(&mut self, el: &ActiveEventLoop, k: KeyIn) {
        // as `window_event`: no window, no key; counted for the profiler; a step file keeps the user's keys out
        let Some(live) = self.0.live.as_mut() else {
            return;
        };
        if let Some(p) = live.prof.as_mut() {
            p.events += 1;
        }
        if crate::steps::active() {
            return;
        }
        self.0.key_in(el, k);
    }

    /// Ends the match as the desktop's loop exit does (`exiting`) and hands back what [`run_window_in_host`] does.
    pub(crate) fn finish(
        mut self,
        el: &ActiveEventLoop,
    ) -> (
        Result<WindowReport, ClientError>,
        Option<crate::sound::SoundDirector>,
        Option<WindowHost>,
    ) {
        self.0.exiting(el);
        self.0.hand_back(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_keys_carry_their_family_and_altgr_carries_none() {
        use crate::input::ModifierKey as M;
        let ctrl = Key::Named(NamedKey::Control);
        let alt = Key::Named(NamedKey::Alt);
        match key_input(KeyCode::AltLeft, &alt, Action::Walk, true) {
            DeviceInput::Key {
                key, family, down, ..
            } => assert_eq!((key, family, down), (56, Some(M::Alt), true)),
            e => panic!("{e:?}"),
        }
        assert_eq!(modifier_of(&ctrl), Some(M::Ctrl));
        assert_eq!(modifier_of(&Key::Named(NamedKey::Shift)), Some(M::Shift));
        // the right Alt of an AltGr layout: the platform reports Ctrl and Alt up while it is held
        assert_eq!(modifier_of(&Key::Named(NamedKey::AltGraph)), None);
        assert_eq!(modifier_of(&Key::Character("c".into())), None);
        let m = winit::keyboard::ModifiersState::CONTROL;
        assert_eq!(
            modifiers_input(m),
            DeviceInput::Modifiers {
                ctrl: true,
                alt: false,
                shift: false,
                logo: false
            }
        );
    }

    #[test]
    fn a_new_window_opens_hidden() {
        // CLI-511: shown only after its first frame was presented (no white, undrawn window on screen)
        let plan = WindowPlan {
            x: 10,
            y: 20,
            width: 800,
            height: 600,
        };
        for (placed, no_activate) in [(true, false), (false, true)] {
            let a = host_attributes(&plan, WindowLook::Borderless, placed, no_activate);
            assert!(!a.visible, "placed {placed} no_activate {no_activate}");
            assert!(!a.decorations && !a.resizable);
            assert_eq!(a.position.is_some(), placed);
        }
    }

    #[test]
    fn the_open_report_form_takes_every_key_and_types_text() {
        use crate::menu::MenuKey;
        // closed: the match gets the keys
        assert_eq!(
            report_form_route(false, KeyCode::KeyW, true, false, Some("w")),
            ReportRoute::Pass
        );
        // open: Escape / Tab / Backspace are the form's, text types, everything else is swallowed
        assert_eq!(
            report_form_route(true, KeyCode::Escape, true, false, None),
            ReportRoute::Key(MenuKey::Escape)
        );
        assert_eq!(
            report_form_route(true, KeyCode::Tab, true, false, None),
            ReportRoute::Key(MenuKey::Tab)
        );
        assert_eq!(
            report_form_route(true, KeyCode::Backspace, true, true, None),
            ReportRoute::Key(MenuKey::Backspace)
        );
        assert_eq!(
            report_form_route(true, KeyCode::KeyW, true, false, Some("w")),
            ReportRoute::Text("w".into())
        );
        assert_eq!(
            report_form_route(true, KeyCode::F5, true, false, None),
            ReportRoute::Swallow
        );
        // a release still ends a held action behind the form
        assert_eq!(
            report_form_route(true, KeyCode::KeyW, false, false, None),
            ReportRoute::Release
        );
    }

    /// CLI-1907: the options over the match take every key as the menu window gives them to the box; a waiting key
    /// box takes the next key itself (Escape still gives up); releases reach the match.
    #[test]
    fn the_options_over_the_match_take_every_key() {
        use crate::menu::MenuKey;
        assert_eq!(
            options_route(KeyCode::Escape, true, false, None, false),
            OptionsRoute::Key(MenuKey::Escape)
        );
        assert_eq!(
            options_route(KeyCode::Escape, true, true, None, false),
            OptionsRoute::Swallow,
            "a held Escape does not close the box twice"
        );
        assert_eq!(
            options_route(KeyCode::ArrowDown, true, true, None, false),
            OptionsRoute::Key(MenuKey::Down),
            "the list scrolls while Down is held"
        );
        assert_eq!(
            options_route(KeyCode::KeyW, true, false, Some("w"), false),
            OptionsRoute::Text("w".into())
        );
        assert_eq!(options_route(KeyCode::F5, true, false, None, false), OptionsRoute::Swallow);
        assert_eq!(
            options_route(KeyCode::KeyW, false, false, None, false),
            OptionsRoute::Release
        );
        // a key box waits: W is its key, Escape still goes to the box
        let w = crate::settings::scan_of_code(KeyCode::KeyW).unwrap();
        assert_eq!(options_route(KeyCode::KeyW, true, false, Some("w"), true), OptionsRoute::Scan(w));
        assert_eq!(
            options_route(KeyCode::Escape, true, false, None, true),
            OptionsRoute::Key(MenuKey::Escape)
        );
    }

    #[test]
    fn one_escape_in_the_match_opens_the_exit_box() {
        use crate::loading::{LeaveBox, LeavePage, LeaveStep};
        // cursor captured (playing): the one press frees it and opens the box
        let mut b = LeaveBox::default();
        assert_eq!(escape_in_match(true, &mut b), (true, LeaveStep::Opened));
        assert_eq!(b.shown().map(|v| v.page), Some(LeavePage::Exit));
        // the next press goes to the open box (leave_box_route) and closes it
        assert_eq!(
            leave_box_route(b.open, KeyCode::Escape, true, false),
            LeaveRoute::Box(crate::loading::LeaveKey::Escape)
        );
        assert_eq!(
            b.key(crate::loading::LeaveKey::Escape),
            LeaveStep::Cancelled
        );
        // cursor already free: the press opens the box too
        assert_eq!(escape_in_match(false, &mut b), (false, LeaveStep::Opened));
    }

    #[test]
    fn the_open_leave_box_takes_every_key() {
        use crate::loading::LeaveKey;
        // closed: every key goes to the match
        for c in [
            KeyCode::Enter,
            KeyCode::Tab,
            KeyCode::Digit1,
            KeyCode::Escape,
        ] {
            assert_eq!(leave_box_route(false, c, true, false), LeaveRoute::Pass);
        }
        // open: only the box's keys act; shell keys (Enter would open chat), Tab, class digits, the observer
        // keys and the game actions do nothing
        assert_eq!(
            leave_box_route(true, KeyCode::Enter, true, false),
            LeaveRoute::Box(LeaveKey::Confirm)
        );
        assert_eq!(
            leave_box_route(true, KeyCode::NumpadEnter, true, false),
            LeaveRoute::Box(LeaveKey::Confirm)
        );
        assert_eq!(
            leave_box_route(true, KeyCode::KeyY, true, false),
            LeaveRoute::Box(LeaveKey::Confirm)
        );
        assert_eq!(
            leave_box_route(true, KeyCode::KeyN, true, false),
            LeaveRoute::Box(LeaveKey::Deny)
        );
        assert_eq!(
            leave_box_route(true, KeyCode::Escape, true, false),
            LeaveRoute::Box(LeaveKey::Escape)
        );
        for c in [
            KeyCode::Tab,
            KeyCode::Digit1,
            KeyCode::Digit9,
            KeyCode::PageDown,
            KeyCode::KeyW,
            KeyCode::F10,
        ] {
            assert_eq!(
                leave_box_route(true, c, true, false),
                LeaveRoute::Swallow,
                "{c:?}"
            );
        }
        // a held Enter does not confirm twice; releases still reach the match
        assert_eq!(
            leave_box_route(true, KeyCode::Enter, true, true),
            LeaveRoute::Swallow
        );
        assert_eq!(
            leave_box_route(true, KeyCode::Tab, false, false),
            LeaveRoute::Release
        );
        assert_eq!(
            leave_box_route(true, KeyCode::KeyW, false, false),
            LeaveRoute::Release
        );
    }

    /// CLI-1032 (round 11: the Control tab's bindings did not drive the match): fire rebound to a key fires on that
    /// key (held), the Activate key is the use key, the weapon commands' keys are the class keys.
    #[test]
    fn rebound_fire_use_and_weapon_keys_drive_the_match_input() {
        use crate::input::{InputSource, KeyboardMouse};
        use crate::settings::{scan_of_code, Bound, KeyBinds};
        let mut b = KeyBinds::default();
        let f = scan_of_code(KeyCode::KeyF).unwrap();
        b.bind(8, Some(f));
        let mut k = KeyboardMouse::new(crate::look::LookParams::DEFAULT);
        k.device(held_input_for(b.lookup(f), true).expect("fire on F"));
        assert!(k.weapon().fire, "the next step fires");
        k.device(held_input_for(b.lookup(f), false).unwrap());
        assert!(!k.weapon().fire);
        let e = scan_of_code(KeyCode::KeyE).unwrap();
        assert_eq!(press_input_for(b.lookup(e)), Some(DeviceInput::Use));
        // Activate moved to G: E does nothing, G is the use key
        let g = scan_of_code(KeyCode::KeyG).unwrap();
        b.bind(2, Some(g));
        assert_eq!(press_input_for(b.lookup(e)), None);
        assert_eq!(press_input_for(b.lookup(g)), Some(DeviceInput::Use));
        assert_eq!(
            b.lookup(scan_of_code(KeyCode::Digit6).unwrap()),
            Some(Bound::Slot(5))
        );
    }

    #[test]
    fn shell_and_zoom_bindings() {
        assert_eq!(shell_keys_for(KeyCode::Digit5), vec![ShellKey::ClassKey(4)]);
        // the chat box: closed, the SendMessage (Enter) / BroadCastChat (F2) / TeamChat (F3) keys open it (CLI-1032)
        // and other keys stay the game's; open, keys are typed
        use crate::hud::chat::ChatPress as P;
        assert_eq!(
            chat_press_for(KeyCode::Enter, true, false, None, false),
            Some(P::Enter)
        );
        assert_eq!(
            chat_press_for(KeyCode::F2, true, false, None, false),
            Some(P::Enter)
        );
        assert_eq!(
            chat_press_for(KeyCode::F3, true, false, None, false),
            Some(P::Team)
        );
        assert_eq!(
            chat_press_for(KeyCode::KeyW, true, false, Some("w"), false),
            None
        );
        assert_eq!(
            chat_press_for(KeyCode::KeyY, true, false, Some("y"), true),
            Some(P::Text("y".into()))
        );
        assert_eq!(
            chat_press_for(KeyCode::Escape, true, false, None, true),
            Some(P::Escape)
        );
        assert_eq!(
            chat_press_for(KeyCode::KeyW, false, false, None, true),
            None
        );
        assert_eq!(shell_keys_for(KeyCode::Digit1), vec![ShellKey::ClassKey(0)]);
        assert_eq!(
            shell_keys_for(KeyCode::Enter),
            vec![ShellKey::ChatOpen, ShellKey::ChatSend]
        );
        assert_eq!(shell_keys_for(KeyCode::Escape), vec![ShellKey::ChatCancel]);
        assert_eq!(shell_keys_for(KeyCode::F10), vec![ShellKey::LeaveBox]);
        assert!(shell_keys_for(KeyCode::KeyW).is_empty());
        assert_eq!(
            action_for(KeyCode::KeyZ, &Key::Character("z".into())),
            Some(Action::Zoom)
        );
        // the number keys are class keys only: no fixed slot any more (M18)
        assert_eq!(weapon_input_for(KeyCode::Digit2), None);
        assert_eq!(weapon_input_for(KeyCode::KeyR), Some(DeviceInput::Reload));
        assert_eq!(weapon_input_for(KeyCode::KeyE), Some(DeviceInput::Use));
        // no shell key shares a physical key with a movement action
        for code in [
            KeyCode::KeyW,
            KeyCode::KeyA,
            KeyCode::KeyS,
            KeyCode::KeyD,
            KeyCode::Space,
        ] {
            assert!(shell_keys_for(code).is_empty());
        }
    }

    #[test]
    fn default_fills_the_monitor_at_its_origin() {
        let p = plan_window(None, (0, 0), (2560, 1440));
        assert_eq!(
            p,
            WindowPlan {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440
            }
        );
    }

    #[test]
    fn second_monitor_origin_may_be_negative() {
        let p = plan_window(None, (-1920, -200), (1920, 1080));
        assert_eq!((p.x, p.y, p.width, p.height), (-1920, -200, 1920, 1080));
    }

    #[test]
    fn requested_size_is_centred() {
        let p = plan_window(Some((1280, 720)), (0, 0), (1920, 1080));
        assert_eq!((p.x, p.y, p.width, p.height), (320, 180, 1280, 720));
        let q = plan_window(Some((800, 600)), (1920, 0), (1920, 1080));
        assert_eq!((q.x, q.y), (1920 + 560, 240));
    }

    #[test]
    fn requested_size_never_exceeds_the_monitor() {
        let p = plan_window(Some((4000, 700)), (10, 20), (1920, 1080));
        assert_eq!((p.x, p.y, p.width, p.height), (10, 20 + 190, 1920, 700));
        let z = plan_window(Some((0, 0)), (0, 0), (100, 100));
        assert_eq!((z.width, z.height), (1, 1));
    }

    #[test]
    fn degenerate_monitor_is_at_least_one_pixel() {
        let p = plan_window(None, (0, 0), (0, 0));
        assert_eq!((p.width, p.height), (1, 1));
    }

    #[test]
    fn alt_enter_toggles_between_the_borderless_and_the_bordered_look() {
        // the toggle is a two-state switch and always returns
        let l = WindowLook::Borderless;
        assert_eq!(l.toggled(), WindowLook::Bordered);
        assert_eq!(l.toggled().toggled(), l);
        // full screen = the monitor rectangle (CLI-111); windowed without a size = 1280x720 centred
        let b = plan_look(WindowLook::Borderless, None, None, (0, 0), (2560, 1440));
        assert_eq!((b.x, b.y, b.width, b.height), (0, 0, 2560, 1440));
        let w = plan_look(WindowLook::Bordered, None, None, (0, 0), (2560, 1440));
        assert_eq!((w.x, w.y, w.width, w.height), (640, 360, 1280, 720));
        // a second monitor with a negative origin keeps its own centre
        let w = plan_look(
            WindowLook::Bordered,
            None,
            None,
            (-1920, -200),
            (1920, 1080),
        );
        assert_eq!(
            (w.x, w.y, w.width, w.height),
            (-1920 + 320, -200 + 180, 1280, 720)
        );
        // a small monitor scales the windowed look into nine tenths of each side, aspect kept
        let w = plan_look(WindowLook::Bordered, None, None, (0, 0), (1024, 768));
        assert_eq!((w.width, w.height), (921, 518));
        let w = plan_look(WindowLook::Bordered, None, None, (0, 0), (0, 0));
        assert_eq!((w.width, w.height), (1, 1));
        // `--size` alone shapes the full-screen look (the test tools' window); the windowed size is its own
        let b = plan_look(
            WindowLook::Borderless,
            Some((800, 600)),
            Some((1600, 900)),
            (0, 0),
            (1920, 1080),
        );
        assert_eq!((b.width, b.height), (800, 600));
        let w = plan_look(
            WindowLook::Bordered,
            Some((800, 600)),
            Some((1600, 900)),
            (0, 0),
            (1920, 1080),
        );
        assert_eq!((w.x, w.y, w.width, w.height), (160, 90, 1600, 900));
    }

    #[test]
    fn a_4k_monitor_fills_full_screen_and_centres_the_resolution_windowed() {
        // CLI-1280: a 4K TV with a 1920x1080 resolution setting
        let mon = (3840, 2160);
        let res = Some((1920, 1080));
        let f = plan_look(WindowLook::Borderless, None, res, (0, 0), mon);
        assert_eq!((f.x, f.y, f.width, f.height), (0, 0, 3840, 2160));
        let w = plan_look(WindowLook::Bordered, None, res, (0, 0), mon);
        assert_eq!((w.x, w.y, w.width, w.height), (960, 540, 1920, 1080));
        // the same resolution on a 1080p monitor is limited to the room left for the frame and the task bar
        let w = plan_look(WindowLook::Bordered, None, res, (0, 0), (1920, 1080));
        assert_eq!((w.x, w.y, w.width, w.height), (96, 54, 1728, 972));
        // a tall request is limited by the height, aspect kept
        assert_eq!(windowed_size(Some((1280, 1024)), (1366, 768)), (863, 691));
        // full -> windowed -> full ends on the whole monitor again
        let mut look = WindowLook::Borderless;
        let mut seen = Vec::new();
        for _ in 0..2 {
            look = look.toggled();
            let p = plan_look(look, None, res, (0, 0), mon);
            seen.push((look, p.width, p.height, p.x, p.y));
        }
        assert_eq!(
            seen,
            [
                (WindowLook::Bordered, 1920, 1080, 960, 540),
                (WindowLook::Borderless, 3840, 2160, 0, 0)
            ]
        );
    }

    #[test]
    fn the_first_window_takes_the_flag_the_saved_look_or_a_first_launch() {
        use WindowLook::{Bordered, Borderless};
        // no saved look / resolution: full screen, and the monitor's size is saved
        assert_eq!(
            first_window(None, false, Some(false), false),
            (Borderless, true)
        );
        // even when the look alone was saved windowed
        assert_eq!(
            first_window(None, false, Some(false), true),
            (Borderless, true)
        );
        // saved: the saved look, nothing noted
        assert_eq!(
            first_window(None, false, Some(true), true),
            (Bordered, false)
        );
        assert_eq!(
            first_window(None, false, Some(true), false),
            (Borderless, false)
        );
        // --windowed / --fullscreen win for the run and never save
        assert_eq!(
            first_window(Some(Bordered), false, Some(false), false),
            (Bordered, false)
        );
        assert_eq!(
            first_window(Some(Borderless), false, Some(true), true),
            (Borderless, false)
        );
        assert_eq!(
            first_window(Some(Bordered), false, Some(true), false),
            (Bordered, false)
        );
        // --size (the test tools) never saves either
        assert_eq!(
            first_window(None, true, Some(false), false),
            (Borderless, false)
        );
        // no settings published (no profile store, the map mode): the old rule, nothing noted
        assert_eq!(first_window(None, false, None, false), (Borderless, false));
        assert_eq!(start_look(None, true), Bordered);
    }

    #[test]
    fn only_alt_with_a_fresh_enter_press_toggles() {
        assert!(is_look_toggle(KeyCode::Enter, true, false, true));
        assert!(is_look_toggle(KeyCode::NumpadEnter, true, false, true));
        assert!(
            !is_look_toggle(KeyCode::Enter, true, false, false),
            "no Alt"
        );
        assert!(
            !is_look_toggle(KeyCode::Enter, false, false, true),
            "release"
        );
        assert!(!is_look_toggle(KeyCode::Enter, true, true, true), "repeat");
        assert!(!is_look_toggle(KeyCode::Escape, true, false, true));
        assert!(!is_look_toggle(KeyCode::AltLeft, true, false, true));
    }

    #[test]
    fn exit_report_starts_with_the_fps_line_and_gives_the_frame_times() {
        // client-fps: the same lines for the window mode and every match the menu starts
        let mut r = WindowReport {
            adapter: "test adapter".into(),
            present_mode: "Immediate".into(),
            frames: 3,
            seconds: 0.5,
            mean_fps: 6.0,
            exit_reason: "close requested".into(),
            ..WindowReport::default()
        };
        for ms in [1.0, 2.0, 40.0] {
            r.frame_times.push_ms(ms);
        }
        let l = exit_report_lines(&r);
        assert!(l[0].starts_with(
            "dc-client window: adapter test adapter (Immediate), 3 frames in 0.50 s = 6.0 fps"
        ));
        assert!(
            l[1].contains("frame times: median") && l[1].contains("worst 40.00 ms"),
            "{}",
            l[1]
        );
        assert!(l
            .iter()
            .any(|x| x == "  exit: close requested; cursor released on exit: false"));
    }

    // CLI-1932: a rebuild after the first failed one asks for the saved renderer's other backend
    #[test]
    fn later_rebuild_attempts_try_the_other_renderer() {
        let dx = AdapterChoice::Backends(wgpu::Backends::DX12);
        let vk = AdapterChoice::Backends(wgpu::Backends::VULKAN);
        assert_eq!(rebuild_choice(dx, true, 1), dx);
        assert_eq!(rebuild_choice(dx, true, 2), vk);
        assert_eq!(rebuild_choice(dx, true, 9), vk);
        assert_eq!(rebuild_choice(vk, true, 2), dx);
        // `--adapter` is exclusive, the default backends already hold both
        assert_eq!(rebuild_choice(dx, false, 3), dx);
        assert_eq!(
            rebuild_choice(AdapterChoice::Auto, true, 3),
            AdapterChoice::Auto
        );
        let metal = AdapterChoice::Backends(wgpu::Backends::METAL);
        assert_eq!(rebuild_choice(metal, true, 2), metal);
    }

    // CLI-1931: the start line names the adapter, its backend and type, driver and ids
    #[test]
    fn the_gpu_line_names_the_adapter_and_its_driver() {
        let info = wgpu::AdapterInfo {
            name: "Intel(R) UHD Graphics 620".into(),
            vendor: 0x8086,
            device: 0x5917,
            device_type: wgpu::DeviceType::IntegratedGpu,
            driver: "31.0.101.2111".into(),
            driver_info: String::new(),
            backend: wgpu::Backend::Dx12,
        };
        assert_eq!(
            gpu_line(&info),
            "Intel(R) UHD Graphics 620 (Dx12, IntegratedGpu); driver 31.0.101.2111; vendor 0x8086 device 0x5917"
        );
    }

    #[test]
    fn monitor_order_puts_the_primary_first() {
        assert_eq!(monitor_order(Some(&2), &[1, 2, 3]), vec![2, 1, 3]);
        assert_eq!(monitor_order(Some(&1), &[1, 2]), vec![1, 2]);
        assert_eq!(monitor_order::<i32>(None, &[5, 6]), vec![5, 6]);
        assert_eq!(monitor_order::<i32>(None, &[]), Vec::<i32>::new());
        // a primary missing from the list is still entry 0
        assert_eq!(monitor_order(Some(&9), &[1]), vec![9, 1]);
    }

    // CLI-1284: the default windowed size follows the monitor's scale on macOS only
    #[test]
    fn the_default_windowed_size_is_in_points_on_macos_and_pixels_elsewhere() {
        assert_eq!(default_windowed_size(2.0, true), (2560, 1440));
        assert_eq!(default_windowed_size(1.5, true), (1920, 1080));
        assert_eq!(default_windowed_size(2.0, false), BORDERED_SIZE);
        assert_eq!(default_windowed_size(1.0, true), BORDERED_SIZE);
        // a bad scale falls back to the plain size
        assert_eq!(default_windowed_size(0.0, true), BORDERED_SIZE);
        assert_eq!(default_windowed_size(f64::NAN, true), BORDERED_SIZE);
        // the windowed plan still fits nine tenths of a small Retina panel
        assert_eq!(
            windowed_size(Some(default_windowed_size(2.0, true)), (2560, 1600)),
            (2304, 1296)
        );
    }

    // CLI-1285: Cmd+Q is a quit on macOS only, as a fresh press with a Command key held
    #[test]
    fn cmd_q_quits_on_macos_only() {
        assert_eq!(is_mac_quit(KeyCode::KeyQ, true, false, true), MACOS);
        assert!(
            !is_mac_quit(KeyCode::KeyQ, true, false, false),
            "no Command key"
        );
        assert!(!is_mac_quit(KeyCode::KeyQ, false, false, true), "release");
        assert!(!is_mac_quit(KeyCode::KeyQ, true, true, true), "repeat");
        assert!(
            !is_mac_quit(KeyCode::KeyW, true, false, true),
            "another key"
        );
    }
}
