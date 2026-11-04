//! Server select screen: the server list of the auth server (group button, one bar per server with its load), a
//! server information panel, Enter / double click picks the server. Picking sends 0x20a; the auth server's
//! answer opens the lobby connection and its start-up requests (the wait box shows meanwhile); the channel list
//! that follows opens the channel select screen.
//!
//! The 2009 client picks a server by itself on the first entry; dc-client highlights the same server and lets the
//! player confirm it (CLI-167). Opened again from the channel screen or the lobby (the lobby connection is up), the
//! screen asks the lobby for the list (0x3b0); picking the current server goes back to the channel screen, another
//! one moves there (0x3b2).
// SPEC: PROTOCOL_2009.md §2.2 (0x2000, 0x20a, 0x20b), §3.2 (server_entry, load thresholds 31 / 81 / 96)
// EVIDENCE: cshell+0x13a500 (build: tiled background, common page, "Server_select_page"), cshell+0x13b520 (group
// buttons into child 3 of window 1), cshell+0x13b750 (row fill into child 4 of window 1: thresholds, strings, images,
// ok / no icon above 100 %), cshell+0x13c430 (window 2), cshell+0x13cc70 (window 3), cshell+0x13bf70 (automatic
// pick), cshell+0x13d7b0 (pick, boxes), cshell+0x16b140 (bar style keys), cshell+0x16b770 (row element positions)
// OBSERVED: the page records of the user's ROM (children 3 and 4 of window 1 are its two list records)

use super::catalog::{colour, int, key, pair, Catalog, KeyPin};
use super::drawlist::{anchor_of, DynNotes, DynWant};
use super::link::{LinkCmd, LinkEvent, Wait};
use super::lobby::PanelArt;
use super::popup::{text_ids, BoxTag, Popup};
use super::result::grouped;
use super::sheet::{image, list_hit, list_row, place_text, Look, Part, Sheet, Tiles, ALIGN_CENTRE};
use super::{build_sheet, pages, Ctx, Go, MenuData, MenuKey, Screen, DOUBLE_CLICK_MS};
use crate::hud::items::DrawItem;
use dc_botclient::lobby::ServerEntry;
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::{UiScreen, WidgetKind};
use std::time::Instant;

/// Keys of the server / channel bar style in the screen's layout section.
// EVIDENCE: cshell+0x16b140 (bar textures, sizes, name colours), cshell+0x16b770 (index, name, state, state
// image and state icon positions)
pub mod bar_keys {
    use super::{key, KeyPin};
    pub const BAR: KeyPin = key("85820e6558c79123", 6);
    pub const BAR_OVER: KeyPin = key("9890097a5082e82f", 10);
    pub const WIDTH: KeyPin = key("4a0a89e74659ea35", 8);
    pub const HEIGHT: KeyPin = key("7b15cfe7354d7747", 9);
    pub const STATE_IMG_SIZE: KeyPin = key("cb3974ef28b613f5", 12);
    pub const STATE_ICON_SIZE: KeyPin = key("ce0b5077ce59df15", 13);
    pub const INDEX_AT: KeyPin = key("65fb375b13800675", 8);
    pub const NAME_AT: KeyPin = key("d5698f80e06d3478", 7);
    pub const STATE_AT: KeyPin = key("d81c20d304555303", 8);
    pub const STATE_IMG_AT: KeyPin = key("9f2de693ae39e44e", 11);
    pub const STATE_ICON_AT: KeyPin = key("770074296702a450", 12);
    /// Load colours: low, moderate, busy, full.
    pub const LOAD: [KeyPin; 4] = [
        key("f793de205ead5ac3", 3),
        key("5c42afc7a23018d3", 8),
        key("2b9cf77b67940911", 4),
        key("008dacb6d1e85bd8", 4),
    ];
    /// Name colours: full, normal, selected.
    pub const NAME_FULL: KeyPin = key("16fee3d8391e8b24", 14);
    pub const NAME_NORMAL: KeyPin = key("69d29752a7f07894", 16);
    pub const NAME_SELECTED: KeyPin = key("2d690c43ec1cd3cc", 16);
}

/// Load strings: low, moderate, busy, full; "checking" (closed channel).
// EVIDENCE: cshell+0x13b750 (0x32d, 0x32c, 0x32b, 0x481), cshell+0xfa860 (0x15c1)
pub const LOAD_TEXT: [i32; 4] = [0x32d, 0x32c, 0x32b, 0x481];
pub const CHECKING_TEXT: i32 = 0x15c1;
/// Server load thresholds (per cent below which the step applies).
// EVIDENCE: cshell+0x13b750 (`< 0x1f`, `< 0x51`, `< 0x60`)
pub const SERVER_LOAD_STEPS: [u32; 3] = [31, 81, 96];
/// The automatic pick skips servers at or above this load.
// EVIDENCE: cshell+0x13bf70
pub const PICK_MAX_LOAD: u32 = 95;

/// Bar look of a server or channel row.
#[derive(Debug, Clone, PartialEq)]
pub struct BarStyle {
    pub bar: Option<String>,
    pub bar_over: Option<String>,
    pub size: [i32; 2],
    pub state_img_size: [i32; 2],
    pub state_icon_size: [i32; 2],
    pub index_at: [i32; 2],
    pub name_at: [i32; 2],
    pub state_at: [i32; 2],
    pub state_img_at: [i32; 2],
    pub state_icon_at: [i32; 2],
    pub load: [u32; 4],
    pub name_full: u32,
    pub name_normal: u32,
    pub name_selected: u32,
}

/// Fallbacks for a missing key.
// DC-DEFAULT: CLI-168 (proposed) bar style fallbacks when a layout key is missing: 303 x 26 bar, grey / gold
// names, the load colours teal / olive / orange / red
const FALLBACK_LOAD: [u32; 4] = [0xff20_7484, 0xff77_841e, 0xffee_6000, 0xffc2_1927];

impl BarStyle {
    pub fn load(cat: &Catalog, screen: UiScreen) -> BarStyle {
        use bar_keys as k;
        let s = |key: &KeyPin| cat.style(screen, key);
        let p = |key: &KeyPin| pair(s(key)).unwrap_or([0, 0]);
        let mut load = FALLBACK_LOAD;
        for (i, key) in k::LOAD.iter().enumerate() {
            if let Some(c) = colour(s(key)) {
                load[i] = c;
            }
        }
        BarStyle {
            bar: s(&k::BAR).map(str::to_string),
            bar_over: s(&k::BAR_OVER).map(str::to_string),
            size: [
                int(s(&k::WIDTH)).unwrap_or(303),
                int(s(&k::HEIGHT)).unwrap_or(26),
            ],
            state_img_size: p(&k::STATE_IMG_SIZE),
            state_icon_size: p(&k::STATE_ICON_SIZE),
            index_at: p(&k::INDEX_AT),
            name_at: p(&k::NAME_AT),
            state_at: p(&k::STATE_AT),
            state_img_at: p(&k::STATE_IMG_AT),
            state_icon_at: p(&k::STATE_ICON_AT),
            load,
            name_full: colour(s(&k::NAME_FULL)).unwrap_or(0xff50_5050),
            name_normal: colour(s(&k::NAME_NORMAL)).unwrap_or(0xffaa_aaaa),
            name_selected: colour(s(&k::NAME_SELECTED)).unwrap_or(0xfffe_ad00),
        }
    }
}

/// One bar row to draw.
pub struct BarRow<'a> {
    /// Row number (0 = the first bar; names the row's layout keys in the draw trace).
    pub index: usize,
    pub name: &'a str,
    /// Load step 0..3, or `None` (closed / checking).
    pub step: Option<usize>,
    pub refused: bool,
    /// `step` is `None` because the row is closed: the checking picture shows (otherwise no picture).
    pub closed: bool,
    pub full: bool,
    pub selected: bool,
    pub hover: bool,
    /// LG-272: a row of another lobby shows this badge (text, colour) in the state's place and no load picture.
    pub badge: Option<(&'a str, u32)>,
}

/// Font index of the bar's texts.
// DC-DEFAULT: CLI-464 (proposed) the name and state texts use font record 14 (cshell+0x16b140 looks the font up
// through cshell+0x1c07b0, but its index argument is not visible in the decompiled C)
pub const BAR_FONT: i32 = 14;
/// Size of the bar's texts (the size slot of the string control's create call, as cshell+0x1b0bd0 fills it from a
/// text record's size).
// EVIDENCE: cshell+0x16b140 (name and state controls created with size 0xe), cshell+0x1b0bd0 (create argument
// order: text, ..., font, size, owner, alignment flags)
// OBSERVED: lobbyfx4/real/Realone_a_servers.png (2009, 1024 x 768: state text 24 px wide, cap 9 rows
// = size 14; ours at size 12 was 18 px wide, 7 rows)
pub const BAR_TEXT_SIZE: i32 = 14;
/// Alignment of the name and the state text about their positions: left, vertically centred (flags 0x41).
// EVIDENCE: cshell+0x16b140 (both controls created with alignment flags 0x41), SPEC FMT_CRES_UI.md §6.3 (value
// code `A`: l = 1, second letter c = 0x40)
// OBSERVED: lobbyfx4/real/Realone_a_servers.png (bar at x 221: name glyphs from x 253 = bar + the
// name position 32; state glyphs from x 447 = bar + the state position 226)
pub const BAR_TEXT_ALIGN: Align = Align {
    first: super::sheet::ALIGN_LEFT,
    second: ALIGN_SECOND_CENTRE,
};

/// What the 2009 bar asks of its name and state texts at bar position `at` (the draw trace's own copy of the
/// rule, apart from [`draw_bar`]): each at the bar plus its style position, left / vertically centred, font
/// [`BAR_FONT`], size [`BAR_TEXT_SIZE`]. The bar never draws its index text.
// EVIDENCE: cshell+0x16b770 (children moved to the bar position plus the index / name / state / state image /
// state icon key positions), cshell+0x16b680 (the bar draws its image, name, state, state image and state icon:
// the index control built by cshell+0x16b140 is never drawn)
pub fn bar_wants(st: &BarStyle, row: usize, at: [i32; 2]) -> [DynWant; 2] {
    let want = |pin: &KeyPin, p: [i32; 2]| DynWant {
        key: format!("dyn/bar{row}/{}", pin.sha16),
        text_at: [(at[0] + p[0]) as f32, (at[1] + p[1]) as f32],
        text_anchor: anchor_of(BAR_TEXT_ALIGN),
        font: Some(BAR_FONT),
        font_size: Some(BAR_TEXT_SIZE),
    };
    [
        want(&bar_keys::NAME_AT, st.name_at),
        want(&bar_keys::STATE_AT, st.state_at),
    ]
}

/// Draws one server / channel bar at `at`; with `notes` also notes what its texts ask for ([`bar_wants`]).
// EVIDENCE: cshell+0x16b770 (element positions relative to the bar), cshell+0x16b680 (what the bar draws),
// cshell+0x16b140 (text controls: size 0xe, alignment 0x41)
pub fn draw_bar(
    st: &BarStyle,
    cat: &mut Catalog,
    row: &BarRow,
    at: [i32; 2],
    out: &mut Vec<DrawItem>,
    mut notes: Option<&mut DynNotes>,
) {
    let wants = notes.as_ref().map(|_| bar_wants(st, row.index, at));
    let mut note = |out: &Vec<DrawItem>, i: usize| {
        if let (Some(n), Some(w)) = (notes.as_deref_mut(), wants.as_ref()) {
            n.push((out.len() - 1, w[i].clone()));
        }
    };
    let tex = if row.selected {
        st.bar.clone()
    } else if row.hover {
        st.bar_over.clone()
    } else {
        None
    };
    if let Some(t) = tex {
        out.push(image(&t, at, st.size));
    }
    let icon = if row.refused {
        cat.paths.state_no.clone()
    } else {
        cat.paths.state_ok.clone()
    };
    // CLI-910: a selected bar shows no state icon: the Click texture's double arrow sits in the icon's place, and the
    // icon drawn over it hid the arrow (the user's "arrow is behind the channel status icon").
    // OBSERVED: lobby-r10b/pairs/item2_channels_reference_rust.png (reference client: the clicked channel
    // row is the black bar with the arrow and no green icon; unselected and hovered rows keep the icon); the 2009
    // draw order (cshell+0x16b680) puts the icon last and the slot that hides it (the select call, vtable +0x94 of
    // cshell+0x16b8e0's class) is not traced, so this follows the reference stack and the art
    if let Some(t) = icon.filter(|_| !row.selected) {
        let size = sized(cat, &t, st.state_icon_size);
        out.push(image(&t, add(at, st.state_icon_at), size));
    }
    // EVIDENCE: cshell+0x16b140 (the normal colour: key NameColor_Full when the state string is 0x481, else
    // NameColor_Normal), cshell+0x16ba40 (the pointer on the bar: NameColor_Select)
    let name_colour = if row.selected || row.hover {
        st.name_selected
    } else if row.full {
        st.name_full
    } else {
        st.name_normal
    };
    out.push(place_text(
        row.name,
        add(at, st.name_at),
        [0, 0],
        BAR_TEXT_ALIGN,
        BAR_FONT,
        BAR_TEXT_SIZE,
        name_colour,
    ));
    note(out, 0);
    let img = match row.step {
        _ if row.badge.is_some() => None,
        Some(s) => cat.paths.state_images[s.min(3)].clone(),
        None if row.closed => cat.paths.state_images[4].clone(),
        None => None,
    };
    if let Some(t) = img {
        let size = sized(cat, &t, st.state_img_size);
        out.push(image(&t, add(at, st.state_img_at), size));
    }
    let (text, c) = match (row.badge, row.step) {
        (Some((b, c)), _) => (b.to_string(), c),
        (None, Some(s)) => (cat.text(LOAD_TEXT[s.min(3)]), st.load[s.min(3)]),
        (None, None) => (cat.text(CHECKING_TEXT), st.load[3]),
    };
    out.push(place_text(
        &text,
        add(at, st.state_at),
        [0, 0],
        BAR_TEXT_ALIGN,
        BAR_FONT,
        BAR_TEXT_SIZE,
        c,
    ));
    note(out, 1);
}

fn add(a: [i32; 2], b: [i32; 2]) -> [i32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn sized(cat: &mut Catalog, tex: &str, size: [i32; 2]) -> [i32; 2] {
    if size[0] > 0 && size[1] > 0 {
        return size;
    }
    cat.tex_size(tex)
        .map_or([16, 16], |(w, h)| [w as i32, h as i32])
}

/// Load per cent of a server (100 for a server without capacity).
pub fn load_pct(e: &ServerEntry) -> u32 {
    if e.capacity == 0 {
        100
    } else {
        ((e.current as u64 * 100) / e.capacity as u64) as u32
    }
}

pub fn server_step(pct: u32) -> usize {
    SERVER_LOAD_STEPS.iter().position(|t| pct < *t).unwrap_or(3)
}

/// The automatic pick: the first entry when it is connectable, below 95 % and open to the player; else the first
/// connectable non-clan entry below 95 %; else the first connectable entry open to the player. A clan server is open
/// to a clan member or an operator (`clan_ok`).
// EVIDENCE: cshell+0x13bf70, cshell+0x139d70 (connectable; its level, rate and grade limits are CLI-1690)
pub fn auto_pick(servers: &[ServerEntry], clan_ok: bool, operator: bool) -> Option<usize> {
    let open = |e: &ServerEntry| {
        super::gm::server_open_to(e, operator) && (e.clan_only == 0 || clan_ok)
    };
    if servers
        .first()
        .is_some_and(|e| open(e) && load_pct(e) < PICK_MAX_LOAD)
    {
        return Some(0);
    }
    servers
        .iter()
        .position(|e| open(e) && e.clan_only == 0 && load_pct(e) < PICK_MAX_LOAD)
        .or_else(|| servers.iter().position(open))
}

/// A clan server is open to a clan member or an operator.
fn clan_ok(data: &MenuData) -> bool {
    data.char_extras.clan_id != 0 || data.gm.operator
}

/// Window and list ids of the screen.
pub const LIST_WINDOW: i32 = 1;
pub const GROUP_LIST: i32 = 2;
pub const SERVER_LIST: i32 = 3;
pub const CHAR_WINDOW: i32 = 2;
pub const INFO_WINDOW: i32 = 3;
/// Group button size and caption place.
// EVIDENCE: cshell+0x13b520 (group buttons from the group button prefix: rectangle 0xab x 0x22; caption about (0x50,
// 0x11) with flags 0x40044, centred on that point; font 0xe, size 0xe; all four colours -1)
pub const GROUP_BUTTON: [i32; 2] = [0xab, 0x22];
pub const GROUP_CAPTION_AT: [i32; 2] = [0x50, 0x11];
pub const GROUP_CAPTION_SIZE: i32 = 0xe;
pub const GROUP_CAPTION_COLOUR: u32 = 0xffff_ffff;
/// Command numbers.
pub const CMD_RELOAD_LIST: i32 = 84;
pub const CMD_ENTER: i32 = 10002;
pub const CMD_FRIENDS_PANEL: i32 = 310;
pub const CMD_QUIT: i32 = 303;

pub struct Servers {
    sheet: Sheet,
    style: BarStyle,
    selected: Option<usize>,
    hover: Option<usize>,
    last_click: Option<(usize, Instant)>,
    busy: Wait,
    /// The entry a server move goes to (it becomes the current server when its channel list arrives).
    moving: Option<ServerEntry>,
    /// A pick or a move waits for its lobby's channel list (a late list of the channel screen just left does not
    /// switch the screen).
    connecting: bool,
    /// The level and grade tables of the Character Info panel (none without a ROM).
    art: Option<PanelArt>,
    /// LG-272: the Mods button of the modded servers' panel.
    mods_button: super::modbrowser::Buttons,
    /// CLI-1843: the Test button of the connection test panel ([`super::diagbox`]).
    test_button: super::modbrowser::Buttons,
}

/// LG-272: the rows of the list: this lobby's entries, then the other lobbies ([`super::moddedrows::modded_rows`]).
fn row_count(data: &MenuData) -> usize {
    data.servers.len() + super::moddedrows::modded_rows(data).len()
}

impl Servers {
    pub fn new(cat: &Catalog, data: &MenuData) -> Servers {
        let mut sheet = build_sheet(cat, &[pages::COMMON, pages::SERVERS]);
        if let Some(prefix) = &cat.paths.server_back {
            sheet.tiles = Tiles::new([0, 0], 3, 4, prefix);
        }
        // EVIDENCE: cshell+0x13a500 (child 1 of window 2 and child 0 of window 3 off with vtable slot 0x9c and 0)
        // DC-DEFAULT: CLI-1600 (proposed) control 2 of window 2, the grade icon the 2009 panel sets, stays hidden
        sheet.show(CHAR_WINDOW, 1, WidgetKind::Image, false);
        sheet.show(CHAR_WINDOW, 2, WidgetKind::Image, false);
        sheet.show(INFO_WINDOW, 0, WidgetKind::Image, false);
        let mut s = Servers {
            sheet,
            style: BarStyle::load(cat, UiScreen::Screen11),
            selected: auto_pick(&data.servers, clan_ok(data), data.gm.operator),
            hover: None,
            last_click: None,
            busy: Wait::default(),
            moving: None,
            connecting: false,
            art: cat.rom.as_deref().and_then(PanelArt::cached),
            mods_button: super::moddedrows::mods_button(),
            test_button: super::diagbox::test_button(),
        };
        // back from the channel screen or the lobby: the current server is the highlighted one
        // EVIDENCE: cshell+0x13d774 (not the screen's own pick: the highlight id = the shell's server id, which
        // cshell+0x13b750 selects; cshell+0x13d790: no highlight id on its own pick)
        if let Some(cur) = data.server.as_ref().filter(|_| data.lobby_up) {
            if let Some(i) = data.servers.iter().position(|e| e.id == cur.id) {
                s.selected = Some(i);
            }
        }
        s.fill_char(cat, data);
        if let Some(i) = s.selected {
            s.fill_info(cat, &data.servers[i]);
        }
        s
    }

    /// The server information of row `i` (an entry of this lobby or another lobby's row).
    fn fill_row(&mut self, cat: &Catalog, data: &MenuData, i: usize) {
        if let Some(e) = data.servers.get(i).cloned() {
            self.fill_info(cat, &e);
            return;
        }
        let rows = super::moddedrows::modded_rows(data);
        if let Some(r) = rows.get(i - data.servers.len()) {
            // CLI-1623: the name and region only, never the address
            let e = ServerEntry {
                id: 0,
                name: super::moddedrows::row_label(r),
                ip: String::new(),
                port: 0,
                open: 1,
                capacity: 0,
                current: 0,
                clan_only: 0,
                op_capacity: 0,
                op_current: 0,
            };
            self.fill_info(cat, &e);
        }
    }

    fn list(&self) -> Option<&Part> {
        self.sheet.part(LIST_WINDOW, SERVER_LIST, WidgetKind::List)
    }

    /// The Character Info panel (window 2): the login reply's record (level, progress, kills, deaths, clan) from the
    /// first entry on, the lobby login's (nickname, NX prepaid / credit, grade, GP) once it is in.
    // EVIDENCE: cshell+0x13c430 (window 2: text 3 name, image 0 rank icon of the experience (shell +0x194), text 4
    // short level name, text 16 `%3.2f %%` of the level, texts 17 / 18 kills / deaths (+0xfc / +0x100), text 7 clan
    // name or string 0x41a without a clan id (+0x124), text 9 prepaid (+0x11c), text 11 credit (+0x108), text 13
    // grade name (+0x440), text 15 GP (+0x104); numbers with thousands separators, cshell+0x1b3ed0)
    // DC-DEFAULT: CLI-1600 (proposed) the lobby login's values stay empty until the lobby login (the 2009 shows its
    // zero record, unseen there as it picks a server by itself); the record's run-time colours are not set
    fn fill_char(&mut self, cat: &Catalog, data: &MenuData) {
        let p = &data.profile;
        let x = &data.char_extras;
        let w = CHAR_WINDOW;
        let s = &mut self.sheet;
        s.set_text(w, 17, &x.kills.to_string());
        s.set_text(w, 18, &x.deaths.to_string());
        // EVIDENCE: cshell+0x13c430 (no clan -> string 0x41a)
        let clan = if x.clan_id != 0 {
            x.clan_label.clone()
        } else {
            cat.text(0x41a)
        };
        s.set_text(w, 7, &clan);
        let lobby = p.char_no != 0;
        if lobby {
            s.set_text(w, 3, &p.nick);
            s.set_text(w, 9, &grouped(i64::from(data.balance.1)));
            s.set_text(w, 11, &grouped(i64::from(data.balance.0)));
            s.set_text(w, 15, &grouped(i64::from(p.gp)));
        }
        let Some(art) = self.art.as_ref() else {
            return;
        };
        s.set_text(w, 4, art.short_name(p.exp));
        let (part, span) = art.part_span(p.exp);
        let pct = if span == 0 {
            0.0
        } else {
            f64::from(part) * 100.0 / f64::from(span)
        };
        s.set_text(w, 16, &format!("{pct:3.2} %"));
        if let Some(icon) = art.levels.icon_of_exp(p.exp, &p.nick) {
            if let Some(part) = s.part_mut(w, 0, WidgetKind::Image) {
                if let Look::Panel { tex, .. } = &mut part.look {
                    *tex = icon.to_string();
                }
            }
        }
        if lobby {
            let grade = art.grade(x.grade).map_or("", |r| r.name.as_str());
            s.set_text(w, 13, grade);
        }
    }

    // EVIDENCE: cshell+0x13cc70 (window 3: control 3 ": %s" of the name; 5, 7, 13 ": %s" of string 0x188e without a
    // requirement; 9, 11 string 0x18d9 without a bonus; 15 string 0x18e0 on a clan server, else ": %s" of 0x188e)
    // DC-DEFAULT: CLI-1690 (proposed) the entry's requirement and bonus fields are not read: always "none"
    fn fill_info(&mut self, cat: &Catalog, e: &ServerEntry) {
        let none = format!(": {}", cat.text(0x188e));
        let w = INFO_WINDOW;
        self.sheet.set_text(w, 3, &format!(": {}", e.name));
        for id in [5, 7, 13] {
            self.sheet.set_text(w, id, &none);
        }
        self.sheet.set_text(w, 9, &cat.text(0x18d9));
        self.sheet.set_text(w, 11, &cat.text(0x18d9));
        if e.clan_only == 0 {
            self.sheet.set_text(w, 15, &none);
        } else {
            self.sheet.set_text(w, 15, &cat.text(0x18e0));
        }
    }

    /// CLI-1617: the Mods screen for the highlighted row (an entry of this lobby: this lobby's set; another lobby's
    /// row: its set from the community document or an earlier join), never its address (CLI-1623).
    fn open_mods(&mut self, ctx: &mut Ctx) {
        let d = &*ctx.data;
        ctx.data.mods_target = match self.selected {
            Some(i) if i < d.servers.len() => {
                let mut t = super::modbrowser::current_target(d);
                t.name = d.servers[i].name.clone();
                Some(t)
            }
            Some(i) => super::moddedrows::modded_rows(d)
                .get(i - d.servers.len())
                .map(|r| super::modbrowser::ModsTarget {
                    name: r.name.clone(),
                    region: r.region.clone(),
                    modded: r.modded,
                    addr: r.addr,
                    set: r.expect.clone(),
                }),
            None => None,
        };
        *ctx.go = Some(Go::Mods);
    }

    fn pick(&mut self, ctx: &mut Ctx) {
        let Some(i) = self.selected else {
            return;
        };
        // LG-272 / CLI-1622: a row of another lobby: leave this one and join it
        if i >= ctx.data.servers.len() {
            let rows = super::moddedrows::modded_rows(ctx.data);
            if let Some(r) = rows.get(i - ctx.data.servers.len()) {
                if !(self.busy.busy() && self.connecting) {
                    super::moddedrows::join(r, ctx);
                }
            }
            return;
        }
        let Some(e) = ctx.data.servers.get(i).cloned() else {
            return;
        };
        // a list request without an answer stops holding the screen after the reply wait
        // DC-DEFAULT: CLI-163 (proposed) menu reply wait 10 s
        if self.busy.busy() && !(self.busy.expired() && !self.connecting) {
            return;
        }
        // the current server again: back to its channels without a connection change
        // EVIDENCE: cshell+0x13d7b0 (0x2711: the entry's id equal to the shell's server id -> screen 0xc)
        if ctx.data.lobby_up && ctx.data.server.as_ref().is_some_and(|c| c.id == e.id) {
            *ctx.go = Some(Go::Channels);
            return;
        }
        // gm-client (CLI-1304): an operator enters by the operator slots (no load or limit checks exist here)
        // EVIDENCE: cshell+0x13da9a (connectable: a clan server without a clan or the operator flag -> box 0x4db, title
        // 0x371), cshell+0x13dd19 (not connectable: full -> box 0x3f5, title 0x371; else box 0x4dc, title 0x346)
        if !super::gm::server_open_to(&e, ctx.data.gm.operator) {
            if e.current < e.capacity {
                ctx.error(text_ids::SERVER_NO_ENTRY, BoxTag::None);
            } else {
                ctx.notice(text_ids::SERVER_FULL);
            }
            return;
        }
        if e.clan_only != 0 && !clan_ok(ctx.data) {
            ctx.notice(text_ids::SERVER_CLAN_ONLY);
            return;
        }
        if ctx.data.lobby_up {
            // another server while the lobby connection is up: the server move, its wait box
            // EVIDENCE: cshell+0x13d7b0 (0x2711: 0x3b2 with the entry's id through cshell+0x18f010, box 0x40b)
            self.moving = Some(e.clone());
            ctx.send(LinkCmd::MoveServer(e.id));
        } else {
            ctx.data.server = Some(e.clone());
            ctx.send(LinkCmd::PickServer(e.id));
        }
        self.busy.start();
        self.connecting = true;
        ctx.wait_box(text_ids::CONNECTING);
    }

    /// The list again from the lobby (the screen's focus after the first entry, the refresh button).
    // EVIDENCE: cshell+0x13d6ab (focus not after the main screen: busy, 0x3b0), cshell+0x13d7b0 (command 0x54: 0x3b0
    // unless busy)
    fn ask_list(&mut self, ctx: &mut Ctx) {
        if ctx.data.lobby_up && (!self.busy.busy() || self.busy.expired()) {
            ctx.send(LinkCmd::ServerList);
            self.busy.start();
        }
    }
}

impl Screen for Servers {
    fn list_rows(&self, data: &MenuData) -> Vec<(i32, i32, usize, i32)> {
        vec![(
            LIST_WINDOW,
            SERVER_LIST,
            row_count(data),
            self.style.size[1],
        )]
    }
    fn name(&self) -> &'static str {
        "servers"
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    // EVIDENCE: cshell+0x13a500 (the server select build calls cshell+0x1ae780: the common page's window 0 plays the
    // navigation sounds)
    fn common_nav(&self) -> bool {
        true
    }

    fn enter(&mut self, ctx: &mut Ctx) {
        // CLI-1622: the server list asked for from a lobby of the list: back to the lobby the player came from
        if ctx.data.lobby_up && ctx.data.modded_join.is_some() {
            super::moddedrows::go_home(ctx);
            return;
        }
        // CLI-1622: the joined lobby's own list: its first entry is picked by itself, its channels open
        if ctx.data.auto_pick && ctx.data.modded_join.is_some() && !ctx.data.lobby_up {
            ctx.data.auto_pick = false;
            let d = &*ctx.data;
            if let Some(i) = auto_pick(&d.servers, clan_ok(d), d.gm.operator) {
                self.selected = Some(i);
                self.pick(ctx);
                return;
            }
        }
        // CLI-1632: the modded rows' public sets, once per opening; the list does not wait for them
        let rows = super::modbrowser::row_set_requests(ctx.data);
        if !rows.is_empty() {
            ctx.send(LinkCmd::ReadRowSets(rows));
        }
        if ctx.data.lobby_up {
            self.ask_list(ctx);
        } else if ctx.data.servers.is_empty() {
            ctx.error(text_ids::NO_SERVER, BoxTag::ToLogin);
        }
    }

    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        let n = row_count(ctx.data);
        match k {
            MenuKey::Enter => self.pick(ctx),
            MenuKey::Escape => {
                let t = ctx.cat.text(text_ids::ASK_QUIT);
                ctx.popups.push(Popup::ask(t, BoxTag::Quit));
            }
            MenuKey::Down if n > 0 => {
                self.selected = Some(self.selected.map_or(0, |s| (s + 1).min(n - 1)));
            }
            MenuKey::Up if n > 0 => {
                self.selected = Some(self.selected.map_or(0, |s| s.saturating_sub(1)));
            }
            _ => {}
        }
        if let Some(i) = self.selected {
            self.fill_row(ctx.cat, ctx.data, i);
        }
    }

    fn click(&mut self, command: i32, _part: usize, ctx: &mut Ctx) {
        match command {
            CMD_ENTER => self.pick(ctx),
            CMD_QUIT => self.key(MenuKey::Escape, ctx),
            // the list came with the login; a refresh needs the lobby's server list (0x3b0), which this screen
            // does not have before the pick
            // DC-DEFAULT: CLI-167 (proposed) refresh / messenger do nothing on the first server select
            CMD_RELOAD_LIST => self.ask_list(ctx),
            CMD_FRIENDS_PANEL => {}
            // LG-272: the Mods window (CLI-1617) from the server list
            super::modbrowser::CMD_MODS => self.open_mods(ctx),
            // CLI-1843: the connection test of every row
            super::diagbox::CMD_TEST => {
                super::diagbox::test_all(ctx.data);
            }
            _ => {}
        }
    }

    fn press(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        // CLI-1843: the connection test panel's Test button
        if self.test_button.press(p) {
            self.test_button.release(p);
            ctx.sound(super::frontfx::SOUND_SELECT);
            super::diagbox::test_all(ctx.data);
            return true;
        }
        if ctx.data.modded_join.is_none() && self.mods_button.press(p) {
            self.mods_button.release(p);
            ctx.sound(super::frontfx::SOUND_SELECT);
            self.open_mods(ctx);
            return true;
        }
        let Some(list) = self.list().cloned() else {
            return false;
        };
        let n = row_count(ctx.data);
        let Some(i) = list_hit(&list, self.style.size[0], self.style.size[1], n, 0, p) else {
            return false;
        };
        let double = self
            .last_click
            .is_some_and(|(j, t)| j == i && t.elapsed().as_millis() < DOUBLE_CLICK_MS);
        self.last_click = if double {
            None
        } else {
            Some((i, Instant::now()))
        };
        self.selected = Some(i);
        self.fill_row(ctx.cat, ctx.data, i);
        if double {
            self.pick(ctx);
        }
        true
    }

    fn hover(&mut self, p: [f32; 2], data: &MenuData) {
        self.mods_button.moved(p);
        self.test_button.moved(p);
        self.hover = self.list().and_then(|l| {
            list_hit(
                l,
                self.style.size[0],
                self.style.size[1],
                row_count(data),
                0,
                p,
            )
        });
    }

    fn event(&mut self, e: &LinkEvent, ctx: &mut Ctx) {
        match e {
            LinkEvent::Profile(_)
            | LinkEvent::LoginRecord { .. }
            | LinkEvent::CharInfo(_)
            | LinkEvent::Grade(_)
            | LinkEvent::Balance { .. } => {
                let data = &*ctx.data;
                self.fill_char(ctx.cat, data);
            }
            LinkEvent::Servers(v) => {
                self.busy.stop();
                // the current server stays highlighted in the new list
                let cur = ctx.data.server.as_ref().map(|c| c.id);
                self.selected = cur
                    .and_then(|id| v.iter().position(|e| e.id == id))
                    .or(self.selected.filter(|i| *i < v.len()));
                if let Some(e) = self.selected.and_then(|i| v.get(i)).cloned() {
                    self.fill_info(ctx.cat, &e);
                }
            }
            LinkEvent::Channels(_) if self.connecting => {
                self.connecting = false;
                self.busy.stop();
                ctx.close_wait();
                if let Some(e) = self.moving.take() {
                    ctx.data.server = Some(e);
                }
                *ctx.go = Some(Go::Channels);
            }
            // DC-DEFAULT: CLI-1690 (proposed) a refused 0x20b says so in a box (cshell+0x13dff0 opens none)
            LinkEvent::ServerRefused(_) => {
                self.connecting = false;
                self.busy.stop();
                ctx.close_wait();
                ctx.error(text_ids::SERVER_NO_ENTRY, BoxTag::None);
            }
            // DC-DEFAULT: LG-313 (proposed) a featured entry's refusal or pack failure says so in words
            LinkEvent::EntryRefused(text) => {
                self.connecting = false;
                self.busy.stop();
                self.moving = None;
                ctx.close_wait();
                ctx.popups.push(Popup::error(text.clone(), BoxTag::None));
            }
            // EVIDENCE: cshell+0x13e200 (0x3b3 refused -> box 0x3f5)
            LinkEvent::MoveRefused => {
                self.connecting = false;
                self.busy.stop();
                self.moving = None;
                ctx.close_wait();
                ctx.error(text_ids::SERVER_FULL, BoxTag::None);
            }
            _ => {}
        }
    }

    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, _secs: f32, out: &mut Vec<DrawItem>) {
        self.draw_rows(cat, data, out, None);
        super::diagbox::draw_panel(cat, data, self.selected, &self.test_button, out);
    }

    fn draw_extra_noted(
        &self,
        cat: &mut Catalog,
        data: &MenuData,
        _secs: f32,
        out: &mut Vec<DrawItem>,
        notes: &mut DynNotes,
    ) {
        self.draw_rows(cat, data, out, Some(notes));
        super::diagbox::draw_panel(cat, data, self.selected, &self.test_button, out);
    }
}

impl Servers {
    fn draw_rows(
        &self,
        cat: &mut Catalog,
        data: &MenuData,
        out: &mut Vec<DrawItem>,
        mut notes: Option<&mut DynNotes>,
    ) {
        // the group list: one button for every server ("all"), selected
        if let Some(g) = self.sheet.part(LIST_WINDOW, GROUP_LIST, WidgetKind::List) {
            if let Some(prefix) = cat.paths.group_button.clone() {
                let tex = format!("{prefix}C.dtx");
                out.push(image(&tex, g.pos, GROUP_BUTTON));
            }
            // DC-DEFAULT: CLI-167 (proposed) one group button captioned "All" (the 2009 group names are not read)
            out.push(place_text(
                "All",
                add(g.pos, GROUP_CAPTION_AT),
                [0, 0],
                Align {
                    first: ALIGN_CENTRE,
                    second: ALIGN_SECOND_CENTRE,
                },
                BAR_FONT,
                GROUP_CAPTION_SIZE,
                GROUP_CAPTION_COLOUR,
            ));
        }
        let Some(list) = self.list() else {
            return;
        };
        for (i, e) in data.servers.iter().enumerate() {
            // rows scrolled above the list or below its end are not drawn
            let Some(at) = list_row(list, self.style.size[1], i, 0) else {
                continue;
            };
            let pct = load_pct(e);
            let row = BarRow {
                index: i,
                name: if e.name.is_empty() { "-" } else { &e.name },
                step: Some(server_step(pct)),
                refused: pct > 100,
                closed: false,
                full: pct >= SERVER_LOAD_STEPS[2],
                selected: self.selected == Some(i),
                hover: self.hover == Some(i),
                badge: None,
            };
            draw_bar(&self.style, cat, &row, at, out, notes.as_deref_mut());
        }
        // LG-272: the other lobbies under this lobby's entries, and their panel with the Mods button
        if data.modded_join.is_some() {
            return;
        }
        let others = super::moddedrows::modded_rows(data);
        let n = data.servers.len();
        for (k, r) in others.iter().enumerate() {
            let i = n + k;
            let Some(at) = list_row(list, self.style.size[1], i, 0) else {
                continue;
            };
            // the name and region end before the badge
            let room = self.style.state_at[0] - self.style.name_at[0] - 8;
            let label = super::modbrowser::fit(cat, &super::moddedrows::row_label(r), room);
            let (badge, colour) = if r.modded {
                (super::modbrowser::T_MODDED, super::modbrowser::GOLD)
            } else {
                (super::modbrowser::T_VANILLA, super::modbrowser::BLUE)
            };
            let row = BarRow {
                index: i,
                name: &label,
                step: None,
                refused: r.addr.is_none(),
                closed: true,
                full: false,
                selected: self.selected == Some(i),
                hover: self.hover == Some(i),
                badge: Some((badge, colour)),
            };
            draw_bar(&self.style, cat, &row, at, out, None);
        }
        let sel = self.selected.and_then(|s| s.checked_sub(n));
        super::moddedrows::draw_panel(cat, &others, sel, &self.mods_button, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u32, cap: u32, cur: u32, clan: u32) -> ServerEntry {
        ServerEntry {
            id,
            name: format!("s{id}"),
            ip: "127.0.0.1".into(),
            port: 1,
            open: 1,
            capacity: cap,
            current: cur,
            clan_only: clan,
            op_capacity: 0,
            op_current: 0,
        }
    }

    #[test]
    fn load_steps_and_automatic_pick_follow_the_2009_thresholds() {
        assert_eq!(server_step(30), 0);
        assert_eq!(server_step(31), 1);
        assert_eq!(server_step(80), 1);
        assert_eq!(server_step(95), 2);
        assert_eq!(server_step(96), 3);
        let v = vec![
            entry(1, 100, 99, 0),
            entry(2, 100, 10, 1),
            entry(3, 100, 50, 0),
        ];
        // 99 %, then a clan server, then the first free one
        assert_eq!(auto_pick(&v, false, false), Some(2));
        let full = vec![entry(1, 100, 96, 0)];
        assert_eq!(auto_pick(&full, false, false), Some(0));
        // a clan server is no fallback without a clan, the first choice with one
        let clan = vec![entry(1, 100, 10, 1), entry(2, 100, 96, 0)];
        assert_eq!(auto_pick(&clan, false, false), Some(1));
        assert_eq!(auto_pick(&clan, true, false), Some(0));
        // no capacity: not connectable
        assert_eq!(auto_pick(&[entry(1, 0, 0, 0)], false, false), None);
    }
}
