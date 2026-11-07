//! The Camo Crate on the lobby side (CRATE-3, CRATE-4): the dedicated server's `@crate` pickup line becomes one inbox row
//! of the crate's product and an inbox-stale push 0x3014, as a picked-up HiSec case does (HS-6, HS-14); opening the row
//! (0x32e) needs no key and draws, by the odds of [`dc_bute::camo_crate::TIER_PERCENT`], either the plain base gun of one
//! of the entry's camos or one of the camos, each for the days of the HiSec case's table.
//!
//! The crate's item and product numbers are DeCombat's own ([`dc_bute::camo_crate`]); its contents come from the entry's
//! curated set, so a new camo pack on the entry adds its camo and its plain gun as soon as it loads.
// SPEC: DSERVER_PIPE.md §4 (event lines; `@crate` is a DeCombat addition, CRATE-3); HISEC_2009.md §4.3 (0x32f case
// layout, +0x1e 0: no key used)
// DC-DEFAULT: CRATE-3, CRATE-4 (proposed; USER REQUEST 2026-11-05 "that's how people will unlock them for right now"),
// CRATE-10 (proposed; USER DECISION 2026-11-06: regular guns and fixed odds; camos timed too, USER REQUEST 2026-11-06)

use crate::delivery;
use crate::inbox;
use crate::shop::reason;
use crate::state::{Core, HResult, Sid};
use dc_bute::camo_crate::{Content, Tier};
use std::collections::BTreeMap;

/// One `@crate uid=<u32> kind=camo key=<case key> name=<nick>` line (`name=` last, to the end of the line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pickup {
    pub uid: u32,
    pub key: u32,
    pub name: String,
}

/// Parses the line; None when `uid` or `key` is missing, or the kind is not `camo` (the only crate there is).
// SPEC: DSERVER_PIPE.md §4 (whitespace split, k=v tokens, unknown keys ignored); DC-DEFAULT: CRATE-3 (proposed)
pub fn pickup_from_line(line: &str) -> Option<Pickup> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (head, name) = match line.find(" name=") {
        Some(i) => (&line[..i], line[i + 6..].to_string()),
        None => (line, String::new()),
    };
    let mut toks = head.split_whitespace();
    if toks.next()? != "@crate" {
        return None;
    }
    let kv: BTreeMap<&str, &str> = toks.filter_map(|t| t.split_once('=')).collect();
    if kv.get("kind").copied() != Some("camo") {
        return None;
    }
    let n = |k: &str| kv.get(k).and_then(|v| v.parse::<u32>().ok());
    Some(Pickup {
        uid: n("uid")?,
        key: n("key")?,
        name,
    })
}

/// A crate picked up in game `game`: one inbox row of the crate product and 0x3014 to the player's sessions, under the
/// HiSec pickup's trust rules (HS-14): only for a player of that game with a character, once per case key per game.
// DC-DEFAULT: CRATE-3 (proposed) as HS-6 / HS-14, with the crate's own product
pub fn on_pickup(st: &mut Core, game: u64, p: &Pickup) -> HResult {
    let Some(g) = st.games.get(&game) else {
        return Ok(());
    };
    let in_game = g.clients.values().any(|&u| u == p.uid);
    if !in_game || st.character(p.uid).is_none() {
        st.note(&format!(
            "game {}: @crate for account {} who is not a player of this game: ignored",
            game, p.uid
        ));
        return Ok(());
    }
    if g.hisec_keys.contains(&p.key) {
        st.note(&format!(
            "game {}: @crate case key {} already delivered: ignored",
            game, p.key
        ));
        return Ok(());
    }
    let product = dc_bute::camo_crate::PRODUCT;
    if st
        .rom
        .shop
        .catalog
        .as_ref()
        .and_then(|c| c.product(product))
        .is_none()
    {
        st.note(&format!(
            "game {}: @crate for account {}: no shop catalogue, no crate",
            game, p.uid
        ));
        return Ok(());
    }
    let row = inbox::add_row(st, p.uid, product, 0, "", "")?;
    if let Some(g) = st.games.get_mut(&game) {
        g.hisec_keys.insert(p.key);
    }
    inbox::push_stale(st, p.uid);
    st.note(&format!(
        "game {}: Camo Crate key {} picked up by account {}: inbox row {}",
        game, p.key, p.uid, row
    ));
    Ok(())
}

/// What a Camo Crate holds on server entry `server` (CRATE-10): the weapon variants (VAR-1) of its curated set with their
/// tiers (CRATE-6) and the plain base gun of each; empty off such an entry.
pub fn pool(st: &Core, server: u32) -> Vec<Content> {
    st.entry_mods
        .get(&server)
        .map(|e| {
            dc_bute::camo_crate::contents(
                e.client
                    .client_content
                    .variants()
                    .iter()
                    .map(|v| (v.row, v.base, v.rarity)),
            )
        })
        .unwrap_or_default()
}

/// 0x32e for an inbox row of the crate (the caller checked the row). No key is needed. The award is drawn by the crate's
/// odds (CRATE-10); a regular gun and a camo alike come for the days of the HiSec case's table (`economy.hisec_case_odds`,
/// HS-8) and are granted or extended as any case award (CD-02). The reply is the case layout with no key (+0x1e 0). Off an
/// entry with variants the row stays and the reply is ok 0.
// SPEC: HISEC_2009.md §4.3 (0x32f case layout)
// DC-DEFAULT: CRATE-4, CRATE-10 (proposed; USER DECISION 2026-11-06 "HiSec durations" for the regular guns; USER REQUEST
// 2026-11-06 "make custom camos also have the same 7 days (50%), 30 days (35%) or permanent (15%)")
pub fn open(st: &mut Core, sid: Sid, account: u32, entry: i64) -> HResult {
    let server = st.sessions.get(&sid).map_or(0, |s| s.server);
    let pool = pool(st, server);
    if pool.is_empty() {
        st.send(sid, inbox::open_reply(false, reason::SHOP_UNAVAILABLE, &[]));
        st.note(&format!(
            "inbox: account {} opened a Camo Crate on entry {} without camos: refused, the row stays",
            account, server
        ));
        return Ok(());
    }
    if !delivery::room_for(st, account, 1)? {
        st.send(sid, inbox::open_reply(false, reason::INVENTORY_FULL, &[]));
        return Ok(());
    }
    let (tier_roll, item_roll) = (st.keys.u32(), st.keys.u32());
    let Some(won) = dc_bute::camo_crate::draw(&pool, tier_roll, item_roll) else {
        st.send(sid, inbox::open_reply(false, reason::SHOP_UNAVAILABLE, &[]));
        return Ok(());
    };
    let item_no = i64::from(won.item);
    let odds = st.rules.hisec.odds.clone();
    let days = delivery::draw_days(&odds, st.keys.u32());
    let award = delivery::grant_drawn(st, account, item_no, days)?;
    st.db
        .execute("DELETE FROM inbox WHERE uproduct = ?1", [entry])?;
    st.send(sid, inbox::case_reply(&award, 0));
    crate::notices::case_award(st, account, &award)?;
    let tier = match won.tier {
        Tier::Regular => "regular",
        Tier::Camo(r) => r.key(),
    };
    let how = match award.days {
        0 => format!("{tier}, permanent"),
        d => format!("{tier}, {d} days"),
    };
    let rows: Vec<i32> = pool.iter().map(|c| c.item).collect();
    st.note(&format!(
        "inbox: account {} opened a Camo Crate on entry {} -> item {} ({}) (pool {:?})",
        account, server, award.item_no, how, rows
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_parses_with_a_name_that_has_spaces() {
        let p = pickup_from_line("@crate uid=7 kind=camo key=1073741825 name=Big Erin\n").unwrap();
        assert_eq!(
            p,
            Pickup {
                uid: 7,
                key: 0x4000_0001,
                name: "Big Erin".into()
            }
        );
        assert!(pickup_from_line("@crate uid=7 kind=gold key=1 name=x").is_none());
        assert!(pickup_from_line("@crate uid=7 kind=camo name=x").is_none());
        assert!(pickup_from_line("@hisec uid=7 kind=camo key=1 name=x").is_none());
    }
}
