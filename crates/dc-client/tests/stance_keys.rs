//! Crouch, walk and fire held together and released in every order (playtest report: "if you crouch-walk
//! and shoot you get stuck crouching even after letting go, or you stop crouching but still run as slow as
//! walking"). The keyboard source and the client's own player are driven tick by tick on a flat floor; the
//! control word sent in the 0x64 update (`LocalUpdate::control`) and the speed must return to the standing run.
//! The keys arrive the way the window sends them: per physical key (`DeviceInput::Key`), with the modifier state
//! (`DeviceInput::Modifiers`) that recovers a Ctrl / Alt key-up the platform never delivered.
// SPEC: MOVEMENT_2009.md §5.1 (CROUCH 0x20, RUN 0x200), §5.4 (crouch release: 100 ms + stand-up test), §7 (speeds)
// DC-DEFAULT: CLI-1520, CLI-1521 (proposed); CLI-1 / CLI-2 (C and Left Ctrl crouch, Left Alt walk, run by default)

use dc_client::input::{Action, DeviceInput, InputSource, KeyboardMouse, ModifierKey};
use dc_client::look::{LookParams, ViewAngles};
use dc_client::world::SpawnPoint;
use dc_client::{LocalPlayer, LocalUpdate};
use dc_sim::player::bits;
use dc_sim::{CollModel, CollisionWorld, ConvexPoly, MovementParams, SURF_SOLID, V3};

const HZ: u32 = 60;
/// Scan codes of the default keys (settings::KeyBinds::default): C, Left Ctrl, Left Alt, Caps Lock.
const KEY_C: u32 = 46;
const KEY_CTRL: u32 = 29;
const KEY_ALT: u32 = 56;
const KEY_CAPS: u32 = 58;

fn floor() -> CollisionWorld {
    let q = ConvexPoly::from_corners(
        vec![
            V3::new(-50000.0, 0.0, -50000.0),
            V3::new(-50000.0, 0.0, 50000.0),
            V3::new(50000.0, 0.0, 50000.0),
            V3::new(50000.0, 0.0, -50000.0),
        ],
        SURF_SOLID,
    );
    CollisionWorld::from_parts(vec![CollModel::from_polys(&[q])], &[])
}

/// The keys the window sends for crouch (Left Ctrl), walk (Left Alt), C (crouch) and Caps Lock (run lock).
fn key(k: u32, down: bool) -> DeviceInput {
    let (family, action) = match k {
        KEY_CTRL => (Some(ModifierKey::Ctrl), Action::Crouch),
        KEY_ALT => (Some(ModifierKey::Alt), Action::Walk),
        KEY_C => (None, Action::Crouch),
        KEY_CAPS => (None, Action::RunLock),
        _ => unreachable!(),
    };
    DeviceInput::Key {
        key: k,
        family,
        action,
        down,
    }
}

fn mods(ctrl: bool, alt: bool) -> DeviceInput {
    DeviceInput::Modifiers {
        ctrl,
        alt,
        shift: false,
        logo: false,
    }
}

struct Rig {
    w: CollisionWorld,
    p: LocalPlayer,
    k: KeyboardMouse,
    view: ViewAngles,
    tick: u64,
    /// Horizontal speed of the last tick (units / s).
    speed: f32,
}

impl Rig {
    fn new() -> Rig {
        let w = floor();
        let sp = SpawnPoint {
            pos: V3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(
            MovementParams::default(),
            &w,
            &sp,
            V3::new(15.0, 50.0, 15.0),
            HZ,
        );
        p.state.crouch_dims = Some(V3::new(15.0, 40.0, 15.0));
        p.set_life_state(1);
        let mut r = Rig {
            w,
            p,
            k: KeyboardMouse::new(LookParams::DEFAULT),
            view: ViewAngles::default(),
            tick: 0,
            speed: 0.0,
        };
        r.k.device(DeviceInput::Action(Action::Forward, true));
        r.run(60);
        r
    }

    fn ev(&mut self, e: DeviceInput) {
        self.k.device(e);
    }

    /// Runs `n` ticks the way the window does: the frame's weapon read (the fire button), then the tick.
    fn run(&mut self, n: u32) {
        for _ in 0..n {
            let _fire = self.k.weapon();
            let c = self.k.tick(self.tick, HZ, &mut self.view);
            let a = self.p.state.pos;
            self.p.tick(&self.w, c, &self.view);
            let b = self.p.state.pos;
            self.speed = ((b.x - a.x).powi(2) + (b.z - a.z).powi(2)).sqrt() * HZ as f32;
            self.tick += 1;
        }
    }

    /// The control word of the 0x64 update the client sends now.
    fn sent(&self) -> u32 {
        LocalUpdate::from_state(&self.p.state, &self.view, 0.0).control
    }

    fn assert_standing_run(&mut self, what: &str) {
        // a second for the stand-up, the eye return and the ramp
        self.run(HZ);
        let c = self.sent();
        assert_eq!(c & bits::CROUCH, 0, "{what}: CROUCH still sent ({c:#x})");
        assert_ne!(c & bits::RUN, 0, "{what}: RUN not sent ({c:#x})");
        let run = MovementParams::default().run_forward;
        assert!(
            (self.speed - run).abs() < 1.0,
            "{what}: speed {} (run {run})",
            self.speed
        );
    }

    /// Crouch (Left Ctrl), walk (Left Alt) and fire go down in that order with ticks between.
    fn crouch_walk_fire(&mut self) {
        self.ev(mods(true, false));
        self.ev(key(KEY_CTRL, true));
        self.run(5);
        self.ev(mods(true, true));
        self.ev(key(KEY_ALT, true));
        self.run(5);
        self.ev(DeviceInput::Fire(true));
        self.run(30);
        let c = self.sent();
        assert_ne!(c & bits::CROUCH, 0, "crouched while held ({c:#x})");
        let crouch = MovementParams::default().crouch_speed;
        assert!(
            (self.speed - crouch).abs() < 1.0,
            "crouch speed {}",
            self.speed
        );
    }
}

#[test]
fn crouch_walk_fire_released_in_every_order_returns_to_the_standing_run() {
    let orders: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for o in orders {
        let mut r = Rig::new();
        r.crouch_walk_fire();
        let (mut ctrl, mut alt) = (true, true);
        for i in o {
            match i {
                0 => {
                    ctrl = false;
                    r.ev(mods(ctrl, alt));
                    r.ev(key(KEY_CTRL, false));
                }
                1 => {
                    alt = false;
                    r.ev(mods(ctrl, alt));
                    r.ev(key(KEY_ALT, false));
                }
                _ => r.ev(DeviceInput::Fire(false)),
            }
            r.run(3);
        }
        r.assert_standing_run(&format!("release order {o:?}"));
    }
}

#[test]
fn a_lost_ctrl_or_alt_key_up_is_recovered_from_the_modifier_state() {
    // the platform delivers the modifier state but not the key-ups of Ctrl and Alt (either one, or both)
    for (lose_ctrl, lose_alt) in [(true, false), (false, true), (true, true)] {
        let mut r = Rig::new();
        r.crouch_walk_fire();
        r.ev(DeviceInput::Fire(false));
        r.ev(mods(false, true));
        if !lose_ctrl {
            r.ev(key(KEY_CTRL, false));
        }
        r.run(2);
        r.ev(mods(false, false));
        if !lose_alt {
            r.ev(key(KEY_ALT, false));
        }
        r.assert_standing_run(&format!("lost ctrl {lose_ctrl}, lost alt {lose_alt}"));
        // the walk key works the right way round afterwards: held walks, released runs
        r.ev(mods(false, true));
        r.ev(key(KEY_ALT, true));
        r.run(HZ);
        assert_eq!(r.sent() & bits::RUN, 0, "walk key held: no RUN");
        let walk = MovementParams::default().walk_speed;
        assert!((r.speed - walk).abs() < 1.0, "walk speed {}", r.speed);
        r.ev(mods(false, false));
        r.ev(key(KEY_ALT, false));
        r.assert_standing_run("walk key released");
    }
}

#[test]
fn two_keys_of_one_action_hold_it_until_both_are_up() {
    let mut r = Rig::new();
    r.ev(key(KEY_C, true));
    r.ev(mods(true, false));
    r.ev(key(KEY_CTRL, true));
    r.run(10);
    // Ctrl goes up, C is still down: still crouched
    r.ev(mods(false, false));
    r.ev(key(KEY_CTRL, false));
    r.run(HZ);
    assert_ne!(r.sent() & bits::CROUCH, 0, "C still holds the crouch");
    // the modifier state never releases a key outside its family
    r.ev(mods(false, false));
    r.run(5);
    assert_ne!(r.sent() & bits::CROUCH, 0, "C is not a Ctrl key");
    r.ev(key(KEY_C, false));
    r.assert_standing_run("both crouch keys up");
}

#[test]
fn the_modifier_state_before_a_modifier_press_does_not_release_it_and_run_lock_counts_once() {
    let mut r = Rig::new();
    // the window sends the new modifier state before the key event of the same press
    r.ev(mods(true, false));
    r.ev(key(KEY_CTRL, true));
    r.ev(mods(true, false));
    r.run(10);
    assert_ne!(r.sent() & bits::CROUCH, 0, "crouched");
    r.ev(mods(false, false));
    r.ev(key(KEY_CTRL, false));
    r.assert_standing_run("crouch released");
    // Caps Lock (run lock) down twice without an up between (a repeat that slipped through) still flips once
    r.ev(key(KEY_CAPS, true));
    r.ev(key(KEY_CAPS, true));
    r.run(HZ);
    assert_eq!(r.sent() & bits::RUN, 0, "run lock on: walking");
    r.ev(key(KEY_CAPS, false));
    r.ev(key(KEY_CAPS, true));
    r.ev(key(KEY_CAPS, false));
    r.assert_standing_run("run lock off again");
}
