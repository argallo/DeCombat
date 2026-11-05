//! Readers of the server messages the HUD takes whole (dc-botclient hands them over as `GameEvent::Shell`): the
//! match begin with its intro block, round result, round close, result screen, match settings, the HiSec case
//! events, the co-op clock and the notes addressed to client objects (case countdown, spawn notes).
//!
//! Every reader follows the bit layout of the 2009 client's handler; a message that ends early gives `None` (the
//! client would read zero bits past the end; the HUD rather ignores it).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 / §2.1.1 / §2.1.2 (0x3f, 0x45, 0x46, 0x48), §3.2 (0x7d), §3.3 (0xc8),
// §3.5 (0xd6, 0xd7), §8.3 (0xa6); HISEC_2009.md §3.1, §3.2; §2.4 (0x68 chat, read by `super::chat`), §3.2 (0x6c)

use dc_wire::udp::BitReader;

/// One `vec96` (three f32).
pub type Vec3 = [f32; 3];

/// The intro block of `GS_MATCH_BEGIN` (0x45), fields in wire order.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 step 5 (fields a..j)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IntroBlock {
    /// Field a, only for mode classes 5, 6, 9, 11.
    pub caption: Option<String>,
    /// Field b: texture path of the banner's title image.
    pub head: String,
    /// Field c: texture path of the banner's text image (empty: no banner).
    pub body: String,
    /// Field d: overview map picture.
    pub map_a: String,
    pub map_min: Vec3,
    pub map_max: Vec3,
    /// Field f: radar picture, second map picture.
    pub radar: String,
    pub map_b: String,
    /// The radar map window (the 2009 radar element reads these as one set): scale pair (x and z used).
    pub radar_map_scale: Vec3,
    /// Offset pair of the window (x and z used).
    pub radar_map_offset: Vec3,
    /// Never read by the 2009 client.
    pub radar_zoom: f32,
    /// Half size of the shown window (picture uv).
    pub radar_half: f32,
    /// Picture width the scale is divided by.
    pub radar_w: u32,
    /// Picture height the scale is divided by.
    pub radar_h: u32,
    /// Field i (not for mode class 6): the two team bases.
    pub bases: Option<[Vec3; 2]>,
    /// Field j (mode class 5 only).
    pub coop: Option<(u8, u8, bool)>,
}

/// `GS_MATCH_BEGIN` (0x45).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 items 1-8
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MatchBegin {
    pub round_secs: u32,
    pub show_intro: bool,
    pub waiting_banner: bool,
    pub countdown_secs: u32,
    pub intro: Option<IntroBlock>,
    /// `show_intro` was set but the reader had no mode class: the block (and `round_no` after it) is unread.
    pub intro_skipped: bool,
    pub round_no: u32,
}

/// `GS_HISEC_CASE` (0xd6).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.5 row 0xd6; HISEC_2009.md §3.2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseEvent {
    /// Sub 0: a case dropped (chat line).
    Dropped,
    /// Sub 1: `player` picked up the case `key`.
    Taken { key: u32, player: u32 },
    /// Sub 2: the case `key` is reserved for `owner` (0xFFFFFFFF = nobody).
    Owner { key: u32, owner: u32 },
}

/// No owner in [`CaseEvent::Owner`].
pub const HISEC_NOBODY: u32 = 0xFFFF_FFFF;

/// A note addressed to a client object (shell 0xa6) that the HUD acts on.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.3 (`[note_kind:u8][obj:objref16]` + class data); HISEC_2009.md §3.1 (type
// 0x17 sub 2 `[remaining:u32]`); §8.3 type 0x18 subs 0x1f / 0x25
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectNote {
    /// Type 0x17 sub 2: the countdown of a pickup object (a negative value reads as 0).
    Countdown { obj: u16, remaining: i32 },
    /// Type 0x18 sub 0x1f (`on`) or 0x25 (off): the character's spawn notes.
    SpawnMark { obj: u16, on: bool },
    /// Type 0x18 sub 0x1d (the death note, `on`) or 0x1e (off): the character's step mute (object +0x539); the
    /// spawn note 0x1f clears it as well.
    // EVIDENCE: cshell+0x206260 (sub 0x1d sets +0x539, subs 0x1e and 0x1f clear it; sub 0x1f sets +0x53a, sub 0x25
    // clears +0x53a), cshell+0x202870 (the step player returns while +0x539 is set)
    StepMute { obj: u16, on: bool },
    /// Type 0x18 sub 0x14 (`armour` false: the health) or 0x15 (the armour): the value a spectator's HUD shows for
    /// that character.
    // DISASM: cshell+0x26f60 (sub 0x14 `read(16)` -> object +0x5ae, sub 0x15 `read(16)` -> +0x5b0), cshell+0x28970 /
    // cshell+0x28980 (the object's slots +0x74 / +0x78 return them)
    Vitals { obj: u16, armour: bool, value: u16 },
}

/// The HUD's reading of a whole shell message.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellRead {
    MatchBegin(Box<MatchBegin>),
    /// 0x46 head: `round_secs`, `win_team`, round wins.
    RoundResult {
        round_secs: u32,
        win_team: i8,
        wins: [u8; 2],
    },
    /// 0x48 `round_secs`.
    RoundClose(u32),
    /// 0x47.
    ObjectsEnd,
    /// 0x3f.
    ResultScreen {
        flag: bool,
        value: u32,
    },
    /// 0x7d: the goal of the room (kills for TDM).
    Goal(u8),
    /// 0xc8: `kills` (the one-kill banner's count), `head` (the head-shot form / decoration), `combo` (> 1: the combo
    /// banner's count).
    // DISASM: cshell+0x2d8ca..0x2d918 (the first field goes to the one-kill form as its count, not a weapon)
    KillRun {
        kills: u16,
        head: bool,
        combo: u32,
    },
    Hisec(CaseEvent),
    /// 0xc6 `GS_OBJECTIVE_STATE` (flag modes).
    Flag(super::modetop::FlagEvent),
    /// 0x51 / 0x50 / 0x4f / 0x56 / 0xad (Search and Destroy).
    Bomb(super::bomb::BombEvent),
    /// 0xd3 `GS_INFECTION_MODE` subs 0 / 1 / 4 (Quarantine).
    Qrt(super::qrt::QrtEvent),
    /// 0xcb `GS_BOUNTY_MODE` subs 0..6 (Spy Hunt).
    Spy(super::spy::SpyEvent),
    /// 0xc9 `GS_COOP_MISSION` subs 1 / 2 / 3 / 5 (Fire Team: the goal list, the banner and the caption).
    Mission(super::mission::MissionEvent),
    /// 0xd7 sub 4: the round clock set to `secs`, running or frozen.
    CoopClock {
        running: bool,
        secs: u32,
    },
    Note(ObjectNote),
    /// 0x68 `GS_CHAT`.
    Chat(super::chat::GsChat),
    /// 0x6c `GS_PLAYER_LEFT` `[pid:u8][kicked:u8]` (the roster side is the session's).
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 row 0x6c (cshell+0x2d4d5..0x2d733, two read(8))
    PlayerLeft {
        pid: u8,
        kicked: bool,
    },
    /// 0xa2 `GS_PING_TABLE`: `(pid, ping ms)` pairs (the roster record's +0x00).
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 row 0xa2; EVIDENCE: cshell+0x2d73a..0x2d798 (u16 pid, u16 ping until
    // pid 0xFFFF; a known pid's record +0x00 := ping as float)
    PingTable(Vec<(u16, u16)>),
    /// DSV-480 (our shell 0xf1, not a 2009 id): other players' health (CLI-1631).
    RemoteHealth(Vec<dc_wire::remotehealth::HealthEntry>),
}

/// The end marker of the ping table's pairs.
pub const PING_TABLE_END: u16 = 0xFFFF;

fn zstr(r: &mut BitReader<'_>) -> String {
    String::from_utf8_lossy(&r.read_zstr8()).into_owned()
}

fn vec3(r: &mut BitReader<'_>) -> Vec3 {
    [r.read_f32(), r.read_f32(), r.read_f32()]
}

/// Reads the body of `GS_MATCH_BEGIN` (after the shell id) for a client of mode class `mode_class`. The block's
/// layout depends on the class (caption, bases, co-op fields); with `None` the block is skipped (`intro_skipped`)
/// rather than read in a guessed form, the head fields stand.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 (DISASM widths cshell+0x49e41 .. 0x4a576); the intro block is read only
// when `show_intro` is set (a normally installed client has both interface images, step 5)
pub fn read_match_begin(r: &mut BitReader<'_>, mode_class: Option<u8>) -> MatchBegin {
    let mut m = MatchBegin {
        round_secs: r.read_u32(),
        show_intro: r.read_bit(),
        waiting_banner: r.read_bit(),
        countdown_secs: r.read_u32(),
        ..MatchBegin::default()
    };
    let Some(mode_class) = mode_class else {
        m.intro_skipped = m.show_intro;
        if !m.show_intro {
            m.round_no = r.read_u32();
        }
        return m;
    };
    if m.show_intro {
        let mut b = IntroBlock::default();
        if matches!(mode_class, 5 | 6 | 9 | 11) {
            b.caption = Some(zstr(r));
        }
        b.head = zstr(r);
        b.body = zstr(r);
        b.map_a = zstr(r);
        b.map_min = vec3(r);
        b.map_max = vec3(r);
        b.radar = zstr(r);
        b.map_b = zstr(r);
        b.radar_map_scale = vec3(r);
        b.radar_map_offset = vec3(r);
        b.radar_zoom = r.read_f32();
        b.radar_half = r.read_f32();
        b.radar_w = r.read_u32();
        b.radar_h = r.read_u32();
        if mode_class != 6 {
            b.bases = Some([vec3(r), vec3(r)]);
        }
        if mode_class == 5 {
            b.coop = Some((r.read_u8(), r.read_u8(), r.read_bit()));
        }
        m.intro = Some(b);
    }
    m.round_no = r.read_u32();
    m
}

/// Reads one whole shell message the HUD takes (`None`: another id, or the body is too short).
pub fn read_shell(
    shell: u8,
    body: &[u8],
    bits: usize,
    mode_class: Option<u8>,
) -> Option<ShellRead> {
    let mut r = BitReader::with_bit_len(body, bits);
    let need = |n: usize, r: &BitReader<'_>| r.remaining_bits() >= n;
    let out = match shell {
        0x45 => {
            if !need(66, &r) {
                return None;
            }
            let m = read_match_begin(&mut r, mode_class);
            ShellRead::MatchBegin(Box::new(m))
        }
        // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.2 (`[round_secs:u32][win_team:i8][wins_team1:u8][wins_team2:u8]`)
        0x46 => {
            if !need(56, &r) {
                return None;
            }
            ShellRead::RoundResult {
                round_secs: r.read_u32(),
                win_team: r.read_s8(),
                wins: [r.read_u8(), r.read_u8()],
            }
        }
        0x48 => {
            if !need(32, &r) {
                return None;
            }
            ShellRead::RoundClose(r.read_u32())
        }
        0x47 => ShellRead::ObjectsEnd,
        super::chat::GS_CHAT => ShellRead::Chat(super::chat::read_gs_chat(body, bits)?),
        0x6c => {
            if !need(16, &r) {
                return None;
            }
            ShellRead::PlayerLeft {
                pid: r.read_u8(),
                kicked: r.read_u8() != 0,
            }
        }
        // the 2009 reader stops at the end marker only; a body that ends without it ends the table here
        0xa2 => {
            let mut v = Vec::new();
            while need(16, &r) {
                let pid = r.read_u16();
                if pid == PING_TABLE_END || !need(16, &r) {
                    break;
                }
                v.push((pid, r.read_u16()));
            }
            ShellRead::PingTable(v)
        }
        // DC-DEFAULT: DSV-480 / CLI-1631 (proposed) our own message; a cut body is dropped
        dc_wire::remotehealth::SHELL_ID => {
            ShellRead::RemoteHealth(dc_wire::remotehealth::read_body(&mut r)?)
        }
        0x3f => {
            if !need(33, &r) {
                return None;
            }
            ShellRead::ResultScreen {
                flag: r.read_bit(),
                value: r.read_u32(),
            }
        }
        // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (0x7d layout; read as dc-botclient's match-settings reader)
        0x7d => {
            let (_team, goal, ..) = dc_botclient::shell::read_match_settings(&mut r);
            ShellRead::Goal(goal)
        }
        // SPEC: GAME_MESSAGES_2009_S2C.md §3.3 row 0xc8 (`[weapon:u16][flag:bool1][count:u32]`)
        0xc8 => {
            if !need(49, &r) {
                return None;
            }
            ShellRead::KillRun {
                kills: r.read_u16(),
                head: r.read_bit(),
                combo: r.read_u32(),
            }
        }
        // SPEC: GAME_MESSAGES_2009_S2C.md §2.2 rows 0x51 (`[bomb_secs:u32]`), 0x50, 0x4f (no body), 0x56
        // (`[item:objref16]`); §4.4 row 0xad (`[obj:objref16][kind:u8][a:u32][hold_secs:f32][c:u32]`)
        0x51 => {
            if !need(32, &r) {
                return None;
            }
            ShellRead::Bomb(super::bomb::BombEvent::Planted(r.read_u32()))
        }
        // SPEC: GAME_MODES_2009.md §5.7 (C-GM-2: sub 0 `[team]` (`[infector:u32]` for team 2) `[pid:u32]` + look;
        // C-GM-3: sub 1 `[a:u8][b:u8][flag:bool1][secs:f32]`; sub 4 no fields)
        0xd3 => {
            if !need(8, &r) {
                return None;
            }
            match r.read_u8() {
                0 => {
                    if !need(8 + 32, &r) {
                        return None;
                    }
                    let team = r.read_u8();
                    let infector = if team == 2 { r.read_u32() } else { u32::MAX };
                    if !need(32, &r) {
                        return None;
                    }
                    ShellRead::Qrt(super::qrt::QrtEvent::Team {
                        team,
                        infector,
                        pid: r.read_u32(),
                    })
                }
                1 => {
                    if !need(8 + 8 + 1 + 32, &r) {
                        return None;
                    }
                    let (a, b) = (r.read_u8(), r.read_u8());
                    r.read_bit();
                    ShellRead::Qrt(super::qrt::QrtEvent::Countdown {
                        a,
                        b,
                        secs: r.read_f32(),
                    })
                }
                // EVIDENCE: cshell+0x2a1e0 (sub 3: one 8-bit read, sign-extended)
                3 => {
                    if !need(8, &r) {
                        return None;
                    }
                    ShellRead::Qrt(super::qrt::QrtEvent::Override(r.read_u8() as i8))
                }
                4 => ShellRead::Qrt(super::qrt::QrtEvent::Note),
                _ => return None,
            }
        }
        // SPEC: GAME_MODES_2009.md §5.6 (sub 0 `[value:u32][team:u8][pid:u32]` + look; sub 1 three u32; sub 5
        // `[phase:u8]` + two zstr8(260) for phase 1; sub 6 two u32; subs 2 / 3 / 4 nothing); EVIDENCE: cshell+0x2bd80
        0xcb => {
            use super::spy::SpyEvent;
            if !need(8, &r) {
                return None;
            }
            ShellRead::Spy(match r.read_u8() {
                0 => {
                    if !need(32 + 8 + 32, &r) {
                        return None;
                    }
                    let value = r.read_u32();
                    let team = r.read_u8();
                    SpyEvent::Team {
                        value,
                        team,
                        pid: r.read_u32(),
                    }
                }
                1 => {
                    if !need(96, &r) {
                        return None;
                    }
                    SpyEvent::Counts {
                        total: r.read_u32(),
                        dropped: r.read_u32(),
                        spies: r.read_u32(),
                    }
                }
                2 => SpyEvent::Loadout,
                3 => SpyEvent::Dropped,
                4 => SpyEvent::Respawned,
                5 => {
                    if !need(8, &r) {
                        return None;
                    }
                    let phase = r.read_u8();
                    let (title, text) = if phase == 1 {
                        let t = String::from_utf8_lossy(&r.read_zstr8()).into_owned();
                        let x = String::from_utf8_lossy(&r.read_zstr8()).into_owned();
                        (t, x)
                    } else {
                        (String::new(), String::new())
                    };
                    SpyEvent::Phase { phase, title, text }
                }
                6 => {
                    if !need(64, &r) {
                        return None;
                    }
                    SpyEvent::Timer {
                        secs: r.read_u32(),
                        colour: r.read_u32(),
                    }
                }
                _ => return None,
            })
        }
        0x50 => ShellRead::Bomb(super::bomb::BombEvent::Defused),
        0x4f => ShellRead::Bomb(super::bomb::BombEvent::Exploded),
        0x56 => {
            if !need(16, &r) {
                return None;
            }
            ShellRead::Bomb(super::bomb::BombEvent::Carrier(r.read_objref16()))
        }
        0xad => {
            if !need(16 + 8 + 32 + 32 + 32, &r) {
                return None;
            }
            let obj = r.read_u16();
            let kind = r.read_u8();
            r.read_u32();
            let secs = r.read_f32();
            ShellRead::Bomb(super::bomb::BombEvent::Track { obj, kind, secs })
        }
        // SPEC: GAME_MESSAGES_2009_S2C.md §6.1 row 0xc6; GAME_MODES_2009.md §5.5 (C-GM-4, DISASM
        // cshell+0x5aae0..0x5ac2f)
        0xc6 => {
            if !need(16 + 32 * 5 + 2, &r) {
                return None;
            }
            ShellRead::Flag(super::modetop::FlagEvent {
                obj: r.read_u16(),
                state: r.read_u32(),
                team: r.read_u32(),
                carrier: r.read_u32(),
                actor: r.read_u32(),
                event: r.read_u32(),
                at_base: r.read_bit(),
                quiet: r.read_bit(),
            })
        }
        0xd6 => {
            if !need(8, &r) {
                return None;
            }
            match r.read_u8() {
                0 => ShellRead::Hisec(CaseEvent::Dropped),
                sub @ (1 | 2) => {
                    if !need(64, &r) {
                        return None;
                    }
                    let (key, who) = (r.read_u32(), r.read_u32());
                    ShellRead::Hisec(if sub == 1 {
                        CaseEvent::Taken { key, player: who }
                    } else {
                        CaseEvent::Owner { key, owner: who }
                    })
                }
                _ => return None,
            }
        }
        // SPEC: GAME_MESSAGES_2009_S2C.md §6.2 row 0xc9 (sub 1 `[n:u32]` n x `[id:u32][label:zstr8(1000)][state:u8]`
        // `[m:u32]` m x `[timer:f32]` `[a:u8][b:u8]`; sub 2 `[text:zstr8(1000)][secs:f32]`; sub 3 `[text:zstr8(1000)]`
        // `[secs:f32]` (DSV-579); sub 5 `[op:u8][id:u32]`, op 1 `[label:zstr8(1000)]`, op 3 `[state:u8]`); the other subs
        // are not read here (their elements are not drawn)
        // EVIDENCE: cshell+0x31490 (the reads in this order; the timer read as 32 bits of a float)
        0xc9 => {
            use super::mission::{wire_text, Goal, MissionEvent};
            if !need(8, &r) {
                return None;
            }
            ShellRead::Mission(match r.read_u8() {
                1 => {
                    if !need(32, &r) {
                        return None;
                    }
                    let n = r.read_u32() as usize;
                    // a goal is at least 48 bits: a count past the message's end is a damaged message
                    if n.saturating_mul(48) > r.remaining_bits() {
                        return None;
                    }
                    let mut goals = Vec::with_capacity(n);
                    for _ in 0..n {
                        let id = r.read_u32();
                        let label = wire_text(&r.read_zstr8());
                        if !need(8, &r) {
                            return None;
                        }
                        goals.push(Goal {
                            id,
                            label,
                            state: r.read_u8(),
                        });
                    }
                    if !need(32, &r) {
                        return None;
                    }
                    let m = r.read_u32() as usize;
                    if m.saturating_mul(32).saturating_add(16) > r.remaining_bits() {
                        return None;
                    }
                    let timers = (0..m).map(|_| r.read_f32()).collect();
                    MissionEvent::List {
                        goals,
                        timers,
                        stage: r.read_u8(),
                        goal: r.read_u8(),
                    }
                }
                2 => {
                    let text = wire_text(&r.read_zstr8());
                    if !need(32, &r) {
                        return None;
                    }
                    MissionEvent::Banner {
                        text,
                        secs: r.read_f32(),
                    }
                }
                // DISASM: cshell+0x31490 case 3 (`read_zstr(1000)`, `read(32)` read as float seconds at
                // cshell+0x81930)
                3 => {
                    let text = wire_text(&r.read_zstr8());
                    if !need(32, &r) {
                        return None;
                    }
                    MissionEvent::Caption {
                        text,
                        secs: r.read_f32(),
                    }
                }
                5 => {
                    if !need(40, &r) {
                        return None;
                    }
                    let (op, id) = (r.read_u8(), r.read_u32());
                    match op {
                        1 => MissionEvent::Added {
                            id,
                            label: wire_text(&r.read_zstr8()),
                        },
                        2 => MissionEvent::Removed { id },
                        3 if need(8, &r) => MissionEvent::State {
                            id,
                            state: r.read_u8(),
                        },
                        _ => return None,
                    }
                }
                _ => return None,
            })
        }
        0xd7 => {
            if !need(8, &r) || r.read_u8() != 4 || !need(33, &r) {
                return None;
            }
            ShellRead::CoopClock {
                running: r.read_bit(),
                secs: r.read_u32(),
            }
        }
        0xa6 => {
            if !need(32, &r) {
                return None;
            }
            let kind = r.read_u8();
            let obj = r.read_u16();
            let sub = r.read_u8();
            match (kind, sub) {
                (0x17, 2) if need(32, &r) => ShellRead::Note(ObjectNote::Countdown {
                    obj,
                    remaining: r.read_u32() as i32,
                }),
                (0x18, 0x1f) => ShellRead::Note(ObjectNote::SpawnMark { obj, on: true }),
                (0x18, 0x25) => ShellRead::Note(ObjectNote::SpawnMark { obj, on: false }),
                (0x18, 0x1d) => ShellRead::Note(ObjectNote::StepMute { obj, on: true }),
                (0x18, 0x1e) => ShellRead::Note(ObjectNote::StepMute { obj, on: false }),
                (0x18, 0x14 | 0x15) if need(16, &r) => ShellRead::Note(ObjectNote::Vitals {
                    obj,
                    armour: sub == 0x15,
                    value: r.read_u16(),
                }),
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(out)
}

/// The HiSec case key of a type 0x17 create blob (type byte first), when its pickup kind is 5.
// SPEC: HISEC_2009.md §3.1 (`[flag_a:bool1][flag_b:bool1][flag_c:bool1][fx_name:zstr8][pickup_kind:u32]`, kind 5:
// `[case_key:u32]`); GAME_MESSAGES_2009_S2C.md §8.2 (blob type 0x17)
pub fn case_key(blob: &[u8], bits: usize) -> Option<u32> {
    let mut r = BitReader::with_bit_len(blob, bits);
    if r.remaining_bits() < 8 || r.read_u8() != 0x17 {
        return None;
    }
    r.read_bit();
    r.read_bit();
    r.read_bit();
    r.read_zstr8();
    if r.remaining_bits() < 64 {
        return None;
    }
    (r.read_u32() == 5).then(|| r.read_u32())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_wire::udp::BitWriter;

    fn intro_writer(w: &mut BitWriter, bases: bool) {
        for s in ["head\\a.dtx", "body\\b.dtx", "map\\a.dtx"] {
            w.write_zstr8(s.as_bytes());
        }
        for v in [0.1f32, 0.0, -0.2, 0.4, 0.0, 0.3] {
            w.write_f32(v);
        }
        w.write_zstr8(b"radar\\r.dtx").write_zstr8(b"map\\b.dtx");
        for v in [0.2f32, 0.0, 0.2, 0.5, 0.0, 0.4] {
            w.write_f32(v);
        }
        w.write_f32(0.0)
            .write_f32(0.34)
            .write_u32(1024)
            .write_u32(1024);
        if bases {
            for v in [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0] {
                w.write_f32(v);
            }
        }
    }

    /// The reference stack's 0x45 form (tdm1 / tdm6: `round_secs` 0, intro set, banner clear, countdown 0, a class-1 intro
    /// block, `round_no` 0) read to the last bit.
    // OBSERVED: capture 20261025_065016_tdm6 conn 6 t 173.437 (2,450 bits, read to bit 0 left)
    #[test]
    fn match_begin_reads_the_intro_block_to_the_end() {
        let mut w = BitWriter::new();
        w.write_u32(0).write_bit(true).write_bit(false).write_u32(0);
        intro_writer(&mut w, true);
        w.write_u32(7);
        let m = match read_shell(0x45, w.as_bytes(), w.bit_len(), Some(1)).unwrap() {
            ShellRead::MatchBegin(m) => m,
            o => panic!("{o:?}"),
        };
        let b = m.intro.as_ref().unwrap();
        assert_eq!(
            (b.head.as_str(), b.body.as_str()),
            ("head\\a.dtx", "body\\b.dtx")
        );
        assert_eq!(b.radar, "radar\\r.dtx");
        assert_eq!(b.bases, Some([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]));
        assert_eq!(b.caption, None);
        assert_eq!(m.round_no, 7, "round_no is read after the block");
        // a class-6 client reads a caption first and no bases
        let mut w6 = BitWriter::new();
        w6.write_u32(600)
            .write_bit(true)
            .write_bit(true)
            .write_u32(0);
        w6.write_zstr8(b"cap");
        intro_writer(&mut w6, false);
        w6.write_u32(2);
        let m6 = match read_shell(0x45, w6.as_bytes(), w6.bit_len(), Some(6)).unwrap() {
            ShellRead::MatchBegin(m) => m,
            o => panic!("{o:?}"),
        };
        assert!(m6.waiting_banner);
        assert_eq!(m6.round_secs, 600);
        assert_eq!(m6.intro.unwrap().caption.as_deref(), Some("cap"));
        assert_eq!(m6.round_no, 2);
        // without the intro: the head and round_no only (DeCombat's form before DSV-50)
        let mut w0 = BitWriter::new();
        w0.write_u32(180)
            .write_bit(false)
            .write_bit(false)
            .write_u32(0)
            .write_u32(0);
        let m0 = match read_shell(0x45, w0.as_bytes(), w0.bit_len(), Some(1)).unwrap() {
            ShellRead::MatchBegin(m) => m,
            o => panic!("{o:?}"),
        };
        assert_eq!((m0.round_secs, m0.intro.is_none()), (180, true));
        assert_eq!(m0.round_no, 0);
        // no mode class: the head stands, the class-dependent block is skipped and noted, not misread
        let mu = match read_shell(0x45, w6.as_bytes(), w6.bit_len(), None).unwrap() {
            ShellRead::MatchBegin(m) => m,
            o => panic!("{o:?}"),
        };
        assert_eq!(
            (
                mu.round_secs,
                mu.show_intro,
                mu.waiting_banner,
                mu.countdown_secs
            ),
            (600, true, true, 0)
        );
        assert!(mu.intro.is_none() && mu.intro_skipped);
        // no mode class and no block: the whole message is read
        let mn = match read_shell(0x45, w0.as_bytes(), w0.bit_len(), None).unwrap() {
            ShellRead::MatchBegin(m) => m,
            o => panic!("{o:?}"),
        };
        assert!(!mn.intro_skipped && mn.intro.is_none());
        assert_eq!(mn.round_secs, 180);
    }

    /// CLI-1050: the bomb messages in their S2C field order.
    #[test]
    fn bomb_messages_read() {
        use super::super::bomb::BombEvent;
        let mut w = BitWriter::new();
        w.write_u32(40);
        assert_eq!(
            read_shell(0x51, w.as_bytes(), w.bit_len(), Some(2)),
            Some(ShellRead::Bomb(BombEvent::Planted(40)))
        );
        assert_eq!(
            read_shell(0x50, &[], 0, Some(2)),
            Some(ShellRead::Bomb(BombEvent::Defused))
        );
        let mut w = BitWriter::new();
        w.write_objref16(Some(9));
        assert_eq!(
            read_shell(0x56, w.as_bytes(), w.bit_len(), Some(2)),
            Some(ShellRead::Bomb(BombEvent::Carrier(Some(9))))
        );
        let mut w = BitWriter::new();
        w.write_u16(5)
            .write_u8(2)
            .write_u32(0)
            .write_f32(7.0)
            .write_u32(0);
        assert_eq!(
            read_shell(0xad, w.as_bytes(), w.bit_len(), Some(2)),
            Some(ShellRead::Bomb(BombEvent::Track {
                obj: 5,
                kind: 2,
                secs: 7.0
            }))
        );
    }

    /// CLI-1060: 0xd3 subs 0 (team 2 with the infector) and 1 in the field order of C-GM-2 / C-GM-3.
    #[test]
    fn infection_messages_read() {
        use super::super::qrt::QrtEvent;
        let mut w = BitWriter::new();
        w.write_u8(0).write_u8(2).write_u32(4).write_u32(6);
        assert_eq!(
            read_shell(0xd3, w.as_bytes(), w.bit_len(), Some(11)),
            Some(ShellRead::Qrt(QrtEvent::Team {
                team: 2,
                infector: 4,
                pid: 6
            }))
        );
        let mut w = BitWriter::new();
        w.write_u8(1)
            .write_u8(1)
            .write_u8(5)
            .write_bit(false)
            .write_f32(20.0);
        assert_eq!(
            read_shell(0xd3, w.as_bytes(), w.bit_len(), Some(11)),
            Some(ShellRead::Qrt(QrtEvent::Countdown {
                a: 1,
                b: 5,
                secs: 20.0
            }))
        );
        // CLI-1211: sub 3, the own override row (signed)
        for row in [6i8, -1] {
            let mut w = BitWriter::new();
            w.write_u8(3).write_u8(row as u8);
            assert_eq!(
                read_shell(0xd3, w.as_bytes(), w.bit_len(), Some(11)),
                Some(ShellRead::Qrt(QrtEvent::Override(row)))
            );
        }
    }

    /// CLI-1080: 0xcb subs 0, 1, 5 and 6 in the field order of GAME_MODES §5.6.
    #[test]
    fn bounty_messages_read() {
        use super::super::spy::SpyEvent;
        let rd = |w: &BitWriter| read_shell(0xcb, w.as_bytes(), w.bit_len(), Some(9));
        let mut w = BitWriter::new();
        w.write_u8(0).write_u32(3).write_u8(2).write_u32(6);
        assert_eq!(
            rd(&w),
            Some(ShellRead::Spy(SpyEvent::Team {
                value: 3,
                team: 2,
                pid: 6
            }))
        );
        let mut w = BitWriter::new();
        w.write_u8(1).write_u32(5).write_u32(2).write_u32(1);
        assert_eq!(
            rd(&w),
            Some(ShellRead::Spy(SpyEvent::Counts {
                total: 5,
                dropped: 2,
                spies: 1
            }))
        );
        let mut w = BitWriter::new();
        w.write_u8(5)
            .write_u8(1)
            .write_zstr8(b"A")
            .write_zstr8(b"B");
        assert_eq!(
            rd(&w),
            Some(ShellRead::Spy(SpyEvent::Phase {
                phase: 1,
                title: "A".into(),
                text: "B".into()
            }))
        );
        let mut w = BitWriter::new();
        w.write_u8(6).write_u32(180).write_u32(7);
        assert_eq!(
            rd(&w),
            Some(ShellRead::Spy(SpyEvent::Timer {
                secs: 180,
                colour: 7
            }))
        );
        let mut w = BitWriter::new();
        w.write_u8(3);
        assert_eq!(rd(&w), Some(ShellRead::Spy(SpyEvent::Dropped)));
    }

    /// CLI-1360: 0xa2 pairs until the end marker (cshell+0x2d73a..0x2d798).
    #[test]
    fn ping_table_reads_pairs_until_the_end_marker() {
        let mut w = BitWriter::new();
        w.write_u16(0).write_u16(40).write_u16(5).write_u16(118);
        w.write_u16(PING_TABLE_END).write_u16(99);
        assert_eq!(
            read_shell(0xa2, w.as_bytes(), w.bit_len(), None),
            Some(ShellRead::PingTable(vec![(0, 40), (5, 118)]))
        );
        let mut w = BitWriter::new();
        w.write_u16(PING_TABLE_END);
        assert_eq!(
            read_shell(0xa2, w.as_bytes(), w.bit_len(), None),
            Some(ShellRead::PingTable(vec![]))
        );
    }

    /// DSV-480 / CLI-1631: our shell 0xf1 carries other players' health; a cut body is dropped.
    #[test]
    fn remote_health_is_read_from_our_own_shell_id() {
        use dc_wire::remotehealth::{write_body, HealthEntry, SHELL_ID};
        let es = vec![
            HealthEntry { pid: 3, health: 57, max: 100 },
            HealthEntry { pid: 9, health: 0, max: 150 },
        ];
        let mut w = BitWriter::new();
        write_body(&mut w, &es);
        assert_eq!(SHELL_ID, 0xf1);
        assert_eq!(
            read_shell(SHELL_ID, w.as_bytes(), w.bit_len(), None),
            Some(ShellRead::RemoteHealth(es))
        );
        assert_eq!(read_shell(SHELL_ID, w.as_bytes(), 30, None), None);
        // the HUD keeps the last value per player and forgets a player who left
        let mut s = crate::hud::state::HudState::default();
        let ev = |pid, health| {
            crate::hud::state::HudEvent::RemoteHealth(vec![HealthEntry { pid, health, max: 100 }])
        };
        s.apply(&ev(3, 57), 0, 0.0);
        s.apply(&ev(3, 20), 0, 0.0);
        assert_eq!(s.remote_health.get(&3), Some(&(20, 100)));
        s.apply(&crate::hud::state::HudEvent::Left(3, "c".into()), 0, 0.0);
        assert!(s.remote_health.is_empty());
    }

    /// BOT-95: every shell the HUD reads reaches it: the session hands it over whole (`GameEvent::Shell`).
    #[test]
    fn every_shell_the_hud_reads_is_handed_over_by_the_session() {
        let body = [0u8; 256];
        for shell in 0..=255u8 {
            for class in [None, Some(7), Some(11)] {
                if read_shell(shell, &body, body.len() * 8, class).is_some() {
                    assert!(
                        dc_botclient::session::RAW_SHELL_EVENTS.contains(&shell),
                        "shell {shell:#04x} is read by the HUD but never handed over"
                    );
                }
            }
        }
    }

    /// CLI-993: 0xc6 in the field order of cshell+0x5aae0 (C-GM-4).
    #[test]
    fn objective_state_reads_the_flag_event() {
        let mut w = BitWriter::new();
        w.write_u16(0x0401);
        for v in [2u32, 1, 3, 3, 2] {
            w.write_u32(v);
        }
        w.write_bit(false).write_bit(true);
        assert_eq!(
            read_shell(0xc6, w.as_bytes(), w.bit_len(), Some(7)),
            Some(ShellRead::Flag(super::super::modetop::FlagEvent {
                obj: 0x0401,
                state: 2,
                team: 1,
                carrier: 3,
                actor: 3,
                event: 2,
                at_base: false,
                quiet: true,
            }))
        );
    }

    /// The 0xc9 bytes our game server sends (its own encoders: the goal list of DSV-574 and the banner) read back as
    /// the goal list, the timers, the two scoreboard bytes and the banner; a damaged list is ignored.
    #[test]
    fn coop_mission_list_and_banner_from_the_servers_encoder() {
        use super::super::mission::{Goal, MissionEvent};
        use dc_dserver::gs::{coop_banner, coop_mission_list, MissionGoal};
        // the server's message is [S2C shell id:u8][shell:u8][body]: the HUD gets the shell id and the body
        let body = |w: &BitWriter| {
            let b = w.as_bytes();
            assert_eq!(b[1], 0xc9);
            (b[2..].to_vec(), w.bit_len() - 16)
        };
        let goals = [
            MissionGoal {
                id: 0,
                label: b"Secure the landing zone".to_vec(),
                state: 1,
            },
            MissionGoal {
                id: 1,
                label: b"Rescue the hostage".to_vec(),
                state: 2,
            },
            MissionGoal {
                id: 4,
                label: b"Destroy the BTR".to_vec(),
                state: 0,
            },
        ];
        let (b, bits) = body(&coop_mission_list(&goals, &[300.0], 2, 1));
        let want = MissionEvent::List {
            goals: vec![
                Goal {
                    id: 0,
                    label: "Secure the landing zone".into(),
                    state: 1,
                },
                Goal {
                    id: 1,
                    label: "Rescue the hostage".into(),
                    state: 2,
                },
                Goal {
                    id: 4,
                    label: "Destroy the BTR".into(),
                    state: 0,
                },
            ],
            timers: vec![300.0],
            stage: 2,
            goal: 1,
        };
        assert_eq!(
            read_shell(0xc9, &b, bits, Some(5)),
            Some(ShellRead::Mission(want))
        );
        // the wave HUD's empty list (Cabin Fever) and its banner
        let (b, bits) = body(&coop_mission_list(&[], &[], 3, 0));
        assert_eq!(
            read_shell(0xc9, &b, bits, Some(5)),
            Some(ShellRead::Mission(MissionEvent::List {
                goals: vec![],
                timers: vec![],
                stage: 3,
                goal: 0,
            }))
        );
        let (b, bits) = body(&coop_banner(b"Wave 3 / 5", 3.0));
        assert_eq!(
            read_shell(0xc9, &b, bits, Some(5)),
            Some(ShellRead::Mission(MissionEvent::Banner {
                text: "Wave 3 / 5".into(),
                secs: 3.0,
            }))
        );
        // sub 3: the caption (DSV-579), its three fields as one text
        let (b, bits) = body(&dc_dserver::gs::coop_caption(
            b"t\\face.dtx|Sgt. Ray|Hold the line",
            4.0,
        ));
        assert_eq!(
            read_shell(0xc9, &b, bits, Some(5)),
            Some(ShellRead::Mission(MissionEvent::Caption {
                text: "t\\face.dtx|Sgt. Ray|Hold the line".into(),
                secs: 4.0,
            }))
        );
        assert_eq!(read_shell(0xc9, &b, bits - 8, Some(5)), None, "short");
        // sub 5 (the 2009 single-goal forms)
        let mut w = BitWriter::new();
        w.write_u8(5).write_u8(1).write_u32(7).write_zstr8(b"Hold");
        assert_eq!(
            read_shell(0xc9, w.as_bytes(), w.bit_len(), Some(5)),
            Some(ShellRead::Mission(MissionEvent::Added {
                id: 7,
                label: "Hold".into(),
            }))
        );
        let mut w = BitWriter::new();
        w.write_u8(5).write_u8(3).write_u32(7).write_u8(1);
        assert_eq!(
            read_shell(0xc9, w.as_bytes(), w.bit_len(), Some(5)),
            Some(ShellRead::Mission(MissionEvent::State { id: 7, state: 1 }))
        );
        // a count past the end
        let mut w = BitWriter::new();
        w.write_u8(1).write_u32(1000).write_u32(0);
        assert_eq!(read_shell(0xc9, w.as_bytes(), w.bit_len(), Some(5)), None);

        // the session hands 0xc9 over to the HUD
        assert!(dc_botclient::session::RAW_SHELL_EVENTS.contains(&0xc9));
    }

    #[test]
    fn hisec_coop_notes_and_round_messages() {
        let mut w = BitWriter::new();
        w.write_u8(2).write_u32(1).write_u32(HISEC_NOBODY);
        assert_eq!(
            read_shell(0xd6, w.as_bytes(), w.bit_len(), Some(1)),
            Some(ShellRead::Hisec(CaseEvent::Owner {
                key: 1,
                owner: HISEC_NOBODY
            }))
        );
        let mut w = BitWriter::new();
        w.write_u8(4).write_bit(false).write_u32(90);
        assert_eq!(
            read_shell(0xd7, w.as_bytes(), w.bit_len(), Some(5)),
            Some(ShellRead::CoopClock {
                running: false,
                secs: 90
            })
        );
        // the reference stack's countdown note to object 0x70: `17 70 00 02 0a 00 00 00`
        // OBSERVED: capture 20261025_065016_tdm6 conn 6 (0xa6 type 0x17 sub 2 notes)
        let b = [0x17u8, 0x70, 0x00, 0x02, 0x0a, 0x00, 0x00, 0x00];
        assert_eq!(
            read_shell(0xa6, &b, 64, Some(1)),
            Some(ShellRead::Note(ObjectNote::Countdown {
                obj: 0x70,
                remaining: 10
            }))
        );
        let b = [0x18u8, 0xfc, 0x00, 0x1d];
        assert_eq!(
            read_shell(0xa6, &b, 32, Some(1)),
            Some(ShellRead::Note(ObjectNote::StepMute {
                obj: 0xfc,
                on: true
            }))
        );
        // the health / armour notes of a character (sub 0x14 / 0x15, 16 bits)
        let b = [0x18u8, 0xfc, 0x00, 0x14, 0x57, 0x00];
        assert_eq!(
            read_shell(0xa6, &b, 48, Some(1)),
            Some(ShellRead::Note(ObjectNote::Vitals {
                obj: 0xfc,
                armour: false,
                value: 87
            }))
        );
        let b = [0x18u8, 0xfc, 0x00, 0x15, 0x2c, 0x01];
        assert_eq!(
            read_shell(0xa6, &b, 48, Some(1)),
            Some(ShellRead::Note(ObjectNote::Vitals {
                obj: 0xfc,
                armour: true,
                value: 300
            }))
        );
        assert_eq!(read_shell(0xa6, &b, 40, Some(1)), None, "a cut value is no note");
        let b = [0x18u8, 0xfc, 0x00, 0x1f];
        assert_eq!(
            read_shell(0xa6, &b, 32, Some(1)),
            Some(ShellRead::Note(ObjectNote::SpawnMark {
                obj: 0xfc,
                on: true
            }))
        );
        let mut w = BitWriter::new();
        w.write_u32(3).write_s8(-1).write_u8(0).write_u8(2);
        assert_eq!(
            read_shell(0x46, w.as_bytes(), w.bit_len(), Some(1)),
            Some(ShellRead::RoundResult {
                round_secs: 3,
                win_team: -1,
                wins: [0, 2]
            })
        );
        let mut w = BitWriter::new();
        w.write_bit(false).write_u32(2);
        assert_eq!(
            read_shell(0x3f, w.as_bytes(), w.bit_len(), Some(1)),
            Some(ShellRead::ResultScreen {
                flag: false,
                value: 2
            })
        );
        assert_eq!(read_shell(0x46, &[0, 0], 16, Some(1)), None, "short body");
    }

    /// The case blob of the reference stack's case creates (`17 01 28 00 00 00 08 00 00 00 00`, 83 bits: key 1).
    // OBSERVED: capture 20261025_065016_tdm6 conn 6 seq 3015 (the dserver test of the same form)
    #[test]
    fn case_key_from_the_create_blob() {
        let b = [0x17u8, 0x01, 0x28, 0, 0, 0, 0x08, 0, 0, 0, 0];
        assert_eq!(case_key(&b, 83), Some(1));
        let b6 = [0x17u8, 0x01, 0x28, 0, 0, 0, 0x30, 0, 0, 0, 0];
        assert_eq!(case_key(&b6, 83), Some(6));
        assert_eq!(case_key(&[0x18, 0, 0], 24), None);
    }
}
