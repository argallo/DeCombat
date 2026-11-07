//! [`HudLayout`]: where and with what each HUD element is drawn, in the 1024 x 768 virtual screen of the 2009 UI.
//!
//! With the user's ROM every value comes from the HUD layout set ([`dc_ui_data::HudView`]: textures, positions,
//! sizes, fonts, colours, the label string ids), the client's own text table and the weapon rows (kill-feed icons,
//! weapon-name images); the few positions the 2009 code holds as constants (round clock) are cited by rva. Without
//! a ROM, [`HudLayout::fallback`] is a text-and-box layout of our own (no ROM value in it).
//!
//! The struct is plain data on purpose: it is the surface a server-declared mod's `hud.draw` hook replaces
//! (HOSTING_AND_MODS.md §3.1); nothing outside it decides where an element goes.
// SPEC: FMT_CRES_UI.md §5 step 5 (1024 x 768 virtual screen), §6.2 (HUD set, set-0 fallback), §8 (elements)
// DOC: HOSTING_AND_MODS.md §3.1 (`hud.draw` hook)

use dc_ui_data::{HudKey as K, HudKeys, HudLiteral, HudView, LayoutTable, UiStrings};
use std::collections::BTreeMap;

/// A texture drawn 1:1 from its top-left texels: `size` texels at `pos` (virtual pixels).
// EVIDENCE: cshell+0x343860 (image item: source rect (0, 0, w, h), destination size (w, h))
#[derive(Debug, Clone, PartialEq)]
pub struct Img {
    pub tex: String,
    pub pos: [i32; 2],
    pub size: [i32; 2],
}

/// Three digit images (hundreds, tens, ones), each `prefix + digit + ".dtx"`.
// EVIDENCE: cshell+0x95b60 (`%s%d.dtx`, value / 100, value / 10 % 10, value % 10; all three drawn)
#[derive(Debug, Clone, PartialEq)]
pub struct Digits {
    pub prefix: String,
    pub pos: [[i32; 2]; 3],
    pub size: [i32; 2],
}

/// A gauge drawn as one of eleven images `prefix + (0, 10, .., 100) + ".dtx"` (the arcs beside the aim mark).
// EVIDENCE: cshell+0x95e80 (health: `(v + 5) - (v + 5) % 10`), cshell+0x6f1b0 (clip: percent of the clip, same rounding)
#[derive(Debug, Clone, PartialEq)]
pub struct StepGauge {
    pub prefix: String,
    pub pos: [i32; 2],
    pub size: [i32; 2],
}

/// Text style: system font index (-1 = the client's default font), straight RGBA, and an optional cell height in
/// virtual pixels that replaces the font record's height (elements with their own font-size key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextStyle {
    pub font: i32,
    pub colour: [u8; 4],
    pub size: Option<i32>,
}

/// Health and armour bars of the fallback layout: rectangles (x, y, w, h) and colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusBars {
    pub health: [i32; 4],
    pub armour: [i32; 4],
    pub health_colour: [u8; 4],
    pub armour_colour: [u8; 4],
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HealthLayout {
    pub health_icon: Option<Img>,
    pub armour_icon: Option<Img>,
    pub health_digits: Option<Digits>,
    pub armour_digits: Option<Digits>,
    pub stance_stand: Option<Img>,
    pub stance_crouch: Option<Img>,
    pub stamina_back: Option<Img>,
    pub stamina_cells: Option<Img>,
    pub health_arc: Option<StepGauge>,
    /// Text form (fallback layout): health and armour numbers at these positions.
    pub text: Option<([i32; 2], [i32; 2], TextStyle)>,
    /// Bar form (fallback layout): health and armour bars (full rectangle, colour), filled by value / maximum.
    pub bars: Option<StatusBars>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AmmoLayout {
    pub clip: Option<Digits>,
    pub slash: Option<Img>,
    pub reserve: Option<Digits>,
    pub fire_single: Option<Img>,
    pub fire_auto: Option<Img>,
    pub clip_arc: Option<StepGauge>,
    /// Position and size of the weapon-name image (the texture comes from the weapon row).
    pub name_image: Option<([i32; 2], [i32; 2])>,
    pub text: Option<([i32; 2], TextStyle)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeedLayout {
    pub font: i32,
    pub pos: [i32; 2],
    pub line_step: i32,
    pub lifetime_ms: u32,
    pub weapon_size: [i32; 2],
    /// Vertical offset of the weapon icon against the line top (negative = up).
    pub weapon_lift: i32,
    pub head_icon: Option<String>,
    pub head_size: [i32; 2],
    pub head_lift: i32,
    pub mate_colour: [u8; 4],
    pub opponent_colour: [u8; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoreLayout {
    pub team1: Option<Digits>,
    pub team2: Option<Digits>,
    pub separator: Option<Img>,
    pub team1_mark: Option<Img>,
    pub team2_mark: Option<Img>,
    pub text: Option<([i32; 2], TextStyle)>,
}

/// Round clock: two minute digits, a separator, two second digits, each a texture of the digit form.
#[derive(Debug, Clone, PartialEq)]
pub struct ClockLayout {
    /// A printf form with one `%d` (the digit).
    pub digit_form: String,
    pub separator: String,
    pub minutes: [[i32; 2]; 2],
    pub separator_pos: [i32; 2],
    pub seconds: [[i32; 2]; 2],
    pub size: [i32; 2],
    /// Tint of the digit images (the layout's round-clock colour).
    pub tint: [u8; 4],
    pub text: Option<([i32; 2], TextStyle)>,
}

/// The scoreboard. `rows`, the labels and the row fields are relative to the window position (the labels then
/// fall on the window's header band and the rows on its row bands, as the ROM window texture shows).
#[derive(Debug, Clone, PartialEq)]
pub struct BoardLayout {
    pub window: Option<Img>,
    /// Plain window rectangle and colour (fallback layout).
    pub panel: Option<([i32; 4], [u8; 4])>,
    pub kill_label: Option<(String, [i32; 2])>,
    pub death_label: Option<(String, [i32; 2])>,
    pub rows: [i32; 2],
    pub row_step: i32,
    pub team_offset: i32,
    pub font: i32,
    /// Cell height of the board text (its own font-size key).
    pub font_size: Option<i32>,
    pub name: [i32; 2],
    pub kills: [i32; 2],
    pub deaths: [i32; 2],
    pub own: [u8; 4],
    pub alive: [u8; 4],
    pub dead: [u8; 4],
}

/// The damage arc: `uv` texels of the texture drawn at `pos` / `size`, turned about the screen centre to the
/// sector's direction.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageLayout {
    pub tex: Option<String>,
    pub pos: [i32; 2],
    pub size: [i32; 2],
    pub uv: [i32; 2],
    pub colour: [u8; 4],
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RespawnLayout {
    pub title: Option<Img>,
    /// Number texture form (one `%d`), position, size.
    pub number: Option<(String, [i32; 2], [i32; 2])>,
    pub note: Option<Img>,
    pub bar: Option<Img>,
    pub fill: Option<Img>,
    /// The gauge's end caps (left, right), shown as the gauge passes them.
    pub left: Option<Img>,
    pub right: Option<Img>,
    pub text: Option<([i32; 2], TextStyle)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatLayout {
    pub font: i32,
    pub pos: [i32; 2],
    pub line_step: i32,
    pub notice: [u8; 4],
    /// Join and leave notice forms (FormatMessage text with `%1` = the name).
    pub join_form: String,
    pub leave_form: String,
    /// HiSec lines: "a case dropped", "%1 obtained a case".
    pub case_drop_form: String,
    pub case_taken_form: String,
    /// The revenge panel's count line (a FormatMessage form with `%1` = the count).
    pub revenge_form: String,
    /// The leave line of a kicked player (`%1` = the name) and the chat input's repeat refusal.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 row 0x6c (CRes 0x185b); EVIDENCE: cshell+0x72df0 (CRes 0x1028)
    pub kicked_form: String,
    pub same_line_form: String,
    /// CLI-1031: the sensitivity hotkeys' line (CRes 0x4f6).
    pub sensitivity_form: String,
    /// CLI-1123: the name of the team chat (CRes 0x543), the tag in front of a team chat line.
    pub team_chat_name: String,
}

/// The intro banner of the 0x45 intro block: the title image at its own place and size, the text image centred at
/// its y with its texture size, and the animation timing.
// EVIDENCE: cshell+0xbbfc0 (title image at the title place / size, start scale = title scale; text image at x =
// (1024 - w) / 2, text y, start scale = text scale, both alpha 0); cshell+0xbbb90 (timing keys, scales x 0.01);
// cshell+0xbc7d0 (animation, below)
#[derive(Debug, Clone, PartialEq)]
pub struct IntroLayout {
    pub head_pos: [i32; 2],
    pub head_size: [i32; 2],
    pub body_y: i32,
    pub head_scale: f32,
    pub body_scale: f32,
    /// Phase ends after the delay: fade-in end, hold end, slide end (ms).
    pub fade_in: u32,
    pub hold: u32,
    pub slide_end: u32,
    pub delay: u32,
    /// Slide distance of the last phase (virtual pixels).
    pub slide: f32,
    /// Removed this long after the delay.
    pub duration: u32,
}

/// The round result mark of 0x46: place, size, the three images.
// EVIDENCE: cshell+0xc1a50
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResultMarkLayout {
    pub pos: [i32; 2],
    pub size: [i32; 2],
    pub win: Option<String>,
    pub lose: Option<String>,
    pub draw: Option<String>,
    /// The scoreboard image (a draw while the waiting-banner latch is set).
    pub score: Option<String>,
    /// Class 11: the humans' and the infectors' round-win pictures.
    pub human_win: Option<String>,
    pub infector_win: Option<String>,
}

/// The top block around the team counters: TDM mode title, kill goal digits and word, team name bases, own-team
/// bars.
// EVIDENCE: cshell+0x8ebe0 (items and places), cshell+0x90820 (the own team's name uses its "on" image and its bar
// is shown; mode class 6 shows neither), cshell+0x916a0 (goal digits: score digit images `prefix + d + ".dtx"`)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TopLayout {
    pub tdm_title: Option<Img>,
    pub goal: Option<Digits>,
    pub goal_word: Option<Img>,
    pub team1_base: Option<Img>,
    pub team2_base: Option<Img>,
    pub own_bar: Option<OwnBar>,
    /// The free-for-all block (mode class 6 shows it in place of the team counters).
    pub ffa: FfaTop,
    /// The Elimination Pro block (mode class 12).
    pub round: super::modetop::RoundTop,
    /// The flag-mode block (mode classes 7 / 8).
    pub flag: super::modetop::FlagTop,
    /// The half-time team-change picture (classes 3, 4, 8, 13).
    pub team_change: super::teamchange::TeamChangeLayout,
    /// The Search and Destroy timer, notices, gauge and held bomb (mode classes 2 / 3).
    pub bomb: super::bomb::BombLayout,
    /// The infection element (mode class 11).
    pub qrt: super::qrt::QrtLayout,
    /// The Spy Hunt element (mode class 9).
    pub spy: super::spy::SpyLayout,
    /// The Fire Team goal list and mission banner (mode class 5).
    pub mission: super::mission::MissionLayout,
    /// The Fire Team score and drop-item notes (mode class 5).
    pub coop: super::coopscore::CoopScoreLayout,
    /// The door use prompt and the activate-type table (CLI-1230).
    pub prompt: super::prompt::PromptLayout,
}

/// The free-for-all top block: title image, "my score" image, the own kills and deaths as two score digits each
/// (tens, ones) with a word image after each.
// EVIDENCE: cshell+0x8d470 (items: title = title key value through the title form, my-score image, kill digits at
// the kill tens / ones places with the team-1 digit size, kill word `value + ".dtx"`, death digits and word; the
// death word takes the kill word's size), cshell+0x92570 (class 6: the team counters hidden, this block shown; both
// numbers reset to 0), cshell+0x8dba0 / 0x8dd00 (digit images `score prefix + d + ".dtx"`)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FfaTop {
    pub title: Option<Img>,
    pub own_score: Option<Img>,
    pub kills: Option<[Img; 2]>,
    pub kill_word: Option<Img>,
    pub deaths: Option<[Img; 2]>,
    pub death_word: Option<Img>,
    /// Score digit prefix (`prefix + d + ".dtx"`).
    pub digit_prefix: String,
    /// The title key's value: the title shown once the room goal is known is `prefix + goal + "kill.dtx"`.
    // EVIDENCE: cshell+0x91100 (class 6 goal title: `%s%dkill.dtx` with the goal at +0x114), cshell+0x92c10 / 0x92e20
    // (the room goal setter calls it for class 6); the creation-time title is the fixed 15-kill form (cshell+0x8d470)
    pub title_prefix: String,
}

/// The own team's bars: texture, the places for team 1 and team 2, the size.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnBar {
    pub tex: String,
    pub team1: [i32; 2],
    pub team2: [i32; 2],
    pub size: [i32; 2],
}

/// Multi-kill notes on the left: name text, then the note image after the name.
// EVIDENCE: cshell+0xa36a0 (keys), cshell+0xa75a0 / cshell+0xa5fe0 (name coloured own team / enemy; image at name
// end + image gap, lifted by the image lift)
#[derive(Debug, Clone, PartialEq)]
pub struct MkLayout {
    pub font: i32,
    pub mate: [u8; 4],
    pub enemy: [u8; 4],
    pub opener_img: Option<String>,
    pub hisec: Option<String>,
    /// The flag line images in [`super::modetop::FlagLine`] order.
    pub flag_imgs: [Option<String>; 6],
    pub pos: [i32; 2],
    pub gap: i32,
    pub lift: i32,
    pub size: [i32; 2],
    pub line_step: i32,
    pub lifetime_ms: u32,
}

/// A centred announcement image with its timing (seconds): the own first-kill announcement and the own HiSec pickup
/// banner share the form.
// EVIDENCE: cshell+0xa1320 / cshell+0xa29c0 (first kill), cshell+0x98ab0 (pickup banner)
#[derive(Debug, Clone, PartialEq)]
pub struct Announce {
    pub tex: String,
    pub size: [i32; 2],
    /// Centre of the image.
    pub centre: [i32; 2],
    pub total: f32,
    pub create: f32,
    pub end: f32,
    pub destroy: f32,
    pub first_scale: f32,
}

/// The HiSec case indicator (image and its size) and the own pickup banner.
// EVIDENCE: cshell+0x9a860 (indicator image and size, number text of font 0xe), cshell+0x98ab0 (pickup banner)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HisecLayout {
    pub indicator: Option<(String, [i32; 2])>,
    pub pickup: Option<Announce>,
}

/// The aim cross at the screen centre: bar length and half gap (virtual pixels), colour, line width 1 pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AimCrossLayout {
    pub bar: f32,
    pub half_gap: f32,
    pub colour: [u8; 4],
}

/// The over-head player marks: the name caption's font and size, its colour by the mark's state, the two timings of
/// the hit colour, and how high above the drawn box the caption sits.
// EVIDENCE: cshell+0x88da0 (caption: font record 3, size 0xe, flags 0x24), cshell+0x8b110 (colours and delays read
// from layout set 0), cshell+0x8b2e0 (per-frame colour and visibility)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarkLayout {
    pub style: TextStyle,
    pub mate: [u8; 4],
    pub enemy: [u8; 4],
    pub dead: [u8; 4],
    pub hit: [u8; 4],
    /// Seconds the hit colour holds after an own hit, then seconds it fades back (traced, not drawn yet).
    pub hit_secs: f32,
    pub fade_secs: f32,
}

/// The caption font record and cell height the 2009 marker uses (code constants of its caption builder).
// EVIDENCE: cshell+0x88da0 (`push 3` font record, `push 0xe` size)
pub const MARK_FONT: i32 = 3;
pub const MARK_FONT_SIZE: i32 = 14;

/// The panel the aim cross shows while a team mate is under it: an image and the mate's health number.
// EVIDENCE: cshell+0x83e40 (image of `MateInfoTex` sized `MateInfoSize` at `MateInfoPos`; text item of font
// `MateHealthFont`, size `MateHealthFontSize`, colour and alpha, at `MateHealthPos`), cshell+0x853f0 (both drawn after
// the aim cross while the own player state is 1), cshell+0x826c0 (`%d` of the mate's health; hidden when <= 0)
#[derive(Debug, Clone, PartialEq)]
pub struct MateInfoLayout {
    pub panel: Option<Img>,
    /// A filled box instead of the image (the fallback layout).
    pub fill: Option<([i32; 4], [u8; 4])>,
    pub health: Option<(TextStyle, [i32; 2])>,
}

/// Every element of the in-match HUD.
#[derive(Debug, Clone, PartialEq)]
pub struct HudLayout {
    /// True when the values came from the user's ROM.
    pub from_rom: bool,
    pub health: HealthLayout,
    pub ammo: AmmoLayout,
    pub feed: FeedLayout,
    pub score: ScoreLayout,
    pub clock: ClockLayout,
    pub board: BoardLayout,
    pub damage: DamageLayout,
    pub respawn: RespawnLayout,
    pub chat: ChatLayout,
    /// The waiting banner of 0x45 `waiting_banner`.
    pub waiting: Option<Img>,
    pub intro: Option<IntroLayout>,
    pub result_mark: ResultMarkLayout,
    pub top: TopLayout,
    pub mk: Option<MkLayout>,
    /// The own first-kill announcement.
    pub opener_banner: Option<Announce>,
    pub hisec: HisecLayout,
    pub aim_cross: Option<AimCrossLayout>,
    /// Centre and size of the weapon row's aim picture (`DotSightPos` / `DotSightSize`, CLI-630).
    pub dot_sight: Option<([i32; 2], [i32; 2])>,
    /// The two arcs beside the aim mark (health and clip, a aim cross style of the 2009 client) are drawn.
    pub arcs: bool,
    /// Kill-feed icon per weapon number (weapon-row field read by the line builder, cshell+0x9d4a0 row +0xd4).
    pub weapon_icons: BTreeMap<u16, String>,
    /// Weapon-name image per weapon number (row +0x168, cshell+0x6f1b0).
    pub name_images: BTreeMap<u16, String>,
    /// Names over the players' heads.
    pub marks: MarkLayout,
    /// The team-mate panel beside the aim cross.
    pub mate_info: Option<MateInfoLayout>,
    /// The panel while the own player is dead ([`super::death`]).
    pub death: super::death::DeathLayout,
    /// The low-health colours, pulse and heartbeat of the health block ([`super::vitals`]).
    pub low_health: super::vitals::LowHealthLayout,
    /// The kill-feed marks beside the names ([`super::feedmarks`]).
    pub feed_marks: super::feedmarks::FeedMarkLayout,
    /// The weapon panel after a weapon change ([`super::panel`]; filled by `Hud::from_rom`, which reads the rows).
    pub weapon_panel: Option<super::panel::WeaponPanelLayout>,
    /// The rank icons of the world marks ([`super::rank`]; filled by `Hud::from_rom`).
    pub rank_icons: Option<super::rank::RankIcons>,
    /// The file of the board window's open sound (`None`: silent).
    pub board_sound_file: Option<String>,
    /// client-scoreboard: the board's place, level icon and header columns ([`super::board`]; filled by
    /// `Hud::from_rom`).
    pub board_columns: super::board::BoardColumns,
    /// CLI-1910: the clan mark table the board rows' clan marks are drawn from (filled by `Hud::from_rom`).
    pub clan_marks: crate::menu::clan::ClanMarks,
    /// The chat colours, the chat input element and the system banner ([`super::chat`]; filled by `Hud::from_rom`).
    pub chat_look: super::chat::ChatLook,
}

fn img(v: &HudView<'_>, tex: K, pos: K, size: K) -> Option<Img> {
    Some(Img {
        tex: v.text(tex)?.to_string(),
        pos: v.pair(pos)?,
        size: v.pair(size)?,
    })
}

fn digits(v: &HudView<'_>, prefix: K, pos: [K; 3], size: K) -> Option<Digits> {
    Some(Digits {
        prefix: v.text(prefix)?.to_string(),
        pos: [v.pair(pos[0])?, v.pair(pos[1])?, v.pair(pos[2])?],
        size: v.pair(size)?,
    })
}

const WHITE: [u8; 4] = [255, 255, 255, 255];

/// The round clock's places: minute digits at x 479 and 492, the separator at 505, second digits at 518 and 531, all
/// at y 66, each 16 x 28.
// EVIDENCE: cshell+0xbe230 (loop x = 0x206 .. 0x220 step 0xd: minutes at (x - 0x27, 0x42), separator (0x1f9, 0x42),
// seconds at (x, y), size 0x10 x 0x1c); DISASM: cshell+0xbe300..0xbe3d7
// DISASM: cshell+0xbe2f8 (the stack slot read for the second digits' y at cshell+0xbe3bf holds 0x42: CLI-45 resolved)
pub const CLOCK_MINUTES: [[i32; 2]; 2] = [[479, 66], [492, 66]];
pub const CLOCK_SEPARATOR: [i32; 2] = [505, 66];
pub const CLOCK_SECONDS: [[i32; 2]; 2] = [[518, 66], [531, 66]];
pub const CLOCK_DIGIT_SIZE: [i32; 2] = [16, 28];

/// How long the intro banner stays after its delay (the 0x45 handler passes 4000 ms).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 field c (4000 ms); EVIDENCE: cshell+0xbc7d0 (removed when the time after
// the delay exceeds it)
pub const INTRO_DURATION_MS: u32 = 4000;

/// Half gap of the aim cross (virtual pixels, scaled by the vertical factor) and its colour.
// OBSERVED: capture 20261025_123446_xhair-frozen/driver/rest_1.png and xhair-rust / xhair2
// (1680 x 1050, M4A1 at rest: 1-pixel green bars, the inner ends 7 pixels from the centre on both axes)
// DC-DEFAULT: CLI-77 (proposed) half gap 5 virtual pixels x the vertical scale; pure green (the 2009 colour is a
// profile option; the screenshots show (5, 241, 5))
pub const AIM_CROSS_HALF_GAP: f32 = 5.0;
pub const AIM_CROSS_COLOUR: [u8; 4] = [0, 255, 0, 255];

/// CRes text ids of the HiSec lines ("a HiSec case dropped", "%1 obtained a HiSec case").
// SPEC: HISEC_2009.md §3.2 (res 0x18c1, res 0x18c2)
pub const HISEC_DROP_ID: u32 = 0x18c1;
pub const HISEC_TAKEN_ID: u32 = 0x18c2;

/// CRes text id of the revenge panel's count line.
// DISASM: cshell+0xbb670 (FUN_372a0280(0x566, buffer, 0x208, value) fills the panel's count text)
pub const REVENGE_TEXT_ID: u32 = 0x566;
/// CLI-1031: the sensitivity hotkeys' line ("... changed to %1!d!.").
// EVIDENCE: cshell+0x49060 (CRes 0x4f6)
pub const SENSITIVITY_TEXT_ID: u32 = 0x4f6;
/// The team chat's name ("Team Chat", the TeamChat command's display text in the user's ROM).
// OBSERVED: CRes string 0x543 next to 0x544 (broadcast chat) and the command names 0x546 / 0x547
pub const TEAM_CHAT_NAME_ID: u32 = 0x543;

/// CRes text ids of the join and leave notices.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2 row 0x6c (0xbb9 leave), §3.2.1 effects (0xbb8 with kind 0)
pub const NOTICE_JOIN_ID: u32 = 0xbb8;
pub const NOTICE_LEAVE_ID: u32 = 0xbb9;

impl MarkLayout {
    /// Our own caption style and colours (no ROM).
    // DC-DEFAULT: CLI-116 (proposed) the no-ROM mark colours: mates light blue, enemies red, the dead grey, hits white
    pub fn fallback() -> MarkLayout {
        MarkLayout {
            style: TextStyle {
                font: -1,
                colour: [255, 255, 255, 255],
                size: None,
            },
            mate: [110, 190, 255, 255],
            enemy: [255, 80, 60, 255],
            dead: [150, 150, 150, 255],
            hit: [255, 255, 255, 255],
            hit_secs: 0.0,
            fade_secs: 0.0,
        }
    }
}

impl HudLayout {
    /// The layout from the HUD set `set` of the user's ROM tables.
    pub fn from_rom(
        table: &LayoutTable,
        keys: &HudKeys,
        set: u32,
        strings: Option<&UiStrings>,
        weapon_rows: &BTreeMap<u16, (Option<String>, Option<String>)>,
    ) -> HudLayout {
        let v = HudView::new(table, keys, set);
        let label = |id: K, pos: K| -> Option<(String, [i32; 2])> {
            let id = v.int(id)?;
            let s = strings?.get(id as u32)?;
            Some((s.to_string(), v.pair(pos)?))
        };
        let health = HealthLayout {
            health_icon: img(&v, K::HealthIcon, K::HealthIconPos, K::StatusIconSize),
            armour_icon: img(&v, K::ArmourIcon, K::ArmourIconPos, K::StatusIconSize),
            health_digits: digits(
                &v,
                K::StatusDigits,
                [K::HealthHundredsPos, K::HealthTensPos, K::HealthOnesPos],
                K::StatusDigitSize,
            ),
            armour_digits: digits(
                &v,
                K::StatusDigits,
                [K::ArmourHundredsPos, K::ArmourTensPos, K::ArmourOnesPos],
                K::StatusDigitSize,
            ),
            stance_stand: img(&v, K::StanceStand, K::StancePos, K::StanceSize),
            stance_crouch: img(&v, K::StanceCrouch, K::StancePos, K::StanceSize),
            stamina_back: img(&v, K::StaminaBack, K::StaminaBackPos, K::StaminaBackSize),
            stamina_cells: img(&v, K::StaminaCells, K::StaminaCellsPos, K::StaminaCellsSize),
            health_arc: Some(StepGauge {
                prefix: v.text(K::HealthArc).unwrap_or_default().to_string(),
                pos: v.pair(K::HealthArcPos).unwrap_or_default(),
                size: v.pair(K::HealthArcSize).unwrap_or_default(),
            })
            .filter(|g| !g.prefix.is_empty()),
            text: None,
            bars: None,
        };
        let ammo = AmmoLayout {
            clip: digits(
                &v,
                K::ClipDigits,
                [K::ClipHundredsPos, K::ClipTensPos, K::ClipOnesPos],
                K::ClipDigitSize,
            ),
            slash: img(&v, K::AmmoSlash, K::AmmoSlashPos, K::AmmoSlashSize),
            reserve: digits(
                &v,
                K::ReserveDigits,
                [K::ReserveHundredsPos, K::ReserveTensPos, K::ReserveOnesPos],
                K::ReserveDigitSize,
            ),
            fire_single: img(&v, K::FireModeSingle, K::FireModePos, K::FireModeSize),
            fire_auto: img(&v, K::FireModeAuto, K::FireModePos, K::FireModeSize),
            clip_arc: Some(StepGauge {
                prefix: v.text(K::ClipArc).unwrap_or_default().to_string(),
                pos: v.pair(K::ClipArcPos).unwrap_or_default(),
                size: v.pair(K::ClipArcSize).unwrap_or_default(),
            })
            .filter(|g| !g.prefix.is_empty()),
            name_image: v.pair(K::WeaponNamePos).zip(v.pair(K::WeaponNameSize)),
            text: None,
        };
        let feed = FeedLayout {
            font: v.int(K::FeedFont).unwrap_or(-1),
            pos: v.pair(K::FeedPos).unwrap_or_default(),
            line_step: v.int(K::FeedLineStep).unwrap_or(0),
            lifetime_ms: v.int(K::FeedLifetime).unwrap_or(0).max(0) as u32,
            weapon_size: v.pair(K::FeedWeaponSize).unwrap_or_default(),
            weapon_lift: v.int(K::FeedWeaponLift).unwrap_or(0),
            head_icon: v.text(K::FeedHeadIcon).map(str::to_string),
            head_size: v.pair(K::FeedHeadSize).unwrap_or_default(),
            head_lift: v.int(K::FeedHeadLift).unwrap_or(0),
            mate_colour: v
                .rgba(K::SameTeamColour, Some(K::SameTeamAlpha))
                .unwrap_or(WHITE),
            opponent_colour: v
                .rgba(K::OtherTeamColour, Some(K::OtherTeamAlpha))
                .unwrap_or(WHITE),
        };
        let score = ScoreLayout {
            team1: digits(
                &v,
                K::ScoreDigits,
                [K::Team1HundredsPos, K::Team1TensPos, K::Team1OnesPos],
                K::Team1DigitSize,
            ),
            team2: digits(
                &v,
                K::ScoreDigits,
                [K::Team2HundredsPos, K::Team2TensPos, K::Team2OnesPos],
                K::Team2DigitSize,
            ),
            separator: img(
                &v,
                K::ScoreSeparator,
                K::ScoreSeparatorPos,
                K::ScoreSeparatorSize,
            ),
            team1_mark: img(&v, K::Team1Mark, K::Team1MarkPos, K::Team1MarkSize),
            team2_mark: img(&v, K::Team2Mark, K::Team2MarkPos, K::Team2MarkSize),
            text: None,
        };
        let clock = ClockLayout {
            digit_form: keys.literal(HudLiteral::ClockDigits).to_string(),
            separator: keys.literal(HudLiteral::ClockSeparator).to_string(),
            minutes: CLOCK_MINUTES,
            separator_pos: CLOCK_SEPARATOR,
            seconds: CLOCK_SECONDS,
            size: CLOCK_DIGIT_SIZE,
            // EVIDENCE: cshell+0x51a50 (colour and alpha keys) passed by the 0x44 handler to cshell+0xbe0c0
            tint: v
                .rgba(K::RoundTimerColour, Some(K::RoundTimerAlpha))
                .unwrap_or(WHITE),
            text: None,
        };
        let board = BoardLayout {
            window: img(&v, K::BoardTex, K::BoardPos, K::BoardSize),
            panel: None,
            kill_label: label(K::BoardKillLabel, K::BoardKillLabelPos),
            death_label: label(K::BoardDeathLabel, K::BoardDeathLabelPos),
            rows: v.pair(K::BoardRowsPos).unwrap_or_default(),
            row_step: v.int(K::BoardRowStep).unwrap_or(0),
            team_offset: v.int(K::BoardTeamOffset).unwrap_or(0),
            font: v.int(K::BoardFont).unwrap_or(-1),
            font_size: v.int(K::BoardFontSize).filter(|n| *n > 0),
            name: v.pair(K::BoardNamePos).unwrap_or_default(),
            kills: v.pair(K::BoardKillsPos).unwrap_or_default(),
            deaths: v.pair(K::BoardDeathsPos).unwrap_or_default(),
            own: v.rgba(K::BoardOwnColour, None).unwrap_or(WHITE),
            alive: v.rgba(K::BoardAliveColour, None).unwrap_or(WHITE),
            dead: v.rgba(K::BoardDeadColour, None).unwrap_or(WHITE),
        };
        let damage = DamageLayout {
            tex: v.text(K::DamageArc).map(str::to_string),
            pos: v.pair(K::DamageArcPos).unwrap_or_default(),
            size: v.pair(K::DamageArcSize).unwrap_or_default(),
            uv: v.pair(K::DamageArcUv).unwrap_or_default(),
            colour: WHITE,
        };
        let respawn = RespawnLayout {
            title: img(&v, K::RespawnTitle, K::RespawnTitlePos, K::RespawnTitleSize),
            // the 2009 client never draws a countdown number: the three accessors of these keys (cshell+0x56ba0 /
            // 0x56c60 / 0x56cd0) have no caller, and the element's layout (cshell+0xcc7e0) and draw (cshell+0xcc4e0)
            // use only the title, message, back, gauge and gauge caps (see `super::death`)
            // DISASM: cshell+0x56ba0..0x56d37 (accessors without a call or pointer reference in the image)
            number: None,
            note: img(&v, K::RespawnNote, K::RespawnNotePos, K::RespawnNoteSize),
            bar: img(&v, K::RespawnBar, K::RespawnBarPos, K::RespawnBarSize),
            fill: img(&v, K::RespawnFill, K::RespawnFillPos, K::RespawnFillSize),
            // EVIDENCE: cshell+0xcc7e0 (the gauge's left and right caps, each its texture, position and size)
            left: img(&v, K::RespawnLeft, K::RespawnLeftPos, K::RespawnLeftSize),
            right: img(&v, K::RespawnRight, K::RespawnRightPos, K::RespawnRightSize),
            text: None,
        };
        let chat = ChatLayout {
            font: v.int(K::ChatLineFont).unwrap_or(-1),
            pos: v.pair(K::ChatPos).unwrap_or_default(),
            line_step: v.int(K::ChatLineStep).unwrap_or(0),
            // DC-DEFAULT: CLI-52 (proposed) notices in the friendly chat colour (the system line colour is not read)
            notice: v.rgba(K::FriendlyColour, None).unwrap_or(WHITE),
            join_form: strings
                .and_then(|s| s.get(NOTICE_JOIN_ID))
                .unwrap_or("%1")
                .to_string(),
            leave_form: strings
                .and_then(|s| s.get(NOTICE_LEAVE_ID))
                .unwrap_or("%1")
                .to_string(),
            case_drop_form: strings
                .and_then(|s| s.get(HISEC_DROP_ID))
                .unwrap_or_default()
                .to_string(),
            case_taken_form: strings
                .and_then(|s| s.get(HISEC_TAKEN_ID))
                .unwrap_or("%1")
                .to_string(),
            kicked_form: strings
                .and_then(|s| s.get(super::chat::KICKED_ID))
                .unwrap_or_default()
                .to_string(),
            same_line_form: strings
                .and_then(|s| s.get(super::chat::REPEAT_NOTICE_ID))
                .unwrap_or_default()
                .to_string(),
            sensitivity_form: strings
                .and_then(|s| s.get(SENSITIVITY_TEXT_ID))
                .unwrap_or_default()
                .to_string(),
            team_chat_name: strings
                .and_then(|s| s.get(TEAM_CHAT_NAME_ID))
                .unwrap_or(super::chat::TEAM_CHAT_FALLBACK)
                .to_string(),
            revenge_form: strings
                .and_then(|s| s.get(REVENGE_TEXT_ID))
                .unwrap_or("%1 x")
                .to_string(),
        };
        let waiting = img(
            &v,
            K::WaitingBannerTex,
            K::WaitingBannerPos,
            K::WaitingBannerSize,
        );
        let ms = |k: K| v.int(k).unwrap_or(0).max(0) as u32;
        let intro = Some(IntroLayout {
            head_pos: v.pair(K::IntroHeadPos).unwrap_or_default(),
            head_size: v.pair(K::IntroHeadSize).unwrap_or_default(),
            body_y: v.pair(K::IntroBodyPos).unwrap_or_default()[1],
            head_scale: v.int(K::IntroHeadScale).unwrap_or(100) as f32 * 0.01,
            body_scale: v.int(K::IntroBodyScale).unwrap_or(100) as f32 * 0.01,
            fade_in: ms(K::IntroFadeIn),
            hold: ms(K::IntroHold),
            slide_end: ms(K::IntroSlideEnd),
            delay: ms(K::IntroDelay),
            slide: v.int(K::IntroSlide).unwrap_or(0) as f32,
            duration: INTRO_DURATION_MS,
        });
        let result_mark = ResultMarkLayout {
            pos: v.pair(K::ResultMarkPos).unwrap_or_default(),
            size: v.pair(K::ResultMarkSize).unwrap_or_default(),
            win: v.text(K::ResultWinTex).map(str::to_string),
            lose: v.text(K::ResultLoseTex).map(str::to_string),
            draw: v.text(K::ResultDrawTex).map(str::to_string),
            score: v.text(K::ResultScoreTex).map(str::to_string),
            human_win: v.text(K::QrtHumanWinTex).map(str::to_string),
            infector_win: v.text(K::QrtInfectorWinTex).map(str::to_string),
        };
        let pair_img = |pos: K, size: K| -> Option<Img> {
            Some(Img {
                tex: String::new(),
                pos: v.pair(pos)?,
                size: v.pair(size)?,
            })
        };
        let word = |tex: K, pos: K| -> Option<Img> {
            Some(Img {
                tex: format!("{}.dtx", v.text(tex)?),
                pos: v.pair(pos)?,
                size: v.pair(K::FfaWordSize)?,
            })
        };
        let ffa = FfaTop {
            title: v.text(K::FfaTitleTex).and_then(|t| {
                Some(Img {
                    tex: keys.literal(HudLiteral::FfaTitle).replacen("%s", t, 1),
                    pos: v.pair(K::FfaTitlePos)?,
                    size: v.pair(K::FfaTitleSize)?,
                })
            }),
            own_score: img(&v, K::FfaMyScoreTex, K::FfaMyScorePos, K::FfaMyScoreSize),
            kills: pair_img(K::FfaKillTensPos, K::Team1DigitSize)
                .zip(pair_img(K::FfaKillOnesPos, K::Team1DigitSize))
                .map(|(a, b)| [a, b]),
            kill_word: word(K::FfaKillWordTex, K::FfaKillWordPos),
            deaths: pair_img(K::FfaDeathTensPos, K::Team1DigitSize)
                .zip(pair_img(K::FfaDeathOnesPos, K::Team1DigitSize))
                .map(|(a, b)| [a, b]),
            death_word: word(K::FfaDeathWordTex, K::FfaDeathWordPos),
            digit_prefix: v.text(K::ScoreDigits).unwrap_or_default().to_string(),
            title_prefix: v.text(K::FfaTitleTex).unwrap_or_default().to_string(),
        };
        let top = TopLayout {
            tdm_title: img(&v, K::ModeTdmTex, K::ModeTdmPos, K::ModeTdmSize),
            goal: digits(
                &v,
                K::ScoreDigits,
                [K::GoalHundredsPos, K::GoalTensPos, K::GoalOnesPos],
                K::GoalDigitSize,
            ),
            goal_word: img(&v, K::GoalWordTex, K::GoalWordPos, K::GoalWordSize),
            team1_base: img(&v, K::Team1BaseMark, K::Team1MarkPos, K::Team1MarkSize),
            team2_base: img(&v, K::Team2BaseMark, K::Team2MarkPos, K::Team2MarkSize),
            own_bar: v.text(K::OwnTeamBarTex).and_then(|t| {
                Some(OwnBar {
                    tex: t.to_string(),
                    team1: v.pair(K::OwnTeamBarPos1)?,
                    team2: v.pair(K::OwnTeamBarPos2)?,
                    size: v.pair(K::OwnTeamBarSize)?,
                })
            }),
            ffa,
            round: super::modetop::RoundTop::load(&v),
            flag: super::modetop::FlagTop::load(&v),
            team_change: super::teamchange::TeamChangeLayout::load(&v),
            bomb: super::bomb::BombLayout::load(&v),
            qrt: super::qrt::QrtLayout::load(&v),
            spy: super::spy::SpyLayout::load(&v),
            mission: super::mission::MissionLayout::load(&v),
            coop: super::coopscore::CoopScoreLayout::load(&v),
            prompt: super::prompt::PromptLayout::load(&v),
        };
        let mk = v.pair(K::MkPos).map(|pos| MkLayout {
            font: v.int(K::MkFont).unwrap_or(-1),
            mate: v.rgba(K::MkColour, Some(K::MkAlpha)).unwrap_or(WHITE),
            enemy: v.rgba(K::MkEnemyColour, Some(K::MkAlpha)).unwrap_or(WHITE),
            opener_img: v.text(K::MkFirstKillImg).map(str::to_string),
            hisec: v.text(K::MkHisecImg).map(str::to_string),
            flag_imgs: [
                K::MkTeamTakeFlagImg,
                K::MkTeamReturnFlagImg,
                K::MkTeamCaptureFlagImg,
                K::MkEnemyTakeFlagImg,
                K::MkEnemyReturnFlagImg,
                K::MkEnemyCaptureFlagImg,
            ]
            .map(|k| v.text(k).map(str::to_string)),
            pos,
            gap: v.int(K::MkImageGap).unwrap_or(0),
            lift: v.int(K::MkImageLift).unwrap_or(0),
            size: v.pair(K::MkSize).unwrap_or_default(),
            line_step: v.int(K::MkLineStep).unwrap_or(0),
            lifetime_ms: ms(K::MkLifetime),
        });
        let secs = |k: K| v.real(k).unwrap_or(0.0) as f32;
        let opener_banner = v.text(K::ComboFirstKillImg).map(|t| Announce {
            tex: t.to_string(),
            size: v.pair(K::ComboFirstKillSize).unwrap_or_default(),
            centre: v.pair(K::ComboPos).unwrap_or_default(),
            total: secs(K::ComboTotal),
            create: secs(K::ComboCreate),
            end: secs(K::ComboEnd),
            destroy: secs(K::ComboDestroy),
            first_scale: secs(K::ComboStartSize),
        });
        let hisec = HisecLayout {
            indicator: v
                .text(K::HisecIndicatorTex)
                .map(str::to_string)
                .zip(v.pair(K::HisecIndicatorSize)),
            pickup: v.text(K::HisecPickupImg).map(|t| Announce {
                tex: t.to_string(),
                size: v.pair(K::HisecPickupSize).unwrap_or_default(),
                centre: v.pair(K::HisecPickupPos).unwrap_or_default(),
                total: secs(K::HisecPickupTotal),
                create: secs(K::HisecPickupCreate),
                end: secs(K::HisecPickupEnd),
                destroy: secs(K::HisecPickupDestroy),
                first_scale: secs(K::HisecPickupStartSize),
            }),
        };
        // EVIDENCE: cshell+0x857e0 (the aim cross init reads the bar length through cshell+0x1c3670, 5.0 when absent)
        let aim_cross = Some(AimCrossLayout {
            bar: v.real(K::CrosshairBar).filter(|b| *b > 0.0).unwrap_or(5.0) as f32,
            half_gap: AIM_CROSS_HALF_GAP,
            colour: AIM_CROSS_COLOUR,
        });
        // the marker's colours are opaque RGB (cshell+0x5a7f0 packs alpha 0xff); its delays are seconds
        // DC-DEFAULT: CLI-116 (proposed) a colour key the ROM lacks falls back to the fallback layout's colour
        let fb = MarkLayout::fallback();
        let opaque = |k: K, d: [u8; 4]| v.rgba(k, None).unwrap_or(d);
        let marks = MarkLayout {
            style: TextStyle {
                font: MARK_FONT,
                colour: [255, 255, 255, 255],
                size: Some(MARK_FONT_SIZE),
            },
            mate: opaque(K::MarkMateColour, fb.mate),
            enemy: opaque(K::MarkEnemyColour, fb.enemy),
            dead: opaque(K::MarkDeadColour, fb.dead),
            hit: opaque(K::MarkHitColour, fb.hit),
            hit_secs: v.real(K::MarkHitTime).unwrap_or(0.0) as f32,
            fade_secs: v.real(K::MarkFadeTime).unwrap_or(0.0) as f32,
        };
        let mate_info = Some(MateInfoLayout {
            panel: img(&v, K::MateInfoTex, K::MateInfoPos, K::MateInfoSize),
            fill: None,
            health: v.pair(K::MateHealthPos).map(|at| {
                (
                    TextStyle {
                        font: v.int(K::MateHealthFont).unwrap_or(0),
                        colour: v
                            .rgba(K::MateHealthColour, Some(K::MateHealthAlpha))
                            .unwrap_or([255, 255, 255, 255]),
                        size: v.int(K::MateHealthFontSize).filter(|n| *n > 0),
                    },
                    at,
                )
            }),
        });
        let weapon_icons = weapon_rows
            .iter()
            .filter_map(|(w, (i, _))| i.clone().map(|i| (*w, i)))
            .collect();
        let name_images = weapon_rows
            .iter()
            .filter_map(|(w, (_, n))| n.clone().map(|n| (*w, n)))
            .collect();
        HudLayout {
            from_rom: true,
            health,
            ammo,
            feed,
            score,
            clock,
            board,
            damage,
            respawn,
            chat,
            waiting,
            intro,
            result_mark,
            top,
            mk,
            opener_banner,
            hisec,
            aim_cross,
            // EVIDENCE: cshell+0x83260 (the picture's quad: x = trunc((pos - size / 2) x screen scale), width
            // = trunc(size x scale) + 1, from the set's DotSightPos / DotSightSize)
            dot_sight: v.pair(K::DotSightPos).zip(v.pair(K::DotSightSize)),
            // OBSERVED: capture 20261025_123446_xhair-frozen/driver/rest_1.png (and xhair-rust,
            // xhair2-*: health 100, clip 30 / 30, alive) show no arc beside the aim mark
            // DC-DEFAULT: CLI-56 (amended, proposed) the arcs are off unless a aim cross style turns them on
            arcs: false,
            weapon_icons,
            name_images,
            marks,
            mate_info,
            death: super::death::DeathLayout::default(),
            low_health: super::vitals::LowHealthLayout::from_view(&v),
            feed_marks: super::feedmarks::FeedMarkLayout::from_view(&v),
            weapon_panel: None,
            rank_icons: None,
            board_sound_file: None,
            board_columns: super::board::BoardColumns::default(),
            clan_marks: Default::default(),
            chat_look: super::chat::ChatLook::fallback(),
        }
    }

    /// Our own layout for runs without a ROM: text and plain boxes, no textures. Every number here is a DeCombat
    /// choice (virtual pixels of the 1024 x 768 screen).
    // DC-DEFAULT: CLI-44 (proposed) the no-ROM HUD layout
    pub fn fallback() -> HudLayout {
        let light = TextStyle {
            font: -1,
            colour: [240, 240, 230, 255],
            size: None,
        };
        HudLayout {
            from_rom: false,
            health: HealthLayout {
                text: Some(([24, 728], [150, 728], light)),
                bars: Some(StatusBars {
                    health: [24, 714, 110, 8],
                    armour: [150, 714, 110, 8],
                    health_colour: [220, 60, 50, 230],
                    armour_colour: [90, 150, 230, 230],
                }),
                ..HealthLayout::default()
            },
            ammo: AmmoLayout {
                text: Some(([880, 728], light)),
                ..AmmoLayout::default()
            },
            feed: FeedLayout {
                font: -1,
                pos: [16, 40],
                line_step: 22,
                lifetime_ms: 20_000,
                weapon_size: [40, 16],
                weapon_lift: 0,
                head_icon: None,
                head_size: [0, 0],
                head_lift: 0,
                mate_colour: [150, 200, 255, 255],
                opponent_colour: [255, 120, 80, 255],
            },
            score: ScoreLayout {
                team1: None,
                team2: None,
                separator: None,
                team1_mark: None,
                team2_mark: None,
                text: Some(([468, 12], light)),
            },
            clock: ClockLayout {
                digit_form: String::new(),
                separator: String::new(),
                minutes: CLOCK_MINUTES,
                separator_pos: CLOCK_SEPARATOR,
                seconds: CLOCK_SECONDS,
                size: CLOCK_DIGIT_SIZE,
                tint: WHITE,
                text: Some(([486, 40], light)),
            },
            board: BoardLayout {
                window: None,
                panel: Some(([112, 120, 800, 480], [10, 14, 20, 200])),
                kill_label: Some(("K".into(), [292, 12])),
                death_label: Some(("D".into(), [332, 12])),
                rows: [16, 40],
                row_step: 24,
                team_offset: 400,
                font: -1,
                font_size: None,
                name: [0, 0],
                kills: [292, 0],
                deaths: [332, 0],
                own: [255, 200, 90, 255],
                alive: [235, 235, 235, 255],
                dead: [110, 110, 110, 255],
            },
            damage: DamageLayout {
                tex: None,
                pos: [452, 260],
                size: [120, 14],
                uv: [120, 14],
                colour: [220, 30, 20, 255],
            },
            respawn: RespawnLayout {
                text: Some(([440, 420], light)),
                ..RespawnLayout::default()
            },
            chat: ChatLayout {
                font: -1,
                pos: [16, 560],
                line_step: 20,
                notice: [230, 230, 160, 255],
                join_form: "%1 joined".into(),
                leave_form: "%1 left".into(),
                case_drop_form: "A HiSec case dropped".into(),
                case_taken_form: "%1 obtained a HiSec case".into(),
                revenge_form: "%1 times by".into(),
                // DC-DEFAULT: CLI-493 (proposed) no-ROM texts of the kick line and the repeat refusal
                kicked_form: "%1 was kicked".into(),
                same_line_form: "Repeated line: wait a minute".into(),
                // DC-DEFAULT: CLI-1031 (proposed) the no-ROM text of the sensitivity line
                sensitivity_form: "Mouse sensitivity %1".into(),
                team_chat_name: super::chat::TEAM_CHAT_FALLBACK.into(),
            },
            waiting: None,
            intro: None,
            result_mark: ResultMarkLayout::default(),
            top: TopLayout::default(),
            mk: None,
            opener_banner: None,
            hisec: HisecLayout::default(),
            // DC-DEFAULT: CLI-630 (proposed) without a ROM layout the aim picture is 32 x 32 at the centre
            dot_sight: Some(([512, 384], [32, 32])),
            aim_cross: Some(AimCrossLayout {
                bar: 8.0,
                half_gap: AIM_CROSS_HALF_GAP,
                colour: AIM_CROSS_COLOUR,
            }),
            arcs: false,
            weapon_icons: BTreeMap::new(),
            name_images: BTreeMap::new(),
            marks: MarkLayout::fallback(),
            mate_info: Some(MateInfoLayout {
                panel: None,
                fill: Some(([522, 394, 44, 6], [110, 190, 255, 220])),
                health: Some((light, [570, 388])),
            }),
            death: super::death::DeathLayout::fallback(),
            // DC-DEFAULT: CLI-280 (proposed) no ROM: the 2009 constructor's steady bright red, no pulse, no heartbeat
            low_health: super::vitals::LowHealthLayout::default(),
            // DC-DEFAULT: CLI-281 (proposed) no ROM: no mark images (the marks only move the victim name by nothing)
            feed_marks: super::feedmarks::FeedMarkLayout::default(),
            // DC-DEFAULT: CLI-282 (proposed) no ROM: no weapon panel (its pictures are ROM images)
            weapon_panel: None,
            // DC-DEFAULT: CLI-283 (proposed) no ROM: no rank icons
            rank_icons: None,
            board_sound_file: None,
            // DC-DEFAULT: CLI-382 (proposed) no ROM: no place numbers, level icons or extra header labels
            board_columns: super::board::BoardColumns::default(),
            clan_marks: Default::default(),
            chat_look: super::chat::ChatLook::fallback(),
        }
    }
}

#[cfg(test)]
mod mark_colour_tests {
    use super::*;
    use dc_ui_data::UiNames;

    /// The mark colours read from a layout text in the ROM's value form (`<r,g,b>`, real components): the channels
    /// come out as written with alpha 0xff, a key the text lacks keeps our colour. The numbers are invented.
    // EVIDENCE: cshell+0x8b110 (five colours of layout set 0), cshell+0x5a7f0 (alpha 0xff), cshell+0x514d0 (colour
    // vector: three reals, each truncated to a byte)
    #[test]
    fn mark_colours_parse_from_the_layout_value_form() {
        let keys = HudKeys::placeholder();
        let names = UiNames::placeholder();
        let set0 = names.hud_set0.clone();
        let text = format!(
            "[{set0}]\n{} = <12.0,34.0,56.0>\n{} = <200.0, 100.0, 50.5>\n{} = <1.0,2.0,3.0>\n{} = 1.5\n{} = 0.25\n",
            keys.key(K::MarkMateColour),
            keys.key(K::MarkEnemyColour),
            keys.key(K::MarkDeadColour),
            keys.key(K::MarkHitTime),
            keys.key(K::MarkFadeTime),
        );
        let table = LayoutTable::parse(&text, names);
        let l = HudLayout::from_rom(&table, &keys, 0, None, &BTreeMap::new());
        let fb = MarkLayout::fallback();
        assert_eq!(l.marks.mate, [12, 34, 56, 255]);
        assert_eq!(l.marks.enemy, [200, 100, 50, 255], "components truncate");
        assert_eq!(l.marks.dead, [1, 2, 3, 255]);
        assert_eq!(l.marks.hit, fb.hit, "an absent key keeps our colour");
        assert_eq!((l.marks.hit_secs, l.marks.fade_secs), (1.5, 0.25));
        // a layout without any of the keys is exactly the fallback colours
        let empty = LayoutTable::parse("", UiNames::placeholder());
        let l = HudLayout::from_rom(&empty, &keys, 0, None, &BTreeMap::new());
        assert_eq!(
            (l.marks.mate, l.marks.enemy, l.marks.dead),
            (fb.mate, fb.enemy, fb.dead)
        );
    }
}
