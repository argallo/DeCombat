//! `dc-lobby-server`: the DeCombat auth + lobby server core for the 2009 client (ROM model).
//!
//! Start-up order follows SPEC: LOBBY_HARNESS.md §2. Wire layouts: SPEC: PROTOCOL_2009.md. Game servers:
//! SPEC: DSERVER_PIPE.md. Server rules: DC_DEFAULTS.md (cited per rule as DC-DEFAULT: CD-nn).

mod accounts;
mod admin;
mod adminfwd;
mod adminweb;
mod announce;
mod auth;
mod beta;
mod betacli;
mod betaweb;
mod botgroups;
mod botpool;
mod botsup;
mod clan;
mod clanweb;
mod cli;
mod clock;
mod config;
mod coupons;
mod db;
mod delivery;
mod deploy;
mod diag;
mod doctor;
mod game;
mod gm;
mod grant;
mod gungame;
mod hisec;
mod hosting;
mod hosts;
mod identity;
mod inbox;
mod inventory;
mod itemuse;
mod lifecycle;
mod limits;
mod loadout;
mod lobby;
mod logs;
mod mail;
mod maps;
mod megaphone;
mod messenger;
mod modhost;
mod modset;
mod msg;
mod net;
mod notices;
mod nxrewards;
mod opscfg;
mod opsmon;
mod opsweb;
mod playeradmin;
mod pwhash;
mod pwpool;
mod recover;
mod register;
mod reportmail;
mod rewards;
mod rom;
mod room;
mod serverconf;
mod servers;
mod shop;
mod shopcfg;
mod shopmsg;
mod state;
mod tls;
mod views;
mod webauthn;
mod wire;

#[cfg(test)]
mod entry_mods_tests;
#[cfg(test)]
mod flow_tests;
#[cfg(test)]
mod gm_tests;
#[cfg(test)]
mod hisec_tests;
#[cfg(test)]
mod hosts_tests;
#[cfg(test)]
mod maps_tests;
#[cfg(test)]
mod shop_tests;

use std::net::TcpListener;
use std::sync::{Arc, Mutex};

fn fail(code: i32, msg: &str) -> ! {
    eprintln!("dc-lobby-server: {}", msg);
    std::process::exit(code)
}

/// The start-up line of the mod set's weapon content (MOD-21).
fn log_content(
    log: &logs::Logger,
    c: &dc_bute::modpatch::WeaponContent,
    path: Option<&std::path::Path>,
) {
    match path {
        Some(p) => log.line(&format!(
            "mods: content: {} weapon patches, {} ammo patches, hash {} -> {} (-content for every game server)",
            c.weapons.len(),
            c.ammo.len(),
            c.hash(),
            p.display()
        )),
        None => log.line("mods: content: no weapon patches"),
    }
}

fn main() {
    // 1. flags and derived defaults; `doctor` checks the set-up instead of starting the server
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let env: Vec<(String, String)> = std::env::vars().collect();
    let doctor_run = args.get(1).map(|a| a == "doctor").unwrap_or(false);
    // `account ...` administers the accounts of the DB file (LG-144)
    if args.get(1).map(|a| a == "account").unwrap_or(false) {
        let rest = args.iter().take(1).chain(args.iter().skip(2)).cloned();
        std::process::exit(accounts::run(rest));
    }
    // `ops` reads the operator monitor's tables (MON-2, MON-3)
    if args.get(1).map(|a| a == "ops").unwrap_or(false) {
        let rest: Vec<String> = args
            .iter()
            .skip(2)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        std::process::exit(opsmon::run_cli(&rest));
    }
    // `tls` prints the certificate's fingerprint (LG-222)
    if args.get(1).map(|a| a == "tls").unwrap_or(false) {
        let rest: Vec<String> = args
            .iter()
            .skip(2)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        std::process::exit(tls::run_subcommand(&rest));
    }
    // `identity` prints the public key of the identity key pair (LG-68)
    if args.get(1).map(|a| a == "identity").unwrap_or(false) {
        let rest: Vec<String> = args
            .iter()
            .skip(2)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        std::process::exit(identity::run_subcommand(&rest));
    }
    // `beta` lists the beta signups and mails invites and feedback requests (LG-376)
    if args.get(1).map(|a| a == "beta").unwrap_or(false) {
        let rest: Vec<String> = args
            .iter()
            .skip(2)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        std::process::exit(betacli::run(&rest, &env));
    }
    // `admin-web` makes the enrolment links of the admin web and lists or removes its security keys (LG-384)
    if args.get(1).map(|a| a == "admin-web").unwrap_or(false) {
        let rest: Vec<String> = args
            .iter()
            .skip(2)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        std::process::exit(adminweb::run_cli(&rest, &env));
    }
    // `reports` reads the diagnostic reports the lobby stored (LG-362)
    if args.get(1).map(|a| a == "reports").unwrap_or(false) {
        let rest: Vec<String> = args
            .iter()
            .skip(2)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        std::process::exit(diag::run_subcommand(&rest));
    }
    if doctor_run {
        let rest = args.iter().take(1).chain(args.iter().skip(2)).cloned();
        std::process::exit(doctor::run(rest, &env));
    }
    let (opts, conf) = match cli::parse_full(args, &env) {
        Ok(p) => (p.opts, p.conf),
        Err(cli::CliError::Usage(e)) => {
            let _ = e.print();
            std::process::exit(e.exit_code());
        }
        Err(cli::CliError::Config(errs)) => {
            for e in errs {
                eprintln!("dc-lobby-server: config: {}", e);
            }
            std::process::exit(2);
        }
    };
    // ROM tables (read-only); --grant-rank needs the rank table, a bad rank is a usage error
    let rom = match serverconf::rom_dir(conf.as_ref()) {
        Some(dir) => rom::RomData::load(&dir),
        None => rom::RomData::empty(),
    };
    if let Err(e) = grant::resolve(&opts, &rom.ranks) {
        fail(2, &e);
    }
    if let Err(e) = grant::new_character_exp(&opts, &rom.ranks) {
        fail(2, &e);
    }
    if let Err(e) = grant::start_loadout(&opts, &rom) {
        fail(2, &e);
    }
    // 2. logs
    let (log, log_errs) = logs::Logger::open(opts.log.as_deref(), opts.pktlog.as_deref());
    let log = Arc::new(log);
    for e in log_errs {
        log.line(&e);
    }
    for n in &rom.notes {
        log.line(n);
    }
    if rom.dir.is_none() {
        // LG-180: without ROM tables the starter kit has no melee / throwable / body defaults
        log.line("WARNING: no ROM folder (--rom, paths.rom or CA_ROM): the starter kit of new characters has no melee, throwable or body default items, and the login repair cannot give them to existing characters");
    }
    if let Some(c) = &conf {
        log.line(&format!(
            "config: {} (flag > DC_* environment > server.toml > lobby.json > built-in)",
            c.describe()
        ));
        for w in &c.warnings {
            // the config check sees only paths.rom; the folder may come from CA_ROM (serverconf::rom_dir order)
            match (w.path.as_str(), rom.dir.as_ref()) {
                ("paths.rom", Some(d)) => log.line(&format!(
                    "config: paths.rom is not set; the ROM folder in use is {} (from CA_ROM)",
                    d.display()
                )),
                _ => log.line(&format!("config: {}", w)),
            }
        }
    }
    // 3. DB with its schema
    let conn = match db::open(&opts.db) {
        Ok(c) => c,
        Err(e) => fail(
            1,
            &format!("cannot open the DB {}: {}", opts.db.display(), e),
        ),
    };
    let cfg_dir = opts
        .db
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let (mut rules, mut notes) = config::Rules::load(&cfg_dir);
    if let Some(c) = &conf {
        notes.extend(c.apply_rules(&mut rules));
    }
    notes.extend(serverconf::ServerConf::install_playable(&rules));
    for n in notes {
        log.line(&n);
    }
    let rom = Arc::new(rom);
    let mut st = state::Core::new(
        opts.clone(),
        rules,
        Arc::clone(&rom),
        Arc::clone(&log),
        conn,
    );
    // LG-245: the recovery mail sender (worker thread unless off or dry run)
    st.mail = mail::Mailer::start(&opts.mail, Arc::clone(&log));
    log.line(&st.mail.describe());
    // LG-68: identity tokens (issue with --identity, accept with --identity-issuer + --identity-key)
    match identity::from_options(&opts) {
        Ok((a, notes)) => {
            for n in notes {
                log.line(&n);
            }
            st.identity = a;
        }
        Err(e) => fail(2, &e),
    }
    // 4. test-bot accounts (LOCAL TEST ONLY): an account with a character named after the login
    for name in opts.test_bot_accounts.clone() {
        let r = st.scope(|st| {
            let login = auth::normalize_login(&name);
            let acc = auth::account_for(st, &login)?;
            if st.character(acc.user_no).is_none() {
                let nick = auth::auto_nick(st, acc.user_no, &login);
                auth::create_character(st, acc.user_no, &nick, 0)?;
            }
            Ok(())
        });
        if let Err(e) = r {
            log.line(&format!("test-bot account {}: {}", name, e));
        }
    }
    // 4b. the bot pool (LG-318)
    if let Err(e) = st.scope(crate::botpool::ensure) {
        log.line(&format!("bot pool: {}", e));
    }
    // 5. on-demand tables are part of the base schema here
    // 5b. --add-account NAME:PASSWORD (LG-32)
    for (name, pw) in opts.add_accounts.clone() {
        let r = st.scope(|st| {
            let no = auth::ensure_account(st, &name, pw.as_str())?;
            st.note(&format!(
                "account {} ready (account {}, password set)",
                name, no
            ));
            Ok(())
        });
        if let Err(e) = r {
            log.line(&format!("add-account {}: {}", name, e));
        }
    }
    // 6. --grant on an existing account
    if let Ok(Some(g)) = grant::resolve(&opts, &rom.ranks) {
        let r = st.scope(|st| {
            match grant::grant_target(&st.db, &g)? {
                Some(a) if grant::apply(&st.db, &g, a)? => {
                    st.note(&format!("grant applied to account {}", a))
                }
                _ => st.note(&format!(
                    "grant target '{}' has no character yet (applied at its creation)",
                    g.target
                )),
            }
            Ok(())
        });
        if let Err(e) = r {
            log.line(&format!("grant: {}", e));
        }
    }
    // 7. clan ranking (previous rank = rank at start-up; stale clan room sides dropped)
    if let Err(e) = st.scope(clan::startup) {
        log.line(&format!("clans: {}", e));
    }
    // 8. --demo
    if opts.demo {
        log.line("--demo: stand-in rooms/users are not part of the core server yet (flag accepted, ignored)");
    }
    // mods (server-hosted, sandboxed Lua)
    let mods_dir = serverconf::mods_dir(&opts, conf.as_ref());
    let mut server_list_doc = None;
    let mut server_list_culling_off: Option<String> = None;
    let mut server_list_no_recoil: Option<String> = None;
    let mut catalog = modset::Catalog::default();
    if let Some(c) = &conf {
        // the advertised mod set and its hash go to the server-list data next to the DB (HOSTING_AND_MODS.md §4 item 6)
        let built = modset::build(c, mods_dir.as_deref());
        for e in &built.errors {
            log.line(&format!("mods: ERROR {}", e));
        }
        for w in &built.warnings {
            log.line(&format!("mods: {}", w));
        }
        // MOD-25: the optional catalogue, on any policy; never loaded here, not part of the set or its hash
        catalog = modset::build_catalog(
            &c.cfg.mods.catalog,
            mods_dir.as_deref(),
            c.cfg.mods.max_zip_bytes,
        );
        for w in &catalog.notes {
            log.line(&format!("mods: catalog: {w}"));
        }
        // MOD-27: the operator's pictures of the set go out on the catalogue's picture route
        catalog
            .shots
            .extend(built.previews.iter().map(|(k, v)| (k.clone(), v.clone())));
        let list = cfg_dir.join("server_list.json");
        server_list_culling_off = built.set.full_visibility_mod().map(str::to_string);
        server_list_no_recoil = built.set.no_recoil_mod().map(str::to_string);
        // LG-271: a login must carry the published mod-set hash
        st.login_mods.set = built.set.hash();
        // MAP-35: the first community index depends on the ROM alone (the set's maps follow it), so it is known
        // before the set loads; the client takes it from here (MAP-32)
        let community_map_base = st.maps().first_community_index() as u32;
        let doc = modset::server_list_json(c, &opts, &built.set, community_map_base);
        server_list_doc = Some(doc.clone());
        match std::fs::write(&list, doc) {
            Ok(()) => log.line(&format!(
                "mods: policy {}, {} declared, mod-set hash {} (server-list data {})",
                if built.set.modded {
                    "modded"
                } else {
                    "vanilla"
                },
                built.set.mods.len(),
                built.set.hash(),
                list.display()
            )),
            Err(e) => log.line(&format!("mods: cannot write {}: {}", list.display(), e)),
        }
    }
    if let Some(c) = &conf {
        // LG-270: with a server.toml the lobby loads exactly its declared set, in order, or does not start
        let cache = cfg_dir.join("mod_cache");
        let declared = match modset::resolve_declared(c, mods_dir.as_deref(), &cache) {
            Ok(d) => d,
            Err(errs) => fail(
                1,
                &format!(
                    "mods: the declared mod set cannot be loaded (LG-270): {}",
                    errs.join("; ")
                ),
            ),
        };
        if c.cfg.mods.policy == "modded" {
            match modhost::ModClient::start_declared(
                declared,
                Arc::clone(&log),
                Some(c.mod_policy()),
            ) {
                Ok(mut m) => {
                    log.line(&format!(
                        "mods: {} loaded in the declared order: {:?}",
                        m.loaded.len(),
                        m.loaded
                    ));
                    // MOD-21 / DSV-478: the set's weapon content goes to every game server as `-content`
                    match m.write_content(&cfg_dir) {
                        Ok(p) => log_content(&log, &m.content, p.as_deref()),
                        Err(e) => fail(1, &format!("mods: content: {e} (MOD-21)")),
                    }
                    // MOD-22: world.full_visibility in the loaded set turns every match's culling off
                    if let Some(id) = server_list_culling_off.clone() {
                        log.line(&format!(
                            "mods: culling off (mod {id} declares world.full_visibility)"
                        ));
                        m.culling_off_by = Some(id);
                    }
                    // MOD-23 / LG-274: combat.no_recoil in the loaded set lets a room turn recoil off
                    if let Some(id) = server_list_no_recoil.clone() {
                        log.line(&format!(
                            "mods: rooms may turn recoil off (mod {id} declares combat.no_recoil)"
                        ));
                        m.no_recoil_by = Some(id);
                    }
                    // MAP-35: the set's community maps join the map table after the ROM's rows
                    let maps = std::mem::take(&mut m.maps);
                    st.mods = Some(m);
                    for n in st.install_community_maps(maps) {
                        log.line(&n);
                    }
                }
                Err(e) => fail(1, &format!("mods: {} (LG-270)", e)),
            }
        } else if let Some(dir) = &mods_dir {
            log.line(&format!(
                "mods: 0 loaded: policy vanilla refused every mod under {}",
                dir.display()
            ));
        }
    } else if let Some(dir) = &mods_dir {
        log.line(&format!(
            "mods: no server.toml mod set; loading every mod folder under {} (folder discovery fallback)",
            dir.display()
        ));
        match modhost::ModClient::start(dir, Arc::clone(&log), None) {
            Some(mut m) => {
                log.line(&format!(
                    "mods: {} loaded from {}: {:?}",
                    m.loaded.len(),
                    dir.display(),
                    m.loaded
                ));
                match m.write_content(&cfg_dir) {
                    Ok(p) => log_content(&log, &m.content, p.as_deref()),
                    Err(e) => log.line(&format!("mods: content not written: {e}")),
                }
                // MAP-39: without a declared set no server list is published, so every login is vanilla and no
                // client could fetch a map mod's world: the discovered maps are not offered
                let maps = std::mem::take(&mut m.maps);
                if !maps.is_empty() {
                    log.line(
                        "mods: maps ignored: community maps need a declared set so clients can fetch the mod",
                    );
                }
                st.mods = Some(m);
            }
            None => log.line("mods: the mod thread could not start"),
        }
    }
    // LG-271 / MOD-21 amendment: the client sends the hash of the content of the mods it loads (those with
    // `requires_client`), not of the `server_rules` patches only the game servers get; `0` without any
    if let Some(m) = st.mods.as_ref().filter(|m| !m.client_content.is_empty()) {
        st.login_mods.content = m.client_content.hash();
    }
    log.line(&format!(
        "mods: a login must carry mod-set hash {}… and content hash {}… (LG-271)",
        dc_wire::loginmods::short(&st.login_mods.set),
        dc_wire::loginmods::short(&st.login_mods.content)
    ));
    // LG-313: the curated sets of the featured server entries, each with its own mod host; the lobby's policy stays
    if conf.as_ref().is_some_and(|c| c.cfg.mods.policy != "modded") {
        // MAP-81: every entry's community rows start where the lobby-wide table's would (MAP-35)
        let community_map_base = st.maps().first_community_index() as u32;
        st.entry_mods = modset::build_entry_sets(
            &st.opts.servers,
            mods_dir.as_deref(),
            &cfg_dir,
            conf.as_ref()
                .map_or(hosting::DEFAULT_MAX_MOD_BYTES, |c| c.cfg.mods.max_zip_bytes),
            community_map_base,
            &log,
        );
        for entry in st.entry_mods.keys() {
            for n in st.map_notes(Some(*entry)) {
                log.line(&n);
            }
        }
    }
    let entry_docs: std::collections::BTreeMap<u32, String> = st
        .entry_mods
        .iter()
        .map(|(id, e)| (*id, e.doc.clone()))
        .collect();
    let community_maps = st.community_maps.len();
    let shared: state::Shared = Arc::new(Mutex::new(st));
    // 8b. TLS on the auth and web ports (LG-220..226, our own deviation from 2009)
    let gate = tls_gate(&opts, &log);
    // 9. three listeners on --bind
    let bind = |port: u16| match TcpListener::bind((opts.bind.as_str(), port)) {
        Ok(l) => l,
        Err(e) => fail(
            1,
            &format!("cannot listen on {}:{}: {}", opts.bind, port, e),
        ),
    };
    let auth_l = bind(opts.port);
    let lobby_l = bind(opts.lobbyport);
    let clan_l = bind(opts.clanport);
    net::spawn_listener_tls(
        Arc::clone(&shared),
        auth_l,
        logs::Listener::Auth,
        Some(Arc::clone(&gate)),
    );
    net::spawn_listener(Arc::clone(&shared), lobby_l, logs::Listener::Lobby);
    net::spawn_listener(Arc::clone(&shared), clan_l, logs::Listener::Clan);
    // 9b. the host port of the remote game hosts (LG-300): only when an agent is configured, TLS only
    if !opts.host_agents.is_empty() {
        match &gate.identity {
            Some(id) => {
                hosts::spawn_listener(
                    Arc::clone(&shared),
                    bind(opts.hosts_port),
                    Arc::new(hosts::gate(Arc::clone(id))),
                );
                log.line(&format!(
                    "hosts: host port {} (TLS only) for {}",
                    opts.hosts_port,
                    opts.host_agents
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            None => log.line(
                "hosts: ERROR the lobby has no certificate, so the host port stays closed and no remote game host can connect",
            ),
        }
    }
    // 10. clan web pages (CD-47) on --clanweb, bound to --bind
    if opts.clanweb != 0 {
        let Ok(web_port) = u16::try_from(opts.clanweb) else {
            fail(2, &format!("--clanweb {} is not a port", opts.clanweb))
        };
        let max_mod_bytes = conf
            .as_ref()
            .map(|c| c.cfg.mods.max_zip_bytes)
            .unwrap_or(hosting::DEFAULT_MAX_MOD_BYTES);
        // also serves /mods/<sha256>.zip and /server_list.json (HOSTING_AND_MODS.md §4 items 3, 6)
        let hosting = hosting::Hosting {
            mods_dir: mods_dir.clone(),
            server_list: server_list_doc,
            catalog: catalog.doc.take(),
            catalog_shots: std::mem::take(&mut catalog.shots),
            max_mod_bytes,
            entry_sets: entry_docs,
            client_version: conf
                .as_ref()
                .and_then(|c| hosting::client_version_doc(&c.cfg.client)),
            folder_packs: Default::default(),
        };
        // diagnostic reports (LG-360..362): the upload endpoint, the UDP echo of the connection test, the mail pump
        diag::configure(opts.db.parent().unwrap_or(std::path::Path::new("")));
        let web_l = bind(web_port);
        clanweb::spawn_with_tls(Arc::clone(&shared), web_l, hosting, Some(Arc::clone(&gate)));
        diag::spawn_echo(&opts.bind, web_port, Arc::clone(&log));
        diag::spawn_background(Arc::clone(&shared));
        if opts.register {
            log.line(&format!(
                "registration page on port {} at /register ({})",
                web_port,
                if opts.register_open {
                    "open"
                } else {
                    "invite codes required"
                }
            ));
        }
    } else if opts.register {
        log.line("--register: the web port is off (--clanweb 0), so there is no registration page");
    }
    // 10b. operator endpoint (LG-131): loopback unless --admin-public with --admin-token
    if let Some(spec) = &opts.admin {
        let addr = match admin::resolve(spec, opts.admin_public, opts.admin_token.as_deref()) {
            Ok(a) => a,
            Err(e) => fail(2, &e),
        };
        match TcpListener::bind(addr) {
            Ok(l) => {
                log.line(&format!(
                    "admin endpoint on http://{} (GET /status, POST /stop, POST /rooms/<ch>-<no>/close){}",
                    addr,
                    if opts.admin_token.is_some() {
                        "; token required"
                    } else {
                        ""
                    }
                ));
                admin::spawn(Arc::clone(&shared), l, opts.admin_token.clone());
            }
            Err(e) => fail(1, &format!("cannot listen on {} (--admin): {}", addr, e)),
        }
    }
    // 10c. admin web (LG-382): the operator pages behind hardware security keys, for a Cloudflare Tunnel
    if opts.admin_web.enabled {
        match adminweb::bind(&opts.admin_web) {
            Ok((addr, l)) => {
                log.line(&format!(
                    "admin web on http://{} for {} (security keys only; dashboard and beta list)",
                    addr, opts.admin_web.public_origin
                ));
                let web =
                    adminweb::Runtime::shared(opts.admin_web.clone(), webauthn::Trust::yubico());
                adminweb::spawn(Arc::clone(&shared), web, l);
            }
            Err(e) => fail(1, &e),
        }
    }
    // 11. periodic item expiry
    net::spawn_expiry(Arc::clone(&shared));
    // LG-316: the report mails waiting in the DB
    net::spawn_report_mail(Arc::clone(&shared));
    net::spawn_beta_mail_drain(Arc::clone(&shared));
    // LG-388: the dashboard's bot groups as child processes (only with bots.managed)
    botsup::spawn(&shared);
    // megaphone queue (CD-40)
    megaphone::spawn_ticker(Arc::clone(&shared));
    // 12. start-up line
    log.line(&format!(
        "dc-lobby-server: auth {} lobby {} clan {} on {} (advertised lobby ip {}); db {}; game {}:{}; ROM maps {} modes {} clan modes {}; community maps {}",
        opts.port,
        opts.lobbyport,
        opts.clanport,
        opts.bind,
        opts.lobbyip,
        opts.db.display(),
        opts.gameip,
        opts.gameport,
        rom.map_count(),
        rom.mode_count(false),
        rom.mode_count(true),
        community_maps
    ));
    // 13. run until Ctrl+C / console close / SIGTERM / POST /stop, then the graceful stop (LG-130)
    match lifecycle::install_stop_handler() {
        Ok(()) => log.line(&format!(
            "running; Ctrl+C stops the server (game servers get {:.0} s to end their match)",
            opts.stop_grace
        )),
        Err(e) => log.line(&format!("stop handler: {} (Ctrl+C kills the process)", e)),
    }
    lifecycle::wait_for_stop();
    lifecycle::shutdown(&shared, opts.stop_grace)
}

/// The certificate and the gate of the auth and web ports. A public lobby (or a non-loopback bind) without a usable
/// certificate does not start unless `--plain-auth-public` (LG-224).
fn tls_gate(opts: &cli::Options, log: &logs::Logger) -> Arc<tls::Gate> {
    let mode = tls::TlsMode::parse(&opts.tls).unwrap_or(tls::TlsMode::Auto);
    let public = opts.public || !tls::is_loopback_bind(&opts.bind);
    let dir = opts
        .tls_dir
        .clone()
        .unwrap_or_else(|| tls::default_dir(&opts.db));
    let names = [opts.bind.clone(), opts.lobbyip.clone(), opts.gameip.clone()];
    let identity = match tls::load_or_create(&dir, &names) {
        Ok(id) => Some(Arc::new(id)),
        Err(e) if (public && !opts.plain_auth_public) || mode == tls::TlsMode::Loopback => fail(
            1,
            &format!(
                "TLS: {e}; a lobby that requires TLS cannot start without its certificate (LG-224)"
            ),
        ),
        Err(e) => {
            log.line(&format!("TLS: {e}; the auth and web ports are plain only"));
            None
        }
    };
    let gate = tls::Gate::new(identity, mode, public, opts.plain_auth_public);
    if let Some(id) = &gate.identity {
        for l in tls::startup_lines(id, &gate) {
            log.line(&l);
        }
    }
    if public && opts.plain_auth_public {
        for _ in 0..3 {
            log.line("WARNING: --plain-auth-public: remote players may log in WITHOUT encryption; their passwords cross the network in clear text. Use it only for the real 2009 client on a network you trust.");
        }
    }
    Arc::new(gate)
}
