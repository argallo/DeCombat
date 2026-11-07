//! LG-427: `/help` over fake connections: each sender gets the lines of the commands it may use, and the table cannot
//! drift from the handlers (their match arms are read from the source).

use crate::db;
use crate::help::{self, Need, ROWS};
use crate::logs::Listener;
use crate::msg::*;
use crate::net::route;
use crate::opscfg::TestCommands;
use crate::state::testing::*;
use crate::state::{Core, Outgoing, Sid};
use dc_wire::PktWriter;
use rusqlite::params;
use std::collections::BTreeSet;
use std::sync::mpsc::Receiver;

struct Client {
    sid: Sid,
    rx: Receiver<Outgoing>,
    got: Vec<(u16, Vec<u8>)>,
}

impl Client {
    fn send(&mut self, st: &mut Core, id: u16, body: &[u8]) {
        let mut w = PktWriter::new(id);
        w.bytes(body);
        let p = w.into_payload();
        st.scope(|st| route(st, self.sid, id as i32, &p)).unwrap();
        self.got.extend(drain(&self.rx));
    }

    /// Types a chat line.
    fn chat(&mut self, st: &mut Core, text: &str) {
        let mut w = PktWriter::empty();
        w.wstr(text, 121);
        let body = w.raw_bytes().to_vec();
        self.send(st, CHAT_SEND, &body);
    }

    /// The chat lines received since the last call.
    fn said(&mut self) -> Vec<String> {
        self.got.extend(drain(&self.rx));
        let out = self
            .got
            .iter()
            .filter(|(id, _)| *id == CHAT_PUSH)
            .map(|(_, d)| {
                let n = u32_at(d, 0x34) as usize;
                String::from_utf16_lossy(
                    &d[0x38..0x38 + 2 * n]
                        .chunks(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        self.got.clear();
        out
    }

    /// `/help` and its lines.
    fn help(&mut self, st: &mut Core) -> Vec<String> {
        self.got.extend(drain(&self.rx));
        self.got.clear();
        self.chat(st, "/help");
        self.said()
    }

    fn open_room(&mut self, st: &mut Core) {
        let mut b = 8i32.to_le_bytes().to_vec();
        for (s, n) in [("", 33), ("", 9)] {
            let mut w = PktWriter::empty();
            w.wstr(s, n);
            b.extend(w.raw_bytes());
        }
        b.extend(0i32.to_le_bytes());
        self.send(st, ROOM_CREATE, &b);
        assert!(self.got.iter().any(|(id, _)| *id == ROOM_CREATE_REPLY));
    }
}

/// An account with a character, logged into the lobby on `sid` and waiting in channel 1.
fn player(st: &mut Core, sid: Sid, login: &str, nick: &str) -> (Client, u32) {
    let acc = db::create_account(&st.db, login, 0).unwrap();
    st.scope(|st| crate::auth::create_character(st, acc, nick, 2))
        .unwrap();
    let rx = connect(st, sid, Listener::Lobby);
    let s = st.sessions.get_mut(&sid).unwrap();
    s.account = Some(acc);
    s.in_lobby = true;
    s.channel = Some(1);
    (
        Client {
            sid,
            rx,
            got: Vec::new(),
        },
        acc,
    )
}

fn set_gm(st: &Core, account: u32) {
    st.db
        .execute(
            "UPDATE accounts SET gm = 2 WHERE user_no = ?1",
            params![account as i64],
        )
        .unwrap();
}

/// A state where the GM test commands are for GM accounts only (the default is every user of a loopback server).
fn gm_only() -> Core {
    let mut st = state();
    st.rules.ops.test_commands = TestCommands::Gm;
    st
}

fn has(lines: &[String], head: &str) -> bool {
    lines
        .iter()
        .any(|l| l == head || l.starts_with(&format!("{head} ")))
}

#[test]
fn a_player_sees_the_player_commands_and_nothing_else() {
    let mut st = gm_only();
    let (mut a, _) = player(&mut st, 1, "alice", "Alice");
    let lines = a.help(&mut st);
    for c in [
        "/help",
        "/report",
        "/diag",
        "/nettest",
        "!nation",
        "!nm add",
        "!nm accept",
        "!nm decline",
        "!nm remove",
        "!nm list",
        "!nm w",
    ] {
        assert!(has(&lines, c), "{c} in {lines:?}");
    }
    // a player without a clan may found or join one
    assert!(
        has(&lines, "!clan create") && has(&lines, "!clan join"),
        "{lines:?}"
    );
    // nothing that needs a clan, a room, or an operator
    for c in [
        "!clan leave",
        "!clan invite",
        "!clan accept",
        "!clan decline",
        "!clan kick",
        "!nm clan",
        "!nm invite",
        "/k",
        "/notice",
        "/expire",
        "/shopver",
        "/grade",
        "!hisec",
    ] {
        assert!(!has(&lines, c), "{c} in {lines:?}");
    }
    assert!(
        lines.iter().all(|l| l.chars().count() <= help::MAX_LINE),
        "{lines:?}"
    );
    // the line is `<command> <args> - <what it does>`
    assert!(
        lines.contains(&"/report [text] - send the server's owner a diagnostic report".to_string()),
        "{lines:?}"
    );
}

#[test]
fn the_answer_is_private_and_is_not_chat() {
    let mut st = gm_only();
    let (mut a, _) = player(&mut st, 1, "alice", "Alice");
    let (mut b, _) = player(&mut st, 2, "bob", "Bob");
    let lines = a.help(&mut st);
    assert!(lines.len() > 5);
    assert!(b.said().is_empty(), "the other player hears nothing");
    // any case, with trailing words; the lines are sent as 0x3051 from sender 0
    a.chat(&mut st, "  /HELP me");
    let d = a
        .got
        .iter()
        .find(|(id, _)| *id == CHAT_PUSH)
        .map(|(_, d)| d.clone())
        .unwrap();
    assert_eq!(u32_at(&d, 6), 0, "a server line, not a player's");
    assert_eq!(a.said().len(), lines.len());
    assert!(b.said().is_empty());
    // `/helpme` is ordinary chat
    a.chat(&mut st, "/helpme");
    assert_eq!(b.said(), vec!["/helpme".to_string()]);
}

#[test]
fn clan_lines_follow_the_clan_and_the_leader() {
    let mut st = gm_only();
    let (mut lead, _) = player(&mut st, 1, "alice", "Alice");
    let (mut mem, _) = player(&mut st, 2, "bob", "Bob");
    lead.chat(&mut st, "!clan create Wolves");
    assert!(lead.said().iter().any(|l| l.contains("created")));
    mem.chat(&mut st, "!clan join Wolves");
    assert!(mem.said().iter().any(|l| l.contains("Application sent")));
    lead.said();
    lead.chat(&mut st, "!clan accept Bob");
    assert!(lead.said().iter().any(|l| l.contains("joined the clan")));
    mem.said();

    let l = lead.help(&mut st);
    for c in [
        "!clan leave",
        "!clan invite",
        "!clan accept",
        "!clan decline",
        "!clan kick",
        "!nm clan",
    ] {
        assert!(has(&l, c), "leader: {c} in {l:?}");
    }
    assert!(!has(&l, "!clan create") && !has(&l, "!clan join"), "{l:?}");

    let m = mem.help(&mut st);
    assert!(has(&m, "!clan leave") && has(&m, "!nm clan"), "{m:?}");
    for c in [
        "!clan invite",
        "!clan accept",
        "!clan decline",
        "!clan kick",
        "!clan create",
        "!clan join",
    ] {
        assert!(!has(&m, c), "member: {c} in {m:?}");
    }

    // with `clan.chat_commands` off no `!clan` line is listed (the clan chat relay is not that rule)
    st.rules.clan.chat_commands = false;
    let l = lead.help(&mut st);
    assert!(
        !l.iter().any(|x| x.starts_with("!clan")) && has(&l, "!nm clan"),
        "{l:?}"
    );
}

#[test]
fn room_lines_need_a_room_and_k_needs_an_operator_in_a_waiting_room() {
    let mut st = gm_only();
    let (mut p, _) = player(&mut st, 1, "alice", "Alice");
    let (mut g, ga) = player(&mut st, 2, "gm", "Boss");
    set_gm(&st, ga);
    let lobby = g.help(&mut st);
    assert!(
        !has(&lobby, "/k") && !has(&lobby, "!nm invite"),
        "in the channel lobby: {lobby:?}"
    );

    p.open_room(&mut st);
    g.open_room(&mut st);
    let pl = p.help(&mut st);
    assert!(has(&pl, "!nm invite"), "{pl:?}");
    assert!(!has(&pl, "/k"), "a player has no /k: {pl:?}");
    let gl = g.help(&mut st);
    assert!(has(&gl, "/k") && has(&gl, "!nm invite"), "{gl:?}");
    assert!(gl.contains(&"/k <nick> - remove a player from this waiting room".to_string()));

    // in a match the waiting-room command is gone
    let key = st.sessions[&g.sid].room.unwrap();
    st.rooms.get_mut(&key).unwrap().state = crate::room::RoomPhase::Playing;
    let gl = g.help(&mut st);
    assert!(!has(&gl, "/k") && has(&gl, "!nm invite"), "{gl:?}");
}

#[test]
fn gm_lines_are_for_gms_unless_the_server_opens_the_test_commands() {
    let mut st = gm_only();
    let (mut p, _) = player(&mut st, 1, "alice", "Alice");
    let (mut g, ga) = player(&mut st, 2, "gm", "Boss");
    set_gm(&st, ga);
    let test = ["/notice", "/expire", "/shopver", "/grade", "!hisec"];
    let pl = p.help(&mut st);
    for c in test {
        assert!(!has(&pl, c), "a non-GM sees {c}: {pl:?}");
    }
    let gl = g.help(&mut st);
    for c in test {
        assert!(has(&gl, c), "a GM misses {c}: {gl:?}");
    }
    // everything the player sees, the GM sees as well
    assert!(pl.iter().all(|l| gl.contains(l)));
    // the same rule as the commands themselves: a loopback server (the default) and `all` open them to every user
    st.rules.ops.test_commands = TestCommands::All;
    assert!(has(&p.help(&mut st), "/notice"));
    st.rules.ops.test_commands = TestCommands::Loopback;
    st.opts.bind = "0.0.0.0".into();
    assert!(!has(&p.help(&mut st), "/notice"));
    st.opts.bind = "127.0.0.1".into();
    assert!(has(&p.help(&mut st), "/notice"));
}

#[test]
fn mod_commands_follow_the_table_and_only_on_a_server_that_declares_them() {
    let mut st = gm_only();
    let (mut p, _) = player(&mut st, 1, "alice", "Alice");
    let base = p.help(&mut st);
    assert!(!base.iter().any(|l| l.starts_with("/wallhack")));
    st.mod_commands = vec![
        dc_mod_lua::ModCommand {
            command: "/wallhack".into(),
            description: "Next see-through style: box, chams, skeleton.".into(),
        },
        dc_mod_lua::ModCommand {
            command: "/wallhack".into(),
            description: "Next see-through style: box, chams, skeleton.".into(),
        },
        dc_mod_lua::ModCommand {
            command: "/long".into(),
            description: "x".repeat(120),
        },
    ];
    let lines = p.help(&mut st);
    assert_eq!(&lines[..base.len()], &base[..], "the table comes first");
    let extra = &lines[base.len()..];
    assert_eq!(
        extra.len(),
        2,
        "a command declared twice is listed once: {extra:?}"
    );
    assert_eq!(
        extra[0],
        "/wallhack - Next see-through style: box, chams, skeleton."
    );
    assert!(
        extra[1].starts_with("/long - xxx")
            && extra[1].ends_with("...")
            && extra[1].chars().count() == help::MAX_LINE
    );
}

#[test]
fn a_featured_entry_lists_its_own_mods_commands() {
    use crate::modset::{EntrySet, ModEntry, ModSet};
    let mut st = gm_only();
    let (mut p, _) = player(&mut st, 1, "alice", "Alice");
    st.mod_commands = vec![dc_mod_lua::ModCommand {
        command: "/lobbywide".into(),
        description: "not this entry's".into(),
    }];
    let id = crate::servers::map_entry_of_session(&st, p.sid)
        .expect("the session owns a channel of an entry");
    let mut set = ModSet::vanilla();
    set.modded = true;
    set.mods.push(ModEntry {
        id: "xray".into(),
        version: "1.0.0".into(),
        sha256: "a".repeat(64),
        kind: Some("shared".into()),
        requires_client: true,
        capabilities: vec![],
        source_repo: None,
        description: String::new(),
        author: String::new(),
        tags: vec![],
        screenshots: vec![],
        commands: vec![dc_mod_lua::ModCommand {
            command: "/health".into(),
            description: "Health bars on or off.".into(),
        }],
        preview: None,
    });
    st.entry_mods.insert(
        id,
        EntrySet {
            entry: id,
            set,
            expected: Default::default(),
            client: crate::modhost::ModClient::start_declared(
                Vec::new(),
                std::sync::Arc::new(crate::logs::Logger::none()),
                None,
            )
            .unwrap(),
            doc: String::new(),
            mods_dir: Default::default(),
        },
    );
    let lines = p.help(&mut st);
    assert!(
        lines.contains(&"/health - Health bars on or off.".to_string()),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("/lobbywide")),
        "{lines:?}"
    );
}

// ---- the table against the handlers ----

/// The quoted words of the match arms (`"a" | "b" => ...`) of function `name` in `src`, one group per arm.
fn arms(src: &str, name: &str) -> Vec<Vec<String>> {
    let src = src.replace("\r\n", "\n");
    let start = src
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("fn {name}"));
    let body = &src[start..];
    let body = &body[..body.find("\n}\n").unwrap()];
    body.lines()
        .map(str::trim)
        .filter(|l| l.starts_with('"') && l.contains("=>"))
        .map(|l| {
            l.split("=>")
                .next()
                .unwrap()
                .split('|')
                .map(|w| w.trim().trim_matches('"').to_string())
                .filter(|w| !w.is_empty())
                .collect()
        })
        .collect()
}

/// Each arm group has a row (`prefix` + any word of it), and each `prefix` row has an arm.
fn agree(prefix: &str, groups: &[Vec<String>], internal: &[&str]) {
    let rows: BTreeSet<&str> = ROWS
        .iter()
        .filter_map(|r| r.cmd.strip_prefix(prefix))
        .collect();
    assert!(!groups.is_empty(), "no arms read for {prefix}");
    for g in groups {
        if g.iter().all(|w| internal.contains(&w.as_str())) {
            continue;
        }
        assert!(
            g.iter().any(|w| rows.contains(w.as_str())),
            "the handler takes {g:?} ({prefix}...) but /help has no row for it"
        );
    }
    for r in rows {
        assert!(
            groups.iter().any(|g| g.iter().any(|w| w == r)),
            "/help lists {prefix}{r} but the handler has no arm for it"
        );
    }
}

#[test]
fn every_dispatched_command_has_a_row_and_every_row_a_handler() {
    // `!clan <verb>`: clan.rs bang_command (the `_` arm is the usage line)
    agree(
        "!clan ",
        &arms(include_str!("clan.rs"), "bang_command"),
        &[],
    );
    // `!nm <verb>`: messenger.rs on_chat; `sync`, `clanyes`, `clanno` are the client's own relay lines
    agree(
        "!nm ",
        &arms(include_str!("messenger.rs"), "on_chat"),
        &["sync", "clanyes", "clanno"],
    );
    // the GM test commands: notices.rs on_chat_command (`/word`, and `!hisec`)
    let gm: Vec<Vec<String>> = arms(include_str!("notices.rs"), "on_chat_command")
        .into_iter()
        .map(|g| {
            g.into_iter()
                .map(|w| {
                    if w == "hisec" {
                        "!hisec".to_string()
                    } else {
                        format!("/{w}")
                    }
                })
                .collect()
        })
        .collect();
    assert_eq!(gm.len(), 5, "{gm:?}");
    for g in &gm {
        assert!(
            ROWS.iter()
                .any(|r| g.iter().any(|w| w == r.cmd) && r.need == Need::TestCommands),
            "{g:?} has no GM row"
        );
    }
    for r in ROWS.iter().filter(|r| r.need == Need::TestCommands) {
        assert!(
            gm.iter().any(|g| g.iter().any(|w| w == r.cmd)),
            "{} has no handler",
            r.cmd
        );
    }
    // the single-word commands handled elsewhere: `!nation` by lobby.rs, `/help` by help.rs; `/report`, `/diag`,
    // `/nettest` by the client's lobby chat (menu/diagbox.rs) and `/k` by its kick prompt (menu/gm.rs)
    assert!(include_str!("lobby.rs").contains("\"!nation\""));
    assert!(help::is_help("/help") && !help::is_help("/helpme"));
    let known = [
        "/help", "/report", "/diag", "/nettest", "!nation", "/k", "/notice", "/expire", "/shopver",
        "/grade", "!hisec",
    ];
    for r in ROWS.iter().filter(|r| !r.cmd.contains(' ')) {
        assert!(
            known.contains(&r.cmd),
            "{} is a new kind of command: say where it is handled",
            r.cmd
        );
    }
}

#[test]
fn every_row_is_one_short_line_and_unique() {
    let mut seen = BTreeSet::new();
    for r in ROWS {
        let l = help::line(r);
        assert!(
            l.chars().count() <= help::MAX_LINE,
            "{} chars: {l}",
            l.chars().count()
        );
        assert!(l.contains(" - ") && !l.contains('\n'), "{l}");
        assert!(r.cmd.starts_with('/') || r.cmd.starts_with('!'), "{l}");
        assert!(seen.insert(r.cmd), "{} twice", r.cmd);
    }
}
