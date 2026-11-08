//! The menu's message box: an OK box (errors, notices), a Yes / No box (quit) and a wait box without buttons
//! (connecting). It takes every key and click while it is open; Enter presses the first button, Escape the
//! second (OK boxes close on either).
// EVIDENCE: cshell+0x26f3c0 (box build: window texture 0x1d8 x 0x100, button textures, button rectangle
// 0x78 x 0x19, warning icon)
// EVIDENCE: cshell+0xfb3b0, cshell+0x13d7eb, cshell+0x13dba5 (the box description: first word 0 = OK, 1 = two
// buttons, 4 = no buttons; word +0x2c the title string, +0x30 / +0x34 the button strings)
// EVIDENCE: cshell+0x271830 (keys: Enter presses the box's button, Escape closes every box but the one without buttons)
// EVIDENCE: cshell+0x271a20 (a box opened while one is shown waits in a queue, a box without buttons is dropped)

use super::catalog::Catalog;
use super::sheet::{image, place_text};
use super::MenuInput;
use crate::hud::compose::HudAssets;
use crate::hud::items::DrawItem;
use dc_ui_data::pages::Align;
use std::time::Instant;

/// String ids of the box captions and buttons.
// SPEC: PROTOCOL_2009.md §2.2, §3.7 (message boxes are cited by their string id)
pub mod text_ids {
    /// Box titles: error, notice.
    pub const TITLE_ERROR: i32 = 0x346;
    pub const TITLE_NOTICE: i32 = 0x371;
    /// Button captions: OK, Yes, No.
    pub const OK: i32 = 0x42;
    pub const YES: i32 = 0x35;
    pub const NO: i32 = 0x36;
    pub const CANCEL: i32 = 0x43;
    /// CLI-1132: the weapon-limit box's first button ("Go to Shop").
    pub const LIMIT_SHOP_CAPTION: i32 = 0x177e;
    /// Quit question of the login and server screens.
    pub const ASK_QUIT: i32 = 0xbd3;
    /// "Connecting to server" wait box.
    pub const CONNECTING: i32 = 0x40b;
    /// The lobby connection closed.
    pub const CONNECTION_ENDED: i32 = 0x29e;
    /// Login refusals by 0x201 reason: banned, verification, any other.
    pub const LOGIN_BANNED: i32 = 0x560;
    pub const LOGIN_VERIFY: i32 = 0x443;
    pub const LOGIN_FAILED: i32 = 0x44f;
    /// Lobby login refused.
    pub const LOBBY_REFUSED: i32 = 0x452;
    /// Server full / cannot enter.
    pub const SERVER_FULL: i32 = 0x3f5;
    /// A clan server without a clan.
    pub const SERVER_CLAN_ONLY: i32 = 0x4db;
    pub const SERVER_NO_ENTRY: i32 = 0x4dc;
    /// No server to pick.
    pub const NO_SERVER: i32 = 0x475;
    /// Channel full, channel closed.
    pub const CHANNEL_FULL: i32 = 0x1843;
    pub const CHANNEL_SHUT: i32 = 0x1012;
    /// Room create refused (no more rooms), room enter refused (password / kicked / other).
    pub const NO_MORE_ROOMS: i32 = 0x1061;
    pub const WRONG_PASSWORD: i32 = 0x1013;
    pub const KICKED_HERE: i32 = 0x14be;
    pub const CANNOT_ENTER: i32 = 0xfb1;
    /// Start refused: final round, fewer than two players.
    pub const LAST_ROUND_BUSY: i32 = 0x41b;
    pub const NEED_PLAYERS: i32 = 0x444;
    /// Settings refused while the game runs.
    pub const GAME_RUNNING: i32 = 0x4e2;
    /// Game join time-out.
    pub const TIMEOUT: i32 = 0xbd0;
    /// Removed from the room: by an elite moderator, by a GM.
    pub const KICKED_MOD: i32 = 0x1851;
    pub const KICKED_GM: i32 = 0x565;
}

/// The invitation's text form and how long its box stays up.
pub const INVITE_TEXT: i32 = 0x508;
pub const INVITE_MS: u64 = 10_000;
/// The add-friend box: title string and name length.
pub const ADD_FRIEND_TITLE: i32 = 0x1619;
pub const ADD_FRIEND_MAX: usize = 16;
/// CLI-1905: the waiting room's lock box (the Change password button): title "Lock Game Room", the text asking for a
/// 4-character password, at most 4 characters, masked.
// EVIDENCE: cshell+0x15fc80 (box 0x1036 of type 3 with title 0x10be, OK 0x42 / Cancel 0x43, empty start text, edit
// limit 3 at +0x14 and mode 8 at +0x18, callback cshell+0x15baf0); cshell+0x2f5830 (a character is refused once the
// length is past the limit, so a limit of 3 takes 4 characters)
pub const LOCK_ROOM_TITLE: i32 = 0x10be;
pub const LOCK_ROOM_TEXT: i32 = 0x1036;
pub const LOCK_ROOM_MAX: usize = 4;

/// Box placement: the window centred on the virtual screen.
// EVIDENCE: cshell+0x26f3c0 (window 0x1d8 x 0x100)
pub const BOX: [i32; 4] = [276, 256, 472, 256];
/// The input field of a type 3 box in the box.
pub const FIELD_AT: [i32; 2] = [138, 128];
pub const FIELD_SIZE: [i32; 2] = [158, 19];
pub const BUTTON: [i32; 2] = [0x78, 0x19];
/// Button row, title and text placement inside the box, and the wrap width in characters.
// EVIDENCE: cshell+0x26f3c0 / 0x270000 (CLI-912: title at (12, 7) = the globals at cshell+0x457060, text at (70, 55) =
// those at cshell+0x457048, icon at (34, 58) = those at cshell+0x457068; the reference stack's text starts at x 69)
// DC-DEFAULT: CLI-164 (proposed) wrapped at 52 characters, 18 px lines, buttons on the row 207 px down (one centred,
// two 10 px apart)
pub const TITLE_AT: [i32; 2] = [12, 7];
pub const TEXT_AT: [i32; 2] = [70, 55];
pub const ICON_AT: [i32; 2] = [34, 58];
pub const WRAP: usize = 52;
/// The body text control's width.
pub const TEXT_WIDTH: f32 = 352.0;
pub const LINE: i32 = 18;
pub const BUTTON_ROW: i32 = 207;
/// The buttons' left edges in the window and the caption's anchor in a button.
// EVIDENCE: cshell+0x26f3c0 / 0x270000 (CLI-1011: the OK button at the globals cshell+0x457018 / 0x45701c = (185, 207), Yes
// at cshell+0x457020 / 24 = (124, 207), No at cshell+0x457028 / 2c = (246, 207); the caption at (0x32, 0xc) of the button,
// alignment 4 / 0x40 = centred about that point, so the caption sits 10 px left of the 120-wide control's middle,
// in the middle of the 100-wide button picture); OBSERVED the reference stack's Yes / No at box + 123 / 246
pub const OK_AT: i32 = 185;
pub const YES_AT: i32 = 124;
pub const NO_AT: i32 = 246;
pub const CAPTION_AT: [i32; 2] = [0x32, 0xc];
/// The button captions' font, size and colour.
pub const CAPTION_FONT: i32 = 14;
pub const CAPTION_SIZE: i32 = 13;
pub const CAPTION_COLOUR: u32 = 0xffee_eeee;

/// What closing the box does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxTag {
    /// Nothing (information).
    None,
    /// Yes quits the client.
    Quit,
    /// Back to the login screen.
    ToLogin,
    /// The password of room `n` (input box).
    RoomPassword(u32),
    /// w2-lobbychat: an invitation to room `n` (Yes joins it with the password the box keeps in `input`).
    Invite(u32),
    /// w3-shop: yes buys product `n`.
    Buy(u32),
    /// lobby-3: the purchase box of a new item; its OK asks whether to equip it (unique id, item number).
    Bought { uid: u32, item: u32 },
    /// lobby-3: the "equip now or keep in storage" box; its first button equips the bought item.
    EquipBought { uid: u32, item: u32 },
    /// storage-functions: the discard confirmation; yes discards the owned item (unique id, item number).
    Discard { uid: u32, item: u32 },
    /// storage-functions: the short-of-NX box; its first button (Charge NX) shows the charge page box.
    NxShort,
    /// storage-functions: the last question of a credit purchase (0x17dd); yes buys product `n`.
    BuyNx(u32),
    /// round 10: yes opens inbox entry `entry` of product `product` (0x32e).
    OpenGift { entry: u32, product: u32 },
    /// lobby20: the messenger's add-friend input box; its OK sends the typed name.
    AddFriend,
    /// lobby20: the remove-friend question (the name sits in `input`); yes removes it.
    RemoveFriend,
    /// lobby20: a friend request (the name sits in `input`); yes accepts it, no declines it.
    FriendRequest,
    /// CLI-1901: a clan leader's invitation (the clan name sits in `input`, the clan number here); yes joins, no
    /// declines, Escape leaves it open.
    ClanInvite(u32),
    /// CLI-1132: the weapon-limit box of a room; Yes (the "Go to Shop" button) opens the shop's weapons page.
    LimitBox,
    /// CLI-1905: the waiting room's lock box; OK sets the typed password as the room's (empty: no password).
    LockRoom,
    /// gm-client (CLI-1303): the kick question; Yes kicks the member (in-channel id, character number).
    Kick { id: u32, char_no: u32 },
    /// use-windows: a use window's question (weapon change 0x1876, record reset); its first button sends the use of
    /// item `uid` (CLI-1370, CLI-1371).
    UseConfirm { uid: u32 },
    /// use-items-sweep: the colour window's in-use question for item `uid` (CLI-1877; [`Popup::chosen`] tells the
    /// three-button form's answer).
    ColourInUse { uid: u32 },
    /// CLI-1841: a box of the diagnostic reports (`menu::diagbox`); the chosen button is [`Popup::chosen`].
    Diag(super::diagbox::Step),
    /// CLI-1863: a box of a newer client (`menu::updatebox`); the chosen button is [`Popup::chosen`].
    Update(super::updatebox::Step),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    Ok,
    YesNo,
    Wait,
    /// A text box with OK / Cancel (room password).
    Input,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Popup {
    pub kind: BoxKind,
    pub title: i32,
    pub text: String,
    pub tag: BoxTag,
    /// The typed text of an input box (masked when drawn); an invitation keeps the room's password here.
    pub input: String,
    /// lobby20: the input box shows what is typed (the room password box masks it) and holds this many characters.
    pub plain: bool,
    pub max_len: usize,
    /// w2-lobbychat: the box closes by itself (as No) at this time.
    pub expires: Option<Instant>,
    /// String ids of the two buttons of a yes / no box (`None`: Yes / No).
    pub captions: Option<[i32; 2]>,
    hover: Option<usize>,
    down: Option<usize>,
    /// CLI-1131: the focused button (the first one when the box opens; the mouse moves it).
    focus: usize,
    /// CLI-912: the box shows the OK icon (a purchase success) instead of the warning icon.
    pub ok_icon: bool,
    /// CLI-960: a HiSec case question (window 10 of the inbox screen) draws the case and key images and the keys-left
    /// line, with the window's own art and button place.
    pub hisec: Option<HisecBox>,
    /// crate-result-window: the supply-case award window (its own art, picture, period line and OK button).
    pub award: Option<super::award::AwardBox>,
    /// CLI-1841: our own button captions (two or three buttons in a row; a yes / no box only), and the button that
    /// closed the box (`None`: Escape).
    pub own_captions: Option<Vec<String>>,
    pub chosen: Option<usize>,
}

/// The art and places of the HiSec case window (window 10 of the shop screen's layout page), read from the ROM.
// EVIDENCE: cshell+0x148e30 (the HiSec case window: controls 0 / 1 take the case's and the key's InventoryIcon
// (record +0x328), control 4 (open question, 0x18ba) or 5 (key needed, 0x18b9) the text, control 6 the keys-left
// line "N Keys Remain" / "1 Key Remains" in ^0xffe7ae54^ with the count at the owned key's +0x14)
// DC-DEFAULT: CLI-960 the text wraps by character count (rect width / 6), the keys-left line ends at the record's
// x; control 2 (a placeholder texture in the record) is not drawn; Yes / No stand side by side at the record's
// button row
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HisecBox {
    pub win: [i32; 4],
    pub bg: String,
    pub case: (String, [i32; 2]),
    pub key: (String, [i32; 2]),
    pub text_at: [i32; 2],
    pub wrap: usize,
    pub keys_text: Option<String>,
    pub keys_at: [i32; 2],
    pub button: [i32; 4],
}

impl HisecBox {
    /// Built from the layout; `ask` = the open question (else the key-needed box). `keys` = the owned count.
    pub fn build(
        cat: &Catalog,
        case_icon: &str,
        key_icon: &str,
        ask: bool,
        keys: u32,
    ) -> Option<HisecBox> {
        use dc_ui_data::pages::Widget;
        for page in &cat.pages.pages {
            let Some(win) = page.widgets.iter().find_map(|w| match &w.widget {
                Widget::Window(w) if w.control_id == 10 && w.texture.contains("Hisec") => Some(w),
                _ => None,
            }) else {
                continue;
            };
            let at = |p: [i32; 2]| [win.pos[0] + p[0], win.pos[1] + p[1]];
            let mut out = HisecBox {
                win: [
                    win.pos[0],
                    win.pos[1],
                    win.source_rect[2],
                    win.source_rect[3],
                ],
                bg: win.texture.clone(),
                case: (case_icon.to_string(), [0, 0]),
                key: (key_icon.to_string(), [0, 0]),
                text_at: [0, 0],
                wrap: 30,
                keys_text: None,
                keys_at: [0, 0],
                button: [0, 0, 0, 0],
            };
            let text_id = if ask { 4 } else { 5 };
            for w in &page.widgets {
                match &w.widget {
                    Widget::Image(i) if i.parent == 10 && i.control_id == 0 => {
                        out.case.1 = at(i.pos)
                    }
                    Widget::Image(i) if i.parent == 10 && i.control_id == 1 => {
                        out.key.1 = at(i.pos)
                    }
                    Widget::Button(b) if b.parent == 10 && b.control_id == 3 => {
                        out.button = [
                            win.pos[0] + b.pos[0],
                            win.pos[1] + b.pos[1],
                            b.source_rect[2],
                            b.source_rect[3],
                        ]
                    }
                    Widget::Text(t) if t.parent == 10 && t.control_id == text_id => {
                        out.text_at = at(t.pos);
                        out.wrap = (t.rect[2] / 6).max(8) as usize;
                    }
                    Widget::Text(t) if t.parent == 10 && t.control_id == 6 && ask => {
                        out.keys_at = at(t.pos);
                    }
                    _ => {}
                }
            }
            if ask {
                // the literals are the client's own (cshell+0x148e30), English only
                out.keys_text = Some(if keys <= 1 {
                    "^0xffe7ae54^1^ Key Remains".to_string()
                } else {
                    format!("^0xffe7ae54^{keys}^ Keys Remain")
                });
            }
            return Some(out);
        }
        None
    }
}

impl Popup {
    pub fn ok(text: String, tag: BoxTag) -> Popup {
        Popup {
            kind: BoxKind::Ok,
            title: text_ids::TITLE_NOTICE,
            text,
            tag,
            input: String::new(),
            plain: false,
            max_len: 8,
            expires: None,
            captions: None,
            hover: None,
            focus: 0,
            down: None,
            ok_icon: false,
            hisec: None,
            award: None,
            own_captions: None,
            chosen: None,
        }
    }

    /// CLI-1841: a question with our own two or three button captions (the first is the default of Enter).
    pub fn choice(text: String, tag: BoxTag, captions: &[&str]) -> Popup {
        Popup {
            own_captions: Some(captions.iter().take(3).map(|c| c.to_string()).collect()),
            ..Popup::ask(text, tag)
        }
    }

    /// An invitation to room `number` from `host` (strings 0x508: `%1!d!` the room number + 1, `%2!s!` the host),
    /// Yes / No, closing as No after [`INVITE_MS`].
    // SPEC: PROTOCOL_2009.md §3.7 (0x3023: Yes -> 0x384)
    // EVIDENCE: cshell+0x39110 (only on screen 0xf; FormatMessage of 0x508 (or 0x50c when shell +0x12c is set) with
    // the room number + 1 and the host's name; title 0x371, buttons 0x35 / 0x36; timer 0x2711 of 10000 ms; the room
    // number and the password kept for the Yes)
    // DC-DEFAULT: CLI-486 (proposed) the 0x508 form always (the shell +0x12c flag is not tracked)
    pub fn invite(cat: &Catalog, number: u32, host: &str, password: &str, now: Instant) -> Popup {
        let form = cat.text(INVITE_TEXT);
        let n = (number + 1).to_string();
        let text = form
            .replace("%1!d!", &n)
            .replace("%2!s!", host)
            .replace("%1", &n)
            .replace("%2", host);
        Popup {
            input: password.to_string(),
            expires: Some(now + std::time::Duration::from_millis(INVITE_MS)),
            ..Popup::ask(text, BoxTag::Invite(number))
        }
    }

    /// The box's time limit has passed at `now`.
    pub fn expired(&self, now: Instant) -> bool {
        self.expires.is_some_and(|t| now >= t)
    }

    pub fn error(text: String, tag: BoxTag) -> Popup {
        Popup {
            title: text_ids::TITLE_ERROR,
            ..Popup::ok(text, tag)
        }
    }

    /// CLI-1132: the box's buttons read 0x177e ("Go to Shop") and 0x43.
    pub fn limit_buttons(self) -> Popup {
        Popup {
            captions: Some([text_ids::LIMIT_SHOP_CAPTION, text_ids::CANCEL]),
            ..self
        }
    }

    pub fn ask(text: String, tag: BoxTag) -> Popup {
        Popup {
            kind: BoxKind::YesNo,
            ..Popup::ok(text, tag)
        }
    }

    /// A password box (room password: at most 8 characters, the 0x384 field holds 9 units with its end).
    // SPEC: PROTOCOL_2009.md §3.7 (0x384 +0xa wstr[9] password)
    // EVIDENCE: cshell+0x129e00 (enter on a locked room opens the password box; its command 0x53 enters with the
    // typed password, an empty one does nothing)
    pub fn password(text: String, room: u32) -> Popup {
        Popup {
            kind: BoxKind::Input,
            ..Popup::ok(text, BoxTag::RoomPassword(room))
        }
    }

    /// lobby20: the input box of the messenger's add friend (title 0x1619, text 0x4c9, 16 characters, typed text shown).
    // EVIDENCE: cshell+0xf07c0 (command 0xdf: box 0x4c9 of type 3 with title 0x1619, OK 0x42 / Cancel 0x43, the edit
    // control limited to 0x10 characters, its callback LAB_371effa0)
    pub fn add_friend(text: String) -> Popup {
        Popup {
            kind: BoxKind::Input,
            title: ADD_FRIEND_TITLE,
            plain: true,
            max_len: ADD_FRIEND_MAX,
            ..Popup::ok(text, BoxTag::AddFriend)
        }
    }

    /// CLI-1905: the host's lock box of the waiting room (masked, [`LOCK_ROOM_MAX`] characters).
    pub fn lock_room(text: String) -> Popup {
        Popup {
            kind: BoxKind::Input,
            title: LOCK_ROOM_TITLE,
            max_len: LOCK_ROOM_MAX,
            ..Popup::ok(text, BoxTag::LockRoom)
        }
    }

    /// lobby20: a yes / no question with a name kept for the answer.
    pub fn ask_name(text: String, tag: BoxTag, name: &str) -> Popup {
        Popup {
            input: name.to_string(),
            ..Popup::ask(text, tag)
        }
    }

    pub fn wait(text: String) -> Popup {
        Popup {
            kind: BoxKind::Wait,
            ..Popup::ok(text, BoxTag::None)
        }
    }

    /// The button rectangles (x, y, w, h).
    pub fn buttons(&self) -> Vec<[i32; 4]> {
        if let Some(a) = &self.award {
            return vec![a.button()];
        }
        if let Some(h) = &self.hisec {
            let [x, y, w, bh] = h.button;
            return match self.kind {
                BoxKind::YesNo => {
                    let mid = h.win[0] + h.win[2] / 2;
                    vec![[mid - 5 - w, y, w, bh], [mid + 5, y, w, bh]]
                }
                _ => vec![[x, y, w, bh]],
            };
        }
        let y = BOX[1] + BUTTON_ROW;
        // CLI-1841: three buttons of our own in a row, centred in the box (10 px apart)
        if let Some(c) = self.own_captions.as_ref().filter(|c| c.len() == 3 && self.kind == BoxKind::YesNo) {
            let left = BOX[0] + (BOX[2] - 3 * BUTTON[0] - 2 * 10) / 2;
            return (0..c.len() as i32)
                .map(|i| [left + i * (BUTTON[0] + 10), y, BUTTON[0], BUTTON[1]])
                .collect();
        }
        match self.kind {
            BoxKind::Wait => vec![],
            BoxKind::Ok => vec![[BOX[0] + OK_AT, y, BUTTON[0], BUTTON[1]]],
            BoxKind::YesNo | BoxKind::Input => vec![
                [BOX[0] + YES_AT, y, BUTTON[0], BUTTON[1]],
                [BOX[0] + NO_AT, y, BUTTON[0], BUTTON[1]],
            ],
        }
    }

    fn button_at(&self, p: [f32; 2]) -> Option<usize> {
        self.buttons().iter().position(|r| {
            p[0] >= r[0] as f32
                && p[1] >= r[1] as f32
                && p[0] < (r[0] + r[2]) as f32
                && p[1] < (r[1] + r[3]) as f32
        })
    }

    /// CLI-1131: the focused button, drawn in the highlighted look while the mouse is elsewhere.
    // DISASM: cshell+0x270000 ends each case with cshell+0x2f8540 / cshell+0x2f8610 (find the button's index in the
    // window's control list and focus it, which calls the button's vtable +0x90 with 1); the callers cshell+0x271c10 /
    // cshell+0x271b60 all pass the focus flag 1, so the OK button / the Yes button is the focused one.
    // EVIDENCE: cshell+0x2f8610 (the previous focus gets +0x90(0), the new one +0x90(1))
    fn focus_of(&self) -> Option<usize> {
        let n = self.buttons().len();
        (n > 0).then(|| self.focus.min(n - 1))
    }

    /// Takes an input; `Some(true)` = closed with the first button (OK / Yes), `Some(false)` = closed with the
    /// second (No) or Escape.
    pub fn input(&mut self, ev: &MenuInput) -> Option<bool> {
        if self.kind == BoxKind::Wait {
            return None;
        }
        // the award window's OK button is disabled until the parts appear
        // EVIDENCE: cshell+0x285b60 (button +0x38 disabled on show), cshell+0x285a80 (enabled after 1000 ms)
        if let Some(a) = self.award.as_mut().filter(|a| !a.revealed) {
            // CRATE-5: a click, Enter or Escape during the reel stops the strip at once
            if matches!(
                ev,
                MenuInput::Release(_)
                    | MenuInput::Key(super::MenuKey::Enter)
                    | MenuInput::Key(super::MenuKey::Escape)
            ) {
                a.skip(Instant::now());
            }
            return None;
        }
        match ev {
            MenuInput::Move(p) => {
                self.hover = self.button_at(*p);
                if let Some(i) = self.hover {
                    self.focus = i;
                }
            }
            MenuInput::Press(p) => self.down = self.button_at(*p),
            MenuInput::Release(p) => {
                let b = self.button_at(*p);
                let was = self.down.take();
                if b.is_some() && b == was {
                    self.chosen = b;
                    return Some(b == Some(0));
                }
            }
            MenuInput::Char(c) if self.kind == BoxKind::Input => {
                if !c.is_control() && self.input.chars().count() < self.max_len {
                    self.input.push(*c);
                }
            }
            MenuInput::Key(super::MenuKey::Backspace) if self.kind == BoxKind::Input => {
                self.input.pop();
            }
            // CLI-1898: a friend request arrives unasked, often while the player types a chat line: the Enter that
            // sends the line must not accept it, so only a click on Yes does
            // DC-DEFAULT: CLI-1898 (proposed; amends CLI-842, deviates from the 2009 box keys of cshell+0x271830)
            MenuInput::Key(super::MenuKey::Enter) if self.tag == BoxTag::FriendRequest => {}
            // CLI-1901: the clan invitation arrives unasked too: only a click answers it
            // DC-DEFAULT: CLI-1901 (proposed; as CLI-1898)
            MenuInput::Key(super::MenuKey::Enter) if matches!(self.tag, BoxTag::ClanInvite(_)) => {}
            MenuInput::Key(super::MenuKey::Enter) => {
                self.chosen = Some(0);
                return Some(true);
            }
            MenuInput::Key(super::MenuKey::Escape) => return Some(self.kind == BoxKind::Ok),
            _ => {}
        }
        None
    }

    /// crate-result-window: runs the award window's reveal; `true` when the box changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        self.award.as_mut().is_some_and(|a| a.tick(now))
    }

    /// CRATE-5: the award reel's interface sounds since the last call.
    pub fn take_sounds(&mut self) -> Vec<super::reel::Sound> {
        self.award.as_mut().map(|a| a.take_sounds()).unwrap_or_default()
    }

    /// The box over the screen.
    pub fn draw(&self, cat: &mut Catalog, out: &mut Vec<DrawItem>) {
        if let Some(a) = &self.award {
            // the looks as the message box's buttons: over (or focused) 1, pressed 2
            let look = match (self.hover.or(self.focus_of()), self.down) {
                (Some(0), Some(0)) => 2,
                (Some(0), _) => 1,
                _ => 0,
            };
            a.draw(cat, look, out);
            return;
        }
        let captions = match self.kind {
            BoxKind::Ok => vec![text_ids::OK],
            BoxKind::YesNo => self
                .captions
                .map_or(vec![text_ids::YES, text_ids::NO], |c| c.to_vec()),
            BoxKind::Input => vec![text_ids::OK, text_ids::CANCEL],
            BoxKind::Wait => vec![],
        };
        let texts = BoxTexts {
            title: cat.text(self.title),
            body: self.text.clone(),
            captions: match &self.own_captions {
                Some(c) if self.kind == BoxKind::YesNo => c.clone(),
                _ => captions.iter().map(|id| cat.text(*id)).collect(),
            },
        };
        let art = BoxArt {
            window: cat.paths.popup.clone(),
            buttons: cat.paths.popup_buttons.clone(),
            warning: if self.ok_icon {
                // the OK icon's texture is not in the shipped archives: the box then shows no icon, as the reference stack
                cat.paths.ok_icon.clone()
            } else {
                cat.paths.warning.clone()
            },
        };
        let sx = cat.scaler().sx;
        if let Some(h) = &self.hisec {
            let state = (self.hover.or(self.focus_of()), self.down);
            hisec_items(
                &mut cat.assets,
                h,
                &art,
                &texts,
                &self.buttons(),
                state,
                sx,
                &self.text,
                out,
            );
            return;
        }
        let shown = if self.plain {
            self.input.clone()
        } else {
            "*".repeat(self.input.chars().count())
        };
        let input = (self.kind == BoxKind::Input).then_some(shown.as_str());
        box_items(
            &mut cat.assets,
            &art,
            &texts,
            &self.buttons(),
            input,
            (self.hover.or(self.focus_of()), self.down),
            sx,
            out,
        );
    }
}

/// The textures of a message box (the shell's code literals, `catalog::CodePaths`): window, the three button
/// states, the warning icon.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoxArt {
    pub window: Option<String>,
    pub buttons: [Option<String>; 3],
    pub warning: Option<String>,
}

/// The texts of one box: title, body and the button captions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoxTexts {
    pub title: String,
    pub body: String,
    pub captions: Vec<String>,
}

/// The HiSec case window over the screen: its art, the case and key images, the question and the keys-left line, and
/// the buttons.
#[allow(clippy::too_many_arguments)]
fn hisec_items(
    assets: &mut HudAssets,
    h: &HisecBox,
    art: &BoxArt,
    texts: &BoxTexts,
    buttons: &[[i32; 4]],
    state: (Option<usize>, Option<usize>),
    sx: f32,
    body: &str,
    out: &mut Vec<DrawItem>,
) {
    out.push(fit(
        assets,
        &h.bg,
        [h.win[0], h.win[1]],
        [h.win[2], h.win[3]],
    ));
    for (tex, at) in [&h.case, &h.key] {
        if let Some(t) = assets.texture(tex) {
            out.push(image(tex, *at, [t.width as i32, t.height as i32]));
        }
    }
    for (i, line) in wrap(body, h.wrap).iter().enumerate() {
        out.push(place_text(
            line,
            [h.text_at[0], h.text_at[1] + i as i32 * 13],
            [0, 0],
            Align::default(),
            25,
            11,
            0xffff_ffff,
        ));
    }
    if let Some(k) = &h.keys_text {
        let atlas = assets.fonts.atlas_sized(14, Some(14));
        let measure = |seg: &str| {
            atlas
                .as_ref()
                .map_or(seg.chars().count() as f32 * 7.0, |a| {
                    a.measure(seg) / sx.max(f32::EPSILON)
                })
        };
        let runs = wrap_runs(k, 200, 0xffff_ffff)
            .into_iter()
            .next()
            .unwrap_or_default();
        let width: f32 = runs.iter().map(|(seg, _)| measure(seg)).sum();
        let mut x = h.keys_at[0] as f32 - width;
        for (seg, colour) in &runs {
            out.push(place_text(
                seg,
                [x.round() as i32, h.keys_at[1]],
                [0, 0],
                Align::default(),
                14,
                14,
                *colour,
            ));
            x += measure(seg);
        }
    }
    let (hover, down) = state;
    for (i, r) in buttons.iter().enumerate() {
        let look = if hover == Some(i) && down == Some(i) {
            2
        } else if hover == Some(i) {
            1
        } else {
            0
        };
        match &art.buttons[look] {
            Some(t) => out.push(fit(assets, t, [r[0], r[1]], [r[2], r[3]])),
            None => out.push(DrawItem::Fill {
                rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
                colour: [70, 70, 70, 255],
            }),
        }
        let label = texts.captions.get(i).cloned().unwrap_or_default();
        out.push(place_text(
            &label,
            [r[0], r[1]],
            [r[2], r[3]],
            Align {
                first: super::sheet::ALIGN_CENTRE,
                second: dc_ui_data::pages::ALIGN_SECOND_CENTRE,
            },
            14,
            13,
            0xffee_eeee,
        ));
    }
}

/// An image clamped to its texture's size (unknown textures keep the box size), from the assets.
fn fit(assets: &mut HudAssets, tex: &str, at: [i32; 2], size: [i32; 2]) -> DrawItem {
    let (w, h) = match assets.texture(tex) {
        Some(t) => (size[0].min(t.width as i32), size[1].min(t.height as i32)),
        None => (size[0], size[1]),
    };
    image(tex, at, [w, h])
}

/// A message box's draw items over the screen: the dim, the window, title, icon, wrapped body, the input field
/// (`input`: typed text, drawn masked) and the buttons `buttons` with their captions (`state`: hovered / pressed
/// button). The menu's boxes and the in-match leave box draw through this.
pub fn box_items(
    assets: &mut HudAssets,
    art: &BoxArt,
    texts: &BoxTexts,
    buttons: &[[i32; 4]],
    input: Option<&str>,
    state: (Option<usize>, Option<usize>),
    sx: f32,
    out: &mut Vec<DrawItem>,
) {
    // the screen under the box is not dimmed
    // EVIDENCE: cshell+0x26f3c0 / cshell+0x270000 (the box builder and the show routine create the window, the
    // title / text / buttons and the icon controls only; no overlay over the screen), and the reference client draws none
    // (storage-functions/cpp/item1/shop_bb01_discard_ask.png). CLI-861 drops the proposed
    // 50 % dim of CLI-164.
    // CLI-1094 (playtest report: "icon scaling is wrong") no icon: the box builder makes the three icon controls
    // (+0x34, ok_icon +0x38, warning_icon +0x3c, cshell+0x26f3c0: created with a 0 x 0 size by cshell+0x3445b0) but
    // adds none of them to a box window (its 0x138 calls add the title, text, buttons and input only), and the show
    // routine's cshell+0x2f8570 moves only a control the window holds, so the shown flag (icon kind 2 from every
    // caller, cshell+0x270000) draws nothing; the reference client draws none either (shop-r12/pairs/
    // item4_*). CLI-1012's warning triangle (64 x 64 over the body text's start, stretched with the window) is gone.
    // EVIDENCE: cshell+0x26f3c0, cshell+0x270000, cshell+0x2f8570, cshell+0x3445b0
    match &art.window {
        Some(t) => out.push(fit(assets, t, [BOX[0], BOX[1]], [BOX[2], BOX[3]])),
        None => out.push(DrawItem::Fill {
            rect: [BOX[0] as f32, BOX[1] as f32, BOX[2] as f32, BOX[3] as f32],
            colour: [24, 24, 24, 240],
        }),
    }
    out.push(place_text(
        &texts.title,
        [BOX[0] + TITLE_AT[0], BOX[1] + TITLE_AT[1]],
        [0, 0],
        Align::default(),
        14,
        14,
        TITLE_COLOUR,
    ));
    if texts.body.contains("^0x") {
        // colour codes: the coloured runs of each line, drawn one after the other
        // EVIDENCE: cshell+0x2ebc10 (the text item splits at "^0x<argb>^text^" and draws the runs in their colours)
        let atlas = assets.fonts.atlas_sized(14, Some(14));
        for (i, line) in wrap_runs(&texts.body, WRAP, BODY_COLOUR).iter().enumerate() {
            let mut x = (BOX[0] + TEXT_AT[0]) as f32;
            for (seg, colour) in line {
                out.push(place_text(
                    seg,
                    [x.round() as i32, BOX[1] + TEXT_AT[1] + i as i32 * LINE],
                    [0, 0],
                    Align::default(),
                    14,
                    14,
                    *colour,
                ));
                x += atlas
                    .as_ref()
                    .map_or(seg.chars().count() as f32 * 7.0, |a| {
                        a.measure(seg) / sx.max(f32::EPSILON)
                    });
            }
        }
    } else {
        // CLI-1010: the text control is 0x160 wide (cshell+0x270000: its rectangle 0x160 x 0 before the text is set), so
        // the lines break at 352 virtual pixels by the font's own widths, not at a count of characters
        let atlas = assets.fonts.atlas_sized(14, Some(14));
        let measure = |seg: &str| {
            atlas
                .as_ref()
                .map_or(seg.chars().count() as f32 * 7.0, |a| {
                    a.measure(seg) / sx.max(f32::EPSILON)
                })
        };
        for (i, line) in wrap_px(&texts.body, TEXT_WIDTH, measure).iter().enumerate() {
            out.push(place_text(
                line,
                [BOX[0] + TEXT_AT[0], BOX[1] + TEXT_AT[1] + i as i32 * LINE],
                [0, 0],
                Align::default(),
                14,
                14,
                BODY_COLOUR,
            ));
        }
    }
    if let Some(typed) = input {
        // OBSERVED: the reference stack's input box (1024 x 768): the field (82, 80, 77) at box + (138, 128), 158 x 19, a light
        // 1 px line on its right and bottom, the text white 2 px in
        let (x, y) = (BOX[0] + FIELD_AT[0], BOX[1] + FIELD_AT[1]);
        out.push(DrawItem::Fill {
            rect: [
                x as f32,
                y as f32,
                FIELD_SIZE[0] as f32,
                FIELD_SIZE[1] as f32,
            ],
            colour: [82, 80, 77, 255],
        });
        out.push(DrawItem::Fill {
            rect: [
                x as f32,
                (y + FIELD_SIZE[1] - 1) as f32,
                FIELD_SIZE[0] as f32,
                1.0,
            ],
            colour: [190, 189, 189, 255],
        });
        out.push(DrawItem::Fill {
            rect: [
                (x + FIELD_SIZE[0] - 1) as f32,
                y as f32,
                1.0,
                FIELD_SIZE[1] as f32,
            ],
            colour: [190, 189, 189, 255],
        });
        out.push(place_text(
            &format!("{typed}|"),
            [x + 2, y + 2],
            [0, 0],
            Align::default(),
            14,
            14,
            0xffff_ffff,
        ));
    }
    let (hover, down) = state;
    for (i, r) in buttons.iter().enumerate() {
        let look = if hover == Some(i) && down == Some(i) {
            2
        } else if hover == Some(i) {
            1
        } else {
            0
        };
        match &art.buttons[look] {
            Some(t) => out.push(fit(assets, t, [r[0], r[1]], [r[2], r[3]])),
            None => out.push(DrawItem::Fill {
                rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
                colour: [70, 70, 70, 255],
            }),
        }
        let label = texts.captions.get(i).cloned().unwrap_or_default();
        out.push(place_text(
            &label,
            [r[0] + CAPTION_AT[0], r[1] + CAPTION_AT[1]],
            [0, 0],
            Align {
                first: super::sheet::ALIGN_CENTRE,
                second: dc_ui_data::pages::ALIGN_SECOND_CENTRE,
            },
            14,
            13,
            0xffee_eeee,
        ));
    }
}

/// The body text colour.
const BODY_COLOUR: u32 = 0xffee_eeee;
/// CLI-1130: the title colour of a message box.
// DISASM: cshell+0x26f3c0 creates the title (+0x64) and the body (+0x18) as the same text control class (cshell+0x1ab50)
// and cshell+0x270000 sets no colour on either (it only sets the size, the text and the font size, vtable +0x140 /
// +0x150 / +0x154), so both draw in the class's one default colour; the reference client's title is white like its body
// (OBSERVED: ui-r12/pairs, 2560 x 1440).
const TITLE_COLOUR: u32 = 0xffff_ffff;

/// [`wrap`] for a text with colour codes: every line as runs of (text, colour); the codes take no width.
pub fn wrap_runs(text: &str, width: usize, base: u32) -> Vec<Vec<(String, u32)>> {
    // one (char, colour) per visible character, line breaks kept
    let mut chars: Vec<(char, u32)> = Vec::new();
    for (t, c) in super::shop::colour_runs(text, base) {
        chars.extend(t.chars().map(|ch| (ch, c)));
    }
    let mut lines: Vec<Vec<(char, u32)>> = Vec::new();
    for para in chars.split(|(c, _)| *c == '\n') {
        let mut line: Vec<(char, u32)> = Vec::new();
        for word in para
            .split(|(c, _)| c.is_whitespace())
            .filter(|w| !w.is_empty())
        {
            if !line.is_empty() && line.len() + 1 + word.len() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push((' ', line.last().map_or(base, |l| l.1)));
            }
            line.extend_from_slice(word);
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
        .into_iter()
        .map(|l| {
            let mut runs: Vec<(String, u32)> = Vec::new();
            for (ch, c) in l {
                match runs.last_mut() {
                    Some((s, rc)) if *rc == c => s.push(ch),
                    _ => runs.push((ch.to_string(), c)),
                }
            }
            runs
        })
        .collect()
}

/// The two-button row of a Yes / No box (x, y, w, h).
pub fn yes_no_buttons() -> Vec<[i32; 4]> {
    Popup::ask(String::new(), BoxTag::None).buttons()
}

/// Splits `text` into lines no wider than `width` (as `measure` reports a string) at spaces (and at line breaks);
/// a single word wider than the line keeps its own line.
pub fn wrap_px(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && measure(&format!("{line} {word}")) > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

/// Splits `text` into lines of at most `width` characters at spaces (and at line breaks).
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::MenuKey;

    #[test]
    fn wrap_breaks_at_spaces() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("x\ny", 10), vec!["x", "y"]);
    }

    #[test]
    fn keys_and_clicks_close_the_box() {
        let mut p = Popup::ask("q".into(), BoxTag::Quit);
        assert_eq!(p.input(&MenuInput::Key(MenuKey::Escape)), Some(false));
        let b = p.buttons()[0];
        let c = [(b[0] + 5) as f32, (b[1] + 5) as f32];
        assert_eq!(p.input(&MenuInput::Press(c)), None);
        assert_eq!(p.input(&MenuInput::Release(c)), Some(true));
        let mut w = Popup::wait("w".into());
        assert_eq!(w.input(&MenuInput::Key(MenuKey::Enter)), None);
    }

    #[test]
    fn a_message_box_does_not_dim_the_screen() {
        let mut cat = Catalog::fallback(None, 1024, 768);
        let art = BoxArt {
            window: None,
            buttons: [None, None, None],
            warning: Some("icon".into()),
        };
        let mut out = Vec::new();
        box_items(
            &mut cat.assets,
            &art,
            &BoxTexts {
                title: "t".into(),
                body: "b".into(),
                captions: vec![],
            },
            &[],
            None,
            (None, None),
            1.0,
            &mut out,
        );
        // the only Fill is the window's fallback rectangle, never a full-screen overlay
        assert!(!out.iter().any(
            |i| matches!(i, DrawItem::Fill { rect, .. } if rect[2] >= 1024.0 && rect[3] >= 768.0)
        ));
    }

    /// CLI-912: the body text starts at box + (70, 55) and the title at (12, 7), as the 2009 box builder places them
    /// (the text used to start at (60, 62), beside a warning icon the success box does not show).
    #[test]
    fn box_text_sits_where_the_2009_builder_puts_it() {
        let mut cat = Catalog::fallback(None, 1024, 768);
        let mut out = Vec::new();
        box_items(
            &mut cat.assets,
            &BoxArt::default(),
            &BoxTexts {
                title: "t".into(),
                body: "b".into(),
                captions: vec![],
            },
            &[],
            None,
            (None, None),
            1.0,
            &mut out,
        );
        assert_eq!((TITLE_AT, TEXT_AT, ICON_AT), ([12, 7], [70, 55], [34, 58]));
        let _ = out;
    }

    /// CLI-1010 / CLI-1011: the buttons sit at the 2009 globals (OK 185, Yes 124, No 246, row 207) and each caption is
    /// centred about (50, 12) of its button, the middle of the 100-wide picture (it sat 10 px right of it); the body
    /// breaks at the text control's 352 px, not at 52 characters.
    #[test]
    fn buttons_captions_and_wrap_follow_the_2009_box() {
        let ok = Popup::ok("x".into(), BoxTag::None);
        assert_eq!(ok.buttons()[0][..2], [BOX[0] + 185, BOX[1] + 207]);
        let yn = Popup::ask("q".into(), BoxTag::None).buttons();
        assert_eq!(
            (yn[0][0] - BOX[0], yn[1][0] - BOX[0], yn[0][1] - BOX[1]),
            (124, 246, 207)
        );
        let mut cat = Catalog::fallback(None, 1024, 768);
        let mut out = Vec::new();
        box_items(
            &mut cat.assets,
            &BoxArt::default(),
            &BoxTexts {
                title: "t".into(),
                body: "b".into(),
                captions: vec!["Yes".into(), "No".into()],
            },
            &yn,
            None,
            (None, None),
            1.0,
            &mut out,
        );
        let want = [(yn[0][0] + 50) as f32, (yn[0][1] + 12) as f32];
        assert!(
            out.iter()
                .any(|i| matches!(i, DrawItem::Label { text, pos, .. }
                if text == "Yes" && *pos == want)),
            "the Yes caption is anchored at (50, 12) of its button"
        );
        // 10 px per character: a 352 px line holds 35 characters
        let lines = wrap_px(
            "aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj",
            352.0,
            |s| s.chars().count() as f32 * 10.0,
        );
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.chars().count() <= 35));
    }

    /// CLI-1130 / CLI-1131: the title draws in the body's white and the first button (Yes / OK) is the focused one,
    /// in the highlighted look, until the mouse moves the focus.
    #[test]
    fn a_box_shows_a_white_title_and_its_first_button_focused() {
        let mut cat = Catalog::fallback(None, 1024, 768);
        cat.paths.popup_buttons = [Some("n".into()), Some("h".into()), Some("p".into())];
        let mut p = Popup::ask("q".into(), BoxTag::None);
        let tex_at = |out: &[DrawItem], at: [i32; 4]| {
            out.iter().find_map(|i| match i {
                DrawItem::Image { tex, dst, .. } if dst[0] == at[0] as f32 && dst[1] == at[1] as f32 => {
                    Some(tex.clone())
                }
                _ => None,
            })
        };
        let b = p.buttons();
        let mut out = Vec::new();
        p.draw(&mut cat, &mut out);
        assert_eq!(tex_at(&out, b[0]).as_deref(), Some("h"));
        assert_eq!(tex_at(&out, b[1]).as_deref(), Some("n"));
        let title_white = out.iter().any(|i| match i {
            DrawItem::Label { style, .. } => style.colour == [255, 255, 255, 255],
            _ => false,
        });
        assert!(title_white, "the title is white");
        let c = [(b[1][0] + 5) as f32, (b[1][1] + 5) as f32];
        p.input(&MenuInput::Move(c));
        p.input(&MenuInput::Move([0.0, 0.0]));
        let mut out = Vec::new();
        p.draw(&mut cat, &mut out);
        assert_eq!(tex_at(&out, b[1]).as_deref(), Some("h"));
        assert_eq!(tex_at(&out, b[0]).as_deref(), Some("n"));
    }

    #[test]
    fn the_invitation_names_room_plus_one_and_runs_out_after_ten_seconds() {
        let cat = Catalog::fallback(None, 1024, 768);
        let t0 = Instant::now();
        let p = Popup::invite(&cat, 4, "Host", "pw", t0);
        assert_eq!(
            (p.kind, p.tag, p.input.as_str()),
            (BoxKind::YesNo, BoxTag::Invite(4), "pw")
        );
        assert!(!p.expired(t0 + std::time::Duration::from_millis(9_999)));
        assert!(p.expired(t0 + std::time::Duration::from_millis(INVITE_MS)));
    }
}

#[cfg(test)]
mod run_tests {
    use super::wrap_runs;

    #[test]
    fn colour_codes_take_no_width_and_split_the_runs() {
        let l = wrap_runs("buy the ^0xFFFFA500^Big Item^ for 7 days now", 20, 1);
        let plain: Vec<String> = l
            .iter()
            .map(|r| r.iter().map(|(t, _)| t.as_str()).collect::<String>())
            .collect();
        assert_eq!(plain, vec!["buy the Big Item for", "7 days now"]);
        assert_eq!(l[0][1].1, 0xffff_a500);
        assert!(l[0][1].0.starts_with("Big Item"));
    }
}
