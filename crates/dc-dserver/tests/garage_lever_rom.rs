//! ROM-gated (skipped without CA_ROM): Rattlesnake's garage (live report 2026-11-02 "the garage door on Rattlesnake is
//! supposed to give the press E popup when you're able to flip the switch"). The world's two levers are rotating
//! doors players use (enabled activate entry, the use prompt); its two shutters are sliding doors only the levers'
//! commands move (disabled entry). Over the loopback of the simulated channel a synthetic 2009-style client stands
//! in front of one lever and presses the use key (`UP_USE` with no object): the lever and its twin turn, both are
//! locked for their wait (entry disabled, then enabled again), and both shutters slide open on every client. No
//! start script of the world hides or disables a lever (DSV-500 / DSV-501 left them alone). Counts and comparisons
//! only; no ROM name appears in the output.
// OBSERVED: the ROM Rattlesnake world's door objects (player-activate, rotation, the levers' opening / open / closing /
// closed commands `msg <lever> on|lock|unlock`, `delay`, `msg <shutter> on|off`; compared, not copied)
// DC-DEFAULT: DSV-270, DSV-340, DSV-460 (proposed) the use key, the activate entry and its notes

use dc_dserver::doors::{OpenBy, TurnKind};
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

/// The `Worlds\<name>` argument of the archive's Rattlesnake world.
fn rattlesnake(rom: &std::path::Path) -> Option<String> {
    let arch = dc_rez::RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE)).ok()?;
    let listed = arch.entries().iter().map(|e| e.0.clone()).find(|n| {
        let u = n.to_ascii_uppercase();
        u.ends_with(WORLD_EXT) && u.contains("RATTLESNAKE")
    })?;
    let base = listed.rsplit('\\').next().unwrap_or(&listed).to_string();
    Some(format!("Worlds\\{}", &base[..base.len() - WORLD_EXT.len()]))
}

/// The activate entry `(row, disabled, state)` of a door create blob.
fn entry(o: &dc_dserver::synth_client::SeenObject) -> Option<(u8, bool, u8)> {
    let mut r = o.blob.as_ref()?.reader();
    (r.read_u8() == 0x39).then(|| (r.read_u8(), r.read_bit(), r.read_u8()))
}

fn mid(lo: [f32; 3], hi: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]))
}

#[test]
fn rom_the_lever_opens_the_garage_for_everyone() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(arg) = rattlesnake(&rom) else {
        println!("skipped: no Rattlesnake world");
        return;
    };
    let (tables, _) = RomTables::load(&rom);
    let w = load_from_rom(&rom, &arg, NameForm::Listed, None).expect("Rattlesnake");
    let levers: Vec<_> = w
        .turns
        .iter()
        .filter(|t| t.kind == TurnKind::Door && t.open_by == OpenBy::Use)
        .cloned()
        .collect();
    let shutters: Vec<_> = w
        .doors
        .iter()
        .filter(|d| d.open_by == OpenBy::Message && !d.usable)
        .cloned()
        .collect();
    assert_eq!((levers.len(), shutters.len()), (2, 2));
    // DSV-500 / DSV-501: no start trigger of the world hides a lever, makes it non-solid or switches it off
    let mut triggers = 0;
    for (name, _) in &w.scripts.triggers {
        let trig = format!("|{}||", String::from_utf8_lossy(name));
        let ov = dc_dserver::worldobj::run_start_scripts(&w.scripts, Some(&trig));
        for l in &levers {
            let o = ov
                .get(&l.name.to_ascii_lowercase())
                .copied()
                .unwrap_or_default();
            assert_ne!(o.visible, Some(false), "a trigger hides a lever");
            assert_ne!(o.solid, Some(false), "a trigger makes a lever non-solid");
        }
        triggers += 1;
    }
    println!("{triggers} start triggers keep the levers");

    let (lo, hi) = (w.extent_min, w.extent_max);
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    let mut server = Server::with_tables(cfg, w, tables, 0);
    assert!(server.handle_line(0, "room key=TDM type=1 goal=9"));
    assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
    assert!(server.handle_line(0, "user 8 4 55 1 0 Erin"));
    let mut peers: Vec<Peer> = [(1u32, "Dave"), (2, "Erin")]
        .into_iter()
        .map(|(addr, name)| {
            let mut c = SynthConfig::named(name);
            c.box_bounds = Some((lo, hi));
            c.walk_step = 0.0;
            c.mode_class_hint = Some(1);
            Peer {
                addr,
                client: SynthClient::new(c),
                up: SimChannel::new(addr as u64 * 2 + 1, Faults::clean(2)),
                down: SimChannel::new(addr as u64 * 2 + 2, Faults::clean(2)),
            }
        })
        .collect();
    let mut now = 0u32;
    while now < 30_000 && !peers.iter().all(|p| p.client.life_state() == 1) {
        step(&mut server, &mut peers, now);
        now += 5;
    }
    assert!(
        peers.iter().all(|p| p.client.life_state() == 1),
        "both alive"
    );
    let obj_of = |server: &Server<u32>, name: &[u8]| -> u16 {
        server
            .turns()
            .iter()
            .map(|t| (t.obj, &t.info.name))
            .chain(server.doors().iter().map(|d| (d.obj, &d.info.name)))
            .find(|(_, n)| n.eq_ignore_ascii_case(name))
            .map(|(o, _)| o)
            .expect("created")
    };
    // DSV-340: every client has the levers with an enabled entry (the use prompt) in the closed state, the shutters
    // with a disabled one
    for p in &peers {
        for l in &levers {
            let o = &p.client.seen.objects[&obj_of(&server, &l.name)];
            assert_eq!(o.kind, 2);
            assert_eq!(entry(o), Some((0, false, 1)), "a lever: enabled, closed");
        }
        for s in &shutters {
            let o = &p.client.seen.objects[&obj_of(&server, &s.name)];
            assert_eq!(entry(o).map(|e| e.1), Some(true), "a shutter: disabled");
        }
    }

    // Dave stands 48 units in front of the first lever (on its side of the shutters) and presses the use key
    let l = &levers[0];
    let (s_mid, l_mid) = (mid(shutters[0].lo, shutters[0].hi), mid(l.lo, l.hi));
    let ext = |k: usize| shutters[0].hi[k] - shutters[0].lo[k];
    let thin = if ext(0) < ext(2) { 0 } else { 2 };
    let mut stand = l_mid;
    stand[thin] += 48.0 * (l_mid[thin] - s_mid[thin]).signum();
    stand[1] = l_mid[1] - 30.0;
    for _ in 0..100 {
        peers[0].client.set_height(stand[1]);
        peers[0].client.walk_to(Some(stand), 100_000.0);
        step(&mut server, &mut peers, now);
        now += 5;
    }
    let mut b = dc_wire::udp::BitWriter::new();
    b.write_u16(0xffff).write_u8(0);
    peers[0].client.send_raw_shell(0x59, &b);
    let used = now;
    while now < used + 1500 {
        step(&mut server, &mut peers, now);
        now += 5;
    }
    for t in server
        .turns()
        .iter()
        .filter(|t| levers.iter().any(|l| l.name == t.info.name))
    {
        assert!(t.want && t.frac > 0.99, "both levers turned");
        assert_eq!(
            t.activate(),
            Some((true, 0)),
            "on and locked: no prompt for the wait"
        );
    }
    // the shutters slide open (8 s in the file) on every client
    while now < used + 12_000 {
        step(&mut server, &mut peers, now);
        now += 5;
    }
    for s in &shutters {
        let d = server
            .doors()
            .iter()
            .find(|d| d.info.name == s.name)
            .expect("shutter");
        assert_eq!(d.offset, s.dist, "open on the server");
        let open = [0, 1, 2].map(|k| s.closed[k] + s.dir[k] * s.dist);
        for p in &peers {
            let at = p.client.seen.objects[&d.obj].pos.expect("moved");
            assert!(
                (0..3).all(|k| (at[k] - open[k]).abs() < 0.05),
                "open on every client: {at:?} vs {open:?}"
            );
        }
    }
    // the lever's entry on the clients: state on, disabled for the lock, enabled again after the 7 s wait
    for p in &peers {
        let lever = obj_of(&server, &l.name);
        let notes: Vec<(u8, u8)> = p
            .client
            .seen
            .door_notes
            .iter()
            .filter(|n| n.1 == lever)
            .map(|n| (n.2, n.3))
            .collect();
        assert!(notes.contains(&(2, 0)), "state on: {notes:?}");
        let lock = notes.iter().position(|n| *n == (1, 1)).expect("locked");
        assert!(notes[lock..].contains(&(1, 0)), "unlocked again: {notes:?}");
        assert!(
            p.client.seen.errors.is_empty(),
            "{:?}",
            p.client.seen.errors
        );
    }
}
