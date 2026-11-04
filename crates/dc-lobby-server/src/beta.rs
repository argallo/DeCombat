//! The beta list: people sign up by e-mail on the website, confirm the address (double opt-in), and the operator picks
//! who gets an invite code, then asks the testers for feedback. This module is the one code path under the public
//! endpoints (`betaweb.rs`), the operator page (`/ops/beta`) and the command line (`betacli.rs`).
//!
//! - **Store:** `beta_signups` (one row per address, case-insensitive) and `beta_mail_log` (one row per mail queued:
//!   the 24-hour budget is counted from it, and it carries the outcome the mail worker reports back).
//! - **Statuses:** `pending` (signed up, not confirmed; dropped after 7 days) -> `confirmed` -> `invited` (an invite
//!   code was mailed) -> `registered` (the code was used, the account number is stored). `unsubscribed` is a
//!   suppression row: such an address is never mailed again, except the one confirmation mail of a fresh signup.
//! - **Mail:** every beta mail carries the unsubscribe link (also as `List-Unsubscribe` with the one-click POST form).
//!   The beta mails of 24 hours are capped (`beta.max_mail_per_day`); a batch over the rest is refused as a whole and
//!   `beta.confirm_reserve` of the cap is kept for confirmation mails.
//! - No address reaches a log line: rows are named by their id (`beta: signup 12 ...`).
// Provenance: ORIGINAL (DeCombat server/web code)
// DC-DEFAULT: LG-370 signup store and endpoint; LG-371 abuse limits and the client address; LG-372 double opt-in,
// unsubscribe; LG-373 mail budget; LG-374 invite and feedback mails; LG-377 invite consumption marks registered;
// LG-379 retention (see DC_DEFAULTS.md)

use crate::accounts;
use crate::logs::Logger;
use crate::mail::{Mail, Mailer};
use crate::reportmail::{one_line, utc_text};
use crate::state::{Core, KeySource};
use dc_config::BetaSection;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

/// The statuses of a signup.
pub const STATUSES: [&str; 5] = [
    "pending",
    "confirmed",
    "invited",
    "registered",
    "unsubscribed",
];
/// The platforms a person may name (anything else is stored as none).
pub const PLATFORMS: [&str; 3] = ["windows", "mac", "ipad"];
/// A pending signup is dropped after this long.
pub const PENDING_SECS: i64 = 7 * 86_400;
/// A pending address gets at most one confirmation mail in this time (a failed one may be repeated at once).
pub const RESEND_SECS: i64 = 86_400;
/// Rows of the beta mail log are kept this long.
pub const LOG_KEEP_SECS: i64 = 30 * 86_400;
/// A mail with no outcome after this long counts as failed.
pub const LOST_SECS: i64 = 900;
/// Token clicks (confirm / unsubscribe) per client address in an hour.
pub const TOKEN_HITS_PER_HOUR: u32 = 60;
/// Addresses the in-memory limiter tracks.
const MAX_TRACKED: usize = 4096;
/// Longest subject of a feedback mail (characters).
pub const SUBJECT_MAX: usize = 120;
/// Longest body of a feedback mail (characters).
pub const BODY_MAX: usize = 6000;
/// Longest note on a signup.
pub const NOTE_MAX: usize = 300;
/// Most rows one operator batch may name.
pub const BATCH_MAX: usize = 500;

pub const DEFAULT_FEEDBACK_SUBJECT: &str = "How is the DeCombat beta going?";
pub const DEFAULT_FEEDBACK_BODY: &str = "Hello {name},\n\n\
thank you for testing DeCombat. We would like to hear how it went for you: what worked, what did not, and what you \
would change.\n\n\
- Just reply to this mail with anything you want to tell us.\n\
- To report a bug while you are in the game, open the Report a Bug form from the menu, or type /report in the chat. \
The report carries the technical details we need (version, what happened), so we can find the cause faster.\n\n\
Every report is read. Thank you for helping.\n";

/// Why a request was not taken.
#[derive(Debug)]
pub enum BetaErr {
    /// The mail sender is off: nothing can be confirmed or sent.
    NotSetUp,
    /// The input is wrong; the text is for the person.
    Bad(String),
    /// A rate limit.
    Slow,
    /// The batch is over the mail budget (the text says by how much).
    Budget(String),
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for BetaErr {
    fn from(e: rusqlite::Error) -> Self {
        BetaErr::Db(e)
    }
}

pub type BetaResult<T> = Result<T, BetaErr>;

/// A sliding one-hour window of event times.
fn hit(q: &mut VecDeque<i64>, unix: i64, cap: u32) -> bool {
    while q.front().is_some_and(|t| *t <= unix - 3600) {
        q.pop_front();
    }
    if q.len() >= cap as usize {
        return false;
    }
    q.push_back(unix);
    true
}

fn hit_keyed(m: &mut HashMap<String, VecDeque<i64>>, key: &str, unix: i64, cap: u32) -> bool {
    if m.len() > MAX_TRACKED && !m.contains_key(key) {
        m.retain(|_, q| q.back().is_some_and(|t| *t > unix - 3600));
        if m.len() > MAX_TRACKED {
            m.clear();
        }
    }
    hit(m.entry(key.to_string()).or_default(), unix, cap)
}

/// What the beta flows keep in memory: the limiter windows, the channel the mail worker answers on and the CSRF token
/// of the operator page.
pub struct Runtime {
    pub tx: Sender<(i64, bool)>,
    rx: Receiver<(i64, bool)>,
    ip_hits: HashMap<String, VecDeque<i64>>,
    all_hits: VecDeque<i64>,
    tok_hits: HashMap<String, VecDeque<i64>>,
    csrf: String,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime::new()
    }
}

impl Runtime {
    pub fn new() -> Runtime {
        let (tx, rx) = channel();
        Runtime {
            tx,
            rx,
            ip_hits: HashMap::new(),
            all_hits: VecDeque::new(),
            tok_hits: HashMap::new(),
            csrf: String::new(),
        }
    }

    /// The per-process token the operator page carries (made on first use).
    // DC-DEFAULT: LG-375 one random token per lobby process
    pub fn csrf(&mut self, keys: &mut KeySource) -> String {
        if self.csrf.is_empty() {
            self.csrf = new_token(keys) + &new_token(keys);
        }
        self.csrf.clone()
    }
}

/// Everything one beta operation touches, borrowed from the lobby's state (or from the command line's own parts).
pub struct Ctx<'a> {
    pub db: &'a Connection,
    pub keys: &'a mut KeySource,
    pub mail: &'a mut Mailer,
    pub cfg: &'a BetaSection,
    pub rt: &'a mut Runtime,
    pub log: &'a Logger,
    pub unix: i64,
}

/// The context of the lobby's state at unix time `unix`.
pub fn ctx(st: &mut Core, unix: i64) -> Ctx<'_> {
    Ctx {
        db: &st.db,
        keys: &mut st.keys,
        mail: &mut st.mail,
        cfg: &st.opts.beta,
        rt: &mut st.beta,
        log: &st.log,
        unix,
    }
}

/// 128 random bits as 32 hex digits.
pub fn new_token(keys: &mut KeySource) -> String {
    keys.key16().iter().map(|b| format!("{b:02x}")).collect()
}

/// True for a well-formed token (32 hex digits); anything else cannot be a row's token.
pub fn token_shape(t: &str) -> bool {
    t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

// ------------------------------------------------------------------------------------------------- rows

/// One row of `beta_signups`.
#[derive(Clone, Debug)]
pub struct Signup {
    pub id: i64,
    pub email: String,
    pub platform: String,
    pub status: String,
    pub invite_code: String,
    pub user_no: i64,
    pub confirm_token: String,
    pub unsub_token: String,
    pub confirm_sent: i64,
}

const COLS: &str =
    "id, email, platform, status, invite_code, user_no, confirm_token, unsub_token, confirm_sent";

fn signup_of(r: &rusqlite::Row) -> rusqlite::Result<Signup> {
    Ok(Signup {
        id: r.get(0)?,
        email: r.get(1)?,
        platform: r.get(2)?,
        status: r.get(3)?,
        invite_code: r.get(4)?,
        user_no: r.get(5)?,
        confirm_token: r.get(6)?,
        unsub_token: r.get(7)?,
        confirm_sent: r.get(8)?,
    })
}

fn by_email(db: &Connection, email: &str) -> rusqlite::Result<Option<Signup>> {
    db.query_row(
        &format!("SELECT {COLS} FROM beta_signups WHERE email = ?1"),
        [email],
        signup_of,
    )
    .optional()
}

fn by_id(db: &Connection, id: i64) -> rusqlite::Result<Option<Signup>> {
    db.query_row(
        &format!("SELECT {COLS} FROM beta_signups WHERE id = ?1"),
        [id],
        signup_of,
    )
    .optional()
}

/// The state (0 queued, 1 sent, 2 failed) of the newest mail of `kind` to a signup.
fn last_state(db: &Connection, id: i64, kind: &str) -> rusqlite::Result<Option<i64>> {
    db.query_row(
        "SELECT state FROM beta_mail_log WHERE signup_id = ?1 AND kind = ?2 ORDER BY id DESC LIMIT 1",
        params![id, kind],
        |r| r.get(0),
    )
    .optional()
}

// ------------------------------------------------------------------------------------------------- budget

/// The beta mail budget of the last 24 hours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub max: i64,
    pub used: i64,
    pub reserve: i64,
}

impl Budget {
    /// Mails still allowed (confirmation mails may use all of it).
    pub fn remaining(&self) -> i64 {
        (self.max - self.used).max(0)
    }
    /// Mails an operator batch may still use (the confirmation reserve stays free).
    pub fn admin_remaining(&self) -> i64 {
        (self.max - self.reserve - self.used).max(0)
    }
    pub fn json(&self) -> Value {
        json!({"max": self.max, "used": self.used, "remaining": self.remaining(),
               "reserve": self.reserve, "admin_remaining": self.admin_remaining()})
    }
}

/// The budget at `c.unix`: every beta mail queued in the last 24 hours counts, sent or not.
pub fn budget(c: &Ctx) -> rusqlite::Result<Budget> {
    let used: i64 = c.db.query_row(
        "SELECT COUNT(*) FROM beta_mail_log WHERE at > ?1",
        [c.unix - 86_400],
        |r| r.get(0),
    )?;
    Ok(Budget {
        max: i64::from(c.cfg.max_mail_per_day),
        used,
        reserve: i64::from(c.cfg.confirm_reserve),
    })
}

/// Applies the mail worker's answers waiting on the channel and fails the mails that never got one.
pub fn drain(db: &Connection, rx: &Receiver<(i64, bool)>, unix: i64) {
    while let Ok((tag, ok)) = rx.try_recv() {
        apply_ack(db, tag, ok);
    }
    let _ = db.execute(
        "UPDATE beta_mail_log SET state = 2 WHERE state = 0 AND at < ?1",
        [unix - LOST_SECS],
    );
}

/// Records the outcome of mail-log row `tag`.
pub fn apply_ack(db: &Connection, tag: i64, ok: bool) {
    let _ = db.execute(
        "UPDATE beta_mail_log SET state = ?2 WHERE id = ?1 AND state = 0",
        params![tag, if ok { 1 } else { 2 }],
    );
}

impl Runtime {
    /// [`drain`] on this runtime's channel.
    pub fn drain(&self, db: &Connection, unix: i64) {
        drain(db, &self.rx, unix);
    }

    /// Waits up to `timeout` for the next answer of the mail worker and records it (the command line waits for its mails).
    pub fn wait_ack(&self, db: &Connection, timeout: Duration) -> bool {
        match self.rx.recv_timeout(timeout) {
            Ok((tag, ok)) => {
                apply_ack(db, tag, ok);
                true
            }
            Err(_) => false,
        }
    }
}

/// Drops pending signups older than 7 days and mail-log rows older than 30 days.
pub fn purge(db: &Connection, unix: i64) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM beta_signups WHERE status = 'pending' AND created < ?1",
        [unix - PENDING_SECS],
    )?;
    db.execute(
        "DELETE FROM beta_mail_log WHERE at < ?1",
        [unix - LOG_KEEP_SECS],
    )?;
    Ok(())
}

// ------------------------------------------------------------------------------------------------- mail

fn base(c: &Ctx) -> String {
    c.cfg.public_url.trim().trim_end_matches('/').to_string()
}

fn unsub_url(c: &Ctx, s: &Signup) -> String {
    format!("{}/beta/unsubscribe?t={}", base(c), s.unsub_token)
}

/// One beta mail: the subject with the configured prefix, the body with the unsubscribe footer, the headers.
fn compose(c: &Ctx, s: &Signup, subject: &str, body: &str, what: &str) -> Mail {
    let unsub = unsub_url(c, s);
    let subject: String = one_line(subject).chars().take(SUBJECT_MAX).collect();
    Mail {
        to: s.email.clone(),
        subject: c.mail.opts.subject(subject.trim()),
        body: format!(
            "{}\n\n--\nYou get this mail because this address signed up for the DeCombat beta.\n\
             Unsubscribe with one click: {unsub}\n",
            body.trim_end()
        ),
        what: what.to_string(),
        reply_to: c.cfg.reply_to.trim().to_string(),
        headers: vec![
            ("List-Unsubscribe".into(), format!("<{unsub}>")),
            (
                "List-Unsubscribe-Post".into(),
                "List-Unsubscribe=One-Click".into(),
            ),
        ],
    }
}

/// Logs the mail and hands it to the sender; false (nothing queued, nothing counted) when mail is off.
fn queue(c: &mut Ctx, id: i64, kind: &str, mail: Mail) -> rusqlite::Result<bool> {
    c.db.execute(
        "INSERT INTO beta_mail_log (signup_id, kind, at, state) VALUES (?1, ?2, ?3, 0)",
        params![id, kind, c.unix],
    )?;
    let log_id = c.db.last_insert_rowid();
    if c.mail.send_with(mail, Some((log_id, c.rt.tx.clone()))) {
        Ok(true)
    } else {
        c.db.execute("DELETE FROM beta_mail_log WHERE id = ?1", [log_id])?;
        Ok(false)
    }
}

fn confirm_mail(c: &Ctx, s: &Signup) -> Mail {
    let body = format!(
        "Hello,\n\n\
         someone (hopefully you) asked to join the DeCombat beta with this address.\n\n\
         To finish signing up, open this link:\n{}/beta/confirm?t={}\n\n\
         If you did not ask for this, ignore this mail: nothing happens without the click, and the signup is deleted \
         after 7 days. We will invite people in small groups, so it can take a while.\n\n\
         Meanwhile you can find the community on Discord: {DISCORD_URL}",
        base(c),
        s.confirm_token
    );
    compose(
        c,
        s,
        "please confirm your beta signup",
        &body,
        &format!("beta confirmation for signup {}", s.id),
    )
}

/// Where the community talks (Discord), shown in the beta mails.
pub const DISCORD_URL: &str = "https://discord.gg/hNWcXpTST";
/// The release page of the project, shown next to the download address in the invite mail.
pub const RELEASES_URL: &str = "https://github.com/argallo/DeCombat/releases";

/// The text of an invite mail.
// DC-DEFAULT: LG-374 (first public beta wording, 2026-11-03)
fn invite_mail(c: &Ctx, s: &Signup, code: &str) -> Mail {
    let until = utc_text(c.unix + i64::from(c.cfg.invite_days) * 86_400);
    let download = c.cfg.download_url.trim();
    let download = if download.is_empty() {
        "https://decombat.com"
    } else {
        download
    };
    let body = format!(
        "Hello,\n\n\
         you signed up for the DeCombat beta and it is your turn. Welcome!\n\n\
         Your invite code: {code}\n\
         It works once, for one account, until {until} UTC.\n\n\
         1. Download\n\
         DeCombat for Windows and macOS: {download}\n\
         All releases: {RELEASES_URL}\n\
         (There is no iPad build yet; it comes later.)\n\n\
         2. Your game files\n\
         DeCombat runs on your own copy of the original Combat Arms EU client files (2009). We don't provide them \
         or link to them. Point DeCombat at the folder of your existing install.\n\n\
         3. Register\n\
         Start DeCombat, open Sign up on the login screen and enter the code there. You can also register in a \
         browser: {}/register\n\n\
         First start on Windows\n\
         The program is not code-signed yet, so SmartScreen may say \"Windows protected your PC\": click More info, \
         then Run anyway. Windows Defender may flag it as Trojan:Win32/Wacatac.B!ml. That is a known false positive \
         for new unsigned programs (it happens to other open-source game projects too), and we have submitted the \
         build to Microsoft. To check your download, compare its hash with the SHA256SUMS file of the release: in \
         PowerShell run Get-FileHash .\\file.zip\n\n\
         First start on macOS\n\
         The app is not notarised. If macOS refuses to open it, go to System Settings > Privacy & Security, scroll \
         down, click Open Anyway and start it again. To check your download: shasum -a 256 file.zip\n\n\
         Feedback\n\
         - Discord: {DISCORD_URL}\n\
         - Reply to this mail.\n\
         - In the game: the Report a Bug form in the menu, or type /report in the chat.\n\n\
         This is a beta: accounts and progress may be reset before the full launch. Thank you for testing.",
        base(c),
        code = accounts::show_code(code),
    );
    compose(
        c,
        s,
        "your beta invite",
        &body,
        &format!("beta invite for signup {}", s.id),
    )
}

/// `template` with `{name}` and `{email}` replaced in one pass (a value is never scanned for placeholders again).
pub fn fill(template: &str, name: &str, email: &str) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("{name}") {
            out.push_str(name);
            rest = r;
        } else if let Some(r) = rest.strip_prefix("{email}") {
            out.push_str(email);
            rest = r;
        } else {
            out.push('{');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// A feedback template from a file: `Subject: ...`, a blank line, the body.
pub fn parse_template(text: &str) -> Result<(String, String), String> {
    let text = text.replace("\r\n", "\n");
    let Some((head, body)) = text.split_once("\n\n") else {
        return Err(
            "the template needs a first line `Subject: ...`, a blank line and the body".into(),
        );
    };
    let Some(subject) = head.trim().strip_prefix("Subject:") else {
        return Err("the template's first line must be `Subject: ...`".into());
    };
    Ok((subject.trim().to_string(), body.to_string()))
}

/// The default template in the file form [`parse_template`] reads.
pub fn default_template() -> String {
    format!("Subject: {DEFAULT_FEEDBACK_SUBJECT}\n\n{DEFAULT_FEEDBACK_BODY}")
}

// ------------------------------------------------------------------------------------------------- public side

/// A signup request from `ip` (the client address as the limits count it). The caller gives the same answer for
/// every `Ok`: a new, a pending, a confirmed and an unsubscribed address look alike.
pub fn signup(c: &mut Ctx, email: &str, platform: &str, consent: bool, ip: &str) -> BetaResult<()> {
    if !c.mail.enabled() {
        return Err(BetaErr::NotSetUp);
    }
    // the limits count every request, valid or not: a flood of bad forms costs the same as a flood of good ones
    if !hit_keyed(&mut c.rt.ip_hits, ip, c.unix, c.cfg.ip_per_hour)
        || !hit(&mut c.rt.all_hits, c.unix, c.cfg.global_per_hour)
    {
        c.log.line("beta: signup refused (rate limit)");
        return Err(BetaErr::Slow);
    }
    let email = email.trim();
    if email.is_empty() {
        return Err(BetaErr::Bad("Enter your e-mail address.".into()));
    }
    let email = accounts::valid_email(email)
        .map_err(|_| BetaErr::Bad("That is not a valid e-mail address.".into()))?;
    if !consent {
        return Err(BetaErr::Bad(
            "Tick the box to agree that we store your address to send beta invites and updates."
                .into(),
        ));
    }
    let platform = platform.trim().to_ascii_lowercase();
    let platform = if PLATFORMS.contains(&platform.as_str()) {
        platform
    } else {
        String::new()
    };
    purge(c.db, c.unix)?;
    match by_email(c.db, &email)? {
        None => {
            let (ct, ut) = (new_token(c.keys), new_token(c.keys));
            c.db.execute(
                "INSERT INTO beta_signups (email, platform, status, created, confirm_token, unsub_token, ip) \
                 VALUES (?1, ?2, 'pending', ?3, ?4, ?5, ?6)",
                params![email, platform, c.unix, ct, ut, ip],
            )?;
            let s = by_id(c.db, c.db.last_insert_rowid())?.expect("inserted");
            c.log
                .line(&format!("beta: signup {} created (pending)", s.id));
            send_confirmation(c, &s)?;
        }
        Some(s) if s.status == "pending" || s.status == "unsubscribed" => {
            let again = s.confirm_sent == 0
                || c.unix - s.confirm_sent >= RESEND_SECS
                || last_state(c.db, s.id, "confirm")? == Some(2);
            if !platform.is_empty() && s.platform != platform {
                c.db.execute(
                    "UPDATE beta_signups SET platform = ?2 WHERE id = ?1",
                    params![s.id, platform],
                )?;
            }
            if again {
                send_confirmation(c, &s)?;
            }
        }
        Some(_) => {}
    }
    Ok(())
}

/// The confirmation mail of a pending (or returning) signup, when the budget allows one.
fn send_confirmation(c: &mut Ctx, s: &Signup) -> rusqlite::Result<()> {
    if budget(c)?.remaining() <= 0 {
        c.log.line(&format!(
            "beta: signup {}: mail budget used up, no confirmation sent",
            s.id
        ));
        return Ok(());
    }
    let mail = confirm_mail(c, s);
    if queue(c, s.id, "confirm", mail)? {
        c.db.execute(
            "UPDATE beta_signups SET confirm_sent = ?2 WHERE id = ?1",
            params![s.id, c.unix],
        )?;
    }
    Ok(())
}

/// What a confirmation link did.
#[derive(Debug, PartialEq, Eq)]
pub enum Confirmed {
    /// The address is on the list now (or was before).
    Yes,
    /// No such link (wrong, or the pending signup expired).
    No,
}

fn token_gate(c: &mut Ctx, ip: &str) -> BetaResult<()> {
    if hit_keyed(&mut c.rt.tok_hits, ip, c.unix, TOKEN_HITS_PER_HOUR) {
        Ok(())
    } else {
        Err(BetaErr::Slow)
    }
}

/// The confirmation link: pending (or unsubscribed, signing up again) becomes confirmed; later statuses stay.
pub fn confirm(c: &mut Ctx, token: &str, ip: &str) -> BetaResult<Confirmed> {
    token_gate(c, ip)?;
    purge(c.db, c.unix)?;
    if !token_shape(token) {
        return Ok(Confirmed::No);
    }
    let row: Option<(i64, String)> =
        c.db.query_row(
            "SELECT id, status FROM beta_signups WHERE confirm_token = ?1",
            [token],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((id, status)) = row else {
        return Ok(Confirmed::No);
    };
    if status == "pending" || status == "unsubscribed" {
        c.db.execute(
            "UPDATE beta_signups SET status = 'confirmed', confirmed = ?2 WHERE id = ?1",
            params![id, c.unix],
        )?;
        c.log.line(&format!("beta: signup {id} confirmed"));
    }
    Ok(Confirmed::Yes)
}

/// The unsubscribe link (also the one-click POST): the row becomes `unsubscribed`; true when the token is known.
pub fn unsubscribe(c: &mut Ctx, token: &str, ip: &str) -> BetaResult<bool> {
    token_gate(c, ip)?;
    if !token_shape(token) {
        return Ok(false);
    }
    let n = c.db.execute(
        "UPDATE beta_signups SET status = 'unsubscribed', ip = '' WHERE unsub_token = ?1 AND status != 'unsubscribed'",
        [token],
    )?;
    let known: i64 = c.db.query_row(
        "SELECT COUNT(*) FROM beta_signups WHERE unsub_token = ?1",
        [token],
        |r| r.get(0),
    )?;
    if n > 0 {
        c.log.line("beta: a signup unsubscribed");
    }
    Ok(known > 0)
}

/// LG-377: the invite code `code` was used by `user_no`: the signup it was mailed to is registered. No row = no
/// effect. An unsubscribed address keeps its status (it stays off the mailing list) but gets the account number.
pub fn code_consumed(db: &Connection, code: &str, user_no: u32) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE beta_signups SET user_no = ?2, \
         status = CASE WHEN status IN ('invited', 'confirmed') THEN 'registered' ELSE status END \
         WHERE invite_code = ?1 AND invite_code != ''",
        params![code, i64::from(user_no)],
    )?;
    Ok(())
}

// ------------------------------------------------------------------------------------------------- operator side

/// The filters of the signup list.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub status: Option<String>,
    pub platform: Option<String>,
    /// Signed up before this unix time.
    pub before: Option<i64>,
    /// Signed up at or after this unix time.
    pub after: Option<i64>,
    /// A part of the address.
    pub q: Option<String>,
    pub limit: Option<usize>,
}

/// One line of the list: the signup plus the account it became and the newest mail.
pub fn list(c: &Ctx, f: &Filter) -> rusqlite::Result<Vec<Value>> {
    let mut sql = String::from(
        "SELECT s.id, s.email, s.platform, s.status, s.created, s.confirmed, s.invited, s.user_no, s.note, \
         a.login, a.last_login, ch.matches, \
         (SELECT kind FROM beta_mail_log m WHERE m.signup_id = s.id ORDER BY m.id DESC LIMIT 1), \
         (SELECT state FROM beta_mail_log m WHERE m.signup_id = s.id ORDER BY m.id DESC LIMIT 1), \
         (SELECT at FROM beta_mail_log m WHERE m.signup_id = s.id ORDER BY m.id DESC LIMIT 1) \
         FROM beta_signups s LEFT JOIN accounts a ON a.user_no = s.user_no AND s.user_no != 0 \
         LEFT JOIN characters ch ON ch.user_no = s.user_no AND s.user_no != 0 WHERE 1 = 1",
    );
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    let mut add = |sql: &mut String, cond: &str, v: rusqlite::types::Value| {
        args.push(v);
        sql.push_str(&format!(
            " AND {}",
            cond.replace('?', &format!("?{}", args.len()))
        ));
    };
    if let Some(s) = f.status.as_deref().filter(|s| !s.is_empty()) {
        add(&mut sql, "s.status = ?", s.to_string().into());
    }
    if let Some(p) = f.platform.as_deref().filter(|s| !s.is_empty()) {
        add(&mut sql, "s.platform = ?", p.to_string().into());
    }
    if let Some(t) = f.before {
        add(&mut sql, "s.created < ?", t.into());
    }
    if let Some(t) = f.after {
        add(&mut sql, "s.created >= ?", t.into());
    }
    if let Some(q) = f.q.as_deref().filter(|s| !s.trim().is_empty()) {
        let esc = q
            .trim()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        add(
            &mut sql,
            "s.email LIKE ? ESCAPE '\\'",
            format!("%{esc}%").into(),
        );
    }
    let limit = f.limit.unwrap_or(500).clamp(1, 2000);
    sql.push_str(&format!(" ORDER BY s.created, s.id LIMIT {limit}"));
    let mut q = c.db.prepare(&sql)?;
    let rows = q.query_map(rusqlite::params_from_iter(args), |r| {
        let user_no: i64 = r.get(7)?;
        let login: Option<String> = r.get(9)?;
        let account = match login {
            Some(l) if user_no != 0 => json!({"user_no": user_no, "login": l,
                "last_login": r.get::<_, Option<i64>>(10)?, "matches": r.get::<_, Option<i64>>(11)?}),
            _ => Value::Null,
        };
        let kind: Option<String> = r.get(12)?;
        let mail = match kind {
            Some(k) => json!({"kind": k, "state": r.get::<_, i64>(13)?, "at": r.get::<_, i64>(14)?}),
            None => Value::Null,
        };
        Ok(json!({"id": r.get::<_, i64>(0)?, "email": r.get::<_, String>(1)?, "platform": r.get::<_, String>(2)?,
                  "status": r.get::<_, String>(3)?, "created": r.get::<_, i64>(4)?,
                  "confirmed": r.get::<_, i64>(5)?, "invited": r.get::<_, i64>(6)?,
                  "note": r.get::<_, String>(8)?, "account": account, "mail": mail}))
    })?;
    rows.collect()
}

/// `YYYY-MM-DD` (UTC midnight) or a plain unix time as seconds; None for anything else.
pub fn parse_day(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    let mut it = s.split('-');
    let (y, m, d) = (
        it.next()?.parse::<i64>().ok()?,
        it.next()?.parse::<i64>().ok()?,
        it.next()?.parse::<i64>().ok()?,
    );
    if it.next().is_some()
        || !(1..=12).contains(&m)
        || !(1..=31).contains(&d)
        || !(1970..=9999).contains(&y)
    {
        return None;
    }
    // days from a civil date (the public algorithm of Howard Hinnant's date notes)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400)
}

/// Rows per status.
pub fn counts(c: &Ctx) -> rusqlite::Result<Value> {
    let mut m = serde_json::Map::new();
    for s in STATUSES {
        m.insert(s.to_string(), json!(0));
    }
    let mut q =
        c.db.prepare("SELECT status, COUNT(*) FROM beta_signups GROUP BY status")?;
    for r in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (s, n) = r?;
        m.insert(s, json!(n));
    }
    Ok(Value::Object(m))
}

/// Who a batch names.
#[derive(Clone, Debug)]
pub enum Pick {
    Ids(Vec<i64>),
    Emails(Vec<String>),
    /// The first N confirmed addresses, the longest waiting first.
    Next(usize),
    /// Every row with this status.
    Status(String),
}

/// What happened to one row of a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowResult {
    pub id: i64,
    pub email: String,
    pub ok: bool,
    pub message: String,
}

impl RowResult {
    pub fn json(&self) -> Value {
        json!({"id": self.id, "ok": self.ok, "message": self.message})
    }
}

enum Plan {
    Send(Box<Signup>),
    Skip(RowResult),
}

fn skip(id: i64, email: &str, msg: impl Into<String>) -> Plan {
    Plan::Skip(RowResult {
        id,
        email: email.to_string(),
        ok: false,
        message: msg.into(),
    })
}

/// The rows a pick names (missing ones become skips).
fn resolve(c: &Ctx, pick: &Pick) -> rusqlite::Result<Vec<Plan>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |out: &mut Vec<Plan>, s: Signup| {
        if seen.insert(s.id) {
            out.push(Plan::Send(Box::new(s)));
        }
    };
    match pick {
        Pick::Ids(ids) => {
            for id in ids.iter().take(BATCH_MAX) {
                match by_id(c.db, *id)? {
                    Some(s) => push(&mut out, s),
                    None => out.push(skip(*id, "", "no such signup")),
                }
            }
        }
        Pick::Emails(emails) => {
            for e in emails.iter().take(BATCH_MAX) {
                match by_email(c.db, e.trim())? {
                    Some(s) => push(&mut out, s),
                    None => out.push(skip(0, e.trim(), "not on the list")),
                }
            }
        }
        Pick::Next(n) => {
            let mut q = c.db.prepare(&format!(
                "SELECT {COLS} FROM beta_signups WHERE status = 'confirmed' ORDER BY confirmed, id LIMIT ?1"
            ))?;
            for s in q.query_map([(*n).min(BATCH_MAX) as i64], signup_of)? {
                push(&mut out, s?);
            }
        }
        Pick::Status(st) => {
            let mut q = c.db.prepare(&format!(
                "SELECT {COLS} FROM beta_signups WHERE status = ?1 ORDER BY created, id LIMIT ?2"
            ))?;
            for s in q.query_map(params![st, BATCH_MAX as i64], signup_of)? {
                push(&mut out, s?);
            }
        }
    }
    Ok(out)
}

/// Refuses the whole batch when it would pass the operator budget; else Ok.
fn check_budget(c: &Ctx, n: usize) -> BetaResult<()> {
    if n == 0 {
        return Ok(());
    }
    let b = budget(c)?;
    if n as i64 > b.admin_remaining() {
        return Err(BetaErr::Budget(format!(
            "{n} mails are over the budget: {} may still go out (limit {} in 24 hours, {} of them kept for confirmation mails, {} used)",
            b.admin_remaining(),
            b.max,
            b.reserve,
            b.used
        )));
    }
    Ok(())
}

fn need_mail(c: &Ctx) -> BetaResult<()> {
    if c.mail.enabled() {
        Ok(())
    } else {
        Err(BetaErr::NotSetUp)
    }
}

/// Makes and mails one invite code to each named confirmed row (or invited row whose invite mail failed, which gets
/// its still open code again). `dry` changes and sends nothing. The whole batch is refused when it is over the budget.
pub fn invite(c: &mut Ctx, pick: &Pick, dry: bool) -> BetaResult<Vec<RowResult>> {
    need_mail(c)?;
    drain(c.db, &c.rt.rx, c.unix);
    let mut plans = resolve(c, pick)?;
    for p in plans.iter_mut() {
        if let Plan::Send(s) = p {
            let ok = s.status == "confirmed"
                || (s.status == "invited" && last_state(c.db, s.id, "invite")? == Some(2));
            if !ok {
                *p = skip(
                    s.id,
                    &s.email,
                    format!(
                        "skipped: status is {}, only confirmed addresses get an invite",
                        s.status
                    ),
                );
            }
        }
    }
    check_budget(
        c,
        plans.iter().filter(|p| matches!(p, Plan::Send(_))).count(),
    )?;
    let mut out = Vec::new();
    for p in plans {
        let s = match p {
            Plan::Skip(r) => {
                out.push(r);
                continue;
            }
            Plan::Send(s) => s,
        };
        if dry {
            out.push(RowResult {
                id: s.id,
                email: s.email.clone(),
                ok: true,
                message: "would send an invite".into(),
            });
            continue;
        }
        let reuse = s.status == "invited"
            && !s.invite_code.is_empty()
            && accounts::invite(c.db, &s.invite_code, c.unix)? == accounts::Invite::Open(None);
        let code = if reuse {
            s.invite_code.clone()
        } else {
            accounts::new_invite(c.db, c.keys, None, c.unix, i64::from(c.cfg.invite_days))?
        };
        let mail = invite_mail(c, &s, &code);
        if queue(c, s.id, "invite", mail)? {
            c.db.execute(
                "UPDATE beta_signups SET status = 'invited', invited = ?2, invite_code = ?3 WHERE id = ?1",
                params![s.id, c.unix, code],
            )?;
            c.log
                .line(&format!("beta: invite queued for signup {}", s.id));
            out.push(RowResult {
                id: s.id,
                email: s.email.clone(),
                ok: true,
                message: "queued".into(),
            });
        } else {
            out.push(RowResult {
                id: s.id,
                email: s.email.clone(),
                ok: false,
                message: "the mail sender is off".into(),
            });
        }
    }
    Ok(out)
}

/// Deletes the named signups (LG-381). Per row: the `beta_signups` row goes; an invite code of the row that is still
/// unused goes from `invites` too (a used code stays, and the account it made is NOT deleted); the row's mail-log
/// entries lose their link to it (`signup_id` becomes 0) but stay, because they are the mail budget's count (LG-373)
/// and age out after 30 days. Only the row id is logged. `dry` reports what would happen and changes nothing.
pub fn delete(c: &Ctx, pick: &Pick, dry: bool) -> BetaResult<Vec<RowResult>> {
    let mut out = Vec::new();
    for p in resolve(c, pick)? {
        let s = match p {
            Plan::Skip(r) => {
                out.push(r);
                continue;
            }
            Plan::Send(s) => s,
        };
        // what the row's invite code is: unused (deleted with the row), used (kept), or none
        let code = accounts::normalize_code(&s.invite_code);
        let used = if code.is_empty() {
            None
        } else {
            c.db.query_row(
                "SELECT used_by FROM invites WHERE code = ?1",
                [&code],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .map(|u| u != 0)
        };
        let mut message = String::from(if dry {
            "would delete the signup"
        } else {
            "deleted"
        });
        match used {
            Some(false) => message.push_str(if dry {
                "; its unused invite code would be deleted too"
            } else {
                "; its unused invite code was deleted too"
            }),
            Some(true) => message.push_str(&format!(
                "; its invite code was already used: the code and account {} are left alone (the account is not deleted)",
                s.user_no
            )),
            None => {}
        }
        if s.status == "registered" && used != Some(true) && s.user_no != 0 {
            message.push_str(&format!("; account {} is left alone", s.user_no));
        }
        if !dry {
            // a savepoint nests inside the lobby's own per-request transaction and also works alone (the command line)
            c.db.execute_batch("SAVEPOINT beta_delete")?;
            let done = (|| -> rusqlite::Result<()> {
                if used == Some(false) {
                    c.db.execute(
                        "DELETE FROM invites WHERE code = ?1 AND used_by = 0",
                        [&code],
                    )?;
                }
                c.db.execute(
                    "UPDATE beta_mail_log SET signup_id = 0 WHERE signup_id = ?1",
                    [s.id],
                )?;
                c.db.execute("DELETE FROM beta_signups WHERE id = ?1", [s.id])?;
                Ok(())
            })();
            if let Err(e) = done {
                let _ =
                    c.db.execute_batch("ROLLBACK TO beta_delete; RELEASE beta_delete");
                return Err(e.into());
            }
            c.db.execute_batch("RELEASE beta_delete")?;
            c.log.line(&format!("beta: signup {} deleted", s.id));
        }
        out.push(RowResult {
            id: s.id,
            email: s.email.clone(),
            ok: true,
            message,
        });
    }
    Ok(out)
}

/// Mails `subject` / `body` (with `{name}` and `{email}`) to each named invited or registered row. `dry` sends nothing.
pub fn feedback(
    c: &mut Ctx,
    pick: &Pick,
    subject: &str,
    body: &str,
    dry: bool,
) -> BetaResult<Vec<RowResult>> {
    need_mail(c)?;
    drain(c.db, &c.rt.rx, c.unix);
    if subject.trim().is_empty() || body.trim().is_empty() {
        return Err(BetaErr::Bad("a subject and a text are needed".into()));
    }
    if one_line(subject).chars().count() > SUBJECT_MAX {
        return Err(BetaErr::Bad(format!(
            "the subject is longer than {SUBJECT_MAX} characters"
        )));
    }
    if body.chars().count() > BODY_MAX {
        return Err(BetaErr::Bad(format!(
            "the text is longer than {BODY_MAX} characters"
        )));
    }
    let mut plans = resolve(c, pick)?;
    for p in plans.iter_mut() {
        if let Plan::Send(s) = p {
            if s.status != "invited" && s.status != "registered" {
                *p = skip(
                    s.id,
                    &s.email,
                    format!(
                        "skipped: status is {}, only invited or registered testers get this mail",
                        s.status
                    ),
                );
            }
        }
    }
    check_budget(
        c,
        plans.iter().filter(|p| matches!(p, Plan::Send(_))).count(),
    )?;
    let mut out = Vec::new();
    for p in plans {
        let s = match p {
            Plan::Skip(r) => {
                out.push(r);
                continue;
            }
            Plan::Send(s) => s,
        };
        if dry {
            out.push(RowResult {
                id: s.id,
                email: s.email.clone(),
                ok: true,
                message: "would send".into(),
            });
            continue;
        }
        let login: Option<String> = if s.user_no != 0 {
            c.db.query_row(
                "SELECT login FROM accounts WHERE user_no = ?1",
                [s.user_no],
                |r| r.get(0),
            )
            .optional()?
        } else {
            None
        };
        let name = login.unwrap_or_else(|| s.email.split('@').next().unwrap_or("").to_string());
        let name = one_line(&name);
        let mail = compose(
            c,
            &s,
            &fill(subject, &name, &s.email),
            &fill(body, &name, &s.email),
            &format!("beta feedback request for signup {}", s.id),
        );
        if queue(c, s.id, "feedback", mail)? {
            out.push(RowResult {
                id: s.id,
                email: s.email.clone(),
                ok: true,
                message: "queued".into(),
            });
        } else {
            out.push(RowResult {
                id: s.id,
                email: s.email.clone(),
                ok: false,
                message: "the mail sender is off".into(),
            });
        }
    }
    c.log.line(&format!(
        "beta: feedback mail queued for {} signups",
        out.iter().filter(|r| r.ok).count()
    ));
    Ok(out)
}

/// Sets the note of a signup.
pub fn set_note(c: &Ctx, id: i64, note: &str) -> BetaResult<()> {
    let note: String = one_line(note).chars().take(NOTE_MAX).collect();
    let n = c.db.execute(
        "UPDATE beta_signups SET note = ?2 WHERE id = ?1",
        params![id, note],
    )?;
    if n == 0 {
        return Err(BetaErr::Bad("no such signup".into()));
    }
    Ok(())
}

/// The newest mail-log state per id (the command line prints the outcome after waiting).
pub fn mail_states(db: &Connection, ids: &[i64]) -> rusqlite::Result<HashMap<i64, (String, i64)>> {
    let mut m = HashMap::new();
    for id in ids {
        let r: Option<(String, i64)> = db
            .query_row(
                "SELECT kind, state FROM beta_mail_log WHERE signup_id = ?1 ORDER BY id DESC LIMIT 1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some(r) = r {
            m.insert(*id, r);
        }
    }
    Ok(m)
}

#[cfg(test)]
#[path = "beta_tests.rs"]
pub(crate) mod tests;
