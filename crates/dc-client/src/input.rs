//! Where the commands of every simulation tick come from.
//!
//! [`InputSource`] is sampled at two rates: once per rendered frame (look input, so the view stays as
//! responsive as the frame rate) and once per simulation tick (the movement commands of that tick). Sources:
//! [`KeyboardMouse`] for the window (fed by backend-independent [`DeviceInput`] events) and [`ScriptedInput`]
//! for the headless runs and the tests. A network client can wrap either one (it only has to forward what
//! they produce); it never has to replace them.
// SPEC: MOVEMENT_2009.md §5.1 (control word from numbered commands), §13.2 (view angles from mouse deltas)

use crate::arms::{WeaponCommands, KEY_SLOTS};
use crate::look::{wrap_angle, LookParams, ViewAngles};
use crate::world::RemoteBody;
use dc_sim::Commands;

/// A movement action of the player (our names). Each maps to one of the engine's numbered commands that
/// MOVEMENT_2009 §5.1 reads; which physical key triggers it is configuration.
// SPEC: MOVEMENT_2009.md §5.1 (commands 0 forward, 1 back, 5 crouch, 6 jump, 7 run, 9 strafe modifier, 10 left,
// 11 right, 12 run-lock toggle, 0x77 sprint)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Forward,
    Back,
    Left,
    Right,
    Jump,
    Crouch,
    Sprint,
    /// Held: walk instead of run (the run command is on unless this is held).
    Walk,
    /// Pressed: flip the run lock (one tick per press).
    RunLock,
    StrafeMod,
    /// Held: scope / zoom (right mouse button or its key): the press cycles the zoom level, the held key sets the
    /// control word's 0x80000 bit ([`crate::zoom`]).
    // DC-DEFAULT: CLI-411 zoom is a held action (the physical keys)
    // SPEC: MOVEMENT_2009.md §5.1 (command 0x3d); EVIDENCE: cshell+0x1f95d0 (command 0x3d -> the zoom step)
    Zoom,
}

/// Number of [`Action`] variants (size of the held-state table).
const HELD_SLOTS: usize = 11;

/// A shell-level key press (edge, not held): the chat box, the class keys, the observer keys, the leave box.
/// Delivered through [`DeviceInput::Shell`] and queued by [`KeyboardMouse`] until a consumer drains them with
/// [`InputSource::shell_keys`]; nothing consumes them yet (behaviour belongs to the M7 / M18 / M19 / m36 tasks).
// DC-DEFAULT: CLI-411 (staged) the key set; the 2009 key-to-command bindings are ROM / profile
// configuration that DeCombat does not read yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellKey {
    /// Open the chat box.
    ChatOpen,
    /// Send the chat line being typed (the consumer decides by chat state whether Enter opens or sends).
    ChatSend,
    /// Cancel the chat line.
    ChatCancel,
    /// Class key `n` (0-based: key 1 is 0). The 2009 client cycles the weapons of order class `n + 1` per press; the
    /// keyboard source also hands it to the weapon ([`crate::arms::WeaponCommands::class_key`]).
    // EVIDENCE: cshell+0x22e530 (number key k cycles the WeaponOrder class k)
    ClassKey(u8),
    /// Observer: follow the next / previous mate.
    ObserverNext,
    ObserverPrev,
    /// Observer: change the camera mode (follow / first person / free).
    ObserverView,
    /// Open the leave-match box.
    // EVIDENCE: cshell+0x48450 (the leave box callback sends 0xbd); the key itself is ours
    LeaveBox,
}

/// Most queued shell keys kept between two drains (a full queue drops its oldest press; a release keeps the queue).
const SHELL_QUEUE_MAX: usize = 32;

/// The modifier family of a physical key (Ctrl, Alt, Shift, the logo key). The window reports the up / down state
/// of each family on its own ([`DeviceInput::Modifiers`]); a held key of a family reported up has lost its key-up
/// and is released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModifierKey {
    Ctrl,
    Alt,
    Shift,
    Logo,
}

/// One physical key holding an action ([`KeyboardMouse`] keeps the set of them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeldKey {
    key: u32,
    family: Option<ModifierKey>,
    action: Action,
}

/// Backend-independent input events (the window maps its key codes and raw mouse motion to these).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeviceInput {
    /// An action held or released directly (scripted input, tests); the window sends [`DeviceInput::Key`].
    Action(Action, bool),
    /// Physical key `key` (any stable id of the key, e.g. its scan code) bound to `action` went down / up. An action
    /// is held while at least one of its keys is down, so two keys of one action (C and Ctrl both crouch) do not
    /// release each other.
    // DC-DEFAULT: CLI-1520 (proposed) held actions follow their physical keys
    Key {
        key: u32,
        family: Option<ModifierKey>,
        action: Action,
        down: bool,
    },
    /// The window's modifier state: a family reported up releases every held key of that family whose own key-up
    /// never arrived (a platform that drops a Ctrl / Alt key-up would otherwise leave crouch or walk held).
    // DC-DEFAULT: CLI-1521 (proposed) the modifier state releases stale modifier keys
    Modifiers {
        ctrl: bool,
        alt: bool,
        shift: bool,
        logo: bool,
    },
    /// Raw relative mouse motion in device counts (+x right, +y toward the user).
    MouseDelta(f64, f64),
    /// Everything was released (focus lost, cursor released): clear held actions and pending motion.
    ReleaseAll,
    /// The fire button went down / up.
    Fire(bool),
    /// The reload key was pressed.
    Reload,
    /// The use key was pressed (take the dropped weapon under the player, [`crate::drops::USE_REACH`]).
    // DC-DEFAULT: CLI-675 (proposed)
    Use,
    /// The use key went up (the end of a hold to plant / defuse).
    // DC-DEFAULT: CLI-1054 (proposed)
    UseUp,
    /// A number key chose weapon slot `n` (0-based).
    Slot(u8),
    /// Mouse wheel steps (positive = next weapon).
    Wheel(i32),
    /// A shell-level key went down (chat, class key, observer, leave box).
    Shell(ShellKey),
}

/// Source of look input and per-tick commands.
pub trait InputSource {
    /// Once per rendered frame, before that frame's ticks: apply look input to `view`.
    fn frame(&mut self, view: &mut ViewAngles);
    /// Commands of simulation tick `tick` (absolute index, `hz` ticks per second). A source may also set the
    /// view angles here (scripted or replayed input).
    fn tick(&mut self, tick: u64, hz: u32, view: &mut ViewAngles) -> Commands;
    /// A device event (keys, mouse); sources that do not read devices ignore it.
    fn device(&mut self, _event: DeviceInput) {}
    /// Once per rendered frame, after [`InputSource::frame`]: the weapon commands of the frame. Sources without
    /// weapons give none.
    fn weapon(&mut self) -> WeaponCommands {
        WeaponCommands::default()
    }
    /// Shell-level key presses since the last call (chat, class keys, observer, leave box), oldest first.
    /// Sources without keys give none.
    fn shell_keys(&mut self) -> Vec<ShellKey> {
        Vec::new()
    }
    /// Once per rendered frame with the eye point and the remote players drawn this frame: a scripted source may
    /// turn the view toward one of them (test input); others ignore it.
    fn aim(
        &mut self,
        _eye: [f32; 3],
        _remotes: &[RemoteBody],
        _own_team: Option<u8>,
        _view: &mut ViewAngles,
    ) {
    }
    /// Once per rendered frame, before the frame's ticks: what the server has set so far (a scripted source may
    /// wait for it before it starts); others ignore it.
    fn server(&mut self, _view: &ServerHold) {}
    /// The zoom level of the scope (the mouse turns `1 / (3 z + 1)` as fast); sources without look input ignore it.
    // SPEC: MOVEMENT_2009.md §13.2 step 2
    fn set_zoom_level(&mut self, _z: u32) {}
    /// CLI-1031: the options' mouse sensitivity changed in the match (its hotkeys); sources without look input
    /// ignore it.
    fn set_sensitivity(&mut self, _s: f32) {}
    /// Short label for logs and the overlay.
    fn label(&self) -> &'static str;
}

/// What the frame loop knows about the server's hold on the body before a frame's ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ServerHold {
    /// Index of the next simulation tick.
    pub next_tick: u64,
    /// The own player state of shell 0x67 (`None`: no server).
    pub life_state: Option<u8>,
    /// Server corrections (0xa0 teleports) applied so far.
    pub corrections: u32,
    pub on_ground: bool,
}

/// Keyboard and mouse state of the window.
#[derive(Debug, Clone, Default)]
pub struct KeyboardMouse {
    pub look: LookParams,
    held: [bool; HELD_SLOTS],
    /// The physical keys down now and the actions they hold.
    keys: Vec<HeldKey>,
    shell: Vec<ShellKey>,
    /// RunLock presses not yet delivered to a tick.
    run_lock_presses: u32,
    pending: (f64, f64),
    fire: bool,
    /// The fire button went down since the last weapon read (CLI-1601).
    fire_press: bool,
    reload: bool,
    use_key: bool,
    use_held: bool,
    slot: Option<u8>,
    wheel: i32,
    /// A class key pressed since the last weapon read.
    class_key: Option<u8>,
}

fn slot(a: Action) -> usize {
    match a {
        Action::Forward => 0,
        Action::Back => 1,
        Action::Left => 2,
        Action::Right => 3,
        Action::Jump => 4,
        Action::Crouch => 5,
        Action::Sprint => 6,
        Action::Walk => 7,
        Action::RunLock => 8,
        Action::StrafeMod => 9,
        Action::Zoom => 10,
    }
}

impl KeyboardMouse {
    pub fn new(look: LookParams) -> KeyboardMouse {
        KeyboardMouse {
            look,
            ..KeyboardMouse::default()
        }
    }

    pub fn held(&self, a: Action) -> bool {
        self.held[slot(a)]
    }

    /// Holds `a` (a run-lock press counts once, on the edge).
    fn press(&mut self, a: Action) {
        let s = slot(a);
        if a == Action::RunLock && !self.held[s] {
            self.run_lock_presses += 1;
        }
        self.held[s] = true;
    }

    /// Drops the keys `gone` selects; each action they held stays held only while another of its keys is down.
    fn drop_keys(&mut self, gone: impl Fn(&HeldKey) -> bool) {
        let mut freed = Vec::new();
        self.keys.retain(|k| {
            let g = gone(k);
            if g {
                freed.push(k.action);
            }
            !g
        });
        for a in freed {
            if !self.keys.iter().any(|k| k.action == a) {
                self.held[slot(a)] = false;
            }
        }
    }

    /// The commands the held actions give.
    pub fn commands(&self) -> Commands {
        Commands {
            forward: self.held(Action::Forward),
            backward: self.held(Action::Back),
            left: self.held(Action::Left),
            right: self.held(Action::Right),
            jump: self.held(Action::Jump),
            crouch: self.held(Action::Crouch),
            // PROPOSED client default CLI-2 (staged): running is the normal pace, the walk key slows down
            run: !self.held(Action::Walk),
            run_lock_toggle: false,
            sprint: self.held(Action::Sprint),
            strafe_mod: self.held(Action::StrafeMod),
        }
    }
}

impl InputSource for KeyboardMouse {
    fn frame(&mut self, view: &mut ViewAngles) {
        let (dx, dy) = std::mem::take(&mut self.pending);
        if dx != 0.0 || dy != 0.0 {
            self.look.apply(view, dx, dy);
        }
    }

    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        let mut c = self.commands();
        // MOVEMENT_2009 §5.1: the run lock flips on every frame the command reads as on, so deliver one tick
        // per press (dc-sim's Commands::run_lock_toggle contract).
        if self.run_lock_presses > 0 {
            self.run_lock_presses -= 1;
            c.run_lock_toggle = true;
        }
        c
    }

    fn device(&mut self, event: DeviceInput) {
        match event {
            DeviceInput::Action(a, true) => self.press(a),
            DeviceInput::Action(a, false) => {
                self.keys.retain(|k| k.action != a);
                self.held[slot(a)] = false;
            }
            DeviceInput::Key {
                key,
                family,
                action,
                down: true,
            } => {
                if !self.keys.iter().any(|k| k.key == key) {
                    self.keys.push(HeldKey {
                        key,
                        family,
                        action,
                    });
                }
                self.press(action);
            }
            DeviceInput::Key {
                key, down: false, ..
            } => self.drop_keys(|k| k.key == key),
            DeviceInput::Modifiers {
                ctrl,
                alt,
                shift,
                logo,
            } => self.drop_keys(|k| match k.family {
                Some(ModifierKey::Ctrl) => !ctrl,
                Some(ModifierKey::Alt) => !alt,
                Some(ModifierKey::Shift) => !shift,
                Some(ModifierKey::Logo) => !logo,
                None => false,
            }),
            DeviceInput::MouseDelta(dx, dy) => {
                self.pending.0 += dx;
                self.pending.1 += dy;
            }
            DeviceInput::ReleaseAll => {
                // the shell queue survives a release: Escape queues ChatCancel and releases the cursor in one event
                self.held = [false; HELD_SLOTS];
                self.keys.clear();
                self.pending = (0.0, 0.0);
                self.fire = false;
                self.fire_press = false;
                self.use_held = false;
            }
            DeviceInput::Fire(down) => {
                // CLI-1601: the press is kept until the weapon reads it, so a press let go before the next frame
                // still fires (the 2009 press is an event, cshell+0x23b050)
                if down && !self.fire {
                    self.fire_press = true;
                }
                self.fire = down;
            }
            DeviceInput::Reload => self.reload = true,
            DeviceInput::Use => {
                self.use_key = true;
                self.use_held = true;
            }
            DeviceInput::UseUp => self.use_held = false,
            DeviceInput::Slot(n) => {
                if n < KEY_SLOTS {
                    self.slot = Some(n);
                }
            }
            DeviceInput::Wheel(d) => self.wheel = self.wheel.saturating_add(d),
            DeviceInput::Shell(k) => {
                if let ShellKey::ClassKey(n) = k {
                    self.class_key = Some(n);
                }
                // full queue: the oldest press goes, so an undrained queue never holds only stale keys
                if self.shell.len() >= SHELL_QUEUE_MAX {
                    self.shell.remove(0);
                }
                self.shell.push(k);
            }
        }
    }

    fn weapon(&mut self) -> WeaponCommands {
        WeaponCommands {
            fire: self.fire,
            fire_press: std::mem::take(&mut self.fire_press),
            reload: std::mem::take(&mut self.reload),
            use_key: std::mem::take(&mut self.use_key),
            use_held: self.use_held,
            select: self.slot.take(),
            wheel: std::mem::take(&mut self.wheel),
            zoom: self.held(Action::Zoom),
            class_key: self.class_key.take(),
        }
    }

    fn set_zoom_level(&mut self, z: u32) {
        self.look.zoom_level = z;
    }

    fn set_sensitivity(&mut self, s: f32) {
        self.look.sensitivity = s;
    }

    fn shell_keys(&mut self) -> Vec<ShellKey> {
        std::mem::take(&mut self.shell)
    }

    fn label(&self) -> &'static str {
        "keyboard+mouse"
    }
}

/// Length of the scripted sequence in seconds.
pub const SCRIPT_SECONDS: f64 = 5.0;

/// The scripted test sequence (our own fixture): move forward for 2 s at the normal (running) pace, stand and
/// turn 90 degrees to the right over 1 s, jump while moving forward, keep moving forward until 4 s, then
/// stand still. Defined on tick time `tick / hz`, so every frame rate produces the same tick inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptedInput {
    pub start: ViewAngles,
    /// Total turn of the turning phase (radians).
    pub turn: f32,
}

impl ScriptedInput {
    pub fn new(start: ViewAngles) -> ScriptedInput {
        ScriptedInput {
            start,
            turn: std::f32::consts::FRAC_PI_2,
        }
    }

    /// Commands and view at simulation time `t` seconds.
    pub fn at(&self, t: f64) -> (Commands, ViewAngles) {
        let forward = |jump: bool| Commands {
            forward: true,
            run: true,
            jump,
            ..Commands::default()
        };
        let idle = Commands {
            run: true,
            ..Commands::default()
        };
        let (cmd, turned) = if t < 2.0 {
            (forward(false), 0.0)
        } else if t < 3.0 {
            (idle, ((t - 2.0) as f32).clamp(0.0, 1.0))
        } else if t < 3.05 {
            (forward(true), 1.0)
        } else if t < 4.0 {
            (forward(false), 1.0)
        } else {
            (idle, 1.0)
        };
        let view = ViewAngles {
            yaw: wrap_angle(self.start.yaw + self.turn * turned),
            pitch: self.start.pitch,
        };
        (cmd, view)
    }

    /// Ticks of the whole sequence at `hz`.
    pub fn ticks(hz: u32) -> u64 {
        (SCRIPT_SECONDS * hz as f64).round() as u64
    }
}

impl InputSource for ScriptedInput {
    fn frame(&mut self, _view: &mut ViewAngles) {}

    fn tick(&mut self, tick: u64, hz: u32, view: &mut ViewAngles) -> Commands {
        let (c, v) = self.at(tick as f64 / hz.max(1) as f64);
        *view = v;
        c
    }

    fn label(&self) -> &'static str {
        "script"
    }
}

/// The movement test sequence of CLI-MOVE-1 (our own fixture): stand, walk, crouch-walk, run, a running jump,
/// sprint, and a standing jump, each phase on the simulation clock (`tick / hz` seconds), so every frame rate gives
/// the same tick inputs. The view keeps the start facing.
///
/// | from (s) | to (s) | commands |
/// |---|---|---|
/// | 0.0 | 0.5 | none (settle) |
/// | 0.5 | 2.5 | forward, walking |
/// | 2.5 | 3.0 | none |
/// | 3.0 | 5.0 | forward, crouched |
/// | 5.0 | 5.5 | none (stand up) |
/// | 5.5 | 6.5 | forward, running |
/// | 6.5 | 6.55 | forward, running, jump |
/// | 6.55 | 7.5 | forward, running |
/// | 7.5 | 9.5 | forward, running, sprint |
/// | 9.5 | 10.5 | none |
/// | 10.5 | 10.55 | jump (standing) |
/// | 10.55 | 12.0 | none |
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveScript {
    pub view: ViewAngles,
}

impl MoveScript {
    /// Length of the sequence in seconds.
    pub const SECONDS: f64 = 12.0;

    pub fn new(view: ViewAngles) -> MoveScript {
        MoveScript { view }
    }

    /// Phase name and commands at simulation time `t` seconds.
    pub fn at(&self, t: f64) -> (&'static str, Commands) {
        let c = |forward: bool, run: bool, crouch: bool, jump: bool, sprint: bool| Commands {
            forward,
            run,
            crouch,
            jump,
            sprint,
            ..Commands::default()
        };
        if t < 0.5 {
            ("settle", c(false, false, false, false, false))
        } else if t < 2.5 {
            ("walk", c(true, false, false, false, false))
        } else if t < 3.0 {
            ("stop", c(false, false, false, false, false))
        } else if t < 5.0 {
            ("crouch-walk", c(true, false, true, false, false))
        } else if t < 5.5 {
            ("stand-up", c(false, false, false, false, false))
        } else if t < 6.5 {
            ("run", c(true, true, false, false, false))
        } else if t < 6.55 {
            ("run-jump", c(true, true, false, true, false))
        } else if t < 7.5 {
            ("run", c(true, true, false, false, false))
        } else if t < 9.5 {
            ("sprint", c(true, true, false, false, true))
        } else if t < 10.5 {
            ("stop", c(false, false, false, false, false))
        } else if t < 10.55 {
            ("stand-jump", c(false, false, false, true, false))
        } else {
            ("rest", c(false, false, false, false, false))
        }
    }
}

impl InputSource for MoveScript {
    fn frame(&mut self, _view: &mut ViewAngles) {}

    fn tick(&mut self, tick: u64, hz: u32, view: &mut ViewAngles) -> Commands {
        *view = self.view;
        self.at(tick as f64 / hz.max(1) as f64).1
    }

    fn label(&self) -> &'static str {
        "move-script"
    }
}

/// Runs [`MoveScript`] from an anchor tick: no commands until the server has placed and released the body (player
/// state 1, at least `corrections` teleports applied, standing), then the script on the ticks after the anchor. With
/// no server (`life_state` `None`) the anchor is the first tick. Test input: the same anchored sequence at any frame
/// rate gives the same tick inputs from the anchor on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchoredScript {
    pub script: MoveScript,
    /// Teleports to wait for.
    pub corrections: u32,
    /// The anchor tick, once found.
    pub anchor: Option<u64>,
}

impl AnchoredScript {
    pub fn new(script: MoveScript, corrections: u32) -> AnchoredScript {
        AnchoredScript {
            script,
            corrections,
            anchor: None,
        }
    }
}

impl InputSource for AnchoredScript {
    fn frame(&mut self, _view: &mut ViewAngles) {}

    fn server(&mut self, v: &ServerHold) {
        if self.anchor.is_none()
            && v.life_state.is_none_or(|s| s == 1)
            && v.corrections >= self.corrections
            && v.on_ground
        {
            self.anchor = Some(v.next_tick);
        }
    }

    fn tick(&mut self, tick: u64, hz: u32, view: &mut ViewAngles) -> Commands {
        *view = self.script.view;
        match self.anchor {
            Some(a) if tick >= a => self.script.at((tick - a) as f64 / hz.max(1) as f64).1,
            _ => Commands::default(),
        }
    }

    fn label(&self) -> &'static str {
        "anchored-move-script"
    }
}

/// The scripted sequence repeated without end (`--walk-test`): every cycle starts facing where the last one ended,
/// so the body walks a closed square path (forward, turn a quarter right, jump forward, stand). Test traffic for a
/// network session (PROPOSED CLI-19, staged: DC_DEFAULTS.md lists this row as a second CLI-10, renumbering proposed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WalkTest {
    pub script: ScriptedInput,
}

impl WalkTest {
    pub fn new(start: ViewAngles) -> WalkTest {
        WalkTest {
            script: ScriptedInput::new(start),
        }
    }

    /// Commands and view at simulation time `t` seconds.
    pub fn at(&self, t: f64) -> (Commands, ViewAngles) {
        let cycle = (t / SCRIPT_SECONDS).floor().max(0.0);
        let (c, v) = self.script.at(t - cycle * SCRIPT_SECONDS);
        let turned = (cycle as f32 * self.script.turn) % std::f32::consts::TAU;
        (
            c,
            ViewAngles {
                yaw: wrap_angle(v.yaw + turned),
                pitch: v.pitch,
            },
        )
    }
}

impl InputSource for WalkTest {
    fn frame(&mut self, _view: &mut ViewAngles) {}

    fn tick(&mut self, tick: u64, hz: u32, view: &mut ViewAngles) -> Commands {
        let (c, v) = self.at(tick as f64 / hz.max(1) as f64);
        *view = v;
        c
    }

    fn label(&self) -> &'static str {
        "walk-test"
    }
}

/// Test input that shoots (`--shoot-test`): it stands still (or walks the [`WalkTest`] square with `walk`), turns the
/// view each frame toward the nearest remote player within `range` (its centre plus half its half height, the
/// bot's aim point, BOT-12) and holds the fire button while one is in front of it; after `burst_ms` of fire it lets
/// go for `pause_ms` (so single-shot weapons fire again and the spread settles). Test traffic only.
// DC-DEFAULT: CLI-35 (proposed) test input: aim at the nearest remote player within 8000 units, 1500 ms bursts,
// 300 ms pauses
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShootTest {
    pub walk: Option<WalkTest>,
    pub range: f32,
    pub burst_ms: f64,
    pub pause_ms: f64,
    /// Seconds of frames seen (the frame loop's own clock: one `frame()` call per frame).
    t: f64,
    frame_s: f64,
    target: bool,
}

impl ShootTest {
    pub fn new(frame_s: f64, walk: Option<WalkTest>) -> ShootTest {
        ShootTest {
            walk,
            range: 8000.0,
            burst_ms: 1500.0,
            pause_ms: 300.0,
            t: 0.0,
            frame_s,
            target: false,
        }
    }

    /// The view that looks from `eye` at `to`.
    pub fn look_at(eye: [f32; 3], to: [f32; 3]) -> ViewAngles {
        let (yaw, pitch) = dc_botclient::shooter::aim_angles(eye, to);
        ViewAngles {
            yaw: wrap_angle(yaw),
            pitch,
        }
    }
}

impl InputSource for ShootTest {
    fn frame(&mut self, _view: &mut ViewAngles) {
        self.t += self.frame_s;
    }

    fn tick(&mut self, tick: u64, hz: u32, view: &mut ViewAngles) -> Commands {
        match self.walk.as_mut() {
            // keep the shooter's view: the walk only moves the body
            Some(w) => {
                let mut v = *view;
                w.tick(tick, hz, &mut v)
            }
            None => Commands {
                run: true,
                ..Commands::default()
            },
        }
    }

    fn aim(
        &mut self,
        eye: [f32; 3],
        remotes: &[RemoteBody],
        own_team: Option<u8>,
        view: &mut ViewAngles,
    ) {
        // DC-DEFAULT: CLI-35 team mates are never a target: with a known own team (1 or 2) only the other team's
        // bodies count (the user saw the scripted shooter empty its magazine into a team mate, 2026-10-27)
        let enemy = |r: &RemoteBody| match own_team {
            Some(t) if t == 1 || t == 2 => r.team != t,
            _ => true,
        };
        let d2 = |p: [f32; 3]| {
            (p[0] - eye[0]).powi(2) + (p[1] - eye[1]).powi(2) + (p[2] - eye[2]).powi(2)
        };
        let best = remotes
            .iter()
            .filter(|r| enemy(r))
            .filter(|r| r.pos.iter().all(|c| c.is_finite()))
            .min_by(|a, b| d2(a.pos).total_cmp(&d2(b.pos)));
        self.target = false;
        if let Some(r) = best {
            if d2(r.pos).sqrt() <= self.range {
                let to = [r.pos[0], r.pos[1] + 0.5 * r.half[1], r.pos[2]];
                *view = ShootTest::look_at(eye, to);
                self.target = true;
            }
        }
    }

    fn weapon(&mut self) -> WeaponCommands {
        let cycle = self.burst_ms + self.pause_ms;
        let in_burst = (self.t * 1000.0) % cycle.max(1.0) < self.burst_ms;
        WeaponCommands {
            fire: self.target && in_burst,
            ..WeaponCommands::default()
        }
    }

    fn label(&self) -> &'static str {
        "shoot-test"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_test_repeats_the_script_and_keeps_turning() {
        let w = WalkTest::new(ViewAngles::default());
        let (c, v) = w.at(1.0);
        assert!(c.forward && v.yaw == 0.0);
        let (c, v) = w.at(SCRIPT_SECONDS + 1.0);
        assert!(c.forward, "second cycle walks again");
        assert!((v.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5, "{v:?}");
        let (_, v) = w.at(4.0 * SCRIPT_SECONDS + 1.0);
        assert!(v.yaw.abs() < 1e-4, "four quarters are a full turn: {v:?}");
    }

    #[test]
    fn keys_map_to_commands_and_run_lock_is_one_tick_per_press() {
        let mut k = KeyboardMouse::new(LookParams::DEFAULT);
        let mut v = ViewAngles::default();
        k.device(DeviceInput::Action(Action::Forward, true));
        k.device(DeviceInput::Action(Action::Sprint, true));
        let c = k.tick(0, 60, &mut v);
        assert!(c.forward && c.run && c.sprint && !c.backward && !c.run_lock_toggle);
        k.device(DeviceInput::Action(Action::Walk, true));
        assert!(!k.tick(1, 60, &mut v).run);
        // a held run-lock key toggles once, a second press once more
        k.device(DeviceInput::Action(Action::RunLock, true));
        k.device(DeviceInput::Action(Action::RunLock, true));
        assert!(k.tick(2, 60, &mut v).run_lock_toggle);
        assert!(!k.tick(3, 60, &mut v).run_lock_toggle);
        k.device(DeviceInput::Action(Action::RunLock, false));
        k.device(DeviceInput::Action(Action::RunLock, true));
        assert!(k.tick(4, 60, &mut v).run_lock_toggle);
        k.device(DeviceInput::ReleaseAll);
        let c = k.tick(5, 60, &mut v);
        assert!(!c.forward && !c.sprint && c.run);
    }

    #[test]
    fn zoom_is_held_and_shell_keys_queue_in_order_until_drained() {
        let mut k = KeyboardMouse::new(LookParams::DEFAULT);
        assert!(!k.held(Action::Zoom));
        k.device(DeviceInput::Action(Action::Zoom, true));
        assert!(k.held(Action::Zoom));
        // zoom does not touch the movement commands
        let mut v = ViewAngles::default();
        assert_eq!(
            k.tick(0, 60, &mut v),
            Commands {
                run: true,
                ..Commands::default()
            }
        );
        k.device(DeviceInput::Shell(ShellKey::ClassKey(2)));
        k.device(DeviceInput::Shell(ShellKey::ChatOpen));
        k.device(DeviceInput::Shell(ShellKey::LeaveBox));
        assert_eq!(
            k.shell_keys(),
            vec![
                ShellKey::ClassKey(2),
                ShellKey::ChatOpen,
                ShellKey::LeaveBox
            ]
        );
        assert!(k.shell_keys().is_empty(), "drained");
        for _ in 0..100 {
            k.device(DeviceInput::Shell(ShellKey::ObserverNext));
        }
        assert_eq!(k.shell_keys().len(), SHELL_QUEUE_MAX, "bounded");
        for _ in 0..SHELL_QUEUE_MAX {
            k.device(DeviceInput::Shell(ShellKey::ObserverNext));
        }
        k.device(DeviceInput::Shell(ShellKey::LeaveBox));
        let q = k.shell_keys();
        assert_eq!(q.len(), SHELL_QUEUE_MAX, "bounded");
        assert_eq!(
            q.last(),
            Some(&ShellKey::LeaveBox),
            "full queue keeps the newest press"
        );
        // Escape while captured: the window queues ChatCancel, then releases the cursor (ReleaseAll)
        k.device(DeviceInput::Shell(ShellKey::ChatCancel));
        k.device(DeviceInput::ReleaseAll);
        assert!(!k.held(Action::Zoom));
        assert_eq!(
            k.shell_keys(),
            vec![ShellKey::ChatCancel],
            "release keeps the queued chat cancel"
        );
        assert!(ScriptedInput::new(v).shell_keys().is_empty());
    }

    #[test]
    fn a_fire_press_is_kept_until_the_weapon_reads_it() {
        // CLI-1601: a press let go before the next frame reaches the weapon once; the held level stays the fire key
        let mut k = KeyboardMouse::new(LookParams::DEFAULT);
        k.device(DeviceInput::Fire(true));
        k.device(DeviceInput::Fire(false));
        let w = k.weapon();
        assert!(w.fire_press && !w.fire);
        assert!(!k.weapon().fire_press, "one press, one read");
        k.device(DeviceInput::Fire(true));
        let w = k.weapon();
        assert!(w.fire_press && w.fire);
        k.device(DeviceInput::Fire(true));
        let w = k.weapon();
        assert!(!w.fire_press && w.fire, "held: no new press");
        k.device(DeviceInput::Fire(false));
        k.device(DeviceInput::Fire(true));
        k.device(DeviceInput::ReleaseAll);
        let w = k.weapon();
        assert!(!w.fire_press && !w.fire, "released with the cursor");
    }

    #[test]
    fn the_weapon_reads_the_held_zoom_key_the_class_keys_and_the_zoom_level_slows_the_mouse() {
        let mut k = KeyboardMouse::new(LookParams::DEFAULT);
        k.device(DeviceInput::Action(Action::Zoom, true));
        k.device(DeviceInput::Shell(ShellKey::ClassKey(3)));
        let w = k.weapon();
        assert!(w.zoom);
        assert_eq!(w.class_key, Some(3));
        let w = k.weapon();
        assert!(w.zoom, "held");
        assert_eq!(w.class_key, None, "one press, one class step");
        assert_eq!(
            k.shell_keys(),
            vec![ShellKey::ClassKey(3)],
            "still a shell key"
        );
        k.device(DeviceInput::ReleaseAll);
        assert!(!k.weapon().zoom);
        // level 1: a quarter of the turn (MOVEMENT_2009.md §13.2: 1 / (3 z + 1))
        let mut v0 = ViewAngles::default();
        k.device(DeviceInput::MouseDelta(100.0, 0.0));
        k.frame(&mut v0);
        k.set_zoom_level(1);
        let mut v1 = ViewAngles::default();
        k.device(DeviceInput::MouseDelta(100.0, 0.0));
        k.frame(&mut v1);
        assert!(
            (v1.yaw * 4.0 - v0.yaw).abs() < 1e-6,
            "{} {}",
            v0.yaw,
            v1.yaw
        );
    }

    #[test]
    fn mouse_motion_is_applied_once_per_frame() {
        let mut k = KeyboardMouse::new(LookParams::DEFAULT);
        let mut v = ViewAngles::default();
        k.device(DeviceInput::MouseDelta(60.0, 0.0));
        k.device(DeviceInput::MouseDelta(40.0, 0.0));
        k.frame(&mut v);
        assert!((v.yaw - 100.0 * LookParams::DEFAULT.axis_scale()).abs() < 1e-6);
        let before = v;
        k.frame(&mut v);
        assert_eq!(v, before, "motion is consumed");
    }

    #[test]
    fn script_phases() {
        let s = ScriptedInput::new(ViewAngles::default());
        let (c, v) = s.at(1.0);
        assert!(c.forward && !c.jump && v.yaw == 0.0);
        let (c, v) = s.at(2.5);
        assert!(!c.forward && (v.yaw - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        let (c, _) = s.at(3.01);
        assert!(c.forward && c.jump);
        let (c, v) = s.at(4.5);
        assert!(!c.forward && (v.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(ScriptedInput::ticks(60), 300);
    }
}
