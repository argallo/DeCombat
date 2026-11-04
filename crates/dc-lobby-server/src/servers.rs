//! The server list's entries, the channel block each entry owns, and the game host of a channel.
// DC-DEFAULT: CD-21 (amended 2026-10-31 for HOST_AGENT.md) server entries from `lobby.servers`, one channel block each
// SPEC: HOST_AGENT.md §1 (a room's channel belongs to one server entry; its match starts on that entry's host)

use crate::state::{Core, Sid};

/// Highest channel id an entry may own: the clan lobby's channels start above it (`clan::CLAN_CHANNEL_BASE`).
pub const MAX_CHANNEL: u32 = 64;

/// One configured (non-clan) entry of the server list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The entry id in 0x2000 / 0x3b1 (1.. in config order).
    pub id: u32,
    pub name: String,
    pub capacity: u32,
    /// "" = this machine; else the name of a `[[hosts.agents]]` entry.
    pub host: String,
    /// The first channel id of its block.
    pub first: u32,
    /// The number of channels of its block (may be 0 when the 64 ids are used up).
    pub channels: u32,
    /// LG-304: `lobby.servers[].features` of the entry.
    pub features: Vec<String>,
}

impl Entry {
    /// True when channel id `ch` belongs to this entry.
    pub fn owns(&self, ch: u32) -> bool {
        ch >= self.first && ch < self.first + self.channels
    }

    /// The channel ids of the block.
    pub fn channel_ids(&self) -> std::ops::Range<u32> {
        self.first..self.first + self.channels
    }
}

/// The configured entries. With no `[[lobby.servers]]` entry or one, the single entry keeps today's form: the
/// `--server-max` capacity, the `server_name`, channels 1..=`lobby.channels`. With two or more, entry n owns the
/// next `channels` ids after entry n-1's block (the first entry's capacity is still `--server-max`).
pub fn entries(st: &Core) -> Vec<Entry> {
    let cap = st.opts.server_max.clamp(1, u32::MAX as i64) as u32;
    let cfg = &st.opts.servers;
    if cfg.len() <= 1 {
        return vec![Entry {
            id: 1,
            name: st.opts.server_name.clone(),
            capacity: cap,
            host: cfg.first().map(|e| e.host.clone()).unwrap_or_default(),
            first: 1,
            channels: st.rules.channels,
            features: cfg.first().map(|e| e.features.clone()).unwrap_or_default(),
        }];
    }
    let mut next = 1u32;
    cfg.iter()
        .enumerate()
        .map(|(i, e)| {
            let left = (MAX_CHANNEL + 1).saturating_sub(next);
            let n = e.channels.min(left);
            let entry = Entry {
                id: i as u32 + 1,
                name: if i == 0 {
                    st.opts.server_name.clone()
                } else {
                    e.name.clone()
                },
                capacity: if i == 0 { cap } else { e.capacity.max(1) },
                host: e.host.clone(),
                first: next,
                channels: n,
                features: e.features.clone(),
            };
            next += n;
            entry
        })
        .collect()
}

/// The id of the clan entry: 2 while there is one configured entry (as before), else the id after the last one.
pub fn clan_id(st: &Core) -> u32 {
    let n = st.opts.servers.len() as u32;
    if n <= 1 {
        2
    } else {
        n + 1
    }
}

/// The entry that owns channel `ch` (none for the clan lobby's channels).
pub fn entry_of_channel(st: &Core, ch: u32) -> Option<Entry> {
    entries(st).into_iter().find(|e| e.owns(ch))
}

/// LG-304: whether the entry owning channel `ch` has `feature` (never for the clan lobby's channels).
pub fn channel_has_feature(st: &Core, ch: u32, feature: &str) -> bool {
    entry_of_channel(st, ch).is_some_and(|e| e.features.iter().any(|f| f == feature))
}

/// The game host of a room's channel: "" (this machine) for the clan lobby and for a channel no entry owns.
pub fn host_of_channel(st: &Core, ch: u32) -> String {
    entry_of_channel(st, ch).map(|e| e.host).unwrap_or_default()
}

/// The entry a lobby session picked (its ticket's server id); the first entry when it picked none of them
/// (`--lenient` logins without a ticket, the clan entry).
pub fn entry_of_session(st: &Core, sid: Sid) -> Entry {
    let picked = st.sessions.get(&sid).map(|s| s.server).unwrap_or(0);
    let mut all = entries(st);
    match all.iter().position(|e| e.id == picked) {
        Some(i) => all.swap_remove(i),
        None => all.swap_remove(0),
    }
}

/// MAP-82: the entry whose map table a lobby session sees: the one owning its channel, else the one it picked (0x301
/// and 0x311 come before any channel). `None`: neither (a `--lenient` login, which a featured entry refuses anyway).
pub fn map_entry_of_session(st: &Core, sid: Sid) -> Option<u32> {
    let s = st.sessions.get(&sid)?;
    s.channel
        .and_then(|ch| entry_of_channel(st, ch))
        .map(|e| e.id)
        .or_else(|| entries(st).iter().find(|e| e.id == s.server).map(|e| e.id))
}

/// LG-313: the curated set of the entry owning channel `ch` (none for a plain entry and the clan lobby).
pub fn entry_set_of_channel(st: &Core, ch: u32) -> Option<&crate::modset::EntrySet> {
    entry_of_channel(st, ch).and_then(|e| st.entry_mods.get(&e.id))
}

/// LG-313: 0x3f11: what this connection's client carries for an entry.
// DC-DEFAULT: LG-313 (proposed, project decision)
pub fn on_entry_claim(st: &mut Core, sid: Sid, p: &[u8]) {
    let live: Vec<Sid> = st
        .entry_claims
        .keys()
        .copied()
        .filter(|s| !st.sessions.contains_key(s))
        .collect();
    for s in live {
        st.entry_claims.remove(&s);
    }
    if let Some(c) = dc_wire::entrymods::parse_claim(p) {
        st.entry_claims.insert(sid, c);
    }
}

/// LG-313: the check of a pick (0x20a) or a move (0x3b2) of a featured entry: the connection's claim must name the entry
/// and carry its exact hashes. True: refused, the text sent to the client as 0x3f12 (a log line says why). A plain entry
/// passes without looking, so its pick is what it was.
// DC-DEFAULT: LG-313 (proposed, project decision): the login comes before the pick, so LG-271 moves here
pub fn pick_refused(st: &mut Core, sid: Sid, server: u32) -> bool {
    use dc_wire::loginmods::short;
    let Some(want) = st.entry_mods.get(&server).map(|e| e.expected.clone()) else {
        return false;
    };
    let claim = st.entry_claims.remove(&sid).filter(|c| c.entry == server);
    let (set, content) = claim.map_or((None, None), |c| (Some(c.set), Some(c.content)));
    if set.as_deref() == Some(want.set.as_str())
        && content.as_deref() == Some(want.content.as_str())
    {
        return false;
    }
    let peer = st
        .sessions
        .get(&sid)
        .map(|s| s.peer.clone())
        .unwrap_or_default();
    st.note(&format!(
        "client {peer}: entry {server} mod set mismatch (client {}…, server {}…)",
        set.as_deref().map_or("none", short),
        short(&want.set)
    ));
    let text = if set.is_none() {
        "This server needs its content pack and your game did not load it. Update DeCombat and pick the server again."
    } else {
        "Your copy of this server's content pack is not the one it runs now. Pick the server again to update it."
    };
    st.send(sid, dc_wire::entrymods::refusal_frame(server, text));
    true
}
