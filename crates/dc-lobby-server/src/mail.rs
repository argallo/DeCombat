//! Outgoing mail of the password recovery (LG-242, LG-243): one worker thread owns the SMTP connection, so a slow or
//! unreachable mail server never holds the lobby's state lock. `mail.dry_run` writes the mails to the log instead.
//! The SMTP password never reaches a log line or a debug print; a recipient is logged masked (LG-244).
//! The report mails (LG-316, `reportmail.rs`) go through the same worker and tell their outcome over a channel.
// DC-DEFAULT: LG-245 the [mail] keys: STARTTLS on 587 (or any port but 465), TLS from the first byte on 465

use crate::accounts::mask_email;
use crate::logs::Logger;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::Duration;

/// The resolved `[mail]` settings (`server.toml`, `DC_MAIL__*`, `--mail-*`).
#[derive(Clone, Default, PartialEq)]
pub struct MailOpts {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub user: String,
    pub password: String,
    pub from: String,
    pub subject_prefix: String,
    pub dry_run: bool,
    /// LG-316: where the in-game reports go; empty = nowhere (they stay in the DB).
    pub reports_to: String,
    /// LG-316: report mails per hour for the whole lobby.
    pub reports_per_hour: u32,
}

impl std::fmt::Debug for MailOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailOpts")
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("user", &self.user)
            .field(
                "password",
                &if self.password.is_empty() { "" } else { "***" },
            )
            .field("from", &self.from)
            .field("subject_prefix", &self.subject_prefix)
            .field("dry_run", &self.dry_run)
            .field("reports_to", &mask_email(&self.reports_to))
            .field("reports_per_hour", &self.reports_per_hour)
            .finish()
    }
}

impl MailOpts {
    /// Recovery is set up: a mail server is named, or the dry run is on.
    pub fn configured(&self) -> bool {
        self.dry_run || !self.smtp_host.trim().is_empty()
    }

    /// `<prefix> <text>` (no leading space without a prefix).
    pub fn subject(&self, text: &str) -> String {
        let p = self.subject_prefix.trim();
        if p.is_empty() {
            text.to_string()
        } else {
            format!("{p} {text}")
        }
    }
}

/// One mail to send.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub body: String,
    /// What the log may say about it (no code, no address).
    pub what: String,
    /// LG-374: the Reply-To address; empty = none.
    pub reply_to: String,
    /// LG-372: more headers (`List-Unsubscribe`); a name or value with a control character is dropped.
    pub headers: Vec<(String, String)>,
}

/// Where a mail's outcome goes: its tag and whether the server took it.
pub type Ack = (i64, Sender<(i64, bool)>);

enum Sink {
    Off,
    DryRun,
    Smtp(Sender<(Mail, Option<Ack>)>),
}

/// The lobby's mail sender (one per `Core`).
pub struct Mailer {
    pub opts: MailOpts,
    sink: Sink,
    log: Arc<Logger>,
    /// Mails written by the dry run (tests read them).
    #[cfg(test)]
    pub dry: Vec<Mail>,
}

impl Mailer {
    /// No mail: recovery answers "not set up".
    pub fn off(log: Arc<Logger>) -> Mailer {
        Mailer {
            opts: MailOpts::default(),
            sink: Sink::Off,
            log,
            #[cfg(test)]
            dry: Vec::new(),
        }
    }

    /// The sender for `opts`: off, dry run, or the SMTP worker thread.
    pub fn start(opts: &MailOpts, log: Arc<Logger>) -> Mailer {
        let sink = if opts.dry_run {
            Sink::DryRun
        } else if !opts.configured() {
            Sink::Off
        } else {
            let (tx, rx) = channel::<(Mail, Option<Ack>)>();
            let o = opts.clone();
            let l = Arc::clone(&log);
            let spawned = std::thread::Builder::new()
                .name("mail".into())
                .spawn(move || {
                    let mut transport = None;
                    for (m, ack) in rx {
                        let tell = |ok: bool| {
                            if let Some((tag, to)) = &ack {
                                let _ = to.send((*tag, ok));
                            }
                        };
                        if transport.is_none() {
                            match smtp_transport(&o) {
                                Ok(t) => transport = Some(t),
                                Err(e) => {
                                    l.line(&format!("mail: {} not sent: {}", m.what, e));
                                    tell(false);
                                    continue;
                                }
                            }
                        }
                        let t = transport.as_ref().expect("made above");
                        match send_one(t, &o, &m) {
                            Ok(()) => {
                                l.line(&format!("mail: {} sent to {}", m.what, mask_email(&m.to)));
                                tell(true);
                            }
                            Err(e) => {
                                l.line(&format!(
                                    "mail: {} to {} failed: {}",
                                    m.what,
                                    mask_email(&m.to),
                                    e
                                ));
                                tell(false);
                            }
                        }
                    }
                });
            match spawned {
                Ok(_) => Sink::Smtp(tx),
                Err(e) => {
                    log.line(&format!("mail: the mail thread could not start: {e}"));
                    Sink::Off
                }
            }
        };
        Mailer {
            opts: opts.clone(),
            sink,
            log,
            #[cfg(test)]
            dry: Vec::new(),
        }
    }

    /// A dry-run sender (tests).
    #[cfg(test)]
    pub fn dry_run(prefix: &str) -> Mailer {
        let opts = MailOpts {
            dry_run: true,
            subject_prefix: prefix.into(),
            from: "noreply@test.invalid".into(),
            ..Default::default()
        };
        Mailer::start(&opts, Arc::new(Logger::none()))
    }

    /// Recovery is set up.
    pub fn enabled(&self) -> bool {
        !matches!(self.sink, Sink::Off)
    }

    /// The start-up line (no password, no address of a player).
    pub fn describe(&self) -> String {
        match &self.sink {
            Sink::Off => {
                "mail: not set up (no mail.smtp_host): password and ID recovery are off".into()
            }
            Sink::DryRun => "mail: dry run (mails are written to this log, not sent)".into(),
            Sink::Smtp(_) => {
                format!(
                    "mail: {}:{} ({}), sender {}",
                    self.opts.smtp_host,
                    self.opts.smtp_port,
                    if self.opts.smtp_port == 465 {
                        "TLS"
                    } else {
                        "STARTTLS"
                    },
                    self.opts.from
                ) + &if self.opts.reports_to.trim().is_empty() {
                    String::new()
                } else {
                    format!("; reports to {}", mask_email(&self.opts.reports_to))
                }
            }
        }
    }

    /// Hands one mail to the sender; never blocks on the network. False when mail is off.
    pub fn send(&mut self, m: Mail) -> bool {
        self.send_with(m, None)
    }

    /// [`Mailer::send`] that reports the outcome to `ack`; a dry run counts as taken at once. False (nothing will
    /// come on `ack`) when mail is off.
    pub fn send_with(&mut self, m: Mail, ack: Option<Ack>) -> bool {
        match &self.sink {
            Sink::Off => false,
            Sink::DryRun => {
                // LG-243: the dry run is the one place the mail text (and so a code) reaches the log
                self.log.line(&format!(
                    "mail (dry run): to {} subject {:?}",
                    mask_email(&m.to),
                    m.subject
                ));
                for l in m.body.lines() {
                    self.log.line(&format!("mail (dry run): | {l}"));
                }
                #[cfg(test)]
                self.dry.push(m);
                if let Some((tag, to)) = ack {
                    let _ = to.send((tag, true));
                }
                true
            }
            Sink::Smtp(tx) => {
                if tx.send((m, ack)).is_err() {
                    self.log.line("mail: the mail thread has stopped");
                    return false;
                }
                true
            }
        }
    }
}

fn smtp_transport(o: &MailOpts) -> Result<lettre::SmtpTransport, String> {
    use lettre::transport::smtp::authentication::Credentials;
    let host = o.smtp_host.trim();
    // the unit tests' fake mail server speaks no TLS
    #[cfg(test)]
    if host == "127.0.0.1" && o.smtp_port != 9 {
        return Ok(lettre::SmtpTransport::builder_dangerous(host)
            .port(o.smtp_port)
            .timeout(Some(Duration::from_secs(5)))
            .build());
    }
    let b = if o.smtp_port == 465 {
        lettre::SmtpTransport::relay(host)
    } else {
        lettre::SmtpTransport::starttls_relay(host)
    }
    .map_err(|e| format!("mail server {host}: {e}"))?
    .port(o.smtp_port)
    .timeout(Some(Duration::from_secs(20)));
    let b = if o.user.is_empty() {
        b
    } else {
        b.credentials(Credentials::new(o.user.clone(), o.password.clone()))
    };
    Ok(b.build())
}

fn send_one(t: &lettre::SmtpTransport, o: &MailOpts, m: &Mail) -> Result<(), String> {
    use lettre::message::header::ContentType;
    use lettre::Transport;
    let from = o
        .from
        .parse()
        .map_err(|e| format!("mail.from is not an address: {e}"))?;
    let to =
        m.to.parse()
            .map_err(|_| "the recipient is not an address the mail library takes".to_string())?;
    let mut b = lettre::Message::builder()
        .from(from)
        .to(to)
        .subject(m.subject.clone());
    if !m.reply_to.trim().is_empty() {
        b =
            b.reply_to(m.reply_to.trim().parse().map_err(|_| {
                "the Reply-To is not an address the mail library takes".to_string()
            })?);
    }
    for (k, v) in &m.headers {
        let plain = |s: &str| s.chars().all(|c| c.is_ascii() && !c.is_control());
        if !plain(k) || !plain(v) {
            continue;
        }
        if let Ok(name) = lettre::message::header::HeaderName::new_from_ascii(k.clone()) {
            b = b.raw_header(lettre::message::header::HeaderValue::new(name, v.clone()));
        }
    }
    let msg = b
        .header(ContentType::TEXT_PLAIN)
        .body(m.body.clone())
        .map_err(|e| e.to_string())?;
    t.send(&msg).map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_password_is_never_in_a_debug_print_and_off_means_not_set_up() {
        let o = MailOpts {
            smtp_host: "smtp.example.org".into(),
            smtp_port: 587,
            user: "lobby".into(),
            password: "hunter2-smtp".into(),
            from: "noreply@example.org".into(),
            subject_prefix: "[X]".into(),
            dry_run: false,
            reports_to: String::new(),
            reports_per_hour: 20,
        };
        assert!(!format!("{o:?}").contains("hunter2"));
        assert!(o.configured());
        assert_eq!(o.subject("your login IDs"), "[X] your login IDs");
        let m = Mailer::off(Arc::new(Logger::none()));
        assert!(!m.enabled() && !m.opts.configured());
        let mut d = Mailer::dry_run("");
        assert!(d.enabled());
        assert_eq!(d.opts.subject("a"), "a");
        assert!(d.send(Mail {
            to: "a@b.cd".into(),
            subject: "s".into(),
            body: "b".into(),
            what: "w".into(),
            ..Default::default()
        }));
        assert_eq!(d.dry.len(), 1);
        // an SMTP sender only queues: an unreachable server costs the caller nothing
        let mut s = Mailer::start(
            &MailOpts {
                smtp_host: "127.0.0.1".into(),
                smtp_port: 9,
                from: "noreply@example.org".into(),
                ..Default::default()
            },
            Arc::new(Logger::none()),
        );
        let t0 = std::time::Instant::now();
        assert!(s.send(Mail {
            to: "a@b.cd".into(),
            subject: "s".into(),
            body: "b".into(),
            what: "w".into(),
            ..Default::default()
        }));
        assert!(t0.elapsed() < Duration::from_millis(500));
        assert!(s.describe().contains("STARTTLS") && !s.describe().contains("hunter"));
    }
}
