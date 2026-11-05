//! The menu's connections to the auth and lobby servers, on a thread of their own so the screens never wait:
//! the screens send [`LinkCmd`]s and read [`LinkEvent`]s.
//!
//! The login steps follow the 2009 client: file check, login, server list (auth connection); server pick, then
//! the lobby connection's start-up requests up to the channel list; the channel is entered by the player. After
//! that every reply and push is read as it arrives. When the game server's address arrives the lobby connection
//! is handed to the game session ([`LinkEvent::Match`]) and comes back with [`LinkCmd::Resume`] when the match is
//! over (the 2009 client keeps its lobby connection up during a match).
//!
//! The password box text goes out once in the login request (the 2009 client's typed-text field) and is never
//! written anywhere: the auth connection is not recorded and its events name the account only.
// DC-DEFAULT: CLI-178 (proposed) the password box text goes into the login request's typed-text field (LIVE 2009
// behaviour, PROTOCOL_2009.md §2.2) and into no log, capture or debug output of dc-client
// SPEC: PROTOCOL_2009.md §1.1, §2.1-§2.2 (auth), §3.1-§3.7 (lobby, rooms), §7 (0x3044, 0x3043, 0x394)
// HARNESS: lobby_harness::login, ::lobby (LOCAL:<id> token, adapter text)
// EVIDENCE: cshell+0x13dff0 (0x20b handler: key stored, auth closed, lobby connect to the entry's address),
// cshell+0x13bf70 (pick: 0x20a with the entry's id through cshell+0x1924a0, box 0x40b)

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dc_botclient::lobby::{
    parse_server_entries, AuthTicket, Lobby, LobbyProfile, RoomMember, RoomSeat, ServerEntry,
    TcpLink, CHANNEL_ENTRY_SIZE, FACE_BASE, LOCAL_TOKEN_PREFIX, REASON_NO_CHARACTER,
    ROOM_RECORD_SIZE, ROOM_USER_RECORD_SIZE,
};
use dc_botclient::record::Recorder;
use dc_wire::*;
use serde_json::json;

/// name-change: the item-use reply (dc-wire lists 0x330 only).
// SPEC: PROTOCOL_2009.md §4.3 0x331 (PROVEN cs+0x2452f0)
pub const ITEM_USE_REPLY: u16 = 0x331;
/// audit-function: the lobby's expiry push (items whose period ended).
// SPEC: PROTOCOL_2009.md §4.2 (0x33f: +6 ok, +0xe count, +0x12 unique ids (at most 100), +0x1a2 re-read flag)
pub const EXPIRY_PUSH: u16 = 0x33f;
/// audit-function (passes-effect): the plain expiry notice, item numbers only.
// SPEC: PROTOCOL_2009.md §4.2 0x3011 (+6 count, +0xa item numbers)
pub const EXPIRY_NOTICE_PUSH: u16 = 0x3011;

/// Width of a `lobby_user_record` (0x371 / 0x3021).
// SPEC: PROTOCOL_2009.md §3.6 table 3.6b (0x42 bytes)
pub const LOBBY_USER_RECORD_SIZE: usize = 0x42;
/// The adapter text the login request carries.
// HARNESS: lobby_harness::login (adapter `00-11-22-33-44-55`)
pub const ADAPTER_TEXT: &str = "00-11-22-33-44-55";
/// The model index a character made from the menu gets (face = 6000 + n).
// DC-DEFAULT: CLI-162 (proposed; amended by CLI-1480: the no-ROM / scripted fallback only) a login without a character
// creates one with the typed ID as nickname, model 2
pub const NEW_CHARACTER_MODEL: i32 = 2;

/// One channel of 0x377.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRow {
    pub id: u32,
    pub name: String,
    pub open: u32,
    pub capacity: u32,
    pub current: u32,
    /// gm-client: the operator slots (capacity, in use) an operator enters by (CLI-1304).
    pub op_capacity: u32,
    pub op_current: u32,
}

/// One room of 0x381 / 0x3031.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoomRow {
    pub number: u32,
    pub title: String,
    pub locked: bool,
    pub host: u32,
    pub map: i32,
    pub mode: i32,
    pub players: i32,
    pub max_players: i32,
    pub state: i32,
}

/// One user of the channel list (0x371 / 0x3021).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserRow {
    pub account: u32,
    pub id: u32,
    pub name: String,
    pub exp: u32,
    /// Clan number (+0x2e), clan mark emblem (+0x32) and background (+0x36); 0 = none.
    // CLI-940: the record's clan fields were dropped, so no row drew a clan mark
    pub clan: u32,
    pub mark: u32,
    pub ground: u32,
    /// Nation index (+0x3e): the row of the country table whose flag the row draws (0 = none).
    // CLI-1024: the field was dropped, so the lobby user list drew no flag
    pub nation: u32,
}

/// The fields of a profile reply 0x379 the lobby's user menu shows.
// SPEC: PROTOCOL_2009.md §6.1 0x379 (account number, in-channel id, wstr[17] name, exp, headshots, kills, deaths,
// wins, losses, rank, grade, clan id, wstr[17] clan name); offsets EVIDENCE: cshell+0x35cf0 (+0xe, +0x12, +0x16
// name, +0x38..+0x50, +0x58 clan id, +0x5c clan name)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfileView {
    pub account: u32,
    pub id: u32,
    pub name: String,
    pub exp: u32,
    pub headshots: u32,
    pub kills: u32,
    pub deaths: u32,
    pub wins: u32,
    pub losses: u32,
    pub clan: String,
    /// +0x50 rank, +0x58 clan id.
    pub rank: u32,
    pub clan_id: u32,
    /// +0xa6 / +0xaa colour numbers, +0xae nation.
    pub name_colour: u32,
    pub clan_colour: u32,
    pub nation: u32,
    /// +0x9e clan emblem, +0xa2 clan mark background.
    pub emblem: u32,
    pub background: u32,
    /// The twelve statistics words +0xb2.. +0xde of the fireteam page (read in the order of the packet).
    pub coop: [u32; 12],
}

/// Reads a profile reply 0x379 (`None` when its ok word is 0).
pub fn profile_view(d: &[u8]) -> Option<ProfileView> {
    (u32_at(d, 6) != 0).then(|| ProfileView {
        account: u32_at(d, 0xe),
        id: u32_at(d, 0x12),
        name: wstr_at(d, 0x16, 17),
        exp: u32_at(d, 0x38),
        headshots: u32_at(d, 0x3c),
        kills: u32_at(d, 0x40),
        deaths: u32_at(d, 0x44),
        wins: u32_at(d, 0x48),
        losses: u32_at(d, 0x4c),
        clan: if u32_at(d, 0x58) != 0 {
            wstr_at(d, 0x5c, 17)
        } else {
            String::new()
        },
        rank: u32_at(d, 0x50),
        clan_id: u32_at(d, 0x58),
        name_colour: u32_at(d, 0xa6),
        clan_colour: u32_at(d, 0xaa),
        nation: u32_at(d, 0xae),
        emblem: u32_at(d, 0x9e),
        background: u32_at(d, 0xa2),
        coop: std::array::from_fn(|i| u32_at(d, 0xb2 + 4 * i)),
    })
}

/// The room the player sits in (0x389).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoomView {
    pub number: u32,
    pub title: String,
    pub locked: bool,
    pub host_id: u32,
    pub map: i32,
    pub mode: i32,
    pub players: i32,
    pub max_players: i32,
    pub state: i32,
    pub goal: i32,
    /// Mid-match join, friendly fire, kill cam.
    pub options: [i32; 3],
    /// Weapon limit (index of the room's weapon rule).
    pub weapons: i32,
    /// gm-client: the host's room-badge level (0 = none; CLI-1303).
    pub badge: i32,
    /// LG-274: the Hacks choices the lobby's mod set offers (`dc_wire::roomhacks` bits; 0 = no Hacks row).
    pub hacks_offered: u32,
    /// LG-274: the room's Hacks setting (`dc_wire::roomhacks` bits).
    pub hacks: u32,
    /// LG-306: this entry offers Hardcore rooms (our push 0x3f08 follows the room info there).
    pub hardcore_offered: bool,
    /// LG-306: the room's Hardcore setting.
    pub hardcore: bool,
    /// LG-305: the room plays Gun Game (`mode` stays the free-for-all row).
    pub gun_game: bool,
    /// LG-307: this entry offers the Possession Arms (our push 0x3f0a follows the room info there).
    pub possession_offered: bool,
    /// LG-307: the room's Possession Arms.
    pub possession: bool,
    /// LG-305: the drawn ladder (weapon rows, rung 0 first) once the room starts.
    pub ladder: Vec<u16>,
}

/// One `inventory_record` of 0x321 (13 u32 words).
// SPEC: PROTOCOL_2009.md §3.3 table 3.3b (+0 unique id, +4 item number, +8 parent unique id, +0xc in-use flag,
// +0x10 / +0x18 FILETIME start / end (0 = permanent), +0x20 use kind, +0x24 remaining count, +0x28 default slot
// kind, +0x2c function word, +0x30 remaining period)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InvRecord {
    pub uid: u32,
    pub item_no: u32,
    pub parent_uid: u32,
    pub in_use: bool,
    pub start: u64,
    pub end: u64,
    pub use_kind: u32,
    pub count: u32,
    pub slot_kind: u32,
    pub function: u32,
    pub period: u32,
}

/// Width of an `inventory_record`.
// SPEC: PROTOCOL_2009.md §3.3 table 3.3b (0x34 bytes)
pub const INV_RECORD_SIZE: usize = 0x34;

/// The records of an inventory reply 0x321 (none when its ok word is 0).
// SPEC: PROTOCOL_2009.md §3.3 (0x321: +6 ok, +0xe i32 count, +0x12 records)
pub fn inventory_records(d: &[u8]) -> Option<Vec<InvRecord>> {
    if u32_at(d, 6) == 0 {
        return None;
    }
    let n = i32_at(d, 0xe).clamp(0, 2048) as usize;
    Some(
        (0..n)
            .map(|i| 0x12 + i * INV_RECORD_SIZE)
            .filter(|o| o + INV_RECORD_SIZE <= d.len())
            .map(|o| InvRecord {
                uid: u32_at(d, o),
                item_no: u32_at(d, o + 4),
                parent_uid: u32_at(d, o + 8),
                in_use: u32_at(d, o + 0xc) != 0,
                start: filetime_at(d, o + 0x10),
                end: filetime_at(d, o + 0x18),
                use_kind: u32_at(d, o + 0x20),
                count: u32_at(d, o + 0x24),
                slot_kind: u32_at(d, o + 0x28),
                function: u32_at(d, o + 0x2c),
                period: u32_at(d, o + 0x30),
            })
            .collect(),
    )
}

/// The item number per equipment slot of an equipment reply 0x323 (-1 = empty; none when its ok word is 0).
// SPEC: PROTOCOL_2009.md §3.3 (0x323: +6 ok, +0x12 i32[15] item number per slot)
pub fn equipment_slots(d: &[u8]) -> Option<[i32; 15]> {
    if u32_at(d, 6) == 0 {
        return None;
    }
    let mut s = [-1; 15];
    for (i, v) in s.iter_mut().enumerate() {
        *v = i32_at(d, 0x12 + 4 * i);
    }
    Some(s)
}

/// function-unequip: the seven function-slot item numbers of an equipment reply 0x323 (+0x8a i32[7], -1 empty; none
/// when its ok word is 0 or the reply is short).
// SPEC: PROTOCOL_2009.md §3.3 (0x323 +0x8a)
// EVIDENCE: cshell+0x229ca0 (the seven words at +0x8a .. +0xa2 kept as n % 9000 at the store's +0xf8, stride 8)
pub fn function_slots(d: &[u8]) -> Option<[i32; 7]> {
    if u32_at(d, 6) == 0 || d.len() < 0x8a + 4 * 7 {
        return None;
    }
    let mut s = [-1; 7];
    for (i, v) in s.iter_mut().enumerate() {
        *v = i32_at(d, 0x8a + 4 * i);
    }
    Some(s)
}

/// function-unequip: the message 0x368 (`put`) / 0x36a for the in-use function item `uid` of item number `item`; it
/// goes out in frame 0x324.
// SPEC: PROTOCOL_2009.md §1.2 (inner-id override), §4.2 (+6 item number, +0xa unique id)
// EVIDENCE: cshell+0x18f7b0 / 0x18f890 (frame 0x324; message 0x368 / 0x36a of 0xe bytes: its second argument, the
// record's item number +8, at +6 and the unique id at +0xa)
pub fn function_slot_message(put: bool, uid: u32, item: u32) -> PktWriter {
    let mut w = PktWriter::new(if put {
        FUNCTION_EQUIP
    } else {
        FUNCTION_UNEQUIP
    });
    w.u32(item).u32(uid);
    w
}

/// One inbox row of 0x32d (0xe8 bytes).
// SPEC: PROTOCOL_2009.md §4.3 (0x32d row: +0 entry id, +4 product id, +8 gift kind (0 own purchase), +0xc opened,
// +0x10 wstr[17] sender, +0x32 FILETIME received, +0x3a wstr[83] message, +0xe0 FILETIME end)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InboxRow {
    pub entry: u32,
    pub product: u32,
    pub gift_kind: u32,
    pub opened: bool,
    pub sender: String,
    pub received: u64,
    pub message: String,
    pub end: u64,
}

/// Width of an inbox row.
// SPEC: PROTOCOL_2009.md §4.3 (rows of 0xe8)
pub const INBOX_ROW_SIZE: usize = 0xe8;

/// The total and the rows of an inbox page reply 0x32d (none when its ok word is 0).
// SPEC: PROTOCOL_2009.md §4.3 (0x32d: +6 ok, +0xe total, +0x12 count, +0x16 rows)
pub fn inbox_page(d: &[u8]) -> Option<(u32, Vec<InboxRow>)> {
    if u32_at(d, 6) == 0 {
        return None;
    }
    let n = (u32_at(d, 0x12) as usize).min(256);
    let rows = (0..n)
        .map(|i| 0x16 + i * INBOX_ROW_SIZE)
        .filter(|o| o + INBOX_ROW_SIZE <= d.len())
        .map(|o| InboxRow {
            entry: u32_at(d, o),
            product: u32_at(d, o + 4),
            gift_kind: u32_at(d, o + 8),
            opened: u32_at(d, o + 0xc) != 0,
            sender: wstr_at(d, o + 0x10, 17),
            received: filetime_at(d, o + 0x32),
            message: wstr_at(d, o + 0x3a, 83),
            end: filetime_at(d, o + 0xe0),
        })
        .collect();
    Some((u32_at(d, 0xe), rows))
}

/// A product id list of 0x351 / 0x353 / 0x355 / 0x357 (`+0x12` count, `+0x16` u16 ids); none when the result word
/// is 0.
// SPEC: PROTOCOL_2009.md §4.1 (0x351: +6 result, +0xa reason, +0xe version, +0x12 count, +0x16 u16[count]; the
// three highlight lists the same)
pub fn product_ids(d: &[u8]) -> Option<Vec<u32>> {
    if u32_at(d, 6) == 0 {
        return None;
    }
    let n = (u32_at(d, 0x12) as usize).min(0x10000);
    Some(
        (0..n)
            .map(|i| 0x16 + 2 * i)
            .filter(|o| o + 2 <= d.len())
            .map(|o| u32::from(u16_at(d, o)))
            .collect(),
    )
}

/// The (product id, price) pairs of a price reply 0x359.
// SPEC: PROTOCOL_2009.md §4.1 (0x359: +0xe version, +0x12 count, +0x16 {u32 product id, u32 price})
pub fn price_pairs(d: &[u8]) -> Vec<(u32, u32)> {
    let n = (u32_at(d, 0x12) as usize).min(0x10000);
    (0..n)
        .map(|i| 0x16 + 8 * i)
        .filter(|o| o + 8 <= d.len())
        .map(|o| (u32_at(d, o), u32_at(d, o + 4)))
        .collect()
}

/// A shop purchase reply 0x329.
// SPEC: PROTOCOL_2009.md §4.2 (0x329: +6 ok, +0xa u16 reason, +0xe unique id, +0x12 FILETIME end, +0x1a GP after)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BuyReply {
    pub ok: bool,
    pub reason: u16,
    pub uid: u32,
    pub gp: u32,
    /// audit-function: +0x1e, the award item of a case opened at the purchase (-1 = none, also for a short reply).
    // EVIDENCE: cshell+0x150c40 (+0x1e: the item a bought supply case gave, -1 for any other purchase)
    pub award: i32,
}

/// The answer to opening an inbox entry (0x32f), the supply-case layout (the client reads it by the opened row's
/// type).
// SPEC: HISEC_2009.md §4.3 (+6 ok, +0xa reason, +0x12 award item number, +0x16 GP total, +0x1a award days, +0x1e unique
// id of the key used, +0x22 award unique id)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpenReply {
    pub ok: bool,
    pub reason: u16,
    pub award_item: u32,
    pub gp: u32,
    pub days: u32,
    pub key_uid: u32,
    pub award_uid: u32,
}

pub fn open_reply(d: &[u8]) -> OpenReply {
    OpenReply {
        ok: u32_at(d, 6) != 0,
        reason: u16_at(d, 0xa),
        award_item: u32_at(d, 0x12),
        gp: u32_at(d, 0x16),
        days: u32_at(d, 0x1a),
        key_uid: u32_at(d, 0x1e),
        award_uid: u32_at(d, 0x22),
    }
}

/// name-change: an item-use reply 0x331 (the fields every use window reads).
// SPEC: PROTOCOL_2009.md §4.3 0x331 (+6 ok, +0xa reason, +0xe unique id of the used item, +0x12 unique id of a second
// item to delete, +0x16 FILETIME end, +0x1e remaining count)
// EVIDENCE: cshell+0x2452f0 (the generic window's reader), cshell+0x249d80 (the name window's reader: +6, +0xa)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UseReply {
    pub ok: bool,
    pub reason: u32,
    pub uid: u32,
    pub removed: u32,
    pub end: u64,
    pub count: u32,
}

pub fn use_reply(d: &[u8]) -> UseReply {
    UseReply {
        ok: u32_at(d, 6) != 0,
        reason: u32_at(d, 0xa),
        uid: u32_at(d, 0xe),
        removed: u32_at(d, 0x12),
        end: u64::from(u32_at(d, 0x16)) | (u64::from(u32_at(d, 0x1a)) << 32),
        count: u32_at(d, 0x1e),
    }
}

/// name-change: the item-use request 0x330 (0x48 bytes).
// SPEC: PROTOCOL_2009.md §4.3 0x330 (+6 u32 unique id; +0xa wstr[17] new name; +0x2c emblem, +0x30 background, +0x34
// face, +0x38 colour, +0x3c used unique id, +0x40 change item number, +0x44 nation)
// EVIDENCE: cshell+0x18e1d0 (the sender; the name copied with at most 16 units)
// use-items: the country window's +0x44
// EVIDENCE: cshell+0x2a9440 (the country window's send: the unique id, an empty name, zeros, the chosen nation last)
pub fn item_use_packet(uid: u32, name: &str, change_item: u32, nation: u32) -> PktWriter {
    let mut w = PktWriter::new(ITEM_USE);
    let name: String = name
        .encode_utf16()
        .take(16)
        .map(|u| char::from_u32(u32::from(u)).unwrap_or('?'))
        .collect();
    w.u32(uid).wstr(&name, 17);
    w.u32(0)
        .u32(0)
        .u32(0)
        .u32(0)
        .u32(0)
        .u32(change_item)
        .u32(nation);
    w
}

/// use-items-sweep: the colour window's 0x330: the unique id, an empty name, the picked colour at +0x38 and the
/// unique id of the item of the same kind in use at +0x3c (0 = none).
// SPEC: PROTOCOL_2009.md §4.3 0x330 (+0x38 colour, +0x3c used unique id)
// EVIDENCE: cshell+0x242b80 (the colour window's send: only with a picked colour; window +0x24 and +0x34)
pub fn colour_use_packet(uid: u32, colour: u32, used: u32) -> PktWriter {
    let mut w = PktWriter::new(ITEM_USE);
    w.u32(uid).wstr("", 17);
    w.u32(0).u32(0).u32(0).u32(colour).u32(used).u32(0).u32(0);
    w
}

/// use-items-emblem: the clan mark window's 0x330: the unique id, an empty name, the picked emblem at +0x2c and
/// background at +0x30.
// SPEC: PROTOCOL_2009.md §4.3 0x330 (+0x2c emblem, +0x30 background)
// EVIDENCE: cshell+0x2ae470 (command 0x262: the unique id, an empty name, the window's emblem and background, zeros)
pub fn mark_use_packet(uid: u32, emblem: u32, background: u32) -> PktWriter {
    let mut w = PktWriter::new(ITEM_USE);
    w.u32(uid).wstr("", 17);
    w.u32(emblem)
        .u32(background)
        .u32(0)
        .u32(0)
        .u32(0)
        .u32(0)
        .u32(0);
    w
}

/// use-items-emblem: the face window's 0x330: the unique id, an empty name and the face value (6000 + the picked
/// face row) at +0x34.
// SPEC: PROTOCOL_2009.md §4.3 0x330 (+0x34 face)
// DISASM: cshell+0x242ad0 (the face window's send: the unique id, an empty name, the picked face + 0x1770 as the face
// word, zeros)
pub fn face_use_packet(uid: u32, face: u32) -> PktWriter {
    let mut w = PktWriter::new(ITEM_USE);
    w.u32(uid).wstr("", 17);
    w.u32(0).u32(0).u32(face).u32(0).u32(0).u32(0).u32(0);
    w
}

/// use-items-emblem: the clan mark check 0x34c (+6 emblem, +0xa background).
// SPEC: PROTOCOL_2009.md §5.1 0x34c
// DISASM: cshell+0x2aada0 (the mark window's check: the window's emblem and background through cshell+0x18e6b0)
pub fn mark_check_packet(emblem: u32, background: u32) -> PktWriter {
    let mut w = PktWriter::new(CLAN_MARK_CHECK);
    w.u32(emblem).u32(background);
    w
}

/// use-items: a megaphone queue reply 0x36f.
// SPEC: PROTOCOL_2009.md §4.3 0x36f (+6 ok; ok: +0xe seconds per message, +0x12 order on air, +0x16 last queued order,
// +0x1e players waiting; refused: +0xa reason)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueueReply {
    pub ok: bool,
    pub reason: u32,
    pub secs: u32,
    pub on_air: u32,
    pub last: u32,
    pub waiting: u32,
}

pub fn queue_reply(d: &[u8]) -> QueueReply {
    QueueReply {
        ok: u32_at(d, 6) != 0,
        reason: u32_at(d, 0xa),
        secs: u32_at(d, 0xe),
        on_air: u32_at(d, 0x12),
        last: u32_at(d, 0x16),
        waiting: u32_at(d, 0x1e),
    }
}

/// use-items: a megaphone send reply 0x36d.
// SPEC: PROTOCOL_2009.md §4.3 0x36d (+6 ok, +0xa reason, +0xe unique id, +0x12 uses left)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SendReply {
    pub ok: bool,
    pub reason: u32,
    pub uid: u32,
    pub left: u32,
}

pub fn send_reply(d: &[u8]) -> SendReply {
    SendReply {
        ok: u32_at(d, 6) != 0,
        reason: u32_at(d, 0xa),
        uid: u32_at(d, 0xe),
        left: u32_at(d, 0x12),
    }
}

/// use-items: the megaphone send 0x36c (0x26a bytes): the item, 1, the message (at most 65 characters) and the colour
/// runs (none: the default colour).
// SPEC: PROTOCOL_2009.md §4.3 0x36c (+6 u32 unique id; +0xa u32 1; +0xe wstr[101] message; +0xd8 wstr[201] colour runs)
// EVIDENCE: cshell+0x2a5be0 (the composer's send: the text copied, 0x41 characters, 1), cshell+0x18f470 (the sender)
pub fn megaphone_packet(uid: u32, text: &str) -> PktWriter {
    let mut w = PktWriter::new(MEGAPHONE_SEND);
    let text: String = text.chars().take(MEGAPHONE_MAX).collect();
    w.u32(uid).u32(1).wstr(&text, 101).pad(402);
    w
}

/// use-items: the most characters a megaphone message has.
// EVIDENCE: cshell+0x2a5be0 / cshell+0x2a8870 (0x41)
pub const MEGAPHONE_MAX: usize = 0x41;

/// audit-function: 0x33f, the expired unique ids and the re-read flag.
pub fn expiry_push(d: &[u8]) -> (Vec<u32>, bool) {
    let n = (u32_at(d, 0xe) as usize).min(100);
    let uids = (0..n)
        .map(|i| 0x12 + 4 * i)
        .filter(|o| o + 4 <= d.len())
        .map(|o| u32_at(d, o))
        .collect();
    (uids, d.len() >= 0x1a6 && u32_at(d, 0x1a2) != 0)
}

pub fn buy_reply(d: &[u8]) -> BuyReply {
    BuyReply {
        ok: u32_at(d, 6) != 0,
        reason: u16_at(d, 0xa),
        uid: u32_at(d, 0xe),
        gp: u32_at(d, 0x1a),
        award: if d.len() >= 0x22 {
            u32_at(d, 0x1e) as i32
        } else {
            -1
        },
    }
}

/// The 0x303 fields the lobby keeps beyond [`LobbyProfile`] (Character Info panel: grade, clan, flag; the chat's
/// mute end; the pending notice flag).
// SPEC: PROTOCOL_2009.md §3.3 table 3.3a (+0x86 clan id, +0x8a wstr[17] clan name, +0xac clan member role, +0xd4 /
// +0xd8 clan emblem / background, +0xec FILETIME chat muted until, +0xf8 grade, +0x100 nation, +0x104 pending
// notice)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CharExtras {
    pub grade: u32,
    pub clan_id: u32,
    pub clan_label: String,
    pub clan_role: u32,
    pub clan_mark: u32,
    pub clan_mark_ground: u32,
    pub nation: u32,
    /// w2-lobbychat: the chat mute end (local FILETIME ticks; 0 = not muted), the field 0x3053 replaces.
    pub muted_until: u64,
    /// w2-lobbychat: a notice waits (the client asks for it with 0x366).
    pub notice: bool,
    /// +0xdc: the inbox holds something new (the inbox is fetched again when its tab opens).
    pub inbox_new: bool,
    /// gm-client: +0xe8 the room-badge flag (shell +0x424; the host's Kick User entry, CLI-1303).
    pub room_badge: bool,
    /// use-windows: +0x62 / +0x66 / +0x6a headshots, kills, deaths (shell +0xd4 / +0xfc / +0x100; the record reset
    /// window shows them and its ok answer clears them).
    pub headshots: u32,
    pub kills: u32,
    pub deaths: u32,
    /// use-items-sweep: +0xe0 / +0xe4 the name and clan-name colour ids (the colour window's ok answer sets them).
    pub name_colour: u32,
    pub clan_colour: u32,
}

/// Reads [`CharExtras`] from a 0x303 frame.
pub fn char_extras(d: &[u8]) -> CharExtras {
    CharExtras {
        grade: u32_at(d, 0xf8),
        clan_id: u32_at(d, 0x86),
        clan_label: wstr_at(d, 0x8a, 17),
        clan_role: u32_at(d, 0xac),
        clan_mark: u32_at(d, 0xd4),
        clan_mark_ground: u32_at(d, 0xd8),
        nation: u32_at(d, 0x100),
        muted_until: u64_at(d, 0xec),
        notice: u32_at(d, 0x104) != 0,
        inbox_new: u32_at(d, 0xdc) != 0,
        // SPEC: PROTOCOL_2009.md §3.3 table 3.3a (+0xe8 room-badge flag)
        room_badge: u32_at(d, 0xe8) != 0,
        // SPEC: PROTOCOL_2009.md §3.3 table 3.3a (+0x62 / +0x66 / +0x6a)
        headshots: u32_at(d, 0x62),
        kills: u32_at(d, 0x66),
        // SPEC: PROTOCOL_2009.md §3.3 table 3.3a (+0xe0 / +0xe4 name / clan-name colour ids)
        name_colour: u32_at(d, 0xe0),
        clan_colour: u32_at(d, 0xe4),
        deaths: u32_at(d, 0x6a),
    }
}

/// A 0x3004 that took effect: the stored experience before it, the new totals and the popup flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProgressApplied {
    pub exp_before: u32,
    pub exp: u32,
    pub gp: u32,
    /// +0xe: with a rank up, the box res 0x185f is shown.
    pub popup: bool,
}

/// w3-clan: the clan record of a clan info reply 0x3c3.
// SPEC: PROTOCOL_2009.md §5.1 0x3c3 (+0xe twelve u32: clan number, member limit, member count, previous rank, rank,
// level, mark word, wins, losses, draws, clan exp, clan points; +0x3e wstr[17] clan name)
// EVIDENCE: cshell+0x106030 (the twelve words and the name read into the clan screen)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClanInfoView {
    pub number: u32,
    pub max_members: u32,
    pub members: u32,
    pub prev_rank: u32,
    pub rank: u32,
    pub level: u32,
    pub mark: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub exp: u32,
    pub points: u32,
    pub name: String,
}

/// Reads a clan info reply 0x3c3 (`None` when its result word is 0: the 2009 screen then only logs a failure).
pub fn clan_info_view(d: &[u8]) -> Option<ClanInfoView> {
    let w = |i: usize| u32_at(d, 0xe + 4 * i);
    (u32_at(d, 6) != 0).then(|| ClanInfoView {
        number: w(0),
        max_members: w(1),
        members: w(2),
        prev_rank: w(3),
        rank: w(4),
        level: w(5),
        mark: w(6),
        wins: w(7),
        losses: w(8),
        draws: w(9),
        exp: w(10),
        points: w(11),
        name: wstr_at(d, 0x3e, 17),
    })
}

/// w3-clan: one member row of a clan member reply 0x3c5.
// SPEC: PROTOCOL_2009.md §5.1 0x3c5 (rows of 0x32 from +0x12: +0 character number, +4 role (5 leader, 0xf the
// second privileged value), +8 exp, +0xc wstr[17] name, +0x2e online)
// EVIDENCE: cshell+0x111990 (steps 0x32 per row)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClanMemberRow {
    pub char_no: u32,
    pub role: u32,
    pub exp: u32,
    pub name: String,
    pub online: bool,
}

/// The clan member roles the 2009 screen tells apart.
// EVIDENCE: cshell+0x111990 (5 and 0xf enable the leader controls), cshell+0x1045f0 (5 -> string 0x1485, 0xf ->
// 0x58e, else 0x58f)
pub const CLAN_ROLE_LEADER: u32 = 5;
pub const CLAN_ROLE_OFFICER: u32 = 0xf;

/// Reads the rows of a clan member reply 0x3c5 (`None` when its result word is 0: the list stays as it was).
pub fn clan_member_rows(d: &[u8]) -> Option<Vec<ClanMemberRow>> {
    if u32_at(d, 6) == 0 {
        return None;
    }
    // the rows that fit in the frame (a count past them is not trusted)
    let n = (i32_at(d, 0xe).max(0) as usize).min(d.len().saturating_sub(0x12) / 0x32);
    Some(
        (0..n)
            .map(|i| {
                let o = 0x12 + i * 0x32;
                ClanMemberRow {
                    char_no: u32_at(d, o),
                    role: u32_at(d, o + 4),
                    exp: u32_at(d, o + 8),
                    name: wstr_at(d, o + 0xc, 17),
                    online: u32_at(d, o + 0x2e) != 0,
                }
            })
            .collect(),
    )
}

/// Reads a clan notice reply 0x3c7 (`None` when its result word is 0).
// SPEC: PROTOCOL_2009.md §5.1 0x3c7 (+6 result, +0xe wstr[121] notice)
// EVIDENCE: cshell+0x105210
pub fn clan_notice_text(d: &[u8]) -> Option<String> {
    (u32_at(d, 6) != 0).then(|| wstr_at(d, 0xe, 121))
}

/// w3-clan: the fields of one DeCombat messenger line (push 0x3f01: a NUL-terminated wstr of tab-separated fields,
/// the first naming the kind).
// SPEC: PROTOCOL_2009.md §6.3 (0x3f01 is a DeCombat extension, not a 2009 message: `status`, `request`, `friends`,
// `online`, `whisper`, `invite` lines)
// DC-DEFAULT: CLI-565 (proposed) the line ends at the first NUL or at 1024 units
pub fn messenger_fields(d: &[u8]) -> Vec<String> {
    let units = (d.len().saturating_sub(6) / 2).min(1024);
    wstr_at(d, 6, units)
        .split('\t')
        .map(str::to_string)
        .collect()
}

/// What the screens ask for. Its `Debug` form never shows the password.
#[derive(Clone)]
pub enum LinkCmd {
    Login {
        auth: SocketAddr,
        user: String,
        password: String,
        nick: String,
    },
    /// CLI-1608: the mod set of the server-list entry the player picked (the later browser screen), kept for the
    /// next login: the lobby's own set must agree with it, and with the web port unreachable only a plain vanilla
    /// entry (or none) joins. `None` forgets it (a direct address).
    ExpectModSet(Option<crate::mods::ServerModSet>),
    /// CLI-1617 / CLI-1618: the player's mod preferences for the next joins: the mod keys
    /// ([`crate::mods::mod_key`]) the player trusts (verified by the community list or accepted before; sending
    /// this turns the consent pause on) and the local mods enabled on the Mods screen.
    ModPrefs {
        trusted: Vec<String>,
        local_enabled: Vec<String>,
    },
    /// CLI-1634: read the optional mods' list of the lobby at `auth`; the answer is [`LinkEvent::ModCatalog`].
    ModCatalog(SocketAddr),
    /// CLI-1632: reads the public mod set of these lobbies of the server list (auth address, name, pin) in the
    /// background, one after the other; each ends in a [`LinkEvent::RowSet`].
    ReadRowSets(Vec<(SocketAddr, String, Option<String>)>),
    /// CLI-1634: download an optional mod from the lobby at the address into `mods/local`; the answer is
    /// [`LinkEvent::ModDownloaded`].
    ModDownload(SocketAddr, crate::modcatalog::CatalogEntry),
    /// CLI-1618: the answer to [`crate::mods::ModState::NeedsConsent`]: true goes on with the download and the
    /// login, false drops the paused login (nothing downloaded).
    ModConsent(bool),
    /// CLI-1616: read the community directory (`None`: switched off) on a thread of its own, through the disk cache
    /// at `cache`; the answer is [`LinkEvent::Community`].
    Community {
        url: Option<String>,
        cache: Option<PathBuf>,
    },
    PickServer(u32),
    /// LG-272 / CLI-1622: leave the lobby logged into (the lobby connection says logout and closes, the auth
    /// connection closes) and unload its mods, before a login at another lobby of the server list.
    Leave,
    /// The server list again over the lobby connection (0x3b0): the server screen opened from the channel screen
    /// or the lobby, or its refresh button.
    ServerList,
    /// Move to another server of the list (0x3b2; the answer's key opens that server's lobby connection).
    MoveServer(u32),
    /// A channel user's profile (0x378 with the user's in-channel id).
    Profile(u32),
    Channels,
    EnterChannel {
        id: u32,
        name: String,
    },
    LeaveChannel,
    Rooms,
    OpenRoom {
        max_players: i32,
        title: String,
        password: String,
        mode: i32,
    },
    JoinRoom {
        number: u32,
        password: String,
    },
    Ready(bool),
    Start,
    Team(u32),
    Setting {
        mode: i32,
        kind: u32,
        value: u32,
    },
    /// LG-274: the host's Hacks pick (our request 0x3f04).
    RoomHacks(u32),
    /// LG-306: the host's Hardcore pick (our request 0x3f09).
    RoomHardcore(bool),
    /// LG-307: the host's Possession pick (our request 0x3f0b).
    RoomPossession(bool),
    /// LG-305: the host's Gun Game pick (our request 0x3f06).
    RoomGunGame(bool),
    QuitRoom,
    /// CLI-1314: the start watchdog ran out: 0x394 {1}.
    StartFailed,
    Chat(String),
    /// w3-options: a player report (0x312, kind 0): the reported name and the description.
    Report {
        name: String,
        text: String,
        kind: u32,
        detail: u32,
    },
    /// w2-lobbychat: the waiting room's away flag (0x392 kind 1: value 2 away after a minute without input, value 0
    /// present again).
    Away(bool),
    /// CLI-1560: the waiting room's shopping flag (0x392 kind 1: value 1 when a bar command leaves the room for the
    /// shop, tuning or clan screen, value 0 back in the room).
    Shopping(bool),
    /// CLI-1561: the private state 0x30a (1 shop, 3 tuning, 4 clan) a room member's shop-family screen sends on
    /// entering it.
    PrivateState(u32),
    /// CLI-1561: the end of the private state (0x30c) when that screen is left.
    PrivateStateEnd,
    Logout,
    Quit,
    /// Back in the waiting room after the result screen: location 1, room info, room users.
    BackToRoom,
    /// w3-shop: the shop's product list (0x350), its three highlight lists (0x352 / 0x354 / 0x356) and the prices
    /// (0x358), each with held version 0.
    ShopLists,
    /// w3-shop: equipment (0x322) and inventory (0x320) again.
    Inventory,
    /// w3-shop: equip (0x324) / unequip (0x326) the item `uid` (item number `item`) in slot `slot`.
    Equip {
        uid: u32,
        item: u32,
        slot: u32,
    },
    Unequip {
        uid: u32,
        item: u32,
        slot: u32,
    },
    /// function-unequip: put the in-use function item `uid` (item number `item`) into its function slot (0x368,
    /// `put`) or take it out (0x36a); both travel in frame 0x324.
    FunctionSlot {
        put: bool,
        uid: u32,
        item: u32,
    },
    /// lobby-fixes10: attach (0x336) / detach (0x338) option `opt_uid` (item number `opt_item`) to / from the weapon
    /// `weapon_uid`.
    Attach {
        opt_uid: u32,
        opt_item: u32,
        weapon_uid: u32,
    },
    Detach {
        opt_uid: u32,
        opt_item: u32,
        weapon_uid: u32,
    },
    /// storage-functions: discard the owned item `uid` (item number `item`, 0x334).
    Discard {
        uid: u32,
        item: u32,
    },
    /// w3-shop: buy product `n` (0x328).
    Buy(u32),
    /// buy `product` (0x328) with payment option `option` and the unique ids of owned items to extend (0 = new).
    BuyWith {
        product: u32,
        option: u32,
        extend: [u32; 5],
    },
    /// w3-shop: inbox page `n` (0x32c).
    InboxPage(u32),
    /// round 10: open inbox entry `entry` of product `product` (0x32e; the answer 0x32f).
    InboxOpen {
        entry: u32,
        product: u32,
    },
    /// name-change: use inventory item `uid` (0x330; the answer 0x331) with a new name (name windows) and the
    /// change item number (weapon change; 0 otherwise).
    ItemUse {
        uid: u32,
        name: String,
        change_item: u32,
        /// use-items: the country window's chosen nation (0 otherwise).
        nation: u32,
    },
    /// use-items: the megaphone queue state (0x36e; the answer 0x36f).
    MegaphoneQueue,
    /// use-items-sweep: the colour window's use of item `uid` with colour index `colour`, replacing the item `used`
    /// of the same kind (0x330; the answer 0x331).
    ColourUse {
        uid: u32,
        colour: u32,
        used: u32,
    },
    /// use-items-emblem: the clan mark window's use of item `uid` with the picked pair (0x330; the answer 0x331).
    MarkUse {
        uid: u32,
        emblem: u32,
        background: u32,
    },
    /// use-items-emblem: may the own clan take this pair (0x34c; the answer 0x34d).
    MarkCheck {
        emblem: u32,
        background: u32,
    },
    /// use-items-emblem: the face window's use of item `uid` with face value `face` (0x330; the answer 0x331).
    FaceUse {
        uid: u32,
        face: u32,
    },
    /// use-items: broadcast `text` with megaphone `uid` (0x36c; the answer 0x36d).
    MegaphoneSend {
        uid: u32,
        text: String,
    },
    /// name-change: is this name free (0x346; the rename window reads the answer 0x347).
    NameCheck(String),
    /// name-change: the own character's name changed (a name window's ok answer): the link's profile takes it.
    OwnName(String),
    /// w3-clan: the clan info request 0x3c2 (clan screen entry; the answer 0x3c3).
    ClanInfo,
    /// w3-clan: the clan member list request 0x3c4.
    ClanMembers,
    /// w3-clan: the clan notice request 0x3c6.
    ClanNotice,
    /// w3-clan: the leader's new clan notice 0x3c8.
    ClanNoticeSet(String),
    /// w3-clan: the location message 0x306 (no reply).
    Location(u32),
    /// w3-clan: an option change 0x308 (index, flag).
    OptionSet {
        index: u32,
        flag: u32,
    },
    /// w3-clan: a DeCombat messenger command (`!nm <verb>` and tab-separated arguments over the chat request).
    Messenger(String),
    /// clan-functions: a GET of the lobby's clan pages (`target` = path and query); the answer comes back as
    /// [`LinkEvent::ClanWeb`].
    ClanWeb {
        addr: SocketAddr,
        target: String,
    },
    /// client-register (CLI-1450): asks the lobby's web port whether it registers; the answer comes back as
    /// [`LinkEvent::RegisterInfo`].
    RegisterInfo {
        addr: SocketAddr,
    },
    /// client-register: posts the registration form; the answer comes back as [`LinkEvent::Registered`].
    Register {
        addr: SocketAddr,
        form: super::signup::RegisterForm,
    },
    /// login-recovery (CLI-1501): asks the lobby's web port whether it can mail (`GET /recover`); the answer comes
    /// back as [`LinkEvent::RecoverInfo`].
    RecoverInfo {
        addr: SocketAddr,
    },
    /// login-recovery: posts a find-ID / find-password request; the answer comes back as [`LinkEvent::Recovered`].
    Recover {
        addr: SocketAddr,
        req: super::signup::RecoverRequest,
    },
    /// nickname-screen: the name check 0x204 on the kept auth connection ([`LinkEvent::NickChecked`]).
    NickCheck(String),
    /// nickname-screen: the character creation 0x202 (`face` = the raw face value, 6000 + the model's face row);
    /// a refusal comes back as [`LinkEvent::CharacterRefused`], success as the server list.
    CreateCharacter {
        face: i32,
        name: String,
    },
    /// nickname-screen: the player left the nickname screen without a character: the auth connection is closed.
    DropCharacter,
    /// gm-client (CLI-1301): an operator's restrict command 0x3d0 (action 1 / 2 restrict, 0 lifts).
    OpRestrict {
        name: String,
        action: u32,
        period: i32,
        reason: String,
    },
    /// gm-client (CLI-1301): an operator's lookup 0x3d2.
    OpLookup(super::gm::OpQuery),
    /// gm-client (CLI-1301): an operator's mute 0x3d4 (`on` false unmutes).
    OpMute {
        name: String,
        on: bool,
        minutes: i32,
    },
    /// gm-client (CLI-1302 / 1303): the room kick 0x38e of the member with in-channel id `id` and character number
    /// `char_no`.
    Kick {
        id: u32,
        char_no: u32,
    },
}

/// One HTTP/1.1 GET of the lobby's clan pages; the body of a 200 answer.
// SPEC: PROTOCOL_2009.md §5.2 (pages served by the DeCombat lobby); DC-DEFAULT: CLI-790 (proposed) the client asks
// for the `fmt=text` form and draws the page itself
pub fn clan_web_get(addr: SocketAddr, target: &str) -> Option<String> {
    use std::io::{Read, Write};
    let t = Duration::from_secs(3);
    // CLI-1470: the web port speaks TLS like the auth port (the registration form carries a password)
    let mut c = dc_botclient::tls::connect(addr, t).ok()?;
    let _ = c.tcp().set_read_timeout(Some(t));
    let _ = c.tcp().set_write_timeout(Some(t));
    let req = format!("GET {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    c.write_all(req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    c.take(1 << 20).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n")?;
    head.lines()
        .next()
        .is_some_and(|l| l.split_whitespace().nth(1) == Some("200"))
        .then(|| body.to_string())
}

/// The player report 0x312 (kind 0).
// SPEC: PROTOCOL_2009.md §3.4 (0xe4c bytes: +6 kind, +0xa / +0xe the player keys, wstr[17] name at +0x12, wstr[601]
// text at +0x34; no reply)
// EVIDENCE: cshell+0x273790 (+0xa is the picked general kind and +0xe the detail index, not player keys)
// DC-DEFAULT: CLI-557 (proposed) kind 0 always (no statistics block, no captured text)
pub fn report_packet(name: &str, text: &str, category: u32, detail: u32) -> PktWriter {
    dc_wire::report::packet(name, text, category, detail)
}

impl std::fmt::Debug for LinkCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkCmd::Login { auth, user, .. } => {
                write!(f, "Login({auth}, {user}, password hidden)")
            }
            LinkCmd::OpenRoom {
                max_players,
                title,
                mode,
                password,
            } => write!(
                f,
                "OpenRoom({max_players}, {title}, mode {mode}, locked {})",
                !password.is_empty()
            ),
            LinkCmd::JoinRoom { number, .. } => write!(f, "JoinRoom({number})"),
            LinkCmd::PickServer(id) => write!(f, "PickServer({id})"),
            LinkCmd::ServerList => write!(f, "ServerList"),
            LinkCmd::MoveServer(id) => write!(f, "MoveServer({id})"),
            LinkCmd::Profile(id) => write!(f, "Profile({id})"),
            LinkCmd::EnterChannel { id, .. } => write!(f, "EnterChannel({id})"),
            LinkCmd::Ready(on) => write!(f, "Ready({on})"),
            LinkCmd::Team(t) => write!(f, "Team({t})"),
            LinkCmd::Setting { kind, value, .. } => write!(f, "Setting({kind}, {value})"),
            LinkCmd::RoomHacks(v) => write!(f, "RoomHacks({v})"),
            LinkCmd::RoomGunGame(v) => write!(f, "RoomGunGame({v})"),
            LinkCmd::RoomHardcore(v) => write!(f, "RoomHardcore({v})"),
            LinkCmd::RoomPossession(v) => write!(f, "RoomPossession({v})"),
            LinkCmd::Chat(_) => write!(f, "Chat"),
            LinkCmd::Report {
                name,
                text,
                kind,
                detail,
            } => write!(f, "Report({name}, {kind}/{detail}, {} chars)", text.chars().count()),
            LinkCmd::Away(on) => write!(f, "Away({on})"),
            LinkCmd::Shopping(on) => write!(f, "Shopping({on})"),
            LinkCmd::PrivateState(v) => write!(f, "PrivateState({v})"),
            LinkCmd::PrivateStateEnd => write!(f, "PrivateStateEnd"),
            LinkCmd::Channels => write!(f, "Channels"),
            LinkCmd::LeaveChannel => write!(f, "LeaveChannel"),
            LinkCmd::Rooms => write!(f, "Rooms"),
            LinkCmd::Start => write!(f, "Start"),
            LinkCmd::QuitRoom => write!(f, "QuitRoom"),
            LinkCmd::StartFailed => write!(f, "StartFailed"),
            LinkCmd::Logout => write!(f, "Logout"),
            LinkCmd::Leave => write!(f, "Leave"),
            LinkCmd::Quit => write!(f, "Quit"),
            LinkCmd::BackToRoom => write!(f, "BackToRoom"),
            LinkCmd::ShopLists => write!(f, "ShopLists"),
            LinkCmd::Inventory => write!(f, "Inventory"),
            LinkCmd::Equip { uid, item, slot } => write!(f, "Equip({uid}, {item}, {slot})"),
            LinkCmd::Unequip { uid, item, slot } => write!(f, "Unequip({uid}, {item}, {slot})"),
            LinkCmd::FunctionSlot {
                put: true,
                uid,
                item,
            } => write!(f, "FunctionEquip({uid}, {item})"),
            LinkCmd::FunctionSlot {
                put: false,
                uid,
                item,
            } => write!(f, "FunctionUnequip({uid}, {item})"),
            LinkCmd::Attach {
                opt_uid,
                opt_item,
                weapon_uid,
            } => write!(f, "Attach({opt_uid}, {opt_item}, {weapon_uid})"),
            LinkCmd::Detach {
                opt_uid,
                opt_item,
                weapon_uid,
            } => write!(f, "Detach({opt_uid}, {opt_item}, {weapon_uid})"),
            LinkCmd::Discard { uid, item } => write!(f, "Discard({uid}, {item})"),
            LinkCmd::Buy(n) => write!(f, "Buy({n})"),
            LinkCmd::BuyWith {
                product,
                option,
                extend,
            } => write!(f, "BuyWith({product}, {option}, {extend:?})"),
            LinkCmd::InboxPage(n) => write!(f, "InboxPage({n})"),
            LinkCmd::InboxOpen { entry, product } => write!(f, "InboxOpen({entry}, {product})"),
            LinkCmd::ItemUse {
                uid,
                name,
                change_item,
                nation,
            } => write!(f, "ItemUse({uid}, {name:?}, {change_item}, {nation})"),
            LinkCmd::MegaphoneQueue => write!(f, "MegaphoneQueue"),
            LinkCmd::ColourUse { uid, colour, used } => {
                write!(f, "ColourUse({uid}, {colour}, {used})")
            }
            LinkCmd::MarkUse {
                uid,
                emblem,
                background,
            } => write!(f, "MarkUse({uid}, {emblem}, {background})"),
            LinkCmd::MarkCheck { emblem, background } => {
                write!(f, "MarkCheck({emblem}, {background})")
            }
            LinkCmd::FaceUse { uid, face } => write!(f, "FaceUse({uid}, {face})"),
            LinkCmd::MegaphoneSend { uid, text } => write!(f, "MegaphoneSend({uid}, {text:?})"),
            LinkCmd::NameCheck(name) => write!(f, "NameCheck({name:?})"),
            LinkCmd::OwnName(name) => write!(f, "OwnName({name:?})"),
            LinkCmd::ClanInfo => write!(f, "ClanInfo"),
            LinkCmd::ClanMembers => write!(f, "ClanMembers"),
            LinkCmd::ClanNotice => write!(f, "ClanNotice"),
            LinkCmd::ClanNoticeSet(_) => write!(f, "ClanNoticeSet"),
            LinkCmd::Location(v) => write!(f, "Location({v})"),
            LinkCmd::OptionSet { index, flag } => write!(f, "OptionSet({index}, {flag})"),
            LinkCmd::Messenger(line) => write!(f, "Messenger({line:?})"),
            LinkCmd::ClanWeb { target, .. } => write!(f, "ClanWeb({target})"),
            LinkCmd::RegisterInfo { addr } => write!(f, "RegisterInfo({addr})"),
            LinkCmd::Register { addr, form } => write!(f, "Register({addr}, {form:?})"),
            LinkCmd::RecoverInfo { addr } => write!(f, "RecoverInfo({addr})"),
            LinkCmd::Recover { addr, req } => write!(f, "Recover({addr}, {req:?})"),
            LinkCmd::OpRestrict {
                name,
                action,
                period,
                reason,
            } => write!(f, "OpRestrict({name}, {action}, {period}, {reason})"),
            LinkCmd::OpLookup(q) => write!(f, "OpLookup({q:?})"),
            LinkCmd::OpMute { name, on, minutes } => write!(f, "OpMute({name}, {on}, {minutes})"),
            LinkCmd::Kick { id, char_no } => write!(f, "Kick({id}, {char_no})"),
            LinkCmd::NickCheck(n) => write!(f, "NickCheck({n:?})"),
            LinkCmd::CreateCharacter { face, name } => {
                write!(f, "CreateCharacter({face}, {name:?})")
            }
            LinkCmd::DropCharacter => write!(f, "DropCharacter"),
            LinkCmd::ExpectModSet(s) => write!(f, "ExpectModSet({})", s.is_some()),
            LinkCmd::ModPrefs {
                trusted,
                local_enabled,
            } => write!(f, "ModPrefs({}, {:?})", trusted.len(), local_enabled),
            LinkCmd::ModConsent(yes) => write!(f, "ModConsent({yes})"),
            LinkCmd::Community { url, .. } => write!(f, "Community({})", url.is_some()),
            LinkCmd::ModCatalog(_) => write!(f, "ModCatalog"),
            LinkCmd::ReadRowSets(v) => write!(f, "ReadRowSets({})", v.len()),
            LinkCmd::ModDownload(_, e) => write!(f, "ModDownload({})", e.id),
        }
    }
}

/// What came back.
pub enum LinkEvent {
    /// A step of the login (for the wait box).
    Step(&'static str),
    /// CLI-1608: the mod step before the login (fetching the set, downloading, verifying, loading, ready, failed).
    Mods(crate::mods::ModState),
    /// CLI-1616: the community directory as read (from the network or the cache), or why not.
    Community(crate::community::CommunityView),
    /// CLI-1634: the optional mods' list as read.
    ModCatalog(crate::modcatalog::CatalogState),
    /// CLI-1632: a modded row's public set as read (`Err`: no answer; what is known of it stays).
    RowSet {
        addr: SocketAddr,
        name: String,
        set: Result<crate::mods::ServerModSet, String>,
    },
    /// CLI-1634: a download ended (`Err`: the text for the player).
    ModDownloaded {
        id: String,
        result: Result<(), String>,
    },
    /// Login refused with the 0x201 reason (0 = connection failure, the text says why).
    LoginRefused {
        reason: u32,
        text: String,
    },
    Servers(Vec<ServerEntry>),
    /// LG-272: the other lobbies the login's lobby lists (our push 0x3f02 before 0x2000; empty without one), sent
    /// right before [`LinkEvent::Servers`].
    OtherLobbies(Vec<OtherLobby>),
    /// 0x20b refused or the lobby login failed.
    ServerRefused(String),
    /// 0x3b3 refused (the 2009 client shows string 0x3f5).
    MoveRefused,
    /// LG-313: a featured server entry was refused or its pack did not load; the text is for the player.
    EntryRefused(String),
    /// A channel user's profile (0x379).
    ProfileInfo(ProfileView),
    Profile(LobbyProfile),
    /// 0x311 map / mode masks.
    RoomSettings(Vec<(u32, u32)>),
    Channels(Vec<ChannelRow>),
    ChannelEntered {
        my_id: u32,
    },
    ChannelRefused,
    ChannelLeft,
    Rooms(Vec<RoomRow>),
    RoomAdded(RoomRow),
    RoomRemoved(u32),
    RoomRowChanged {
        number: u32,
        kind: u32,
        value: u32,
    },
    Users(Vec<UserRow>),
    UserAdded(UserRow),
    UserRemoved(u32),
    RoomCreated {
        number: u32,
        slot: u32,
    },
    CreateRefused(u16),
    RoomEntered {
        number: u32,
        slot: u32,
    },
    EnterRefused(u16),
    RoomInfo(RoomView),
    Members(BTreeMap<u32, RoomMember>),
    MemberAdded(u32, RoomMember),
    MemberRemoved {
        id: u32,
        reason: u32,
    },
    MemberTeam {
        id: u32,
        team: i32,
    },
    /// 0x3042: a member's state (0 waiting, 1 ready, 2 starting, 3 in game) and sub-state (0 none, 1 shopping,
    /// 2 away); also sent for every member of a 0x38b / 0x3041 whose sub-state is set.
    MemberState {
        id: u32,
        state: i32,
        sub: i32,
    },
    RoomSetting {
        kind: u32,
        value: u32,
        text: String,
    },
    /// LG-274: our push 0x3f03, the room's Hacks choices and setting.
    RoomHacks {
        offered: u32,
        value: u32,
    },
    /// LG-306: our push 0x3f08: the room's Hardcore setting.
    RoomHardcore(bool),
    /// LG-307: our push 0x3f0a: the room's Possession Arms.
    RoomPossession(bool),
    /// LG-305: our push 0x3f05: this server entry offers Gun Game.
    GunGameOffer,
    /// LG-305: our push 0x3f07: a room's Gun Game setting and, from its start, the ladder.
    GunGameRoom {
        room: u32,
        on: bool,
        ladder: Vec<u16>,
    },
    FlagReply {
        ok: bool,
        reason: u16,
        kind: i16,
    },
    TeamReply {
        ok: bool,
        reason: u32,
    },
    SettingReply {
        ok: bool,
    },
    Countdown(u32),
    LeftRoom,
    /// w2-lobbychat: a chat line 0x3051 (the server echoes the player's own lines too) with the sender's name colour
    /// id (0 = the chat's colour).
    Chat {
        name: String,
        text: String,
        colour: u32,
    },
    /// w2-lobbychat: the chat mute end (local FILETIME ticks; 0x3053).
    MuteUntil(u64),
    /// w2-lobbychat: a notice for its box (0x367).
    Notice(String),
    /// w2-lobbychat: a text for the lobby's notice bar: 0x3052 (`repeat` passes of `show_ms` each) or a broadcast
    /// 0x3065 (one pass).
    Ticker {
        text: String,
        show_ms: u32,
        repeat: u32,
    },
    /// w2-lobbychat: an invitation to room `number` (0x3023: the host's name and the room's password).
    Invite {
        number: u32,
        host: String,
        password: String,
    },
    /// The game server's address: the lobby connection now belongs to the game session.
    Match {
        server: SocketAddr,
        lobby: Box<Lobby>,
    },
    /// w1-result: the character fields of 0x303 the lobby's Character Info panel shows beyond [`LobbyProfile`]
    /// (sent once after the lobby login, before the channel list).
    CharInfo(CharExtras),
    /// w1-result: the grade push 0x3003 (the same field as the 0x303 grade).
    Grade(u32),
    /// w1-result: the balance reply 0x345 (after the lobby login and after a match): NX Credit and NX Prepaid.
    Balance {
        credit: u32,
        prepaid: u32,
    },
    /// w1-result: a progress push 0x3004 took effect (at once, or when the result screen closed). A
    /// [`LinkEvent::Profile`] with the new totals goes out with it; this one carries what the rank-up boxes need.
    Progress(ProgressApplied),
    /// w3-shop: the inventory records of 0x321 (after the lobby login and after every reread).
    Inventory(Vec<InvRecord>),
    /// w3-shop: the item number per equipment slot of 0x323.
    Equipment([i32; 15]),
    /// function-unequip: the seven function-slot item numbers of 0x323 (+0x8a, -1 empty).
    FunctionSlots([i32; 7]),
    /// function-unequip: the answer to 0x368 (`put`, 0x369) or 0x36a (0x36b): ok and the item's unique id.
    FunctionSlotDone {
        put: bool,
        ok: bool,
        uid: u32,
    },
    /// w3-shop: the products the shop shows (0x351).
    ShopView(Vec<u32>),
    /// w3-shop: highlight list `list` (0 = 0x353, 1 = 0x355, 2 = 0x357).
    Highlight {
        list: usize,
        ids: Vec<u32>,
    },
    /// w3-shop: product prices (0x359).
    Prices(Vec<(u32, u32)>),
    /// w3-shop: an equip (0x325) or unequip (0x327) reply.
    EquipDone {
        ok: bool,
        unequip: bool,
    },
    /// lobby-fixes10: an attach (0x337) or detach (0x339) reply.
    AttachDone {
        ok: bool,
        detach: bool,
    },
    /// storage-functions: a discard reply (0x335).
    Discarded {
        ok: bool,
    },
    /// w3-shop: a purchase reply (0x329).
    Bought(BuyReply),
    /// w3-shop: an inbox page (0x32d): the total and the rows.
    Inbox {
        total: u32,
        rows: Vec<InboxRow>,
    },
    /// round 10: the answer to opening an inbox entry (0x32f).
    InboxOpened(OpenReply),
    /// w3-shop: the inbox holds something new (0x3014).
    InboxStale,
    /// name-change: the answer to an item use (0x331).
    ItemUsed(UseReply),
    /// use-items: the megaphone queue state (0x36f).
    MegaphoneQueued(QueueReply),
    /// use-items: the answer to a megaphone send (0x36d).
    MegaphoneSent(SendReply),
    /// audit-function (CLI-1389): 0x33f, the unique ids of expired items and whether the equipment is read again.
    Expired {
        uids: Vec<u32>,
        refresh: bool,
    },
    /// passes-effect (CLI-1391): 0x3011, the numbers of items whose period ended (a box each, the inventory unchanged).
    ExpiryBoxes(Vec<u32>),
    /// passes-effect (CLI-1392): a megaphone broadcast of the own account (a box besides the ticker line).
    OwnMegaphone,
    /// name-change: the answer to a name check (0x347): the name is free.
    NameChecked(bool),
    /// use-items-emblem: the answer to a clan mark check (0x34d): ok, else the reason.
    MarkChecked {
        ok: bool,
        reason: u32,
    },
    /// w3-clan: an ok clan info reply 0x3c3.
    ClanInfo(ClanInfoView),
    /// w3-clan: an ok clan member reply 0x3c5.
    ClanMembers(Vec<ClanMemberRow>),
    /// w3-clan: an ok clan notice reply 0x3c7.
    ClanNotice(String),
    /// w3-clan: a clan push that asks for a list again (0x3062 members, 0x3063 notice).
    ClanStale(u16),
    /// w3-clan: the clan removal push 0x3064.
    ClanRemoved,
    /// w3-clan: a DeCombat messenger line (0x3f01), as its tab-separated fields.
    Messenger(Vec<String>),
    /// clan-functions: the answer of a clan page request (`None`: no answer, the page server is not reachable).
    ClanWeb {
        target: String,
        body: Option<String>,
    },
    /// client-register: whether the lobby registers (`None`: no answer in time).
    RegisterInfo(Option<super::signup::RegisterInfo>),
    /// client-register: the answer to a registration post.
    Registered(super::signup::RegisterReply),
    /// login-recovery: whether the lobby can mail (`None`: no answer in time; an old lobby = `Some(false)`).
    RecoverInfo(Option<bool>),
    /// login-recovery: the answer to a find-ID / find-password request.
    Recovered(super::signup::RecoverReply),
    /// nickname-screen: the login found no character (0x201 reason 0x0003001e): the nickname screen opens; the auth
    /// connection stays up for the name check and the creation.
    NoCharacter,
    /// nickname-screen: the 0x205 answer for `name` (`free`: the name is available).
    NickChecked {
        name: String,
        free: bool,
    },
    /// nickname-screen: 0x203 refused the creation with `code` (0xe / 0xf: the name is taken).
    CharacterRefused(u32),
    /// gm-client (CLI-1300): the login reply's operator flag (0x201 +0x26 == 2).
    Operator(bool),
    /// CLI-1600: the login reply's character record (0x201 +0x16 exp, +0x1a clan id, +0x1e kills, +0x22 deaths), the
    /// server screen's Character Info before the lobby login.
    LoginRecord {
        exp: u32,
        clan_id: u32,
        kills: u32,
        deaths: u32,
    },
    /// gm-client (CLI-1301): 0x3d1 (`ok` = result 1; `reason` the u16 at +0xa).
    OpRestrictReply {
        ok: bool,
        reason: u16,
    },
    /// gm-client (CLI-1301): 0x3d3.
    OpLookupReply(super::gm::LookupView),
    /// gm-client (CLI-1301): 0x3d5 (`ok` = result non-zero).
    OpMuteReply {
        ok: bool,
        reason: u16,
    },
    /// gm-client (CLI-1303): 0x38f (`reason` the u32 at +0xa).
    KickReply {
        ok: bool,
        reason: u32,
    },
    /// The lobby or auth connection closed or failed.
    Closed(String),
}

impl std::fmt::Debug for LinkEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkEvent::Match { server, .. } => write!(f, "Match({server})"),
            LinkEvent::Step(s) => write!(f, "Step({s})"),
            LinkEvent::Mods(s) => write!(f, "Mods({s:?})"),
            LinkEvent::ModCatalog(_) => write!(f, "ModCatalog"),
            LinkEvent::RowSet { .. } => write!(f, "RowSet"),
            LinkEvent::ModDownloaded { id, result } => {
                write!(f, "ModDownloaded({id}, {})", result.is_ok())
            }
            LinkEvent::Community(v) => write!(
                f,
                "Community({} servers, {:?})",
                v.doc.as_ref().map_or(0, |d| d.servers.len()),
                v.error
            ),
            LinkEvent::LoginRefused { reason, text } => {
                write!(f, "LoginRefused({reason:#x}, {text})")
            }
            LinkEvent::EntryRefused(t) => write!(f, "EntryRefused({t})"),
            LinkEvent::Servers(v) => write!(f, "Servers({})", v.len()),
            LinkEvent::Channels(v) => write!(f, "Channels({})", v.len()),
            LinkEvent::Rooms(v) => write!(f, "Rooms({})", v.len()),
            LinkEvent::Users(v) => write!(f, "Users({})", v.len()),
            LinkEvent::Members(v) => write!(f, "Members({})", v.len()),
            LinkEvent::Closed(s) => write!(f, "Closed({s})"),
            LinkEvent::ServerRefused(s) => write!(f, "ServerRefused({s})"),
            LinkEvent::Grade(g) => write!(f, "Grade({g})"),
            LinkEvent::Balance { credit, prepaid } => write!(f, "Balance({credit}, {prepaid})"),
            LinkEvent::Progress(p) => write!(
                f,
                "Progress({} -> {}, gp {}, popup {})",
                p.exp_before, p.exp, p.gp, p.popup
            ),
            other => write!(f, "{}", event_label(other)),
        }
    }
}

fn event_label(e: &LinkEvent) -> &'static str {
    match e {
        LinkEvent::Profile(_) => "Profile",
        LinkEvent::RoomSettings(_) => "RoomSettings",
        LinkEvent::ChannelEntered { .. } => "ChannelEntered",
        LinkEvent::ChannelRefused => "ChannelRefused",
        LinkEvent::ChannelLeft => "ChannelLeft",
        LinkEvent::RoomAdded(_) => "RoomAdded",
        LinkEvent::RoomRemoved(_) => "RoomRemoved",
        LinkEvent::RoomRowChanged { .. } => "RoomRowChanged",
        LinkEvent::UserAdded(_) => "UserAdded",
        LinkEvent::UserRemoved(_) => "UserRemoved",
        LinkEvent::RoomCreated { .. } => "RoomCreated",
        LinkEvent::CreateRefused(_) => "CreateRefused",
        LinkEvent::RoomEntered { .. } => "RoomEntered",
        LinkEvent::EnterRefused(_) => "EnterRefused",
        LinkEvent::RoomInfo(_) => "RoomInfo",
        LinkEvent::MemberAdded(..) => "MemberAdded",
        LinkEvent::MemberRemoved { .. } => "MemberRemoved",
        LinkEvent::MemberTeam { .. } => "MemberTeam",
        LinkEvent::MemberState { .. } => "MemberState",
        LinkEvent::RoomSetting { .. } => "RoomSetting",
        LinkEvent::RoomHacks { .. } => "RoomHacks",
        LinkEvent::RoomHardcore(_) => "RoomHardcore",
        LinkEvent::RoomPossession(_) => "RoomPossession",
        LinkEvent::GunGameOffer => "GunGameOffer",
        LinkEvent::GunGameRoom { .. } => "GunGameRoom",
        LinkEvent::FlagReply { .. } => "FlagReply",
        LinkEvent::TeamReply { .. } => "TeamReply",
        LinkEvent::SettingReply { .. } => "SettingReply",
        LinkEvent::Countdown(_) => "Countdown",
        LinkEvent::LeftRoom => "LeftRoom",
        LinkEvent::Chat { .. } => "Chat",
        LinkEvent::MuteUntil(_) => "MuteUntil",
        LinkEvent::Notice(_) => "Notice",
        LinkEvent::Ticker { .. } => "Ticker",
        LinkEvent::Invite { .. } => "Invite",
        LinkEvent::CharInfo(_) => "CharInfo",
        LinkEvent::ClanInfo(_) => "ClanInfo",
        LinkEvent::ClanMembers(_) => "ClanMembers",
        LinkEvent::ClanNotice(_) => "ClanNotice",
        LinkEvent::ClanStale(_) => "ClanStale",
        LinkEvent::ClanRemoved => "ClanRemoved",
        LinkEvent::Messenger(_) => "Messenger",
        LinkEvent::ClanWeb { .. } => "ClanWeb",
        LinkEvent::RegisterInfo(_) => "RegisterInfo",
        LinkEvent::Registered(_) => "Registered",
        LinkEvent::RecoverInfo(_) => "RecoverInfo",
        LinkEvent::Recovered(_) => "Recovered",
        LinkEvent::MoveRefused => "MoveRefused",
        LinkEvent::EntryRefused(_) => "EntryRefused",
        LinkEvent::ProfileInfo(_) => "ProfileInfo",
        LinkEvent::Inventory(_) => "Inventory",
        LinkEvent::Equipment(_) => "Equipment",
        LinkEvent::FunctionSlots(_) => "FunctionSlots",
        LinkEvent::FunctionSlotDone { .. } => "FunctionSlotDone",
        LinkEvent::ShopView(_) => "ShopView",
        LinkEvent::Highlight { .. } => "Highlight",
        LinkEvent::Prices(_) => "Prices",
        LinkEvent::EquipDone { .. } => "EquipDone",
        LinkEvent::Discarded { .. } => "Discarded",
        LinkEvent::AttachDone { .. } => "AttachDone",
        LinkEvent::Bought(_) => "Bought",
        LinkEvent::Inbox { .. } => "Inbox",
        LinkEvent::InboxOpened(_) => "InboxOpened",
        LinkEvent::InboxStale => "InboxStale",
        LinkEvent::ItemUsed(_) => "ItemUsed",
        LinkEvent::MegaphoneQueued(_) => "MegaphoneQueued",
        LinkEvent::MegaphoneSent(_) => "MegaphoneSent",
        LinkEvent::Expired { .. } => "Expired",
        LinkEvent::ExpiryBoxes(_) => "ExpiryBoxes",
        LinkEvent::OwnMegaphone => "OwnMegaphone",
        LinkEvent::NameChecked(_) => "NameChecked",
        LinkEvent::MarkChecked { .. } => "MarkChecked",
        _ => "event",
    }
}

/// Options of the menu's connections.
#[derive(Debug, Clone)]
pub struct LinkOptions {
    /// Wait for one reply during the login steps.
    pub timeout: Duration,
    /// The player's session log / capture (the lobby connection only).
    pub log: Option<PathBuf>,
    pub capture: Option<PathBuf>,
    pub name: String,
    /// A login without a character makes one at once (the ID as its name, CLI-162) instead of opening the nickname
    /// screen: scripted runs and runs without a ROM. The player's client sets it off (CLI-1480).
    pub auto_character: bool,
    /// CLI-1608: the data folder and caps of the mod step before each login (no data folder: a modded server
    /// cannot be joined, a vanilla one can).
    pub mods: crate::mods::ModConfig,
    /// CLI-1610: the identity authority the login asks for a token first; None = the plain password login.
    pub identity: Option<crate::identity::Authority>,
}

impl LinkOptions {
    pub fn new(name: &str) -> LinkOptions {
        LinkOptions {
            timeout: Duration::from_secs(10),
            log: None,
            capture: None,
            name: name.to_string(),
            auto_character: true,
            mods: crate::mods::ModConfig::default(),
            identity: None,
        }
    }
}

/// Handle of the connection thread.
pub struct Link {
    tx: Sender<Box<Msg>>,
    rx: Receiver<LinkEvent>,
    handle: Option<JoinHandle<()>>,
    /// Commands sent so far (the step runner's click check).
    sent: std::sync::atomic::AtomicU64,
    /// w3-shop: the last command sent, as its `Debug` text (passwords stay out of it): headless tests.
    last: std::sync::Mutex<String>,
    /// A detached link keeps its commands here ([`Link::detached`]).
    kept: Option<std::sync::Mutex<Receiver<Box<Msg>>>>,
    /// CLI-1608: the mod step of the last login (state, set, hash), written by the connection thread.
    mods: std::sync::Arc<std::sync::Mutex<crate::mods::JoinReport>>,
}

enum Msg {
    Cmd(LinkCmd),
    Resume(Box<Lobby>),
    /// The connection comes back while the result screen is up: read, but no waiting-room request yet.
    ResumeHeld(Box<Lobby>),
}

impl Link {
    pub fn start(opts: LinkOptions) -> Link {
        let (tx, crx) = mpsc::channel::<Box<Msg>>();
        let (etx, rx) = mpsc::channel::<LinkEvent>();
        let mods = std::sync::Arc::new(std::sync::Mutex::new(crate::mods::JoinReport::default()));
        let shared = std::sync::Arc::clone(&mods);
        let handle = std::thread::Builder::new()
            .name("dc-client menu link".into())
            .spawn(move || {
                let mut w = Worker::new(opts, etx);
                w.join_report = shared;
                w.run(crx)
            })
            .ok();
        Link {
            tx,
            rx,
            handle,
            sent: std::sync::atomic::AtomicU64::new(0),
            last: std::sync::Mutex::new(String::new()),
            kept: None,
            mods,
        }
    }

    /// A link with no connection thread: the commands sent are kept for [`Link::take_sent`] (the in-match report
    /// form hands them to the match session's lobby connection).
    pub fn detached() -> Link {
        let (tx, crx) = mpsc::channel::<Box<Msg>>();
        let (_etx, rx) = mpsc::channel::<LinkEvent>();
        Link {
            tx,
            rx,
            handle: None,
            sent: std::sync::atomic::AtomicU64::new(0),
            last: std::sync::Mutex::new(String::new()),
            kept: Some(std::sync::Mutex::new(crx)),
            mods: std::sync::Arc::default(),
        }
    }

    /// The commands a detached link kept since the last call.
    pub fn take_sent(&self) -> Vec<LinkCmd> {
        let Some(k) = &self.kept else {
            return Vec::new();
        };
        let Ok(rx) = k.lock() else {
            return Vec::new();
        };
        rx.try_iter()
            .filter_map(|m| match *m {
                Msg::Cmd(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    pub fn send(&self, c: LinkCmd) {
        self.sent.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut l) = self.last.lock() {
            *l = format!("{c:?}");
        }
        let _ = self.tx.send(Box::new(Msg::Cmd(c)));
    }

    /// Commands sent so far.
    pub fn sent(&self) -> u64 {
        self.sent.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The last command sent (its `Debug` text; empty before the first).
    pub fn latest_request(&self) -> String {
        self.last.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// Gives the lobby connection back after a match (the game session's keeper returned it).
    pub fn resume(&self, lobby: Lobby) {
        let _ = self.tx.send(Box::new(Msg::Resume(Box::new(lobby))));
    }

    /// The connection back from the game while the result screen is up; [`LinkCmd::BackToRoom`] then sends the
    /// waiting room's requests.
    pub fn resume_held(&self, lobby: Lobby) {
        let _ = self.tx.send(Box::new(Msg::ResumeHeld(Box::new(lobby))));
    }

    /// CLI-1608: the mod step of the last login: its state (a failure keeps the reason), the server's set and the
    /// mod-set hash.
    pub fn join_report(&self) -> crate::mods::JoinReport {
        self.mods.lock().map(|m| m.clone()).unwrap_or_default()
    }

    /// Every event that arrived since the last call.
    pub fn poll(&self) -> Vec<LinkEvent> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = self.tx.send(Box::new(Msg::Cmd(LinkCmd::Quit)));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn u32_at(d: &[u8], off: usize) -> u32 {
    PktReader::from_raw(d).at(off).u32()
}
fn i32_at(d: &[u8], off: usize) -> i32 {
    PktReader::from_raw(d).at(off).i32()
}
fn u64_at(d: &[u8], off: usize) -> u64 {
    u64::from(u32_at(d, off)) | (u64::from(u32_at(d, off + 4)) << 32)
}
fn u16_at(d: &[u8], off: usize) -> u16 {
    PktReader::from_raw(d).at(off).u16()
}
fn i16_at(d: &[u8], off: usize) -> i16 {
    PktReader::from_raw(d).at(off).i16()
}
fn wstr_at(d: &[u8], off: usize, n: usize) -> String {
    PktReader::from_raw(d).at(off).wstr(n)
}
/// A FILETIME (lo, hi) at `off`.
fn filetime_at(d: &[u8], off: usize) -> u64 {
    u64::from(u32_at(d, off)) | (u64::from(u32_at(d, off + 4)) << 32)
}

/// A `room_record` (0x7e bytes) at `o`.
// SPEC: PROTOCOL_2009.md §3.6 table 3.6a
pub fn room_row(d: &[u8], o: usize) -> RoomRow {
    RoomRow {
        number: u32_at(d, o),
        title: wstr_at(d, o + 4, 33),
        locked: u32_at(d, o + 0x4a) != 0,
        host: u32_at(d, o + 0x4e),
        map: i32_at(d, o + 0x52),
        mode: i32_at(d, o + 0x56),
        players: i32_at(d, o + 0x5a),
        max_players: i32_at(d, o + 0x5e),
        state: i32_at(d, o + 0x62),
    }
}

/// A `lobby_user_record` (0x42 bytes) at `o`.
// SPEC: PROTOCOL_2009.md §3.6 table 3.6b
pub fn user_row(d: &[u8], o: usize) -> UserRow {
    UserRow {
        account: u32_at(d, o),
        id: u32_at(d, o + 4),
        name: wstr_at(d, o + 8, 17),
        exp: u32_at(d, o + 0x2a),
        clan: u32_at(d, o + 0x2e),
        mark: u32_at(d, o + 0x32),
        ground: u32_at(d, o + 0x36),
        nation: u32_at(d, o + 0x3e),
    }
}

/// A `room_user_record` (0xb8 bytes) at `o`: in-channel id and member.
// SPEC: PROTOCOL_2009.md §3.7 table 3.7b
pub fn member_row(d: &[u8], o: usize) -> (u32, RoomMember) {
    (
        u32_at(d, o + 4),
        RoomMember {
            char_no: i32_at(d, o),
            name: wstr_at(d, o + 0x10, 17),
            team: i32_at(d, o + 0x36),
            state: i32_at(d, o + 0x3a),
            exp: u32_at(d, o + 0x42),
        },
    )
}

/// A [`LinkEvent::MemberState`] for a member record whose sub-state (+0x3e) is set.
// SPEC: PROTOCOL_2009.md §3.7 table 3.7b (+0x3e sub-state, as 0x3042)
fn member_sub(d: &[u8], o: usize, id: u32, state: i32) -> Option<LinkEvent> {
    let sub = i32_at(d, o + 0x3e);
    (sub != 0).then_some(LinkEvent::MemberState { id, state, sub })
}

/// A chat line 0x3051 (`None` without text).
// SPEC: PROTOCOL_2009.md §3.6 (0x3051: +0xa u32 name length, +0xe u32 name colour id, +0x12 wstr[17] name, +0x34 u32
// text length, +0x38 wstr text)
// EVIDENCE: cshell+0x192f0 (name copied up to its length + 1 from +0x12, text up to its length + 1 from +0x38, the
// colour id handed on with them)
pub fn chat_push_event(d: &[u8]) -> Option<LinkEvent> {
    let name_len = (u32_at(d, 0xa) as usize).min(16);
    let len = (u32_at(d, 0x34) as usize).min(0x100);
    let text = wstr_at(d, 0x38, len + 1);
    (!text.is_empty()).then(|| LinkEvent::Chat {
        name: wstr_at(d, 0x12, (name_len + 1).min(17)),
        text,
        colour: u32_at(d, 0xe),
    })
}

/// A ticker notice 0x3052 for the lobby's notice bar.
// SPEC: PROTOCOL_2009.md §3.6 (0x3052: +6 u32 period (s), +0xa u32 repeat count, +0xe u32 text length, +0x12 wstr)
// EVIDENCE: cshell+0x34270 (text copied up to its length + 1; on a lobby screen handed with period, count and the
// colour 0xffeb191e to the screen's notice bar)
pub fn ticker_notice(d: &[u8]) -> Option<LinkEvent> {
    let len = (u32_at(d, 0xe) as usize).min(0x100);
    let text = wstr_at(d, 0x12, len + 1);
    (!text.is_empty()).then(|| LinkEvent::Ticker {
        text,
        show_ms: u32_at(d, 6).saturating_mul(1000),
        repeat: u32_at(d, 0xa).max(1),
    })
}

/// A broadcast 0x3065 for the lobby's notice bar: the system form (type 0) its text, the player form (type 1) the
/// sender's name, the chat separator and the message; both on the air for their seconds.
// SPEC: PROTOCOL_2009.md §3.6 (0x3065: type 0 +0xa seconds, +0x12 wstr[101] text; type 1 +0xa seconds, +0x26
// wstr[17] name, +0x48 wstr[100] message)
// EVIDENCE: cshell+0x1202d0 (type 0: the text with seconds x 1000 to cshell+0x6ce30; type 1: name + the separator
// literal + message with seconds x 1000 to cshell+0x6cd20)
pub fn broadcast(d: &[u8]) -> Option<LinkEvent> {
    let show_ms = u32_at(d, 0xa).saturating_mul(1000);
    let text = match u32_at(d, 6) {
        0 => wstr_at(d, 0x12, 101),
        1 => {
            let msg = wstr_at(d, 0x48, 100);
            if msg.is_empty() {
                return None;
            }
            format!("{} : {msg}", wstr_at(d, 0x26, 17))
        }
        _ => return None,
    };
    (!text.is_empty()).then_some(LinkEvent::Ticker {
        text,
        show_ms,
        repeat: 1,
    })
}

/// The 0x389 room view.
// SPEC: PROTOCOL_2009.md §3.7 table 3.7a
pub fn room_view(d: &[u8]) -> RoomView {
    RoomView {
        number: u32_at(d, 0xe),
        title: wstr_at(d, 0x12, 33),
        locked: i32_at(d, 0x58) != 0,
        host_id: u32_at(d, 0x5c),
        map: i32_at(d, 0x60),
        mode: i32_at(d, 0x64),
        players: i32_at(d, 0x68),
        max_players: i32_at(d, 0x6c),
        state: i32_at(d, 0x70),
        goal: i32_at(d, 0x74),
        options: [i32_at(d, 0x78), i32_at(d, 0x7c), i32_at(d, 0x80)],
        weapons: i32_at(d, 0x84),
        // SPEC: PROTOCOL_2009.md §3.7 table 3.7a (+0x88 host badge level)
        badge: i32_at(d, 0x88),
        // LG-274: our own push 0x3f03 follows the room info
        hacks_offered: 0,
        hacks: 0,
        hardcore_offered: false,
        hardcore: false,
        gun_game: false,
        possession_offered: false,
        possession: false,
        ladder: Vec::new(),
    }
}

/// The frames the steady-state reader handles.
const HANDLED: &[u16] = &[
    dc_wire::seatticket::PUSH,
    CHANNEL_LIST_REPLY,
    CHANNEL_ENTER_REPLY,
    CHANNEL_LEAVE_REPLY,
    ROOM_LIST_REPLY,
    CHANNEL_USERS_REPLY,
    LOBBY_USER_ADD_PUSH,
    LOBBY_USER_REMOVE_PUSH,
    LOBBY_ROOM_ADD_PUSH,
    LOBBY_ROOM_REMOVE_PUSH,
    ROOM_CREATE_REPLY,
    ROOM_ENTER_REPLY,
    ROOM_LEAVE_REPLY,
    ROOM_INFO_REPLY,
    ROOM_USERS_REPLY,
    ROOM_USER_ADD_PUSH,
    ROOM_USER_REMOVE_PUSH,
    ROOM_UPDATE_PUSH,
    ROOM_INFO_STALE_PUSH,
    ROOM_TEAM_PUSH,
    ROOM_USER_STATE_PUSH,
    ROOM_USER_FLAG_REPLY,
    TEAM_CHANGE_REPLY,
    ROOM_SETTING_CHANGE_REPLY,
    START_COUNTDOWN_PUSH,
    MATCH_ADDRESS_PUSH,
    CHAT_PUSH,
    MUTE_UNTIL_PUSH,
    NOTICE_REPLY,
    TICKER_NOTICE_PUSH,
    BROADCAST_PUSH,
    ROOM_INVITE_PUSH,
    PROGRESS_PUSH,
    GRADE_PUSH,
    BALANCE_REPLY,
    SERVER_LIST_REPLY,
    PROFILE_REPLY,
    // w3-shop
    INVENTORY_REPLY,
    EQUIPMENT_REPLY,
    EQUIP_REPLY,
    UNEQUIP_REPLY,
    // function-unequip
    FUNCTION_EQUIP_REPLY,
    FUNCTION_UNEQUIP_REPLY,
    // lobby-fixes10: the Modification screen's attach / detach
    ATTACH,
    ATTACH_REPLY,
    DETACH,
    DETACH_REPLY,
    SHOP_VIEW_REPLY,
    LIST_REPLY_0,
    LIST_REPLY_1,
    LIST_REPLY_2,
    PRICE_REPLY,
    SHOP_BUY_REPLY,
    // storage-functions
    ITEM_DISCARD_REPLY,
    INBOX_PAGE_REPLY,
    INBOX_OPEN,
    INBOX_OPEN_REPLY,
    INBOX_STALE_PUSH,
    // name-change
    ITEM_USE_REPLY,
    // use-items
    MEGAPHONE_QUEUE_REPLY,
    MEGAPHONE_SEND_REPLY,
    // audit-function
    EXPIRY_PUSH,
    EXPIRY_NOTICE_PUSH,
    RECIPIENT_CHECK_REPLY,
    // use-items-emblem
    CLAN_MARK_CHECK_REPLY,
    // w3-clan
    CHARACTER_INFO_REPLY,
    CLAN_LOGIN_REPLY,
    CLAN_INFO_REPLY,
    CLAN_MEMBERS_REPLY,
    CLAN_NOTICE_REPLY,
    CLAN_NOTICE_SET_REPLY,
    CLAN_INFO_STALE_PUSH,
    CLAN_MEMBERS_STALE_PUSH,
    CLAN_NOTICE_STALE_PUSH,
    CLAN_REMOVED_PUSH,
    DC_MESSENGER_PUSH,
    // LG-274
    DC_ROOM_HACKS_PUSH,
    // LG-305, LG-306, LG-307
    dc_wire::gungame::OFFER,
    dc_wire::gungame::ROOM,
    dc_wire::roomhardcore::PUSH,
    dc_wire::possession::PUSH,
    // gm-client
    OP_RESTRICT_REPLY,
    OP_LOOKUP_REPLY,
    OP_MUTE_REPLY,
    ROOM_KICK_REPLY,
];

struct Session {
    link: TcpLink,
    ticket: AuthTicket,
    profile: LobbyProfile,
    room: Option<RoomSeat>,
    channels: Vec<ChannelRow>,
    /// The channel the player asked to enter (id, name).
    entering: Option<(u32, String)>,
    in_room: bool,
    /// client-scoreboard: the result screen is up (a progress push is parked until the waiting room), and the
    /// parked experience total, GP total and popup flag.
    held: bool,
    parked: Option<Progress>,
    /// LG-327: the seat ticket of the coming match's address push.
    seat_ticket: Option<dc_wire::seatticket::Ticket>,
}

/// A progress push 0x3004: experience total, GP total, popup flag.
type Progress = (u32, u32, bool);

/// The experience and GP totals and the popup flag of a progress push 0x3004.
// SPEC: PROTOCOL_2009.md §3 0x3004 (+6 u32 exp total, +0xa u32 GP total, +0xe popup flag; in a match or on the
// result screen the values are parked and applied later)
fn progress_of(d: &[u8]) -> Progress {
    (u32_at(d, 6), u32_at(d, 0xa), u32_at(d, 0xe) != 0)
}

/// The balance reply 0x345 (`None` when its ok word is 0).
// SPEC: PROTOCOL_2009.md §3.3 0x345 (+6 ok, +0xe credit, +0x12 prepaid)
// EVIDENCE: cshell+0x337b0 (the values are taken only when +6 is non-zero: credit -> shell +0x108, prepaid -> +0x11c)
fn balance_of(d: &[u8]) -> Option<LinkEvent> {
    (u32_at(d, 6) != 0).then(|| LinkEvent::Balance {
        credit: u32_at(d, 0xe),
        prepaid: u32_at(d, 0x12),
    })
}

/// The grade push 0x3003.
// SPEC: PROTOCOL_2009.md §3.3 0x3003 (+6 grade -> shell +0x440, the field of 0x303 +0xf8)
// EVIDENCE: cshell+0x36690
fn grade_of(d: &[u8]) -> LinkEvent {
    LinkEvent::Grade(u32_at(d, 6))
}

impl Session {
    /// New totals take effect: the profile's experience and GP, a [`LinkEvent::Profile`] and a
    /// [`LinkEvent::Progress`] (the screens decide the rank-up boxes from the experience before and after).
    // EVIDENCE: cshell+0x40580 (outside a match and the result screen: rank of the new experience above the rank of
    // the stored one -> rank-up flag, box res 0x185f with the popup flag; then GP -> shell +0x104, exp -> +0x194)
    fn apply_progress(&mut self, (exp, gp, popup): Progress, ev: &mut Vec<LinkEvent>) {
        let exp_before = self.profile.exp;
        (self.profile.exp, self.profile.gp) = (exp, gp);
        ev.push(LinkEvent::Profile(self.profile.clone()));
        ev.push(LinkEvent::Progress(ProgressApplied {
            exp_before,
            exp,
            gp,
            popup,
        }));
    }

    /// The result screen is gone: later progress pushes take effect at once, and the parked one now.
    // EVIDENCE: cshell+0x40580 (parking only while screen 7 is up or the match runs: the values go to the waiting
    // room's +0x5960 / +0x5964 / +0x5968), cshell+0x164020 (the waiting room's focus applies them with the same
    // rank-up rule when experience and GP are both non-zero, then clears them)
    // DC-DEFAULT: CLI-386 (proposed) leaving the room from the result screen (command 0x3b) or being removed from it
    // applies the parked values at once; the 2009 client keeps them in the waiting room until its next focus, so
    // its lobby shows the totals from before the match until the next room is entered
    fn release_held(&mut self, ev: &mut Vec<LinkEvent>) {
        self.held = false;
        if let Some(p) = self.parked.take() {
            if p.0 != 0 && p.1 != 0 {
                self.apply_progress(p, ev);
            }
        }
    }
}

/// CLI-1618: what the consent pause decided.
enum Gate {
    /// Go on with the mod step (with the set already read, when the pause read one).
    Go(Option<crate::mods::ServerModSet>),
    /// Wait for [`LinkCmd::ModConsent`].
    Pause(crate::mods::ServerModSet),
}

/// LG-272: one other lobby of the server list, its address resolved on the link's thread (`None`: the name did not
/// resolve). The address is data only: no screen shows it (CLI-1623).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtherLobby {
    pub listed: dc_wire::moddedlist::ListedLobby,
    pub addr: Option<SocketAddr>,
}

/// LG-272: the other lobbies of a 0x3f02 payload with their addresses (a DNS name is resolved here, never on the
/// menu's thread).
pub fn other_lobbies(d: &[u8]) -> Vec<OtherLobby> {
    use std::net::ToSocketAddrs;
    dc_wire::moddedlist::parse(d)
        .into_iter()
        .map(|listed| {
            let addr = listed
                .lobby
                .to_socket_addrs()
                .ok()
                .and_then(|mut a| a.next());
            if let (Some(a), Some((host, _))) = (addr, listed.lobby.rsplit_once(':')) {
                dc_botclient::tls::note_host(a.ip(), host.trim_matches(['[', ']']));
            }
            OtherLobby { listed, addr }
        })
        .collect()
}

/// CLI-1618: a login waiting for the player's answer to the consent prompt.
struct PausedLogin {
    auth: SocketAddr,
    user: String,
    password: String,
    nick: String,
    set: crate::mods::ServerModSet,
}

struct Worker {
    opts: LinkOptions,
    out: Sender<LinkEvent>,
    rec: Recorder,
    auth: Option<(TcpLink, Vec<ServerEntry>, u32)>,
    /// nickname-screen: the auth connection of a login without a character, and its account number, while the
    /// nickname screen is up.
    unnamed: Option<(TcpLink, u32)>,
    session: Option<Session>,
    /// The last server list (the login's 0x2000 or a 0x3b1): a server move connects to its entry's address.
    servers: Vec<ServerEntry>,
    quit: bool,
    /// CLI-1608 / CLI-1609: the sandbox of the server logged into last (none on vanilla) and its join status.
    mods: crate::mods::ClientMods,
    /// The server-list entry's mod set for the next login ([`LinkCmd::ExpectModSet`]).
    expected_mods: Option<crate::mods::ServerModSet>,
    /// The copy of the join status the [`Link`] handle reads.
    join_report: std::sync::Arc<std::sync::Mutex<crate::mods::JoinReport>>,
    /// CLI-1618: the mod keys the player trusts (`None`: no consent pause, scripted runs) and the login paused for
    /// the player's answer, with the set it read.
    trusted: Option<Vec<String>>,
    paused: Option<PausedLogin>,
    /// The mod-set hash of the login in progress (the empty-set hash on a vanilla server): the login message
    /// carries it (LG-271, `dc_wire::loginmods`; HOSTING_AND_MODS.md §4 item 4). `None` before the mod step or
    /// after it failed.
    pub mod_set_hash: Option<String>,
    /// LG-271: the content hash of the login in progress (`0`: the set produced no weapon content).
    pub mod_content_hash: Option<String>,
    /// CLI-1610: the token of this run (also in the cache file).
    token: Option<crate::identity::Cached>,
    /// LG-313: the lobby's auth address (its web port serves the entries' sets), the featured entries of its last server
    /// list, and whether a featured entry's set is what is loaded now.
    login_auth: Option<SocketAddr>,
    entry_sets: Vec<dc_wire::entrymods::EntryHashes>,
    on_entry_set: bool,
}

impl Worker {
    fn new(opts: LinkOptions, out: Sender<LinkEvent>) -> Worker {
        let rec = Recorder::new(opts.capture.as_deref(), opts.log.as_deref(), &opts.name)
            .unwrap_or_else(|_| Recorder::null(&opts.name));
        Worker {
            opts,
            out,
            rec,
            auth: None,
            unnamed: None,
            session: None,
            servers: Vec::new(),
            quit: false,
            mods: crate::mods::ClientMods::new(),
            expected_mods: None,
            join_report: std::sync::Arc::default(),
            trusted: None,
            paused: None,
            mod_set_hash: None,
            mod_content_hash: None,
            token: None,
            login_auth: None,
            entry_sets: Vec::new(),
            on_entry_set: false,
        }
    }

    fn emit(&self, e: LinkEvent) {
        let _ = self.out.send(e);
    }

    fn run(mut self, rx: Receiver<Box<Msg>>) {
        while !self.quit {
            let wait = if self.session.is_some() {
                Duration::from_millis(1)
            } else {
                Duration::from_millis(20)
            };
            match rx.recv_timeout(wait) {
                Ok(m) => self.message(*m),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            while let Ok(m) = rx.try_recv() {
                self.message(*m);
            }
            self.read();
        }
        if let Some(mut s) = self.session.take() {
            s.link.close();
        }
        self.rec.flush();
    }

    fn message(&mut self, m: Msg) {
        match m {
            Msg::Cmd(c) => self.command(c),
            Msg::ResumeHeld(lobby) => {
                let l = *lobby;
                // the result screen is up: the frames that came during the match go, the room's pushes are read
                // from now on; the waiting room's requests wait for the screen's exit
                // DISASM: cshell+0x132400 (exit command 6 -> screen 9, whose focus cshell+0x164020 sends location 1)
                let mut link = l.link;
                // client-scoreboard: the progress push of the match waits for the waiting room (the result screen
                // shows the totals from the values before the match)
                let parked = link
                    .take_ids(&[PROGRESS_PUSH])
                    .last()
                    .map(|(_, d)| progress_of(d));
                // w1-result: the balance and grade of the match end are taken as they come (no parking)
                for e in kept_pushes(&mut link) {
                    self.emit(e);
                }
                link.clear();
                self.session = Some(Session {
                    link,
                    ticket: l.ticket,
                    profile: l.profile,
                    room: l.room,
                    channels: Vec::new(),
                    entering: None,
                    in_room: true,
                    held: true,
                    parked,
                    seat_ticket: None,
                });
            }
            Msg::Resume(lobby) => {
                let l = *lobby;
                let mut s = Session {
                    link: l.link,
                    ticket: l.ticket,
                    profile: l.profile,
                    room: l.room,
                    channels: Vec::new(),
                    entering: None,
                    in_room: true,
                    held: false,
                    parked: None,
                    seat_ticket: None,
                };
                // client-scoreboard: no result screen: the match's progress push applies at once
                let mut ev = Vec::new();
                if let Some((_, d)) = s.link.take_ids(&[PROGRESS_PUSH]).last() {
                    s.apply_progress(progress_of(d), &mut ev);
                }
                ev.extend(kept_pushes(&mut s.link));
                for e in ev {
                    self.emit(e);
                }
                // back from the game screen: location 1, room info, room users (the frames kept meanwhile go)
                // OBSERVED: PROTOCOL_2009.md §7.1 "leaving a running match" (0x306 {1}, 0x388, 0x38a when the game
                // screen is closed)
                s.link.clear();
                let _ = send_u32(&mut s.link, LOCATION, 1);
                let _ = s.link.send_empty(ROOM_INFO);
                let _ = s.link.send_empty(ROOM_USERS);
                self.session = Some(s);
            }
        }
    }

    fn command(&mut self, c: LinkCmd) {
        match c {
            LinkCmd::Quit => self.quit = true,
            LinkCmd::Login {
                auth,
                user,
                password,
                nick,
            } => {
                self.unnamed = None;
                self.paused = None;
                // CLI-1651: a client below the lobby's oldest allowed version says so and does not log in
                if let Err(text) = crate::clientver::before_login(auth) {
                    self.auth = None;
                    self.emit(LinkEvent::LoginRefused { reason: 0, text });
                    return;
                }
                // CLI-1618: with the consent pause on, the set is read first; an untrusted mod waits for the player
                match self.consent_gate(auth) {
                    Ok(Gate::Pause(set)) => {
                        self.paused = Some(PausedLogin {
                            auth,
                            user,
                            password,
                            nick,
                            set,
                        });
                    }
                    Ok(Gate::Go(set)) => self.login_after_mods(auth, &user, &password, &nick, set),
                    Err(text) => {
                        self.auth = None;
                        self.emit(LinkEvent::LoginRefused { reason: 0, text });
                    }
                }
            }
            LinkCmd::ModPrefs {
                trusted,
                local_enabled,
            } => {
                self.trusted = Some(trusted);
                self.opts.mods.local_enabled = Some(local_enabled);
            }
            LinkCmd::ModConsent(yes) => {
                if let Some(p) = self.paused.take() {
                    if yes {
                        // the accepted mods are trusted for this run (the menu remembers them in the profile)
                        let t = self.trusted.get_or_insert_with(Vec::new);
                        t.extend(p.set.mods.iter().map(|m| m.key()));
                        self.login_after_mods(p.auth, &p.user, &p.password, &p.nick, Some(p.set));
                    } else {
                        // DC-DEFAULT: CLI-1618 (proposed) a declined prompt downloads nothing and logs in nowhere
                        self.mods.declined();
                        if let Ok(mut g) = self.join_report.lock() {
                            *g = self.mods.status.clone();
                        }
                        self.rec.event("menu_mods", json!({"declined": true}));
                        self.emit(LinkEvent::Mods(crate::mods::ModState::Idle));
                    }
                }
            }
            LinkCmd::ModCatalog(auth) => {
                let out = self.out.clone();
                let web = super::clanpage::web_addr(Some(auth));
                let paths = self
                    .opts
                    .mods
                    .data_dir
                    .as_deref()
                    .map(crate::mods::ModPaths::new);
                let timeout = self.opts.mods.timeout;
                std::thread::spawn(move || {
                    let v = crate::modcatalog::fetch_catalog(web, paths.as_ref(), timeout);
                    let _ = out.send(LinkEvent::ModCatalog(v));
                });
            }
            LinkCmd::ReadRowSets(rows) => {
                let out = self.out.clone();
                let paths = self
                    .opts
                    .mods
                    .data_dir
                    .as_deref()
                    .map(crate::mods::ModPaths::new);
                std::thread::spawn(move || {
                    for (addr, name, pin) in rows {
                        let set = crate::mods::read_row_set(addr, pin.as_deref());
                        if let (Ok(s), Some(p)) = (&set, &paths) {
                            crate::mods::store_row_previews(addr, pin.as_deref(), s, p);
                        }
                        let _ = out.send(LinkEvent::RowSet { addr, name, set });
                    }
                });
            }
            LinkCmd::ModDownload(auth, entry) => {
                let out = self.out.clone();
                let web = super::clanpage::web_addr(Some(auth));
                let cfg = self.opts.mods.clone();
                std::thread::spawn(move || {
                    let result = match cfg.data_dir.as_deref().map(crate::mods::ModPaths::new) {
                        Some(p) => crate::modcatalog::download(web, &p, &entry, &cfg),
                        None => Err("There is no profile folder to install into.".to_string()),
                    };
                    let _ = out.send(LinkEvent::ModDownloaded {
                        id: entry.id,
                        result,
                    });
                });
            }
            LinkCmd::Community { url, cache } => {
                let out = self.out.clone();
                std::thread::spawn(move || {
                    let v = crate::community::load_view(url.as_deref(), cache.as_deref());
                    let _ = out.send(LinkEvent::Community(v));
                });
            }
            LinkCmd::NickCheck(name) => {
                if let Err(text) = self.name_check(&name) {
                    self.unnamed = None;
                    self.emit(LinkEvent::LoginRefused { reason: 0, text });
                }
            }
            LinkCmd::CreateCharacter { face, name } => {
                if let Err(text) = self.create_character(face, &name) {
                    self.unnamed = None;
                    self.emit(LinkEvent::LoginRefused { reason: 0, text });
                }
            }
            LinkCmd::DropCharacter => {
                if let Some((mut a, _)) = self.unnamed.take() {
                    a.close();
                }
            }
            LinkCmd::PickServer(id) => {
                if let Err(e) = self.pick(id) {
                    self.emit(LinkEvent::ServerRefused(e));
                }
            }
            LinkCmd::MoveServer(id) => self.move_server(id),
            LinkCmd::ExpectModSet(set) => self.expected_mods = set,
            // DC-DEFAULT: CLI-1622 (proposed, project decision) leaving a lobby of the list: logout, both
            // connections closed, its mods unloaded (the cache stays)
            LinkCmd::Leave => {
                self.paused = None;
                if let Some((mut a, _, _)) = self.auth.take() {
                    a.close();
                }
                if let Some((mut a, _)) = self.unnamed.take() {
                    a.close();
                }
                if let Some(mut s) = self.session.take() {
                    let _ = s.link.send_empty(LOGOUT);
                    s.link.close();
                }
                self.mods.unload();
                self.mod_set_hash = None;
                self.mod_content_hash = None;
                if let Ok(mut g) = self.join_report.lock() {
                    *g = self.mods.status.clone();
                }
                self.rec.event("menu_leave", json!({}));
            }
            LinkCmd::Logout => {
                if let Some(s) = self.session.as_mut() {
                    let _ = s.link.send_empty(LOGOUT);
                }
            }
            LinkCmd::ClanWeb { addr, target } => {
                // the page server answers on its own connection: a thread per request, the lobby link goes on
                let out = self.out.clone();
                std::thread::spawn(move || {
                    let body = clan_web_get(addr, &target);
                    let _ = out.send(LinkEvent::ClanWeb { target, body });
                });
            }
            // client-register: the registration requests run on their own thread too (5 s limit each)
            LinkCmd::RegisterInfo { addr } => {
                let out = self.out.clone();
                std::thread::spawn(move || {
                    let _ = out.send(LinkEvent::RegisterInfo(super::signup::fetch_info(addr)));
                });
            }
            LinkCmd::Register { addr, form } => {
                let out = self.out.clone();
                std::thread::spawn(move || {
                    let _ = out.send(LinkEvent::Registered(super::signup::post(addr, &form)));
                });
            }
            LinkCmd::RecoverInfo { addr } => {
                let out = self.out.clone();
                std::thread::spawn(move || {
                    let _ = out.send(LinkEvent::RecoverInfo(super::signup::fetch_recover_info(
                        addr,
                    )));
                });
            }
            LinkCmd::Recover { addr, req } => {
                let out = self.out.clone();
                std::thread::spawn(move || {
                    let _ = out.send(LinkEvent::Recovered(super::signup::recover(addr, &req)));
                });
            }
            other => self.lobby_command(other),
        }
    }

    /// CLI-1618: the consent pause before the mod step. Without a trusted list (no menu profile) nothing pauses.
    /// Else the set is read (nothing downloaded); a set naming a mod whose key is not trusted pauses the login with
    /// [`crate::mods::ModState::NeedsConsent`].
    // DC-DEFAULT: CLI-1618 (proposed) every server-declared mod of the set needs trust (verified or accepted)
    fn consent_gate(&mut self, auth: SocketAddr) -> Result<Gate, String> {
        let Some(trusted) = self.trusted.clone() else {
            return Ok(Gate::Go(None));
        };
        self.mod_set_hash = None;
        self.mod_content_hash = None;
        self.emit(LinkEvent::Step("mods"));
        let web = super::clanpage::web_addr(Some(auth));
        let out = self.out.clone();
        let shared = std::sync::Arc::clone(&self.join_report);
        let r = self.mods.read_set(
            web,
            self.expected_mods.as_ref(),
            &self.opts.mods,
            &mut |s| {
                if let Ok(mut g) = shared.lock() {
                    g.state = s.clone();
                }
                let _ = out.send(LinkEvent::Mods(s));
            },
        );
        let set = match r {
            Ok(set) => set,
            Err(e) => {
                if let Ok(mut g) = self.join_report.lock() {
                    *g = self.mods.status.clone();
                }
                self.rec.event(
                    "menu_mods",
                    json!({"ok": false, "loaded": [], "hash": null}),
                );
                return Err(format!("Cannot load the server's mods: {e}"));
            }
        };
        if set.mods.iter().all(|m| trusted.contains(&m.key())) {
            return Ok(Gate::Go(Some(set)));
        }
        self.mods.wait_consent(&set);
        if let Ok(mut g) = self.join_report.lock() {
            *g = self.mods.status.clone();
        }
        self.rec.event("menu_mods", json!({"consent": "asked"}));
        self.emit(LinkEvent::Mods(crate::mods::ModState::NeedsConsent(
            set.clone(),
        )));
        Ok(Gate::Pause(set))
    }

    /// The mod step (with the set read before, when there is one), then the login.
    fn login_after_mods(
        &mut self,
        auth: SocketAddr,
        user: &str,
        password: &str,
        nick: &str,
        set: Option<crate::mods::ServerModSet>,
    ) {
        self.login_auth = Some(auth);
        self.on_entry_set = false;
        self.entry_sets.clear();
        // CLI-1608: the server's mods first; a failed mod step means no login at all
        let r = self
            .mod_step(auth, set)
            .map_err(|text| (0u32, text))
            .and_then(|()| match self.opts.identity.clone() {
                Some(id) => self.identity_login(&id, auth, user, password, nick),
                None => self.login(auth, user, password, nick),
            });
        if let Err((reason, text)) = r {
            self.auth = None;
            self.emit(LinkEvent::LoginRefused { reason, text });
        }
    }

    /// The mod step before a login (CLI-1608 / CLI-1609): the lobby's mod set from its web port (the auth port +
    /// 8080, or `DC_CLANWEB`), downloaded, verified and loaded; vanilla unloads every mod. `Err` = the reason, kept in
    /// [`Link::join_report`]; the login is then not attempted. `set`: the set the consent pause read already.
    fn mod_step(
        &mut self,
        auth: SocketAddr,
        set: Option<crate::mods::ServerModSet>,
    ) -> Result<(), String> {
        self.mod_set_hash = None;
        if set.is_none() {
            self.emit(LinkEvent::Step("mods"));
        }
        let web = super::clanpage::web_addr(Some(auth));
        let out = self.out.clone();
        let shared = std::sync::Arc::clone(&self.join_report);
        let mut progress = |s: crate::mods::ModState| {
            if let Ok(mut g) = shared.lock() {
                g.state = s.clone();
            }
            let _ = out.send(LinkEvent::Mods(s));
        };
        let r = match set {
            Some(set) => self.mods.join_set(web, set, &self.opts.mods, &mut progress),
            None => self.mods.join(
                web,
                self.expected_mods.as_ref(),
                &self.opts.mods,
                &mut progress,
            ),
        };
        if let Ok(mut g) = self.join_report.lock() {
            *g = self.mods.status.clone();
        }
        self.rec.event(
            "menu_mods",
            json!({"ok": r.is_ok(), "loaded": self.mods.loaded(), "hash": r.as_ref().ok()}),
        );
        let hash = r.map_err(|e| format!("Cannot load the server's mods: {e}"))?;
        self.mod_set_hash = Some(hash);
        self.mod_content_hash = Some(crate::mods::content_hash(&self.mods.status.content));
        Ok(())
    }

    /// CLI-1610: the login through the identity authority: a cached token for `user` when no password was typed,
    /// else a new token for name + password; then the lobby login with `tok:<token>` and an empty password. A lobby
    /// refusing the token (wrong-password reply) drops the cached one.
    // DC-DEFAULT: CLI-1610 (proposed, project decision)
    fn identity_login(
        &mut self,
        id: &crate::identity::Authority,
        auth: SocketAddr,
        user: &str,
        password: &str,
        nick: &str,
    ) -> Result<(), (u32, String)> {
        use crate::identity::{self as ident, Issued};
        let now = ident::now();
        let cached = self
            .token
            .clone()
            .filter(|c| c.usable(now))
            .or_else(|| id.cache.as_deref().and_then(|p| ident::load(p, now)))
            .filter(|c| c.fits(id, user));
        let token = match cached {
            Some(c) if password.is_empty() => c,
            _ => {
                self.emit(LinkEvent::Step("identity"));
                match ident::request_token(id, user, password) {
                    Issued::Token(c) => {
                        if let Some(p) = id.cache.as_deref() {
                            if let Err(e) = ident::store(p, &c) {
                                eprintln!("dc-client: identity cache {}: {e}", p.display());
                            }
                        }
                        self.rec.event(
                            "menu_identity",
                            json!({"authority": id.key, "account": c.account}),
                        );
                        c
                    }
                    Issued::Refused => return Err((ident::REASON_WRONG_PASSWORD, String::new())),
                    // the default authority is the lobby's own host: the password goes to that lobby anyway
                    Issued::NotIssuing if id.derived => {
                        return self.login(auth, user, password, nick)
                    }
                    Issued::Unreachable(_) if id.derived && auth.ip().is_loopback() => {
                        return self.login(auth, user, password, nick)
                    }
                    Issued::NotIssuing => {
                        return Err((0, format!("{} does not issue identity tokens", id.key)))
                    }
                    Issued::Unreachable(text) => {
                        eprintln!("dc-client: identity authority {}: {text}", id.key);
                        return Err((0, text));
                    }
                }
            }
        };
        let r = self.login(
            auth,
            &format!("{}{}", ident::LOGIN_PREFIX, token.token),
            "",
            nick,
        );
        match &r {
            Err((reason, _))
                if *reason != 0 && reason & 0xffff == ident::REASON_WRONG_PASSWORD & 0xffff =>
            {
                self.token = None;
                if let Some(p) = id.cache.as_deref() {
                    ident::clear(p);
                }
            }
            _ => self.token = Some(token),
        }
        r
    }

    /// LG-271: `;m=<set hash>;c=<content hash or 0>` after a mod step, empty without one.
    fn login_suffix(&self) -> String {
        match &self.mod_set_hash {
            Some(set) => dc_wire::loginmods::suffix(
                set,
                self.mod_content_hash
                    .as_deref()
                    .unwrap_or(dc_wire::loginmods::NO_CONTENT),
            ),
            None => String::new(),
        }
    }

    /// Auth half: file check, login (character creation when needed), server list.
    // SPEC: PROTOCOL_2009.md §2.1, §2.2 (0x206, 0x200, 0x204 / 0x202, 0x2000)
    fn login(
        &mut self,
        auth: SocketAddr,
        user: &str,
        password: &str,
        nick: &str,
    ) -> Result<(), (u32, String)> {
        let t = self.opts.timeout;
        let fail = |e: dc_botclient::BotError| (0u32, e.0);
        self.emit(LinkEvent::Step("auth"));
        // the auth connection is not recorded: its login request carries the password box text
        let quiet = Recorder::null(&self.opts.name);
        // CLI-1470: TLS on the auth port (the password never crosses the network in clear text)
        let mut a = TcpLink::connect_auth(auth, "auth", &quiet, t).map_err(fail)?;
        let mut w = PktWriter::new(FILE_CHECK);
        w.pad(32);
        a.send(&w).map_err(fail)?;
        let d = a.recv_until(FILE_CHECK_REPLY, t).map_err(fail)?;
        if u32_at(&d, 6) == 0 {
            return Err((0, "file check refused".into()));
        }
        // SPEC: PROTOCOL_2009.md §2.2 0x200 (+6 token, +0x2006 typed text = the password box, +0x2028 adapter)
        // LG-271: the mod-set and content hashes follow the name (`dc_wire::loginmods`), never without a mod step
        let mods = self.login_suffix();
        let mut w = PktWriter::new(LOGIN_REQUEST);
        w.wstr(&format!("{LOCAL_TOKEN_PREFIX}{user}{mods}"), 4096)
            .wstr(password, 17)
            .wstr(ADAPTER_TEXT, 20)
            .i32(0);
        a.send(&w).map_err(fail)?;
        // CLI-1610: a token never goes into the log
        let shown = if user.starts_with(crate::identity::LOGIN_PREFIX) {
            "tok:(hidden)"
        } else {
            user
        };
        self.rec
            .event("menu_login", json!({"user": shown, "mods": mods}));
        let d = a.recv_until(LOGIN_REPLY, t).map_err(fail)?;
        let account = if u32_at(&d, 6) != 0 {
            // gm-client (CLI-1300): the privilege word; 2 = operator
            // SPEC: PROTOCOL_2009.md §2.2 (0x201 +0x26 i32 privilege)
            self.emit(LinkEvent::Operator(super::gm::is_operator(i32_at(
                &d, 0x26,
            ))));
            // CLI-1600: the character record the 2009 keeps from the login reply (the server screen shows it)
            // SPEC: PROTOCOL_2009.md §2.2 (0x201 +0x16 exp, +0x1a clan id, +0x1e kills, +0x22 deaths)
            // EVIDENCE: cshell+0x42450 (shell +0x194 exp, +0x124 clan id, +0xfc kills, +0x100 deaths)
            self.emit(LinkEvent::LoginRecord {
                exp: u32_at(&d, 0x16),
                clan_id: u32_at(&d, 0x1a),
                kills: u32_at(&d, 0x1e),
                deaths: u32_at(&d, 0x22),
            });
            u32_at(&d, 0x12)
        } else {
            let reason = u32_at(&d, 0xa);
            if reason != REASON_NO_CHARACTER {
                return Err((reason, String::new()));
            }
            self.emit(LinkEvent::Operator(false));
            if !self.opts.auto_character {
                // nickname-screen (CLI-1480): the 2009 client opens its nickname screen (0xe) here and sends the
                // name check and the creation from it; the auth connection waits for them
                // SPEC: PROTOCOL_2009.md §2.1 (reason 0x0003001e -> nickname screen), §2.2 0x201 (+0x12 account)
                self.unnamed = Some((a, u32_at(&d, 0x12)));
                self.emit(LinkEvent::NoCharacter);
                return Ok(());
            }
            // SPEC: PROTOCOL_2009.md §2.2 (0x204 name check, 0x202 create: i32 face, wstr[17] name)
            let mut w = PktWriter::new(NAME_CHECK);
            w.wstr(nick, 17);
            a.send(&w).map_err(fail)?;
            let d = a.recv_until(NAME_CHECK_REPLY, t).map_err(fail)?;
            if u32_at(&d, 6) == 0 {
                return Err((reason, format!("nickname {nick:?} not available")));
            }
            let mut w = PktWriter::new(CHARACTER_CREATE);
            w.i32(FACE_BASE + NEW_CHARACTER_MODEL).wstr(nick, 17);
            a.send(&w).map_err(fail)?;
            let d = a.recv_until(CHARACTER_CREATE_REPLY, t).map_err(fail)?;
            if u32_at(&d, 6) == 0 {
                return Err((reason, "character not created".into()));
            }
            u32_at(&d, 0xe)
        };
        self.server_list(a, account)
    }

    /// The login's server list push 0x2000 (after the login reply or the character creation reply): the server
    /// screen's entries.
    // SPEC: PROTOCOL_2009.md §2.2 (0x2000: +6 result, +0x12 count, +0x16 entries; empty -> error 5)
    fn server_list(&mut self, mut a: TcpLink, account: u32) -> Result<(), (u32, String)> {
        let t = self.opts.timeout;
        let d = a.recv_until(SERVER_LIST_PUSH, t).map_err(|e| (0u32, e.0))?;
        let entries = parse_server_entries(&d, 0x12, 0x16);
        if u32_at(&d, 6) == 0 || entries.is_empty() {
            return Err((u32_at(&d, 0xa), "empty server list".into()));
        }
        // LG-272: our push of the other lobbies came before 0x2000 (none from a lobby without such entries)
        let others: Vec<OtherLobby> = a
            .take_ids(&[dc_wire::moddedlist::MODDED_SERVERS_PUSH])
            .last()
            .map(|(_, d)| other_lobbies(d))
            .unwrap_or_default();
        // LG-313: the featured entries' hashes came before 0x2000 too (none from a lobby without such entries)
        self.entry_sets = a
            .take_ids(&[dc_wire::entrymods::PUSH])
            .last()
            .map(|(_, d)| dc_wire::entrymods::parse_push(d))
            .unwrap_or_default();
        self.rec.event(
            "menu_servers",
            json!({"entries": entries.len(), "account": account, "other_lobbies": others.len()}),
        );
        self.emit(LinkEvent::OtherLobbies(others));
        self.emit(LinkEvent::Servers(entries.clone()));
        self.servers = entries.clone();
        self.auth = Some((a, entries, account));
        Ok(())
    }

    /// nickname-screen: the name check on the kept auth connection; `Err` = the connection failed.
    // SPEC: PROTOCOL_2009.md §2.2 (0x204 +6 wstr[17] name; 0x205 +6 u32 free)
    // EVIDENCE: cshell+0x1921f0 (0x204 sender), cshell+0x12cb70 (0x205 on the nickname screen)
    fn name_check(&mut self, name: &str) -> Result<(), String> {
        let t = self.opts.timeout;
        let Some((a, _)) = self.unnamed.as_mut() else {
            return Err("no login waits for a character".into());
        };
        let mut w = PktWriter::new(NAME_CHECK);
        w.wstr(name, 17);
        a.send(&w).map_err(|e| e.0)?;
        let d = a.recv_until(NAME_CHECK_REPLY, t).map_err(|e| e.0)?;
        self.emit(LinkEvent::NickChecked {
            name: name.to_string(),
            free: u32_at(&d, 6) != 0,
        });
        Ok(())
    }

    /// nickname-screen: the creation on the kept auth connection; success goes on with the server list as a login
    /// does, a refusal keeps the connection for another try.
    // SPEC: PROTOCOL_2009.md §2.2 (0x202 +6 i32 face, +0xa wstr[17] name; 0x203 +6 ok, +0xa code, +0xe account;
    // success: the server follows with 0x2000)
    // EVIDENCE: cshell+0x1922d0 (0x202 sender), cshell+0x12e9d0 (0x203 on the nickname screen)
    fn create_character(&mut self, face: i32, name: &str) -> Result<(), String> {
        let t = self.opts.timeout;
        let Some((mut a, account)) = self.unnamed.take() else {
            return Err("no login waits for a character".into());
        };
        let mut w = PktWriter::new(CHARACTER_CREATE);
        w.i32(face).wstr(name, 17);
        a.send(&w).map_err(|e| e.0)?;
        let d = a.recv_until(CHARACTER_CREATE_REPLY, t).map_err(|e| e.0)?;
        if u32_at(&d, 6) == 0 {
            self.unnamed = Some((a, account));
            self.emit(LinkEvent::CharacterRefused(u32_at(&d, 0xa)));
            return Ok(());
        }
        self.rec
            .event("menu_character", json!({"name": name, "face": face}));
        let account = match u32_at(&d, 0xe) {
            0 => account,
            n => n,
        };
        self.server_list(a, account).map_err(|(_, text)| text)
    }

    /// Server pick and the lobby connection's start-up requests up to the channel list.
    // SPEC: PROTOCOL_2009.md §2.2 (0x20a / 0x20b), §3.1, §3.3 (0x300 .. 0x345), §3.5 (0x310), §3.4 (0x376)
    /// LG-313: before a pick or a move of a featured entry: its set downloaded, verified and loaded (no consent: the
    /// lobby's curated list). `Ok(None)`: a plain entry. `Ok(Some)`: the claim to send before the pick.
    // DC-DEFAULT: LG-313 (proposed, project decision)
    fn entry_load(&mut self, id: u32) -> Result<Option<dc_wire::entrymods::EntryHashes>, String> {
        let Some(want) = self.entry_sets.iter().find(|e| e.entry == id).cloned() else {
            return Ok(None);
        };
        self.emit(LinkEvent::Step("mods"));
        let web = super::clanpage::web_addr(self.login_auth);
        let out = self.out.clone();
        let shared = std::sync::Arc::clone(&self.join_report);
        let mut progress = |s: crate::mods::ModState| {
            if let Ok(mut g) = shared.lock() {
                g.state = s.clone();
            }
            let _ = out.send(LinkEvent::Mods(s));
        };
        let r = self
            .mods
            .join_entry(web, id, &want.set, &self.opts.mods, &mut progress);
        self.on_entry_set = true;
        if let Ok(mut g) = self.join_report.lock() {
            *g = self.mods.status.clone();
        }
        self.rec.event(
            "menu_mods",
            json!({"entry": id, "ok": r.is_ok(), "loaded": self.mods.loaded(), "hash": r.as_ref().ok()}),
        );
        let set = r.map_err(|e| format!("Cannot load this server's content pack: {e}"))?;
        Ok(Some(dc_wire::entrymods::EntryHashes {
            entry: id,
            set,
            content: crate::mods::content_hash(&self.mods.status.content),
        }))
    }

    /// LG-313: leaving a featured entry for a plain one (or a refused pick): back to vanilla, nothing of the pack loaded.
    /// MAP-87: the report's map rows and base go with it (the room list reads the report when it opens).
    // DC-DEFAULT: MAP-87 (proposed)
    fn entry_leave(&mut self) {
        if !std::mem::take(&mut self.on_entry_set) {
            return;
        }
        let vanilla = crate::mods::ServerModSet::vanilla();
        let _ = self
            .mods
            .join_set(None, vanilla, &self.opts.mods, &mut |_| {});
        if let Ok(mut g) = self.join_report.lock() {
            *g = self.mods.status.clone();
        }
        self.rec.event(
            "menu_mods",
            json!({"entry": 0, "ok": true, "loaded": [], "hash": null}),
        );
    }

    fn pick(&mut self, id: u32) -> Result<(), String> {
        let t = self.opts.timeout;
        let Some((mut a, entries, account)) = self.auth.take() else {
            return Err("not logged in".into());
        };
        let entry = entries
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or("unknown server")?;
        let claim = match self.entry_load(id) {
            Ok(c) => c,
            Err(text) => {
                self.entry_leave();
                self.auth = Some((a, entries, account));
                self.emit(LinkEvent::EntryRefused(text.clone()));
                return Ok(());
            }
        };
        self.emit(LinkEvent::Step("server"));
        if let Some(c) = &claim {
            a.send(&dc_wire::entrymods::claim_message(c))
                .map_err(|e| e.0)?;
        }
        send_u32(&mut a, SERVER_PICK, id).map_err(|e| e.0)?;
        let d = a.recv_until(SERVER_PICK_REPLY, t).map_err(|e| e.0)?;
        if u32_at(&d, 6) == 0 {
            let text = a
                .take_ids(&[dc_wire::entrymods::REFUSAL])
                .last()
                .and_then(|(_, d)| dc_wire::entrymods::parse_refusal(d));
            self.entry_leave();
            // the auth connection stays for another pick
            self.auth = Some((a, entries, account));
            if let Some(text) = text {
                self.emit(LinkEvent::EntryRefused(text));
                return Ok(());
            }
            return Err(format!("server pick refused ({})", u32_at(&d, 0xa)));
        }
        if claim.is_none() {
            self.entry_leave();
        }
        let mut key = [0u8; 16];
        key.copy_from_slice(&PktReader::from_raw(&d).at(0xe).bytes_padded(16));
        a.close();
        let ip = entry
            .ip
            .parse::<Ipv4Addr>()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let ticket = AuthTicket {
            session_key: key,
            account_no: account,
            lobby: SocketAddr::new(ip, entry.port as u16),
            server_id: entry.id,
            server_name: entry.name.clone(),
            created_character: false,
        };
        self.open_lobby(ticket)
    }

    /// The lobby connection of `ticket` and its start-up requests up to the channel list (after a server pick or a
    /// server move).
    // SPEC: PROTOCOL_2009.md §3.1, §3.3 (0x300 .. 0x345), §3.5 (0x310), §3.4 (0x376)
    fn open_lobby(&mut self, ticket: AuthTicket) -> Result<(), String> {
        let t = self.opts.timeout;
        self.emit(LinkEvent::Step("lobby"));
        let mut link = TcpLink::connect(ticket.lobby, "lobby", &self.rec, t).map_err(|e| e.0)?;
        let mut p = LobbyProfile::default();
        let mut w = PktWriter::new(LOBBY_LOGIN);
        w.raw(&ticket.session_key).u32(ticket.account_no);
        link.send(&w).map_err(|e| e.0)?;
        let d = link.recv_until(LOBBY_LOGIN_REPLY, t).map_err(|e| e.0)?;
        if u32_at(&d, 6) == 0 {
            return Err("lobby login refused".into());
        }
        p.lobby_index = u32_at(&d, 0x1e);
        for _ in 0..64 {
            link.send_empty(ITEM_SYNC).map_err(|e| e.0)?;
            let d = link.recv_until(ITEM_SYNC_REPLY, t).map_err(|e| e.0)?;
            if i32_at(&d, 0xe) <= 0 {
                break;
            }
        }
        link.send_empty(CHARACTER_INFO).map_err(|e| e.0)?;
        let d = link.recv_until(CHARACTER_INFO_REPLY, t).map_err(|e| e.0)?;
        // SPEC: PROTOCOL_2009.md §3.3 table 3.3a
        p.char_no = u32_at(&d, 0xe);
        p.nick = wstr_at(&d, 0x34, 17);
        p.face = i32_at(&d, 0x56);
        p.gp = u32_at(&d, 0x5a);
        p.exp = u32_at(&d, 0x5e);
        p.clan_id = u32_at(&d, 0x86);
        p.nation = u32_at(&d, 0x100);
        // w2-lobbychat: the extras are fields of this 0x303 (they were read from the inventory reply below)
        let extras = char_extras(&d);
        link.send_empty(EQUIPMENT).map_err(|e| e.0)?;
        let d = link.recv_until(EQUIPMENT_REPLY, t).map_err(|e| e.0)?;
        p.equipped = (0..15).map(|i| i32_at(&d, 0x12 + 4 * i)).collect();
        let slots = equipment_slots(&d);
        let fslots = function_slots(&d);
        link.send_empty(INVENTORY).map_err(|e| e.0)?;
        let d = link.recv_until(INVENTORY_REPLY, t).map_err(|e| e.0)?;
        p.inventory_count = i32_at(&d, 0xe);
        // w3-shop: the inventory records are kept for the shop's storage (they were counted and dropped)
        let records = inventory_records(&d);
        // w3-clan: a character in a clan logs into the clan after the inventory (the answer 0x3c1 is read with the
        // other pushes once the session is up)
        // SPEC: PROTOCOL_2009.md §3.1 (0x321 -> 0x3c0 if clan), §5.1 0x3c0 (header only, after 0x321 when the clan id
        // != 0)
        if p.clan_id != 0 {
            link.send_empty(CLAN_LOGIN).map_err(|e| e.0)?;
        }
        link.send_empty(BALANCE).map_err(|e| e.0)?;
        let d = link.recv_until(BALANCE_REPLY, t).map_err(|e| e.0)?;
        // w1-result: the balance goes to the screens (it was read and dropped)
        let balance = balance_of(&d);
        let mut w = PktWriter::new(ROOM_SETTINGS_FETCH);
        w.u32(0).u32(0);
        link.send(&w).map_err(|e| e.0)?;
        let d = link.recv_until(ROOM_SETTINGS_REPLY, t).map_err(|e| e.0)?;
        // SPEC: PROTOCOL_2009.md §3.5 (0x311: +0x16 count <= 30, +0x1a pairs of map index, mode mask)
        let n = (u32_at(&d, 0x16) as usize).min(30);
        let masks: Vec<(u32, u32)> = (0..n)
            .map(|i| (u32_at(&d, 0x1a + 8 * i), u32_at(&d, 0x1e + 8 * i)))
            .collect();
        self.emit(LinkEvent::RoomSettings(masks));
        self.emit(LinkEvent::Profile(p.clone()));
        let notice = extras.notice;
        self.emit(LinkEvent::CharInfo(extras));
        if let Some(b) = balance {
            self.emit(b);
        }
        if let Some(s) = slots {
            self.emit(LinkEvent::Equipment(s));
        }
        if let Some(s) = fslots {
            self.emit(LinkEvent::FunctionSlots(s));
        }
        if let Some(r) = records {
            self.emit(LinkEvent::Inventory(r));
        }
        link.send_empty(CHANNEL_LIST).map_err(|e| e.0)?;
        // w2-lobbychat: a waiting notice is asked for (its reply 0x367 opens a box on any screen)
        // EVIDENCE: cshell+0x3f2c0 (0x303 handler: +0x104 non-zero -> 0x366 through cshell+0x18dcb0)
        // DC-DEFAULT: CLI-486 (proposed) 0x366 goes out after the start-up requests (with the channel list request)
        // instead of in the middle of them
        if notice {
            link.send_empty(NOTICE_FETCH).map_err(|e| e.0)?;
        }
        self.session = Some(Session {
            link,
            ticket,
            profile: p,
            room: None,
            channels: Vec::new(),
            entering: None,
            in_room: false,
            held: false,
            parked: None,
            seat_ticket: None,
        });
        Ok(())
    }

    /// Server move over the lobby connection: 0x3b2 with the entry's id; an ok 0x3b3 carries the new session key,
    /// the old lobby connection closes and the entry's lobby opens with its start-up requests (the channel list
    /// then opens the channel screen).
    // SPEC: PROTOCOL_2009.md §3.4 (0x3b2 +6 server id; 0x3b3 +6 ok, +0xe u8[16] key; the address comes from the
    // chosen entry; the client reconnects and sends 0x300)
    // EVIDENCE: cshell+0x13e200 (0x3b3: ok 0 -> box 0x3f5; else the key, the entry's server id and name, the old
    // lobby connection closed, the entry's lobby connected and the lobby login sent)
    fn move_server(&mut self, id: u32) {
        let t = self.opts.timeout;
        let Some(entry) = self.servers.iter().find(|e| e.id == id).cloned() else {
            self.emit(LinkEvent::MoveRefused);
            return;
        };
        if self.session.is_none() {
            self.emit(LinkEvent::Closed("no lobby connection".into()));
            return;
        }
        // LG-313: a featured target's pack first; if it fails or is refused, the entry the player is on gets its own back
        let claim = match self.entry_load(id) {
            Ok(c) => c,
            Err(text) => {
                self.entry_back();
                self.emit(LinkEvent::EntryRefused(text));
                return;
            }
        };
        let Some(mut s) = self.session.take() else {
            return;
        };
        self.emit(LinkEvent::Step("server"));
        let claimed = match &claim {
            Some(c) => s.link.send(&dc_wire::entrymods::claim_message(c)),
            None => Ok(()),
        };
        let reply = claimed
            .and_then(|_| send_u32(&mut s.link, SERVER_MOVE, id))
            .and_then(|_| s.link.recv_until(SERVER_MOVE_REPLY, t));
        let d = match reply {
            Ok(d) => d,
            Err(e) => {
                s.link.close();
                self.emit(LinkEvent::Closed(e.0));
                return;
            }
        };
        if u32_at(&d, 6) == 0 {
            let text = s
                .link
                .take_ids(&[dc_wire::entrymods::REFUSAL])
                .last()
                .and_then(|(_, d)| dc_wire::entrymods::parse_refusal(d));
            self.session = Some(s);
            self.entry_back();
            self.emit(match text {
                Some(t) => LinkEvent::EntryRefused(t),
                None => LinkEvent::MoveRefused,
            });
            return;
        }
        if claim.is_none() {
            self.entry_leave();
        }
        let mut key = [0u8; 16];
        key.copy_from_slice(&PktReader::from_raw(&d).at(0xe).bytes_padded(16));
        s.link.close();
        let ip = entry
            .ip
            .parse::<Ipv4Addr>()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let ticket = AuthTicket {
            session_key: key,
            account_no: s.ticket.account_no,
            lobby: SocketAddr::new(ip, entry.port as u16),
            server_id: entry.id,
            server_name: entry.name.clone(),
            created_character: false,
        };
        self.rec
            .event("menu_server_move", json!({"server": entry.id}));
        if let Err(e) = self.open_lobby(ticket) {
            self.emit(LinkEvent::Closed(e));
        }
    }

    /// LG-313: a move did not happen: the set of the entry the session is still on, loaded again. MAP-87: its map rows
    /// and base with it.
    // DC-DEFAULT: MAP-87 (proposed)
    fn entry_back(&mut self) {
        let here = self.session.as_ref().map_or(0, |s| s.ticket.server_id);
        match self.entry_load(here) {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => self.entry_leave(),
        }
    }

    fn lobby_command(&mut self, c: LinkCmd) {
        // name-change: the own name after a name window's ok answer; the screens get the profile again
        // EVIDENCE: cshell+0x249d80 (kind 4 ok: the client sets its own name, cshell+0x131fd0)
        if let LinkCmd::OwnName(name) = &c {
            if let Some(s) = self.session.as_mut() {
                s.profile.nick = name.clone();
                let p = s.profile.clone();
                self.emit(LinkEvent::Profile(p));
            }
            return;
        }
        // DC-DEFAULT: CLI-1561 (proposed) the screens' state notices (location, private state, shopping flag) need no
        // reply: without a lobby connection they are dropped, not reported as a lost connection
        let notice = matches!(
            c,
            LinkCmd::Location(_)
                | LinkCmd::PrivateState(_)
                | LinkCmd::PrivateStateEnd
                | LinkCmd::Shopping(_)
        );
        let Some(s) = self.session.as_mut() else {
            if !notice {
                self.emit(LinkEvent::Closed("no lobby connection".into()));
            }
            return;
        };
        let r = match c {
            LinkCmd::Channels => s.link.send_empty(CHANNEL_LIST),
            // SPEC: PROTOCOL_2009.md §3.4 (0x3b0 header only)
            LinkCmd::ServerList => s.link.send_empty(SERVER_LIST),
            // SPEC: PROTOCOL_2009.md §6.1 (0x378: +6 u32 in-channel id)
            LinkCmd::Profile(id) => send_u32(&mut s.link, PROFILE_FETCH, id),
            LinkCmd::EnterChannel { id, name } => {
                // SPEC: PROTOCOL_2009.md §3.4 (0x372: account number, channel id, wstr[32] name)
                let mut w = PktWriter::new(CHANNEL_ENTER);
                w.u32(s.ticket.account_no).u32(id).wstr(&name, 32);
                s.entering = Some((id, name));
                s.link.send(&w)
            }
            LinkCmd::LeaveChannel => s.link.send_empty(CHANNEL_LEAVE),
            LinkCmd::Rooms => s
                .link
                .send_empty(ROOM_LIST)
                .and_then(|_| send_u32(&mut s.link, LOCATION, 0)),
            LinkCmd::OpenRoom {
                max_players,
                title,
                password,
                mode,
            } => {
                // SPEC: PROTOCOL_2009.md §3.7 (0x382: i32 max players, wstr[33] title, wstr[9] password, i32 mode)
                let mut w = PktWriter::new(ROOM_CREATE);
                w.i32(max_players)
                    .wstr(&title, 33)
                    .wstr(&password, 9)
                    .i32(mode);
                s.room = Some(RoomSeat {
                    host: true,
                    host_id: s.profile.my_id,
                    mode,
                    ..RoomSeat::default()
                });
                s.link.send(&w)
            }
            LinkCmd::JoinRoom { number, password } => {
                // SPEC: PROTOCOL_2009.md §3.7 (0x384: i32 room number, wstr[9] password, wstr[65] extra)
                let mut w = PktWriter::new(ROOM_ENTER);
                w.i32(number as i32).wstr(&password, 9).wstr("", 65);
                s.room = Some(RoomSeat {
                    number,
                    ..RoomSeat::default()
                });
                s.link.send(&w)
            }
            LinkCmd::Ready(on) => send_flag(&mut s.link, 0, on as u32),
            LinkCmd::Start => send_flag(&mut s.link, 0, 2),
            // SPEC: PROTOCOL_2009.md §3.7 (0x394 +6 u32 phase; 1 = the start failed, cshell+0x18b010 via cshell+0x1608b0)
            LinkCmd::StartFailed => send_u32(&mut s.link, MATCH_EXIT, 1),
            // SPEC: PROTOCOL_2009.md §3.7 (0x392 (1, 2) away, (1, 0) present)
            LinkCmd::Away(away) => send_flag(&mut s.link, 1, if away { 2 } else { 0 }),
            // SPEC: PROTOCOL_2009.md §3.7 (0x392 (1, 1) shopping, (1, 0) present)
            // EVIDENCE: cshell+0x164810 (commands 0x3d / 0x8e / 0xa8 / 0xb8 -> 0x392 (1, 1) at cshell+0x1648cb),
            // cshell+0x164020 (the room's focus coming from screen 0x14 / 0x16 / 0x1c -> 0x392 (1, 0) at cshell+0x16420b)
            LinkCmd::Shopping(on) => send_flag(&mut s.link, 1, on as u32),
            // SPEC: PROTOCOL_2009.md §3.4 (0x30a +6 u32 state; 0x30c header only)
            // EVIDENCE: cshell+0x18b6b0 (0x30a), cshell+0x18b780 (0x30c); both only while the player is in a room
            // (shell +0x18 tested before the calls at cshell+0x14f361 / cshell+0x14f78c, cshell+0x159ad5 /
            // cshell+0x159d12, cshell+0x10498e / cshell+0x104c39)
            LinkCmd::PrivateState(v) if s.in_room => send_u32(&mut s.link, PRIVATE_STATE, v),
            LinkCmd::PrivateStateEnd if s.in_room => s.link.send_empty(PRIVATE_STATE_END),
            LinkCmd::PrivateState(_) | LinkCmd::PrivateStateEnd => Ok(()),
            LinkCmd::Team(team) => {
                // SPEC: PROTOCOL_2009.md §3.7 (0x390: +0xe destination team; +6 / +0xa not written by the client)
                let mut w = PktWriter::new(TEAM_CHANGE);
                w.u32(0).u32(0).u32(team);
                s.link.send(&w)
            }
            LinkCmd::Setting { mode, kind, value } => {
                // SPEC: PROTOCOL_2009.md §3.7 (0x38c: i32 mode, u32 field type, u32 value, 0x16 bytes)
                let mut w = PktWriter::new(ROOM_SETTING_CHANGE);
                w.i32(mode).u32(kind).u32(value).u32(0);
                s.link.send(&w)
            }
            LinkCmd::RoomHacks(value) => {
                // DC-DEFAULT: LG-274 (proposed) our request 0x3f04: +6 u32 the wanted Hacks bits
                let mut w = PktWriter::new(roomhacks::ROOM_HACKS_REQUEST);
                w.u32(value);
                s.link.send(&w)
            }
            LinkCmd::RoomHardcore(on) => {
                // DC-DEFAULT: LG-306 (proposed) our request 0x3f09: +6 u32 1 / 0
                let mut w = PktWriter::new(dc_wire::roomhardcore::REQUEST);
                w.u32(on as u32);
                s.link.send(&w)
            }
            LinkCmd::RoomPossession(on) => {
                // DC-DEFAULT: LG-307 (proposed) our request 0x3f0b: +6 u32 1 / 0
                let mut w = PktWriter::new(dc_wire::possession::REQUEST);
                w.u32(on as u32);
                s.link.send(&w)
            }
            LinkCmd::RoomGunGame(on) => {
                // DC-DEFAULT: LG-305 (proposed) our request 0x3f06: +6 u32 1 / 0
                let mut w = PktWriter::new(dc_wire::gungame::REQUEST);
                w.u32(on as u32);
                s.link.send(&w)
            }
            LinkCmd::QuitRoom => {
                // w1-result: a room leave from the result screen (command 0x3b) ends the parking too
                let mut ev = Vec::new();
                if s.held {
                    s.release_held(&mut ev);
                }
                for e in ev {
                    let _ = self.out.send(e);
                }
                s.link.send_empty(ROOM_LEAVE)
            }
            LinkCmd::BackToRoom => {
                // client-scoreboard: the parked progress push applies now (the waiting room shows the new totals)
                let mut ev = Vec::new();
                s.release_held(&mut ev);
                for e in ev {
                    let _ = self.out.send(e);
                }
                // OBSERVED: capture 20261025_204341_realclient (746.047: 0x306 {1}, 0x388, 0x38a
                // when the result screen closed by itself)
                send_u32(&mut s.link, LOCATION, 1)
                    .and_then(|_| s.link.send_empty(ROOM_INFO))
                    .and_then(|_| s.link.send_empty(ROOM_USERS))
            }
            LinkCmd::Chat(text) => {
                // SPEC: PROTOCOL_2009.md §3.6 (0x3a0: wstr[121] text)
                let mut w = PktWriter::new(CHAT_SEND);
                w.wstr(&text, 121);
                s.link.send(&w)
            }
            // SPEC: PROTOCOL_2009.md §4.1 (0x350 / 0x352 / 0x354 / 0x356 / 0x358: +6 u32 held version)
            // DC-DEFAULT: CLI-544 the menu asks for the product list, the three highlight lists and the
            // prices with held version 0 when the shop opens the first time (the 2009 client asks at the lobby
            // login when 0x301 names newer versions, cs+0x420d0)
            LinkCmd::ShopLists => send_u32(&mut s.link, SHOP_VIEW_FETCH, 0)
                .and_then(|_| send_u32(&mut s.link, LIST_FETCH_0, 0))
                .and_then(|_| send_u32(&mut s.link, LIST_FETCH_1, 0))
                .and_then(|_| send_u32(&mut s.link, LIST_FETCH_2, 0))
                .and_then(|_| send_u32(&mut s.link, PRICE_FETCH, 0)),
            // SPEC: PROTOCOL_2009.md §3.3 (0x322 / 0x320 header only)
            LinkCmd::Inventory => reread_items(&mut s.link),
            LinkCmd::Equip { uid, item, slot } | LinkCmd::Unequip { uid, item, slot } => {
                // SPEC: PROTOCOL_2009.md §4.2 (0x324 / 0x326: +6 unique id, +0xa item number, +0xe slot)
                let id = if matches!(c, LinkCmd::Equip { .. }) {
                    EQUIP
                } else {
                    UNEQUIP
                };
                let mut w = PktWriter::new(id);
                w.u32(uid).u32(item).u32(slot);
                s.link.send(&w)
            }
            // SPEC: PROTOCOL_2009.md §1.2 (0x368 / 0x36a in frame 0x324)
            LinkCmd::FunctionSlot { put, uid, item } => s
                .link
                .send_in_frame(EQUIP, &function_slot_message(put, uid, item)),
            LinkCmd::Attach {
                opt_uid,
                opt_item,
                weapon_uid,
            }
            | LinkCmd::Detach {
                opt_uid,
                opt_item,
                weapon_uid,
            } => {
                // SPEC: PROTOCOL_2009.md §4.2 (0x336 / 0x338: +6 option unique id, +0xa option item number, +0xe
                // parent weapon unique id)
                // EVIDENCE: cshell+0x18b1c0, cshell+0x18b2a0
                let id = if matches!(c, LinkCmd::Attach { .. }) {
                    ATTACH
                } else {
                    DETACH
                };
                let mut w = PktWriter::new(id);
                w.u32(opt_uid).u32(opt_item).u32(weapon_uid);
                s.link.send(&w)
            }
            // SPEC: PROTOCOL_2009.md §4.2 (0x334: +6 unique id, +0xa item number)
            // EVIDENCE: cshell+0x142ca0 (the storage page's discard: the selected item's unique id and item number to
            // the sender cshell+0x18a480)
            LinkCmd::Discard { uid, item } => {
                let mut w = PktWriter::new(ITEM_DISCARD);
                w.u32(uid).u32(item);
                s.link.send(&w)
            }
            LinkCmd::Buy(product) => {
                // SPEC: PROTOCOL_2009.md §4.2 (0x328: +6 product id, +0xa option (0), +0xe u32[5] unique ids to
                // extend (0 = new))
                let mut w = PktWriter::new(SHOP_BUY);
                w.u32(product).u32(0);
                for _ in 0..5 {
                    w.u32(0);
                }
                s.link.send(&w)
            }
            LinkCmd::BuyWith {
                product,
                option,
                extend,
            } => {
                // SPEC: PROTOCOL_2009.md §4.2 (0x328: +6 product id, +0xa option, +0xe u32[5] unique ids to extend)
                // EVIDENCE: cshell+0x18cf50 (the sender), cshell+0x145b60 (its caller: option and extension ids)
                let mut w = PktWriter::new(SHOP_BUY);
                w.u32(product).u32(option);
                for e in extend {
                    w.u32(e);
                }
                s.link.send(&w)
            }
            // SPEC: PROTOCOL_2009.md §4.3 (0x32c: +6 u32 page)
            LinkCmd::InboxPage(page) => send_u32(&mut s.link, INBOX_PAGE, page),
            // SPEC: PROTOCOL_2009.md §4.3 (0x32e: +6 inbox entry id, +0xa product id); HISEC_2009.md §4.2 step 4
            // EVIDENCE: cshell+0x18dfe0 (the sender)
            LinkCmd::InboxOpen { entry, product } => {
                let mut w = PktWriter::new(INBOX_OPEN);
                w.u32(entry).u32(product);
                s.link.send(&w)
            }
            LinkCmd::ItemUse {
                uid,
                name,
                change_item,
                nation,
            } => s
                .link
                .send(&item_use_packet(uid, &name, change_item, nation)),
            // SPEC: PROTOCOL_2009.md §4.3 0x36e (header only)
            // EVIDENCE: cshell+0x18f5d0 (the sender, called by the composer's OK cshell+0x2a8d60)
            LinkCmd::MegaphoneQueue => s.link.send_empty(MEGAPHONE_QUEUE),
            LinkCmd::ColourUse { uid, colour, used } => {
                s.link.send(&colour_use_packet(uid, colour, used))
            }
            LinkCmd::MarkUse {
                uid,
                emblem,
                background,
            } => s.link.send(&mark_use_packet(uid, emblem, background)),
            LinkCmd::MarkCheck { emblem, background } => {
                s.link.send(&mark_check_packet(emblem, background))
            }
            LinkCmd::FaceUse { uid, face } => s.link.send(&face_use_packet(uid, face)),
            LinkCmd::MegaphoneSend { uid, text } => s.link.send(&megaphone_packet(uid, &text)),
            // SPEC: PROTOCOL_2009.md §4.3 0x346 (+6 wstr[17] name)
            // EVIDENCE: cshell+0x18e410 (the sender), cshell+0x242660 (the rename window's check)
            LinkCmd::NameCheck(name) => {
                let mut w = PktWriter::new(RECIPIENT_CHECK);
                w.wstr(&name, 17);
                s.link.send(&w)
            }
            // taken before the match (it answers with an event)
            LinkCmd::OwnName(_) => Ok(()),
            LinkCmd::Report {
                name,
                text,
                kind,
                detail,
            } => s.link.send(&report_packet(&name, &text, kind, detail)),
            // w3-clan
            // SPEC: PROTOCOL_2009.md §5.1 (0x3c2 / 0x3c4 / 0x3c6 header only; 0x3c8 +6 wstr[121] notice)
            // EVIDENCE: cshell+0x18d630 (0x3c2), cshell+0x18d6f0 (0x3c4), cshell+0x18d7b0 (0x3c6), cshell+0x18d870
            // (0x3c8)
            LinkCmd::ClanInfo => s.link.send_empty(CLAN_INFO),
            LinkCmd::ClanMembers => s.link.send_empty(CLAN_MEMBERS),
            LinkCmd::ClanNotice => s.link.send_empty(CLAN_NOTICE),
            LinkCmd::ClanNoticeSet(text) => {
                let mut w = PktWriter::new(CLAN_NOTICE_SET);
                w.wstr(&text, 121);
                s.link.send(&w)
            }
            // SPEC: PROTOCOL_2009.md §3.4 (0x306: +6 u32 where; 0x308: +6 u32 index, +0xa u32 flag)
            // EVIDENCE: cshell+0x18ac20 (0x306), cshell+0x18da20 (0x308)
            LinkCmd::Location(v) => send_u32(&mut s.link, LOCATION, v),
            LinkCmd::OptionSet { index, flag } => {
                let mut w = PktWriter::new(OPTION_SET);
                w.u32(index).u32(flag);
                s.link.send(&w)
            }
            // SPEC: PROTOCOL_2009.md §6.3 (DeCombat messenger relay: `!nm <verb>` lines over 0x3a0)
            LinkCmd::Messenger(line) => {
                let mut w = PktWriter::new(CHAT_SEND);
                w.wstr(&line, 121);
                s.link.send(&w)
            }
            LinkCmd::OpRestrict {
                name,
                action,
                period,
                reason,
            } => s
                .link
                .send(&super::gm::restrict_packet(&name, action, period, &reason)),
            LinkCmd::OpLookup(q) => s.link.send(&super::gm::lookup_packet(&q)),
            LinkCmd::OpMute { name, on, minutes } => {
                s.link.send(&super::gm::mute_packet(&name, on, minutes))
            }
            LinkCmd::Kick { id, char_no } => s.link.send(&super::gm::kick_packet(id, char_no)),
            _ => Ok(()),
        };
        if let Err(e) = r {
            self.session = None;
            self.emit(LinkEvent::Closed(e.0));
        }
    }

    /// Reads what arrived on the lobby connection and turns it into events.
    fn read(&mut self) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let alive = s.link.pump(Duration::from_millis(15)).unwrap_or(false);
        let frames = s.link.take_ids(HANDLED);
        s.link.clear();
        let mut events = Vec::new();
        let mut hand_over: Option<SocketAddr> = None;
        for (id, d) in frames {
            if let Some(a) = dispatch(s, id, &d, &mut events) {
                hand_over = Some(a);
                break;
            }
        }
        for e in events {
            if let LinkEvent::Servers(v) = &e {
                self.servers = v.clone();
            }
            self.emit(e);
        }
        if let Some(server) = hand_over {
            if let Some(s) = self.session.take() {
                self.rec
                    .event("menu_match", json!({"server": server.to_string()}));
                let seat_ticket = s.seat_ticket;
                let mut lobby = Lobby::from_parts(
                    s.link,
                    s.ticket,
                    s.profile,
                    s.room,
                    self.opts.timeout,
                    self.rec.clone(),
                );
                lobby.seat_ticket = seat_ticket;
                self.emit(LinkEvent::Match {
                    server,
                    lobby: Box::new(lobby),
                });
            }
            return;
        }
        if !alive && self.session.as_ref().is_some_and(|s| s.link.is_closed()) {
            self.session = None;
            self.emit(LinkEvent::Closed("lobby connection closed".into()));
        }
    }
}

/// Equipment (0x322) and inventory (0x320) again.
// SPEC: PROTOCOL_2009.md §3.3 (0x322 / 0x320 header only)
fn reread_items(link: &mut TcpLink) -> dc_botclient::BotResult<()> {
    link.send_empty(EQUIPMENT)
        .and_then(|_| link.send_empty(INVENTORY))
}

fn send_u32(link: &mut TcpLink, id: u16, v: u32) -> dc_botclient::BotResult<()> {
    let mut w = PktWriter::new(id);
    w.u32(v);
    link.send(&w)
}

/// 0x392 `[kind][value]`: (0, 0/1) ready off / on, (0, 2) start, (1, 0) present.
// SPEC: PROTOCOL_2009.md §3.7 (0x392)
fn send_flag(link: &mut TcpLink, kind: u32, value: u32) -> dc_botclient::BotResult<()> {
    let mut w = PktWriter::new(ROOM_USER_FLAG);
    w.u32(kind).u32(value);
    link.send(&w)
}

/// One received frame; returns the game server's address when 0x3043 arrived.
fn dispatch(s: &mut Session, id: u16, d: &[u8], ev: &mut Vec<LinkEvent>) -> Option<SocketAddr> {
    let my_id = s.profile.my_id;
    match id {
        CHANNEL_LIST_REPLY => {
            // SPEC: PROTOCOL_2009.md §3.4 (0x377 channel_entry: id, wstr[33] name, open, capacity, current)
            let n = i32_at(d, 0xe).clamp(0, 64) as usize;
            s.channels = (0..n)
                .map(|i| 0x12 + i * CHANNEL_ENTRY_SIZE)
                .filter(|o| o + CHANNEL_ENTRY_SIZE <= d.len())
                .map(|o| ChannelRow {
                    id: u32_at(d, o),
                    name: wstr_at(d, o + 4, 33),
                    open: u32_at(d, o + 0x46),
                    capacity: u32_at(d, o + 0x4a),
                    current: u32_at(d, o + 0x4e),
                    // EVIDENCE: cshell+0xfab60 (an operator's entry compares the row's +0x52 capacity with +0x56)
                    op_capacity: u32_at(d, o + 0x52),
                    op_current: u32_at(d, o + 0x56),
                })
                .collect();
            ev.push(LinkEvent::Channels(s.channels.clone()));
        }
        CHANNEL_ENTER_REPLY => {
            if u32_at(d, 6) == 0 {
                ev.push(LinkEvent::ChannelRefused);
            } else {
                s.profile.my_id = u32_at(d, 0xe);
                if let Some((cid, name)) = s.entering.take() {
                    s.profile.chan_id = cid;
                    s.profile.chan_label = name;
                }
                ev.push(LinkEvent::ChannelEntered {
                    my_id: s.profile.my_id,
                });
                ev.push(LinkEvent::Profile(s.profile.clone()));
                // the lobby screen: room list, location 0 (the users follow the room list)
                // SPEC: PROTOCOL_2009.md §3.1 (0x380 -> 0x381, then 0x370; 0x306(0))
                let _ = s.link.send_empty(ROOM_LIST);
                let _ = send_u32(&mut s.link, LOCATION, 0);
            }
        }
        CHANNEL_LEAVE_REPLY => ev.push(LinkEvent::ChannelLeft),
        SERVER_LIST_REPLY => {
            // SPEC: PROTOCOL_2009.md §3.4 (0x3b1: +6 ok, +0xe count, +0x12 entries; nothing when ok or count is 0)
            // EVIDENCE: cshell+0x13f0b0
            let v = parse_server_entries(d, 0xe, 0x12);
            if u32_at(d, 6) != 0 && !v.is_empty() {
                ev.push(LinkEvent::Servers(v));
            }
        }
        PROFILE_REPLY => ev.extend(profile_view(d).map(LinkEvent::ProfileInfo)),
        // client-scoreboard: new experience and GP totals; parked while the result screen is up
        // SPEC: PROTOCOL_2009.md §3 0x3004
        // DC-DEFAULT: CLI-385 (proposed) the popup flag's rank-up box is not shown
        PROGRESS_PUSH => {
            let p = progress_of(d);
            if s.held {
                s.parked = Some(p);
            } else {
                s.apply_progress(p, ev);
            }
        }
        GRADE_PUSH => ev.push(grade_of(d)),
        BALANCE_REPLY => ev.extend(balance_of(d)),
        ROOM_LIST_REPLY => {
            let n = (u32_at(d, 0xe) as usize).min(256);
            let rows = (0..n)
                .map(|i| 0x12 + i * ROOM_RECORD_SIZE)
                .filter(|o| o + ROOM_RECORD_SIZE <= d.len())
                .map(|o| room_row(d, o))
                .collect();
            ev.push(LinkEvent::Rooms(rows));
            let _ = s.link.send_empty(CHANNEL_USERS);
        }
        CHANNEL_USERS_REPLY => {
            let n = i32_at(d, 0xe).clamp(0, 512) as usize;
            let rows = (0..n)
                .map(|i| 0x12 + i * LOBBY_USER_RECORD_SIZE)
                .filter(|o| o + LOBBY_USER_RECORD_SIZE <= d.len())
                .map(|o| user_row(d, o))
                .collect();
            ev.push(LinkEvent::Users(rows));
        }
        LOBBY_USER_ADD_PUSH => ev.push(LinkEvent::UserAdded(user_row(d, 6))),
        LOBBY_USER_REMOVE_PUSH => ev.push(LinkEvent::UserRemoved(u32_at(d, 6))),
        LOBBY_ROOM_ADD_PUSH => ev.push(LinkEvent::RoomAdded(room_row(d, 6))),
        LOBBY_ROOM_REMOVE_PUSH => ev.push(LinkEvent::RoomRemoved(u32_at(d, 6))),
        ROOM_CREATE_REPLY => {
            if u32_at(d, 6) == 0 {
                s.room = None;
                ev.push(LinkEvent::CreateRefused(u16_at(d, 0xa)));
            } else {
                let (number, slot) = (u32_at(d, 0xe), u32_at(d, 0x12));
                if let Some(r) = s.room.as_mut() {
                    r.number = number;
                    r.slot = slot;
                }
                s.in_room = true;
                enter_requests(&mut s.link);
                ev.push(LinkEvent::RoomCreated { number, slot });
            }
        }
        ROOM_ENTER_REPLY => {
            if u32_at(d, 6) == 0 {
                s.room = None;
                ev.push(LinkEvent::EnterRefused(u16_at(d, 0xa)));
            } else {
                let slot = u32_at(d, 0xe);
                let number = s.room.as_ref().map_or(0, |r| r.number);
                if let Some(r) = s.room.as_mut() {
                    r.slot = slot;
                }
                s.in_room = true;
                enter_requests(&mut s.link);
                ev.push(LinkEvent::RoomEntered { number, slot });
            }
        }
        // CLI-1313: a refused leave (+6 ok 0) of a starting member keeps the room (the lobby refuses it during the
        // start countdown, LG-177)
        // DISASM: cshell+0x33400 (only a nonzero +6 clears the room and opens the lobby screen)
        // DC-DEFAULT: CLI-1313 (proposed) any other refused leave still goes back to the lobby as before (a room the
        // lobby no longer has for us must not hold the screen)
        ROOM_LEAVE_REPLY
            if d.len() >= 0xa
                && u32_at(d, 6) == 0
                && s.room
                    .as_ref()
                    .and_then(|r| r.members.get(&my_id))
                    .is_some_and(|m| m.state == 2) => {}
        ROOM_LEAVE_REPLY => {
            s.room = None;
            s.in_room = false;
            ev.push(LinkEvent::LeftRoom);
            // back on the lobby screen: the room list again
            let _ = s.link.send_empty(ROOM_LIST);
            let _ = send_u32(&mut s.link, LOCATION, 0);
        }
        ROOM_INFO_REPLY => {
            let v = room_view(d);
            if let Some(r) = s.room.as_mut() {
                r.host_id = v.host_id;
                r.host = v.host_id == my_id;
                r.map = v.map;
                r.mode = v.mode;
            }
            ev.push(LinkEvent::RoomInfo(v));
        }
        ROOM_USERS_REPLY => {
            let n = i32_at(d, 0xe).clamp(0, 32) as usize;
            let mut members = BTreeMap::new();
            let mut subs = Vec::new();
            for i in 0..n {
                let o = 0x12 + i * ROOM_USER_RECORD_SIZE;
                if o + ROOM_USER_RECORD_SIZE <= d.len() {
                    let (uid, m) = member_row(d, o);
                    subs.extend(member_sub(d, o, uid, m.state));
                    members.insert(uid, m);
                }
            }
            if let Some(r) = s.room.as_mut() {
                r.members = members.clone();
            }
            ev.push(LinkEvent::Members(members));
            ev.extend(subs);
        }
        ROOM_USER_ADD_PUSH => {
            let (uid, m) = member_row(d, 6);
            if let Some(r) = s.room.as_mut() {
                r.members.insert(uid, m.clone());
            }
            let sub = member_sub(d, 6, uid, m.state);
            ev.push(LinkEvent::MemberAdded(uid, m));
            ev.extend(sub);
        }
        ROOM_USER_REMOVE_PUSH => {
            let (uid, reason) = (u32_at(d, 6), u32_at(d, 0xa));
            if let Some(r) = s.room.as_mut() {
                r.members.remove(&uid);
            }
            if uid == my_id {
                s.room = None;
                s.in_room = false;
                // w1-result: removed from the room while the result screen is up: the parking ends
                if s.held {
                    s.release_held(ev);
                }
            }
            ev.push(LinkEvent::MemberRemoved { id: uid, reason });
        }
        ROOM_UPDATE_PUSH => {
            let (number, kind, value) = (u32_at(d, 6), u32_at(d, 0xa), u32_at(d, 0xe));
            if s.in_room {
                if let Some(r) = s.room.as_mut() {
                    match kind {
                        2 => {
                            r.host_id = value;
                            r.host = value == my_id;
                        }
                        3 => r.map = value as i32,
                        4 => r.mode = value as i32,
                        _ => {}
                    }
                }
                let text = if d.len() > 0x16 {
                    wstr_at(d, 0x16, 33)
                } else {
                    String::new()
                };
                ev.push(LinkEvent::RoomSetting { kind, value, text });
            } else {
                ev.push(LinkEvent::RoomRowChanged {
                    number,
                    kind,
                    value,
                });
            }
        }
        ROOM_INFO_STALE_PUSH => {
            let _ = s.link.send_empty(ROOM_INFO);
        }
        ROOM_TEAM_PUSH => {
            let (uid, team) = (u32_at(d, 6), i32_at(d, 0xa));
            if let Some(m) = s.room.as_mut().and_then(|r| r.members.get_mut(&uid)) {
                m.team = team;
            }
            ev.push(LinkEvent::MemberTeam { id: uid, team });
        }
        ROOM_USER_STATE_PUSH => {
            // SPEC: PROTOCOL_2009.md §3.7 (0x3042: +6 id, +0xa state, +0xe sub-state)
            let (uid, st, sub) = (u32_at(d, 6), i32_at(d, 0xa), i32_at(d, 0xe));
            if let Some(m) = s.room.as_mut().and_then(|r| r.members.get_mut(&uid)) {
                m.state = st;
            }
            ev.push(LinkEvent::MemberState {
                id: uid,
                state: st,
                sub,
            });
        }
        ROOM_USER_FLAG_REPLY => ev.push(LinkEvent::FlagReply {
            ok: u32_at(d, 6) != 0,
            reason: u16_at(d, 0xa),
            kind: i16_at(d, 0xc),
        }),
        TEAM_CHANGE_REPLY => ev.push(LinkEvent::TeamReply {
            ok: u32_at(d, 6) != 0,
            reason: u32_at(d, 0xa),
        }),
        ROOM_SETTING_CHANGE_REPLY => ev.push(LinkEvent::SettingReply {
            ok: u32_at(d, 6) != 0,
        }),
        START_COUNTDOWN_PUSH => ev.push(LinkEvent::Countdown(u32_at(d, 6))),
        CHAT_PUSH => ev.extend(chat_push_event(d)),
        MUTE_UNTIL_PUSH => {
            // SPEC: PROTOCOL_2009.md §6.4 (0x3053: +6 FILETIME chat muted until, local time)
            // EVIDENCE: cshell+0x361c0 -> cshell+0x1b1620 (both words to shell +0x448)
            ev.push(LinkEvent::MuteUntil(u64_at(d, 6)));
        }
        NOTICE_REPLY => {
            // SPEC: PROTOCOL_2009.md §3.3 (0x367: +0x12 wstr[513] text; +0x414 u32 more)
            // EVIDENCE: cshell+0x380b0 (a non-empty text -> OK box with title 0x371; more set -> 0x366 again)
            let text = wstr_at(d, 0x12, 513);
            if !text.is_empty() {
                ev.push(LinkEvent::Notice(text));
            }
            if u32_at(d, 0x414) != 0 {
                let _ = s.link.send_empty(NOTICE_FETCH);
            }
        }
        TICKER_NOTICE_PUSH => ev.extend(ticker_notice(d)),
        BROADCAST_PUSH => {
            ev.extend(broadcast(d));
            // EVIDENCE: cshell+0x38190 (type 1 with +0x16 equal to the own account number: box res 0x1904)
            if u32_at(d, 6) == 1 && u32_at(d, 0x16) != 0 && u32_at(d, 0x16) == s.profile.my_id {
                ev.push(LinkEvent::OwnMegaphone);
            }
        }
        // EVIDENCE: cshell+0x39280 (one box res 0x1786 per item number, the inventory unchanged)
        EXPIRY_NOTICE_PUSH => {
            let n = (u32_at(d, 6) as usize).min(100);
            let nos: Vec<u32> = (0..n)
                .map(|i| 0xa + 4 * i)
                .filter(|o| o + 4 <= d.len())
                .map(|o| u32_at(d, o))
                .collect();
            ev.push(LinkEvent::ExpiryBoxes(nos));
        }
        ROOM_INVITE_PUSH => {
            // SPEC: PROTOCOL_2009.md §3.7 (0x3023: +6 room number, +0xa wstr[17] host name, +0x2c wstr[10] password)
            // EVIDENCE: cshell+0x39110
            ev.push(LinkEvent::Invite {
                number: u32_at(d, 6),
                host: wstr_at(d, 0xa, 17),
                password: wstr_at(d, 0x2c, 10),
            });
        }
        // w3-shop: the shop's lists, the inventory and the replies of the storage and the inbox
        INVENTORY_REPLY => ev.extend(inventory_records(d).map(LinkEvent::Inventory)),
        EQUIPMENT_REPLY => {
            ev.extend(equipment_slots(d).map(LinkEvent::Equipment));
            ev.extend(function_slots(d).map(LinkEvent::FunctionSlots));
        }
        // function-unequip: the function-slot answers (+6 ok, +0xe unique id); an ok one is applied by the store
        // and the equipment and inventory are read again
        // DC-DEFAULT: CLI-1397 (proposed) an ok 0x369 / 0x36b also reads 0x323 / 0x321 again (2009 edits its own copy)
        // SPEC: PROTOCOL_2009.md §4.2 0x369 / 0x36b
        // EVIDENCE: cshell+0x36710 / 0x36740 (to cshell+0x14cea0 / 0x14d010 while the Shop / Storage screen 0x16 is
        // up)
        FUNCTION_EQUIP_REPLY | FUNCTION_UNEQUIP_REPLY => {
            let ok = u32_at(d, 6) != 0;
            if ok {
                let _ = reread_items(&mut s.link);
            }
            ev.push(LinkEvent::FunctionSlotDone {
                put: id == FUNCTION_EQUIP_REPLY,
                ok,
                uid: u32_at(d, 0xe),
            });
        }
        SHOP_VIEW_REPLY => ev.extend(product_ids(d).map(LinkEvent::ShopView)),
        LIST_REPLY_0 | LIST_REPLY_1 | LIST_REPLY_2 => {
            let list = match id {
                LIST_REPLY_0 => 0,
                LIST_REPLY_1 => 1,
                _ => 2,
            };
            ev.extend(product_ids(d).map(|ids| LinkEvent::Highlight { list, ids }));
        }
        PRICE_REPLY => ev.push(LinkEvent::Prices(price_pairs(d))),
        // SPEC: PROTOCOL_2009.md §4.2 (0x325 / 0x327: +6 ok; the 2009 client moves the item itself)
        // DC-DEFAULT: CLI-547 after an ok equip, unequip or purchase the menu reads equipment and
        // inventory again (0x322 / 0x320) instead of editing its own copy
        EQUIP_REPLY | UNEQUIP_REPLY => {
            let ok = u32_at(d, 6) != 0;
            if ok {
                let _ = reread_items(&mut s.link);
            }
            ev.push(LinkEvent::EquipDone {
                ok,
                unequip: id == UNEQUIP_REPLY,
            });
        }
        // SPEC: PROTOCOL_2009.md §4.2 (0x337 / 0x339: +6 ok)
        // EVIDENCE: cshell+0x34b10 / cshell+0x34b70 (an ok reply goes to the Modification screen while it is up,
        // which edits the client's own inventory; a failed one is dropped)
        ATTACH_REPLY | DETACH_REPLY => ev.push(LinkEvent::AttachDone {
            ok: u32_at(d, 6) != 0,
            detach: id == DETACH_REPLY,
        }),
        // SPEC: PROTOCOL_2009.md §4.2 (0x335: +6 ok)
        // EVIDENCE: cshell+0x32ee0 (an ok reply goes to the Shop / Storage screen (0x16) or the Modification screen
        // (0x14) while it is up; a failed one is dropped)
        ITEM_DISCARD_REPLY => ev.push(LinkEvent::Discarded {
            ok: u32_at(d, 6) != 0,
        }),
        SHOP_BUY_REPLY => {
            let r = buy_reply(d);
            if r.ok {
                s.profile.gp = r.gp;
                ev.push(LinkEvent::Profile(s.profile.clone()));
                let _ = reread_items(&mut s.link);
            }
            ev.push(LinkEvent::Bought(r));
        }
        INBOX_OPEN_REPLY => ev.push(LinkEvent::InboxOpened(open_reply(d))),
        INBOX_PAGE_REPLY => {
            if let Some((total, rows)) = inbox_page(d) {
                ev.push(LinkEvent::Inbox { total, rows });
            }
        }
        // SPEC: PROTOCOL_2009.md §4.3 (0x3014 marks the inbox stale)
        INBOX_STALE_PUSH => ev.push(LinkEvent::InboxStale),
        // name-change: the open use window reads the reply (cshell+0x33790 -> cshell+0x1db910)
        ITEM_USE_REPLY => ev.push(LinkEvent::ItemUsed(use_reply(d))),
        // use-items: the megaphone composer reads both (cshell+0x367b0 -> cshell+0x2a8990, cshell+0x36770 ->
        // cshell+0x2a5c90)
        MEGAPHONE_QUEUE_REPLY => ev.push(LinkEvent::MegaphoneQueued(queue_reply(d))),
        MEGAPHONE_SEND_REPLY => ev.push(LinkEvent::MegaphoneSent(send_reply(d))),
        // audit-function (CLI-1389): the expiry push; a set +0x1a2 asks for the equipment again
        // EVIDENCE: cshell+0x22cde0 (each unique id: box 0x1786 with the item's name, title 0x371, the record
        // removed (cshell+0x22ccb0); +0x1a2 != 0 -> the equipment request cshell+0x28adb0)
        EXPIRY_PUSH => {
            let (uids, refresh) = expiry_push(d);
            if refresh {
                let _ = reread_items(&mut s.link);
            }
            ev.push(LinkEvent::Expired { uids, refresh });
        }
        // SPEC: PROTOCOL_2009.md §4.3 0x347 (the rename window reads ok != 0 as "name free")
        // EVIDENCE: cshell+0x2426d0
        RECIPIENT_CHECK_REPLY => ev.push(LinkEvent::NameChecked(u32_at(d, 6) != 0)),
        // use-items-emblem: the open clan mark window reads it (cshell+0x33940 -> cshell+0x1db9b0, its vfunc +0x48)
        // SPEC: PROTOCOL_2009.md §5.1 0x34d (+6 ok, +0xa reason)
        CLAN_MARK_CHECK_REPLY => ev.push(LinkEvent::MarkChecked {
            ok: u32_at(d, 6) != 0,
            reason: u32_at(d, 0xa),
        }),
        // w3-clan: a character info reply after the login (a clan join or leave sends it again)
        // SPEC: PROTOCOL_2009.md §3.3 table 3.3a; §5.2 (create / join / leave push 0x303 to the user)
        CHARACTER_INFO_REPLY => {
            s.profile.clan_id = u32_at(d, 0x86);
            ev.push(LinkEvent::CharInfo(char_extras(d)));
        }
        // the clan login and notice-set answers and the info push: the client asks for the clan record itself
        // SPEC: PROTOCOL_2009.md §5.1 (0x3c1 / 0x3c9 non-zero -> 0x3c2; 0x3061 -> 0x3c2 always)
        // EVIDENCE: cshell+0x356f0 (0x3c1), cshell+0x358e0 (0x3c9), cshell+0x42df0 (0x3061 inline -> cshell+0x18d630)
        CLAN_LOGIN_REPLY | CLAN_NOTICE_SET_REPLY if u32_at(d, 6) != 0 => {
            let _ = s.link.send_empty(CLAN_INFO);
        }
        CLAN_INFO_STALE_PUSH => {
            let _ = s.link.send_empty(CLAN_INFO);
        }
        CLAN_INFO_REPLY => {
            if let Some(v) = clan_info_view(d) {
                ev.push(LinkEvent::ClanInfo(v));
            }
        }
        CLAN_MEMBERS_REPLY => {
            if let Some(v) = clan_member_rows(d) {
                ev.push(LinkEvent::ClanMembers(v));
            }
        }
        CLAN_NOTICE_REPLY => {
            if let Some(t) = clan_notice_text(d) {
                ev.push(LinkEvent::ClanNotice(t));
            }
        }
        // EVIDENCE: cshell+0x35980 (0x3062), cshell+0x35940 (0x3063): the clan screen asks again, elsewhere a flag
        CLAN_MEMBERS_STALE_PUSH | CLAN_NOTICE_STALE_PUSH => ev.push(LinkEvent::ClanStale(id)),
        CLAN_REMOVED_PUSH => ev.push(LinkEvent::ClanRemoved),
        // gm-client: the operator replies and the room kick's answer
        // SPEC: PROTOCOL_2009.md §6.4 (0x3d1 / 0x3d5: +6 u32 result, +0xa u16 reason; 0x3d3 table), §3.7 (0x38f)
        OP_RESTRICT_REPLY => ev.push(LinkEvent::OpRestrictReply {
            ok: u32_at(d, 6) == 1,
            reason: u16_at(d, 0xa),
        }),
        OP_LOOKUP_REPLY => ev.push(LinkEvent::OpLookupReply(super::gm::lookup_view(d))),
        OP_MUTE_REPLY => ev.push(LinkEvent::OpMuteReply {
            ok: u32_at(d, 6) != 0,
            reason: u16_at(d, 0xa),
        }),
        ROOM_KICK_REPLY => ev.push(LinkEvent::KickReply {
            ok: u32_at(d, 6) != 0,
            reason: u32_at(d, 0xa),
        }),
        DC_MESSENGER_PUSH => ev.push(LinkEvent::Messenger(messenger_fields(d))),
        // LG-274: our push after the room info and on every change (the lobby sends none without a Hacks choice)
        DC_ROOM_HACKS_PUSH => {
            if let Some((offered, value)) = roomhacks::parse_push(d) {
                ev.push(LinkEvent::RoomHacks { offered, value });
            }
        }
        // LG-305: our pushes (a lobby entry without the feature sends none)
        dc_wire::roomhardcore::PUSH => {
            if let Some(on) = dc_wire::roomhardcore::parse(d) {
                ev.push(LinkEvent::RoomHardcore(on));
            }
        }
        dc_wire::possession::PUSH => {
            if let Some(on) = dc_wire::possession::parse(d) {
                ev.push(LinkEvent::RoomPossession(on));
            }
        }
        dc_wire::gungame::OFFER => ev.push(LinkEvent::GunGameOffer),
        dc_wire::gungame::ROOM => {
            if let Some((room, on, ladder)) = dc_wire::gungame::parse_room(d) {
                ev.push(LinkEvent::GunGameRoom { room, on, ladder });
            }
        }
        // DC-DEFAULT: LG-327 (proposed, project decision) comes right before 0x3043; a later push replaces it
        dc_wire::seatticket::PUSH => s.seat_ticket = dc_wire::seatticket::parse_push(d),
        MATCH_ADDRESS_PUSH => {
            // SPEC: PROTOCOL_2009.md §7 (0x3043: +6 wstr[17] IPv4 text, +0x28 u32 UDP port)
            let ip = wstr_at(d, 6, 17);
            let port = u32_at(d, 0x28) as u16;
            if let Ok(ip) = ip.trim().parse::<Ipv4Addr>() {
                return Some(SocketAddr::new(IpAddr::V4(ip), port));
            }
        }
        _ => {}
    }
    None
}

/// The balance replies and grade pushes kept while the connection belonged to the game session, as events (the
/// last of each kind; neither is parked by the 2009 client).
// EVIDENCE: cshell+0x337b0 (0x345), cshell+0x36690 (0x3003): both store their values whatever screen is up
fn kept_pushes(link: &mut TcpLink) -> Vec<LinkEvent> {
    let frames = link.take_ids(&[BALANCE_REPLY, GRADE_PUSH]);
    let last = |id: u16| frames.iter().rev().find(|(i, _)| *i == id).map(|(_, d)| d);
    let mut out = Vec::new();
    if let Some(d) = last(GRADE_PUSH) {
        out.push(grade_of(d));
    }
    if let Some(e) = last(BALANCE_REPLY).and_then(|d| balance_of(d)) {
        out.push(e);
    }
    out
}

/// The waiting-room requests after entering a room: location 1, room info, room users, present.
// SPEC: PROTOCOL_2009.md §3.7 (waiting 9: 0x306(1); 0x388; 0x38a; 0x392(1, 0))
fn enter_requests(link: &mut TcpLink) {
    let _ = send_u32(link, LOCATION, 1);
    let _ = link.send_empty(ROOM_INFO);
    let _ = link.send_empty(ROOM_USERS);
    let _ = send_flag(link, 1, 0);
}

/// How long the screens wait for a reply before they show the time-out box.
// DC-DEFAULT: CLI-163 (proposed) menu reply wait 10 s
pub const REPLY_WAIT: Duration = Duration::from_secs(10);

/// A deadline helper for the screens' waits.
#[derive(Debug, Clone, Copy, Default)]
pub struct Wait(Option<Instant>);

impl Wait {
    pub fn start(&mut self) {
        self.0 = Some(Instant::now());
    }
    pub fn stop(&mut self) {
        self.0 = None;
    }
    pub fn busy(&self) -> bool {
        self.0.is_some()
    }
    pub fn expired(&self) -> bool {
        self.0.is_some_and(|t| t.elapsed() >= REPLY_WAIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_packet_has_the_0x312_layout() {
        let p = report_packet("Bot1", "wall hack", 0, 3).into_payload();
        assert_eq!(p.len(), 0xe4c);
        assert_eq!(u16::from_le_bytes([p[0], p[1]]), 0x312);
        assert_eq!(u32::from_le_bytes(p[2..6].try_into().unwrap()), 0xe4c);
        assert_eq!(
            u32::from_le_bytes(p[6..10].try_into().unwrap()),
            0,
            "kind 0"
        );
        let wide = |at: usize, n: usize| -> String {
            let u: Vec<u16> = p[at..at + 2 * n]
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&c| c != 0)
                .collect();
            String::from_utf16_lossy(&u)
        };
        assert_eq!(u32::from_le_bytes(p[10..14].try_into().unwrap()), 0, "kind");
        assert_eq!(
            u32::from_le_bytes(p[14..18].try_into().unwrap()),
            3,
            "detail"
        );
        assert_eq!(wide(0x12, 17), "Bot1");
        assert_eq!(wide(0x34, 601), "wall hack");
    }

    #[test]
    fn records_read_at_the_spec_offsets() {
        let mut d = vec![0u8; 0x12 + ROOM_RECORD_SIZE];
        d[0x12..0x16].copy_from_slice(&7u32.to_le_bytes());
        d[0x12 + 0x52..0x12 + 0x56].copy_from_slice(&2u32.to_le_bytes());
        d[0x12 + 0x5a..0x12 + 0x5e].copy_from_slice(&3u32.to_le_bytes());
        d[0x12 + 0x5e..0x12 + 0x62].copy_from_slice(&8u32.to_le_bytes());
        let r = room_row(&d, 0x12);
        assert_eq!((r.number, r.map, r.players, r.max_players), (7, 2, 3, 8));
        let mut u = vec![0u8; 6 + LOBBY_USER_RECORD_SIZE];
        u[6 + 4..6 + 8].copy_from_slice(&42u32.to_le_bytes());
        assert_eq!(user_row(&u, 6).id, 42);
    }

    /// A frame of `len` bytes with u32 fields at their offsets.
    fn frame(len: usize, fields: &[(usize, u32)]) -> Vec<u8> {
        let mut d = vec![0u8; len];
        for (o, v) in fields {
            d[*o..*o + 4].copy_from_slice(&v.to_le_bytes());
        }
        d
    }

    /// A worker with a lobby session on a loopback socket (the accepted end is returned so the frames stay
    /// readable), its result screen up and the match's progress push parked.
    fn held_worker(parked: Option<Progress>) -> (Worker, Receiver<LinkEvent>, std::net::TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let rec = Recorder::null("test");
        let link = TcpLink::connect(addr, "lobby", &rec, Duration::from_secs(5)).unwrap();
        let (peer, _) = listener.accept().unwrap();
        let (tx, rx) = mpsc::channel();
        let mut w = Worker::new(LinkOptions::new("test"), tx);
        w.session = Some(Session {
            link,
            ticket: AuthTicket {
                session_key: [0; 16],
                account_no: 1,
                lobby: addr,
                server_id: 1,
                server_name: String::new(),
                created_character: false,
            },
            profile: LobbyProfile {
                exp: 100,
                gp: 10_000,
                ..LobbyProfile::default()
            },
            room: Some(RoomSeat::default()),
            channels: Vec::new(),
            entering: None,
            in_room: true,
            held: true,
            parked,
            seat_ticket: None,
        });
        (w, rx, peer)
    }

    fn progress_events(rx: &Receiver<LinkEvent>) -> Vec<ProgressApplied> {
        rx.try_iter()
            .filter_map(|e| match e {
                LinkEvent::Progress(p) => Some(p),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn leaving_the_room_from_the_result_screen_applies_the_parked_progress() {
        // w1-result (M2): command 0x3b on the result screen sent only 0x386; the parked 0x3004 was lost and every
        // later push stayed parked
        let (mut w, rx, _peer) = held_worker(Some((500, 10_500, true)));
        w.command(LinkCmd::QuitRoom);
        let p = progress_events(&rx);
        assert_eq!(
            p,
            [ProgressApplied {
                exp_before: 100,
                exp: 500,
                gp: 10_500,
                popup: true
            }]
        );
        let s = w.session.as_mut().unwrap();
        assert!(!s.held && s.parked.is_none());
        assert_eq!((s.profile.exp, s.profile.gp), (500, 10_500));
        // a later push takes effect at once
        let mut ev = Vec::new();
        let d = frame(0x12, &[(6, 700), (0xa, 10_700)]);
        assert!(dispatch(s, PROGRESS_PUSH, &d, &mut ev).is_none());
        assert_eq!((s.profile.exp, s.profile.gp), (700, 10_700));
        assert!(matches!(
            ev.last(),
            Some(LinkEvent::Progress(ProgressApplied {
                exp_before: 500,
                exp: 700,
                popup: false,
                ..
            }))
        ));
    }

    #[test]
    fn the_result_screen_parks_pushes_until_its_exit_and_removal_ends_the_parking() {
        let (mut w, rx, _peer) = held_worker(None);
        {
            let s = w.session.as_mut().unwrap();
            let mut ev = Vec::new();
            dispatch(
                s,
                PROGRESS_PUSH,
                &frame(0x12, &[(6, 300), (0xa, 10_300)]),
                &mut ev,
            );
            assert!(ev.is_empty(), "parked while the result screen is up");
            assert_eq!(s.parked, Some((300, 10_300, false)));
        }
        w.command(LinkCmd::BackToRoom);
        assert_eq!(progress_events(&rx).len(), 1);
        // removed from the room while the screen is up: the parking ends as on a leave
        let (mut w, _rx, _peer) = held_worker(Some((400, 10_400, false)));
        let s = w.session.as_mut().unwrap();
        let mut ev = Vec::new();
        dispatch(
            s,
            ROOM_USER_REMOVE_PUSH,
            &frame(0xe, &[(6, 0), (0xa, 1)]),
            &mut ev,
        );
        assert!(!s.held);
        assert_eq!(s.profile.exp, 400);
        // a parked push with a zero total is dropped (cshell+0x164020 applies only non-zero values)
        let (mut w, rx, _peer) = held_worker(Some((0, 10_400, false)));
        w.command(LinkCmd::BackToRoom);
        assert!(progress_events(&rx).is_empty());
        assert_eq!(w.session.as_ref().unwrap().profile.exp, 100);
    }

    #[test]
    fn balance_grade_and_character_fields_read_at_the_spec_offsets() {
        let b = frame(0x16, &[(6, 1), (0xe, 1200), (0x12, 3400)]);
        assert!(matches!(
            balance_of(&b),
            Some(LinkEvent::Balance {
                credit: 1200,
                prepaid: 3400
            })
        ));
        assert!(balance_of(&frame(0x16, &[(0xe, 5)])).is_none(), "ok 0");
        assert!(matches!(
            grade_of(&frame(0xa, &[(6, 3)])),
            LinkEvent::Grade(3)
        ));
        let mut c = frame(
            0x108,
            &[
                (0x86, 77),
                (0xac, 5),
                (0xd4, 9),
                (0xd8, 4),
                (0xf8, 2),
                (0x100, 12),
                (0xec, 0x89ab_cdef),
                (0xf0, 0x0123_4567),
                (0x104, 1),
            ],
        );
        for (i, ch) in "Crew".encode_utf16().enumerate() {
            c[0x8a + 2 * i..0x8a + 2 * i + 2].copy_from_slice(&ch.to_le_bytes());
        }
        assert_eq!(
            char_extras(&c),
            CharExtras {
                grade: 2,
                clan_id: 77,
                clan_label: "Crew".into(),
                clan_role: 5,
                clan_mark: 9,
                clan_mark_ground: 4,
                nation: 12,
                muted_until: 0x0123_4567_89ab_cdef,
                notice: true,
                inbox_new: false,
                room_badge: false,
                ..Default::default()
            }
        );
        // the session reads both pushes in the steady state
        let (mut w, _rx, _peer) = held_worker(None);
        let s = w.session.as_mut().unwrap();
        let mut ev = Vec::new();
        dispatch(s, BALANCE_REPLY, &b, &mut ev);
        dispatch(s, GRADE_PUSH, &frame(0xa, &[(6, 4)]), &mut ev);
        assert!(matches!(
            ev[..],
            [LinkEvent::Balance { .. }, LinkEvent::Grade(4)]
        ));
    }

    /// The frames that reached `peer` within a short wait (id, payload with its inner header).
    fn peer_frames(peer: &mut std::net::TcpStream) -> Vec<(u16, Vec<u8>)> {
        use std::io::Read;
        peer.set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        while let Ok(n) = peer.read(&mut tmp) {
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        parse_tcp_frames(&buf)
            .unwrap()
            .0
            .into_iter()
            .map(|(id, d)| (id as u16, d))
            .collect()
    }

    fn payload(w: &PktWriter) -> Vec<u8> {
        w.to_frame()[8..].to_vec()
    }

    #[test]
    fn chat_goes_out_as_a_0x3a0_of_0xf8_bytes_with_the_typed_text() {
        // w2-lobbychat (M6): LinkCmd::Chat existed but nothing built it; its frame is the 2009 one
        let (mut w, _rx, mut peer) = held_worker(None);
        w.command(LinkCmd::Chat("hello there".into()));
        let f = peer_frames(&mut peer);
        let (_, d) = f
            .iter()
            .find(|(id, _)| *id == CHAT_SEND)
            .expect("0x3a0 sent");
        assert_eq!(
            d.len(),
            0xf8,
            "SPEC: PROTOCOL_2009.md §3.6 0x3a0 is 0xf8 bytes"
        );
        assert_eq!(wstr_at(d, 6, 121), "hello there");
        // the away flag (m62) is 0x392 kind 1 value 2, present again value 0
        w.command(LinkCmd::Away(true));
        w.command(LinkCmd::Away(false));
        let f = peer_frames(&mut peer);
        let flags: Vec<(u32, u32)> = f
            .iter()
            .filter(|(id, _)| *id == ROOM_USER_FLAG)
            .map(|(_, d)| (u32_at(d, 6), u32_at(d, 0xa)))
            .collect();
        assert_eq!(flags, [(1, 2), (1, 0)]);
    }

    // CLI-1560 / CLI-1561: the shopping flag is 0x392 kind 1 value 1 / 0; the private state 0x30a / 0x30c goes out
    // only in a room (cshell+0x14f361 / cshell+0x14f78c test shell +0x18 first)
    #[test]
    fn shopping_flag_and_private_state_frames() {
        let (mut w, _rx, mut peer) = held_worker(None);
        w.session.as_mut().unwrap().in_room = false;
        w.command(LinkCmd::Location(2));
        w.command(LinkCmd::PrivateState(1));
        w.command(LinkCmd::PrivateStateEnd);
        let f = peer_frames(&mut peer);
        let ids: Vec<u16> = f.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [LOCATION], "out of a room only the location");
        assert_eq!(u32_at(&f[0].1, 6), 2);
        w.session.as_mut().unwrap().in_room = true;
        w.command(LinkCmd::Shopping(true));
        w.command(LinkCmd::PrivateState(1));
        w.command(LinkCmd::PrivateStateEnd);
        w.command(LinkCmd::Shopping(false));
        let f = peer_frames(&mut peer);
        let got: Vec<(u16, Vec<u32>)> = f
            .iter()
            .map(|(id, d)| {
                let words = (6..d.len()).step_by(4).map(|o| u32_at(d, o)).collect();
                (*id, words)
            })
            .collect();
        assert_eq!(
            got,
            [
                (ROOM_USER_FLAG, vec![1, 1]),
                (PRIVATE_STATE, vec![1]),
                (PRIVATE_STATE_END, vec![]),
                (ROOM_USER_FLAG, vec![1, 0]),
            ]
        );
    }

    // CLI-1391 / CLI-1392: the plain expiry notice lists item numbers; an own megaphone line asks for box 0x1904
    #[test]
    fn expiry_notice_and_own_megaphone_events() {
        let (mut w, _rx, _peer) = held_worker(None);
        let s = w.session.as_mut().unwrap();
        s.profile.my_id = 5;
        let mut ev = Vec::new();
        let mut n = PktWriter::new(EXPIRY_NOTICE_PUSH);
        n.u32(2).u32(9019).u32(9020);
        dispatch(s, EXPIRY_NOTICE_PUSH, &payload(&n), &mut ev);
        assert!(matches!(&ev[0], LinkEvent::ExpiryBoxes(v) if v == &[9019, 9020]));
        for (sender, own) in [(5u32, true), (6, false)] {
            let mut ev = Vec::new();
            let mut b = PktWriter::new(BROADCAST_PUSH);
            b.u32(1).u32(7).u32(1).u32(1).u32(sender);
            b.at(0x26).wstr("Bob", 17).wstr("sale", 100);
            dispatch(s, BROADCAST_PUSH, &payload(&b), &mut ev);
            assert_eq!(ev.iter().any(|e| matches!(e, LinkEvent::OwnMegaphone)), own);
        }
    }

    // use-items: the country window's nation at +0x44; the megaphone send and both megaphone answers
    #[test]
    fn country_and_megaphone_requests_and_answers_follow_the_spec() {
        let p = payload(&item_use_packet(9, "", 0, 23));
        assert_eq!((p.len(), u32_at(&p, 6), u32_at(&p, 0x44)), (0x48, 9, 23));
        // use-items-sweep: the colour window's colour at +0x38 and the replaced item at +0x3c
        let p = payload(&colour_use_packet(9, 5, 31));
        assert_eq!(
            (
                p.len(),
                u32_at(&p, 6),
                u32_at(&p, 0x38),
                u32_at(&p, 0x3c),
                u32_at(&p, 0x44)
            ),
            (0x48, 9, 5, 31, 0)
        );
        // use-items-emblem: the mark window's pair at +0x2c / +0x30, the face window's face at +0x34, the check 0x34c
        let p = payload(&mark_use_packet(9, 12, 34));
        assert_eq!(
            (
                p.len(),
                u32_at(&p, 6),
                u32_at(&p, 0x2c),
                u32_at(&p, 0x30),
                u32_at(&p, 0x34)
            ),
            (0x48, 9, 12, 34, 0)
        );
        let p = payload(&face_use_packet(9, 6014));
        assert_eq!(
            (
                p.len(),
                u32_at(&p, 0x2c),
                u32_at(&p, 0x34),
                u32_at(&p, 0x38)
            ),
            (0x48, 0, 6014, 0)
        );
        let p = payload(&mark_check_packet(12, 34));
        assert_eq!((p.len(), u32_at(&p, 6), u32_at(&p, 0xa)), (0xe, 12, 34));
        let long: String = "x".repeat(80);
        let p = payload(&megaphone_packet(7, &long));
        assert_eq!((p.len(), u32_at(&p, 6), u32_at(&p, 0xa)), (0x26a, 7, 1));
        assert_eq!(wstr_at(&p, 0xe, 101).chars().count(), MEGAPHONE_MAX);
        let p = payload(&megaphone_packet(7, "hello"));
        assert_eq!(wstr_at(&p, 0xe, 101), "hello");
        let (mut w, _rx, _peer) = held_worker(None);
        let s = w.session.as_mut().unwrap();
        let mut ev = Vec::new();
        let mut q = PktWriter::new(MEGAPHONE_QUEUE_REPLY);
        q.u32(1).u32(0).u32(30).u32(4).u32(6).u32(0).u32(2);
        dispatch(s, MEGAPHONE_QUEUE_REPLY, &payload(&q), &mut ev);
        let mut r = PktWriter::new(MEGAPHONE_SEND_REPLY);
        r.u32(1).u32(0).u32(7).u32(2);
        dispatch(s, MEGAPHONE_SEND_REPLY, &payload(&r), &mut ev);
        let q = QueueReply {
            ok: true,
            reason: 0,
            secs: 30,
            on_air: 4,
            last: 6,
            waiting: 2,
        };
        assert!(matches!(&ev[0], LinkEvent::MegaphoneQueued(r) if *r == q));
        let sent = SendReply {
            ok: true,
            reason: 0,
            uid: 7,
            left: 2,
        };
        assert!(matches!(&ev[1], LinkEvent::MegaphoneSent(r) if *r == sent));
    }

    #[test]
    fn chat_mute_notice_ticker_broadcast_and_invite_pushes_are_read_at_the_spec_offsets() {
        let (mut w, _rx, mut peer) = held_worker(None);
        let s = w.session.as_mut().unwrap();
        let mut ev = Vec::new();
        // 0x3051: name length, colour id, name, text length, text
        let mut c = PktWriter::new(CHAT_PUSH);
        c.u32(9)
            .u32(4)
            .u32(3)
            .wstr("Anna", 17)
            .u32(5)
            .wstr("hi yo", 6);
        dispatch(s, CHAT_PUSH, &payload(&c), &mut ev);
        // 0x3053: FILETIME
        let mut m = PktWriter::new(MUTE_UNTIL_PUSH);
        m.u32(0x89ab_cdef).u32(0x0123_4567);
        dispatch(s, MUTE_UNTIL_PUSH, &payload(&m), &mut ev);
        // 0x367 with the more flag: the client asks again
        let mut n = PktWriter::new(NOTICE_REPLY);
        n.u32(1).u32(0).u32(0).wstr("Server notice", 513).u32(1);
        let nd = payload(&n);
        assert_eq!(nd.len(), 0x418);
        dispatch(s, NOTICE_REPLY, &nd, &mut ev);
        // 0x3052: period, count, length, text
        let mut t = PktWriter::new(TICKER_NOTICE_PUSH);
        t.u32(12).u32(3).u32(4).wstr("News", 5);
        dispatch(s, TICKER_NOTICE_PUSH, &payload(&t), &mut ev);
        // 0x3065 type 1: seconds, order, serial, account, (2 words), colour, name, message
        let mut b = PktWriter::new(BROADCAST_PUSH);
        b.u32(1).u32(7).u32(1).u32(1).u32(5).u32(0).u32(0).u32(0);
        b.wstr("Bob", 17).wstr("sale", 100);
        let bd = payload(&b);
        assert_eq!(
            (u32_at(&bd, 0xa), wstr_at(&bd, 0x26, 17)),
            (7, "Bob".into())
        );
        dispatch(s, BROADCAST_PUSH, &bd, &mut ev);
        // 0x3023: room number, host, password
        let mut i = PktWriter::new(ROOM_INVITE_PUSH);
        i.u32(41).wstr("Host", 17).wstr("pw", 10);
        dispatch(s, ROOM_INVITE_PUSH, &payload(&i), &mut ev);
        // 0x3042 with a sub-state
        dispatch(
            s,
            ROOM_USER_STATE_PUSH,
            &frame(0x12, &[(6, 5), (0xa, 0), (0xe, 2)]),
            &mut ev,
        );
        let mut it = ev.into_iter();
        assert!(
            matches!(it.next(), Some(LinkEvent::Chat { name, text, colour: 3 })
            if name == "Anna" && text == "hi yo")
        );
        assert!(matches!(
            it.next(),
            Some(LinkEvent::MuteUntil(0x0123_4567_89ab_cdef))
        ));
        assert!(matches!(it.next(), Some(LinkEvent::Notice(t)) if t == "Server notice"));
        assert!(
            matches!(it.next(), Some(LinkEvent::Ticker { text, show_ms: 12_000, repeat: 3 })
            if text == "News")
        );
        assert!(
            matches!(it.next(), Some(LinkEvent::Ticker { text, show_ms: 7_000, repeat: 1 })
            if text == "Bob : sale")
        );
        assert!(
            matches!(it.next(), Some(LinkEvent::Invite { number: 41, host, password })
            if host == "Host" && password == "pw")
        );
        assert!(matches!(
            it.next(),
            Some(LinkEvent::MemberState {
                id: 5,
                state: 0,
                sub: 2
            })
        ));
        assert!(it.next().is_none());
        let again = peer_frames(&mut peer);
        assert!(
            again.iter().any(|(id, _)| *id == NOTICE_FETCH),
            "more -> 0x366"
        );
    }

    #[test]
    fn member_records_with_a_sub_state_give_a_member_state_event() {
        let (mut w, _rx, _peer) = held_worker(None);
        let s = w.session.as_mut().unwrap();
        let mut d = vec![0u8; 0x12 + 2 * ROOM_USER_RECORD_SIZE];
        d[0xe..0x12].copy_from_slice(&2u32.to_le_bytes());
        for (k, (id, sub)) in [(11u32, 0i32), (12, 1)].iter().enumerate() {
            let o = 0x12 + k * ROOM_USER_RECORD_SIZE;
            d[o + 4..o + 8].copy_from_slice(&id.to_le_bytes());
            d[o + 0x3e..o + 0x42].copy_from_slice(&sub.to_le_bytes());
        }
        let mut ev = Vec::new();
        dispatch(s, ROOM_USERS_REPLY, &d, &mut ev);
        assert!(matches!(ev[0], LinkEvent::Members(ref m) if m.len() == 2));
        assert!(matches!(
            ev[1..],
            [LinkEvent::MemberState {
                id: 12,
                state: 0,
                sub: 1
            }]
        ));
    }

    #[test]
    fn wait_expires_only_when_started() {
        let mut w = Wait::default();
        assert!(!w.busy() && !w.expired());
        w.start();
        assert!(w.busy() && !w.expired());
        w.stop();
        assert!(!w.busy());
    }

    // ---------------------------------------------------------------------------------------- CLI-1610 identity

    /// Accepts on `l` until a plain connection comes (a TLS client hello is dropped: the client then falls back to
    /// plain on loopback).
    fn accept_plain(l: &std::net::TcpListener) -> (std::net::TcpStream, Vec<u8>) {
        use std::io::Read;
        loop {
            let (mut c, _) = l.accept().unwrap();
            let _ = c.set_read_timeout(Some(Duration::from_secs(5)));
            let mut first = [0u8; 4096];
            let n = c.read(&mut first).unwrap_or(0);
            if n == 0 || first[0] == 0x16 {
                continue;
            }
            return (c, first[..n].to_vec());
        }
    }

    /// A fake authority: answers one `POST /identity/token` with `status` and `body`; sends the request text.
    fn fake_authority(status: u16, body: &'static str) -> (SocketAddr, Receiver<String>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut c, mut buf) = accept_plain(&l);
            let mut tmp = [0u8; 4096];
            loop {
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|h| h.strip_prefix("Content-Length: "))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if rest.len() >= len {
                        break;
                    }
                }
                match c.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&buf).to_string());
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = c.write_all(reply.as_bytes());
        });
        (addr, rx)
    }

    /// A fake auth port: file check yes, then the login request's (name, password) fields are sent and the login is
    /// refused with `reason` (None: the connection is closed instead).
    fn fake_auth(reason: Option<u32>) -> (SocketAddr, Receiver<(String, String)>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut c, mut buf) = accept_plain(&l);
            let mut tmp = [0u8; 65536];
            let mut checked = false;
            loop {
                let (frames, used) = parse_tcp_frames(&buf).unwrap();
                buf.drain(..used);
                for (id, d) in frames {
                    if id as u16 == FILE_CHECK && !checked {
                        checked = true;
                        let mut w = PktWriter::new(FILE_CHECK_REPLY);
                        w.u32(1);
                        c.write_all(&w.to_frame()).unwrap();
                    } else if id as u16 == LOGIN_REQUEST {
                        let _ = tx.send((wstr_at(&d, 6, 4096), wstr_at(&d, 0x2006, 17)));
                        if let Some(r) = reason {
                            let mut w = PktWriter::new(LOGIN_REPLY);
                            w.u32(0).u32(r).pad(0x40);
                            c.write_all(&w.to_frame()).unwrap();
                            std::thread::sleep(Duration::from_millis(200));
                        }
                        return;
                    }
                }
                match c.read(&mut tmp) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
        });
        (addr, rx)
    }

    fn identity_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc_identity_link_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn identity_worker(
        authority: SocketAddr,
        dir: &std::path::Path,
    ) -> (Worker, Receiver<LinkEvent>) {
        let (tx, rx) = mpsc::channel();
        let mut o = LinkOptions::new("identity");
        o.timeout = Duration::from_secs(5);
        o.identity = Some(crate::identity::Authority {
            addr: authority,
            key: authority.to_string(),
            trust: dc_botclient::tls::Trust::default(),
            derived: false,
            cache: Some(dir.join(crate::identity::CACHE_FILE)),
        });
        (Worker::new(o, tx), rx)
    }

    fn refusal(rx: &Receiver<LinkEvent>) -> Option<u32> {
        rx.try_iter().find_map(|e| match e {
            LinkEvent::LoginRefused { reason, .. } => Some(reason),
            _ => None,
        })
    }

    /// LG-271: after a vanilla mod step (no web port answers: the empty set) the login name carries the empty-set
    /// hash and content `0`; a login without a mod step carries nothing.
    #[test]
    fn the_login_carries_the_mod_set_and_content_hashes() {
        let (auth, login) = fake_auth(Some(crate::identity::REASON_WRONG_PASSWORD));
        let (tx, _rx) = mpsc::channel();
        let mut o = LinkOptions::new("modhash");
        o.timeout = Duration::from_secs(5);
        let mut w = Worker::new(o, tx);
        w.command(LinkCmd::Login {
            auth,
            user: "robin".into(),
            password: "pw".into(),
            nick: "robin".into(),
        });
        let (name, _) = login.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            name,
            format!("LOCAL:robin;m={};c=0", crate::mods::EMPTY_SET_HASH)
        );
        assert_eq!(w.mod_set_hash.as_deref(), Some(crate::mods::EMPTY_SET_HASH));
        // a modded set's hashes go out as computed
        w.mod_set_hash = Some("ab".repeat(32));
        w.mod_content_hash = Some("cd".repeat(32));
        assert_eq!(
            w.login_suffix(),
            format!(";m={};c={}", "ab".repeat(32), "cd".repeat(32))
        );
        w.mod_set_hash = None;
        assert_eq!(w.login_suffix(), "", "no hash: no suffix");
    }

    #[test]
    fn a_password_login_becomes_a_token_login_at_the_lobby() {
        let (authority, req) = fake_authority(
            200,
            r#"{"token":"cGF5bG9hZA.c2ln","expires":4102444800,"account":7,"nick":"Robin"}"#,
        );
        let (auth, login) = fake_auth(None);
        let dir = identity_dir("token");
        let (mut w, rx) = identity_worker(authority, &dir);
        let cmd = LinkCmd::Login {
            auth,
            user: "robin".into(),
            password: "secret1".into(),
            nick: "robin".into(),
        };
        assert!(!format!("{cmd:?}").contains("secret1"));
        w.command(cmd);
        let text = req.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(text.starts_with("POST /identity/token HTTP/1.1"), "{text}");
        assert!(
            text.ends_with(r#"{"login":"robin","password":"secret1"}"#),
            "{text}"
        );
        let (name, password) = login.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            name,
            format!(
                "LOCAL:tok:cGF5bG9hZA.c2ln;m={};c=0",
                crate::mods::EMPTY_SET_HASH
            )
        );
        assert_eq!(password, "", "the lobby never gets the password");
        // the connection closed after the login request: a connection failure, the token stays cached
        assert_eq!(refusal(&rx), Some(0));
        let c = crate::identity::load(&dir.join(crate::identity::CACHE_FILE), 0).unwrap();
        assert_eq!(
            (c.login.as_str(), c.account, c.nick.as_str()),
            ("robin", 7, "Robin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cached_token_logs_in_without_the_password_and_a_refusal_drops_it() {
        let dir = identity_dir("cached");
        let path = dir.join(crate::identity::CACHE_FILE);
        // no authority runs: the cached token must not need it
        let authority: SocketAddr = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        crate::identity::store(
            &path,
            &crate::identity::Cached {
                token: "aGk.c2ln".into(),
                expires: crate::identity::now() + 3600,
                account: 3,
                nick: String::new(),
                login: "Kim".into(),
                authority: authority.to_string(),
            },
        )
        .unwrap();
        let (auth, login) = fake_auth(Some(crate::identity::REASON_WRONG_PASSWORD));
        let (mut w, rx) = identity_worker(authority, &dir);
        w.command(LinkCmd::Login {
            auth,
            user: "kim".into(),
            password: String::new(),
            nick: "kim".into(),
        });
        let (name, password) = login.recv_timeout(Duration::from_secs(5)).unwrap();
        let want = format!("LOCAL:tok:aGk.c2ln;m={};c=0", crate::mods::EMPTY_SET_HASH);
        assert_eq!((name.as_str(), password.as_str()), (want.as_str(), ""));
        assert_eq!(refusal(&rx), Some(crate::identity::REASON_WRONG_PASSWORD));
        assert!(!path.exists(), "a refused token is dropped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrong_password_at_the_authority_is_the_wrong_password_outcome() {
        let (authority, _req) = fake_authority(401, r#"{"error":"wrong login or password"}"#);
        let (auth, login) = fake_auth(None);
        let dir = identity_dir("refused");
        let (mut w, rx) = identity_worker(authority, &dir);
        w.command(LinkCmd::Login {
            auth,
            user: "robin".into(),
            password: "nope".into(),
            nick: "robin".into(),
        });
        let reason = refusal(&rx).unwrap();
        assert_eq!(
            reason & 0xffff,
            super::super::login::REASON_LOW_BAD_PASSWORD
        );
        assert!(reason != 0);
        assert!(login.try_recv().is_err(), "no lobby login after a refusal");
        assert!(!dir.join(crate::identity::CACHE_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The server entry pushes reach `dispatch` only through the list `poll` takes from the socket.
    #[test]
    fn entry_feature_pushes_are_handled() {
        for id in [
            DC_ROOM_HACKS_PUSH,
            dc_wire::gungame::OFFER,
            dc_wire::gungame::ROOM,
            dc_wire::roomhardcore::PUSH,
            dc_wire::possession::PUSH,
        ] {
            assert!(HANDLED.contains(&id), "{id:#x} is never read");
        }
    }
}
