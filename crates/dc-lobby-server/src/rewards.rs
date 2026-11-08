//! Match rewards. The lobby computes them itself from the match statistics; the numbers a game server reports
//! (`@player exp= gp=`) are passed to the hook as advisory fields only, so a modified game server cannot mint
//! currency. Mods replace or adjust the rule with `mod.hooks:wrap("lobby.calc_exp_gp", fn(base, ctx))`.
// DC-DEFAULT: CD-03 (match EXP), CD-04 (match GP); hook lobby.calc_exp_gp

use crate::config::{RewardRule, Rules};
use crate::state::Core;
use crate::{db, inbox};
use dc_mod_lua::{LuaTable, LuaValue};

pub const HOOK: &str = "lobby.calc_exp_gp";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RewardCtx {
    pub account: u32,
    pub kills: i64,
    pub deaths: i64,
    pub headshots: i64,
    pub team_kills: i64,
    /// 1-based team of the player (`@player team`).
    pub team: i64,
    /// Winning team (`@result win`), 0 = draw.
    pub winner: i64,
    /// DSV-472: the server's per-player verdict (`@player win=`), which replaces the team rule when present.
    pub player_win: Option<bool>,
    /// DSV-472: the Spy Hunt agent whose upload won (`@player agent=1`): the flat agent bonus (CD-86).
    pub agent_win: bool,
    /// Objective count (not reported by game servers yet; 0).
    pub objective: i64,
    pub map: u32,
    pub mode: u32,
    pub mode_key: String,
    pub exp_factor: f64,
    pub gp_factor: f64,
    pub reported_exp: Option<i64>,
    pub reported_gp: Option<i64>,
}

impl RewardCtx {
    pub fn won(&self) -> bool {
        self.player_win
            .unwrap_or(self.winner != 0 && self.team == self.winner)
    }
}

/// One reward: (finish + kill * kills + objective * objectives), +win bonus % for the winners, plus the flat agent
/// bonus for the Spy Hunt agent whose upload won (CD-86), minus the kill value per team kill, times the map factor;
/// never negative.
pub fn one(rule: &RewardRule, c: &RewardCtx, factor: f64) -> i64 {
    let mut raw = rule.finish + rule.kill * c.kills as f64 + rule.objective * c.objective as f64;
    if c.won() {
        raw *= 1.0 + rule.win_bonus_pct / 100.0;
    }
    if c.agent_win {
        raw += rule.agent_win;
    }
    raw -= rule.kill * c.team_kills as f64;
    let f = if factor.is_finite() && factor > 0.0 {
        factor
    } else {
        1.0
    };
    (raw * f).round().max(0.0) as i64
}

/// The DeCombat default (CD-03 / CD-04), or the game server's own numbers when the operator trusts it.
// DC-DEFAULT: CD-54 (proposed) the lobby computes rewards; reported numbers are advisory unless
// economy.trust_server_rewards is set
pub fn base(rules: &Rules, c: &RewardCtx) -> (i64, i64) {
    if rules.trust_server_rewards {
        if let (Some(e), Some(g)) = (c.reported_exp, c.reported_gp) {
            return (e.max(0), g.max(0));
        }
    }
    (
        one(&rules.exp, c, c.exp_factor),
        one(&rules.gp, c, c.gp_factor),
    )
}

fn num(v: f64) -> LuaValue {
    LuaValue::Number(v)
}

pub fn to_table(c: &RewardCtx) -> LuaTable {
    let mut t = LuaTable::new();
    t.set_str("account", num(c.account as f64));
    t.set_str("kills", num(c.kills as f64));
    t.set_str("deaths", num(c.deaths as f64));
    t.set_str("headshots", num(c.headshots as f64));
    t.set_str("team_kills", num(c.team_kills as f64));
    t.set_str("team", num(c.team as f64));
    t.set_str("winner", num(c.winner as f64));
    t.set_str("won", LuaValue::Bool(c.won()));
    if let Some(w) = c.player_win {
        t.set_str("player_win", LuaValue::Bool(w));
    }
    t.set_str("agent_win", LuaValue::Bool(c.agent_win));
    t.set_str("objective", num(c.objective as f64));
    t.set_str("map", num(c.map as f64));
    t.set_str("mode", num(c.mode as f64));
    t.set_str("mode_key", LuaValue::String(c.mode_key.clone()));
    t.set_str("exp_factor", num(c.exp_factor));
    t.set_str("gp_factor", num(c.gp_factor));
    if let Some(v) = c.reported_exp {
        t.set_str("reported_exp", num(v as f64));
    }
    if let Some(v) = c.reported_gp {
        t.set_str("reported_gp", num(v as f64));
    }
    t
}

/// Reads a (possibly mod-modified) context table back.
pub fn from_table(t: &LuaTable) -> RewardCtx {
    let f = |k: &str| t.get_str(k).as_f64().unwrap_or(0.0);
    let i = |k: &str| f(k) as i64;
    RewardCtx {
        account: i("account").max(0) as u32,
        kills: i("kills"),
        deaths: i("deaths"),
        headshots: i("headshots"),
        team_kills: i("team_kills"),
        team: i("team"),
        winner: i("winner"),
        player_win: match t.get_str("player_win") {
            LuaValue::Bool(b) => Some(b),
            _ => None,
        },
        agent_win: matches!(t.get_str("agent_win"), LuaValue::Bool(true)),
        objective: i("objective"),
        map: i("map").max(0) as u32,
        mode: i("mode").max(0) as u32,
        mode_key: t.get_str("mode_key").as_str().unwrap_or("").to_string(),
        exp_factor: t.get_str("exp_factor").as_f64().unwrap_or(1.0),
        gp_factor: t.get_str("gp_factor").as_f64().unwrap_or(1.0),
        reported_exp: t.get_str("reported_exp").as_f64().map(|v| v as i64),
        reported_gp: t.get_str("reported_gp").as_f64().map(|v| v as i64),
    }
}

pub fn reward_table(exp: i64, gp: i64) -> LuaValue {
    let mut r = LuaTable::new();
    r.set_str("exp", num(exp as f64));
    r.set_str("gp", num(gp as f64));
    LuaValue::Table(std::rc::Rc::new(std::cell::RefCell::new(r)))
}

/// Reads `{exp=, gp=}` from a hook result; None when the shape is wrong.
pub fn read_result(v: &LuaValue) -> Option<(i64, i64)> {
    let t = v.as_table()?;
    let t = t.borrow();
    let e = t.get_str("exp").as_f64()?;
    let g = t.get_str("gp").as_f64()?;
    if !e.is_finite() || !g.is_finite() {
        return None;
    }
    Some((e.round() as i64, g.round() as i64))
}

/// Rewards for one player: through the mod hook when mods are loaded, else the default directly.
pub fn calc(st: &Core, c: &RewardCtx) -> (i64, i64) {
    let dflt = base(&st.rules, c);
    let Some(m) = &st.mods else { return dflt };
    match m.calc_exp_gp(c, &st.rules) {
        Ok(v) => v,
        Err(e) => {
            st.note(&format!(
                "mods: {} failed ({}); DeCombat default used",
                HOOK, e
            ));
            dflt
        }
    }
}

/// `SubType` of the bonus pass items: 13 EXP, 14 GP, 15 both.
pub const SUB_EXP_PASS: i64 = 13;
pub const SUB_GP_PASS: i64 = 14;
pub const SUB_BOTH_PASS: i64 = 15;

/// The account's bonus pass items that count now: in use, not expired, sitting in a function slot (a pass taken out
/// of its slot with 0x36a pays nothing).
fn active_passes(st: &Core, account: u32) -> Vec<(crate::db::Item, i64)> {
    let now = crate::clock::now_ticks();
    let items = crate::db::items(&st.db, account).unwrap_or_default();
    let slotted = |uid: i64| {
        st.db
            .query_row(
                "SELECT COUNT(*) FROM function_slots WHERE uid = ?1",
                [uid],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0
    };
    items
        .into_iter()
        .filter(|i| i.used != 0 && (i.end_time == 0 || i.end_time > now) && slotted(i.uid))
        .filter_map(|i| {
            let k = crate::itemuse::subtype(st, i.item_no)?;
            (SUB_EXP_PASS..=SUB_BOTH_PASS)
                .contains(&k)
                .then_some((i, k))
        })
        .collect()
}

/// The EXP and GP fractions of the active passes (0.25 = +25 %). The ROM keys are `ExpPercent` (kinds 13 and 15) and
/// `GpPercent` (kinds 14 and 15) of the pass row, written as fractions; the 2009 client only stores them, so the
/// server applies them. The descriptions allow one pass of each kind at a time: the largest counts.
// DC-DEFAULT: LG-200 (proposed) a match pays the account's active EXP / GP passes
// EVIDENCE: cshell+0x25dd60 (parser stores ExpPercent / GpPercent, nothing reads them), cshell+0x137720 (the result
// screen shows the bonus part from the 0x65 bonus fields)
pub fn pass_fractions(st: &Core, account: u32) -> (f64, f64) {
    let key = |no: i64, k: &str| {
        crate::itemuse::use_key(st, no, k)
            .and_then(|v| v.trim_matches('"').parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .map_or(0.0, |v| v.clamp(0.0, 10.0))
    };
    let mut out = (0.0f64, 0.0f64);
    for (it, k) in active_passes(st, account) {
        if k == SUB_EXP_PASS || k == SUB_BOTH_PASS {
            out.0 = out.0.max(key(it.item_no, "ExpPercent"));
        }
        if k == SUB_GP_PASS || k == SUB_BOTH_PASS {
            out.1 = out.1.max(key(it.item_no, "GpPercent"));
        }
    }
    out
}

/// The extra EXP and GP the passes add to a reward of (exp, gp).
pub fn pass_extra(fractions: (f64, f64), got: (i64, i64)) -> (i64, i64) {
    (
        (got.0.max(0) as f64 * fractions.0).round() as i64,
        (got.1.max(0) as f64 * fractions.1).round() as i64,
    )
}

/// After a played match every active counted pass (use kind 3) loses one count; a pass at 0 is deleted and the owner
/// gets the 0x33f expiry push (the client mirrors the decrement itself).
// DC-DEFAULT: LG-201 (proposed) a counted pass counts matches played while it is active, not matches in a slot
// EVIDENCE: cshell+0x134b80 (after each match the client lowers the count of every active kind 13..15 row by one)
pub fn count_down_passes(st: &mut Core, account: u32) -> rusqlite::Result<()> {
    let mut gone = Vec::new();
    for (it, _) in active_passes(st, account) {
        if it.use_type != 3 || it.remain <= 0 {
            continue;
        }
        if it.remain > 1 {
            st.db.execute(
                "UPDATE items SET remain = remain - 1 WHERE uid = ?1",
                [it.uid],
            )?;
        } else {
            crate::db::delete_item(&st.db, it.uid)?;
            st.db
                .execute("DELETE FROM function_slots WHERE uid = ?1", [it.uid])?;
            gone.push(it.uid as u32);
        }
    }
    if !gone.is_empty() {
        for sid in st.sids_of(account) {
            st.send(sid, crate::wire::expiry_push(&gone, true));
        }
    }
    Ok(())
}

/// What a match result's promotion paid (LG-303).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Promotion {
    /// Inbox rows of the promotion gifts (CD-89).
    pub gift_rows: usize,
    /// The ranks' GP bonus (CD-41).
    pub bonus_gp: i64,
}

/// The experience rank `rank` asks (the client's rank rule: rank r from `Experience<r-1>`; rank 0 from 0); `None`
/// past the table.
fn rank_exp(st: &Core, rank: u32) -> Option<i64> {
    match rank {
        0 => Some(0),
        r => st.rom.ranks.need.get(r as usize - 1).copied(),
    }
}

/// CD-89: the products a promotion to `rank` delivers: the `rewards.rank_gifts` entry of the rank when there is one,
/// else the GP products the rank unlocks ([`unlocked_products`]), else the supply case ([`rank_case_product`]).
// DC-DEFAULT: CD-89 (amended, project decision "if that rank unlocked something you get it in your inbox,
// and if that rank didn't have something that unlocks then you get a supply crate"; the crate is a random NX
// "Supply Case <weapon>" product)
pub fn rank_products(st: &mut Core, account: u32, rank: u32) -> rusqlite::Result<Vec<i32>> {
    if let Some((_, products)) = st.rules.rank_gifts.iter().find(|(r, _)| *r == rank) {
        return Ok(products.clone());
    }
    let unlocked = unlocked_products(st, account, rank)?;
    if !unlocked.is_empty() {
        return Ok(unlocked);
    }
    Ok(rank_case_product(st, rank).into_iter().collect())
}

/// CD-89: the GP products rank `rank` unlocks, one per item: each shown GP product of the shop list whose first
/// item row's `GP_Unlock` (as `shop::unlock_exp` reads it) lies in (the previous rank's experience, this rank's
/// experience], so an unlock between two ranks counts for the first rank that reaches it. Of an item's products
/// the one of `rewards.rank_unlock_days` days is taken, else the shortest period, else (permanent only) the lowest
/// id. Default / starter items and items the character owns permanently are skipped (with a note).
pub fn unlocked_products(st: &Core, account: u32, rank: u32) -> rusqlite::Result<Vec<i32>> {
    let (Some(cat), Some(lo), Some(hi)) = (
        st.rom.shop.catalog.as_ref(),
        rank.checked_sub(1).and_then(|r| rank_exp(st, r)),
        rank_exp(st, rank),
    ) else {
        return Ok(Vec::new());
    };
    let scan: Vec<i32> = if st.rom.shop.all_products.is_empty() {
        (0..0x10000).collect()
    } else {
        st.rom.shop.all_products.clone()
    };
    let mut by_item: std::collections::BTreeMap<i64, Vec<&dc_bute::ShopProduct>> =
        Default::default();
    for pid in scan {
        let Some(p) = cat.product(pid) else {
            continue;
        };
        if p.sale_type != 0 || !crate::shop::is_shown(st, pid) {
            continue;
        }
        let unlock = crate::shop::unlock_exp(cat, p);
        if unlock <= lo || unlock > hi {
            continue;
        }
        let items = crate::shop::product_items(cat, p);
        match items.first() {
            Some(i) if i.consume != crate::delivery::Consume::GpGrant => {
                by_item.entry(i.item_no).or_default().push(p)
            }
            _ => {}
        }
    }
    let owned = db::items(&st.db, account)?;
    let days = st.rules.rank_unlock_days as i32;
    let mut out = Vec::new();
    for (item, variants) in by_item {
        let starter = st.rom.starter.iter().any(|s| s.item_no == item);
        let default = owned
            .iter()
            .any(|i| i.item_no == item && i.default_type != 0);
        let permanent = owned
            .iter()
            .any(|i| i.item_no == item && i.end_time == 0 && i.remain == 0);
        if starter || default || permanent {
            st.note(&format!(
                "promotion: rank {rank} unlocks item {item}, skipped (a default item or owned permanently)"
            ));
            continue;
        }
        let timed = |p: &&&dc_bute::ShopProduct| (1..=998).contains(&p.period);
        let pick = variants
            .iter()
            .filter(|p| p.period == days)
            .min_by_key(|p| p.id)
            .or_else(|| {
                variants
                    .iter()
                    .filter(timed)
                    .min_by_key(|p| (p.period, p.id))
            })
            .or_else(|| variants.iter().min_by_key(|p| p.id));
        if let Some(p) = pick {
            out.push(p.id);
        }
    }
    Ok(out)
}

/// CD-89: the NX supply case pool: `rewards.rank_case_products`, else every credit product of the shop list named
/// "Supply Case ..." that holds one supply case (consume type 2 or item class 0x14, one case: no pack) whose ROM
/// supply table awards weapons only (item numbers 0..=999), one product per case item (the lowest id). The GP cases
/// (sale type GP, or the credit ones that award GP) and the HiSec cases are not in it.
pub fn case_pool(st: &Core) -> Vec<i32> {
    if !st.rules.rank_case_products.is_empty() {
        return st.rules.rank_case_products.clone();
    }
    let Some(cat) = st.rom.shop.catalog.as_ref() else {
        return Vec::new();
    };
    let scan: Vec<i32> = if st.rom.shop.all_products.is_empty() {
        (0..0x10000).collect()
    } else {
        st.rom.shop.all_products.clone()
    };
    let mut by_item = std::collections::BTreeMap::new();
    for pid in scan {
        let Some(p) = cat.product(pid) else {
            continue;
        };
        let name = p.name.trim().trim_matches('"').trim();
        if p.sale_type != 1 || !name.starts_with("Supply Case") {
            continue;
        }
        let items = crate::shop::product_items(cat, p);
        if items.len() != 1 || !crate::shop::is_case(&items) || items[0].period > 1 {
            continue;
        }
        // a weapon case: every item its ROM supply table can award is a weapon (0..=999); not the GP cases
        let weapons = st.rom.items.as_ref().is_none_or(|t| {
            let awards = t.supply_pool(items[0].item_no as i32);
            !awards.is_empty() && awards.iter().all(|i| (0..1000).contains(i))
        });
        if weapons {
            by_item.entry(items[0].item_no).or_insert(pid);
        }
    }
    by_item.into_values().collect()
}

/// CD-89: the supply case of a rank that unlocks nothing: one product of [`case_pool`] at random (logged).
pub fn rank_case_product(st: &mut Core, rank: u32) -> Option<i32> {
    let pool = case_pool(st);
    if pool.is_empty() {
        return None;
    }
    let p = pool[st.keys.u32() as usize % pool.len()];
    st.note(&format!(
        "promotion: rank {rank} unlocks nothing; NX supply case product {p} of a pool of {}",
        pool.len()
    ));
    Some(p)
}

/// LG-303: a match result raised `account` from rank `before` to rank `after` (the client's rank numbers): each rank
/// above both `before` and the highest rank already rewarded, up to `after`, adds its ROM GP bonus (CD-41, off with
/// `economy.rank_bonus = false`) and puts its [`rank_products`] into the inbox (CD-89: the override, else the GP
/// products it unlocks, else one NX supply case), so a rank pays once per character; the rewarded rank is stored. A
/// product the shop list does not hold is skipped with a note.
// EVIDENCE: cshell+0x40580 (0x3004: a rank above the stored one, with the popup flag, shows box 0x185f "...Your
// Event items can be found in the Inbox."), cshell+0x15fb50 (notice 0x487 "...A rank bonus of %d GP has been
// awarded!" with the level table's bonus, cshell+0x267ec0); no ROM table names promotion items
// DC-DEFAULT: LG-303 (proposed) the promotion pays the rank bonus and the rank's gifts, once per rank
pub fn promote(
    st: &mut Core,
    account: u32,
    before: u32,
    after: u32,
) -> rusqlite::Result<Promotion> {
    let mut out = Promotion::default();
    if after <= before {
        return Ok(out);
    }
    let from = before.max(db::rank_rewarded(&st.db, account)?);
    for rank in from.saturating_add(1)..=after {
        if st.rules.rank_bonus {
            out.bonus_gp += st.rom.ranks.bonus_of(rank);
        }
        for p in rank_products(st, account, rank)? {
            let known = st
                .rom
                .shop
                .catalog
                .as_ref()
                .is_some_and(|c| c.product(p).is_some());
            if !known {
                st.note(&format!(
                    "promotion: rank {rank} gift product {p} is not in the shop list, skipped"
                ));
                continue;
            }
            out.gift_rows += inbox::add_product_rows(st, account, p, 0, "", "")?.len();
        }
    }
    if out.bonus_gp > 0 {
        db::grant(&st.db, account, 0, 0, out.bonus_gp, 0)?;
    }
    if from < after {
        db::set_rank_rewarded(&st.db, account, after)?;
        st.note(&format!(
            "promotion: account {account} rank {before} -> {after}: +{} GP bonus, {} inbox gift row(s)",
            out.bonus_gp, out.gift_rows
        ));
    }
    Ok(out)
}

/// LG-317 (amended by LG-320): the bots of a finished match that are paid nothing: a seat of the bot pool, wherever it
/// connects from, and any other bot ([`crate::lifecycle::is_human`] false) connected from another machine. The people
/// with them are always paid. A bot that hides behind a person's name and a changed binary is out of its reach
/// (docs/BOTS.md).
// DC-DEFAULT: LG-317, LG-320 (proposed)
pub fn unpaid_bots(
    st: &Core,
    players: &[crate::game::PlayerResult],
    leavers: &[crate::game::PlayerResult],
) -> std::collections::BTreeSet<u32> {
    players
        .iter()
        .chain(leavers)
        .filter(|p| {
            st.pool.has(p.uid)
                || (!crate::lifecycle::is_human(st, p.uid)
                    && st.sessions.values().any(|s| {
                        s.account == Some(p.uid) && crate::announce::is_remote_peer(&s.peer)
                    }))
        })
        .map(|p| p.uid)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(kills: i64, team: i64, winner: i64) -> RewardCtx {
        RewardCtx {
            kills,
            team,
            winner,
            exp_factor: 1.0,
            gp_factor: 1.0,
            ..Default::default()
        }
    }

    /// CD-86 / DSV-472: a per-player verdict replaces the team rule, and the agent whose upload won gets the flat
    /// extra on top of the win bonus; the other players of the same match get nothing of either.
    #[test]
    fn the_agent_win_extra_and_the_per_player_verdict() {
        let r = Rules::default();
        // a Spy Hunt result: @result win=0, the agent's line says win=1 agent=1
        let mut agent = ctx(2, 1, 0);
        agent.player_win = Some(true);
        agent.agent_win = true;
        // (15 + 12) * 1.25 + 45 = 78.75 -> 79 EXP; GP: (20 + 12) * 1.25 + 0 = 40
        assert_eq!(base(&r, &agent), (79, 40));
        assert!(agent.won());
        // a hunter with the same kills on the same lobby side: win=0 -> no win bonus, no extra
        let mut hunter = ctx(2, 1, 0);
        hunter.player_win = Some(false);
        assert_eq!(base(&r, &hunter), (27, 32));
        assert!(!hunter.won());
        // the hunters' win after the agent fell: win=1 without agent=1 is the plain win bonus
        let mut hunter_won = ctx(2, 1, 0);
        hunter_won.player_win = Some(true);
        assert_eq!(base(&r, &hunter_won), (34, 40));
        // a per-player verdict beats the team rule either way
        let mut overruled = ctx(2, 1, 1);
        overruled.player_win = Some(false);
        assert!(!overruled.won());
        // the mod hook's table carries both flags back and forth
        let back = from_table(&to_table(&agent));
        assert_eq!((back.player_win, back.agent_win), (Some(true), true));
    }

    #[test]
    fn cd03_cd04_defaults() {
        let r = Rules::default();
        // winner with 2 kills: (15 + 12) * 1.25 = 33.75 -> 34 exp; (20 + 12) * 1.25 = 40 GP
        assert_eq!(base(&r, &ctx(2, 1, 1)), (34, 40));
        // loser with no kills: finish only
        assert_eq!(base(&r, &ctx(0, 2, 1)), (15, 20));
        // draw: no win bonus
        assert_eq!(base(&r, &ctx(1, 1, 0)), (21, 26));
        // team kills cost the kill value, never below zero
        let mut c = ctx(0, 2, 1);
        c.team_kills = 10;
        assert_eq!(base(&r, &c), (0, 0));
        // map factor scales
        let mut c = ctx(0, 2, 1);
        c.exp_factor = 1.5;
        c.gp_factor = 2.0;
        // 15 * 1.5 = 22.5 -> 23 (half away from zero)
        assert_eq!(base(&r, &c), (23, 40));
    }

    #[test]
    fn reported_numbers_do_not_change_the_default() {
        let mut r = Rules::default();
        let mut c = ctx(2, 1, 1);
        c.reported_exp = Some(3);
        c.reported_gp = Some(1);
        assert_eq!(base(&r, &c), (34, 40));
        r.trust_server_rewards = true;
        assert_eq!(
            base(&r, &c),
            (3, 1),
            "opt-in: the game server's own numbers"
        );
    }

    #[test]
    fn table_round_trip() {
        let mut c = ctx(3, 2, 1);
        c.mode_key = "TDM".into();
        c.reported_gp = Some(5);
        let back = from_table(&to_table(&c));
        assert_eq!(back, c);
        assert_eq!(read_result(&reward_table(7, 8)), Some((7, 8)));
        assert_eq!(read_result(&LuaValue::Nil), None);
    }
}
