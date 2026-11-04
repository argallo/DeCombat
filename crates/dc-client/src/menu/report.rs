//! The player report box: a box over the screen with two category lists, a target name with a find button, a
//! description box and send / cancel buttons. Send checks the entries, hands the report to the lobby (0x312) and
//! closes the box; a second report within ten seconds only shows a notice.
//!
//! The 2009 code builds the box from the layout section of the report box (window size, the list, button and box
//! places, each relative to the window) and keeps the box in the middle of the screen: `(screen - size) / 2`.
// SPEC: PROTOCOL_2009.md §3.4 0x312 (kind 0 plain: +0x12 wstr[17] name, +0x34 wstr[601] text; no reply)
// EVIDENCE: cshell+0x274160 (the box build: blind, window, lists, find button, description box, send / cancel
// buttons), cshell+0x273b70 (open: a new report within 0x2710 ms of the last send shows the notice instead; else the
// window is set to the middle of the screen), cshell+0x273d50 (commands 100000 cancel, 0x186a1 send, 0x186a2
// / 0x186a3 / 0x186a4 the lists and the find button), cshell+0x272770 (entry check: no text -> refusal 3, target
// missing where the category needs one -> refusal 2), cshell+0x273630 (the lists' placeholder texts)
// OBSERVED: the layout section of the user's ROM (the keys below) and the box's textures
// EVIDENCE: cshell+0x273080 (the category table: 14 entries of general kind, detail index, the two flags and the
// two names), cshell+0x272480 (entry lookup), cshell+0x272850 (after a detail pick: a category with either flag
// needs the target box, else the box hides and the description takes the keyboard), cshell+0x2723f0 / 0x274050
// (the detail list's rows: the names of the entries of the picked kind; the general list's rows: one per kind),
// cshell+0x2727f0 / 0x272a10 (a pick writes the name on its box; a general pick picks detail 0), cshell+0x273790
// (the packet: +0xa kind, +0xe detail index), cshell+0x274160 (the three lists, hidden at first, under the
// GeneralErrorList / DetailErrorList / UserList places), cshell+0x273d50 (0x186a2 / 3 / 4 show or hide a list)
// DC-DEFAULT: CLI-557 (proposed) the packet is always kind 0 (the 2009 statistics block and the captured free text
// are not built); the user list holds the other users of the channel (2009: the match's players and recent
// speakers); one list is open at a time; the lists show every row without a scroll bar except the user list;
// the description is one line; the resend and refusal texts fall back to our own words when the ROM texts are
// absent; names fall back to "Type n" without the binary

use super::catalog::{int, key, pair, Catalog, KeyPin};
use super::dropdown::{self, Dropdown};
use super::link::LinkCmd;
use super::popup::{BoxTag, Popup};
use super::sheet::{Look, Part, Sheet, ALIGN_CENTRE, ALIGN_LEFT, CARET_PERIOD};
use super::{Ctx, Go, MenuData, MenuKey, Screen};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::WidgetKind;

pub const NAME: &str = "report";

/// A report sent less than this long ago (ms) turns the next opening into a notice.
// EVIDENCE: cshell+0x273b70 (0x2710 ms: the check is `elapsed < 0x2711`)
pub const RESEND_MS: u128 = 0x2710;

/// The layout section and its keys.
// OBSERVED: the user's ROM layout table (matched by SHA-256 prefix and length)
pub const SECTION: KeyPin = key("9d4168409680c845", 13);
pub mod keys {
    use super::{key, KeyPin};
    pub const WINDOW_POS: KeyPin = key("7fd8459190304736", 9);
    pub const WINDOW_WIDTH: KeyPin = key("af29812ecc6aa531", 11);
    pub const WINDOW_HEIGHT: KeyPin = key("6cffa9fd407edebe", 12);
    pub const DROPDOWN_WIDTH: KeyPin = key("21be8e158ae7d771", 13);
    pub const DROPDOWN_HEIGHT: KeyPin = key("579eec9fe1c4464f", 14);
    pub const GENERAL_TEXT_POS: KeyPin = key("9304a87256741b9c", 18);
    pub const GENERAL_BUTTON_POS: KeyPin = key("4403f655cb651c91", 16);
    pub const DETAIL_TEXT_POS: KeyPin = key("b2da1aecfe1a69e9", 17);
    pub const DETAIL_BUTTON_POS: KeyPin = key("14bc85cfb42a1c16", 15);
    pub const FIND_POS: KeyPin = key("d91682b6e3ddf6c9", 14);
    pub const FIND_WIDTH: KeyPin = key("7c83ab8d5a382301", 16);
    pub const FIND_HEIGHT: KeyPin = key("ebc4bc224f1cbcec", 17);
    pub const USER_TEXT_POS: KeyPin = key("038954541f573052", 12);
    pub const USER_POS: KeyPin = key("277db6755295063e", 11);
    pub const USER_WIDTH: KeyPin = key("d73a251a9a6a07b5", 13);
    pub const MESSAGE_WIDTH: KeyPin = key("17c5ded38d36dee8", 15);
    pub const MESSAGE_HEIGHT: KeyPin = key("04407f4975d511c7", 16);
    pub const MESSAGE_POS: KeyPin = key("af77232c19cb5b04", 13);
    pub const BUTTON_WIDTH: KeyPin = key("561b4802408f78aa", 17);
    pub const BUTTON_HEIGHT: KeyPin = key("7cd97b509c5f8e75", 18);
    pub const SEND_POS: KeyPin = key("09ce81752e1b47d1", 13);
    pub const CANCEL_POS: KeyPin = key("4f52114eebe8187f", 15);
    pub const GENERAL_LIST_WIDTH: KeyPin = key("5b4d4490bcbf4f04", 21);
    pub const GENERAL_LIST_POS: KeyPin = key("3ab5c47d790e655e", 19);
    pub const DETAIL_LIST_WIDTH: KeyPin = key("17d455c644640e31", 20);
    pub const DETAIL_LIST_POS: KeyPin = key("be03bf4ef79741ca", 18);
}

/// Layers and commands (the 2009 command numbers of the box).
// EVIDENCE: cshell+0x273d50
pub const BLIND_LAYER: i32 = 0;
pub const WINDOW: i32 = 1;
pub const CMD_CANCEL: i32 = 100_000;
pub const CMD_SEND: i32 = 0x186a1;
pub const CMD_GENERAL: i32 = 0x186a2;
pub const CMD_DETAIL: i32 = 0x186a3;
pub const CMD_FIND: i32 = 0x186a4;
/// Control ids of the two entry boxes.
/// The texts on the general and detail boxes.
pub const LIST_TEXT_IDS: [i32; 2] = [10, 11];
pub const TARGET_ID: i32 = 20;
pub const MESSAGE_ID: i32 = 21;
/// Control id of the find button (the user list).
pub const FIND_ID: i32 = 14;
/// The unit limits of the two entries (wstr[17] and wstr[601] of the packet, each with its terminator).
// SPEC: PROTOCOL_2009.md §3.4 0x312
pub const NAME_UNITS: usize = 16;
pub const TEXT_UNITS: usize = 600;
/// Font of the box (the pages' common font).
// EVIDENCE: cshell+0x274160 (font index 0xe)
const FONT: i32 = 14;
/// The entries' text size (also the caret's height).
// EVIDENCE: cshell+0x274160 (the target's edit control built with size 0xe)
// DC-DEFAULT: CLI-1810 (proposed) the description uses the same size
pub const ENTRY_SIZE: i32 = 0xe;
/// The entries' text colours (the normal-state colour, the second of the four the colour setter gets).
// EVIDENCE: cshell+0x274160 (vtable +0xb0: the target 0xffffffff x 4; the description 0xffaaaaaa x 3, 0xffffffff),
// cshell+0x18930 (the normal state shows the second colour)
pub const TARGET_COLOUR: u32 = 0xffff_ffff;
pub const MESSAGE_COLOUR: u32 = 0xffaa_aaaa;
/// The target's caret colour: the edit control's default (the build sets only the blink period).
// EVIDENCE: cshell+0x274160 (cshell+0x2f79c0 with the float 0x3eaa7efa), cshell+0x2f74c0 (+0x1a8 = 0x89bc6ae0)
pub const TARGET_CARET: u32 = super::textbox::CHAT_CARET_COLOUR;
/// The shade over the name row while the category needs no target: the window's own background colour, mostly
/// opaque, so the row's printed label, its frame and the find button read as off.
// DC-DEFAULT: CLI-1810 (proposed) the 2009 box hides only the name entry (cshell+0x272850); the label and frame are
// printed on the window texture and the find button stays (OBSERVED: the user's ROM window texture, background
// (29, 26, 22))
pub const NAME_ROW_SHADE: u32 = 0xc01d_1a16;
const SCREEN: [i32; 2] = [1024, 768];
const WHITE: u32 = 0xffff_ffff;

/// What the player sees after a send (our words; 0x312 has no reply, so it says the report left the game).
pub const SENT_TEXT: &str = "Your report was sent. Thank you.";

/// Our words where the ROM texts are absent.
fn fallback(i: usize) -> &'static str {
    [
        "Your report was sent a moment ago. Please try again a little later.",
        "Select general type",
        "Select detail type",
        "Enter the player to report first.",
        "Enter a description of the problem first.",
        "Send",
        "Cancel",
    ][i]
}

fn literal(cat: &Catalog, i: usize) -> String {
    cat.paths.overlay.report_texts[i]
        .clone()
        .unwrap_or_else(|| fallback(i).to_string())
}

/// The notice shown instead of the box right after a send.
pub fn resend_notice(cat: &Catalog) -> String {
    literal(cat, 0)
}

/// The category table: (general kind, detail index, statistics flag, text flag) of the 14 entries, in table order.
// EVIDENCE: cshell+0x273080 (the flags at record +0x418 / +0x419: the first on the hack / glitch kinds and the
// second on the harassment, advertisement, impersonation, spam and scam kinds; neither on the bug and comment kinds)
pub const TABLE: [(i32, i32, bool, bool); 14] = [
    (0, 0, false, true),
    (0, 1, false, true),
    (0, 2, false, true),
    (0, 3, true, false),
    (0, 4, true, false),
    (0, 5, false, true),
    (0, 6, false, true),
    (1, 0, false, false),
    (1, 1, false, false),
    (1, 2, false, false),
    (1, 3, false, false),
    (1, 4, false, false),
    (1, 5, false, false),
    (2, 0, false, false),
];
/// The general kinds' number.
pub const KINDS: usize = 3;
/// Names of the three lists in the draw list.
pub const LIST_GENERAL: &str = "report_general";
pub const LIST_DETAIL: &str = "report_detail";
pub const LIST_USER: &str = "report_user";

/// The table entry of a kind and detail index.
// EVIDENCE: cshell+0x272480
pub fn entry_of(kind: i32, detail: i32) -> Option<usize> {
    TABLE.iter().position(|e| e.0 == kind && e.1 == detail)
}

/// Whether the entry asks for a target name.
// EVIDENCE: cshell+0x272770 / 0x272850 (either flag)
pub fn needs_target(kind: i32, detail: i32) -> bool {
    entry_of(kind, detail).is_some_and(|i| TABLE[i].2 || TABLE[i].3)
}

pub struct Report {
    sheet: Sheet,
    /// The name row (label, frame and find button) in virtual pixels: shaded while no target is needed.
    name_row: [i32; 4],
    /// The picked general kind and detail index.
    kind: i32,
    detail: i32,
    drop: Option<Dropdown>,
}

impl Report {
    pub fn new(cat: &Catalog, _data: &MenuData) -> Report {
        let sheet = build(cat);
        let name_row = name_row(cat);
        // the box opens on the category table's first entry (general and detail item 0)
        // EVIDENCE: cshell+0x273b70, cshell+0x2727f0, cshell+0x272a10
        let mut r = Report {
            sheet,
            name_row,
            kind: 0,
            detail: 0,
            drop: None,
        };
        r.pick(cat, 0, 0);
        r
    }

    /// The general kind's name.
    fn kind_name(cat: &Catalog, kind: i32) -> String {
        usize::try_from(kind)
            .ok()
            .and_then(|k| cat.paths.overlay.report_general.get(k))
            .and_then(|t| t.clone())
            .unwrap_or_else(|| {
                // DC-DEFAULT: CLI-1650 (proposed) our words without the binary
                match kind {
                    0 => "Report a player".to_string(),
                    1 => "Report a bug".to_string(),
                    2 => "Comments".to_string(),
                    _ => format!("Type {}", kind + 1),
                }
            })
    }

    /// The detail name of table entry `e`.
    fn detail_name(cat: &Catalog, e: usize) -> String {
        cat.paths.overlay.report_detail[e]
            .clone()
            .unwrap_or_else(|| format!("Type {}", e + 1))
    }

    /// The rows of the detail list: the entries of the picked kind, valued with their detail index.
    // EVIDENCE: cshell+0x2723f0
    fn detail_rows(&self, cat: &Catalog) -> Vec<(String, i32)> {
        (0..TABLE.len())
            .filter(|&e| TABLE[e].0 == self.kind)
            .map(|e| (Self::detail_name(cat, e), TABLE[e].1))
            .collect()
    }

    /// Takes a kind and detail index: both names on their boxes, the target box shown for the kinds that need it.
    // EVIDENCE: cshell+0x2727f0 (kind: the name on the general box, then detail 0), cshell+0x272a10 (detail: the
    // name on the detail box, then the state of cshell+0x272850)
    fn pick(&mut self, cat: &Catalog, kind: i32, detail: i32) {
        self.kind = kind;
        self.detail = detail;
        self.sheet
            .set_text(WINDOW, LIST_TEXT_IDS[0], &Self::kind_name(cat, kind));
        if let Some(e) = entry_of(kind, detail) {
            self.sheet
                .set_text(WINDOW, LIST_TEXT_IDS[1], &Self::detail_name(cat, e));
        }
        let target = needs_target(kind, detail);
        // EVIDENCE: cshell+0x272850 (the target's edit control shown and enabled, then hidden for a category with
        // neither flag)
        if let Some(i) = self.sheet.find(WINDOW, TARGET_ID, WidgetKind::EditBox) {
            self.sheet.parts[i].shown = target;
        }
        // DC-DEFAULT: CLI-1810 (proposed) the find button is off with the entry (2009 leaves it on)
        if let Some(i) = self.sheet.find(WINDOW, FIND_ID, WidgetKind::Button) {
            self.sheet.parts[i].enabled = target;
            self.sheet.parts[i].hover = false;
        }
        if !target && self.drop.as_ref().is_some_and(|d| d.list == LIST_USER) {
            self.drop = None;
        }
        let want = if target { TARGET_ID } else { MESSAGE_ID };
        let at = self.sheet.find(WINDOW, want, WidgetKind::EditBox);
        self.sheet.focus(at);
    }

    /// The anchor of a list opened at the window-relative place `pos` with the width `w`: the list's frame starts
    /// at `pos` itself.
    // EVIDENCE: cshell+0x274160 (each list added at its layout place, width from the layout)
    fn anchor(&self, cat: &Catalog, pos: &KeyPin, w: &KeyPin) -> [i32; 4] {
        let p = pair(cat.section_value(&SECTION, pos)).unwrap_or([0, 0]);
        let width = int(cat.section_value(&SECTION, w)).unwrap_or(150);
        let at = Self::window_at(cat);
        [at[0] + p[0], at[1] + p[1] - dropdown::GAP, width, 0]
    }

    fn window_at(cat: &Catalog) -> [i32; 2] {
        let v = |k: &KeyPin| cat.section_value(&SECTION, k);
        let w = int(v(&keys::WINDOW_WIDTH)).unwrap_or(566);
        let h = int(v(&keys::WINDOW_HEIGHT)).unwrap_or(437);
        [(SCREEN[0] - w) / 2, (SCREEN[1] - h) / 2]
    }

    /// The names the user list offers.
    // DC-DEFAULT: CLI-557 (proposed) the other users of the channel
    fn user_names(&self, ctx: &Ctx) -> Vec<String> {
        let me = &ctx.data.profile.nick;
        ctx.data
            .users
            .iter()
            .filter(|u| &u.name != me && !u.name.is_empty())
            .map(|u| u.name.clone())
            .collect()
    }

    /// Whether the picked category asks for a target name.
    pub fn wants_target(&self) -> bool {
        needs_target(self.kind, self.detail)
    }

    /// The picked general kind and detail index.
    pub fn category(&self) -> (i32, i32) {
        (self.kind, self.detail)
    }

    fn entry(&self, id: i32) -> String {
        self.sheet
            .part(WINDOW, id, WidgetKind::EditBox)
            .map(|p| p.text.trim().to_string())
            .unwrap_or_default()
    }

    /// The refusal text of the entries as they are, or `None` when the report may go.
    // EVIDENCE: cshell+0x272770 (the target check is the later one and wins), cshell+0x273d50 (the two refusals)
    pub fn refusal(&self, cat: &Catalog) -> Option<String> {
        if needs_target(self.kind, self.detail) && self.entry(TARGET_ID).is_empty() {
            Some(literal(cat, 3))
        } else if self.entry(MESSAGE_ID).is_empty() {
            Some(literal(cat, 4))
        } else {
            None
        }
    }
}

/// A button prefix the code builds (the literal is the prefix of the four state textures).
// EVIDENCE: cshell+0x274160 (the literals are handed to the button build as prefixes)
fn prefix(t: &Option<String>) -> String {
    t.clone().unwrap_or_default()
}

pub fn build(cat: &Catalog) -> Sheet {
    let v = |k: &KeyPin| cat.section_value(&SECTION, k);
    let ov = &cat.paths.overlay;
    let tex = |t: &Option<String>| t.clone().unwrap_or_default();
    let n = |k: &KeyPin, d: i32| int(v(k)).unwrap_or(d);
    let size = [n(&keys::WINDOW_WIDTH, 566), n(&keys::WINDOW_HEIGHT, 437)];
    let at = [(SCREEN[0] - size[0]) / 2, (SCREEN[1] - size[1]) / 2];
    // a child's place in the layout section is relative to the window's top-left
    // EVIDENCE: cshell+0x274160 (children added with the window's add-child call, which takes window-relative places)
    // OBSERVED: the user's ROM window texture (the form's fields sit at the section's places from the window corner)
    let place = |k: &KeyPin, def: [i32; 2]| {
        let p = pair(v(k)).unwrap_or(def);
        [at[0] + p[0], at[1] + p[1]]
    };
    let mut sheet = Sheet::default();
    sheet.parts.push(Part::synthetic(
        WidgetKind::Window,
        BLIND_LAYER,
        BLIND_LAYER,
        0,
        [0, 0],
        SCREEN,
        Look::Panel {
            tex: tex(&ov.blind),
            stretch: true,
        },
    ));
    sheet.parts.push(Part::synthetic(
        WidgetKind::Window,
        WINDOW,
        WINDOW,
        0,
        at,
        size,
        // the window is the top-left part of its texture (the texture is larger than the window)
        Look::Panel {
            tex: tex(&ov.report_window),
            stretch: false,
        },
    ));
    let on = |sheet: &mut Sheet, mut p: Part| {
        p.parent = Some(1);
        sheet.parts.push(p);
    };
    let label = |id: i32, text: String, at: [i32; 2], align: Align| {
        let mut p = Part::synthetic(
            WidgetKind::Text,
            WINDOW,
            id,
            0,
            at,
            [0, 0],
            Look::Text {
                align,
                font: FONT,
                size: 0,
                colour: WHITE,
            },
        );
        p.text = text;
        p
    };
    let left = Align {
        first: ALIGN_LEFT,
        second: 0,
    };
    let centre = Align {
        first: ALIGN_CENTRE,
        second: ALIGN_SECOND_CENTRE,
    };
    let drop = [n(&keys::DROPDOWN_WIDTH, 18), n(&keys::DROPDOWN_HEIGHT, 16)];
    let lists = [
        (
            &keys::GENERAL_TEXT_POS,
            &keys::GENERAL_BUTTON_POS,
            CMD_GENERAL,
        ),
        (&keys::DETAIL_TEXT_POS, &keys::DETAIL_BUTTON_POS, CMD_DETAIL),
    ];
    for (i, (text_at, button_at, cmd)) in lists.into_iter().enumerate() {
        on(
            &mut sheet,
            label(
                LIST_TEXT_IDS[i],
                literal(cat, 1 + i),
                place(text_at, [198, 144 + 40 * i as i32]),
                left,
            ),
        );
        on(
            &mut sheet,
            Part::synthetic(
                WidgetKind::Button,
                WINDOW,
                12 + i as i32,
                cmd,
                place(button_at, [522, 143 + 40 * i as i32]),
                drop,
                Look::Button {
                    prefix: prefix(&ov.report_dropdown),
                    caption: None,
                    frame: None,
                },
            ),
        );
    }
    on(
        &mut sheet,
        Part::synthetic(
            WidgetKind::Button,
            WINDOW,
            FIND_ID,
            CMD_FIND,
            place(&keys::FIND_POS, [418, 221]),
            [n(&keys::FIND_WIDTH, 126), n(&keys::FIND_HEIGHT, 29)],
            Look::Button {
                prefix: prefix(&ov.report_find),
                caption: None,
                frame: None,
            },
        ),
    );
    let edit =
        |id: i32, at: [i32; 2], size: [i32; 2], max_len: usize, (text, caret): (u32, u32)| {
            let mut p = Part::synthetic(
                WidgetKind::EditBox,
                WINDOW,
                id,
                0,
                at,
                size,
                Look::Edit {
                    font: FONT,
                    size: ENTRY_SIZE,
                    max_len,
                    caret_colour: caret,
                    caret_period: CARET_PERIOD,
                    clip: size[0],
                    text_colour: text,
                },
            );
            p.hit = Some([at[0], at[1], size[0], size[1]]);
            p
        };
    // the entries' text starts at the list text place (the target) and the box place (the description); the click
    // area is the form's frame around it
    // OBSERVED: the user's ROM window texture (target frame 6 px above and 5 px left of the text place, the
    // description frame 17 px wider than the box for its scroll bar)
    let tp = place(&keys::USER_TEXT_POS, [198, 224]);
    let mut target = edit(
        TARGET_ID,
        tp,
        [n(&keys::USER_WIDTH, 224) - 10, drop[1]],
        NAME_UNITS,
        (TARGET_COLOUR, TARGET_CARET),
    );
    target.hit = Some([tp[0] - 5, tp[1] - 6, n(&keys::USER_WIDTH, 224), 26]);
    on(&mut sheet, target);
    let mp = place(&keys::MESSAGE_POS, [200, 267]);
    let msize = [n(&keys::MESSAGE_WIDTH, 332), n(&keys::MESSAGE_HEIGHT, 106)];
    // DC-DEFAULT: CLI-1810 (proposed) the description's caret is white (its control's default is not read)
    let mut message = edit(MESSAGE_ID, mp, msize, TEXT_UNITS, (MESSAGE_COLOUR, WHITE));
    message.hit = Some([mp[0] - 8, mp[1] - 5, msize[0] + 17, msize[1] + 13]);
    on(&mut sheet, message);
    let button = [n(&keys::BUTTON_WIDTH, 104), n(&keys::BUTTON_HEIGHT, 26)];
    let solid = prefix(&ov.solid_button);
    let buttons = [
        (&keys::SEND_POS, CMD_SEND, [153, 399]),
        (&keys::CANCEL_POS, CMD_CANCEL, [309, 399]),
    ];
    for (i, (k, cmd, def)) in buttons.into_iter().enumerate() {
        let p = place(k, def);
        on(
            &mut sheet,
            Part::synthetic(
                WidgetKind::Button,
                WINDOW,
                30 + i as i32,
                cmd,
                p,
                button,
                Look::Button {
                    prefix: solid.clone(),
                    caption: None,
                    frame: None,
                },
            ),
        );
        // the caption: the box's own wide text, centred on the button
        on(
            &mut sheet,
            label(
                40 + i as i32,
                literal(cat, 5 + i),
                [p[0] + button[0] / 2, p[1] + button[1] / 2],
                centre,
            ),
        );
    }
    // the first entry takes the keyboard
    let first = sheet
        .parts
        .iter()
        .position(|p| p.kind == WidgetKind::EditBox);
    sheet.focus(first);
    sheet
}

/// The name row: the window's width less 8 px a side, from 8 px over the target's text place, 30 px high.
// DC-DEFAULT: CLI-1810 (proposed) (OBSERVED: the user's ROM window texture, the name label, frame and find button
// sit in that band)
pub fn name_row(cat: &Catalog) -> [i32; 4] {
    let v = |k: &KeyPin| cat.section_value(&SECTION, k);
    let w = int(v(&keys::WINDOW_WIDTH)).unwrap_or(566);
    let h = int(v(&keys::WINDOW_HEIGHT)).unwrap_or(437);
    let at = [(SCREEN[0] - w) / 2, (SCREEN[1] - h) / 2];
    let tp = pair(v(&keys::USER_TEXT_POS)).unwrap_or([198, 224]);
    [at[0] + 8, at[1] + tp[1] - 8, w - 16, 30]
}

impl Screen for Report {
    fn name(&self) -> &'static str {
        NAME
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

    fn draw_extra(
        &self,
        _cat: &mut Catalog,
        _data: &MenuData,
        _secs: f32,
        out: &mut Vec<crate::hud::DrawItem>,
    ) {
        if !self.wants_target() {
            let r = self.name_row;
            out.push(crate::hud::DrawItem::Fill {
                rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
                colour: crate::hud::board::argb(NAME_ROW_SHADE),
            });
        }
    }

    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        match k {
            MenuKey::Escape => self.click(CMD_CANCEL, usize::MAX, ctx),
            MenuKey::Tab | MenuKey::BackTab => {
                // between the two entry boxes
                let boxes: Vec<usize> = (0..self.sheet.parts.len())
                    .filter(|&i| {
                        self.sheet.parts[i].kind == WidgetKind::EditBox && self.sheet.parts[i].shown
                    })
                    .collect();
                let cur = self
                    .sheet
                    .focused()
                    .and_then(|f| boxes.iter().position(|&b| b == f));
                let next = cur.map_or(0, |c| (c + 1) % boxes.len().max(1));
                self.sheet.focus(boxes.get(next).copied());
            }
            MenuKey::Backspace => {
                self.sheet.backspace();
            }
            // the menu window delivers Space as a key (CLI-483): a space in the focused entry
            MenuKey::Space => {
                self.sheet.type_char(' ');
            }
            _ => {}
        }
    }

    fn dropdown(&self) -> Option<&Dropdown> {
        self.drop.as_ref()
    }

    fn dropdown_mut(&mut self) -> Option<&mut Dropdown> {
        self.drop.as_mut()
    }

    fn close_dropdown(&mut self) {
        self.drop = None;
    }

    // EVIDENCE: cshell+0x273d50 (0x186a5 / 0x186a6 / 0x186a7: the kind, detail and user picks)
    fn dropdown_pick(&mut self, list: &'static str, value: i32, _index: usize, ctx: &mut Ctx) {
        match list {
            LIST_GENERAL => self.pick(ctx.cat, value, 0),
            LIST_DETAIL => {
                let kind = self.kind;
                self.pick(ctx.cat, kind, value);
            }
            _ => {
                let name = self.user_names(ctx).into_iter().nth(value as usize);
                if let (Some(name), Some(p)) = (
                    name,
                    self.sheet.part_mut(WINDOW, TARGET_ID, WidgetKind::EditBox),
                ) {
                    p.text = name;
                }
            }
        }
    }

    fn click(&mut self, cmd: i32, _part: usize, ctx: &mut Ctx) {
        match cmd {
            CMD_CANCEL => {
                *ctx.go = Some(Go::Back);
            }
            CMD_SEND => {
                if let Some(text) = self.refusal(ctx.cat) {
                    ctx.popups.push(Popup::error(text, BoxTag::None));
                    return;
                }
                // EVIDENCE: cshell+0x273790 (+0xa the kind, +0xe the detail index)
                // DC-DEFAULT: CLI-1810 (proposed) a category without a target sends no name (2009 sends the
                // hidden entry's text, cshell+0x273d50)
                let name = if self.wants_target() {
                    self.entry(TARGET_ID)
                } else {
                    String::new()
                };
                let text = self.entry(MESSAGE_ID);
                // CLI-1841: a bug report (the second general kind) is offered its diagnostics
                if self.kind == 1 {
                    crate::diag::note_bug_report(&text);
                }
                ctx.send(LinkCmd::Report {
                    name,
                    text,
                    kind: self.kind as u32,
                    detail: self.detail as u32,
                });
                ctx.data.last_report = Some(std::time::Instant::now());
                // DC-DEFAULT: CLI-1650 (proposed) the 2009 box closes in silence; ours says the report went
                ctx.popups
                    .push(Popup::ok(SENT_TEXT.to_string(), BoxTag::None));
                *ctx.go = Some(Go::Back);
            }
            // CLI-1810: no user list while the category needs no target
            CMD_FIND if !self.wants_target() => {}
            CMD_GENERAL | CMD_DETAIL | CMD_FIND => {
                // a list's command shows it, or hides it when shown
                // EVIDENCE: cshell+0x273d50 (0x186a2 / 3 / 4: the list's shown state, then show or hide)
                let was = self.drop.take().map(|d| d.list);
                let (list, rows, at, scroll) = match cmd {
                    CMD_GENERAL => (
                        LIST_GENERAL,
                        (0..KINDS as i32)
                            .map(|k| (Self::kind_name(ctx.cat, k), k))
                            .collect::<Vec<_>>(),
                        self.anchor(ctx.cat, &keys::GENERAL_LIST_POS, &keys::GENERAL_LIST_WIDTH),
                        false,
                    ),
                    CMD_DETAIL => (
                        LIST_DETAIL,
                        self.detail_rows(ctx.cat),
                        self.anchor(ctx.cat, &keys::DETAIL_LIST_POS, &keys::DETAIL_LIST_WIDTH),
                        false,
                    ),
                    _ => (
                        LIST_USER,
                        self.user_names(ctx)
                            .into_iter()
                            .enumerate()
                            .map(|(i, n)| (n, i as i32))
                            .collect::<Vec<_>>(),
                        self.anchor(ctx.cat, &keys::USER_POS, &keys::USER_WIDTH),
                        true,
                    ),
                };
                if was != Some(list) && !rows.is_empty() {
                    let mut d = Dropdown::new(list, at, rows, dropdown::ROOM_ROW_SIZE, scroll);
                    // OBSERVED: the reference stack's report lists draw their rows from the left
                    d.left = true;
                    self.drop = Some(d);
                }
            }
            _ => {}
        }
    }
}
