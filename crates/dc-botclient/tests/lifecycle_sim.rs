//! Death, respawn and dead-link exit of the bot against a scripted server (start-up script as in session_sim.rs):
//! 0x67 state 3 then 2, 0xd7 sub 2 arming, the 7000 ms wait, the 0x89 request, 0x67 state 1, silence, bye. Simulated time.

use dc_botclient::record::Recorder;
use dc_botclient::session::{BotSession, GameConfig, Stage};
use dc_net_engine::sim::{Faults, SimChannel};
use dc_net_engine::{Host, HostConfig, HostEvent};
use dc_wire::udp::BitWriter;

const BOT: u32 = 1;

fn bytes(b: &[u8]) -> BitWriter {
    BitWriter::from_bytes(b)
}

/// One object create record: kind 0, full precision flags, position (UDP_SESSION_2009_B.md §3.3, §4.2).
fn create_record(obj: u16, pos: [f32; 3]) -> BitWriter {
    let mut rec = BitWriter::new();
    rec.write_u8(0x0B).write_u16(obj).write_u8(0);
    for c in pos {
        rec.write_f32(c);
    }
    rec.write_u16(0x0100).write_u16(0).write_u32(0).write_u32(0);
    for c in pos {
        rec.write_f32(c);
    }
    for _ in 0..3 {
        rec.write_f32(0.0);
    }
    let mut m = BitWriter::new();
    m.write_u8(8).write_u32(rec.bit_len() as u32).append(&rec);
    m
}

struct Out {
    bot: BotSession,
    /// (time, shell) of every client shell message the server saw.
    seen: Vec<(u32, u8)>,
}

/// `arm`: the server sends 0xd7 sub 2 after the death (1000 ms after the world-ready message).
fn run(
    arm: bool,
    respawn_after: Option<u32>,
    idle_ms: u32,
    silent_from: Option<u32>,
    ms: u32,
) -> Out {
    let rec = Recorder::null("Sim");
    let mut cfg = GameConfig::new("SimBot");
    cfg.load_ms = 300;
    cfg.idle_exit_ms = idle_ms;
    cfg.respawn_after_ms = respawn_after;
    let mut bot = BotSession::new(cfg, None, rec);
    let mut host: Host<u32> = Host::new(HostConfig::default());
    let mut up = SimChannel::new(11, Faults::clean(5));
    let mut down = SimChannel::new(12, Faults::clean(5));
    let (mut hello, mut acked, mut world_sent) = (false, false, false);
    let mut seen = Vec::new();
    let mut ready_at: Option<u32> = None;
    let mut died = false;
    let mut respawned = false;
    let mut peer_key = None;
    for now in 0..ms {
        for d in bot.poll(now) {
            up.send(now, d);
        }
        let silent = silent_from.is_some_and(|s| now >= s);
        for d in up.deliver(now) {
            if !silent {
                host.handle_datagram(now, BOT, &d);
            }
        }
        if silent {
            // the server process is gone: nothing is polled, nothing is sent
            let _ = down.deliver(now);
            continue;
        }
        for e in host.poll(now) {
            match e {
                HostEvent::Transmit { bytes, .. } => down.send(now, bytes),
                HostEvent::LinkOpened { peer, .. } => {
                    peer_key = Some(peer);
                    host.send_reliable(
                        &peer,
                        bytes(&[0x04, 0x07, 0, 0, 0, 0x00, 0x20, 0x4E, 0x00]),
                    );
                    host.send_reliable(&peer, bytes(&[0x0C, 0x01, 0x00, 0x00]));
                    let mut fl = bytes(&[0x32, 0x01, 0x00, 0x10, 0x00, 0x00, 0x00]);
                    fl.write_zstr8(b"Worlds\\Sim.dat");
                    host.send_reliable(&peer, fl);
                }
                HostEvent::Message { peer, bits, .. } => {
                    let b = bits.as_bytes().to_vec();
                    let id = b[0];
                    if id == 11 {
                        seen.push((now, b[1]));
                        if b[1] == 0x87 {
                            ready_at = Some(now);
                        }
                        if b[1] == 0x89 && !respawned {
                            respawned = true;
                            host.send_reliable(&peer, bytes(&[0x0D, 0x67, 0x01]));
                        }
                    }
                    match (id, b.get(1).copied()) {
                        (5, _) => hello = true,
                        (0x36, _) => acked = b.len() == 3 && b[1] == 0x01 && b[2] == 0x80,
                        (9, Some(0)) => {
                            host.send_reliable(&peer, bytes(&[0x13, 0x00]));
                            host.send_reliable(&peer, bytes(&[0x13, 0x01]));
                        }
                        (9, Some(1)) => {
                            host.send_reliable(&peer, create_record(5, [10.0, 50.0, -20.0]));
                            host.send_reliable(&peer, bytes(&[0x07, 0x05, 0x00]));
                            host.send_reliable(&peer, bytes(&[0x0D, 0x72]));
                            host.send_reliable(&peer, bytes(&[0x0D, 0x88, 0x02]));
                            host.send_reliable(&peer, bytes(&[0x0D, 0x67, 0x01]));
                        }
                        (6, _) => host.disconnect(now, &peer, dc_net_engine::Bye::Code(5)),
                        _ => {}
                    }
                    if hello && acked && !world_sent {
                        world_sent = true;
                        host.send_reliable(&peer, bytes(&[0x06, 0, 0, 0, 0, 0x01, 0x00]));
                    }
                }
                HostEvent::LinkClosed { .. } => {}
            }
        }
        // the death, 1000 ms after the world-ready message: state 3 then 2 (the order the reference stack used)
        if let (Some(r), Some(peer)) = (ready_at, peer_key.as_ref()) {
            if !died && now >= r + 1000 {
                died = true;
                host.send_reliable(peer, bytes(&[0x0D, 0x67, 0x03]));
                host.send_reliable(peer, bytes(&[0x0D, 0x67, 0x02]));
                if arm {
                    host.send_reliable(peer, bytes(&[0x0D, 0xD7, 0x02, 0x05, 0x05]));
                }
            }
        }
        if world_sent && now % 100 == 0 {
            let mut t = BitWriter::new();
            t.write_u8(10)
                .write_u16(0xFFFF)
                .write_bits(0, 4)
                .write_f32(now as f32 / 1000.0);
            host.send_unreliable(now, &BOT, t);
        }
        for d in down.deliver(now) {
            bot.handle_datagram(now, &d);
        }
    }
    Out { bot, seen }
}

fn times(o: &Out, shell: u8) -> Vec<u32> {
    o.seen
        .iter()
        .filter(|s| s.1 == shell)
        .map(|s| s.0)
        .collect()
}

#[test]
fn death_arm_wait_request_respawn_then_updates_resume() {
    let o = run(true, None, 0, None, 30_000);
    let req = times(&o, 0x89);
    assert_eq!(req.len(), 1, "exactly one respawn request: {req:?}");
    let ready = times(&o, 0x87)[0];
    let upd = times(&o, 0x64);
    let died_at = ready + 1000;
    // the request comes more than 7000 ms after the arming message
    assert!(
        req[0] >= died_at + 7000 && req[0] <= died_at + 7400,
        "request at {} died {}",
        req[0],
        died_at
    );
    // no updates from just after the death until the respawn, updates again afterwards
    let in_dead: Vec<_> = upd
        .iter()
        .filter(|t| **t > died_at + 300 && **t < req[0])
        .collect();
    assert!(in_dead.is_empty(), "updates while dead: {in_dead:?}");
    assert!(
        upd.iter().filter(|t| **t > req[0] + 300).count() >= 10,
        "updates resume"
    );
    assert_eq!(o.bot.obs.life_states, vec![1, 3, 2, 1]);
    assert_eq!(o.bot.stage(), Stage::InWorld);
}

#[test]
fn an_unarmed_death_sends_nothing_without_the_fallback() {
    let o = run(false, None, 0, None, 20_000);
    assert!(times(&o, 0x89).is_empty());
    assert_eq!(o.bot.obs.life_states, vec![1, 3, 2]);
}

#[test]
fn the_fallback_switch_requests_after_its_delay() {
    let o = run(false, Some(2500), 0, None, 20_000);
    let req = times(&o, 0x89);
    let ready = times(&o, 0x87)[0];
    assert_eq!(req.len(), 1, "{req:?}");
    // BOT-168: the first fallback request waits for both the switch's delay (2500 ms) and the class wait
    // (7000 ms; the test room is not class 10), so it comes 7000 ms after the death, not 2500 ms.
    let died_at = ready + 1000;
    assert!(
        req[0] >= died_at + 7000 && req[0] <= died_at + 7400,
        "{req:?} {ready}"
    );
    assert_eq!(o.bot.obs.life_states, vec![1, 3, 2, 1]);
}

#[test]
fn a_silent_server_ends_the_session_after_the_idle_time() {
    // the server vanishes at 6000 ms; the idle time is 15000 ms
    let o = run(false, None, 15_000, Some(6000), 30_000);
    assert_eq!(o.bot.stage(), Stage::Closed, "end: {:?}", o.bot.obs.end);
    assert!(o
        .bot
        .obs
        .end
        .as_deref()
        .unwrap_or("")
        .starts_with("server gone"));
}

#[test]
fn a_silent_server_before_the_idle_time_keeps_the_session() {
    let o = run(false, None, 15_000, Some(6000), 18_000);
    assert_eq!(o.bot.stage(), Stage::InWorld);
}

#[test]
fn the_servers_bye_ends_the_session_cleanly() {
    // the host sends its bye after the bot's first message
    let rec = Recorder::null("Sim");
    let mut cfg = GameConfig::new("SimBot");
    cfg.load_ms = 100;
    let mut bot = BotSession::new(cfg, None, rec);
    let mut host: Host<u32> = Host::new(HostConfig::default());
    let mut up = SimChannel::new(1, Faults::clean(2));
    let mut down = SimChannel::new(2, Faults::clean(2));
    let mut said = false;
    for now in 0..3000 {
        for d in bot.poll(now) {
            up.send(now, d);
        }
        for d in up.deliver(now) {
            host.handle_datagram(now, BOT, &d);
        }
        for e in host.poll(now) {
            match e {
                HostEvent::Transmit { bytes, .. } => down.send(now, bytes),
                HostEvent::Message { peer, .. } if !said => {
                    said = true;
                    host.disconnect(now, &peer, dc_net_engine::Bye::Code(5));
                }
                _ => {}
            }
        }
        for d in down.deliver(now) {
            bot.handle_datagram(now, &d);
        }
    }
    assert_eq!(bot.stage(), Stage::Closed, "end: {:?}", bot.obs.end);
    assert!(
        bot.obs
            .end
            .as_deref()
            .unwrap_or("")
            .starts_with("server bye"),
        "{:?}",
        bot.obs.end
    );
}
