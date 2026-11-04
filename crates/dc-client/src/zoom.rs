//! The scope of the local player: the zoom level the zoom key cycles, the field-of-view transition, the zoom sounds
//! and the scope overlay slots.
//!
//! The 2009 client keeps the zoom on its player-manager object: an integer level (+0xf0), a "changing" flag (+0xf4)
//! and the direction of the change (+0xf8). The zoom command (0x3d) cycles the level `0 -> 1 -> .. -> ZoomLevel -> 0`
//! of the sight mod of the held weapon (the mod filed under socket 1, `Kinds` 1); while the level is at least 1 or
//! the flag is set, the shot cone uses the zoomed bounds and the 0x82 `moving` byte, the 0x64 `pu_flags` 0x0800 and
//! the sprint test of the mover read "zoomed". A change moves the horizontal field of view linearly toward the sight's
//! `ZoomFov<level - 1>` (zooming in) or back to the normal field (zooming out) at
//! `(normal - target) / ZoomTime` radians per second (`(normal - 20 degrees) / ZoomTime` when zooming out); the
//! vertical field follows in proportion. The mouse turns `1 / (3 z + 1)` as fast at level `z`
//! ([`crate::look::LookParams::zoom_level`]).
//!
//! Every step is a function of the session clock: the transition is evaluated at the frame's time from where and
//! when it started, never by adding a per-frame amount, so the field of view, the flags and the bytes they feed do
//! not depend on the frame rate (project decision: no frame-rate cap).
// EVIDENCE: cshell+0x1f4180 (the zoom step: sight mod via slot +0x11c(1), `Type` 2 toggles a flag instead, `ZoomLevel`
// mod +0x110, the cycle, +0xf4 / +0xf8, sound cshell+0x1f3590(0), overlays cshell+0x1b8bc0), cshell+0x1f95d0 (command
// 0x3d -> cshell+0x1f4180(-1)), cshell+0x1f9100 (the transition: rates, ZoomTime row +0x284, end of the change),
// cshell+0x1f1620 (`ZoomFov<level - 1>`), cshell+0x176f10 (mod row loader: Type, ZoomLevel, ZoomFov<i> (at most 10),
// ZoomSpriteName, ZoomSpriteScale, Kinds, SilencerLevel, ZoomInSound, ZoomOutSound), cshell+0x175730 (mod constructor
// defaults: Type 6, Kinds -1, ZoomSpriteScale 0.001), cshell+0x1f3590 (zoom sound / instant reset)
// SPEC: MOVEMENT_2009.md §5.1 (control 0x80000 = command 0x3d and not sprinting), §7 step 2 (sprint test reads +0xf0 /
// +0xf4), §13.2 step 2 (mouse / (3 z + 1)); GAME_MESSAGES_2009_C2S.md §2.1 (pu_flags 0x0800), §4.1 (`moving`);
// WEAPON_FIRE_2009_addendum_cone.md AC-0 item 4 (the zoom byte, PROVEN-REPLAY)
// OBSERVED: capture 20261025_153705_rust7 (capanalyze of the real client: the first press sets
// pu_flags 0x0800 in the same update, the second press clears it 120-200 ms later, the 0x80000 bit only while the key
// is held); 20261025_125432_rust6 (0x80000 in 13 updates with a weapon without a sight: pu_flags 0x0800 never set)

use crate::hud::scope::{
    OverlaySprite, OverlayTable, OVERLAY_CHANGE_IN, OVERLAY_CHANGE_OUT, OVERLAY_LEVEL_SLOTS,
    OVERLAY_SCOPE, OVERLAY_SLOTS,
};
use dc_bute::SectionMap;

/// Control-word bit of the held zoom command (sent in the 0x64 update).
// SPEC: MOVEMENT_2009.md §5.1 (0x00080000: command 0x3d on and not sprinting); OBSERVED: rust6 / rust7 updates.csv
pub const ZOOM_CONTROL_BIT: u32 = 0x0008_0000;
/// The 0x64 `pu_flags` bit of the zoom state (level >= 1 or a change running).
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (0x0800 = local_player +0xf0 > 0 or +0xf4 != 0); OBSERVED: rust7 updates.csv
pub const ZOOM_PU_FLAG: u16 = 0x0800;
/// The mod socket of the sight (`Kinds` 1) and of the muzzle (`Kinds` 0).
// DISASM: cshell+0x235ec0 (a mod is filed under socket `Kinds` 0..2), cshell+0x1f4180 (the zoom asks socket 1),
// cshell+0x2368dd (the silenced sound asks socket 0)
pub const SIGHT_SOCKET: usize = 1;
pub const MUZZLE_SOCKET: usize = 0;
/// The mod `Type` whose zoom key toggles a flag instead of zooming.
// EVIDENCE: cshell+0x1f4180 (mod +8 == 2: player manager +0x1c4 toggled, no zoom)
pub const TYPE_TOGGLE: i32 = 2;
/// `ZoomTime` when the weapon row has none (seconds).
// EVIDENCE: cshell+0x17faa2 (row default 0x3f000000 = 0.5)
pub const ZOOM_TIME_DEFAULT: f32 = 0.5;
/// The field a zoom-out runs its rate against (radians): `(normal - 20 degrees) / ZoomTime` per second.
// EVIDENCE: cshell+0x1f9100 (0.34906587 subtracted from the normal field when the level is 0)
pub const ZOOM_OUT_REFERENCE: f32 = 0.349_065_87;
/// A bolt-action (`ReloadAfterFire`) weapon zooms out this long after a zoomed shot or a reload asked while zoomed
/// (milliseconds; the zoom command is issued once the time is over and the scope is still up).
// DISASM: cshell+0x1f8b40 (`300 < GetTickCount - +0x1b4` with +0x1b3 set, zoomed and row +0x200 -> command 0x3d)
pub const BOLT_UNZOOM_MS: u32 = 300;
/// Most `ZoomFov<i>` keys a mod row carries.
// EVIDENCE: cshell+0x176f10 (the key probe stops after 10)
pub const MAX_ZOOM_FOVS: usize = 10;
/// Mod row keys read here (names only; the values are the user's ROM).
// OBSERVED: ROM table keys WEAPONS.TXT [Mod<n>] (names read by the 2009 loader cshell+0x176f10)
pub mod key {
    pub const TYPE: &str = "Type";
    pub const ZOOM_LEVEL: &str = "ZoomLevel";
    pub const ZOOM_FOV: &str = "ZoomFov";
    pub const SPRITE: &str = "ZoomSpriteName";
    pub const SPRITE_SCALE: &str = "ZoomSpriteScale";
    pub const KINDS: &str = "Kinds";
    pub const SILENCER: &str = "SilencerLevel";
    pub const SOUND_IN: &str = "ZoomInSound";
    pub const SOUND_OUT: &str = "ZoomOutSound";
    // OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] / [Mod<n>] CameraSwayXSpeed .. CameraSwayYFreq
    // DISASM: cshell+0x17e038..0x17e0a2 (weapon row +0x3f0 / +0x3f4 / +0x3f8 / +0x3fc), cshell+0x176f10 (mod row
    // +0x2a4 / +0x2a8 / +0x2ac / +0x2b0)
    pub const SWAY_X_SPEED: &str = "CameraSwayXSpeed";
    pub const SWAY_Y_SPEED: &str = "CameraSwayYSpeed";
    pub const SWAY_X_FREQ: &str = "CameraSwayXFreq";
    pub const SWAY_Y_FREQ: &str = "CameraSwayYFreq";
}

/// The scope sway of one row: the view turns at `speed x 0.001 x sin(freq x 0.1 x t)` radians per second (`t` the
/// clock in seconds), yaw from the X pair and pitch from the Y pair, while the scope is up (zoom level >= 1 or a
/// change running). The held weapon's row and its three mod rows add up.
// EVIDENCE: cshell+0x1f2b70 (sum of the three mod rows +0x2a4..+0x2b0 and the weapon row +0x3f0..+0x3fc; controller
// +0x48 pitch += (Ys) sin((Yf) x time x 0.1) x dt x 0.001, +0x4c yaw += (Xs) sin((Xf) x time x 0.1) x dt x 0.001,
// time = client interface +0x6c, dt = shell +0x1770 (frame time, at most 0.1 s, cshell+0x1aa810)), cshell+0x1f4fbe..
// 0x1f4fd6 (called from the controller's frame update through vtable +0xe4 when +0xf0 > 0 or +0xf4 != 0)
// DC-DEFAULT: CLI-670 (proposed) the rate is integrated exactly between frames on the session clock (no frame-rate
// dependence), at most 0.1 s per frame as the 2009 clamp
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CameraSway {
    pub x_speed: f32,
    pub y_speed: f32,
    pub x_freq: f32,
    pub y_freq: f32,
}

impl CameraSway {
    /// Weapon row defaults before the keys are read (a mod row's are 0).
    // EVIDENCE: cshell+0x17f7b0 (row +0x3f0 := 12.0, +0x3f4 := 2.0, +0x3f8 := 6.0, +0x3fc := 20.0), cshell+0x175730
    // (mod +0x2a4..+0x2b0 := 0)
    pub const WEAPON_DEFAULT: CameraSway = CameraSway {
        x_speed: 12.0,
        y_speed: 2.0,
        x_freq: 6.0,
        y_freq: 20.0,
    };
    /// Longest frame the sway integrates (seconds).
    // DISASM: cshell+0x1aa842..0x1aa855 (shell +0x1770 := 0.1 when the frame time is longer)
    pub const MAX_FRAME_S: f64 = 0.1;

    /// The row's sway; a missing key keeps `dflt`.
    pub fn of_section(sec: &SectionMap, dflt: CameraSway) -> CameraSway {
        CameraSway {
            x_speed: num(sec, key::SWAY_X_SPEED).unwrap_or(dflt.x_speed),
            y_speed: num(sec, key::SWAY_Y_SPEED).unwrap_or(dflt.y_speed),
            x_freq: num(sec, key::SWAY_X_FREQ).unwrap_or(dflt.x_freq),
            y_freq: num(sec, key::SWAY_Y_FREQ).unwrap_or(dflt.y_freq),
        }
    }

    /// Field by field sum (the weapon row plus its mods).
    pub fn plus(self, o: CameraSway) -> CameraSway {
        CameraSway {
            x_speed: self.x_speed + o.x_speed,
            y_speed: self.y_speed + o.y_speed,
            x_freq: self.x_freq + o.x_freq,
            y_freq: self.y_freq + o.y_freq,
        }
    }

    /// The view change `(yaw, pitch)` in radians from clock `t0` to `t1` (seconds).
    pub fn delta(&self, t0: f64, t1: f64) -> (f32, f32) {
        if !(t1 > t0) {
            return (0.0, 0.0);
        }
        let t0 = t0.max(t1 - Self::MAX_FRAME_S);
        // integral of s x 0.001 x sin(w t) dt with w = f x 0.1
        let axis = |speed: f32, freq: f32| -> f32 {
            let w = f64::from(freq) * 0.1;
            if w == 0.0 || speed == 0.0 {
                return 0.0;
            }
            (f64::from(speed) * 0.001 * ((w * t0).cos() - (w * t1).cos()) / w) as f32
        };
        (
            axis(self.x_speed, self.x_freq),
            axis(self.y_speed, self.y_freq),
        )
    }

    /// The half swing of the yaw and the pitch (radians): `speed x 0.001 / (freq x 0.1)`.
    pub fn amplitude(&self) -> (f32, f32) {
        let a = |s: f32, f: f32| if f == 0.0 { 0.0 } else { s * 0.01 / f };
        (a(self.x_speed, self.x_freq), a(self.y_speed, self.y_freq))
    }
}

/// Mod row defaults the constructor stores before the keys are read.
// EVIDENCE: cshell+0x175730 (+8 := 6, +0x17c := -1, +0x19c := 0x3a83126f, +0x110 / +0x1a8 := 0)
pub mod mod_default {
    pub const TYPE: i32 = 6;
    pub const KINDS: i32 = -1;
    pub const SPRITE_SCALE: f32 = 0.001;
}

/// What the scope and the fire sound read from one `[Mod<n>]` row.
#[derive(Debug, Clone, PartialEq)]
pub struct ModInfo {
    /// `Type` (mod +8): 0..6 may be attached; 2 = a toggle sight.
    pub kind: i32,
    /// `Kinds` (mod +0x17c): the socket 0..2, -1 none.
    pub socket: i32,
    /// `ZoomLevel` (mod +0x110): the highest zoom level, 0 = no zoom.
    pub zoom_level: u32,
    /// `ZoomFov0..`: the horizontal field of each level, degrees (level `z` uses entry `z - 1`).
    pub zoom_fov: Vec<f32>,
    pub sprite: Option<String>,
    pub sprite_scale: f32,
    /// `SilencerLevel` (mod +0x1a8): 1..9 picks the row's silenced fire sound.
    pub silencer: i32,
    pub sound_in: Option<String>,
    pub sound_out: Option<String>,
    /// `CameraSway*` (mod +0x2a4..+0x2b0), added to the weapon row's while the scope is up.
    pub sway: CameraSway,
    /// `ShotsPerClipPlus<k>` (mod +0x1b0 + 4 k): rounds a magazine mod adds to the clip of a weapon whose `ModModel`
    /// for it is `k` (0 when missing). Signed: the Fast Loader rows take rounds away (`-6` on a 30-round rifle).
    // EVIDENCE: cshell+0x176f10 (k < 10, read as an integer), cshell+0x233430 (the clip size adds the socket-2 mod's
    // entry with a plain signed add)
    pub clip_plus: [i32; 10],
    /// `Fireanirate` (mod +0x2a0): added over the three sockets to 1 to make the fire clip's play rate.
    // EVIDENCE: cshell+0x238110 (sum of the three mods' +0x2a0, plus 1, is the rate every fire clip is played with)
    pub fire_clip_rate: f32,
    /// `Reloadanirate` (mod +0x2b8): added over the three sockets to 1 to make the reload clip's play rate (the
    /// extended magazines -0.1 / -0.2, the Fast Loaders +0.2 / +0.3).
    // EVIDENCE: cshell+0x23a210 (sum of the three mods' +0x2b8, plus 1, is the reload clip's rate)
    pub reload_clip_rate: f32,
    /// The recoil terms of the row (see [`ModRecoil`]).
    pub recoil: ModRecoil,
}

/// The recoil keys of a `[Mod<n>]` row as the 2009 kick reads them.
// EVIDENCE: cshell+0x176f10 (MaxCamRecoilPitchPer +0x238, BaseCamRecoilPitchPer +0x23c, MaxRandomPitch +0x254,
// BaseCamRecoilPitch +0x258, BaseCamRecoilAccelPitch +0x25c; all default 0)
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ModRecoil {
    /// `MaxCamRecoilPitchPer`: the kick's pitch bound is `(1 + this) x` the weapon row's.
    pub max_per: f32,
    /// `BaseCamRecoilPitchPer` and `BaseCamRecoilPitch`: their sum `s` makes the kick's base `(1 + s) x` the row's
    /// (the suppressors' -0.1).
    pub base_per: f32,
    pub base: f32,
    /// `BaseCamRecoilAccelPitch`: added to the row's acceleration term.
    pub accel: f32,
    /// `MaxRandomPitch`: added to the row's yaw-kick threshold.
    pub random_pitch: f32,
}

/// Mod row keys of the fire cycle and the recoil.
// OBSERVED: ROM table keys WEAPONS.TXT [Mod<n>] Fireanirate, Reloadanirate, MaxCamRecoilPitchPer, BaseCamRecoilPitchPer,
// BaseCamRecoilPitch, BaseCamRecoilAccelPitch, MaxRandomPitch (names read by cshell+0x176f10)
pub mod rate_key {
    pub const FIRE: &str = "Fireanirate";
    pub const RELOAD: &str = "Reloadanirate";
    pub const MAX_PER: &str = "MaxCamRecoilPitchPer";
    pub const BASE_PER: &str = "BaseCamRecoilPitchPer";
    pub const BASE: &str = "BaseCamRecoilPitch";
    pub const ACCEL: &str = "BaseCamRecoilAccelPitch";
    pub const RANDOM_PITCH: &str = "MaxRandomPitch";
}

impl Default for ModInfo {
    fn default() -> Self {
        ModInfo {
            kind: mod_default::TYPE,
            socket: mod_default::KINDS,
            zoom_level: 0,
            zoom_fov: Vec::new(),
            sprite: None,
            sprite_scale: mod_default::SPRITE_SCALE,
            silencer: 0,
            sound_in: None,
            sound_out: None,
            sway: CameraSway::default(),
            clip_plus: [0; 10],
            fire_clip_rate: 0.0,
            reload_clip_rate: 0.0,
            recoil: ModRecoil::default(),
        }
    }
}

fn get<'a>(sec: &'a SectionMap, k: &str) -> Option<&'a String> {
    sec.iter()
        .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
        .map(|(_, v)| v)
}

fn num(sec: &SectionMap, k: &str) -> Option<f32> {
    let t = get(sec, k)?.trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

fn text(sec: &SectionMap, k: &str) -> Option<String> {
    let t = get(sec, k)?.trim().trim_matches('"').trim();
    (!t.is_empty()).then(|| t.to_string())
}

impl ModInfo {
    /// The row's numbers; a missing key keeps the constructor default.
    pub fn of_section(sec: &SectionMap) -> ModInfo {
        let d = ModInfo::default();
        let int = |k: &str, dflt: i32| num(sec, k).map_or(dflt, |v| v as i32);
        let mut zoom_fov = Vec::new();
        for i in 0..MAX_ZOOM_FOVS {
            match num(sec, &format!("{}{i}", key::ZOOM_FOV)) {
                Some(v) => zoom_fov.push(v),
                None => break,
            }
        }
        ModInfo {
            kind: int(key::TYPE, d.kind),
            socket: int(key::KINDS, d.socket),
            zoom_level: int(key::ZOOM_LEVEL, 0).max(0) as u32,
            zoom_fov,
            sprite: text(sec, key::SPRITE),
            sprite_scale: num(sec, key::SPRITE_SCALE).unwrap_or(d.sprite_scale),
            silencer: int(key::SILENCER, 0),
            sound_in: text(sec, key::SOUND_IN),
            sound_out: text(sec, key::SOUND_OUT),
            sway: CameraSway::of_section(sec, d.sway),
            clip_plus: std::array::from_fn(|k| {
                num(sec, &format!("ShotsPerClipPlus{k}")).map_or(0, |v| v as i32)
            }),
            fire_clip_rate: num(sec, rate_key::FIRE).unwrap_or(0.0),
            reload_clip_rate: num(sec, rate_key::RELOAD).unwrap_or(0.0),
            recoil: ModRecoil {
                max_per: num(sec, rate_key::MAX_PER).unwrap_or(0.0),
                base_per: num(sec, rate_key::BASE_PER).unwrap_or(0.0),
                base: num(sec, rate_key::BASE).unwrap_or(0.0),
                accel: num(sec, rate_key::ACCEL).unwrap_or(0.0),
                random_pitch: num(sec, rate_key::RANDOM_PITCH).unwrap_or(0.0),
            },
        }
    }

    /// The socket the mod is filed under (`Kinds` 0..2 and `Type` 0..6), else `None`.
    // DISASM: cshell+0x235ec0 (mod +8 in 0..6, `Kinds` in 0..2, else the attach fails)
    pub fn attach_socket(&self) -> Option<usize> {
        ((0..=6).contains(&self.kind) && (0..3).contains(&self.socket))
            .then_some(self.socket as usize)
    }

    /// The horizontal field of zoom level `z` (radians), `None` outside the row's list.
    // EVIDENCE: cshell+0x1f1620 (entry `z - 1` when 0 < z <= count, else 0)
    pub fn fov_of(&self, z: u32) -> Option<f32> {
        let i = usize::try_from(z).ok()?.checked_sub(1)?;
        self.zoom_fov
            .get(i)
            .copied()
            .filter(|v| *v != 0.0)
            .map(f32::to_radians)
    }

    /// The silencer level the fire sound uses (1..9), else `None`.
    // EVIDENCE: cshell+0x2330e0 (0 < SilencerLevel < 10 -> weapon sound 9), cshell+0x2368dd (the same range picks the
    // row's silenced sound `level - 1`)
    pub fn silencer_level(&self) -> Option<u8> {
        (1..10)
            .contains(&self.silencer)
            .then_some(self.silencer as u8)
    }
}

/// A sound the scope asks for (played locally, like the 2009 interface call with volume 100).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoomSound {
    pub file: String,
    /// True for the zoom-in sound.
    pub zoom_in: bool,
}

/// What a press of the zoom key did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// The level went up (a change toward the next field started).
    In(u32),
    /// The level went back to 0 (a change toward the normal field started).
    Out,
    /// A toggle sight flipped its flag.
    Toggled(bool),
    /// Nothing (no sight with a zoom, a change running, a refused state).
    None,
}

/// The transition of the horizontal field of view: linear in the session clock from `from` at `t0` toward `to`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Change {
    t0: u32,
    from: f32,
    to: f32,
    /// Radians per second (>= 0; infinite = at once).
    rate: f32,
}

impl Change {
    fn at(&self, now: u32) -> f32 {
        let dt = now.saturating_sub(self.t0) as f32 / 1000.0;
        let step = self.rate * dt;
        if self.to < self.from {
            (self.from - step).max(self.to)
        } else {
            (self.from + step).min(self.to)
        }
    }

    fn done(&self, now: u32) -> bool {
        self.at(now) == self.to
    }
}

/// The zoom state of the local player.
#[derive(Debug, Clone, Default)]
pub struct Zoom {
    level: u32,
    change: Option<Change>,
    zoom_in: bool,
    /// The horizontal field when no change runs and the level is >= 1 (radians); `None` = the normal field.
    fov: Option<f32>,
    /// A reload asked while zoomed runs at the end of the zoom-out.
    reload_after: bool,
    /// The flag a toggle sight flips.
    toggled: bool,
    overlays: OverlayTable,
    sounds: Vec<ZoomSound>,
    /// The first-person weapon models are hidden (weapon manager +0x11 cleared).
    weapons_hidden: bool,
    /// The bolt-action zoom-out timer (+0x1b3 / +0x1b4): armed at this time.
    bolt_unzoom: Option<u32>,
}

impl Zoom {
    /// The integer zoom level (player manager +0xf0).
    pub fn level(&self) -> u32 {
        self.level
    }

    /// A change is running (player manager +0xf4).
    pub fn changing(&self) -> bool {
        self.change.is_some()
    }

    /// The zoom state the cone, the 0x82 `moving` byte, `pu_flags` 0x0800 and the sprint test read.
    // DISASM: cshell+0x23bca0; SPEC: WEAPON_FIRE_2009.md §4.1 (`+0xf0 >= 1 or +0xf4 != 0`)
    pub fn active(&self) -> bool {
        self.level >= 1 || self.change.is_some()
    }

    /// The flag of a toggle sight.
    pub fn toggled(&self) -> bool {
        self.toggled
    }

    /// The horizontal field of view at `now` (radians), `None` for the normal field.
    pub fn fov_x(&self, now: u32) -> Option<f32> {
        match &self.change {
            Some(c) => Some(c.at(now)),
            None => self.fov,
        }
    }

    /// The first-person weapon (and its mod models) is hidden: from a zoom-in that leaves level 0 until the zoom-out
    /// back to level 0 ends, or the instant reset.
    // DISASM: cshell+0x1f4180 -> cshell+0x1f3520 (a change in from level 0) -> cshell+0x22d350 (every weapon of the
    // weapon manager: vtable +0x48(0), manager +0x11 := 0); cshell+0x1f9100 (a zoom-out ending at level 0) and
    // cshell+0x1f3590(1) -> cshell+0x22d320 (the held weapon: vtable +0x48(1), +0x11 := 1)
    pub fn weapons_hidden(&self) -> bool {
        self.weapons_hidden
    }

    /// Arms the bolt-action zoom-out at `now`: after a zoomed shot (`restamp`: the time is taken again) or a reload
    /// asked while zoomed (only when not armed yet).
    // DISASM: cshell+0x1f8b40 (a "fire now" update while zoomed with row +0x200: +0x1b3 := 1, +0x1b4 := now),
    // cshell+0x1f6520 (row +0x200 and +0x1b3 clear: the same, then +0x1ac := 1)
    pub fn arm_bolt_unzoom(&mut self, now: u32, restamp: bool) {
        if restamp || self.bolt_unzoom.is_none() {
            self.bolt_unzoom = Some(now);
        }
    }

    /// The bolt-action zoom-out is due: armed more than [`BOLT_UNZOOM_MS`] ago and the scope is up.
    // DISASM: cshell+0x1f8b40 (the test runs every update until a zoom-out step clears +0x1b3)
    pub fn bolt_unzoom_due(&self, now: u32) -> bool {
        self.active()
            && self
                .bolt_unzoom
                .is_some_and(|t| now.saturating_sub(t) > BOLT_UNZOOM_MS)
    }

    /// The overlay slots shown now.
    pub fn overlays(&self) -> &OverlayTable {
        &self.overlays
    }

    /// Sounds asked since the last call.
    pub fn take_sounds(&mut self) -> Vec<ZoomSound> {
        std::mem::take(&mut self.sounds)
    }

    /// A reload is waiting for the end of the zoom-out.
    pub fn reload_after(&self) -> bool {
        self.reload_after
    }

    // DC-DEFAULT: CLI-497 the sounds and the overlay sprite come from the sight mod (socket 1)
    fn sound(&mut self, sight: Option<&ModInfo>, zoom_in: bool) {
        let file = sight.and_then(|m| {
            if zoom_in {
                m.sound_in.clone()
            } else {
                m.sound_out.clone()
            }
        });
        if let Some(file) = file {
            self.sounds.push(ZoomSound { file, zoom_in });
        }
    }

    fn overlay(&mut self, slot: usize, sight: Option<&ModInfo>) {
        if self.overlays[slot].is_some() {
            return;
        }
        if let Some(sprite) = sight.and_then(|m| m.sprite.clone()) {
            self.overlays[slot] = Some(OverlaySprite {
                sprite,
                scale: sight.map_or(mod_default::SPRITE_SCALE, |m| m.sprite_scale),
            });
        }
    }

    /// Server 0x77 `GS_OVERLAY`: slot `slot` shows the sight's sprite (`on`) or nothing; slots >= 8 are ignored.
    // SPEC: GAME_MESSAGES_2009_S2C.md 0x77 row (cshell+0x1b90e0: create from the held weapon's sight when free, remove)
    pub fn server_overlay(&mut self, slot: u8, on: bool, sight: Option<&ModInfo>) {
        let s = usize::from(slot);
        if s >= OVERLAY_SLOTS {
            return;
        }
        if on {
            self.overlay(s, sight);
        } else {
            self.overlays[s] = None;
        }
    }

    /// The level asked by a press (`None`: the next level, wrapping to 0 past `ZoomLevel`) or by a forced zoom-out
    /// (`Some(0)`). `normal` is the normal horizontal field (radians), `zoom_time` the held weapon row's `ZoomTime`.
    /// The caller has already refused the press for the weapon's state (reload, a bolt cycling, sprinting).
    // EVIDENCE: cshell+0x1f4180 (the cycle; a press while a change runs or with `ZoomLevel` 0 does nothing; a change
    // in from level 0 opens the scope overlay; the sound is cshell+0x1f3590(0): ZoomInSound / ZoomOutSound of the sight)
    pub fn step_level(
        &mut self,
        now: u32,
        want: Option<u32>,
        sight: Option<&ModInfo>,
        normal: f32,
        zoom_time: f32,
    ) -> Press {
        let Some(m) = sight else {
            return Press::None;
        };
        if m.kind == TYPE_TOGGLE {
            if want.is_none() {
                self.toggled = !self.toggled;
                return Press::Toggled(self.toggled);
            }
            return Press::None;
        }
        if self.change.is_some() || m.zoom_level == 0 {
            return Press::None;
        }
        let old = self.level;
        let mut new = want.unwrap_or(old + 1);
        if new > m.zoom_level {
            new = 0;
        }
        self.level = new;
        let from = self.fov_x(now).unwrap_or(normal);
        let time = if zoom_time > 0.0 && zoom_time.is_finite() {
            zoom_time
        } else {
            0.0
        };
        if new > old {
            self.zoom_in = true;
            // DC-DEFAULT: CLI-500 a level without a `ZoomFov` entry keeps the normal field
            let to = m.fov_of(new).unwrap_or(normal);
            self.change = Some(Change {
                t0: now,
                from,
                to,
                rate: rate(normal - to, time),
            });
            if old == 0 {
                self.overlay(OVERLAY_SCOPE, sight);
                self.weapons_hidden = true;
            }
            self.overlay(OVERLAY_CHANGE_IN, sight);
            self.sound(sight, true);
            Press::In(new)
        } else if new < old {
            self.zoom_in = false;
            self.change = Some(Change {
                t0: now,
                from,
                to: normal,
                rate: rate(normal - ZOOM_OUT_REFERENCE, time),
            });
            self.overlay(OVERLAY_CHANGE_OUT, sight);
            self.sound(sight, false);
            // DISASM: cshell+0x1f4180 (a step down clears +0x1b3)
            self.bolt_unzoom = None;
            Press::Out
        } else {
            Press::None
        }
    }

    /// The end of a running change at `now`: the flag clears and the change overlays go; a zoom-out back at level 0
    /// removes the scope overlay. Returns true when a reload waited for this zoom-out (the caller starts it now).
    // EVIDENCE: cshell+0x1f9100 (the change ends when the field reaches its target: +0xf4 := 0, cshell+0x1b3890; at
    // level 0 cshell+0x1b37f0; the waiting reload runs cshell+0x1f6520)
    pub fn frame(&mut self, now: u32) -> bool {
        let Some(c) = self.change else {
            return false;
        };
        if !c.done(now) {
            return false;
        }
        self.change = None;
        self.overlays[OVERLAY_CHANGE_IN] = None;
        self.overlays[OVERLAY_CHANGE_OUT] = None;
        if self.zoom_in {
            self.fov = Some(c.to);
            return false;
        }
        self.fov = None;
        if self.level == 0 {
            for s in OVERLAY_LEVEL_SLOTS {
                self.overlays[s] = None;
            }
            self.weapons_hidden = false;
        }
        std::mem::take(&mut self.reload_after)
    }

    /// A reload asked while zoomed: the zoom goes out first and the reload runs when it ends.
    // EVIDENCE: cshell+0x1f6520 (zoomed and no change running: zoom-out cshell+0x1f4180(0) and +0x1ac := 1)
    pub fn defer_reload(&mut self) {
        self.reload_after = true;
    }

    /// The instant reset: level 0, no change, the normal field, every overlay gone, the zoom-out sound when the scope
    /// was up.
    // EVIDENCE: cshell+0x1f3590(1) (+0xf0 / +0xf4 / +0xf8 := 0, overlays cshell+0x1b3890 / cshell+0x1b37f0, camera
    // field := the normal one, then the sound of the direction 0 = ZoomOutSound); cshell+0x1f3b30, cshell+0x1fb3b0
    // DC-DEFAULT: CLI-495 the triggers (a weapon change, a death, a new life, the match end; the 2009 callers of the
    // reset were not traced)
    pub fn reset(&mut self, sight: Option<&ModInfo>) {
        let was = self.active();
        self.level = 0;
        self.change = None;
        self.zoom_in = false;
        self.fov = None;
        self.reload_after = false;
        self.overlays = Default::default();
        self.weapons_hidden = false;
        // DISASM: cshell+0x1f6430 (a weapon change clears +0x1b3)
        self.bolt_unzoom = None;
        if was {
            self.sound(sight, false);
        }
    }
}

fn rate(distance: f32, time: f32) -> f32 {
    if time <= 0.0 {
        f32::INFINITY
    } else {
        (distance / time).abs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sight(levels: u32, fovs: &[f32]) -> ModInfo {
        ModInfo {
            kind: 1,
            socket: 1,
            zoom_level: levels,
            zoom_fov: fovs.to_vec(),
            sprite: Some("scope_pic".into()),
            sprite_scale: 0.5,
            sound_in: Some("in.wav".into()),
            sound_out: Some("out.wav".into()),
            ..ModInfo::default()
        }
    }

    fn deg(r: Option<f32>) -> f32 {
        r.map_or(f32::NAN, f32::to_degrees)
    }

    #[test]
    fn the_mod_row_reads_its_scope_keys_and_keeps_the_constructor_defaults() {
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Mod1]\nKinds = 1\nZoomLevel = 2\nZoomFov0 = 40.0\nZoomFov1 = 20\nZoomFov3 = 9\n",
            "ZoomSpriteName = \"a\"\n[Mod2]\nKinds = 0\n",
            "SilencerLevel = 3\n",
        ));
        let m1 = ModInfo::of_section(doc.get("Mod1").unwrap());
        assert_eq!(
            m1.zoom_fov,
            vec![40.0, 20.0],
            "the list stops at the first missing key"
        );
        assert_eq!(m1.attach_socket(), Some(1));
        assert_eq!(m1.kind, mod_default::TYPE);
        assert_eq!(m1.sprite_scale, mod_default::SPRITE_SCALE);
        assert!((deg(m1.fov_of(2)) - 20.0).abs() < 1e-4);
        assert_eq!(m1.fov_of(3), None);
        assert_eq!(m1.fov_of(0), None);
        let m2 = ModInfo::of_section(doc.get("Mod2").unwrap());
        assert_eq!(m2.silencer_level(), Some(3));
        assert_eq!(m2.attach_socket(), Some(0));
        assert_eq!(
            ModInfo::default().attach_socket(),
            None,
            "Kinds -1 is filed nowhere"
        );
    }

    /// The fire-cycle and recoil keys of a mod row (cshell+0x176f10): signed clip entries (the ROM writes a comment
    /// after the Fast Loader numbers), the two clip rates and the recoil terms; a missing key reads 0.
    #[test]
    fn the_mod_row_reads_its_clip_rate_and_recoil_keys() {
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Mod45]\nKinds = 2\nType = 5\nReloadanirate = 0.2\nShotsPerClipPlus0 = -6//30 (20%)\n",
            "ShotsPerClipPlus1 = 10\n[Mod0]\nInstdamage = -0.1\nMaxRandomPitch = -2\nBaseCamRecoilPitch = -0.1\n",
            "Fireanirate = 0.5\nMaxCamRecoilPitchPer = 0.3\nBaseCamRecoilPitchPer = 0.05\n",
            "BaseCamRecoilAccelPitch = 0.4\n",
        ));
        let m = ModInfo::of_section(doc.get("Mod45").unwrap());
        assert_eq!(m.clip_plus[..3], [-6, 10, 0]);
        assert_eq!((m.reload_clip_rate, m.fire_clip_rate), (0.2, 0.0));
        assert_eq!(m.recoil, ModRecoil::default());
        let s = ModInfo::of_section(doc.get("Mod0").unwrap());
        assert_eq!(s.fire_clip_rate, 0.5);
        assert_eq!(
            s.recoil,
            ModRecoil {
                max_per: 0.3,
                base_per: 0.05,
                base: -0.1,
                accel: 0.4,
                random_pitch: -2.0,
            }
        );
    }

    #[test]
    fn a_press_cycles_the_level_and_the_field_moves_with_the_clock_only() {
        let m = sight(2, &[40.0, 20.0]);
        let normal = 90f32.to_radians();
        let mut z = Zoom::default();
        assert_eq!(
            z.step_level(1000, None, Some(&m), normal, 0.5),
            Press::In(1)
        );
        assert!(z.active() && z.changing());
        assert_eq!(z.take_sounds()[0].file, "in.wav");
        assert!(z.overlays()[OVERLAY_SCOPE].is_some() && z.overlays()[OVERLAY_CHANGE_IN].is_some());
        // 90 -> 40 at (90 - 40) / 0.5 = 100 degrees per second: half way after 250 ms, whatever the frames
        assert!((deg(z.fov_x(1250)) - 65.0).abs() < 1e-3);
        // a press during the change does nothing
        assert_eq!(z.step_level(1100, None, Some(&m), normal, 0.5), Press::None);
        assert!(!z.frame(1499));
        assert!(z.changing());
        z.frame(1500);
        assert!(!z.changing() && z.level() == 1);
        assert!(z.overlays()[OVERLAY_CHANGE_IN].is_none() && z.overlays()[OVERLAY_SCOPE].is_some());
        assert!((deg(z.fov_x(9999)) - 40.0).abs() < 1e-4);
        // level 2: 40 -> 20 at (90 - 20) / 0.5 = 140 degrees per second
        assert_eq!(
            z.step_level(2000, None, Some(&m), normal, 0.5),
            Press::In(2)
        );
        assert!((deg(z.fov_x(2100)) - 26.0).abs() < 1e-3);
        z.frame(3000);
        // past ZoomLevel: back to 0, at (90 - 20) / 0.5 = 140 degrees per second from 20
        assert_eq!(z.step_level(4000, None, Some(&m), normal, 0.5), Press::Out);
        assert_eq!(z.level(), 0);
        assert!(z.active(), "the change out still counts as zoomed");
        assert_eq!(z.take_sounds().last().unwrap().file, "out.wav");
        assert!((deg(z.fov_x(4250)) - 55.0).abs() < 1e-3);
        z.frame(4490);
        assert!(z.active());
        // (the 20 degrees of the rate are the 2009 constant 0.34906587 rad: the end may fall a fraction of a ms late)
        z.frame(4502);
        assert!(!z.active());
        assert_eq!(z.fov_x(4502), None);
        assert!(z.overlays().iter().all(Option::is_none));
    }

    #[test]
    fn the_frame_rate_does_not_change_the_field_or_the_end_of_a_change() {
        let m = sight(1, &[30.0]);
        let normal = 100f32.to_radians();
        let run = |step: u32| {
            let mut z = Zoom::default();
            z.step_level(0, None, Some(&m), normal, 0.3);
            let mut end = None;
            let mut t = 0;
            while t <= 1000 {
                z.frame(t);
                if end.is_none() && !z.changing() {
                    end = Some(t);
                }
                t += step;
            }
            (
                z.fov_x(1000).map(f32::to_degrees),
                z.fov_x(150).map(f32::to_degrees),
                end,
            )
        };
        let a = run(1);
        let b = run(7);
        let c = run(33);
        assert_eq!(a.0, b.0);
        assert_eq!(a.0, c.0);
        assert_eq!(a.1, b.1);
        assert_eq!(a.1, c.1);
        assert_eq!(a.2, Some(300));
        assert!(b.2.unwrap() >= 300 && b.2.unwrap() < 307);
    }

    #[test]
    fn no_sight_a_toggle_sight_and_the_instant_reset() {
        let normal = 90f32.to_radians();
        let mut z = Zoom::default();
        assert_eq!(z.step_level(0, None, None, normal, 0.5), Press::None);
        let no_zoom = sight(0, &[]);
        assert_eq!(
            z.step_level(0, None, Some(&no_zoom), normal, 0.5),
            Press::None
        );
        let toggle = ModInfo {
            kind: TYPE_TOGGLE,
            ..sight(1, &[40.0])
        };
        assert_eq!(
            z.step_level(0, None, Some(&toggle), normal, 0.5),
            Press::Toggled(true)
        );
        assert!(!z.active());
        let m = sight(1, &[40.0]);
        z.step_level(0, None, Some(&m), normal, 0.5);
        z.take_sounds();
        z.reset(Some(&m));
        assert!(!z.active() && z.fov_x(10).is_none());
        assert!(z.overlays().iter().all(Option::is_none));
        assert_eq!(
            z.take_sounds().len(),
            1,
            "the reset of a raised scope plays the zoom-out sound"
        );
        z.reset(Some(&m));
        assert!(z.take_sounds().is_empty());
    }

    #[test]
    fn a_reload_waits_for_the_zoom_out_and_the_server_overlay_obeys_its_slot() {
        let m = sight(1, &[40.0]);
        let normal = 90f32.to_radians();
        let mut z = Zoom::default();
        z.step_level(0, None, Some(&m), normal, 0.5);
        z.frame(1000);
        z.defer_reload();
        assert_eq!(
            z.step_level(1000, Some(0), Some(&m), normal, 0.5),
            Press::Out
        );
        assert!(!z.frame(1100));
        assert!(z.frame(2000), "the reload runs at the end of the zoom-out");
        assert!(!z.reload_after());
        z.server_overlay(2, true, Some(&m));
        assert!(z.overlays()[2].is_some());
        z.server_overlay(9, true, Some(&m));
        z.server_overlay(2, false, Some(&m));
        assert!(z.overlays().iter().all(Option::is_none));
    }
}
