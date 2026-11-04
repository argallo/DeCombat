//! The play-session pause switch: a control file the bot polls about once a second.
//!
//! Own tooling, no 2009 rule. DC-DEFAULT: BOT-59 (proposed). File lines: `pause` / `resume` (every bot) and
//! `pause <name>` / `resume <name>` (one bot, case-insensitive); the last line that applies wins. No file, an
//! unreadable file or no applicable line mean "resume". The path comes from the `DC_BOT_CONTROL` environment
//! variable (see [`ENV`]); without it nothing is polled.

use std::path::PathBuf;

use dc_net_engine::time::{later, Millis};

/// Environment variable naming the control file.
pub const ENV: &str = "DC_BOT_CONTROL";
/// Poll period.
pub const POLL_MS: u32 = 1000;

/// What the file says for the bot `name`.
pub fn parse(text: &str, name: &str) -> bool {
    let mut paused = false;
    for line in text.lines() {
        let mut w = line.split_whitespace();
        let (Some(cmd), who) = (w.next(), w.next()) else {
            continue;
        };
        if who.is_some_and(|n| !n.eq_ignore_ascii_case(name)) {
            continue;
        }
        match cmd.to_ascii_lowercase().as_str() {
            "pause" => paused = true,
            "resume" => paused = false,
            _ => {}
        }
    }
    paused
}

/// The polling state machine.
pub struct Control {
    path: Option<PathBuf>,
    name: String,
    next: Option<Millis>,
    paused: bool,
}

impl Control {
    pub fn new(path: Option<PathBuf>, name: &str) -> Self {
        Control {
            path,
            name: name.to_string(),
            next: None,
            paused: false,
        }
    }

    /// From the environment ([`ENV`]; empty counts as unset).
    pub fn from_env(name: &str) -> Self {
        let p = std::env::var_os(ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Control::new(p, name)
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Reads the file when the poll period is over; returns the new state when it changed.
    pub fn poll(&mut self, now: Millis) -> Option<bool> {
        let path = self.path.as_ref()?;
        if self.next.is_some_and(|t| later(t, now)) {
            return None;
        }
        self.next = Some(now.wrapping_add(POLL_MS));
        let want = std::fs::read_to_string(path).is_ok_and(|t| parse(&t, &self.name));
        (want != self.paused).then(|| {
            self.paused = want;
            want
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_apply_to_all_or_one_bot_last_wins() {
        assert!(!parse("", "Ann"));
        assert!(parse("pause\n", "Ann"));
        assert!(!parse("pause\nresume\n", "Ann"));
        assert!(parse("pause ann\n", "Ann"));
        assert!(!parse("pause bob\n", "Ann"));
        assert!(!parse("pause\nresume ANN\n", "Ann"));
        assert!(parse("pause\nresume bob\n", "Ann"));
    }

    #[test]
    fn the_file_pauses_and_removing_it_resumes_once_a_second() {
        let p = std::env::temp_dir().join(format!("dc_ctl_{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut c = Control::new(Some(p.clone()), "Ann");
        assert_eq!(c.poll(0), None);
        std::fs::write(&p, "pause\n").unwrap();
        assert_eq!(c.poll(500), None, "within the period nothing is read");
        assert_eq!(c.poll(1000), Some(true));
        assert!(c.paused());
        assert_eq!(c.poll(2000), None, "no change, no event");
        std::fs::remove_file(&p).unwrap();
        assert_eq!(c.poll(3000), Some(false));
        assert!(!c.paused());
        assert_eq!(Control::new(None, "Ann").poll(0), None);
    }
}
