//! The per-mode top blocks of the score element beside the team counters, for the round and objective modes:
//! Elimination Pro (mode class 12) shows its mode title, the round limit as two digits with the round word, and each
//! team's live mark with the number of its living players.
//!
//! The 2009 element builds every piece once (cshell+0x8ebe0) and shows the pieces of the room's class
//! (cshell+0x92570); the digits are score digit images `prefix + d + ".dtx"`.
// EVIDENCE: cshell+0x8ebe0 (items and places), cshell+0x92570 (class 12: title at its own place, round-limit
// digits, round word, both live marks and live digits shown; the mission title hidden), cshell+0x91310 (round
// limit: tens = limit / 10 (mod 10), ones = limit - 10 x tens), cshell+0x90ea0 (live digits, not in classes 1 / 4),
// cshell+0x930a0 (live number = the team's members without the dead flag), cshell+0x93970 (0x6d sets the flag),
// cshell+0x93120 (0x44 / 0x3f clear it)

use super::banners::Voice;
use super::items::DrawItem;
use super::layout::Img;
use super::state::HudState;
use dc_ui_data::{HudKey as K, HudView};

/// The Elimination Pro block (mode class 12).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoundTop {
    pub title: Option<Img>,
    /// The mission title of the Search and Destroy classes (2 / 3) in place of the Elimination Pro title.
    pub mission: Option<Img>,
    /// Round-limit tens and ones places (the goal digit size).
    pub limit: Option<[Img; 2]>,
    pub word: Option<Img>,
    /// Team 1 and team 2 live marks.
    pub marks: [Option<Img>; 2],
    /// Team 1 and team 2 live-number places.
    pub live: [Option<Img>; 2],
    pub digit_prefix: String,
    /// The texels of one score digit (the team counters' digit size): a digit place shows the whole cell shrunk
    /// into its size (CLI-1102).
    pub digit_cell: [i32; 2],
    /// CLI-1163 the clan Elimination Pro (class 13) block's own places: the title, the round limit's tens and ones,
    /// the round word, the current round's tens and ones, and the division mark between them.
    pub clan: ClanRoundTop,
}

/// The places the clan Elimination Pro block (mode class 13) differs in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClanRoundTop {
    pub title_pos: Option<[i32; 2]>,
    pub limit: Option<[[i32; 2]; 2]>,
    pub word_pos: Option<[i32; 2]>,
    pub current: Option<[[i32; 2]; 2]>,
    pub division: Option<Img>,
}

fn img(v: &HudView<'_>, tex: K, pos: K, size: K) -> Option<Img> {
    Some(Img {
        tex: v.text(tex)?.to_string(),
        pos: v.pair(pos)?,
        size: v.pair(size)?,
    })
}

fn place(v: &HudView<'_>, pos: K, size: K) -> Option<Img> {
    Some(Img {
        tex: String::new(),
        pos: v.pair(pos)?,
        size: v.pair(size)?,
    })
}

impl RoundTop {
    pub fn load(v: &HudView<'_>) -> Self {
        RoundTop {
            title: img(v, K::TsvTitleTex, K::TsvTitlePos, K::TsvTitleSize),
            mission: img(v, K::SndTitleTex, K::SndTitlePos, K::SndTitleSize),
            limit: place(v, K::MaxRoundTensPos, K::GoalDigitSize)
                .zip(place(v, K::MaxRoundOnesPos, K::GoalDigitSize))
                .map(|(a, b)| [a, b]),
            word: img(v, K::RoundWordTex, K::RoundWordPos, K::RoundWordSize),
            marks: [
                img(v, K::AlphaLiveMarkTex, K::AlphaLiveMarkPos, K::LiveMarkSize),
                img(v, K::BravoLiveMarkTex, K::BravoLiveMarkPos, K::LiveMarkSize),
            ],
            live: [
                place(v, K::AlphaLiveNumPos, K::LiveNumSize),
                place(v, K::BravoLiveNumPos, K::LiveNumSize),
            ],
            digit_prefix: v.text(K::ScoreDigits).unwrap_or_default().to_string(),
            digit_cell: v.pair(K::Team1DigitSize).unwrap_or([36, 40]),
            clan: ClanRoundTop {
                title_pos: v.pair(K::ClassCTsvTitlePos),
                limit: v
                    .pair(K::ClassCTsvMaxRoundTensPos)
                    .zip(v.pair(K::ClassCTsvMaxRoundOnesPos))
                    .map(|(a, b)| [a, b]),
                word_pos: v.pair(K::ClassCTsvRoundWordPos),
                current: v
                    .pair(K::CurrentRoundTensPos)
                    .zip(v.pair(K::CurrentRoundOnesPos))
                    .map(|(a, b)| [a, b]),
                division: img(
                    v,
                    K::RoundDivisionTex,
                    K::RoundDivisionPos,
                    K::RoundDivisionSize,
                ),
            },
        }
    }
}

const OPAQUE: [u8; 4] = [255, 255, 255, 255];

fn image(i: &Img, tex: Option<String>) -> DrawItem {
    DrawItem::Image {
        tex: tex.unwrap_or_else(|| i.tex.clone()),
        src: [i.size[0] as f32, i.size[1] as f32],
        dst: [
            i.pos[0] as f32,
            i.pos[1] as f32,
            i.size[0] as f32,
            i.size[1] as f32,
        ],
        tint: OPAQUE,
        turn: None,
    }
}

/// The living players of team 1 and team 2 on the board (members without the dead mark).
// EVIDENCE: cshell+0x930a0 (count of each team list's entries whose flag byte is 0)
pub fn live_counts(s: &HudState) -> [u32; 2] {
    let mut n = [0u32; 2];
    for r in s.roster.values().filter(|r| !r.dead) {
        if let Some(c) = (r.team as usize).checked_sub(1).and_then(|i| n.get_mut(i)) {
            *c += 1;
        }
    }
    n
}

/// Draws the block of the state's mode class (nothing for the other classes).
// DC-DEFAULT: CLI-990 (proposed) the round limit is the room goal of 0x7d (the 2009 element takes it from the same
// setter as the kill goal, cshell+0x92e20); a live number above 9 shows its last digit (the 2009 form `%s%d.dtx` would
// name an image the ROM does not have)
// EVIDENCE: cshell+0x92570 (classes 2 / 3 show the same round-limit, round-word, live-mark and live-digit items as
// 12 with the mission title, item +0x18 = the mission title image, where 12 hides it for its own title)
// DC-DEFAULT: CLI-1055 (proposed) the Search and Destroy top block is the class-12 block with the mission title
pub fn draw(out: &mut Vec<DrawItem>, round: &RoundTop, s: &HudState) {
    let clan = s.mode_class == Some(13);
    let title = match s.mode_class {
        Some(12 | 13) => &round.title,
        Some(2 | 3) => &round.mission,
        _ => return,
    };
    let digit = |d: u32| format!("{}{d}.dtx", round.digit_prefix);
    // DC-DEFAULT: CLI-1102 (proposed, playtest report; as CLI-1111) the round-limit and live digits draw the whole
    // score digit cell (the team counters' digit size) shrunk into their place: their own size alone cut the top-left
    // texels of the larger digit image, which the 2560 x 1440 frame showed as a mis-scaled glyph
    let cell = [round.digit_cell[0] as f32, round.digit_cell[1] as f32];
    let digit_image = |p: &Img, d: u32| DrawItem::Image {
        tex: digit(d),
        src: cell,
        dst: [
            p.pos[0] as f32,
            p.pos[1] as f32,
            p.size[0] as f32,
            p.size[1] as f32,
        ],
        tint: OPAQUE,
        turn: None,
    };
    // CLI-1163 the clan block moves the title, the round limit and the round word to its own places and adds the
    // current round and the division mark
    // EVIDENCE: cshell+0x92570 (class 13: the title at its clan place, the current-round and division items shown),
    // cshell+0x91310 (the limit digits and the round word at the clan places), cshell+0x91550 (the current round's
    // tens and ones, the digit form of the limit), cshell+0x92120 (the current round is the 0x44 round number),
    // cshell+0x8ebe0 (the current-round places and the division mark at the layout places)
    let moved = |i: &Img, p: Option<[i32; 2]>| Img {
        pos: p.filter(|_| clan).unwrap_or(i.pos),
        ..i.clone()
    };
    if let Some(t) = title {
        out.push(image(&moved(t, round.clan.title_pos), None));
    }
    if let (Some(places), Some(goal)) = (&round.limit, s.goal) {
        let tens = (goal / 10) % 10;
        let ones = goal % 10;
        let at = round.clan.limit.filter(|_| clan);
        for (k, (p, d)) in places.iter().zip([tens, ones]).enumerate() {
            let mut p = p.clone();
            if let Some(a) = at {
                p.pos = a[k];
            }
            out.push(digit_image(&p, d));
        }
    }
    if let Some(w) = &round.word {
        out.push(image(&moved(w, round.clan.word_pos), None));
    }
    if clan {
        if let (Some(places), Some(first)) = (&round.clan.current, &round.limit) {
            let n = s.round_no;
            for (k, (p, d)) in places.iter().zip([(n / 10) % 10, n % 10]).enumerate() {
                let mut at = first[k].clone();
                at.pos = *p;
                out.push(digit_image(&at, d));
            }
        }
        if let Some(d) = &round.clan.division {
            out.push(image(d, None));
        }
    }
    for m in round.marks.iter().flatten() {
        out.push(image(m, None));
    }
    let live = live_counts(s);
    for (p, n) in round.live.iter().zip(live) {
        if let Some(p) = p {
            out.push(digit_image(p, n % 10));
        }
    }
}

/// One 0xc6 `GS_OBJECTIVE_STATE` (client ids; `u32::MAX` = nobody).
// SPEC: GAME_MODES_2009.md §5.5 (C-GM-4)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlagEvent {
    pub obj: u16,
    pub state: u32,
    pub team: u32,
    pub carrier: u32,
    pub actor: u32,
    pub event: u32,
    pub at_base: bool,
    pub quiet: bool,
}

/// Which notice a flag event shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagNotice {
    Taken,
    TakenMe,
    Captured,
    Returned,
    ReturnedMe,
    EnemyTaken,
    EnemyCaptured,
    EnemyReturned,
}

/// How long a flag notice stays.
// EVIDENCE: cshell+0xbdb50 (the notice element gets 3000 ms)
pub const FLAG_NOTICE_MS: u32 = 3000;

/// The flag modes' HUD state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CtfHud {
    /// The side each team's flag is drawn on (1 / 2; 0 = not drawn); `None` before any event: the flag's own side.
    pub state: [Option<u32>; 2],
    /// The own player carries a flag.
    pub carrying: bool,
    /// The last notice and when it started.
    pub notice: Option<(FlagNotice, u32)>,
}

/// The flag line images of the multi-kill notes, in the layout's order: own team took / returned / captured, enemy
/// took / returned / captured.
// EVIDENCE: cshell+0xa36a0 (the six images at +0x47c .. +0x490 in this order)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagLine {
    TeamTake,
    TeamReturn,
    TeamCapture,
    EnemyTake,
    EnemyReturn,
    EnemyCapture,
}

/// The flag line of a 0xc6 event: (image, the actor is of the own team), `None` for a drop or another event.
// EVIDENCE: cshell+0xa6bd0 (own flag: 1 enemy captured, 2 enemy took, 3 own team returned; the enemy flag: 1 own team
// captured, 2 own team took, 3 enemy returned; the own team's lines in the first colour)
pub fn flag_line(own_flag: bool, event: u32) -> Option<(FlagLine, bool)> {
    Some(match (own_flag, event) {
        (true, 1) => (FlagLine::EnemyCapture, false),
        (true, 2) => (FlagLine::EnemyTake, false),
        (true, 3) => (FlagLine::TeamReturn, true),
        (false, 1) => (FlagLine::TeamCapture, true),
        (false, 2) => (FlagLine::TeamTake, true),
        (false, 3) => (FlagLine::EnemyReturn, false),
        _ => return None,
    })
}

/// A capture flag (or the own capture zone) on screen this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlagMark {
    pub team: u8,
    pub base: bool,
    /// Virtual-screen point (`None`: behind the camera).
    pub at: Option<[f32; 2]>,
    /// A flag's return countdown in seconds (0 or less: none drawn).
    pub remaining: i32,
    /// A zone's distance from the camera in metres.
    pub metres: i32,
}

/// CLI-1162 the base marker's distance text: the straight distance from the camera to the zone in world units, as a
/// whole number, divided by 40 (whole metres), printed `<n> m`.
// EVIDENCE: cshell+0x86f10 (the zone's point and the camera's point -> the square root of the squared differences ->
// the integer conversion cshell+0x374a5f00 -> a multiply by 0x66666667 with the shift 4 = division by 40 -> the
// format of a number and an `m`)
pub fn metres_between(a: [f32; 3], b: [f32; 3]) -> i32 {
    let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    d as i32 / 40
}

/// Height above a flag or zone at which its marker stands (the console value's default, as the case indicator).
// EVIDENCE: cshell+0x86f10 (the flag offset console value, default 70, added to the object's height)
pub const FLAG_MARK_LIFT: f32 = 70.0;

/// The team of a capture flag's create blob (type 0x17, kind 2; `None` for any other blob).
// SPEC: GAME_MODES_2009.md §4.4 (kind 2 `[state][team][at_base]`), GAME_MESSAGES_2009_S2C.md §8.2
pub fn flag_blob_team(bytes: &[u8], bits: usize) -> Option<u8> {
    let b = crate::drops::read_pickup_blob(bytes, bits)?;
    if b.kind != crate::drops::PICKUP_KIND_FLAG {
        return None;
    }
    let mut r = dc_wire::udp::BitReader::with_bit_len(bytes, bits);
    r.skip_bits(8 + 3);
    r.read_zstr8();
    r.skip_bits(32 + 32);
    if r.remaining_bits() < 32 {
        return None;
    }
    u8::try_from(r.read_u32()).ok()
}

/// The team of a capture zone's create blob (type 0x37, kind 6; `None` for any other blob or no team).
// SPEC: GAME_MODES_2009.md §4.3 (`z_icon`, `z_team`, `z_class`, `z_c`, `z_value`, `z_kind` 6)
pub fn capture_zone_team(bytes: &[u8], bits: usize) -> Option<u8> {
    let mut r = dc_wire::udp::BitReader::with_bit_len(bytes, bits);
    if r.remaining_bits() < 8 + 64 + 1 + 72 + 32 + 64 || r.read_u8() != 0x37 {
        return None;
    }
    r.skip_bits(64 + 1 + 72 + 8);
    let team = r.read_u8();
    r.skip_bits(8 + 8 + 32);
    (r.read_u32() == 6 && matches!(team, 1 | 2)).then_some(team)
}

/// One team's top flag: texture, place on the first team's side, place on the second team's side.
pub type TopFlag = (String, [i32; 2], [i32; 2]);

/// The flag-mode top block and the notices.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlagTop {
    pub title: Option<Img>,
    /// Capture goal hundreds / tens / ones places (the goal digit size).
    pub goal: Option<[Img; 3]>,
    pub flags: [Option<TopFlag>; 2],
    pub flag_size: [i32; 2],
    /// Notice textures in [`FlagNotice`] order, and their size.
    pub notices: Vec<Option<String>>,
    pub notice_size: [i32; 2],
    pub digit_prefix: String,
    /// The texels of one score digit (the team counters' digit size), shrunk into the goal digit size (CLI-1212).
    pub digit_cell: [i32; 2],
    /// In-world markers: each team's flag image and the size, each team's base image and the size.
    pub marks: [Option<String>; 2],
    pub mark_size: [i32; 2],
    pub base_marks: [Option<String>; 2],
    pub base_mark_size: [i32; 2],
    /// CLI-1162 the markers' texts: font, size and the height offset of the base text (layout keys, set 0).
    pub text_font: i32,
    pub text_size: Option<i32>,
    pub text_offset: i32,
    /// The base marker's text format: a number and the metre sign (the layout holds no such literal; DC-DEFAULT).
    pub metre_sign: String,
}

impl FlagTop {
    pub fn load(v: &HudView<'_>) -> Self {
        let flag = |tex: K, a: K, b: K| Some((v.text(tex)?.to_string(), v.pair(a)?, v.pair(b)?));
        FlagTop {
            title: img(v, K::CtfTitleTex, K::CtfTitlePos, K::CtfTitleSize),
            goal: place(v, K::MaxScoreHundredsPos, K::GoalDigitSize)
                .zip(place(v, K::MaxScoreTensPos, K::GoalDigitSize))
                .zip(place(v, K::MaxScoreOnesPos, K::GoalDigitSize))
                .map(|((a, b), c)| [a, b, c]),
            flags: [
                flag(
                    K::TopAlphaFlagTex,
                    K::TopAlphaFlagAlphaPos,
                    K::TopAlphaFlagBravoPos,
                ),
                flag(
                    K::TopBravoFlagTex,
                    K::TopBravoFlagAlphaPos,
                    K::TopBravoFlagBravoPos,
                ),
            ],
            flag_size: v.pair(K::TopFlagSize).unwrap_or_default(),
            notices: [
                K::FlagMsgTaken,
                K::FlagMsgTakenMe,
                K::FlagMsgCaptured,
                K::FlagMsgReturned,
                K::FlagMsgReturnedMe,
                K::FlagMsgEnemyTaken,
                K::FlagMsgEnemyCaptured,
                K::FlagMsgEnemyReturned,
            ]
            .into_iter()
            .map(|k| v.text(k).map(str::to_string))
            .collect(),
            notice_size: v.pair(K::FlagMsgSize).unwrap_or_default(),
            digit_prefix: v.text(K::ScoreDigits).unwrap_or_default().to_string(),
            digit_cell: v.pair(K::Team1DigitSize).unwrap_or([36, 40]),
            marks: [K::FlagMarkAlphaTex, K::FlagMarkBravoTex]
                .map(|k| v.text(k).map(str::to_string)),
            mark_size: v.pair(K::FlagMarkSize).unwrap_or_default(),
            base_marks: [K::FlagBaseMarkAlphaTex, K::FlagBaseMarkBravoTex]
                .map(|k| v.text(k).map(str::to_string)),
            base_mark_size: v.pair(K::FlagBaseMarkSize).unwrap_or_default(),
            text_font: v.int(K::FlagTimerFont).unwrap_or(0),
            text_size: v.int(K::FlagTimerFontSize).filter(|n| *n > 0),
            text_offset: v.int(K::FlagTimerOffset).unwrap_or(0) & 0xff,
            metre_sign: "m".to_string(),
        }
    }
}

/// The notice of an event on a flag (`own_flag`: the own team's) with the own player as the actor (`me`).
// EVIDENCE: cshell+0xbdb50 (enemy flag: 1 capture-success, 2 taken / taken-by-me, 3 enemy-returned; own flag: 1 enemy
// captured, 2 enemy took it, 3 returned / returned-by-me; the field order of cshell+0xbcfd0)
pub fn notice_of(own_flag: bool, event: u32, me: bool) -> Option<FlagNotice> {
    use FlagNotice::*;
    Some(match (own_flag, event) {
        (false, 1) => Captured,
        (false, 2) if me => TakenMe,
        (false, 2) => Taken,
        (false, 3) => EnemyReturned,
        (true, 1) => EnemyCaptured,
        (true, 2) => EnemyTaken,
        (true, 3) if me => ReturnedMe,
        (true, 3) => Returned,
        _ => return None,
    })
}

/// The voices of an event: the own one for taking / returning, then the team one.
// EVIDENCE: cshell+0x86da0 (actor = own player and event 2 / 3: the "by me" sound; own flag: 1 fail, 2 bad, 3
// recover-good; enemy flag: 1 success, 2 good, 3 recover-bad)
pub fn voices_of(own_flag: bool, event: u32, me: bool) -> Vec<Voice> {
    let mut v = Vec::new();
    if me && (event == 2 || event == 3) {
        v.push(Voice::FlagByMe);
    }
    v.extend(match (own_flag, event) {
        (true, 1) => Some(Voice::FlagOwnCaptured),
        (true, 2) => Some(Voice::FlagOwnTaken),
        (true, 3) => Some(Voice::FlagOwnReturned),
        (false, 1) => Some(Voice::FlagEnemyCaptured),
        (false, 2) => Some(Voice::FlagEnemyTaken),
        (false, 3) => Some(Voice::FlagEnemyReturned),
        _ => None,
    });
    v
}

impl HudState {
    /// 0xc6: the flag's side, the own carrying mark (set when the own player is the carrier; for an own-team flag
    /// only changed by event 4), the notice and the voices.
    // EVIDENCE: cshell+0x5aae0 (the pickup's state), cshell+0x5d9e0 (carrying := carrier is me unless the flag is
    // the own team's and the event is not 4; notice and sounds by own flag / event / actor is me)
    // DC-DEFAULT: CLI-993 (proposed)
    pub fn on_flag(&mut self, f: &FlagEvent, now: u32) {
        let own_team = self
            .own_pid
            .and_then(|p| self.roster.get(&p))
            .map(|r| u32::from(r.team));
        let own_id = self.own_pid.map(u32::from);
        let own_flag = own_team == Some(f.team);
        if !own_flag || f.event == 4 {
            self.ctf.carrying = f.carrier != u32::MAX && Some(f.carrier) == own_id;
        }
        if let Some(slot) = (f.team as usize)
            .checked_sub(1)
            .and_then(|i| self.ctf.state.get_mut(i))
        {
            *slot = Some(f.state);
        }
        let me = f.actor != u32::MAX && Some(f.actor) == own_id;
        // the board's mission values of the actor: a capture or a return counts one (not for a quiet event); the
        // server's next 0x6b refresh carries the same totals
        // EVIDENCE: cshell+0x5d9e0 (param_5 == 0: event 1 -> cshell+0xc5b80, event 3 -> cshell+0xc5bc0 with the actor),
        // cshell+0xc5b80 / cshell+0xc5bc0 (the actor's board row +0x804 / +0x80c += 1, the board redrawn)
        // DC-DEFAULT: CLI-1112 (proposed) the two counters are the row's `v68` / `v6c` (the `%d/%d` mission text)
        if !f.quiet && matches!(f.event, 1 | 3) {
            if let Ok(actor) = u8::try_from(f.actor) {
                if let Some(m) = self.row_marks.get_mut(&actor) {
                    let k = if f.event == 1 { 1 } else { 2 };
                    m.mission[k] = m.mission[k].saturating_add(1);
                }
            }
        }
        // the flag line of the multi-kill notes (a known actor only)
        // EVIDENCE: cshell+0x5d9e0 (events 1 / 2 / 3 with the actor's record), cshell+0xa6bd0
        if let (Some((line, mate)), Ok(actor)) =
            (flag_line(own_flag, f.event), u8::try_from(f.actor))
        {
            if self.roster.contains_key(&actor) {
                let name = self.name(actor);
                self.push_mk(super::state::MkNote {
                    at: now,
                    name,
                    mate,
                    kind: super::state::MkKind::Flag(line),
                });
            }
        }
        if let Some(n) = notice_of(own_flag, f.event, me) {
            self.ctf.notice = Some((n, now));
        }
        self.banner.voices.extend(voices_of(own_flag, f.event, me));
    }
}

/// Draws the flag-mode block: title, capture goal digits, each team's top flag on the side its state names, and the
/// running notice at the centre.
// EVIDENCE: cshell+0x92570 (classes 7 / 8: the title and the three goal digits), cshell+0x86f10 (state 1: the
// flag at its first-team place, 2: at its second-team place, else hidden), cshell+0xbdb50 (the notice at y 0x1a6)
// DC-DEFAULT: CLI-992 (proposed) the notice centred across the 1024-wide screen
pub fn draw_flag(out: &mut Vec<DrawItem>, f: &FlagTop, s: &HudState) {
    if !matches!(s.mode_class, Some(7 | 8)) {
        return;
    }
    if let Some(t) = &f.title {
        out.push(image(t, None));
    }
    if let (Some(places), Some(goal)) = (&f.goal, s.goal) {
        // DC-DEFAULT: CLI-1212 (proposed, playtest report "the top text shows max points is too squished"; as CLI-1102 /
        // CLI-1111) the goal digits draw the whole score digit cell shrunk into the goal digit size: the goal size
        // alone took the top-left 18 x 20 texels of the 36 x 40 digit
        let v = goal.min(999);
        for (p, d) in places.iter().zip([v / 100, (v / 10) % 10, v % 10]) {
            out.push(DrawItem::Image {
                tex: format!("{}{d}.dtx", f.digit_prefix),
                src: [f.digit_cell[0] as f32, f.digit_cell[1] as f32],
                dst: [
                    p.pos[0] as f32,
                    p.pos[1] as f32,
                    p.size[0] as f32,
                    p.size[1] as f32,
                ],
                tint: OPAQUE,
                turn: None,
            });
        }
    }
    // the in-world markers: each flag, the own team's capture zone
    // EVIDENCE: cshell+0x86f10 (the flag indicator by the flag's team centred on the raised point, a flag's return
    // countdown above it (white, red at 5 or less, hidden at 0); the base indicator for a kind-6 zone of the own team
    // at its left edge half a flag width left of the point and its top at the point, its distance text `<n> m` at the
    // point raised by the layout's text offset, centred; nothing behind the camera)
    // CLI-1162 the countdown note is the one the HiSec cases take (cshell+0x2828e0, object +0x90)
    // DC-DEFAULT: CLI-1162 (proposed) a text's line bottom 2 px above its image as the HiSec case countdown (CLI-79):
    // the 2009 text's alignment flags 0x44 were not decoded
    let own_team = s.own_pid.and_then(|p| s.roster.get(&p)).map(|r| r.team);
    let text = |text: String, at: [f32; 2], colour: [u8; 4]| DrawItem::Label {
        style: super::layout::TextStyle {
            font: f.text_font,
            colour,
            size: f.text_size,
        },
        pos: at,
        text,
        anchor: [0.5, 1.0],
    };
    for m in &s.flag_marks {
        let Some(p) = m.at else {
            continue;
        };
        if m.base && own_team != Some(m.team) {
            continue;
        }
        let (texs, size) = if m.base {
            (&f.base_marks, f.base_mark_size)
        } else {
            (&f.marks, f.mark_size)
        };
        let Some(Some(tex)) = texs.get(usize::from(m.team).wrapping_sub(1)) else {
            continue;
        };
        // the base indicator stands at the flag indicator's half width left of the point, its top at the point
        let pos = if m.base {
            [p[0] as i32 - f.mark_size[0] / 2, p[1] as i32]
        } else {
            [p[0] as i32 - size[0] / 2, p[1] as i32 - size[1] / 2]
        };
        out.push(image(
            &Img {
                tex: tex.clone(),
                pos,
                size,
            },
            None,
        ));
        if m.base {
            out.push(text(
                format!("{} {}", m.metres, f.metre_sign),
                [p[0], p[1] - f.text_offset as f32],
                [255; 4],
            ));
        } else if m.remaining > 0 {
            let colour = if m.remaining > 5 {
                [255; 4]
            } else {
                [255, 0, 0, 255]
            };
            out.push(text(
                m.remaining.to_string(),
                [p[0], (pos[1] - 2) as f32],
                colour,
            ));
        }
    }
    for (i, fl) in f.flags.iter().enumerate() {
        let Some((tex, a, b)) = fl else {
            continue;
        };
        let state = s.ctf.state[i].unwrap_or(i as u32 + 1);
        let pos = match state {
            1 => *a,
            2 => *b,
            _ => continue,
        };
        out.push(image(
            &Img {
                tex: tex.clone(),
                pos,
                size: f.flag_size,
            },
            None,
        ));
    }
    if let Some((n, at)) = s.ctf.notice {
        if s.now_ms.wrapping_sub(at) < FLAG_NOTICE_MS {
            if let Some(Some(tex)) = f.notices.get(n as usize) {
                out.push(image(
                    &Img {
                        tex: tex.clone(),
                        pos: [(1024 - f.notice_size[0]) / 2, 0x1a6],
                        size: f.notice_size,
                    },
                    None,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::state::BoardRow;

    fn im(t: &str, x: i32) -> Img {
        Img {
            tex: t.into(),
            pos: [x, 0],
            size: [10, 10],
        }
    }

    fn block() -> RoundTop {
        RoundTop {
            title: Some(im("tsv.dtx", 1)),
            mission: Some(im("mission.dtx", 1)),
            limit: Some([im("", 2), im("", 3)]),
            word: Some(im("round.dtx", 4)),
            marks: [Some(im("alpha.dtx", 5)), Some(im("bravo.dtx", 6))],
            live: [Some(im("", 7)), Some(im("", 8))],
            digit_prefix: "sd".into(),
            digit_cell: [36, 40],
            clan: ClanRoundTop {
                title_pos: Some([101, 1]),
                limit: Some([[102, 1], [103, 1]]),
                word_pos: Some([104, 1]),
                current: Some([[110, 2], [111, 2]]),
                division: Some(im("div.dtx", 120)),
            },
        }
    }

    /// CLI-1163: class 13 draws the class-12 pieces at the clan places, plus the current round (the last 0x44's
    /// round number as two digits) and the division mark; class 12 keeps its own places and has neither.
    #[test]
    fn clan_elimination_pro_top_moves_its_places_and_adds_the_current_round() {
        let mut s = HudState {
            mode_class: Some(13),
            goal: Some(7),
            round_no: 3,
            ..HudState::default()
        };
        let mut out = Vec::new();
        draw(&mut out, &block(), &s);
        let t = texs_at(&out);
        assert!(t.contains(&("tsv.dtx".into(), 101.0)), "{t:?}");
        assert!(t.contains(&("round.dtx".into(), 104.0)), "{t:?}");
        assert!(t.contains(&("sd0.dtx".into(), 102.0)) && t.contains(&("sd7.dtx".into(), 103.0)));
        assert!(t.contains(&("sd0.dtx".into(), 110.0)) && t.contains(&("sd3.dtx".into(), 111.0)));
        assert!(t.contains(&("div.dtx".into(), 120.0)), "{t:?}");
        s.mode_class = Some(12);
        let mut out = Vec::new();
        draw(&mut out, &block(), &s);
        let t = texs_at(&out);
        assert!(t.contains(&("tsv.dtx".into(), 1.0)), "{t:?}");
        assert!(!t.iter().any(|x| x.0 == "div.dtx" || x.1 == 110.0), "{t:?}");
    }

    /// CLI-1102: the round-limit and live digits show the whole digit cell shrunk into their place size.
    #[test]
    fn the_top_block_digits_shrink_the_whole_digit_cell() {
        let s = HudState {
            mode_class: Some(2),
            goal: Some(5),
            ..HudState::default()
        };
        let mut out = Vec::new();
        draw(&mut out, &block(), &s);
        let digits: Vec<([f32; 2], [f32; 4])> = out
            .iter()
            .filter_map(|d| match d {
                DrawItem::Image { tex, src, dst, .. } if tex.starts_with("sd") => {
                    Some((*src, *dst))
                }
                _ => None,
            })
            .collect();
        assert_eq!(digits.len(), 4, "two limit and two live digits");
        for (src, dst) in digits {
            assert_eq!(src, [36.0, 40.0]);
            assert_eq!(dst[2..], [10.0, 10.0]);
        }
    }

    fn texs_at(out: &[DrawItem]) -> Vec<(String, f32)> {
        out.iter()
            .filter_map(|d| match d {
                DrawItem::Image { tex, dst, .. } => Some((tex.clone(), dst[0])),
                _ => None,
            })
            .collect()
    }

    /// CLI-990: class 12 draws the title, the round limit 7 as "0" "7", the round word, both live marks and the
    /// living players of each team (team 1: 2 of 3, team 2: 1 of 1).
    #[test]
    fn elimination_pro_top_shows_round_limit_and_live_players() {
        let mut s = HudState {
            mode_class: Some(12),
            goal: Some(7),
            ..HudState::default()
        };
        for (pid, team, dead) in [(0, 1, false), (1, 1, true), (2, 1, false), (3, 2, false)] {
            s.roster.insert(
                pid,
                BoardRow {
                    pid,
                    team,
                    dead,
                    ..BoardRow::default()
                },
            );
        }
        let mut out = Vec::new();
        draw(&mut out, &block(), &s);
        let t = texs_at(&out);
        for want in [
            ("tsv.dtx", 1.0),
            ("sd0.dtx", 2.0),
            ("sd7.dtx", 3.0),
            ("round.dtx", 4.0),
            ("alpha.dtx", 5.0),
            ("bravo.dtx", 6.0),
            ("sd2.dtx", 7.0),
            ("sd1.dtx", 8.0),
        ] {
            assert!(t.contains(&(want.0.to_string(), want.1)), "{want:?} {t:?}");
        }
        // CLI-1055: Search and Destroy draws the same block under the mission title
        s.mode_class = Some(2);
        let mut out = Vec::new();
        draw(&mut out, &block(), &s);
        let t = texs_at(&out);
        assert!(t.contains(&("mission.dtx".to_string(), 1.0)), "{t:?}");
        assert!(!t.iter().any(|x| x.0 == "tsv.dtx"));
        assert!(t.contains(&("sd7.dtx".to_string(), 3.0)));
        // a TDM room draws none of it
        s.mode_class = Some(1);
        let mut out = Vec::new();
        draw(&mut out, &block(), &s);
        assert!(out.is_empty());
    }

    /// CLI-991: 0x44 makes the board rows alive again, so the live numbers count everybody at a round start.
    #[test]
    fn a_round_begin_counts_every_player_alive_again() {
        let mut s = HudState::default();
        for (pid, team) in [(0, 1), (1, 2), (2, 2)] {
            s.roster.insert(
                pid,
                BoardRow {
                    pid,
                    team,
                    dead: true,
                    ..BoardRow::default()
                },
            );
        }
        assert_eq!(live_counts(&s), [0, 0]);
        s.on_round_begin(60, 0);
        assert_eq!(live_counts(&s), [1, 2]);
    }

    fn flag_top() -> FlagTop {
        FlagTop {
            title: Some(im("ctf.dtx", 1)),
            goal: Some([im("", 2), im("", 3), im("", 4)]),
            flags: [
                Some(("alpha_flag.dtx".into(), [10, 0], [20, 0])),
                Some(("bravo_flag.dtx".into(), [30, 0], [40, 0])),
            ],
            flag_size: [5, 5],
            notices: (0..8).map(|i| Some(format!("n{i}.dtx"))).collect(),
            notice_size: [512, 64],
            digit_prefix: "sd".into(),
            digit_cell: [36, 40],
            marks: [Some("mark_a.dtx".into()), Some("mark_b.dtx".into())],
            mark_size: [20, 30],
            base_marks: [Some("base_a.dtx".into()), Some("base_b.dtx".into())],
            base_mark_size: [40, 40],
            text_font: 3,
            text_size: Some(12),
            text_offset: 10,
            metre_sign: "m".into(),
        }
    }

    fn ctf_state() -> HudState {
        let mut s = HudState {
            mode_class: Some(7),
            goal: Some(5),
            own_pid: Some(0),
            ..HudState::default()
        };
        for (pid, team) in [(0, 1), (1, 2)] {
            s.roster.insert(
                pid,
                BoardRow {
                    pid,
                    team,
                    ..BoardRow::default()
                },
            );
        }
        s
    }

    fn ev(team: u32, carrier: u32, actor: u32, event: u32, state: u32) -> FlagEvent {
        FlagEvent {
            team,
            carrier,
            actor,
            event,
            state,
            ..FlagEvent::default()
        }
    }

    /// CLI-992 / CLI-993: the flag block (title, goal 005, each top flag on its own side) and, after the own player
    /// takes the enemy flag, the bravo flag drawn on the alpha side, the "taken by me" notice centred at y 422 for 3 s,
    /// the carrying mark and the by-me + enemy-taken voices; the capture clears the carrying mark.
    #[test]
    fn capture_the_flag_top_notice_voices_and_carrying() {
        let mut s = ctf_state();
        let mut out = Vec::new();
        draw_flag(&mut out, &flag_top(), &s);
        let t = texs_at(&out);
        for want in [
            ("ctf.dtx", 1.0),
            ("sd0.dtx", 2.0),
            ("sd0.dtx", 3.0),
            ("sd5.dtx", 4.0),
            ("alpha_flag.dtx", 10.0),
            ("bravo_flag.dtx", 40.0),
        ] {
            assert!(t.contains(&(want.0.to_string(), want.1)), "{want:?} {t:?}");
        }
        s.on_flag(&ev(2, 0, 0, 2, 1), 1000);
        assert!(s.ctf.carrying);
        assert_eq!(
            s.banner.voices,
            vec![Voice::FlagByMe, Voice::FlagEnemyTaken]
        );
        s.now_ms = 2000;
        let mut out = Vec::new();
        draw_flag(&mut out, &flag_top(), &s);
        let t = texs_at(&out);
        assert!(t.contains(&("bravo_flag.dtx".into(), 30.0)), "{t:?}");
        let notice = format!("n{}.dtx", FlagNotice::TakenMe as usize);
        assert!(t.contains(&(notice.clone(), 256.0)), "{t:?}");
        s.now_ms = 4000;
        let mut out = Vec::new();
        draw_flag(&mut out, &flag_top(), &s);
        assert!(
            !texs_at(&out).iter().any(|x| x.0 == notice),
            "gone after 3 s"
        );
        // the capture: carrier none
        s.row_marks.insert(0, Default::default());
        s.row_marks.insert(1, Default::default());
        s.on_flag(&ev(2, u32::MAX, 0, 1, 2), 5000);
        assert!(!s.ctf.carrying);
        assert_eq!(s.ctf.notice, Some((FlagNotice::Captured, 5000)));
        // CLI-1112: the capturer's board row counts one capture; a return by the other player one return; the
        // mission column reads captures / returns
        s.on_flag(&ev(2, u32::MAX, 1, 3, 2), 5100);
        assert_eq!(s.row_marks[&0].mission, [0, 1, 0]);
        assert_eq!(s.row_marks[&1].mission, [0, 0, 1]);
        assert_eq!(
            super::super::board::mission_text(s.row_marks.get(&0), Some(7)),
            "1/0"
        );
        // a TDM room draws none of it
        s.mode_class = Some(1);
        let mut out = Vec::new();
        draw_flag(&mut out, &flag_top(), &s);
        assert!(out.is_empty());
    }

    /// CLI-1073: each flag's marker by its team centred on its screen point, the base marker only over the own team's
    /// zone, nothing behind the camera; CLI-1074: the flag lines of the multi-kill notes by event, flag side and actor.
    #[test]
    fn flag_markers_and_flag_lines() {
        let mut s = ctf_state();
        let mark =
            |team: u8, base: bool, at: Option<[f32; 2]>, remaining: i32, metres: i32| FlagMark {
                team,
                base,
                at,
                remaining,
                metres,
            };
        s.flag_marks = vec![
            mark(1, false, Some([100.0, 200.0]), 12, 0),
            mark(2, false, None, 0, 0),
            mark(1, true, Some([500.0, 300.0]), 0, 25),
            mark(2, true, Some([600.0, 300.0]), 0, 25),
        ];
        let mut out = Vec::new();
        draw_flag(&mut out, &flag_top(), &s);
        let t = texs_at(&out);
        assert!(t.contains(&("mark_a.dtx".into(), 90.0)), "{t:?}");
        // CLI-1162 the base marker's left edge is half a flag marker's width left of the point
        assert!(t.contains(&("base_a.dtx".into(), 490.0)), "{t:?}");
        assert!(!t.iter().any(|x| x.0 == "mark_b.dtx" || x.0 == "base_b.dtx"));
        // CLI-1162 the flag's return countdown above its marker (white), the base's distance at the point raised by
        // the text offset
        let labels = |out: &[DrawItem]| -> Vec<(String, [f32; 2], [u8; 4])> {
            out.iter()
                .filter_map(|d| match d {
                    DrawItem::Label {
                        text, pos, style, ..
                    } => Some((text.clone(), *pos, style.colour)),
                    _ => None,
                })
                .collect()
        };
        let l = labels(&out);
        assert!(
            l.contains(&("12".into(), [100.0, 183.0], [255; 4])),
            "{l:?}"
        );
        assert!(
            l.contains(&("25 m".into(), [500.0, 290.0], [255; 4])),
            "{l:?}"
        );
        assert_eq!(
            l.len(),
            2,
            "no text for a flag with no countdown, none for the enemy zone: {l:?}"
        );
        // red at five or less
        s.flag_marks[0].remaining = 5;
        let mut out = Vec::new();
        draw_flag(&mut out, &flag_top(), &s);
        assert!(labels(&out).contains(&("5".into(), [100.0, 183.0], [255, 0, 0, 255])));
        // the distance rule: whole units over 40
        assert_eq!(metres_between([0.0; 3], [0.0, 0.0, 120.0]), 3);
        assert_eq!(metres_between([0.0; 3], [0.0, 79.0, 0.0]), 1);
        // lines: the enemy (1) takes our flag, we (0) return it, we take and capture theirs; a timer return has no
        // actor and no line
        s.on_flag(&ev(1, 1, 1, 2, 2), 100);
        s.on_flag(&ev(1, u32::MAX, 0, 3, 1), 200);
        s.on_flag(&ev(2, 0, 0, 2, 1), 300);
        s.on_flag(&ev(2, u32::MAX, 0, 1, 2), 400);
        s.on_flag(&ev(2, u32::MAX, u32::MAX, 3, 2), 500);
        let lines: Vec<(super::super::state::MkKind, bool)> =
            s.mk.iter().map(|n| (n.kind, n.mate)).collect();
        use super::super::state::MkKind::Flag;
        assert_eq!(
            lines,
            vec![
                (Flag(FlagLine::EnemyTake), false),
                (Flag(FlagLine::TeamReturn), true),
                (Flag(FlagLine::TeamTake), true),
                (Flag(FlagLine::TeamCapture), true),
            ]
        );
    }

    /// The flag and zone blob readers: a kind-2 pickup's team, a kind-6 zone's team.
    #[test]
    fn flag_and_capture_zone_blobs_read_their_team() {
        let mut w = dc_wire::udp::BitWriter::new();
        w.write_u8(0x17)
            .write_bit(true)
            .write_bit(false)
            .write_bit(false)
            .write_zstr8(b"")
            .write_u32(2)
            .write_u32(2)
            .write_u32(2)
            .write_bit(true);
        assert_eq!(flag_blob_team(w.as_bytes(), w.bit_len()), Some(2));
        let mut z = dc_wire::udp::BitWriter::new();
        z.write_u8(0x37).write_u32(0).write_u32(0).write_bit(false);
        for _ in 0..9 {
            z.write_u8(0);
        }
        z.write_u8(0xff)
            .write_u8(1)
            .write_u8(7)
            .write_u8(0)
            .write_u32(3)
            .write_u32(6)
            .write_u32(0)
            .write_u8(0)
            .write_u32(0)
            .write_zstr8(b"Z")
            .write_zstr8(b"");
        assert_eq!(capture_zone_team(z.as_bytes(), z.bit_len()), Some(1));
        assert_eq!(flag_blob_team(z.as_bytes(), z.bit_len()), None);
    }

    /// CLI-993: the notice and voice tables of cshell+0xbdb50 / cshell+0x86da0.
    #[test]
    fn flag_notices_and_voices_follow_the_flag_side_and_the_actor() {
        use FlagNotice::*;
        assert_eq!(notice_of(true, 2, false), Some(EnemyTaken));
        assert_eq!(notice_of(true, 3, true), Some(ReturnedMe));
        assert_eq!(notice_of(true, 1, false), Some(EnemyCaptured));
        assert_eq!(notice_of(false, 3, false), Some(EnemyReturned));
        assert_eq!(notice_of(false, 4, false), None);
        assert_eq!(voices_of(true, 1, false), vec![Voice::FlagOwnCaptured]);
        assert_eq!(
            voices_of(true, 3, true),
            vec![Voice::FlagByMe, Voice::FlagOwnReturned]
        );
        assert!(voices_of(false, 4, true).is_empty());
    }
}
