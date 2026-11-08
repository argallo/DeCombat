//! Quarantine for a human bot (mode class 11, team 1): keep away from the infected (their claws reach about 120
//! units; a human shoots from afar), fall back to a place it can shoot from, and stay with the other humans. The
//! infected (team 2) rush the humans with the plain hunt. A DeCombat bot behaviour, not a 2009 rule.
// DC-DEFAULT: BOT-128 (proposed)

/// The class of the mode and the two teams (dc-dserver `infect`: every player joins as a human, team 1; an infected
/// player is team 2).
pub const QRT_CLASS: u8 = 11;
pub const HUMANS: u8 = 1;
pub const INFECTED: u8 = 2;

/// BOT-128: an infected within this flat distance makes the human fall back.
// DC-DEFAULT: BOT-128 (proposed)
pub const KEEP_AWAY: f32 = 400.0;
/// Within this flat distance of an infected a human never hunts toward it (it holds or regroups and shoots).
pub const NO_HUNT: f32 = 650.0;
/// The places a human falls back to: grid nodes between these flat distances from it, at most this much higher or
/// lower.
pub const RETREAT_MIN: f32 = 160.0;
pub const RETREAT_MAX: f32 = 900.0;
pub const RETREAT_HEIGHT: f32 = 160.0;
/// How long a picked place stands before the bot picks again, and the most places whose sight line is tested.
pub const RETREAT_MS: u32 = 1000;
pub const RETREAT_RAYS: usize = 48;
/// With no grid place the bot walks this far straight away from the infected.
pub const RETREAT_STRAIGHT: f32 = 500.0;
/// A human farther than [`REGROUP_FAR`] from the other humans' middle walks back to it until within
/// [`REGROUP_NEAR`].
pub const REGROUP_FAR: f32 = 600.0;
pub const REGROUP_NEAR: f32 = 250.0;

/// BOT-143: the Quarantine shell (0xd3) and its sub for the own override row (`[3][row:i8]`, dc-dserver
/// `infect::override_message`; -1 = a human again).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.5 row 0xd3
pub const QRT_SHELL: u8 = 0xd3;
pub const QRT_SUB_OVERRIDE: u8 = 3;
/// BOT-143: a damaging hit on an infected body slows it this long by this factor, as the client's mover does
/// (CLI-1760); a fall (damage kind 0) does not.
// DC-DEFAULT: BOT-143 (proposed) mirrors CLI-1760: 400 ms, x 0.5
pub const SHOT_SLOW_MS: u32 = 400;
pub const SHOT_SLOW_FACTOR: f32 = 0.5;
pub const FALL_DAMAGE_KIND: u8 = 0;
/// BOT-143: the damage type a hazard volume hurts with (dc-dserver `killvol::DAMAGE_TYPE_POISON`, the 0x74 `zone`).
pub const HAZARD_DAMAGE_TYPE: u8 = 9;
/// BOT-143: after a hazard hit the bot walks to a safe grid place for this long.
// DC-DEFAULT: BOT-143 (proposed) 4 s, a safe place within 900 units
pub const HAZARD_LEAVE_MS: u32 = 4000;
pub const HAZARD_LEAVE_RANGE: f32 = 900.0;
/// BOT-143: an infected bot holds the sprint key while it walks to a human within this flat distance; with a spent
/// gauge it lets go until the gauge is back to this fraction.
// DC-DEFAULT: BOT-143 (proposed) 400 units, 0.5
pub const SPRINT_RANGE: f32 = 400.0;
pub const SPRINT_RESUME: f32 = 0.5;
/// BOT-143: the regroup place is the safe grid node nearest the humans' middle within this flat distance (else the
/// living human nearest the middle).
// DC-DEFAULT: BOT-143 (proposed) 400 units
pub const REGROUP_SNAP: f32 = 400.0;

/// BOT-143: after the outbreak a human with the group guards: a grid place within [`GUARD_RADIUS`] of the humans'
/// middle (on about its level), a new one every [`GUARD_MS`]; at [`GUARD_REACHED`] it stands there.
// DC-DEFAULT: BOT-143 (proposed) 250 units, 6 s, 48 units
pub const GUARD_RADIUS: f32 = 250.0;
pub const GUARD_MS: u32 = 6000;
pub const GUARD_REACHED: f32 = 48.0;

/// BOT-143: the guard place picked by `pick` among `nodes` (in their order); `None` without nodes.
pub fn guard_post(nodes: &[[f32; 3]], pick: u64) -> Option<[f32; 3]> {
    (!nodes.is_empty()).then(|| nodes[(pick % nodes.len() as u64) as usize])
}

/// BOT-143: whether the infected bot holds the sprint key: it chases (`chasing`), and its gauge (`gauge` = value and
/// full, `None`: no gauge rule) is not resting; a spent or empty gauge rests (`rest`) until it is back to
/// [`SPRINT_RESUME`] of full.
pub fn sprint_key(
    chasing: bool,
    gauge: Option<(f32, i32)>,
    mode: dc_sim::GaugeMode,
    rest: &mut bool,
) -> bool {
    if let Some((v, full)) = gauge {
        if mode == dc_sim::GaugeMode::Spent || v < 1.0 {
            *rest = true;
        } else if *rest && v >= SPRINT_RESUME * full as f32 {
            *rest = false;
        }
    }
    chasing && !*rest
}

/// BOT-143: the human an infected bot with no hunt goal walks to: the nearest living one it knows of (flat).
pub fn chase(at: [f32; 3], humans: &[[f32; 3]]) -> Option<[f32; 3]> {
    nearest(at, humans).map(|h| h.0)
}

/// BOT-143: the regroup place for the humans' middle `mid`: the safe grid place `snap` found for it, else the human
/// of `mates` nearest to it (a place a player stands), else none.
pub fn regroup_place(
    mid: Option<[f32; 3]>,
    mates: &[[f32; 3]],
    snap: impl Fn([f32; 3]) -> Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    let m = mid?;
    snap(m).or_else(|| nearest(m, mates).map(|n| n.0))
}

/// BOT-150: how far ahead (seconds) a Quarantine walk is run on a copy for the danger test.
// DC-DEFAULT: BOT-150 (proposed) 0.5 s, 5 samples, every 250 ms
pub const WALK_GUARD_SECS: f32 = 0.5;

/// BOT-150: the infected the humans fire at first: the one nearest the humans' middle `mid` (the bot's own place
/// among them), so the group's fire meets on one body.
// DC-DEFAULT: BOT-150 (proposed)
pub fn focus(mid: [f32; 3], infected: &[(u8, [f32; 3])]) -> Option<u8> {
    infected
        .iter()
        .min_by(|a, b| {
            flat(a.1, mid)
                .total_cmp(&flat(b.1, mid))
                .then(a.0.cmp(&b.0))
        })
        .map(|i| i.0)
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

/// The nearest of `points` to `at` (flat) and its distance.
pub fn nearest(at: [f32; 3], points: &[[f32; 3]]) -> Option<([f32; 3], f32)> {
    points
        .iter()
        .map(|p| (*p, flat(*p, at)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// The middle of `points`.
pub fn centroid(points: &[[f32; 3]]) -> Option<[f32; 3]> {
    if points.is_empty() {
        return None;
    }
    let n = points.len() as f32;
    let s = points
        .iter()
        .fold([0.0f32; 3], |s, p| [s[0] + p[0], s[1] + p[1], s[2] + p[2]]);
    Some([s[0] / n, s[1] / n, s[2] / n])
}

/// BOT-128: where a human at `at` falls back to from the nearest infected `threat` (the others are `infected`):
/// of the grid places `nodes`, one [`RETREAT_MIN`]..[`RETREAT_MAX`] away on about its level that leaves the threat
/// farther behind than the bot is now and does not lie past it; the best is the farthest from every infected (up
/// to 1200 units), with a sight line to the threat (`sees`, place to threat) worth 300 units and the humans' middle
/// `mates` pulling it in (0.3 per unit). Without such a place: straight away from the threat.
pub fn retreat(
    at: [f32; 3],
    threat: [f32; 3],
    infected: &[[f32; 3]],
    mates: Option<[f32; 3]>,
    nodes: &[[f32; 3]],
    sees: impl Fn([f32; 3], [f32; 3]) -> bool,
) -> [f32; 3] {
    let now_d = flat(at, threat);
    let away = [at[0] - threat[0], at[2] - threat[2]];
    let mut cand: Vec<[f32; 3]> = nodes
        .iter()
        .copied()
        .filter(|n| {
            let d = flat(*n, at);
            (RETREAT_MIN..=RETREAT_MAX).contains(&d)
                && (n[1] - at[1]).abs() <= RETREAT_HEIGHT
                && flat(*n, threat) >= now_d + RETREAT_MIN * 0.5
                && (n[0] - at[0]) * away[0] + (n[2] - at[2]) * away[1] >= 0.0
        })
        .collect();
    if cand.len() > RETREAT_RAYS {
        let step = cand.len() as f32 / RETREAT_RAYS as f32;
        cand = (0..RETREAT_RAYS)
            .map(|i| cand[(i as f32 * step) as usize])
            .collect();
    }
    let score = |n: [f32; 3]| {
        let room = infected
            .iter()
            .map(|p| flat(n, *p))
            .fold(f32::INFINITY, f32::min)
            .min(1200.0);
        let sight = if sees(n, threat) { 300.0 } else { 0.0 };
        let pull = mates.map_or(0.0, |m| 0.3 * flat(n, m));
        room + sight - pull
    };
    let best = cand
        .into_iter()
        .map(|n| (score(n), n))
        .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1[0].total_cmp(&a.1[0])));
    if let Some((_, n)) = best {
        return n;
    }
    let len = away[0].hypot(away[1]).max(1.0);
    [
        at[0] + away[0] / len * RETREAT_STRAIGHT,
        at[1],
        at[2] + away[1] / len * RETREAT_STRAIGHT,
    ]
}

/// What a human does in Quarantine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HumanMove {
    /// Fall back from an infected inside [`KEEP_AWAY`].
    KeepAway,
    /// An infected inside [`NO_HUNT`]: no hunt toward it; regroup (or hold) and shoot.
    Hold,
    /// No infected near: regroup when far from the humans' middle, else the usual hunt / roam.
    Free,
}

/// BOT-128: the human's move from the nearest living infected's distance.
pub fn human_move(threat: Option<f32>) -> HumanMove {
    match threat {
        Some(d) if d <= KEEP_AWAY => HumanMove::KeepAway,
        Some(d) if d <= NO_HUNT => HumanMove::Hold,
        _ => HumanMove::Free,
    }
}

/// BOT-128: whether the human at `at` walks to the humans' middle `mid` (`regrouping`: it does already; it walks
/// until within [`REGROUP_NEAR`], it starts beyond [`REGROUP_FAR`]).
pub fn regroup(at: [f32; 3], mid: Option<[f32; 3]>, regrouping: bool) -> Option<[f32; 3]> {
    let m = mid?;
    let d = flat(at, m);
    (d > REGROUP_FAR || (regrouping && d > REGROUP_NEAR)).then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BOT-128: an infected inside 400 units makes the human fall back, inside 650 it does not hunt toward it.
    #[test]
    fn a_human_falls_back_from_a_near_infected_and_never_hunts_toward_it() {
        assert_eq!(human_move(Some(120.0)), HumanMove::KeepAway);
        assert_eq!(human_move(Some(400.0)), HumanMove::KeepAway);
        assert_eq!(human_move(Some(500.0)), HumanMove::Hold);
        assert_eq!(human_move(Some(900.0)), HumanMove::Free);
        assert_eq!(human_move(None), HumanMove::Free);
    }

    /// BOT-128: the fall-back place lies away from the infected (never past it), prefers a sight line to it and the
    /// other humans; with no place on the grid the bot walks straight away.
    #[test]
    fn the_fall_back_place_is_away_from_the_infected_with_a_sight_line() {
        let at = [0.0, 0.0, 0.0];
        let threat = [300.0, 0.0, 0.0];
        let nodes = [
            [600.0, 0.0, 0.0],    // past the infected
            [-500.0, 0.0, 0.0],   // away, no sight line
            [-450.0, 0.0, 200.0], // away, in sight
            [-500.0, 400.0, 0.0], // another floor
            [-50.0, 0.0, 0.0],    // too near
        ];
        let sees = |n: [f32; 3], _t: [f32; 3]| n[2] > 100.0;
        assert_eq!(
            retreat(at, threat, &[threat], None, &nodes, sees),
            [-450.0, 0.0, 200.0]
        );
        // the humans' middle pulls toward them
        let nodes = [[-500.0, 0.0, 600.0], [-500.0, 0.0, -600.0]];
        let mates = Some([-300.0, 0.0, -700.0]);
        assert_eq!(
            retreat(at, threat, &[threat], mates, &nodes, |_, _| true),
            [-500.0, 0.0, -600.0]
        );
        // nothing on the grid: straight away
        let g = retreat(at, threat, &[threat], None, &[], |_, _| true);
        assert!(
            (g[0] + RETREAT_STRAIGHT).abs() < 1e-3 && g[2].abs() < 1e-3,
            "{g:?}"
        );
    }

    /// BOT-143: the sprint key holds while chasing until the gauge is spent, then rests until half full.
    #[test]
    fn the_sprint_rests_a_spent_gauge_until_half_full() {
        use dc_sim::GaugeMode::*;
        let mut rest = false;
        assert!(sprint_key(true, Some((80.0, 100)), Sprint, &mut rest));
        assert!(!sprint_key(false, Some((80.0, 100)), Rest, &mut rest));
        assert!(!sprint_key(true, Some((0.0, 100)), Spent, &mut rest) && rest);
        assert!(!sprint_key(true, Some((30.0, 100)), Rest, &mut rest));
        assert!(sprint_key(true, Some((50.0, 100)), Rest, &mut rest) && !rest);
        assert!(sprint_key(true, None, Sprint, &mut rest), "no gauge rule");
    }

    /// BOT-143: the regroup place is the snapped safe place, else the human nearest the middle; an infected with no
    /// hunt goal chases the nearest human.
    #[test]
    fn the_regroup_place_is_a_safe_place_and_the_infected_chase_the_nearest_human() {
        let mates = [[0.0, 0.0, 0.0], [1000.0, 0.0, 0.0], [600.0, 0.0, 0.0]];
        let mid = centroid(&mates);
        assert_eq!(
            regroup_place(mid, &mates, |_| Some([500.0, 10.0, 0.0])),
            Some([500.0, 10.0, 0.0])
        );
        assert_eq!(
            regroup_place(mid, &mates, |_| None),
            Some([600.0, 0.0, 0.0])
        );
        assert_eq!(regroup_place(None, &mates, |_| None), None);
        assert_eq!(chase([900.0, 0.0, 0.0], &mates), Some([1000.0, 0.0, 0.0]));
        assert_eq!(chase([0.0; 3], &[]), None);
    }

    /// ROM-gated (skipped without CA_ROM): with the ROM speed rows every infected override row that moves faster
    /// (MovePenalty below -0.01) gives a lower penalty than a human with a rifle, and a hit's slow makes it slower.
    #[test]
    fn rom_the_infected_override_rows_are_faster_than_a_human() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let rows = dc_sim::SpeedTable::from_vfs(&dc_rez::RezVfs::new(&rom)).expect("speed rows");
        let human = crate::motion::loadout_mods(&rows, Some(145), (None, None, None), None, false);
        let n = rows.bodies.len() - rows.override_base;
        let mut faster = 0;
        for slot in 0..n as u32 {
            let o = rows.bodies[rows.override_base + slot as usize];
            if o.penalty >= -0.01 {
                continue;
            }
            let m = crate::motion::loadout_mods(&rows, None, (None, None, None), Some(slot), false);
            eprintln!(
                "override {slot}: penalty {} sprint add {} mul {:?} stamina {:?} (human {} / {:?})",
                m.penalty, m.sprint_add, m.sprint_mul, m.stamina, human.penalty, human.stamina
            );
            assert!(
                m.penalty < human.penalty - 0.1,
                "{slot}: {m:?} vs {human:?}"
            );
            eprintln!("claws 191: {:?}", rows.weapons.get(&191));
            assert!(
                crate::motion::loadout_mods(&rows, None, (None, None, None), Some(slot), true)
                    .penalty
                    > m.penalty
            );
            faster += 1;
        }
        assert!(faster >= 2, "the infected and host rows");
    }

    /// BOT-150: the humans fire first at the infected nearest their middle (ties: the lower pid).
    #[test]
    fn the_humans_focus_the_infected_nearest_their_middle() {
        let mid = [0.0, 0.0, 0.0];
        let inf = [
            (5, [300.0, 0.0, 0.0]),
            (2, [-200.0, 0.0, 0.0]),
            (7, [0.0, 0.0, 200.0]),
        ];
        assert_eq!(focus(mid, &inf), Some(2));
        assert_eq!(focus(mid, &inf[..1]), Some(5));
        assert_eq!(focus(mid, &[]), None);
    }

    /// BOT-128: a human far from the others walks back to them and stops when near.
    #[test]
    fn a_human_far_from_the_others_regroups() {
        let mid = centroid(&[[1000.0, 0.0, 0.0], [1200.0, 0.0, 0.0]]);
        assert_eq!(mid, Some([1100.0, 0.0, 0.0]));
        assert_eq!(regroup([0.0; 3], mid, false), mid);
        assert_eq!(regroup([600.0, 0.0, 0.0], mid, false), None);
        assert_eq!(regroup([600.0, 0.0, 0.0], mid, true), mid);
        assert_eq!(regroup([900.0, 0.0, 0.0], mid, true), None);
        assert_eq!(regroup([0.0; 3], None, true), None);
    }
}
