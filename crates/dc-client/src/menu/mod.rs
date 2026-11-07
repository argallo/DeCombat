//! The 2009 front end: login, server select, channel select, lobby (room list), room creation, waiting room;
//! then the game and back to the waiting room.
//!
//! Screens are drawn with the 2009 layout pages, images and strings of the user's ROM ([`catalog`], [`sheet`])
//! into the same CPU display list the in-match HUD uses, and are driven by [`MenuInput`]s (keys, characters,
//! pointer moves and clicks in the virtual 1024 x 768 screen) from a window ([`window`], feature `window`), a
//! headless run or a test ([`pilot`]). The connections live on their own thread ([`link`]); when the game server's
//! address arrives the lobby connection goes to the game session ([`handoff`]) and comes back after the match.
//!
//! Screen flow (OBSERVED in the 2009 client's menu log, `captures/20261025_202429_sbs4-real/driver/menu_steps.log`):
//! login -> server select -> channel select -> lobby -> room create -> waiting room -> game -> waiting room.
// SPEC: PROTOCOL_2009.md §2-§3, §7; FMT_CRES_UI.md §6.3
// OBSERVED: capture 20261025_202429_sbs4-real/driver/menu_steps.log (screen ids 0x11 login,
// 0xb servers, 0xc channels, 0xf lobby, 0x8 create, 0x9 room; back to the room after leaving the match)

pub mod award;
pub mod buybox;
pub mod catalog;
pub mod clipboard;
pub mod channels;
pub mod charinfo;
pub mod chatdrop;
pub mod clan;
pub mod clanpage;
pub mod colorbox;
pub mod countrybox;
pub mod create;
pub mod cursor;
pub mod diagbox;
pub mod drawlist;
pub mod dropdown;
pub mod facebox;
pub mod figure;
pub mod frontfx;
pub mod gm;
pub mod handoff;
pub mod help;
pub mod inbox;
pub mod link;
pub mod lobby;
pub mod login;
pub mod markbox;
pub mod megabox;
pub mod messenger;
pub mod modbrowser;
pub mod modcache;
pub mod modsettings;
pub mod moddedrows;
pub mod modelview;
pub mod modpanel;
pub mod nickname;
pub mod nine;
pub mod options;
pub mod optionsmods;
pub mod ownflag;
pub mod paint;
pub mod pane;
pub mod pilot;
pub mod popup;
pub mod prefetch;
pub mod recordbox;
pub mod reel;
pub mod reelfx;
pub mod report;
pub mod reportdlg;
pub mod result;
pub mod room;
pub mod roomtext;
pub mod scrollbar;
pub mod servers;
pub mod sheet;
pub mod shop;
pub mod shopxray;
pub mod signup;
pub mod slotpick;
pub mod storage;
pub mod sysfont;
pub mod textbox;
pub mod tuning;
pub mod updatebox;
pub mod usebox;
#[cfg(feature = "window")]
pub mod window;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Instant;

use dc_botclient::lobby::{Lobby, LobbyProfile, RoomMember, ServerEntry};
use dc_ui_data::{PageTag, WidgetKind};

use crate::hud::items::DrawItem;
use crate::overlay::text::RgbaImage;
use catalog::Catalog;
use link::{ChannelRow, Link, LinkCmd, LinkEvent, LinkOptions, RoomRow, RoomView, UserRow};
use popup::{text_ids, BoxTag, Popup};

/// audit-function: the expired item's box text (the item's name filled in; CLI-1389).
// EVIDENCE: cshell+0x22cde0 (res 0x1786 formatted with the name)
pub const EXPIRED_TEXT: i32 = 0x1786;
/// passes-effect: the box of the own megaphone message (CLI-1392).
// EVIDENCE: cshell+0x38190 (res 0x1904)
pub const OWN_MEGAPHONE_TEXT: i32 = 0x1904;
use sheet::Sheet;

use crate::music::{cue_for_screen_state, MusicCue};

/// Keys the screens use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    Enter,
    Escape,
    Tab,
    BackTab,
    Backspace,
    Space,
    Up,
    Down,
    /// CLI-1173: the chat target keys (channel chat, clan chat).
    F2,
    F3,
}

/// One input. Positions are virtual 1024 x 768 coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuInput {
    Key(MenuKey),
    Char(char),
    Move([f32; 2]),
    Press([f32; 2]),
    Release([f32; 2]),
    /// A press of the other (right) mouse button.
    PressAlt([f32; 2]),
    /// A screen command as a button with this command number would send it (scripts, tests).
    Command(i32),
    /// w3-options: a key by its keyboard scan code (set 1), for a screen waiting for one ([`Menu::wants_scan`]: the
    /// options' key boxes).
    Scan(u8),
    /// A wheel turn at the pointer: notches (one per 120 of the window's delta), positive away from the user.
    // EVIDENCE: cshell+0x1a4520 (the window's wheel message -> the interface with x, y and delta / 0x78)
    Wheel([f32; 2], i32),
    /// The clipboard's text for the focused box (Ctrl+V, Cmd+V on macOS): inserted like typed characters, control
    /// characters dropped, cut off at the box's length.
    // DC-DEFAULT: CLI-1510
    Paste(String),
    /// Cut (Ctrl+X, Cmd+X): the window has copied the box's text; this empties the focused plain edit box.
    // DC-DEFAULT: CLI-1511
    Cut,
}

/// Pages of the ROM's page table by the client's tag index (the index -> page name map was read from the user's
/// ROM at run time; the names stay there).
// OBSERVED: page table of the user's ROM (tag index -> page name, report reports/client-lobby.md)
// EVIDENCE: cshell+0x1d2670 (page tags by table index), cshell+0x1ad200 (screens load their pages by name)
pub mod pages {
    use dc_ui_data::PageTag;
    pub const COMMON: PageTag = PageTag::Page0;
    pub const LOBBY_COMMON: PageTag = PageTag::Page1;
    pub const LOGIN: PageTag = PageTag::Page2;
    pub const SERVERS: PageTag = PageTag::Page3;
    pub const CHANNELS: PageTag = PageTag::Page4;
    pub const ROOM_CREATE: PageTag = PageTag::Page5;
    pub const WAITING: PageTag = PageTag::Page6;
    pub const ROOM_LIST: PageTag = PageTag::Page8;
    pub const CHAT: PageTag = PageTag::Page9;
    /// The result screen's page (the screen picks it by the name its build loads, this tag when absent).
    // EVIDENCE: cshell+0x132350 (screen 7 loads its page by name)
    pub const RESULT: PageTag = PageTag::Page7;
    /// w3-shop: the shop & storage page (screen 0x16) and the modification shop page (screen 0x14).
    // OBSERVED: page table of the user's ROM (tag index 11 -> the shop / inventory page, 10 -> the tuning shop page;
    // scratch dump of the page names, never committed)
    pub const SHOP: PageTag = PageTag::Page11;
    pub const TUNING: PageTag = PageTag::Page10;
    /// w3-clan: the clan screen's page.
    // OBSERVED: page table of the user's ROM (tag index 19 holds the clan screen's page)
    pub const CLAN: PageTag = PageTag::Page19;
    /// nickname-screen: the nickname screen's page (screen 0xe).
    // OBSERVED: page table of the user's ROM (tag index 12 holds the page the nickname screen's build loads by
    // name, cshell+0x12c0a1)
    pub const NAME_ENTRY: PageTag = PageTag::Page12;
}

/// A login password held for the login screen: its `Debug` form hides the text (CLI-178: never logged).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct LoginPassword(pub String);

impl std::fmt::Debug for LoginPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() {
            "\"\""
        } else {
            "<hidden>"
        })
    }
}

/// The options' volumes and switches in a sound director.
pub fn apply_sound(s: &crate::settings::Settings, d: &mut crate::sound::SoundDirector) -> bool {
    let mut a = d.settings.clone();
    a.effects_volume = s.effects_volume;
    a.music_volume = s.music_volume;
    a.effects_on = s.effects_on;
    a.music_on = s.music_on;
    d.apply_settings(a);
    // CLI-1599: the BGM style; a change is reported so the caller cues the screen again
    d.set_bgm_classic(s.bgm == crate::settings::BGM_CLASSIC)
}

/// Writes the settings into the profile text and saves it (the options' OK, the match's sensitivity keys).
pub(crate) fn store_settings(data: &mut MenuData, cat: &Catalog) {
    let mut doc = data.profile_doc.clone().unwrap_or_default();
    data.settings
        .to_doc(&mut doc, &data.commands, &|id| cat.text(id));
    if let Some(store) = &data.profile_store {
        if let Err(e) = store.save(&doc) {
            eprintln!("dc-client options: profile not saved: {e}");
        }
    }
    data.profile_doc = Some(doc);
}

/// CLI-1907: the empty screen under the options over a match (the match is drawn under the menu's list there).
#[derive(Default)]
pub struct MatchUnder {
    sheet: Sheet,
}

impl Screen for MatchUnder {
    fn name(&self) -> &'static str {
        "match"
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }
}

/// What the screens share.
#[derive(Debug, Default)]
pub struct MenuData {
    /// LG-313: the keys of the joined curated set: Verified by the lobby's reviewed list.
    pub curated_keys: Vec<String>,
    /// MOD-26: text a button asks to put on the clipboard, and that a button asks for its text
    /// ([`Menu::pump_clipboard`]).
    pub clipboard_out: Option<String>,
    pub clipboard_wanted: bool,
    pub auth: Option<SocketAddr>,
    /// Login ID shown in the ID box when the screen opens (`--name`).
    pub user: String,
    /// Text the login screen's password box starts with (`--login-password`, CLI-810; never logged).
    pub login_password: LoginPassword,
    /// CLI-1610: the login name of an unexpired cached identity token (the login screen offers it without asking
    /// for the password), and the authority's web port (registration and recovery go there).
    pub identity_login: Option<String>,
    pub identity_web: Option<SocketAddr>,
    pub servers: Vec<ServerEntry>,
    pub server: Option<ServerEntry>,
    pub profile: LobbyProfile,
    /// 0x311 map index / mode mask pairs.
    pub masks: Vec<(u32, u32)>,
    pub channels: Vec<ChannelRow>,
    pub rooms: Vec<RoomRow>,
    pub users: Vec<UserRow>,
    pub room: RoomView,
    pub members: BTreeMap<u32, RoomMember>,
    /// CLI-1899: when the waiting room's last invite (0x396) went out; the next waits [`room::INVITE_GAP_SECS`]
    /// (kept across rooms like 2009's shell-wide flag, cshell+0x38f20; cleared by a failed 0x397).
    pub invite_sent: Option<std::time::Instant>,
    pub chat: Vec<String>,
    /// Colours of the chat lines kept by [`MenuData::say_in`] (0 = the chat's own colour); used while it is as long
    /// as `chat`.
    pub chat_colour: Vec<u32>,
    /// The chat page's combos: where the next line goes ([`chatdrop::TARGET_CHANNEL`] when 0) and which lines show
    /// ([`chatdrop::FILTER_ALL`] when 0).
    pub chat_target: i32,
    pub chat_filter: i32,
    /// A lobby connection is up (the channel list of a lobby login arrived; cleared when it closes).
    pub lobby_up: bool,
    /// CLI-1628: the lobby logged in to offers wallhacks (modded policy and a declared `world.full_visibility` mod, the
    /// condition of the room's wallhacks bit, LG-274): only then the shop shows the x-ray block. Set where the lobby
    /// login completes, cleared with `lobby_up`.
    pub xray_offered: bool,
    /// LG-305: this entry offers Gun Game (our push 0x3f05 after the channel enter; cleared at every enter).
    pub gun_offered: bool,
    /// LG-305: the channel's Gun Game rooms (our push 0x3f07 after the room list and on changes).
    pub gun_rooms: std::collections::BTreeSet<u32>,
    /// The mode the room was created with (the game session's mode class comes from it).
    pub mode: i32,
    /// The last match's result while its result screen is up ([`crate::matchexit`]).
    pub result: Option<crate::matchexit::MatchResult>,
    /// The character's extra fields of the login (0x303) with the grade kept current by 0x3003.
    pub char_extras: link::CharExtras,
    /// The profile the Character Info box shows (set by a 0x379 reply, CLI-931).
    pub char_view: Option<link::ProfileView>,
    /// Credit and prepaid balance (0x345), for the Character Info panel.
    pub balance: (u32, u32),
    /// w3-options: the options' values in effect, the 2009 bindable command table, where the profile lives and the
    /// profile text read at start (kept so unknown lines survive a save).
    pub settings: crate::settings::Settings,
    pub commands: crate::settings::CommandTable,
    pub profile_store: Option<crate::settings::ProfileStore>,
    pub profile_doc: Option<crate::settings::ProfileDoc>,
    /// The options' OK changed the settings since [`Menu::take_settings_change`].
    pub settings_changed: bool,
    /// CLI-1843: a connection test the lobby chat started (`/nettest`): the probe generation then, and whether its
    /// result goes as a report; the menu's [`diagbox`] takes it.
    pub diag_chat_probe: Option<(u64, bool)>,
    /// CLI-1030: the sound page's values to hear now (each change of a sound row, and Cancel's restore); the next
    /// [`Menu::sound_frame`] hands them to the sound director.
    pub sound_live: Option<crate::settings::Settings>,
    /// Interface sound ids the screens asked for (the menu moves them to its sound queue after each call).
    pub sound_cues: Vec<i32>,
    /// When the last player report was sent (the report box refuses a new one for a while).
    pub last_report: Option<Instant>,
    /// w2-lobbychat: the members' sub-states (0x3042 +0xe: 1 shopping, 2 away) by in-channel id.
    pub member_sub: BTreeMap<u32, i32>,
    /// CLI-1887: the members' name and clan-name colour numbers (room record +0xac / +0xb0) by in-channel id.
    pub member_colour: BTreeMap<u32, (u32, u32)>,
    /// CLI-1895: the members' clans (room record +0x32 / +0x5e / +0xa4 / +0xa8) by in-channel id.
    pub member_clan: BTreeMap<u32, link::MemberClan>,
    /// w2-lobbychat: the chat mute end (local FILETIME ticks; 0x303 +0xec, 0x3053).
    pub muted_until: u64,
    /// w2-lobbychat: the chat's repeat rule ([`textbox::ChatGuard`]).
    pub chat_guard: textbox::ChatGuard,
    /// w2-lobbychat: the room list's notice bar ([`textbox::Ticker`]).
    pub ticker: textbox::Ticker,
    /// w2-lobbychat: the waiting room's idle rules (away flag, idle leave) run: on in a menu the player drives
    /// ([`Menu::new`]); scripted drivers turn them off ([`Menu::set_idle_rules`]).
    pub idle_rules: bool,
    /// The loading page's state while it is up ([`crate::loading`]).
    pub loading: Option<crate::loading::LoadingState>,
    /// w3-shop: the inventory, equipment, shop lists and inbox ([`storage::Store`]).
    pub store: storage::Store,
    /// w3-shop: where the shop and tuning screens' Exit returns (the lobby or the waiting room).
    pub shop_back: Go,
    /// w3-clan: the clan record, members, notice and clan chat of the clan replies.
    pub clan: clan::ClanState,
    /// w3-clan: the messenger relay's friend list and requests.
    pub friends: messenger::Friends,
    /// gm-client: the operator flag of the login and the operator requests waiting for their reply.
    pub gm: gm::GmState,
    /// CLI-1616: the lobby the client was started with (`--server`), the community list, the name of the server
    /// the browser picked and the run's own certificate pin (restored when the configured lobby is picked again).
    pub lobby: Option<SocketAddr>,
    pub community: modbrowser::CommunityState,
    pub picked_server: Option<String>,
    pub lobby_pin: Option<String>,
    /// CLI-1617: the profile folder the mods live under (`<data>/mods`); `None`: no profile folder.
    pub mods_dir: Option<PathBuf>,
    /// CLI-1908: the options are over a match: their Mods tab lists these mods (`None`: the lobby's options).
    pub match_mods: Option<optionsmods::MatchModsInfo>,
    /// LG-272: the other lobbies the login's lobby lists (push 0x3f02).
    pub other_lobbies: Vec<link::OtherLobby>,
    /// CLI-1622: the lobby of the list the player joined (its name) and where the player came from; the login screen
    /// logs in by itself once (`auto_login`, the identity token) and the server screen picks the joined lobby's first
    /// entry by itself once (`auto_pick`).
    pub modded_join: Option<String>,
    pub home: Option<moddedrows::Home>,
    pub auto_login: bool,
    pub auto_pick: bool,
    /// CLI-1617 (amended 2026-11-01): the server whose mods the Mods screen shows first (the row highlighted where
    /// it was opened; `None`: the current lobby), and the sets read at this run's joins by lobby address.
    pub mods_target: Option<modbrowser::ModsTarget>,
    /// CLI-1634: the optional mods' list of the login lobby and the download under way (a mod id).
    pub catalog: crate::modcatalog::CatalogState,
    pub catalog_busy: Option<String>,
    /// CLI-1634: why the last download was refused, shown under the Get mods header.
    pub catalog_note: Option<String>,
    /// CLI-1632: what is known of servers' sets, kept between runs.
    pub known_sets: Vec<modbrowser::KnownSet>,
    /// The login's lobby has said which other lobbies it lists (they may be read at all only after this).
    pub other_lobbies_read: bool,
}

impl MenuData {
    pub fn host(&self) -> bool {
        self.room.host_id == self.profile.my_id && self.profile.my_id != 0
    }

    /// Keeps the last chat lines (countdown, system lines, chat): [`textbox::CHAT_LINES`] of them.
    pub fn say(&mut self, line: String) {
        self.say_in(line, 0);
    }

    /// A chat line in its own colour (`0xAARRGGBB`; 0 = the chat's).
    pub fn say_in(&mut self, line: String, colour: u32) {
        if self.chat_colour.len() != self.chat.len() {
            self.chat_colour = vec![0; self.chat.len()];
        }
        self.chat.push(line);
        self.chat_colour.push(colour);
        let n = self.chat.len();
        let keep = textbox::CHAT_LINES;
        if n > keep {
            self.chat.drain(..n - keep);
            self.chat_colour.drain(..n - keep);
        }
    }

    /// The lines the chat list shows under the chat filter: the channel lines (all / channel only) or the clan
    /// chat's (clan only), each with its colour (0 = the chat's own).
    // DC-DEFAULT: CLI-871 (proposed) "All Chat Show" shows the channel lines (the clan chat's lines stay on the clan
    // screen), "Clan Chat Only" shows the clan chat's lines instead
    pub fn chat_view(&self) -> Vec<(String, u32)> {
        if self.chat_filter == chatdrop::FILTER_CLAN {
            self.clan
                .chat
                .iter()
                .map(|(n, t)| (format!("{n} : {t}"), 0))
                .collect()
        } else {
            (0..self.chat.len())
                .map(|i| (self.chat[i].clone(), self.chat_colour_of(i)))
                .collect()
        }
    }

    /// The colour of chat line `i` (0 = the chat's own).
    pub fn chat_colour_of(&self, i: usize) -> u32 {
        if self.chat_colour.len() == self.chat.len() {
            self.chat_colour.get(i).copied().unwrap_or(0)
        } else {
            0
        }
    }
}

/// Where to go next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Go {
    Login,
    /// nickname-screen: character creation after a login without a character.
    Nickname,
    Servers,
    Channels,
    #[default]
    Lobby,
    Create,
    Room,
    /// The result screen after a match.
    Result,
    /// The loading page between the game start and the match (interface state 3, [`crate::loading`]).
    Loading,
    /// w3-shop: the shop & storage screen on its weapons tab, on its black market tab, on its inbox page; the
    /// modification screen.
    Shop,
    Market,
    Inbox,
    Tuning,
    /// w3-options: the options screen (opened over the screen it came from by the top bar's option button), the
    /// player report box and the help box (both over the screen), and the way back from any of them.
    Options,
    Report,
    Help,
    /// The Character Info box (a profile reply opens it over the screen).
    CharInfo,
    Back,
    /// w3-clan: the clan screen.
    Clan,
    /// CLI-1616 / CLI-1617: the community server browser and the Mods window (our own, over the login screen).
    Community,
    Mods,
}

impl Go {
    /// The screens opened over another one (the one under them is kept and comes back with [`Go::Back`]).
    pub fn is_overlay(self) -> bool {
        matches!(
            self,
            Go::Options | Go::Report | Go::Help | Go::CharInfo | Go::Community | Go::Mods
        )
    }
}

/// What the screens may use while they handle an input or an event.
pub struct Ctx<'a> {
    pub cat: &'a Catalog,
    pub data: &'a mut MenuData,
    pub link: &'a Link,
    pub popups: &'a mut Vec<Popup>,
    pub go: &'a mut Option<Go>,
    pub quit: &'a mut bool,
    /// Interface sound ids the screen asks for ([`frontfx`] ids; the menu hands them to the sound director).
    pub sounds: &'a mut Vec<i32>,
}

impl Ctx<'_> {
    pub fn send(&self, c: LinkCmd) {
        self.link.send(c);
    }

    /// Plays interface sound `id` ([`frontfx`] ids).
    // EVIDENCE: cshell+0x1b6470 (the request plays the id at once)
    pub fn sound(&mut self, id: i32) {
        self.sounds.push(id);
    }

    /// An OK box with string `id`.
    pub fn notice(&mut self, id: i32) {
        let t = self.cat.text(id);
        self.popups.push(Popup::ok(t, BoxTag::None));
    }

    pub fn error(&mut self, id: i32, tag: BoxTag) {
        let t = self.cat.text(id);
        self.popups.push(Popup::error(t, tag));
    }

    // EVIDENCE: cshell+0x271a20 (a box without buttons opened while a box is shown is dropped)
    pub fn wait_box(&mut self, id: i32) {
        let t = self.cat.text(id);
        self.close_wait();
        if self.popups.is_empty() {
            self.popups.push(Popup::wait(t));
        }
    }

    /// Closes the wait boxes.
    pub fn close_wait(&mut self) {
        self.popups.retain(|p| p.kind != popup::BoxKind::Wait);
    }
}

/// One screen.
pub trait Screen {
    fn name(&self) -> &'static str;
    fn sheet(&self) -> &Sheet;
    fn sheet_mut(&mut self) -> &mut Sheet;
    fn enter(&mut self, _ctx: &mut Ctx) {}
    fn key(&mut self, _k: MenuKey, _ctx: &mut Ctx) {}
    /// A button with `command` was clicked (`part` = its index; `usize::MAX` for [`MenuInput::Command`]).
    fn click(&mut self, _cmd: i32, _part: usize, _ctx: &mut Ctx) {}
    fn check(&mut self, _cmd: i32, _part: usize, _on: bool, _ctx: &mut Ctx) {}
    /// A press the screen takes itself (list rows); true = taken.
    fn press(&mut self, _p: [f32; 2], _ctx: &mut Ctx) -> bool {
        false
    }
    /// The left button went up (anywhere: drags end).
    fn release(&mut self, _p: [f32; 2]) {}
    /// A modal window of the screen (the registration dialog) takes the pointer inputs `Move`, `Press` and
    /// `Release` before the screen's controls do; `true` = taken.
    fn modal_input(&mut self, _ev: &MenuInput, _ctx: &mut Ctx) -> bool {
        false
    }
    /// A press of the other mouse button.
    fn press_alt(&mut self, _p: [f32; 2], _ctx: &mut Ctx) {}
    /// Backspace before the focused edit box gets it; `true` = taken.
    fn backspace(&mut self, _ctx: &mut Ctx) -> bool {
        false
    }
    /// name-change: a typed character before the focused edit box gets it; `true` = taken.
    fn type_char(&mut self, _c: char, _ctx: &mut Ctx) -> bool {
        false
    }
    /// The keyboard focus moved to edit box `part` by a click.
    fn focused(&mut self, _part: usize, _ctx: &mut Ctx) {}
    /// The pointer moved (list row hover).
    fn hover(&mut self, _p: [f32; 2], _data: &MenuData) {}
    /// w2-lobbychat: any key, character or pointer input reached the screen (the waiting room's idle clock).
    fn input_seen(&mut self, _ctx: &mut Ctx) {}
    /// CLI-1310: the screen takes no pointer input now (the waiting room during the start countdown); keys and
    /// the wheel still reach it, and its commands are filtered by the screen's own `click`.
    fn input_locked(&self) -> bool {
        false
    }
    fn event(&mut self, _e: &LinkEvent, _ctx: &mut Ctx) {}
    /// CLI-1623 layout check: the frame of the screen's own window or prompt and its buttons (none: no such
    /// window).
    fn layout_rects(&self) -> Option<([i32; 4], Vec<[i32; 4]>)> {
        None
    }
    /// MOD-24: the sliders on the screen, `[x, y, w, h]` of each track.
    fn slider_rects(&self) -> Vec<[i32; 4]> {
        Vec::new()
    }
    /// MOD-26: what Ctrl+C copies when no box has the focus.
    fn copy_text(&self) -> Option<String> {
        None
    }
    /// MOD-26: the clipboard's whole text for a screen that reads it as one (a share code); `true` = taken.
    fn paste(&mut self, _text: &str, _ctx: &mut Ctx) -> bool {
        false
    }
    fn popup_done(&mut self, _p: &Popup, _yes: bool, _ctx: &mut Ctx) {}
    /// An overlay that draws the screen under it first (the report and help boxes).
    fn shows_under(&self) -> bool {
        false
    }
    /// A key by scan code for a screen waiting for one; true = taken.
    fn scan(&mut self, _scan: u8, _ctx: &mut Ctx) -> bool {
        false
    }
    /// The screen waits for a key by scan code (the window then sends [`MenuInput::Scan`] instead of the key).
    fn wants_scan(&self) -> bool {
        false
    }
    /// A command the screen gives itself now (timers), run as a click with that command.
    fn due(&mut self) -> Option<i32> {
        None
    }
    /// Tests: the screen's start watchdog clock moved back by `by` (CLI-1314).
    fn age_start_watch(&mut self, _by: std::time::Duration) {}
    /// The screen is the 2009 server-select screen, whose common page's window 0 controls play the navigation
    /// sounds instead of the select sounds.
    // EVIDENCE: cshell+0x1ae780 (common page built; window 0 and its controls get +0x58 = 1), called by the server
    // select build cshell+0x13a500 (and the character screens, not in DeCombat); cshell+0x1aeb90 / cshell+0x1af490
    // (+0x58 set: ids 2 / 1, else 0x14 / 0x13)
    fn common_nav(&self) -> bool {
        false
    }
    /// The open drop-down list ([`dropdown`]).
    fn dropdown(&self) -> Option<&dropdown::Dropdown> {
        None
    }
    fn dropdown_mut(&mut self) -> Option<&mut dropdown::Dropdown> {
        None
    }
    /// Row `index` (of the whole list) of the open drop-down list `list` was picked (`value` = the row's value);
    /// the list is closed already.
    fn dropdown_pick(&mut self, _list: &'static str, _value: i32, _index: usize, _ctx: &mut Ctx) {}
    /// Closes the open drop-down list.
    fn close_dropdown(&mut self) {}
    /// The scrolling layout lists the screen fills: (window, control id, rows, row height). The menu gives them their
    /// scroll bars ([`sheet::Sheet::set_list_rows`]); the screens' row geometry then follows the bars.
    fn list_rows(&self, _data: &MenuData) -> Vec<(i32, i32, usize, i32)> {
        Vec::new()
    }
    /// The menu clock reached `secs` before a frame is drawn: screens with timed slides move them; true = the
    /// picture changed.
    fn animate(&mut self, _secs: f32) -> bool {
        false
    }
    /// A slide is still running (the window keeps drawing).
    fn animating(&self) -> bool {
        false
    }
    /// A code-built field of the screen shows a blinking caret (the image is drawn again when its phase flips).
    fn caret_blinks(&self) -> bool {
        false
    }
    /// The lists got their rows (a bar may have moved since): screens that place their rows themselves follow here.
    fn lists_synced(&mut self) {}
    /// CLI-1634: the optional mods' list or the local mods changed.
    fn mods_changed(&mut self, _data: &MenuData, _report: crate::mods::JoinReport) {}
    /// The lists of [`Screen::list_rows`] (window, control id) that follow their newest row: when rows are added
    /// while the list shows its last row, the bar moves to the end (the chat lists).
    // EVIDENCE: cshell+0x2f18a0 (a row added: the bar's range grows; with the list's follow flag +0x244 set and the
    // last shown row +0x108 being the previous last row, the bar goes to its end through cshell+0x3457e0)
    fn list_follow(&self) -> Vec<(i32, i32)> {
        Vec::new()
    }
    /// The row count from which list (window, control id) of [`Screen::list_rows`] shows its scroll bar; with fewer
    /// rows the list has no bar (0: always, the default).
    fn list_bar_from(&self, _layer: i32, _id: i32) -> usize {
        0
    }
    /// A wheel turn the screen takes itself (lists it scrolls on its own); true = taken.
    fn wheel(&mut self, _p: [f32; 2], _notches: i32, _ctx: &mut Ctx) -> bool {
        false
    }
    /// use-items-emblem: a window of the screen is over its layout lists, so a press goes to the screen before a list's
    /// scroll bar can take it.
    fn over_lists(&self) -> bool {
        false
    }
    /// shop-model: the character model the screen shows ([`modelview`]); `None`: none.
    fn pane_figure(&self, _data: &MenuData) -> Option<modelview::ViewReq> {
        None
    }
    fn draw_extra(
        &self,
        _cat: &mut Catalog,
        _data: &MenuData,
        _secs: f32,
        _out: &mut Vec<DrawItem>,
    ) {
    }
    /// [`Screen::draw_extra`] for the draw trace: the screen also notes what its dynamic texts ask for
    /// ([`drawlist::DynWant`]); screens without notes draw as usual.
    fn draw_extra_noted(
        &self,
        cat: &mut Catalog,
        data: &MenuData,
        secs: f32,
        out: &mut Vec<DrawItem>,
        _notes: &mut drawlist::DynNotes,
    ) {
        self.draw_extra(cat, data, secs, out);
    }
}

/// How the menu ended.
pub enum MenuOutcome {
    /// The game server's address arrived: join it with the handed-over lobby connection.
    Play {
        server: SocketAddr,
        lobby: Box<Lobby>,
        mode: i32,
    },
    Quit,
}

/// Options of a menu run.
#[derive(Debug, Clone)]
pub struct MenuOptions {
    pub auth: SocketAddr,
    pub user: String,
    pub link: LinkOptions,
    pub rom: Option<PathBuf>,
    /// A font file for runs without a ROM (tests).
    pub font: Option<(Vec<u8>, usize)>,
    pub width: u32,
    pub height: u32,
}

/// A double click: the second press on the same list row within this time.
// DC-DEFAULT: CLI-169 (proposed) double click = second press on the same row within 500 ms
pub const DOUBLE_CLICK_MS: u128 = 500;

/// The longest the headless image waits for the character render.
const MODEL_VIEW_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// The front end.
pub struct Menu {
    pub cat: Catalog,
    pub data: MenuData,
    link: Link,
    screen: Box<dyn Screen>,
    /// The 2009 screen-state id of `screen` (CLI-1561: the focus messages of a screen change).
    screen_state: u32,
    popups: Vec<Popup>,
    /// CLI-1841: the diagnostic reports' boxes ([`diagbox`]).
    diag: diagbox::DiagUi,
    /// CLI-1863: the boxes of a newer client ([`updatebox`]).
    self_update: updatebox::UpdateUi,
    started: Instant,
    pressed: Option<usize>,
    outcome: Option<MenuOutcome>,
    quit: bool,
    /// Screen names in the order they were shown, and other milestones (tests, logs).
    pub trail: Vec<String>,
    dirty: bool,
    /// CLI-1596: the "Show FPS" text of the front end (`None`: off), top right of every screen.
    fps_text: Option<String>,
    /// The caret's blink phase of the last image (the screen is drawn again only when it flips).
    caret_shown: bool,
    /// CLI-1900: the bar button's blink phase of the last image (drawn again when it flips).
    bar_blink_shown: bool,
    /// The loading bar's update count of the last image ([`crate::loading::bar_updates`]).
    loading_shown: u32,
    /// The menu's textures decoded ahead ([`prefetch`]); taken into the catalogue every update.
    pub prefetch: Option<prefetch::Prefetch>,
    /// The background-music cue the last screen change gave, until [`Menu::take_music`] takes it.
    music_cue: Option<MusicCue>,
    /// lobby-nav: the screen fade that runs ([`frontfx`]), the literals it and the countdown use, and the interface
    /// sound ids asked for since [`Menu::take_sound_cues`].
    fade: Option<frontfx::Fade>,
    lits: frontfx::FrontLits,
    sounds: Vec<i32>,
    /// CRATE-7: the case reel's gun sounds (the won gun's own) since the last [`Menu::take_sound_cues`].
    reel_cues: Vec<crate::sound::SoundCue>,
    held_inputs: Vec<MenuInput>,
    /// The pointer (virtual pixels) of the last pointer input, [`cursor::DEFAULT_POINTER`] before any.
    pointer: [f32; 2],
    /// w3-options: the screen under the open overlay (options, report, help).
    under: Vec<Box<dyn Screen>>,
    /// The drawn cursor is shown (the window hides it while the pointer is outside or the window lost focus).
    pointer_shown: bool,
    /// The clickable control under the pointer (its change plays the select-change sound).
    hover_part: Option<usize>,
    /// The drop-down row the last press went to (picked when the release is on the same row).
    drop_pressed: Option<usize>,
    /// Lobby events queued by [`Menu::feed_event`], handled after the link's own at the next update.
    fed_events: Vec<LinkEvent>,
    /// w3-clan: the messenger window over the screen.
    messenger: messenger::Messenger,
    /// shop-model: the character view (created when a screen first asks for it; needs the ROM).
    figure: Option<modelview::ModelView>,
    /// CRATE-8: the case reel's won gun, rendered on its own view.
    showcase: Option<modelview::ModelView>,
    /// The next display list waits for the character render ([`Menu::image`]).
    figure_wait: bool,
    /// shop-model: the clip time the pane figure is posed at (`None`: the window's live clock, the headless image
    /// path frame 0).
    pub figure_time_ms: Option<f32>,
    /// The menu is drawn through [`Menu::image`] (headless): its figure keeps the fixed pose on every path.
    figure_still: bool,
}

/// The state id of the overlays that are no 2009 screen (no music cue is given for them).
pub const NOT_A_SCREEN: u32 = u32::MAX;

impl Go {
    /// The 2009 screen-state id of the screen (the ids the client's menu log shows).
    // OBSERVED: capture 20261025_202429_sbs4-real/driver/menu_steps.log (0x11 login, 0xb
    // servers, 0xc channels, 0xf lobby, 0x8 create, 0x9 room, 0x7 result)
    pub fn state_id(self) -> u32 {
        match self {
            Go::Login => 0x11,
            // EVIDENCE: cshell+0x42450 (0x201 reason 0x0003001e -> screen 0xe)
            Go::Nickname => 0xe,
            Go::Servers => 0xb,
            Go::Channels => 0xc,
            Go::Lobby => 0xf,
            Go::Create => 0x8,
            Go::Room => 0x9,
            Go::Result => 0x7,
            // the options screen's first page (graphic); the boxes are no screens of the 2009 table
            // EVIDENCE: cshell+0x1ac4b0 (command 0x12d -> screen 0x18)
            Go::Options => 0x18,
            Go::Report | Go::Help | Go::CharInfo | Go::Back | Go::Community | Go::Mods => {
                NOT_A_SCREEN
            }
            // not a screen of the screen table: the interface's loading state
            // EVIDENCE: cshell+0x1bb0b0 (interface state 3)
            Go::Loading => 0x3,
            // EVIDENCE: cshell+0x1bf20 (the bar's shop commands open screen 0x16, modification 0x14)
            Go::Shop | Go::Market | Go::Inbox => 0x16,
            Go::Tuning => 0x14,
            // EVIDENCE: cshell+0x1bf20 (command 0xa8 -> screen 0x1c)
            Go::Clan => 0x1c,
        }
    }
}

/// The lobby messages of a screen change from screen state `from` to `to` (2009 screen-state ids) that the 2009
/// screens send from their focus handlers: leaving the shop (0x16), modification (0x14) or clan (0x1c) screen ends
/// the private state (0x30c); entering the shop sends location 2 and private state 1, modification location 3 and
/// private state 3, clan private state 4; the waiting room entered from one of them sends location 1 and the
/// present flag 0x392 (1, 0), which ends the shopping state the room's bar command set (CLI-1560). The link sends
/// the private state messages only while the player is in a room.
// EVIDENCE: cshell+0x14f300 (shop focus: on -> 0x306 {2} at cshell+0x14f33e, in a room 0x30a {1} at cshell+0x14f361;
// off -> in a room 0x30c at cshell+0x14f78c), cshell+0x159a99 (modification: 0x306 {3} at cshell+0x159ab4, 0x30a {3}
// at cshell+0x159ad5, 0x30c at cshell+0x159d12), cshell+0x104960 (clan: 0x30a {4} at cshell+0x10498e, 0x30c at
// cshell+0x104c39), cshell+0x164020 (waiting room focus: 0x306 {1} at cshell+0x16408b; previous screen 0x14 / 0x16 /
// 0x1c -> 0x392 (1, 0) at cshell+0x16420b)
// DC-DEFAULT: CLI-1561 (proposed) a change between two screens of one state id (shop, black market, inbox: all 0x16)
// sends nothing (the 2009 tabs of one screen take no focus change)
pub fn focus_messages(from: u32, to: u32) -> Vec<LinkCmd> {
    const SHOP: u32 = 0x16;
    const TUNING: u32 = 0x14;
    const CLAN: u32 = 0x1c;
    const ROOM: u32 = 0x9;
    let mut out = Vec::new();
    if from == to {
        return out;
    }
    if matches!(from, SHOP | TUNING | CLAN) {
        out.push(LinkCmd::PrivateStateEnd);
    }
    match to {
        SHOP => {
            out.push(LinkCmd::Location(2));
            out.push(LinkCmd::PrivateState(1));
        }
        TUNING => {
            out.push(LinkCmd::Location(3));
            out.push(LinkCmd::PrivateState(3));
        }
        CLAN => out.push(LinkCmd::PrivateState(4)),
        ROOM if matches!(from, SHOP | TUNING | CLAN) => {
            out.push(LinkCmd::Location(1));
            out.push(LinkCmd::Shopping(false));
        }
        _ => {}
    }
    out
}

impl Menu {
    pub fn new(opts: MenuOptions) -> Menu {
        let cat = opts
            .rom
            .as_deref()
            .and_then(|r| Catalog::from_rom(r, opts.width, opts.height).ok())
            .unwrap_or_else(|| Catalog::fallback(opts.font.clone(), opts.width, opts.height));
        // CLI-1616: the community list is read on the link's thread (cached for an hour in the profile folder)
        let community_url = crate::community::url();
        let data = MenuData {
            auth: Some(opts.auth),
            user: opts.user.clone(),
            idle_rules: true,
            lobby: Some(opts.auth),
            lobby_pin: dc_botclient::tls::current().pin,
            mods_dir: opts.link.mods.data_dir.clone(),
            known_sets: opts
                .link
                .mods
                .data_dir
                .as_deref()
                .map(|d| crate::knownsets::load(d, std::time::SystemTime::now()))
                .unwrap_or_default(),
            community: if community_url.is_some() {
                modbrowser::CommunityState::Loading
            } else {
                modbrowser::CommunityState::Off
            },
            ..MenuData::default()
        };
        let prefetch = cat
            .rom
            .clone()
            .and_then(|rom| prefetch::Prefetch::start(&rom, prefetch::menu_textures(&cat)));
        // CLI-1480: without the ROM's nickname page a login without a character makes one at once (CLI-162)
        let mut link_opts = opts.link.clone();
        if cat.rom.is_none() {
            link_opts.auto_character = true;
        }
        let cache = link_opts
            .mods
            .data_dir
            .as_ref()
            .map(|d| d.join(crate::community::CACHE_FILE));
        let link = Link::start(link_opts);
        if let Some(url) = community_url {
            link.send(LinkCmd::Community {
                url: Some(url),
                cache,
            });
        }
        let screen: Box<dyn Screen> = Box::new(login::Login::new(&cat, &data));
        let lits = opts
            .rom
            .as_deref()
            .map(frontfx::FrontLits::from_rom)
            .unwrap_or_default();
        let mut m = Menu::assemble(cat, data, link, screen, prefetch, lits);
        m.with_ctx(|s, ctx| s.enter(ctx));
        m
    }

    /// The menu's state around `screen` (the first screen, entered by the caller).
    fn assemble(
        cat: Catalog,
        data: MenuData,
        link: Link,
        screen: Box<dyn Screen>,
        prefetch: Option<prefetch::Prefetch>,
        lits: frontfx::FrontLits,
    ) -> Menu {
        let mut m = Menu {
            cat,
            data,
            link,
            screen,
            popups: Vec::new(),
            diag: diagbox::DiagUi::default(),
            self_update: updatebox::UpdateUi::default(),
            started: Instant::now(),
            pressed: None,
            outcome: None,
            quit: false,
            trail: Vec::new(),
            dirty: true,
            fps_text: None,
            caret_shown: false,
            bar_blink_shown: false,
            loading_shown: 0,
            prefetch,
            screen_state: Go::Login.state_id(),
            music_cue: cue_for_screen_state(Go::Login.state_id()),
            fade: None,
            lits,
            sounds: Vec::new(),
            reel_cues: Vec::new(),
            held_inputs: Vec::new(),
            pointer: cursor::DEFAULT_POINTER,
            pointer_shown: true,
            under: Vec::new(),
            hover_part: None,
            drop_pressed: None,
            fed_events: Vec::new(),
            messenger: messenger::Messenger::default(),
            figure: None,
            showcase: None,
            figure_wait: false,
            figure_time_ms: None,
            figure_still: false,
        };
        m.trail.push(m.screen.name().into());
        m
    }

    /// CLI-1907: the options over a match (the exit box's Options button): a menu without a lobby connection whose
    /// only screen is the options box over an empty screen (the match shows through the frame's blind); `rom`,
    /// `font`, `width` x `height` as for [`Menu::new`]; `settings` and `commands`: the menu's settings and key table
    /// at the match start; `mods`: what the Mods tab lists (CLI-1908). OK publishes and applies the settings
    /// ([`Menu::take_settings_change`]); closing the box leaves [`Menu::over_match_open`] false.
    // DC-DEFAULT: CLI-1907 (proposed) the lobby's options box, unchanged, over the match
    pub fn over_match(
        rom: Option<&std::path::Path>,
        font: Option<(Vec<u8>, usize)>,
        (width, height): (u32, u32),
        settings: crate::settings::Settings,
        commands: crate::settings::CommandTable,
        mods: optionsmods::MatchModsInfo,
    ) -> Menu {
        let cat = rom
            .and_then(|r| Catalog::from_rom(r, width, height).ok())
            .unwrap_or_else(|| Catalog::fallback(font, width, height));
        let lits = rom.map(frontfx::FrontLits::from_rom).unwrap_or_default();
        let data = MenuData {
            settings,
            commands,
            ..MenuData::default()
        };
        let mut m = Menu::assemble(cat, data, Link::detached(), Box::new(MatchUnder::default()), None, lits);
        m.music_cue = None;
        m.screen_state = NOT_A_SCREEN;
        m.pointer_shown = false;
        m.reopen_over_match(None, mods);
        m
    }

    /// CLI-1907: the options over the match again (a later Options click in the same match): the settings in effect
    /// now (`None`: the ones the box last kept) and the Mods tab's list.
    pub fn reopen_over_match(
        &mut self,
        settings: Option<crate::settings::Settings>,
        mods: optionsmods::MatchModsInfo,
    ) {
        if let Some(s) = settings {
            self.data.settings = s;
        }
        self.data.settings_changed = false;
        self.data.sound_live = None;
        self.data.match_mods = Some(mods);
        self.popups.clear();
        self.under.clear();
        self.screen = Box::new(MatchUnder::default());
        self.pressed = None;
        self.drop_pressed = None;
        self.hover_part = None;
        self.overlay(Go::Options);
        self.dirty = true;
    }

    /// CLI-1907: the options over the match are still up (Cancel, OK, the close button or Escape take them down).
    pub fn over_match_open(&self) -> bool {
        self.screen.name() == options::NAME
    }

    /// CLI-1907: the settings the options over the match changed become the menu's and are saved (right after the
    /// match, also when the player quit from it, and with every update).
    pub fn store_match_settings(&mut self) {
        let Some(s) = crate::settings::take_match_settings_note() else {
            return;
        };
        if s != self.data.settings {
            self.data.settings = s;
            self.data.settings_changed = true;
            crate::settings::publish(&self.data.settings);
            store_settings(&mut self.data, &self.cat);
        }
    }

    /// CLI-1610: the identity authority's web port and the login name of a cached token (main sets both at start).
    pub fn set_identity(&mut self, web: Option<SocketAddr>, cached_login: Option<String>) {
        self.data.identity_web = web;
        self.data.identity_login = cached_login;
        if self.screen.name() == "login" {
            self.with_ctx(|s, ctx| s.enter(ctx));
        }
    }

    /// `--login-password` (CLI-810): the login screen's password box starts with `pw` (by-hand menu runs; the
    /// pilot types its plan's password itself). The text is never logged (CLI-178).
    // DC-DEFAULT: CLI-810 (proposed) --login-password fills the login password box
    pub fn set_login_password(&mut self, pw: &str) {
        self.data.login_password = LoginPassword(pw.to_string());
        if self.screen.name() == "login" {
            self.with_ctx(|s, ctx| s.enter(ctx));
        }
    }

    /// CLI-1623 layout check: the current screen's window or prompt frame and its buttons.
    pub fn layout_rects(&self) -> Option<([i32; 4], Vec<[i32; 4]>)> {
        self.screen.layout_rects()
    }

    /// MOD-24: the sliders on the screen, `[x, y, w, h]` of each track.
    pub fn slider_rects(&self) -> Vec<[i32; 4]> {
        self.screen.slider_rects()
    }

    /// MOD-26: a Copy or Paste button pressed since the last call reaches the clipboard (the window calls this after
    /// each input; a screen has no clipboard of its own).
    pub fn pump_clipboard(&mut self, clip: &mut dyn clipboard::Clipboard) {
        if let Some(t) = self.data.clipboard_out.take() {
            clip.set(&t);
        }
        if std::mem::take(&mut self.data.clipboard_wanted) {
            if let Some(t) = clip.get() {
                self.input(MenuInput::Paste(t));
            }
        }
    }

    pub fn screen_name(&self) -> &'static str {
        self.screen.name()
    }

    pub fn sheet(&self) -> &Sheet {
        self.screen.sheet()
    }

    pub fn popup(&self) -> Option<&Popup> {
        self.popups.first()
    }

    /// crate-result-window: runs the shown box's timers at `now` (the award window's reveal).
    pub fn tick_popup_at(&mut self, now: Instant) {
        if self.popups.first_mut().is_some_and(|p| p.tick(now)) {
            self.dirty = true;
        }
        // CRATE-5: the reel's ticks
        if let Some(p) = self.popups.first_mut() {
            for s in p.take_sounds() {
                match s {
                    reel::Sound::Rom(id) => self.sounds.push(id),
                    reel::Sound::Weapon { k, weapon } => self
                        .reel_cues
                        .push(crate::sound::SoundCue::OwnWeapon { k, weapon }),
                }
            }
        }
    }

    /// The boxes open now (the first one is shown, the others wait behind it).
    pub fn popup_count(&self) -> usize {
        self.popups.len()
    }

    pub fn source(&self) -> &str {
        &self.cat.source
    }

    /// CLI-1897: the catalog's mode lookups follow the joined server entry (clan-only: the clan mode rows).
    fn sync_clan_table(&mut self) {
        let clan = self.data.server.as_ref().is_some_and(|e| e.clan_only != 0);
        self.cat.set_clan_table(clan);
    }

    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut dyn Screen, &mut Ctx) -> R) -> R {
        self.sync_clan_table();
        let mut go = None;
        let mut quit = false;
        let r = {
            let mut ctx = Ctx {
                cat: &self.cat,
                data: &mut self.data,
                link: &self.link,
                popups: &mut self.popups,
                go: &mut go,
                quit: &mut quit,
                sounds: &mut self.sounds,
            };
            f(self.screen.as_mut(), &mut ctx)
        };
        if quit {
            self.quit = true;
            self.outcome.get_or_insert(MenuOutcome::Quit);
        }
        self.sounds.append(&mut self.data.sound_cues);
        if let Some(g) = go {
            self.go(g);
        }
        // w3-clan: the lobby bar's messenger button asked for the window
        if std::mem::take(&mut self.data.friends.toggle_asked) {
            self.toggle_messenger();
        }
        self.dirty = true;
        r
    }

    /// w3-clan: opens or closes the messenger window over the current screen and sends the place (0x306).
    pub fn toggle_messenger(&mut self) {
        let here = self.screen_go();
        let place = self.messenger.toggle(&self.cat, here);
        self.link.send(LinkCmd::Location(place));
        // CLI-1900: opening the window takes the waiting notices (the bar button stops blinking)
        // EVIDENCE: cshell+0x12b350 (shown -> pending +0x5534 = 0), cshell+0x1bf20 (0x136: the button's period = 0)
        self.data.friends.shown = self.messenger.open;
        if self.messenger.open {
            self.data.friends.unread = 0;
        }
        // the relay's friend list and open requests are asked for each time the window opens
        // DC-DEFAULT: CLI-565 (proposed) opening the window sends the relay's `!nm list`
        if self.messenger.open {
            self.link
                .send(LinkCmd::Messenger(messenger::LIST_COMMAND.into()));
        }
        self.trail.push(if self.messenger.open {
            "messenger".into()
        } else {
            "messenger closed".into()
        });
        self.dirty = true;
    }

    /// CLI-1900: the bar's messenger button blinks on every page that carries it (the current screen and the one
    /// under an overlay) while notices wait and the window is closed.
    // EVIDENCE: cshell+0x1bb70 (each screen focus: pending -> the common page's window 1 control 2 gets the period),
    // cshell+0x13af50 (the same control: the bar's messenger button, command 0x136)
    fn sync_bar_blink(&mut self) {
        let period = if self.data.friends.unread > 0 && !self.messenger.open {
            messenger::BUTTON_BLINK
        } else {
            0.0
        };
        let sheets = std::iter::once(self.screen.sheet_mut())
            .chain(self.under.iter_mut().map(|u| u.sheet_mut()));
        for sheet in sheets {
            for p in sheet
                .parts
                .iter_mut()
                .filter(|p| p.command == messenger::BAR_FRIENDS_CMD && p.kind == WidgetKind::Button)
            {
                p.blink = period;
            }
        }
    }

    /// The messenger window is open.
    pub fn messenger_open(&self) -> bool {
        self.messenger.open
    }

    /// The messenger window (tests, tools).
    pub fn messenger(&self) -> &messenger::Messenger {
        &self.messenger
    }

    /// The pointer is over the bar's messenger button of the current screen.
    fn messenger_button_at(&self, p: [f32; 2]) -> bool {
        if !BAR_SCREENS.contains(&self.base_screen_name()) {
            return false;
        }
        let sheet = self.screen.sheet();
        sheet.hit(p).is_some_and(|i| {
            sheet.parts[i].command == messenger::BAR_FRIENDS_CMD && sheet.parts[i].enabled
        })
    }

    /// Something a dismiss closes: the open box, the messenger window or a screen opened from a lobby bar button
    /// (the step runner's `dismiss` sends Escape then).
    pub fn dismissable(&self) -> bool {
        !self.popups.is_empty() || self.messenger.open || self.screen.name() == "clan"
    }

    /// Sends a request on the lobby connection (tests and tools; the screens send through their context).
    pub fn send_link(&self, c: LinkCmd) {
        self.link.send(c);
    }

    /// The screen that is up, as a [`Go`].
    fn screen_go(&self) -> Go {
        match self.screen.name() {
            "login" => Go::Login,
            "servers" => Go::Servers,
            "channels" => Go::Channels,
            "create" => Go::Create,
            "room" => Go::Room,
            "result" => Go::Result,
            "clan" => Go::Clan,
            _ => Go::Lobby,
        }
    }

    /// A screen's change: through the black fade where the 2009 client fades ([`frontfx::fades`]), else at once.
    fn go(&mut self, g: Go) {
        if g.is_overlay() || g == Go::Back {
            self.overlay(g);
            return;
        }
        if frontfx::fades(self.screen.name(), g) {
            self.fade = Some(frontfx::Fade::new(g));
            self.trail.push("fade".into());
        } else {
            self.switch(g);
        }
    }

    /// The fade that runs: its target and the black's alpha now.
    pub fn fading(&self) -> Option<(Go, u8)> {
        self.fade
            .map(|f| (f.target, frontfx::fade_alpha(f.age_ms(), f.fading_in)))
    }

    /// The interface sounds asked for since the last call, as sound cues (the file from the ROM's interface section,
    /// `None` without one). The window hands them to [`crate::sound::SoundDirector`].
    // EVIDENCE: cshell+0x1b20c0 (a key without a file plays nothing)
    pub fn take_sound_cues(&mut self) -> Vec<crate::sound::SoundCue> {
        let rom = self.lits.rom;
        let mut cues: Vec<crate::sound::SoundCue> = std::mem::take(&mut self.sounds)
            .into_iter()
            .map(|id| (self.lits.sounds.get(&id).cloned(), id))
            .filter(|(file, _)| file.is_some() || !rom)
            .map(|(file, id)| crate::sound::SoundCue::Interface {
                file,
                what: frontfx::cue_name(id),
            })
            .collect();
        cues.append(&mut self.reel_cues);
        cues
    }

    /// One menu frame's sound hand-over (the window's per-frame hook, also run by the tests): the screen's music cue
    /// if it changed, the interface sounds asked for since the last frame (clicks, hover changes, countdown,
    /// result) played through `sound`, then one music step. Without a director (`--no-sound`) the cues are dropped.
    // EVIDENCE: cshell+0x1b20c0 (the interface play helper; the front end has no listener: local voices)
    pub fn sound_frame(&mut self, sound: Option<&mut crate::sound::SoundDirector>) {
        let mut cue = self.take_music();
        let cues = self.take_sound_cues();
        let live = self.data.sound_live.take();
        if let Some(s) = sound {
            if let Some(l) = &live {
                // CLI-1599: a BGM style heard while editing (or restored by Cancel) cues the screen again: the
                // playing track crossfades to the other style's
                if apply_sound(l, s) && cue.is_none() {
                    cue = cue_for_screen_state(self.screen_state);
                }
            }
            if let Some(c) = cue {
                crate::events::music_cue(&format!("{c:?}"));
                s.music_cue(c);
            }
            s.menu_cues(&cues);
            s.music_tick();
            if crate::events::on() {
                crate::events::sound_poll(&s.mixer());
            }
        } else if let Some(c) = cue {
            crate::events::music_cue(&format!("{c:?}"));
        }
    }

    /// The interface sound a click (`select`) or a hover change (not `select`) on control `i` plays.
    fn control_sound(&self, i: usize, select: bool) -> i32 {
        let p = &self.screen.sheet().parts[i];
        let nav = self.screen.common_nav() && p.page == pages::COMMON && p.layer == 0;
        match (nav, select) {
            (true, true) => frontfx::SOUND_NAV_SELECT,
            (true, false) => frontfx::SOUND_NAV_CHANGE,
            (false, true) => frontfx::SOUND_SELECT,
            (false, false) => frontfx::SOUND_SELECT_CHANGE,
        }
    }

    /// The open drop-down list of the screen (tests, tools).
    pub fn dropdown(&self) -> Option<&dropdown::Dropdown> {
        self.screen.dropdown()
    }

    /// The interface sound ids asked for since the last [`Menu::take_sound_cues`] (tests).
    pub fn pending_sounds(&self) -> &[i32] {
        &self.sounds
    }

    /// w3-options: opens an overlay over the current screen (the same one again closes it, as the help button
    /// toggles its box) or, with [`Go::Back`], closes it.
    // EVIDENCE: cshell+0x1ac4b0 (option button: screen 0x18 pushed over the current one; help: the box toggled;
    // report: the report box opened over the screen), cshell+0x1149a0 (the options' OK / cancel: back to the screen
    // the options were opened from)
    // DC-DEFAULT: CLI-560 (proposed) the screen under an overlay is kept as it is (not built again) and gets no
    // lobby events while covered (the shared lists still follow them); one overlay at a time
    fn overlay(&mut self, g: Go) {
        self.pressed = None;
        if g == Go::Back || self.screen.name() == overlay_name(g) {
            if let Some(s) = self.under.pop() {
                self.screen = s;
                self.trail.push(self.screen.name().into());
            }
            return;
        }
        let fresh: Box<dyn Screen> = match g {
            Go::Options => Box::new(options::Options::new(&self.cat, &self.data)),
            Go::Report => Box::new(report::Report::new(&self.cat, &self.data)),
            Go::CharInfo => Box::new(charinfo::CharInfo::new(&self.cat, &self.data)),
            Go::Community => Box::new(modbrowser::Community::new(&self.cat, &self.data)),
            Go::Mods => Box::new(modbrowser::ModsScreen::new(&self.cat, &self.data)),
            _ => Box::new(help::Help::new(&self.cat, &self.data)),
        };
        if self.under.is_empty() {
            let old = std::mem::replace(&mut self.screen, fresh);
            self.under.push(old);
        } else {
            self.screen = fresh;
        }
        self.trail.push(self.screen.name().into());
        self.with_ctx(|s, ctx| s.enter(ctx));
    }

    /// The current screen waits for a key by scan code (the options' key boxes).
    pub fn wants_scan(&self) -> bool {
        self.popups.is_empty() && self.screen.wants_scan()
    }

    /// An overlay (options, report or help) is open over a screen.
    pub fn overlay_open(&self) -> bool {
        !self.under.is_empty()
    }

    /// The name of the screen under the open overlay (the current screen's without one).
    pub fn base_screen_name(&self) -> &'static str {
        self.under.first().map_or(self.screen.name(), |s| s.name())
    }

    /// The top bar's buttons every front-end screen after the login shares: option, report, help. They open their
    /// overlay with the interface click sound.
    // EVIDENCE: cshell+0x1ac4b0 (the screens' shared command handler: 0x12d option, 0x12e help, 0x273 report),
    // cshell+0x1aeb90 (a button's mouse-up asks for interface sound 0x14 or 2 by the control's flag)
    // DC-DEFAULT: CLI-559 (proposed) these buttons sound as every control does (the generic mouse-up cue)
    fn top_bar(&mut self, cmd: i32) -> bool {
        // the messenger button of the bottom bar (every page that carries it)
        // EVIDENCE: cshell+0x1bf20 (0x136 is the common handler's: any screen with the bar, not the lobby's own)
        if cmd == messenger::BAR_FRIENDS_CMD {
            if !BAR_SCREENS.contains(&self.base_screen_name()) {
                return false;
            }
            self.toggle_messenger();
            return true;
        }
        // CLI-1316: the bar's top-right X asks the quit question on every page that carries the bar (the waiting
        // room and the create page had no handler: the X a match handed back to was dead until Escape left the room)
        // DISASM: cshell+0x1ac4b0 (the shared handler: 0x12f -> screen 0x10, the quit question, from any screen)
        if cmd == shop::CMD_BAR_QUIT {
            if !TOP_BAR_SCREENS.contains(&self.base_screen_name()) {
                return false;
            }
            let t = self.cat.text(text_ids::ASK_QUIT);
            self.popups.push(Popup::ask(t, BoxTag::Quit));
            self.dirty = true;
            return true;
        }
        let g = match cmd {
            CMD_BAR_PREFS => Go::Options,
            CMD_BAR_ASSIST => Go::Help,
            CMD_BAR_ABUSE => Go::Report,
            _ => return false,
        };
        if !TOP_BAR_SCREENS.contains(&self.base_screen_name()) {
            return false;
        }
        // the click cue of a pressed button is the window's own (see Release above)
        self.dirty = true;
        // a report sent less than ten seconds ago: a notice instead of the box
        // EVIDENCE: cshell+0x273b70 (opening within 0x2710 ms of the last send shows the notice)
        if g == Go::Report
            && self
                .data
                .last_report
                .is_some_and(|t| t.elapsed().as_millis() <= report::RESEND_MS)
        {
            let t = report::resend_notice(&self.cat);
            self.popups.push(Popup::error(t, BoxTag::None));
            return true;
        }
        self.overlay(g);
        true
    }

    fn switch(&mut self, g: Go) {
        if g.is_overlay() || g == Go::Back {
            self.overlay(g);
            return;
        }
        self.pressed = None;
        self.under.clear();
        self.hover_part = None;
        self.drop_pressed = None;
        self.sync_clan_table();
        // MAP-32: the room list opens on the joined server's community maps and its mod files
        if g == Go::Lobby {
            self.sync_mods();
        }
        self.screen = match g {
            Go::Login => Box::new(login::Login::new(&self.cat, &self.data)),
            Go::Nickname => Box::new(nickname::Nickname::new(&self.cat, &self.data)),
            Go::Servers => Box::new(servers::Servers::new(&self.cat, &self.data)),
            Go::Channels => Box::new(channels::Channels::new(&self.cat, &self.data)),
            Go::Lobby => {
                // the login's character fields and balance came before this screen existed; live ones reach it as
                // events
                let mut s = lobby::LobbyScreen::new(&self.cat, &self.data);
                s.apply_char_data(
                    &self.cat,
                    &self.data,
                    &self.data.char_extras,
                    self.data.balance,
                );
                Box::new(s)
            }
            Go::Create => Box::new(create::Create::new(&self.cat, &self.data)),
            Go::Room => Box::new(room::Room::new(&self.cat, &self.data)),
            Go::Result => Box::new(result::ResultScreen::new(&self.cat, &self.data)),
            Go::Options
            | Go::Report
            | Go::Help
            | Go::CharInfo
            | Go::Back
            | Go::Community
            | Go::Mods => {
                unreachable!("overlays open above")
            }
            Go::Loading => Box::new(crate::loading::LoadingScreen::new(&self.cat, &self.data)),
            Go::Shop | Go::Market | Go::Inbox => {
                Box::new(shop::ShopScreen::new(&self.cat, &self.data, g))
            }
            Go::Tuning => Box::new(tuning::TuningScreen::new(&self.cat, &self.data)),
            Go::Clan => Box::new(clan::ClanScreen::new(&self.cat, &self.data)),
        };
        self.trail.push(self.screen.name().into());
        let from = std::mem::replace(&mut self.screen_state, g.state_id());
        for c in focus_messages(from, g.state_id()) {
            self.link.send(c);
        }
        if g != Go::Loading {
            self.data.loading = None;
        }
        // the screen-state music rule (state 7 keeps what plays: no new cue); the loading page is no screen of that
        // rule: the game start cues the loading track itself (`menu::window`)
        // EVIDENCE: cshell+0x1fba00 (state 0 stop, 7 nothing, 9 waiting room, others front end)
        // EVIDENCE: cshell+0x1630a0 (the game start plays the loading track)
        let cue = (g != Go::Loading)
            .then(|| cue_for_screen_state(g.state_id()))
            .flatten();
        if let Some(c) = cue {
            self.music_cue = Some(c);
        }
        self.with_ctx(|s, ctx| s.enter(ctx));
    }

    /// The settings the options' OK changed since the last call (the window applies the volumes at once and the
    /// rest to the next match).
    pub fn take_settings_change(&mut self) -> Option<crate::settings::Settings> {
        std::mem::take(&mut self.data.settings_changed).then(|| self.data.settings.clone())
    }

    /// Reads the player's settings from `store` (the bindable command table from the ROM when there is one) and
    /// makes them the ones in effect; returns the file they came from.
    pub fn load_settings(&mut self, store: crate::settings::ProfileStore) -> Option<PathBuf> {
        if let Some(rom) = self.cat.rom.clone() {
            let cat = &self.cat;
            let strings = |id: i32| id > 0 && !cat.text(id).is_empty();
            self.data.commands = crate::settings::CommandTable::from_rom(&rom, &strings);
        }
        let loaded = store.load();
        let from = loaded.as_ref().map(|(_, p)| p.clone());
        if let Some((doc, _)) = loaded {
            let cat = &self.cat;
            self.data.settings =
                crate::settings::Settings::from_doc(&doc, &self.data.commands, &|id| cat.text(id));
            self.data.profile_doc = Some(doc);
        }
        self.data.profile_store = Some(store);
        crate::settings::publish(&self.data.settings);
        // CLI-1617 / CLI-1618: the player's mod preferences (this turns the consent pause on)
        self.link.send(modbrowser::prefs_cmd(&self.data));
        from
    }

    /// The music cue of the last screen change (the window or pilot hands it to
    /// [`crate::sound::SoundDirector::music_cue`]).
    pub fn take_music(&mut self) -> Option<MusicCue> {
        self.music_cue.take()
    }

    /// CLI-1599: cues the current screen again (the BGM style changed under it).
    pub fn recue_music(&mut self) {
        if self.music_cue.is_none() {
            self.music_cue = cue_for_screen_state(self.screen_state);
        }
    }

    /// Opens screen `g` (tools and tests that show a screen with prepared data).
    pub fn open(&mut self, g: Go) {
        self.popups.clear();
        self.switch(g);
    }

    /// One input.
    pub fn input(&mut self, ev: MenuInput) {
        // a paste is the same as typing its characters one by one (each box applies its own length cap)
        if let MenuInput::Paste(text) = ev {
            if self.popups.is_empty() && self.with_ctx(|s, ctx| s.paste(&text, ctx)) {
                return;
            }
            for c in text.chars().filter(|c| !c.is_control()) {
                self.input(MenuInput::Char(c));
            }
            return;
        }
        // the drawn cursor follows every pointer input, also while a fade or a box holds the input
        if let MenuInput::Move(p)
        | MenuInput::Press(p)
        | MenuInput::Release(p)
        | MenuInput::PressAlt(p)
        | MenuInput::Wheel(p, _) = ev
        {
            if p != self.pointer {
                self.pointer = p;
                self.dirty = true;
            }
        }
        if self.outcome.is_some() {
            return;
        }
        // the 2009 lobby takes no command while its fade runs
        // EVIDENCE: cshell+0x129e00 (returns at once while the fade flag is set)
        // DC-DEFAULT: CLI-470 (staged) menu inputs given in a fade's second half (new screen up) are kept (up to 64)
        // and run when it ends; those of the first half are dropped at the screen switch (see update)
        if self.fade.is_some() {
            if self.held_inputs.len() < 64 {
                self.held_inputs.push(ev);
            }
            return;
        }
        self.dirty = true;
        self.sync_lists();
        if !matches!(ev, MenuInput::Command(_)) {
            self.with_ctx(|s, ctx| s.input_seen(ctx));
        }
        if let Some(top) = self.popups.first_mut() {
            if let Some(yes) = top.input(&ev) {
                // a box's button is a control of its window: the click plays the select sound
                // EVIDENCE: cshell+0x1aeb90
                if matches!(ev, MenuInput::Release(_)) {
                    self.sounds.push(frontfx::SOUND_SELECT);
                }
                let p = self.popups.remove(0);
                match (p.tag, yes) {
                    (BoxTag::Quit, true) => {
                        self.link.send(LinkCmd::Logout);
                        self.quit = true;
                        self.outcome = Some(MenuOutcome::Quit);
                    }
                    (BoxTag::ToLogin, _) => self.switch(Go::Login),
                    // lobby20: the messenger's boxes (any screen)
                    // EVIDENCE: cshell+0xf07c0 (0xdf: the typed name goes to the add request), cshell+0xed4b0 (a
                    // friend request asks 0x5ac, Yes / No)
                    (BoxTag::AddFriend, true) => {
                        let n = p.input.trim().to_string();
                        if !n.is_empty() {
                            self.link
                                .send(LinkCmd::Messenger(messenger::add_command(&n)));
                        }
                    }
                    (BoxTag::RemoveFriend, true) => self
                        .link
                        .send(LinkCmd::Messenger(messenger::remove_command(&p.input))),
                    // CLI-1898: Yes (a click) accepts, No declines and the row goes; Escape only closes the box and
                    // the request stays open (a row of the messenger window)
                    // DC-DEFAULT: CLI-1898 (proposed; amends CLI-842)
                    (BoxTag::FriendRequest, _) => match p.chosen {
                        Some(0) => self
                            .link
                            .send(LinkCmd::Messenger(messenger::accept_command(&p.input))),
                        Some(_) => {
                            self.link
                                .send(LinkCmd::Messenger(messenger::decline_command(&p.input)));
                            self.data.friends.requests.retain(|r| *r != p.input);
                        }
                        None => {}
                    },
                    // CLI-1901: a click on Yes joins the inviting clan, No declines; Escape closes the box and the
                    // invitation stays open (the lobby asks again at the next login)
                    // DC-DEFAULT: CLI-1901 (proposed)
                    (BoxTag::ClanInvite(uid), _) => match p.chosen {
                        Some(0) => self
                            .link
                            .send(LinkCmd::Messenger(messenger::clan_yes_command(uid))),
                        Some(_) => self
                            .link
                            .send(LinkCmd::Messenger(messenger::clan_no_command(uid))),
                        None => {}
                    },
                    (BoxTag::AddFriend | BoxTag::RemoveFriend, false) => {}
                    (BoxTag::Diag(step), _) => {
                        let three = diagbox::is_three(&p);
                        diagbox::answer(self, step, p.chosen, three);
                    }
                    (BoxTag::Update(step), _) => {
                        let caption = updatebox::chosen_caption(&p);
                        updatebox::answer(self, step, caption.as_deref());
                    }
                    _ => self.with_ctx(|s, ctx| s.popup_done(&p, yes, ctx)),
                }
            }
            return;
        }
        // CLI-1310: the waiting room's start lock drops every press, release and move before any control, the
        // messenger and the bar see it; a command goes to the screen only (its click filters it), not to the bar
        // DISASM: cshell+0x15e970 / cshell+0x15ec10 / cshell+0x160b00 (return 0 while byte +0x56f4 is set, before
        // the forwarding to the other windows); cshell+0x15f470 (the wheel has no such test)
        if self.screen.input_locked() {
            match ev {
                MenuInput::Move(_) | MenuInput::Press(_) | MenuInput::PressAlt(_) => return,
                MenuInput::Release(_) => {
                    self.pressed = None;
                    self.drop_pressed = None;
                    for part in &mut self.screen.sheet_mut().parts {
                        part.down = false;
                    }
                    return;
                }
                MenuInput::Command(cmd) => {
                    self.with_ctx(|s, ctx| s.click(cmd, usize::MAX, ctx));
                    return;
                }
                _ => {}
            }
        }
        // CLI-951: a press on the bar's own messenger button is the button's: the window stays up and the release
        // toggles it shut (the outside-press rule would close it on the press and the release would reopen it)
        // EVIDENCE: cshell+0x1bf20 (0x136, the bar button, toggles the window), cshell+0x12b590 (an outside press
        // closes the window and goes on)
        let on_bar_button = matches!(ev, MenuInput::Press(p) if self.messenger_button_at(p));
        // w3-clan: the messenger window takes the inputs over it first
        if self.messenger.open && !on_bar_button {
            let mut close = false;
            let mut sent = Vec::new();
            let mut asked = Vec::new();
            let taken = self.messenger.input(
                &ev,
                &mut self.cat,
                &self.data,
                &mut |c| sent.push(c),
                &mut asked,
                &mut close,
            );
            self.popups.extend(asked);
            for c in sent {
                self.link.send(c);
            }
            if taken == messenger::Taken::Clicked {
                // EVIDENCE: cshell+0x1aeb90 (a released control that takes the click: interface sound 0x14)
                self.sounds.push(frontfx::SOUND_SELECT);
            }
            if close {
                self.toggle_messenger();
            }
            if taken != messenger::Taken::No {
                return;
            }
        }
        // a modal window of the screen holds the pointer: no control under it is hovered, pressed or clicked
        if matches!(
            ev,
            MenuInput::Move(_) | MenuInput::Press(_) | MenuInput::Release(_)
        ) && self.with_ctx(|s, ctx| s.modal_input(&ev, ctx))
        {
            self.pressed = None;
            self.pressed = None;
            self.hover_part = None;
        }
        match ev {
            MenuInput::Move(p) => {
                // a held scroll bar thumb follows the pointer
                // EVIDENCE: cshell+0x33cd70 (the open list's bar), cshell+0x2f16d0 (a layout list's bar)
                if self.screen.dropdown_mut().is_some_and(|d| d.drag_to(p))
                    || self.screen.sheet_mut().list_drag(p)
                {
                    return;
                }
                let in_list = match self.screen.dropdown_mut() {
                    Some(d) => {
                        d.hover = d.row_at(p);
                        d.contains(p)
                    }
                    None => false,
                };
                let sheet = self.screen.sheet_mut();
                sheet.hover(p);
                // the clickable control under the pointer; a change to another one plays the select-change sound
                // EVIDENCE: cshell+0x1af490 / cshell+0x1aed60 (pointer over a selectable control of the window that
                // takes it: 0x13, or 1 with +0x58; nothing when it leaves every control)
                // DC-DEFAULT: CLI-504 (proposed) one sound per change of the control under the pointer, for enabled
                // buttons and check buttons only (the controls' own +0x70 / +0xbc answers are not traced)
                let target = sheet.hit(p).filter(|_| !in_list).filter(|&i| {
                    let q = &sheet.parts[i];
                    q.enabled && matches!(q.kind, WidgetKind::Button | WidgetKind::CheckButton)
                });
                if target != self.hover_part {
                    self.hover_part = target;
                    if let Some(i) = target {
                        let id = self.control_sound(i, false);
                        self.sounds.push(id);
                    }
                }
                self.screen.hover(p, &self.data);
            }
            MenuInput::Press(p) => {
                // the open drop-down list sits over the screen and takes the presses inside it
                // EVIDENCE: cshell+0x33cbb0 (scroll bar first, then the rows; no interface sound)
                // a press inside the frame but off a row and off the scroll bar is not the list's: the window's own
                // hit test goes on (cshell+0x33cbb0 answers 0)
                if let Some(d) = self.screen.dropdown_mut() {
                    if d.takes(p) {
                        if !d.press_bar(p) {
                            self.drop_pressed = d.row_at(p);
                        }
                        self.pressed = None;
                        return;
                    }
                }
                // a scrolling layout list's bar takes the press before the screen's rows and the controls
                // EVIDENCE: cshell+0x2f07e0 (the list asks its bar first), cshell+0x2f1030
                // DC-DEFAULT: CLI-1884 (proposed) not while a window of the screen lies over the lists (a use window's
                // slot over the storage list's bar)
                if !self.screen.over_lists() && self.screen.sheet_mut().list_press(p) {
                    self.pressed = None;
                    return;
                }
                if self.with_ctx(|s, ctx| s.press(p, ctx)) {
                    // a list row the screen takes: the list control's click
                    // EVIDENCE: cshell+0x1aeb90 (a control that takes the click: 0x14)
                    self.sounds.push(frontfx::SOUND_SELECT);
                    // register-dialog-fix: a control pressed before a box took the release is not clicked later
                    self.pressed = None;
                    return;
                }
                let sheet = self.screen.sheet_mut();
                let hit = sheet.hit(p);
                self.pressed = hit;
                if let Some(i) = hit {
                    match sheet.parts[i].kind {
                        WidgetKind::Button => {
                            sheet.parts[i].down = true;
                            sheet.parts[i].hover = true;
                        }
                        WidgetKind::EditBox => {
                            sheet.focus(Some(i));
                            self.with_ctx(|s, ctx| s.focused(i, ctx));
                        }
                        _ => {}
                    }
                }
            }
            MenuInput::Release(p) => {
                self.screen.release(p);
                // EVIDENCE: cshell+0x3455c0 (the bar's drag ends)
                let dragged = self.screen.dropdown().is_some_and(|d| d.dragging());
                if let Some(d) = self.screen.dropdown_mut() {
                    d.release();
                }
                if self.screen.sheet_mut().list_release() || dragged {
                    return;
                }
                if let Some(row) = self.drop_pressed.take() {
                    let picked = self
                        .screen
                        .dropdown()
                        .filter(|d| d.row_at(p) == Some(row))
                        .map(|d| (d.list, d.values.get(row).copied().unwrap_or(0)));
                    if let Some((list, value)) = picked {
                        // EVIDENCE: cshell+0x33cbb0 (the list hides, then the owner gets the row's index)
                        self.screen.close_dropdown();
                        // DC-DEFAULT: CLI-930 a picked row plays the select click like every other control the
                        // user releases on (cshell+0x33cbb0 itself asks for no sound; the user hears the pick)
                        self.sounds.push(frontfx::SOUND_SELECT);
                        self.with_ctx(|s, ctx| s.dropdown_pick(list, value, row, ctx));
                    }
                    return;
                }
                let sheet = self.screen.sheet_mut();
                for part in &mut sheet.parts {
                    part.down = false;
                }
                let hit = sheet.hit(p);
                let pressed = self.pressed.take();
                let Some(i) = hit.filter(|h| Some(*h) == pressed) else {
                    return;
                };
                // the window's click on its control: the select sound (navigation select on the server screen's
                // common window) before the screen's command runs
                // EVIDENCE: cshell+0x1aeb90 (release on the pressed control that takes the click: id 0x14, or 2 with
                // +0x58, through cshell+0x1b6470)
                let (kind, enabled) = (sheet.parts[i].kind, sheet.parts[i].enabled);
                if kind == WidgetKind::CheckButton || (kind == WidgetKind::Button && enabled) {
                    let id = self.control_sound(i, true);
                    self.sounds.push(id);
                }
                let part = &mut self.screen.sheet_mut().parts[i];
                match part.kind {
                    WidgetKind::CheckButton => {
                        part.checked = !part.checked;
                        let (cmd, on) = (part.command, part.checked);
                        self.with_ctx(|s, ctx| s.check(cmd, i, on, ctx));
                    }
                    WidgetKind::Button if part.enabled && part.command != 0 => {
                        let cmd = part.command;
                        if !self.top_bar(cmd) {
                            self.with_ctx(|s, ctx| s.click(cmd, i, ctx));
                        }
                    }
                    _ => {}
                }
            }
            MenuInput::Key(MenuKey::Backspace) => {
                // gm-client: a screen may take Backspace first (the waiting room's `/k` prompt, CLI-1302)
                if !self.with_ctx(|s, ctx| s.backspace(ctx)) {
                    self.screen.sheet_mut().backspace();
                }
            }
            MenuInput::Key(k) => self.with_ctx(|s, ctx| s.key(k, ctx)),
            MenuInput::Char(c) => {
                // name-change: a window of the screen's own with an edit field takes the character first
                if !self.with_ctx(|s, ctx| s.type_char(c, ctx)) {
                    self.screen.sheet_mut().type_char(c);
                }
            }
            MenuInput::Paste(_) => {}
            MenuInput::Cut => {
                if self.copy_text().is_some() {
                    self.screen.sheet_mut().clear_focused();
                }
            }
            MenuInput::Command(cmd) => {
                if !self.top_bar(cmd) {
                    self.with_ctx(|s, ctx| s.click(cmd, usize::MAX, ctx));
                }
            }
            MenuInput::PressAlt(p) => self.with_ctx(|s, ctx| s.press_alt(p, ctx)),
            MenuInput::Scan(k) => {
                self.with_ctx(|s, ctx| s.scan(k, ctx));
            }
            MenuInput::Wheel(p, notches) => {
                // the open drop-down list takes every turn, wherever the pointer is; else the scrolling list under
                // the pointer
                // EVIDENCE: cshell+0x15f470 (the waiting room's wheel: its open list first, then cshell+0x1af0a0,
                // the control under the pointer)
                if let Some(d) = self.screen.dropdown_mut() {
                    d.wheel(notches);
                    return;
                }
                if !self.with_ctx(|s, ctx| s.wheel(p, notches, ctx)) {
                    self.screen.sheet_mut().list_wheel(p, notches);
                }
            }
        }
    }

    /// The text Ctrl+C copies: the focused edit box of the screen, never a masked (password) box, nothing while a
    /// box is open over the screen. None when there is nothing to copy.
    // DC-DEFAULT: CLI-1512
    pub fn copy_text(&self) -> Option<String> {
        if !self.popups.is_empty() {
            return None;
        }
        self.screen.copy_text().or_else(|| self.screen.sheet().focused_plain_text())
    }

    /// Hands the screen's scrolling lists their rows (their scroll bars' ranges).
    pub fn sync_lists(&mut self) {
        let place = location_text(&self.data);
        set_location(self.screen.sheet_mut(), &place);
        let art = self.lits.drop_art.scroll.clone();
        let rows = self.screen.list_rows(&self.data);
        let follow = self.screen.list_follow();
        let from: Vec<usize> = rows
            .iter()
            .map(|(layer, id, _, _)| self.screen.list_bar_from(*layer, *id))
            .collect();
        let sheet = self.screen.sheet_mut();
        for ((layer, id, n, row_h), from) in rows.into_iter().zip(from) {
            if let Some(i) = sheet.find(layer, id, WidgetKind::List) {
                if n < from {
                    sheet.drop_list_bar(i);
                    continue;
                }
                // a new bar starts at the end of a following list
                let at_end = sheet.list_bar(i).is_none_or(|b| b.value >= b.top());
                sheet.set_list_rows(i, n, row_h, &art);
                if at_end && follow.contains(&(layer, id)) {
                    if let Some(b) = sheet.list_bar_mut(i) {
                        b.value = b.top();
                    }
                }
            }
        }
        self.screen.lists_synced();
    }

    /// Tests: moves the waiting room's start watchdog clock back by `by` (CLI-1314).
    pub fn age_start_watch(&mut self, by: std::time::Duration) {
        self.screen.age_start_watch(by);
    }

    /// Turns the waiting room's idle rules on or off (scripted drivers: off).
    pub fn set_idle_rules(&mut self, on: bool) {
        self.data.idle_rules = on;
    }

    /// CLI-1280: a look / resolution the window noted (the first launch, an Alt+Enter) becomes the setting and is
    /// saved; the next launches start with it.
    pub fn store_display_note(&mut self) {
        let Some(n) = crate::settings::take_display_note() else {
            return;
        };
        if n.apply(&mut self.data.settings) {
            self.data.settings_changed = true;
            crate::settings::publish(&self.data.settings);
            store_settings(&mut self.data, &self.cat);
        }
    }

    /// Reads the connection events (and takes the textures decoded ahead).
    pub fn update(&mut self) {
        self.store_display_note();
        self.store_match_settings();
        // CLI-1031: a sensitivity the match's hotkeys changed becomes the setting and is saved (2009 stores it in
        // the profile values, cshell+0x49060); the next match starts with it
        // CLI-1570: a network overlay mode the match's `/network` chose is saved in our own profile section
        if let Some(v) = crate::settings::take_net_overlay_note() {
            if v != self.data.settings.net_overlay {
                self.data.settings.net_overlay = v;
                self.data.settings_changed = true;
                store_settings(&mut self.data, &self.cat);
            }
        }
        // CLI-1625: an x-ray style the match's `/wallhack` chose is saved in our own profile section
        if let Some(v) = crate::settings::take_xray_note() {
            if v != self.data.settings.xray {
                self.data.settings.xray = v;
                self.data.settings_changed = true;
                store_settings(&mut self.data, &self.cat);
            }
        }
        if let Some(v) = crate::settings::take_sensitivity_note() {
            if v != self.data.settings.sensitivity {
                self.data.settings.sensitivity = v;
                self.data.settings_changed = true;
                store_settings(&mut self.data, &self.cat);
            }
        }
        if let Some(p) = self.prefetch.as_mut().filter(|p| !p.done) {
            p.drain(&mut self.cat.assets);
        }
        // a box with a time limit (the invitation) closes as declined when it runs out
        // EVIDENCE: cshell+0x39110 (the invitation box's 10 s timer 0x2711)
        // crate-result-window: the award window shows its parts one second after it opens
        // EVIDENCE: cshell+0x285a80
        self.tick_popup_at(Instant::now());
        // CLI-1841: the diagnostic reports: problems to ask about or send, their outcome lines, the test's result
        diagbox::tick(self);
        // CLI-1863: a newer client: its box, the update's progress, the quit for the swap
        updatebox::tick(self);
        if self
            .popups
            .first()
            .is_some_and(|p| p.expired(Instant::now()))
        {
            let p = self.popups.remove(0);
            self.dirty = true;
            self.with_ctx(|s, ctx| s.popup_done(&p, false, ctx));
        }
        // the fade: the screen changes at black, then the black lifts
        // DISASM: cshell+0x1ab3a9..0x1ab3f5 (600 ms into the first half: screen change, second half from now; 600 ms
        // into the second: the fade ends)
        if let Some(f) = self.fade {
            self.dirty = true;
            if f.age_ms() >= frontfx::FADE_MS {
                if f.fading_in {
                    self.fade = None;
                    for ev in std::mem::take(&mut self.held_inputs) {
                        self.input(ev);
                    }
                } else {
                    // DC-DEFAULT: CLI-470 (staged) inputs held in the fade-out half were aimed at the old
                    // screen: dropped at the switch, never replayed on the new one
                    self.held_inputs.clear();
                    self.fade = Some(frontfx::Fade {
                        fading_in: true,
                        ..frontfx::Fade::new(f.target)
                    });
                    self.switch(f.target);
                }
            }
        }
        let fed = std::mem::take(&mut self.fed_events);
        for e in self.link.poll().into_iter().chain(fed) {
            self.dirty = true;
            match e {
                LinkEvent::Match { server, lobby } => {
                    self.trail.push("match".into());
                    let mode = self.data.room.mode.max(self.data.mode);
                    self.outcome = Some(MenuOutcome::Play {
                        server,
                        lobby,
                        mode,
                    });
                }
                // CLI-1616: the community list arrived: the browser shows it, the verified mods are trusted
                LinkEvent::Community(v) => {
                    self.data.community = modbrowser::CommunityState::Ready(v);
                    if self.data.profile_store.is_some() {
                        self.link.send(modbrowser::prefs_cmd(&self.data));
                    }
                }
                LinkEvent::ModCatalog(c) => {
                    self.data.catalog = c;
                    let r = self.link.join_report();
                    self.screen.mods_changed(&self.data, r);
                }
                LinkEvent::RowSet { addr, name, set } => {
                    match set {
                        Ok(set) => {
                            modbrowser::note_row_set(&mut self.data, addr, name, set, std::time::SystemTime::now())
                        }
                        // what is known of it stays: no answer is never "declares nothing"
                        Err(e) => eprintln!("mods: the set of {name} was not read: {e}"),
                    }
                    let r = self.link.join_report();
                    self.screen.mods_changed(&self.data, r);
                }
                LinkEvent::ModDownloaded { id, result } => {
                    self.trail.push(format!("mod download {id}: {}", result.is_ok()));
                    self.data.catalog_busy = None;
                    self.data.catalog_note = result.err();
                    let r = self.link.join_report();
                    self.screen.mods_changed(&self.data, r);
                }
                LinkEvent::Closed(why) => {
                    self.data.lobby_up = false;
                    self.data.xray_offered = false;
                    self.trail.push(format!("closed: {why}"));
                    let t = self.cat.text(text_ids::CONNECTION_ENDED);
                    self.popups.clear();
                    self.popups.push(Popup::error(t, BoxTag::ToLogin));
                }
                other => {
                    // lobby20: a new friend request asks the player
                    let request = messenger::new_request(&self.data.friends, &other);
                    // CLI-1901: a clan invitation asks on any screen (one box per clan)
                    if let Some((name, uid)) = messenger::clan_invite(&other) {
                        let open = self.popups.iter().any(|p| p.tag == BoxTag::ClanInvite(uid));
                        if !open {
                            self.popups.push(messenger::clan_invite_box(&name, uid));
                        }
                    }
                    // audit-function (CLI-1389): one box per expired item, named, on any screen (before the
                    // records leave the store)
                    // EVIDENCE: cshell+0x22cde0 (box 0x1786 with the item's name, title 0x371, OK 0x42)
                    if let LinkEvent::Expired { uids, .. } = &other {
                        let wares = storage::Wares::cached(self.cat.rom.as_deref());
                        for u in uids {
                            if let Some(r) = self.data.store.items.iter().find(|i| i.uid == *u) {
                                let name = wares.label_of(r.item_no as i32);
                                let t = buybox::fill_form(&self.cat.text(EXPIRED_TEXT), &[&name]);
                                let mut q = Popup::ok(t, BoxTag::None);
                                q.title = text_ids::TITLE_NOTICE;
                                self.popups.push(q);
                            }
                        }
                    }
                    // passes-effect (CLI-1391): the plain expiry notice, a box per item number, nothing removed
                    // EVIDENCE: cshell+0x39280 (one box res 0x1786 per item number)
                    if let LinkEvent::ExpiryBoxes(nos) = &other {
                        let wares = storage::Wares::cached(self.cat.rom.as_deref());
                        for n in nos {
                            let name = wares.label_of(*n as i32);
                            let t = buybox::fill_form(&self.cat.text(EXPIRED_TEXT), &[&name]);
                            let mut q = Popup::ok(t, BoxTag::None);
                            q.title = text_ids::TITLE_NOTICE;
                            self.popups.push(q);
                        }
                    }
                    // passes-effect (CLI-1392): the own megaphone message also opens box 0x1904
                    // EVIDENCE: cshell+0x38190
                    if matches!(other, LinkEvent::OwnMegaphone) {
                        let t = self.cat.text(OWN_MEGAPHONE_TEXT);
                        self.popups.push(Popup::ok(t, BoxTag::None));
                    }
                    // CLI-1617: the set of the lobby just entered is remembered for the Mods screen
                    if matches!(other, LinkEvent::Channels(_)) {
                        let set = self.link.join_report().set;
                        self.data.xray_offered = set.as_ref().is_some_and(modbrowser::offers_wallhacks);
                        modbrowser::remember_set(&mut self.data, set);
                        // CLI-1632: the packages this join superseded (and week-old retired ones) leave the cache
                        let report = self.link.join_report();
                        self.data.curated_keys = match &report.set {
                            Some(s) if report.curated => s.mods.iter().map(|m| m.key()).collect(),
                            _ => Vec::new(),
                        };
                        self.prune_mod_cache(&report, std::time::SystemTime::now());
                    }
                    shared_event(&mut self.data, &other);
                    clan::shared_event(&mut self.data, &other);
                    let waiting = self.data.friends.unread;
                    messenger::shared_event(&mut self.data, &other);
                    // CLI-1900: a notice that lights the bar button plays the messenger twinkle on a page with the bar
                    // EVIDENCE: cshell+0x13af50 (sound 0x12 with the blink, on the bar screens only: cshell+0x42df0
                    // only marks the notice pending on the others)
                    if self.data.friends.unread > waiting
                        && BAR_SCREENS.contains(&self.base_screen_name())
                    {
                        self.sounds.push(frontfx::SOUND_MESSENGER_TWINKLE);
                    }
                    gm::shared_event(&mut self.data, &self.cat, &other);
                    if let Some(n) = request {
                        if self.data.gm.operator {
                            // CLI-1304: an operator's friend requests are answered at once, no box
                            // EVIDENCE: cshell+0xed4b0 (operator flag -> the messenger's answer call with 2, return)
                            // DC-DEFAULT: CLI-1304 (proposed) the 2009 answer code 2 is read as a decline (INFERRED;
                            // the relay's decline verb)
                            self.link
                                .send(LinkCmd::Messenger(messenger::decline_command(&n)));
                        } else {
                            let t = self.cat.text(0x5ac).replace("%s", &n);
                            self.popups
                                .push(Popup::ask_name(t, BoxTag::FriendRequest, &n));
                        }
                    }
                    // the waiting room's start countdown: a red chat line and the start-count sound each second
                    // EVIDENCE: cshell+0x15f2b0 (line in 0xffff0000, sound id 0xc through cshell+0x1b6470)
                    if let (LinkEvent::Countdown(n), "room") = (&other, self.screen.name()) {
                        let text = self.cat.text(frontfx::COUNTDOWN_TEXT);
                        if let Some(line) = self.lits.countdown_line(&text, *n) {
                            self.data.say_in(line, frontfx::COUNTDOWN_COLOUR);
                            self.sounds.push(frontfx::SOUND_START_COUNT);
                        }
                    }
                    // a 0x3004 that took effect opens the rank-up boxes (`result::rank_up_boxes`)
                    if let LinkEvent::Progress(p) = &other {
                        self.popups.extend(result::rank_up_boxes(&self.cat, p));
                    }
                    // w2-lobbychat: a notice opens its box on any screen (title 0x371, OK)
                    // EVIDENCE: cshell+0x380b0
                    if let LinkEvent::Notice(t) = &other {
                        self.popups.push(Popup::ok(t.clone(), BoxTag::None));
                    }
                    self.with_ctx(|s, ctx| s.event(&other, ctx));
                }
            }
        }
        if self.outcome.is_none() && self.popups.is_empty() {
            if let Some(cmd) = self.screen.due() {
                self.with_ctx(|s, ctx| s.click(cmd, usize::MAX, ctx));
            }
        }
    }

    /// name-change: the lobby connection becomes a detached link (no connection thread, so no "connection ended"
    /// event after the first request); the requests still show in [`Menu::link_latest_request`] (tests and tools).
    pub fn detach_link(&mut self) {
        self.link = Link::detached();
    }

    /// Queues a lobby event as if the link had delivered it; the next [`Menu::update`] handles it (tests and tools).
    pub fn feed_event(&mut self, e: LinkEvent) {
        self.fed_events.push(e);
    }

    /// The game start: the loading page opens at once (no fade, no box) and stays until the match takes the window
    /// or the join fails.
    // EVIDENCE: cshell+0x1630a0 -> cshell+0x1bb0b0(3) (the interface enters the loading state before the engine
    // connects)
    // DISASM: cshell+0x22fa50 (`mission_enter_loading`, UDP_SESSION_2009_A.md §2.2 step 4)
    pub fn begin_loading(&mut self) {
        self.popups.clear();
        self.fade = None;
        self.held_inputs.clear();
        self.switch(Go::Loading);
        self.data.loading = Some(crate::loading::LoadingState {
            since: self.secs(),
            stage: crate::loading::stage::JOIN,
        });
        self.dirty = true;
    }

    /// The load stage the session reached ([`crate::loading::stage`]).
    pub fn set_load_stage(&mut self, stage: u8) {
        if let Some(l) = self.data.loading.as_mut() {
            if l.stage != stage {
                l.stage = stage;
                self.dirty = true;
            }
        }
    }

    /// The loading bar's update count now (0 without the page): the page changes only when it (or the stage) does.
    // DC-DEFAULT: CLI-507 (proposed)
    fn loading_updates(&self) -> u32 {
        self.data
            .loading
            .map_or(0, |l| crate::loading::bar_updates(self.secs() - l.since))
    }

    /// The loading page is up.
    pub fn loading_up(&self) -> bool {
        self.screen.name() == "loading"
    }

    /// The box the waiting room opens after a match that did not end normally (join time-out, lost link, server
    /// bye; [`crate::loading::end_box`]). Call after [`Menu::back_from_match`].
    /// No box is stacked on one already open: when the lobby link was lost during the match too, the
    /// connection-ended box (to the login screen) is the only one.
    // DC-DEFAULT: CLI-509 (proposed) one box after a match: the lost lobby link's box takes the place of the end box
    pub fn match_end_box(&mut self, end: crate::loading::SessionEnd) {
        if !self.popups.is_empty() {
            if let Some(id) = crate::loading::end_box(end) {
                self.trail
                    .push(format!("end box 0x{id:x} skipped (a box is open)"));
            }
            return;
        }
        if let Some(id) = crate::loading::end_box(end) {
            self.trail.push(format!("end box 0x{id:x}"));
            // the 2009 box: title 0x371 (notice) and one OK button 0x42
            // EVIDENCE: cshell+0x1bb910 (0xbd0 with 0x371 / 0x42), cshell+0x1bb2f0 (same title and button)
            let t = self.cat.text(id);
            self.popups.push(Popup::ok(t, BoxTag::None));
            self.dirty = true;
        }
    }

    /// CLI-1611: the weapon content of the server's mod set (the last login's mod step) for the next match; `None`
    /// on vanilla or a set without weapon content.
    pub fn mod_content(&self) -> Option<std::sync::Arc<dc_bute::modpatch::WeaponContent>> {
        let c = self.link.join_report().content;
        (!c.is_empty()).then(|| std::sync::Arc::new(c))
    }

    /// CLI-1619: the recipe of the last login's mod set, from which the match builds its own host (`None`: vanilla).
    pub fn mod_recipe(&self) -> Option<crate::mods::HostRecipe> {
        // MOD-24: the values of the settings page as they are now: a change after the join counts for the next match
        self.link.join_report().recipe.map(|mut r| {
            r.settings = self.data.settings.mods_settings.clone();
            r
        })
    }

    /// MAP-32: the menu's view of the last login's mod set: the community map rows of the catalogue and the
    /// read-only mod file layer (the records' `@mod:` pictures, drawn through the catalogue's textures). Called when
    /// the room list opens and when the menu takes the window back after a match (the match's own mods installed
    /// nothing again when they ended). Vanilla: no rows, nothing installed.
    // DC-DEFAULT: MAP-32 (proposed) the menu's read-only view of the join's mod set, see DC_DEFAULTS.md
    pub fn sync_mods(&mut self) {
        let report = self.link.join_report();
        self.cat
            .set_community_maps(report.community_map_base, report.maps);
        crate::modassets::install(
            report
                .recipe
                .as_ref()
                .map(crate::modassets::ModAssets::from_recipe),
        );
        self.dirty = true;
    }

    /// The community map rows (tests and tools): what [`Menu::sync_mods`] reads from the join report, with the
    /// lobby's first community index (`None`: the ROM row count).
    pub fn set_community_maps(&mut self, base: Option<u32>, maps: Vec<dc_mod_lua::MapRecord>) {
        self.cat.set_community_maps(base, maps);
        self.dirty = true;
    }

    /// The menu ended (game start or quit).
    pub fn take_outcome(&mut self) -> Option<MenuOutcome> {
        self.outcome.take()
    }

    /// Requests the screens gave the lobby connection so far (the step runner's click check).
    pub fn link_sent(&self) -> u64 {
        self.link.sent()
    }

    /// CLI-1632: the cache prune after a successful join (`report`, that join's): superseded packages and retired
    /// ones unused for a week (`now` is the clock); a changed consent list is saved and sent to the link.
    pub fn prune_mod_cache(&mut self, report: &crate::mods::JoinReport, now: std::time::SystemTime) {
        if report.state != crate::mods::ModState::Ready {
            return;
        }
        let before = self.data.settings.mods_accepted.len();
        modbrowser::prune_cache(&mut self.data, report, crate::mods::PruneMode::Auto, now);
        if self.data.settings.mods_accepted.len() != before {
            store_settings(&mut self.data, &self.cat);
            self.link.send(modbrowser::prefs_cmd(&self.data));
        }
    }

    /// The last request the screens gave the lobby connection (`Debug` text): headless tests.
    pub fn link_latest_request(&self) -> String {
        self.link.latest_request()
    }

    /// The requests a detached link ([`Menu::detach_link`]) kept since the last call, as `Debug` texts: headless
    /// tests.
    pub fn take_link_sent(&self) -> Vec<String> {
        self.link
            .take_sent()
            .iter()
            .map(|c| format!("{c:?}"))
            .collect()
    }

    pub fn quitting(&self) -> bool {
        self.quit
    }

    pub fn has_outcome(&self) -> bool {
        self.outcome.is_some()
    }

    /// Back from a match with its result: the result screen opens on the returned lobby connection and goes on to
    /// the waiting room by its exit (the click or its 60 s timer, [`crate::matchexit`]); without a result as
    /// [`Menu::back_from_game`].
    // DISASM: cshell+0x137dd0 (screen 7's focus), cshell+0x132400 (exit 6 -> screen 9)
    pub fn back_from_match(
        &mut self,
        lobby: Option<Lobby>,
        result: Option<crate::matchexit::MatchResult>,
    ) {
        // MAP-32: the match's mods installed nothing when they ended; the menu's own view again
        self.sync_mods();
        match (lobby, result) {
            (Some(l), Some(r)) => {
                self.outcome = None;
                self.link.resume_held(l);
                self.data.result = Some(r);
                self.trail.push("back".into());
                self.switch(Go::Result);
            }
            (lobby, _) => self.back_from_game(lobby),
        }
    }

    /// Back from the game: the lobby connection returns to the connection thread and the waiting room opens
    /// again (`None`: the connection was lost during the match -> login).
    pub fn back_from_game(&mut self, lobby: Option<Lobby>) {
        self.outcome = None;
        match lobby {
            Some(l) => {
                self.link.resume(l);
                self.trail.push("back".into());
                self.switch(Go::Room);
            }
            None => {
                let t = self.cat.text(text_ids::CONNECTION_ENDED);
                self.popups.push(Popup::error(t, BoxTag::ToLogin));
            }
        }
    }

    pub fn set_size(&mut self, width: u32, height: u32) {
        self.cat.set_size(width, height);
        self.dirty = true;
    }

    /// Seconds since the menu started (caret blink).
    pub fn secs(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// The blink phase of the caret: on for one period, off for the next.
    fn caret_phase(&self) -> bool {
        (self.secs() % (2.0 * sheet::CARET_PERIOD)) < sheet::CARET_PERIOD
    }

    /// CLI-1900: the blink phase of the bar's messenger button now (off while nothing waits or the window is open).
    fn bar_blink_phase(&self) -> bool {
        self.data.friends.unread > 0
            && !self.messenger.open
            && sheet::blink_on(messenger::BUTTON_BLINK, self.secs())
    }

    /// Something changed since the last image (or the caret may have blinked).
    /// CLI-1596: the frame-rate text the window measured (`None`: the setting is off); a changed text redraws.
    pub fn set_fps_text(&mut self, text: Option<String>) {
        if text != self.fps_text {
            self.fps_text = text;
            self.dirty = true;
        }
    }

    pub fn needs_redraw(&self) -> bool {
        self.dirty
            || self.fade.is_some()
            || self.screen.animating()
            || (self.screen.name() == "lobby" && self.data.ticker.active(Instant::now()))
            || (self.loading_up() && self.loading_updates() != self.loading_shown)
            || ((self.screen.sheet().focused().is_some() || self.screen.caret_blinks())
                && self.caret_phase() != self.caret_shown)
            || self.bar_blink_phase() != self.bar_blink_shown
            || self
                .figure
                .as_ref()
                .is_some_and(|v| v.pending() || v.due(self.secs() * 1000.0))
    }

    /// shop-model: hands the screen's character request to the view and takes a finished render (`wait`: block until
    /// it is there, the headless image path). Returns the image to draw.
    fn figure_item(&mut self, wait: bool, secs: f32) -> Option<DrawItem> {
        let req = self.screen.pane_figure(&self.data);
        if req.is_none() && self.figure.is_none() {
            return None;
        }
        let size = self.cat.size();
        let rom = self.cat.rom.clone()?;
        let v = self
            .figure
            .get_or_insert_with(|| modelview::ModelView::new(rom));
        // the headless image path draws a fixed pose (deterministic shots); the window loops the clip
        let time = if wait || self.figure_still {
            Some(self.figure_time_ms.unwrap_or(0.0))
        } else {
            self.figure_time_ms
        };
        v.want(req, size, time, secs * 1000.0);
        if wait {
            v.wait(&mut self.cat.assets, MODEL_VIEW_WAIT);
        } else if v.poll(&mut self.cat.assets) {
            self.dirty = true;
        }
        v.item()
    }

    /// CRATE-8: hands the reel's showcase to its own view (made ready during the spin) and returns the render placed and
    /// faded as the frame asks (`wait`: block until the render is there, the headless image path).
    fn showcase_item(&mut self, frame: Option<reel::ShowcaseFrame>, wait: bool) -> Option<DrawItem> {
        if frame.is_none() && self.showcase.is_none() {
            return None;
        }
        let size = self.cat.size();
        let rom = self.cat.rom.clone()?;
        let v = self
            .showcase
            .get_or_insert_with(|| modelview::ModelView::named(rom, "#case-showcase/"));
        let Some(f) = frame else {
            v.want(None, size, None, 0.0);
            return None;
        };
        let req = modelview::ViewReq {
            look: None,
            pieces: Vec::new(),
            lights: Vec::new(),
            place: modelview::Placement {
                origin: [0.0; 3],
                yaw: f.yaw,
            },
            rect: reel::SHOWCASE_RECT,
            xray: None,
            showcase: Some(modelview::Showcase {
                model: f.model.clone(),
                yaw: f.yaw,
                pitch: f.pitch,
                roll: 0.0,
                fill: reel::SHOWCASE_FILL,
                rim: f.rim,
            }),
        };
        v.want(Some(req), size, Some(0.0), 0.0);
        // made ready during the spin without holding the frame up; a shown one is waited for on the image path
        if wait && f.shown {
            v.wait(&mut self.cat.assets, MODEL_VIEW_WAIT);
        } else if v.poll(&mut self.cat.assets) {
            self.dirty = true;
        }
        if !f.shown {
            return None;
        }
        let mut item = v.item()?;
        if let DrawItem::Image { dst, tint, .. } = &mut item {
            *dst = f.dst;
            tint[3] = (f.alpha.clamp(0.0, 1.0) * 255.0) as u8;
        }
        Some(item)
    }

    /// The character request of the current screen (tests).
    pub fn figure_request(&self) -> Option<modelview::ViewReq> {
        self.screen.pane_figure(&self.data)
    }

    /// The renders the draw path asked the character view for (tests).
    pub fn figure_requests(&self) -> u64 {
        self.figure.as_ref().map_or(0, |v| v.requests)
    }

    /// A character render is in flight (tests).
    pub fn figure_pending(&self) -> bool {
        self.figure.as_ref().is_some_and(|v| v.pending())
    }

    /// The yaw of the character render shown (tests).
    pub fn figure_shown_yaw(&self) -> Option<f32> {
        self.figure.as_ref().and_then(|v| v.shown_yaw)
    }

    /// The character view's renders so far (tests).
    pub fn figure_renders(&self) -> u64 {
        self.figure.as_ref().map_or(0, |v| v.renders)
    }

    /// The display list of the current screen and the open box.
    pub fn items(&mut self) -> Vec<DrawItem> {
        let secs = self.secs();
        self.items_at(secs)
    }

    /// [`Menu::items`] at a fixed clock (`secs`): the step runner compares two display lists without the caret's
    /// blink or a timer between them.
    pub fn items_at(&mut self, secs: f32) -> Vec<DrawItem> {
        self.sync_lists();
        self.screen.animate(secs);
        self.sync_bar_blink();
        let mut out = Vec::new();
        self.draw_under(secs, &mut out);
        self.screen.sheet().draw(&mut self.cat, secs, &mut out);
        let wait = self.figure_wait;
        out.extend(self.figure_item(wait, secs));
        self.screen
            .draw_extra(&mut self.cat, &self.data, secs, &mut out);
        if let Some(d) = self.screen.dropdown() {
            d.draw(&mut self.cat, &self.lits.drop_art, &mut out);
        }
        self.messenger.draw(&mut self.cat, &self.data, &mut out);
        if let Some(p) = self.popups.first() {
            p.draw(&mut self.cat, &mut out);
        }
        // CRATE-8: the won gun's model over the reel
        let frame = self
            .popups
            .first()
            .and_then(|p| p.award.as_ref())
            .and_then(|a| a.showcase(Instant::now()));
        out.extend(self.showcase_item(frame, wait));
        if let Some((_, alpha)) = self.fading() {
            let fader = self.lits.fader.clone();
            let size = fader.as_deref().and_then(|t| self.cat.tex_size(t));
            out.push(frontfx::fade_item(fader.as_deref(), size, alpha));
        }
        // CLI-1596: the "Show FPS" text, top right, over everything but the cursor
        if let Some(t) = &self.fps_text {
            out.push(sheet::place_text(
                t,
                [dc_ui_data::VIRTUAL_WIDTH as i32 - 8, 6],
                [0, 0],
                dc_ui_data::pages::Align {
                    first: sheet::ALIGN_RIGHT,
                    second: 0,
                },
                options::FONT,
                options::SIZE,
                options::VALUE_COLOUR,
            ));
        }
        out.extend(self.cursor_item());
        out
    }

    /// The screen under an overlay that shows it (the options, report and help boxes).
    pub(crate) fn draw_under(&mut self, secs: f32, out: &mut Vec<DrawItem>) {
        if !self.screen.shows_under() {
            return;
        }
        if let Some(u) = self.under.last() {
            u.sheet().draw(&mut self.cat, secs, out);
            u.draw_extra(&mut self.cat, &self.data, secs, out);
        }
    }

    /// The pointer the drawn cursor is at (virtual pixels).
    pub fn pointer(&self) -> [f32; 2] {
        self.pointer
    }

    /// Shows or hides the drawn cursor (the window: pointer inside and window focused).
    pub fn set_pointer_shown(&mut self, shown: bool) {
        if shown != self.pointer_shown {
            self.pointer_shown = shown;
            self.dirty = true;
        }
    }

    /// The menu draws its own cursor image (the window hides the system cursor then).
    pub fn has_cursor_image(&self) -> bool {
        self.cat.cursor_size().is_some()
    }

    /// The cursor image at the pointer, the last item of every frame (`None`: no image or hidden).
    // DISASM: cshell+0x1ef530 (drawn after the screen, at the pointer)
    pub fn cursor_item(&self) -> Option<DrawItem> {
        let size = self.cat.cursor_size()?;
        // the loading page draws itself without the interface (no cursor)
        // EVIDENCE: cshell+0x28d640 (the page's update draws and presents its own frame)
        if self.loading_up() {
            return None;
        }
        self.pointer_shown
            .then(|| cursor::item(size, self.pointer, self.cat.size()))
    }

    /// The display list of this frame, marked as drawn: the screen and the open box (see [`Menu::items`]).
    fn take_items(&mut self) -> Vec<DrawItem> {
        let items = self.items();
        self.dirty = false;
        self.caret_shown = self.caret_phase();
        self.bar_blink_shown = self.bar_blink_phase();
        self.loading_shown = self.loading_updates();
        items
    }

    /// The current screen as an opaque image of the catalogue's size (the CPU reference: headless PNGs, tests).
    pub fn image(&mut self) -> RgbaImage {
        self.figure_wait = true;
        self.figure_still = true;
        let items = self.take_items();
        self.figure_wait = false;
        self.cat.compose(&items)
    }

    /// The current screen's whole display list (the black backdrop first, as [`Menu::image`] draws it), marked as
    /// drawn: what [`paint::MenuPainter`] puts on the GPU.
    pub fn frame_items(&mut self) -> Vec<DrawItem> {
        let items = self.take_items();
        catalog::with_backdrop(&items)
    }
}

/// Events every screen sees the same way (the shared lists).
fn shared_event(d: &mut MenuData, e: &LinkEvent) {
    d.store.apply(e);
    match e {
        LinkEvent::Servers(v) => d.servers = v.clone(),
        LinkEvent::OtherLobbies(v) => {
            d.other_lobbies = v.clone();
            d.other_lobbies_read = true;
        }
        LinkEvent::Profile(p) => d.profile = p.clone(),
        LinkEvent::RoomSettings(m) => d.masks = m.clone(),
        LinkEvent::Channels(v) => {
            d.channels = v.clone();
            d.lobby_up = true;
        }
        LinkEvent::Rooms(v) => {
            d.rooms = v.clone();
            d.gun_rooms.clear();
        }
        // LG-305: only a featured entry's offer follows the enter reply
        LinkEvent::ChannelEntered { .. } => d.gun_offered = false,
        // LG-306: the room's Hardcore setting (our push after the room info)
        LinkEvent::RoomHardcore(on) => {
            d.room.hardcore_offered = true;
            d.room.hardcore = *on;
        }
        // LG-307: the room's Possession Arms (our push after the room info)
        LinkEvent::RoomPossession(on) => {
            d.room.possession_offered = true;
            d.room.possession = *on;
        }
        LinkEvent::GunGameOffer => d.gun_offered = true,
        LinkEvent::GunGameRoom { room, on, ladder } => {
            if *on {
                d.gun_rooms.insert(*room);
            } else {
                d.gun_rooms.remove(room);
            }
            if d.room.number == *room {
                d.room.gun_game = *on;
                d.room.ladder = ladder.clone();
            }
        }
        LinkEvent::RoomAdded(r) => {
            d.rooms.retain(|x| x.number != r.number);
            d.rooms.push(r.clone());
        }
        LinkEvent::RoomRemoved(n) => {
            d.gun_rooms.remove(n);
            if let Some(i) = d.rooms.iter().position(|x| x.number == *n) {
                d.rooms.remove(i);
            }
        }
        LinkEvent::RoomRowChanged {
            number,
            kind,
            value,
        } => {
            if let Some(r) = d.rooms.iter_mut().find(|r| r.number == *number) {
                apply_row_change(r, *kind, *value);
            }
        }
        LinkEvent::Users(v) => d.users = v.clone(),
        LinkEvent::UserAdded(u) => {
            d.users.retain(|x| x.id != u.id);
            d.users.push(u.clone());
        }
        LinkEvent::UserRemoved(id) => d.users.retain(|x| x.id != *id),
        LinkEvent::RoomInfo(v) => d.room = v.clone(),
        // LG-274: the room's Hacks choices and setting (our push after the room info)
        LinkEvent::RoomHacks { offered, value } => {
            d.room.hacks_offered = *offered;
            d.room.hacks = *value;
        }
        LinkEvent::Members(m) => {
            d.members = m.clone();
            d.member_sub.clear();
            d.member_colour.clear();
            d.member_clan.clear();
        }
        LinkEvent::MemberAdded(id, m) => {
            d.members.insert(*id, m.clone());
            d.member_sub.remove(id);
            d.member_colour.remove(id);
            d.member_clan.remove(id);
        }
        LinkEvent::MemberRemoved { id, .. } => {
            d.members.remove(id);
            d.member_sub.remove(id);
            d.member_colour.remove(id);
            d.member_clan.remove(id);
        }
        LinkEvent::MemberColour { id, name, clan } => {
            d.member_colour.insert(*id, (*name, *clan));
        }
        LinkEvent::MemberClan { id, clan } => {
            d.member_clan.insert(*id, clan.clone());
        }
        LinkEvent::MemberTeam { id, team } => {
            if let Some(m) = d.members.get_mut(id) {
                m.team = *team;
            }
        }
        LinkEvent::MemberState { id, state, sub } => {
            if let Some(m) = d.members.get_mut(id) {
                m.state = *state;
            }
            if *sub == 0 {
                d.member_sub.remove(id);
            } else {
                d.member_sub.insert(*id, *sub);
            }
        }
        LinkEvent::RoomSetting { kind, value, text } => {
            let r = &mut d.room;
            match kind {
                1 => r.locked = *value != 0,
                2 => r.host_id = *value,
                3 => r.map = *value as i32,
                4 => r.mode = *value as i32,
                5 => r.players = *value as i32,
                6 => r.max_players = *value as i32,
                7 => r.state = *value as i32,
                8 => r.goal = *value as i32,
                9 => r.options[0] = *value as i32,
                0xa => r.options[1] = *value as i32,
                0xd => r.options[2] = *value as i32,
                0xb => r.title = text.clone(),
                // gm-client: the host's room-badge level (CLI-1303)
                // SPEC: PROTOCOL_2009.md §3.7 (0x3035 type 0xf host badge)
                0xf => r.badge = *value as i32,
                // SPEC: PROTOCOL_2009.md §3.7 (0x38c field types as 0x3035)
                // EVIDENCE: cshell+0x1614f0 (type 0xe: the weapon limit)
                0xe => r.weapons = *value as i32,
                _ => {}
            }
        }
        LinkEvent::Chat { name, text, .. } => textbox::say_chat(d, name, text),
        LinkEvent::MuteUntil(t) => d.muted_until = *t,
        LinkEvent::Ticker {
            text,
            show_ms,
            repeat,
            colour,
            runs,
        } => d.ticker.push(
            text.clone(),
            *show_ms,
            *repeat,
            *colour,
            runs.clone(),
            Instant::now(),
        ),
        LinkEvent::CharInfo(x) => {
            d.char_extras = x.clone();
            d.muted_until = x.muted_until;
        }
        // CLI-1600: the login reply's record lands where the lobby login's (0x303) will (the 2009 shell keeps one
        // character record for both)
        // EVIDENCE: cshell+0x42450 (0x201 -> shell +0x194 / +0x124 / +0xfc / +0x100)
        LinkEvent::LoginRecord {
            exp,
            clan_id,
            kills,
            deaths,
        } => {
            d.profile.exp = *exp;
            d.profile.clan_id = *clan_id;
            d.char_extras.clan_id = *clan_id;
            d.char_extras.kills = *kills;
            d.char_extras.deaths = *deaths;
        }
        LinkEvent::Grade(g) => d.char_extras.grade = *g,
        LinkEvent::Balance { credit, prepaid } => d.balance = (*credit, *prepaid),
        _ => {}
    }
}

/// A lobby-form 0x3035 on a room row.
// SPEC: PROTOCOL_2009.md §3.6 (0x3035 lobby form), §3.7 (types 1 locked, 2 host, 3 map, 4 mode, 5 players,
// 6 max players, 7 state)
pub fn apply_row_change(r: &mut RoomRow, kind: u32, value: u32) {
    match kind {
        1 => r.locked = value != 0,
        2 => r.host = value,
        3 => r.map = value as i32,
        4 => r.mode = value as i32,
        5 => r.players = value as i32,
        6 => r.max_players = value as i32,
        7 => r.state = value as i32,
        _ => {}
    }
}

/// The top bar's option, help and report commands.
// OBSERVED: the top bar records of the user's ROM pages (common and lobby common: option, report, help, exit)
// EVIDENCE: cshell+0x1ac4b0 (0x12d, 0x12e, 0x273)
pub const CMD_BAR_PREFS: i32 = 301;
pub const CMD_BAR_ASSIST: i32 = 302;
pub const CMD_BAR_ABUSE: i32 = 627;
/// The screens whose pages carry that top bar.
// EVIDENCE: the ROM layout pages of shop / market / inbox, modification and clan carry the same three buttons
// (gate-steps widget dump); the shop family answers to screen name "shop"
pub const TOP_BAR_SCREENS: [&str; 8] = [
    "servers", "channels", "lobby", "create", "room", "shop", "tuning", "clan",
];
/// The screens whose bottom bar carries the messenger button.
pub const BAR_SCREENS: [&str; 7] = [
    "servers", "lobby", "create", "room", "shop", "tuning", "clan",
];

/// The lobby bar's place text: "<server> - <channel>" once a channel was entered (empty before).
// EVIDENCE: cshell+0x19e30 (called from every screen focus, cshell+0x1bb70: the common page's window 1 text 4 gets
// "%s - %s" of the server name and the channel name), cshell+0xfacd0 (the channel enter answer keeps the channel name)
pub fn location_text(data: &MenuData) -> String {
    let server = data.server.as_ref().map(|s| s.name.as_str()).unwrap_or("");
    let channel = data.profile.chan_label.as_str();
    if server.is_empty() || channel.is_empty() {
        return String::new();
    }
    format!("{server} - {channel}")
}

/// The lobby bar's place text control: window 1 text 4 of the lobby screens' common page.
// EVIDENCE: cshell+0x19e30 (page 1, window 1, control 4, set text vtable +0x150)
pub const LOCATION_WINDOW: i32 = 1;
pub const LOCATION_TEXT: i32 = 4;

/// Writes `place` into the common page's place text of `sheet` (screens without that page are left alone).
fn set_location(sheet: &mut Sheet, place: &str) {
    if place.is_empty() {
        return;
    }
    if let Some(p) = sheet.parts.iter_mut().find(|p| {
        p.page == pages::LOBBY_COMMON
            && p.kind == WidgetKind::Text
            && p.layer == LOCATION_WINDOW
            && p.id == LOCATION_TEXT
    }) {
        if p.text != place {
            p.text = place.to_string();
        }
    }
}

/// The screen name an overlay shows.
fn overlay_name(g: Go) -> &'static str {
    match g {
        Go::Options => options::NAME,
        Go::Report => report::NAME,
        Go::Help => help::NAME,
        Go::CharInfo => charinfo::NAME,
        Go::Community => modbrowser::COMMUNITY_NAME,
        Go::Mods => modbrowser::MODS_NAME,
        _ => "",
    }
}

/// The pages of a screen; an empty list without a ROM.
pub fn build_sheet(cat: &Catalog, tags: &[PageTag]) -> Sheet {
    let mut sheet = Sheet::build(&cat.pages, tags, cat);
    // CLI-902: the common bar's text (1, 5) is the notice string slot, not a label: the record's placeholder ("-" in
    // red, centred on the Inbox button: the user's "red dot on the inbox") is replaced by the stored notice text,
    // which is empty until a notice arrives (the control stays hidden meanwhile); the notice bar draws the ticker itself ([`textbox::draw_ticker`]).
    // EVIDENCE: cshell+0x19680 (control (1, 5): text := the last notice string through the control's text setter
    // +0x150, moved to the notice bar, colour set), cshell+0x1f850 (the same control placed for screens 9, 0xf and
    // 10, 0x13 .. 0x16, 0x1c)
    for p in sheet.parts.iter_mut().filter(|p| {
        p.page == pages::LOBBY_COMMON
            && p.kind == WidgetKind::Text
            && p.layer == BAR_STRING_WINDOW
            && p.id == BAR_STRING_ID
    }) {
        p.shown = false;
    }
    sheet
}

/// The common page's notice string control (window 1, text 5).
pub const BAR_STRING_WINDOW: i32 = 1;
pub const BAR_STRING_ID: i32 = 5;

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI-1561: the focus messages of the shop-family screens and the room (cshell+0x14f300, cshell+0x159a99,
    /// cshell+0x104960, cshell+0x164020).
    #[test]
    fn focus_messages_follow_the_2009_screen_focus_handlers() {
        let f = |from: Go, to: Go| -> Vec<String> {
            focus_messages(from.state_id(), to.state_id())
                .iter()
                .map(|c| format!("{c:?}"))
                .collect()
        };
        assert_eq!(f(Go::Room, Go::Shop), ["Location(2)", "PrivateState(1)"]);
        assert_eq!(f(Go::Lobby, Go::Tuning), ["Location(3)", "PrivateState(3)"]);
        assert_eq!(f(Go::Room, Go::Clan), ["PrivateState(4)"]);
        assert_eq!(
            f(Go::Shop, Go::Tuning),
            ["PrivateStateEnd", "Location(3)", "PrivateState(3)"]
        );
        assert_eq!(
            f(Go::Tuning, Go::Room),
            ["PrivateStateEnd", "Location(1)", "Shopping(false)"]
        );
        assert_eq!(f(Go::Shop, Go::Lobby), ["PrivateStateEnd"]);
        // one screen state (shop, black market, inbox), and the room after create / result: nothing
        assert!(f(Go::Shop, Go::Inbox).is_empty());
        assert!(f(Go::Create, Go::Room).is_empty());
        assert!(f(Go::Result, Go::Room).is_empty());
    }

    #[test]
    fn character_fields_grade_and_balance_are_kept_for_the_lobby_panel() {
        let mut d = MenuData::default();
        let x = link::CharExtras {
            grade: 2,
            clan_id: 77,
            ..Default::default()
        };
        shared_event(&mut d, &LinkEvent::CharInfo(x.clone()));
        assert_eq!(d.char_extras, x);
        shared_event(&mut d, &LinkEvent::Grade(5));
        assert_eq!((d.char_extras.grade, d.char_extras.clan_id), (5, 77));
        shared_event(
            &mut d,
            &LinkEvent::Balance {
                credit: 1200,
                prepaid: 30,
            },
        );
        assert_eq!(d.balance, (1200, 30));
    }
}
