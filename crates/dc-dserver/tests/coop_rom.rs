//! ROM-gated (skipped when CA_ROM is unset): co-op rooms on the co-op worlds of the user's ROM. The world named by
//! each co-op GMS loads with its spawners, AI templates and AI nodes (classes by hash); a class-5 room on it finds
//! that GMS, and a synthetic client sees the first AIs created at spawner positions (the spawn table's spawners
//! where the GMS has spawn tables), each on a body row that shares the player rig, with the NPC tail and the weapon
//! at socket 2. Output is counts and hashes only; no ROM value is asserted.

use dc_bute::GameRoomData;
use dc_dserver::coop_rom::{self, AiTables, CoopPlan};
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig};
use dc_dserver::world::{load_from_rom, NameForm};
use dc_net_engine::sim::{Faults, SimChannel};
use dc_rez::RezVfs;

/// `(mode key, Worlds\<name>)` of every map row with a GMS for the class-5 mode row.
fn coop_worlds(vfs: &RezVfs) -> Vec<(String, String)> {
    let Ok(gr) = GameRoomData::from_vfs(vfs) else {
        return Vec::new();
    };
    let modes = gr.modes(false);
    let Some((mi, key)) = modes.iter().enumerate().find_map(|(i, m)| {
        let t = m.get("GameType")?.trim().trim_matches('"').to_string();
        let k = m.get("Key")?.trim().trim_matches('"').to_string();
        (t == "5").then_some((i, k))
    }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, m) in gr.maps().iter().enumerate() {
        if !m.contains_key(&format!("GMS_{key}")) {
            continue;
        }
        let (_md, doc) = dc_bute::gms_file_with_vfs(vfs, &gr, i, mi, false);
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
        out.push((key.clone(), format!("Worlds\\{base}")));
    }
    out
}

fn short(s: &str) -> String {
    dc_rez::dc_rom::sha256_hex(s.to_ascii_uppercase().as_bytes())[..12].to_string()
}

#[test]
fn rom_coop_worlds_spawn_ai_from_their_spawners() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let vfs = RezVfs::new(&rom);
    let worlds = coop_worlds(&vfs);
    assert!(!worlds.is_empty(), "the ROM room table has co-op maps");
    // the AI rows that share the player rig (FIRETEAM_2009.md §1.8: 9 of the 12)
    let tables = AiTables::from_vfs(&vfs);
    let mut cache = Default::default();
    let player = {
        let t = RomTables::load(&rom).0;
        let m = t.chars.bodies[&0].model.clone();
        dc_dserver::chars::read_rig(&vfs, &m, &mut cache).expect("player rig")
    };
    let mut eligible = Vec::new();
    for r in tables.bodies.values() {
        let Ok(rig) = dc_dserver::chars::read_rig(&vfs, &r.model, &mut cache) else {
            continue;
        };
        let sock = coop_rom::weapon_socket_present(&vfs, &r.model, rig.grip_socket);
        if coop_rom::shares_player_rig(&rig, &player, sock) {
            eligible.push(dc_dserver::chars::announce_spelling(&r.model));
        }
    }
    println!(
        "AI body rows: {} of {} share the player rig",
        eligible.len(),
        tables.bodies.len()
    );
    assert_eq!(eligible.len(), 9, "FIRETEAM_2009.md §1.8 count");
    for (key, arg) in worlds {
        let w = load_from_rom(&rom, &arg, NameForm::Listed, None)
            .unwrap_or_else(|e| panic!("co-op world {}: {e}", short(&arg)));
        let c = w.coop.clone();
        println!(
            "co-op world {}: {} spawners ({} with a target), {} AI characters ({} templates), {} AI nodes",
            short(&arg),
            c.spawners.len(),
            c.spawners.iter().filter(|s| s.target.is_some()).count(),
            c.templates.len(),
            c.templates.iter().filter(|t| t.template).count(),
            c.nodes.len()
        );
        assert!(!c.spawners.is_empty() && !c.templates.is_empty());
        let doc = coop_rom::find_gms(&vfs, &key, &w.announce).expect("the GMS naming this world");
        let plan = CoopPlan::from_gms(&doc, 1);
        let table_spawners: Vec<[f32; 3]> = plan
            .rounds
            .first()
            .map(|r| {
                r.spawners
                    .iter()
                    .filter_map(|n| {
                        c.spawners
                            .iter()
                            .find(|s| s.name.eq_ignore_ascii_case(n.as_bytes()))
                            .map(|s| s.pos)
                    })
                    .collect()
            })
            .unwrap_or_default();
        println!(
            "  GMS: {} spawn table rows, first row names {} spawners, {} health rates, lives {:?}",
            plan.rounds.len(),
            table_spawners.len(),
            plan.health_rates.len(),
            plan.difficulty.lives.map(|_| "set")
        );
        let allowed: Vec<[f32; 3]> = if table_spawners.is_empty() {
            // a mission world (DSV-571): its statements' spawners and the AI it places live
            c.spawners
                .iter()
                .map(|s| s.pos)
                .chain(c.templates.iter().filter(|t| !t.template).map(|t| t.pos))
                .collect()
        } else {
            table_spawners
        };
        let mut cfg = ServerTuning::default();
        let cap = cfg.game.coop.speed;
        cfg.game.coop.first_wave_ms = 200;
        cfg.game.coop.spawn_interval_ms = 50;
        cfg.game.coop.max_alive = 4;
        let (tables, _) = RomTables::load(&rom);
        let bounds = (w.extent_min, w.extent_max);
        let mut server: Server<u32> = Server::with_tables(cfg, w, tables, 0);
        assert!(server.handle_line(0, &format!("room key={key} type=5 goal=1 ff=0 time=600")));
        assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
        let mut sc = SynthConfig::named("Dave");
        sc.box_bounds = Some(bounds);
        let mut client = SynthClient::new(sc);
        let (mut up, mut down) = (
            SimChannel::new(1, Faults::clean(2)),
            SimChannel::new(2, Faults::clean(2)),
        );
        let mut logs = Vec::new();
        let mut now = 0u32;
        let created = |s: &Server<u32>| {
            s.game()
                .ai_chars()
                .iter()
                .filter(|a| a.obj.is_some())
                .count()
        };
        while now < 30_000 && created(&server) < 2 {
            for d in client.poll(now) {
                up.send(now, d);
            }
            for d in up.deliver(now) {
                server.handle_datagram(now, 1, &d);
            }
            for o in server.poll(now) {
                match o {
                    Output::Transmit { bytes, .. } => down.send(now, bytes),
                    Output::Log(l) => logs.push(l),
                    Output::Pipe(_) => {}
                }
            }
            for d in down.deliver(now) {
                client.handle_datagram(now, &d);
            }
            now += 10;
        }
        // let the create records arrive
        for _ in 0..50 {
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
            now += 10;
        }
        assert_eq!(client.state(), ClientState::InWorld);
        assert!(client.seen.errors.is_empty(), "{:?}", client.seen.errors);
        assert!(
            logs.iter()
                .any(|l| l.contains("co-op room") && l.contains("the world's co-op GMS")),
            "{:?}",
            logs.iter()
                .filter(|l| l.contains("co-op"))
                .collect::<Vec<_>>()
        );
        let ais: Vec<_> = server
            .game()
            .ai_chars()
            .iter()
            .filter(|a| a.obj.is_some())
            .cloned()
            .collect();
        assert!(ais.len() >= 2, "AIs created: {}", ais.len());
        let mut armed = 0;
        for a in &ais {
            let obj = a.obj.unwrap();
            let o = &client.seen.objects[&obj];
            assert_eq!((o.kind, o.blob_pid), (1, Some(0xff)));
            let created_at = o.pos.expect("create position");
            let near = allowed
                .iter()
                .any(|s| (s[0] - created_at[0]).abs() < 1.0 && (s[2] - created_at[2]).abs() < 1.0);
            let best = allowed
                .iter()
                .map(|s| ((s[0] - created_at[0]).powi(2) + (s[2] - created_at[2]).powi(2)).sqrt())
                .fold(f32::MAX, f32::min);
            assert!(near, "AI slot {} created at a spawner (nearest {best:.1} units; server pos {:?}, client {:?})", a.slot, a.pos, created_at);
            let body = client.seen.files[&o.models[0]].1.clone();
            let body = String::from_utf8_lossy(&body).to_string();
            assert!(
                eligible.contains(&body),
                "AI body {} shares the player rig",
                short(&body)
            );
            // DSV-589: a melee AI (the round world's infected) holds no weapon object; the others hold theirs at
            // socket 2 and walk at `coop.speed` (DSV-588: their attribute rows are faster)
            // DSV-590: an AI whose template's hand slot is empty (the mission world's placed live AI) holds nothing
            if a.spec.melee.is_some() {
                assert!(o.attached.is_empty(), "a melee AI holds no weapon object");
                assert!(a.spec.speed < cap, "a melee AI walks slower than the cap");
            } else if a.spec.unarmed {
                assert!(
                    o.attached.is_empty(),
                    "an unarmed AI holds no weapon object"
                );
            } else {
                assert_eq!(
                    o.attached.first().map(|x| x.1),
                    Some(2),
                    "weapon at socket 2"
                );
                assert_eq!(a.spec.speed, cap, "an armed AI walks at coop.speed");
                armed += 1;
            }
            let mut r = o.blob.as_ref().unwrap().reader();
            r.read_u8();
            assert_eq!((r.read_bit(), r.read_bit()), (false, true));
            r.skip_bits(48 + 64 + 192 + 192 + 64 + 9 + 1);
            let (_rank, team, kind) = (r.read_u8(), r.read_u8(), r.read_u8());
            assert_eq!(team, 2);
            assert!(kind == 1 || kind == 2);
            r.read_u32();
            assert!(!r.read_zstr8().is_empty(), "a caption (the bar needs one)");
            assert!(matches!(
                a.state,
                dc_dserver::game::coop::AiState::Alive
                    | dc_dserver::game::coop::AiState::Dead { .. }
            ));
        }
        // the mission world's soldiers keep their guns (DSV-589 leaves an AI with a ranged brain armed)
        let unarmed = ais.iter().filter(|a| a.spec.unarmed).count();
        if c.mission && c.rounds.is_none() {
            assert_eq!(
                armed + unarmed,
                ais.len(),
                "a mission-world AI holds its weapon or none"
            );
        }
        println!(
            "  {} AI created by {} ms, bodies {:?}",
            ais.len(),
            now,
            ais.iter()
                .map(|a| short(&a.spec.look.assets.body[0]))
                .collect::<Vec<_>>()
        );
    }
}

/// DSV-591: every brain row a co-op world's AI templates name has drop rows whose item words are packs we know
/// (matched by hash), each row's chance in (0, 100] and the rows' sum at most 100; the three pack models resolve
/// from the ROM with a texture each. Counts only; no ROM value is asserted beyond those bounds.
#[test]
fn rom_ai_brains_drop_known_packs_with_rom_models() {
    use dc_dserver::game::packs::{pack_kind, pack_looks_from_rom, PackKind};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let vfs = RezVfs::new(&rom);
    let tables = AiTables::from_vfs(&vfs);
    let mut kinds = std::collections::BTreeSet::new();
    let mut brains = 0;
    for (_key, w) in coop_worlds(&vfs) {
        let world = load_from_rom(&rom, &w, NameForm::Argument, None).expect("co-op world");
        for t in &world.coop.templates {
            let Some(b) = t.brain.as_deref().and_then(|b| tables.brain_named(b)) else {
                continue;
            };
            brains += 1;
            assert!(
                !b.drops.is_empty(),
                "brain {} has drop rows",
                short(&b.name)
            );
            let mut sum = 0.0;
            for (word, chance) in &b.drops {
                let k = pack_kind(word);
                assert!(
                    k.is_some(),
                    "brain {}: an unknown item word",
                    short(&b.name)
                );
                kinds.insert(k.unwrap());
                assert!(*chance > 0.0 && *chance <= 100.0);
                sum += chance;
            }
            assert!(sum <= 100.0, "brain {}: rows sum {sum}", short(&b.name));
        }
    }
    assert!(brains > 0, "the templates name brain rows");
    println!("brains named by templates: {brains}, pack kinds: {kinds:?}");
    assert_eq!(kinds.len(), 4, "every pack kind is dropped by some brain");
    let looks = pack_looks_from_rom(&vfs);
    for k in [PackKind::Health, PackKind::Ammo, PackKind::HeavyAmmo] {
        let l = looks.of(k).unwrap_or_else(|| panic!("{k:?} model"));
        assert_eq!(l.textures.len(), 1, "{k:?} skin");
    }
}
