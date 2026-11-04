//! Coupons (0x340 / 0x341): codes issued by the operator in `shop.coupons`; a valid code drops its product into
//! the inbox. No code is compiled in.
// SPEC: PROTOCOL_2009.md §4.4

use crate::clock;
use crate::inbox;
use crate::shop::account_of;
use crate::shopmsg::*;
use crate::state::{Core, HResult, Sid};
use dc_wire::{PktReader, PktWriter};
use rusqlite::{params, OptionalExtension};

/// 0x341 refusal reasons (client boxes res 0x1828 / 0x182a / 0x182e).
// SPEC: PROTOCOL_2009.md §4.4 0x341 (EVIDENCE: cshell+0x26a3e0; reason table cshell+0x393f0)
pub const REASON_INVALID: u32 = 0x71;
pub const REASON_USED: u32 = 0x72;
pub const REASON_TRY_LATER: u32 = 0x73;

/// Canonical form of a code: letters and digits only, upper case (the client's input box joins six parts).
// DC-DEFAULT: CD-84 (proposed) codes compare case-insensitively, ignoring separators and blanks
pub fn normalize(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// 0x341 (>= 0x12): +6 ok; +0xa reason; +0xe product id.
// SPEC: PROTOCOL_2009.md §4.4 0x341
pub fn redeem_reply(ok: bool, reason: u32, product: u32) -> Vec<u8> {
    let mut w = PktWriter::new(COUPON_REDEEM_REPLY);
    w.u32(ok as u32).u32(reason).u32(product);
    w.into_frame()
}

/// Brings the coupon table in line with the operator's list (codes, products, use caps).
fn sync(st: &Core) -> rusqlite::Result<()> {
    for c in &st.rules.shop.coupons {
        let code = normalize(&c.code);
        if code.is_empty() {
            continue;
        }
        st.db.execute(
            "INSERT INTO coupons (code, product_id, uses, created) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(code) DO UPDATE SET product_id = excluded.product_id, uses = excluded.uses",
            params![code, c.product, c.uses, clock::unix_now()],
        )?;
    }
    Ok(())
}

// DC-DEFAULT: CD-39 operator codes, one use per account, the product goes to the inbox; CD-84 (proposed) a total
// use cap per code, an unknown product answers "try later", the user gets 0x3014
pub fn on_redeem(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let code = normalize(&PktReader::new(p).at(6).wstr(65));
    let Some(account) = account_of(st, sid) else {
        st.send(sid, redeem_reply(false, REASON_INVALID, 0));
        return Ok(());
    };
    sync(st)?;
    let row: Option<(i64, i64)> = st
        .db
        .query_row(
            "SELECT product_id, uses FROM coupons WHERE code = ?1",
            [&code],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((product, uses)) = row.filter(|_| !code.is_empty()) else {
        st.send(sid, redeem_reply(false, REASON_INVALID, 0));
        return Ok(());
    };
    let mine: i64 = st.db.query_row(
        "SELECT COUNT(*) FROM coupon_uses WHERE code = ?1 AND user_no = ?2",
        params![code, account as i64],
        |r| r.get(0),
    )?;
    let all: i64 = st.db.query_row(
        "SELECT COUNT(*) FROM coupon_uses WHERE code = ?1",
        [&code],
        |r| r.get(0),
    )?;
    if mine > 0 || (uses >= 0 && all >= uses) {
        st.send(sid, redeem_reply(false, REASON_USED, 0));
        return Ok(());
    }
    let known = st
        .rom
        .shop
        .catalog
        .as_ref()
        .map(|c| c.product(product as i32).is_some())
        .unwrap_or(false);
    if !known {
        st.send(sid, redeem_reply(false, REASON_TRY_LATER, 0));
        return Ok(());
    }
    st.db.execute(
        "INSERT INTO coupon_uses (code, user_no, used) VALUES (?1, ?2, ?3)",
        params![code, account as i64, clock::unix_now()],
    )?;
    inbox::add_product_rows(st, account, product as i32, 0, "", "")?;
    st.send(sid, redeem_reply(true, 0, product as u32));
    inbox::push_stale(st, account);
    st.note(&format!(
        "coupon: account {} redeemed a code for product {}",
        account, product
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::u32_at;

    #[test]
    fn codes_normalize() {
        assert_eq!(normalize(" abcde-FGHIJ 12345 "), "ABCDEFGHIJ12345");
        assert_eq!(normalize("--"), "");
    }

    #[test]
    fn reply_layout() {
        let (frames, _) = dc_wire::parse_tcp_frames(&redeem_reply(true, 0, 1748)).unwrap();
        let p = &frames[0].1;
        assert_eq!((frames[0].0, p.len()), (0x341, 0x12));
        assert_eq!((u32_at(p, 6), u32_at(p, 0xa), u32_at(p, 0xe)), (1, 0, 1748));
        let mut w = PktWriter::new(COUPON_REDEEM);
        w.wstr("abc", 65);
        assert_eq!(w.into_payload().len(), 0x88, "0x340 size");
    }
}
