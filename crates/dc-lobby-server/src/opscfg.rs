//! Operator rules of the GM tools, notices, reports and the messenger relay. Every value is a DeCombat default
//! (`DC_DEFAULTS.md` CD-07 / CD-33 / CD-34 / CD-42 / CD-43 / CD-64 and the OPS-n proposals staged in
//! `DC_DEFAULTS.md`, cited "(proposed)"), never a 2009 server value.
//! Overrides use the flat `lobby.json` of `config.rs`; `Rules::apply` forwards the keys below here.

use dc_mod_lua::JsonValue;

/// Who may use the test chat commands `/notice`, `/expire`, `/shopver`, `/grade`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestCommands {
    /// GM accounts only.
    Gm,
    /// Everyone.
    All,
    /// Everyone while every listener is bound to a loopback address (a one-machine test server), else GM only.
    Loopback,
}

/// Which supply case awards are announced with 0x3013.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseAnnounce {
    Off,
    Permanent,
    All,
}

#[derive(Clone, Debug)]
pub struct OpsRules {
    // DC-DEFAULT: OPS-1 (proposed) test chat commands: GM always, everyone on a loopback-only server
    pub test_commands: TestCommands,
    // DC-DEFAULT: OPS-2 (proposed) operator ticker 0x3052: scroll period and repeat count
    pub ticker_period_secs: u32,
    pub ticker_repeat: u32,
    // DC-DEFAULT: OPS-4 (proposed) mute length when 0x3d4 carries no minutes
    pub default_mute_minutes: i64,
    // DC-DEFAULT: OPS-4 (proposed) upper bound of one mute (minutes)
    pub max_mute_minutes: i64,
    // DC-DEFAULT: OPS-3 (proposed) day count 0x201 +0x2a reports for a permanent ban
    pub permanent_ban_days: u32,
    // DC-DEFAULT: OPS-5 (proposed) 0x3d3 +0x56 shows the target's address to the operator
    pub lookup_shows_address: bool,
    // DC-DEFAULT: OPS-7 (proposed) at most N stored reports per reporter per hour
    pub reports_per_hour: u32,
    // DC-DEFAULT: OPS-8 (proposed) messenger: friend list size, whisper scope, invite
    pub max_friends: u32,
    pub whisper_friends_only: bool,
    // DC-DEFAULT: OPS-9 (proposed) supply case awards announced with 0x3013
    pub case_announce: CaseAnnounce,
    // DC-DEFAULT: OPS-10 (proposed) operator texts shown at every lobby login (empty = none)
    pub login_ticker: String,
    pub login_box: String,
    // DC-DEFAULT: OPS-12 (proposed) the periodic expiry scan also sends 0x3011 boxes (CD-42 plain notices)
    pub expiry_boxes: bool,
    // DC-DEFAULT: CD-64 moderators are flagged to the game server (pipe `user` line token `mod=1`)
    pub flag_moderators_to_game: bool,
    // DC-DEFAULT: MON-2 (proposed) days the operator monitor keeps its rows (0 = forever)
    pub monitor_retention_days: u32,
    // DC-DEFAULT: OPS-16 (proposed) absolute path of the public match board snapshot (empty = off)
    pub monitor_public_games_file: String,
    // DC-DEFAULT: LG-395 (proposed) the most one dashboard change may add or take: NX, GP, EXP
    pub admin_max_nx: i64,
    pub admin_max_gp: i64,
    pub admin_max_exp: i64,
}

impl Default for OpsRules {
    fn default() -> Self {
        Self {
            test_commands: TestCommands::Loopback,
            ticker_period_secs: 30,
            ticker_repeat: 3,
            default_mute_minutes: 60,
            max_mute_minutes: 60 * 24 * 365,
            permanent_ban_days: 0,
            lookup_shows_address: true,
            reports_per_hour: 20,
            max_friends: 50,
            whisper_friends_only: false,
            case_announce: CaseAnnounce::Permanent,
            login_ticker: String::new(),
            login_box: String::new(),
            expiry_boxes: true,
            flag_moderators_to_game: true,
            monitor_retention_days: 14,
            monitor_public_games_file: String::new(),
            admin_max_nx: 100_000,
            admin_max_gp: 1_000_000,
            admin_max_exp: 10_000_000,
        }
    }
}

fn num(v: &JsonValue) -> Option<f64> {
    match v {
        JsonValue::Number(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}

impl OpsRules {
    /// Keys handled here.
    pub fn owns(key: &str) -> bool {
        key.starts_with("gm.")
            || key.starts_with("notice.")
            || key.starts_with("messenger.")
            || key == "reports.per_hour"
            || key == "monitor.retention_days"
            || key == "monitor.public_games_file"
    }

    /// Applies one override; false for an unknown key or a value of the wrong type.
    pub fn apply(&mut self, key: &str, v: &JsonValue) -> bool {
        let n = num(v);
        let b = match v {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        };
        let s = match v {
            JsonValue::String(s) => Some(s.clone()),
            _ => None,
        };
        match key {
            "gm.test_commands" => match s.as_deref() {
                Some("gm") => {
                    self.test_commands = TestCommands::Gm;
                    true
                }
                Some("all") => {
                    self.test_commands = TestCommands::All;
                    true
                }
                Some("loopback") => {
                    self.test_commands = TestCommands::Loopback;
                    true
                }
                _ => false,
            },
            "gm.default_mute_minutes" => n
                .map(|x| self.default_mute_minutes = x.clamp(1.0, 1e9) as i64)
                .is_some(),
            "gm.max_mute_minutes" => n
                .map(|x| self.max_mute_minutes = x.clamp(1.0, 1e9) as i64)
                .is_some(),
            "gm.permanent_ban_days" => n
                .map(|x| self.permanent_ban_days = x.clamp(0.0, 1e6) as u32)
                .is_some(),
            "gm.lookup_shows_address" => b.map(|x| self.lookup_shows_address = x).is_some(),
            "gm.flag_moderators_to_game" => b.map(|x| self.flag_moderators_to_game = x).is_some(),
            "gm.admin_max_nx" => n
                .map(|x| self.admin_max_nx = x.clamp(0.0, 1e9) as i64)
                .is_some(),
            "gm.admin_max_gp" => n
                .map(|x| self.admin_max_gp = x.clamp(0.0, 1e10) as i64)
                .is_some(),
            "gm.admin_max_exp" => n
                .map(|x| self.admin_max_exp = x.clamp(0.0, 1e10) as i64)
                .is_some(),
            "notice.ticker_period_secs" => n
                .map(|x| self.ticker_period_secs = x.clamp(1.0, 3600.0) as u32)
                .is_some(),
            "notice.ticker_repeat" => n
                .map(|x| self.ticker_repeat = x.clamp(1.0, 100.0) as u32)
                .is_some(),
            "notice.case_announce" => match s.as_deref() {
                Some("off") => {
                    self.case_announce = CaseAnnounce::Off;
                    true
                }
                Some("permanent") => {
                    self.case_announce = CaseAnnounce::Permanent;
                    true
                }
                Some("all") => {
                    self.case_announce = CaseAnnounce::All;
                    true
                }
                _ => false,
            },
            "notice.login_ticker" => s.map(|x| self.login_ticker = x).is_some(),
            "notice.login_box" => s.map(|x| self.login_box = x).is_some(),
            "notice.expiry_boxes" => b.map(|x| self.expiry_boxes = x).is_some(),
            "reports.per_hour" => n
                .map(|x| self.reports_per_hour = x.clamp(0.0, 1e6) as u32)
                .is_some(),
            "messenger.max_friends" => n
                .map(|x| self.max_friends = x.clamp(0.0, 1000.0) as u32)
                .is_some(),
            "messenger.whisper_friends_only" => b.map(|x| self.whisper_friends_only = x).is_some(),
            "monitor.retention_days" => n
                .map(|x| self.monitor_retention_days = x.clamp(0.0, 3650.0) as u32)
                .is_some(),
            "monitor.public_games_file" => s.map(|x| self.monitor_public_games_file = x).is_some(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_apply_and_reject_bad_values() {
        let mut r = OpsRules::default();
        assert!(OpsRules::owns("gm.test_commands") && OpsRules::owns("messenger.max_friends"));
        assert!(!OpsRules::owns("shop.coupons"));
        assert!(r.apply("gm.test_commands", &JsonValue::String("gm".into())));
        assert_eq!(r.test_commands, TestCommands::Gm);
        assert!(!r.apply("gm.test_commands", &JsonValue::String("nobody".into())));
        assert!(r.apply("notice.ticker_repeat", &JsonValue::Number(5.0)));
        assert_eq!(r.ticker_repeat, 5);
        assert!(r.apply("notice.case_announce", &JsonValue::String("off".into())));
        assert_eq!(r.case_announce, CaseAnnounce::Off);
        assert!(!r.apply("gm.bogus", &JsonValue::Bool(true)));
    }

    #[test]
    fn defaults_are_the_proposed_rules() {
        let r = OpsRules::default();
        assert_eq!(r.test_commands, TestCommands::Loopback);
        assert_eq!(
            (
                r.ticker_period_secs,
                r.ticker_repeat,
                r.default_mute_minutes
            ),
            (30, 3, 60)
        );
        assert_eq!(
            (r.max_friends, r.reports_per_hour, r.case_announce),
            (50, 20, CaseAnnounce::Permanent)
        );
    }
}
