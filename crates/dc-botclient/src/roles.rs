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
    /// Teammates still standing (streamed and not parked), the bot itself not counted.
    pub mates_alive: usize,
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
        7 | 8 => {
            if role == Role::Defend && !s.own_carries_flag {
                s.own_flag.map(|p| Objective::at(p, FLAG_GUARD))
            } else {
                None
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

/// ELIM (no respawn): the last one standing of its team does not hunt; it holds where it is and fights what it
/// sees.
pub fn cautious(s: &Situation) -> bool {
    matches!(s.class, Some(12 | 13)) && s.mates_alive == 0
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
