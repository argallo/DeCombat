//! Sans-IO life cycle of the bot inside a match: the respawn wait and the dead-link watch. The session feeds it
//! server messages and the clock and sends what it asks for.
//!
//! Respawn (GAME_MESSAGES_2009_C2S.md section 3.2-3.4, GAME_MODES_2009.md section 4.2): a not-playing state
//! (server 0x67 value 2, 3, 5 or 6) lets the client's own wait run. The client arms it itself once per death, after
//! it processed 0x67 state 2, by mode class (7000 ms in classes 1, 4, 6, 7, 8; 1500 ms in class 10; class 5 and 9
//! conditionally; the other classes never ask); server 0xd7 sub 2 and 0xdb sub 6 re-arm it with 7000 ms. Once armed
//! and the wait passed (strictly), the client sends one respawn request chosen by the mode class and disarms.
// DISASM: cshell+0x60ac0 (local_player derived slot +0x70, called per frame by cshell+0x61e40 while the death time
// +0x440 set by state 2 is non-zero), read by capval-kill: capval-kill_CAPTURE_VALIDATION.md
// §4 (correction to C2S §3.3, staged); OBSERVED: tdm4 (the reference stack never arms the wait and never respawns unasked)
// SPEC: GAME_MESSAGES_2009_C2S.md §3.3 (wait armed by 0xd7 sub 2 / 0xdb sub 6, 7000 ms), §3.4 (0x89 / 0xc5 / 0xd2)
// SPEC: GAME_MESSAGES_2009_S2C.md §4.1 (0x67 states), §3.5 (0xd7 sub 2 [a:u8][b:u8]), 0xdb [k:u8][sub:u8] sub 6 [n:u32]
// DC-DEFAULT: PD-S2C-2' (S2C §4.1) / DC-C2S-10' (C2S §12.3) / DSV-32 (proposed), see DC_DEFAULTS.md

use dc_net_engine::time::since;
use dc_net_engine::Millis;

/// The client-side respawn wait.
// SPEC: GAME_MESSAGES_2009_C2S.md §3.3 (7000 ms, cshell+0x5df50)
pub const RESPAWN_WAIT_MS: u32 = 7000;
/// Server messages that arm the wait.
pub const GS_COOP: u8 = 0xd7;
pub const GS_RESPAWN_TOKEN: u8 = 0xdb;
/// Respawn requests (client -> server shell ids).
pub const UP_RESPAWN_REQUEST: u8 = 0x89;
pub const UP_RESPAWN_REQUEST_B: u8 = 0xc5;
pub const UP_RESPAWN_REQUEST_C: u8 = 0xd2;
/// The client's own wait in mode class 10.
// DISASM: cshell+0x60bd6..0x60bec (+0x450 := 1500), capval-kill §4 item 1
pub const SELF_ARM_WAIT_CLASS10_MS: u32 = 1500;
/// Default for the dead-link watch: no datagram from the server for this long ends the session.
// DC-DEFAULT: BOT-10 (proposed) 15000 ms
pub const DEFAULT_IDLE_EXIT_MS: u32 = 15_000;
/// Server `GS_MATCH_RESULT_SCREEN` `[result_flag:bool1][result_value:u32]`: the client opens its result screen,
/// sets its leaving latches and leaves the game server.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x3f); GAME_MESSAGES_2009_C2S.md Appendix A step A13
pub const GS_MATCH_RESULT_SCREEN: u8 = 0x3f;
/// How long the bot stays after 0x3f before it leaves (the time a player looks at the result screen).
// DC-DEFAULT: BOT-16 (proposed) 3000 ms, `--stay-after-result` keeps the bot in the session
pub const LEAVE_AFTER_RESULT_MS: u32 = 3000;

/// The respawn request for a mode class and whether it carries `[zero:u8]`; `None` = the class has no respawn
/// request (the dead player waits for the next round).
// SPEC: GAME_MESSAGES_2009_C2S.md §3.4 (table), GAME_MODES_2009.md §4.2
pub fn request_for(mode_class: u8) -> Option<(u8, bool)> {
    match mode_class {
        1 | 4 | 5 | 6 | 9 => Some((UP_RESPAWN_REQUEST, false)),
        7 | 8 => Some((UP_RESPAWN_REQUEST_B, false)),
        10 => Some((UP_RESPAWN_REQUEST_C, true)),
        _ => None,
    }
}

/// The wait the client arms itself after a death in `mode_class` (`None` = this class never asks). `coop_value` is
/// the own record's value stored by 0xd7 sub 2 (class 5 arms only above 0). Class 9 also depends on a mode-rules
/// phase the bot does not track; it is taken as 0 (armed).
// DISASM: cshell+0x60b2e..0x60c0e (capval-kill §4 item 3: 1, 4, 6, 7, 8 -> 7000; 5 when +0x1f8 > 0; 9 when
// mode_rules+0xb4 = 0; 10 -> 1500; others nothing)
pub fn self_arm_wait(mode_class: u8, coop_value: u8) -> Option<u32> {
    match mode_class {
        1 | 4 | 6 | 7 | 8 | 9 => Some(RESPAWN_WAIT_MS),
        5 => (coop_value > 0).then_some(RESPAWN_WAIT_MS),
        10 => Some(SELF_ARM_WAIT_CLASS10_MS),
        _ => None,
    }
}

/// What the machine reports to the session (which logs it).
#[derive(Debug, Clone, PartialEq)]
pub enum LifeEvent {
    /// Alive -> not playing (`state` is the 0x67 value).
    Died { state: u8 },
    /// The wait was armed (`via` names the message; `a` / `b` or the token are its payload).
    Armed { via: &'static str, a: u32, b: u32 },
    /// Back to state 1 after a death, `after_ms` since the death.
    Respawned { after_ms: u32 },
}

/// A request to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub shell: u8,
    pub with_zero: bool,
    pub waited_ms: u32,
    /// "armed" (the server armed the wait) or "fallback" (`--respawn-after`).
    pub source: &'static str,
}

/// The respawn state machine.
#[derive(Debug, Clone, Default)]
pub struct Respawn {
    state: u8,
    died_at: Option<Millis>,
    armed_at: Option<Millis>,
    /// Fallback: ask this long after the death even when nothing armed the wait (not 2009 behaviour).
    fallback_ms: Option<u32>,
    last_fallback: Option<Millis>,
    requests: u32,
    /// The death time the client keeps: set by state 2, cleared by state 1 (state 3 leaves it).
    death_time: Option<Millis>,
    /// The armed request already went out for this death: the `--respawn-after` fallback stays quiet (one 0x89 per
    /// death, C2S §3.3-3.4). Cleared by the next death.
    asked: bool,
    /// The self-arming already ran for this death.
    self_armed: bool,
    /// Length of the armed wait.
    wait_ms: u32,
    /// The value 0xd7 sub 2 stored in the own record (`b`).
    coop_value: u8,
    /// Server 0x46 was seen: the round is over. Cleared by the next state 1 (spawn / round start).
    round_over: bool,
    /// The single request allowed after the round end already went out.
    post_round_sent: bool,
}

impl Respawn {
    pub fn new(fallback_ms: Option<u32>) -> Self {
        Respawn {
            fallback_ms,
            wait_ms: RESPAWN_WAIT_MS,
            ..Respawn::default()
        }
    }

    /// Length of the current (or last) armed wait.
    pub fn wait_ms(&self) -> u32 {
        self.wait_ms
    }

    /// The client's own arming, once per death: due once state 2 was processed while not playing and the mode
    /// class is known. Returns the `Armed` event when it armed.
    pub fn self_arm(&mut self, now: Millis, mode_class: Option<u8>) -> Option<LifeEvent> {
        if self.self_armed || self.death_time.is_none() || !self.dead() {
            return None;
        }
        let class = mode_class?;
        self.self_armed = true;
        let w = self_arm_wait(class, self.coop_value)?;
        self.armed_at = Some(now);
        self.wait_ms = w;
        Some(LifeEvent::Armed {
            via: "self after 0x67 state 2",
            a: w,
            b: class as u32,
        })
    }

    /// True from the round result (0x46) until the next state 1 (spawn / round start).
    pub fn round_over(&self) -> bool {
        self.round_over
    }

    /// True while the last 0x67 state was a not-playing one after a death.
    pub fn dead(&self) -> bool {
        self.died_at.is_some() && matches!(self.state, 2 | 3 | 5 | 6)
    }

    pub fn armed(&self) -> bool {
        self.armed_at.is_some()
    }

    pub fn requests(&self) -> u32 {
        self.requests
    }

    /// Server 0x67 `[state]`. A death is a not-playing state seen while playing (state 1); the reference stack's 3 then 2
    /// in one instant is one death.
    pub fn on_state(&mut self, now: Millis, s: u8) -> Option<LifeEvent> {
        let was_alive = self.state == 1;
        let was_dead = self.dead();
        self.state = s;
        if s == 2 && self.death_time.is_none() {
            self.death_time = Some(now);
            self.self_armed = false;
        }
        if s == 1 {
            self.death_time = None;
            self.round_over = false;
        }
        match s {
            2 | 3 | 5 | 6 if was_alive => {
                self.died_at = Some(now);
                self.armed_at = None;
                self.asked = false;
                self.last_fallback = None;
                Some(LifeEvent::Died { state: s })
            }
            1 if was_dead => {
                let after = self.died_at.take().map_or(0, |d| since(now, d));
                self.armed_at = None;
                Some(LifeEvent::Respawned { after_ms: after })
            }
            _ => None,
        }
    }

    /// Server 0x46 (round result): disarm the wait. The 2009 client's behaviour after it is unproven, so at most one
    /// fallback request may still go out while dead and none afterwards, until the next state 1.
    // DC-DEFAULT: BOT-N2 (proposed) bot-roundend.md; EVIDENCE: stack-verify.md N2 (7,568 requests on the result screen)
    pub fn on_round_over(&mut self) {
        if !self.round_over {
            self.post_round_sent = false;
        }
        self.round_over = true;
        self.armed_at = None;
    }

    /// Server 0xd7 sub 2 `[a][b]`: arms when `a == b` and the player is not playing.
    pub fn on_coop_sub2(&mut self, now: Millis, a: u8, b: u8) -> Option<LifeEvent> {
        self.coop_value = b;
        if self.dead() && a == b {
            self.armed_at = Some(now);
            self.wait_ms = RESPAWN_WAIT_MS;
            return Some(LifeEvent::Armed {
                via: "0xd7 sub 2",
                a: a as u32,
                b: b as u32,
            });
        }
        None
    }

    /// Server 0xdb `[k][sub=6][n:u32]`: arms unconditionally while not playing.
    pub fn on_token_sub6(&mut self, now: Millis, n: u32) -> Option<LifeEvent> {
        if self.dead() {
            self.armed_at = Some(now);
            self.wait_ms = RESPAWN_WAIT_MS;
            return Some(LifeEvent::Armed {
                via: "0xdb sub 6",
                a: n,
                b: 0,
            });
        }
        None
    }

    /// The request due at `now`, if any. Armed: one request more than 7000 ms after arming, then disarmed (as the
    /// 2009 client does) and the fallback then stays quiet for this death (DC-C2S-10', C2S §12.3; one 0x89 per
    /// death). Fallback (only while no armed request went out for this death, e.g. the self-armed wait is off or
    /// the class never self-arms): one request `fallback_ms` after the death and again every `fallback_ms` while
    /// still dead (DSV-32, proposed).
    pub fn poll(&mut self, now: Millis, mode_class: Option<u8>) -> Option<Request> {
        if !self.dead() {
            return None;
        }
        // an unknown class falls back to the TDM request; a class without a request never asks
        let (shell, with_zero) = request_for(mode_class.unwrap_or(1))?;
        if self.round_over && self.post_round_sent {
            return None;
        }
        if let Some(a) = self.armed_at {
            if since(now, a) > self.wait_ms {
                self.armed_at = None;
                self.asked = true;
                self.requests += 1;
                return Some(Request {
                    shell,
                    with_zero,
                    waited_ms: since(now, a),
                    source: "armed",
                });
            }
            return None;
        }
        if self.asked {
            return None;
        }
        let (Some(f), Some(d)) = (self.fallback_ms, self.died_at) else {
            return None;
        };
        let from = self.last_fallback.unwrap_or(d);
        if since(now, from) >= f {
            self.last_fallback = Some(now);
            if self.round_over {
                self.post_round_sent = true;
            }
            self.requests += 1;
            return Some(Request {
                shell,
                with_zero,
                waited_ms: since(now, d),
                source: "fallback",
            });
        }
        None
    }
}

/// The dead-link watch: time of the last datagram from the server.
#[derive(Debug, Clone, Copy)]
pub struct IdleWatch {
    idle_ms: u32,
    last: Option<Millis>,
}

impl IdleWatch {
    /// `idle_ms` = 0 switches the watch off.
    pub fn new(idle_ms: u32) -> Self {
        IdleWatch {
            idle_ms,
            last: None,
        }
    }

    pub fn touch(&mut self, now: Millis) {
        self.last = Some(now);
    }

    /// `Some(silent_ms)` once nothing came for longer than the idle time.
    pub fn expired(&self, now: Millis) -> Option<u32> {
        let l = self.last?;
        let s = since(now, l);
        (self.idle_ms > 0 && s > self.idle_ms).then_some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_by_mode_class() {
        assert_eq!(request_for(1), Some((0x89, false)));
        assert_eq!(request_for(9), Some((0x89, false)));
        assert_eq!(request_for(7), Some((0xc5, false)));
        assert_eq!(request_for(10), Some((0xd2, true)));
        assert_eq!(request_for(2), None);
        assert_eq!(request_for(11), None);
    }

    #[test]
    fn scripted_death_arm_wait_request_respawn() {
        let mut r = Respawn::new(None);
        assert_eq!(r.on_state(100, 1), None);
        assert!(!r.dead());
        // the reference stack order: 3, then 2 in the same instant; one death only
        assert_eq!(r.on_state(1000, 3), Some(LifeEvent::Died { state: 3 }));
        assert_eq!(r.on_state(1000, 2), None);
        assert!(r.dead());
        // not armed: nothing is ever requested
        assert_eq!(r.poll(60_000, Some(1)), None);
        // 0xd7 sub 2 with a != b does not arm
        assert_eq!(r.on_coop_sub2(2000, 1, 2), None);
        let ev = r.on_coop_sub2(2000, 5, 5).expect("armed");
        assert_eq!(
            ev,
            LifeEvent::Armed {
                via: "0xd7 sub 2",
                a: 5,
                b: 5
            }
        );
        // the wait: exactly 7000 ms is not enough, more than 7000 is
        assert_eq!(r.poll(2000, Some(1)), None);
        assert_eq!(r.poll(8999, Some(1)), None);
        assert_eq!(r.poll(9000, Some(1)), None);
        let q = r.poll(9001, Some(1)).expect("request");
        assert_eq!((q.shell, q.with_zero, q.source), (0x89, false, "armed"));
        assert_eq!(q.waited_ms, 7001);
        // one request per arming
        assert_eq!(r.poll(20_000, Some(1)), None);
        assert!(!r.armed());
        assert_eq!(r.requests(), 1);
        // state 1 again
        assert_eq!(
            r.on_state(9500, 1),
            Some(LifeEvent::Respawned { after_ms: 8500 })
        );
        assert!(!r.dead());
        assert_eq!(r.poll(30_000, Some(1)), None);
        // a second death starts clean
        assert_eq!(r.on_state(31_000, 2), Some(LifeEvent::Died { state: 2 }));
        assert!(!r.armed());
    }

    #[test]
    fn token_message_arms_and_class_picks_the_request() {
        let mut r = Respawn::new(None);
        r.on_state(0, 1);
        r.on_state(10, 3);
        assert!(r.on_token_sub6(20, 77).is_some());
        assert_eq!(r.poll(7021, Some(7)).map(|q| q.shell), Some(0xc5));
        let mut r = Respawn::new(None);
        r.on_state(0, 1);
        r.on_state(10, 2);
        r.on_token_sub6(20, 1);
        assert_eq!(
            r.poll(7021, Some(10)).map(|q| (q.shell, q.with_zero)),
            Some((0xd2, true))
        );
        // a class without a request never asks
        let mut r = Respawn::new(None);
        r.on_state(0, 1);
        r.on_state(10, 2);
        r.on_token_sub6(20, 1);
        assert_eq!(r.poll(70_000, Some(2)), None);
    }

    #[test]
    fn arming_while_alive_is_ignored() {
        let mut r = Respawn::new(None);
        r.on_state(0, 1);
        assert_eq!(r.on_coop_sub2(5, 3, 3), None);
        assert_eq!(r.on_token_sub6(5, 3), None);
        assert_eq!(r.poll(100_000, Some(1)), None);
    }

    #[test]
    fn fallback_asks_after_the_delay_and_repeats() {
        let mut r = Respawn::new(Some(3000));
        r.on_state(0, 1);
        r.on_state(1000, 2);
        assert_eq!(r.poll(3999, Some(1)), None);
        let q = r.poll(4000, Some(1)).expect("fallback");
        assert_eq!((q.shell, q.source, q.waited_ms), (0x89, "fallback", 3000));
        assert_eq!(r.poll(6999, Some(1)), None);
        assert!(r.poll(7000, Some(1)).is_some());
        assert_eq!(
            r.on_state(7100, 1),
            Some(LifeEvent::Respawned { after_ms: 6100 })
        );
        assert_eq!(r.poll(99_000, Some(1)), None);
    }

    #[test]
    fn the_client_arms_itself_after_state_2_by_mode_class() {
        assert_eq!(self_arm_wait(1, 0), Some(7000));
        assert_eq!(self_arm_wait(10, 0), Some(1500));
        assert_eq!(self_arm_wait(5, 0), None);
        assert_eq!(self_arm_wait(5, 3), Some(7000));
        assert_eq!(self_arm_wait(2, 0), None);
        let mut r = Respawn::new(None);
        r.on_state(0, 1);
        // the reference stack's order: 3 then 2 in one instant; the death time comes from state 2
        r.on_state(1000, 3);
        assert_eq!(
            r.self_arm(1000, Some(1)),
            None,
            "state 3 alone sets no death time"
        );
        r.on_state(1000, 2);
        // unknown class: not yet
        assert_eq!(r.self_arm(1005, None), None);
        let ev = r.self_arm(1010, Some(1)).expect("self-armed");
        assert_eq!(
            ev,
            LifeEvent::Armed {
                via: "self after 0x67 state 2",
                a: 7000,
                b: 1
            }
        );
        // once per death
        assert_eq!(r.self_arm(1020, Some(1)), None);
        assert_eq!(r.poll(8010, Some(1)), None, "strictly more than 7000 ms");
        let q = r.poll(8011, Some(1)).expect("request");
        assert_eq!((q.shell, q.source), (0x89, "armed"));
        assert_eq!(r.poll(30_000, Some(1)), None, "one request per arming");
        // respawn and a second death arm again; class 10 waits 1500 ms
        r.on_state(9000, 1);
        r.on_state(10_000, 2);
        assert!(r.self_arm(10_000, Some(10)).is_some());
        assert_eq!(r.wait_ms(), 1500);
        assert_eq!(r.poll(11_500, Some(10)), None);
        assert_eq!(r.poll(11_501, Some(10)).map(|q| q.shell), Some(0xd2));
        // a class that never asks: nothing
        let mut r = Respawn::new(None);
        r.on_state(0, 1);
        r.on_state(10, 2);
        assert_eq!(r.self_arm(20, Some(2)), None);
        assert_eq!(r.poll(60_000, Some(2)), None);
    }

    #[test]
    fn idle_watch_expires_only_after_the_idle_time() {
        let mut w = IdleWatch::new(15_000);
        assert_eq!(
            w.expired(1_000_000),
            None,
            "no datagram yet: nothing to time"
        );
        w.touch(1000);
        assert_eq!(w.expired(16_000), None);
        assert_eq!(w.expired(16_001), Some(15_001));
        w.touch(16_000);
        assert_eq!(w.expired(30_000), None);
        let mut off = IdleWatch::new(0);
        off.touch(0);
        assert_eq!(off.expired(1_000_000), None);
    }

    #[test]
    fn round_end_while_dead_allows_at_most_one_request() {
        let mut r = Respawn::new(Some(0));
        r.on_state(0, 1);
        r.on_state(1000, 2);
        r.self_arm(1000, Some(1));
        assert_eq!(r.poll(1000, Some(1)), None, "armed wait pending");
        r.on_round_over();
        let n = count_until(&mut r, 1050, 11_050, 1);
        assert!(n <= 1, "{n} requests after the round end");
        assert_eq!(count_until(&mut r, 11_100, 21_100, 1), 0);
        // the next spawn lifts it
        r.on_state(30_000, 1);
        r.on_state(31_000, 2);
        assert_eq!(count_until(&mut r, 31_000, 45_000, 1), 1);
    }

    #[test]
    fn round_end_while_alive_asks_nothing() {
        let mut r = Respawn::new(Some(0));
        r.on_state(0, 1);
        r.on_round_over();
        assert_eq!(count_until(&mut r, 100, 10_100, 1), 0);
    }

    #[test]
    fn round_end_before_the_wait_cancels_the_armed_request() {
        let mut r = Respawn::new(None);
        r.on_state(0, 1);
        r.on_state(1000, 2);
        r.self_arm(1000, Some(1));
        assert!(r.armed());
        r.on_round_over();
        assert!(!r.armed());
        assert_eq!(count_until(&mut r, 1100, 20_000, 1), 0);
        assert_eq!(r.requests(), 0);
    }

    fn count_until(r: &mut Respawn, from: Millis, to: Millis, class: u8) -> u32 {
        let mut n = 0;
        let mut t = from;
        while t <= to {
            r.self_arm(t, Some(class));
            if r.poll(t, Some(class)).is_some() {
                n += 1;
            }
            t += 50;
        }
        n
    }

    #[test]
    fn one_request_per_death_with_fallback_zero() {
        let mut r = Respawn::new(Some(0));
        r.on_state(0, 1);
        r.on_state(1000, 2);
        // self-armed wait on: exactly the armed request, the fallback never adds one
        assert_eq!(count_until(&mut r, 1000, 20_000, 1), 1);
        assert_eq!(r.requests(), 1);
        r.on_state(21_000, 1);
        // the next death re-arms: again exactly one
        r.on_state(30_000, 2);
        assert_eq!(count_until(&mut r, 30_000, 50_000, 1), 1);
        assert_eq!(r.requests(), 2);
    }

    #[test]
    fn one_request_per_death_without_fallback() {
        let mut r = Respawn::new(None);
        r.on_state(0, 1);
        r.on_state(1000, 2);
        assert_eq!(count_until(&mut r, 1000, 20_000, 1), 1);
        r.on_state(21_000, 1);
        r.on_state(30_000, 2);
        assert_eq!(count_until(&mut r, 30_000, 50_000, 1), 1);
    }

    #[test]
    fn a_new_death_without_respawn_rearms_the_ask() {
        let mut r = Respawn::new(Some(0));
        r.on_state(0, 1);
        r.on_state(1000, 2);
        assert_eq!(count_until(&mut r, 1000, 10_000, 1), 1);
        // 1 then death again (state 1 clears death time)
        r.on_state(11_000, 1);
        r.on_state(12_000, 3);
        r.on_state(12_000, 2);
        assert_eq!(count_until(&mut r, 12_000, 30_000, 1), 1);
    }

    #[test]
    fn fallback_still_fires_when_the_wait_is_not_self_armed() {
        // class 2 never self-arms and has no request; class 1 with an unknown class never self-arms
        let mut r = Respawn::new(Some(0));
        r.on_state(0, 1);
        r.on_state(1000, 2);
        assert!(
            r.poll(1000, None).is_some(),
            "fallback with the class unknown"
        );
        assert!(
            r.poll(1050, None).is_some(),
            "and keeps asking while nothing armed it"
        );
        // class 5 with coop value 0 does not self-arm
        let mut r = Respawn::new(Some(0));
        r.on_state(0, 1);
        r.on_state(1000, 2);
        assert_eq!(r.self_arm(1000, Some(5)), None);
        assert!(r.poll(1001, Some(5)).is_some());
    }
}
