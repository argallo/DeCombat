//! Scripted bot timeline (`--timeline <file>`): what a bot does at fixed times of its match, so a comparison harness
//! (`arena.py`) can replay the same scene against different clients under test.
//!
//! The file is plain text, one step per line; `#` starts a comment. `[Name]` opens a section that applies only to
//! the bot with that nick (case-insensitive); lines before the first section apply to every bot, so one file serves
//! all bots of a session (which get the same command line).
//!
//! ```text
//! team 2                 # lobby: ask for team 2 (0x390) before ready (no time)
//! host 4 0               # this bot creates the room (mode index 0) and starts it with 4 members (no time)
//! setting 10 1           # host: one room setting change 0x38c (field type, value) after creating it (no time)
//! sync sbs4real 60000    # the clock starts once this bot AND that player are spawned (give up after 60 s)
//! @0 hold                # @<ms since this bot's first spawn (or the sync)> <verb> [args]
//! @1500 target sbs4real  # shoot only at this player (nick) | target nearest | target pid 3
//! @2000 fire 3           # let the shooter fire; hold again after 3 shots (no count: until the next hold)
//! @6000 throw            # one throw of the slot-3 grant now
//! @8000 goto 400 50 -1200    # walk to this point (needs --hunt); goto off
//! @8000 goto 400 50 -1200 8  # walk to within 8 units, STAY there (no hunting, no shot on the way) until goto off
//! @8000 look 400 60 -1200    # face this point while there is no target; look off
//! @8000 approach sbs4real    # walk up to this player (nick) and stop close to it (needs --hunt); approach off
//! @9000 mark checkpoint  # an event-log line only
//! @30000 leave           # quit the game session
//! ```
//!
//! Steps run in time order (ties in file order). Each applied step is logged as event `timeline`. A test-tool
//! choice, not 2009 client behaviour.
//!
//! `sync <nick>` gives every bot of a session the same clock zero: the moment the named player (the client under
//! test) is first seen spawned, or this bot's own first spawn if that comes later. Until then the bot holds fire.
// DC-DEFAULT: BOT-40 (proposed) timeline steps relative to the bot's first spawn, see DC_DEFAULTS.md
// DC-DEFAULT: BOT-43 (staged, arena-bots) `sync <nick> [give-up ms]`, default give-up 60000 ms

use std::path::Path;

use crate::shooter::Seen;

/// Whom the shooter may aim at.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TargetSel {
    /// The shooter's own choice (the nearest enemy).
    #[default]
    Nearest,
    /// The player with this nick (case-insensitive).
    Nick(String),
    /// The player with this game player id.
    Pid(u8),
}

/// One timeline action.
#[derive(Debug, Clone, PartialEq)]
pub enum Verb {
    /// Stop firing (the shooter sees no players).
    Hold,
    /// Fire again; `Some(n)`: hold after `n` more shots.
    Fire(Option<u32>),
    /// One throw of the slot-3 grant.
    Throw,
    Target(TargetSel),
    /// Walk to this point (`None` = stop walking there).
    Goto(Option<[f32; 3]>),
    /// Walk to within the radius of this point and stay there (the shooter's own hunting is off) until `goto off`.
    // DC-DEFAULT: BOT-46 (proposed)
    GotoNear([f32; 3], f32),
    /// Face this point while there is no target (`None` = off).
    Look(Option<[f32; 3]>),
    /// Walk up to this player (lower-case nick) and stop close to it (`None` = off).
    Approach(Option<String>),
    /// Quit the game session.
    Leave,
    /// An event-log line only.
    Mark(String),
}

impl Verb {
    /// The verb word (the event log's `verb`).
    pub fn word(&self) -> &'static str {
        match self {
            Verb::Hold => "hold",
            Verb::Fire(_) => "fire",
            Verb::Throw => "throw",
            Verb::Target(_) => "target",
            Verb::Goto(_) | Verb::GotoNear(..) => "goto",
            Verb::Look(_) => "look",
            Verb::Approach(_) => "approach",
            Verb::Leave => "leave",
            Verb::Mark(_) => "mark",
        }
    }
}

/// One timed step.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// Milliseconds after the bot's first spawn.
    pub at_ms: u32,
    pub verb: Verb,
    /// 1-based line in the file (for the log).
    pub line: usize,
}

/// A parsed timeline for one bot.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Timeline {
    /// Lobby team request (1 or 2) before ready.
    pub team: Option<u32>,
    /// This bot hosts: (members to start with, room mode index); overrides `--room`.
    pub host: Option<(usize, i32)>,
    /// Host: room setting changes (0x38c field type, value) in file order, sent after creating the room.
    pub settings: Vec<(u32, u32)>,
    /// Steps in time order (ties keep the file order).
    pub steps: Vec<Step>,
    /// `sync <nick> [ms]`: the clock waits for this player's first spawn (lower-case nick, give-up time after the
    /// bot's own first spawn).
    pub sync: Option<(String, u32)>,
}

/// Give-up time of `sync` without a value: the clock starts this long after the bot's own first spawn even when
/// the named player was never seen spawned (a typo in the nick must not freeze the scene).
// DC-DEFAULT: BOT-43 (staged, arena-bots)
pub const SYNC_GIVE_UP_MS: u32 = 60_000;

fn point(args: &[&str], line: usize) -> Result<Option<[f32; 3]>, String> {
    if args == ["off"] {
        return Ok(None);
    }
    if args.len() != 3 {
        return Err(format!("line {line}: expected x y z or off"));
    }
    let mut p = [0.0f32; 3];
    for (i, a) in args.iter().enumerate() {
        p[i] = a
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("line {line}: bad coordinate {a:?}"))?;
    }
    Ok(Some(p))
}

fn verb(word: &str, args: &[&str], line: usize) -> Result<Verb, String> {
    let none = |v: Verb| {
        if args.is_empty() {
            Ok(v)
        } else {
            Err(format!("line {line}: {word} takes no arguments"))
        }
    };
    match word {
        "hold" => none(Verb::Hold),
        "throw" => none(Verb::Throw),
        "leave" => none(Verb::Leave),
        "fire" => match args {
            [] => Ok(Verb::Fire(None)),
            [n] => n
                .parse::<u32>()
                .map(|n| Verb::Fire(Some(n)))
                .map_err(|_| format!("line {line}: fire: bad shot count {n:?}")),
            _ => Err(format!("line {line}: fire takes at most one count")),
        },
        "target" => match args {
            ["nearest"] => Ok(Verb::Target(TargetSel::Nearest)),
            ["pid", n] => n
                .parse::<u8>()
                .map(|p| Verb::Target(TargetSel::Pid(p)))
                .map_err(|_| format!("line {line}: target pid: bad id {n:?}")),
            [nick] => Ok(Verb::Target(TargetSel::Nick(nick.to_ascii_lowercase()))),
            _ => Err(format!("line {line}: target <nick> | nearest | pid <n>")),
        },
        "goto" if args.len() == 4 => {
            let p = point(&args[..3], line)?
                .ok_or_else(|| format!("line {line}: goto x y z [radius]"))?;
            let r = args[3]
                .parse::<f32>()
                .ok()
                .filter(|r| r.is_finite() && *r > 0.0)
                .ok_or_else(|| format!("line {line}: goto: bad radius {:?}", args[3]))?;
            Ok(Verb::GotoNear(p, r))
        }
        "goto" => point(args, line).map(Verb::Goto),
        "look" => point(args, line).map(Verb::Look),
        "approach" => match args {
            ["off"] => Ok(Verb::Approach(None)),
            [nick] => Ok(Verb::Approach(Some(nick.to_ascii_lowercase()))),
            _ => Err(format!("line {line}: approach <nick> | off")),
        },
        "mark" => Ok(Verb::Mark(args.join(" "))),
        other => Err(format!("line {line}: unknown verb {other:?}")),
    }
}

/// Parses the timeline text for the bot named `nick`.
pub fn parse(text: &str, nick: &str) -> Result<Timeline, String> {
    let me = nick.to_ascii_lowercase();
    let mut tl = Timeline::default();
    let mut mine = true;
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let body = raw.split('#').next().unwrap_or("").trim();
        if body.is_empty() {
            continue;
        }
        if let Some(name) = body.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            mine = name.trim().to_ascii_lowercase() == me;
            continue;
        }
        let words: Vec<&str> = body.split_whitespace().collect();
        if words[0] == "host" {
            let n = |i: usize| words.get(i).and_then(|w| w.parse::<i64>().ok());
            let members = n(1).filter(|m| (1..=16).contains(m));
            let mode = if words.len() > 2 { n(2) } else { Some(0) };
            let (Some(m), Some(mode), true) = (members, mode, words.len() <= 3) else {
                return Err(format!("line {line}: host <members 1..16> [mode index]"));
            };
            if mine {
                tl.host = Some((m as usize, mode as i32));
            }
            continue;
        }
        if words[0] == "setting" {
            let v: Vec<u32> = words[1..].iter().filter_map(|w| w.parse().ok()).collect();
            if v.len() != 2 || words.len() != 3 {
                return Err(format!("line {line}: setting <field type> <value>"));
            }
            if mine {
                tl.settings.push((v[0], v[1]));
            }
            continue;
        }
        if words[0] == "sync" {
            let give_up = match words.get(2) {
                None => Some(SYNC_GIVE_UP_MS),
                Some(w) => w.parse::<u32>().ok(),
            };
            let (Some(nick), Some(ms), true) = (words.get(1), give_up, words.len() <= 3) else {
                return Err(format!("line {line}: sync <nick> [give-up ms]"));
            };
            if mine {
                tl.sync = Some((nick.to_ascii_lowercase(), ms));
            }
            continue;
        }
        if words[0] == "team" {
            let t = match words.get(1..) {
                Some([t]) => t.parse::<u32>().ok().filter(|t| *t == 1 || *t == 2),
                _ => None,
            }
            .ok_or_else(|| format!("line {line}: team 1 | team 2"))?;
            if mine {
                tl.team = Some(t);
            }
            continue;
        }
        let at = words[0]
            .strip_prefix('@')
            .ok_or_else(|| format!("line {line}: a step starts with @<ms>"))?;
        let at_ms = at
            .parse::<u32>()
            .map_err(|_| format!("line {line}: bad time {at:?}"))?;
        let word = words
            .get(1)
            .ok_or_else(|| format!("line {line}: missing verb"))?;
        let v = verb(word, &words[2..], line)?;
        if mine {
            tl.steps.push(Step {
                at_ms,
                verb: v,
                line,
            });
        }
    }
    tl.steps.sort_by_key(|s| s.at_ms);
    Ok(tl)
}

/// Reads and parses a timeline file for `nick`.
pub fn load(path: &Path, nick: &str) -> Result<Timeline, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text, nick).map_err(|e| format!("{}: {e}", path.display()))
}

/// The running state of a timeline in a session.
#[derive(Debug, Clone)]
pub struct Player {
    tl: Timeline,
    next: usize,
    t0: Option<u32>,
    /// The bot's own first spawn (the `sync` give-up time counts from it).
    alive_at: Option<u32>,
    /// False while held: the shooter sees no players.
    pub firing: bool,
    pub shots_left: Option<u32>,
    pub target: TargetSel,
    pub goto: Option<[f32; 3]>,
    /// `goto x y z r`: the reach radius, and the bot stays there (no hunting) while the goto is set.
    pub goto_radius: Option<f32>,
    pub look: Option<[f32; 3]>,
    pub approach: Option<String>,
}

impl Player {
    pub fn new(tl: Timeline) -> Self {
        Player {
            tl,
            next: 0,
            t0: None,
            alive_at: None,
            firing: true,
            shots_left: None,
            target: TargetSel::Nearest,
            goto: None,
            goto_radius: None,
            look: None,
            approach: None,
        }
    }

    /// The timeline clock starts at the first call (the bot's first spawn); later calls do nothing.
    pub fn start(&mut self, now_ms: u32) {
        self.t0.get_or_insert(now_ms);
    }

    /// The `sync` player, if the clock waits for one and has not started yet.
    pub fn sync_wanted(&self) -> Option<&str> {
        match (&self.tl.sync, self.t0) {
            (Some((nick, _)), None) => Some(nick),
            _ => None,
        }
    }

    /// Called every tick while the bot is alive: starts the clock now when there is no `sync`, when the `sync`
    /// player is spawned (`peer_spawned`), or when the give-up time after the bot's first spawn ran out. Returns
    /// `Some(gave_up)` at the moment the clock starts, else `None`.
    pub fn start_alive(&mut self, now_ms: u32, peer_spawned: bool) -> Option<bool> {
        if self.t0.is_some() {
            return None;
        }
        let first = *self.alive_at.get_or_insert(now_ms);
        let gave_up = match &self.tl.sync {
            None => false,
            Some(_) if peer_spawned => false,
            Some((_, give_up)) if now_ms.wrapping_sub(first) >= *give_up => true,
            Some(_) => return None,
        };
        self.t0 = Some(now_ms);
        Some(gave_up)
    }

    /// Milliseconds on the timeline clock (`None` before the first spawn).
    pub fn clock(&self, now_ms: u32) -> Option<u32> {
        self.t0.map(|t| now_ms.wrapping_sub(t))
    }

    /// Applies every step that is due at `now_ms` and returns them (for the log and the one-shot verbs: `Throw`,
    /// `Leave`, `Mark`).
    pub fn due(&mut self, now_ms: u32) -> Vec<Step> {
        let Some(t) = self.clock(now_ms) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Some(s) = self.tl.steps.get(self.next) {
            if s.at_ms > t {
                break;
            }
            let s = s.clone();
            self.next += 1;
            match &s.verb {
                Verb::Hold => {
                    self.firing = false;
                    self.shots_left = None;
                }
                Verb::Fire(n) => {
                    self.firing = *n != Some(0);
                    self.shots_left = *n;
                }
                Verb::Target(sel) => self.target = sel.clone(),
                Verb::Goto(p) => {
                    self.goto = *p;
                    self.goto_radius = None;
                }
                Verb::GotoNear(p, r) => {
                    self.goto = Some(*p);
                    self.goto_radius = Some(*r);
                }
                Verb::Look(p) => self.look = *p,
                Verb::Approach(n) => self.approach = n.clone(),
                Verb::Throw | Verb::Leave | Verb::Mark(_) => {}
            }
            out.push(s);
        }
        out
    }

    /// Keeps the players the shooter may aim at: none while held; only the selected one with a target.
    pub fn filter(&self, players: &mut Vec<Seen>, nick_of: impl Fn(u8) -> Option<String>) {
        if !self.firing || self.sync_wanted().is_some() {
            players.clear();
            return;
        }
        match &self.target {
            TargetSel::Nearest => {}
            TargetSel::Pid(p) => players.retain(|s| s.pid == *p),
            TargetSel::Nick(n) => {
                players.retain(|s| nick_of(s.pid).is_some_and(|m| m.to_ascii_lowercase() == *n))
            }
        }
    }

    /// Counts shots fired under a `fire <n>` step; returns true when the count ran out (the bot holds again).
    pub fn shots_fired(&mut self, n: u32) -> bool {
        let Some(left) = self.shots_left.as_mut() else {
            return false;
        };
        *left = left.saturating_sub(n);
        if *left == 0 {
            self.shots_left = None;
            self.firing = false;
            return true;
        }
        false
    }

    /// Steps not yet applied.
    pub fn remaining(&self) -> usize {
        self.tl.steps.len() - self.next
    }

    /// The lobby team request.
    pub fn team(&self) -> Option<u32> {
        self.tl.team
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "\
# shared
@0 hold
[Bot1]
team 2
@2000 fire 2      # two shots
@1500 target SBS4Real
@6000 throw
@8000 goto 400 50 -1200
@9000 goto off
@9000 look 1 2 3
@30000 leave
[Bot2]
team 1
@100 fire
";

    fn seen(pid: u8) -> Seen {
        Seen {
            pid,
            team: 1,
            obj: 70 + pid as u16,
            pos: Some([0.0; 3]),
            dims: None,
            silent_ms: 0,
            parked: false,
            pos_known: true,
        }
    }

    #[test]
    fn sections_pick_the_bot_and_steps_sort_by_time() {
        let tl = parse(TEXT, "bot1").unwrap();
        assert_eq!(tl.team, Some(2));
        let words: Vec<_> = tl.steps.iter().map(|s| (s.at_ms, s.verb.word())).collect();
        assert_eq!(
            words,
            [
                (0, "hold"),
                (1500, "target"),
                (2000, "fire"),
                (6000, "throw"),
                (8000, "goto"),
                (9000, "goto"),
                (9000, "look"),
                (30000, "leave")
            ]
        );
        assert_eq!(
            tl.steps[1].verb,
            Verb::Target(TargetSel::Nick("sbs4real".into()))
        );
        let two = parse(TEXT, "BOT2").unwrap();
        assert_eq!(two.team, Some(1));
        assert_eq!(two.steps.len(), 2);
        assert!(parse(TEXT, "Bot9").unwrap().team.is_none());
    }

    #[test]
    fn bad_lines_name_their_line() {
        assert!(parse("@x hold", "a").unwrap_err().contains("line 1"));
        assert!(parse("\n@5 jump", "a").unwrap_err().contains("line 2"));
        assert!(parse("@5 goto 1 2", "a").is_err());
        assert!(parse("@5 goto 1 2 3 0", "a").is_err());
        assert!(parse("@5 goto 1 2 3 x", "a").is_err());
        let near = parse("@5 goto 1 2 3 8\n@9 goto off\n", "a").unwrap();
        assert_eq!(near.steps[0].verb, Verb::GotoNear([1.0, 2.0, 3.0], 8.0));
        let mut p = Player::new(near);
        p.start(0);
        p.due(5);
        assert_eq!((p.goto, p.goto_radius), (Some([1.0, 2.0, 3.0]), Some(8.0)));
        p.due(9);
        assert_eq!((p.goto, p.goto_radius), (None, None));
        assert!(parse("team 3", "a").is_err());
        assert!(parse("hold", "a").is_err());
        assert!(parse("@1 fire x", "a").is_err());
        assert!(parse("host 0", "a").is_err());
        assert!(parse("setting 10", "a").is_err());
        assert!(parse("@1 approach", "a").is_err());
        let ap = parse("@1 approach SBS4\n@2 approach off\n", "a").unwrap();
        assert_eq!(ap.steps[0].verb, Verb::Approach(Some("sbs4".into())));
        assert_eq!(ap.steps[1].verb, Verb::Approach(None));
        let mut pl = Player::new(ap);
        pl.start(0);
        pl.due(1);
        assert_eq!(pl.approach.as_deref(), Some("sbs4"));
        pl.due(2);
        assert_eq!(pl.approach, None);
        let h = parse(
            "[A]\nhost 4 2\nsetting 10 1\nsetting 8 5\n[B]\nhost 3\n",
            "a",
        )
        .unwrap();
        assert_eq!(h.host, Some((4, 2)));
        assert_eq!(h.settings, [(10, 1), (8, 5)]);
        assert_eq!(parse("[B]\nhost 3\n", "b").unwrap().host, Some((3, 0)));
    }

    #[test]
    fn sync_waits_for_the_named_player_or_gives_up() {
        let tl = parse("sync ArenaA 5000\n@0 hold\n@100 fire\n", "bot1").unwrap();
        assert_eq!(tl.sync, Some(("arenaa".into(), 5000)));
        assert_eq!(
            parse("sync x\n", "a").unwrap().sync,
            Some(("x".into(), SYNC_GIVE_UP_MS))
        );
        assert!(parse("sync\n", "a").is_err());
        assert!(parse("sync x y\n", "a").is_err());
        let mut p = Player::new(tl.clone());
        assert_eq!(p.sync_wanted(), Some("arenaa"));
        let mut ps = vec![seen(0)];
        p.filter(&mut ps, |_| None);
        assert!(ps.is_empty(), "no fire while waiting for the sync");
        assert_eq!(p.start_alive(1_000, false), None);
        assert!(p.due(3_000).is_empty());
        assert_eq!(
            p.start_alive(3_000, true),
            Some(false),
            "the client spawned"
        );
        assert_eq!(p.clock(3_500), Some(500));
        assert_eq!(p.start_alive(4_000, true), None, "started once");
        let mut q = Player::new(tl);
        assert_eq!(q.start_alive(1_000, false), None);
        assert_eq!(q.start_alive(5_999, false), None);
        assert_eq!(
            q.start_alive(6_000, false),
            Some(true),
            "gave up 5 s after the own spawn"
        );
        let mut r = Player::new(parse("@0 hold\n", "a").unwrap());
        assert_eq!(
            r.start_alive(7, false),
            Some(false),
            "no sync: the own spawn"
        );
        assert_eq!(r.clock(10), Some(3));
    }

    #[test]
    fn the_clock_starts_at_the_first_spawn_and_steps_apply_in_order() {
        let mut p = Player::new(parse(TEXT, "bot1").unwrap());
        assert!(p.due(5_000).is_empty(), "nothing before the first spawn");
        p.start(10_000);
        p.start(20_000); // a respawn does not restart the clock
        let first: Vec<_> = p.due(10_000).iter().map(|s| s.verb.word()).collect();
        assert_eq!(first, ["hold"]);
        assert!(!p.firing);
        let mut ps = vec![seen(0), seen(1)];
        p.filter(&mut ps, |_| None);
        assert!(ps.is_empty(), "held: the shooter sees nobody");
        let next: Vec<_> = p.due(12_000).iter().map(|s| s.verb.word()).collect();
        assert_eq!(next, ["target", "fire"]);
        assert!(p.firing);
        let mut ps = vec![seen(0), seen(1)];
        p.filter(&mut ps, |pid| {
            Some(if pid == 0 { "SBS4REAL" } else { "other" }.into())
        });
        assert_eq!(ps.iter().map(|s| s.pid).collect::<Vec<_>>(), [0]);
        assert!(!p.shots_fired(1));
        assert!(p.shots_fired(1), "the second shot ends the burst");
        assert!(!p.firing);
        let later: Vec<_> = p.due(19_000).iter().map(|s| s.verb.word()).collect();
        assert_eq!(later, ["throw", "goto", "goto", "look"]);
        assert_eq!(p.goto, None);
        assert_eq!(p.look, Some([1.0, 2.0, 3.0]));
        assert_eq!(p.remaining(), 1);
        assert_eq!(p.due(40_000)[0].verb, Verb::Leave);
        assert_eq!(p.remaining(), 0);
    }
}
