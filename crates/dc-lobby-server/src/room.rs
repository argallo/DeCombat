//! Rooms: create, enter, leave, info / user lists, settings, team, ready, kick, host hand-over, invite.
// SPEC: PROTOCOL_2009.md §3.7 (flow and layouts)

use crate::game;
use crate::msg::{field, *};
use crate::state::{Core, HResult, Sid};
use crate::wire::{self, RoomRow};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RoomKey {
    pub channel: u32,
    pub no: u32,
}

/// Room state as the client labels it (room record +0x62, 0x3035 type 7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoomPhase {
    Waiting = 0,
    Starting = 1,
    Playing = 2,
}

/// Member states of 0x3042 (+0xa).
pub const MEMBER_WAITING: u32 = 0;
pub const MEMBER_READY: u32 = 1;
pub const MEMBER_STARTING: u32 = 2;
pub const MEMBER_PLAYING: u32 = 3;

#[derive(Clone, Debug)]
pub struct Member {
    pub account: u32,
    pub sid: Sid,
    pub team: u32,
    pub state: u32,
    pub sub: u32,
    /// Entry order (CD-08: the longest-present member is the next host).
    pub seq: u64,
}

#[derive(Clone, Debug)]
pub struct Room {
    pub key: RoomKey,
    pub title: String,
    pub password: String,
    pub mode: u32,
    pub map: u32,
    pub goal: u32,
    pub max_players: u32,
    pub mid_join: u32,
    pub ff_on: u32,
    pub killcam: u32,
    pub weapon_limit: u32,
    /// LG-274: the Hacks setting (`dc_wire::roomhacks` bits; 0 when the set offers none).
    pub hacks: u32,
    /// LG-306: the room's Hardcore setting.
    pub hardcore: bool,
    /// LG-305: the room plays Gun Game (its `mode` stays the free-for-all row on every 2009 message).
    pub gun_game: bool,
    /// LG-307: the room's Possession Arms.
    pub possession: bool,
    /// LG-305: the seed of the last start.
    pub gun_seed: u64,
    /// LG-305: the ladder of the last start (weapon rows, rung 0 first).
    pub ladder: Vec<u16>,
    pub host_badge: u32,
    pub host: u32,
    pub state: RoomPhase,
    pub members: Vec<Member>,
    /// Accounts kicked from this room instance (CD-27).
    pub kicked: BTreeSet<u32>,
    /// Bumped on every start / end so that stale countdown threads do nothing.
    pub gen: u64,
    pub game: Option<u64>,
    pub address_sent: bool,
    /// When the newest match started playing (LG-311); stays set after its end for a late `@result`.
    pub played_since: Option<std::time::Instant>,
}

impl Room {
    pub fn row(&self) -> RoomRow {
        RoomRow {
            no: self.key.no,
            title: self.title.clone(),
            locked: !self.password.is_empty(),
            host_id: self.host,
            map: self.map,
            mode: self.mode,
            players: self.members.len() as u32,
            max_players: self.max_players,
            state: self.state as u32,
            goal: self.goal,
            opt_mid_join: self.mid_join,
            opt_ff: self.ff_on,
            opt_kill_cam: self.killcam,
            weapon_limit: self.weapon_limit,
            host_badge: self.host_badge,
        }
    }

    pub fn member(&self, account: u32) -> Option<&Member> {
        self.members.iter().find(|m| m.account == account)
    }

    pub fn member_mut(&mut self, account: u32) -> Option<&mut Member> {
        self.members.iter_mut().find(|m| m.account == account)
    }

    pub fn sids(&self) -> Vec<Sid> {
        self.members.iter().map(|m| m.sid).collect()
    }

    pub fn count_team(&self, team: u32) -> usize {
        self.members.iter().filter(|m| m.team == team).count()
    }

    /// Team for a new member: the smaller team, ties to team 1.
    // DC-DEFAULT: CD-51 (proposed) a new member joins the team with fewer members, ties to team 1
    pub fn entry_team(&self) -> u32 {
        if self.count_team(2) < self.count_team(1) {
            2
        } else {
            1
        }
    }

    /// Next host when the host leaves: the longest-present remaining member.
    // DC-DEFAULT: CD-08 host hand-over on leave
    pub fn pick_host(&self) -> Option<u32> {
        self.members.iter().min_by_key(|m| m.seq).map(|m| m.account)
    }

    pub fn anyone_in_match(&self) -> bool {
        self.members
            .iter()
            .any(|m| m.state == MEMBER_STARTING || m.state == MEMBER_PLAYING)
    }
}

fn account_of(st: &Core, sid: Sid) -> Option<u32> {
    st.sessions
        .get(&sid)
        .filter(|s| s.in_lobby)
        .and_then(|s| s.account)
}

fn room_of(st: &Core, sid: Sid) -> Option<RoomKey> {
    st.sessions.get(&sid).and_then(|s| s.room)
}

impl Core {
    pub fn push_room(&mut self, key: RoomKey, frame: &[u8]) {
        if let Some(r) = self.rooms.get(&key) {
            let sids = r.sids();
            self.send_all(&sids, frame);
        }
    }

    pub fn push_lobby(&mut self, channel: u32, frame: &[u8]) {
        let sids = self.lobby_audience(channel);
        self.send_all(&sids, frame);
    }

    /// 0x3035 to the room and, for fields the lobby row shows, to the channel lobby.
    pub fn push_room_update(
        &mut self,
        key: RoomKey,
        f: u32,
        value: u32,
        text: &str,
        lobby_too: bool,
    ) {
        let frame = wire::room_update_push(key.no, f, value, text);
        self.push_room(key, &frame);
        if lobby_too {
            self.push_lobby(key.channel, &frame);
        }
    }

    /// The room's game port in the address push (0x3043 +0x28).
    pub fn game_port(&self, key: RoomKey) -> u32 {
        if let Some(g) = self
            .rooms
            .get(&key)
            .and_then(|r| r.game)
            .and_then(|g| self.games.get(&g))
        {
            return g.port as u32;
        }
        let base = self.opts.gameport as u32;
        if self.opts.port_per_room {
            // SPEC: LOBBY_HARNESS.md §1 --port-per-room: gameport + channel*100 + room
            (base + key.channel * 100 + key.no) & 0xffff
        } else {
            base
        }
    }
}

/// Puts a user back into the channel lobby list (0x3021 to the other lobby users).
fn announce_in_lobby(st: &mut Core, sid: Sid, account: u32) {
    let Some(ch) = st.sessions.get(&sid).and_then(|s| s.channel) else {
        return;
    };
    let rec = wire::lobby_user_add_push(&st.lobby_user(account));
    let others: Vec<Sid> = st
        .lobby_audience(ch)
        .into_iter()
        .filter(|&s| s != sid)
        .collect();
    st.send_all(&others, &rec);
}

/// Removes a user from its room (leave, kick, disconnect, new login). `reason` goes into 0x3034.
pub fn remove_member(st: &mut Core, sid: Sid, reason: u32, back_to_lobby: bool) -> HResult {
    let Some(key) = room_of(st, sid) else {
        return Ok(());
    };
    let Some(account) = st.sessions.get(&sid).and_then(|s| s.account) else {
        return Ok(());
    };
    let was_human = crate::lifecycle::is_human(st, account);
    if let Some(s) = st.sessions.get_mut(&sid) {
        s.room = None;
    }
    let Some(room) = st.rooms.get_mut(&key) else {
        return Ok(());
    };
    room.members.retain(|m| m.account != account);
    let empty = room.members.is_empty();
    let was_host = room.host == account;
    let remove = wire::room_user_remove_push(account, reason);
    if reason == wire::REMOVE_KICKED || reason == wire::REMOVE_KICKED_BY_GM {
        st.send(sid, remove.clone());
    }
    st.push_room(key, &remove);
    // DC-DEFAULT: LG-132 (proposed) a room whose last player (not a test bot) leaves ends its match and closes;
    // the bots go back to the lobby and the game port is freed
    let players_left = was_human
        && !empty
        && st
            .rooms
            .get(&key)
            .map(|r| {
                r.members
                    .iter()
                    .any(|m| crate::lifecycle::is_human(st, m.account))
            })
            .unwrap_or(false);
    if empty {
        close_room(st, key);
    } else if was_human && !players_left {
        if back_to_lobby {
            announce_in_lobby(st, sid, account);
        }
        st.note(&format!(
            "room {}-{}: account {} left (reason {}), the last player",
            key.channel, key.no, account, reason
        ));
        return crate::lifecycle::close_room_now(st, key, "its last player left, LG-132");
    } else {
        if was_host {
            hand_over(st, key, None);
        }
        let n = st
            .rooms
            .get(&key)
            .map(|r| r.members.len() as u32)
            .unwrap_or(0);
        st.push_room_update(key, field::PLAYERS, n, "", true);
        game::check_end(st, key);
    }
    if back_to_lobby {
        announce_in_lobby(st, sid, account);
    }
    st.note(&format!(
        "room {}-{}: account {} left (reason {})",
        key.channel, key.no, account, reason
    ));
    Ok(())
}

/// Moves the host role (`to` = requested new host, None = CD-08 pick). The new host's ready mark is cleared.
// DC-DEFAULT: CD-08 (host leaves), CD-28 (host names a new host with 0x38c type 2)
fn hand_over(st: &mut Core, key: RoomKey, to: Option<u32>) -> bool {
    // LG-319: a person before a bot, whoever came first: a bot does not start a person's room
    let person = st.rooms.get(&key).and_then(|r| {
        r.members
            .iter()
            .filter(|m| crate::lifecycle::is_human(st, m.account))
            .min_by_key(|m| m.seq)
            .map(|m| m.account)
    });
    let Some(room) = st.rooms.get_mut(&key) else {
        return false;
    };
    let new = match to {
        Some(a) if room.member(a).is_some() => a,
        Some(_) => return false,
        None => match person.or_else(|| room.pick_host()) {
            Some(a) => a,
            None => return false,
        },
    };
    room.host = new;
    let (state, sub) = match room.member_mut(new) {
        Some(m) => {
            if m.state == MEMBER_READY {
                m.state = MEMBER_WAITING;
            }
            (m.state, m.sub)
        }
        None => (MEMBER_WAITING, 0),
    };
    st.push_room_update(key, field::HOST, new, "", true);
    st.push_room(key, &wire::room_user_state_push(new, state, sub));
    true
}

pub fn close_room(st: &mut Core, key: RoomKey) {
    if let Some(room) = st.rooms.remove(&key) {
        if let Some(g) = room.game {
            game::quit_game(st, g);
        }
        st.push_lobby(key.channel, &wire::u32_push(LOBBY_ROOM_REMOVE_PUSH, key.no));
        st.note(&format!("room {}-{} closed", key.channel, key.no));
    }
}

fn on_create(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let req = wire::parse_room_create(p);
    let (Some(account), Some(channel)) = (
        account_of(st, sid),
        st.sessions.get(&sid).and_then(|s| s.channel),
    ) else {
        st.send(sid, wire::room_create_reply(false, 0, 0, 0));
        return Ok(());
    };
    remove_member(st, sid, wire::REMOVE_LEFT, false)?;
    let used: BTreeSet<u32> = st
        .rooms
        .keys()
        .filter(|k| k.channel == channel)
        .map(|k| k.no)
        .collect();
    // DC-DEFAULT: CD-25 lowest free room number, at most room.max_per_channel rooms
    let no = (0..st.rules.max_rooms_per_channel).find(|n| !used.contains(n));
    let Some(no) = no else {
        st.send(
            sid,
            wire::room_create_reply(false, wire::CODE_NO_MORE_ROOMS, 0, 0),
        );
        return Ok(());
    };
    let mode = req.mode.max(0) as u32;
    let (map, goal) = st.maps_of_channel(channel).mode_defaults(mode);
    let key = RoomKey { channel, no };
    st.join_seq += 1;
    // DC-DEFAULT: LG-274 (proposed) a new room starts on wallhacks when offered, else the first offered choice
    let hacks = dc_wire::roomhacks::default_value(game::hacks_offered(st.mods.as_ref()));
    let room = Room {
        key,
        title: req.title.chars().take(32).collect(),
        password: req.password.chars().take(8).collect(),
        mode,
        map,
        goal,
        // DC-DEFAULT: CD-56 (proposed) room size 1..room.max_players_cap, title <= 32, password <= 8 characters
        max_players: req
            .max_players
            .clamp(1, st.rules.max_players_cap.max(1) as i32) as u32,
        mid_join: st.rules.default_mid_join,
        ff_on: 0,
        killcam: 0,
        weapon_limit: 0,
        hacks,
        hardcore: false,
        gun_game: false,
        possession: false,
        gun_seed: 0,
        ladder: Vec::new(),
        host_badge: 0,
        host: account,
        state: RoomPhase::Waiting,
        members: vec![Member {
            account,
            sid,
            team: 1,
            state: MEMBER_WAITING,
            sub: 0,
            seq: st.join_seq,
        }],
        kicked: BTreeSet::new(),
        gen: 0,
        game: None,
        address_sent: false,
        played_since: None,
    };
    let row = room.row();
    st.rooms.insert(key, room);
    if let Some(s) = st.sessions.get_mut(&sid) {
        s.room = Some(key);
    }
    st.send(sid, wire::room_create_reply(true, 0, no, 0));
    // the host leaves the lobby list first, then the room appears (one 0x3031 per room)
    st.push_lobby(channel, &wire::u32_push(LOBBY_USER_REMOVE_PUSH, account));
    st.push_lobby(channel, &wire::lobby_room_add_push(&row));
    st.note(&format!(
        "room {}-{} created by {} mode {} map {}",
        channel, no, account, mode, map
    ));
    Ok(())
}

fn on_enter(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (no, pw) = wire::parse_room_enter(p);
    let (Some(account), Some(channel)) = (
        account_of(st, sid),
        st.sessions.get(&sid).and_then(|s| s.channel),
    ) else {
        st.send(sid, wire::room_enter_reply(false, wire::REASON_GENERIC, 0));
        return Ok(());
    };
    let key = RoomKey {
        channel,
        no: no.max(0) as u32,
    };
    if room_of(st, sid) == Some(key) {
        st.send(sid, wire::room_enter_reply(true, 0, 0));
        return Ok(());
    }
    // LG-319: a person may come into a full room when a bot makes way; a bot leaves the last seat free
    let human = crate::lifecycle::is_human(st, account);
    // DC-DEFAULT: CD-26 entry checks: kicked, wrong password, full, playing without the mid-match option
    let refusal = match st.rooms.get(&key) {
        None => Some(wire::REASON_GENERIC),
        Some(r) if r.kicked.contains(&account) => Some(wire::REASON_KICKED),
        Some(r) if !r.password.is_empty() && r.password != pw => Some(wire::REASON_WRONG_PASSWORD),
        Some(r)
            if r.members.len() as u32 >= r.max_players
                && !(human && crate::botpool::seat_can_open(st, key)) =>
        {
            Some(wire::REASON_GENERIC)
        }
        Some(r)
            if r.state != RoomPhase::Waiting && (r.mid_join == 0 || !st.rules.allow_late_join) =>
        {
            Some(wire::REASON_GENERIC)
        }
        Some(_) if !human && crate::botpool::last_seat_kept(st, key) => Some(wire::REASON_GENERIC),
        Some(_) => None,
    };
    if let Some(reason) = refusal {
        st.send(sid, wire::room_enter_reply(false, reason, 0));
        return Ok(());
    }
    // LG-319: the seat for a person comes first; a room of one bot is gone after that
    while st
        .rooms
        .get(&key)
        .is_some_and(|r| r.members.len() as u32 >= r.max_players)
    {
        if !crate::botpool::yield_one(st, key)? {
            break;
        }
    }
    if st
        .rooms
        .get(&key)
        .is_none_or(|r| r.members.len() as u32 >= r.max_players)
    {
        st.send(sid, wire::room_enter_reply(false, wire::REASON_GENERIC, 0));
        return Ok(());
    }
    remove_member(st, sid, wire::REMOVE_LEFT, false)?;
    st.join_seq += 1;
    let seq = st.join_seq;
    let (team, slot) = {
        let room = st.rooms.get_mut(&key).expect("checked above");
        let team = if st.rules.entry_team_balance {
            room.entry_team()
        } else {
            1
        };
        room.members.push(Member {
            account,
            sid,
            team,
            state: MEMBER_WAITING,
            sub: 0,
            seq,
        });
        (team, room.members.len() as u32 - 1)
    };
    if let Some(s) = st.sessions.get_mut(&sid) {
        s.room = Some(key);
    }
    st.send(sid, wire::room_enter_reply(true, 0, slot));
    let rec = wire::room_user_add_push(&st.seat_row(account, team, MEMBER_WAITING, 0));
    let others: Vec<Sid> = st.rooms[&key]
        .members
        .iter()
        .filter(|m| m.account != account)
        .map(|m| m.sid)
        .collect();
    st.send_all(&others, &rec);
    st.push_lobby(channel, &wire::u32_push(LOBBY_USER_REMOVE_PUSH, account));
    let n = st.rooms[&key].members.len() as u32;
    st.push_lobby(
        channel,
        &wire::room_update_push(key.no, field::PLAYERS, n, ""),
    );
    let others_n = others.len();
    if others_n > 0 {
        st.push_room(key, &wire::room_update_push(key.no, field::PLAYERS, n, ""));
    }
    st.note(&format!(
        "room {}-{}: account {} entered (team {})",
        channel, key.no, account, team
    ));
    if human {
        crate::botpool::after_person_joined(st, key)?;
    }
    Ok(())
}

fn on_leave(st: &mut Core, sid: Sid) -> HResult {
    let in_room = room_of(st, sid).is_some();
    // the host and the players who were ready at the start are held for the countdown: their leave is refused and
    // the countdown ends with them in the match (a disconnect still removes them)
    // DISASM: cshell+0x15f2b0 / cshell+0x15e970 (the 2009 client locks its room input during the countdown for a
    // member in state 2, so it sends no leave then); cshell+0x33400 (0x387 with +6 = 0: the client stays)
    // DC-DEFAULT: LG-177 (proposed) 0x386 from a member in state 2 (starting) of a starting room is answered
    // 0x387 {0} and changes nothing
    if starting_member(st, sid) {
        st.send(sid, wire::ok_reply(ROOM_LEAVE_REPLY, false, 0xa));
        return Ok(());
    }
    remove_member(st, sid, wire::REMOVE_LEFT, true)?;
    st.send(sid, wire::ok_reply(ROOM_LEAVE_REPLY, in_room, 0xa));
    Ok(())
}

/// Whether `sid` is a member in state 2 (starting) of a room in its start countdown (LG-177).
fn starting_member(st: &Core, sid: Sid) -> bool {
    let (Some(account), Some(key)) = (account_of(st, sid), room_of(st, sid)) else {
        return false;
    };
    st.rooms.get(&key).is_some_and(|r| {
        r.state == RoomPhase::Starting
            && r.member(account)
                .is_some_and(|m| m.state == MEMBER_STARTING)
    })
}

fn on_info(st: &mut Core, sid: Sid) -> HResult {
    match room_of(st, sid)
        .and_then(|k| st.rooms.get(&k))
        .map(|r| r.row())
    {
        Some(row) => {
            st.send(sid, wire::room_info_reply(&row));
            send_hacks(st, sid);
            if let Some(k) = room_of(st, sid) {
                crate::gungame::send_room(st, sid, k);
                // LG-306: nothing on an entry without the feature
                if crate::servers::channel_has_feature(st, k.channel, game::HARDCORE_FEATURE) {
                    let on = st.rooms.get(&k).is_some_and(|r| r.hardcore);
                    st.send(sid, dc_wire::roomhardcore::push_frame(on));
                }
                // LG-307: nothing on an entry without the feature
                if crate::servers::channel_has_feature(st, k.channel, game::POSSESSION_FEATURE) {
                    let on = st.rooms.get(&k).is_some_and(|r| r.possession);
                    st.send(sid, dc_wire::possession::push_frame(on));
                }
            }
        }
        None => st.send(sid, wire::ok_reply(ROOM_INFO_REPLY, false, 0x8c)),
    }
    Ok(())
}

/// LG-274: the room's Hacks setting to `sid` (our push 0x3f03 after the room info; nothing when the set offers none).
// SPEC: PROTOCOL_2009.md §1.2 (the 2009 dispatcher drops ids it has no case for)
fn send_hacks(st: &mut Core, sid: Sid) {
    let offered = game::hacks_offered(st.mods.as_ref());
    let Some(value) = room_of(st, sid)
        .and_then(|k| st.rooms.get(&k))
        .map(|r| r.hacks)
    else {
        return;
    };
    if offered != 0 {
        st.send(sid, dc_wire::roomhacks::push_frame(offered, value));
    }
}

/// LG-274: the host's 0x3f04. Taken while the room waits and when the value is one of the set's choices; every
/// member gets the room's value (a refused request only its sender, so its row shows the room's value again). A
/// change clears the other members' ready state like a 0x38c game setting (RM-1). Ignored when nothing is offered.
// DC-DEFAULT: LG-274 (proposed, project decision)
fn on_hacks(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let offered = game::hacks_offered(st.mods.as_ref());
    let (Some(account), Some(key), Some(value)) = (
        account_of(st, sid),
        room_of(st, sid),
        dc_wire::roomhacks::parse_request(p),
    ) else {
        return Ok(());
    };
    if offered == 0 {
        return Ok(());
    }
    let Some(room) = st.rooms.get_mut(&key) else {
        return Ok(());
    };
    let ok = room.host == account
        && room.state == RoomPhase::Waiting
        && dc_wire::roomhacks::allowed(offered, value);
    if !ok {
        let now = room.hacks;
        st.send(sid, dc_wire::roomhacks::push_frame(offered, now));
        st.note(&format!(
            "room {}-{}: hacks {} from account {} refused (offered {})",
            key.channel, key.no, value, account, offered
        ));
        return Ok(());
    }
    let changed = room.hacks != value;
    room.hacks = value;
    st.push_room(key, &dc_wire::roomhacks::push_frame(offered, value));
    if changed {
        st.note(&format!(
            "room {}-{}: hacks {} (account {})",
            key.channel, key.no, value, account
        ));
        if st.rules.settings_change_clears_ready {
            clear_ready(st, key, account);
        }
    }
    Ok(())
}

/// LG-305: the host's Gun Game request (0x3f06). On an entry without the feature nothing answers (the lobby logs the
/// refusal; the room is unchanged). On a featured entry only the host of a waiting free-for-all room changes it, any
/// other request gets the room's value back. A change clears the other members' ready state (RM-1) and drops a weapon
/// limit (the ladder decides the weapons).
fn on_gun_game(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (account, key, on) = (
        account_of(st, sid),
        room_of(st, sid),
        dc_wire::gungame::parse_request(p),
    );
    let (Some(account), Some(key), Some(on)) = (account, key, on) else {
        return Ok(());
    };
    if !crate::gungame::offered(st, key.channel) {
        st.note(&format!(
            "room {}-{}: gun game request from account {} refused (not offered here)",
            key.channel, key.no, account
        ));
        return Ok(());
    }
    let class = st
        .rooms
        .get(&key)
        .and_then(|r| crate::lobby::mode_class_of_row(st, r.mode, false));
    let Some(room) = st.rooms.get_mut(&key) else {
        return Ok(());
    };
    let allowed = room.host == account
        && room.state == RoomPhase::Waiting
        && (!on || class == Some(crate::gungame::FFA_CLASS))
        // DC-DEFAULT: LG-307 (proposed) the guns of the two rules conflict
        && (!on || !room.possession);
    if !allowed {
        let now = room.gun_game;
        st.send(sid, dc_wire::gungame::room_frame(key.no, now, &[]));
        return Ok(());
    }
    let changed = room.gun_game != on;
    room.gun_game = on;
    let limited = on && room.weapon_limit != 0;
    if limited {
        room.weapon_limit = 0;
    }
    if limited {
        st.push_room_update(key, field::WEAPON_LIMIT, 0, "", true);
    }
    crate::gungame::push_change(st, key, on);
    if changed {
        st.note(&format!(
            "room {}-{}: gun game {} (account {})",
            key.channel, key.no, on, account
        ));
        if st.rules.settings_change_clears_ready {
            clear_ready(st, key, account);
        }
    }
    Ok(())
}

/// LG-306: the host's Hardcore request (0x3f09). On an entry without the feature nothing answers (logged). Else only
/// the host of a waiting room changes it, every member hears the value, a refusal gets the room's value back; a change
/// clears the other members' ready state (RM-1).
fn on_hardcore(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (Some(account), Some(key), Some(on)) = (
        account_of(st, sid),
        room_of(st, sid),
        dc_wire::roomhardcore::parse(p),
    ) else {
        return Ok(());
    };
    if !crate::servers::channel_has_feature(st, key.channel, game::HARDCORE_FEATURE) {
        st.note(&format!(
            "room {}-{}: hardcore request from account {} refused (not offered here)",
            key.channel, key.no, account
        ));
        return Ok(());
    }
    let Some(room) = st.rooms.get_mut(&key) else {
        return Ok(());
    };
    if room.host != account || room.state != RoomPhase::Waiting {
        let now = room.hardcore;
        st.send(sid, dc_wire::roomhardcore::push_frame(now));
        return Ok(());
    }
    let changed = room.hardcore != on;
    room.hardcore = on;
    st.push_room(key, &dc_wire::roomhardcore::push_frame(on));
    if changed {
        st.note(&format!(
            "room {}-{}: hardcore {} (account {})",
            key.channel, key.no, on, account
        ));
        if st.rules.settings_change_clears_ready {
            clear_ready(st, key, account);
        }
    }
    Ok(())
}

/// LG-307: the host's Possession request (0x3f0b). On an entry without the feature nothing answers (logged). Else only
/// the host of a waiting room changes it, on a mode that allows it and not in a Gun Game room; a refusal gets the room's
/// value back. It is a row of the exclusive Arms list: turning it on sets the ROM limit to 0.
fn on_possession(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (Some(account), Some(key), Some(on)) = (
        account_of(st, sid),
        room_of(st, sid),
        dc_wire::possession::parse(p),
    ) else {
        return Ok(());
    };
    if !crate::servers::channel_has_feature(st, key.channel, game::POSSESSION_FEATURE) {
        st.note(&format!(
            "room {}-{}: possession request from account {} refused (not offered here)",
            key.channel, key.no, account
        ));
        return Ok(());
    }
    let class = st
        .rooms
        .get(&key)
        .and_then(|r| crate::lobby::mode_class_of_row(st, r.mode, false));
    let Some(room) = st.rooms.get_mut(&key) else {
        return Ok(());
    };
    let allowed = room.host == account
        && room.state == RoomPhase::Waiting
        && (!on || (!room.gun_game && class.is_some_and(dc_wire::possession::class_allows)));
    if !allowed {
        let now = room.possession;
        st.send(sid, dc_wire::possession::push_frame(now));
        return Ok(());
    }
    let changed = room.possession != on;
    room.possession = on;
    let limited = on && room.weapon_limit != 0;
    if limited {
        room.weapon_limit = 0;
        st.push_room_update(key, field::WEAPON_LIMIT, 0, "", true);
    }
    st.push_room(key, &dc_wire::possession::push_frame(on));
    if changed {
        st.note(&format!(
            "room {}-{}: possession {} (account {})",
            key.channel, key.no, on, account
        ));
        if st.rules.settings_change_clears_ready {
            clear_ready(st, key, account);
        }
    }
    Ok(())
}

/// LG-307: a ROM Arms pick or a mode without it ends the room's Possession.
fn drop_possession(st: &mut Core, key: RoomKey) {
    if let Some(r) = st.rooms.get_mut(&key).filter(|r| r.possession) {
        r.possession = false;
        st.push_room(key, &dc_wire::possession::push_frame(false));
    }
}

/// LG-305: a mode change ends the room's Gun Game (the client asks again for the "Gun Game" row).
fn drop_gun_game(st: &mut Core, key: RoomKey) {
    if let Some(r) = st.rooms.get_mut(&key).filter(|r| r.gun_game) {
        r.gun_game = false;
        crate::gungame::push_change(st, key, false);
    }
}

fn on_users(st: &mut Core, sid: Sid) -> HResult {
    let Some(room) = room_of(st, sid).and_then(|k| st.rooms.get(&k)) else {
        st.send(sid, wire::room_users_reply(&[]));
        return Ok(());
    };
    let members = room.members.clone();
    let users: Vec<_> = members
        .iter()
        .map(|m| st.seat_row(m.account, m.team, m.state, m.sub))
        .collect();
    st.send(sid, wire::room_users_reply(&users));
    Ok(())
}

fn on_setting(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let req = wire::parse_setting_change(p);
    let account = account_of(st, sid);
    let key = room_of(st, sid);
    let (Some(account), Some(key)) = (account, key) else {
        st.send(sid, wire::ok_reply(ROOM_SETTING_CHANGE_REPLY, false, 0xa));
        return Ok(());
    };
    // MAP-35 / D12: a clan room picks from the ROM rows only (clan rooms do not play community maps; the plain
    // table's community rows come after the ROM's)
    let clan = crate::clan::is_clan_channel(key.channel);
    let (is_host, waiting, members, map_count, mode_count) = {
        let r = &st.rooms[&key];
        (
            r.host == account,
            r.state == RoomPhase::Waiting,
            r.members.len() as u32,
            if clan {
                st.rom.map_count()
            } else {
                st.maps_of_channel(key.channel).map_count()
            },
            st.rom.mode_count(false),
        )
    };
    // only the host changes settings, and only while the room waits (0 -> `res 0x4e2`); handing the host role
    // over is allowed in any state (CD-28 has no state condition, and the host role only matters for Start)
    // DC-DEFAULT: CD-57 (proposed) room.handover_any_state
    let mut ok = is_host && (waiting || (req.field == field::HOST && st.rules.handover_any_state));
    if ok {
        let v = req.value;
        ok = match req.field {
            field::HOST => v != account && hand_over(st, key, Some(v)),
            field::LOCKED => {
                let pw: String = req
                    .text
                    .clone()
                    .unwrap_or_default()
                    .chars()
                    .take(8)
                    .collect();
                let room = st.rooms.get_mut(&key).expect("room");
                room.password = pw.clone();
                let locked = !pw.is_empty() as u32;
                st.push_room_update(key, field::LOCKED, locked, "", true);
                st.push_room_update(key, field::PASSWORD, 0, &pw, false);
                true
            }
            field::MAP if map_count == 0 || (v as usize) < map_count => {
                set_field(st, key, req.field, v) && {
                    // DC-DEFAULT: MAP-37 (proposed) a community map's record goal for the room's mode becomes
                    // the room's goal when the map is picked (the echo below shows it; the host may change it)
                    let mode = st.rooms.get(&key).map(|r| r.mode).unwrap_or(0);
                    if let Some(goal) = st.maps_of_channel(key.channel).record_goal(v, mode) {
                        if let Some(r) = st.rooms.get_mut(&key) {
                            r.goal = goal;
                        }
                    }
                    echo_linked_fields(st, key, req.field);
                    true
                }
            }
            field::MODE if mode_count == 0 || (v as usize) < mode_count => {
                set_field(st, key, req.field, v) && {
                    drop_gun_game(st, key);
                    // DC-DEFAULT: LG-307 (proposed) not in Fireteam, Spy Hunt, Quarantine
                    let class = crate::lobby::mode_class_of_row(st, v, false);
                    if !class.is_some_and(dc_wire::possession::class_allows) {
                        drop_possession(st, key);
                    }
                    reset_for_mode(st, key, v, mode_count);
                    echo_linked_fields(st, key, req.field);
                    true
                }
            }
            field::MAX_PLAYERS if (members.max(1)..=st.rules.max_players_cap).contains(&v) => {
                set_field(st, key, req.field, v)
            }
            // DC-DEFAULT: LG-305 (proposed) the ladder decides the weapons: no limit in a Gun Game room
            field::WEAPON_LIMIT if st.rooms[&key].gun_game => false,
            // DC-DEFAULT: LG-307 (proposed) one exclusive Arms choice: a ROM pick replaces Possession
            field::WEAPON_LIMIT if st.rooms[&key].possession => {
                drop_possession(st, key);
                set_field(st, key, req.field, v)
            }
            field::GOAL
            | field::OPT_MID_JOIN
            | field::OPT_FRIENDLY_FIRE
            | field::OPT_KILL_CAM
            | field::WEAPON_LIMIT => set_field(st, key, req.field, v),
            // DC-DEFAULT: LG-203 (proposed) only a room badge (Elite Moderator) holder or a GM sets the room's badge level
            field::HOST_BADGE if badge_user(st, account) => set_field(st, key, req.field, v),
            _ => false,
        };
    }
    // DC-DEFAULT: RM-1 (proposed) a game setting change clears every other member's ready state (0x3042 state 0 each);
    // handing over the host role and the password are not game settings
    if ok
        && st.rules.settings_change_clears_ready
        && waiting
        && req.field != field::HOST
        && req.field != field::LOCKED
    {
        clear_ready(st, key, account);
    }
    st.send(sid, wire::ok_reply(ROOM_SETTING_CHANGE_REPLY, ok, 0xa));
    Ok(())
}

/// Sets every ready member except `except` back to waiting and pushes their 0x3042.
// DC-DEFAULT: RM-1 (proposed)
fn clear_ready(st: &mut Core, key: RoomKey, except: u32) {
    let Some(room) = st.rooms.get_mut(&key) else {
        return;
    };
    let mut cleared = Vec::new();
    for m in room.members.iter_mut() {
        if m.account != except && m.state == MEMBER_READY {
            m.state = MEMBER_WAITING;
            cleared.push((m.account, m.sub));
        }
    }
    for (a, sub) in cleared {
        st.push_room(key, &wire::room_user_state_push(a, MEMBER_WAITING, sub));
    }
}

fn set_field(st: &mut Core, key: RoomKey, f: u32, v: u32) -> bool {
    let Some(room) = st.rooms.get_mut(&key) else {
        return false;
    };
    match f {
        field::MAP => room.map = v,
        field::MODE => room.mode = v,
        field::MAX_PLAYERS => room.max_players = v,
        field::GOAL => room.goal = v,
        field::OPT_MID_JOIN => room.mid_join = (v != 0) as u32,
        field::OPT_FRIENDLY_FIRE => room.ff_on = (v != 0) as u32,
        field::OPT_KILL_CAM => room.killcam = (v != 0) as u32,
        field::WEAPON_LIMIT => room.weapon_limit = v,
        field::HOST_BADGE => room.host_badge = v,
        _ => return false,
    }
    let stored = match f {
        field::OPT_MID_JOIN | field::OPT_FRIENDLY_FIRE | field::OPT_KILL_CAM => (v != 0) as u32,
        _ => v,
    };
    st.push_room_update(key, f, stored, "", true);
    true
}

/// A mode change puts the room's goal on the new mode's `DefaultGoal`: the old mode's goal is in another unit and
/// often outside the new mode's goal list (an Elimination room's 100 kills switched to One Man Army kept 100, and
/// the FFA match ran to that count, not to a goal the One Man Army list offers). When the room's map has no GMS for
/// the new mode (a Team Deathmatch map switched to Fireteam), the map moves to the mode's default map as well. The
/// echo of the linked fields that follows tells the room the new goal and map.
// DC-DEFAULT: LG-65 (proposed) a mode change resets the goal to the ROM mode row's `DefaultGoal`, as room creation
// does (`mode_defaults`); a later goal request (0x38c type 8) sets the value the host picks
// DC-DEFAULT: LG-100 (proposed, playtest report "switching to fireteam doesn't force the map change to a valid one") a
// mode change keeps the room's map when that map has a GMS for the new mode, otherwise the room takes the mode's
// default map under the LG-2 rule (`mode_defaults`: the row's default map, or the first enabled map with the GMS)
fn reset_for_mode(st: &mut Core, key: RoomKey, mode: u32, mode_count: usize) {
    if (mode as usize) >= mode_count {
        return;
    }
    let maps = st.maps_of_channel(key.channel);
    let (default_map, goal) = maps.mode_defaults(mode);
    let keep_map = st
        .rooms
        .get(&key)
        .is_some_and(|r| maps.map_count() == 0 || maps.map_plays_mode(r.map, mode));
    // DC-DEFAULT: MAP-37 (proposed) a kept community map lends its record goal for the new mode, when it has one
    let record_goal = st
        .rooms
        .get(&key)
        .filter(|_| keep_map)
        .and_then(|r| maps.record_goal(r.map, mode));
    if let Some(r) = st.rooms.get_mut(&key) {
        r.goal = record_goal.unwrap_or(goal);
        if !keep_map {
            r.map = default_map;
        }
    }
}

/// The 2009 waiting room marks a map or mode request (0x38c type 3 / 4) as pending for the four linked fields map,
/// mode, max players and goal at once and keeps every option list of those fields shut until a 0x3035 of each type
/// has cleared its own mark; a 0x38d {1} clears none. After an accepted map or mode change we therefore echo the
/// other three fields with the room's current values (the changed field itself was pushed by `set_field`).
// EVIDENCE: cshell+0x1614f0 (the type 3 and type 4 request paths set the pending bytes of types 3, 4, 6 and 8; the
// type 6 / 8 / 0xe paths set only their own byte; a set byte blocks the request), cshell+0x168680 (0x3035 clears the
// byte of its own type), cshell+0x162370 (0x38d clears all of them only when +6 is 0)
// OBSERVED: capture 20261025_101848_menu-frida8 (lobby TCP: 0x38c type 3 -> 0x38d {1} and
// 0x3035 types 3, 4, 6, 8; the following type 8 request is sent) vs 20261025_104223_rust5-auto (0x3035 type 3
// only; the type 8 request never leaves the client, menu agent timeout)
fn echo_linked_fields(st: &mut Core, key: RoomKey, changed: u32) {
    let Some(room) = st.rooms.get(&key) else {
        return;
    };
    let linked = [
        (field::MAP, room.map),
        (field::MODE, room.mode),
        (field::MAX_PLAYERS, room.max_players),
        (field::GOAL, room.goal),
    ];
    for (f, v) in linked {
        if f != changed {
            st.push_room_update(key, f, v, "", true);
        }
    }
}

// DC-DEFAULT: CD-27 only the host kicks; the kicked user gets 0x3034 reason 1 and may not re-enter this room
// DC-DEFAULT: LG-173 a GM account kicks in any room it is in (reason 2); a GM target is refused with 0x38f 0x82
// The 2009 client sends the same 0x38e for the member menu's "Kick User" and for the operator `/k NAME` line: it
// resolves the name in its own member list (EVIDENCE: cshell+0x15fef0 and cshell+0x15d740 -> cshell+0x18f280)
/// True when the account may use the room badge field and the kick: it has a room badge item in use, a GM does, and
/// the operator can switch the rule off (`room.kick_needs_badge`).
// DC-DEFAULT: LG-203 (proposed)
pub(crate) fn badge_user(st: &Core, account: u32) -> bool {
    !st.rules.kick_needs_badge || st.room_badge(account) || crate::gm::is_gm(st, account)
}

fn on_kick(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (target, _char_no) = wire::parse_kick(p);
    let (Some(account), Some(key)) = (account_of(st, sid), room_of(st, sid)) else {
        st.send(sid, wire::room_kick_reply(false, 0));
        return Ok(());
    };
    let by_gm = crate::gm::is_gm(st, account);
    let target_sid = {
        let r = &st.rooms[&key];
        // DC-DEFAULT: LG-203 (proposed) the host kicks only with a room badge item in use (the 2009 menu offers Kick
        // User only with the badge level, cshell+0x15dde0)
        let host_may = r.host == account && badge_user(st, account);
        if (!host_may && !by_gm) || target == account {
            None
        } else {
            r.member(target).map(|m| m.sid)
        }
    };
    let Some(tsid) = target_sid else {
        st.send(sid, wire::room_kick_reply(false, 0));
        return Ok(());
    };
    if crate::gm::kick_immune(st, target) {
        st.send(sid, wire::room_kick_reply(false, wire::KICK_REFUSED_GM));
        st.note(&format!(
            "room kick refused: account {} is a GM (kick by account {})",
            target, account
        ));
        return Ok(());
    }
    if let Some(r) = st.rooms.get_mut(&key) {
        r.kicked.insert(target);
    }
    st.send(sid, wire::room_kick_reply(true, 0));
    let reason = if by_gm {
        wire::REMOVE_KICKED_BY_GM
    } else {
        wire::REMOVE_KICKED
    };
    remove_member(st, tsid, reason, true)?;
    Ok(())
}

/// Mode classes played without teams (one ranking list): One Man Army and its forms.
// EVIDENCE: cshell+0x137720 (classes 6 / 9 / 0xb take the single sorted list)
pub const FREE_FOR_ALL_CLASSES: [u32; 3] = [6, 9, 11];

fn on_team(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let dest = wire::parse_team_change(p);
    let (Some(account), Some(key)) = (account_of(st, sid), room_of(st, sid)) else {
        st.send(sid, wire::team_change_reply(false, 0));
        return Ok(());
    };
    // a free-for-all room has no teams to change to: refused without a message, as a clan room refuses it
    // EVIDENCE: cshell+0x137720 / cshell+0x132b80 (classes 6 / 9 / 0xb: one ranking, no team score pair)
    // DC-DEFAULT: LG-10 (amended, proposed) 0x390 in a room of mode class 6 / 9 / 11 is answered 0x391 {fail,
    // reason 0} and the member keeps his team (CLI-782; the 2009 client offers no team button there)
    let class = st.rooms.get(&key).and_then(|r| {
        crate::lobby::mode_class_of_row(st, r.mode, crate::clan::is_clan_channel(key.channel))
    });
    if class.is_some_and(|c| FREE_FOR_ALL_CLASSES.contains(&c)) {
        st.send(sid, wire::team_change_reply(false, 0));
        return Ok(());
    }
    let ok = {
        let r = &st.rooms[&key];
        let cur = r.member(account).map(|m| m.team).unwrap_or(0);
        let half = r.max_players.div_ceil(2) as usize;
        // a team is full at half the room (reason 0x27 -> `res 0x1063`)
        (dest == 1 || dest == 2)
            && r.state == RoomPhase::Waiting
            && (cur == dest || r.count_team(dest) < half.max(1))
    };
    if !ok {
        st.send(sid, wire::team_change_reply(false, 0x27));
        return Ok(());
    }
    if let Some(m) = st.rooms.get_mut(&key).and_then(|r| r.member_mut(account)) {
        m.team = dest;
    }
    st.send(sid, wire::team_change_reply(true, 0));
    st.push_room(key, &wire::room_team_push(account, dest));
    Ok(())
}

/// 0x392: ready / start / late join (kind 0) and the shopping / away sub-state (kind 1, no reply: the client
/// does not wait for one, EVIDENCE: cshell+0x15f470 and cshell+0x1608b0 send kind 1 without setting the pending
/// flag that the 0x393 handler cshell+0x15f0d0 clears).
// DC-DEFAULT: CD-24 only the host starts, only from the waiting state; the host cannot mark ready; a ready
// request while the room plays is a late join when the room option and room.allow_late_join allow it
fn on_flag(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (kind, value) = wire::parse_user_flag(p);
    let (Some(account), Some(key)) = (account_of(st, sid), room_of(st, sid)) else {
        if kind == 0 {
            st.send(sid, wire::user_flag_reply(false, 0, 0));
        }
        return Ok(());
    };
    let (is_host, state, mid_join, member_state) = {
        let r = &st.rooms[&key];
        (
            r.host == account,
            r.state,
            r.mid_join,
            r.member(account).map(|m| m.state).unwrap_or(0),
        )
    };
    if kind == 1 {
        let sub = value.min(2);
        let st_now = if let Some(m) = st.rooms.get_mut(&key).and_then(|r| r.member_mut(account)) {
            m.sub = sub;
            // DC-DEFAULT: LG-260 (proposed) a ready member who goes shopping is no longer ready: the room shows his
            // shopping text, which 2009 draws only with state 0 (cshell+0x163d00); the 2009 client un-readies itself
            // before the flag (cshell+0x15d610 from cshell+0x164810), this covers a client that does not
            if sub == 1 && m.state == MEMBER_READY {
                m.state = MEMBER_WAITING;
            }
            m.state
        } else {
            return Ok(());
        };
        st.push_room(key, &wire::room_user_state_push(account, st_now, sub));
        return Ok(());
    }
    if kind != 0 {
        st.send(sid, wire::user_flag_reply(false, 0, 0));
        return Ok(());
    }
    // LG-66: Ready, Start and a late join need equipped weapons that pass the room's weapon limit
    if (value == 1 || value == 2) && !arms_ok(st, key, account) {
        st.note(&format!(
            "room {}-{}: account {} refused (value {}): the equipped weapons fail weapon limit {}",
            key.channel, key.no, account, value, st.rooms[&key].weapon_limit
        ));
        // MON-3
        crate::opsmon::event(
            st,
            account,
            crate::opsmon::kind::READY_REFUSED,
            &format!(
                "room {}-{}: {} refused (value {value}): the equipped weapons fail weapon limit {}",
                key.channel,
                key.no,
                if value == 2 { "start" } else { "ready" },
                st.rooms[&key].weapon_limit
            ),
        );
        // DC-DEFAULT: LG-342 (proposed, project decision) the refusal names its cause, so our client shows the
        // weapon-limit box instead of a start notice
        st.send(
            sid,
            wire::user_flag_reply(
                false,
                dc_wire::flagrefusal::REASON_WEAPON_LIMIT,
                dc_wire::flagrefusal::KIND_WEAPON_LIMIT,
            ),
        );
        return Ok(());
    }
    match (state, value) {
        (RoomPhase::Waiting, 2) if is_host => game::start(st, sid, key)?,
        // only the host starts (CD-24)
        (RoomPhase::Waiting, 2) => st.send(sid, wire::user_flag_reply(false, 0, 0)),
        (RoomPhase::Waiting, 0) | (RoomPhase::Waiting, 1) => {
            if is_host && value == 1 {
                st.send(sid, wire::user_flag_reply(false, 0, 0));
            } else {
                let new = if value == 1 {
                    MEMBER_READY
                } else {
                    MEMBER_WAITING
                };
                let sub =
                    if let Some(m) = st.rooms.get_mut(&key).and_then(|r| r.member_mut(account)) {
                        m.state = new;
                        m.sub
                    } else {
                        0
                    };
                st.send(sid, wire::user_flag_reply(true, 0, 0));
                st.push_room(key, &wire::room_user_state_push(account, new, sub));
            }
        }
        // the host's Start needs a waiting room (HARNESS: lobby_harness::test_rejoin step 1)
        (_, 2) if is_host => st.send(sid, wire::user_flag_reply(false, 0, 0)),
        (_, 1) | (_, 2) => {
            if member_state == MEMBER_STARTING || member_state == MEMBER_PLAYING {
                st.send(sid, wire::user_flag_reply(true, 0, 0));
            } else if mid_join != 0 && st.rules.allow_late_join {
                game::late_join(st, sid, key, account)?;
            } else {
                st.send(
                    sid,
                    wire::user_flag_reply(
                        false,
                        wire::REASON_JOIN_FAILED,
                        wire::FLAG_REPLY_KIND_JOIN,
                    ),
                );
            }
        }
        _ => st.send(sid, wire::user_flag_reply(true, 0, 0)),
    }
    Ok(())
}

/// Equipment slots holding weapons: the four weapon slots and the four backpack weapon slots of the 0x323 view.
// SPEC: DSERVER_PIPE.md §12.A (slots 0..3 weapons, 10..13 backpack weapons)
const WEAPON_EQUIP_SLOTS: [std::ops::RangeInclusive<i64>; 2] = [0..=3, 10..=13];

/// Whether `account`'s equipped weapons pass the weapon limit of room `key` (the 2009 client's own check before
/// Ready / Start, `dc_bute::arms::entry_ok`), counted on the weapons the match would equip (`game::match_items`: a
/// passing inventory primary stands in for a failing slot-0 one, LG-67). Without the ROM item table (no weapon types
/// known) every loadout passes.
// EVIDENCE: cshell+0x160f70 (the check on the waiting room's Start / Ready command and before joining a running match)
// DC-DEFAULT: LG-66 (proposed) the lobby repeats the client's check and refuses a failing Ready / Start / late join
// with 0x393 {fail, 0, 0}; the 2009 client does not send it at all
// DC-DEFAULT: LG-67 (proposed) a player with a passing weapon anywhere in the inventory is not refused
fn arms_ok(st: &Core, key: RoomKey, account: u32) -> bool {
    let limit = st.rooms.get(&key).map_or(0, |r| r.weapon_limit);
    if limit == 0 || st.rom.weapon_kinds.is_empty() {
        return true;
    }
    let Ok(items) = crate::db::items(&st.db, account) else {
        return true;
    };
    let items = crate::inventory::visible_items(st, account, items);
    let items = crate::game::match_items(st, limit, &items);
    let types = items
        .iter()
        .filter(|i| WEAPON_EQUIP_SLOTS.iter().any(|r| r.contains(&i.slot)))
        .filter_map(|i| st.rom.weapon_kinds.get(&i.item_no).copied());
    dc_bute::arms::entry_ok(limit, types)
}

// DC-DEFAULT: CD-44 invite all = a room invite to every channel-lobby user who is not in a room
fn on_invite_all(st: &mut Core, sid: Sid) -> HResult {
    let (Some(account), Some(key)) = (account_of(st, sid), room_of(st, sid)) else {
        st.send(sid, wire::ok_reply(ROOM_INVITE_ALL_REPLY, false, 0xa));
        return Ok(());
    };
    let host = st.character(account).map(|c| c.nick).unwrap_or_default();
    let pw = st.rooms[&key].password.clone();
    st.send(sid, wire::ok_reply(ROOM_INVITE_ALL_REPLY, true, 0xa));
    st.push_lobby(key.channel, &wire::room_invite_push(key.no, &host, &pw));
    Ok(())
}

/// Room messages; returns false when `id` is not one of them.
pub fn dispatch(st: &mut Core, sid: Sid, id: u16, p: &[u8]) -> Result<bool, crate::state::Fault> {
    match id {
        ROOM_CREATE => on_create(st, sid, p)?,
        ROOM_ENTER => on_enter(st, sid, p)?,
        ROOM_LEAVE => on_leave(st, sid)?,
        ROOM_INFO => on_info(st, sid)?,
        ROOM_USERS => on_users(st, sid)?,
        ROOM_SETTING_CHANGE => on_setting(st, sid, p)?,
        ROOM_KICK => on_kick(st, sid, p)?,
        TEAM_CHANGE => on_team(st, sid, p)?,
        ROOM_USER_FLAG => on_flag(st, sid, p)?,
        MATCH_EXIT => {
            if let (Some(a), Some(k)) = (account_of(st, sid), room_of(st, sid)) {
                // SPEC: PROTOCOL_2009.md §3.7 (0x394 +6 u32 phase: 0 = left the match, 1 = the start failed)
                let phase = p
                    .get(6..10)
                    .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                game::back_to_room(st, k, a, phase);
            }
        }
        ROOM_INVITE_ALL => on_invite_all(st, sid)?,
        dc_wire::roomhacks::ROOM_HACKS_REQUEST => on_hacks(st, sid, p)?,
        dc_wire::gungame::REQUEST => on_gun_game(st, sid, p)?,
        dc_wire::roomhardcore::REQUEST => on_hardcore(st, sid, p)?,
        dc_wire::possession::REQUEST => on_possession(st, sid, p)?,
        _ => return Ok(false),
    }
    Ok(true)
}
