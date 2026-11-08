//! What a bot does for its mode beyond hunting (BOT-95), sans-IO: a role per bot (attack / defend) and the walk
//! goal it gives in the objective modes, and the caution of the last one alive in an elimination round. A
//! DeCombat bot behaviour, no 2009 rule. Mode classes as GAME_MODES_2009.md §5: TDM 1 / 4, BOMB 2 / 3, COOP 5,
//! FFA 6, FLAG 7 / 8, BOUNTY 9, INFECT 11, ELIM 12 / 13.
// DC-DEFAULT: BOT-95 (proposed)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Goes for the objective (the enemy flag, the plant with or beside the carrier).
    Attack,
    /// Holds the objective (the own flag, a plant site).
    Defend,
}

/// The number of a bot name (`...bot<digits>`, any case), as the lobby reads it (LG-260).
pub fn bot_number(nick: &str) -> Option<u32> {
    let head = nick.trim_end_matches(|c: char| c.is_ascii_digit());
    let n: u32 = nick[head.len()..].parse().ok()?;
    head.to_ascii_lowercase().ends_with("bot").then_some(n)
}

/// The role of a bot: even-numbered bots defend, odd ones attack; a bot without a number attacks.
pub fn role_for(nick: &str) -> Role {
    match bot_number(nick) {
        Some(n) if n % 2 == 0 => Role::Defend,
        _ => Role::Attack,
    }
}

/// BOT-125: a bot's flag-mode job from its team's roster rows (the player ids of the team in order, the bot itself
/// included, humans too): every second member defends (member 1, 3, ..., so 2 of 4, 1 of 2 or 3), the rest attack.
/// Every bot of the team works out the same list, so the jobs need no server help and no numbered nick. The
/// `FlagJob` carries the defender's place among the team's defenders (their guard points are spread around the
/// flag). `None` when the bot's id is not in the list.
// DC-DEFAULT: BOT-125 (proposed)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagJob {
    pub role: Role,
    /// The defender's index among the defenders (0 for an attacker) and how many defenders the team has.
    pub guard: usize,
    pub guards: usize,
}

pub fn flag_job(team_pids: &[u8], own_pid: u8) -> Option<FlagJob> {
    let i = team_pids.iter().position(|p| *p == own_pid)?;
    let guards = team_pids.len() / 2;
    Some(if i % 2 == 1 {
        FlagJob {
            role: Role::Defend,
            guard: i / 2,
            guards,
        }
    } else {
        FlagJob {
            role: Role::Attack,
            guard: 0,
            guards,
        }
    })
}

/// BOT-125: a flag defender's guard place: on a ring of [`FLAG_GUARD_RING`] units around its flag at base, the
/// defenders spread evenly over it, the whole ring turned by a third every [`FLAG_GUARD_TURN_MS`] (`turn` counts
/// those periods), so the guards move between places and never stack on the flag.
// DC-DEFAULT: BOT-125 (proposed) ring 300 units, a third of a turn every 10 s (20 s left a guard standing still
// for 15 s and more, which reads as idle)
pub const FLAG_GUARD_RING: f32 = 300.0;
pub const FLAG_GUARD_TURN_MS: u32 = 10_000;
/// A guard stands within this of its guard place (BOT-103's stand rule with the resume margin).
pub const FLAG_GUARD_STAND: f32 = 96.0;

pub fn flag_guard_point(flag: [f32; 3], job: FlagJob, turn: u32) -> [f32; 3] {
    let n = job.guards.max(1) as f32;
    let a = std::f32::consts::TAU * (job.guard as f32 / n + (turn % 3) as f32 / 3.0);
    [
        flag[0] + FLAG_GUARD_RING * a.cos(),
        flag[1],
        flag[2] + FLAG_GUARD_RING * a.sin(),
    ]
}

/// BOT-138: the grid node a guard place `p` of the flag at `flag` moves onto: of `nodes` (the grid's nodes around the
/// flag), the nearest to `p` that lies [`GUARD_NEAR`]..[`GUARD_FAR`] from the flag (flat), within [`GUARD_RISE`] of
/// its height and, at a turn of the ring, at least [`GUARD_MOVE`] from the guard's last place `avoid`; else the
/// nearest such node of the band; `None` when the grid has no node in the band yet (the guard walks to the ring
/// point itself, and the grid grows there; a node outside the band, such as the spawn the grid was seeded at, held
/// a guard there for 108 s on Power Surge). Before, the nearest node of all to the ring point: where
/// the ring ran through walls around a flag room both guards snapped to the same node at every turn (Death Room: two
/// guards 30 units apart, 36-42 s still; Waverider: guards 178-350 s on one place).
/// BOT-144: `avoid` holds every place the new one keeps [`GUARD_MOVE`] from (the guard's last place, where it stands,
/// the team's other guards); with no band node that far from all of them `None` (the ring point itself: the grid
/// grows there). Before, the nearest node of the band whatever its distance: where the grid had grown the band on
/// one side only, a guard got the same node at every turn (Roadkill: 327 s on one spot, 0 shots) and both guards of
/// a team the same cluster (Two Towers: < 150 units apart 82 % of the time).
// DC-DEFAULT: BOT-138 (proposed) 150..450 units (half and one and a half ring), 128 units in height, 192 units;
// BOT-144 (proposed) no fallback onto a node near an avoided place
pub const GUARD_NEAR: f32 = 150.0;
pub const GUARD_FAR: f32 = 450.0;
pub const GUARD_RISE: f32 = 128.0;
pub const GUARD_MOVE: f32 = 192.0;

pub fn snap_guard(
    flag: [f32; 3],
    p: [f32; 3],
    nodes: &[[f32; 3]],
    avoid: &[[f32; 3]],
) -> Option<[f32; 3]> {
    let by_p = |a: &&[f32; 3], b: &&[f32; 3]| flat_dist(**a, p).total_cmp(&flat_dist(**b, p));
    let band = |n: &&[f32; 3]| {
        let d = flat_dist(**n, flag);
        (GUARD_NEAR..=GUARD_FAR).contains(&d) && (n[1] - flag[1]).abs() <= GUARD_RISE
    };
    nodes
        .iter()
        .filter(band)
        .filter(|n| avoid.iter().all(|a| flat_dist(**n, *a) >= GUARD_MOVE))
        .min_by(by_p)
        .copied()
}

/// BOT-144: the last guard of a team with two or more (`job.guard == job.guards - 1`) goes and attacks after
/// [`ROVE_QUIET_MS`] at its post with its flag at base and no enemy known within [`ROVE_THREAT`] of the flag; it is
/// a guard again once its flag leaves the base or it dies (the quiet clock starts again at the post). Before, half
/// of every team guarded whatever the map: on Roadkill the four guards saw nobody in 6 min and the four attackers
/// took the flag once.
// DC-DEFAULT: BOT-144 (proposed) 60 s, 1200 units
pub const ROVE_QUIET_MS: u32 = 60_000;
pub const ROVE_THREAT: f32 = 1200.0;

pub fn may_rove(job: FlagJob) -> bool {
    job.role == Role::Defend && job.guards >= 2 && job.guard + 1 == job.guards
}

/// What the session knows about the round, for the goal.
#[derive(Debug, Clone, Default)]
pub struct Situation {
    pub class: Option<u8>,
    pub own_team: Option<u8>,
    pub own_pos: [f32; 3],
    pub bot_no: Option<u32>,
    /// The round number of the last round start (0 before the first).
    pub round: u32,
    /// Bomb modes: the plant points (in object id order, the same on every client), whether a bomb is planted,
    /// the carrier's team and position (when a carrier is marked and streamed), whether this bot carries it.
    pub plant_points: Vec<[f32; 3]>,
    pub planted: bool,
    pub carrier_team: Option<u8>,
    pub carrier_pos: Option<[f32; 3]>,
    pub own_carries_bomb: bool,
    /// The planted bomb's place (the defuse point the server streams at it).
    pub bomb_pos: Option<[f32; 3]>,
    /// Flag modes: the own flag's position (it moves with a carrier) and whether this bot carries a flag.
    pub own_flag: Option<[f32; 3]>,
    pub own_carries_flag: bool,
    /// BOT-125: the own flag is off its base (taken or dropped, by the 0xc6 events), the bot's flag job, and its
    /// guard place (on the bot's way grid when it has one).
    pub own_flag_away: bool,
    pub flag_job: Option<FlagJob>,
    pub flag_guard: Option<[f32; 3]>,
    /// Teammates still standing (streamed and not parked), the bot itself not counted.
    pub mates_alive: usize,
    /// BOT-130: enemies still standing (streamed and not parked; 0 also when none is streamed).
    pub enemies_alive: usize,
    /// BOT-122: the bot's team job order in a bomb round (`None`: no team known, the BOT-103 rules).
    pub squad: Option<crate::squad::Order>,
}

/// The team that attacks in the bomb modes (it holds the bomb; the other team defuses).
// SPEC: GAME_MODES_2009.md §5.2 (the carried-item notice: the bomb item for team 1, the defuse item for team 2)
pub const BOMB_ATTACKERS: u8 = 1;
/// A bot that reached its objective stands once it is this close to the goal and walks again only when the goal
/// is [`RESUME_MARGIN`] farther: a defender holding a site, an attacker guarding the planted bomb, a flag
/// defender at its flag. Without the stop the way-finder kept passing the goal and turning back (an orbit).
// DC-DEFAULT: BOT-103 (proposed) site hold 160, bomb guard 256, flag guard 160, resume margin 96 units
pub const SITE_HOLD: f32 = 160.0;
pub const BOMB_GUARD: f32 = 256.0;
pub const FLAG_GUARD: f32 = 160.0;
pub const RESUME_MARGIN: f32 = 96.0;

/// An enemy in clear sight suspends the walk to an objective only this close (a far one is left for later: a
/// defender shooting at a planter 1700 units away for 36 s let the bomb blow up); while the bot runs to defuse, only
/// [`DEFUSE_ENGAGE`].
// DC-DEFAULT: BOT-103 (proposed) 900 units, 400 on the way to the defuse
pub const OBJECTIVE_ENGAGE: f32 = 900.0;
pub const DEFUSE_ENGAGE: f32 = 400.0;

/// A walk goal of the role and how close the bot gets before it stands there (0 = walk all the way).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Objective {
    pub goal: [f32; 3],
    pub stand_within: f32,
}

impl Objective {
    fn at(goal: [f32; 3], stand_within: f32) -> Self {
        Objective { goal, stand_within }
    }
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Whether the bot attacks in a bomb round: team 1, or the bomb carrier; without a known team the carrier's team,
/// else the role.
pub fn bomb_attacker(role: Role, s: &Situation) -> bool {
    if s.own_carries_bomb {
        return true;
    }
    match (s.own_team, s.carrier_team) {
        (Some(t), _) => t == BOMB_ATTACKERS,
        (None, Some(_)) => false,
        (None, None) => role == Role::Attack,
    }
}

/// BOT-103: the plant point the attackers go for this round: one site for the whole team (they arrive together and
/// cover each other), the next site in the next round.
pub fn team_site(s: &Situation) -> Option<[f32; 3]> {
    let n = s.plant_points.len();
    (n > 0).then(|| s.plant_points[s.round as usize % n])
}

/// The site a defender holds: the bot's number spreads the defenders over the sites.
fn held_site(s: &Situation) -> Option<[f32; 3]> {
    let n = s.plant_points.len();
    (n > 0).then(|| s.plant_points[s.bot_no.unwrap_or(0) as usize % n])
}

/// The role's walk goal in the objective modes; `None` where the mode's own runs (the carrier's plant, the
/// defuse, the flag runner, the intel bot) or the hunt decide.
pub fn objective_goal(role: Role, s: &Situation) -> Option<Objective> {
    match s.class? {
        // bomb (BOT-103): the carriers plant at the team's site (bombrun); after the plant the attackers guard the
        // bomb and the defenders run to it (the defuse is bombrun's); before it the defending role holds a site
        // and the attacking role of the defending team hunts
        2 | 3 => {
            if s.plant_points.is_empty() {
                return None;
            }
            let attacking = bomb_attacker(role, s);
            // BOT-122: the team job decides the walk; the carrier's plant and the defuse stay bombrun's
            if let Some(o) = s.squad {
                return match (attacking, s.planted) {
                    (true, false) if s.own_carries_bomb => None,
                    // BOT-124: a defuser walks all the way (bombrun holds), a cover stands near the bomb
                    (false, true) => o.goal.or(s.bomb_pos.map(|p| Objective::at(p, 0.0))),
                    _ => o.goal,
                };
            }
            match (attacking, s.planted) {
                (true, false) if s.own_carries_bomb => None,
                (true, false) => team_site(s).map(|p| Objective::at(p, SITE_HOLD)),
                (true, true) => s
                    .bomb_pos
                    .or_else(|| team_site(s))
                    .map(|p| Objective::at(p, BOMB_GUARD)),
                (false, true) => s
                    .bomb_pos
                    .map(|p| Objective::at(p, 0.0))
                    .or_else(|| nearest(&s.plant_points, s.own_pos).map(|p| Objective::at(p, SITE_HOLD))),
                (false, false) if role == Role::Defend => held_site(s).map(|p| Objective::at(p, SITE_HOLD)),
                (false, false) => None,
            }
        }
        // flag: the attackers run the flags (flagrun); a defender guards the own flag (chasing it when carried)
        // BOT-125: with a flag job a defender whose flag is off its base walks all the way to it (a dropped flag
        // is returned by touching it, a carried one is chased), else it guards its place on the ring around it
        7 | 8 => {
            let role = s.flag_job.map_or(role, |j| j.role);
            if role != Role::Defend || s.own_carries_flag {
                return None;
            }
            match s.flag_job {
                Some(_) if s.own_flag_away => s.own_flag.map(|p| Objective::at(p, 0.0)),
                Some(_) => s
                    .flag_guard
                    .or(s.own_flag)
                    .map(|p| Objective::at(p, FLAG_GUARD_STAND)),
                None => s.own_flag.map(|p| Objective::at(p, FLAG_GUARD)),
            }
        }
        _ => None,
    }
}

/// BOT-103: where a bot goes when the hunt has no goal either: a bomb-round defender holds its site.
pub fn idle_goal(s: &Situation) -> Option<Objective> {
    match s.class? {
        2 | 3 if !s.planted && !bomb_attacker(Role::Defend, s) => s
            .squad
            .and_then(|o| o.goal)
            .or_else(|| held_site(s).map(|p| Objective::at(p, SITE_HOLD))),
        _ => None,
    }
}

/// BOT-103: whether a bot `d` units from its objective stands there: within `stand_within`, or within that plus
/// [`RESUME_MARGIN`] while it already stood at this goal.
pub fn stands(o: &Objective, d: f32, stood: bool) -> bool {
    o.stand_within > 0.0
        && d <= o.stand_within + if stood { RESUME_MARGIN } else { 0.0 }
}

/// The flat distance from `a` to `b` (for the stand check).
pub fn flat_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    flat(a, b)
}

/// ELIM (no respawn): whether the bot is the last one standing of its team.
pub fn cautious(s: &Situation) -> bool {
    matches!(s.class, Some(12 | 13)) && s.mates_alive == 0
}

/// BOT-130: the last one standing holds where it is (and fights what it sees) for this long after it became the
/// last; then it hunts (the enemy's last-seen place, BOT-19) and roams (BOT-117) again.
// DC-DEFAULT: BOT-130 (proposed) 15 s
pub const CAUTIOUS_HOLD_MS: u32 = 15_000;

/// BOT-130: whether the last one standing still holds, `held_ms` after it became the last: not once the hold is
/// over, and not at all when the other side is down to one too (the two would otherwise wait each other out).
pub fn cautious_holds(s: &Situation, held_ms: u32) -> bool {
    cautious(s) && s.enemies_alive != 1 && held_ms < CAUTIOUS_HOLD_MS
}

fn nearest(points: &[[f32; 3]], at: [f32; 3]) -> Option<[f32; 3]> {
    points
        .iter()
        .copied()
        .min_by(|a, b| flat(*a, at).total_cmp(&flat(*b, at)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_alternate_by_bot_number() {
        assert_eq!(role_for("Bot1"), Role::Attack);
        assert_eq!(role_for("VpsBot2"), Role::Defend);
        assert_eq!(role_for("Erika"), Role::Attack);
        assert_eq!(bot_number("pcbot12"), Some(12));
    }

    /// BOT-103: team 1 attacks: the carriers are left to bombrun, a non-carrier heads for the team's site of the
    /// round; after the plant the attackers guard the bomb and the defenders go to it; before it a defending-role
    /// defender holds its site and an attacking-role defender hunts (its idle goal is the site).
    #[test]
    fn bomb_roles_by_team_site_guard_and_defuse() {
        let sites = vec![[1000.0, 0.0, 0.0], [-1000.0, 0.0, 0.0]];
        let att = Situation {
            class: Some(2),
            own_team: Some(BOMB_ATTACKERS),
            own_pos: [0.0, 0.0, 0.0],
            plant_points: sites.clone(),
            round: 1,
            ..Default::default()
        };
        let carrier = Situation {
            own_carries_bomb: true,
            ..att.clone()
        };
        assert_eq!(objective_goal(Role::Attack, &carrier), None, "bombrun plants");
        // the team's site is the round's, whatever the bot's role or the nearest site
        for role in [Role::Attack, Role::Defend] {
            assert_eq!(
                objective_goal(role, &att),
                Some(Objective::at(sites[1], SITE_HOLD))
            );
        }
        assert_eq!(team_site(&Situation { round: 2, ..att.clone() }), Some(sites[0]));
        // planted: guard the bomb
        let bomb = [-980.0, 0.0, 30.0];
        let guard = Situation {
            planted: true,
            bomb_pos: Some(bomb),
            ..att.clone()
        };
        assert_eq!(
            objective_goal(Role::Attack, &guard),
            Some(Objective::at(bomb, BOMB_GUARD))
        );
        // defenders (team 2, no carrier mark ever reaches them)
        let def = Situation {
            own_team: Some(2),
            bot_no: Some(3),
            ..att.clone()
        };
        assert!(!bomb_attacker(Role::Attack, &def));
        assert_eq!(
            objective_goal(Role::Defend, &def),
            Some(Objective::at(sites[1], SITE_HOLD)),
            "a defender holds the site of its number"
        );
        assert_eq!(objective_goal(Role::Attack, &def), None, "it hunts");
        assert_eq!(
            idle_goal(&def),
            Some(Objective::at(sites[1], SITE_HOLD)),
            "with nobody to hunt it holds its site"
        );
        let def_planted = Situation {
            planted: true,
            bomb_pos: Some(bomb),
            ..def.clone()
        };
        for role in [Role::Attack, Role::Defend] {
            assert_eq!(
                objective_goal(role, &def_planted),
                Some(Objective::at(bomb, 0.0)),
                "every defender runs to the bomb"
            );
        }
        assert_eq!(idle_goal(&def_planted), None);
        // no team known: the carrier's team, else the role
        let unknown = Situation {
            own_team: None,
            ..att.clone()
        };
        assert!(bomb_attacker(Role::Attack, &unknown));
        assert!(!bomb_attacker(Role::Defend, &unknown));
        assert!(!bomb_attacker(
            Role::Attack,
            &Situation {
                carrier_team: Some(1),
                ..unknown
            }
        ));
    }

    /// BOT-103: a bot at its objective stands; it walks again only past the resume margin; a zero radius never
    /// stands.
    #[test]
    fn a_bot_stands_at_its_objective_with_a_margin() {
        let o = Objective::at([0.0; 3], SITE_HOLD);
        assert!(stands(&o, SITE_HOLD - 1.0, false));
        assert!(!stands(&o, SITE_HOLD + 10.0, false));
        assert!(stands(&o, SITE_HOLD + 10.0, true));
        assert!(!stands(&o, SITE_HOLD + RESUME_MARGIN + 1.0, true));
        assert!(!stands(&Objective::at([0.0; 3], 0.0), 0.0, true));
    }

    /// BOT-125: the flag jobs come from the team's roster order, whatever the nicks: 2 of 4 defend, spread over the
    /// ring; a defender returns its flag when it is off its base and guards its place while it is home; an
    /// attacker is left to flagrun.
    #[test]
    fn flag_jobs_follow_the_roster_order_and_defenders_return_their_flag() {
        let team = [1u8, 3, 4, 6];
        let jobs: Vec<FlagJob> = team.iter().map(|p| flag_job(&team, *p).unwrap()).collect();
        let roles: Vec<Role> = jobs.iter().map(|j| j.role).collect();
        assert_eq!(
            roles,
            [Role::Attack, Role::Defend, Role::Attack, Role::Defend]
        );
        assert_eq!((jobs[1].guard, jobs[3].guard, jobs[1].guards), (0, 1, 2));
        assert_eq!(flag_job(&team, 9), None);
        assert_eq!(flag_job(&[5], 5).map(|j| j.role), Some(Role::Attack));
        // the two guards stand on opposite sides of the flag, never on it; the ring turns
        let flag = [100.0, 0.0, 100.0];
        let (a, b) = (
            flag_guard_point(flag, jobs[1], 0),
            flag_guard_point(flag, jobs[3], 0),
        );
        assert!((flat(a, flag) - FLAG_GUARD_RING).abs() < 0.5);
        assert!((flat(a, b) - 2.0 * FLAG_GUARD_RING).abs() < 0.5);
        assert!(flat(a, flag_guard_point(flag, jobs[1], 1)) > 100.0);
        let s = Situation {
            class: Some(7),
            own_flag: Some([5.0, 0.0, 5.0]),
            flag_job: Some(jobs[1]),
            flag_guard: Some(a),
            ..Default::default()
        };
        // the nick says nothing: the job decides
        assert_eq!(
            objective_goal(Role::Attack, &s),
            Some(Objective::at(a, FLAG_GUARD_STAND))
        );
        let away = Situation {
            own_flag_away: true,
            ..s.clone()
        };
        assert_eq!(
            objective_goal(Role::Attack, &away),
            Some(Objective::at([5.0, 0.0, 5.0], 0.0)),
            "the dropped flag is walked to and touched"
        );
        let attacker = Situation {
            flag_job: Some(jobs[0]),
            ..s
        };
        assert_eq!(objective_goal(Role::Defend, &attacker), None);
    }

    /// BOT-138: a guard place moves onto a node of the ring's band, so guards at opposite ring points in a walled
    /// flag room do not share the room's one node; with no node in the band yet none (the ring point itself).
    #[test]
    fn guard_places_snap_onto_the_ring_band() {
        let flag = [0.0, 0.0, 0.0];
        // a corridor along x beside the flag room: one node on the flag, the band nodes on either side
        let nodes = [
            [0.0, 0.0, 20.0],
            [-300.0, 0.0, 40.0],
            [300.0, 0.0, 40.0],
            [0.0, 400.0, 300.0],
        ];
        let east = snap_guard(flag, [0.0, 0.0, 300.0], &nodes, &[]).unwrap();
        assert!(flat_dist(east, flag) >= GUARD_NEAR, "{east:?}");
        assert_eq!(
            snap_guard(flag, [290.0, 0.0, 0.0], &nodes, &[]),
            Some([300.0, 0.0, 40.0])
        );
        assert_eq!(
            snap_guard(flag, [-290.0, 0.0, 0.0], &nodes, &[]),
            Some([-300.0, 0.0, 40.0])
        );
        assert_eq!(
            snap_guard(flag, [0.0, 0.0, 1.0], &nodes[..1], &[]),
            None,
            "no band node: the ring point itself"
        );
        assert_eq!(snap_guard(flag, [0.0, 0.0, 1.0], &[], &[]), None);
        // at a turn the guard leaves its last place for another node of the band, even when the ring point is nearer
        // the old one
        let last = [300.0, 0.0, 40.0];
        assert_eq!(
            snap_guard(flag, [290.0, 0.0, 0.0], &nodes, &[last]),
            Some([-300.0, 0.0, 40.0])
        );
        assert_eq!(
            snap_guard(flag, [290.0, 0.0, 0.0], &nodes[..3], &[[0.0; 3]]),
            Some([300.0, 0.0, 40.0])
        );
        // BOT-144: a band grown on one side only (a cluster within GUARD_MOVE of the last place) gives no node: the
        // ring point itself, not the same node again
        let cluster = [[300.0, 0.0, 40.0], [320.0, 0.0, 100.0], [260.0, 0.0, -60.0]];
        assert_eq!(
            snap_guard(flag, [-300.0, 0.0, 0.0], &cluster, &[last]),
            None
        );
        // and a node near the team's other guard is not taken either
        let mate = [-300.0, 0.0, 40.0];
        assert_eq!(
            snap_guard(flag, [-290.0, 0.0, 0.0], &nodes, &[last, mate]),
            None
        );
        let mut wide = nodes.to_vec();
        wide.push([0.0, 0.0, -300.0]);
        assert_eq!(
            snap_guard(flag, [-290.0, 0.0, 0.0], &wide, &[last, mate]),
            Some([0.0, 0.0, -300.0])
        );
        // the last guard of two may rove, a single guard and the first of two stay
        let g = |guard, guards| FlagJob {
            role: Role::Defend,
            guard,
            guards,
        };
        assert!(may_rove(g(1, 2)) && !may_rove(g(0, 2)) && !may_rove(g(0, 1)));
    }

    #[test]
    fn flag_defenders_guard_the_own_flag_and_elim_last_alive_is_cautious() {
        let s = Situation {
            class: Some(7),
            own_flag: Some([5.0, 0.0, 5.0]),
            ..Default::default()
        };
        assert_eq!(
            objective_goal(Role::Defend, &s),
            Some(Objective::at([5.0, 0.0, 5.0], FLAG_GUARD))
        );
        assert_eq!(objective_goal(Role::Attack, &s), None, "flagrun decides");
        assert_eq!(
            objective_goal(
                Role::Defend,
                &Situation {
                    own_carries_flag: true,
                    ..s.clone()
                }
            ),
            None
        );
        assert!(cautious(&Situation {
            class: Some(12),
            mates_alive: 0,
            ..Default::default()
        }));
        assert!(!cautious(&Situation {
            class: Some(12),
            mates_alive: 1,
            ..Default::default()
        }));
        assert!(!cautious(&Situation {
            class: Some(1),
            mates_alive: 0,
            ..Default::default()
        }));
        // BOT-130: the last one holds for a while, then hunts; at once when the other side is down to one too
        let last = Situation {
            class: Some(12),
            mates_alive: 0,
            enemies_alive: 2,
            ..Default::default()
        };
        assert!(cautious_holds(&last, 0));
        assert!(cautious_holds(&last, CAUTIOUS_HOLD_MS - 1));
        assert!(!cautious_holds(&last, CAUTIOUS_HOLD_MS), "it hunts again");
        assert!(!cautious_holds(
            &Situation {
                enemies_alive: 1,
                ..last.clone()
            },
            0
        ));
        assert!(!cautious_holds(
            &Situation {
                mates_alive: 1,
                ..last
            },
            0
        ));
        assert_eq!(
            objective_goal(
                Role::Defend,
                &Situation {
                    class: Some(1),
                    ..Default::default()
                }
            ),
            None
        );
    }
}
