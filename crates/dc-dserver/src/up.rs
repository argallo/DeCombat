//! Client -> server game messages (shell ids inside `C2S_SHELL_MSG`, engine id 11) that a TDM round needs, parsed
//! from the bits after `shell_id`. The player update 0x64 has its own module ([`crate::shell64`]). Names are those of
//! `GAME_MESSAGES_2009_C2S.md` (`UP_` prefix). Bits past the end of a message read as 0, as on the 2009 side;
//! parsers that need a minimum length say so with `complete`.

use crate::gs::Look;
use dc_wire::udp::{read_quat128, BitReader, BitWriter};

/// `UP_PLAYER_INFO`: the answer to the server's `GS_IDENTIFY_REQUEST`.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.1 (cshell+0x23ccb0)
pub const UP_PLAYER_INFO: u8 = 0x72;
/// `UP_CLIENT_OPTIONS`: `[opt_a:u8][opt_b:bool1]`, sent right after `UP_PLAYER_INFO`.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.2 (cshell+0x1ed040)
pub const UP_CLIENT_OPTIONS: u8 = 0x85;
/// `UP_WORLD_READY`: no fields; sent once when the loading screen closes.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.4 (cshell+0x1a9dc0)
pub const UP_WORLD_READY: u8 = 0x87;
/// `UP_FIRE`: one message per pellet with the client's trace result.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (cshell+0x238b10)
pub const UP_FIRE: u8 = 0x82;
/// `UP_RELOAD`: `[ammo_id:u16]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.2 (region cshell+0x2399e0)
pub const UP_RELOAD: u8 = 0x83;
/// `UP_WEAPON_CHANGE`: `[slot:u8][weapon_id:u16][aux:u16]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.4 (cshell+0x1f8fa0)
pub const UP_WEAPON_CHANGE: u8 = 0x7f;
/// `UP_RESPAWN_REQUEST` (mode classes 1, 4, 5, 6, 9), `_B` (7, 8), `_C` (10, `[zero:u8]`).
// SPEC: GAME_MESSAGES_2009_C2S.md §3.4 (cshell+0x61e40)
pub const UP_RESPAWN_REQUEST: u8 = 0x89;
pub const UP_RESPAWN_REQUEST_B: u8 = 0xc5;
pub const UP_RESPAWN_REQUEST_C: u8 = 0xd2;
/// `UP_CHAT`: `[text:zstr16][tag:zstr16]` and, for chat modes 0 / 1, `[scope:u8]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §6.1 (cshell+0x72df0)
pub const UP_CHAT: u8 = 0x68;
/// `UP_LEAVE_MATCH`: no fields.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.7 (cshell+0x48450)
pub const UP_LEAVE_MATCH: u8 = 0xbd;
/// `UP_ROSTER_QUERY`: `[pid:u32]`, 0xFFFFFFFF = all.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.5 (cshell+0x29e80)
pub const UP_ROSTER_QUERY: u8 = 0x8c;
/// `UP_PERF_REPORT`: `[sub:u8][own_id:u32]` + sub fields.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.8 (cshell+0x2ae70)
pub const UP_PERF_REPORT: u8 = 0xc3;
/// `UP_MOVE_EVENT` / `UP_WEAPON_EVENT` / `UP_DEFLECT`: `[category:u8]` + fields.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.2, §4.5
pub const UP_EVENT: u8 = 0x76;
/// `UP_SPRINT_STATE`: `[sprinting:bool1][stopped:bool1]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.3 (cshell+0x1ff7f0)
pub const UP_SPRINT_STATE: u8 = 0xca;
/// `UP_OBSERVE_TARGET` `[target_id:u8]` and `UP_OBSERVE_FREE` (no fields).
// SPEC: GAME_MESSAGES_2009_C2S.md §3.5
pub const UP_OBSERVE_TARGET: u8 = 0x60;
pub const UP_OBSERVE_FREE: u8 = 0x61;
/// `UP_WEAPON_SOUND` / `UP_WEAPON_LOOP`: animation-key sounds (relay format UNPROVEN, U5).
// SPEC: GAME_MESSAGES_2009_C2S.md §4.3
pub const UP_WEAPON_SOUND: u8 = 0x80;
pub const UP_WEAPON_LOOP: u8 = 0x81;
/// `UP_THROW_HOLD` (formerly `UP_THROW_RELEASE`): no fields; sent repeatedly while a throwable is held, the release
/// is the next `UP_FIRE`.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.6, Appendix F.2 K7
pub const UP_THROW_HOLD: u8 = 0x4d;

/// `UP_USE`: `[target:objref16][phase:u8]` (0 pressed, 1 released). The 2009 client sends it for a use target of
/// its own team (a mode object); a DeCombat client also sends it for a dropped weapon under the player (DSV-170).
// SPEC: GAME_MESSAGES_2009_C2S.md §7.1 (cshell+0x61e40)
// DC-DEFAULT: DSV-170 (proposed) a use on a type-0x17 drop object asks to take it
pub const UP_USE: u8 = 0x59;
/// `UP_USE_HOLD`: `[phase:u8][target:objref16]`, phase 1 starts a hold-to-use on the use target (a plant or defuse
/// point), phase 4 ends it.
// SPEC: GAME_MESSAGES_2009_C2S.md §7.1 (0xd8, cshell+0x1f7890)
pub const UP_USE_HOLD: u8 = 0xd8;
/// `UP_USE_PROGRESS_END`: `[target:objref16][remaining:f32]`, the client's use progress stopped with this much time
/// left (0 = ran out).
// SPEC: GAME_MESSAGES_2009_C2S.md §7.2 (0xad, cshell+0x275ad0)
pub const UP_USE_PROGRESS_END: u8 = 0xad;
/// `UP_SCAN`: `[sub:u8]` then sub 1 `[team:u16][on:u8]` (the heartbeat sensor switched on / off), sub 2
/// `[team:u16][name:zstr8]` (another scanner's charge spent).
// SPEC: GAME_MESSAGES_2009_C2S.md §7.3 (0xcd, cshell+0xb7a20, cshell+0xb5c40)
pub const UP_SCAN: u8 = 0xcd;
/// The switch sub-message of `UP_SCAN`.
pub const SCAN_SWITCH: u8 = 1;

/// Developer, console and local-world messages that are dropped (and logged once).
// SPEC: GAME_MESSAGES_2009_C2S.md §5.3, §8; DC-DEFAULT: DC-C2S-1 (proposed)
pub const DEV_IDS: [u8; 10] = [0x33, 0x69, 0x6e, 0x75, 0x8b, 0xa7, 0xa8, 0xa9, 0xae, 0xb0];

/// The parsed `UP_PLAYER_INFO` (§5.1). Field meanings beyond the head, team, GP, experience and clan are UNPROVEN.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpPlayerInfo {
    pub room_slot: u32,
    pub char_no: u32,
    pub session_idx: u16,
    pub exp: u32,
    pub look: Look,
    pub r1_4: [i8; 4],
    pub r5: u32,
    pub team: i8,
    pub r6: i8,
    pub r7: u32,
    pub gp: u32,
    /// `(item_no, e1, e2, e3)` loadout entries.
    pub loadout: Vec<(u16, u8, u8, u8)>,
    pub clan: Option<UpClan>,
    pub word_d: u32,
    pub nation: u32,
    pub flag_e: bool,
    pub flag_f: bool,
    /// False when the message ended before the last field.
    pub complete: bool,
}

/// The clan block of `UP_PLAYER_INFO`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpClan {
    pub band_name: Vec<u16>,
    pub clan_id: u64,
    pub mark_a: u32,
    pub mark_b: u32,
    pub word_c: u32,
}

/// Reads a look block (15 signed bytes, two `zstr8`).
// SPEC: GAME_MESSAGES_2009_C2S.md §5.1 (`look`, cshell+0x1c2bc0)
pub fn read_look(r: &mut BitReader<'_>) -> Look {
    let mut l = Look::default();
    for b in &mut l.bytes {
        *b = r.read_s8();
    }
    l.text_a = r.read_zstr8();
    l.text_b = r.read_zstr8();
    l
}

/// Parses `UP_PLAYER_INFO`; `n` loadout entries are read as written (§5.1 parsing note).
// SPEC: GAME_MESSAGES_2009_C2S.md §5.1 (DISASM cshell+0x23cd2a..0x23d1ea)
pub fn parse_player_info(body: &BitWriter) -> UpPlayerInfo {
    let mut r = body.reader();
    let mut p = UpPlayerInfo {
        room_slot: r.read_u32(),
        char_no: r.read_u32(),
        session_idx: r.read_u16(),
        exp: r.read_u32(),
        look: read_look(&mut r),
        ..UpPlayerInfo::default()
    };
    for x in &mut p.r1_4 {
        *x = r.read_s8();
    }
    p.r5 = r.read_u32();
    p.team = r.read_s8();
    p.r6 = r.read_s8();
    p.r7 = r.read_u32();
    p.gp = r.read_u32();
    let n = r.read_u8();
    for _ in 0..n {
        if r.remaining_bits() < 40 {
            return p;
        }
        p.loadout
            .push((r.read_u16(), r.read_u8(), r.read_u8(), r.read_u8()));
    }
    if r.read_bit() {
        p.clan = Some(UpClan {
            band_name: r.read_zstr16(),
            clan_id: r.read_u64(),
            mark_a: r.read_u32(),
            mark_b: r.read_u32(),
            word_c: r.read_u32(),
        });
    }
    if r.remaining_bits() < 64 + 2 {
        return p;
    }
    p.word_d = r.read_u32();
    p.nation = r.read_u32();
    p.flag_e = r.read_bit();
    p.flag_f = r.read_bit();
    p.complete = true;
    p
}

/// The parsed `UP_FIRE` (§4.1). `has_hit` with `hit_dist` and `hit_obj` is the client's own trace result.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpFire {
    pub wkind: u8,
    pub weapon_id: u16,
    pub ammo_id: u16,
    pub weapon_pos: [f32; 3],
    pub origin: [f32; 3],
    pub aim: [f32; 4],
    pub yaw: f32,
    pub dir: [f32; 3],
    pub seed: u8,
    pub moving: bool,
    pub wflag: bool,
    pub mode1: bool,
    pub frac: u8,
    pub shot_no: u32,
    pub time_ms: u32,
    pub extra: Option<FireExtra>,
    pub flag_c: bool,
    pub last: bool,
    /// `(hit_dist, hit_obj)` when the pellet hit an object.
    pub hit: Option<(f32, Option<u16>)>,
    pub stat: u32,
    pub complete: bool,
}

/// The optional `has_extra` block of `UP_FIRE` (meanings UNPROVEN).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FireExtra {
    pub obj: Option<u16>,
    pub kind: u8,
    pub a: [f32; 3],
    pub b: [f32; 3],
}

impl UpFire {
    /// The hit point the server reconstructs: `origin + hit_dist * dir`.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1 ("the server reconstructs it as origin + hit_dist * dir")
    pub fn shot_end(&self) -> Option<[f32; 3]> {
        let (d, _) = self.hit?;
        Some([
            self.origin[0] + d * self.dir[0],
            self.origin[1] + d * self.dir[1],
            self.origin[2] + d * self.dir[2],
        ])
    }
}

fn vec3(r: &mut BitReader<'_>) -> [f32; 3] {
    [r.read_f32(), r.read_f32(), r.read_f32()]
}

/// Bits of `UP_FIRE` up to `has_extra` (fixed part).
const FIRE_FIXED_BITS: usize = 8 + 16 + 16 + 96 + 96 + 128 + 32 + 96 + 8 + 3 + 8 + 32 + 32 + 1;

/// Parses `UP_FIRE`.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (DISASM cshell+0x238b7c..0x238e41, order and widths PROVEN)
pub fn parse_fire(body: &BitWriter) -> UpFire {
    let mut r = body.reader();
    let short = r.remaining_bits() < FIRE_FIXED_BITS;
    let mut f = UpFire {
        wkind: r.read_u8(),
        weapon_id: r.read_u16(),
        ammo_id: r.read_u16(),
        weapon_pos: vec3(&mut r),
        origin: vec3(&mut r),
        aim: read_quat128(&mut r),
        yaw: r.read_f32(),
        dir: vec3(&mut r),
        seed: r.read_u8(),
        moving: r.read_bit(),
        wflag: r.read_bit(),
        mode1: r.read_bit(),
        frac: r.read_u8(),
        shot_no: r.read_u32(),
        time_ms: r.read_u32(),
        ..UpFire::default()
    };
    if short {
        return f;
    }
    if r.read_bit() {
        f.extra = Some(FireExtra {
            obj: r.read_objref16(),
            kind: r.read_u8(),
            a: vec3(&mut r),
            b: vec3(&mut r),
        });
    }
    f.flag_c = r.read_bit();
    f.last = r.read_bit();
    if r.read_bit() {
        let d = r.read_f32();
        f.hit = Some((d, r.read_objref16()));
    }
    f.complete = r.remaining_bits() >= 32;
    f.stat = r.read_u32();
    f
}

/// The parsed `UP_PERF_REPORT`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UpPerf {
    /// sub 2: `[fps:u16][ping:u16][s2:u16][s3:u16]`.
    Stats {
        own_id: u32,
        values: [u16; 4],
    },
    /// sub 3: `[t:f32][3 x u32]`.
    Echo {
        own_id: u32,
        t: f32,
        words: [u32; 3],
    },
    /// sub 1 (console only; dropped, DC-C2S-1).
    Console {
        own_id: u32,
    },
    Other {
        sub: u8,
    },
}

/// One parsed client game message.
#[derive(Debug, Clone, PartialEq)]
pub enum Up {
    PlayerInfo(Box<UpPlayerInfo>),
    ClientOptions {
        a: u8,
        b: bool,
    },
    WorldReady,
    Fire(Box<UpFire>),
    Reload {
        ammo_id: u16,
    },
    WeaponChange {
        slot: u8,
        weapon_id: u16,
        aux: u16,
    },
    RespawnRequest {
        id: u8,
    },
    Chat {
        text: Vec<u16>,
        tag: Vec<u16>,
        scope: Option<u8>,
    },
    LeaveMatch,
    RosterQuery {
        pid: u32,
    },
    Perf(UpPerf),
    /// `UP_MOVE_EVENT` (category 1), `UP_WEAPON_EVENT` (3), `UP_DEFLECT` (5).
    Event {
        category: u8,
        event: u8,
    },
    Sprint {
        sprinting: bool,
        stopped: bool,
    },
    ObserveTarget {
        target: u8,
    },
    ObserveFree,
    WeaponSound {
        sound: u8,
        weapon_id: u16,
        /// 0x81 (loop sound) instead of 0x80.
        looped: bool,
        /// 0x80 only: the sender's own id and the weapon position.
        own_id: Option<u8>,
        pos: Option<[f32; 3]>,
    },
    /// `UP_THROW_HOLD` 0x4d (no fields, repeated while held): the next `UP_FIRE` is the throw.
    ThrowHold,
    /// `UP_USE` 0x59: the use key on object `target` (phase 0 pressed, 1 released).
    Use {
        target: u16,
        phase: u8,
    },
    /// `UP_USE_HOLD` 0xd8: phase 1 starts a hold on `target`, phase 4 ends it.
    UseHold {
        phase: u8,
        target: u16,
    },
    /// `UP_USE_PROGRESS_END` 0xad: the hold on `target` stopped with `remaining` seconds left.
    UseProgressEnd {
        target: u16,
        remaining: f32,
    },
    /// `UP_SCAN` 0xcd sub 1: the sender's heartbeat sensor switched `on` (team = the sender's own team).
    Scan {
        team: u16,
        on: bool,
    },
    /// Our team ping request 0xf4 (DSV-651).
    TeamPing(dc_wire::teamping::Request),
    /// A developer / console message (DC-C2S-1).
    Developer,
    /// Any other id.
    Unknown,
}

/// Parses one client game message (not 0x64).
// SPEC: GAME_MESSAGES_2009_C2S.md §1.3 (id inventory), sections 2-7
pub fn parse_up(shell_id: u8, body: &BitWriter) -> Up {
    let mut r = body.reader();
    match shell_id {
        UP_PLAYER_INFO => Up::PlayerInfo(Box::new(parse_player_info(body))),
        UP_CLIENT_OPTIONS => Up::ClientOptions {
            a: r.read_u8(),
            b: r.read_bit(),
        },
        UP_WORLD_READY => Up::WorldReady,
        UP_FIRE => Up::Fire(Box::new(parse_fire(body))),
        UP_RELOAD => Up::Reload {
            ammo_id: r.read_u16(),
        },
        UP_WEAPON_CHANGE => Up::WeaponChange {
            slot: r.read_u8(),
            weapon_id: r.read_u16(),
            aux: r.read_u16(),
        },
        UP_RESPAWN_REQUEST | UP_RESPAWN_REQUEST_B | UP_RESPAWN_REQUEST_C => {
            Up::RespawnRequest { id: shell_id }
        }
        UP_CHAT => {
            let text = r.read_zstr16();
            let tag = r.read_zstr16();
            // SPEC: GAME_MESSAGES_2009_C2S.md §6.1 (no scope byte for other chat modes)
            let scope = (r.remaining_bits() >= 8).then(|| r.read_u8());
            Up::Chat { text, tag, scope }
        }
        UP_LEAVE_MATCH => Up::LeaveMatch,
        UP_ROSTER_QUERY => Up::RosterQuery { pid: r.read_u32() },
        UP_PERF_REPORT => {
            let sub = r.read_u8();
            let own_id = r.read_u32();
            Up::Perf(match sub {
                2 => UpPerf::Stats {
                    own_id,
                    values: [r.read_u16(), r.read_u16(), r.read_u16(), r.read_u16()],
                },
                3 => UpPerf::Echo {
                    own_id,
                    t: r.read_f32(),
                    words: [r.read_u32(), r.read_u32(), r.read_u32()],
                },
                1 => UpPerf::Console { own_id },
                s => UpPerf::Other { sub: s },
            })
        }
        UP_EVENT => Up::Event {
            category: r.read_u8(),
            event: r.read_u8(),
        },
        UP_SPRINT_STATE => Up::Sprint {
            sprinting: r.read_bit(),
            stopped: r.read_bit(),
        },
        UP_OBSERVE_TARGET => Up::ObserveTarget {
            target: r.read_u8(),
        },
        UP_OBSERVE_FREE => Up::ObserveFree,
        // SPEC: GAME_MESSAGES_2009_C2S.md §4.3 (0x80 `[sound_no:u8][weapon_id:u16][own_id:u8][weapon_pos:vec96]`,
        // 0x81 `[sound_no:u8][weapon_id:u16]`)
        UP_WEAPON_SOUND | UP_WEAPON_LOOP => {
            let sound = r.read_u8();
            let weapon_id = r.read_u16();
            let looped = shell_id == UP_WEAPON_LOOP;
            let (own_id, pos) = if !looped && r.remaining_bits() >= 8 + 96 {
                (Some(r.read_u8()), Some(vec3(&mut r)))
            } else {
                (None, None)
            };
            Up::WeaponSound {
                sound,
                weapon_id,
                looped,
                own_id,
                pos,
            }
        }
        // SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (0x4d, no fields)
        UP_THROW_HOLD => Up::ThrowHold,
        // SPEC: GAME_MESSAGES_2009_C2S.md §7.1 (`[target:objref16][phase:u8]`)
        UP_USE => Up::Use {
            target: r.read_u16(),
            phase: r.read_u8(),
        },
        // SPEC: GAME_MESSAGES_2009_C2S.md §7.1 (`[phase:u8][target:objref16]`)
        UP_USE_HOLD => Up::UseHold {
            phase: r.read_u8(),
            target: r.read_u16(),
        },
        // SPEC: GAME_MESSAGES_2009_C2S.md §7.2 (`[target:objref16][remaining:f32]`)
        UP_USE_PROGRESS_END => Up::UseProgressEnd {
            target: r.read_u16(),
            remaining: r.read_f32(),
        },
        // SPEC: GAME_MESSAGES_2009_C2S.md §7.3 (sub 1 `[team:u16][on:u8]`; sub 2 has no server rule here)
        UP_SCAN => match r.read_u8() {
            SCAN_SWITCH => Up::Scan {
                team: r.read_u16(),
                on: r.read_u8() != 0,
            },
            _ => Up::Unknown,
        },
        // DC-DEFAULT: DSV-651 (proposed) our team ping request
        dc_wire::teamping::UP_ID => {
            dc_wire::teamping::read_request(&mut r).map_or(Up::Unknown, Up::TeamPing)
        }
        id if DEV_IDS.contains(&id) => Up::Developer,
        _ => Up::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gs::write_look;

    fn info_bits(n: u8, clan: bool) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u32(4).write_u32(77).write_u16(9).write_u32(1200);
        let look = Look {
            bytes: [1; 15],
            text_a: b"a".to_vec(),
            text_b: b"bb".to_vec(),
        };
        write_look(&mut w, &look);
        for _ in 0..4 {
            w.write_s8(-1);
        }
        w.write_u32(5)
            .write_s8(2)
            .write_s8(0)
            .write_u32(6)
            .write_u32(5000);
        w.write_u8(n);
        for i in 0..n {
            w.write_u16(100 + i as u16)
                .write_u8(1)
                .write_u8(2)
                .write_u8(3);
        }
        w.write_bit(clan);
        if clan {
            w.write_zstr16_presumed(&[0x41, 0x42]);
            w.write_u64(55).write_u32(1).write_u32(2).write_u32(3);
        }
        w.write_u32(8)
            .write_u32(44)
            .write_bit(true)
            .write_bit(false);
        w
    }

    #[test]
    fn scan_switch_parses_and_other_subs_are_unknown() {
        let mut w = BitWriter::new();
        w.write_u8(1).write_u16(2).write_u8(1);
        assert_eq!(parse_up(UP_SCAN, &w), Up::Scan { team: 2, on: true });
        let mut off = BitWriter::new();
        off.write_u8(1).write_u16(1).write_u8(0);
        assert_eq!(parse_up(UP_SCAN, &off), Up::Scan { team: 1, on: false });
        let mut sub2 = BitWriter::new();
        sub2.write_u8(2).write_u16(1).write_u8(0);
        assert_eq!(parse_up(UP_SCAN, &sub2), Up::Unknown);
    }

    #[test]
    fn player_info_with_and_without_clan() {
        let p = parse_player_info(&info_bits(2, false));
        assert!(p.complete);
        assert_eq!(
            (p.room_slot, p.char_no, p.session_idx, p.exp),
            (4, 77, 9, 1200)
        );
        assert_eq!(p.look.text_b, b"bb");
        assert_eq!(p.team, 2);
        assert_eq!(p.gp, 5000);
        assert_eq!(p.loadout, vec![(100, 1, 2, 3), (101, 1, 2, 3)]);
        assert_eq!((p.nation, p.flag_e, p.flag_f), (44, true, false));
        let q = parse_player_info(&info_bits(0, true));
        assert!(q.complete);
        assert_eq!(q.clan.as_ref().map(|c| c.clan_id), Some(55));
        let mut short = info_bits(3, false).to_bytes();
        short.truncate(20);
        assert!(!parse_player_info(&BitWriter::from_bytes(&short)).complete);
    }

    fn pellet_bits(hit: Option<(f32, u16)>) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(0).write_u16(12).write_u16(34);
        for v in [1.0, 2.0, 3.0, 10.0, 20.0, 30.0] {
            w.write_f32(v);
        }
        for v in [0.0, 0.0, 0.0, 1.0] {
            w.write_f32(v);
        }
        w.write_f32(0.5);
        for v in [0.0, 0.0, 1.0] {
            w.write_f32(v);
        }
        w.write_u8(7)
            .write_bit(false)
            .write_bit(false)
            .write_bit(true);
        w.write_u8(128).write_u32(3).write_u32(4500);
        w.write_bit(false); // has_extra
        w.write_bit(false).write_bit(true); // flag_c, last
        w.write_bit(hit.is_some());
        if let Some((d, o)) = hit {
            w.write_f32(d).write_u16(o);
        }
        w.write_u32(0);
        w
    }

    #[test]
    fn fire_hit_and_miss() {
        let f = parse_fire(&pellet_bits(Some((100.0, 5))));
        assert!(f.complete);
        assert_eq!((f.weapon_id, f.ammo_id), (12, 34));
        assert_eq!(f.origin, [10.0, 20.0, 30.0]);
        assert!(f.mode1 && f.last);
        assert_eq!(f.hit, Some((100.0, Some(5))));
        assert_eq!(f.shot_end(), Some([10.0, 20.0, 130.0]));
        let m = parse_fire(&pellet_bits(None));
        assert!(m.complete && m.hit.is_none() && m.shot_end().is_none());
        assert!(!parse_fire(&BitWriter::from_bytes(&[0, 1, 2])).complete);
    }

    #[test]
    fn small_messages() {
        let chat = {
            let mut w = BitWriter::new();
            w.write_zstr16_presumed(&[0x68, 0x69])
                .write_zstr16_presumed(&[]);
            w.write_u8(1);
            w
        };
        assert_eq!(
            parse_up(UP_CHAT, &chat),
            Up::Chat {
                text: vec![0x68, 0x69],
                tag: vec![],
                scope: Some(1)
            }
        );
        let mut nos = BitWriter::new();
        nos.write_zstr16_presumed(&[0x41])
            .write_zstr16_presumed(&[]);
        assert!(matches!(
            parse_up(UP_CHAT, &nos),
            Up::Chat { scope: None, .. }
        ));
        assert_eq!(parse_up(UP_WORLD_READY, &BitWriter::new()), Up::WorldReady);
        assert_eq!(
            parse_up(UP_WEAPON_CHANGE, &BitWriter::from_bytes(&[2, 5, 0, 0, 0])),
            Up::WeaponChange {
                slot: 2,
                weapon_id: 5,
                aux: 0
            }
        );
        assert_eq!(parse_up(0xa7, &BitWriter::new()), Up::Developer);
        assert_eq!(parse_up(0xee, &BitWriter::new()), Up::Unknown);
        assert_eq!(
            parse_up(UP_USE, &BitWriter::from_bytes(&[0x2a, 0x01, 0])),
            Up::Use {
                target: 0x012a,
                phase: 0
            }
        );
        assert_eq!(
            parse_up(UP_USE_HOLD, &BitWriter::from_bytes(&[1, 0x2a, 0x01])),
            Up::UseHold {
                phase: 1,
                target: 0x012a
            }
        );
        let mut w = BitWriter::new();
        w.write_u16(7).write_f32(0.0);
        assert_eq!(
            parse_up(UP_USE_PROGRESS_END, &w),
            Up::UseProgressEnd {
                target: 7,
                remaining: 0.0
            }
        );
        let mut perf = BitWriter::new();
        perf.write_u8(3)
            .write_u32(1)
            .write_f32(2.5)
            .write_u32(0)
            .write_u32(0)
            .write_u32(0);
        assert_eq!(
            parse_up(UP_PERF_REPORT, &perf),
            Up::Perf(UpPerf::Echo {
                own_id: 1,
                t: 2.5,
                words: [0; 3]
            })
        );
    }
}
