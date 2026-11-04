//! In-match chat of the 2009 client: the server's chat line `GS_CHAT` (S2C 0x68) shown in the chat log with the
//! team / friendly / enemy / moderator colours, the system notice (sender 0xFFFFFFFF) as a red centre banner, and the
//! chat input that sends `UP_CHAT` (C2S 0x68) with its repeat guard.
//!
//! - [`read_gs_chat`]: the S2C reader; [`classify`]: what the 2009 handler shows for a line.
//! - [`ChatInput`]: the typed line, its scope and the repeat guard; [`up_chat_body`]: the C2S body.
//! - [`ChatLook`]: the colours, the input element's images and the banner style from the user's ROM (pinned key
//!   texts, read from `HUDLayout0` like the rest of the HUD), or our own look without a ROM.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68 (cshell+0x4b30e..0x4b4cb), §3.2.2 (tdm1 echo);
// GAME_MESSAGES_2009_C2S.md §6.1 (`UP_CHAT`, cshell+0x72df0)

use dc_ui_data::layout::{parse_value, LayoutTable, Value};
use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_wire::udp::{BitReader, BitWriter};
use std::path::Path;

/// Shell id of the server's chat line and of the client's chat message (the same number both ways).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68; GAME_MESSAGES_2009_C2S.md §6.1
pub const GS_CHAT: u8 = 0x68;
pub const UP_CHAT: u8 = 0x68;

/// `sender_id` of a system notice: no chat-log line, a red banner instead.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68
pub const SYSTEM_SENDER: u32 = 0xFFFF_FFFF;

/// Buffer sizes of the two strings in units (at most N - 1 units are kept).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68 (`zstr16(32)`, `zstr16(376)`), §1 (`zstr16(N)`)
pub const NAME_UNITS: usize = 32;
pub const TEXT_UNITS: usize = 376;

/// How long a chat line stays in the log and how long the red banner stays.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68 (10000 ms both)
pub const CHAT_LINE_MS: u32 = 10_000;
pub const BANNER_MS: u32 = 10_000;
/// The banner's colour (0xFFFF0000), its centre point, font record and size.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68 (red 0xFFFF0000)
// EVIDENCE: cshell+0xbd920 (font record 4, size 0x12, placed at the element's +0x64 / +0x68), cshell+0xbcfd0 (those
// two set to 0x200 / 0xe6)
pub const BANNER_COLOUR: [u8; 4] = [255, 0, 0, 255];
pub const BANNER_POS: [i32; 2] = [0x200, 0xe6];
pub const BANNER_FONT: i32 = 4;
pub const BANNER_SIZE: i32 = 0x12;

/// The chat input keeps at most this many characters.
// EVIDENCE: cshell+0x72130 (the edit control is created with 0x78 and its limit set to 0x78)
pub const INPUT_MAX_CHARS: usize = 0x78;

/// The repeat guard: counter after a new line, after the lock ran out, and the lock time.
// EVIDENCE: cshell+0x72df0 (counter +0x1dec: 9 after a new line, minus 1 per repeat, < 1 stores the time +0x1df0;
// below 1 every send within 60000 ms is refused with CRes string 0x1028 for 5000 ms, then the counter is 10)
pub const REPEAT_NEW_LINE: i32 = 9;
pub const REPEAT_AFTER_LOCK: i32 = 10;
pub const REPEAT_LOCK_MS: u32 = 60_000;
pub const REPEAT_NOTICE_ID: u32 = 0x1028;
pub const REPEAT_NOTICE_MS: u32 = 5000;

/// The leave line of a player the server kicked (0x6c `kicked` non-zero); the plain leave line is 0xbb9.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2 row 0x6c (CRes 0x185b when kicked, else 0xbb9, 5000 ms)
pub const KICKED_ID: u32 = 0x185b;

/// `GS_CHAT` (S2C 0x68) as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GsChat {
    pub name: String,
    pub text: String,
    pub sender: u32,
    pub team_only: bool,
    pub whisper: u32,
}

fn zstr16_cut(r: &mut BitReader<'_>, units: usize) -> String {
    let mut u = r.read_zstr16();
    u.truncate(units - 1);
    String::from_utf16_lossy(&u)
}

/// Reads a `GS_CHAT` body (after the shell id): `[sender_name:zstr16(32)][text:zstr16(376)][sender_id:u32]
/// [team_only:u8][whisper:u32]`. `None` when the body ends before the three numbers.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68 (reads 0x4b34e / 0x4b362 strings, 0x4b36b read(32), 0x4b376 read(8),
// 0x4b386 read(32))
pub fn read_gs_chat(body: &[u8], bits: usize) -> Option<GsChat> {
    let mut r = BitReader::with_bit_len(body, bits);
    let name = zstr16_cut(&mut r, NAME_UNITS);
    let text = zstr16_cut(&mut r, TEXT_UNITS);
    if r.remaining_bits() < 72 {
        return None;
    }
    Some(GsChat {
        name,
        text,
        sender: r.read_u32(),
        team_only: r.read_u8() != 0,
        whisper: r.read_u32(),
    })
}

/// The colour class of a chat line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatClass {
    Team,
    Friendly,
    Enemy,
    Moderator,
}

/// What the 2009 handler does with one `GS_CHAT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatShow {
    /// The system notice: the red centre banner.
    Banner(String),
    /// A chat-log line.
    Line {
        class: ChatClass,
        text: String,
        /// The own line (the server's echo): `sender_id` is the own client id.
        own: bool,
        /// Non-zero `whisper`: the whisper form of the line.
        whisper: bool,
    },
}

/// The handler's decision for `m`: `sender_team` is the roster record's team of `sender_id` (`None`: no record, the
/// 2009 client then shows nothing and leaves the message unconsumed), `own_team` the local player's team.
/// `moderator_tag` is the 2-unit name tag of the moderator colour (read from the user's CShell; `None` without it).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68 (team_only -> team colour; else own team == record team ->
// friendly, else enemy; a name starting with the tag -> moderator colour, checked last)
pub fn classify(
    m: &GsChat,
    own_pid: Option<u8>,
    own_team: Option<u8>,
    sender_team: Option<u8>,
    moderator_tag: Option<&str>,
) -> Option<ChatShow> {
    if m.sender == SYSTEM_SENDER {
        return Some(ChatShow::Banner(m.text.clone()));
    }
    let team = sender_team?;
    let mut class = if m.team_only {
        ChatClass::Team
    } else if own_team == Some(team) {
        ChatClass::Friendly
    } else {
        ChatClass::Enemy
    };
    if moderator_tag.is_some_and(|t| !t.is_empty() && m.name.starts_with(t)) {
        class = ChatClass::Moderator;
    }
    Some(ChatShow::Line {
        class,
        text: line_text(&m.name, &m.text),
        own: own_pid.is_some_and(|p| u32::from(p) == m.sender),
        whisper: m.whisper != 0,
    })
}

/// The tag text without a ROM.
// DC-DEFAULT: CLI-1123 (proposed) no-ROM name of the team chat
pub const TEAM_CHAT_FALLBACK: &str = "Team Chat";

/// A team line (`team_only`) carries the team chat's name in brackets in front: `[Team Chat] name: line`; other lines
/// and an empty name stay as they are.
// DC-DEFAULT: CLI-1123 (proposed) the team line's tag (the 2009 client draws the line as the server sent it, cshell+
// 0x4b30e / 0x75b70, and no team chat capture exists; the name is the ROM's CRes 0x543)
pub fn team_tagged(name: &str, team_only: bool, text: String) -> String {
    let name = name.trim();
    if !team_only || name.is_empty() {
        return text;
    }
    format!("[{name}] {text}")
}

/// The text of a chat-log line: the sender's name, a colon and the line, unless the server already put the name and
/// its colon in front (the reference stack sends `name : line`; a line that merely starts with the name's letters still
/// gets the name).
// DC-DEFAULT: CLI-491 (proposed) the line names its sender (dc-dserver relays the bare line; whether the 2009 client
// prints the name itself is not visible on the wire, S2C §3.2.2); the whisper line uses the same form
pub fn line_text(name: &str, text: &str) -> String {
    if name.is_empty()
        || text
            .strip_prefix(name)
            .is_some_and(|r| r.starts_with(" : ") || r.starts_with(':'))
    {
        return text.to_string();
    }
    format!("{name}: {text}")
}

/// The scope of the chat input (`hud_chat_input`+0x1e10): all players or the own team.
// SPEC: GAME_MESSAGES_2009_C2S.md §6.1 (mode 0 -> scope byte 0, mode 1 -> scope byte 1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatScope {
    All,
    Team,
}

impl ChatScope {
    pub fn scope(self) -> u8 {
        match self {
            ChatScope::All => 0,
            ChatScope::Team => 1,
        }
    }
}

/// The `UP_CHAT` body: `[text:zstr16][tag:zstr16 empty][scope:u8]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §6.1 (cshell+0x730d2..0x7312b; the tag is written from a null string)
pub fn up_chat_body(text: &str, mode: ChatScope) -> BitWriter {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut w = BitWriter::new();
    w.write_zstr16_presumed(&units)
        .write_zstr16_presumed(&[])
        .write_u8(mode.scope());
    w
}

/// What the chat input did with the line on Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Submit {
    /// Send this line with this scope.
    Send(String, ChatScope),
    /// The repeat guard refused it: show the refusal notice.
    Refused,
    /// Nothing typed: the box just closes.
    Empty,
}

/// The chat input element: open with a scope, the typed line, the repeat guard. Sans-IO: every call carries its time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatInput {
    open: Option<ChatScope>,
    text: String,
    /// When the box opened (the caret blinks from there).
    pub opened_at: u32,
    prev_line: String,
    repeat: i32,
    locked_at: u32,
}

impl Default for ChatInput {
    fn default() -> Self {
        ChatInput {
            open: None,
            text: String::new(),
            opened_at: 0,
            prev_line: String::new(),
            repeat: REPEAT_AFTER_LOCK,
            locked_at: 0,
        }
    }
}

impl ChatInput {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn mode(&self) -> Option<ChatScope> {
        self.open
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Opens the box with `mode` (an open box keeps its line and takes the new scope).
    pub fn open(&mut self, mode: ChatScope, now: u32) {
        if self.open.is_none() {
            self.text.clear();
            self.opened_at = now;
        }
        self.open = Some(mode);
    }

    /// Closes the box and drops the line.
    pub fn cancel(&mut self) {
        self.open = None;
        self.text.clear();
    }

    /// Types `s` (control characters are dropped; the line stops at [`INPUT_MAX_CHARS`]).
    pub fn type_text(&mut self, s: &str) {
        if self.open.is_none() {
            return;
        }
        for c in s.chars().filter(|c| !c.is_control()) {
            if self.text.chars().count() >= INPUT_MAX_CHARS {
                break;
            }
            self.text.push(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.open.is_some() {
            self.text.pop();
        }
    }

    /// Enter: the line goes out unless the repeat guard holds it; the box closes either way.
    // EVIDENCE: cshell+0x72df0 (guard first, before the line is looked at; after a send the same line lowers the counter, a new line sets it to 9;
    // the box closes on both paths)
    // DC-DEFAULT: CLI-492 (proposed) an empty or blank line is not sent; the client's local commands and its word
    // filter (cshell+0x42920 / 0x40ca0 / 0x3c5d0, object VA 0x375bbdf4) are not modelled
    pub fn submit(&mut self, now: u32) -> Submit {
        let Some(mode) = self.open.take() else {
            return Submit::Empty;
        };
        let line = std::mem::take(&mut self.text);
        // the guard runs before anything else: a locked box refuses even an empty Enter
        if self.repeat < 1 {
            if now.wrapping_sub(self.locked_at) < REPEAT_LOCK_MS {
                return Submit::Refused;
            }
            self.repeat = REPEAT_AFTER_LOCK;
            self.locked_at = 0;
        }
        if line.trim().is_empty() {
            return Submit::Empty;
        }
        // CLI-1625: a local command is never sent, so it does not count towards the repeat guard (`/wallhack` three
        // times in a row cycles the styles)
        if local_command(&line).is_some() {
            return Submit::Send(line, mode);
        }
        if line == self.prev_line {
            self.repeat -= 1;
            if self.repeat < 1 {
                self.locked_at = now;
            }
        } else {
            self.prev_line = line.clone();
            self.repeat = REPEAT_NEW_LINE;
        }
        Submit::Send(line, mode)
    }
}

/// A key press as the chat input sees it (the window maps its key codes to these).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatPress {
    /// Enter: opens the box for all players, or sends the line.
    Enter,
    /// The team chat key: opens the box for the own team (typed as text while the box is open).
    Team,
    /// Escape: closes the box and drops the line.
    Escape,
    Backspace,
    /// Text the key produced while the box is open.
    Text(String),
}

/// What a press did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatOutcome {
    /// The press belongs to the chat box (the game does not see it).
    pub consumed: bool,
    /// The box just opened (the game's held keys are released).
    pub opened: bool,
    /// A line to send as `UP_CHAT`.
    pub send: Option<(String, ChatScope)>,
    /// The repeat guard refused the line (show the refusal line).
    pub refused: bool,
    /// The line was a local command (not sent, CLI-1570).
    pub command: Option<LocalCommand>,
}

/// A chat line the client handles itself instead of sending it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCommand {
    /// CLI-1636: `/help`: the local commands of this match in the chat window.
    Help,
    /// `/network` or `/net` with its argument ([`super::netview`]).
    NetOverlay(super::netview::NetArg),
    /// CLI-1625: `/wallhack` (any case, arguments ignored): the next x-ray style.
    Wallhack,
    /// CLI-1630: `/nametags` (any case, arguments ignored): the x-ray's name tags on / off.
    Nametags,
    /// CLI-1630: `/health` or `/healthbars` (any case, arguments ignored): the x-ray's health bars on / off.
    HealthBars,
    /// CLI-1635: `/teammates` (any case, arguments ignored): the team mates' x-ray on / off.
    Teammates,
    /// CLI-1841: `/report` (any case, arguments ignored): a diagnostic report now.
    Report,
}

/// CLI-1636: what `/help` lists in every match, one chat line each. The chat window holds six lines (`CHAT_MAX_LINES`).
pub const HELP_LINES: &[&str] = &[
    "/help - lists these commands; /report sends the server's owner a diagnostic report",
    "/network (or /net) - network numbers on / off; /network 2 adds the players, /network off hides them",
];
/// CLI-1636: what `/help` adds on a declared x-ray server (the CLI-1620 bridge exists).
pub const HELP_MOD_LINES: &[&str] = &[
    "/wallhack - next see-through style: box, chams, skeleton",
    "/nametags - enemy name tags on / off",
    "/health (or /healthbars) - health bars on / off",
    "/teammates - team mates' see-through colours on / off",
];

/// The local command of a typed line: `/network` or `/net` (any case), alone or with one argument (`2` the players
/// block, `off` / `0` off, anything else ignored); every other line is chat.
// DC-DEFAULT: CLI-1570 (proposed)
pub fn local_command(line: &str) -> Option<LocalCommand> {
    use super::netview::NetArg;
    let mut words = line.split_whitespace();
    let head = words.next()?.to_ascii_lowercase();
    // DC-DEFAULT: CLI-1625 (proposed)
    // DC-DEFAULT: CLI-1636 (proposed, USER REQUEST 2026-11-01)
    if head == "/help" {
        return Some(LocalCommand::Help);
    }
    if head == "/wallhack" {
        return Some(LocalCommand::Wallhack);
    }
    // DC-DEFAULT: CLI-1841 (proposed, USER REQUEST 2026-11-02)
    if head == "/report" {
        return Some(LocalCommand::Report);
    }
    // DC-DEFAULT: CLI-1630 (proposed, USER REQUEST 2026-11-01)
    if head == "/nametags" {
        return Some(LocalCommand::Nametags);
    }
    if head == "/health" || head == "/healthbars" {
        return Some(LocalCommand::HealthBars);
    }
    // DC-DEFAULT: CLI-1635 (proposed, USER REQUEST 2026-11-01)
    if head == "/teammates" {
        return Some(LocalCommand::Teammates);
    }
    if head != "/network" && head != "/net" {
        return None;
    }
    let arg = match (words.next(), words.next()) {
        (None, _) => NetArg::Toggle,
        (Some("2"), None) => NetArg::Players,
        (Some(a), None) if a == "0" || a.eq_ignore_ascii_case("off") => NetArg::Off,
        _ => NetArg::Unknown,
    };
    Some(LocalCommand::NetOverlay(arg))
}

impl ChatInput {
    /// One key press at `now`.
    // DC-DEFAULT: CLI-490 (proposed) Enter opens the box for all players and sends, the team chat key opens it for
    // the own team, Escape closes it, keys typed while it is open do not reach the game (the 2009 bindings are profile
    // configuration that is not read)
    pub fn press(&mut self, p: ChatPress, now: u32) -> ChatOutcome {
        let mut o = ChatOutcome::default();
        if !self.is_open() {
            let mode = match p {
                ChatPress::Enter => ChatScope::All,
                ChatPress::Team => ChatScope::Team,
                _ => return o,
            };
            self.open(mode, now);
            o.consumed = true;
            o.opened = true;
            return o;
        }
        o.consumed = true;
        match p {
            ChatPress::Enter => match self.submit(now) {
                Submit::Send(l, m) => match local_command(&l) {
                    Some(c) => o.command = Some(c),
                    None => o.send = Some((l, m)),
                },
                Submit::Refused => o.refused = true,
                Submit::Empty => {}
            },
            ChatPress::Escape => self.cancel(),
            ChatPress::Backspace => self.backspace(),
            ChatPress::Text(t) => self.type_text(&t),
            ChatPress::Team => {}
        }
        o
    }
}

// ---- look ---------------------------------------------------------------------------------------------------------

/// An image of the input element: texture, place, size (`tex` may be a `%s` form base, see [`ChatLook::mode_tex`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputImage {
    pub tex: String,
    pub pos: [i32; 2],
    pub size: [i32; 2],
}

/// The chat colours, the input element and the banner style.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatLook {
    pub team: [u8; 4],
    pub friendly: [u8; 4],
    pub enemy: [u8; 4],
    pub moderator: [u8; 4],
    /// The repeat-guard refusal line's colour.
    pub same_line: [u8; 4],
    /// The moderator name tag (from CShell; `None`: no tag, no moderator colour).
    pub moderator_tag: Option<String>,
    /// Input line: font record, place, colour; caret colour and blink time (ms).
    pub input_font: i32,
    pub input_pos: [i32; 2],
    pub input_colour: [u8; 4],
    pub caret_colour: [u8; 4],
    pub caret_ms: u32,
    /// Images in the 2009 draw order: picture, back, enter, then the all / team scope buttons (their texture is the
    /// base name with the selected or the unselected suffix form).
    pub images: Vec<InputImage>,
    pub all_button: Option<InputImage>,
    pub team_button: Option<InputImage>,
    /// The two suffix forms (`%s` + selected / unselected) of the scope buttons.
    pub selected_form: String,
    pub unselected_form: String,
}

impl ChatLook {
    /// The colour of a line class.
    pub fn colour(&self, c: ChatClass) -> [u8; 4] {
        match c {
            ChatClass::Team => self.team,
            ChatClass::Friendly => self.friendly,
            ChatClass::Enemy => self.enemy,
            ChatClass::Moderator => self.moderator,
        }
    }

    /// The texture of a scope button: the selected form when `selected`.
    // EVIDENCE: cshell+0x72130 (mode == button -> first form, else the second, sprintf of the base name)
    pub fn mode_tex(&self, base: &str, selected: bool) -> String {
        let form = if selected {
            &self.selected_form
        } else {
            &self.unselected_form
        };
        form.replacen("%s", base, 1)
    }

    /// Our look without a ROM.
    // DC-DEFAULT: CLI-493 (proposed) no-ROM chat look: team green, friendly light blue, enemy red, moderator yellow,
    // the input line white at (16, 530) with a white caret blinking every 500 ms, no images
    pub fn fallback() -> ChatLook {
        ChatLook {
            team: [120, 230, 120, 255],
            friendly: [150, 200, 255, 255],
            enemy: [255, 110, 100, 255],
            moderator: [255, 230, 80, 255],
            same_line: [255, 255, 255, 255],
            moderator_tag: None,
            input_font: -1,
            input_pos: [16, 530],
            input_colour: [255, 255, 255, 255],
            caret_colour: [255, 255, 255, 255],
            caret_ms: 500,
            images: Vec::new(),
            all_button: None,
            team_button: None,
            selected_form: "%s".into(),
            unselected_form: "%s".into(),
        }
    }

    /// The look from the user's ROM: the key texts from CShell, the values from the loaded layout table (set 0).
    pub fn from_rom(rom: &Path, table: &LayoutTable) -> Option<ChatLook> {
        let p = rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]);
        let pe = dc_rom::PeFile::from_file(&p).ok()?;
        let keys = ChatKeys::from_cshell(&pe).ok()?;
        Some(Self::from_table(table, &keys))
    }

    /// The look from a layout table and key texts.
    pub fn from_table(table: &LayoutTable, keys: &ChatKeys) -> ChatLook {
        let raw = |k: LookKey| table.hud_raw(0, keys.key(k)).map(parse_value);
        let real = |k: LookKey| match raw(k)? {
            Value::Real(v) => Some(v),
            Value::Int(v) => Some(v as f64),
            _ => None,
        };
        // EVIDENCE: cshell+0x50510 / 0x50690 / 0x50750 / 0x505d0 / 0x50c90 (colour vector and alpha, default alpha 1.0,
        // four truncations); the packing as `HudView::rgba` (cshell+0x514d0)
        let rgba = |c: LookKey, a: LookKey, dflt: [u8; 4]| -> [u8; 4] {
            let Some(Value::Vec3(v)) = raw(c) else {
                return dflt;
            };
            let b = |x: f64| x.clamp(0.0, 255.0) as u8;
            [b(v[0]), b(v[1]), b(v[2]), b(real(a).unwrap_or(1.0) * 255.0)]
        };
        let pair = |k: LookKey| match raw(k)? {
            Value::Pair(p) => Some(p),
            _ => None,
        };
        let text = |k: LookKey| match raw(k)? {
            Value::Str(s) => Some(s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()),
            _ => None,
        };
        let image = |t: LookKey, p: LookKey, s: LookKey| {
            Some(InputImage {
                tex: text(t)?,
                pos: pair(p)?,
                size: pair(s)?,
            })
        };
        let fb = ChatLook::fallback();
        use LookKey as C;
        ChatLook {
            team: rgba(C::TeamColour, C::TeamOpacity, fb.team),
            friendly: rgba(C::FriendlyColour, C::FriendlyAlpha, fb.friendly),
            enemy: rgba(C::EnemyColour, C::EnemyAlpha, fb.enemy),
            moderator: rgba(C::ModeratorColour, C::ModeratorAlpha, fb.moderator),
            same_line: rgba(C::SameLineColour, C::SameLineAlpha, fb.same_line),
            moderator_tag: keys.moderator_tag.clone(),
            input_font: match raw(C::InputFont) {
                Some(Value::Int(i)) => i,
                _ => fb.input_font,
            },
            input_pos: pair(C::InputPos).unwrap_or(fb.input_pos),
            input_colour: rgba(C::InputColour, C::InputAlpha, fb.input_colour),
            caret_colour: rgba(C::CaretColour, C::CaretAlpha, fb.caret_colour),
            // EVIDENCE: cshell+0x4fd50 (the toggle time is a real, seconds)
            caret_ms: real(C::CaretToggle)
                .filter(|v| *v > 0.0)
                .map_or(fb.caret_ms, |v| (v * 1000.0) as u32),
            // EVIDENCE: cshell+0x729f0 (draw order: edit box, picture, back, enter, all, team, clan)
            images: [
                (C::ImgTex, C::ImgPos, C::ImgRec),
                (C::BackTex, C::BackPos, C::BackRec),
                (C::EnterTex, C::EnterPos, C::EnterRec),
            ]
            .into_iter()
            .filter_map(|(t, p, s)| image(t, p, s))
            .collect(),
            all_button: image(C::AllTex, C::AllPos, C::AllRec),
            team_button: image(C::TeamTex, C::TeamPos, C::TeamRec),
            selected_form: keys.selected_form.clone(),
            unselected_form: keys.unselected_form.clone(),
        }
    }
}

/// The layout keys the chat elements read (our names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookKey {
    TeamColour,
    TeamOpacity,
    ModeratorColour,
    ModeratorAlpha,
    FriendlyColour,
    FriendlyAlpha,
    EnemyColour,
    EnemyAlpha,
    SameLineColour,
    SameLineAlpha,
    InputFont,
    InputPos,
    InputColour,
    InputAlpha,
    CaretToggle,
    CaretColour,
    CaretAlpha,
    ImgTex,
    ImgPos,
    ImgRec,
    BackTex,
    BackPos,
    BackRec,
    EnterTex,
    EnterPos,
    EnterRec,
    AllTex,
    AllPos,
    AllRec,
    TeamTex,
    TeamPos,
    TeamRec,
}

/// `(key, literal rva, sha16, length)`; the evidence is the accessor or layout routine that pushes the literal.
// EVIDENCE: cshell+0x50510 (team), 0x505d0 (moderator), 0x50690 (friendly), 0x50750 (enemy), 0x50c90 (same line),
// 0x4fa10 / 0x4fa80 / 0x4faf0 (input font / place / colour), 0x4fd50 / 0x4fdc0 (caret), 0x72130 (input images)
const CHAT_PINS: [(LookKey, u32, &str, usize); 32] = [
    (LookKey::TeamColour, 0x3d18c8, "7d6b13d551f72c5a", 13),
    (LookKey::TeamOpacity, 0x3d18b8, "e4a2b4cf80f1fe90", 13),
    (LookKey::ModeratorColour, 0x3d18e4, "d7e30091c622642b", 11),
    (LookKey::ModeratorAlpha, 0x3d18d8, "129dfe47c0cc179f", 11),
    (LookKey::FriendlyColour, 0x3d1900, "0b5f2656d8bf31b8", 13),
    (LookKey::FriendlyAlpha, 0x3d18f0, "29eea57f9931cc59", 13),
    (LookKey::EnemyColour, 0x3d191c, "1beb01c24b868b00", 10),
    (LookKey::EnemyAlpha, 0x3d1910, "fe101432a5a30a18", 10),
    (LookKey::SameLineColour, 0x3d1a04, "ff76c4ec5cfbbe17", 17),
    (LookKey::SameLineAlpha, 0x3d19f0, "dbf12cdcd865cdf4", 17),
    (LookKey::InputFont, 0x3d1714, "d17da6ade21abbcf", 13),
    (LookKey::InputPos, 0x3d1724, "74d4c68f2e2f28ae", 12),
    (LookKey::InputColour, 0x3d1744, "8373e356117f891d", 14),
    (LookKey::InputAlpha, 0x3d1734, "d7626bcf4b98e12e", 14),
    (LookKey::CaretToggle, 0x3d1778, "098b789f916bda3b", 15),
    (LookKey::CaretColour, 0x3d1794, "aef856e1713dd129", 10),
    (LookKey::CaretAlpha, 0x3d1788, "574b39d1374662cf", 10),
    (LookKey::ImgTex, 0x3d5288, "dfc9f7d9f1efdd07", 15),
    (LookKey::ImgPos, 0x3d5278, "b36db0a6ecc318f7", 15),
    (LookKey::ImgRec, 0x3d5268, "20cc6511252aa016", 15),
    (LookKey::BackTex, 0x3d5254, "35a5ffc1aa70ff11", 16),
    (LookKey::BackPos, 0x3d5240, "266e13df302f3a15", 16),
    (LookKey::BackRec, 0x3d522c, "6e402106aa90e7f2", 16),
    (LookKey::EnterTex, 0x3d5218, "cf06413e8164fe97", 17),
    (LookKey::EnterPos, 0x3d5204, "c055ab02cb5c4dae", 17),
    (LookKey::EnterRec, 0x3d51f0, "e240b3e74449e435", 17),
    (LookKey::AllTex, 0x3d5170, "af2d81298aa92079", 15),
    (LookKey::AllPos, 0x3d51e0, "1e040a046f269f72", 15),
    (LookKey::AllRec, 0x3d51d0, "35cacc30edf20a85", 15),
    (LookKey::TeamTex, 0x3d514c, "5a8c3539f7ef2cab", 16),
    (LookKey::TeamPos, 0x3d51bc, "cda65aa3c6a3ff3e", 16),
    (LookKey::TeamRec, 0x3d51a8, "895c38fbefc2b9c1", 16),
];

/// The scope buttons' two texture forms (`%s` + suffix).
// EVIDENCE: cshell+0x72130 (sprintf of the base name with one of the two forms)
const SELECTED_FORM_PIN: Pin = Pin {
    what: "chat input form",
    rva: 0x3d5168,
    sha16: "0721173f74674471",
    len: 7,
};
const UNSELECTED_FORM_PIN: Pin = Pin {
    what: "chat input form",
    rva: 0x3d5160,
    sha16: "dbe7fdf0f1d9653c",
    len: 7,
};

/// The moderator name tag: a 2-unit wide literal, pinned by the SHA-256 of its UTF-16LE bytes.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68 (str16(VA 0x374cd9cc; 69bb7a1f3feb2eec; 2))
const MODERATOR_TAG_RVA: u32 = 0x3cd9cc;
const MODERATOR_TAG_SHA16: &str = "69bb7a1f3feb2eec";
const MODERATOR_TAG_UNITS: usize = 2;

impl LookKey {
    fn pin(self) -> Pin {
        let r = CHAT_PINS[self as usize];
        Pin {
            what: "chat layout key",
            rva: r.1,
            sha16: r.2,
            len: r.3,
        }
    }
}

/// The key texts of the user's CShell.dll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatKeys {
    keys: Vec<String>,
    pub selected_form: String,
    pub unselected_form: String,
    pub moderator_tag: Option<String>,
}

impl ChatKeys {
    /// Reads every pinned literal; any mismatch means "not the 2009 build". The moderator tag is optional.
    pub fn from_cshell(pe: &dc_rom::PeFile) -> Result<ChatKeys, String> {
        let keys = CHAT_PINS
            .iter()
            .map(|r| read_pinned(pe, &r.0.pin()).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let selected_form = read_pinned(pe, &SELECTED_FORM_PIN).map_err(|e| e.to_string())?;
        let unselected_form = read_pinned(pe, &UNSELECTED_FORM_PIN).map_err(|e| e.to_string())?;
        let moderator_tag = pe.wstr(MODERATOR_TAG_RVA).ok().filter(|t| {
            let b: Vec<u8> = t.encode_utf16().flat_map(u16::to_le_bytes).collect();
            t.encode_utf16().count() == MODERATOR_TAG_UNITS
                && dc_rom::sha256_hex(&b)[..16] == *MODERATOR_TAG_SHA16
        });
        Ok(ChatKeys {
            keys,
            selected_form,
            unselected_form,
            moderator_tag,
        })
    }

    /// Invented names (`chat_key_<n>`) for fixtures without a ROM.
    pub fn placeholder() -> ChatKeys {
        ChatKeys {
            keys: (0..CHAT_PINS.len())
                .map(|i| format!("chat_key_{i}"))
                .collect(),
            selected_form: "%s_on.dtx".into(),
            unselected_form: "%s_off.dtx".into(),
            moderator_tag: Some("[M]".into()),
        }
    }

    pub fn key(&self, k: LookKey) -> &str {
        &self.keys[k as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(name: &str, text: &str, sender: u32, team_only: bool, whisper: u32) -> BitWriter {
        let n: Vec<u16> = name.encode_utf16().collect();
        let t: Vec<u16> = text.encode_utf16().collect();
        let mut w = BitWriter::new();
        w.write_zstr16_presumed(&n)
            .write_zstr16_presumed(&t)
            .write_u32(sender)
            .write_u8(team_only as u8)
            .write_u32(whisper);
        w
    }

    #[test]
    fn pins_are_in_enum_order_and_distinct() {
        for (i, r) in CHAT_PINS.iter().enumerate() {
            assert_eq!(r.0 as usize, i, "{:?}", r.0);
            assert_eq!(r.2.len(), 16);
        }
        let mut rvas: Vec<u32> = CHAT_PINS.iter().map(|r| r.1).collect();
        rvas.sort_unstable();
        rvas.dedup();
        assert_eq!(rvas.len(), CHAT_PINS.len());
    }

    #[test]
    fn reader_takes_the_whole_body_and_cuts_the_strings() {
        let w = body("Ann", "hello", 3, true, 0);
        let m = read_gs_chat(w.as_bytes(), w.bit_len()).unwrap();
        assert_eq!(
            m,
            GsChat {
                name: "Ann".into(),
                text: "hello".into(),
                sender: 3,
                team_only: true,
                whisper: 0
            }
        );
        let long: String = "x".repeat(40);
        let w = body(&long, "t", 1, false, 0);
        assert_eq!(
            read_gs_chat(w.as_bytes(), w.bit_len()).unwrap().name.len(),
            NAME_UNITS - 1
        );
        // a body that ends before the numbers is not shown
        let mut short = BitWriter::new();
        short
            .write_zstr16_presumed(&[0x41])
            .write_zstr16_presumed(&[0x42])
            .write_u32(1);
        assert_eq!(read_gs_chat(short.as_bytes(), short.bit_len()), None);
    }

    #[test]
    fn classes_follow_the_handler_order() {
        let m = |name: &str, sender: u32, team_only: bool| GsChat {
            name: name.into(),
            text: "hi".into(),
            sender,
            team_only,
            whisper: 0,
        };
        let class = |s: Option<ChatShow>| match s {
            Some(ChatShow::Line { class, .. }) => Some(class),
            _ => None,
        };
        // own team 1: a mate, an enemy, a team line from the enemy side (team_only wins), a tagged name (last)
        assert_eq!(
            class(classify(
                &m("Bo", 2, false),
                Some(1),
                Some(1),
                Some(1),
                Some("[M]")
            )),
            Some(ChatClass::Friendly)
        );
        assert_eq!(
            class(classify(
                &m("Cy", 3, false),
                Some(1),
                Some(1),
                Some(2),
                Some("[M]")
            )),
            Some(ChatClass::Enemy)
        );
        assert_eq!(
            class(classify(
                &m("Cy", 3, true),
                Some(1),
                Some(1),
                Some(2),
                Some("[M]")
            )),
            Some(ChatClass::Team)
        );
        assert_eq!(
            class(classify(
                &m("[M]x", 3, true),
                Some(1),
                Some(1),
                Some(2),
                Some("[M]")
            )),
            Some(ChatClass::Moderator)
        );
        // no roster record: nothing shown
        assert_eq!(
            classify(&m("Zed", 9, false), Some(1), Some(1), None, None),
            None
        );
        // the system notice is the banner, with or without a record
        assert_eq!(
            classify(&m("", SYSTEM_SENDER, false), Some(1), Some(1), None, None),
            Some(ChatShow::Banner("hi".into()))
        );
        // the own echo is marked
        match classify(&m("Me", 1, false), Some(1), Some(1), Some(1), None) {
            Some(ChatShow::Line { own, text, .. }) => {
                assert!(own);
                assert_eq!(text, "Me: hi");
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn line_text_does_not_name_the_sender_twice() {
        assert_eq!(line_text("Ann", "Ann : yo"), "Ann : yo");
        assert_eq!(line_text("Ann", "yo"), "Ann: yo");
        assert_eq!(line_text("Ann", "Ann: yo"), "Ann: yo");
        // a line that only starts with the name's letters still names its sender
        assert_eq!(line_text("Bo", "Bob is here"), "Bo: Bob is here");
        assert_eq!(line_text("A", "Attack"), "A: Attack");
    }

    #[test]
    fn up_chat_body_is_text_empty_tag_and_scope() {
        let w = up_chat_body("ab", ChatScope::Team);
        // 2 units + terminator, empty tag terminator, scope byte
        assert_eq!(w.bit_len(), 3 * 16 + 16 + 8);
        let mut r = w.reader();
        assert_eq!(r.read_zstr16(), vec![0x61, 0x62]);
        assert!(r.read_zstr16().is_empty());
        assert_eq!(r.read_u8(), 1);
    }

    #[test]
    fn input_types_sends_and_closes() {
        let mut c = ChatInput::default();
        c.type_text("ignored");
        assert_eq!(c.text(), "");
        c.open(ChatScope::All, 10);
        c.type_text("hel\u{8}lo");
        c.backspace();
        assert_eq!(c.text(), "hell");
        assert_eq!(c.submit(20), Submit::Send("hell".into(), ChatScope::All));
        assert!(!c.is_open());
        c.open(ChatScope::Team, 30);
        c.type_text(&"y".repeat(200));
        assert_eq!(c.text().chars().count(), INPUT_MAX_CHARS);
        c.cancel();
        assert!(!c.is_open());
        c.open(ChatScope::Team, 40);
        c.type_text("   ");
        assert_eq!(c.submit(50), Submit::Empty);
    }

    #[test]
    fn presses_open_type_send_and_cancel() {
        let mut c = ChatInput::default();
        // closed: text and escape are the game's
        assert!(!c.press(ChatPress::Text("w".into()), 0).consumed);
        assert!(!c.press(ChatPress::Escape, 0).consumed);
        let o = c.press(ChatPress::Team, 5);
        assert!(o.consumed && o.opened);
        assert_eq!(c.mode(), Some(ChatScope::Team));
        c.press(ChatPress::Text("gg".into()), 6);
        let o = c.press(ChatPress::Enter, 7);
        assert_eq!(o.send, Some(("gg".to_string(), ChatScope::Team)));
        assert!(!c.is_open());
        c.press(ChatPress::Enter, 8);
        assert_eq!(c.mode(), Some(ChatScope::All));
        c.press(ChatPress::Text("x".into()), 9);
        assert!(c.press(ChatPress::Escape, 10).consumed);
        assert!(!c.is_open());
    }

    /// cshell+0x72df0 tests the guard before the line: a locked box refuses an empty Enter too.
    #[test]
    fn network_commands_are_parsed_and_not_sent() {
        use crate::hud::netview::NetArg;
        let cmd = |l: &str| local_command(l);
        assert_eq!(cmd("/network"), Some(LocalCommand::NetOverlay(NetArg::Toggle)));
        assert_eq!(cmd(" /NET "), Some(LocalCommand::NetOverlay(NetArg::Toggle)));
        assert_eq!(cmd("/network 2"), Some(LocalCommand::NetOverlay(NetArg::Players)));
        assert_eq!(cmd("/net off"), Some(LocalCommand::NetOverlay(NetArg::Off)));
        assert_eq!(cmd("/network banana"), Some(LocalCommand::NetOverlay(NetArg::Unknown)));
        assert_eq!(cmd("/network 2 3"), Some(LocalCommand::NetOverlay(NetArg::Unknown)));
        assert_eq!(cmd("/networks"), None);
        assert_eq!(cmd("/wallhack"), Some(LocalCommand::Wallhack));
        assert_eq!(cmd(" /WallHack x"), Some(LocalCommand::Wallhack));
        assert_eq!(cmd("/wallhacks"), None);
        assert_eq!(cmd("/nametags"), Some(LocalCommand::Nametags));
        assert_eq!(cmd(" /NameTags on"), Some(LocalCommand::Nametags));
        assert_eq!(cmd("/nametag"), None);
        assert_eq!(cmd("/health"), Some(LocalCommand::HealthBars));
        assert_eq!(cmd("/HealthBars"), Some(LocalCommand::HealthBars));
        assert_eq!(cmd("/teammates"), Some(LocalCommand::Teammates));
        assert_eq!(cmd(" /TeamMates off"), Some(LocalCommand::Teammates));
        assert_eq!(cmd("/healthbar"), None);
        assert_eq!(cmd("gg /net"), None);
        let mut c = ChatInput::default();
        for (line, sent) in [("/network", false), ("/network 2", false), ("/net x", false), ("hi", true)] {
            c.press(ChatPress::Enter, 0);
            c.press(ChatPress::Text(line.into()), 0);
            let o = c.press(ChatPress::Enter, 0);
            assert_eq!(o.send.is_some(), sent, "{line}");
            assert_eq!(o.command.is_some(), !sent, "{line}");
        }
        // a local command repeated is never refused
        for _ in 0..6 {
            c.press(ChatPress::Enter, 0);
            c.press(ChatPress::Text("/wallhack".into()), 0);
            let o = c.press(ChatPress::Enter, 0);
            assert_eq!(o.command, Some(LocalCommand::Wallhack));
            assert!(!o.refused);
        }
        for line in ["/nametags", "/health", "/healthbars"].iter().cycle().take(9) {
            c.press(ChatPress::Enter, 0);
            c.press(ChatPress::Text((*line).into()), 0);
            let o = c.press(ChatPress::Enter, 0);
            assert!(o.command.is_some() && o.send.is_none() && !o.refused, "{line}");
        }
    }

    #[test]
    fn locked_guard_refuses_an_empty_enter() {
        let mut c = ChatInput::default();
        for i in 0..10 {
            c.open(ChatScope::All, i);
            c.type_text("spam");
            assert!(matches!(c.submit(i), Submit::Send(..)));
        }
        c.open(ChatScope::All, 20);
        c.type_text("spam");
        assert_eq!(c.submit(20), Submit::Refused);
        c.open(ChatScope::All, 30);
        assert_eq!(c.submit(30), Submit::Refused);
        // after the lock an empty Enter resets the counter and closes the box
        c.open(ChatScope::All, 20 + REPEAT_LOCK_MS);
        assert_eq!(c.submit(20 + REPEAT_LOCK_MS), Submit::Empty);
        assert!(!c.is_open());
    }

    #[test]
    fn repeat_guard_locks_for_a_minute_after_nine_repeats() {
        let mut c = ChatInput::default();
        let send = |c: &mut ChatInput, t: u32| {
            c.open(ChatScope::All, t);
            c.type_text("spam");
            c.submit(t)
        };
        // the first line and nine repeats go out; the tenth repeat is held
        for i in 0..10 {
            assert!(
                matches!(send(&mut c, i * 100), Submit::Send(..)),
                "send {i}"
            );
        }
        assert_eq!(send(&mut c, 2000), Submit::Refused);
        assert_eq!(send(&mut c, 900 + REPEAT_LOCK_MS - 1), Submit::Refused);
        assert!(matches!(
            send(&mut c, 900 + REPEAT_LOCK_MS),
            Submit::Send(..)
        ));
        // a new line resets the count
        c.open(ChatScope::All, 0);
        c.type_text("other");
        assert!(matches!(
            c.submit(900 + REPEAT_LOCK_MS + 1),
            Submit::Send(..)
        ));
    }

    #[test]
    fn placeholder_look_reads_a_made_up_table() {
        let keys = ChatKeys::placeholder();
        let look = ChatLook::fallback();
        assert_eq!(look.mode_tex("x", true), "x");
        let rom_like = ChatLook {
            selected_form: keys.selected_form.clone(),
            ..look
        };
        assert_eq!(rom_like.mode_tex("x", true), "x_on.dtx");
        assert_eq!(keys.key(LookKey::TeamRec), "chat_key_31");
    }
}
