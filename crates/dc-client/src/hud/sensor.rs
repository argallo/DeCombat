//! The heartbeat sensor: a radar accessory carried as a weapon (the weapon rows with `SensorOption` 0 or 1). While its
//! user holds the fire key with the sensor drawn and does not sprint, the radar shows the enemies that stand inside the
//! camera's view and within the row's scan distance; walls do not hide them. A battery limits it: it drains while the
//! sensor scans and recharges while it does not. The rim image of the radar shows the charge.
//!
//! What the 2009 client does, by part (the radar element `hud_radar`, RADAR_2009.md):
//! - the weapon row: `SensorOption` (-1 without), `ScanDist`, the four battery keys (cshell+0x17d04f..0x17d0d3);
//! - the loadout: a sensor row among the weapons the player is given arms the battery at its maximum, once per row
//!   (cshell+0x60d90 -> cshell+0xb5390), the round begin fills it again (cshell+0x49dbd -> cshell+0xb4f00 with 1) and a
//!   spawn clears the timers (cshell+0xb4f00 with 2);
//! - the switch: per frame the local player update gives the radar "on" while the fire key's scan flag of the drawn
//!   sensor is set and the player does not sprint (cshell+0x6285b..0x6287a); the radar sends `UP_SCAN` 0xcd sub 1
//!   on / off and drains or refills the battery (cshell+0xb7a20, cshell+0xb5230, cshell+0xb50b0);
//! - the display: every frame each enemy entry is tested against the camera's view frustum while the sensor scans
//!   (cshell+0xb5050 -> the engine's six-plane test engine+0x2fdd0) and drawn with the enemy icon while its radar
//!   distance is within the scan distance (cshell+0xb60f0); the rim image shows the charge in steps of ten.
//!
//! DeCombat keeps the rules and leaves out what only the 2009 server's answer did (the client does it itself, see
//! [`Sensor::step`]) and the team share of the `SensorOption` 1 row (its `UP_SPOTTED` 0x84 per enemy, relayed to the team).
// SPEC: rust/specs/RADAR_2009.md §R-5 (the enemy rule), GAME_MESSAGES_2009_C2S.md §7.3 (0xcd), GAME_MESSAGES_2009_S2C.md
// §7.1 (0xcd, 0x84)

use dc_wire::udp::BitWriter;

/// Keys of a weapon row's sensor numbers.
// OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] SensorOption, ScanDist, BatteryMaxQuantity, BatteryRefillQuantity,
// BatteryUsingQuantity, BatteryRefillTime (read at run time)
// DISASM: cshell+0x17d04f..0x17d0d3 (the row loader cshell+0x17bd20: `ScanDist` -> row +0x48c, `BatteryRefillQuantity`
// -> +0x490, `BatteryUsingQuantity` -> +0x494, `BatteryMaxQuantity` -> +0x498, `BatteryRefillTime` -> +0x49c,
// `SensorOption` -> +0x21c with the default -1)
pub mod key {
    pub const OPTION: &str = "SensorOption";
    pub const SCAN_DIST: &str = "ScanDist";
    pub const MAX: &str = "BatteryMaxQuantity";
    pub const REFILL: &str = "BatteryRefillQuantity";
    pub const USING: &str = "BatteryUsingQuantity";
    pub const REFILL_TIME: &str = "BatteryRefillTime";
}

/// `Guntype` of the sensor rows: only a drawn weapon of this type is the selected sensor.
// DISASM: cshell+0x489cf..0x48a00 (the weapon change: a row with `Guntype` 0xb calls the radar's cshell+0xb54b0 with
// selected 1 and the row's `SensorOption`, any other row with 0), cshell+0x23b26c (the scan flag only for 0xb)
pub const GUN_TYPE_SENSOR: u32 = 0xb;

/// The sensor numbers of one weapon row (all integers, as the 2009 loader reads them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SensorSpec {
    /// `SensorOption`: 0 the heartbeat sensor, 1 the one that also shares (its share is not implemented), 2 / 3 other
    /// scanners (not this module).
    pub option: i32,
    /// `ScanDist`, in radar distance (world distance x the radar's character scale).
    pub scan_dist: i32,
    /// `BatteryMaxQuantity`, `BatteryRefillQuantity`, `BatteryUsingQuantity`.
    pub max: i32,
    pub refill: i32,
    pub using: i32,
    /// `BatteryRefillTime`: milliseconds between two drain or refill steps.
    pub step_ms: i32,
}

impl SensorSpec {
    /// The row's sensor numbers from its keys (`num` reads one key as a number); `None` when the row has no
    /// `SensorOption` or -1. A missing battery key counts 0.
    // DC-DEFAULT: CLI-1912 (proposed) a sensor row without a battery or distance key takes 0 for it (the 2009 loader
    // keeps the value the weapon object held before, not traced)
    pub fn of_row(num: &dyn Fn(&str) -> Option<f32>) -> Option<SensorSpec> {
        let i = |k: &str| num(k).map(|v| v as i32);
        let option = i(key::OPTION)?;
        if option == -1 {
            return None;
        }
        Some(SensorSpec {
            option,
            scan_dist: i(key::SCAN_DIST).unwrap_or(0),
            max: i(key::MAX).unwrap_or(0),
            refill: i(key::REFILL).unwrap_or(0),
            using: i(key::USING).unwrap_or(0),
            step_ms: i(key::REFILL_TIME).unwrap_or(0),
        })
    }

    /// A row this module runs: the heartbeat sensors (options 0 and 1).
    pub fn heartbeat(&self) -> bool {
        matches!(self.option, 0 | 1)
    }
}

/// `UP_SCAN` and its switch body `[sub:u8 = 1][team:u16][on:u8]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §7.3
// DISASM: cshell+0xb7a6e..0xb7ad7 (on: id 0xcd, 8 bits 1, 16 bits the radar's own team +0x1a0, 8 bits 1),
// cshell+0xb7b30..0xb7b92 (off: the same with 0)
pub const UP_SCAN: u8 = 0xcd;
pub const SCAN_SWITCH: u8 = 1;

pub fn scan_body(team: u16, on: bool) -> BitWriter {
    let mut b = BitWriter::new();
    b.write_u8(SCAN_SWITCH).write_u16(team).write_u8(on as u8);
    b
}

/// `GS_RADAR_PING` and its switch answer `[kind:u8 = 1][on:u8][weapon:u32]` (other kinds are not the sensor's).
// SPEC: GAME_MESSAGES_2009_S2C.md §7.1 (0xcd); DISASM cshell+0x451d6..0x45285 (kind 8 bits, the second byte 8 bits,
// kind 1 then 32 bits -> cshell+0xb8650)
pub const GS_RADAR_PING: u8 = 0xcd;

pub fn read_switch_answer(r: &mut dc_wire::udp::BitReader<'_>) -> Option<bool> {
    if r.remaining_bits() < 16 || r.read_u8() != SCAN_SWITCH {
        return None;
    }
    Some(r.read_u8() != 0)
}

/// Which of the two battery forms of the layout the rim shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RimForm {
    /// The radar's rim form (the charge while it scans, and full).
    Charge,
    /// The layout's second battery form (recharging, and empty).
    Recharge,
}

/// What the HUD needs of the sensor in a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorView {
    /// The rim picture: the form and the number printed into it.
    pub rim: (RimForm, i32),
    /// The rim's alpha (the blink after a full recharge).
    pub rim_alpha: u8,
    /// While the sensor scans: the scan distance (radar distance) within which enemies in view are drawn.
    pub range: Option<f32>,
}

/// The frame's facts the sensor runs on.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SensorInput {
    /// The first carried heartbeat-sensor row: its weapon number and numbers.
    pub carried: Option<(u16, SensorSpec)>,
    /// That row is the drawn weapon (and its `Guntype` is [`GUN_TYPE_SENSOR`]).
    pub held: bool,
    /// The drawn weapon is in its firing state (the fire key down).
    pub firing: bool,
    pub sprinting: bool,
    pub alive: bool,
}

/// The sensor state of the own player (the battery and the switch of the 2009 radar element).
#[derive(Debug, Clone, PartialEq)]
pub struct Sensor {
    /// The armed row (+0x2c8 and the row's numbers).
    row: Option<(u16, SensorSpec)>,
    /// A sensor row is in the loadout (+0x3a0).
    carried: bool,
    /// The charge (+0x2ac).
    charge: i32,
    /// The switch sent "on" and not "off" yet (+0x2b8).
    active: bool,
    /// The first drain of this scan was taken (+0x2b9).
    drained: bool,
    /// The empty battery's "off" went out for this press.
    empty_sent: bool,
    /// The server answered this press's "on" with "off" (refused): no scan and no drain until the next press, or
    /// until a later "on" answer.
    refused: bool,
    last_drain: u32,
    last_refill: u32,
    /// A recharge reached the maximum at this time: the rim blinks (+0x3b4 / +0x3a8).
    full_at: Option<u32>,
    rim: (RimForm, i32),
    alpha: u8,
    /// The radar shows enemies this frame.
    scanning: bool,
}

impl Default for Sensor {
    fn default() -> Self {
        Sensor {
            row: None,
            carried: false,
            charge: 0,
            active: false,
            drained: false,
            empty_sent: false,
            refused: false,
            last_drain: 0,
            last_refill: 0,
            full_at: None,
            rim: (RimForm::Charge, 0),
            alpha: 255,
            scanning: false,
        }
    }
}

/// The blink after a full recharge: on, off, on, off for one second each, then the plain full picture.
// EVIDENCE: cshell+0xb49e0 (alpha 0xff below 1000 ms and from 2000 to 2999, 0 from 1000 to 1999 and 3000 to 3999; from
// 4000 to 4999 the full picture with alpha 0xff and the flag cleared)
// DC-DEFAULT: CLI-1913 (proposed) a frame later than 4999 ms ends the blink as well (2009 leaves it set)
const BLINK_STEP_MS: u32 = 1000;
const BLINK_STEPS: u32 = 4;

impl Sensor {
    /// The own player state changed (0x67): a spawn clears the switch and the timers, the charge stays.
    // EVIDENCE: cshell+0xb4f00 (argument 2: +0x2b8, +0x2b9, the two timers, the full flag and the selection cleared);
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.1 (state 1 calls it with 2)
    pub fn on_life(&mut self, state: u8) {
        if state == 1 {
            self.active = false;
            self.drained = false;
            self.empty_sent = false;
            self.refused = false;
            self.last_drain = 0;
            self.last_refill = 0;
            self.full_at = None;
            self.alpha = 255;
            self.scanning = false;
        }
    }

    /// A round begins (0x44): the battery is full again.
    // EVIDENCE: cshell+0xb4f00 (argument 1: charge := the row's maximum, the rim form printed with it); DISASM
    // cshell+0x49d07, cshell+0x49dbd (the 0x44 handler calls it with 1)
    pub fn on_round_begin(&mut self) {
        if let Some((_, s)) = self.row {
            self.charge = s.max;
            self.rim = (RimForm::Charge, self.charge);
        }
    }

    /// The server's answer to a switch (`GS_RADAR_PING` kind 1): "off" while this client scans stops the scan and the
    /// drain until the key is let go (the server refused it: no sensor drawn on its side, or the round ended); a
    /// later "on" (the server took the sensor's draw after the switch) lets it go on.
    // EVIDENCE: cshell+0xb8650 (the 2009 client's answer handler: "off" clears the range, so no enemy is drawn; its
    // switch flag +0x2b8 stays, and the next frame's own range write would show them again)
    // DC-DEFAULT: CLI-1912 (amended) a refused scan also stops the drain and stays off until the next press
    pub fn on_answer(&mut self, on: bool) {
        if self.active {
            self.refused = !on;
            if !on {
                self.scanning = false;
            }
        }
    }

    /// One frame at `now` (ms); returns the switch to send (`UP_SCAN` on / off), if any.
    ///
    /// DeCombat leaves out the 2009 server's answer to the switch (0xcd kind 1 back, cshell+0xb8650): with "off" it
    /// set the range to 0 and restarted the refill timer; this client does that itself when it sends "off".
    // EVIDENCE: cshell+0xb7a20 (on: not yet active and charge > the row's refill quantity -> send on, active; off and
    // active -> send off, inactive; active and charge > refill quantity -> drain step and the range; off and selected
    // and charge < max -> refill step; on, active and charge <= refill quantity -> send off), cshell+0xb60f0 (head: not
    // selected, carried and charge < max -> refill step; else a full flag -> blink), cshell+0xb5390 (a new row: charge
    // := max, rim form with it), cshell+0xb8650 (the answer "off": range 0, drain timer 0, +0x2b9 0, refill timer := now)
    // DISASM: cshell+0x6285b..0x6287a ("on" = the scan flag `local_player`+0x478 and not the sprint bit 0x400000),
    // cshell+0x23b26c..0x23b282 (the fire press in the firing state sets the flag for an order-class 4 row of `Guntype`
    // 0xb), cshell+0x23af82..0x23afb5 (cleared when the weapon's animation state returns to 0xe)
    // DC-DEFAULT: CLI-1912 (proposed) "on" = the sensor drawn, its firing state, not sprinting, alive; the empty battery's
    // "off" is sent once per press (2009: every frame while the key stays down)
    pub fn step(&mut self, now: u32, i: &SensorInput) -> Option<bool> {
        // the loadout
        match i.carried {
            Some((w, spec)) => {
                if self.row.map(|r| r.0) != Some(w) {
                    self.row = Some((w, spec));
                    self.charge = spec.max;
                    self.rim = (RimForm::Charge, self.charge);
                    self.full_at = None;
                    self.alpha = 255;
                }
                self.carried = true;
            }
            None => self.carried = false,
        }
        let Some((_, spec)) = self.row.filter(|_| self.carried) else {
            let out = self.active.then_some(false);
            self.active = false;
            self.scanning = false;
            return out;
        };
        let on = i.held && i.firing && !i.sprinting && i.alive;
        let mut out = None;
        if !on {
            if self.active {
                out = Some(false);
                self.active = false;
                self.released(now);
            }
        } else if !self.active && self.charge > spec.refill {
            out = Some(true);
            self.active = true;
            self.empty_sent = false;
            self.refused = false;
        }
        self.scanning = false;
        if self.active && self.refused {
            // refused by the server: held without a scan, nothing drains or refills
        } else if self.active && spec.refill < self.charge {
            self.drain(now, &spec);
            self.scanning = i.held;
        } else if !on {
            if i.held && self.charge < spec.max {
                self.drained = false;
                self.refill(now, &spec, i.alive);
            }
        } else if self.active && !self.empty_sent {
            self.drained = false;
            self.empty_sent = true;
            out = Some(false);
        }
        // the radar's own update: the refill while the sensor is put away, else the blink of a full battery
        if !i.held && self.charge < spec.max {
            self.refill(now, &spec, i.alive);
        } else if let Some(t) = self.full_at {
            let k = now.saturating_sub(t) / BLINK_STEP_MS;
            if k >= BLINK_STEPS {
                self.alpha = 255;
                self.full_at = None;
            } else {
                self.alpha = if k.is_multiple_of(2) { 255 } else { 0 };
            }
        }
        out
    }

    /// The 2009 server's answer to "off", done here (see [`Sensor::step`]).
    fn released(&mut self, now: u32) {
        self.refused = false;
        self.last_drain = 0;
        self.drained = false;
        self.last_refill = now;
    }

    /// One drain step: the first frame of a scan, then every step time while the charge covers the use.
    // EVIDENCE: cshell+0xb5230 (step time passed since +700 and charge >= the use, or active and not yet drained: charge
    // -= the use, at least 0; the rim: the second form at 0, else the rim form, printed with the charge rounded down to
    // tens; a running blink is ended with alpha 0xff)
    fn drain(&mut self, now: u32, s: &SensorSpec) {
        let due = now.saturating_sub(self.last_drain) as i64 >= s.step_ms as i64 && self.charge >= s.using;
        if due || (self.active && !self.drained) {
            self.charge = (self.charge - s.using).max(0);
            self.last_drain = now;
            let form = if self.charge == 0 {
                RimForm::Recharge
            } else {
                RimForm::Charge
            };
            self.rim = (form, self.charge / 10 * 10);
            self.drained = true;
            if self.full_at.take().is_some() {
                self.alpha = 255;
            }
        }
    }

    /// One refill step every step time; reaching the maximum starts the blink.
    // EVIDENCE: cshell+0xb50b0 (gate cshell+0x1f0820; step time passed since +0x2c0: charge += the refill quantity, at
    // the maximum clamped and the full flag set with the time; the rim: the rim form when full, else the second form,
    // printed with the charge rounded down to tens)
    // DC-DEFAULT: CLI-1912 (proposed) the gate (a test on an object of the local player, cshell+0x1f0820, not traced) is
    // read as "alive"
    fn refill(&mut self, now: u32, s: &SensorSpec, alive: bool) {
        if !alive || (now.saturating_sub(self.last_refill) as i64) < s.step_ms as i64 {
            return;
        }
        self.last_refill = now;
        self.charge += s.refill;
        if self.charge >= s.max {
            self.charge = s.max;
            self.full_at = Some(now);
        }
        let form = if self.full_at.is_some() {
            RimForm::Charge
        } else {
            RimForm::Recharge
        };
        self.rim = (form, self.charge / 10 * 10);
    }

    /// The HUD's part; `None` while no sensor row is carried (the radar's plain rim).
    pub fn view(&self) -> Option<SensorView> {
        let (_, s) = self.row.filter(|_| self.carried)?;
        Some(SensorView {
            rim: self.rim,
            rim_alpha: self.alpha,
            range: self.scanning.then_some(s.scan_dist as f32),
        })
    }

    /// The charge (tests, logs).
    pub fn charge(&self) -> i32 {
        self.charge
    }
}

/// Whether world point `p` lies inside the camera's view frustum: between the near and far planes and within both
/// field-of-view angles (the full view, not a drawn window). Nothing in between is tested: walls do not count.
// EVIDENCE: cshell+0xb5050 (the camera object `camera_mgr`+0x12c and the entry's position to the draw-primitive
// interface's slot +0xbc); DISASM: engine+0x2fdd0 (the slot builds the six planes of the camera's frustum from its
// fields of view, view rectangle and near / far distances and answers 1 when the point is on the inner side of all six)
pub fn in_view(cam: &dc_render::Camera, p: [f32; 3]) -> bool {
    let d = glam::Vec3::from_array(p) - cam.pos;
    let c = cam.rot.inverse() * d;
    if !c.is_finite() || c.z < cam.near() || c.z > cam.far() {
        return false;
    }
    let tx = (cam.fov_x * 0.5).tan();
    let ty = (cam.fov_y * 0.5).tan();
    c.x.abs() <= c.z * tx && c.y.abs() <= c.z * ty
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SensorSpec {
        // made-up numbers: 100 charge, 5 back per step, 10 per step of use, 1000 ms steps, distance 1600
        SensorSpec {
            option: 0,
            scan_dist: 1600,
            max: 100,
            refill: 5,
            using: 10,
            step_ms: 1000,
        }
    }

    fn input(held: bool, firing: bool) -> SensorInput {
        SensorInput {
            carried: Some((31, spec())),
            held,
            firing,
            sprinting: false,
            alive: true,
        }
    }

    #[test]
    fn a_row_without_the_option_or_with_minus_one_is_no_sensor() {
        let none = |_: &str| None;
        assert_eq!(SensorSpec::of_row(&none), None);
        let off = |k: &str| (k == key::OPTION).then_some(-1.0);
        assert_eq!(SensorSpec::of_row(&off), None);
        let on = |k: &str| match k {
            key::OPTION => Some(0.0),
            key::SCAN_DIST => Some(1600.0),
            key::MAX => Some(100.0),
            _ => None,
        };
        let s = SensorSpec::of_row(&on).unwrap();
        assert_eq!((s.option, s.scan_dist, s.max, s.using), (0, 1600, 100, 0));
        assert!(s.heartbeat());
        assert!(!SensorSpec { option: 2, ..s }.heartbeat());
    }

    #[test]
    fn holding_fire_switches_on_drains_every_step_and_off_at_the_release() {
        let mut s = Sensor::default();
        assert_eq!(s.step(0, &input(true, false)), None);
        assert_eq!(s.view().unwrap().rim, (RimForm::Charge, 100));
        assert_eq!(s.view().unwrap().range, None);
        // the press: on, the first drain at once, the range shown
        assert_eq!(s.step(100, &input(true, true)), Some(true));
        assert_eq!(s.charge(), 90);
        assert_eq!(s.view().unwrap().range, Some(1600.0));
        assert_eq!(s.view().unwrap().rim, (RimForm::Charge, 90));
        // no second drain before the step time
        assert_eq!(s.step(1099, &input(true, true)), None);
        assert_eq!(s.charge(), 90);
        assert_eq!(s.step(1100, &input(true, true)), None);
        assert_eq!(s.charge(), 80);
        // sprinting counts as off
        let sprint = SensorInput {
            sprinting: true,
            ..input(true, true)
        };
        assert_eq!(s.step(1200, &sprint), Some(false));
        assert_eq!(s.view().unwrap().range, None);
        // the refill waits one step after the release, then 5 per step in the second form
        assert_eq!(s.step(1300, &input(true, false)), None);
        assert_eq!(s.charge(), 80);
        s.step(2200, &input(true, false));
        assert_eq!(s.charge(), 85);
        assert_eq!(s.view().unwrap().rim, (RimForm::Recharge, 80));
    }

    #[test]
    fn an_empty_battery_switches_off_once_and_refills_to_a_blink() {
        let mut s = Sensor::default();
        s.step(0, &input(true, false));
        let mut sent = Vec::new();
        for t in (0..20_000).step_by(100) {
            if let Some(on) = s.step(t, &input(true, true)) {
                sent.push((t, on));
            }
        }
        // ten drains from 100, the last (10 -> 0, 10 being above the refill quantity 5) at 9000 ms; the next frame
        // finds 0 <= 5 and sends off, once
        assert_eq!(sent, vec![(0, true), (9100, false)]);
        assert_eq!(s.charge(), 0);
        assert_eq!(s.view().unwrap().rim, (RimForm::Recharge, 0));
        assert_eq!(s.view().unwrap().range, None);
        // held on an empty battery: no refill
        s.step(25_000, &input(true, true));
        assert_eq!(s.charge(), 0);
        // let go (off again, as 2009 sends it at the release) and put away: the refill to full, then the blink
        assert_eq!(s.step(25_100, &input(false, false)), Some(false));
        let mut t = 25_100;
        while s.charge() < 100 {
            t += 100;
            s.step(t, &input(false, false));
        }
        assert_eq!(s.view().unwrap().rim, (RimForm::Charge, 100));
        assert_eq!(s.view().unwrap().rim_alpha, 255);
        s.step(t + 1500, &input(false, false));
        assert_eq!(s.view().unwrap().rim_alpha, 0);
        s.step(t + 2500, &input(false, false));
        assert_eq!(s.view().unwrap().rim_alpha, 255);
        s.step(t + 4200, &input(false, false));
        assert_eq!(s.view().unwrap().rim_alpha, 255);
        assert_eq!(s.full_at, None);
    }

    #[test]
    fn a_charge_at_the_refill_quantity_does_not_start_and_a_new_row_or_round_is_full() {
        let mut s = Sensor::default();
        s.step(0, &input(true, false));
        s.charge = 5;
        assert_eq!(s.step(10, &input(true, true)), None);
        assert_eq!(s.view().unwrap().range, None);
        s.on_round_begin();
        assert_eq!(s.charge(), 100);
        // a spawn keeps the charge
        s.charge = 40;
        s.on_life(1);
        assert_eq!(s.charge(), 40);
        // another sensor row arms full
        let other = SensorInput {
            carried: Some((93, spec())),
            ..input(false, false)
        };
        s.step(20, &other);
        assert_eq!(s.charge(), 100);
        // no sensor carried: no view, an active scan is switched off
        s.step(30, &input(true, true));
        assert_eq!(s.step(40, &SensorInput::default()), Some(false));
        assert!(s.view().is_none());
    }

    #[test]
    fn a_refused_on_stops_the_scan_and_the_drain_until_the_next_press() {
        let mut s = Sensor::default();
        s.step(0, &input(true, false));
        assert_eq!(s.step(100, &input(true, true)), Some(true));
        assert_eq!(s.charge(), 90);
        // an answer while not scanning changes nothing; the server's "off" while scanning refuses it
        s.on_answer(false);
        assert_eq!(s.view().unwrap().range, None);
        for t in (200..3000).step_by(100) {
            assert_eq!(s.step(t, &input(true, true)), None, "no switch while held");
        }
        assert_eq!(s.charge(), 90, "no drain while refused");
        assert_eq!(s.view().unwrap().range, None);
        // a later "on" (the server took the sensor's draw) lets it go on
        s.on_answer(true);
        s.step(3000, &input(true, true));
        assert_eq!(s.view().unwrap().range, Some(1600.0));
        assert_eq!(s.charge(), 80);
        // refused again, then let go and pressed: a fresh "on"
        s.on_answer(false);
        s.step(3100, &input(true, true));
        assert_eq!(s.view().unwrap().range, None);
        assert_eq!(s.step(3200, &input(true, false)), Some(false));
        assert_eq!(s.step(3300, &input(true, true)), Some(true));
        assert_eq!(s.view().unwrap().range, Some(1600.0));
        // not scanning: an "off" answer (to the release) is ignored at the next press
        let mut q = Sensor::default();
        q.step(0, &input(true, false));
        q.on_answer(false);
        assert_eq!(q.step(10, &input(true, true)), Some(true));
        assert_eq!(q.view().unwrap().range, Some(1600.0));
    }

    #[test]
    fn the_switch_answer_reads_kind_one_only() {
        let mut w = BitWriter::new();
        w.write_u8(1).write_u8(0).write_u32(31);
        assert_eq!(read_switch_answer(&mut w.reader()), Some(false));
        let mut on = BitWriter::new();
        on.write_u8(1).write_u8(1).write_u32(31);
        assert_eq!(read_switch_answer(&mut on.reader()), Some(true));
        let mut other = BitWriter::new();
        other.write_u8(2).write_u8(1).write_u32(0).write_u32(0);
        assert_eq!(read_switch_answer(&mut other.reader()), None);
        assert_eq!(read_switch_answer(&mut BitWriter::new().reader()), None);
    }

    #[test]
    fn the_switch_body_is_sub_team_on() {
        let b = scan_body(2, true);
        let mut r = b.reader();
        assert_eq!((r.read_u8(), r.read_u16(), r.read_u8()), (1, 2, 1));
        assert_eq!(b.bit_len(), 32);
    }

    #[test]
    fn rom_sensor_rows_and_radar_battery_keys_resolve() {
        let Ok(rom) = std::env::var("CA_ROM") else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let rom = std::path::Path::new(&rom);
        let t = crate::arms::WeaponTable::open(rom).expect("weapon table");
        let sensors: Vec<(u16, SensorSpec)> = t
            .rows
            .iter()
            .filter_map(|(w, r)| r.sensor.map(|s| (*w, s)))
            .collect();
        // the ROM has heartbeat rows of both options, of the sensor `Guntype`, with a battery that can start a scan
        for opt in [0, 1] {
            let (w, s) = sensors
                .iter()
                .find(|(_, s)| s.option == opt)
                .unwrap_or_else(|| panic!("a SensorOption {opt} row"));
            assert_eq!(t.rows[w].gun_type, Some(GUN_TYPE_SENSOR), "row {w}");
            assert!(s.scan_dist > 0 && s.max > s.refill && s.using > 0 && s.step_ms > 0, "row {w}");
        }
        let mut hud = crate::hud::Hud::from_rom(rom, 1024, 768, None).expect("ROM HUD");
        let l = hud.radar.as_ref().expect("ROM radar").layout.clone();
        let (Some(charge), Some(fill), Some(enemy)) = (&l.rim_form, &l.fill_form, &l.enemy_icon) else {
            panic!("battery forms and enemy icon");
        };
        let s = sensors.iter().find(|(_, s)| s.option == 0).unwrap().1;
        for v in [0, s.max / 2 / 10 * 10, s.max] {
            for f in [charge, fill] {
                let tex = f.replacen("%d", &v.to_string(), 1);
                assert!(hud.assets.texture(&tex).is_some(), "battery texture {v}");
            }
        }
        assert!(hud.assets.texture(enemy).is_some(), "enemy icon");
        // the radar while the sensor scans: two enemies ahead within the range, one behind, one beyond the range, a
        // mate; DC_SENSOR_PNG=<dir> writes the pictures (scanning, recharging)
        use crate::hud::state::{BoardRow, HudState};
        let row = |pid: u8, team: u8| BoardRow {
            pid,
            name: format!("p{pid}"),
            team,
            kills: 0,
            deaths: 0,
            dead: false,
            ping: None,
        };
        let body = |pid: u8, team: u8, pos: [f32; 3]| crate::world::RemoteBody {
            obj: 100 + pid as u16,
            pid,
            name: format!("p{pid}"),
            team,
            pos,
            half: [16.0, 50.0, 16.0],
            yaw: None,
            look: None,
        };
        let mut st = HudState {
            own_pid: Some(0),
            life_state: 1,
            ..HudState::default()
        };
        for r in [row(0, 1), row(1, 1), row(2, 2), row(3, 2), row(4, 2), row(5, 2)] {
            st.roster.insert(r.pid, r);
        }
        let near = s.scan_dist as f32 / l.char_scale.max(1e-3) * 0.4;
        let remotes = [
            body(1, 1, [-near, 0.0, -near * 0.5]),
            body(2, 2, [near * 0.3, 0.0, near]),
            body(3, 2, [-near * 0.6, 0.0, near * 0.8]),
            body(4, 2, [0.0, 0.0, -near]),
            body(5, 2, [0.0, 0.0, near * 3.0]),
        ];
        let cam = dc_render::Camera::looking(
            glam::Vec3::new(0.0, 50.0, 0.0),
            0.0,
            0.0,
            90f32.to_radians(),
            75f32.to_radians(),
        );
        let view = |rim: (RimForm, i32), range: Option<f32>| SensorView {
            rim,
            rim_alpha: 255,
            range,
        };
        let mut pictures = Vec::new();
        for (name, v) in [
            ("scanning", view((RimForm::Charge, 70), Some(s.scan_dist as f32))),
            ("recharging", view((RimForm::Recharge, 40), None)),
        ] {
            st.sensor = Some(v);
            let f = crate::hud::radar::frame(&l, &st, &remotes, [0.0, 50.0, 0.0], 0.0, &|p| {
                in_view(&cam, p)
            });
            let enemies: Vec<u8> = f
                .blips
                .iter()
                .filter(|b| b.kind == crate::hud::radar::BlipKind::Enemy)
                .map(|b| b.pid)
                .collect();
            let expect: Vec<u8> = if v.range.is_some() { vec![2, 3] } else { vec![] };
            assert_eq!(enemies, expect, "{name}");
            st.radar = Some(f);
            pictures.push((name, hud.render(&st)));
        }
        if let Ok(dir) = std::env::var("DC_SENSOR_PNG") {
            for (name, img) in pictures {
                image::save_buffer(
                    format!("{dir}/sensor_{name}.png"),
                    &img.rgba,
                    img.width,
                    img.height,
                    image::ExtendedColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn the_view_frustum_has_no_walls_and_ends_at_the_angles() {
        let cam = dc_render::Camera::looking(
            glam::Vec3::ZERO,
            0.0,
            0.0,
            90f32.to_radians(),
            70f32.to_radians(),
        );
        assert!(in_view(&cam, [0.0, 0.0, 500.0]));
        assert!(in_view(&cam, [490.0, 0.0, 500.0]));
        assert!(!in_view(&cam, [510.0, 0.0, 500.0]));
        assert!(!in_view(&cam, [0.0, 0.0, -500.0]));
        assert!(!in_view(&cam, [0.0, 0.0, 3.0]), "nearer than the near plane");
        assert!(!in_view(&cam, [0.0, 400.0, 500.0]), "above the vertical angle");
    }
}
