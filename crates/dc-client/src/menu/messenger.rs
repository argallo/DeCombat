//! w3-clan: the messenger window (the lobby bar's messenger button, command 0x136): the friend list with each
//! friend's online state, add friend, whisper and the friend requests, over the screen it was opened on.
//!
//! The 2009 window is built in code (no layout page): a 250 x 458 window at (768, 277) with group tabs, a friend
//! tree, three text buttons and a close button; a press outside it closes it and goes on to the screen. Opening and
//! closing tell the lobby where the player is (0x306: 0xd while it is open, the screen's place when it closes). The
//! 2009 friends and whispers ran through an external messenger component; DeCombat carries them over the lobby
//! connection with its own relay (`!nm` commands out, 0x3f01 lines in).
// SPEC: PROTOCOL_2009.md §3.4 (0x306 location), §6.3 (the DeCombat messenger relay; 0x3f01 is not a 2009 message)
// EVIDENCE: cshell+0x1bf20 (command 0x136 -> cshell+0x12b350), cshell+0x12b350 (shown flag toggled: shown -> 0x306
// {0xd}; hidden -> 0x306 with the screen's place: 9 -> 1, 0xf -> 0, 0x14 -> 3, 0x16 -> 2, else 0xe),
// cshell+0x12af90 (window texture, 0xfa x 0x1ca at (0x300, 0x115)), cshell+0x12b590 (a press outside the window
// closes it and is not taken), cshell+0xf1100 (the window's controls), cshell+0xecb40 (the friend rows)

use super::catalog::Catalog;
use super::link::{LinkCmd, LinkEvent};
use super::popup::{BoxTag, Popup};
use super::scrollbar::{self, ScrollBar};
use super::sheet::{image_fit, place_text, ALIGN_CENTRE, ALIGN_LEFT, CAPTION_COLOUR};
use super::{Go, MenuData, MenuInput, MenuKey, DOUBLE_CLICK_MS};
use crate::hud::items::DrawItem;
use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use std::path::Path;
use std::time::Instant;

/// The lobby bar's messenger command.
// EVIDENCE: cshell+0x1bf20 (0x136)
pub const BAR_FRIENDS_CMD: i32 = 0x136;
/// The window's place and size.
// EVIDENCE: cshell+0x12af90 (size 0xfa x 0x1ca, place (0x300, 0x115))
pub const WINDOW_AT: [i32; 2] = [0x300, 0x115];
pub const WINDOW_SIZE: [i32; 2] = [0xfa, 0x1ca];
/// The first group tab (the other two are built hidden and disabled): place, size, caption string, caption point and
/// its colours (normal, selected).
// EVIDENCE: cshell+0xf1100 (tabs at (0xc, 0x34), (0x5a, 0x34), (0xa8, 0x34), 0x46 x 0x14, strings 0x586 / 0x587 /
// 0x588 at (0x23, 10) centred (4 | 0x40) in 0xff7a7a7a / 0xffcb8c01; the first selected, the others hidden)
pub const TAB_AT: [i32; 2] = [0xc, 0x34];
pub const TAB_SIZE: [i32; 2] = [0x46, 0x14];
pub const TAB_TEXT: i32 = 0x586;
pub const TAB_CAPTION_AT: [i32; 2] = [0x23, 10];
pub const TAB_SELECTED_COLOUR: u32 = 0xffcb_8c01;
/// The text buttons along the bottom: string, command, x (all at y 0x1ad, font 14 size 12, left / top), the first
/// disabled; their four colours.
// EVIDENCE: cshell+0xf1100 (0x589 command 0x2717 at 0x16 disabled; 0x13f7 command 0xdf at 0x61; 0x49e command 0x2716
// at 0xaf; y 0x1ad; size 0xc, flags 0x11; colours 0xffcd8c02, 0xffacacac, 0xff595858, 0xffcd8c02)
pub const TEXT_BUTTONS: [(i32, i32, i32, bool); 3] = [
    (0x589, 0x2717, 0x16, false),
    (0x13f7, 0xdf, 0x61, true),
    (0x49e, 0x2716, 0xaf, true),
];
pub const TEXT_BUTTON_Y: i32 = 0x1ad;
pub const TEXT_BUTTON_SIZE: i32 = 0xc;
pub const TEXT_BUTTON_COLOURS: [u32; 4] = [0xffcd_8c02, 0xffac_acac, 0xff59_5858, 0xffcd_8c02];
pub const FRIEND_ADD_CMD: i32 = 0xdf;
pub const GROUP_ADD_CMD: i32 = 0x2716;
/// The close button: place, size, command.
// EVIDENCE: cshell+0xf1100 (prefix VA 0x374dd328, command 0x271d, 0xf x 0xf at (0xd1, 0x14))
pub const CLOSE_AT: [i32; 2] = [0xd1, 0x14];
pub const CLOSE_SIZE: [i32; 2] = [0xf, 0xf];
pub const WINDOW_SHUT_CMD: i32 = 0x271d;
/// The friend list: place and size in the window; its rows.
// EVIDENCE: cshell+0xf1100 (list at (0x13, 0x56), 0xd5 x 0x13c), cshell+0xecb40 (rows 0x14 high; the bar under the
// pointer 0xb4 x 0x14; group name size 0xc; online name 0xffcdcdcd, offline 0xff53514b)
pub const LIST_AT: [i32; 2] = [0x13, 0x56];
pub const LIST_SIZE: [i32; 2] = [0xd5, 0x13c];
pub const ROW_HEIGHT: i32 = 0x14;
pub const ROW_BAR: [i32; 2] = [0xb4, 0x14];
pub const ROW_TEXT_SIZE: i32 = 0xc;
pub const ONLINE_NAME_COLOUR: u32 = 0xffcd_cdcd;
pub const OFFLINE_NAME_COLOUR: u32 = 0xff53_514b;
/// The places the 2009 screen reports when the window closes.
// EVIDENCE: cshell+0x12b350
pub const LOCATION_SHOWN: u32 = 0xd;
pub const LOCATION_OTHER: u32 = 0xe;

/// The friend tree in the list: the inner list's place in the list and its height; its scroll bar's x in the inner
/// list (0xe wide, as high as the list, page = the height).
// EVIDENCE: cshell+0xf0d90 (inner list at (0xb, 8), height 0x13f; scroll bar at (0xbc, 0) with page 0x13f and the
// UI_SCR_Base Scroll_UP / Scroll_DOWN / Scroll_Back_A / Scroll_Bar_A textures), cshell+0x2f00f0 (bar height = list
// height - bar y)
pub const TREE_AT: [i32; 2] = [0xb, 8];
pub const TREE_HEIGHT: i32 = 0x13f;
pub const TREE_BAR_X: i32 = 0xbc;
/// A tree row: the state icon (group open / closed, user online / offline) and the name relative to the row at the
/// row's indent; members are indented 7 per level; a top-level group's members are followed by an empty row 0x17
/// high. Group rows: the name in font 0xe size 0xc and 0xff81705c; user rows: font 3 size 0xe. The group row's under
/// bar (Group_UnderBar at (0, 0x11) in cshell+0xecb40) is not drawn: the reference stack frame shows none.
// EVIDENCE: cshell+0xecb40 (row 0x122 x 0x14; icons at (indent, -4); name at (indent + 0x13, 2); group: under bar
// Group_UnderBar at (0, 0x11), colours 0xff81705c, Group_Open shown while the group's closed field is 0; user:
// font 3 size 0xe, colours 0xff53514b offline / 0xffcdcdcd online), cshell+0xf0000 (indent = depth x 7; after a
// top-level group's members a row 0x122 x 0x17 that takes no press)
/// Where the icon and the name land in the row as the reference stack frame shows them (the open branch of cshell+0xecb40
/// moves the shown icon; its amount and the name control's own top inset are not read from the decompile).
// OBSERVED: screens-sweep/cpp/messenger/shop_bb00_messenger.png (reference client, 1024 x 768: the
// group's 11 x 11 mark at tree + (2, -2), the name's cap top at tree y + 3, one pixel above our label at y + 2)
pub const ROW_ICON_AT: [i32; 2] = [2, -2];
pub const ROW_NAME_AT: [i32; 2] = [0x13, -1];
pub const INDENT: i32 = 7;
pub const GROUP_GAP: i32 = 0x17;
pub const GROUP_NAME_COLOUR: u32 = 0xff81_705c;
pub const GROUP_FONT: i32 = 0xe;
pub const USER_FONT: i32 = 3;
pub const USER_TEXT_SIZE: i32 = 0xe;
/// DeCombat's look of what the 2009 window builds elsewhere or not at all.
// DC-DEFAULT: CLI-564 (proposed) a pending friend request is a row in the selected-tab colour with the offline icon;
// the text field (add friend / whisper) is one line at the bottom of the list (y + 0x13c - 0x14) with a dark fill,
// font 14 size 12
pub const FIELD_FILL: [u8; 4] = [16, 16, 16, 230];
/// The one group of the tree: the 2009 groups came from the external messenger component, the DeCombat relay has
/// none.
// DC-DEFAULT: CLI-750 (proposed) one open top-level group "Friends" holding the friends, then the open requests
pub const GROUP_NAME: &str = "Friends";
/// The whisper lines' colour in the lobby chat and the relay's status lines' colour.
// DC-DEFAULT: CLI-565 (proposed) whisper lines `from >> to : text` in 0xffe6b4ff, status lines in 0xffcdcdcd
pub const WHISPER_COLOUR: u32 = 0xffe6_b4ff;
pub const STATUS_COLOUR: u32 = 0xffcd_cdcd;

const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
    Pin {
        what,
        rva,
        sha16,
        len,
    }
}

/// Texture names of the window (literals of the user's CShell.dll).
// EVIDENCE: cshell+0x12af90 (window VA 0x374e1dc8), cshell+0xf1100 (tab prefix VA 0x374dd358, close prefix VA
// 0x374dd328), cshell+0xecb40 (online / offline icons VA 0x374dcad0 / 0x374dcaa8, row bar VA 0x374dcb6c)
pub const WINDOW_TEX: Pin = pin("messenger window", 0x3e1dc8, "4055c5a74e6a8a0f", 37);
pub const TAB_PREFIX: Pin = pin("messenger tab prefix", 0x3dd358, "108abc15d3bad6c0", 39);
pub const CLOSE_PREFIX: Pin = pin("messenger close prefix", 0x3dd328, "88fc3e2f6ca4d2b2", 46);
pub const ONLINE_ICON: Pin = pin("messenger online icon", 0x3dcad0, "4f2aeec9eb222f34", 37);
pub const OFFLINE_ICON: Pin = pin("messenger offline icon", 0x3dcaa8, "52298e435d10b10d", 38);
pub const ROW_BAR_TEX: Pin = pin("messenger row bar", 0x3dcb6c, "87788524297c641e", 34);
// EVIDENCE: cshell+0xecb40 (group icons VA 0x374dcaf8 / 0x374dcb20)
pub const GROUP_OPEN_ICON: Pin = pin("messenger group open", 0x3dcaf8, "30d6746a5f78fccc", 36);
pub const GROUP_CLOSE_ICON: Pin = pin("messenger group close", 0x3dcb20, "1faf46d1fab51a19", 37);

/// The window's textures (none without the ROM: fills and plain texts then).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessengerArt {
    pub window: Option<String>,
    pub tab: Option<String>,
    pub close: Option<String>,
    pub online: Option<String>,
    pub offline: Option<String>,
    pub bar: Option<String>,
    /// Group_Open (the "open" mark of a closed group) and Group_Close (the "close" mark of an open group).
    pub group_open: Option<String>,
    pub group_close: Option<String>,
    /// The tree's scroll bar: back, thumb, up, down (the drop-down list's literals).
    pub scroll: [Option<String>; 4],
}

impl MessengerArt {
    pub fn from_rom(rom: &Path) -> MessengerArt {
        let Ok(pe) =
            dc_rom::PeFile::from_file(&rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]))
        else {
            return MessengerArt::default();
        };
        let get = |p: &Pin| read_pinned(&pe, p).ok();
        MessengerArt {
            window: get(&WINDOW_TEX),
            tab: get(&TAB_PREFIX),
            close: get(&CLOSE_PREFIX),
            online: get(&ONLINE_ICON),
            offline: get(&OFFLINE_ICON),
            bar: get(&ROW_BAR_TEX),
            group_open: get(&GROUP_OPEN_ICON),
            group_close: get(&GROUP_CLOSE_ICON),
            scroll: super::dropdown::SCROLL_TEX.map(|p| get(&p)),
        }
    }
}

/// One friend of the list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Friend {
    pub name: String,
    pub online: bool,
}

/// What the menu keeps of the relay's lines (they arrive on any screen, also while the window is closed).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Friends {
    pub me: String,
    pub list: Vec<Friend>,
    /// Open friend requests (names).
    pub requests: Vec<String>,
    /// The lobby button asked for the window (the menu toggles it after the screen's handler).
    pub toggle_asked: bool,
    /// lobby20: the whispers kept per other party (the 1:1 chat panel's lines), at most [`CHAT_KEEP`] parties.
    pub chats: Vec<Chat>,
    /// The messenger window is shown (the menu keeps it in step with [`Messenger::open`]).
    pub shown: bool,
    /// CLI-1900: the notices (whispers of others, new friend requests) that came while the window was closed; while
    /// any wait the bar's messenger button blinks ([`BUTTON_BLINK`]); opening the window clears them.
    // EVIDENCE: cshell+0x13af50 / cshell+0x13afc0 (a messenger notice while the window is not shown (+0x5530 = 0):
    // the bar button's blink on, sound 0x12, pending +0x5534 = 1), cshell+0x12b350 (opening the window clears the
    // pending flag), cshell+0x1bb70 (every screen focus starts the blink again while pending)
    pub unread: u32,
}

/// CLI-1900: the blink period of the bar's messenger button while [`Friends::unread`] (seconds).
// EVIDENCE: cshell+0x13af50 (the button's period +0x1e8 set to 0x3ea8f5c3 = 0.33), cshell+0x1bf20 (command 0x136
// sets it back to 0)
pub const BUTTON_BLINK: f32 = 0.33;

/// One 1:1 conversation: the other party and its lines (speaker, text).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chat {
    pub name: String,
    pub lines: Vec<(String, String)>,
}

/// How many conversations and lines per conversation are kept.
// EVIDENCE: cshell+0xf07c0 (the chat tabs list holds 5 chats: the next / previous commands 0x2714 / 0x2715 test for
// more than 5)
// DC-DEFAULT: CLI-842 (proposed) 5 conversations, the last 64 lines each
pub const CHAT_KEEP: usize = 5;
pub const CHAT_LINES: usize = 64;

impl Friends {
    /// One relay line (its tab-separated fields; the first is the kind). Returns a chat line to show, if any.
    // SPEC: PROTOCOL_2009.md §6.3 (`friends` own name and friend names; `online` name and 1 / 0; `request` from-name;
    // `whisper` from, to, text; `status` text; `invite` from-name, room number)
    pub fn apply(&mut self, f: &[String]) -> Option<(String, u32)> {
        let get = |i: usize| f.get(i).map_or("", String::as_str);
        match get(0) {
            "friends" => {
                self.me = get(1).to_string();
                let old = std::mem::take(&mut self.list);
                self.list = f
                    .iter()
                    .skip(2)
                    .filter(|n| !n.is_empty())
                    .map(|n| Friend {
                        name: n.clone(),
                        online: old.iter().any(|o| &o.name == n && o.online),
                    })
                    .collect();
                // a friend is no longer a request
                self.requests
                    .retain(|r| !self.list.iter().any(|x| &x.name == r));
                None
            }
            "online" => {
                let on = get(2) == "1";
                if let Some(x) = self.list.iter_mut().find(|x| x.name == get(1)) {
                    x.online = on;
                }
                None
            }
            "request" => {
                let n = get(1).to_string();
                if !n.is_empty() && !self.requests.contains(&n) {
                    self.requests.push(n);
                    // DC-DEFAULT: CLI-1900 (proposed) a new friend request while the window is closed lights the button
                    self.unread += u32::from(!self.shown);
                }
                None
            }
            "whisper" => {
                // lobby20: the conversation with the other party keeps the line (the relay sends it to both sides)
                let other = if get(1) == self.me { get(2) } else { get(1) };
                self.hear(other, get(1), get(3));
                // DC-DEFAULT: CLI-1900 (proposed) another player's whisper while the window is closed lights the
                // button (the own line, which the relay echoes, does not)
                if get(1) != self.me {
                    self.unread += u32::from(!self.shown);
                }
                Some((
                    format!("{} >> {} : {}", get(1), get(2), get(3)),
                    WHISPER_COLOUR,
                ))
            }
            "status" if !get(1).is_empty() => Some((get(1).to_string(), STATUS_COLOUR)),
            _ => None,
        }
    }

    /// Adds a line to the conversation with `other`.
    pub fn hear(&mut self, other: &str, from: &str, text: &str) {
        let i = match self.chats.iter().position(|c| c.name == other) {
            Some(i) => i,
            None => {
                if self.chats.len() >= CHAT_KEEP {
                    self.chats.remove(0);
                }
                self.chats.push(Chat {
                    name: other.to_string(),
                    lines: Vec::new(),
                });
                self.chats.len() - 1
            }
        };
        let c = &mut self.chats[i];
        c.lines.push((from.to_string(), text.to_string()));
        if c.lines.len() > CHAT_LINES {
            c.lines.remove(0);
        }
    }

    /// The conversation with `name`.
    pub fn chat(&self, name: &str) -> Option<&Chat> {
        self.chats.iter().find(|c| c.name == name)
    }

    /// Rows of the list: friends, then the open requests (name, online, request).
    pub fn rows(&self) -> Vec<(&str, bool, bool)> {
        self.list
            .iter()
            .map(|f| (f.name.as_str(), f.online, false))
            .chain(self.requests.iter().map(|r| (r.as_str(), false, true)))
            .collect()
    }
}

/// One row of the friend tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeRow {
    /// A group: open or closed.
    Group(bool),
    /// Entry `i` of [`Friends::rows`].
    User(usize),
    /// The empty row after a top-level group's members.
    Gap,
}

impl TreeRow {
    pub fn height(self) -> i32 {
        match self {
            TreeRow::Gap => GROUP_GAP,
            _ => ROW_HEIGHT,
        }
    }
}

/// The tree's rows with their tops (from the tree's top): the one group, open, with every friend and request.
// EVIDENCE: cshell+0xf0000 (group, its members one level deeper, the gap after a top-level group)
pub fn tree_rows(friends: &Friends) -> Vec<(TreeRow, i32)> {
    let rows = std::iter::once(TreeRow::Group(true))
        .chain((0..friends.rows().len()).map(TreeRow::User))
        .chain(std::iter::once(TreeRow::Gap));
    let mut y = 0;
    rows.map(|r| {
        let top = y;
        y += r.height();
        (r, top)
    })
    .collect()
}

/// The name of a friend request `e` brings that is not open yet.
pub fn new_request(friends: &Friends, e: &LinkEvent) -> Option<String> {
    match e {
        LinkEvent::Messenger(f) if f.first().map(String::as_str) == Some("request") => f
            .get(1)
            .filter(|n| !n.is_empty() && !friends.requests.contains(n))
            .cloned(),
        _ => None,
    }
}

/// The question a friend request asks: string 0x5ac with the name, Yes accepts, No declines.
// EVIDENCE: cshell+0xed4b0 (CMessengerWnd::AddFriendConfirm: box text 0x5ac formatted with the requester's name,
// title 0x371, buttons 0x35 / 0x36)
pub fn request_box(cat: &Catalog, name: &str) -> Popup {
    let t = cat.text(0x5ac).replace("%s", name);
    Popup::ask_name(t, BoxTag::FriendRequest, name)
}

/// CLI-1901: a clan leader's invitation (relay line `claninvite<TAB>clan name<TAB>clan number`, LG-420): the clan's
/// name and number.
pub fn clan_invite(e: &LinkEvent) -> Option<(String, u32)> {
    match e {
        LinkEvent::Messenger(f) if f.first().map(String::as_str) == Some("claninvite") => {
            let name = f.get(1).filter(|n| !n.is_empty())?.clone();
            let uid = f.get(2)?.trim().parse::<u32>().ok().filter(|u| *u != 0)?;
            Some((name, uid))
        }
        _ => None,
    }
}

/// The question of a clan invitation. The ROM string table has no clan membership invitation (its clan invitations,
/// 0x50c, are to a clan match room), so the text is ours; the box is the friend request's (title 0x371, Yes 0x35 /
/// No 0x36).
// DC-DEFAULT: CLI-1901 (proposed) the clan invitation box and its text
pub const CLAN_INVITE_TEXT: &str = "Clan %s invites you to join. Do you accept the invitation?";

pub fn clan_invite_box(name: &str, uid: u32) -> Popup {
    let t = CLAN_INVITE_TEXT.replace("%s", name);
    Popup::ask_name(t, BoxTag::ClanInvite(uid), name)
}

/// The answers of the clan invitation box (LG-420 relay commands over the own lobby connection).
pub fn clan_yes_command(uid: u32) -> String {
    format!("!nm clanyes\t{uid}")
}
pub fn clan_no_command(uid: u32) -> String {
    format!("!nm clanno\t{uid}")
}

/// The messenger parts of the events every screen sees: the relay's lines.
pub fn shared_event(d: &mut MenuData, e: &LinkEvent) {
    if let LinkEvent::Messenger(f) = e {
        // CLI-1141: Block Whispers drops the lines another player whispers (the own lines go on)
        // DC-DEFAULT: CLI-1141 (proposed) the Other tab's Block Whispers / Block Invitations switches; the 2009 effect is
        // not traced, ours is: no whisper line of another player and no room invitation box while on
        if d.settings.block_whispers()
            && f.first().map(String::as_str) == Some("whisper")
            && f.get(1).is_some_and(|from| *from != d.friends.me)
        {
            return;
        }
        if let Some((line, colour)) = d.friends.apply(f) {
            d.say_in(line, colour);
        }
    }
}

/// The relay commands.
// SPEC: PROTOCOL_2009.md §6.3 (`!nm` verbs add, accept, w, list; tab-separated arguments)
pub fn add_command(name: &str) -> String {
    format!("!nm add\t{name}")
}
pub fn accept_command(name: &str) -> String {
    format!("!nm accept\t{name}")
}
pub fn whisper_command(to: &str, text: &str) -> String {
    format!("!nm w\t{to}\t{text}")
}
pub const LIST_COMMAND: &str = "!nm list";
pub fn decline_command(name: &str) -> String {
    format!("!nm decline	{name}")
}
pub fn remove_command(name: &str) -> String {
    format!("!nm remove	{name}")
}
pub fn invite_command(name: &str) -> String {
    format!("!nm invite	{name}")
}

/// The friend row menu's entries: string, what it does.
// EVIDENCE: cshell+0xeb6c0 (the friend row menu is built with strings 0x57d Chat, 0x57e Invite to Game Room, 0x57f
// Join User's Game Room, 0x580 Move to Another Group, 0x581 Block/Unblock, 0x1412 Remove), cshell+0xf0250 (its
// commands: 0x2718 chat attempt, 0x271e invite, 0x271f join, 0x271b move, 0x271a block, 0x2719 remove)
// DC-DEFAULT: CLI-842 (proposed) only the entries the relay serves (chat, invite, remove); the menu's look is ours
// (the 2009 one is the WaitRoom BOX_32_32 / BOX_8_8 frame, 0xf0 wide, not drawn from it here)
pub const MENU_ITEMS: [(i32, MenuItem); 3] = [
    (0x57d, MenuItem::Chat),
    (0x57e, MenuItem::Invite),
    (0x1412, MenuItem::Remove),
];
pub const MENU_WIDTH: i32 = 0xf0;
pub const MENU_ROW: i32 = 20;
pub const MENU_FILL: [u8; 4] = [24, 22, 18, 250];
pub const MENU_EDGE: [u8; 4] = [190, 189, 189, 255];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Chat,
    Invite,
    Remove,
}

/// The open friend row menu: the friend's name, its top-left and the entry under the pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendMenu {
    pub name: String,
    pub at: [i32; 2],
    pub hover: Option<usize>,
}

impl FriendMenu {
    fn rect(&self) -> [i32; 4] {
        [
            self.at[0],
            self.at[1],
            MENU_WIDTH,
            MENU_ROW * MENU_ITEMS.len() as i32,
        ]
    }
    fn item_at(&self, p: [f32; 2]) -> Option<usize> {
        inside(self.rect(), p).then(|| ((p[1] as i32 - self.at[1]) / MENU_ROW) as usize)
    }
}

/// The 1:1 chat panel over the friend list: its close button's command and place in the list.
// EVIDENCE: cshell+0xf07c0 (0x2711 CMD_CLOSE_CHAT closes the chat; 0x271c sends the field's text to the chat's
// user; the title is string 0x5ab "Chatting with %s", cshell+0xedbf0), PROTOCOL_2009.md §6.3 (whispers: our relay)
// DC-DEFAULT: CLI-842 (proposed) one conversation at a time over the list's rectangle: the title line, the lines
// (16 px, wrapped), the input line at the list's bottom and a Close caption (0x1613) at its top right
pub const CHAT_CLOSE_CMD: i32 = 0x2711;
pub const PANEL_PITCH: i32 = 16;
pub const CHAT_INPUT_MAX: usize = 96;

/// The open window's state.
#[derive(Debug, Default)]
pub struct Messenger {
    pub open: bool,
    art: Option<MessengerArt>,
    pub selected: Option<usize>,
    hover_row: Option<usize>,
    hover_button: Option<i32>,
    pressed: Option<i32>,
    last_row_press: Option<(usize, Instant)>,
    /// lobby20: the friend row menu.
    pub menu: Option<FriendMenu>,
    /// lobby20: the conversation shown over the list and the text typed for it.
    pub chat_with: Option<String>,
    pub chat_input: String,
    /// lobby20: the 0x306 place the window was opened over (1: waiting room, where an invite is possible).
    place: u32,
    /// The tree's scroll bar (built when the window opens).
    bar: Option<ScrollBar>,
}

/// What the window did with an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    /// Not the window's: the screen gets it.
    No,
    /// Taken.
    Yes,
    /// A button of the window was clicked (the select sound plays).
    Clicked,
}

fn inside(r: [i32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] as f32
        && p[1] >= r[1] as f32
        && p[0] < (r[0] + r[2]) as f32
        && p[1] < (r[1] + r[3]) as f32
}

fn at(o: [i32; 2]) -> [i32; 2] {
    [WINDOW_AT[0] + o[0], WINDOW_AT[1] + o[1]]
}

/// The 2009 place of a screen for 0x306 when the window closes over it.
// EVIDENCE: cshell+0x12b350 (screen 9 -> 1, 0xf -> 0, 0x14 -> 3, 0x16 -> 2, else 0xe)
pub fn location_of(g: Go) -> u32 {
    match g.state_id() {
        9 => 1,
        0xf => 0,
        0x14 => 3,
        0x16 => 2,
        _ => LOCATION_OTHER,
    }
}

impl Messenger {
    /// The window's rectangle.
    pub fn rect() -> [i32; 4] {
        [WINDOW_AT[0], WINDOW_AT[1], WINDOW_SIZE[0], WINDOW_SIZE[1]]
    }

    /// The tree's top-left on the screen.
    fn tree_at() -> [i32; 2] {
        at([LIST_AT[0] + TREE_AT[0], LIST_AT[1] + TREE_AT[1]])
    }

    /// The tree's scroll bar's rectangle.
    pub fn bar_rect() -> [i32; 4] {
        let t = Self::tree_at();
        [t[0] + TREE_BAR_X, t[1], scrollbar::WIDTH, TREE_HEIGHT]
    }

    /// The tree's rows shown whole (scrolled): row, its rectangle (row bar wide, row high).
    fn shown_rows(&self, data: &MenuData) -> Vec<(TreeRow, [i32; 4])> {
        let t = Self::tree_at();
        let v = self.bar.as_ref().map_or(0, |b| b.value);
        tree_rows(&data.friends)
            .into_iter()
            .filter(|(r, y)| *y >= v && y - v + r.height() <= TREE_HEIGHT)
            .map(|(r, y)| (r, [t[0], t[1] + y - v, ROW_BAR[0], r.height()]))
            .collect()
    }

    /// The scroll bar with its range set to the tree's height.
    fn sync_bar(&mut self, data: &MenuData) {
        let content: i32 = tree_rows(&data.friends)
            .iter()
            .map(|(r, _)| r.height())
            .sum();
        if let Some(b) = &mut self.bar {
            b.step = ROW_HEIGHT;
            b.set_content(content);
        }
    }

    fn field_rect() -> [i32; 4] {
        let a = at(LIST_AT);
        [
            a[0],
            a[1] + LIST_SIZE[1] - ROW_HEIGHT,
            LIST_SIZE[0],
            ROW_HEIGHT,
        ]
    }

    /// The text buttons' rectangles (measured captions; `enabled` from the build).
    fn text_buttons(cat: &mut Catalog) -> Vec<([i32; 4], i32, i32, bool)> {
        TEXT_BUTTONS
            .iter()
            .map(|&(s, cmd, x, enabled)| {
                let t = cat.text(s);
                let (w, h) = cat
                    .measure_text(14, TEXT_BUTTON_SIZE, &t)
                    .unwrap_or((60.0, 14.0));
                let a = at([x, TEXT_BUTTON_Y]);
                (
                    [a[0], a[1], w.ceil() as i32, h.ceil() as i32],
                    s,
                    cmd,
                    enabled,
                )
            })
            .collect()
    }

    fn close_rect() -> [i32; 4] {
        let a = at(CLOSE_AT);
        [a[0], a[1], CLOSE_SIZE[0], CLOSE_SIZE[1]]
    }

    /// Opens or closes the window and tells the lobby where the player is; returns the 0x306 place to send.
    pub fn toggle(&mut self, cat: &Catalog, screen: Go) -> u32 {
        self.open = !self.open;
        self.menu = None;
        self.chat_with = None;
        self.chat_input.clear();
        self.place = location_of(screen);
        self.pressed = None;
        self.hover_button = None;
        if self.open {
            if self.art.is_none() {
                self.art = Some(
                    cat.rom
                        .as_deref()
                        .map(MessengerArt::from_rom)
                        .unwrap_or_default(),
                );
            }
            let art = self.art.clone().unwrap_or_default();
            self.bar = Some(ScrollBar::new(Self::bar_rect(), TREE_HEIGHT, art.scroll));
            LOCATION_SHOWN
        } else {
            location_of(screen)
        }
    }

    /// The command of the window's control under `p` (close button, enabled text buttons).
    fn button_at(&self, cat: &mut Catalog, p: [f32; 2]) -> Option<i32> {
        if inside(Self::close_rect(), p) {
            return Some(WINDOW_SHUT_CMD);
        }
        Self::text_buttons(cat)
            .into_iter()
            .find(|(r, _, _, enabled)| *enabled && inside(*r, p))
            .map(|(_, _, cmd, _)| cmd)
    }

    /// The friend row (index of [`Friends::rows`]) under `p`.
    fn row_at(&self, data: &MenuData, p: [f32; 2]) -> Option<usize> {
        self.shown_rows(data)
            .into_iter()
            .find_map(|(r, rect)| match r {
                TreeRow::User(i) if inside(rect, p) => Some(i),
                _ => None,
            })
    }

    /// The Close caption of the chat panel (top right of the list).
    fn chat_close_rect(cat: &mut Catalog) -> [i32; 4] {
        let t = cat.text(0x1613);
        let (w, h) = cat
            .measure_text(14, TEXT_BUTTON_SIZE, &t)
            .unwrap_or((40.0, 14.0));
        let a = at(LIST_AT);
        let (w, h) = (w.ceil() as i32, h.ceil() as i32);
        [a[0] + LIST_SIZE[0] - w - 4, a[1] + 3, w, h]
    }

    /// Opens the conversation with `name` over the list.
    pub fn open_chat(&mut self, name: &str) {
        self.chat_with = Some(name.to_string());
        self.chat_input.clear();
        self.menu = None;
    }

    /// An input while the window is open. `close` is set when the window must close (the menu sends the place);
    /// `ask` gets the boxes the window opens.
    pub fn input(
        &mut self,
        ev: &MenuInput,
        cat: &mut Catalog,
        data: &MenuData,
        send: &mut dyn FnMut(LinkCmd),
        ask: &mut Vec<Popup>,
        close: &mut bool,
    ) -> Taken {
        let in_window = |p: [f32; 2]| inside(Self::rect(), p);
        self.sync_bar(data);
        // lobby20: the friend row menu takes the pointer first
        if let Some(m) = &mut self.menu {
            match ev {
                MenuInput::Move(p) => {
                    m.hover = m.item_at(*p);
                    return Taken::Yes;
                }
                MenuInput::Press(p) => {
                    if m.item_at(*p).is_none() {
                        self.menu = None;
                    }
                    return Taken::Yes;
                }
                MenuInput::Release(p) => {
                    let item = m.item_at(*p);
                    let name = m.name.clone();
                    self.menu = None;
                    if let Some(i) = item {
                        self.act(MENU_ITEMS[i].1, &name, cat, send, ask);
                        return Taken::Clicked;
                    }
                    return Taken::Yes;
                }
                MenuInput::Key(MenuKey::Escape) => {
                    self.menu = None;
                    return Taken::Yes;
                }
                _ => {}
            }
        }
        // lobby20: the chat panel
        if let Some(to) = self.chat_with.clone() {
            match ev {
                MenuInput::Press(p) if in_window(*p) => {
                    self.pressed = if inside(Self::chat_close_rect(cat), *p) {
                        Some(CHAT_CLOSE_CMD)
                    } else if inside(Self::close_rect(), *p) {
                        Some(WINDOW_SHUT_CMD)
                    } else {
                        None
                    };
                    return Taken::Yes;
                }
                MenuInput::Release(p) if in_window(*p) => {
                    let was = self.pressed.take();
                    let now = if inside(Self::chat_close_rect(cat), *p) {
                        Some(CHAT_CLOSE_CMD)
                    } else if inside(Self::close_rect(), *p) {
                        Some(WINDOW_SHUT_CMD)
                    } else {
                        None
                    };
                    if was.is_some() && was == now {
                        if now == Some(WINDOW_SHUT_CMD) {
                            *close = true;
                        } else {
                            self.chat_with = None;
                        }
                        return Taken::Clicked;
                    }
                    return Taken::Yes;
                }
                MenuInput::Char(c) => {
                    if !c.is_control() && self.chat_input.chars().count() < CHAT_INPUT_MAX {
                        self.chat_input.push(*c);
                    }
                    return Taken::Yes;
                }
                MenuInput::Key(MenuKey::Backspace) => {
                    self.chat_input.pop();
                    return Taken::Yes;
                }
                // EVIDENCE: cshell+0xf07c0 (0x271c: the chat field's text goes to the chat's user)
                MenuInput::Key(MenuKey::Enter) => {
                    let t = std::mem::take(&mut self.chat_input).trim().to_string();
                    if !t.is_empty() {
                        send(LinkCmd::Messenger(whisper_command(&to, &t)));
                    }
                    return Taken::Yes;
                }
                MenuInput::Key(MenuKey::Escape) => {
                    self.chat_with = None;
                    return Taken::Yes;
                }
                _ => {}
            }
        }
        match ev {
            MenuInput::Move(p) if self.bar.as_mut().is_some_and(|b| b.drag_to(*p)) => Taken::Yes,
            MenuInput::Press(p)
                if self.chat_with.is_none() && self.bar.as_mut().is_some_and(|b| b.press(*p)) =>
            {
                Taken::Yes
            }
            MenuInput::Release(_) if self.bar.as_ref().is_some_and(|b| b.drag) => {
                if let Some(b) = &mut self.bar {
                    b.release();
                }
                Taken::Yes
            }
            MenuInput::Move(p) => {
                self.hover_row = self.row_at(data, *p).filter(|_| self.chat_with.is_none());
                self.hover_button = self.button_at(cat, *p);
                if in_window(*p) {
                    Taken::Yes
                } else {
                    Taken::No
                }
            }
            MenuInput::Press(p) => {
                if !in_window(*p) {
                    // EVIDENCE: cshell+0x12b590 (outside: the window closes, the press goes on)
                    *close = true;
                    return Taken::No;
                }
                self.pressed = self.button_at(cat, *p);
                if self.pressed.is_none() {
                    if let Some(i) = self.row_at(data, *p) {
                        let double = self.last_row_press.is_some_and(|(j, t)| {
                            j == i && t.elapsed().as_millis() < DOUBLE_CLICK_MS
                        });
                        self.last_row_press = (!double).then(|| (i, Instant::now()));
                        self.selected = Some(i);
                        // a double press on a friend opens the chat with it
                        if double {
                            if let Some((name, _, false)) = data.friends.rows().get(i).copied() {
                                self.open_chat(name);
                            }
                        }
                    }
                }
                Taken::Yes
            }
            MenuInput::Release(p) => {
                let was = self.pressed.take();
                let now = if in_window(*p) {
                    self.button_at(cat, *p)
                } else {
                    None
                };
                match (was, now) {
                    (Some(a), Some(b)) if a == b => {
                        self.command(a, cat, data, send, ask, close);
                        Taken::Clicked
                    }
                    _ if in_window(*p) => Taken::Yes,
                    _ => Taken::No,
                }
            }
            // lobby20: a right press on a friend opens its menu
            // EVIDENCE: cshell+0xf0d90 / 0xeb6c0 (the friend row menu)
            MenuInput::PressAlt(p) if in_window(*p) => {
                if self.chat_with.is_none() {
                    if let Some(i) = self.row_at(data, *p) {
                        if let Some((name, _, false)) = data.friends.rows().get(i).copied() {
                            self.selected = Some(i);
                            let w = MENU_WIDTH;
                            let h = MENU_ROW * MENU_ITEMS.len() as i32;
                            // inside the screen
                            let x = (p[0] as i32).min(1024 - w);
                            let y = (p[1] as i32).min(768 - h);
                            self.menu = Some(FriendMenu {
                                name: name.to_string(),
                                at: [x, y],
                                hover: None,
                            });
                        }
                    }
                }
                Taken::Yes
            }
            MenuInput::Key(MenuKey::Escape) => {
                *close = true;
                Taken::Yes
            }
            _ => Taken::No,
        }
    }

    /// A row menu entry for friend `name`.
    fn act(
        &mut self,
        item: MenuItem,
        name: &str,
        cat: &mut Catalog,
        send: &mut dyn FnMut(LinkCmd),
        ask: &mut Vec<Popup>,
    ) {
        match item {
            MenuItem::Chat => self.open_chat(name),
            // EVIDENCE: cshell+0xf0250 (0x271e: the invite is made on screen 9 only, else box 0x48b)
            MenuItem::Invite => {
                if self.place == 1 {
                    send(LinkCmd::Messenger(invite_command(name)));
                } else {
                    ask.push(Popup::ok(cat.text(0x48b), BoxTag::None));
                }
            }
            // the question 0x13b1 with the friend's name, yes removes
            MenuItem::Remove => {
                let t = cat.text(0x13b1).replace("%1!s!", name);
                ask.push(Popup::ask_name(t, BoxTag::RemoveFriend, name));
            }
        }
    }

    /// A clicked control of the window.
    fn command(
        &mut self,
        cmd: i32,
        cat: &mut Catalog,
        data: &MenuData,
        send: &mut dyn FnMut(LinkCmd),
        ask: &mut Vec<Popup>,
        close: &mut bool,
    ) {
        match cmd {
            WINDOW_SHUT_CMD => *close = true,
            // add friend: a selected request is accepted, else the name box opens
            // EVIDENCE: cshell+0xf07c0 (0xdf opens the input box 0x4c9)
            FRIEND_ADD_CMD => {
                let rows = data.friends.rows();
                match self.selected.and_then(|i| rows.get(i).copied()) {
                    Some((name, _, true)) => send(LinkCmd::Messenger(accept_command(name))),
                    _ => ask.push(Popup::add_friend(cat.text(0x4c9))),
                }
            }
            // groups are not part of the relay: the list is asked for again
            GROUP_ADD_CMD => send(LinkCmd::Messenger(LIST_COMMAND.into())),
            _ => {}
        }
    }

    /// The window over the screen.
    pub fn draw(&self, cat: &mut Catalog, data: &MenuData, out: &mut Vec<DrawItem>) {
        if !self.open {
            return;
        }
        let art = self.art.clone().unwrap_or_default();
        match &art.window {
            Some(t) if cat.tex_size(t).is_some() => {
                out.push(image_fit(cat, t, WINDOW_AT, WINDOW_SIZE))
            }
            _ => out.push(DrawItem::Fill {
                rect: Self::rect().map(|v| v as f32),
                colour: [24, 24, 24, 240],
            }),
        }
        // the selected first tab
        let centre = Align {
            first: ALIGN_CENTRE,
            second: ALIGN_SECOND_CENTRE,
        };
        let tab = at(TAB_AT);
        if let Some(prefix) = &art.tab {
            let t = format!("{prefix}C.dtx");
            let t = if cat.tex_size(&t).is_some() {
                t
            } else {
                format!("{prefix}A.dtx")
            };
            out.push(image_fit(cat, &t, tab, TAB_SIZE));
        }
        out.push(place_text(
            &cat.text(TAB_TEXT),
            [tab[0] + TAB_CAPTION_AT[0], tab[1] + TAB_CAPTION_AT[1]],
            [0, 0],
            centre,
            14,
            TEXT_BUTTON_SIZE,
            TAB_SELECTED_COLOUR,
        ));
        // close button
        if let Some(prefix) = &art.close {
            let letter = if self.hover_button == Some(WINDOW_SHUT_CMD) {
                "B"
            } else {
                "A"
            };
            let t = format!("{prefix}{letter}.dtx");
            let t = if cat.tex_size(&t).is_some() {
                t
            } else {
                format!("{prefix}A.dtx")
            };
            out.push(image_fit(cat, &t, at(CLOSE_AT), CLOSE_SIZE));
        }
        // the friend tree and its scroll bar
        let left = Align {
            first: ALIGN_LEFT,
            second: 0,
        };
        let rows = data.friends.rows();
        let icon =
            |cat: &mut Catalog, out: &mut Vec<DrawItem>, t: &Option<String>, at: [i32; 2]| {
                if let Some(t) = t {
                    if let Some((w, h)) = cat.tex_size(t) {
                        out.push(image_fit(cat, t, at, [w as i32, h as i32]));
                    }
                }
            };
        for (row, r) in self.shown_rows(data) {
            match row {
                TreeRow::Gap => {}
                TreeRow::Group(open) => {
                    let t = if open {
                        &art.group_close
                    } else {
                        &art.group_open
                    };
                    icon(cat, out, t, [r[0] + ROW_ICON_AT[0], r[1] + ROW_ICON_AT[1]]);
                    out.push(place_text(
                        GROUP_NAME,
                        [r[0] + ROW_NAME_AT[0], r[1] + ROW_NAME_AT[1]],
                        [0, 0],
                        left,
                        GROUP_FONT,
                        ROW_TEXT_SIZE,
                        GROUP_NAME_COLOUR,
                    ));
                }
                TreeRow::User(i) => {
                    let Some(&(name, online, request)) = rows.get(i) else {
                        continue;
                    };
                    if self.selected == Some(i) || self.hover_row == Some(i) {
                        match &art.bar {
                            Some(t) => out.push(image_fit(cat, t, [r[0], r[1]], ROW_BAR)),
                            None => out.push(DrawItem::Fill {
                                rect: [
                                    r[0] as f32,
                                    r[1] as f32,
                                    ROW_BAR[0] as f32,
                                    ROW_BAR[1] as f32,
                                ],
                                colour: [90, 70, 20, 255],
                            }),
                        }
                    }
                    let x = r[0] + INDENT;
                    let t = if online { &art.online } else { &art.offline };
                    icon(cat, out, t, [x + ROW_ICON_AT[0], r[1] + ROW_ICON_AT[1]]);
                    let colour = if request {
                        TAB_SELECTED_COLOUR
                    } else if online {
                        ONLINE_NAME_COLOUR
                    } else {
                        OFFLINE_NAME_COLOUR
                    };
                    out.push(place_text(
                        name,
                        [x + ROW_NAME_AT[0], r[1] + ROW_NAME_AT[1]],
                        [0, 0],
                        left,
                        USER_FONT,
                        USER_TEXT_SIZE,
                        colour,
                    ));
                }
            }
        }
        if let Some(b) = &self.bar {
            b.draw(cat, out);
        }
        self.draw_chat(cat, data, out);
        // the text buttons: normal, over, disabled colours by the caption rule
        for (r, s, cmd, enabled) in Self::text_buttons(cat) {
            let state = if !enabled {
                3
            } else if self.hover_button == Some(cmd) {
                1
            } else {
                0
            };
            out.push(place_text(
                &cat.text(s),
                [r[0], r[1]],
                [0, 0],
                Align {
                    first: ALIGN_LEFT,
                    second: 0x10,
                },
                14,
                TEXT_BUTTON_SIZE,
                TEXT_BUTTON_COLOURS[CAPTION_COLOUR[state]],
            ));
        }
        self.draw_menu(cat, out);
    }

    /// The chat panel over the list (title, wrapped lines, input line, Close).
    fn draw_chat(&self, cat: &mut Catalog, data: &MenuData, out: &mut Vec<DrawItem>) {
        let Some(to) = &self.chat_with else {
            return;
        };
        let left = Align {
            first: ALIGN_LEFT,
            second: 0,
        };
        let a = at(LIST_AT);
        out.push(DrawItem::Fill {
            rect: [
                a[0] as f32,
                a[1] as f32,
                LIST_SIZE[0] as f32,
                LIST_SIZE[1] as f32,
            ],
            colour: [20, 19, 16, 255],
        });
        let title = cat.text(0x5ab).replace("%s", to);
        out.push(place_text(
            &title,
            [a[0] + 4, a[1] + 3],
            [0, 0],
            left,
            14,
            TEXT_BUTTON_SIZE,
            TAB_SELECTED_COLOUR,
        ));
        let close = Self::chat_close_rect(cat);
        let state = if self.pressed == Some(CHAT_CLOSE_CMD) {
            2
        } else {
            0
        };
        out.push(place_text(
            &cat.text(0x1613),
            [close[0], close[1]],
            [0, 0],
            left,
            14,
            TEXT_BUTTON_SIZE,
            TEXT_BUTTON_COLOURS[CAPTION_COLOUR[state]],
        ));
        // the lines that fit between the title and the input line, newest at the bottom
        let width = (LIST_SIZE[0] - 8) as f32;
        let mut rows: Vec<(String, u32)> = Vec::new();
        if let Some(c) = data.friends.chat(to) {
            for (from, text) in &c.lines {
                let own = *from == data.friends.me;
                let colour = if own { STATUS_COLOUR } else { WHISPER_COLOUR };
                let full = format!("{from}: {text}");
                for l in wrap_by_width(&full, width, &mut |t| {
                    cat.measure_text(14, TEXT_BUTTON_SIZE, t)
                        .map_or(0.0, |m| m.0)
                }) {
                    rows.push((l, colour));
                }
            }
        }
        let top = a[1] + PANEL_PITCH + 6;
        let room = ((LIST_SIZE[1] - ROW_HEIGHT - PANEL_PITCH - 10) / PANEL_PITCH).max(1) as usize;
        let skip = rows.len().saturating_sub(room);
        for (k, (l, colour)) in rows.iter().skip(skip).enumerate() {
            out.push(place_text(
                l,
                [a[0] + 4, top + k as i32 * PANEL_PITCH],
                [0, 0],
                left,
                14,
                TEXT_BUTTON_SIZE,
                *colour,
            ));
        }
        let r = Self::field_rect();
        out.push(DrawItem::Fill {
            rect: r.map(|v| v as f32),
            colour: FIELD_FILL,
        });
        let tail = {
            let mut m = |t: &str| {
                cat.measure_text(14, TEXT_BUTTON_SIZE, t)
                    .map_or(0.0, |m| m.0)
            };
            super::sheet::fitting_tail(&self.chat_input, (r[2] - 12) as f32, &mut m).to_string()
        };
        out.push(place_text(
            &format!("{tail}|"),
            [r[0] + 4, r[1] + 3],
            [0, 0],
            left,
            14,
            TEXT_BUTTON_SIZE,
            0xffff_ffff,
        ));
    }

    /// The friend row menu.
    fn draw_menu(&self, cat: &mut Catalog, out: &mut Vec<DrawItem>) {
        let Some(m) = &self.menu else {
            return;
        };
        let r = m.rect();
        out.push(DrawItem::Fill {
            rect: r.map(|v| v as f32),
            colour: MENU_EDGE,
        });
        out.push(DrawItem::Fill {
            rect: [
                (r[0] + 1) as f32,
                (r[1] + 1) as f32,
                (r[2] - 2) as f32,
                (r[3] - 2) as f32,
            ],
            colour: MENU_FILL,
        });
        for (i, (s, _)) in MENU_ITEMS.iter().enumerate() {
            let colour = if m.hover == Some(i) {
                TAB_SELECTED_COLOUR
            } else {
                ONLINE_NAME_COLOUR
            };
            out.push(place_text(
                &cat.text(*s),
                [r[0] + 8, r[1] + i as i32 * MENU_ROW + 3],
                [0, 0],
                Align {
                    first: ALIGN_LEFT,
                    second: 0,
                },
                14,
                TEXT_BUTTON_SIZE,
                colour,
            ));
        }
    }
}

/// `text` cut into lines of at most `width` pixels, breaking between characters.
pub fn wrap_by_width(text: &str, width: f32, measure: &mut dyn FnMut(&str) -> f32) -> Vec<String> {
    let mut lines = vec![String::new()];
    for c in text.chars() {
        let cur = lines.last_mut().unwrap();
        cur.push(c);
        if measure(cur) > width && cur.chars().count() > 1 {
            cur.pop();
            lines.push(c.to_string());
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn relay_lines_build_the_friend_list() {
        let mut d = Friends::default();
        assert_eq!(d.apply(&f(&["request", "Cy"])), None);
        assert_eq!(d.apply(&f(&["friends", "Me", "Ann", "Bo"])), None);
        d.apply(&f(&["online", "Bo", "1"]));
        assert_eq!(
            d.rows(),
            vec![
                ("Ann", false, false),
                ("Bo", true, false),
                ("Cy", false, true)
            ]
        );
        // the request became a friend; the online flag survives a new list
        d.apply(&f(&["friends", "Me", "Ann", "Bo", "Cy"]));
        assert_eq!(d.requests, Vec::<String>::new());
        assert!(d.list[1].online);
        let (line, c) = d.apply(&f(&["whisper", "Ann", "Me", "hi"])).unwrap();
        assert_eq!((line.as_str(), c), ("Ann >> Me : hi", WHISPER_COLOUR));
        assert_eq!(whisper_command("Ann", "yo"), "!nm w\tAnn\tyo");
    }

    #[test]
    fn notices_while_closed_wait_and_none_while_shown() {
        let mut d = Friends::default();
        d.apply(&f(&["friends", "Me", "Ann"]));
        assert_eq!(d.unread, 0);
        // another player's whisper and a new request wait; the own (echoed) line and a repeated request do not
        d.apply(&f(&["whisper", "Ann", "Me", "hi"]));
        d.apply(&f(&["whisper", "Me", "Ann", "yo"]));
        d.apply(&f(&["request", "Cy"]));
        d.apply(&f(&["request", "Cy"]));
        assert_eq!(d.unread, 2);
        // with the window shown nothing waits
        d.unread = 0;
        d.shown = true;
        d.apply(&f(&["whisper", "Ann", "Me", "again"]));
        d.apply(&f(&["request", "Dee"]));
        assert_eq!(d.unread, 0);
        // the blink: on in the even periods, never without a period
        assert!(crate::menu::sheet::blink_on(BUTTON_BLINK, 0.1));
        assert!(!crate::menu::sheet::blink_on(BUTTON_BLINK, 0.4));
        assert!(crate::menu::sheet::blink_on(BUTTON_BLINK, 0.7));
        assert!(!crate::menu::sheet::blink_on(0.0, 0.1));
    }

    #[test]
    fn closing_reports_the_screen_place() {
        assert_eq!(location_of(Go::Lobby), 0);
        assert_eq!(location_of(Go::Room), 1);
        assert_eq!(location_of(Go::Clan), LOCATION_OTHER);
    }
}
