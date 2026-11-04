//! ROM- and capture-gated (skipped without CA_ROM or the captures): the world-object rules of world-objects.
//!
//! 1. Every breakable and explosion object of the 21 ROM worlds parses into the destructible data; every explosion a
//!    death command names is an explosion object of the same world, and most effects it names are placed effects.
//! 2. The 2009 client draws only the world models the server creates (engine+0xa2190: a kind-2 create binds to the
//!    world model of its name; the client's only own pass over the world file's objects is the light scan,
//!    engine+0xcea90): on the real client's links of the JUNK_FLEA captures (the reference stack's tdm6 and our own stack's
//!    sbs4-real, both TDM rooms) the kind-2 creates are exactly our TDM plan, while the world file holds linked models
//!    that the file itself marks visible and solid but no TDM start script leaves in the set; the quarantine room's
//!    start scripts swap them. A client that draws the file's models by their own properties shows them (the user's
//!    "extra boxes"). Names never appear in the output: counts and hashes only.
// SPEC: FMT_DAT85.md §6 (object list, read by the server engine+0x93fd0 and scanned by the client for lights only,
// engine+0xcea90), §6.4; UDP_SESSION_2009_B.md §3.3, §3.5.1; RENDER_2009.md §5.1 step 6
// DISASM: engine+0xa2190 (kind-2 create: binds the object to the world model of its name, else the error 0x17 with
// the engine's missing-world-model text)

use dc_dserver::destructibles::DeathAction;
use dc_dserver::world::{load_from_rom, NameForm, WORLD_ARCHIVE, WORLD_EXT};

fn world_entries(rom: &std::path::Path) -> Vec<String> {
    let Ok(arch) = dc_rez::RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE)) else {
        return Vec::new();
    };
    arch.entries()
        .iter()
        .map(|e| e.0.clone())
        .filter(|n| n.to_ascii_uppercase().ends_with(WORLD_EXT))
        .collect()
}

#[test]
fn rom_breakables_and_explosions_parse_and_name_their_own_world() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let mut worlds = 0;
    let (mut breakables, mut explosions, mut actions, mut effects) = (0, 0, 0, 0);
    for listed in world_entries(&rom) {
        let base = listed.rsplit('\\').next().unwrap_or(&listed);
        let arg = format!("Worlds\\{}", &base[..base.len() - WORLD_EXT.len()]);
        let w = load_from_rom(&rom, &arg, NameForm::Listed, None).expect("world");
        let d = &w.destructibles;
        for b in &d.breakables {
            assert!(b.hit_points > 0.0);
            assert!(
                w.brushes
                    .iter()
                    .any(|x| x.name.eq_ignore_ascii_case(&b.name)
                        && x.class == dc_dserver::world::BrushClass::Breakable),
                "a breakable is a breakable brush"
            );
            for a in &b.death {
                match a {
                    // most name a placed effect; a few switch on other objects (the session skips those)
                    DeathAction::Effect(n) => {
                        if w.effects.iter().any(|e| e.name.eq_ignore_ascii_case(n)) {
                            effects += 1;
                        }
                    }
                    DeathAction::Explode(i) => assert!(*i < d.explosions.len()),
                }
                actions += 1;
            }
        }
        for e in &d.explosions {
            assert!(e.damage >= 0.0 && e.radius >= 0.0);
        }
        breakables += d.breakables.len();
        explosions += d.explosions.len();
        worlds += 1;
    }
    println!(
        "{worlds} worlds: {breakables} breakables, {explosions} explosion objects, {actions} death actions ({effects} name a placed effect)"
    );
    assert!(worlds > 0 && breakables > 0 && explosions > 0);
    assert!(
        effects * 10 >= actions * 5,
        "most death actions name a placed effect or an explosion"
    );
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

/// The world announced in the link's file list and the names of the kind-2 creates of its object groups.
fn brush_creates(msgs: &[dc_wire::udp::BitWriter]) -> (Option<String>, Vec<Vec<u8>>) {
    let mut world = None;
    let mut names = Vec::new();
    for m in msgs {
        let mut r = m.reader();
        match r.read_u8() {
            0x32 => {
                while r.remaining_bits() >= 48 {
                    r.read_u16();
                    r.read_u32();
                    let n = r.read_zstr8();
                    world.get_or_insert_with(|| String::from_utf8_lossy(&n).into_owned());
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
                    if kb & 0x3f != 2 {
                        continue;
                    }
                    q.skip_bits(96);
                    if kb & 0x40 != 0 {
                        let n = if kb & 0x80 != 0 {
                            q.read_u16() as usize
                        } else {
                            q.read_u8() as usize
                        };
                        q.skip_bits(n);
                    }
                    names.push(q.read_zstr8().to_ascii_lowercase());
                }
            }
            _ => {}
        }
    }
    (world, names)
}

/// The room's `trig` setting for this world in a room of the mode with this key, as our lobby writes it.
fn trig_for(rom: &std::path::Path, world: &str, key: &str) -> Option<String> {
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

fn plan_names(
    w: &dc_dserver::world::WorldInfo,
    tables: &dc_dserver::rom_tables::RomTables,
    trig: Option<&str>,
) -> std::collections::BTreeSet<Vec<u8>> {
    use dc_dserver::worldobj::{plan_scene, WorldObject, WorldObjects, ZoneClass};
    let mut next = 1000u16..;
    plan_scene(
        w,
        tables,
        WorldObjects::World,
        ZoneClass::Room,
        1,
        trig,
        || next.next().unwrap(),
    )
    .into_iter()
    .filter_map(|o| match o {
        WorldObject::Brush { brush, .. } => Some(brush.name.to_ascii_lowercase()),
        _ => None,
    })
    .collect()
}

#[test]
fn rom_capture_the_2009_client_got_exactly_the_server_planned_models() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    let mut checked = 0;
    for (label, folder, conns) in [
        ("tdm6 (reference)", "20261025_065016_tdm6", &[6i64][..]),
        (
            "sbs4-real (our stack)",
            "20261025_202429_sbs4-real",
            &[5i64, 6][..],
        ),
    ] {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        for &conn in conns {
            let (world, names) = brush_creates(&link_messages(&cap, conn));
            let Some(world) = world else {
                continue;
            };
            let w =
                load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
            let created: std::collections::BTreeSet<Vec<u8>> = names.into_iter().collect();
            let tdm = trig_for(&rom, &world, "TDM");
            let ours = plan_names(&w, &tables, tdm.as_deref());
            assert_eq!(
                created, ours,
                "{label} conn {conn}: the kind-2 creates are the TDM plan"
            );
            // linked models the file marks visible and solid that no TDM start script keeps: a client drawing the
            // file by its own properties shows them, the 2009 client never got them
            let extra: Vec<&dc_dserver::world::BrushInfo> = w
                .brushes
                .iter()
                .filter(|b| b.visible && b.solid && b.class != dc_dserver::world::BrushClass::Sky)
                .filter(|b| !created.contains(&b.name.to_ascii_lowercase()))
                .collect();
            println!(
                "{label} conn {conn}: world {}, {} models created, {} visible solid file models not created",
                dc_rez::dc_rom::sha256_hex(world.as_bytes()),
                created.len(),
                extra.len()
            );
            assert!(
                !extra.is_empty(),
                "{label}: the world holds models the client never got"
            );
            // the quarantine room of the same world keeps some of them and drops others
            if let Some(qrt) = trig_for(&rom, &world, "QRT") {
                let q = plan_names(&w, &tables, Some(&qrt));
                let only_q = q.difference(&ours).count();
                let only_t = ours.difference(&q).count();
                println!("{label}: quarantine plan +{only_q} / -{only_t} models against TDM");
                assert!(
                    only_q > 0 && only_t > 0,
                    "the quarantine scripts swap models"
                );
                assert!(extra
                    .iter()
                    .any(|b| q.contains(&b.name.to_ascii_lowercase())));
            }
            // the elimination room (one life per round) runs the TDM start scripts
            if let Some(elim) = trig_for(&rom, &world, "TSV") {
                assert_eq!(plan_names(&w, &tables, Some(&elim)), ours);
            }
            checked += 1;
        }
    }
    println!("checked {checked} link(s)");
}

/// DSV-230: every world with Spy Hunt objects has at least five intel places and one uplink box with a positive time
/// (the seven Spy Hunt worlds of the room table among them); the Spy Hunt GMS rows give five intel, a case model and
/// three agent weapon rows of the weapon table.
#[test]
fn rom_spy_hunt_worlds_have_intel_points_an_uplink_and_gms_rows() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let mut with = 0;
    for listed in world_entries(&rom) {
        let base = listed.rsplit('\\').next().unwrap_or(&listed);
        let arg = format!("Worlds\\{}", &base[..base.len() - WORLD_EXT.len()]);
        let w = load_from_rom(&rom, &arg, NameForm::Listed, None).expect("world");
        if w.spy.intel_points.len() < 5 {
            continue;
        }
        with += 1;
        assert!(!w.spy.uplinks.is_empty());
        for u in &w.spy.uplinks {
            assert!(u.secs.is_some_and(|s| s > 0.0));
            assert!(u.dims.iter().all(|d| *d > 0.0));
            // DSV-323: the station scripts (locked at the start, work / release / activate commands)
            assert!(u.locked, "{arg}");
            assert!(
                !u.work.is_empty() && !u.release.is_empty() && !u.activate.is_empty(),
                "{arg}"
            );
            let act = dc_dserver::game::station::parse(&u.activate);
            assert!(
                act.iter()
                    .any(|(_, s)| *s == dc_dserver::game::station::Stmt::SideWin),
                "{arg}: {act:?}"
            );
        }
        // the GMS agent trigger is one of the world's triggers and opens a box after its delay
        let opens = w.spy.triggers.iter().any(|(_, cmds)| {
            cmds.iter().any(|c| {
                dc_dserver::game::station::parse(c).iter().any(|(d, s)| {
                    *d > 0.0 && matches!(s, dc_dserver::game::station::Stmt::Unlock(_))
                })
            })
        });
        assert!(opens, "{arg}: a trigger unlocks the box later");
    }
    assert!(with >= 7, "{with} worlds with Spy Hunt places");
    let vfs = dc_rez::RezVfs::new(&rom);
    let gr = dc_bute::GameRoomData::from_vfs(&vfs).expect("room table");
    let key = gr
        .modes(false)
        .iter()
        .find(|m| m.get("GameType").map(|v| v.trim()) == Some("9"))
        .and_then(|m| m.get("Key").cloned())
        .expect("a class-9 row");
    let doc = dc_dserver::coop_rom::find_gms(&vfs, &key, "Worlds\\WARHEAD.dat").expect("GMS");
    let g = dc_dserver::game::spy::SpyGms::from_gms(&doc);
    assert_eq!(g.total, 5);
    assert!(g.intel_model.is_some() && !g.intel_skins.is_empty());
    assert_eq!(g.agent_weapons.len(), 3);
    assert_eq!(g.stage2_secs, 180);
    let w = load_from_rom(&rom, r"Worlds\WARHEAD", NameForm::Listed, None).expect("world");
    let trig = g.agent_trigger.clone().expect("AgentTrigger");
    assert!(
        w.spy
            .triggers
            .iter()
            .any(|(t, _)| t.eq_ignore_ascii_case(trig.as_bytes())),
        "the agent trigger is a world trigger"
    );
    assert!(g.stage2_title.is_some() && g.hunter_comment.is_some() && g.bounty_comment.is_some());
    let t = dc_dserver::rom_tables::WeaponTable::from_vfs(&vfs).expect("weapons");
    for w in &g.agent_weapons {
        assert!(t.weapons.contains_key(w), "agent weapon row {w}");
    }
}

/// DSV-240: every Search and Destroy GMS row (class 2) names the attackers' bomb item, a weapon row of the weapon
/// table with a hand model, in the last order class of the weapon order (the mode item class).
#[test]
fn rom_search_and_destroy_gms_rows_name_the_bomb_item() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let vfs = dc_rez::RezVfs::new(&rom);
    let gr = dc_bute::GameRoomData::from_vfs(&vfs).expect("room table");
    let key = gr
        .modes(false)
        .iter()
        .find(|m| m.get("GameType").map(|v| v.trim()) == Some("2"))
        .and_then(|m| m.get("Key").cloned())
        .expect("a class-2 row");
    let t = dc_dserver::rom_tables::WeaponTable::from_vfs(&vfs).expect("weapons");
    let mut found = 0;
    for world in [r"Worlds\SnowValley.dat", r"Worlds\WARHEAD.dat"] {
        let doc = dc_dserver::coop_rom::find_gms(&vfs, &key, world).expect("GMS");
        let item = dc_dserver::game::bomb::bomb_item_of(&doc, &t).expect("bomb item row");
        assert!(
            t.hand_model(item).is_some(),
            "the item row {item} has a hand model"
        );
        let last = t.class_of(t.commands.last().copied().expect("command table"));
        assert_eq!(t.class_of(item), last, "the item is in the mode item class");
        found += 1;
    }
    assert_eq!(found, 2);
}

/// DSV-241: a Search and Destroy room spawns every team on its own S&D spawn set (the points whose mode filter is the
/// pinned S&D set), never on the TDM or CTF points of the same team spread over the map.
#[test]
fn rom_search_and_destroy_spawns_use_the_mode_set() {
    use dc_dserver::game::bomb::SND_SPAWN_FILTER;
    use dc_dserver::game::{Game, GameTuning};
    use dc_dserver::rom_tables::RomTables;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = RomTables::load(&rom);
    let w = load_from_rom(&rom, r"Worlds\SnowValley", NameForm::Listed, None).expect("world");
    let set = |i: usize| {
        w.spawn_modes[i].as_deref().is_some_and(|m| {
            dc_dserver::chars::name_key(m.trim_ascii().to_ascii_lowercase().as_slice())
                == SND_SPAWN_FILTER
        })
    };
    let team_of = |i: usize| tables.teams.team_of(w.spawns[i].team_filter.as_deref());
    assert!((0..w.spawns.len()).any(set), "the world has the S&D set");
    assert!(
        (0..w.spawns.len()).any(|i| !set(i) && team_of(i) == Some(1)),
        "and team points of other modes"
    );
    let mut g = Game::new(GameTuning::default(), &w, tables.clone());
    let dc_dserver::pipe::Control::Room(room) =
        dc_dserver::pipe::parse_control("room key=TMM type=2 goal=5")
    else {
        panic!("room line");
    };
    g.set_room(&room);
    for team in [1u8, 2] {
        for _ in 0..300 {
            let (pos, _) = g.choose_spawn(team);
            let i = w
                .spawns
                .iter()
                .position(|s| s.pos == pos)
                .expect("a spawn point");
            assert!(set(i) && team_of(i) == Some(team), "team {team}: point {i}");
        }
    }
}

/// DSV-470 (project decision): Elimination Pro (class 12) spawns on the Search and Destroy team set on the six
/// worlds that have it (team 1 on the S&D team-1 points, team 2 on the team-2 points), and on the team deathmatch set
/// of a world without it. No ROM spawn point names Elimination Pro itself.
#[test]
fn rom_elimination_pro_spawns_use_the_search_and_destroy_set() {
    use dc_dserver::game::bomb::{ELIM_FALLBACK_SET_KEY, SND_SPAWN_FILTER};
    use dc_dserver::game::{Game, GameTuning};
    use dc_dserver::rom_tables::RomTables;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = RomTables::load(&rom);
    let initials = |m: &[u8]| -> String {
        m.iter()
            .filter(|c| c.is_ascii_uppercase())
            .map(|&c| c as char)
            .collect()
    };
    for (world, snd) in [
        ("SandHog", true),
        ("SnowValley", true),
        ("GrayHammer", true),
        ("Warhead", true),
        ("Rattlesnake", true),
        ("RapidStream", true),
        ("PumpJack", false),
        ("Gravedigger", false),
    ] {
        let w =
            load_from_rom(&rom, &format!("Worlds/{world}"), NameForm::Listed, None).expect("world");
        let in_set = |i: usize| {
            w.spawn_modes[i].as_deref().is_some_and(|m| {
                if snd {
                    dc_dserver::chars::name_key(m.trim_ascii().to_ascii_lowercase().as_slice())
                        == SND_SPAWN_FILTER
                } else {
                    initials(m) == ELIM_FALLBACK_SET_KEY
                }
            })
        };
        let team_of = |i: usize| tables.teams.team_of(w.spawns[i].team_filter.as_deref());
        assert!(
            !w.spawn_modes.iter().flatten().any(|m| initials(m) == "TSV"),
            "{world}: no point names Elimination Pro"
        );
        let mut g = Game::new(GameTuning::default(), &w, tables.clone());
        let dc_dserver::pipe::Control::Room(room) =
            dc_dserver::pipe::parse_control("room key=TSV type=12 goal=5")
        else {
            panic!("room line");
        };
        g.set_room(&room);
        for team in [1u8, 2] {
            let n = (0..w.spawns.len())
                .filter(|&i| in_set(i) && team_of(i) == Some(team))
                .count();
            assert!(n >= 11, "{world} team {team}: {n} set points");
            let mut seen = std::collections::BTreeSet::new();
            for _ in 0..300 {
                let (pos, _) = g.choose_spawn(team);
                // a world may place points of several modes at one position: any of them in the set counts
                let i = (0..w.spawns.len())
                    .find(|&i| w.spawns[i].pos == pos && in_set(i) && team_of(i) == Some(team))
                    .unwrap_or_else(|| panic!("{world} team {team}: {pos:?} is not a set point"));
                seen.insert(i);
            }
            assert!(
                seen.len() > n / 2,
                "{world} team {team}: {} of {n} points drawn",
                seen.len()
            );
        }
    }
}

/// DSV-302: Snow Valley's sliding doors carry their opening and closing sound files and radius from the world file,
/// and each file is in the ROM's sound archives.
#[test]
fn rom_snow_valley_doors_have_their_sounds() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let w = load_from_rom(&rom, r"Worlds\SnowValley", NameForm::Listed, None).expect("world");
    assert!(!w.doors.is_empty(), "sliding doors");
    let vfs = dc_rez::RezVfs::new(&rom);
    for d in &w.doors {
        let s = &d.sounds;
        assert!(!s.opening.is_empty() && !s.closing.is_empty(), "{s:?}");
        assert!(s.radius > 0.0);
        for f in [&s.opening, &s.closing] {
            assert!(
                dc_dserver::sounds::rom_play_ms(&vfs, f).is_some(),
                "{f} readable in the ROM"
            );
        }
    }
}
