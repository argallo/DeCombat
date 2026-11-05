//! ROM-gated (skipped without CA_ROM): the co-op round world's AI (the Cabin Fever room's infected) as the world and
//! the AI table describe them (DSV-588..DSV-590). One human holds the start; the AI of the first two rounds come at
//! it. Checked for every AI kind the rounds send: no weapon object is attached (its hand weapon does hands damage:
//! a melee AI), the body is the row its template names when that row shares the player rig, else a row another
//! template of the world names; it walks at its attribute row's run speed (below `coop.speed`), its steps never
//! exceed that speed, it strikes only within its brain's melee reach, and its upper body is not posed on the
//! player's (rifle) stand / walk clips. The mission world's (Desert Thunder's) AI keep a weapon at socket 2 and
//! `coop.speed` (`coop_rom.rs` checks the weapon of the first AI of both worlds). Each kind spawns with its attribute
//! row's `HitPoints` x the round's rate (DSV-603), the most durable one outlasting two magazines of the default rifle,
//! and its body's gear row protects the kinds' death blasts in full but not the rifle (DSV-604). No ROM value is
//! written here.
// OBSERVED: ROM CabinFever (its AI templates, spawn tables), AIBUTES.TXT, MODELBUTES.TXT, WEAPONS.TXT (read at run
// time)
// DC-DEFAULT: DSV-588..DSV-590 (proposed), DSV-603, DSV-604 (proposed)

use std::collections::BTreeMap;

use dc_bute::GameRoomData;
use dc_dserver::coop_rom::AiTables;
use dc_dserver::game::coop::AiState;
use dc_dserver::game::Motion;
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{load_from_rom, NameForm, WorldInfo};
use dc_net_engine::sim::{Faults, SimChannel};
use dc_rez::RezVfs;

/// The co-op mode key and the worlds its GMS rows name: (round world, mission world).
fn coop_worlds(rom: &std::path::Path) -> (String, Option<WorldInfo>, Option<WorldInfo>) {
    let vfs = RezVfs::new(rom);
    let gr = GameRoomData::from_vfs(&vfs).expect("room table");
    let modes = gr.modes(false);
    let (mi, key) = modes
        .iter()
        .enumerate()
        .find_map(|(i, m)| {
            let t = m.get("GameType")?.trim().trim_matches('"').to_string();
            let k = m.get("Key")?.trim().trim_matches('"').to_string();
            (t == "5").then_some((i, k))
        })
        .expect("a co-op mode row");
    let (mut rounds, mut mission) = (None, None);
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
            rounds = Some(info);
        } else if info.coop.mission {
            mission = Some(info);
        }
    }
    (key, rounds, mission)
}

/// The room line's `trig=` field as the lobby builds it from the world's GMS (the difficulty's start and end).
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
        if let Output::Transmit { bytes, .. } = o {
            p.down.send(now, bytes);
        }
    }
    for d in p.down.deliver(now) {
        p.client.handle_datagram(now, &d);
    }
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt()
}

/// What one AI kind (a template) showed.
#[derive(Debug, Default)]
struct Kind {
    seen: u32,
    speed: f32,
    reach: Option<f32>,
    /// The fastest flat move over a one-second window of walking, as a speed (units per second), and the
    /// windows faster than the kind's speed.
    top_speed: f32,
    fast_windows: u32,
    windows: u32,
    /// Flat distance walked and the time spent walking (ms).
    walked: f32,
    walk_ms: u32,
    strikes: u32,
    /// The longest distance to the human at a strike.
    strike_dist: f32,
    attached: usize,
    rifle_upper: u32,
    upper_samples: u32,
    /// Per spawn: its health maximum and the round's health rate (DSV-603).
    health: Vec<(f32, f32)>,
}

#[test]
fn rom_cabin_fever_infected_walk_slowly_unarmed_and_strike_in_melee() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (key, Some(w), mission) = coop_worlds(&rom) else {
        println!("skipped: no co-op round world");
        return;
    };
    let vfs = RezVfs::new(&rom);
    let ai = AiTables::from_vfs(&vfs);
    assert!(
        !ai.attributes.is_empty() && !ai.brains.is_empty(),
        "the AI table's rows"
    );
    let (tables, _) = RomTables::load(&rom);
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    cfg.game.fall.enabled = false;
    // the human stays up: the strikes are counted, not felt
    cfg.game.coop.hit_chance = [0.0; 3];
    cfg.game.coop.melee_hit = 0.0;
    let cap = cfg.game.coop.speed;
    let trig = trig_field(&rom, &key, &w.announce, 1);
    let mut server = Server::with_tables(cfg, w.clone(), tables, 0);
    assert!(server.handle_line(
        0,
        &format!("room key={key} type=5 goal=1 ff=0 time=36000{trig}")
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
    let mut kinds: BTreeMap<String, Kind> = BTreeMap::new();
    let mut last: BTreeMap<usize, ([f32; 3], u32, Motion)> = BTreeMap::new();
    // per AI: the window's start (time, position) and whether it walked the whole window
    let mut window: BTreeMap<usize, (u32, [f32; 3], bool)> = BTreeMap::new();
    // every AI is killed 12 s after its spawn (time to walk, rush and strike, DSV-597), at once when its kind is
    // known: the rounds move on to the round that first sends the last kind (Hard: round 11)
    let mut born: BTreeMap<usize, u32> = BTreeMap::new();
    let end = now + 1_500_000;
    // the last kind is watched for 3 s more
    let mut all_at = None;
    while now < end && all_at.is_none_or(|t| now < t + 3_000) {
        if kinds.len() == 4 && all_at.is_none() {
            all_at = Some(now);
        }
        p.client.set_height(start[1]);
        p.client.walk_to(Some(start), 100_000.0);
        step(&mut server, &mut p, now);
        if server.game().coop().is_some_and(|c| c.stage.is_some()) {
            break;
        }
        let Some(human) = server.game().players().get(&pid).map(|h| h.pos) else {
            panic!(
                "the human left at {now} ms (round {:?})",
                server.game().coop().map(|c| c.round)
            );
        };
        let ais: Vec<_> = server.game().ai_chars().to_vec();
        for a in &ais {
            if a.state != AiState::Alive {
                last.remove(&a.slot);
                window.remove(&a.slot);
                born.remove(&a.slot);
                continue;
            }
            let k = kinds.entry(a.spec.key.clone()).or_default();
            let b = *born.entry(a.slot).or_insert(now);
            if b == now {
                let rate = server.game().coop().map_or(1.0, |c| {
                    let r = &c.plan.health_rates;
                    r.get(c.wave).or(r.last()).copied().unwrap_or(1.0)
                });
                k.health.push((a.max_health, rate));
                k.seen += 1;
                k.speed = a.spec.speed;
                k.reach = a.spec.melee;
                if let Some(o) = a.obj.and_then(|o| p.client.seen.objects.get(&o)) {
                    k.attached = k.attached.max(o.attached.len());
                }
            }
            if let Some((pos, shots, motion)) = last.get(&a.slot).copied() {
                let d = flat(pos, a.pos);
                if motion == Motion::Walk || a.motion == Motion::Walk {
                    k.walked += d;
                    k.walk_ms += 10;
                }
                if a.shots > shots {
                    k.strikes += 1;
                    k.strike_dist = k.strike_dist.max(dist(a.pos, human));
                }
            }
            last.insert(a.slot, (a.pos, a.shots, a.motion));
            let wd = window.entry(a.slot).or_insert((now, a.pos, true));
            wd.2 &= a.motion == Motion::Walk;
            if now.saturating_sub(wd.0) >= 1_000 {
                if wd.2 {
                    let v = flat(wd.1, a.pos) / ((now - wd.0) as f32 / 1000.0);
                    k.top_speed = k.top_speed.max(v);
                    k.windows += 1;
                    if v > a.spec.speed * 1.05 + 1.0 {
                        k.fast_windows += 1;
                    }
                }
                *wd = (now, a.pos, true);
            }
            if let Some(r) = a.obj.and_then(|o| p.client.seen.remote.get(&o)) {
                if let Some(u) = r.last_upper {
                    k.upper_samples += 1;
                    let rifle = a.spec.look.anims.upper;
                    if u.0 == rifle[0].0 || u.0 == rifle[1].0 {
                        k.rifle_upper += 1;
                    }
                }
            }
            // a kind seen long enough goes at once: the rounds move on to the kinds of the later rounds
            let done = k.walk_ms >= 60_000 && k.strikes > 0;
            if now.saturating_sub(b) >= if done { 250 } else { 12_000 } {
                server.harness_kill_ai(now, a.slot, pid);
            }
        }
        now += 10;
    }
    println!(
        "{kinds:#?}
round {:?} at {} s",
        server.game().coop().map(|c| c.round),
        now / 1000
    );
    assert_eq!(
        kinds.len(),
        4,
        "the four AI kinds of the first rounds: {:?}",
        kinds.keys()
    );
    let templates = &w.coop.templates;
    let world_rows: Vec<&str> = templates
        .iter()
        .filter_map(|t| t.body_row.as_deref())
        .collect();
    let spec_of = |name: &str| {
        server
            .game()
            .ai_chars()
            .iter()
            .find(|a| a.spec.key == name)
            .map(|a| a.spec.clone())
            .expect("a spec")
    };
    for (name, k) in &kinds {
        let t = templates
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name.as_bytes()))
            .expect("the template");
        let spec = spec_of(name);
        // DSV-589: the hand weapon of the template does hands or melee damage, so the AI fights in melee, holds no
        // weapon object and strikes within its brain's reach
        let brain = ai
            .brain_named(t.brain.as_deref().unwrap())
            .expect("the brain row");
        assert!(brain.melee_only(), "{name}: a melee-only brain");
        assert_eq!(k.reach, brain.melee_max, "{name}");
        assert!(
            spec.weapon.is_some() && spec.look.weapon.is_none(),
            "{name}: {:?}",
            spec.look.weapon
        );
        assert_eq!(k.attached, 0, "{name}: no weapon object on the create");
        if k.strikes > 0 {
            assert!(
                k.strike_dist <= k.reach.unwrap() + 1.0,
                "{name}: struck from {:.0} (reach {:?})",
                k.strike_dist,
                k.reach
            );
        }
        // DSV-588: the attribute row of its body row gives the speed, below `coop.speed`
        let row = ai
            .attributes_named(t.body_row.as_deref().unwrap())
            .expect("the attribute row");
        let want = row.run_speed.unwrap().min(cap);
        assert!(want < cap, "{name}: slower than the soldiers' cap");
        assert_eq!(k.speed, want, "{name}");
        assert_eq!(
            k.fast_windows, 0,
            "{name}: {} of {} windows faster",
            k.fast_windows, k.windows
        );
        assert!(
            k.top_speed <= want * 1.05 + 1.0,
            "{name}: walked {:.0} u/s",
            k.top_speed
        );
        // DSV-603: the health maximum is the attribute row's `HitPoints` x the round's rate, not the placed objects'
        // common `MaxHitPoints`
        let hp = row.hit_points.expect("the row's HitPoints");
        for (max, rate) in &k.health {
            assert!(
                (max - (hp * rate).max(1.0)).abs() < 0.01,
                "{name}: health {max} (row {hp} x {rate})"
            );
        }
        let spawned = k.health.iter().map(|h| h.0);
        println!(
            "{name}: HitPoints {hp}, spawned with {:.0}..{:.0}",
            spawned.clone().fold(f32::MAX, f32::min),
            spawned.fold(0.0, f32::max)
        );
        // DSV-590: its own body row, else one another template of the world names
        let body = &spec.look.assets.body[0];
        let own = ai.body_named(t.body_row.as_deref().unwrap()).unwrap();
        assert!(
            body.eq_ignore_ascii_case(&own.model)
                || world_rows.iter().any(|r| ai
                    .body_named(r)
                    .is_some_and(|b| b.model.eq_ignore_ascii_case(body))),
            "{name}: a body of the world's rows"
        );
        // DSV-589: the upper body is posed by the hand weapon's family, not on the rifle stand / walk clips
        assert!(k.upper_samples > 0, "{name}: upper-body samples");
        assert_eq!(k.rifle_upper, 0, "{name}: posed on the rifle clips");
    }
    // DSV-603: the kinds differ in health, and the most durable one takes several magazines of the default rifle
    // (body hits, the round's rate included)
    let rows: Vec<(f32, f32)> = kinds
        .iter()
        .filter_map(|(n, k)| {
            let t = templates
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(n.as_bytes()))?;
            let hp = ai.attributes_named(t.body_row.as_deref()?)?.hit_points?;
            Some((hp, k.health.iter().map(|h| h.0).fold(0.0, f32::max)))
        })
        .collect();
    let lo = rows.iter().map(|r| r.0).fold(f32::MAX, f32::min);
    let (hi, hi_spawned) =
        rows.iter()
            .copied()
            .fold((0.0f32, 0.0f32), |a, r| if r.0 > a.0 { r } else { a });
    assert!(hi > lo, "the kinds differ in health: {rows:?}");
    let (tables, _) = RomTables::load(&rom);
    let rifle = tables.items.issue_weapons[0].expect("a default rifle");
    let ammo = tables.weapons.ammo_of(rifle).expect("its ammunition");
    let clip = tables.weapons.weapons[&rifle].clip.expect("its clip") as f32;
    let magazine = clip * tables.weapons.damage(rifle, ammo).expect("its damage");
    assert!(
        hi_spawned > 2.0 * magazine,
        "the durable kind ({hi_spawned}) outlasts two magazines ({magazine} each)"
    );
    // DSV-604: every kind's own body row names a gear row that protects the kinds' death blasts in full (so the
    // blasts of the infected do not hurt the infected) and the default rifle's damage type not at all
    let death_types: Vec<u8> =
        ai.brains
            .iter()
            .filter_map(|b| b.death_blast.as_deref())
            .filter_map(|n| {
                let (w, r) =
                    tables.weapons.weapons.iter().find(|(_, r)| {
                        r.name.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(n))
                    })?;
                tables.weapons.area(*w, r.ammo?)?.damage_type
            })
            .collect();
    assert!(!death_types.is_empty(), "a death blast with a damage type");
    let rifle_type = tables.weapons.damage_type(rifle, ammo).expect("its type");
    for name in kinds.keys() {
        let gear = spec_of(name).gear.expect("the body's gear row");
        let row = &tables.weapons.gear[&gear];
        for t in &death_types {
            assert_eq!(row.share(*t, 2), 1.0, "{name}: type {t} protected");
        }
        assert_eq!(row.share(rifle_type, 2), 0.0, "{name}: the rifle hurts");
    }
    // at least one kind came close enough to strike, and walked at its speed while it walked
    assert!(kinds.values().any(|k| k.strikes > 0), "a strike: {kinds:?}");
    for (name, k) in &kinds {
        if k.walk_ms >= 2_000 {
            let mean = k.walked / (k.walk_ms as f32 / 1000.0);
            assert!(
                mean > 0.5 * k.speed,
                "{name}: walked {mean:.0} u/s on average"
            );
        }
    }
    // the mission world's AI keep their guns and `coop.speed`
    if let Some(m) = mission {
        let dt = m
            .coop
            .templates
            .iter()
            .filter(|t| t.template && t.right_hand.is_some())
            .count();
        assert!(dt > 0, "the mission world's armed templates");
        let brains = m
            .coop
            .templates
            .iter()
            .filter_map(|t| ai.brain_named(t.brain.as_deref()?))
            .filter(|b| b.melee_only())
            .count();
        assert_eq!(brains, 0, "no melee-only brain on the mission world");
    }
}

/// What one kind's death did to the human standing next to it, and its rush.
#[derive(Debug, Default, Clone)]
struct Death {
    /// The template measured.
    template: String,
    /// Health and armour lost in the 3 s after the kill.
    lost: f32,
    /// Thrown objects in the air just after the kill.
    flights: usize,
    clouds: usize,
    walk: Option<f32>,
    rush: Option<f32>,
    speed: f32,
}

/// DSV-597..DSV-601 against the ROM rows: every kind of the first rounds (Hard, up to the round that first sends the
/// last kind) is killed once when it has come to strike the human holding the start, with no hazard of an earlier kill
/// left; what the human lost in the 3 s after it (health and armour), the objects and clouds it left, and its rush.
/// The kind whose brain names a blast weapon hurts him, the kind whose brain names a dropped weapon drops that many
/// objects, which hurt him, the kind whose brain names a progressive-damage weapon leaves a cloud that hurts him, the
/// kind whose brain names none of these does not. The rush: each melee kind walks at its attribute row's walk speed
/// below its run speed until a human is within its row's sight distance. Outside a measurement the human is healed
/// and the other AI are killed far from him. No ROM value is written here.
// OBSERVED: ROM CabinFever and AIBUTES.TXT / WEAPONS.TXT / MODELBUTES.TXT (read at run time)
// DC-DEFAULT: DSV-597..DSV-601 (proposed)
#[test]
fn rom_cabin_fever_infected_death_specials_and_rush() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (key, Some(w), _) = coop_worlds(&rom) else {
        println!("skipped: no co-op round world");
        return;
    };
    let vfs = RezVfs::new(&rom);
    let ai = AiTables::from_vfs(&vfs);
    let (tables, _) = RomTables::load(&rom);
    let weapons = tables.weapons.clone();
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    cfg.game.fall.enabled = false;
    cfg.game.coop.hit_chance = [0.0; 3];
    cfg.game.coop.melee_hit = 0.0;
    let trig = trig_field(&rom, &key, &w.announce, 1);
    let mut server = Server::with_tables(cfg, w.clone(), tables, 0);
    assert!(server.handle_line(
        0,
        &format!("room key={key} type=5 goal=1 ff=0 time=36000{trig}")
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
    let vitals = |s: &Server<u32>| {
        let h = &s.game().players()[&pid];
        h.health + h.armor
    };
    let mut done: BTreeMap<String, Death> = BTreeMap::new();
    let mut born: BTreeMap<usize, u32> = BTreeMap::new();
    // the kind measured, the kill's time, the vitals before it
    let mut measuring: Option<(String, u32, f32)> = None;
    let end = now + 1_500_000;
    while now < end && done.len() < 4 {
        p.client.set_height(start[1]);
        p.client.walk_to(Some(start), 100_000.0);
        step(&mut server, &mut p, now);
        if server.game().coop().is_some_and(|c| c.stage.is_some()) {
            break;
        }
        let human = server.game().players()[&pid].pos;
        if let Some((kind, at, before)) = measuring.clone() {
            if now.saturating_sub(at) >= 3_000 {
                done.get_mut(&kind).unwrap().lost = before - vitals(&server);
                measuring = None;
            }
        } else {
            server.harness_vitals(pid, 100.0, None);
        }
        let quiet = server.game().flights_in_air() == 0
            && server.game().burning().is_empty()
            && server.game().coop().is_some_and(|c| c.clouds.is_empty());
        let ais: Vec<_> = server.game().ai_chars().to_vec();
        for a in &ais {
            if a.state != AiState::Alive {
                born.remove(&a.slot);
                continue;
            }
            let b = *born.entry(a.slot).or_insert(now);
            if now.saturating_sub(b) < 250 {
                continue;
            }
            // a kind is its brain row (several templates share one)
            let kind = a
                .template
                .as_deref()
                .and_then(|t| w.coop.template(t))
                .and_then(|t| t.brain.clone())
                .unwrap_or_else(|| a.spec.key.clone())
                .to_ascii_uppercase();
            let d = dist(a.pos, human);
            let known = done.contains_key(&kind);
            if !known && measuring.is_none() && quiet && d < 110.0 {
                server.harness_vitals(pid, 100.0, None);
                let before = vitals(&server);
                server.harness_kill_ai(now, a.slot, pid);
                done.insert(
                    kind.clone(),
                    Death {
                        template: a.spec.key.clone(),
                        flights: server.game().flights_in_air(),
                        clouds: server.game().coop().map_or(0, |c| c.clouds.len()),
                        walk: a.spec.walk,
                        rush: a.spec.rush,
                        speed: a.spec.speed,
                        ..Death::default()
                    },
                );
                measuring = Some((kind, now, before));
            } else if known && (measuring.is_none() || d > 450.0) {
                server.harness_kill_ai(now, a.slot, pid);
            }
        }
        now += 10;
    }
    if let Some((kind, at, before)) = measuring.clone() {
        while now < at + 3_000 {
            step(&mut server, &mut p, now);
            now += 10;
        }
        done.get_mut(&kind).unwrap().lost = before - vitals(&server);
    }
    println!(
        "{done:#?}
round {:?} at {} s",
        server.game().coop().map(|c| c.round),
        now / 1000
    );
    assert_eq!(done.len(), 4, "the four kinds: {:?}", done.keys());
    let templates = &w.coop.templates;
    for k in done.values() {
        let name = &k.template;
        let t = templates
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name.as_bytes()))
            .expect("the template");
        let brain = ai.brain_named(t.brain.as_deref().unwrap()).unwrap();
        let weapon_of = |n: &str| {
            weapons
                .weapons
                .iter()
                .find(|(_, w)| w.name.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(n)))
                .map(|(id, _)| *id)
        };
        // DSV-597: the walk below the run speed until the row's sight distance
        let row = ai.attributes_named(t.body_row.as_deref().unwrap()).unwrap();
        assert_eq!(k.walk, row.walk_speed, "{name}: walk");
        assert_eq!(k.rush, row.see_enemy, "{name}: rush distance");
        assert!(
            k.walk.unwrap() < k.speed,
            "{name}: walks slower than it runs"
        );
        match (brain.death_blast.as_deref(), brain.death_throw.as_ref()) {
            (Some(b), None) => {
                let wid = weapon_of(b).expect("the blast weapon row");
                let ammo = weapons.ammo_of(wid).unwrap();
                if weapons.area(wid, ammo).is_some() {
                    // DSV-598
                    assert!(k.lost > 0.0, "{name}: the blast hurt him: {k:?}");
                    assert_eq!((k.flights, k.clouds), (0, 0), "{name}");
                } else {
                    // DSV-600
                    assert!(weapons.prog(wid, ammo).is_some(), "{name}: a cloud row");
                    assert_eq!(k.clouds, 1, "{name}: a cloud");
                    assert!(k.lost > 0.0, "{name}: the cloud hurt him: {k:?}");
                }
            }
            (None, Some((thrown, n))) => {
                // DSV-599
                assert!(weapon_of(thrown).is_some(), "{name}: the thrown row");
                assert_eq!(k.flights, *n as usize, "{name}: dropped objects");
                assert!(k.lost > 0.0, "{name}: they hurt him: {k:?}");
            }
            (None, None) => {
                assert_eq!((k.flights, k.clouds), (0, 0), "{name}");
                assert_eq!(k.lost, 0.0, "{name}: a plain death: {k:?}");
            }
            other => panic!("{name}: both death keys {other:?}"),
        }
    }
    // the kinds whose death is a blast or a drop rush from farther than the plain kind
    let plain = done
        .iter()
        .find(|(brain, _)| {
            let b = ai.brain_named(brain).unwrap();
            b.death_blast.is_none() && b.death_throw.is_none()
        })
        .map(|(_, k)| k.rush.unwrap())
        .expect("a plain kind");
    assert!(
        done.values().filter(|k| k.rush.unwrap() > plain).count() >= 2,
        "two kinds rush from farther: {done:?}"
    );
}
