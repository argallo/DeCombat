//! Account administration: `dc-lobby-server account ...` works on the DB file, also while the lobby runs (the DB is
//! in WAL mode and the lobby reads the account row at every login, so a change counts from the next login on).
//!
//! ```text
//! dc-lobby-server account [--db FILE] add NAME [--password PW | --invite] [--days D] [--email ADDR]
//! dc-lobby-server account [--db FILE] set-email NAME ADDR
//! dc-lobby-server account [--db FILE] disable NAME | enable NAME
//! dc-lobby-server account [--db FILE] reset-password NAME PW
//! dc-lobby-server account [--db FILE] list
//! dc-lobby-server account [--db FILE] gm NAME --on | --off
//! dc-lobby-server account [--db FILE] invite [--count N] [--days D]
//! dc-lobby-server account [--db FILE] loadout NAME SPEC [--days N] [--rom DIR]
//! ```
//!
//! Also the name / password rules and the invite codes the registration page uses.
// DC-DEFAULT: LG-141 login name and password rules; LG-142 disabled accounts; LG-143 invite codes; LG-144 the CLI
// DC-DEFAULT: LG-171 `gm NAME --on|--off` sets accounts.gm (1 / 0); it counts from the account's next login
// DC-DEFAULT: LG-240 the recovery address (`add --email`, `set-email`); `list` shows it masked (LG-244)

use crate::db;
use crate::state::KeySource;
use clap::{Arg, ArgAction, ArgMatches, Command};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

/// Login name length in characters: the login id is a wstr[17] field (PROTOCOL_2009.md §6.4 0x3d3 +0x12).
pub const NAME_MIN_LEN: usize = 3;
pub const NAME_MAX_LEN: usize = 16;
/// Password length in UTF-16 units. The upper bound is the 2009 login request's typed-text field, a wstr[17]
/// (PROTOCOL_2009.md §2.2 0x200 +0x2006): 16 units and a NUL, shorter than the 64 NIST SP 800-63B asks a verifier to
/// accept; a longer password could not be sent by the game, so the limit stays 16 (LG-140).
pub const PW_MIN_LEN: usize = 8;
pub const PW_MAX_LEN: usize = 16;
/// The passwords refused as too common (LG-140), one lower-case entry per line.
const COMMON_PASSWORDS: &str = include_str!("common_passwords.txt");
/// Invite codes: 12 characters of an alphabet without 0 / O / 1 / I (60 bits), shown as XXXX-XXXX-XXXX.
pub(crate) const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 12;
pub const DEFAULT_INVITE_DAYS: i64 = 7;

/// Longest recovery address (the SMTP path limit).
pub const EMAIL_MAX_LEN: usize = 254;

/// LG-240: a recovery address, trimmed: at most 254 characters, no spaces or control characters, exactly one `@`
/// with text before it and a dot inside the part after it. Returns the address to store.
pub fn valid_email(raw: &str) -> Result<String, String> {
    let e = raw.trim();
    if e.is_empty() {
        return Err("enter your e-mail address (it is how a forgotten password is reset)".into());
    }
    if e.chars().count() > EMAIL_MAX_LEN {
        return Err(format!(
            "the e-mail address is longer than {EMAIL_MAX_LEN} characters"
        ));
    }
    let bad = || Err("that is not a valid e-mail address".to_string());
    if e.chars().any(|c| c.is_whitespace() || c.is_control()) || e.matches('@').count() != 1 {
        return bad();
    }
    let (local, domain) = e.split_once('@').expect("one @");
    let dot_inside = domain
        .find('.')
        .is_some_and(|i| i > 0 && i + 1 < domain.len());
    if local.is_empty() || !dot_inside || domain.ends_with('.') {
        return bad();
    }
    Ok(e.to_string())
}

/// LG-244: an address as logs and lists show it: the first character, `***`, then `@` and the domain
/// (`r***@example.org`); `-` for none.
pub fn mask_email(e: &str) -> String {
    match e.split_once('@') {
        _ if e.is_empty() => "-".into(),
        Some((l, d)) => format!("{}***@{}", l.chars().next().unwrap_or('*'), d),
        None => "***".into(),
    }
}

/// LG-141: a login name for a new account is 3..16 ASCII letters, digits, `_`, `-` or `.`.
pub fn valid_login(name: &str) -> Result<(), String> {
    let n = name.chars().count();
    if !(NAME_MIN_LEN..=NAME_MAX_LEN).contains(&n) {
        return Err(format!(
            "the name must be {NAME_MIN_LEN} to {NAME_MAX_LEN} characters long"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err("the name may only use the letters A-Z, digits and _ - .".into());
    }
    Ok(())
}

/// LG-140: a new password is 8..16 UTF-16 units (a character outside the BMP counts two, as the game's field
/// does), any printable Unicode, no composition rules, never one of the common passwords. Stored exactly as typed:
/// no Unicode normalization is applied (neither at registration nor at login), so both sides hash the same units.
pub fn valid_password(pw: &str) -> Result<(), String> {
    let n = pw.encode_utf16().count();
    if !(PW_MIN_LEN..=PW_MAX_LEN).contains(&n) {
        return Err(format!(
            "the password must be {PW_MIN_LEN} to {PW_MAX_LEN} characters long (the game sends at most {PW_MAX_LEN})"
        ));
    }
    if pw.chars().any(|c| c.is_control()) {
        return Err("the password may not contain control characters".into());
    }
    let lower = pw.to_lowercase();
    if COMMON_PASSWORDS
        .lines()
        .any(|l| !l.starts_with('#') && l.trim() == lower)
    {
        return Err("that password is too common, pick another".into());
    }
    Ok(())
}

/// [`valid_password`] plus: not the login name (any case).
pub fn valid_password_for(login: &str, pw: &str) -> Result<(), String> {
    valid_password(pw)?;
    if pw.to_lowercase() == login.to_lowercase() {
        return Err("the password may not be your login name".into());
    }
    Ok(())
}

/// Canonical form of a typed invite code: upper case, without spaces and dashes.
pub fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// `ABCDEFGHJKLM` -> `ABCD-EFGH-JKLM`.
pub fn show_code(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// Creates a one-time invite code valid for `days` days (0 = no expiry), optionally bound to one login name.
pub fn new_invite(
    c: &Connection,
    keys: &mut KeySource,
    login: Option<&str>,
    now: i64,
    days: i64,
) -> db::DbResult<String> {
    loop {
        let bytes = keys.key16();
        let code: String = bytes
            .iter()
            .take(CODE_LEN)
            .map(|b| CODE_ALPHABET[(*b & 31) as usize] as char)
            .collect();
        let expires = if days > 0 { now + days * 86_400 } else { 0 };
        let n = c.execute(
            "INSERT OR IGNORE INTO invites (code, login, created, expires) VALUES (?1, ?2, ?3, ?4)",
            params![code, login, now, expires],
        )?;
        if n == 1 {
            return Ok(code);
        }
    }
}

/// What an invite code allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invite {
    /// Valid; `Some(name)` when the code is bound to that login name.
    Open(Option<String>),
    Unknown,
    Used,
    Expired,
}

pub fn invite(c: &Connection, code: &str, now: i64) -> db::DbResult<Invite> {
    let code = normalize_code(code);
    let row: Option<(Option<String>, i64, i64)> = c
        .query_row(
            "SELECT login, expires, used_by FROM invites WHERE code = ?1",
            params![code],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(match row {
        None => Invite::Unknown,
        Some((_, _, used)) if used != 0 => Invite::Used,
        Some((_, exp, _)) if exp != 0 && exp <= now => Invite::Expired,
        Some((login, _, _)) => Invite::Open(login),
    })
}

/// Marks an invite code used by `user_no`; false when it was already used.
pub fn consume_invite(c: &Connection, code: &str, user_no: u32, now: i64) -> db::DbResult<bool> {
    let n = c.execute(
        "UPDATE invites SET used_by = ?2, used_at = ?3 WHERE code = ?1 AND used_by = 0",
        params![normalize_code(code), user_no as i64, now],
    )?;
    if n == 1 {
        // LG-377: a code that came from a beta signup marks it registered (a failure here never stops the registration)
        let _ = crate::beta::code_consumed(c, &normalize_code(code), user_no);
    }
    Ok(n == 1)
}

/// LG-142: sets the disabled flag; false when there is no such account.
pub fn set_disabled(c: &Connection, login: &str, disabled: bool) -> db::DbResult<bool> {
    let n = c.execute(
        "UPDATE accounts SET disabled = ?2 WHERE login = ?1",
        params![login, disabled as i64],
    )?;
    Ok(n == 1)
}

/// LG-171: sets the GM flag (accounts.gm 1 / 0); false when there is no such account.
pub fn set_gm(c: &Connection, login: &str, gm: bool) -> db::DbResult<bool> {
    let n = c.execute(
        "UPDATE accounts SET gm = ?2 WHERE login = ?1",
        params![login, gm as i64],
    )?;
    Ok(n == 1)
}

/// LG-172: the registration page refuses a login name with the GM prefix: a server-made character takes the
/// login as its name, and the 2009 client draws any "GM..." name as a game master.
// DC-DEFAULT: LG-172 names beginning with "GM" (case as the 2009 client compares) are reserved for GM accounts
pub fn valid_new_login(name: &str) -> Result<(), String> {
    valid_login(name)?;
    if crate::gm::gm_named(name) {
        return Err("names beginning with GM are reserved for game masters".into());
    }
    Ok(())
}

/// [`create`] with the password already hashed (LG-328: the hash was made outside the lock).
pub fn create_with_phc(
    c: &Connection,
    login: &str,
    phc: &str,
    first_no: u32,
    now: i64,
) -> Result<u32, String> {
    valid_login(login)?;
    if db::account_by_login(c, login)
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err(format!("the name {login} is taken"));
    }
    let no = db::create_account_from(c, login, now, first_no).map_err(|e| e.to_string())?;
    db::set_password_phc(c, no, phc).map_err(|e| e.to_string())?;
    Ok(no)
}

/// Creates a new account `login` (LG-141 name rule) with an optional password; an existing name is refused.
pub fn create(
    c: &Connection,
    keys: &mut KeySource,
    login: &str,
    pw: Option<&str>,
    first_no: u32,
    now: i64,
) -> Result<u32, String> {
    valid_login(login)?;
    if let Some(pw) = pw {
        valid_password_for(login, pw)?;
    }
    if db::account_by_login(c, login)
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err(format!("the name {login} is taken"));
    }
    let no = db::create_account_from(c, login, now, first_no).map_err(|e| e.to_string())?;
    if let Some(pw) = pw {
        db::set_password_phc(c, no, &crate::pwhash::encode(&keys.key16(), pw))
            .map_err(|e| e.to_string())?;
    }
    Ok(no)
}

fn command() -> Command {
    let name = || Arg::new("name").required(true).help("login name");
    let days = || {
        Arg::new("days")
            .long("days")
            .value_parser(clap::value_parser!(i64))
            .help("invite validity in days (default 7, 0 = no expiry)")
    };
    Command::new("dc-lobby-server account")
        .bin_name("dc-lobby-server account")
        .about("create, disable and list the accounts of the lobby DB (works while the lobby runs)")
        .arg(
            Arg::new("db")
                .long("db")
                .global(true)
                .help("the lobby's SQLite file (default <exe dir>/data/ca.db)"),
        )
        .subcommand_required(true)
        .subcommand(
            Command::new("add")
                .about("create an account")
                .arg(name())
                .arg(
                    Arg::new("password")
                        .long("password")
                        .help("its password (6 to 16 characters)"),
                )
                .arg(
                    Arg::new("invite")
                        .long("invite")
                        .action(ArgAction::SetTrue)
                        .conflicts_with("password")
                        .help("no password: print an invite code for this name; the player sets the password on the registration page"),
                )
                .arg(days())
                .arg(
                    Arg::new("email")
                        .long("email")
                        .help("the player's e-mail address, for password recovery"),
                ),
        )
        .subcommand(
            Command::new("set-email")
                .about("set the e-mail address used for password recovery")
                .arg(name())
                .arg(Arg::new("address").required(true)),
        )
        .subcommand(
            Command::new("disable")
                .about("refuse the account's logins")
                .arg(name()),
        )
        .subcommand(
            Command::new("enable")
                .about("allow the account's logins again")
                .arg(name()),
        )
        .subcommand(
            Command::new("reset-password")
                .about("set a new password")
                .arg(name())
                .arg(Arg::new("password").required(true)),
        )
        .subcommand(
            Command::new("event")
                .about("special event: give NX once per event id to active accounts or a named list, with a box at their next login")
                .arg(Arg::new("id").required(true).help("event id; running it again pays nobody twice"))
                .arg(Arg::new("nx").long("nx").required(true).value_parser(clap::value_parser!(i64)).help("NX (prepaid) per account, 1..=1000000"))
                .arg(Arg::new("label").long("label").required(true).help("shown as '<label>: +<nx> NX', at most 200 characters"))
                .arg(Arg::new("active-days").long("active-days").value_parser(clap::value_parser!(i64)).conflicts_with("names").help("every non-bot account seen in the last D days"))
                .arg(Arg::new("names").long("names").value_delimiter(',').help("these account names (comma separated)")),
        )
        .subcommand(
            Command::new("grant")
                .about("give (or take, with a negative amount) NX, NX credit, GP or EXP; counts from the account's next login")
                .arg(name())
                .arg(Arg::new("nx").long("nx").allow_hyphen_values(true).value_parser(clap::value_parser!(i64)).help("NX (prepaid) to add"))
                .arg(Arg::new("nx-credit").long("nx-credit").allow_hyphen_values(true).value_parser(clap::value_parser!(i64)).help("NX credit to add"))
                .arg(Arg::new("gp").long("gp").allow_hyphen_values(true).value_parser(clap::value_parser!(i64)).help("GP to add"))
                .arg(Arg::new("exp").long("exp").allow_hyphen_values(true).value_parser(clap::value_parser!(i64)).help("EXP to add")),
        )
        .subcommand(
            Command::new("loadout")
                .about("give the account items and equip them (replacing what its slots hold); counts from the next login; needs the ROM folder to resolve slots")
                .arg(name())
                .arg(Arg::new("spec").required(true).help("<slot>:<item>[+<option>],... or bare item numbers, as --start-loadout; `random` = a fresh random GP primary and secondary of the ROM shop (LG-260, new draw every run)"))
                .arg(Arg::new("days").long("days").value_parser(clap::value_parser!(i64)).help("items expire after N days (default: permanent)"))
                .arg(Arg::new("rom").long("rom").help("the ROM folder (default: CA_ROM, else DC_PATHS__ROM)")),
        )
        .subcommand(
            Command::new("rename")
                .about("give the account's character another name (LG-273: e.g. an identity login that got `Name2`); counts from the next login")
                .arg(name())
                .arg(Arg::new("nick").required(true).help("the new character name (1..16 characters, unique on this lobby)")),
        )
        .subcommand(Command::new("list").about("list the accounts and the open invite codes"))
        .subcommand(
            Command::new("gm")
                .about("make the account a game master (--on) or a player again (--off); counts from its next login")
                .arg(name())
                .arg(
                    Arg::new("on")
                        .long("on")
                        .action(ArgAction::SetTrue)
                        .help("GM: the login reply carries the operator privilege 2"),
                )
                .arg(
                    Arg::new("off")
                        .long("off")
                        .action(ArgAction::SetTrue)
                        .help("player"),
                )
                .group(
                    clap::ArgGroup::new("flag")
                        .args(["on", "off"])
                        .required(true),
                ),
        )
        .subcommand(
            Command::new("invite")
                .about("print one-time invite codes for the registration page")
                .arg(
                    Arg::new("count")
                        .long("count")
                        .value_parser(clap::value_parser!(u32).range(1..=1000))
                        .default_value("1"),
                )
                .arg(days()),
        )
}

fn default_db() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("data")
        .join("ca.db")
}

/// `YYYY-MM-DD` (UTC) of a unix time, `-` for 0.
fn when(t: i64) -> String {
    if t <= 0 {
        return "-".into();
    }
    // civil date from the day number (days-from-civil inverse, proleptic Gregorian)
    let z = t.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn list(c: &Connection, now: i64) -> rusqlite::Result<String> {
    let mut out = format!(
        "{:<8} {:<16} {:<9} {:<8} {:<3} {:<10} {:<10} {:<24} character\n",
        "account", "name", "password", "state", "gm", "created", "last login", "e-mail"
    );
    let mut q = c.prepare(
        "SELECT a.user_no, a.login, a.pw_hash, a.pw_salt, a.disabled, a.gm, a.created, a.last_login, \
         COALESCE(c.nick, ''), a.email FROM accounts a LEFT JOIN characters c ON c.user_no = a.user_no \
         ORDER BY a.user_no",
    )?;
    let rows = q.query_map([], |r| {
        let h: Option<String> = r.get(2)?;
        let salt: Option<String> = r.get(3)?;
        let pw = match (h.filter(|h| !h.is_empty()), salt.filter(|s| !s.is_empty())) {
            (None, _) => "none",
            (Some(_), Some(_)) => "sha256",
            (Some(_), None) => "argon2id",
        };
        Ok(format!(
            "{:<8} {:<16} {:<9} {:<8} {:<3} {:<10} {:<10} {:<24} {}\n",
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            pw,
            if r.get::<_, i64>(4)? != 0 {
                "disabled"
            } else {
                "ok"
            },
            r.get::<_, i64>(5)?,
            when(r.get(6)?),
            when(r.get(7)?),
            mask_email(&r.get::<_, String>(9)?),
            r.get::<_, String>(8)?
        ))
    })?;
    for r in rows {
        out.push_str(&r?);
    }
    let mut q = c.prepare(
        "SELECT code, login, expires FROM invites WHERE used_by = 0 AND (expires = 0 OR expires > ?1) \
         ORDER BY created, code",
    )?;
    let codes = q.query_map(params![now], |r| {
        Ok(format!(
            "invite {} {} expires {}\n",
            show_code(&r.get::<_, String>(0)?),
            r.get::<_, Option<String>>(1)?
                .map(|l| format!("for {l}"))
                .unwrap_or_else(|| "for any name".into()),
            when(r.get(2)?)
        ))
    })?;
    for r in codes {
        out.push_str(&r?);
    }
    Ok(out.trim_end().to_string())
}

/// `loadout`: gives the existing account the items of `spec` (permanent, or `days` long) and equips them; what the
/// slots held before stays in the inventory, unequipped. Returns the output line.
pub(crate) fn equip_loadout(
    c: &Connection,
    rom: &crate::rom::RomData,
    login: &str,
    spec: &str,
    days: Option<i64>,
) -> Result<String, String> {
    let e = |e: rusqlite::Error| e.to_string();
    if days.is_some_and(|d| d <= 0) {
        return Err("--days must be at least 1".into());
    }
    if spec.trim().is_empty() {
        return Err("loadout: give at least one item".into());
    }
    // LG-251 amendment: `random` re-rolls the LG-260 kit with a fresh seed (the clock), unlike the first login's
    // draw by the name, so every run gives another kit
    let spec = if spec.trim().eq_ignore_ascii_case("random") {
        rom.random_kit(crate::grant::bot_seed(login) ^ crate::clock::now_ticks() as u64)
            .ok_or("loadout random: the ROM folder has no shop catalog to draw from")?
    } else {
        spec.to_string()
    };
    let spec = spec.as_str();
    let acc = db::account_by_login(c, login)
        .map_err(e)?
        .ok_or_else(|| format!("no account {login}"))?;
    let ch = db::character(c, acc.user_no).map_err(e)?.ok_or_else(|| {
        format!(
            "account {} has no character yet (it is made at the first login)",
            acc.login
        )
    })?;
    let mut o = crate::cli::parse_full(["dc-lobby-server"], &[])
        .map_err(|e| match e {
            crate::cli::CliError::Usage(u) => u.to_string(),
            crate::cli::CliError::Config(lines) => lines.join("; "),
        })?
        .opts;
    o.start_loadout = spec.to_string();
    let now = crate::clock::now_ticks();
    let end = days.map_or(0, |d| now + d * crate::clock::TICKS_PER_DAY);
    let seed = crate::grant::apply_start_loadout_until(c, &o, rom, acc.user_no, now, end)
        .map_err(|m| m.replace("--start-loadout", "loadout"))?;
    let Some(&(first_slot, first_item)) = seed.first() else {
        return Err("loadout: give at least one item".into());
    };
    let more = match seed.len() {
        1 => String::new(),
        2 => " (+1 item)".to_string(),
        n => format!(" (+{} items)", n - 1),
    };
    Ok(format!(
        "{} ({}): equipped {first_slot}:{first_item}{more}, counts from the next login",
        acc.login, ch.nick
    ))
}

/// Runs one parsed command against `c`; returns the text to print or the error.
fn exec(
    c: &Connection,
    keys: &mut KeySource,
    first_no: u32,
    m: &ArgMatches,
    now: i64,
) -> Result<String, String> {
    let e = |e: rusqlite::Error| e.to_string();
    let name = |sm: &ArgMatches| sm.get_one::<String>("name").cloned().unwrap_or_default();
    let days = |sm: &ArgMatches| {
        sm.get_one::<i64>("days")
            .copied()
            .unwrap_or(DEFAULT_INVITE_DAYS)
    };
    match m.subcommand() {
        Some(("add", sm)) => {
            let login = name(sm);
            let pw = sm.get_one::<String>("password").map(String::as_str);
            let email = sm
                .get_one::<String>("email")
                .map(|a| valid_email(a))
                .transpose()?;
            let no = create(c, keys, &login, pw, first_no, now)?;
            let mut out = format!("account {login} created (account {no})");
            if let Some(a) = &email {
                db::set_email(c, no, a).map_err(e)?;
                out.push_str(&format!(", e-mail {}", mask_email(a)));
            }
            if pw.is_some() {
                out.push_str(", password set");
            } else if sm.get_flag("invite") {
                let code = new_invite(c, keys, Some(&login), now, days(sm)).map_err(e)?;
                out.push_str(&format!(
                    "\ninvite code for {login}: {} (enter it with the name {login} on the registration page)",
                    show_code(&code)
                ));
            } else {
                out.push_str(
                    ", no password yet (the first login sets it under --check-password, except on a --public server)",
                );
            }
            Ok(out)
        }
        Some((cmd @ ("disable" | "enable"), sm)) => {
            let login = name(sm);
            if !set_disabled(c, &login, cmd == "disable").map_err(e)? {
                return Err(format!("no account {login}"));
            }
            Ok(format!("account {login} {cmd}d (from its next login on)"))
        }
        Some(("reset-password", sm)) => {
            let login = name(sm);
            let pw = sm
                .get_one::<String>("password")
                .cloned()
                .unwrap_or_default();
            valid_password_for(&login, &pw)?;
            let acc = db::account_by_login(c, &login)
                .map_err(e)?
                .ok_or_else(|| format!("no account {login}"))?;
            db::set_password_phc(c, acc.user_no, &crate::pwhash::encode(&keys.key16(), &pw))
                .map_err(e)?;
            Ok(format!("password of {} set", acc.login))
        }
        Some(("set-email", sm)) => {
            let login = name(sm);
            let addr = valid_email(
                sm.get_one::<String>("address")
                    .map(String::as_str)
                    .unwrap_or(""),
            )?;
            let acc = db::account_by_login(c, &login)
                .map_err(e)?
                .ok_or_else(|| format!("no account {login}"))?;
            db::set_email(c, acc.user_no, &addr).map_err(e)?;
            Ok(format!(
                "e-mail of {} set to {}",
                acc.login,
                mask_email(&addr)
            ))
        }
        // DC-DEFAULT: LG-250 (proposed) the host gives NX / GP / EXP from the account CLI
        // DC-DEFAULT: LG-312 (proposed) special events from the host's account CLI
        Some(("event", sm)) => {
            let id = sm.get_one::<String>("id").cloned().unwrap_or_default();
            let label = sm.get_one::<String>("label").cloned().unwrap_or_default();
            let nx = sm.get_one::<i64>("nx").copied().unwrap_or(0);
            if id.trim().is_empty() || id.chars().count() > 64 {
                return Err("event: the id needs 1..=64 characters".into());
            }
            if label.trim().is_empty() || label.chars().count() > 200 {
                return Err("event: --label needs 1..=200 characters".into());
            }
            if !(1..=1_000_000).contains(&nx) {
                return Err("event: --nx must be 1..=1000000".into());
            }
            let names: Vec<String> = sm
                .get_many::<String>("names")
                .map(|v| v.cloned().collect())
                .unwrap_or_default();
            let targets = match sm.get_one::<i64>("active-days").copied() {
                Some(d) if (1..=36500).contains(&d) => {
                    crate::nxrewards::EventTargets::ActiveSince(now - d * 86_400)
                }
                Some(_) => return Err("event: --active-days must be 1..=36500".into()),
                None if !names.is_empty() => crate::nxrewards::EventTargets::Names(&names),
                None => return Err("event: give --active-days D or --names a,b".into()),
            };
            let o = crate::nxrewards::run_event(c, &id, &label, nx, &targets, now).map_err(e)?;
            Ok(format!(
                "event {id}: {} account(s) paid +{nx} NX, {} already paid, {} skipped (no such account or no character); shown at the next login",
                o.paid, o.already, o.skipped
            ))
        }
        Some(("grant", sm)) => {
            let login = name(sm);
            let amount = |k: &str| sm.get_one::<i64>(k).copied().unwrap_or(0);
            let (nx, nx_cr, gp, exp) = (
                amount("nx"),
                amount("nx-credit"),
                amount("gp"),
                amount("exp"),
            );
            if nx == 0 && nx_cr == 0 && gp == 0 && exp == 0 {
                return Err("nothing to grant: give --nx, --nx-credit, --gp or --exp".into());
            }
            let acc = db::account_by_login(c, &login)
                .map_err(e)?
                .ok_or_else(|| format!("no account {login}"))?;
            if db::grant(c, acc.user_no, nx, nx_cr, gp, exp).map_err(e)? == 0 {
                return Err(format!(
                    "account {} has no character yet (it is made at the first login)",
                    acc.login
                ));
            }
            let ch = db::character(c, acc.user_no)
                .map_err(e)?
                .ok_or_else(|| format!("no character for {}", acc.login))?;
            Ok(format!(
                "{} ({}): NX {} (+{nx}), NX credit {} (+{nx_cr}), GP {} (+{gp}), EXP {} (+{exp}); counts from the next login",
                acc.login, ch.nick, ch.nx, ch.nx_cr, ch.gp, ch.exp
            ))
        }
        // DC-DEFAULT: LG-251 (proposed) the host equips items on an existing account from the account CLI
        Some(("loadout", sm)) => {
            let login = name(sm);
            let dir = sm
                .get_one::<String>("rom")
                .map(PathBuf::from)
                .or_else(|| dc_rom::resolve_rom().ok())
                .or_else(|| {
                    std::env::var("DC_PATHS__ROM")
                        .ok()
                        .map(PathBuf::from)
                        .filter(|p| p.is_dir())
                })
                .ok_or("the ROM folder is needed to resolve slots: give --rom DIR or set CA_ROM")?;
            let rom = crate::rom::RomData::load(&dir);
            let spec = sm.get_one::<String>("spec").cloned().unwrap_or_default();
            let days = sm.get_one::<i64>("days").copied();
            equip_loadout(c, &rom, &login, &spec, days)
        }
        // DC-DEFAULT: LG-273 (proposed) the host renames a character from the account CLI
        Some(("rename", sm)) => {
            let login = name(sm);
            let nick = sm.get_one::<String>("nick").cloned().unwrap_or_default();
            if !crate::auth::valid_nick(&nick) {
                return Err(format!(
                    "{nick:?} is not a valid name (1..16 characters, no leading or trailing space)"
                ));
            }
            let acc = db::account_by_login(c, &login)
                .map_err(e)?
                .ok_or_else(|| format!("no account {login}"))?;
            let old = db::character(c, acc.user_no)
                .map_err(e)?
                .ok_or_else(|| format!("account {} has no character yet", acc.login))?;
            // the row's key says how this lobby compares names (CD-16): a key equal to a mixed-case nick is the
            // case-sensitive mode, anything else the case-insensitive default
            let key: String = c
                .query_row(
                    "SELECT nick_key FROM characters WHERE user_no = ?1",
                    [acc.user_no as i64],
                    |r| r.get(0),
                )
                .map_err(e)?;
            let ci = !(key == old.nick && old.nick != old.nick.to_lowercase());
            for case in [ci, true] {
                if let Some(h) = db::character_by_nick(c, &nick, case).map_err(e)? {
                    if h.user_no != acc.user_no {
                        return Err(format!("the name {nick} is held by account {}", h.user_no));
                    }
                }
            }
            c.execute(
                "UPDATE characters SET nick = ?2, nick_key = ?3 WHERE user_no = ?1",
                params![acc.user_no as i64, nick, db::nick_key(&nick, ci)],
            )
            .map_err(e)?;
            Ok(format!(
                "{}: character {} renamed to {nick} (from its next login on)",
                acc.login, old.nick
            ))
        }
        Some(("list", _)) => list(c, now).map_err(e),
        Some(("gm", sm)) => {
            let login = name(sm);
            let on = sm.get_flag("on");
            if !set_gm(c, &login, on).map_err(e)? {
                return Err(format!("no account {login}"));
            }
            Ok(format!(
                "account {login} is {} (from its next login on; a running session keeps its old privilege)",
                if on { "a GM" } else { "a player" }
            ))
        }
        Some(("invite", sm)) => {
            let n = sm.get_one::<u32>("count").copied().unwrap_or(1);
            let d = days(sm);
            let mut out = Vec::new();
            for _ in 0..n {
                out.push(show_code(&new_invite(c, keys, None, now, d).map_err(e)?));
            }
            Ok(out.join("\n"))
        }
        _ => Err("unknown command".into()),
    }
}

/// `dc-lobby-server account ...`; `args` includes argv[0] and excludes the word `account`. Returns the exit code.
pub fn run<I>(args: I) -> i32
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let m = match command().try_get_matches_from(args) {
        Ok(m) => m,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };
    let path = m
        .get_one::<String>("db")
        .map(PathBuf::from)
        .unwrap_or_else(default_db);
    match run_on(&path, &m) {
        Ok(out) => {
            println!("{out}");
            0
        }
        Err(e) => {
            eprintln!("dc-lobby-server account: {e}");
            1
        }
    }
}

/// Opens the DB file (it must exist: a typo must not create a second, empty DB) and runs `m` in one transaction.
fn run_on(path: &Path, m: &ArgMatches) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!(
            "no DB file {} (start the lobby once, or give --db)",
            path.display()
        ));
    }
    let c = db::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let (rules, _) = crate::config::Rules::load(&dir);
    let mut keys = KeySource::new();
    c.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let r = exec(
        &c,
        &mut keys,
        rules.first_account_no,
        m,
        crate::clock::unix_now(),
    );
    let end = if r.is_ok() { "COMMIT" } else { "ROLLBACK" };
    c.execute_batch(end).map_err(|e| e.to_string())?;
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("dc_accounts_{}_{}.db", tag, std::process::id()));
        remove(&p);
        drop(db::open(&p).unwrap());
        p
    }

    fn remove(p: &Path) {
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", p.display(), ext));
        }
    }

    fn cli(p: &Path, args: &[&str]) -> Result<String, String> {
        let head = ["account", "--db", p.to_str().unwrap()];
        let m = command()
            .try_get_matches_from(head.iter().chain(args.iter()))
            .map_err(|e| e.to_string())?;
        run_on(p, &m)
    }

    #[test]
    fn rename_gives_the_character_a_free_name() {
        let p = temp_db("rename");
        {
            let c = db::open(&p).unwrap();
            for (no, login, nick) in [(1u32, "kim", "Quinn2"), (2, "lee", "Quinn")] {
                c.execute(
                    "INSERT INTO accounts (user_no, login, created) VALUES (?1, ?2, 0)",
                    params![no as i64, login],
                )
                .unwrap();
                db::create_character(
                    &c,
                    &db::NewCharacter {
                        user_no: no,
                        nick,
                        face: 0,
                        exp: 0,
                        gp: 0,
                        nx: 0,
                        now: 0,
                    },
                    true,
                )
                .unwrap();
            }
        }
        assert!(cli(&p, &["rename", "kim", "quinn"])
            .unwrap_err()
            .contains("held by account 2"));
        assert!(cli(&p, &["rename", "kim", " bad"]).is_err());
        assert!(cli(&p, &["rename", "nobody", "Fine"]).is_err());
        assert!(cli(&p, &["rename", "kim", "Quinnway"])
            .unwrap()
            .contains("renamed to Quinnway"));
        let c = db::open(&p).unwrap();
        assert_eq!(db::character(&c, 1).unwrap().unwrap().nick, "Quinnway");
        assert!(db::character_by_nick(&c, "quinnway", true)
            .unwrap()
            .is_some());
        drop(c);
        remove(&p);
    }

    #[test]
    fn name_and_password_rules() {
        assert!(valid_login("Erika").is_ok());
        assert!(valid_login("a.b-c_9").is_ok());
        for bad in [
            "ab",
            "seventeen_chars_x",
            "has space",
            "LOCAL:x",
            "Ünïcode",
            "",
        ] {
            assert!(valid_login(bad).is_err(), "{bad}");
        }
        assert!(valid_password("hunter22").is_ok());
        assert!(valid_password("1234567").is_err(), "7 units");
        assert!(valid_password("12345678901234567").is_err(), "17 units");
        assert!(valid_password("tab\there22").is_err());
        // no composition rules: all lower case, all digits, Unicode and spaces are fine
        for ok in [
            "correcthorse",
            "73914826",
            "p\u{e4}ssw\u{f6}rd!",
            "four words long",
            "\u{5bc6}\u{7801}\u{5bc6}\u{7801}\u{5bc6}\u{7801}\u{5bc6}\u{7801}",
        ] {
            assert!(valid_password(ok).is_ok(), "{ok}");
        }
        // eight astral characters are 16 units, nine are 18
        assert!(valid_password(&"\u{1f600}".repeat(8)).is_ok());
        assert!(valid_password(&"\u{1f600}".repeat(9)).is_err());
        // common passwords, in any case
        for bad in [
            "password",
            "PassWord1",
            "12345678",
            "QWERTYUI",
            "combatarms",
        ] {
            assert!(valid_password(bad).is_err(), "{bad}");
        }
        for l in COMMON_PASSWORDS.lines().filter(|l| !l.starts_with('#')) {
            assert!(
                l == l.trim().to_lowercase() && l.encode_utf16().count() >= PW_MIN_LEN,
                "{l}"
            );
        }
        assert!(valid_password_for("Erikaie99", "ERIKAIE99").is_err());
        assert!(valid_password_for("Erika", "erikaie99").is_ok());
        assert_eq!(normalize_code(" abcd-efgh jklm "), "ABCDEFGHJKLM");
        assert_eq!(show_code("ABCDEFGHJKLM"), "ABCD-EFGH-JKLM");
        assert_eq!(when(0), "-");
        assert_eq!(when(1_761_696_000), "2025-10-29");
    }

    #[test]
    fn gm_flag_on_and_off_and_reserved_names() {
        let p = temp_db("gm");
        let lobby = db::open(&p).unwrap();
        cli(&p, &["add", "Quinn", "--password", "hunter22"]).unwrap();
        let gm = |c: &Connection| db::account_by_login(c, "quinn").unwrap().unwrap().gm;
        assert_eq!(gm(&lobby), 0);
        let out = cli(&p, &["gm", "quinn", "--on"]).unwrap();
        assert!(
            out.contains("is a GM") && out.contains("next login"),
            "{out}"
        );
        assert_eq!(
            gm(&lobby),
            1,
            "the running lobby's connection sees the flag"
        );
        assert_eq!(crate::gm::privilege_of(gm(&lobby)), 2);
        let list = cli(&p, &["list"]).unwrap();
        let row = list.lines().find(|l| l.contains("Quinn")).unwrap();
        assert!(row.split_whitespace().nth(4) == Some("1"), "{row}");
        cli(&p, &["gm", "Quinn", "--off"]).unwrap();
        assert_eq!(gm(&lobby), 0);
        assert_eq!(crate::gm::privilege_of(0), 0);
        assert!(cli(&p, &["gm", "nobody", "--on"])
            .unwrap_err()
            .contains("no account"));
        assert!(
            cli(&p, &["gm", "Quinn"]).is_err(),
            "--on or --off is required"
        );
        assert!(cli(&p, &["gm", "Quinn", "--on", "--off"]).is_err());
        // LG-172: the registration name rule; the operator CLI may still make such an account
        assert!(valid_new_login("GMerika").is_err());
        assert!(
            valid_new_login("gmerika").is_ok(),
            "the 2009 prefix test is case sensitive"
        );
        assert!(valid_new_login("ErikaGM").is_ok());
        drop(lobby);
        remove(&p);
    }

    /// LG-240 / LG-244: `add --email`, `set-email`, and `list` showing the address masked.
    // LG-250: grant adds to the character's balances, floors them at 0 and refuses an account without a character
    #[test]
    fn cli_event_pays_once_and_checks_its_arguments() {
        let p = temp_db("event");
        cli(&p, &["add", "Kim", "--password", "hunter22"]).unwrap();
        let c = db::open(&p).unwrap();
        let acc = db::account_by_login(&c, "Kim").unwrap().unwrap();
        db::create_character(
            &c,
            &db::NewCharacter {
                user_no: acc.user_no,
                nick: "Kim",
                face: 0,
                exp: 0,
                gp: 0,
                nx: 0,
                now: 0,
            },
            true,
        )
        .unwrap();
        drop(c);
        let run = [
            "event",
            "e1",
            "--nx",
            "500",
            "--label",
            "Launch",
            "--names",
            "Kim,Nobody",
        ];
        let out = cli(&p, &run).unwrap();
        assert!(
            out.contains("1 account(s) paid") && out.contains("1 skipped"),
            "{out}"
        );
        let out = cli(&p, &run).unwrap();
        assert!(
            out.contains("0 account(s) paid") && out.contains("1 already"),
            "{out}"
        );
        let c = db::open(&p).unwrap();
        assert_eq!(db::character(&c, acc.user_no).unwrap().unwrap().nx, 500);
        drop(c);
        assert!(cli(
            &p,
            &["event", "e2", "--nx", "0", "--label", "x", "--names", "Kim"]
        )
        .is_err());
        assert!(cli(&p, &["event", "e2", "--nx", "5", "--label", "x"]).is_err());
        assert!(cli(
            &p,
            &[
                "event",
                "e2",
                "--nx",
                "5",
                "--label",
                "x",
                "--active-days",
                "0"
            ]
        )
        .is_err());
        assert!(cli(
            &p,
            &["event", "e2", "--nx", "5", "--label", " ", "--names", "Kim"]
        )
        .is_err());
        remove(&p);
    }

    #[test]
    fn cli_grant_adds_nx_gp_and_exp_from_the_next_login() {
        let p = temp_db("grant");
        cli(&p, &["add", "Kim", "--password", "hunter22"]).unwrap();
        let c = db::open(&p).unwrap();
        let acc = db::account_by_login(&c, "Kim").unwrap().unwrap();
        assert!(cli(&p, &["grant", "Kim", "--nx", "100"])
            .unwrap_err()
            .contains("no character"));
        db::create_character(
            &c,
            &db::NewCharacter {
                user_no: acc.user_no,
                nick: "Kimmy",
                face: 0,
                exp: 10,
                gp: 20,
                nx: 30,
                now: 0,
            },
            true,
        )
        .unwrap();
        assert!(cli(&p, &["grant", "Kim"])
            .unwrap_err()
            .contains("nothing to grant"));
        let out = cli(
            &p,
            &["grant", "Kim", "--nx", "1000", "--gp", "-5", "--exp", "7"],
        )
        .unwrap();
        assert!(
            out.contains("NX 1030") && out.contains("GP 15") && out.contains("EXP 17"),
            "{out}"
        );
        let out = cli(&p, &["grant", "Kim", "--nx", "-5000", "--nx-credit", "40"]).unwrap();
        assert!(
            out.contains("NX 0 ") && out.contains("NX credit 40"),
            "{out}"
        );
        assert!(cli(&p, &["grant", "Nobody", "--nx", "1"])
            .unwrap_err()
            .contains("no account"));
        drop(c);
        remove(&p);
    }

    // LG-251 amendment: `random` needs a ROM shop catalog to draw from
    #[test]
    fn loadout_random_needs_a_shop_catalog() {
        let p = temp_db("loadout_random");
        cli(&p, &["add", "Kim", "--password", "hunter22"]).unwrap();
        let c = db::open(&p).unwrap();
        let rom = crate::rom::RomData::empty();
        let err = equip_loadout(&c, &rom, "Kim", "random", None).unwrap_err();
        assert!(err.contains("no shop catalog"), "{err}");
    }

    // LG-251: loadout gives an existing character the listed items and equips them over what the slots held
    #[test]
    fn loadout_equips_an_existing_character_with_a_weapon_by_number() {
        let p = temp_db("loadout");
        cli(&p, &["add", "Kim", "--password", "hunter22"]).unwrap();
        let c = db::open(&p).unwrap();
        let acc = db::account_by_login(&c, "Kim").unwrap().unwrap();
        let mut rom = crate::rom::RomData::empty();
        assert!(equip_loadout(&c, &rom, "Kim", "0:27", None)
            .unwrap_err()
            .contains("no character"));
        db::create_character(
            &c,
            &db::NewCharacter {
                user_no: acc.user_no,
                nick: "Kimmy",
                face: 0,
                exp: 0,
                gp: 0,
                nx: 0,
                now: 0,
            },
            true,
        )
        .unwrap();
        let old = db::add_item(
            &c,
            &db::NewItem {
                user_no: acc.user_no,
                item_no: 5,
                start_time: 0,
                end_time: 0,
                slot: 0,
                default_type: 0,
            },
        )
        .unwrap();
        // no ROM entry: a bare number has no known slot
        assert!(equip_loadout(&c, &rom, "Kim", "27", None)
            .unwrap_err()
            .contains("no slot known"));
        assert!(equip_loadout(&c, &rom, "Kim", "x:y", None).is_err());
        assert!(equip_loadout(&c, &rom, "Nobody", "0:27", None)
            .unwrap_err()
            .contains("no account"));
        let class = (0..200)
            .find(|k| dc_bute::equip_slot_by_class_code(*k) == Some(0))
            .expect("a weapon class code for slot 0");
        rom.weapon_kinds.insert(27, class);
        let out = equip_loadout(&c, &rom, "Kim", "27,3:900", None).unwrap();
        assert!(
            out.contains("Kim (Kimmy): equipped 0:27 (+1 item), counts from the next login"),
            "{out}"
        );
        let rows: Vec<(i64, i64, i64)> = db::items(&c, acc.user_no)
            .unwrap()
            .iter()
            .map(|i| (i.item_no, i.slot, i.end_time))
            .collect();
        assert!(rows.contains(&(27, 0, 0)), "{rows:?}");
        assert!(rows.contains(&(900, 3, 0)), "{rows:?}");
        let was = db::item(&c, old).unwrap().unwrap().slot;
        assert_eq!(was, -1, "the old item stays, unequipped");
        equip_loadout(&c, &rom, "Kim", "27", Some(2)).unwrap();
        let end: i64 = c
            .query_row(
                "SELECT MAX(end_time) FROM items WHERE item_no = 27",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(end > 0, "--days sets an end time");
        drop(c);
        remove(&p);
    }

    #[test]
    fn cli_email_is_checked_stored_and_listed_masked() {
        let p = temp_db("email");
        let lobby = db::open(&p).unwrap();
        let out = cli(
            &p,
            &[
                "add",
                "Erika",
                "--password",
                "hunter22",
                "--email",
                " erika.doe@example.org ",
            ],
        )
        .unwrap();
        assert!(
            out.contains("e-mail e***@example.org") && !out.contains("erika.doe"),
            "{out}"
        );
        let a = db::account_by_login(&lobby, "erika").unwrap().unwrap();
        assert_eq!(
            db::account_email(&lobby, a.user_no).unwrap(),
            "erika.doe@example.org"
        );
        assert!(cli(&p, &["add", "Kim", "--email", "kim-at-example.org"])
            .unwrap_err()
            .contains("not a valid e-mail address"));
        assert!(
            db::account_by_login(&lobby, "Kim").unwrap().is_none(),
            "nothing made"
        );
        cli(&p, &["add", "Kim"]).unwrap();
        let list = cli(&p, &["list"]).unwrap();
        assert!(list.lines().next().unwrap().contains("e-mail"), "{list}");
        let erika = list.lines().find(|l| l.contains("Erika")).unwrap();
        assert!(
            erika.contains("e***@example.org") && !list.contains("erika.doe"),
            "{list}"
        );
        let kim = list.lines().find(|l| l.contains("Kim")).unwrap();
        assert_eq!(kim.split_whitespace().nth(7), Some("-"), "{kim}");
        let out = cli(&p, &["set-email", "kim", "Kim.Lee@Example.org"]).unwrap();
        assert_eq!(out, "e-mail of Kim set to K***@Example.org");
        let k = db::account_by_login(&lobby, "kim").unwrap().unwrap();
        assert_eq!(
            db::account_email(&lobby, k.user_no).unwrap(),
            "Kim.Lee@Example.org"
        );
        assert!(cli(&p, &["set-email", "nobody", "a@b.cd"])
            .unwrap_err()
            .contains("no account"));
        assert!(cli(&p, &["set-email", "kim", "a@b"]).is_err());
        assert!(!cli(&p, &["list"]).unwrap().contains("Kim.Lee"));
        drop(lobby);
        remove(&p);
    }

    #[test]
    fn missing_db_file_is_not_created() {
        let p = std::env::temp_dir().join(format!("dc_accounts_none_{}.db", std::process::id()));
        remove(&p);
        let m = command().try_get_matches_from(["account", "list"]).unwrap();
        assert!(run_on(&p, &m).unwrap_err().contains("no DB file"));
        assert!(!p.exists());
    }

    #[test]
    fn cli_on_a_db_file_seen_by_a_second_connection() {
        let p = temp_db("cli");
        // the lobby's own connection stays open the whole time (WAL)
        let lobby = db::open(&p).unwrap();
        let out = cli(&p, &["add", "Erika", "--password", "hunter22"]).unwrap();
        assert!(
            out.contains("account Erika created (account 1001)"),
            "{out}"
        );
        let a = db::account_by_login(&lobby, "erika").unwrap().unwrap();
        assert!(a.pw_hash.as_deref().unwrap().starts_with("$argon2id$"));
        assert!(!a.disabled);
        assert!(cli(&p, &["add", "ERIKA", "--password", "hunter22"])
            .unwrap_err()
            .contains("taken"));
        assert!(
            cli(&p, &["add", "x", "--password", "hunter22"]).is_err(),
            "name rule"
        );
        assert!(
            cli(&p, &["add", "Kim", "--password", "short"]).is_err(),
            "password rule"
        );
        assert!(cli(&p, &["add", "Kim", "--password", "longenough", "--invite"]).is_err());
        // disable / enable: the lobby's connection sees the flag at its next read
        cli(&p, &["disable", "Erika"]).unwrap();
        assert!(
            db::account_by_login(&lobby, "Erika")
                .unwrap()
                .unwrap()
                .disabled
        );
        cli(&p, &["enable", "erika"]).unwrap();
        assert!(
            !db::account_by_login(&lobby, "Erika")
                .unwrap()
                .unwrap()
                .disabled
        );
        assert!(cli(&p, &["disable", "nobody"]).is_err());
        // reset-password (LG-425: it ends the account's older identity tokens)
        lobby
            .execute("UPDATE accounts SET tokens_valid_after = 0", [])
            .unwrap();
        let before = crate::clock::unix_now();
        cli(&p, &["reset-password", "Erika", "newpass1"]).unwrap();
        let a = db::account_by_login(&lobby, "Erika").unwrap().unwrap();
        assert!(a.tokens_valid_after >= before);
        assert_eq!(
            crate::auth::pw_verify(&a, "newpass1"),
            crate::auth::PwCheck::Ok
        );
        assert_eq!(
            crate::auth::pw_verify(&a, "hunter22"),
            crate::auth::PwCheck::Bad
        );
        // invites: plain codes and a code bound to a new password-less account
        let now = crate::clock::unix_now();
        let codes = cli(&p, &["invite", "--count", "3", "--days", "2"]).unwrap();
        let codes: Vec<&str> = codes.lines().collect();
        assert_eq!(codes.len(), 3);
        for k in &codes {
            assert_eq!(k.len(), 14);
            assert_eq!(invite(&lobby, k, now).unwrap(), Invite::Open(None));
        }
        assert_eq!(
            invite(&lobby, codes[0], now + 3 * 86_400).unwrap(),
            Invite::Expired
        );
        let out = cli(&p, &["add", "Kim", "--invite"]).unwrap();
        let code = out.split("invite code for Kim: ").nth(1).unwrap()[..14].to_string();
        assert_eq!(
            invite(&lobby, &code.to_lowercase(), now).unwrap(),
            Invite::Open(Some("Kim".into()))
        );
        let list = cli(&p, &["list"]).unwrap();
        assert!(
            list.contains("Erika") && list.contains("argon2id"),
            "{list}"
        );
        assert!(list.contains("Kim") && list.contains("none"), "{list}");
        assert_eq!(list.matches("invite ").count(), 4, "{list}");
        assert!(consume_invite(&lobby, &code, 1002, 5).unwrap());
        assert!(
            !consume_invite(&lobby, &code, 1002, 6).unwrap(),
            "one use only"
        );
        assert_eq!(invite(&lobby, &code, 0).unwrap(), Invite::Used);
        assert_eq!(
            invite(&lobby, "NOPE-NOPE-NOPE", 0).unwrap(),
            Invite::Unknown
        );
        let list = cli(&p, &["list"]).unwrap();
        assert_eq!(
            list.matches("invite ").count(),
            3,
            "a used code is not listed: {list}"
        );
        assert!(cli(&p, &["frobnicate"]).is_err());
        drop(lobby);
        remove(&p);
    }
}
