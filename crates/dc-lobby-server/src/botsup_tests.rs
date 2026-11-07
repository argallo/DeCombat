//! Tests of the bot supervisor with a fake `dc-botclient`: this test binary itself, started on the ignored test
//! `fake_bot`, which writes its flags to a file and then behaves as told (ends with its pipe, ends at once, or ignores
//! its pipe until killed).
// Provenance: ORIGINAL (DeCombat tests)

use super::*;
use crate::botgroups::tests::three_entries;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc-botsup-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// How the fake bot of a name behaves: `pipe` (ends when its stdin closes), `exit` (ends at once), `deaf` (ignores
/// its stdin and sleeps).
struct Fake {
    dir: PathBuf,
    modes: BTreeMap<String, &'static str>,
}

impl Launch for Fake {
    fn launch(&self, name: &str, args: &[String]) -> std::io::Result<Child> {
        let mode = self.modes.get(name).copied().unwrap_or("pipe");
        Command::new(std::env::current_exe()?)
            .args([
                "botsup::tests::fake_bot",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads",
                "1",
                "-q",
            ])
            .env("DC_FAKE_BOT_DIR", &self.dir)
            .env("DC_FAKE_BOT_NAME", name)
            .env("DC_FAKE_BOT_ARGS", args.join("\n"))
            .env("DC_FAKE_BOT_MODE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }
}

#[test]
#[ignore = "the fake dc-botclient of the supervisor tests: does nothing when run by hand"]
fn fake_bot() {
    let (Ok(dir), Ok(name)) = (
        std::env::var("DC_FAKE_BOT_DIR"),
        std::env::var("DC_FAKE_BOT_NAME"),
    ) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let args = std::env::var("DC_FAKE_BOT_ARGS").unwrap_or_default();
    std::fs::write(dir.join(format!("{name}.args")), &args).unwrap();
    use std::io::Write;
    let mut starts = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{name}.starts")))
        .unwrap();
    writeln!(starts, "{}", std::process::id()).unwrap();
    match std::env::var("DC_FAKE_BOT_MODE").as_deref() {
        Ok("exit") => std::process::exit(3),
        Ok("deaf") => std::thread::sleep(Duration::from_secs(60)),
        _ => {
            let mut sink = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut sink);
            let _ = std::fs::write(dir.join(format!("{name}.end")), "pipe closed");
        }
    }
    std::process::exit(0);
}

fn quick() -> Timing {
    Timing {
        restart_min: Duration::from_millis(150),
        restart_max: Duration::from_millis(600),
        ran_well: Duration::from_secs(30),
        stop_grace: Duration::from_millis(1500),
        stagger: Duration::ZERO,
    }
}

fn sup(dir: &Path, modes: &[(&str, &'static str)]) -> Supervisor {
    let modes = modes.iter().map(|(n, m)| (n.to_string(), *m)).collect();
    Supervisor::new(
        Box::new(Fake {
            dir: dir.to_path_buf(),
            modes,
        }),
        quick(),
    )
}

fn want(name: &str, channel: u32) -> Want {
    Want {
        name: name.into(),
        group: 1,
        channel,
        args: vec![
            "--name".into(),
            name.into(),
            "--channel".into(),
            channel.to_string(),
        ],
    }
}

fn starts(dir: &Path, name: &str) -> usize {
    std::fs::read_to_string(dir.join(format!("{name}.starts"))).map_or(0, |s| s.lines().count())
}

/// Runs reconcile rounds with `wants` until `done` holds (at most 20 s).
fn until(s: &mut Supervisor, wants: &[Want], mut done: impl FnMut(&Supervisor) -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        s.reconcile(wants.to_vec(), Instant::now());
        if done(s) {
            return;
        }
        assert!(
            Instant::now() < end,
            "timed out: {:?}",
            s.status(Instant::now())
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn file_says(dir: &Path, f: &str) -> bool {
    dir.join(f).is_file()
}

#[test]
fn the_supervisor_runs_count_bots_and_stops_the_extra_on_resize_down() {
    let dir = scratch("resize");
    let mut s = sup(&dir, &[]);
    let w = vec![
        want("PlusBot1", 4),
        want("PlusBot2", 6),
        want("PlusBot3", 4),
    ];
    until(&mut s, &w, |_| {
        (1..=3).all(|i| file_says(&dir, &format!("PlusBot{i}.args")))
    });
    let st = s.status(Instant::now());
    assert_eq!(st.len(), 3);
    assert!(
        st.values().all(|p| p.state == "running" && p.pid.is_some()),
        "{st:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("PlusBot2.args")).unwrap(),
        "--name\nPlusBot2\n--channel\n6"
    );
    // down to two, and bot 2 moves to channel 5: bot 3 ends by its pipe, bot 2 restarts, bot 1 runs on
    let pid1 = st["PlusBot1"].pid;
    let w2 = vec![want("PlusBot1", 4), want("PlusBot2", 5)];
    until(&mut s, &w2, |_| {
        file_says(&dir, "PlusBot3.end") && starts(&dir, "PlusBot2") == 2
    });
    until(&mut s, &w2, |s| {
        s.status(Instant::now())
            .values()
            .all(|p| p.state == "running")
    });
    let st = s.status(Instant::now());
    assert_eq!(
        st.keys().cloned().collect::<Vec<_>>(),
        vec!["PlusBot1", "PlusBot2"]
    );
    assert_eq!(st["PlusBot1"].pid, pid1, "an unchanged bot is not touched");
    assert_eq!((starts(&dir, "PlusBot1"), starts(&dir, "PlusBot3")), (1, 1));
    assert!(
        file_says(&dir, "PlusBot2.end"),
        "the old process ended by its pipe before the new one started"
    );
    assert!(std::fs::read_to_string(dir.join("PlusBot2.args"))
        .unwrap()
        .ends_with("--channel\n5"));
    assert_eq!(st["PlusBot2"].restarts, 0, "a settings change is no crash");
    // a group stopped: nothing wanted, every pipe closes
    until(&mut s, &[], |s| s.status(Instant::now()).is_empty());
    assert!(file_says(&dir, "PlusBot1.end"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bot_that_ends_starts_again_after_a_growing_wait() {
    let dir = scratch("restart");
    let mut s = sup(&dir, &[("QuitBot1", "exit")]);
    let w = vec![want("QuitBot1", 1)];
    let t0 = Instant::now();
    until(&mut s, &w, |_| starts(&dir, "QuitBot1") >= 3);
    // waits 150 ms, then 300 ms (fast exits in a row double it): at least 450 ms for three starts
    assert!(
        t0.elapsed() >= Duration::from_millis(440),
        "{:?}",
        t0.elapsed()
    );
    until(&mut s, &w, |s| {
        s.status(Instant::now())["QuitBot1"].state == "backoff"
    });
    let p = &s.status(Instant::now())["QuitBot1"];
    assert!(p.restarts >= 2, "{p:?}");
    assert!(p.last_exit.contains('3'), "the exit code is shown: {p:?}");
    assert_eq!(s.wait_after(1), Duration::from_millis(150));
    assert_eq!(s.wait_after(2), Duration::from_millis(300));
    assert_eq!(s.wait_after(9), Duration::from_millis(600), "capped");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_graceful_stop_ends_every_bot_and_kills_one_that_does_not_listen() {
    let dir = scratch("stop");
    let mut s = sup(&dir, &[("DeafBot1", "deaf")]);
    let w = vec![want("PlusBot1", 1), want("DeafBot1", 1)];
    until(&mut s, &w, |_| {
        file_says(&dir, "PlusBot1.args") && file_says(&dir, "DeafBot1.args")
    });
    let t0 = Instant::now();
    assert_eq!(s.stop_all(), 2);
    let took = t0.elapsed();
    assert!(
        took >= Duration::from_millis(1400) && took < Duration::from_secs(8),
        "the deaf one waited out the grace: {took:?}"
    );
    assert!(
        file_says(&dir, "PlusBot1.end"),
        "the listening bot ended by its pipe"
    );
    assert!(!file_says(&dir, "DeafBot1.end"));
    assert!(s.status(Instant::now()).is_empty());
    // no start after the stop
    assert!(s.reconcile(w, Instant::now()).is_empty());
    assert_eq!(starts(&dir, "PlusBot1"), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bot_that_cannot_start_waits_and_says_why() {
    struct Missing;
    impl Launch for Missing {
        fn launch(&self, _: &str, _: &[String]) -> std::io::Result<Child> {
            Command::new("dc-no-such-bot-binary-x").spawn()
        }
    }
    let mut s = Supervisor::new(Box::new(Missing), quick());
    let lines = s.reconcile(vec![want("PlusBot1", 1)], Instant::now());
    assert!(lines[0].contains("cannot start"), "{lines:?}");
    let p = &s.status(Instant::now())["PlusBot1"];
    assert_eq!((p.state, p.pid), ("backoff", None));
}

fn managed(dir: &Path) -> Core {
    let mut st = three_entries();
    st.opts.bots_managed = true;
    st.opts.port = 10000;
    st.opts.bind = "0.0.0.0".into();
    st.opts.db = dir.join("ca.db");
    st
}

fn group(prefix: &str, count: u32, server: u32, channels: &[u32], room: RoomPick) -> Group {
    Group {
        id: 0,
        prefix: prefix.into(),
        count,
        server_id: server,
        channels: channels.to_vec(),
        room,
        skill: "40-60".into(),
        loadout: String::new(),
        enabled: true,
    }
}

fn flag<'a>(args: &'a [String], k: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == k)
        .map(|i| args[i + 1].as_str())
}

#[test]
fn each_bot_gets_the_flags_of_bots_sh_and_its_groups_channel_round_robin() {
    let dir = scratch("args");
    let st = managed(&dir);
    let mut a = group("PlusBot", 3, 2, &[4, 6], RoomPick::Any);
    a.id = 1;
    let mut b = group("FollowBot", 1, 3, &[9], RoomPick::Follow("Zoe".into()));
    b.id = 2;
    let mut c = group("FunBot", 2, 1, &[2], RoomPick::Title("fun".into()));
    c.id = 3;
    let mut off = group("IdleBot", 2, 1, &[1], RoomPick::Any);
    off.id = 4;
    off.enabled = false;
    let f = facts(&st, &dir.join("bots"));
    assert_eq!(f.lobby, "127.0.0.1:10000");
    assert_eq!(f.mods, None, "a vanilla lobby passes no hashes");
    let w = wants(&st, &[a, b, c, off], &f, &BTreeMap::new());
    let names: Vec<(&str, u32)> = w.iter().map(|w| (w.name.as_str(), w.channel)).collect();
    assert_eq!(
        names,
        vec![
            ("PlusBot1", 4),
            ("PlusBot2", 6),
            ("PlusBot3", 4),
            ("FollowBot1", 9),
            ("FunBot1", 2),
            ("FunBot2", 2)
        ]
    );
    let p2 = &w[1].args;
    assert_eq!(p2[0], "--parent-pipe");
    for (k, v) in [
        ("--lobby", "127.0.0.1:10000"),
        ("--name", "PlusBot2"),
        ("--room", "any"),
        ("--matches", "0"),
        ("--skill", "40-60"),
        ("--respawn-after", "0"),
        ("--filler", "PlusBot"),
        ("--server-id", "2"),
        ("--channel", "6"),
        ("--duration", "3600"),
    ] {
        assert_eq!(flag(p2, k), Some(v), "{k} in {p2:?}");
    }
    assert!(p2.contains(&"--shooter".to_string()) && p2.contains(&"--hunt".to_string()));
    assert!(flag(p2, "--log")
        .unwrap()
        .ends_with("PlusBot2.events.jsonl"));
    assert_eq!(flag(p2, "--room-title"), None);
    assert_eq!(flag(&w[3].args, "--filler"), Some("=Zoe"));
    assert_eq!(flag(&w[4].args, "--room-title"), Some("fun"));
    assert_eq!(flag(&w[4].args, "--filler"), Some("FunBot"));
    // the total cap in group order; nothing on a lobby without bots.managed or while it stops
    let mut capped = managed(&dir);
    capped.opts.bots_max_total = 4;
    let mut a2 = group("PlusBot", 3, 2, &[4], RoomPick::Any);
    a2.id = 1;
    let mut b2 = group("WestBot", 3, 2, &[5], RoomPick::Any);
    b2.id = 2;
    let groups = [a2, b2];
    assert_eq!(
        wants(&capped, &groups, &f, &BTreeMap::new())
            .iter()
            .map(|w| w.name.clone())
            .collect::<Vec<_>>(),
        vec!["PlusBot1", "PlusBot2", "PlusBot3", "WestBot1"]
    );
    capped.stopping = true;
    assert!(wants(&capped, &groups, &f, &BTreeMap::new()).is_empty());
    capped.stopping = false;
    capped.opts.bots_managed = false;
    assert!(wants(&capped, &groups, &f, &BTreeMap::new()).is_empty());
    // a modded lobby's bots carry its hashes; a lobby on one address is reached there
    let mut modded = managed(&dir);
    modded.login_mods.set = "ab".repeat(32);
    modded.login_mods.content = "0".into();
    let mf = facts(&modded, &dir);
    let mw = wants(&modded, &groups[..1], &mf, &BTreeMap::new());
    assert_eq!(
        flag(&mw[0].args, "--mod-set-hash"),
        Some("ab".repeat(32).as_str())
    );
    assert_eq!(flag(&mw[0].args, "--content-hash"), Some("0"));
    assert_eq!(lobby_addr("192.0.2.5", 10000), "192.0.2.5:10000");
    assert_eq!(lobby_addr("::", 10000), "127.0.0.1:10000");
    assert_eq!(lobby_addr("::1", 10003), "[::1]:10003");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn loadouts_are_given_once_per_spec_after_the_first_login() {
    let dir = scratch("loadout");
    let st = managed(&dir);
    let mut g = group("PlusBot", 2, 1, &[1], RoomPick::Any);
    g.loadout = "0:27".into();
    g.id = crate::botgroups::save(&st.db, &g, 1).unwrap();
    let f = facts(&st, &dir);
    let groups = vec![g.clone()];
    let w = wants(&st, &groups, &f, &BTreeMap::new());
    let mut lo = Loadouts::default();
    let (restart, notes) = lo.apply(&st, &groups, &w);
    assert!(restart.is_empty());
    assert_eq!(notes[0].1, "loadout 0:27 after the first login");
    // PlusBot1's first login made its account and character: the kit goes on, the bot restarts
    let acc = crate::db::create_account(&st.db, "plusbot1", 1).unwrap();
    crate::db::create_character(
        &st.db,
        &crate::db::NewCharacter {
            user_no: acc,
            nick: "PlusBot1",
            face: 0,
            exp: 0,
            gp: 0,
            nx: 0,
            now: 0,
        },
        true,
    )
    .unwrap();
    let (restart, notes) = lo.apply(&st, &groups, &w);
    assert_eq!(restart, vec!["PlusBot1"]);
    assert_eq!(notes[0].1, "loadout 0:27");
    assert_eq!(
        crate::botgroups::applied_loadout(&st.db, "plusbot1").unwrap(),
        "0:27"
    );
    // once only, also after a lobby restart (a new Loadouts reads the table)
    assert!(lo.apply(&st, &groups, &w).0.is_empty());
    assert!(Loadouts::default().apply(&st, &groups, &w).0.is_empty());
    // a spec the lobby cannot resolve is refused once and shown
    let bad = vec![Group {
        loadout: "27".into(),
        ..g
    }];
    let (restart, notes) = lo.apply(&st, &bad, &w);
    assert!(restart.is_empty());
    assert!(notes[0].1.starts_with("loadout 27 refused"), "{notes:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_lobby_thread_runs_the_stored_groups_and_its_stop_ends_them() {
    let dir = scratch("tick");
    let st = managed(&dir);
    let mut g = group("PlusBot", 2, 2, &[4, 5], RoomPick::Any);
    g.id = crate::botgroups::save(&st.db, &g, 1).unwrap();
    let shared: Shared = Arc::new(Mutex::new(st));
    let h: Handle = Arc::new(Mutex::new(sup(&dir, &[])));
    crate::net::lock(&shared).bot_sup = Some(Arc::clone(&h));
    let mut lo = Loadouts::default();
    let mut fo = Follows::default();
    let end = Instant::now() + Duration::from_secs(20);
    while !(file_says(&dir, "PlusBot1.args") && file_says(&dir, "PlusBot2.args")) {
        assert!(tick(&shared, &h, &mut lo, &mut fo, &dir));
        assert!(Instant::now() < end, "the bots did not start");
        std::thread::sleep(Duration::from_millis(30));
    }
    let a2 = std::fs::read_to_string(dir.join("PlusBot2.args")).unwrap();
    assert!(
        a2.contains("--channel\n5") && a2.contains("--server-id\n2"),
        "{a2}"
    );
    let v = crate::botgroups::status_json(&shared);
    let b = &v["groups"][0]["bots"];
    assert_eq!(
        (b[0]["state"].as_str(), b[1]["state"].as_str()),
        (Some("running"), Some("running")),
        "{v}"
    );
    assert!(b[0]["pid"].as_u64().is_some());
    assert_eq!(v["running"], 2);
    // the graceful stop's part: both end by their pipes, the thread's next round ends it
    shutdown(&shared);
    assert!(file_says(&dir, "PlusBot1.end") && file_says(&dir, "PlusBot2.end"));
    assert!(!tick(&shared, &h, &mut lo, &mut fo, &dir));
    assert_eq!((starts(&dir, "PlusBot1"), starts(&dir, "PlusBot2")), (1, 1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_following_group_moves_to_the_players_entry_and_channel_and_stays_while_they_are_away() {
    use crate::botgroups::tests::player_at;
    let dir = scratch("follow");
    let mut st = managed(&dir);
    let mut g = group("ZoeBot", 2, 1, &[1, 2], RoomPick::Follow("Zoe".into()));
    g.id = 4;
    let mut any = group("PlusBot", 1, 1, &[3], RoomPick::Any);
    any.id = 5;
    let groups = vec![g.clone(), any];
    let f = facts(&st, &dir);
    let mut fo = Follows::default();
    let t0 = Instant::now();
    let at = |w: &[Want], i: usize| {
        (
            flag(&w[i].args, "--server-id").map(str::to_string),
            flag(&w[i].args, "--channel").map(str::to_string),
            w[i].channel,
        )
    };
    let s = |v: &str| Some(v.to_string());
    // not online here: the bots sit on the group's own entry and channels
    let (spots, moves) = fo.update(&st, &groups, t0);
    assert!(moves.is_empty());
    let w = wants(&st, &groups, &f, &spots);
    assert_eq!(at(&w, 0), (s("1"), s("1"), 1));
    assert_eq!(at(&w, 1), (s("1"), s("2"), 2));
    assert_eq!(
        at(&w, 2),
        (s("1"), s("3"), 3),
        "a group that follows nobody"
    );
    // Zoe goes to Vanilla++ (entry 3) channel 8: both bots go there at once, the other group stays
    let _a = player_at(&mut st, 11, "Zoe", Some(8), None);
    let (spots, moves) = fo.update(&st, &groups, t0);
    assert_eq!(
        moves,
        vec!["group 4 ZoeBot follows Zoe to Vanilla++ channel 8"]
    );
    let w = wants(&st, &groups, &f, &spots);
    assert_eq!(at(&w, 0), (s("3"), s("8"), 8));
    assert_eq!(at(&w, 1), (s("3"), s("8"), 8));
    assert_eq!(flag(&w[0].args, "--filler"), Some("=Zoe"));
    assert_eq!(at(&w, 2), (s("1"), s("3"), 3));
    // she hops to US West channel 5 at once: the move waits for the gap, then follows
    let _b = player_at(&mut st, 12, "Zoe", Some(5), Some((1, "")));
    let (spots, moves) = fo.update(&st, &groups, t0 + Duration::from_secs(3));
    assert!(moves.is_empty());
    assert_eq!(spots[&4].channels, vec![8]);
    let (spots, moves) = fo.update(&st, &groups, t0 + Duration::from_secs(10));
    assert_eq!(moves.len(), 1, "{moves:?}");
    let w = wants(&st, &groups, &f, &spots);
    assert_eq!(at(&w, 1), (s("2"), s("5"), 5));
    // she logs off: the bots stay on channel 5
    let _c = player_at(&mut st, 13, "Zoe", None, None);
    let (spots, moves) = fo.update(&st, &groups, t0 + Duration::from_secs(60));
    assert!(moves.is_empty());
    assert_eq!(
        spots[&4],
        Spot {
            entry: 2,
            channels: vec![5]
        }
    );
    // an edit of the group's entry or channels starts it again from them; a stopped group is forgotten
    let edited = vec![Group {
        channels: vec![2],
        ..g.clone()
    }];
    let (spots, _) = fo.update(&st, &edited, t0 + Duration::from_secs(61));
    assert_eq!(
        spots[&4],
        Spot {
            entry: 1,
            channels: vec![2]
        }
    );
    let stopped = vec![Group {
        enabled: false,
        ..g
    }];
    assert!(fo
        .update(&st, &stopped, t0 + Duration::from_secs(62))
        .0
        .is_empty());
    assert!(fo.steer.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
