//! `dc-config check`: semantic validation of a loaded [`Config`]. Collects every problem, with the config
//! path and a message that says what to change.

use crate::directory::{check_host_port, check_mod_ref, is_http_url, REGIONS};
use crate::schema::{Config, LobbyServerCfg};
use crate::Issue;
use std::collections::BTreeSet;
use std::net::IpAddr;
use std::path::Path;

#[derive(Debug, Clone, Copy, Default)]
pub struct CheckOptions {
    /// Also look at the file system: ROM folder, mods dir, mod zips, DB parent directory.
    pub fs: bool,
}

/// The listener ports after the "0 = derive" rules. SPEC: LOBBY_HARNESS.md §1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    pub auth: u32,
    pub lobby: u32,
    pub clan: u32,
    pub http: u32,
}

impl Config {
    /// Resolve derived ports (values may exceed 65535 when the base is high; `check` reports that).
    pub fn resolved_ports(&self) -> Ports {
        let s = &self.server;
        let auth = u32::from(s.auth_port);
        let lobby = if s.lobby_port == 0 {
            auth + 2
        } else {
            u32::from(s.lobby_port)
        };
        let clan = if s.clan_port == 0 {
            lobby + 1
        } else {
            u32::from(s.clan_port)
        };
        let web = if self.clan.web_port != 0 {
            u32::from(self.clan.web_port)
        } else {
            u32::from(s.http_port)
        };
        let http = if web == 0 { auth + 8080 } else { web };
        Ports {
            auth,
            lobby,
            clan,
            http,
        }
    }

    /// Parse `dserver.port_range` ("lo-hi").
    pub fn dserver_range(&self) -> Result<(u16, u16), String> {
        let s = &self.dserver.port_range;
        let (a, b) = s
            .split_once('-')
            .ok_or_else(|| format!("'{s}' must look like 29901-29999"))?;
        let p = |x: &str| {
            x.trim()
                .parse::<u16>()
                .map_err(|_| format!("'{}' is not a port number", x.trim()))
        };
        let (lo, hi) = (p(a)?, p(b)?);
        if lo > hi {
            return Err(format!("range start {lo} is above its end {hi}"));
        }
        Ok((lo, hi))
    }
}

fn is_loopback(s: &str) -> bool {
    s.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(s.eq_ignore_ascii_case("localhost"))
}

fn one_of(out: &mut Vec<Issue>, path: &str, v: &str, allowed: &[&str]) {
    if !allowed.contains(&v) {
        out.push(Issue::error(
            path,
            format!("'{v}' is not valid; use one of: {}", allowed.join(", ")),
        ));
    }
}

/// The "listens publicly but advertises loopback" finding: `public` (or a non-loopback `bind`) together with a
/// loopback `advertise` address. A warning in `check`; launchers that host publicly treat it as a refusal.
pub fn public_loopback_advertise(public: bool, bind: &str, advertise: &str) -> Option<Issue> {
    ((public || !is_loopback(bind)) && is_loopback(advertise)).then(|| {
        Issue::warning(
            "server.advertise_ip",
            "the server listens publicly but advertises a loopback address; remote players will not be able to join",
        )
    })
}

/// Validate `cfg`. Returns all issues (errors and warnings); the config is usable when none is an error.
pub fn check(cfg: &Config, opts: CheckOptions) -> Vec<Issue> {
    let mut out = Vec::new();
    let s = &cfg.server;

    // --- network ---------------------------------------------------------------------------
    if s.bind.parse::<IpAddr>().is_err() {
        out.push(Issue::error(
            "server.bind",
            format!("'{}' is not an IP address such as 127.0.0.1", s.bind),
        ));
    }
    if s.bind == "0.0.0.0" && !s.public {
        out.push(Issue::error(
            "server.bind",
            "0.0.0.0 exposes the server to the network; set server.public = true (or pass --public) to confirm, or use 127.0.0.1",
        ));
    }
    if s.advertise_ip.trim().is_empty() || s.advertise_ip.chars().any(|c| c.is_whitespace()) {
        out.push(Issue::error(
            "server.advertise_ip",
            "must be an IP address or host name without spaces",
        ));
    } else if let Some(i) = public_loopback_advertise(s.public, &s.bind, &s.advertise_ip) {
        out.push(i);
    }
    if s.name.trim().is_empty() || s.name.chars().count() > 48 {
        out.push(Issue::error("server.name", "must be 1-48 characters"));
    }
    if s.auth_port == 0 {
        out.push(Issue::error("server.auth_port", "must be non-zero"));
    }
    if s.game_port == 0 {
        out.push(Issue::error("server.game_port", "must be non-zero"));
    }
    let ports = cfg.resolved_ports();
    let mut tcp: Vec<(&str, u32)> = vec![
        ("server.auth_port", ports.auth),
        ("server.lobby_port", ports.lobby),
        ("server.clan_port", ports.clan),
    ];
    if s.http_enabled {
        tcp.push(("server.http_port", ports.http));
    }
    if cfg.metrics.enabled {
        tcp.push(("metrics.port", u32::from(cfg.metrics.port)));
    }
    for (name, p) in &tcp {
        if *p == 0 || *p > 65535 {
            out.push(Issue::error(
                *name,
                format!("resolves to {p}, outside 1..=65535; set it explicitly"),
            ));
        }
    }
    for i in 0..tcp.len() {
        for j in i + 1..tcp.len() {
            if tcp[i].1 == tcp[j].1 {
                out.push(Issue::error(
                    tcp[j].0,
                    format!("port {} is already used by {}", tcp[j].1, tcp[i].0),
                ));
            }
        }
    }
    match cfg.dserver_range() {
        Ok((lo, hi)) => {
            if lo < 1024 {
                out.push(Issue::error(
                    "dserver.port_range",
                    "start must be 1024 or higher",
                ));
            }
            if (lo..=hi).contains(&s.game_port) {
                out.push(Issue::error(
                    "dserver.port_range",
                    format!("contains server.game_port {}", s.game_port),
                ));
            }
            for (name, p) in &tcp {
                if (u32::from(lo)..=u32::from(hi)).contains(p) {
                    out.push(Issue::warning("dserver.port_range", format!("contains {name} ({p}); TCP and UDP can share a number, but this is confusing")));
                }
            }
        }
        Err(e) => out.push(Issue::error("dserver.port_range", e)),
    }

    // --- logging, metrics ------------------------------------------------------------------
    one_of(&mut out, "log.format", &cfg.log.format, &["text", "json"]);
    one_of(
        &mut out,
        "log.level",
        &cfg.log.level,
        &["error", "warn", "info", "debug", "trace"],
    );
    if cfg.metrics.bind.parse::<IpAddr>().is_err() {
        out.push(Issue::error(
            "metrics.bind",
            format!("'{}' is not an IP address", cfg.metrics.bind),
        ));
    } else if cfg.metrics.enabled && !is_loopback(&cfg.metrics.bind) {
        out.push(Issue::warning(
            "metrics.bind",
            "the metrics endpoint has no authentication; keep it on loopback or firewall it",
        ));
    }

    // --- paths -----------------------------------------------------------------------------
    if cfg.paths.rom.is_empty() {
        out.push(Issue::warning(
            "paths.rom",
            "not set; the server needs the 2009 client folder (--rom <folder> or DC_PATHS__ROM)",
        ));
    } else if opts.fs && !Path::new(&cfg.paths.rom).is_dir() {
        out.push(Issue::error(
            "paths.rom",
            format!("'{}' is not a folder", cfg.paths.rom),
        ));
    }
    if cfg.paths.db.is_empty() {
        out.push(Issue::error("paths.db", "must name the SQLite file"));
    } else if opts.fs {
        let parent = Path::new(&cfg.paths.db)
            .parent()
            .filter(|p| !p.as_os_str().is_empty());
        if let Some(p) = parent {
            if !p.is_dir() {
                out.push(Issue::error(
                    "paths.db",
                    format!("directory '{}' does not exist", p.display()),
                ));
            }
        }
    }

    // --- directory registration ------------------------------------------------------------
    let d = &cfg.directory;
    if !REGIONS.contains(&d.region.as_str()) {
        out.push(Issue::error(
            "directory.region",
            format!("'{}' is not one of {}", d.region, REGIONS.join(", ")),
        ));
    }
    if !d.website.is_empty() && !is_http_url(&d.website) {
        out.push(Issue::error(
            "directory.website",
            "must be an http(s) URL or empty",
        ));
    }
    if d.description.chars().count() > 400 {
        out.push(Issue::error(
            "directory.description",
            "must be at most 400 characters",
        ));
    }
    for (i, u) in d.urls.iter().enumerate() {
        if !is_http_url(u) {
            out.push(Issue::error(
                format!("directory.urls[{i}]"),
                format!("'{u}' is not an http(s) URL"),
            ));
        }
    }
    if d.register {
        if d.urls.is_empty() {
            out.push(Issue::error(
                "directory.urls",
                "registration is on but no directory URL is listed",
            ));
        }
        if d.public_address.is_empty() {
            out.push(Issue::error(
                "directory.public_address",
                "registration is on; set the host:port players should join",
            ));
        } else if let Err(e) = check_host_port(&d.public_address) {
            out.push(Issue::error("directory.public_address", e));
        }
        if !(10..=3600).contains(&d.heartbeat_secs) {
            out.push(Issue::error(
                "directory.heartbeat_secs",
                "must be between 10 and 3600",
            ));
        }
        if !s.public {
            out.push(Issue::warning(
                "directory.register",
                "registering a server that only listens on loopback; set server.public = true",
            ));
        }
    }

    // --- mod-set declaration ---------------------------------------------------------------
    let m = &cfg.mods;
    match m.policy.as_str() {
        "vanilla" => {
            if !m.set.is_empty() {
                out.push(Issue::error("mods.set", "policy 'vanilla' means no mods at all; empty the set or set mods.policy = \"modded\""));
            }
            if m.client_visual_allowed {
                out.push(Issue::error(
                    "mods.client_visual_allowed",
                    "must be false on a vanilla server",
                ));
            }
            // DC-DEFAULT: MOD-22 (proposed): only a declared mod (world.full_visibility) turns culling off
            if !cfg.vis.enabled {
                out.push(Issue::error(
                    "vis.enabled",
                    "a vanilla server keeps visibility culling on; culling off belongs to a modded server whose mod declares world.full_visibility",
                ));
            }
        }
        "modded" => {
            if m.set.is_empty() {
                out.push(Issue::error(
                    "mods.set",
                    "policy 'modded' needs at least one [[mods.set]] entry",
                ));
            }
            if !s.http_enabled {
                out.push(Issue::error(
                    "server.http_enabled",
                    "a modded server serves its mods over HTTP; enable it",
                ));
            }
        }
        other => out.push(Issue::error(
            "mods.policy",
            format!("'{other}' is not one of vanilla, modded"),
        )),
    }
    if m.max_zip_bytes > crate::schema::MAX_ZIP_BYTES {
        out.push(Issue::error(
            "mods.max_zip_bytes",
            "must be at most 33554432 (32 MiB), the largest package a client will download",
        ));
    }
    if m.max_zip_bytes == 0 {
        out.push(Issue::error("mods.max_zip_bytes", "must be greater than 0"));
    }
    // DC-DEFAULT: MOD-25 (proposed): the optional catalogue, on any policy
    let mut offered = BTreeSet::new();
    for (i, r) in m.catalog.iter().enumerate() {
        out.extend(check_mod_ref(&format!("mods.catalog[{i}]"), r));
        if !offered.insert(r.id.as_str()) {
            out.push(Issue::error(
                format!("mods.catalog[{i}].id"),
                format!("mod '{}' is listed twice", r.id),
            ));
        }
        if m.set.iter().any(|d| d.id == r.id) {
            out.push(Issue::error(
                format!("mods.catalog[{i}].id"),
                format!(
                    "mod '{}' is in the declared set; the catalogue is for optional mods",
                    r.id
                ),
            ));
        }
        if !s.http_enabled {
            out.push(Issue::error(
                "server.http_enabled",
                "a catalogue is served over HTTP; enable it",
            ));
        }
    }
    let mut ids = BTreeSet::new();
    for (i, r) in m.set.iter().enumerate() {
        out.extend(check_mod_ref(&format!("mods.set[{i}]"), r));
        if !ids.insert(r.id.as_str()) {
            out.push(Issue::error(
                format!("mods.set[{i}].id"),
                format!("mod '{}' is listed twice", r.id),
            ));
        }
        if opts.fs && !cfg.paths.mods_dir.is_empty() {
            let zip = Path::new(&cfg.paths.mods_dir).join(format!("{}.zip", r.sha256));
            if !zip.is_file() {
                out.push(Issue::error(
                    format!("mods.set[{i}]"),
                    format!(
                        "{} is missing from the mods dir (expected '{}')",
                        r.id,
                        zip.display()
                    ),
                ));
            }
        }
    }

    // --- DC_DEFAULTS keys --------------------------------------------------------------------
    let e = &cfg.economy;
    if e.case_odds.is_empty() {
        out.push(Issue::error(
            "economy.case_odds",
            "needs at least one [days, weight] pair",
        ));
    } else {
        let total: u32 = e.case_odds.iter().map(|p| p[1]).sum();
        if total == 0 {
            out.push(Issue::error("economy.case_odds", "weights add up to 0"));
        } else if total != 100 {
            out.push(Issue::warning(
                "economy.case_odds",
                format!("weights add up to {total}, not 100; they are used as relative weights"),
            ));
        }
    }
    // DC-DEFAULT: HS-8 (proposed) the HiSec duration table needs one weighted pair (hisec.rs refuses an all-zero
    // table the same way)
    if e.hisec_case_odds.is_empty() || e.hisec_case_odds.iter().all(|p| p[1] == 0) {
        out.push(Issue::error(
            "economy.hisec_case_odds",
            "needs at least one [days, weight] pair with a weight above 0",
        ));
    }
    if cfg.shop.hot_percent > 100 {
        out.push(Issue::error("shop.hot_percent", "must be 0..=100"));
    }
    // DC-DEFAULT: CD-89 (proposed) a promotion gift names a rank 1..=255 (the client reads at most 255 level rows)
    // once, and at least one product id above 0
    let mut gift_ranks = std::collections::BTreeSet::new();
    for (k, g) in cfg.rewards.rank_gifts.iter().enumerate() {
        let at = format!("rewards.rank_gifts[{k}]");
        if g.rank == 0 || g.rank > 255 {
            out.push(Issue::error(format!("{at}.rank"), "must be 1..=255"));
        } else if !gift_ranks.insert(g.rank) {
            out.push(Issue::error(
                format!("{at}.rank"),
                format!("rank {} is listed twice", g.rank),
            ));
        }
        if g.products.is_empty() || g.products.iter().any(|p| *p == 0 || *p > i32::MAX as u32) {
            out.push(Issue::error(
                format!("{at}.products"),
                "needs at least one shop product id, each 1..=2147483647",
            ));
        }
    }
    // DC-DEFAULT: CD-89 (project decision the unlocked variant's days 1..=3650; the case pool's product ids
    // above 0
    if !(1..=3650).contains(&cfg.rewards.rank_unlock_days) {
        out.push(Issue::error("rewards.rank_unlock_days", "must be 1..=3650"));
    }
    if cfg
        .rewards
        .rank_case_products
        .iter()
        .any(|p| *p == 0 || *p > i32::MAX as u32)
    {
        out.push(Issue::error(
            "rewards.rank_case_products",
            "shop product ids, each 1..=2147483647 (empty = every NX supply case of the ROM)",
        ));
    }
    // DC-DEFAULT: LG-310, LG-311 (proposed) the NX reward ranges
    let r = &cfg.rewards;
    for (key, value, low, high) in [
        ("rewards.nx_daily", r.nx_daily, 0, 1_000_000),
        ("rewards.nx_clan_win", r.nx_clan_win, 0, 1_000_000),
        (
            "rewards.nx_clan_win_daily_cap",
            r.nx_clan_win_daily_cap,
            1,
            100,
        ),
        ("rewards.nx_clan_min_players", r.nx_clan_min_players, 1, 8),
        (
            "rewards.nx_clan_min_seconds",
            r.nx_clan_min_seconds,
            0,
            3600,
        ),
    ] {
        if !(low..=high).contains(&value) {
            out.push(Issue::error(key, format!("must be {low}..={high}")));
        }
    }
    if cfg.shop.coupons.keys().any(|k| k.trim().is_empty()) {
        out.push(Issue::error(
            "shop.coupons",
            "coupon codes must not be empty",
        ));
    }
    for (k, w) in &cfg.clan.mode_weight {
        if !w.is_finite() || *w < 0.0 {
            out.push(Issue::error(
                format!("clan.mode_weight.{k}"),
                "must be a finite number >= 0",
            ));
        }
    }
    if cfg.room.default_mid_join > 1 {
        out.push(Issue::error("room.default_mid_join", "must be 0 or 1"));
    }
    if !(1..=64).contains(&cfg.room.max_players_cap) {
        out.push(Issue::error(
            "room.max_players_cap",
            "must be between 1 and 64",
        ));
    }
    if !(cfg.room.blast_damage_scale.is_finite()
        && (0.0..=100.0).contains(&cfg.room.blast_damage_scale))
    {
        out.push(Issue::error(
            "room.blast_damage_scale",
            "must be a number between 0 and 100",
        ));
    }
    if cfg.room.max_per_channel == 0 {
        out.push(Issue::error("room.max_per_channel", "must be at least 1"));
    }
    if cfg.room.countdown_ms == 0 && cfg.room.countdown_ticks > 0 {
        out.push(Issue::warning(
            "room.countdown_ms",
            "0 ms ticks make the countdown instant",
        ));
    }
    if cfg.auth.session_ttl_secs == 0 {
        out.push(Issue::error("auth.session_ttl_secs", "must be at least 1"));
    }
    if cfg.inventory.expiry_scan_secs == 0 {
        out.push(Issue::error(
            "inventory.expiry_scan_secs",
            "must be at least 1",
        ));
    }
    if !(1..=240).contains(&cfg.dserver.tick_hz) {
        out.push(Issue::error("dserver.tick_hz", "must be between 1 and 240"));
    }
    one_of(&mut out, "auth.file_check", &cfg.auth.file_check, &["off"]);
    one_of(
        &mut out,
        "names.referrer",
        &cfg.names.referrer,
        &["optional", "off"],
    );
    one_of(
        &mut out,
        "inventory.default_repair",
        &cfg.inventory.default_repair,
        &["weapons", "all", "off"],
    );
    one_of(&mut out, "shop.lists", &cfg.shop.lists, &["rom"]);
    one_of(
        &mut out,
        "gm.test_commands",
        &cfg.gm.test_commands,
        &["gm", "all", "loopback"],
    );
    one_of(
        &mut out,
        "notice.case_announce",
        &cfg.notice.case_announce,
        &["off", "permanent", "all"],
    );
    if cfg.gm.default_mute_minutes == 0 || cfg.gm.max_mute_minutes == 0 {
        out.push(Issue::error(
            "gm.default_mute_minutes",
            "mute minutes must be at least 1",
        ));
    }
    if cfg.clan.level_steps.first() != Some(&0)
        || cfg.clan.level_steps.windows(2).any(|w| w[0] > w[1])
        || cfg.clan.level_steps.len() > 6
    {
        out.push(Issue::error(
            "clan.level_steps",
            "must start at 0, never decrease and hold at most 6 levels",
        ));
    }
    if cfg.clan.member_limits.is_empty()
        || cfg.clan.member_limits.len() > 6
        || cfg.clan.member_limits.contains(&0)
    {
        out.push(Issue::error(
            "clan.member_limits",
            "needs 1..6 limits, each at least 1",
        ));
    }
    if cfg.inbox.max_rows == 0 {
        out.push(Issue::error("inbox.max_rows", "must be at least 1"));
    }
    one_of(
        &mut out,
        "dserver.file_info",
        &cfg.dserver.file_info,
        &["size", "zero"],
    );
    one_of(
        &mut out,
        "dserver.hello_match",
        &cfg.dserver.hello_match,
        &["name"],
    );
    one_of(
        &mut out,
        "dserver.remote_console",
        &cfg.dserver.remote_console,
        &["moderators", "off"],
    );
    one_of(
        &mut out,
        "combat.zone_factors",
        &cfg.combat.zone_factors,
        &["rom", "off"],
    );
    one_of(&mut out, "combat.trace", &cfg.combat.trace, &["on", "off"]);
    one_of(
        &mut out,
        "combat.armor_cost",
        &cfg.combat.armor_cost,
        &["base", "hit"],
    );
    {
        let g = cfg.combat.armor_gear.trim();
        let ok = g.eq_ignore_ascii_case("worn")
            || g.eq_ignore_ascii_case("items")
            || g.eq_ignore_ascii_case("off")
            || g.parse::<u16>().is_ok();
        if !ok {
            out.push(Issue::error(
                "combat.armor_gear",
                "must be \"worn\", an armour row number (0..65535), \"items\" or \"off\"",
            ));
        }
    }
    {
        let c = &cfg.combat;
        let mut nums = vec![
            ("head_factor", c.head_factor),
            ("body_factor", c.body_factor),
            ("head_fraction", c.head_fraction),
            ("start_health", c.start_health),
            ("max_health", c.max_health),
            ("default_damage", c.default_damage),
            ("hit_tolerance", c.hit_tolerance),
            ("origin_tolerance", c.origin_tolerance),
            ("trace_slack", c.trace_slack),
        ];
        for (name, r) in [
            ("armor_absorb", c.armor_absorb),
            ("start_armor", c.start_armor),
            ("max_armor", c.max_armor),
        ] {
            if let crate::RomOr::Value(v) = r {
                nums.push((name, v));
            }
        }
        for (name, v) in nums {
            if !(v.is_finite() && v >= 0.0) {
                out.push(Issue::error(
                    format!("combat.{name}"),
                    "must be a number of at least 0 (or \"rom\" where the key has a ROM value)",
                ));
            }
        }
    }
    // DC-DEFAULT: DSV-451 (proposed, amended 2026-10-30) the agent's health is at least 1, his armour at least 0
    for (name, v, min) in [
        ("spy.agent_health", cfg.spy.agent_health, 1.0),
        ("spy.agent_armor", cfg.spy.agent_armor, 0.0),
        ("spy.agent_damage_scale", cfg.spy.agent_damage_scale, 0.0),
    ] {
        if !(v.is_finite() && v >= min) {
            out.push(Issue::error(
                name,
                format!("must be a number of at least {min}"),
            ));
        }
    }
    // DC-DEFAULT: DSV-39 (amendment, proposed) the full-damage share of the blast radius is a fraction 0..=1
    if !(cfg.combat.blast_inner.is_finite() && (0.0..=1.0).contains(&cfg.combat.blast_inner)) {
        out.push(Issue::error(
            "combat.blast_inner",
            "must be a number from 0 to 1",
        ));
    }
    // DC-DEFAULT: DSV-60 / DSV-47 (proposed) `-evars world|off`; every `[evars.extra]` entry is one `-evar
    // name=value` launch argument, so the name carries no space or '=' and the value is one line
    one_of(&mut out, "evars.mode", &cfg.evars.mode, &["world", "off"]);
    for (k, v) in &cfg.evars.extra {
        if k.trim().is_empty() || k.contains(char::is_whitespace) || k.contains('=') {
            out.push(Issue::error(
                format!("evars.extra.{k}"),
                "a variable name must be non-empty, without spaces or '='",
            ));
        }
        if v.contains(['\r', '\n', '\0']) {
            out.push(Issue::error(
                format!("evars.extra.{k}"),
                "a value must be one line of text",
            ));
        }
    }
    // DC-DEFAULT: HS-9 (proposed) announcement scope; HS-7 (proposed) the 0x32f reason is a u16 above 0; HS-6
    // (proposed) the case product is a u16 (0 = resolved from the ROM tables)
    one_of(
        &mut out,
        "hisec.announce",
        &cfg.hisec.announce,
        &["all", "channel", "off"],
    );
    if cfg.hisec.no_key_reason == 0 || cfg.hisec.no_key_reason > 0xffff {
        out.push(Issue::error("hisec.no_key_reason", "must be 1..=65535"));
    }
    if cfg.hisec.case_product > 0xffff {
        out.push(Issue::error(
            "hisec.case_product",
            "must be 0..=65535 (0 = from the ROM)",
        ));
    }
    // DC-DEFAULT: GMD-2 (clarification; project decision: every room has a time limit) the fallback table
    // keys are `fallback`, `default` and a ROM mode class 0..=255; seconds at most one day, like the lobby.json keys
    let rs = &cfg.room.round_secs;
    if rs.default > 86_400 {
        out.push(Issue::error(
            "room.round_secs.default",
            "must be at most 86400 (s)",
        ));
    }
    for (k, v) in &rs.classes {
        if !k.parse::<u32>().map(|c| c <= 255).unwrap_or(false) {
            out.push(Issue::error(
                format!("room.round_secs.{k}"),
                "keys of room.round_secs are fallback, default or a mode class 0..=255",
            ));
        } else if *v > 86_400 {
            out.push(Issue::error(
                format!("room.round_secs.{k}"),
                "must be at most 86400 (s)",
            ));
        }
    }
    if !(cfg.vis.always_radius.is_finite() && cfg.vis.always_radius >= 0.0) {
        out.push(Issue::error(
            "vis.always_radius",
            "must be a number of at least 0 (units)",
        ));
    }
    if cfg.vis.exposure_ms > 60_000 {
        out.push(Issue::error(
            "vis.exposure_ms",
            "must be at most 60000 (ms)",
        ));
    }
    // DC-DEFAULT: LG-322 (proposed) client versions are MAJOR.MINOR.PATCH, the oldest allowed not above the newest
    let version = |s: &str| -> Option<[u32; 3]> {
        let mut p = s.split('.').map(|n| n.parse::<u32>().ok());
        let v = [p.next()??, p.next()??, p.next()??];
        p.next().is_none().then_some(v)
    };
    for (key, v) in [
        ("client.latest", &cfg.client.latest),
        ("client.min_version", &cfg.client.min_version),
    ] {
        if !v.is_empty() && version(v).is_none() {
            out.push(Issue::error(
                key,
                "must be MAJOR.MINOR.PATCH, for example 0.3.0",
            ));
        }
    }
    if let (Some(lo), Some(hi)) = (
        version(&cfg.client.min_version),
        version(&cfg.client.latest),
    ) {
        if lo > hi {
            out.push(Issue::error("client.min_version", "is above client.latest"));
        }
    }
    if cfg.client.update_url.chars().count() > 100
        || cfg.client.update_url.chars().any(char::is_control)
    {
        out.push(Issue::error(
            "client.update_url",
            "at most 100 characters, no control characters",
        ));
    }
    // DC-DEFAULT: LG-404 (proposed) the direct zip links are https:// addresses of at most 300 plain characters
    for (key, v) in [
        ("client.download.windows", &cfg.client.download.windows),
        ("client.download.macos", &cfg.client.download.macos),
    ] {
        let plain = v.chars().all(|c| {
            c.is_ascii_graphic()
                && !matches!(c, '"' | '\\' | '<' | '>' | '^' | '`' | '{' | '|' | '}')
        });
        if !v.is_empty() && (!v.starts_with("https://") || v.len() > 300 || !plain) {
            out.push(Issue::error(
                key,
                "an https:// address of at most 300 characters, no spaces or quotes",
            ));
        }
    }
    // DC-DEFAULT: LG-245 (proposed) a mail server needs a port and a sender address with an @
    let m = &cfg.mail;
    if !m.smtp_host.trim().is_empty() {
        if m.smtp_port == 0 {
            out.push(Issue::error(
                "mail.smtp_port",
                "must be 1..=65535 (587 or 465)",
            ));
        }
        if !m.from.contains('@') {
            out.push(Issue::error(
                "mail.from",
                "needs the sender address (name@domain) when mail.smtp_host is set",
            ));
        }
        if !m.user.is_empty() && m.password.is_empty() {
            out.push(Issue::warning(
                "mail.password",
                "mail.user is set without a password (set DC_MAIL__PASSWORD)",
            ));
        }
    }
    // DC-DEFAULT: LG-316 (proposed) one plain address, and a way to send to it
    let to = m.reports_to.trim();
    if !to.is_empty() {
        if to.matches('@').count() != 1
            || to
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == ',')
        {
            out.push(Issue::error(
                "mail.reports_to",
                "must be one address (name@domain)",
            ));
        }
        if m.smtp_host.trim().is_empty() && !m.dry_run {
            out.push(Issue::warning(
                "mail.reports_to",
                "no mail.smtp_host: reports stay in the DB and no mail goes out",
            ));
        }
        if m.reports_per_hour == 0 {
            out.push(Issue::warning(
                "mail.reports_per_hour",
                "0: no report mail ever goes out; the reports wait in the DB",
            ));
        }
    }
    // DC-DEFAULT: LG-370 (proposed) the beta signup needs a mail sender, plain http(s) origins and one reply address
    let b = &cfg.beta;
    if b.enabled {
        let origin_ok = |s: &str| {
            (s.starts_with("https://") || s.starts_with("http://"))
                && !s.ends_with('/')
                && s.chars().all(|c| c.is_ascii_graphic())
        };
        if !origin_ok(&b.site_origin) || b.site_origin.matches('/').count() != 2 {
            out.push(Issue::error(
                "beta.site_origin",
                "must be one origin like https://example.org (scheme and host, no path, no trailing slash)",
            ));
        }
        if !origin_ok(&b.public_url) {
            out.push(Issue::error(
                "beta.public_url",
                "must be an http(s) address without spaces and without a trailing slash",
            ));
        }
        if !b.download_url.is_empty() && !origin_ok(b.download_url.trim_end_matches('/')) {
            out.push(Issue::error(
                "beta.download_url",
                "must be an http(s) address without spaces",
            ));
        }
        let rt = b.reply_to.trim();
        if !rt.is_empty()
            && (rt.matches('@').count() != 1
                || rt
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == ','))
        {
            out.push(Issue::error(
                "beta.reply_to",
                "must be one address (name@domain)",
            ));
        }
        if m.smtp_host.trim().is_empty() && !m.dry_run {
            out.push(Issue::warning(
                "beta.enabled",
                "no mail.smtp_host: a signup cannot be confirmed, the endpoint answers 503",
            ));
        }
        if b.max_mail_per_day == 0 {
            out.push(Issue::warning(
                "beta.max_mail_per_day",
                "0: no beta mail ever goes out",
            ));
        } else if b.confirm_reserve >= b.max_mail_per_day {
            out.push(Issue::warning(
                "beta.confirm_reserve",
                "is not below max_mail_per_day: invites and feedback mails can never be sent",
            ));
        }
        if b.ip_per_hour == 0 || b.global_per_hour == 0 {
            out.push(Issue::warning(
                "beta.ip_per_hour",
                "0: every signup is refused",
            ));
        }
    }
    // DC-DEFAULT: LG-388 (proposed) the managed bots' total cap: at least one group's worth, at most 25 full groups
    if !(1..=175).contains(&cfg.bots.managed_max_total) {
        out.push(Issue::error("bots.managed_max_total", "must be 1..=175"));
    }
    // DC-DEFAULT: LG-382 / LG-383 (proposed) the admin web listener is loopback only, its origin is https and the
    // relying-party id is that origin's host or a parent domain of it
    let a = &cfg.admin_web;
    if a.enabled {
        match a.bind.trim().parse::<std::net::SocketAddr>() {
            Ok(s) if s.ip().is_loopback() => {}
            Ok(_) => out.push(Issue::error(
                "admin_web.bind",
                "must be a loopback address (127.0.0.1:PORT): the tunnel connects locally",
            )),
            Err(_) => out.push(Issue::error(
                "admin_web.bind",
                "must be ADDR:PORT, for example 127.0.0.1:10096",
            )),
        }
        let host = a.public_origin.strip_prefix("https://").unwrap_or("");
        let host_ok = !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'));
        if !host_ok {
            out.push(Issue::error(
                "admin_web.public_origin",
                "must be https:// and a host name (no path, no trailing slash), for example https://admin.example.org",
            ));
        } else {
            let name = host.split(':').next().unwrap_or("").to_ascii_lowercase();
            let rp = a.rp_id.trim().to_ascii_lowercase();
            if rp.is_empty() || !(name == rp || name.ends_with(&format!(".{rp}"))) {
                out.push(Issue::error(
                    "admin_web.rp_id",
                    "must be the host of public_origin (or a parent domain of it)",
                ));
            }
        }
        if a.idle_minutes == 0 || a.session_hours == 0 {
            out.push(Issue::error(
                "admin_web.idle_minutes",
                "the session lengths must be at least 1",
            ));
        }
        if a.max_keys == 0 {
            out.push(Issue::error("admin_web.max_keys", "must be at least 1"));
        }
        if a.attempts_per_ip == 0 || a.attempts_global == 0 {
            out.push(Issue::warning(
                "admin_web.attempts_per_ip",
                "0: every sign-in is refused",
            ));
        }
    }
    // DC-DEFAULT: LG-399 (proposed) further instances: a path-safe unique name, a label, a loopback admin port, at most
    // 8; the listener refuses the same (adminweb::instances)
    if a.instances.len() > 8 {
        out.push(Issue::error("admin_web.instances", "at most 8 instances"));
    }
    if a.label.trim().is_empty() || a.label.chars().count() > 40 {
        out.push(Issue::error("admin_web.label", "must be 1..40 characters"));
    }
    for (i, inst) in a.instances.iter().enumerate() {
        let n = &inst.name;
        let name_ok = (1..=32).contains(&n.len())
            && n.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !n.starts_with('-');
        if !name_ok {
            out.push(Issue::error(
                "admin_web.instances.name",
                format!(
                    "instance {}: the name must be 1..32 of a-z 0-9 - (not starting with -)",
                    i + 1
                ),
            ));
        } else if a.instances[..i].iter().any(|o| o.name == *n) {
            out.push(Issue::error(
                "admin_web.instances.name",
                format!("instance {n} is listed twice"),
            ));
        }
        if inst.label.trim().is_empty() || inst.label.chars().count() > 40 {
            out.push(Issue::error(
                "admin_web.instances.label",
                format!("instance {}: the label must be 1..40 characters", i + 1),
            ));
        }
        match inst.admin.trim().parse::<std::net::SocketAddr>() {
            Ok(s) if s.ip().is_loopback() => {
                if a.bind.trim().parse::<std::net::SocketAddr>().ok() == Some(s) {
                    out.push(Issue::error(
                        "admin_web.instances.admin",
                        format!("instance {}: {s} is the admin web itself", i + 1),
                    ));
                }
            }
            _ => out.push(Issue::error(
                "admin_web.instances.admin",
                format!(
                    "instance {}: must be a loopback ADDR:PORT, for example 127.0.0.1:10091",
                    i + 1
                ),
            )),
        }
    }
    // DC-DEFAULT: LG-392 (proposed) the deploy folder is an absolute path (the root units name it so)
    let dd = cfg.deploy.dir.trim();
    if !dd.is_empty() && !(dd.starts_with('/') || std::path::Path::new(dd).is_absolute()) {
        out.push(Issue::error(
            "deploy.dir",
            "must be an absolute path, for example /opt/decombat/deploy",
        ));
    }
    // DC-DEFAULT: LG-68 (proposed) issuing needs its issuer id and a key pair path; accepting needs issuer + key
    let id = &cfg.identity;
    let issuer_ok = |s: &str| {
        (1..=64).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
    };
    if !id.issuer.is_empty() && !issuer_ok(&id.issuer) {
        out.push(Issue::error(
            "identity.issuer",
            "must be 1..64 of A-Z a-z 0-9 . _ : -",
        ));
    }
    if id.issue {
        if id.issuer.is_empty() {
            out.push(Issue::error(
                "identity.issuer",
                "identity.issue = true needs this lobby's issuer id",
            ));
        }
        if id.key_file.trim().is_empty() && cfg.paths.db.trim().is_empty() {
            out.push(Issue::error(
                "identity.key_file",
                "identity.issue = true needs a key pair path: set identity.key_file, or paths.db (the pair is kept next to the DB)",
            ));
        }
        if id.token_hours == 0 {
            out.push(Issue::error("identity.token_hours", "must be at least 1"));
        }
    } else if id.issuer.is_empty() != id.public_key.trim().is_empty() {
        out.push(Issue::error(
            "identity.public_key",
            "accepting identity tokens needs both identity.issuer and identity.public_key",
        ));
    }
    if !id.public_key.trim().is_empty() {
        let k = id.public_key.trim().trim_end_matches('=');
        let b64 = k
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'-' | b'_'));
        if !b64 || k.len() != 43 {
            out.push(Issue::error(
                "identity.public_key",
                "must be the issuer's 32-byte public key in base64 (44 characters, from `dc-lobby-server identity`)",
            ));
        }
    }
    if cfg.auth.first_account_no == 0 {
        out.push(Issue::error("auth.first_account_no", "must be at least 1"));
    }
    let mut names = BTreeSet::new();
    for (i, c) in cfg.lobby.channels.iter().enumerate() {
        if c.name.trim().is_empty() || c.capacity == 0 || !names.insert(c.name.as_str()) {
            out.push(Issue::error(
                format!("lobby.channels[{i}]"),
                "needs a unique non-empty name and capacity > 0",
            ));
        }
    }
    for (i, c) in cfg.lobby.servers.iter().enumerate() {
        // DC-DEFAULT: LG-304 (proposed) an unknown feature is refused; another lobby's features are its own
        for f in &c.features {
            if c.is_remote() {
                out.push(Issue::error(
                    format!("lobby.servers[{i}].features"),
                    "is for an entry of this lobby only",
                ));
            } else if !crate::schema::SERVER_FEATURES.contains(&f.as_str()) {
                out.push(Issue::error(
                    format!("lobby.servers[{i}].features"),
                    format!(
                        "unknown feature '{f}'; known: {}",
                        crate::schema::SERVER_FEATURES.join(", ")
                    ),
                ));
            }
        }
        check_entry_mods(cfg, i, c, &mut out);
        if c.is_remote() {
            check_remote_server(i, c, &mut out);
            continue;
        }
        if c.name.trim().is_empty() || c.capacity == 0 {
            out.push(Issue::error(
                format!("lobby.servers[{i}]"),
                "needs a non-empty name and capacity > 0",
            ));
        }
        for (field, v) in [("fingerprint", &c.fingerprint), ("policy", &c.policy)] {
            if !v.trim().is_empty() {
                out.push(Issue::error(
                    format!("lobby.servers[{i}].{field}"),
                    "is for an entry naming another lobby (lobby = \"host:port\") only",
                ));
            }
        }
    }
    check_hosts(cfg, &tcp, &mut out);
    out
}

/// An entry's curated set (LG-313): only on an entry of a vanilla lobby with its web port on, well-formed references,
/// each id once.
// DC-DEFAULT: LG-313 (proposed)
fn check_entry_mods(cfg: &Config, i: usize, c: &LobbyServerCfg, out: &mut Vec<Issue>) {
    if c.mods.is_empty() {
        return;
    }
    let path = format!("lobby.servers[{i}].mods");
    if c.is_remote() {
        out.push(Issue::error(
            path.clone(),
            "is for an entry of this lobby only",
        ));
    }
    if cfg.mods.policy == "modded" {
        out.push(Issue::error(
            path.clone(),
            "needs mods.policy = \"vanilla\": a modded lobby's set covers every entry",
        ));
    }
    if !cfg.server.http_enabled {
        out.push(Issue::error(
            path.clone(),
            "needs server.http_enabled: clients download the set from the web port",
        ));
    }
    let mut seen = BTreeSet::new();
    for (j, m) in c.mods.iter().enumerate() {
        out.extend(check_mod_ref(&format!("{path}[{j}]"), m));
        if !seen.insert(m.id.as_str()) {
            out.push(Issue::error(
                format!("{path}[{j}].id"),
                format!("mod '{}' listed twice", m.id),
            ));
        }
    }
}

/// An entry naming another lobby instance (LG-272): a name, `host:port`, an optional SHA-256 pin, a known policy.
// DC-DEFAULT: LG-272 (proposed)
fn check_remote_server(i: usize, c: &LobbyServerCfg, out: &mut Vec<Issue>) {
    let path = format!("lobby.servers[{i}]");
    if c.name.trim().is_empty() {
        out.push(Issue::error(path.clone(), "needs a non-empty name"));
    }
    let port_ok = c.lobby.trim().rsplit_once(':').is_some_and(|(h, p)| {
        !h.trim_matches(['[', ']']).is_empty() && p.parse::<u16>().is_ok_and(|p| p != 0)
    });
    if !port_ok || c.lobby.chars().any(char::is_whitespace) {
        out.push(Issue::error(
            format!("{path}.lobby"),
            "must be host:port (the other lobby's auth port)",
        ));
    }
    let fp: String = c.fingerprint.chars().filter(|ch| *ch != ':').collect();
    if !fp.is_empty() && (fp.len() != 64 || !fp.chars().all(|ch| ch.is_ascii_hexdigit())) {
        out.push(Issue::error(
            format!("{path}.fingerprint"),
            "must be 64 hex digits (a SHA-256, colon pairs allowed) or empty",
        ));
    }
    if !matches!(c.policy.trim(), "" | "modded" | "vanilla") {
        out.push(Issue::error(
            format!("{path}.policy"),
            "must be \"modded\" or \"vanilla\"",
        ));
    }
    if !c.host.is_empty() {
        out.push(Issue::error(
            format!("{path}.host"),
            "an entry naming another lobby has no game host here (that lobby picks its own)",
        ));
    }
}

/// Shortest accepted agent token (HOST_AGENT.md §2).
pub const MIN_HOST_TOKEN: usize = 24;

/// Most channel ids the server entries may own together (the clan lobby's channels start at 101, CLN rows).
pub const MAX_CHANNELS: u32 = 64;

/// `hosts.*` and the `host` / `channels` of the server entries. DC-DEFAULT: CD-88 (proposed); SPEC: HOST_AGENT.md §2.
fn check_hosts(cfg: &Config, tcp: &[(&str, u32)], out: &mut Vec<Issue>) {
    let h = &cfg.hosts;
    let mut names = BTreeSet::new();
    for (i, a) in h.agents.iter().enumerate() {
        let path = format!("hosts.agents[{i}]");
        if a.name.trim().is_empty() || a.name.chars().any(|c| c.is_whitespace() || c.is_control()) {
            out.push(Issue::error(
                format!("{path}.name"),
                "must be a non-empty name without spaces",
            ));
        } else if !names.insert(a.name.as_str()) {
            out.push(Issue::error(
                format!("{path}.name"),
                format!("agent '{}' is listed twice", a.name),
            ));
        }
        if a.token.chars().count() < MIN_HOST_TOKEN {
            out.push(Issue::error(
                format!("{path}.token"),
                format!("must be at least {MIN_HOST_TOKEN} characters (a random secret)"),
            ));
        } else if a.token.chars().any(|c| c.is_whitespace() || c.is_control()) {
            out.push(Issue::error(
                format!("{path}.token"),
                "must not contain spaces (it travels as one hello token)",
            ));
        }
        if a.public_address.chars().any(|c| c.is_whitespace()) {
            out.push(Issue::error(
                format!("{path}.public_address"),
                "must be an IP address or host name without spaces, or empty",
            ));
        } else if !a.public_address.is_empty() && is_loopback(&a.public_address) {
            out.push(Issue::warning(
                format!("{path}.public_address"),
                "a loopback address reaches only players on the agent's own machine",
            ));
        }
    }
    let local = cfg.lobby.local_servers();
    for (i, e) in cfg.lobby.servers.iter().enumerate() {
        if e.is_remote() {
            continue;
        }
        if !e.host.is_empty() && !names.contains(e.host.as_str()) {
            out.push(Issue::error(
                format!("lobby.servers[{i}].host"),
                format!(
                    "'{}' names no [[hosts.agents]] entry; use \"\" for this machine or add the agent",
                    e.host
                ),
            ));
        }
        if local.len() > 1 && e.channels == 0 {
            out.push(Issue::error(
                format!("lobby.servers[{i}].channels"),
                "must be at least 1",
            ));
        }
    }
    if local.len() > 1 {
        let total: u32 = local.iter().map(|e| e.channels).sum();
        if total > MAX_CHANNELS {
            out.push(Issue::error(
                "lobby.servers",
                format!("the entries own {total} channels together; at most {MAX_CHANNELS}"),
            ));
        }
    }
    if h.agents.is_empty() {
        return;
    }
    let port = h.resolved_port(cfg.server.auth_port);
    if port == 0 || port > 65535 {
        out.push(Issue::error(
            "hosts.port",
            format!("resolves to {port}, outside 1..=65535; set it explicitly"),
        ));
    }
    for (name, p) in tcp {
        if *p == port {
            out.push(Issue::error(
                "hosts.port",
                format!("port {port} is already used by {name}"),
            ));
        }
    }
}

#[cfg(test)]
mod feature_tests {
    use super::*;

    /// LG-304: a known feature passes, an unknown one and one on another lobby's entry are refused.
    /// LG-313: an entry's set: a vanilla lobby's own entry with the web port on, good references, each id once.
    #[test]
    fn an_entry_set_is_checked() {
        use crate::schema::{LobbyServerCfg, ModRef};
        let r = |id: &str, sha: &str| ModRef {
            id: id.into(),
            version: "1.0.0".into(),
            sha256: sha.into(),
        };
        let sha = "ab".repeat(32);
        let errors = |f: &dyn Fn(&mut LobbyServerCfg, &mut Config)| {
            let mut cfg = Config::default();
            let mut e = LobbyServerCfg {
                name: "V".into(),
                capacity: 5,
                host: String::new(),
                channels: 3,
                features: Vec::new(),
                mods: vec![r("a", &sha)],
                lobby: String::new(),
                fingerprint: String::new(),
                policy: String::new(),
                region: String::new(),
            };
            f(&mut e, &mut cfg);
            cfg.lobby.servers = vec![e];
            crate::check(&cfg, CheckOptions::default())
                .into_iter()
                .filter(|i| i.path.contains(".mods") && i.severity == crate::Severity::Error)
                .map(|i| i.message)
                .collect::<Vec<_>>()
        };
        assert!(errors(&|_, _| {}).is_empty());
        assert!(errors(&|_, c| c.mods.policy = "modded".into())[0].contains("vanilla"));
        assert!(errors(&|_, c| c.server.http_enabled = false)[0].contains("http_enabled"));
        assert!(errors(&|e, _| e.mods = vec![r("a", "zz")])[0].contains("hex"));
        assert!(errors(&|e, _| e.mods = vec![r("a", &sha), r("a", &sha)])[0].contains("twice"));
        assert!(errors(&|e, _| e.lobby = "h:1".into())[0].contains("this lobby"));
    }

    /// LG-388: the managed bots' total cap.
    #[test]
    fn the_managed_bot_cap_is_checked() {
        let errors = |n: u32| {
            let mut cfg = Config::default();
            cfg.bots.managed = true;
            cfg.bots.managed_max_total = n;
            check(&cfg, CheckOptions::default())
                .into_iter()
                .filter(|i| i.path == "bots.managed_max_total")
                .count()
        };
        assert_eq!(
            (errors(0), errors(1), errors(21), errors(175), errors(176)),
            (1, 0, 0, 0, 1)
        );
        assert!(!Config::default().bots.managed, "off by default");
    }

    #[test]
    fn server_entry_features_are_checked() {
        let mut cfg = Config::default();
        let e = |features: Vec<String>, lobby: &str| crate::schema::LobbyServerCfg {
            name: "E".into(),
            capacity: 10,
            host: String::new(),
            channels: 3,
            features,
            mods: Vec::new(),
            lobby: lobby.into(),
            fingerprint: String::new(),
            policy: String::new(),
            region: String::new(),
        };
        let errors = |cfg: &Config| {
            check(cfg, CheckOptions::default())
                .into_iter()
                .filter(|i| i.path.ends_with(".features"))
                .count()
        };
        cfg.lobby.servers = vec![e(vec!["gun_game".into()], "")];
        assert_eq!(errors(&cfg), 0);
        cfg.lobby.servers = vec![e(vec!["gun_gam".into()], "")];
        assert_eq!(errors(&cfg), 1);
        cfg.lobby.servers = vec![e(vec!["gun_game".into()], "1.2.3.4:5")];
        assert_eq!(errors(&cfg), 1);
    }
}
