//! The bot's game session against a scripted server built here from the spec byte strings (UDP_SESSION_2009_A.md
//! §9.2 S2-S12, GAME_MESSAGES_2009_S2C.md §4.1 / §5.1), over dc-net-engine's simulated channel: clean and lossy.
//! No sockets, no processes; time is simulated.

use std::collections::BTreeMap;

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

#[derive(Default)]
struct Script {
    hello: bool,
    acked: bool,
    world_sent: bool,
    seen: Vec<(u8, Option<u8>, bool)>,
    shell_counts: BTreeMap<u8, u32>,
    send_player_state: bool,
}

/// Runs the scripted server and the bot for `ms` simulated milliseconds over the given channel faults.
fn run(faults: Faults, ms: u32, send_player_state: bool) -> (BotSession, Script) {
    let rec = Recorder::null("Sim");
    let mut cfg = GameConfig::new("SimBot");
    cfg.load_ms = 300;
    let mut bot = BotSession::new(cfg, None, rec);
    let mut host: Host<u32> = Host::new(HostConfig::default());
    let mut up = SimChannel::new(11, faults);
    let mut down = SimChannel::new(12, faults);
    let mut sc = Script {
        send_player_state,
        ..Script::default()
    };
    for now in 0..ms {
        if now == ms - 3000 {
            bot.quit(now);
        }
        for d in bot.poll(now) {
            up.send(now, d);
        }
        for d in up.deliver(now) {
            host.handle_datagram(now, BOT, &d);
        }
        for e in host.poll(now) {
            match e {
                HostEvent::Transmit { bytes, .. } => down.send(now, bytes),
                HostEvent::LinkOpened { peer, .. } => {
                    // SPEC: UDP_SESSION_2009_A.md §9.2 S2, S3, S4
                    host.send_reliable(
                        &peer,
                        bytes(&[0x04, 0x07, 0, 0, 0, 0x00, 0x20, 0x4E, 0x00]),
                    );
                    host.send_reliable(&peer, bytes(&[0x0C, 0x01, 0x00, 0x00]));
                    let mut fl = bytes(&[0x32, 0x01, 0x00, 0x10, 0x00, 0x00, 0x00]);
                    fl.write_zstr8(b"Worlds\\Sim.dat");
                    host.send_reliable(&peer, fl);
                }
                HostEvent::Message {
                    peer,
                    reliable,
                    bits,
                } => {
                    let b = bits.as_bytes().to_vec();
                    let id = b[0];
                    let sh = (id == 11).then(|| b[1]);
                    sc.seen.push((id, sh, reliable));
                    if let Some(s) = sh {
                        *sc.shell_counts.entry(s).or_default() += 1;
                    }
                    match (id, b.get(1).copied()) {
                        (5, _) => sc.hello = true,
                        (0x36, _) => sc.acked = b.len() == 3 && b[1] == 0x01 && b[2] == 0x80,
                        (9, Some(0)) => {
                            // SPEC: UDP_SESSION_2009_A.md §9.2 S7
                            host.send_reliable(&peer, bytes(&[0x13, 0x00]));
                            host.send_reliable(&peer, bytes(&[0x13, 0x01]));
                        }
                        (9, Some(1)) => {
                            // SPEC: UDP_SESSION_2009_A.md §9.2 S9-S11; addendum A17 (S12 = shell 0x67 state 1)
                            host.send_reliable(&peer, create_record(5, [10.0, 50.0, -20.0]));
                            host.send_reliable(&peer, bytes(&[0x07, 0x05, 0x00]));
                            host.send_reliable(&peer, bytes(&[0x0D, 0x72]));
                            host.send_reliable(&peer, bytes(&[0x0D, 0x88, 0x02]));
                            if sc.send_player_state {
                                host.send_reliable(&peer, bytes(&[0x0D, 0x67, 0x01]));
                            }
                        }
                        (6, _) => host.disconnect(now, &peer, dc_net_engine::Bye::Code(5)),
                        _ => {}
                    }
                    if sc.hello && sc.acked && !sc.world_sent {
                        sc.world_sent = true;
                        // SPEC: UDP_SESSION_2009_A.md §9.2 S6 (`06`, srv_clock 0.0, W)
                        host.send_reliable(&peer, bytes(&[0x06, 0, 0, 0, 0, 0x01, 0x00]));
                    }
                }
                HostEvent::LinkClosed { .. } => {}
            }
        }
        // time stream every 100 ms once the world was sent
        if sc.world_sent && now % 100 == 0 {
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
    (bot, sc)
}

fn check_full_session(bot: &BotSession, sc: &Script) {
    let o = &bot.obs;
    for m in [
        "join_ask",
        "joined",
        "S1_hello_sent",
        "S2_version",
        "S3_client_id",
        "S4_file_list",
        "S4a_file_ack_sent",
        "S6_load_world",
        "S6a_world_loaded_sent",
        "S7_preload_end",
        "S8a_preload_done_sent",
        "S9_own_object",
        "S10_view_object",
        "S11_switch_state",
        "S12_player_state",
        "info_request",
        "info_sent",
        "world_ready_sent",
        "rate_sent",
        "spawned",
        "first_update_sent",
        "S13_first_time_entry",
    ] {
        assert!(
            o.reached(m),
            "milestone {m} missing; have {:?}",
            o.milestones
        );
    }
    assert!(o.fatal.is_empty(), "{:?}", o.fatal);
    assert_eq!(o.version, Some((7, 5_120_000)));
    assert_eq!(o.client_id, Some((1, 0)));
    assert_eq!(
        o.files.get(&1).map(|f| f.1.as_str()),
        Some("Worlds\\Sim.dat")
    );
    // hello is the first message, reliable
    assert_eq!(sc.seen.first(), Some(&(5, None, true)));
    // the server got: player info + client options + world ready, all reliable; updates unreliable
    assert_eq!(sc.shell_counts.get(&0x72), Some(&1));
    assert_eq!(sc.shell_counts.get(&0x85), Some(&1));
    assert_eq!(sc.shell_counts.get(&0x87), Some(&1));
    assert!(sc.seen.iter().filter(|s| s.1 == Some(0x64)).all(|s| !s.2));
    let updates = sc.shell_counts.get(&0x64).copied().unwrap_or(0);
    assert!(updates >= 10, "updates {updates}");
    // 0x87 only after the player state; rate report once
    assert!(sc.seen.iter().any(|s| s.0 == 7));
    assert!(sc.seen.iter().any(|s| s.0 == 6), "C2S_BYE sent");
    // the body stood at its spawn (flat floor under the create position)
    let p = bot.body().expect("body").pos();
    assert!(
        (p[0] - 10.0).abs() < 0.01 && (p[2] + 20.0).abs() < 0.01,
        "{p:?}"
    );
    assert!(bot.table.server_time.unwrap_or(0.0) > 1.0);
}

#[test]
fn full_start_up_on_a_clean_channel() {
    let (bot, sc) = run(Faults::clean(5), 12_000, true);
    check_full_session(&bot, &sc);
    assert_eq!(bot.stage(), Stage::Closed);
    let s = bot.link_stats().expect("stats");
    assert_eq!(s.bad_check, 0);
    assert!(s.resends <= 2, "resends {}", s.resends);
}

#[test]
fn full_start_up_on_a_lossy_reordering_channel() {
    let f = Faults {
        loss_permille: 150,
        dup_permille: 50,
        corrupt_permille: 20,
        min_delay_ms: 5,
        max_delay_ms: 60,
    };
    let (bot, sc) = run(f, 20_000, true);
    check_full_session(&bot, &sc);
    let s = bot.link_stats().expect("stats");
    assert!(s.resends > 0, "a lossy channel needs resends");
}

#[test]
fn without_player_state_the_client_stays_loading_and_sends_no_update() {
    let (bot, sc) = run(Faults::clean(5), 8_000, false);
    let o = &bot.obs;
    assert!(o.reached("S11_switch_state") && o.reached("engine_in_world"));
    assert!(!o.reached("world_ready_sent"));
    assert_eq!(sc.shell_counts.get(&0x64), None);
    assert_eq!(sc.shell_counts.get(&0x87), None);
    // the identify request is answered even before the player state
    assert_eq!(sc.shell_counts.get(&0x72), Some(&1));
}
