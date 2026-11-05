//! Login screen: ID and password boxes, the login button, "remember ID", the quit button; Tab walks the
//! controls, Enter logs in, Escape asks to quit.
//!
//! Login sends the auth server's file check and login request (the ID as the `LOCAL:<id>` token, the password box
//! text in the typed-text field); the server list that follows opens the server select screen. A refused login
//! shows the "wrong password" message image (reason low word 0x25) or the 2009 boxes.
// SPEC: PROTOCOL_2009.md §2.1, §2.2 (0x201 reasons: low word 0x39 -> res 0x560, 0x25 -> res 0x443, other -> 0x44f)
// DISASM: cshell+0x11ce40 (login screen build: tiles, "login_page", tab frames of controls 9, 10, 3, 11, 4, 5, 7, 6,
// 8, the first two 0x103 x 0x1c at (-13, -5) / (-13, -10); tab order 9, 10, 3, 11, 4, 5, 6, 7, 8)
// DISASM: cshell+0x11c9f0 (keys: Tab / Shift+Tab walk the order; Enter on the first three stops -> command 0xbc, on
// the fourth nothing, else the stop's command; Escape -> command 0xa; Space on the fourth stop -> its command)
// EVIDENCE: cshell+0x11bd10 (message images 0, 1, 2 of window 0: one shown), cshell+0x11da90 (focus: message 0),
// cshell+0x11d4e0 (0xbc: wrong ID -> message 1, wrong password -> message 2; 0xa: quit question 0xbd3)
// DC-DEFAULT: CLI-1691 (proposed) an empty ID shows message 1 without asking the server; the password box draws one
// `*` per character

use super::catalog::Catalog;
use super::link::{LinkCmd, LinkEvent, Wait};
use super::popup::{text_ids, BoxTag, Popup};
use super::sheet::{Sheet, Tiles};
use super::modbrowser::ConsentBox;
use super::signup::{self, Kind, RecoverReply, RegisterReply, SignupArt, SignupBox, Step};
use crate::mods::ModState;
use super::{build_sheet, frontfx, pages, Ctx, Go, MenuData, MenuInput, MenuKey, Screen};
use crate::hud::items::DrawItem;
use dc_ui_data::WidgetKind;

/// Control ids of the login window (window 0).
pub const MESSAGE_IMAGES: [i32; 3] = [0, 1, 2];
pub const LOGIN_BUTTON: i32 = 3;
pub const ID_BOX: i32 = 9;
pub const PASSWORD_BOX: i32 = 10;
pub const REMEMBER_CHECK: i32 = 11;
/// Command numbers of the login window's buttons.
pub const CMD_LOGIN: i32 = 188;
pub const CMD_QUIT: i32 = 10;
pub const CMD_WEB_PAGES: [i32; 4] = [10000, 10001, 10002, 10003];
pub const CMD_REMEMBER: i32 = 10004;
/// The register button: the third of the four web-page commands (find ID, find password, register, homepage;
/// DC_DEFAULTS CLI-166).
// DISASM: cshell+0x11d4e0 (0x2712 opens the passport `Registration/Signup` page, pointer VA 0x37548c00)
pub const CMD_REGISTER: i32 = 10002;
/// login-recovery (CLI-1501): the find-ID and find-password buttons, the first two web-page commands.
// DISASM: cshell+0x11d4e0 (0x2710 / 0x2711 open the passport find-ID / find-password pages, VA 0x37548bf8 /
// 0x37548bfc)
pub const CMD_FIND_ID: i32 = 10000;
pub const CMD_FIND_PASSWORD: i32 = 10001;
/// CLI-1616: the homepage button, the fourth web-page command, opens the community server browser.
// DISASM: cshell+0x11d4e0 (0x2713 opens the game's homepage, VA 0x37548c04)
pub const CMD_HOMEPAGE: i32 = 10003;
/// The lobby's web port is the auth port + 8080 (dc-lobby-server `--clanweb` default, HOSTING.md).
pub const WEB_PORT_OFFSET: u16 = 8080;
/// Keyboard tab order by control id.
pub const TAB_ORDER: [i32; 9] = [9, 10, 3, 11, 4, 5, 6, 7, 8];
/// Message image shown: 0 enter ID, 1 wrong ID, 2 wrong password.
pub const MSG_ENTER_ID: usize = 0;
pub const MSG_WRONG_ID: usize = 1;
pub const MSG_WRONG_PASSWORD: usize = 2;
/// Tab frame of the two boxes: size and offsets (the boxes answer clicks inside it).
// DISASM: cshell+0x11ceb8..0x11cec2 (0x1c, 0x103 pushed with the frame texture; offsets -13 / -5 and -13 / -10)
pub const BOX_FRAME: [i32; 2] = [0x103, 0x1c];
pub const ID_FRAME_AT: [i32; 2] = [-13, -5];
pub const PASSWORD_FRAME_AT: [i32; 2] = [-13, -10];
/// 0x201 reason low words with their own reaction.
pub const REASON_LOW_BAD_PASSWORD: u32 = 0x25;
pub const REASON_LOW_BANNED: u32 = 0x39;

/// The registration page of the lobby at `auth`: `http://<auth host>:<auth port + 8080>/register` (CLI-1290).
pub fn registration_page(auth: std::net::SocketAddr) -> Option<String> {
    let port = auth.port().checked_add(WEB_PORT_OFFSET)?;
    // CLI-1475: https:// except on loopback (a public lobby's web port refuses plain forms, LG-224)
    let ip = match auth.ip() {
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map(std::net::IpAddr::V4).unwrap_or(auth.ip()),
        v4 => v4,
    };
    let scheme = if ip.is_loopback() { "http" } else { "https" };
    Some(format!(
        "{scheme}://{}/register",
        std::net::SocketAddr::new(auth.ip(), port)
    ))
}

/// The web port of the account pages (registration, recovery): the identity authority's when the identity is on
/// (CLI-1610: the accounts live there), else the lobby's (auth port + 8080).
pub fn account_web(data: &MenuData) -> Option<std::net::SocketAddr> {
    data.identity_web
        .or_else(|| super::clanpage::web_addr(data.auth))
}

pub struct Login {
    sheet: Sheet,
    tab: usize,
    busy: Wait,
    /// The ID when the layout has no ID box (no ROM): the `--name` / `--user` ID.
    id_without_box: String,
    /// client-register (CLI-1450): the account creation dialog, its art, and the registration question in flight.
    pub signup: Option<SignupBox>,
    signup_art: SignupArt,
    asking: bool,
    /// login-recovery: the dialog the recovery question (`GET /recover`) in flight opens.
    recover_ask: Option<Kind>,
    /// CLI-1618: the consent prompt for the unverified mods of the server being joined.
    pub consent: Option<ConsentBox>,
    /// CLI-1866: a friend pack's "Check update" button (none for other builds).
    update_button: super::modbrowser::Buttons,
}

impl Login {
    pub fn new(cat: &Catalog, _data: &MenuData) -> Login {
        let mut sheet = build_sheet(cat, &[pages::LOGIN]);
        if let Some(prefix) = &cat.paths.login_back {
            sheet.tiles = Tiles::new([0, 0], 3, 4, prefix);
        }
        // the caps-lock notice (the page's control on the screen) shows only while caps lock is on
        sheet.show(-1, 0, WidgetKind::Image, false);
        for (id, at) in [(ID_BOX, ID_FRAME_AT), (PASSWORD_BOX, PASSWORD_FRAME_AT)] {
            if let Some(p) = sheet.part_mut(0, id, WidgetKind::EditBox) {
                p.hit = Some([
                    p.pos[0] + at[0],
                    p.pos[1] + at[1],
                    BOX_FRAME[0],
                    BOX_FRAME[1],
                ]);
            }
        }
        if let Some(p) = sheet.part_mut(0, PASSWORD_BOX, WidgetKind::EditBox) {
            p.masked = true;
        }
        Login {
            sheet,
            tab: 0,
            busy: Wait::default(),
            id_without_box: String::new(),
            signup: None,
            signup_art: SignupArt::from_rom(cat.rom.as_deref()),
            asking: false,
            recover_ask: None,
            consent: None,
            update_button: super::updatebox::check_button(),
        }
    }

    /// CLI-1618: the player's answer to the consent prompt. Accept remembers the mods (id, version, sha256) in the
    /// profile and lets the link go on with the download and the login; Cancel drops the login (nothing is
    /// downloaded) and returns to the server browser.
    // DC-DEFAULT: CLI-1618 (proposed, project decision)
    fn consent_answer(&mut self, yes: bool, ctx: &mut Ctx) {
        let Some(b) = self.consent.take() else {
            return;
        };
        if yes {
            for k in b.keys() {
                if !ctx.data.settings.mods_accepted.contains(&k) {
                    ctx.data.settings.mods_accepted.push(k);
                }
            }
            super::store_settings(ctx.data, ctx.cat);
            ctx.send(super::modbrowser::prefs_cmd(ctx.data));
            ctx.send(LinkCmd::ModConsent(true));
            ctx.wait_box(text_ids::CONNECTING);
        } else {
            ctx.send(LinkCmd::ModConsent(false));
            self.busy.stop();
            self.set_tab(self.tab);
            // CLI-1622: from the server list: back to the lobby the player came from (its login again)
            if ctx.data.modded_join.is_some() {
                super::moddedrows::go_home(ctx);
                if *ctx.go == Some(Go::Login) {
                    *ctx.go = None;
                    if std::mem::take(&mut ctx.data.auto_login) {
                        self.login(ctx);
                    }
                }
            } else {
                *ctx.go = Some(Go::Community);
            }
        }
    }

    /// client-register: the register button asks the lobby whether it registers; the answer opens the dialog.
    // DC-DEFAULT: CLI-1450 (proposed) the register button opens the account creation dialog (the 2009 client opened
    // the publisher's sign-up page in the browser); registration off shows a box instead
    fn ask_register(&mut self, ctx: &mut Ctx) {
        if self.asking || self.recover_ask.is_some() || self.signup.is_some() || self.busy.busy() {
            return;
        }
        match account_web(ctx.data) {
            Some(addr) => {
                self.asking = true;
                ctx.send(LinkCmd::RegisterInfo { addr });
                ctx.wait_box(text_ids::CONNECTING);
            }
            None => ctx
                .popups
                .push(Popup::ok(signup::T_UNREACHABLE.into(), BoxTag::None)),
        }
    }

    /// login-recovery: find ID / find password ask the lobby whether it can mail; yes opens the dialog `kind`.
    // DC-DEFAULT: CLI-1501 (proposed) the find-ID / find-password buttons open in-client dialogs (the 2009 client
    // opened the publisher's pages in the browser); a lobby that cannot mail shows a box instead
    fn ask_recover(&mut self, kind: Kind, ctx: &mut Ctx) {
        if self.asking || self.recover_ask.is_some() || self.signup.is_some() || self.busy.busy() {
            return;
        }
        match account_web(ctx.data) {
            Some(addr) => {
                self.recover_ask = Some(kind);
                ctx.send(LinkCmd::RecoverInfo { addr });
                ctx.wait_box(text_ids::CONNECTING);
            }
            None => ctx
                .popups
                .push(Popup::ok(signup::T_UNREACHABLE.into(), BoxTag::None)),
        }
    }

    /// The open dialog, if the user can see it. CLI-1450 safety rule: a dialog that is not on the screen does not
    /// hold the input; it is closed and the login screen gets its input back.
    fn shown_signup(&mut self) -> Option<&mut SignupBox> {
        if self.signup.as_ref().is_some_and(|b| !b.visible()) {
            self.signup = None;
            self.set_tab(self.tab);
        }
        self.signup.as_mut()
    }

    /// What the dialog's input asked for.
    fn signup_step(&mut self, step: Step, ctx: &mut Ctx) {
        match step {
            Step::None | Step::Back => {}
            Step::Cancel => {
                self.signup = None;
                self.set_tab(self.tab);
            }
            Step::Create(form) => self.post(move |addr| LinkCmd::Register { addr, form }, ctx),
            Step::Recover(req) => self.post(move |addr| LinkCmd::Recover { addr, req }, ctx),
        }
    }

    /// Sends the dialog's post to the web port behind the connecting box (no address: the unreachable box).
    fn post(&mut self, cmd: impl FnOnce(std::net::SocketAddr) -> LinkCmd, ctx: &mut Ctx) {
        match account_web(ctx.data) {
            Some(addr) => {
                ctx.send(cmd(addr));
                ctx.wait_box(text_ids::CONNECTING);
            }
            None => {
                if let Some(b) = self.signup.as_mut() {
                    b.waiting = false;
                }
                ctx.popups
                    .push(Popup::ok(signup::T_UNREACHABLE.into(), BoxTag::None));
            }
        }
    }

    /// Puts `login` into the ID box, clears the password box and gives it the keyboard.
    fn fill_login(&mut self, login: &str, ctx: &mut Ctx) {
        ctx.data.user = login.to_string();
        self.id_without_box = login.to_string();
        if let Some(p) = self.sheet.part_mut(0, ID_BOX, WidgetKind::EditBox) {
            p.text = login.to_string();
        }
        if let Some(p) = self.sheet.part_mut(0, PASSWORD_BOX, WidgetKind::EditBox) {
            p.text.clear();
        }
        self.message(None);
        self.set_tab(1);
    }

    /// login-recovery: the answer to a find-ID / find-password request (CLI-1503). Find ID: ok closes the dialog;
    /// the code request: ok opens the code step; the change: ok closes the dialog and fills the ID box; every
    /// answer shows the lobby's message in a box (our own text after a change, and for an empty message); a
    /// refusal keeps the dialog and its fields.
    fn recovered(&mut self, reply: &RecoverReply, ctx: &mut Ctx) {
        let Some(b) = self.signup.as_mut() else {
            return;
        };
        b.waiting = false;
        ctx.close_wait();
        let or_sent = |m: &str| {
            if m.is_empty() {
                signup::T_MAIL_SENT.to_string()
            } else {
                m.to_string()
            }
        };
        let text = match (b.kind, reply) {
            (_, RecoverReply::Unreachable) => signup::T_UNREACHABLE.to_string(),
            (_, RecoverReply::Refused(m)) => m.clone(),
            (Kind::PasswordCode, RecoverReply::Done(m)) => {
                b.code_sent();
                or_sent(m)
            }
            (Kind::PasswordChange, RecoverReply::Done(_)) => {
                let login = b.carry[0].clone();
                self.signup = None;
                self.fill_login(&login, ctx);
                signup::changed_message(&login)
            }
            (_, RecoverReply::Done(m)) => {
                self.signup = None;
                self.set_tab(self.tab);
                or_sent(m)
            }
        };
        ctx.popups.push(Popup::ok(text, BoxTag::None));
    }

    /// The answer to the post: success closes the dialog and fills the ID box; a refusal shows the lobby's reason.
    fn registered(&mut self, reply: &RegisterReply, ctx: &mut Ctx) {
        let Some(b) = self.signup.as_mut() else {
            return;
        };
        b.waiting = false;
        ctx.close_wait();
        let text = match reply {
            RegisterReply::Created(login) => {
                self.signup = None;
                self.fill_login(login, ctx);
                signup::ready_message(login)
            }
            RegisterReply::Refused(m) => m.clone(),
            RegisterReply::Unreachable => signup::T_UNREACHABLE.to_string(),
        };
        ctx.popups.push(Popup::ok(text, BoxTag::None));
    }

    fn message(&mut self, n: Option<usize>) {
        for (i, id) in MESSAGE_IMAGES.iter().enumerate() {
            self.sheet.show(0, *id, WidgetKind::Image, Some(i) == n);
        }
    }

    fn focus_box(&mut self, id: Option<i32>) {
        let i = id.and_then(|id| self.sheet.find(0, id, WidgetKind::EditBox));
        self.sheet.focus(i);
    }

    fn box_text(&self, id: i32) -> String {
        match self.sheet.part(0, id, WidgetKind::EditBox) {
            Some(p) => p.text.clone(),
            None if id == ID_BOX => self.id_without_box.clone(),
            None => String::new(),
        }
    }

    fn set_tab(&mut self, n: usize) {
        self.tab = n % TAB_ORDER.len();
        let focus = match self.tab {
            0 => Some(ID_BOX),
            1 => Some(PASSWORD_BOX),
            _ => None,
        };
        self.focus_box(focus);
    }

    fn login(&mut self, ctx: &mut Ctx) {
        if self.busy.busy() {
            return;
        }
        let user = self.box_text(ID_BOX).trim().to_string();
        self.message(None);
        if user.is_empty() {
            self.message(Some(MSG_WRONG_ID));
            self.set_tab(0);
            return;
        }
        let password = self.box_text(PASSWORD_BOX);
        let Some(auth) = ctx.data.auth else {
            ctx.error(text_ids::LOGIN_FAILED, BoxTag::None);
            return;
        };
        ctx.data.user = user.clone();
        ctx.send(LinkCmd::Login {
            auth,
            nick: user.clone(),
            user,
            password,
        });
        self.busy.start();
        self.focus_box(None);
        ctx.wait_box(text_ids::CONNECTING);
    }
}

impl Screen for Login {
    fn name(&self) -> &'static str {
        "login"
    }
    fn layout_rects(&self) -> Option<([i32; 4], Vec<[i32; 4]>)> {
        self.consent.as_ref().map(ConsentBox::layout_rects)
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    fn enter(&mut self, ctx: &mut Ctx) {
        self.message(Some(MSG_ENTER_ID));
        // CLI-1866: a friend pack looks into its update folder once per run
        super::updatebox::check_at_start();
        let user = ctx.data.user.clone();
        self.id_without_box = user.clone();
        if let Some(p) = self.sheet.part_mut(0, ID_BOX, WidgetKind::EditBox) {
            p.text = user.clone();
        }
        if let Some(p) = self.sheet.part_mut(0, PASSWORD_BOX, WidgetKind::EditBox) {
            // CLI-810: --login-password fills the box (empty by default)
            p.text = ctx.data.login_password.0.clone();
        }
        // CLI-1610: an unexpired cached identity is offered: its name in the ID box, no password asked (the keyboard
        // goes to the login button; a typed password asks for a new token)
        let cached = ctx
            .data
            .identity_login
            .clone()
            .filter(|l| user.is_empty() || l.eq_ignore_ascii_case(&user));
        if let Some(l) = &cached {
            self.id_without_box = l.clone();
            if let Some(p) = self.sheet.part_mut(0, ID_BOX, WidgetKind::EditBox) {
                p.text = l.clone();
            }
        }
        let offered = cached.is_some() && ctx.data.login_password.0.is_empty();
        let user = cached.unwrap_or(user);
        // a remembered ID puts the keyboard in the password box
        self.set_tab(if user.is_empty() {
            0
        } else if offered {
            2
        } else {
            1
        });
        if let Some(p) = self
            .sheet
            .part_mut(0, REMEMBER_CHECK, WidgetKind::CheckButton)
        {
            p.checked = !user.is_empty();
        }
        // CLI-1622: a lobby of the server list joined (or left) with the identity on: the login goes at once with the
        // cached token (an empty password box; the link asks the authority for nothing)
        if std::mem::take(&mut ctx.data.auto_login) && !ctx.data.user.trim().is_empty() {
            if let Some(p) = self.sheet.part_mut(0, PASSWORD_BOX, WidgetKind::EditBox) {
                p.text.clear();
            }
            if let Some(p) = self.sheet.part_mut(0, ID_BOX, WidgetKind::EditBox) {
                p.text = ctx.data.user.clone();
            }
            self.login(ctx);
        }
    }

    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        if self.consent.is_some() {
            match k {
                MenuKey::Enter => self.consent_answer(true, ctx),
                MenuKey::Escape => self.consent_answer(false, ctx),
                _ => {}
            }
            return;
        }
        if let Some(b) = self.shown_signup() {
            let step = b.key(k);
            // Enter on a button and Escape press it: the select sound
            if step != Step::None {
                ctx.sound(frontfx::SOUND_SELECT);
            }
            self.signup_step(step, ctx);
            return;
        }
        match k {
            MenuKey::Tab => self.set_tab(self.tab + 1),
            MenuKey::BackTab => self.set_tab(self.tab + TAB_ORDER.len() - 1),
            MenuKey::Enter => {
                if self.tab <= 2 {
                    self.login(ctx);
                } else if TAB_ORDER[self.tab] != REMEMBER_CHECK {
                    let cmd = self
                        .sheet
                        .part(0, TAB_ORDER[self.tab], WidgetKind::Button)
                        .map_or(0, |p| p.command);
                    self.click(cmd, usize::MAX, ctx);
                }
            }
            MenuKey::Space if TAB_ORDER[self.tab] == REMEMBER_CHECK => {
                if let Some(p) = self
                    .sheet
                    .part_mut(0, REMEMBER_CHECK, WidgetKind::CheckButton)
                {
                    p.checked = !p.checked;
                }
            }
            MenuKey::Escape => self.click(CMD_QUIT, usize::MAX, ctx),
            _ => {}
        }
    }

    fn click(&mut self, command: i32, _part: usize, ctx: &mut Ctx) {
        if self.consent.is_some() {
            match command {
                super::modbrowser::CMD_AGREE => self.consent_answer(true, ctx),
                super::modbrowser::CMD_DECLINE => self.consent_answer(false, ctx),
                _ => {}
            }
            return;
        }
        match command {
            CMD_LOGIN => {
                self.tab = 2;
                self.login(ctx);
            }
            CMD_QUIT => {
                let t = ctx.cat.text(text_ids::ASK_QUIT);
                ctx.popups.push(Popup::ask(t, BoxTag::Quit));
            }
            // CLI-1450 (amends CLI-1290): register opens the in-client dialog, not the browser
            CMD_REGISTER => self.ask_register(ctx),
            // CLI-1501 (amends CLI-166): find ID / find password open the in-client dialogs
            CMD_FIND_ID => self.ask_recover(Kind::FindId, ctx),
            CMD_FIND_PASSWORD => self.ask_recover(Kind::PasswordCode, ctx),
            // CLI-1616 (amends CLI-166): the homepage button opens the community server browser
            CMD_HOMEPAGE if !self.busy.busy() => *ctx.go = Some(Go::Community),
            c if CMD_WEB_PAGES.contains(&c) => {}
            _ => {}
        }
    }

    // client-signup-buttons: the dialog's buttons behave as the message boxes' (CLI-1450): hover and down looks,
    // the click on release, interface sounds EVIDENCE: cshell+0x1aeb90 (release on the pressed control = select),
    // cshell+0x1af490 (pointer onto a control = select change)
    fn modal_input(&mut self, ev: &MenuInput, ctx: &mut Ctx) -> bool {
        if let Some(b) = self.consent.as_mut() {
            if let Some(yes) = b.input(ev) {
                ctx.sound(frontfx::SOUND_SELECT);
                self.consent_answer(yes, ctx);
            }
            return matches!(
                ev,
                MenuInput::Move(_) | MenuInput::Press(_) | MenuInput::Release(_)
            );
        }
        let Some(b) = self.shown_signup() else {
            return false;
        };
        match *ev {
            MenuInput::Move(p) => {
                if b.moved(p) {
                    ctx.sound(frontfx::SOUND_SELECT_CHANGE);
                }
            }
            MenuInput::Press(p) => b.press(p),
            MenuInput::Release(p) => {
                let (step, clicked) = b.release(p);
                if clicked {
                    ctx.sound(frontfx::SOUND_SELECT);
                }
                self.signup_step(step, ctx);
            }
            _ => return false,
        }
        true
    }

    fn type_char(&mut self, c: char, _ctx: &mut Ctx) -> bool {
        match self.shown_signup() {
            Some(b) => {
                b.type_char(c);
                true
            }
            None => false,
        }
    }

    fn backspace(&mut self, _ctx: &mut Ctx) -> bool {
        match self.shown_signup() {
            Some(b) => {
                b.backspace();
                true
            }
            None => false,
        }
    }

    // CLI-1866: the friend pack's "Check update" button (a click on press, as the server screen's Test button)
    fn press(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        if self.consent.is_some() || self.signup.is_some() || !self.update_button.press(p) {
            return false;
        }
        self.update_button.release(p);
        ctx.sound(frontfx::SOUND_SELECT);
        crate::selfupdate::start_check(true);
        true
    }

    fn hover(&mut self, p: [f32; 2], _data: &MenuData) {
        self.update_button.moved(p);
    }

    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, secs: f32, out: &mut Vec<DrawItem>) {
        self.update_button.draw(cat, out);
        // CLI-1616: the server the browser picked (nothing while the configured lobby is used)
        if let Some(name) = &data.picked_server {
            out.push(super::sheet::place_text(
                &format!("Server: {name}"),
                [16, 740],
                [0, 0],
                dc_ui_data::pages::Align {
                    first: super::sheet::ALIGN_LEFT,
                    second: 0,
                },
                super::usebox::FONT,
                super::usebox::FONT_SIZE,
                super::usebox::TEXT_COLOUR,
            ));
        }
        if let Some(b) = &self.signup {
            b.draw(cat, &self.signup_art, secs, out);
        }
        if let Some(b) = &self.consent {
            b.draw(cat, out);
        }
    }

    fn focused(&mut self, part: usize, _ctx: &mut Ctx) {
        let id = self.sheet.parts[part].id;
        if let Some(t) = TAB_ORDER.iter().position(|x| *x == id) {
            self.tab = t;
        }
    }

    fn event(&mut self, e: &LinkEvent, ctx: &mut Ctx) {
        match e {
            // CLI-1618: the set names mods the player has not trusted: the prompt before any download
            LinkEvent::Mods(ModState::NeedsConsent(set)) => {
                let ask = super::modbrowser::untrusted(set, ctx.data);
                if ask.is_empty() {
                    ctx.send(LinkCmd::ModConsent(true));
                } else {
                    ctx.close_wait();
                    self.focus_box(None);
                    self.consent = Some(ConsentBox::new(ctx.cat, set.clone(), ask));
                }
            }
            LinkEvent::RegisterInfo(info) if self.asking => {
                self.asking = false;
                ctx.close_wait();
                match info {
                    None => ctx
                        .popups
                        .push(Popup::ok(signup::T_UNREACHABLE.into(), BoxTag::None)),
                    Some(i) if !i.open => ctx
                        .popups
                        .push(Popup::ok(signup::T_OFF.into(), BoxTag::None)),
                    Some(i) => {
                        // register-dialog-fix: centred in virtual pixels (the window's pixel size put it off
                        // screen at 2560 x 1440 while it held the input)
                        // CLI-1500: the email row when the lobby asks for an address
                        self.signup = Some(SignupBox::centred_as(Kind::Register {
                            invite: i.invite,
                            email: i.email,
                        }));
                        self.focus_box(None);
                    }
                }
            }
            LinkEvent::Registered(reply) => self.registered(reply, ctx),
            LinkEvent::RecoverInfo(mail) if self.recover_ask.is_some() => {
                let kind = self.recover_ask.take().unwrap_or(Kind::FindId);
                ctx.close_wait();
                match mail {
                    None => ctx
                        .popups
                        .push(Popup::ok(signup::T_UNREACHABLE.into(), BoxTag::None)),
                    Some(false) => ctx
                        .popups
                        .push(Popup::ok(signup::T_RECOVER_OFF.into(), BoxTag::None)),
                    Some(true) => {
                        self.signup = Some(SignupBox::centred_as(kind));
                        self.focus_box(None);
                    }
                }
            }
            LinkEvent::Recovered(reply) => self.recovered(reply, ctx),
            // nickname-screen (CLI-1480): no character yet: the nickname screen opens
            // SPEC: PROTOCOL_2009.md §2.1 (reason 0x0003001e -> nickname screen)
            LinkEvent::NoCharacter => {
                self.busy.stop();
                ctx.close_wait();
                *ctx.go = Some(Go::Nickname);
            }
            LinkEvent::Servers(_) => {
                self.busy.stop();
                ctx.close_wait();
                // CLI-1470: the trust file could not be saved: the player is told once
                if let Some(w) = dc_botclient::tls::take_store_warning() {
                    ctx.popups.push(Popup::error(w, BoxTag::None));
                }
                // CLI-1651 / CLI-1863: a newer client exists: its box ([`super::updatebox`]) follows once
                // the 2009 client shows its main screen for about 3 s between the login and the server list
                // DC-DEFAULT: CLI-176 (proposed) no main screen: the server list opens at once
                *ctx.go = Some(Go::Servers);
            }
            // CLI-1471: a trust refusal (changed / mismatched certificate, no TLS) gets a box of its own
            LinkEvent::LoginRefused { text, .. } if dc_botclient::tls::is_trust_message(text) => {
                self.busy.stop();
                ctx.close_wait();
                super::moddedrows::restore_home(ctx);
                eprintln!("dc-client: {text}");
                let first = text.lines().next().unwrap_or_default().to_string();
                ctx.popups.push(Popup::error(first, BoxTag::None));
            }
            // CLI-1651: too old for this lobby: our own text, not "login failed"; CLI-1863: the update box with it
            // ([`super::updatebox`]) when the offer waits, else the plain notice
            LinkEvent::LoginRefused { text, .. } if crate::clientver::is_refusal(text) => {
                self.busy.stop();
                ctx.close_wait();
                super::moddedrows::restore_home(ctx);
                if !crate::selfupdate::has_offer() {
                    ctx.popups.push(Popup::error(text.clone(), BoxTag::None));
                }
            }
            LinkEvent::LoginRefused { reason, .. } => {
                self.busy.stop();
                ctx.close_wait();
                // CLI-1622: a refused join of a lobby of the list: the next login goes to the lobby the player came
                // from (that lobby's session was left already)
                super::moddedrows::restore_home(ctx);
                match reason & 0xffff {
                    REASON_LOW_BAD_PASSWORD if *reason != 0 => {
                        // CLI-1610: a refused cached token is gone (the link dropped it): the password is asked
                        ctx.data.identity_login = None;
                        self.message(Some(MSG_WRONG_PASSWORD));
                        if let Some(p) = self.sheet.part_mut(0, PASSWORD_BOX, WidgetKind::EditBox) {
                            p.text.clear();
                        }
                        self.set_tab(1);
                    }
                    REASON_LOW_BANNED => ctx.error(text_ids::LOGIN_BANNED, BoxTag::None),
                    _ => ctx.error(text_ids::LOGIN_FAILED, BoxTag::None),
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod login_password_tests {
    use super::*;
    use crate::menu::link::LinkOptions;
    use crate::menu::{Menu, MenuOptions};

    #[test]
    fn registration_page_is_the_auth_host_at_port_plus_8080() {
        assert_eq!(
            registration_page("203.0.113.5:10000".parse().unwrap()).as_deref(),
            Some("https://203.0.113.5:18080/register")
        );
        assert_eq!(
            registration_page("[::1]:10000".parse().unwrap()).as_deref(),
            Some("http://[::1]:18080/register")
        );
        assert_eq!(registration_page("127.0.0.1:65500".parse().unwrap()), None);
        // an IPv4-mapped loopback address is loopback too
        assert!(registration_page("[::ffff:127.0.0.1]:10000".parse().unwrap())
            .unwrap()
            .starts_with("http://"));
        assert!(CMD_WEB_PAGES.contains(&CMD_REGISTER));
    }

    // CLI-810: --login-password fills the password box of a by-hand run; its Debug form hides it (CLI-178)
    #[test]
    fn login_password_fills_the_box_and_stays_hidden() {
        // the login sheet's boxes come from the ROM layout
        let Some(rom) = std::env::var_os("CA_ROM")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
        else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let mut menu = Menu::new(MenuOptions {
            auth: "127.0.0.1:1".parse().unwrap(),
            user: "Robin".into(),
            link: LinkOptions::new("login-password"),
            rom: Some(rom),
            font: Some((crate::testkit::synth_font(), 0)),
            width: 1024,
            height: 768,
        });
        let text = |m: &Menu| {
            m.sheet()
                .part(0, PASSWORD_BOX, WidgetKind::EditBox)
                .map(|p| p.text.clone())
        };
        assert_eq!(text(&menu), Some(String::new()));
        menu.set_login_password("test");
        assert_eq!(menu.screen_name(), "login");
        assert_eq!(text(&menu), Some("test".to_string()));
        assert_eq!(
            menu.sheet()
                .part(0, ID_BOX, WidgetKind::EditBox)
                .map(|p| p.text.clone()),
            Some("Robin".to_string())
        );
        let dbg = format!("{:?}", menu.data.login_password);
        assert!(!dbg.contains("test"), "{dbg}");
    }
}
