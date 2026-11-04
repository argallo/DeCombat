//! The lobby <-> dedicated server control channel: control lines read from stdin and event lines written to
//! stdout (UTF-8, one per line, tokens separated by spaces).

use std::collections::BTreeMap;

/// One expected player (`user <uid> <sid> <key> <team> <exp> <nick> [k=v ...]`).
// SPEC: DSERVER_PIPE.md §3.1
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEntry {
    pub uid: u32,
    pub sid: u64,
    pub key: u32,
    /// 0 = Alpha, 1 = Bravo (0-based on this line).
    pub team: u8,
    pub exp: i64,
    /// Display name; the lobby replaced spaces with `_`.
    pub nick: String,
    /// Optional trailing `k=v` tokens (DeCombat design, ignored by slice 1).
    pub extra: Vec<(String, String)>,
}

/// Trailing `user` line token: the account is a game moderator (`mod=1`, sent by the lobby when
/// `gm.flag_moderators_to_game` is on).
// DC-DEFAULT: CD-64 (approved) remote console for moderators only; OPS-11 (proposed) the lobby's `mod=1` token
pub const TOKEN_MODERATOR: &str = "mod";
/// Trailing `user` line token: the loadout as weapon table indices, the first one drawn (`loadout=4,0,7`).
// DC-DEFAULT: DSV-15 (proposed)
pub const TOKEN_LOADOUT: &str = "loadout";
/// Trailing `user` line tokens compared with the client's `UP_PLAYER_INFO` (`char=<n>` = `char_no`,
/// `slot=<n>` = `room_slot`).
// DC-DEFAULT: DSV-19 (proposed)
pub const TOKEN_CHAR: &str = "char";
pub const TOKEN_SLOT: &str = "slot";
/// Trailing `user` line tokens choosing the character rows of the ROM model table (`model=`, `skin=`, `face=`).
/// The lobby does not send them yet; without them the operator default applies.
// DC-DEFAULT: DSV-24 (proposed)
pub const TOKEN_MODEL: &str = "model";
pub const TOKEN_SKIN: &str = "skin";
pub const TOKEN_FACE: &str = "face";
/// Trailing `user` line token: the attachments of the equipped weapons, `parts=<slot>:<muzzle>/<sight>/<magazine>,...`
/// (`slot` = the equipment slot of the `loadout=` entry, each part the attached option's ROM item number, -1 = none).
// SPEC: DSERVER_PIPE_addendum_parts.md §1; DC-DEFAULT: LDO-2 (proposed, the lobby side), DSV-99 (proposed)
pub const TOKEN_PARTS: &str = "parts";

/// Trailing `user` line token: the player's rank number, which the lobby computed from the account's experience with
/// the 2009 client's own rank rule over the user's ROM level table (`rank=7`). The dedicated server puts it into the
/// world mark field of `GS_PLAYER_INFO`; without the token the rank is 0 (the trainee icon).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 field 17 (`h2`, record +0x210 = the rank of the world mark)
// DC-DEFAULT: DSV-141 (proposed) the lobby's badge tokens
pub const TOKEN_RANK: &str = "rank";
/// Trailing `user` line tokens of the player's clan: `clan=<clan number>` (absent or 0 = no clan),
/// `clanlabel=<name>` (escaped with [`escape_token`]), `mark=<emblem>/<background>` (the clan mark's two ids as the
/// lobby sends them in its user records).
// SPEC: PROTOCOL_2009.md table 3.6b (+0x2e clan id, +0x32 / +0x36 emblem / background), table 3.7b (+0x5e clan name)
// DC-DEFAULT: DSV-141 (proposed)
pub const TOKEN_CLAN: &str = "clan";
pub const TOKEN_CLAN_NAME: &str = "clanlabel";
pub const TOKEN_CLAN_MARK: &str = "mark";

/// The clan part of a player's badge (`clan=`, `clanlabel=`, `mark=` tokens).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClanBadge {
    /// The lobby's clan number (never 0).
    pub id: u32,
    pub name: String,
    /// Clan mark ids in the lobby's order: emblem, then background.
    pub emblem: u32,
    pub background: u32,
}

/// Escapes a value for one `k=v` token: `%` and every whitespace character become `%XX` (UTF-8 bytes), so the value
/// survives the whitespace split of the control line.
// DC-DEFAULT: DSV-141 (proposed)
pub fn escape_token(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        if c == '%' || c.is_whitespace() || c.is_control() {
            let mut b = [0u8; 4];
            for x in c.encode_utf8(&mut b).bytes() {
                out.push_str(&format!("%{x:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Reverses [`escape_token`]; a `%` without two hex digits stays as it is.
pub fn unescape_token(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |x: u8| (x as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What a roster entry asks of a hello, see [`RosterEntry::seat_ticket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeatTicket {
    NotRequired,
    /// `None`: the token was not a ticket, and no hello matches.
    Required(Option<dc_wire::seatticket::Ticket>),
}

impl RosterEntry {
    /// The value of a trailing `k=v` token (the last one wins).
    pub fn token(&self, k: &str) -> Option<&str> {
        self.extra
            .iter()
            .rev()
            .find(|x| x.0 == k)
            .map(|x| x.1.as_str())
    }

    fn token_u32(&self, k: &str) -> Option<u32> {
        self.token(k).and_then(|v| v.trim().parse().ok())
    }

    /// `ticket=<hex>` from the lobby: what this player's hello must carry. Without the token (a roster written by hand,
    /// a lobby that sends none) the name alone identifies the player. A token that cannot be read seats nobody rather
    /// than turning the check off.
    // DC-DEFAULT: LG-327 (proposed, project decision)
    pub fn seat_ticket(&self) -> SeatTicket {
        match self.token(dc_wire::seatticket::TOKEN) {
            None => SeatTicket::NotRequired,
            Some(v) => SeatTicket::Required(dc_wire::seatticket::from_hex(v)),
        }
    }

    /// True when `sent` (the ticket of a hello) satisfies this entry; compares in constant time.
    // DC-DEFAULT: LG-327 (proposed, project decision)
    pub fn ticket_accepts(&self, sent: Option<&dc_wire::seatticket::Ticket>) -> bool {
        match self.seat_ticket() {
            SeatTicket::NotRequired => true,
            SeatTicket::Required(want) => {
                let want = want.as_ref();
                // an unreadable token still costs one compare and passes nothing
                let same = dc_wire::seatticket::same(
                    want.unwrap_or(&[0; dc_wire::seatticket::LEN]),
                    sent.unwrap_or(&[0; dc_wire::seatticket::LEN]),
                );
                same && want.is_some() && sent.is_some()
            }
        }
    }

    /// `mod=1`: a game moderator account.
    pub fn moderator(&self) -> bool {
        self.token(TOKEN_MODERATOR) == Some("1")
    }

    /// `loadout=w1,w2,...` (non-numbers skipped); `None` without the token.
    pub fn loadout(&self) -> Option<Vec<u16>> {
        self.token(TOKEN_LOADOUT).map(|v| {
            v.split(',')
                .filter_map(|x| x.trim().parse::<u16>().ok())
                .collect()
        })
    }

    /// The `loadout=` entries as `(slot, item number)`: the lobby's form `slot:item` (the equipment slot of the
    /// 0x323 record and the ROM item number it holds) or a bare number (no slot, the slice-2 form). Entries that are
    /// not numbers are skipped; `None` without the token.
    // DC-DEFAULT: DSV-38 (proposed); LDO-1 (proposed, the lobby's tokens)
    pub fn loadout_items(&self) -> Option<Vec<(Option<u8>, u32)>> {
        self.token(TOKEN_LOADOUT).map(|v| {
            v.split(',')
                .filter_map(|x| {
                    let x = x.trim();
                    match x.split_once(':') {
                        Some((s, n)) => {
                            Some((Some(s.trim().parse().ok()?), n.trim().parse().ok()?))
                        }
                        None => Some((None, x.parse().ok()?)),
                    }
                })
                .collect()
        })
    }

    /// The `parts=` entries as `(equipment slot, [muzzle, sight, magazine])` with the option item numbers (`None` for
    /// -1 or any negative value). Entries without a slot, with a part count other than three or with a part that is
    /// not a number are skipped; empty without the token.
    // DC-DEFAULT: DSV-99 (proposed); LDO-2 (proposed, the lobby's token)
    pub fn weapon_options(&self) -> Vec<(u8, [Option<u32>; 3])> {
        let Some(v) = self.token(TOKEN_PARTS) else {
            return Vec::new();
        };
        v.split(',')
            .filter_map(|x| {
                let (s, rest) = x.trim().split_once(':')?;
                let slot: u8 = s.trim().parse().ok()?;
                let p: Vec<i64> = rest
                    .split('/')
                    .map(|n| n.trim().parse::<i64>())
                    .collect::<Result<_, _>>()
                    .ok()?;
                let part = |i: usize| u32::try_from(p[i]).ok();
                (p.len() == 3).then(|| (slot, [part(0), part(1), part(2)]))
            })
            .collect()
    }

    /// `rank=<n>`: the rank number the lobby computed (`None` without the token or with a bad value).
    // DC-DEFAULT: DSV-141 (proposed)
    pub fn rank(&self) -> Option<u32> {
        self.token_u32(TOKEN_RANK)
    }

    /// The clan badge: `clan=<n>` with `n` > 0, its name and mark (missing parts empty / 0); `None` when the player
    /// has no clan.
    // DC-DEFAULT: DSV-141 (proposed)
    pub fn clan(&self) -> Option<ClanBadge> {
        let id = self.token_u32(TOKEN_CLAN).filter(|&n| n != 0)?;
        let (emblem, background) = self
            .token(TOKEN_CLAN_MARK)
            .and_then(|m| {
                let (e, b) = m.split_once('/')?;
                Some((e.trim().parse().ok()?, b.trim().parse().ok()?))
            })
            .unwrap_or((0, 0));
        Some(ClanBadge {
            id,
            name: self
                .token(TOKEN_CLAN_NAME)
                .map(unescape_token)
                .unwrap_or_default(),
            emblem,
            background,
        })
    }

    /// `char=<n>`.
    pub fn char_no(&self) -> Option<u32> {
        self.token_u32(TOKEN_CHAR)
    }

    /// `slot=<n>`.
    pub fn slot(&self) -> Option<u32> {
        self.token_u32(TOKEN_SLOT)
    }

    /// `model=`, `skin=`, `face=` (each optional).
    pub fn character_rows(&self) -> (Option<u32>, Option<u32>, Option<u32>) {
        (
            self.token_u32(TOKEN_MODEL),
            self.token_u32(TOKEN_SKIN),
            self.token_u32(TOKEN_FACE),
        )
    }
}

/// Match settings (`room k=v ...`); unknown keys are kept and ignored.
// SPEC: DSERVER_PIPE.md §3.2
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoomSettings {
    pub pairs: BTreeMap<String, String>,
}

impl RoomSettings {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.pairs.get(k).map(String::as_str)
    }

    /// Mode key (`key=`), `-` when unset.
    pub fn mode_key(&self) -> &str {
        self.get("key").filter(|s| !s.is_empty()).unwrap_or("-")
    }

    /// Goal (`goal=`), 0 when unset or not a number.
    pub fn goal(&self) -> i64 {
        self.get("goal").and_then(|v| v.parse().ok()).unwrap_or(0)
    }
}

/// A parsed control line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    User(RosterEntry),
    /// `room` with its `k=v` pairs (the last one before the start wins).
    Room(RoomSettings),
    /// `con <console string>`: operator only.
    Console(String),
    /// `gain <uid> <exp> <gp>`: the rewards the lobby computed for the account (DeCombat addition), shown in the
    /// result screen's summary.
    Gain {
        uid: u32,
        exp: u32,
        gp: u32,
        /// The bonus part of each gain (0 from an older lobby).
        // DC-DEFAULT: DSV-142 (proposed)
        exp_bonus: u32,
        gp_bonus: u32,
    },
    Quit,
    Blank,
    /// A line that is not a known command, or a known one with bad fields (logged, never fatal).
    Bad(String),
}

/// Parses one stdin line (`\r\n` accepted, extra spaces tolerated).
// SPEC: DSERVER_PIPE.md §1 (whitespace split), §3, §7 (malformed lines are logged and ignored)
pub fn parse_control(line: &str) -> Control {
    let line = line.trim_end_matches(['\r', '\n']);
    let toks: Vec<&str> = line.split_whitespace().collect();
    match toks.first().copied() {
        None => Control::Blank,
        Some("quit") => Control::Quit,
        Some("con") => {
            let rest = line.trim_start();
            Control::Console(rest.get(3..).unwrap_or("").trim_start().to_string())
        }
        Some("room") => {
            let mut s = RoomSettings::default();
            for t in &toks[1..] {
                if let Some((k, v)) = t.split_once('=') {
                    s.pairs.insert(k.to_string(), v.to_string());
                }
            }
            Control::Room(s)
        }
        Some("user") => {
            parse_user(&toks).map_or_else(|| Control::Bad(line.to_string()), Control::User)
        }
        // DC-DEFAULT: DSV-35 (proposed) the lobby's gains for the summary (staged DSERVER_PIPE addendum)
        Some("gain") => match parse_gain(&toks) {
            Some(g) => Control::Gain {
                uid: g.uid,
                exp: g.exp,
                gp: g.gp,
                exp_bonus: g.exp_bonus,
                gp_bonus: g.gp_bonus,
            },
            None => Control::Bad(line.to_string()),
        },
        Some(_) => Control::Bad(line.to_string()),
    }
}

/// One `gain` line: `gain <uid> <exp> <gp> [<exp_bonus> <gp_bonus>]`. The bonus pair is the part of each gain that
/// the lobby's reward rule added on top of the plain match reward (0 when it is absent or not a number); the
/// summary message carries it in its own fields.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.4 row 0x65 (`exp_bonus` record +0x198, `gp_bonus` +0x19c; +0x1ae := sum > 0)
// DC-DEFAULT: DSV-35 (proposed) the gain line; DSV-142 (proposed) its bonus pair
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GainLine {
    pub uid: u32,
    pub exp: u32,
    pub gp: u32,
    pub exp_bonus: u32,
    pub gp_bonus: u32,
}

/// Parses the tokens of a `gain` line (token 0 is `gain`); a bonus larger than its gain is cut to the gain.
pub fn parse_gain(t: &[&str]) -> Option<GainLine> {
    let n = |i: usize| t.get(i).and_then(|x| x.parse::<u32>().ok());
    let (uid, exp, gp) = (n(1)?, n(2)?, n(3)?);
    let (eb, gb) = match (n(4), n(5)) {
        (Some(e), Some(g)) => (e.min(exp), g.min(gp)),
        _ => (0, 0),
    };
    Some(GainLine {
        uid,
        exp,
        gp,
        exp_bonus: eb,
        gp_bonus: gb,
    })
}

fn parse_user(t: &[&str]) -> Option<RosterEntry> {
    if t.len() < 7 {
        return None;
    }
    let team: u8 = t[4].parse().ok()?;
    Some(RosterEntry {
        uid: t[1].parse().ok()?,
        sid: t[2].parse().ok()?,
        key: t[3].parse().ok()?,
        team: team.min(1),
        exp: t[5].parse().ok()?,
        nick: t[6].to_string(),
        extra: t[7..]
            .iter()
            .filter_map(|x| x.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    })
}

/// `@ready port=<p> world=<w>`.
// SPEC: DSERVER_PIPE.md §4.1
pub fn ready_line(port: u16, world: &str) -> String {
    format!("@ready port={port} world={world}")
}

/// `@join <client> <uid> <key> <team> <name>` (header form; the lobby reads tokens 1 and 2). `<team>` is 0-based
/// like the `user` line (`@player team=` is 1-based); stated for DSERVER_PIPE_addendum_e2e.md B4.
// SPEC: DSERVER_PIPE.md §4.3, OQ-1
pub fn join_line(client_id: u16, p: &RosterEntry) -> String {
    format!(
        "@join {} {} {} {} {}",
        client_id, p.uid, p.key, p.team, p.nick
    )
}

/// `@seat_refused <uid> noticket`: a client with no ticket at all asked for this player's seat while nobody holds it. The
/// lobby tells the account that its client is too old. A wrong ticket is not told: a client that sends one is not old.
// DC-DEFAULT: LG-327 (proposed, project decision)
pub fn seat_refused_line(uid: u32) -> String {
    format!("@seat_refused {uid} noticket")
}

/// `@leave <client>`.
// SPEC: DSERVER_PIPE.md §4.6
pub fn leave_line(client_id: u16) -> String {
    format!("@leave {client_id}")
}

/// `@net client=<id> uid=<uid> ms=<window> rtt=<mean> rtt_min=<ms> rtt_max=<ms> rtt_n=<samples> jitter=<ms>
/// input=<per s> gap=<ms> resends=<n> drops=<n> in=<n> out=<n>`: one client's link window (every 10 s). `uid` is
/// 0 before the client was identified by its hello name. Counters are the window's, not totals.
// SPEC: DSERVER_PIPE_addendum_net.md §1; DC-DEFAULT: DSV-411 (proposed)
pub fn net_line(client_id: u16, uid: u32, w: &crate::linkwatch::LinkWindow) -> String {
    let (mean, lo, hi) = w.rtt_triple();
    format!(
        "@net client={client_id} uid={uid} ms={} rtt={mean:.0} rtt_min={lo:.0} rtt_max={hi:.0} rtt_n={} jitter={:.1} input={:.1} gap={} resends={} drops={} in={} out={}",
        w.ms,
        w.rtt_samples,
        w.jitter_ms,
        w.input_rate,
        w.max_gap_ms,
        w.resends,
        w.flow_drops,
        w.datagrams_in,
        w.datagrams_out,
    )
}

/// How a player's link ended, for `@linksum how=`.
// DC-DEFAULT: MON-1 (proposed)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveHow {
    /// The client said bye (it left the match, or quit).
    Bye,
    /// Nothing arrived for the silent-drop limit (DSV-490).
    Silent,
    /// The game dropped the player (vote kick, a rule).
    Kicked,
    /// The match ended with the player still on.
    End,
    /// The client did not reach the world before the start-up deadline.
    Startup,
    /// The client left the match and never sent its bye.
    NoBye,
    /// The transport's own 90 s idle drop.
    Timeout,
    /// The client asked to join again long after its last traffic (a fresh link replaced this one).
    Rejoin,
    /// Closed here for another reason.
    Closed,
}

impl LeaveHow {
    pub fn token(self) -> &'static str {
        match self {
            LeaveHow::Bye => "bye",
            LeaveHow::Silent => "silent",
            LeaveHow::Kicked => "kicked",
            LeaveHow::End => "end",
            LeaveHow::Startup => "startup",
            LeaveHow::NoBye => "nobye",
            LeaveHow::Timeout => "timeout",
            LeaveHow::Rejoin => "rejoin",
            LeaveHow::Closed => "closed",
        }
    }
}

/// `@linksum client=<id> uid=<uid> how=<bye|silent|kicked|end|startup|nobye|timeout|rejoin|closed> code=<bye code>
/// secs=<s> rtt=<mean> rtt_p95=<ms> rtt_max=<ms> rtt_n=<samples> jitter=<ms> input=<per s> updates=<n> gap=<ms>
/// resends=<n> rel=<reliable blocks> drops=<n> in=<n> out=<n> maxdg=<bytes>`: one identified player's link over its
/// whole life, once, when the link closes or the match ends. A lobby that does not know the line ignores it.
// DC-DEFAULT: MON-1 (proposed)
pub fn linksum_line(
    client_id: u16,
    uid: u32,
    how: LeaveHow,
    code: u32,
    t: &crate::linkwatch::LinkTotals,
) -> String {
    format!(
        "@linksum client={client_id} uid={uid} how={} code={code} secs={:.1} rtt={:.0} rtt_p95={:.0} rtt_max={:.0} rtt_n={} jitter={:.1} input={:.1} updates={} gap={} resends={} rel={} drops={} in={} out={} maxdg={}",
        how.token(),
        t.secs,
        t.rtt_mean,
        t.rtt_p95,
        t.rtt_max,
        t.rtt_samples,
        t.jitter_ms,
        t.input_rate,
        t.updates,
        t.max_gap_ms,
        t.resends,
        t.reliable_blocks,
        t.flow_drops,
        t.datagrams_in,
        t.datagrams_out,
        t.max_datagram_bytes,
    )
}

/// Per-player result. `exp` / `gp` are omitted: the lobby computes rewards (CD-54, proposed) and falls back to its
/// own formula when a server sends none. `team` is 1-based here. `name=` is last and runs to the end of the line.
// SPEC: DSERVER_PIPE.md §4.4
pub fn player_line(
    uid: u32,
    kills: u32,
    deaths: u32,
    team0: u8,
    tk: u32,
    hs: u32,
    nick: &str,
) -> String {
    player_line_with(uid, kills, deaths, team0, tk, hs, &[], nick)
}

/// [`player_line`] with extra `key=value` pairs before `name=` (DSV-472: Spy Hunt's per-player `win=` and
/// `agent=`; a lobby that does not know a key ignores it).
// SPEC: DSERVER_PIPE.md §4.4; DC-DEFAULT: DSV-472 (proposed)
#[allow(clippy::too_many_arguments)] // one positional field per `@player` key, as `player_line` has
pub fn player_line_with(
    uid: u32,
    kills: u32,
    deaths: u32,
    team0: u8,
    tk: u32,
    hs: u32,
    extra: &[(&str, u32)],
    nick: &str,
) -> String {
    let mut s = format!(
        "@player uid={uid} kills={kills} deaths={deaths} team={} tk={tk} hs={hs}",
        team0 as u32 + 1
    );
    for (k, v) in extra {
        s.push_str(&format!(" {k}={v}"));
    }
    s.push_str(&format!(" name={nick}"));
    s
}

/// `@result mode=<key> win=<team> alpha=<n> bravo=<n> limit=<n> rounds=<a>:<b>`; `win = 0` is a draw.
// SPEC: DSERVER_PIPE.md §4.5
pub fn result_line(
    mode: &str,
    win: u8,
    alpha: u32,
    bravo: u32,
    limit: i64,
    rounds: (u32, u32),
) -> String {
    format!(
        "@result mode={mode} win={win} alpha={alpha} bravo={bravo} limit={limit} rounds={}:{}",
        rounds.0, rounds.1
    )
}

/// `@hisec uid=<u32> case=<row> key=<case key> name=<nick>`: the player picked up a HiSec case in the match (the
/// lobby delivers the case, HS-6). `name=` is last and runs to the end of the line. A DeCombat addition: the lobby
/// reads unknown `@` lines as "other" and ignores them.
// SPEC: DSERVER_PIPE.md §4 (event lines `@<name> k=v ...`); HISEC_2009.md §6.1 (P's case is delivered)
// DC-DEFAULT: HS-5 / HS-6 (proposed)
pub fn hisec_line(uid: u32, case_row: u32, key: u32, nick: &str) -> String {
    format!("@hisec uid={uid} case={case_row} key={key} name={nick}")
}

/// `@end`: the server is stopping.
// SPEC: DSERVER_PIPE.md §4.7
pub const END_LINE: &str = "@end";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_line_carries_the_window_and_falls_back_to_the_estimate() {
        let w = crate::linkwatch::LinkWindow {
            ms: 10_012,
            rtt_ms: 118.4,
            rtt_samples: 9,
            rtt_mean: Some(116.2),
            rtt_min: Some(80.0),
            rtt_max: Some(1044.0),
            input_rate: 29.94,
            jitter_ms: 7.26,
            max_gap_ms: 423,
            resends: 2,
            flow_drops: 0,
            datagrams_in: 610,
            datagrams_out: 305,
            max_datagram_bytes: 548,
        };
        assert_eq!(
            net_line(3, 1006, &w),
            "@net client=3 uid=1006 ms=10012 rtt=116 rtt_min=80 rtt_max=1044 rtt_n=9 jitter=7.3 input=29.9 gap=423 resends=2 drops=0 in=610 out=305"
        );
        let none = crate::linkwatch::LinkWindow {
            rtt_ms: 40.0,
            ..crate::linkwatch::LinkWindow::default()
        };
        assert!(net_line(0, 0, &none).contains(" uid=0 ms=0 rtt=40 rtt_min=40 rtt_max=40 rtt_n=0 "));
    }

    #[test]
    fn linksum_line_carries_the_totals_and_how_the_link_ended() {
        let t = crate::linkwatch::LinkTotals {
            secs: 312.46,
            rtt_mean: 61.4,
            rtt_p95: 95.0,
            rtt_max: 410.0,
            rtt_samples: 1200,
            updates: 9300,
            input_rate: 29.76,
            jitter_ms: 3.21,
            max_gap_ms: 840,
            resends: 7,
            reliable_blocks: 2100,
            flow_drops: 1,
            datagrams_in: 19000,
            datagrams_out: 9400,
            max_datagram_bytes: 548,
        };
        assert_eq!(
            linksum_line(4, 1006, LeaveHow::Silent, 3, &t),
            "@linksum client=4 uid=1006 how=silent code=3 secs=312.5 rtt=61 rtt_p95=95 rtt_max=410 rtt_n=1200 jitter=3.2 input=29.8 updates=9300 gap=840 resends=7 rel=2100 drops=1 in=19000 out=9400 maxdg=548"
        );
        assert!(
            linksum_line(0, 7, LeaveHow::End, 0, &Default::default()).contains(" how=end code=0 ")
        );
    }

    #[test]
    fn control_lines() {
        // HARNESS: the lobby's user_line("user 7 3 99 0 120 Dave") shape (DSERVER_PIPE.md §3.1)
        match parse_control("user 7 3 99 0 120 Dave\r\n") {
            Control::User(u) => {
                assert_eq!((u.uid, u.sid, u.key, u.team, u.exp), (7, 3, 99, 0, 120));
                assert_eq!(u.nick, "Dave");
            }
            other => panic!("{other:?}"),
        }
        match parse_control("user  8 4 1 1 0 Big_Erin clan=x") {
            Control::User(u) => {
                assert_eq!(u.team, 1);
                assert_eq!(u.extra, vec![("clan".into(), "x".into())]);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse_control("user 1 2"), Control::Bad(_)));
        // HARNESS: the lobby's trailing tokens (dc-lobby-server game.rs `user_line(..) + moderator_token`)
        match parse_control("user 9 5 1 0 0 Mo mod=1 loadout=4,x,7 char=12 slot=3 model=2 skin=1") {
            Control::User(u) => {
                assert!(u.moderator());
                assert_eq!(u.loadout(), Some(vec![4, 7]));
                assert_eq!((u.char_no(), u.slot()), (Some(12), Some(3)));
                assert_eq!(u.character_rows(), (Some(2), Some(1), None));
                assert!(u.weapon_options().is_empty(), "no parts token");
            }
            other => panic!("{other:?}"),
        }
        // DC-DEFAULT: DSV-99 (proposed) the lobby's `parts=` token (LDO-2); bad entries are skipped
        match parse_control(
            "user 9 5 1 0 0 Mo loadout=0:145,10:27 parts=0:-1/1035/-1,10:1003/-1/1006,x:1/2/3,3:1/2,4:1/a/3 face=1",
        ) {
            Control::User(u) => {
                assert_eq!(
                    u.weapon_options(),
                    vec![
                        (0, [None, Some(1035), None]),
                        (10, [Some(1003), None, Some(1006)])
                    ]
                );
            }
            other => panic!("{other:?}"),
        }
        match parse_control("user 9 5 1 0 0 Plain") {
            Control::User(u) => {
                assert!(!u.moderator());
                assert_eq!((u.loadout(), u.char_no(), u.slot()), (None, None, None));
            }
            other => panic!("{other:?}"),
        }
        match parse_control("room key=TDM type=0 goal=50 ff=0 junk") {
            Control::Room(r) => {
                assert_eq!(r.mode_key(), "TDM");
                assert_eq!(r.goal(), 50);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse_control("con  sv_x 1"),
            Control::Console("sv_x 1".into())
        );
        assert_eq!(parse_control("quit"), Control::Quit);
        assert_eq!(
            parse_control("gain 7 38 40"),
            Control::Gain {
                uid: 7,
                exp: 38,
                gp: 40,
                exp_bonus: 0,
                gp_bonus: 0
            }
        );
        assert!(matches!(parse_control("gain 7 x 40"), Control::Bad(_)));
        assert_eq!(parse_control(""), Control::Blank);
        assert!(matches!(parse_control("dance"), Control::Bad(_)));
    }

    #[test]
    fn badge_tokens_of_the_user_line() {
        // DC-DEFAULT: DSV-141 (proposed) the lobby's `rank=`, `clan=`, `clanlabel=`, `mark=` tokens
        match parse_control(
            "user 9 5 1 0 3570 Mo face=1 rank=4 clan=77 clanlabel=Night%20Owls%25 mark=5/9",
        ) {
            Control::User(u) => {
                assert_eq!(u.rank(), Some(4));
                assert_eq!(
                    u.clan(),
                    Some(ClanBadge {
                        id: 77,
                        name: "Night Owls%".into(),
                        emblem: 5,
                        background: 9
                    })
                );
            }
            other => panic!("{other:?}"),
        }
        match parse_control("user 9 5 1 0 0 Mo rank=x clan=0 clanlabel=A mark=1/2") {
            Control::User(u) => {
                assert_eq!(u.rank(), None, "a bad rank is no rank");
                assert_eq!(u.clan(), None, "clan 0 = no clan");
            }
            other => panic!("{other:?}"),
        }
        match parse_control("user 9 5 1 0 0 Mo clan=3 mark=7") {
            Control::User(u) => {
                let c = u.clan().unwrap();
                assert_eq!(
                    (c.id, c.name.as_str(), c.emblem, c.background),
                    (3, "", 0, 0)
                );
            }
            other => panic!("{other:?}"),
        }
        for v in [
            "Wolves",
            "Night Owls",
            "50% off",
            "a\tb",
            "x\u{3000}y",
            "%zz",
            "%4",
        ] {
            let e = escape_token(v);
            assert_eq!(e.split_whitespace().count(), 1, "{e:?}");
            assert_eq!(unescape_token(&e), v, "{e:?}");
        }
        assert_eq!(unescape_token("%zz%4"), "%zz%4", "bad escapes stay");
    }

    #[test]
    fn gain_lines_with_and_without_the_bonus_pair() {
        // DC-DEFAULT: DSV-35 / DSV-142 (proposed)
        let toks = |l: &'static str| l.split_whitespace().collect::<Vec<_>>();
        assert_eq!(
            parse_gain(&toks("gain 7 23 40 5 8")),
            Some(GainLine {
                uid: 7,
                exp: 23,
                gp: 40,
                exp_bonus: 5,
                gp_bonus: 8
            })
        );
        assert_eq!(
            parse_gain(&toks("gain 7 23 40")).map(|g| (g.exp_bonus, g.gp_bonus)),
            Some((0, 0)),
            "the older three-number form"
        );
        assert_eq!(
            parse_gain(&toks("gain 7 3 4 9 9")).map(|g| (g.exp_bonus, g.gp_bonus)),
            Some((3, 4)),
            "a bonus never exceeds its gain"
        );
        assert_eq!(
            parse_gain(&toks("gain 7 3 4 x 9")).map(|g| (g.exp_bonus, g.gp_bonus)),
            Some((0, 0))
        );
        assert_eq!(parse_gain(&toks("gain 7 3")), None);
        assert_eq!(
            parse_control("gain 7 23 40 5 8"),
            Control::Gain {
                uid: 7,
                exp: 23,
                gp: 40,
                exp_bonus: 5,
                gp_bonus: 8
            }
        );
    }

    #[test]
    fn event_lines() {
        let u = RosterEntry {
            uid: 7,
            sid: 3,
            key: 99,
            team: 1,
            exp: 0,
            nick: "Dave".into(),
            extra: vec![],
        };
        assert_eq!(ready_line(5000, "w"), "@ready port=5000 world=w");
        assert_eq!(join_line(0, &u), "@join 0 7 99 1 Dave");
        assert_eq!(leave_line(0), "@leave 0");
        assert_eq!(
            player_line(7, 2, 0, 0, 0, 1, "Dave"),
            "@player uid=7 kills=2 deaths=0 team=1 tk=0 hs=1 name=Dave"
        );
        // DSV-472: Spy Hunt's per-player flags sit before the name
        assert_eq!(
            player_line_with(
                7,
                2,
                0,
                0,
                0,
                1,
                &[("win", 1), ("agent", 1)],
                "Dave the Great"
            ),
            "@player uid=7 kills=2 deaths=0 team=1 tk=0 hs=1 win=1 agent=1 name=Dave the Great"
        );
        assert_eq!(
            result_line("TDM", 0, 0, 0, 2, (0, 0)),
            "@result mode=TDM win=0 alpha=0 bravo=0 limit=2 rounds=0:0"
        );
    }
}
