//! The menu's side of the diagnostic reports (CLI-1841, CLI-1843; project decision, no 2009 counterpart): the
//! consent questions the first time, the box after a problem (send / test the connection / no), the connection
//! test's result, the bug form's "attach diagnostics", the lobby chat's `/report`, `/diag` and `/nettest`, and the
//! connection test panel of the server screen. The boxes are the 2009 message box with our own words and button
//! captions; they open only when no other box is up, so the 2009 box of a failure (the lost connection) comes first.
//! What a report holds and where it goes: [`crate::diag`], docs/DIAGNOSTICS.md.
// DC-DEFAULT: CLI-1841 (proposed, USER REQUEST 2026-11-02) the consent boxes and the after-failure box; CLI-1843
// (proposed, project decision 2026-11-02) the connection test on the server screen and in the after-failure box

use super::popup::{BoxKind, BoxTag, Popup};
use super::{store_settings, Ctx, Menu, MenuData};
use crate::diag::{self, probe, Consent, Failure, Kind, Plan};
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

/// Which of our boxes is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// "Send reports automatically?" Yes = always, No = the second question.
    Consent1,
    /// "Ask each time instead?" Yes = ask, No = never.
    Consent2,
    /// After a problem (consent ask): send / (test) / no.
    Ask,
    /// After a network problem whose report went (consent always): test / OK.
    AlwaysTest,
    /// The test runs (a wait box).
    Testing,
    /// The test's result: send with it / no (ask), or OK (always).
    Result,
    /// After a bug report: attach diagnostics?
    BugAttach,
}

pub const T_SEND: &str = "Send report";
pub const T_TEST: &str = "Test connection";
pub const T_NO: &str = "No";
pub const T_OK: &str = "OK";
pub const T_CONSENT1: &str = "Send diagnostic reports to the server's owner automatically when the game has a \
                              problem? A report holds your game logs and system details, never your password. \
                              (No: more choices)";
pub const T_CONSENT2: &str = "Ask you each time a problem happens instead? (No: never send reports.) You can change \
                              this later with /diag in the lobby chat.";
pub const T_ASK: &str = "Send a diagnostic report to the server's owner? It holds your game logs and system \
                         details, never your password.";
pub const T_SENT_TEST: &str = "A diagnostic report goes to the server's owner. Test your connection to this server?";
pub const T_TESTING: &str = "Testing your connection to the server...";
pub const T_RESULT_ASK: &str = "Send a diagnostic report with this result?";
pub const T_BUG: &str = "Attach diagnostics (game logs and system details, never your password) to your bug \
                         report?";
/// The colour of our lines in the lobby chat.
pub const NOTE_COLOUR: u32 = 0xffe7_ae54;
/// How often the menu tells [`diag`] what it knows.
const CONTEXT_EVERY: Duration = Duration::from_secs(2);

/// The menu's state of the reports.
#[derive(Debug, Default)]
pub struct DiagUi {
    /// The problem whose boxes are open.
    current: Option<Failure>,
    /// The connection test of the current problem runs (its generation when it started).
    testing: Option<u64>,
    /// The current problem's report was sent already (consent always).
    sent: bool,
    /// The bug report's text waiting for "attach diagnostics".
    bug_note: Option<String>,
    /// A test started from the lobby chat: say its results there (and send them with `/nettest send`).
    chat_probe: Option<(u64, bool)>,
    /// The probe generation last drawn.
    pub seen_generation: u64,
    last_context: Option<Instant>,
}

/// The settings a report names.
pub fn settings_json(data: &MenuData) -> Value {
    let s = &data.settings;
    json!({
        "resolution": s.resolution.map(|(w, h)| format!("{w}x{h}")),
        "window": format!("{:?}", s.window),
        "fps_cap": s.fps_cap,
        "antialias": s.antialias,
        "show_fps": s.show_fps,
        "net_overlay": s.net_overlay,
        "gamma": s.gamma,
        "diag_consent": Consent::from_u8(s.diag_consent).name(),
    })
}

/// The servers' addresses a report may name: the lobby, its entries, the other lobbies of the list.
pub fn server_ips(data: &MenuData) -> Vec<IpAddr> {
    let mut v: Vec<IpAddr> = data.auth.map(|a| a.ip()).into_iter().collect();
    v.extend(data.servers.iter().filter_map(|e| e.ip.parse::<IpAddr>().ok()));
    v.extend(super::moddedrows::modded_rows(data).iter().filter_map(|r| r.addr.map(|a| a.ip())));
    v.sort();
    v.dedup();
    v
}

/// Where the match was, for a report.
pub fn match_json(data: &MenuData, end: &str) -> Value {
    json!({
        "lobby": data.auth.map(|a| a.to_string()),
        "entry": data.server.as_ref().map(|e| json!({"id": e.id, "name": e.name})),
        "room": data.room.number,
        "map": data.room.map,
        "mode": data.room.mode,
        "end": end,
    })
}

fn consent(data: &MenuData) -> Consent {
    Consent::from_u8(data.settings.diag_consent)
}

fn set_consent(m: &mut Menu, c: Consent) {
    m.data.settings.diag_consent = c.as_u8();
    m.data.settings_changed = true;
    store_settings(&mut m.data, &m.cat);
}

/// The connection test's targets: one per echo address (the lobby's web port number over UDP), named after the
/// rows that use it: this lobby's entries, then the other lobbies of the list.
pub fn targets(data: &MenuData) -> Vec<(String, probe::Target)> {
    let mut out: Vec<(String, probe::Target)> = Vec::new();
    let Some(auth) = data.auth else {
        return out;
    };
    let mut add = |name: String, ip: IpAddr, port_of: SocketAddr, lobby: bool| {
        if let Some(addr) = diag::web_of(SocketAddr::new(ip, port_of.port())) {
            out.push((name, probe::Target { name: String::new(), addr, lobby }));
        }
    };
    if data.servers.is_empty() {
        add(data.picked_server.clone().unwrap_or_else(|| "Lobby".into()), auth.ip(), auth, true);
    }
    for e in &data.servers {
        let ip = e
            .ip
            .parse::<IpAddr>()
            .ok()
            .filter(|ip| !ip.is_unspecified())
            .unwrap_or(auth.ip());
        // an entry on the lobby's host has the lobby's echo; another host may have none
        add(e.name.clone(), ip, auth, ip == auth.ip());
    }
    for r in super::moddedrows::modded_rows(data) {
        if let Some(a) = r.addr {
            add(super::moddedrows::row_label(&r), a.ip(), a, true);
        }
    }
    for (name, t) in out.iter_mut() {
        t.name = name.clone();
    }
    out
}

/// The unique echo addresses of `targets` (one measurement each).
pub fn unique(targets: &[(String, probe::Target)]) -> Vec<probe::Target> {
    let mut v: Vec<probe::Target> = Vec::new();
    for (_, t) in targets {
        if !v.iter().any(|x| x.addr == t.addr) {
            v.push(t.clone());
        }
    }
    v
}

/// The test of the current problem's lobby.
fn failure_target(m: &Menu, f: &Failure) -> Option<probe::Target> {
    let auth = f.lobby.or(m.data.auth)?;
    Some(probe::Target {
        name: m
            .data
            .server
            .as_ref()
            .map(|e| e.name.clone())
            .or_else(|| m.data.picked_server.clone())
            .unwrap_or_else(|| "the server".into()),
        addr: diag::web_of(auth)?,
        lobby: true,
    })
}

fn push(m: &mut Menu, p: Popup) {
    m.popups.push(p);
    m.dirty = true;
}

fn ask_box(f: &Failure) -> Popup {
    let text = format!("{} {T_ASK}", f.kind.what_happened());
    if f.kind.is_network() {
        Popup::choice(text, BoxTag::Diag(Step::Ask), &[T_SEND, T_TEST, T_NO])
    } else {
        Popup::choice(text, BoxTag::Diag(Step::Ask), &[T_SEND, T_NO])
    }
}

/// A problem goes by its plan.
fn handle(m: &mut Menu, mut f: Failure) {
    if f.lobby.is_none() {
        f.lobby = m.data.auth;
    }
    f.keep_ips.extend(server_ips(&m.data));
    match diag::plan(consent(&m.data), f.kind, diag::off()) {
        Plan::Drop => diag::mark_seen(&f.artifacts),
        Plan::AskConsent => {
            m.diag.current = Some(f);
            push(m, Popup::ask(T_CONSENT1.into(), BoxTag::Diag(Step::Consent1)));
        }
        Plan::Ask { .. } => {
            let p = ask_box(&f);
            m.diag.current = Some(f);
            m.diag.sent = false;
            push(m, p);
        }
        Plan::Send { offer_test } => {
            diag::send(f.clone());
            if offer_test {
                m.diag.current = Some(f);
                m.diag.sent = true;
                let text = format!("{} {T_SENT_TEST}", m.diag.current.as_ref().map_or("", |f| f.kind.what_happened()));
                push(m, Popup::choice(text, BoxTag::Diag(Step::AlwaysTest), &[T_TEST, T_OK]));
            }
        }
    }
}

/// Every menu frame: the context for the reports, the counters (stall, GPU loss), the outcome lines, the test of
/// the open box, and the next problem when no box is up.
pub fn tick(m: &mut Menu) {
    // a menu of a test or a tool (no `diag::configure`) records and shows nothing
    if !diag::armed() {
        return;
    }
    adopt(m);
    let due = m
        .diag
        .last_context
        .is_none_or(|t| t.elapsed() >= CONTEXT_EVERY);
    if due {
        m.diag.last_context = Some(Instant::now());
        diag::note_context(settings_json(&m.data), &m.data.user, m.data.auth, server_ips(&m.data));
        diag::poll_counters();
    }
    for line in diag::take_notices() {
        m.data.say_in(line, NOTE_COLOUR);
        m.dirty = true;
    }
    let view = diag::probe_view();
    if view.generation != m.diag.seen_generation {
        m.diag.seen_generation = view.generation;
        m.dirty = true;
    }
    // the lobby chat's test
    if let Some((gen, send)) = m.diag.chat_probe {
        if !view.running && view.generation > gen {
            m.diag.chat_probe = None;
            for r in &view.results {
                m.data.say_in(format!("{}: {}", r.target.name, r.words()), NOTE_COLOUR);
            }
            if send {
                let mut f = Failure::new(Kind::NetTest, "connection test from the lobby chat");
                f.lobby = m.data.auth;
                f.nettest = diag::last_nettest();
                f.keep_ips = server_ips(&m.data);
                diag::send(f);
            }
        }
    }
    // the box's test ended: its result
    if let Some(gen) = m.diag.testing {
        if !view.running && view.generation > gen {
            m.diag.testing = None;
            m.popups.retain(|p| p.tag != BoxTag::Diag(Step::Testing));
            let words = view
                .results
                .first()
                .map_or_else(|| "The test could not run.".to_string(), probe::Report::words);
            if m.diag.sent {
                // consent always: the result goes as a report of its own
                if let Some(f) = m.diag.current.take() {
                    let mut t = Failure::new(Kind::NetTest, format!("connection test after {}", f.kind.name()));
                    t.lobby = f.lobby;
                    t.keep_ips = f.keep_ips.clone();
                    t.nettest = diag::last_nettest();
                    diag::send(t);
                }
                push(m, Popup::ok(words, BoxTag::None));
            } else {
                if let Some(f) = m.diag.current.as_mut() {
                    f.nettest = diag::last_nettest();
                }
                push(
                    m,
                    Popup::choice(format!("{words} {T_RESULT_ASK}"), BoxTag::Diag(Step::Result), &[T_SEND, T_NO]),
                );
            }
        }
    }
    if !m.popups.is_empty() || m.diag.current.is_some() || m.fade.is_some() {
        return;
    }
    if let Some(note) = m.diag.bug_note.take() {
        m.diag.bug_note = Some(note);
        push(m, Popup::ask(T_BUG.into(), BoxTag::Diag(Step::BugAttach)));
        return;
    }
    if let Some(f) = diag::take_pending() {
        handle(m, f);
    }
}

/// The answer of one of our boxes (`chosen`: the button, `None` for Escape).
pub fn answer(m: &mut Menu, step: Step, chosen: Option<usize>, three: bool) {
    let first = chosen == Some(0);
    match step {
        Step::Consent1 => {
            if first {
                set_consent(m, Consent::Always);
                if let Some(f) = m.diag.current.take() {
                    handle(m, f);
                }
            } else {
                push(m, Popup::ask(T_CONSENT2.into(), BoxTag::Diag(Step::Consent2)));
            }
        }
        Step::Consent2 => {
            set_consent(m, if first { Consent::Ask } else { Consent::Never });
            if let Some(f) = m.diag.current.take() {
                handle(m, f);
            }
        }
        Step::Ask => match chosen {
            Some(0) => {
                if let Some(f) = m.diag.current.take() {
                    diag::send(f);
                }
            }
            Some(1) if three => start_box_test(m),
            _ => {
                if let Some(f) = m.diag.current.take() {
                    diag::mark_seen(&f.artifacts);
                }
            }
        },
        Step::AlwaysTest => {
            if first {
                start_box_test(m);
            } else {
                m.diag.current = None;
            }
        }
        Step::Result => {
            let f = m.diag.current.take();
            match (first, f) {
                (true, Some(f)) => diag::send(f),
                (false, Some(f)) => diag::mark_seen(&f.artifacts),
                _ => {}
            }
        }
        Step::BugAttach => {
            let note = m.diag.bug_note.take();
            if first {
                let mut f = Failure::new(Kind::BugReport, "bug report from the in-game report form");
                f.note = note;
                f.lobby = m.data.auth;
                f.keep_ips = server_ips(&m.data);
                diag::send(f);
            }
        }
        Step::Testing => {}
    }
}

/// The after-failure box's test: the wait box until the result.
fn start_box_test(m: &mut Menu) {
    let Some(t) = m.diag.current.as_ref().and_then(|f| failure_target(m, f)) else {
        push(m, Popup::ok("No server address to test.".into(), BoxTag::None));
        m.diag.current = None;
        return;
    };
    let gen = diag::probe_view().generation;
    if diag::start_probe(vec![t]) {
        m.diag.testing = Some(gen);
        let mut w = Popup::wait(T_TESTING.into());
        w.tag = BoxTag::Diag(Step::Testing);
        push(m, w);
    } else {
        push(m, Popup::ok("A connection test is already running.".into(), BoxTag::None));
        m.diag.current = None;
    }
}

/// A match the menu started ended: a problem when it did not end normally.
pub fn session_ended(m: &mut Menu, end: crate::loading::SessionEnd, link: Value) {
    use crate::loading::SessionEnd;
    let (kind, summary) = match end {
        SessionEnd::Played => return,
        SessionEnd::JoinFailed => (Kind::JoinFailed, "the join failed or timed out".to_string()),
        SessionEnd::Dropped => (Kind::LinkLost, "the match link was lost".to_string()),
        SessionEnd::ServerBye(code) => (Kind::ServerGone, format!("the game server closed the session (bye code {code})")),
    };
    let mut f = Failure::new(kind, summary);
    f.match_info = Some(match_json(&m.data, &format!("{end:?}")));
    if let Some(ip) = link
        .get("game_server")
        .and_then(|g| g.as_str())
        .and_then(|g| g.parse::<SocketAddr>().ok())
    {
        f.keep_ips.push(ip.ip());
    }
    f.link = Some(link);
    f.lobby = m.data.auth;
    diag::failure(f);
}

/// The lobby chat's own commands: `/report [text]`, `/diag [always|ask|never]`, `/nettest [send]`. `true`: the
/// line was one of them (it is not sent as chat).
pub fn lobby_line(line: &str, ctx: &mut Ctx) -> bool {
    let mut words = line.split_whitespace();
    let Some(head) = words.next().map(str::to_ascii_lowercase) else {
        return false;
    };
    let rest = line.trim_start()[head.len()..].trim().to_string();
    match head.as_str() {
        "/report" => {
            let mut f = Failure::new(Kind::Manual, "sent by hand with /report");
            f.note = (!rest.is_empty()).then_some(rest);
            f.lobby = ctx.data.auth;
            f.keep_ips = server_ips(ctx.data);
            ctx.data.say_in("Sending a diagnostic report...".into(), NOTE_COLOUR);
            diag::send(f);
            true
        }
        "/diag" => {
            match Consent::parse(&rest) {
                Some(c) => {
                    ctx.data.settings.diag_consent = c.as_u8();
                    ctx.data.settings_changed = true;
                    store_settings(ctx.data, ctx.cat);
                    ctx.data.say_in(format!("Diagnostic reports: {}.", c.name()), NOTE_COLOUR);
                }
                None => {
                    let c = consent(ctx.data);
                    ctx.data.say_in(
                        format!("Diagnostic reports: {}. Change with /diag always, /diag ask or /diag never.", c.name()),
                        NOTE_COLOUR,
                    );
                }
            }
            true
        }
        "/nettest" => {
            let t = unique(&targets(ctx.data));
            let gen = diag::probe_view().generation;
            if t.is_empty() {
                ctx.data.say_in("No server address to test.".into(), NOTE_COLOUR);
            } else if diag::start_probe(t) {
                ctx.data.say_in("Testing your connection...".into(), NOTE_COLOUR);
                ctx.data.diag_chat_probe = Some((gen, rest.eq_ignore_ascii_case("send")));
            } else {
                ctx.data.say_in("A connection test is already running.".into(), NOTE_COLOUR);
            }
            true
        }
        _ => false,
    }
}

/// What the screens left for the menu's state: the lobby chat's test (the chat has only the data at hand), the bug
/// report form's text.
fn adopt(m: &mut Menu) {
    if let Some(p) = m.data.diag_chat_probe.take() {
        m.diag.chat_probe = Some(p);
    }
    if let Some(n) = diag::take_bug_report() {
        m.diag.bug_note = Some(n);
    }
}

// ------------------------------------------------------------------------------------------------ server screen

/// The connection test panel of the server screen: the lower right panel of the 2009 page, empty in 2009 (our own
/// title, the per-row results, the Test button).
// OBSERVED: the server select page of the user's ROM at 1024 x 768 (the panel under Event / Notice holds no control)
pub const PANEL: [i32; 4] = [594, 486, 384, 190];
pub const CMD_TEST: i32 = 110_201;
pub const T_PANEL_TITLE: &str = "Connection Test";
pub const T_PANEL_NOTE: &str = "Checks the network between you and each server: delay, loss, large packets.";
const ROWS: usize = 3;

/// The Test button (bottom right of the panel).
pub fn test_button() -> super::modbrowser::Buttons {
    use super::modbrowser::{Button, ButtonKind, Buttons, BUTTON};
    let mut b = Buttons::default();
    b.list = vec![Button {
        cmd: CMD_TEST,
        at: [PANEL[0] + PANEL[2] - BUTTON[0] - 4, PANEL[1] + PANEL[3] - BUTTON[1] - 4],
        size: BUTTON,
        label: "Test".into(),
        on: true,
        kind: ButtonKind::Art,
    }];
    b
}

/// Starts the test of every row (the button).
pub fn test_all(data: &MenuData) -> bool {
    diag::start_probe(unique(&targets(data)))
}

/// One row of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelRow {
    pub name: String,
    /// The verdict and its numbers.
    pub short: String,
    /// The verdict in plain words (empty before a result).
    pub words: String,
    pub colour: u32,
}

/// The panel's rows: the server's name and the result (colour by verdict).
pub fn panel_rows(data: &MenuData, view: &diag::ProbeView) -> Vec<PanelRow> {
    targets(data)
        .into_iter()
        .map(|(name, t)| {
            match view.results.iter().find(|r| r.target.addr == t.addr) {
                Some(r) => {
                    let colour = match r.verdict() {
                        probe::Verdict::Good => 0xff7c_d67c,
                        probe::Verdict::Far | probe::Verdict::Unstable => 0xffe7_ae54,
                        _ => 0xffff_6a5a,
                    };
                    PanelRow { name, short: r.measured.short(r.target.lobby), words: r.words(), colour }
                }
                None => PanelRow {
                    name,
                    short: if view.running { "testing..." } else { "not tested" }.into(),
                    words: String::new(),
                    colour: if view.running { 0xffb0_b0b0 } else { 0xff90_9090 },
                },
            }
        })
        .collect()
}

/// One line of the panel's text, placed.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelLine {
    pub text: String,
    pub at: [i32; 2],
    /// The width the line may take (its text is cut to fit it).
    pub width: i32,
    pub colour: u32,
}

/// Where the panel puts its lines: all inside [`PANEL`], none under the Test button.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PanelLayout {
    pub lines: Vec<PanelLine>,
    /// The highlight behind the selected row.
    pub highlight: Option<[i32; 4]>,
    /// The index of the first line of the verdict's words (`lines.len()` without one).
    pub verdict_from: usize,
}

/// Greedy wrap where line `i` may take `width(i)` pixels; a word wider than its line is cut with "...".
fn wrap_varying(
    cat: &mut super::catalog::Catalog,
    s: &str,
    width: impl Fn(usize) -> i32,
) -> Vec<String> {
    use super::usebox::measure;
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        let w = width(out.len());
        let wider = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if line.is_empty() || measure(cat, &wider) <= w {
            line = wider;
        } else {
            out.push(std::mem::take(&mut line));
            line = word.to_string();
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// The panel's lines for `rows`: the title, the note (only while it leaves room for the verdict), the rows (from
/// the selected one on when it would not show) and the plain words of the selected row (else the first tested one)
/// wrapped to the panel and, beside the Test button, to what the button leaves. The words may take every line the
/// panel has left; a text that still does not fit ends in "..." on the last line, so nothing leaves the panel.
// DC-DEFAULT: CLI-1843 (proposed) the verdict wraps within the panel and never ends below it: the note gives way first
pub fn panel_layout(
    cat: &mut super::catalog::Catalog,
    rows: &[PanelRow],
    selected: Option<usize>,
) -> PanelLayout {
    use super::modbrowser::fit;
    use super::usebox::measure;
    let [x, y, w, h] = PANEL;
    let bottom = y + h;
    let line = super::usebox::FONT_SIZE + 4;
    let btn_top = bottom - 4 - super::modbrowser::BUTTON[1] - 2;
    let bw = super::modbrowser::BUTTON[0] + 12;
    let mut lay = PanelLayout::default();
    let put = |lay: &mut PanelLayout, text: String, at: [i32; 2], width: i32, colour: u32| {
        lay.lines.push(PanelLine { text, at, width, colour });
    };
    put(&mut lay, T_PANEL_TITLE.into(), [x, y], w, super::usebox::TITLE_COLOUR);
    let top = y + line + 2;
    let note: Vec<String> = wrap_varying(cat, T_PANEL_NOTE, |_| w).into_iter().take(2).collect();
    let shown = rows.len().min(ROWS);
    let words = selected
        .and_then(|s| rows.get(s))
        .filter(|r| !r.words.is_empty())
        .or_else(|| rows.iter().find(|r| !r.words.is_empty()));
    // the note first; the verdict needs the room when it is long
    let mut note_lines = note.len();
    let (word_lines, words_y) = loop {
        let note_h = if note_lines > 0 { note_lines as i32 * line + 4 } else { 0 };
        let rows_end = top + note_h + shown as i32 * line;
        let wy = rows_end + 4;
        let Some(r) = words else { break (Vec::new(), wy) };
        let lines = wrap_varying(cat, &r.words, |i| if wy + (i as i32 + 1) * line > btn_top { w - bw } else { w });
        if wy + lines.len() as i32 * line <= bottom || note_lines == 0 {
            break (lines, wy);
        }
        note_lines = 0;
    };
    let mut ly = top;
    if note_lines > 0 {
        for l in note.iter().take(note_lines) {
            put(&mut lay, l.clone(), [x, ly], w, super::usebox::MESSAGE_COLOUR);
            ly += line;
        }
        ly += 4;
    }
    let first = selected.filter(|s| *s >= ROWS).map_or(0, |s| s + 1 - ROWS);
    for (i, r) in rows.iter().enumerate().skip(first).take(ROWS) {
        if selected == Some(i) {
            lay.highlight = Some([x - 4, ly - 1, w + 8, line]);
        }
        let label = fit(cat, &r.name, w / 3);
        put(&mut lay, label, [x, ly], w / 3, super::usebox::MESSAGE_COLOUR);
        let short = fit(cat, &r.short, w - w / 3 - 8);
        put(&mut lay, short, [x + w / 3 + 8, ly], w - w / 3 - 8, r.colour);
        ly += line;
    }
    lay.verdict_from = lay.lines.len();
    if let Some(r) = words {
        let width_of = |wy: i32| if wy + line > btn_top { w - bw } else { w };
        let room = ((bottom - words_y) / line).max(1) as usize;
        let mut lines = word_lines;
        if lines.len() > room {
            // the rest of the text joins the last line that fits and is cut there
            let rest = lines.split_off(room - 1).join(" ");
            let at = words_y + (room as i32 - 1) * line;
            lines.push(fit(cat, &rest, width_of(at)));
        }
        let mut wy = words_y;
        for l in lines {
            let width = width_of(wy);
            let l = if measure(cat, &l) > width { fit(cat, &l, width) } else { l };
            put(&mut lay, l, [x, wy], width, r.colour);
            wy += line;
        }
    }
    lay
}

/// Draws the panel.
pub fn draw_panel(
    cat: &mut super::catalog::Catalog,
    data: &MenuData,
    selected: Option<usize>,
    button: &super::modbrowser::Buttons,
    out: &mut Vec<crate::hud::items::DrawItem>,
) {
    let view = diag::probe_view();
    let rows = panel_rows(data, &view);
    let lay = panel_layout(cat, &rows, selected);
    if let Some([hx, hy, hw, hh]) = lay.highlight {
        out.push(super::usebox::fill([hx, hy], [hw, hh], [70, 60, 30, 200]));
    }
    for l in &lay.lines {
        out.push(super::modbrowser::text(&l.text, l.at, l.colour));
    }
    button.draw(cat, out);
}

/// Whether a popup is ours with three buttons.
pub fn is_three(p: &Popup) -> bool {
    p.kind == BoxKind::YesNo && p.own_captions.as_ref().is_some_and(|c| c.len() == 3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_botclient::lobby::ServerEntry;

    fn entry(id: u32, ip: &str) -> ServerEntry {
        ServerEntry {
            id,
            name: format!("Server {id}"),
            ip: ip.into(),
            port: 10081,
            open: 1,
            capacity: 100,
            current: 1,
            clan_only: 0,
            op_capacity: 0,
            op_current: 0,
        }
    }

    #[test]
    fn the_test_targets_are_the_lobbys_echo_once_per_host() {
        let mut d = MenuData::default();
        assert!(targets(&d).is_empty());
        d.auth = Some("198.51.100.7:10000".parse().unwrap());
        let t = targets(&d);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].1.addr, "198.51.100.7:18080".parse().unwrap());
        d.servers = vec![entry(1, "198.51.100.7"), entry(2, "0.0.0.0"), entry(3, "81.2.69.160")];
        let t = targets(&d);
        assert_eq!(t.len(), 3);
        assert!(t[0].1.lobby && t[1].1.lobby && !t[2].1.lobby);
        assert_eq!(t[1].1.addr, "198.51.100.7:18080".parse().unwrap());
        assert_eq!(t[2].1.addr, "81.2.69.160:18080".parse().unwrap());
        assert_eq!(unique(&t).len(), 2);
        let rows = panel_rows(&d, &diag::ProbeView::default());
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].short, "not tested");
        assert!(rows[0].words.is_empty());
    }

    #[test]
    fn the_panel_and_its_button_stay_inside_the_empty_lower_right_panel() {
        let b = test_button();
        let r = super::super::modbrowser::button_rects(&b)[0];
        assert!(r[0] >= PANEL[0] && r[0] + r[2] <= PANEL[0] + PANEL[2]);
        assert!(r[1] >= PANEL[1] && r[1] + r[3] <= PANEL[1] + PANEL[3]);
        // the Event / Notice panel above (and its Mods button) ends before it
        let mods = super::super::modbrowser::button_rects(&super::super::moddedrows::mods_button())[0];
        assert!(mods[1] + mods[3] < PANEL[1]);
    }

    /// Layout look (not a gate): `DIAG_SHOT_DIR=<dir>` with `CA_ROM` writes the server screen with the test panel,
    /// the after-failure box and the result box as PNGs at 800 x 600, 1024 x 768 and 1920 x 1080. Ignored by default (it waits for a probe that nobody answers).
    #[test]
    #[ignore]
    fn shots_of_the_panel_and_the_box() {
        let (Some(rom), Ok(out)) = (dc_rom::test_rom(), std::env::var("DIAG_SHOT_DIR")) else {
            return;
        };
        let mut shots = Vec::new();
        for (width, height) in [(800u32, 600u32), (1024, 768), (1920, 1080)] {
            let mut m = Menu::new(super::super::MenuOptions {
                auth: "127.0.0.1:1".parse().unwrap(),
                user: "tester".into(),
                link: super::super::link::LinkOptions::new("diag-shot"),
                rom: Some(rom.clone()),
                font: Some((crate::testkit::synth_font(), 0)),
                width,
                height,
            });
            m.detach_link();
            m.data.auth = Some("127.0.0.1:1".parse().unwrap());
            m.data.servers = vec![entry(1, "127.0.0.1"), entry(2, "127.0.0.1"), entry(3, "81.2.69.160")];
            m.data.server = m.data.servers.first().cloned();
            m.open(super::super::Go::Servers);
            assert!(test_all(&m.data));
            while diag::probe_view().running {
                std::thread::sleep(Duration::from_millis(100));
            }
            let save = |m: &mut Menu, name: &str| {
                let img = m.image();
                image::RgbaImage::from_raw(img.width, img.height, img.rgba)
                    .unwrap()
                    .save(format!("{out}/{name}_{width}x{height}.png"))
                    .unwrap();
            };
            save(&mut m, "diag_servers_panel");
            let f = Failure::new(Kind::LinkLost, "x");
            m.popups.push(ask_box(&f));
            m.dirty = true;
            save(&mut m, "diag_ask_box");
            m.popups.clear();
            let words = diag::probe_view().results.first().map(probe::Report::words).unwrap_or_default();
            m.popups.push(Popup::choice(format!("{words} {T_RESULT_ASK}"), BoxTag::Diag(Step::Result), &[T_SEND, T_NO]));
            m.dirty = true;
            save(&mut m, "diag_result_box");
            shots.push((width, height));
        }
        assert_eq!(shots.len(), 3);
    }

    #[test]
    fn our_three_button_box_lays_out_inside_the_message_box() {
        let p = Popup::choice("x".into(), BoxTag::Diag(Step::Ask), &[T_SEND, T_TEST, T_NO]);
        assert!(is_three(&p));
        let b = p.buttons();
        assert_eq!(b.len(), 3);
        let bx = super::super::popup::BOX;
        for r in &b {
            assert!(r[0] >= bx[0] && r[0] + r[2] <= bx[0] + bx[2]);
        }
        assert!(b[0][0] + b[0][2] < b[1][0] && b[1][0] + b[1][2] < b[2][0]);
        let two = Popup::choice("x".into(), BoxTag::Diag(Step::Ask), &[T_SEND, T_NO]);
        assert_eq!(two.buttons().len(), 2);
    }
}
