//! `dc-mapkit walk --sight`: which team spawns see an enemy spawn, and from where outside its own base a player can
//! see a spawn, upper positions counted apart (DC-DEFAULT: MAP-126).
//!
//! It runs on the walk world of [`crate::walk`] (the client's collision). The standing positions are the nodes of the
//! reach grid ([`crate::reach::find_nodes`]): every floor of every column a standing body fits on, deadly ones left
//! out, kept to the main component of the reach analysis ([`crate::reach`], seeded by the player spawns): floors players
//! can get to and back from, not the hollow under a slab. A spawn's body is placed as a walk's start is; its eye is [`EYE`] over its feet. A spawn sees an enemy spawn
//! when the segment between the two eyes meets no collision face. A position sees a spawn when the segment from the
//! position's eye to the spawn's eye or to its chest ([`CHEST`] over the feet) meets none, the position lies within
//! `range` of the spawn's eye and farther than `own_radius` (across) from every spawn of the spawn's own team (its
//! base). A position whose floor is at least `upper` over the spawn's floor is an upper position.

use crate::cli::Outcome;
use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::project::{Project, PropValue};
use crate::reach::{
    analyse as reach_analyse, find_moves, find_nodes, node_of_point, parallel, pick_most, threads,
    Class, Grid, Nodes,
};
use crate::render_sw::{draw_disc, draw_line};
use crate::walk::{self, top_picture, write_png, WalkRequest, WalkWorld, BODY};
use dc_sim::{Cylinder, MovementParams, V3};
use serde_json::{json, Value};

/// The sight grid's spacing unless `--step` says otherwise.
// DC-DEFAULT: MAP-126
pub const SIGHT_STEP: f32 = 96.0;
/// How far a position may be from a spawn's eye and still count.
// DC-DEFAULT: MAP-126
pub const SIGHT_RANGE: f32 = 4000.0;
/// Positions within this distance (across) of a spawn of the same team are its base and do not count.
// DC-DEFAULT: MAP-126
pub const OWN_RADIUS: f32 = 700.0;
/// A floor this much higher than the spawn's is an upper position.
// DC-DEFAULT: MAP-126
pub const UPPER: f32 = 150.0;
/// The eye over the feet of a standing body (the body is 100 tall).
// DC-DEFAULT: MAP-126
pub const EYE: f32 = 80.0;
/// The chest over the feet: a position that sees it sees the spawned player.
// DC-DEFAULT: MAP-126
pub const CHEST: f32 = 50.0;
/// Most spawns reported with a warning.
const WARN_CAP: usize = 20;

/// The `--sight` job.
#[derive(Debug, Clone, PartialEq)]
pub struct SightJob {
    pub area: Option<walk::AreaArg>,
    pub step: f32,
    pub range: f32,
    pub own_radius: f32,
    pub upper: f32,
}

impl Default for SightJob {
    fn default() -> Self {
        SightJob {
            area: None,
            step: SIGHT_STEP,
            range: SIGHT_RANGE,
            own_radius: OWN_RADIUS,
            upper: UPPER,
        }
    }
}

/// A team spawn of the project, placed on its floor.
#[derive(Debug, Clone)]
pub struct TeamSpawn {
    pub id: String,
    pub team: String,
    pub feet: V3,
}

/// The project's player spawns with team "1" or "2", each placed as a walk's start is (`None` feet: no floor).
pub fn team_spawns(p: &Project, w: &WalkWorld) -> Vec<TeamSpawn> {
    let shape = Cylinder::from_dims(V3::from_array(BODY));
    let mp = MovementParams::DEFAULT;
    p.objects
        .iter()
        .filter(|o| o.class == crate::vocab_names::CLASS_PLAYER_SPAWN)
        .filter_map(|o| {
            let team = match o.props.get("team") {
                Some(PropValue::Text(t)) => t.trim().to_string(),
                Some(PropValue::Num(n)) => format!("{n}"),
                _ => String::new(),
            };
            if team != "1" && team != "2" {
                return None;
            }
            let body = walk::place(w, &shape, &mp, V3::from_array(o.pos));
            Some(TeamSpawn {
                id: o.id.clone(),
                team,
                feet: body - V3::new(0.0, BODY[1], 0.0),
            })
        })
        .collect()
}

fn clear(w: &WalkWorld, a: V3, b: V3) -> bool {
    w.collision.ray(a, b).is_none()
}

fn across(a: V3, b: V3) -> f32 {
    ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

/// What one spawn sees and is seen from.
#[derive(Debug, Clone, Default)]
pub struct SpawnSight {
    pub enemy_spawns: Vec<usize>,
    /// Node indices of the positions that see it.
    pub seen_from: Vec<u32>,
    pub seen_upper: usize,
    pub nearest: Option<(u32, f32)>,
    pub nearest_upper: Option<(u32, f32)>,
}

/// The sight of every spawn against the nodes `watch` allows.
pub fn analyse(
    w: &WalkWorld,
    nodes: &Nodes,
    watch: &[bool],
    spawns: &[TeamSpawn],
    job: &SightJob,
) -> Vec<SpawnSight> {
    let eye = V3::new(0.0, EYE, 0.0);
    let chest = V3::new(0.0, CHEST, 0.0);
    parallel(spawns.len(), threads(), |k| {
        let s = &spawns[k];
        let mut out = SpawnSight::default();
        for (j, e) in spawns.iter().enumerate() {
            if e.team != s.team && clear(w, s.feet + eye, e.feet + eye) {
                out.enemy_spawns.push(j);
            }
        }
        for (n, (nd, &ok)) in nodes.list.iter().zip(watch).enumerate() {
            if nd.kill.is_some() || !ok {
                continue;
            }
            let at = nodes.pos(n);
            if spawns
                .iter()
                .any(|o| o.team == s.team && across(o.feet, at) <= job.own_radius)
            {
                continue;
            }
            let from = at + eye;
            let d = (from - (s.feet + eye)).len();
            if d > job.range {
                continue;
            }
            if !(clear(w, from, s.feet + eye) || clear(w, from, s.feet + chest)) {
                continue;
            }
            out.seen_from.push(n as u32);
            if out.nearest.is_none_or(|(_, b)| d < b) {
                out.nearest = Some((n as u32, d));
            }
            if nd.floor - s.feet.y >= job.upper {
                out.seen_upper += 1;
                if out.nearest_upper.is_none_or(|(_, b)| d < b) {
                    out.nearest_upper = Some((n as u32, d));
                }
            }
        }
        out
    })
}

fn r1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

fn pt(v: V3) -> [f64; 3] {
    [r1(v.x), r1(v.y), r1(v.z)]
}

/// The `walk --sight` answer.
#[allow(clippy::too_many_arguments)]
pub fn report(
    req: &WalkRequest,
    p: &Project,
    world: &dc_world::World<'_>,
    w: &WalkWorld,
    area: [f32; 4],
    job: &SightJob,
    world_info: Value,
    mut diags: Diagnostics,
) -> Result<Outcome, Diagnostics> {
    let invalid =
        |path: &str, msg: String| Diagnostics::one(Diagnostic::new(Code::OpInvalid, path, msg));
    for (v, name) in [
        (job.range, "--range"),
        (job.own_radius, "--own-radius"),
        (job.upper, "--upper"),
    ] {
        if !(v.is_finite() && v >= 0.0) {
            return Err(invalid(name, format!("{v} (a number of at least 0)")));
        }
    }
    let grid = Grid::new(area, job.step).map_err(|e| invalid("--box", e))?;
    let nodes = find_nodes(w, grid, threads()).map_err(|e| invalid("--step", e))?;
    let spawns = team_spawns(p, w);
    // the watchers: the main component of the reach analysis, seeded by every player spawn in the box
    let moves = find_moves(w, &nodes, threads());
    let seeds: Vec<u32> = crate::reach::spawns(world)
        .iter()
        .filter_map(|s| node_of_point(w, &nodes, s.pos))
        .collect();
    let an = reach_analyse(&nodes, &moves, &|c: &[u32], z: &[u32]| {
        pick_most(&seeds, c, z)
    });
    let watch: Vec<bool> = an.class.iter().map(|&k| k == Class::Main).collect();
    let watchers = watch.iter().filter(|b| **b).count();
    let sights = analyse(w, &nodes, &watch, &spawns, job);
    let near = |x: Option<(u32, f32)>| {
        x.map(|(n, d)| json!({"at": pt(nodes.pos(n as usize)), "dist": r1(d)}))
    };
    let mut rows = Vec::new();
    let mut text = Vec::new();
    let (mut seeing, mut seen, mut seen_up, mut warned) = (0usize, 0usize, 0usize, 0usize);
    for (s, v) in spawns.iter().zip(&sights) {
        let enemy: Vec<&str> = v
            .enemy_spawns
            .iter()
            .map(|&j| spawns[j].id.as_str())
            .collect();
        seeing += usize::from(!enemy.is_empty());
        seen += usize::from(!v.seen_from.is_empty());
        seen_up += usize::from(v.seen_upper > 0);
        if !enemy.is_empty() || !v.seen_from.is_empty() {
            let msg = format!(
                "{} (team {}) sees {} enemy spawn(s); seen from {} position(s) outside its base, {} upper{}",
                s.id,
                s.team,
                enemy.len(),
                v.seen_from.len(),
                v.seen_upper,
                v.nearest
                    .map(|(n, d)| {
                        let q = nodes.pos(n as usize);
                        format!(", nearest {:.0} away at {:.0},{:.0},{:.0}", d, q.x, q.y, q.z)
                    })
                    .unwrap_or_default()
            );
            text.push(msg.clone());
            if warned < WARN_CAP {
                diags.push(Diagnostic::new(
                    Code::WalkSight,
                    format!("objects.{}", s.id),
                    msg,
                ));
                warned += 1;
            }
        }
        rows.push(json!({
            "id": s.id,
            "team": s.team,
            "feet": pt(s.feet),
            "enemy_spawns_seen": enemy,
            "seen_from": v.seen_from.len(),
            "seen_from_upper": v.seen_upper,
            "nearest": near(v.nearest),
            "nearest_upper": near(v.nearest_upper),
        }));
    }
    text.insert(
        0,
        format!(
            "{} team spawns, {} positions ({} reachable): {} see an enemy spawn, {} are seen from outside their base, {} from upper positions",
            spawns.len(),
            nodes.len(),
            watchers,
            seeing,
            seen,
            seen_up
        ),
    );
    let out = req
        .out
        .clone()
        .unwrap_or_else(|| req.dir.join("build").join("walk").join("sight.png"));
    let size = req.size.unwrap_or_else(|| walk::picture_size(area));
    let img = top_picture(world, p, area, size, &mut |img, map| {
        // every position that sees some spawn, then a line from each spawn to its nearest watcher
        let mut seen_by = vec![false; nodes.len()];
        for v in &sights {
            for &n in &v.seen_from {
                seen_by[n as usize] = true;
            }
        }
        for (n, &b) in seen_by.iter().enumerate() {
            if b {
                let q = nodes.pos(n);
                let (x, y) = map(q.x, q.z);
                draw_disc(img, x as i32, y as i32, 2, [230, 60, 40], [230, 60, 40]);
            }
        }
        for (s, v) in spawns.iter().zip(&sights) {
            let a = map(s.feet.x, s.feet.z);
            for &j in &v.enemy_spawns {
                let e = &spawns[j];
                draw_line(img, a, map(e.feet.x, e.feet.z), [255, 0, 255], 0.9);
            }
            if let Some((n, _)) = v.nearest {
                let q = nodes.pos(n as usize);
                draw_line(img, a, map(q.x, q.z), [255, 220, 0], 0.9);
            }
            let fill = if s.team == "1" {
                [70, 130, 255]
            } else {
                [255, 90, 70]
            };
            draw_disc(img, a.0 as i32, a.1 as i32, 5, fill, [20, 20, 20]);
        }
    })?;
    write_png(&out, &img)?;
    let result = json!({
        "world": world_info,
        "box": area,
        "step": job.step,
        "range": job.range,
        "own_radius": job.own_radius,
        "upper": job.upper,
        "eye": EYE,
        "positions": nodes.len(),
        "watchers": watchers,
        "summary": {"spawns": spawns.len(), "seeing_enemy_spawn": seeing, "seen": seen, "seen_from_upper": seen_up},
        "spawns": rows,
        "image": out.display().to_string(),
        "width": img.width,
        "height": img.height,
    });
    text.push(format!("picture {}", out.display()));
    let mut o = Outcome::new(result, text.join("\n"));
    o.diagnostics = diags;
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two rooms on a flat floor with a wall between them: a spawn of each team, one per room. The wall hides them
    /// from each other and from the other room; a gap in it shows them.
    #[test]
    fn a_wall_hides_spawns_and_a_hole_shows_them() {
        let mut seen = Vec::new();
        for hole in [false, true] {
            let dir = std::env::temp_dir().join(format!(
                "dc-mapkit-sight-{}-{}",
                std::process::id(),
                hole
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("assets")).unwrap();
            let made = crate::template::make(crate::template::Template::Flat, "Sight", "tests", 7);
            for file in &made.files {
                std::fs::write(dir.join(&file.path), &file.bytes).unwrap();
            }
            let (mut p, mut h) = (made.project, made.heights);
            let mut ops = vec![
                json!({"op": "clear", "class": "dc.player_spawn"}),
                json!({"op": "add", "class": "dc.player_spawn", "pos": [-600, 40, 0], "rot": [0, 90, 0], "props": {"team": "1", "modes": ""}}),
                json!({"op": "add", "class": "dc.player_spawn", "pos": [600, 40, 0], "rot": [0, 270, 0], "props": {"team": "2", "modes": ""}}),
            ];
            if hole {
                ops.push(json!({"op": "solid.add", "kind": "box", "pos": [0, 300, 650], "size": [32, 600, 1100], "material": "wall"}));
                ops.push(json!({"op": "solid.add", "kind": "box", "pos": [0, 300, -650], "size": [32, 600, 1100], "material": "wall"}));
            } else {
                ops.push(json!({"op": "solid.add", "kind": "box", "pos": [0, 300, 0], "size": [32, 600, 2400], "material": "wall"}));
            }
            let ops: Vec<crate::ops::Op> = serde_json::from_value(Value::Array(ops)).unwrap();
            crate::ops::apply(&mut p, &mut h, &ops).unwrap();
            let req = WalkRequest {
                dir: dir.clone(),
                rom: None,
                job: walk::Job::Sight(SightJob {
                    area: Some(walk::AreaArg::Box([-900.0, -400.0, 900.0, 400.0])),
                    step: 64.0,
                    range: 3000.0,
                    own_radius: 300.0,
                    upper: 150.0,
                }),
                options: walk::WalkOptions::default(),
                open_doors: false,
                out: Some(dir.join("sight.png")),
                size: None,
            };
            let o = walk::walk_project_with(&req, &p, &h, None).unwrap();
            let s = &o.result["summary"];
            let first = &o.result["spawns"][0];
            assert_eq!(first["team"], "1", "{first}");
            seen.push((
                s["seeing_enemy_spawn"].as_u64().unwrap(),
                first["seen_from"].as_u64().unwrap(),
                first["nearest"]["at"][0].as_f64().unwrap_or(-1.0),
            ));
            let _ = std::fs::remove_dir_all(&dir);
        }
        // behind the wall only its own room (past the 300 units of its base) sees the team 1 spawn
        assert_eq!(
            seen[0].0, 0,
            "the wall hides the spawns from each other: {seen:?}"
        );
        assert!(seen[0].1 > 0 && seen[0].2 < 0.0, "{seen:?}");
        // through the hole they see each other, and the other room sees it too
        assert_eq!(seen[1].0, 2, "{seen:?}");
        assert!(seen[1].1 > seen[0].1, "{seen:?}");
    }
}
