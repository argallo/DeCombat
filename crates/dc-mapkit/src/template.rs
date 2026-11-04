//! The templates `flat`, `arena` and `starter` (MAPKIT_PLAN.md 5.1 template contract; DC-DEFAULT: MAP-12).
//!
//! Every template is built through the same ops an agent would send ([`crate::ops::apply`]), on a 4096 x 4096
//! terrain, with OWN procedural textures only (PNGs generated here with the `image` crate: no ROM path ever
//! enters a scaffolded project, CLEANROOM §4), and satisfies [`check_contract`], which a test pins:
//!
//! - level geometry: the terrain, four boundary walls (group `wall`) whose top is at least 256 units above the
//!   highest terrain point, a sky lid (material with `sky: true`, so its faces are sky faces);
//! - one solid with brush `sky` (the `dc.sky_model`), `settings` with fog on and `far_z` at least the diagonal,
//!   a sun (`sun_bright > 0`, the `dc.global_light`);
//! - one brush `kill_volume` slab whose top lies below the lowest terrain point;
//! - 8 `dc.player_spawn` per team and 16 team-less ones, all with empty `modes`, standing on the floor;
//! - 2 `dc.bomb_site` numbered 0 / 1 with half extents 128 x 48 x 128, 2 `dc.flag_zone` (team 1 / 2) with a
//!   `dc.capture_flag` of the same team inside, 4 `dc.item_spawn`;
//! - `mod_meta` with every mode of [`ALL_MODES`], a 20..500 character description and an author.
//!
//! `flat` = the contract on a flat plane; `arena` = `flat` + two raised bases and mirrored cover; `starter` =
//! `arena` on seeded valley noise. Spawn yaw: team 1 looks along +x, team 2 along -x, the FFA ring faces the
//! centre (yaw 0 = +z, degrees about y).

use crate::diag::Diagnostics;
use crate::geom::sincos_deg;
use crate::ops::{self, Op, Preset, SolidSpec};
use crate::project::{
    key_from_name, BrushRole, Heights, MaterialFlags, Players, Project, PropValue, Terrain,
    TerrainLayer, TextureRef, Vec3f, STAND_OFFSET,
};
use crate::vocab_names as v;
use std::collections::BTreeMap;

/// Modes every template declares.
pub const ALL_MODES: &[&str] = &["TDM", "FFA", "ELIM", "BOMB", "FLAG"];
/// Map side in world units.
pub const SIZE: f32 = 4096.0;
/// Bomb site / flag zone half extents.
pub const ZONE_EXTENT: Vec3f = [128.0, 48.0, 128.0];
/// Wall top above the highest terrain point, at least.
pub const WALL_CLEARANCE: f32 = 256.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    Flat,
    Arena,
    Starter,
}

impl Template {
    pub const ALL: [Template; 3] = [Template::Flat, Template::Arena, Template::Starter];

    pub fn parse(s: &str) -> Option<Template> {
        Some(match s {
            "flat" => Template::Flat,
            "arena" => Template::Arena,
            "starter" => Template::Starter,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Template::Flat => "flat",
            Template::Arena => "arena",
            Template::Starter => "starter",
        }
    }
}

/// A generated project file (project-relative path + bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Made {
    pub project: Project,
    pub heights: Heights,
    pub files: Vec<OwnFile>,
}

/// The textures every template ships: (material id, file, tile, sky flag).
const TEXTURES: &[(&str, &str, f32, bool)] = &[
    ("ground", "assets/ground.png", 256.0, false),
    ("wall", "assets/wall.png", 128.0, false),
    ("block", "assets/block.png", 64.0, false),
    ("sky", "assets/sky.png", 512.0, true),
];

fn text(s: &str) -> PropValue {
    PropValue::Text(s.into())
}

fn add(class: &str, pos: Vec3f, yaw: f32, props: &[(&str, PropValue)]) -> Op {
    Op::Add {
        class: class.into(),
        name: None,
        pos,
        rot: Some([0.0, yaw, 0.0]),
        props: props
            .iter()
            .map(|(k, val)| (k.to_string(), val.clone()))
            .collect(),
        ground: true,
    }
}

fn solid(
    kind: SolidSpec,
    pos: Vec3f,
    size: Vec3f,
    material: &str,
    group: &str,
    brush: Option<BrushRole>,
    ground: bool,
) -> Op {
    Op::SolidAdd {
        kind,
        pos,
        rot_y_deg: 0.0,
        size,
        material: material.into(),
        faces: BTreeMap::new(),
        group: Some(group.into()),
        brush,
        ground,
        rises: None,
    }
}

fn run(p: &mut Project, h: &mut Heights, ops: &[Op]) {
    if let Err(e) = ops::apply(p, h, ops) {
        panic!("template ops failed: {:?}", e.items);
    }
}

/// Make a project from a template. Deterministic: the same arguments give the same project, heights and files.
pub fn make(t: Template, name: &str, author: &str, seed: u64) -> Made {
    let key = key_from_name(name);
    let mut p = Project {
        name: name.to_string(),
        key: key.clone(),
        seed,
        terrain: Some(Terrain {
            layers: vec![TerrainLayer {
                material: "ground".into(),
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    p.mod_meta.id = key;
    p.mod_meta.author = if author.is_empty() {
        "unknown".into()
    } else {
        author.into()
    };
    p.mod_meta.description = format!(
        "{name}: a DeCombat community map made with dc-mapkit from the {} template.",
        t.name()
    );
    p.mod_meta.modes = ALL_MODES.iter().map(|m| m.to_string()).collect();
    p.mod_meta.tags = vec!["map".into(), t.name().into()];
    p.mod_meta.players = Some(Players { min: 2, max: 16 });
    let terrain = p.terrain.clone().expect("set above");
    let mut h = Heights::for_terrain(&terrain, terrain.y_to_raw(0.0));

    // materials
    let mats: Vec<Op> = TEXTURES
        .iter()
        .map(|(id, file, tile, sky)| Op::MaterialSet {
            id: id.to_string(),
            texture: Some(TextureRef::Own(file.to_string())),
            tile: Some(*tile),
            surface_id: Some(0),
            flags: Some(MaterialFlags {
                solid: true,
                no_step: false,
                sky: *sky,
                hidden: false,
                skin: false,
            }),
            fit: None,
        })
        .collect();
    run(&mut p, &mut h, &mats);

    // terrain
    if t == Template::Starter {
        run(
            &mut p,
            &mut h,
            &[Op::TerrainGen {
                preset: Preset::Valley,
                seed: Some(seed),
                amplitude: 128.0,
                base: 0.0,
                scale: 1024.0,
                octaves: 5,
                cells_x: None,
                cells_z: None,
                cell_size: None,
            }],
        );
    }
    let (lo, hi) = h.raw_range().expect("terrain");
    let (min_y, max_y) = (terrain.raw_to_y(lo), terrain.raw_to_y(hi));

    // walls, sky lid, sky model, kill slab
    let half = SIZE / 2.0;
    let bottom = terrain.origin[1];
    let wall_top = (max_y + WALL_CLEARANCE + 256.0).max(640.0);
    let wall_h = wall_top - bottom;
    let wall_y = bottom + wall_h / 2.0;
    let thick = 64.0;
    let span = SIZE + 2.0 * thick;
    let mut shell = vec![
        solid(
            SolidSpec::Box,
            [0.0, wall_y, half + thick / 2.0],
            [span, wall_h, thick],
            "wall",
            "wall",
            None,
            false,
        ),
        solid(
            SolidSpec::Box,
            [0.0, wall_y, -(half + thick / 2.0)],
            [span, wall_h, thick],
            "wall",
            "wall",
            None,
            false,
        ),
        solid(
            SolidSpec::Box,
            [half + thick / 2.0, wall_y, 0.0],
            [thick, wall_h, span],
            "wall",
            "wall",
            None,
            false,
        ),
        solid(
            SolidSpec::Box,
            [-(half + thick / 2.0), wall_y, 0.0],
            [thick, wall_h, span],
            "wall",
            "wall",
            None,
            false,
        ),
        solid(
            SolidSpec::Box,
            [0.0, wall_top + thick / 2.0, 0.0],
            [span, thick, span],
            "sky",
            "sky_lid",
            None,
            false,
        ),
        solid(
            SolidSpec::Box,
            [0.0, 0.0, 0.0],
            p.settings.sky_size,
            "sky",
            "sky",
            Some(BrushRole::Sky),
            false,
        ),
        solid(
            SolidSpec::Box,
            [0.0, min_y - 96.0, 0.0],
            [SIZE, 64.0, SIZE],
            "wall",
            "kill",
            Some(BrushRole::KillVolume),
            false,
        ),
    ];
    if t != Template::Flat {
        // raised bases and mirrored cover (on the floor)
        for sx in [-1.0f32, 1.0] {
            shell.push(solid(
                SolidSpec::Box,
                [sx * 1536.0, 0.0, 0.0],
                [640.0, 64.0, 1664.0],
                "block",
                "base",
                None,
                true,
            ));
            for sz in [-1.0f32, 1.0] {
                shell.push(solid(
                    SolidSpec::Box,
                    [sx * 512.0, 0.0, sz * 320.0],
                    [128.0, 96.0, 320.0],
                    "block",
                    "cover",
                    None,
                    true,
                ));
                shell.push(solid(
                    SolidSpec::Wedge,
                    [sx * 896.0, 0.0, sz * 640.0],
                    [192.0, 96.0, 192.0],
                    "block",
                    "cover",
                    None,
                    true,
                ));
            }
            shell.push(solid(
                SolidSpec::Box,
                [0.0, 0.0, sx * 768.0],
                [448.0, 80.0, 128.0],
                "block",
                "cover",
                None,
                true,
            ));
            shell.push(solid(
                SolidSpec::Cylinder { sides: 12 },
                [sx * 320.0, 0.0, 0.0],
                [160.0, 128.0, 160.0],
                "block",
                "cover",
                None,
                true,
            ));
        }
    }
    run(&mut p, &mut h, &shell);

    // objects
    let mut objs = Vec::new();
    for (team, sx, yaw) in [("1", -1.0f32, 90.0f32), ("2", 1.0, -90.0)] {
        for i in 0..8 {
            let z = -700.0 + i as f32 * 200.0;
            objs.push(add(
                v::CLASS_PLAYER_SPAWN,
                [sx * 1536.0, 0.0, z],
                yaw,
                &[(v::PROP_TEAM, text(team)), (v::PROP_MODES, text(""))],
            ));
        }
    }
    for i in 0..16 {
        let a = i as f32 * 22.5;
        let (s, c) = sincos_deg(a);
        let r = 1100.0;
        objs.push(add(
            v::CLASS_PLAYER_SPAWN,
            [r * s, 0.0, r * c],
            a + 180.0,
            &[(v::PROP_TEAM, text("")), (v::PROP_MODES, text(""))],
        ));
    }
    for (n, z) in [(0.0, -1024.0), (1.0, 1024.0)] {
        objs.push(add(
            v::CLASS_BOMB_SITE,
            [0.0, 0.0, z],
            0.0,
            &[
                (v::PROP_EXTENT, PropValue::Vec3(ZONE_EXTENT)),
                (v::PROP_NUMBER, PropValue::Num(n)),
                (v::PROP_MODES, text("")),
            ],
        ));
    }
    for (team, sx) in [("1", -1.0f32), ("2", 1.0)] {
        objs.push(add(
            v::CLASS_FLAG_ZONE,
            [sx * 1536.0, 0.0, 0.0],
            0.0,
            &[
                (v::PROP_EXTENT, PropValue::Vec3(ZONE_EXTENT)),
                (v::PROP_TEAM, text(team)),
                (v::PROP_MODES, text("")),
            ],
        ));
        objs.push(add(
            v::CLASS_CAPTURE_FLAG,
            [sx * 1536.0, 0.0, 0.0],
            0.0,
            &[(v::PROP_TEAM, text(team)), (v::PROP_MODEL, text(""))],
        ));
    }
    for sx in [-1.0f32, 1.0] {
        for sz in [-1.0f32, 1.0] {
            objs.push(add(
                v::CLASS_ITEM_SPAWN,
                [sx * 768.0, 0.0, sz * 768.0],
                0.0,
                &[(v::PROP_MODES, text(""))],
            ));
        }
    }
    run(&mut p, &mut h, &objs);

    let files = TEXTURES
        .iter()
        .map(|(id, file, _, _)| OwnFile {
            path: file.to_string(),
            bytes: textures::png(id, seed),
        })
        .collect();
    Made {
        project: p,
        heights: h,
        files,
    }
}

/// The contract checks; an empty list means the project satisfies it.
pub fn check_contract(p: &Project, h: &Heights) -> Vec<String> {
    let mut bad = Vec::new();
    let Some(t) = &p.terrain else {
        return vec!["no terrain".into()];
    };
    if h.cols != t.cols() || h.rows != t.rows() {
        return vec!["heights do not fit the terrain".into()];
    }
    let (lo, hi) = h.raw_range().expect("sized");
    let (min_y, max_y) = (t.raw_to_y(lo), t.raw_to_y(hi));
    let walls: Vec<_> = p
        .solids
        .iter()
        .filter(|s| s.group.as_deref() == Some("wall") && s.brush.is_none())
        .collect();
    if walls.len() != 4 {
        bad.push(format!("{} boundary walls, want 4", walls.len()));
    }
    for w in &walls {
        if w.pos[1] + w.size[1] / 2.0 < max_y + WALL_CLEARANCE {
            bad.push(format!(
                "wall {} is not {WALL_CLEARANCE} above the highest terrain point",
                w.id
            ));
        }
    }
    let sky_faces = p
        .solids
        .iter()
        .filter(|s| s.brush.is_none() && p.material(&s.material).is_some_and(|m| m.flags.sky))
        .count();
    if sky_faces == 0 {
        bad.push("no sky faces (a level solid with a sky material)".into());
    }
    let sky_models = p
        .solids
        .iter()
        .filter(|s| s.brush == Some(BrushRole::Sky))
        .count();
    if sky_models != 1 {
        bad.push(format!("{sky_models} sky models, want 1"));
    }
    if p.material(&p.settings.sky_material).is_none() {
        bad.push("settings.sky_material is not a material".into());
    }
    let diag = (SIZE * SIZE * 2.0 + (max_y - min_y) * (max_y - min_y)).sqrt();
    if !p.settings.fog_enable {
        bad.push("fog is off".into());
    }
    if p.settings.far_z < diag {
        bad.push(format!(
            "far_z {} below the diagonal {diag}",
            p.settings.far_z
        ));
    }
    if p.settings.sun_bright <= 0.0 {
        bad.push("no sun (global light)".into());
    }
    let kills: Vec<_> = p
        .solids
        .iter()
        .filter(|s| s.brush == Some(BrushRole::KillVolume))
        .collect();
    if kills.len() != 1 {
        bad.push(format!("{} kill volumes, want 1", kills.len()));
    }
    for k in &kills {
        if k.pos[1] + k.size[1] / 2.0 >= min_y {
            bad.push("the kill volume's top is not below the lowest terrain point".into());
        }
    }
    let spawns: Vec<_> = p.of_class(v::CLASS_PLAYER_SPAWN).collect();
    let count = |team: &str| {
        spawns
            .iter()
            .filter(|o| o.text(v::PROP_TEAM).unwrap_or("") == team)
            .count()
    };
    for (team, want) in [("1", 8), ("2", 8), ("", 16)] {
        let n = count(team);
        if n != want {
            bad.push(format!("{n} spawns of team \"{team}\", want {want}"));
        }
    }
    for o in &spawns {
        if !o.text(v::PROP_MODES).unwrap_or("").is_empty() {
            bad.push(format!("{} has a modes filter", o.id));
        }
        match ops::ground_height(p, h, o.pos[0], o.pos[2]) {
            Some(f) => {
                if (o.pos[1] - (f + STAND_OFFSET)).abs() > 1.0 {
                    bad.push(format!(
                        "{} is not standing on the floor ({} vs floor {f})",
                        o.id, o.pos[1]
                    ));
                }
            }
            None => bad.push(format!("{} has no floor", o.id)),
        }
    }
    let sites: Vec<_> = p.of_class(v::CLASS_BOMB_SITE).collect();
    let mut numbers: Vec<i64> = sites
        .iter()
        .filter_map(|o| o.num(v::PROP_NUMBER))
        .map(|n| n as i64)
        .collect();
    numbers.sort_unstable();
    if sites.len() != 2 || numbers != [0, 1] {
        bad.push(format!("bomb sites {:?}, want numbers 0 and 1", numbers));
    }
    for o in &sites {
        if o.props.get(v::PROP_EXTENT).and_then(PropValue::as_vec3) != Some(ZONE_EXTENT) {
            bad.push(format!("{} has no {:?} extent", o.id, ZONE_EXTENT));
        }
    }
    let zones: Vec<_> = p.of_class(v::CLASS_FLAG_ZONE).collect();
    let flags: Vec<_> = p.of_class(v::CLASS_CAPTURE_FLAG).collect();
    for team in ["1", "2"] {
        let z: Vec<_> = zones
            .iter()
            .filter(|o| o.text(v::PROP_TEAM) == Some(team))
            .collect();
        let f: Vec<_> = flags
            .iter()
            .filter(|o| o.text(v::PROP_TEAM) == Some(team))
            .collect();
        if z.len() != 1 || f.len() != 1 {
            bad.push(format!(
                "team {team}: {} flag zones and {} flags, want 1 each",
                z.len(),
                f.len()
            ));
            continue;
        }
        let (z, f) = (z[0], f[0]);
        let e = z
            .props
            .get(v::PROP_EXTENT)
            .and_then(PropValue::as_vec3)
            .unwrap_or([0.0; 3]);
        if (f.pos[0] - z.pos[0]).abs() > e[0] || (f.pos[2] - z.pos[2]).abs() > e[2] {
            bad.push(format!("flag {} is outside zone {}", f.id, z.id));
        }
    }
    let items = p.of_class(v::CLASS_ITEM_SPAWN).count();
    if items != 4 {
        bad.push(format!("{items} item spawns, want 4"));
    }
    for m in ALL_MODES {
        if !p.mod_meta.modes.iter().any(|x| x == m) {
            bad.push(format!("mod_meta.modes lacks {m}"));
        }
    }
    let dl = p.mod_meta.description.chars().count();
    if !(20..=500).contains(&dl) {
        bad.push(format!("description has {dl} characters"));
    }
    if p.mod_meta.author.is_empty() {
        bad.push("no author".into());
    }
    for m in &p.materials {
        match &m.texture {
            TextureRef::Rom(path) => {
                bad.push(format!("material {} uses a ROM texture ({path})", m.id))
            }
            TextureRef::Own(path) => {
                if !path.starts_with("assets/") {
                    bad.push(format!("material {} texture is not under assets/", m.id));
                }
            }
        }
    }
    if !crate::project::check_key(&p.key) {
        bad.push(format!("bad key `{}`", p.key));
    }
    bad
}

/// Scaffolded `AGENTS.md` text for a project.
pub fn agents_md(p: &Project, template: Template) -> String {
    let t = p.terrain.as_ref();
    format!(
        "# {name} (dc-mapkit project)\n\n\
This folder is a DeCombat community map: `map.json` (the project, format 1), `terrain.r16` (heights), `assets/` \
(own textures), `build/` (outputs, ignored by git). Made from the `{template}` template; key `{key}`; \
{cx} x {cz} cells of {cs} units.\n\n\
## The loop\n\n\
1. `dc-mapkit info . --json` to see what is here (counts, spawns per team, modes, size).\n\
2. Change it: `dc-mapkit apply . ops.json` (or `-` for stdin) with a JSON list of ops, or `dc-mapkit terrain . <op>`; \
`dc-mapkit schema --ops` lists every op and field. `dc-mapkit get . --class dc.player_spawn --json` queries objects. \
Or generate a whole layout with a Lua script against the `map` API: `dc-mapkit run . gen.lua` (playbook section 6; \
`examples/mapkit/` in the repository; a script that may run twice starts with `map.clear()`). An MCP client gets the same verbs from `dc-mapkit mcp`.\n\
3. `dc-mapkit validate . --json` (project checks; exit 1 on errors, each with a code and a fix hint).\n\
4. `dc-mapkit build .` then `dc-mapkit render . --view top -o build/top.png` and look.\n\
5. Undo: `dc-mapkit history .` / `dc-mapkit revert . --to <n>` (apply snapshots before every change).\n\n\
Facts: units are world units (a player stands 100 tall); solids are centre + full size; `ground: true` snaps to \
the floor; ids are `<prefix>_<n>` and only start over after a `clear` empties the project; paths are forward-slash relative to this folder; only own \
textures under `assets/` may be committed.\n\n\
The playbook is `docs/MAPKIT_AGENTS.md` in the DeCombat repository.\n",
        name = p.name,
        template = template.name(),
        key = p.key,
        cx = t.map_or(0, |t| t.cells_x),
        cz = t.map_or(0, |t| t.cells_z),
        cs = t.map_or(0.0, |t| t.cell_size),
    )
}

/// Scaffolded `.gitignore`.
pub const GITIGNORE: &str = "build/\n.mapkit/\n";

/// Procedural 256 x 256 PNG textures (checker, noise, stripes, sky), seeded, no ROM content.
pub mod textures {
    use crate::ops::noise::Noise;
    use std::io::Cursor;

    pub const SIZE: u32 = 256;

    fn encode(img: image::RgbaImage) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .expect("png encodes");
        buf.into_inner()
    }

    pub fn checker(a: [u8; 3], b: [u8; 3], cell: u32) -> Vec<u8> {
        encode(image::RgbaImage::from_fn(SIZE, SIZE, |x, y| {
            let c = if ((x / cell) + (y / cell)).is_multiple_of(2) {
                a
            } else {
                b
            };
            image::Rgba([c[0], c[1], c[2], 255])
        }))
    }

    pub fn noise(seed: u64, dark: [u8; 3], light: [u8; 3]) -> Vec<u8> {
        let n = Noise::new(seed);
        encode(image::RgbaImage::from_fn(SIZE, SIZE, |x, y| {
            let t = (n.fbm(x as f32 / 32.0, y as f32 / 32.0, 4) * 0.5 + 0.5).clamp(0.0, 1.0);
            let mix = |i: usize| (dark[i] as f32 + (light[i] as f32 - dark[i] as f32) * t) as u8;
            image::Rgba([mix(0), mix(1), mix(2), 255])
        }))
    }

    pub fn stripes(a: [u8; 3], b: [u8; 3], band: u32) -> Vec<u8> {
        encode(image::RgbaImage::from_fn(SIZE, SIZE, |x, y| {
            let edge = x % band < 2 || y % band < 2;
            let c = if edge { b } else { a };
            image::Rgba([c[0], c[1], c[2], 255])
        }))
    }

    pub fn sky(top: [u8; 3], bottom: [u8; 3]) -> Vec<u8> {
        encode(image::RgbaImage::from_fn(SIZE, SIZE, |_, y| {
            let t = y as f32 / (SIZE - 1) as f32;
            let mix = |i: usize| (top[i] as f32 + (bottom[i] as f32 - top[i] as f32) * t) as u8;
            image::Rgba([mix(0), mix(1), mix(2), 255])
        }))
    }

    /// The texture of a template material id.
    pub fn png(id: &str, seed: u64) -> Vec<u8> {
        match id {
            "ground" => noise(seed, [70, 90, 45], [130, 150, 90]),
            "wall" => stripes([150, 145, 135], [90, 85, 80], 64),
            "block" => checker([120, 120, 125], [160, 160, 165], 32),
            _ => sky([80, 120, 200], [190, 210, 235]),
        }
    }
}

/// Validate every template through the project validator too (used by the pinning test and `new`).
pub fn validate_made(dir: &std::path::Path, made: &Made) -> Diagnostics {
    crate::validate::validate(dir, &made.project, &made.heights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{MAP_FILE, TERRAIN_FILE};
    use std::fs;

    #[test]
    fn every_template_meets_the_contract_and_validates_without_errors() {
        for t in Template::ALL {
            let made = make(t, "Test Map", "tester", 7);
            let bad = check_contract(&made.project, &made.heights);
            assert!(bad.is_empty(), "{}: {bad:?}", t.name());
            assert_eq!(made.files.len(), TEXTURES.len());
            // write it out and validate against the files
            let dir = std::env::temp_dir().join(format!(
                "dc-mapkit-template-{}-{}",
                t.name(),
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            for f in &made.files {
                let path = Project::resolve(&dir, &f.path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, &f.bytes).unwrap();
                assert_eq!(image::image_dimensions(&path).unwrap(), (256, 256));
            }
            made.project.save_all(&dir, &made.heights).unwrap();
            let d = validate_made(&dir, &made);
            assert!(!d.has_errors(), "{}: {:?}", t.name(), d.items);
            assert!(dir.join(MAP_FILE).exists() && dir.join(TERRAIN_FILE).exists());
            // deterministic
            let again = make(t, "Test Map", "tester", 7);
            assert_eq!(again, made);
            let _ = fs::remove_dir_all(&dir);
        }
        // the templates differ as described
        let flat = make(Template::Flat, "a", "b", 1);
        let arena = make(Template::Arena, "a", "b", 1);
        let starter = make(Template::Starter, "a", "b", 1);
        assert!(arena.project.solids.len() > flat.project.solids.len());
        assert_eq!(arena.project.solids.len(), starter.project.solids.len());
        assert_eq!(flat.heights.raw_range().map(|(a, b)| a == b), Some(true));
        assert_ne!(starter.heights.raw_range().map(|(a, b)| a == b), Some(true));
        assert_eq!(flat.project.mod_meta.id, "a");
        assert!(agents_md(&flat.project, Template::Flat).contains("dc-mapkit apply"));
    }

    #[test]
    fn the_contract_check_catches_a_broken_project() {
        let mut made = make(Template::Flat, "x", "y", 1);
        made.project
            .objects
            .retain(|o| o.class != v::CLASS_BOMB_SITE);
        made.project.settings.fog_enable = false;
        let bad = check_contract(&made.project, &made.heights);
        assert!(bad.iter().any(|b| b.contains("bomb sites")));
        assert!(bad.iter().any(|b| b.contains("fog")));
    }
}
