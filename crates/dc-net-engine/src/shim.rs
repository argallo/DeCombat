//! A client UDP socket with an optional [`Impairer`] around it (the network lab, docs/NETLAB.md): in-process, no
//! administrator rights or operating system tools, the same on Windows, Linux and macOS.
//!
//! The shim owns the socket and its [`SocketReader`]. Outgoing datagrams wait in a queue until the path delivers them
//! to the network; incoming ones wait until the path delivers them to the caller. A NAT rebind (scheduled, an expired
//! mapping, a restarted flow) closes the socket and opens a new one on a new port: the server then sees the client
//! come from a new address, and whatever it still sends to the old port is lost, as on a real NAT. Without a spec
//! (or with a clean one) datagrams go straight through, so a caller can use the shim always.
// DC-DEFAULT: NET-20 (proposed) network lab impairment shim, see DC_DEFAULTS.md

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::impair::{DirStats, Impairer, PathSpec, PathStats};
use crate::socket_reader::{Received, SocketReader};
use crate::time::{later, Millis};

type Queued = Reverse<(Millis, u64, Vec<u8>, SocketAddr)>;

/// Socket-level counters of the shim (what really went over the operating system's socket).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShimIo {
    pub sent: u64,
    pub send_errors: u64,
    pub received: u64,
}

/// The client socket of the network lab.
pub struct NetShim {
    bind: IpAddr,
    sock: UdpSocket,
    reader: SocketReader,
    buf_bytes: usize,
    imp: Option<Impairer>,
    start: Instant,
    out_q: BinaryHeap<Queued>,
    in_q: BinaryHeap<Queued>,
    order: u64,
    io: ShimIo,
    last_err: Option<String>,
    /// Local ports used so far (the first one included), for the report.
    ports: Vec<u16>,
}

impl NetShim {
    /// Binds a socket on `bind` (port 0) and reads it with a [`SocketReader`] of `buf_bytes`.
    pub fn bind(bind: IpAddr, buf_bytes: usize, spec: Option<PathSpec>) -> io::Result<NetShim> {
        let sock = UdpSocket::bind(SocketAddr::new(bind, 0))?;
        let reader = SocketReader::start(&sock, buf_bytes)?;
        let port = sock.local_addr()?.port();
        Ok(NetShim {
            bind,
            sock,
            reader,
            buf_bytes,
            imp: spec.filter(|s| !s.is_clean()).map(Impairer::new),
            start: Instant::now(),
            out_q: BinaryHeap::new(),
            in_q: BinaryHeap::new(),
            order: 0,
            io: ShimIo::default(),
            last_err: None,
            ports: vec![port],
        })
    }

    fn now(&self) -> Millis {
        self.start.elapsed().as_millis() as Millis
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// A clone of the current socket (for a last-gasp sender; it goes stale at a rebind).
    pub fn try_clone_socket(&self) -> io::Result<UdpSocket> {
        self.sock.try_clone()
    }

    pub fn impairer(&self) -> Option<&Impairer> {
        self.imp.as_ref()
    }

    pub fn io(&self) -> ShimIo {
        self.io
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_err.as_deref()
    }

    pub fn ports(&self) -> &[u16] {
        &self.ports
    }

    /// Connection-reset notices the socket reported (an earlier send hit a closed port).
    pub fn resets(&self) -> u64 {
        self.reader.resets()
    }

    /// Queues `bytes` to `to` (sent at once without an impairment).
    pub fn send_to(&mut self, bytes: &[u8], to: SocketAddr) {
        let now = self.now();
        let Some(imp) = self.imp.as_mut() else {
            self.raw_send(bytes, to);
            return;
        };
        let times = imp.outbound(now, bytes.len());
        self.check_rebind();
        for t in times {
            self.order += 1;
            self.out_q
                .push(Reverse((t, self.order, bytes.to_vec(), to)));
        }
        self.flush_out(now);
    }

    fn raw_send(&mut self, bytes: &[u8], to: SocketAddr) {
        match self.sock.send_to(bytes, to) {
            Ok(_) => self.io.sent += 1,
            Err(e) => {
                self.io.send_errors += 1;
                self.last_err = Some(e.to_string());
            }
        }
    }

    fn flush_out(&mut self, now: Millis) {
        while let Some(Reverse((t, _, _, _))) = self.out_q.peek() {
            if later(*t, now) {
                break;
            }
            let Some(Reverse((_, _, d, to))) = self.out_q.pop() else {
                break;
            };
            self.raw_send(&d, to);
        }
    }

    fn check_rebind(&mut self) {
        let due = self.imp.as_mut().is_some_and(Impairer::take_rebind);
        if !due {
            return;
        }
        match UdpSocket::bind(SocketAddr::new(self.bind, 0))
            .and_then(|s| SocketReader::start(&s, self.buf_bytes).map(|r| (s, r)))
        {
            Ok((s, r)) => {
                // what is still in the old socket arrived for the old mapping: lost
                while let Some(x) = self.reader.try_next() {
                    if x.is_ok() {
                        if let Some(imp) = self.imp.as_mut() {
                            imp.note_stale_port();
                        }
                    }
                }
                self.reader = r;
                self.sock = s;
                if let Ok(a) = self.sock.local_addr() {
                    self.ports.push(a.port());
                }
            }
            Err(e) => self.last_err = Some(format!("rebind: {e}")),
        }
    }

    fn ingest(&mut self, now: Millis, d: Vec<u8>, from: SocketAddr) {
        self.io.received += 1;
        match self.imp.as_mut() {
            None => {
                self.order += 1;
                self.in_q.push(Reverse((now, self.order, d, from)));
            }
            Some(imp) => {
                let times = imp.inbound(now, d.len());
                for t in times {
                    self.order += 1;
                    self.in_q.push(Reverse((t, self.order, d.clone(), from)));
                }
            }
        }
    }

    fn pop_due_in(&mut self, now: Millis) -> Option<(Vec<u8>, SocketAddr)> {
        let Reverse((t, _, _, _)) = self.in_q.peek()?;
        if later(*t, now) {
            return None;
        }
        self.in_q.pop().map(|Reverse((_, _, d, from))| (d, from))
    }

    /// The next datagram the path delivers, waiting at most `wait` (`None` = nothing came). Also sends what the
    /// path has delivered to the network meanwhile and moves the socket when a rebind is due.
    pub fn recv_within(&mut self, wait: Duration) -> Option<Received> {
        let deadline = Instant::now() + wait;
        loop {
            let now = self.now();
            if let Some(imp) = self.imp.as_mut() {
                imp.tick(now);
            }
            self.check_rebind();
            self.flush_out(now);
            if let Some(x) = self.pop_due_in(now) {
                return Some(Ok(x));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let got = if left.is_zero() {
                self.reader.try_next()
            } else {
                // wake up for the next queued datagram in either direction
                let next = [self.in_q.peek(), self.out_q.peek()]
                    .into_iter()
                    .flatten()
                    .map(|Reverse((t, ..))| *t)
                    .min_by(|a, b| {
                        if later(*a, *b) {
                            std::cmp::Ordering::Greater
                        } else {
                            std::cmp::Ordering::Less
                        }
                    });
                let cap = next
                    .map(|t| Duration::from_millis(t.wrapping_sub(now).min(1000) as u64 + 1))
                    .unwrap_or(left);
                self.reader.next_within(left.min(cap))
            };
            match got {
                Some(Ok((d, from))) => {
                    let now = self.now();
                    self.ingest(now, d, from);
                }
                Some(Err(e)) => return Some(Err(e)),
                None => {
                    if left.is_zero() {
                        let now = self.now();
                        self.flush_out(now);
                        return self.pop_due_in(now).map(Ok);
                    }
                }
            }
        }
    }

    /// The next datagram that is due now, without waiting.
    pub fn try_recv(&mut self) -> Option<Received> {
        self.recv_within(Duration::ZERO)
    }

    /// One report line: profile, per-direction drops, rebinds, ports.
    pub fn summary(&self) -> Option<(String, DirStats, DirStats, PathStats)> {
        self.imp.as_ref().map(|i| {
            (
                i.spec().name.clone(),
                i.up_stats(),
                i.down_stats(),
                i.path_stats(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn echo_server() -> (UdpSocket, SocketAddr) {
        let s = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let a = s.local_addr().unwrap();
        (s, a)
    }

    #[test]
    fn clean_shim_passes_datagrams_straight_through() {
        let (srv, addr) = echo_server();
        let mut c = NetShim::bind(IpAddr::V4(Ipv4Addr::LOCALHOST), 2048, None).unwrap();
        c.send_to(b"hello", addr);
        let mut buf = [0u8; 64];
        let (n, from) = srv.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello");
        srv.send_to(b"back", from).unwrap();
        let (d, f) = c.recv_within(Duration::from_secs(5)).unwrap().unwrap();
        assert_eq!((d.as_slice(), f), (&b"back"[..], addr));
        assert!(c.summary().is_none());
    }

    #[test]
    fn delayed_path_holds_datagrams_and_rebind_moves_the_port() {
        let (srv, addr) = echo_server();
        srv.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let spec = PathSpec::parse("latency=60,rebind=0.3").unwrap();
        let mut c = NetShim::bind(IpAddr::V4(Ipv4Addr::LOCALHOST), 2048, Some(spec)).unwrap();
        let t = Instant::now();
        c.send_to(b"a", addr);
        // the datagram leaves only when the caller polls after its delivery time
        assert!(c.recv_within(Duration::from_millis(100)).is_none());
        let mut buf = [0u8; 64];
        let (_, from1) = srv.recv_from(&mut buf).unwrap();
        assert!(t.elapsed() >= Duration::from_millis(60));
        srv.send_to(b"b", from1).unwrap();
        let (d, _) = c.recv_within(Duration::from_secs(2)).unwrap().unwrap();
        assert_eq!(d, b"b");
        assert!(t.elapsed() >= Duration::from_millis(120));
        // after the rebind the client sends from a new port
        while t.elapsed() < Duration::from_millis(400) {
            let _ = c.recv_within(Duration::from_millis(20));
        }
        c.send_to(b"c", addr);
        let _ = c.recv_within(Duration::from_millis(100));
        let (_, from2) = srv.recv_from(&mut buf).unwrap();
        assert_ne!(from1.port(), from2.port());
        assert_eq!(c.ports().len(), 2);
        assert_eq!(c.summary().unwrap().3.rebinds, 1);
    }
}
