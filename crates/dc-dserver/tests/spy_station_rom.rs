//! ROM-gated (skipped without CA_ROM): the Spy Hunt upload station over the loopback of the simulated channel on two
//! ROM Spy Hunt worlds. A synthetic 2009-style client seizes the one intel case (`-spyintel 1`) and becomes the agent;
//! the world trigger the mode's GMS file names switches the station's placed effects on, so every client gets
//! type-0x64 placed-effect objects whose effect groups are the world's station effects, inside the world box and
//! next to the world's activation box, and 40 s later the box's ready effect. Names never appear in the output:
//! counts only.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.4 (type 0x64 placed effect `[name:zstr8][value:u32]`); GAME_MODES_2009.md §5.6
// OBSERVED: ROM worlds' activation box and trigger scripts, ROM GMS `[AgentModelSetting] AgentTrigger`
// DC-DEFAULT: DSV-323 (proposed) the stage-2 station flow; CLI-1582 (proposed) the id the ready box takes again

use dc_dserver::game::StartOrder;
use dc_dserver::rom_tables::RomTables;
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{SynthClient, SynthConfig};
use dc_dserver::world::{load_from_rom, NameForm};
use dc_net_engine::sim::{Faults, SimChannel};

struct Peer {
    addr: u32,
    client: SynthClient,
    up: SimChannel,
    down: SimChannel,
}

fn step(server: &mut Server<u32>, peers: &mut [Peer], logs: &mut Vec<String>, now: u32) {
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
            Output::Log(l) => logs.push(l),
            Output::Pipe(_) => {}
        }
    }
    for p in peers.iter_mut() {
        for d in p.down.deliver(now) {
            p.client.handle_datagram(now, &d);
        }
    }
}

/// The effect group name of a type-0x64 blob.
fn effect_name(o: &dc_dserver::synth_client::SeenObject) -> Option<Vec<u8>> {
    let b = o.blob.as_ref()?;
    let mut r = b.reader();
    (r.read_u8() == 0x64).then(|| r.read_zstr8())
}

#[test]
fn rom_the_agent_brings_the_upload_station_to_every_client() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = RomTables::load(&rom);
    let vfs = dc_rez::RezVfs::new(&rom);
    let gr = dc_bute::GameRoomData::from_vfs(&vfs).expect("room table");
    let key = gr
        .modes(false)
        .iter()
        .find(|m| m.get("GameType").map(|v| v.trim()) == Some("9"))
        .and_then(|m| m.get("Key").cloned())
        .expect("a class-9 row");
    for world in ["WARHEAD", "SANDHOG"] {
        let w = load_from_rom(&rom, &format!("Worlds\\{world}"), NameForm::Listed, None).expect("world");
        let (lo, hi) = (w.extent_min, w.extent_max);
        let uplinks = w.spy.uplinks.clone();
        assert!(!uplinks.is_empty(), "{world}: an activation box");
        let station_fx: Vec<Vec<u8>> = w
            .effects
            .iter()
            .filter(|e| {
                uplinks.iter().any(|u| {
                    (0..3).all(|i| (e.pos[i] - u.pos[i]).abs() < 400.0)
                })
            })
            .map(|e| e.fx.to_ascii_lowercase())
            .collect();
        let mut cfg = ServerTuning::default();
        cfg.game.start_order = StartOrder::Spec;
        cfg.game.move_check = false;
        cfg.game.spy_intel = Some(1);
        let mut server = Server::with_tables(cfg, w, tables.clone(), 0);
        assert!(server.handle_line(0, &format!("room key={key} type=9 goal=12 time=10")));
        assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
        assert!(server.handle_line(0, "user 8 4 55 0 0 Erin"));
        let mut peers: Vec<Peer> = [(1u32, "Dave"), (2, "Erin")]
            .into_iter()
            .map(|(addr, name)| {
                let mut c = SynthConfig::named(name);
                c.box_bounds = Some((lo, hi));
                c.walk_step = 0.0;
                c.mode_class_hint = Some(9);
                Peer {
                    addr,
                    client: SynthClient::new(c),
                    up: SimChannel::new(addr as u64 * 2 + 1, Faults::clean(2)),
                    down: SimChannel::new(addr as u64 * 2 + 2, Faults::clean(2)),
                }
            })
            .collect();
        let mut logs = Vec::new();
        let mut now = 0u32;
        while now < 30_000 && !peers.iter().all(|p| p.client.life_state() == 1) {
            step(&mut server, &mut peers, &mut logs, now);
            now += 5;
        }
        assert!(peers.iter().all(|p| p.client.life_state() == 1), "{world}: both alive");
        // Dave walks onto the case
        let mut agent = false;
        let end = now + 20_000;
        while now < end && !agent {
            if let Some(i) = server.game().spy().intel.first() {
                let at = i.pos;
                peers[0].client.set_height(at[1]);
                peers[0].client.walk_to(Some(at), 10_000.0);
            }
            step(&mut server, &mut peers, &mut logs, now);
            now += 5;
            agent = server.game().spy().agent.is_some();
        }
        assert!(agent, "{world}: the agent");
        let effects = |peers: &[Peer], i: usize| -> Vec<(Vec<u8>, [f32; 3])> {
            peers[i]
                .client
                .seen
                .objects
                .values()
                .filter_map(|o| Some((effect_name(o)?.to_ascii_lowercase(), o.pos.unwrap_or(o.create_pos))))
                .collect()
        };
        let before: Vec<Vec<u8>> = effects(&peers, 1).into_iter().map(|e| e.0).collect();
        for _ in 0..200 {
            step(&mut server, &mut peers, &mut logs, now);
            now += 5;
        }
        let station_now = |peers: &[Peer], i: usize| -> Vec<(u16, Vec<u8>)> {
            peers[i]
                .client
                .seen
                .objects
                .iter()
                .filter_map(|(id, o)| Some((*id, effect_name(o)?.to_ascii_lowercase(), o.pos.unwrap_or(o.create_pos))))
                .filter(|e| station_fx.contains(&e.1) && !before.contains(&e.1))
                .map(|(id, n, p)| {
                    assert!((0..3).all(|k| p[k] > lo[k] && p[k] < hi[k]), "{world}: inside the world");
                    assert!(
                        uplinks.iter().any(|u| (0..3).all(|k| (p[k] - u.pos[k]).abs() < 400.0)),
                        "{world}: at the activation box"
                    );
                    (id, n)
                })
                .collect()
        };
        let first = station_now(&peers, 0);
        for i in 0..2 {
            assert!(station_now(&peers, i).len() >= 2, "{world} client {i}: the station's model and particles");
        }
        // 40 s later the box is open: the model leaves and the ready box comes, on the same id (freed and given
        // again in one tick: the client must see a new object there, CLI-1582)
        let end = now + 42_000;
        while now < end {
            step(&mut server, &mut peers, &mut logs, now);
            now += 5;
        }
        assert!(logs.iter().any(|l| l.contains("spy: the uplink") && l.contains("is open")), "{world}: open");
        let later = station_now(&peers, 1);
        let ready: Vec<&(u16, Vec<u8>)> = later.iter().filter(|e| !first.iter().any(|f| f.1 == e.1)).collect();
        assert_eq!(ready.len(), 1, "{world}: the ready box");
        assert!(
            first.iter().any(|f| f.0 == ready[0].0),
            "{world}: the ready box took the model's id"
        );
        for p in &peers {
            assert!(p.client.seen.errors.is_empty(), "{world}: {:?}", p.client.seen.errors);
        }
    }
}
