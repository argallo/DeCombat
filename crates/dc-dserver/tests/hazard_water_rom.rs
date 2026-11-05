//! ROM-gated (skipped without CA_ROM): the hazard volumes over WarHead's sewer water and Two Towers' water hurt the
//! players in them (owner reports 2026-11-05: "in warhead quarantine im able to walk under the poison water", "in two
//! towers the quarantine infected were able to walk in the water"). Over the loopback of the simulated channel three
//! synthetic 2009-style clients play a Quarantine room on each world; one stands in the water (on the floor below the
//! water's top as the server's level model has it), one on a floor above the water and above the hazard volume.
//! Checked: the one in the water takes damage steps of the poison type with no attacker (0x74 `zone` = the damage
//! type, `dmg_kind` 0, `from_dir` up) every 500 ms and dies with no killer (0x6d killer 0xff), the one above takes
//! none; after the outbreak an infected player in the water is hurt as well. A TDM room on the same world runs the
//! map's TDM start trigger, which switches the hazard volume off: nobody is hurt there. Counts and comparisons only;
//! no ROM name or value is written here.
// OBSERVED: the ROM WarHead and Two Towers worlds' hazard volumes, water volumes and start triggers (the TDM, TMM,
// FFA, CTF and BNT start triggers send `off` to the hazard volume and the water, the Quarantine one does not; read at
// run time, compared, not copied); ROM GAMEROOM / GMS tables (each mode's start trigger)
// DC-DEFAULT: DSV-607 (proposed)

use dc_dserver::killvol::DAMAGE_TYPE_POISON;
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server, ServerTuning, HAZARD_STEP_MS};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{load_from_rom, NameForm, WorldInfo, WORLD_ARCHIVE, WORLD_EXT};
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

/// The archive's world whose upper-case listed name ends in `<name>.DAT`: the `Worlds\<name>` argument and the
/// decrypted bytes.
fn world_named(rom: &std::path::Path, upper: &str) -> Option<(String, Vec<u8>)> {
    let arch = dc_rez::RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE)).ok()?;
    let listed = arch.entries().iter().map(|e| e.0.clone()).find(|n| {
        let u = n.to_ascii_uppercase();
        u.ends_with(&format!("\\{upper}{WORLD_EXT}"))
    })?;
    let raw = arch.entry_raw(&listed).ok()??;
    let data = if raw.starts_with(&85u32.to_le_bytes()) {
        raw
    } else {
        let key = dc_rez::dc_rom::crypt_key(rom).ok()?;
        dc_rez::decrypt_with_key(&raw, &key).ok()?
    };
    let base = listed.rsplit('\\').next().unwrap_or(&listed).to_string();
    Some((
        format!("Worlds\\{}", &base[..base.len() - WORLD_EXT.len()]),
        data,
    ))
}

/// The model boxes of the world's water volumes.
fn water_boxes(data: &[u8]) -> Vec<([f32; 3], [f32; 3])> {
    let w = dc_world::World::parse(data).expect("world");
    w.objects
        .iter()
        .filter(|o| o.is_class(&dc_world::names::CLASS_WATER))
        .filter_map(|o| w.brush_of(o).map(|m| (m.box_min, m.box_max)))
        .collect()
}

/// The room line the lobby writes for mode `key` on the world `arg`.
fn room_line(rom: &std::path::Path, key: &str, arg: &str) -> Option<String> {
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
        b.strip_suffix(WORLD_EXT).map(str::to_string).unwrap_or(b)
    };
    let want = base(arg);
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
            return Some(dc_bute::room_line_with_vfs(
                &vfs,
                &gr,
                &cfg,
                &dc_bute::RoomOptions::default(),
            ));
        }
    }
    None
}

/// Two standing places over the water, found on the server's level model, outside the kill volumes the room acts
/// on: one in the water (dropped onto the floor from just under the water's top; kept there when nothing is below)
/// inside a hurting hazard volume, and one on a floor above the water and above every hazard volume there. Returns
/// (in, above, hazard index).
fn places(
    w: &WorldInfo,
    tables: &RomTables,
    water: &[([f32; 3], [f32; 3])],
    kills: &[usize],
) -> ([f32; 3], [f32; 3], usize) {
    let g = dc_dserver::game::Game::new(dc_dserver::game::GameTuning::default(), w, tables.clone());
    let dims = dc_dserver::game::GameTuning::default().player_dims;
    let killed = |p: [f32; 3]| kills.iter().any(|&i| w.kill_shapes[i].contains(p));
    let (mut inside, mut above) = (None, None);
    for (lo, hi) in water {
        // grid cells nearest the water's middle first
        let n = 48usize;
        let mut cells: Vec<(f32, f32)> = (0..n * n)
            .map(|c| {
                let x = lo[0] + (hi[0] - lo[0]) * ((c / n) as f32 + 0.5) / n as f32;
                let z = lo[2] + (hi[2] - lo[2]) * ((c % n) as f32 + 0.5) / n as f32;
                (x, z)
            })
            .collect();
        let mid = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[2] + hi[2])];
        let d = |c: &(f32, f32)| (c.0 - mid[0]).powi(2) + (c.1 - mid[1]).powi(2);
        cells.sort_by(|a, b| d(a).total_cmp(&d(b)));
        for (x, z) in cells {
            if inside.is_none() {
                let p = g.floor_point([x, hi[1] - 4.0, z], dims);
                if p[1] <= hi[1] && !killed(p) {
                    if let Some(i) = w.hazards.iter().position(|h| h.shape.contains(p)) {
                        inside = Some((p, i));
                    }
                }
            }
            if above.is_none() {
                let top = w
                    .hazards
                    .iter()
                    .filter(|h| h.shape.lo[0] <= x && x <= h.shape.hi[0])
                    .filter(|h| h.shape.lo[2] <= z && z <= h.shape.hi[2])
                    .map(|h| h.shape.hi[1])
                    .fold(hi[1], f32::max);
                let from = [x, top + 400.0, z];
                let p = g.floor_point(from, dims);
                if p[1] < from[1] - 1.0
                    && p[1] - dims[1] > top
                    && !w.hazards.iter().any(|h| h.shape.contains(p))
                    && !killed(p)
                {
                    above = Some(p);
                }
            }
        }
    }
    let (inside, i) = inside.expect("a standing place in the water inside a hazard volume");
    (inside, above.expect("a floor above the water"), i)
}

struct Match {
    server: Server<u32>,
    peers: Vec<Peer>,
    now: u32,
}

fn start(w: WorldInfo, tables: &RomTables, line: &str, class: u8) -> Match {
    let (lo, hi) = (w.extent_min, w.extent_max);
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    let mut server = Server::with_tables(cfg, w, tables.clone(), 0);
    assert!(server.handle_line(0, line), "room line");
    for u in [
        "user 7 3 99 0 120 Dave",
        "user 8 4 55 1 0 Erin",
        "user 9 5 56 1 0 Finn",
    ] {
        assert!(server.handle_line(0, u));
    }
    let peers: Vec<Peer> = [(1u32, "Dave"), (2, "Erin"), (3, "Finn")]
        .into_iter()
        .map(|(addr, name)| {
            let mut c = SynthConfig::named(name);
            c.box_bounds = Some((lo, hi));
            c.walk_step = 0.0;
            c.mode_class_hint = Some(class);
            Peer {
                addr,
                client: SynthClient::new(c),
                up: SimChannel::new(addr as u64 * 2 + 1, Faults::clean(2)),
                down: SimChannel::new(addr as u64 * 2 + 2, Faults::clean(2)),
            }
        })
        .collect();
    let mut m = Match {
        server,
        peers,
        now: 0,
    };
    while m.now < 30_000 && !m.peers.iter().all(|p| p.client.life_state() == 1) {
        m.tick();
    }
    assert!(
        m.peers.iter().all(|p| p.client.life_state() == 1),
        "everyone alive"
    );
    m
}

impl Match {
    fn tick(&mut self) {
        step(&mut self.server, &mut self.peers, self.now);
        self.now += 5;
    }

    /// Keeps peer `k` at `at` (every player update reports it there).
    fn hold(&mut self, k: usize, at: [f32; 3]) {
        self.peers[k].client.set_height(at[1]);
        self.peers[k].client.walk_to(Some(at), 1.0e6);
    }

    fn pid(&self, k: usize) -> u8 {
        self.peers[k].client.seen.client_id.expect("client id").0 as u8
    }
}

fn check_world(rom: &std::path::Path, tables: &RomTables, upper: &str) {
    let Some((arg, data)) = world_named(rom, upper) else {
        println!("skipped: no {upper} world");
        return;
    };
    let water = water_boxes(&data);
    assert!(!water.is_empty(), "{upper}: a water volume");
    let w = load_from_rom(rom, &arg, NameForm::Listed, None).expect("world");
    // the Quarantine room: the hazard acts
    let line = room_line(rom, "QRT", &arg).expect("QRT room line");
    let mut m = start(w.clone(), tables, &line, 11);
    let (in_water, above, hz) = places(&w, tables, &water, m.server.active_kill_volumes());
    let h = w.hazards[hz].clone();
    assert_eq!(h.dtype, DAMAGE_TYPE_POISON, "{upper}: the hazard is poison");
    println!(
        "{upper}: hazard {hz} of {} ({} per second); in the water {:.0} below the water's top, the floor above {:.0} \
         over the hazard's top",
        w.hazards.len(),
        h.damage_per_sec,
        water.iter().map(|b| b.1[1]).fold(f32::MIN, f32::max) - in_water[1],
        above[1] - h.shape.hi[1]
    );

    assert!(
        m.server.active_hazards().contains(&hz),
        "{upper}: Quarantine keeps the hazard on"
    );
    let health0 = m.server.game().players()[&m.pid(0)].health;
    let t0 = m.now;
    let mut died = None;
    while m.now < t0 + 12_000 && died.is_none() {
        m.hold(0, in_water);
        m.hold(1, above);
        m.tick();
        if m.peers[0].client.life_state() != 1 {
            died = Some(m.now);
        }
    }
    let seen = &m.peers[0].client.seen;
    assert!(
        !seen.damage_forms.is_empty(),
        "{upper}: damage in the water"
    );
    for d in &seen.damage_forms {
        assert_eq!(d.zone, DAMAGE_TYPE_POISON, "{upper}: the poison type");
        assert_eq!(d.dmg_kind, 0, "{upper}: no-attacker kind");
        assert!(
            d.from_dir[0] == 0.0 && d.from_dir[2] == 0.0 && d.from_dir[1] > 0.0,
            "{upper}: from up"
        );
    }
    let first = seen.damage_forms[0].at;
    assert!(
        first <= t0 + 1000,
        "{upper}: hurt within a second of stepping in"
    );
    for pair in seen.damage_forms.windows(2) {
        let gap = pair[1].at - pair[0].at;
        assert!(
            (HAZARD_STEP_MS - 20..=HAZARD_STEP_MS + 20).contains(&gap),
            "{upper}: a step every {HAZARD_STEP_MS} ms, got {gap}"
        );
    }
    let died = died.expect("died in the water");
    let kill = seen
        .kills
        .iter()
        .find(|k| k.victim == m.pid(0))
        .expect("a kill feed line");
    assert_eq!(kill.killer, 0xff, "{upper}: no killer");
    let steps = seen.damage_forms.len();
    // no more steps than the health needs at the step damage (the worn armour may take a share and add some)
    let per_step = h.damage_per_sec * HAZARD_STEP_MS as f32 / 1000.0;
    let dealt: u32 = seen.damage_forms.iter().map(|d| d.damage).sum();
    assert!(
        dealt as f32 >= health0 - 1.0
            && seen.damage_forms[..steps - 1]
                .iter()
                .all(|d| d.damage as f32 <= per_step + 1.0),
        "{upper}: {steps} steps dealt {dealt} of {health0}"
    );
    assert!(
        m.peers[1].client.seen.damage.is_empty(),
        "{upper}: no damage on the floor above"
    );
    println!(
        "{upper}: Quarantine: {steps} damage steps, death {} ms after stepping in; the player above unhurt",
        died - t0
    );

    // after the outbreak: an infected player in the water is hurt too
    while m.now < t0 + 60_000 && m.server.game().infect().infected.is_empty() {
        m.hold(1, above);
        m.tick();
    }
    let infected = *m
        .server
        .game()
        .infect()
        .infected
        .iter()
        .next()
        .expect("an outbreak");
    let k = (0..m.peers.len())
        .find(|&k| m.pid(k) == infected)
        .expect("the infected peer");
    assert_eq!(m.peers[k].client.life_state(), 1, "the infected is alive");
    let before = m.peers[k].client.seen.damage_forms.len();
    let health = m.server.game().players()[&infected].health;
    let t1 = m.now;
    while m.now < t1 + 3000 {
        m.hold(k, in_water);
        m.tick();
    }
    let forms = &m.peers[k].client.seen.damage_forms[before..];
    assert!(
        !forms.is_empty(),
        "{upper}: the infected is hurt in the water"
    );
    assert!(forms.iter().all(|d| d.zone == DAMAGE_TYPE_POISON));
    let after = &m.server.game().players()[&infected];
    println!(
        "{upper}: an infected in the water: {} steps in 3 s, health {health:.0} -> {:.0} ({:?})",
        forms.len(),
        after.health,
        after.life
    );

    // the TDM room: its start trigger switches the hazard volume off; nobody in the water is hurt
    let line = room_line(rom, "TDM", &arg).expect("TDM room line");
    let mut m = start(w.clone(), tables, &line, 1);
    assert!(
        !m.server.active_hazards().contains(&hz),
        "{upper}: TDM's start trigger switches the hazard off"
    );
    let t0 = m.now;
    while m.now < t0 + 3000 {
        m.hold(0, in_water);
        m.tick();
    }
    // (a kill volume of the TDM room may take the player there: the TDM start trigger leaves it on)
    assert!(
        !m.peers[0]
            .client
            .seen
            .damage_forms
            .iter()
            .any(|d| d.zone == DAMAGE_TYPE_POISON),
        "{upper}: TDM: no poison in the water"
    );
}

#[test]
fn rom_the_quarantine_waters_hurt_and_the_floor_above_does_not() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = RomTables::load(&rom);
    for upper in ["WARHEAD", "TWOTOWERS"] {
        check_world(&rom, &tables, upper);
    }
}
