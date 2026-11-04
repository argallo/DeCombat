//! The in-game reports by mail (LG-316): every report stays in the `reports` table first; this sends the rows waiting
//! there to `mail.reports_to`, one at a time, through the mail worker, so the game loop never touches the network. A
//! failed send leaves the row waiting and the next try comes after [`RETRY_SECS`], also after a restart. Mails per
//! hour are capped for the whole lobby (the sent rows are the count); what is over the cap waits. `reports.mailed`: 0
//! waiting, 1 sent, 2 not to be mailed (stored with the mail off, or before this column existed).
//! No player text reaches a mail header: the subject is fixed words and the row id.
// DC-DEFAULT: LG-316 (proposed, USER REQUEST 2026-11-02), see DC_DEFAULTS.md

use crate::clock;
use crate::mail::Mail;
use crate::state::{Core, HResult, Sid};
use rusqlite::{params, OptionalExtension};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

/// How often the waiting rows are looked at.
pub const PUMP_EVERY: Duration = Duration::from_secs(15);
/// Pause after a failed send.
pub const RETRY_SECS: i64 = 300;
/// A send without an answer after this long counts as failed (the worker gives up long before).
const LOST_SECS: i64 = 180;
/// Longest mail body (characters); the packet's own limits are below it, the fence is here for the mail.
pub const BODY_MAX: usize = 6000;

/// The sends on their way.
pub struct ReportMail {
    tx: Sender<(i64, bool)>,
    rx: Receiver<(i64, bool)>,
    /// The row being sent and when it left.
    inflight: Option<(i64, i64)>,
    next_try: i64,
    cap_noted: bool,
}

impl ReportMail {
    pub fn new() -> ReportMail {
        let (tx, rx) = channel();
        ReportMail {
            tx,
            rx,
            inflight: None,
            next_try: 0,
            cap_noted: false,
        }
    }
}

/// Reports go by mail on this lobby.
pub fn enabled(st: &Core) -> bool {
    !st.mail.opts.reports_to.trim().is_empty() && st.mail.enabled()
}

/// Where the reporter is: the entry, channel and room of the session.
pub fn place_of(st: &Core, sid: Sid) -> String {
    let Some(s) = st.sessions.get(&sid) else {
        return String::new();
    };
    let entry = crate::servers::entries(st)
        .into_iter()
        .find(|e| e.id == s.server || (s.server == 0 && s.channel.is_some_and(|c| e.owns(c))));
    let mut out = match entry {
        Some(e) => format!("entry {} ({})", e.id, one_line(&e.name)),
        None => "no entry".to_string(),
    };
    if let Some(c) = s.channel {
        out.push_str(&format!(", channel {c}"));
    }
    if let Some(r) = s.room {
        out.push_str(&format!(", room {}", r.no));
    }
    out
}

/// A text without what could end a log line or spoof a mail's look: no control characters, no direction overrides.
fn clean(s: &str, keep_lines: bool) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .map(|c| match c {
            '\n' | '\t' if keep_lines => c,
            c if c.is_control() => ' ',
            c => c,
        })
        .collect()
}

/// `s` as one line.
pub fn one_line(s: &str) -> String {
    clean(s, false)
}

/// Our words for the box's first list (the report box's general kinds).
// EVIDENCE: cshell+0x273080 (three general kinds: player abuse, bug, comment)
pub fn category_name(category: i64) -> &'static str {
    match category {
        0 => "Player report",
        1 => "Bug report",
        2 => "Comment",
        _ => "Report",
    }
}

/// `YYYY-MM-DD HH:MM:SS` of a unix time (UTC).
pub fn utc_text(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // civil date from a day count (the public algorithm of Howard Hinnant's date notes)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// One stored report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    pub id: i64,
    pub form: i64,
    pub category: i64,
    pub sub: i64,
    pub target: String,
    pub text: String,
    pub extra: String,
    pub reporter: i64,
    pub login: String,
    pub nick: String,
    pub place: String,
    pub created: i64,
}

/// The mail's subject: fixed words and the id.
pub fn subject(r: &Row) -> String {
    format!("{} #{}", category_name(r.category), r.id)
}

/// The mail's body, at most [`BODY_MAX`] characters.
pub fn body(r: &Row) -> String {
    let target = if r.target.trim().is_empty() {
        "-".to_string()
    } else {
        one_line(&r.target)
    };
    let mut out = format!(
        "{} #{}\n\nKind: {} (category {}, detail {}, form {})\nTarget: {}\nReporter: account {} login {} nick {}\nWhere: {}\nClient version: not known (the report frame carries none); lobby {}\nTime (UTC): {}\n\nText:\n{}\n",
        category_name(r.category),
        r.id,
        category_name(r.category),
        r.category,
        r.sub,
        r.form,
        target,
        r.reporter,
        one_line(&r.login),
        one_line(&r.nick),
        one_line(&r.place),
        env!("CARGO_PKG_VERSION"),
        utc_text(r.created),
        clean(&r.text, true),
    );
    if !r.extra.is_empty() {
        out.push_str(&format!("\nExtra:\n{}\n", clean(&r.extra, true)));
    }
    if out.chars().count() > BODY_MAX {
        out = out.chars().take(BODY_MAX).collect();
        out.push_str("\n[cut]\n");
    }
    out
}

/// Takes the worker's answers, then hands the oldest waiting row to it unless one is on its way, the last send failed
/// a moment ago or the hour's mails are used up. Never waits on the network.
pub fn pump(st: &mut Core) -> HResult {
    if !enabled(st) {
        return Ok(());
    }
    let now = clock::unix_now();
    while let Ok((id, ok)) = st.report_mail.rx.try_recv() {
        if st.report_mail.inflight.map(|x| x.0) == Some(id) {
            st.report_mail.inflight = None;
        }
        if ok {
            st.db.execute(
                "UPDATE reports SET mailed = 1, mailed_at = ?1 WHERE id = ?2",
                params![now, id],
            )?;
        } else {
            st.report_mail.next_try = now + RETRY_SECS;
            st.note(&format!(
                "report #{id}: the mail failed; it stays in the table, next try in {RETRY_SECS} s"
            ));
        }
    }
    if let Some((id, since)) = st.report_mail.inflight {
        if now - since < LOST_SECS {
            return Ok(());
        }
        st.note(&format!(
            "report #{id}: no answer from the mail thread; will retry"
        ));
        st.report_mail.inflight = None;
        st.report_mail.next_try = now + RETRY_SECS;
    }
    if now < st.report_mail.next_try {
        return Ok(());
    }
    let cap = st.mail.opts.reports_per_hour as i64;
    let sent: i64 = st.db.query_row(
        "SELECT COUNT(*) FROM reports WHERE mailed = 1 AND mailed_at > ?1",
        [now - 3600],
        |r| r.get(0),
    )?;
    if sent >= cap {
        if !st.report_mail.cap_noted {
            st.report_mail.cap_noted = true;
            st.note(&format!(
                "report mails: hourly cap {cap} reached; the rest wait in the table"
            ));
        }
        return Ok(());
    }
    st.report_mail.cap_noted = false;
    let row = st
        .db
        .query_row(
            "SELECT id, kind, category, sub, target_nick, text, extra, reporter, reporter_login, reporter_nick, place, created \
             FROM reports WHERE mailed = 0 ORDER BY id LIMIT 1",
            [],
            |r| {
                Ok(Row {
                    id: r.get(0)?,
                    form: r.get(1)?,
                    category: r.get(2)?,
                    sub: r.get(3)?,
                    target: r.get(4)?,
                    text: r.get(5)?,
                    extra: r.get(6)?,
                    reporter: r.get(7)?,
                    login: r.get(8)?,
                    nick: r.get(9)?,
                    place: r.get(10)?,
                    created: r.get(11)?,
                })
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(());
    };
    let mail = Mail {
        to: st.mail.opts.reports_to.trim().to_string(),
        subject: st.mail.opts.subject(&subject(&row)),
        body: body(&row),
        what: format!("report #{}", row.id),
        ..Default::default()
    };
    let ack = (row.id, st.report_mail.tx.clone());
    if st.mail.send_with(mail, Some(ack)) {
        st.report_mail.inflight = Some((row.id, now));
    } else {
        st.report_mail.next_try = now + RETRY_SECS;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::Listener;
    use crate::mail::{MailOpts, Mailer};
    use crate::state::testing::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn player(
        st: &mut Core,
        sid: Sid,
        login: &str,
    ) -> (std::sync::mpsc::Receiver<crate::state::Outgoing>, u32) {
        let acc = crate::db::create_account(&st.db, login, 0).unwrap();
        st.scope(|st| crate::auth::create_character(st, acc, login, 2))
            .unwrap();
        let rx = connect(st, sid, Listener::Lobby);
        let s = st.sessions.get_mut(&sid).unwrap();
        s.account = Some(acc);
        s.in_lobby = true;
        s.channel = Some(1);
        (rx, acc)
    }

    /// The client's own frame, through the lobby's router.
    fn client_report(st: &mut Core, sid: Sid, name: &str, text: &str, category: u32, detail: u32) {
        let p = dc_wire::report::packet(name, text, category, detail).into_payload();
        st.scope(|st| crate::net::route(st, sid, crate::notices::PLAYER_REPORT as i32, &p))
            .unwrap();
    }

    fn state_of(st: &Core, id: i64) -> i64 {
        st.db
            .query_row("SELECT mailed FROM reports WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    }

    fn wait_pump(st: &mut Core, want: impl Fn(&Core) -> bool) {
        let end = std::time::Instant::now() + Duration::from_secs(15);
        while !want(st) {
            assert!(std::time::Instant::now() < end, "timed out");
            std::thread::sleep(Duration::from_millis(20));
            st.scope(pump).unwrap();
        }
    }

    /// A mail server that speaks just enough SMTP and refuses the first `refuse` messages at DATA's end.
    fn fake_smtp(refuse: usize) -> (u16, Arc<Mutex<Vec<String>>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let got = Arc::new(Mutex::new(Vec::new()));
        let out = Arc::clone(&got);
        std::thread::spawn(move || {
            let mut left = refuse;
            for s in l.incoming().flatten() {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                let _ = w.write_all(b"220 fake ESMTP\r\n");
                let mut line = String::new();
                loop {
                    line.clear();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let cmd = line.to_ascii_uppercase();
                    if cmd.starts_with("DATA") {
                        let _ = w.write_all(b"354 go\r\n");
                        let mut msg = String::new();
                        loop {
                            line.clear();
                            if r.read_line(&mut line).unwrap_or(0) == 0 || line == ".\r\n" {
                                break;
                            }
                            msg.push_str(&line);
                        }
                        if left > 0 {
                            left -= 1;
                            let _ = w.write_all(b"451 try later\r\n");
                        } else {
                            out.lock().unwrap().push(msg);
                            let _ = w.write_all(b"250 ok\r\n");
                        }
                    } else if cmd.starts_with("QUIT") {
                        let _ = w.write_all(b"221 bye\r\n");
                        break;
                    } else {
                        let _ = w.write_all(b"250 ok\r\n");
                    }
                }
            }
        });
        (port, got)
    }

    fn smtp_mailer(port: u16, cap: u32) -> Mailer {
        Mailer::start(
            &MailOpts {
                smtp_host: "127.0.0.1".into(),
                smtp_port: port,
                from: "noreply@test.invalid".into(),
                subject_prefix: "[T]".into(),
                reports_to: "owner@test.invalid".into(),
                reports_per_hour: cap,
                ..Default::default()
            },
            Arc::new(crate::logs::Logger::none()),
        )
    }

    #[test]
    fn a_client_report_reaches_the_table_and_the_mail_after_a_failed_try() {
        let mut st = state();
        let (port, got) = fake_smtp(1);
        st.mail = smtp_mailer(port, 20);
        let (_rx, acc) = player(&mut st, 7, "zoe");
        // a line break and a header in the text must stay body text
        client_report(
            &mut st,
            7,
            "",
            "Bcc: evil@test.invalid\r\nSubject: spam\nthe shop freezes",
            1,
            2,
        );
        let row: (i64, i64, i64, String, String) = st
            .db
            .query_row(
                "SELECT category, sub, reporter, reporter_nick, place FROM reports",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((row.0, row.1, row.2), (1, 2, acc as i64));
        assert_eq!(row.3, "zoe");
        assert!(row.4.contains("channel 1"), "{}", row.4);
        // the first try fails: the row waits, the pause holds it back
        wait_pump(&mut st, |st| st.report_mail.next_try > 0);
        assert_eq!(state_of(&st, 1), 0);
        assert!(got.lock().unwrap().is_empty());
        st.scope(pump).unwrap();
        assert!(st.report_mail.inflight.is_none(), "paused");
        // the pause over, it goes out
        st.report_mail.next_try = 0;
        wait_pump(&mut st, |st| state_of(st, 1) == 1);
        let mails = got.lock().unwrap().clone();
        assert_eq!(mails.len(), 1);
        let m = &mails[0];
        let (head, body) = m.split_once("\r\n\r\n").unwrap();
        assert!(head.contains("Subject: [T] Bug report #1"), "{head}");
        assert!(!head.contains("evil") && !head.contains("spam"), "{head}");
        assert!(
            body.contains("the shop freezes") && body.contains("zoe"),
            "{body}"
        );
        assert!(
            body.contains("Bug report") && body.contains("channel 1"),
            "{body}"
        );
    }

    #[test]
    fn the_hourly_cap_holds_the_rest_back_without_losing_them() {
        let mut st = state();
        st.mail = Mailer::start(
            &MailOpts {
                dry_run: true,
                reports_to: "owner@test.invalid".into(),
                reports_per_hour: 2,
                ..Default::default()
            },
            Arc::new(crate::logs::Logger::none()),
        );
        let (_rx, _) = player(&mut st, 7, "zoe");
        for i in 0..4 {
            client_report(&mut st, 7, "Bot1", &format!("r{i}"), 0, 0);
            // the dry run answers at once
            st.scope(pump).unwrap();
            st.scope(pump).unwrap();
        }
        let n = |st: &Core, m: i64| -> i64 {
            st.db
                .query_row("SELECT COUNT(*) FROM reports WHERE mailed = ?1", [m], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        assert_eq!((n(&st, 1), n(&st, 0)), (2, 2));
        assert_eq!(st.mail.dry.len(), 2);
        // an hour later the next ones go
        st.db
            .execute("UPDATE reports SET mailed_at = mailed_at - 4000", [])
            .unwrap();
        st.scope(pump).unwrap();
        st.scope(pump).unwrap();
        assert_eq!(n(&st, 1), 3);
        assert_eq!(st.mail.dry[2].subject, "Player report #3");
    }

    #[test]
    fn mail_off_stores_only_and_is_not_mailed_when_turned_on_later() {
        let mut st = state();
        let (_rx, _) = player(&mut st, 7, "zoe");
        client_report(&mut st, 7, "Bot1", "wall hack", 0, 3);
        assert_eq!(state_of(&st, 1), 2);
        st.mail = Mailer::start(
            &MailOpts {
                dry_run: true,
                reports_to: "owner@test.invalid".into(),
                reports_per_hour: 2,
                ..Default::default()
            },
            Arc::new(crate::logs::Logger::none()),
        );
        st.scope(pump).unwrap();
        assert!(st.mail.dry.is_empty());
    }

    #[test]
    fn the_body_is_capped_and_cleaned_and_the_time_is_utc() {
        let r = Row {
            id: 9,
            category: 1,
            target: "a\r\nBcc: x".into(),
            text: "y\u{202e}z\u{7}\n".repeat(5000),
            created: 1_793_620_800,
            ..Default::default()
        };
        let b = body(&r);
        assert!(b.chars().count() <= BODY_MAX + 8 && b.ends_with("[cut]\n"));
        assert!(!b.contains('\u{202e}') && !b.contains('\u{7}'));
        assert!(b.contains("Target: a  Bcc: x"));
        assert_eq!(subject(&r), "Bug report #9");
        assert_eq!(utc_text(0), "1970-01-01 00:00:00");
        assert_eq!(utc_text(1_772_323_199), "2026-02-28 23:59:59");
        assert_eq!(utc_text(1_772_323_200), "2026-03-01 00:00:00");
    }
}
