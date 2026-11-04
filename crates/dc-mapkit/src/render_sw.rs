//! `dc-mapkit render`: the software rasteriser (MAPKIT_PLAN.md 5.2, D8, section 7: camera-generic, z-buffered,
//! flat per-vertex Gouraud colour, no GPU or ROM). DC-DEFAULT: MAP-53 (views, markers, legend), MAP-52 (the
//! export screenshot and radar come from this path, so `export` never needs a GPU).
//!
//! A [`Scene`] is a flat list of coloured triangles plus object markers and spawn cameras. It is built from the
//! project geometry ([`Scene::from_level`] + [`Scene::add_project_objects`], the `render` verb) or from a built
//! world file ([`Scene::from_world`]: the baked ARGB of the render vertices when the file carries render data,
//! else its BSP polygons; `export` renders the screenshot and the radar from the bytes it ships).
//!
//! Views ([`View`]): `top` (orthographic straight down, +x right, +z up: the radar orientation), `iso`
//! (orthographic from the +x +z corner, 45 degrees of yaw, 35 degrees down), `spawn:<n>` (perspective from the
//! n-th `dc.player_spawn` in file order, 0-based, eye 24 units above the point, looking along its yaw) and
//! `free:<x,y,z,yaw,pitch>` (perspective, degrees, yaw 0 = +z about y, pitch up positive; 90 degree horizontal
//! field of view). Orthographic views fit the geometry extent with a 4 % margin and skip sky faces (the sky lid
//! would hide the map); perspective views draw them. Markers (`labels`) are discs coloured by class (team
//! variants for spawns), drawn over the geometry without a depth test, so an agent sees every object; their
//! screen positions and ids come back in [`Rendered::markers`], and [`Rendered::legend`] names the colours, so no
//! font is needed. The grid (`grid`) draws world lines every `grid_step` units on the floor plane of the extent
//! in the orthographic views.
//!
//! Determinism: every projection uses [`sincos_deg`] (no libm), so the same scene gives the same PNG bytes on
//! every platform (the export's twice-built comparison includes the pictures).

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::geom::{sincos_deg, Level, SURF_SKY, V3};
use crate::project::{BrushRole, Project};
use crate::vocab_names as v;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One triangle with a colour per corner (Gouraud); `sky` marks a 0x10 face.
#[derive(Debug, Clone, PartialEq)]
pub struct Tri {
    pub v: [V3; 3],
    pub c: [[u8; 3]; 3],
    pub sky: bool,
}

impl Tri {
    /// A flat-coloured triangle.
    pub fn flat(v: [V3; 3], color: [u8; 3]) -> Tri {
        Tri {
            v,
            c: [color; 3],
            sky: false,
        }
    }
}

/// A label at a point (object ids).
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub at: V3,
    pub text: String,
}

/// An object marker: a disc at `at`, coloured by its legend `key`.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub at: V3,
    pub id: String,
    pub class: String,
    pub key: String,
}

/// A spawn camera (`spawn:<n>`): the point and its yaw in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnCam {
    pub pos: V3,
    pub yaw: f32,
}

/// A flat triangle list with colours, markers and spawn cameras.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scene {
    pub tris: Vec<Tri>,
    pub labels: Vec<Label>,
    pub markers: Vec<Marker>,
    pub spawns: Vec<SpawnCam>,
    pub extent_min: V3,
    pub extent_max: V3,
}

/// Camera choice (`--view`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    Top,
    Iso,
    Spawn(u32),
    Free { pos: [f32; 3], yaw: f32, pitch: f32 },
}

impl View {
    /// Parse `top | iso | spawn:<n> | free:<x,y,z,yaw,pitch>`.
    pub fn parse(s: &str) -> Result<View, String> {
        match s {
            "top" => Ok(View::Top),
            "iso" => Ok(View::Iso),
            _ => {
                if let Some(n) = s.strip_prefix("spawn:") {
                    return n
                        .parse()
                        .map(View::Spawn)
                        .map_err(|_| format!("bad spawn index `{n}`"));
                }
                if let Some(rest) = s.strip_prefix("free:") {
                    let v: Result<Vec<f32>, _> =
                        rest.split(',').map(|t| t.trim().parse::<f32>()).collect();
                    let v = v.map_err(|_| format!("bad free view `{rest}` (x,y,z,yaw,pitch)"))?;
                    if v.len() != 5 {
                        return Err("free view needs x,y,z,yaw,pitch".into());
                    }
                    return Ok(View::Free {
                        pos: [v[0], v[1], v[2]],
                        yaw: v[3],
                        pitch: v[4],
                    });
                }
                Err(format!(
                    "unknown view `{s}` (top | iso | spawn:<n> | free:<x,y,z,yaw,pitch>)"
                ))
            }
        }
    }

    /// The text form (`top`, `iso`, `spawn:3`, `free:x,y,z,yaw,pitch`).
    pub fn text(&self) -> String {
        match self {
            View::Top => "top".into(),
            View::Iso => "iso".into(),
            View::Spawn(n) => format!("spawn:{n}"),
            View::Free { pos, yaw, pitch } => {
                format!("free:{},{},{},{},{}", pos[0], pos[1], pos[2], yaw, pitch)
            }
        }
    }

    /// A file-name-safe form (`top`, `iso`, `spawn_3`, `free`).
    pub fn slug(&self) -> String {
        match self {
            View::Top => "top".into(),
            View::Iso => "iso".into(),
            View::Spawn(n) => format!("spawn_{n}"),
            View::Free { .. } => "free".into(),
        }
    }

    /// True for the two orthographic views.
    pub fn is_ortho(&self) -> bool {
        matches!(self, View::Top | View::Iso)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    /// Draw the object markers (the discs; ids and positions come back in the report).
    pub labels: bool,
    /// Draw the world grid (orthographic views).
    pub grid: bool,
    /// Grid spacing in world units.
    pub grid_step: f32,
    pub background: [u8; 3],
    pub sun_dir: V3,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            labels: false,
            grid: false,
            grid_step: GRID_STEP,
            background: [24, 26, 32],
            sun_dir: V3::new(-0.4, -0.8, 0.3),
        }
    }
}

/// Default grid spacing (DC-DEFAULT: MAP-53): 512 units, eight cells of the templates' 64-unit terrain.
pub const GRID_STEP: f32 = 512.0;
/// Eye height above a spawn point (the point stands `STAND_OFFSET` = 53 above the floor, MAP-11; the eye of the
/// server's 106-unit standing body is near 77).
pub const SPAWN_EYE: f32 = 24.0;
/// Near plane of the perspective views (world units).
pub const NEAR: f32 = 4.0;
/// Margin around the geometry in the orthographic views.
const ORTHO_MARGIN: f32 = 0.04;
/// Iso view angles (DC-DEFAULT: MAP-53): yaw from the +x +z corner, 35 degrees down.
const ISO_YAW: f32 = 225.0;
const ISO_PITCH: f32 = -35.0;
/// Colour of a sky face.
const SKY_COLOR: [u8; 3] = [140, 170, 220];

/// An RGBA8 image (row-major, top-left first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbaImage {
    pub fn new(width: u32, height: u32, bg: [u8; 3]) -> RgbaImage {
        let mut data = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..width as usize * height as usize {
            data.extend_from_slice(&[bg[0], bg[1], bg[2], 255]);
        }
        RgbaImage {
            width,
            height,
            data,
        }
    }

    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    pub fn put(&mut self, x: u32, y: u32, c: [u8; 3]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.data[i] = c[0];
        self.data[i + 1] = c[1];
        self.data[i + 2] = c[2];
        self.data[i + 3] = 255;
    }

    /// Blend `c` over the pixel with weight `a` (0..1).
    pub fn blend(&mut self, x: u32, y: u32, c: [u8; 3], a: f32) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        for (k, ck) in c.iter().enumerate() {
            let old = self.data[i + k] as f32;
            self.data[i + k] = (old + (*ck as f32 - old) * a).round().clamp(0.0, 255.0) as u8;
        }
        self.data[i + 3] = 255;
    }

    /// Encode as PNG.
    pub fn png(&self) -> Vec<u8> {
        let img = image::RgbaImage::from_raw(self.width, self.height, self.data.clone())
            .expect("sizes match");
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .expect("png encodes");
        buf.into_inner()
    }

    /// Nearest-neighbour resample to `w` x `h` (box average when shrinking by a whole factor).
    pub fn resized(&self, w: u32, h: u32) -> RgbaImage {
        let mut out = RgbaImage::new(w.max(1), h.max(1), [0, 0, 0]);
        let fx = self.width as f32 / out.width as f32;
        let fy = self.height as f32 / out.height as f32;
        let bx = (fx.floor() as u32).max(1);
        let by = (fy.floor() as u32).max(1);
        for y in 0..out.height {
            for x in 0..out.width {
                let sx = ((x as f32 * fx) as u32).min(self.width - 1);
                let sy = ((y as f32 * fy) as u32).min(self.height - 1);
                let mut acc = [0u32; 4];
                let mut n = 0;
                for dy in 0..by {
                    for dx in 0..bx {
                        let px = self.get(
                            (sx + dx).min(self.width - 1),
                            (sy + dy).min(self.height - 1),
                        );
                        for k in 0..4 {
                            acc[k] += px[k] as u32;
                        }
                        n += 1;
                    }
                }
                let i = (y as usize * out.width as usize + x as usize) * 4;
                for (k, sum) in acc.iter().enumerate() {
                    out.data[i + k] = (sum / n) as u8;
                }
            }
        }
        out
    }
}

/// A fixed, readable palette by material index, lit by the normal. Past the palette each index gets its own hue
/// (golden-angle steps): wrapping gave a ninth material the first one's colour, which hid it against the floor.
fn material_color(i: usize) -> [u8; 3] {
    const PALETTE: [[u8; 3]; 8] = [
        [120, 150, 90],
        [160, 140, 110],
        [130, 130, 140],
        [170, 120, 90],
        [100, 140, 160],
        [150, 110, 150],
        [180, 170, 120],
        [110, 160, 130],
    ];
    if let Some(c) = PALETTE.get(i) {
        return *c;
    }
    const GOLDEN_DEG: f32 = 137.508;
    let hue = ((i - PALETTE.len() + 1) as f32 * GOLDEN_DEG + 20.0) % 360.0;
    // a muted colour like the palette's: the channel follows the distance from its hue centre
    let channel = |centre: f32| {
        let d = ((hue - centre + 540.0) % 360.0 - 180.0).abs();
        (90.0 + 110.0 * (1.0 - (d / 120.0).min(1.0))) as u8
    };
    [channel(0.0), channel(120.0), channel(240.0)]
}

fn shade(base: [u8; 3], n: V3, sun: V3) -> [u8; 3] {
    let l = sun.normalized().unwrap_or(V3::new(0.0, -1.0, 0.0));
    let k = 0.55 + 0.45 * (-n.dot(l)).max(0.0);
    [
        (base[0] as f32 * k) as u8,
        (base[1] as f32 * k) as u8,
        (base[2] as f32 * k) as u8,
    ]
}

fn brush_tint(role: BrushRole) -> Option<[u8; 3]> {
    Some(match role {
        BrushRole::KillVolume => [200, 60, 60],
        BrushRole::Ladder => [220, 200, 80],
        BrushRole::Breakable => [220, 140, 60],
        BrushRole::WorldModel => [150, 150, 170],
        BrushRole::Sky => return None,
    })
}

/// The tint of a brush model named by an object of `class` (a `dc.*` class), `None` for the sky model.
fn class_tint(class: &str) -> Option<[u8; 3]> {
    let role = match class {
        v::CLASS_KILL_VOLUME | v::CLASS_HAZARD => BrushRole::KillVolume,
        v::CLASS_LADDER => BrushRole::Ladder,
        v::CLASS_BREAKABLE => BrushRole::Breakable,
        v::CLASS_SKY_MODEL => BrushRole::Sky,
        _ => BrushRole::WorldModel,
    };
    brush_tint(role)
}

/// The legend key and colour of an object marker (DC-DEFAULT: MAP-53): one colour per class, team variants
/// for spawns, zones and flags.
pub fn marker_style(class: &str, team: &str) -> (String, [u8; 3]) {
    let team_key = |base: &str| match team {
        "1" => format!("{base} team 1"),
        "2" => format!("{base} team 2"),
        _ => base.to_string(),
    };
    match class {
        v::CLASS_PLAYER_SPAWN => (
            team_key(class),
            match team {
                "1" => [70, 130, 255],
                "2" => [255, 90, 70],
                _ => [240, 240, 240],
            },
        ),
        v::CLASS_ITEM_SPAWN => (class.into(), [120, 230, 120]),
        v::CLASS_BOMB_SITE => (class.into(), [255, 200, 40]),
        v::CLASS_FLAG_ZONE => (
            team_key(class),
            match team {
                "1" => [120, 80, 255],
                "2" => [255, 80, 160],
                _ => [170, 100, 220],
            },
        ),
        v::CLASS_CAPTURE_FLAG => (
            team_key(class),
            match team {
                "1" => [40, 200, 255],
                "2" => [255, 160, 40],
                _ => [255, 220, 180],
            },
        ),
        v::CLASS_POINT_LIGHT => (class.into(), [255, 255, 150]),
        v::CLASS_SOUND => (class.into(), [80, 220, 220]),
        v::CLASS_SPRITE => (class.into(), [220, 160, 90]),
        _ => (class.into(), [180, 180, 180]),
    }
}

fn push_fan(tris: &mut Vec<Tri>, corners: &[V3], color: [u8; 3], sky: bool) {
    for i in 1..corners.len().saturating_sub(1) {
        tris.push(Tri {
            v: [corners[0], corners[i], corners[i + 1]],
            c: [color; 3],
            sky,
        });
    }
}

impl Scene {
    /// Flatten a level: every polygon fan-triangulated with its shaded material colour.
    pub fn from_level(level: &Level, opts: &RenderOptions) -> Scene {
        let mut s = Scene {
            extent_min: level.extent_min,
            extent_max: level.extent_max,
            ..Default::default()
        };
        for p in &level.polys {
            let sky = p.flags & SURF_SKY != 0;
            let c = if sky {
                SKY_COLOR
            } else {
                shade(material_color(p.material), p.plane.n, opts.sun_dir)
            };
            push_fan(&mut s.tris, &p.corners, c, sky);
        }
        for p in &level.sky_faces {
            push_fan(&mut s.tris, &p.corners, SKY_COLOR, true);
        }
        for b in &level.brushes {
            let Some(tint) = brush_tint(b.role) else {
                continue;
            };
            for p in &b.polys {
                push_fan(
                    &mut s.tris,
                    &p.corners,
                    shade(tint, p.plane.n, opts.sun_dir),
                    false,
                );
            }
        }
        s
    }

    /// Add the project's point objects as markers (and labels) and its spawns as cameras.
    pub fn add_project_objects(&mut self, p: &Project) {
        for o in &p.objects {
            let at = V3::from_array(o.pos);
            let team = o.text(v::PROP_TEAM).unwrap_or("");
            let (key, _) = marker_style(&o.class, team);
            self.markers.push(Marker {
                at,
                id: o.id.clone(),
                class: o.class.clone(),
                key,
            });
            self.labels.push(Label {
                at,
                text: o.id.clone(),
            });
            if o.class == v::CLASS_PLAYER_SPAWN {
                self.spawns.push(SpawnCam {
                    pos: at,
                    yaw: o.rot[1],
                });
            }
        }
    }

    /// Build the scene of a world file: triangles from the render data (baked ARGB, `texture * colour * 2`, so
    /// 0x80 = unit brightness; the sky model's nested world is skipped) when the file carries any, else the BSP
    /// polygons shaded by their plane (brush models tinted by the class of the object naming them); the level
    /// model's 0x10 polygons are the sky faces; objects in the `dc.*` vocabulary become markers and spawn
    /// cameras.
    // SPEC: FMT_DAT85.md §10.2 (vertex: position at 0, colour u32 ARGB at 28; triangles 3 indices + source)
    pub fn from_world(w: &dc_world::World<'_>, opts: &RenderOptions) -> Scene {
        let mut s = Scene {
            extent_min: V3::from_array(w.extent_min),
            extent_max: V3::from_array(w.extent_max),
            ..Default::default()
        };
        // object names -> class (brush tints, the sky model to skip)
        let mut class_of_name: BTreeMap<Vec<u8>, String> = BTreeMap::new();
        for o in &w.objects {
            let class = String::from_utf8_lossy(o.class_name).into_owned();
            if let Some(n) = o.name() {
                class_of_name.insert(n.to_vec(), class.clone());
            }
            let Some(pos) = o.position() else {
                continue;
            };
            if !class.starts_with("dc.") {
                continue;
            }
            let at = V3::from_array(pos);
            let team = o
                .props
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(v::PROP_TEAM.as_bytes()))
                .and_then(|p| match p.value {
                    dc_world::FieldValue::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
                    _ => None,
                })
                .unwrap_or_default();
            let id = o
                .name()
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default();
            let (key, _) = marker_style(&class, &team);
            if class == v::CLASS_PLAYER_SPAWN {
                let yaw = o.rotation().map(|r| r[1].to_degrees()).unwrap_or(0.0);
                s.spawns.push(SpawnCam { pos: at, yaw });
            }
            if v::is_point_class(&class) {
                s.labels.push(Label {
                    at,
                    text: id.clone(),
                });
                s.markers.push(Marker { at, id, class, key });
            }
        }
        let has_render = w
            .render
            .as_ref()
            .is_some_and(|r| r.world.blocks.iter().any(|b| b.triangle_count > 0));
        for m in &w.models {
            let brush = m.is_dual();
            let tint = if brush {
                match class_of_name.get(m.name) {
                    Some(c) => match class_tint(c) {
                        Some(t) => Some(t),
                        None => continue, // the sky model
                    },
                    None => Some([150, 150, 170]),
                }
            } else {
                None
            };
            for p in &m.polygons {
                let Some(surf) = m.surfaces.get(p.surface as usize) else {
                    continue;
                };
                let sky = surf.flags & SURF_SKY != 0;
                if has_render && !sky {
                    continue; // drawn from the render data
                }
                let Some(plane) = m.planes.get(p.plane as usize) else {
                    continue;
                };
                let corners: Vec<V3> = m
                    .polygon_points(p)
                    .iter()
                    .filter_map(|&i| m.points.get(i as usize).map(|q| V3::from_array(*q)))
                    .collect();
                if corners.len() < 3 {
                    continue;
                }
                let n = V3::from_array(plane.normal);
                let c = if sky {
                    SKY_COLOR
                } else {
                    shade(
                        tint.unwrap_or_else(|| material_color(surf.texture as usize)),
                        n,
                        opts.sun_dir,
                    )
                };
                push_fan(&mut s.tris, &corners, c, sky);
            }
        }
        if let Some(r) = &w.render {
            if has_render {
                for b in &r.world.blocks {
                    push_block(&mut s.tris, b);
                }
                for sub in &r.world.models {
                    if class_of_name
                        .get(sub.name)
                        .is_some_and(|c| c == v::CLASS_SKY_MODEL)
                    {
                        continue;
                    }
                    for b in &sub.world.blocks {
                        push_block(&mut s.tris, b);
                    }
                }
            }
        }
        s
    }

    /// The legend of the markers present: key -> colour, sorted by key.
    pub fn legend(&self) -> Vec<LegendRow> {
        let mut rows: BTreeMap<String, [u8; 3]> = BTreeMap::new();
        for m in &self.markers {
            let team = m
                .key
                .rsplit("team ")
                .next()
                .filter(|_| m.key.contains(" team "));
            let (_, color) = marker_style(&m.class, team.unwrap_or(""));
            rows.entry(m.key.clone()).or_insert(color);
        }
        rows.into_iter()
            .map(|(key, color)| LegendRow { key, color })
            .collect()
    }
}

/// Triangles of a render block from its 68-byte vertices (position at 0, ARGB at 28).
// SPEC: FMT_DAT85.md §10.2 item 6 (OBSERVED field split: position at 0, colour u32 ARGB at 28)
fn push_block(tris: &mut Vec<Tri>, b: &dc_world::RenderBlock<'_>) {
    let vert = |i: u32| -> Option<(V3, [u8; 3])> {
        let off = (i as usize).checked_mul(dc_world::RENDER_VERTEX_BYTES)?;
        let r = b.vertices.get(off..off + 32)?;
        let f = |o: usize| f32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
        // u32 ARGB little-endian: bytes B, G, R, A; 0x80 = unit brightness (colour * 2)
        let rgb = [r[30], r[29], r[28]];
        let bright = |c: u8| (c as u32 * 2).min(255) as u8;
        let c = if rgb == [0, 0, 0] {
            [128, 128, 128]
        } else {
            [bright(rgb[0]), bright(rgb[1]), bright(rgb[2])]
        };
        Some((V3::new(f(0), f(4), f(8)), c))
    };
    for i in 0..b.triangle_count {
        let Some(t) = b.triangle(i) else {
            continue;
        };
        let (Some(a), Some(bb), Some(c)) = (vert(t[0]), vert(t[1]), vert(t[2])) else {
            continue;
        };
        tris.push(Tri {
            v: [a.0, bb.0, c.0],
            c: [a.1, bb.1, c.1],
            sky: false,
        });
    }
}

/// One legend row: the marker key and its colour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegendRow {
    pub key: String,
    pub color: [u8; 3],
}

/// Where a marker landed on the picture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkerHit {
    pub id: String,
    pub class: String,
    pub key: String,
    pub x: i32,
    pub y: i32,
    /// On the picture (in front of a perspective camera and inside the frame).
    pub visible: bool,
}

/// The picture with what was drawn on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub image: RgbaImage,
    pub view: View,
    pub markers: Vec<MarkerHit>,
    pub legend: Vec<LegendRow>,
    pub grid_step: Option<f32>,
    pub triangles: usize,
    /// The world rectangle the picture covers in the `top` view: `[x_min, z_min, x_max, z_max]` (the radar
    /// mapping numbers of the map record).
    pub top_bounds: Option<[f32; 4]>,
}

// ---------------------------------------------------------------------------------------------------------------
// camera

struct Camera {
    eye: V3,
    right: V3,
    up: V3,
    forward: V3,
    ortho: bool,
    /// Pixels per world unit (orthographic) or the focal length in pixels (perspective).
    scale: f32,
    /// View-space offset so the fitted box is centred (orthographic).
    center: (f32, f32),
    w: f32,
    h: f32,
}

/// The camera basis of a yaw / pitch (degrees; yaw 0 = +z about y, pitch up positive).
fn basis(yaw: f32, pitch: f32) -> (V3, V3, V3) {
    let (sy, cy) = sincos_deg(yaw);
    let (sp, cp) = sincos_deg(pitch);
    let forward = V3::new(sy * cp, sp, cy * cp);
    let right = V3::new(cy, 0.0, -sy);
    let up = forward.cross(right);
    (right, up, forward)
}

impl Camera {
    fn new(view: &View, scene: &Scene, size: (u32, u32)) -> Result<Camera, Diagnostics> {
        let w = size.0 as f32;
        let h = size.1 as f32;
        let (eye, right, up, forward, ortho) = match view {
            View::Top => {
                let c = (scene.extent_min + scene.extent_max) * 0.5;
                let eye = V3::new(c.x, scene.extent_max.y + 1.0, c.z);
                (
                    eye,
                    V3::new(1.0, 0.0, 0.0),
                    V3::new(0.0, 0.0, 1.0),
                    V3::new(0.0, -1.0, 0.0),
                    true,
                )
            }
            View::Iso => {
                let (r, u, f) = basis(ISO_YAW, ISO_PITCH);
                let c = (scene.extent_min + scene.extent_max) * 0.5;
                let diag = (scene.extent_max - scene.extent_min).length().max(1.0);
                (c - f * (diag * 2.0), r, u, f, true)
            }
            View::Spawn(n) => {
                let Some(sp) = scene.spawns.get(*n as usize) else {
                    return Err(Diagnostics::one(
                        Diagnostic::new(
                            Code::RenderView,
                            "--view",
                            format!(
                                "spawn {n} does not exist ({} player spawns)",
                                scene.spawns.len()
                            ),
                        )
                        .with_fix("list them with `get --class dc.player_spawn`"),
                    ));
                };
                let (r, u, f) = basis(sp.yaw, 0.0);
                (sp.pos + V3::new(0.0, SPAWN_EYE, 0.0), r, u, f, false)
            }
            View::Free { pos, yaw, pitch } => {
                let (r, u, f) = basis(*yaw, *pitch);
                (V3::from_array(*pos), r, u, f, false)
            }
        };
        let mut cam = Camera {
            eye,
            right,
            up,
            forward,
            ortho,
            scale: w * 0.5,
            center: (0.0, 0.0),
            w,
            h,
        };
        if ortho {
            // fit the extent box: view-space bounds of its eight corners
            let lo = scene.extent_min;
            let hi = scene.extent_max;
            let mut vmin = (f32::MAX, f32::MAX);
            let mut vmax = (f32::MIN, f32::MIN);
            for i in 0..8 {
                let p = V3::new(
                    if i & 1 == 0 { lo.x } else { hi.x },
                    if i & 2 == 0 { lo.y } else { hi.y },
                    if i & 4 == 0 { lo.z } else { hi.z },
                );
                let q = cam.view(p);
                vmin = (vmin.0.min(q.x), vmin.1.min(q.y));
                vmax = (vmax.0.max(q.x), vmax.1.max(q.y));
            }
            let span_x = (vmax.0 - vmin.0).max(1.0);
            let span_y = (vmax.1 - vmin.1).max(1.0);
            let usable = 1.0 - 2.0 * ORTHO_MARGIN;
            cam.scale = (w * usable / span_x).min(h * usable / span_y);
            cam.center = ((vmin.0 + vmax.0) * 0.5, (vmin.1 + vmax.1) * 0.5);
        }
        Ok(cam)
    }

    /// World -> view space (x right, y up, z forward).
    fn view(&self, p: V3) -> V3 {
        let d = p - self.eye;
        V3::new(d.dot(self.right), d.dot(self.up), d.dot(self.forward))
    }

    /// View -> screen (x, y) and depth; `None` behind a perspective camera.
    fn project(&self, q: V3) -> Option<(f32, f32, f32)> {
        if self.ortho {
            let sx = self.w * 0.5 + (q.x - self.center.0) * self.scale;
            let sy = self.h * 0.5 - (q.y - self.center.1) * self.scale;
            Some((sx, sy, q.z))
        } else {
            if q.z < NEAR {
                return None;
            }
            let sx = self.w * 0.5 + q.x / q.z * self.scale;
            let sy = self.h * 0.5 - q.y / q.z * self.scale;
            Some((sx, sy, q.z))
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// rasteriser

struct Target {
    img: RgbaImage,
    z: Vec<f32>,
}

/// A projected corner: screen x, y, depth, colour.
#[derive(Clone, Copy)]
struct Vtx {
    x: f32,
    y: f32,
    z: f32,
    c: [f32; 3],
}

fn raster_tri(t: &mut Target, a: Vtx, b: Vtx, c: Vtx) {
    let w = t.img.width as i32;
    let h = t.img.height as i32;
    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if area.abs() < 1e-6 || !area.is_finite() {
        return;
    }
    let (b, c) = if area < 0.0 { (c, b) } else { (b, c) };
    let area = area.abs();
    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i32;
    let max_x = (a.x.max(b.x).max(c.x).ceil() as i32).min(w - 1);
    let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as i32;
    let max_y = (a.y.max(b.y).max(c.y).ceil() as i32).min(h - 1);
    if min_x > max_x || min_y > max_y {
        return;
    }
    let inv = 1.0 / area;
    for py in min_y..=max_y {
        let yf = py as f32 + 0.5;
        for px in min_x..=max_x {
            let xf = px as f32 + 0.5;
            let w0 = (b.x - xf) * (c.y - yf) - (b.y - yf) * (c.x - xf);
            let w1 = (c.x - xf) * (a.y - yf) - (c.y - yf) * (a.x - xf);
            let w2 = (a.x - xf) * (b.y - yf) - (a.y - yf) * (b.x - xf);
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let (l0, l1, l2) = (w0 * inv, w1 * inv, w2 * inv);
            let z = a.z * l0 + b.z * l1 + c.z * l2;
            let i = py as usize * w as usize + px as usize;
            if z >= t.z[i] {
                continue;
            }
            t.z[i] = z;
            let col = [
                (a.c[0] * l0 + b.c[0] * l1 + c.c[0] * l2)
                    .round()
                    .clamp(0.0, 255.0) as u8,
                (a.c[1] * l0 + b.c[1] * l1 + c.c[1] * l2)
                    .round()
                    .clamp(0.0, 255.0) as u8,
                (a.c[2] * l0 + b.c[2] * l1 + c.c[2] * l2)
                    .round()
                    .clamp(0.0, 255.0) as u8,
            ];
            t.img.put(px as u32, py as u32, col);
        }
    }
}

/// A view-space corner with its colour (before projection).
#[derive(Clone, Copy)]
struct VCorner {
    p: V3,
    c: [f32; 3],
}

/// Clip a view-space triangle against `z >= NEAR` (Sutherland-Hodgman on one plane; 0, 1 or 2 triangles).
fn clip_near(tri: [VCorner; 3]) -> Vec<[VCorner; 3]> {
    let inside = |v: &VCorner| v.p.z >= NEAR;
    let n_in = tri.iter().filter(|v| inside(v)).count();
    if n_in == 3 {
        return vec![tri];
    }
    if n_in == 0 {
        return Vec::new();
    }
    let mut out: Vec<VCorner> = Vec::with_capacity(4);
    for i in 0..3 {
        let cur = tri[i];
        let nxt = tri[(i + 1) % 3];
        let ci = inside(&cur);
        let ni = inside(&nxt);
        if ci {
            out.push(cur);
        }
        if ci != ni {
            let t = (NEAR - cur.p.z) / (nxt.p.z - cur.p.z);
            let p = cur.p + (nxt.p - cur.p) * t;
            let c = [
                cur.c[0] + (nxt.c[0] - cur.c[0]) * t,
                cur.c[1] + (nxt.c[1] - cur.c[1]) * t,
                cur.c[2] + (nxt.c[2] - cur.c[2]) * t,
            ];
            out.push(VCorner { p, c });
        }
    }
    match out.len() {
        3 => vec![[out[0], out[1], out[2]]],
        4 => vec![[out[0], out[1], out[2]], [out[0], out[2], out[3]]],
        _ => Vec::new(),
    }
}

fn draw_line(img: &mut RgbaImage, a: (f32, f32), b: (f32, f32), c: [u8; 3], alpha: f32) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as i32;
    if steps > 1 << 16 {
        return;
    }
    let w = img.width as i32;
    let h = img.height as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = (a.0 + dx * t).round() as i32;
        let y = (a.1 + dy * t).round() as i32;
        if x >= 0 && y >= 0 && x < w && y < h {
            img.blend(x as u32, y as u32, c, alpha);
        }
    }
}

fn draw_disc(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, fill: [u8; 3], ring: [u8; 3]) {
    let w = img.width as i32;
    let h = img.height as i32;
    for y in (cy - r - 1)..=(cy + r + 1) {
        for x in (cx - r - 1)..=(cx + r + 1) {
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let d2 = (x - cx) * (x - cx) + (y - cy) * (y - cy);
            if d2 <= (r - 1) * (r - 1) {
                img.put(x as u32, y as u32, fill);
            } else if d2 <= (r + 1) * (r + 1) {
                img.put(x as u32, y as u32, ring);
            }
        }
    }
}

/// Render the scene to a picture; the report carries the markers' screen positions and the legend.
pub fn render_report(
    scene: &Scene,
    view: View,
    size: (u32, u32),
    opts: &RenderOptions,
) -> Result<Rendered, Diagnostics> {
    if size.0 == 0 || size.1 == 0 || size.0 > 8192 || size.1 > 8192 {
        return Err(Diagnostics::one(
            Diagnostic::new(
                Code::OpInvalid,
                "--size",
                format!("{}x{} (1..8192 on each side)", size.0, size.1),
            )
            .with_fix("e.g. 1024x768"),
        ));
    }
    let cam = Camera::new(&view, scene, size)?;
    let mut t = Target {
        img: RgbaImage::new(size.0, size.1, opts.background),
        z: vec![f32::MAX; size.0 as usize * size.1 as usize],
    };
    let ortho = view.is_ortho();
    let mut triangles = 0;
    for tri in &scene.tris {
        if ortho && tri.sky {
            continue;
        }
        let corners = [0, 1, 2].map(|i| VCorner {
            p: cam.view(tri.v[i]),
            c: [tri.c[i][0] as f32, tri.c[i][1] as f32, tri.c[i][2] as f32],
        });
        let parts = if ortho {
            vec![corners]
        } else {
            clip_near(corners)
        };
        for part in parts {
            let mut vt = [Vtx {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                c: [0.0; 3],
            }; 3];
            let mut ok = true;
            for (i, vc) in part.iter().enumerate() {
                match cam.project(vc.p) {
                    // depth: view z (orthographic) or -1/z (perspective: 1/z is affine across the screen, so
                    // a large wall and a small near object sort correctly; smaller = nearer in both cases)
                    Some((x, y, z)) => {
                        let depth = if ortho { z } else { -1.0 / z };
                        vt[i] = Vtx {
                            x,
                            y,
                            z: depth,
                            c: vc.c,
                        }
                    }
                    None => ok = false,
                }
            }
            if ok {
                raster_tri(&mut t, vt[0], vt[1], vt[2]);
                triangles += 1;
            }
        }
    }
    let mut grid_step = None;
    if opts.grid && ortho && opts.grid_step > 0.0 {
        let step = opts.grid_step;
        grid_step = Some(step);
        let lo = scene.extent_min;
        let hi = scene.extent_max;
        let y = lo.y;
        let x0 = (lo.x / step).floor() * step;
        let z0 = (lo.z / step).floor() * step;
        let max_lines = 512;
        let mut x = x0;
        let mut n = 0;
        while x <= hi.x && n < max_lines {
            let a = cam.project(cam.view(V3::new(x, y, lo.z)));
            let b = cam.project(cam.view(V3::new(x, y, hi.z)));
            if let (Some(a), Some(b)) = (a, b) {
                let strong = (x / step).round() == 0.0;
                draw_line(
                    &mut t.img,
                    (a.0, a.1),
                    (b.0, b.1),
                    [255, 255, 255],
                    if strong { 0.5 } else { 0.25 },
                );
            }
            x += step;
            n += 1;
        }
        let mut z = z0;
        n = 0;
        while z <= hi.z && n < max_lines {
            let a = cam.project(cam.view(V3::new(lo.x, y, z)));
            let b = cam.project(cam.view(V3::new(hi.x, y, z)));
            if let (Some(a), Some(b)) = (a, b) {
                let strong = (z / step).round() == 0.0;
                draw_line(
                    &mut t.img,
                    (a.0, a.1),
                    (b.0, b.1),
                    [255, 255, 255],
                    if strong { 0.5 } else { 0.25 },
                );
            }
            z += step;
            n += 1;
        }
    }
    let mut markers = Vec::new();
    if opts.labels {
        let r = ((size.0.min(size.1) as f32 / 512.0 * 5.0).round() as i32).clamp(3, 12);
        for m in &scene.markers {
            let team = m
                .key
                .rsplit("team ")
                .next()
                .filter(|_| m.key.contains(" team "));
            let (_, color) = marker_style(&m.class, team.unwrap_or(""));
            let q = cam.view(m.at);
            let (x, y, visible) = match cam.project(q) {
                Some((x, y, _)) => {
                    let inside = x >= 0.0 && y >= 0.0 && x < cam.w && y < cam.h;
                    (x.round() as i32, y.round() as i32, inside)
                }
                None => (0, 0, false),
            };
            if visible {
                draw_disc(&mut t.img, x, y, r, color, [20, 20, 24]);
            }
            markers.push(MarkerHit {
                id: m.id.clone(),
                class: m.class.clone(),
                key: m.key.clone(),
                x,
                y,
                visible,
            });
        }
    }
    let top_bounds = if view == View::Top {
        let hx = cam.w * 0.5 / cam.scale;
        let hz = cam.h * 0.5 / cam.scale;
        Some([
            cam.center.0 - hx + cam.eye.x,
            cam.center.1 - hz + cam.eye.z,
            cam.center.0 + hx + cam.eye.x,
            cam.center.1 + hz + cam.eye.z,
        ])
    } else {
        None
    };
    Ok(Rendered {
        image: t.img,
        view,
        markers,
        legend: if opts.labels {
            scene.legend()
        } else {
            Vec::new()
        },
        grid_step,
        triangles,
        top_bounds,
    })
}

/// Render the scene to a picture (the stage-1 entry; [`render_report`] also returns the markers and legend).
pub fn render(
    scene: &Scene,
    view: View,
    size: (u32, u32),
    opts: RenderOptions,
) -> Result<RgbaImage, Diagnostics> {
    render_report(scene, view, size, &opts).map(|r| r.image)
}

/// Parse `WxH`.
pub fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("`{s}`: want WxH"))?;
    let w: u32 = w.trim().parse().map_err(|_| format!("`{s}`: bad width"))?;
    let h: u32 = h.trim().parse().map_err(|_| format!("`{s}`: bad height"))?;
    if w == 0 || h == 0 {
        return Err(format!("`{s}`: zero size"));
    }
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_material_past_the_palette_does_not_repeat_an_earlier_colour() {
        let colours: Vec<[u8; 3]> = (0..24).map(material_color).collect();
        for (i, c) in colours.iter().enumerate() {
            assert!(!colours[..i].contains(c), "material {i} repeats {c:?}");
        }
    }
    use crate::geom::{solid_polys, Surface};
    use crate::project::{MapObject, PropValue, Solid};

    fn box_level() -> Level {
        let mut level = Level::default();
        let s = Solid {
            size: [512.0, 64.0, 512.0],
            ..Default::default()
        };
        for p in solid_polys(&s, &|_| Surface::default()).unwrap() {
            level.push(p);
        }
        level.update_extent();
        level
    }

    #[test]
    fn views_parse_and_a_level_flattens_to_triangles() {
        assert_eq!(View::parse("top").unwrap(), View::Top);
        assert_eq!(View::parse("spawn:3").unwrap(), View::Spawn(3));
        assert!(
            matches!(View::parse("free:1,2,3,4,5").unwrap(), View::Free { yaw, .. } if yaw == 4.0)
        );
        assert!(View::parse("free:1,2").is_err());
        assert!(View::parse("side").is_err());
        assert_eq!(View::parse("spawn:2").unwrap().slug(), "spawn_2");
        assert_eq!(View::Iso.text(), "iso");
        assert_eq!(parse_size("640x480").unwrap(), (640, 480));
        assert!(parse_size("640").is_err());
        let mut level = Level::default();
        let s = Solid::default();
        for p in solid_polys(&s, &|_| Surface::default()).unwrap() {
            level.push(p);
        }
        level.update_extent();
        let scene = Scene::from_level(&level, &RenderOptions::default());
        assert_eq!(scene.tris.len(), 12);
    }

    #[test]
    fn the_top_view_draws_the_box_in_the_middle_and_leaves_the_corners_to_the_background() {
        let level = box_level();
        let scene = Scene::from_level(&level, &RenderOptions::default());
        let img = render(&scene, View::Top, (128, 96), RenderOptions::default()).unwrap();
        assert_eq!((img.width, img.height), (128, 96));
        let bg = RenderOptions::default().background;
        let centre = img.get(64, 48);
        assert_ne!(
            [centre[0], centre[1], centre[2]],
            bg,
            "the box top is drawn"
        );
        let corner = img.get(0, 0);
        assert_eq!(
            [corner[0], corner[1], corner[2]],
            bg,
            "the margin stays background"
        );
        // deterministic: same input, same bytes, and the PNG decodes back to the same size
        let again = render(&scene, View::Top, (128, 96), RenderOptions::default()).unwrap();
        assert_eq!(again, img);
        let png = img.png();
        let back = image::load_from_memory(&png).unwrap();
        assert_eq!((back.width(), back.height()), (128, 96));
    }

    #[test]
    fn sky_faces_are_skipped_above_but_drawn_in_perspective_and_the_near_plane_clips() {
        let mut level = box_level();
        // a sky lid above the box
        let lid = Solid {
            pos: [0.0, 400.0, 0.0],
            size: [600.0, 16.0, 600.0],
            ..Default::default()
        };
        let sky = Surface {
            flags: crate::geom::SURF_SOLID | SURF_SKY,
            ..Default::default()
        };
        for p in solid_polys(&lid, &|_| sky).unwrap() {
            level.push(p);
        }
        level.update_extent();
        assert_eq!(level.sky_faces.len(), 6);
        let scene = Scene::from_level(&level, &RenderOptions::default());
        let top = render(&scene, View::Top, (64, 64), RenderOptions::default()).unwrap();
        let c = top.get(32, 32);
        assert_ne!(
            [c[0], c[1], c[2]],
            SKY_COLOR,
            "the lid does not hide the box from above"
        );
        // from inside the lid, looking up: sky colour
        let up = View::Free {
            pos: [0.0, 100.0, 0.0],
            yaw: 0.0,
            pitch: 89.0,
        };
        let img = render(&scene, up, (64, 64), RenderOptions::default()).unwrap();
        let c = img.get(32, 32);
        assert_eq!([c[0], c[1], c[2]], SKY_COLOR);
        // a camera inside the box sees its walls, and a triangle straddling the near plane is clipped, not lost
        let inside = View::Free {
            pos: [0.0, 0.0, 0.0],
            yaw: 90.0,
            pitch: 0.0,
        };
        let r = render_report(&scene, inside, (64, 64), &RenderOptions::default()).unwrap();
        assert!(r.triangles > 0);
        let c = r.image.get(32, 32);
        assert_ne!([c[0], c[1], c[2]], RenderOptions::default().background);
    }

    #[test]
    fn markers_grid_and_spawn_cameras_report_what_they_drew() {
        let level = box_level();
        let mut scene = Scene::from_level(&level, &RenderOptions::default());
        let mut p = Project::default();
        let mut props = std::collections::BTreeMap::new();
        props.insert(v::PROP_TEAM.to_string(), PropValue::Text("1".into()));
        p.objects.push(MapObject {
            id: "player_spawn_1".into(),
            pos: [-200.0, 72.0, 0.0],
            rot: [0.0, 90.0, 0.0],
            props,
            ..Default::default()
        });
        p.objects.push(MapObject {
            id: "item_spawn_2".into(),
            class: v::CLASS_ITEM_SPAWN.into(),
            pos: [200.0, 72.0, 100.0],
            ..Default::default()
        });
        scene.add_project_objects(&p);
        assert_eq!(scene.spawns.len(), 1);
        let opts = RenderOptions {
            labels: true,
            grid: true,
            grid_step: 128.0,
            ..Default::default()
        };
        let r = render_report(&scene, View::Top, (256, 256), &opts).unwrap();
        assert_eq!(r.markers.len(), 2);
        assert_eq!(r.legend.len(), 2);
        assert_eq!(r.grid_step, Some(128.0));
        let spawn = &r.markers[0];
        assert_eq!(spawn.key, "dc.player_spawn team 1");
        assert!(
            spawn.visible && spawn.x < 128,
            "team 1 spawn at -x is on the left"
        );
        let item = &r.markers[1];
        assert!(item.x > 128 && item.y < 128, "+x right, +z up");
        let px = r.image.get(spawn.x as u32, spawn.y as u32);
        assert_eq!(
            [px[0], px[1], px[2]],
            marker_style(v::CLASS_PLAYER_SPAWN, "1").1
        );
        // the spawn camera looks along +x from the spawn: the item spawn is ahead and visible
        let r = render_report(&scene, View::Spawn(0), (128, 96), &opts).unwrap();
        assert!(r.markers[1].visible);
        assert!(
            !r.markers[0].visible,
            "the camera's own point is behind the near plane"
        );
        let e = render_report(&scene, View::Spawn(5), (16, 16), &opts).unwrap_err();
        assert_eq!(e.items[0].code, Code::RenderView);
        assert_eq!(
            render_report(&scene, View::Top, (0, 16), &opts)
                .unwrap_err()
                .items[0]
                .code,
            Code::OpInvalid
        );
    }

    #[test]
    fn a_world_file_renders_from_its_bsp_polygons_and_from_its_render_vertices() {
        let dat = dc_modkit::placeholder_world();
        let w = dc_world::World::parse(&dat).unwrap();
        let scene = Scene::from_world(&w, &RenderOptions::default());
        assert_eq!(scene.tris.len(), 2, "one floor quad = two triangles");
        let img = render(&scene, View::Top, (64, 64), RenderOptions::default()).unwrap();
        let c = img.get(32, 32);
        assert_ne!([c[0], c[1], c[2]], RenderOptions::default().background);
        // a block of one triangle with baked colours: 0x80 = unit brightness
        use dc_world::synth::{SynthBlock, SynthModel, SynthSection, SynthWorld};
        let vtx = |p: [f32; 3], rgb: [u8; 3]| -> [u8; 68] {
            let mut v = [0u8; 68];
            for (i, f) in p.iter().enumerate() {
                v[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
            }
            v[28..32].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
            v
        };
        let world = SynthWorld {
            extent_min: [-100.0, -1.0, -100.0],
            extent_max: [100.0, 1.0, 100.0],
            tree_shape: vec![false],
            models: vec![SynthModel {
                name: b"level".to_vec(),
                textures: vec![b"t".to_vec()],
                planes: vec![([0.0, 1.0, 0.0], 0.0)],
                surfaces: vec![(1, 0, 0)],
                polys: vec![(0, 0, vec![0, 1, 2])],
                nodes: vec![(0, 0, -2, -1)],
                points: vec![
                    [-100.0, 0.0, -100.0],
                    [-100.0, 0.0, 100.0],
                    [100.0, 0.0, 100.0],
                ],
                root: 0,
                box_min: [-100.0, 0.0, -100.0],
                box_max: [100.0, 0.0, 100.0],
                ..Default::default()
            }],
            render: dc_world::synth::SynthRenderWorld {
                blocks: vec![SynthBlock {
                    half: [100.0, 1.0, 100.0],
                    sections: vec![SynthSection {
                        textures: [b"t".to_vec(), Vec::new()],
                        shader_kind: 1,
                        triangles: 1,
                        ..Default::default()
                    }],
                    vertices: vec![
                        vtx([-100.0, 0.0, -100.0], [0x80, 0x80, 0x80]),
                        vtx([-100.0, 0.0, 100.0], [0x80, 0x80, 0x80]),
                        vtx([100.0, 0.0, 100.0], [0x80, 0x80, 0x80]),
                    ],
                    triangles: vec![[0, 1, 2, 0]],
                    ..Default::default()
                }],
                models: Vec::new(),
            },
            ..Default::default()
        }
        .build();
        let w = dc_world::World::parse(&world).unwrap();
        let scene = Scene::from_world(&w, &RenderOptions::default());
        assert_eq!(scene.tris.len(), 1, "render data wins over the BSP polygon");
        assert_eq!(scene.tris[0].c[0], [255, 255, 255]);
    }
}
