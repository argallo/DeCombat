//! `server.toml` layer of the lobby (SPEC: HOSTING_AND_MODS.md §2: one `server.toml` plus env overrides).
//!
//! Precedence, highest first: command line flag, `DC_*` environment, `server.toml`, `lobby.json`, built-in
//! default. The layer is active only when `--config <file>` or `DC_CONFIG` names a file, or when at least one
//! `DC_<SECTION>__<KEY>` variable is set; otherwise nothing here runs and the lobby behaves exactly as before.
//!
//! A `Config` cannot tell "unset" from "set to the default", so every value is taken only when the operator
//! set its path in the file or the environment (`dc_config::Explicit`). Values the operator left out keep
//! whatever the lower layers (`lobby.json`, then the built-in default) say.

use crate::config::Rules;
use dc_config::{CheckOptions, CliOverrides, Config, Explicit, Issue};
use dc_mod_lua::{JsonValue, ModPolicy};
use std::path::{Path, PathBuf};

/// The loaded `server.toml` layer.
#[derive(Clone, Debug)]
pub struct ServerConf {
    /// The effective config (defaults < file < environment).
    pub cfg: Config,
    /// Paths the file or the environment set.
    pub ex: Explicit,
    /// The file, if one was given (an environment-only layer has none).
    pub path: Option<PathBuf>,
    /// Non-fatal findings of `dc_config::check` (set by [`ServerConf::load`]).
    pub warnings: Vec<Issue>,
}

/// Name of the variable that names the file when `--config` is absent.
pub const ENV_CONFIG: &str = "DC_CONFIG";

fn num(x: f64) -> JsonValue {
    JsonValue::Number(x)
}

fn list<T: Copy + Into<f64>>(v: &[T]) -> JsonValue {
    JsonValue::Array(v.iter().map(|x| num((*x).into())).collect())
}

fn text(s: &str) -> JsonValue {
    JsonValue::String(s.to_string())
}

impl ServerConf {
    /// Loads the layer. `flag` is the `--config` value, `env` the process environment.
    /// `Ok(None)`: no file and no `DC_*__*` variable, the layer is off.
    /// `Err`: unreadable file, parse error, bad override or a `dc_config::check` error.
    pub fn load(
        flag: Option<&Path>,
        env: &[(String, String)],
    ) -> Result<Option<ServerConf>, Vec<String>> {
        let Some(mut conf) = Self::load_unchecked(flag, env)? else {
            return Ok(None);
        };
        let issues = conf.check(false);
        let errors: Vec<String> = issues
            .iter()
            .filter(|i| i.is_error())
            .map(|i| i.to_string())
            .collect();
        if !errors.is_empty() {
            return Err(errors);
        }
        conf.warnings = issues;
        Ok(Some(conf))
    }

    /// [`ServerConf::load`] without the semantic check (parse and override errors still fail); `doctor` reports
    /// the check findings itself.
    pub fn load_unchecked(
        flag: Option<&Path>,
        env: &[(String, String)],
    ) -> Result<Option<ServerConf>, Vec<String>> {
        let env_file = env
            .iter()
            .find(|(k, v)| k == ENV_CONFIG && !v.trim().is_empty())
            .map(|(_, v)| PathBuf::from(v.trim()));
        let path = flag.map(Path::to_path_buf).or(env_file);
        let has_env = env.iter().any(|(k, _)| {
            k.starts_with(dc_config::ENV_PREFIX) && k[dc_config::ENV_PREFIX.len()..].contains("__")
        });
        if path.is_none() && !has_env {
            return Ok(None);
        }
        let text = match &path {
            Some(p) => Some(
                std::fs::read_to_string(p)
                    .map_err(|e| vec![format!("cannot read {}: {}", p.display(), e)])?,
            ),
            None => None,
        };
        let (cfg, ex) = dc_config::load_layered_explicit(
            text.as_deref(),
            env.iter().cloned(),
            &CliOverrides::default(),
        )
        .map_err(|e| e.iter().map(|i| i.to_string()).collect::<Vec<_>>())?;
        Ok(Some(ServerConf {
            cfg,
            ex,
            path,
            warnings: Vec::new(),
        }))
    }

    /// `dc_config::check` of the effective config; `fs` also looks at the ROM folder, mods zips and DB directory.
    pub fn check(&self, fs: bool) -> Vec<Issue> {
        dc_config::check(&self.cfg, CheckOptions { fs })
    }

    /// The mod policy for `ModHost::set_policy` (vanilla = every load is refused).
    pub fn mod_policy(&self) -> ModPolicy {
        if self.cfg.mods.policy == "modded" {
            ModPolicy::Modded {
                client_visual_allowed: self.cfg.mods.client_visual_allowed,
            }
        } else {
            ModPolicy::Vanilla
        }
    }

    /// The `Rules::apply` keys (the `lobby.json` spelling) this file or the environment set, with their values.
    /// The test `every_schema_key_is_mapped_or_listed` keeps this in step with the schema.
    pub fn rule_overrides(&self) -> Vec<(String, JsonValue)> {
        let c = &self.cfg;
        let mut o: Vec<(String, JsonValue)> = Vec::new();
        let mut put = |path: &str, key: &str, v: JsonValue| {
            if self.ex.has(path) {
                o.push((key.to_string(), v));
            }
        };
        // rewards (CD-03, CD-04)
        for (sec, r) in [("exp", &c.economy.exp), ("gp", &c.economy.gp)] {
            put(
                &format!("economy.{sec}.finish"),
                &format!("economy.{sec}.finish"),
                num(r.finish.into()),
            );
            put(
                &format!("economy.{sec}.kill"),
                &format!("economy.{sec}.kill"),
                num(r.kill.into()),
            );
            put(
                &format!("economy.{sec}.assist"),
                &format!("economy.{sec}.assist"),
                num(r.assist.into()),
            );
            put(
                &format!("economy.{sec}.win_percent"),
                &format!("economy.{sec}.win_bonus_pct"),
                num(r.win_percent.into()),
            );
            put(
                &format!("economy.{sec}.objective"),
                &format!("economy.{sec}.objective"),
                num(r.objective.into()),
            );
            put(
                &format!("economy.{sec}.agent_win"),
                &format!("economy.{sec}.agent_win"),
                num(r.agent_win.into()),
            );
        }
        put(
            "economy.trust_server_rewards",
            "economy.trust_server_rewards",
            JsonValue::Bool(c.economy.trust_server_rewards),
        );
        put(
            "economy.case_odds",
            "economy.case_odds",
            JsonValue::Array(c.economy.case_odds.iter().map(|p| list(&p[..])).collect()),
        );
        put(
            "economy.case_split_on_purchase",
            "economy.case_split_on_purchase",
            JsonValue::Bool(c.economy.case_split_on_purchase),
        );
        put(
            "economy.gp_case_open_on_buy",
            "economy.gp_case_open_on_buy",
            JsonValue::Bool(c.economy.gp_case_open_on_buy),
        );
        // auth, names, lobby, rooms
        put(
            "auth.session_ttl_secs",
            "auth.session_ttl_secs",
            num(c.auth.session_ttl_secs.into()),
        );
        put(
            "names.case_insensitive",
            "names.case_insensitive",
            JsonValue::Bool(c.names.case_insensitive),
        );
        if !c.lobby.channels.is_empty() {
            put(
                "lobby.channels",
                "lobby.channels",
                num(c.lobby.channels.len() as f64),
            );
        }
        put(
            "lobby.single_session",
            "lobby.single_session",
            JsonValue::Bool(c.lobby.single_session),
        );
        put(
            "room.countdown_ticks",
            "room.countdown_ticks",
            num(c.room.countdown_ticks.into()),
        );
        put(
            "room.allow_late_join",
            "room.allow_late_join",
            JsonValue::Bool(c.room.allow_late_join),
        );
        put(
            "room.max_per_channel",
            "room.max_per_channel",
            num(c.room.max_per_channel.into()),
        );
        put(
            "room.default_mid_join",
            "room.default_mid_join",
            num(c.room.default_mid_join.into()),
        );
        put(
            "room.entry_team_balance",
            "room.entry_team_balance",
            JsonValue::Bool(c.room.entry_team_balance),
        );
        put(
            "room.max_players_cap",
            "room.max_players_cap",
            num(c.room.max_players_cap.into()),
        );
        // RM-1, AUTH-1 (proposed)
        put(
            "room.settings_change_clears_ready",
            "room.settings_change_clears_ready",
            JsonValue::Bool(c.room.settings_change_clears_ready),
        );
        put(
            "auth.first_account_no",
            "auth.first_account_no",
            num(c.auth.first_account_no.into()),
        );
        put(
            "room.handover_any_state",
            "room.handover_any_state",
            JsonValue::Bool(c.room.handover_any_state),
        );
        // game servers
        if let Ok((lo, hi)) = c.dserver_range() {
            put("dserver.port_range", "dserver.port_range", list(&[lo, hi]));
        }
        put(
            "dserver.quit_grace_secs",
            "dserver.quit_grace_secs",
            num(c.dserver.quit_grace_secs.into()),
        );
        // LDO-1, DSV-35 (proposed): the same keys as lobby.json; server.toml / env win when set
        put(
            "dserver.loadout_tokens",
            "dserver.loadout_tokens",
            JsonValue::Bool(c.dserver.loadout_tokens),
        );
        put(
            "dserver.send_gain_lines",
            "dserver.send_gain_lines",
            JsonValue::Bool(c.dserver.send_gain_lines),
        );
        // MG-1 (staged): the playable-mode gate (lobby.rs ModePolicy)
        put(
            "modes.playable",
            "modes.playable",
            JsonValue::Array(c.modes.playable.iter().map(|n| text(n)).collect()),
        );
        // inventory
        put(
            "inventory.expiry_scan_secs",
            "inventory.expiry_scan_secs",
            num(c.inventory.expiry_scan_secs.into()),
        );
        put(
            "inventory.starter_items",
            "inventory.starter",
            list(&c.inventory.starter_items),
        );
        put(
            "inventory.starter_backpack",
            "inventory.starter_backpack",
            JsonValue::Bool(c.inventory.starter_backpack),
        );
        put(
            "inventory.default_repair",
            "inventory.default_repair",
            text(&c.inventory.default_repair),
        );
        put(
            "inventory.period_activation",
            "inventory.period_activation",
            JsonValue::Bool(c.inventory.period_activation),
        );
        put(
            "inventory.max_items",
            "inventory.max_items",
            num(c.inventory.max_items.into()),
        );
        put(
            "inventory.backpack_kinds",
            "inventory.backpack_kinds",
            list(&c.inventory.backpack_kinds),
        );
        put(
            "inbox.max_rows",
            "inbox.max_rows",
            num(c.inbox.max_rows.into()),
        );
        // shop and megaphone
        let s = &c.shop;
        put("shop.new_days", "shop.new_days", num(s.new_days.into()));
        put(
            "shop.hot_percent",
            "shop.hot_percent",
            num(s.hot_percent.into()),
        );
        put("shop.seasonal", "shop.seasonal", JsonValue::Bool(s.seasonal));
        put("shop.sale_list", "shop.sale_list", list(&s.sale_list));
        let mut lists = std::collections::BTreeMap::new();
        for (name, ids) in [
            ("view", &s.list_override.view),
            ("hot", &s.list_override.hot),
            ("new", &s.list_override.new),
            ("sale", &s.list_override.sale),
        ] {
            if self.ex.has(&format!("shop.list_override.{name}")) {
                lists.insert(name.to_string(), list(ids));
            }
        }
        if !lists.is_empty() {
            o.push(("shop.lists".to_string(), JsonValue::Object(lists)));
        }
        let mut put = |path: &str, key: &str, v: JsonValue| {
            if self.ex.has(path) {
                o.push((key.to_string(), v));
            }
        };
        put(
            "shop.credit_to_inbox",
            "shop.credit_to_inbox",
            JsonValue::Bool(s.credit_to_inbox),
        );
        put(
            "shop.price_override",
            "shop.price_override",
            JsonValue::Object(
                s.price_override
                    .iter()
                    .map(|(k, v)| (k.clone(), num((*v).into())))
                    .collect(),
            ),
        );
        put("shop.extend", "shop.extend", JsonValue::Bool(s.extend));
        put(
            "shop.coupons",
            "shop.coupons",
            JsonValue::Array(
                s.coupons
                    .iter()
                    .map(|(code, product)| {
                        let mut m = std::collections::BTreeMap::new();
                        m.insert("code".to_string(), text(code));
                        m.insert("product".to_string(), num((*product).into()));
                        if let Some(u) = s.coupon_uses.get(code) {
                            m.insert("uses".to_string(), num(*u as f64));
                        }
                        JsonValue::Object(m)
                    })
                    .collect(),
            ),
        );
        put(
            "shop.notify_offline_receiver",
            "shop.notify_offline_receiver",
            JsonValue::Bool(s.notify_offline_receiver),
        );
        put(
            "shop.default_primary_item",
            "shop.default_primary_item",
            num(s.default_primary_item.into()),
        );
        put(
            "shop.default_secondary_item",
            "shop.default_secondary_item",
            num(s.default_secondary_item.into()),
        );
        put(
            "rewards.pay_leavers",
            "rewards.pay_leavers",
            JsonValue::Bool(c.rewards.pay_leavers),
        );
        put(
            "bots.pool_size",
            "bots.pool_size",
            JsonValue::Number(c.bots.pool_size.into()),
        );
        put(
            "bots.pool_per_ip",
            "bots.pool_per_ip",
            JsonValue::Number(c.bots.pool_per_ip.into()),
        );
        put(
            "bots.keep_seat_online_players",
            "bots.keep_seat_online_players",
            JsonValue::Number(c.bots.keep_seat_online_players.into()),
        );
        put(
            "rewards.nx_enabled",
            "rewards.nx_enabled",
            JsonValue::Bool(c.rewards.nx_enabled),
        );
        put(
            "rewards.nx_daily",
            "rewards.nx_daily",
            num(c.rewards.nx_daily.into()),
        );
        put(
            "rewards.nx_clan_win",
            "rewards.nx_clan_win",
            num(c.rewards.nx_clan_win.into()),
        );
        put(
            "rewards.nx_clan_win_daily_cap",
            "rewards.nx_clan_win_daily_cap",
            num(c.rewards.nx_clan_win_daily_cap.into()),
        );
        put(
            "rewards.nx_clan_min_players",
            "rewards.nx_clan_min_players",
            num(c.rewards.nx_clan_min_players.into()),
        );
        put(
            "rewards.nx_clan_min_seconds",
            "rewards.nx_clan_min_seconds",
            num(c.rewards.nx_clan_min_seconds.into()),
        );
        put(
            "rewards.rank_gifts",
            "rewards.rank_gifts",
            JsonValue::Array(
                c.rewards
                    .rank_gifts
                    .iter()
                    .map(|g| {
                        let mut m = std::collections::BTreeMap::new();
                        m.insert("rank".to_string(), num(g.rank.into()));
                        m.insert("products".to_string(), list(&g.products));
                        JsonValue::Object(m)
                    })
                    .collect(),
            ),
        );
        put(
            "economy.rank_bonus",
            "economy.rank_bonus",
            JsonValue::Bool(c.economy.rank_bonus),
        );
        put(
            "rewards.rank_unlock_days",
            "rewards.rank_unlock_days",
            num(c.rewards.rank_unlock_days.into()),
        );
        put(
            "rewards.rank_case_products",
            "rewards.rank_case_products",
            list(&c.rewards.rank_case_products),
        );
        put(
            "megaphone.secs",
            "megaphone.secs",
            num(c.megaphone.secs.into()),
        );
        put(
            "megaphone.max_chars",
            "megaphone.max_chars",
            num(c.megaphone.max_chars.into()),
        );
        // GM tools, notices, reports, messenger
        let g = &c.gm;
        put(
            "gm.test_commands",
            "gm.test_commands",
            text(&g.test_commands),
        );
        put(
            "gm.default_mute_minutes",
            "gm.default_mute_minutes",
            num(g.default_mute_minutes.into()),
        );
        put(
            "gm.max_mute_minutes",
            "gm.max_mute_minutes",
            num(g.max_mute_minutes.into()),
        );
        put(
            "gm.permanent_ban_days",
            "gm.permanent_ban_days",
            num(g.permanent_ban_days.into()),
        );
        put(
            "gm.lookup_shows_address",
            "gm.lookup_shows_address",
            JsonValue::Bool(g.lookup_shows_address),
        );
        put(
            "gm.flag_moderators_to_game",
            "gm.flag_moderators_to_game",
            JsonValue::Bool(g.flag_moderators_to_game),
        );
        for (k, v) in [
            ("gm.admin_max_nx", g.admin_max_nx),
            ("gm.admin_max_gp", g.admin_max_gp),
            ("gm.admin_max_exp", g.admin_max_exp),
        ] {
            put(k, k, num(v.into()));
        }
        let n = &c.notice;
        put(
            "notice.ticker_period_secs",
            "notice.ticker_period_secs",
            num(n.ticker_period_secs.into()),
        );
        put(
            "notice.ticker_repeat",
            "notice.ticker_repeat",
            num(n.ticker_repeat.into()),
        );
        put(
            "notice.case_announce",
            "notice.case_announce",
            text(&n.case_announce),
        );
        put(
            "notice.login_ticker",
            "notice.login_ticker",
            text(&n.login_ticker),
        );
        put("notice.login_box", "notice.login_box", text(&n.login_box));
        put(
            "notice.expiry_boxes",
            "notice.expiry_boxes",
            JsonValue::Bool(n.expiry_boxes),
        );
        put(
            "reports.per_hour",
            "reports.per_hour",
            num(c.reports.per_hour.into()),
        );
        put(
            "monitor.retention_days",
            "monitor.retention_days",
            num(c.monitor.retention_days.into()),
        );
        put(
            "messenger.max_friends",
            "messenger.max_friends",
            num(c.messenger.max_friends.into()),
        );
        put(
            "messenger.whisper_friends_only",
            "messenger.whisper_friends_only",
            JsonValue::Bool(c.messenger.whisper_friends_only),
        );
        // clans
        let cl = &c.clan;
        put(
            "clan.channel_members_only",
            "clan.channel_members_only",
            JsonValue::Bool(cl.channel_members_only),
        );
        put(
            "clan.start_min_per_side",
            "clan.start_min_per_side",
            num(cl.start_min_per_side.into()),
        );
        put(
            "clan.level_steps",
            "clan.level_steps",
            JsonValue::Array(cl.level_steps.iter().map(|x| num(*x as f64)).collect()),
        );
        put(
            "clan.member_limits",
            "clan.member_limits",
            list(&cl.member_limits),
        );
        put(
            "clan.web_requires_session",
            "clan.web_requires_session",
            JsonValue::Bool(cl.web_requires_session),
        );
        put(
            "clan.chat_commands",
            "clan.chat_commands",
            JsonValue::Bool(cl.chat_commands),
        );
        put(
            "clan.basic_point_default",
            "clan.basic_point_default",
            num(cl.basic_point_default.into()),
        );
        put(
            "clan.winner_point_default",
            "clan.winner_point_default",
            num(cl.winner_point_default.into()),
        );
        for (mode, w) in &cl.mode_weight {
            put(
                &format!("clan.mode_weight.{mode}"),
                &format!("clan.mode_weight.{mode}"),
                num(*w),
            );
        }
        // GMD-2 (project decision): the switch first (it resets the table), then the default, then the
        // classes, so that a file's class entries survive its own `fallback = true`
        let rs = &c.room.round_secs;
        put(
            "room.round_secs.fallback",
            "room.round_secs.fallback",
            JsonValue::Bool(rs.fallback),
        );
        put(
            "room.round_secs.default",
            "room.round_secs.default",
            num(rs.default.into()),
        );
        for (class, secs) in &rs.classes {
            put(
                &format!("room.round_secs.{class}"),
                &format!("room.round_secs.{class}"),
                num((*secs).into()),
            );
        }
        // HS-6..HS-9 (proposed): the HiSec lobby keys
        put(
            "economy.hisec_case_odds",
            "economy.hisec_case_odds",
            JsonValue::Array(
                c.economy
                    .hisec_case_odds
                    .iter()
                    .map(|p| list(&p[..]))
                    .collect(),
            ),
        );
        put(
            "hisec.case_product",
            "hisec.case_product",
            num(c.hisec.case_product.into()),
        );
        put("hisec.announce", "hisec.announce", text(&c.hisec.announce));
        put(
            "hisec.no_key_reason",
            "hisec.no_key_reason",
            num(c.hisec.no_key_reason.into()),
        );
        o
    }

    /// Applies [`ServerConf::rule_overrides`] on top of `rules` (which already hold `lobby.json`). Returns one
    /// note per value the rules refused.
    pub fn apply_rules(&self, rules: &mut Rules) -> Vec<String> {
        let mut notes = Vec::new();
        for (k, v) in self.rule_overrides() {
            if !rules.apply(&k, &v) {
                notes.push(format!(
                    "server.toml: key '{k}' refused (bad value or shape)"
                ));
            }
        }
        notes
    }

    /// Installs the playable-mode list of `rules` (lobby.json, then server.toml / env) as the lobby's gate; returns
    /// one log line per name that names no mode (those names are skipped).
    pub fn install_playable(rules: &Rules) -> Vec<String> {
        let Some(names) = &rules.modes_playable else {
            return Vec::new();
        };
        crate::lobby::set_playable(names)
            .into_iter()
            .map(|n| format!("modes.playable: unknown mode name '{n}' skipped"))
            .collect()
    }

    /// Where the file's own relative paths resolve from: the process working directory, like the flags.
    pub fn describe(&self) -> String {
        match &self.path {
            Some(p) => format!("server.toml {}", p.display()),
            None => "DC_* environment".to_string(),
        }
    }
}

/// The ROM folder: `CA_ROM`, else `paths.rom` when the operator set it.
pub fn rom_dir(conf: Option<&ServerConf>) -> Option<PathBuf> {
    if let Ok(d) = dc_rom::resolve_rom() {
        return Some(d);
    }
    let c = conf.filter(|c| c.ex.has("paths.rom") && !c.cfg.paths.rom.is_empty())?;
    let p = PathBuf::from(&c.cfg.paths.rom);
    p.is_dir().then_some(p)
}

/// The mods folder in use: the `--mods-dir` / `paths.mods_dir` value, else `paths.mods_dir` of a modded config.
pub fn mods_dir(opts: &crate::cli::Options, conf: Option<&ServerConf>) -> Option<PathBuf> {
    opts.mods_dir.clone().or_else(|| {
        // LG-313: a vanilla lobby's featured entries keep their zips there too
        conf.filter(|c| c.cfg.mods.policy == "modded" || c.cfg.lobby.servers.iter().any(|e| !e.mods.is_empty()))
            .map(|c| PathBuf::from(&c.cfg.paths.mods_dir))
    })
}

/// Config paths that map to no lobby rule or option. Each has a reason; the schema test below requires every
/// other leaf under a lobby-owned section to be mapped.
#[cfg(test)]
pub const NOT_READ_BY_LOBBY: &[(&str, &str)] = &[
    (
        "room.blast_damage_scale",
        "the dserver reads it from its own config / DC_ROOM__BLAST_DAMAGE_SCALE (DSV-165)",
    ),
    (
        "spy.agent_health",
        "the dserver reads it from its own config / DC_SPY__AGENT_HEALTH (DSV-451)",
    ),
    (
        "spy.agent_armor",
        "the dserver reads it from its own config / DC_SPY__AGENT_ARMOR (DSV-451)",
    ),
    (
        "spy.agent_damage_scale",
        "the dserver reads it from its own config / DC_SPY__AGENT_DAMAGE_SCALE (DSV-456)",
    ),
    (
        "economy.exp.team_kill_penalty",
        "the lobby fixes a team kill at minus the kill value (CD-03)",
    ),
    (
        "economy.gp.team_kill_penalty",
        "the lobby fixes a team kill at minus the kill value (CD-04)",
    ),
    (
        "economy.ranks",
        "rank table override (CD-05) is not implemented yet",
    ),
    ("auth.file_check", "only 'off' exists (CD-13)"),
    (
        "names.referrer",
        "referrer check mode (CD-17) is not configurable yet",
    ),
    (
        "room.countdown_ms",
        "read as the --countdown option (seconds), see cli.rs",
    ),
    (
        "lobby.servers",
        "read as the servers option (the server list, each entry's host and channel block, CD-21 / CD-88); the first entry's capacity is the --server-max option and its name the server list's first name, see cli.rs",
    ),
    (
        "hosts",
        "read as the hosts_port / host_agents options (the host port and its agents, LG-300 / CD-88), see cli.rs",
    ),
    (
        "inventory.starter",
        "CD-18 is a switch in the schema; the item list is inventory.starter_items",
    ),
    (
        "shop.lists",
        "CD-35 mode word; the replacement lists are shop.list_override.*",
    ),
    (
        "gm.mute_server_side",
        "mute is always enforced by the server (CD-33)",
    ),
    ("clan.web_port", "read as the --clanweb option, see cli.rs"),
    ("bots.managed", "read as the bots_managed option (LG-388), see cli.rs"),
    (
        "bots.managed_max_total",
        "read as the bots_max_total option (LG-388), see cli.rs",
    ),
];

/// Sections whose leaves the lobby does not read (hosting keys of other processes or read through `Options`).
#[cfg(test)]
pub const OTHER_SECTIONS: &[&str] = &[
    "client",
    "server",
    "paths",
    "log",
    "metrics",
    "directory",
    "mods",
    "dserver",
    "combat",
    "vis",
    "evars",
    "beta",
    "admin_web",
    "deploy",
    "mail",
    "identity",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(prefix: &str, v: &toml::Value, out: &mut Vec<String>) {
        match v {
            toml::Value::Table(t) if !t.is_empty() => {
                for (k, c) in t {
                    let p = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    leaves(&p, c, out);
                }
            }
            _ => out.push(prefix.to_string()),
        }
    }

    fn all_set() -> ServerConf {
        // a file that sets every leaf of the default config (to its default) plus one mode weight
        let mut cfg = Config::default();
        cfg.clan.mode_weight.insert("TEAMDM".into(), 1.0);
        cfg.shop.coupons.insert("ABC".into(), 7);
        cfg.shop.price_override.insert("9".into(), 5);
        let text = cfg.to_toml();
        let ex = Explicit::from_toml(&text).unwrap();
        ServerConf {
            cfg,
            ex,
            path: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn config_wins_for_the_lobby_json_keys_of_ldo1_rm1_auth1_dsv35() {
        // DC-DEFAULT: LDO-1, RM-1, AUTH-1, DSV-35 (proposed)
        let text = "[room]
settings_change_clears_ready = false
[auth]
first_account_no = 7000
[dserver]
loadout_tokens = false
send_gain_lines = false
";
        let conf = ServerConf {
            cfg: dc_config::parse_toml(text).unwrap(),
            ex: Explicit::from_toml(text).unwrap(),
            path: None,
            warnings: Vec::new(),
        };
        // lobby.json said the opposite; the config layer is applied after it
        let mut r = Rules {
            dserver_loadout_tokens: true,
            send_gain_lines: true,
            settings_change_clears_ready: true,
            first_account_no: 5000,
            ..Rules::default()
        };
        assert!(conf.apply_rules(&mut r).is_empty());
        assert!(!r.dserver_loadout_tokens && !r.send_gain_lines && !r.settings_change_clears_ready);
        assert_eq!(r.first_account_no, 7000);
        // keys the file does not name leave lobby.json's values alone
        let none = ServerConf {
            cfg: Config::default(),
            ex: Explicit::default(),
            path: None,
            warnings: Vec::new(),
        };
        let mut r = Rules {
            first_account_no: 5000,
            send_gain_lines: false,
            ..Rules::default()
        };
        none.apply_rules(&mut r);
        assert_eq!((r.first_account_no, r.send_gain_lines), (5000, false));
    }

    #[test]
    fn nx_rewards_reach_the_rules() {
        // DC-DEFAULT: LG-310, LG-311 (proposed); a missing section keeps the defaults
        let text = "[rewards]
nx_enabled = false
nx_daily = 11
nx_clan_win = 22
nx_clan_win_daily_cap = 4
nx_clan_min_players = 5
nx_clan_min_seconds = 66
";
        let cfg = dc_config::parse_toml(text).unwrap();
        let conf = ServerConf {
            cfg,
            ex: Explicit::from_toml(text).unwrap(),
            path: None,
            warnings: Vec::new(),
        };
        let mut r = Rules::default();
        assert!(conf.apply_rules(&mut r).is_empty());
        assert_eq!(
            (r.nx_enabled, r.nx_daily, r.nx_clan_win),
            (false, 11, 22)
        );
        assert_eq!(
            (r.nx_clan_win_daily_cap, r.nx_clan_min_players, r.nx_clan_min_seconds),
            (4, 5, 66)
        );
    }

    #[test]
    fn rank_gifts_and_the_rank_bonus_reach_the_rules() {
        // DC-DEFAULT: CD-89 (proposed), CD-41 (amended, LG-303)
        let text = "[economy]
rank_bonus = false
[rewards]
rank_unlock_days = 30
rank_case_products = [41, 42]
[[rewards.rank_gifts]]
rank = 5
products = [1234, 77]
[[rewards.rank_gifts]]
rank = 9
products = [3]
";
        let conf = ServerConf {
            cfg: dc_config::parse_toml(text).unwrap(),
            ex: Explicit::from_toml(text).unwrap(),
            path: None,
            warnings: Vec::new(),
        };
        let mut r = Rules::default();
        assert!(r.rank_gifts.is_empty() && r.rank_bonus);
        assert!(conf.apply_rules(&mut r).is_empty());
        assert_eq!(r.rank_gifts, vec![(5, vec![1234, 77]), (9, vec![3])]);
        assert!(!r.rank_bonus);
        assert_eq!(
            (r.rank_unlock_days, r.rank_case_products.clone()),
            (30, vec![41, 42])
        );
        let d = Rules::default();
        assert_eq!((d.rank_unlock_days, d.rank_case_products.len()), (7, 0));
        // lobby.json's form; a rank 0 or a product 0 is refused
        let ok = JsonValue::parse(r#"[{"rank": 2, "products": [8]}]"#).unwrap();
        assert!(r.apply("rewards.rank_gifts", &ok));
        assert_eq!(r.rank_gifts, vec![(2, vec![8])]);
        for bad in [
            r#"[{"rank": 0, "products": [8]}]"#,
            r#"[{"rank": 2, "products": [0]}]"#,
            r#"[{"rank": 2}]"#,
        ] {
            assert!(!r.apply("rewards.rank_gifts", &JsonValue::parse(bad).unwrap()));
        }
    }

    #[test]
    fn modes_playable_reaches_the_rules() {
        // DC-DEFAULT: MG-1 (staged)
        let text = "[modes]
playable = [\"TDM\", \"BOMB\", \"NOPE\"]
";
        let conf = ServerConf {
            cfg: dc_config::parse_toml(text).unwrap(),
            ex: Explicit::from_toml(text).unwrap(),
            path: None,
            warnings: Vec::new(),
        };
        let mut r = Rules::default();
        assert!(conf.apply_rules(&mut r).is_empty());
        assert_eq!(
            r.modes_playable.as_deref(),
            Some(&["TDM".to_string(), "BOMB".into(), "NOPE".into()][..])
        );
        // a file that does not name the key leaves the built-in list in force
        let mut r = Rules::default();
        ServerConf {
            cfg: Config::default(),
            ex: Explicit::default(),
            path: None,
            warnings: Vec::new(),
        }
        .apply_rules(&mut r);
        assert!(r.modes_playable.is_none());
        assert!(ServerConf::install_playable(&r).is_empty());
        // lobby.json form: an array of names; anything else is refused
        assert!(!r.apply("modes.playable", &JsonValue::Bool(true)));
        assert!(r.apply(
            "modes.playable",
            &JsonValue::Array(vec![JsonValue::String("*".into())])
        ));
    }

    #[test]
    fn every_schema_key_is_mapped_or_listed() {
        let conf = all_set();
        let mut keys = Vec::new();
        leaves("", &conf.cfg.to_value(), &mut keys);
        // the free maps are leaves only when empty; list_override children are always leaves
        let mapped: Vec<String> = conf.rule_overrides().into_iter().map(|(k, _)| k).collect();
        assert!(!mapped.is_empty());
        let mut missing = Vec::new();
        for k in &keys {
            let section = k.split('.').next().unwrap();
            if OTHER_SECTIONS.contains(&section)
                || k.starts_with("gm.")
                    && conf.cfg.gm.extra.contains_key(k.trim_start_matches("gm."))
            {
                continue;
            }
            if NOT_READ_BY_LOBBY
                .iter()
                .any(|(p, _)| k == p || k.starts_with(&format!("{p}.")))
            {
                continue;
            }
            // the toml path set must appear as the config side of some mapping: check by perturbing is overkill,
            // so map each rule key back to its schema path through the known renames
            let rule_key = match k.as_str() {
                "economy.exp.win_percent" => "economy.exp.win_bonus_pct".to_string(),
                "economy.gp.win_percent" => "economy.gp.win_bonus_pct".to_string(),
                "inventory.starter_items" => "inventory.starter".to_string(),
                "lobby.channels" => "lobby.channels".to_string(),
                s if s.starts_with("shop.list_override.") => "shop.lists".to_string(),
                s if s.starts_with("shop.coupon_uses") => "shop.coupons".to_string(),
                s if s.starts_with("shop.coupons.") => "shop.coupons".to_string(),
                s if s.starts_with("shop.price_override.") => "shop.price_override".to_string(),
                s => s.to_string(),
            };
            let covered = mapped
                .iter()
                .any(|m| *m == rule_key || m.starts_with(&format!("{rule_key}.")));
            // lobby.channels is empty by default and only mapped when non-empty; coupon_uses only with coupons;
            // auth.auto_create is read as the --no-auto-create option (cli.rs, LG-32)
            let special = matches!(
                k.as_str(),
                "lobby.channels" | "shop.coupon_uses" | "auth.auto_create"
            );
            if !covered && !special {
                missing.push(k.clone());
            }
        }
        assert!(
            missing.is_empty(),
            "schema keys with no mapping and no NOT_READ_BY_LOBBY entry: {missing:?}"
        );
    }

    #[test]
    fn every_mapped_value_is_accepted_by_the_rules() {
        let mut conf = all_set();
        conf.cfg.lobby.channels = vec![dc_config::LobbyChannelCfg {
            name: "A".into(),
            capacity: 10,
        }];
        conf.ex = Explicit::from_toml(&conf.cfg.to_toml()).unwrap();
        let mut rules = Rules::default();
        let notes = conf.apply_rules(&mut rules);
        assert!(notes.is_empty(), "{notes:?}");
        // all-default config leaves the rules equal to the built-in defaults
        let d = Rules::default();
        assert_eq!(rules.exp, d.exp);
        assert_eq!(
            (rules.countdown_ticks, rules.max_players_cap, rules.channels),
            (d.countdown_ticks, d.max_players_cap, 1)
        );
        assert_eq!(rules.shop.case_odds, d.shop.case_odds);
    }

    #[test]
    fn only_explicit_keys_override() {
        let (cfg, ex) = dc_config::load_layered_explicit(
            Some("[room]\nmax_players_cap = 8\n[economy.exp]\nkill = 4\n"),
            Vec::<(String, String)>::new(),
            &CliOverrides::default(),
        )
        .unwrap();
        let conf = ServerConf {
            cfg,
            ex,
            path: None,
            warnings: Vec::new(),
        };
        let keys: Vec<String> = conf.rule_overrides().into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            keys,
            vec![
                "economy.exp.kill".to_string(),
                "room.max_players_cap".to_string()
            ]
        );
    }

    #[test]
    fn layer_is_off_without_file_or_env() {
        let env = vec![
            ("DC_TARGET".to_string(), "x86_64".to_string()),
            ("PATH".to_string(), "x".to_string()),
        ];
        assert!(ServerConf::load(None, &env).unwrap().is_none());
        let env = vec![("DC_ROOM__MAX_PLAYERS_CAP".to_string(), "9".to_string())];
        let c = ServerConf::load(None, &env).unwrap().unwrap();
        assert_eq!(c.cfg.room.max_players_cap, 9);
        assert!(c.path.is_none() && c.ex.has("room.max_players_cap"));
    }

    #[test]
    fn check_errors_are_fatal_and_bad_overrides_reported() {
        let env = vec![("DC_ROOM__MAX_PLAYERS_CAP".to_string(), "999".to_string())];
        let e = ServerConf::load(None, &env).unwrap_err();
        assert!(e[0].contains("room.max_players_cap"), "{e:?}");
        let env = vec![("DC_NO_SUCH__KEY".to_string(), "1".to_string())];
        assert!(ServerConf::load(None, &env).is_err());
    }

    #[test]
    fn toml_wins_over_lobby_json_only_for_the_keys_it_sets() {
        let dir = std::env::temp_dir().join(format!("dc_conf_layers_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("lobby.json"),
            r#"{"economy.exp.kill": 9, "room.max_players_cap": 3, "megaphone.secs": 12}"#,
        )
        .unwrap();
        let (mut rules, notes) = Rules::load(&dir);
        assert!(notes.is_empty());
        let (cfg, ex) = dc_config::load_layered_explicit(
            Some(
                "[room]
max_players_cap = 8
[economy.exp]
kill = 4
",
            ),
            vec![("DC_ECONOMY__EXP__KILL".to_string(), "6".to_string())],
            &CliOverrides::default(),
        )
        .unwrap();
        let conf = ServerConf {
            cfg,
            ex,
            path: None,
            warnings: Vec::new(),
        };
        assert!(conf.apply_rules(&mut rules).is_empty());
        assert_eq!(rules.exp.kill, 6.0, "env > toml > lobby.json");
        assert_eq!(rules.max_players_cap, 8, "toml > lobby.json");
        assert_eq!(
            rules.shop.megaphone_secs, 12,
            "a key the toml leaves out keeps the lobby.json value"
        );
        assert_eq!(
            rules.exp.finish, 10.0,
            "and an untouched key keeps the built-in default"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mod_policy_follows_the_config() {
        let mut c = all_set();
        assert_eq!(c.mod_policy(), ModPolicy::Vanilla);
        c.cfg.mods.policy = "modded".into();
        c.cfg.mods.client_visual_allowed = true;
        assert_eq!(
            c.mod_policy(),
            ModPolicy::Modded {
                client_visual_allowed: true
            }
        );
    }

    #[test]
    fn round_secs_and_hisec_keys_reach_the_rules() {
        // DC-DEFAULT: GMD-2 (clarification), HS-6..HS-9 (proposed)
        let text = "[room.round_secs]
fallback = true
default = 300
1 = 900
[economy]
hisec_case_odds = [[0, 1]]
[hisec]
case_product = 31
announce = \"channel\"
no_key_reason = 77
";
        let conf = ServerConf {
            cfg: dc_config::parse_toml(text).unwrap(),
            ex: Explicit::from_toml(text).unwrap(),
            path: None,
            warnings: Vec::new(),
        };
        // the round-time table goes out as the lobby.json keys of the same names, the switch first (it resets the
        // table), then the default, then the classes, so a file's class entries survive its own `fallback = true`
        // (what each key does to the rules is config.rs's business and tested there)
        let rs = |c: &ServerConf| -> Vec<(String, JsonValue)> {
            c.rule_overrides()
                .into_iter()
                .filter(|(k, _)| k.starts_with("room.round_secs."))
                .collect()
        };
        assert_eq!(
            rs(&conf),
            vec![
                (
                    "room.round_secs.fallback".to_string(),
                    JsonValue::Bool(true)
                ),
                ("room.round_secs.default".to_string(), num(300.0)),
                ("room.round_secs.1".to_string(), num(900.0)),
            ]
        );
        let mut r = Rules::default();
        assert!(conf.apply_rules(&mut r).is_empty());
        assert_eq!(r.hisec.odds, vec![(0, 1)]);
        assert_eq!(
            (
                r.hisec.case_product,
                r.hisec.announce,
                r.hisec.no_key_reason
            ),
            (31, crate::hisec::Announce::Channel, 77)
        );
        // only the keys the file sets go out: the switch and one class, no default
        let text = "[room.round_secs]
fallback = false
4 = 120
";
        let conf = ServerConf {
            cfg: dc_config::parse_toml(text).unwrap(),
            ex: Explicit::from_toml(text).unwrap(),
            path: None,
            warnings: Vec::new(),
        };
        assert_eq!(
            rs(&conf),
            vec![
                (
                    "room.round_secs.fallback".to_string(),
                    JsonValue::Bool(false)
                ),
                ("room.round_secs.4".to_string(), num(120.0)),
            ]
        );
        assert!(conf.apply_rules(&mut Rules::default()).is_empty());
    }
}
