//! The megaphone: queue status (0x36e / 0x36f), sending (0x36c / 0x36d) and the broadcast push 0x3065 type 1.
//! One message is on air at a time; the queue lives in the DB (`megaphone`), so the serials keep growing over
//! restarts. A one-second ticker thread puts the next message on air.
// SPEC: PROTOCOL_2009.md §4.3 (0x36c..0x36f), §3.6 / §6.2 (0x3065)

use crate::shop::account_of;
use crate::shopmsg::*;
use crate::state::{Core, HResult, Shared, Sid};
use crate::{clock, db};
use dc_wire::{PktReader, PktWriter};
use rusqlite::{params, OptionalExtension};
use std::time::Duration;

/// 0x36f / 0x36d refusal reasons (client boxes res 0x1903 pending, res 0x1905 unavailable).
// SPEC: PROTOCOL_2009.md §4.3 0x36f (reason 0x8f pending, 0x90 unavailable); reason table cshell+0x393f0
pub const REASON_PENDING: u32 = 0x8f;
pub const REASON_UNAVAILABLE: u32 = 0x90;

/// Queue state for 0x36f.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueState {
    pub on_air: u32,
    pub last: u32,
    pub waiting: u32,
}

/// 0x36f (0x22): +6 ok; ok = 1: +0xe seconds per message, +0x12 order on air, +0x16 last queued order,
/// +0x1e players waiting; ok = 0: +0xa reason.
// SPEC: PROTOCOL_2009.md §4.3 0x36f (EVIDENCE: cshell+0x367b0 -> cshell+0x2a8990)
pub fn queue_reply(ok: bool, reason: u32, secs: u32, q: QueueState) -> Vec<u8> {
    let mut w = PktWriter::new(MEGAPHONE_QUEUE_REPLY);
    w.u32(ok as u32)
        .u32(reason)
        .u32(secs)
        .u32(q.on_air)
        .u32(q.last)
        .u32(0)
        .u32(q.waiting);
    w.into_frame()
}

/// Parsed 0x36c: megaphone unique id, message, colour runs (raw u16 pairs as sent).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendRequest {
    pub uid: u32,
    pub text: String,
    pub runs: Vec<u8>,
}

// SPEC: PROTOCOL_2009.md §4.3 0x36c (EVIDENCE: cshell+0x18f470): +6 uid; +0xa 1; +0xe wstr[101]; +0xd8 wstr[201]
pub fn parse_send(p: &[u8]) -> SendRequest {
    let mut r = PktReader::new(p);
    let uid = r.at(6).u32();
    let text = r.at(0xe).wstr(101);
    let runs = r.at(0xd8).bytes_padded(402);
    SendRequest { uid, text, runs }
}

/// 0x36d (>= 0x16): +6 ok; +0xa reason; +0xe unique id; +0x12 uses left (0 = the client removes the item).
// SPEC: PROTOCOL_2009.md §4.3 0x36d (EVIDENCE: cshell+0x36770 -> cshell+0x2a5c90)
pub fn send_reply(ok: bool, reason: u32, uid: u32, left: u32) -> Vec<u8> {
    let mut w = PktWriter::new(MEGAPHONE_SEND_REPLY);
    w.u32(ok as u32).u32(reason).u32(uid).u32(left);
    w.into_frame()
}

/// One message as broadcast.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Broadcast {
    pub secs: u32,
    pub serial: u32,
    pub sender: u32,
    pub colour: u32,
    pub name: String,
    pub text: String,
    pub runs: Vec<u8>,
}

/// 0x3065 type 1 (0x2a2): +6 1; +0xa seconds on air; +0xe order; +0x12 serial (older or equal is dropped);
/// +0x16 sender account (own -> res 0x1904); +0x22 sender colour; +0x26 wstr[17] name; +0x48 wstr[100] message;
/// +0x110 wstr[201] colour runs.
// SPEC: PROTOCOL_2009.md §3.6 0x3065 (EVIDENCE: cshell+0x38190 -> cshell+0x1202d0)
pub fn broadcast_push(b: &Broadcast) -> Vec<u8> {
    let mut w = PktWriter::new(BROADCAST_PUSH);
    w.u32(1)
        .u32(b.secs)
        .u32(b.serial)
        .u32(b.serial)
        .u32(b.sender);
    w.at(0x22)
        .u32(b.colour)
        .wstr(&b.name, 17)
        .wstr(&b.text, 100);
    let mut runs = b.runs.clone();
    runs.resize(402, 0);
    w.bytes(&runs);
    debug_assert_eq!(w.len(), 0x2a2);
    w.into_frame()
}

fn state(st: &Core) -> rusqlite::Result<QueueState> {
    let on_air: i64 = st.db.query_row(
        "SELECT COALESCE(MAX(serial), 0) FROM megaphone WHERE aired > 0 AND done = 0",
        [],
        |r| r.get(0),
    )?;
    let waiting: i64 = st.db.query_row(
        "SELECT COUNT(*) FROM megaphone WHERE aired = 0 AND done = 0",
        [],
        |r| r.get(0),
    )?;
    let last: i64 = st.db.query_row(
        "SELECT COALESCE(MAX(serial), 0) FROM megaphone WHERE done = 0",
        [],
        |r| r.get(0),
    )?;
    let on_air = if on_air == 0 && waiting == 0 {
        last
    } else {
        on_air
    };
    Ok(QueueState {
        on_air: on_air as u32,
        last: last.max(on_air) as u32,
        waiting: waiting as u32,
    })
}

/// A message of the account still waits for its turn (res 0x1903: wait until it is broadcast).
fn pending_of(st: &Core, account: u32) -> rusqlite::Result<bool> {
    let n: i64 = st.db.query_row(
        "SELECT COUNT(*) FROM megaphone WHERE user_no = ?1 AND aired = 0 AND done = 0",
        [account as i64],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

// DC-DEFAULT: CD-40 one message on air for megaphone.secs seconds, FIFO; CD-83 (proposed) one pending message per
// account (reason 0x8f)
pub fn on_queue(st: &mut Core, sid: Sid) -> HResult {
    let secs = st.rules.shop.megaphone_secs;
    let Some(account) = account_of(st, sid) else {
        st.send(
            sid,
            queue_reply(false, REASON_UNAVAILABLE, secs, QueueState::default()),
        );
        return Ok(());
    };
    if pending_of(st, account)? {
        st.send(
            sid,
            queue_reply(false, REASON_PENDING, secs, QueueState::default()),
        );
        return Ok(());
    }
    let q = state(st)?;
    st.send(sid, queue_reply(true, 0, secs, q));
    Ok(())
}

/// True for a megaphone item (ROM UseItem SubType 22).
fn is_megaphone(st: &Core, item_no: i64) -> bool {
    crate::itemuse::subtype(st, item_no) == Some(crate::itemuse::SUB_MEGAPHONE)
}

// DC-DEFAULT: CD-40 megaphone queue; CD-83 (proposed) 1..megaphone.max_chars characters, one use per message
// (0 left -> the row is removed, as the client removes the item), every lobby session hears it
pub fn on_send(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let req = parse_send(p);
    let Some(account) = account_of(st, sid) else {
        st.send(sid, send_reply(false, REASON_UNAVAILABLE, req.uid, 0));
        return Ok(());
    };
    let item = db::item(&st.db, req.uid as i64)?.filter(|i| i.user_no == account);
    let Some(item) =
        item.filter(|i| is_megaphone(st, i.item_no) && (i.use_type != 3 || i.remain > 0))
    else {
        st.send(sid, send_reply(false, REASON_UNAVAILABLE, req.uid, 0));
        return Ok(());
    };
    let text: String = req.text.trim_end_matches('\0').to_string();
    let chars = text.chars().count();
    if chars == 0 || chars > st.rules.shop.megaphone_max_chars {
        st.send(
            sid,
            send_reply(
                false,
                REASON_UNAVAILABLE,
                req.uid,
                item.remain.max(0) as u32,
            ),
        );
        return Ok(());
    }
    if pending_of(st, account)? {
        st.send(
            sid,
            send_reply(false, REASON_PENDING, req.uid, item.remain.max(0) as u32),
        );
        return Ok(());
    }
    let now = clock::now_ticks();
    // DC-DEFAULT: CD-33 a muted player's text is dropped server-side; the megaphone counts as chat
    if st
        .character(account)
        .map(|c| c.mute_until > now)
        .unwrap_or(false)
    {
        st.send(
            sid,
            send_reply(
                false,
                REASON_UNAVAILABLE,
                req.uid,
                item.remain.max(0) as u32,
            ),
        );
        return Ok(());
    }
    if item.end_time > 0 && item.end_time <= now {
        st.send(sid, send_reply(false, REASON_UNAVAILABLE, req.uid, 0));
        return Ok(());
    }
    let left = if item.use_type == 3 {
        (item.remain - 1).max(0)
    } else {
        1
    };
    if item.use_type == 3 {
        if left == 0 {
            db::delete_item(&st.db, item.uid)?;
        } else {
            st.db.execute(
                "UPDATE items SET remain = ?2, used = 1 WHERE uid = ?1",
                params![item.uid, left],
            )?;
        }
    }
    let ch = st.character(account).unwrap_or_default();
    st.db.execute(
        "INSERT INTO megaphone (user_no, name, colour, text, runs, queued, aired, done) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0)",
        params![account as i64, ch.nick, ch.nick_color, text, req.runs, clock::unix_now()],
    )?;
    st.send(sid, send_reply(true, 0, req.uid, left as u32));
    st.note(&format!(
        "megaphone: {} queued a message ({} uses left)",
        ch.nick, left
    ));
    tick(st)?;
    Ok(())
}

/// Ends the message on air when its time is up and puts the next one on air (broadcast to every lobby session).
pub fn tick(st: &mut Core) -> HResult {
    let secs = st.rules.shop.megaphone_secs as i64;
    let now = clock::unix_now();
    st.db.execute(
        "UPDATE megaphone SET done = 1 WHERE aired > 0 AND done = 0 AND aired + ?1 <= ?2",
        params![secs, now],
    )?;
    let busy: i64 = st.db.query_row(
        "SELECT COUNT(*) FROM megaphone WHERE aired > 0 AND done = 0",
        [],
        |r| r.get(0),
    )?;
    if busy > 0 {
        return Ok(());
    }
    let next: Option<(i64, i64, String, i64, String, Vec<u8>)> = st
        .db
        .query_row(
            "SELECT serial, user_no, name, colour, text, runs FROM megaphone WHERE aired = 0 AND done = 0 ORDER BY serial LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    let Some((serial, user, name, colour, text, runs)) = next else {
        return Ok(());
    };
    st.db.execute(
        "UPDATE megaphone SET aired = ?2 WHERE serial = ?1",
        params![serial, now],
    )?;
    let frame = broadcast_push(&Broadcast {
        secs: secs as u32,
        serial: serial as u32,
        sender: user as u32,
        colour: colour.clamp(0, u32::MAX as i64) as u32,
        name,
        text,
        runs,
    });
    let audience: Vec<Sid> = st
        .sessions
        .values()
        .filter(|s| s.in_lobby && !s.closing)
        .map(|s| s.sid)
        .collect();
    st.send_all(&audience, &frame);
    Ok(())
}

/// The one-second ticker thread.
pub fn spawn_ticker(shared: Shared) {
    let _ = std::thread::Builder::new()
        .name("megaphone".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let mut g = crate::net::lock(&shared);
            if let Err(e) = g.scope(tick) {
                g.note(&format!("megaphone tick (rolled back): {}", e));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::u32_at;

    fn split(f: &[u8]) -> (i32, Vec<u8>) {
        let (frames, _) = dc_wire::parse_tcp_frames(f).unwrap();
        frames.into_iter().next().unwrap()
    }

    #[test]
    fn queue_and_send_layouts() {
        let (id, p) = split(&queue_reply(
            true,
            0,
            30,
            QueueState {
                on_air: 4,
                last: 6,
                waiting: 2,
            },
        ));
        assert_eq!((id, p.len()), (0x36f, 0x22));
        assert_eq!(
            (
                u32_at(&p, 6),
                u32_at(&p, 0xe),
                u32_at(&p, 0x12),
                u32_at(&p, 0x16),
                u32_at(&p, 0x1e)
            ),
            (1, 30, 4, 6, 2)
        );
        let (_, p) = split(&queue_reply(
            false,
            REASON_PENDING,
            30,
            QueueState::default(),
        ));
        assert_eq!((u32_at(&p, 6), u32_at(&p, 0xa)), (0, 0x8f));
        let (id, p) = split(&send_reply(true, 0, 9, 1));
        assert_eq!(
            (id, p.len(), u32_at(&p, 0xe), u32_at(&p, 0x12)),
            (0x36d, 0x16, 9, 1)
        );
        let mut w = PktWriter::new(MEGAPHONE_SEND);
        w.u32(5)
            .u32(1)
            .wstr("hello all", 101)
            .u16(2)
            .u16(5)
            .pad(398);
        let p = w.into_payload();
        assert_eq!(p.len(), 0x26a);
        let r = parse_send(&p);
        assert_eq!(
            (r.uid, r.text.as_str(), &r.runs[..4]),
            (5, "hello all", &[2u8, 0, 5, 0][..])
        );
    }

    #[test]
    fn broadcast_layout() {
        let b = Broadcast {
            secs: 30,
            serial: 3,
            sender: 17,
            colour: 10,
            name: "Alice".into(),
            text: "hello all".into(),
            runs: vec![2, 0, 5, 0],
        };
        let (id, p) = split(&broadcast_push(&b));
        assert_eq!((id, p.len()), (0x3065, 0x2a2));
        assert_eq!(
            (
                u32_at(&p, 6),
                u32_at(&p, 0xa),
                u32_at(&p, 0x12),
                u32_at(&p, 0x16),
                u32_at(&p, 0x22)
            ),
            (1, 30, 3, 17, 10)
        );
        assert_eq!(PktReader::from_raw(&p).at(0x26).wstr(17), "Alice");
        assert_eq!(PktReader::from_raw(&p).at(0x48).wstr(100), "hello all");
        assert_eq!(&p[0x110..0x114], &[2, 0, 5, 0]);
    }
}
