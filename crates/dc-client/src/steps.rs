//! Step files (`--steps FILE.jsonl [--steps-strict]`): a scenario as one JSON object per line, run inside the
//! process in the window and in headless runs. The steps put their input into the client's own input path (the
//! menu's [`MenuInput`]s, the game's [`DeviceInput`]s); the operating system's cursor, keyboard and focus are never
//! touched, so a window run may sit behind other windows without focus (`--no-activate`).
//!
//! A line: `{"do": VERB, ...arguments, ...schedule, "id": NAME, "timeout": S}`; empty lines and lines starting with
//! `#` or `//` are skipped. The steps run in order, one at a time; a step starts when all its schedule keys hold:
//! - `"at": S`: S seconds after the run started;
//! - `"after": S`: S seconds after the previous step finished (default [`DEFAULT_GAP_S`] when no `after` is given);
//! - `"on_screen": NAME`: the screen is NAME (`login`, `servers`, `channels`, `lobby`, `create`, `room`, `result`,
//!   `game`);
//! - `"on_event": NAME` or `{"ev": NAME, field: value, ...}`: such an event came after the previous step finished;
//! - `"on_room_ready": N`: the waiting room has N members or more and every other member is ready.
//!
//! `"optional": true` skips a step whose schedule never holds (a `step_skip` event) instead of failing it;
//! `"handled": true` on a click fails the step when the click changed nothing (a dead button).
//!
//! A step that waits longer than its `timeout` (default [`DEFAULT_TIMEOUT_S`] s) fails; a failed step writes a
//! `step_fail` event and the run goes on with the next step (a scenario lists every dead button, not the first).
//!
//! Verbs (menu verbs wait for a menu screen, game verbs for the match):
//! - `wait_screen {screen}`; `click {widget | x, y}`; `hover {widget | x, y}`; `type {text}` (in the match: into
//!   the chat box, as the keys would); `key {key, hold_ms}`
//!   (menu: enter escape tab backtab backspace space up down; game: w a s d space ctrl shift alt z q 1-5 (class keys) r enter
//!   escape backspace f10; in the match enter / escape / backspace also go to the chat box, so `key enter`, `type
//!   {text}`, `key enter` sends a chat line or a local command such as `/network`);
//! - `look {yaw, pitch}` (degrees, exact); `fire {count, hold_ms, gap_ms}`; `slot {n}` (1-4); `reload`;
//!   `throw {hold_ms}`; `sprint {ms}` (forward + sprint); `crouch {ms}`; `jump`;
//! - `shot {name}`; `burst {name, count, gap_ms}`; `mark {label}`;
//! - `expect {screen}` or `expect {event, match: {...}, min: {...}, within, since}` (the event after the previous
//!   step started, or any event of the run with `"since": "run"`; `min` compares numbers with `>=`);
//! - `dismiss` (closes the open box, the messenger window or the clan screen, if any);
//! - `leave` (ends the match window, back to the menu); `quit` (ends the run).
//!
//! Widgets: `SCREEN.NAME` ([`alias_command`], our own names for the ROM layout's buttons), `cmd:N` (the visible button
//! with command N), `part:N` (the screen sheet's part N), `ctl:LAYER/ID` (a control of the layout record),
//! `popup.ok` / `popup.yes` / `popup.no`. A click goes to the widget's centre as pointer move, press and release.
//! STRICT (`--steps-strict`): a widget without a rectangle, or one the hit-test at its centre does not find (another
//! control, or an open box, on top), fails the step; a strict run never sends a screen command directly
//! ([`MenuInput::Command`]). Without strict a widget without a rectangle is clicked by its command.
//!
//! Every click writes a `click` event: `handled` is true when the click changed anything (the screen, a box, a lobby
//! request, the drawn screen apart from the button's own hover / press look); a dead button is `handled: false`.
// DC-DEFAULT: CLI-GATE-T2-2 (staged) the scenario step files (gate T2), see reports/gate-steps_DC_DEFAULTS.md

use crate::events;
use crate::input::{Action, DeviceInput, InputSource, KeyboardMouse, ShellKey};
use crate::look::{LookParams, ViewAngles};
use crate::menu::{Menu, MenuInput, MenuKey};
use dc_sim::Commands;
use dc_ui_data::WidgetKind;
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// Seconds between two steps when a step names no `after`.
pub const DEFAULT_GAP_S: f64 = 0.25;
/// Seconds a step may wait for its schedule or its screen.
pub const DEFAULT_TIMEOUT_S: f64 = 30.0;

/// Where a pointer verb goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Widget(String),
    Point([f32; 2]),
}

/// What an `expect` checks.
#[derive(Debug, Clone, PartialEq)]
pub enum Expect {
    Screen(String),
    Event {
        ev: String,
        fields: Map<String, Value>,
        min: Map<String, Value>,
        /// Look at every event of the run (`"since": "run"`), not only those after the previous step started.
        from_run: bool,
    },
}

/// One verb with its arguments.
#[derive(Debug, Clone, PartialEq)]
pub enum Verb {
    WaitScreen(String),
    Click(Target),
    Hover(Target),
    Type(String),
    Key {
        key: String,
        hold_ms: u64,
    },
    Look {
        yaw: f32,
        pitch: f32,
    },
    Fire {
        count: u32,
        hold_ms: u64,
        gap_ms: u64,
    },
    Slot(u8),
    Reload,
    Throw {
        hold_ms: u64,
    },
    Sprint {
        ms: u64,
    },
    Crouch {
        ms: u64,
    },
    Jump,
    Shot(String),
    Burst {
        name: String,
        count: u32,
        gap_ms: u64,
    },
    Mark(String),
    Expect(Expect),
    /// Closes the open box (Escape to the box) when one is open; nothing otherwise.
    Dismiss,
    /// lobby20: a right press at a point (the messenger's friend row menu).
    Rclick([f32; 2]),
    Leave,
    Quit,
}

impl Verb {
    pub fn name(&self) -> &'static str {
        match self {
            Verb::WaitScreen(_) => "wait_screen",
            Verb::Click(_) => "click",
            Verb::Hover(_) => "hover",
            Verb::Type(_) => "type",
            Verb::Key { .. } => "key",
            Verb::Look { .. } => "look",
            Verb::Fire { .. } => "fire",
            Verb::Slot(_) => "slot",
            Verb::Reload => "reload",
            Verb::Throw { .. } => "throw",
            Verb::Sprint { .. } => "sprint",
            Verb::Crouch { .. } => "crouch",
            Verb::Jump => "jump",
            Verb::Shot(_) => "shot",
            Verb::Burst { .. } => "burst",
            Verb::Mark(_) => "mark",
            Verb::Expect(_) => "expect",
            Verb::Dismiss => "dismiss",
            Verb::Rclick(_) => "rclick",
            Verb::Leave => "leave",
            Verb::Quit => "quit",
        }
    }

    /// Where the verb runs.
    fn place(&self) -> Place {
        match self {
            Verb::Click(_) | Verb::Hover(_) | Verb::Dismiss | Verb::Rclick(_) => Place::Menu,
            // `type` runs on a menu screen (its text box) and in the match (the chat box)
            Verb::Look { .. }
            | Verb::Fire { .. }
            | Verb::Slot(_)
            | Verb::Reload
            | Verb::Throw { .. }
            | Verb::Sprint { .. }
            | Verb::Crouch { .. }
            | Verb::Jump
            | Verb::Leave => Place::Game,
            _ => Place::Any,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Menu,
    Game,
    Any,
}

/// When a step starts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct When {
    pub at: Option<f64>,
    pub after: f64,
    pub on_screen: Option<String>,
    pub on_event: Option<(String, Map<String, Value>)>,
    /// `"on_room_ready": N`: the waiting room has N members or more and every other member is ready.
    pub room_ready: Option<usize>,
}

/// One line of a step file.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub line: usize,
    pub id: String,
    pub verb: Verb,
    pub when: When,
    pub timeout: f64,
    /// `"handled": true` on a click: the click fails the step when it changed nothing (a dead button).
    pub require_handled: bool,
    /// `"optional": true`: a schedule that never holds skips the step (`step_skip`) instead of failing it.
    pub optional: bool,
}

fn num(o: &Map<String, Value>, k: &str) -> Option<f64> {
    o.get(k).and_then(Value::as_f64)
}

fn text(o: &Map<String, Value>, k: &str) -> Option<String> {
    o.get(k).and_then(Value::as_str).map(str::to_string)
}

fn need_text(o: &Map<String, Value>, k: &str, verb: &str) -> Result<String, String> {
    text(o, k).ok_or_else(|| format!("{verb} needs \"{k}\""))
}

fn ms(o: &Map<String, Value>, k: &str, default: u64) -> u64 {
    num(o, k).map_or(default, |v| v.max(0.0) as u64)
}

fn target(o: &Map<String, Value>, verb: &str) -> Result<Target, String> {
    if let Some(w) = text(o, "widget") {
        return Ok(Target::Widget(w));
    }
    match (num(o, "x"), num(o, "y")) {
        (Some(x), Some(y)) => Ok(Target::Point([x as f32, y as f32])),
        _ => Err(format!("{verb} needs \"widget\" or \"x\" and \"y\"")),
    }
}

fn event_match(v: &Value) -> Result<(String, Map<String, Value>), String> {
    match v {
        Value::String(s) => Ok((s.clone(), Map::new())),
        Value::Object(o) => {
            let ev = text(o, "ev").ok_or("on_event object needs \"ev\"")?;
            let mut m = o.clone();
            m.remove("ev");
            Ok((ev, m))
        }
        _ => Err("on_event: a name or an object".into()),
    }
}

/// Parses one step line (already a JSON object).
pub fn parse_step(line: usize, o: &Map<String, Value>) -> Result<Step, String> {
    let verb_name = text(o, "do").ok_or("no \"do\"")?;
    let v = verb_name.as_str();
    let verb = match v {
        "wait_screen" => Verb::WaitScreen(need_text(o, "screen", v)?),
        "click" => Verb::Click(target(o, v)?),
        "rclick" => match target(o, v)? {
            Target::Point(p) => Verb::Rclick(p),
            _ => return Err("rclick needs x and y".into()),
        },
        "hover" => Verb::Hover(target(o, v)?),
        "type" => Verb::Type(need_text(o, "text", v)?),
        "key" => Verb::Key {
            key: need_text(o, "key", v)?.to_ascii_lowercase(),
            hold_ms: ms(o, "hold_ms", 100),
        },
        "look" => Verb::Look {
            yaw: num(o, "yaw").ok_or("look needs \"yaw\"")? as f32,
            pitch: num(o, "pitch").unwrap_or(0.0) as f32,
        },
        "fire" => Verb::Fire {
            count: num(o, "count").unwrap_or(1.0).max(1.0) as u32,
            hold_ms: ms(o, "hold_ms", 60),
            gap_ms: ms(o, "gap_ms", 150),
        },
        "slot" => {
            let n = num(o, "n").ok_or("slot needs \"n\" (1-4)")?;
            if !(1.0..=4.0).contains(&n) {
                return Err("slot: n is 1..4".into());
            }
            Verb::Slot(n as u8)
        }
        "reload" => Verb::Reload,
        "throw" => Verb::Throw {
            hold_ms: ms(o, "hold_ms", 600),
        },
        "sprint" => Verb::Sprint {
            ms: ms(o, "ms", 1500),
        },
        "crouch" => Verb::Crouch {
            ms: ms(o, "ms", 1000),
        },
        "jump" => Verb::Jump,
        "shot" => Verb::Shot(need_text(o, "name", v)?),
        "burst" => Verb::Burst {
            name: need_text(o, "name", v)?,
            count: num(o, "count").unwrap_or(10.0).clamp(1.0, 200.0) as u32,
            gap_ms: ms(o, "gap_ms", 50),
        },
        "mark" => Verb::Mark(text(o, "label").unwrap_or_default()),
        "expect" => {
            if let Some(s) = text(o, "screen") {
                Verb::Expect(Expect::Screen(s))
            } else {
                let ev = text(o, "event").ok_or("expect needs \"screen\" or \"event\"")?;
                let obj = |k: &str| -> Result<Map<String, Value>, String> {
                    match o.get(k) {
                        None => Ok(Map::new()),
                        Some(Value::Object(m)) => Ok(m.clone()),
                        Some(_) => Err(format!("expect: \"{k}\" is an object")),
                    }
                };
                Verb::Expect(Expect::Event {
                    ev,
                    fields: obj("match")?,
                    min: obj("min")?,
                    from_run: text(o, "since").as_deref() == Some("run"),
                })
            }
        }
        "dismiss" => Verb::Dismiss,
        "leave" => Verb::Leave,
        "quit" => Verb::Quit,
        other => return Err(format!("unknown verb {other:?}")),
    };
    let on_event = match o.get("on_event") {
        Some(e) => Some(event_match(e)?),
        None => None,
    };
    // an expect with `within` waits that long; others wait for their schedule up to `timeout`
    let timeout = num(o, "timeout")
        .or_else(|| num(o, "within"))
        .unwrap_or(match &verb {
            Verb::Expect(Expect::Screen(_)) => 5.0,
            Verb::Expect(_) => 2.0,
            _ => DEFAULT_TIMEOUT_S,
        });
    Ok(Step {
        line,
        id: text(o, "id").unwrap_or_else(|| format!("L{line}")),
        when: When {
            at: num(o, "at"),
            after: num(o, "after").unwrap_or(DEFAULT_GAP_S).max(0.0),
            on_screen: text(o, "on_screen"),
            on_event,
            room_ready: num(o, "on_room_ready").map(|n| n.max(1.0) as usize),
        },
        require_handled: o.get("handled") == Some(&Value::Bool(true)),
        optional: o.get("optional") == Some(&Value::Bool(true)),
        verb,
        timeout,
    })
}

/// Parses a step file. Errors name the line.
pub fn parse(src: &str) -> Result<Vec<Step>, String> {
    let mut out = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let n = i + 1;
        let t = raw.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with("//") {
            continue;
        }
        let v: Value = serde_json::from_str(t).map_err(|e| format!("line {n}: {e}"))?;
        let o = v
            .as_object()
            .ok_or_else(|| format!("line {n}: not an object"))?;
        out.push(parse_step(n, o).map_err(|e| format!("line {n}: {e}"))?);
    }
    Ok(out)
}

// ----------------------------------------------------------------------------------------------- widgets

/// The top bar every front-end screen after the login shares.
const BAR: &[(&str, i32)] = &[
    ("option", 301),
    ("settings", 301),
    ("report", 627),
    ("help", 302),
    ("exit_game", 303),
];
/// The lobby pages' upper and lower bars (lobby, room creation, waiting room).
const LOBBY_BAR: &[(&str, i32)] = &[
    ("market", 622),
    ("shop", 61),
    ("modification", 184),
    ("clan", 168),
    ("messenger", 310),
    ("inbox", 619),
    ("chat_send", 117),
];

/// The command of our widget name `name` on screen `screen` (the button commands of the user's ROM layout pages).
// OBSERVED: the ROM layout pages of the user's ROM as dc-client's sheets build them (the gate-steps widget dump,
// report reports/gate-steps.md); the screens' own command constants
pub fn alias_command(screen: &str, name: &str) -> Option<i32> {
    use crate::menu::{channels, create, lobby, login, room, servers, shop, tuning};
    let own: &[(&str, i32)] = match screen {
        "login" => &[
            ("ok", login::CMD_LOGIN),
            ("login", login::CMD_LOGIN),
            ("quit", login::CMD_QUIT),
            ("remember", login::CMD_REMEMBER),
            ("register", login::CMD_REGISTER),
            ("find_id", 10000),
            ("find_pw", 10001),
            ("web_a", 10002),
            ("web_b", 10003),
        ],
        "servers" => &[
            ("refresh", servers::CMD_RELOAD_LIST),
            ("enter", servers::CMD_ENTER),
            ("messenger", servers::CMD_FRIENDS_PANEL),
        ],
        // the channel list has no separate back button in the ROM page: its way back is the server-list button
        "channels" => &[
            ("server_list", channels::CMD_SERVER_LIST),
            ("back", channels::CMD_SERVER_LIST),
            ("refresh", channels::CMD_RELOAD_LIST),
            ("enter", channels::CMD_ENTER),
        ],
        "lobby" => &[
            ("enter", lobby::CMD_ENTER),
            ("quick_start", lobby::CMD_QUICK),
            ("create", lobby::CMD_CREATE),
            ("change_channel", lobby::CMD_CHANGE_CHANNEL),
            ("change_server", lobby::CMD_EXIT),
            ("exit", lobby::CMD_EXIT),
        ],
        "create" => &[
            ("ok", create::CMD_OK),
            ("cancel", create::CMD_CANCEL),
            ("lock", create::CMD_LOCK_CHECK),
            ("more_players", create::CMD_MORE_PLAYERS),
            ("fewer_players", create::CMD_FEWER_PLAYERS),
            ("mode_menu", create::CMD_MODE_LIST),
        ],
        "room" => &[
            ("start", room::CMD_START),
            ("ready", room::CMD_READY),
            ("exit", room::CMD_EXIT),
            ("team", room::CMD_TEAM),
            ("invite", 30001),
            ("password", 20),
            ("map", room::CMD_MAP),
            ("mode", room::CMD_MODE),
            ("goal", room::CMD_GOAL),
            ("players", room::CMD_PLAYERS),
            ("arms", 600),
        ],
        "result" => &[("exit", crate::matchexit::RESULT_CMD_EXIT)],
        // w3-shop
        "shop" => &[
            ("exit", shop::CMD_EXIT),
            ("weapons", shop::CMD_TAB_WEAPONS),
            ("body", shop::CMD_TAB_BODY),
            ("market_tab", shop::CMD_TAB_MARKET),
            ("inbox_tab", shop::CMD_TAB_INBOX),
            ("buy", shop::CMD_BUY),
            ("equip", shop::CMD_WEAR),
            ("page_back", shop::CMD_PAGE_BACK),
            ("page_next", shop::CMD_PAGE_FWD),
        ],
        "tuning" => &[("exit", shop::CMD_EXIT), ("buy", tuning::CMD_BUY)],
        // w3-clan: the clan screen's own buttons
        "clan" => &[
            ("exit", crate::menu::clan::CMD_EXIT),
            ("back", crate::menu::clan::CMD_EXIT),
            ("manage", crate::menu::clan::CMD_MANAGE),
            ("new_clan", crate::menu::clan::CMD_CREATE),
            ("find", crate::menu::clan::CMD_FIND),
            ("war", crate::menu::clan::CMD_WAR),
            ("quit_clan", crate::menu::clan::CMD_LEAVE),
            ("notice", crate::menu::clan::CMD_NOTICE),
            ("invite_option", crate::menu::clan::CMD_INVITE_OPTION),
        ],
        _ => &[],
    };
    let bars: &[&[(&str, i32)]] = match screen {
        "servers" | "channels" => &[BAR],
        "lobby" | "create" | "room" | "shop" | "tuning" | "clan" => &[BAR, LOBBY_BAR],
        _ => &[],
    };
    own.iter()
        .chain(bars.iter().flat_map(|b| b.iter()))
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
}

/// Our name of sheet part `i` on the current screen: the alias of its command, `cmd:N`, or `ctl:LAYER/ID`.
pub fn widget_name(menu: &Menu, i: usize) -> String {
    let screen = menu.screen_name();
    let Some(p) = menu.sheet().parts.get(i) else {
        return format!("part:{i}");
    };
    if p.command != 0 {
        for name in ALIAS_NAMES {
            if alias_command(screen, name) == Some(p.command) {
                return format!("{screen}.{name}");
            }
        }
        return format!("cmd:{}", p.command);
    }
    format!("ctl:{}/{}", p.layer, p.id)
}

/// Every alias name (for [`widget_name`]'s reverse look-up; the first one of a command wins).
const ALIAS_NAMES: &[&str] = &[
    "ok",
    "login",
    "quit",
    "remember",
    "find_id",
    "find_pw",
    "web_a",
    "web_b",
    "refresh",
    "enter",
    "server_list",
    "quick_start",
    "create",
    "change_channel",
    "change_server",
    "exit",
    "cancel",
    "lock",
    "more_players",
    "fewer_players",
    "mode_menu",
    "start",
    "ready",
    "team",
    "invite",
    "password",
    "map",
    "mode",
    "goal",
    "players",
    "arms",
    "option",
    "report",
    "help",
    "exit_game",
    "market",
    "shop",
    "modification",
    "clan",
    "messenger",
    "inbox",
    "chat_send",
    "weapons",
    "body",
    "market_tab",
    "inbox_tab",
    "buy",
    "equip",
    "page_back",
    "page_next",
    "back",
    "manage",
    "new_clan",
    "find",
    "war",
    "quit_clan",
    "notice",
    "invite_option",
];

/// Where a widget is.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// Sheet part `part` (its centre `at`).
    Part {
        part: usize,
        at: [f32; 2],
        command: i32,
        enabled: bool,
    },
    /// Button `n` of the open box.
    Popup { n: usize, at: [f32; 2] },
    /// The screen has no visible control for the widget (`command`: the command it stands for, when known).
    NoRect { command: Option<i32>, why: String },
    /// The widget belongs to another screen (the step waits for it).
    OtherScreen(String),
}

fn centre(r: [i32; 4]) -> [f32; 2] {
    [
        r[0] as f32 + r[2] as f32 / 2.0,
        r[1] as f32 + r[3] as f32 / 2.0,
    ]
}

fn find_part(menu: &Menu, pick: impl Fn(&crate::menu::sheet::Part) -> bool) -> Option<usize> {
    let s = menu.sheet();
    (0..s.parts.len()).find(|&i| pick(&s.parts[i]) && s.visible(i))
}

/// Resolves a widget name on the current screen.
pub fn resolve(menu: &Menu, name: &str) -> Resolved {
    let screen = menu.screen_name();
    if let Some(b) = name.strip_prefix("popup.") {
        let n = match b {
            "ok" | "yes" => 0,
            "no" | "cancel" => 1,
            _ => {
                return Resolved::NoRect {
                    command: None,
                    why: format!("unknown box button {b:?}"),
                }
            }
        };
        return match menu.popup().and_then(|p| p.buttons().get(n).copied()) {
            Some(r) => Resolved::Popup { n, at: centre(r) },
            None => Resolved::NoRect {
                command: None,
                why: "no box with that button is open".into(),
            },
        };
    }
    let by_part = |i: Option<usize>, command: Option<i32>, what: String| match i {
        Some(i) => {
            let p = &menu.sheet().parts[i];
            Resolved::Part {
                part: i,
                at: centre(p.rect()),
                command: p.command,
                enabled: p.enabled,
            }
        }
        None => Resolved::NoRect {
            command,
            why: format!("no visible control for {what} on {screen}"),
        },
    };
    if let Some(n) = name.strip_prefix("cmd:") {
        let Ok(c) = n.parse::<i32>() else {
            return Resolved::NoRect {
                command: None,
                why: format!("bad command {n:?}"),
            };
        };
        let i = find_part(menu, |p| {
            matches!(p.kind, WidgetKind::Button | WidgetKind::CheckButton) && p.command == c
        });
        return by_part(i, Some(c), format!("command {c}"));
    }
    if let Some(n) = name.strip_prefix("part:") {
        let i = n
            .parse::<usize>()
            .ok()
            .filter(|i| *i < menu.sheet().parts.len() && menu.sheet().visible(*i));
        return by_part(i, None, format!("part {n}"));
    }
    if let Some(n) = name.strip_prefix("ctl:") {
        let mut it = n.split('/');
        let layer = it.next().and_then(|x| x.parse::<i32>().ok());
        let id = it.next().and_then(|x| x.parse::<i32>().ok());
        let (Some(layer), Some(id)) = (layer, id) else {
            return Resolved::NoRect {
                command: None,
                why: format!("ctl:LAYER/ID, got {n:?}"),
            };
        };
        let i = find_part(menu, |p| {
            p.layer == layer
                && p.id == id
                && matches!(
                    p.kind,
                    WidgetKind::Button | WidgetKind::CheckButton | WidgetKind::EditBox
                )
        });
        return by_part(i, None, format!("control {layer}/{id}"));
    }
    let Some((scr, alias)) = name.split_once('.') else {
        return Resolved::NoRect {
            command: None,
            why: format!("widget {name:?}: SCREEN.NAME, cmd:N, part:N, ctl:L/ID or popup.X"),
        };
    };
    if scr != screen {
        return Resolved::OtherScreen(scr.to_string());
    }
    match (scr, alias) {
        ("login", "id") | ("login", "password") => {
            let id = if alias == "id" { 9 } else { 10 };
            let i = find_part(menu, |p| p.kind == WidgetKind::EditBox && p.id == id);
            return by_part(i, None, format!("the {alias} box"));
        }
        _ => {}
    }
    match alias_command(scr, alias) {
        Some(c) => {
            let i = find_part(menu, |p| {
                matches!(p.kind, WidgetKind::Button | WidgetKind::CheckButton) && p.command == c
            });
            by_part(i, Some(c), format!("{name} (command {c})"))
        }
        None => Resolved::NoRect {
            command: None,
            why: format!("unknown widget {name:?}"),
        },
    }
}

// ----------------------------------------------------------------------------------------------- the click probe

/// What a click may change, before and after it.
#[derive(Debug, Clone, PartialEq)]
struct Probe {
    screen: &'static str,
    boxes: Vec<String>,
    sent: u64,
    outcome: bool,
    quit: bool,
    drawn: u64,
    windows: Vec<usize>,
    sounds: u64,
}

fn probe(menu: &mut Menu) -> Probe {
    // the drawn cursor follows the pointer: a pointer move is not a click's effect
    let mut items = menu.items_at(0.0);
    items.retain(|i| !crate::menu::cursor::is_cursor(i));
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{items:?}").hash(&mut h);
    let s = menu.sheet();
    let windows = (0..s.parts.len())
        .filter(|&i| s.parts[i].kind == WidgetKind::Window && s.visible(i))
        .collect();
    Probe {
        screen: menu.screen_name(),
        boxes: menu.popup().map(|p| p.text.clone()).into_iter().collect(),
        sent: menu.link_sent(),
        outcome: menu.has_outcome(),
        quit: menu.quitting(),
        drawn: h.finish(),
        windows,
        sounds: events::sound_starts().0,
    }
}

/// The click event's body from the states before and after a click (and the dropdown events of the windows it
/// opened).
fn click_body(menu: &Menu, before: &Probe, after: &Probe, cmd: i32) -> (Value, bool) {
    let mut effects = Vec::new();
    if before.screen != after.screen {
        effects.push("screen");
    }
    if before.boxes != after.boxes {
        effects.push("box");
    }
    if before.sent != after.sent {
        effects.push("lobby_request");
    }
    if before.outcome != after.outcome {
        effects.push("match");
    }
    if before.quit != after.quit {
        effects.push("quit");
    }
    if before.drawn != after.drawn {
        effects.push("draw");
    }
    let handled = !effects.is_empty();
    let sound = (after.sounds != before.sounds).then(|| events::sound_starts().1);
    if before.screen == after.screen {
        let s = menu.sheet();
        for &w in after.windows.iter().filter(|w| !before.windows.contains(w)) {
            let items = (0..s.parts.len())
                .filter(|&i| {
                    s.parts[i].parent == Some(w)
                        && s.visible(i)
                        && matches!(
                            s.parts[i].kind,
                            WidgetKind::Button | WidgetKind::CheckButton
                        )
                })
                .count();
            events::emit(
                "dropdown",
                json!({ "layer": s.parts[w].layer, "items": items, "open": true }),
            );
        }
    }
    let handler = handled.then(|| format!("{}:{cmd}", before.screen));
    (
        json!({ "handled": handled, "handler": handler, "effects": effects, "sound": sound.flatten(),
                "screen": before.screen, "screen_after": after.screen }),
        handled,
    )
}

fn merge(a: Value, b: Value) -> Value {
    let mut m = a.as_object().cloned().unwrap_or_default();
    if let Value::Object(o) = b {
        m.extend(o);
    }
    Value::Object(m)
}

/// Clicks at `p` (pointer move, press, release) and writes the click event; returns whether it was handled.
pub fn click_at(menu: &mut Menu, p: [f32; 2], widget: &str, enabled: bool) -> bool {
    menu.input(MenuInput::Move(p));
    let cmd = menu
        .sheet()
        .hit(p)
        .map_or(0, |i| menu.sheet().parts[i].command);
    let before = probe(menu);
    menu.input(MenuInput::Press(p));
    menu.input(MenuInput::Release(p));
    let after = probe(menu);
    let (body, handled) = click_body(menu, &before, &after, cmd);
    events::emit(
        "click",
        merge(
            json!({ "widget": widget, "x": p[0], "y": p[1], "enabled": enabled, "via": "pointer" }),
            body,
        ),
    );
    handled
}

/// Watches the user's own clicks in a window run (the press's state, compared at the release).
#[derive(Debug, Default)]
pub struct ClickWatch {
    pressed: Option<(Probe, [f32; 2], String, i32)>,
}

impl ClickWatch {
    /// Feeds one pointer input of the user to the menu and writes its click / hover event.
    pub fn input(&mut self, menu: &mut Menu, ev: MenuInput) {
        if !events::on() {
            menu.input(ev);
            return;
        }
        match ev {
            MenuInput::Press(p) => {
                let hit = menu.sheet().hit(p);
                let name = hit.map_or_else(|| "none".into(), |i| widget_name(menu, i));
                let cmd = hit.map_or(0, |i| menu.sheet().parts[i].command);
                self.pressed = Some((probe(menu), p, name, cmd));
                menu.input(ev);
            }
            MenuInput::Release(p) => {
                menu.input(ev);
                if let Some((before, at, name, cmd)) = self.pressed.take() {
                    let after = probe(menu);
                    let (body, _) = click_body(menu, &before, &after, cmd);
                    events::emit(
                        "click",
                        merge(
                            json!({ "widget": name, "x": at[0], "y": at[1], "release": [p[0], p[1]],
                                    "via": "user" }),
                            body,
                        ),
                    );
                }
            }
            MenuInput::Move(p) => {
                let before = menu.sheet().hit(p);
                menu.input(ev);
                hover_event(menu, p, before);
            }
            other => menu.input(other),
        }
    }
}

thread_local! {
    static LAST_HOVER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn hover_event(menu: &Menu, p: [f32; 2], hit: Option<usize>) {
    if LAST_HOVER.with(|c| c.replace(hit)) == hit {
        return;
    }
    let widget = hit.map(|i| widget_name(menu, i));
    events::emit(
        "hover",
        json!({ "widget": widget, "x": p[0], "y": p[1], "screen": menu.screen_name() }),
    );
}

// ----------------------------------------------------------------------------------------------- the runner

/// Where the runner is called from this frame.
pub enum Here<'a> {
    Menu(&'a mut Menu),
    Game {
        view: &'a mut ViewAngles,
        out: &'a mut Vec<DeviceInput>,
    },
}

impl Here<'_> {
    fn screen(&self) -> String {
        match self {
            Here::Menu(m) => m.screen_name().to_string(),
            Here::Game { .. } => "game".into(),
        }
    }
}

/// An input or a shot due later (key holds, fire sequences, bursts).
#[derive(Debug, Clone, PartialEq)]
enum Timed {
    Device(DeviceInput),
    Shot(String),
}

enum Outcome {
    Done,
    Wait(String),
    Fail(String),
}

/// The step runner.
pub struct StepRunner {
    steps: Vec<Step>,
    strict: bool,
    next: usize,
    started: Instant,
    /// When the current step began waiting.
    waiting_since: Option<Instant>,
    prev_done: Instant,
    /// The event sequence when the previous step finished (`on_event` looks after it) and started (`expect`).
    prev_seq: u64,
    prev_start_seq: u64,
    /// The event sequence when the current step's schedule first held.
    cur_start_seq: Option<u64>,
    timed: VecDeque<(Instant, Timed)>,
    shots: VecDeque<String>,
    /// Chat-box presses of the game verbs (`type`, `key enter` ...), taken by the match window each frame.
    chat: VecDeque<crate::hud::chat::ChatPress>,
    quit: bool,
    leave: bool,
    pub ok: usize,
    pub failed: Vec<String>,
    finished: bool,
}

impl StepRunner {
    pub fn new(steps: Vec<Step>, strict: bool) -> StepRunner {
        let now = Instant::now();
        StepRunner {
            steps,
            strict,
            next: 0,
            started: now,
            waiting_since: None,
            prev_done: now,
            prev_seq: 0,
            prev_start_seq: 0,
            cur_start_seq: None,
            timed: VecDeque::new(),
            shots: VecDeque::new(),
            chat: VecDeque::new(),
            quit: false,
            leave: false,
            ok: 0,
            failed: Vec::new(),
            finished: false,
        }
    }

    pub fn done(&self) -> bool {
        self.next >= self.steps.len() && self.timed.is_empty()
    }

    fn schedule(&mut self, at: Instant, t: Timed) {
        let i = self.timed.iter().position(|(w, _)| *w > at);
        match i {
            Some(i) => self.timed.insert(i, (at, t)),
            None => self.timed.push_back((at, t)),
        }
    }

    /// One frame of the runner.
    pub fn poll(&mut self, here: &mut Here) {
        let now = Instant::now();
        let had_timed = !self.timed.is_empty();
        while let Some((t, _)) = self.timed.front() {
            if *t > now {
                break;
            }
            let Some((_, x)) = self.timed.pop_front() else {
                break;
            };
            match (x, &mut *here) {
                (Timed::Shot(n), _) => self.shots.push_back(n),
                (Timed::Device(d), Here::Game { out, .. }) => out.push(d),
                // a hold released after the match ended: nothing holds it any more
                (Timed::Device(_), Here::Menu(_)) => {}
            }
        }
        if !self.timed.is_empty() {
            return;
        }
        if had_timed {
            self.prev_done = now;
            self.prev_seq = events::last_seq();
        }
        if self.next >= self.steps.len() {
            if !self.finished {
                self.finished = true;
                events::emit(
                    "steps_done",
                    json!({ "ok": self.ok, "failed": self.failed.len() }),
                );
            }
            return;
        }
        let step = self.steps[self.next].clone();
        let since = *self.waiting_since.get_or_insert(now);
        let waited = now.duration_since(since).as_secs_f64();
        if let Some(why) = self.not_yet(&step, here, now) {
            if waited > step.timeout && step.optional {
                events::emit(
                    "step_skip",
                    json!({ "id": step.id, "line": step.line, "verb": step.verb.name(), "why": why }),
                );
                self.finish_skip();
            } else if waited > step.timeout {
                self.finish(
                    &step,
                    Err(format!("timeout after {:.1} s: {why}", step.timeout)),
                );
            }
            return;
        }
        self.cur_start_seq.get_or_insert_with(events::last_seq);
        match self.run(&step, here, now) {
            Outcome::Done => self.finish(&step, Ok(())),
            Outcome::Fail(why) => self.finish(&step, Err(why)),
            Outcome::Wait(why) => {
                if waited > step.timeout {
                    self.finish(
                        &step,
                        Err(format!("timeout after {:.1} s: {why}", step.timeout)),
                    );
                }
            }
        }
    }

    /// Why the step's schedule does not hold yet (`None`: it holds).
    fn not_yet(&self, s: &Step, here: &Here, now: Instant) -> Option<String> {
        if let Some(at) = s.when.at {
            if now.duration_since(self.started).as_secs_f64() < at {
                return Some(format!("at {at} s"));
            }
        }
        if now.duration_since(self.prev_done).as_secs_f64() < s.when.after {
            return Some(format!("after {} s", s.when.after));
        }
        let screen = here.screen();
        if let Some(want) = &s.when.on_screen {
            if *want != screen {
                return Some(format!("screen {want} (now {screen})"));
            }
        }
        if let Some((ev, fields)) = &s.when.on_event {
            if !events::since(self.prev_seq)
                .iter()
                .any(|e| e.matches(ev, fields))
            {
                return Some(format!("event {ev}"));
            }
        }
        if let Some(n) = s.when.room_ready {
            match here {
                Here::Menu(m) if room_ready(m, n) => {}
                _ => return Some(format!("{n} members in the waiting room, the others ready")),
            }
        }
        match (s.verb.place(), here) {
            (Place::Menu, Here::Game { .. }) => Some("a menu screen (the match runs)".into()),
            (Place::Game, Here::Menu(_)) => Some(format!("the match (screen {screen})")),
            _ => None,
        }
    }

    fn finish_skip(&mut self) {
        self.next += 1;
        self.waiting_since = None;
        self.prev_start_seq = self.cur_start_seq.take().unwrap_or_else(events::last_seq);
        self.prev_done = Instant::now();
        self.prev_seq = events::last_seq();
    }

    fn finish(&mut self, s: &Step, r: Result<(), String>) {
        let screen = events::current_screen();
        match r {
            Ok(()) => {
                self.ok += 1;
                events::emit(
                    "step",
                    json!({ "id": s.id, "line": s.line, "verb": s.verb.name(), "status": "ok" }),
                );
            }
            Err(why) => {
                println!(
                    "dc-client steps: step {} (line {}) {} failed: {why}",
                    s.id,
                    s.line,
                    s.verb.name()
                );
                self.failed
                    .push(format!("{} {}: {why}", s.id, s.verb.name()));
                events::emit(
                    "step_fail",
                    json!({ "id": s.id, "line": s.line, "verb": s.verb.name(), "why": why, "screen": screen,
                            "widget": match &s.verb { Verb::Click(Target::Widget(w)) | Verb::Hover(Target::Widget(w)) => Some(w.clone()), _ => None } }),
                );
            }
        }
        self.next += 1;
        self.waiting_since = None;
        self.prev_start_seq = self.cur_start_seq.take().unwrap_or_else(events::last_seq);
        if self.timed.is_empty() {
            self.prev_done = Instant::now();
            self.prev_seq = events::last_seq();
        }
    }

    fn run(&mut self, s: &Step, here: &mut Here, now: Instant) -> Outcome {
        match &s.verb {
            Verb::WaitScreen(want) => {
                let screen = here.screen();
                if *want == screen {
                    Outcome::Done
                } else {
                    Outcome::Wait(format!("screen {want} (now {screen})"))
                }
            }
            Verb::Click(t) | Verb::Hover(t) => {
                let Here::Menu(menu) = here else {
                    return Outcome::Wait("a menu screen".into());
                };
                let click = matches!(s.verb, Verb::Click(_));
                self.pointer(menu, t, click, s.require_handled)
            }
            Verb::Rclick(p) => {
                let Here::Menu(menu) = here else {
                    return Outcome::Wait("a menu screen".into());
                };
                menu.input(MenuInput::Move(*p));
                menu.input(MenuInput::PressAlt(*p));
                Outcome::Done
            }
            Verb::Dismiss => {
                // the open box, the messenger window or the clan screen (w3-clan), or (w3-shop) the shop or tuning screen a lobby bar button opened (Escape = its Exit)
                if let Here::Menu(menu) = here {
                    if menu.dismissable() || matches!(menu.screen_name(), "shop" | "tuning") {
                        menu.input(MenuInput::Key(MenuKey::Escape));
                    }
                    // w3-options: an open options / report / help box closes too
                    if menu.overlay_open() {
                        menu.input(MenuInput::Key(MenuKey::Escape));
                    }
                }
                Outcome::Done
            }
            Verb::Type(text) => {
                match here {
                    Here::Menu(menu) => {
                        for c in text.chars() {
                            menu.input(MenuInput::Char(c));
                        }
                    }
                    // the match: the text goes to the chat box one key at a time, as typed keys would
                    Here::Game { .. } => {
                        for c in text.chars() {
                            self.chat
                                .push_back(crate::hud::chat::ChatPress::Text(c.to_string()));
                        }
                    }
                }
                Outcome::Done
            }
            Verb::Key { key, hold_ms } => match here {
                Here::Menu(menu) => match menu_key_named(key) {
                    Some(k) => {
                        menu.input(MenuInput::Key(k));
                        Outcome::Done
                    }
                    None => Outcome::Fail(format!("no menu key {key:?}")),
                },
                Here::Game { out, .. } => match game_key_named(key) {
                    // the chat box sees these as the window's key handler would show them to it
                    Some(GameKey::Chat(p)) => {
                        self.chat.push_back(p);
                        Outcome::Done
                    }
                    Some(GameKey::Hold(a)) => {
                        out.push(DeviceInput::Action(a, true));
                        self.schedule(
                            now + Duration::from_millis(*hold_ms),
                            Timed::Device(DeviceInput::Action(a, false)),
                        );
                        Outcome::Done
                    }
                    Some(GameKey::Press(d)) => {
                        out.extend(d);
                        Outcome::Done
                    }
                    None => Outcome::Fail(format!("no game key {key:?}")),
                },
            },
            Verb::Look { yaw, pitch } => {
                if let Here::Game { view, .. } = here {
                    view.yaw = yaw.to_radians();
                    view.pitch = pitch.to_radians();
                }
                Outcome::Done
            }
            Verb::Fire {
                count,
                hold_ms,
                gap_ms,
            } => {
                let mut t = now;
                for _ in 0..*count {
                    self.schedule(t, Timed::Device(DeviceInput::Fire(true)));
                    t += Duration::from_millis(*hold_ms);
                    self.schedule(t, Timed::Device(DeviceInput::Fire(false)));
                    t += Duration::from_millis(*gap_ms);
                }
                self.flush_now(here, now);
                Outcome::Done
            }
            Verb::Throw { hold_ms } => {
                self.schedule(now, Timed::Device(DeviceInput::Fire(true)));
                self.schedule(
                    now + Duration::from_millis(*hold_ms),
                    Timed::Device(DeviceInput::Fire(false)),
                );
                self.flush_now(here, now);
                Outcome::Done
            }
            Verb::Slot(n) => {
                if let Here::Game { out, .. } = here {
                    out.push(DeviceInput::Slot(n - 1));
                }
                Outcome::Done
            }
            Verb::Reload => {
                if let Here::Game { out, .. } = here {
                    out.push(DeviceInput::Reload);
                }
                Outcome::Done
            }
            Verb::Sprint { ms } => {
                self.hold(here, now, &[Action::Forward, Action::Sprint], *ms);
                Outcome::Done
            }
            Verb::Crouch { ms } => {
                self.hold(here, now, &[Action::Crouch], *ms);
                Outcome::Done
            }
            Verb::Jump => {
                self.hold(here, now, &[Action::Jump], 100);
                Outcome::Done
            }
            Verb::Shot(name) => {
                self.shots.push_back(name.clone());
                Outcome::Done
            }
            Verb::Burst {
                name,
                count,
                gap_ms,
            } => {
                for k in 0..*count {
                    self.schedule(
                        now + Duration::from_millis(*gap_ms * k as u64),
                        Timed::Shot(format!("{name}_{k:02}")),
                    );
                }
                self.flush_now(here, now);
                Outcome::Done
            }
            Verb::Mark(label) => {
                events::emit("mark", json!({ "label": label, "id": s.id }));
                Outcome::Done
            }
            Verb::Expect(Expect::Screen(want)) => {
                let screen = here.screen();
                if *want == screen {
                    Outcome::Done
                } else {
                    Outcome::Wait(format!("expected screen {want}, the screen is {screen}"))
                }
            }
            Verb::Expect(Expect::Event {
                ev,
                fields,
                min,
                from_run,
            }) => {
                let seen = events::since(if *from_run { 0 } else { self.prev_start_seq });
                let same: Vec<&events::Event> = seen.iter().filter(|e| e.ev == *ev).collect();
                if same
                    .iter()
                    .any(|e| e.matches(ev, fields) && meets_min(e, min))
                {
                    return Outcome::Done;
                }
                let last = same
                    .last()
                    .map(|e| Value::Object(e.body.clone()).to_string());
                let why = match last {
                    Some(b) => format!(
                        "expected {ev} {} {}; last {ev}: {b}",
                        Value::Object(fields.clone()),
                        Value::Object(min.clone())
                    ),
                    None => format!("expected {ev} {}; none seen", Value::Object(fields.clone())),
                };
                // a click's event is written by the click itself: no reason to wait for a better one
                if ev == "click" && !same.is_empty() {
                    return Outcome::Fail(why);
                }
                Outcome::Wait(why)
            }
            Verb::Leave => {
                self.leave = true;
                Outcome::Done
            }
            Verb::Quit => {
                self.quit = true;
                Outcome::Done
            }
        }
    }

    /// The inputs of a just scheduled sequence that are due now (fire's first press, a burst's first shot).
    fn flush_now(&mut self, here: &mut Here, now: Instant) {
        while self.timed.front().is_some_and(|(t, _)| *t <= now) {
            let Some((_, x)) = self.timed.pop_front() else {
                break;
            };
            match (x, &mut *here) {
                (Timed::Shot(n), _) => self.shots.push_back(n),
                (Timed::Device(d), Here::Game { out, .. }) => out.push(d),
                (Timed::Device(_), Here::Menu(_)) => {}
            }
        }
    }

    fn hold(&mut self, here: &mut Here, now: Instant, actions: &[Action], ms: u64) {
        if let Here::Game { out, .. } = here {
            for a in actions {
                out.push(DeviceInput::Action(*a, true));
                self.schedule(
                    now + Duration::from_millis(ms),
                    Timed::Device(DeviceInput::Action(*a, false)),
                );
            }
        }
    }

    /// A click or a hover on a menu widget or point.
    fn pointer(&mut self, menu: &mut Menu, t: &Target, click: bool, require: bool) -> Outcome {
        let (at, label, enabled) = match t {
            Target::Point(p) => {
                let hit = menu.sheet().hit(*p);
                let name = hit.map_or_else(|| "none".into(), |i| widget_name(menu, i));
                (*p, name, hit.is_none_or(|i| menu.sheet().parts[i].enabled))
            }
            Target::Widget(w) => match resolve(menu, w) {
                Resolved::OtherScreen(s) => {
                    return Outcome::Wait(format!(
                        "screen {s} for {w} (now {})",
                        menu.screen_name()
                    ))
                }
                Resolved::NoRect { command, why } => {
                    if self.strict || command.is_none() || !click {
                        return Outcome::Fail(format!("{w}: {why}"));
                    }
                    // not strict: the command as the button would send it
                    let c = command.unwrap_or_default();
                    let before = probe(menu);
                    menu.input(MenuInput::Command(c));
                    let after = probe(menu);
                    let (body, handled) = click_body(menu, &before, &after, c);
                    events::emit(
                        "click",
                        merge(json!({ "widget": w, "via": "command", "command": c }), body),
                    );
                    if require && !handled {
                        return Outcome::Fail(format!(
                            "{w}: the command changed nothing (dead button)"
                        ));
                    }
                    return Outcome::Done;
                }
                Resolved::Popup { at, .. } => (at, w.clone(), true),
                Resolved::Part {
                    part, at, enabled, ..
                } => {
                    if menu.popup().is_some() {
                        let why = format!("{w}: a box is open over the screen");
                        if self.strict {
                            return Outcome::Fail(why);
                        }
                    }
                    let hit = menu.sheet().hit(at);
                    if hit != Some(part) && self.strict {
                        let other = hit.map_or_else(|| "nothing".into(), |i| widget_name(menu, i));
                        return Outcome::Fail(format!(
                            "{w}: the hit-test at ({:.0}, {:.0}) finds {other}",
                            at[0], at[1]
                        ));
                    }
                    (at, w.clone(), enabled)
                }
            },
        };
        if click {
            let handled = click_at(menu, at, &label, enabled);
            if require && !handled {
                return Outcome::Fail(format!(
                    "{label}: the click changed nothing (dead button{})",
                    if enabled { "" } else { ", drawn disabled" }
                ));
            }
        } else {
            let before = menu.sheet().hit(at);
            menu.input(MenuInput::Move(at));
            LAST_HOVER.with(|c| c.set(None));
            hover_event(menu, at, before.or(menu.sheet().hit(at)));
        }
        Outcome::Done
    }

    /// The next shot to save (its name).
    pub fn take_shot(&mut self) -> Option<String> {
        self.shots.pop_front()
    }
}

/// The waiting room has `n` members or more and every member but the own is ready.
fn room_ready(m: &Menu, n: usize) -> bool {
    let d = &m.data;
    let me = d.profile.my_id;
    m.screen_name() == "room"
        && d.members.len() >= n
        && d.members
            .iter()
            .filter(|(id, _)| **id != me)
            .all(|(_, x)| x.state == 1)
}

thread_local! {
    static LAST_ROOM: std::cell::RefCell<Option<(usize, usize, bool)>> = const { std::cell::RefCell::new(None) };
}

/// A `room` event when the waiting room's member count, ready count or host changed.
fn room_event(m: &Menu) {
    if m.screen_name() != "room" {
        LAST_ROOM.with(|c| c.replace(None));
        return;
    }
    let d = &m.data;
    let ready = d.members.values().filter(|x| x.state == 1).count();
    let now = (d.members.len(), ready, d.host());
    if LAST_ROOM.with(|c| c.replace(Some(now))) != Some(now) {
        events::emit(
            "room",
            json!({ "members": now.0, "ready": now.1, "host": now.2,
                    "others_ready": room_ready(m, 1) }),
        );
    }
}

fn meets_min(e: &events::Event, min: &Map<String, Value>) -> bool {
    min.iter().all(
        |(k, v)| match (e.body.get(k).and_then(Value::as_f64), v.as_f64()) {
            (Some(a), Some(b)) => a >= b,
            _ => false,
        },
    )
}

fn menu_key_named(k: &str) -> Option<MenuKey> {
    Some(match k {
        "enter" | "return" => MenuKey::Enter,
        "escape" | "esc" => MenuKey::Escape,
        "tab" => MenuKey::Tab,
        "backtab" | "shift+tab" => MenuKey::BackTab,
        "backspace" => MenuKey::Backspace,
        "space" => MenuKey::Space,
        "up" => MenuKey::Up,
        "down" => MenuKey::Down,
        "f2" => MenuKey::F2,
        "f3" => MenuKey::F3,
        _ => return None,
    })
}

enum GameKey {
    Hold(Action),
    Press(Vec<DeviceInput>),
    /// A key the match's chat box takes (Enter opens or sends, Escape closes, Backspace edits).
    Chat(crate::hud::chat::ChatPress),
}

/// The game keys by name (the window's key map: [`crate::window`]'s `action_for` / weapon / shell keys).
fn game_key_named(k: &str) -> Option<GameKey> {
    use GameKey::*;
    Some(match k {
        "w" | "forward" => Hold(Action::Forward),
        "s" | "back" => Hold(Action::Back),
        "a" | "left" => Hold(Action::Left),
        "d" | "right" => Hold(Action::Right),
        "space" | "jump" => Hold(Action::Jump),
        "ctrl" | "c" | "crouch" => Hold(Action::Crouch),
        "shift" | "sprint" => Hold(Action::Sprint),
        "alt" | "walk" => Hold(Action::Walk),
        "capslock" => Hold(Action::RunLock),
        "q" => Hold(Action::StrafeMod),
        "z" | "zoom" => Hold(Action::Zoom),
        // the number keys are class keys (cshell+0x22e530); the `slot` verb still names a slot
        "1" | "2" | "3" | "4" | "5" => Press(vec![DeviceInput::Shell(ShellKey::ClassKey(
            k.as_bytes()[0] - b'1',
        ))]),
        "r" | "reload" => Press(vec![DeviceInput::Reload]),
        "e" | "use" => Press(vec![DeviceInput::Use]),
        "enter" => Chat(crate::hud::chat::ChatPress::Enter),
        "escape" => Chat(crate::hud::chat::ChatPress::Escape),
        "backspace" => Chat(crate::hud::chat::ChatPress::Backspace),
        "f10" => Press(vec![DeviceInput::Shell(ShellKey::LeaveBox)]),
        _ => return None,
    })
}

// ----------------------------------------------------------------------------------------------- process-wide

/// Settings of the run (`--shot-dir`, `--no-activate`).
#[derive(Debug, Clone, Default)]
pub struct StepConfig {
    pub shot_dir: Option<PathBuf>,
    pub no_activate: bool,
}

static CONFIG: OnceLock<StepConfig> = OnceLock::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static RUNNER: Mutex<Option<StepRunner>> = Mutex::new(None);

fn runner() -> MutexGuard<'static, Option<StepRunner>> {
    RUNNER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sets the run's settings (once; later calls are ignored).
pub fn configure(c: StepConfig) {
    let _ = CONFIG.set(c);
}

pub fn config() -> StepConfig {
    CONFIG.get().cloned().unwrap_or_default()
}

/// Installs the step runner of the run: the windows and the headless loops drive it from now on.
pub fn install(r: StepRunner) {
    *runner() = Some(r);
    ACTIVE.store(true, Ordering::Release);
}

/// A step file drives this run (no OS input is read, the cursor is never grabbed).
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// One menu frame: the screen event, then the steps (when a step file drives the run).
pub fn menu_frame(menu: &mut Menu) {
    events::screen(menu.screen_name());
    if events::on() {
        room_event(menu);
    }
    if !active() {
        return;
    }
    if let Some(r) = runner().as_mut() {
        r.poll(&mut Here::Menu(menu));
    }
}

/// One game frame of the steps: the view may be set, device inputs come back in `out`.
pub fn game_frame(view: &mut ViewAngles, out: &mut Vec<DeviceInput>) {
    if !active() {
        return;
    }
    if let Some(r) = runner().as_mut() {
        r.poll(&mut Here::Game { view, out });
    }
}

/// The chat-box presses the game verbs queued since the last call (the match window feeds them to its chat box).
pub fn take_chat() -> Vec<crate::hud::chat::ChatPress> {
    if !active() {
        return Vec::new();
    }
    runner()
        .as_mut()
        .map(|r| r.chat.drain(..).collect())
        .unwrap_or_default()
}

/// The next shot to save, when a step asked for one and a shot folder is set.
pub fn take_shot() -> Option<String> {
    if !active() {
        return None;
    }
    runner().as_mut().and_then(StepRunner::take_shot)
}

/// A step asked to end the run.
pub fn quit_requested() -> bool {
    active() && runner().as_ref().is_some_and(|r| r.quit)
}

/// A step asked to leave the match (taken once).
pub fn take_leave() -> bool {
    if !active() {
        return false;
    }
    runner()
        .as_mut()
        .is_some_and(|r| std::mem::take(&mut r.leave))
}

/// The game window or headless match should end now (quit or leave asked; a leave is taken).
pub fn end_game() -> bool {
    quit_requested() || take_leave()
}

/// `ok N, failed M` and the failed steps, once the run ended.
pub fn summary() -> Option<(usize, Vec<String>, bool)> {
    runner()
        .as_ref()
        .map(|r| (r.ok, r.failed.clone(), r.done()))
}

fn safe_name(n: &str) -> String {
    n.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Saves the shot `name` into the shot folder and writes its event (no folder: the event says so).
pub fn save_shot(name: &str, img: &dc_render::FrameImage) {
    let Some(dir) = config().shot_dir else {
        events::emit(
            "shot",
            json!({ "name": name, "path": null, "why": "no --shot-dir" }),
        );
        return;
    };
    let path = dir.join(format!("{}.png", safe_name(name)));
    let r = std::fs::create_dir_all(&dir)
        .map_err(|e| e.to_string())
        .and_then(|_| img.save_png(&path).map_err(|e| e.to_string()));
    match r {
        Ok(()) => events::emit(
            "shot",
            json!({ "name": name, "path": path.display().to_string(), "w": img.width, "h": img.height,
                    "screen": events::current_screen() }),
        ),
        Err(e) => events::emit("shot", json!({ "name": name, "path": null, "why": e })),
    };
}

/// A shot that could not be read (the event says why).
pub fn shot_failed(name: &str, why: &str) {
    events::emit("shot", json!({ "name": name, "path": null, "why": why }));
}

/// Reads a texture back (the window's swapchain image before it is presented): the copy, the map and the row
/// unpacking of the headless read-back ([`dc_render::unpack_rows`]).
pub fn read_texture(
    ctx: &dc_render::GpuContext,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> Result<dc_render::FrameImage, String> {
    let row = 4 * width;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("steps shot read-back"),
        size: padded as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("steps shot copy"),
        });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([enc.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = ctx.device.poll(wgpu::PollType::Wait);
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let data = slice.get_mapped_range();
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let rgba = dc_render::unpack_rows(&data, width, height, padded, bgra);
    drop(data);
    buffer.unmap();
    Ok(dc_render::FrameImage {
        width,
        height,
        rgba,
    })
}

/// The game input of a run a step file drives: the steps' device inputs and view through the keyboard / mouse
/// state machine (no OS device is read).
pub struct StepInput {
    inner: KeyboardMouse,
    out: Vec<DeviceInput>,
}

impl StepInput {
    pub fn new(look: LookParams) -> StepInput {
        StepInput {
            inner: KeyboardMouse::new(look),
            out: Vec::new(),
        }
    }
}

impl InputSource for StepInput {
    fn frame(&mut self, view: &mut ViewAngles) {
        game_frame(view, &mut self.out);
        for d in self.out.drain(..) {
            self.inner.device(d);
        }
        self.inner.frame(view);
    }

    fn tick(&mut self, tick: u64, hz: u32, view: &mut ViewAngles) -> Commands {
        self.inner.tick(tick, hz, view)
    }

    fn weapon(&mut self) -> crate::arms::WeaponCommands {
        self.inner.weapon()
    }

    fn shell_keys(&mut self) -> Vec<ShellKey> {
        self.inner.shell_keys()
    }

    fn set_zoom_level(&mut self, z: u32) {
        self.inner.set_zoom_level(z);
    }

    fn set_sensitivity(&mut self, s: f32) {
        self.inner.set_sensitivity(s);
    }

    fn label(&self) -> &'static str {
        "steps"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_file_parses_with_its_schedule_and_defaults() {
        let src = r#"
# login idle
{"do":"wait_screen","screen":"login","timeout":20}
{"do":"shot","name":"login","after":2}
// a click by widget and by point
{"do":"click","widget":"lobby.shop","on_screen":"lobby","id":"L8-shop"}
{"do":"click","x":512,"y":384,"at":3.5}
{"do":"expect","event":"click","match":{"handled":true}}
{"do":"expect","event":"dropdown","min":{"items":1},"within":3}
{"do":"burst","name":"enter","count":12,"gap_ms":40,"on_event":{"ev":"screen","name":"channels"}}
{"do":"fire","count":5}
{"do":"quit"}
"#;
        let s = parse(src).unwrap();
        assert_eq!(s.len(), 9);
        assert_eq!(s[0].verb, Verb::WaitScreen("login".into()));
        assert_eq!(s[0].timeout, 20.0);
        assert_eq!(s[0].when.after, DEFAULT_GAP_S);
        assert_eq!(s[1].when.after, 2.0);
        assert_eq!(s[2].id, "L8-shop");
        assert_eq!(s[2].when.on_screen.as_deref(), Some("lobby"));
        assert_eq!(s[3].verb, Verb::Click(Target::Point([512.0, 384.0])));
        assert_eq!(s[3].when.at, Some(3.5));
        assert_eq!(s[4].timeout, 2.0);
        assert_eq!(s[5].timeout, 3.0);
        let (ev, f) = s[6].when.on_event.clone().unwrap();
        assert_eq!(
            (ev.as_str(), f.get("name")),
            ("screen", Some(&json!("channels")))
        );
        assert_eq!(
            s[7].verb,
            Verb::Fire {
                count: 5,
                hold_ms: 60,
                gap_ms: 150
            }
        );
        assert_eq!(s[8].verb, Verb::Quit);
        assert_eq!(s[8].id, "L12");
    }

    #[test]
    fn bad_lines_name_their_line() {
        assert!(parse("{\"do\":\"fly\"}").unwrap_err().starts_with("line 1"));
        assert!(parse("\n{\"do\":\"click\"}")
            .unwrap_err()
            .starts_with("line 2"));
        assert!(parse("{\"do\":\"slot\",\"n\":7}").is_err());
        assert!(parse("[1]").is_err());
    }

    #[test]
    fn aliases_name_the_rom_commands_per_screen() {
        assert_eq!(alias_command("lobby", "shop"), Some(61));
        assert_eq!(alias_command("room", "shop"), Some(61));
        assert_eq!(alias_command("servers", "shop"), None);
        assert_eq!(alias_command("channels", "back"), Some(10001));
        assert_eq!(alias_command("lobby", "change_server"), Some(67));
        assert_eq!(alias_command("channels", "help"), Some(302));
        assert_eq!(alias_command("login", "help"), None);
    }

    #[test]
    fn game_steps_hold_and_release_on_time() {
        let steps = parse(
            "{\"do\":\"sprint\",\"ms\":0,\"after\":0}\n{\"do\":\"fire\",\"count\":2,\"hold_ms\":0,\"gap_ms\":0,\"after\":0}\n{\"do\":\"look\",\"yaw\":90,\"pitch\":-10,\"after\":0}\n{\"do\":\"leave\",\"after\":0}",
        )
        .unwrap();
        let mut r = StepRunner::new(steps, true);
        let mut view = ViewAngles::default();
        let mut got = Vec::new();
        for _ in 0..12 {
            let mut out = Vec::new();
            r.poll(&mut Here::Game {
                view: &mut view,
                out: &mut out,
            });
            got.extend(out);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            got,
            vec![
                DeviceInput::Action(Action::Forward, true),
                DeviceInput::Action(Action::Sprint, true),
                DeviceInput::Action(Action::Forward, false),
                DeviceInput::Action(Action::Sprint, false),
                DeviceInput::Fire(true),
                DeviceInput::Fire(false),
                DeviceInput::Fire(true),
                DeviceInput::Fire(false),
            ]
        );
        assert!((view.yaw - 90f32.to_radians()).abs() < 1e-6);
        assert!((view.pitch + 10f32.to_radians()).abs() < 1e-6);
        assert!(r.leave);
        assert_eq!(r.ok, 4);
        assert!(r.done());
    }

    /// ROM-gated (CA_ROM): the lobby's clan button changes nothing, the create button opens the room creation.
    #[test]
    fn rom_lobby_clicks_tell_dead_buttons_from_live_ones() {
        let Ok(rom) = dc_rom::resolve_rom() else {
            eprintln!("skipped: no CA_ROM");
            return;
        };
        events::open(None).unwrap();
        let mut m = Menu::new(crate::menu::MenuOptions {
            auth: "127.0.0.1:9".parse().unwrap(),
            user: "t".into(),
            link: crate::menu::link::LinkOptions::new("t"),
            rom: Some(rom),
            font: None,
            width: 1024,
            height: 768,
        });
        m.open(crate::menu::Go::Lobby);
        // every lobby bar button is live since Wave 3; a dead click is one that lands on no widget at all
        // (the corner hits the background tile, which has no command: the click changes nothing)
        let at = [1.0, 1.0];
        assert!(!click_at(&mut m, at, "lobby.background", true));
        let Resolved::Part { at, .. } = resolve(&m, "lobby.create") else {
            panic!("no create button");
        };
        assert!(click_at(&mut m, at, "lobby.create", true));
        assert_eq!(m.screen_name(), "create");
        assert_eq!(
            resolve(&m, "lobby.shop"),
            Resolved::OtherScreen("lobby".into())
        );
        assert!(matches!(
            resolve(&m, "cmd:999999"),
            Resolved::NoRect {
                command: Some(999999),
                ..
            }
        ));
    }

    #[test]
    fn a_menu_verb_waits_through_the_match_and_times_out() {
        let steps =
            parse("{\"do\":\"click\",\"widget\":\"room.ready\",\"after\":0,\"timeout\":0.01}")
                .unwrap();
        let mut r = StepRunner::new(steps, true);
        let mut view = ViewAngles::default();
        let mut out = Vec::new();
        r.poll(&mut Here::Game {
            view: &mut view,
            out: &mut out,
        });
        assert_eq!(r.failed.len(), 0);
        std::thread::sleep(Duration::from_millis(30));
        r.poll(&mut Here::Game {
            view: &mut view,
            out: &mut out,
        });
        assert_eq!(r.failed.len(), 1);
        assert!(r.failed[0].contains("timeout"), "{:?}", r.failed);
    }
}
