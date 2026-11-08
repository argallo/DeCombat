//! A scripted player for the menu screens (headless runs, tests, PNGs of every screen): it types the login,
//! confirms the server and the channel, creates or joins a room and readies or starts, by clicking the ROM
//! layout's buttons where the screen has them (else by the button's command), and saves one image per screen.
//! [`run`] drives a [`Menu`] until it quits, playing every match with a caller's game runner and returning to the
//! waiting room after it.

use super::{Menu, MenuInput, MenuKey, MenuOutcome};
use crate::headless::ensure_dir;
use dc_botclient::lobby::Lobby;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// What the scripted player does in the lobby.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PilotRoom {
    /// Create a room (title, mode row, players) and start it once `start_after` members are in and ready.
    Create {
        title: String,
        /// The mode row picked in the creation box's mode list (`None`: the box's start row).
        mode: Option<i32>,
        /// The map row picked in the waiting room's map list before the start (`None`: the room's map).
        map: Option<i32>,
        start_after: usize,
    },
    /// Join room `n` (or the first listed room) and get ready.
    Join(Option<u32>),
}

/// Our mode names and the ROM mode class each one stands for (the lobby's `modes.playable` names).
// SPEC: GAME_MODES_2009.md section 2 (the classes); the names are ours (DC_DEFAULTS GMD-26)
pub const MODE_CLASS_NAMES: [(&str, u8); 13] = [
    ("TDM", 1),
    ("BOMB", 2),
    ("BOMB_CLAN", 3),
    ("TDM_CLAN", 4),
    ("COOP", 5),
    ("FFA", 6),
    ("FLAG", 7),
    ("FLAG_CLAN", 8),
    ("BOUNTY", 9),
    ("CLASS10", 10),
    ("INFECT", 11),
    ("ELIM", 12),
    ("ELIM_CLAN", 13),
];

/// One map or mode row of the ROM room table as the command line names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowName {
    pub index: i32,
    /// The row's short key.
    pub key: String,
    /// The row's display name.
    pub label: String,
    /// The mode class (mode rows).
    pub class: Option<u8>,
}

/// The ROM room table's map and mode rows, for `--pilot-map` / `--mode` by name.
// DC-DEFAULT: PILOT-1 (proposed) a map is named by its row number, its key or its display name; a mode by its row
// number, its key or our class name (the first row of that class); case, spaces, `_` and `-` do not count
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoomNames {
    pub maps: Vec<RowName>,
    pub modes: Vec<RowName>,
}

fn fold_name(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(*c, ' ' | '_' | '-' | '\'' | '"'))
        .flat_map(char::to_lowercase)
        .collect()
}

impl RoomNames {
    /// From the room table's map and mode rows.
    pub fn from_rows(maps: &[dc_bute::SectionMap], modes: &[dc_bute::SectionMap]) -> RoomNames {
        use super::catalog::{find_key, room_keys};
        let row = |i: usize, r: &dc_bute::SectionMap, label_key| RowName {
            index: i as i32,
            key: find_key(r.iter(), &room_keys::ROW_KEY)
                .unwrap_or_default()
                .trim()
                .to_string(),
            label: find_key(r.iter(), label_key)
                .unwrap_or_default()
                .trim()
                .trim_matches('"')
                .trim()
                .to_string(),
            class: find_key(r.iter(), &room_keys::MODE_CLASS).and_then(|v| v.trim().parse().ok()),
        };
        RoomNames {
            maps: maps
                .iter()
                .enumerate()
                .map(|(i, r)| row(i, r, &room_keys::MAP_LABEL))
                .collect(),
            modes: modes
                .iter()
                .enumerate()
                .map(|(i, r)| row(i, r, &room_keys::MODE_NAME))
                .collect(),
        }
    }

    /// The menu catalog's rows.
    pub fn from_catalog(cat: &super::catalog::Catalog) -> RoomNames {
        RoomNames::from_rows(cat.map_rows(), cat.mode_rows())
    }

    /// The map rows as the command line accepts them (`index key (label)` per line).
    pub fn maps_text(&self) -> String {
        self.maps
            .iter()
            .map(|m| format!("{} {} ({})", m.index, m.key, m.label))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The mode names the command line accepts.
    pub fn modes_text(&self) -> String {
        let rows = self.modes.iter().map(|m| {
            let class = m
                .class
                .and_then(|c| MODE_CLASS_NAMES.iter().find(|(_, k)| *k == c))
                .map_or(String::new(), |(n, _)| format!(" = {n}"));
            format!("{} {}{class}", m.index, m.key)
        });
        rows.collect::<Vec<_>>().join(", ")
    }

    /// The map row named `s`: a row number, a row key or a display name.
    pub fn map(&self, s: &str) -> Result<i32, String> {
        let t = s.trim();
        if let Ok(n) = t.parse::<i32>() {
            // MAP-32: community maps follow the ROM rows, so a number past the ROM table is a community index
            // (its name lives in the lobby's record, not in the ROM table); the pilot reports a row the room's
            // map list does not offer
            if self.maps.is_empty()
                || self.maps.iter().any(|m| m.index == n)
                || n >= self.maps.len() as i32
            {
                return Ok(n);
            }
        }
        let f = fold_name(t);
        self.maps
            .iter()
            .find(|m| fold_name(&m.key) == f)
            .or_else(|| self.maps.iter().find(|m| fold_name(&m.label) == f))
            .map(|m| m.index)
            .ok_or_else(|| match self.maps.is_empty() {
                true => format!(
                    "--pilot-map {t:?}: no ROM room table to look names up in (give a row number)"
                ),
                false => format!(
                    "--pilot-map {t:?}: no such map; ROM maps: {}",
                    self.maps_text()
                ),
            })
    }

    /// The mode row named `s`: a row number, a row key or our class name (the first row of that class).
    pub fn mode(&self, s: &str) -> Result<i32, String> {
        let t = s.trim();
        if let Ok(n) = t.parse::<i32>() {
            return Ok(n);
        }
        let f = fold_name(t);
        if let Some(m) = self.modes.iter().find(|m| fold_name(&m.key) == f) {
            return Ok(m.index);
        }
        let class = MODE_CLASS_NAMES
            .iter()
            .find(|(n, _)| fold_name(n) == f)
            .map(|(_, c)| *c);
        class
            .and_then(|c| self.modes.iter().find(|m| m.class == Some(c)))
            .map(|m| m.index)
            .ok_or_else(|| {
                format!(
                    "--mode {t:?}: no such mode; ROM modes: {}; class names: {}",
                    self.modes_text(),
                    MODE_CLASS_NAMES.map(|(n, _)| n).join(" ")
                )
            })
    }
}

/// The scripted player's plan.
#[derive(Debug, Clone)]
pub struct PilotPlan {
    pub password: String,
    pub room: PilotRoom,
    /// Matches to play before quitting (each match returns to the waiting room).
    pub matches: usize,
    /// One PNG per screen (and per box) into this folder (ROM-derived images: local files, never committed).
    pub shots: Option<PathBuf>,
    /// Give up after this long.
    pub timeout: Duration,
}

/// What a scripted run did.
#[derive(Debug, Clone, Default)]
pub struct PilotReport {
    /// Screens in order, with milestones ("match", "back").
    pub trail: Vec<String>,
    pub shots: Vec<PathBuf>,
    pub matches: usize,
    /// Texts of the boxes with buttons that opened (errors, refusals, questions): each one asks the player.
    pub boxes: Vec<String>,
    /// Texts of the notices that were seen open: wait / progress boxes without buttons that the menu closes by
    /// itself ("connecting"); whether one is seen at all depends on how long the connection took.
    pub notices: Vec<String>,
    /// The box still open when the run ended (`None`: the screen was clear).
    pub open_at_end: Option<String>,
    pub end: String,
}

/// Delay between two scripted actions on the same screen (the screens and the lobby need a moment).
// DC-DEFAULT: CLI-174 (proposed) scripted menu player: 250 ms between actions, one image per screen after 400 ms
pub const ACTION_GAP: Duration = Duration::from_millis(250);
pub const SHOT_SETTLE: Duration = Duration::from_millis(400);

/// The scripted player.
pub struct Pilot {
    plan: PilotPlan,
    last_action: Option<Instant>,
    screen: String,
    since: Instant,
    typed_password: bool,
    shot_taken: bool,
    shot_box: Option<String>,
    picked_room: bool,
    asked: bool,
    /// MAP-69: the server's mod consent prompt was accepted (once per run).
    consented: bool,
    matches: usize,
    /// The result screen is the player's (no Exit click): it closes by its Exit button or its own 60 s timer.
    keep_result: bool,
    /// The planned mode was picked in the creation box (or given up on).
    mode_done: bool,
    mode_list_opened: bool,
    /// The planned map is the room's (or was given up on); list openings so far.
    map_done: bool,
    map_tries: u32,
    /// What the scripted player could not do as planned (also printed when it happens).
    pub warnings: Vec<String>,
    /// MAP-88: the entry to enter on the server screen (`None`: the screen's own highlighted row).
    entry: Option<u32>,
    /// MAP-88: why the plan cannot go on; [`run`] ends there.
    gave_up: Option<String>,
}

impl Pilot {
    pub fn new(plan: PilotPlan) -> Pilot {
        Pilot {
            plan,
            last_action: None,
            screen: String::new(),
            since: Instant::now(),
            typed_password: false,
            shot_taken: false,
            shot_box: None,
            picked_room: false,
            asked: false,
            consented: false,
            matches: 0,
            keep_result: false,
            mode_done: false,
            mode_list_opened: false,
            map_done: false,
            map_tries: 0,
            warnings: Vec::new(),
            entry: None,
            gave_up: None,
        }
    }

    /// MAP-88: enter server entry `id` of the lobby's list, by moving the list's highlight as a player does. An entry
    /// the list does not hold, or a pick that ends in an error box (the entry's refusal or pack failure), ends the
    /// run ([`Pilot::gave_up`]): it never enters another entry instead.
    // DC-DEFAULT: MAP-88 (proposed) the scripted player picks a server entry by id
    pub fn on_entry(mut self, id: u32) -> Pilot {
        self.entry = Some(id);
        self
    }

    /// MAP-88: why the scripted player stopped following its plan (`None`: it has not).
    pub fn gave_up(&self) -> Option<&str> {
        self.gave_up.as_deref()
    }

    fn give_up(&mut self, why: String) {
        self.warn(why.clone());
        self.gave_up = Some(why);
    }

    /// A pilot for a person at the window: it logs in, opens and starts the room as before, but leaves the result
    /// screen after each match to the player (the 2009 screen 7: Exit, or back to the waiting room by itself after
    /// 60 s, [`crate::matchexit::RESULT_AUTO_EXIT_MS`]); the next match starts from the waiting room again.
    // DC-DEFAULT: CLI-611 (proposed) play_rust's hands-free menu path shows the result screen to the player
    pub fn keep_result(mut self) -> Pilot {
        self.keep_result = true;
        self
    }

    /// A click on the visible button with `command` (its centre), or the command itself without a layout. The step
    /// runner's strict mode never takes this path ([`crate::steps`]: no layout = a failed step).
    pub fn click(menu: &Menu, command: i32) -> Vec<MenuInput> {
        match menu.sheet().button(command) {
            Some(b) => {
                let r = b.rect();
                let c = [
                    r[0] as f32 + r[2] as f32 / 2.0,
                    r[1] as f32 + r[3] as f32 / 2.0,
                ];
                vec![
                    MenuInput::Move(c),
                    MenuInput::Press(c),
                    MenuInput::Release(c),
                ]
            }
            None => vec![MenuInput::Command(command)],
        }
    }

    fn gap_passed(&self) -> bool {
        self.last_action.is_none_or(|t| t.elapsed() >= ACTION_GAP)
            && self.since.elapsed() >= ACTION_GAP
    }

    fn ready(&self) -> bool {
        // with images asked for, a screen's image comes before its first action
        let shot_done = self.plan.shots.is_none() || self.shot_taken;
        shot_done && self.gap_passed()
    }

    /// An open notice may be dismissed: its image is saved (when images are asked for) and the gaps have passed.
    fn notice_ready(&self, text: &str) -> bool {
        (self.plan.shots.is_none() || self.shot_box.as_deref() == Some(text))
            && self.last_action.is_none_or(|t| t.elapsed() >= ACTION_GAP)
            && self.since.elapsed() >= ACTION_GAP
    }

    /// The inputs for this moment.
    pub fn next(&mut self, menu: &Menu) -> Vec<MenuInput> {
        let name = menu.screen_name().to_string();
        if name != self.screen {
            self.screen = name.clone();
            self.since = Instant::now();
            self.shot_taken = false;
            self.asked = false;
        }
        if let Some(p) = menu.popup() {
            // CLI-1942: the login's own error box after the Login click (no lobby, refused, too old, a trust
            // refusal): the scripted player cannot go on, so the run ends with the reason, not at its time-out
            if name == "login"
                && self.asked
                && self.gave_up.is_none()
                && p.title == super::popup::text_ids::TITLE_ERROR
            {
                self.give_up(format!("login: {}", p.text));
            }
            // MAP-88: the planned entry's pick ended in an error box: the entry is refused, nothing else is entered
            if let Some(id) = self.entry.filter(|_| {
                name == "servers"
                    && self.asked
                    && self.gave_up.is_none()
                    && p.title == super::popup::text_ids::TITLE_ERROR
            }) {
                self.give_up(format!("--pilot-entry {id}: {}", p.text));
            }
            // a plain notice (OK only: the daily bonus of LG-310, a promotion, an inbox note) is read by the
            // caller's loop and dismissed here, as a player would; yes/no, wait and input boxes are left to the
            // screen's own step (the test asserts on them)
            // DC-DEFAULT: MAP-69 (proposed) the scripted player dismisses OK notices
            // MAP-88: a box that opens before the screen's own image waits for the box's image, not the screen's (the
            // screen's image waits for the box to close; the daily bonus arrives with the first channel list)
            if p.kind == super::popup::BoxKind::Ok && self.notice_ready(&p.text) {
                self.last_action = Some(Instant::now());
                return vec![MenuInput::Key(super::MenuKey::Escape)];
            }
            return vec![];
        }
        if !self.ready() {
            return vec![];
        }
        let out = match name.as_str() {
            "login" => {
                if !self.typed_password {
                    self.typed_password = true;
                    // the remembered ID puts the keyboard in the password box
                    self.plan.password.chars().map(MenuInput::Char).collect()
                } else if !self.asked {
                    self.asked = true;
                    Pilot::click(menu, super::login::CMD_LOGIN)
                } else if !self.consented && menu.layout_rects().is_some() {
                    // the consent prompt for the server's unverified mods (CLI-1618) is the login screen's own
                    // window, not a popup: the scripted player, started against this server on purpose, accepts
                    // it once (the window's Accept / the command); a declined set would end the run at the login
                    // DC-DEFAULT: MAP-69 (proposed) the scripted player accepts the server's mod set
                    self.consented = true;
                    Pilot::click(menu, super::modbrowser::CMD_AGREE)
                } else {
                    vec![]
                }
            }
            "servers" if !self.asked && !menu.data.servers.is_empty() => {
                self.asked = true;
                self.server_pick(menu)
            }
            "channels" if !self.asked && !menu.data.channels.is_empty() => {
                self.asked = true;
                // no row is selected when the screen opens
                let mut v = vec![MenuInput::Key(super::MenuKey::Down)];
                v.extend(Pilot::click(menu, super::channels::CMD_ENTER));
                v
            }
            "lobby" if !self.asked => match &self.plan.room {
                PilotRoom::Create { .. } => {
                    self.asked = true;
                    Pilot::click(menu, super::lobby::CMD_CREATE)
                }
                PilotRoom::Join(n) => {
                    let pos = match n {
                        Some(n) => menu.data.rooms.iter().position(|r| r.number == *n),
                        None => (!menu.data.rooms.is_empty()).then_some(0),
                    };
                    match pos {
                        Some(i) if !self.picked_room => {
                            self.picked_room = true;
                            let mut v = vec![MenuInput::Key(MenuKey::Down); i + 1];
                            v.push(MenuInput::Key(MenuKey::Enter));
                            self.asked = true;
                            v
                        }
                        _ => vec![],
                    }
                }
            },
            "create" if !self.asked && !self.mode_done => self.pick_mode(menu),
            "create" if !self.asked => {
                self.asked = true;
                let mut v = Vec::new();
                if let PilotRoom::Create { title, .. } = &self.plan.room {
                    // replace the title box's text (it has the keyboard)
                    v.extend(std::iter::repeat_n(MenuInput::Key(MenuKey::Backspace), 20));
                    v.extend(title.chars().map(MenuInput::Char));
                }
                v.extend(Pilot::click(menu, super::create::CMD_OK));
                v
            }
            "room" => self.room_step(menu),
            // the result screen: its image, then the exit button (a player who does not wait for its 60 s)
            "result" if !self.asked && !self.keep_result => {
                self.asked = true;
                Pilot::click(menu, crate::matchexit::RESULT_CMD_EXIT)
            }
            _ => vec![],
        };
        if !out.is_empty() {
            self.last_action = Some(Instant::now());
        }
        out
    }

    /// The server screen's pick: the screen's own row, or (MAP-88) the planned entry's: up to the top of the list,
    /// down to its row, Enter's button.
    fn server_pick(&mut self, menu: &Menu) -> Vec<MenuInput> {
        let servers = &menu.data.servers;
        let Some(id) = self.entry else {
            return Pilot::click(menu, super::servers::CMD_ENTER);
        };
        let Some(row) = servers.iter().position(|e| e.id == id) else {
            let listed: Vec<u32> = servers.iter().map(|e| e.id).collect();
            self.give_up(format!(
                "--pilot-entry {id}: the lobby lists no such entry (it lists {listed:?})"
            ));
            return vec![];
        };
        let mut v = vec![MenuInput::Key(MenuKey::Up); servers.len()];
        v.extend(std::iter::repeat_n(MenuInput::Key(MenuKey::Down), row));
        v.extend(Pilot::click(menu, super::servers::CMD_ENTER));
        v
    }

    fn warn(&mut self, w: String) {
        eprintln!("dc-client pilot: {w}");
        self.warnings.push(w);
    }

    /// The inputs that pick the row holding `value` in the open list: a click on its row, or on the scroll arrow
    /// towards it. `None` when the list has no such row.
    pub fn pick_row(d: &super::dropdown::Dropdown, value: i32) -> Option<(Vec<MenuInput>, bool)> {
        let i = d.values.iter().position(|v| *v == value)?;
        let click = |p: [f32; 2]| {
            vec![
                MenuInput::Move(p),
                MenuInput::Press(p),
                MenuInput::Release(p),
            ]
        };
        if let Some(p) = d.row_point(i) {
            return Some((click(p), true));
        }
        let dir = if i < d.shown().start { -1 } else { 1 };
        d.arrow_point(dir).map(|p| (click(p), false))
    }

    /// The creation box: opens the mode list and picks the planned mode row.
    // DC-DEFAULT: PILOT-2 (proposed) the scripted player picks the mode in the creation box's mode list and the map
    // in the waiting room's map list (the host's 0x38c), as a player does; a row the lists do not offer (the lobby's
    // modes.playable, the mode's maps) is reported and the room keeps its own
    fn pick_mode(&mut self, menu: &Menu) -> Vec<MenuInput> {
        let want = match &self.plan.room {
            PilotRoom::Create { mode: Some(m), .. } => *m,
            _ => {
                self.mode_done = true;
                return vec![];
            }
        };
        match menu.dropdown() {
            Some(d) => match Pilot::pick_row(d, want) {
                Some((v, picked)) => {
                    self.mode_done = picked;
                    v
                }
                None => {
                    self.mode_done = true;
                    self.warn(format!(
                        "mode row {want} is not offered by the lobby (its modes.playable or the maps' modes); the room keeps the box's mode"
                    ));
                    Pilot::click(menu, super::create::CMD_MODE_LIST)
                }
            },
            None if !self.mode_list_opened => {
                // the click opens the list at once (a screen without a layout has none)
                self.mode_list_opened = true;
                Pilot::click(menu, super::create::CMD_MODE_LIST)
            }
            None => {
                self.mode_done = true;
                self.warn(format!(
                    "mode row {want}: the creation box has no mode list"
                ));
                vec![]
            }
        }
    }

    /// The waiting room as host: opens the map list and picks the planned map until the room has it.
    fn pick_map(&mut self, menu: &Menu, want: i32) -> Vec<MenuInput> {
        if menu.data.room.map == want {
            self.map_done = true;
            return vec![];
        }
        match menu.dropdown() {
            Some(d) if d.list == "map" => match Pilot::pick_row(d, want) {
                Some((v, picked)) => {
                    if picked {
                        // the lobby's answer (0x3035) sets the room's map; the list opens again after a while
                        self.since = Instant::now();
                    }
                    v
                }
                None => {
                    self.map_done = true;
                    self.warn(format!(
                        "map row {want} is not offered for the room's mode (the map list has {:?}); the room keeps its map",
                        d.items
                    ));
                    Pilot::click(menu, super::room::CMD_MAP)
                }
            },
            Some(_) => vec![],
            None if self.map_tries < 4 => {
                if self.map_tries > 0 && self.since.elapsed() < Duration::from_secs(2) {
                    return vec![];
                }
                self.map_tries += 1;
                Pilot::click(menu, super::room::CMD_MAP)
            }
            None => {
                self.map_done = true;
                self.warn(format!(
                    "map row {want}: the room did not take it (map {} after {} tries)",
                    menu.data.room.map, self.map_tries
                ));
                vec![]
            }
        }
    }

    fn room_step(&mut self, menu: &Menu) -> Vec<MenuInput> {
        // every planned match was played: the waiting room is only looked at
        if self.matches >= self.plan.matches {
            return vec![];
        }
        let d = &menu.data;
        let me = d.profile.my_id;
        let own_state = d.members.get(&me).map_or(0, |m| m.state);
        match &self.plan.room {
            PilotRoom::Create {
                map: Some(want), ..
            } if d.host() && !self.map_done => {
                let want = *want;
                self.pick_map(menu, want)
            }
            PilotRoom::Create { start_after, .. } => {
                let others_ready = d
                    .members
                    .iter()
                    .filter(|(id, _)| **id != me)
                    .all(|(_, m)| m.state == 1);
                if d.host()
                    && d.members.len() >= *start_after
                    && others_ready
                    && own_state == 0
                    && (!self.asked || self.since.elapsed() > Duration::from_secs(3))
                {
                    self.asked = true;
                    self.since = Instant::now();
                    return Pilot::click(menu, super::room::CMD_START);
                }
                vec![]
            }
            PilotRoom::Join(_) => {
                if own_state == 0 && (!self.asked || self.since.elapsed() > Duration::from_secs(2))
                {
                    self.asked = true;
                    self.since = Instant::now();
                    return Pilot::click(menu, super::room::CMD_READY);
                }
                vec![]
            }
        }
    }

    /// Saves the screen's image once it settled (and every box once).
    pub fn shoot(&mut self, menu: &mut Menu, report: &mut PilotReport) {
        let Some(dir) = self.plan.shots.clone() else {
            return;
        };
        let box_text = menu.popup().map(|p| p.text.clone());
        let take_box = box_text.is_some() && box_text != self.shot_box;
        let take_screen =
            !self.shot_taken && self.since.elapsed() >= SHOT_SETTLE && box_text.is_none();
        if !(take_box || take_screen) {
            return;
        }
        if take_box {
            self.shot_box = box_text;
        } else {
            self.shot_taken = true;
        }
        let n = report.shots.len();
        let tag = if take_box { "box" } else { "screen" };
        let name = format!("menu_{n:02}_{}_{tag}.png", menu.screen_name());
        let img = menu.image();
        if ensure_dir(&dir).is_ok() {
            let p = dir.join(name);
            let frame = dc_render::FrameImage {
                width: img.width,
                height: img.height,
                rgba: img.rgba,
            };
            if frame.save_png(&p).is_ok() {
                report.shots.push(p);
            }
        }
    }
}

/// What a match gives back: the lobby connection and the match's result (`None` without a result screen).
pub struct GameBack {
    pub lobby: Option<Lobby>,
    pub result: Option<crate::matchexit::MatchResult>,
}

/// Plays one match: the game session runs on the handed-over connection; returns the connection afterwards.
pub type GameRunner<'a> = dyn FnMut(SocketAddr, Lobby, i32) -> GameBack + 'a;

/// Drives `menu` with `pilot` until it quits (or the plan's matches are played, or the time-out).
pub fn run(menu: &mut Menu, pilot: &mut Pilot, play: &mut GameRunner) -> PilotReport {
    // w2-lobbychat: the scripted player runs without the waiting room's idle rules (CLI-486)
    menu.set_idle_rules(false);
    let mut report = PilotReport::default();
    let start = Instant::now();
    // the last match was played and its result screen is up: the run ends in the waiting room after it
    let mut finishing = false;
    loop {
        menu.update();
        if let Some(p) = menu.popup() {
            let list = if p.kind == super::popup::BoxKind::Wait {
                &mut report.notices
            } else {
                &mut report.boxes
            };
            if list.last() != Some(&p.text) {
                list.push(p.text.clone());
            }
        }
        pilot.shoot(menu, &mut report);
        for i in pilot.next(menu) {
            menu.input(i);
        }
        // MAP-88: the planned entry cannot be entered: the run ends with the reason, not at its time-out
        if let Some(why) = pilot.gave_up() {
            report.end = why.to_string();
            break;
        }
        match menu.take_outcome() {
            Some(MenuOutcome::Play {
                server,
                lobby,
                mode,
            }) => {
                set_match_content(menu.mod_content());
                set_match_mods(
                    menu.mod_recipe(),
                    (menu.data.room.hacks_offered != 0).then_some(menu.data.room.hacks),
                );
                let back = play(server, *lobby, mode);
                pilot.matches += 1;
                report.matches = pilot.matches;
                menu.back_from_match(back.lobby, back.result);
                if pilot.matches >= pilot.plan.matches && menu.screen_name() == "result" {
                    finishing = true;
                } else if pilot.matches >= pilot.plan.matches {
                    // one image of the waiting room after the match, then the end
                    let t = Instant::now();
                    while t.elapsed() < SHOT_SETTLE * 2 {
                        menu.update();
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    pilot.screen.clear();
                    pilot.since = Instant::now() - SHOT_SETTLE;
                    pilot.shot_taken = false;
                    pilot.shoot(menu, &mut report);
                    report.end = "matches played".into();
                    break;
                }
            }
            Some(MenuOutcome::Quit) => {
                report.end = "quit".into();
                break;
            }
            None => {}
        }
        // the pilot's own record of the screen (it resets its clock and image flag on a change) must be the room
        if finishing
            && menu.screen_name() == "room"
            && pilot.screen == "room"
            && pilot.since.elapsed() >= SHOT_SETTLE * 2
            && (pilot.plan.shots.is_none() || pilot.shot_taken)
        {
            report.end = "matches played".into();
            break;
        }
        if start.elapsed() > pilot.plan.timeout {
            report.end = format!("timeout on {}", menu.screen_name());
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    report.trail = menu.trail.clone();
    report.open_at_end = menu.popup().map(|p| p.text.clone());
    report
}

thread_local! {
    /// CLI-1611: the server set's weapon content for the match the runner is about to play (set by the pilot
    /// loops from the menu's join report, read by [`headless_runner`]; the runner has no menu).
    static MATCH_CONTENT: std::cell::RefCell<Option<std::sync::Arc<dc_bute::modpatch::WeaponContent>>> =
        const { std::cell::RefCell::new(None) };
}

thread_local! {
    /// CLI-1619: the server's mods and the room's Hacks setting for the same match (read by [`headless_runner`]).
    static MATCH_MODS: std::cell::RefCell<(Option<crate::mods::HostRecipe>, Option<u32>)> =
        const { std::cell::RefCell::new((None, None)) };
}

fn set_match_mods(recipe: Option<crate::mods::HostRecipe>, hacks: Option<u32>) {
    MATCH_MODS.with(|m| *m.borrow_mut() = (recipe, hacks));
}

fn set_match_content(c: Option<std::sync::Arc<dc_bute::modpatch::WeaponContent>>) {
    MATCH_CONTENT.with(|m| *m.borrow_mut() = c);
}

/// The options with the weapon content and the mod recipe the pilot loop recorded for this match (CLI-1611 and
/// MAP-31, as the window path does with `menu.mod_content()` / `menu.mod_recipe()`).
fn with_match_content(mut o: crate::net::NetOptions) -> crate::net::NetOptions {
    o.content = MATCH_CONTENT.with(|m| m.borrow().clone());
    o.room_hacks = MATCH_MODS.with(|m| m.borrow().1.unwrap_or(0));
    o
}

/// A game runner that plays each match headless (the repeating walk as input) with `hopts` and returns the lobby
/// connection afterwards; one summary line per match goes to `log`.
pub fn headless_runner<'a>(
    base: &'a crate::net::NetOptions,
    login: String,
    hopts: &'a crate::headless::HeadlessOptions,
    log: &'a mut Vec<String>,
) -> impl FnMut(SocketAddr, Lobby, i32) -> GameBack + 'a {
    move |server, lobby, mode| {
        let nick = lobby.profile.nick.clone();
        let handoff = super::handoff::LobbyHandoff::new(server, lobby, mode);
        let mut net = crate::net::NetWorld::start(with_match_content(
            super::handoff::game_options(base, handoff.clone(), &nick, &login),
        ));
        let handle = net.handle();
        let mut walk = crate::input::WalkTest::new(crate::look::ViewAngles::default());
        // a step file drives the match when one is installed (gate T2)
        let mut steps = crate::steps::StepInput::new(crate::look::LookParams::DEFAULT);
        let input: &mut dyn crate::input::InputSource = if crate::steps::active() {
            &mut steps
        } else {
            &mut walk
        };
        let mut opts = hopts.clone();
        (opts.mods, opts.room_hacks) = MATCH_MODS.with(|m| m.borrow().clone());
        let r = crate::headless::run_headless(&mut net, Some(input), &opts);
        let st = handle.shutdown(Duration::from_secs(5));
        drop(net);
        let result = handoff.take_result();
        log.push(match r {
            Ok(r) => format!(
                "match on {server}: {} | world {:?} | updates {} | end {:?}",
                r.summary(),
                st.world,
                st.updates_sent,
                st.end
            ),
            Err(e) => format!("match on {server}: {e} (phase {:?})", st.phase),
        });
        GameBack {
            lobby: handoff.take_back(),
            result,
        }
    }
}

/// Drives `menu` with the installed step file ([`crate::steps`]) until a `quit` step, the menu's own quit, or
/// `timeout`; every match runs with `play`. A `shot` step saves the menu's CPU image (the headless reference of the
/// window's read-back).
pub fn run_steps(menu: &mut Menu, play: &mut GameRunner, timeout: Duration) -> PilotReport {
    menu.set_idle_rules(false);
    let mut report = PilotReport::default();
    let start = Instant::now();
    // a headless run has no system cursor (the window run writes the one it shows); the menu draws its own image
    crate::events::emit(
        "cursor",
        serde_json::json!({ "image": if menu.has_cursor_image() { "menu-pcx" } else { "none" }, "visible": false, "grabbed": false, "headless": true }),
    );
    loop {
        menu.update();
        crate::steps::menu_frame(menu);
        if let Some(name) = crate::steps::take_shot() {
            let img = menu.image();
            crate::steps::save_shot(
                &name,
                &dc_render::FrameImage {
                    width: img.width,
                    height: img.height,
                    rgba: img.rgba,
                },
            );
        }
        if let Some(p) = menu.popup() {
            let list = if p.kind == super::popup::BoxKind::Wait {
                &mut report.notices
            } else {
                &mut report.boxes
            };
            if list.last() != Some(&p.text) {
                list.push(p.text.clone());
            }
        }
        match menu.take_outcome() {
            Some(MenuOutcome::Play {
                server,
                lobby,
                mode,
            }) => {
                set_match_content(menu.mod_content());
                set_match_mods(
                    menu.mod_recipe(),
                    (menu.data.room.hacks_offered != 0).then_some(menu.data.room.hacks),
                );
                let back = play(server, *lobby, mode);
                report.matches += 1;
                menu.back_from_match(back.lobby, back.result);
            }
            Some(MenuOutcome::Quit) => {
                report.end = "quit".into();
                break;
            }
            None => {}
        }
        if crate::steps::quit_requested() {
            report.end = "steps quit".into();
            break;
        }
        if start.elapsed() > timeout {
            report.end = format!("timeout on {}", menu.screen_name());
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    report.trail = menu.trail.clone();
    report.open_at_end = menu.popup().map(|p| p.text.clone());
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI-1611: the pilot's headless runner hands the menu's joined-set content to the match options.
    #[test]
    fn match_options_carry_the_recorded_content() {
        let o = || {
            crate::net::NetOptions::new(
                crate::net::Connect::Udp("127.0.0.1:1".parse().unwrap()),
                "t",
            )
        };
        assert!(with_match_content(o()).content.is_none());
        let c = dc_bute::modpatch::WeaponContent::from_json(
            r#"{"version":1,"weapons":[{"weapon":145,"op":"patch","fields":{"AmmoDamage":"41"}}],"ammo":[]}"#,
        )
        .unwrap();
        set_match_content(Some(std::sync::Arc::new(c.clone())));
        assert_eq!(
            with_match_content(o()).content.as_deref().map(|c| c.hash()),
            Some(c.hash())
        );
        set_match_content(None);
        assert!(with_match_content(o()).content.is_none());
    }

    fn names() -> RoomNames {
        let row = |index, key: &str, label: &str, class| RowName {
            index,
            key: key.into(),
            label: label.into(),
            class,
        };
        RoomNames {
            maps: vec![
                row(0, "Alpha_One", "Alpha One", None),
                row(1, "Beta", "Big Beta", None),
            ],
            modes: vec![
                row(0, "TDM", "Team", Some(1)),
                row(1, "XQ", "Other", Some(6)),
                row(2, "XR", "Other 2", Some(6)),
            ],
        }
    }

    use crate::menu::popup::{BoxTag, Popup};

    fn entry(id: u32) -> dc_botclient::lobby::ServerEntry {
        dc_botclient::lobby::ServerEntry {
            id,
            name: format!("entry {id}"),
            ip: "127.0.0.1".into(),
            port: 1,
            open: 1,
            capacity: 50,
            current: 0,
            clan_only: 0,
            op_capacity: 0,
            op_current: 0,
        }
    }

    fn server_menu() -> Menu {
        let mut menu = result_menu();
        menu.data.servers = vec![entry(1), entry(2), entry(3)];
        menu.open(Go::Servers);
        menu
    }

    /// Steps `pilot` until it has acted or given up.
    fn step(menu: &mut Menu, pilot: &mut Pilot) {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            let v = pilot.next(menu);
            if !v.is_empty() || pilot.gave_up().is_some() {
                for i in v {
                    menu.input(i);
                }
                return;
            }
            assert!(Instant::now() < end, "the pilot did nothing");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// MAP-88: the planned entry is entered whatever row the screen highlighted by itself.
    #[test]
    fn the_pilot_enters_the_planned_entry_by_its_id() {
        let mut menu = server_menu();
        let mut pilot = Pilot::new(plan()).on_entry(3);
        step(&mut menu, &mut pilot);
        assert_eq!(menu.link_latest_request(), "PickServer(3)");
        assert_eq!(pilot.gave_up(), None);
        // without a plan the screen's own highlight stands
        let mut menu = server_menu();
        let mut pilot = Pilot::new(plan());
        step(&mut menu, &mut pilot);
        assert_eq!(menu.link_latest_request(), "PickServer(1)");
    }

    /// MAP-88: an entry the list lacks ends the run with the reason at once; no other entry is entered.
    #[test]
    fn an_entry_the_list_lacks_ends_the_run_with_the_reason() {
        let mut menu = server_menu();
        let mut pilot = Pilot::new(plan()).on_entry(9);
        let mut play = |_: SocketAddr, _: Lobby, _: i32| -> GameBack { panic!("no match") };
        let report = run(&mut menu, &mut pilot, &mut play);
        assert!(report.end.contains("--pilot-entry 9"), "{}", report.end);
        assert!(report.end.contains("[1, 2, 3]"), "{}", report.end);
        let last = menu.link_latest_request();
        assert!(!last.starts_with("PickServer"), "nothing was picked: {last}");
    }

    /// CLI-1942: the login's error box after the Login click ends the run with its text; before the click (a box
    /// left from somewhere else) and on a plain notice the pilot goes on.
    #[test]
    fn a_login_error_box_after_the_click_ends_the_run() {
        let mut menu = result_menu();
        menu.open(Go::Login);
        assert_eq!(menu.screen_name(), "login");
        let mut pilot = Pilot::new(plan());
        menu.popups.push(Popup::error("stale".into(), BoxTag::None));
        std::thread::sleep(ACTION_GAP + Duration::from_millis(50));
        pilot.next(&menu);
        assert_eq!(pilot.gave_up(), None, "no Login click yet");
        menu.popups.clear();
        // the password (empty here) and the Login click
        step(&mut menu, &mut pilot);
        if !pilot.asked {
            step(&mut menu, &mut pilot);
        }
        assert!(pilot.asked, "the Login click went out");
        menu.popups.push(Popup::ok("a notice".into(), BoxTag::None));
        std::thread::sleep(ACTION_GAP + Duration::from_millis(50));
        pilot.next(&menu);
        assert_eq!(
            pilot.gave_up(),
            None,
            "a notice is dismissed, not a refusal"
        );
        menu.popups.clear();
        menu.popups
            .push(Popup::error("Login failed.".into(), BoxTag::None));
        pilot.next(&menu);
        assert_eq!(pilot.gave_up(), Some("login: Login failed."));
        let report = run(&mut menu, &mut pilot, &mut |_, _, _| unreachable!());
        assert_eq!(report.end, "login: Login failed.");
    }

    /// MAP-88: an error box after the pick (the entry's refusal, a pack failure) ends the run; a notice does not.
    #[test]
    fn an_error_box_after_the_pick_gives_up_and_a_notice_does_not() {
        let mut menu = server_menu();
        let mut pilot = Pilot::new(plan()).on_entry(2);
        step(&mut menu, &mut pilot);
        menu.popups.push(Popup::ok("a bonus".into(), BoxTag::None));
        pilot.next(&menu);
        assert_eq!(
            pilot.gave_up(),
            None,
            "an OK notice is dismissed, not a refusal"
        );
        menu.popups.clear();
        menu.popups
            .push(Popup::error("refused".into(), BoxTag::None));
        pilot.next(&menu);
        assert!(pilot.gave_up().is_some_and(|w| w.contains("refused")));
    }

    /// MAP-88: a notice that opens before the screen's own image is dismissed once the box's image is saved: the
    /// screen's image waits for the box to close, so waiting for it here never ended.
    #[test]
    fn a_notice_before_the_screens_image_waits_only_for_its_own_image() {
        let mut menu = server_menu();
        let mut pilot = Pilot::new(PilotPlan {
            shots: Some(PathBuf::from("unused")),
            ..plan()
        });
        menu.popups.push(Popup::ok("a bonus".into(), BoxTag::None));
        assert!(pilot.next(&menu).is_empty());
        std::thread::sleep(ACTION_GAP + Duration::from_millis(50));
        assert!(pilot.next(&menu).is_empty(), "its image comes first");
        assert!(!pilot.shot_taken);
        pilot.shot_box = Some("a bonus".into());
        assert_eq!(pilot.next(&menu), [MenuInput::Key(MenuKey::Escape)]);
        assert!(pilot.next(&menu).is_empty(), "one key per gap");
    }

    #[test]
    fn maps_are_named_by_number_key_or_display_name() {
        let n = names();
        assert_eq!(n.map("1"), Ok(1));
        assert_eq!(n.map("alpha one"), Ok(0));
        assert_eq!(n.map("ALPHA_ONE"), Ok(0));
        assert_eq!(n.map("big-beta"), Ok(1));
        let e = n.map("Gamma").unwrap_err();
        assert!(
            e.contains("0 Alpha_One (Alpha One)") && e.contains("1 Beta (Big Beta)"),
            "{e}"
        );
        // a row number past the ROM table is a community map's index (MAP-32); a negative one is no map
        assert_eq!(n.map("7"), Ok(7));
        assert!(n.map("-1").is_err());
        // without a ROM table only numbers work
        assert_eq!(RoomNames::default().map("7"), Ok(7));
        assert!(RoomNames::default().map("Alpha").is_err());
    }

    #[test]
    fn modes_are_named_by_number_key_or_class_name() {
        let n = names();
        assert_eq!(n.mode("2"), Ok(2));
        assert_eq!(n.mode("tdm"), Ok(0));
        assert_eq!(n.mode("xr"), Ok(2));
        // a class name: the first row of the class
        assert_eq!(n.mode("FFA"), Ok(1));
        let e = n.mode("ELIM").unwrap_err();
        assert!(e.contains("1 XQ = FFA"), "{e}");
    }

    /// The user's ROM: every map and mode row as the command line names them (printed for the launcher's help).
    #[test]
    fn rom_room_names() {
        let Some(rom) = std::env::var_os("CA_ROM")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
        else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let g = dc_bute::GameRoomData::from_rom(&rom).expect("room table");
        let n = RoomNames::from_rows(&g.maps(), &g.modes(false));
        println!("maps: {}", n.maps_text());
        println!("modes: {}", n.modes_text());
        assert!(!n.maps.is_empty() && !n.modes.is_empty());
        let tdm = n.mode("TDM").expect("a TDM row");
        assert_eq!(n.modes[tdm as usize].class, Some(1));
        for m in &n.maps {
            assert_eq!(n.map(&m.key), Ok(m.index), "{m:?}");
        }
    }
    use crate::menu::link::LinkOptions;
    use crate::menu::{Go, MenuOptions};

    fn result_menu() -> Menu {
        let mut menu = Menu::new(MenuOptions {
            auth: "127.0.0.1:1".parse().unwrap(),
            user: "PilotUser".into(),
            link: LinkOptions::new("endflow-pilot"),
            rom: None,
            font: Some((crate::testkit::synth_font(), 0)),
            width: 1024,
            height: 768,
        });
        menu.data.result = Some(crate::matchexit::MatchResult::default());
        menu.open(Go::Result);
        menu
    }

    fn plan() -> PilotPlan {
        PilotPlan {
            password: String::new(),
            room: PilotRoom::Join(None),
            matches: usize::MAX,
            shots: None,
            timeout: Duration::from_secs(60),
        }
    }

    /// CLI-611: the hands-free pilot clicks the result screen's Exit; the player's pilot leaves the screen up.
    #[test]
    fn the_players_pilot_leaves_the_result_screen_up() {
        let menu = result_menu();
        assert_eq!(menu.screen_name(), "result");
        let mut hands_free = Pilot::new(plan());
        let mut player = Pilot::new(plan()).keep_result();
        // the screen is seen first, an action comes after the gap
        assert!(hands_free.next(&menu).is_empty());
        assert!(player.next(&menu).is_empty());
        std::thread::sleep(ACTION_GAP + Duration::from_millis(50));
        let click = hands_free.next(&menu);
        assert!(
            !click.is_empty(),
            "the scripted run leaves the result screen"
        );
        for _ in 0..3 {
            assert!(
                player.next(&menu).is_empty(),
                "the player's result screen stays"
            );
            std::thread::sleep(ACTION_GAP);
        }
    }
}
