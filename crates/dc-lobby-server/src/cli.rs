//! Command line: exactly the flags, types and defaults of `specs/LOBBY_HARNESS.md` section 1, plus
//! `--mods-dir`, resolved in the order that section prescribes.
// SPEC: LOBBY_HARNESS.md §1 (flag table and resolution order)

use crate::serverconf::ServerConf;
use clap::parser::ValueSource;
use clap::{Arg, ArgAction, ArgMatches, Command};
use std::path::{Path, PathBuf};

/// Fully resolved command line (defaults applied, derived ports filled in).
#[derive(Clone, Debug)]
pub struct Options {
    pub port: u16,
    pub lobbyport: u16,
    pub clanport: u16,
    pub lobbyip: String,
    pub bind: String,
    pub db: PathBuf,
    pub log: Option<PathBuf>,
    pub pktlog: Option<PathBuf>,
    pub gameip: String,
    pub gameport: u16,
    pub port_per_room: bool,
    pub countdown: f64,
    pub dserver: bool,
    pub dserver_exe: PathBuf,
    pub dserver_world: String,
    pub dserver_guid: String,
    pub dserver_idle: i64,
    pub dserver_timeout: f64,
    pub dserver_args: String,
    pub dserver_logdir: Option<PathBuf>,
    pub dserver_verbose: bool,
    pub test_bot_accounts: Vec<String>,
    pub require_ready: bool,
    pub new_exp: i64,
    /// Raw `--start-rank` text (rank number or name); when set it replaces `--start-exp` once the ROM is open.
    pub start_rank: String,
    /// Raw `--start-loadout` text (`<slot>:<item>,...` or bare item numbers); resolved by `grant::start_loadout`.
    pub start_loadout: String,
    /// Raw `--start-items` text: item numbers a new player character owns, not equipped (DC-DEFAULT: CD-99).
    pub start_items: String,
    /// Raw `--bot-start-loadout` text (same syntax); replaces `start_loadout` for new accounts named `Bot<digits>` in any case (LG-7, LG-9).
    pub bot_start_loadout: String,
    /// LG-260 `--bot-kits random|off`: a new bot account gets a random GP primary and secondary of the ROM shop.
    pub bot_kits_random: bool,
    /// LG-260 `--bot-faces random|off`: a new bot account gets a random face of the creation table.
    pub bot_faces_random: bool,
    pub new_gp: i64,
    pub new_nx: i64,
    /// `--start-nx-credit`: the NX credit balance of new characters (`--start-nx` is the prepaid balance).
    pub new_nx_credit: i64,
    pub server_max: i64,
    /// CD-21: the server list's names: the first `lobby.servers` entry (else `server.name`, else "DeCombat") and
    /// the clan entry (the second `lobby.servers` entry, else "<name> Clan").
    pub server_name: String,
    pub clan_name: String,
    /// CD-21 (amended), CD-88: the configured `[[lobby.servers]]` entries (empty = the implicit single entry).
    pub servers: Vec<dc_config::LobbyServerCfg>,
    /// LG-272: the `[[lobby.servers]]` entries naming other lobby instances (`lobby = "host:port"`): our clients
    /// list them after this lobby's own entries (push 0x3f02).
    pub other_lobbies: Vec<dc_config::LobbyServerCfg>,
    /// CD-88: the host port the agents dial (`hosts.port`, 0 = port + 5, already resolved).
    pub hosts_port: u16,
    /// CD-88: the remote game hosts allowed to connect (`[[hosts.agents]]`); empty = no host port.
    pub host_agents: Vec<dc_config::HostAgentCfg>,
    pub channel_max: i64,
    /// Resolved clan web port: `port + 8080` when the flag was negative, 0 = off.
    pub clanweb: i64,
    pub delay: f64,
    pub lenient: bool,
    pub demo: bool,
    pub pw_check: bool,
    pub autochar: bool,
    pub grant: String,
    /// Raw `--grant-rank` text; resolved against the rank table once the ROM is open.
    pub grant_rank: String,
    pub grant_exp: Option<i64>,
    pub grant_gp: Option<i64>,
    pub grant_nx: Option<i64>,
    pub grant_nx_credit: Option<i64>,
    pub grant_force: bool,
    pub mods_dir: Option<PathBuf>,
    /// `--public`: every interface; loopback logins skip the account gate (LG-31, LG-32).
    pub public: bool,
    /// An unknown login creates its account (CD-14); `--no-auto-create` / `auth.auto_create = false` refuses it.
    pub auto_create: bool,
    /// `--add-account NAME:PASSWORD`: accounts created (or their password reset) at start-up (LG-32).
    pub add_accounts: Vec<(String, String)>,
    /// `--admin ADDR:PORT` (or a bare port on 127.0.0.1): the operator HTTP endpoint (LG-131); None = off.
    pub admin: Option<String>,
    /// `--admin-public`: allow a non-loopback `--admin` address (needs `--admin-token`).
    pub admin_public: bool,
    /// `--admin-token`: shared secret every admin request must carry (header or `?token=`).
    pub admin_token: Option<String>,
    /// `--stop-grace`: seconds the graceful stop waits for the game servers before it kills them (LG-130).
    pub stop_grace: f64,
    /// `--register` / `--register-open`: the registration page on the web port (LG-145); open = no invite code needed.
    pub register: bool,
    pub register_open: bool,
    /// `--tls auto|loopback` (LG-224, LG-225); checked by `parse_full`.
    pub tls: String,
    /// `--tls-dir`: the folder of the certificate + key (LG-221); None = the DB's folder.
    pub tls_dir: Option<PathBuf>,
    /// `--plain-auth-public`: a public lobby also takes plain logins from remote peers (LG-224; a loud warning).
    pub plain_auth_public: bool,
    /// `[mail]` / `--mail-*`: the password-recovery mail (LG-245); the password is hidden in the Debug form.
    pub mail: crate::mail::MailOpts,
    /// `[beta]`: the beta signup by e-mail (LG-370..LG-379); off by default.
    pub beta: dc_config::BetaSection,
    /// `[admin_web]`: the operator pages behind hardware security keys (LG-382..LG-387); off by default.
    pub admin_web: dc_config::AdminWebSection,
    /// `bots.managed`: this lobby runs the dashboard's bot groups as child processes (LG-388); off by default.
    pub bots_managed: bool,
    /// `bots.managed_max_total`: most managed bots of all groups together (LG-388; default 21).
    pub bots_max_total: u32,
    /// `[deploy] dir`: the deploy folder of tools/vps/deploy_setup.sh (LG-392); None = no deploys from the dashboard.
    pub deploy_dir: Option<PathBuf>,
    /// `--identity` / `identity.issue`: this lobby issues identity tokens (LG-68).
    pub identity_issue: bool,
    /// `--identity-issuer` / `identity.issuer`: the own (issuing) or trusted (accepting) issuer id.
    pub identity_issuer: String,
    /// `--identity-key` / `identity.public_key`: the trusted issuer's public key (base64); not secret.
    pub identity_public_key: String,
    /// `--identity-token-hours` / `identity.token_hours`: token lifetime (default 24).
    pub identity_token_hours: u32,
    /// `--identity-key-file` / `identity.key_file`: the issuing key pair; None = `identity_key.pem` by the DB.
    pub identity_key_file: Option<PathBuf>,
}

/// The DB path without `--db`: `<exe dir>/data/ca.db`.
pub fn default_db() -> PathBuf {
    exe_dir().join("data").join("ca.db")
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn opt(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .help(help)
        .action(ArgAction::Set)
        .allow_negative_numbers(true)
}

fn flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .help(help)
        .action(ArgAction::SetTrue)
}

/// The clap command. `--flag value` and `--flag=value` both parse; an unknown flag exits with status 2.
pub fn command() -> Command {
    Command::new("dc-lobby-server")
        .about("DeCombat clean-room lobby / auth server (2009 client protocol)")
        .after_long_help(
            "SETTINGS PRECEDENCE (highest first): CLI > env > server.toml > lobby.json > built-in.
             A command-line flag beats a DC_* environment variable, which beats the server.toml given by
             --config / DC_CONFIG, which beats <db dir>/lobby.json (rule values only), which beats the
             built-in default. `dc-lobby-server doctor` prints what was resolved.",
        )
        .infer_long_args(true)
        .arg(opt("port", "auth port (the client's -authport)").default_value("10000").value_parser(clap::value_parser!(u16)))
        .arg(opt("lobbyport", "lobby port; 0 = port + 2").default_value("0").value_parser(clap::value_parser!(u16)))
        .arg(opt("clanport", "lobby port of the clan server entry; 0 = lobbyport + 1").default_value("0").value_parser(clap::value_parser!(u16)))
        .arg(opt("lobbyip", "address advertised in the server list").default_value("127.0.0.1"))
        .arg(opt("bind", "listen address of every socket").default_value("127.0.0.1"))
        .arg(flag("public", "listen on 0.0.0.0 (every interface; overrides --bind); refused while --lobbyip or --gameip is loopback"))
        .arg(opt("db", "SQLite file (default <exe dir>/data/ca.db)"))
        .arg(opt("log", "append-mode UTF-8 log").default_value(""))
        .arg(opt("pktlog", "append-mode hex packet log").default_value(""))
        .arg(opt("gameip", "game-server address in the game-start packet").default_value("127.0.0.1"))
        .arg(opt("gameport", "game-server port in the game-start packet").default_value("27888").value_parser(clap::value_parser!(u16)))
        .arg(flag("port-per-room", "game port = gameport + channel*100 + room"))
        .arg(opt("countdown", "seconds between countdown ticks").default_value("1.0").value_parser(clap::value_parser!(f64)))
        .arg(flag("dserver", "start a dedicated server per room on Start"))
        .arg(opt("dserver-exe", "dedicated server executable (.py runs through Python)"))
        .arg(opt("dserver-world", "world for every room; empty = from the room map").default_value(""))
        // the identity token is passed through to the game server, which validates it (SPEC: DSERVER_PIPE.md §2 -guid)
        .arg(opt("dserver-guid", "network identity passed to the game server").default_value("ca"))
        .arg(opt("dserver-idle", "server stops after N s without clients; 0 = never, negative = 0").default_value("60").value_parser(clap::value_parser!(i64)))
        .arg(opt("dserver-timeout", "seconds to wait for @ready").default_value("30").value_parser(clap::value_parser!(f64)))
        // argparse takes a value with a leading '-' and a space ("-fps 30") as a value; so do we
        .arg(opt("dserver-args", "extra server arguments (whitespace split)").default_value("").allow_hyphen_values(true))
        .arg(opt("dserver-logdir", "per-room server logs and working directory").default_value(""))
        .arg(flag("dserver-verbose", "copy server console lines into the lobby log"))
        .arg(
            Arg::new("test-bot-account")
                .long("test-bot-account")
                .help("LOCAL TEST ONLY: register this account (repeatable)")
                .action(ArgAction::Append),
        )
        .arg(flag("require-ready", "refuse Start until every guest is ready"))
        .arg(opt("start-exp", "starting experience of new characters").default_value("0").value_parser(clap::value_parser!(i64)))
        .arg(opt("start-rank", "starting rank of new characters (number 0..55 or name); overrides --start-exp").default_value(""))
        .arg(opt("start-loadout", "equipped items of new characters: <slot>:<item>,... or <item> (slot from the ROM)").default_value(""))
        .arg(opt("start-items", "items a new player character owns in the inventory, not equipped: <item>,...").default_value(""))
        .arg(opt("bot-kits", "random (default): a new bot account (name ending in bot<digits>) gets a random GP primary and secondary of the ROM shop; off: the starter kit (LG-260)").default_value("random"))
        .arg(opt("bot-faces", "random (default): a new bot account gets a random face of the creation table; off: the face the bot asked for (LG-260)").default_value("random"))
        .arg(opt("bot-start-loadout", "equipped items of new accounts named Bot<digits>, any case (same syntax as --start-loadout); empty = they get --start-loadout").default_value(""))
        .arg(opt("start-gp", "starting GP of new characters").default_value("10000").value_parser(clap::value_parser!(i64)))
        .arg(opt("start-nx", "starting NX (prepaid) of new characters").default_value("0").value_parser(clap::value_parser!(i64)))
        .arg(opt("start-nx-credit", "starting NX credit of new characters").default_value("0").value_parser(clap::value_parser!(i64)))
        .arg(opt("server-max", "capacity shown in the server list").default_value("1000").value_parser(clap::value_parser!(i64)))
        .arg(opt("channel-max", "channel capacity; 0 = 200").default_value("0").value_parser(clap::value_parser!(i64)))
        .arg(opt("clanweb", "clan web port; -1 = port + 8080, 0 = off").default_value("-1").value_parser(clap::value_parser!(i32)))
        .arg(opt("delay", "seconds before answering the server pick").default_value("0").value_parser(clap::value_parser!(f64)))
        .arg(flag("lenient", "accept a lobby login with an unknown session key"))
        .arg(flag("demo", "stand-in rooms/users for single-client tests"))
        .arg(flag("no-auto-create", "refuse an unknown login instead of creating its account (auth.auto_create = false); with --public loopback logins are exempt"))
        .arg(opt("add-account", "NAME:PASSWORD: create this account (or reset its password) at start-up; repeatable").action(ArgAction::Append))
        .arg(flag("check-password", "first login sets the password, later logins must match"))
        .arg(flag("register", "serve the registration page /register on the web port (--clanweb); an invite code is needed"))
        .arg(flag("register-open", "like --register, but no invite code is needed"))
        .arg(flag("autochar", "new accounts get a character named after the login"))
        .arg(opt("grant", "login or nick of an account to raise").default_value(""))
        .arg(opt("grant-rank", "rank number 0..55 or rank name").default_value(""))
        .arg(opt("grant-exp", "experience to grant").value_parser(clap::value_parser!(i64)))
        .arg(opt("grant-gp", "GP to grant").value_parser(clap::value_parser!(i64)))
        .arg(opt("grant-nx", "NX prepaid and credit to grant").value_parser(clap::value_parser!(i64)))
        .arg(opt("grant-nx-credit", "NX credit to grant (overrides --grant-nx for the credit balance)").value_parser(clap::value_parser!(i64)))
        .arg(flag("grant-force", "set the grant values exactly (default: only raise)"))
        .arg(opt("mods-dir", "directory of server-hosted mod packages"))
        .arg(opt("config", "server.toml (or DC_CONFIG); precedence: CLI > env > server.toml > lobby.json > built-in"))
        .arg(opt("admin", "operator HTTP endpoint ADDR:PORT or PORT (127.0.0.1); GET /status, POST /stop, POST /rooms/<ch>-<no>/close").default_value(""))
        .arg(flag("admin-public", "allow a non-loopback --admin address (requires --admin-token)"))
        .arg(opt("admin-token", "shared secret for the admin endpoint (header X-Admin-Token, Authorization: Bearer, or ?token=)").default_value(""))
        .arg(opt("tls", "auto: the auth and web ports require TLS from non-loopback peers of a public lobby (plain stays for loopback); loopback: TLS required from every peer").default_value("auto"))
        .arg(opt("tls-dir", "folder of the lobby's certificate + key (tls_cert.pem / tls_key.pem, or fullchain.pem / privkey.pem); default: the DB's folder"))
        .arg(flag("plain-auth-public", "UNSAFE: a public lobby also accepts plain (unencrypted) logins from remote players, e.g. the real 2009 client"))
        .arg(opt("mail-smtp-host", "SMTP server of the password-recovery mail (mail.smtp_host); empty = recovery off"))
        .arg(opt("mail-smtp-port", "SMTP port: 587 STARTTLS (default), 465 TLS (mail.smtp_port)").value_parser(clap::value_parser!(u16)))
        .arg(opt("mail-user", "SMTP login (mail.user)"))
        .arg(opt("mail-password", "SMTP password (mail.password); better: the DC_MAIL__PASSWORD environment variable"))
        .arg(opt("mail-from", "sender address of the recovery mails (mail.from)"))
        .arg(opt("mail-subject-prefix", "text in front of the mail subjects (mail.subject_prefix, default [DeCombat])"))
        .arg(opt("mail-reports-to", "operator's address for the in-game reports (mail.reports_to); empty = they stay in the DB"))
        .arg(opt("mail-reports-per-hour", "report mails per hour for the whole lobby (mail.reports_per_hour, default 20)").value_parser(clap::value_parser!(u32)))
        .arg(flag("mail-dry-run", "write the recovery mails to the log instead of sending them (mail.dry_run)"))
        .arg(flag("identity", "issue identity tokens (identity.issue): POST /identity/token on the web port; needs --identity-issuer"))
        .arg(opt("identity-issuer", "identity issuer id: this lobby's own with --identity, else the trusted issuer whose tokens log in (identity.issuer)"))
        .arg(opt("identity-key", "base64 public key of the trusted identity issuer (identity.public_key); `dc-lobby-server identity` prints it on the issuer"))
        .arg(opt("identity-token-hours", "lifetime of issued identity tokens in hours (identity.token_hours, default 24)").value_parser(clap::value_parser!(u32)))
        .arg(opt("identity-key-file", "the issuer's Ed25519 key pair (identity.key_file); default identity_key.pem in the DB's folder"))
        .arg(opt("stop-grace", "seconds a graceful stop (Ctrl+C, POST /stop) waits for the game servers before killing them").default_value("10").value_parser(clap::value_parser!(f64)))
}

fn s(m: &ArgMatches, k: &str) -> String {
    m.get_one::<String>(k).cloned().unwrap_or_default()
}

fn path_opt(m: &ArgMatches, k: &str) -> Option<PathBuf> {
    let v = s(m, k);
    if v.is_empty() {
        None
    } else {
        Some(PathBuf::from(v))
    }
}

/// True when the flag was typed on the command line (not a clap default).
fn given(m: &ArgMatches, id: &str) -> bool {
    m.value_source(id) == Some(ValueSource::CommandLine)
}

/// Flag if typed, else the `server.toml` / environment value when the operator set its path, else the flag's
/// built-in default. Precedence is documented in `serverconf.rs`.
fn pick<T>(m: &ArgMatches, id: &str, cli: T, conf: Option<T>) -> T {
    if given(m, id) {
        cli
    } else {
        conf.unwrap_or(cli)
    }
}

/// The config value at `path` through `f`, when the operator set that path.
fn cv<T>(conf: Option<&ServerConf>, path: &str, f: impl Fn(&dc_config::Config) -> T) -> Option<T> {
    conf.filter(|c| c.ex.has(path)).map(|c| f(&c.cfg))
}

fn nonempty_path(s: &str) -> Option<PathBuf> {
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// Builds [`Options`] from parsed matches and applies the derived defaults (lobbyport, clanport, clanweb).
#[cfg(test)]
pub fn resolve(m: &ArgMatches) -> Options {
    resolve_with(m, None)
}

/// [`resolve`] with a `server.toml` layer underneath the flags.
pub fn resolve_with(m: &ArgMatches, conf: Option<&ServerConf>) -> Options {
    let port = pick(
        m,
        "port",
        *m.get_one::<u16>("port").unwrap_or(&10000),
        cv(conf, "server.auth_port", |c| c.server.auth_port),
    );
    let mut lobbyport = pick(
        m,
        "lobbyport",
        *m.get_one::<u16>("lobbyport").unwrap_or(&0),
        cv(conf, "server.lobby_port", |c| c.server.lobby_port),
    );
    if lobbyport == 0 {
        lobbyport = port.wrapping_add(2);
    }
    let mut clanport = pick(
        m,
        "clanport",
        *m.get_one::<u16>("clanport").unwrap_or(&0),
        cv(conf, "server.clan_port", |c| c.server.clan_port),
    );
    if clanport == 0 {
        clanport = lobbyport.wrapping_add(1);
    }
    let clanweb_cli = *m.get_one::<i32>("clanweb").unwrap_or(&-1) as i64;
    let clanweb_conf = conf.and_then(|c| {
        if c.ex.has("server.http_enabled") && !c.cfg.server.http_enabled {
            Some(0)
        } else if c.ex.has("clan.web_port") && c.cfg.clan.web_port != 0 {
            Some(c.cfg.clan.web_port as i64)
        } else if c.ex.has("server.http_port") {
            Some(if c.cfg.server.http_port == 0 {
                -1
            } else {
                c.cfg.server.http_port as i64
            })
        } else {
            None
        }
    });
    let clanweb_raw = pick(m, "clanweb", clanweb_cli, clanweb_conf);
    let clanweb = if clanweb_raw < 0 {
        port as i64 + 8080
    } else {
        clanweb_raw
    };
    let dir = exe_dir();
    let advertise = cv(conf, "server.advertise_ip", |c| {
        c.server.advertise_ip.clone()
    });
    let bind_conf = conf
        .filter(|c| c.ex.has("server.bind") || c.ex.has("server.public"))
        .map(|c| c.cfg.effective_bind().to_string());
    let db_conf = cv(conf, "paths.db", |c| nonempty_path(&c.paths.db)).flatten();
    let log_conf = cv(conf, "paths.log_dir", |c| nonempty_path(&c.paths.log_dir))
        .flatten()
        .map(|d| d.join("dc-lobby-server.log"));
    let mods_conf = cv(conf, "paths.mods_dir", |c| nonempty_path(&c.paths.mods_dir)).flatten();
    let mut dserver_args = s(m, "dserver-args");
    if let Some(hz) = cv(conf, "dserver.tick_hz", |c| c.dserver.tick_hz) {
        // the game server reads its world tick rate from -tickhz (CD-63); a flag-supplied -tickhz wins
        if !dserver_args.to_ascii_lowercase().contains("-tickhz") {
            if !dserver_args.is_empty() {
                dserver_args.push(' ');
            }
            dserver_args.push_str(&format!("-tickhz {hz}"));
        }
    }
    let mail_def = dc_config::MailSection::default();
    let mail_s = |id: &str, path: &str, f: fn(&dc_config::Config) -> String, def: String| {
        if given(m, id) {
            s(m, id)
        } else {
            cv(conf, path, f).unwrap_or(def)
        }
    };
    let mail = crate::mail::MailOpts {
        smtp_host: mail_s(
            "mail-smtp-host",
            "mail.smtp_host",
            |c| c.mail.smtp_host.clone(),
            mail_def.smtp_host.clone(),
        ),
        smtp_port: m
            .get_one::<u16>("mail-smtp-port")
            .copied()
            .or_else(|| cv(conf, "mail.smtp_port", |c| c.mail.smtp_port))
            .unwrap_or(mail_def.smtp_port),
        user: mail_s(
            "mail-user",
            "mail.user",
            |c| c.mail.user.clone(),
            mail_def.user.clone(),
        ),
        password: mail_s(
            "mail-password",
            "mail.password",
            |c| c.mail.password.clone(),
            mail_def.password.clone(),
        ),
        from: mail_s(
            "mail-from",
            "mail.from",
            |c| c.mail.from.clone(),
            mail_def.from.clone(),
        ),
        subject_prefix: mail_s(
            "mail-subject-prefix",
            "mail.subject_prefix",
            |c| c.mail.subject_prefix.clone(),
            mail_def.subject_prefix.clone(),
        ),
        dry_run: m.get_flag("mail-dry-run")
            || cv(conf, "mail.dry_run", |c| c.mail.dry_run).unwrap_or(false),
        reports_to: mail_s(
            "mail-reports-to",
            "mail.reports_to",
            |c| c.mail.reports_to.clone(),
            mail_def.reports_to.clone(),
        ),
        reports_per_hour: m
            .get_one::<u32>("mail-reports-per-hour")
            .copied()
            .or_else(|| cv(conf, "mail.reports_per_hour", |c| c.mail.reports_per_hour))
            .unwrap_or(mail_def.reports_per_hour),
    };
    let id_s = |id: &str, path: &str, f: fn(&dc_config::Config) -> String| {
        if given(m, id) {
            s(m, id)
        } else {
            cv(conf, path, f).unwrap_or_default()
        }
    };
    let identity_key_file = nonempty_path(&id_s("identity-key-file", "identity.key_file", |c| {
        c.identity.key_file.clone()
    }));
    // CD-21 (amended 2026-10-31, user request "name the server US Central"): the list's names from the config
    let server_name = cv(conf, "lobby.servers", |c| {
        c.lobby.local_servers().first().map(|e| e.name.clone())
    })
    .flatten()
    .or_else(|| cv(conf, "server.name", |c| c.server.name.clone()))
    .filter(|n| !n.trim().is_empty())
    .unwrap_or_else(|| "DeCombat".to_string());
    // CD-21 (amended for HOST_AGENT.md): every configured entry is a server of its own, so the clan entry is always
    // "<name> Clan"
    let clan_name = format!("{server_name} Clan");
    // LG-272: an entry naming another lobby is no server of this one
    let servers = cv(conf, "lobby.servers", |c| c.lobby.local_servers()).unwrap_or_default();
    let other_lobbies = cv(conf, "lobby.servers", |c| c.lobby.remote_servers()).unwrap_or_default();
    let host_agents = cv(conf, "hosts.agents", |c| c.hosts.agents.clone()).unwrap_or_default();
    let hosts_port = cv(conf, "hosts.port", |c| c.hosts.port)
        .filter(|&p| p != 0)
        .unwrap_or_else(|| port.wrapping_add(5));
    Options {
        mail,
        beta: cv(conf, "beta", |c| c.beta.clone()).unwrap_or_default(),
        admin_web: cv(conf, "admin_web", |c| c.admin_web.clone()).unwrap_or_default(),
        bots_managed: cv(conf, "bots.managed", |c| c.bots.managed).unwrap_or(false),
        bots_max_total: cv(conf, "bots.managed_max_total", |c| c.bots.managed_max_total)
            .unwrap_or(dc_config::BotsSection::default().managed_max_total),
        deploy_dir: cv(conf, "deploy.dir", |c| c.deploy.dir.trim().to_string())
            .and_then(|d| nonempty_path(&d)),
        identity_issue: m.get_flag("identity")
            || cv(conf, "identity.issue", |c| c.identity.issue).unwrap_or(false),
        identity_issuer: id_s("identity-issuer", "identity.issuer", |c| {
            c.identity.issuer.clone()
        }),
        identity_public_key: id_s("identity-key", "identity.public_key", |c| {
            c.identity.public_key.clone()
        }),
        identity_token_hours: m
            .get_one::<u32>("identity-token-hours")
            .copied()
            .or_else(|| cv(conf, "identity.token_hours", |c| c.identity.token_hours))
            .unwrap_or(crate::identity::DEFAULT_TOKEN_HOURS),
        identity_key_file,
        server_name,
        clan_name,
        servers,
        other_lobbies,
        hosts_port,
        host_agents,
        port,
        lobbyport,
        clanport,
        lobbyip: pick(m, "lobbyip", s(m, "lobbyip"), advertise.clone()),
        bind: if m.get_flag("public") {
            "0.0.0.0".to_string()
        } else {
            pick(m, "bind", s(m, "bind"), bind_conf)
        },
        db: pick(
            m,
            "db",
            path_opt(m, "db").unwrap_or_else(|| dir.join("data").join("ca.db")),
            db_conf,
        ),
        log: if given(m, "log") {
            path_opt(m, "log")
        } else {
            log_conf.or_else(|| path_opt(m, "log"))
        },
        pktlog: path_opt(m, "pktlog"),
        gameip: pick(m, "gameip", s(m, "gameip"), advertise),
        gameport: pick(
            m,
            "gameport",
            *m.get_one::<u16>("gameport").unwrap_or(&27888),
            cv(conf, "server.game_port", |c| c.server.game_port),
        ),
        port_per_room: m.get_flag("port-per-room"),
        countdown: pick(
            m,
            "countdown",
            *m.get_one::<f64>("countdown").unwrap_or(&1.0),
            cv(conf, "room.countdown_ms", |c| {
                c.room.countdown_ms as f64 / 1000.0
            }),
        ),
        dserver: m.get_flag("dserver"),
        dserver_exe: path_opt(m, "dserver-exe").unwrap_or_else(|| default_dserver_exe(&dir)),
        dserver_world: s(m, "dserver-world"),
        dserver_guid: s(m, "dserver-guid"),
        dserver_idle: m
            .get_one::<i64>("dserver-idle")
            .copied()
            .unwrap_or(60)
            .max(0),
        dserver_timeout: *m.get_one::<f64>("dserver-timeout").unwrap_or(&30.0),
        dserver_args,
        dserver_logdir: path_opt(m, "dserver-logdir"),
        dserver_verbose: m.get_flag("dserver-verbose"),
        test_bot_accounts: m
            .get_many::<String>("test-bot-account")
            .map(|v| v.cloned().collect())
            .unwrap_or_default(),
        require_ready: m.get_flag("require-ready"),
        new_exp: *m.get_one::<i64>("start-exp").unwrap_or(&0),
        start_rank: s(m, "start-rank"),
        start_loadout: s(m, "start-loadout"),
        start_items: s(m, "start-items"),
        bot_start_loadout: s(m, "bot-start-loadout"),
        bot_kits_random: s(m, "bot-kits") != "off",
        bot_faces_random: s(m, "bot-faces") != "off",
        new_gp: *m.get_one::<i64>("start-gp").unwrap_or(&10000),
        new_nx: *m.get_one::<i64>("start-nx").unwrap_or(&0),
        new_nx_credit: *m.get_one::<i64>("start-nx-credit").unwrap_or(&0),
        server_max: pick(
            m,
            "server-max",
            *m.get_one::<i64>("server-max").unwrap_or(&1000),
            cv(conf, "lobby.servers", |c| {
                c.lobby.local_servers().first().map(|e| e.capacity as i64)
            })
            .flatten(),
        ),
        channel_max: pick(
            m,
            "channel-max",
            *m.get_one::<i64>("channel-max").unwrap_or(&0),
            cv(conf, "lobby.channels", |c| {
                c.lobby.channels.first().map(|e| e.capacity as i64)
            })
            .flatten(),
        ),
        clanweb,
        delay: *m.get_one::<f64>("delay").unwrap_or(&0.0),
        lenient: m.get_flag("lenient"),
        demo: m.get_flag("demo"),
        pw_check: m.get_flag("check-password"),
        autochar: m.get_flag("autochar"),
        grant: s(m, "grant"),
        grant_rank: s(m, "grant-rank"),
        grant_exp: m.get_one::<i64>("grant-exp").copied(),
        grant_gp: m.get_one::<i64>("grant-gp").copied(),
        grant_nx: m.get_one::<i64>("grant-nx").copied(),
        grant_nx_credit: m.get_one::<i64>("grant-nx-credit").copied(),
        grant_force: m.get_flag("grant-force"),
        public: m.get_flag("public"),
        auto_create: !m.get_flag("no-auto-create")
            && cv(conf, "auth.auto_create", |c| c.auth.auto_create).unwrap_or(true),
        register: m.get_flag("register") || m.get_flag("register-open"),
        register_open: m.get_flag("register-open"),
        tls: s(m, "tls"),
        tls_dir: path_opt(m, "tls-dir"),
        plain_auth_public: m.get_flag("plain-auth-public"),
        add_accounts: m
            .get_many::<String>("add-account")
            .map(|v| v.filter_map(|x| account_spec(x).ok()).collect())
            .unwrap_or_default(),
        admin: Some(s(m, "admin")).filter(|v| !v.trim().is_empty()),
        admin_public: m.get_flag("admin-public"),
        admin_token: Some(s(m, "admin-token")).filter(|v| !v.is_empty()),
        stop_grace: m
            .get_one::<f64>("stop-grace")
            .copied()
            .unwrap_or(10.0)
            .clamp(0.0, 3600.0),
        mods_dir: if given(m, "mods-dir") {
            path_opt(m, "mods-dir")
        } else {
            mods_conf.or_else(|| path_opt(m, "mods-dir"))
        },
    }
}

/// Why a command line could not be turned into [`Options`].
pub enum CliError {
    /// clap usage error (exit status 2).
    Usage(clap::Error),
    /// `server.toml` / environment problems, one line each (exit status 2).
    Config(Vec<String>),
}

/// Options plus the `server.toml` layer they were resolved against.
pub struct Parsed {
    pub opts: Options,
    pub conf: Option<ServerConf>,
}

/// Parses `args` (including argv[0]) and loads the `server.toml` layer named by `--config` / `DC_CONFIG`
/// (or implied by `DC_*__*` variables in `env`).
pub fn parse_full<I, T>(args: I, env: &[(String, String)]) -> Result<Parsed, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let m = command()
        .try_get_matches_from(args)
        .map_err(CliError::Usage)?;
    let flag = path_opt(&m, "config");
    let conf = ServerConf::load(flag.as_deref(), env).map_err(CliError::Config)?;
    let opts = resolve_with(&m, conf.as_ref());
    let bad: Vec<String> = m
        .get_many::<String>("add-account")
        .map(|v| v.filter_map(|x| account_spec(x).err()).collect())
        .unwrap_or_default();
    if !bad.is_empty() {
        return Err(CliError::Config(bad));
    }
    if let Err(e) = crate::tls::TlsMode::parse(&opts.tls) {
        return Err(CliError::Config(vec![e]));
    }
    if m.get_flag("public") {
        let refused = public_refusals(&opts);
        if !refused.is_empty() {
            return Err(CliError::Config(refused));
        }
    }
    Ok(Parsed { opts, conf })
}

/// `--add-account NAME:PASSWORD` -> (login, password): split at the first ':', both parts non-empty.
// DC-DEFAULT: LG-32 pre-created accounts for a public server
pub fn account_spec(v: &str) -> Result<(String, String), String> {
    match v.split_once(':') {
        Some((n, p)) if !n.trim().is_empty() && !p.is_empty() => {
            Ok((n.trim().to_string(), p.to_string()))
        }
        _ => Err(format!(
            "--add-account '{v}': give NAME:PASSWORD (both non-empty)"
        )),
    }
}

/// `--public` with a loopback `--lobbyip` / `--gameip`: remote players would be told to connect to themselves. The
/// rule is dc-config's `server.advertise_ip` finding (a warning in `dc-config check`), a refusal for `--public`.
// DC-DEFAULT: LG-31 `--public` refuses a loopback announced address
pub fn public_refusals(opts: &Options) -> Vec<String> {
    let mut out: Vec<String> = [("--lobbyip", &opts.lobbyip), ("--gameip", &opts.gameip)]
        .into_iter()
        .filter_map(|(flag, ip)| {
            dc_config::public_loopback_advertise(true, &opts.bind, ip)
                .map(|i| format!("--public with {flag} {ip}: {}", i.message))
        })
        .collect();
    // DC-DEFAULT: LG-325 (proposed) --lenient lets a lobby login name any account without a key: never on a public lobby
    if opts.lenient {
        out.push(
            "--public with --lenient: anybody could log in as any account; remove --lenient".into(),
        );
    }
    out
}

/// Parses `args` (including argv[0]) without a `server.toml` layer.
#[cfg(test)]
pub fn parse_from<I, T>(args: I) -> Result<Options, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let m = command().try_get_matches_from(args)?;
    Ok(resolve(&m))
}

/// The game server the lobby starts without `--dserver-exe`: `dc-dserver` (with the platform's executable suffix)
/// next to the lobby binary (the cargo target folder and the local-play packs). The path is returned even when the
/// file is missing, so starting the game server reports it.
// DC-DEFAULT: LG-126 (proposed)
pub fn default_dserver_exe(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(format!("dc-dserver{}", std::env::consts::EXE_SUFFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_binds_every_interface_and_refuses_a_loopback_address() {
        let env: Vec<(String, String)> = Vec::new();
        let ok = parse_full(
            [
                "t",
                "--public",
                "--lobbyip",
                "203.0.113.7",
                "--gameip",
                "203.0.113.7",
                "--bind",
                "127.0.0.1",
            ],
            &env,
        )
        .ok()
        .unwrap();
        assert_eq!(ok.opts.bind, "0.0.0.0");
        match parse_full(["t", "--public", "--gameip", "203.0.113.7"], &env) {
            Err(CliError::Config(v)) => {
                assert_eq!(v.len(), 1, "{v:?}");
                assert!(v[0].contains("--lobbyip 127.0.0.1"), "{v:?}");
            }
            _ => panic!("a loopback --lobbyip with --public must be refused"),
        }
        assert!(parse_full(["t", "--public"], &env).is_err());
        // without --public a loopback address is the normal local setup
        assert_eq!(parse_full(["t"], &env).ok().unwrap().opts.bind, "127.0.0.1");
    }

    #[test]
    fn add_account_and_auto_create_flags() {
        let env: Vec<(String, String)> = Vec::new();
        let p = parse_full(
            [
                "t",
                "--add-account",
                "Friend:s3cr:et",
                "--add-account",
                "b:x",
                "--no-auto-create",
            ],
            &env,
        )
        .ok()
        .unwrap();
        assert_eq!(
            p.opts.add_accounts,
            vec![
                ("Friend".to_string(), "s3cr:et".to_string()),
                ("b".to_string(), "x".to_string())
            ]
        );
        assert!(!p.opts.auto_create);
        assert!(parse_full(["t"], &env).ok().unwrap().opts.auto_create);
        assert!(matches!(
            parse_full(["t", "--add-account", "nopass"], &env),
            Err(CliError::Config(_))
        ));
        assert!(parse_full(["t", "--add-account", "x:"], &env).is_err());
    }

    #[test]
    fn a_negative_idle_is_clamped_to_zero() {
        // SPEC: DSERVER_PIPE addendum e2e B1
        assert_eq!(
            parse_from(["x", "--dserver-idle", "-3"])
                .unwrap()
                .dserver_idle,
            0
        );
        assert_eq!(
            parse_from(["x", "--dserver-idle", "7"])
                .unwrap()
                .dserver_idle,
            7
        );
    }

    #[test]
    fn help_states_the_precedence_order() {
        let h = command().render_long_help().to_string();
        assert!(
            h.contains("CLI > env > server.toml > lobby.json > built-in"),
            "{h}"
        );
    }

    #[test]
    fn defaults_follow_the_harness_spec() {
        let o = parse_from(["x"]).unwrap();
        assert_eq!((o.port, o.lobbyport, o.clanport), (10000, 10002, 10003));
        assert_eq!(o.clanweb, 18080);
        assert_eq!(o.gameport, 27888);
        assert_eq!(o.countdown, 1.0);
        assert_eq!(o.new_gp, 10000);
        assert_eq!((o.new_exp, o.new_nx), (0, 0));
        assert_eq!(o.server_max, 1000);
        assert_eq!(o.dserver_guid, "ca");
        assert_eq!((o.dserver_idle, o.dserver_timeout), (60, 30.0));
        assert!(o.log.is_none() && o.pktlog.is_none() && o.mods_dir.is_none());
        assert!(o.db.ends_with(Path::new("data").join("ca.db")));
    }

    #[test]
    fn derived_ports_resolve_in_order() {
        let o = parse_from(["x", "--port", "20000", "--lobbyport=21000"]).unwrap();
        assert_eq!((o.lobbyport, o.clanport, o.clanweb), (21000, 21001, 28080));
        let o = parse_from(["x", "--port", "20000", "--clanweb", "0", "--clanport", "5"]).unwrap();
        assert_eq!((o.lobbyport, o.clanport, o.clanweb), (20002, 5, 0));
    }

    #[test]
    fn every_harness_flag_parses() {
        let o = parse_from([
            "x",
            "--port",
            "1",
            "--lobbyport",
            "2",
            "--clanport",
            "3",
            "--clanweb",
            "4",
            "--db",
            "d.db",
            "--countdown",
            "0.05",
            "--start-nx",
            "5000",
            "--dserver",
            "--dserver-exe",
            "f.py",
            "--dserver-world",
            "Worlds\\SnowValley",
            "--dserver-logdir",
            "t",
            "--gameport",
            "7",
            "--log",
            "l",
            "--check-password",
            "--autochar",
            "--grant",
            "gina",
            "--grant-rank",
            "General Of The Army",
            "--grant-gp",
            "999999",
            "--grant-nx",
            "999999",
            "--grant-force",
            "--test-bot-account",
            "a",
            "--test-bot-account",
            "b",
            "--require-ready",
            "--lenient",
            "--demo",
            "--delay",
            "0.5",
            "--port-per-room",
            "--dserver-verbose",
            "--dserver-args",
            "-fps 30",
            "--dserver-guid",
            "other",
            "--channel-max",
            "9",
            "--server-max",
            "8",
            "--start-exp",
            "1",
            "--start-gp",
            "2",
            "--grant-exp",
            "3",
            "--pktlog",
            "p",
            "--mods-dir",
            "m",
            "--bind",
            "0.0.0.0",
            "--lobbyip",
            "1.2.3.4",
            "--gameip",
            "5.6.7.8",
            "--dserver-idle",
            "5",
            "--dserver-timeout",
            "2.5",
        ])
        .unwrap();
        assert_eq!(o.test_bot_accounts, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(o.grant_rank, "General Of The Army");
        assert_eq!(o.clanweb, 4);
        assert!(o.dserver && o.port_per_room && o.grant_force && o.demo);
        assert_eq!(o.mods_dir, Some(PathBuf::from("m")));
    }

    #[test]
    fn unknown_flag_is_a_usage_error() {
        let e = parse_from(["x", "--no-such-flag"]).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        let e = parse_from(["x", "--port", "notanumber"]).unwrap_err();
        assert_eq!(e.exit_code(), 2);
    }

    /// LG-245: the [mail] keys, DC_MAIL__* and the --mail-* flags; the password stays out of the Debug form.
    #[test]
    fn mail_settings_layer_and_hide_the_password() {
        let o = with(&[], None);
        assert!(!o.mail.configured());
        assert_eq!(
            (o.mail.smtp_port, o.mail.subject_prefix.as_str()),
            (587, "[DeCombat]")
        );
        let c = conf(
            "[mail]\nsmtp_host = \"smtp.example.org\"\nsmtp_port = 465\nuser = \"lobby\"\nfrom = \"noreply@example.org\"\n",
            &[("DC_MAIL__PASSWORD", "s3cret-smtp")],
        );
        let o = with(&[], c.as_ref());
        assert_eq!(o.mail.smtp_host, "smtp.example.org");
        assert_eq!(o.mail.smtp_port, 465);
        assert_eq!(o.mail.password, "s3cret-smtp");
        assert!(o.mail.configured() && !o.mail.dry_run);
        assert!(!format!("{o:?}").contains("s3cret"));
        let o = with(
            &[
                "--mail-smtp-port",
                "587",
                "--mail-from",
                "x@y.zz",
                "--mail-dry-run",
            ],
            c.as_ref(),
        );
        assert_eq!(
            (o.mail.smtp_port, o.mail.from.as_str(), o.mail.dry_run),
            (587, "x@y.zz", true)
        );
    }

    #[test]
    fn negative_clanweb_means_port_plus_8080() {
        let o = parse_from(["x", "--port", "100", "--clanweb", "-1"]).unwrap();
        assert_eq!(o.clanweb, 8180);
    }

    fn conf(toml: &str, env: &[(&str, &str)]) -> Option<ServerConf> {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let (cfg, ex) =
            dc_config::load_layered_explicit(Some(toml), env, &dc_config::CliOverrides::default())
                .unwrap();
        Some(ServerConf {
            cfg,
            ex,
            path: None,
            warnings: Vec::new(),
        })
    }

    fn with(args: &[&str], c: Option<&ServerConf>) -> Options {
        let m = command()
            .try_get_matches_from(std::iter::once("x").chain(args.iter().copied()))
            .unwrap();
        resolve_with(&m, c)
    }

    #[test]
    fn flag_beats_env_beats_toml_beats_default() {
        let c = conf(
            "[server]
auth_port = 12000
game_port = 30000
advertise_ip = \"10.1.1.1\"
bind = \"127.0.0.2\"
[room]
countdown_ms = 500
",
            &[("DC_SERVER__GAME_PORT", "31000")],
        );
        let o = with(&[], c.as_ref());
        assert_eq!(
            (o.port, o.lobbyport, o.clanport, o.clanweb),
            (12000, 12002, 12003, 20080),
            "derived ports follow the toml auth port"
        );
        assert_eq!(
            (
                o.gameport,
                o.lobbyip.as_str(),
                o.gameip.as_str(),
                o.bind.as_str(),
                o.countdown
            ),
            (31000, "10.1.1.1", "10.1.1.1", "127.0.0.2", 0.5)
        );
        let o = with(
            &[
                "--port",
                "13000",
                "--lobbyip",
                "5.5.5.5",
                "--countdown",
                "2",
            ],
            c.as_ref(),
        );
        assert_eq!(
            (o.port, o.lobbyip.as_str(), o.gameip.as_str(), o.countdown),
            (13000, "5.5.5.5", "10.1.1.1", 2.0)
        );
        // nothing set: the built-in values, and a config without paths leaves the default DB location
        let o = with(
            &[],
            conf(
                "[room]
max_players_cap = 8
",
                &[],
            )
            .as_ref(),
        );
        let d = with(&[], None);
        assert_eq!(
            (o.port, o.gameport, o.countdown, o.bind.clone()),
            (d.port, d.gameport, d.countdown, d.bind.clone())
        );
        assert_eq!(o.db, d.db);
    }

    #[test]
    fn env_tick_reaches_the_dserver_args_next_to_the_flag_args() {
        // the VPS case: the toml has no tick, the service environment sets it, the unit passes -bindip
        let c = conf(
            "[dserver]
port_range = \"27016-27100\"
",
            &[("DC_DSERVER__TICK_HZ", "30")],
        );
        let o = with(
            &["--port", "20000", "--dserver-args=-bindip 0.0.0.0"],
            c.as_ref(),
        );
        assert_eq!(o.dserver_args, "-bindip 0.0.0.0 -tickhz 30");
    }

    /// LG-272: an entry naming another lobby is no server of this lobby: the names, the capacity and the channel
    /// blocks come from the other entries; the entry goes to `other_lobbies`. A bad entry fails the check.
    #[test]
    fn an_entry_naming_another_lobby_is_listed_but_owns_nothing_here() {
        let toml = "[[lobby.servers]]
name = \"Modded\"
lobby = \"203.0.113.7:11000\"
fingerprint = \"ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB\"
policy = \"modded\"
region = \"US Central\"
[[lobby.servers]]
name = \"US Central\"
capacity = 55
";
        let c = conf(toml, &[]);
        let o = with(&["--port", "20000"], c.as_ref());
        assert_eq!((o.server_name.as_str(), o.server_max), ("US Central", 55));
        assert_eq!(
            o.servers
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["US Central"]
        );
        assert_eq!(o.other_lobbies.len(), 1);
        assert_eq!(o.other_lobbies[0].lobby, "203.0.113.7:11000");
        let cfg: dc_config::Config = toml::from_str(toml).unwrap();
        let errors = |cfg: &dc_config::Config| {
            dc_config::check(cfg, dc_config::CheckOptions::default())
                .into_iter()
                .filter(|i| {
                    i.severity == dc_config::Severity::Error && i.path.starts_with("lobby.servers")
                })
                .map(|i| i.path)
                .collect::<Vec<_>>()
        };
        assert!(errors(&cfg).is_empty(), "{:?}", errors(&cfg));
        let mut bad = cfg.clone();
        bad.lobby.servers[0].lobby = "no-port".into();
        bad.lobby.servers[0].fingerprint = "xyz".into();
        bad.lobby.servers[1].policy = "modded".into();
        assert_eq!(
            errors(&bad),
            [
                "lobby.servers[0].lobby",
                "lobby.servers[0].fingerprint",
                "lobby.servers[1].policy"
            ]
        );
    }

    #[test]
    fn toml_ports_capacity_web_and_tick_reach_the_options() {
        let c = conf(
            "[server]
http_port = 0
[[lobby.servers]]
name = \"A\"
capacity = 55
[[lobby.channels]]
name = \"C\"
capacity = 66
[dserver]
tick_hz = 30
[paths]
mods_dir = \"m\"
db = \"x.db\"
",
            &[],
        );
        let o = with(&["--port", "20000"], c.as_ref());
        assert_eq!(
            (o.server_max, o.channel_max, o.clanweb),
            (55, 66, 28080),
            "http_port = 0 means port + 8080"
        );
        assert_eq!(
            (o.server_name.as_str(), o.clan_name.as_str()),
            ("A", "A Clan"),
            "CD-21: the list's names from the first lobby.servers entry"
        );
        assert_eq!(o.dserver_args, "-tickhz 30");
        assert_eq!(
            (o.mods_dir, o.db),
            (Some(PathBuf::from("m")), PathBuf::from("x.db"))
        );
        let o = with(
            &[
                "--dserver-args",
                "-tickhz 5 -open",
                "--server-max",
                "9",
                "--clanweb",
                "0",
            ],
            c.as_ref(),
        );
        assert_eq!(
            (o.dserver_args.as_str(), o.server_max, o.clanweb),
            ("-tickhz 5 -open", 9, 0)
        );
        let off = conf(
            "[server]
http_enabled = false
",
            &[],
        );
        assert_eq!(
            with(&[], off.as_ref()).clanweb,
            0,
            "http_enabled = false turns the web port off"
        );
        let web = conf(
            "[clan]
web_port = 4444
",
            &[],
        );
        assert_eq!(with(&[], web.as_ref()).clanweb, 4444);
    }

    #[test]
    fn servers_and_hosts_reach_the_options() {
        // CD-21 amended, CD-88 (proposed)
        let c = conf(
            "[[lobby.servers]]
name = \"US Central\"
capacity = 100
[[lobby.servers]]
name = \"US West\"
capacity = 200
host = \"us-west\"
channels = 2
[[hosts.agents]]
name = \"us-west\"
token = \"0123456789abcdef01234567\"
",
            &[],
        );
        let o = with(&["--port", "20000"], c.as_ref());
        assert_eq!(
            (o.server_name.as_str(), o.clan_name.as_str(), o.server_max),
            ("US Central", "US Central Clan", 100),
            "the second entry is a server of its own, the clan entry is the first name's"
        );
        assert_eq!(o.servers.len(), 2);
        assert_eq!(
            (o.servers[1].host.as_str(), o.servers[1].channels),
            ("us-west", 2)
        );
        assert_eq!(o.host_agents.len(), 1);
        assert_eq!(o.hosts_port, 20005, "hosts.port 0 = port + 5");
        let c = conf("[hosts]\nport = 4000\n", &[]);
        assert_eq!(with(&[], c.as_ref()).hosts_port, 4000);
        let o = with(&[], None);
        assert!(o.servers.is_empty() && o.host_agents.is_empty());
    }

    #[test]
    fn config_flag_is_accepted_and_missing_file_is_a_config_error() {
        let o = parse_from(["x", "--config", "nowhere.toml"]).unwrap();
        assert_eq!(
            o.port, 10000,
            "parse_from ignores the file; parse_full loads it"
        );
        match parse_full(["x", "--config", "nowhere.toml"], &[]) {
            Err(CliError::Config(e)) => assert!(e[0].contains("nowhere.toml"), "{e:?}"),
            _ => panic!("expected a config error"),
        }
        assert!(parse_full(["x"], &[]).ok().unwrap().conf.is_none());
    }

    // LG-126: without --dserver-exe the lobby starts the dc-dserver next to itself when there is one
    #[test]
    fn the_default_game_server_is_the_sibling_dc_dserver_when_present() {
        let dir = std::env::temp_dir().join(format!("dc_lobby_cli_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sibling = dir.join(format!("dc-dserver{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(default_dserver_exe(&dir), sibling);
        std::fs::write(&sibling, b"").unwrap();
        assert_eq!(default_dserver_exe(&dir), sibling);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The ExecStart words of a systemd unit: whitespace split, a double-quoted word kept whole (the subset of
    /// systemd's quoting examples/decombat-lobby.service uses).
    fn exec_start_words(unit: &str) -> Vec<String> {
        let line = unit
            .lines()
            .find_map(|l| l.strip_prefix("ExecStart="))
            .expect("ExecStart= line");
        let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
        for ch in line.chars() {
            match ch {
                '"' => quoted = !quoted,
                c if c.is_whitespace() && !quoted => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    // docs/VPS.md: the VPS unit's command line with examples/server-vps.toml resolves to the documented setup, and
    // without the advertised address from decombat.env the --public start is refused (LG-31)
    #[test]
    fn vps_unit_and_example_config_resolve_to_the_public_setup() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let unit = std::fs::read_to_string(root.join("examples/decombat-lobby.service")).unwrap();
        let toml = root.join("examples/server-vps.toml");
        let mut args = exec_start_words(&unit);
        assert_eq!(args[0], "/opt/decombat/bin/dc-lobby-server");
        let i = args.iter().position(|a| a == "--config").unwrap();
        assert_eq!(args[i + 1], "/opt/decombat/config/server.toml");
        args[i + 1] = toml.to_string_lossy().into_owned();
        let env = vec![(
            "DC_SERVER__ADVERTISE_IP".to_string(),
            "203.0.113.7".to_string(),
        )];
        let o = match parse_full(args.clone(), &env) {
            Ok(p) => p.opts,
            Err(CliError::Config(e)) => panic!("{e:?}"),
            Err(CliError::Usage(e)) => panic!("{e}"),
        };
        assert_eq!(o.bind, "0.0.0.0");
        assert!(
            o.public && o.pw_check && !o.auto_create && o.register && !o.register_open && o.dserver
        );
        assert_eq!(
            (o.lobbyip.as_str(), o.gameip.as_str()),
            ("203.0.113.7", "203.0.113.7")
        );
        assert_eq!(
            (o.port, o.lobbyport, o.clanport, o.clanweb),
            (10000, 10002, 10003, 18080)
        );
        assert_eq!(o.gameport, 27015);
        assert!(
            o.dserver_args.contains("-bindip 0.0.0.0"),
            "{}",
            o.dserver_args
        );
        assert_eq!(
            crate::game::bindip_for(&o.gameip, &o.dserver_args),
            None,
            "the operator's -bindip wins over the advertised address"
        );
        assert_eq!(o.db, std::path::PathBuf::from("/opt/decombat/data/ca.db"));
        assert_eq!(
            o.log,
            Some(std::path::Path::new("/opt/decombat/logs").join("dc-lobby-server.log"))
        );
        assert_eq!(o.admin.as_deref(), Some("127.0.0.1:10090"));
        assert!(!o.admin_public && o.admin_token.is_none());
        assert_eq!(o.stop_grace, 10.0);
        // no address in decombat.env: the toml's loopback address stays and --public refuses it
        match parse_full(args, &[]) {
            Err(CliError::Config(v)) => assert!(v.iter().any(|e| e.contains("--lobbyip")), "{v:?}"),
            _ => panic!("--public with the example's loopback advertise_ip must be refused"),
        }
    }
}
