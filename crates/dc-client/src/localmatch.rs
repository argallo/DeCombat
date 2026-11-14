//! `dc-client --local-match <spec.json>`: the client runs a match on this PC. It reads a match description
//! (`dc_localhost::LocalMatch`), starts the game server and the bots as its own children through the supervisor
//! (`dc_localhost::supervise`), and joins its own server on loopback with the player's seat ticket. The children end
//! with the client: the supervisor stops them when the session ends, and they end on their own when the client's
//! process goes away (their stdin pipes close; on Windows a job object kills them as well).
// DC-DEFAULT: LOC-10 (proposed)

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use dc_localhost::supervise::{Background, LocalHost, Options, Programs};
use dc_localhost::{LocalMatch, World};

use crate::net::{Connect, NetOptions, WorldFile};

/// Reads a match description from a JSON file.
pub fn read_spec(path: &Path) -> Result<LocalMatch, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("--local-match {}: {e}", path.display()))?;
    let m: LocalMatch = serde_json::from_str(&text)
        .map_err(|e| format!("--local-match {}: {e}", path.display()))?;
    m.validate()
        .map_err(|e| format!("--local-match {}: {e}", path.display()))?;
    Ok(m)
}

/// A local match the client started: where to join, as whom, and the running supervisor (dropping it ends the
/// match).
pub struct Started {
    pub server: SocketAddr,
    pub name: String,
    pub ticket: dc_wire::seatticket::Hidden,
    /// The world file the server runs, for the client's own world load (`None`: a ROM world).
    pub world_file: Option<PathBuf>,
    pub supervisor: Background,
}

/// Starts the match: the server (until `@ready`), the room and seats; the bots start once the player joined.
pub fn start(m: &LocalMatch, programs: &Programs, options: Options) -> Result<Started, String> {
    let host = LocalHost::start(m, programs, options).map_err(|e| format!("local match: {e}"))?;
    let server: SocketAddr = format!("127.0.0.1:{}", host.port())
        .parse()
        .map_err(|e| format!("local match: {e}"))?;
    let ticket = host.player_ticket();
    let world_file = match &m.world {
        World::File { path, .. } => Some(path.clone()),
        World::Rom { .. } => None,
    };
    Ok(Started {
        server,
        name: m.player.name.clone(),
        ticket,
        world_file,
        supervisor: host
            .run_in_background(Some(dc_localhost::PLAYER_UID))
            .map_err(|e| format!("local match: {e}"))?,
    })
}

/// The supervisor's settings for the client: logs in `<profile>/logs/local-match`, the client's ROM for the bots.
pub fn options(profile: &Path, rom: Option<&Path>) -> Options {
    let mut bot_args = Vec::new();
    if let Some(r) = rom {
        bot_args.push("--rom".into());
        bot_args.push(r.display().to_string());
    }
    Options {
        log_dir: Some(profile.join("logs").join("local-match")),
        bot_args,
        ..Options::default()
    }
}

/// The network options of the join: a direct join of `s.server` as the player, with the player's ticket; the
/// match's world file when the server runs one and no other was given.
pub fn net_options(s: &Started, mut o: NetOptions) -> Result<NetOptions, String> {
    o.connect = Connect::Udp(s.server);
    o.name = s.name.clone();
    o.seat_ticket = Some(s.ticket);
    if o.world_file.is_none() {
        if let Some(p) = &s.world_file {
            o.world_file = Some(WorldFile::from_path(p).map_err(|e| e.to_string())?);
        }
    }
    Ok(o)
}

/// The supervisor's programs: `dc-dserver` and `dc-botclient` beside the client's own program.
pub fn programs() -> Result<Programs, String> {
    let me = std::env::current_exe().map_err(|e| format!("local match: {e}"))?;
    let dir = me
        .parent()
        .ok_or("local match: the client's folder is unknown")?;
    Ok(Programs::beside(dir))
}
