//! `dc-host`: the remote game host agent of specs/HOST_AGENT.md. It dials OUT to the lobby (TLS, the lobby pinned by
//! its certificate fingerprint), says hello with its name and token, and runs one `dc-dserver` per `start` the lobby
//! sends, multiplexing each game server's stdin / stdout pipe lines (DSERVER_PIPE.md) over the one control stream.
//!
//! - [`config`]: `host.toml` (§2).
//! - [`proto`]: the control lines (§3): parser of the lobby's lines, writer of the agent's, the line splitter with
//!   the 16 KiB limit, the `start` request and its argument filter (§5).
//! - [`ports`]: the UDP port picker (§6).
//! - [`tls`]: the pinned TLS client (§5).
//! - [`games`]: the game server processes (spawn, pipes, quit grace, exit).
//! - [`packs`]: a featured entry's map packs on this machine (the `pack` lines, the check, the unpacked folders).
//! - [`update`]: a server build the lobby offers (the `update` lines), left for `tools/host/update.sh`.
//! - [`link`]: one control connection and the reconnect loop (§4).
//! - [`log`]: timestamped lines on stderr (the systemd journal keeps them).
// Provenance: ORIGINAL (our own design; nothing of the 2009 host protocol is known)

pub mod config;
pub mod games;
pub mod link;
pub mod log;
pub mod packs;
pub mod ports;
pub mod proto;
pub mod tls;
pub mod update;

/// The git short sha this agent was built from (`DC_GIT_COMMIT` at build time, else git), or "unknown".
pub fn version() -> &'static str {
    let v = env!("DC_HOST_GIT_SHA");
    if v.is_empty() || v.chars().any(|c| c.is_whitespace() || c.is_control()) {
        "unknown"
    } else {
        v
    }
}
