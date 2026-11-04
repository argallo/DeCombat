//! Password and login-ID recovery on the lobby's web port (LG-241..LG-243), next to the registration page:
//!
//! - `GET /recover`: whether recovery is set up (`{"mail":bool}`) or the HTML page with both forms;
//! - `POST /recover/password` (`name`, `email`): mails a one-time code when the name and the address match;
//! - `POST /recover/password/confirm` (`name`, `code`, `pw`, `pw2`): sets the new password with that code;
//! - `POST /recover/id` (`email`): mails the login names registered with the address.
//!
//! The answers never tell whether an account or an address exists. Codes are stored as SHA-256 only.
// DC-DEFAULT: LG-241 no account enumeration, the registration page's per-address throttle; LG-242 the code rules;
// LG-243 the mails; LG-244 addresses masked in the log

use crate::accounts;
use crate::clanweb::{esc, parse_query};
use crate::mail::Mail;
use crate::register::{json_str, reg_page};
use crate::state::Core;
use rusqlite::{params, OptionalExtension};
use std::time::Instant;

/// Code length in characters of the invite alphabet (no 0 / O / 1 / I): 40 bits.
pub const CODE_LEN: usize = 8;
/// Seconds a code stays valid.
pub const CODE_TTL_SECS: i64 = 15 * 60;
/// Wrong entries that burn a code.
pub const CODE_TRIES: i64 = 5;
/// Codes one account may get in an hour.
pub const CODES_PER_HOUR: i64 = 3;
/// Login-ID mails one address may get in an hour.
pub const ID_MAILS_PER_HOUR: usize = 3;

pub const MSG_NOT_SET_UP: &str =
    "Password recovery is not set up on this server. Ask the host to reset your password.";
pub const MSG_ID_NOT_SET_UP: &str =
    "Login ID recovery is not set up on this server. Ask the host for your login name.";
pub const MSG_SENT: &str = "If the account and the address match, a code was sent.";
pub const MSG_ID_SENT: &str = "If accounts use this address, a mail with their login IDs was sent.";
pub const MSG_CHANGED: &str = "Password changed. Log in with the new one.";
pub const MSG_BAD_CODE: &str = "That code is wrong or has expired. Ask for a new one.";
pub const MSG_SLOW: &str = "Too many attempts from your address. Try again in ten minutes.";

/// True for the paths this module answers.
pub fn handles(path: &str) -> bool {
    matches!(
        path.trim_end_matches('/'),
        "/recover" | "/recover/password" | "/recover/password/confirm" | "/recover/id"
    )
}

fn json_answer(ok: bool, m: &str) -> String {
    format!("{{\"ok\":{},\"message\":{}}}", ok, json_str(m))
}

/// The SHA-256 of a normalised code, as hex.
fn code_hash(code: &str) -> String {
    dc_rom::sha256(code.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A typed code as stored: upper case, without spaces or dashes.
pub fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_uppercase)
        .collect()
}

/// A fresh code from the lobby's key source (each byte picks one of the 32 characters, so no bias).
fn new_code(st: &mut Core) -> String {
    st.keys.key16()[..CODE_LEN]
        .iter()
        .map(|b| accounts::CODE_ALPHABET[(*b & 31) as usize] as char)
        .collect()
}

// ------------------------------------------------------------------------------------------------- HTML pages

const ASK_PAGE_SUB: &str = "Get a code or your login names by e-mail.";

fn ask_page(st: &Core, msg: Option<&str>, ok: bool, name: &str, email: &str) -> String {
    if !st.mail.enabled() {
        return reg_page("Account recovery", "", Some(MSG_NOT_SET_UP), false, "");
    }
    let body = format!(
        "<form method=\"post\" action=\"/recover/password\">\n\
         <label for=\"name\">Login name</label>\
         <input type=\"text\" id=\"name\" name=\"name\" maxlength=\"{nmax}\" value=\"{name}\" autocomplete=\"username\" \
         autocapitalize=\"off\" spellcheck=\"false\">\n\
         <label for=\"email\">E-mail address</label>\
         <input type=\"email\" id=\"email\" name=\"email\" maxlength=\"{emax}\" value=\"{email}\" autocomplete=\"email\">\n\
         <button type=\"submit\" class=\"btn\">Send me a code</button>\n</form>\n\
         <p class=\"hint\"><a href=\"/recover/password/confirm\" style=\"color:var(--accent2)\">I have a code</a></p>\n\
         <form method=\"post\" action=\"/recover/id\">\n\
         <label for=\"email2\">Forgot your login name? E-mail address</label>\
         <input type=\"email\" id=\"email2\" name=\"email\" maxlength=\"{emax}\" value=\"{email}\" autocomplete=\"email\">\n\
         <button type=\"submit\" class=\"btn\">Send me my login names</button>\n</form>\n",
        nmax = accounts::NAME_MAX_LEN,
        emax = accounts::EMAIL_MAX_LEN,
        name = esc(name),
        email = esc(email),
    );
    reg_page("Account recovery", ASK_PAGE_SUB, msg, ok, &body)
}

fn confirm_page(st: &Core, msg: Option<&str>, ok: bool, name: &str) -> String {
    if !st.mail.enabled() {
        return reg_page("Account recovery", "", Some(MSG_NOT_SET_UP), false, "");
    }
    let body = format!(
        "<form method=\"post\" action=\"/recover/password/confirm\">\n\
         <label for=\"name\">Login name</label>\
         <input type=\"text\" id=\"name\" name=\"name\" maxlength=\"{nmax}\" value=\"{name}\" autocomplete=\"username\" \
         autocapitalize=\"off\" spellcheck=\"false\">\n\
         <label for=\"code\">Code from the mail</label>\
         <input type=\"text\" id=\"code\" name=\"code\" maxlength=\"12\" autocomplete=\"one-time-code\" \
         autocapitalize=\"characters\" spellcheck=\"false\">\n\
         <label for=\"pw\">New password</label>\
         <input type=\"password\" id=\"pw\" name=\"pw\" maxlength=\"{pmax}\" autocomplete=\"new-password\">\n\
         <label for=\"pw2\">Confirm new password</label>\
         <input type=\"password\" id=\"pw2\" name=\"pw2\" maxlength=\"{pmax}\" autocomplete=\"new-password\">\n\
         <button type=\"submit\" class=\"btn\">Change password</button>\n</form>\n\
         <p class=\"hint\">The code works once and for 15 minutes. Password: {pmin} to {pmax} characters, not your \
         login name.</p>\n",
        nmax = accounts::NAME_MAX_LEN,
        pmin = accounts::PW_MIN_LEN,
        pmax = accounts::PW_MAX_LEN,
        name = esc(name),
    );
    reg_page(
        "Set a new password",
        "Enter the code we mailed you.",
        msg,
        ok,
        &body,
    )
}

// ------------------------------------------------------------------------------------------------- routing

/// Answers one request on a recovery path inside the caller's scope: (status, HTML or JSON).
pub fn route(
    st: &mut Core,
    method: &str,
    path: &str,
    body: &[u8],
    peer: &str,
    now: Instant,
    json: bool,
) -> rusqlite::Result<(u16, String)> {
    route_at(
        st,
        method,
        path,
        body,
        peer,
        now,
        crate::clock::unix_now(),
        json,
    )
}

/// [`route`] at the unix time `unix` (tests move the clock).
#[allow(clippy::too_many_arguments)]
pub fn route_at(
    st: &mut Core,
    method: &str,
    path: &str,
    body: &[u8],
    peer: &str,
    now: Instant,
    unix: i64,
    json: bool,
) -> rusqlite::Result<(u16, String)> {
    let path = path.trim_end_matches('/');
    let confirm = path == "/recover/password/confirm";
    match method {
        "GET" | "HEAD" => {
            if json {
                return Ok((200, format!("{{\"mail\":{}}}", st.mail.enabled())));
            }
            let page = if confirm {
                confirm_page(st, None, false, "")
            } else {
                ask_page(st, None, false, "", "")
            };
            return Ok((200, page));
        }
        "POST" if path != "/recover" => {}
        _ => {
            if json {
                return Ok((405, json_answer(false, "Use POST.")));
            }
            return Ok((
                405,
                reg_page("Account recovery", "", Some("Use the form."), false, ""),
            ));
        }
    }
    let id = path == "/recover/id";
    if !st.mail.enabled() {
        let m = if id {
            MSG_ID_NOT_SET_UP
        } else {
            MSG_NOT_SET_UP
        };
        if json {
            return Ok((200, json_answer(false, m)));
        }
        return Ok((200, reg_page("Account recovery", "", Some(m), false, "")));
    }
    let ip = crate::limits::peer_ip(peer);
    if st.limits.recover.hit(&ip, now) > crate::limits::REGISTER_ATTEMPTS {
        st.note(&format!("recovery refused (rate limit): {ip}"));
        if json {
            return Ok((429, json_answer(false, MSG_SLOW)));
        }
        return Ok((429, reg_page("Slow down", "", Some(MSG_SLOW), false, "")));
    }
    let f = parse_query(&String::from_utf8_lossy(body));
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    if confirm {
        let name = get("name").trim().to_string();
        let (ok, m) = confirm_code(st, &name, &get("code"), &get("pw"), &get("pw2"), peer, unix)?;
        if json {
            return Ok((200, json_answer(ok, &m)));
        }
        return Ok((200, confirm_page(st, Some(&m), ok, &name)));
    }
    if id {
        let typed = get("email");
        let (ok, m) = match accounts::valid_email(&typed) {
            Err(e) => (false, format!("Cannot send: {e}.")),
            Ok(email) => {
                send_ids(st, &email, now)?;
                (true, MSG_ID_SENT.to_string())
            }
        };
        if json {
            return Ok((200, json_answer(ok, &m)));
        }
        return Ok((200, ask_page(st, Some(&m), ok, "", typed.trim())));
    }
    // /recover/password
    let name = get("name").trim().to_string();
    let typed = get("email");
    let (ok, m) = if name.is_empty() {
        (false, "Enter your login name.".to_string())
    } else {
        match accounts::valid_email(&typed) {
            Err(e) => (false, format!("Cannot send: {e}.")),
            Ok(email) => {
                send_code(st, &name, &email, &ip, unix)?;
                (true, MSG_SENT.to_string())
            }
        }
    };
    if json {
        return Ok((200, json_answer(ok, &m)));
    }
    if ok {
        return Ok((200, confirm_page(st, Some(&m), true, &name)));
    }
    Ok((200, ask_page(st, Some(&m), false, &name, typed.trim())))
}

/// LG-242: a code for `name` when its address is `email`; nothing (and the same answer) otherwise.
/// LG-242: mails a code when `name` and `email` match and the account is under the hourly cap; true when a mail was
/// handed to the sender. Also the dashboard's "send a password reset mail" (playeradmin.rs, LG-396).
pub(crate) fn send_code(
    st: &mut Core,
    name: &str,
    email: &str,
    ip: &str,
    unix: i64,
) -> rusqlite::Result<bool> {
    let Some(acc) = crate::db::account_by_login(&st.db, name)? else {
        st.note(&format!(
            "recovery: code request from {ip}: no match, nothing sent"
        ));
        return Ok(false);
    };
    let stored = crate::db::account_email(&st.db, acc.user_no)?;
    if stored.is_empty() || !stored.eq_ignore_ascii_case(email) {
        st.note(&format!(
            "recovery: code request from {ip}: no match, nothing sent"
        ));
        return Ok(false);
    }
    let recent: i64 = st.db.query_row(
        "SELECT COUNT(*) FROM recovery_codes WHERE user_no = ?1 AND created > ?2",
        params![acc.user_no as i64, unix - 3600],
        |r| r.get(0),
    )?;
    if recent >= CODES_PER_HOUR {
        st.note(&format!(
            "recovery: account {} already got {CODES_PER_HOUR} codes this hour, nothing sent",
            acc.user_no
        ));
        return Ok(false);
    }
    let code = new_code(st);
    // a new code replaces the open one; rows older than a day are dropped
    st.db.execute(
        "UPDATE recovery_codes SET used = 1 WHERE user_no = ?1 AND used = 0",
        params![acc.user_no as i64],
    )?;
    st.db.execute(
        "DELETE FROM recovery_codes WHERE created < ?1",
        params![unix - 86_400],
    )?;
    st.db.execute(
        "INSERT INTO recovery_codes (user_no, code_hash, created, expires) VALUES (?1, ?2, ?3, ?4)",
        params![
            acc.user_no as i64,
            code_hash(&code),
            unix,
            unix + CODE_TTL_SECS
        ],
    )?;
    let body = format!(
        "Hello,\n\n\
         someone asked to reset the password of the login {login} on this DeCombat server.\n\n\
         Your code: {code}\n\n\
         Enter it in the game's password recovery window (or on the server's page /recover/password/confirm) \
         together with a new password. The code works once and expires in 15 minutes.\n\n\
         If you did not ask for this, ignore this mail: your password stays as it is.\n",
        login = acc.login,
    );
    let mail = Mail {
        to: stored,
        subject: st.mail.opts.subject("password reset code"),
        body,
        what: format!("password reset code for account {}", acc.user_no),
        ..Default::default()
    };
    if !st.mail.send(mail) {
        st.note("recovery: the code could not be handed to the mail sender");
        return Ok(false);
    }
    st.note(&format!(
        "recovery: code made for account {} (request from {ip})",
        acc.user_no
    ));
    Ok(true)
}

/// LG-243: the login names of `email`, mailed to it; nothing when none uses it.
fn send_ids(st: &mut Core, email: &str, now: Instant) -> rusqlite::Result<()> {
    let names = crate::db::logins_by_email(&st.db, email)?;
    if names.is_empty() {
        st.note("recovery: login-ID request: no account uses that address, nothing sent");
        return Ok(());
    }
    let key = email.to_lowercase();
    if st.limits.id_mail.hit(&key, now) > ID_MAILS_PER_HOUR {
        st.note(&format!(
            "recovery: {} already got {ID_MAILS_PER_HOUR} login-ID mails this hour, nothing sent",
            accounts::mask_email(email)
        ));
        return Ok(());
    }
    let list: String = names.iter().map(|n| format!("  {n}\n")).collect();
    let body = format!(
        "Hello,\n\n\
         these login IDs on this DeCombat server use this e-mail address:\n\n\
         {list}\n\
         If you also forgot the password, use Find password with the login ID and this address.\n\n\
         If you did not ask for this, ignore this mail.\n"
    );
    let mail = Mail {
        to: email.to_string(),
        subject: st.mail.opts.subject("your login IDs"),
        body,
        what: format!("login-ID list ({} names)", names.len()),
        ..Default::default()
    };
    st.mail.send(mail);
    Ok(())
}

/// LG-242: checks the code and sets the new password. (ok, message).
fn confirm_code(
    st: &mut Core,
    name: &str,
    code: &str,
    pw: &str,
    pw2: &str,
    peer: &str,
    unix: i64,
) -> rusqlite::Result<(bool, String)> {
    let code = normalize_code(code);
    if name.is_empty() || code.is_empty() {
        return Ok((
            false,
            "Enter your login name and the code from the mail.".into(),
        ));
    }
    // the password rules first: their answer says nothing about the code
    if let Err(e) = accounts::valid_password_for(name, pw) {
        return Ok((false, format!("Cannot change the password: {e}.")));
    }
    if pw != pw2 {
        return Ok((
            false,
            "Cannot change the password: the two passwords differ.".into(),
        ));
    }
    let Some(acc) = crate::db::account_by_login(&st.db, name)? else {
        return Ok((false, MSG_BAD_CODE.into()));
    };
    let open: Option<(i64, String, i64)> = st
        .db
        .query_row(
            "SELECT id, code_hash, tries FROM recovery_codes WHERE user_no = ?1 AND used = 0 AND expires > ?2 \
             AND tries < ?3 ORDER BY id DESC LIMIT 1",
            params![acc.user_no as i64, unix, CODE_TRIES],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((id, hash, tries)) = open else {
        st.note(&format!(
            "recovery: code entered for account {} with no open code",
            acc.user_no
        ));
        return Ok((false, MSG_BAD_CODE.into()));
    };
    if code.len() != CODE_LEN || code_hash(&code) != hash {
        let tries = tries + 1;
        st.db.execute(
            "UPDATE recovery_codes SET tries = ?2, used = (?2 >= ?3) WHERE id = ?1",
            params![id, tries, CODE_TRIES],
        )?;
        st.note(&format!(
            "recovery: wrong code for account {} ({tries} of {CODE_TRIES})",
            acc.user_no
        ));
        return Ok((false, MSG_BAD_CODE.into()));
    }
    crate::auth::store_password(st, acc.user_no, pw)?;
    st.db.execute(
        "UPDATE recovery_codes SET used = 1 WHERE user_no = ?1",
        params![acc.user_no as i64],
    )?;
    // the name's login lockout (LG-146) ends with the new password
    let keys = crate::limits::login_keys(&acc.login, peer);
    st.limits.login.success(&keys[0]);
    st.note(&format!(
        "recovery: password of account {} changed with a mailed code (from {})",
        acc.user_no,
        crate::limits::peer_ip(peer)
    ));
    Ok((true, MSG_CHANGED.into()))
}

#[cfg(test)]
#[path = "recover_tests.rs"]
mod tests;
