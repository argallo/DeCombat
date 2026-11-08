//! ROM-gated (skipped without CA_ROM): the co-op mission world (the Desert Thunder room's world) runs its own trigger
//! network (DSV-570..DSV-575). One human walks the mission: every player-triggerable box of the world in the order of
//! its distance from the start, again while new ones open; the world models the network needs are broken, the
//! activation boxes are held. Checked: the spawners' statements create AI (the count rises stage by stage), the AI's
//! death commands run, the checkpoints give a life each, the goals of the difficulty row are added and completed, and
//! the final activation box ends the mission with success. What the clients are shown (DSV-579..DSV-584): the caption
//! and message triggers' texts as captions and banners, the music triggers' tracks, the ending's camera, no wave
//! banner; a second player who joins at the second checkpoint gets the goal list so far and the music.
//! No ROM name is written here: every object is found by its role in the network.
// OBSERVED: ROM SANDHOG_CP1 (its triggers, counters, timer, activation boxes and AI templates, read at run time)
// DC-DEFAULT: DSV-570..DSV-575 (proposed)

use dc_dserver::game::coop::STAGE_COMPLETE;
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{load_from_rom, NameForm, WORLD_ARCHIVE, WORLD_EXT};
use dc_net_engine::sim::{Faults, SimChannel};

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
        match o {
            Output::Transmit { to, bytes } => {
                if let Some(p) = peers.iter_mut().find(|p| p.addr == to) {
                    p.down.send(now, bytes);
                }
            }
            Output::Log(l) if std::env::var("MISSION_LOG").is_ok() => println!("{now}: {l}"),
            _ => {}
        }
    }
    for p in peers.iter_mut() {
        for d in p.down.deliver(now) {
            p.client.handle_datagram(now, &d);
        }
    }
}

fn world_arg(rom: &std::path::Path, stem: &str) -> Option<String> {
    let arch = dc_rez::RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE)).ok()?;
    let listed = arch.entries().iter().map(|e| e.0.clone()).find(|n| {
        let u = n.to_ascii_uppercase();
        let base = u.rsplit('\\').next().unwrap_or(&u).to_string();
        base == format!("{stem}{WORLD_EXT}")
    })?;
    let base = listed.rsplit('\\').next().unwrap_or(&listed).to_string();
    Some(format!("Worlds\\{}", &base[..base.len() - WORLD_EXT.len()]))
}

/// The co-op room key of the ROM room table (the mode whose game type is 5).
fn coop_key(rom: &std::path::Path) -> Option<String> {
    let vfs = dc_rez::RezVfs::new(rom);
    let gr = dc_bute::GameRoomData::from_vfs(&vfs).ok()?;
    gr.modes(false).iter().find_map(|m| {
        let t = m.get("GameType")?.trim().trim_matches('"').to_string();
        let k = m.get("Key")?.trim().trim_matches('"').to_string();
        (t == "5").then_some(k)
    })
}

/// The room line's `trig=` field as the lobby builds it from the world's GMS (start triggers, end trigger, controller).
// SPEC: DSERVER_PIPE.md §3.2 (room line); OBSERVED: ROM table key GMS [Mission0] StartTrigger / Controller,
// [Object<n>] StartTrigger / EndTrigger
fn trig_field(rom: &std::path::Path, key: &str, world: &str, goal: u32) -> String {
    let vfs = dc_rez::RezVfs::new(rom);
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

fn stand_at(server: &mut Server<u32>, peers: &mut [Peer], now: &mut u32, at: [f32; 3], ms: u32) {
    let until = *now + ms;
    while *now < until {
        peers[0].client.set_height(at[1]);
        peers[0].client.walk_to(Some(at), 100_000.0);
        step(server, peers, *now);
        *now += 5;
    }
}

struct Stage {
    spawns: u32,
    alive: usize,
    lives: u32,
    goals: Vec<(u32, u8)>,
}

#[test]
fn rom_desert_thunder_runs_its_trigger_network_to_the_end() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(arg) = world_arg(&rom, "SANDHOG_CP1") else {
        println!("skipped: no co-op mission world");
        return;
    };
    let Some(key) = coop_key(&rom) else {
        println!("skipped: no co-op mode in the room table");
        return;
    };
    let (tables, _) = RomTables::load(&rom);
    let w = load_from_rom(&rom, &arg, NameForm::Listed, None).expect("mission world");
    assert!(
        w.coop.mission,
        "the world's statements spawn AI: a mission world"
    );
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    // the test walks the mission: the AI do not hit the walker
    cfg.game.coop.hit_chance = [0.0; 3];
    cfg.game.coop.melee_hit = 0.0;
    cfg.game.coop.full_accuracy = false;
    let cap = cfg.game.coop.speed;
    // the walker is moved from box to box: no fall damage
    cfg.game.fall.enabled = false;
    let trig = trig_field(&rom, &key, &w.announce, 0);
    let mut server = Server::with_tables(cfg, w.clone(), tables, 0);
    assert!(server.handle_line(
        0,
        &format!("room key={key} type=5 goal=0 ff=0 time=1260{trig}")
    ));
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
    let mut now = 0u32;
    while now < 30_000 && peers[0].client.life_state() != 1 {
        step(&mut server, &mut peers, now);
        now += 5;
    }
    assert_eq!(peers[0].client.life_state(), 1, "alive");
    let pid = server.game().players().values().next().unwrap().pid;
    let start = server.game().players()[&pid].pos;
    let coop = |s: &Server<u32>| s.game().coop().expect("a co-op room").clone();
    assert!(
        coop(&server).mission && coop(&server).waves_total() == 0,
        "no wave plan"
    );
    let placed = coop(&server).mission_spawns;
    println!("start: {placed} placed AI spawned");

    // the walk: the boxes a player walks into (the logic boxes of the file are 16-unit cubes outside the level)
    let mut order: Vec<usize> = (0..w.triggers.len())
        .filter(|i| w.triggers[*i].touch && w.triggers[*i].dims.iter().any(|d| *d > 16.5))
        .collect();
    let d = |i: &usize| {
        let p = w.triggers[*i].pos;
        (p[0] - start[0]).hypot(p[2] - start[2])
    };
    order.sort_by(|a, b| d(a).total_cmp(&d(b)));
    let mut stages: Vec<Stage> = Vec::new();
    let mut joined: Option<(u32, bool, usize)> = None;
    let mut broke = false;
    let mut kills = 0u32;
    // DSV-588..DSV-590: the soldiers the statements spawn hold their guns and walk at `coop.speed`
    let mut armed = 0u32;
    let mut single_shot = 0u32;
    for pass in 0..16 {
        let mut progress = false;
        for &i in &order {
            let st = server.world_statements();
            let t = &w.triggers[i];
            // each box once (a box without an activation limit fires on every entry)
            if st.locked[i] || st.fires[i] > 0 {
                continue;
            }
            let fires = st.fires[i];
            let lives = server.game().coop_lives_of(pid);
            stand_at(&mut server, &mut peers, &mut now, t.pos, 400);
            // away again (a box fires on the way in)
            stand_at(
                &mut server,
                &mut peers,
                &mut now,
                [t.pos[0], t.pos[1] + 3000.0, t.pos[2]],
                50,
            );
            if server.world_statements().fires[i] > fires {
                progress = true;
            } else if std::env::var("MISSION_LOG").is_ok() {
                let me = &server.game().players()[&pid];
                println!(
                    "box {i} at {:?} dims {:?} did not fire: me {:?} {:?}",
                    t.pos, t.dims, me.pos, me.life
                );
            }
            let l = server.game().coop_lives_of(pid);
            if l > lives {
                let c = coop(&server);
                stages.push(Stage {
                    spawns: c.mission_spawns,
                    alive: c.alive(),
                    lives: l,
                    goals: c.goals.clone(),
                });
                // DSV-584: a player who joins the running mission (at the second checkpoint)
                if stages.len() == 2 && joined.is_none() {
                    assert!(server.handle_line(now, "user 8 4 99 0 120 Erin"));
                    let mut c = SynthConfig::named("Erin");
                    c.box_bounds = Some((w.extent_min, w.extent_max));
                    c.walk_step = 0.0;
                    c.mode_class_hint = Some(5);
                    peers.push(Peer {
                        addr: 2,
                        client: SynthClient::new(c),
                        up: SimChannel::new(5, Faults::clean(2)),
                        down: SimChannel::new(6, Faults::clean(2)),
                    });
                    let until = now + 30_000;
                    while now < until && peers[1].client.life_state() != 1 {
                        step(&mut server, &mut peers, now);
                        now += 5;
                    }
                    assert_eq!(peers[1].client.life_state(), 1, "the joiner is in");
                    let goals = coop(&server).goals.len() as u32;
                    let looping = server
                        .game()
                        .coop()
                        .unwrap()
                        .show
                        .music
                        .is_some_and(|i| w.coop.music[i].looped);
                    let plays = peers[1]
                        .client
                        .seen
                        .music
                        .iter()
                        .filter(|m| m.1.is_some())
                        .count();
                    joined = Some((goals, looping, plays));
                }
            }
            for a in coop(&server)
                .ais
                .iter()
                .filter(|a| a.state == dc_dserver::game::coop::AiState::Alive && !a.spec.unarmed)
            {
                assert!(a.spec.melee.is_none(), "{}: a soldier shoots", a.spec.key);
                assert!(
                    a.spec.look.weapon.is_some(),
                    "{}: holds its gun",
                    a.spec.key
                );
                assert_eq!(a.spec.speed, cap, "{}", a.spec.key);
                // DSV-640 / DSV-641: the soldier's weapon row gives its bursts, its attribute row the full-accuracy
                // distance (the few templates holding a player row fire every `fire_ms`)
                if let Some(b) = a.spec.burst {
                    single_shot += u32::from(b.shots == (1, 1));
                    assert!(b.pause_s.0 >= 0.0 && b.pause_s.1 >= b.pause_s.0);
                }
                assert!(
                    a.spec.full_accuracy.is_some_and(|r| r > 0.0),
                    "{}",
                    a.spec.key
                );
                armed += 1;
            }
            // the AI fall: their death commands run (respawns, counters)
            let alive: Vec<usize> = coop(&server)
                .ais
                .iter()
                .filter(|a| a.state == dc_dserver::game::coop::AiState::Alive)
                .map(|a| a.slot)
                .collect();
            for s in alive {
                server.harness_kill_ai(now, s, pid);
                kills += 1;
            }
            if peers[0].client.life_state() != 1 {
                // a kill box of the level: wait for the respawn
                let until = now + 15_000;
                while now < until && peers[0].client.life_state() != 1 {
                    step(&mut server, &mut peers, now);
                    now += 5;
                }
            }
        }
        if coop(&server).stage.is_some() {
            break;
        }
        if !progress && !broke {
            // the world models whose death drives the network (the armoured vehicle's engines)
            broke = true;
            for b in w
                .destructibles
                .breakables
                .iter()
                .filter(|b| !b.script.is_empty())
            {
                let hit = server.harness_break(now, &b.name);
                println!("pass {pass}: broke a network world model: {hit}");
            }
            stand_at(&mut server, &mut peers, &mut now, start, 2000);
            continue;
        }
        if !progress {
            // the activation boxes that are open: stay in each for its time
            let net = &server.world_statements().net;
            let open: Vec<(usize, [f32; 3], f32)> = net
                .objects
                .activates
                .iter()
                .enumerate()
                .filter(|(k, a)| {
                    !net.activate_locked[*k] && (a.max == 0 || net.activate_fires[*k] < a.max)
                })
                .map(|(k, a)| (k, a.pos, a.secs))
                .collect();
            for (k, at, secs) in open {
                let before = server.world_statements().net.activate_fires[k];
                stand_at(
                    &mut server,
                    &mut peers,
                    &mut now,
                    at,
                    (secs * 1000.0) as u32 + 1500,
                );
                if server.world_statements().net.activate_fires[k] > before {
                    progress = true;
                    println!("pass {pass}: activation box {k} done");
                }
            }
        }
        if !progress {
            break;
        }
    }
    // the end: the final box's commands complete the mission after their delays
    let until = now + 40_000;
    while now < until && coop(&server).stage.is_none() {
        stand_at(&mut server, &mut peers, &mut now, start, 100);
    }
    let c = coop(&server);
    for (k, s) in stages.iter().enumerate() {
        println!(
            "checkpoint {}: {} AI spawned so far, {} alive, {} lives, goals {:?}",
            k + 1,
            s.spawns,
            s.alive,
            s.lives,
            s.goals
        );
    }
    println!(
        "end: stage {:?}, {} AI spawned by statements ({} dropped), {kills} killed, goals {:?}, {} statements skipped, {} s",
        c.stage,
        c.mission_spawns,
        c.mission_dropped,
        c.goals,
        server.world_statements().net.skipped,
        now / 1000
    );
    assert!(armed > 0, "armed soldiers were spawned");
    assert!(
        single_shot > 0,
        "one-shot bursts (the launcher and bolt rifle rows)"
    );
    let net = &server.world_statements().net;
    println!(
        "counters {:?}, activation boxes locked {:?} fired {:?}, vars {}",
        net.counters,
        net.activate_locked,
        net.activate_fires,
        net.vars.len()
    );
    let unfired: Vec<String> = order
        .iter()
        .filter(|i| server.world_statements().fires[**i] == 0)
        .map(|i| {
            format!(
                "{}{}",
                i,
                if server.world_statements().locked[*i] {
                    "L"
                } else {
                    ""
                }
            )
        })
        .collect();
    println!("boxes never fired: {unfired:?}");
    // DSV-571: the spawners' statements created AI, more at every stage
    assert!(
        stages.len() >= 3,
        "three checkpoints with a life each: {}",
        stages.len()
    );
    assert!(
        stages.windows(2).all(|p| p[1].spawns > p[0].spawns),
        "the AI count rises stage by stage"
    );
    assert!(stages[0].spawns > placed);
    // DSV-572: each checkpoint gave a life
    assert!(stages.windows(2).all(|p| p[1].lives > p[0].lives));
    // DSV-574: the goals were added and the first ones completed
    assert!(c.goals.len() >= 4, "{:?}", c.goals);
    assert!(
        c.goals
            .iter()
            .filter(|g| g.1 == dc_dserver::game::coop::GOAL_DONE)
            .count()
            >= 3
    );
    // DSV-571 / DSV-573: the final activation box ended the mission with success
    assert_eq!(c.stage, Some(STAGE_COMPLETE), "mission complete");
    // DSV-579..DSV-581: the walker was shown the captions (three fields each, checked by the reader), the message
    // trigger's banners (no wave banner, no goal-text banner: the world has its own), the music, the ending's camera
    let seen = &peers[0].client.seen;
    assert!(seen.errors.is_empty(), "{:?}", seen.errors);
    let show = &c.show;
    println!(
        "shown: {} captions, {} banners, {} music changes, {} camera fields of view, {} unresolved, characters hidden {} times",
        seen.captions.len(),
        seen.banners.len(),
        seen.music.len(),
        seen.camera_fov.len(),
        show.unresolved,
        show.hide_changes
    );
    assert!(seen.captions.len() >= 10, "{}", seen.captions.len());
    assert_eq!(seen.captions.len() as u32, show.captions);
    assert!(seen.banners.len() >= 4 && seen.banners.len() as u32 == show.banners);
    assert!(!seen.banners.iter().any(|b| b.starts_with(b"Wave")));
    assert!(seen.music.iter().filter(|m| m.1.is_some()).count() >= 3);
    // the ending's shot: the world camera live on the walker's client (the stage-2 cut scene with its fields of view
    // is behind the hostage door, which this walk does not open)
    let live: Vec<u16> = server
        .world_cameras()
        .iter()
        .filter(|c| c.live)
        .map(|c| c.obj)
        .collect();
    assert_eq!(live.len(), 1, "the ending's camera");
    assert_eq!(seen.objects[&live[0]].uflags, Some(0x20));
    assert_eq!(show.unresolved, 0);
    // DSV-584: the joiner got the goal list as it stood and the looping track that played
    let (goals, looping, plays) = joined.expect("a second player joined");
    let j = &peers[1].client.seen;
    assert!(j.errors.is_empty(), "{:?}", j.errors);
    assert!(
        j.missions.iter().any(|m| m.0 == goals && goals > 0),
        "the joiner's goal lists {:?}, {goals} goals",
        j.missions
    );
    assert!(!looping || plays >= 1, "the looping track to the joiner");
}

/// DSV-642: the AI the players leave behind do not starve the mission's later spawns. One human walks the boxes once
/// in the order of their distance from the start, stands 2 s in each and kills only the AI within 1500 units of the
/// box (the placed AI the file keeps far below the level and the AI of the areas he walks past live on). After the
/// third checkpoint (the way into the last area) the statements' spawns still appear while he walks 30 boxes on
/// (without the rule: none, 2026-11-08).
#[test]
fn rom_desert_thunder_spawns_go_on_past_the_ai_left_behind() {
    // long enough a stay for the AI to come to him and for those he passed to age (DSV-642's `retire_age_ms`)
    let stay = 2_000;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (Some(arg), Some(key)) = (world_arg(&rom, "SANDHOG_CP1"), coop_key(&rom)) else {
        println!("skipped: no co-op mission world");
        return;
    };
    let (tables, _) = RomTables::load(&rom);
    let w = load_from_rom(&rom, &arg, NameForm::Listed, None).expect("mission world");
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    cfg.game.coop.hit_chance = [0.0; 3];
    cfg.game.coop.melee_hit = 0.0;
    cfg.game.coop.full_accuracy = false;
    cfg.game.fall.enabled = false;
    let trig = trig_field(&rom, &key, &w.announce, 0);
    let mut server = Server::with_tables(cfg, w.clone(), tables, 0);
    assert!(server.handle_line(
        0,
        &format!("room key={key} type=5 goal=0 ff=0 time=1260{trig}")
    ));
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
    let mut now = 0u32;
    while now < 30_000 && peers[0].client.life_state() != 1 {
        step(&mut server, &mut peers, now);
        now += 5;
    }
    let pid = server.game().players().values().next().unwrap().pid;
    let start = server.game().players()[&pid].pos;
    let mut order: Vec<usize> = (0..w.triggers.len())
        .filter(|i| w.triggers[*i].touch && w.triggers[*i].dims.iter().any(|d| *d > 16.5))
        .collect();
    let d = |i: &usize| {
        let p = w.triggers[*i].pos;
        (p[0] - start[0]).hypot(p[2] - start[2])
    };
    order.sort_by(|a, b| d(a).total_cmp(&d(b)));
    let spawns = |s: &Server<u32>| s.game().coop().unwrap().mission_spawns;
    let (mut checkpoints, mut after, mut boxes_after) = (0u32, None, 0u32);
    for &i in &order {
        let st = server.world_statements();
        if st.locked[i] || st.fires[i] > 0 {
            continue;
        }
        let t = &w.triggers[i];
        let (fires, lives) = (st.fires[i], server.game().coop_lives_of(pid));
        stand_at(&mut server, &mut peers, &mut now, t.pos, stay);
        stand_at(
            &mut server,
            &mut peers,
            &mut now,
            [t.pos[0], t.pos[1] + 3000.0, t.pos[2]],
            50,
        );
        if server.world_statements().fires[i] > fires && after.is_some() {
            boxes_after += 1;
        }
        if server.game().coop_lives_of(pid) > lives {
            checkpoints += 1;
            if checkpoints == 3 {
                after = Some(spawns(&server));
            }
        }
        let near: Vec<usize> = server
            .game()
            .coop()
            .unwrap()
            .ais
            .iter()
            .filter(|a| a.state == dc_dserver::game::coop::AiState::Alive)
            .filter(|a| {
                (0..3).map(|k| (a.pos[k] - t.pos[k]).powi(2)).sum::<f32>() < 1500.0f32.powi(2)
            })
            .map(|a| a.slot)
            .collect();
        for s in near {
            server.harness_kill_ai(now, s, pid);
        }
        if peers[0].client.life_state() != 1 {
            let until = now + 15_000;
            while now < until && peers[0].client.life_state() != 1 {
                step(&mut server, &mut peers, now);
                now += 5;
            }
        }
        if boxes_after >= 30 {
            break;
        }
    }
    let at3 = after.expect("the third checkpoint was reached");
    let later = spawns(&server);
    println!("spawns at the third checkpoint {at3}, {boxes_after} boxes later {later}");
    assert!(
        boxes_after >= 30,
        "{boxes_after} boxes after the checkpoint"
    );
    assert!(
        later >= at3 + 8,
        "the last area's spawns appear: {at3} -> {later}"
    );
}
