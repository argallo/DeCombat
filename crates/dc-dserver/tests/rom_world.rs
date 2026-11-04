//! ROM-gated (skipped when CA_ROM is unset): the server loads the room's world from the user's own archive the way
//! the lobby names it (`Worlds\<name>`), finds player spawn points by `dc-world` role, and a synthetic client
//! reaches its object at one of them. Nothing from the ROM is asserted by value; output is counts and hashes only.

use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig};
use dc_dserver::world::{load_from_rom, NameForm, WorldInfo, WORLD_ARCHIVE, WORLD_EXT};
use dc_net_engine::sim::{Faults, SimChannel};

fn world_entries(rom: &std::path::Path) -> Option<Vec<String>> {
    let arch = dc_rez::RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE)).ok()?;
    Some(
        arch.entries()
            .iter()
            .map(|e| e.0.clone())
            .filter(|n| n.to_ascii_uppercase().ends_with(WORLD_EXT))
            .collect(),
    )
}

/// `Worlds\<name>` as the lobby passes it (dc-bute world_for_room form).
fn lobby_arg(listed: &str) -> String {
    let base = listed.rsplit('\\').next().unwrap_or(listed);
    format!("Worlds\\{}", &base[..base.len() - WORLD_EXT.len()])
}

fn load_some(rom: &std::path::Path, n: usize) -> Vec<WorldInfo> {
    let Some(entries) = world_entries(rom) else {
        return Vec::new();
    };
    entries
        .iter()
        .take(n)
        .map(|listed| {
            let w = load_from_rom(rom, &lobby_arg(listed), NameForm::Listed, None).unwrap_or_else(
                |e| {
                    panic!(
                        "world {}: {e}",
                        dc_rez::dc_rom::sha256_hex(listed.as_bytes())
                    )
                },
            );
            assert_eq!(&w.announce, listed, "announced exactly as listed");
            w
        })
        .collect()
}

#[test]
fn rom_worlds_load_with_player_spawns() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let worlds = load_some(&rom, 4);
    if worlds.is_empty() {
        println!("skipped: world archive unavailable");
        return;
    }
    let mut with_spawns = 0;
    for w in &worlds {
        for i in 0..3 {
            assert!(
                w.extent_min[i] < w.extent_max[i],
                "box of {}",
                &w.sha256[..16]
            );
        }
        let inside = w
            .spawns
            .iter()
            .filter(|s| {
                (0..3)
                    .all(|i| s.pos[i] >= w.extent_min[i] - 1.0 && s.pos[i] <= w.extent_max[i] + 1.0)
            })
            .count();
        println!(
            "world sha256 {}: {} bytes, {} objects, {} player spawns ({} inside the box)",
            &w.sha256[..16],
            w.size,
            w.objects,
            w.spawns.len(),
            inside
        );
        assert_eq!(
            inside,
            w.spawns.len(),
            "every spawn point lies inside the world box"
        );
        if !w.spawns.is_empty() {
            with_spawns += 1;
        }
    }
    assert!(
        with_spawns >= 1,
        "at least one world has player spawn points"
    );
}

/// DSV-34: every ROM world has a level model to trace shots against; it is solid under the spawn points (a ray
/// from 60 units above a spawn point down 600 units meets it) and open above them (a short ray up is free). Ray
/// counts between the two teams' spawn points show the trace refuses some lines and accepts others. Counts only.
#[test]
fn rom_worlds_trace_geometry_under_and_between_spawns() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let worlds = load_some(&rom, 4);
    if worlds.is_empty() {
        println!("skipped: world archive unavailable");
        return;
    }
    for w in &worlds {
        let t = w.trace.as_ref().expect("a level model");
        let v = |p: [f32; 3]| dc_sim::V3::from_array(p);
        let mut floor = 0;
        let mut open = 0;
        for s in &w.spawns {
            let eye = [s.pos[0], s.pos[1] + 60.0, s.pos[2]];
            if t.ray(v(eye), v([eye[0], eye[1] - 600.0, eye[2]])).is_some() {
                floor += 1;
            }
            if t.ray(v(eye), v([eye[0], eye[1] + 20.0, eye[2]])).is_none() {
                open += 1;
            }
        }
        let (mut clear, mut blocked) = (0, 0);
        for a in w.spawns.iter().step_by(7) {
            for b in w.spawns.iter().step_by(5) {
                let ea = v([a.pos[0], a.pos[1] + 60.0, a.pos[2]]);
                let eb = v([b.pos[0], b.pos[1] + 60.0, b.pos[2]]);
                if (ea - eb).len() < 1.0 {
                    continue;
                }
                match t.ray(ea, eb) {
                    Some(_) => blocked += 1,
                    None => clear += 1,
                }
            }
        }
        println!(
            "world {}: {} spawns, floor under {floor}, open above {open}; spawn-to-spawn lines clear {clear} blocked {blocked}",
            &w.sha256[..16],
            w.spawns.len()
        );
        let n = w.spawns.len();
        assert!(floor * 10 >= n * 9, "the level is solid under the spawns");
        assert!(open * 10 >= n * 9, "and open above them");
        assert!(
            clear > 0 && blocked > 0,
            "the trace both accepts and refuses lines"
        );
    }
}

#[test]
fn rom_world_slice_puts_the_player_on_a_spawn_point() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(w) = load_some(&rom, 4)
        .into_iter()
        .find(|w| !w.spawns.is_empty())
    else {
        println!("skipped: no world with spawn points");
        return;
    };
    let spawns: Vec<[f32; 3]> = w.spawns.iter().map(|s| s.pos).collect();
    let mut server: Server<u32> = Server::new(ServerTuning::default(), w, 0);
    server.handle_line(0, "user 7 3 99 0 120 Dave");
    let mut client = SynthClient::new(SynthConfig::named("Dave"));
    let (mut up, mut down) = (
        SimChannel::new(1, Faults::clean(2)),
        SimChannel::new(2, Faults::clean(2)),
    );
    let mut now = 0u32;
    while now < 20_000 && client.state() != ClientState::InWorld {
        for d in client.poll(now) {
            up.send(now, d);
        }
        for d in up.deliver(now) {
            server.handle_datagram(now, 1, &d);
        }
        for o in server.poll(now) {
            if let Output::Transmit { bytes, .. } = o {
                down.send(now, bytes);
            }
        }
        for d in down.deliver(now) {
            client.handle_datagram(now, &d);
        }
        now += 5;
    }
    assert_eq!(client.state(), ClientState::InWorld);
    let obj = client.seen.view_object.unwrap();
    let pos = client.seen.objects[&obj].pos.unwrap();
    assert!(
        spawns.contains(&pos),
        "the player's object stands on a spawn point"
    );
}

#[test]
fn rom_binary_loads_the_lobby_world_argument_and_reports_ready() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(listed) = world_entries(&rom).and_then(|e| e.into_iter().next()) else {
        println!("skipped: world archive unavailable");
        return;
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_dc-dserver"))
        .args(["-port", "0", "-guid", "ca", "-idle", "10", "-world"])
        .arg(lobby_arg(&listed))
        .env("CA_ROM", &rom)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut ready = None;
    for l in lines.by_ref().map_while(Result::ok) {
        if l.starts_with("@ready") {
            ready = Some(l);
            break;
        }
        if l == "@end" {
            break;
        }
    }
    let ok = ready
        .as_deref()
        .is_some_and(|r| r.contains(" port=") && !r.contains("port=0 "));
    let _ = writeln!(child.stdin.as_mut().unwrap(), "quit");
    let rest: Vec<String> = lines.map_while(Result::ok).collect();
    let status = child.wait().expect("wait");
    assert!(ok, "no @ready from a ROM world");
    assert_eq!(rest.last().map(String::as_str), Some("@end"));
    assert!(status.success());
}

/// DSV-39 (amended): the ROM has weapons that are their own ammunition (`IsAmmo`); each holds one in the hand
/// (`ShotsPerClip` 1) and names an ammunition row with a granted amount, so the spawn mirror takes the hand's round out
/// of the grant. Counts only.
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] IsAmmo, ShotsPerClip; [Ammo<n>] MaxAmount
#[test]
fn rom_own_ammo_weapons_hold_one_round_in_the_hand() {
    use dc_dserver::rom_tables::RomTables;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, notes) = RomTables::load(&rom);
    assert!(notes.is_empty(), "{notes:?}");
    let wt = &tables.weapons;
    let own: Vec<u16> = wt
        .weapons
        .iter()
        .filter(|(_, r)| r.own_ammo)
        .map(|(n, _)| *n)
        .collect();
    assert!(!own.is_empty(), "no IsAmmo weapon");
    for w in &own {
        assert_eq!(
            wt.clip(*w),
            Some(1),
            "an own-ammo weapon with a clip other than 1"
        );
        let granted = wt.ammo_of(*w).and_then(|a| wt.max_amount(a));
        assert!(
            granted.is_some_and(|g| g >= 1),
            "an own-ammo weapon without a grant"
        );
    }
    eprintln!("{} own-ammo weapon rows", own.len());
}

/// Slice 2 on 4 ROM worlds with the ROM game tables: two players of different teams spawn at spawn points of
/// their own team (team filter = the ROM team name), the drawn weapon of the loadout is a ROM weapon, its ROM
/// damage reaches the victim, and the round ends on the goal of one kill. Counts and hashes only.
#[test]
fn rom_worlds_tdm_round_with_team_spawns_and_rom_damage() {
    use dc_dserver::game::GameTuning;
    use dc_dserver::rom_tables::RomTables;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (mut tables, notes) = RomTables::load(&rom);
    assert!(notes.is_empty(), "{notes:?}");
    // the synthetic client shoots across the map; the weapon picked below may be the knife, whose row Range would
    // refuse such claims (DSV-161, its own test: game_sim knife_swings_kill_in_range_and_do_nothing_beyond_it)
    for row in tables.weapons.weapons.values_mut() {
        row.range = None;
    }
    let wt = &tables.weapons;
    // a weapon whose clip carries a kill (100 health) and whose ammunition has a damage value
    let weapon = wt
        .weapons
        .iter()
        .filter_map(|(&w, row)| {
            let a = row.ammo?;
            let d = wt.damage(w, a)?;
            let clip = row.clip.unwrap_or(0);
            (d >= 1.0 && d * clip as f32 >= 100.0 && row.pellets.unwrap_or(1) <= 1)
                .then_some((w, a, d))
        })
        .next()
        .expect("a ROM weapon with damage and clip");
    let worlds = load_some(&rom, 4);
    assert_eq!(worlds.len(), 4);
    for w in worlds {
        let team_spawns: Vec<Vec<[f32; 3]>> = [1u8, 2]
            .iter()
            .map(|&t| {
                w.spawns
                    .iter()
                    .filter(|s| tables.teams.team_of(s.team_filter.as_deref()) == Some(t))
                    .map(|s| s.pos)
                    .collect()
            })
            .collect();
        let cfg = ServerTuning {
            game: GameTuning {
                default_loadout: vec![weapon.0],
                // the synthetic client shoots across the map from spawn to spawn: the world trace (DSV-34, its own
                // test above) would refuse most of these lines
                trace: false,
                ..GameTuning::default()
            },
            // the shots cross the map from spawn to spawn: culling (its own test, vis_rom.rs) would park the target
            vis: dc_dserver::visibility::VisTuning::off(),
            ..ServerTuning::default()
        };
        let wbox = (w.extent_min, w.extent_max);
        let sha = w.sha256[..16].to_string();
        let mut server: Server<u32> = Server::with_tables(cfg, w, tables.clone(), 0);
        server.handle_line(0, "room key=TDM type=1 goal=1 ff=0");
        server.handle_line(0, "user 7 3 99 0 120 Dave");
        server.handle_line(0, "user 8 4 55 1 0 Erin");
        let mut clients: Vec<SynthClient> = ["Dave", "Erin"]
            .iter()
            .map(|n| {
                let mut c = SynthConfig::named(n);
                c.box_bounds = Some(wbox);
                c.walk_step = 0.0;
                SynthClient::new(c)
            })
            .collect();
        let mut chans: Vec<(SimChannel, SimChannel)> = (0..2u64)
            .map(|i| {
                (
                    SimChannel::new(10 + i, Faults::clean(2)),
                    SimChannel::new(20 + i, Faults::clean(2)),
                )
            })
            .collect();
        let mut now = 0u32;
        let mut step = |server: &mut Server<u32>, clients: &mut Vec<SynthClient>, now: u32| {
            for (i, c) in clients.iter_mut().enumerate() {
                for d in c.poll(now) {
                    chans[i].0.send(now, d);
                }
            }
            for (i, ch) in chans.iter_mut().enumerate() {
                for d in ch.0.deliver(now) {
                    server.handle_datagram(now, i as u32 + 1, &d);
                }
            }
            for o in server.poll(now) {
                if let Output::Transmit { to, bytes } = o {
                    chans[to as usize - 1].1.send(now, bytes);
                }
            }
            for (i, ch) in chans.iter_mut().enumerate() {
                for d in ch.1.deliver(now) {
                    clients[i].handle_datagram(now, &d);
                }
            }
        };
        while now < 30_000 && !clients.iter().all(|c| c.life_state() == 1) {
            step(&mut server, &mut clients, now);
            now += 5;
        }
        assert!(
            clients.iter().all(|c| c.life_state() == 1),
            "world {sha}: not alive"
        );
        for _ in 0..100 {
            step(&mut server, &mut clients, now);
            now += 5;
        }
        // own team's spawn points (when the world has team-filtered points); DSV-48: on the floor below the point (or
        // lifted out of the floor by at most the spawn lift where the point lies closer to it than the box's half height)
        for (i, c) in clients.iter().enumerate() {
            let p = c.seen.teleports[0].2;
            if !team_spawns[i].is_empty() {
                assert!(
                    team_spawns[i].iter().any(|s| s[0] == p[0]
                        && s[2] == p[2]
                        && p[1] <= s[1] + dc_dserver::game::SPAWN_LIFT + 0.01),
                    "world {sha}: client {i} not on a team {} point: {p:?} in {:?}",
                    i + 1,
                    team_spawns[i]
                );
            }
            assert_eq!(
                c.seen.weapons.last().map(|x| (x.0, x.1, x.2)),
                Some((0xb3, weapon.0, weapon.1))
            );
            assert!(c.seen.errors.is_empty(), "world {sha}: {:?}", c.seen.errors);
        }
        let target = clients[1].seen.view_object.unwrap();
        let mut shots = 0;
        while clients[1].seen.kills.is_empty() && shots < 200 {
            assert!(clients[0].fire_at(target, [0.0; 3], true));
            shots += 1;
            for _ in 0..4 {
                step(&mut server, &mut clients, now);
                now += 5;
            }
        }
        let dmg = clients[1].seen.damage.clone();
        assert!(!dmg.is_empty(), "world {sha}: no damage");
        assert_eq!(
            dmg[0].1,
            weapon.2.round().max(1.0) as u32,
            "ROM damage per hit"
        );
        assert_eq!(clients[1].seen.kills.len(), 1);
        for _ in 0..20 {
            step(&mut server, &mut clients, now);
            now += 5;
        }
        assert_eq!(clients[0].seen.round_result, Some((1, 0, 0)), "world {sha}");
        println!(
            "world sha256 {sha}: team spawn points {}/{}, kill after {} hits, round over",
            team_spawns[0].len(),
            team_spawns[1].len(),
            dmg.len()
        );
    }
}

/// ROM-gated, slice 3: on a ROM world with the ROM tables, a two-player session in the default (reference) order
/// shows each player to the other as a character resolved from the ROM at run time (kind 1, type-0x18 blob with the
/// player id, body + child + face models, a weapon model attached at the grip socket), streams the standing
/// animation pair of the ROM rig, gives the own body with friction, and switches to the drawn weapon with 0x7f when
/// the weapon is in the ROM command table. A second reader parses every object group. Counts and hashes only.
#[test]
fn rom_characters_in_a_two_player_session() {
    use dc_dserver::game::{Game, GameTuning};
    use dc_dserver::rom_tables::RomTables;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, notes) = RomTables::load(&rom);
    assert!(notes.is_empty(), "{notes:?}");
    // the first weapon of the command table that has a third-person model
    let weapon = tables
        .weapons
        .commands
        .iter()
        .copied()
        .find(|w| tables.weapons.hand_model(*w).is_some())
        .expect("a weapon with a hand model");
    let w = load_some(&rom, 1).pop().expect("a world");
    let cfg = ServerTuning {
        game: GameTuning {
            default_loadout: vec![weapon],
            ..GameTuning::default()
        },
        // the streamed animation of the other character is checked: no culling between the two spawns
        vis: dc_dserver::visibility::VisTuning::off(),
        ..ServerTuning::default()
    };
    let mut expect = Game::new(cfg.game.clone(), &w, tables.clone());
    let look = expect
        .character_for(None)
        .unwrap()
        .expect("a ROM character");
    let wbox = (w.extent_min, w.extent_max);
    let mut server: Server<u32> = Server::with_tables(cfg, w, tables.clone(), 0);
    server.handle_line(0, "room key=TDM type=1 goal=1 ff=0");
    server.handle_line(0, "user 7 3 99 0 120 Dave");
    server.handle_line(0, "user 8 4 55 1 0 Erin");
    let mut clients: Vec<SynthClient> = ["Dave", "Erin"]
        .iter()
        .map(|n| {
            let mut c = SynthConfig::named(n);
            c.box_bounds = Some(wbox);
            c.walk_step = 0.0;
            c.weapon_commands = Some(tables.weapons.commands.clone());
            SynthClient::new(c)
        })
        .collect();
    let mut chans: Vec<(SimChannel, SimChannel)> = (0..2u64)
        .map(|i| {
            (
                SimChannel::new(30 + i, Faults::clean(2)),
                SimChannel::new(40 + i, Faults::clean(2)),
            )
        })
        .collect();
    let mut now = 0u32;
    while now < 40_000 {
        for (i, c) in clients.iter_mut().enumerate() {
            for d in c.poll(now) {
                chans[i].0.send(now, d);
            }
        }
        for (i, ch) in chans.iter_mut().enumerate() {
            for d in ch.0.deliver(now) {
                server.handle_datagram(now, i as u32 + 1, &d);
            }
        }
        for o in server.poll(now) {
            if let Output::Transmit { to, bytes } = o {
                chans[to as usize - 1].1.send(now, bytes);
            }
        }
        for (i, ch) in chans.iter_mut().enumerate() {
            for d in ch.1.deliver(now) {
                clients[i].handle_datagram(now, &d);
            }
        }
        now += 5;
        if clients
            .iter()
            .all(|c| c.life_state() == 1 && c.seen.remote.values().any(|r| r.anim_entries > 20))
        {
            break;
        }
    }
    let command = tables.weapons.command_of(weapon).unwrap();
    for (i, c) in clients.iter().enumerate() {
        let s = &c.seen;
        assert!(s.errors.is_empty(), "client {i}: {:?}", s.errors);
        assert_eq!(c.life_state(), 1);
        assert_eq!(s.bodies.len(), 1, "client {i}: own body");
        assert_eq!(s.frictions, vec![5.0, 5.0, 5.0]);
        assert!(s.weapon_switches.contains(&(command, true)));
        let other = clients[1 - i].seen.view_object.unwrap();
        let o = &s.objects[&other];
        assert_eq!((o.kind, o.blob_type), (1, Some(0x18)));
        assert_eq!(o.blob_pid, clients[1 - i].seen.client_id.map(|x| x.0 as u8));
        assert_eq!(
            o.models.len(),
            look.assets.body.len() + usize::from(look.assets.face.is_some())
        );
        assert_eq!(o.flags, Some(0x3803));
        assert_eq!(o.attached.len(), 1);
        assert_eq!(o.attached[0].1, look.assets.rig.grip_socket);
        let r = s.remote[&other];
        assert_eq!(
            r.last_anim,
            [
                look.anims.main.0,
                look.anims.upper[0].0,
                look.anims.lower[0].0
            ]
        );
        let mut t = dc_botclient::objects::ObjectTable::default();
        for g in &s.groups {
            t.apply_group(g);
        }
        assert!(t.errors.is_empty(), "client {i}: {:?}", t.errors);
    }
    println!(
        "ROM session: weapon {weapon} command {command:#04x}, {} files announced, character models {}, stream animations {:?}",
        clients[0].seen.files.len(),
        look.assets.body.len() + usize::from(look.assets.face.is_some()),
        clients[0].seen.remote.values().map(|r| r.last_anim).collect::<Vec<_>>()
    );
}

/// Four ROM worlds that have objective zones or capture flags (else the first four).
fn worlds_with_objects(rom: &std::path::Path) -> Vec<WorldInfo> {
    let Some(entries) = world_entries(rom) else {
        return Vec::new();
    };
    let all: Vec<WorldInfo> = entries
        .iter()
        .filter_map(|listed| load_from_rom(rom, &lobby_arg(listed), NameForm::Listed, None).ok())
        .collect();
    let mut with: Vec<WorldInfo> = all
        .iter()
        .filter(|w| !w.zones.is_empty() || !w.flags.is_empty())
        .take(4)
        .cloned()
        .collect();
    for w in all {
        if with.len() >= 4 {
            break;
        }
        if !with.iter().any(|x| x.sha256 == w.sha256) {
            with.push(w);
        }
    }
    with
}

/// DSV-37 on 4 ROM worlds: every objective zone and every capture flag of the world file becomes a create record
/// (type 0x37 zone on a kind-0 object, type 0x17 kind 2 flag on a kind-1 model object), `-worldobjects all` adds a
/// kind-2 record per named brush; the records parse with our synthetic client's reader and with dc-botclient's object
/// table, the flags' teams resolve through the ROM team names and their files exist in the ROM. Counts only.
#[test]
fn rom_worlds_world_objects_parse_with_two_readers() {
    use dc_dserver::objects::PosBox;
    use dc_dserver::rom_tables::RomTables;
    use dc_dserver::worldobj::{plan, WorldObject, WorldObjects, ZoneClass};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, notes) = RomTables::load(&rom);
    assert!(notes.is_empty(), "{notes:?}");
    let worlds = worlds_with_objects(&rom);
    if worlds.is_empty() {
        println!("skipped: world archive unavailable");
        return;
    }
    assert_eq!(worlds.len(), 4);
    let vfs = dc_rez::RezVfs::new(&rom);
    let (mut zones_seen, mut flags_seen) = (0, 0);
    for w in &worlds {
        let mut next = 100u16..;
        let objs = plan(w, &tables, WorldObjects::All, ZoneClass::Own, 1, || {
            next.next().unwrap()
        });
        let zones = objs
            .iter()
            .filter(|o| matches!(o, WorldObject::Zone { .. }))
            .count();
        let flags: Vec<&WorldObject> = objs
            .iter()
            .filter(|o| matches!(o, WorldObject::Flag { .. }))
            .collect();
        let brushes = objs
            .iter()
            .filter(|o| matches!(o, WorldObject::Brush { .. }))
            .count();
        let others = objs.len() - zones - flags.len() - brushes;
        assert_eq!(zones, w.zones.len());
        let water = w
            .brushes
            .iter()
            .filter(|b| b.class == dc_dserver::world::BrushClass::Water)
            .count();
        assert_eq!(brushes, w.brushes.len() - water);
        assert_eq!(
            others,
            w.volumes.len() + w.sprites.len() + w.effects.len(),
            "volumes, sprites and effects"
        );
        assert_eq!(
            flags.len(),
            w.flags.len(),
            "every flag has a model and a team"
        );
        // file ids as a session would announce them
        let mut files: Vec<String> = objs.iter().flat_map(|o| o.files()).collect();
        files.dedup();
        for f in &files {
            assert!(
                dc_dserver::chars::resource_size(&vfs, f).is_some(),
                "world object file {} not in the ROM",
                dc_dserver::chars::short_hash(f)
            );
        }
        let id_of = |p: &str| {
            files
                .iter()
                .position(|f| f == p)
                .map_or(0xFFFF, |i| 10 + i as u16)
        };
        let pbox = PosBox {
            min: w.extent_min,
            max: w.extent_max,
        };
        let mut body = dc_wire::udp::BitWriter::new();
        for o in &objs {
            let rec = o.record(&pbox, &id_of);
            body.write_u32(rec.bit_len() as u32).append(&rec);
        }
        let mut group = dc_wire::udp::BitWriter::new();
        group.write_u8(8).append(&body);
        let mut c = SynthClient::new(SynthConfig::named("x"));
        c.read_group_for_test(&group).expect("synthetic reader");
        assert_eq!(c.seen.objects.len(), objs.len());
        let mut t = dc_botclient::objects::ObjectTable::default();
        t.apply_group(&body);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        assert_eq!(t.creates as usize, objs.len());
        for o in &objs {
            let seen = &c.seen.objects[&o.obj()];
            match o {
                WorldObject::Zone { .. } | WorldObject::Uplink { .. } => {
                    assert_eq!((seen.kind, seen.blob_type), (0, Some(0x37)))
                }
                WorldObject::Flag { team, .. } => {
                    assert_eq!((seen.kind, seen.blob_type), (1, Some(0x17)));
                    assert!(*team == 1 || *team == 2);
                }
                WorldObject::Brush { .. } => assert_eq!(seen.kind, 2),
                WorldObject::Volume { .. } => {
                    assert_eq!((seen.kind, seen.blob_type), (9, Some(0x0b)))
                }
                WorldObject::Sprite { .. } => assert_eq!(seen.kind, 3),
                WorldObject::Effect { .. } => {
                    assert_eq!((seen.kind, seen.blob_type), (0, Some(0x64)))
                }
                WorldObject::Sound { .. } => unreachable!("plan_scene plans no sounds"),
            }
        }
        zones_seen += zones;
        flags_seen += flags.len();
        println!(
            "world {}: {} zones, {} flags, {} brushes; {} files",
            &w.sha256[..16],
            zones,
            flags.len(),
            brushes,
            files.len()
        );
    }
    assert!(
        zones_seen > 0 && flags_seen > 0,
        "zones and flags exist in the ROM worlds"
    );
}

/// The capture of the two-player run (the real client against the reference stack) when present.
fn tdm2_capture() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("DC_TDM2_CAPTURE") {
        let p = std::path::PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let p = std::path::PathBuf::from(std::env::var_os("DC_CAPTURES")?)
        .join("20261025_001409_tdm2/capture.jsonl");
    p.is_file().then_some(p)
}

/// DSV-37 against the reference stack (REFERENCE forms the real client accepted in capture tdm2, connection 6): with the reference stack's
/// zone class (the room's, `-zoneclass room`) our zone and flag records equal the reference stack's blobs bit for bit, the
/// flags name the same files (compared by SHA-256), and `-worldobjects all` names the same brushes; the brush flag
/// words (UNPROVEN rule) are reported as an agreement count. Skipped without the capture or the ROM.
#[test]
fn rom_world_objects_match_the_reference_tdm2() {
    use dc_dserver::rom_tables::RomTables;
    use dc_dserver::worldobj::{flag_blob, plan, zone_blob, WorldObject, WorldObjects, ZoneClass};
    use dc_net_engine::{Link, LinkConfig, LinkEvent};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(cap) = tdm2_capture() else {
        println!("skipped: no tdm2 capture");
        return;
    };
    let unhex = |s: &str| -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    };
    let text = std::fs::read_to_string(&cap).unwrap();
    let mut link: Option<Link> = None;
    let mut msgs = Vec::new();
    for line in text.lines() {
        let Ok(j) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if j["ev"] != "dgram" || j["dir"] != "s2c" || j["conn"].as_i64() != Some(6) {
            continue;
        }
        let ms = (j["t"].as_f64().unwrap() * 1000.0) as u32;
        let l = link.get_or_insert_with(|| Link::new(ms, LinkConfig::default()));
        l.handle_datagram(ms, &unhex(j["hex"].as_str().unwrap()));
        for ev in l.poll(ms) {
            if let LinkEvent::Message { bits, .. } = ev {
                msgs.push(bits);
            }
        }
    }
    let mut files: std::collections::BTreeMap<u16, Vec<u8>> = Default::default();
    let mut capture_world = None;
    // reference creates: (kind, blob, name of a kind-2 record, flag words, model files)
    let mut zones = Vec::new();
    let mut flags = Vec::new();
    let mut brushes = Vec::new();
    for m in &msgs {
        let mut r = m.reader();
        match r.read_u8() {
            0x32 => {
                while r.remaining_bits() >= 48 {
                    let id = r.read_u16();
                    r.read_u32();
                    let n = r.read_zstr8();
                    capture_world.get_or_insert_with(|| String::from_utf8_lossy(&n).into_owned());
                    files.insert(id, n);
                }
            }
            8 => {
                while r.remaining_bits() >= 32 {
                    let len = r.read_u32() as usize;
                    let rec = r.read_bit_string(len);
                    let mut q = rec.reader();
                    let lo = q.read_u8();
                    if lo == 0 {
                        continue;
                    }
                    let cf = if lo & 0x80 != 0 {
                        lo as u16 | (q.read_u8() as u16) << 8
                    } else {
                        lo as u16
                    };
                    q.read_u16();
                    if cf & 1 == 0 {
                        continue;
                    }
                    let kb = q.read_u8();
                    q.skip_bits(96);
                    let blob = (kb & 0x40 != 0).then(|| {
                        let n = if kb & 0x80 != 0 {
                            q.read_u16() as usize
                        } else {
                            q.read_u8() as usize
                        };
                        q.read_bit_string(n)
                    });
                    let ty = blob.as_ref().map(|b| b.reader().read_u8());
                    match (kb & 0x3f, ty) {
                        (0, Some(0x37)) => zones.push(blob.unwrap()),
                        (1, Some(0x17)) => {
                            // every file id of the model block: model, subs, textures, styles
                            let mut models = Vec::new();
                            for m in 0..q.read_bits(6) {
                                models.push(q.read_u16());
                                if m == 0 {
                                    for _ in 0..q.read_bits(6).saturating_sub(1) {
                                        models.push(q.read_u16());
                                    }
                                }
                                for _ in 0..q.read_bits(6) {
                                    models.push(q.read_u16());
                                }
                                for _ in 0..q.read_bits(4) {
                                    models.push(q.read_u16());
                                }
                            }
                            let names: Vec<Vec<u8>> = models
                                .iter()
                                .filter_map(|i| files.get(i).cloned())
                                .collect();
                            flags.push((blob.unwrap(), names));
                        }
                        (2, _) => {
                            let name = q.read_zstr8();
                            let f = (q.read_u16(), q.read_u16());
                            brushes.push((name, f));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let capture_world = capture_world.expect("a file list");
    let w =
        load_from_rom(&rom, &capture_world, NameForm::Listed, None).expect("the capture's world");
    let (tables, _) = RomTables::load(&rom);
    let mut next = 1000u16..;
    let ours = plan(&w, &tables, WorldObjects::All, ZoneClass::Room, 1, || {
        next.next().unwrap()
    });
    // zones: every reference blob is one of ours
    let our_zones: Vec<dc_wire::udp::BitWriter> = ours
        .iter()
        .filter_map(|o| match o {
            WorldObject::Zone {
                zone, team, class, ..
            } => Some(zone_blob(zone, *team, *class)),
            _ => None,
        })
        .collect();
    assert!(!zones.is_empty(), "the reference stack sent zones");
    let all_zones = zones.len();
    let kind_of = |b: &dc_wire::udp::BitWriter| {
        let mut r = b.reader();
        r.skip_bits(8 + 64 + 1 + 72 + 32 + 32);
        r.read_u32()
    };
    // the reference stack also sent a use-target zone (kind 8) of another object class, which DeCombat does not create
    let zones: Vec<dc_wire::udp::BitWriter> = zones
        .into_iter()
        .filter(|z| matches!(kind_of(z), 4 | 6))
        .collect();
    let zone_hits = zones.iter().filter(|z| our_zones.contains(z)).count();
    // flags: the same blob and the same file names
    let mut flag_hits = 0;
    for (blob, names) in &flags {
        let found = ours.iter().any(|o| match o {
            WorldObject::Flag {
                team,
                model,
                textures,
                styles,
                ..
            } => {
                let want: Vec<Vec<u8>> = std::iter::once(model)
                    .chain(textures)
                    .chain(styles)
                    .map(|p| dc_dserver::chars::announce_spelling(p).into_bytes())
                    .collect();
                flag_blob(*team) == *blob
                    && want.len() == names.len()
                    && want
                        .iter()
                        .zip(names)
                        .all(|(a, b)| a.eq_ignore_ascii_case(b))
            }
            _ => false,
        });
        if found {
            flag_hits += 1;
        }
    }
    // brushes: names and flag words
    let our_brushes: Vec<(Vec<u8>, (u16, u16))> = ours
        .iter()
        .filter_map(|o| match o {
            WorldObject::Brush { brush, .. } => {
                let f = dc_dserver::worldobj::brush_flags(brush);
                Some((brush.name.clone(), (f.0, f.1)))
            }
            _ => None,
        })
        .collect();
    let named = brushes
        .iter()
        .filter(|b| our_brushes.iter().any(|o| o.0 == b.0))
        .count();
    let words = brushes
        .iter()
        .filter(|b| our_brushes.iter().any(|o| o.0 == b.0 && o.1 == b.1))
        .count();
    println!(
        "tdm2 world {}: reference zones {all_zones}, bomb / flag zones {} (ours equal {zone_hits}), flags {} (equal \
         {flag_hits}), brushes {} (ours {}, same names {named}, same flag words {words})",
        &w.sha256[..16],
        zones.len(),
        flags.len(),
        brushes.len(),
        our_brushes.len()
    );
    assert_eq!(
        zone_hits,
        zones.len(),
        "zone blobs equal the reference stack's"
    );
    assert_eq!(
        flag_hits,
        flags.len(),
        "flag blobs and files equal the reference stack's"
    );
    assert_eq!(named, brushes.len(), "every reference brush is one of ours");
    // the fog-disable bit (0x0080) and the spinning model's 0x0080 of the second word closed the last two gaps
    assert_eq!(
        words,
        brushes.len(),
        "the flag-word rule agrees for every brush"
    );
}

/// DSV-40 (proposed): the world's global light object, read at world load, on every ROM world. The worlds that have
/// the object produce a direction of unit length that points down (y < 0) and a colour in 0..255 units (the property
/// range; the two brightness scales multiply it and may lift a channel past 255, which is sent unclamped); worlds without the object produce no light (no message). Counts only.
#[test]
fn rom_worlds_global_light_points_down_in_colour_units() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(entries) = world_entries(&rom) else {
        println!("skipped: world archive unavailable");
        return;
    };
    let (mut lit, mut dark, mut top) = (0, 0, 0.0f32);
    for listed in &entries {
        let w =
            load_from_rom(&rom, &lobby_arg(listed), NameForm::Listed, None).unwrap_or_else(|e| {
                panic!(
                    "world {}: {e}",
                    dc_rez::dc_rom::sha256_hex(listed.as_bytes())
                )
            });
        let tag = &w.sha256[..16];
        let Some(l) = w.global_light else {
            dark += 1;
            continue;
        };
        lit += 1;
        let len = l.dir.iter().map(|c| c * c).sum::<f32>().sqrt();
        assert!((len - 1.0).abs() < 1e-4, "world {tag}: dir length {len}");
        assert!(l.dir[1] < 0.0, "world {tag}: dir {:?} points down", l.dir);
        for c in l.colour {
            assert!(c.is_finite() && c >= 0.0, "world {tag}: colour {c}");
            top = top.max(c);
        }
        assert!(l.extra.is_finite() && l.extra >= 0.0, "world {tag}: extra");
        let m = dc_dserver::msg::s2c_dir_light(l.dir, l.colour, l.extra);
        assert_eq!(m.bit_len(), 8 + 7 * 32, "world {tag}: message size");
    }
    println!(
        "global light: {lit} worlds with the object, {dark} without ({} worlds), highest colour channel {top}",
        entries.len()
    );
    // the colour is in the 0..255 units of the static light records times the two scales: a few worlds scale above 1
    assert!(top <= 255.0 * 2.0, "highest channel {top}");
    assert_eq!(lit + dark, entries.len());
    assert!(lit >= 1, "at least one ROM world has the object");
}

/// A capture folder under `DC_CAPTURES` when present.
fn capture_named(folder: &str) -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("DC_CAPTURES") {
        let c = std::path::PathBuf::from(p)
            .join(folder)
            .join("capture.jsonl");
        return c.is_file().then_some(c);
    }
    None
}

/// The server's messages on one link of a capture, reassembled by our own transport reader.
fn link_messages(cap: &std::path::Path, conn: i64) -> Vec<dc_wire::udp::BitWriter> {
    use dc_net_engine::{Link, LinkConfig, LinkEvent};
    let unhex = |s: &str| -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    };
    let text = std::fs::read_to_string(cap).unwrap();
    let mut link: Option<Link> = None;
    let mut msgs = Vec::new();
    for line in text.lines() {
        let Ok(j) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if j["ev"] != "dgram" || j["dir"] != "s2c" || j["conn"].as_i64() != Some(conn) {
            continue;
        }
        let ms = (j["t"].as_f64().unwrap() * 1000.0) as u32;
        let l = link.get_or_insert_with(|| Link::new(ms, LinkConfig::default()));
        l.handle_datagram(ms, &unhex(j["hex"].as_str().unwrap()));
        for ev in l.poll(ms) {
            if let LinkEvent::Message { bits, .. } = ev {
                msgs.push(bits);
            }
        }
    }
    msgs
}

/// One create record of the object group, as far as the world-object comparison needs it.
#[derive(Debug, Clone)]
struct ReferenceCreate {
    obj: u16,
    kind: u8,
    blob: Option<dc_wire::udp::BitWriter>,
    name: Option<Vec<u8>>,
    code: Option<u16>,
    file: Option<u16>,
    flags: Option<(u16, u16, u32, u32)>,
    scale: Option<[f32; 2]>,
    pos48: Option<[u16; 3]>,
    raw_pos: Option<[f32; 3]>,
    rot: Option<[f32; 4]>,
    color: Option<[u8; 5]>,
}

/// Reads the create records of kinds 0, 2, 3 and 9 (the world-file kinds) out of one object group message; records
/// of other kinds and change records are skipped by their length.
fn world_creates(m: &dc_wire::udp::BitWriter, out: &mut Vec<ReferenceCreate>) {
    let mut r = m.reader();
    if r.read_u8() != 8 {
        return;
    }
    while r.remaining_bits() >= 32 {
        let len = r.read_u32() as usize;
        let rec = r.read_bit_string(len);
        let mut q = rec.reader();
        let lo = q.read_u8();
        if lo == 0 {
            continue;
        }
        let cf = if lo & 0x80 != 0 {
            lo as u16 | (q.read_u8() as u16) << 8
        } else {
            lo as u16
        };
        let obj = q.read_u16();
        if cf & 1 == 0 {
            continue;
        }
        let kb = q.read_u8();
        let kind = kb & 0x3f;
        if !matches!(kind, 0 | 2 | 3 | 9) {
            continue;
        }
        q.skip_bits(96);
        let blob = (kb & 0x40 != 0).then(|| {
            let n = if kb & 0x80 != 0 {
                q.read_u16() as usize
            } else {
                q.read_u8() as usize
            };
            q.read_bit_string(n)
        });
        let mut c = ReferenceCreate {
            obj,
            kind,
            blob,
            name: None,
            code: None,
            file: None,
            flags: None,
            scale: None,
            pos48: None,
            raw_pos: None,
            rot: None,
            color: None,
        };
        match kind {
            2 => c.name = Some(q.read_zstr8()),
            9 => {
                c.name = Some(q.read_zstr8());
                c.code = Some(q.read_u16());
            }
            3 => c.file = Some(q.read_u16()),
            _ => {}
        }
        let mut full = false;
        if cf & 0x0008 != 0 {
            let f = (q.read_u16(), q.read_u16(), q.read_u32(), q.read_u32());
            full = f.0 & 0x0100 != 0;
            c.flags = Some(f);
        }
        if cf & 0x0040 != 0 {
            if matches!(kind, 0 | 5 | 9) {
                continue;
            }
            c.color = Some([
                q.read_u8(),
                q.read_u8(),
                q.read_u8(),
                q.read_u8(),
                q.read_u8(),
            ]);
        }
        if cf & 0x0010 != 0 {
            let sx = q.read_f32();
            let sy = q.read_f32();
            if kind != 3 {
                q.read_f32();
            }
            c.scale = Some([sx, sy]);
        }
        // creates always carry the position block
        if full {
            c.raw_pos = Some([q.read_f32(), q.read_f32(), q.read_f32()]);
            q.skip_bits(96);
        } else {
            c.pos48 = Some([q.read_u16(), q.read_u16(), q.read_u16()]);
            q.skip_bits(72);
        }
        if cf & 0x0404 != 0 {
            c.rot = Some([q.read_f32(), q.read_f32(), q.read_f32(), q.read_f32()]);
        }
        out.push(c);
    }
}

/// The room's `trig` setting for this world in a TDM room, as our lobby writes it (dc-bute's room line).
fn tdm_trig(rom: &std::path::Path, world: &str) -> Option<String> {
    room_trig(rom, world, "TDM")
}

/// The room's `trig` setting for this world in a room of the mode with this key, as our lobby writes it.
fn room_trig(rom: &std::path::Path, world: &str, key: &str) -> Option<String> {
    let vfs = dc_rez::RezVfs::new(rom);
    let gr = dc_bute::GameRoomData::from_vfs(&vfs).ok()?;
    let unq = |s: &str| s.trim().trim_matches('"').to_string();
    let mode = gr.modes(false).iter().position(|m| {
        m.get("Key")
            .is_some_and(|k| unq(k).eq_ignore_ascii_case(key))
    })?;
    let base = |p: &str| {
        let n = p.replace('/', "\\").to_ascii_uppercase();
        let b = n.rsplit('\\').next().unwrap_or(&n).to_string();
        b.strip_suffix(".DAT").map(str::to_string).unwrap_or(b)
    };
    let want = base(world);
    for (i, m) in gr.maps().iter().enumerate() {
        if !m.contains_key(&format!("GMS_{key}")) {
            continue;
        }
        let (_md, doc) = dc_bute::gms_file_with_vfs(&vfs, &gr, i, mode, false);
        if doc
            .get_str("MapInfo", "Map")
            .is_some_and(|w| base(&unq(w)) == want)
        {
            let cfg = dc_bute::RoomConfig {
                map: i,
                mode,
                ..dc_bute::RoomConfig::default()
            };
            let line =
                dc_bute::room_line_with_vfs(&vfs, &gr, &cfg, &dc_bute::RoomOptions::default());
            return match dc_dserver::pipe::parse_control(&line) {
                dc_dserver::pipe::Control::Room(r) => r.get("trig").map(str::to_string),
                _ => None,
            };
        }
    }
    None
}

/// DSV-37 extended and DSV-45 against the reference stack's world load on the real client's link (captures tdm6 and
/// tdm4, two different worlds, connection 6; REFERENCE forms the real client accepted): with the start scripts of a
/// TDM room our `world` set names the same model-linked objects, volumes, sprites and effects as the reference stack's creates
/// of kinds 2, 9, 3 and 0 (type 0x64); brush flag words and volume records agree field by field (the brushes' surface
/// byte of the user word is not derived and is compared without it), volume and effect blobs bit for bit, sprite
/// files, flags, scale and position, and the sky table block bit for bit with its slot naming the same sky model.
/// Skipped without the ROM or the captures.
#[test]
fn rom_world_objects_match_the_reference_world_load() {
    use dc_dserver::objects::PosBox;
    use dc_dserver::rom_tables::RomTables;
    use dc_dserver::worldobj::{plan_scene, sky_table, WorldObject, WorldObjects, ZoneClass};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = RomTables::load(&rom);
    let mut checked = 0;
    for (label, folder) in [
        ("tdm6", "20261025_065016_tdm6"),
        ("tdm4", "20261025_002236_tdm4"),
    ] {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let msgs = link_messages(&cap, 6);
        let mut files: std::collections::BTreeMap<u16, Vec<u8>> = Default::default();
        let mut cap_world = None;
        let mut creates = Vec::new();
        let mut sky = None;
        for m in &msgs {
            let mut r = m.reader();
            match r.read_u8() {
                0x32 => {
                    while r.remaining_bits() >= 48 {
                        let id = r.read_u16();
                        r.read_u32();
                        let n = r.read_zstr8();
                        cap_world.get_or_insert_with(|| String::from_utf8_lossy(&n).into_owned());
                        files.insert(id, n);
                    }
                }
                8 => world_creates(m, &mut creates),
                16 if sky.is_none() => {
                    let mut block = [0u8; 48];
                    for b in block.iter_mut() {
                        *b = r.read_u8();
                    }
                    let n = r.read_u16() as usize;
                    let ids: Vec<u16> = (0..n).map(|_| r.read_u16()).collect();
                    sky = Some((block, ids));
                }
                _ => {}
            }
        }
        let cap_world = cap_world.expect("a file list");
        let w =
            load_from_rom(&rom, &cap_world, NameForm::Listed, None).expect("the capture's world");
        let trig = tdm_trig(&rom, &cap_world);
        assert!(
            trig.is_some(),
            "{label}: the TDM room line names start triggers"
        );
        let mut next = 1000u16..;
        let ours = plan_scene(
            &w,
            &tables,
            WorldObjects::World,
            ZoneClass::Room,
            1,
            trig.as_deref(),
            || next.next().unwrap(),
        );
        let pbox = PosBox {
            min: w.extent_min,
            max: w.extent_max,
        };
        let enc = |p: [f32; 3]| {
            let mut b = dc_wire::udp::BitWriter::new();
            dc_dserver::stream::pos48_encode(p, &pbox).write(&mut b);
            let mut r = b.reader();
            [r.read_u16(), r.read_u16(), r.read_u16()]
        };
        // the reference stack's `pos48` encoder rounds differently in the last step at some points (its scale is UNPROVEN,
        // UDP_TRANSPORT_2009.md §15.1): one step (at most 2 units here) is the same position
        let near = |a: [u16; 3], b: [u16; 3]| (0..3).all(|i| a[i].abs_diff(b[i]) <= 1);
        let mut exact = 0;
        let mut compact = 0;
        // brushes; doors and switches are left out on purpose (they need their own client object and the server's
        // door logic, DSV-37 extended): the reference stack's doors are counted, not compared
        let is_door = |n: &Option<Vec<u8>>| {
            w.brushes.iter().any(|b| {
                Some(&b.name) == n.as_ref()
                    && matches!(
                        b.class,
                        dc_dserver::world::BrushClass::Door | dc_dserver::world::BrushClass::Switch
                    )
            })
        };
        let doors = creates
            .iter()
            .filter(|c| c.kind == 2 && is_door(&c.name))
            .count();
        let theirs: Vec<&ReferenceCreate> = creates
            .iter()
            .filter(|c| c.kind == 2 && !is_door(&c.name))
            .collect();
        type Words = (u16, u16, u32, u32);
        let mine: Vec<(&dc_dserver::world::BrushInfo, Words)> = ours
            .iter()
            .filter_map(|o| match o {
                WorldObject::Brush { brush, .. } => {
                    Some((brush, dc_dserver::worldobj::brush_flags(brush)))
                }
                _ => None,
            })
            .collect();
        let mut names_t: Vec<Vec<u8>> = theirs.iter().filter_map(|c| c.name.clone()).collect();
        let mut names_m: Vec<Vec<u8>> = mine.iter().map(|b| b.0.name.clone()).collect();
        names_t.sort();
        names_m.sort();
        assert_eq!(names_m, names_t, "{label}: the same model-linked objects");
        let mut words = 0;
        let mut users = 0;
        for c in &theirs {
            let (b, f) = mine
                .iter()
                .find(|b| Some(&b.0.name) == c.name.as_ref())
                .unwrap();
            let of = c.flags.unwrap();
            if (f.0, f.1) == (of.0, of.1) {
                words += 1;
            } else {
                println!(
                    "{label}: brush {:?} flags ours {:#06x}/{:#06x} reference {:#06x}/{:#06x}",
                    String::from_utf8_lossy(&b.name),
                    f.0,
                    f.1,
                    of.0,
                    of.1
                );
            }
            if f.2 == of.2 & 0x00ff_ffff {
                users += 1;
            }
            match (c.raw_pos, c.pos48) {
                (Some(p), _) => assert_eq!(p, b.pos, "{label}: raw brush position"),
                (_, Some(p)) => {
                    assert!(near(p, enc(b.pos)), "{label}: compact brush position");
                    compact += 1;
                    exact += usize::from(p == enc(b.pos));
                }
                _ => panic!("no position"),
            }
        }
        // volumes
        let tv: Vec<&ReferenceCreate> = creates.iter().filter(|c| c.kind == 9).collect();
        let mv: Vec<(u16, &dc_dserver::world::VolumeInfo)> = ours
            .iter()
            .filter_map(|o| match o {
                WorldObject::Volume { obj, volume } => Some((*obj, volume)),
                _ => None,
            })
            .collect();
        assert_eq!(mv.len(), tv.len(), "{label}: the same number of volumes");
        for c in &tv {
            let v = mv
                .iter()
                .find(|v| Some(&v.1.name) == c.name.as_ref())
                .unwrap_or_else(|| panic!("{label}: volume missing"));
            let rec = dc_dserver::worldobj::volume_record(v.0, v.1, &pbox);
            let mut one = Vec::new();
            let mut g = dc_wire::udp::BitWriter::new();
            g.write_u8(8).write_u32(rec.bit_len() as u32).append(&rec);
            world_creates(&g, &mut one);
            let o = &one[0];
            assert_eq!(
                (o.code, o.flags),
                (c.code, c.flags),
                "{label}: volume fields"
            );
            assert!(
                near(o.pos48.unwrap(), c.pos48.unwrap()),
                "{label}: volume position"
            );
            compact += 1;
            exact += usize::from(o.pos48 == c.pos48);
            assert_eq!(o.blob, c.blob, "{label}: volume blob bit for bit");
        }
        // sprites
        let ts: Vec<&ReferenceCreate> = creates.iter().filter(|c| c.kind == 3).collect();
        let ms: Vec<&dc_dserver::world::SpriteInfo> = ours
            .iter()
            .filter_map(|o| match o {
                WorldObject::Sprite { sprite, .. } => Some(sprite),
                _ => None,
            })
            .collect();
        assert_eq!(ms.len(), ts.len(), "{label}: the same number of sprites");
        for c in &ts {
            let s = ms
                .iter()
                .find(|s| near(enc(s.pos), c.pos48.unwrap()))
                .unwrap_or_else(|| panic!("{label}: no sprite at {:?}", c.pos48));
            compact += 1;
            exact += usize::from(Some(enc(s.pos)) == c.pos48);
            let file = &files[&c.file.unwrap()];
            assert!(
                dc_dserver::chars::announce_spelling(&s.file)
                    .as_bytes()
                    .eq_ignore_ascii_case(file),
                "{label}: sprite file"
            );
            assert_eq!(c.flags, Some((0x0081, 0x0002, 1, 0)));
            assert_eq!(c.scale, Some(s.scale), "{label}: sprite scale");
            assert_eq!(
                c.color,
                dc_dserver::worldobj::sprite_color(s),
                "{label}: sprite colour"
            );
            let q = dc_dserver::worldobj::euler_quat(s.rot);
            let t = c.rot.unwrap();
            let dot: f32 = (0..4).map(|i| q[i] * t[i]).sum();
            assert!(dot.abs() > 0.9999, "{label}: sprite rotation {q:?} {t:?}");
        }
        // effects
        let te: Vec<&ReferenceCreate> = creates
            .iter()
            .filter(|c| c.kind == 0 && c.blob.as_ref().is_some_and(|b| b.as_bytes()[0] == 0x64))
            .collect();
        let me: Vec<&dc_dserver::world::EffectInfo> = ours
            .iter()
            .filter_map(|o| match o {
                WorldObject::Effect { effect, .. } => Some(effect),
                _ => None,
            })
            .collect();
        assert_eq!(me.len(), te.len(), "{label}: the same number of effects");
        for c in &te {
            let e = me
                .iter()
                .find(|e| near(enc(e.pos), c.pos48.unwrap()))
                .unwrap_or_else(|| panic!("{label}: no effect at {:?}", c.pos48));
            compact += 1;
            exact += usize::from(Some(enc(e.pos)) == c.pos48);
            assert_eq!(
                c.blob.as_ref(),
                Some(&dc_dserver::worldobj::effect_blob(e)),
                "{label}: effect blob"
            );
            assert_eq!(c.flags, Some((0, 0, 0x0080_0000, 0)));
        }
        // the sky table: the same block, the slot names the same sky model
        let (block, ids) = sky.expect("the reference stack's sky table");
        let (our_block, our_ids) = sky_table(&ours).expect("our sky table");
        assert_eq!(our_block, block, "{label}: sky block bit for bit");
        assert_eq!(our_ids.len(), ids.len());
        for (slot, (a, b)) in our_ids.iter().zip(&ids).enumerate() {
            let tn = creates
                .iter()
                .find(|c| c.obj == *b)
                .and_then(|c| c.name.clone());
            let mn = ours.iter().find_map(|o| match o {
                WorldObject::Brush { obj, brush } if obj == a => Some(brush.name.clone()),
                _ => None,
            });
            assert_eq!(mn, tn, "{label}: sky slot {slot}");
        }
        println!(
            "{label} world {}: brushes {} (same names {}, same flag words {words}, same user words without the \
             surface byte {users}; {doors} reference doors not created), volumes {}, sprites {}, effects {}, sky \
             slots {} equal; compact positions {compact} ({exact} bit for bit, the rest one step apart)",
            &w.sha256[..16],
            theirs.len(),
            names_t.len(),
            tv.len(),
            ts.len(),
            te.len(),
            ids.iter().filter(|i| **i != 0xFFFF).count()
        );
        assert_eq!(words, theirs.len(), "{label}: brush flag words");
        // every record of our set parses with both readers (the synthetic client and dc-botclient's object table)
        let mut names: Vec<String> = ours.iter().flat_map(|o| o.files()).collect();
        names.dedup();
        let id_of = |p: &str| {
            names
                .iter()
                .position(|f| f == p)
                .map_or(0xFFFF, |i| 10 + i as u16)
        };
        let mut body = dc_wire::udp::BitWriter::new();
        for o in &ours {
            let rec = o.record(&pbox, &id_of);
            body.write_u32(rec.bit_len() as u32).append(&rec);
        }
        let mut group = dc_wire::udp::BitWriter::new();
        group.write_u8(8).append(&body);
        let mut sc = SynthClient::new(SynthConfig::named("x"));
        sc.read_group_for_test(&group).expect("synthetic reader");
        assert_eq!(sc.seen.objects.len(), ours.len());
        let mut t = dc_botclient::objects::ObjectTable::default();
        t.apply_group(&body);
        assert!(t.errors.is_empty(), "{label}: {:?}", t.errors);
        checked += 1;
    }
    if checked == 0 {
        println!("skipped: no capture");
    }
}

/// DSV-53: our step-height message is byte for byte the reference stack's on SNOWVALLEY (tdm4, both links) and JUNK_FLEA
/// (tdm6, the real client's link), and like the reference stack's it comes before `S2C_LOAD_WORLD`. The name is found by
/// hash in the user's CShell.dll and in the captures; it is not in the tree.
// OBSERVED: capture 20261025_002236_tdm4, 20261025_065016_tdm6 (REFERENCE)
#[test]
fn rom_step_height_variable_matches_the_reference() {
    use dc_dserver::clientvars::{messages, DEFAULT_STEP_HEIGHT, STEP_HEIGHT_VAR};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let mut checked = 0;
    for (label, folder, conns) in [
        ("tdm4", "20261025_002236_tdm4", &[5i64, 6][..]),
        ("tdm6", "20261025_065016_tdm6", &[6i64][..]),
    ] {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        for &conn in conns {
            let msgs = link_messages(&cap, conn);
            let mut world = None;
            let mut theirs = None;
            let mut before_load = false;
            for m in &msgs {
                let mut r = m.reader();
                match r.read_u8() {
                    0x32 if world.is_none() && r.remaining_bits() >= 48 => {
                        r.read_u16();
                        r.read_u32();
                        world = Some(String::from_utf8_lossy(&r.read_zstr8()).into_owned());
                    }
                    15 => {
                        let n = r.read_zstr8();
                        if n.len() == STEP_HEIGHT_VAR.1 && dc_world::hash16(&n) == STEP_HEIGHT_VAR.0
                        {
                            theirs.get_or_insert_with(|| m.as_bytes().to_vec());
                        }
                    }
                    dc_dserver::msg::S2C_LOAD_WORLD => {
                        before_load = theirs.is_some();
                        break;
                    }
                    _ => {}
                }
            }
            let theirs = theirs
                .unwrap_or_else(|| panic!("{label} conn {conn}: the reference stack's variable"));
            assert!(
                before_load,
                "{label} conn {conn}: sent before the world load"
            );
            let world = world.expect("a file list");
            let w =
                load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
            let ours = messages(&w.client_vars, Some(DEFAULT_STEP_HEIGHT));
            assert_eq!(
                ours.len(),
                1,
                "{label}: the name is in the ROM's CShell.dll"
            );
            assert_eq!(ours[0].as_bytes(), theirs.as_slice(), "{label} conn {conn}");
            checked += 1;
        }
    }
    println!("step height checked on {checked} reference links");
}

/// DSV-56 against the reference stack's world load (captures tdm4 links 6 and 5, tdm2 link 5, tdm1 link 5, menu-frida6 link 3;
/// REFERENCE forms the real client accepted): the tracked-sound records in the reference stack's world-load object groups equal
/// our planned world sounds of the same world field by field (the sound file by name, `sflags`, priority, outer
/// radius, inner ratio, volume; the position within one `pos48` step), the reference stack's `dmask` cache aside, and the sound
/// files have the size the reference stack's file list gave. Every ROM world's sound records read to their end. Skipped
/// without the ROM or the captures.
#[test]
fn rom_world_sounds_match_the_reference_world_load() {
    use dc_dserver::objects::PosBox;
    use dc_dserver::worldobj::{plan_sounds, WorldObject, WorldObjects};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let vfs = dc_rez::RezVfs::new(&rom);
    // every ROM world: the records of its planned sounds decode to their end, the files exist
    if let Some(entries) = world_entries(&rom) {
        let mut total = 0;
        for listed in &entries {
            let w = load_from_rom(&rom, &lobby_arg(listed), NameForm::Listed, None).unwrap();
            let mut n = 2000u16..;
            let sounds = plan_sounds(&w, WorldObjects::All, None, || n.next().unwrap());
            let pbox = PosBox {
                min: w.extent_min,
                max: w.extent_max,
            };
            let mut g = dc_wire::udp::BitWriter::new();
            g.write_u8(8);
            for o in &sounds {
                for f in o.files() {
                    assert!(
                        dc_dserver::chars::resource_size(&vfs, &f).is_some(),
                        "world {}: sound file {} not in the ROM",
                        &w.sha256[..16],
                        dc_dserver::chars::short_hash(&f)
                    );
                }
                let r = o.record(&pbox, &|_| 7);
                g.write_u32(r.bit_len() as u32).append(&r);
            }
            let mut c = SynthClient::new(SynthConfig::named("x"));
            c.read_group_for_test(&g).unwrap();
            assert_eq!(c.seen.sounds.len(), sounds.len());
            total += sounds.len();
        }
        println!("{total} world sounds in {} ROM worlds", entries.len());
        assert!(total > 0);
    }
    let mut checked = 0;
    for (label, folder, conn) in [
        ("tdm4", "20261025_002236_tdm4", 6),
        ("tdm4", "20261025_002236_tdm4", 5),
        ("tdm2", "20261025_001409_tdm2", 5),
        ("tdm1", "20261025_000014_tdm1", 5),
        ("menu-frida6", "20261025_095156_menu-frida6", 3),
    ] {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let msgs = link_messages(&cap, conn);
        let mut files: std::collections::BTreeMap<u16, (u32, Vec<u8>)> = Default::default();
        let mut cap_world = None;
        let mut theirs = None;
        for m in &msgs {
            let mut r = m.reader();
            match r.read_u8() {
                0x32 => {
                    while r.remaining_bits() >= 48 {
                        let id = r.read_u16();
                        let info = r.read_u32();
                        let n = r.read_zstr8();
                        cap_world.get_or_insert_with(|| String::from_utf8_lossy(&n).into_owned());
                        files.insert(id, (info, n));
                    }
                }
                8 if theirs.is_none() => {
                    // the tracked-sound records of the first group that has any, re-framed into a group of their own
                    let mut g = dc_wire::udp::BitWriter::new();
                    g.write_u8(8);
                    let mut n = 0;
                    while r.remaining_bits() >= 32 {
                        let len = r.read_u32() as usize;
                        let rec = r.read_bit_string(len);
                        let mut q = rec.reader();
                        if q.read_u8() == 0 && q.read_u8() == 1 {
                            g.write_u32(len as u32).append(&rec);
                            n += 1;
                        }
                    }
                    if n > 0 {
                        theirs = Some(g);
                    }
                }
                _ => {}
            }
        }
        let cap_world = cap_world.expect("a file list");
        let w =
            load_from_rom(&rom, &cap_world, NameForm::Listed, None).expect("the capture's world");
        let theirs = theirs.expect("the reference stack's world sounds");
        let mut oc = SynthClient::new(SynthConfig::named("reference"));
        oc.read_group_for_test(&theirs).unwrap();
        let trig = tdm_trig(&rom, &cap_world);
        let mut next = 3000u16..;
        let ours = plan_sounds(&w, WorldObjects::World, trig.as_deref(), || {
            next.next().unwrap()
        });
        let pbox = PosBox {
            min: w.extent_min,
            max: w.extent_max,
        };
        let enc = |p: [f32; 3]| {
            let mut b = dc_wire::udp::BitWriter::new();
            dc_dserver::stream::pos48_encode(p, &pbox).write(&mut b);
            let mut r = b.reader();
            [r.read_u16(), r.read_u16(), r.read_u16()]
        };
        assert_eq!(
            ours.len(),
            oc.seen.sounds.len(),
            "{label} link {conn}: the same number of world sounds"
        );
        let mut matched = vec![false; ours.len()];
        for s in oc.seen.sounds.values() {
            assert_eq!((s.tmask, s.tag), (3, 0), "{label}: tmask / tag");
            let (size, name) = &files[&s.sfile];
            let hit = ours.iter().enumerate().find(|(i, o)| {
                let WorldObject::Sound { sound, .. } = o else {
                    return false;
                };
                let p = enc(sound.pos);
                let q = s.pos48.unwrap();
                !matched[*i]
                    && dc_dserver::chars::announce_spelling(&sound.file).as_bytes()
                        == name.to_ascii_uppercase().as_slice()
                    && dc_dserver::worldobj::world_sound_bits(sound) == s.sflags
                    && sound.priority == s.prio
                    && sound.outer as u16 == s.outer
                    && dc_dserver::worldobj::inner_ratio(sound) == s.ratio
                    && (sound.volume != 100) == s.volume.is_some()
                    && s.volume.is_none_or(|v| u32::from(v) == sound.volume)
                    && (0..3).all(|k| p[k].abs_diff(q[k]) <= 1)
            });
            let Some((i, o)) = hit else {
                panic!(
                    "{label} link {conn}: reference sound {} (flags {:#x}) has no equal",
                    s.track, s.sflags
                );
            };
            matched[i] = true;
            let WorldObject::Sound { sound, .. } = o else {
                unreachable!()
            };
            assert_eq!(
                dc_dserver::chars::resource_size(&vfs, &sound.file),
                Some(*size),
                "{label}: the reference stack's file size"
            );
        }
        checked += 1;
        println!(
            "{label} link {conn}: {} world sounds equal the reference stack's",
            ours.len()
        );
    }
    println!("{checked} links checked");
}

/// DSV-270 / DSV-460: the open rules from the ROM worlds' door properties: the one world whose levers switch on doors
/// that are not player-activated (its garage shutters) gets two message-only sliding doors and two use-key levers;
/// Warhead's player-activated B-site doors open by the use key, its other rotating doors only by messages (DSV-500).
#[test]
fn rom_garage_shutters_open_only_by_their_levers() {
    use dc_dserver::doors::OpenBy;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(entries) = world_entries(&rom) else {
        return;
    };
    let (mut garages, mut warhead) = (0, 0);
    for listed in &entries {
        let Ok(w) = load_from_rom(&rom, &lobby_arg(listed), NameForm::Listed, None) else {
            continue;
        };
        let doors: Vec<OpenBy> = w.doors.iter().map(|d| d.open_by).collect();
        let turns: Vec<OpenBy> = w.turns.iter().map(|d| d.open_by).collect();
        if listed.to_ascii_uppercase().contains("RATTLESNAKE") {
            assert_eq!(doors, [OpenBy::Message; 2]);
            assert_eq!(turns, [OpenBy::Use; 2]);
            garages += 1;
        } else if listed.to_ascii_uppercase().contains("WARHEAD") {
            // DSV-460: the B-site sliding doors are player-activated: the use key opens them, nobody near does
            assert_eq!(doors, [OpenBy::Use; 3]);
            // DSV-500: one rotating door is player-activated; the five others (the spawn gate in front of a fall
            // volume, the hatches and their zero-turn boxes) are message doors: created, closed until a script
            let count = |b: OpenBy| turns.iter().filter(|t| **t == b).count();
            assert_eq!(
                (count(OpenBy::Use), count(OpenBy::Message)),
                (1, 5),
                "{turns:?}"
            );
            warhead += 1;
        }
    }
    assert_eq!((garages, warhead), (1, 1));
}

/// DSV-271: with the ROM tables, a player whose `loadout=` names a vest wears that vest's body row (another body model
/// than the default character's) and its armour maximum; a loadout without a vest keeps the default character. Counts
/// and comparisons only.
#[test]
fn rom_the_vest_picks_the_body_row() {
    use dc_dserver::game::{Game, GameTuning};
    use dc_dserver::pipe::{parse_control, Control};
    use dc_dserver::rom_tables::RomTables;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = RomTables::load(&rom);
    let w = load_some(&rom, 1).pop().expect("a world");
    let mut g = Game::new(GameTuning::default(), &w, tables.clone());
    let base = g.character_for(None).unwrap().expect("a ROM character");
    // the first body row after row 0 whose model file differs from row 0's
    let b0 = &tables.chars.bodies[&0].model;
    let (&row, body) = tables
        .chars
        .bodies
        .iter()
        .find(|(n, b)| **n > 0 && b.model != *b0 && b.face_relation.first().is_none_or(|m| *m < 0))
        .expect("another body row");
    let entry = |line: &str| match parse_control(line) {
        Control::User(e) => e,
        other => panic!("{other:?}"),
    };
    let vest = entry(&format!(
        "user 7 3 99 0 120 Dave loadout=0:5,8:{} face=0",
        3000 + row
    ));
    let look = g
        .character_for(Some(&vest))
        .unwrap()
        .expect("a vest character");
    assert_eq!(look.assets.body[0], body.model, "the vest's body row");
    assert_ne!(look.assets.body[0], base.assets.body[0]);
    assert_eq!(look.assets.max_armor, body.max_armor);
    let plain = entry("user 8 4 55 1 0 Erin loadout=0:5 face=0");
    let look = g.character_for(Some(&plain)).unwrap().expect("a character");
    assert_eq!(look.assets.body[0], base.assets.body[0]);
}

/// CLI-1340 (user's ROM): every world with bomb sites names, in each site's planted command, world effects to switch:
/// at least one switched on (an effect that is off at the start: the red indicator) and one switched off (the green
/// ones). Counts only.
#[test]
fn rom_bomb_sites_switch_their_indicator_effects_at_the_plant() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(entries) = world_entries(&rom) else {
        println!("skipped: world archive unavailable");
        return;
    };
    let mut worlds = 0;
    for listed in &entries {
        let Ok(w) = load_from_rom(&rom, &lobby_arg(listed), NameForm::Listed, None) else {
            continue;
        };
        let snd = dc_dserver::game::bomb::SndState::from_world(&w);
        if snd.sites.is_empty() {
            continue;
        }
        worlds += 1;
        for (number, fx) in &snd.plant_fx {
            let on: Vec<&Vec<u8>> = fx.iter().filter(|f| f.1).map(|f| &f.0).collect();
            assert!(
                !on.is_empty() && fx.iter().any(|f| !f.1),
                "world {} site {number}: {} on, {} off",
                &w.sha256[..16],
                on.len(),
                fx.len() - on.len()
            );
            for name in on {
                let e = w
                    .effects
                    .iter()
                    .find(|e| e.name.eq_ignore_ascii_case(name))
                    .unwrap();
                assert!(
                    !e.start_on,
                    "world {}: the switched-on effect starts off",
                    &w.sha256[..16]
                );
            }
        }
    }
    println!("{worlds} world(s) with bomb sites");
    assert!(worlds > 0);
}

/// DSV-500 / DSV-501 on WarHead (playtest report: a fence piece missing at a spawn area, and walking through the gap
/// kills): the gap is a rotating door the players do not open, standing in front of a fall volume that the
/// quarantine mode's start trigger switches off when it opens the door. In a TDM room the door is created and stays
/// closed (a wall, no use prompt); in a quarantine room it is open from the start. Counts and positions only.
#[test]
fn rom_warhead_spawn_gate_stands_closed_in_tdm_and_open_in_quarantine() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(entries) = world_entries(&rom) else {
        return;
    };
    let Some(listed) = entries
        .iter()
        .find(|l| l.to_ascii_uppercase().contains("WARHEAD"))
    else {
        println!("skipped: no WarHead");
        return;
    };
    let arg = lobby_arg(listed);
    type Gate = (Vec<u8>, f32, Option<(bool, u8)>, bool);
    let gate_of = |key: &str| -> Option<Gate> {
        let trig = room_trig(&rom, listed, key)?;
        let w = load_from_rom(&rom, &arg, NameForm::Listed, None).expect("WarHead");
        let kills = w.kill_shapes.clone();
        let mut server: Server<u32> = Server::new(ServerTuning::default(), w, 0);
        assert!(server.handle_line(0, &format!("room key={key} type=1 goal=9 trig={trig}")));
        server.handle_line(0, "user 7 3 99 0 120 Dave");
        let mut client = SynthClient::new(SynthConfig::named("Dave"));
        let (mut up, mut down) = (
            SimChannel::new(1, Faults::clean(2)),
            SimChannel::new(2, Faults::clean(2)),
        );
        let mut now = 0u32;
        while now < 20_000 && client.state() != ClientState::InWorld {
            for d in client.poll(now) {
                up.send(now, d);
            }
            for d in up.deliver(now) {
                server.handle_datagram(now, 1, &d);
            }
            for o in server.poll(now) {
                if let Output::Transmit { bytes, .. } = o {
                    down.send(now, bytes);
                }
            }
            for d in down.deliver(now) {
                client.handle_datagram(now, &d);
            }
            now += 5;
        }
        assert_eq!(client.state(), ClientState::InWorld);
        // the message door with a kill volume right behind it: 40 units out from the middle of its box across its
        // thin side, one side lies in a kill volume and the other does not
        let fronts_a_fall = |t: &dc_dserver::doors::TurnDoor| {
            let (lo, hi) = (t.info.lo, t.info.hi);
            let mid = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
            let thin = if hi[0] - lo[0] < hi[2] - lo[2] { 0 } else { 2 };
            let side = |d: f32| {
                let mut p = mid;
                p[thin] += d;
                kills.iter().any(|k| k.contains(p))
            };
            side(40.0) != side(-40.0)
        };
        let t = server
            .turns()
            .iter()
            .find(|t| t.info.open_by == dc_dserver::doors::OpenBy::Message && fronts_a_fall(t))?;
        let created = client.seen.objects.get(&t.obj).is_some_and(|o| o.kind == 2);
        Some((t.info.name.clone(), t.frac, t.activate(), created))
    };
    let Some((tdm_name, frac, entry, created)) = gate_of("TDM") else {
        panic!("no message door in front of a kill volume");
    };
    assert_eq!(frac, 0.0, "closed in TDM");
    assert_eq!(entry.map(|e| e.0), Some(true), "no use prompt");
    assert!(created, "the client got the door's brush");
    if let Some((qrt_name, frac, _, _)) = gate_of("QRT") {
        assert_eq!(qrt_name, tdm_name);
        assert_eq!(frac, 1.0, "open from the start in quarantine");
    }
}

/// BOT-110 on the ROM worlds of the playtest report (WarHead: bots floated over the sewer; Two Towers: bots walked
/// on the water): with the objects our server creates in a TDM room, every linked world model the server leaves
/// non-solid (or does not create) is out of the bot's collision set: a ray straight down through the middle of
/// such a model's top meets it in the file's set and passes it in the bot's. Counts only.
#[test]
fn rom_bots_fall_through_what_the_players_fall_through() {
    use dc_botclient::objects::{kind, ClientObject, ObjectTable};
    use dc_dserver::worldobj::{brush_flags, plan_scene, WorldObject, WorldObjects, ZoneClass};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(entries) = world_entries(&rom) else {
        return;
    };
    let Ok(worlds) = dc_botclient::romworld::RomWorlds::open(&rom) else {
        return;
    };
    let mut checked = 0;
    for listed in entries.iter().filter(|l| {
        let u = l.to_ascii_uppercase();
        u.contains("WARHEAD") || u.contains("TWOTOWERS")
    }) {
        let Some(trig) = tdm_trig(&rom, listed) else {
            continue;
        };
        let w = load_from_rom(&rom, &lobby_arg(listed), NameForm::Listed, None).expect("world");
        let mut n = 0u16;
        let objs = plan_scene(
            &w,
            &dc_dserver::rom_tables::RomTables::default(),
            WorldObjects::World,
            ZoneClass::Own,
            1,
            Some(&trig),
            || {
                n += 1;
                n
            },
        );
        let mut t = ObjectTable::default();
        for (i, o) in objs.iter().enumerate() {
            if let WorldObject::Brush { brush, .. } = o {
                let mut c = ClientObject::default();
                c.kind = kind::BRUSH;
                c.name = Some(brush.name.clone());
                c.flags = brush_flags(brush).0;
                t.objects.insert(i as u16 + 1, c);
            }
        }
        let mut bot = worlds.load(listed).expect("bot world");
        let file = bot.collision.clone();
        bot.follow_server(&t)
            .expect("the server's objects change the set");
        let solid = bot.solid.clone().unwrap();
        let (mut dropped, mut kept) = (0, 0);
        for (l, s) in bot.linked.iter().zip(&solid) {
            if *s {
                kept += 1;
                continue;
            }
            // the top middle of the model's file geometry
            let m = &bot.full.models[l.index];
            let (lo, hi) = m.bounds();
            if lo.x > hi.x {
                continue;
            }
            let mid = [0.5 * (lo.x + hi.x), hi.y, 0.5 * (lo.z + hi.z)];
            let a = dc_sim::V3::new(mid[0], mid[1] + 1.0, mid[2]);
            let b = dc_sim::V3::new(mid[0], mid[1] - 1.0, mid[2]);
            assert_eq!(
                bot.collision.models[l.index].face_count(),
                0,
                "{listed}: out of the bots' set"
            );
            if file.ray(a, b).is_some() && bot.collision.ray(a, b).is_none() {
                dropped += 1;
            }
        }
        println!("{listed}: {dropped} non-solid floors the bots no longer stand on, {kept} solid models kept");
        assert!(dropped > 0 && kept > 0);
        checked += 1;
    }
    assert!(checked > 0 || world_entries(&rom).is_none());
}
