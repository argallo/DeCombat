//! The client's life cycle on dc-botclient's session as dc-client configures it, against a scripted server in
//! simulated time (start-up script as in dc-botclient's lifecycle_sim.rs): the own respawn wait after a death
//! (0x67 state 3 then 2, the reference stack's order), the one 0x89 more than 7000 ms after it, no 0x64 while dead, updates
//! again after 0x67 state 1; and the leave of a running match (0xbd, then `C2S_BYE` one frame later, no 0x64 after
//! the 0xbd). The scripted server is ours; no ROM, no network.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (gate), §3.3 / §3.4 (respawn wait and request), §5.7 (0xbd)
// OBSERVED: CAPTURE_tdm4_KILL_VALIDATION.md §1, §4 (reference 0x67 3 then 2, no arming message; the client's own wait)

use dc_botclient::objects::ExtentBox;
use dc_botclient::record::Recorder;
use dc_botclient::session::{BotSession, Stage};
use dc_client::net::{session_config, Connect, NetOptions};
use dc_client::LocalUpdate;
use dc_net_engine::sim::{Faults, SimChannel};
use dc_net_engine::{Host, HostConfig, HostEvent};
use dc_sim::{MovementParams, PlayerState, V3};
use dc_wire::udp::BitWriter;

const PEER: u32 = 1;

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

/// What the scripted server saw: (time, engine id, shell id or 0, first bytes).
struct Seen {
    msgs: Vec<(u32, u8, u8, Vec<u8>)>,
    client: BotSession,
}

impl Seen {
    fn shell_times(&self, shell: u8) -> Vec<u32> {
        self.msgs
            .iter()
            .filter(|m| m.1 == 11 && m.2 == shell)
            .map(|m| m.0)
            .collect()
    }
    fn engine_times(&self, id: u8) -> Vec<u32> {
        self.msgs
            .iter()
            .filter(|m| m.1 == id)
            .map(|m| m.0)
            .collect()
    }
}

/// Runs the client session for `ms` simulated milliseconds. `mode_class`: what dc-client derives from the room's
/// ROM mode row (no 0x9e is sent by this server). `leave_at`: the client leaves the running match then (as
/// net.rs does: 0xbd, `C2S_BYE` 16 ms later).
fn run(mode_class: Option<u8>, die: bool, leave_at: Option<u32>, ms: u32) -> Seen {
    let opts = NetOptions::new(Connect::Udp("127.0.0.1:1".parse().unwrap()), "SimClient");
    let cfg = session_config(&opts, "SimClient", None, mode_class);
    let mut c = BotSession::new(cfg, None, Recorder::null("SimClient"));
    let mut host: Host<u32> = Host::new(HostConfig::default());
    let mut up = SimChannel::new(21, Faults::clean(5));
    let mut down = SimChannel::new(22, Faults::clean(5));
    let (mut hello, mut acked, mut world_sent) = (false, false, false);
    let mut msgs = Vec::new();
    let mut ready_at: Option<u32> = None;
    let mut died = false;
    let mut respawned = false;
    let mut peer_key = None;
    let mut bye_at: Option<u32> = None;
    let params = MovementParams::default();
    let body = PlayerState::new(
        V3::new(10.0, 50.0, -20.0),
        V3::new(15.0, 50.0, 15.0),
        &params,
    );
    for now in 0..ms {
        if c.world_request().is_some() {
            c.world_loaded(
                now,
                ExtentBox {
                    min: [-1000.0; 3],
                    max: [1000.0; 3],
                },
                "sim",
            );
        }
        // the client loop hands its update every frame (60 Hz)
        if now % 16 == 0 {
            let u = LocalUpdate::from_state(
                &body,
                &dc_client::ViewAngles::default(),
                now as f32 / 1000.0,
            );
            c.set_player_update_ext(u.to_wire(), u.extras());
        }
        if leave_at == Some(now) {
            assert!(c.leave_match(now), "leave_match in a running match");
            bye_at = Some(now + dc_client::net::LEAVE_BYE_DELAY_MS);
        }
        if bye_at == Some(now) {
            c.quit(now);
        }
        for d in c.poll(now) {
            up.send(now, d);
        }
        for d in up.deliver(now) {
            host.handle_datagram(now, PEER, &d);
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
                    let sh = if id == 11 {
                        b.get(1).copied().unwrap_or(0)
                    } else {
                        0
                    };
                    msgs.push((now, id, sh, b.iter().take(6).copied().collect()));
                    if id == 11 && sh == 0x87 {
                        ready_at = Some(now);
                    }
                    if id == 11 && sh == 0x89 && !respawned {
                        // the reference stack's answer to 0x89 (REFERENCE_FINDINGS_botduel F2): state 1 again
                        respawned = true;
                        host.send_reliable(&peer, bytes(&[0x0D, 0x67, 0x01]));
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
        // the death, 1000 ms after the world-ready message: state 3 then 2 in one instant, no arming message
        if let (Some(r), Some(peer)) = (ready_at, peer_key.as_ref()) {
            if die && !died && now >= r + 1000 {
                died = true;
                host.send_reliable(peer, bytes(&[0x0D, 0x67, 0x03]));
                host.send_reliable(peer, bytes(&[0x0D, 0x67, 0x02]));
            }
        }
        if world_sent && now % 100 == 0 {
            let mut t = BitWriter::new();
            t.write_u8(10)
                .write_u16(0xFFFF)
                .write_bits(0, 4)
                .write_f32(now as f32 / 1000.0);
            host.send_unreliable(now, &PEER, t);
        }
        for d in down.deliver(now) {
            c.handle_datagram(now, &d);
        }
    }
    Seen { msgs, client: c }
}

#[test]
fn the_client_asks_for_its_respawn_after_its_own_wait_in_tdm() {
    let o = run(Some(1), true, None, 20_000);
    let ready = o.shell_times(0x87)[0];
    let died_at = ready + 1000;
    let req = o.shell_times(0x89);
    assert_eq!(req.len(), 1, "exactly one respawn request: {req:?}");
    // more than 7000 ms after the death (arrival and processing add a few ms)
    assert!(
        req[0] > died_at + 7000 && req[0] <= died_at + 7200,
        "request at {} died {died_at}",
        req[0]
    );
    let upd = o.shell_times(0x64);
    let in_dead: Vec<_> = upd
        .iter()
        .filter(|t| **t > died_at + 100 && **t < req[0])
        .collect();
    assert!(in_dead.is_empty(), "updates while dead: {in_dead:?}");
    assert!(
        upd.iter().filter(|t| **t > req[0] + 200).count() >= 20,
        "updates resume after state 1"
    );
    assert_eq!(o.client.obs.life_states, vec![1, 3, 2, 1]);
    assert_eq!(o.client.stage(), Stage::InWorld);
    // the reliable state form 0b 64 10 00 01 once, right before 0x87 (the 2009 client's start form)
    let forms: Vec<_> = o
        .msgs
        .iter()
        .filter(|m| m.1 == 11 && m.2 == 0x64 && m.3.get(2..5) == Some(&[0x10, 0x00, 0x01][..]))
        .collect();
    assert_eq!(forms.len(), 1, "{forms:?}");
    assert!(forms[0].0 <= ready);
    // the unreliable updates carry the 2009 client's flag words: 0x1521 first, then 0x1021 (nothing moved)
    let flags: Vec<u16> = o
        .msgs
        .iter()
        .filter(|m| m.1 == 11 && m.2 == 0x64 && m.3.len() >= 4 && m.3[2] != 0x10)
        .map(|m| u16::from_le_bytes([m.3[2], m.3[3]]))
        .collect();
    assert_eq!(flags[0], 0x1521, "{:04x?}", &flags[..4]);
    assert!(
        flags[1..].iter().all(|f| *f == 0x1021),
        "{:04x?}",
        &flags[..8]
    );
}

#[test]
fn without_a_mode_class_the_client_never_asks() {
    // the wiring matters: no mode class (no ROM row, no 0x9e) arms no wait, as for a 2009 client in a class
    // without respawn requests
    let o = run(None, true, None, 12_000);
    assert!(o.shell_times(0x89).is_empty());
    assert_eq!(o.client.obs.life_states, vec![1, 3, 2]);
}

#[test]
fn leaving_a_running_match_sends_0xbd_then_the_bye_and_no_more_updates() {
    let o = run(Some(1), false, Some(1500), 4000);
    let ready = o.shell_times(0x87)[0];
    assert!(ready < 1200, "in the world before the leave: {ready}");
    let leave = o.shell_times(0xbd);
    assert_eq!(leave.len(), 1, "{leave:?}");
    let bye = o.engine_times(6);
    assert_eq!(bye.len(), 1, "one C2S_BYE: {bye:?}");
    assert!(
        bye[0] >= leave[0] + 16,
        "bye {} after leave {}",
        bye[0],
        leave[0]
    );
    assert!(
        o.shell_times(0x64).iter().all(|t| *t <= leave[0]),
        "no 0x64 after 0xbd"
    );
    // the server closed the link after the bye (code 5)
    assert!(o.client.is_over(), "{:?}", o.client.stage());
}
