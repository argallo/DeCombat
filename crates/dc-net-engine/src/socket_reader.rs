//! A receive thread for a UDP socket: the one piece of I/O in this crate.
//!
//! Why it exists: on Windows a blocking `recv_from` with a socket receive timeout (`SO_RCVTIMEO`, what
//! `UdpSocket::set_read_timeout` sets) can lose a datagram that arrives while the timeout expires. On an idle machine
//! this is invisible. On a machine at full CPU load it is not: measured with a plain two-socket loopback test (10
//! clients asking every 300 ms, a server replying) 9 of 60 clients saw their first reply late or not at all with the
//! receive timeout, none of 60 with a `select`-style timeout, and with the game server and the bots (both used
//! `set_read_timeout(2 ms)` in their main loops) a bot regularly needed 5 to 12 join asks, sometimes none of 10 was
//! answered, and a running link went silent for 15 s. A receive that never has a timeout does not lose datagrams, so
//! the socket is read by a thread that blocks without one and hands the datagrams to the caller's loop through a
//! channel, where waiting with a timeout is safe.
//!
//! The caller must not set a read timeout on the socket (the reader uses a clone of the same socket).

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

/// One received datagram, or the error `recv_from` returned (other than the connection-reset notice).
pub type Received = io::Result<(Vec<u8>, SocketAddr)>;

/// Marks the reader thread as gone when it unwinds or returns.
struct Leave(Arc<AtomicBool>);

impl Drop for Leave {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// The receive thread of one socket. Dropping it stops the thread.
pub struct SocketReader {
    rx: Receiver<Received>,
    stop: Arc<AtomicBool>,
    resets: Arc<AtomicU64>,
    /// Set by the thread when it leaves.
    exited: Arc<AtomicBool>,
    wake: SocketAddr,
}

impl SocketReader {
    /// Starts reading `sock` (datagrams longer than `buf_bytes` are cut by the OS and reported as an error).
    // SPEC: UDP_TRANSPORT_2009.md §4 (WSAECONNRESET from an ICMP port-unreachable is ignored)
    pub fn start(sock: &UdpSocket, buf_bytes: usize) -> io::Result<SocketReader> {
        let reader_sock = sock.try_clone()?;
        let local = sock.local_addr()?;
        let wake = SocketAddr::new(
            if local.ip().is_unspecified() {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            } else {
                local.ip()
            },
            local.port(),
        );
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let resets = Arc::new(AtomicU64::new(0));
        let exited = Arc::new(AtomicBool::new(false));
        let (stop2, resets2, exited2) = (stop.clone(), resets.clone(), exited.clone());
        std::thread::Builder::new()
            .name("udp-reader".into())
            .spawn(move || {
                let mut buf = vec![0u8; buf_bytes.max(1)];
                let _leave = Leave(exited2);
                loop {
                    let got = reader_sock.recv_from(&mut buf);
                    if stop2.load(Ordering::Relaxed) {
                        return;
                    }
                    let item = match got {
                        Ok((n, from)) => Ok((buf[..n].to_vec(), from)),
                        Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {
                            resets2.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        Err(e) => {
                            // an oversized datagram or a transient error: report it and keep reading
                            std::thread::sleep(Duration::from_millis(1));
                            Err(e)
                        }
                    };
                    if tx.send(item).is_err() {
                        return;
                    }
                }
            })?;
        Ok(SocketReader {
            rx,
            stop,
            resets,
            exited,
            wake,
        })
    }

    /// The next datagram, waiting at most `wait` (`None` = nothing came).
    pub fn next_within(&self, wait: Duration) -> Option<Received> {
        self.rx.recv_timeout(wait).ok()
    }

    /// The next datagram that is already waiting (`None` = none).
    pub fn try_next(&self) -> Option<Received> {
        self.rx.try_recv().ok()
    }

    /// How many connection-reset notices (an earlier send hit a closed port) the socket reported.
    pub fn resets(&self) -> u64 {
        self.resets.load(Ordering::Relaxed)
    }
}

impl Drop for SocketReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // wake the thread out of its blocking receive with a datagram to the socket itself
        if let Ok(s) = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)) {
            let _ = s.send_to(&[0], self.wake);
        }
        // give the thread a moment to leave (it does as soon as the wake-up datagram arrives)
        for _ in 0..100 {
            if self.exited.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn delivers_every_datagram_in_order_and_times_out_when_idle() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let reader = SocketReader::start(&server, 2048).unwrap();
        let t = Instant::now();
        assert!(reader.next_within(Duration::from_millis(20)).is_none());
        assert!(t.elapsed() >= Duration::from_millis(15));
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        for i in 0..300u32 {
            client.send_to(&i.to_be_bytes(), addr).unwrap();
        }
        for i in 0..300u32 {
            let (d, from) = reader
                .next_within(Duration::from_secs(5))
                .expect("datagram")
                .expect("no error");
            assert_eq!(d, i.to_be_bytes());
            assert_eq!(from, client.local_addr().unwrap());
        }
        assert!(reader.try_next().is_none());
    }

    #[test]
    fn dropping_the_reader_frees_the_socket_thread() {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let reader = SocketReader::start(&sock, 64).unwrap();
        let exited = reader.exited.clone();
        drop(reader);
        assert!(
            exited.load(Ordering::Relaxed),
            "the reader thread did not leave"
        );
    }
}
