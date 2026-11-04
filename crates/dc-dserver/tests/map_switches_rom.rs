//! ROM-gated (skipped without CA_ROM): the world switches and locked doors of two live reports (2026-11-02 "Overdose
//! light/elevator switches never usable; Sand Hog CP1 locked door opens without prompt").
//!
//! - Overdose's two light switches are player-activated rotating switches whose on / off commands fire the room's
//!   light-on / light-off trigger, which makes the lit lamp models visible and the unlit ones invisible (and the
//!   other way). Over the loopback of the simulated channel a synthetic 2009-style client stands at a switch and
//!   presses the use key (`UP_USE` with no object): the switch turns on, the lit models appear and the unlit ones
//!   vanish on the client; the use key again turns it off and back.
//! - SANDHOG_CP1 (the co-op Desert Thunder world) has player-activated doors its file locks: they are created in a
//!   co-op room (before: left out, an open doorway) with a disabled entry (no prompt); the use key does not open
//!   them; the trigger box in front of the sliding one unlocks it when a player walks in, then the use key opens it;
//!   a locked gate's locked command fires the trigger that unlocks the other side's gates.
//!
//! Counts and comparisons only; no ROM name appears in the source or the output.
// OBSERVED: the ROM OVERDOSE world (rotating switches: player-activate, no activate type, on / off commands
// `msg <room>.<trigger> trigger`, the triggers' `msg <model> (visible 0|1)` and a light-group switch; the lit models
// start invisible, non-solid and without ray-hit); the ROM SANDHOG_CP1 world (locked player-activated rotating and
// sliding doors with locked sounds and commands; player-triggerable boxes that fire once and unlock them)
// DC-DEFAULT: DSV-560, DSV-561, DSV-562, DSV-563 (proposed)

use dc_dserver::doors::{
    parse_statements, run_world, use_key, DelayedStatements, Door, OpenBy, Triggers, TurnDoor,
    TurnKind,
};
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{load_from_rom, NameForm, WorldInfo, WORLD_ARCHIVE, WORLD_EXT};
use dc_dserver::worldobj::WorldObject;
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

/// The `Worlds\<name>` argument of the archive's world whose file name is `stem` + the world extension.
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

/// The activate entry `(row, disabled, state)` of a door create blob.
fn entry(o: &dc_dserver::synth_client::SeenObject) -> Option<(u8, bool, u8)> {
    let mut r = o.blob.as_ref()?.reader();
    (r.read_u8() == 0x39).then(|| (r.read_u8(), r.read_bit(), r.read_u8()))
}

fn mid(lo: [f32; 3], hi: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]))
}

/// The models a command text makes visible (`true`) or invisible through the triggers it fires.
fn switched_models(w: &WorldInfo, text: &[u8]) -> Vec<(Vec<u8>, bool)> {
    let mut out = Vec::new();
    for st in parse_statements(text) {
        let Some(t) = w
            .triggers
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(&st.target))
        else {
            continue;
        };
        for c in &t.cmds {
            for s in parse_statements(c) {
                if let Some(v) = s.value {
                    if w.brushes
                        .iter()
                        .any(|b| b.name.eq_ignore_ascii_case(&s.target))
                    {
                        out.push((s.target.clone(), v));
                    }
                }
            }
        }
    }
    out
}

fn peers(lo: [f32; 3], hi: [f32; 3], names: &[(u32, &str)], class: u8) -> Vec<Peer> {
    names
        .iter()
        .map(|&(addr, name)| {
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
        .collect()
}

fn stand_at(server: &mut Server<u32>, peers: &mut [Peer], now: &mut u32, at: [f32; 3]) {
    for _ in 0..100 {
        peers[0].client.set_height(at[1]);
        peers[0].client.walk_to(Some(at), 100_000.0);
        step(server, peers, *now);
        *now += 5;
    }
}

fn press_use(server: &mut Server<u32>, peers: &mut [Peer], now: &mut u32, ms: u32) {
    let mut b = dc_wire::udp::BitWriter::new();
    b.write_u16(0xffff).write_u8(0);
    peers[0].client.send_raw_shell(0x59, &b);
    let until = *now + ms;
    while *now < until {
        step(server, peers, *now);
        *now += 5;
    }
}

/// The object of the planned brush of this name and whether it is visible (`None`: not created).
fn brush_obj(server: &Server<u32>, name: &[u8]) -> Option<(u16, bool)> {
    server.world_objects().iter().find_map(|o| match o {
        WorldObject::Brush { obj, brush } if brush.name.eq_ignore_ascii_case(name) => {
            Some((*obj, brush.visible))
        }
        _ => None,
    })
}

#[test]
fn rom_overdose_light_switches_turn_and_switch_the_room_models() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(arg) = world_arg(&rom, "OVERDOSE") else {
        println!("skipped: no Overdose world");
        return;
    };
    let (tables, _) = RomTables::load(&rom);
    let w = load_from_rom(&rom, &arg, NameForm::Listed, None).expect("Overdose");
    let switches: Vec<_> = w
        .turns
        .iter()
        .filter(|t| t.kind == TurnKind::Switch)
        .cloned()
        .collect();
    assert_eq!(switches.len(), 2);
    // DSV-561: both are use switches (before: created, never turned, no entry)
    assert!(switches.iter().all(|s| s.open_by == OpenBy::Use));
    let sw = &switches[0];
    let on = switched_models(&w, &sw.cmds.open);
    let off = switched_models(&w, &sw.cmds.closed);
    assert!(
        on.iter().any(|m| m.1) && on.iter().any(|m| !m.1),
        "the on command shows some models and hides others: {}",
        on.len()
    );
    assert!(!off.is_empty());
    println!(
        "switch: {} models by the on command, {} by the off command",
        on.len(),
        off.len()
    );

    let (lo, hi) = (w.extent_min, w.extent_max);
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    let mut server = Server::with_tables(cfg, w.clone(), tables, 0);
    assert!(server.handle_line(0, "room key=TDM type=1 goal=9"));
    assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
    let mut peers = peers(lo, hi, &[(1, "Dave")], 1);
    let mut now = 0u32;
    while now < 30_000 && peers[0].client.life_state() != 1 {
        step(&mut server, &mut peers, now);
        now += 5;
    }
    assert_eq!(peers[0].client.life_state(), 1, "alive");
    let sobj = server
        .turns()
        .iter()
        .find(|t| t.info.name == sw.name)
        .map(|t| t.obj)
        .expect("switch created");
    // DSV-561: the switch's entry is enabled and off on the client (the use prompt)
    assert_eq!(
        entry(&peers[0].client.seen.objects[&sobj]),
        Some((0, false, 1)),
        "an enabled switch entry, off"
    );
    // the lit models are not created at the start (none of the three flags), the unlit ones are, visible
    for (name, v) in &on {
        if *v {
            assert_eq!(brush_obj(&server, name), None, "a lit model at the start");
        } else {
            assert_eq!(brush_obj(&server, name).map(|b| b.1), Some(true));
        }
    }
    // Dave stands 60 units below the switch's middle (within the use key's reach) and presses the use key
    let mut stand = mid(sw.lo, sw.hi);
    stand[1] -= 60.0;
    stand_at(&mut server, &mut peers, &mut now, stand);
    press_use(&mut server, &mut peers, &mut now, 1000);
    let t = server
        .turns()
        .iter()
        .find(|t| t.obj == sobj)
        .expect("switch");
    assert!(t.want && t.frac > 0.99, "the switch turned on");
    let seen = |peers: &[Peer], obj: u16| {
        peers[0]
            .client
            .seen
            .objects
            .get(&obj)
            .and_then(|o| o.flags)
            .map(|f| f & 1 == 1)
    };
    for (name, v) in &on {
        let (obj, vis) = brush_obj(&server, name).expect("the model exists now");
        assert_eq!(vis, *v, "planned state");
        assert_eq!(seen(&peers, obj), Some(*v), "the client's flags");
    }
    assert!(peers[0]
        .client
        .seen
        .door_notes
        .iter()
        .any(|n| n.1 == sobj && (n.2, n.3) == (2, 0)));
    // the use key again: off, the models back
    press_use(&mut server, &mut peers, &mut now, 1000);
    for (name, v) in &off {
        let (obj, vis) = brush_obj(&server, name).expect("model");
        assert_eq!(vis, *v);
        assert_eq!(seen(&peers, obj), Some(*v));
    }
    assert!(
        peers[0].client.seen.errors.is_empty(),
        "{:?}",
        peers[0].client.seen.errors
    );
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

#[test]
fn rom_sandhog_cp1_locked_doors_stay_shut_until_unlocked() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(arg) = world_arg(&rom, "SANDHOG_CP1") else {
        println!("skipped: no co-op Sand Hog world");
        return;
    };
    let (tables, _) = RomTables::load(&rom);
    let w = load_from_rom(&rom, &arg, NameForm::Listed, None).expect("Sand Hog CP1");
    // DSV-560: the file's locked player doors are use doors that start locked (before: rotating ones were message
    // doors left out of a co-op room, the sliding one was unlocked for the use key)
    let locked_turns: Vec<_> = w
        .turns
        .iter()
        .filter(|t| t.kind == TurnKind::Door && t.locked)
        .cloned()
        .collect();
    let locked_slides: Vec<_> = w.doors.iter().filter(|d| d.locked).cloned().collect();
    assert!(!locked_turns.is_empty() && !locked_slides.is_empty());
    assert!(locked_turns.iter().all(|t| t.open_by == OpenBy::Use));
    assert!(locked_slides
        .iter()
        .all(|d| d.open_by == OpenBy::Use && d.usable));
    println!(
        "{} locked rotating doors, {} locked sliding doors",
        locked_turns.len(),
        locked_slides.len()
    );

    // doors level: every locked door keeps shut on the use key; a gate whose locked command fires a trigger unlocks
    // the doors that trigger names
    let mut doors: Vec<Door> = w
        .doors
        .iter()
        .enumerate()
        .map(|(i, d)| Door::new(100 + i as u16, d.clone()))
        .collect();
    let mut turns: Vec<TurnDoor> = w
        .turns
        .iter()
        .enumerate()
        .map(|(i, d)| TurnDoor::new(300 + i as u16, d.clone()))
        .collect();
    let mut trig = Triggers::new(w.triggers.clone());
    let mut dl = DelayedStatements::new();
    let gate = turns
        .iter()
        .position(|t| t.locked && !t.info.cmds.locked.is_empty())
        .expect("a gate with a locked command");
    let at = mid(turns[gate].info.lo, turns[gate].info.hi);
    let unlocked_before = turns.iter().filter(|t| !t.locked).count();
    assert!(
        !use_key(&mut doors, &mut turns, at, 0),
        "a locked gate does not turn"
    );
    assert!(!turns[gate].info.sounds.locked.is_empty());
    assert_eq!(
        turns[gate].sounds_due,
        [turns[gate].info.sounds.locked.clone()]
    );
    run_world(&mut doors, &mut turns, &mut trig, &mut dl, Vec::new(), 0);
    assert!(
        turns[gate].locked && !turns[gate].want,
        "still locked and shut"
    );
    let unlocked_after = turns.iter().filter(|t| !t.locked).count();
    assert!(
        unlocked_after > unlocked_before,
        "the locked command's trigger unlocked other gates"
    );

    // session level, a co-op room: the locked doors are created, closed, with a disabled entry; the sliding one
    // keeps shut on the use key until its player is in the trigger box that unlocks it
    let Some(key) = coop_key(&rom) else {
        println!("skipped: no co-op mode in the room table");
        return;
    };
    let slide = &locked_slides[0];
    let unlocker = w
        .triggers
        .iter()
        .find(|t| {
            t.touch
                && t.cmds.iter().any(|c| {
                    parse_statements(c)
                        .iter()
                        .any(|s| s.target.eq_ignore_ascii_case(&slide.name))
                })
        })
        .cloned()
        .expect("a trigger box unlocks the sliding door");
    let (lo, hi) = (w.extent_min, w.extent_max);
    let mut cfg = ServerTuning::default();
    cfg.game.move_check = false;
    let mut server = Server::with_tables(cfg, w.clone(), tables, 0);
    assert!(server.handle_line(0, &format!("room key={key} type=5 goal=1 ff=0 time=600")));
    assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
    let mut peers = peers(lo, hi, &[(1, "Dave")], 5);
    let mut now = 0u32;
    while now < 30_000 && peers[0].client.life_state() != 1 {
        step(&mut server, &mut peers, now);
        now += 5;
    }
    assert_eq!(peers[0].client.life_state(), 1, "alive");
    for t in &locked_turns {
        let obj = server
            .turns()
            .iter()
            .find(|x| x.info.name == t.name)
            .map(|x| x.obj)
            .expect("a locked rotating door is created in a co-op room");
        assert_eq!(
            entry(&peers[0].client.seen.objects[&obj]).map(|e| e.1),
            Some(true),
            "disabled entry: no prompt"
        );
    }
    let sobj = server
        .doors()
        .iter()
        .find(|d| d.info.name == slide.name)
        .map(|d| d.obj)
        .expect("sliding door");
    // in front of the door, away from the unlocking box: the use key does nothing
    let dm = mid(slide.lo, slide.hi);
    let mut front = dm;
    let away = if dm[0] < unlocker.pos[0] { -1.0 } else { 1.0 };
    front[0] += away * 60.0;
    stand_at(&mut server, &mut peers, &mut now, front);
    press_use(&mut server, &mut peers, &mut now, 2000);
    let d = server.doors().iter().find(|d| d.obj == sobj).unwrap();
    assert!(d.locked && d.offset == 0.0, "locked and shut");
    // into the trigger box: unlocked; the use key there opens it, with the prompt entry enabled
    stand_at(&mut server, &mut peers, &mut now, unlocker.pos);
    let d = server.doors().iter().find(|d| d.obj == sobj).unwrap();
    assert!(!d.locked, "the box unlocked it");
    assert!(peers[0]
        .client
        .seen
        .door_notes
        .iter()
        .any(|n| n.1 == sobj && (n.2, n.3) == (1, 0)));
    let near = [0, 1, 2].map(|k| unlocker.pos[k].clamp(slide.lo[k] - 100.0, slide.hi[k] + 100.0));
    stand_at(&mut server, &mut peers, &mut now, near);
    press_use(&mut server, &mut peers, &mut now, 3000);
    let d = server.doors().iter().find(|d| d.obj == sobj).unwrap();
    assert!(d.offset > 0.0, "opened by the use key");
}
