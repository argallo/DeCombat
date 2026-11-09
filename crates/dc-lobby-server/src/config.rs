//! Server-side rules: every value here is a DeCombat default (`DC_DEFAULTS.md`), never a 2009 server value.
//! Operators override them with a flat JSON object in `<db dir>/lobby.json`, e.g.
//! `{"economy.exp.kill": 5, "room.countdown_ticks": 3}`. Unknown keys are reported and ignored.
//! Mods override the reward rule through the `lobby.calc_exp_gp` hook (see `rewards.rs`).

use dc_mod_lua::JsonValue;
use std::path::Path;

/// One reward rule (CD-03 shape; CD-04 uses the same shape with its own numbers).
#[derive(Clone, Debug, PartialEq)]
pub struct RewardRule {
    pub finish: f64,
    pub kill: f64,
    pub assist: f64,
    pub win_bonus_pct: f64,
    pub objective: f64,
    /// CD-86: the flat extra of the Spy Hunt agent whose upload won.
    pub agent_win: f64,
}

#[derive(Clone, Debug)]
pub struct Rules {
    // DC-DEFAULT: CD-03 match EXP: finish 15, kill 6, assist 0, win +25 %, objective 22 (x1.5 2026-11-07), team kill = -kill value
    pub exp: RewardRule,
    // DC-DEFAULT: CD-04 match GP: finish 20, kill 6, win +25 %, objective 25
    pub gp: RewardRule,
    // DC-DEFAULT: CD-06 expiry scan every 60 s (and at login)
    pub expiry_scan_secs: u64,
    // DC-DEFAULT: CD-10 dedicated server UDP port range
    pub dserver_port_lo: u16,
    pub dserver_port_hi: u16,
    // DC-DEFAULT: CD-16 names unique per server, case-insensitive
    pub names_case_insensitive: bool,
    // DC-DEFAULT: CD-19 session key lifetime
    pub session_ttl_secs: u64,
    // DC-DEFAULT: CD-21 one server, three channels
    pub channels: u32,
    // DC-DEFAULT: CD-23 five countdown ticks (the interval is the --countdown flag, default 1 s = CD-23 1000 ms)
    pub countdown_ticks: u32,
    // DC-DEFAULT: CD-24 a ready request while the room plays is a late join (when the room option allows it)
    pub allow_late_join: bool,
    // DC-DEFAULT: CD-25 at most 100 rooms per channel
    pub max_rooms_per_channel: u32,
    // DC-DEFAULT: CD-18 starter loadout override (empty = pick from the ROM item table)
    pub starter_items: Vec<i32>,
    // DC-DEFAULT: CD-49 (proposed) new rooms allow joining a running match
    pub default_mid_join: u32,
    // DC-DEFAULT: CD-50 (proposed) grace period between `quit` and a forced kill of a game server
    pub dserver_quit_grace_secs: u64,
    // DC-DEFAULT: CD-48 (proposed) the starter kit includes the first ROM backpack with >= 1 extra slot
    pub starter_backpack: bool,
    // DC-DEFAULT: CD-51 (proposed) a new member joins the smaller team (false: always team 1)
    pub entry_team_balance: bool,
    // DC-DEFAULT: CD-52 (proposed) an empty weapon / backpack slot gets its default item back
    pub default_repair: bool,
    // DC-DEFAULT: CD-52 (approved with the user's change 2026-10-24) the backpack slot is never auto-filled;
    // inventory.default_repair = "all" turns it back on, "weapons" (default) / true repairs weapon slots only
    pub default_repair_backpack: bool,
    // DC-DEFAULT: CD-53 (proposed) a counted period item starts its period when used
    pub period_activation: bool,
    // DC-DEFAULT: CD-54 (proposed) rewards are computed by the lobby; true = take a game server's reported
    // exp / gp when present (DSERVER_PIPE.md §4.4 behaviour, for trusted servers only)
    pub trust_server_rewards: bool,
    // DC-DEFAULT: REW-1 (project decision false = a player who left before the round end gets no
    // experience / GP, no match count and no 0x3004; true = every `@player` entry is paid
    pub pay_leavers: bool,
    /// LG-318: the accounts of the bot pool (`bots.pool_size`; 0 = no pool).
    pub bot_pool_size: u32,
    /// LG-318: seats per address (`bots.pool_per_ip`; this machine has no limit).
    pub bot_pool_per_ip: u32,
    /// LG-319: people online from which rooms keep a seat free of bots (`bots.keep_seat_online_players`; 0 = never).
    pub bot_keep_seat_online: u32,
    /// LG-438: the kit the pool's seats wear (`bots.pool_kit`): `auto` = the ROM's light vest, "" / `none` = no
    /// kit, else a loadout spec as `account loadout` takes it.
    pub bot_pool_kit: String,
    // DC-DEFAULT: CD-89 (proposed) the promotion gifts: (rank, shop product ids) a match result's rank-up delivers to
    // the inbox (LG-303); empty = none
    pub rank_gifts: Vec<(u32, Vec<i32>)>,
    // DC-DEFAULT: CD-89 (project decision days of the unlocked product's variant a promotion gives
    pub rank_unlock_days: u32,
    // DC-DEFAULT: CD-89 (project decision the NX supply case pool of a rank that unlocks nothing; empty =
    // every NX "Supply Case ..." product of the ROM
    pub rank_case_products: Vec<i32>,
    // DC-DEFAULT: LG-310, LG-311 (proposed) NX is earned: the daily login bonus and the clan match win (prepaid balance)
    pub nx_enabled: bool,
    pub nx_daily: i64,
    pub nx_clan_win: i64,
    pub nx_clan_win_daily_cap: u32,
    pub nx_clan_min_players: u32,
    pub nx_clan_min_seconds: u32,
    // DC-DEFAULT: CD-41 (amended, LG-303) a rank-up adds the ROM level table's `Bonus<rank>` GP
    pub rank_bonus: bool,
    // DC-DEFAULT: CD-55 (proposed) one lobby session per account: a second login drops the older connection
    pub single_session: bool,
    // DC-DEFAULT: CD-56 (proposed) room size cap (players per room)
    pub max_players_cap: u32,
    // DC-DEFAULT: CD-57 (proposed) the host may hand over the host role in any room state
    pub handover_any_state: bool,
    // DC-DEFAULT: LDO-1 (proposed) the `user` line carries `loadout=` and `face=` tokens
    pub dserver_loadout_tokens: bool,
    // DC-DEFAULT: DSV-35 (proposed) the lobby sends `gain <uid> <exp> <gp>` after a stored @result (DSERVER_PIPE §12.B)
    pub send_gain_lines: bool,
    // DC-DEFAULT: RM-1 (proposed) a host settings change clears every other member's ready state
    pub settings_change_clears_ready: bool,
    // DC-DEFAULT: LG-203 (proposed) the room badge field and the room kick need an Elite Moderator holder (or a GM)
    pub kick_needs_badge: bool,
    // DC-DEFAULT: AUTH-1 (proposed) the first account number handed to a new account (reference parity)
    pub first_account_no: u32,
    // shop, inventory, inbox, coupon and megaphone rules (shopcfg.rs)
    pub shop: crate::shopcfg::ShopRules,
    // clan rules (clan.rs: CD-11, CLN-n)
    pub clan: crate::clan::ClanRules,
    // GM tools, notices, reports and the messenger relay (opscfg.rs)
    pub ops: crate::opscfg::OpsRules,
    // DC-DEFAULT: GMD-2 (clarification, project decision) round time of a room whose mode file gives none:
    // `room.round_secs.<mode class>`, `room.round_secs.default`, `room.round_secs.fallback` (false = DSV-20 time=0)
    pub round_clock: dc_bute::RoundTimePolicy,
    // HiSec case delivery, key, award table and announcement (hisec.rs: HS-6..HS-9)
    pub hisec: crate::hisec::HisecRules,
    // DC-DEFAULT: MG-1 (staged, project decision pending) `modes.playable`: the mode names the lobby offers and lets
    // start; None = the built-in list of lobby.rs (installed with `lobby::set_playable` at start-up)
    pub modes_playable: Option<Vec<String>>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            exp: RewardRule {
                finish: 15.0,
                kill: 6.0,
                assist: 0.0,
                win_bonus_pct: 25.0,
                objective: 22.0,
                agent_win: 45.0,
            },
            gp: RewardRule {
                finish: 20.0,
                kill: 6.0,
                assist: 0.0,
                win_bonus_pct: 25.0,
                objective: 25.0,
                agent_win: 0.0,
            },
            expiry_scan_secs: 60,
            dserver_port_lo: 29901,
            dserver_port_hi: 29999,
            names_case_insensitive: true,
            session_ttl_secs: 60,
            channels: 3,
            countdown_ticks: 5,
            allow_late_join: true,
            max_rooms_per_channel: 100,
            starter_items: Vec::new(),
            default_mid_join: 1,
            dserver_quit_grace_secs: 5,
            starter_backpack: true,
            entry_team_balance: true,
            default_repair: true,
            default_repair_backpack: false,
            period_activation: true,
            trust_server_rewards: false,
            nx_enabled: true,
            nx_daily: 250,
            nx_clan_win: 200,
            nx_clan_win_daily_cap: 3,
            nx_clan_min_players: 2,
            nx_clan_min_seconds: 180,
            pay_leavers: false,
            bot_pool_size: 100,
            bot_pool_per_ip: 7,
            bot_keep_seat_online: 8,
            bot_pool_kit: "auto".into(),
            rank_gifts: Vec::new(),
            rank_unlock_days: 7,
            rank_case_products: Vec::new(),
            rank_bonus: true,
            single_session: true,
            max_players_cap: 16,
            handover_any_state: true,
            dserver_loadout_tokens: true,
            send_gain_lines: true,
            settings_change_clears_ready: true,
            kick_needs_badge: true,
            first_account_no: 1001,
            modes_playable: None,
            shop: Default::default(),
            clan: Default::default(),
            ops: Default::default(),
            round_clock: Default::default(),
            hisec: Default::default(),
        }
    }
}

fn num(v: &JsonValue) -> Option<f64> {
    match v {
        JsonValue::Number(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}

/// One `rewards.rank_gifts` entry: `{"rank": n, "products": [id, ...]}` (rank 1..=255, ids above 0).
fn rank_gift(e: &JsonValue) -> Option<(u32, Vec<i32>)> {
    let JsonValue::Object(m) = e else {
        return None;
    };
    let rank = num(m.get("rank")?).filter(|r| (1.0..=255.0).contains(r))? as u32;
    let JsonValue::Array(a) = m.get("products")? else {
        return None;
    };
    let products: Option<Vec<i32>> = a
        .iter()
        .map(|p| {
            num(p)
                .filter(|x| *x >= 1.0 && *x <= i32::MAX as f64)
                .map(|x| x as i32)
        })
        .collect();
    Some((rank, products?))
}

impl Rules {
    /// Applies one override; returns false for an unknown key or a value of the wrong type.
    pub fn apply(&mut self, key: &str, v: &JsonValue) -> bool {
        let n = num(v);
        let b = match v {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        };
        let set_f = |dst: &mut f64| n.map(|x| *dst = x).is_some();
        match key {
            "economy.exp.finish" => set_f(&mut self.exp.finish),
            "economy.exp.kill" => set_f(&mut self.exp.kill),
            "economy.exp.assist" => set_f(&mut self.exp.assist),
            "economy.exp.win_bonus_pct" => set_f(&mut self.exp.win_bonus_pct),
            "economy.exp.objective" => set_f(&mut self.exp.objective),
            "economy.exp.agent_win" => set_f(&mut self.exp.agent_win),
            "economy.gp.finish" => set_f(&mut self.gp.finish),
            "economy.gp.kill" => set_f(&mut self.gp.kill),
            "economy.gp.assist" => set_f(&mut self.gp.assist),
            "economy.gp.win_bonus_pct" => set_f(&mut self.gp.win_bonus_pct),
            "economy.gp.objective" => set_f(&mut self.gp.objective),
            "economy.gp.agent_win" => set_f(&mut self.gp.agent_win),
            "inventory.expiry_scan_secs" => n
                .map(|x| self.expiry_scan_secs = x.max(1.0) as u64)
                .is_some(),
            "dserver.port_range" => match v {
                JsonValue::Array(a) if a.len() == 2 => match (num(&a[0]), num(&a[1])) {
                    (Some(lo), Some(hi)) if lo >= 1.0 && hi <= 65535.0 && lo <= hi => {
                        self.dserver_port_lo = lo as u16;
                        self.dserver_port_hi = hi as u16;
                        true
                    }
                    _ => false,
                },
                _ => false,
            },
            "names.case_insensitive" => b.map(|x| self.names_case_insensitive = x).is_some(),
            "auth.session_ttl_secs" => n
                .map(|x| self.session_ttl_secs = x.max(1.0) as u64)
                .is_some(),
            "lobby.channels" => n
                .map(|x| self.channels = x.clamp(1.0, 64.0) as u32)
                .is_some(),
            "room.countdown_ticks" => n
                .map(|x| self.countdown_ticks = x.clamp(0.0, 5.0) as u32)
                .is_some(),
            "room.allow_late_join" => b.map(|x| self.allow_late_join = x).is_some(),
            "room.max_per_channel" => n
                .map(|x| self.max_rooms_per_channel = x.clamp(1.0, 1000.0) as u32)
                .is_some(),
            "room.default_mid_join" => n
                .map(|x| self.default_mid_join = (x != 0.0) as u32)
                .is_some(),
            "dserver.quit_grace_secs" => n
                .map(|x| self.dserver_quit_grace_secs = x.max(0.0) as u64)
                .is_some(),
            "inventory.starter_backpack" => b.map(|x| self.starter_backpack = x).is_some(),
            "room.entry_team_balance" => b.map(|x| self.entry_team_balance = x).is_some(),
            "inventory.default_repair" => match v {
                JsonValue::String(s) if s == "weapons" || s == "all" || s == "off" => {
                    self.default_repair = s != "off";
                    self.default_repair_backpack = s == "all";
                    true
                }
                _ => b.map(|x| self.default_repair = x).is_some(),
            },
            "inventory.period_activation" => b.map(|x| self.period_activation = x).is_some(),
            "economy.trust_server_rewards" => b.map(|x| self.trust_server_rewards = x).is_some(),
            "rewards.pay_leavers" => b.map(|x| self.pay_leavers = x).is_some(),
            "bots.pool_size" => n
                .filter(|x| (0.0..=10_000.0).contains(x))
                .map(|x| self.bot_pool_size = x as u32)
                .is_some(),
            "bots.pool_per_ip" => n
                .filter(|x| (1.0..=10_000.0).contains(x))
                .map(|x| self.bot_pool_per_ip = x as u32)
                .is_some(),
            "bots.keep_seat_online_players" => n
                .filter(|x| (0.0..=100_000.0).contains(x))
                .map(|x| self.bot_keep_seat_online = x as u32)
                .is_some(),
            "bots.pool_kit" => match v {
                JsonValue::String(s) => {
                    let t = s.trim().to_ascii_lowercase();
                    let kit = if t == "auto" || t == "none" {
                        Ok(t)
                    } else {
                        crate::botgroups::clean_loadout(s)
                    };
                    kit.map(|k| self.bot_pool_kit = k).is_ok()
                }
                _ => false,
            },
            "rewards.nx_enabled" => b.map(|x| self.nx_enabled = x).is_some(),
            "rewards.nx_daily" => n
                .filter(|x| (0.0..=1_000_000.0).contains(x))
                .map(|x| self.nx_daily = x as i64)
                .is_some(),
            "rewards.nx_clan_win" => n
                .filter(|x| (0.0..=1_000_000.0).contains(x))
                .map(|x| self.nx_clan_win = x as i64)
                .is_some(),
            "rewards.nx_clan_win_daily_cap" => n
                .filter(|x| (1.0..=100.0).contains(x))
                .map(|x| self.nx_clan_win_daily_cap = x as u32)
                .is_some(),
            "rewards.nx_clan_min_players" => n
                .filter(|x| (1.0..=8.0).contains(x))
                .map(|x| self.nx_clan_min_players = x as u32)
                .is_some(),
            "rewards.nx_clan_min_seconds" => n
                .filter(|x| (0.0..=3600.0).contains(x))
                .map(|x| self.nx_clan_min_seconds = x as u32)
                .is_some(),
            "rewards.rank_gifts" => match v {
                JsonValue::Array(a) => {
                    let gifts: Option<Vec<(u32, Vec<i32>)>> = a.iter().map(rank_gift).collect();
                    gifts.map(|g| self.rank_gifts = g).is_some()
                }
                _ => false,
            },
            "economy.rank_bonus" => b.map(|x| self.rank_bonus = x).is_some(),
            "rewards.rank_unlock_days" => n
                .filter(|x| (1.0..=3650.0).contains(x))
                .map(|x| self.rank_unlock_days = x as u32)
                .is_some(),
            "rewards.rank_case_products" => match v {
                JsonValue::Array(a) => {
                    let ids: Option<Vec<i32>> = a
                        .iter()
                        .map(|x| {
                            num(x)
                                .filter(|f| *f >= 1.0 && *f <= i32::MAX as f64)
                                .map(|f| f as i32)
                        })
                        .collect();
                    ids.map(|i| self.rank_case_products = i).is_some()
                }
                _ => false,
            },
            "lobby.single_session" => b.map(|x| self.single_session = x).is_some(),
            "room.max_players_cap" => n
                .map(|x| self.max_players_cap = x.clamp(1.0, 64.0) as u32)
                .is_some(),
            "room.handover_any_state" => b.map(|x| self.handover_any_state = x).is_some(),
            "dserver.loadout_tokens" => b.map(|x| self.dserver_loadout_tokens = x).is_some(),
            "dserver.send_gain_lines" => b.map(|x| self.send_gain_lines = x).is_some(),
            "room.settings_change_clears_ready" => {
                b.map(|x| self.settings_change_clears_ready = x).is_some()
            }
            "room.kick_needs_badge" => b.map(|x| self.kick_needs_badge = x).is_some(),
            "auth.first_account_no" => n
                .map(|x| self.first_account_no = x.clamp(1.0, 4_000_000_000.0) as u32)
                .is_some(),
            "room.round_secs.default" => n
                .filter(|x| *x >= 0.0)
                .map(|x| self.round_clock.default_secs = x.min(86_400.0) as i32)
                .is_some(),
            "room.round_secs.fallback" => b
                .map(|on| {
                    if !on {
                        self.round_clock = dc_bute::RoundTimePolicy::none();
                    } else if self.round_clock == dc_bute::RoundTimePolicy::none() {
                        self.round_clock = dc_bute::RoundTimePolicy::default();
                    }
                })
                .is_some(),
            k if k.starts_with("room.round_secs.") => {
                match (k["room.round_secs.".len()..].parse::<i32>(), n) {
                    (Ok(class), Some(x)) if (0..=255).contains(&class) && x >= 0.0 => {
                        self.round_clock.set(class, x.min(86_400.0) as i32);
                        true
                    }
                    _ => false,
                }
            }
            k if crate::shopcfg::ShopRules::owns(k) => self.shop.apply(k, v),
            k if crate::clan::ClanRules::owns(k) => self.clan.apply(k, v),
            k if crate::opscfg::OpsRules::owns(k) => self.ops.apply(k, v),
            k if crate::hisec::HisecRules::owns(k) => self.hisec.apply(k, v),
            "modes.playable" => match v {
                JsonValue::Array(a) => {
                    let names: Option<Vec<String>> = a
                        .iter()
                        .map(|x| match x {
                            JsonValue::String(s) => Some(s.clone()),
                            _ => None,
                        })
                        .collect();
                    names.map(|n| self.modes_playable = Some(n)).is_some()
                }
                _ => false,
            },
            "inventory.starter" => match v {
                JsonValue::Array(a) => {
                    let items: Option<Vec<i32>> =
                        a.iter().map(|x| num(x).map(|f| f as i32)).collect();
                    items.map(|i| self.starter_items = i).is_some()
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Reads `<dir>/lobby.json` when present. Returns the rules and the problems found (reported by the caller).
    pub fn load(dir: &Path) -> (Self, Vec<String>) {
        let mut r = Self::default();
        let mut notes = Vec::new();
        let path = dir.join("lobby.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (r, notes);
        };
        match JsonValue::parse(&text) {
            Ok(JsonValue::Object(map)) => {
                for (k, v) in &map {
                    if !r.apply(k, v) {
                        notes.push(format!(
                            "{}: ignored key or bad value '{}'",
                            path.display(),
                            k
                        ));
                    }
                }
            }
            Ok(_) => notes.push(format!("{}: top level must be an object", path.display())),
            Err(e) => notes.push(format!("{}: {}", path.display(), e)),
        }
        (r, notes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_apply_and_unknown_keys_are_reported() {
        let dir = std::env::temp_dir().join(format!("ca_lobby_cfg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("lobby.json"),
            r#"{"economy.exp.kill": 7, "room.countdown_ticks": 3, "dserver.port_range": [30000, 30010],
                "inventory.starter": [1, 2], "bogus": 1, "room.allow_late_join": false}"#,
        )
        .unwrap();
        let (r, notes) = Rules::load(&dir);
        assert_eq!(r.exp.kill, 7.0);
        assert_eq!(r.countdown_ticks, 3);
        assert_eq!((r.dserver_port_lo, r.dserver_port_hi), (30000, 30010));
        assert_eq!(r.starter_items, vec![1, 2]);
        assert!(!r.allow_late_join);
        assert_eq!(notes.len(), 1, "{notes:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // DC-DEFAULT: GMD-2 (clarification) the round-time fallback keys
    #[test]
    fn round_time_fallback_keys() {
        let mut r = Rules::default();
        assert_eq!(r.round_clock.secs_for(1), 600);
        assert_eq!(r.round_clock.secs_for(12), 150);
        assert_eq!(r.round_clock.secs_for(5), 0);
        assert!(r.apply("room.round_secs.1", &JsonValue::Number(900.0)));
        assert!(r.apply("room.round_secs.default", &JsonValue::Number(120.0)));
        assert_eq!(
            (r.round_clock.secs_for(1), r.round_clock.secs_for(99)),
            (900, 120)
        );
        assert!(!r.apply("room.round_secs.x", &JsonValue::Number(1.0)));
        assert!(!r.apply("room.round_secs.1", &JsonValue::Number(-1.0)));
        assert!(!r.apply("room.round_secs.300", &JsonValue::Number(1.0)));
        assert!(r.apply("room.round_secs.fallback", &JsonValue::Bool(false)));
        assert_eq!(
            (r.round_clock.secs_for(1), r.round_clock.secs_for(4)),
            (0, 0)
        );
        assert!(r.apply("room.round_secs.fallback", &JsonValue::Bool(true)));
        assert_eq!(r.round_clock.secs_for(1), 600);
    }

    #[test]
    fn defaults_match_cak_defaults_md() {
        let r = Rules::default();
        assert_eq!(
            r.exp,
            RewardRule {
                finish: 15.0,
                kill: 6.0,
                assist: 0.0,
                win_bonus_pct: 25.0,
                objective: 22.0,
                agent_win: 45.0,
            }
        );
        assert_eq!(
            (r.gp.finish, r.gp.kill, r.gp.win_bonus_pct, r.gp.objective),
            (20.0, 6.0, 25.0, 25.0)
        );
        assert_eq!(
            (r.countdown_ticks, r.max_rooms_per_channel, r.channels),
            (5, 100, 3)
        );
        assert_eq!(
            (
                r.dserver_port_lo,
                r.dserver_port_hi,
                r.expiry_scan_secs,
                r.session_ttl_secs
            ),
            (29901, 29999, 60, 60)
        );
    }
}
