//! client-register: account creation inside the client. The login screen's register button opens a code-built
//! dialog (ID, password, password again, an invite code when the server asks for one, Create, Cancel) instead of a
//! browser; Create posts the lobby's registration form (`POST /register` on the lobby's web port, the page's own
//! fields) and asks for the JSON answer (LG-210); the answer shows in the 2009 message box.
//!
//! login-recovery: the same window serves the login screen's "Find ID" and "Find password" buttons (the lobby mails
//! the IDs of an address, or a code that changes the password: `/recover/...` on the web port, CLI-1500 .. CLI-1503).
//!
//! The 2009 buttons opened the publisher's sign-up / find-ID / find-password pages in the system browser and
//! minimised the game: the dialogs are our deviation (CLI-1450, CLI-1501). Their look follows the 2009 code-built
//! windows (the rename window's 478 x 258 frame, the list frame in nine parts for the fields, the window buttons and
//! the window fonts and colours).
// DISASM: cshell+0x11d4e0 (login screen commands: 10000 / 0x2711 / 0x2712 / 0x2713 each minimise the game window
// (ShowWindow 6) and ShellExecuteA "OPEN" the URL behind the pointers VA 0x37548bf8 / 0x37548bfc / 0x37548c00 /
// 0x37548c04: the passport find-ID, find-password and Registration/Signup pages and the game's homepage)
// DC-DEFAULT: CLI-1450 (proposed) the register button opens this dialog; CLI-1451 its fields, keys and boxes;
// CLI-1500 the email row; CLI-1501 .. CLI-1503 the find-ID / find-password dialogs

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use dc_ui_data::names::{read_pinned, CSHELL_REL_PATH};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};

use super::catalog::Catalog;
use super::clanpage::url_encode;
use super::sheet::{place_text, ALIGN_CENTRE, ALIGN_LEFT};
use super::usebox::{
    fill, inside, measure, NameArt, BUTTON_SIZE, CANCEL_TEXT, CAPTION_OFF, FONT, FONT_SIZE,
    MESSAGE_COLOUR, TEXT_COLOUR, TITLE_COLOUR, WARNING_COLOUR,
};
use super::MenuKey;
use crate::hud::items::DrawItem;

/// The registration path of the lobby's web port and the time a request may take.
// DC-DEFAULT: CLI-1451 (proposed) 5 s for the connection and for each read / write
pub const PATH: &str = "/register";
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// login-recovery (CLI-1501): whether the lobby can mail (`GET`), and the three requests (`POST`).
pub const RECOVER_PATH: &str = "/recover";
pub const RECOVER_ID_PATH: &str = "/recover/id";
pub const RECOVER_PASSWORD_PATH: &str = "/recover/password";
pub const RECOVER_CONFIRM_PATH: &str = "/recover/password/confirm";

/// The longest texts the fields take (the lobby's form: names and passwords up to 16, codes up to 20; LG-141).
pub const FIELD_MAX: [usize; 4] = [16, 16, 16, 20];
/// The longest email address the field takes.
// DC-DEFAULT: CLI-1500 (proposed) 64 characters
pub const EMAIL_MAX: usize = 64;
/// The longest mailed code the field takes (the invite code's limit).
pub const CODE_MAX: usize = 20;

/// The dialog's texts (ours: the ROM has no strings for an in-game sign-up).
pub const T_TITLE: &str = "Create account";
pub const T_LABELS: [&str; 4] = ["ID", "Password", "Password again", "Invite code"];
pub const T_CREATE: &str = "Create";
pub const T_WARNING: &str = "Use a password you use nowhere else.";
pub const T_HINT: &str = "Ask the host for an invite code.";
pub const T_EMPTY_ID: &str = "Enter the ID you want.";
pub const T_READY: &str = "The account %s is ready. Enter its password and log in.";
pub const T_OFF: &str = "Registration is off on this server. Ask its host for an account.";
pub const T_UNREACHABLE: &str = "Cannot reach the server's account service. Try again later.";
pub const T_BAD_ANSWER: &str = "The server did not create the account.";
/// login-recovery texts (ours, CLI-1500 .. CLI-1503).
pub const T_EMAIL: &str = "Email";
pub const T_BAD_EMAIL: &str = "Enter a valid email address.";
pub const T_FIND_ID: &str = "Find ID";
pub const T_FIND_PASSWORD: &str = "Find password";
pub const T_LOGIN_ID: &str = "Login ID";
pub const T_CODE: &str = "Code";
pub const T_NEW_PASSWORD: &str = "New password";
pub const T_SEND: &str = "Send";
pub const T_SEND_CODE: &str = "Send code";
pub const T_CHANGE: &str = "Change";
pub const T_BACK: &str = "Back";
pub const T_ENTER_LOGIN: &str = "Enter your login ID.";
pub const T_ENTER_CODE: &str = "Enter the code from the email.";
pub const T_FIND_ID_HINT: &str = "The server mails the IDs of this address to it.";
pub const T_FIND_PASSWORD_HINT: &str = "The server mails a code to the account's address.";
pub const T_CODE_HINT: &str = "Enter the mailed code and a new password.";
pub const T_RECOVER_OFF: &str = "Password recovery is not set up on this server. Ask the host.";
pub const T_MAIL_SENT: &str = "If the address belongs to an account, a mail is on its way.";
pub const T_CHANGED: &str = "The password of %s is changed. Enter it and log in.";
pub const T_NO_ANSWER: &str = "The server did not answer the request.";

/// What the lobby says about registration (`GET /register` as JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterInfo {
    pub open: bool,
    pub invite: bool,
    /// CLI-1500: the lobby wants an email address (`"email": true`; a lobby without the key wants none).
    pub email: bool,
}

/// The form the dialog posts. Its `Debug` form never shows the passwords (CLI-810).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RegisterForm {
    pub name: String,
    pub pw: String,
    pub pw2: String,
    pub code: String,
    /// CLI-1500: the email address, when the lobby asks for one.
    pub email: Option<String>,
}

impl std::fmt::Debug for RegisterForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RegisterForm({}, code {}, ",
            self.name,
            if self.code.is_empty() { "none" } else { "set" }
        )?;
        if self.email.is_some() {
            write!(f, "email set, ")?;
        }
        write!(f, "passwords hidden)")
    }
}

impl RegisterForm {
    /// The form body (the registration page's field names).
    pub fn body(&self) -> String {
        let mut b = format!(
            "name={}&pw={}&pw2={}&code={}",
            url_encode(&self.name),
            url_encode(&self.pw),
            url_encode(&self.pw2),
            url_encode(&self.code)
        );
        if let Some(e) = &self.email {
            b.push_str(&format!("&email={}", url_encode(e)));
        }
        b
    }
}

/// The answer to a post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterReply {
    /// The account is made: its login.
    Created(String),
    /// Refused: the lobby's reason.
    Refused(String),
    /// No answer within the time.
    Unreachable,
}

/// login-recovery (CLI-1501): one request of the find-ID / find-password dialogs. Its `Debug` form never shows the
/// email address, the code or the passwords.
#[derive(Clone, PartialEq, Eq)]
pub enum RecoverRequest {
    /// Mail the IDs of this address.
    Id { email: String },
    /// Mail a code for this account to this address.
    Code { name: String, email: String },
    /// Change the account's password with the mailed code.
    Change {
        name: String,
        code: String,
        pw: String,
        pw2: String,
    },
}

impl std::fmt::Debug for RecoverRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecoverRequest::Id { .. } => write!(f, "FindId(email hidden)"),
            RecoverRequest::Code { name, .. } => write!(f, "PasswordCode({name}, email hidden)"),
            RecoverRequest::Change { name, .. } => {
                write!(f, "PasswordChange({name}, code and passwords hidden)")
            }
        }
    }
}

impl RecoverRequest {
    pub fn path(&self) -> &'static str {
        match self {
            RecoverRequest::Id { .. } => RECOVER_ID_PATH,
            RecoverRequest::Code { .. } => RECOVER_PASSWORD_PATH,
            RecoverRequest::Change { .. } => RECOVER_CONFIRM_PATH,
        }
    }

    /// The form body (the contract's field names).
    pub fn body(&self) -> String {
        match self {
            RecoverRequest::Id { email } => format!("email={}", url_encode(email)),
            RecoverRequest::Code { name, email } => {
                format!("name={}&email={}", url_encode(name), url_encode(email))
            }
            RecoverRequest::Change { name, code, pw, pw2 } => format!(
                "name={}&code={}&pw={}&pw2={}",
                url_encode(name),
                url_encode(code),
                url_encode(pw),
                url_encode(pw2)
            ),
        }
    }
}

/// The answer to a recovery request: `{"ok":bool,"message":string}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverReply {
    /// ok: the lobby's message (may be empty).
    Done(String),
    /// Refused: the lobby's message.
    Refused(String),
    /// No answer within the time.
    Unreachable,
}

pub fn parse_info(body: &str) -> Option<RegisterInfo> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    Some(RegisterInfo {
        open: v.get("open")?.as_bool()?,
        invite: v.get("invite").and_then(|b| b.as_bool()).unwrap_or(true),
        email: v.get("email").and_then(|b| b.as_bool()).unwrap_or(false),
    })
}

pub fn parse_reply(body: &str) -> Option<RegisterReply> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let text = |k: &str| v.get(k).and_then(|s| s.as_str()).map(str::to_string);
    if v.get("ok")?.as_bool()? {
        Some(RegisterReply::Created(text("login")?))
    } else {
        Some(RegisterReply::Refused(
            text("message").unwrap_or_else(|| T_BAD_ANSWER.to_string()),
        ))
    }
}

/// `GET /recover`: whether the lobby can mail (`{"mail": true}`); anything else (an old lobby's 404) = no.
pub fn parse_recover_info(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("mail").and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

pub fn parse_recover_reply(body: &str) -> Option<RecoverReply> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let message = v
        .get("message")
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .to_string();
    if v.get("ok")?.as_bool()? {
        Some(RecoverReply::Done(message))
    } else if message.is_empty() {
        Some(RecoverReply::Refused(T_NO_ANSWER.to_string()))
    } else {
        Some(RecoverReply::Refused(message))
    }
}

/// CLI-1500: the client's own check of an email address: one `@` with something before it, a dot after it with
/// something on both sides, no space. The lobby checks again and its message wins.
pub fn valid_email(s: &str) -> bool {
    let s = s.trim();
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    let dot = domain.find('.');
    !local.is_empty()
        && !domain.contains('@')
        && !s.chars().any(char::is_whitespace)
        && dot.is_some_and(|i| i > 0)
        && !domain.ends_with('.')
}

/// One request to the lobby's web port asking for JSON: (status, body); `None` when no answer came in time.
fn request(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> Option<(u16, String)> {
    // CLI-1470: the web port speaks TLS (the form carries the password); plain only to a loopback lobby without TLS
    // CLI-1610: the identity authority's web port is trusted by its own fingerprint
    let mut c = dc_botclient::tls::connect_with(addr, TIMEOUT, &crate::identity::trust_for(addr)).ok()?;
    let _ = c.tcp().set_read_timeout(Some(TIMEOUT));
    let _ = c.tcp().set_write_timeout(Some(TIMEOUT));
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    if let Some(b) = body {
        req.push_str(&format!(
            "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ));
    } else {
        req.push_str("\r\n");
    }
    c.write_all(req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    c.take(1 << 16).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some((status, body.to_string()))
}

/// Asks the lobby whether it registers (`None`: no answer). A lobby without the JSON answer counts as closed.
pub fn fetch_info(addr: SocketAddr) -> Option<RegisterInfo> {
    let (_, body) = request(addr, "GET", PATH, None)?;
    Some(parse_info(&body).unwrap_or(RegisterInfo {
        open: false,
        invite: false,
        email: false,
    }))
}

/// Posts the form.
pub fn post(addr: SocketAddr, form: &RegisterForm) -> RegisterReply {
    match request(addr, "POST", PATH, Some(&form.body())) {
        None => RegisterReply::Unreachable,
        Some((_, body)) => {
            parse_reply(&body).unwrap_or_else(|| RegisterReply::Refused(T_BAD_ANSWER.to_string()))
        }
    }
}

/// login-recovery: asks the lobby whether it can mail (`None`: no answer; an old lobby's 404 = `Some(false)`).
pub fn fetch_recover_info(addr: SocketAddr) -> Option<bool> {
    let (_, body) = request(addr, "GET", RECOVER_PATH, None)?;
    Some(parse_recover_info(&body))
}

/// login-recovery: posts one request.
pub fn recover(addr: SocketAddr, req: &RecoverRequest) -> RecoverReply {
    match request(addr, "POST", req.path(), Some(&req.body())) {
        None => RecoverReply::Unreachable,
        Some((_, body)) => parse_recover_reply(&body)
            .unwrap_or_else(|| RecoverReply::Refused(T_NO_ANSWER.to_string())),
    }
}

/// The dialog's places: window size, the rows of label + field, the warning line and the buttons (offsets inside
/// the window).
// DC-DEFAULT: CLI-1451 (proposed) the 2009 code-built windows' size (478 x 258), title place and button row (y 207);
// one row per field every 30 px from y 36, fields 260 x 26; CLI-1500: a fifth row makes the window one row taller
pub const WINDOW_W: i32 = 478;
pub const WINDOW_H: i32 = 258;
pub const TITLE_AT: [i32; 2] = [10, 7];
pub const ROW_Y0: i32 = 36;
pub const ROW_DY: i32 = 30;
pub const LABEL_X: i32 = 30;
pub const FIELD_X: i32 = 168;
pub const FIELD_SIZE: [i32; 2] = [260, 26];
// the message boxes' Yes and No columns (popup::YES_AT, NO_AT), so buttons and captions sit as in every popup
pub const CREATE_X: i32 = super::popup::YES_AT;
pub const CANCEL_X: i32 = super::popup::NO_AT;
/// Three buttons (the code step: Change, Back, Cancel): the popup columns' 122 px pitch, centred in the window.
// DC-DEFAULT: CLI-1502 (proposed) x 67 / 189 / 311
pub const THREE_X: [i32; 3] = [67, 189, 311];
pub const BUTTONS_Y: i32 = 207;
/// The menu's virtual screen (draw items and pointer inputs; the window scales it to its pixel size).
const VIRTUAL_W: i32 = dc_ui_data::scale::VIRTUAL_WIDTH as i32;
const VIRTUAL_H: i32 = dc_ui_data::scale::VIRTUAL_HEIGHT as i32;

/// The textures: the code-built windows' art and the list frame for the fields.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SignupArt {
    pub name: NameArt,
    pub field: Option<String>,
}

impl SignupArt {
    pub fn from_rom(rom: Option<&Path>) -> SignupArt {
        let field = rom
            .and_then(|r| {
                dc_rom::PeFile::from_file(&r.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1])).ok()
            })
            .and_then(|pe| read_pinned(&pe, &super::dropdown::FRAME_TEX).ok());
        SignupArt {
            name: NameArt::from_rom(rom),
            field,
        }
    }
}

/// Which dialog the window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Create account (CLI-1451), with the invite code row and the email row when the lobby asks for them.
    Register { invite: bool, email: bool },
    /// Find ID: the email address (CLI-1501).
    FindId,
    /// Find password, step 1: login ID and email address.
    PasswordCode,
    /// Find password, step 2: the mailed code and the new password twice.
    PasswordChange,
}

/// What a field holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Password,
    Again,
    Email,
    Invite,
    Code,
}

/// One row of the dialog: label, what it holds, its length limit, masked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub label: &'static str,
    pub field: Field,
    pub max: usize,
    pub masked: bool,
}

const fn row(label: &'static str, field: Field, max: usize, masked: bool) -> Row {
    Row {
        label,
        field,
        max,
        masked,
    }
}

impl Kind {
    pub fn rows(self) -> Vec<Row> {
        match self {
            Kind::Register { invite, email } => {
                let mut v = vec![
                    row(T_LABELS[0], Field::Name, FIELD_MAX[0], false),
                    row(T_LABELS[1], Field::Password, FIELD_MAX[1], true),
                    row(T_LABELS[2], Field::Again, FIELD_MAX[2], true),
                ];
                if email {
                    v.push(row(T_EMAIL, Field::Email, EMAIL_MAX, false));
                }
                if invite {
                    v.push(row(T_LABELS[3], Field::Invite, FIELD_MAX[3], false));
                }
                v
            }
            Kind::FindId => vec![row(T_EMAIL, Field::Email, EMAIL_MAX, false)],
            Kind::PasswordCode => vec![
                row(T_LOGIN_ID, Field::Name, FIELD_MAX[0], false),
                row(T_EMAIL, Field::Email, EMAIL_MAX, false),
            ],
            Kind::PasswordChange => vec![
                row(T_CODE, Field::Code, CODE_MAX, false),
                row(T_NEW_PASSWORD, Field::Password, FIELD_MAX[1], true),
                row(T_LABELS[2], Field::Again, FIELD_MAX[2], true),
            ],
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Kind::Register { .. } => T_TITLE,
            Kind::FindId => T_FIND_ID,
            Kind::PasswordCode | Kind::PasswordChange => T_FIND_PASSWORD,
        }
    }

    /// The first button's caption.
    pub fn submit(self) -> &'static str {
        match self {
            Kind::Register { .. } => T_CREATE,
            Kind::FindId => T_SEND,
            Kind::PasswordCode => T_SEND_CODE,
            Kind::PasswordChange => T_CHANGE,
        }
    }

    pub fn buttons(self) -> &'static [Button] {
        match self {
            Kind::PasswordChange => &[Button::Submit, Button::Back, Button::Cancel],
            _ => &[Button::Submit, Button::Cancel],
        }
    }

    /// The line under the fields when no message is up.
    fn hint(self) -> Option<&'static str> {
        match self {
            Kind::Register { invite: true, .. } => Some(T_HINT),
            Kind::Register { .. } => None,
            Kind::FindId => Some(T_FIND_ID_HINT),
            Kind::PasswordCode => Some(T_FIND_PASSWORD_HINT),
            Kind::PasswordChange => Some(T_CODE_HINT),
        }
    }

    /// The red line: the windows that take a new password.
    fn warning(self) -> Option<&'static str> {
        match self {
            Kind::Register { .. } | Kind::PasswordChange => Some(T_WARNING),
            _ => None,
        }
    }
}

/// A button of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Create / Send / Send code / Change.
    Submit,
    /// Back to the first step (the code step only).
    Back,
    Cancel,
}

/// A keyboard stop of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Field(usize),
    /// The first button (Create, Send, Send code, Change).
    Create,
    Back,
    Cancel,
}

/// What an input did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    None,
    /// Post this registration form.
    Create(RegisterForm),
    /// Post this recovery request.
    Recover(RecoverRequest),
    /// Went back to the first step of the password dialog.
    Back,
    Cancel,
}

/// The open dialog. Its `Debug` form shows no field text (passwords, codes and addresses stay hidden, CLI-1451).
#[derive(Clone, PartialEq)]
pub struct SignupBox {
    pub kind: Kind,
    /// One text per row of [`Kind::rows`].
    pub texts: Vec<String>,
    /// The password dialog's login ID and email address from its first step (kept for the code step and Back).
    pub carry: [String; 2],
    /// The keyboard stop (index into [`SignupBox::stops`]).
    pub focus: usize,
    /// A post waits for its answer.
    pub waiting: bool,
    /// A local message (the empty ID, a bad address) above the buttons.
    pub message: Option<&'static str>,
    pub at: [i32; 2],
    /// The button under the pointer / held down (index into [`Kind::buttons`]), tracked as the message boxes do.
    pub hover: Option<usize>,
    pub down: Option<usize>,
}

impl std::fmt::Debug for SignupBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SignupBox({:?}, focus {}, waiting {}, at {:?}, texts hidden)",
            self.kind, self.focus, self.waiting, self.at
        )
    }
}

const TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};

impl SignupBox {
    /// The register dialog without the email row, centred on the menu's virtual 1024 x 768 screen (the space every
    /// menu item and pointer input uses, whatever the window's pixel size); `invite`: the code row is shown.
    pub fn centred(invite: bool) -> SignupBox {
        SignupBox::centred_as(Kind::Register {
            invite,
            email: false,
        })
    }

    /// Any dialog centred on the virtual screen.
    pub fn centred_as(kind: Kind) -> SignupBox {
        SignupBox::new(kind, VIRTUAL_W, VIRTUAL_H)
    }

    /// The register dialog (no email row) centred on a `w` x `h` area of virtual pixels.
    pub fn open(invite: bool, w: i32, h: i32) -> SignupBox {
        SignupBox::new(
            Kind::Register {
                invite,
                email: false,
            },
            w,
            h,
        )
    }

    /// A dialog centred on a `w` x `h` area of virtual pixels (not window pixels: see [`SignupBox::centred`]).
    pub fn new(kind: Kind, w: i32, h: i32) -> SignupBox {
        let mut b = SignupBox {
            kind,
            texts: vec![String::new(); kind.rows().len()],
            carry: Default::default(),
            focus: 0,
            waiting: false,
            message: None,
            at: [0, 0],
            hover: None,
            down: None,
        };
        b.at = [(w - WINDOW_W) / 2, (h - b.height()) / 2];
        b
    }

    /// The whole window lies on the virtual screen (a modal dialog the user cannot see must not hold the input).
    pub fn visible(&self) -> bool {
        self.at[0] >= 0
            && self.at[1] >= 0
            && self.at[0] + WINDOW_W <= VIRTUAL_W
            && self.at[1] + self.height() <= VIRTUAL_H
    }

    pub fn rows(&self) -> usize {
        self.texts.len()
    }

    /// Rows past four make the window taller.
    fn extra(&self) -> i32 {
        (self.rows() as i32 - 4).max(0) * ROW_DY
    }

    /// The frame art's rectangle (x, y, w, h): the whole dialog.
    pub fn frame_rect(&self) -> [i32; 4] {
        [self.at[0], self.at[1], WINDOW_W, self.height()]
    }

    /// The y of the message line and of the warning line: anchored to the buttons, so the margin to the bottom
    /// edge is the same for every kind and row count.
    fn lines_y(&self) -> i32 {
        let n = 1 + i32::from(self.kind.warning().is_some());
        self.buttons_y() - 4 - n * (FONT_SIZE + 4)
    }

    pub fn height(&self) -> i32 {
        WINDOW_H + self.extra()
    }

    /// The first row's y offset (fewer than four rows sit lower, centred on the four-row block).
    fn row_y0(&self) -> i32 {
        ROW_Y0 + (4 - self.rows() as i32).max(0) * ROW_DY / 2
    }

    /// The tab order: the fields, then the buttons (the login screen's: boxes first, then its button).
    pub fn stops(&self) -> Vec<Stop> {
        let mut v: Vec<Stop> = (0..self.rows()).map(Stop::Field).collect();
        v.extend(self.kind.buttons().iter().map(|b| match b {
            Button::Submit => Stop::Create,
            Button::Back => Stop::Back,
            Button::Cancel => Stop::Cancel,
        }));
        v
    }

    pub fn stop(&self) -> Stop {
        let s = self.stops();
        s[self.focus.min(s.len() - 1)]
    }

    fn field(&self) -> Option<usize> {
        match self.stop() {
            Stop::Field(i) => Some(i),
            _ => None,
        }
    }

    /// The row holding `f`.
    pub fn row_of(&self, f: Field) -> Option<usize> {
        self.kind.rows().iter().position(|r| r.field == f)
    }

    /// The text of the `f` row (names, addresses and codes trimmed; passwords as typed); empty without the row.
    pub fn text(&self, f: Field) -> String {
        let Some(i) = self.row_of(f) else {
            return String::new();
        };
        match f {
            Field::Password | Field::Again => self.texts[i].clone(),
            _ => self.texts[i].trim().to_string(),
        }
    }

    pub fn field_rect(&self, i: usize) -> [i32; 4] {
        [
            self.at[0] + FIELD_X,
            self.at[1] + self.row_y0() + i as i32 * ROW_DY,
            FIELD_SIZE[0],
            FIELD_SIZE[1],
        ]
    }

    fn buttons_y(&self) -> i32 {
        self.at[1] + BUTTONS_Y + self.extra()
    }

    /// The `i`th button's place.
    pub fn button_place(&self, i: usize) -> [i32; 2] {
        let x = match self.kind.buttons().len() {
            3 => THREE_X[i.min(2)],
            _ if i == 0 => CREATE_X,
            _ => CANCEL_X,
        };
        [self.at[0] + x, self.buttons_y()]
    }

    fn place_of(&self, b: Button) -> [i32; 2] {
        let i = self.kind.buttons().iter().position(|x| *x == b).unwrap_or(0);
        self.button_place(i)
    }

    /// The first button (Create / Send / Send code / Change).
    pub fn create_at(&self) -> [i32; 2] {
        self.place_of(Button::Submit)
    }

    pub fn back_at(&self) -> Option<[i32; 2]> {
        self.kind
            .buttons()
            .contains(&Button::Back)
            .then(|| self.place_of(Button::Back))
    }

    pub fn cancel_at(&self) -> [i32; 2] {
        self.place_of(Button::Cancel)
    }

    pub fn form(&self) -> RegisterForm {
        let email = matches!(self.kind, Kind::Register { email: true, .. });
        RegisterForm {
            name: self.text(Field::Name),
            pw: self.text(Field::Password),
            pw2: self.text(Field::Again),
            code: self.text(Field::Invite),
            email: email.then(|| self.text(Field::Email)),
        }
    }

    pub fn type_char(&mut self, c: char) {
        let Some(i) = self.field() else {
            return;
        };
        let max = self.kind.rows()[i].max;
        if self.waiting || c.is_control() || self.texts[i].chars().count() >= max {
            return;
        }
        self.texts[i].push(c);
    }

    pub fn backspace(&mut self) {
        if let Some(i) = self.field().filter(|_| !self.waiting) {
            self.texts[i].pop();
        }
    }

    /// A local refusal: the message and the keyboard in the row of `f`.
    fn refuse(&mut self, m: &'static str, f: Field) -> Step {
        self.message = Some(m);
        self.focus = self.row_of(f).unwrap_or(0);
        Step::None
    }

    /// The first button: the form or request, unless a post waits or a field fails the client's check (the
    /// field gets the keyboard and the message shows).
    pub fn create(&mut self) -> Step {
        if self.waiting {
            return Step::None;
        }
        match self.kind {
            Kind::Register { .. } if self.text(Field::Name).is_empty() => {
                return self.refuse(T_EMPTY_ID, Field::Name)
            }
            Kind::PasswordCode if self.text(Field::Name).is_empty() => {
                return self.refuse(T_ENTER_LOGIN, Field::Name)
            }
            Kind::PasswordChange if self.text(Field::Code).is_empty() => {
                return self.refuse(T_ENTER_CODE, Field::Code)
            }
            _ => {}
        }
        if self.row_of(Field::Email).is_some() && !valid_email(&self.text(Field::Email)) {
            return self.refuse(T_BAD_EMAIL, Field::Email);
        }
        self.message = None;
        self.waiting = true;
        match self.kind {
            Kind::Register { .. } => Step::Create(self.form()),
            Kind::FindId => Step::Recover(RecoverRequest::Id {
                email: self.text(Field::Email),
            }),
            Kind::PasswordCode => Step::Recover(RecoverRequest::Code {
                name: self.text(Field::Name),
                email: self.text(Field::Email),
            }),
            Kind::PasswordChange => Step::Recover(RecoverRequest::Change {
                name: self.carry[0].clone(),
                code: self.text(Field::Code),
                pw: self.text(Field::Password),
                pw2: self.text(Field::Again),
            }),
        }
    }

    /// The code was mailed: the password dialog's second step (code, new password twice), keyboard in the code.
    pub fn code_sent(&mut self) {
        if self.kind != Kind::PasswordCode {
            return;
        }
        self.carry = [self.text(Field::Name), self.text(Field::Email)];
        self.kind = Kind::PasswordChange;
        self.texts = vec![String::new(); self.kind.rows().len()];
        self.reset_step();
    }

    /// Back: the first step again with its login ID and address.
    pub fn back(&mut self) -> Step {
        if self.kind != Kind::PasswordChange || self.waiting {
            return Step::None;
        }
        self.kind = Kind::PasswordCode;
        self.texts = self.carry.to_vec();
        self.reset_step();
        Step::Back
    }

    fn reset_step(&mut self) {
        self.focus = 0;
        self.message = None;
        self.waiting = false;
        self.hover = None;
        self.down = None;
    }

    /// A key: Tab / Shift+Tab walk the stops, Enter runs the first button (the focused button on a button stop),
    /// Escape cancels.
    pub fn key(&mut self, k: MenuKey) -> Step {
        let n = self.stops().len();
        match k {
            MenuKey::Tab => self.focus = (self.focus + 1) % n,
            MenuKey::BackTab => self.focus = (self.focus + n - 1) % n,
            MenuKey::Enter => {
                return match self.stop() {
                    Stop::Cancel => Step::Cancel,
                    Stop::Back => self.back(),
                    _ => self.create(),
                }
            }
            MenuKey::Escape => return Step::Cancel,
            MenuKey::Backspace => self.backspace(),
            _ => {}
        }
        Step::None
    }

    /// The button (index into [`Kind::buttons`]) at `p`.
    fn button_at(&self, p: [f32; 2]) -> Option<usize> {
        (0..self.kind.buttons().len()).find(|i| inside(self.button_place(*i), BUTTON_SIZE, p))
    }

    /// The pointer moved; true = it came onto a button (the select-change sound).
    pub fn moved(&mut self, p: [f32; 2]) -> bool {
        let now = self.button_at(p);
        let onto = now.is_some() && now != self.hover;
        self.hover = now;
        onto
    }

    /// A press at `p` (the dialog is modal: every press is its): a button goes down, a field takes the focus.
    pub fn press(&mut self, p: [f32; 2]) {
        self.down = self.button_at(p);
        if self.down.is_some() {
            return;
        }
        for i in 0..self.rows() {
            let r = self.field_rect(i);
            if inside([r[0], r[1]], [r[2], r[3]], p) {
                self.focus = i;
            }
        }
    }

    /// The button went up at `p`: the button pressed and released on runs. Returns the step and whether an
    /// enabled button was clicked (the select sound).
    pub fn release(&mut self, p: [f32; 2]) -> (Step, bool) {
        let was = self.down.take();
        let Some(i) = was.filter(|_| was == self.button_at(p)) else {
            return (Step::None, false);
        };
        let enabled = !self.waiting;
        match self.kind.buttons()[i] {
            Button::Submit => (self.create(), enabled),
            Button::Back => (self.back(), enabled),
            Button::Cancel => (Step::Cancel, true),
        }
    }

    pub fn draw(&self, cat: &mut Catalog, art: &SignupArt, secs: f32, out: &mut Vec<DrawItem>) {
        let size = [WINDOW_W, self.height()];
        // the rename window's frame art in nine parts over the dialog's own rectangle (corners and title bar at
        // natural size, the middle stretched), so extra rows never push the buttons off the picture
        let framed = match art.name.window.as_deref() {
            Some(t) => super::nine::draw(cat, t, self.frame_rect(), [255; 4], out),
            None => 0,
        };
        if framed == 0 {
            out.push(fill(self.at, size, [24, 24, 24, 240]));
        }
        let text = |s: &str, at: [i32; 2], colour: u32| {
            place_text(s, at, [0, 0], TL, FONT, FONT_SIZE, colour)
        };
        out.push(text(
            self.kind.title(),
            [self.at[0] + TITLE_AT[0], self.at[1] + TITLE_AT[1]],
            TITLE_COLOUR,
        ));
        let focus = self.field();
        for (i, r) in self.kind.rows().iter().enumerate() {
            let f = self.field_rect(i);
            out.push(place_text(
                r.label,
                [self.at[0] + LABEL_X, f[1]],
                [0, f[3]],
                Align {
                    first: ALIGN_LEFT,
                    second: ALIGN_SECOND_CENTRE,
                },
                FONT,
                FONT_SIZE,
                TEXT_COLOUR,
            ));
            let drawn = match art.field.as_deref() {
                Some(t) => super::nine::draw(cat, t, f, [255; 4], out),
                None => 0,
            };
            if drawn == 0 {
                out.push(fill([f[0], f[1]], [f[2], f[3]], [0, 0, 0, 204]));
            }
            let mut shown = if r.masked {
                "*".repeat(self.texts[i].chars().count())
            } else {
                self.texts[i].clone()
            };
            // CLI-1500: a text longer than its field shows its end (the caret stays in the field)
            while !shown.is_empty() && measure(cat, &shown) > f[2] - 20 {
                shown.remove(0);
            }
            let at = [f[0] + 8, f[1]];
            out.push(place_text(
                &shown,
                at,
                [0, f[3]],
                Align {
                    first: ALIGN_LEFT,
                    second: ALIGN_SECOND_CENTRE,
                },
                FONT,
                FONT_SIZE,
                if focus == Some(i) { 0xffff_ffff } else { TEXT_COLOUR },
            ));
            if focus == Some(i) && !self.waiting && super::sheet::caret_on(secs, 0.0) {
                let x = at[0] + measure(cat, &shown) + 1;
                out.push(fill(
                    [x, f[1] + 6],
                    [super::sheet::CARET_WIDTH, f[3] - 12],
                    [238, 238, 238, 255],
                ));
            }
        }
        let below = self.lines_y();
        let line = |s: &str, k: i32, colour: u32| {
            place_text(
                s,
                [self.at[0] + WINDOW_W / 2, below + k * (FONT_SIZE + 4)],
                [0, 0],
                Align {
                    first: ALIGN_CENTRE,
                    second: 0,
                },
                FONT,
                FONT_SIZE,
                colour,
            )
        };
        if let Some(m) = self.message.or(self.kind.hint()) {
            out.push(line(m, 0, MESSAGE_COLOUR));
        }
        if let Some(w) = self.kind.warning() {
            out.push(line(w, 1, WARNING_COLOUR));
        }
        // the buttons as the message boxes draw them: the popup button pictures (normal / hover / down) with the
        // caption centred on (50, 12) of the button, hover = the pointer, else the focused stop
        let focus_button = match focus {
            Some(_) => None,
            None => self.focus.checked_sub(self.rows()),
        };
        let (hover, down) = (self.hover.or(focus_button), self.down);
        for (i, b) in self.kind.buttons().iter().enumerate() {
            let at = self.button_place(i);
            let caption = match b {
                Button::Submit => self.kind.submit().to_string(),
                Button::Back => T_BACK.to_string(),
                Button::Cancel => cat.text(CANCEL_TEXT),
            };
            let on = *b == Button::Cancel || !self.waiting;
            let look = if hover == Some(i) && down == Some(i) {
                2
            } else if hover == Some(i) {
                1
            } else {
                0
            };
            let tex = if on {
                cat.paths.popup_buttons[look]
                    .clone()
                    .or_else(|| art.name.button.clone())
            } else {
                art.name.button_off.clone()
            };
            match tex {
                Some(t) => out.push(super::sheet::image_fit(cat, &t, at, BUTTON_SIZE)),
                None => out.push(fill(at, BUTTON_SIZE, [60, 60, 60, 255])),
            }
            out.push(place_text(
                &caption,
                [
                    at[0] + super::popup::CAPTION_AT[0],
                    at[1] + super::popup::CAPTION_AT[1],
                ],
                [0, 0],
                Align {
                    first: ALIGN_CENTRE,
                    second: ALIGN_SECOND_CENTRE,
                },
                super::popup::CAPTION_FONT,
                super::popup::CAPTION_SIZE,
                if on { super::popup::CAPTION_COLOUR } else { CAPTION_OFF },
            ));
        }
    }
}

/// The success box's text.
pub fn ready_message(login: &str) -> String {
    T_READY.replace("%s", login)
}

/// login-recovery: the box after a changed password.
pub fn changed_message(login: &str) -> String {
    T_CHANGED.replace("%s", login)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_body_escapes_and_the_debug_form_hides_the_passwords() {
        let f = RegisterForm {
            name: "Neo".into(),
            pw: "p&ss w=1".into(),
            pw2: "p&ss w=1".into(),
            code: "AB CD".into(),
            email: None,
        };
        assert_eq!(
            f.body(),
            "name=Neo&pw=p%26ss%20w%3D1&pw2=p%26ss%20w%3D1&code=AB%20CD"
        );
        let d = format!("{f:?}");
        assert!(!d.contains("p&ss") && d.contains("Neo"), "{d}");
    }

    #[test]
    fn json_answers_parse() {
        assert_eq!(
            parse_info(r#"{"open":true,"invite":false}"#),
            Some(RegisterInfo {
                open: true,
                invite: false,
                email: false
            })
        );
        assert!(parse_info(r#"{"open":true,"invite":false,"email":true}"#).unwrap().email);
        assert_eq!(parse_info("<html>"), None);
        assert_eq!(
            parse_reply(r#"{"ok":true,"login":"Neo"}"#),
            Some(RegisterReply::Created("Neo".into()))
        );
        assert_eq!(
            parse_reply(r#"{"ok":false,"message":"Cannot register: that name is taken."}"#),
            Some(RegisterReply::Refused(
                "Cannot register: that name is taken.".into()
            ))
        );
        assert_eq!(parse_reply("<html>"), None);
        assert_eq!(
            ready_message("Neo"),
            "The account Neo is ready. Enter its password and log in."
        );
    }

    #[test]
    fn tab_order_enter_escape_and_the_code_row() {
        let mut b = SignupBox::open(false, 1024, 768);
        assert_eq!(
            b.stops(),
            vec![
                Stop::Field(0),
                Stop::Field(1),
                Stop::Field(2),
                Stop::Create,
                Stop::Cancel
            ]
        );
        assert_eq!(
            b.key(MenuKey::Enter),
            Step::None,
            "an empty ID posts nothing"
        );
        assert_eq!(b.message, Some(T_EMPTY_ID));
        for c in "Neo".chars() {
            b.type_char(c);
        }
        b.key(MenuKey::Tab);
        for c in "secret1".chars() {
            b.type_char(c);
        }
        b.key(MenuKey::Tab);
        for c in "secret1".chars() {
            b.type_char(c);
        }
        b.key(MenuKey::Backspace);
        b.type_char('1');
        b.key(MenuKey::Tab);
        assert_eq!(b.stop(), Stop::Create);
        b.key(MenuKey::Tab);
        assert_eq!(b.stop(), Stop::Cancel);
        assert_eq!(b.key(MenuKey::Enter), Step::Cancel);
        b.key(MenuKey::Tab);
        assert_eq!(b.stop(), Stop::Field(0));
        b.key(MenuKey::BackTab);
        assert_eq!(b.stop(), Stop::Cancel);
        b.focus = 1;
        let Step::Create(f) = b.key(MenuKey::Enter) else {
            panic!("Enter on a field creates");
        };
        assert_eq!(f.body(), "name=Neo&pw=secret1&pw2=secret1&code=");
        assert!(b.waiting);
        assert_eq!(b.key(MenuKey::Enter), Step::None, "one post at a time");
        assert_eq!(b.key(MenuKey::Escape), Step::Cancel);
        let mut k = SignupBox::open(true, 1024, 768);
        assert_eq!(k.stops().len(), 6);
        assert!(k.field_rect(0)[1] < b.field_rect(0)[1]);
        k.focus = 3;
        for c in "x".repeat(30).chars() {
            k.type_char(c);
        }
        assert_eq!(k.texts[3].len(), FIELD_MAX[3]);
    }

    #[test]
    fn the_dialog_sits_on_the_virtual_screen() {
        // register-dialog-fix: the window's pixel size (2560 x 1440) put the dialog at x 1041, off the 1024 wide
        // virtual screen, while it kept every input
        let b = SignupBox::centred(false);
        assert_eq!(b.at, [(1024 - WINDOW_W) / 2, (768 - WINDOW_H) / 2]);
        assert!(b.visible());
        assert!(!SignupBox::open(false, 2560, 1440).visible());
    }

    #[test]
    fn presses_hit_the_fields_and_buttons() {
        let mut b = SignupBox::open(true, 1024, 768);
        let r = b.field_rect(2);
        b.press([(r[0] + 3) as f32, (r[1] + 3) as f32]);
        assert_eq!(b.stop(), Stop::Field(2));
        // a button runs on its release, like the message boxes' buttons
        let c = b.cancel_at();
        let on = [(c[0] + 3) as f32, (c[1] + 3) as f32];
        b.press(on);
        assert_eq!(b.down, Some(1));
        assert_eq!(b.release(on), (Step::Cancel, true));
        // pressed on one button, released on the other or off both: nothing
        b.press(on);
        let c = b.create_at();
        assert_eq!(b.release([(c[0] + 3) as f32, (c[1] + 3) as f32]).0, Step::None);
        b.texts[0] = "Neo".into();
        b.press([(c[0] + 3) as f32, (c[1] + 3) as f32]);
        let (step, clicked) = b.release([(c[0] + 3) as f32, (c[1] + 3) as f32]);
        assert!(matches!(step, Step::Create(_)) && clicked);
        // waiting: Create is off and makes no sound
        b.press([(c[0] + 3) as f32, (c[1] + 3) as f32]);
        assert_eq!(b.release([(c[0] + 3) as f32, (c[1] + 3) as f32]), (Step::None, false));
    }

    #[test]
    fn the_pointer_hover_and_one_select_change_per_button() {
        let mut b = SignupBox::open(false, 1024, 768);
        let c = b.create_at();
        let p = [(c[0] + 5) as f32, (c[1] + 5) as f32];
        assert!(b.moved(p));
        assert_eq!(b.hover, Some(0));
        assert!(!b.moved([p[0] + 1.0, p[1]]), "no second sound on the same button");
        assert!(!b.moved([2.0, 2.0]));
        assert_eq!(b.hover, None);
    }

    #[test]
    fn captions_sit_inside_their_buttons_centred_on_the_picture() {
        use crate::menu::popup::CAPTION_AT;
        // the caption's anchor (50, 12) is the middle of the 100 x 25 button picture and lies inside the
        // 120 x 25 control rectangle; the buttons are the message boxes' Yes / No columns
        assert!(CAPTION_AT[0] > 0 && CAPTION_AT[0] < BUTTON_SIZE[0]);
        assert!(CAPTION_AT[1] > 0 && CAPTION_AT[1] < BUTTON_SIZE[1]);
        assert!((CAPTION_AT[0] - 100 / 2).abs() <= 2);
        assert!((CAPTION_AT[1] - 25 / 2).abs() <= 2);
        assert_eq!((CREATE_X, CANCEL_X), (crate::menu::popup::YES_AT, crate::menu::popup::NO_AT));
    }

    // login-recovery (CLI-1500): the email row comes after the passwords and before the invite code, the window
    // grows by one row with five rows, and a bad address posts nothing
    #[test]
    fn the_email_row_checks_the_address_and_joins_the_form() {
        let mut b = SignupBox::centred_as(Kind::Register {
            invite: true,
            email: true,
        });
        assert_eq!(b.rows(), 5);
        assert_eq!(b.row_of(Field::Email), Some(3));
        assert_eq!(b.row_of(Field::Invite), Some(4));
        assert_eq!(b.height(), WINDOW_H + ROW_DY);
        assert!(b.visible());
        assert!(b.create_at()[1] > b.field_rect(4)[1] + ROW_DY);
        b.texts = vec!["Neo".into(), "pw".into(), "pw".into(), "neo@mail".into(), "C".into()];
        assert_eq!(b.create(), Step::None);
        assert_eq!((b.message, b.stop()), (Some(T_BAD_EMAIL), Stop::Field(3)));
        b.texts[3] = " neo@mail.example ".into();
        let Step::Create(f) = b.create() else {
            panic!("a good address posts");
        };
        assert_eq!(f.body(), "name=Neo&pw=pw&pw2=pw&code=C&email=neo%40mail.example");
        assert!(!format!("{f:?}").contains("mail.example"));
        // an old lobby: no email row, no email field in the body
        let old = SignupBox::centred(false);
        assert_eq!(old.form().email, None);
        for (a, ok) in [
            ("a@b.c", true),
            ("a@b", false),
            ("@b.c", false),
            ("a@@b.c", false),
            ("a@b@c.d", false),
            ("a@.c", false),
            ("a@b.", false),
            ("a b@c.d", false),
            ("", false),
        ] {
            assert_eq!(valid_email(a), ok, "{a}");
        }
    }

    #[test]
    fn find_id_posts_the_address() {
        let mut b = SignupBox::centred_as(Kind::FindId);
        assert_eq!(b.stops(), vec![Stop::Field(0), Stop::Create, Stop::Cancel]);
        for c in "x@y.z".chars() {
            b.type_char(c);
        }
        let Step::Recover(r) = b.key(MenuKey::Enter) else {
            panic!("Enter sends");
        };
        assert_eq!((r.path(), r.body()), (RECOVER_ID_PATH, "email=x%40y.z".to_string()));
        assert_eq!(format!("{r:?}"), "FindId(email hidden)");
        assert!(b.waiting);
    }

    #[test]
    fn find_password_two_steps_and_back() {
        let mut b = SignupBox::centred_as(Kind::PasswordCode);
        assert_eq!(b.key(MenuKey::Enter), Step::None);
        assert_eq!(b.message, Some(T_ENTER_LOGIN));
        b.texts = vec!["Neo".into(), "neo@mail.example".into()];
        let Step::Recover(r) = b.key(MenuKey::Enter) else {
            panic!("Send code");
        };
        assert_eq!(r.path(), RECOVER_PASSWORD_PATH);
        assert_eq!(r.body(), "name=Neo&email=neo%40mail.example");
        b.code_sent();
        assert_eq!(b.kind, Kind::PasswordChange);
        assert!(!b.waiting);
        assert_eq!(
            b.stops(),
            vec![
                Stop::Field(0),
                Stop::Field(1),
                Stop::Field(2),
                Stop::Create,
                Stop::Back,
                Stop::Cancel
            ]
        );
        assert_eq!(b.key(MenuKey::Enter), Step::None);
        assert_eq!(b.message, Some(T_ENTER_CODE));
        for (i, t) in ["AB12", "n&w pass1", "n&w pass1"].iter().enumerate() {
            b.focus = i;
            for c in t.chars() {
                b.type_char(c);
            }
        }
        b.focus = 0;
        let Step::Recover(r) = b.key(MenuKey::Enter) else {
            panic!("Change");
        };
        assert_eq!(r.path(), RECOVER_CONFIRM_PATH);
        assert_eq!(r.body(), "name=Neo&code=AB12&pw=n%26w%20pass1&pw2=n%26w%20pass1");
        let d = format!("{r:?} {b:?}");
        assert!(!d.contains("AB12") && !d.contains("pass1") && !d.contains("mail"), "{d}");
        b.waiting = false;
        // the three buttons sit in their own columns; Back returns to step 1 with its fields
        let [x0, x1, x2] = [b.create_at()[0], b.back_at().unwrap()[0], b.cancel_at()[0]];
        assert!(x0 < x1 && x1 < x2);
        let back = b.back_at().unwrap();
        let on = [(back[0] + 5) as f32, (back[1] + 5) as f32];
        b.press(on);
        assert_eq!(b.release(on), (Step::Back, true));
        assert_eq!(b.kind, Kind::PasswordCode);
        assert_eq!(b.texts, vec!["Neo".to_string(), "neo@mail.example".to_string()]);
    }

    #[test]
    fn recovery_answers_parse() {
        assert!(parse_recover_info(r#"{"mail":true}"#));
        assert!(!parse_recover_info(r#"{"mail":false}"#));
        assert!(!parse_recover_info("Not found"));
        assert_eq!(
            parse_recover_reply(r#"{"ok":true,"message":"Sent."}"#),
            Some(RecoverReply::Done("Sent.".into()))
        );
        assert_eq!(
            parse_recover_reply(r#"{"ok":false,"message":"Wrong code."}"#),
            Some(RecoverReply::Refused("Wrong code.".into()))
        );
        assert_eq!(
            parse_recover_reply(r#"{"ok":false}"#),
            Some(RecoverReply::Refused(T_NO_ANSWER.into()))
        );
        assert_eq!(parse_recover_reply("<html>"), None);
        assert_eq!(
            changed_message("Neo"),
            "The password of Neo is changed. Enter it and log in."
        );
    }

    // signup-frame: the frame art covers the whole dialog for every kind and row count, and the message line and
    // buttons keep one margin from the bottom edge
    #[test]
    fn the_frame_covers_every_field_line_and_button_of_every_kind() {
        let kinds = [
            Kind::Register { invite: false, email: false },
            Kind::Register { invite: true, email: false },
            Kind::Register { invite: false, email: true },
            Kind::Register { invite: true, email: true },
            Kind::FindId,
            Kind::PasswordCode,
            Kind::PasswordChange,
        ];
        let mut cat = Catalog::fallback(None, 1024, 768);
        let mut margins = std::collections::BTreeSet::new();
        for kind in kinds {
            let mut b = SignupBox::centred_as(kind);
            b.message = Some("m");
            let mut out = Vec::new();
            b.draw(&mut cat, &SignupArt::default(), 0.0, &mut out);
            let Some(DrawItem::Fill { rect, .. }) = out.first() else {
                panic!("{kind:?}: the frame comes first");
            };
            let f = b.frame_rect();
            assert_eq!(*rect, [f[0] as f32, f[1] as f32, f[2] as f32, f[3] as f32], "{kind:?}");
            let inside_frame = |r: [i32; 4]| {
                r[0] >= f[0] && r[1] >= f[1] && r[0] + r[2] <= f[0] + f[2] && r[1] + r[3] <= f[1] + f[3]
            };
            for i in 0..b.rows() {
                assert!(inside_frame(b.field_rect(i)), "{kind:?} field {i}");
            }
            let lines = 1 + i32::from(kind.warning().is_some());
            assert!(inside_frame([b.at[0], b.lines_y(), WINDOW_W, lines * (FONT_SIZE + 4)]), "{kind:?} lines");
            assert!(b.lines_y() >= b.field_rect(b.rows() - 1)[1] + FIELD_SIZE[1], "{kind:?} lines under fields");
            for i in 0..kind.buttons().len() {
                let p = b.button_place(i);
                assert!(inside_frame([p[0], p[1], BUTTON_SIZE[0], BUTTON_SIZE[1]]), "{kind:?} button {i}");
            }
            margins.insert(f[1] + f[3] - b.buttons_y());
        }
        assert_eq!(margins.len(), 1, "the buttons keep one margin from the bottom edge: {margins:?}");
    }
}
