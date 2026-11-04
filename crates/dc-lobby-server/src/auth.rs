//! Auth server: file check, login, nickname / referrer / character creation, server list and server pick.
// SPEC: PROTOCOL_2009.md §2 (flow §2.1, layouts §2.2)

use crate::clock;
use crate::db;
use crate::grant;
use crate::inventory;
use crate::msg::*;
use crate::state::{Core, HResult, Sid, Ticket};
use crate::views::face_index;
use crate::wire::{self, LoginReply, ServerEntry};
use rusqlite::params;
use std::time::{Duration, Instant};

/// Logins arrive as `LOCAL:<name>` from our launcher; the account key is stored without the prefix.
// SPEC: LOBBY_HARNESS.md §6 (accounts.login stored WITHOUT the `LOCAL:` prefix)
pub fn normalize_login(token: &str) -> String {
    let t = token.trim();
    let t = match t.get(..6) {
        Some(p) if p.eq_ignore_ascii_case("LOCAL:") => &t[6..],
        _ => t,
    };
    t.trim().to_string()
}

/// Salted SHA-256 of a password: the store before LG-140 (rows with a `pw_salt`); only verified now, never written.
pub fn pw_digest(salt: &str, pw: &str) -> String {
    let mut m = salt.as_bytes().to_vec();
    m.push(0);
    m.extend_from_slice(pw.as_bytes());
    dc_rom::sha256_hex(&m)
}

/// Argon2id hash of a password as a PHC string (`$argon2id$v=19$m=...`), with a 16-byte salt from `keys`.
// DC-DEFAULT: LG-140 Argon2id for every password written
pub fn hash_password(keys: &mut crate::state::KeySource, pw: &str) -> String {
    crate::pwhash::encode(&keys.key16(), pw)
}

/// The box of a login that was not checked for want of room: the general login failure, as for an empty name.
const REASON_LOGIN_FAILED: u32 = 0x0003_0000;

/// Result of a password check against the stored row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PwCheck {
    /// Matches the stored Argon2id hash.
    Ok,
    /// Matches an old salted SHA-256 row: the caller rewrites it as Argon2id (LG-140).
    OkUpgrade,
    Bad,
    /// The account has no password yet.
    NoPassword,
}

/// Checks `pw` against an account row. A row with a `pw_salt` is the old salted SHA-256 store; a row without one
/// holds an Argon2id PHC string.
pub fn pw_verify(acc: &db::Account, pw: &str) -> PwCheck {
    let Some(h) = acc.pw_hash.as_deref().filter(|h| !h.is_empty()) else {
        return PwCheck::NoPassword;
    };
    match acc.pw_salt.as_deref().filter(|s| !s.is_empty()) {
        Some(salt) if pw_digest(salt, pw) == h => PwCheck::OkUpgrade,
        Some(_) => PwCheck::Bad,
        None if crate::pwhash::verify(h, pw) => PwCheck::Ok,
        None => PwCheck::Bad,
    }
}

/// Stores a new Argon2id hash for `user_no` (clears the old salt column).
pub fn store_password(st: &mut Core, user_no: u32, pw: &str) -> db::DbResult<()> {
    let h = hash_password(&mut st.keys, pw);
    db::set_password_phc(&st.db, user_no, &h)
}

/// Name acceptable as a nick: 1..16 UTF-16 units, no control characters, no leading/trailing space.
pub fn valid_nick(n: &str) -> bool {
    let len = n.encode_utf16().count();
    (1..=16).contains(&len) && n.trim() == n && !n.chars().any(|c| c.is_control())
}

/// The server list: the lobby entry and the clan lobby entry.
// DC-DEFAULT: CD-21 one server (plus the clan lobby the harness expects on --clanport); live user counts
pub fn server_entries(st: &Core) -> Vec<ServerEntry> {
    server_entries_at(st, &st.opts.lobbyip)
}

/// The server list as one connection sees it: the entries carry the address announced to that peer (LG-30).
pub fn server_entries_for(st: &Core, sid: Sid) -> Vec<ServerEntry> {
    let ip = st.announced_ip(sid, &st.opts.lobbyip);
    server_entries_at(st, &ip)
}

fn server_entries_at(st: &Core, ip: &str) -> Vec<ServerEntry> {
    let online = |id: Option<u32>| {
        st.sessions
            .values()
            .filter(|s| s.in_lobby && !s.closing && id.map(|i| s.server == i).unwrap_or(true))
            .count() as u32
    };
    let cap = st.opts.server_max.clamp(1, u32::MAX as i64) as u32;
    let configured = crate::servers::entries(st);
    // CD-21 amended: one entry counts every lobby session (as before), several count the sessions that picked them
    let single = configured.len() == 1;
    let mut v: Vec<ServerEntry> = configured
        .iter()
        .map(|e| ServerEntry {
            id: e.id,
            name: e.name.clone(),
            ip: ip.to_string(),
            port: st.opts.lobbyport as u32,
            open: true,
            capacity: e.capacity,
            current: online((!single).then_some(e.id)),
            op_capacity: crate::gm::OPERATOR_SLOTS,
            op_current: if single {
                crate::gm::gms_online(st, None)
            } else {
                e.channel_ids()
                    .map(|ch| crate::gm::gms_online(st, Some(ch)))
                    .sum()
            },
            clan_only: false,
        })
        .collect();
    v.push(ServerEntry {
        id: crate::servers::clan_id(st),
        name: st.opts.clan_name.clone(),
        ip: ip.to_string(),
        port: st.opts.clanport as u32,
        open: true,
        capacity: cap,
        current: 0,
        op_capacity: crate::gm::OPERATOR_SLOTS,
        op_current: 0,
        clan_only: true,
    });
    v
}

fn reply_for(st: &Core, account: u32) -> LoginReply {
    let c = st.character(account).unwrap_or_default();
    let gm = db::account_by_no(&st.db, account)
        .ok()
        .flatten()
        .map(|a| a.gm)
        .unwrap_or(0);
    LoginReply {
        ok: true,
        account,
        exp: c.exp.max(0) as u32,
        clan_id: c.clan_uid.max(0) as u32,
        kills: c.kills.max(0) as u32,
        deaths: c.deaths.max(0) as u32,
        // DC-DEFAULT: LG-170 a GM account (accounts.gm != 0) logs in with the operator privilege 2
        privilege: crate::gm::privilege_of(gm),
        ..Default::default()
    }
}

/// Creates the character of a new account plus its starter kit, then applies `--grant` when it targets it.
pub fn create_character(st: &mut Core, account: u32, nick: &str, face: i64) -> HResult {
    let ci = st.rules.names_case_insensitive;
    // LG-260: a bot's face comes from the creation table by its name
    let login_of = db::account_by_no(&st.db, account)
        .ok()
        .flatten()
        .map(|a| a.login)
        .unwrap_or_else(|| nick.to_string());
    let face = if st.opts.bot_faces_random && grant::is_bot_name(&login_of) {
        st.rom
            .random_face(grant::bot_seed(&login_of))
            .unwrap_or(face)
    } else {
        face
    };
    db::create_character(
        &st.db,
        &db::NewCharacter {
            user_no: account,
            nick,
            face,
            exp: grant::new_character_exp(&st.opts, &st.rom.ranks).unwrap_or(st.opts.new_exp),
            gp: st.opts.new_gp,
            nx: st.opts.new_nx,
            now: clock::unix_now(),
        },
        ci,
    )?;
    grant::set_new_credit(&st.db, account, st.opts.new_nx_credit)?;
    inventory::grant_starter(st, account)?;
    let login = login_of;
    let lo = grant::options_for_account_with(&st.opts, &login, Some(&st.rom));
    if !lo.start_loadout.trim().is_empty() {
        let now = crate::clock::now_ticks();
        match grant::apply_start_loadout(&st.db, &lo, &st.rom, account, now) {
            Ok(seed) => {
                let opts = grant::start_loadout_options(&lo, &st.rom).unwrap_or_default();
                st.note(&format!(
                    "start loadout equipped on {} ({}): {}",
                    account,
                    login,
                    seed.iter()
                        .map(|(s, i)| {
                            let mut t = format!("{s}:{i}");
                            for o in opts.get(s).into_iter().flatten() {
                                t.push_str(&format!("+{o}"));
                            }
                            t
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                ))
            }
            Err(e) => st.note(&format!("start loadout not applied to {}: {}", account, e)),
        }
    }
    if let Ok(Some(g)) = grant::resolve(&st.opts, &st.rom.ranks) {
        if grant::is_target(&st.db, &g, account) {
            grant::apply(&st.db, &g, account)?;
            st.note(&format!(
                "grant applied to new character {} ({})",
                nick, account
            ));
        }
    }
    st.note(&format!(
        "character created: account {} nick {}",
        account, nick
    ));
    Ok(())
}

/// A free nick derived from `base` (`--autochar`, test bots).
/// A free character name for a server-made character (autochar, test bots): the login, unless it carries the GM
/// prefix a non-GM account may not use (LG-172), then "Player".
pub fn auto_nick(st: &Core, account: u32, login: &str) -> String {
    let n = free_nick(st, login);
    if crate::gm::name_reserved_for(st, Some(account), &n) {
        free_nick(st, "Player")
    } else {
        n
    }
}

pub fn free_nick(st: &Core, base: &str) -> String {
    let base: String = base
        .chars()
        .filter(|c| !c.is_control() && *c != ' ')
        .take(16)
        .collect();
    let base = if base.is_empty() {
        "Player".to_string()
    } else {
        base
    };
    let ci = st.rules.names_case_insensitive;
    let taken = |n: &str| {
        db::character_by_nick(&st.db, n, ci)
            .ok()
            .flatten()
            .is_some()
    };
    if !taken(&base) {
        return base;
    }
    for i in 2..10000 {
        let suffix = i.to_string();
        let keep = 16usize.saturating_sub(suffix.len());
        let cand: String = base.chars().take(keep).collect::<String>() + &suffix;
        if !taken(&cand) {
            return cand;
        }
    }
    base
}

/// `--add-account`: creates the account of `name` (a login token) when missing and sets its password. Returns the
/// account number.
// DC-DEFAULT: LG-32 pre-created accounts for a public server
pub fn ensure_account(st: &mut Core, name: &str, password: &str) -> db::DbResult<u32> {
    let login = normalize_login(name);
    let acc = account_for(st, &login)?;
    store_password(st, acc.user_no, password)?;
    Ok(acc.user_no)
}

/// Finds or creates the account of a login token.
// DC-DEFAULT: CD-14 an unknown login creates its account
pub fn account_for(st: &Core, login: &str) -> db::DbResult<db::Account> {
    if let Some(a) = db::account_by_login(&st.db, login)? {
        return Ok(a);
    }
    let no = db::create_account_from(&st.db, login, clock::unix_now(), st.rules.first_account_no)?;
    Ok(db::account_by_no(&st.db, no)?.unwrap_or_default())
}

/// A login whole, under one lock (the tests' way in). The connection threads use [`login_begin`],
/// [`LoginPending::compute`] on the pool and [`login_finish`], so that Argon2 runs outside the lock (LG-328).
fn on_login(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    match login_begin(st, sid, p)? {
        LoginStep::Done => Ok(()),
        LoginStep::Check(pending) => {
            let checked = pending.compute();
            login_finish(st, sid, pending, checked)
        }
    }
}

/// Phase 1 of a login (locked): everything before the password check, in the order of the checks.
// DC-DEFAULT: LG-328 (proposed, project decision)
pub fn login_begin(st: &mut Core, sid: Sid, p: &[u8]) -> Result<LoginStep, crate::state::Fault> {
    let req = wire::parse_login(p);
    // LG-271: the mod-set / content suffix comes off first (every login form), then the hashes must match ours
    let (field, mods) = dc_wire::loginmods::split(&req.token);
    if !mods_match(st, sid, &mods) {
        st.send(
            sid,
            wire::login_reply(&LoginReply {
                reason: wire::REASON_BAD_PASSWORD,
                ..Default::default()
            }),
        );
        return Ok(LoginStep::Done);
    }
    // LG-317: a dc-botclient says what it is; the mark goes on the account below, once the login holds
    let (field, declared_bot) = dc_wire::loginmods::split_bot(field);
    let login = normalize_login(field);
    // LG-318: the bot mark without a name asks for a seat of the bot pool
    if declared_bot && login.is_empty() {
        crate::botpool::seat_login(st, sid)?;
        return Ok(LoginStep::Done);
    }
    // LG-68: `tok:<token>` in the login-name field logs in with an identity token (the password field is ignored)
    if let Some(token) = login.strip_prefix(crate::identity::LOGIN_PREFIX) {
        token_login(st, sid, token)?;
        return Ok(LoginStep::Done);
    }
    if login.is_empty() {
        st.send(
            sid,
            wire::login_reply(&LoginReply {
                reason: 0x0003_0000,
                ..Default::default()
            }),
        );
        return Ok(LoginStep::Done);
    }
    // LG-32: on a public server a loopback login (the host's own client and bots) skips the account gate
    let exempt = st.opts.public
        && st
            .sessions
            .get(&sid)
            .map(|s| crate::announce::is_loopback_peer(&s.peer))
            .unwrap_or(false);
    // LG-146: a name or address with ten failed logins in ten minutes is refused for five minutes, password unchecked (never a permanent lock)
    let peer = st
        .sessions
        .get(&sid)
        .map(|s| s.peer.clone())
        .unwrap_or_default();
    let keys = crate::limits::login_keys(&login, &peer);
    let now = Instant::now();
    if !exempt && st.limits.login.locked(&keys, now).is_some() {
        st.send(
            sid,
            wire::login_reply(&LoginReply {
                reason: wire::REASON_BAD_PASSWORD,
                ..Default::default()
            }),
        );
        return Ok(LoginStep::Done);
    }
    // DC-DEFAULT: CD-14 / LG-32 an unknown login is refused when auto-create is off (same box as a wrong password)
    if !exempt && !st.opts.auto_create && db::account_by_login(&st.db, &login)?.is_none() {
        count_failure(st, &keys, now);
        st.note(&format!("login refused (unknown account): {}", login));
        st.send(
            sid,
            wire::login_reply(&LoginReply {
                reason: wire::REASON_BAD_PASSWORD,
                ..Default::default()
            }),
        );
        return Ok(LoginStep::Done);
    }
    let acc = account_for(st, &login)?;
    // LG-318: a pool account has no password and no way in by name, also from this machine
    if st.pool.has(acc.user_no) {
        count_failure(st, &keys, now);
        st.note(&format!("login refused (bot pool account): {}", login));
        st.send(
            sid,
            wire::login_reply(&LoginReply {
                reason: wire::REASON_BAD_PASSWORD,
                ..Default::default()
            }),
        );
        return Ok(LoginStep::Done);
    }
    // LG-142: a disabled account is refused with the wrong-password box, before any password check
    if acc.disabled {
        st.note(&format!("login refused (account disabled): {}", login));
        st.send(
            sid,
            wire::login_reply(&LoginReply {
                reason: wire::REASON_BAD_PASSWORD,
                ..Default::default()
            }),
        );
        return Ok(LoginStep::Done);
    }
    // LG-69: an account made for an identity token logs in with a token only (it has no password of its own)
    if !exempt && db::is_identity_linked(&st.db, acc.user_no)? {
        count_failure(st, &keys, now);
        st.note(&format!(
            "login refused (identity account, token login only): {}",
            login
        ));
        st.send(
            sid,
            wire::login_reply(&LoginReply {
                reason: wire::REASON_BAD_PASSWORD,
                ..Default::default()
            }),
        );
        return Ok(LoginStep::Done);
    }
    if st.opts.pw_check && !exempt {
        // LG-328: the check runs off the lock; its place counts against the name's and the address's limit meanwhile
        if !st.limits.login.begin_attempt(&keys, now) {
            refuse_bad_password(st, sid);
            return Ok(LoginStep::Done);
        }
        let salt = st.keys.key16();
        let public = st.opts.public;
        return Ok(LoginStep::Check(LoginPending {
            acc,
            login,
            keys,
            password: req.password,
            declared_bot,
            salt,
            public,
        }));
    }
    login_accepted(st, sid, &acc, &login, declared_bot)?;
    Ok(LoginStep::Done)
}

fn refuse_bad_password(st: &mut Core, sid: Sid) {
    st.send(
        sid,
        wire::login_reply(&LoginReply {
            reason: wire::REASON_BAD_PASSWORD,
            ..Default::default()
        }),
    );
}

fn login_accepted(
    st: &mut Core,
    sid: Sid,
    acc: &db::Account,
    login: &str,
    declared_bot: bool,
) -> HResult {
    if declared_bot {
        st.db.execute(
            "UPDATE accounts SET self_bot = 1 WHERE user_no = ?1",
            [acc.user_no as i64],
        )?;
    }
    finish_login(st, sid, acc, login, None)
}

/// What the first (locked) phase of a login leaves to do.
#[allow(clippy::large_enum_variant)] // one value per login, moved straight into the next phase; boxing buys nothing
pub enum LoginStep {
    /// Answered.
    Done,
    /// The password work still to run, outside the `Core` lock.
    Check(LoginPending),
}

/// A login waiting for its password check.
pub struct LoginPending {
    acc: db::Account,
    login: String,
    keys: [String; 2],
    password: String,
    declared_bot: bool,
    salt: [u8; 16],
    public: bool,
}

/// What the slow part of a login found: the check, and the Argon2id string to store when it calls for a new one.
pub struct LoginChecked {
    check: PwCheck,
    new_phc: Option<String>,
}

impl LoginPending {
    /// The slow part; takes no lock and changes nothing.
    pub fn compute(&self) -> LoginChecked {
        let check = pw_verify(&self.acc, &self.password);
        let store = match check {
            PwCheck::OkUpgrade => true,
            PwCheck::NoPassword => !self.public,
            PwCheck::Ok | PwCheck::Bad => false,
        };
        LoginChecked {
            check,
            new_phc: store.then(|| crate::pwhash::encode(&self.salt, &self.password)),
        }
    }

    /// The limit keys of this login.
    pub fn keys(&self) -> &[String; 2] {
        &self.keys
    }
}

/// The pool had no room for the check: nothing is counted against the player, the box says to try again.
pub fn login_busy(st: &mut Core, sid: Sid, keys: &[String]) {
    st.limits.login.end_attempt(keys);
    st.note("login refused: too many password checks waiting (LG-328)");
    st.send(
        sid,
        wire::login_reply(&LoginReply {
            reason: REASON_LOGIN_FAILED,
            ..Default::default()
        }),
    );
}

/// Phase 2 (locked again): the outcome. The account is read again: it may have been disabled or given a new password
/// while the check ran.
pub fn login_finish(st: &mut Core, sid: Sid, p: LoginPending, c: LoginChecked) -> HResult {
    let LoginPending {
        acc,
        login,
        keys,
        declared_bot,
        ..
    } = p;
    st.limits.login.end_attempt(&keys);
    let now = Instant::now();
    let fresh = db::account_by_no(&st.db, acc.user_no)?;
    let same = fresh
        .as_ref()
        .is_some_and(|f| !f.disabled && f.pw_hash == acc.pw_hash && f.pw_salt == acc.pw_salt);
    if !same {
        st.note(&format!(
            "login refused (account changed during the check): {login}"
        ));
        refuse_bad_password(st, sid);
        return Ok(());
    }
    match c.check {
        PwCheck::Ok => {}
        // LG-140: an old salted SHA-256 row is rewritten as Argon2id on its first good login
        PwCheck::OkUpgrade => {
            if let Some(h) = &c.new_phc {
                db::set_password_phc(&st.db, acc.user_no, h)?;
            }
            st.note(&format!("password of {} upgraded to Argon2id", login));
        }
        PwCheck::Bad => {
            count_failure(st, &keys, now);
            st.note(&format!("login refused (password): {}", login));
            refuse_bad_password(st, sid);
            return Ok(());
        }
        // LG-32: on a public server a remote login never claims an account that has no password yet
        PwCheck::NoPassword if c.new_phc.is_none() => {
            count_failure(st, &keys, now);
            st.note(&format!(
                "login refused (no password set, public): {}",
                login
            ));
            refuse_bad_password(st, sid);
            return Ok(());
        }
        // first login with the check on sets the password
        PwCheck::NoPassword => {
            if let Some(h) = &c.new_phc {
                db::set_password_phc(&st.db, acc.user_no, h)?;
            }
        }
    }
    st.limits.login.success(&keys[0]);
    login_accepted(st, sid, &acc, &login, declared_bot)
}

/// LG-271: the client's mod-set hash must equal this lobby's (a vanilla lobby: the empty-set hash; a login without
/// the suffix stands for it) and its content hash the content this lobby's game servers get (`0`: none). A mismatch
/// is logged; the caller refuses the login with the wrong-password box (not counted as a failed password).
// DC-DEFAULT: LG-271 (proposed, project decision)
fn mods_match(st: &Core, sid: Sid, mods: &dc_wire::loginmods::LoginMods) -> bool {
    use dc_wire::loginmods::short;
    let want = &st.login_mods;
    let what = if mods.set != want.set {
        Some(("mod set", &mods.set, &want.set))
    } else if mods.content != want.content {
        Some(("mod content", &mods.content, &want.content))
    } else {
        None
    };
    let Some((what, client, server)) = what else {
        return true;
    };
    let peer = st
        .sessions
        .get(&sid)
        .map(|s| s.peer.clone())
        .unwrap_or_default();
    st.note(&format!(
        "client {peer}: {what} mismatch (client {}…, server {}…){}",
        short(client),
        short(server),
        if mods.sent { "" } else { " (no mod hash sent)" }
    ));
    false
}

/// LG-68 / LG-69: a login with an identity token. A refused token gets the wrong-password box and counts as a failed
/// login of the source address (LG-146); a good one goes on exactly as a password login.
// DC-DEFAULT: LG-68 token login, LG-69 local account mapping
fn token_login(st: &mut Core, sid: Sid, token: &str) -> HResult {
    let peer = st
        .sessions
        .get(&sid)
        .map(|s| s.peer.clone())
        .unwrap_or_default();
    let ip = crate::limits::peer_ip(&peer);
    // DC-DEFAULT: LG-330 (proposed, project decision) a refusal names no account number
    let refuse = |st: &mut Core| refuse_bad_password(st, sid);
    if !st.identity.accepts() {
        st.note(&format!(
            "login refused (identity tokens are not accepted here) from {ip}"
        ));
        refuse(st);
        return Ok(());
    }
    let addr = [crate::limits::login_keys("", &peer)[1].clone()];
    let now = Instant::now();
    if st.limits.login.locked(&addr, now).is_some() {
        refuse(st);
        return Ok(());
    }
    let claims = match st.identity.check(token, clock::unix_now()) {
        Ok(c) => c,
        Err(e) => {
            count_failure(st, &addr, now);
            st.note(&format!("login refused (identity token: {e}) from {ip}"));
            refuse(st);
            return Ok(());
        }
    };
    // LG-146: the name in the token is locked out like a password login's name, and its refusals count against it
    let keys = crate::limits::login_keys(&claims.login, &peer);
    if st.limits.login.locked(&keys, now).is_some() {
        refuse(st);
        return Ok(());
    }
    let acc = match crate::identity::local_account(st, &claims)? {
        Ok(a) => a,
        Err(why) => {
            count_failure(st, &keys, now);
            st.note(&format!("login refused (identity token): {why}"));
            refuse(st);
            return Ok(());
        }
    };
    if acc.disabled {
        count_failure(st, &keys, now);
        st.note(&format!("login refused (account disabled): {}", acc.login));
        refuse(st);
        return Ok(());
    }
    st.note(&format!(
        "identity: token login of {}:{} ({}) as account {}",
        claims.issuer, claims.account, claims.login, acc.user_no
    ));
    let login = acc.login.clone();
    finish_login(st, sid, &acc, &login, Some(&claims))
}

/// LG-273: the character name of an identity's first login on this lobby: the token's nick when no character here
/// holds it, else the nick with the first free number (`Name2`, `Name3`, ...; cut to 16 UTF-16 units). None when the
/// token carries no usable nick (empty, not a valid name, or the GM prefix on a non-GM account): the nickname screen
/// opens as for a local account.
// DC-DEFAULT: LG-273 (proposed, project decision) the in-game name follows the identity
pub fn identity_nick(st: &Core, account: u32, nick: &str) -> Option<String> {
    if !valid_nick(nick) || crate::gm::name_reserved_for(st, Some(account), nick) {
        return None;
    }
    let ci = st.rules.names_case_insensitive;
    let taken = |n: &str| {
        db::character_by_nick(&st.db, n, ci)
            .ok()
            .flatten()
            .is_some()
    };
    if !taken(nick) {
        return Some(nick.to_string());
    }
    for i in 2..10000u32 {
        let suffix = i.to_string();
        let room = 16usize.saturating_sub(suffix.len());
        let mut base = String::new();
        for c in nick.chars() {
            if base.encode_utf16().count() + c.len_utf16() > room {
                break;
            }
            base.push(c);
        }
        let cand = format!("{}{suffix}", base.trim_end());
        if valid_nick(&cand)
            && !crate::gm::name_reserved_for(st, Some(account), &cand)
            && !taken(&cand)
        {
            return Some(cand);
        }
    }
    None
}

/// The common end of a password or token login: ban check, session, character (or the nickname screen), reply.
/// `identity`: the claims of a token login; its first login here makes the character from the token's nick (LG-273).
pub(crate) fn finish_login(
    st: &mut Core,
    sid: Sid,
    acc: &db::Account,
    login: &str,
    identity: Option<&crate::identity::Claims>,
) -> HResult {
    if crate::gm::refuse_banned_login(st, sid, acc.user_no)? {
        return Ok(());
    }
    db::touch_login(&st.db, acc.user_no, clock::unix_now())?;
    if let Some(s) = st.sessions.get_mut(&sid) {
        s.account = Some(acc.user_no);
    }
    if st.character(acc.user_no).is_none() {
        // LG-273: an identity login never sees the nickname screen when its token names a usable nick
        if let Some(c) = identity {
            if let Some(nick) = identity_nick(st, acc.user_no, &c.nick) {
                if !nick.eq_ignore_ascii_case(&c.nick) {
                    let holder =
                        db::character_by_nick(&st.db, &c.nick, st.rules.names_case_insensitive)?
                            .map(|h| h.user_no)
                            .unwrap_or(0);
                    st.note(&format!(
                        "identity: nick {} taken on this lobby by account {}; {} used for {}:{}",
                        c.nick, holder, nick, c.issuer, c.account
                    ));
                }
                let face = st.rom.faces.first().copied().unwrap_or(0);
                create_character(st, acc.user_no, &nick, face)?;
            } else {
                st.note(&format!(
                    "identity: the token of {}:{} carries no usable nick ({:?}); the nickname screen opens",
                    c.issuer, c.account, c.nick
                ));
            }
        }
    }
    if st.character(acc.user_no).is_none() {
        if st.opts.autochar {
            let nick = auto_nick(st, acc.user_no, login);
            create_character(st, acc.user_no, &nick, 0)?;
        } else {
            // DC-DEFAULT: CD-15 no character -> reason 0x0003001e opens the nickname screen
            st.send(
                sid,
                wire::login_reply(&LoginReply {
                    reason: wire::REASON_NO_CHARACTER,
                    account: acc.user_no,
                    ..Default::default()
                }),
            );
            st.note(&format!(
                "login: {} (account {}) has no character",
                login, acc.user_no
            ));
            return Ok(());
        }
    }
    st.note(&format!("login: {} (account {})", login, acc.user_no));
    let r = reply_for(st, acc.user_no);
    st.send(sid, wire::login_reply(&r));
    send_server_list(st, sid);
    Ok(())
}

/// LG-272: the entries naming other lobby instances, as our clients read them (none: no push).
pub fn other_lobbies(st: &Core) -> Vec<dc_wire::moddedlist::ListedLobby> {
    st.opts
        .other_lobbies
        .iter()
        .map(|e| dc_wire::moddedlist::ListedLobby {
            name: e.name.trim().to_string(),
            region: e.region.trim().to_string(),
            lobby: e.lobby.trim().to_string(),
            fingerprint: e.fingerprint.trim().to_string(),
            policy: match e.policy.trim() {
                "" => "modded".to_string(),
                p => p.to_string(),
            },
        })
        .collect()
}

/// The login's server list: our push 0x3f02 with the other lobbies first (only when there are any, LG-272; the 2009
/// dispatcher drops the id), then the 2009 list 0x2000.
// SPEC: PROTOCOL_2009.md §1.2 (unknown ids are dropped), §2.2 0x2000
// DC-DEFAULT: LG-272 (proposed, project decision)
fn send_server_list(st: &mut Core, sid: Sid) {
    let others = other_lobbies(st);
    if !others.is_empty() {
        st.send(sid, dc_wire::moddedlist::frame(&others));
    }
    // LG-313: the hashes of the featured entries' sets (only when there are any; the 2009 dispatcher drops the id)
    if !st.entry_mods.is_empty() {
        let sets: Vec<_> = st
            .entry_mods
            .values()
            .map(crate::modset::EntrySet::hashes)
            .collect();
        st.send(sid, dc_wire::entrymods::push_frame(&sets));
    }
    let list = wire::server_list_push(&server_entries_for(st, sid));
    st.send(sid, list);
}

/// Counts one failed login for the name and the address (LG-146); the log says so once when a lockout starts.
pub(crate) fn count_failure(st: &mut Core, keys: &[String], now: Instant) {
    for k in st.limits.login.fail(keys, now) {
        let (kind, what) = k.split_once(':').unwrap_or(("key", &k));
        st.note(&format!(
            "login lockout: {} failed logins for {} {} within 10 minutes; its logins are refused for {} s",
            crate::limits::LOGIN_FAILURES,
            if kind == "addr" { "address" } else { kind },
            what,
            crate::limits::LOCKOUT.as_secs()
        ));
    }
}

fn session_account(st: &Core, sid: Sid) -> Option<u32> {
    st.sessions.get(&sid).and_then(|s| s.account)
}

fn on_name_check(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let name = wire::parse_name_at6(p);
    let acc = session_account(st, sid);
    let free = acc.is_some()
        && valid_nick(&name)
        && !crate::gm::name_reserved_for(st, acc, &name)
        && db::character_by_nick(&st.db, &name, st.rules.names_case_insensitive)?.is_none();
    st.send(sid, wire::name_check_reply(free));
    Ok(())
}

// DC-DEFAULT: CD-17 referrer accepted when it names an existing character, else reason 0x38
fn on_referrer(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let name = wire::parse_name_at6(p);
    let Some(acc) = session_account(st, sid) else {
        st.send(sid, wire::referrer_check_reply(false, 0x38));
        return Ok(());
    };
    match db::character_by_nick(&st.db, &name, true)? {
        Some(r) if r.user_no != acc => {
            st.db.execute(
                "INSERT OR REPLACE INTO recommends (user_no, recommender) VALUES (?1, ?2)",
                params![acc as i64, r.nick],
            )?;
            st.send(sid, wire::referrer_check_reply(true, 0));
        }
        _ => st.send(sid, wire::referrer_check_reply(false, 0x38)),
    }
    Ok(())
}

fn on_character_create(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (face, name) = wire::parse_new_char(p);
    let Some(acc) = session_account(st, sid) else {
        st.send(sid, wire::new_char_reply(false, 0xf, 0, 0));
        return Ok(());
    };
    if st.character(acc).is_some() {
        st.send(sid, wire::new_char_reply(false, 0xf, acc, 0));
        return Ok(());
    }
    // DC-DEFAULT: LG-172 a "GM" name prefix is refused for a non-GM account with the bad-name code 0xe
    if !valid_nick(&name)
        || crate::gm::name_reserved_for(st, Some(acc), &name)
        || db::character_by_nick(&st.db, &name, st.rules.names_case_insensitive)?.is_some()
    {
        st.send(sid, wire::new_char_reply(false, 0xe, acc, 0));
        return Ok(());
    }
    create_character(st, acc, &name, face_index(face))?;
    let level = st.character(acc).map(|c| st.rank_of(c.exp)).unwrap_or(0);
    st.send(sid, wire::new_char_reply(true, 0, acc, level));
    // no screen change on success: the server follows with the server list (SPEC: PROTOCOL_2009.md §2.2 0x203)
    send_server_list(st, sid);
    Ok(())
}

/// Issues a session key for `account` on `server` (0x20b / 0x3b3).
pub fn issue_ticket(st: &mut Core, account: u32, server: u32) -> [u8; 16] {
    let now = Instant::now();
    st.tickets.retain(|_, t| t.expires > now);
    let key = st.keys.key16();
    let ttl = Duration::from_secs(st.rules.session_ttl_secs);
    st.tickets.insert(
        key,
        Ticket {
            account,
            server,
            expires: now + ttl,
        },
    );
    crate::botpool::on_ticket(st, account, now + ttl);
    key
}

fn on_server_pick(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let server = dc_wire::PktReader::new(p).at(6).u32();
    let acc = session_account(st, sid);
    let known = server_entries(st).iter().any(|e| e.id == server && e.open);
    let known = known && acc.is_some() && !crate::servers::pick_refused(st, sid, server);
    // LG-318: no bot seat on the clan server
    let known =
        known && !(server == crate::servers::clan_id(st) && acc.is_some_and(|a| st.pool.has(a)));
    match acc {
        Some(a) if known && st.character(a).is_some() => {
            let key = issue_ticket(st, a, server);
            st.send(sid, wire::session_key_reply(SERVER_PICK_REPLY, true, &key));
        }
        _ => st.send(
            sid,
            wire::session_key_reply(SERVER_PICK_REPLY, false, &[0u8; 16]),
        ),
    }
    Ok(())
}

/// Auth-side messages; returns false when `id` is not one of them.
pub fn dispatch(st: &mut Core, sid: Sid, id: u16, p: &[u8]) -> Result<bool, crate::state::Fault> {
    match id {
        // DC-DEFAULT: CD-13 the file check is not verified (0x207 ok = 1)
        FILE_CHECK => st.send(sid, wire::file_check_reply(true)),
        LOGIN_REQUEST => on_login(st, sid, p)?,
        NAME_CHECK => on_name_check(st, sid, p)?,
        REFERRER_CHECK => on_referrer(st, sid, p)?,
        NEW_CHAR => on_character_create(st, sid, p)?,
        SERVER_PICK => on_server_pick(st, sid, p)?,
        dc_wire::entrymods::CLAIM => crate::servers::on_entry_claim(st, sid, p),
        _ => return Ok(false),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::Listener;
    use crate::state::testing::*;
    use dc_wire::PktWriter;

    fn new_account(st: &mut Core) -> u32 {
        let mut acc = 0;
        st.scope(|st| {
            acc = ensure_account(st, "neo", "red pill")?;
            Ok(())
        })
        .unwrap();
        acc
    }

    fn begin_login(st: &mut Core, p: &[u8]) -> LoginStep {
        let mut r = None;
        st.scope(|st| {
            r = Some(login_begin(st, 1, p)?);
            Ok(())
        })
        .unwrap();
        r.unwrap()
    }

    /// LG-330: no refused login names the account.
    #[test]
    fn a_refused_login_does_not_name_the_account() {
        let mut st = state();
        st.opts.pw_check = true;
        let acc = new_account(&mut st);
        assert!(acc > 0);
        let rx = connect(&mut st, 1, Listener::Auth);
        run(
            &mut st,
            1,
            LOGIN_REQUEST,
            &login_payload("neo", "blue pill"),
        );
        let got = drain(&rx);
        assert_eq!(u32_at(&got[0].1, 0xa), wire::REASON_BAD_PASSWORD);
        let no = acc.to_le_bytes();
        assert!(
            !got[0].1.windows(4).any(|w| w == no),
            "the account number is not in the reply"
        );
    }

    /// LG-328: the phases keep the limit: a check under way counts, its end gives the place back, a full pool counts
    /// nothing against the player, and an account disabled during the check is refused.
    #[test]
    fn the_two_login_phases_count_right() {
        let mut st = state();
        st.opts.pw_check = true;
        let acc = new_account(&mut st);
        let _rx = connect(&mut st, 1, Listener::Auth);
        let good = login_payload("neo", "red pill");
        let mut pending = Vec::new();
        for _ in 0..crate::limits::LOGIN_FAILURES {
            match begin_login(&mut st, &good) {
                LoginStep::Check(p) => pending.push(p),
                LoginStep::Done => panic!("a place is free"),
            }
        }
        assert!(
            matches!(begin_login(&mut st, &good), LoginStep::Done),
            "ten checks at once are the limit"
        );
        let keys = pending[0].keys().clone();
        st.scope(|st| {
            login_busy(st, 1, &keys);
            Ok(())
        })
        .unwrap();
        assert!(
            st.limits.login.locked(&keys, Instant::now()).is_none(),
            "a full pool counts no failure"
        );
        // disabled while its check runs
        let p = pending.pop().unwrap();
        let c = p.compute();
        st.db
            .execute(
                "UPDATE accounts SET disabled = 1 WHERE user_no = ?1",
                [acc as i64],
            )
            .unwrap();
        let rx2 = connect(&mut st, 2, Listener::Auth);
        st.scope(|st| login_finish(st, 2, p, c)).unwrap();
        assert_eq!(u32_at(&drain(&rx2)[0].1, 0xa), wire::REASON_BAD_PASSWORD);
    }

    fn login_payload(user: &str, pw: &str) -> Vec<u8> {
        let mut w = PktWriter::new(0x200);
        w.wstr(&format!("LOCAL:{user}"), 4096)
            .wstr(pw, 17)
            .wstr("00-11-22-33-44-55", 20)
            .i32(0);
        w.into_payload()
    }

    fn run(st: &mut Core, sid: Sid, id: u16, p: &[u8]) {
        st.scope(|st| dispatch(st, sid, id, p).map(|_| ())).unwrap();
    }

    /// LG-271: (refused with the wrong-password box, the account the login made or found).
    fn mods_login(st: &mut Core, sid: Sid, field: &str) -> (bool, Option<String>) {
        let rx = connect(st, sid, Listener::Auth);
        let mut w = PktWriter::new(0x200);
        w.wstr(field, 4096)
            .wstr("pw", 17)
            .wstr("00-11-22-33-44-55", 20)
            .i32(0);
        run(st, sid, LOGIN_REQUEST, &w.into_payload());
        let got = drain(&rx);
        let r = &got
            .iter()
            .find(|(id, _)| *id == 0x201)
            .expect("a login reply")
            .1;
        let refused = u32_at(r, 6) == 0 && u32_at(r, 0xa) == wire::REASON_BAD_PASSWORD;
        let acc = u32_at(r, 0x12);
        let login = db::account_by_no(&st.db, acc).unwrap().map(|a| a.login);
        (refused, if refused { None } else { login })
    }

    /// LG-317: the bot mark comes off the name and lands on the account only with a login that holds.
    #[test]
    fn the_bot_mark_lands_on_the_account_only_after_a_good_login() {
        let mut st = state();
        let (refused, login) = mods_login(&mut st, 21, "plain");
        assert!(!refused);
        let acc = db::account_by_login(&st.db, login.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert!(crate::lifecycle::is_human(&st, acc.user_no));
        // a wrong password with the mark: refused, and a stranger cannot mark the account
        store_password(&mut st, acc.user_no, "right-password").unwrap();
        st.opts.pw_check = true;
        assert!(mods_login(&mut st, 22, "plain;b=1").0);
        assert!(crate::lifecycle::is_human(&st, acc.user_no));
        // no password check: the login holds, the name is the plain one, the account is a bot from now on
        st.opts.pw_check = false;
        let (refused, login) = mods_login(&mut st, 23, "plain;b=1");
        assert!(!refused);
        assert_eq!(login.as_deref(), Some("plain"));
        assert!(!crate::lifecycle::is_human(&st, acc.user_no));
    }

    fn hashes() -> (String, String) {
        ("1f".repeat(32), "2e".repeat(32))
    }

    #[test]
    fn a_vanilla_lobby_takes_no_suffix_or_the_empty_set_only() {
        let (set, content) = hashes();
        let (set, content) = (set.as_str(), content.as_str());
        use dc_wire::loginmods::{suffix, EMPTY_SET_HASH};
        assert_eq!(EMPTY_SET_HASH, dc_mod_lua::EMPTY_SET_HASH);
        let mut st = state();
        assert_eq!(
            mods_login(&mut st, 1, "LOCAL:alice"),
            (false, Some("alice".into()))
        );
        let f = format!("LOCAL:bob{}", suffix(EMPTY_SET_HASH, "0"));
        assert_eq!(
            mods_login(&mut st, 2, &f),
            (false, Some("bob".into())),
            "the suffix is not part of the name"
        );
        assert_eq!(
            mods_login(&mut st, 3, &format!("bob;m={EMPTY_SET_HASH}")),
            (false, Some("bob".into()))
        );
        assert!(mods_login(&mut st, 4, &format!("LOCAL:carl{}", suffix(set, "0"))).0);
        assert!(
            mods_login(
                &mut st,
                5,
                &format!("LOCAL:carl{}", suffix(EMPTY_SET_HASH, content))
            )
            .0
        );
        assert!(
            db::account_by_login(&st.db, "carl").unwrap().is_none(),
            "a refused login makes no account"
        );
        // a token login with another set is refused before the token is looked at
        assert!(
            mods_login(
                &mut st,
                6,
                &format!("LOCAL:tok:aGk.c2ln{}", suffix(set, "0"))
            )
            .0
        );
    }

    #[test]
    fn a_modded_lobby_takes_its_exact_set_and_content_hashes() {
        let (set, content) = hashes();
        let (set, content) = (set.as_str(), content.as_str());
        use dc_wire::loginmods::{suffix, EMPTY_SET_HASH};
        let mut st = state();
        st.login_mods.set = set.into();
        assert!(
            mods_login(&mut st, 1, "LOCAL:alice").0,
            "no suffix = the empty set"
        );
        assert!(
            mods_login(
                &mut st,
                2,
                &format!("LOCAL:alice{}", suffix(EMPTY_SET_HASH, "0"))
            )
            .0
        );
        assert!(
            mods_login(
                &mut st,
                3,
                &format!("LOCAL:alice{}", suffix(&"3c".repeat(32), "0"))
            )
            .0
        );
        let f = format!("LOCAL:alice{}", suffix(&set.to_ascii_uppercase(), "0"));
        assert_eq!(mods_login(&mut st, 4, &f), (false, Some("alice".into())));
        // with content: `0` and another hash are refused, the exact one passes
        st.login_mods.content = content.into();
        assert!(mods_login(&mut st, 5, &format!("dave{}", suffix(set, "0"))).0);
        assert!(
            mods_login(&mut st, 6, &format!("dave;m={set}")).0,
            "no content field = no content"
        );
        assert!(mods_login(&mut st, 7, &format!("dave{}", suffix(set, set))).0);
        assert_eq!(
            mods_login(&mut st, 8, &format!("dave{}", suffix(set, content))),
            (false, Some("dave".into()))
        );
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_login("LOCAL:dave"), "dave");
        assert_eq!(normalize_login("local:Dave "), "Dave");
        assert_eq!(normalize_login("erin"), "erin");
    }

    #[test]
    fn nickname_flow_and_server_pick() {
        let mut st = state();
        let rx = connect(&mut st, 1, Listener::Auth);
        run(&mut st, 1, FILE_CHECK, &[0u8; 0x26]);
        run(&mut st, 1, LOGIN_REQUEST, &login_payload("alice", "pw"));
        let got = drain(&rx);
        assert_eq!(got[0].0, 0x207);
        assert_eq!(got[1].0, 0x201);
        assert_eq!(
            (u32_at(&got[1].1, 6), u32_at(&got[1].1, 0xa)),
            (0, 0x0003001e)
        );
        let acc = u32_at(&got[1].1, 0x12);
        let n: String = st
            .db
            .query_row(
                "SELECT login FROM accounts WHERE user_no=?1",
                [acc as i64],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, "alice");
        let mut w = PktWriter::new(0x204);
        w.wstr("Alice", 17);
        run(&mut st, 1, NAME_CHECK, &w.into_payload());
        assert_eq!(u32_at(&drain(&rx)[0].1, 6), 1);
        let mut w = PktWriter::new(0x202);
        w.i32(6002).wstr("Alice", 17);
        run(&mut st, 1, NEW_CHAR, &w.into_payload());
        let got = drain(&rx);
        assert_eq!(
            (got[0].0, u32_at(&got[0].1, 6), u32_at(&got[0].1, 0xe)),
            (0x203, 1, acc)
        );
        assert_eq!((got[1].0, got[1].1.len()), (0x2000, 0xf16));
        assert_eq!(u32_at(&got[1].1, 0x16 + 0x88), st.opts.lobbyport as u32);
        let face: i64 = st
            .db
            .query_row("SELECT face FROM characters", [], |r| r.get(0))
            .unwrap();
        assert_eq!(face, 2);
        assert_eq!(db::items(&st.db, acc).unwrap().len(), 5);
        // the name is now taken, case-insensitively
        let mut w = PktWriter::new(0x204);
        w.wstr("ALICE", 17);
        run(&mut st, 1, NAME_CHECK, &w.into_payload());
        assert_eq!(u32_at(&drain(&rx)[0].1, 6), 0);
        run(&mut st, 1, SERVER_PICK, &{
            let mut w = PktWriter::new(0x20a);
            w.u32(1);
            w.into_payload()
        });
        let got = drain(&rx);
        assert_eq!((got[0].0, u32_at(&got[0].1, 6)), (0x20b, 1));
        let mut key = [0u8; 16];
        key.copy_from_slice(&got[0].1[0xe..0x1e]);
        assert_eq!(st.tickets.get(&key).unwrap().account, acc);
    }

    #[test]
    fn check_password_sets_then_enforces() {
        let mut st = state();
        st.opts.pw_check = true;
        st.opts.autochar = true;
        let rx = connect(&mut st, 1, Listener::Auth);
        run(&mut st, 1, LOGIN_REQUEST, &login_payload("dave", "pw"));
        let got = drain(&rx);
        assert_eq!(u32_at(&got[0].1, 6), 1, "autochar: straight to ok");
        assert_eq!(got[1].0, 0x2000);
        run(&mut st, 1, LOGIN_REQUEST, &login_payload("dave", "nope"));
        let got = drain(&rx);
        assert_eq!(
            (u32_at(&got[0].1, 6), u32_at(&got[0].1, 0xa)),
            (0, 0x00030025)
        );
        run(&mut st, 1, LOGIN_REQUEST, &login_payload("dave", "pw"));
        assert_eq!(u32_at(&drain(&rx)[0].1, 6), 1);
    }

    #[test]
    fn argon2_hash_verifies_and_old_rows_upgrade_in_place() {
        let mut st = state();
        st.opts.pw_check = true;
        st.opts.autochar = true;
        // a new password is an Argon2id PHC string without a salt column
        st.scope(|st| {
            ensure_account(st, "neo", "red pill")?;
            Ok(())
        })
        .unwrap();
        let a = db::account_by_login(&st.db, "neo").unwrap().unwrap();
        assert!(a.pw_hash.as_deref().unwrap().starts_with("$argon2id$"));
        assert_eq!(a.pw_salt, None);
        assert_eq!(pw_verify(&a, "red pill"), PwCheck::Ok);
        assert_eq!(pw_verify(&a, "blue pill"), PwCheck::Bad);
        // an old salted SHA-256 row
        let old = db::create_account(&st.db, "trinity", 0).unwrap();
        db::set_password(&st.db, old, &pw_digest("0011aabb", "hunter2"), "0011aabb").unwrap();
        let rx = connect(&mut st, 1, Listener::Auth);
        run(
            &mut st,
            1,
            LOGIN_REQUEST,
            &login_payload("trinity", "wrong"),
        );
        assert_eq!(u32_at(&drain(&rx)[0].1, 0xa), wire::REASON_BAD_PASSWORD);
        let still_old = db::account_by_no(&st.db, old).unwrap().unwrap();
        assert_eq!(
            still_old.pw_salt.as_deref(),
            Some("0011aabb"),
            "a refused login rewrites nothing"
        );
        run(
            &mut st,
            1,
            LOGIN_REQUEST,
            &login_payload("trinity", "hunter2"),
        );
        assert_eq!(u32_at(&drain(&rx)[0].1, 6), 1, "the old row verifies");
        let up = db::account_by_no(&st.db, old).unwrap().unwrap();
        assert_eq!(up.pw_salt, None);
        assert!(up.pw_hash.as_deref().unwrap().starts_with("$argon2id$"));
        assert_eq!(pw_verify(&up, "hunter2"), PwCheck::Ok);
        run(
            &mut st,
            1,
            LOGIN_REQUEST,
            &login_payload("trinity", "hunter2"),
        );
        assert_eq!(u32_at(&drain(&rx)[0].1, 6), 1, "the rewritten row verifies");
        run(
            &mut st,
            1,
            LOGIN_REQUEST,
            &login_payload("trinity", "hunter3"),
        );
        assert_eq!(
            u32_at(&drain(&rx)[0].1, 6),
            0,
            "wrong password still refused"
        );
    }

    #[test]
    fn a_disabled_account_is_refused_even_with_its_password() {
        let mut st = state();
        st.opts.pw_check = true;
        st.opts.autochar = true;
        st.scope(|st| {
            ensure_account(st, "morpheus", "zion1999")?;
            Ok(())
        })
        .unwrap();
        crate::accounts::set_disabled(&st.db, "Morpheus", true).unwrap();
        let rx = connect(&mut st, 1, Listener::Auth);
        run(
            &mut st,
            1,
            LOGIN_REQUEST,
            &login_payload("morpheus", "zion1999"),
        );
        let got = drain(&rx);
        assert_eq!(
            (u32_at(&got[0].1, 6), u32_at(&got[0].1, 0xa)),
            (0, wire::REASON_BAD_PASSWORD)
        );
        assert_eq!(got.len(), 1, "no server list");
        crate::accounts::set_disabled(&st.db, "morpheus", false).unwrap();
        run(
            &mut st,
            1,
            LOGIN_REQUEST,
            &login_payload("morpheus", "zion1999"),
        );
        assert_eq!(u32_at(&drain(&rx)[0].1, 6), 1);
    }

    #[test]
    fn referrer_check() {
        let mut st = state();
        st.opts.autochar = true;
        let rx = connect(&mut st, 1, Listener::Auth);
        run(&mut st, 1, LOGIN_REQUEST, &login_payload("alice", "pw"));
        drain(&rx);
        st.opts.autochar = false;
        let rx2 = connect(&mut st, 2, Listener::Auth);
        run(&mut st, 2, LOGIN_REQUEST, &login_payload("carol", "pw"));
        let carol = u32_at(&drain(&rx2)[0].1, 0x12);
        let mut w = PktWriter::new(0x20e);
        w.wstr("Nobody", 17);
        run(&mut st, 2, REFERRER_CHECK, &w.into_payload());
        let got = drain(&rx2);
        assert_eq!(
            (
                u32_at(&got[0].1, 6),
                u16::from_le_bytes([got[0].1[0xa], got[0].1[0xb]])
            ),
            (0, 0x38)
        );
        let mut w = PktWriter::new(0x20e);
        w.wstr("ALICE", 17);
        run(&mut st, 2, REFERRER_CHECK, &w.into_payload());
        assert_eq!(u32_at(&drain(&rx2)[0].1, 6), 1);
        let r: String = st
            .db
            .query_row(
                "SELECT recommender FROM recommends WHERE user_no=?1",
                [carol as i64],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(r, "alice", "canonical nick of the referred character");
    }
}
