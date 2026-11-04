//! The player's view of mods (project decision): the community server browser (CLI-1616), the Mods screen
//! with its Browse and My Mods tabs, one row per mod and a detail panel (CLI-1617), and the consent
//! prompt for unverified server mods before anything is downloaded (CLI-1618).
//!
//! None of these has a 2009 counterpart: the 2009 server-select screen lists the auth server's own entries (0x2000)
//! after the login and cannot carry a lobby address, badges or mod names, so the browser is a window of its own
//! before the login, opened by the login screen's homepage button (CLI-166 had left it dead). The windows are drawn
//! by our own layout code with the ROM's generic window frame (the rename window's frame in nine parts, as the
//! account dialog of CLI-1450 uses it) and the message boxes' button pictures.
// SPEC: HOSTING_AND_MODS.md §4, §5, §8.2; docs/MODDING_GUIDE.md §7, §7.1, §10
// DC-DEFAULT: CLI-1616 (proposed, project decision) the browser window, its rows and badges, the cache;
// CLI-1617 the Mods window and the local enable list; CLI-1618 the consent prompt and its memory; see DC_DEFAULTS.md

use std::cell::Cell;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Instant;

use dc_config::directory::{badges, Badge};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};

use super::catalog::Catalog;
use super::link::LinkCmd;
use super::modsettings::{self, SettingsPage};
use super::modcache::{list_cache, open_folder, remove_cached, size_words, CachedMod};
use super::popup::{BoxTag, Popup};
use super::sheet::{place_text, Sheet, ALIGN_CENTRE, ALIGN_LEFT, ALIGN_RIGHT};
use super::usebox::{
    fill, inside, measure, NameArt, FONT, FONT_SIZE, MESSAGE_COLOUR, TEXT_COLOUR, TITLE_COLOUR,
};
use super::{Ctx, Go, MenuData, MenuInput, MenuKey, Screen, DOUBLE_CLICK_MS};
use crate::community::{self, CommunityView};
use crate::hud::compose::Tex;
use crate::hud::items::DrawItem;
use crate::mods::{self, JoinReport, LocalMod, ModPaths, ModState, Policy, ServerMod, ServerModSet};

pub const COMMUNITY_NAME: &str = "community";
pub const MODS_NAME: &str = "mods";

/// Commands of the windows' buttons (also reachable as [`MenuInput::Command`]).
pub const CMD_JOIN: i32 = 110_001;
pub const CMD_MODS: i32 = 110_002;
pub const CMD_CLOSE: i32 = 110_003;
/// Row `i` selected: `CMD_ROW_BASE + i`; local mod `i` toggled: `CMD_TOGGLE_BASE + i`.
pub const CMD_ROW_BASE: i32 = 111_000;
pub const CMD_TOGGLE_BASE: i32 = 112_000;
pub const CMD_FORGET_BASE: i32 = 113_000;
pub const CMD_REMOVE_BASE: i32 = 114_000;
pub const CMD_OPEN_FOLDER: i32 = 110_004;
/// CLI-1632: the "Downloaded mods on this PC" section's Clean up button.
pub const CMD_CLEAN_UP: i32 = 110_005;
/// The Mods window's tabs.
pub const CMD_TAB_BROWSE: i32 = 110_006;
pub const CMD_TAB_MINE: i32 = 110_007;
/// Local mod `i`'s settings page / download of optional mod `i` / Remove of downloaded local mod `i`.
pub const CMD_SETTINGS_BASE: i32 = 117_000;
pub const CMD_DOWNLOAD_BASE: i32 = 118_000;
pub const CMD_REMOVE_LOCAL_BASE: i32 = 119_000;
/// The consent prompt's buttons (the login screen routes them).
pub const CMD_AGREE: i32 = 110_101;
pub const CMD_DECLINE: i32 = 110_102;

/// Our texts (the ROM has none for these windows).
pub const T_SERVERS: &str = "Servers";
pub const T_MODS: &str = "Mods";
pub const T_JOIN: &str = "Join";
pub const T_CLOSE: &str = "Close";
pub const T_VANILLA: &str = "VANILLA";
pub const T_MODDED: &str = "MODDED";
pub const T_VERIFIED: &str = "Verified";
pub const T_UNVERIFIED: &str = "Unverified";
pub const T_ACCEPTED: &str = "Accepted";
pub const T_CONFIGURED: &str = "Configured lobby";
pub const T_LIST_OFF: &str = "The community list is off: only the configured lobby is shown.";
pub const T_LIST_LOADING: &str = "Reading the community list...";
pub const T_ENABLE: &str = "Enable";
pub const T_DISABLE: &str = "Disable";
pub const T_SET_AT_JOIN: &str = "Its mods show when you join it.";
pub const T_IN_USE: &str = "In use now.";
pub const T_REMOVE: &str = "Remove";
pub const T_DOWNLOAD: &str = "Get";
pub const T_INSTALL: &str = "Install";
pub const T_UPDATE: &str = "Update";
pub const T_INSTALLED: &str = "Installed";
pub const T_WAIT: &str = "Wait...";
pub const T_SETTINGS: &str = "Settings";
pub const T_BACK: &str = "Back";
pub const T_CLEAN_UP: &str = "Remove unused";
pub const T_FORGET: &str = "Revoke trust";
pub const T_OPEN_FOLDER: &str = "Open folder";
pub const T_NEXT_JOIN: &str = "Changes apply the next time you join a server.";
pub const T_BROWSE: &str = "Browse";
pub const T_MY_MODS: &str = "My Mods";
pub const T_ON: &str = "On";
pub const T_OFF: &str = "Off";
pub const T_CTX_MODDED: &str = "Modded server: mods load when you join";
pub const T_CTX_VANILLA: &str = "Vanilla server: no mods run here";
pub const T_NOTHING_INSTALLED: &str = "Nothing installed yet. Get mods on the Browse tab.";
pub const T_REQUIRED_BY: &str = "Required by";
pub const T_UNUSED: &str = "Unused";
pub const T_NOT_NEEDED: &str = "No server you know needs it any more.";
pub const T_KEPT: &str = "Kept";
pub const T_KEPT_NOTE: &str = "Kept for now: not every server in your list has told which mods it needs yet.";
pub const T_WITH_SERVER: &str = "With server";
pub const T_CANT_LOAD: &str = "Can't load";
pub const T_WHAT_IT_DOES: &str = "What it does";
pub const T_COMMANDS: &str = "Commands";
pub const T_LOOKS_ONLY: &str = "Changes how the game looks, on your screen only";
pub const T_OFF_VANILLA: &str = "Off on vanilla servers.";
pub const T_OFF_NOT_ALLOWED: &str = "Off here: this server does not allow your own mods.";
pub const T_CONSENT_TITLE: &str = "This server runs mods that are not verified";
pub const T_CONSENT_NOTE: &str = "Verified mods are listed in the community directory. Accept only if you trust \
this server and the authors; nothing is downloaded before you accept.";
pub const T_ACCEPT: &str = "Accept";
pub const T_CANCEL: &str = "Cancel";

/// Badge colours.
// DC-DEFAULT: CLI-1616 (proposed) green verified / vanilla, orange unverified / modded
pub const GREEN: u32 = 0xff5a_d25a;
pub const ORANGE: u32 = 0xffff_a040;
pub const BLUE: u32 = 0xff8f_d0ff;
pub const GOLD: u32 = 0xffff_c040;

/// The browser and Mods windows: 800 x 600 in the middle of the virtual screen.
pub const WINDOW_AT: [i32; 2] = [112, 84];
pub const WINDOW_SIZE: [i32; 2] = [800, 600];
/// A list row and how many show.
pub const ROW_H: i32 = 44;
pub const LIST_TOP: i32 = 64;
pub const LIST_ROWS: usize = 8;
/// The button picture's size and caption place (the message boxes' buttons).
pub const BUTTON: [i32; 2] = [100, 25];
const LINE: i32 = FONT_SIZE + 4;
/// The visible part of the button texture: the file is padded to a power of two (128 x 32) past its art, so the nine
/// parts are cut from this size and never from the texture's.
// OBSERVED: 2026-11-01 alpha of the three button textures of the EU-test client (opaque 0..=101 x 0..=24)
pub const BUTTON_ART: (u32, u32) = (102, 25);
const SWITCH_KNOB: i32 = 12;
/// A disabled button: dimmed art, a caption light enough to read on it.
const DISABLED_TINT: [u8; 4] = [130, 130, 130, 255];
const CAPTION_DISABLED: u32 = 0xffc8_c8c8;

const TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};
const TR: Align = Align {
    first: ALIGN_RIGHT,
    second: 0,
};
const MID: Align = Align {
    first: ALIGN_CENTRE,
    second: ALIGN_SECOND_CENTRE,
};

/// Where the community list stands.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum CommunityState {
    /// `--community off` or no URL configured.
    #[default]
    Off,
    Loading,
    Ready(CommunityView),
}

impl CommunityState {
    pub fn view(&self) -> Option<&CommunityView> {
        match self {
            CommunityState::Ready(v) => Some(v),
            _ => None,
        }
    }
}

/// The mod keys the player trusts: the directory's verified mods and the ones accepted before.
pub fn trusted_keys(data: &MenuData) -> Vec<String> {
    let mut t: Vec<String> = data
        .community
        .view()
        .and_then(|v| v.doc.as_ref())
        .map(community::verified_keys)
        .unwrap_or_default();
    t.extend(data.settings.mods_accepted.iter().cloned());
    t
}

/// The preferences command for the link (trusted keys, enabled local mods).
pub fn prefs_cmd(data: &MenuData) -> LinkCmd {
    LinkCmd::ModPrefs {
        trusted: trusted_keys(data),
        local_enabled: data.settings.mods_local_enabled.clone(),
    }
}

/// How a server mod is marked for the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Verified,
    Accepted,
    Unverified,
}

impl Mark {
    pub fn text(self) -> &'static str {
        match self {
            Mark::Verified => T_VERIFIED,
            Mark::Accepted => T_ACCEPTED,
            Mark::Unverified => T_UNVERIFIED,
        }
    }
    pub fn colour(self) -> u32 {
        match self {
            Mark::Verified => GREEN,
            Mark::Accepted => GOLD,
            Mark::Unverified => ORANGE,
        }
    }
}

/// The mark of mod key `key`.
pub fn mark_of(data: &MenuData, key: &str) -> Mark {
    let verified = data
        .community
        .view()
        .and_then(|v| v.doc.as_ref())
        .is_some_and(|d| community::known(d, key).is_some());
    if verified || data.curated_keys.iter().any(|k| k == key) {
        Mark::Verified
    } else if data.settings.mods_accepted.iter().any(|k| k == key) {
        Mark::Accepted
    } else {
        Mark::Unverified
    }
}

/// The mods of `set` the player has not trusted yet (the consent prompt lists them).
pub fn untrusted(set: &ServerModSet, data: &MenuData) -> Vec<ServerMod> {
    let t = trusted_keys(data);
    set.mods
        .iter()
        .filter(|m| !t.contains(&m.key()))
        .cloned()
        .collect()
}

/// The capability list in plain words.
pub fn capability_words(caps: &[String]) -> String {
    if caps.is_empty() {
        return "declares nothing it may change".into();
    }
    caps.iter()
        .map(|c| mods::capability_text(c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `s` cut to `w` pixels with "..." when longer.
pub(crate) fn fit(cat: &mut Catalog, s: &str, w: i32) -> String {
    if measure(cat, s) <= w {
        return s.to_string();
    }
    let mut t: String = s.to_string();
    while !t.is_empty() && measure(cat, &format!("{t}...")) > w {
        t.pop();
    }
    format!("{t}...")
}

pub(crate) fn text(s: &str, at: [i32; 2], colour: u32) -> DrawItem {
    place_text(s, at, [0, 0], TL, FONT, FONT_SIZE, colour)
}

pub(crate) fn wrap(cat: &mut Catalog, s: &str, w: i32) -> Vec<String> {
    super::roomtext::wrap_lines(s, w as f32, |x| {
        cat.measure_text(FONT, FONT_SIZE, x)
            .map_or(x.chars().count() as f32 * 7.0, |m| m.0)
    })
}

/// The window art: the ROM's generic window frame and the message boxes' button pictures.
#[derive(Debug, Clone, Default)]
pub struct WindowArt {
    pub frame: Option<String>,
}

impl WindowArt {
    pub fn from_rom(rom: Option<&Path>) -> WindowArt {
        WindowArt {
            frame: NameArt::from_rom(rom).window,
        }
    }
}

/// The size of the frame art inside its texture: the 2009 window's own rectangle of the layout section (the texture
/// is padded past the art to a power of two), at most the texture.
// EVIDENCE: cshell+0x2437d6 (the name window drawn from this texture at the layout's window rectangle)
pub fn frame_art_size(cat: &mut Catalog, tex: &str) -> Option<(u32, u32)> {
    let (tw, th) = cat.tex_size(tex)?;
    let [w, h] = super::usebox::NameLayout::read(cat).window_size;
    Some((
        (w.max(1) as u32).min(tw),
        (h.max(1) as u32).min(th),
    ))
}

/// The window frame over `rect`: the frame art in nine parts (its visible art, CLI-1623 layout fix), a dark fill
/// without it.
fn frame(cat: &mut Catalog, art: &WindowArt, rect: [i32; 4], out: &mut Vec<DrawItem>) {
    let drawn = match art.frame.as_deref() {
        Some(t) => match frame_art_size(cat, t) {
            Some(size) => super::nine::draw_art(t, size, rect, [255; 4], out),
            None => 0,
        },
        None => 0,
    };
    if drawn == 0 {
        out.push(fill([rect[0], rect[1]], [rect[2], rect[3]], [24, 24, 24, 240]));
    }
}

/// The title bar of the frame art, measured from the art (the test `the_frame_bar_constants_are_the_arts` checks them
/// against the ROM's texture): under the 4 px top border, 20 px high, then the 1 px divider. It lies in the art's top
/// quarter, which the nine parts draw at the art's own size, so these hold for a frame of any size.
pub const FRAME_BAR_TOP: i32 = 4;
pub const FRAME_BAR_H: i32 = 20;
/// From the frame's top to under the divider: a window's content starts below this.
pub const FRAME_HEAD_H: i32 = FRAME_BAR_TOP + FRAME_BAR_H + 1;

/// The y of the middle of the title bar of a frame whose top is `top`.
pub const fn frame_title_y(top: i32) -> i32 {
    top + FRAME_BAR_TOP + FRAME_BAR_H / 2
}

/// A line in the frame's title bar, centred on the bar's height, starting at `x` (`right`: ending at `x`).
fn title_text(s: &str, x: i32, top: i32, right: bool, colour: u32) -> DrawItem {
    let first = if right { TR.first } else { TL.first };
    let align = Align { first, second: ALIGN_SECOND_CENTRE };
    place_text(s, [x, frame_title_y(top)], [0, 0], align, FONT, FONT_SIZE, colour)
}

/// One button of a window.
#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    pub cmd: i32,
    pub at: [i32; 2],
    pub size: [i32; 2],
    pub label: String,
    pub on: bool,
    pub kind: ButtonKind,
}

/// How a window's button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// The message boxes' button picture.
    Art,
    /// A tab of the Mods window (true: the open one).
    Tab(bool),
    /// The On / Off switch of a row.
    Switch(bool),
}

/// The pointer state of a window's buttons.
#[derive(Debug, Clone, Default)]
pub struct Buttons {
    pub list: Vec<Button>,
    hover: Option<i32>,
    down: Option<i32>,
}

impl Buttons {
    pub(crate) fn at(&self, p: [f32; 2]) -> Option<i32> {
        self.list
            .iter()
            .find(|b| b.on && inside(b.at, b.size, p))
            .map(|b| b.cmd)
    }
    /// Pointer moved: true when the hovered button changed to another one.
    pub(crate) fn moved(&mut self, p: [f32; 2]) -> bool {
        let h = self.at(p);
        let changed = h.is_some() && h != self.hover;
        self.hover = h;
        changed
    }
    pub(crate) fn press(&mut self, p: [f32; 2]) -> bool {
        self.down = self.at(p);
        self.down.is_some()
    }
    /// Release: the command clicked (release on the pressed button).
    pub(crate) fn release(&mut self, p: [f32; 2]) -> Option<i32> {
        let d = self.down.take();
        d.filter(|c| self.at(p) == Some(*c))
    }
    pub(crate) fn draw(&self, cat: &mut Catalog, out: &mut Vec<DrawItem>) {
        for b in &self.list {
            let rect = [b.at[0], b.at[1], b.size[0], b.size[1]];
            let hover = self.hover == Some(b.cmd);
            let mut colour = if b.on { super::popup::CAPTION_COLOUR } else { CAPTION_DISABLED };
            match b.kind {
                ButtonKind::Art => {
                    let look = match (b.on, hover, self.down == Some(b.cmd)) {
                        (true, true, true) => 2,
                        (true, true, false) => 1,
                        _ => 0,
                    };
                    // the art fills the whole rectangle: the quarters are taken of the art, not of the padded
                    // texture, so the caption below is centred on what the player sees
                    let tint = if b.on { [255; 4] } else { DISABLED_TINT };
                    match cat.paths.popup_buttons[look].clone() {
                        Some(t) if cat.tex_size(&t).is_some() => {
                            super::nine::draw_art(&t, BUTTON_ART, rect, tint, out);
                        }
                        _ => out.push(fill(b.at, b.size, [60, 60, 60, 255])),
                    }
                }
                ButtonKind::Tab(active) => {
                    let back = match (active, hover) {
                        (true, _) => [70, 60, 30, 230],
                        (false, true) => [50, 50, 50, 220],
                        (false, false) => [0, 0, 0, 170],
                    };
                    out.push(fill(b.at, b.size, back));
                    if active {
                        out.push(fill([b.at[0], b.at[1] + b.size[1] - 2], [b.size[0], 2], [0xfe, 0xc0, 0x40, 255]));
                        colour = TITLE_COLOUR;
                    }
                }
                ButtonKind::Switch(on) => {
                    let track = if on { [34, 110, 44, 255] } else { [58, 58, 58, 255] };
                    out.push(fill(b.at, b.size, track));
                    // the knob at the side the switch stands on; the word stays centred beside it
                    let kx = if on { b.at[0] + b.size[0] - SWITCH_KNOB - 2 } else { b.at[0] + 2 };
                    out.push(fill([kx, b.at[1] + 2], [SWITCH_KNOB, b.size[1] - 4], [225, 225, 225, 255]));
                }
            }
            let label = fit_caption(cat, &b.label, b.size[0] - 4);
            out.push(place_text(
                &label,
                [b.at[0] + b.size[0] / 2, b.at[1] + b.size[1] / 2],
                [0, 0],
                MID,
                super::popup::CAPTION_FONT,
                super::popup::CAPTION_SIZE,
                colour,
            ));
        }
    }
}

/// `s` cut with "..." to `w` pixels of the caption font.
fn fit_caption(cat: &mut Catalog, s: &str, w: i32) -> String {
    let mut m = |x: &str| {
        cat.measure_text(super::popup::CAPTION_FONT, super::popup::CAPTION_SIZE, x)
            .map_or(x.chars().count() as f32 * 7.0, |v| v.0)
    };
    if m(s) <= w as f32 {
        return s.to_string();
    }
    let mut t = s.to_string();
    while !t.is_empty() && m(&format!("{t}...")) > w as f32 {
        t.pop();
    }
    format!("{t}...")
}

/// The rectangles of a window's buttons.
pub fn button_rects(b: &Buttons) -> Vec<[i32; 4]> {
    b.list
        .iter()
        .map(|b| [b.at[0], b.at[1], b.size[0], b.size[1]])
        .collect()
}

/// One row of the browser.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserRow {
    pub name: String,
    /// Data only: never drawn (CLI-1623).
    pub address: String,
    /// The region in words ("" = none).
    pub region: String,
    pub addr: Option<SocketAddr>,
    pub modded: bool,
    pub players: Option<(u32, u32)>,
    /// (mod name, mark) in the server's order.
    pub mods: Vec<(String, Mark)>,
    pub description: String,
    /// The pinned set (`None`: the configured lobby, no pin).
    pub expect: Option<ServerModSet>,
    pub fingerprint: Option<String>,
}

/// The mods of directory server `s` by known-mods name (else id) with their marks, in the server's order.
pub fn server_marks(
    data: &MenuData,
    doc: &dc_config::directory::Community,
    s: &dc_config::directory::ServerEntry,
) -> Vec<(String, Mark)> {
    badges(s, doc)
        .into_iter()
        .map(|(m, b)| {
            let key = mods::mod_key(&m.id, &m.version, &m.sha256);
            let name = community::known(doc, &key)
                .map(|k| k.name.clone())
                .unwrap_or_else(|| m.id.clone());
            let mark = match b {
                Badge::Verified => Mark::Verified,
                Badge::Unverified if data.settings.mods_accepted.contains(&key) => Mark::Accepted,
                Badge::Unverified => Mark::Unverified,
            };
            (name, mark)
        })
        .collect()
}

/// The browser's rows: the configured lobby first (when the list does not carry it), then the directory's servers.
pub fn browser_rows(data: &MenuData) -> Vec<BrowserRow> {
    let mut rows = Vec::new();
    let view = data.community.view();
    let doc = view.and_then(|v| v.doc.as_ref());
    if let Some(doc) = doc {
        for (i, s) in doc.servers.iter().enumerate() {
            let modded = s.policy == "modded";
            let mods = server_marks(data, doc, s);
            rows.push(BrowserRow {
                name: s.name.clone(),
                address: s.address.clone(),
                region: super::moddedrows::region_words(&s.region),
                addr: view.and_then(|v| v.addrs.get(i).copied().flatten()),
                modded,
                players: s.players.as_ref().map(|p| (p.current, p.max)),
                mods,
                description: s.description.clone(),
                expect: Some(community::expected_set(s)),
                fingerprint: s.fingerprint.clone(),
            });
        }
    }
    if let Some(a) = data.lobby {
        if !rows.iter().any(|r| r.addr == Some(a)) {
            rows.insert(
                0,
                BrowserRow {
                    name: T_CONFIGURED.into(),
                    address: a.to_string(),
                    region: String::new(),
                    addr: Some(a),
                    modded: false,
                    players: None,
                    mods: Vec::new(),
                    description: "The lobby this client was started with (--server). Its mods show when you \
                                  join it."
                        .into(),
                    expect: None,
                    fingerprint: None,
                },
            );
        }
    }
    rows
}

/// The selected row's title under the list: the name and the region, never the address (CLI-1623).
fn row_title(r: &BrowserRow) -> String {
    if r.region.is_empty() {
        r.name.clone()
    } else {
        format!("{}  -  {}", r.name, r.region)
    }
}

/// The community server browser (CLI-1616).
pub struct Community {
    sheet: Sheet,
    art: WindowArt,
    pub selected: usize,
    top: usize,
    buttons: Buttons,
    last_press: Option<(usize, Instant)>,
}

impl Community {
    pub fn new(cat: &Catalog, _data: &MenuData) -> Community {
        let [x, y] = WINDOW_AT;
        let [w, h] = WINDOW_SIZE;
        let by = y + h - 45;
        let b = |cmd, i: i32, label: &str| Button {
            cmd,
            at: [x + w - 20 - (3 - i) * (BUTTON[0] + 10) + 10, by],
            size: BUTTON,
            label: label.into(),
            on: true,
            kind: ButtonKind::Art,
        };
        Community {
            sheet: Sheet::default(),
            art: WindowArt::from_rom(cat.rom.as_deref()),
            selected: 0,
            top: 0,
            buttons: Buttons {
                list: vec![b(CMD_JOIN, 0, T_JOIN), b(CMD_MODS, 1, T_MODS), b(CMD_CLOSE, 2, T_CLOSE)],
                ..Buttons::default()
            },
            last_press: None,
        }
    }

    /// The window's frame and its buttons (the layout test checks the buttons lie inside the frame).
    pub fn layout_rects(&self) -> ([i32; 4], Vec<[i32; 4]>) {
        (
            [WINDOW_AT[0], WINDOW_AT[1], WINDOW_SIZE[0], WINDOW_SIZE[1]],
            button_rects(&self.buttons),
        )
    }

    fn row_rect(&self, k: usize) -> [i32; 4] {
        let [x, y] = WINDOW_AT;
        [
            x + 16,
            y + LIST_TOP + k as i32 * ROW_H,
            WINDOW_SIZE[0] - 32,
            ROW_H - 4,
        ]
    }

    fn select(&mut self, i: usize, n: usize) {
        if n == 0 {
            return;
        }
        self.selected = i.min(n - 1);
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + LIST_ROWS {
            self.top = self.selected + 1 - LIST_ROWS;
        }
    }

    /// Joins the selected row: the lobby address, the pin and the expected set for the login; back to the login.
    fn join(&mut self, ctx: &mut Ctx) {
        let rows = browser_rows(ctx.data);
        let Some(r) = rows.get(self.selected) else {
            return;
        };
        let Some(addr) = r.addr else {
            ctx.popups.push(Popup::ok(
                super::moddedrows::T_NO_ADDRESS.into(),
                BoxTag::None,
            ));
            return;
        };
        ctx.data.auth = Some(addr);
        ctx.data.picked_server = Some(r.name.clone());
        if let Some((host, _)) = r.address.rsplit_once(':') {
            dc_botclient::tls::note_host(addr.ip(), host.trim_matches(['[', ']']));
        }
        // the entry's certificate pin, else the run's own (`--server-fingerprint`, or none)
        let pin = r
            .fingerprint
            .clone()
            .or_else(|| ctx.data.lobby_pin.clone());
        match pin {
            Some(p) => {
                if let Err(e) = dc_botclient::tls::set_pin(&p) {
                    eprintln!("dc-client community: {e}");
                }
            }
            None => dc_botclient::tls::clear_pin(),
        }
        ctx.send(LinkCmd::ExpectModSet(r.expect.clone()));
        *ctx.go = Some(Go::Back);
    }

    fn act(&mut self, cmd: i32, ctx: &mut Ctx) {
        let n = browser_rows(ctx.data).len();
        match cmd {
            CMD_JOIN => self.join(ctx),
            CMD_MODS => {
                // CLI-1617: the Mods screen shows the highlighted row's mods first
                ctx.data.mods_target = browser_rows(ctx.data).get(self.selected).map(|r| ModsTarget {
                    name: r.name.clone(),
                    region: r.region.clone(),
                    modded: r.modded,
                    addr: r.addr,
                    set: r.expect.clone(),
                });
                *ctx.go = Some(Go::Mods);
            }
            CMD_CLOSE => *ctx.go = Some(Go::Back),
            c if (CMD_ROW_BASE..CMD_ROW_BASE + 1000).contains(&c) => {
                self.select((c - CMD_ROW_BASE) as usize, n)
            }
            _ => {}
        }
    }
}

impl Screen for Community {
    fn name(&self) -> &'static str {
        COMMUNITY_NAME
    }
    fn layout_rects(&self) -> Option<([i32; 4], Vec<[i32; 4]>)> {
        Some(Community::layout_rects(self))
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }
    fn shows_under(&self) -> bool {
        true
    }
    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        let n = browser_rows(ctx.data).len();
        match k {
            MenuKey::Escape => *ctx.go = Some(Go::Back),
            MenuKey::Enter => self.join(ctx),
            MenuKey::Up => self.select(self.selected.saturating_sub(1), n),
            MenuKey::Down => self.select(self.selected + 1, n),
            _ => {}
        }
    }
    fn click(&mut self, cmd: i32, _part: usize, ctx: &mut Ctx) {
        self.act(cmd, ctx);
    }
    fn wheel(&mut self, _p: [f32; 2], notches: i32, ctx: &mut Ctx) -> bool {
        let n = browser_rows(ctx.data).len();
        let max = n.saturating_sub(LIST_ROWS);
        self.top = (self.top as i32 - notches).clamp(0, max as i32) as usize;
        true
    }
    fn modal_input(&mut self, ev: &MenuInput, ctx: &mut Ctx) -> bool {
        match *ev {
            MenuInput::Move(p) => {
                if self.buttons.moved(p) {
                    ctx.sound(super::frontfx::SOUND_SELECT_CHANGE);
                }
            }
            MenuInput::Press(p) => {
                if self.buttons.press(p) {
                    return true;
                }
                let n = browser_rows(ctx.data).len();
                for k in 0..LIST_ROWS {
                    let i = self.top + k;
                    let r = self.row_rect(k);
                    if i < n && inside([r[0], r[1]], [r[2], r[3]], p) {
                        let double = self.last_press.is_some_and(|(j, t)| {
                            j == i && t.elapsed().as_millis() <= DOUBLE_CLICK_MS
                        });
                        self.select(i, n);
                        ctx.sound(super::frontfx::SOUND_SELECT);
                        self.last_press = Some((i, Instant::now()));
                        if double {
                            self.join(ctx);
                        }
                    }
                }
            }
            MenuInput::Release(p) => {
                if let Some(c) = self.buttons.release(p) {
                    ctx.sound(super::frontfx::SOUND_SELECT);
                    self.act(c, ctx);
                }
            }
            _ => return false,
        }
        true
    }
    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, _secs: f32, out: &mut Vec<DrawItem>) {
        let [x, y] = WINDOW_AT;
        let [w, h] = WINDOW_SIZE;
        frame(cat, &self.art, [x, y, w, h], out);
        out.push(title_text(T_SERVERS, x + 20, y, false, TITLE_COLOUR));
        let status = match &data.community {
            CommunityState::Off => T_LIST_OFF.to_string(),
            CommunityState::Loading => T_LIST_LOADING.to_string(),
            CommunityState::Ready(v) => match (&v.doc, &v.error, v.cached) {
                (Some(d), None, true) => format!("Community list of {} (cached)", d.updated),
                (Some(d), None, false) => format!("Community list of {}", d.updated),
                (Some(d), Some(e), _) => {
                    format!("Community list of {} (cached; the network failed: {e})", d.updated)
                }
                (None, Some(e), _) => format!("The community list cannot be read: {e}"),
                (None, None, _) => T_LIST_OFF.to_string(),
            },
        };
        let status = fit(cat, &status, w - 40);
        out.push(text(&status, [x + 20, y + 36], MESSAGE_COLOUR));
        let rows = browser_rows(data);
        if rows.is_empty() {
            out.push(text(
                "No server to show: start the client with --server HOST:PORT or turn the community list on.",
                [x + 20, y + LIST_TOP + 8],
                TEXT_COLOUR,
            ));
        }
        for k in 0..LIST_ROWS {
            let i = self.top + k;
            let Some(r) = rows.get(i) else { break };
            let rect = self.row_rect(k);
            let sel = i == self.selected;
            out.push(fill(
                [rect[0], rect[1]],
                [rect[2], rect[3]],
                if sel { [70, 60, 30, 220] } else { [0, 0, 0, 150] },
            ));
            let (bx, by) = (rect[0] + 8, rect[1] + 4);
            let badge = if r.modded { T_MODDED } else { T_VANILLA };
            let bc = if r.modded { GOLD } else { BLUE };
            out.push(text(badge, [bx, by], bc));
            let nx = bx + 80;
            let name = fit(cat, &r.name, rect[2] - 260);
            out.push(text(
                &name,
                [nx, by],
                if sel { 0xffff_ffff } else { TEXT_COLOUR },
            ));
            let right = match r.players {
                Some((c, m)) => format!("{c} / {m} players"),
                // CLI-1623: no address on screen: the region instead
                None => r.region.clone(),
            };
            out.push(place_text(
                &right,
                [rect[0] + rect[2] - 8, by],
                [0, 0],
                TR,
                FONT,
                FONT_SIZE,
                MESSAGE_COLOUR,
            ));
            // the second line: the mods with their marks, or the description
            let ly = by + LINE;
            if r.modded {
                let mut mx = nx;
                let end = rect[0] + rect[2] - 8;
                for (j, (m, mark)) in r.mods.iter().enumerate() {
                    let label = format!("{m} [{}]", mark.text());
                    let lw = measure(cat, &label);
                    if mx + lw > end {
                        out.push(text(
                            &format!("+{} more", r.mods.len() - j),
                            [mx, ly],
                            MESSAGE_COLOUR,
                        ));
                        break;
                    }
                    let mw = measure(cat, m);
                    out.push(text(m, [mx, ly], TEXT_COLOUR));
                    out.push(text(
                        &format!(" [{}]", mark.text()),
                        [mx + mw, ly],
                        mark.colour(),
                    ));
                    mx += lw + 14;
                }
            } else {
                let d = fit(cat, &r.description, rect[2] - 100);
                out.push(text(&d, [nx, ly], MESSAGE_COLOUR));
            }
        }
        if let Some(r) = rows.get(self.selected) {
            let dy = y + LIST_TOP + LIST_ROWS as i32 * ROW_H + 6;
            out.push(text(
                &fit(cat, &row_title(r), w - 40),
                [x + 20, dy],
                TITLE_COLOUR,
            ));
            for (k, l) in wrap(cat, &r.description, w - 40).iter().take(2).enumerate() {
                out.push(text(l, [x + 20, dy + (k as i32 + 1) * LINE], MESSAGE_COLOUR));
            }
        }
        self.buttons.draw(cat, out);
    }
}

/// The server whose mods the Mods screen shows first (the row highlighted where it was opened).
#[derive(Debug, Clone, PartialEq)]
pub struct ModsTarget {
    pub name: String,
    /// The region in words ("" = none).
    pub region: String,
    pub modded: bool,
    /// Data only: never drawn (CLI-1623).
    pub addr: Option<SocketAddr>,
    /// The set the community document gives (`None`: not known before a join).
    pub set: Option<ServerModSet>,
}

pub use crate::knownsets::KnownSet;

/// The community document's entry of `addr` (by resolved address).
fn doc_entry(data: &MenuData, addr: Option<SocketAddr>) -> Option<&dc_config::directory::ServerEntry> {
    let addr = addr?;
    let view = data.community.view()?;
    let doc = view.doc.as_ref()?;
    let i = view.addrs.iter().position(|a| *a == Some(addr))?;
    doc.servers.get(i)
}

/// The name of the lobby logged into (or to be logged into): the picked name, the community document's, the
/// current entry's; never its address (CLI-1623).
pub fn lobby_title(data: &MenuData) -> String {
    data.picked_server
        .clone()
        .filter(|n| !n.is_empty())
        .or_else(|| doc_entry(data, data.auth).map(|s| s.name.clone()))
        .or_else(|| data.server.as_ref().map(|e| e.name.clone()))
        .unwrap_or_else(|| "the server you log in to".into())
}

/// The current lobby as a Mods screen target.
pub fn current_target(data: &MenuData) -> ModsTarget {
    let doc = doc_entry(data, data.auth);
    ModsTarget {
        name: lobby_title(data),
        region: doc
            .map(|s| super::moddedrows::region_words(&s.region))
            .unwrap_or_default(),
        modded: doc.is_some_and(|s| s.policy == "modded"),
        addr: data.auth,
        set: doc.map(community::expected_set),
    }
}

/// Notes the set of the lobby just entered (its address and name), replacing an older note of that address.
/// CLI-1628: the set offers wallhacks: modded policy and a declared `world.full_visibility` mod.
pub fn offers_wallhacks(set: &ServerModSet) -> bool {
    set.policy != crate::mods::Policy::Vanilla
        && set
            .mods
            .iter()
            .any(|m| m.capabilities.iter().any(|c| c == "world.full_visibility"))
}

pub fn remember_set(data: &mut MenuData, set: Option<ServerModSet>) {
    let (Some(addr), Some(set)) = (data.auth, set) else {
        return;
    };
    let name = lobby_title(data);
    note_known(data, addr, name, set, std::time::SystemTime::now());
}

/// Notes a set read now, replacing the older note of that address, and saves the list.
fn note_known(data: &mut MenuData, addr: SocketAddr, name: String, set: ServerModSet, seen: std::time::SystemTime) {
    data.known_sets.retain(|k| k.addr != addr);
    data.known_sets.push(KnownSet { addr, name, set, seen });
    if let Some(dir) = data.mods_dir.as_deref() {
        crate::knownsets::save(dir, &data.known_sets);
    }
}

/// CLI-1632: a modded row's public set as read in the background. A late answer of the lobby the player has just
/// entered is dropped: the join's own read is newer.
pub fn note_row_set(data: &mut MenuData, addr: SocketAddr, name: String, set: ServerModSet, now: std::time::SystemTime) {
    if !(data.lobby_up && data.auth == Some(addr)) {
        note_known(data, addr, name, set, now);
    }
}

/// CLI-1632: the modded rows of the server list whose public set can be asked for (auth address, name, pin).
pub fn row_set_requests(data: &MenuData) -> Vec<(SocketAddr, String, Option<String>)> {
    super::moddedrows::modded_rows(data)
        .into_iter()
        .filter(|r| r.modded && r.addr != data.auth)
        .filter_map(|r| Some((r.addr?, r.name, r.fingerprint)))
        .collect()
}

/// CLI-1632: whether a server may declare a package without the client knowing: the login's lobby has not said yet
/// which others it lists, the community document is still being read, or a modded row has no set (no read answered
/// yet, none kept from earlier runs, none in the document). A wrong "Unused" deletes what a server needs, so while
/// this holds nothing is called unused, offered for removal or pruned.
// DC-DEFAULT: CLI-1632 (proposed, amended 2026-11-01)
pub fn sets_uncertain(data: &MenuData, now: std::time::SystemTime) -> bool {
    if !data.other_lobbies_read || matches!(data.community, CommunityState::Loading) {
        return true;
    }
    let known = |a: Option<SocketAddr>| data.known_sets.iter().any(|k| Some(k.addr) == a && k.live(now));
    let view = data.community.view();
    let doc = view.and_then(|v| v.doc.as_ref());
    let doc_addr = |i: usize| view.and_then(|v| v.addrs.get(i).copied().flatten());
    let doc_declares = |o: &super::link::OtherLobby| {
        doc.is_some_and(|d| {
            d.servers.iter().enumerate().any(|(i, s)| {
                !s.mods.is_empty()
                    && ((o.addr.is_some() && doc_addr(i) == o.addr)
                        || s.address.eq_ignore_ascii_case(&o.listed.lobby))
            })
        })
    };
    let listed = data
        .other_lobbies
        .iter()
        .filter(|o| o.listed.policy != "vanilla")
        .any(|o| !known(o.addr) && !doc_declares(o));
    let in_doc = doc.is_some_and(|d| {
        d.servers.iter().enumerate().any(|(i, s)| {
            let a = doc_addr(i);
            s.policy == "modded" && s.mods.is_empty() && a.is_some() && a != data.auth && !known(a)
        })
    });
    listed || in_doc
}

/// The set of `t`: the last join's when `t` is the lobby of that join, else the one the document gives, else the
/// one read at an earlier join of this run.
fn target_set(data: &MenuData, report: &JoinReport, t: &ModsTarget) -> Option<ServerModSet> {
    if t.addr.is_some() && t.addr == data.auth {
        if let Some(s) = report.set.as_ref() {
            return Some(s.clone());
        }
    }
    t.set.clone().or_else(|| {
        data.known_sets
            .iter()
            .find(|k| Some(k.addr) == t.addr)
            .map(|k| k.set.clone())
    })
}

/// Every server set the client knows of, by server name: the current lobby's, the ones read at this run's joins and
/// the community document's.
fn server_sets(data: &MenuData, report: &JoinReport, now: std::time::SystemTime) -> Vec<(String, ServerModSet)> {
    let mut out = Vec::new();
    if let Some(s) = report.set.as_ref() {
        out.push((lobby_title(data), s.clone()));
    }
    out.extend(data.known_sets.iter().filter(|k| k.live(now)).map(|k| (k.name.clone(), k.set.clone())));
    if let Some(doc) = data.community.view().and_then(|v| v.doc.as_ref()) {
        out.extend(doc.servers.iter().map(|s| (s.name.clone(), community::expected_set(s))));
    }
    out
}

/// CLI-1632: the sets of every known server other than the lobby of the login (its own entries may be stale after
/// an update): this run's other joins and the community document's other servers.
fn other_sets(data: &MenuData, now: std::time::SystemTime) -> Vec<ServerModSet> {
    let mut out: Vec<ServerModSet> = data
        .known_sets
        .iter()
        .filter(|k| Some(k.addr) != data.auth && k.live(now))
        .map(|k| k.set.clone())
        .collect();
    if let Some(view) = data.community.view() {
        if let Some(doc) = view.doc.as_ref() {
            for (i, s) in doc.servers.iter().enumerate() {
                let addr = view.addrs.get(i).copied().flatten();
                if addr.is_some() && addr == data.auth {
                    continue;
                }
                out.push(community::expected_set(s));
            }
        }
    }
    out
}

/// CLI-1632: the hashes the loaded sandbox uses (never pruned).
fn in_use_hashes(report: &JoinReport) -> Vec<String> {
    let mut keep: Vec<String> = report
        .recipe
        .as_ref()
        .map(|r| {
            r.mods
                .iter()
                .filter_map(|m| m.dir.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    if report.state == ModState::Ready {
        if let Some(s) = report.set.as_ref() {
            keep.extend(s.mods.iter().map(|m| m.sha256.clone()));
        }
    }
    keep
}

/// CLI-1632: prunes the downloaded packages. `Auto` (after a successful join, `report` being that join's): the
/// packages the joined set supersedes (same id, other sha256) unless another known server still declares them, and
/// packages of ids no known server declares that went unused for a week (`now` is the clock). `CleanUp` (the Mods
/// screen's button): every package no known server declares. Each removal is logged, and the consent keys of the
/// removed hashes are dropped from the profile's `mods.accepted` (the caller saves the profile and sends the new
/// preferences when the list changed). Local mods are never touched.
// DC-DEFAULT: CLI-1632 (proposed, project decision) superseded and unused packages leave the cache
pub fn prune_cache(
    data: &mut MenuData,
    report: &JoinReport,
    mode: mods::PruneMode,
    now: std::time::SystemTime,
) -> Vec<mods::Pruned> {
    let Some(paths) = data.mods_dir.as_deref().map(ModPaths::new) else {
        return Vec::new();
    };
    if sets_uncertain(data, now) {
        return Vec::new();
    }
    let joined = report
        .set
        .as_ref()
        .filter(|s| report.state == ModState::Ready && s.policy != Policy::Vanilla);
    let others = match mode {
        mods::PruneMode::Auto => other_sets(data, now),
        mods::PruneMode::CleanUp => server_sets(data, report, now).into_iter().map(|s| s.1).collect(),
    };
    let others: Vec<&ServerModSet> = others.iter().collect();
    let keep = in_use_hashes(report);
    let keep: Vec<&str> = keep.iter().map(String::as_str).collect();
    let removed = mods::prune_cache(&paths, joined, &others, &keep, mode, now);
    for p in &removed {
        eprintln!("{}", p.log_line());
    }
    data.settings
        .mods_accepted
        .retain(|k| !removed.iter().any(|p| k.ends_with(&format!(":{}", p.sha256))));
    removed
}

/// A package kind in plain words.
pub fn kind_words(kind: &str) -> String {
    match kind {
        "shared" | "server_rules" => "server mod",
        "client_visual" => "cosmetic",
        other => other,
    }
    .to_string()
}

/// One server mod of the Mods window.
#[derive(Debug, Clone, PartialEq)]
pub struct SetRow {
    pub name: String,
    pub version: String,
    /// In plain words ([`kind_words`]).
    pub kind: String,
    pub author: String,
    pub key: String,
    pub mark: Mark,
    pub caps: String,
    pub cap_list: Vec<String>,
    pub description: String,
    /// The first screenshot, when the package is unpacked in the cache.
    pub shot: Option<PathBuf>,
    /// MOD-27: the lobby operator's picture, when it is in the cache.
    pub preview: Option<PathBuf>,
    pub commands: Vec<dc_mod_lua::ModCommand>,
}

/// The rows of server set `set`: the mod-set entry, filled in from the unpacked package's manifest and the
/// known-mods row.
pub fn set_rows(data: &MenuData, set: &ServerModSet) -> Vec<SetRow> {
    let paths = data.mods_dir.as_deref().map(ModPaths::new);
    set.mods
        .iter()
        .map(|m| {
            let dir = paths.as_ref().map(|p| p.unpacked(&m.sha256));
            let man = dir
                .as_ref()
                .and_then(|d| dc_mod_lua::ModManifest::from_file(&d.join("manifest.json")).ok());
            let doc = data.community.view().and_then(|v| v.doc.as_ref());
            let known = doc.and_then(|d| community::known(d, &m.key()));
            let pick = |a: &Option<String>, b: Option<&str>, c: Option<&str>| {
                a.clone()
                    .or(b.map(str::to_string))
                    .or(c.map(str::to_string))
                    .filter(|s| !s.is_empty())
            };
            let caps = if m.capabilities.is_empty() {
                man.as_ref()
                    .map(|x| x.capabilities.clone())
                    .or(known.map(|k| k.capabilities.clone()))
                    .unwrap_or_default()
            } else {
                m.capabilities.clone()
            };
            let shots = if m.screenshots.is_empty() {
                man.as_ref().map(|x| x.screenshots.clone()).unwrap_or_default()
            } else {
                m.screenshots.clone()
            };
            SetRow {
                name: pick(
                    &m.name,
                    known.map(|k| k.name.as_str()),
                    man.as_ref().map(|x| x.name.as_str()),
                )
                .unwrap_or_else(|| m.id.clone()),
                version: m.version.clone(),
                kind: kind_words(
                    &m.kind
                        .clone()
                        .or(man.as_ref().map(|x| x.kind.as_str().to_string()))
                        .unwrap_or_else(|| "?".into()),
                ),
                author: pick(
                    &m.author,
                    known.map(|k| k.author.as_str()),
                    man.as_ref().map(|x| x.author.as_str()),
                )
                .unwrap_or_else(|| "unknown".into()),
                key: m.key(),
                mark: mark_of(data, &m.key()),
                caps: capability_words(&caps),
                cap_list: caps,
                description: pick(
                    &m.description,
                    known.map(|k| k.description.as_str()),
                    man.as_ref().map(|x| x.description.as_str()),
                )
                .unwrap_or_default(),
                preview: m
                    .preview
                    .as_deref()
                    .zip(paths.as_ref())
                    .map(|(sha, p)| p.catalog_shot(sha))
                    .filter(|f| f.is_file()),
                commands: if m.commands.is_empty() {
                    man.as_ref().map(|x| x.commands.clone()).unwrap_or_default()
                } else {
                    m.commands.clone()
                },
                shot: dir
                    .filter(|d| d.is_dir())
                    .zip(shots.first())
                    .map(|(d, s)| s.split('/').fold(d, |p, c| p.join(c))),
            }
        })
        .collect()
}

/// One package of the "Downloaded mods on this PC" section.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheRow {
    pub package: CachedMod,
    /// The known servers whose set names this package.
    pub used_by: Vec<String>,
    /// The loaded sandbox uses it (no Remove while it does).
    pub in_use: bool,
}

/// The rows of the by-hash cache with the servers that use each package.
pub fn cache_rows(data: &MenuData, report: &JoinReport) -> Vec<CacheRow> {
    let Some(paths) = data.mods_dir.as_deref().map(ModPaths::new) else {
        return Vec::new();
    };
    let sets = server_sets(data, report, std::time::SystemTime::now());
    list_cache(&paths)
        .into_iter()
        .map(|p| {
            let mut used_by: Vec<String> = Vec::new();
            for (name, set) in &sets {
                if set.mods.iter().any(|m| m.sha256 == p.sha256) && !used_by.contains(name) {
                    used_by.push(name.clone());
                }
            }
            let dir = paths.unpacked(&p.sha256);
            let in_use = report
                .recipe
                .as_ref()
                .is_some_and(|r| r.mods.iter().any(|m| m.dir == dir))
                || (report.state == ModState::Ready
                    && report
                        .set
                        .as_ref()
                        .is_some_and(|s| s.mods.iter().any(|m| m.sha256 == p.sha256)));
            CacheRow {
                package: p,
                used_by,
                in_use,
            }
        })
        .collect()
}

/// The two tabs of the Mods window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModsTab {
    /// The mods the player can get.
    Browse,
    /// What is on this PC.
    Mine,
}

/// What a row shows at its right edge (one state, never more).
#[derive(Debug, Clone, PartialEq)]
enum RowState {
    /// Browse: the size and the Get / Update / Wait button of catalogue entry `i`.
    Get { i: usize, size: String, label: &'static str, on: bool },
    /// Plain words at the right edge.
    Words(String, u32),
    /// My Mods: the On / Off switch of local mod `i`.
    Switch { i: usize, on: bool },
}

/// One action of the detail panel.
#[derive(Debug, Clone, PartialEq)]
struct Action {
    cmd: i32,
    label: String,
    on: bool,
}

/// One mod of a tab: everything from one id merged into a single row (a local folder, its downloaded packages, the
/// selected server's entry and the catalogue's entry).
#[derive(Debug, Clone, PartialEq)]
struct ModRow {
    id: String,
    name: String,
    description: String,
    version: String,
    author: String,
    kind: String,
    mark: Option<Mark>,
    caps: Vec<String>,
    commands: Vec<dc_mod_lua::ModCommand>,
    shot: Option<PathBuf>,
    state: RowState,
    /// Plain notes of the detail panel with their colours, shown only when they matter.
    notes: Vec<(String, u32)>,
    actions: Vec<Action>,
}

/// The indexes of one mod's sources in the screen's lists.
struct Sources {
    local: Option<usize>,
    packages: Vec<usize>,
    set: Option<usize>,
    get: Option<usize>,
}

/// The Mods window (CLI-1617, redesigned 2026-11-01): two tabs, a list with one row per mod and a detail panel.
pub struct ModsScreen {
    sheet: Sheet,
    art: WindowArt,
    pub report: JoinReport,
    pub target: Option<ModsTarget>,
    pub set: Vec<SetRow>,
    pub cache: Vec<CacheRow>,
    pub local: Vec<LocalMod>,
    /// CLI-1634: the optional mods as listed.
    pub get: Vec<crate::modcatalog::CatalogEntry>,
    /// MOD-24: the open settings page (it takes the window).
    pub page: Option<SettingsPage>,
    /// `None` until the first read chooses: My Mods when something is installed, else Browse.
    pub tab: Option<ModsTab>,
    /// The data folder, for the pictures of the optional mods.
    shots: Option<ModPaths>,
    rows: Vec<ModRow>,
    /// The context line and its colour.
    context: (String, u32),
    /// The line in the middle of an empty list.
    empty: String,
    /// The consent keys the Revoke trust button clears.
    forget: Vec<Vec<String>>,
    pub selected: Option<usize>,
    selected_id: Option<String>,
    top: usize,
    detail_top: usize,
    /// How far the detail text can scroll (known once it is wrapped, at the draw).
    detail_max: Cell<usize>,
    buttons: Buttons,
}

/// The Mods window's whole layout: everything below derives from these.
pub const MODS_RECT: [i32; 4] = [44, 34, 936, 700];
const MODS_PAD: i32 = 18;
const TAB_Y: i32 = MODS_RECT[1] + FRAME_HEAD_H + 15;
const TAB: [i32; 2] = [110, 26];
const BODY_Y: i32 = TAB_Y + TAB[1] + 8;
const LIST_W: i32 = 500;
const GUTTER: i32 = 14;
/// Rows on screen, a row's height and the distance of two rows.
pub const MODS_ROWS: usize = 9;
pub const MODS_ROW_H: i32 = 59;
pub const MODS_ROW_STEP: i32 = 63;
const BODY_H: i32 = MODS_ROWS as i32 * MODS_ROW_STEP - (MODS_ROW_STEP - MODS_ROW_H);
const SCROLL_W: i32 = 6;
/// The footer's button line.
pub const FOOT_Y: i32 = BODY_Y + BODY_H + 12;
/// The list and the detail panel.
pub const LIST_RECT: [i32; 4] = [MODS_RECT[0] + MODS_PAD, BODY_Y, LIST_W, BODY_H];
pub const DETAIL_RECT: [i32; 4] = [
    LIST_RECT[0] + LIST_W + GUTTER,
    BODY_Y,
    MODS_RECT[2] - 2 * MODS_PAD - LIST_W - GUTTER,
    BODY_H,
];
const ROW_PAD: i32 = 8;
const THUMB: [i32; 2] = [72, 43];
const ROW_TEXT_DX: i32 = ROW_PAD + THUMB[0] + 10;
const GET_BUTTON: [i32; 2] = [68, 25];
const SWITCH: [i32; 2] = [60, 24];
const STATE_MAX_W: i32 = 190;
const DETAIL_PAD: i32 = 12;
const SHOT_H: i32 = 196;
const SHOT_MIN_H: i32 = 90;
const ACTION: [i32; 2] = [82, 25];
const ACTION_WIDE: i32 = 96;
const ACTION_GAP: i32 = 6;
const FOOT_FOLDER_W: i32 = 110;
const FOOT_WIDE: i32 = 150;
const BODY_TEXT: u32 = 0xffc8_c8c8;
const ROW_BACK: [u8; 4] = [0, 0, 0, 150];
const ROW_PICKED: [u8; 4] = [70, 60, 30, 220];
const PANEL_BACK: [u8; 4] = [0, 0, 0, 130];

/// Whether a local mod can never load as a local mod (the switch would do nothing).
fn local_blocked(m: &LocalMod) -> bool {
    m.broken.is_some()
        || m.kind != "client_visual"
        || dc_mod_lua::CLIENT_VIEW_CAPABILITIES
            .iter()
            .any(|c| m.capabilities.iter().any(|x| x == *c))
}

/// The one note of local mod `m` worth showing. A mod that is merely off has none; a mod that cannot load says why in
/// full; an enabled one says plainly where it stays off.
fn local_note(m: &LocalMod, report: &JoinReport, enabled: bool, selected: Option<Policy>) -> Option<(String, u32)> {
    let refused = report
        .local_refused
        .iter()
        .any(|r| r.starts_with(&format!("Mod '{}' refused", m.id)));
    if local_blocked(m) || refused {
        return mods::local_reason(m, report).map(|r| (r, ORANGE));
    }
    if !enabled {
        return None;
    }
    let policy = selected.or(report.set.as_ref().map(|s| s.policy));
    Some(match policy {
        Some(Policy::Vanilla) => (T_OFF_VANILLA.to_string(), BLUE),
        Some(Policy::Modded { client_visual_allowed: false }) => (T_OFF_NOT_ALLOWED.to_string(), BLUE),
        _ => (T_NEXT_JOIN.to_string(), MESSAGE_COLOUR),
    })
}

/// A capability phrase as a line of "What it does".
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn plain_caps(caps: &[String]) -> Vec<String> {
    caps.iter().map(|c| sentence(&mods::capability_text(c))).collect()
}

fn package_id(c: &CacheRow) -> String {
    if c.package.id.is_empty() {
        format!("sha:{}", c.package.sha256)
    } else {
        c.package.id.clone()
    }
}

fn set_row_id(r: &SetRow) -> String {
    r.key.split('@').next().unwrap_or(&r.key).to_string()
}

impl ModsScreen {
    pub fn new(cat: &Catalog, data: &MenuData) -> ModsScreen {
        ModsScreen {
            sheet: Sheet::default(),
            art: WindowArt::from_rom(cat.rom.as_deref()),
            report: JoinReport::default(),
            target: data.mods_target.clone(),
            set: Vec::new(),
            cache: Vec::new(),
            local: Vec::new(),
            get: Vec::new(),
            page: None,
            tab: None,
            shots: data.mods_dir.as_deref().map(ModPaths::new),
            rows: Vec::new(),
            context: (String::new(), MESSAGE_COLOUR),
            empty: String::new(),
            forget: Vec::new(),
            selected: None,
            selected_id: None,
            top: 0,
            detail_top: 0,
            detail_max: Cell::new(0),
            buttons: Buttons::default(),
        }
    }

    /// Reads the last join's report, the cache and the local folder again, then lays the window out.
    pub fn refresh(&mut self, data: &MenuData, report: JoinReport) {
        self.report = report;
        let target = self.target.clone().unwrap_or_else(|| current_target(data));
        let set = target_set(data, &self.report, &target);
        self.set = set.as_ref().map(|s| set_rows(data, s)).unwrap_or_default();
        self.cache = cache_rows(data, &self.report);
        // the selected server's set counts as a known server even when no other list names it
        if let Some(s) = set.as_ref() {
            for c in &mut self.cache {
                if s.mods.iter().any(|m| m.sha256 == c.package.sha256) && !c.used_by.contains(&target.name) {
                    c.used_by.push(target.name.clone());
                }
            }
        }
        let paths = data.mods_dir.as_deref().map(ModPaths::new);
        self.local = paths.as_ref().map(mods::list_local).unwrap_or_default();
        self.get = match &data.catalog {
            crate::modcatalog::CatalogState::Ready(l) => l.clone(),
            _ => Vec::new(),
        };
        if self.tab.is_none() {
            let installed = !self.local.is_empty() || !self.cache.is_empty();
            self.tab = Some(if installed { ModsTab::Mine } else { ModsTab::Browse });
        }
        // the context line names the selected server, never its address (CLI-1623)
        let policy = set.as_ref().map(|s| s.policy);
        let modded = match policy {
            Some(Policy::Vanilla) => false,
            Some(Policy::Modded { .. }) => true,
            None => target.modded,
        };
        let who = if target.region.is_empty() {
            sentence(&target.name)
        } else {
            format!("{} ({})", target.name, target.region)
        };
        self.context = if modded {
            (format!("{who}  -  {T_CTX_MODDED}"), GOLD)
        } else if policy.is_some() {
            (format!("{who}  -  {T_CTX_VANILLA}"), BLUE)
        } else {
            (format!("{who}  -  {T_SET_AT_JOIN}"), MESSAGE_COLOUR)
        };
        self.relayout(data);
    }

    /// The rows of the current tab, the selection, the scroll positions and every button (no disk access).
    fn relayout(&mut self, data: &MenuData) {
        let tab = self.tab.unwrap_or(ModsTab::Browse);
        let target = self.target.clone().unwrap_or_else(|| current_target(data));
        self.rows = self.tab_rows(data, tab, &target);
        self.empty = match tab {
            ModsTab::Mine => T_NOTHING_INSTALLED.to_string(),
            ModsTab::Browse => {
                use crate::modcatalog::{CatalogState, T_NO_CATALOG, T_NO_SERVER, T_READING};
                match (&data.catalog_note, &data.catalog) {
                    (Some(n), _) => n.clone(),
                    (_, CatalogState::Unread) => T_NO_SERVER.to_string(),
                    (_, CatalogState::Loading) => T_READING.to_string(),
                    (_, CatalogState::Nothing(t)) => t.clone(),
                    (_, CatalogState::Ready(_)) => T_NO_CATALOG.to_string(),
                }
            }
        };
        let n = self.rows.len();
        self.selected = self
            .selected_id
            .as_ref()
            .and_then(|id| self.rows.iter().position(|r| r.id == *id))
            .or(self.selected.map(|i| i.min(n.saturating_sub(1))))
            .or(Some(0))
            .filter(|_| n > 0);
        self.selected_id = self.selected.map(|i| self.rows[i].id.clone());
        self.top = self.top.min(n.saturating_sub(MODS_ROWS));
        self.rebuild_buttons(data);
    }

    /// One row per mod id of tab `tab`.
    fn tab_rows(&mut self, data: &MenuData, tab: ModsTab, target: &ModsTarget) -> Vec<ModRow> {
        // the ids in order: on My Mods the player's folders and then the packages; on Browse the catalogue and then
        // the selected server's mods that are not on this PC yet
        let mut ids: Vec<String> = Vec::new();
        let mut add = |id: String| {
            if !ids.contains(&id) {
                ids.push(id);
            }
        };
        match tab {
            ModsTab::Mine => {
                self.local.iter().for_each(|m| add(m.id.clone()));
                self.cache.iter().for_each(|c| add(package_id(c)));
            }
            ModsTab::Browse => {
                self.get.iter().for_each(|e| add(e.id.clone()));
                self.set
                    .iter()
                    .filter(|r| {
                        let id = set_row_id(r);
                        !self.local.iter().any(|m| m.id == id) && !self.cache.iter().any(|c| package_id(c) == id)
                    })
                    .for_each(|r| add(set_row_id(r)));
            }
        }
        let mut forget = Vec::new();
        let rows = ids
            .into_iter()
            .map(|id| {
                // the package shown first: the loaded one, then one a server needs
                let mut packages: Vec<usize> =
                    (0..self.cache.len()).filter(|i| package_id(&self.cache[*i]) == id).collect();
                packages.sort_by_key(|i| {
                    let c = &self.cache[*i];
                    (!c.in_use, c.used_by.is_empty())
                });
                let src = Sources {
                    local: self.local.iter().position(|m| m.id == id),
                    packages,
                    set: self.set.iter().position(|r| set_row_id(r) == id),
                    get: self.get.iter().position(|e| e.id == id),
                };
                self.mod_row(data, tab, target, id, src, &mut forget)
            })
            .collect();
        self.forget = forget;
        rows
    }

    fn mod_row(
        &self,
        data: &MenuData,
        tab: ModsTab,
        target: &ModsTarget,
        id: String,
        src: Sources,
        forget: &mut Vec<Vec<String>>,
    ) -> ModRow {
        let Sources { local, packages, set, get } = src;
        let (m, s, e) = (
            local.map(|i| &self.local[i]),
            set.map(|i| &self.set[i]),
            get.map(|i| &self.get[i]),
        );
        let p = packages.first().map(|i| &self.cache[*i].package);
        let first = |v: [Option<&str>; 4]| v.into_iter().flatten().find(|x| !x.is_empty()).map(str::to_string);
        let name = first([
            m.map(|x| x.name.as_str()),
            p.map(|x| x.name.as_str()),
            e.map(|x| x.name.as_str()),
            s.map(|x| x.name.as_str()),
        ])
        .unwrap_or_else(|| id.clone());
        let description = first([
            m.map(|x| x.description.as_str()),
            p.map(|x| x.description.as_str()),
            e.map(|x| x.description.as_str()),
            s.map(|x| x.description.as_str()),
        ])
        .unwrap_or_default();
        let version = first([
            m.map(|x| x.version.as_str()),
            s.map(|x| x.version.as_str()),
            p.map(|x| x.version.as_str()),
            e.map(|x| x.version.as_str()),
        ])
        .unwrap_or_default();
        let author = first([
            m.map(|x| x.author.as_str()),
            s.map(|x| x.author.as_str()),
            p.map(|x| x.author.as_str()),
            e.map(|x| x.author.as_str()),
        ])
        .unwrap_or_default();
        let kind = match (m, s, p) {
            (Some(x), ..) => kind_words(&x.kind),
            (_, Some(x), _) => x.kind.clone(),
            (_, _, Some(x)) => kind_words(&x.kind),
            _ => kind_words("client_visual"),
        };
        let mut keys: Vec<String> = Vec::new();
        for k in s
            .map(|x| x.key.clone())
            .into_iter()
            .chain(packages.iter().filter_map(|i| self.cache[*i].package.key()))
        {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        let mark = keys.first().map(|_| {
            let marks: Vec<Mark> = keys.iter().map(|k| mark_of(data, k)).collect();
            [Mark::Verified, Mark::Accepted, Mark::Unverified]
                .into_iter()
                .find(|w| marks.contains(w))
                .unwrap_or(Mark::Unverified)
        });
        let caps = match (m, s, p) {
            (Some(x), ..) if !x.capabilities.is_empty() => plain_caps(&x.capabilities),
            (_, Some(x), _) if !x.cap_list.is_empty() => plain_caps(&x.cap_list),
            (_, _, Some(x)) if !x.capabilities.is_empty() => plain_caps(&x.capabilities),
            _ if e.is_some() => vec![T_LOOKS_ONLY.to_string()],
            _ => Vec::new(),
        };
        // DC-DEFAULT: MOD-27 (proposed) the lobby operator's picture goes first
        let preview = s.and_then(|x| x.preview.clone()).or(e.and_then(|x| {
            let sha = x.preview.as_deref()?;
            self.shots.as_ref().map(|d| d.catalog_shot(sha)).filter(|f| f.is_file())
        }));
        let commands = [
            m.map(|x| &x.commands),
            p.map(|x| &x.commands),
            e.map(|x| &x.commands),
            s.map(|x| &x.commands),
        ]
        .into_iter()
        .flatten()
        .find(|c| !c.is_empty())
        .cloned()
        .unwrap_or_default();
        let shot = preview
            .or(m.and_then(|x| {
                x.screenshots
                    .first()
                    .map(|f| f.split('/').fold(x.dir.clone(), |d, c| d.join(c)))
            }))
            .or(p.and_then(|x| x.shot.clone()))
            .or(s.and_then(|x| x.shot.clone()))
            .or(e.and_then(|x| {
                self.shots
                    .as_ref()
                    .map(|d| d.catalog_shot(&x.sha256))
                    .filter(|f| f.is_file())
            }));

        // who needs it
        let mut used_by: Vec<String> = Vec::new();
        for i in &packages {
            for u in &self.cache[*i].used_by {
                if !used_by.contains(u) {
                    used_by.push(u.clone());
                }
            }
        }
        let in_use = packages.iter().any(|i| self.cache[*i].in_use);
        // CLI-1632: a server not asked yet may need it: not called unused and not removable until it is known
        let kept = used_by.is_empty() && sets_uncertain(data, std::time::SystemTime::now());
        let required = match used_by.as_slice() {
            [] => None,
            [one] => Some(format!("{T_REQUIRED_BY} {one}")),
            [one, more @ ..] => Some(format!("{T_REQUIRED_BY} {one} +{}", more.len())),
        };

        // the get button's state
        let busy = data.catalog_busy.is_some();
        let fetch = e.map(|x| match m {
            _ if data.catalog_busy.as_deref() == Some(&x.id) => (T_WAIT, false),
            Some(y) if y.version == x.version => (T_INSTALLED, false),
            Some(_) => (T_UPDATE, !busy),
            None => (T_DOWNLOAD, !busy),
        });
        let enabled = m.is_some_and(|x| data.settings.mods_local_enabled.contains(&x.id));
        let switchable = m.is_some_and(|x| !local_blocked(x));

        let state = match tab {
            ModsTab::Browse => match (get, fetch) {
                (Some(_), Some((T_INSTALLED, _))) => RowState::Words(T_INSTALLED.to_string(), GREEN),
                (Some(i), Some((label, on))) => RowState::Get {
                    i,
                    size: size_words(self.get[i].size),
                    label,
                    on,
                },
                _ => RowState::Words(T_WITH_SERVER.to_string(), MESSAGE_COLOUR),
            },
            ModsTab::Mine => match (local, switchable, required) {
                (Some(i), true, _) => RowState::Switch { i, on: enabled },
                (Some(_), false, None) => RowState::Words(T_CANT_LOAD.to_string(), ORANGE),
                (_, _, Some(r)) => RowState::Words(r, MESSAGE_COLOUR),
                (None, _, None) if kept => RowState::Words(T_KEPT.to_string(), MESSAGE_COLOUR),
                (None, _, None) => RowState::Words(T_UNUSED.to_string(), MESSAGE_COLOUR),
            },
        };

        // the notes of the detail panel
        let mut notes = Vec::new();
        if tab == ModsTab::Browse {
            if let Some(n) = &data.catalog_note {
                notes.push((n.clone(), ORANGE));
            }
        }
        if let Some(x) = m {
            let selected = self
                .report
                .set
                .as_ref()
                .filter(|_| target.addr == data.auth)
                .map(|r| r.policy)
                .or(target.set.as_ref().map(|t| t.policy));
            notes.extend(local_note(x, &self.report, enabled, selected));
        }
        if !used_by.is_empty() {
            notes.push((format!("{T_REQUIRED_BY} {}.", used_by.join(", ")), MESSAGE_COLOUR));
        } else if m.is_none() && !packages.is_empty() {
            notes.push((if kept { T_KEPT_NOTE } else { T_NOT_NEEDED }.to_string(), MESSAGE_COLOUR));
        }
        if in_use {
            notes.push((T_IN_USE.to_string(), GREEN));
        }
        if m.is_none() && packages.is_empty() && s.is_some() {
            notes.push((format!("It comes with {} when you join.", target.name), MESSAGE_COLOUR));
        }
        if let Some(x) = e {
            notes.push((format!("Download size {}.", size_words(x.size)), MESSAGE_COLOUR));
        } else if !packages.is_empty() {
            let bytes = packages.iter().map(|i| self.cache[*i].package.bytes).sum();
            notes.push((format!("{} on this PC.", size_words(bytes)), MESSAGE_COLOUR));
        }

        // the actions, in one line
        let mut actions = Vec::new();
        if let (Some(i), Some((label, on))) = (get, fetch) {
            if label != T_INSTALLED {
                let label = if label == T_DOWNLOAD { T_INSTALL } else { label };
                actions.push(Action { cmd: CMD_DOWNLOAD_BASE + i as i32, label: label.into(), on });
            }
        }
        if let (Some(i), Some(x)) = (local, m) {
            if switchable {
                actions.push(Action {
                    cmd: CMD_TOGGLE_BASE + i as i32,
                    label: if enabled { T_DISABLE } else { T_ENABLE }.into(),
                    on: true,
                });
            }
            if !x.settings.is_empty() {
                actions.push(Action { cmd: CMD_SETTINGS_BASE + i as i32, label: T_SETTINGS.into(), on: true });
            }
            if x.downloaded {
                actions.push(Action { cmd: CMD_REMOVE_LOCAL_BASE + i as i32, label: T_REMOVE.into(), on: true });
            }
        }
        if let Some(i) = packages.first().filter(|_| !m.is_some_and(|x| x.downloaded)) {
            actions.push(Action { cmd: CMD_REMOVE_BASE + *i as i32, label: T_REMOVE.into(), on: !in_use && !kept });
        }
        let accepted: Vec<String> = keys
            .iter()
            .filter(|k| data.settings.mods_accepted.iter().any(|a| a == *k))
            .cloned()
            .collect();
        if !accepted.is_empty() {
            actions.push(Action {
                cmd: CMD_FORGET_BASE + forget.len() as i32,
                label: T_FORGET.into(),
                on: true,
            });
            forget.push(accepted);
        }

        ModRow { id, name, description, version, author, kind, mark, caps, commands, shot, state, notes, actions }
    }

    /// A row's rectangle: `k` rows under the list's top, the list's width less the scroll bar when there is one.
    pub fn row_rect(&self, k: usize) -> [i32; 4] {
        let bar = if self.rows.len() > MODS_ROWS { SCROLL_W + 4 } else { 0 };
        [
            LIST_RECT[0],
            LIST_RECT[1] + k as i32 * MODS_ROW_STEP,
            LIST_RECT[2] - bar,
            MODS_ROW_H,
        ]
    }

    fn rebuild_buttons(&mut self, data: &MenuData) {
        let tab = self.tab.unwrap_or(ModsTab::Browse);
        let mut list = Vec::new();
        let art = |cmd, at, size, label: &str, on| Button {
            cmd,
            at,
            size,
            label: label.into(),
            on,
            kind: ButtonKind::Art,
        };
        // the tabs
        for (k, (cmd, label, t)) in [
            (CMD_TAB_BROWSE, T_BROWSE, ModsTab::Browse),
            (CMD_TAB_MINE, T_MY_MODS, ModsTab::Mine),
        ]
        .into_iter()
        .enumerate()
        {
            list.push(Button {
                cmd,
                at: [LIST_RECT[0] + k as i32 * (TAB[0] + 4), TAB_Y],
                size: TAB,
                label: label.into(),
                on: true,
                kind: ButtonKind::Tab(tab == t),
            });
        }
        // the footer
        let right = MODS_RECT[0] + MODS_RECT[2] - MODS_PAD;
        list.push(art(
            CMD_OPEN_FOLDER,
            [LIST_RECT[0], FOOT_Y],
            [FOOT_FOLDER_W, BUTTON[1]],
            T_OPEN_FOLDER,
            data.mods_dir.is_some(),
        ));
        let unused = if sets_uncertain(data, std::time::SystemTime::now()) {
            0
        } else {
            self.cache.iter().filter(|c| c.used_by.is_empty() && !c.in_use).count()
        };
        if tab == ModsTab::Mine && unused > 0 {
            list.push(art(
                CMD_CLEAN_UP,
                [LIST_RECT[0] + FOOT_FOLDER_W + ACTION_GAP, FOOT_Y],
                [FOOT_WIDE, BUTTON[1]],
                &format!("{T_CLEAN_UP} ({unused})"),
                true,
            ));
        }
        list.push(art(CMD_CLOSE, [right - BUTTON[0], FOOT_Y], BUTTON, T_CLOSE, true));
        // the rows' states
        for k in 0..MODS_ROWS {
            let Some(r) = self.rows.get(self.top + k) else { break };
            let rect = self.row_rect(k);
            let re = rect[0] + rect[2] - ROW_PAD;
            match &r.state {
                RowState::Get { i, label, on, .. } => list.push(art(
                    CMD_DOWNLOAD_BASE + *i as i32,
                    [re - GET_BUTTON[0], rect[1] + (rect[3] - GET_BUTTON[1]) / 2],
                    GET_BUTTON,
                    label,
                    *on,
                )),
                RowState::Switch { i, on } => list.push(Button {
                    cmd: CMD_TOGGLE_BASE + *i as i32,
                    at: [re - SWITCH[0], rect[1] + (rect[3] - SWITCH[1]) / 2],
                    size: SWITCH,
                    label: if *on { T_ON } else { T_OFF }.into(),
                    on: true,
                    kind: ButtonKind::Switch(*on),
                }),
                RowState::Words(..) => {}
            }
        }
        // the detail panel's actions, in one line at its foot
        if let Some(r) = self.selected.and_then(|i| self.rows.get(i)) {
            let mut x = DETAIL_RECT[0] + DETAIL_PAD;
            let y = DETAIL_RECT[1] + DETAIL_RECT[3] - DETAIL_PAD - ACTION[1];
            for a in &r.actions {
                let w = if a.label == T_FORGET { ACTION_WIDE } else { ACTION[0] };
                list.push(art(a.cmd, [x, y], [w, ACTION[1]], &a.label, a.on));
                x += w + ACTION_GAP;
            }
        }
        self.buttons.list = list;
        if let Some(page) = &self.page {
            self.buttons.list = page_buttons(page);
        }
    }

    /// Row `i` of the current tab selected.
    fn select_row(&mut self, i: usize, data: &MenuData) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        let i = i.min(n - 1);
        self.selected = Some(i);
        self.selected_id = Some(self.rows[i].id.clone());
        self.detail_top = 0;
        if i < self.top {
            self.top = i;
        } else if i >= self.top + MODS_ROWS {
            self.top = i + 1 - MODS_ROWS;
        }
        self.rebuild_buttons(data);
    }

    fn set_tab(&mut self, t: ModsTab, data: &MenuData) {
        if self.tab != Some(t) {
            self.tab = Some(t);
            self.top = 0;
            self.detail_top = 0;
            self.selected = None;
            self.selected_id = None;
            self.relayout(data);
        }
    }

    /// MOD-24: opens local mod `i`'s settings page.
    pub fn open_settings(&mut self, i: usize, ctx: &mut Ctx) {
        let Some(m) = self.local.get(i).filter(|m| !m.settings.is_empty()) else {
            return;
        };
        self.page = Some(SettingsPage::open(m, ctx.data.settings.mods_settings.get(&m.id)));
        self.rebuild(ctx);
    }

    /// MOD-24: the page's values into the profile (they reach the mod at the next match load).
    fn store_page(&mut self, ctx: &mut Ctx) {
        if let Some(p) = &self.page {
            ctx.data
                .settings
                .mods_settings
                .insert(p.id.clone(), p.values.clone());
            super::store_settings(ctx.data, ctx.cat);
        }
        self.rebuild(ctx);
    }

    /// CLI-1634: downloaded local mod `i` leaves the disk and the enable list.
    pub fn remove_local(&mut self, i: usize, ctx: &mut Ctx) {
        let Some(m) = self.local.get(i).filter(|m| m.downloaded) else {
            return;
        };
        let id = m.id.clone();
        if let Err(e) = crate::modcatalog::remove_downloaded(&m.dir) {
            ctx.popups.push(Popup::ok(e, BoxTag::None));
        }
        ctx.data.settings.mods_local_enabled.retain(|x| *x != id);
        super::store_settings(ctx.data, ctx.cat);
        ctx.send(prefs_cmd(ctx.data));
        self.rebuild(ctx);
    }

    /// CLI-1634: optional mod `i` is fetched on the link's side; it arrives disabled.
    pub fn download(&mut self, i: usize, ctx: &mut Ctx) {
        let (Some(e), Some(auth)) = (self.get.get(i).cloned(), ctx.data.auth) else {
            return;
        };
        if ctx.data.catalog_busy.is_some() {
            return;
        }
        ctx.data.catalog_busy = Some(e.id.clone());
        ctx.data.catalog_note = None;
        ctx.send(LinkCmd::ModDownload(auth, e));
        self.rebuild(ctx);
    }

    fn rebuild(&mut self, ctx: &mut Ctx) {
        let report = self.report.clone();
        self.refresh(ctx.data, report);
    }

    /// Local mod `i` on / off (saved in the profile; the link takes it for the next join).
    pub fn toggle(&mut self, i: usize, ctx: &mut Ctx) {
        let Some(m) = self.local.get(i) else {
            return;
        };
        if m.broken.is_some() {
            return;
        }
        let id = m.id.clone();
        let list = &mut ctx.data.settings.mods_local_enabled;
        if let Some(k) = list.iter().position(|x| *x == id) {
            list.remove(k);
        } else {
            list.push(id);
        }
        super::store_settings(ctx.data, ctx.cat);
        ctx.send(prefs_cmd(ctx.data));
        self.rebuild(ctx);
    }

    /// Forgets the consent of `key` (the next join that needs the mod asks again).
    pub fn forget_consent(&mut self, key: &str, ctx: &mut Ctx) {
        ctx.data.settings.mods_accepted.retain(|k| k != key);
        super::store_settings(ctx.data, ctx.cat);
        ctx.send(prefs_cmd(ctx.data));
        self.rebuild(ctx);
    }

    /// Removes downloaded package `i` and the other copies of the same mod (zip and unpacked folder); a copy the
    /// loaded mods use stays, and the package itself is refused while they use it.
    pub fn remove(&mut self, i: usize, ctx: &mut Ctx) {
        let Some(c) = self.cache.get(i) else {
            return;
        };
        if c.in_use {
            return;
        }
        let id = c.package.id.clone();
        let sha = c.package.sha256.clone();
        let all: Vec<String> = self
            .cache
            .iter()
            .filter(|o| !o.in_use && (o.package.sha256 == sha || (!id.is_empty() && o.package.id == id)))
            .map(|o| o.package.sha256.clone())
            .collect();
        if let Some(paths) = ctx.data.mods_dir.as_deref().map(ModPaths::new) {
            for s in &all {
                if let Err(e) = remove_cached(&paths, s) {
                    ctx.popups.push(Popup::ok(
                        format!("The mod could not be removed: {e}"),
                        BoxTag::None,
                    ));
                }
            }
        }
        self.rebuild(ctx);
    }

    /// CLI-1632: removes every downloaded package no known server declares (not the loaded ones); the consent keys
    /// of the removed packages go with them.
    pub fn clean_up(&mut self, ctx: &mut Ctx) {
        let before = ctx.data.settings.mods_accepted.len();
        let report = self.report.clone();
        prune_cache(ctx.data, &report, mods::PruneMode::CleanUp, std::time::SystemTime::now());
        if ctx.data.settings.mods_accepted.len() != before {
            super::store_settings(ctx.data, ctx.cat);
            ctx.send(prefs_cmd(ctx.data));
        }
        self.rebuild(ctx);
    }

    fn act(&mut self, cmd: i32, ctx: &mut Ctx) {
        match cmd {
            CMD_TAB_BROWSE => self.set_tab(ModsTab::Browse, ctx.data),
            CMD_TAB_MINE => self.set_tab(ModsTab::Mine, ctx.data),
            modsettings::CMD_PAGE_BACK => {
                self.page = None;
                self.rebuild(ctx);
            }
            c if (modsettings::CMD_PAGE_STEP_BASE..modsettings::CMD_PAGE_NUDGE_BASE).contains(&c) => {
                let k = (c - modsettings::CMD_PAGE_STEP_BASE) as usize;
                if self.page.as_mut().is_some_and(|p| p.step(k)) {
                    self.store_page(ctx);
                }
            }
            c if (modsettings::CMD_PAGE_NUDGE_BASE..modsettings::CMD_PAGE_RESET).contains(&c) => {
                let n = (c - modsettings::CMD_PAGE_NUDGE_BASE) as usize;
                if self.page.as_mut().is_some_and(|p| p.nudge(n / 2, n % 2 == 1)) {
                    self.store_page(ctx);
                }
            }
            modsettings::CMD_PAGE_RESET => {
                if self.page.as_mut().is_some_and(|p| p.reset()) {
                    self.store_page(ctx);
                }
            }
            modsettings::CMD_PAGE_COPY => {
                if let Some(p) = &mut self.page {
                    ctx.data.clipboard_out = Some(p.code.clone());
                    p.message = Some((modsettings::T_COPIED.to_string(), false));
                }
            }
            modsettings::CMD_PAGE_PASTE => ctx.data.clipboard_wanted = true,
            c if (modsettings::CMD_PAGE_COLOUR_BASE..CMD_SETTINGS_BASE).contains(&c) => {
                let n = c - modsettings::CMD_PAGE_COLOUR_BASE;
                let (k, cell) = (n / modsettings::CELLS_PER_ROW, n % modsettings::CELLS_PER_ROW);
                if self.page.as_mut().is_some_and(|p| p.pick(k as usize, cell as usize)) {
                    self.store_page(ctx);
                }
            }
            c if (CMD_SETTINGS_BASE..CMD_SETTINGS_BASE + 1000).contains(&c) => {
                self.open_settings((c - CMD_SETTINGS_BASE) as usize, ctx)
            }
            c if (CMD_DOWNLOAD_BASE..CMD_DOWNLOAD_BASE + 1000).contains(&c) => {
                self.download((c - CMD_DOWNLOAD_BASE) as usize, ctx)
            }
            c if (CMD_REMOVE_LOCAL_BASE..CMD_REMOVE_LOCAL_BASE + 1000).contains(&c) => {
                self.remove_local((c - CMD_REMOVE_LOCAL_BASE) as usize, ctx)
            }
            CMD_CLOSE => *ctx.go = Some(Go::Back),
            CMD_CLEAN_UP => self.clean_up(ctx),
            CMD_OPEN_FOLDER => {
                if let Some(d) = ctx.data.mods_dir.as_deref() {
                    open_folder(&ModPaths::new(d).local());
                }
            }
            c if (CMD_TOGGLE_BASE..CMD_TOGGLE_BASE + 1000).contains(&c) => {
                self.toggle((c - CMD_TOGGLE_BASE) as usize, ctx)
            }
            c if (CMD_FORGET_BASE..CMD_FORGET_BASE + 1000).contains(&c) => {
                if let Some(keys) = self.forget.get((c - CMD_FORGET_BASE) as usize).cloned() {
                    for k in keys {
                        self.forget_consent(&k, ctx);
                    }
                }
            }
            c if (CMD_REMOVE_BASE..CMD_REMOVE_BASE + 1000).contains(&c) => {
                self.remove((c - CMD_REMOVE_BASE) as usize, ctx)
            }
            c if (CMD_ROW_BASE..CMD_ROW_BASE + 1000).contains(&c) => {
                self.select_row((c - CMD_ROW_BASE) as usize, ctx.data)
            }
            _ => {}
        }
    }

    fn draw_row(&self, cat: &mut Catalog, k: usize, r: &ModRow, out: &mut Vec<DrawItem>) {
        let rect = self.row_rect(k);
        let sel = self.selected == Some(self.top + k);
        out.push(fill([rect[0], rect[1]], [rect[2], rect[3]], if sel { ROW_PICKED } else { ROW_BACK }));
        // the thumbnail's column is always there, so the rows' text keeps one left edge
        let thumb = [rect[0] + ROW_PAD, rect[1] + (rect[3] - THUMB[1]) / 2];
        out.push(fill(thumb, THUMB, [0, 0, 0, 170]));
        if let Some(t) = r.shot.as_ref().and_then(|s| shot_texture(cat, s)) {
            contained(cat, &t, [thumb[0], thumb[1], THUMB[0], THUMB[1]], out);
        }
        let re = rect[0] + rect[2] - ROW_PAD;
        let state_w = match &r.state {
            RowState::Get { size, .. } => GET_BUTTON[0] + ROW_PAD + measure(cat, size),
            RowState::Switch { .. } => SWITCH[0],
            RowState::Words(w, _) => measure(cat, w).min(STATE_MAX_W),
        };
        let tx = rect[0] + ROW_TEXT_DX;
        let room = re - state_w - ROW_PAD - tx;
        let top = rect[1] + (rect[3] - 2 * LINE) / 2;
        out.push(text(&fit(cat, &r.name, room), [tx, top], if sel { 0xffff_ffff } else { TEXT_COLOUR }));
        out.push(text(&fit(cat, &r.description, room), [tx, top + LINE], MESSAGE_COLOUR));
        let mid = rect[1] + rect[3] / 2;
        match &r.state {
            RowState::Get { size, .. } => out.push(right_mid(size, re - GET_BUTTON[0] - ROW_PAD, mid, MESSAGE_COLOUR)),
            RowState::Words(w, c) => out.push(right_mid(&fit(cat, w, STATE_MAX_W), re, mid, *c)),
            RowState::Switch { .. } => {}
        }
    }

    fn draw_detail(&self, cat: &mut Catalog, r: &ModRow, out: &mut Vec<DrawItem>) {
        let [px, py, pw, ph] = DETAIL_RECT;
        let (ix, iw) = (px + DETAIL_PAD, pw - 2 * DETAIL_PAD);
        // the text below the picture, wrapped, never cut
        let mut lines: Vec<(String, u32)> = Vec::new();
        let mut head = format!("Version {}", r.version);
        if !r.author.is_empty() {
            head.push_str(&format!(" by {}", r.author));
        }
        head.push_str(&format!("  -  {}", r.kind));
        lines.extend(wrap(cat, &head, iw).into_iter().map(|l| (l, MESSAGE_COLOUR)));
        if !r.description.is_empty() {
            lines.push((String::new(), 0));
            lines.extend(wrap(cat, &r.description, iw).into_iter().map(|l| (l, BODY_TEXT)));
        }
        if !r.caps.is_empty() {
            lines.push((String::new(), 0));
            lines.push((T_WHAT_IT_DOES.to_string(), TITLE_COLOUR));
            for c in &r.caps {
                lines.extend(wrap(cat, &format!("- {c}"), iw).into_iter().map(|l| (l, BODY_TEXT)));
            }
        }
        // DC-DEFAULT: MOD-27 (proposed, USER REQUEST 2026-11-01) the manifest's chat commands
        if !r.commands.is_empty() {
            lines.push((String::new(), 0));
            lines.push((T_COMMANDS.to_string(), TITLE_COLOUR));
            for c in &r.commands {
                let line = format!("{} - {}", c.command, c.description);
                lines.extend(wrap(cat, &line, iw).into_iter().map(|l| (l, BODY_TEXT)));
            }
        }
        if !r.notes.is_empty() {
            lines.push((String::new(), 0));
            for (n, c) in &r.notes {
                lines.extend(wrap(cat, n, iw).into_iter().map(|l| (l, *c)));
            }
        }
        let foot = py + ph - DETAIL_PAD - ACTION[1] - 8;
        let mut y = py + DETAIL_PAD;
        let shot = r.shot.as_ref().and_then(|s| shot_texture(cat, s));
        // the picture takes what the text leaves, down to a minimum
        let name_h = LINE + 4;
        let free = foot - y - name_h - lines.len() as i32 * LINE - 8;
        if let Some(t) = &shot {
            let h = free.clamp(SHOT_MIN_H, SHOT_H);
            out.push(fill([ix, y], [iw, h], [0, 0, 0, 170]));
            contained(cat, t, [ix, y, iw, h], out);
            y += h + 8;
        }
        let mark_w = r.mark.map_or(0, |m| measure(cat, m.text()) + 10);
        out.push(text(&fit(cat, &r.name, iw - mark_w), [ix, y], TITLE_COLOUR));
        if let Some(m) = r.mark {
            out.push(place_text(m.text(), [ix + iw, y], [0, 0], TR, FONT, FONT_SIZE, m.colour()));
        }
        y += name_h;
        // what does not fit scrolls with the wheel over the panel
        let fits = ((foot - y) / LINE).max(1) as usize;
        let max = lines.len().saturating_sub(fits);
        self.detail_max.set(max);
        let from = self.detail_top.min(max);
        for (k, (l, c)) in lines.iter().skip(from).take(fits).enumerate() {
            if !l.is_empty() {
                out.push(text(l, [ix, y + k as i32 * LINE], *c));
            }
        }
        if max > 0 {
            scroll_bar([px + pw - SCROLL_W - 2, y, SCROLL_W, fits as i32 * LINE], from, fits, lines.len(), out);
        }
    }
}

/// `s` right-aligned at `x`, centred on `y`.
fn right_mid(s: &str, x: i32, y: i32, colour: u32) -> DrawItem {
    place_text(
        s,
        [x, y],
        [0, 0],
        Align { first: ALIGN_RIGHT, second: ALIGN_SECOND_CENTRE },
        FONT,
        FONT_SIZE,
        colour,
    )
}

/// A scroll bar in `r`: `shown` of `total` entries from `from`.
fn scroll_bar(r: [i32; 4], from: usize, shown: usize, total: usize, out: &mut Vec<DrawItem>) {
    out.push(fill([r[0], r[1]], [r[2], r[3]], [0, 0, 0, 150]));
    let total = total.max(1) as i32;
    let h = (r[3] * shown as i32 / total).max(16).min(r[3]);
    let y = r[1] + (r[3] - h) * from as i32 / (total - shown as i32).max(1);
    out.push(fill([r[0], y], [r[2], h], [150, 130, 70, 255]));
}

/// Texture `tex` whole inside `b`, its proportions kept on the real screen (the virtual screen is stretched by
/// different factors per axis on a non-4:3 window).
fn contained(cat: &mut Catalog, tex: &str, b: [i32; 4], out: &mut Vec<DrawItem>) {
    let Some((tw, th)) = cat.tex_size(tex) else {
        return;
    };
    let (sw, sh) = cat.screen_size();
    let (sx, sy) = (sw as f32 / 1024.0, sh as f32 / 768.0);
    let k = (b[2] as f32 * sx / tw as f32).min(b[3] as f32 * sy / th as f32);
    let (w, h) = (tw as f32 * k / sx, th as f32 * k / sy);
    out.push(DrawItem::Image {
        tex: tex.to_string(),
        src: [tw as f32, th as f32],
        dst: [
            b[0] as f32 + (b[2] as f32 - w) / 2.0,
            b[1] as f32 + (b[3] as f32 - h) / 2.0,
            w,
            h,
        ],
        tint: [255; 4],
        turn: None,
    });
}

/// The texture name of a screenshot file, decoded into the catalogue on first use (PNG; anything else, or a file
/// that does not decode, gives none).
// DC-DEFAULT: CLI-1617 (proposed) PNG screenshots are shown; a JPEG one shows no picture yet
fn shot_texture(cat: &mut Catalog, path: &Path) -> Option<String> {
    let name = format!("modshot:{}", path.display());
    if cat.assets.generated(&name).is_none() {
        let tex = std::fs::read(path)
            .ok()
            .filter(|b| b.len() as u64 <= dc_mod_lua::MAX_SCREENSHOT_BYTES)
            .and_then(|b| image::load_from_memory_with_format(&b, image::ImageFormat::Png).ok())
            .map(|i| i.to_rgba8())
            .map(|i| Tex {
                width: i.width(),
                height: i.height(),
                rgba: i.into_raw(),
            })
            .unwrap_or(Tex {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            });
        cat.assets.put(&name, tex);
    }
    cat.tex_size(&name)
        .filter(|(w, h)| *w > 1 || *h > 1)
        .map(|_| name)
}

impl Screen for ModsScreen {
    fn name(&self) -> &'static str {
        MODS_NAME
    }
    fn layout_rects(&self) -> Option<([i32; 4], Vec<[i32; 4]>)> {
        Some((MODS_RECT, button_rects(&self.buttons)))
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }
    fn shows_under(&self) -> bool {
        true
    }
    fn enter(&mut self, ctx: &mut Ctx) {
        // CLI-1634: the login lobby's optional mods, read each time the window opens
        if let Some(auth) = ctx.data.auth {
            ctx.data.catalog = crate::modcatalog::CatalogState::Loading;
            ctx.send(LinkCmd::ModCatalog(auth));
        }
        let r = ctx.link.join_report();
        self.refresh(ctx.data, r);
    }
    fn mods_changed(&mut self, data: &MenuData, report: JoinReport) {
        self.refresh(data, report);
    }
    fn animate(&mut self, _secs: f32) -> bool {
        self.page.is_some()
    }
    fn animating(&self) -> bool {
        self.page.is_some()
    }
    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        match k {
            MenuKey::Escape => {
                if self.page.take().is_some() {
                    self.rebuild(ctx);
                } else {
                    *ctx.go = Some(Go::Back);
                }
            }
            _ if self.page.is_some() => {}
            MenuKey::Up => self.select_row(self.selected.unwrap_or(0).saturating_sub(1), ctx.data),
            MenuKey::Down => self.select_row(self.selected.map_or(0, |i| i + 1), ctx.data),
            MenuKey::Tab | MenuKey::BackTab => {
                let t = if self.tab == Some(ModsTab::Mine) { ModsTab::Browse } else { ModsTab::Mine };
                self.set_tab(t, ctx.data);
            }
            _ => {}
        }
    }
    fn click(&mut self, cmd: i32, _part: usize, ctx: &mut Ctx) {
        self.act(cmd, ctx);
    }
    fn copy_text(&self) -> Option<String> {
        self.page.as_ref().map(|p| p.code.clone())
    }
    fn paste(&mut self, text: &str, ctx: &mut Ctx) -> bool {
        let Some(p) = &mut self.page else {
            return false;
        };
        if p.import(text) {
            self.store_page(ctx);
        }
        true
    }
    fn slider_rects(&self) -> Vec<[i32; 4]> {
        self.page.as_ref().map_or_else(Vec::new, |p| {
            p.sliders().into_iter().map(|sl| p.track_rect(PAGE_X, PAGE_TOP, sl)).collect()
        })
    }
    fn wheel(&mut self, p: [f32; 2], notches: i32, ctx: &mut Ctx) -> bool {
        if let Some(page) = &mut self.page {
            page.scroll_by(notches);
            self.rebuild_buttons(ctx.data);
            return true;
        }
        let [dx, dy, dw, dh] = DETAIL_RECT;
        if inside([dx, dy], [dw, dh], p) {
            self.detail_top =
                (self.detail_top as i32 - notches).clamp(0, self.detail_max.get() as i32) as usize;
            return true;
        }
        let max = self.rows.len().saturating_sub(MODS_ROWS);
        self.top = (self.top as i32 - notches).clamp(0, max as i32) as usize;
        self.rebuild_buttons(ctx.data);
        true
    }
    fn modal_input(&mut self, ev: &MenuInput, ctx: &mut Ctx) -> bool {
        match *ev {
            MenuInput::Move(p) => {
                if let Some(page) = &mut self.page {
                    if let Some(sl) = page.drag {
                        page.drag_to(PAGE_X, sl, p[0]);
                        return true;
                    }
                }
                if self.buttons.moved(p) {
                    ctx.sound(super::frontfx::SOUND_SELECT_CHANGE);
                }
            }
            MenuInput::Press(p) => {
                if self.buttons.press(p) {
                    return true;
                }
                if let Some(page) = &mut self.page {
                    if let Some(sl) = page.slider_at(PAGE_X, PAGE_TOP, p) {
                        page.drag = Some(sl);
                        page.drag_to(PAGE_X, sl, p[0]);
                        return true;
                    }
                    if let Some((k, c)) = page.cell_at(PAGE_X, PAGE_TOP, p) {
                        let cmd = modsettings::CMD_PAGE_COLOUR_BASE
                            + k as i32 * modsettings::CELLS_PER_ROW
                            + c as i32;
                        self.act(cmd, ctx);
                    }
                    return true;
                }
                for k in 0..MODS_ROWS {
                    let r = self.row_rect(k);
                    let i = self.top + k;
                    if i < self.rows.len() && inside([r[0], r[1]], [r[2], r[3]], p) {
                        ctx.sound(super::frontfx::SOUND_SELECT);
                        self.select_row(i, ctx.data);
                        break;
                    }
                }
            }
            MenuInput::Release(p) => {
                if self.page.as_mut().is_some_and(|page| page.drag.take().is_some()) {
                    // the profile takes the value the drag ended on, not each step of it
                    self.store_page(ctx);
                    return true;
                }
                if let Some(c) = self.buttons.release(p) {
                    ctx.sound(super::frontfx::SOUND_SELECT);
                    self.act(c, ctx);
                }
            }
            _ => return false,
        }
        true
    }
    fn draw_extra(&self, cat: &mut Catalog, _data: &MenuData, secs: f32, out: &mut Vec<DrawItem>) {
        let [x, _, w, _] = MODS_RECT;
        frame(cat, &self.art, MODS_RECT, out);
        if let Some(page) = &self.page {
            draw_page(cat, page, secs, out);
            self.buttons.draw(cat, out);
            return;
        }
        out.push(title_text(T_MODS, LIST_RECT[0], MODS_RECT[1], false, TITLE_COLOUR));
        let title_w = measure(cat, T_MODS) + 24;
        let (ctx_line, colour) = &self.context;
        let ctx_line = fit(cat, ctx_line, w - 2 * MODS_PAD - title_w);
        out.push(title_text(&ctx_line, x + w - MODS_PAD, MODS_RECT[1], true, *colour));
        if self.rows.is_empty() {
            let [lx, ly, lw, lh] = [LIST_RECT[0], LIST_RECT[1], MODS_RECT[2] - 2 * MODS_PAD, LIST_RECT[3]];
            out.push(fill([lx, ly], [lw, lh], PANEL_BACK));
            let lines = wrap(cat, &self.empty, lw - 40);
            let top = ly + lh / 2 - (lines.len() as i32 - 1) * LINE / 2;
            for (k, l) in lines.iter().enumerate() {
                out.push(place_text(
                    l,
                    [lx + lw / 2, top + k as i32 * LINE],
                    [0, 0],
                    MID,
                    FONT,
                    FONT_SIZE,
                    MESSAGE_COLOUR,
                ));
            }
        } else {
            for k in 0..MODS_ROWS {
                if let Some(r) = self.rows.get(self.top + k) {
                    self.draw_row(cat, k, r, out);
                }
            }
            if self.rows.len() > MODS_ROWS {
                let [lx, ly, lw, lh] = LIST_RECT;
                scroll_bar([lx + lw - SCROLL_W, ly, SCROLL_W, lh], self.top, MODS_ROWS, self.rows.len(), out);
            }
            out.push(fill([DETAIL_RECT[0], DETAIL_RECT[1]], [DETAIL_RECT[2], DETAIL_RECT[3]], PANEL_BACK));
            if let Some(r) = self.selected.and_then(|i| self.rows.get(i)) {
                self.draw_detail(cat, r, out);
            }
        }
        self.buttons.draw(cat, out);
    }
}

/// MOD-24: the settings page's places.
const PAGE_X: i32 = MODS_RECT[0] + MODS_PAD + 8;
const PAGE_TOP: i32 = MODS_RECT[1] + 76;
const PAGE_VALUE_X: i32 = PAGE_X + modsettings::VALUE_DX;
const PAGE_VALUE: [i32; 2] = [180, 24];
const PREVIEW_AT: [i32; 2] = [
    MODS_RECT[0] + MODS_RECT[2] - MODS_PAD - 8 - modsettings::PREVIEW[0],
    PAGE_TOP,
];
/// The share code's place under the preview.
const SHARE_Y: i32 = PAGE_TOP + modsettings::PREVIEW[1] + 32;

fn page_buttons(page: &SettingsPage) -> Vec<Button> {
    let mut list = vec![Button {
        cmd: modsettings::CMD_PAGE_BACK,
        at: [MODS_RECT[0] + MODS_RECT[2] - MODS_PAD - BUTTON[0], FOOT_Y],
        size: BUTTON,
        label: T_BACK.into(),
        on: true,
        kind: ButtonKind::Art,
    }];
    list.push(Button {
        cmd: modsettings::CMD_PAGE_RESET,
        at: [PAGE_X, FOOT_Y],
        size: [150, BUTTON[1]],
        label: modsettings::T_RESET.into(),
        on: true,
        kind: ButtonKind::Art,
    });
    for (i, (cmd, label)) in [
        (modsettings::CMD_PAGE_COPY, modsettings::T_COPY),
        (modsettings::CMD_PAGE_PASTE, modsettings::T_PASTE),
    ]
    .into_iter()
    .enumerate()
    {
        list.push(Button {
            cmd,
            at: [PREVIEW_AT[0] + i as i32 * (BUTTON[0] + 10), SHARE_Y + 58],
            size: BUTTON,
            label: label.into(),
            on: true,
            kind: ButtonKind::Art,
        });
    }
    for (k, d) in page.decl.iter().enumerate() {
        let y = page.row_y(PAGE_TOP, k);
        if matches!(d.kind, dc_mod_lua::SettingKind::Bool | dc_mod_lua::SettingKind::Choice(_))
            && page.in_view(PAGE_TOP, y, PAGE_VALUE[1])
        {
            list.push(Button {
                cmd: modsettings::CMD_PAGE_STEP_BASE + k as i32,
                at: [PAGE_VALUE_X, y],
                size: PAGE_VALUE,
                label: page.value_label(k),
                on: true,
                kind: ButtonKind::Art,
            });
        }
    }
    for (i, sl) in page.sliders().into_iter().enumerate() {
        let [tx, ty, tw, _] = page.track_rect(PAGE_X, PAGE_TOP, sl);
        if !page.in_view(PAGE_TOP, ty, modsettings::NUDGE[1]) {
            continue;
        }
        let nudge = modsettings::NUDGE;
        for (up, x, label) in [(0, tx - nudge[0] - 6, "-"), (1, tx + tw + 6, "+")] {
            list.push(Button {
                cmd: modsettings::CMD_PAGE_NUDGE_BASE + 2 * i as i32 + up,
                at: [x, ty],
                size: nudge,
                label: label.into(),
                on: true,
                // the art needs more width than a step button has
                kind: ButtonKind::Tab(false),
            });
        }
    }
    list
}

fn draw_page(cat: &mut Catalog, page: &SettingsPage, secs: f32, out: &mut Vec<DrawItem>) {
    let [x, y, ..] = MODS_RECT;
    let tx = x + MODS_PAD;
    out.push(title_text(&format!("{} - {T_SETTINGS}", page.name), tx, y, false, TITLE_COLOUR));
    out.push(text(modsettings::T_APPLIES, [tx, TAB_Y], MESSAGE_COLOUR));
    let row_text = |s: &str, x: i32, y: i32, colour: u32| {
        place_text(
            s,
            [x, y + PAGE_VALUE[1] / 2],
            [0, 0],
            Align { first: ALIGN_LEFT, second: ALIGN_SECOND_CENTRE },
            FONT,
            FONT_SIZE,
            colour,
        )
    };
    let label_w = modsettings::VALUE_DX - 10;
    for (k, d) in page.decl.iter().enumerate() {
        let y = page.row_y(PAGE_TOP, k);
        if let Some(g) = page.group_of(k) {
            let gy = y - modsettings::GROUP_H;
            if page.in_view(PAGE_TOP, gy, modsettings::GROUP_H) {
                out.push(text(g, [PAGE_X, gy + 1], TITLE_COLOUR));
                out.push(fill([PAGE_X, gy + 19], [PREVIEW_AT[0] - PAGE_X - 12, 1], [90, 96, 104, 255]));
            }
        }
        if page.in_view(PAGE_TOP, y, PAGE_VALUE[1]) {
            out.push(row_text(&fit(cat, &d.label, label_w), PAGE_X, y, TEXT_COLOUR));
        }
    }
    page.draw_palettes(PAGE_X, PAGE_TOP, out);
    page.draw_sliders(PAGE_X, PAGE_TOP, out);
    for sl in page.sliders() {
        let [tx, ty, tw, _] = page.track_rect(PAGE_X, PAGE_TOP, sl);
        if !page.in_view(PAGE_TOP, ty, PAGE_VALUE[1]) {
            continue;
        }
        if let modsettings::Slider::Channel(_, c) = sl {
            out.push(row_text(modsettings::CHANNELS[c], PAGE_X + 12, ty, TEXT_COLOUR));
        }
        let vx = tx + tw + 6 + modsettings::NUDGE[0] + 6;
        out.push(row_text(&page.slider_text(sl), vx, ty, TEXT_COLOUR));
    }
    let max = page.max_scroll();
    if max > 0 {
        // where the rows stand, beside them
        let bar = PREVIEW_AT[0] - 8;
        let h = modsettings::VIEW_H;
        let thumb = (h * h / (h + max)).max(20);
        out.push(fill([bar, PAGE_TOP], [4, h], [12, 14, 16, 255]));
        out.push(fill([bar, PAGE_TOP + (h - thumb) * page.scroll / max], [4, thumb], [160, 160, 160, 255]));
    }
    let [px, py] = PREVIEW_AT;
    let [pw, ph] = modsettings::PREVIEW;
    let centre = [(px + pw / 2) as f32, (py + ph / 2) as f32];
    if let Some(c) = page.preview_item(centre, secs) {
        out.push(fill([px, py], [pw, ph], [30, 34, 38, 255]));
        out.push(DrawItem::Crosshair(c));
        out.push(text(modsettings::T_PREVIEW, [px, py + ph + 6], MESSAGE_COLOUR));
    }
    out.push(text(modsettings::T_SHARE, [px, SHARE_Y], TITLE_COLOUR));
    // the code on two lines at a group's dash when it does not fit on one
    let dash = page.code.char_indices().filter(|(_, c)| *c == '-').nth(4).map(|(i, _)| i);
    let lines: [&str; 2] = match dash.filter(|_| measure(cat, &page.code) > pw) {
        Some(i) => [&page.code[..i + 1], &page.code[i + 1..]],
        None => [&page.code, ""],
    };
    for (i, l) in lines.into_iter().enumerate() {
        out.push(text(l, [px, SHARE_Y + 20 + i as i32 * LINE], TEXT_COLOUR));
    }
    if let Some((m, bad)) = &page.message {
        let colour = if *bad { 0xFFFF_6060 } else { MESSAGE_COLOUR };
        for (i, l) in wrap(cat, m, pw).iter().enumerate() {
            out.push(text(l, [px, SHARE_Y + 92 + i as i32 * LINE], colour));
        }
    }
}

/// The consent prompt (CLI-1618): the unverified mods of the server with their author, source and capabilities in
/// plain words; Accept / Cancel. Drawn over the login screen.
#[derive(Debug, Clone)]
pub struct ConsentBox {
    pub set: ServerModSet,
    pub mods: Vec<ServerMod>,
    art: WindowArt,
    buttons: Buttons,
}

const CONSENT_W: i32 = 600;

impl ConsentBox {
    pub fn new(cat: &Catalog, set: ServerModSet, mods: Vec<ServerMod>) -> ConsentBox {
        let mut b = ConsentBox {
            set,
            mods,
            art: WindowArt::from_rom(cat.rom.as_deref()),
            buttons: Buttons::default(),
        };
        let r = b.rect();
        let by = r[1] + r[3] - 45;
        let cx = r[0] + r[2] / 2;
        b.buttons.list = vec![
            Button {
                cmd: CMD_AGREE,
                at: [cx - BUTTON[0] - 10, by],
                size: BUTTON,
                label: T_ACCEPT.into(),
                on: true,
                kind: ButtonKind::Art,
            },
            Button {
                cmd: CMD_DECLINE,
                at: [cx + 10, by],
                size: BUTTON,
                label: T_CANCEL.into(),
                on: true,
                kind: ButtonKind::Art,
            },
        ];
        b
    }

    /// The prompt's frame and its buttons (the layout test checks the buttons lie inside the frame).
    pub fn layout_rects(&self) -> ([i32; 4], Vec<[i32; 4]>) {
        (self.rect(), button_rects(&self.buttons))
    }

    /// The keys of the mods the player is asked about (remembered on Accept).
    pub fn keys(&self) -> Vec<String> {
        self.mods.iter().map(|m| m.key()).collect()
    }

    /// The lines of one mod: title, author, source, capabilities.
    pub fn mod_lines(m: &ServerMod) -> [String; 4] {
        let source = m
            .source_repo
            .clone()
            .or(m.homepage.clone())
            .unwrap_or_else(|| "not given".into());
        [
            format!("{} {}", m.name.clone().unwrap_or_else(|| m.id.clone()), m.version),
            format!("Author: {}", m.author.clone().unwrap_or_else(|| "unknown".into())),
            format!("Source: {source}"),
            format!("It {}", capability_words(&m.capabilities)),
        ]
    }

    pub fn rect(&self) -> [i32; 4] {
        let shown = self.mods.len().min(5) as i32;
        let h = 60 + shown * (4 * LINE + 8) + 3 * LINE + 60;
        [(1024 - CONSENT_W) / 2, (768 - h) / 2, CONSENT_W, h]
    }

    /// A pointer input; `Some(true)` accept, `Some(false)` cancel.
    pub fn input(&mut self, ev: &MenuInput) -> Option<bool> {
        match *ev {
            MenuInput::Move(p) => {
                self.buttons.moved(p);
            }
            MenuInput::Press(p) => {
                self.buttons.press(p);
            }
            MenuInput::Release(p) => match self.buttons.release(p) {
                Some(CMD_AGREE) => return Some(true),
                Some(CMD_DECLINE) => return Some(false),
                _ => {}
            },
            _ => {}
        }
        None
    }

    pub fn draw(&self, cat: &mut Catalog, out: &mut Vec<DrawItem>) {
        let r = self.rect();
        out.push(fill([0, 0], [1024, 768], [0, 0, 0, 110]));
        frame(cat, &self.art, r, out);
        out.push(title_text(T_CONSENT_TITLE, r[0] + 24, r[1], false, TITLE_COLOUR));
        let mut y = r[1] + 44;
        for m in self.mods.iter().take(5) {
            let l = Self::mod_lines(m);
            out.push(text(&fit(cat, &l[0], r[2] - 140), [r[0] + 24, y], TEXT_COLOUR));
            out.push(place_text(
                T_UNVERIFIED,
                [r[0] + r[2] - 24, y],
                [0, 0],
                TR,
                FONT,
                FONT_SIZE,
                ORANGE,
            ));
            for (k, s) in l[1..].iter().enumerate() {
                out.push(text(
                    &fit(cat, s, r[2] - 60),
                    [r[0] + 36, y + (k as i32 + 1) * LINE],
                    if k == 2 { ORANGE } else { MESSAGE_COLOUR },
                ));
            }
            y += 4 * LINE + 8;
        }
        if self.mods.len() > 5 {
            out.push(text(
                &format!("and {} more", self.mods.len() - 5),
                [r[0] + 24, y],
                MESSAGE_COLOUR,
            ));
        }
        let ny = r[1] + r[3] - 45 - 3 * LINE;
        for (k, l) in wrap(cat, T_CONSENT_NOTE, r[2] - 40).iter().take(3).enumerate() {
            out.push(text(l, [r[0] + 20, ny + k as i32 * LINE], MESSAGE_COLOUR));
        }
        self.buttons.draw(cat, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capability_words_follow_the_fixed_table() {
        assert_eq!(mods::capability_text("content.weapons"), "changes weapon stats");
        assert_eq!(
            mods::capability_text("world.full_visibility"),
            "everyone is visible through walls"
        );
        assert_eq!(mods::capability_text("hooks.lobby"), "changes rewards and lobby rules");
        assert_eq!(mods::capability_text("content.maps"), "adds new maps");
        assert_eq!(mods::capability_text("odd.thing"), "odd.thing");
        assert_eq!(
            capability_words(&["content.weapons".into(), "hooks.lobby".into()]),
            "changes weapon stats, changes rewards and lobby rules"
        );
    }
}
