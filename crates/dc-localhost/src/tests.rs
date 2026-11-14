//! The builders against the game server's own parsers (`dc_dserver::cli::parse`, `dc_dserver::pipe::parse_control`).

use super::*;
use dc_dserver::pipe::{parse_control, Control};
use std::net::Ipv4Addr;

const T0: Ticket = [0x11; 16];
const T1: Ticket = [0x22; 16];
const T2: Ticket = [0x33; 16];

/// The practice match a launcher runs today: a world file with a mod announce, the ROM, a forced loadout, FFA to 500
/// for an hour, two bots, the shot-direction check off.
fn practice() -> LocalMatch {
    LocalMatch {
        world: World::File {
            path: PathBuf::from("map.dat"),
            announce: Some("@mod:test.map/worlds/map.dat".into()),
        },
        rom: Some(PathBuf::from("rom")),
        content: Some(PathBuf::from("content.json")),
        mode: Mode {
            key: "FFA".into(),
            kind: 6,
            goal: 500,
            time_secs: 3600,
            friendly_fire: false,
            teams: false,
        },
        player: Seat {
            name: "Tester".into(),
            loadout: vec![
                LoadoutItem {
                    slot: Some(0),
                    item: 27,
                },
                LoadoutItem {
                    slot: Some(1),
                    item: 5,
                },
            ],
        },
        bots: Bots {
            count: 2,
            skill: 70,
            name_prefix: "Bot".into(),
        },
        practice: Practice {
            cone_check_off: true,
        },
        server_loadout: vec![27, 5],
        tick_hz: None,
        hosting: Hosting::Solo,
        idle_secs: Some(90),
        team_empty_secs: Some(3600),
        log: Some(PathBuf::from("server.log")),
        verbose: true,
    }
}

fn tickets() -> Tickets {
    Tickets {
        player: T0,
        bots: vec![T1, T2],
    }
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

#[test]
fn the_launch_line_is_the_practice_launchers_and_the_server_reads_all_of_it() {
    let m = practice();
    let a = m.server_args().unwrap();
    assert!(m.server_env(Some(&T0)).is_err(), "solo: no join secret");
    assert_eq!(m.server_env(None).unwrap(), vec![]);
    assert_eq!(
        a,
        words(
            "-port 0 -bindip 127.0.0.1 -worldfile map.dat -announce @mod:test.map/worlds/map.dat -rom rom -idle 90 \
             -roundsecs 3600 -loadout 27,5 -content content.json -log server.log -verbose -teamempty 3600 \
             -combat cone_check=off"
        )
    );
    let p = dc_dserver::cli::parse(a).unwrap();
    assert!(p.ignored.is_empty(), "{:?}", p.ignored);
    assert_eq!((p.port, p.bind), (0, Ipv4Addr::LOCALHOST));
    assert_eq!(p.world_file, Some(PathBuf::from("map.dat")));
    assert_eq!(
        p.announce_path.as_deref(),
        Some("@mod:test.map/worlds/map.dat")
    );
    assert_eq!(p.rom, Some(PathBuf::from("rom")));
    assert_eq!(p.content, Some(PathBuf::from("content.json")));
    assert_eq!(p.log, Some(PathBuf::from("server.log")));
    assert_eq!(p.server.idle_ms, Some(90_000));
    assert_eq!(p.server.game.round_secs, Some(3600));
    assert_eq!(p.server.game.default_loadout, vec![27, 5]);
    assert_eq!(p.server.game.team_empty_ms, 3_600_000);
    assert_eq!(p.server.game.cone_check, dc_dserver::cone::ConeMode::Off);
    assert!(p.server.game.verbose_hits);
    assert!(!p.server.accept_unlisted, "never an open server");
    assert_eq!(p.server.max_players, None);
}

#[test]
fn a_hosted_match_binds_every_address_on_its_port_with_the_seat_cap() {
    let mut m = practice();
    m.hosting = Hosting::Host {
        port: 29950,
        max_players: 8,
    };
    m.world = World::Rom {
        name: "Worlds\\Synth".into(),
    };
    m.tick_hz = Some(30);
    m.practice = Practice::default();
    assert!(m.server_env(None).is_err(), "hosted: a join secret");
    let a = m.server_args().unwrap();
    let env = m.server_env(Some(&T0)).unwrap();
    assert_eq!(JOIN_SECRET_ENV, dc_dserver::cli::JOIN_SECRET_ENV);
    let hex = seatticket::to_hex(&T0);
    assert!(
        !a.iter().any(|x| x.contains(&hex)),
        "no secret on the command line"
    );
    let p = dc_dserver::cli::parse_with_env(a.clone(), &env).unwrap();
    assert!(p.ignored.is_empty(), "{:?}", p.ignored);
    assert_eq!((p.port, p.bind), (29950, Ipv4Addr::UNSPECIFIED));
    assert_eq!(p.world.as_deref(), Some("Worlds\\Synth"));
    assert_eq!(p.server.max_players, Some(8));
    assert_eq!(p.server.tick_hz, 30);
    assert_eq!(
        p.server.join_secret,
        Some(dc_dserver::session::JoinSecret(T0))
    );
    assert_eq!(p.server.game.cone_check, dc_dserver::cone::ConeMode::Log);
    assert!(!a.iter().any(|x| x == "-combat"));
}

#[test]
fn the_control_lines_are_what_the_server_reads_and_every_seat_has_its_ticket() {
    let m = practice();
    let l = m.pipe_lines(&tickets()).unwrap();
    let hex = |t: &Ticket| seatticket::to_hex(t);
    assert_eq!(
        l,
        vec![
            "room key=FFA type=6 goal=500 ff=0 time=3600".to_string(),
            format!("user 1 1 1 0 0 Tester loadout=0:27,1:5 ticket={}", hex(&T0)),
            format!("user 2 2 2 0 0 Bot1 ticket={}", hex(&T1)),
            format!("user 3 3 3 0 0 Bot2 ticket={}", hex(&T2)),
        ]
    );
    let Control::Room(r) = parse_control(&l[0]) else {
        panic!("not a room line")
    };
    assert_eq!((r.mode_key(), r.goal()), ("FFA", 500));
    assert_eq!(r.get("time"), Some("3600"));
    let users: Vec<_> = l[1..]
        .iter()
        .map(|x| match parse_control(x) {
            Control::User(u) => u,
            o => panic!("{x}: {o:?}"),
        })
        .collect();
    assert_eq!(
        users
            .iter()
            .map(|u| (u.uid, u.nick.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "Tester"), (2, "Bot1"), (3, "Bot2")]
    );
    assert_eq!(
        users[0].loadout_items(),
        Some(vec![(Some(0), 27), (Some(1), 5)])
    );
    assert_eq!(users[1].loadout_items(), None);
    for (u, t) in users.iter().zip([T0, T1, T2]) {
        assert!(u.ticket_accepts(Some(&t)), "{}: its own ticket", u.nick);
        assert!(!u.ticket_accepts(None), "{}: no ticket", u.nick);
        assert!(!u.ticket_accepts(Some(&[0x44; 16])), "{}: another", u.nick);
    }
}

#[test]
fn team_modes_alternate_the_seats() {
    let mut m = practice();
    m.mode.teams = true;
    m.mode.key = "TDM".into();
    m.mode.kind = 0;
    m.bots.count = 3;
    let t = Tickets {
        player: T0,
        bots: vec![T1, T2, [0x44; 16]],
    };
    let teams: Vec<u8> = m.pipe_lines(&t).unwrap()[1..]
        .iter()
        .map(|x| match parse_control(x) {
            Control::User(u) => u.team,
            o => panic!("{o:?}"),
        })
        .collect();
    assert_eq!(teams, vec![0, 1, 0, 1]);
}

#[test]
fn a_home_host_takes_at_most_eight_players() {
    let host = |max_players: u8, bots: u8| {
        let mut m = practice();
        m.hosting = Hosting::Host {
            port: 29950,
            max_players,
        };
        m.bots.count = bots;
        m.validate()
    };
    assert!(host(8, 7).is_ok());
    assert!(host(9, 0).is_err(), "above the home-host cap");
    assert!(host(1, 0).is_err(), "nobody could join");
    assert!(host(4, 4).is_err(), "the bots take the seats of the cap");
    let mut m = practice();
    m.hosting = Hosting::Host {
        port: 0,
        max_players: 4,
    };
    assert!(m.validate().is_err(), "a hosted match needs a fixed port");
    // solo: the player and up to 15 bots
    let mut m = practice();
    m.bots.count = 15;
    assert!(m.validate().is_ok());
    m.bots.count = 16;
    assert!(m.validate().is_err());
}

#[test]
fn bad_descriptions_are_refused() {
    let check = |f: fn(&mut LocalMatch)| {
        let mut m = practice();
        f(&mut m);
        let e = m.validate().expect_err("refused");
        assert!(m.server_args().is_err() && m.pipe_lines(&tickets()).is_err());
        e
    };
    check(|m| m.player.name = String::new());
    check(|m| m.player.name = "Two Words".into());
    check(|m| m.player.name = "x".repeat(NAME_MAX_CHARS + 1));
    check(|m| m.player.name = "Bot1".into());
    check(|m| m.mode.key = "T DM".into());
    check(|m| m.mode.goal = 0);
    check(|m| m.mode.time_secs = 0);
    check(|m| m.bots.skill = 101);
    check(|m| m.tick_hz = Some(0));
    check(|m| m.tick_hz = Some(241));
    check(|m| m.world = World::Rom { name: " ".into() });
    check(|m| {
        m.player.loadout = vec![
            LoadoutItem {
                slot: None,
                item: 1
            };
            LOADOUT_MAX + 1
        ]
    });
    // the tickets must match the bots
    let m = practice();
    assert!(m
        .pipe_lines(&Tickets {
            player: T0,
            bots: vec![T1]
        })
        .is_err());
}

#[test]
fn the_description_reads_from_json_with_defaults() {
    let m: LocalMatch = serde_json::from_str(
        r#"{"world": {"rom": {"name": "Worlds\\Synth"}},
            "mode": {"key": "TDM", "goal": 50, "time_secs": 600},
            "player": {"name": "Me"},
            "bots": {"count": 3}}"#,
    )
    .unwrap();
    assert_eq!(m.hosting, Hosting::Solo);
    assert!(m.mode.teams && !m.mode.friendly_fire);
    assert_eq!(m.bots.skill, BOT_SKILL_STANDARD);
    assert_eq!(m.bot_names(), ["Bot1", "Bot2", "Bot3"]);
    m.validate().unwrap();
    let back: LocalMatch =
        serde_json::from_str(&serde_json::to_string(&practice()).unwrap()).unwrap();
    assert_eq!(back, practice());
    assert!(serde_json::from_str::<LocalMatch>(
        r#"{"world": {"rom": {"name": "w"}}, "mode": {"key": "TDM", "goal": 1, "time_secs": 1},
            "player": {"name": "Me"}, "surprise": 1}"#
    )
    .is_err());
}
