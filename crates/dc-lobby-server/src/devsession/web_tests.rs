//! The device-session endpoints at handler level (SES-1..SES-6, SES-9), and the client crate's flow driven
//! against them in-process.

use super::*;
use crate::devsession::db::{self as dsdb, reason};
use crate::identity::{verify, Authority};
use crate::state::testing::*;
use ring::signature::{Ed25519KeyPair, KeyPair};

const PEER: &str = "203.0.113.5:4000";
const PW: &str = "rabbit-hole";

fn pair() -> Ed25519KeyPair {
    let doc = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
    Ed25519KeyPair::from_pkcs8(doc.as_ref()).unwrap()
}

/// An issuing lobby with device sessions on and the account `alice`; returns its public key.
fn lobby() -> (Core, Vec<u8>) {
    let mut st = state();
    let kp = pair();
    let public = kp.public_key().as_ref().to_vec();
    st.identity = Authority::issuing("dc.main", kp, 24);
    st.identity.devices = DeviceRules::on();
    st.opts.pw_check = true;
    st.scope(|st| {
        crate::auth::ensure_account(st, "alice", PW)?;
        Ok(())
    })
    .unwrap();
    (st, public)
}

fn alice(st: &Core) -> db::Account {
    db::account_by_login(&st.db, "alice").unwrap().unwrap()
}

/// A client-made secret and its proposed hash (as dc-session makes them).
fn secret() -> (String, String) {
    let s = dc_session::Secret::generate().unwrap();
    (s.expose().to_string(), s.proposed_hash())
}

fn get(json: &str, k: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    match &v[k] {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `POST /identity/device` through both phases, as the web port runs it.
fn sign_in_at(st: &mut Core, body: &str, peer: &str, now: Instant) -> (u16, String) {
    match begin_sign_in(st, "POST", body.as_bytes(), peer, now).unwrap() {
        Step::Done(r) => r,
        Step::Verify(p) => {
            let c = p.check();
            finish_sign_in(st, p, c, now).unwrap()
        }
    }
}

fn sign_in(st: &mut Core, hash: &str) -> (u16, String) {
    let body = format!(
        r#"{{"login":"alice","password":"{PW}","next_hash":"{hash}","name":"Desk","platform":"windows"}}"#
    );
    sign_in_at(st, &body, PEER, Instant::now())
}

fn post(st: &mut Core, path: &str, body: &str) -> (u16, String) {
    route(st, path, "POST", body.as_bytes(), PEER, Instant::now()).unwrap()
}

fn refresh_at(st: &mut Core, device: &str, secret: &str, next: &str, peer: &str) -> (u16, String) {
    let body = format!(r#"{{"device":"{device}","secret":"{secret}","next_hash":"{next}"}}"#);
    route(
        st,
        REFRESH_PATH,
        "POST",
        body.as_bytes(),
        peer,
        Instant::now(),
    )
    .unwrap()
}

fn refresh(st: &mut Core, device: &str, secret: &str, next: &str) -> (u16, String) {
    refresh_at(st, device, secret, next, PEER)
}

/// A device made by a sign-in: (device id, its current secret).
fn device(st: &mut Core) -> (String, String) {
    let (s0, h0) = secret();
    let (code, body) = sign_in(st, &h0);
    assert_eq!(code, 200, "{body}");
    (get(&body, "device"), s0)
}

#[test]
fn a_lobby_without_device_sessions_answers_as_if_the_endpoints_were_not_there() {
    let (_, h) = secret();
    // not issuing at all
    let mut plain = state();
    assert_eq!(sign_in(&mut plain, &h).0, 404);
    // issuing, but identity.device_sessions = false
    let (mut st, _) = lobby();
    st.identity.devices.enabled = false;
    let (code, body) = sign_in(&mut st, &h);
    assert_eq!((code, body.as_str()), (404, MSG_OFF));
    assert_eq!(post(&mut st, REFRESH_PATH, "{}").0, 404);
    assert_eq!(post(&mut st, SIGNOUT_PATH, "{}").0, 404);
    // the token endpoint is not affected
    let (code, _) = crate::identity::route(
        &mut st,
        "POST",
        format!(r#"{{"login":"alice","password":"{PW}"}}"#).as_bytes(),
        PEER,
        Instant::now(),
    )
    .unwrap();
    assert_eq!(code, 200);
    // on: only POST
    st.identity.devices.enabled = true;
    assert_eq!(
        route(&mut st, REFRESH_PATH, "GET", b"", PEER, Instant::now())
            .unwrap()
            .0,
        405
    );
    assert!(handles("/identity/device/") && handles(REFRESH_PATH) && handles(SIGNOUT_PATH));
    assert!(!handles("/identity/token") && is_sign_in(DEVICE_PATH) && !is_sign_in(REFRESH_PATH));
}

#[test]
fn a_sign_in_makes_a_device_and_a_short_token() {
    let (mut st, public) = lobby();
    let (s0, h0) = secret();
    let (code, body) = sign_in(&mut st, &h0);
    assert_eq!(code, 200, "{body}");
    let dev = get(&body, "device");
    assert!(dsdb::valid_device_id(&dev));
    let c = verify(
        &get(&body, "token"),
        &public,
        "dc.main",
        crate::clock::unix_now(),
    )
    .unwrap();
    assert_eq!(
        c.expires - c.issued,
        15 * 60,
        "SES-4: 15-minute device tokens"
    );
    assert_eq!(get(&body, "expires"), c.expires.to_string());
    assert_eq!(get(&body, "account"), alice(&st).user_no.to_string());
    assert_eq!(get(&body, "idle_days"), "180");
    assert_eq!(
        get(&body, "device_expires").parse::<i64>().unwrap(),
        c.issued + 180 * 86_400
    );
    let row = dsdb::get(&st.db, &dev).unwrap().unwrap();
    assert_eq!(
        (
            row.user_no,
            row.name.as_str(),
            row.platform.as_str(),
            row.last_place.as_str()
        ),
        (alice(&st).user_no, "Desk", "windows", "203.0.113.0/24")
    );
    // the stored hash is the proposed one: the first secret spends
    let (_, h1) = secret();
    assert_eq!(refresh(&mut st, &dev, &s0, &h1).0, 200);
    // the password endpoint keeps its 24 h
    let (_, body) = crate::identity::route(
        &mut st,
        "POST",
        format!(r#"{{"login":"alice","password":"{PW}"}}"#).as_bytes(),
        PEER,
        Instant::now(),
    )
    .unwrap();
    let c = verify(
        &get(&body, "token"),
        &public,
        "dc.main",
        crate::clock::unix_now(),
    )
    .unwrap();
    assert_eq!(c.expires - c.issued, 24 * 3600);
}

#[test]
fn a_sign_in_refuses_a_wrong_password_a_bad_request_and_a_lockout() {
    let (mut st, _) = lobby();
    let (_, h) = secret();
    let t0 = Instant::now();
    let wrong = format!(r#"{{"login":"alice","password":"nope","next_hash":"{h}"}}"#);
    let (code, body) = sign_in_at(&mut st, &wrong, "198.51.100.1:1", t0);
    assert_eq!((code, body.as_str()), (401, crate::identity::MSG_REFUSED));
    for bad in [
        "login=alice".to_string(),
        format!(r#"{{"password":"{PW}","next_hash":"{h}"}}"#),
        format!(r#"{{"login":"alice","password":"{PW}","next_hash":"short"}}"#),
        format!(
            r#"{{"login":"alice","password":"{PW}","next_hash":"{}"}}"#,
            crate::identity::b64url(&[1u8; 33])
        ),
        format!(
            r#"{{"login":"alice","password":"{PW}","next_hash":"{h}","name":"{}"}}"#,
            "n".repeat(65)
        ),
    ] {
        assert_eq!(sign_in_at(&mut st, &bad, PEER, t0).0, 400, "{bad}");
    }
    assert_eq!(dsdb::count_live(&st.db, alice(&st).user_no).unwrap(), 0);
    // nine more wrong passwords lock the name: the right one is then a 429, not a 401
    for _ in 0..9 {
        sign_in_at(&mut st, &wrong, "198.51.100.1:1", t0);
    }
    let right = format!(r#"{{"login":"alice","password":"{PW}","next_hash":"{h}"}}"#);
    let (code, body) = sign_in_at(&mut st, &right, "192.0.2.9:1", t0);
    assert_eq!((code, body.as_str()), (429, MSG_LIMITED));
}

#[test]
fn a_banned_or_disabled_account_gets_no_device() {
    let (mut st, _) = lobby();
    let (_, h) = secret();
    let no = alice(&st).user_no;
    st.db
        .execute(
            "INSERT INTO op_bans (user_no, until) VALUES (?1, 0)",
            [no as i64],
        )
        .unwrap();
    assert_eq!(sign_in(&mut st, &h).0, 401);
    st.db.execute("DELETE FROM op_bans", []).unwrap();
    crate::accounts::set_disabled(&st.db, "alice", true).unwrap();
    assert_eq!(sign_in(&mut st, &h).0, 401);
    assert_eq!(dsdb::count_live(&st.db, no).unwrap(), 0);
}

#[test]
fn a_refresh_rotates_issues_a_short_token_and_a_resend_is_answered_alike() {
    let (mut st, public) = lobby();
    let (dev, s0) = device(&mut st);
    let (s1, h1) = secret();
    let (code, body) = refresh(&mut st, &dev, &s0, &h1);
    assert_eq!(code, 200, "{body}");
    let c = verify(
        &get(&body, "token"),
        &public,
        "dc.main",
        crate::clock::unix_now(),
    )
    .unwrap();
    assert_eq!(c.expires - c.issued, 15 * 60);
    assert_eq!(get(&body, "account"), alice(&st).user_no.to_string());
    assert!(get(&body, "device_expires").parse::<i64>().unwrap() >= c.issued + 180 * 86_400);
    assert_eq!(dsdb::get(&st.db, &dev).unwrap().unwrap().generation, 1);
    // the same request again (a retry after a lost answer): answered as the rotation, nothing changes
    let (code, _) = refresh(&mut st, &dev, &s0, &h1);
    assert_eq!(code, 200);
    let row = dsdb::get(&st.db, &dev).unwrap().unwrap();
    assert!(row.live());
    assert_eq!(row.generation, 1);
    let (_, h2) = secret();
    assert_eq!(refresh(&mut st, &dev, &s1, &h2).0, 200);
}

#[test]
fn reuse_revokes_the_device_and_ends_the_accounts_tokens() {
    let (mut st, public) = lobby();
    let (dev, s0) = device(&mut st);
    let (s1, h1) = secret();
    let (_, body) = refresh(&mut st, &dev, &s0, &h1);
    let earlier = verify(
        &get(&body, "token"),
        &public,
        "dc.main",
        crate::clock::unix_now(),
    )
    .unwrap();
    let (s2, h2) = secret();
    assert_eq!(refresh(&mut st, &dev, &s1, &h2).0, 200);
    // a copy of s0 (two rotations old) comes back
    let (_, h9) = secret();
    let (code, body) = refresh(&mut st, &dev, &s0, &h9);
    assert_eq!((code, body.as_str()), (401, SIGNED_OUT));
    assert_eq!(
        dsdb::get(&st.db, &dev).unwrap().unwrap().revoked,
        reason::REUSE
    );
    let a = alice(&st);
    assert!(
        a.tokens_valid_after > earlier.issued,
        "tokens issued so far are ended"
    );
    assert!(crate::identity::local_account(&mut st, &earlier)
        .unwrap()
        .is_err());
    // the rightful holder is signed out too
    let (_, h3) = secret();
    assert_eq!(refresh(&mut st, &dev, &s2, &h3).1, SIGNED_OUT);
}

#[test]
fn an_idle_device_expires() {
    let (mut st, _) = lobby();
    let (dev, s0) = device(&mut st);
    st.db
        .execute(
            "UPDATE device_sessions SET last_used = last_used - ?2 WHERE device = ?1",
            rusqlite::params![dev, 181 * 86_400i64],
        )
        .unwrap();
    let (_, h1) = secret();
    assert_eq!(refresh(&mut st, &dev, &s0, &h1), (401, SIGNED_OUT.into()));
    assert_eq!(
        dsdb::get(&st.db, &dev).unwrap().unwrap().revoked,
        reason::IDLE
    );
}

#[test]
fn the_cap_evicts_the_longest_idle_device_and_a_day_allows_twenty_new_ones() {
    let (mut st, _) = lobby();
    st.identity.devices.max_devices = 2;
    let (first, s_first) = device(&mut st);
    st.db
        .execute(
            "UPDATE device_sessions SET last_used = last_used - 100 WHERE device = ?1",
            [&first],
        )
        .unwrap();
    device(&mut st);
    device(&mut st);
    assert_eq!(
        dsdb::get(&st.db, &first).unwrap().unwrap().revoked,
        reason::CAP
    );
    assert_eq!(dsdb::count_live(&st.db, alice(&st).user_no).unwrap(), 2);
    let (_, h) = secret();
    assert_eq!(refresh(&mut st, &first, &s_first, &h).1, SIGNED_OUT);
    // 3 made so far; 17 more are fine, the 21st of the day is a 429 and makes nothing
    for _ in 0..17 {
        device(&mut st);
    }
    let (code, body) = sign_in(&mut st, &h);
    assert_eq!((code, body.as_str()), (429, MSG_LIMITED));
    assert_eq!(
        dsdb::created_since(&st.db, alice(&st).user_no, 0).unwrap(),
        DEVICES_PER_DAY
    );
}

#[test]
fn a_banned_disabled_or_deleted_account_cannot_refresh_and_nothing_is_spent() {
    let (mut st, _) = lobby();
    let (dev, s0) = device(&mut st);
    let no = alice(&st).user_no;
    let (_, h1) = secret();
    st.db
        .execute(
            "INSERT INTO op_bans (user_no, until) VALUES (?1, 0)",
            [no as i64],
        )
        .unwrap();
    assert_eq!(refresh(&mut st, &dev, &s0, &h1), (401, SIGNED_OUT.into()));
    st.db.execute("DELETE FROM op_bans", []).unwrap();
    crate::accounts::set_disabled(&st.db, "alice", true).unwrap();
    assert_eq!(refresh(&mut st, &dev, &s0, &h1).0, 401);
    crate::accounts::set_disabled(&st.db, "alice", false).unwrap();
    let row = dsdb::get(&st.db, &dev).unwrap().unwrap();
    assert!(row.live() && row.generation == 0, "nothing was spent");
    // the ban over, the same secret still works
    assert_eq!(refresh(&mut st, &dev, &s0, &h1).0, 200);
    // a deleted account
    let (dev2, s) = device(&mut st);
    st.db
        .execute("DELETE FROM accounts WHERE user_no = ?1", [no as i64])
        .unwrap();
    let (_, h) = secret();
    assert_eq!(refresh(&mut st, &dev2, &s, &h).0, 401);
}

#[test]
fn guesses_are_counted_and_lock_the_address_with_a_429() {
    let (mut st, _) = lobby();
    let (dev, s0) = device(&mut st);
    let (wrong, h) = secret();
    let other = dsdb::new_device_id().unwrap();
    let guesser = "192.0.2.77:1";
    // unknown device, malformed device, wrong secret, oversized secret: one answer each
    for (d, s) in [
        (other.as_str(), s0.as_str()),
        ("dv_nonsense", s0.as_str()),
        (dev.as_str(), wrong.as_str()),
        (dev.as_str(), &"x".repeat(MAX_SECRET + 1)),
    ] {
        assert_eq!(
            refresh_at(&mut st, d, s, &h, guesser),
            (401, SIGNED_OUT.into())
        );
    }
    assert!(
        dsdb::get(&st.db, &dev).unwrap().unwrap().live(),
        "a guess never revokes"
    );
    for _ in 0..6 {
        refresh_at(&mut st, &dev, &wrong, &h, guesser);
    }
    // ten guesses: the address is locked; even the right secret from there is a 429 and spends nothing
    assert_eq!(
        refresh_at(&mut st, &dev, &s0, &h, guesser),
        (429, MSG_LIMITED.into())
    );
    assert_eq!(dsdb::get(&st.db, &dev).unwrap().unwrap().generation, 0);
    // from the owner's own address it works
    assert_eq!(refresh(&mut st, &dev, &s0, &h).0, 200);
}

#[test]
fn a_malformed_next_hash_is_a_400_and_spends_nothing() {
    let (mut st, _) = lobby();
    let (dev, s0) = device(&mut st);
    for bad in [
        "".to_string(),
        "not base64!".into(),
        crate::identity::b64url(&[7u8; 31]),
        crate::identity::b64url(&[7u8; 33]),
    ] {
        assert_eq!(refresh(&mut st, &dev, &s0, &bad).0, 400, "{bad}");
    }
    assert_eq!(dsdb::get(&st.db, &dev).unwrap().unwrap().generation, 0);
    assert_eq!(post(&mut st, REFRESH_PATH, "not json").0, 400);
}

#[test]
fn refreshes_per_device_and_per_address_are_limited() {
    let (mut st, _) = lobby();
    let (dev, mut cur) = device(&mut st);
    for _ in 0..REFRESH_PER_DEVICE {
        let (next, h) = secret();
        assert_eq!(refresh(&mut st, &dev, &cur, &h).0, 200);
        cur = next;
    }
    let (_, h) = secret();
    assert_eq!(refresh(&mut st, &dev, &cur, &h), (429, MSG_LIMITED.into()));
    // the address limit counts every refresh and sign-out of the address
    let busy = "192.0.2.200:1";
    for _ in 0..REFRESH_PER_ADDR {
        post_at(&mut st, SIGNOUT_PATH, "{}", busy);
    }
    assert_eq!(post_at(&mut st, SIGNOUT_PATH, "{}", busy).0, 429);
}

fn post_at(st: &mut Core, path: &str, body: &str, peer: &str) -> (u16, String) {
    route(st, path, "POST", body.as_bytes(), peer, Instant::now()).unwrap()
}

#[test]
fn a_sign_out_needs_the_current_secret_and_always_answers_200() {
    let (mut st, _) = lobby();
    let (dev, s0) = device(&mut st);
    let (wrong, _) = secret();
    for body in [
        format!(r#"{{"device":"{dev}","secret":"{wrong}"}}"#),
        r#"{"device":"dv_x","secret":"y"}"#.to_string(),
        "{}".into(),
    ] {
        assert_eq!(post(&mut st, SIGNOUT_PATH, &body), (200, "{}".into()));
    }
    assert!(dsdb::get(&st.db, &dev).unwrap().unwrap().live());
    let body = format!(r#"{{"device":"{dev}","secret":"{s0}"}}"#);
    assert_eq!(post(&mut st, SIGNOUT_PATH, &body), (200, "{}".into()));
    assert_eq!(
        dsdb::get(&st.db, &dev).unwrap().unwrap().revoked,
        reason::SIGN_OUT
    );
    let (_, h) = secret();
    assert_eq!(refresh(&mut st, &dev, &s0, &h).1, SIGNED_OUT);
}

#[test]
fn the_sweep_expires_idle_devices_and_forgets_old_revoked_ones() {
    let (mut st, _) = lobby();
    let (idle, _) = device(&mut st);
    let (old, _) = device(&mut st);
    let (fresh, _) = device(&mut st);
    st.db
        .execute(
            "UPDATE device_sessions SET last_used = last_used - ?2 WHERE device = ?1",
            rusqlite::params![idle, 181 * 86_400i64],
        )
        .unwrap();
    st.db
        .execute(
            "UPDATE device_sessions SET revoked = 1, revoked_at = 1 WHERE device = ?1",
            [&old],
        )
        .unwrap();
    sweep(&mut st).unwrap();
    assert_eq!(
        dsdb::get(&st.db, &idle).unwrap().unwrap().revoked,
        reason::IDLE
    );
    assert!(dsdb::get(&st.db, &old).unwrap().is_none());
    assert!(dsdb::get(&st.db, &fresh).unwrap().unwrap().live());
    // with device sessions off the sweep leaves everything alone
    st.identity.devices.enabled = false;
    st.db
        .execute("UPDATE device_sessions SET revoked = 1, revoked_at = 1", [])
        .unwrap();
    sweep(&mut st).unwrap();
    assert!(dsdb::get(&st.db, &fresh).unwrap().is_some());
}

#[test]
fn the_place_is_masked() {
    assert_eq!(place_of("203.0.113.77:9"), "203.0.113.0/24");
    assert_eq!(place_of("[2001:db8:1:2::5]:9"), "2001:db8:1::/48");
    assert_eq!(place_of("nonsense"), "");
}

// ------------------------------------------------------------------------------------------------ the client flow

/// The client crate's transport straight into the handlers (the web port's two phases for the sign-in).
struct Handlers<'a> {
    st: &'a mut Core,
}

impl dc_session::Transport for Handlers<'_> {
    fn post(&mut self, path: &str, body: &str) -> Result<dc_session::Reply, dc_session::NetError> {
        let (status, body) = if is_sign_in(path) {
            sign_in_at(self.st, body, PEER, Instant::now())
        } else {
            route(self.st, path, "POST", body.as_bytes(), PEER, Instant::now()).unwrap()
        };
        Ok(dc_session::Reply { status, body })
    }
}

#[test]
fn the_client_flow_runs_against_the_real_handlers() {
    use dc_session::{refresh as c_refresh, sign_in as c_sign_in, sign_out as c_sign_out};
    let (mut st, public) = lobby();
    let store = dc_session::MemoryStore::new();
    let info = dc_session::DeviceInfo {
        name: "Desk".into(),
        platform: "windows".into(),
    };
    let auth = "lobby.example:28012";
    // a wrong password keeps nothing
    let r = c_sign_in(
        &store,
        &mut Handlers { st: &mut st },
        auth,
        "alice",
        "nope",
        &info,
    );
    assert_eq!(r.unwrap_err(), dc_session::SignInError::Refused);
    let s = c_sign_in(
        &store,
        &mut Handlers { st: &mut st },
        auth,
        "alice",
        PW,
        &info,
    )
    .unwrap();
    assert!(s.store_error.is_none());
    let unix = crate::clock::unix_now();
    let c = verify(&s.session.token, &public, "dc.main", unix).unwrap();
    assert_eq!(c.login, "alice");
    // two launches: each refresh rotates and gives a working token
    for g in 1..=2 {
        let t = c_refresh(&store, &mut Handlers { st: &mut st }, auth, "alice").unwrap();
        assert!(verify(&t.token, &public, "dc.main", unix).is_ok());
        assert_eq!(dsdb::get(&st.db, &s.device).unwrap().unwrap().generation, g);
    }
    // someone copies the credential and refreshes first; the owner's next refresh is reuse: both are out
    let thief = dc_session::MemoryStore::new();
    use dc_session::CredentialStore;
    thief
        .save(&store.load(auth, "alice").unwrap().unwrap())
        .unwrap();
    c_refresh(&thief, &mut Handlers { st: &mut st }, auth, "alice").unwrap();
    assert_eq!(
        c_refresh(&store, &mut Handlers { st: &mut st }, auth, "alice").unwrap_err(),
        dc_session::RefreshError::SignedOut
    );
    assert!(store.load(auth, "alice").unwrap().is_none());
    assert_eq!(
        dsdb::get(&st.db, &s.device).unwrap().unwrap().revoked,
        reason::REUSE
    );
    assert_eq!(
        c_refresh(&thief, &mut Handlers { st: &mut st }, auth, "alice").unwrap_err(),
        dc_session::RefreshError::SignedOut
    );
    // a new sign-in makes a new device (its token of this same second is under the reuse cut-off of now + 1 s, so
    // it is refused at a lobby login until the next refresh) and a sign-out revokes it
    let s2 = c_sign_in(
        &store,
        &mut Handlers { st: &mut st },
        auth,
        "alice",
        PW,
        &info,
    )
    .unwrap();
    c_sign_out(&store, &mut Handlers { st: &mut st }, auth, "alice").unwrap();
    assert!(store.load(auth, "alice").unwrap().is_none());
    assert_eq!(
        dsdb::get(&st.db, &s2.device).unwrap().unwrap().revoked,
        reason::SIGN_OUT
    );
    // with device sessions off the client hears "not supported" and keeps nothing
    st.identity.devices.enabled = false;
    let r = c_sign_in(
        &store,
        &mut Handlers { st: &mut st },
        auth,
        "alice",
        PW,
        &info,
    );
    assert_eq!(r.unwrap_err(), dc_session::SignInError::NotSupported);
}
