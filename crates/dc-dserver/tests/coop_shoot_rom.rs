//! ROM-gated (skipped without CA_ROM): a human shoots the co-op AI of both co-op worlds (the Desert Thunder room's
//! mission world and the Cabin Fever room's round world) at point-blank range with the weapon the server gave it,
//! through the shot message the client sends (not the harness kill). Checked: every hit lowers the AI's health, the
//! AI dies after enough hits, the kill counts for the shooter and in the mission's kill tally, the dead AI is not
//! back on its feet with full health before the corpse time is over, and the client is shown the death at once (the
//! AI's character hidden, a corpse where it fell; DSV-585). No ROM name is written here: the worlds are
//! found by their role (the co-op mode's GMS, a world whose statements spawn AI, a world with a round loop).
// OBSERVED: ROM SANDHOG_CP1, CabinFever (their AI templates and spawners, read at run time); the owner's report of
// 2026-11-04 ("the bots had unlimited health")

use dc_bute::GameRoomData;
use dc_dserver::game::coop::AiState;
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{load_from_rom, NameForm, WorldInfo};
use dc_net_engine::sim::{Faults, SimChannel};
use dc_rez::RezVfs;

/// The co-op mode key and the co-op worlds of the room table: (mission world, round world).
fn coop_worlds(rom: &std::path::Path) -> Option<(String, Option<WorldInfo>, Option<WorldInfo>)> {
    let vfs = RezVfs::new(rom);
    let gr = GameRoomData::from_vfs(&vfs).ok()?;
    let modes = gr.modes(false);
    let (mi, key) = modes.iter().enumerate().find_map(|(i, m)| {
        let t = m.get("GameType")?.trim().trim_matches('"').to_string();
        let k = m.get("Key")?.trim().trim_matches('"').to_string();
        (t == "5").then_some((i, k))
    })?;
    let (mut mission, mut rounds) = (None, None);
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
            rounds.get_or_insert(info);
        } else if info.coop.mission {
            mission.get_or_insert(info);
        }
    }
    Some((key, mission, rounds))
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

/// The points a first kill (no chain) may pay on this map, from the ROM's score table and the difficulty's factor:
/// the kill (or the head kill) plus the zone points of the lethal hit's zone.
// OBSERVED: ROM table keys GMS [CooperativeModeScoreTable], [Object<goal>] ScoreFactor (read at run time)
// DC-DEFAULT: DSV-605 USER-PROVIDED OBSERVATION 2026-11-05 (the lethal hit pays with the kill)
fn first_kill_values(rom: &std::path::Path, key: &str, world: &str, goal: u32) -> Vec<u32> {
    let vfs = RezVfs::new(rom);
    let Some(doc) = dc_dserver::coop_rom::find_gms(&vfs, key, world) else {
        return Vec::new();
    };
    let plan = dc_dserver::coop_rom::CoopPlan::from_gms(&doc, goal);
    let (s, f) = (plan.score, plan.difficulty.score_factor.unwrap_or(1.0));
    let r = |v: f32| (v * f).round().max(0.0) as u32;
    let hit = |i: usize| r(s.hit[i].unwrap_or([10.0, 8.0, 5.0, 3.0][i]));
    let kill = s.kill.unwrap_or(40.0);
    let mut v = vec![r(kill + s.head_kill.unwrap_or(50.0)) + hit(0)];
    v.extend((1..4).map(|i| r(kill) + hit(i)));
    v
}

struct Peer {
    client: SynthClient,
    up: SimChannel,
    down: SimChannel,
}

fn step(server: &mut Server<u32>, p: &mut Peer, now: u32, logs: &mut Vec<String>) {
    for d in p.client.poll(now) {
        p.up.send(now, d);
    }
    for d in p.up.deliver(now) {
        server.handle_datagram(now, 1, &d);
    }
    for o in server.poll(now) {
        match o {
            Output::Transmit { bytes, .. } => p.down.send(now, bytes),
            Output::Log(l) => {
                if std::env::var("SHOOT_LOG").is_ok() {
                    println!("{now}: {l}");
                }
                logs.push(l);
            }
            _ => {}
        }
    }
    for d in p.down.deliver(now) {
        p.client.handle_datagram(now, &d);
    }
}

/// What the shooting reached.
#[derive(Debug, Default)]
struct Shot {
    /// The health of the AI after each hit (from the server's view of the slot).
    healths: Vec<f32>,
    died: bool,
    /// The shooter's kill count in the co-op tally afterwards.
    kills: u32,
    /// The AI's state 1 s after its death (the corpse time is longer).
    after: Option<AiState>,
    /// What the client was shown 0.1 s after the killing shot and 1 s later: the flag words of the AI's character
    /// (0x2802 = hidden) and the corpse objects (shell blob type 0x26) within 64 units of where it died.
    flags_at_death: Option<u16>,
    corpses_at_death: usize,
    flags_later: Option<u16>,
    corpses_later: usize,
    /// The 0xc8 banners the shooter received: `(kills, head, combo)` (DSV-602).
    banners: Vec<(u16, bool, u32)>,
    /// The points of each kill the shooter was told (0xd7 sub 1) and the values the map's score table allows for
    /// a first kill: kill (+ head kill) + the lethal hit's zone points, x the difficulty's factor (DSV-605).
    kill_points: Vec<u32>,
    kill_allowed: Vec<u32>,
    refusals: Vec<String>,
}

/// One human joins the room, stands (for the round world, at the start, so round 1 begins), waits for an AI that
/// stands, steps next to it and shoots it with its own weapon until it dies (at most 60 shots).
fn shoot_one(rom: &std::path::Path, key: &str, w: &WorldInfo, goal: u32) -> Shot {
    let (tables, _) = RomTables::load(rom);
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    cfg.game.coop.hit_chance = [0.0; 3];
    cfg.game.fall.enabled = false;
    // the human is moved next to the AI: the level's kill volumes stay off
    cfg.kill_volumes = false;
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
    let mut logs = Vec::new();
    let mut now = 0u32;
    while now < 30_000 && p.client.life_state() != 1 {
        step(&mut server, &mut p, now, &mut logs);
        now += 10;
    }
    assert_eq!(p.client.life_state(), 1, "alive");
    let pid = server.game().players().values().next().unwrap().pid;
    let start = server.game().players()[&pid].pos;
    // wait (at the start) for a standing AI the client has a stream position for
    let target = loop {
        assert!(now < 240_000, "no AI stood within 4 min");
        p.client.set_height(start[1]);
        p.client.walk_to(Some(start), 100_000.0);
        step(&mut server, &mut p, now, &mut logs);
        now += 10;
        let found = server
            .game()
            .ai_chars()
            .iter()
            .filter(|a| a.state == AiState::Alive)
            .filter_map(|a| {
                let o = a.obj?;
                let d =
                    (a.pos[0] - start[0]).hypot(a.pos[2] - start[2]) + (a.pos[1] - start[1]).abs();
                p.client
                    .seen
                    .remote
                    .contains_key(&o)
                    .then_some((d, a.slot, o))
            })
            .min_by(|x, y| x.0.total_cmp(&y.0))
            .map(|x| (x.1, x.2));
        if let Some(t) = found {
            break t;
        }
    };
    let (slot, obj) = target;
    let ai = |s: &Server<u32>| s.game().ai_chars()[slot].clone();
    println!(
        "AI slot {slot} obj {obj} at {:?}: health {} of {}, dims {:?}, weapon of the human {:?}",
        ai(&server).pos,
        ai(&server).health,
        ai(&server).max_health,
        server.game().ai_dims(slot),
        server.game().players()[&pid]
            .weapons
            .first()
            .map(|w| w.weapon)
    );
    let mut out = Shot::default();
    let before = logs.len();
    for _ in 0..60 {
        // stand 120 units beside the AI (x), at its height, and let the stream catch up
        let a = ai(&server);
        if a.state != AiState::Alive {
            break;
        }
        let at = [a.pos[0] + 120.0, a.pos[1], a.pos[2]];
        for _ in 0..12 {
            p.client.set_height(at[1]);
            p.client.walk_to(Some(at), 100_000.0);
            step(&mut server, &mut p, now, &mut logs);
            now += 10;
        }
        let h0 = ai(&server).health;
        assert!(p.client.fire_at(obj, [0.0; 3], true), "the AI is streamed");
        for _ in 0..3 {
            step(&mut server, &mut p, now, &mut logs);
            now += 10;
        }
        let a = ai(&server);
        if a.health < h0 || a.state != AiState::Alive {
            out.healths.push(a.health);
        }
        if !matches!(a.state, AiState::Alive) {
            out.died = true;
            break;
        }
    }
    out.refusals = logs[before..]
        .iter()
        .filter(|l| l.contains("refused") || l.contains("no damage"))
        .cloned()
        .collect();
    out.kills = server.game().coop().map_or(0, |c| c.ai_killed);
    if out.died {
        let died_at = ai(&server).pos;
        let corpses = |c: &SynthClient| {
            c.seen
                .objects
                .values()
                .filter(|o| o.blob_type == Some(0x26))
                .filter(|o| {
                    let q = o.pos.unwrap_or(o.create_pos);
                    (0..3).all(|i| (q[i] - died_at[i]).abs() < 64.0)
                })
                .count()
        };
        // the records of the kill's tick reach the client
        for _ in 0..10 {
            step(&mut server, &mut p, now, &mut logs);
            now += 10;
        }
        out.banners = p.client.seen.multi_kill_banners.clone();
        out.kill_points = p.client.seen.coop_points.iter().map(|c| c.3).collect();
        out.kill_allowed = first_kill_values(rom, key, &w.announce, goal);
        out.flags_at_death = p.client.seen.objects.get(&obj).and_then(|o| o.flags);
        out.corpses_at_death = corpses(&p.client);
        let until = now + 1_000;
        while now < until {
            step(&mut server, &mut p, now, &mut logs);
            now += 10;
        }
        out.after = Some(ai(&server).state);
        out.flags_later = p.client.seen.objects.get(&obj).and_then(|o| o.flags);
        out.corpses_later = corpses(&p.client);
    }
    println!("{out:?}");
    out
}

fn check(name: &str, s: &Shot) {
    assert!(
        !s.healths.is_empty(),
        "{name}: no shot lowered the AI's health: {:?}",
        s.refusals
    );
    assert!(
        s.healths.windows(2).all(|w| w[1] < w[0]),
        "{name}: every hit lowers the health: {:?}",
        s.healths
    );
    assert!(s.died, "{name}: the AI never died: {s:?}");
    assert!(s.kills >= 1, "{name}: the kill was not counted");
    // DSV-605: the first kill pays the kill (or head kill) and the lethal hit's zone points, no chain bonus; on the
    // ROM's Cabin Fever row (factor 0.6) a body kill is 24 + 9
    assert!(
        s.kill_points
            .first()
            .is_some_and(|x| s.kill_allowed.contains(x)),
        "{name}: the first kill's points {:?} are not one of {:?}",
        s.kill_points,
        s.kill_allowed
    );
    // DSV-602: the kill sent its banner: the first kill of the match, first of a chain (the head flag is the shot's)
    assert!(
        matches!(s.banners.as_slice(), [(1, _, 1)]),
        "{name}: one 0xc8 (kills 1, combo 1): {:?}",
        s.banners
    );
    assert!(
        matches!(s.after, Some(AiState::Dead { .. })),
        "{name}: the AI is up again 1 s after its death: {:?}",
        s.after
    );
    // DSV-585: the client is shown the death at once: the AI's character hidden (not left standing and hittable)
    // and a corpse where it fell, both still so 1 s later (no stream entry puts the body back)
    assert_eq!(
        s.flags_at_death,
        Some(0x2802),
        "{name}: the dead AI's character is hidden"
    );
    assert_eq!(s.corpses_at_death, 1, "{name}: one corpse where it fell");
    assert_eq!(
        s.flags_later,
        Some(0x2802),
        "{name}: still hidden 1 s later"
    );
    assert_eq!(s.corpses_later, 1, "{name}: the corpse lies 1 s later");
}

#[test]
fn rom_coop_ai_die_to_a_humans_shots_in_both_coop_worlds() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some((key, mission, rounds)) = coop_worlds(&rom) else {
        println!("skipped: no co-op mode in the room table");
        return;
    };
    let mission = mission.expect("a co-op mission world");
    let rounds = rounds.expect("a co-op round world");
    let s = shoot_one(&rom, &key, &mission, 0);
    check("mission world", &s);
    let s = shoot_one(&rom, &key, &rounds, 0);
    check("round world", &s);
}
