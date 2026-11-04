//! The 2009 scoreboard's row columns and the level table's rank icons (client-scoreboard).
//!
//! The in-game board (Tab, the death board and the result board) builds one row widget per player: the place
//! number right-aligned at the row's place point, the level icon at its own place with its own size, the name
//! left-aligned, kills and deaths centred on their points (the text control's flag 4 centres the line on the x
//! point), and the own row under a translucent white bar. Rows are placed per team list: team 1 from the rows'
//! origin, team 2 one team offset to the right, one row step per place, at most 12 rows per list. The header labels
//! of the place and name columns come from the same layout set as the kill and death labels.
//!
//! The level table (attribute archive) gives two lists used here: the small icon of each level and the
//! experience bound of each level; a level is the first row whose bound is above the experience.
//!
//! Each row also carries the clan name (the record's clan colour, else the layout's), the name in the record's own
//! colour word when it has one, and the mission value centred on its point; the header gains the clan and mission
//! labels (w2-board). The 2009 row also builds a clan mark, a flag and an internet-cafe mark; DeCombat does not draw
//! them yet (CLI-533), and the class mark is built but hidden by the 2009 row (cshell+0xbe7c0).
//!
//! No key or texture text is in this file: the layout keys and the level-table keys are read from the user's
//! `CShell.dll` by rva and accepted only by length and SHA-256 prefix.
// EVIDENCE: cshell+0xc0ac0 (row widget: level icon at the level place with the level size, cshell+0x53410 /
// 0x53480; place text at cshell+0x533a0 with flag 1, created empty; name at cshell+0x535d0 flag 1; kills / deaths /
// mission at cshell+0x53790 / 0x53800 / 0x53870 flag 4 = centred), cshell+0xbf100 (place text `%d` set with flag 2 =
// right-aligned), cshell+0x2ee7c0 (text x: flag 4 subtracts half the line width, flag 2 the whole width),
// cshell+0x2ee860 (flag 0x40 / 0x20: vertical centre / bottom, else top)
// DISASM: cshell+0xc3226..0xc3263 (team 1: place = list count + 1 while < 12, row at origin + step x count),
// cshell+0xc327a..0xc32b8 (team 2: x + team offset), cshell+0xc34b7..0xc3533 (own row: quad (x - 2, y + 1, 286, 28)
// colour 0x4cffffff; 286 = cshell+0xc2490 `+0xa4` = 0x11e)
// EVIDENCE: cshell+0xbf540 (header labels: font from the header font keys cshell+0x52ae0 / 0x52b50, the place and
// name labels by string index and place cshell+0x52bc0 / 0x52c30 and cshell+0x52ca0 / 0x52d10, flag 0)

use super::layout::Img;
use dc_ui_data::layout::parse_value;
use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::{LayoutTable, UiStrings, Value};
use std::path::Path;

/// Rows per team list.
// DISASM: cshell+0xc322e (`cmp eax, 0xc`)
pub const MAX_ROWS: usize = 12;
/// Mode classes whose board is one ranking list over both columns (free for all, bounty, infection).
// EVIDENCE: cshell+0xc3180 (board +0x12c = mode class; 6, 9 and 0xb set the single-list flag)
pub const SINGLE_LIST_CLASSES: [u8; 3] = [6, 9, 11];
/// Rows of the first column in the single-list board; the rest go on in the second column from place 9.
// EVIDENCE: cshell+0xc3180 (single list: place = count + 1 while the first column holds fewer than 8, else the
// second column's count + 9, row at the second column's count)
pub const SINGLE_LIST_FIRST_COLUMN: i32 = 8;

/// Where the next board row goes: `(column 0 / 1, row index in the column, place number)`, `None` when its team list
/// is full. `counts` holds the rows placed so far per column. Team boards: team 2 in the second column, any other
/// team in the first, 12 rows each. Single-list boards ([`SINGLE_LIST_CLASSES`]): 8 rows in the first column with
/// places 1..8, the rest in the second column with places 9...
// EVIDENCE: cshell+0xc3180 (both forms)
pub fn board_slot(single: bool, team: u8, counts: &mut [i32; 2]) -> Option<(usize, i32, i32)> {
    if single {
        let col = usize::from(counts[0] >= SINGLE_LIST_FIRST_COLUMN);
        let row = counts[col];
        counts[col] += 1;
        let place = if col == 0 {
            row + 1
        } else {
            row + SINGLE_LIST_FIRST_COLUMN + 1
        };
        return Some((col, row, place));
    }
    let col = usize::from(team == 2);
    if counts[col] as usize >= MAX_ROWS {
        return None;
    }
    let row = counts[col];
    counts[col] += 1;
    Some((col, row, row + 1))
}

/// The own row's bar: left of the row x by 2, below the row y by 1, 286 x 28, white at alpha 0x4c.
// DISASM: cshell+0xc34d4..0xc352d
pub const OWN_BAR_INSET: [i32; 2] = [-2, 1];
pub const OWN_BAR_SIZE: [i32; 2] = [286, 28];
pub const OWN_BAR_COLOUR: [u8; 4] = [255, 255, 255, 0x4c];

/// The ping column's header label (our text: the 2009 board has none).
pub const PING_LABEL: &str = "Ping";

/// The row point of the ping column (relative to the row like the kill and death points): one count step (death x
/// minus kill x) left of the kill point, at the kills' height; with a clan point at least [`CLAN_ROOM`] right of it
/// (room for the clan name), never closer to the kills than one three-digit value ([`PING_HALF_WIDTH`]). The ROM
/// board has no room right of its mission column (the row band ends about 28 px right of the mission point and the
/// mission label fills it), but 120 px between the clan and kill points.
///
/// The 2009 client keeps each player's ping from the server's ping table (0xa2) in the roster record but its board
/// row builds no control for it: the row widget's controls are the level, flag, internet-cafe, class and infection
/// marks, place, name, clan name, kills, deaths, mission and a `%d/%d` pair, and the layout's `Player_*` key set has
/// no ping key; the six header labels are place, name, clan, kill, death and mission. The column is DeCombat's.
// EVIDENCE: cshell+0x2d73a..0x2d798 (0xa2 writes record +0x00), cshell+0x29830 (the record constructor zeroes it),
// cshell+0xc0ac0 (row widget controls), cshell+0xbf540 (six header labels)
// DC-DEFAULT: CLI-1360 (proposed) ping column left of the kills
pub fn ping_column(kills: [i32; 2], deaths: [i32; 2], clan: Option<[i32; 2]>) -> [i32; 2] {
    let step = (deaths[0] - kills[0]).abs().max(2 * PING_HALF_WIDTH);
    let mut x = kills[0] - step;
    if let Some(c) = clan {
        x = x.max(c[0] + CLAN_ROOM);
    }
    [x.min(kills[0] - 2 * PING_HALF_WIDTH), kills[1]]
}

/// Half the width a three-digit ping takes at the board's text size (ours).
pub const PING_HALF_WIDTH: i32 = 16;
/// Room kept for the clan name right of the clan point (ours; about 12 characters of the board font).
pub const CLAN_ROOM: i32 = 80;

/// One pinned key of the board's HUD layout set: the literal (rva, SHA-256 prefix, length) and the rva of the code
/// that reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardKey {
    PlacePos,
    LevelPos,
    LevelSize,
    PlaceLabel,
    PlaceLabelPos,
    NameLabel,
    NameLabelPos,
    HeaderFont,
    HeaderFontSize,
    /// w2-board: the clan and mission header labels (string index, place).
    ClanLabel,
    ClanLabelPos,
    MissionLabel,
    MissionLabelPos,
    /// w2-board: the row's clan name point, its default colour and the mission point.
    GuildAt,
    ClanNameColour,
    TaskAt,
}

const BOARD_PINS: [(BoardKey, u32, &str, usize, u32); 16] = [
    (
        BoardKey::PlacePos,
        0x3d1fa8,
        "20b1cc5ef110add2",
        14,
        0x533a0,
    ),
    (
        BoardKey::LevelPos,
        0x3d1fb8,
        "af996a88dc3fd6ca",
        15,
        0x53410,
    ),
    (
        BoardKey::LevelSize,
        0x3d1fc8,
        "808b0e29445338eb",
        16,
        0x53480,
    ),
    (
        BoardKey::PlaceLabel,
        0x3d1e90,
        "79411b956f8b94f3",
        16,
        0x52bc0,
    ),
    (
        BoardKey::PlaceLabelPos,
        0x3d1ea4,
        "c94cadaf391daa8d",
        7,
        0x52c30,
    ),
    (
        BoardKey::NameLabel,
        0x3d1eac,
        "c44eb99c899a1fad",
        16,
        0x52ca0,
    ),
    (
        BoardKey::NameLabelPos,
        0x3d1ec0,
        "89fe1399c2c9c93f",
        7,
        0x52d10,
    ),
    (
        BoardKey::HeaderFont,
        0x3d1e78,
        "2ca5bf1133473a89",
        9,
        0x52ae0,
    ),
    (
        BoardKey::HeaderFontSize,
        0x3d1e84,
        "8fab546981e2c7a0",
        8,
        0x52b50,
    ),
    // EVIDENCE: cshell+0xbf540 (the third and sixth header labels: cshell+0x52d80 / 0x52df0 and cshell+0x53020 /
    // 0x53090); DISASM: cshell+0x52da3, 0x52e13, 0x53043, 0x530b3 (the key literals the accessors push)
    (
        BoardKey::ClanLabel,
        0x3d1ec8,
        "b0d019a029f06f79",
        16,
        0x52d80,
    ),
    (
        BoardKey::ClanLabelPos,
        0x3d1edc,
        "efbc979d0340f2f0",
        7,
        0x52df0,
    ),
    (
        BoardKey::MissionLabel,
        0x3d1f20,
        "08f39c4d7dfb6090",
        19,
        0x53020,
    ),
    (
        BoardKey::MissionLabelPos,
        0x3d1f34,
        "1bfeb422a2c608ba",
        10,
        0x53090,
    ),
    // EVIDENCE: cshell+0xc0ac0 (the clan text at cshell+0x53640, its colour cshell+0x53a30, the mission text at
    // cshell+0x53870); DISASM: cshell+0x53663, 0x53a88, 0x53893 (the key literals)
    (BoardKey::GuildAt, 0x3d2018, "4f9e9836ceb76638", 18, 0x53640),
    (
        BoardKey::ClanNameColour,
        0x3d20b0,
        "6063dd778b518cfb",
        20,
        0x53a30,
    ),
    (BoardKey::TaskAt, 0x3d2074, "802af19fe8dcbd15", 17, 0x53870),
];

impl BoardKey {
    pub fn pin(self) -> Pin {
        let r = BOARD_PINS[self as usize];
        Pin {
            what: "board layout key",
            rva: r.1,
            sha16: r.2,
            len: r.3,
        }
    }

    /// Rva of the accessor that reads the key.
    pub fn evidence(self) -> u32 {
        BOARD_PINS[self as usize].4
    }
}

/// The board's extra columns (the HUD layout set): place number, level icon, the place / name header labels.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardColumns {
    /// Place number point (right end of the text), relative to the row.
    pub place: Option<[i32; 2]>,
    /// Level icon place and size, relative to the row.
    pub level: Option<([i32; 2], [i32; 2])>,
    /// Header labels (text, place relative to the window).
    pub place_label: Option<(String, [i32; 2])>,
    pub name_label: Option<(String, [i32; 2])>,
    /// Header font record and cell size (`None`: the row font).
    pub header_font: Option<i32>,
    pub header_font_size: Option<i32>,
    /// w2-board: the clan and mission header labels (text, place relative to the window).
    pub clan_label: Option<(String, [i32; 2])>,
    pub mission_label: Option<(String, [i32; 2])>,
    /// w2-board: the clan name's top-left point, relative to the row.
    pub clan_at: Option<[i32; 2]>,
    /// w2-board: the clan name's colour when the record names none (opaque, from the colour key).
    pub clan_colour: Option<[u8; 4]>,
    /// w2-board: the mission value's point (the line centred on it), relative to the row.
    pub mission: Option<[i32; 2]>,
}

impl BoardColumns {
    /// The columns from the user's CShell key texts and the loaded layout table (HUD set `set`).
    pub fn from_rom(
        rom: &Path,
        table: &LayoutTable,
        set: u32,
        strings: Option<&UiStrings>,
    ) -> Option<BoardColumns> {
        let pe = dc_rom::PeFile::from_file(&rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]))
            .ok()?;
        let keys: Vec<String> = BOARD_PINS
            .iter()
            .map(|r| read_pinned(&pe, &r.0.pin()))
            .collect::<Result<_, _>>()
            .ok()?;
        Some(Self::from_table(
            |k| table.hud_raw(set, &keys[k as usize]).map(str::to_string),
            strings,
        ))
    }

    /// The columns from a key lookup (`raw(key)` = the key's value text in the HUD set).
    pub fn from_table(
        raw: impl Fn(BoardKey) -> Option<String>,
        strings: Option<&UiStrings>,
    ) -> BoardColumns {
        let value = |k: BoardKey| raw(k).map(|s| parse_value(&s));
        let pair = |k: BoardKey| match value(k)? {
            Value::Pair(p) => Some(p),
            _ => None,
        };
        let int = |k: BoardKey| match value(k)? {
            Value::Int(i) => Some(i),
            _ => None,
        };
        // EVIDENCE: cshell+0x53a30 (colour vector -> `0xff000000 | r << 16 | g << 8 | b`, components truncated)
        let colour = |k: BoardKey| match value(k)? {
            Value::Vec3(c) => {
                let b = |v: f64| v.clamp(0.0, 255.0) as u8;
                Some([b(c[0]), b(c[1]), b(c[2]), 255])
            }
            _ => None,
        };
        let label = |id: BoardKey, pos: BoardKey| -> Option<(String, [i32; 2])> {
            let s = strings?.get(u32::try_from(int(id)?).ok()?)?;
            Some((s.to_string(), pair(pos)?))
        };
        BoardColumns {
            place: pair(BoardKey::PlacePos),
            level: pair(BoardKey::LevelPos).zip(pair(BoardKey::LevelSize)),
            place_label: label(BoardKey::PlaceLabel, BoardKey::PlaceLabelPos),
            name_label: label(BoardKey::NameLabel, BoardKey::NameLabelPos),
            header_font: int(BoardKey::HeaderFont),
            header_font_size: int(BoardKey::HeaderFontSize).filter(|n| *n > 0),
            clan_label: label(BoardKey::ClanLabel, BoardKey::ClanLabelPos),
            mission_label: label(BoardKey::MissionLabel, BoardKey::MissionLabelPos),
            clan_at: pair(BoardKey::GuildAt),
            clan_colour: colour(BoardKey::ClanNameColour),
            mission: pair(BoardKey::TaskAt),
        }
    }
}

/// The icon of list position `index` for a player called `name`: a name beginning with the operator mark takes the
/// last icon; an index past the list has no icon.
// EVIDENCE: cshell+0x268300 (index < list length, else none; the operator-name compare against the two-letter
// mark picks the last entry)
pub fn level_icon<'a>(small: &'a [String], index: u32, name: &str) -> Option<&'a str> {
    if (index as usize) >= small.len() {
        return None;
    }
    let i = if name.starts_with(OPERATOR_MARK) {
        small.len() - 1
    } else {
        index as usize
    };
    small.get(i).map(String::as_str).filter(|s| !s.is_empty())
}

/// The two-letter mark of operator names (a wide-string literal of the icon picker).
// EVIDENCE: cshell+0x268300 (`wcsncmp(name, mark, wcslen(mark))`)
const OPERATOR_MARK: &str = "GM";

/// The level table's lists used by the result screen: experience bound and small icon per level.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LevelTable {
    /// Experience bound of each level row.
    pub bounds: Vec<u32>,
    /// Small icon of each list position.
    pub small: Vec<String>,
}

/// The level section and its experience column key (numbered keys from 0).
// EVIDENCE: cshell+0x268810 (the level section's columns are read by name, the experience column third),
// cshell+0x267f10 (the bound column at table +0x14)
const LEVEL_PINS: [Pin; 2] = [
    Pin {
        what: "level section",
        rva: 0x3f4a68,
        sha16: "9782640c0625e02f",
        len: 8,
    },
    Pin {
        what: "experience column key",
        rva: 0x3f4a44,
        sha16: "8eab0f09df013b1b",
        len: 10,
    },
];

impl LevelTable {
    /// Reads both lists from the user's level table with the keys of the user's CShell.dll.
    pub fn from_rom(rom: &Path) -> Option<LevelTable> {
        let pe = dc_rom::PeFile::from_file(&rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]))
            .ok()?;
        let s: Vec<String> = LEVEL_PINS
            .iter()
            .map(|p| read_pinned(&pe, p))
            .collect::<Result<_, _>>()
            .ok()?;
        let vfs = dc_rez::RezVfs::new(rom);
        let doc = dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", super::rank::LEVEL_ENTRY)?;
        let sec = doc.get(&s[0])?;
        let bounds = (0..1024)
            .map_while(|i| sec.get(&format!("{}{i}", s[1])))
            .map(|v| atoi(v))
            .collect();
        let small = super::rank::RankIcons::from_rom(rom)
            .map(|r| r.small)
            .unwrap_or_default();
        Some(LevelTable { bounds, small })
    }

    /// The level of `exp`: the first row whose bound is above it, 0 when none is.
    // EVIDENCE: cshell+0x267f10 (`if (exp < atoi(bound[i])) return i`; 0 after the last row)
    pub fn level_of(&self, exp: u32) -> u32 {
        self.bounds
            .iter()
            .position(|b| exp < *b)
            .map_or(0, |i| i as u32)
    }

    /// The small icon of the level of `exp` (by the icon picker's rule).
    pub fn icon_of_exp(&self, exp: u32, name: &str) -> Option<&str> {
        level_icon(&self.small, self.level_of(exp), name)
    }
}

/// `atoi`: leading sign and digits, 0 when there are none (negative values clamp to 0).
fn atoi(v: &str) -> u32 {
    let t = v.trim().trim_matches('"').trim();
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

/// The level icon image of a board row at `row` (`None` without an icon).
pub fn row_icon(
    cols: &BoardColumns,
    small: &[String],
    rank: u32,
    name: &str,
    row: [i32; 2],
) -> Option<Img> {
    let (at, size) = cols.level?;
    let tex = level_icon(small, rank, name)?;
    Some(Img {
        tex: tex.to_string(),
        pos: [row[0] + at[0], row[1] + at[1]],
        size,
    })
}

// ---- w2-board: the clan, name-colour, level and mission fields of a row ----------------------------------------

/// The clan of a roster record as the board and the death panel use it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClanTag {
    pub name: String,
    /// `group_no`: 0 hides the clan name on the death panel (cshell+0xcbea0 tests record +0x1cc).
    pub group_no: u64,
    /// The clan name's colour word (`0xAARRGGBB`), 0 = the element's default colour.
    pub colour: u32,
}

/// What the board row and the death panel take from a 0x6b record beyond the counts: the level index, the name
/// colour, the clan and the three mission values.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 (fields 6, 9-11, 15-17)
// DISASM: cshell+0x2cc94..0x2cd10 (the 0x6b handler's board call cshell+0xc5590: look slot +0x10 = the fifth look
// byte as the level, v64 / v68 / v6c, has_group, the clan name, group_no, g1, g2, h1 as the name colour, g3 as the
// clan colour, h2, e3); DISASM: cshell+0xc564c..0xc56db (cshell+0xc5590 hands them on to the row builder
// cshell+0xc0ac0 as its arguments 11 (level), 8-10 (mission values), 13-17 (clan), 18 (name colour), 19 (clan
// colour), 20 (flag), 22 (internet-cafe mark))
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RowMark {
    /// The fifth look byte (record +0x1a4, clamped to >= 0 by the look parser cshell+0x1c2a90).
    pub look_level: u32,
    /// `h1`: the name colour word, 0 = the row's own colour.
    pub name_colour: u32,
    pub clan: Option<ClanTag>,
    /// `v64`, `v68`, `v6c`.
    pub mission: [u8; 3],
}

impl RowMark {
    pub fn of_record(r: &dc_botclient::shell::RosterRecord) -> RowMark {
        RowMark {
            look_level: r.look[4].max(0) as u32,
            name_colour: r.name_colour,
            clan: r.clan.as_ref().map(|c| ClanTag {
                name: c.name.clone(),
                group_no: c.group_no,
                colour: c.colour,
            }),
            mission: r.mission,
        }
    }
}

/// The level icon's list position of a board row or of the death panel: the fifth look byte; DeCombat's own
/// client sends 0 there, so a 0 falls back to the 0x6b rank `h2` the DeCombat server fills from the experience.
// EVIDENCE: cshell+0xc0ac0 (the row's level icon = the icon picker of argument 11, cshell+0x268300), cshell+0xcbea0
// (the death panel's level icon = the icon picker of record +0x1a4); DISASM: cshell+0x2c8d1..0x2c8ed (argument 11 is
// the parsed look's slot +0x10)
// DC-DEFAULT: CLI-531 (proposed) look byte 4 = 0 -> the `h2` rank (supersedes the rank choice of CLI-381)
pub fn level_index(mark: Option<&RowMark>, rank: u32) -> u32 {
    match mark.map(|m| m.look_level) {
        Some(l) if l > 0 => l,
        _ => rank,
    }
}

/// A colour word `0xAARRGGBB` as straight RGBA.
pub fn argb(c: u32) -> [u8; 4] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]
}

/// The mode class whose board rows keep the row colour for the name.
// EVIDENCE: cshell+0xc0ac0 (`mode_class == 0xb` -> the row colour even with a name colour)
pub const NAME_COLOUR_OFF_CLASS: u8 = 11;

/// The colour of a row's name: a dead row takes the dead colour, else the record's name colour when it is not 0
/// (and the mode class is not [`NAME_COLOUR_OFF_CLASS`]), else the row colour (own / alive).
// EVIDENCE: cshell+0xc0ac0 (argument 18 non-zero -> the name's colour), cshell+0xbf220 (a dead row: every text in the
// dead colour; a living row: the name in its own colour, the counts in the row colour, the clan in the clan colour)
pub fn name_colour(
    mark: Option<&RowMark>,
    row: [u8; 4],
    dead: Option<[u8; 4]>,
    mode_class: Option<u8>,
) -> [u8; 4] {
    if let Some(d) = dead {
        return d;
    }
    match mark.map(|m| m.name_colour) {
        Some(c) if c != 0 && mode_class != Some(NAME_COLOUR_OFF_CLASS) => argb(c),
        _ => row,
    }
}

/// The clan text of a board row and its colour: none without a clan block; the dead colour on a dead row, else the
/// record's clan colour when it is not 0, else the layout's clan colour (`fallback` without the key).
// EVIDENCE: cshell+0xc0ac0 (the clan text control: the clan name, flag 1, colour argument 19 when non-zero, else the
// colour key of cshell+0x53a30), cshell+0xbf220 (dead row: the dead colour), cshell+0xbe7c0 (the clan text is shown)
pub fn clan_text(
    cols: &BoardColumns,
    mark: Option<&RowMark>,
    dead: Option<[u8; 4]>,
    fallback: [u8; 4],
) -> Option<(String, [u8; 4])> {
    let c = mark?.clan.as_ref()?;
    let colour = match (dead, c.colour) {
        (Some(d), _) => d,
        (None, 0) => cols.clan_colour.unwrap_or(fallback),
        (None, w) => argb(w),
    };
    Some((c.name.clone(), colour))
}

/// The mission column's text: `v68/v6c` in mode classes 7 and 8, else `v64`.
// EVIDENCE: cshell+0xc0ac0 (the mission text `%d` of argument 8 and a second text `%d/%d` of arguments 9 and 10, both
// at the mission point with flag 4), cshell+0xbe7c0 (the first is shown, the second hidden, except in mode classes 7
// and 8: the first hidden, the second shown)
pub fn mission_text(mark: Option<&RowMark>, mode_class: Option<u8>) -> String {
    mission_values_text(mark.map_or([0; 3], |m| m.mission), mode_class)
}

/// The mission column's text of the three values `v64`, `v68`, `v6c`.
/// CLI-1161 the result page's row takes the same rule: the 2009 row builder prints `%d/%d` of two values in the
/// mode classes 7 and 8 and `%d` of the first elsewhere.
// EVIDENCE: cshell+0x130220 (the row builder's mission text: mode class 7 or 8 -> the format of two numbers with
// arguments 18 and 19, else one number with argument 17; the cell sits at the mission column of the row)
pub fn mission_values_text(m: [u8; 3], mode_class: Option<u8>) -> String {
    if matches!(mode_class, Some(7 | 8)) {
        format!("{}/{}", m[1], m[2])
    } else {
        m[0].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FFA (class 6): one ranking list, 8 rows on the left with places 1..8, then the right column from place 9,
    /// whatever the team values; a team board splits by team.
    #[test]
    fn ffa_board_is_one_list_over_both_columns() {
        let mut n = [0; 2];
        let single: Vec<_> = (0..10u8)
            .map(|t| board_slot(true, t % 3 + 1, &mut n))
            .collect();
        assert_eq!(single[0], Some((0, 0, 1)));
        assert_eq!(single[7], Some((0, 7, 8)));
        assert_eq!(single[8], Some((1, 0, 9)));
        assert_eq!(single[9], Some((1, 1, 10)));
        let mut n = [0; 2];
        assert_eq!(board_slot(false, 2, &mut n), Some((1, 0, 1)));
        assert_eq!(board_slot(false, 5, &mut n), Some((0, 0, 1)));
        assert_eq!(board_slot(false, 1, &mut n), Some((0, 1, 2)));
    }

    #[test]
    fn level_icon_follows_the_picker_rule() {
        let small: Vec<String> = ["a.dtx", "b.dtx", "c.dtx"].map(String::from).to_vec();
        assert_eq!(level_icon(&small, 0, "Player"), Some("a.dtx"));
        assert_eq!(level_icon(&small, 2, "Player"), Some("c.dtx"));
        assert_eq!(level_icon(&small, 3, "Player"), None, "past the list: none");
        assert_eq!(
            level_icon(&small, 0, "GMx"),
            Some("c.dtx"),
            "operator: last"
        );
        assert_eq!(level_icon(&[], 0, "Player"), None);
    }

    #[test]
    fn level_is_the_first_row_above_the_experience() {
        let t = LevelTable {
            bounds: vec![100, 500, 2000],
            small: ["l0", "l1", "l2"].map(String::from).to_vec(),
        };
        assert_eq!(t.level_of(0), 0);
        assert_eq!(t.level_of(99), 0);
        assert_eq!(t.level_of(100), 1);
        assert_eq!(t.level_of(1999), 2);
        assert_eq!(t.level_of(2000), 0, "past the last bound: 0");
        assert_eq!(t.icon_of_exp(600, "x"), Some("l2"));
        assert_eq!(atoi(" 42abc"), 42);
        assert_eq!(atoi("\"7\""), 7);
        assert_eq!(atoi("x"), 0);
    }

    #[test]
    fn columns_from_a_key_lookup() {
        let raw = |k: BoardKey| -> Option<String> {
            Some(
                match k {
                    BoardKey::PlacePos => "(30,4)",
                    BoardKey::LevelPos => "(36,1)",
                    BoardKey::LevelSize => "(30,22)",
                    BoardKey::HeaderFont => "3",
                    BoardKey::HeaderFontSize => "0",
                    _ => return None,
                }
                .to_string(),
            )
        };
        let c = BoardColumns::from_table(raw, None);
        assert_eq!(c.place, Some([30, 4]));
        assert_eq!(c.level, Some(([36, 1], [30, 22])));
        assert_eq!((c.header_font, c.header_font_size), (Some(3), None));
        assert_eq!(c.place_label, None, "no string table: no label");
        let img = row_icon(&c, &["i.dtx".to_string()], 0, "p", [100, 200]).unwrap();
        assert_eq!((img.pos, img.size), ([136, 201], [30, 22]));
    }

    #[test]
    fn row_mark_rules() {
        // w2-board
        let rec = dc_botclient::shell::RosterRecord {
            look: [0, 0, 0, 0, 5, 1, 0, 0, 0, 0, -1, -1, 0, 0, -1],
            name_colour: 0x11223344,
            mission: [1, 2, 3],
            clan: Some(dc_botclient::shell::RosterClan {
                name: "Owls".into(),
                group_no: 9,
                mark: [1, 2],
                colour: 0,
            }),
            ..Default::default()
        };
        let m = RowMark::of_record(&rec);
        assert_eq!(
            (m.look_level, m.name_colour, m.mission),
            (5, 0x11223344, [1, 2, 3])
        );
        assert_eq!(level_index(Some(&m), 2), 5);
        let mut neg = rec.clone();
        neg.look[4] = -3;
        let n = RowMark::of_record(&neg);
        assert_eq!(n.look_level, 0, "the look parser clamps to >= 0");
        assert_eq!(
            level_index(Some(&n), 2),
            2,
            "CLI-531: 0 falls back to the rank"
        );
        assert_eq!(level_index(None, 4), 4);
        assert_eq!(argb(0x80ff0102), [255, 1, 2, 0x80]);
        let row = [1, 1, 1, 255];
        let dead = [2, 2, 2, 255];
        assert_eq!(
            name_colour(Some(&m), row, None, Some(1)),
            [0x22, 0x33, 0x44, 0x11]
        );
        assert_eq!(name_colour(Some(&m), row, Some(dead), Some(1)), dead);
        assert_eq!(name_colour(Some(&m), row, None, Some(11)), row);
        assert_eq!(name_colour(None, row, None, None), row);
        let cols = BoardColumns {
            clan_colour: Some([7, 7, 7, 255]),
            ..Default::default()
        };
        assert_eq!(
            clan_text(&cols, Some(&m), None, row),
            Some(("Owls".to_string(), [7, 7, 7, 255]))
        );
        assert_eq!(
            clan_text(&BoardColumns::default(), Some(&m), None, row).map(|c| c.1),
            Some(row)
        );
        assert_eq!(
            clan_text(&cols, Some(&n), Some(dead), row).map(|c| c.1),
            Some(dead)
        );
        assert_eq!(clan_text(&cols, Some(&RowMark::default()), None, row), None);
        assert_eq!(mission_text(Some(&m), Some(1)), "1");
        assert_eq!(mission_text(Some(&m), Some(8)), "2/3");
        assert_eq!(mission_text(None, None), "0");
    }

    #[test]
    fn new_columns_from_a_key_lookup() {
        let raw = |k: BoardKey| -> Option<String> {
            Some(
                match k {
                    BoardKey::GuildAt => "(120,2)",
                    BoardKey::ClanNameColour => "<10,20,300>",
                    BoardKey::TaskAt => "(330,4)",
                    _ => return None,
                }
                .to_string(),
            )
        };
        let c = BoardColumns::from_table(raw, None);
        assert_eq!(c.clan_at, Some([120, 2]));
        assert_eq!(c.clan_colour, Some([10, 20, 255, 255]));
        assert_eq!(c.mission, Some([330, 4]));
        assert_eq!(
            (c.clan_label.clone(), c.mission_label.clone()),
            (None, None)
        );
        for k in [
            BoardKey::ClanLabel,
            BoardKey::ClanLabelPos,
            BoardKey::MissionLabel,
            BoardKey::MissionLabelPos,
            BoardKey::GuildAt,
            BoardKey::ClanNameColour,
            BoardKey::TaskAt,
        ] {
            assert_eq!(BOARD_PINS[k as usize].0, k, "pin table in enum order");
        }
    }
}
