//! The UDP port picker (HOST_AGENT.md §6): the first port of the range that is not used by one of the agent's own
//! game servers and that a UDP bind on `bind_ip` gets; a requested port (non-zero, inside the range) is tried first.

use std::net::{IpAddr, SocketAddr, UdpSocket};

/// Picks a port of `lo..=hi`. `requested` 0 = no wish; a wish outside the range is not taken (the router forwards
/// only the range). `ours` are the ports of running game servers; `free` is the bind probe.
pub fn pick(
    lo: u16,
    hi: u16,
    requested: u16,
    ours: &[u16],
    free: &dyn Fn(u16) -> bool,
) -> Option<u16> {
    let usable = |p: u16| p >= lo && p <= hi && !ours.contains(&p) && free(p);
    if requested != 0 && usable(requested) {
        return Some(requested);
    }
    (lo..=hi).find(|&p| p != requested && usable(p))
}

/// True when a UDP bind of `ip:port` succeeds now (the socket is closed again at once).
pub fn udp_free(ip: IpAddr, port: u16) -> bool {
    UdpSocket::bind(SocketAddr::new(ip, port)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_free_port_not_ours() {
        let all = |_| true;
        assert_eq!(pick(100, 105, 0, &[], &all), Some(100));
        assert_eq!(pick(100, 105, 0, &[100, 101], &all), Some(102));
        // a port somebody else holds is skipped
        assert_eq!(pick(100, 105, 0, &[100], &|p| p != 101), Some(102));
        // nothing left
        assert_eq!(pick(100, 101, 0, &[100], &|p| p != 101), None);
        assert_eq!(pick(100, 100, 0, &[], &|_| false), None);
    }

    #[test]
    fn a_requested_port_goes_first_when_usable() {
        let all = |_| true;
        assert_eq!(pick(100, 105, 104, &[], &all), Some(104));
        // taken by one of ours, held by another, or outside the range: the normal order
        assert_eq!(pick(100, 105, 104, &[104], &all), Some(100));
        assert_eq!(pick(100, 105, 104, &[], &|p| p != 104), Some(100));
        assert_eq!(pick(100, 105, 99, &[], &all), Some(100));
        assert_eq!(pick(100, 105, 106, &[100], &all), Some(101));
    }

    #[test]
    fn the_bind_probe_sees_a_held_port() {
        let ip = IpAddr::from([127, 0, 0, 1]);
        let held = UdpSocket::bind((ip, 0)).unwrap();
        let p = held.local_addr().unwrap().port();
        assert!(!udp_free(ip, p));
        assert_eq!(pick(p, p, 0, &[], &|q| udp_free(ip, q)), None);
        drop(held);
        assert!(udp_free(ip, p));
    }
}
