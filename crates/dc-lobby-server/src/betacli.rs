//! `dc-lobby-server beta ...`: the operator page's actions on the command line, on the same code (`beta.rs`).
//!
//! ```text
//! dc-lobby-server beta list [--status S] [--platform P] [--before DAY] [--after DAY] [--q TEXT] [--limit N]
//! dc-lobby-server beta invite (--emails a,b | --next N) [--dry-run]
//! dc-lobby-server beta mail --template FILE (--emails a,b | --status S) [--dry-run]
//! dc-lobby-server beta delete (--emails a,b | --ids 1,2) [--dry-run]
//! dc-lobby-server beta template            (prints the default feedback template)
//! ```
//!
//! Any other flag is an ordinary lobby flag (`--config FILE`, `--db FILE`, `--mail-*`): the mail and `[beta]` settings
//! come from the same `server.toml` as the running lobby's. The command sends through its own mail worker and waits
//! for the answers, so the outcome it prints is the mail server's. `--dry-run` changes and sends nothing.
// Provenance: ORIGINAL (DeCombat server tooling)
// DC-DEFAULT: LG-376 the command line shares the page's code path; LG-381 delete

use crate::beta::{self, BetaErr, Ctx, Filter, Pick, RowResult, Runtime};
use crate::logs::Logger;
use crate::mail::Mailer;
use crate::reportmail::utc_text;
use crate::state::KeySource;
use std::sync::Arc;
use std::time::Duration;

const USAGE: &str = "usage: dc-lobby-server beta list [--status S] [--platform P] [--before DAY] [--after DAY] [--q TEXT] [--limit N]\n\
       dc-lobby-server beta invite (--emails a,b | --next N) [--dry-run]\n\
       dc-lobby-server beta mail --template FILE (--emails a,b | --status S) [--dry-run]\n\
       dc-lobby-server beta delete (--emails a,b | --ids 1,2) [--dry-run]\n\
       dc-lobby-server beta template\n\
  (also --config FILE / --db FILE / --mail-* as for the lobby itself; DAY is YYYY-MM-DD)";

/// The beta flags and the rest, which is the lobby's own command line.
#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub sub: String,
    pub status: Option<String>,
    pub platform: Option<String>,
    pub emails: Option<String>,
    pub ids: Option<String>,
    pub next: Option<usize>,
    pub template: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub q: Option<String>,
    pub limit: Option<usize>,
    pub dry_run: bool,
    pub rest: Vec<String>,
}

/// Splits `args` (after `beta`) into the beta flags and the lobby's own flags.
pub fn split_args(args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut i = 0;
    while i < args.len() {
        let raw = &args[i];
        i += 1;
        let Some(flag) = raw.strip_prefix("--") else {
            if a.sub.is_empty() {
                a.sub = raw.clone();
                continue;
            }
            return Err(format!("unexpected argument {raw}"));
        };
        let (name, inline) = match flag.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (flag, None),
        };
        let value = |i: &mut usize| -> Result<String, String> {
            if let Some(v) = &inline {
                return Ok(v.clone());
            }
            match args.get(*i) {
                Some(v) => {
                    *i += 1;
                    Ok(v.clone())
                }
                None => Err(format!("--{name} needs a value")),
            }
        };
        match name {
            "status" => a.status = Some(value(&mut i)?),
            "platform" => a.platform = Some(value(&mut i)?),
            "emails" => a.emails = Some(value(&mut i)?),
            "ids" => a.ids = Some(value(&mut i)?),
            "template" => a.template = Some(value(&mut i)?),
            "before" => a.before = Some(value(&mut i)?),
            "after" => a.after = Some(value(&mut i)?),
            "q" => a.q = Some(value(&mut i)?),
            "next" => {
                a.next = Some(
                    value(&mut i)?
                        .parse()
                        .map_err(|_| "--next needs a number".to_string())?,
                )
            }
            "limit" => {
                a.limit = Some(
                    value(&mut i)?
                        .parse()
                        .map_err(|_| "--limit needs a number".to_string())?,
                )
            }
            "dry-run" => a.dry_run = true,
            _ => {
                // a lobby flag: with its value when the next word is not a flag
                a.rest.push(raw.clone());
                if inline.is_none() && args.get(i).is_some_and(|v| !v.starts_with("--")) {
                    a.rest.push(args[i].clone());
                    i += 1;
                }
            }
        }
    }
    Ok(a)
}

fn emails_of(s: &str) -> Vec<String> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .collect()
}

fn day(unix: i64) -> String {
    utc_text(unix)[..10].to_string()
}

fn state_word(state: i64) -> &'static str {
    match state {
        0 => "queued",
        1 => "sent",
        _ => "FAILED",
    }
}

fn err_text(e: &BetaErr) -> String {
    match e {
        BetaErr::NotSetUp => "mail is not set up: set [mail] in server.toml (or --mail-*)".into(),
        BetaErr::Bad(m) | BetaErr::Budget(m) => m.clone(),
        BetaErr::Slow => "slow down".into(),
        BetaErr::Db(e) => format!("db: {e}"),
    }
}

/// Runs the command; the exit code.
pub fn run(args: &[String], env: &[(String, String)]) -> i32 {
    let a = match split_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("dc-lobby-server beta: {e}\n{USAGE}");
            return 2;
        }
    };
    if a.sub == "template" {
        print!("{}", beta::default_template());
        return 0;
    }
    if !matches!(a.sub.as_str(), "list" | "invite" | "mail" | "delete") {
        eprintln!("{USAGE}");
        return 2;
    }
    let argv = std::iter::once("dc-lobby-server".to_string()).chain(a.rest.iter().cloned());
    let parsed = match crate::cli::parse_full(argv, env) {
        Ok(p) => p,
        Err(crate::cli::CliError::Usage(e)) => {
            let _ = e.print();
            return 2;
        }
        Err(crate::cli::CliError::Config(errs)) => {
            for e in errs {
                eprintln!("dc-lobby-server: config: {e}");
            }
            return 2;
        }
    };
    let opts = parsed.opts;
    let db = match crate::db::open(&opts.db) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot open the DB {}: {e}", opts.db.display());
            return 1;
        }
    };
    let (log, _) = Logger::open(None, None);
    let log = Arc::new(log);
    let mut mail = Mailer::start(&opts.mail, Arc::clone(&log));
    let mut keys = KeySource::new();
    let mut rt = Runtime::new();
    let unix = crate::clock::unix_now();
    let mut c = Ctx {
        db: &db,
        keys: &mut keys,
        mail: &mut mail,
        cfg: &opts.beta,
        rt: &mut rt,
        log: &log,
        unix,
    };

    match a.sub.as_str() {
        "list" => {
            c.rt.drain(c.db, unix);
            let f = Filter {
                status: a.status.clone(),
                platform: a.platform.clone(),
                before: a.before.as_deref().and_then(beta::parse_day),
                after: a.after.as_deref().and_then(beta::parse_day),
                q: a.q.clone(),
                limit: a.limit,
            };
            match (beta::list(&c, &f), beta::counts(&c), beta::budget(&c)) {
                (Ok(rows), Ok(counts), Ok(b)) => {
                    let n = |s: &str| counts[s].as_i64().unwrap_or(0);
                    println!(
                        "{} shown; pending {}, confirmed {}, invited {}, registered {}, unsubscribed {}; mail budget {} of {} used (24 h)",
                        rows.len(),
                        n("pending"),
                        n("confirmed"),
                        n("invited"),
                        n("registered"),
                        n("unsubscribed"),
                        b.used,
                        b.max
                    );
                    for r in rows {
                        let acct = match r["account"].as_object() {
                            Some(o) => format!(
                                "  account {} (last login {}, {} matches)",
                                o["login"].as_str().unwrap_or(""),
                                day(o["last_login"].as_i64().unwrap_or(0)),
                                o["matches"].as_i64().unwrap_or(0)
                            ),
                            None => String::new(),
                        };
                        let mailed = match r["mail"].as_object() {
                            Some(o) => format!(
                                "  last mail {} {}",
                                o["kind"].as_str().unwrap_or(""),
                                state_word(o["state"].as_i64().unwrap_or(0))
                            ),
                            None => String::new(),
                        };
                        println!(
                            "{:>5}  {:<12} {:<8} {}  {}{}{}",
                            r["id"],
                            r["status"].as_str().unwrap_or(""),
                            r["platform"].as_str().unwrap_or("-"),
                            day(r["created"].as_i64().unwrap_or(0)),
                            r["email"].as_str().unwrap_or(""),
                            acct,
                            mailed
                        );
                    }
                    0
                }
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                    eprintln!("db: {e}");
                    1
                }
            }
        }
        "delete" => {
            let pick = match (&a.emails, &a.ids, a.next, &a.status) {
                (Some(e), None, None, None) => Pick::Emails(emails_of(e)),
                (None, Some(i), None, None) => {
                    let ids: Vec<i64> =
                        emails_of(i).iter().filter_map(|x| x.parse().ok()).collect();
                    if ids.len() != emails_of(i).len() {
                        eprintln!("--ids needs numbers (row ids from `beta list`)");
                        return 2;
                    }
                    Pick::Ids(ids)
                }
                _ => {
                    eprintln!("{USAGE}");
                    return 2;
                }
            };
            match beta::delete(&c, &pick, a.dry_run) {
                Ok(results) => {
                    for r in &results {
                        println!("{:>5}  {}  {}", r.id, r.email, r.message);
                    }
                    let n = results.iter().filter(|r| r.ok).count();
                    println!(
                        "{} rows, {} {}{}",
                        results.len(),
                        n,
                        if a.dry_run {
                            "would be deleted"
                        } else {
                            "deleted"
                        },
                        if a.dry_run {
                            " (dry run: nothing was changed)"
                        } else {
                            "; accounts made from a used invite are not deleted"
                        }
                    );
                    i32::from(n != results.len() && !a.dry_run)
                }
                Err(e) => {
                    eprintln!("refused: {}", err_text(&e));
                    1
                }
            }
        }
        sub => {
            let pick = match (sub, &a.emails, a.next, &a.status) {
                (_, Some(e), None, None) => Pick::Emails(emails_of(e)),
                ("invite", None, Some(n), None) => Pick::Next(n),
                ("mail", None, None, Some(s)) => Pick::Status(s.clone()),
                _ => {
                    eprintln!("{USAGE}");
                    return 2;
                }
            };
            let result = if sub == "invite" {
                beta::invite(&mut c, &pick, a.dry_run)
            } else {
                let Some(path) = &a.template else {
                    eprintln!("beta mail needs --template FILE (see `beta template`)");
                    return 2;
                };
                let text = match std::fs::read_to_string(path) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("cannot read {path}: {e}");
                        return 1;
                    }
                };
                let (subject, body) = match beta::parse_template(&text) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("{path}: {e}");
                        return 1;
                    }
                };
                beta::feedback(&mut c, &pick, &subject, &body, a.dry_run)
            };
            let results = match result {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("refused: {}", err_text(&e));
                    return 1;
                }
            };
            report(&mut c, &results, a.dry_run)
        }
    }
}

/// Waits for the mail worker's answers (up to a minute each) and prints one line per row.
fn report(c: &mut Ctx, results: &[RowResult], dry: bool) -> i32 {
    let queued: Vec<i64> = results
        .iter()
        .filter(|r| r.ok && !dry)
        .map(|r| r.id)
        .collect();
    for _ in 0..queued.len() {
        if !c.rt.wait_ack(c.db, Duration::from_secs(60)) {
            break;
        }
    }
    let states = beta::mail_states(c.db, &queued).unwrap_or_default();
    let mut failed = 0;
    for r in results {
        let outcome = match states.get(&r.id) {
            Some((_, s)) if r.ok && !dry => state_word(*s).to_string(),
            _ => r.message.clone(),
        };
        if !r.ok || outcome == "FAILED" {
            failed += 1;
        }
        println!("{:>5}  {}  {}", r.id, r.email, outcome);
    }
    let b = beta::budget(c)
        .map(|b| format!("; mail budget {} of {} used (24 h)", b.used, b.max))
        .unwrap_or_default();
    println!(
        "{} rows, {} not sent{}{}",
        results.len(),
        failed,
        if dry {
            " (dry run: nothing was sent)"
        } else {
            ""
        },
        b
    );
    i32::from(failed > 0 && !dry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn beta_flags_and_lobby_flags_are_split() {
        let a = split_args(&v(&[
            "invite",
            "--next",
            "5",
            "--config",
            "x.toml",
            "--dry-run",
            "--mail-dry-run",
        ]))
        .unwrap();
        assert_eq!(a.sub, "invite");
        assert_eq!(a.next, Some(5));
        assert!(a.dry_run);
        assert_eq!(a.rest, v(&["--config", "x.toml", "--mail-dry-run"]));
        let a = split_args(&v(&[
            "mail",
            "--template=t.txt",
            "--status",
            "invited",
            "--db=d.db",
        ]))
        .unwrap();
        assert_eq!(
            (a.template.as_deref(), a.status.as_deref()),
            (Some("t.txt"), Some("invited"))
        );
        assert_eq!(a.rest, v(&["--db=d.db"]));
        let a = split_args(&v(&["delete", "--ids", "3,4", "--dry-run", "--db=d.db"])).unwrap();
        assert_eq!(
            (a.sub.as_str(), a.ids.as_deref(), a.dry_run),
            ("delete", Some("3,4"), true)
        );
        assert_eq!(a.rest, v(&["--db=d.db"]));
        assert!(split_args(&v(&["list", "--next", "x"])).is_err());
        assert!(split_args(&v(&["list", "extra"])).is_err());
        assert_eq!(
            emails_of("a@example.com, b@example.com c@example.com"),
            v(&["a@example.com", "b@example.com", "c@example.com"])
        );
    }
}
