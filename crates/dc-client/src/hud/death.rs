//! The HUD while the own player is dead, as the 2009 client draws it.
//!
//! What the 2009 client does (all read from the clean disassembly of CShell.dll):
//!
//! - **The HUD render level.** The HUD manager draws an element only when the element's layer is not above the
//!   manager's render level (cshell+0x26b0d0..0x26b108: element slot +0x20 = layer, compared with manager +0x10).
//!   The player-state handler sets the level: state 1 -> 3 (cshell+0x1f99f8..0x1f9a09), state 2 -> 2
//!   (cshell+0x1f9a89..0x1f9aa4), state 3 -> 2 (the derived handler, cshell+0x61980..0x61991), state 5 -> 0
//!   (cshell+0x61a64..0x61a74), state 6 -> 2 (cshell+0x61aeb..0x61afc). The aim cross with its damage arcs and
//!   team-mate panel (ctor cshell+0x852b0), the ammunition block (cshell+0x6ea50), the health block (cshell+0x97200)
//!   and the radar (cshell+0xb9140) are layer 3, so a dead player sees none of them; the death panel below is layer 2
//!   (cshell+0xca8e0); the respawn wait, round clock, chat and name marks are layer 1 (hidden at level 0, life state
//!   5); the kill feed and scoreboard are layer 0 and stay.
//! - **The death panel** (HUD element 15 with ctor cshell+0xca8e0, layout cshell+0xca9a0, draw
//!   cshell+0xcbb00, update cshell+0xcbea0): a dark band at the bottom with the "user information" frame (the
//!   observed player's rank icon, name, soldier icon, HP and AP as three digit images each), the "damaged area" frame
//!   (the body-zone silhouette of the last hit, the word image and the last damage as three digits) and the
//!   change-view hint. Every texture, place and size is a key of one named layout section, read at run time.
//!   It is drawn while the own state is 2, a target record is bound and the observer camera is not the killer camera
//!   (cshell+0xcbb00: state == 2, element +0x18 non-zero, `local_player`+0x428 != 4).
//! - **When it appears.** After a death whose killer is known the client runs the killer camera (mode 4) until the
//!   engine value "dying time" (default 3500 ms, cshell+0x5e725..0x5e73d) has passed since the death time of 0x67
//!   state 2 (cshell+0x60c0f..0x60c5f), then switches to the follow camera (mode 2, cshell+0x60c69); the panel
//!   therefore appears 3.5 s after such a death. Without a known killer the follow camera starts at once
//!   (cshell+0x60b1a..0x60b29). The target is the own record (state 3 binds the own object, cshell+0x619a2..0x619b4;
//!   mode 2 binds the followed record, which at that moment is the own one, cshell+0x60af0..0x60b09 and
//!   cshell+0x60680 case 2) until the follow camera takes another player (cshell+0x60010 -> cshell+0xc9e20): then the
//!   panel shows that player's record and the health and armour of his type-0x18 notes (CLI-128).
//! - **The damage frame** shows the last 0x74 received while alive: `damage` (clamped to 1..=999,
//!   cshell+0xcc23f..0xcc257 and cshell+0xca400) and the zone image chosen by the 0x74 `dmg_kind` (1 head, 3 arms, 4 legs,
//!   5 groin, anything else the body; cshell+0xca5c0); 0x74 stores both (cshell+0x1f7e33 / cshell+0x1f7e3d) and is
//!   ignored while dead.
//! - **No respawn countdown digit.** The respawn wait element (cshell+0xcd0c0, layout cshell+0xcc7e0, draw
//!   cshell+0xcc4e0) draws its title, message, back, gauge and the two gauge caps, never a number: the three layout
//!   accessors of the countdown number keys (cshell+0x56ba0 / 0x56c60 / 0x56cd0) have no caller and no pointer to
//!   them exists in the image. The title's alpha swings between two layout bounds once per gauge tick
//!   (cshell+0xcc520).
// SPEC: FMT_CRES_UI.md §8.1 (element 15, update slot 4; elements 21, 33-36), GAME_MESSAGES_2009_S2C.md §4.1, §4.3
// SPEC: GAME_MESSAGES_2009_C2S.md §3.2-§3.5 (life states, the respawn wait, the observer camera modes)
// OBSERVED: reports/sidebyside3/pairs, sidebyside4/pairs (the 2009 client on JUNK_FLEA killed by
// a bot: ammunition, health and radar gone at the death, the panel 3-4 s later until the respawn, no digit)

use super::layout::{Digits, Img, TextStyle};
use super::state::HudState;
use super::DrawItem;
use dc_ui_data::layout::parse_value;
use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::{LayoutTable, Value};
use std::path::Path;

/// Milliseconds the killer camera runs after a death with a known killer (the engine value's default).
// DISASM: cshell+0x5e725..0x5e73d (console value registered with default 0x455ac000 = 3500.0), cshell+0x60c0f..0x60c5f
// (elapsed ms since `local_player`+0x440 compared with the value, mode 4 kept while below)
pub const DYING_TIME_MS: u32 = 3500;

/// The HUD render level of a life state: the HUD draws an element only when its layer is at most this.
// DISASM: cshell+0x1f99f8..0x1f9a09 (state 1: 3), cshell+0x1f9a89..0x1f9aa4 (state 2: 2), cshell+0x61980..0x61991
// (state 3: 2), cshell+0x61a64..0x61a74 (state 5: 0), cshell+0x61aeb..0x61afc (state 6: 2)
// DC-DEFAULT: CLI-125 (proposed) before the first 0x67 (state 0) and for unknown states the level is 3
pub fn hud_level(life_state: u8) -> u8 {
    match life_state {
        2 | 3 | 6 => 2,
        5 => 0,
        _ => 3,
    }
}

/// Layer of the aim cross, health and ammunition blocks (and the damage arcs and team-mate panel of the aim cross).
// EVIDENCE: cshell+0x852b0, cshell+0x6ea50, cshell+0x97200 (each constructor stores 3 at +0x08)
pub const LIVE_LAYER: u8 = 3;
/// Layer of the death panel.
// EVIDENCE: cshell+0xca8e0 (constructor stores 2 at +0x08); cshell+0x619c8..0x619ce (state 2 stores it again)
pub const PANEL_LAYER: u8 = 2;

/// True when the layer-3 elements (aim cross, damage arcs, health, ammunition) are drawn.
pub fn live_widgets(s: &HudState) -> bool {
    hud_level(s.life_state) >= LIVE_LAYER
}

/// True while the killer camera runs: a known killer and less than [`DYING_TIME_MS`] since the death time.
// DISASM: cshell+0x60b0f..0x60b1e (mode 4 when the last killer id is not negative), cshell+0x60c0f..0x60c5f
// DC-DEFAULT: CLI-126 (proposed) "known killer" = the own death note names a killer (a kill by the world, id 0xff,
// starts no killer camera in dc-client; the 2009 client stores the raw id there)
pub fn killer_camera(s: &HudState) -> bool {
    let Some(at) = s.death_at else {
        return false;
    };
    s.death.as_ref().is_some_and(|d| d.killer.is_some())
        && s.now_ms.saturating_sub(at) < DYING_TIME_MS
}

/// True when the death panel is drawn this frame.
// EVIDENCE: cshell+0xcbb00 (own state == 2, a bound target, camera mode != 4)
pub fn panel_shown(s: &HudState) -> bool {
    s.life_state == 2
        && s.death_at.is_some()
        && hud_level(s.life_state) >= PANEL_LAYER
        && !killer_camera(s)
}

/// The last 0x74 the own player took while alive: the damage and the location kind the panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WoundNote {
    pub damage: u32,
    pub kind: u8,
}

impl HudState {
    /// 0x74 `damage` / `dmg_kind`: kept for the death panel (ignored while dead, never cleared).
    // DISASM: cshell+0x1f7d20 (returns at states 6, 2, 3, 5; then +0x198 := `damage`, +0x190 := `dmg_kind`)
    pub fn on_wound(&mut self, damage: u32, kind: u8) {
        if matches!(self.life_state, 2 | 3 | 5 | 6) {
            return;
        }
        self.wound = Some(WoundNote { damage, kind });
    }
}

/// The panel's damage number: below 2 shows 1, above 999 shows 999.
// DISASM: cshell+0xcc23f..0xcc257 (`v <= 1` passes 1), cshell+0xca400 (`if v > 999 then 999`)
pub fn shown_damage(v: u32) -> u32 {
    if v < 2 {
        1
    } else {
        v.min(999)
    }
}

/// Which zone image a 0x74 `dmg_kind` selects: 0 head, 1 body, 2 arms, 3 legs, 4 groin.
// EVIDENCE: cshell+0xca5c0 (switch: 1, 2 / default, 3, 4, 5)
pub fn zone_image(kind: u8) -> usize {
    match kind {
        1 => 0,
        3 => 2,
        4 => 3,
        5 => 4,
        _ => 1,
    }
}

// ---- keys --------------------------------------------------------------------------------------------------------

/// The layout keys the death panel reads (our names). None of their texts is in this tree: each is read from the
/// user's CShell.dll at the pinned rva and accepted only when its length and SHA-256 prefix match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PanelKey {
    BandTex,
    BandPos,
    BandSize,
    InfoFrameTex,
    InfoFramePos,
    InfoFrameSize,
    TextFont,
    TextFontSize,
    NamePos,
    RankTex,
    RankPos,
    RankSize,
    SoldierTex,
    SoldierPos,
    SoldierSize,
    HpLabelTex,
    HpLabelPos,
    HpLabelSize,
    ApLabelTex,
    ApLabelPos,
    ApLabelSize,
    DigitPrefix,
    HpOnesPos,
    DigitSize,
    HpTensPos,
    HpHundredsPos,
    ApOnesPos,
    ApTensPos,
    ApHundredsPos,
    DamageOnesPos,
    DamageTensPos,
    DamageHundredsPos,
    DamageFrameTex,
    DamageFramePos,
    DamageFrameSize,
    ZoneHeadTex,
    ZonePos,
    ZoneSize,
    DamageWordTex,
    DamageWordPos,
    DamageWordSize,
    HintTex,
    HintPos,
    HintSize,
    ZoneArmsTex,
    ZoneLegsTex,
    ZoneGroinTex,
    ZoneBodyTex,
    /// The respawn title's alpha bounds and step (HUD layout set keys of the respawn wait element).
    TitleAlphaMax,
    TitleAlphaMin,
    TitleAlphaStep,
    /// w2-board: the clan name's top-left point.
    ClanTextPos,
}

/// `(key, literal rva, sha16, length, evidence rva)`.
// EVIDENCE: cshell+0xca9a0 (layout of the death panel: every key below except the zone images), cshell+0xca5c0 (the
// zone images), cshell+0xcc7e0 (the respawn title's alpha keys)
const PANEL_PINS: [(PanelKey, u32, &str, usize, u32); 52] = [
    (PanelKey::BandTex, 0x3da21c, "e6834a457054b825", 13, 0xca9e3),
    (PanelKey::BandPos, 0x3da20c, "6154dd9c2f3ce940", 13, 0xcaa10),
    (
        PanelKey::BandSize,
        0x3da1fc,
        "013ee373a1b8175d",
        13,
        0xcaa37,
    ),
    (
        PanelKey::InfoFrameTex,
        0x3da1ec,
        "5548a59e7ac9fa91",
        15,
        0xcaa7e,
    ),
    (
        PanelKey::InfoFramePos,
        0x3da1dc,
        "29765d7b9ea94cd5",
        15,
        0xcaaa9,
    ),
    (
        PanelKey::InfoFrameSize,
        0x3da1cc,
        "b11b06f7b14da311",
        15,
        0xcaac4,
    ),
    (PanelKey::TextFont, 0x3da1c0, "01e759a6d7eea5ee", 8, 0xcab0e),
    (
        PanelKey::TextFontSize,
        0x3da1b0,
        "16d88813a84281b9",
        12,
        0xcab2f,
    ),
    (PanelKey::NamePos, 0x3da160, "09cd8f87e8741f6a", 16, 0xcabdf),
    (PanelKey::RankTex, 0x3da14c, "bb3cf78496f9c771", 16, 0xcacc3),
    (PanelKey::RankPos, 0x3da138, "27f321d3a564de51", 16, 0xcace8),
    (
        PanelKey::RankSize,
        0x3da124,
        "f67d26f33d351698",
        16,
        0xcad0f,
    ),
    (
        PanelKey::SoldierTex,
        0x3da0c8,
        "15bc224b022a8ad0",
        19,
        0xcade9,
    ),
    (
        PanelKey::SoldierPos,
        0x3da0b4,
        "7204901c7583eda7",
        19,
        0xcae14,
    ),
    (
        PanelKey::SoldierSize,
        0x3da0a0,
        "dd9d20cbc63cc78b",
        19,
        0xcae2f,
    ),
    (
        PanelKey::HpLabelTex,
        0x3da08c,
        "6f4cc5afc860359b",
        16,
        0xcae7c,
    ),
    (
        PanelKey::HpLabelPos,
        0x3da078,
        "8eff7a69d6f6d1e1",
        16,
        0xcaea7,
    ),
    (
        PanelKey::HpLabelSize,
        0x3da064,
        "fe505ba046676f6a",
        16,
        0xcaec2,
    ),
    (
        PanelKey::ApLabelTex,
        0x3da050,
        "0b860075b1efe632",
        16,
        0xcaf0f,
    ),
    (
        PanelKey::ApLabelPos,
        0x3da03c,
        "bc62c9755da25030",
        16,
        0xcaf3a,
    ),
    (
        PanelKey::ApLabelSize,
        0x3da028,
        "d18574bfeccca66a",
        16,
        0xcaf55,
    ),
    (
        PanelKey::DigitPrefix,
        0x3d9ca4,
        "6cd66f868d8a72c5",
        16,
        0xcafa2,
    ),
    (
        PanelKey::HpOnesPos,
        0x3da014,
        "ab9f0a612ca6a257",
        19,
        0xcafdc,
    ),
    (
        PanelKey::DigitSize,
        0x3da000,
        "12141e613e2697fc",
        16,
        0xcaff7,
    ),
    (
        PanelKey::HpTensPos,
        0x3d9fec,
        "0f6870c269f68787",
        19,
        0xcb045,
    ),
    (
        PanelKey::HpHundredsPos,
        0x3d9fd8,
        "a71a1fbb6409b5ad",
        19,
        0xcb099,
    ),
    (
        PanelKey::ApOnesPos,
        0x3d9fc4,
        "9d59aa0ac66b0118",
        19,
        0xcb0e1,
    ),
    (
        PanelKey::ApTensPos,
        0x3d9fb0,
        "33522dfd7a4aa2dc",
        19,
        0xcb135,
    ),
    (
        PanelKey::ApHundredsPos,
        0x3d9f9c,
        "c18c7107d4baf58b",
        19,
        0xcb17d,
    ),
    (
        PanelKey::DamageOnesPos,
        0x3d9f8c,
        "46c7ea0c8f16d767",
        15,
        0xcb1d1,
    ),
    (
        PanelKey::DamageTensPos,
        0x3d9f7c,
        "cd99a197b3961a03",
        15,
        0xcb219,
    ),
    (
        PanelKey::DamageHundredsPos,
        0x3d9f6c,
        "9ad494cd5a2e4735",
        15,
        0xcb26d,
    ),
    (
        PanelKey::DamageFrameTex,
        0x3d9f58,
        "93d6e887c7f6a604",
        17,
        0xcb2c0,
    ),
    (
        PanelKey::DamageFramePos,
        0x3d9f44,
        "8c6c41127535cf60",
        17,
        0xcb2eb,
    ),
    (
        PanelKey::DamageFrameSize,
        0x3d9f30,
        "6bba49f070a73d0c",
        17,
        0xcb306,
    ),
    (
        PanelKey::ZoneHeadTex,
        0x3d9d18,
        "32269af7b6c361d0",
        22,
        0xcb353,
    ),
    (PanelKey::ZonePos, 0x3d9f1c, "418d54608fb8d950", 18, 0xcb37e),
    (
        PanelKey::ZoneSize,
        0x3d9f08,
        "634c2d3eec06fa64",
        18,
        0xcb399,
    ),
    (
        PanelKey::DamageWordTex,
        0x3d9ef8,
        "897afe65d0e25f7c",
        12,
        0xcb3e6,
    ),
    (
        PanelKey::DamageWordPos,
        0x3d9ee8,
        "aae682770d78725d",
        12,
        0xcb411,
    ),
    (
        PanelKey::DamageWordSize,
        0x3d9ed8,
        "beae01108da7380c",
        12,
        0xcb42c,
    ),
    (PanelKey::HintTex, 0x3d9ebc, "f98364c8085c9d71", 25, 0xcb479),
    (PanelKey::HintPos, 0x3d9ea0, "5c89ae08ee6df771", 25, 0xcb4a4),
    (
        PanelKey::HintSize,
        0x3d9e84,
        "784656dc57db7b65",
        25,
        0xcb4bf,
    ),
    (
        PanelKey::ZoneArmsTex,
        0x3d9d00,
        "ff5953a44893c0c1",
        22,
        0xca62b,
    ),
    (
        PanelKey::ZoneLegsTex,
        0x3d9ce8,
        "a5be00f3e706eecc",
        21,
        0xca637,
    ),
    (
        PanelKey::ZoneGroinTex,
        0x3d9cd0,
        "7ebf3130a7b14223",
        21,
        0xca643,
    ),
    (
        PanelKey::ZoneBodyTex,
        0x3d9cb8,
        "8db1377e1156e44b",
        22,
        0xca64f,
    ),
    (
        PanelKey::TitleAlphaMax,
        0x3da280,
        "593a42d2dfeb4cf9",
        20,
        0xccc8f,
    ),
    (
        PanelKey::TitleAlphaMin,
        0x3da268,
        "d45b331e74f4fb98",
        20,
        0xccca1,
    ),
    (
        PanelKey::TitleAlphaStep,
        0x3da24c,
        "466728d5ec4f5bcd",
        24,
        0xcccbc,
    ),
    // w2-board: the clan text control (created with the panel font, flags 0x11)
    // EVIDENCE: cshell+0xca9a0 (the clan text placed at this key's pair); DISASM: cshell+0xcab7c (the literal)
    (
        PanelKey::ClanTextPos,
        0x3da174,
        "cf47a2ccba6f12f1",
        18,
        0xcab7c,
    ),
];

/// The named layout section of the death panel (pushed with every panel key).
// EVIDENCE: cshell+0xca9e8
const SECTION_PIN: Pin = Pin {
    what: "death panel section",
    rva: 0x3d3118,
    sha16: "50e9a8927fdedb83",
    len: 12,
};

impl PanelKey {
    fn row(self) -> (PanelKey, u32, &'static str, usize, u32) {
        PANEL_PINS[self as usize]
    }

    /// The pinned literal of the key.
    pub fn pin(self) -> Pin {
        let r = self.row();
        Pin {
            what: "death panel key",
            rva: r.1,
            sha16: r.2,
            len: r.3,
        }
    }

    /// Rva of the code that pushes the key.
    pub fn evidence(self) -> u32 {
        self.row().4
    }

    /// The respawn title's keys live in the HUD layout set, the others in the panel's section.
    pub fn in_hud_set(self) -> bool {
        matches!(
            self,
            PanelKey::TitleAlphaMax | PanelKey::TitleAlphaMin | PanelKey::TitleAlphaStep
        )
    }
}

/// The key texts of the user's CShell.dll (or invented names for fixtures).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelKeys {
    section: String,
    keys: Vec<String>,
}

impl PanelKeys {
    /// Reads every key from `Game/CShell.dll` of the client folder `rom`.
    pub fn from_rom(rom: &Path) -> Result<Self, String> {
        let p = rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]);
        let pe = dc_rom::PeFile::from_file(&p).map_err(|e| e.to_string())?;
        let section = read_pinned(&pe, &SECTION_PIN).map_err(|e| e.to_string())?;
        let keys = PANEL_PINS
            .iter()
            .map(|r| read_pinned(&pe, &r.0.pin()).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PanelKeys { section, keys })
    }

    /// Invented names (`panel_key_<n>` in the section `panel_section`) for fixtures without a ROM.
    pub fn placeholder() -> Self {
        PanelKeys {
            section: "panel_section".into(),
            keys: (0..PANEL_PINS.len())
                .map(|i| format!("panel_key_{i}"))
                .collect(),
        }
    }

    pub fn section(&self) -> &str {
        &self.section
    }

    pub fn key(&self, k: PanelKey) -> &str {
        &self.keys[k as usize]
    }
}

// ---- layout ------------------------------------------------------------------------------------------------------

/// The respawn title's alpha swing: start and upper bound, lower bound, step per gauge tick (alpha units).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitleSwing {
    pub max: i32,
    pub min: i32,
    pub step: i32,
}

/// The zone silhouette: place, size and the five images (head, body, arms, legs, groin).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ZoneImages {
    pub pos: [i32; 2],
    pub size: [i32; 2],
    pub tex: [Option<String>; 5],
}

/// The text form of the panel (fallback layout): band rectangle and colour, line style, first line top-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextPanel {
    pub band: [i32; 4],
    pub colour: [u8; 4],
    pub style: TextStyle,
    pub at: [i32; 2],
}

/// Every element of the death panel. Positions and sizes are virtual pixels (1024 x 768).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeathLayout {
    pub band: Option<Img>,
    pub info_frame: Option<Img>,
    pub rank: Option<Img>,
    /// The name text: style and top-left.
    pub name: Option<(TextStyle, [i32; 2])>,
    /// w2-board: the clan text: style (the name's font, size and default colour) and top-left.
    pub clan: Option<(TextStyle, [i32; 2])>,
    pub soldier: Option<Img>,
    pub hp_label: Option<Img>,
    pub hp: Option<Digits>,
    pub ap_label: Option<Img>,
    pub ap: Option<Digits>,
    pub damage_frame: Option<Img>,
    /// The zone silhouette: place, size and the five images (head, body, arms, legs, groin).
    pub zone: Option<ZoneImages>,
    pub damage_word: Option<Img>,
    pub damage: Option<Digits>,
    pub hint: Option<Img>,
    /// Text form (fallback layout): a band rectangle and colour, and the text lines' style and first line top-left.
    pub text: Option<TextPanel>,
    pub title_swing: Option<TitleSwing>,
}

/// White, the colour of a text item the client creates without one.
// DC-DEFAULT: CLI-127 (proposed) the name's and the clan's default colour is white: the 2009 panel takes the record's
// own colour (+0x1d8 name, +0x1dc clan) when non-zero, else the text item's default colour (cshell+0xcbea0); w2-board
// reads both words (`panel_view`)
const NAME_COLOUR: [u8; 4] = [255, 255, 255, 255];

impl DeathLayout {
    /// The panel from the user's ROM: the CShell key texts and the loaded layout table.
    pub fn from_rom(rom: &Path, table: &LayoutTable) -> Option<DeathLayout> {
        let keys = PanelKeys::from_rom(rom).ok()?;
        Some(Self::from_table(table, &keys))
    }

    /// The panel from a layout table and key texts.
    pub fn from_table(table: &LayoutTable, keys: &PanelKeys) -> DeathLayout {
        let sec = table.section(keys.section());
        let raw = |k: PanelKey| -> Option<Value> {
            if k.in_hud_set() {
                table.hud_raw(0, keys.key(k)).map(parse_value)
            } else {
                sec.as_ref()?.raw(keys.key(k)).map(parse_value)
            }
        };
        let text = |k: PanelKey| -> Option<String> {
            match raw(k)? {
                Value::Str(s) => {
                    Some(s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty())
                }
                _ => None,
            }
        };
        let pair = |k: PanelKey| -> Option<[i32; 2]> {
            match raw(k)? {
                Value::Pair(p) => Some(p),
                _ => None,
            }
        };
        let int = |k: PanelKey| -> Option<i32> {
            match raw(k)? {
                Value::Int(i) => Some(i),
                _ => None,
            }
        };
        // EVIDENCE: cshell+0x343860 (image item: source (0, 0, w, h) drawn at its position with size (w, h)); the
        // panel creates each image with the "Rec" pair as its size (cshell+0xca9a0)
        let img = |t: PanelKey, p: PanelKey, s: PanelKey| -> Option<Img> {
            Some(Img {
                tex: text(t)?,
                pos: pair(p)?,
                size: pair(s)?,
            })
        };
        // EVIDENCE: cshell+0xca080 / cshell+0xca400 (`%s%d.dtx` of the digit prefix: hundreds, tens, ones)
        let digits = |h: PanelKey, t: PanelKey, o: PanelKey| -> Option<Digits> {
            Some(Digits {
                prefix: text(PanelKey::DigitPrefix)?,
                pos: [pair(h)?, pair(t)?, pair(o)?],
                size: pair(PanelKey::DigitSize)?,
            })
        };
        // EVIDENCE: cshell+0xca9a0 (the clan and the name text: the panel's font record and size, flags 0x11); flags
        // 0x11 hold none of the placement bits (4 / 2 horizontal, 0x40 / 0x20 vertical; cshell+0x2ee7c0,
        // cshell+0x2ee860), so the line's top-left is the key's point
        let text_style = TextStyle {
            font: int(PanelKey::TextFont).unwrap_or(-1),
            colour: NAME_COLOUR,
            size: int(PanelKey::TextFontSize).filter(|n| *n > 0),
        };
        let name = pair(PanelKey::NamePos).map(|at| (text_style, at));
        let clan = pair(PanelKey::ClanTextPos).map(|at| (text_style, at));
        let zone = pair(PanelKey::ZonePos)
            .zip(pair(PanelKey::ZoneSize))
            .map(|(pos, size)| ZoneImages {
                pos,
                size,
                tex: [
                    text(PanelKey::ZoneHeadTex),
                    text(PanelKey::ZoneBodyTex),
                    text(PanelKey::ZoneArmsTex),
                    text(PanelKey::ZoneLegsTex),
                    text(PanelKey::ZoneGroinTex),
                ],
            });
        let title_swing = int(PanelKey::TitleAlphaMax)
            .zip(int(PanelKey::TitleAlphaMin))
            .map(|(max, min)| TitleSwing {
                max,
                min,
                step: int(PanelKey::TitleAlphaStep).unwrap_or(0),
            });
        DeathLayout {
            band: img(PanelKey::BandTex, PanelKey::BandPos, PanelKey::BandSize),
            info_frame: img(
                PanelKey::InfoFrameTex,
                PanelKey::InfoFramePos,
                PanelKey::InfoFrameSize,
            ),
            rank: img(PanelKey::RankTex, PanelKey::RankPos, PanelKey::RankSize),
            name,
            clan,
            soldier: img(
                PanelKey::SoldierTex,
                PanelKey::SoldierPos,
                PanelKey::SoldierSize,
            ),
            hp_label: img(
                PanelKey::HpLabelTex,
                PanelKey::HpLabelPos,
                PanelKey::HpLabelSize,
            ),
            hp: digits(
                PanelKey::HpHundredsPos,
                PanelKey::HpTensPos,
                PanelKey::HpOnesPos,
            ),
            ap_label: img(
                PanelKey::ApLabelTex,
                PanelKey::ApLabelPos,
                PanelKey::ApLabelSize,
            ),
            ap: digits(
                PanelKey::ApHundredsPos,
                PanelKey::ApTensPos,
                PanelKey::ApOnesPos,
            ),
            damage_frame: img(
                PanelKey::DamageFrameTex,
                PanelKey::DamageFramePos,
                PanelKey::DamageFrameSize,
            ),
            zone,
            damage_word: img(
                PanelKey::DamageWordTex,
                PanelKey::DamageWordPos,
                PanelKey::DamageWordSize,
            ),
            damage: digits(
                PanelKey::DamageHundredsPos,
                PanelKey::DamageTensPos,
                PanelKey::DamageOnesPos,
            ),
            hint: img(PanelKey::HintTex, PanelKey::HintPos, PanelKey::HintSize),
            text: None,
            title_swing,
        }
    }

    /// Our own panel for runs without a ROM: a dark band at the bottom with three text lines.
    // DC-DEFAULT: CLI-44 (proposed, extended) the no-ROM death panel: band (0, 690, 1024, 78) at 70 % black, the lines
    // from (24, 700) one cell apart
    pub fn fallback() -> DeathLayout {
        DeathLayout {
            text: Some(TextPanel {
                band: [0, 690, 1024, 78],
                colour: [0, 0, 0, 180],
                style: TextStyle {
                    font: -1,
                    colour: [240, 240, 230, 255],
                    size: None,
                },
                at: [24, 700],
            }),
            ..DeathLayout::default()
        }
    }
}

// ---- drawing -----------------------------------------------------------------------------------------------------

const OPAQUE: [u8; 4] = [255, 255, 255, 255];

fn image(i: &Img) -> DrawItem {
    DrawItem::Image {
        tex: i.tex.clone(),
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

/// Hundreds, tens and ones of `v` (0..=999) as digit images.
// EVIDENCE: cshell+0xca080 (hundreds = v / 100, tens = v / 10 (mod 10 when above 9), ones = v - 10 x (tens + 10 x
// hundreds))
fn digits(out: &mut Vec<DrawItem>, d: &Digits, v: u32) {
    let v = v.min(999);
    for (i, n) in [v / 100, (v / 10) % 10, v % 10].into_iter().enumerate() {
        out.push(image(&Img {
            tex: format!("{}{n}.dtx", d.prefix),
            pos: d.pos[i],
            size: d.size,
        }));
    }
}

/// The values the panel shows this frame: name, HP, AP, damage, zone image index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelView {
    pub name: String,
    /// w2-board: the name's colour (the record's `h1` when not 0), the clan name and its colour (shown when the
    /// record's `group_no` is not 0; the record's `g3` when not 0), the level index of the level icon.
    pub name_colour: Option<[u8; 4]>,
    pub clan: Option<(String, Option<[u8; 4]>)>,
    pub level: u32,
    pub hp: u32,
    pub ap: u32,
    pub damage: u32,
    pub zone: usize,
}

/// The panel's values: the target's record (name, colours, clan, level); HP and AP of the target (0 for a dead
/// target); the last wound. The target is the player the follow camera follows ([`HudState::observed`]), else the
/// own record.
// EVIDENCE: cshell+0xcbea0 (name = record +0x0c; HP / AP from the target object's slots +0x74 / +0x78, digits redone
// when either changes; damage and zone each frame while the own state is 2)
// EVIDENCE: cshell+0x60010 -> cshell+0xc9e20 (the follow camera's new target is bound to the panel: object +0x14,
// its roster record +0x18), cshell+0x28970 / cshell+0x28980 (a remote character's slots +0x74 / +0x78 return the
// health / armour of its type-0x18 notes, cshell+0x26f60)
// DC-DEFAULT: CLI-128 (amended 2026-11-02, proposed) the own record while no other player is followed; a followed
// player without a health / armour note yet shows 0; a dead own target shows HP 0 and AP 0 (OBSERVED: sidebyside3 /
// sidebyside4, 000 / 000 in every panel frame although the last armour value before the death was not 0)
// EVIDENCE: cshell+0xcbea0 (w2-board: the clan text hidden when record +0x1cc is 0, else the clan name with colour
// +0x1dc when non-zero; the name with colour +0x1d8 when non-zero; the level icon from the icon picker of record
// +0x1a4, cshell+0x268300)
pub fn panel_view(s: &HudState) -> PanelView {
    let followed = s.observed.filter(|(p, _)| Some(*p) != s.own_pid);
    let pid = followed.map(|(p, _)| p).or(s.own_pid);
    let name = pid
        .and_then(|p| s.roster.get(&p))
        .map(|r| r.name.clone())
        .unwrap_or_default();
    let mark = pid.and_then(|p| s.row_marks.get(&p));
    let word = |c: u32| (c != 0).then(|| super::board::argb(c));
    let name_colour = mark.and_then(|m| word(m.name_colour));
    let clan = mark
        .and_then(|m| m.clan.as_ref())
        .filter(|c| c.group_no != 0)
        .map(|c| (c.name.clone(), word(c.colour)));
    let rank = pid.and_then(|p| s.ranks.get(&p)).copied().unwrap_or(0);
    let level = super::board::level_index(mark, rank);
    let dead = s.life_state != 1;
    let value = |v: Option<f32>| {
        if dead {
            0
        } else {
            v.unwrap_or(0.0).max(0.0) as u32
        }
    };
    let (hp, ap) = match followed {
        Some((_, obj)) => {
            let v = s.vitals.get(&obj).copied().unwrap_or_default();
            (
                u32::from(v[0].unwrap_or(0)),
                u32::from(v[1].unwrap_or(0)),
            )
        }
        None => (value(s.health), value(s.armour)),
    };
    let w = s.wound.unwrap_or_default();
    PanelView {
        name,
        name_colour,
        clan,
        level,
        hp,
        ap,
        damage: shown_damage(w.damage),
        zone: zone_image(w.kind),
    }
}

/// Appends the death panel to `out` when it is shown, in the 2009 draw order. `small` is the level table's small
/// icon list (the level icon's texture; empty without a ROM: the layout's own level image).
// EVIDENCE: cshell+0xcbb00 (band, frame, level icon, clan text, name, clan mark, soldier, HP label, HP digits, AP
// label, AP digits; then the damage frame, zone image, damage word, damage digits and the change-view hint; DISASM
// cshell+0xcbb34..0xcbc03: element slots +0x1c, +0x20, +0x24, +0x2c, +0x30, +0x74, +0x34..+0x54); the class mark
// (+0x28) is never drawn by the 2009 panel
// DC-DEFAULT: CLI-533 (proposed) the clan mark (two layers of the clan mark table) is not drawn
pub fn items(out: &mut Vec<DrawItem>, l: &DeathLayout, s: &HudState, small: &[String]) {
    if !panel_shown(s) {
        return;
    }
    let v = panel_view(s);
    if let Some(TextPanel {
        band,
        colour,
        style,
        at,
    }) = l.text
    {
        out.push(DrawItem::Fill {
            rect: band.map(|n| n as f32),
            colour,
        });
        let cell = 22;
        for (i, line) in [
            v.name.clone(),
            format!("HP {:03}  AP {:03}", v.hp, v.ap),
            format!("{:03} DAMAGE", v.damage),
        ]
        .into_iter()
        .enumerate()
        {
            out.push(DrawItem::Text {
                style,
                pos: [at[0] as f32, (at[1] + cell * i as i32) as f32],
                text: line,
                right: false,
            });
        }
        return;
    }
    for i in [&l.band, &l.info_frame].into_iter().flatten() {
        out.push(image(i));
    }
    // the level icon: the icon picker's texture for the level index (no icon past the list); without the level
    // table the layout's own image (DC-DEFAULT: CLI-531)
    if let Some(r) = &l.rank {
        let tex = if small.is_empty() {
            Some(r.tex.clone())
        } else {
            super::board::level_icon(small, v.level, &v.name).map(str::to_string)
        };
        if let Some(tex) = tex {
            out.push(image(&Img { tex, ..r.clone() }));
        }
    }
    if let (Some((style, at)), Some((clan, colour))) = (l.clan, &v.clan) {
        out.push(DrawItem::Text {
            style: TextStyle {
                colour: colour.unwrap_or(style.colour),
                ..style
            },
            pos: [at[0] as f32, at[1] as f32],
            text: clan.clone(),
            right: false,
        });
    }
    if let Some((style, at)) = l.name {
        out.push(DrawItem::Text {
            style: TextStyle {
                colour: v.name_colour.unwrap_or(style.colour),
                ..style
            },
            pos: [at[0] as f32, at[1] as f32],
            text: v.name.clone(),
            right: false,
        });
    }
    for i in [&l.soldier, &l.hp_label].into_iter().flatten() {
        out.push(image(i));
    }
    if let Some(d) = &l.hp {
        digits(out, d, v.hp);
    }
    if let Some(i) = &l.ap_label {
        out.push(image(i));
    }
    if let Some(d) = &l.ap {
        digits(out, d, v.ap);
    }
    if let Some(i) = &l.damage_frame {
        out.push(image(i));
    }
    if let Some(z) = &l.zone {
        if let Some(t) = z.tex[v.zone].as_ref().or(z.tex[1].as_ref()) {
            out.push(image(&Img {
                tex: t.clone(),
                pos: z.pos,
                size: z.size,
            }));
        }
    }
    if let Some(i) = &l.damage_word {
        out.push(image(i));
    }
    if let Some(d) = &l.damage {
        digits(out, d, v.damage);
    }
    if let Some(i) = &l.hint {
        out.push(image(i));
    }
}

/// The respawn title's alpha `since_ms` after the wait of `wait_ms` was armed.
///
/// The 2009 element ticks its gauge `1000 / rate` ms apart (rate = 100 / the whole seconds left at the first update,
/// 6 when that is below 1); before a tick the rate grows by one while `rate x seconds left < 100 - ticks`; every tick
/// moves the alpha by the step toward the current bound and turns at the bound (it starts at the upper bound,
/// going down).
// DISASM: cshell+0xcc520 (+0x40 seconds, +0x4c rate, +0x50 ticks, +0x58 alpha from +0x5c, bounds +0x5c / +0x60, step
// +0x64, direction +0x68; `GetTickCount() - last > 1000 / rate` starts a tick; the alpha is stored into the title
// image every update, cshell+0xcc7a1)
// DC-DEFAULT: CLI-129 (proposed) the rate catch-up is tested once per tick (the 2009 client tests it every frame, a
// frame-rate dependent rule)
pub fn title_alpha(swing: &TitleSwing, wait_ms: u32, since_ms: u32) -> u8 {
    gauge_ticks(Some(swing), wait_ms, since_ms).1
}

/// The respawn element's tick count `since_ms` after the wait of `wait_ms` was armed, and the title's alpha after
/// those ticks (255 without a swing). The ticks are the gauge's fill in percent (w2-board).
// DISASM: cshell+0xcc520 (one tick: +0x50 += 1, the alpha moved by the step; the left cap is shown and the middle bar
// set to `+0x50 - left share` once +0x50 passes the left cap's share of the gauge width, the right cap once
// `100 - +0x50` drops below its share: the fill is the tick count in percent of the gauge)
// DC-DEFAULT: CLI-129 (proposed) the catch-up once per tick, as for the alpha
pub fn gauge_ticks(swing: Option<&TitleSwing>, wait_ms: u32, since_ms: u32) -> (u32, u8) {
    let secs0 = (wait_ms.saturating_sub(1) / 1000) as i64;
    let secs0 = if secs0 < 1 { 6 } else { secs0 };
    let mut rate = (100 / secs0).max(1);
    let (max, min, step) = swing.map_or((255, 255, 0), |w| {
        (w.max as i64, w.min as i64, w.step as i64)
    });
    let (mut alpha, mut up, mut ticks, mut last) = (max, false, 0i64, 0i64);
    let now = since_ms as i64;
    for _ in 0..100_000 {
        let left = (wait_ms as i64 - (last + 1)).max(0) / 1000;
        if left > 0 && rate * left < 100 - ticks {
            rate += 1;
        }
        let next = last + 1000 / rate + 1;
        if next > now {
            break;
        }
        last = next;
        if up {
            alpha += step;
            if alpha > max {
                up = false;
                alpha = max;
            }
        } else {
            alpha -= step;
            if alpha < min {
                up = true;
                alpha = min;
            }
        }
        ticks += 1;
    }
    (ticks.max(0) as u32, alpha.clamp(0, 255) as u8)
}

/// When the 2009 timetable of [`gauge_ticks`] reaches its 100th tick for a wait of `wait_ms` (ms after the arming).
pub fn gauge_full_ms(wait_ms: u32) -> u32 {
    let (mut lo, mut hi) = (0u32, wait_ms.max(1).saturating_mul(4));
    if gauge_ticks(None, wait_ms, hi).0 < 100 {
        return hi;
    }
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if gauge_ticks(None, wait_ms, mid).0 >= 100 {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

/// The gauge's tick count `since_ms` after the arming of a wait of `wait_ms`: the 2009 timetable stretched so that its
/// 100th tick falls on the end of the wait, the moment the respawn request goes out.
// DISASM: cshell+0x60b94..0x60bc5 (the same arming starts the request wait, `local_player` +0x44c / +0x450 = 7000 ms,
// and the element's timer, cshell+0x5c860 with 7 s), cshell+0xcc520 (the element's rate = 100 / the truncated
// seconds left at its first update: 6 for a 7 s timer, so its 100th tick comes near 5.9 s, about 1.1 s before the
// request at 7.0 s)
// DC-DEFAULT: CLI-1351 (proposed; playtest report "respawn is slightly delayed once the bar hits 100 %") the 2009 gap
// between the full bar and the request is removed: the bar fills over the whole wait
pub fn gauge_fill_ticks(wait_ms: u32, since_ms: u32) -> u32 {
    let full = gauge_full_ms(wait_ms);
    let since = if full < wait_ms {
        (u64::from(since_ms) * u64::from(full) / u64::from(wait_ms)) as u32
    } else {
        since_ms
    };
    gauge_ticks(None, wait_ms, since).0.min(100)
}

/// The filled part of the respawn gauge this frame (0..=1): the element's tick count in percent, at most 100.
// DC-DEFAULT: CLI-532 (proposed) the 2009 three-part gauge (left cap, middle bar, right cap, cshell+0xcc520) is drawn
// as the layout's one fill image cut at the tick percentage (supersedes the elapsed-time fill of CLI-51)
pub fn respawn_gauge_fraction(s: &HudState) -> f32 {
    respawn_gauge_ticks(s) as f32 / 100.0
}

/// The respawn element's tick count this frame (0..=100, [`gauge_fill_ticks`]), 0 without a running wait.
pub fn respawn_gauge_ticks(s: &HudState) -> u32 {
    match (s.death_at, s.respawn_wait_ms) {
        (Some(at), Some(wait)) if wait > 0 => gauge_fill_ticks(wait, s.now_ms.saturating_sub(at)),
        _ => 0,
    }
}

/// What the respawn gauge shows after `ticks` ticks: the left cap, the filled share of the middle bar (0..=1) and
/// the right cap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GaugeParts {
    pub left: bool,
    pub middle: f32,
    pub right: bool,
}

/// The 2009 three-part respawn gauge (CLI-983). The caps and the middle bar share 100 ticks by their widths: the
/// left cap appears once the ticks pass its share (whole percent of the three widths), the middle bar then fills
/// with the ticks beyond that share over a range of `(50 - left share) x 2`, and the right cap appears (the middle
/// full) once fewer ticks than its share are left.
// DISASM: cshell+0xcc520 (first tick: the three widths summed at +0x48, the middle range +0x54 =
// `(50 - left x 100 / sum) x 2`, the middle value 0, both caps hidden; every tick: +0x50 += 1, the left cap shown
// and the middle value `+0x50 - left x 100 / sum` once `left x 100 / sum < +0x50`, the middle value `+0x54` and the
// right cap shown once `100 - +0x50 < right x 100 / sum`)
// DC-DEFAULT: CLI-983 (proposed) the middle bar's value is drawn as that share of its width (the gauge control's
// own draw, cshell+0x441ba0 kind 0xc, is not read)
pub fn gauge_parts(ticks: u32, widths: [i32; 3]) -> GaugeParts {
    let [l, m, r] = widths.map(|w| w.max(0) as i64);
    let sum = l + m + r;
    let ticks = ticks.min(100) as i64;
    if sum <= 0 {
        return GaugeParts {
            left: false,
            middle: ticks as f32 / 100.0,
            right: false,
        };
    }
    let left_share = l * 100 / sum;
    let right_share = r * 100 / sum;
    let range = ((50 - left_share) * 2).max(1);
    let mut value = 0;
    let left = left_share < ticks;
    if left {
        value = ticks - left_share;
    }
    let right = 100 - ticks < right_share;
    if right {
        value = range;
    }
    GaugeParts {
        left,
        middle: (value as f32 / range as f32).clamp(0.0, 1.0),
        right,
    }
}

/// The HUD render level at which an element of `layer` is drawn this frame (its layer at most the level).
// EVIDENCE: cshell+0x26b0d0..0x26b108 (the element's layer compared with the manager's level)
pub fn layer_shown(s: &HudState, layer: u8) -> bool {
    layer <= hud_level(s.life_state)
}

/// Layer of the chat log and chat entry, the world name marks, the respawn wait, the round clock, the multi-kill
/// notes and announcement and the revenge panel: hidden at render level 0 (life state 5), shown in every other state.
// EVIDENCE: the element constructors store 1 at +0x08 after the base constructor's 3: cshell+0x757b0 (chat log),
// cshell+0x72cc0 (chat entry), cshell+0x8b030 (world marks), cshell+0xcd0c0 (respawn wait), cshell+0xa5d60
// (multi-kill), cshell+0xbb000 (revenge), cshell+0xbe4e0 (round clock; its vtable slot +0x10 is the clock update
// cshell+0xbe570); the kill feed (cshell+0x9d3b0) and scoreboard (cshell+0xc4600) store 0 and stay
pub const NOTE_LAYER: u8 = 1;

/// The tint of the respawn title this frame (opaque without a swing or an armed wait).
pub fn respawn_title_tint(l: &DeathLayout, s: &HudState) -> [u8; 4] {
    match (&l.title_swing, s.death_at, s.respawn_wait_ms) {
        (Some(w), Some(at), Some(wait)) => [
            255,
            255,
            255,
            title_alpha(w, wait, s.now_ms.saturating_sub(at)),
        ],
        _ => OPAQUE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::state::{DeathNote, HudEvent, Relation};
    use dc_botclient::shell::RosterRecord;

    fn dead_state(killer: Option<&str>, since: u32) -> HudState {
        let mut s = HudState::default();
        for e in [
            HudEvent::Own {
                pid: Some(0),
                mode_class: Some(1),
            },
            HudEvent::Roster(RosterRecord {
                name: "me".into(),
                pid: 0,
                team: 1,
                complete: true,
                ..RosterRecord::default()
            }),
            HudEvent::Life(1),
            HudEvent::Status {
                kind: 2,
                value: 19.0,
            },
            HudEvent::Status {
                kind: 3,
                value: 64.0,
            },
            HudEvent::Wound {
                damage: 27,
                kind: 3,
            },
        ] {
            s.apply(&e, 1000, 0.0);
        }
        s.death = Some(DeathNote {
            at: 2000,
            killer: killer.map(str::to_string),
            killer_rel: Relation::Opponent,
            weapon: 145,
            head_shot: false,
        });
        s.apply(&HudEvent::Life(3), 2000, 0.0);
        s.apply(&HudEvent::Life(2), 2000, 0.0);
        s.now_ms = 2000 + since;
        s
    }

    #[test]
    fn the_render_level_follows_the_life_state() {
        assert_eq!([0, 1, 2, 3, 4, 5, 6].map(hud_level), [3, 3, 2, 2, 3, 0, 2]);
        let s = dead_state(Some("bot"), 0);
        assert!(!live_widgets(&s));
        let mut a = HudState::default();
        a.apply(&HudEvent::Life(1), 0, 0.0);
        assert!(live_widgets(&a));
    }

    #[test]
    fn the_panel_waits_for_the_killer_camera() {
        let s = dead_state(Some("bot"), DYING_TIME_MS - 1);
        assert!(killer_camera(&s) && !panel_shown(&s));
        let s = dead_state(Some("bot"), DYING_TIME_MS);
        assert!(!killer_camera(&s) && panel_shown(&s));
        // no known killer: the follow camera at once
        let s = dead_state(None, 0);
        assert!(panel_shown(&s));
        // state 3 alone (no death time) draws nothing; alive draws nothing
        let mut t = dead_state(None, 0);
        t.life_state = 3;
        assert!(!panel_shown(&t));
        t.apply(&HudEvent::Life(1), 9000, 0.0);
        assert!(!panel_shown(&t));
    }

    #[test]
    fn the_panel_shows_the_last_wound_and_zeroes_for_the_dead() {
        let s = dead_state(None, 10);
        let v = panel_view(&s);
        assert_eq!(v.name, "me");
        assert_eq!((v.hp, v.ap, v.damage, v.zone), (0, 0, 27, 2));
        // a wound while dead is ignored
        let mut s = s;
        s.apply(&HudEvent::Wound { damage: 5, kind: 1 }, 2100, 0.0);
        assert_eq!(panel_view(&s).damage, 27);
        assert_eq!([0, 1, 2, 1000].map(shown_damage), [1, 1, 2, 999]);
        assert_eq!([0, 1, 2, 3, 4, 5, 9].map(zone_image), [1, 0, 1, 2, 3, 4, 1]);
    }

    /// CLI-128 (amended): while the follow camera follows a team mate the panel shows his name and the health and
    /// armour of his type-0x18 notes (the user's "Search and Destroy: while dead and viewing a teammate, show that
    /// player's information"); back on the own body it shows the own record again.
    #[test]
    fn the_panel_shows_the_followed_team_mate() {
        use crate::hud::msgs::ObjectNote;
        let mut s = dead_state(None, 10);
        s.apply(
            &HudEvent::Roster(RosterRecord {
                name: "mate".into(),
                pid: 4,
                team: 1,
                complete: true,
                ..RosterRecord::default()
            }),
            2005,
            0.0,
        );
        for (armour, value) in [(false, 73), (true, 41)] {
            s.apply(
                &HudEvent::Note(ObjectNote::Vitals {
                    obj: 0x120,
                    armour,
                    value,
                }),
                2006,
                0.0,
            );
        }
        s.observed = Some((4, 0x120));
        assert!(panel_shown(&s));
        let v = panel_view(&s);
        assert_eq!((v.name.as_str(), v.hp, v.ap), ("mate", 73, 41));
        assert_eq!(v.damage, 27, "the damage frame stays the own last wound");
        // a followed player without notes shows zeroes
        s.observed = Some((4, 0x121));
        assert_eq!((panel_view(&s).hp, panel_view(&s).ap), (0, 0));
        // the own body again
        s.observed = None;
        let v = panel_view(&s);
        assert_eq!((v.name.as_str(), v.hp, v.ap), ("me", 0, 0));
    }

    fn fixture_layout() -> (LayoutTable, PanelKeys) {
        let keys = PanelKeys::placeholder();
        let mut t = format!("[{}]\n", keys.section());
        let pos = |k: PanelKey, x: i32, y: i32| format!("{} = ({x}, {y})\n", keys.key(k));
        let tex = |k: PanelKey, n: &str| format!("{} = \"{n}\"\n", keys.key(k));
        use PanelKey as P;
        for (tk, pk, sk, n, x) in [
            (P::BandTex, P::BandPos, P::BandSize, "band", 0),
            (
                P::InfoFrameTex,
                P::InfoFramePos,
                P::InfoFrameSize,
                "frame",
                10,
            ),
            (P::RankTex, P::RankPos, P::RankSize, "rank", 20),
            (P::SoldierTex, P::SoldierPos, P::SoldierSize, "soldier", 30),
            (P::HpLabelTex, P::HpLabelPos, P::HpLabelSize, "hp", 40),
            (P::ApLabelTex, P::ApLabelPos, P::ApLabelSize, "ap", 50),
            (
                P::DamageFrameTex,
                P::DamageFramePos,
                P::DamageFrameSize,
                "dframe",
                60,
            ),
            (
                P::DamageWordTex,
                P::DamageWordPos,
                P::DamageWordSize,
                "dword",
                70,
            ),
            (P::HintTex, P::HintPos, P::HintSize, "hint", 80),
        ] {
            t += &tex(tk, n);
            t += &pos(pk, x, 700);
            t += &pos(sk, 8, 8);
        }
        for (k, n) in [
            (P::ZoneHeadTex, "zh"),
            (P::ZoneBodyTex, "zb"),
            (P::ZoneArmsTex, "za"),
            (P::ZoneLegsTex, "zl"),
            (P::ZoneGroinTex, "zg"),
            (P::DigitPrefix, "dg"),
        ] {
            t += &tex(k, n);
        }
        for (k, x) in [
            (P::ZonePos, 90),
            (P::HpHundredsPos, 100),
            (P::HpTensPos, 110),
            (P::HpOnesPos, 120),
            (P::ApHundredsPos, 130),
            (P::ApTensPos, 140),
            (P::ApOnesPos, 150),
            (P::DamageHundredsPos, 160),
            (P::DamageTensPos, 170),
            (P::DamageOnesPos, 180),
            (P::NamePos, 190),
        ] {
            t += &pos(k, x, 710);
        }
        t += &pos(P::ZoneSize, 9, 9);
        t += &pos(P::DigitSize, 6, 6);
        t += &format!(
            "{} = 3\n{} = 12\n",
            keys.key(P::TextFont),
            keys.key(P::TextFontSize)
        );
        let names = dc_ui_data::UiNames::placeholder();
        t += &format!(
            "[{}]\n{} = 255\n{} = 100\n{} = 20\n",
            names.hud_set0,
            keys.key(P::TitleAlphaMax),
            keys.key(P::TitleAlphaMin),
            keys.key(P::TitleAlphaStep)
        );
        (LayoutTable::parse(&t, names), keys)
    }

    #[test]
    fn the_layout_reads_the_panel_section_and_draws_in_order() {
        let (table, keys) = fixture_layout();
        let l = DeathLayout::from_table(&table, &keys);
        assert_eq!(l.band.as_ref().map(|i| i.pos), Some([0, 700]));
        assert_eq!(
            l.name.map(|n| (n.0.font, n.0.size, n.1)),
            Some((3, Some(12), [190, 710]))
        );
        assert_eq!(
            l.title_swing,
            Some(TitleSwing {
                max: 255,
                min: 100,
                step: 20
            })
        );
        let mut out = Vec::new();
        items(&mut out, &l, &dead_state(Some("bot"), 100), &[]);
        assert!(out.is_empty(), "killer camera: no panel");
        items(
            &mut out,
            &l,
            &dead_state(Some("bot"), DYING_TIME_MS + 100),
            &[],
        );
        let texs: Vec<String> = out
            .iter()
            .filter_map(|i| match i {
                DrawItem::Image { tex, .. } => Some(tex.clone()),
                DrawItem::Text { text, .. } => Some(format!("text:{text}")),
                _ => None,
            })
            .collect();
        assert_eq!(
            texs,
            [
                "band", "frame", "rank", "text:me", "soldier", "hp", "dg0.dtx", "dg0.dtx",
                "dg0.dtx", "ap", "dg0.dtx", "dg0.dtx", "dg0.dtx", "dframe", "za", "dword",
                "dg0.dtx", "dg2.dtx", "dg7.dtx", "hint"
            ]
        );
    }

    #[test]
    fn the_fallback_panel_is_text() {
        let mut out = Vec::new();
        items(
            &mut out,
            &DeathLayout::fallback(),
            &dead_state(None, 0),
            &[],
        );
        assert!(matches!(out[0], DrawItem::Fill { .. }));
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn the_title_alpha_swings_between_its_bounds() {
        let w = TitleSwing {
            max: 255,
            min: 100,
            step: 20,
        };
        assert_eq!(title_alpha(&w, 7000, 0), 255);
        let seen: Vec<u8> = (0..7000)
            .step_by(20)
            .map(|t| title_alpha(&w, 7000, t))
            .collect();
        assert!(seen.iter().all(|a| (100..=255).contains(a)));
        assert!(seen.contains(&100) && seen.contains(&255));
        // the gauge ticks about 100 times over the wait
        let flips = seen.windows(2).filter(|p| p[0] != p[1]).count();
        assert!(flips > 50, "{flips}");
    }

    #[test]
    fn the_panel_draws_the_clan_the_name_colour_and_the_level_icon() {
        // w2-board: band, frame, level icon, clan text, name (cshell+0xcbb00); colours from h1 / g3
        let (mut table_src, keys) = fixture_layout();
        let _ = &mut table_src;
        let mut l = DeathLayout::from_table(&table_src, &keys);
        l.clan = l.name.map(|(st, _)| (st, [190, 690]));
        let mut s = dead_state(None, 100);
        s.row_marks.insert(
            0,
            crate::hud::board::RowMark {
                look_level: 2,
                name_colour: 0xff00ff00,
                clan: Some(crate::hud::board::ClanTag {
                    name: "Owls".into(),
                    group_no: 7,
                    colour: 0x80ff0000,
                    mark: [0; 2],
                }),
                mission: [0; 3],
            },
        );
        let small: Vec<String> = ["l0", "l1", "l2"].map(String::from).to_vec();
        let mut out = Vec::new();
        items(&mut out, &l, &s, &small);
        let seq: Vec<String> = out
            .iter()
            .take(5)
            .filter_map(|i| match i {
                DrawItem::Image { tex, .. } => Some(tex.clone()),
                DrawItem::Text {
                    text, style, pos, ..
                } => Some(format!("{text}@{},{}#{:?}", pos[0], pos[1], style.colour)),
                _ => None,
            })
            .collect();
        assert_eq!(
            seq,
            [
                "band",
                "frame",
                "l2",
                "Owls@190,690#[255, 0, 0, 128]",
                "me@190,710#[0, 255, 0, 255]"
            ]
        );
        // group_no 0: no clan text; colour words 0: the default white
        let m = s.row_marks.get_mut(&0).unwrap();
        m.name_colour = 0;
        m.clan.as_mut().unwrap().group_no = 0;
        let mut out = Vec::new();
        items(&mut out, &l, &s, &small);
        let t: Vec<String> = out
            .iter()
            .filter_map(|i| match i {
                DrawItem::Text { text, style, .. } => Some(format!("{text}#{:?}", style.colour)),
                _ => None,
            })
            .collect();
        assert_eq!(t, ["me#[255, 255, 255, 255]"]);
    }

    #[test]
    fn gauge_ticks_count_about_a_hundred_over_the_wait() {
        assert_eq!(gauge_ticks(None, 7000, 0), (0, 255));
        let (n, _) = gauge_ticks(None, 7000, 7000);
        assert!(
            (100..=130).contains(&n),
            "the gauge is full before the wait ends: {n}"
        );
        let w = TitleSwing {
            max: 255,
            min: 100,
            step: 20,
        };
        for t in [0, 500, 3333, 6999] {
            assert_eq!(gauge_ticks(Some(&w), 7000, t).1, title_alpha(&w, 7000, t));
        }
    }

    #[test]
    fn the_gauge_is_full_when_the_respawn_request_goes_out() {
        // CLI-1351: the 2009 timetable is full near 5.9 s of a 7 s wait; the drawn fill reaches 100 at the wait's end
        let full = gauge_full_ms(7000);
        assert!((5800..=6000).contains(&full), "{full}");
        assert_eq!(gauge_ticks(None, 7000, full).0, 100);
        assert!(gauge_fill_ticks(7000, 6900) < 100, "not full before the request");
        assert_eq!(gauge_fill_ticks(7000, 7000), 100);
        assert_eq!(gauge_fill_ticks(7000, 9000), 100);
        assert_eq!(gauge_fill_ticks(7000, 0), 0);
        // half way through the wait about half the gauge
        let half = gauge_fill_ticks(7000, 3500);
        assert!((40..=60).contains(&half), "{half}");
        // the ticks never go back
        let mut last = 0;
        for t in (0..=7000).step_by(7) {
            let n = gauge_fill_ticks(7000, t);
            assert!(n >= last, "{t}: {n} < {last}");
            last = n;
        }
        // the 1.5 s wait of class 10 too
        assert_eq!(gauge_fill_ticks(1500, 1500), 100);
        assert!(gauge_fill_ticks(1500, 1400) < 100);
    }
}
