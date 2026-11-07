//! nickname-screen: the 2009 nickname screen (screen 0xe), opened when the login found no character. The player
//! picks a character model from the row of portraits (the chosen one stands as a 3D figure on the left), types a
//! name, checks it with the server (Check) and creates the character (Create); Cancel asks whether to close the
//! game. The answers show as the page's message images.
//!
//! - Check: an empty box does nothing; the login ID itself, a length outside 4..12 or a check in flight is refused
//!   on the client, anything else goes to the server as 0x204; its answer shows "free" (Create enabled, the checked
//!   name kept) or "taken".
//! - Create: the box text must still be the checked name (else the checked name is dropped and the "check first"
//!   message shows); then 0x202 goes out with the face value 6000 + the model's face row, Check and Create are
//!   disabled until the answer. A refusal re-enables Check (codes 0xe / 0xf: the "name taken" box); success is
//!   followed by the server list, which opens the server screen as after a login.
//! - The referrer field, its OK button and its message stay hidden and disabled, as in the 2009 client.
// SPEC: PROTOCOL_2009.md §2.1 (reason 0x0003001e -> nickname screen), §2.2 (0x204 / 0x205, 0x202 / 0x203, the
// client-side name filter, the face value rule)
// EVIDENCE: cshell+0x12da20 (create: 0x202 only for the checked name), cshell+0x12cb70 (the check answer keeps the
// name and enables create, or drops it)
// EVIDENCE: cshell+0x12e550 (command handler: 0xc5 check -> 0x204, 0x133 create, 0x12f cancel -> quit question 0xbd3 whose
// yes is command 0xb, 0x135 model picked, 0x2712 referrer confirm)
// DISASM: cshell+0x12c070 (build: 3 x 4 background tiles, the page, the name box's click area 0x6b x 0xf, control
// 10 = the create button), cshell+0x12f3e0 (focus: state cleared, create disabled, first model picked, controls 6, 7,
// 8 and 12 hidden)

use std::path::Path;
use std::sync::Arc;

use dc_bute::{ButeDoc, SectionMap};
use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::WidgetKind;

use super::catalog::{pair, Catalog};
use super::figure::{FigureTables, LookIds, SLOT_WEAPON};
use super::link::{LinkCmd, LinkEvent};
use super::popup::{text_ids, BoxTag, Popup};
use super::sheet::{image, Look, Sheet, Tiles};
use super::{build_sheet, modelview, pages, Ctx, Go, MenuData, MenuKey, Screen};
use crate::hud::items::DrawItem;

/// Window 0 holds the name box (its control 0), window 1 the referrer box.
pub const NAME_WINDOW: i32 = 0;
pub const NAME_BOX: i32 = 0;
pub const REFERRER_WINDOW: i32 = 1;
/// Images on the screen: the chosen model's large portrait, the message, the model's age and description, the
/// referrer message.
pub const PICTURE: i32 = 2;
pub const MESSAGE: i32 = 5;
pub const AGE: i32 = 6;
pub const DESCRIPTION: i32 = 7;
pub const REFERRER_MESSAGE: i32 = 8;
/// Buttons: check, create, cancel, the referrer's OK.
pub const CHECK_BUTTON: i32 = 9;
pub const CREATE_BUTTON: i32 = 10;
pub const CANCEL_BUTTON: i32 = 11;
pub const REFERRER_OK: i32 = 12;
/// Command numbers of the buttons (the page's records) and of a model pick.
// EVIDENCE: cshell+0x12e550 (0xc5, 0x133, 0x12f, 0x2712; 0x135 = a portrait picked, sent by cshell+0x12d8b0)
pub const CMD_CHECK: i32 = 0xc5;
pub const CMD_CREATE: i32 = 0x133;
pub const CMD_CANCEL: i32 = 0x12f;
pub const CMD_REFERRER: i32 = 0x2712;
pub const CMD_PICK: i32 = 0x135;
/// Name length the client accepts (UTF-16 units).
// EVIDENCE: cshell+0x12e550 (text length below 4 or above 0xc -> the length message)
pub const NAME_MIN: usize = 4;
pub const NAME_MAX: usize = 12;
/// The name box's click area (from its position).
// DISASM: cshell+0x12c10a..0x12c128 (rect = pos .. pos + (0x6b, 0xf))
pub const NAME_HIT: [i32; 2] = [0x6b, 0xf];
/// The portrait row: first portrait's place, the step between portraits, portraits per page.
// EVIDENCE: cshell+0x12e050 (pages of 6 through cshell+0x12dc60 with x 0x3b, y 0x75, x step 0x4c)
pub const PORTRAIT_AT: [i32; 2] = [0x3b, 0x75];
pub const PORTRAIT_STEP: i32 = 0x4c;
pub const PORTRAITS_PER_PAGE: usize = 6;
/// 0x203 refusal codes that show the "name taken" box (string 0xfa3).
// EVIDENCE: cshell+0x12e9d0 (code 0xe or 0xf -> box 0xfa3; other codes no box)
pub const TAKEN_CODES: [u32; 2] = [0xe, 0xf];
pub const TEXT_TAKEN: i32 = 0xfa3;
/// The face value of a created character: this base plus the model's face row.
pub const FACE_BASE: i32 = dc_botclient::lobby::FACE_BASE;
/// Where the chosen model stands and the pane it is drawn in (virtual 1024 x 768 pixels).
// EVIDENCE: cshell+0x12cc60 (the screen's character: the inventory figure set-up, position (-40, -15, 205))
// DC-DEFAULT: CLI-1482 (proposed) the frame of that position is not traced: origin and pane fit to the real 2009
// screen OBSERVED ingame9/ref/nick_r_face3.png (figure centre x ~335, head y ~240, feet y ~685)
pub const PLACE: modelview::Placement = modelview::Placement {
    origin: [-38.0, -65.0, 203.0],
    yaw: std::f32::consts::PI,
};
pub const PANE_RECT: [i32; 4] = [150, 195, 390, 560];

/// The page's code literals (CShell.dll): background tiles, the message images, the portrait highlight.
// DISASM: cshell+0x12c089 (background prefix), cshell+0x12e550 (length / bad word / account ID messages),
// cshell+0x12da20 (check first), cshell+0x12cb70 (free / taken), cshell+0x12e240 (portrait highlight)
pub mod pins {
    use super::Pin;
    const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
        Pin {
            what,
            rva,
            sha16,
            len,
        }
    }
    pub const BACK: Pin = pin(
        "nickname background tiles",
        0x3e2390,
        "ae034cafea89410d",
        33,
    );
    pub const MSG_LENGTH: Pin = pin("nickname length message", 0x3e2578, "04b3dedf9d5a0903", 40);
    pub const MSG_BAD_WORD: Pin = pin(
        "nickname word filter message",
        0x3e25e0,
        "c77d59219b4e15ea",
        35,
    );
    pub const MSG_ACCOUNT: Pin = pin(
        "nickname account ID message",
        0x3e2604,
        "aba224115ac0748c",
        43,
    );
    pub const MSG_CHECK_FIRST: Pin = pin(
        "nickname check first message",
        0x3e252c,
        "fc9f843036996904",
        35,
    );
    pub const MSG_FREE: Pin = pin("nickname free message", 0x3e23f8, "e720c13ba6934438", 35);
    pub const MSG_TAKEN: Pin = pin("nickname taken message", 0x3e23d4, "d2c70948b3e6ad1b", 35);
    pub const HIGHLIGHT: Pin = pin(
        "nickname portrait highlight",
        0x3e2550,
        "718f809ad3580199",
        38,
    );
}

/// The message the screen shows (image control 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    /// The page's own image (the length rule).
    Rule,
    Length,
    /// The 2009 word filter's message (never shown: the filter is not ported, CLI-1481).
    BadWord,
    LoginId,
    CheckFirst,
    Free,
    Taken,
}

/// The pinned literals as read from the user's CShell.dll (`None`: absent or not matching).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NickArt {
    pub back: Option<String>,
    pub length: Option<String>,
    pub bad_word: Option<String>,
    pub account: Option<String>,
    pub check_first: Option<String>,
    pub free: Option<String>,
    pub taken: Option<String>,
    pub highlight: Option<String>,
}

impl NickArt {
    pub fn from_rom(rom: Option<&Path>) -> NickArt {
        let Some(pe) = rom.and_then(|r| {
            dc_rom::PeFile::from_file(&r.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1])).ok()
        }) else {
            return NickArt::default();
        };
        let r = |p: &Pin| read_pinned(&pe, p).ok();
        NickArt {
            back: r(&pins::BACK),
            length: r(&pins::MSG_LENGTH),
            bad_word: r(&pins::MSG_BAD_WORD),
            account: r(&pins::MSG_ACCOUNT),
            check_first: r(&pins::MSG_CHECK_FIRST),
            free: r(&pins::MSG_FREE),
            taken: r(&pins::MSG_TAKEN),
            highlight: r(&pins::HIGHLIGHT),
        }
    }

    /// The texture of a message (`None` for the page's own image or a literal that did not resolve).
    pub fn message(&self, m: Message) -> Option<&String> {
        match m {
            Message::Rule => None,
            Message::Length => self.length.as_ref(),
            Message::BadWord => self.bad_word.as_ref(),
            Message::LoginId => self.account.as_ref(),
            Message::CheckFirst => self.check_first.as_ref(),
            Message::Free => self.free.as_ref(),
            Message::Taken => self.taken.as_ref(),
        }
    }
}

/// One model of the creation table the screen offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    /// The face row: the created character's face value is [`FACE_BASE`] + this.
    pub face: i32,
    /// The weapon the figure holds.
    pub weapon: i32,
    /// Portrait (small, with its size) and large picture.
    pub small: String,
    pub small_size: [i32; 2],
    pub large: String,
}

const REZ: &str = "ATTRIBUTES.rez";
const CREATE_TABLE: &str = "ATTRIBUTES\\CREATEMODEL.TXT";
const SECTION_PREFIX: &str = "Create_Model";

fn unquote(v: &str) -> String {
    let v = v.split("//").next().unwrap_or("").trim();
    v.trim_matches('"').trim().to_string()
}

fn int(sec: &SectionMap, key: &str) -> Option<i32> {
    sec.get(key).and_then(|v| unquote(v).parse().ok())
}

impl ModelChoice {
    /// The look the figure wears for this model: the default look with the row's face, holding the row's weapon.
    pub fn look_ids(&self) -> LookIds {
        let mut eq = [-1i32; 15];
        eq[SLOT_WEAPON] = self.weapon;
        LookIds::with_account_face(&eq, self.face)
    }
}

/// The models of the creation table in row order, rows with a view type other than 0 left out (the 2009 list
/// skips them).
// OBSERVED: ROM table CREATEMODEL.TXT [Create_Model<n>] FacePieceId, WeaponID, ViewType, SmallFaceImg,
// SmallFaceImgRec, LargeFaceImg
// EVIDENCE: cshell+0x12e240 (entries with +0xdc != 0 skipped; small picture +0x3c sized +0xc8 / +0xcc, large
// picture +0x58); cshell+0x12da20 (face value = 6000 + the entry's face field)
// DC-DEFAULT: CLI-1482 (proposed) the entry's face field is the row's FacePieceId (the record's key order is not
// traced; FacePieceId equals the row number for every row the EU table shows)
pub fn models_from_doc(doc: &ButeDoc) -> Vec<ModelChoice> {
    let mut rows: Vec<(i32, ModelChoice)> = doc
        .iter()
        .filter_map(|(name, sec)| {
            let n: i32 = name.strip_prefix(SECTION_PREFIX)?.parse().ok()?;
            (int(sec, "ViewType").unwrap_or(0) == 0).then_some(())?;
            Some((
                n,
                ModelChoice {
                    face: int(sec, "FacePieceId").unwrap_or(0),
                    weapon: int(sec, "WeaponID").unwrap_or(-1),
                    small: sec
                        .get("SmallFaceImg")
                        .map(|v| unquote(v))
                        .unwrap_or_default(),
                    small_size: pair(sec.get("SmallFaceImgRec").map(|v| v.as_str()))
                        .unwrap_or([72, 72]),
                    large: sec
                        .get("LargeFaceImg")
                        .map(|v| unquote(v))
                        .unwrap_or_default(),
                },
            ))
        })
        .collect();
    rows.sort_by_key(|(n, _)| *n);
    rows.into_iter().map(|(_, m)| m).collect()
}

pub fn models_from_rom(rom: &Path) -> Vec<ModelChoice> {
    create_table(rom).map_or_else(Vec::new, |doc| models_from_doc(&doc))
}

/// use-items-emblem: the creation table of the ROM (the face window reads it too).
pub fn create_table(rom: &Path) -> Option<ButeDoc> {
    let vfs = dc_rez::RezVfs::new(rom);
    ButeDoc::try_from_vfs(&vfs, REZ, CREATE_TABLE).ok().flatten()
}

/// use-items-emblem: the row number of a creation table section name (`None` for another section).
pub fn create_row(section: &str) -> Option<i32> {
    section.strip_prefix(SECTION_PREFIX)?.parse().ok()
}

/// What a check press does with the box text (`account`: the login ID).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStep {
    /// Nothing (empty box or a check / creation in flight).
    None,
    /// Refused on the client with this message.
    Refused(Message),
    /// Ask the server.
    Ask(String),
}

/// The client-side tests of a check press, in the 2009 order: busy, empty, the login ID, the length; the word filter
/// is not ported (CLI-1481: every name passes it and the server decides).
// EVIDENCE: cshell+0x12e550 (flag +0x5e54 set -> nothing; empty text -> nothing; equal to the login screen's ID
// ignoring case -> account ID message; length outside 4..12 -> length message; word filter cshell+0x2fea70 refusing
// -> bad word message; else 0x204)
pub fn check_step(text: &str, account: &str, busy: bool) -> CheckStep {
    if busy || text.is_empty() {
        return CheckStep::None;
    }
    if !account.is_empty() && text.eq_ignore_ascii_case(account) {
        return CheckStep::Refused(Message::LoginId);
    }
    let n = text.encode_utf16().count();
    if !(NAME_MIN..=NAME_MAX).contains(&n) {
        return CheckStep::Refused(Message::Length);
    }
    CheckStep::Ask(text.to_string())
}

pub struct Nickname {
    sheet: Sheet,
    art: NickArt,
    pub models: Vec<ModelChoice>,
    /// The chosen model and the portrait under the pointer (indices into `models`).
    pub selected: usize,
    hover: Option<usize>,
    /// The name the server called free (Create acts on it only).
    pub checked: Option<String>,
    /// A check or a creation waits for its answer.
    pub busy: bool,
    pub message: Message,
    figure: Option<Arc<FigureTables>>,
}

impl Nickname {
    pub fn new(cat: &Catalog, _data: &MenuData) -> Nickname {
        let mut sheet = build_sheet(cat, &[pages::NAME_ENTRY]);
        let art = NickArt::from_rom(cat.rom.as_deref());
        if let Some(prefix) = &art.back {
            sheet.tiles = Tiles::new([0, 0], 3, 4, prefix);
        }
        if let Some(p) = sheet.part_mut(NAME_WINDOW, NAME_BOX, WidgetKind::EditBox) {
            p.hit = Some([p.pos[0], p.pos[1], NAME_HIT[0], NAME_HIT[1]]);
        }
        Nickname {
            sheet,
            art,
            models: cat.rom.as_deref().map(models_from_rom).unwrap_or_default(),
            selected: 0,
            hover: None,
            checked: None,
            busy: false,
            message: Message::Rule,
            figure: cat.rom.as_deref().map(FigureTables::cached),
        }
    }

    fn name_text(&self) -> String {
        self.sheet
            .part(NAME_WINDOW, NAME_BOX, WidgetKind::EditBox)
            .map(|p| p.text.clone())
            .unwrap_or_default()
    }

    fn enable(&mut self, id: i32, on: bool) {
        if let Some(p) = self.sheet.part_mut(-1, id, WidgetKind::Button) {
            p.enabled = on;
        }
    }

    /// The button's enabled flag (tests).
    pub fn enabled(&self, id: i32) -> bool {
        self.sheet
            .part(-1, id, WidgetKind::Button)
            .is_some_and(|p| p.enabled)
    }

    fn show_message(&mut self, m: Message) {
        self.message = m;
        let Some(tex) = self.art.message(m).cloned() else {
            return;
        };
        if let Some(p) = self.sheet.part_mut(-1, MESSAGE, WidgetKind::Image) {
            if let Look::Panel { tex: t, .. } = &mut p.look {
                *t = tex;
            }
        }
    }

    /// Model `i` becomes the chosen one: its large picture.
    // EVIDENCE: cshell+0x12d8b0 (a portrait clicked: the highlight moves, command 0x135), cshell+0x12dd10 (0x135:
    // control 2 takes the entry's large picture; the face goes to the own look, the figure is rebuilt)
    fn pick(&mut self, i: usize) {
        let Some(m) = self.models.get(i) else {
            return;
        };
        self.selected = i;
        let large = m.large.clone();
        if let Some(p) = self.sheet.part_mut(-1, PICTURE, WidgetKind::Image) {
            if let Look::Panel { tex, .. } = &mut p.look {
                *tex = large;
            }
        }
    }

    /// Portrait `i`'s rectangle (virtual pixels).
    pub fn portrait_rect(&self, i: usize) -> Option<[i32; 4]> {
        let m = self.models.get(i)?;
        let k = (i % PORTRAITS_PER_PAGE) as i32;
        Some([
            PORTRAIT_AT[0] + k * PORTRAIT_STEP,
            PORTRAIT_AT[1],
            m.small_size[0],
            m.small_size[1],
        ])
    }

    fn portrait_at(&self, p: [f32; 2]) -> Option<usize> {
        (0..self.models.len().min(PORTRAITS_PER_PAGE)).find(|i| {
            self.portrait_rect(*i).is_some_and(|r| {
                p[0] >= r[0] as f32
                    && p[1] >= r[1] as f32
                    && p[0] < (r[0] + r[2]) as f32
                    && p[1] < (r[1] + r[3]) as f32
            })
        })
    }

    /// The face value the creation sends for the chosen model.
    pub fn face_value(&self) -> i32 {
        FACE_BASE + self.models.get(self.selected).map_or(0, |m| m.face)
    }

    fn check(&mut self, ctx: &mut Ctx) {
        let text = self.name_text();
        match check_step(&text, &ctx.data.user, self.busy) {
            CheckStep::None => {}
            CheckStep::Refused(m) => {
                self.checked = None;
                self.enable(CREATE_BUTTON, false);
                self.show_message(m);
            }
            CheckStep::Ask(name) => {
                self.checked = None;
                self.enable(CREATE_BUTTON, false);
                self.busy = true;
                ctx.send(LinkCmd::NickCheck(name));
            }
        }
    }

    // EVIDENCE: cshell+0x12da20 (the box text differs from the checked name -> checked dropped, create disabled;
    // not checked -> the "check first" message; else 0x202 with 6000 + the face field, flag +0x5e54 set, control 9
    // (check) and the create button disabled)
    fn create(&mut self, ctx: &mut Ctx) {
        let text = self.name_text();
        if self.checked.as_deref() != Some(text.as_str()) {
            self.checked = None;
            self.enable(CREATE_BUTTON, false);
            self.show_message(Message::CheckFirst);
            return;
        }
        if self.busy {
            return;
        }
        self.busy = true;
        self.enable(CHECK_BUTTON, false);
        self.enable(CREATE_BUTTON, false);
        ctx.send(LinkCmd::CreateCharacter {
            face: self.face_value(),
            name: text,
        });
    }
}

impl Screen for Nickname {
    fn name(&self) -> &'static str {
        "nickname"
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    // DISASM: cshell+0x12f3e0 (focus: flags and names cleared, create disabled, the first entry picked, controls 6,
    // 7, 8 and 12 hidden); cshell+0x12c4a0 (with 0: the name box takes the input, the referrer box does not)
    fn enter(&mut self, _ctx: &mut Ctx) {
        self.busy = false;
        self.checked = None;
        self.hover = None;
        self.message = Message::Rule;
        self.enable(CREATE_BUTTON, false);
        self.enable(CHECK_BUTTON, true);
        for id in [AGE, DESCRIPTION, REFERRER_MESSAGE] {
            self.sheet.show(-1, id, WidgetKind::Image, false);
        }
        self.sheet.show(-1, REFERRER_OK, WidgetKind::Button, false);
        // DC-DEFAULT: CLI-1483 (proposed) the referrer box is hidden (the 2009 screen leaves it without input and
        // hides its OK button and message: no referrer can be confirmed)
        self.sheet
            .show(REFERRER_WINDOW, 0, WidgetKind::EditBox, false);
        self.pick(0);
        let i = self.sheet.find(NAME_WINDOW, NAME_BOX, WidgetKind::EditBox);
        self.sheet.focus(i);
    }

    // DC-DEFAULT: CLI-1484 (proposed) Enter presses Create when it is enabled, else Check; Escape presses Cancel (the
    // quit question); Tab does nothing (one input field)
    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        match k {
            MenuKey::Enter => {
                let cmd = if self.enabled(CREATE_BUTTON) {
                    CMD_CREATE
                } else {
                    CMD_CHECK
                };
                if self.enabled(if cmd == CMD_CREATE {
                    CREATE_BUTTON
                } else {
                    CHECK_BUTTON
                }) {
                    self.click(cmd, usize::MAX, ctx);
                }
            }
            MenuKey::Escape => self.click(CMD_CANCEL, usize::MAX, ctx),
            _ => {}
        }
    }

    fn click(&mut self, command: i32, _part: usize, ctx: &mut Ctx) {
        match command {
            CMD_CHECK => self.check(ctx),
            CMD_CREATE => self.create(ctx),
            // EVIDENCE: cshell+0x12e550 (0x12f: box 0xbd3 yes / no; yes -> command 0xb = close the game)
            CMD_CANCEL => {
                let t = ctx.cat.text(text_ids::ASK_QUIT);
                ctx.popups.push(Popup::ask(t, BoxTag::Quit));
            }
            _ => {}
        }
    }

    fn press(&mut self, p: [f32; 2], _ctx: &mut Ctx) -> bool {
        match self.portrait_at(p) {
            Some(i) => {
                self.pick(i);
                true
            }
            None => false,
        }
    }

    // EVIDENCE: cshell+0x12d970 (the highlight shows on the portrait under the pointer and on the chosen one)
    fn hover(&mut self, p: [f32; 2], _data: &MenuData) {
        self.hover = self.portrait_at(p);
    }

    fn event(&mut self, e: &LinkEvent, ctx: &mut Ctx) {
        match e {
            // EVIDENCE: cshell+0x12cb70 (free: message warning_2, the checked name kept, create enabled; taken:
            // checked name dropped, create disabled, message warning_3; flag +0x5e54 cleared either way)
            LinkEvent::NickChecked { name, free } => {
                self.busy = false;
                if *free {
                    self.checked = Some(name.clone());
                    self.enable(CREATE_BUTTON, true);
                    self.show_message(Message::Free);
                } else {
                    self.checked = None;
                    self.enable(CREATE_BUTTON, false);
                    self.show_message(Message::Taken);
                }
            }
            // EVIDENCE: cshell+0x12e9d0 (refused: flags cleared, create disabled, codes 0xe / 0xf -> box 0xfa3,
            // control 9 (check) enabled again)
            LinkEvent::CharacterRefused(code) => {
                self.busy = false;
                self.checked = None;
                self.enable(CREATE_BUTTON, false);
                self.enable(CHECK_BUTTON, true);
                if TAKEN_CODES.contains(code) {
                    ctx.notice(TEXT_TAKEN);
                }
            }
            // success: the server list follows the creation reply and opens the server screen as after a login
            // SPEC: PROTOCOL_2009.md §2.2 0x203 (no screen change on success: the server follows with 0x2000)
            LinkEvent::Servers(_) => {
                self.busy = false;
                ctx.close_wait();
                *ctx.go = Some(Go::Servers);
            }
            // the auth connection failed meanwhile: back to the login screen
            // DC-DEFAULT: CLI-1480 (proposed) a lost auth connection on the nickname screen shows the login failure
            // box and returns to the login screen
            LinkEvent::LoginRefused { text, .. } => {
                self.busy = false;
                ctx.close_wait();
                if !text.is_empty() {
                    eprintln!("dc-client nickname: {text}");
                }
                let t = ctx.cat.text(text_ids::LOGIN_FAILED);
                ctx.popups.push(Popup::error(t, BoxTag::ToLogin));
            }
            _ => {}
        }
    }

    fn pane_figure(&self, _data: &MenuData) -> Option<modelview::ViewReq> {
        let m = self.models.get(self.selected)?;
        let t = self.figure.as_ref()?;
        let ids = m.look_ids();
        let (look, pieces) = t
            .look(&ids)
            .map_or((None, Vec::new()), |(l, p)| (Some(l), p));
        Some(modelview::ViewReq {
            look,
            pieces,
            lights: t.lights(&ids),
            place: PLACE,
            rect: PANE_RECT,
            xray: None,
            showcase: None,
        })
    }

    // EVIDENCE: cshell+0x12d820 (the portraits and their highlights drawn with the screen)
    fn draw_extra(
        &self,
        _cat: &mut Catalog,
        _data: &MenuData,
        _secs: f32,
        out: &mut Vec<DrawItem>,
    ) {
        for (i, m) in self.models.iter().enumerate().take(PORTRAITS_PER_PAGE) {
            let Some(r) = self.portrait_rect(i) else {
                continue;
            };
            out.push(image(&m.small, [r[0], r[1]], [r[2], r[3]]));
            if let Some(h) = &self.art.highlight {
                if self.hover == Some(i) || self.selected == i {
                    out.push(image(h, [r[0], r[1]], [r[2], r[3]]));
                }
            }
        }
    }
}

#[cfg(test)]
mod nickname_tests {
    use super::*;

    #[test]
    fn check_refuses_on_the_client_in_the_2009_order() {
        assert_eq!(check_step("", "robin", false), CheckStep::None);
        assert_eq!(check_step("Rambo", "robin", true), CheckStep::None);
        assert_eq!(
            check_step("ROBIN", "robin", false),
            CheckStep::Refused(Message::LoginId)
        );
        assert_eq!(
            check_step("abc", "robin", false),
            CheckStep::Refused(Message::Length)
        );
        assert_eq!(
            check_step("abcdefghijklm", "robin", false),
            CheckStep::Refused(Message::Length)
        );
        assert_eq!(
            check_step("abcd", "robin", false),
            CheckStep::Ask("abcd".into())
        );
        assert_eq!(
            check_step("abcdefghijkl", "", false),
            CheckStep::Ask("abcdefghijkl".into())
        );
    }

    #[test]
    fn models_skip_other_view_types_and_keep_row_order() {
        let doc = ButeDoc::parse(
            "[Create_Model1]\nFacePieceId = 1\nWeaponID = 12\nViewType = 0\nSmallFaceImg = \"s1.dtx\"\n\
             SmallFaceImgRec = (72,72)\nLargeFaceImg = \"l1.dtx\"\n\
             [Create_Model0]\nFacePieceId = 0\nWeaponID = 12\nViewType = 0\nSmallFaceImg = \"s0.dtx\"\n\
             SmallFaceImgRec = (70,71)\nLargeFaceImg = \"l0.dtx\"\n\
             [Create_Model2]\nFacePieceId = 14\nViewType = 1\n",
        );
        let m = models_from_doc(&doc);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].face, m[0].small_size), (0, [70, 71]));
        assert_eq!((m[1].face, m[1].large.as_str()), (1, "l1.dtx"));
        assert_eq!(m[1].weapon, 12);
    }
}
