//! The world map: a key (M by default) shows the whole map over the match, north up, with the own position and
//! facing and the team mates, so a player sees where the map goes (DC-DEFAULT: CLI-1953).
//!
//! The picture is the match's radar picture (the one the radar shows a window of), or, for a map composed from
//! pieces of the player's ROM worlds (MAP-116), a top-down picture the client draws itself from the composed world
//! when it loads ([`top_map`]): such a map's pack holds only its own geometry, and its radar picture shows nothing
//! of the pieces. The picture is made on the player's machine from the world it loaded and is never stored or sent.
// DC-DEFAULT: CLI-1953 (proposed) the world map overlay, its key and its picture

use super::compose::{HudAssets, Tex};
use super::items::DrawItem;
use super::radar::RadarMap;
use super::state::HudState;
use dc_ui_data::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use dc_world::RenderBlock;

/// The name the client's own world picture is stored under in the HUD's textures (not an archive path).
pub const WORLD_MAP_TEX: &str = "@client:world_map";

/// The side of the client's own world picture (pixels).
pub const TOP_MAP_SIZE: u32 = 1024;

/// A picture of the world from above and the world rectangle it covers: `[x0, z0, x1, z1]`, world +X to the
/// right and +Z up (north up, as the radar).
#[derive(Debug, Clone)]
pub struct TopMap {
    pub tex: Tex,
    pub bounds: [f32; 4],
}

/// A face this much towards up (the y of its unit normal) is ground or a roof; steeper ones are walls.
const FLOOR_UP: f32 = 0.3;
/// A height step this large between neighbouring pixels draws an edge (a wall, a cliff).
const EDGE_STEP: f32 = 80.0;
/// Render vertex layout (FMT_DAT85.md §9.2): the position's three floats first.
const VERTEX_BYTES: usize = 68;

fn vertex(b: &RenderBlock<'_>, i: u32) -> Option<[f32; 3]> {
    let o = (i as usize).checked_mul(VERTEX_BYTES)?;
    let v = b.vertices.get(o..o + 12)?;
    let f = |k: usize| f32::from_le_bytes([v[k], v[k + 1], v[k + 2], v[k + 3]]);
    Some([f(0), f(4), f(8)])
}

/// Every upward triangle of the world's render blocks (the level as drawn: no collision-only lid, no sky).
fn floor_triangles(blocks: &[RenderBlock<'_>]) -> Vec<([[f32; 3]; 3], f32)> {
    let mut out = Vec::new();
    for b in blocks {
        for t in 0..b.triangle_count {
            let Some(tri) = b.triangle(t) else { continue };
            let (Some(p0), Some(p1), Some(p2)) =
                (vertex(b, tri[0]), vertex(b, tri[1]), vertex(b, tri[2]))
            else {
                continue;
            };
            let (u, v) = (sub(p1, p0), sub(p2, p0));
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if !(len > 1e-6) {
                continue;
            }
            let up = (n[1] / len).abs();
            if up < FLOOR_UP || [p0, p1, p2].iter().flatten().any(|c| !c.is_finite()) {
                continue;
            }
            out.push(([p0, p1, p2], up));
        }
    }
    out
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The world seen from above: the highest upward face over each pixel, coloured by its height and lit by how
/// steep it is, with an edge where the height jumps; `None` when the blocks hold no upward face.
pub fn top_map(blocks: &[RenderBlock<'_>], size: u32) -> Option<TopMap> {
    let tris = floor_triangles(blocks);
    if tris.is_empty() || size == 0 {
        return None;
    }
    let (mut x0, mut z0, mut x1, mut z1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let (mut y0, mut y1) = (f32::MAX, f32::MIN);
    for (p, _) in &tris {
        for c in p {
            x0 = x0.min(c[0]);
            x1 = x1.max(c[0]);
            z0 = z0.min(c[2]);
            z1 = z1.max(c[2]);
            y0 = y0.min(c[1]);
            y1 = y1.max(c[1]);
        }
    }
    // a square picture over the square around the floor
    let side = (x1 - x0).max(z1 - z0).max(1.0);
    let (cx, cz) = ((x0 + x1) / 2.0, (z0 + z1) / 2.0);
    let bounds = [
        cx - side / 2.0,
        cz - side / 2.0,
        cx + side / 2.0,
        cz + side / 2.0,
    ];
    let n = size as usize;
    let per = side / size as f32;
    let mut height = vec![f32::NEG_INFINITY; n * n];
    let mut light = vec![0.0f32; n * n];
    // picture column of world x, row of world z (north up: row 0 is the largest z)
    let col = |x: f32| (x - bounds[0]) / per;
    let row = |z: f32| (bounds[3] - z) / per;
    for (p, up) in &tris {
        let s = p.map(|c| [col(c[0]), row(c[2]), c[1]]);
        let lo_c = s
            .iter()
            .map(|q| q[0])
            .fold(f32::MAX, f32::min)
            .floor()
            .max(0.0) as usize;
        let hi_c = (s.iter().map(|q| q[0]).fold(f32::MIN, f32::max).ceil() as usize).min(n - 1);
        let lo_r = s
            .iter()
            .map(|q| q[1])
            .fold(f32::MAX, f32::min)
            .floor()
            .max(0.0) as usize;
        let hi_r = (s.iter().map(|q| q[1]).fold(f32::MIN, f32::max).ceil() as usize).min(n - 1);
        if lo_c > hi_c || lo_r > hi_r {
            continue;
        }
        let area =
            (s[1][0] - s[0][0]) * (s[2][1] - s[0][1]) - (s[2][0] - s[0][0]) * (s[1][1] - s[0][1]);
        let top = s[0][2].max(s[1][2]).max(s[2][2]);
        let mut put = |c: usize, r: usize, y: f32| {
            let i = r * n + c;
            if y > height[i] {
                height[i] = y;
                light[i] = *up;
            }
        };
        if area.abs() < 1.0 {
            // under a pixel: its highest corner marks the pixel it lies in
            let (c, r) = (
                (s[0][0].max(0.0) as usize).min(n - 1),
                (s[0][1].max(0.0) as usize).min(n - 1),
            );
            put(c, r, top);
            continue;
        }
        for r in lo_r..=hi_r {
            for c in lo_c..=hi_c {
                let (px, py) = (c as f32 + 0.5, r as f32 + 0.5);
                let w0 = ((s[1][0] - px) * (s[2][1] - py) - (s[2][0] - px) * (s[1][1] - py)) / area;
                let w1 = ((s[2][0] - px) * (s[0][1] - py) - (s[0][0] - px) * (s[2][1] - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                    continue;
                }
                put(c, r, w0 * s[0][2] + w1 * s[1][2] + w2 * s[2][2]);
            }
        }
    }
    let span = (y1 - y0).max(1.0);
    let mut rgba = vec![0u8; n * n * 4];
    for r in 0..n {
        for c in 0..n {
            let i = r * n + c;
            let px = &mut rgba[i * 4..i * 4 + 4];
            let h = height[i];
            if !h.is_finite() {
                px.copy_from_slice(&[18, 20, 26, 255]);
                continue;
            }
            let mut rgb = palette(((h - y0) / span).clamp(0.0, 1.0));
            // steeper ground is darker (a slope away from the light reads as relief)
            let mut k = 0.55 + 0.45 * light[i];
            let edge = [(0isize, 1isize), (1, 0), (0, -1), (-1, 0)]
                .iter()
                .any(|&(dr, dc)| {
                    let (rr, cc) = (r as isize + dr, c as isize + dc);
                    if rr < 0 || cc < 0 || rr >= n as isize || cc >= n as isize {
                        return false;
                    }
                    let o = height[rr as usize * n + cc as usize];
                    !o.is_finite() || (o - h).abs() > EDGE_STEP
                });
            if edge {
                k *= 0.5;
            }
            for v in rgb.iter_mut() {
                *v = (*v as f32 * k).round().clamp(0.0, 255.0) as u8;
            }
            px.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    Some(TopMap {
        tex: Tex {
            width: size,
            height: size,
            rgba,
        },
        bounds,
    })
}

/// Our colours by height, low to high: deep blue-grey, sand, olive, light stone.
fn palette(t: f32) -> [u8; 3] {
    const STOPS: [(f32, [f32; 3]); 4] = [
        (0.0, [52.0, 66.0, 84.0]),
        (0.35, [176.0, 160.0, 120.0]),
        (0.6, [128.0, 142.0, 96.0]),
        (1.0, [222.0, 222.0, 214.0]),
    ];
    for w in STOPS.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if t <= b {
            let f = ((t - a) / (b - a)).clamp(0.0, 1.0);
            return [0, 1, 2].map(|k| (ca[k] + (cb[k] - ca[k]) * f) as u8);
        }
    }
    STOPS[3].1.map(|v| v as u8)
}

/// Where the overlay draws: a square in the middle of the 1024 x 768 virtual screen.
const MAP_SIDE: f32 = 700.0;
const MAP_LEFT: f32 = (VIRTUAL_WIDTH as f32 - MAP_SIDE) / 2.0;
const MAP_TOP: f32 = (VIRTUAL_HEIGHT as f32 - MAP_SIDE) / 2.0;

/// The picture the overlay shows and how a world point maps to it (picture uv, 0..1 from the top-left).
enum Shown {
    Own([f32; 4]),
    Radar(RadarMap),
}

impl Shown {
    fn uv(&self, p: [f32; 3]) -> [f32; 2] {
        match self {
            Shown::Own(b) => [(p[0] - b[0]) / (b[2] - b[0]), (b[3] - p[2]) / (b[3] - b[1])],
            Shown::Radar(m) => m.uv(p),
        }
    }
}

/// The overlay's items while the map is open: a dark backdrop, the picture, the team mates, the own position and
/// facing. Nothing when it is closed or there is no picture.
pub fn push_items(out: &mut Vec<DrawItem>, s: &HudState, assets: &mut HudAssets) {
    if !s.map_open {
        return;
    }
    let (name, shown) = match (&s.world_map, assets.generated(WORLD_MAP_TEX)) {
        (Some(b), Some(_)) => (WORLD_MAP_TEX.to_string(), Shown::Own(*b)),
        _ => match s.intro_block.as_ref().and_then(RadarMap::of) {
            Some(m) => (m.picture.clone(), Shown::Radar(m)),
            None => return,
        },
    };
    let Some(tex) = assets.texture(&name) else {
        return;
    };
    out.push(DrawItem::Fill {
        rect: [0.0, 0.0, VIRTUAL_WIDTH as f32, VIRTUAL_HEIGHT as f32],
        colour: [0, 0, 0, 170],
    });
    out.push(DrawItem::Image {
        tex: name,
        src: [tex.width as f32, tex.height as f32],
        dst: [MAP_LEFT, MAP_TOP, MAP_SIDE, MAP_SIDE],
        tint: [255, 255, 255, 255],
        turn: None,
    });
    let at = |p: [f32; 3]| {
        let uv = shown.uv(p);
        [
            MAP_LEFT + uv[0].clamp(0.0, 1.0) * MAP_SIDE,
            MAP_TOP + uv[1].clamp(0.0, 1.0) * MAP_SIDE,
        ]
    };
    let dot = |out: &mut Vec<DrawItem>, c: [f32; 2], r: f32, colour: [u8; 4]| {
        out.push(DrawItem::Fill {
            rect: [c[0] - r, c[1] - r, 2.0 * r, 2.0 * r],
            colour,
        });
    };
    for m in &s.map_mates {
        let c = at(*m);
        dot(out, c, 5.0, [0, 0, 0, 255]);
        dot(out, c, 3.5, [90, 200, 255, 255]);
    }
    let Some(f) = &s.radar else {
        return;
    };
    let c = at(f.own);
    // the facing: dots ahead of the own mark (yaw from +Z towards +X; screen up is +Z)
    let (dx, dy) = (f.yaw.sin(), -f.yaw.cos());
    for k in 1..=4 {
        let d = 4.0 + 4.0 * k as f32;
        dot(
            out,
            [c[0] + dx * d, c[1] + dy * d],
            2.0,
            [255, 230, 80, 255],
        );
    }
    dot(out, c, 6.0, [0, 0, 0, 255]);
    dot(out, c, 4.5, [255, 230, 80, 255]);
}

/// The team mates' world points for the overlay: alive players of the own team (the radar's team rule).
pub fn mates(s: &HudState, remotes: &[crate::world::RemoteBody]) -> Vec<[f32; 3]> {
    let Some(team) = super::radar::own_team_of(s) else {
        return Vec::new();
    };
    remotes
        .iter()
        .filter(|r| Some(r.pid) != s.own_pid)
        .filter(|r| {
            let row = s.roster.get(&r.pid);
            !row.is_some_and(|b| b.dead) && row.map_or(r.team, |b| b.team) == team
        })
        .map(|r| r.pos)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One render block of two quads: a floor at y 0 over x/z 0..1000 and a roof at y 300 over x 0..200, z 0..200.
    fn block_bytes() -> (Vec<u8>, Vec<u8>) {
        let mut v = Vec::new();
        let mut put = |p: [f32; 3]| {
            let mut b = [0u8; VERTEX_BYTES];
            for (k, x) in p.iter().enumerate() {
                b[4 * k..4 * k + 4].copy_from_slice(&x.to_le_bytes());
            }
            v.extend_from_slice(&b);
        };
        for p in [
            [0.0, 0.0, 0.0],
            [1000.0, 0.0, 0.0],
            [1000.0, 0.0, 1000.0],
            [0.0, 0.0, 1000.0],
            [0.0, 300.0, 0.0],
            [200.0, 300.0, 0.0],
            [200.0, 300.0, 200.0],
            [0.0, 300.0, 200.0],
        ] {
            put(p);
        }
        let mut t = Vec::new();
        for tri in [[0u32, 2, 1], [0, 3, 2], [4, 6, 5], [4, 7, 6]] {
            for i in tri.iter().chain(std::iter::once(&0)) {
                t.extend_from_slice(&i.to_le_bytes());
            }
        }
        (v, t)
    }

    fn block<'a>(v: &'a [u8], t: &'a [u8]) -> RenderBlock<'a> {
        RenderBlock {
            center: [500.0, 150.0, 500.0],
            half: [500.0, 150.0, 500.0],
            sections: Vec::new(),
            vertex_count: (v.len() / VERTEX_BYTES) as u32,
            vertices: v,
            triangle_count: (t.len() / 16) as u32,
            triangles: t,
            sky_portals: Vec::new(),
            occluders: Vec::new(),
            light_groups: Vec::new(),
            child_mask: 0,
            children: [0, 0],
        }
    }

    #[test]
    fn the_top_map_shows_the_highest_floor_north_up() {
        let (v, t) = block_bytes();
        let m = top_map(&[block(&v, &t)], 100).expect("a picture");
        assert_eq!((m.tex.width, m.tex.height), (100, 100));
        assert_eq!(m.bounds, [0.0, 0.0, 1000.0, 1000.0]);
        let px = |c: usize, r: usize| {
            let i = (r * 100 + c) * 4;
            [m.tex.rgba[i], m.tex.rgba[i + 1], m.tex.rgba[i + 2]]
        };
        // the roof (high, light) lies at small x and small z: left and at the BOTTOM (north up)
        let roof = px(10, 90);
        let floor = px(60, 40);
        assert!(roof.iter().map(|&x| x as u32).sum::<u32>() > 0);
        assert!(
            roof.iter().map(|&x| x as u32).sum::<u32>()
                > floor.iter().map(|&x| x as u32).sum::<u32>(),
            "the roof is lighter than the floor: {roof:?} {floor:?}"
        );
        // the roof's edge against the floor is drawn darker than the roof inside
        let edge = px(20, 90);
        assert!(
            edge.iter().map(|&x| x as u32).sum::<u32>()
                < roof.iter().map(|&x| x as u32).sum::<u32>()
        );
    }

    #[test]
    fn walls_and_nothing_give_no_picture() {
        assert!(top_map(&[], 64).is_none());
    }

    #[test]
    fn the_own_picture_maps_north_up() {
        let s = Shown::Own([0.0, 0.0, 1000.0, 1000.0]);
        assert_eq!(s.uv([0.0, 0.0, 1000.0]), [0.0, 0.0]);
        assert_eq!(s.uv([1000.0, 0.0, 0.0]), [1.0, 1.0]);
    }
}
