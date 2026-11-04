//! Byte layouts of every message the core server sends, and parsers for what it receives.
//! Offsets count from the start of the inner header, so the first field is +6 (SPEC: PROTOCOL_2009.md §1.2).
//! Framing is `dc_wire::tcp` (`PktWriter`: frame id = inner id, inner length patched).

use crate::msg::*;
use dc_wire::{PktReader, PktWriter};

/// Finishes a writer padded to at least `min` payload bytes and returns the whole frame.
fn done(mut w: PktWriter, min: usize) -> Vec<u8> {
    if w.len() < min {
        w.at(min);
    }
    w.into_frame()
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

/// Generic `{+6 u32 ok}` reply padded to `min` bytes.
pub fn ok_reply(id: u16, ok: bool, min: usize) -> Vec<u8> {
    let mut w = PktWriter::new(id);
    w.u32(ok as u32);
    done(w, min)
}

// ---------------------------------------------------------------------------------------------------------------
// auth server (§2.2)
// ---------------------------------------------------------------------------------------------------------------

/// 0x207 `file_check_reply`: +6 u32 ok (0 = the client shuts down).
// SPEC: PROTOCOL_2009.md §2.2 0x207 (EVIDENCE: cshell+0x38ce0)
pub fn file_check_reply(ok: bool) -> Vec<u8> {
    ok_reply(FILE_CHECK_REPLY, ok, 0xa)
}

/// Parsed 0x200 `login_request`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginRequest {
    pub token: String,
    pub password: String,
    pub adapter: String,
    pub partner: i32,
}

// SPEC: PROTOCOL_2009.md §2.2 0x200 (EVIDENCE: cshell+0x191e30)
pub fn parse_login(p: &[u8]) -> LoginRequest {
    let mut r = PktReader::new(p);
    let token = r.at(6).wstr(4096);
    let password = r.at(0x2006).wstr(17);
    let adapter = r.at(0x2028).wstr(20);
    let partner = r.at(0x2050).i32();
    LoginRequest {
        token,
        password,
        adapter,
        partner,
    }
}

#[derive(Debug, Clone, Default)]
pub struct LoginReply {
    pub ok: bool,
    pub reason: u32,
    pub sub_error: u32,
    pub account: u32,
    pub exp: u32,
    pub clan_id: u32,
    pub kills: u32,
    pub deaths: u32,
    pub privilege: i32,
    pub days: u32,
}

/// 0x201 `login_reply` (>= 0x2e).
// SPEC: PROTOCOL_2009.md §2.2 0x201 (EVIDENCE: cshell+0x42450)
pub fn login_reply(v: &LoginReply) -> Vec<u8> {
    let mut w = PktWriter::new(LOGIN_REPLY);
    w.u32(v.ok as u32)
        .u32(v.reason)
        .u32(v.sub_error)
        .u32(v.account)
        .u32(v.exp)
        .u32(v.clan_id);
    w.u32(v.kills).u32(v.deaths).i32(v.privilege).u32(v.days);
    done(w, 0x2e)
}

/// 0x201 reason: no character yet -> nickname screen.
pub const REASON_NO_CHARACTER: u32 = 0x0003_001e;
/// 0x201 reason: wrong password (`res 0x443`).
pub const REASON_BAD_PASSWORD: u32 = 0x0003_0025;

/// Highest grade a `server_entry` admits (+0xa4). The ROM rank table has far fewer ranks, so every account
/// qualifies.
// REFERENCE / PROVEN-CAPTURE: 0x63 at entry +0xa4 of 0x2000 seq 7 in captures 20261025_065016_tdm6 and
// 20261025_002236_tdm4; our 0 at the same offset (rust1 seq 7, exp 225000) made the client refuse the list.
pub const SERVER_MAX_GRADE: u32 = 99;

/// One `server_entry` (0xc0 bytes) of 0x2000 / 0x3b1.
#[derive(Debug, Clone)]
pub struct ServerEntry {
    pub id: u32,
    pub name: String,
    pub ip: String,
    pub port: u32,
    pub open: bool,
    pub capacity: u32,
    pub current: u32,
    /// Operator slots: +0x98 capacity, +0x9c current (LG-175).
    pub op_capacity: u32,
    pub op_current: u32,
    pub clan_only: bool,
}

// SPEC: PROTOCOL_2009.md §3.2 (EVIDENCE: cshell+0x13dff0, cshell+0x13eda0)
pub fn server_entry(e: &ServerEntry) -> Vec<u8> {
    let mut w = PktWriter::empty();
    w.u32(e.id).wstr(&e.name, 33);
    w.at(0x46).wstr(&e.ip, 33);
    w.at(0x88)
        .u32(e.port)
        .u32(e.open as u32)
        .u32(e.capacity)
        .u32(e.current);
    w.u32(e.op_capacity).u32(e.op_current); // operator capacity / current
                                            // The client lists or auto-picks an entry only when min grade <= the grade of the 0x201 exp <= max grade
                                            // (EVIDENCE: cshell+0x139d70, grade of exp cshell+0x267f10); a max of 0 refuses every ranked account and the
                                            // picker then shows `res 0x475` "Every lobby is currently busy" (DISASM: cshell+0x13d690..0x13d6a4).
    w.u32(0).u32(SERVER_MAX_GRADE);
    w.f32(0.0).f32(0.0); // kill ratio min / max (0 = none)
    w.u32(0); // paid grade
    w.f32(1.0).f32(1.0); // exp / GP display rate
    w.u32(e.clan_only as u32);
    debug_assert_eq!(w.len(), 0xc0);
    w.raw_bytes().to_vec()
}

/// 0x2000 `server_list_push`: result 6, count, entries; padded to the 0xf16 bytes the client copies.
// SPEC: PROTOCOL_2009.md §2.2 0x2000 (EVIDENCE: cshell+0x38a20, copy cshell+0x36280)
pub fn server_list_push(entries: &[ServerEntry]) -> Vec<u8> {
    let mut w = PktWriter::new(SERVER_LIST_PUSH);
    let n = entries.len().min(20);
    w.u32(6).u32(0).u32(0).u32(n as u32);
    for e in &entries[..n] {
        w.bytes(&server_entry(e));
    }
    done(w, 0xf16)
}

/// 0x205 `name_check_reply`: +6 u32 free.
// SPEC: PROTOCOL_2009.md §2.2 0x205 (EVIDENCE: cshell+0x34de0)
pub fn name_check_reply(free: bool) -> Vec<u8> {
    ok_reply(NAME_CHECK_REPLY, free, 0xa)
}

/// 0x20f `referrer_check_reply`: +6 u32 ok; +0xa u16 reason (0x38 = not found).
// SPEC: PROTOCOL_2009.md §2.2 0x20f (EVIDENCE: cshell+0x35b80 -> cshell+0x12cf70)
pub fn referrer_check_reply(ok: bool, reason: u16) -> Vec<u8> {
    let mut w = PktWriter::new(REFERRER_CHECK_REPLY);
    w.u32(ok as u32).u16(reason);
    done(w, 0xc)
}

/// Parsed 0x202 (new character): face value (6000 + n) and name.
// SPEC: PROTOCOL_2009.md §2.2 0x202 (EVIDENCE: cshell+0x1922d0)
pub fn parse_new_char(p: &[u8]) -> (i32, String) {
    let mut r = PktReader::new(p);
    let face = r.at(6).i32();
    let name = r.at(0xa).wstr(17);
    (face, name)
}

/// 0x203 `new_char_reply`: +6 ok; +0xa code; +0xe account; +0x12 level.
// SPEC: PROTOCOL_2009.md §2.2 0x203 (EVIDENCE: cshell+0x34e60)
pub fn new_char_reply(ok: bool, code: u32, account: u32, level: u32) -> Vec<u8> {
    let mut w = PktWriter::new(NEW_CHAR_REPLY);
    w.u32(ok as u32).u32(code).u32(account).u32(level);
    done(w, 0x16)
}

/// 0x20b `server_pick_reply` / 0x3b3 `server_move_reply`: +6 ok; +0xa reason; +0xe u8[16] key.
// SPEC: PROTOCOL_2009.md §2.2 0x20b (EVIDENCE: cshell+0x34f60), §3.4 0x3b3 (EVIDENCE: cshell+0x34fb0)
pub fn session_key_reply(id: u16, ok: bool, key: &[u8; 16]) -> Vec<u8> {
    let mut w = PktWriter::new(id);
    w.u32(ok as u32).u32(0).bytes(key);
    done(w, 0x1e)
}

/// Reads a single wide name field at +6 (0x204, 0x20e).
pub fn parse_name_at6(p: &[u8]) -> String {
    PktReader::new(p).at(6).wstr(17)
}

// ---------------------------------------------------------------------------------------------------------------
// lobby login and character (§3.3)
// ---------------------------------------------------------------------------------------------------------------

/// Parsed 0x300 `lobby_login`: key and account number.
// SPEC: PROTOCOL_2009.md §3.3 0x300 (EVIDENCE: cshell+0x18ab20)
pub fn parse_lobby_login(p: &[u8]) -> ([u8; 16], u32) {
    let mut r = PktReader::new(p);
    let mut key = [0u8; 16];
    key.copy_from_slice(&r.at(6).bytes_padded(16));
    let account = r.at(0x16).u32();
    (key, account)
}

#[derive(Debug, Clone, Default)]
pub struct LobbyLoginReply {
    pub ok: bool,
    pub server_time: [u8; 16],
    pub lobby_slot: u32,
    pub price_counter: u32,
    pub shop_view_version: u32,
    pub list_versions: [u32; 5],
    pub room_settings_key: u32,
    pub room_settings_version: u32,
}

/// 0x301 `lobby_login_reply` (>= 0x4a).
// SPEC: PROTOCOL_2009.md §3.3 0x301 (EVIDENCE: cshell+0x420d0)
pub fn lobby_login_reply(v: &LobbyLoginReply) -> Vec<u8> {
    let mut w = PktWriter::new(LOBBY_LOGIN_REPLY);
    w.u32(v.ok as u32);
    w.at(0xe).bytes(&v.server_time);
    w.at(0x1e)
        .u32(v.lobby_slot)
        .u32(0)
        .u32(v.price_counter)
        .u32(v.shop_view_version);
    w.u32(v.list_versions[0])
        .u32(v.list_versions[1])
        .u32(v.list_versions[2]);
    w.u32(v.room_settings_key).u32(v.room_settings_version);
    w.u32(v.list_versions[3]).u32(v.list_versions[4]);
    done(w, 0x4a)
}

/// 0x343 `item_sync_reply`: +6 ok; +0xe i32 more (0 = done).
// SPEC: PROTOCOL_2009.md §3.3 0x343 (EVIDENCE: cshell+0x35c40)
pub fn item_sync_reply(more: i32) -> Vec<u8> {
    let mut w = PktWriter::new(ITEM_SYNC_REPLY);
    w.u32(1).at(0xe).i32(more);
    done(w, 0x12)
}

/// Fields of 0x303 `char_sheet_reply` that the server fills.
#[derive(Debug, Clone, Default)]
pub struct CharSheet {
    pub char_no: u32,
    pub name: String,
    pub face_value: i32,
    pub gp: u32,
    pub exp: u32,
    pub headshots: u32,
    pub kills: u32,
    pub deaths: u32,
    pub wins: u32,
    pub losses: u32,
    pub combined_rank: u32,
    pub clan_id: u32,
    pub clan_title: String,
    pub clan_role: u32,
    pub option_word: u32,
    pub emblem: u32,
    pub background: u32,
    pub inbox_unread: u32,
    pub name_colour: u32,
    pub clan_colour: u32,
    pub badge: u32,
    pub muted_until: u64,
    pub grade: u32,
    pub nation: u32,
    pub pending_notice: u32,
}

/// 0x303 `char_sheet_reply` (0x108 bytes, table 3.3a; unread fields are sent as 0).
// SPEC: PROTOCOL_2009.md §3.3 table 3.3a (EVIDENCE: cshell+0x3f2c0)
pub fn char_sheet_reply(c: &CharSheet) -> Vec<u8> {
    let mut w = PktWriter::new(CHAR_SHEET_REPLY);
    w.u32(1);
    w.at(0xe).u32(c.char_no);
    w.at(0x34).wstr(&c.name, 17);
    w.at(0x56).i32(c.face_value).u32(c.gp).u32(c.exp);
    w.u32(c.headshots)
        .u32(c.kills)
        .u32(c.deaths)
        .u32(c.wins)
        .u32(c.losses);
    w.at(0x7a).u32(c.combined_rank);
    w.at(0x86).u32(c.clan_id).wstr(&c.clan_title, 17);
    w.at(0xac).u32(c.clan_role).u32(c.option_word);
    w.at(0xd4)
        .u32(c.emblem)
        .u32(c.background)
        .u32(c.inbox_unread)
        .u32(c.name_colour)
        .u32(c.clan_colour);
    w.u32(c.badge).u64(c.muted_until);
    w.at(0xf8).u32(c.grade);
    w.at(0x100).u32(c.nation).u32(c.pending_notice);
    done(w, 0x108)
}

/// 0x323 `equipment_reply` (0xa6): +6 ok; +0x12 i32[15] item per slot (-1 empty); +0x4e i32[15] flags;
/// +0x8a i32[7] function-slot items.
// SPEC: PROTOCOL_2009.md §3.3 0x323 (EVIDENCE: cshell+0x229ca0)
pub fn equipment_reply(slots: &[i32; 15], flags: &[i32; 15], function_slots: &[i32; 7]) -> Vec<u8> {
    let mut w = PktWriter::new(EQUIPMENT_REPLY);
    w.u32(1).at(0x12);
    for v in slots {
        w.i32(*v);
    }
    for v in flags {
        w.i32(*v);
    }
    for v in function_slots {
        w.i32(*v);
    }
    done(w, 0xa6)
}

/// One `inventory_record` (0x34 bytes, 13 u32 words).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InventoryRecord {
    pub uid: u32,
    pub item_no: u32,
    pub parent_uid: u32,
    pub in_use: u32,
    pub start: u64,
    pub end: u64,
    pub use_kind: u32,
    pub remain: u32,
    pub default_kind: u32,
    pub function_word: u32,
    pub remain_period: u32,
}

/// 0x321 `inventory_reply`: +6 ok; +0xe count; +0x12 records (table 3.3b).
// SPEC: PROTOCOL_2009.md §3.3 0x321 + table 3.3b (EVIDENCE: cshell+0x42340, cshell+0x229130)
pub fn inventory_reply(items: &[InventoryRecord]) -> Vec<u8> {
    let mut w = PktWriter::new(INVENTORY_REPLY);
    // keep the frame under the 0x8000 body limit (SPEC: PROTOCOL_2009.md §1.2)
    let n = items.len().min((0x8000 - 0x12) / 0x34);
    w.u32(1).at(0xe).i32(n as i32);
    for it in &items[..n] {
        w.u32(it.uid)
            .u32(it.item_no)
            .u32(it.parent_uid)
            .u32(it.in_use)
            .u64(it.start)
            .u64(it.end);
        w.u32(it.use_kind)
            .u32(it.remain)
            .u32(it.default_kind)
            .u32(it.function_word)
            .u32(it.remain_period);
    }
    done(w, 0x12)
}

/// 0x345 `balance_reply`: +6 ok; +0xe credit; +0x12 prepaid.
// SPEC: PROTOCOL_2009.md §3.3 0x345 (EVIDENCE: cshell+0x337b0)
pub fn balance_reply(credit: u32, prepaid: u32) -> Vec<u8> {
    let mut w = PktWriter::new(BALANCE_REPLY);
    w.u32(1).at(0xe).u32(credit).u32(prepaid);
    done(w, 0x16)
}

/// 0x3004 `progress_push`: exp total, GP total, popup flag.
// SPEC: PROTOCOL_2009.md §3.3 0x3004 (EVIDENCE: cshell+0x40580)
pub fn progress_push(exp: u32, gp: u32, popup: bool) -> Vec<u8> {
    let mut w = PktWriter::new(PROGRESS_PUSH);
    w.u32(exp).u32(gp).u32(popup as u32);
    done(w, 0x12)
}

/// 0x33f `expiry_push`: +6 ok; +0xe count; +0x12 u32[100] unique ids; +0x1a2 refresh flag.
// SPEC: PROTOCOL_2009.md §4.2 0x33f (EVIDENCE: cshell+0x22cde0)
pub fn expiry_push(uids: &[u32], refresh: bool) -> Vec<u8> {
    let mut w = PktWriter::new(EXPIRY_PUSH);
    let n = uids.len().min(100);
    w.u32(1).at(0xe).u32(n as u32);
    for u in &uids[..n] {
        w.u32(*u);
    }
    w.at(0x1a2).u32(refresh as u32);
    done(w, 0x1a6)
}

/// Parsed 0x330 `item_use` (fields the core server reads).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemUse {
    pub uid: u32,
    pub new_name: String,
    pub emblem: u32,
    pub background: u32,
    pub face: u32,
    pub colour: u32,
    pub used_uid: u32,
    pub change_item: u32,
    pub nation: u32,
}

// SPEC: PROTOCOL_2009.md §4.3 0x330 (EVIDENCE: cshell+0x18e1d0)
pub fn parse_item_use(p: &[u8]) -> ItemUse {
    let mut r = PktReader::new(p);
    let uid = r.at(6).u32();
    let new_name = r.at(0xa).wstr(17);
    r.at(0x2c);
    ItemUse {
        uid,
        new_name,
        emblem: r.u32(),
        background: r.u32(),
        face: r.u32(),
        colour: r.u32(),
        used_uid: r.u32(),
        change_item: r.u32(),
        nation: r.u32(),
    }
}

/// 0x331 item-use reply. The 2009 layout is UNPROVEN (PROTOCOL_2009.md Appendix B); this layout is the harness
/// expectation: +6 ok; +0xa reason; +0xe unique id; +0x12 item number; +0x16 FILETIME end; +0x1e remaining.
// HARNESS: lobby_harness::test_main_suite (0x331 +6 ok, +0xe uid), ::test_lb5use (+0x16 end, len >= 0x22)
pub fn item_use_reply(
    ok: bool,
    reason: u32,
    uid: u32,
    item_no: u32,
    end: u64,
    remain: u32,
) -> Vec<u8> {
    let mut w = PktWriter::new(ITEM_USE_REPLY);
    w.u32(ok as u32)
        .u32(reason)
        .u32(uid)
        .u32(item_no)
        .u64(end)
        .u32(remain);
    done(w, 0x22)
}

// ---------------------------------------------------------------------------------------------------------------
// servers, channels, lobby lists (§3.4 - §3.6)
// ---------------------------------------------------------------------------------------------------------------

/// 0x3b1 `server_list_reply`: +6 ok; +0xe count; +0x12 entries (open entries only).
// SPEC: PROTOCOL_2009.md §3.4 0x3b1 (EVIDENCE: cshell+0x34f10, cshell+0x13f0b0)
pub fn server_list_reply(entries: &[ServerEntry]) -> Vec<u8> {
    let mut w = PktWriter::new(SERVER_LIST_REPLY);
    let open: Vec<&ServerEntry> = entries.iter().filter(|e| e.open).collect();
    w.u32(1).at(0xe).u32(open.len() as u32);
    for e in open {
        w.bytes(&server_entry(e));
    }
    done(w, 0x12)
}

#[derive(Debug, Clone)]
pub struct ChannelEntry {
    pub id: u32,
    pub name: String,
    pub open: bool,
    pub capacity: u32,
    pub current: u32,
    /// Operator slots: +0x52 capacity, +0x56 current (LG-175).
    pub op_capacity: u32,
    pub op_current: u32,
}

/// 0x377 `channel_list_reply`: +6 ok; +0xe count; +0x12 entries of 0x5a.
// SPEC: PROTOCOL_2009.md §3.4 0x377 (EVIDENCE: cshell+0x350f0, cshell+0xfb880)
pub fn channel_list_reply(ch: &[ChannelEntry]) -> Vec<u8> {
    let mut w = PktWriter::new(CHANNEL_LIST_REPLY);
    w.u32(1).at(0xe).i32(ch.len() as i32);
    for c in ch {
        let base = w.len();
        w.u32(c.id).wstr(&c.name, 33);
        w.at(base + 0x46)
            .u32(c.open as u32)
            .u32(c.capacity)
            .u32(c.current)
            .u32(c.op_capacity)
            .u32(c.op_current);
        debug_assert_eq!(w.len() - base, 0x5a);
    }
    done(w, 0x12)
}

/// Parsed 0x372 `channel_enter`: account, channel id.
// SPEC: PROTOCOL_2009.md §3.4 0x372 (EVIDENCE: cshell+0x189db0)
pub fn parse_channel_enter(p: &[u8]) -> (u32, u32) {
    let mut r = PktReader::new(p);
    let acc = r.at(6).u32();
    let ch = r.u32();
    (acc, ch)
}

/// 0x373 `channel_enter_reply`: +6 ok; +0xe my in-channel id.
// SPEC: PROTOCOL_2009.md §3.4 0x373 (EVIDENCE: cshell+0x32e30, cshell+0xfacd0)
pub fn channel_enter_reply(ok: bool, my_id: u32) -> Vec<u8> {
    let mut w = PktWriter::new(CHANNEL_ENTER_REPLY);
    w.u32(ok as u32).at(0xe).u32(my_id);
    done(w, 0x12)
}

/// One `lobby_user_record` (0x42 bytes, table 3.6b).
#[derive(Debug, Clone, Default)]
pub struct LobbyUser {
    pub account: u32,
    pub chan_id: u32,
    pub name: String,
    pub exp: u32,
    pub clan_id: u32,
    pub emblem: u32,
    pub background: u32,
    pub name_colour: u32,
    pub nation: u32,
}

// SPEC: PROTOCOL_2009.md §3.6 table 3.6b (EVIDENCE: cshell+0x1220e0, cshell+0x122230)
pub fn lobby_user_record(u: &LobbyUser) -> Vec<u8> {
    let mut w = PktWriter::empty();
    w.u32(u.account).u32(u.chan_id).wstr(&u.name, 17);
    w.u32(u.exp)
        .u32(u.clan_id)
        .u32(u.emblem)
        .u32(u.background)
        .u32(u.name_colour)
        .u32(u.nation);
    debug_assert_eq!(w.len(), 0x42);
    w.raw_bytes().to_vec()
}

/// 0x371 `channel_users_reply`: +6 ok; +0xe count; +0x12 records.
// SPEC: PROTOCOL_2009.md §3.6 0x371
pub fn channel_users_reply(users: &[LobbyUser]) -> Vec<u8> {
    let mut w = PktWriter::new(CHANNEL_USERS_REPLY);
    let n = users.len().min((0x8000 - 0x12) / 0x42);
    w.u32(1).at(0xe).i32(n as i32);
    for u in &users[..n] {
        w.bytes(&lobby_user_record(u));
    }
    done(w, 0x12)
}

/// 0x3021 `lobby_user_add_push` (0x48).
pub fn lobby_user_add_push(u: &LobbyUser) -> Vec<u8> {
    let mut w = PktWriter::new(LOBBY_USER_ADD_PUSH);
    w.bytes(&lobby_user_record(u));
    done(w, 0x48)
}

/// Pushes with a single u32 at +6 (0x3022 user remove, 0x3032 room remove, 0x3044 countdown).
pub fn u32_push(id: u16, v: u32) -> Vec<u8> {
    let mut w = PktWriter::new(id);
    w.u32(v);
    done(w, 0xa)
}

/// One `room_record` (0x7e bytes, table 3.6a).
#[derive(Debug, Clone, Default)]
pub struct RoomRow {
    pub no: u32,
    pub title: String,
    pub locked: bool,
    pub host_id: u32,
    pub map: u32,
    pub mode: u32,
    pub players: u32,
    pub max_players: u32,
    pub state: u32,
    pub goal: u32,
    pub opt_mid_join: u32,
    pub opt_ff: u32,
    pub opt_kill_cam: u32,
    pub weapon_limit: u32,
    pub host_badge: u32,
}

// SPEC: PROTOCOL_2009.md §3.6 table 3.6a (EVIDENCE: cshell+0x12a530)
pub fn room_record(r: &RoomRow) -> Vec<u8> {
    let mut w = PktWriter::empty();
    w.u32(r.no).wstr(&r.title, 33);
    w.at(0x46)
        .u32(0)
        .u32(r.locked as u32)
        .u32(r.host_id)
        .u32(r.map)
        .u32(r.mode);
    w.u32(r.players).u32(r.max_players).u32(r.state).u32(r.goal);
    w.u32(r.opt_mid_join)
        .u32(r.opt_ff)
        .u32(r.opt_kill_cam)
        .u32(r.weapon_limit)
        .u32(r.host_badge);
    debug_assert_eq!(w.len(), 0x7e);
    w.raw_bytes().to_vec()
}

/// 0x381 `room_list_reply`: +6 ok; +0xe count; +0x12 records.
// SPEC: PROTOCOL_2009.md §3.6 0x381 (EVIDENCE: cshell+0x332a0)
pub fn room_list_reply(rooms: &[RoomRow]) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_LIST_REPLY);
    let n = rooms.len().min((0x8000 - 0x12) / 0x7e);
    w.u32(1).at(0xe).u32(n as u32);
    for r in &rooms[..n] {
        w.bytes(&room_record(r));
    }
    done(w, 0x12)
}

/// 0x3031 `lobby_room_add_push` (0x84).
// SPEC: PROTOCOL_2009.md §3.6 0x3031 (EVIDENCE: cshell+0x346f0 -> cshell+0x12a8b0)
pub fn lobby_room_add_push(r: &RoomRow) -> Vec<u8> {
    let mut w = PktWriter::new(LOBBY_ROOM_ADD_PUSH);
    w.bytes(&room_record(r));
    done(w, 0x84)
}

/// 0x3035 `room_update_push` (>= 0x58): +6 room; +0xa type; +0xe value; +0x12 text length; +0x16 wstr text.
// SPEC: PROTOCOL_2009.md §3.7 0x3035 (EVIDENCE: cshell+0x168680, lobby form cshell+0x12ae60)
pub fn room_update_push(room: u32, field: u32, value: u32, text: &str) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_UPDATE_PUSH);
    w.u32(room)
        .u32(field)
        .u32(value)
        .u32(utf16_len(text))
        .wstr(text, 33);
    done(w, 0x58)
}

/// Parsed 0x3a0 `chat_send`.
// SPEC: PROTOCOL_2009.md §3.6 0x3a0 (EVIDENCE: cshell+0x18a0d0)
pub fn parse_chat(p: &[u8]) -> String {
    PktReader::new(p).at(6).wstr(121)
}

/// 0x3051 `chat_push`: +6 sender; +0xa name length; +0xe colour; +0x12 wstr[17] name; +0x34 text length;
/// +0x38 text (NUL-terminated).
// SPEC: PROTOCOL_2009.md §3.6 0x3051 (EVIDENCE: cshell+0x334b0 -> cshell+0x192f0)
pub fn chat_push(sender: u32, name: &str, colour: u32, text: &str) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().take(120).collect();
    let mut w = PktWriter::new(CHAT_PUSH);
    w.u32(sender)
        .u32(utf16_len(name).min(16))
        .u32(colour)
        .wstr(name, 17);
    w.at(0x34).u32(units.len() as u32);
    for u in &units {
        w.u16(*u);
    }
    w.u16(0);
    done(w, 0x38)
}

/// 0x379 `profile_reply` (0xe2).
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub account: u32,
    pub chan_id: u32,
    pub name: String,
    pub exp: u32,
    pub headshots: u32,
    pub kills: u32,
    pub deaths: u32,
    pub wins: u32,
    pub losses: u32,
    pub rank: u32,
    pub grade: u32,
    pub clan_id: u32,
    pub clan_title: String,
    pub emblem: u32,
    pub background: u32,
    pub name_colour: u32,
    pub clan_colour: u32,
    pub nation: u32,
}

// SPEC: PROTOCOL_2009.md §6.1 0x379 field order (LIVE); offsets EVIDENCE: cshell+0x35cf0 (+0xe, +0x12, +0x16 name,
// +0x38..+0x50, +0x58 clan id, +0x5c clan name, +0x9e / +0xa2 mark, +0xa6 / +0xaa / +0xae, words to +0xde)
pub fn profile_reply(ok: bool, p: &Profile) -> Vec<u8> {
    let mut w = PktWriter::new(PROFILE_REPLY);
    w.u32(ok as u32);
    w.at(0xe).u32(p.account).u32(p.chan_id).wstr(&p.name, 17);
    w.at(0x38)
        .u32(p.exp)
        .u32(p.headshots)
        .u32(p.kills)
        .u32(p.deaths)
        .u32(p.wins)
        .u32(p.losses);
    w.u32(p.rank)
        .u32(p.grade)
        .u32(p.clan_id)
        .wstr(&p.clan_title, 17);
    w.at(0x9e)
        .u32(p.emblem)
        .u32(p.background)
        .u32(p.name_colour)
        .u32(p.clan_colour)
        .u32(p.nation);
    done(w, 0xe2)
}

// ---------------------------------------------------------------------------------------------------------------
// rooms (§3.7) and the game start (§7)
// ---------------------------------------------------------------------------------------------------------------

/// Parsed 0x382 `room_create`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRoom {
    pub max_players: i32,
    pub title: String,
    pub password: String,
    pub mode: i32,
}

// SPEC: PROTOCOL_2009.md §3.7 0x382 (EVIDENCE: cshell+0x18a560)
pub fn parse_room_create(p: &[u8]) -> NewRoom {
    let mut r = PktReader::new(p);
    let max_players = r.at(6).i32();
    let title = r.at(0xa).wstr(33);
    let password = r.at(0x4c).wstr(9);
    let mode = r.at(0x5e).i32();
    NewRoom {
        max_players,
        title,
        password,
        mode,
    }
}

/// 0x383 `room_create_reply`: +6 ok; +0xa u16 code; +0xe room; +0x12 slot.
// SPEC: PROTOCOL_2009.md §3.7 0x383 (EVIDENCE: cshell+0x33320, cshell+0x138ad0)
pub fn room_create_reply(ok: bool, code: u16, room: u32, slot: u32) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_CREATE_REPLY);
    w.u32(ok as u32).u16(code).at(0xe).u32(room).u32(slot);
    done(w, 0x16)
}
pub const CODE_NO_MORE_ROOMS: u16 = 0x8d;

/// Parsed 0x384 `room_enter`: room number, password.
// SPEC: PROTOCOL_2009.md §3.7 0x384 (EVIDENCE: cshell+0x18a710)
pub fn parse_room_enter(p: &[u8]) -> (i32, String) {
    let mut r = PktReader::new(p);
    let no = r.at(6).i32();
    let pw = r.at(0xa).wstr(9);
    (no, pw)
}

/// 0x385 `room_enter_reply`: +6 ok; +0xa u16 reason; +0xe slot.
// SPEC: PROTOCOL_2009.md §3.7 0x385 (EVIDENCE: wrapper cshell+0x33370; body LIVE)
pub fn room_enter_reply(ok: bool, reason: u16, slot: u32) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_ENTER_REPLY);
    w.u32(ok as u32).u16(reason).at(0xe).u32(slot);
    done(w, 0x12)
}
pub const REASON_WRONG_PASSWORD: u16 = 0x2a;
pub const REASON_KICKED: u16 = 0x8a;
/// Any other reason shows the generic box (`res 0xfb1`); used for "full", "playing", "no such room".
pub const REASON_GENERIC: u16 = 0x01;

/// 0x389 `room_info_reply` (0x8c, table 3.7a).
// SPEC: PROTOCOL_2009.md §3.7 table 3.7a (EVIDENCE: cshell+0x1684c0)
pub fn room_info_reply(r: &RoomRow) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_INFO_REPLY);
    w.i32(1);
    w.at(0xe).u32(r.no).wstr(&r.title, 33);
    w.at(0x54)
        .u32(0)
        .u32(r.locked as u32)
        .u32(r.host_id)
        .u32(r.map)
        .u32(r.mode);
    w.u32(r.players).u32(r.max_players).u32(r.state).u32(r.goal);
    w.u32(r.opt_mid_join)
        .u32(r.opt_ff)
        .u32(r.opt_kill_cam)
        .u32(r.weapon_limit)
        .u32(r.host_badge);
    done(w, 0x8c)
}

/// One `room_user_record` (0xb8 bytes, table 3.7b).
#[derive(Debug, Clone, Default)]
pub struct SeatRow {
    pub char_no: u32,
    pub chan_id: u32,
    pub rank: u32,
    pub class: u32,
    pub name: String,
    pub clan_id: u32,
    pub team: u32,
    pub state: u32,
    pub sub_state: u32,
    pub exp: u32,
    pub clan_title: String,
    pub option_word: u32,
    pub emblem: u32,
    pub background: u32,
    pub name_colour: u32,
    pub clan_colour: u32,
    pub nation: u32,
}

// SPEC: PROTOCOL_2009.md §3.7 table 3.7b (EVIDENCE: cshell+0x166660, cshell+0x166310)
pub fn room_user_record(u: &SeatRow) -> Vec<u8> {
    let mut w = PktWriter::empty();
    w.u32(u.char_no)
        .u32(u.chan_id)
        .u32(u.rank)
        .u32(u.class)
        .wstr(&u.name, 17);
    w.u32(u.clan_id)
        .u32(u.team)
        .u32(u.state)
        .u32(u.sub_state)
        .u32(u.exp);
    w.at(0x5e).wstr(&u.clan_title, 17).u32(u.option_word);
    w.at(0xa4)
        .u32(u.emblem)
        .u32(u.background)
        .u32(u.name_colour)
        .u32(u.clan_colour)
        .u32(u.nation);
    debug_assert_eq!(w.len(), 0xb8);
    w.raw_bytes().to_vec()
}

/// 0x38b `room_users_reply`: +6 ok; +0xe count; +0x12 records.
pub fn room_users_reply(users: &[SeatRow]) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_USERS_REPLY);
    w.u32(1).at(0xe).i32(users.len() as i32);
    for u in users {
        w.bytes(&room_user_record(u));
    }
    done(w, 0x12)
}

/// 0x3033 `room_user_add_push` (0xbe).
pub fn room_user_add_push(u: &SeatRow) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_USER_ADD_PUSH);
    w.bytes(&room_user_record(u));
    done(w, 0xbe)
}

/// 0x3034 `room_user_remove_push`: +6 in-channel id; +0xa reason (1 kicked).
// SPEC: PROTOCOL_2009.md §3.7 0x3034 (EVIDENCE: cshell+0x34990 -> cshell+0x165400)
pub fn room_user_remove_push(chan_id: u32, reason: u32) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_USER_REMOVE_PUSH);
    w.u32(chan_id).u32(reason);
    done(w, 0xe)
}
pub const REMOVE_LEFT: u32 = 0;
pub const REMOVE_KICKED: u32 = 1;
/// 0x3034 reason 2: the kicked player's lobby shows the GM-kick text (`res 0x565`).
// DC-DEFAULT: LG-173 a kick by a GM account carries reason 2
pub const REMOVE_KICKED_BY_GM: u32 = 2;

/// 0x3041 `room_team_push`: +6 in-channel id; +0xa team.
// SPEC: PROTOCOL_2009.md §3.7 0x3041 (EVIDENCE: cshell+0x349e0 -> cshell+0x166c90)
pub fn room_team_push(chan_id: u32, team: u32) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_TEAM_PUSH);
    w.u32(chan_id).u32(team);
    done(w, 0xe)
}

/// 0x3042 `room_user_state_push`: +6 id; +0xa state; +0xe sub-state.
// SPEC: PROTOCOL_2009.md §3.7 0x3042 (EVIDENCE: cshell+0x33f50 -> cshell+0x1673a0)
pub fn room_user_state_push(chan_id: u32, state: u32, sub: u32) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_USER_STATE_PUSH);
    w.u32(chan_id).u32(state).u32(sub);
    done(w, 0x12)
}

/// Parsed 0x390 `team_change` destination (+0xe; +6 / +0xa are unwritten client stack bytes).
// SPEC: PROTOCOL_2009.md §3.7 0x390 (EVIDENCE: cshell+0x18b500)
pub fn parse_team_change(p: &[u8]) -> u32 {
    PktReader::new(p).at(0xe).u32()
}

/// 0x391 `team_change_reply`: +6 ok; +0xa reason.
pub fn team_change_reply(ok: bool, reason: u32) -> Vec<u8> {
    let mut w = PktWriter::new(TEAM_CHANGE_REPLY);
    w.u32(ok as u32).u32(reason);
    done(w, 0xe)
}

/// Parsed 0x392 `room_user_flag`: kind, value.
// SPEC: PROTOCOL_2009.md §3.7 0x392 (EVIDENCE: cshell+0x18b5d0)
pub fn parse_user_flag(p: &[u8]) -> (u32, u32) {
    let mut r = PktReader::new(p);
    let k = r.at(6).u32();
    let v = r.u32();
    (k, v)
}

/// 0x393 `room_user_flag_reply`: +6 ok; +0xa u16 reason; +0xc i16 kind (3 = join errors 10 / 11 / 0x8e).
// SPEC: PROTOCOL_2009.md §3.7 0x393 (EVIDENCE: cshell+0x33ef0 -> cshell+0x15f0d0)
pub fn user_flag_reply(ok: bool, reason: u16, kind: i16) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_USER_FLAG_REPLY);
    w.u32(ok as u32).u16(reason).i16(kind);
    done(w, 0xe)
}
pub const FLAG_REPLY_KIND_JOIN: i16 = 3;
pub const REASON_JOIN_FAILED: u16 = 0x8e;

/// Parsed 0x38c `room_setting_change`: type, value, and the password text of the 0x58-byte form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingChange {
    pub field: u32,
    pub value: u32,
    pub text: Option<String>,
}

// SPEC: PROTOCOL_2009.md §3.7 0x38c (EVIDENCE: cshell+0x18a8e0, password form cshell+0x18a9c0)
pub fn parse_setting_change(p: &[u8]) -> SettingChange {
    let mut r = PktReader::new(p);
    let field = r.at(0xa).u32();
    let value = r.u32();
    let text = if p.len() >= 0x58 && field == crate::msg::field::LOCKED {
        let n = (r.at(0x12).u16() as usize).min(32);
        let s = r.at(0x16).wstr(33);
        Some(s.chars().take(n.max(s.chars().count().min(n))).collect())
    } else {
        None
    };
    SettingChange { field, value, text }
}

/// 0x38f `room_kick_reply`: +6 u32 ok; +0xa u32 reason of a refusal (0x82 = the target is a GM, `res 0x1845`;
/// other values 0x7e..0x83 pick `res 0x1850 / 0x184f / 0x184e / 0x1846 / 0x1845 / 0x1844`, anything else `res 0x1847`).
// SPEC: PROTOCOL_2009.md §3.7 0x38f (EVIDENCE: cshell+0x160340)
pub fn room_kick_reply(ok: bool, reason: u32) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_KICK_REPLY);
    w.u32(ok as u32).u32(reason);
    done(w, 0xe)
}
/// 0x38f reason: the target is a GM and cannot be kicked (`res 0x1845`).
// EVIDENCE: cshell+0x160340 (case 0x82)
pub const KICK_REFUSED_GM: u32 = 0x82;

/// Parsed 0x38e `room_kick`: in-channel id, character number.
// SPEC: PROTOCOL_2009.md §3.7 0x38e (EVIDENCE: cshell+0x18f280, cshell+0x18f1a0)
pub fn parse_kick(p: &[u8]) -> (u32, u32) {
    let mut r = PktReader::new(p);
    let a = r.at(6).u32();
    let b = r.u32();
    (a, b)
}

/// 0x3023 `room_invite_push`: +6 room; +0xa wstr[17] host name; +0x2c wstr[10] password.
// SPEC: PROTOCOL_2009.md §3.7 0x3023 (EVIDENCE: cshell+0x39110)
pub fn room_invite_push(room: u32, host: &str, password: &str) -> Vec<u8> {
    let mut w = PktWriter::new(ROOM_INVITE_PUSH);
    w.u32(room).wstr(host, 17).wstr(password, 10);
    done(w, 0x40)
}

/// 0x3043 `match_address_push`: +6 wstr[17] ip; +0x28 port; +0x2c record count (0: the ranking record layout is
/// unproven).
// SPEC: PROTOCOL_2009.md §7 0x3043 (EVIDENCE: cshell+0x165650)
pub fn match_address_push(ip: &str, port: u32) -> Vec<u8> {
    let mut w = PktWriter::new(MATCH_ADDRESS_PUSH);
    w.wstr(ip, 17).u32(port).u32(0);
    done(w, 0x30)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (frame id, payload) of a built frame, checking both headers.
    fn split(f: &[u8]) -> (i32, Vec<u8>) {
        let (frames, used) = dc_wire::parse_tcp_frames(f).unwrap();
        assert_eq!(used, f.len());
        assert_eq!(frames.len(), 1);
        let (id, p) = frames.into_iter().next().unwrap();
        assert_eq!(
            u16::from_le_bytes([p[0], p[1]]) as i32,
            id,
            "inner id = frame id"
        );
        assert_eq!(
            u32::from_le_bytes([p[2], p[3], p[4], p[5]]) as usize,
            p.len(),
            "inner length"
        );
        (id, p)
    }
    fn u32_at(p: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]])
    }
    fn u16_at(p: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([p[o], p[o + 1]])
    }
    fn ws(p: &[u8], o: usize, n: usize) -> String {
        PktReader::from_raw(p).at(o).wstr(n)
    }

    #[test]
    fn auth_replies() {
        let (id, p) = split(&file_check_reply(true));
        assert_eq!((id, p.len(), u32_at(&p, 6)), (0x207, 0xa, 1));
        let (_, p) = split(&login_reply(&LoginReply {
            ok: false,
            reason: REASON_NO_CHARACTER,
            account: 7,
            ..Default::default()
        }));
        assert_eq!(p.len(), 0x2e);
        assert_eq!(
            (u32_at(&p, 6), u32_at(&p, 0xa), u32_at(&p, 0x12)),
            (0, 0x0003001e, 7)
        );
        let (_, p) = split(&login_reply(&LoginReply {
            ok: true,
            privilege: 2,
            days: 3,
            ..Default::default()
        }));
        assert_eq!((u32_at(&p, 0x26), u32_at(&p, 0x2a)), (2, 3));
        let (_, p) = split(&new_char_reply(true, 0, 9, 1));
        assert_eq!(
            (p.len(), u32_at(&p, 6), u32_at(&p, 0xe), u32_at(&p, 0x12)),
            (0x16, 1, 9, 1)
        );
        let (_, p) = split(&referrer_check_reply(false, 0x38));
        assert_eq!((p.len(), u32_at(&p, 6), u16_at(&p, 0xa)), (0xc, 0, 0x38));
        let key = [7u8; 16];
        let (id, p) = split(&session_key_reply(SERVER_PICK_REPLY, true, &key));
        assert_eq!((id, p.len(), u32_at(&p, 6)), (0x20b, 0x1e, 1));
        assert_eq!(&p[0xe..0x1e], &key);
    }

    #[test]
    fn server_list_layout() {
        let e = ServerEntry {
            id: 1,
            name: "S".into(),
            ip: "127.0.0.1".into(),
            port: 4242,
            open: true,
            capacity: 1000,
            current: 2,
            op_capacity: 10,
            op_current: 1,
            clan_only: false,
        };
        let (id, p) = split(&server_list_push(std::slice::from_ref(&e)));
        assert_eq!((id, p.len()), (0x2000, 0xf16));
        assert_eq!((u32_at(&p, 6), u32_at(&p, 0x12)), (6, 1));
        assert_eq!(
            u32_at(&p, 0x16 + 0x88),
            4242,
            "harness reads the lobby port at 0x16+0x88"
        );
        assert_eq!(ws(&p, 0x16 + 0x46, 33), "127.0.0.1");
        assert_eq!(
            (
                u32_at(&p, 0x16 + 0x8c),
                u32_at(&p, 0x16 + 0x90),
                u32_at(&p, 0x16 + 0x94)
            ),
            (1, 1000, 2)
        );
        // LG-175 operator slots at +0x98 / +0x9c
        assert_eq!((u32_at(&p, 0x16 + 0x98), u32_at(&p, 0x16 + 0x9c)), (10, 1));
        let mut closed = e.clone();
        closed.open = false;
        let (_, p) = split(&server_list_reply(&[e, closed]));
        assert_eq!((p.len(), u32_at(&p, 0xe)), (0x12 + 0xc0, 1));
    }

    /// Byte layout of the whole 0x2000 frame against the capture form (tdm6 / tdm4 seq 7, the reference lobby's
    /// list that the real client accepted), with our names, address and ports substituted.
    // REFERENCE / PROVEN-CAPTURE: captures 20261025_065016_tdm6 seq 7, 20261025_002236_tdm4 seq 7; rust1 seq 7 differed
    // only at entry +0xa4 (0 instead of 0x63) and the client showed `res 0x475` and closed (rust1 seq 8).
    #[test]
    fn server_list_push_matches_capture_form() {
        let entry = |id: u32, name: &str, port: u32, clan: bool| ServerEntry {
            id,
            name: name.into(),
            ip: "127.0.0.1".into(),
            port,
            open: true,
            capacity: 1000,
            current: 0,
            op_capacity: 1000,
            op_current: 0,
            clan_only: clan,
        };
        let frame = server_list_push(&[
            entry(1, "DeCombat", 0x81ab, false),
            entry(2, "DeCombat Clan", 0xdb8e, true),
        ]);
        let utf16 = |s: &str| -> Vec<u8> {
            let mut v: Vec<u8> = s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
            v.resize(66, 0);
            v
        };
        let mut want = Vec::new();
        // stream frame (u32 id, u32 length) then the packet header (u16 id, u32 length), as on the wire
        want.extend_from_slice(&[
            0x00, 0x20, 0, 0, 0x16, 0x0f, 0, 0, 0x00, 0x20, 0x16, 0x0f, 0x00, 0x00,
        ]);
        for v in [6u32, 0, 0, 2] {
            want.extend_from_slice(&v.to_le_bytes()); // result, reason, sub-error, count
        }
        for (id, name, port, clan) in [
            (1u32, "DeCombat", 0x81abu32, 0u32),
            (2, "DeCombat Clan", 0xdb8e, 1),
        ] {
            want.extend_from_slice(&id.to_le_bytes());
            want.extend_from_slice(&utf16(name));
            want.extend_from_slice(&utf16("127.0.0.1"));
            // port, open, capacity, current, op capacity, op current, min grade, max grade (0x63)
            for v in [port, 1, 1000, 0, 1000, 0, 0, 0x63] {
                want.extend_from_slice(&v.to_le_bytes());
            }
            // kill ratio min / max, paid grade, exp rate 1.0, GP rate 1.0, clan flag
            for v in [0u32, 0, 0, 0x3f80_0000, 0x3f80_0000, clan] {
                want.extend_from_slice(&v.to_le_bytes());
            }
        }
        want.resize(8 + 0xf16, 0);
        assert_eq!(frame.len(), 8 + 0xf16);
        let first_diff = frame.iter().zip(&want).position(|(a, b)| a != b);
        assert_eq!(first_diff, None, "0x2000 differs from the capture form");
        assert_eq!(&frame[8 + 0x16 + 0xa4..8 + 0x16 + 0xa8], &[0x63, 0, 0, 0]);
    }

    #[test]
    fn login_and_lobby_login_parse() {
        let mut w = PktWriter::new(0x200);
        w.wstr("LOCAL:alice", 4096)
            .wstr("pw", 17)
            .wstr("00-11", 20)
            .i32(5);
        let p = w.into_payload();
        assert_eq!(p.len(), 0x2054);
        let l = parse_login(&p);
        assert_eq!(
            (
                l.token.as_str(),
                l.password.as_str(),
                l.adapter.as_str(),
                l.partner
            ),
            ("LOCAL:alice", "pw", "00-11", 5)
        );
        let mut w = PktWriter::new(0x300);
        w.bytes(&[3u8; 16]).u32(42);
        let p = w.into_payload();
        assert_eq!(p.len(), 0x1a);
        assert_eq!(parse_lobby_login(&p), ([3u8; 16], 42));
    }

    #[test]
    fn lobby_login_reply_layout() {
        let v = LobbyLoginReply {
            ok: true,
            server_time: [9u8; 16],
            lobby_slot: 5,
            price_counter: 1,
            shop_view_version: 2,
            list_versions: [3, 4, 5, 6, 7],
            room_settings_key: 8,
            room_settings_version: 9,
        };
        let (_, p) = split(&lobby_login_reply(&v));
        assert_eq!(p.len(), 0x4a);
        assert_eq!(&p[0xe..0x1e], &[9u8; 16]);
        let words: Vec<u32> = (0x1e..0x4a).step_by(4).map(|o| u32_at(&p, o)).collect();
        assert_eq!(words, vec![5, 0, 1, 2, 3, 4, 5, 8, 9, 6, 7]);
    }

    #[test]
    fn character_info_layout() {
        let c = CharSheet {
            char_no: 3,
            name: "Dave".into(),
            face_value: 6002,
            gp: 6081,
            exp: 3,
            headshots: 1,
            kills: 2,
            deaths: 0,
            wins: 1,
            losses: 0,
            clan_id: 4,
            clan_title: "Persist".into(),
            clan_role: 5,
            option_word: 2,
            emblem: 3,
            background: 4,
            name_colour: 10,
            muted_until: 0x1122334455667788,
            grade: 7,
            nation: 17,
            pending_notice: 1,
            ..Default::default()
        };
        let (_, p) = split(&char_sheet_reply(&c));
        assert_eq!(p.len(), 0x108);
        assert_eq!((u32_at(&p, 6), u32_at(&p, 0xe)), (1, 3));
        assert_eq!(ws(&p, 0x34, 17), "Dave");
        assert_eq!(
            (u32_at(&p, 0x56), u32_at(&p, 0x5a), u32_at(&p, 0x5e)),
            (6002, 6081, 3)
        );
        let st: Vec<u32> = (0..5).map(|i| u32_at(&p, 0x62 + 4 * i)).collect();
        assert_eq!(st, vec![1, 2, 0, 1, 0]);
        assert_eq!(u32_at(&p, 0x86), 4);
        assert_eq!(ws(&p, 0x8a, 17), "Persist");
        assert_eq!((u32_at(&p, 0xac), u32_at(&p, 0xb0)), (5, 2));
        assert_eq!(
            (u32_at(&p, 0xd4), u32_at(&p, 0xd8), u32_at(&p, 0xe0)),
            (3, 4, 10)
        );
        assert_eq!(
            u64::from_le_bytes(p[0xec..0xf4].try_into().unwrap()),
            0x1122334455667788
        );
        assert_eq!(
            (u32_at(&p, 0xf8), u32_at(&p, 0x100), u32_at(&p, 0x104)),
            (7, 17, 1)
        );
    }

    #[test]
    fn equipment_and_inventory_layout() {
        let mut slots = [-1i32; 15];
        slots[0] = 145;
        slots[9] = 7001;
        let (_, p) = split(&equipment_reply(&slots, &[0; 15], &[0; 7]));
        assert_eq!(p.len(), 0xa6);
        assert_eq!(u32_at(&p, 0x12) as i32, 145);
        assert_eq!(u32_at(&p, 0x12 + 4 * 9) as i32, 7001);
        assert_eq!(u32_at(&p, 0x12 + 4) as i32, -1);
        let rec = InventoryRecord {
            uid: 5,
            item_no: 3,
            in_use: 1,
            end: 0xabcdef,
            function_word: 10,
            ..Default::default()
        };
        let (_, p) = split(&inventory_reply(&[rec.clone(), rec]));
        assert_eq!(p.len(), 0x12 + 2 * 0x34);
        assert_eq!(u32_at(&p, 0xe), 2);
        let r0 = 0x12;
        assert_eq!(
            (u32_at(&p, r0), u32_at(&p, r0 + 4), u32_at(&p, r0 + 0xc)),
            (5, 3, 1)
        );
        assert_eq!(
            u64::from_le_bytes(p[r0 + 0x18..r0 + 0x20].try_into().unwrap()),
            0xabcdef
        );
        assert_eq!(u32_at(&p, r0 + 0x2c), 10);
        let (_, p) = split(&balance_reply(7, 5000));
        assert_eq!(
            (p.len(), u32_at(&p, 0xe), u32_at(&p, 0x12)),
            (0x16, 7, 5000)
        );
        let (_, p) = split(&expiry_push(&[11, 12], true));
        assert_eq!(
            (
                p.len(),
                u32_at(&p, 0xe),
                u32_at(&p, 0x12),
                u32_at(&p, 0x16),
                u32_at(&p, 0x1a2)
            ),
            (0x1a6, 2, 11, 12, 1)
        );
        let (_, p) = split(&progress_push(3, 6081, false));
        assert_eq!(
            (p.len(), u32_at(&p, 6), u32_at(&p, 0xa), u32_at(&p, 0xe)),
            (0x12, 3, 6081, 0)
        );
        let (_, p) = split(&item_use_reply(true, 0, 9, 9010, 1 << 60, 0));
        assert_eq!((p.len(), u32_at(&p, 6), u32_at(&p, 0xe)), (0x22, 1, 9));
        assert_eq!(
            u64::from_le_bytes(p[0x16..0x1e].try_into().unwrap()),
            1 << 60
        );
        let (_, p) = split(&item_sync_reply(0));
        assert_eq!((p.len(), u32_at(&p, 0xe)), (0x12, 0));
    }

    #[test]
    fn channel_and_lobby_records() {
        let ch = [ChannelEntry {
            id: 1,
            name: "Channel 1".into(),
            open: true,
            capacity: 200,
            current: 3,
            op_capacity: 10,
            op_current: 2,
        }];
        let (_, p) = split(&channel_list_reply(&ch));
        assert_eq!(
            (p.len(), u32_at(&p, 0xe), u32_at(&p, 0x12)),
            (0x12 + 0x5a, 1, 1)
        );
        assert_eq!(ws(&p, 0x12 + 4, 33), "Channel 1");
        assert_eq!(
            (
                u32_at(&p, 0x12 + 0x46),
                u32_at(&p, 0x12 + 0x4a),
                u32_at(&p, 0x12 + 0x4e)
            ),
            (1, 200, 3)
        );
        // LG-175 operator slots at +0x52 / +0x56
        assert_eq!((u32_at(&p, 0x12 + 0x52), u32_at(&p, 0x12 + 0x56)), (10, 2));
        let (_, p) = split(&channel_enter_reply(true, 77));
        assert_eq!((p.len(), u32_at(&p, 6), u32_at(&p, 0xe)), (0x12, 1, 77));
        let u = LobbyUser {
            account: 5,
            chan_id: 5,
            name: "Bob".into(),
            exp: 9,
            nation: 2,
            ..Default::default()
        };
        assert_eq!(lobby_user_record(&u).len(), 0x42);
        let (id, p) = split(&lobby_user_add_push(&u));
        assert_eq!(
            (id, p.len(), u32_at(&p, 6 + 4)),
            (0x3021, 0x48, 5),
            "harness: 0x3021 +0xa = in-channel id"
        );
        assert_eq!(u32_at(&p, 6 + 0x3e), 2);
        let (_, p) = split(&channel_users_reply(&[u.clone(), u]));
        assert_eq!((p.len(), u32_at(&p, 0xe)), (0x12 + 2 * 0x42, 2));
        let (id, p) = split(&u32_push(LOBBY_USER_REMOVE_PUSH, 5));
        assert_eq!((id, p.len(), u32_at(&p, 6)), (0x3022, 0xa, 5));
    }

    #[test]
    fn chat_push_layout() {
        let kr = "\u{c548}\u{b155}\u{d558}\u{c138}\u{c694}";
        let (id, p) = split(&chat_push(3, "Bob", 0, kr));
        assert_eq!(id, 0x3051);
        assert_eq!(ws(&p, 0x12, 17), "Bob");
        assert_eq!(u32_at(&p, 0xa), 3);
        let n = u32_at(&p, 0x34) as usize;
        assert_eq!(n, 5);
        let text =
            String::from_utf16(&(0..n).map(|i| u16_at(&p, 0x38 + 2 * i)).collect::<Vec<_>>())
                .unwrap();
        assert_eq!(text, kr);
        assert_eq!(u16_at(&p, 0x38 + 2 * n), 0);
        let mut w = PktWriter::new(0x3a0);
        w.wstr("hello", 121);
        let q = w.into_payload();
        assert_eq!(q.len(), 0xf8);
        assert_eq!(parse_chat(&q), "hello");
    }

    #[test]
    fn room_records_and_pushes() {
        let r = RoomRow {
            no: 4,
            title: "Test room".into(),
            locked: true,
            host_id: 9,
            map: 2,
            mode: 1,
            players: 2,
            max_players: 8,
            state: 1,
            goal: 50,
            opt_mid_join: 1,
            ..Default::default()
        };
        let rec = room_record(&r);
        assert_eq!(rec.len(), 0x7e);
        assert_eq!(
            (
                u32_at(&rec, 0x4a),
                u32_at(&rec, 0x4e),
                u32_at(&rec, 0x52),
                u32_at(&rec, 0x56)
            ),
            (1, 9, 2, 1)
        );
        assert_eq!(
            (
                u32_at(&rec, 0x5a),
                u32_at(&rec, 0x5e),
                u32_at(&rec, 0x62),
                u32_at(&rec, 0x66)
            ),
            (2, 8, 1, 50)
        );
        assert_eq!(u32_at(&rec, 0x6a), 1);
        let (id, p) = split(&lobby_room_add_push(&r));
        assert_eq!((id, p.len(), u32_at(&p, 6)), (0x3031, 0x84, 4));
        let (_, p) = split(&room_list_reply(std::slice::from_ref(&r)));
        assert_eq!((p.len(), u32_at(&p, 0xe)), (0x12 + 0x7e, 1));
        let (_, p) = split(&room_info_reply(&r));
        assert_eq!(p.len(), 0x8c);
        assert_eq!(
            (u32_at(&p, 0xe), u32_at(&p, 0x58), u32_at(&p, 0x5c)),
            (4, 1, 9)
        );
        assert_eq!(ws(&p, 0x12, 33), "Test room");
        assert_eq!(
            (
                u32_at(&p, 0x60),
                u32_at(&p, 0x64),
                u32_at(&p, 0x68),
                u32_at(&p, 0x6c)
            ),
            (2, 1, 2, 8)
        );
        assert_eq!(
            (u32_at(&p, 0x70), u32_at(&p, 0x74), u32_at(&p, 0x78)),
            (1, 50, 1)
        );
        let (_, p) = split(&room_update_push(4, 3, 5, ""));
        assert_eq!(
            (p.len(), u32_at(&p, 6), u32_at(&p, 0xa), u32_at(&p, 0xe)),
            (0x58, 4, 3, 5)
        );
        let (_, p) = split(&room_update_push(4, 0xb, 0, "New title"));
        assert_eq!(
            (u32_at(&p, 0x12), ws(&p, 0x16, 33).as_str()),
            (9, "New title")
        );
    }

    #[test]
    fn room_user_records() {
        let u = SeatRow {
            char_no: 5,
            chan_id: 5,
            name: "Bob".into(),
            team: 2,
            state: 1,
            exp: 70,
            clan_title: "C".into(),
            nation: 3,
            ..Default::default()
        };
        let rec = room_user_record(&u);
        assert_eq!(rec.len(), 0xb8);
        assert_eq!(
            (
                u32_at(&rec, 0),
                u32_at(&rec, 4),
                u32_at(&rec, 0x36),
                u32_at(&rec, 0x3a),
                u32_at(&rec, 0x42)
            ),
            (5, 5, 2, 1, 70)
        );
        assert_eq!(ws(&rec, 0x10, 17), "Bob");
        assert_eq!(ws(&rec, 0x5e, 17), "C");
        assert_eq!(u32_at(&rec, 0xb4), 3);
        let (id, p) = split(&room_user_add_push(&u));
        assert_eq!((id, p.len(), u32_at(&p, 6 + 4)), (0x3033, 0xbe, 5));
        let (_, p) = split(&room_users_reply(&[u.clone(), u]));
        assert_eq!((p.len(), u32_at(&p, 0xe)), (0x12 + 2 * 0xb8, 2));
        let (_, p) = split(&room_user_remove_push(5, 1));
        assert_eq!((p.len(), u32_at(&p, 6), u32_at(&p, 0xa)), (0xe, 5, 1));
        let (_, p) = split(&room_team_push(5, 2));
        assert_eq!((p.len(), u32_at(&p, 6), u32_at(&p, 0xa)), (0xe, 5, 2));
        let (_, p) = split(&room_user_state_push(5, 3, 1));
        assert_eq!(
            (p.len(), u32_at(&p, 6), u32_at(&p, 0xa), u32_at(&p, 0xe)),
            (0x12, 5, 3, 1)
        );
    }

    #[test]
    fn room_requests_parse() {
        let mut w = PktWriter::new(0x382);
        w.i32(8).wstr("Test room", 33).wstr("1234", 9).i32(3);
        let p = w.into_payload();
        assert_eq!(p.len(), 0x62);
        assert_eq!(
            parse_room_create(&p),
            NewRoom {
                max_players: 8,
                title: "Test room".into(),
                password: "1234".into(),
                mode: 3
            }
        );
        let mut w = PktWriter::new(0x384);
        w.i32(2).wstr("0000", 9).wstr("", 65);
        let p = w.into_payload();
        assert_eq!(p.len(), 0x9e);
        assert_eq!(parse_room_enter(&p), (2, "0000".to_string()));
        let mut w = PktWriter::new(0x38c);
        w.i32(0).u32(3).u32(5);
        assert_eq!(
            parse_setting_change(&w.into_payload()),
            SettingChange {
                field: 3,
                value: 5,
                text: None
            }
        );
        let mut w = PktWriter::new(0x38c);
        w.i32(0).u32(1).u32(0).u16(4).u16(0).wstr("abcd", 33);
        let p = w.into_payload();
        assert_eq!(p.len(), 0x58);
        assert_eq!(parse_setting_change(&p).text.as_deref(), Some("abcd"));
        let mut w = PktWriter::new(0x390);
        w.u32(0xdead).u32(0xbeef).u32(2);
        assert_eq!(parse_team_change(&w.into_payload()), 2);
        let mut w = PktWriter::new(0x392);
        w.u32(0).u32(2);
        assert_eq!(parse_user_flag(&w.into_payload()), (0, 2));
        let mut w = PktWriter::new(0x38e);
        w.u32(7).u32(8);
        assert_eq!(parse_kick(&w.into_payload()), (7, 8));
    }

    #[test]
    fn replies_and_game_start_pushes() {
        let (_, p) = split(&room_create_reply(true, 0, 3, 0));
        assert_eq!((p.len(), u32_at(&p, 6), u32_at(&p, 0xe)), (0x16, 1, 3));
        let (_, p) = split(&room_create_reply(false, CODE_NO_MORE_ROOMS, 0, 0));
        assert_eq!(u16_at(&p, 0xa), 0x8d);
        let (_, p) = split(&room_enter_reply(false, REASON_WRONG_PASSWORD, 0));
        assert_eq!((p.len(), u32_at(&p, 6), u16_at(&p, 0xa)), (0x12, 0, 0x2a));
        let (_, p) = split(&user_flag_reply(
            false,
            REASON_JOIN_FAILED,
            FLAG_REPLY_KIND_JOIN,
        ));
        assert_eq!(
            (p.len(), u32_at(&p, 6), u16_at(&p, 0xa), u16_at(&p, 0xc)),
            (0xe, 0, 0x8e, 3)
        );
        let (id, p) = split(&u32_push(START_COUNTDOWN_PUSH, 4));
        assert_eq!((id, p.len(), u32_at(&p, 6)), (0x3044, 0xa, 4));
        let (id, p) = split(&match_address_push("127.0.0.1", 27888));
        assert_eq!((id, p.len()), (0x3043, 0x30));
        assert_eq!(ws(&p, 6, 17), "127.0.0.1");
        assert_eq!((u32_at(&p, 0x28), u32_at(&p, 0x2c)), (27888, 0));
        let (_, p) = split(&room_invite_push(3, "Alice", "1234"));
        assert_eq!((p.len(), u32_at(&p, 6)), (0x40, 3));
        assert_eq!(ws(&p, 0xa, 17), "Alice");
        assert_eq!(ws(&p, 0x2c, 10), "1234");
        let (_, p) = split(&team_change_reply(true, 0));
        assert_eq!((p.len(), u32_at(&p, 6)), (0xe, 1));
    }

    #[test]
    fn profile_layout() {
        let pr = Profile {
            account: 1,
            chan_id: 1,
            name: "Alice".into(),
            exp: 10,
            rank: 2,
            clan_id: 3,
            clan_title: "Testers".into(),
            nation: 17,
            emblem: 5,
            background: 6,
            ..Default::default()
        };
        let (_, p) = split(&profile_reply(true, &pr));
        assert_eq!(p.len(), 0xe2);
        assert_eq!(
            (u32_at(&p, 6), u32_at(&p, 0xe), u32_at(&p, 0x12)),
            (1, 1, 1)
        );
        assert_eq!(ws(&p, 0x16, 17), "Alice");
        assert_eq!(
            (u32_at(&p, 0x38), u32_at(&p, 0x50), u32_at(&p, 0x58)),
            (10, 2, 3)
        );
        assert_eq!(ws(&p, 0x5c, 17), "Testers");
        assert_eq!(
            (u32_at(&p, 0x9e), u32_at(&p, 0xa2), u32_at(&p, 0xae)),
            (5, 6, 17)
        );
    }

    #[test]
    fn item_use_parse() {
        let mut w = PktWriter::new(0x330);
        w.u32(9)
            .wstr("Bob", 17)
            .u32(1)
            .u32(2)
            .u32(6002)
            .u32(10)
            .u32(0)
            .u32(144)
            .u32(16);
        let p = w.into_payload();
        assert_eq!(p.len(), 0x48);
        let u = parse_item_use(&p);
        assert_eq!(
            (
                u.uid,
                u.new_name.as_str(),
                u.face,
                u.colour,
                u.change_item,
                u.nation
            ),
            (9, "Bob", 6002, 10, 144, 16)
        );
    }
}
