//! Tests of the beta list core: double opt-in, unsubscribe, the limits, the mail budget, invites and their use,
//! the feedback mail, and the one-code-path property of the batch functions. Addresses are `example.com` ones.

use super::*;
use crate::state::testing::state;

const NOW: i64 = 1_793_620_800; // 2026-11-02 12:00:00 UTC

pub(crate) fn beta_state() -> Core {
    let mut st = state();
    st.opts.beta.enabled = true;
    st.opts.beta.public_url = "https://beta.example.com/".into();
    st.opts.beta.download_url = "https://example.com/get".into();
    st.opts.beta.reply_to = "contact@example.com".into();
    st.mail = Mailer::dry_run("[T]");
    // the limits have their own test
    st.opts.beta.ip_per_hour = 1000;
    st.opts.beta.global_per_hour = 1000;
    st
}

/// The 32-hex token after `t=` in a mail body.
pub(crate) fn token_in(body: &str) -> String {
    let i = body.find("?t=").expect("a link with a token") + 3;
    body[i..i + 32].to_string()
}

fn signup_at(st: &mut Core, email: &str, ip: &str, unix: i64) -> BetaResult<()> {
    let mut c = ctx(st, unix);
    signup(&mut c, email, "windows", true, ip)
}

fn row(st: &Core, email: &str) -> Signup {
    by_email(&st.db, email).unwrap().expect("a row")
}

fn status_of(st: &Core, email: &str) -> String {
    row(st, email).status
}

/// A signup made confirmed, by the real path.
fn confirmed(st: &mut Core, email: &str, unix: i64) {
    signup_at(st, email, "198.51.100.1", unix).unwrap();
    let tok = row(st, email).confirm_token;
    let mut c = ctx(st, unix);
    assert_eq!(
        confirm(&mut c, &tok, "198.51.100.1").unwrap(),
        Confirmed::Yes
    );
}

#[test]
fn a_signup_is_pending_until_the_mailed_link_is_opened() {
    let mut st = beta_state();
    signup_at(&mut st, "ann@example.com", "198.51.100.1", NOW).unwrap();
    assert_eq!(status_of(&st, "ann@example.com"), "pending");
    // exactly one confirmation mail, with the link, the unsubscribe link, the headers and the reply address
    assert_eq!(st.mail.dry.len(), 1);
    let m = &st.mail.dry[0];
    assert_eq!(m.to, "ann@example.com");
    assert!(m.subject.starts_with("[T] "), "{}", m.subject);
    let tok = token_in(&m.body);
    assert!(
        m.body
            .contains(&format!("https://beta.example.com/beta/confirm?t={tok}")),
        "{}",
        m.body
    );
    assert!(
        m.body.contains("https://discord.gg/hNWcXpTST"),
        "{}",
        m.body
    );
    let unsub = row(&st, "ann@example.com").unsub_token;
    assert!(m.body.contains(&format!(
        "https://beta.example.com/beta/unsubscribe?t={unsub}"
    )));
    assert!(m.headers.iter().any(|(k, v)| k == "List-Unsubscribe"
        && v == &format!("<https://beta.example.com/beta/unsubscribe?t={unsub}>")));
    assert!(m
        .headers
        .iter()
        .any(|(k, v)| k == "List-Unsubscribe-Post" && v == "List-Unsubscribe=One-Click"));
    assert_eq!(m.reply_to, "contact@example.com");
    // a pending address is not invitable
    let mut c = ctx(&mut st, NOW);
    let r = invite(&mut c, &Pick::Emails(vec!["ann@example.com".into()]), false).unwrap();
    assert!(!r[0].ok && r[0].message.contains("only confirmed"), "{r:?}");
    assert_eq!(c.mail.dry.len(), 1);
    // a wrong link does nothing; the right one confirms (and again, the same)
    assert_eq!(
        confirm(&mut c, &"0".repeat(32), "198.51.100.1").unwrap(),
        Confirmed::No
    );
    assert_eq!(
        confirm(&mut c, "short", "198.51.100.1").unwrap(),
        Confirmed::No
    );
    assert_eq!(
        confirm(&mut c, &tok, "198.51.100.1").unwrap(),
        Confirmed::Yes
    );
    assert_eq!(
        confirm(&mut c, &tok, "198.51.100.1").unwrap(),
        Confirmed::Yes
    );
    assert_eq!(status_of(&st, "ann@example.com"), "confirmed");
    // now it is invitable (a dry run changes nothing)
    let mut c = ctx(&mut st, NOW);
    let r = invite(&mut c, &Pick::Next(5), true).unwrap();
    assert!(r[0].ok && r[0].message.contains("would"), "{r:?}");
    assert_eq!(status_of(&st, "ann@example.com"), "confirmed");
}

#[test]
fn a_repeated_signup_gets_the_same_answer_and_at_most_one_mail_a_day() {
    let mut st = beta_state();
    for _ in 0..3 {
        signup_at(&mut st, "bob@example.com", "198.51.100.1", NOW).unwrap();
    }
    assert_eq!(st.mail.dry.len(), 1, "no second mail within a day");
    // a day later a pending address is mailed again, with the same token
    signup_at(
        &mut st,
        "BOB@example.com",
        "198.51.100.1",
        NOW + RESEND_SECS,
    )
    .unwrap();
    assert_eq!(st.mail.dry.len(), 2);
    assert_eq!(
        token_in(&st.mail.dry[0].body),
        token_in(&st.mail.dry[1].body)
    );
    // a failed confirmation mail is repeated at once
    let id = st
        .db
        .query_row("SELECT MAX(id) FROM beta_mail_log", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    apply_ack(&st.db, id, false);
    signup_at(
        &mut st,
        "bob@example.com",
        "198.51.100.1",
        NOW + RESEND_SECS + 60,
    )
    .unwrap();
    assert_eq!(st.mail.dry.len(), 3);
    // every status answers Ok, a confirmed or invited one sends nothing
    let t = NOW + 2 * RESEND_SECS;
    confirmed(&mut st, "cy@example.com", t);
    let n = st.mail.dry.len();
    signup_at(&mut st, "cy@example.com", "198.51.100.2", t + 5).unwrap();
    {
        let mut c = ctx(&mut st, t + 6);
        invite(&mut c, &Pick::Emails(vec!["cy@example.com".into()]), false).unwrap();
    }
    let n2 = st.mail.dry.len();
    signup_at(&mut st, "cy@example.com", "198.51.100.3", t + 7).unwrap();
    assert_eq!((n2, st.mail.dry.len()), (n + 1, n + 1));
    assert_eq!(status_of(&st, "cy@example.com"), "invited");
}

#[test]
fn bad_input_is_refused_and_pending_rows_expire() {
    let mut st = beta_state();
    let bad = |st: &mut Core, e: &str, consent: bool| {
        let mut c = ctx(st, NOW);
        signup(&mut c, e, "", consent, "198.51.100.9")
    };
    assert!(matches!(bad(&mut st, "", true), Err(BetaErr::Bad(_))));
    assert!(matches!(
        bad(&mut st, "no-at-sign", true),
        Err(BetaErr::Bad(_))
    ));
    assert!(matches!(
        bad(&mut st, "a b@example.com", true),
        Err(BetaErr::Bad(_))
    ));
    assert!(matches!(
        bad(&mut st, &format!("{}@example.com", "x".repeat(260)), true),
        Err(BetaErr::Bad(_))
    ));
    assert!(
        matches!(bad(&mut st, "dee@example.com", false), Err(BetaErr::Bad(m)) if m.contains("Tick the box"))
    );
    assert_eq!(
        st.db
            .query_row("SELECT COUNT(*) FROM beta_signups", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    // an unknown platform is stored as none
    {
        let mut c = ctx(&mut st, NOW);
        signup(&mut c, "dee@example.com", "amiga", true, "198.51.100.9").unwrap();
    }
    assert_eq!(row(&st, "dee@example.com").platform, "");
    // pending rows are dropped after 7 days; a returning address starts again with a fresh token
    let old = row(&st, "dee@example.com");
    signup_at(
        &mut st,
        "eve@example.com",
        "198.51.100.9",
        NOW + PENDING_SECS + 5,
    )
    .unwrap();
    assert!(by_email(&st.db, "dee@example.com").unwrap().is_none());
    let mut c = ctx(&mut st, NOW + PENDING_SECS + 5);
    assert_eq!(
        confirm(&mut c, &old.confirm_token, "198.51.100.9").unwrap(),
        Confirmed::No
    );
    // a confirmed row does not expire
    confirmed(&mut st, "fay@example.com", NOW);
    purge(&st.db, NOW + 400 * 86_400).unwrap();
    assert_eq!(status_of(&st, "fay@example.com"), "confirmed");
}

#[test]
fn unsubscribe_ends_all_mail_and_a_new_signup_needs_a_new_confirmation() {
    let mut st = beta_state();
    confirmed(&mut st, "gus@example.com", NOW);
    {
        let mut c = ctx(&mut st, NOW);
        invite(&mut c, &Pick::Next(1), false).unwrap();
    }
    let sent = st.mail.dry.len();
    let unsub = row(&st, "gus@example.com").unsub_token;
    {
        let mut c = ctx(&mut st, NOW + 1);
        assert!(!unsubscribe(&mut c, "nope", "198.51.100.1").unwrap());
        assert!(!unsubscribe(&mut c, &"f".repeat(32), "198.51.100.1").unwrap());
        assert!(unsubscribe(&mut c, &unsub, "198.51.100.1").unwrap());
        assert!(
            unsubscribe(&mut c, &unsub, "198.51.100.1").unwrap(),
            "one click twice is fine"
        );
    }
    assert_eq!(status_of(&st, "gus@example.com"), "unsubscribed");
    assert_eq!(row(&st, "gus@example.com").platform, "windows");
    // neither an invite nor a feedback mail goes to it
    let mut c = ctx(&mut st, NOW + 2);
    let r = invite(&mut c, &Pick::Emails(vec!["gus@example.com".into()]), false).unwrap();
    assert!(!r[0].ok);
    let r = feedback(
        &mut c,
        &Pick::Status("unsubscribed".into()),
        "s",
        "b",
        false,
    )
    .unwrap();
    assert!(!r[0].ok && r[0].message.contains("unsubscribed"), "{r:?}");
    assert_eq!(c.mail.dry.len(), sent);
    // signing up again gets one confirmation mail; the link brings it back
    let later = NOW + RESEND_SECS + 3;
    signup_at(&mut st, "gus@example.com", "198.51.100.2", later).unwrap();
    assert_eq!(st.mail.dry.len(), sent + 1);
    assert_eq!(
        status_of(&st, "gus@example.com"),
        "unsubscribed",
        "not before the link is opened"
    );
    let tok = token_in(&st.mail.dry[sent].body);
    let mut c = ctx(&mut st, later + 1);
    assert_eq!(
        confirm(&mut c, &tok, "198.51.100.2").unwrap(),
        Confirmed::Yes
    );
    assert_eq!(status_of(&st, "gus@example.com"), "confirmed");
}

#[test]
fn signup_requests_are_limited_per_address_and_in_all() {
    let mut st = beta_state();
    st.opts.beta.ip_per_hour = 2;
    st.opts.beta.global_per_hour = 4;
    let go = |st: &mut Core, n: u32, ip: &str, unix: i64| {
        signup_at(st, &format!("u{n}@example.com"), ip, unix)
    };
    go(&mut st, 1, "198.51.100.1", NOW).unwrap();
    // a refused request (bad form) counts too
    assert!(matches!(
        signup_at(&mut st, "bad", "198.51.100.1", NOW),
        Err(BetaErr::Bad(_))
    ));
    assert!(matches!(
        go(&mut st, 2, "198.51.100.1", NOW + 1),
        Err(BetaErr::Slow)
    ));
    go(&mut st, 3, "198.51.100.2", NOW + 2).unwrap();
    go(&mut st, 4, "198.51.100.3", NOW + 3).unwrap();
    assert!(
        matches!(go(&mut st, 5, "198.51.100.4", NOW + 4), Err(BetaErr::Slow)),
        "the global cap"
    );
    assert!(by_email(&st.db, "u5@example.com").unwrap().is_none());
    // an hour later both windows are open again
    go(&mut st, 6, "198.51.100.1", NOW + 3700).unwrap();
}

#[test]
fn a_batch_over_the_budget_is_refused_whole_and_confirmations_keep_a_reserve() {
    let mut st = beta_state();
    st.opts.beta.max_mail_per_day = 6;
    st.opts.beta.confirm_reserve = 2;
    for n in 0..6 {
        // direct rows: the budget is about the mails, not about how the rows got there
        st.db
            .execute(
                "INSERT INTO beta_signups (email, status, created, confirmed, confirm_token, unsub_token) \
                 VALUES (?1, 'confirmed', ?2, ?2, ?3, ?4)",
                params![format!("t{n}@example.com"), NOW + n, format!("{n:032x}"), format!("{:032x}", n + 100)],
            )
            .unwrap();
    }
    let mut c = ctx(&mut st, NOW + 100);
    let b = budget(&c).unwrap();
    assert_eq!(
        (b.max, b.used, b.remaining(), b.admin_remaining()),
        (6, 0, 6, 4)
    );
    // 5 > 4: nothing is sent, nothing changes
    let e = invite(&mut c, &Pick::Next(5), false).unwrap_err();
    assert!(
        matches!(&e, BetaErr::Budget(m) if m.contains("5 mails") && m.contains("4 may still")),
        "{e:?}"
    );
    assert_eq!(c.mail.dry.len(), 0);
    assert_eq!(counts(&c).unwrap()["confirmed"], 6);
    // a dry run is refused the same way
    assert!(matches!(
        invite(&mut c, &Pick::Next(5), true),
        Err(BetaErr::Budget(_))
    ));
    // 4 go out, in the order the rows were confirmed
    let r = invite(&mut c, &Pick::Next(4), false).unwrap();
    assert_eq!(r.iter().filter(|r| r.ok).count(), 4);
    assert_eq!(
        c.mail.dry.iter().map(|m| m.to.as_str()).collect::<Vec<_>>(),
        [
            "t0@example.com",
            "t1@example.com",
            "t2@example.com",
            "t3@example.com"
        ]
    );
    assert_eq!(budget(&c).unwrap().admin_remaining(), 0);
    assert!(matches!(
        invite(&mut c, &Pick::Next(1), false),
        Err(BetaErr::Budget(_))
    ));
    assert!(matches!(
        feedback(&mut c, &Pick::Status("invited".into()), "s", "b", false),
        Err(BetaErr::Budget(_))
    ));
    // the two reserved mails still go to new signups, then even those stop
    signup(&mut c, "new1@example.com", "", true, "198.51.100.1").unwrap();
    signup(&mut c, "new2@example.com", "", true, "198.51.100.1").unwrap();
    assert_eq!(c.mail.dry.len(), 6);
    signup(&mut c, "new3@example.com", "", true, "198.51.100.1").unwrap();
    assert_eq!(
        c.mail.dry.len(),
        6,
        "the budget is used up: the row waits, the answer is the same"
    );
    assert_eq!(budget(&c).unwrap().used, 6);
    assert_eq!(row_status(&c, "new3@example.com"), "pending");
    // after 24 hours the counter has room again
    let c2 = ctx(&mut st, NOW + 100 + 86_401);
    assert_eq!(budget(&c2).unwrap().used, 0);
}

fn row_status(c: &Ctx, email: &str) -> String {
    by_email(c.db, email).unwrap().unwrap().status
}

#[test]
fn an_invite_is_mailed_and_using_its_code_marks_the_signup_registered() {
    let mut st = beta_state();
    confirmed(&mut st, "hal@example.com", NOW);
    let (id, until) = {
        let mut c = ctx(&mut st, NOW + 10);
        let r = invite(&mut c, &Pick::Next(1), false).unwrap();
        assert_eq!(r.len(), 1);
        assert!(r[0].ok && r[0].message == "queued");
        (r[0].id, utc_text(NOW + 10 + 14 * 86_400))
    };
    let s = row(&st, "hal@example.com");
    assert_eq!((s.status.as_str(), s.invite_code.len()), ("invited", 12));
    let m = st.mail.dry.last().unwrap();
    assert!(
        m.body.contains(&accounts::show_code(&s.invite_code)),
        "{}",
        m.body
    );
    assert!(
        m.body.contains("https://example.com/get")
            && m.body.contains("https://beta.example.com/register")
    );
    assert!(
        m.body.contains("Sign up")
            && m.body.contains(&until[..10])
            && m.body.contains("Unsubscribe with one click")
    );
    // the first-public-beta wording: where to get it, the game files, the first start, how to give feedback, the reset
    for needle in [
        "https://github.com/argallo/DeCombat/releases",
        "Windows and macOS",
        "no iPad build yet",
        "your own copy of the original Combat Arms EU client files (2009)",
        "We don't provide them or link to them",
        "Windows protected your PC",
        "Trojan:Win32/Wacatac.B!ml",
        "SHA256SUMS",
        "Get-FileHash",
        "shasum -a 256",
        "Open Anyway",
        "https://discord.gg/hNWcXpTST",
        "Reply to this mail",
        "/report",
        "accounts and progress may be reset before the full launch",
    ] {
        assert!(m.body.contains(needle), "{needle}: {}", m.body);
    }
    assert!(!m.body.contains('\u{c}'), "a stray escape in the text");
    assert_eq!(
        accounts::invite(&st.db, &s.invite_code, NOW + 11).unwrap(),
        accounts::Invite::Open(None)
    );
    // an invited row is not invited again (its mail did not fail)
    {
        let mut c = ctx(&mut st, NOW + 20);
        let r = invite(&mut c, &Pick::Ids(vec![id]), false).unwrap();
        assert!(!r[0].ok && r[0].message.contains("only confirmed"));
    }
    // the account registers with the code (the one place both the web page and the game's dialog end in)
    let user_no = crate::db::create_account(&st.db, "Halo", NOW + 30).unwrap();
    assert!(
        accounts::consume_invite(&st.db, &s.invite_code.to_lowercase(), user_no, NOW + 30).unwrap()
    );
    let s = row(&st, "hal@example.com");
    assert_eq!(
        (s.status.as_str(), s.user_no),
        ("registered", i64::from(user_no))
    );
    // a code that came from no signup changes no row
    let other = accounts::new_invite(&st.db, &mut st.keys, None, NOW, 7).unwrap();
    let u2 = crate::db::create_account(&st.db, "Other", NOW).unwrap();
    assert!(accounts::consume_invite(&st.db, &other, u2, NOW).unwrap());
    assert_eq!(counts(&ctx(&mut st, NOW)).unwrap()["registered"], 1);
}

#[test]
fn a_failed_invite_mail_can_be_sent_again_with_the_same_code() {
    let mut st = beta_state();
    confirmed(&mut st, "ivy@example.com", NOW);
    {
        let mut c = ctx(&mut st, NOW);
        invite(&mut c, &Pick::Next(1), false).unwrap();
        // the dry run reports success at once
        c.rt.drain(c.db, NOW);
        assert_eq!(last_state(c.db, row_id(c.db), "invite").unwrap(), Some(1));
    }
    let code = row(&st, "ivy@example.com").invite_code;
    let id = st
        .db
        .query_row("SELECT MAX(id) FROM beta_mail_log", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    st.db
        .execute("UPDATE beta_mail_log SET state = 0 WHERE id = ?1", [id])
        .unwrap();
    apply_ack(&st.db, id, false);
    let mut c = ctx(&mut st, NOW + 5);
    let r = invite(&mut c, &Pick::Emails(vec!["ivy@example.com".into()]), false).unwrap();
    assert!(r[0].ok, "{r:?}");
    assert_eq!(
        row(&st, "ivy@example.com").invite_code,
        code,
        "the open code is reused"
    );
    assert_eq!(st.mail.dry.len(), 3);
    // a mail with no answer after a quarter of an hour counts as failed
    let c = ctx(&mut st, NOW + 5 + LOST_SECS + 1);
    c.rt.drain(c.db, NOW + 5);
    st.db
        .execute("UPDATE beta_mail_log SET state = 0", [])
        .unwrap();
    drain(&st.db, &st.beta.rx, NOW + 5 + LOST_SECS + 1);
    assert_eq!(
        st.db
            .query_row(
                "SELECT COUNT(*) FROM beta_mail_log WHERE state = 2",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        3
    );
}

fn row_id(db: &Connection) -> i64 {
    db.query_row("SELECT MAX(id) FROM beta_signups", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn the_feedback_mail_goes_to_invited_and_registered_testers_with_their_names() {
    let mut st = beta_state();
    confirmed(&mut st, "jo@example.com", NOW);
    confirmed(&mut st, "kit@example.com", NOW);
    confirmed(&mut st, "lee@example.com", NOW);
    {
        let mut c = ctx(&mut st, NOW);
        invite(
            &mut c,
            &Pick::Emails(vec!["jo@example.com".into(), "kit@example.com".into()]),
            false,
        )
        .unwrap();
    }
    let user_no = crate::db::create_account(&st.db, "Kitty", NOW).unwrap();
    st.db
        .execute(
            "UPDATE accounts SET last_login = ?2 WHERE user_no = ?1",
            params![user_no, NOW + 50],
        )
        .unwrap();
    st.db
        .execute(
            "UPDATE characters SET matches = 7 WHERE user_no = ?1",
            [user_no],
        )
        .ok();
    let code = row(&st, "kit@example.com").invite_code;
    accounts::consume_invite(&st.db, &code, user_no, NOW + 40).unwrap();
    let before = st.mail.dry.len();
    let mut c = ctx(&mut st, NOW + 60);
    // a dry run changes and sends nothing
    let r = feedback(
        &mut c,
        &Pick::Emails(vec![
            "jo@example.com".into(),
            "kit@example.com".into(),
            "lee@example.com".into(),
            "who@example.com".into(),
        ]),
        "Hi {name}",
        "Dear {name} <{email}>",
        true,
    )
    .unwrap();
    assert_eq!(
        r.iter().map(|r| r.ok).collect::<Vec<_>>(),
        [true, true, false, false]
    );
    assert!(r[3].message.contains("not on the list") && r[2].message.contains("confirmed"));
    assert_eq!(
        (c.mail.dry.len(), budget(&c).unwrap().used),
        (before, 5),
        "3 confirmations and 2 invites"
    );
    let r = feedback(
        &mut c,
        &Pick::Status("invited".into()),
        "How is it {name}?",
        "Hello {name} at {email}.\nReply here.",
        false,
    )
    .unwrap();
    assert_eq!(r.len(), 1);
    let m = c.mail.dry.last().unwrap().clone();
    assert_eq!(
        (m.to.as_str(), m.subject.as_str()),
        ("jo@example.com", "[T] How is it jo?")
    );
    assert!(
        m.body.starts_with("Hello jo at jo@example.com.")
            && m.body.contains("Unsubscribe with one click")
    );
    assert_eq!(m.reply_to, "contact@example.com");
    // a registered tester is greeted with the account's login
    let r = feedback(
        &mut c,
        &Pick::Status("registered".into()),
        "Hi {name}",
        "x",
        false,
    )
    .unwrap();
    assert!(r[0].ok);
    assert_eq!(c.mail.dry.last().unwrap().subject, "[T] Hi Kitty");
    // the admin list shows the account, its last login and the matches
    let rows = list(
        &c,
        &Filter {
            status: Some("registered".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["account"]["login"], "Kitty");
    assert_eq!(rows[0]["account"]["last_login"], NOW + 50);
    assert_eq!(rows[0]["mail"]["kind"], "feedback");
    // empty and oversized texts are refused
    assert!(matches!(
        feedback(&mut c, &Pick::Status("invited".into()), " ", "b", false),
        Err(BetaErr::Bad(_))
    ));
    assert!(matches!(
        feedback(
            &mut c,
            &Pick::Status("invited".into()),
            "s",
            &"x".repeat(BODY_MAX + 1),
            false
        ),
        Err(BetaErr::Bad(_))
    ));
    // a subject cannot carry a header break
    let r = feedback(
        &mut c,
        &Pick::Status("invited".into()),
        "A\r\nBcc: x@example.com",
        "b",
        false,
    )
    .unwrap();
    assert!(r[0].ok);
    assert!(!c.mail.dry.last().unwrap().subject.contains('\n'));
}

#[test]
fn the_list_filters_by_status_platform_date_and_text() {
    let mut st = beta_state();
    for (n, (email, plat, day)) in [
        ("a@example.com", "windows", 0),
        ("b@example.com", "mac", 2),
        ("c@example.com", "windows", 4),
    ]
    .iter()
    .enumerate()
    {
        let mut c = ctx(&mut st, NOW + day * 86_400);
        signup(&mut c, email, plat, true, &format!("198.51.100.{n}")).unwrap();
    }
    let c = ctx(&mut st, NOW + 5 * 86_400);
    let n = |f: Filter| list(&c, &f).unwrap().len();
    assert_eq!(n(Filter::default()), 3);
    assert_eq!(
        n(Filter {
            platform: Some("windows".into()),
            ..Default::default()
        }),
        2
    );
    assert_eq!(
        n(Filter {
            status: Some("confirmed".into()),
            ..Default::default()
        }),
        0
    );
    assert_eq!(
        n(Filter {
            q: Some("b@".into()),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        n(Filter {
            q: Some("%".into()),
            ..Default::default()
        }),
        0,
        "a % is text, not a wildcard"
    );
    assert_eq!(
        n(Filter {
            after: Some(NOW + 86_400),
            ..Default::default()
        }),
        2
    );
    assert_eq!(
        n(Filter {
            before: Some(NOW + 3 * 86_400),
            ..Default::default()
        }),
        2
    );
    assert_eq!(
        n(Filter {
            after: Some(NOW + 86_400),
            before: Some(NOW + 3 * 86_400),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        n(Filter {
            limit: Some(1),
            ..Default::default()
        }),
        1
    );
    assert_eq!(counts(&c).unwrap()["pending"], 3);
}

#[test]
fn helpers_fill_template_day_and_token_shapes() {
    assert_eq!(
        fill("{name}/{email}/{x}/{", "A", "b@example.com"),
        "A/b@example.com/{x}/{"
    );
    // a value is never scanned again
    assert_eq!(fill("{name}", "{email}", "x@example.com"), "{email}");
    let (s, b) = parse_template(&default_template()).unwrap();
    assert_eq!(
        (s.as_str(), b.as_str()),
        (DEFAULT_FEEDBACK_SUBJECT, DEFAULT_FEEDBACK_BODY)
    );
    assert!(parse_template("no subject here\n\nbody").is_err());
    assert!(parse_template("Subject: only a line").is_err());
    assert_eq!(parse_day("1970-01-02"), Some(86_400));
    assert_eq!(parse_day("2026-11-02"), Some(NOW - 12 * 3600));
    assert_eq!(parse_day("1234"), Some(1234));
    assert_eq!(parse_day("2026-13-02"), None);
    assert_eq!(parse_day("soon"), None);
    assert!(
        token_shape(&format!("{}01", "aB3".repeat(10)))
            && !token_shape("xyz")
            && !token_shape(&"g".repeat(32))
    );
    assert!(
        DEFAULT_FEEDBACK_BODY.contains("/report") && DEFAULT_FEEDBACK_BODY.contains("Report a Bug")
    );
}

#[test]
fn nothing_goes_out_when_the_mail_sender_is_off() {
    let mut st = beta_state();
    st.mail = Mailer::off(st.log.clone());
    assert!(matches!(
        signup_at(&mut st, "mo@example.com", "198.51.100.1", NOW),
        Err(BetaErr::NotSetUp)
    ));
    let mut c = ctx(&mut st, NOW);
    assert!(matches!(
        invite(&mut c, &Pick::Next(1), false),
        Err(BetaErr::NotSetUp)
    ));
    assert!(matches!(
        feedback(&mut c, &Pick::Next(1), "s", "b", false),
        Err(BetaErr::NotSetUp)
    ));
}

fn invites_left(st: &Core, code: &str) -> i64 {
    st.db
        .query_row(
            "SELECT COUNT(*) FROM invites WHERE code = ?1",
            [code],
            |r| r.get(0),
        )
        .unwrap()
}

fn log_rows(st: &Core) -> (i64, i64) {
    st.db
        .query_row(
            "SELECT COUNT(*), COUNT(CASE WHEN signup_id != 0 THEN 1 END) FROM beta_mail_log",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

#[test]
fn delete_removes_the_row_and_an_unused_code_but_keeps_a_used_code_and_its_account() {
    let mut st = beta_state();
    for e in ["una@example.com", "usa@example.com", "pen@example.com"] {
        confirmed(&mut st, e, NOW);
    }
    signup_at(&mut st, "new@example.com", "198.51.100.2", NOW).unwrap();
    {
        let mut c = ctx(&mut st, NOW + 10);
        let r = invite(
            &mut c,
            &Pick::Emails(vec!["una@example.com".into(), "usa@example.com".into()]),
            false,
        )
        .unwrap();
        assert!(r.iter().all(|x| x.ok), "{r:?}");
    }
    let unused = row(&st, "una@example.com").invite_code;
    let used = row(&st, "usa@example.com").invite_code;
    let user_no = crate::db::create_account(&st.db, "Usa", NOW + 20).unwrap();
    assert!(accounts::consume_invite(&st.db, &used, user_no, NOW + 20).unwrap());
    let (before_all, _) = log_rows(&st);
    let used_before = budget(&ctx(&mut st, NOW + 30)).unwrap().used;
    assert!(
        before_all >= 6 && used_before == before_all,
        "{before_all} {used_before}"
    );
    let una_id = row(&st, "una@example.com").id;

    // a dry run reports and changes nothing
    {
        let c = ctx(&mut st, NOW + 30);
        let pick = Pick::Emails(vec!["una@example.com".into(), "usa@example.com".into()]);
        let r = delete(&c, &pick, true).unwrap();
        assert!(
            r.iter()
                .all(|x| x.ok && x.message.starts_with("would delete")),
            "{r:?}"
        );
        assert!(
            r[0].message.contains("unused invite code would be deleted"),
            "{r:?}"
        );
    }
    assert_eq!(status_of(&st, "una@example.com"), "invited");
    assert_eq!(invites_left(&st, &unused), 1);
    assert_eq!(log_rows(&st).0, before_all);
    assert_eq!(log_rows(&st).1, before_all);

    // the real run: an unused code goes, a used code and its account stay, an unknown address is reported
    let r = {
        let c = ctx(&mut st, NOW + 40);
        let pick = Pick::Emails(vec![
            "una@example.com".into(),
            "USA@example.com".into(),
            "pen@example.com".into(),
            "nobody@example.com".into(),
        ]);
        delete(&c, &pick, false).unwrap()
    };
    assert_eq!(r.len(), 4, "{r:?}");
    assert!(
        r[0].ok && r[0].message.contains("unused invite code was deleted"),
        "{r:?}"
    );
    assert!(
        r[1].ok && r[1].message.contains("already used") && r[1].message.contains("not deleted"),
        "{r:?}"
    );
    assert!(r[2].ok && r[2].message == "deleted", "{r:?}");
    assert!(!r[3].ok && r[3].message == "not on the list", "{r:?}");
    assert_eq!(invites_left(&st, &unused), 0);
    assert_eq!(invites_left(&st, &used), 1);
    assert!(crate::db::account_by_login(&st.db, "Usa")
        .unwrap()
        .is_some());
    for e in ["una@example.com", "usa@example.com", "pen@example.com"] {
        assert!(by_email(&st.db, e).unwrap().is_none(), "{e}");
    }
    assert!(by_email(&st.db, "new@example.com").unwrap().is_some());
    // the mail log keeps its rows (the budget count) but none points at a deleted signup any more
    let (all, linked) = log_rows(&st);
    assert_eq!(all, before_all);
    assert_eq!(
        linked, 1,
        "only the remaining pending row's confirmation mail is linked"
    );
    assert_eq!(budget(&ctx(&mut st, NOW + 50)).unwrap().used, used_before);
    // ids that no longer exist are reported, and the id is never reused
    let c = ctx(&mut st, NOW + 60);
    let r = delete(&c, &Pick::Ids(vec![una_id]), false).unwrap();
    assert!(!r[0].ok && r[0].message == "no such signup", "{r:?}");
}
