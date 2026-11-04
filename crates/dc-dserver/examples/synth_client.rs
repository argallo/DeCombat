//! Runs the synthetic client against a running game server over UDP (loopback by default) and prints what it saw.
//!
//! Usage: `cargo run -p dc-dserver --example synth_client -- <host:port> [name] [seconds]`
//! The server must know the name (a `user` line from the lobby, or `-open`). Our own test tooling.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig};
use dc_net_engine::socket_reader::SocketReader;

fn main() {
    let mut args = std::env::args().skip(1);
    let server: SocketAddr = match args.next().and_then(|a| a.parse().ok()) {
        Some(a) => a,
        None => {
            eprintln!("usage: synth_client <host:port> [name] [seconds]");
            std::process::exit(64);
        }
    };
    let name = args.next().unwrap_or_else(|| "Synth".into());
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);
    let sock = UdpSocket::bind("0.0.0.0:0").expect("bind");
    let rd = SocketReader::start(&sock, 8192).expect("reader");
    let mut client = SynthClient::new(SynthConfig::named(&name));
    let clock = Instant::now();
    let end = clock + Duration::from_secs(secs);
    let mut last = client.state();
    while Instant::now() < end {
        let now = clock.elapsed().as_millis() as u32;
        for d in client.poll(now) {
            let _ = sock.send_to(&d, server);
        }
        while let Some(Ok((d, from))) = rd.next_within(Duration::from_millis(2)) {
            if from == server {
                client.handle_datagram(clock.elapsed().as_millis() as u32, &d);
            }
        }
        if client.state() != last {
            last = client.state();
            println!("{:>6} ms  state {:?}", now, last);
            if matches!(last, ClientState::Failed(_) | ClientState::Dropped(_)) {
                break;
            }
        }
    }
    if client.state() == ClientState::InWorld {
        client.quit();
        let stop = Instant::now() + Duration::from_secs(2);
        while Instant::now() < stop && client.state() == ClientState::InWorld {
            let now = clock.elapsed().as_millis() as u32;
            for d in client.poll(now) {
                let _ = sock.send_to(&d, server);
            }
            while let Some(Ok((d, from))) = rd.next_within(Duration::from_millis(2)) {
                if from == server {
                    client.handle_datagram(clock.elapsed().as_millis() as u32, &d);
                }
            }
        }
    }
    let s = &client.seen;
    println!(
        "version {:?}, client id {:?}, files {}",
        s.version,
        s.client_id,
        s.files.len()
    );
    println!(
        "load world {:?}, preload {:?}, view object {:?}, switch state {:?}",
        s.load_world, s.preload_subs, s.view_object, s.switch_state
    );
    println!(
        "time entries {}, updates sent {}, errors {:?}, final state {:?}",
        s.server_times.len(),
        s.updates_sent,
        s.errors,
        client.state()
    );
}
