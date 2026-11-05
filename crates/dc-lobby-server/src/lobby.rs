//! Lobby: login, character / equipment / inventory / balance, room settings, servers and channels, lobby
//! lists, chat, profile, options, logout, and the connection-loss path.
// SPEC: PROTOCOL_2009.md §3

use crate::auth;
use crate::clock;
use crate::db;
use crate::inventory;
use crate::msg::*;
use crate::room;
use crate::state::{Core, HResult, Sid};
use crate::wire::{self, ChannelEntry, LobbyLoginReply};
use dc_wire::{PktReader, PktWriter};
use rusqlite::params;
use std::time::Instant;

/// Room-settings key and version announced in 0x301 (+0x3a / +0x3e); the client fetches 0x311 when its copy is
/// missing or older.
pub const ROOM_SETTINGS_KEY: u32 = 1;
/// The vanilla value; a modded lobby announces `Core::room_settings_version` (MAP-38).
pub const ROOM_SETTINGS_VERSION: u32 = 1;
/// 0x311 carries at most this many (map, mode mask) pairs (its fixed 0x10a bytes).
// SPEC: PROTOCOL_2009.md §3.5 0x311
pub const ROOM_SETTINGS_MAX_PAIRS: usize = 30;

fn lobby_account(st: &Core, sid: Sid) -> Option<u32> {
    st.sessions
        .get(&sid)
        .filter(|s| s.in_lobby)
        .and_then(|s| s.account)
}

/// Drops a lobby session from its room and channel (used by disconnects and by a second login).
pub fn leave_everything(st: &mut Core, sid: Sid) -> HResult {
    room::remove_member(st, sid, crate::wire::REMOVE_LEFT, false)?;
    let (account, channel) = match st.sessions.get(&sid) {
        Some(s) => (s.account, s.channel),
        None => return Ok(()),
    };
    if let (Some(a), Some(ch)) = (account, channel) {
        if let Some(s) = st.sessions.get_mut(&sid) {
            s.channel = None;
        }
        let others: Vec<Sid> = st
            .lobby_audience(ch)
            .into_iter()
            .filter(|&s| s != sid)
            .collect();
        st.send_all(&others, &wire::u32_push(LOBBY_USER_REMOVE_PUSH, a));
    }
    Ok(())
}

/// The connection is gone.
pub fn disconnect(st: &mut Core, sid: Sid) -> HResult {
    let Some(s) = st.sessions.get(&sid) else {
        return Ok(());
    };
    if s.closing {
        return Ok(());
    }
    let (account, peer) = (s.account, s.peer.clone());
    // MON-3: a lobby connection that ends while its player is in a match is a problem event
    if let Some(a) = account.filter(|_| s.in_lobby) {
        let place = s.room.and_then(|k| {
            let r = st.rooms.get(&k)?;
            let playing = r.state == room::RoomPhase::Playing
                && r.member(a).is_some_and(|m| m.state == room::MEMBER_PLAYING);
            Some(format!(
                " (in room {}-{}{})",
                k.channel,
                k.no,
                if playing { ", in a match" } else { "" }
            ))
        });
        crate::opsmon::event(
            st,
            a,
            crate::opsmon::kind::DISCONNECT,
            &format!(
                "lobby connection {sid} from {peer} closed{}",
                place.unwrap_or_default()
            ),
        );
    }
    crate::gm::on_disconnect(st, sid)?;
    leave_everything(st, sid)?;
    st.sessions.remove(&sid);
    st.note(&format!(
        "connection {} from {} closed (account {:?})",
        sid, peer, account
    ));
    Ok(())
}

// DC-DEFAULT: CD-19 single-use key; --lenient accepts an unknown key for an existing character
fn on_login(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (key, account) = wire::parse_lobby_login(p);
    let now = Instant::now();
    let (ticket_ok, picked) = match st.tickets.remove(&key) {
        Some(t) => (t.expires > now && t.account == account, t.server),
        None => (false, 0),
    };
    let has_char = st.character(account).is_some();
    // LG-318: a bot seat only with its key
    let lenient = st.opts.lenient && !st.pool.has(account);
    if !(has_char && (ticket_ok || lenient)) {
        st.note(&format!(
            "lobby login refused: account {} (key ok {})",
            account, ticket_ok
        ));
        st.send(sid, wire::lobby_login_reply(&LobbyLoginReply::default()));
        return Ok(());
    }
    // DC-DEFAULT: CD-55 (proposed) one lobby session per account: an older one is dropped
    let old: Vec<Sid> = if st.rules.single_session {
        st.sids_of(account)
            .into_iter()
            .filter(|&s| s != sid)
            .collect()
    } else {
        Vec::new()
    };
    for o in old {
        leave_everything(st, o)?;
        st.close(o);
        st.note(&format!(
            "account {}: older lobby connection {} replaced",
            account, o
        ));
    }
    if let Some(s) = st.sessions.get_mut(&sid) {
        s.account = Some(account);
        s.in_lobby = true;
        // CD-21 amended: the picked entry's channel block is the channel list of this session
        s.server = if ticket_ok { picked } else { 0 };
    }
    crate::botpool::on_lobby_login(st, sid, account);
    // DC-DEFAULT: CD-06 expiry check at login
    inventory::expire(st, Some(account), false)?;
    inventory::repair_defaults(st, account)?;
    crate::nxrewards::on_lobby_login(st, account)?;
    let shop_versions = crate::shop::versions(st)?;
    let reply = LobbyLoginReply {
        ok: true,
        server_time: clock::local_now().0.to_bytes(),
        // DC-DEFAULT: CD-20 lobby session index = connection number
        lobby_slot: sid as u32,
        price_counter: shop_versions.prices,
        shop_view_version: shop_versions.view,
        list_versions: shop_versions.lists,
        room_settings_key: ROOM_SETTINGS_KEY,
        room_settings_version: st
            .room_settings_version_for(crate::servers::map_entry_of_session(st, sid)),
    };
    st.send(sid, wire::lobby_login_reply(&reply));
    crate::gm::on_lobby_login(st, sid, account)?;
    // DC-DEFAULT: CD-87 (proposed, USER REQUEST 2026-10-31 "when you log into the server you would get a popup for
    // any item that was in your inventory that expired since last time you logged in") the items that expired since
    // the last login, one box each (0x3011: the client shows `res 0x1786` with the item's name, CLI-1391)
    let expired = db::take_expiry_notes(&st.db, account)?;
    if !expired.is_empty() {
        for chunk in expired.chunks(crate::notices::EXPIRY_BOX_MAX) {
            st.send(sid, crate::notices::expiry_box_frame(chunk));
        }
        st.note(&format!(
            "expiry: account {} told of {} item(s) that expired while away {:?}",
            account,
            expired.len(),
            expired
        ));
    }
    st.lobby_logins += 1;
    // MON-3: the timeline starts at the login; old monitor rows go now and then
    let from = st
        .sessions
        .get(&sid)
        .map(|s| s.peer.clone())
        .unwrap_or_default();
    crate::opsmon::event(
        st,
        account,
        crate::opsmon::kind::LOGIN,
        &format!("lobby login on connection {sid} from {from}"),
    );
    crate::opsmon::maybe_purge(st);
    st.note(&format!(
        "lobby login: account {} on connection {}",
        account, sid
    ));
    Ok(())
}

fn on_character_info(st: &mut Core, sid: Sid) -> HResult {
    let Some(c) = lobby_account(st, sid).and_then(|a| st.character(a)) else {
        st.send(sid, wire::ok_reply(CHAR_SHEET_REPLY, false, 0x108));
        return Ok(());
    };
    let info = st.char_sheet(&c);
    st.send(sid, wire::char_sheet_reply(&info));
    Ok(())
}

fn on_equipment(st: &mut Core, sid: Sid) -> HResult {
    let Some(a) = lobby_account(st, sid) else {
        st.send(sid, wire::ok_reply(EQUIPMENT_REPLY, false, 0xa6));
        return Ok(());
    };
    inventory::expire(st, Some(a), true)?;
    inventory::repair_defaults(st, a)?;
    let items = inventory::visible_items(st, a, db::items(&st.db, a)?);
    // LG-407: the free weapons of the Weapon Free Passes hold their slots, flagged at +0x4e
    let items = crate::freepass::equipped_view(st, a, items);
    let slots = inventory::equipment_slots(&items);
    st.send(
        sid,
        wire::equipment_reply(
            &slots,
            &crate::freepass::flags(st, a),
            &crate::loadout::function_slots(&st.db, a)?,
        ),
    );
    Ok(())
}

fn on_inventory(st: &mut Core, sid: Sid) -> HResult {
    let Some(a) = lobby_account(st, sid) else {
        st.send(sid, wire::ok_reply(INVENTORY_REPLY, false, 0x12));
        return Ok(());
    };
    inventory::expire(st, Some(a), true)?;
    inventory::repair_defaults(st, a)?;
    let items = inventory::visible_items(st, a, db::items(&st.db, a)?);
    st.send(
        sid,
        wire::inventory_reply(&inventory::inventory_records(&items)),
    );
    Ok(())
}

fn on_balance(st: &mut Core, sid: Sid) -> HResult {
    let c = lobby_account(st, sid)
        .and_then(|a| st.character(a))
        .unwrap_or_default();
    // DC-DEFAULT: CD-37 balance report: +0xe credit (nx_cr), +0x12 prepaid (nx)
    st.send(
        sid,
        wire::balance_reply(
            c.nx_cr.clamp(0, u32::MAX as i64) as u32,
            c.nx.clamp(0, u32::MAX as i64) as u32,
        ),
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// playable-mode gate (config `modes.playable`)
// ---------------------------------------------------------------------------------------------------------------

/// The modes `dc-dserver` plays today, by our own names (GAME_MODES_2009.md section 2). The vocabulary lives in
/// `dc_config::modes` (MAP-6) so the map records and the game server spell the modes the same way.
// DC-DEFAULT: GMD-26 (amended by project decision: the other modes are to be played) `modes.playable` =
// TDM, TDM_CLAN, COOP, FFA, ELIM, ELIM_CLAN, FLAG, FLAG_CLAN, BOMB, BOMB_CLAN, INFECT, BOUNTY (LG-70, LG-71, LG-80,
// LG-81, LG-85); see DC_DEFAULTS.md
pub const DEFAULT_PLAYABLE: &[&str] = dc_config::modes::MODE_NAMES;

/// Our name -> ROM mode class; moved to `dc_config::modes` (MAP-6) and re-exported for the lobby's callers.
// SPEC: GAME_MODES_2009.md §2 (the classes are PROVEN client constants, the names are ours)
pub use dc_config::modes::mode_class_of_name;

/// Which mode classes the lobby offers and lets start. `None` = every mode (gate off).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModePolicy {
    classes: Option<std::collections::BTreeSet<u32>>,
}

impl Default for ModePolicy {
    fn default() -> Self {
        Self::from_names(DEFAULT_PLAYABLE)
    }
}

impl ModePolicy {
    /// An empty list, `*` or `all` switches the gate off; an unknown name is ignored (returned in the second value).
    pub fn parse<S: AsRef<str>>(names: &[S]) -> (Self, Vec<String>) {
        let mut set = std::collections::BTreeSet::new();
        let mut bad = Vec::new();
        let mut open = names.is_empty();
        for n in names {
            let n = n.as_ref();
            if n.trim() == "*" || n.trim().eq_ignore_ascii_case("all") {
                open = true;
            } else if let Some(c) = mode_class_of_name(n) {
                set.insert(c);
            } else {
                bad.push(n.to_string());
            }
        }
        let classes = if open { None } else { Some(set) };
        (Self { classes }, bad)
    }

    pub fn from_names<S: AsRef<str>>(names: &[S]) -> Self {
        Self::parse(names).0
    }

    #[allow(dead_code)]
    pub fn allow_all() -> Self {
        Self { classes: None }
    }

    pub fn allows(&self, class: u32) -> bool {
        self.classes.as_ref().is_none_or(|s| s.contains(&class))
    }
}

static POLICY: std::sync::RwLock<Option<ModePolicy>> = std::sync::RwLock::new(None);

/// Installs `modes.playable` (the integrator calls this from the config loader). Returns the ignored names.
#[allow(dead_code)] // called by the integrator's config hook (report section 5)
pub fn set_playable<S: AsRef<str>>(names: &[S]) -> Vec<String> {
    let (p, bad) = ModePolicy::parse(names);
    if let Ok(mut g) = POLICY.write() {
        *g = Some(p);
    }
    bad
}

/// The policy in force: the installed one, else the built-in default.
pub fn policy() -> ModePolicy {
    POLICY
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default()
}

/// Mode class of row `index` of the plain or clan mode table; `None` when the ROM has no such row.
pub fn mode_class_of_row(st: &Core, index: u32, clan: bool) -> Option<u32> {
    let rows = st.rom.game_room.modes(clan);
    let row = rows.get(index as usize)?;
    row.get("GameType")
        .and_then(|v| v.trim().parse::<u32>().ok())
}

/// Drops the mode bits the policy does not allow from `(map, mask)` pairs and the pairs left empty.
pub fn filter_masks(
    st: &Core,
    masks: Vec<(usize, u32)>,
    clan: bool,
    pol: &ModePolicy,
) -> Vec<(usize, u32)> {
    let classes: Vec<Option<u32>> = (0..32).map(|i| mode_class_of_row(st, i, clan)).collect();
    masks
        .into_iter()
        .filter_map(|(map, mask)| {
            let mut keep = 0u32;
            for (i, c) in classes.iter().enumerate() {
                // a row the ROM does not have (no-ROM fallback table) cannot be classified: kept
                if mask & (1 << i) != 0 && c.is_none_or(|c| pol.allows(c)) {
                    keep |= 1 << i;
                }
            }
            (keep != 0).then_some((map, keep))
        })
        .collect()
}

/// True when the host's Start in room `key` must be refused: the room's mode is not playable.
// DC-DEFAULT: GMD-26 a room of an unplayable mode never starts; the refusal is 0x393 {fail, reason 0x8e, kind 3}
// ("join failed (server error)"), the answer the client already shows for a failed game server start
// (SPEC: PROTOCOL_2009.md §3.7 0x393 kind 3; EVIDENCE: cshell+0x33ef0 -> cshell+0x15f0d0)
pub fn start_refused(st: &Core, key: room::RoomKey) -> bool {
    let Some(r) = st.rooms.get(&key) else {
        return false;
    };
    let clan = crate::clan::is_clan_channel(key.channel);
    match mode_class_of_row(st, r.mode, clan) {
        Some(c) => !policy().allows(c),
        None => false,
    }
}

/// Answers the refusal and logs it.
pub fn refuse_start(st: &mut Core, sid: Sid, key: room::RoomKey) {
    st.send(
        sid,
        wire::user_flag_reply(false, wire::REASON_JOIN_FAILED, wire::FLAG_REPLY_KIND_JOIN),
    );
    let mode = st.rooms.get(&key).map(|r| r.mode).unwrap_or(0);
    st.note(&format!(
        "room {}-{}: start refused, mode {} is not in modes.playable",
        key.channel, key.no, mode
    ));
    if let Some(a) = st.sessions.get(&sid).and_then(|s| s.account) {
        crate::opsmon::event(
            st,
            a,
            crate::opsmon::kind::START_REFUSED,
            &format!(
                "room {}-{}: start refused, mode {mode} is not playable here",
                key.channel, key.no
            ),
        );
    }
}

/// True (and answered) when `p` is the host's Start (0x392 {0, 2}) in a waiting room of an unplayable mode.
fn start_gate_hit(st: &mut Core, sid: Sid, p: &[u8]) -> bool {
    let (kind, value) = wire::parse_user_flag(p);
    if kind != 0 || value != 2 {
        return false;
    }
    let (Some(account), Some(key)) = (
        st.sessions.get(&sid).and_then(|s| s.account),
        st.sessions.get(&sid).and_then(|s| s.room),
    ) else {
        return false;
    };
    let host_waiting = st
        .rooms
        .get(&key)
        .is_some_and(|r| r.host == account && r.state == room::RoomPhase::Waiting);
    if host_waiting && start_refused(st, key) {
        refuse_start(st, sid, key);
        return true;
    }
    false
}

/// 0x311 room settings: the map / mode masks of the ROM room table and of the community maps after it (MAP-35),
/// exactly 0x10a bytes.
// SPEC: PROTOCOL_2009.md §3.5 0x311 (EVIDENCE: cshell+0x3dbf0, cshell+0x3db40); DC-DEFAULT: CD-46, GMD-26 (only the
// playable modes keep their bits), MAP-35 (community maps appended, the cap warned about at start-up), MAP-82 (`entry`
// = the session's entry: a featured entry's table and version)
pub fn room_settings_reply(st: &Core, entry: Option<u32>) -> Vec<u8> {
    let masks = filter_masks(st, st.maps_for(entry).map_mode_masks(), false, &policy());
    let n = masks.len().min(ROOM_SETTINGS_MAX_PAIRS);
    let mut w = PktWriter::new(ROOM_SETTINGS_REPLY);
    w.u32(1)
        .u32(0)
        .u32(ROOM_SETTINGS_KEY)
        .u32(st.room_settings_version_for(entry))
        .u32(n as u32);
    for &(map, mask) in &masks[..n] {
        w.u32(map as u32).u32(mask);
    }
    w.at(0x10a);
    w.into_frame()
}

fn chan_cap(st: &Core) -> u32 {
    // SPEC: LOBBY_HARNESS.md §1 --channel-max (0 => 200)
    if st.opts.channel_max > 0 {
        st.opts.channel_max.min(u32::MAX as i64) as u32
    } else {
        200
    }
}

fn users_in_channel(st: &Core, ch: u32) -> u32 {
    st.sessions
        .values()
        .filter(|s| s.in_lobby && !s.closing && s.channel == Some(ch))
        .count() as u32
}

/// The channel list of the first server entry (the whole list while one entry is configured).
// DC-DEFAULT: CD-21 three channels named by number
#[cfg(test)]
pub fn channels(st: &Core) -> Vec<ChannelEntry> {
    channels_of(st, &crate::servers::entries(st)[0])
}

/// The channel list one lobby session sees: the block of the entry its ticket picked (CD-21 amended).
pub fn channels_for(st: &Core, sid: Sid) -> Vec<ChannelEntry> {
    channels_of(st, &crate::servers::entry_of_session(st, sid))
}

/// One entry's channels, named by their number inside the entry ("Channel 1" is the entry's first channel).
fn channels_of(st: &Core, e: &crate::servers::Entry) -> Vec<ChannelEntry> {
    let cap = chan_cap(st);
    e.channel_ids()
        .map(|id| ChannelEntry {
            id,
            name: format!("Channel {}", id + 1 - e.first),
            open: true,
            capacity: cap,
            current: users_in_channel(st, id),
            op_capacity: crate::gm::OPERATOR_SLOTS,
            op_current: crate::gm::gms_online(st, Some(id)),
        })
        .collect()
}

fn on_channel_enter(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (_acc, ch) = wire::parse_channel_enter(p);
    let Some(account) = lobby_account(st, sid) else {
        st.send(sid, wire::channel_enter_reply(false, 0));
        return Ok(());
    };
    let current = st.sessions.get(&sid).and_then(|s| s.channel);
    // CD-21 amended: only a channel of the entry this session picked
    let entry = crate::servers::entry_of_session(st, sid);
    // LG-313: a featured entry's channels need its pick (a --lenient login without a ticket falls to the first entry)
    let picked = st.sessions.get(&sid).is_some_and(|s| s.server == entry.id);
    let exists = entry.owns(ch) && (picked || !st.entry_mods.contains_key(&entry.id));
    let full = current != Some(ch) && users_in_channel(st, ch) >= chan_cap(st);
    // DC-DEFAULT: LG-175 a GM enters a full channel while its operator slots are not used up
    let op_slot = full
        && crate::gm::is_gm(st, account)
        && crate::gm::gms_online(st, Some(ch)) < crate::gm::OPERATOR_SLOTS;
    if !exists || (full && !op_slot) {
        st.send(sid, wire::channel_enter_reply(false, 0));
        return Ok(());
    }
    if current.is_some() {
        leave_everything(st, sid)?;
    }
    if let Some(s) = st.sessions.get_mut(&sid) {
        s.channel = Some(ch);
    }
    // DC-DEFAULT: CD-22 in-channel id = account number
    st.send(sid, wire::channel_enter_reply(true, account));
    // DC-DEFAULT: LG-305 (proposed) only a featured entry tells the client
    if crate::gungame::offered(st, ch) {
        st.send(sid, dc_wire::gungame::offer_frame());
    }
    let rec = wire::lobby_user_add_push(&st.lobby_user(account));
    let others: Vec<Sid> = st
        .lobby_audience(ch)
        .into_iter()
        .filter(|&s| s != sid)
        .collect();
    st.send_all(&others, &rec);
    st.note(&format!("account {} entered channel {}", account, ch));
    Ok(())
}

fn on_channel_leave(st: &mut Core, sid: Sid) -> HResult {
    let ok = lobby_account(st, sid).is_some();
    leave_everything(st, sid)?;
    st.send(sid, wire::ok_reply(CHAN_EXIT_REPLY, ok, 0xa));
    Ok(())
}

fn on_channel_users(st: &mut Core, sid: Sid) -> HResult {
    let Some(ch) = st.sessions.get(&sid).and_then(|s| s.channel) else {
        st.send(sid, wire::channel_users_reply(&[]));
        return Ok(());
    };
    let accounts: Vec<u32> = st
        .lobby_audience(ch)
        .into_iter()
        .filter_map(|s| st.sessions.get(&s).and_then(|x| x.account))
        .collect();
    let users: Vec<_> = accounts.into_iter().map(|a| st.lobby_user(a)).collect();
    st.send(sid, wire::channel_users_reply(&users));
    Ok(())
}

fn on_room_list(st: &mut Core, sid: Sid) -> HResult {
    let ch = st.sessions.get(&sid).and_then(|s| s.channel);
    let rows: Vec<_> = st
        .rooms
        .values()
        .filter(|r| Some(r.key.channel) == ch)
        .map(|r| r.row())
        .collect();
    st.send(sid, wire::room_list_reply(&rows));
    if let Some(ch) = ch {
        crate::gungame::send_channel_rooms(st, sid, ch);
    }
    Ok(())
}

/// Chat goes to everyone in the same room, or in the same channel lobby, including the sender.
// DC-DEFAULT: CD-30 chat echo to the sender
fn on_chat(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let text = wire::parse_chat(p);
    let Some(account) = lobby_account(st, sid) else {
        return Ok(());
    };
    if text.trim().is_empty() {
        return Ok(());
    }
    if let Some(reply) = nation_command(st, account, &text)? {
        st.send(sid, wire::chat_push(0, "", 0, &reply));
        return Ok(());
    }
    let c = st.character(account).unwrap_or_default();
    let (room, ch) = st
        .sessions
        .get(&sid)
        .map(|s| (s.room, s.channel))
        .unwrap_or((None, None));
    let audience: Vec<Sid> = match (room, ch) {
        (Some(k), _) => st
            .rooms
            .get(&k)
            .map(|r| r.sids())
            .unwrap_or_else(|| vec![sid]),
        (None, Some(ch)) => st.lobby_audience(ch),
        _ => vec![sid],
    };
    let frame = wire::chat_push(
        account,
        &c.nick,
        c.nick_color.clamp(0, u32::MAX as i64) as u32,
        &text,
    );
    st.send_all(&audience, &frame);
    let place = match (room, ch) {
        (Some(k), _) => format!("room {}-{}", k.channel, k.no),
        (None, Some(c)) => format!("channel {}", c),
        _ => "nowhere".into(),
    };
    st.note(&format!("chat [{}] {}: {}", place, c.nick, text));
    Ok(())
}

/// `!nation <n>`: sets the character's country code shown in 0x303 +0x100 (0..255, the range the country
/// item window accepts: CD-80 kind 17). A server-side chat command; the 2009 client changes the country only
/// through that item. HARNESS: lobby_harness::lobby3 drives it.
fn nation_command(
    st: &mut Core,
    account: u32,
    text: &str,
) -> Result<Option<String>, crate::state::Fault> {
    let mut words = text.split_whitespace();
    if !words
        .next()
        .map(|w| w.eq_ignore_ascii_case("!nation"))
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let Some(n) = words
        .next()
        .and_then(|w| w.parse::<i64>().ok())
        .filter(|n| (0..=255).contains(n))
    else {
        return Ok(Some("Usage: !nation <0..255>".into()));
    };
    st.db.execute(
        "UPDATE characters SET nation = ?2 WHERE user_no = ?1",
        params![account as i64, n],
    )?;
    Ok(Some(format!("Your nation is now {}.", n)))
}

fn on_profile(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let target = PktReader::new(p).at(6).u32();
    let found = lobby_account(st, sid).and_then(|_| st.character(target));
    match found {
        Some(c) => {
            let pr = st.profile(&c);
            st.send(sid, wire::profile_reply(true, &pr));
        }
        None => st.send(sid, wire::profile_reply(false, &Default::default())),
    }
    Ok(())
}

/// 0x308: option word bit `index` set or cleared (0x303 +0xb0 reports the word).
fn on_option(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let mut r = PktReader::new(p);
    let index = r.at(6).u32();
    let on = r.u32() != 0;
    let ok = match lobby_account(st, sid) {
        Some(a) if index < 32 => {
            let bit = 1i64 << index;
            let sql = if on {
                "UPDATE characters SET options = options | ?2 WHERE user_no = ?1"
            } else {
                "UPDATE characters SET options = options & ~?2 WHERE user_no = ?1"
            };
            st.db.execute(sql, params![a as i64, bit])?;
            true
        }
        _ => false,
    };
    st.send(sid, wire::ok_reply(OPTION_SET_REPLY, ok, 0xa));
    Ok(())
}

/// 0x30a: the client enters a private state (1 shop, 3 tuning, 4 clan); answered with 0x30b (+6 u32 result).
// SPEC: PROTOCOL_2009.md §3.4 0x30a / 0x30b (EVIDENCE: cshell+0x18b6b0, cshell+0x35c90)
// DC-DEFAULT: LG-1 (proposed) result 1 for a known state of a lobby session, 0 otherwise; the state is only remembered
fn on_private_state(st: &mut Core, sid: Sid, p: &[u8]) {
    let state = PktReader::new(p).at(6).u32();
    let ok = lobby_account(st, sid).is_some() && matches!(state, 1 | 3 | 4);
    if ok {
        if let Some(s) = st.sessions.get_mut(&sid) {
            s.private_state = state;
        }
    }
    st.send(sid, wire::ok_reply(PRIVATE_STATE_REPLY, ok, 0xa));
    // LG-405: the page that shows and spends the balances opens with the DB row's values
    if ok {
        crate::shop::send_balance(st, sid);
    }
}

/// 0x30c: the client leaves the private state; answered with 0x30d (+6 u32 result).
// SPEC: PROTOCOL_2009.md §3.4 0x30c / 0x30d (EVIDENCE: cshell+0x18b780, cshell+0x35cc0)
fn on_private_state_end(st: &mut Core, sid: Sid) {
    let ok = lobby_account(st, sid).is_some();
    if let Some(s) = st.sessions.get_mut(&sid) {
        s.private_state = 0;
    }
    st.send(sid, wire::ok_reply(PRIVATE_STATE_END_REPLY, ok, 0xa));
}

/// 0x366: the pending notice text; the flag is cleared once read.
fn on_notice(st: &mut Core, sid: Sid) -> HResult {
    if let Some(a) = lobby_account(st, sid) {
        st.db.execute(
            "UPDATE characters SET notice_pending = 0 WHERE user_no = ?1",
            params![a as i64],
        )?;
    }
    let mut w = PktWriter::new(NOTICE_REPLY);
    w.u32(1).at(0x12).wstr("", 513).u32(0);
    w.at(0x418);
    st.send(sid, w.into_frame());
    Ok(())
}

fn on_server_move(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let server = PktReader::new(p).at(6).u32();
    let known = auth::server_entries(st)
        .iter()
        .any(|e| e.id == server && e.open);
    let known =
        known && lobby_account(st, sid).is_some() && !crate::servers::pick_refused(st, sid, server);
    match lobby_account(st, sid) {
        Some(a) if known => {
            let key = auth::issue_ticket(st, a, server);
            st.send(sid, wire::session_key_reply(SERVER_MOVE_REPLY, true, &key));
        }
        _ => st.send(
            sid,
            wire::session_key_reply(SERVER_MOVE_REPLY, false, &[0u8; 16]),
        ),
    }
    Ok(())
}

/// Lobby messages; returns false when `id` is not one of them.
pub fn dispatch(st: &mut Core, sid: Sid, id: u16, p: &[u8]) -> Result<bool, crate::state::Fault> {
    match id {
        LOBBY_LOGIN => on_login(st, sid, p)?,
        ITEM_SYNC => st.send(sid, wire::item_sync_reply(0)),
        CHAR_SHEET_FETCH => on_character_info(st, sid)?,
        EQUIPMENT => on_equipment(st, sid)?,
        INVENTORY => on_inventory(st, sid)?,
        BALANCE => on_balance(st, sid)?,
        ROOM_SETTINGS_FETCH => {
            let f = room_settings_reply(st, crate::servers::map_entry_of_session(st, sid));
            st.send(sid, f);
        }
        // the host's Start in a room whose mode dc-dserver cannot play (GMD-26)
        ROOM_USER_FLAG if start_gate_hit(st, sid, p) => {}
        NOTICE_FETCH => on_notice(st, sid)?,
        ITEM_USE => match lobby_account(st, sid) {
            Some(a) => inventory::handle_item_window(st, sid, a, p)?,
            None => st.send(sid, wire::item_use_reply(false, 0, 0, 0, 0, 0)),
        },
        // the 2009 client has no caller for 0x33e; answered with an empty list (SPEC: PROTOCOL_2009.md §4.2)
        EXPIRY_CHECK => st.send(sid, wire::expiry_push(&[], true)),
        CHANNEL_LIST => {
            let ch = channels_for(st, sid);
            st.send(sid, wire::channel_list_reply(&ch));
        }
        CHANNEL_ENTER => on_channel_enter(st, sid, p)?,
        CHAN_EXIT => on_channel_leave(st, sid)?,
        CHANNEL_USERS => on_channel_users(st, sid)?,
        ROOM_LIST => on_room_list(st, sid)?,
        CHAT_SEND => on_chat(st, sid, p)?,
        PROFILE_FETCH => on_profile(st, sid, p)?,
        OPTION_SET => on_option(st, sid, p)?,
        LOCATION => {
            let w = PktReader::new(p).at(6).u32();
            if let Some(s) = st.sessions.get_mut(&sid) {
                s.location = w;
            }
            if w == 1 {
                crate::nxrewards::on_back_in_room(st, sid)?;
            }
        }
        PRIVATE_STATE => on_private_state(st, sid, p),
        PRIVATE_STATE_END => on_private_state_end(st, sid),
        LOGOUT => st.send(sid, wire::ok_reply(LOGOUT_REPLY, true, 0xa)),
        SERVER_LIST => {
            let e = auth::server_entries_for(st, sid);
            st.send(sid, wire::server_list_reply(&e));
        }
        SERVER_MOVE => on_server_move(st, sid, p)?,
        _ => return Ok(false),
    }
    Ok(true)
}

#[cfg(test)]
mod modegate_tests {
    use super::*;
    use crate::logs::Listener;
    use crate::room::{Room, RoomKey, RoomPhase};
    use crate::state::testing::*;
    use dc_bute::{ButeDoc, GameRoomData};
    use std::collections::BTreeSet;

    // synthetic room table: TDM (1), CLASS10 (10), COOP (5) and clan TDM (4), a clan CLASS10 row (10); Map1 has CLASS10
    // only (LG-80: BOMB is playable now; LG-85: BOUNTY too, CLASS10 stands for the unplayable mode)
    const TABLE: &str = r"[Map0]
Enable=TRUE
GMS_TDM=a
GMS_BNT=b
GMS_COOP=c
GMS_CTDM=d
GMS_CBNT=e
[Map1]
Enable=TRUE
GMS_BNT=b
GMS_CBNT=e
[Mode0]
Key=TDM
GameType=1
[Mode1]
Key=BNT
GameType=10
[Mode2]
Key=COOP
GameType=5
[ClanMode0]
Key=CTDM
GameType=4
EnablePlay=TRUE
[ClanMode1]
Key=CBNT
GameType=10
EnablePlay=TRUE
";

    fn with_table() -> Core {
        let mut st = state();
        Arc::get_mut(&mut st.rom).unwrap().game_room =
            GameRoomData::from_doc(ButeDoc::parse(TABLE));
        st
    }
    use std::sync::Arc;

    #[test]
    fn names_parse_and_unknown_names_are_reported() {
        let (p, bad) = ModePolicy::parse(&["tdm", "COOP", "7", "nope"]);
        assert_eq!(bad, vec!["nope".to_string()]);
        assert!(p.allows(1) && p.allows(5) && p.allows(7) && !p.allows(2));
        assert!(ModePolicy::parse::<&str>(&[]).0.allows(99));
        assert!(ModePolicy::from_names(&["*"]).allows(2));
        assert!(ModePolicy::from_names(&["All"]).allows(13));
    }

    #[test]
    fn default_policy_is_tdm_clan_tdm_coop_ffa_elimination_pro_flag_bomb_infect_and_spy_hunt() {
        let p = ModePolicy::default();
        let on: BTreeSet<u32> = (0..20).filter(|c| p.allows(*c)).collect();
        assert_eq!(on, BTreeSet::from([1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13]));
    }

    #[test]
    fn masks_keep_only_playable_bits_and_drop_empty_maps() {
        let st = with_table();
        let all = st.rom.map_mode_masks();
        assert_eq!(all, vec![(0, 0b111), (1, 0b010)]);
        let f = filter_masks(&st, all.clone(), false, &ModePolicy::default());
        assert_eq!(f, vec![(0, 0b101)]);
        assert_eq!(
            filter_masks(&st, all.clone(), false, &ModePolicy::allow_all()),
            all
        );
        let clan = st.rom.game_room.map_mode_masks(true);
        assert_eq!(clan, vec![(0, 0b11), (1, 0b10)]);
        assert_eq!(
            filter_masks(&st, clan, true, &ModePolicy::default()),
            vec![(0, 0b01)]
        );
        // a config override widens the list
        let bomb = ModePolicy::from_names(&["TDM", "CLASS10"]);
        assert_eq!(
            filter_masks(&st, st.rom.map_mode_masks(), false, &bomb),
            vec![(0, 0b011), (1, 0b010)]
        );
    }

    #[test]
    fn room_settings_reply_carries_the_filtered_masks() {
        let st = with_table();
        let f = room_settings_reply(&st, None);
        let (frames, _) = dc_wire::parse_tcp_frames(&f).unwrap();
        let p = &frames[0].1;
        assert_eq!(p.len(), 0x10a);
        let u = |o: usize| u32::from_le_bytes(p[o..o + 4].try_into().unwrap());
        assert_eq!((u(0x16), u(0x1a), u(0x1e)), (1, 0, 0b101));
    }

    fn host_in_room(st: &mut Core, mode: u32, channel: u32) -> (crate::state::Sid, RoomKey) {
        let sid = 7;
        let _rx = connect(st, sid, Listener::Lobby);
        let key = RoomKey { channel, no: 0 };
        let s = st.sessions.get_mut(&sid).unwrap();
        s.account = Some(42);
        s.in_lobby = true;
        s.room = Some(key);
        st.rooms.insert(
            key,
            Room {
                key,
                title: String::new(),
                password: String::new(),
                mode,
                map: 0,
                goal: 0,
                max_players: 8,
                mid_join: 0,
                ff_on: 0,
                killcam: 0,
                weapon_limit: 0,
                hacks: 0,
                hardcore: false,
                gun_game: false,
                possession: false,
                gun_seed: 0,
                ladder: Vec::new(),
                host_badge: 0,
                host: 42,
                state: RoomPhase::Waiting,
                members: Vec::new(),
                kicked: BTreeSet::new(),
                gen: 0,
                game: None,
                address_sent: false,
                played_since: None,
            },
        );
        std::mem::forget(_rx);
        (sid, key)
    }

    #[test]
    fn start_is_refused_for_an_unplayable_mode_only() {
        let st = &mut with_table();
        let (_, bomb) = host_in_room(st, 1, 1);
        assert!(start_refused(st, bomb));
        st.rooms.get_mut(&bomb).unwrap().mode = 0;
        assert!(!start_refused(st, bomb));
        st.rooms.get_mut(&bomb).unwrap().mode = 2;
        assert!(!start_refused(st, bomb), "co-op is playable");
        // a mode the ROM does not know cannot be classified and is left to the normal start path
        st.rooms.get_mut(&bomb).unwrap().mode = 30;
        assert!(!start_refused(st, bomb));
    }

    #[test]
    fn start_gate_answers_0x393_fail_8e_kind_3_and_leaves_the_room_waiting() {
        let st = &mut with_table();
        let (sid, key) = host_in_room(st, 1, 1);
        let rx = {
            let (tx, rx) = std::sync::mpsc::channel();
            st.sessions.get_mut(&sid).unwrap().tx = tx;
            rx
        };
        let mut body = PktWriter::new(ROOM_USER_FLAG);
        body.u32(0).u32(2);
        let p = body.into_payload();
        st.scope(|st| crate::net::route(st, sid, ROOM_USER_FLAG as i32, &p))
            .unwrap();
        let got = drain(&rx);
        let (_, d) = got
            .iter()
            .find(|(i, _)| *i == ROOM_USER_FLAG_REPLY)
            .unwrap();
        assert_eq!(u32_at(d, 6), 0);
        assert_eq!(
            u16::from_le_bytes([d[0xa], d[0xb]]),
            wire::REASON_JOIN_FAILED
        );
        assert_eq!(
            i16::from_le_bytes([d[0xc], d[0xd]]),
            wire::FLAG_REPLY_KIND_JOIN
        );
        assert_eq!(st.rooms[&key].state, RoomPhase::Waiting);
        assert!(!got.iter().any(|(i, _)| *i == MATCH_ADDRESS_PUSH));
    }

    // synthetic room table: Elimination Pro (class 12) and a flag row (class 7) on one map
    const ROUND_TABLE: &str = r"[Map0]
Enable=TRUE
GMS_TSV=a
GMS_CTF=b
GMS_SND=c
GMS_QRT=d
GMS_BNT=e
[Mode0]
Key=TSV
GameType=12
[Mode1]
Key=CTF
GameType=7
[Mode2]
Key=SND
GameType=2
[Mode3]
Key=QRT
GameType=11
[Mode4]
Key=BNT
GameType=9
";

    /// LG-70: by default the lobby offers the Elimination Pro row (its bit stays in 0x311) and its room starts.
    #[test]
    fn elimination_pro_is_offered_and_starts_by_default() {
        let mut st = state();
        Arc::get_mut(&mut st.rom).unwrap().game_room =
            GameRoomData::from_doc(ButeDoc::parse(ROUND_TABLE));
        let masks = filter_masks(&st, st.rom.map_mode_masks(), false, &ModePolicy::default());
        assert_eq!(masks.first().map(|m| m.1 & 1), Some(1), "{masks:?}");
        let (_, key) = host_in_room(&mut st, 0, 1);
        assert!(!start_refused(&st, key));
        // LG-71: Capture the Flag too
        assert_eq!(masks.first().map(|m| m.1 & 2), Some(2), "{masks:?}");
        st.rooms.get_mut(&key).unwrap().mode = 1;
        assert!(!start_refused(&st, key));
        // LG-80: Search and Destroy too
        assert_eq!(masks.first().map(|m| m.1 & 4), Some(4), "{masks:?}");
        st.rooms.get_mut(&key).unwrap().mode = 2;
        assert!(!start_refused(&st, key));
        // LG-81: Quarantine too
        assert_eq!(masks.first().map(|m| m.1 & 8), Some(8), "{masks:?}");
        st.rooms.get_mut(&key).unwrap().mode = 3;
        assert!(!start_refused(&st, key));
        // LG-85: Spy Hunt too
        assert_eq!(masks.first().map(|m| m.1 & 16), Some(16), "{masks:?}");
        st.rooms.get_mut(&key).unwrap().mode = 4;
        assert!(!start_refused(&st, key));
    }

    #[test]
    fn clan_channel_rooms_use_the_clan_table() {
        let st = &mut with_table();
        // clan channels sit above CLAN_CHANNEL_BASE
        let ch = crate::clan::CLAN_CHANNEL_BASE + 1;
        let (_, key) = host_in_room(st, 1, ch);
        assert!(start_refused(st, key), "clan BOMB (class 3)");
        st.rooms.get_mut(&key).unwrap().mode = 0;
        assert!(!start_refused(st, key), "clan TDM (class 4)");
    }
}
