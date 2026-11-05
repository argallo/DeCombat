//! FT-S11 on a synthetic world (no ROM): a co-op AI that spawns inside a cup of walls open away from the human plans
//! its way out over the navigation grid and around the walls, and reaches the human; it never stands inside the
//! walls. With `coop.nav = off` (FT-S5's straight steering) the same AI stays in the cup.

use dc_dserver::chars::{BodyRow, CharTables, FaceRow};
use dc_dserver::game::coop::{AiState, CoopTuning};
use dc_dserver::game::GameTuning;
use dc_dserver::rom_tables::{RomTables, TeamNames};
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{
    synthetic_world_bytes, world_from_bytes, AiTemplate, CoopSpawner, CoopWorld, SpawnPoint,
};
use dc_net_engine::sim::{Faults, SimChannel};
use dc_sim::{Collider, Cylinder, V3};

const HUMAN: [f32; 3] = [0.0, 0.0, 1500.0];
const SPAWNER: [f32; 3] = [0.0, 0.0, 0.0];

/// Synthetic character tables in the ROM key shape (made-up names), as `game_sim.rs` has them.
fn tables() -> RomTables {
    let mut t = RomTables {
        teams: TeamNames {
            names: [Some(b"red".to_vec()), Some(b"BLUE".to_vec())],
        },
        ..RomTables::default()
    };
    let doc = dc_bute::ButeDoc::parse(concat!(
        "[Weapon3]\nName = \"G3\"\nHHModel = \"HH\\\\g3.ltb\"\nHHSkin0 = \"HH\\\\g3.dtx\"\n",
        "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"G3\"\n",
    ));
    t.weapons = dc_dserver::rom_tables::WeaponTable::from_doc(&doc);
    let mut chars = CharTables::default();
    chars.bodies.insert(
        0,
        BodyRow {
            model: "BODY\\b.ltb".into(),
            styles: vec!["RS\\a.ltb".into()],
            skins: [(0, (Some("BODY\\u.dtx".into()), Some("BODY\\l.dtx".into())))].into(),
            ..BodyRow::default()
        },
    );
    chars.faces.insert(
        0,
        FaceRow {
            model: "FACE\\f.ltb".into(),
            skins: vec!["FACE\\f.dtx".into()],
            styles: vec!["RS\\a.ltb".into()],
        },
    );
    t.chars = chars;
    t
}

fn rig() -> dc_dserver::chars::Rig {
    use dc_dserver::chars::{
        ClipFact, Rig, CLIP_LOWER_STAND, CLIP_LOWER_WALK, CLIP_UPPER_STAND, CLIP_UPPER_WALK,
    };
    let c = |key, len_ms| ClipFact {
        key,
        len_ms,
        extent: [15.0, 50.0, 15.0],
        footsteps: 0,
        upper: None,
    };
    Rig {
        children: vec!["BODY\\kid.ltb".into()],
        clips: vec![
            c(([1; 8], 4), 7200),
            c(CLIP_LOWER_STAND, 1000),
            c(CLIP_LOWER_WALK, 800),
            c(CLIP_UPPER_STAND, 1500),
            c(CLIP_UPPER_WALK, 900),
        ],
        own_clips: 1,
        grip_socket: 2,
        sockets: Vec::new(),
        stand_extent: Some([15.0, 50.0, 15.0]),
        tracker_masks: [None; 2],
    }
}

struct Peer {
    addr: u32,
    client: SynthClient,
    up: SimChannel,
    down: SimChannel,
}

fn step(server: &mut Server<u32>, peers: &mut [Peer], now: u32) {
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
        if let Output::Transmit { to, bytes } = o {
            if let Some(p) = peers.iter_mut().find(|p| p.addr == to) {
                p.down.send(now, bytes);
            }
        }
    }
    for p in peers.iter_mut() {
        for d in p.down.deliver(now) {
            p.client.handle_datagram(now, &d);
        }
    }
}

/// Runs the cup world for `secs` with the AI's navigation on or off; returns the AI's nearest approach to the human
/// (flat), the time of it, and the samples (every 100 ms) the AI stood inside the walls.
fn run(nav: bool, secs: u32) -> (f32, u32, u32) {
    let cup = dc_nav::testworld::cupped();
    let mut w = world_from_bytes(&synthetic_world_bytes(), "Worlds\\Cup.dat", "Cup").unwrap();
    w.extent_min = [-5000.0, -64.0, -5000.0];
    w.extent_max = [5000.0, 512.0, 5000.0];
    w.trace = Some(cup.clone());
    w.spawns = vec![SpawnPoint {
        pos: HUMAN,
        rot: [0.0; 3],
        team_filter: None,
    }];
    w.coop = CoopWorld {
        spawners: vec![CoopSpawner {
            name: b"s0".to_vec(),
            pos: SPAWNER,
            rot: [0.0; 3],
            target: Some(b"tmpl".to_vec()),
        }],
        templates: vec![AiTemplate {
            name: b"tmpl".to_vec(),
            pos: [0.0; 3],
            rot: [0.0; 3],
            body_row: None,
            right_hand: Some("G3".into()),
            max_health: Some(60.0),
            template: true,
            death_cmd: Vec::new(),
        }],
        nodes: Vec::new(),
        mission: false,
        rounds: None,
        ..Default::default()
    };
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![3],
            move_check: false,
            coop: CoopTuning {
                max_alive: 1,
                waves: 1,
                lives: Some(3),
                first_wave_ms: 500,
                spawn_interval_ms: 100,
                // the human stays far from the spawner (FT-10's 256-unit rule does not skip it)
                spawn_clear: 256.0,
                hit_chance: [0.0; 3],
                nav,
                ..CoopTuning::default()
            },
            ..GameTuning::default()
        },
        ..ServerTuning::default()
    };
    let mut server = Server::with_tables(cfg, w.clone(), tables(), 0);
    server.insert_rig("BODY\\b.ltb", rig());
    assert!(server.handle_line(0, "room key=COOP type=5 goal=1 ff=0 time=0"));
    assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
    let mut c = SynthConfig::named("Dave");
    c.box_bounds = Some((w.extent_min, w.extent_max));
    c.walk_step = 0.0;
    c.mode_class_hint = Some(5);
    let mut peers = vec![Peer {
        addr: 1,
        client: SynthClient::new(c),
        up: SimChannel::new(3, Faults::clean(2)),
        down: SimChannel::new(4, Faults::clean(2)),
    }];
    let body = Cylinder::from_dims(V3::new(14.0, 49.0, 14.0));
    let (mut best, mut best_at, mut inside) = (f32::MAX, 0u32, 0u32);
    let mut now = 0u32;
    while now < secs * 1000 {
        step(&mut server, &mut peers, now);
        now += 5;
        if !now.is_multiple_of(100) {
            continue;
        }
        let Some(a) = server
            .game()
            .ai_chars()
            .iter()
            .find(|a| a.state == AiState::Alive)
        else {
            continue;
        };
        if cup.overlaps(&body, V3::from_array(a.pos)) {
            inside += 1;
        }
        let d = (a.pos[0] - HUMAN[0]).hypot(a.pos[2] - HUMAN[2]);
        if d < best {
            best = d;
            best_at = now;
        }
    }
    (best, best_at, inside)
}

/// FT-S11: the AI in the cup walks out of its open side, around the back wall and up to the human (within the hold
/// range of 400 units, where it stops), never inside a wall; FT-S5's steering alone keeps pushing into the back wall.
#[test]
fn a_coop_ai_finds_its_way_around_the_walls_to_the_human() {
    let (near, at, inside) = run(true, 60);
    println!("nav on: nearest {near:.0} at {at} ms, {inside} samples inside the walls");
    assert!(near <= 420.0, "the AI came within {near:.0} units");
    assert_eq!(inside, 0, "never inside a wall");
    let (near_off, at_off, _) = run(false, 60);
    println!("nav off: nearest {near_off:.0} at {at_off} ms");
    assert!(
        near_off > 1000.0,
        "the straight steering stays in the cup ({near_off:.0})"
    );
}
