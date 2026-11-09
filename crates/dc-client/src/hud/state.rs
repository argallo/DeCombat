//! What the in-match HUD shows, kept from the server's game messages the way the 2009 client keeps it: health and
//! armour from 0x66, its own kill / death counts from 0x6d (S2C 3.3.1 step 6), the roster of 0x6b, the team pair
//! of 0xb5, the round clock of 0x44 / 0x45 / 0x46 / 0x48 / 0x41 and the co-op 0xd7 sub 4, the waiting banner and
//! the match intro of 0x45, the round result mark of 0x46, the result screen of 0x3f with the 0x65 rows, the kill
//! goal of 0x7d, the multi-kill notes and the own first-kill announcement of 0x6d, the HiSec case HUD of 0xd6 and
//! the case countdown notes, the own spawn notes, the damage sectors of 0x74, the own respawn wait armed by 0x67
//! state 2, and chat-log notices. Sans-IO: every input carries its time in ms.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x44, 0x45, 0x41, 0x46), §3.1-§3.4 (roster, 0x6d, 0xb5), §4.1 (0x67),
// §4.3 (0x66, 0x74); GAME_MESSAGES_2009_C2S.md §3.3 (the client's own respawn wait)
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 (0x45), §2.1.2 (0x46), §3.4 (0x65), §3.5 (0xd6, 0xd7), §8.3; HISEC_2009.md §3

use super::banners::Voice;
use super::msgs::{CaseEvent, IntroBlock, MatchBegin, ObjectNote, HISEC_NOBODY};
use dc_botclient::shell::{KillRecord, RosterRecord, SummaryRow, NO_KILLER};
use std::collections::{BTreeMap, VecDeque};

/// Number of damage-indicator sectors. DISASM: cshell+0x1f7f30..0x1f7f52 (angle + pi, times 6 / pi = 1.90986,
/// truncated; 12 floats at controller +0x15c, cleared by cshell+0x1f0bc0)
pub const DAMAGE_SECTORS: usize = 12;

/// A sector's value falls by this much per second after a hit.
// DC-DEFAULT: CLI-49 (proposed) damage sector decay 1.0 per second (the 2009 decay is in code decompiler did not split)
pub const DAMAGE_DECAY_PER_S: f32 = 1.0;

/// Kill-feed lines kept at most (older lines leave first).
// DC-DEFAULT: CLI-46 (proposed) at most 6 lines; the 2009 queue size is not read
pub const FEED_MAX_LINES: usize = 6;

/// Chat-log lines kept at most, and how long a notice stays.
// DC-DEFAULT: CLI-52 (proposed) 6 lines; notices 5000 ms (S2C 3.2 row 0x6c: the leave notice is added for 5000 ms)
pub const CHAT_MAX_LINES: usize = 6;
pub const CHAT_NOTICE_MS: u32 = 5000;

/// How long the aim cross's hit flash fades after a hit on the local player.
// DC-DEFAULT: CLI-87 (proposed) 300 ms linear fade from alpha 255 (the 2009 flash, cshell+0x835d0, takes its
// duration from a console value at handle +0x4c that is not traced)
pub const HIT_FLASH_MS: u32 = 300;

/// The 0x74 `zone` value for which the 2009 client draws no hit flash.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 row 0x74 (flash when `damage` > 0 and `zone` != 0x25)
pub const NO_FLASH_ZONE: u8 = 0x25;

/// How long the revenge panel of a 0x6d with a revenge block stays.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 field 14 (`hud_revenge` cshell+0xbb670(1, killer, value, 7000)); C2S §3.4
// (the respawn request hides it, cshell+0xbb670(0, -1, 0, 7000))
pub const REVENGE_MS: u32 = 7000;

/// The own death as the HUD keeps it: who killed the local player and with what.
#[derive(Debug, Clone, PartialEq)]
pub struct DeathNote {
    pub at: u32,
    /// The killer's name (`None`: the world, a fall, or the player himself).
    pub killer: Option<String>,
    pub killer_rel: Relation,
    pub weapon: u16,
    pub head_shot: bool,
}

/// The revenge panel: the killer's name and the count line of the 0x6d revenge block.
#[derive(Debug, Clone, PartialEq)]
pub struct RevengePanel {
    pub at: u32,
    pub killer: String,
    pub count: u32,
}

/// How a name relates to the local player's team (the kill feed colours names by it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    Mate,
    Opponent,
}

/// One kill-feed line.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedLine {
    pub at_ms: u32,
    /// Empty for no killer (world, fall).
    pub killer: String,
    pub victim: String,
    pub killer_rel: Relation,
    pub victim_rel: Relation,
    pub weapon: u16,
    pub head_shot: bool,
    /// The other marks of the line ([`super::feedmarks`], client-hudgaps).
    pub marks: super::feedmarks::FeedMarks,
}

/// One scoreboard row (the roster record the client keeps).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BoardRow {
    pub pid: u8,
    pub name: String,
    pub team: u8,
    pub kills: i32,
    pub deaths: i32,
    pub dead: bool,
    /// The last 0xa2 ping of the player (ms; `None` before the first table names him).
    pub ping: Option<u16>,
}

/// Chat-log line kinds (the colour the line gets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKind {
    /// A system notice (player joined / left).
    Notice,
    /// A HiSec case line (dropped / picked up), drawn yellow.
    // SPEC: HISEC_2009.md §3.2 (colour 0xffffff00, 5000 ms)
    Hisec,
    /// A player's line of `GS_CHAT` (S2C 0x68) in the colour of its class ([`super::chat::classify`]).
    Chat {
        class: super::chat::ChatClass,
        whisper: bool,
        own: bool,
    },
    /// The chat input's repeat-guard refusal (CRes 0x1028, 5000 ms) in the same-line colour.
    // EVIDENCE: cshell+0x72df0 (the refusal line, colour cshell+0x50c90)
    SameLine,
}

/// How long the sensitivity line stays.
// EVIDENCE: cshell+0x49060 (5000 ms)
pub const SENSITIVITY_LINE_MS: u32 = 5000;

/// Client texts the HUD forms its lines from (FormatMessage forms, `%1` = a name): join and leave notices, the HiSec
/// "case dropped" line and the "case obtained by %1" line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HudTexts {
    pub join: String,
    pub leave: String,
    pub case_drop: String,
    pub case_taken: String,
    /// The revenge count line (`%1!d!` = the count; the killer's name follows it).
    pub revenge: String,
    /// The leave line of a kicked player (CRes 0x185b) and the chat input's repeat refusal (CRes 0x1028).
    pub kicked: String,
    pub same_line: String,
    /// CLI-1031: the sensitivity hotkeys' line (CRes 0x4f6, `%1!d!` = the value).
    pub sensitivity: String,
    /// CLI-1123: the team chat's name, the tag of a team line (empty: no tag).
    pub team_chat: String,
}

/// The round clock (`hud_round_timer`): shown flag, the seconds it was set to, when, and whether it runs.
// EVIDENCE: cshell+0xbe0c0 (set: seconds, start = now, frozen unless running), cshell+0x6f860 (left = start +
// seconds - now, 0 when spent), cshell+0xbe570 (shown as minutes / seconds), cshell+0xbe070 (drawn while shown)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RoundClock {
    pub shown: bool,
    pub secs: u32,
    pub at: u32,
    pub running: bool,
}

impl RoundClock {
    /// Milliseconds left at `now` (a frozen clock keeps its seconds).
    pub fn ms_left(&self, now: u32) -> u32 {
        let total = self.secs.saturating_mul(1000);
        if self.running {
            total.saturating_sub(now.saturating_sub(self.at))
        } else {
            total
        }
    }

    /// The (minutes, seconds) the clock shows: whole seconds left truncated; the seconds field is the whole
    /// count while at most 60 s are left (so exactly 60 s reads 1:60, as the 2009 split does).
    // EVIDENCE: cshell+0xbe570 (ftol of the float left; minutes = n / 60; seconds = n % 60, or n when left <= 60.0)
    pub fn shows(&self, now: u32) -> (u32, u32) {
        let ms = self.ms_left(now);
        let n = ms / 1000;
        (n / 60, if ms <= 60_000 { n } else { n % 60 })
    }
}

/// The round result mark of 0x46 (and its reset form).
// EVIDENCE: cshell+0xc1a50 (win_team == own team: win image; other team: lose image; 0: draw image unless the
// waiting-banner latch is set)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultMark {
    Win,
    Lose,
    Draw,
    /// A draw while the waiting-banner latch is set: the plain scoreboard image.
    Score,
    /// Class 11: the humans (win value 1) or the infectors (another non-zero value) won the round, for every viewer.
    HumanWin,
    InfectorWin,
}

/// The scoreboard with the 0x46 result header shows this long after the result.
// EVIDENCE: cshell+0xc02e0 (GetTickCount - result time > 3000), cshell+0xc1a50 (the result time is set with the mark)
pub const RESULT_BOARD_DELAY_MS: u32 = 3000;

/// The match intro banner started by a 0x45 intro block: title and text images, start time.
#[derive(Debug, Clone, PartialEq)]
pub struct IntroShow {
    pub head: String,
    pub body: String,
    pub at: u32,
}

/// CRATE-2: the Camo Crate's match lines (our own words; the name in the HiSec line's white).
pub const CAMO_DROP_TEXT: &str = "A Camo Crate has dropped.";
pub const CAMO_TAKEN_TEXT: &str = "^0xFFFFFFFF^%s^ has obtained a Camo Crate.";

/// Kinds of the multi-kill notes on the left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MkKind {
    /// 0x6d with `streak_flag` set: "<killer> first kill".
    // EVIDENCE: cshell+0xa5fe0 (a non-zero second argument picks the first-kill image for any known killer)
    Opener,
    /// 0xd6 sub 1: "<player> obtained a HiSec case".
    // EVIDENCE: cshell+0xa75a0
    Hisec,
    /// 0xc6 events 1 / 2 / 3: "<actor> took / returned / captured a flag", the image by [`super::modetop::FlagLine`].
    // EVIDENCE: cshell+0x5d9e0 -> cshell+0xa6bd0
    Flag(super::modetop::FlagLine),
}

/// One multi-kill note: when, the name, whether the name is of the own team, the kind.
#[derive(Debug, Clone, PartialEq)]
pub struct MkNote {
    pub at: u32,
    pub name: String,
    pub mate: bool,
    pub kind: MkKind,
}

/// Multi-kill notes kept at most (the newest plus four older lines).
// EVIDENCE: cshell+0xa5fe0 / cshell+0xa75a0 (the newest line plus four shifted lines, 0xdc bytes apart)
pub const MK_LINES: usize = 5;

/// A HiSec case the frame loop placed on screen this frame: its key, screen position (virtual pixels, `None`
/// behind the camera) and countdown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaseMark {
    pub key: u32,
    pub at: Option<[f32; 2]>,
    pub remaining: i32,
}

/// The result screen opened by 0x3f, with the time it opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultScreen {
    pub flag: bool,
    pub value: u32,
    pub at: u32,
    /// The last round's result mark when 0x3f came: the header of the result board on the end camera (CLI-682).
    pub mark: Option<ResultMark>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    pub at_ms: u32,
    pub text: String,
    pub kind: ChatKind,
    /// How long the line stays (ms).
    pub life_ms: u32,
}

/// The own weapon as the ammunition block shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmmoView {
    pub weapon: u16,
    pub clip: u32,
    /// Rounds outside the clip (the 2009 block shows `total - clip`, cshell+0x6f1b0).
    pub reserve: u32,
    pub clip_size: Option<u32>,
    /// One shot per press (fire-mode image "single").
    pub single_shot: bool,
    /// The weapon's order class (2009 numbering, 0 = unknown: no weapon table): the fire-mode image is drawn for
    /// classes 1 and 2 only.
    pub order_class: u8,
    /// The row's `InfiniteAmmo` (the knife): the clip and reserve digits, the slash and the fire-mode image are hidden.
    pub infinite: bool,
}

/// Everything the HUD draws from.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HudState {
    /// CLI-1639: Gun Game's rung line (`None` in every other match).
    pub gun_game: Option<GunGameHud>,
    pub now_ms: u32,
    /// The letterbox bars' fade clock (CLI-610).
    pub letterbox: super::letterbox::LetterboxClock,
    /// The "Reality" option (off by default): the damage-direction arcs light only when it is on.
    // EVIDENCE: cshell+0x1f7d20 (the 0x74 handler adds the hit's magnitude to a sector only inside the Reality gate,
    // clean decomp lines 91-110); DC-DEFAULT: CLI-313 (proposed) off, the camera jar reads the same option
    // ([`crate::camfx`]); the driver copies it in with [`super::HudDriver::set_reality`]
    pub reality: bool,
    /// Play a voice for a drawn round (0x46 with no winner). No 2009 evidence: the module plays nothing there.
    // DC-DEFAULT: CLI-410 (staged) off; `--draw-voice` style switches set it through
    // [`super::HudDriver::set_draw_voice`]
    pub draw_voice: bool,
    /// The flag modes' state (0xc6): flag sides, own carrying, the event notice (CLI-993).
    pub ctf: super::modetop::CtfHud,
    /// The Search and Destroy state (0x51 / 0x50 / 0x4f / 0x56 / 0xad, CLI-1050..1054).
    pub bomb: super::bomb::BombHud,
    /// The Quarantine state (0xd3, CLI-1060 / CLI-1061).
    pub qrt: super::qrt::QrtHud,
    /// The Spy Hunt state (0xcb, CLI-1080 / CLI-1081).
    pub spy: super::spy::SpyHud,
    /// The Fire Team mission state (0xc9: goals and the banner, CLI-1858 / CLI-1859).
    pub mission: super::mission::MissionHud,
    pub life_state: u8,
    /// The activate entry of the door under the cross this frame ([`super::prompt`], CLI-1230).
    pub use_target: Option<super::prompt::UseEntry>,
    /// A dropped weapon the use key would take lies in reach this frame ([`super::prompt`], CLI-1352).
    pub item_target: bool,
    pub health: Option<f32>,
    /// The last health above 0 a 0x66 carried: what a living player's block shows when a health of 0 arrives
    /// without the death (CLI-1853).
    pub live_health: Option<f32>,
    pub max_health: Option<f32>,
    pub armour: Option<f32>,
    pub max_armour: Option<f32>,
    pub ammo: Option<AmmoView>,
    pub crouched: bool,
    /// The sprint gauge in percent (`None`: no gauge value, drawn full; CLI-589).
    pub stamina_pct: Option<u32>,
    pub own_pid: Option<u8>,
    /// The player the death camera follows this frame and his object (`None`: the own view or body).
    pub observed: Option<(u8, u16)>,
    /// The last health / armour notes (type 0x18 subs 0x14 / 0x15) of each character object.
    pub vitals: BTreeMap<u16, [Option<u16>; 2]>,
    /// Fire Team: each AI character's last health note (type 0x18 sub 0x24, CLI-1871).
    pub npc_health: BTreeMap<u16, super::npcbar::NpcHealth>,
    /// Fire Team: the AI health bars of this frame ([`super::npcbar::frame_bars`]).
    pub npc_bars: Vec<super::npcbar::NpcBar>,
    /// Fire Team: each player's points total from 0xd7 sub 1 (record +0x1f4), by engine client id.
    pub coop_points: BTreeMap<u32, u32>,
    /// Fire Team: the own points popup of the last 0xd7 sub 1 / 3 naming the own id (CLI-1872).
    pub points_popup: Option<super::npcbar::PointsPopup>,
    /// Fire Team: the own score shown, the drop-item notes and the kill run (CLI-1881..1883).
    pub coop: super::coopscore::CoopScoreHud,
    pub mode_class: Option<u8>,
    pub roster: BTreeMap<u8, BoardRow>,
    pub feed: VecDeque<FeedLine>,
    /// The 0xb5 pair (TDM team kills; the top counters of mode classes 1, 4, 7, 8, 10).
    pub team_pair: [u32; 2],
    /// The 0x46 round wins (the top counters of the round classes 2, 3, 12, 13).
    pub round_wins: [u32; 2],
    /// The round clock.
    pub clock: RoundClock,
    /// 0x45 `waiting_banner`: the "waiting for players" banner is up until the round result.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 step 3 (cleared at every 0x45 and by 0x46)
    pub waiting_banner: bool,
    /// The latch of the last 0x45 `waiting_banner` (`match_ui`+0x9a95): only 0x45 writes it.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 step 3, §2.1.2 (read by the 0x46 handler and cshell+0xc1a50)
    pub banner_latch: bool,
    /// The last 0x44's round number and limit (`match_ui`+0x9a58 and the half-time test's limit).
    pub round_no: u32,
    pub round_limit: u32,
    /// The half-time team-change picture of the clan classes (CLI-1164).
    pub team_change: super::teamchange::TeamChange,
    /// The last intro block (radar and map pictures) and the running intro banner.
    pub intro_block: Option<IntroBlock>,
    pub intro: Option<IntroShow>,
    /// The intro banner latch (`match_ui`+0x9a50): set when a banner starts, cleared by the round-flow ids.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2 (common side effects), §2.1.1 field c
    pub intro_latch: bool,
    /// 0x46 result mark, until the next reset (0x44, 0x45, 0x3f).
    pub result_mark: Option<ResultMark>,
    /// When the result mark was set (the board waits [`RESULT_BOARD_DELAY_MS`] after it) and the 0x46 winner.
    pub result_mark_at: Option<u32>,
    pub round_winner: Option<i8>,
    /// The kill goal of 0x7d (the top block shows it for TDM).
    pub goal: Option<u32>,
    /// Multi-kill notes, newest last.
    pub mk: VecDeque<MkNote>,
    /// The own first-kill announcement (start time).
    pub combo_at: Option<u32>,
    /// HiSec case map (key -> owner client id, 0xFFFFFFFF nobody) and this frame's case marks.
    pub case_owner: BTreeMap<u32, u32>,
    /// CRATE-2: a sub 0 drop line waits for the sub 2 that follows it, whose key says whether a Camo Crate dropped.
    pub case_drop_pending: bool,
    pub case_marks: Vec<CaseMark>,
    /// The capture flags and own capture zones on screen this frame (CLI-1073).
    pub flag_marks: Vec<super::modetop::FlagMark>,
    /// The own pickup banner (start time).
    pub case_banner_at: Option<u32>,
    /// The own character object (for the spawn notes) and its spawn-note state (`Some(since)` between 0x1f and 0x25).
    // DC-DEFAULT: CLI-82 (proposed) the spawn-note state is kept for the HUD and the renderer; no HUD element is drawn
    // for it (no 2009 element reading object +0x539 / +0x53a was found)
    pub view_object: Option<u16>,
    pub spawn_mark: Option<u32>,
    /// 0x65 rows by client id and the result screen of 0x3f.
    pub summary: BTreeMap<u8, SummaryRow>,
    pub result_screen: Option<ResultScreen>,
    /// The client texts the lines are formed from.
    pub texts: HudTexts,

    pub damage: [f32; DAMAGE_SECTORS],
    /// The fall's screen-edge band strength (1 at the hit, decays like a sector; [`super::fallborder`]).
    pub fall_border: f32,
    /// The running screen flash of 0x66 kind 0x0c ([`super::whiteout`], CLI-1926).
    pub whiteout: Option<super::whiteout::Whiteout>,
    /// The drawn weapon's flash numbers (`None`: no weapon in hand, the white is not drawn), set every frame.
    pub flash_decl: Option<super::whiteout::FlashDecl>,
    /// When the sectors last decayed (ms).
    pub damage_at: u32,
    /// Death time (0x67 state 2) and the wait the client armed for it.
    pub death_at: Option<u32>,
    pub respawn_wait_ms: Option<u32>,
    pub chat: VecDeque<LogLine>,
    /// The chat input element (open state, typed line, repeat guard; [`super::chat::ChatInput`]).
    pub chat_input: super::chat::ChatInput,
    /// The network overlay's mode (`/network`, [`super::netview`]) and its lines of the last refresh.
    pub net_mode: u8,
    pub net_lines: Vec<String>,
    /// CLI-1596: the frame-rate line of the "Show FPS" setting, top right (`None`: off).
    pub fps_line: Option<String>,
    /// The red system banner of a `GS_CHAT` from sender 0xFFFFFFFF: text and start time.
    pub chat_banner: Option<(String, u32)>,
    /// The moderator name tag of the chat colours (from the layout; `None`: no moderator colour).
    pub chat_moderator_tag: Option<String>,
    /// Players whose 0x6c said "kicked" (the next leave line of that player uses the kick form).
    pub kicked: std::collections::BTreeSet<u8>,
    /// Scoreboard key held.
    pub show_board: bool,
    /// CLI-1953: the world map is open (its key toggles it).
    pub map_open: bool,
    /// CLI-1953: the team mates' world points while the map is open.
    pub map_mates: Vec<[f32; 3]>,
    /// CLI-1953: the world rectangle `[x0, z0, x1, z1]` of the client's own world picture
    /// ([`super::worldmap::WORLD_MAP_TEX`]), when it drew one for this match.
    pub world_map: Option<[f32; 4]>,
    /// Board openings (the key went down outside the result board) not yet turned into the open sound.
    pub board_opens: u32,
    /// The leave-the-match box as drawn (`None`: closed; w2-loading, CLI-920, [`crate::loading::LeaveBox`]).
    pub leave_box: Option<crate::loading::LeaveView>,
    /// The held weapon's aim_mark (`None`: not drawn).
    pub aim_mark: Option<crate::spread::AimMarkView>,
    /// use-items-sweep: the reticle item in use at the match start: the mark's shape bits and its colour (alpha 0:
    /// the weapon's colour; CLI-1878).
    pub reticle: Option<(u16, [u8; 4])>,
    /// The held weapon hides the aim cross (CLI-616).
    pub aim_cross_hidden: bool,
    /// A round result came and the next round has not begun (0x46 until 0x44 / 0x45): no aim cross (CLI-1103).
    pub between_rounds: bool,
    /// The held weapon row's aim picture (`DotSightName`: the knife's red X; CLI-630).
    pub aim_picture: Option<String>,
    /// The scope overlay slots ([`super::scope`]).
    pub scope: super::scope::ScopeView,
    /// The last 0x74 that started the aim cross's hit flash.
    pub hit_flash_at: Option<u32>,
    /// The own death (from the 0x6d naming the local player as the victim), until the next 0x67 state 1.
    pub death: Option<DeathNote>,
    /// The revenge panel of the last own death, when the 0x6d carried a revenge block.
    pub revenge: Option<RevengePanel>,
    /// Names over the players' heads this frame ([`super::marks::frame_marks`]).
    pub marks: Vec<super::marks::PlayerMark>,
    /// CLI-1620: the x-ray boxes a declared server mod's `render.entity_visibility` hook asked for this frame
    /// (virtual 1024 x 768; empty without the hook).
    pub xray: Vec<super::items::DrawItem>,
    /// CLI-1631: other players' `(health, maximum)` by roster id, from the server's DSV-480 message (empty in every
    /// culled match).
    pub remote_health: BTreeMap<u8, (u16, u16)>,
    /// The living team mate under the aim cross ([`super::marks::mate_target`]).
    pub mate_target: Option<super::marks::MateTarget>,
    /// An own hit was confirmed since the last mark step ([`HudEvent::OwnHit`]); the next step gives the object under
    /// the cross its hit colour.
    pub own_hit: bool,
    /// When the last own hit was confirmed (ms): the clock of a mod's `hitmarker` picture (CLI-1613).
    pub own_hit_at: Option<u32>,
    /// Per object: when its mark took the hit colour (ms; [`super::marks::mark_colour`]).
    pub mark_hits: BTreeMap<u16, u32>,
    /// The last 0x74 taken while alive, for the death panel ([`super::death`]).
    pub wound: Option<super::death::WoundNote>,
    /// The radar of this frame ([`super::radar::frame`]); `None` until the frame loop gives the camera.
    pub radar: Option<super::radar::RadarFrame>,
    /// The own heartbeat sensor of this frame ([`super::sensor`]); `None`: none carried.
    pub sensor: Option<super::sensor::SensorView>,
    /// CLI-1952: the team's live pings of this frame (empty without an offer).
    pub pings: super::teamping::PingView,
    /// CLI-1952: the place markers on the screen this frame ([`super::teamping::marker_place`]).
    pub ping_marks: Vec<super::teamping::PingMark>,
    /// The own kill banner and its pending voice lines ([`super::banners`]).
    pub banner: super::banners::BannerState,
    /// The low-health pulse and heartbeat clock ([`super::vitals`]).
    pub low_health: super::vitals::LowHealthClock,
    /// The weapon panel of the last local weapon change ([`super::panel`]).
    pub weapon_panel: Option<super::panel::PanelShow>,
    /// Each roster player's rank (0x6b field 17b) for the rank icon of the world mark ([`super::rank`]).
    pub ranks: BTreeMap<u8, u32>,
    /// w2-board: each roster player's level index, name colour, clan and mission values (0x6b) for the board rows
    /// and the death panel ([`super::board::RowMark`]).
    pub row_marks: BTreeMap<u8, super::board::RowMark>,
}

/// The damage-indicator sector of a hit: the direction toward the attacker (`-from_dir`) in the view's
/// horizontal frame, angle 0 straight ahead and positive to the right, mapped to `trunc((angle + pi) * 6 / pi)`.
// DISASM: cshell+0x1f7f2e..0x1f7f3e (fpatan of two dot products, + pi, * 1.90986, ftol)
// DC-DEFAULT: CLI-48 (proposed) which view axes the two dot products use is not read; ours: right and forward of the
// view yaw, with the direction toward the attacker
// DC-DEFAULT: CLI-1125 (proposed) a straight up or down direction (a fall's `from_dir` is up x the loss / 100) takes
// the sector of angle 0: the 2009 angle is fpatan of the direction's dot products with two camera axes, both 0 for a
// vertical direction and a level view, and fpatan(0, 0) is 0 (the 2009 update adds the length to that sector whenever
// the length is above 0, cshell+0x1f7d20); our axes are horizontal, so the view's pitch does not tilt it
pub fn damage_sector(from_dir: [f32; 3], view_yaw: f32) -> Option<usize> {
    let (x, z) = (-from_dir[0], -from_dir[2]);
    if !(x.is_finite() && z.is_finite() && from_dir[1].is_finite()) {
        return None;
    }
    if x.abs() + z.abs() < 1e-6 {
        return (from_dir[1].abs() > 0.0).then_some(DAMAGE_SECTORS / 2);
    }
    // dc-sim / dc-render convention: yaw 0 looks along +Z, positive yaw turns toward +X
    let (s, c) = view_yaw.sin_cos();
    let forward = x * s + z * c;
    let right = x * c - z * s;
    let angle = right.atan2(forward);
    let k = ((angle + std::f32::consts::PI) * (6.0 / std::f32::consts::PI)) as usize;
    Some(k.min(DAMAGE_SECTORS - 1))
}

/// Centre angle of a sector (radians, 0 ahead, positive right), the inverse of [`damage_sector`].
pub fn sector_angle(k: usize) -> f32 {
    (k as f32 + 0.5) * (std::f32::consts::PI / 6.0) - std::f32::consts::PI
}

impl HudState {
    /// Advances the clock-driven parts (damage decay, feed and chat expiry) to `now`.
    pub fn tick(&mut self, now: u32, feed_lifetime_ms: u32) {
        let dt = now.saturating_sub(self.damage_at) as f32 / 1000.0;
        self.damage_at = now;
        for v in &mut self.damage {
            *v = (*v - dt * DAMAGE_DECAY_PER_S).max(0.0);
        }
        self.fall_border = (self.fall_border - dt * DAMAGE_DECAY_PER_S).max(0.0);
        self.now_ms = now;
        // CLI-1926: the screen flash ends at its time, or for a fading one when its white is gone, and at the own
        // death
        // EVIDENCE: cshell+0x485fc..0x48611 (the own player's state, local player +0x30, other than 1 ends it unless
        // local player +0x428 is 3)
        // DC-DEFAULT: CLI-1926 (proposed) the death states 2, 3, 5, 6 end it (+0x428 is not read; a HUD that has no
        // state yet keeps the flash)
        let decl = self.flash_decl.unwrap_or_default();
        let dead = matches!(self.life_state, 2 | 3 | 5 | 6);
        if self
            .whiteout
            .is_some_and(|w| dead || w.level(now, decl).is_none())
        {
            self.whiteout = None;
        }
        // CLI-1051: the bomb timer's beeps, the gauge's end
        self.tick_bomb(now);
        self.tick_qrt(now);
        while self
            .feed
            .front()
            .is_some_and(|l| now.saturating_sub(l.at_ms) >= feed_lifetime_ms)
        {
            self.feed.pop_front();
        }
        // each line has its own life (notices 5000 ms, chat lines 10000 ms)
        self.chat
            .retain(|l| now.saturating_sub(l.at_ms) < l.life_ms);
        if self
            .chat_banner
            .as_ref()
            .is_some_and(|(_, at)| now.saturating_sub(*at) >= super::chat::BANNER_MS)
        {
            self.chat_banner = None;
        }
    }

    /// Drops the multi-kill notes older than `lifetime_ms` (the layout's note time).
    pub fn expire_notes(&mut self, now: u32, lifetime_ms: u32) {
        while self
            .mk
            .front()
            .is_some_and(|n| now.saturating_sub(n.at) >= lifetime_ms)
        {
            self.mk.pop_front();
        }
    }

    /// 0x66 `[kind][b1][b2][value]`: 2 health, 3 armour, 0x0f / 0x10 the maxima.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (cshell+0x5cb80)
    pub fn on_status(&mut self, kind: u8, value: f32) {
        match kind {
            2 => {
                self.health = Some(value);
                if value > 0.0 && value.is_finite() {
                    self.live_health = Some(value);
                }
            }
            3 => self.armour = Some(value),
            0x0f => self.max_health = Some(value),
            0x10 => self.max_armour = Some(value),
            _ => {}
        }
    }

    /// The health the block shows: the 2009 whole number (the fraction dropped), but a living player never shows 0.
    /// Above 0 and below 1 shows 1; a health of 0 (or none, or not a number) while the own state is alive (1) shows
    /// the last live value, since the death states follow the lethal 0x66 in the same block (cshell+0x5cb80 drops
    /// the fraction with cshell+0x3a5f00, so the 2009 block showed 0 for any health below 1).
    // EVIDENCE: cshell+0x5cb80 (kind 2: the f32 through cshell+0x3a5f00, a truncating float-to-int, into the stats),
    // cshell+0x967a0 (the refresh reads the whole number, clamps it at 0)
    // DC-DEFAULT: CLI-1853 (proposed; user report 2026-11-04 "my HP says 0 but I'm still alive": a blast left 0.348)
    pub fn shown_health(&self) -> f32 {
        let h = self
            .health
            .filter(|h| h.is_finite())
            .unwrap_or(0.0)
            .max(0.0);
        let h = if h <= 0.0 && self.life_state == 1 {
            self.live_health.unwrap_or(0.0)
        } else {
            h
        };
        if h > 0.0 && h < 1.0 {
            1.0
        } else {
            h
        }
    }

    /// 0x67: state 2 stores the death time and arms the own wait; state 1 clears it. State 2 also clears the
    /// damage sectors (cshell+0x1f0bc0 is the base handler's 12-word clear).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.1 (state 2: 12 words at +0x15c cleared, death time), §4.1.1
    // SPEC: GAME_MESSAGES_2009_C2S.md §3.3 (wait by mode class)
    pub fn on_life(&mut self, state: u8, now: u32) {
        self.life_state = state;
        match state {
            1 => {
                self.death_at = None;
                self.respawn_wait_ms = None;
                self.death = None;
                self.revenge = None;
                self.hit_flash_at = None;
            }
            2 => {
                self.death_at = Some(now);
                self.respawn_wait_ms = self
                    .mode_class
                    .and_then(|c| dc_botclient::lifecycle::self_arm_wait(c, 0));
                self.damage = [0.0; DAMAGE_SECTORS];
                self.fall_border = 0.0;
            }
            _ => {}
        }
    }

    /// 0x74: ignored while dead (states 2, 3, 5, 6); else the sector of the hit gains the magnitude of
    /// `from_dir`, capped at 1.0.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 row 0x74, §4.3.1 (magnitude = health loss / 100)
    pub fn on_hit(&mut self, from_dir: [f32; 3], view_yaw: f32) {
        // EVIDENCE: cshell+0x1f7d20 (clean decomp lines 91-92: the whole update sits inside the Reality gate)
        if !self.reality || matches!(self.life_state, 2 | 3 | 5 | 6) {
            return;
        }
        let m = (from_dir[0] * from_dir[0] + from_dir[1] * from_dir[1] + from_dir[2] * from_dir[2])
            .sqrt();
        if m.is_nan() || m <= 0.0 {
            return;
        }
        // DC-DEFAULT: CLI-1847 (proposed; USER DECISION 2026-11-04, amends CLI-1846 and CLI-1125) a vertical hit (a
        // fall) has no bearing and its length is only the health loss / 100 (0.028 for 3 HP: invisible), and the
        // ring of arcs about the crosshair is too small for it: it starts the red band along the screen edges at
        // full strength whatever the damage ([`super::fallborder`]), decaying like a sector; no sector lights
        if crate::camfx::is_vertical(from_dir) {
            self.fall_border = 1.0;
        } else if let Some(k) = damage_sector(from_dir, view_yaw) {
            self.damage[k] = (self.damage[k] + m).min(1.0);
        }
    }

    /// 0x74 `damage` / `zone` while alive: the aim cross's hit flash starts.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 row 0x74 (ignored in states 2, 3, 5, 6; cshell+0x5cd20 calls the flash
    // cshell+0x835d0 when `damage` > 0 and `zone` != 0x25)
    pub fn on_hit_flash(&mut self, damage: u32, zone: u8, now: u32) {
        if matches!(self.life_state, 2 | 3 | 5 | 6) || damage == 0 || zone == NO_FLASH_ZONE {
            return;
        }
        self.hit_flash_at = Some(now);
    }

    /// The screen flash's white level at the HUD clock (0: none, or no weapon in hand to read the numbers from).
    // EVIDENCE: cshell+0x485b4..0x485c1 (no drawn weapon object: the frame draws nothing, the flash runs on)
    pub fn white_level(&self) -> f32 {
        match (self.whiteout, self.flash_decl) {
            (Some(w), Some(d)) => w.level(self.now_ms, d).unwrap_or(0.0),
            _ => 0.0,
        }
    }

    /// The hit flash's strength at `now`: 1 at the hit, falling to 0 over [`HIT_FLASH_MS`].
    pub fn hit_flash(&self) -> f32 {
        match self.hit_flash_at {
            Some(t) if self.now_ms.saturating_sub(t) < HIT_FLASH_MS => {
                1.0 - self.now_ms.saturating_sub(t) as f32 / HIT_FLASH_MS as f32
            }
            _ => 0.0,
        }
    }

    /// The revenge panel while it is shown (until [`REVENGE_MS`] after the death or the respawn).
    pub fn revenge_shown(&self) -> Option<&RevengePanel> {
        self.revenge
            .as_ref()
            .filter(|r| self.now_ms.saturating_sub(r.at) < REVENGE_MS)
    }

    fn own_team(&self) -> Option<u8> {
        self.own_pid
            .and_then(|p| self.roster.get(&p))
            .map(|r| r.team)
    }

    fn relation(&self, pid: u8) -> Relation {
        let team = self.roster.get(&pid).map(|r| r.team);
        match (team, self.own_team()) {
            (Some(t), Some(o)) if t == o && t != 0 && t != 0xff => Relation::Mate,
            _ if Some(pid) == self.own_pid => Relation::Mate,
            _ => Relation::Opponent,
        }
    }

    /// The marks of a 0x6d line besides the head-shot icon.
    // EVIDENCE: cshell+0x2db9e (zone 5 = the groin flag), cshell+0x2e21e..0x2e245 (flag mark: relation bit and a known
    // victim record; 1 when the victim's team equals the own team), cshell+0x9d4a0 (forms by feed value and killer name)
    // DC-DEFAULT: CLI-281 (proposed) "victim record known" = the victim is in the HUD roster or is the own player
    fn feed_marks(&self, k: &KillRecord) -> super::feedmarks::FeedMarks {
        let victim_team = self.roster.get(&k.victim).map(|r| r.team).or_else(|| {
            (Some(k.victim) == self.own_pid)
                .then(|| self.own_team())
                .flatten()
        });
        super::feedmarks::FeedMarks {
            form: super::feedmarks::FeedMarks::form_of(k.feed_value, k.killer != NO_KILLER),
            payback: k.revenge_done,
            groin: k.hit_zone == 5,
            flag_victim_mate: if k.relation {
                victim_team.map(|t| Some(t) == self.own_team())
            } else {
                None
            },
            bounty: k.feed6,
            freeze: k.feed2,
        }
    }

    pub(super) fn name(&self, pid: u8) -> String {
        self.roster
            .get(&pid)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| format!("#{pid}"))
    }

    /// CLI-1639: the own record's kill column is the rung in Gun Game (DSV-481): a change makes the centre line.
    fn gun_game_on_own_record(&mut self, kills: i32, now: u32) {
        let Some(g) = self.gun_game.as_mut() else {
            return;
        };
        let rung = kills.max(0) as usize;
        if rung > g.rung {
            if let Some(n) = g.names.get(rung) {
                g.line = Some((format!("Next gun: {n}"), false, now));
            }
        } else if rung < g.rung {
            if let Some(n) = g.names.get(rung) {
                g.line = Some((format!("Knifed! Back to {n}"), true, now));
            }
        }
        g.rung = rung;
    }

    /// 0x6b: the record's absolute kills / deaths and team.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 (fields 1, 2, 7, 8, 12)
    pub fn on_roster(&mut self, r: &RosterRecord) {
        let row = self.roster.entry(r.pid).or_default();
        row.pid = r.pid;
        row.name = r.name.clone();
        row.team = r.team;
        row.kills = r.kills as i32;
        row.deaths = r.deaths as i32;
        if Some(r.pid) == self.own_pid {
            let now = self.now_ms;
            self.gun_game_on_own_record(r.kills as i32, now);
        }
        let row = self.roster.entry(r.pid).or_default();
        // the record's dead state (flag_b) replaces the board's and the marker's: the respawn refresh clears the
        // grey look a 0x6d gave
        // EVIDENCE: cshell+0x8a760 (a marker entry of the same client id takes the passed flag), DISASM
        // cshell+0x2ce9c (the 0x6b handler passes it on every record with a team); PLAUSIBLE: the passed value is
        // flag_b, see `RosterRecord::dead`
        if matches!(r.kind, 0..=2) {
            row.dead = r.dead;
        }
        // client-hudgaps: the rank the world mark's icon shows
        self.ranks.insert(r.pid, r.rank);
        // w2-board: the clan, name colour, level index and mission values of the board row and the death panel
        self.row_marks
            .insert(r.pid, super::board::RowMark::of_record(r));
    }

    /// 0x6c: the record goes, unless the result screen 0x3f already came: then the row stays for the result.
    // DISASM: cshell+0x2d4d5..0x2d733 (0x6c deletes the record, cshell+0x2bc70), cshell+0x1a0470 (its gate tests
    // the game-start request type, which 0x3f does not change), cshell+0x137dd0 / 0x137720 (the result rows are
    // filled from the live record list when the result screen takes the focus)
    // DC-DEFAULT: CLI-380 (proposed) a player who leaves after 0x3f keeps the result row (the 2009 client drops it
    // when the 0x6c comes before its own leave; DeCombat's bots leave 3 s after 0x3f, the client about 8 s)
    pub fn on_left(&mut self, pid: u8) {
        if self.result_screen.is_some() {
            return;
        }
        self.roster.remove(&pid);
    }

    /// 0x6d: victim dead, the client's own counting, and one kill-feed line.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 effects 2, 6 and 8 (the client keeps its own kill and death counts)
    pub fn on_kill(&mut self, k: &KillRecord, now: u32) {
        let killer_known = k.killer != NO_KILLER && self.roster.contains_key(&k.killer);
        if !k.no_count {
            let teams = (
                self.roster.get(&k.killer).map(|r| r.team),
                self.roster.get(&k.victim).map(|r| r.team),
            );
            if killer_known && k.killer != k.victim {
                if teams.0 != teams.1 {
                    // CLI-1639: in Gun Game the kill column is the server's rung (0x6b), no own count
                    // CLI-1850: the own kills still count for the kill banner's number (the rung is not a count)
                    if let Some(g) = self.gun_game.as_mut() {
                        if Some(k.killer) == self.own_pid {
                            g.kills += 1;
                        }
                    } else if let Some(r) = self.roster.get_mut(&k.killer) {
                        r.kills += 1;
                    }
                    if let Some(r) = self.roster.get_mut(&k.victim) {
                        r.deaths += 1;
                    }
                }
            } else if let Some(r) = self.roster.get_mut(&k.victim) {
                r.deaths += 1;
            }
        }
        if let Some(r) = self.roster.get_mut(&k.victim) {
            r.dead = true;
        }
        self.on_qrt_kill(k.killer, k.victim, now);
        // first-kill notes: every client notes the killer; the killer also gets the big announcement
        // EVIDENCE: cshell+0xa5fe0 (second argument = `streak_flag`; the own-name compare picks the announcement)
        if k.streak_flag != 0 && killer_known {
            self.push_mk(MkNote {
                at: now,
                name: self.name(k.killer),
                mate: self.relation(k.killer) == Relation::Mate,
                kind: MkKind::Opener,
            });
            if Some(k.killer) == self.own_pid {
                self.combo_at = Some(now);
            }
        }
        // the own death: who killed the local player (0xff and a self kill name nobody, S2C 3.3.1 effect 3), and the
        // revenge panel when the message has a revenge block (effect: `hud_revenge`, the local victim only)
        if self.own_pid == Some(k.victim) {
            let killer =
                (k.killer != NO_KILLER && k.killer != k.victim).then(|| self.name(k.killer));
            if let (Some(n), Some(count)) = (&killer, k.revenge) {
                self.revenge = Some(RevengePanel {
                    at: now,
                    killer: n.clone(),
                    count,
                });
            }
            self.death = Some(DeathNote {
                at: now,
                killer,
                killer_rel: self.relation(k.killer),
                weapon: k.weapon,
                head_shot: k.hit_zone == 1,
            });
        }
        // the co-op class has no kill feed (its goal list sits where the feed would be)
        // EVIDENCE: cshell+0x9d4a0 (the line builder returns at once in class 5)
        if self.mode_class == Some(5) {
            return;
        }
        let line = FeedLine {
            at_ms: now,
            killer: if k.killer == NO_KILLER {
                String::new()
            } else {
                self.name(k.killer)
            },
            victim: self.name(k.victim),
            killer_rel: self.relation(k.killer),
            victim_rel: self.relation(k.victim),
            weapon: k.weapon,
            head_shot: k.hit_zone == 1,
            marks: self.feed_marks(k),
        };
        self.feed.push_back(line);
        while self.feed.len() > FEED_MAX_LINES {
            self.feed.pop_front();
        }
    }

    /// 0x67 state 1 of the own player and every 0x6b refresh make a player alive again on the board.
    pub fn mark_alive(&mut self, pid: u8) {
        if let Some(r) = self.roster.get_mut(&pid) {
            r.dead = false;
        }
    }

    /// 0xb5 `[team 1][team 2]`.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.4 row 0xb5 (cshell+0x92bb0 stores the pair for the top counters)
    pub fn on_team_pair(&mut self, p: [u8; 2]) {
        self.team_pair = [p[0] as u32, p[1] as u32];
    }

    /// 0x46: the round wins.
    pub fn on_round_wins(&mut self, w: [u8; 2]) {
        self.round_wins = [w[0] as u32, w[1] as u32];
    }

    /// Sets the round clock (`hud_round_timer` set, cshell+0xbe0c0) and its shown flag.
    pub fn set_clock(&mut self, secs: u32, now: u32, running: bool, shown: bool) {
        self.clock = RoundClock {
            shown,
            secs,
            at: now,
            running,
        };
    }

    /// The round-flow reset shared by 0x3f, 0x44, 0x46, 0x47, 0x48: the intro latch is cleared.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2 (common side effects: `match_ui`+0x9a50 cleared)
    pub fn round_flow(&mut self) {
        self.intro_latch = false;
    }

    /// 0x44: the clock shown and running with `secs` (also with 0: it then reads 0:00), the kill feed cleared,
    /// the result mark reset.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 row 0x44 (`hud_round_timer` shown and started; `hud_kill_feed` cleared;
    // result banners cleared); DISASM cshell+0x49c3d (shown flag := 1 before cshell+0xbe0c0(secs, colour, 1, 1))
    pub fn on_round_begin(&mut self, secs: u32, now: u32) {
        self.round_flow();
        self.between_rounds = false;
        self.feed.clear();
        self.result_mark = None;
        self.set_clock(secs, now, true, true);
        // CLI-1051: a new round has no bomb timer, gauge or carrier mark
        self.bomb_round_reset();
        // CLI-991: a new round makes every board row alive again (the round modes respawn everybody)
        // EVIDENCE: cshell+0x49a40 case 0x44 (unconditional calls cshell+0x93120 / 0xc3150 / 0x7d950, the
        // resets of the dead flags that 0x6d sets through cshell+0x93970 / 0xc3110 / 0x7d910)
        for r in self.roster.values_mut() {
            r.dead = false;
        }
    }

    /// 0x45: clock shown with `round_secs` (hidden at 0), the waiting banner, the intro banner when the block has
    /// a text image and the latch is clear, the result mark reset.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 steps 1, 3, 5 (field c: shown when non-empty and the latch is clear),
    // step 9 (result banners cleared)
    // EVIDENCE: cshell+0x49f00..0x49f7b (own state 1: the first-spawn voice when the waiting flag is set, else the
    // round-start voice; any other state plays nothing; the voice has no latch)
    pub fn on_match_begin(&mut self, m: &MatchBegin, now: u32) {
        if self.life_state == 1 {
            self.banner.voices.push(if m.waiting_banner {
                Voice::FirstSpawn
            } else {
                Voice::RoundStart
            });
        }
        self.set_clock(m.round_secs, now, true, m.round_secs != 0);
        self.between_rounds = false;
        self.waiting_banner = m.waiting_banner;
        self.banner_latch = m.waiting_banner;
        self.result_mark = None;
        if let Some(b) = &m.intro {
            if !b.body.is_empty() && !self.intro_latch {
                self.intro = Some(IntroShow {
                    head: b.head.clone(),
                    body: b.body.clone(),
                    at: now,
                });
                self.intro_latch = true;
            }
            self.intro_block = Some(b.clone());
        }
    }

    /// 0x46: clock set to `round_secs` and hidden, the waiting banner down, the round wins, the result mark.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.2 (timer set then hidden; `hud_side_switch`+0x6a := 0; wins; the result
    // banner cshell+0xc1a50(1, win_team) for mode classes other than 5)
    pub fn on_round_result(&mut self, round_secs: u32, win_team: i8, wins: [u8; 2], now: u32) {
        self.round_flow();
        let latch = self.banner_latch;
        self.set_clock(round_secs, now, true, false);
        self.waiting_banner = false;
        self.on_round_wins(wins);
        self.round_winner = Some(win_team);
        // CLI-1164 the half-time picture's test, while the latch of the last 0x45 is clear (cshell+0x49a40 case 0x46)
        if !latch {
            let (limit, no) = (self.round_limit, self.round_no);
            self.team_change.on_round_result(
                self.mode_class,
                limit,
                [wins[0] as u32, wins[1] as u32],
                no,
                now,
            );
        }
        self.result_mark_at = Some(now);
        self.between_rounds = true;
        // CLI-1243: the bomb timer and its beeps stop with the round
        self.bomb_round_end();
        if let Some(v) = self.round_win_voice(win_team) {
            self.banner.voices.push(v);
        }
        if self.mode_class != Some(5) {
            // EVIDENCE: cshell+0xc1a50 (class 11 with a non-zero win value: the humans' picture for 1, the
            // infectors' for any other, whatever the own team)
            let qrt = self.mode_class == Some(11);
            self.result_mark = match (win_team, self.own_team()) {
                (0, _) if !latch => Some(ResultMark::Draw),
                (0, _) => Some(ResultMark::Score),
                (1, _) if qrt => Some(ResultMark::HumanWin),
                (_, _) if qrt => Some(ResultMark::InfectorWin),
                (w, Some(t)) if w as i32 == t as i32 => Some(ResultMark::Win),
                (_, Some(_)) => Some(ResultMark::Lose),
                (_, None) => None,
            };
        }
    }

    /// The announcer line of a round result: a team number plays that team's win line (1 the first team's, any other
    /// number the second's); class 11 plays the human / infector pair; classes 5, 6 and 9 and win team 0 play none.
    // EVIDENCE: cshell+0xc1a50 (the sound is picked in the branch for a non-zero team; class 11: the human /
    // infector pair by team 1 or not; classes 5, 6, 9 skip it; the key goes through cshell+0x1b2c00 and cshell+0x1b20c0, a local sound)
    // DC-DEFAULT: CLI-410 (staged) win team 0 plays the draw line only when `draw_voice` is on
    fn round_win_voice(&self, win_team: i8) -> Option<Voice> {
        if win_team == 0 {
            return self.draw_voice.then_some(Voice::RoundDraw);
        }
        let first = win_team == 1;
        match self.mode_class {
            Some(5 | 6 | 9) => None,
            Some(11) => Some(if first {
                Voice::WinHuman
            } else {
                Voice::WinInfector
            }),
            _ => Some(if first {
                Voice::WinAlpha
            } else {
                Voice::WinBravo
            }),
        }
    }

    /// The result board (board window + result mark) is drawn: a mark is set and [`RESULT_BOARD_DELAY_MS`] passed.
    // EVIDENCE: cshell+0xc02e0 (3000 < now - result time)
    pub fn result_board_shown(&self) -> bool {
        self.result_mark.is_some()
            && self
                .result_mark_at
                .is_some_and(|t| self.now_ms.saturating_sub(t) > RESULT_BOARD_DELAY_MS)
    }

    /// Within [`RESULT_BOARD_DELAY_MS`] of the result mark: the board draws nothing (the key does not show it either).
    // EVIDENCE: cshell+0xc02e0 (with the result flag set the draw returns until the 3000 ms passed)
    pub fn result_board_held(&self) -> bool {
        self.result_mark.is_some() && !self.result_board_shown()
    }

    /// 0x48 (and 0x41): the clock set to `secs` and hidden.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 rows 0x48 (set, running, hidden) and 0x41 (set, hidden)
    pub fn on_round_close(&mut self, secs: u32, now: u32) {
        self.round_flow();
        self.set_clock(secs, now, true, false);
        self.bomb_round_end();
    }

    /// 0xd7 sub 4: the clock shown, set to `secs`, running or frozen.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.5 row 0xd7 (sub 4: shown, set, frozen when `running` = 0)
    pub fn on_coop_clock(&mut self, running: bool, secs: u32, now: u32) {
        self.set_clock(secs, now, running, true);
    }

    /// 0x3f: timers hidden, the result mark reset, the result screen opened.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 row 0x3f (timers hidden; banner reset; screen 7)
    pub fn on_result_screen(&mut self, flag: bool, value: u32, now: u32) {
        self.round_flow();
        self.clock.shown = false;
        self.waiting_banner = false;
        self.bomb_round_end();
        // DC-DEFAULT: CLI-682 (proposed; extends CLI-80) the mark 0x3f resets stays the header of our result board
        // on the end camera (2009 drew no HUD after 0x3f, only the result page, which shows the result)
        let mark = self.result_mark.take();
        self.result_screen = Some(ResultScreen {
            flag,
            value,
            at: now,
            mark,
        });
    }

    /// Seconds left on the round clock while it is shown.
    pub fn clock_left(&self) -> Option<u32> {
        self.clock
            .shown
            .then(|| self.clock.ms_left(self.now_ms) / 1000)
    }

    pub(super) fn push_mk(&mut self, n: MkNote) {
        self.mk.push_back(n);
        while self.mk.len() > MK_LINES {
            self.mk.pop_front();
        }
    }

    /// 0xd6: the HiSec case map, its chat lines and notes, the own pickup banner.
    // SPEC: HISEC_2009.md §3.2 (sub 0 line 0x18c1; sub 1 only for a player in the roster: line 0x18c2 with the
    // name, a multi-kill note, entry removed, own player: pickup banner; sub 2 insert / update the owner)
    pub fn on_hisec(&mut self, e: CaseEvent, now: u32) {
        let camo = |key: u32| dc_bute::camo_crate::is_camo_key(key);
        // CRATE-2: the drop line names the case once its key is known (the server sends sub 2 right after sub 0)
        let drop_line = |h: &mut Self, crate_drop: bool| {
            if std::mem::take(&mut h.case_drop_pending) {
                let t = if crate_drop {
                    CAMO_DROP_TEXT.to_string()
                } else {
                    h.texts.case_drop.clone()
                };
                h.line(t, ChatKind::Hisec, now);
            }
        };
        match e {
            CaseEvent::Dropped => {
                drop_line(self, false);
                self.case_drop_pending = true;
            }
            CaseEvent::Taken { key, player } => {
                drop_line(self, false);
                let Some(pid) = u8::try_from(player)
                    .ok()
                    .filter(|p| self.roster.contains_key(p))
                else {
                    return;
                };
                let name = self.name(pid);
                let form = if camo(key) {
                    CAMO_TAKEN_TEXT.to_string()
                } else {
                    self.texts.case_taken.clone()
                };
                let t = fill_name(&form, &name);
                self.line(t, ChatKind::Hisec, now);
                // the note's picture names a HiSec case: none for a Camo Crate
                if !camo(key) {
                    self.push_mk(MkNote {
                        at: now,
                        name,
                        mate: self.relation(pid) == Relation::Mate,
                        kind: MkKind::Hisec,
                    });
                }
                // EVIDENCE: cshell+0x2e6e7..0x2e7fb: first cshell+0x9a3e0 (removes the case entry, the picked-up
                // sound only when one existed), then cshell+0x993e0 whenever the player is the own client (the
                // picked-up-by-me sound, no entry check); cshell+0x98d50 maps the kinds to keys
                if self.case_owner.remove(&key).is_some() {
                    self.banner.voices.push(Voice::CasePicked);
                }
                if Some(pid) == self.own_pid {
                    self.banner.voices.push(Voice::CaseByMe);
                    self.case_banner_at = Some(now);
                }
            }
            CaseEvent::Owner { key, owner } => {
                drop_line(self, camo(key));
                // EVIDENCE: cshell+0x2e6c1..0x2e6e2 (owner -1: the timer-running-out sound, any other owner: the
                // drop sound); PROVEN cshell+0x9a620 (the map insert / update)
                self.banner.voices.push(if owner == HISEC_NOBODY {
                    Voice::CaseTimer
                } else {
                    Voice::CaseDrop
                });
                self.case_owner.insert(key, owner);
            }
        }
    }

    /// Whether the indicator of case `key` is drawn for this client: no entry, nobody, or the own id.
    // SPEC: HISEC_2009.md §3.3 step 2 (another owner: the case is hidden and no indicator drawn)
    pub fn case_visible(&self, key: u32) -> bool {
        match self.case_owner.get(&key) {
            None => true,
            Some(&o) => o == HISEC_NOBODY || Some(o) == self.own_pid.map(u32::from),
        }
    }

    /// A note to a client object: the spawn notes of the own character.
    // SPEC: GAME_MESSAGES_2009_S2C.md §8.3 type 0x18 subs 0x1f / 0x25 (PLAUSIBLY a spawn-protection window)
    pub fn on_note(&mut self, n: ObjectNote, now: u32) {
        match n {
            ObjectNote::SpawnMark { obj, on } => {
                if Some(obj) == self.view_object {
                    self.spawn_mark = on.then_some(now);
                }
            }
            ObjectNote::Vitals { obj, armour, value } => {
                self.vitals.entry(obj).or_default()[usize::from(armour)] = Some(value);
            }
            ObjectNote::NpcHealth { obj, percent } => {
                super::npcbar::on_health(&mut self.npc_health, obj, percent, now);
            }
            _ => {}
        }
    }

    /// The respawn wait: (ms left, fraction elapsed 0..1) while dead with an armed wait.
    pub fn respawn_left(&self) -> Option<(u32, f32)> {
        let (at, wait) = (self.death_at?, self.respawn_wait_ms?);
        if !matches!(self.life_state, 2 | 3) || wait == 0 {
            return None;
        }
        let gone = self.now_ms.saturating_sub(at).min(wait);
        Some((wait - gone, gone as f32 / wait as f32))
    }

    /// S2C 0x68: a chat-log line in its class colour, the red banner for the system sender, nothing for a sender
    /// without a roster record.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68
    pub fn on_chat(&mut self, m: &super::chat::GsChat, now: u32) {
        let sender_team = u8::try_from(m.sender)
            .ok()
            .and_then(|p| self.roster.get(&p))
            .map(|r| r.team);
        let tag = self.chat_moderator_tag.clone();
        match super::chat::classify(
            m,
            self.own_pid,
            self.own_team(),
            sender_team,
            tag.as_deref(),
        ) {
            Some(super::chat::ChatShow::Banner(t)) => self.chat_banner = Some((t, now)),
            Some(super::chat::ChatShow::Line {
                class,
                text,
                own,
                whisper,
            }) => self.line_for(
                super::chat::team_tagged(&self.texts.team_chat, m.team_only, text),
                ChatKind::Chat {
                    class,
                    whisper,
                    own,
                },
                now,
                super::chat::CHAT_LINE_MS,
            ),
            None => {}
        }
    }

    /// The chat input's repeat refusal line.
    // EVIDENCE: cshell+0x72df0 (CRes 0x1028, 5000 ms)
    pub fn chat_refused(&mut self, now: u32) {
        let t = self.texts.same_line.clone();
        if !t.is_empty() {
            self.line_for(t, ChatKind::SameLine, now, super::chat::REPEAT_NOTICE_MS);
        }
    }

    /// CLI-1031: the sensitivity hotkeys' line for 5000 ms.
    // EVIDENCE: cshell+0x49060 (CRes 0x4f6 formatted with the value, shown through the chat-log helper for 5000 ms)
    pub fn sensitivity_changed(&mut self, v: i32, now: u32) {
        let t = self
            .texts
            .sensitivity
            .replace("%1!d!", &v.to_string())
            .replace("%1", &v.to_string());
        if !t.is_empty() {
            self.line_for(t, ChatKind::Notice, now, SENSITIVITY_LINE_MS);
        }
    }

    /// A chat-log notice (join / leave lines formed from the client's own text table).
    pub fn notice(&mut self, text: String, now: u32) {
        self.line(text, ChatKind::Notice, now);
    }

    /// A chat-log line of `kind` for the notice time.
    pub fn line(&mut self, text: String, kind: ChatKind, now: u32) {
        self.line_for(text, kind, now, CHAT_NOTICE_MS);
    }

    /// A chat-log line of `kind` that stays `life_ms`.
    pub fn line_for(&mut self, text: String, kind: ChatKind, now: u32, life_ms: u32) {
        self.chat.push_back(LogLine {
            at_ms: now,
            text,
            kind,
            life_ms,
        });
        while self.chat.len() > CHAT_MAX_LINES {
            self.chat.pop_front();
        }
    }
}

/// Fills a client text with a name: a printf `%s` form (the HiSec line is formed with swprintf), else a
/// FormatMessage `%1` form.
// DISASM: cshell+0x2e766..0x2e77c (res 0x18c2 and the name through the CRT swprintf import)
pub fn fill_name(form: &str, name: &str) -> String {
    if form.contains("%s") {
        form.replacen("%s", name, 1)
    } else {
        dc_ui_data::strings::format_message(form, &[dc_ui_data::FmtArg::Str(name)])
    }
}

/// A HUD input in arrival order (the network side queues these; the frame loop applies them with its view).
#[derive(Debug, Clone, PartialEq)]
pub enum HudEvent {
    /// 0x66 value kinds (2, 3, 0x0f, 0x10).
    Status { kind: u8, value: f32 },
    /// 0x67.
    Life(u8),
    /// 0x6d.
    Kill(KillRecord),
    /// 0x6b record (new or changed).
    Roster(RosterRecord),
    /// A roster record appeared while in the world (a join notice).
    Joined(String),
    /// 0x6c: the record left (pid, name).
    Left(u8, String),
    /// 0x6c with `kicked` non-zero (comes before the [`HudEvent::Left`] of the same player).
    Kicked(u8),
    /// 0xa2: `(pid, ping ms)` pairs for the board's ping column (CLI-1360).
    Pings(Vec<(u16, u16)>),
    /// DSV-480 (our shell 0xf1): other players' health in a match with wallhacks (CLI-1631).
    RemoteHealth(Vec<dc_wire::remotehealth::HealthEntry>),
    /// S2C 0x68 `GS_CHAT`.
    Chat(super::chat::GsChat),
    /// 0xb5.
    TeamPair([u8; 2]),
    /// 0x46.
    RoundWins([u8; 2]),
    /// 0x44 (round seconds).
    RoundBegin(u32),
    /// 0x44's round number and round limit.
    RoundNumbers { round_no: u32, round_limit: u32 },
    /// 0x45 whole.
    MatchBegin(Box<MatchBegin>),
    /// 0x46 head.
    RoundResult {
        round_secs: u32,
        win_team: i8,
        wins: [u8; 2],
    },
    /// 0x48 or 0x41 (seconds; the clock is hidden).
    ClockStop(u32),
    /// 0xd7 sub 4.
    CoopClock { running: bool, secs: u32 },
    /// 0xd7 sub 1 / 3 (`record`: sub 1).
    CoopPoints {
        pid: u32,
        gained: u32,
        total: u32,
        record: bool,
    },
    /// 0x3f.
    ResultScreen { flag: bool, value: u32 },
    /// 0x65 row.
    Summary(SummaryRow),
    /// 0x7d goal.
    Goal(u8),
    /// 0xd6.
    Hisec(CaseEvent),
    /// 0xc6 (flag modes).
    Flag(super::modetop::FlagEvent),
    /// 0x51 / 0x50 / 0x4f / 0x56 / 0xad (Search and Destroy).
    Bomb(super::bomb::BombEvent),
    /// 0xd3 (Quarantine).
    Qrt(super::qrt::QrtEvent),
    /// 0xcb (Spy Hunt).
    Spy(super::spy::SpyEvent),
    /// 0xc9 (Fire Team: the goal list and the banner).
    Mission(super::mission::MissionEvent),
    /// 0xa6 notes the HUD reads.
    Note(ObjectNote),
    /// The own character object.
    ViewObject(Option<u16>),
    /// The door under the cross changed ([`super::prompt`], CLI-1230).
    UseTarget(Option<super::prompt::UseEntry>),
    /// A dropped weapon came into or left the use key's reach ([`super::prompt`], CLI-1352).
    ItemTarget(bool),
    /// 0x74 `from_dir`.
    Damage([f32; 3]),
    /// 0x74 `damage` and `zone` (the hit flash).
    Hit { damage: u32, zone: u8 },
    /// An own shot hit a character: the server's shot record carried the own hit feedback (the hit sound).
    OwnHit,
    /// The own client id and the mode class, when known.
    Own {
        pid: Option<u8>,
        mode_class: Option<u8>,
    },
    /// 0x74 `damage` and `dmg_kind` (the death panel's last wound).
    Wound { damage: u32, kind: u8 },
    /// 0xc8: a banner the server orders ([`super::banners::on_run`]).
    KillRun { kills: u16, head: bool, combo: u32 },
    /// A local weapon change (the 0x7f the client sent): the carried weapons in slot order and the selected one
    /// ([`super::panel`], client-hudgaps).
    WeaponPanel { carried: Vec<u16>, selected: u16 },
    /// 0x66 kind 0x0c: the screen flash (`fade`: the non-zero first sub-byte; its length in seconds), CLI-1926.
    ScreenFlash { fade: bool, secs: f32 },
}

impl HudState {
    /// Applies one input at `now` with the view yaw of this frame (lines are formed from [`HudState::texts`]).
    pub fn apply(&mut self, e: &HudEvent, now: u32, view_yaw: f32) {
        match e {
            HudEvent::Status { kind, value } => self.on_status(*kind, *value),
            HudEvent::ScreenFlash { fade, secs } => {
                // a new flash replaces the running one (cshell+0x47bb0 resets the state before it starts)
                self.whiteout = super::whiteout::Whiteout::start(now, *secs, *fade);
            }
            HudEvent::Life(st) => {
                self.on_life(*st, now);
                if *st == 1 {
                    if let Some(p) = self.own_pid {
                        self.mark_alive(p);
                    }
                }
            }
            HudEvent::Kill(k) => {
                self.on_kill(k, now);
                super::banners::on_kill(self, k, now);
            }
            HudEvent::KillRun { kills, head, combo } => {
                // DC-DEFAULT: CLI-1882 (proposed) a server that sends 0xc8 orders the co-op kill banners itself
                self.coop.server_runs = true;
                super::banners::on_run(self, *kills, *head, *combo, now)
            }
            // EVIDENCE: cshell+0x9f750 (shown flag, class and show time on every local weapon change)
            HudEvent::WeaponPanel { carried, selected } => {
                self.weapon_panel = Some(super::panel::PanelShow {
                    at: now,
                    carried: carried.clone(),
                    selected: *selected,
                })
            }
            HudEvent::Roster(r) => {
                self.on_roster(r);
            }
            HudEvent::Joined(name) => {
                let t = dc_ui_data::strings::format_message(
                    &self.texts.join,
                    &[dc_ui_data::FmtArg::Str(name)],
                );
                self.notice(t, now)
            }
            HudEvent::Left(pid, name) => {
                // the leave line only for a player with a team (not 0 / 0xff); the kick form after a 0x6c "kicked"
                // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 row 0x6c (CRes 0x185b when kicked, else 0xbb9, 5000 ms)
                // DC-DEFAULT: CLI-489 (proposed) no HUD record: the line is still shown
                let team = self.roster.get(pid).map(|r| r.team);
                let kicked = self.kicked.remove(pid);
                self.on_left(*pid);
                self.remote_health.remove(pid);
                if team.is_none_or(|t| t != 0 && t != 0xff) {
                    let form = if kicked && !self.texts.kicked.is_empty() {
                        &self.texts.kicked
                    } else {
                        &self.texts.leave
                    };
                    let t =
                        dc_ui_data::strings::format_message(form, &[dc_ui_data::FmtArg::Str(name)]);
                    self.notice(t, now);
                }
            }
            HudEvent::Kicked(pid) => {
                // the 2009 client ignores a 0x6c for its own id or an id without a record: nothing is kept (a kept
                // id would give the next player with that id the kick form)
                // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 row 0x6c
                if self.roster.contains_key(pid) && Some(*pid) != self.own_pid {
                    self.kicked.insert(*pid);
                }
            }
            // the 2009 client writes the ping of each known record and ignores an unknown id
            // EVIDENCE: cshell+0x2d73a..0x2d798 (record +0x00 := ping as float, found by record +0x04)
            // DC-DEFAULT: CLI-1361 (proposed) the ping is kept in the board row
            HudEvent::Pings(v) => {
                for &(pid, ping) in v {
                    let row = u8::try_from(pid)
                        .ok()
                        .and_then(|p| self.roster.get_mut(&p));
                    if let Some(r) = row {
                        r.ping = Some(ping);
                    }
                }
            }
            // DC-DEFAULT: CLI-1631 (proposed) the last health the server sent per player, dropped when he leaves
            HudEvent::RemoteHealth(v) => {
                for e in v {
                    self.remote_health.insert(e.pid, (e.health, e.max));
                }
            }
            HudEvent::Chat(m) => self.on_chat(m, now),
            HudEvent::TeamPair(p) => self.on_team_pair(*p),
            HudEvent::RoundWins(w) => self.on_round_wins(*w),
            HudEvent::RoundBegin(secs) => self.on_round_begin(*secs, now),
            HudEvent::RoundNumbers {
                round_no,
                round_limit,
            } => {
                self.round_no = *round_no;
                self.round_limit = *round_limit;
            }
            HudEvent::MatchBegin(m) => self.on_match_begin(m, now),
            HudEvent::RoundResult {
                round_secs,
                win_team,
                wins,
            } => self.on_round_result(*round_secs, *win_team, *wins, now),
            HudEvent::ClockStop(secs) => self.on_round_close(*secs, now),
            HudEvent::CoopClock { running, secs } => self.on_coop_clock(*running, *secs, now),
            HudEvent::CoopPoints {
                pid,
                gained,
                total,
                record,
            } => super::npcbar::on_points(self, *pid, *gained, *total, *record, now),
            HudEvent::ResultScreen { flag, value } => self.on_result_screen(*flag, *value, now),
            HudEvent::Summary(row) => {
                self.summary.insert(row.pid, *row);
            }
            HudEvent::Goal(g) => {
                self.goal = Some(*g as u32);
                // the match settings clear the half-time picture's flags (cshell+0xcd330)
                self.team_change.reset();
            }
            HudEvent::Hisec(h) => self.on_hisec(*h, now),
            HudEvent::Flag(f) => self.on_flag(f, now),
            HudEvent::Bomb(b) => self.on_bomb(b, now),
            HudEvent::Qrt(q) => self.on_qrt(q, now),
            HudEvent::Spy(e) => self.on_spy(e, now),
            HudEvent::Mission(e) => self.on_mission(e, now),
            HudEvent::Note(n) => self.on_note(*n, now),
            HudEvent::ViewObject(o) => self.view_object = *o,
            HudEvent::UseTarget(t) => self.use_target = *t,
            HudEvent::ItemTarget(t) => self.item_target = *t,
            HudEvent::Damage(d) => self.on_hit(*d, view_yaw),
            HudEvent::Hit { damage, zone } => self.on_hit_flash(*damage, *zone, now),
            HudEvent::OwnHit => {
                self.own_hit = true;
                self.own_hit_at = Some(now);
            }
            HudEvent::Own { pid, mode_class } => {
                if pid.is_some() {
                    self.own_pid = *pid;
                }
                if mode_class.is_some() {
                    self.mode_class = *mode_class;
                }
            }
            HudEvent::Wound { damage, kind } => self.on_wound(*damage, *kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(pid: u8, name: &str, team: u8) -> RosterRecord {
        RosterRecord {
            name: name.into(),
            pid,
            team,
            complete: true,
            ..RosterRecord::default()
        }
    }

    fn kill(victim: u8, killer: u8) -> KillRecord {
        KillRecord {
            victim,
            killer,
            weapon: 145,
            hit_zone: 3,
            complete: true,
            ..KillRecord::default()
        }
    }

    fn gs_chat(name: &str, text: &str, sender: u32, team_only: bool) -> HudEvent {
        HudEvent::Chat(super::super::chat::GsChat {
            name: name.into(),
            text: text.into(),
            sender,
            team_only,
            whisper: 0,
        })
    }

    #[test]
    fn a_team_line_says_team_and_takes_the_team_colour_class() {
        use super::super::chat::ChatClass;
        let mut h = HudState {
            own_pid: Some(0),
            texts: HudTexts {
                team_chat: "Team Chat".into(),
                ..HudTexts::default()
            },
            ..HudState::default()
        };
        h.apply(&HudEvent::Roster(rec(0, "me", 1)), 0, 0.0);
        h.apply(&HudEvent::Roster(rec(2, "mate", 1)), 0, 0.0);
        h.apply(&gs_chat("me", "push B", 0, true), 100, 0.0);
        h.apply(&gs_chat("mate", "on it", 2, true), 100, 0.0);
        h.apply(&gs_chat("mate", "gg", 2, false), 100, 0.0);
        let lines: Vec<(String, ChatClass, bool)> = h
            .chat
            .iter()
            .filter_map(|l| match l.kind {
                ChatKind::Chat { class, own, .. } => Some((l.text.clone(), class, own)),
                _ => None,
            })
            .collect();
        assert_eq!(
            lines,
            [
                ("[Team Chat] me: push B".to_string(), ChatClass::Team, true),
                (
                    "[Team Chat] mate: on it".to_string(),
                    ChatClass::Team,
                    false
                ),
                ("mate: gg".to_string(), ChatClass::Friendly, false),
            ]
        );
        // the drawn colour of a team line is the team colour, not the friendly one
        let look = super::super::chat::ChatLook::fallback();
        assert_ne!(
            look.colour(ChatClass::Team),
            look.colour(ChatClass::Friendly)
        );
    }

    #[test]
    fn chat_lines_take_their_class_and_expire_after_ten_seconds() {
        use super::super::chat::ChatClass;
        let mut h = HudState {
            own_pid: Some(0),
            ..HudState::default()
        };
        h.apply(&HudEvent::Roster(rec(0, "me", 1)), 0, 0.0);
        h.apply(&HudEvent::Roster(rec(1, "foe", 2)), 0, 0.0);
        h.apply(&HudEvent::Roster(rec(2, "mate", 1)), 0, 0.0);
        h.apply(&gs_chat("mate", "go", 2, false), 100, 0.0);
        h.apply(&gs_chat("foe", "hi", 1, false), 100, 0.0);
        h.apply(&gs_chat("mate", "left", 2, true), 100, 0.0);
        h.apply(&gs_chat("me", "ok", 0, false), 100, 0.0);
        // no roster record: nothing
        h.apply(&gs_chat("ghost", "boo", 9, false), 100, 0.0);
        let kinds: Vec<(ChatClass, bool)> = h
            .chat
            .iter()
            .filter_map(|l| match l.kind {
                ChatKind::Chat { class, own, .. } => Some((class, own)),
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            [
                (ChatClass::Friendly, false),
                (ChatClass::Enemy, false),
                (ChatClass::Team, false),
                (ChatClass::Friendly, true)
            ]
        );
        assert_eq!(h.chat[0].text, "mate: go");
        // the system notice is the banner, not a line
        h.apply(
            &gs_chat("", "server going down", 0xFFFF_FFFF, false),
            200,
            0.0,
        );
        assert_eq!(h.chat.len(), 4);
        assert_eq!(h.chat_banner, Some(("server going down".into(), 200)));
        // a notice (5000 ms) leaves before the chat lines (10000 ms); the banner after 10000 ms
        h.notice("joined".into(), 300);
        h.tick(5400, 1000);
        assert_eq!(h.chat.len(), 4);
        h.tick(10_099, 1000);
        assert_eq!(h.chat.len(), 4);
        h.tick(10_100, 1000);
        assert!(h.chat.is_empty());
        assert!(h.chat_banner.is_some());
        h.tick(10_200, 1000);
        assert!(h.chat_banner.is_none());
    }

    #[test]
    fn leave_lines_follow_the_kick_byte_and_the_team_rule() {
        let mut h = HudState {
            own_pid: Some(0),
            texts: HudTexts {
                leave: "%1 left".into(),
                kicked: "%1 kicked".into(),
                ..HudTexts::default()
            },
            ..HudState::default()
        };
        h.apply(&HudEvent::Roster(rec(1, "a", 1)), 0, 0.0);
        h.apply(&HudEvent::Roster(rec(2, "b", 2)), 0, 0.0);
        h.apply(&HudEvent::Roster(rec(3, "obs", 0xff)), 0, 0.0);
        h.apply(&HudEvent::Kicked(1), 10, 0.0);
        h.apply(&HudEvent::Left(1, "a".into()), 10, 0.0);
        h.apply(&HudEvent::Left(2, "b".into()), 20, 0.0);
        h.apply(&HudEvent::Left(3, "obs".into()), 30, 0.0);
        let lines: Vec<&str> = h.chat.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(lines, ["a kicked", "b left"]);
        assert!(h.kicked.is_empty());
    }

    /// A 0x6c "kicked" for the own id or an unknown id keeps nothing: a later player with that id leaves normally.
    #[test]
    fn kick_for_unknown_or_own_id_is_not_kept() {
        let mut h = HudState {
            own_pid: Some(0),
            texts: HudTexts {
                leave: "%1 left".into(),
                kicked: "%1 kicked".into(),
                ..HudTexts::default()
            },
            ..HudState::default()
        };
        h.apply(&HudEvent::Roster(rec(0, "me", 1)), 0, 0.0);
        h.apply(&HudEvent::Kicked(0), 5, 0.0);
        h.apply(&HudEvent::Kicked(4), 5, 0.0);
        assert!(h.kicked.is_empty());
        h.apply(&HudEvent::Roster(rec(4, "late", 1)), 10, 0.0);
        h.apply(&HudEvent::Left(4, "late".into()), 20, 0.0);
        let lines: Vec<&str> = h.chat.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(lines, ["late left"]);
    }

    #[test]
    fn the_client_counts_kills_from_the_kill_messages() {
        let mut h = HudState {
            own_pid: Some(0),
            ..HudState::default()
        };
        h.on_roster(&rec(0, "me", 1));
        h.on_roster(&rec(1, "foe", 2));
        h.on_roster(&rec(2, "mate", 1));
        h.on_kill(&kill(1, 0), 100);
        h.on_kill(&kill(0, 1), 200);
        h.on_kill(&kill(2, 0), 300); // same team: nothing counted
        h.on_kill(&kill(1, NO_KILLER), 400); // world: victim death only
        assert_eq!((h.roster[&0].kills, h.roster[&0].deaths), (1, 1));
        assert_eq!((h.roster[&1].kills, h.roster[&1].deaths), (1, 2));
        assert_eq!((h.roster[&2].kills, h.roster[&2].deaths), (0, 0));
        assert_eq!(h.feed.len(), 4);
        assert_eq!(h.feed[0].killer, "me");
        assert_eq!(h.feed[0].killer_rel, Relation::Mate);
        assert_eq!(h.feed[0].victim_rel, Relation::Opponent);
        assert_eq!(h.feed[3].killer, "");
        let mut nc = kill(0, 1);
        nc.no_count = true;
        h.on_kill(&nc, 500);
        assert_eq!(h.roster[&1].kills, 1, "no_count");
        h.tick(850, 400);
        assert_eq!(h.feed.len(), 1, "lines older than the lifetime leave");
        h.on_round_begin(180, 700);
        assert!(h.feed.is_empty());
    }

    #[test]
    fn a_fall_lights_the_damage_indicator_like_a_hit() {
        // the reference stack's fall: `from_dir` = up x the health loss / 100 (dc-dserver fall.rs, tdm4 c6 seq 6695)
        let fall = [0.0, 0.0199994, 0.0];
        assert_eq!(damage_sector(fall, 0.0), Some(DAMAGE_SECTORS / 2));
        assert_eq!(
            damage_sector(fall, 2.0),
            Some(DAMAGE_SECTORS / 2),
            "any yaw"
        );
        assert_eq!(damage_sector([0.0; 3], 0.0), None, "no direction, no flash");
        let mut h = HudState {
            life_state: 1,
            reality: true,
            ..HudState::default()
        };
        h.on_hit(fall, 0.7);
        // CLI-1847: a fall lights no sector: it starts the screen-edge band at full strength whatever its damage (the
        // logged 3 HP fall has from_dir length 0.028), decaying like a sector (gone after 1 s)
        assert!(h.damage.iter().all(|v| *v == 0.0), "{:?}", h.damage);
        assert_eq!(h.fall_border, 1.0);
        let mut g = HudState {
            life_state: 1,
            reality: true,
            ..HudState::default()
        };
        g.on_hit([0.0, 0.028, 0.0], 0.0);
        assert_eq!(g.fall_border, 1.0);
        g.tick(0, 1000);
        g.tick(500, 1000);
        assert!((g.fall_border - 0.5).abs() < 1e-5, "{}", g.fall_border);
        assert!(g.damage.iter().all(|v| *v == 0.0));
        g.tick(1000, 1000);
        assert_eq!(g.fall_border, 0.0, "gone after one second");
        // a shot lights one sector with its own magnitude (the 2009 rule)
        let mut sh = HudState {
            life_state: 1,
            reality: true,
            ..HudState::default()
        };
        sh.on_hit([-0.27, -0.10, 0.25], 0.0);
        assert_eq!(sh.damage.iter().filter(|v| **v > 0.0).count(), 1);
        assert_eq!(sh.fall_border, 0.0, "a shot draws no border");
        // and the aim cross's hit flash (0x74 damage > 0, zone 5)
        h.on_hit_flash(2, 5, 100);
        assert_eq!(h.hit_flash_at, Some(100));
    }

    #[test]
    fn damage_sectors_point_at_the_attacker_and_decay() {
        // attacker on the right (+X) of a player looking along +Z: the shot travels toward -X
        let right = damage_sector([-1.0, 0.0, 0.0], 0.0).unwrap();
        let left = damage_sector([1.0, 0.0, 0.0], 0.0).unwrap();
        let ahead = damage_sector([0.0, 0.0, -1.0], 0.0).unwrap();
        assert!(sector_angle(right) > 0.0 && sector_angle(left) < 0.0);
        assert!(sector_angle(ahead).abs() < 0.6);
        // the same shot after a half turn comes from the other side
        assert_eq!(
            damage_sector([-1.0, 0.0, 0.0], std::f32::consts::PI),
            Some(left)
        );
        let mut h = HudState {
            life_state: 1,
            reality: true,
            ..HudState::default()
        };
        h.on_hit([-0.3, 0.0, 0.0], 0.0);
        h.on_hit([-0.9, 0.0, 0.0], 0.0);
        assert_eq!(h.damage[right], 1.0, "capped");
        h.tick(0, 1000);
        h.tick(500, 1000);
        assert!((h.damage[right] - 0.5).abs() < 1e-5);
        h.on_life(2, 600);
        assert_eq!(h.damage, [0.0; DAMAGE_SECTORS]);
        h.on_hit([-1.0, 0.0, 0.0], 0.0);
        assert_eq!(h.damage, [0.0; DAMAGE_SECTORS], "ignored while dead");
    }

    #[test]
    fn respawn_wait_follows_the_mode_class() {
        let mut h = HudState {
            mode_class: Some(1),
            ..HudState::default()
        };
        h.on_life(2, 1000);
        h.tick(3500, 1000);
        let (left, frac) = h.respawn_left().unwrap();
        assert_eq!(left, 4500);
        assert!((frac - 2500.0 / 7000.0).abs() < 1e-6);
        h.on_life(1, 8100);
        assert_eq!(h.respawn_left(), None);
        let mut r = HudState {
            mode_class: Some(2),
            ..HudState::default()
        };
        r.on_life(2, 0);
        assert_eq!(
            r.respawn_left(),
            None,
            "round classes never respawn in a round"
        );
    }

    fn tdm(own: u8) -> HudState {
        let mut h = HudState {
            own_pid: Some(own),
            mode_class: Some(1),
            life_state: 1,
            ..HudState::default()
        };
        h.on_roster(&rec(0, "alpha", 1));
        h.on_roster(&rec(1, "bravo", 2));
        h
    }

    fn begin(secs: u32, banner: bool, body: &str) -> MatchBegin {
        MatchBegin {
            round_secs: secs,
            show_intro: !body.is_empty(),
            waiting_banner: banner,
            intro: (!body.is_empty()).then(|| IntroBlock {
                head: "h.dtx".into(),
                body: body.into(),
                ..IntroBlock::default()
            }),
            ..MatchBegin::default()
        }
    }

    /// The reference stack's start (0x44 0 then 0x45 0) leaves the clock hidden; a time limit shows it running; 0x46 / 0x48
    /// hide it; 0xd7 sub 4 shows it frozen; 1:60 at exactly 60 s left.
    #[test]
    fn round_clock_follows_the_round_flow_messages() {
        let mut h = tdm(0);
        h.on_round_begin(0, 1000);
        assert!(h.clock.shown, "0x44 shows the clock even with 0");
        h.on_match_begin(&begin(0, false, ""), 1000);
        assert!(!h.clock.shown, "0x45 with 0 hides it (reference start)");
        h.on_round_begin(600, 2000);
        h.on_match_begin(&begin(600, false, ""), 2000);
        h.now_ms = 2000;
        assert_eq!(h.clock.shows(2000), (10, 0));
        assert_eq!(h.clock.shows(2001), (9, 59), "truncated");
        assert_eq!(h.clock.shows(2000 + 540_000), (1, 60), "exactly 60 s");
        assert_eq!(h.clock.shows(2000 + 540_001), (0, 59));
        assert_eq!(h.clock.shows(2000 + 700_000), (0, 0));
        h.on_round_result(3, 1, [0, 0], 5000);
        assert!(!h.clock.shown);
        h.on_coop_clock(false, 90, 6000);
        assert!(h.clock.shown);
        assert_eq!(h.clock.shows(60_000), (1, 30), "frozen");
        h.on_round_close(3, 7000);
        assert!(!h.clock.shown);
        assert_eq!(h.clock_left(), None);
    }

    /// The intro banner starts with a non-empty text image while the latch is clear; the latch holds until a
    /// round-flow message; the waiting banner stays up until the round result.
    #[test]
    fn intro_banner_latch_and_waiting_banner() {
        let mut h = tdm(0);
        h.on_match_begin(&begin(0, true, "b.dtx"), 100);
        assert_eq!(h.intro.as_ref().map(|i| i.at), Some(100));
        assert!(h.waiting_banner);
        h.intro = None;
        h.on_match_begin(&begin(0, true, "b.dtx"), 200);
        assert!(h.intro.is_none(), "latched");
        h.on_round_begin(0, 300);
        h.on_match_begin(&begin(0, false, ""), 300);
        assert!(!h.waiting_banner, "0x45 rewrites the flag");
        assert!(h.intro_block.is_some(), "the last block is kept");
        h.on_match_begin(&begin(0, false, "c.dtx"), 400);
        assert_eq!(h.intro.as_ref().map(|i| i.body.as_str()), Some("c.dtx"));
        h.on_match_begin(&begin(0, true, ""), 500);
        h.on_round_result(3, 0, [0, 0], 600);
        assert!(!h.waiting_banner, "0x46 takes it down");
        assert_eq!(
            h.result_mark,
            Some(ResultMark::Score),
            "a draw under the waiting-banner latch shows the scoreboard image"
        );
    }

    #[test]
    fn result_mark_by_the_own_team() {
        let mut a = tdm(0);
        a.on_round_result(3, 1, [0, 0], 0);
        assert_eq!(a.result_mark, Some(ResultMark::Win));
        let mut b = tdm(1);
        b.on_round_result(3, 1, [0, 0], 0);
        assert_eq!(b.result_mark, Some(ResultMark::Lose));
        let mut c = tdm(1);
        c.on_round_result(3, 0, [0, 0], 0);
        assert_eq!(c.result_mark, Some(ResultMark::Draw));
        c.on_round_begin(0, 10);
        assert_eq!(c.result_mark, None, "0x44 resets it");
        c.on_result_screen(false, 2, 20);
        assert_eq!(c.result_screen.map(|r| r.value), Some(2));
        assert!(!c.clock.shown);
    }

    /// CLI-1152: a Quarantine round result shows the humans' or the infectors' picture to everybody (2009 picks it by
    /// the win value alone in class 11); we showed the team win / loss pictures.
    #[test]
    fn quarantine_round_result_shows_the_side_picture() {
        for (own, win, want) in [
            (0, 1, ResultMark::HumanWin),
            (1, 1, ResultMark::HumanWin),
            (0, 2, ResultMark::InfectorWin),
            (1, 2, ResultMark::InfectorWin),
        ] {
            let mut h = tdm(own);
            h.mode_class = Some(11);
            h.on_round_result(3, win, [0, 0], 0);
            assert_eq!(h.result_mark, Some(want), "own {own} win {win}");
        }
    }

    /// `streak_flag` notes the first killer for everyone; only the killer gets the announcement.
    #[test]
    fn first_kill_note_and_announcement() {
        let mut me = tdm(0);
        let mut other = tdm(1);
        let mut k = kill(1, 0);
        k.streak_flag = 1;
        me.on_kill(&k, 100);
        other.on_kill(&k, 100);
        assert_eq!(me.mk.len(), 1);
        assert_eq!(me.mk[0].kind, MkKind::Opener);
        assert!(me.mk[0].mate);
        assert!(
            !other.mk[0].mate,
            "the killer is of the other team for bravo"
        );
        assert_eq!(me.combo_at, Some(100));
        assert_eq!(other.combo_at, None);
        me.on_kill(&kill(1, 0), 200);
        assert_eq!(me.mk.len(), 1, "no note without the flag");
        for i in 0..7 {
            me.on_kill(&k, 300 + i);
        }
        assert_eq!(me.mk.len(), MK_LINES);
        me.expire_notes(304 + 3750, 3750);
        assert_eq!(me.mk.len(), 2, "notes older than the note time leave");
    }

    /// The HiSec case map: a reserved case is hidden for others, a free one for nobody; a pickup by a known player
    /// removes the entry, adds a line and a note, and the own pickup starts the banner; an unknown player does nothing.
    #[test]
    fn hisec_case_events() {
        let mut h = tdm(0);
        h.texts.case_taken = "%1 took it".into();
        h.texts.case_drop = "dropped".into();
        h.on_hisec(CaseEvent::Dropped, 10);
        h.on_hisec(CaseEvent::Owner { key: 1, owner: 1 }, 10);
        assert!(!h.case_visible(1));
        assert!(h.case_visible(2), "no entry: visible");
        h.on_hisec(
            CaseEvent::Owner {
                key: 1,
                owner: HISEC_NOBODY,
            },
            20,
        );
        assert!(h.case_visible(1));
        h.on_hisec(CaseEvent::Owner { key: 3, owner: 0 }, 20);
        assert!(h.case_visible(3), "reserved for me");
        h.on_hisec(CaseEvent::Taken { key: 1, player: 9 }, 30);
        assert!(
            h.case_owner.contains_key(&1),
            "unknown player: nothing happens"
        );
        h.on_hisec(CaseEvent::Taken { key: 1, player: 0 }, 40);
        assert!(!h.case_owner.contains_key(&1));
        assert_eq!(h.case_banner_at, Some(40));
        assert_eq!(h.mk.back().map(|n| n.kind), Some(MkKind::Hisec));
        let lines: Vec<&str> = h.chat.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(lines, ["dropped", "alpha took it"]);
        assert!(h.chat.iter().all(|l| l.kind == ChatKind::Hisec));
        // CRATE-2: a Camo Crate's key names it in both lines, and its pickup adds no HiSec note
        let crate_key = 2 | dc_bute::camo_crate::CAMO_KEY_BIT;
        let notes = h.mk.len();
        h.on_hisec(CaseEvent::Dropped, 50);
        h.on_hisec(CaseEvent::Owner { key: crate_key, owner: 0 }, 50);
        h.on_hisec(CaseEvent::Taken { key: crate_key, player: 0 }, 60);
        let lines: Vec<&str> = h.chat.iter().map(|l| l.text.as_str()).skip(2).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], CAMO_DROP_TEXT);
        assert!(lines[1].contains("alpha") && lines[1].contains("Camo Crate"), "{lines:?}");
        assert_eq!(h.mk.len(), notes);
    }

    #[test]
    fn spawn_notes_of_the_own_character() {
        let mut h = tdm(0);
        h.view_object = Some(0xfc);
        h.on_note(
            ObjectNote::SpawnMark {
                obj: 0x8c,
                on: true,
            },
            5,
        );
        assert_eq!(h.spawn_mark, None, "another character");
        h.on_note(
            ObjectNote::SpawnMark {
                obj: 0xfc,
                on: true,
            },
            6,
        );
        assert_eq!(h.spawn_mark, Some(6));
        h.on_note(
            ObjectNote::SpawnMark {
                obj: 0xfc,
                on: false,
            },
            3500,
        );
        assert_eq!(h.spawn_mark, None);
    }

    #[test]
    fn hits_flash_the_aim_cross_and_the_own_death_keeps_the_killer() {
        let mut h = HudState {
            own_pid: Some(0),
            ..HudState::default()
        };
        h.on_roster(&rec(0, "me", 1));
        h.on_roster(&rec(1, "foe", 2));
        h.on_life(1, 0);
        // 0x74: a hit with damage flashes; a zero hit or the no-flash zone does not
        h.on_hit_flash(0, 3, 100);
        h.on_hit_flash(20, NO_FLASH_ZONE, 100);
        assert_eq!(h.hit_flash_at, None);
        h.on_hit_flash(20, 3, 100);
        h.now_ms = 100;
        assert_eq!(h.hit_flash(), 1.0);
        h.now_ms = 100 + HIT_FLASH_MS / 2;
        assert!((h.hit_flash() - 0.5).abs() < 1e-3);
        h.now_ms = 100 + HIT_FLASH_MS;
        assert_eq!(h.hit_flash(), 0.0);
        // a kill of somebody else is no own death
        h.on_kill(&kill(1, 0), 200);
        assert_eq!(h.death, None);
        // the own death: killer, weapon; with a revenge block the panel
        let mut k = kill(0, 1);
        k.revenge = Some(3);
        k.hit_zone = 1;
        h.on_kill(&k, 300);
        h.on_life(2, 310);
        let d = h.death.clone().expect("own death");
        assert_eq!(d.killer.as_deref(), Some("foe"));
        assert!(d.head_shot);
        assert_eq!(d.killer_rel, Relation::Opponent);
        h.now_ms = 400;
        assert_eq!(
            h.revenge_shown().map(|r| (r.killer.as_str(), r.count)),
            Some(("foe", 3))
        );
        // dead: hits do not flash
        h.on_hit_flash(20, 3, 500);
        assert_eq!(h.hit_flash_at, Some(100));
        h.now_ms = 300 + REVENGE_MS;
        assert!(h.revenge_shown().is_none(), "the panel times out");
        // a world death names nobody; state 1 clears everything
        h.on_kill(&kill(0, NO_KILLER), 600);
        assert_eq!(h.death.as_ref().map(|d| d.killer.clone()), Some(None));
        h.on_life(1, 700);
        assert_eq!((h.death.clone(), h.revenge.clone()), (None, None));
    }

    fn take_voices(h: &mut HudState) -> Vec<Voice> {
        std::mem::take(&mut h.banner.voices)
    }

    /// The 0x45 voice needs own state 1; the waiting flag swaps the line; no latch (a second 0x45 plays again).
    #[test]
    fn round_start_voice_follows_state_and_waiting_flag() {
        let mut h = tdm(0);
        h.on_match_begin(&begin(600, false, ""), 100);
        assert_eq!(take_voices(&mut h), [Voice::RoundStart]);
        h.on_match_begin(&begin(0, true, ""), 200);
        assert_eq!(take_voices(&mut h), [Voice::FirstSpawn]);
        h.on_match_begin(&begin(600, false, ""), 300);
        assert_eq!(take_voices(&mut h), [Voice::RoundStart], "no latch");
        h.life_state = 2;
        h.on_match_begin(&begin(600, false, ""), 400);
        h.on_match_begin(&begin(0, true, ""), 500);
        assert!(take_voices(&mut h).is_empty(), "dead: silent");
    }

    /// CLI-1602: a respawn of a respawn mode says "round start" again through the server's 0x45 after the spawn
    /// set (DSV-473): the 0x67 [1] alone plays nothing, the 0x45 then plays the line, keeps the seconds it brings
    /// and does not show the latched intro again.
    // DISASM: cshell+0x49f00..0x49f7b (the voice for every 0x45 in own state 1; `RoundStartSound` has no other
    // reference in the module)
    #[test]
    fn a_respawn_match_begin_replays_the_round_start_voice() {
        let mut h = tdm(0);
        h.on_match_begin(&begin(600, false, "mission"), 100);
        assert_eq!(take_voices(&mut h), [Voice::RoundStart]);
        assert!(h.intro.take().is_some());
        h.on_life(2, 10_000);
        h.on_life(1, 17_000);
        assert!(take_voices(&mut h).is_empty(), "the spawn state alone is silent");
        h.on_match_begin(&begin(583, false, ""), 17_010);
        assert_eq!(take_voices(&mut h), [Voice::RoundStart], "again at the respawn");
        assert!(h.intro.is_none(), "the intro stays latched");
        // a round mode's next round (0x44, then 0x45 in state 1) says it as before
        h.mode_class = Some(2);
        h.on_round_begin(120, 30_000);
        assert!(take_voices(&mut h).is_empty(), "0x44 plays nothing");
        h.on_match_begin(&begin(120, false, ""), 30_010);
        assert_eq!(take_voices(&mut h), [Voice::RoundStart]);
    }

    /// 0x46: the winning team's line (1 first, else second), the infection pair for class 11, nothing for classes
    /// 5, 6, 9 or win team 0 (the draw line only when switched on).
    #[test]
    fn team_win_voice_by_class_and_team() {
        let mut h = tdm(0);
        h.on_round_result(3, 1, [1, 0], 10);
        h.on_round_result(3, 2, [1, 1], 20);
        assert_eq!(take_voices(&mut h), [Voice::WinAlpha, Voice::WinBravo]);
        h.on_round_result(3, 0, [1, 1], 30);
        assert!(
            take_voices(&mut h).is_empty(),
            "a draw plays nothing in 2009"
        );
        h.draw_voice = true;
        h.on_round_result(3, 0, [1, 1], 40);
        assert_eq!(take_voices(&mut h), [Voice::RoundDraw]);
        for class in [5, 6, 9] {
            h.mode_class = Some(class);
            h.on_round_result(3, 1, [1, 0], 50);
            assert!(take_voices(&mut h).is_empty(), "class {class}");
        }
        h.mode_class = Some(11);
        h.on_round_result(3, 1, [1, 0], 60);
        h.on_round_result(3, 2, [1, 1], 70);
        assert_eq!(take_voices(&mut h), [Voice::WinHuman, Voice::WinInfector]);
    }

    /// 0xd6 sounds: owner update (-1 timer, else drop), pickup (picked-up only with an entry; by-me for every own pickup).
    #[test]
    fn hisec_case_sounds() {
        let mut h = tdm(0);
        h.on_hisec(CaseEvent::Dropped, 1);
        assert!(take_voices(&mut h).is_empty(), "sub 0 plays nothing");
        h.on_hisec(CaseEvent::Owner { key: 1, owner: 1 }, 2);
        h.on_hisec(
            CaseEvent::Owner {
                key: 1,
                owner: HISEC_NOBODY,
            },
            3,
        );
        assert_eq!(take_voices(&mut h), [Voice::CaseDrop, Voice::CaseTimer]);
        h.on_hisec(CaseEvent::Taken { key: 1, player: 1 }, 4);
        assert_eq!(take_voices(&mut h), [Voice::CasePicked]);
        h.on_hisec(CaseEvent::Taken { key: 1, player: 1 }, 5);
        assert!(take_voices(&mut h).is_empty(), "no entry: no sound");
        h.on_hisec(CaseEvent::Owner { key: 2, owner: 0 }, 6);
        take_voices(&mut h);
        h.on_hisec(CaseEvent::Taken { key: 2, player: 0 }, 7);
        assert_eq!(take_voices(&mut h), [Voice::CasePicked, Voice::CaseByMe]);
        h.on_hisec(CaseEvent::Taken { key: 3, player: 0 }, 8);
        assert_eq!(
            take_voices(&mut h),
            [Voice::CaseByMe],
            "own pickup without an entry: by-me only"
        );
        h.on_hisec(CaseEvent::Taken { key: 2, player: 9 }, 8);
        assert!(take_voices(&mut h).is_empty(), "not in the roster");
    }

    #[test]
    fn arcs_need_the_reality_option() {
        let mut h = tdm(0);
        h.on_hit([-1.0, 0.0, 0.0], 0.0);
        assert_eq!(h.damage, [0.0; DAMAGE_SECTORS]);
        h.reality = true;
        h.on_hit([-1.0, 0.0, 0.0], 0.0);
        assert!(h.damage.iter().any(|v| *v > 0.0));
    }
}

/// CLI-1639: How long the promotion / demotion line stays.
pub const GUN_LINE_MS: u32 = 2_500;

/// CLI-1639: what the HUD knows of a Gun Game match: the guns' names and the own rung (from the own 0x6b record).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GunGameHud {
    pub names: Vec<String>,
    pub rung: usize,
    /// CLI-1850: the own foe kills of the match (0x6d), the number of the "N Kills" banner; the rung is no count.
    pub kills: u32,
    /// The centre line: text, demotion, since when.
    pub line: Option<(String, bool, u32)>,
}

impl GunGameHud {
    pub fn new(names: Vec<String>) -> Self {
        GunGameHud {
            names,
            rung: 0,
            kills: 0,
            line: None,
        }
    }
}
