//! The client sound cues of the parity audit's W2-8 group (w2-audiocues): the own footsteps of the view-bob cycle with
//! their gates (m38: step source, spawn mute, state gate), the ladder's movement-sound gate (m37), the team callout of
//! a throw relay (m39), the bullet fly-by of another player's shot (m42) and the silenced fire sound of another
//! player's shot (M17, the record's mod bytes). Pure rules and the ROM tables they read; [`super::SoundDirector`]
//! plays what they decide. File and key texts of the ROM are read at run time, never written to logs.
// SPEC: AUDIO_2009.md §3.2 (request flags), §9 (the sound attribute table), §14.2 (footsteps)

use std::collections::BTreeMap;
use std::f32::consts::{FRAC_PI_2, TAU};

use dc_botclient::objects::ClientObject;
use dc_bute::{ButeDoc, SectionMap};
use dc_rez::RezVfs;
use dc_wire::udp::BitWriter;

fn text(v: Option<&String>) -> String {
    v.map(|s| s.trim().trim_matches('"').trim().to_string())
        .unwrap_or_default()
}

fn number(v: Option<&String>) -> Option<f32> {
    let t = v?.trim().trim_matches('"').trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

fn index_of(section: &str, prefix: &str) -> Option<u16> {
    let rest = section.strip_prefix(prefix)?;
    (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| rest.parse().ok())
        .flatten()
}

// --------------------------------------------------------------------------------------------- m38: own steps

/// Pace of the view-bob cycle (cycles per second) while the control word carries the run bit 0x200, and without it.
// DISASM: cshell+0x2aff10..0x2aff44 (two console variables registered with 1.4 (0x3fb33333, handle VA 0x375b4784) and
// 0.6 (0x3f19999a, handle VA 0x375b4790)); EVIDENCE: cshell+0x2b0050 (the first when the control word of the player
// manager +0x2c has 0x200, else the second; phase += pace x 2 pi x frame time)
pub const BOB_PACE_RUN: f32 = 1.4;
pub const BOB_PACE_WALK: f32 = 0.6;
/// After the player stops, the bob amplitude decays to 0 in this time; then the phase restarts at 0.
// DISASM: cshell+0x2aff49..0x2aff61 (console variable registered with 0.1 (0x3dcccccd), handle VA 0x375b479c);
// EVIDENCE: cshell+0x2b0050 (amplitude = max x (1 - ease(t / that time)) after the stop, 0 at the end; a zero
// amplitude zeroes both phases)
pub const BOB_DECAY_MS: u32 = 100;
/// The least speed (units per second) that counts as moving for the step cycle.
// DISASM: cshell+0x2b0242..0x2b0255 (|velocity| x frame time compared with 0.1, constant at VA 0x374ce3dc)
// DC-DEFAULT: CLI-526 (proposed) 6 units per second: the 2009 test is 0.1 unit per frame, a frame-rate dependent
// threshold; 6 u/s is that test at 60 frames per second, read from the clock at any frame rate
pub const STEP_MIN_SPEED: f32 = 6.0;

/// The view-bob step cycle of the local player: a phase that runs at [`BOB_PACE_RUN`] or [`BOB_PACE_WALK`] cycles per
/// second while the player moves, and a step each time it passes a quarter (the right foot) and three quarters (the
/// left foot), alternately.
// EVIDENCE: cshell+0x2b0050 (phase +0xc wraps at 2 pi; past pi/2 with the latch at VA 0x3755838c set: step, foot 0,
// latch moves to VA 0x375b4780; past 3 pi/2 with that latch set: step, foot 1, back; DISASM cshell+0x2b0192..0x2b023e;
// the step needs |v| x dt > 0.1 and the control bit 0x800 (cshell+0x2b0257..0x2b025c), then calls the step player
// cshell+0x202870 on the local character FX (mover +0x304) with the foot as its fifth argument, DISASM
// cshell+0x2b0743..0x2b0765); initial latch values from the image: VA 0x3755838c = 1, VA 0x375b4780 = 0
#[derive(Debug, Clone, PartialEq)]
pub struct BobSteps {
    phase: f32,
    armed_right: bool,
    armed_left: bool,
    active: bool,
    stopped_at: Option<u32>,
    last_ms: Option<u32>,
}

impl Default for BobSteps {
    fn default() -> Self {
        BobSteps {
            phase: 0.0,
            armed_right: true,
            armed_left: false,
            active: false,
            stopped_at: None,
            last_ms: None,
        }
    }
}

impl BobSteps {
    /// One frame at clock time `now_ms`: the steps it passed, `true` = the left foot. The time step comes from the
    /// clock, and a long frame is cut into quarter-cycle pieces so it passes every crossing (the 2009 client tests
    /// the phase once per frame and drops a second crossing).
    // DC-DEFAULT: CLI-526 (proposed) every crossing of a long frame counts (frame-rate independence)
    pub fn frame(&mut self, now_ms: u32, control: u32, speed: f32) -> Vec<bool> {
        use dc_sim::player::bits;
        // a clock that went back is a new match's: the old stop time and phase mean nothing on it
        // DC-DEFAULT: CLI-1541 (proposed) a restarted clock restarts the bob cycle
        if self.last_ms.is_some_and(|l| now_ms < l) {
            *self = BobSteps::default();
        }
        let dt = self
            .last_ms
            .map_or(0, |l| now_ms.saturating_sub(l))
            .min(10_000) as f32
            / 1000.0;
        self.last_ms = Some(now_ms);
        let moving = speed > STEP_MIN_SPEED && control & bits::ANY_MOVE != 0;
        let pace = if control & bits::RUN != 0 {
            BOB_PACE_RUN
        } else {
            BOB_PACE_WALK
        };
        let mut steps = Vec::new();
        if !self.active {
            self.phase = 0.0;
        } else {
            let mut left = pace * TAU * dt;
            while left > 0.0 {
                let d = left.min(FRAC_PI_2);
                left -= d;
                self.phase += d;
                if self.phase > TAU {
                    self.phase -= TAU;
                }
                if let Some(foot) = self.crossing() {
                    if moving {
                        steps.push(foot);
                    }
                }
            }
        }
        if moving {
            self.active = true;
            self.stopped_at = None;
        } else if self.active {
            let at = *self.stopped_at.get_or_insert(now_ms);
            if now_ms.saturating_sub(at) >= BOB_DECAY_MS {
                self.active = false;
                self.stopped_at = None;
            }
        }
        steps
    }

    fn crossing(&mut self) -> Option<bool> {
        let x = self.phase;
        if x <= FRAC_PI_2 || x > 3.0 * FRAC_PI_2 {
            if x > 3.0 * FRAC_PI_2 && self.armed_left {
                self.armed_left = false;
                self.armed_right = true;
                return Some(true);
            }
        } else if self.armed_right {
            self.armed_right = false;
            self.armed_left = true;
            return Some(false);
        }
        None
    }

    /// The current phase (radians, 0..2 pi).
    pub fn phase(&self) -> f32 {
        self.phase
    }
}

/// Whether the step player plays the local player's own step: the run or sprint bit must be in the control word and
/// the player state must not be one of the dead / waiting states.
// EVIDENCE: cshell+0x202870 (own FX: return without 0x400200 in mover +0x24; return when the player manager's +0x30
// is 6, 2, 3 or 5)
pub fn own_step_allowed(control: u32, life_state: Option<u8>) -> bool {
    use dc_sim::player::bits;
    control & (bits::RUN | bits::SPRINT) != 0 && !matches!(life_state, Some(2 | 3 | 5 | 6))
}

// --------------------------------------------------------------------------------------------- m37: ladders

/// The surface type a character on a ladder steps on.
// SPEC: MOVEMENT_2009.md §12.2 (mover +0x50 = 200 on a ladder); EVIDENCE: cshell+0x202870 (compares the surface with
// 200, the float bits 2.8026e-43)
pub const SURFACE_LADDER: u16 = 200;
/// Shell object type of a volume's create blob and the container code of a ladder volume.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 row 0x0b (`OBJ_VOLUME`, the shared block of cshell+0x2163d0)
pub const VOLUME_BLOB_TYPE: u8 = 0x0b;
// OBSERVED: capture 20261025_002236_tdm4 (10 ladder volumes of container code 11; REFERENCE)
pub const VOLUME_CODE_LADDER: u16 = 11;

/// What a ladder volume does to the footsteps on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LadderSound {
    pub obj: u16,
    pub pos: [f32; 3],
    /// The volume's movement-sound switch: off = no footstep on that ladder.
    pub sounds: bool,
    /// The volume's surface override (0 = none: the steps keep surface 200).
    pub surface: u8,
}

/// The movement-sound switch and the surface override of a volume's create blob (type byte first): the bool and the
/// byte after the first byte that follows the three colour vectors.
// DISASM: cshell+0x2163d0 (the block: bool1, u32, u32, three vec96, u8 -> +0x38, bool1 -> +0x3c, u8 -> +0x40, ...);
// EVIDENCE: cshell+0x202870 (the ladder FX of class 0x0b: return when its +0x6c is 0, surface := its +0x70 when not
// 0); PLAUSIBLE: the FX keeps the block at +0x30 (as the type 0x19 record's block, DISASM cshell+0x2177f0..0x217865),
// so +0x6c / +0x70 are the block's +0x3c / +0x40
pub fn ladder_flags(blob: &BitWriter) -> Option<(bool, u8)> {
    let mut r = blob.reader();
    if r.remaining_bits() < 8 + 1 + 64 + 288 + 8 + 1 + 8 || r.read_u8() != VOLUME_BLOB_TYPE {
        return None;
    }
    r.skip_bits(1 + 64 + 288 + 8);
    let sounds = r.read_bit();
    Some((sounds, r.read_u8()))
}

/// The ladder volumes among the session's objects: named volumes of the ladder container code with a volume blob.
pub fn ladder_sounds(objects: &BTreeMap<u16, ClientObject>) -> Vec<LadderSound> {
    objects
        .iter()
        .filter(|(_, o)| {
            o.code == Some(VOLUME_CODE_LADDER) && o.blob_type == Some(VOLUME_BLOB_TYPE)
        })
        .filter_map(|(id, o)| {
            let (sounds, surface) = ladder_flags(o.blob.as_ref()?)?;
            Some(LadderSound {
                obj: *id,
                pos: o.pos.unwrap_or(o.create_pos),
                sounds,
                surface,
            })
        })
        .collect()
}

/// The ladder rule for a footstep on [`SURFACE_LADDER`] at `feet`: `None` = no step, else the surface to play.
// EVIDENCE: cshell+0x202870 (the own FX on a ladder takes the mover's container, mover +0x30 / +0x34; another FX takes
// the first class-0x0b object of an engine query around its position; no such volume: the step plays with surface
// 200)
// DC-DEFAULT: CLI-528 (proposed) the volume is the ladder volume object nearest to the feet, for the own player too
pub fn ladder_surface(ladders: &[LadderSound], feet: [f32; 3]) -> Option<u16> {
    let d2 = |p: [f32; 3]| (0..3).map(|i| (p[i] - feet[i]).powi(2)).sum::<f32>();
    let Some(l) = ladders
        .iter()
        .min_by(|a, b| d2(a.pos).total_cmp(&d2(b.pos)))
    else {
        return Some(SURFACE_LADDER);
    };
    if !l.sounds {
        return None;
    }
    Some(if l.surface != 0 {
        u16::from(l.surface)
    } else {
        SURFACE_LADDER
    })
}

// --------------------------------------------------------------------------------------------- m39: team callouts

/// The auto-cast sound table entry of `ATTRIBUTES.rez`, matched by `sha256(name)[:16]` and length.
// SPEC: AUDIO_2009.md §9 (other sound-related entries: the auto-cast sounds, sha `b1d0ddf6234d2b31`, len 17)
pub const AUTOCAST_ENTRY: dc_audio::KeyId = dc_audio::KeyId::new("b1d0ddf6234d2b31", 17);
// DISASM: cshell+0x22456e..0x2245b8 (`Template<i>` keys of the template section), cshell+0x224a1b (the team 3D rows
// `AutoCastTeams3DSound<n>`), cshell+0x224a94 (their radius key), cshell+0x224b1a..0x224b38 (the file keys
// `<template>_Snd<i>`)
const SECTION_TEMPLATES: &str = "SoundTemplate";
const KEY_TEMPLATE: &str = "Template";
const SECTION_TEAM_ROWS: &str = "AutoCastTeams3DSound";
/// The local rows (no radius): the loader's second row loop, kept at +0x15c0 with their count at +0x15e0.
// EVIDENCE: cshell+0x2244a0 (second loop: rows without `3DsoundRadius`, list +0x15c0, count +0x15e0); OBSERVED: ROM
// table (auto-cast entry) sections `AutoCastLocalsound<n>`, row 1 named for the enemy kill
const SECTION_LOCAL_ROWS: &str = "AutoCastLocalsound";
/// The local row the sprint breath plays (the table names it for the dash breathing).
// EVIDENCE: cshell+0x95850 (row 0 while the sprint gauge drains, see `SoundDirector::breath`)
pub const LOCAL_ROW_BREATH: u8 = 0;
/// The local row the own kill plays.
// EVIDENCE: cshell+0x2c430 (case 0x6d, the play returning to cshell+0x2dd1b: the local row count +0x15e0 must exceed 1,
// then cshell+0x223a10 picks a file of row 1 for the own character's template, cshell+0x223430)
pub const LOCAL_ROW_ENEMY_KILL: u8 = 1;
/// Priority of the own kill voice (a local voice at volume 100, class 0).
// EVIDENCE: cshell+0x2c430 (case 0x6d: cshell+0x1b1e20 with priority 6, then 0, volume 100, pitch 1.0, 0; the same
// arguments as the local row player cshell+0x2c6620)
pub const OWN_KILL_PRIO: u8 = 6;
const KEY_RADIUS: &str = "3DsoundRadius";
const KEY_FILES: &str = "_Snd";

/// One team 3D callout row: its radius and the files of each voice template.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CalloutRow {
    pub radius: f32,
    pub files: Vec<Vec<String>>,
}

/// The team 3D rows of the auto-cast table, and its local rows (radius 0).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TeamCallouts {
    pub rows: Vec<CalloutRow>,
    pub local: Vec<CalloutRow>,
}

impl TeamCallouts {
    /// Rows `<prefix>0`, `<prefix>1`, ... up to the first missing one; per row the radius and, per template of the
    /// template section, the keys `<template>_Snd0`, `_Snd1`, ... up to the first missing one.
    // EVIDENCE: cshell+0x2244a0 (the loader: the template names, then each row section while it exists)
    pub fn from_doc(doc: &ButeDoc) -> TeamCallouts {
        let templates: Vec<String> = doc
            .get(SECTION_TEMPLATES)
            .map(|s| {
                (0..)
                    .map(|i| text(s.get(&format!("{KEY_TEMPLATE}{i}"))))
                    .take_while(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let files_of = |s: &SectionMap, t: &str| -> Vec<String> {
            (0..)
                .map(|i| text(s.get(&format!("{t}{KEY_FILES}{i}"))))
                .take_while(|f| !f.is_empty())
                .collect()
        };
        let rows = (0..)
            .map_while(|n| doc.get(&format!("{SECTION_TEAM_ROWS}{n}")))
            .map(|s| CalloutRow {
                radius: number(s.get(KEY_RADIUS)).unwrap_or(0.0),
                files: templates.iter().map(|t| files_of(s, t)).collect(),
            })
            .collect();
        let local = (0..)
            .map_while(|n| doc.get(&format!("{SECTION_LOCAL_ROWS}{n}")))
            .map(|s| CalloutRow {
                radius: 0.0,
                files: templates.iter().map(|t| files_of(s, t)).collect(),
            })
            .collect();
        TeamCallouts { rows, local }
    }

    /// The file of local row `row` for voice template `template` (picked by `pick` among its files); `None` when the
    /// row or the template has no file.
    // EVIDENCE: cshell+0x223a10 (row < the local row count, template < the template count, a non-zero file count; the
    // pick is uniform over the template's files)
    pub fn pick_local(&self, row: u8, template: u8, pick: usize) -> Option<&str> {
        let files = self
            .local
            .get(usize::from(row))?
            .files
            .get(usize::from(template))?;
        (!files.is_empty()).then(|| files[pick % files.len()].as_str())
    }

    /// Reads the table from the user's ROM.
    pub fn open(vfs: &RezVfs) -> Option<TeamCallouts> {
        let list = vfs.list_entries("ATTRIBUTES.rez").ok()?;
        let (full, _, _) = list.into_iter().find(|(full, _, _)| {
            let base = full.rsplit(['\\', '/']).next().unwrap_or(full);
            AUTOCAST_ENTRY.matches(base)
        })?;
        Some(TeamCallouts::from_doc(&ButeDoc::from_vfs(
            vfs,
            "ATTRIBUTES.rez",
            &full,
        )?))
    }

    /// The file (picked by `pick` among the template's files) and radius of row `row` for voice template `template`;
    /// `None` when the row is out of range, its radius is not positive or it names no file.
    // EVIDENCE: cshell+0x2cbd30 (row < the row count, radius > 0, a non-empty name; the pick is uniform:
    // cshell+0x223bb0 with cshell+0x19eb40(0, count - 1))
    pub fn pick(&self, row: u8, template: u8, pick: usize) -> Option<(&str, f32)> {
        let r = self.rows.get(usize::from(row))?;
        let files = r.files.get(usize::from(template))?;
        if r.radius <= 0.0 || files.is_empty() {
            return None;
        }
        Some((files[pick % files.len()].as_str(), r.radius))
    }
}

/// Whether this client plays the throw relay's callout: the record's team byte must be the own team.
// EVIDENCE: cshell+0x2cbd30 (k 0x12: plays only when the record's +0x48 equals the local player's slot +0xb0, which
// returns its +0x1e4, DISASM cshell+0x1f2490); SPEC: GAME_MESSAGES_2009_S2C.md §4.2 0x45 notes (the own team is
// `local_player` +0x1e4 via slot +0xb0); DISASM: cshell+0x217845..0x217865 (the record's trailing `b` -> +0x48, `c` ->
// +0x49, the leading byte -> +0x46)
pub fn callout_heard(team_byte: u8, own_team: Option<u8>) -> bool {
    own_team == Some(team_byte)
}

/// Whether a 0x6d plays the own kill voice (auto-cast local row [`LOCAL_ROW_ENEMY_KILL`]): the killer is the own
/// player and the hit zone is not the groin zone 5.
// EVIDENCE: cshell+0x2c430 (case 0x6d: zone 5 sets the flag that skips the voice; the killer byte must equal the own
// id, the id the victim test uses; the play's handle is kept at roster +0x14)
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 effect 5
pub fn own_kill_voice(killer: u8, hit_zone: u8, own_pid: Option<u8>) -> bool {
    own_pid == Some(killer) && hit_zone != 5
}

/// Priority of the team callout.
// EVIDENCE: cshell+0x2cbd30 (cshell+0x27de90 with priority 2, flags 0, volume 100, class 0, inner -1)
pub const CALLOUT_PRIO: u8 = 2;

// --------------------------------------------------------------------------------------------- m42: fly-by

/// The fly-by radius: another player's bullet passing the listener closer than this plays the fly-by sound.
// DISASM: cshell+0x2c5871..0x2c5891 (console variable registered with 600.0 (0x44160000), handle VA 0x375b4ba8);
// EVIDENCE: cshell+0x2c73a0 (squared distance from the listener to the shot line below its square)
pub const FLYBY_RADIUS: f32 = 600.0;
/// No fly-by when the listener is within this share of the impact row's sound radius from the shot's end.
// DISASM: cshell+0x2c73fa..0x2c743a (`fild [row + 0xa4]` x 0.75 (VA 0x374d3b30) compared with the distance to the
// shot's end +0x6c); DISASM: cshell+0x292b4b..0x292b60 (impact row +0xa4 = its `SoundRadius`)
pub const FLYBY_IMPACT_SHARE: f32 = 0.75;
/// The ammunition's damage type that takes the fly-by test, and its `Type` that the test needs.
// DISASM: cshell+0x2c8efb..0x2c8f16 (`cmp [ammo row], 2`, then a foreign shot: +0xdd != +0xdc), cshell+0x174dea..
// 0x174dfd (ammo row +0x00 = `InstDamageType`), cshell+0x2c73c2..0x2c73c7 (ammo row +0x0c must be 0), cshell+0x174d55..
// 0x174d7b (+0x0c = `Type`)
pub const FLYBY_DAMAGE_TYPE: u32 = 2;
pub const FLYBY_AMMO_TYPE: u32 = 0;
/// The sound attribute record the fly-by plays, matched by `sha256(name)[:16]` and length.
// DISASM: cshell+0x2c754f (the record name passed to cshell+0x27dd70, VA 0x374f9ec4)
pub const FLYBY_RECORD: dc_audio::KeyId = dc_audio::KeyId::new("1e2b59501505369a", 11);

/// Where the fly-by of a shot from `from` to `to` plays for a listener at `l`, or `None`: the point of the shot's line
/// nearest to the listener when that is within [`FLYBY_RADIUS`], unless the listener is within
/// [`FLYBY_IMPACT_SHARE`] x `impact_radius` of the shot's end. The line is not cut at its ends.
// EVIDENCE: cshell+0x2c73a0 (d = dot(L - start, dir); offset = start + d x dir - L; |offset|^2 < radius^2); the
// shot's start +0x60 and unit direction +0xa4 (DISASM cshell+0x2c85d1..0x2c8642: dir = normalise(end - start))
// DC-DEFAULT: CLI-529 (proposed) the voice plays at that nearest point; the 2009 shell normalises the listener vector
// in place before adding it (DISASM cshell+0x2c7496..0x2c74f2: the normalise call cshell+0xb5ab0 gets the listener's
// address), which places the voice at offset + L / |L|, near the world origin ([`flyby_shell_point`])
pub fn flyby_point(
    l: [f32; 3],
    from: [f32; 3],
    to: [f32; 3],
    impact_radius: Option<f32>,
) -> Option<[f32; 3]> {
    let off = flyby_offset(l, from, to, impact_radius)?;
    Some([l[0] + off[0], l[1] + off[1], l[2] + off[2]])
}

/// The 2009 shell's own placement of the fly-by: the offset plus the unit listener vector (see [`flyby_point`]).
pub fn flyby_shell_point(
    l: [f32; 3],
    from: [f32; 3],
    to: [f32; 3],
    impact_radius: Option<f32>,
) -> Option<[f32; 3]> {
    let off = flyby_offset(l, from, to, impact_radius)?;
    let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
    let u = if n > 0.0 {
        [l[0] / n, l[1] / n, l[2] / n]
    } else {
        [0.0; 3]
    };
    Some([off[0] + u[0], off[1] + u[1], off[2] + u[2]])
}

fn flyby_offset(
    l: [f32; 3],
    from: [f32; 3],
    to: [f32; 3],
    impact_radius: Option<f32>,
) -> Option<[f32; 3]> {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    if let Some(r) = impact_radius {
        let e = sub(to, l);
        if dot(e, e).sqrt() < FLYBY_IMPACT_SHARE * r {
            return None;
        }
    }
    let span = sub(to, from);
    let len = dot(span, span).sqrt();
    if len <= 0.0 || !len.is_finite() {
        return None;
    }
    let dir = [span[0] / len, span[1] / len, span[2] / len];
    let d = dot(sub(l, from), dir);
    let off = [
        from[0] + dir[0] * d - l[0],
        from[1] + dir[1] * d - l[1],
        from[2] + dir[2] * d - l[2],
    ];
    (dot(off, off) < FLYBY_RADIUS * FLYBY_RADIUS).then_some(off)
}

// --------------------------------------------------------------------------------------------- M17: silenced fire

/// The fields of a `[Mod<n>]` row the fire sound reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModSound {
    /// `Type`.
    pub kind: i32,
    /// `SilencerLevel`.
    pub silencer: i32,
}

/// `[Mod<n>]` rows: `Type` (row +0x08) and `SilencerLevel` (row +0x1a8).
// DISASM: cshell+0x176f3c..0x176f58 (`Type` -> +0x08), cshell+0x1773aa..0x1773c4 (`SilencerLevel` -> +0x1a8)
pub fn mod_sounds(doc: &ButeDoc) -> BTreeMap<u16, ModSound> {
    let mut out = BTreeMap::new();
    for (name, sec) in doc.iter() {
        if let Some(n) = index_of(name, "Mod") {
            let int = |k: &str| number(sec.get(k)).map_or(0, |x| x as i32);
            out.insert(
                n,
                ModSound {
                    kind: int("Type"),
                    silencer: int("SilencerLevel"),
                },
            );
        }
    }
    out
}

/// Silenced fire sounds of every `[Weapon<n>]` row: levels 1..=10 (index `level - 1`), file and radius (the fire
/// radius where the row names none).
// DISASM: cshell+0x17c963..0x17ca31 (`SilencedFireSnd<i>` -> row +0x64 + 4 (i - 1), `SilencedFireRadius<i>` -> row
// +0x1d0 + 4 (i - 1), i = 1..10)
// DC-DEFAULT: CLI-525 (proposed) the fire radius when a row names no silenced radius (the reader's default is not
// traced; every row of the user's ROM names both)
pub fn silenced_sounds(doc: &ButeDoc) -> BTreeMap<u16, Vec<(String, f32)>> {
    let mut out = BTreeMap::new();
    for (name, sec) in doc.iter() {
        let Some(n) = index_of(name, "Weapon") else {
            continue;
        };
        let fire = number(sec.get("FireSndRadius")).unwrap_or(super::WEAPON_OUTER_DEFAULT);
        let rows: Vec<(String, f32)> = (1..=10)
            .map(|i| {
                (
                    text(sec.get(&format!("SilencedFireSnd{i}"))),
                    number(sec.get(&format!("SilencedFireRadius{i}"))).unwrap_or(fire),
                )
            })
            .collect();
        if rows.iter().any(|r| !r.0.is_empty()) {
            out.insert(n, rows);
        }
    }
    out
}

/// What another player's shot plays for the mod in its weapon's first socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotSound {
    /// Weapon sound 6, the fire sound.
    Fire,
    /// Weapon sound 9: the row's silenced sound of this level.
    Silenced(u8),
    /// Nothing: a silencer-type mod whose level is outside 1..=9.
    Silent,
}

/// The fire sound of another player's shot: a mod row of `Type` 0 or 4 picks weapon sound 9 with its silencer level,
/// which plays only for levels 1..=9; any other mod, or none, the fire sound.
// EVIDENCE: cshell+0x2c6580 (mod row of +0xfc: +0x08 == 0 or 4 -> sound 9 with +0x1a8, else 6), cshell+0x1a2b50 (case
// 9: return when the level is < 1 or > 9; the file and radius of index level - 1; no fire-voice cut), cshell+0x1724e0
// (the row by number; 0xff and other numbers beyond the table: none)
pub fn remote_shot_sound(m: Option<&ModSound>) -> ShotSound {
    match m {
        Some(m) if m.kind == 0 || m.kind == 4 => {
            if (1..=9).contains(&m.silencer) {
                ShotSound::Silenced(m.silencer as u8)
            } else {
                ShotSound::Silent
            }
        }
        _ => ShotSound::Fire,
    }
}

/// The silenced file and radius of `weapon` at `level` (1..=10).
pub fn silenced_file(
    table: &BTreeMap<u16, Vec<(String, f32)>>,
    weapon: u16,
    level: u8,
) -> Option<(String, f32)> {
    let (f, r) = table
        .get(&weapon)?
        .get(usize::from(level).checked_sub(1)?)?;
    (!f.is_empty()).then(|| (f.clone(), *r))
}

/// A uniform pick in `0..n` from a 64-bit state (xorshift, the director's generator).
pub fn pick_n(state: &mut u64, n: usize) -> usize {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    if n == 0 {
        0
    } else {
        ((x >> 33) as usize) % n
    }
}

/// A uniform value in `[0, 1)` from the same generator.
pub fn unit(state: &mut u64) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    ((x >> 40) as f32) / (1u64 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_sim::player::bits;

    const RUNNING: u32 = bits::FWD | bits::RUN | bits::ANY_MOVE;

    // CLI-1541: the bob cycle lives across matches in the sound director while every match's clock starts at 0
    #[test]
    fn a_restarted_clock_restarts_the_bob_cycle_at_once() {
        let mut b = BobSteps::default();
        // match 1: run for a minute and a bit, then stand still at 60 s (the stop time is stamped on that clock)
        let mut t = 0;
        while t <= 60_100 {
            b.frame(t, RUNNING, 300.0);
            t += 16;
        }
        b.frame(t, 0, 0.0);
        assert!(b.stopped_at.is_some() && b.active);
        // match 2 from clock 0: standing for the decay time ends the cycle (not after 60 s)
        let mut t = 0;
        while t <= BOB_DECAY_MS + 32 {
            b.frame(t, 0, 0.0);
            t += 16;
        }
        assert!(!b.active, "the old stop time kept the cycle alive");
        // and the first step of a run comes at the quarter cycle, 179 ms in, as on a fresh cycle
        let mut b2 = BobSteps::default();
        let first = |b: &mut BobSteps| {
            (0..40u32)
                .map(|i| i * 16)
                .find(|&t| !b.frame(t, RUNNING, 300.0).is_empty())
        };
        assert_eq!(first(&mut b), first(&mut b2));
    }

    #[test]
    fn the_bob_cycle_steps_right_then_left_at_the_run_pace_at_any_frame_rate() {
        // 1.4 cycles per second, two steps a cycle: 357 ms apart, the first at a quarter cycle (179 ms)
        for frame_ms in [7u32, 16, 33, 100] {
            let mut b = BobSteps::default();
            let mut steps = Vec::new();
            let mut t = 0;
            while t <= 3000 {
                for left in b.frame(t, RUNNING, 300.0) {
                    steps.push((t, left));
                }
                t += frame_ms;
            }
            assert!(
                (8..=9).contains(&steps.len()),
                "{frame_ms} ms frames: {steps:?}"
            );
            assert!(!steps[0].1, "the first step is the right foot");
            assert!(steps.windows(2).all(|w| w[0].1 != w[1].1), "feet alternate");
            let first = steps[0].0 as f32;
            assert!(
                (first - 179.0).abs() <= frame_ms as f32 + 1.0,
                "{frame_ms}: first step {first}"
            );
            let gap = (steps[steps.len() - 1].0 - steps[0].0) as f32 / (steps.len() - 1) as f32;
            assert!((gap - 357.0).abs() < 30.0, "{frame_ms}: gap {gap}");
        }
    }

    #[test]
    fn the_walk_pace_and_the_stop_follow_the_control_word_and_the_speed() {
        let mut b = BobSteps::default();
        let walking = bits::FWD | bits::ANY_MOVE;
        let n: usize = (0..=300u32)
            .map(|i| b.frame(i * 10, walking, 100.0).len())
            .sum();
        // 0.6 cycles per second for 3 s: 1.8 cycles, 3 to 4 crossings
        assert!((3..=4).contains(&n), "walk steps {n}");
        // standing (no move bit, or too slow) passes no step; the phase restarts after the decay
        let mut b = BobSteps::default();
        assert!(b.frame(0, RUNNING, 300.0).is_empty());
        assert!(b.frame(100, RUNNING, 300.0).is_empty());
        assert!(b.frame(150, RUNNING, 2.0).is_empty(), "below the speed");
        assert!(b.frame(400, 0, 0.0).is_empty());
        assert!(b.frame(420, 0, 0.0).is_empty());
        assert_eq!(b.phase(), 0.0, "the decay ended: the phase restarts");
    }

    #[test]
    fn own_steps_need_run_or_sprint_and_a_live_state() {
        assert!(own_step_allowed(bits::RUN, Some(1)));
        assert!(own_step_allowed(bits::SPRINT, None));
        assert!(!own_step_allowed(bits::FWD | bits::ANY_MOVE, Some(1)));
        for s in [2, 3, 5, 6] {
            assert!(!own_step_allowed(bits::RUN, Some(s)), "state {s}");
        }
        assert!(own_step_allowed(bits::RUN, Some(4)));
    }

    fn volume_blob(sounds: bool, surface: u8) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(VOLUME_BLOB_TYPE)
            .write_bit(false)
            .write_f32(0.0)
            .write_f32(0.0);
        for _ in 0..9 {
            w.write_f32(0.5);
        }
        w.write_u8(0).write_bit(sounds).write_u8(surface);
        for _ in 0..3 {
            w.write_f32(0.0);
        }
        w.write_f32(-200.0)
            .write_f32(0.4)
            .write_f32(0.0)
            .write_u8(0);
        w
    }

    #[test]
    fn the_ladder_volume_switches_its_steps_off_or_changes_their_surface() {
        assert_eq!(ladder_flags(&volume_blob(true, 0)), Some((true, 0)));
        assert_eq!(ladder_flags(&volume_blob(false, 0)), Some((false, 0)));
        assert_eq!(ladder_flags(&volume_blob(true, 71)), Some((true, 71)));
        let mut objects = BTreeMap::new();
        for (id, sounds, surface, x, code) in [
            (10u16, true, 0u8, 0.0f32, VOLUME_CODE_LADDER),
            (11, false, 0, 500.0, VOLUME_CODE_LADDER),
            (12, true, 71, 1000.0, VOLUME_CODE_LADDER),
            (13, false, 0, 1600.0, 7),
        ] {
            let mut o = ClientObject::default();
            o.kind = 9;
            o.code = Some(code);
            o.blob_type = Some(VOLUME_BLOB_TYPE);
            o.blob = Some(volume_blob(sounds, surface));
            o.pos = Some([x, 0.0, 0.0]);
            objects.insert(id, o);
        }
        let l = ladder_sounds(&objects);
        assert_eq!(l.len(), 3, "the kill volume is no ladder");
        assert_eq!(ladder_surface(&l, [10.0, 50.0, 0.0]), Some(SURFACE_LADDER));
        assert_eq!(ladder_surface(&l, [520.0, 50.0, 0.0]), None);
        assert_eq!(ladder_surface(&l, [990.0, 50.0, 0.0]), Some(71));
        assert_eq!(ladder_surface(&[], [0.0; 3]), Some(SURFACE_LADDER));
    }

    #[test]
    fn the_callout_rows_read_the_templates_and_the_team_byte_gates_them() {
        let doc = ButeDoc::parse(
            r#"[SoundTemplate]
Template0 = "A"
Template1 = "B"
[AutoCastTeams3DSound0]
3DsoundRadius = 300.0
A_Snd0 = "a0.wav"
[AutoCastTeams3DSound1]
3DsoundRadius = 500.0
A_Snd0 = "a1.wav"
A_Snd1 = "a2.wav"
B_Snd0 = "b1.wav"
[AutoCastTeams3DSound3]
3DsoundRadius = 1.0
A_Snd0 = "gap.wav"
"#,
        );
        let t = TeamCallouts::from_doc(&doc);
        assert_eq!(t.rows.len(), 2, "rows stop at the first missing one");
        assert_eq!(t.pick(1, 0, 1), Some(("a2.wav", 500.0)));
        assert_eq!(t.pick(1, 1, 7), Some(("b1.wav", 500.0)));
        assert_eq!(t.pick(0, 1, 0), None, "no file for that template");
        assert_eq!(t.pick(2, 0, 0), None);
        assert!(callout_heard(1, Some(1)));
        assert!(!callout_heard(1, Some(2)));
        assert!(!callout_heard(1, None));
    }

    #[test]
    fn the_local_rows_give_the_own_kill_voice_and_the_kill_rule_needs_the_own_killer() {
        let doc = ButeDoc::parse(
            r#"[SoundTemplate]
Template0 = "A"
Template1 = "B"
[AutoCastLocalsound0]
A_Snd0 = "l0.wav"
[AutoCastLocalsound1]
A_Snd0 = "k0.wav"
A_Snd1 = "k1.wav"
"#,
        );
        let t = TeamCallouts::from_doc(&doc);
        assert_eq!(t.local.len(), 2);
        assert!(t.rows.is_empty());
        assert_eq!(t.pick_local(LOCAL_ROW_ENEMY_KILL, 0, 3), Some("k1.wav"));
        assert_eq!(
            t.pick_local(LOCAL_ROW_ENEMY_KILL, 1, 0),
            None,
            "template B has no file"
        );
        assert_eq!(t.pick_local(2, 0, 0), None);
        assert!(own_kill_voice(3, 2, Some(3)));
        assert!(own_kill_voice(3, 1, Some(3)), "a head shot too");
        assert!(!own_kill_voice(3, 5, Some(3)), "the groin zone skips it");
        assert!(!own_kill_voice(4, 2, Some(3)));
        assert!(!own_kill_voice(3, 2, None));
    }

    #[test]
    fn a_bullet_passing_the_listener_plays_the_flyby_at_the_nearest_point() {
        let l = [0.0, 0.0, 0.0];
        // a shot along x 300 units beside the listener, ending far away
        let p = flyby_point(l, [-2000.0, 0.0, 300.0], [2000.0, 0.0, 300.0], Some(500.0)).unwrap();
        assert!((p[0]).abs() < 1e-3 && (p[2] - 300.0).abs() < 1e-3, "{p:?}");
        // beyond the radius
        assert_eq!(
            flyby_point(l, [-2000.0, 0.0, 700.0], [2000.0, 0.0, 700.0], None),
            None
        );
        // the shot ends next to the listener: the impact is heard instead
        assert_eq!(
            flyby_point(l, [-2000.0, 0.0, 100.0], [200.0, 0.0, 100.0], Some(500.0)),
            None
        );
        assert!(flyby_point(l, [-2000.0, 0.0, 100.0], [400.0, 0.0, 100.0], Some(500.0)).is_some());
        // the 2009 placement: offset + the unit listener vector
        let l = [1000.0, 0.0, 0.0];
        let s = flyby_shell_point(l, [1000.0, 0.0, -2000.0], [1000.0, 0.0, 2000.0], None).unwrap();
        assert!((s[0] - 1.0).abs() < 1e-3 && s[2].abs() < 1e-3, "{s:?}");
    }

    #[test]
    fn a_silencer_mod_picks_sound_nine_with_its_level() {
        let silencer = ModSound {
            kind: 0,
            silencer: 1,
        };
        assert_eq!(remote_shot_sound(Some(&silencer)), ShotSound::Silenced(1));
        assert_eq!(
            remote_shot_sound(Some(&ModSound {
                kind: 4,
                silencer: 9
            })),
            ShotSound::Silenced(9)
        );
        assert_eq!(
            remote_shot_sound(Some(&ModSound {
                kind: 0,
                silencer: 0
            })),
            ShotSound::Silent
        );
        assert_eq!(
            remote_shot_sound(Some(&ModSound {
                kind: 1,
                silencer: 1
            })),
            ShotSound::Fire,
            "a sight"
        );
        assert_eq!(remote_shot_sound(None), ShotSound::Fire);
        let doc = ButeDoc::parse(
            r#"[Weapon5]
FireSndRadius = 3000
SilencedFireSnd1 = "s1.wav"
SilencedFireRadius1 = 1200
SilencedFireSnd2 = "s2.wav"
[Mod7]
Type = 0
SilencerLevel = 1
"#,
        );
        let t = silenced_sounds(&doc);
        assert_eq!(silenced_file(&t, 5, 1), Some(("s1.wav".into(), 1200.0)));
        assert_eq!(silenced_file(&t, 5, 2), Some(("s2.wav".into(), 3000.0)));
        assert_eq!(silenced_file(&t, 5, 3), None);
        assert_eq!(mod_sounds(&doc).get(&7), Some(&silencer));
    }
}
