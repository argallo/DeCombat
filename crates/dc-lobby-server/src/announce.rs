//! Per-peer announced address: what the server list (0x204 / 0x3b1 entries) and the match address (0x3043) tell
//! one connection to reach.
//!
//! A peer on loopback or a private network (RFC 1918: 10/8, 172.16/12, 192.168/16) is told the local address of its
//! own accepted TCP stream, the address it already reached us at: loopback for a client on the hosting machine, the
//! LAN address for a LAN machine. Every other peer is told the configured address (`server.advertise_ip`,
//! `--lobbyip` / `--gameip`), normally the public address the router forwards. A home router then needs no NAT
//! hairpin for the host's own client or the LAN, and a remote friend still gets the public address.
//!
//! A configured address that is itself loopback or private is announced unchanged to everyone: every local peer
//! reaches it already, and a relay setup (the capture tap announcing 127.0.0.1 in front of servers on 127.0.0.2)
//! must keep its announced address. The match address of a substituted peer is the game server's own bind address
//! when that is a single address, the peer's stream address only when the game server binds every address.
// DC-DEFAULT: LG-30 per-peer announced address; SPEC: RFC 1918 §3 private address ranges

use std::net::{IpAddr, SocketAddr};

/// The IPv4 form of `ip` (an IPv4-mapped IPv6 address included).
fn v4(ip: IpAddr) -> Option<std::net::Ipv4Addr> {
    match ip {
        IpAddr::V4(a) => Some(a),
        IpAddr::V6(a) => a.to_ipv4_mapped(),
    }
}

/// True for loopback and the RFC 1918 private ranges.
pub fn is_local_peer(ip: IpAddr) -> bool {
    if ip.is_loopback() {
        return true;
    }
    match v4(ip) {
        Some(a) => a.is_loopback() || a.is_private(),
        None => false,
    }
}

/// True when `peer` ("ip:port" or a bare ip) is a loopback address: a process on the hosting machine.
pub fn is_loopback_peer(peer: &str) -> bool {
    parse_ip(peer)
        .map(|ip| ip.is_loopback() || v4(ip).map(|a| a.is_loopback()).unwrap_or(false))
        .unwrap_or(false)
}

/// LG-317: an address that is not this machine (false for a peer text that is no address).
pub fn is_remote_peer(peer: &str) -> bool {
    parse_ip(peer).is_some() && !is_loopback_peer(peer)
}

/// Parses "1.2.3.4:5" / "[::1]:5" or a bare address.
fn parse_ip(s: &str) -> Option<IpAddr> {
    let s = s.trim();
    s.parse::<SocketAddr>()
        .map(|a| a.ip())
        .ok()
        .or_else(|| s.parse::<IpAddr>().ok())
}

/// The address to announce to a peer whose TCP source is `peer` and whose stream was accepted on `local`
/// (both "ip:port" or a bare ip); `configured` otherwise. An unknown or unspecified local address gives
/// `configured` as well.
pub fn announced_ip(peer: &str, local: &str, configured: &str) -> String {
    if parse_ip(configured).map(is_local_peer).unwrap_or(false) {
        return configured.to_string();
    }
    let (Some(p), Some(l)) = (parse_ip(peer), parse_ip(local)) else {
        return configured.to_string();
    };
    if !is_local_peer(p) || l.is_unspecified() {
        return configured.to_string();
    }
    match v4(l) {
        Some(a) => a.to_string(),
        None => l.to_string(),
    }
}

/// The match address (0x3043) for one peer: [`announced_ip`], except that a substituted address becomes the game
/// server's bind address `bind` unless that is 0.0.0.0 (a server bound to one address is reachable only there).
pub fn announced_game_ip(
    peer: &str,
    local: &str,
    configured: &str,
    bind: std::net::Ipv4Addr,
) -> String {
    let a = announced_ip(peer, local, configured);
    if a == configured || bind.is_unspecified() {
        a
    } else {
        bind.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_peer_gets_its_own_loopback_address() {
        assert_eq!(
            announced_ip("127.0.0.1:50000", "127.0.0.1:10002", "203.0.113.7"),
            "127.0.0.1"
        );
        assert_eq!(
            announced_ip("127.0.0.1:50000", "127.0.0.2:10002", "203.0.113.7"),
            "127.0.0.2"
        );
    }

    #[test]
    fn lan_peer_gets_the_lan_address_it_reached() {
        for peer in [
            "192.168.1.30:4000",
            "10.0.0.9:4000",
            "172.20.1.1:4000",
            "172.31.255.1:1",
        ] {
            assert_eq!(
                announced_ip(peer, "192.168.1.20:10002", "203.0.113.7"),
                "192.168.1.20",
                "{peer}"
            );
        }
        assert_eq!(
            announced_ip(
                "[::ffff:192.168.1.30]:4000",
                "[::ffff:192.168.1.20]:10002",
                "x"
            ),
            "192.168.1.20"
        );
    }

    #[test]
    fn remote_peer_gets_the_configured_address() {
        for peer in [
            "198.51.100.4:6000",
            "172.32.0.1:1",
            "172.15.0.1:1",
            "8.8.8.8:53",
            "100.64.0.1:9",
        ] {
            assert_eq!(
                announced_ip(peer, "192.168.1.20:10002", "203.0.113.7"),
                "203.0.113.7",
                "{peer}"
            );
        }
    }

    #[test]
    fn a_local_configured_address_is_never_replaced() {
        // the capture tap: lobby on 127.0.0.2 announces the tap on 127.0.0.1
        assert_eq!(
            announced_ip("127.0.0.1:5", "127.0.0.2:10002", "127.0.0.1"),
            "127.0.0.1"
        );
        assert_eq!(
            announced_ip("192.168.1.30:5", "127.0.0.1:1", "192.168.1.20"),
            "192.168.1.20"
        );
        // a host name is treated as public
        assert_eq!(
            announced_ip("127.0.0.1:5", "127.0.0.1:1", "play.example.org"),
            "127.0.0.1"
        );
    }

    #[test]
    fn game_address_uses_the_game_server_bind() {
        use std::net::Ipv4Addr;
        let any = Ipv4Addr::UNSPECIFIED;
        let lan = Ipv4Addr::new(192, 168, 1, 20);
        assert_eq!(
            announced_game_ip("127.0.0.1:5", "127.0.0.1:1", "203.0.113.7", any),
            "127.0.0.1"
        );
        assert_eq!(
            announced_game_ip("127.0.0.1:5", "127.0.0.1:1", "203.0.113.7", lan),
            "192.168.1.20"
        );
        assert_eq!(
            announced_game_ip("8.8.8.8:5", "192.168.1.20:1", "203.0.113.7", lan),
            "203.0.113.7"
        );
        assert_eq!(
            announced_game_ip(
                "127.0.0.1:5",
                "127.0.0.2:1",
                "127.0.0.1",
                Ipv4Addr::new(127, 0, 0, 2)
            ),
            "127.0.0.1"
        );
    }

    #[test]
    fn unknown_addresses_fall_back_to_configured() {
        assert_eq!(announced_ip("test", "test", "1.2.3.4"), "1.2.3.4");
        assert_eq!(announced_ip("127.0.0.1:1", "", "1.2.3.4"), "1.2.3.4");
        assert_eq!(
            announced_ip("127.0.0.1:1", "0.0.0.0:10002", "1.2.3.4"),
            "1.2.3.4"
        );
    }
}
