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
use dc_dserver::game::coop::{AiState, STAGE_COMPLETE};
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
