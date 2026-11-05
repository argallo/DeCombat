//! ROM-gated (skipped without CA_ROM): the co-op round world (the world whose GMS has spawn tables, the Cabin Fever
//! room's) runs its own round loop from its trigger network (DSV-576..DSV-578). One human holds the start; every
//! AI the rounds send is killed as soon as it stands. Checked per difficulty: the server's start box counts round 1,
//! each cleared round runs the world's round end (a heal, the next round's box), the round counter and the kill
//! counter move, the round-11 event of the harder rows runs (a life, the cut-scene spawners' AI), and the difficulty's
//! last round ends the mission with success through the world's own ending (Normal after round 10, Hard after
//! round 15, Extreme after round 20). What the human is shown (DSV-579..DSV-584): the round captions, a
//! "Wave n / N" banner counting to the difficulty's last round, the music, the cut scenes' camera with its field of
//! view; the GMS weapon rows lie as drops from round 11 (none on Normal). No ROM name is written here: the world and
//! its counters are found by their role in the network.
// OBSERVED: ROM CabinFever (its triggers, event counters, AI templates and GMS rows, read at run time)
// DC-DEFAULT: DSV-576..DSV-578 (proposed)

use dc_bute::GameRoomData;
use dc_dserver::game::coop::{AiState, STAGE_COMPLETE, STAGE_TIMEOUT};
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{load_from_rom, NameForm, WorldInfo};
use dc_net_engine::sim::{Faults, SimChannel};
use dc_rez::RezVfs;

/// The co-op mode key and the round world (a co-op world whose statements carry a round loop).
fn round_world(rom: &std::path::Path) -> Option<(String, WorldInfo)> {
    let vfs = RezVfs::new(rom);
    let gr = GameRoomData::from_vfs(&vfs).ok()?;
    let modes = gr.modes(false);
    let (mi, key) = modes.iter().enumerate().find_map(|(i, m)| {
        let t = m.get("GameType")?.trim().trim_matches('"').to_string();
        let k = m.get("Key")?.trim().trim_matches('"').to_string();
        (t == "5").then_some((i, k))
    })?;
    for (i, m) in gr.maps().iter().enumerate() {
        if !m.contains_key(&format!("GMS_{key}")) {
            continue;
        }
        let (_md, doc) = dc_bute::gms_file_with_vfs(&vfs, &gr, i, mi, false);
        let Some(world) = doc.get_str("MapInfo", "Map") else {
            continue;
        };
        let w = world.trim().trim_matches('"').replace('/', "\\");
        let base = w.rsplit('\\').next().unwrap_or(&w).to_string();
        let base = base
            .strip_suffix(".dat")
            .or_else(|| base.strip_suffix(".DAT"))
            .unwrap_or(&base)
            .to_string();
        let Ok(info) = load_from_rom(rom, &format!("Worlds\\{base}"), NameForm::Listed, None)
        else {
            continue;
        };
        if info.coop.rounds.is_some() {
            return Some((key, info));
        }
    }
    None
}

/// The room line's `trig=` field as the lobby builds it from the world's GMS.
// SPEC: DSERVER_PIPE.md §3.2 (room line); OBSERVED: ROM table key GMS [Mission0] StartTrigger / Controller,
// [Object<n>] StartTrigger / EndTrigger
fn trig_field(rom: &std::path::Path, key: &str, world: &str, goal: u32) -> String {
    let vfs = RezVfs::new(rom);
    let Some(doc) = dc_dserver::coop_rom::find_gms(&vfs, key, world) else {
        return String::new();
    };
    let get = |s: &str, k: &str| {
        doc.get_str(s, k)
            .map(|v| v.trim().trim_matches('"').to_string())
            .unwrap_or_default()
    };
    let o = format!("Object{goal}");
    format!(
        " trig={}|{}|{}|{}",
        get("Mission0", "StartTrigger"),
        get(&o, "StartTrigger"),
        get(&o, "EndTrigger"),
        get("Mission0", "Controller")
    )
}

struct Peer {
    client: SynthClient,
    up: SimChannel,
    down: SimChannel,
}

fn step(server: &mut Server<u32>, p: &mut Peer, now: u32) {
    for d in p.client.poll(now) {
        p.up.send(now, d);
    }
    for d in p.up.deliver(now) {
        server.handle_datagram(now, 1, &d);
    }
    for o in server.poll(now) {
        match o {
            Output::Transmit { bytes, .. } => p.down.send(now, bytes),
            Output::Log(l) if std::env::var("ROUNDS_LOG").is_ok() => println!("{now}: {l}"),
            _ => {}
        }
    }
    for d in p.down.deliver(now) {
        p.client.handle_datagram(now, &d);
    }
}

/// What one match reached.
#[derive(Debug)]
struct Outcome {
    stage: Option<u8>,
    round: u32,
    cleared: u32,
    round_count: i32,
    kill_count: i32,
    killed: u32,
    heals: u32,
    /// The lives at the start and after each round that gave one.
    lives: Vec<u32>,
    /// The statements' own spawns (the cut-scene spawners).
    statement_spawns: u32,
    /// The round when the statements first spawned AI.
    first_statement_round: Option<u32>,
    /// The weapon rows laid out (DSV-583) and the round of the first.
    weapon_drops: u32,
    first_drop_round: Option<u32>,
    /// What the client read: captions, banners, music plays, camera fields of view; reader errors.
    captions: usize,
    banners: Vec<String>,
    music_plays: usize,
    camera_fov: usize,
    errors: Vec<String>,
    hide_changes: u32,
}

fn play(rom: &std::path::Path, key: &str, w: &WorldInfo, goal: u32) -> Outcome {
    let rounds = w.coop.rounds.clone().expect("a round world");
    let (tables, _) = RomTables::load(rom);
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    // the test holds the start: the AI do not hit the human
    cfg.game.coop.hit_chance = [0.0; 3];
    cfg.game.coop.melee_hit = 0.0;
    cfg.game.fall.enabled = false;
    let trig = trig_field(rom, key, &w.announce, goal);
    let mut server = Server::with_tables(cfg, w.clone(), tables, 0);
    assert!(server.handle_line(
        0,
        &format!("room key={key} type=5 goal={goal} ff=0 time=36000{trig}")
    ));
    assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
    let mut c = SynthConfig::named("Dave");
    c.box_bounds = Some((w.extent_min, w.extent_max));
    c.walk_step = 0.0;
    c.mode_class_hint = Some(5);
    let mut p = Peer {
        client: SynthClient::new(c),
        up: SimChannel::new(3, Faults::clean(2)),
        down: SimChannel::new(4, Faults::clean(2)),
    };
    let mut now = 0u32;
    while now < 30_000 && p.client.life_state() != 1 {
        step(&mut server, &mut p, now);
        now += 10;
    }
    assert_eq!(p.client.life_state(), 1, "alive");
    let pid = server.game().players().values().next().unwrap().pid;
    let start = server.game().players()[&pid].pos;
    let counter = |s: &Server<u32>, name: &[u8]| {
        let net = &s.world_statements().net;
        net.objects
            .counters
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
            .map_or(-1, |i| net.counters[i])
    };
    let mut lives = vec![server.game().coop_lives_of(pid)];
    let mut killed = 0u32;
    let mut first_statement_round = None;
    let mut first_drop_round = None;
    // DC-DEFAULT: the cap is far above the longest difficulty (20 rounds of at most ~110 s and the 12 s breaks)
    let cap = 3 * 3600 * 1000;
    while now < cap {
        let coop = server.game().coop().expect("a co-op room");
        if coop.stage.is_some() {
            break;
        }
        // hold the start
        p.client.set_height(start[1]);
        p.client.walk_to(Some(start), 100_000.0);
        step(&mut server, &mut p, now);
        now += 10;
        if now.is_multiple_of(250) {
            let alive: Vec<usize> = server
                .game()
                .coop()
                .unwrap()
                .ais
                .iter()
                .filter(|a| a.state == AiState::Alive)
                .map(|a| a.slot)
                .collect();
            for s in alive {
                server.harness_kill_ai(now, s, pid);
                killed += 1;
            }
            let c = server.game().coop().unwrap();
            if first_statement_round.is_none() && c.mission_spawns > 0 {
                first_statement_round = Some(c.round);
            }
            if first_drop_round.is_none() && c.show.weapon_drops > 0 {
                first_drop_round = Some(c.round);
            }
            let l = server.game().coop_lives_of(pid);
            if l != *lives.last().unwrap() {
                lives.push(l);
            }
        }
    }
    let c = server.game().coop().unwrap().clone();
    let out = Outcome {
        stage: c.stage,
        round: c.round,
        cleared: c.waves_cleared,
        round_count: counter(&server, &rounds.round_counter),
        kill_count: counter(&server, &rounds.kill_counter),
        killed,
        heals: c.heals,
        lives,
        statement_spawns: c.mission_spawns,
        first_statement_round,
        weapon_drops: c.show.weapon_drops,
        first_drop_round,
        captions: p.client.seen.captions.len(),
        banners: p
            .client
            .seen
            .banners
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .filter(|b| b.starts_with("Wave"))
            .collect(),
        music_plays: p.client.seen.music.iter().filter(|m| m.1.is_some()).count(),
        camera_fov: p.client.seen.camera_fov.len(),
        errors: p.client.seen.errors.clone(),
        hide_changes: c.show.hide_changes,
    };
    println!(
        "difficulty {goal}: {out:?} in {} s; {} statements skipped, {} client-only",
        now / 1000,
        server.world_statements().net.skipped,
        server.world_statements().net.client_only
    );
    out
}

#[test]
fn rom_cabin_fever_runs_its_round_loop_to_each_difficultys_end() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some((key, w)) = round_world(&rom) else {
        println!("skipped: no co-op round world");
        return;
    };
    let r = w.coop.rounds.clone().unwrap();
    println!(
        "round world: {} start box(es), {} round-end events",
        r.start_boxes.len(),
        r.clear_events.len()
    );
    assert_eq!(r.start_boxes.len(), 1, "one box counts round 1");
    assert_eq!(
        r.clear_events.len(),
        20,
        "a round end per spawn-table round"
    );
    for (goal, last) in [(0u32, 10u32), (1, 15), (2, 20)] {
        let o = play(&rom, &key, &w, goal);
        // DSV-576: the world's round counter ran the rounds, each cleared round ended by the world's event
        assert_eq!(o.stage, Some(STAGE_COMPLETE), "difficulty {goal}: {o:?}");
        assert_eq!(o.round, last, "difficulty {goal}: the last round");
        assert_eq!(o.round_count, last as i32);
        assert_eq!(o.cleared, last);
        // DSV-576: every spawn-table AI's death counted the kill counter (the cut-scene AI have no death command)
        assert!(o.kill_count > 0 && o.kill_count as u32 <= o.killed);
        assert_eq!(
            o.kill_count as u32 + o.statement_spawns,
            o.killed,
            "difficulty {goal}"
        );
        // DSV-578: a heal at the round ends that do not give a life
        assert!(o.heals >= last / 2, "difficulty {goal}: {} heals", o.heals);
        // DSV-572 through the round ends and the round-11 / round-16 boxes: lives
        assert!(o.lives.windows(2).all(|p| p[1] > p[0]), "{:?}", o.lives);
        if goal == 0 {
            assert_eq!(o.statement_spawns, 0, "Normal ends before round 11");
        } else {
            // the round-11 event: its cut-scene spawners
            assert_eq!(o.first_statement_round, Some(11), "difficulty {goal}");
            assert!(o.statement_spawns >= 4);
        }
        // DSV-579..DSV-582: the client read every form; a caption per round at least; the music; the cut scenes'
        // camera with its field of view (the ending's at least); `hidecharacter` taken (logged only)
        assert!(o.errors.is_empty(), "difficulty {goal}: {:?}", o.errors);
        assert!(
            o.captions as u32 >= last,
            "difficulty {goal}: {} captions",
            o.captions
        );
        assert!(o.music_plays >= 2, "difficulty {goal}");
        assert!(o.camera_fov >= 1, "difficulty {goal}");
        assert!(o.hide_changes >= 2, "difficulty {goal}");
        // DSV-584: the wave banner counts to the difficulty's last round
        assert_eq!(
            o.banners.len() as u32,
            last,
            "difficulty {goal}: {:?}",
            o.banners
        );
        assert_eq!(o.banners[0], format!("Wave 1 / {last}"));
        assert!(o.banners.iter().all(|b| b.ends_with(&format!("/ {last}"))));
        // DSV-583: the weapon rows from round 11 (two of them on Hard, five on Extreme)
        let want = [0u32, 2, 5][goal as usize];
        assert_eq!(o.weapon_drops, want, "difficulty {goal}");
        assert_eq!(o.first_drop_round, (want > 0).then_some(11));
    }
}

/// One synthetic client of the two-player match, by its address.
struct Peer2 {
    addr: u32,
    client: SynthClient,
    up: SimChannel,
    down: SimChannel,
}

fn step_all(server: &mut Server<u32>, peers: &mut [Peer2], now: u32) {
    for p in peers.iter_mut() {
        for d in p.client.poll(now) {
            p.up.send(now, d);
        }
    }
    for p in peers.iter_mut() {
        for d in p.up.deliver(now) {
            server.handle_datagram(now, p.addr, &d);
        }
    }
    for o in server.poll(now) {
        match o {
            Output::Transmit { to, bytes } => {
                if let Some(p) = peers.iter_mut().find(|p| p.addr == to) {
                    p.down.send(now, bytes);
                }
            }
            Output::Log(l) if std::env::var("ROUNDS_LOG").is_ok() => println!("{now}: {l}"),
            _ => {}
        }
    }
    for p in peers.iter_mut() {
        for d in p.down.deliver(now) {
            p.client.handle_datagram(now, &d);
        }
    }
}

/// A server on the round world for `room` (the line after `room `) with the humans `names` in the world and alive,
/// the AI harmless; the time reached.
fn two_up(
    rom: &std::path::Path,
    w: &WorldInfo,
    room: &str,
    names: &[&str],
) -> (Server<u32>, Vec<Peer2>, u32) {
    let (tables, _) = RomTables::load(rom);
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    cfg.game.coop.hit_chance = [0.0; 3];
    cfg.game.coop.melee_hit = 0.0;
    cfg.game.fall.enabled = false;
    let mut server = Server::with_tables(cfg, w.clone(), tables, 0);
    assert!(server.handle_line(0, &format!("room {room}")));
    let mut peers: Vec<Peer2> = Vec::new();
    for (i, n) in names.iter().enumerate() {
        assert!(server.handle_line(0, &format!("user {} {} 99 0 120 {n}", 7 + i, 3 + i)));
        let mut c = SynthConfig::named(n);
        c.box_bounds = Some((w.extent_min, w.extent_max));
        c.walk_step = 0.0;
        c.mode_class_hint = Some(5);
        peers.push(Peer2 {
            addr: 1 + i as u32,
            client: SynthClient::new(c),
            up: SimChannel::new(3 + 2 * i as u64, Faults::clean(2)),
            down: SimChannel::new(4 + 2 * i as u64, Faults::clean(2)),
        });
    }
    let mut now = 0u32;
    while now < 30_000 && peers.iter().any(|p| p.client.life_state() != 1) {
        step_all(&mut server, &mut peers, now);
        now += 10;
    }
    assert!(peers.iter().all(|p| p.client.life_state() == 1), "alive");
    (server, peers, now)
}

fn pid_named(server: &Server<u32>, name: &str) -> u8 {
    server
        .game()
        .players()
        .values()
        .find(|p| p.name8.starts_with(name.as_bytes()))
        .map(|p| p.pid)
        .expect("placed")
}

/// DSV-593 / DSV-594 on the ROM's round world, Hard (it crosses the first chapter): two humans; in round 1 Dave is
/// set to 33 health and 9 + 20 in his gun: the round end's `healinpart` gives each of the two living half the row's
/// round total, his reserve grows to a new kit's count beside the 9 in the clip; in round 2 Erin dies (her only
/// life): the round end brings her back before round 3 starts; in round 10 Dave is set to 40 health: round 11 (a new
/// chapter) starts with his health at the maximum.
// OBSERVED: ROM CabinFever (its round loop, the GMS rows' `RecoverRoundTotalHP` and health-rate rows, read at run
// time)
// DC-DEFAULT: DSV-593, DSV-594 (proposed)
#[test]
fn rom_cabin_fever_recovers_between_rounds_and_chapters() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some((key, w)) = round_world(&rom) else {
        println!("skipped: no co-op round world");
        return;
    };
    let goal = 1u32;
    let vfs = RezVfs::new(&rom);
    let doc = dc_dserver::coop_rom::find_gms(&vfs, &key, &w.announce).expect("the GMS");
    let row = dc_dserver::coop_rom::CoopPlan::from_gms(&doc, goal).difficulty;
    let total = row.recover_hp.expect("the row's round total");
    let trig = trig_field(&rom, &key, &w.announce, goal);
    let (mut server, mut peers, mut now) = two_up(
        &rom,
        &w,
        &format!("key={key} type=5 goal={goal} ff=0 time=36000{trig}"),
        &["Dave", "Erin"],
    );
    let dave = pid_named(&server, "Dave");
    let erin = pid_named(&server, "Erin");
    let gun = |s: &Server<u32>| {
        s.game().players()[&dave]
            .weapons
            .iter()
            .find(|w| w.clip_size > 0 && !w.own_ammo)
            .map(|w| (w.clip, w.stock, w.clip_size))
            .expect("a gun")
    };
    // the new kit: a full clip and the stock cap
    let (_, cap, clip_size) = gun(&server);
    assert!(clip_size > 9, "a clip above 9");
    let max = server.game().players()[&dave].health;
    let mut step = 0;
    let mut erin_back_by_round = None;
    let cap_ms = 3600 * 1000;
    while now < cap_ms {
        step_all(&mut server, &mut peers, now);
        now += 10;
        if !now.is_multiple_of(250) {
            continue;
        }
        let c = server.game().coop().expect("a co-op room").clone();
        assert!(c.stage.is_none(), "the mission went on: {:?}", c.stage);
        match step {
            0 if c.round == 1 => {
                server.harness_vitals(dave, 33.0, Some((9, 20)));
                step = 1;
            }
            1 if c.recoveries == 1 => {
                // DSV-593: two living share the total; the reserve to the kit's count, the clip kept
                let share = (total / 2.0).floor();
                let p = &server.game().players()[&dave];
                assert_eq!(p.health, (33.0 + share).min(max), "the share");
                assert_eq!(gun(&server).0, 9, "the clip untouched");
                assert_eq!(gun(&server).1, cap + clip_size - 9, "a new kit's count");
                assert_eq!(c.heals, 1);
                step = 2;
            }
            2 if c.round == 2 => {
                server.harness_kill_human(now, erin);
                assert_eq!(server.game().coop_lives_of(erin), 0, "her only life");
                step = 3;
            }
            3 if c.round >= 3 => {
                erin_back_by_round = Some(c.round);
                assert_eq!(
                    server.game().players()[&erin].life,
                    dc_dserver::game::Life::Alive,
                    "Erin back at the round start"
                );
                step = 4;
            }
            4 if c.round == 10 => {
                server.harness_vitals(dave, 40.0, None);
                step = 5;
            }
            5 if c.round == 11 => {
                assert_eq!(c.chapter_recoveries, 1, "the chapter's recovery");
                assert_eq!(server.game().players()[&dave].health, max);
                step = 6;
            }
            _ => {}
        }
        if step == 6 {
            break;
        }
        let alive: Vec<usize> = c
            .ais
            .iter()
            .filter(|a| a.state == AiState::Alive)
            .map(|a| a.slot)
            .collect();
        for s in alive {
            server.harness_kill_ai(now, s, dave);
        }
    }
    assert_eq!(step, 6, "reached round 11 in {} s", now / 1000);
    assert_eq!(erin_back_by_round, Some(3));
    // Erin's client took the spawn set (alive again), and every message read
    assert_eq!(peers[1].client.life_state(), 1);
    for p in &peers {
        assert!(
            p.client.seen.errors.is_empty(),
            "{:?}",
            p.client.seen.errors
        );
    }
    let c = server.game().coop().unwrap();
    println!(
        "recoveries {} (chapter {}), heals {} by round {} in {} s",
        c.recoveries,
        c.chapter_recoveries,
        c.heals,
        c.round,
        now / 1000
    );
}

/// DSV-595 / FT-15 on the round world: a room line without `time=` takes the difficulty row's `TimeLimit` as the
/// mission timer; a room time of one minute running out ends the mission as timed out.
#[test]
fn rom_cabin_fever_mission_timer_ends_the_match() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some((key, w)) = round_world(&rom) else {
        println!("skipped: no co-op round world");
        return;
    };
    let vfs = RezVfs::new(&rom);
    let doc = dc_dserver::coop_rom::find_gms(&vfs, &key, &w.announce).expect("the GMS");
    let limit = dc_dserver::coop_rom::CoopPlan::from_gms(&doc, 0)
        .difficulty
        .time_limit_s
        .expect("the row's time limit");
    let trig = trig_field(&rom, &key, &w.announce, 0);
    let (server, _, _) = two_up(
        &rom,
        &w,
        &format!("key={key} type=5 goal=0 ff=0{trig}"),
        &["Dave"],
    );
    assert_eq!(server.game().rules().round_secs, limit);
    let (mut server, mut peers, mut now) = two_up(
        &rom,
        &w,
        &format!("key={key} type=5 goal=0 ff=0 time=60{trig}"),
        &["Dave"],
    );
    assert_eq!(server.game().rules().round_secs, 60);
    while now < 120_000 && server.game().coop().unwrap().stage.is_none() {
        step_all(&mut server, &mut peers, now);
        now += 10;
    }
    assert_eq!(server.game().coop().unwrap().stage, Some(STAGE_TIMEOUT));
    assert!(now >= 60_000, "not before the time ran out ({now} ms)");
}
