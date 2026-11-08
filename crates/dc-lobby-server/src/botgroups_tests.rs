//! Tests of the bot groups: their checks, the table, the dashboard's writes and their guard, the status JSON.
// Provenance: ORIGINAL (DeCombat tests)

use super::*;
use crate::clanweb::parse_request;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};

const LO: Option<IpAddr> = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
const HOST: &str = "127.0.0.1:10090";
const ORIGIN: &str = "http://127.0.0.1:10090";

fn entry(name: &str, channels: u32) -> dc_config::LobbyServerCfg {
    toml::from_str(&format!(
        "name = \"{name}\"\ncapacity = 10\nchannels = {channels}"
    ))
    .unwrap()
}

/// A lobby with three entries: 1 = channels 1..3, 2 = 4..6, 3 = 7..9.
pub fn three_entries() -> Core {
    let mut st = crate::state::testing::state();
    st.opts.servers = vec![
        entry("US Central", 3),
        entry("US West", 3),
        entry("Vanilla++", 3),
    ];
    st
}

fn shared_of(st: Core) -> Shared {
    Arc::new(Mutex::new(st))
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => panic!("not an object"),
    }
}

fn group_body(prefix: &str, count: u32, server: u32, channels: &[u32]) -> Value {
    json!({"prefix": prefix, "count": count, "server_id": server, "channels": channels, "room": "any", "enabled": true})
}

#[test]
fn a_group_is_checked_against_the_server_list() {
    let st = three_entries();
    let e = crate::servers::entries(&st);
    assert_eq!(
        e.iter()
            .map(|x| (x.id, x.first, x.channels))
            .collect::<Vec<_>>(),
        vec![(1, 1, 3), (2, 4, 3), (3, 7, 3)]
    );
    let g = parse(&obj(group_body("PlusBot", 15, 2, &[4, 6, 4])), &e).unwrap();
    assert_eq!(
        (g.prefix.as_str(), g.count, g.server_id, g.channels.clone()),
        ("PlusBot", 15, 2, vec![4, 6])
    );
    assert_eq!(
        (g.skill.as_str(), g.loadout.as_str(), g.room.clone()),
        (BOT_SKILL_RANGE, "", RoomPick::Any)
    );
    let err = |v: Value| parse(&obj(v), &e).unwrap_err();
    assert!(err(group_body("PlusBot", 0, 2, &[4])).contains("count"));
    assert!(
        err(group_body("PlusBot", 16, 2, &[4])).contains("count"),
        "a room holds up to 16"
    );
    assert!(
        err(group_body("PlusBot", 3, 4, &[4])).contains("server_id: 4"),
        "unknown entry"
    );
    assert!(
        err(group_body("PlusBot", 3, 2, &[3])).contains("not a channel of US West"),
        "another entry's channel"
    );
    assert!(err(group_body("PlusBot", 3, 2, &[])).contains("at least one"));
    assert!(err(group_body("PlusBot", 3, 2, &[99])).contains("channels"));
    for bad in [
        "",
        "P",
        "1Bot",
        "Bot7",
        "Bot-x",
        "ThisPrefixIsTooLong",
        "poolbotx",
        "PoolBot",
    ] {
        assert!(
            err(group_body(bad, 3, 2, &[4])).contains("prefix"),
            "{bad:?}"
        );
    }
    let with = |k: &str, v: Value| {
        let mut b = group_body("PlusBot", 3, 2, &[4]);
        b[k] = v;
        b
    };
    assert!(err(with("skill", json!("101"))).contains("skill"));
    assert!(err(with("skill", json!("70-20"))).contains("skill"));
    assert!(err(with("skill", json!("x"))).contains("skill"));
    assert_eq!(
        parse(&obj(with("skill", json!(" 20-70 "))), &e)
            .unwrap()
            .skill,
        "20-70"
    );
    assert_eq!(
        parse(&obj(with("skill", json!("OFF"))), &e).unwrap().skill,
        "off"
    );
    assert!(err(with("loadout", json!("0:27; rm -rf"))).contains("loadout"));
    assert_eq!(
        parse(&obj(with("loadout", json!("0:27, 1:3"))), &e)
            .unwrap()
            .loadout,
        "0:27,1:3"
    );
    assert_eq!(
        parse(&obj(with("loadout", json!("Random"))), &e)
            .unwrap()
            .loadout,
        "random"
    );
    assert!(
        err(with("room", json!("follow"))).contains("follow"),
        "follow needs a name"
    );
    assert!(
        err(with("room", json!("title"))).contains("title"),
        "title needs text"
    );
    assert!(err(with("room", json!("lobby"))).contains("room"));
    let mut b = with("room", json!("follow"));
    b["room_arg"] = json!("Zoe");
    assert_eq!(
        parse(&obj(b), &e).unwrap().room,
        RoomPick::Follow("Zoe".into())
    );
}

#[test]
fn caps_and_prefixes_are_checked_across_groups() {
    let g = |id: i64, prefix: &str, count: u32, enabled: bool| Group {
        id,
        prefix: prefix.into(),
        count,
        server_id: 1,
        channels: vec![1],
        room: RoomPick::Any,
        skill: BOT_SKILL_RANGE.into(),
        loadout: String::new(),
        enabled,
    };
    let all = vec![
        g(1, "AlphaBot", 7, true),
        g(2, "BetaBot", 7, true),
        g(3, "IdleBot", 7, false),
    ];
    assert!(
        check_against(&g(0, "GammaBot", 7, true), &all, 21).is_ok(),
        "7 + 7 + 7 = 21"
    );
    assert!(check_against(&g(0, "GammaBot", 7, true), &all, 20)
        .unwrap_err()
        .contains("at most 20"));
    assert!(
        check_against(&g(0, "GammaBot", 7, false), &all, 14).is_ok(),
        "a stopped group counts nothing"
    );
    assert!(
        check_against(&g(2, "BetaBot", 7, true), &all, 14).is_ok(),
        "an edit does not count itself twice"
    );
    assert!(check_against(&g(0, "alphabot", 1, false), &all, 21)
        .unwrap_err()
        .contains("already uses"));
    assert!(
        check_against(&g(1, "AlphaBot", 7, true), &all, 21).is_ok(),
        "its own prefix"
    );
    // the channels a bot gets: round-robin
    let ch = [4, 6];
    assert_eq!(
        (1..=5)
            .map(|i| Group::channel_of(&ch, i))
            .collect::<Vec<_>>(),
        vec![4, 6, 4, 6, 4]
    );
    assert_eq!(
        g(1, "AlphaBot", 3, true).names(),
        vec!["AlphaBot1", "AlphaBot2", "AlphaBot3"]
    );
}

#[test]
fn groups_are_stored_and_a_changed_server_list_is_noticed() {
    let st = three_entries();
    let e = crate::servers::entries(&st);
    let mut g = parse(&obj(group_body("PlusBot", 3, 2, &[4, 5])), &e).unwrap();
    let id = save(&st.db, &g, 100).unwrap();
    g.id = id;
    assert_eq!(get(&st.db, id).unwrap().unwrap(), g);
    g.room = RoomPick::Title("fun".into());
    g.loadout = "random".into();
    save(&st.db, &g, 200).unwrap();
    assert_eq!(list(&st.db).unwrap(), vec![g.clone()]);
    assert_eq!(usable_channels(&g, &e).unwrap(), vec![4, 5]);
    // the entry lost a channel, then went away
    let mut fewer = e.clone();
    fewer[1].channels = 1;
    assert_eq!(usable_channels(&g, &fewer).unwrap(), vec![4]);
    assert!(usable_channels(&g, &e[..1])
        .unwrap_err()
        .contains("not in the server list"));
    assert!(remove(&st.db, id).unwrap());
    assert!(!remove(&st.db, id).unwrap());
    assert_eq!(applied_loadout(&st.db, "plusbot1").unwrap(), "");
    set_applied_loadout(&st.db, "PlusBot1", "0:27", 5).unwrap();
    set_applied_loadout(&st.db, "plusbot1", "random", 6).unwrap();
    assert_eq!(applied_loadout(&st.db, "PLUSBOT1").unwrap(), "random");
}

/// A request through the loopback admin port's dashboard handler.
fn request(
    sh: &Shared,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, Value) {
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {HOST}\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let (st, _, out) =
        crate::opsweb::handle_with_body(sh, &head, body.as_bytes(), LO, None, true).unwrap();
    (st, serde_json::from_str(&out).unwrap_or(Value::Null))
}

fn page_token(sh: &Shared) -> String {
    let head = format!("GET /ops HTTP/1.1\r\nHost: {HOST}\r\n\r\n");
    let (st, _, page) = crate::opsweb::handle_with_body(sh, &head, &[], LO, None, true).unwrap();
    assert_eq!(st, 200);
    assert!(!page.contains("__CSRF__"), "the token is filled in");
    let at = page
        .find("name=\"csrf\" content=\"")
        .expect("the page carries its token")
        + 21;
    page[at..at + page[at..].find('"').unwrap()].to_string()
}

fn post(sh: &Shared, path: &str, tok: &str, body: &Value) -> (u16, Value) {
    request(
        sh,
        "POST",
        path,
        &[
            ("Content-Type", "application/json"),
            ("Origin", ORIGIN),
            ("X-Admin-CSRF", tok),
        ],
        &body.to_string(),
    )
}

#[test]
fn writes_need_json_the_page_origin_and_its_token() {
    let sh = shared_of(three_entries());
    let tok = page_token(&sh);
    assert_eq!(tok.len(), 64);
    let body = group_body("PlusBot", 3, 2, &[4]).to_string();
    let refused = |h: &[(&str, &str)]| request(&sh, "POST", "/ops/bots/group", h, &body);
    let (s, v) = refused(&[("Origin", ORIGIN), ("X-Admin-CSRF", &tok)]);
    assert_eq!(s, 415, "{v}");
    assert_eq!(
        refused(&[("Content-Type", "application/json"), ("X-Admin-CSRF", &tok)]).0,
        403
    );
    assert_eq!(
        refused(&[
            ("Content-Type", "application/json"),
            ("Origin", "http://evil.example"),
            ("X-Admin-CSRF", &tok)
        ])
        .0,
        403
    );
    assert_eq!(
        refused(&[("Content-Type", "application/json"), ("Origin", ORIGIN)]).0,
        403
    );
    assert_eq!(
        refused(&[
            ("Content-Type", "application/json"),
            ("Origin", ORIGIN),
            ("X-Admin-CSRF", "x")
        ])
        .0,
        403
    );
    assert!(
        list(&crate::net::lock(&sh).db).unwrap().is_empty(),
        "nothing was written"
    );
    // the admin web's own origin and session token, not the loopback port's
    let guard = WriteGuard {
        origin: "https://admin.example.com".into(),
        csrf: "s".repeat(43),
        by: "the admin web".into(),
    };
    let head = |origin: &str, tok: &str| {
        format!(
            "POST /ops/bots/group HTTP/1.1\r\nHost: admin.example.com\r\nContent-Type: application/json\r\n\
             Origin: {origin}\r\nX-Admin-CSRF: {tok}\r\n\r\n"
        )
    };
    let via_web = |h: String| {
        admin(
            &sh,
            &parse_request(&h).unwrap(),
            &h,
            body.as_bytes(),
            Some(&guard),
        )
        .0
    };
    assert_eq!(
        via_web(head(ORIGIN, &tok)),
        403,
        "the loopback token and origin do not pass there"
    );
    assert_eq!(
        via_web(head("https://admin.example.com", &"s".repeat(43))),
        200
    );
    // GET only for the status, the rest is POST
    assert_eq!(request(&sh, "GET", "/ops/bots/group", &[], "").0, 405);
    assert_eq!(request(&sh, "GET", "/ops/botsx", &[], "").0, 404);
}

#[test]
fn the_dashboard_adds_changes_starts_stops_and_deletes_a_group() {
    let sh = shared_of(three_entries());
    let tok = page_token(&sh);
    let (s, v) = post(
        &sh,
        "/ops/bots/group",
        &tok,
        &group_body("PlusBot", 3, 2, &[4, 6]),
    );
    assert_eq!(s, 200, "{v}");
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .contains("bots.managed is off"),
        "this lobby does not run them"
    );
    let id = v["group"]["id"].as_i64().unwrap();
    // a second group over the total cap is refused; an unknown entry too
    {
        let mut g = crate::net::lock(&sh);
        g.opts.bots_max_total = 9;
    }
    let (s, v) = post(
        &sh,
        "/ops/bots/group",
        &tok,
        &group_body("WestBot", 7, 2, &[5]),
    );
    assert_eq!(s, 409, "{v}");
    assert_eq!(
        post(
            &sh,
            "/ops/bots/group",
            &tok,
            &group_body("WestBot", 2, 9, &[5])
        )
        .0,
        400
    );
    assert_eq!(
        post(
            &sh,
            "/ops/bots/group",
            &tok,
            &group_body("plusbot", 2, 1, &[1])
        )
        .0,
        409,
        "prefix taken"
    );
    // an edit without `enabled` keeps the group running
    let mut edit = group_body("PlusBot", 5, 3, &[7, 8, 9]);
    edit["id"] = json!(id);
    edit.as_object_mut().unwrap().remove("enabled");
    edit["loadout"] = json!("0:27");
    let (s, v) = post(&sh, "/ops/bots/group", &tok, &edit);
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (
            v["group"]["enabled"].as_bool(),
            v["group"]["server_id"].as_u64()
        ),
        (Some(true), Some(3))
    );
    edit["id"] = json!(999);
    assert_eq!(post(&sh, "/ops/bots/group", &tok, &edit).0, 404);
    // stop, start (the cap is checked again), delete
    assert_eq!(post(&sh, "/ops/bots/stop", &tok, &json!({"id": id})).0, 200);
    assert!(!get(&crate::net::lock(&sh).db, id).unwrap().unwrap().enabled);
    let (s, v) = post(
        &sh,
        "/ops/bots/group",
        &tok,
        &group_body("WestBot", 6, 2, &[5]),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        post(&sh, "/ops/bots/start", &tok, &json!({"id": id})).0,
        409,
        "5 + 6 > 9"
    );
    assert_eq!(
        post(&sh, "/ops/bots/delete", &tok, &json!({"id": 999})).0,
        404
    );
    assert_eq!(
        post(&sh, "/ops/bots/delete", &tok, &json!({"id": id})).0,
        200
    );
    assert!(get(&crate::net::lock(&sh).db, id).unwrap().is_none());
    // every change is an ops event of kind bots, by group id and prefix
    let g = crate::net::lock(&sh);
    let ev = crate::opsmon::events_json(&g.db, false, 50)
        .unwrap()
        .to_json_string();
    let ev: Value = serde_json::from_str(&ev).unwrap();
    let details: Vec<String> = ev
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "bots")
        .map(|e| e["detail"].as_str().unwrap().to_string())
        .collect();
    let want = [
        format!("group {id} added: PlusBot x3 on entry 2"),
        format!("group {id} changed: PlusBot x5 on entry 3 channels 7,8,9"),
        format!("group {id} PlusBot stopped"),
        format!("group {id} PlusBot deleted"),
    ];
    for w in want {
        assert!(
            details.iter().any(|d| d.starts_with(&w)),
            "{w} in {details:?}"
        );
    }
}

fn playing_room(key: crate::room::RoomKey, host: u32) -> crate::room::Room {
    crate::room::Room {
        key,
        title: "Fun room".into(),
        password: String::new(),
        mode: 0,
        map: 0,
        goal: 0,
        max_players: 8,
        mid_join: 1,
        ff_on: 0,
        killcam: 0,
        weapon_limit: 0,
        hacks: 0,
        hardcore: false,
        gun_game: false,
        possession: false,
        gun_seed: 0,
        ladder: Vec::new(),
        host_badge: 0,
        host,
        state: crate::room::RoomPhase::Playing,
        members: Vec::new(),
        kicked: Default::default(),
        gen: 0,
        game: Some(1),
        address_sent: true,
        played_since: None,
    }
}

#[test]
fn the_status_lists_entries_groups_and_where_each_bot_is() {
    let mut st = three_entries();
    // OPS-14: an entry naming another lobby is no entry of this one: the status lists it apart, for the form to show disabled
    st.opts.other_lobbies =
        vec![
            serde_json::from_value(json!({"name": "Modded", "lobby": "127.0.0.1:10091"})).unwrap(),
        ];
    let e = crate::servers::entries(&st);
    let mut g = parse(&obj(group_body("PlusBot", 2, 2, &[4, 6])), &e).unwrap();
    g.id = save(&st.db, &g, 1).unwrap();
    // PlusBot2 is logged in, on channel 6, in room 6-1 which plays
    let acc = crate::db::create_account(&st.db, "plusbot2", 1).unwrap();
    let sh = shared_of(st);
    let _rx = {
        let mut guard = crate::net::lock(&sh);
        let st = &mut *guard;
        let rx = crate::state::testing::connect(st, 77, crate::logs::Listener::Lobby);
        let key = crate::room::RoomKey { channel: 6, no: 1 };
        let s = st.sessions.get_mut(&77).unwrap();
        s.account = Some(acc);
        s.in_lobby = true;
        s.channel = Some(6);
        s.room = Some(key);
        st.rooms.insert(key, playing_room(key, acc));
        rx
    };
    let (s, v) = request(&sh, "GET", "/ops/bots", &[], "");
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (
            v["managed"].as_bool(),
            v["max_total"].as_u64(),
            v["per_group"].as_u64()
        ),
        (Some(false), Some(21), Some(15))
    );
    assert_eq!(
        v["entries"].as_array().unwrap().len(),
        3,
        "every entry of this lobby"
    );
    assert_eq!(
        v["others"],
        json!([{"name": "Modded", "lobby": "127.0.0.1:10091"}])
    );
    assert_eq!(v["entries"][1]["name"], "US West");
    assert_eq!(v["entries"][1]["channels"], json!([4, 5, 6]));
    let grp = &v["groups"][0];
    assert_eq!(
        (grp["prefix"].as_str(), grp["entry_name"].as_str()),
        (Some("PlusBot"), Some("US West"))
    );
    assert!(grp["problem"].is_null());
    let b = &grp["bots"];
    assert_eq!(
        (b[0]["name"].as_str(), b[0]["channel"].as_u64()),
        (Some("PlusBot1"), Some(4))
    );
    assert_eq!(b[0]["state"], "stopped", "no supervisor on this lobby");
    assert_eq!(b[0]["lobby"]["online"], false);
    assert_eq!(b[1]["channel"], 6);
    assert_eq!(b[1]["lobby"]["online"], true);
    assert_eq!(b[1]["lobby"]["room"]["id"], "6-1");
    assert_eq!(b[1]["lobby"]["room"]["title"], "Fun room");
    assert_eq!(b[1]["lobby"]["room"]["in_match"], true);
}

/// A character `nick` with a lobby session `sid` on `channel` (None: offline), in room `room` of it.
pub fn player_at(
    st: &mut Core,
    sid: u64,
    nick: &str,
    channel: Option<u32>,
    room: Option<(u32, &str)>,
) -> std::sync::mpsc::Receiver<crate::state::Outgoing> {
    let login = nick.to_ascii_lowercase();
    let acc = match crate::db::account_by_login(&st.db, &login).unwrap() {
        Some(a) => a.user_no,
        None => {
            let acc = crate::db::create_account(&st.db, &login, 1).unwrap();
            crate::db::create_character(
                &st.db,
                &crate::db::NewCharacter {
                    user_no: acc,
                    nick,
                    face: 0,
                    exp: 0,
                    gp: 0,
                    nx: 0,
                    now: 0,
                },
                true,
            )
            .unwrap();
            acc
        }
    };
    st.sessions.retain(|_, s| s.account != Some(acc));
    let rx = crate::state::testing::connect(st, sid, crate::logs::Listener::Lobby);
    let Some(ch) = channel else {
        st.sessions.remove(&sid);
        return rx;
    };
    let key = room.map(|(no, password)| {
        let key = crate::room::RoomKey { channel: ch, no };
        let mut r = playing_room(key, acc);
        r.password = password.into();
        st.rooms.insert(key, r);
        key
    });
    let s = st.sessions.get_mut(&sid).unwrap();
    s.account = Some(acc);
    s.in_lobby = true;
    s.channel = Some(ch);
    s.room = key;
    rx
}

#[test]
fn the_lobby_sees_where_a_followed_player_is() {
    let mut st = three_entries();
    let e = crate::servers::entries(&st);
    assert_eq!(
        place_of_player(&st, &e, "Zoe"),
        Place::Unknown,
        "no character of that name"
    );
    let _a = player_at(&mut st, 5, "Zoe", None, None);
    assert_eq!(place_of_player(&st, &e, "Zoe"), Place::Offline);
    let _b = player_at(&mut st, 6, "Zoe", Some(8), None);
    assert_eq!(
        place_of_player(&st, &e, "zoe"),
        Place::Channel {
            entry: 3,
            channel: 8,
            room: None
        },
        "names are case-insensitive (CD-16); channel 8 is entry 3's"
    );
    let _c = player_at(&mut st, 7, "Zoe", Some(5), Some((2, "")));
    assert_eq!(
        place_of_player(&st, &e, "Zoe"),
        Place::Channel {
            entry: 2,
            channel: 5,
            room: Some(SeenRoom {
                no: 2,
                locked: false
            })
        }
    );
    let _d = player_at(&mut st, 8, "Zoe", Some(5), Some((3, "secret")));
    assert_eq!(
        place_of_player(&st, &e, "Zoe"),
        Place::Channel {
            entry: 2,
            channel: 5,
            room: Some(SeenRoom {
                no: 3,
                locked: true
            })
        }
    );
    let _f = player_at(
        &mut st,
        9,
        "Zoe",
        Some(crate::clan::CLAN_CHANNEL_BASE + 1),
        None,
    );
    assert_eq!(place_of_player(&st, &e, "Zoe"), Place::Clan);
    st.sessions.get_mut(&9).unwrap().channel = None;
    assert_eq!(place_of_player(&st, &e, "Zoe"), Place::NoChannel);
    st.sessions.get_mut(&9).unwrap().closing = true;
    assert_eq!(
        place_of_player(&st, &e, "Zoe"),
        Place::Offline,
        "a closing session is gone"
    );
}

#[test]
fn a_following_group_goes_to_the_players_channel_at_most_every_ten_seconds() {
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let home = Spot {
        entry: 1,
        channels: vec![1, 2],
    };
    let on = |entry, channel| Place::Channel {
        entry,
        channel,
        room: None,
    };
    // the first move is at once, all bots on the player's channel of the player's entry
    let there = Spot {
        entry: 3,
        channels: vec![8],
    };
    assert_eq!(follow_step(&home, None, &on(3, 8), t0), Some(there.clone()));
    // already there: no move
    let in_room = Place::Channel {
        entry: 3,
        channel: 8,
        room: Some(SeenRoom {
            no: 1,
            locked: false,
        }),
    };
    assert_eq!(follow_step(&there, None, &in_room, t0), None);
    // offline, on no channel, in the clan lobby, unknown here: the bots stay where they are
    for p in [
        Place::Offline,
        Place::NoChannel,
        Place::Clan,
        Place::Unknown,
    ] {
        assert_eq!(follow_step(&there, None, &p, t0), None, "{p:?}");
    }
    // the rate limit: a hop within FOLLOW_MOVE_GAP of the last move waits, then the bots go
    assert_eq!(FOLLOW_MOVE_GAP, Duration::from_secs(10));
    let moved = Some(t0);
    assert_eq!(
        follow_step(&there, moved, &on(2, 5), t0 + Duration::from_secs(9)),
        None
    );
    assert_eq!(
        follow_step(&there, moved, &on(2, 5), t0 + Duration::from_secs(10)),
        Some(Spot {
            entry: 2,
            channels: vec![5]
        })
    );
    // the steering of one group: a player hopping channels moves the bots once per gap, to where they are then
    let mut s = Steer::new(home.clone());
    assert!(s.step(&on(3, 8), t0));
    assert!(!s.step(&on(3, 7), t0 + Duration::from_secs(1)));
    assert!(!s.step(&on(2, 4), t0 + Duration::from_secs(5)));
    assert_eq!(s.now, there, "still on channel 8");
    assert!(
        !s.step(&Place::Offline, t0 + Duration::from_secs(30)),
        "offline: they stay"
    );
    assert!(s.step(&on(2, 4), t0 + Duration::from_secs(31)));
    assert_eq!(
        (s.now.entry, s.now.channels.clone(), s.home.clone()),
        (2, vec![4], home)
    );
}

#[test]
fn the_follow_status_says_where_the_player_is_and_that_a_group_follows_in_its_lobby_only() {
    let st = three_entries();
    let e = crate::servers::entries(&st);
    let others = vec!["US Central".to_string()];
    let (lv, t) = follow_text("Zoe", &Place::Unknown, &e, &others);
    assert_eq!(lv, "warn");
    assert!(t.contains("Zoe is not a player of this lobby"), "{t}");
    assert!(
        t.contains("within its own lobby only") && t.contains("US Central (another lobby)"),
        "{t}"
    );
    assert!(t.contains("that instance's Bots tab"), "{t}");
    let (lv, t) = follow_text("Zoe", &Place::Offline, &e, &others);
    assert_eq!(lv, "wait");
    assert!(
        t.starts_with("Waiting for Zoe: not online on this lobby"),
        "{t}"
    );
    assert!(t.contains("US Central"), "{t}");
    let (_, t) = follow_text("Zoe", &Place::Offline, &e, &[]);
    assert!(
        t.ends_with("A group follows within its own lobby only."),
        "{t}"
    );
    assert_eq!(follow_text("Zoe", &Place::NoChannel, &e, &[]).0, "wait");
    assert!(follow_text("Zoe", &Place::Clan, &e, &[])
        .1
        .contains("clan lobby"));
    let on8 = Place::Channel {
        entry: 3,
        channel: 8,
        room: None,
    };
    assert_eq!(
        follow_text("Zoe", &on8, &e, &[]),
        (
            "ok",
            "Zoe is on Vanilla++ channel 8, in no room: the bots wait on that channel.".to_string()
        )
    );
    let r = |locked| Place::Channel {
        entry: 2,
        channel: 5,
        room: Some(SeenRoom { no: 3, locked }),
    };
    assert_eq!(
        follow_text("Zoe", &r(false), &e, &[]),
        (
            "ok",
            "Zoe is in room 5-3 on US West channel 5: the bots join it while it has a free seat."
                .to_string()
        )
    );
    let (lv, t) = follow_text("Zoe", &r(true), &e, &[]);
    assert_eq!(lv, "warn");
    assert!(t.contains("has a password: the bots do not join it"), "{t}");
}

#[test]
fn the_status_of_a_following_group_carries_the_follow_line() {
    let mut st = three_entries();
    st.opts.other_lobbies =
        vec![
            serde_json::from_value(json!({"name": "US Central", "lobby": "127.0.0.1:10091"}))
                .unwrap(),
        ];
    let e = crate::servers::entries(&st);
    let mut b = group_body("ZoeBot", 2, 1, &[1]);
    b["room"] = json!("follow");
    b["room_arg"] = json!("Zoe");
    let mut g = parse(&obj(b), &e).unwrap();
    g.id = save(&st.db, &g, 1).unwrap();
    let mut any = parse(&obj(group_body("PlusBot", 1, 1, &[1])), &e).unwrap();
    any.id = save(&st.db, &any, 1).unwrap();
    let sh = shared_of(st);
    let (_, v) = request(&sh, "GET", "/ops/bots", &[], "");
    assert_eq!(v["groups"][0]["follow"]["level"], "warn", "{v}");
    assert!(v["groups"][0]["follow"]["text"]
        .as_str()
        .unwrap()
        .contains("not a player of this lobby"));
    assert!(
        v["groups"][1]["follow"].is_null(),
        "only a following group has the line"
    );
    let _rx = player_at(&mut crate::net::lock(&sh), 5, "Zoe", Some(9), None);
    let (_, v) = request(&sh, "GET", "/ops/bots", &[], "");
    assert_eq!(v["groups"][0]["follow"]["level"], "ok");
    assert_eq!(
        v["groups"][0]["follow"]["text"],
        "Zoe is on Vanilla++ channel 9, in no room: the bots wait on that channel."
    );
}

#[test]
fn the_status_says_why_bots_over_the_cap_wait_and_when_the_bot_exe_is_missing() {
    let mut st = three_entries();
    st.opts.bots_managed = true;
    st.opts.bots_max_total = 3;
    let e = crate::servers::entries(&st);
    for (p, n) in [("AaBot", 2), ("BbBot", 2), ("CcBot", 1)] {
        let mut g = parse(&obj(group_body(p, n, 2, &[4])), &e).unwrap();
        g.id = save(&st.db, &g, 1).unwrap();
    }
    let sh = shared_of(st);
    let (s, v) = request(&sh, "GET", "/ops/bots", &[], "");
    assert_eq!(s, 200, "{v}");
    assert!(v["groups"][0]["problem"].is_null(), "{v}");
    assert_eq!(
        v["groups"][1]["problem"],
        "1 bot over bots.managed_max_total (3): not started"
    );
    assert_eq!(
        v["groups"][2]["problem"],
        "1 bot over bots.managed_max_total (3): not started"
    );
    // the test binary has no dc-botclient beside it
    assert_eq!(v["exe_missing"], !crate::botsup::bot_exe().is_file());
    crate::net::lock(&sh).opts.bots_managed = false;
    let (_, v) = request(&sh, "GET", "/ops/bots", &[], "");
    assert_eq!(v["exe_missing"], false, "an unmanaged lobby needs none");
    assert!(v["groups"][1]["problem"].is_null());
}
