//! Team jobs in the bomb modes (BOT-122), sans-IO: every bot of a team works out the same job list from the team's
//! roster (the player ids in order, turned by the round number), so no server help and no talk between the bots is
//! needed. Attackers: a lead who walks straight to the round's site, one or two escorts who trail him at a distance,
//! flankers who walk a side way (a waypoint off the straight line) and, with enough of them, one who goes for the
//! other site. Defenders: split over the sites, an odd one out rotating to the site with fewer defenders standing.
//! A bot moving to the same place as a teammate ahead of it keeps a spacing, and the jobs leave the spawn one after
//! another. A DeCombat bot behaviour, no 2009 rule.
// DC-DEFAULT: BOT-122 (proposed)

use crate::roles::{Objective, BOMB_GUARD, RESUME_MARGIN, SITE_HOLD};

/// A bot's job in a bomb round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// Attacker: straight to the round's site and plants there.
    Lead,
    /// Attacker: trails the lead (the first living one of lead and escorts) at [`ESCORT_NEAR`]..[`ESCORT_FAR`].
    Escort(u8),
    /// Attacker: to the round's site by a side way (the `n`th flank waypoint), plants when it gets there first.
    Flank(u8),
    /// Attacker: goes for the other site and plants there if nobody stops it.
    Feint,
    /// Defender: holds the site of this index (in plant point order).
    Hold(u8),
    /// Defender: holds the site with fewer living holders, between the sites in turn while they are even.
    Rotate,
}

/// A teammate following another one to the same place waits while it is closer than this to the one ahead (and
/// walks on once it is [`SPACING_RESUME`] away): a burst or a grenade aimed at one bot does not reach the next, and
/// the next is still near enough to see and cover the first (at the bots' run speed about one second behind it).
// DC-DEFAULT: BOT-122 (proposed) 256 units (8 body widths; the bomb guard radius of BOT-103), resume at 320
pub const SPACING: f32 = 256.0;
pub const SPACING_RESUME: f32 = 320.0;
/// A wait behind a teammate that is still on its way (not at the objective) lasts at most this long; then the
/// spacing is off for [`SPACING_RELEASE_MS`] and the bot passes (the one ahead may be stuck).
// DC-DEFAULT: BOT-122 (proposed) 4 s wait, 3 s release
pub const SPACING_WAIT_MAX_MS: u32 = 4000;
pub const SPACING_RELEASE_MS: u32 = 3000;
/// An escort walks toward the lead while it is farther than [`ESCORT_FAR`] and stops once within [`ESCORT_NEAR`].
// DC-DEFAULT: BOT-122 (proposed) 320..448 units
pub const ESCORT_NEAR: f32 = 320.0;
pub const ESCORT_FAR: f32 = 448.0;
/// A flank waypoint counts as passed this close, once the bot is nearer the site than the waypoint is, or when the
/// bot gets no [`VIA_GAIN`] units closer to it in [`VIA_STALL_MS`].
// DC-DEFAULT: BOT-122 (proposed) 192 units; 64 units in 6 s
pub const VIA_REACH: f32 = 192.0;
pub const VIA_GAIN: f32 = 64.0;
pub const VIA_STALL_MS: u32 = 6000;
/// No flank waypoint when the site is nearer than this to where the bot started the round.
// DC-DEFAULT: BOT-122 (proposed) 640 units
pub const FLANK_MIN_LEG: f32 = 640.0;
/// A rotating defender between sites with even holders changes site this often.
// DC-DEFAULT: BOT-122 (proposed) 25 s
pub const ROTATE_MS: u32 = 25_000;
/// A bot this close to its plant or defuse point never waits (it is at the point; the hold is bombrun's).
// DC-DEFAULT: BOT-122 (proposed) twice the plant reach of BOT-70
pub const NO_WAIT_NEAR: f32 = 2.0 * crate::bombrun::PLANT_REACH;
/// A bot whose last order is older than this starts the round again (it was dead, paused or between rounds).
pub const GAP_MS: u32 = 3000;

/// The attackers' jobs for a team of `n` (job order, before the round's turn): one lead, one escort up to four
/// players and two from five, the rest flankers; with three or more flankers and two sites the last one feints.
pub fn attack_jobs(n: usize, sites: usize) -> Vec<Job> {
    if n == 0 {
        return Vec::new();
    }
    let escorts = match n {
        1 => 0,
        2..=4 => 1,
        _ => 2,
    };
    let flankers = n - 1 - escorts;
    let feint = sites >= 2 && flankers >= 3;
    let mut jobs = vec![Job::Lead];
    jobs.extend((0..escorts).map(|k| Job::Escort(k as u8)));
    jobs.extend((0..flankers - usize::from(feint)).map(|k| Job::Flank(k as u8)));
    if feint {
        jobs.push(Job::Feint);
    }
    jobs
}

/// The defenders' jobs for a team of `n`: holders spread over the sites in turn; with an odd team (and more than
/// one site) the last one rotates.
pub fn defend_jobs(n: usize, sites: usize) -> Vec<Job> {
    let sites = sites.max(1);
    let rotate = sites >= 2 && n % 2 == 1;
    let holders = n - usize::from(rotate);
    let mut jobs: Vec<Job> = (0..holders).map(|j| Job::Hold((j % sites) as u8)).collect();
    if rotate {
        jobs.push(Job::Rotate);
    }
    jobs
}

/// The job of the team member at `index` (player id order) in round `round`: the job list turned by the round, so
/// a different bot leads each round.
pub fn job_of(index: usize, n: usize, round: u32, attacking: bool, sites: usize) -> Option<Job> {
    let jobs = if attacking {
        attack_jobs(n, sites)
    } else {
        defend_jobs(n, sites)
    };
    (index < n).then(|| jobs[(index + round as usize) % n])
}

/// How long after its round start a job stays at the spawn: the lead and the feint go first, the flankers in turn,
/// the escorts once the lead is ahead, the holders one by one, the rotator last.
// DC-DEFAULT: BOT-122 (proposed) lead / feint 0, flank 400 + 700 k, escort 1200 + 600 k, hold 500 per slot,
// rotate 2500 ms
pub fn start_delay_ms(job: Job, hold_slot: u32) -> u32 {
    match job {
        Job::Lead | Job::Feint => 0,
        Job::Flank(k) => 400 + 700 * u32::from(k),
        Job::Escort(k) => 1200 + 600 * u32::from(k),
        Job::Hold(_) => 500 * hold_slot,
        Job::Rotate => 2500,
    }
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn same(a: [f32; 3], b: [f32; 3]) -> bool {
    (a[0] - b[0]).abs() < 1.0 && (a[2] - b[2]).abs() < 1.0
}

/// The `k`th flank waypoint from `origin` to `site`: halfway along the line, to the side by 40 % of the line's
/// length (at least 384, at most 1536 units), the even ones to the left, the odd ones to the right, every second
/// pair half as far again. `None` when the site is nearer than [`FLANK_MIN_LEG`].
// DC-DEFAULT: BOT-122 (proposed)
pub fn flank_via(origin: [f32; 3], site: [f32; 3], k: u8) -> Option<[f32; 3]> {
    let (dx, dz) = (site[0] - origin[0], site[2] - origin[2]);
    let len = (dx * dx + dz * dz).sqrt();
    if len < FLANK_MIN_LEG {
        return None;
    }
    let side = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
    let off = (0.4 * len).clamp(384.0, 1536.0) * (1.0 + 0.5 * f32::from(k / 2)) * side;
    let (px, pz) = (-dz / len, dx / len);
    Some([
        origin[0] + dx * 0.5 + px * off,
        (origin[1] + site[1]) * 0.5,
        origin[2] + dz * 0.5 + pz * off,
    ])
}

/// A member of the bot's team as the bot sees it (the bot itself included).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mate {
    pub pid: u8,
    /// The streamed position (`None`: not streamed).
    pub pos: Option<[f32; 3]>,
    pub alive: bool,
}

/// What the bot knows about its team and the round.
#[derive(Debug, Clone, Default)]
pub struct Team {
    pub own_pid: u8,
    pub own_pos: [f32; 3],
    /// The team in player id order, the bot itself included.
    pub mates: Vec<Mate>,
    pub round: u32,
    pub attacking: bool,
    /// The plant points the bot can use (object id order, BOT-103).
    pub sites: Vec<[f32; 3]>,
    pub planted: bool,
    /// The planted bomb's defuse point.
    pub bomb_pos: Option<[f32; 3]>,
}

impl Team {
    /// The index of the round's site (BOT-103) and of the other one.
    fn site_index(&self) -> usize {
        self.round as usize % self.sites.len().max(1)
    }

    fn other_index(&self) -> usize {
        (self.round as usize + 1) % self.sites.len().max(1)
    }

    fn job(&self, index: usize) -> Option<Job> {
        job_of(
            index,
            self.mates.len(),
            self.round,
            self.attacking,
            self.sites.len(),
        )
    }

    /// The point a member with `job` heads for, shared with every teammate with the same point (spacing is kept
    /// between those): the planted bomb, else the site of the job. `None` for the rotator (its pick is its own).
    fn shared_point(&self, job: Job) -> Option<[f32; 3]> {
        if self.planted {
            return self.bomb_pos;
        }
        let site = |i: usize| self.sites.get(i).copied();
        match job {
            Job::Lead | Job::Escort(_) | Job::Flank(_) => site(self.site_index()),
            Job::Feint => site(self.other_index()),
            Job::Hold(s) => site(usize::from(s)),
            Job::Rotate => None,
        }
    }

    /// The order slot of a holder among the holders of its site (for the start delay).
    fn hold_slot(&self, index: usize, job: Job) -> u32 {
        (0..index).filter(|&i| self.job(i) == Some(job)).count() as u32
    }
}

/// The bot's order this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Order {
    pub job: Job,
    /// Attackers before the plant: the plant point this bot plants at.
    pub plant_at: Option<[f32; 3]>,
    /// Where the job walks to and how close it stands there.
    pub goal: Option<Objective>,
    /// The bot stands where it is (its start delay, an escort near the lead, the spacing to a teammate ahead).
    pub wait: bool,
    /// Why it waits (logs): "start", "escort", "spacing".
    pub why: &'static str,
}

/// The bot's job state over a round.
#[derive(Debug, Clone, Default)]
pub struct Squad {
    round: Option<u32>,
    last: Option<u32>,
    started: Option<u32>,
    origin: Option<[f32; 3]>,
    job: Option<Job>,
    /// The flank waypoint: place, best distance and when it last got [`VIA_GAIN`] closer; `done` once passed.
    via: Option<([f32; 3], f32, u32)>,
    via_done: bool,
    escort_walk: bool,
    spacing_wait: bool,
    wait_since: Option<u32>,
    released_until: Option<u32>,
}

impl Squad {
    /// The bot's job this round.
    pub fn job(&self) -> Option<Job> {
        self.job
    }

    /// The flank waypoint the bot walks by (logs).
    pub fn via(&self) -> Option<[f32; 3]> {
        self.via.filter(|_| !self.via_done).map(|v| v.0)
    }

    /// The order for this tick, called while the bot lives in a running round; `snap` moves a waypoint onto the
    /// bot's way grid (the known node nearest to it), `None` leaves it where it is.
    pub fn order(
        &mut self,
        now: u32,
        t: &Team,
        snap: &mut dyn FnMut([f32; 3]) -> Option<[f32; 3]>,
    ) -> Option<Order> {
        let index = t.mates.iter().position(|m| m.pid == t.own_pid)?;
        if t.sites.is_empty() {
            return None;
        }
        let job = t.job(index)?;
        let fresh = self.round != Some(t.round)
            || self.last.is_none_or(|l| now.wrapping_sub(l) > GAP_MS)
            || self.job != Some(job);
        if fresh {
            *self = Squad {
                round: Some(t.round),
                started: Some(now),
                origin: Some(t.own_pos),
                job: Some(job),
                ..Squad::default()
            };
        }
        self.last = Some(now);
        let me = t.own_pos;
        let elapsed = now.wrapping_sub(self.started.unwrap_or(now));
        let site = t.sites[t.site_index()];
        let plant_at = (t.attacking && !t.planted).then(|| match job {
            Job::Feint => t.sites[t.other_index()],
            _ => site,
        });
        let mut order = Order {
            job,
            plant_at,
            goal: None,
            wait: false,
            why: "",
        };
        // the start delay holds the job at the spawn (not after a plant: then everybody runs)
        if !t.planted && elapsed < start_delay_ms(job, t.hold_slot(index, job)) {
            order.wait = true;
            order.why = "start";
            let point = plant_at
                .or(t.shared_point(job))
                .unwrap_or_else(|| self.rotate_site(t, elapsed));
            order.goal = Some(Objective {
                goal: point,
                stand_within: SITE_HOLD,
            });
            return Some(order);
        }
        let at = |goal: [f32; 3], stand_within: f32| Some(Objective { goal, stand_within });
        order.goal = match (t.attacking, t.planted) {
            (true, true) => at(t.bomb_pos.unwrap_or(site), BOMB_GUARD),
            (false, true) => t.bomb_pos.and_then(|b| at(b, 0.0)),
            (true, false) => match job {
                Job::Escort(_) => match self.lead(t) {
                    Some(lead) if lead != t.own_pid => {
                        let lead_pos = t.mates.iter().find(|m| m.pid == lead).and_then(|m| m.pos);
                        match lead_pos {
                            Some(lp) => {
                                let d = flat(me, lp);
                                if d > ESCORT_FAR {
                                    self.escort_walk = true;
                                } else if d < ESCORT_NEAR {
                                    self.escort_walk = false;
                                }
                                if !self.escort_walk {
                                    order.wait = true;
                                    order.why = "escort";
                                }
                                at(lp, 0.0)
                            }
                            None => at(site, SITE_HOLD),
                        }
                    }
                    _ => at(site, SITE_HOLD),
                },
                Job::Flank(k) => match self.flank_step(now, me, site, k, snap) {
                    Some(v) => at(v, 0.0),
                    None => at(site, SITE_HOLD),
                },
                Job::Feint => at(t.sites[t.other_index()], SITE_HOLD),
                _ => at(site, SITE_HOLD),
            },
            (false, false) => match job {
                Job::Hold(s) => at(t.sites[usize::from(s) % t.sites.len()], SITE_HOLD),
                _ => at(self.rotate_site(t, elapsed), SITE_HOLD),
            },
        };
        if !order.wait {
            if let Some(shared) = order.goal.map(|g| match job {
                // the escort and the flanker keep the spacing on the way to their site, not to their waypoint
                Job::Escort(_) | Job::Flank(_) if !t.planted => site,
                _ => g.goal,
            }) {
                if self.spacing(now, t, index, shared) {
                    order.wait = true;
                    order.why = "spacing";
                }
            }
        }
        Some(order)
    }

    /// The acting lead: the first living member in job order among the lead and the escorts.
    fn lead(&self, t: &Team) -> Option<u8> {
        let mut cands: Vec<(Job, u8)> = t
            .mates
            .iter()
            .enumerate()
            .filter(|(_, m)| m.alive)
            .filter_map(|(i, m)| match t.job(i)? {
                j @ (Job::Lead | Job::Escort(_)) => Some((j, m.pid)),
                _ => None,
            })
            .collect();
        cands.sort_by_key(|(j, _)| match j {
            Job::Lead => 0,
            Job::Escort(k) => 1 + u32::from(*k),
            _ => u32::MAX,
        });
        cands.first().map(|c| c.1)
    }

    /// The flank waypoint while it is not passed (`None`: walk to the site).
    fn flank_step(
        &mut self,
        now: u32,
        me: [f32; 3],
        site: [f32; 3],
        k: u8,
        snap: &mut dyn FnMut([f32; 3]) -> Option<[f32; 3]>,
    ) -> Option<[f32; 3]> {
        if self.via_done {
            return None;
        }
        if self.via.is_none() {
            let Some(v) = self.origin.and_then(|o| flank_via(o, site, k)) else {
                self.via_done = true;
                return None;
            };
            let v = snap(v).unwrap_or(v);
            self.via = Some((v, flat(me, v), now));
        }
        let (v, best, at) = self.via?;
        let d = flat(me, v);
        if d <= VIA_REACH || flat(me, site) < flat(v, site) * 0.9 {
            self.via_done = true;
            return None;
        }
        if d < best - VIA_GAIN {
            self.via = Some((v, d, now));
        } else if now.wrapping_sub(at) >= VIA_STALL_MS {
            self.via_done = true;
            return None;
        }
        Some(v)
    }

    /// The rotator's site: the one with fewest living holders, the sites in turn every [`ROTATE_MS`] while even.
    fn rotate_site(&self, t: &Team, elapsed: u32) -> [f32; 3] {
        let n = t.sites.len();
        let alive_at = |s: usize| {
            t.mates
                .iter()
                .enumerate()
                .filter(|(i, m)| m.alive && t.job(*i) == Some(Job::Hold(s as u8)))
                .count()
        };
        let turn = (elapsed / ROTATE_MS) as usize;
        let pick = (0..n)
            .map(|k| (k + turn) % n)
            .min_by_key(|&s| alive_at(s))
            .unwrap_or(0);
        t.sites[pick]
    }

    /// Whether the bot waits for a teammate ahead of it on the way to `shared` (closer to it, nearer than
    /// [`SPACING`] to the bot). A wait behind one that stands at the objective lasts; behind one still on its way at
    /// most [`SPACING_WAIT_MAX_MS`].
    fn spacing(&mut self, now: u32, t: &Team, index: usize, shared: [f32; 3]) -> bool {
        let me = t.own_pos;
        let own_d = flat(me, shared);
        if own_d <= NO_WAIT_NEAR {
            self.spacing_wait = false;
            self.wait_since = None;
            return false;
        }
        let reach = if self.spacing_wait {
            SPACING_RESUME
        } else {
            SPACING
        };
        let settled = if t.planted && t.attacking {
            BOMB_GUARD
        } else {
            SITE_HOLD
        } + RESUME_MARGIN;
        let mut ahead: Option<bool> = None;
        for (i, m) in t.mates.iter().enumerate() {
            if i == index || !m.alive {
                continue;
            }
            let (Some(p), Some(mj)) = (m.pos, t.job(i)) else {
                continue;
            };
            let theirs = if mj == Job::Rotate && !t.planted {
                None
            } else {
                t.shared_point(mj)
            };
            if !theirs.is_some_and(|s| same(s, shared)) {
                continue;
            }
            let d = flat(p, shared);
            let before = d < own_d || (d == own_d && m.pid < t.own_pid);
            if before && flat(me, p) < reach {
                let at_objective = d <= settled;
                ahead = Some(ahead.unwrap_or(false) || at_objective);
            }
        }
        let Some(settled_ahead) = ahead else {
            self.spacing_wait = false;
            self.wait_since = None;
            return false;
        };
        if !settled_ahead {
            if self
                .released_until
                .is_some_and(|u| (u.wrapping_sub(now) as i32) > 0)
            {
                // released: the bot passes the one ahead
                self.spacing_wait = false;
                return false;
            }
            let since = *self.wait_since.get_or_insert(now);
            if now.wrapping_sub(since) >= SPACING_WAIT_MAX_MS {
                self.released_until = Some(now.wrapping_add(SPACING_RELEASE_MS));
                self.wait_since = None;
                self.spacing_wait = false;
                return false;
            }
        } else {
            self.wait_since = None;
        }
        self.spacing_wait = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: [f32; 3]) -> Option<[f32; 3]> {
        None
    }

    /// Orders every 100 ms from `from` to `to` (a session asks every tick); the last one.
    fn step(sq: &mut Squad, t: &Team, from: u32, to: u32) -> Order {
        let mut o = None;
        let mut ms = from;
        while ms <= to {
            o = sq.order(ms, t, &mut none);
            ms += 100;
        }
        o.unwrap()
    }

    fn team(n: usize, own: usize, attacking: bool) -> Team {
        Team {
            own_pid: own as u8 + 1,
            own_pos: [0.0; 3],
            mates: (0..n)
                .map(|i| Mate {
                    pid: i as u8 + 1,
                    pos: Some([0.0, 0.0, -100.0 * i as f32]),
                    alive: true,
                })
                .collect(),
            round: 0,
            attacking,
            sites: vec![[3000.0, 0.0, 0.0], [-3000.0, 0.0, 0.0]],
            planted: false,
            bomb_pos: None,
        }
    }

    /// BOT-122: seven attackers are lead, two escorts, three flankers and a feint; seven defenders three and three
    /// over two sites and a rotator; every member of a team gets a different job, the same on every bot, turned by
    /// the round.
    #[test]
    fn jobs_split_the_team_and_turn_with_the_round() {
        use Job::*;
        assert_eq!(
            attack_jobs(7, 2),
            vec![
                Lead,
                Escort(0),
                Escort(1),
                Flank(0),
                Flank(1),
                Flank(2),
                Feint
            ]
        );
        assert_eq!(attack_jobs(4, 2), vec![Lead, Escort(0), Flank(0), Flank(1)]);
        assert_eq!(
            attack_jobs(6, 1),
            vec![Lead, Escort(0), Escort(1), Flank(0), Flank(1), Flank(2)]
        );
        assert_eq!(attack_jobs(1, 2), vec![Lead]);
        assert_eq!(
            defend_jobs(7, 2),
            vec![Hold(0), Hold(1), Hold(0), Hold(1), Hold(0), Hold(1), Rotate]
        );
        assert_eq!(defend_jobs(4, 2), vec![Hold(0), Hold(1), Hold(0), Hold(1)]);
        assert_eq!(defend_jobs(3, 1), vec![Hold(0), Hold(0), Hold(0)]);
        for round in 0..3 {
            let jobs: Vec<_> = (0..7)
                .map(|i| job_of(i, 7, round, true, 2).unwrap())
                .collect();
            let mut sorted = jobs.clone();
            sorted.dedup();
            assert_eq!(sorted.len(), 7, "seven different jobs");
            assert_eq!(
                jobs[(7 - round as usize) % 7],
                Lead,
                "the lead turns with the round"
            );
        }
        assert_eq!(job_of(7, 7, 0, true, 2), None);
    }

    /// BOT-122: the flank waypoints lie to both sides of the straight line, farther for the second pair; none for a
    /// near site.
    #[test]
    fn flank_waypoints_are_off_the_line_on_both_sides() {
        let (o, s) = ([0.0, 0.0, 0.0], [2000.0, 0.0, 0.0]);
        let a = flank_via(o, s, 0).unwrap();
        let b = flank_via(o, s, 1).unwrap();
        let c = flank_via(o, s, 2).unwrap();
        assert_eq!((a[0], b[0]), (1000.0, 1000.0));
        assert!(a[2] > 700.0 && b[2] < -700.0, "{a:?} {b:?}");
        assert!(c[2] > a[2] + 300.0);
        assert_eq!(flank_via(o, [500.0, 0.0, 0.0], 0), None);
    }

    /// BOT-122: the start delays leave the spawn one job after another; the lead goes at once.
    #[test]
    fn jobs_leave_the_spawn_in_turn() {
        let mut sq = Squad::default();
        // the escort of round 0 is member 1
        let t = team(7, 1, true);
        let o = sq.order(0, &t, &mut none).unwrap();
        assert_eq!((o.job, o.wait, o.why), (Job::Escort(0), true, "start"));
        assert!(step(&mut sq, &t, 100, 1100).wait);
        let mut lead = Squad::default();
        let o = lead.order(0, &team(7, 0, true), &mut none).unwrap();
        assert_eq!((o.job, o.wait), (Job::Lead, false));
        assert_eq!(o.goal.map(|g| g.goal), Some([3000.0, 0.0, 0.0]));
        assert_eq!(o.plant_at, Some([3000.0, 0.0, 0.0]));
    }

    /// BOT-122: an escort walks to the lead while it is far, stands once near; the next escort takes over a dead
    /// lead's job.
    #[test]
    fn escorts_trail_the_lead_in_a_band() {
        let mut t = team(7, 1, true);
        t.mates[0].pos = Some([1000.0, 0.0, 0.0]);
        let mut sq = Squad::default();
        let o = step(&mut sq, &t, 0, 1500);
        assert_eq!(
            (o.wait, o.goal.map(|g| g.goal)),
            (false, Some([1000.0, 0.0, 0.0]))
        );
        t.own_pos = [700.0, 0.0, 0.0];
        let o = sq.order(1600, &t, &mut none).unwrap();
        assert_eq!((o.wait, o.why), (true, "escort"));
        t.own_pos = [600.0, 0.0, 0.0];
        assert!(
            sq.order(1700, &t, &mut none).unwrap().wait,
            "inside the band it stays"
        );
        t.mates[0].alive = false;
        let o = sq.order(1800, &t, &mut none).unwrap();
        assert_eq!(
            o.goal.map(|g| g.goal),
            Some([3000.0, 0.0, 0.0]),
            "it leads now"
        );
    }

    /// BOT-122: a flanker walks by its waypoint (snapped onto the grid) and to the site once it passed it or made no
    /// way to it; the feint goes for the other site and plants there.
    #[test]
    fn flankers_walk_by_their_waypoint_and_the_feint_takes_the_other_site() {
        let mut t = team(7, 3, true);
        let mut sq = Squad::default();
        let mut snap = |p: [f32; 3]| Some([p[0], p[1], p[2] + 10.0]);
        let o = sq.order(0, &t, &mut snap).unwrap();
        assert_eq!(o.job, Job::Flank(0));
        let o = sq.order(500, &t, &mut snap).unwrap();
        let via = o.goal.unwrap().goal;
        assert!(via[2] > 1000.0 && via[0] == 1500.0, "{via:?}");
        assert_eq!(sq.via(), Some(via));
        t.own_pos = [via[0] - 100.0, 0.0, via[2]];
        let o = sq.order(600, &t, &mut snap).unwrap();
        assert_eq!(o.goal.unwrap().goal, [3000.0, 0.0, 0.0], "passed");
        // a waypoint it gets no closer to is given up
        let mut t2 = team(7, 4, true);
        let mut sq2 = Squad::default();
        assert!(step(&mut sq2, &t2, 0, 1200).goal.unwrap().goal[2] < -1000.0);
        t2.own_pos = [1.0, 0.0, 0.0];
        // its start delay ends at 1100 ms, when it takes its waypoint
        assert!(
            step(&mut sq2, &t2, 1300, 1100 + VIA_STALL_MS - 100)
                .goal
                .unwrap()
                .goal[2]
                < -1000.0
        );
        assert_eq!(
            step(&mut sq2, &t2, 1100 + VIA_STALL_MS, 1100 + VIA_STALL_MS)
                .goal
                .unwrap()
                .goal,
            [3000.0, 0.0, 0.0]
        );
        let mut f = Squad::default();
        let o = f.order(0, &team(7, 6, true), &mut none).unwrap();
        assert_eq!(o.job, Job::Feint);
        assert_eq!(o.plant_at, Some([-3000.0, 0.0, 0.0]));
    }

    /// BOT-122: a bot behind a teammate on the way to the same site waits while it is within the spacing, at most a
    /// while behind one still walking, for good behind one at the site; a bot at its point never waits.
    #[test]
    fn spacing_to_the_teammate_ahead() {
        // two defenders holding site 0 (members 0 and 2), the bot is member 2
        let mut t = team(7, 2, false);
        t.mates[0].pos = Some([1000.0, 0.0, 0.0]);
        t.own_pos = [800.0, 0.0, 0.0];
        let mut sq = Squad::default();
        let o = step(&mut sq, &t, 0, 600);
        assert_eq!((o.job, o.wait, o.why), (Job::Hold(0), true, "spacing"));
        // its start delay (the second holder of the site) ends at 500 ms, when the wait starts
        assert!(step(&mut sq, &t, 700, 500 + SPACING_WAIT_MAX_MS - 100).wait);
        assert!(
            !step(
                &mut sq,
                &t,
                500 + SPACING_WAIT_MAX_MS,
                500 + SPACING_WAIT_MAX_MS
            )
            .wait,
            "released behind one still walking"
        );
        // the one ahead stands at the site: the wait lasts
        t.mates[0].pos = Some([2900.0, 0.0, 0.0]);
        t.own_pos = [2700.0, 0.0, 0.0];
        let mut sq = Squad::default();
        assert!(step(&mut sq, &t, 0, 600).wait);
        assert!(
            step(&mut sq, &t, 700, 20_000).wait,
            "behind one at the site the wait lasts"
        );
        // hysteresis: it walks again only past the resume distance
        t.own_pos = [2900.0 - 300.0, 0.0, 0.0];
        assert!(sq.order(20_100, &t, &mut none).unwrap().wait);
        t.own_pos = [2900.0 - 330.0, 0.0, 0.0];
        assert!(!sq.order(20_200, &t, &mut none).unwrap().wait);
        // a teammate of the other site does not count
        let mut t = team(7, 2, false);
        t.mates[1].pos = Some([1000.0, 0.0, 0.0]);
        t.own_pos = [800.0, 0.0, 0.0];
        let mut sq = Squad::default();
        sq.order(0, &t, &mut none);
        assert!(!sq.order(600, &t, &mut none).unwrap().wait);
        // planted: every defender goes for the bomb, the nearest one never waits
        let mut t = team(7, 2, false);
        t.planted = true;
        t.bomb_pos = Some([3000.0, 0.0, 0.0]);
        t.own_pos = [2950.0, 0.0, 0.0];
        t.mates[0].pos = Some([2900.0, 0.0, 10.0]);
        let mut sq = Squad::default();
        let o = sq.order(0, &t, &mut none).unwrap();
        assert_eq!(o.goal.map(|g| g.goal), Some([3000.0, 0.0, 0.0]));
        assert!(!o.wait, "at the bomb it defuses");
    }

    /// BOT-122: the rotator goes to the site with fewer living holders.
    #[test]
    fn the_rotator_backs_the_thinner_site() {
        let mut t = team(7, 6, false);
        let mut sq = Squad::default();
        assert_eq!(sq.order(0, &t, &mut none).unwrap().job, Job::Rotate);
        step(&mut sq, &t, 100, 2900);
        // holders of site 0 are members 0, 2, 4; two of them die
        t.mates[0].alive = false;
        t.mates[2].alive = false;
        let o = sq.order(3000, &t, &mut none).unwrap();
        assert_eq!(o.goal.unwrap().goal, [3000.0, 0.0, 0.0]);
        t.mates[0].alive = true;
        t.mates[2].alive = true;
        t.mates[1].alive = false;
        let o = sq.order(3100, &t, &mut none).unwrap();
        assert_eq!(o.goal.unwrap().goal, [-3000.0, 0.0, 0.0]);
    }
}
