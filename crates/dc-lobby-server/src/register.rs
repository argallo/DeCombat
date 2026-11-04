//! Registration page on the lobby's web port (`--register`, off by default): `GET /register` serves a form (name,
//! password twice, e-mail address, invite code), `POST /register` creates the account with an Argon2id password and
//! the recovery address (LG-240). Invite codes are
//! required unless `--register-open`. No JavaScript.
//!
//! The success page names only the login: the release downloads come set up for their server (the launcher preset),
//! so there is no host or port to copy.
// DC-DEFAULT: LG-145 registration page; names and passwords LG-141, invites LG-143, passwords LG-140, address LG-240

use crate::accounts::{self, Invite};
use crate::clanweb::{esc, parse_query};
use crate::state::Core;
use std::time::Instant;

/// Largest request body read (the form is four short fields).
pub const MAX_BODY: usize = 4096;
const TITLE: &str = "DeCombat registration";

/// True for the paths this module answers.
pub fn handles(path: &str) -> bool {
    path.trim_end_matches('/') == "/register"
}

/// `Host` header value without its port (none when absent or empty).
pub fn host_of(head: &str) -> Option<String> {
    let v = head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("host")
            .then(|| v.trim().to_string())
    })?;
    let h = if let Some(rest) = v.strip_prefix('[') {
        rest.split(']').next().unwrap_or("").to_string()
    } else {
        v.rsplit_once(':')
            .filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit()))
            .map(|(h, _)| h.to_string())
            .unwrap_or(v)
    };
    (!h.is_empty()
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')))
    .then_some(h)
}

/// The page's own style sheet: a dark, tactical look of our own (no 2009 asset or mark), one file, no script, no
/// external font or image, readable at phone width.
const STYLE: &str = r##"<style>
:root{--bg:#0d0f0d;--panel:#171a17;--panel2:#1e221d;--line:#2f3630;--text:#dcdbd2;--muted:#8d9385;--accent:#d9a73a;--accent2:#f1c65e;--bad:#e2604f;--good:#86c470}
*{box-sizing:border-box}
html,body{margin:0;min-height:100%}
body{background:var(--bg);color:var(--text);font:15px/1.5 "Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif;
background-image:linear-gradient(rgba(255,255,255,.022) 1px,transparent 1px),linear-gradient(90deg,rgba(255,255,255,.022) 1px,transparent 1px),radial-gradient(ellipse at 50% -10%,rgba(217,167,58,.16),transparent 55%);
background-size:32px 32px,32px 32px,100% 100%}
.wrap{min-height:100vh;display:flex;flex-direction:column;align-items:center;justify-content:center;padding:40px 16px}
.brand{display:flex;align-items:center;gap:14px;margin-bottom:22px}
.brand .mark{width:38px;height:38px;border:2px solid var(--accent);transform:rotate(45deg);display:grid;place-items:center;flex:none}
.brand .mark i{width:12px;height:12px;background:var(--accent);display:block}
.brand b{display:block;font-size:22px;line-height:1.1;letter-spacing:.24em;text-transform:uppercase;font-weight:700}
.brand small{display:block;color:var(--muted);font-size:11px;letter-spacing:.3em;text-transform:uppercase;margin-top:3px}
.card{width:100%;max-width:440px;position:relative;padding:30px 30px 26px;background:linear-gradient(180deg,var(--panel2),var(--panel));border:1px solid var(--line);box-shadow:0 24px 60px rgba(0,0,0,.65),inset 0 1px 0 rgba(255,255,255,.04)}
.card:before{content:"";position:absolute;left:0;right:0;top:-1px;height:3px;background:linear-gradient(90deg,var(--accent) 0,var(--accent) 35%,transparent 100%)}
.corner{position:absolute;width:12px;height:12px;border:2px solid var(--accent);opacity:.9}
.tl{top:-1px;left:-1px;border-right:0;border-bottom:0}.tr{top:-1px;right:-1px;border-left:0;border-bottom:0}
.bl{bottom:-1px;left:-1px;border-right:0;border-top:0}.br{bottom:-1px;right:-1px;border-left:0;border-top:0}
h1{margin:0 0 4px;font-size:17px;letter-spacing:.16em;text-transform:uppercase;font-weight:700}
.sub{margin:0 0 18px;color:var(--muted);font-size:13px}
label{display:block;font-size:11px;letter-spacing:.2em;text-transform:uppercase;color:var(--muted);margin:16px 0 6px}
input[type=text],input[type=password],input[type=email]{width:100%;padding:11px 12px;background:#0b0d0b;border:1px solid var(--line);color:var(--text);font:inherit;outline:0;border-radius:2px;transition:border-color .15s,box-shadow .15s}
input:focus{border-color:var(--accent);box-shadow:0 0 0 3px rgba(217,167,58,.2)}
.btn{display:block;width:100%;margin-top:24px;padding:13px;border:0;border-radius:2px;background:linear-gradient(180deg,var(--accent2),var(--accent));color:#191303;font:inherit;font-weight:700;letter-spacing:.22em;text-transform:uppercase;cursor:pointer;clip-path:polygon(10px 0,100% 0,100% calc(100% - 10px),calc(100% - 10px) 100%,0 100%,0 10px)}
.btn:hover{filter:brightness(1.08)}.btn:active{filter:brightness(.95)}
.msg{padding:10px 12px;margin:0 0 4px;border-left:3px solid var(--bad);background:rgba(226,96,79,.12);font-size:14px}
.msg.ok{border-color:var(--good);background:rgba(134,196,112,.12)}
.hint{color:var(--muted);font-size:12px;margin:14px 0 0;line-height:1.55}
.hint b{color:var(--text)}
.kv{width:100%;border-collapse:collapse;margin:14px 0 6px}
.kv td{padding:9px 10px;border:1px solid var(--line);font-size:14px;vertical-align:top}
.kv td:first-child{color:var(--muted);font-size:11px;letter-spacing:.2em;text-transform:uppercase;width:36%}
.foot{color:var(--muted);font-size:11px;letter-spacing:.24em;text-transform:uppercase;margin-top:20px;text-align:center}
@media (max-width:480px){.card{padding:24px 18px}.brand b{font-size:19px}}
</style>"##;

/// The registration pages' frame: brand, a card with `heading` and `sub`, the message (`ok` green, else red), the
/// body. One HTML document, no script (clanweb_tests check for that).
pub(crate) fn reg_page(
    heading: &str,
    sub: &str,
    msg: Option<&str>,
    ok: bool,
    body: &str,
) -> String {
    let mut s = String::with_capacity(4096);
    s.push_str(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" \
                content=\"width=device-width,initial-scale=1\"><title>",
    );
    s.push_str(&esc(TITLE));
    s.push_str("</title>");
    s.push_str(STYLE);
    s.push_str(
        "</head><body><div class=\"wrap\">\n<div class=\"brand\"><span class=\"mark\"><i></i></span>\
         <span><b>DeCombat</b><small>Account registration</small></span></div>\n\
         <main class=\"card\"><span class=\"corner tl\"></span><span class=\"corner tr\"></span>\
         <span class=\"corner bl\"></span><span class=\"corner br\"></span>\n",
    );
    s.push_str(&format!(
        "<h1>{}</h1>\n<p class=\"sub\">{}</p>\n",
        esc(heading),
        esc(sub)
    ));
    if let Some(m) = msg {
        s.push_str(&format!(
            "<p class=\"msg{}\">{}</p>\n",
            if ok { " ok" } else { "" },
            esc(m)
        ));
    }
    s.push_str(body);
    s.push_str("</main>\n<p class=\"foot\">Private DeCombat server</p>\n</div></body></html>\n");
    s
}

fn form(st: &Core, msg: Option<&str>, name: &str, email: &str, code: &str) -> String {
    let open = st.opts.register_open;
    let body = format!(
        "<form method=\"post\" action=\"/register\">\n\
         <label for=\"name\">Login name</label>\
         <input type=\"text\" id=\"name\" name=\"name\" maxlength=\"{nmax}\" value=\"{name}\" autocomplete=\"username\" \
         autocapitalize=\"off\" spellcheck=\"false\" autofocus>\n\
         <label for=\"pw\">Password</label>\
         <input type=\"password\" id=\"pw\" name=\"pw\" maxlength=\"{pmax}\" autocomplete=\"new-password\">\n\
         <label for=\"pw2\">Confirm password</label>\
         <input type=\"password\" id=\"pw2\" name=\"pw2\" maxlength=\"{pmax}\" autocomplete=\"new-password\">\n\
         <label for=\"email\">E-mail address</label>\
         <input type=\"email\" id=\"email\" name=\"email\" maxlength=\"{emax}\" value=\"{email}\" autocomplete=\"email\" \
         spellcheck=\"false\">\n\
         <label for=\"code\">Invite code{opt}</label>\
         <input type=\"text\" id=\"code\" name=\"code\" maxlength=\"20\" value=\"{code}\" autocapitalize=\"characters\" \
         spellcheck=\"false\">\n\
         <button type=\"submit\" class=\"btn\">Create account</button>\n</form>\n\
         <p class=\"hint\">Login name: {nmin} to {nmax} letters A-Z, digits, _ - . &nbsp;Password: {pmin} to {pmax} \
         characters (the game cannot send more).</p>\n\
         <p class=\"hint\">Any characters are allowed; a long phrase beats a clever mix. \
         <b>Your password is stored hashed on the server. Use one you use nowhere else.</b></p>\n\
         <p class=\"hint\">The e-mail address is only used to send you a code when you forget your password or \
         login name; it is never shown to other players.</p>\n{recover}",
        nmin = accounts::NAME_MIN_LEN,
        nmax = accounts::NAME_MAX_LEN,
        pmin = accounts::PW_MIN_LEN,
        pmax = accounts::PW_MAX_LEN,
        emax = accounts::EMAIL_MAX_LEN,
        name = esc(name),
        email = esc(email),
        code = esc(code),
        opt = if open { " (optional)" } else { "" },
        recover = if st.mail.enabled() {
            "<p class=\"hint\"><a href=\"/recover\" style=\"color:var(--accent2)\">Forgot your password or \
             login name?</a></p>\n"
        } else {
            ""
        },
    );
    reg_page(
        "Create your account",
        "One account per player. Your name is shown in game.",
        msg,
        false,
        &body,
    )
}

/// True when `code` is an open invite made for `login` and that account is a GM: the one way to register a
/// GM-prefixed login (`account add GMname --invite` then `account gm GMname --on`).
// DC-DEFAULT: LG-172 a GM account's own invite may register its GM-prefixed login
fn gm_account_invite(st: &Core, code: &str, login: &str) -> rusqlite::Result<bool> {
    if code.is_empty() {
        return Ok(false);
    }
    let bound = matches!(
        accounts::invite(&st.db, code, crate::clock::unix_now())?,
        Invite::Open(Some(name)) if name.eq_ignore_ascii_case(login)
    );
    Ok(bound && crate::db::account_by_login(&st.db, login)?.is_some_and(|a| a.gm != 0))
}

fn done(login: &str) -> String {
    // the release downloads come set up for their server, so the page names only what the player types
    let body = format!(
        "<p>The account <b>{login}</b> is ready. Start DeCombat and sign in with:</p>\n\
         <table class=\"kv\">\
         <tr><td>Login</td><td><b>{login}</b></td></tr>\
         <tr><td>Password</td><td>the one you just chose</td></tr>\
         </table>\n\
         <p class=\"hint\">Your in-game name is chosen at the first sign-in, and it must differ from the login.</p>\n",
        login = esc(login),
    );
    reg_page(
        "Account ready",
        "Welcome aboard.",
        Some("Account created."),
        true,
        &body,
    )
}

/// Answers one request on a registration path inside the caller's scope: (status, HTML).
#[cfg(test)]
pub fn route(
    st: &mut Core,
    method: &str,
    body: &[u8],
    peer: &str,
    host: Option<&str>,
    now: Instant,
) -> rusqlite::Result<(u16, String)> {
    route_as(st, method, body, peer, host, now, false)
}

/// The JSON reply of the game's own registration dialog (LG-210): `json` asks for it.
pub fn wants_json(head: &str, form: &[u8], query: &crate::clanweb::Query) -> bool {
    let accept = head.lines().skip(1).any(|l| {
        l.split_once(':').is_some_and(|(k, v)| {
            k.trim().eq_ignore_ascii_case("accept")
                && v.to_ascii_lowercase().contains("application/json")
        })
    });
    accept
        || query.get("fmt").is_some_and(|v| v == "json")
        || parse_query(&String::from_utf8_lossy(form))
            .get("fmt")
            .is_some_and(|v| v == "json")
}

/// A JSON string literal.
pub fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

// DC-DEFAULT: LG-210 (proposed) the JSON forms of the answers: GET `{"open", "invite", "email"}`, POST
// `{"ok", "login"}` or `{"ok", "message"}` (the HTML page's message text), with the HTML path's status codes;
// `email` true = the form needs the address (LG-240)
fn json_info(st: &Core) -> String {
    format!(
        "{{\"open\":{},\"invite\":{},\"email\":true}}",
        st.opts.register,
        st.opts.register && !st.opts.register_open
    )
}

fn json_refused(m: &str) -> String {
    format!("{{\"ok\":false,\"message\":{}}}", json_str(m))
}

fn json_done(login: &str) -> String {
    format!("{{\"ok\":true,\"login\":{}}}", json_str(login))
}

/// [`route`] with the answer as HTML or (`json`) as the LG-210 JSON, whole, under one lock (the tests' way in).
#[cfg(test)]
pub fn route_as(
    st: &mut Core,
    method: &str,
    body: &[u8],
    peer: &str,
    host: Option<&str>,
    now: Instant,
    json: bool,
) -> rusqlite::Result<(u16, String)> {
    match begin(st, method, body, peer, host, now, json)? {
        RegStep::Done(r) => Ok(r),
        RegStep::Hash(h) => {
            let phc = h.compute();
            finish(st, body, peer, host, json, &phc)
        }
    }
}

/// Phase 1 of a request (locked): the rate limit and every check. The web server then runs [`RegHash::compute`] on the
/// password pool and calls [`finish`], so Argon2 runs outside the lock (LG-328).
pub fn begin(
    st: &mut Core,
    method: &str,
    body: &[u8],
    peer: &str,
    host: Option<&str>,
    now: Instant,
    json: bool,
) -> rusqlite::Result<RegStep> {
    if let Some(r) = gate(st, method, peer, now, json) {
        return Ok(RegStep::Done(r));
    }
    post(st, body, peer, host, json, None)
}

/// The answer of anything but a POST within the rate limit.
fn gate(
    st: &mut Core,
    method: &str,
    peer: &str,
    now: Instant,
    json: bool,
) -> Option<(u16, String)> {
    if json && method != "POST" && !st.opts.register {
        return Some((404, json_info(st)));
    }
    if json && !st.opts.register {
        return Some((
            404,
            json_refused("Registration is off on this server. Ask its host for an account."),
        ));
    }
    if json && matches!(method, "GET" | "HEAD") {
        return Some((200, json_info(st)));
    }
    if !st.opts.register {
        return Some((
            404,
            reg_page(
                "Registration closed",
                "This server does not take new accounts by itself.",
                Some("Registration is off on this server. Ask its host for an account."),
                false,
                "",
            ),
        ));
    }
    match method {
        "GET" | "HEAD" => return Some((200, form(st, None, "", "", ""))),
        "POST" => {}
        _ => {
            return Some((
                405,
                reg_page("Create your account", "", Some("Use the form."), false, ""),
            ))
        }
    }
    let ip = crate::limits::peer_ip(peer);
    if st.limits.register.hit(&ip, now) > crate::limits::REGISTER_ATTEMPTS {
        st.note(&format!("registration refused (rate limit): {}", ip));
        if json {
            return Some((
                429,
                json_refused("Too many attempts from your address. Try again in ten minutes."),
            ));
        }
        return Some((
            429,
            reg_page(
                "Slow down",
                "",
                Some("Too many attempts from your address. Try again in ten minutes."),
                false,
                "",
            ),
        ));
    }
    None
}

/// What the first (locked) phase of a registration leaves to do.
pub enum RegStep {
    /// Answered.
    Done((u16, String)),
    /// The hash still to make, outside the `Core` lock.
    Hash(RegHash),
}

/// A registration that passed every check and waits for its password hash.
pub struct RegHash {
    password: String,
    salt: [u8; 16],
}

impl RegHash {
    /// The slow part; takes no lock.
    pub fn compute(&self) -> String {
        crate::pwhash::encode(&self.salt, &self.password)
    }
}

/// Phase 2 (locked again): the same checks with the hash in hand, then the writes. The rate limit was counted in phase 1.
pub fn finish(
    st: &mut Core,
    body: &[u8],
    peer: &str,
    host: Option<&str>,
    json: bool,
    phc: &str,
) -> crate::db::DbResult<(u16, String)> {
    match post(st, body, peer, host, json, Some(phc))? {
        RegStep::Done(r) => Ok(r),
        RegStep::Hash(_) => Err(rusqlite::Error::QueryReturnedNoRows),
    }
}

/// The form's checks in order; with `phc`, the writes after them.
// DC-DEFAULT: LG-328 (proposed, project decision)
fn post(
    st: &mut Core,
    body: &[u8],
    peer: &str,
    // the success page no longer names the host (the downloads are preset); kept for the callers' shape
    _host: Option<&str>,
    json: bool,
    phc: Option<&str>,
) -> crate::db::DbResult<RegStep> {
    let ip = crate::limits::peer_ip(peer);
    let f = parse_query(&String::from_utf8_lossy(body));
    let get = |k: &str| f.get(k).cloned().unwrap_or_default();
    let (login, pw, pw2, code, email_typed) = (
        get("name").trim().to_string(),
        get("pw"),
        get("pw2"),
        get("code"),
        get("email"),
    );
    let code = accounts::normalize_code(&code);
    let refuse = |st: &Core, m: &str| {
        if json {
            return Ok(RegStep::Done((200, json_refused(m))));
        }
        Ok(RegStep::Done((
            200,
            form(st, Some(m), &login, email_typed.trim(), &code),
        )))
    };
    let checked = if gm_account_invite(st, &code, &login)? {
        accounts::valid_login(&login)
    } else {
        accounts::valid_new_login(&login)
    };
    if let Err(e) = checked {
        return refuse(st, &format!("Cannot register: {e}."));
    }
    if let Err(e) = accounts::valid_password_for(&login, &pw) {
        return refuse(st, &format!("Cannot register: {e}."));
    }
    if pw != pw2 {
        return refuse(st, "Cannot register: the two passwords differ.");
    }
    let email = match accounts::valid_email(&email_typed) {
        Ok(e) => e,
        Err(e) => return refuse(st, &format!("Cannot register: {e}.")),
    };
    let unix = crate::clock::unix_now();
    let bound = if code.is_empty() && st.opts.register_open {
        None
    } else if code.is_empty() {
        return refuse(st, "Cannot register: enter your invite code.");
    } else {
        match accounts::invite(&st.db, &code, unix)? {
            Invite::Open(b) => Some(b),
            Invite::Unknown => return refuse(st, "Cannot register: unknown invite code."),
            Invite::Used => {
                return refuse(st, "Cannot register: that invite code was already used.")
            }
            Invite::Expired => return refuse(st, "Cannot register: that invite code has expired."),
        }
    };
    // LG-318: the pool's prefix is for the pool
    if crate::botpool::pool_named(&login) {
        return refuse(st, "Cannot register: that name is taken.");
    }
    let existing = crate::db::account_by_login(&st.db, &login)?;
    match (&bound, &existing) {
        // a code made by `account add NAME --invite` sets the password of that password-less account
        (Some(Some(name)), _) if !name.eq_ignore_ascii_case(&login) => {
            return refuse(st, "Cannot register: that invite code is for another name.")
        }
        (Some(Some(_)), Some(acc)) if acc.pw_hash.as_deref().is_some_and(|h| !h.is_empty()) => {
            return refuse(st, "Cannot register: that account already has a password.")
        }
        (Some(Some(_)), Some(_)) | (_, None) => {}
        (_, Some(_)) => return refuse(st, "Cannot register: that name is taken."),
    }
    // LG-328: everything above is checked again with the hash in hand; the hash itself is not made under the lock
    let Some(phc) = phc else {
        return Ok(RegStep::Hash(RegHash {
            password: pw,
            salt: st.keys.key16(),
        }));
    };
    let user_no = match existing {
        Some(acc) => {
            crate::db::set_password_phc(&st.db, acc.user_no, phc)?;
            acc.user_no
        }
        None => {
            let first = st.rules.first_account_no;
            match accounts::create_with_phc(&st.db, &login, phc, first, unix) {
                Ok(n) => n,
                Err(e) => return refuse(st, &format!("Cannot register: {e}.")),
            }
        }
    };
    // read and used inside one IMMEDIATE transaction, so the code is still open; should it not be, the error
    // rolls the new account back
    if bound.is_some() && !accounts::consume_invite(&st.db, &code, user_no, unix)? {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    crate::db::set_email(&st.db, user_no, &email)?;
    st.note(&format!(
        "registration: account {} ready (account {}, e-mail {}) from {}{}",
        login,
        user_no,
        accounts::mask_email(&email),
        ip,
        if bound.is_some() {
            " with an invite code"
        } else {
            ""
        }
    ));
    if json {
        return Ok(RegStep::Done((200, json_done(&login))));
    }
    Ok(RegStep::Done((200, done(&login))))
}

/// The answer to a registration when the password pool has no room (LG-328).
pub fn busy(json: bool) -> (u16, String) {
    let m = "The server is busy. Try again in a moment.";
    if json {
        (503, json_refused(m))
    } else {
        (503, reg_page("Slow down", "", Some(m), false, ""))
    }
}
