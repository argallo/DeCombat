//! The radar at the top right of the in-match HUD (the 2009 element `hud_radar`, FMT_CRES_UI §8.1 row 4).
//!
//! What the 2009 client draws, in order (cshell+0xb5f10, the element's draw slot):
//!
//! 1. **The map layer**: a fixed rectangle (the layout's radar place and drawn size) showing a square window of the
//!    match's radar picture centred on the own camera object. North is fixed: the window is axis aligned, world +X
//!    to the right, world +Z up. The colour comes from the picture, the alpha from the layout's disc texture (two
//!    texture stages: stage 0 the disc, alpha only; stage 1 the picture, colour only), so the picture shows through
//!    a round, translucent disc.
//! 2. **The view disc**: a second layout texture (a dark disc with the view wedge and the own arrow) turned about the
//!    radar centre so that its top points along the view.
//! 3. **The entries**: one icon per entry of the element's object list. Players enter that list through the roster
//!    (kind 0); a team mate is drawn with the friendly icon while his distance on the radar is within the layout
//!    radius; an enemy only while the radar accessory reveals him (radar pings 0xcd and 0x84 with a range), so never
//!    in a plain match; a dead player (0x6d) and the own player never.
//!
//! [`RadarKeys`] reads the layout key names from the user's CShell.dll (pinned like dc-ui-data's HUD keys; no key
//! text is in this tree), [`RadarLayout`] holds the values (or our own no-ROM layout), [`RadarMap`] the per-match
//! picture and mapping of the 0x45 intro block, [`frame`] decides the entries of a frame and [`push_items`] turns
//! everything into display-list items (the two generated layer textures are composed on the CPU here).
// SPEC: FMT_CRES_UI.md §8.1 (element 4 `hud_radar`, ctor cshell+0xb9140, method table cshell+0x3d9010, update slot 35);
// GAME_MESSAGES_2009_S2C.md §2.1.1 step 5 fields f..h (picture, two vectors, two reals, two sizes); RENDER_2009.md
// §12.3 (draw-primitive interface: colour op 4 = two texture stages)
// EVIDENCE: cshell+0xb3e30 (init: layout keys, texture handles), cshell+0xb4bc0 (map quad place and size),
// cshell+0xb9410 (per frame: map window, view-disc corners), cshell+0xb60f0 (entries), cshell+0xb5f10 (draw)
// SPEC: rust/specs/RADAR_2009.md (this module's rules in our words)

use super::compose::{HudAssets, Tex};
use super::items::DrawItem;
use super::msgs::IntroBlock;
use super::state::HudState;
use crate::world::RemoteBody;
use dc_rom::PeFile;
use dc_ui_data::layout::parse_value;
use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::{LayoutTable, UiDataError, Value, VirtualScaler};
use std::path::Path;
use std::sync::Arc;

/// One radar layout key (our names). All live in the numbered HUD layout set with the set-0 fallback.
// EVIDENCE: cshell+0x53cd0 .. cshell+0x54b40 (one accessor per key: `HUD set prefix + n`, else set 0, cshell+0x4bce0
// family)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RadarKey {
    /// Real: x of the entries' origin, relative to the radar place (the icon's top-left at zero distance).
    BlipOriginX,
    /// Real: y of the same.
    BlipOriginY,
    /// Real: screen scale of the entries' radar distance.
    BlipScale,
    /// Pair: the radar's top-left (virtual pixels).
    Place,
    /// Pair: entry icon size (virtual pixels).
    IconSize,
    /// Int: the largest radar distance of an entry (virtual pixels).
    Radius,
    /// Text: the map picture before a match names its own.
    DefaultMap,
    /// Text: the disc texture whose alpha shapes the map layer.
    Disc,
    /// Text: the team-mate icon.
    MateIcon,
    /// Text: the turning view disc.
    ViewDisc,
    /// Real: world distance to radar distance factor of the entries.
    CharScale,
    /// Pair: full texel size of the two disc textures.
    DiscTexels,
    /// Pair: the used texel region of the two disc textures (= the drawn size in virtual pixels).
    DiscUsed,
    /// Text: the rim image under the disc, a printf form with one `%d` (the accessory charge, 0 or 100).
    RimForm,
    /// Pair: the rim image's texel size (drawn 1:1 at the radar place).
    RimSize,
}

/// `(key, literal rva, sha16, length, accessor rva)`.
const PINS: [(RadarKey, u32, &str, usize, u32); 15] = [
    (
        RadarKey::BlipOriginX,
        0x3d20f0,
        "366a1bf92a0e4ed9",
        12,
        0x53cd0,
    ),
    (
        RadarKey::BlipOriginY,
        0x3d2100,
        "3d8b26ccfe098416",
        12,
        0x53d40,
    ),
    (
        RadarKey::BlipScale,
        0x3d2124,
        "55c152505130fdcf",
        14,
        0x53e20,
    ),
    (RadarKey::Place, 0x3d2180, "b0169e467b49e3f1", 8, 0x540c0),
    (
        RadarKey::IconSize,
        0x3d2198,
        "593e5865d42a1f0d",
        13,
        0x541a0,
    ),
    (RadarKey::Radius, 0x3d21ec, "2ad5534d3d421ddd", 11, 0x543d0),
    (
        RadarKey::DefaultMap,
        0x3d21f8,
        "05b75f0e6efb803f",
        17,
        0x54440,
    ),
    (RadarKey::Disc, 0x3d220c, "ea6b3adb6badbfb2", 15, 0x54500),
    (
        RadarKey::MateIcon,
        0x3d2228,
        "8d141c081813d002",
        16,
        0x54680,
    ),
    (RadarKey::ViewDisc, 0x3d221c, "962fbf029f897a67", 8, 0x545c0),
    (
        RadarKey::CharScale,
        0x3d22cc,
        "92be87df004c1529",
        21,
        0x54b40,
    ),
    (
        RadarKey::DiscTexels,
        0x3d2168,
        "13a014a7b6c002ee",
        11,
        0x53fe0,
    ),
    (RadarKey::DiscUsed, 0x3d2174, "e4463ed9830ed7fa", 9, 0x54050),
    (RadarKey::RimForm, 0x3d2264, "7365824dea260bb9", 15, 0x548c0),
    (RadarKey::RimSize, 0x3d218c, "bbd98df9e329b2dd", 9, 0x54130),
];

impl RadarKey {
    pub const ALL: [RadarKey; 15] = [
        RadarKey::BlipOriginX,
        RadarKey::BlipOriginY,
        RadarKey::BlipScale,
        RadarKey::Place,
        RadarKey::IconSize,
        RadarKey::Radius,
        RadarKey::DefaultMap,
        RadarKey::Disc,
        RadarKey::MateIcon,
        RadarKey::ViewDisc,
        RadarKey::CharScale,
        RadarKey::DiscTexels,
        RadarKey::DiscUsed,
        RadarKey::RimForm,
        RadarKey::RimSize,
    ];

    fn row(self) -> (RadarKey, u32, &'static str, usize, u32) {
        PINS[self as usize]
    }

    /// The pinned literal of the key in CShell.dll.
    pub fn pin(self) -> Pin {
        let r = self.row();
        Pin {
            what: "radar layout key",
            rva: r.1,
            sha16: r.2,
            len: r.3,
        }
    }

    /// Rva of the accessor that reads the key (the pin's evidence).
    pub fn evidence(self) -> u32 {
        self.row().4
    }
}

/// The radar key names of the user's CShell.dll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadarKeys {
    texts: Vec<String>,
}

impl RadarKeys {
    pub fn from_rom(rom: &Path) -> Result<Self, UiDataError> {
        let p = rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]);
        Self::from_cshell(&PeFile::from_file(&p)?)
    }

    /// Every key from a loaded CShell.dll; a mismatch means "not the 2009 build".
    pub fn from_cshell(pe: &PeFile) -> Result<Self, UiDataError> {
        let texts = RadarKey::ALL
            .iter()
            .map(|k| read_pinned(pe, &k.pin()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RadarKeys { texts })
    }

    /// Invented names for fixtures without a ROM (`radar_key_<n>`).
    pub fn placeholder() -> Self {
        RadarKeys {
            texts: (0..RadarKey::ALL.len())
                .map(|i| format!("radar_key_{i}"))
                .collect(),
        }
    }

    pub fn key(&self, k: RadarKey) -> &str {
        &self.texts[k as usize]
    }
}

/// Where the radar is drawn and with what. Positions and sizes in the 1024 x 768 virtual screen.
#[derive(Debug, Clone, PartialEq)]
pub struct RadarLayout {
    /// The rim image drawn first at `place`, 1:1 from its top-left texels (texture, texel size); `None`: no rim.
    pub rim: Option<(String, [f32; 2])>,
    /// Top-left of the map layer.
    pub place: [f32; 2],
    /// Drawn size of the map layer (also the used texel region of the disc textures).
    pub size: [f32; 2],
    /// Full texel size the used region is a part of (the uv of the disc textures = `size / disc_texels`).
    pub disc_texels: [f32; 2],
    /// Disc texture (its alpha shapes the map layer); `None`: our generated disc.
    pub disc: Option<String>,
    /// The turning view disc; `None`: our generated one.
    pub view: Option<String>,
    /// The map picture before the match names one.
    pub default_map: Option<String>,
    /// Team-mate icon; `None`: a filled square of `mate_colour`.
    pub mate_icon: Option<String>,
    pub mate_colour: [u8; 4],
    /// The entries' origin relative to `place` (an icon's top-left at zero distance).
    pub blip_origin: [f32; 2],
    pub icon: [f32; 2],
    /// Largest radar distance of a drawn entry (virtual pixels).
    pub radius: f32,
    /// World distance to radar distance.
    pub char_scale: f32,
    /// Radar distance to virtual pixels.
    pub blip_scale: f32,
}

impl RadarLayout {
    /// The layout from HUD set `set` of the ROM table; `None` when a key the radar needs is missing.
    // EVIDENCE: cshell+0xb3e30 (every value read once at init), cshell+0xb4bc0 (map quad = place x size, both scaled
    // per axis to the screen), cshell+0xb42ad..0xb42e2 (disc uv = used region / full size)
    pub fn from_rom(table: &LayoutTable, keys: &RadarKeys, set: u32) -> Option<RadarLayout> {
        let v = |k: RadarKey| table.hud_raw(set, keys.key(k)).map(parse_value);
        let pair = |k: RadarKey| match v(k)? {
            Value::Pair(p) => Some([p[0] as f32, p[1] as f32]),
            _ => None,
        };
        let real = |k: RadarKey| match v(k)? {
            Value::Int(i) => Some(i as f32),
            Value::Real(r) => Some(r as f32),
            _ => None,
        };
        let text = |k: RadarKey| {
            table
                .hud_raw(set, keys.key(k))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        // EVIDENCE: cshell+0xb40b4..0xb4149 / cshell+0xb4375..0xb441f (two rim images: the form printed with 100 and
        // with 0, each an image item of the rim size at the radar place); DISASM cshell+0xb5fa4..0xb5fc3 (the 0 image is
        // drawn while the accessory flag +0x3a0 is clear, which it always is without the accessory)
        let rim = text(RadarKey::RimForm)
            .map(|f| f.replacen("%d", "0", 1))
            .zip(pair(RadarKey::RimSize));
        Some(RadarLayout {
            rim,
            place: pair(RadarKey::Place)?,
            size: pair(RadarKey::DiscUsed)?,
            disc_texels: pair(RadarKey::DiscTexels)?,
            disc: text(RadarKey::Disc),
            view: text(RadarKey::ViewDisc),
            default_map: text(RadarKey::DefaultMap),
            mate_icon: text(RadarKey::MateIcon),
            mate_colour: FALLBACK_MATE,
            blip_origin: [
                real(RadarKey::BlipOriginX).unwrap_or(0.0),
                real(RadarKey::BlipOriginY).unwrap_or(0.0),
            ],
            icon: pair(RadarKey::IconSize)?,
            radius: real(RadarKey::Radius).unwrap_or(0.0),
            char_scale: real(RadarKey::CharScale).unwrap_or(0.0),
            blip_scale: real(RadarKey::BlipScale).unwrap_or(0.0),
        })
    }

    /// Our own radar without a ROM: a generated disc at the top right, team mates as small squares.
    // DC-DEFAULT: CLI-140 (proposed) the no-ROM radar's place, size, icon, radius and scales are ours
    pub fn fallback() -> RadarLayout {
        RadarLayout {
            rim: None,
            place: [844.0, 14.0],
            size: [168.0, 168.0],
            disc_texels: [168.0, 168.0],
            disc: None,
            view: None,
            default_map: None,
            mate_icon: None,
            mate_colour: FALLBACK_MATE,
            blip_origin: [80.0, 80.0],
            icon: [8.0, 8.0],
            radius: 76.0,
            char_scale: 1.0,
            blip_scale: 0.06,
        }
    }

    /// The radar centre (centre of the map layer).
    pub fn centre(&self) -> [f32; 2] {
        [
            self.place[0] + self.size[0] * 0.5,
            self.place[1] + self.size[1] * 0.5,
        ]
    }
}

/// Team-mate colour of the no-ROM radar (the no-ROM mark colour of CLI-116).
// DC-DEFAULT: CLI-116 (the no-ROM team-mate colour)
pub const FALLBACK_MATE: [u8; 4] = [110, 190, 255, 255];

/// The match's radar picture and its mapping, from the 0x45 intro block (fields f..h).
///
/// A world point `(x, z)` lies at picture uv `(x * scale_x / width + origin_x, origin_z - z * scale_z / height)`;
/// the radar shows the square of half side `half` (uv units) around the own point.
// EVIDENCE: cshell+0x47e30 (first vector to element +0x370..+0x378), cshell+0x47e60 (second vector to +0x37c..+0x384),
// GAME_MESSAGES_2009_S2C.md §2.1.1 field h (+0x394 the first real, +0x38c / +0x390 the two sizes, +0x388 the second
// real); DISASM cshell+0xb956b..0xb95cb (stage-1 uv rectangle: u0 = x * [+0x370] / [+0x38c] + [+0x37c] - [+0x394],
// v0 = [+0x384] - z * [+0x378] / [+0x390] - [+0x394], width = height = 2 [+0x394])
#[derive(Debug, Clone, PartialEq)]
pub struct RadarMap {
    pub picture: String,
    /// x and z of the first vector.
    pub scale: [f32; 2],
    /// x and z of the second vector.
    pub origin: [f32; 2],
    /// The first real (`radar_half` in the message reader): half side of the shown square, picture uv.
    pub half: f32,
    /// The two sizes (picture width and height).
    pub image: [f32; 2],
}

impl RadarMap {
    /// The mapping of an intro block; `None` when it names no picture or a size is 0.
    pub fn of(b: &IntroBlock) -> Option<RadarMap> {
        let picture = b.radar.trim().to_string();
        if picture.is_empty() || b.radar_w == 0 || b.radar_h == 0 {
            return None;
        }
        Some(RadarMap {
            picture,
            scale: [b.radar_map_scale[0], b.radar_map_scale[2]],
            origin: [b.radar_map_offset[0], b.radar_map_offset[2]],
            half: b.radar_half,
            image: [b.radar_w as f32, b.radar_h as f32],
        })
    }

    /// The picture uv of a world point.
    pub fn uv(&self, p: [f32; 3]) -> [f32; 2] {
        [
            p[0] * self.scale[0] / self.image[0] + self.origin[0],
            self.origin[1] - p[2] * self.scale[1] / self.image[1],
        ]
    }

    /// The shown window `[u0, v0, w, h]` (picture uv) around the own point.
    pub fn window(&self, own: [f32; 3]) -> [f32; 4] {
        let c = self.uv(own);
        [
            c[0] - self.half,
            c[1] - self.half,
            2.0 * self.half,
            2.0 * self.half,
        ]
    }
}

/// The world-north angle the view disc is turned against (radians).
// EVIDENCE: cshell+0xb95f7..0xb9618 (the angle = console value `WorldNorth` + pi/4 - camera yaw, cshell+0x19fec0);
// the client copies the server variable of that name into the console value at world entry (dc-dserver
// clientvars.rs, DSV-60: DeCombat sends 0)
// DC-DEFAULT: CLI-141 (proposed) dc-client does not read the server variable yet and uses 0, the value DSV-60 sends
pub const WORLD_NORTH: f32 = 0.0;

/// Turn of the view disc, radians clockwise on the screen, for a view `yaw` (dc-sim convention: 0 looks along +Z,
/// positive toward +X).
///
/// 2009: the disc's four corners lie at screen angles `-(a + k pi/2)`, `a = north + pi/4 - yaw`, with the texture's
/// top-left corner first. That is the texture mirrored left to right and turned clockwise by `yaw - north`: its
/// top points along the view on the north-up map.
// DISASM: cshell+0xb962d..0xb970f (centre = place + used size / 2, radius = used width / (2 sin(pi/4)), corner k at
// angle -(a + k pi/2), x scaled by the screen's x factor and y by its y factor, corners to the quad in order)
pub fn view_turn(yaw: f32) -> f32 {
    yaw - WORLD_NORTH
}

/// How an entry is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlipKind {
    Mate,
}

/// One entry icon this frame: its top-left in virtual pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct RadarBlip {
    pub pid: u8,
    pub at: [f32; 2],
    pub kind: BlipKind,
}

/// The radar of one frame: the own camera point and view, and the entries shown.
#[derive(Debug, Clone, PartialEq)]
pub struct RadarFrame {
    pub own: [f32; 3],
    pub yaw: f32,
    pub blips: Vec<RadarBlip>,
}

/// The icon's top-left of a world point `p` seen from `own`, `None` beyond the layout radius.
///
/// 2009: radar distance `d` = horizontal distance x the character scale; the icon's place = origin + (sin t, -cos t)
/// x `d` x the blip scale with `t` the bearing from +Z toward +X, i.e. world +X right and +Z up; drawn while
/// `d` x the blip scale is at most the radius.
// DISASM: cshell+0xb6224..0xb6407 (offsets of both objects added alike, horizontal distance x [+0x3ac], bearing by
// fpatan, place = (place + origin) x screen factor + sin / -cos x d x [+0x60] x screen factor, truncated),
// cshell+0xb7566..0xb75c6 (the default kinds: shown while d x [+0x60] <= the radius [+0x64]; quad at the place - 0.5, size
// icon + 1)
pub fn blip_at(l: &RadarLayout, own: [f32; 3], p: [f32; 3]) -> Option<[f32; 2]> {
    let (dx, dz) = (p[0] - own[0], p[2] - own[2]);
    let d = (dx * dx + dz * dz).sqrt() * l.char_scale;
    if !d.is_finite() || d * l.blip_scale > l.radius {
        return None;
    }
    let k = l.char_scale * l.blip_scale;
    Some([
        l.place[0] + l.blip_origin[0] + dx * k,
        l.place[1] + l.blip_origin[1] - dz * k,
    ])
}

fn own_team(s: &HudState) -> Option<u8> {
    s.own_pid
        .and_then(|p| s.roster.get(&p))
        .map(|r| r.team)
        .filter(|t| matches!(t, 1 | 2))
}

/// The radar of a frame: `own` / `yaw` of the camera, the entries of `remotes` by the 2009 rule.
///
/// Rule (players, entry kind 0): never the own player; never a player the board lists as dead (0x6d until his next
/// spawn); a team mate (the own team) with the friendly icon when within the radius; an enemy only while the radar
/// accessory reveals him, which DeCombat never does (no 0xcd ping, no range): enemies are not drawn.
// DISASM: cshell+0xb6277..0xb6299 (an entry with its dead byte +1 set or the own client object is skipped),
// cshell+0xb75cc..0xb775e (own team -> friendly icon, shown by element +0x1a4 (1 from the ctor); team 0xff -> not
// shown; another team -> shown only within the accessory range +0x1b0 and with the reveal byte +0x118 or the scan
// flag +0x1b8); cshell+0x2ce0e (the roster adds every player with kind 0), cshell+0x2db01 (0x6d sets the dead byte)
// DC-DEFAULT: CLI-142 (proposed) no radar accessory: enemies are never drawn; the own point is the camera's
pub fn frame(
    l: &RadarLayout,
    s: &HudState,
    remotes: &[RemoteBody],
    own: [f32; 3],
    yaw: f32,
) -> RadarFrame {
    let team = own_team(s);
    let mut blips = Vec::new();
    for r in remotes {
        if Some(r.pid) == s.own_pid {
            continue;
        }
        let row = s.roster.get(&r.pid);
        if row.is_some_and(|b| b.dead) {
            continue;
        }
        let t = row.map(|b| b.team).unwrap_or(r.team);
        if team.is_none() || Some(t) != team {
            continue;
        }
        if let Some(at) = blip_at(l, own, r.pos) {
            blips.push(RadarBlip {
                pid: r.pid,
                at,
                kind: BlipKind::Mate,
            });
        }
    }
    RadarFrame { own, yaw, blips }
}

/// Name of the generated map-layer texture (not an archive path).
pub const MAP_LAYER: &str = "<radar map layer>";
/// Name of the generated view-disc texture.
pub const VIEW_LAYER: &str = "<radar view disc>";

fn texel(t: &Tex, x: i64, y: i64) -> [f32; 4] {
    let (w, h) = (t.width as i64, t.height as i64);
    let o = ((y.rem_euclid(h) * w + x.rem_euclid(w)) * 4) as usize;
    [
        t.rgba[o] as f32,
        t.rgba[o + 1] as f32,
        t.rgba[o + 2] as f32,
        t.rgba[o + 3] as f32,
    ]
}

/// Bilinear sample at texel coordinates `(u, v)` (texel centres at +0.5); `wrap` repeats the texture, else the edge
/// texels are clamped.
fn bilinear(t: &Tex, u: f32, v: f32, wrap: bool) -> [f32; 4] {
    if t.width == 0 || t.height == 0 {
        return [0.0; 4];
    }
    let (mut u, mut v) = (u - 0.5, v - 0.5);
    if !wrap {
        u = u.clamp(0.0, (t.width - 1) as f32);
        v = v.clamp(0.0, (t.height - 1) as f32);
    }
    let (x0, y0) = (u.floor(), v.floor());
    let (fx, fy) = (u - x0, v - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (lx, ly) = (t.width as i64 - 1, t.height as i64 - 1);
    let clampx = |x: i64| if wrap { x } else { x.clamp(0, lx) };
    let clampy = |y: i64| if wrap { y } else { y.clamp(0, ly) };
    let a = texel(t, clampx(x0), clampy(y0));
    let b = texel(t, clampx(x0 + 1), clampy(y0));
    let c = texel(t, clampx(x0), clampy(y0 + 1));
    let d = texel(t, clampx(x0 + 1), clampy(y0 + 1));
    let mut out = [0.0; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bot = c[i] + (d[i] - c[i]) * fx;
        out[i] = top + (bot - top) * fy;
    }
    out
}

/// Our generated disc (no ROM): alpha `alpha` inside the inscribed circle, 0 outside, a lighter rim.
// DC-DEFAULT: CLI-140 (proposed) the look of the no-ROM radar (dark teal disc, rim, view wedge, centre arrow)
fn generated_disc(w: u32, h: u32, view: bool) -> Tex {
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
    let r = cx.min(cy) - 1.0;
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let d = (dx * dx + dy * dy).sqrt();
            let o = ((y * w + x) * 4) as usize;
            if d > r {
                continue;
            }
            let mut px = if d > r - 2.0 {
                [120, 200, 200, 200]
            } else {
                [20, 60, 60, 110]
            };
            if view {
                // a lighter wedge of 90 degrees pointing up and a small arrow at the centre
                let up = -dy;
                if up > 0.0 && dx.abs() < up && d <= r - 2.0 {
                    px[3] = 70;
                }
                if up > -4.0 && up < 6.0 && dx.abs() <= (6.0 - up) * 0.6 {
                    px = [220, 255, 255, 255];
                }
            }
            rgba[o..o + 4].copy_from_slice(&px);
        }
    }
    Tex {
        width: w,
        height: h,
        rgba,
    }
}

/// The disc's alpha over the map layer at target resolution (`w` x `h` pixels), from the disc's used region.
pub fn disc_alpha(disc: &Tex, used: [f32; 2], w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let fx = (x as f32 + 0.5) / w as f32;
            let fy = (y as f32 + 0.5) / h as f32;
            let a = bilinear(disc, fx * used[0], fy * used[1], false)[3];
            out[(y * w + x) as usize] = (a + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// The map layer at target resolution (`w` x `h` pixels, `alpha` from [`disc_alpha`]): colour from the picture window,
/// alpha from the disc. The picture repeats outside 0..1 (the device's default addressing).
// EVIDENCE: engine+0x26630 case 4 (stage 0: colour = diffuse, alpha = texture; stage 1: colour = texture x current,
// alpha = current; stage 0 takes uv set 0, stage 1 uv set 1), cshell+0xb5fc6..0xb6021 (stage 0 = the disc texture
// [+0x180], stage 1 = the picture [+0x17c], white vertices), cshell+0xb4ee0 (the 0x45 picture replaces [+0x17c]);
// SPEC: RENDER_2009.md §2.4 (default addressing WRAP)
// DC-DEFAULT: CLI-143 (proposed) the picture repeats outside its uv range (stage 1 addressing not read: the device
// default); before a picture is known the default picture is shown whole
pub fn map_layer(alpha: &[u8], picture: Option<(&Tex, [f32; 4])>, w: u32, h: u32) -> Tex {
    // client-fps: the same per-pixel arithmetic as `bilinear(.., wrap = true)`, with the texel columns and rows (and
    // their wrapped indices, an integer remainder each) worked out once per column and once per row instead of
    // four 64-bit remainders per pixel (the layer is recomposed whenever the player moves)
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    // per column / row: wrapped texel indices (this, next) and the fraction, as `bilinear` computes them
    let axis = |n: u32, size: u32, start: f32, span: f32| -> Vec<(usize, usize, f32)> {
        (0..n)
            .map(|i| {
                let f = (i as f32 + 0.5) / n as f32;
                let t = (start + f * span) * size as f32 - 0.5;
                let t0 = t.floor();
                let frac = t - t0;
                let t0 = t0 as i64;
                let s = size as i64;
                (
                    t0.rem_euclid(s) as usize,
                    (t0 + 1).rem_euclid(s) as usize,
                    frac,
                )
            })
            .collect()
    };
    let pic = picture.filter(|(p, _)| p.width > 0 && p.height > 0);
    let (cols, rows) = match pic {
        Some((p, win)) => (
            axis(w, p.width, win[0], win[2]),
            axis(h, p.height, win[1], win[3]),
        ),
        None => (Vec::new(), Vec::new()),
    };
    for y in 0..h {
        for x in 0..w {
            let a = alpha.get((y * w + x) as usize).copied().unwrap_or(0);
            if a == 0 {
                continue;
            }
            let rgb = match (picture, pic) {
                (_, Some((p, _))) => {
                    let (x0, x1, fx) = cols[x as usize];
                    let (y0, y1, fy) = rows[y as usize];
                    let pw = p.width as usize;
                    let at = |tx: usize, ty: usize, i: usize| p.rgba[(ty * pw + tx) * 4 + i] as f32;
                    let mut out = [0.0f32; 3];
                    for (i, o) in out.iter_mut().enumerate() {
                        let (ta, tb) = (at(x0, y0, i), at(x1, y0, i));
                        let (tc, td) = (at(x0, y1, i), at(x1, y1, i));
                        let top = ta + (tb - ta) * fx;
                        let bot = tc + (td - tc) * fx;
                        *o = top + (bot - top) * fy;
                    }
                    out
                }
                // a picture without texels samples as black (as `bilinear` does)
                (Some(_), None) => [0.0; 3],
                (None, _) => [255.0; 3],
            };
            let o = ((y * w + x) * 4) as usize;
            for i in 0..3 {
                rgba[o + i] = (rgb[i] + 0.5).clamp(0.0, 255.0) as u8;
            }
            rgba[o + 3] = a;
        }
    }
    Tex {
        width: w,
        height: h,
        rgba,
    }
}

/// The view disc's used region mirrored left to right (the 2009 corner order, see [`view_turn`]).
pub fn view_layer(view: &Tex, used_texels: [u32; 2]) -> Tex {
    let (w, h) = (
        used_texels[0].min(view.width),
        used_texels[1].min(view.height),
    );
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let s = ((y * view.width + x) * 4) as usize;
            let d = ((y * w + (w - 1 - x)) * 4) as usize;
            rgba[d..d + 4].copy_from_slice(&view.rgba[s..s + 4]);
        }
    }
    Tex {
        width: w,
        height: h,
        rgba,
    }
}

/// The key of a composed map layer: the picture, its window and the layer's pixel size.
#[derive(Debug, Clone, PartialEq)]
struct MapKey {
    picture: Option<String>,
    window: [f32; 4],
    size: (u32, u32),
}

/// The disc alpha of one layer size: the size and one byte per pixel.
type DiscAlpha = ((u32, u32), Arc<Vec<u8>>);

/// What the radar keeps between frames so the layers are composed only when they change.
#[derive(Debug, Clone, Default)]
struct RadarCache {
    /// Disc alpha at the layer's pixel size (shared with the background composer).
    alpha: Option<DiscAlpha>,
    map: Option<MapKey>,
    /// The key last handed to the background composer and not yet composed.
    pending: Option<MapKey>,
    view: bool,
}

/// One map layer to compose off the frame thread.
struct MapJob {
    key: MapKey,
    alpha: Arc<Vec<u8>>,
    picture: Option<Arc<Tex>>,
}

/// The map-layer composer thread of a window loop (client-fps): the layer is recomposed whenever the player moves,
/// and at the window's pixel size that is a few milliseconds of per-pixel work per frame while walking. The frame
/// thread hands the composer the newest window and keeps drawing the last composed layer until the new one is
/// back (at most the time of one composition behind, a few milliseconds; the first layer of a picture is still
/// composed inline, so a frame never draws without one). The composer skips to the newest queued window.
// DC-DEFAULT: CLI-391 (proposed) the windowed client draws the radar map layer composed from the player position of
// a frame a few milliseconds earlier; headless runs and tests compose inline (identical pixels), see DC_DEFAULTS.md
#[derive(Debug)]
struct MapComposer {
    jobs: std::sync::mpsc::Sender<MapJob>,
    done: std::sync::Mutex<std::sync::mpsc::Receiver<(MapKey, Tex)>>,
}

impl MapComposer {
    /// Starts the composer thread; `None` when it cannot start (the layer is then composed inline).
    #[cfg(not(target_arch = "wasm32"))]
    fn start() -> Option<Arc<MapComposer>> {
        let (jobs, rx) = std::sync::mpsc::channel::<MapJob>();
        let (tx, done) = std::sync::mpsc::channel::<(MapKey, Tex)>();
        std::thread::Builder::new()
            .name("dc-radar-map".into())
            .spawn(move || {
                while let Ok(mut job) = rx.recv() {
                    // only the newest window matters
                    while let Ok(newer) = rx.try_recv() {
                        job = newer;
                    }
                    let (w, h) = job.key.size;
                    let layer = map_layer(
                        &job.alpha,
                        job.picture.as_deref().map(|t| (t, job.key.window)),
                        w,
                        h,
                    );
                    if tx.send((job.key, layer)).is_err() {
                        return;
                    }
                }
            })
            .ok()?;
        Some(Arc::new(MapComposer {
            jobs,
            done: std::sync::Mutex::new(done),
        }))
    }

    #[cfg(target_arch = "wasm32")]
    fn start() -> Option<Arc<MapComposer>> {
        None
    }

    /// The newest composed layer, when one is back.
    fn take(&self) -> Option<(MapKey, Tex)> {
        let rx = self.done.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = None;
        while let Ok(r) = rx.try_recv() {
            out = Some(r);
        }
        out
    }
}

/// The radar of a HUD: its layout (the surface a mod may replace) and the composed layers.
#[derive(Debug, Clone)]
pub struct Radar {
    pub layout: RadarLayout,
    cache: RadarCache,
    composer: Option<Arc<MapComposer>>,
}

impl Radar {
    pub fn new(layout: RadarLayout) -> Self {
        Radar {
            layout,
            cache: RadarCache::default(),
            composer: None,
        }
    }

    /// Composes the map layer on a thread of its own from now on (the window loop's choice, see [`MapComposer`]);
    /// `true` when the thread started.
    pub fn compose_in_background(&mut self) -> bool {
        if self.composer.is_none() {
            self.composer = MapComposer::start();
        }
        self.composer.is_some()
    }

    /// The display-list items of the radar (rim, map layer, view disc, entries), drawn first; the two layer
    /// textures are composed here when they change and stored under [`MAP_LAYER`] / [`VIEW_LAYER`]. Nothing before
    /// the frame loop gave a [`RadarFrame`].
    pub fn push_items(
        &mut self,
        out: &mut Vec<DrawItem>,
        s: &HudState,
        assets: &mut HudAssets,
        target: (u32, u32),
    ) {
        let Some(f) = &s.radar else {
            return;
        };
        // the radar is a layer-3 element: drawn only while the life state's render level reaches 3 (alive)
        // DISASM: cshell+0xb9164 (the radar's constructor stores layer 3 at +0x08, like the aim cross / health / ammunition
        // of `super::death::LIVE_LAYER`), cshell+0x26b0d0..0x26b108 (the HUD manager's draw loop gates every element by the
        // life state's render level, `super::death::hud_level`)
        if !super::death::live_widgets(s) {
            return;
        }
        let l = &self.layout;
        let sc = VirtualScaler::new(target.0, target.1);
        let (w, h) = (
            (l.size[0] * sc.sx).ceil().max(1.0) as u32,
            (l.size[1] * sc.sy).ceil().max(1.0) as u32,
        );
        let mut items = Vec::new();
        // ---- rim (drawn before the layers)
        if let Some((tex, size)) = &l.rim {
            items.push(DrawItem::Image {
                tex: tex.clone(),
                src: *size,
                dst: [l.place[0], l.place[1], size[0], size[1]],
                tint: [255; 4],
                turn: None,
            });
        }
        // ---- map layer: the disc alpha once per size, the picture window when it moves
        if self.cache.alpha.as_ref().is_none_or(|a| a.0 != (w, h)) {
            let disc = l.disc.as_deref().and_then(|t| assets.texture(t));
            let disc = disc.unwrap_or_else(|| {
                let (dw, dh) = (l.disc_texels[0] as u32, l.disc_texels[1] as u32);
                Arc::new(generated_disc(dw.max(1), dh.max(1), false))
            });
            let used = [
                l.size[0] / l.disc_texels[0].max(1.0) * disc.width as f32,
                l.size[1] / l.disc_texels[1].max(1.0) * disc.height as f32,
            ];
            self.cache.alpha = Some(((w, h), Arc::new(disc_alpha(&disc, used, w, h))));
            self.cache.map = None;
            self.cache.pending = None;
        }
        let map = s.intro_block.as_ref().and_then(RadarMap::of);
        let picture: Option<(String, [f32; 4])> = match &map {
            Some(m) => {
                let name = if assets.texture(&m.picture).is_some() {
                    Some(m.picture.clone())
                } else {
                    l.default_map.clone()
                };
                name.map(|n| (n, m.window(f.own)))
            }
            None => l.default_map.clone().map(|n| (n, [0.0, 0.0, 1.0, 1.0])),
        };
        let key = MapKey {
            picture: picture.as_ref().map(|p| p.0.clone()),
            window: picture.as_ref().map_or([0.0; 4], |p| p.1),
            size: (w, h),
        };
        // a layer back from the background composer is the newest one there is
        if let Some((k, layer)) = self.composer.as_ref().and_then(|c| c.take()) {
            if self.cache.pending.as_ref() == Some(&k) {
                self.cache.pending = None;
            }
            assets.put(MAP_LAYER, layer);
            self.cache.map = Some(k);
        }
        if self.cache.map.as_ref() != Some(&key) || assets.generated(MAP_LAYER).is_none() {
            let tex = picture.as_ref().and_then(|p| assets.texture(&p.0));
            let alpha = self.cache.alpha.clone().map(|a| a.1).unwrap_or_default();
            let win = picture.as_ref().map_or([0.0; 4], |p| p.1);
            let background = self
                .composer
                .as_ref()
                .filter(|_| assets.generated(MAP_LAYER).is_some());
            match background {
                // already on its way
                Some(_) if self.cache.pending.as_ref() == Some(&key) => {}
                Some(c) => {
                    let job = MapJob {
                        key: key.clone(),
                        alpha,
                        picture: tex,
                    };
                    if c.jobs.send(job).is_ok() {
                        self.cache.pending = Some(key);
                    }
                }
                None => {
                    let layer = map_layer(&alpha, tex.as_deref().map(|t| (t, win)), w, h);
                    assets.put(MAP_LAYER, layer);
                    self.cache.map = Some(key);
                    self.cache.pending = None;
                }
            }
        }
        items.push(DrawItem::Image {
            tex: MAP_LAYER.into(),
            src: [w as f32, h as f32],
            dst: [l.place[0], l.place[1], l.size[0], l.size[1]],
            tint: [255; 4],
            turn: None,
        });
        // ---- view disc: a square of the used width about the centre, turned with the view
        // DC-DEFAULT: CLI-145 (proposed, amended) the compositor turns the square in virtual units and scales the
        // turned corners per axis, as 2009 does: the disc stretches exactly like the ring image around it
        if !self.cache.view || assets.generated(VIEW_LAYER).is_none() {
            let view = match l.view.as_deref().and_then(|t| assets.texture(t)) {
                Some(t) => {
                    let uw = (l.size[0] / l.disc_texels[0].max(1.0) * t.width as f32).round();
                    let uh = (l.size[1] / l.disc_texels[1].max(1.0) * t.height as f32).round();
                    view_layer(&t, [(uw as u32).max(1), (uh as u32).max(1)])
                }
                None => generated_disc((l.size[0] as u32).max(1), (l.size[1] as u32).max(1), true),
            };
            assets.put(VIEW_LAYER, view);
            self.cache.view = true;
        }
        let src = assets
            .generated(VIEW_LAYER)
            .map_or([1.0, 1.0], |t| [t.width as f32, t.height as f32]);
        let c = l.centre();
        let side = l.size[0];
        items.push(DrawItem::Image {
            tex: VIEW_LAYER.into(),
            src,
            dst: [c[0] - side * 0.5, c[1] - side * 0.5, side, side],
            tint: [255; 4],
            turn: Some((c, view_turn(f.yaw))),
        });
        // ---- entries: the whole icon texture stretched over the icon size
        // EVIDENCE: cshell+0xb8919..0xb8936 (entry quad: white, uv (0, 0, 1, 1))
        for b in &f.blips {
            match b.kind {
                BlipKind::Mate => match l.mate_icon.as_deref().and_then(|t| assets.texture(t)) {
                    Some(t) => items.push(DrawItem::Image {
                        tex: l.mate_icon.clone().unwrap_or_default(),
                        src: [t.width as f32, t.height as f32],
                        dst: [b.at[0], b.at[1], l.icon[0], l.icon[1]],
                        tint: [255; 4],
                        turn: None,
                    }),
                    None => items.push(DrawItem::Fill {
                        rect: [b.at[0], b.at[1], l.icon[0], l.icon[1]],
                        colour: l.mate_colour,
                    }),
                },
            }
        }
        out.splice(0..0, items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::state::BoardRow;

    fn layout() -> RadarLayout {
        RadarLayout {
            rim: None,
            place: [800.0, 20.0],
            size: [160.0, 160.0],
            disc_texels: [256.0, 256.0],
            disc: None,
            view: None,
            default_map: None,
            mate_icon: None,
            mate_colour: FALLBACK_MATE,
            blip_origin: [70.0, 70.0],
            icon: [20.0, 20.0],
            radius: 60.0,
            char_scale: 2.0,
            blip_scale: 0.025,
        }
    }

    fn map() -> RadarMap {
        RadarMap {
            picture: "pic".into(),
            scale: [0.25, 0.2],
            origin: [0.5, 0.4],
            half: 0.3,
            image: [1000.0, 800.0],
        }
    }

    #[test]
    fn map_window_is_centred_on_the_own_point_north_up() {
        let m = map();
        // at the world origin the window is centred on the second vector
        let w = m.window([0.0, 0.0, 0.0]);
        assert_eq!(w, [0.5 - 0.3, 0.4 - 0.3, 0.6, 0.6]);
        // +X moves the window right by x * scale / width, +Z moves it up (smaller v), y is ignored
        let w2 = m.window([400.0, 999.0, 400.0]);
        assert!((w2[0] - (w[0] + 400.0 * 0.25 / 1000.0)).abs() < 1e-6);
        assert!((w2[1] - (w[1] - 400.0 * 0.2 / 800.0)).abs() < 1e-6);
        assert_eq!((w2[2], w2[3]), (0.6, 0.6));
    }

    #[test]
    fn intro_block_fields_map_to_the_2009_slots() {
        let b = IntroBlock {
            radar: "pic.dtx".into(),
            radar_map_scale: [0.25, 7.0, 0.2],
            radar_map_offset: [0.5, 9.0, 0.4],
            radar_zoom: 1.5,
            radar_half: 0.3,
            radar_w: 1000,
            radar_h: 800,
            ..IntroBlock::default()
        };
        let m = RadarMap::of(&b).unwrap();
        assert_eq!(m.scale, [0.25, 0.2]);
        assert_eq!(m.origin, [0.5, 0.4]);
        assert_eq!(m.half, 0.3);
        assert_eq!(m.image, [1000.0, 800.0]);
        assert!(RadarMap::of(&IntroBlock {
            radar_w: 0,
            ..b.clone()
        })
        .is_none());
        assert!(RadarMap::of(&IntroBlock {
            radar: String::new(),
            ..b
        })
        .is_none());
    }

    #[test]
    fn blips_sit_north_up_and_stop_at_the_radius() {
        let l = layout();
        let own = [100.0, 0.0, -50.0];
        let origin = [870.0, 90.0];
        assert_eq!(blip_at(&l, own, own), Some(origin));
        // 200 units east: right by 200 x 2 x 0.025 = 10
        assert_eq!(blip_at(&l, own, [300.0, 50.0, -50.0]), Some([880.0, 90.0]));
        // 200 units north (+Z): up by 10
        assert_eq!(blip_at(&l, own, [100.0, 0.0, 150.0]), Some([870.0, 80.0]));
        // radius 60 = 1200 world units: on it shown, past it not
        assert!(blip_at(&l, own, [100.0, 0.0, 1150.0]).is_some());
        assert!(blip_at(&l, own, [100.0, 0.0, 1151.0]).is_none());
    }

    #[test]
    fn map_and_blips_agree_when_the_scales_match() {
        // the ROM pairs the two scales so that a blip lies on its point of the picture: virtual pixels per world unit
        // of the map layer = scale / width / (2 half) x drawn size; of the entries = char_scale x blip_scale
        let m = map();
        let mut l = layout();
        l.blip_origin = [
            l.size[0] * 0.5 - l.icon[0] * 0.5,
            l.size[1] * 0.5 - l.icon[1] * 0.5,
        ];
        l.blip_scale = m.scale[0] / m.image[0] / (2.0 * m.half) * l.size[0] / l.char_scale;
        let own = [40.0, 0.0, 0.0];
        let p = [340.0, 0.0, 0.0];
        let b = blip_at(&l, own, p).unwrap();
        // where the point is drawn on the map layer
        let win = m.window(own);
        let x_on_layer = l.place[0] + (m.uv(p)[0] - win[0]) / win[2] * l.size[0];
        assert!((b[0] + l.icon[0] * 0.5 - x_on_layer).abs() < 1e-3);
    }

    #[test]
    fn view_disc_turns_clockwise_with_the_yaw() {
        assert_eq!(view_turn(0.0), 0.0);
        assert!(
            (view_turn(std::f32::consts::FRAC_PI_2) - std::f32::consts::FRAC_PI_2).abs() < 1e-6
        );
    }

    fn remote(pid: u8, team: u8, pos: [f32; 3]) -> RemoteBody {
        RemoteBody {
            obj: 100 + pid as u16,
            pid,
            name: format!("p{pid}"),
            team,
            pos,
            half: [16.0, 50.0, 16.0],
            yaw: None,
            look: None,
        }
    }

    fn row(pid: u8, team: u8, dead: bool) -> BoardRow {
        BoardRow {
            pid,
            name: format!("p{pid}"),
            team,
            kills: 0,
            deaths: 0,
            dead,
            ping: None,
        }
    }

    #[test]
    fn only_living_team_mates_are_drawn() {
        let l = layout();
        let mut s = HudState {
            own_pid: Some(0),
            ..HudState::default()
        };
        for r in [
            row(0, 1, false),
            row(1, 1, false),
            row(2, 2, false),
            row(3, 1, true),
            row(4, 1, false),
        ] {
            s.roster.insert(r.pid, r);
        }
        let remotes = [
            remote(0, 1, [0.0; 3]),
            remote(1, 1, [100.0, 0.0, 0.0]),
            remote(2, 2, [0.0, 0.0, 100.0]),
            remote(3, 1, [-100.0, 0.0, 0.0]),
            remote(4, 1, [0.0, 0.0, 5000.0]),
        ];
        let f = frame(&l, &s, &remotes, [0.0; 3], 0.3);
        let pids: Vec<u8> = f.blips.iter().map(|b| b.pid).collect();
        // own (0) never, enemy (2) not revealed, dead mate (3) skipped, far mate (4) past the radius
        assert_eq!(pids, vec![1]);
        assert_eq!(f.yaw, 0.3);
        // without a known own team nobody is a mate
        s.own_pid = Some(9);
        assert!(frame(&l, &s, &remotes, [0.0; 3], 0.0).blips.is_empty());
    }

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Tex {
        Tex {
            width: w,
            height: h,
            rgba: px.repeat((w * h) as usize),
        }
    }

    #[test]
    fn map_layer_takes_colour_from_the_picture_and_alpha_from_the_disc() {
        let disc = disc_alpha(&solid(4, 4, [10, 20, 30, 100]), [4.0, 4.0], 8, 8);
        // picture: left half red, right half blue (alpha 7, not used)
        let mut pic = solid(8, 1, [200, 0, 0, 7]);
        for t in 4..8 {
            pic.rgba[t * 4..t * 4 + 4].copy_from_slice(&[0, 0, 200, 7]);
        }
        let px = |t: &Tex, x: u32| t.rgba[(x * 4) as usize..(x * 4 + 4) as usize].to_vec();
        // window over the left half: red, the disc's alpha
        let t = map_layer(&disc, Some((&pic, [0.0, 0.0, 0.5, 1.0])), 8, 8);
        assert_eq!(px(&t, 3), vec![200, 0, 0, 100]);
        // a window past the right edge repeats the picture (the left half again at u 1.0 .. 1.5)
        let t = map_layer(&disc, Some((&pic, [1.0, 0.0, 0.5, 1.0])), 8, 8);
        assert_eq!(px(&t, 3), vec![200, 0, 0, 100]);
        let t = map_layer(&disc, Some((&pic, [0.5, 0.0, 0.5, 1.0])), 8, 8);
        assert_eq!(px(&t, 3), vec![0, 0, 200, 100]);
        // no picture: white with the disc's alpha
        let alpha = disc_alpha(&solid(4, 4, [10, 20, 30, 100]), [4.0, 4.0], 2, 2);
        let t = map_layer(&alpha, None, 2, 2);
        assert_eq!(&t.rgba[0..4], &[255, 255, 255, 100]);
    }

    /// The per-pixel map layer as it was written before client-fps (the equality reference).
    fn map_layer_reference(alpha: &[u8], picture: Option<(&Tex, [f32; 4])>, w: u32, h: u32) -> Tex {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let a = alpha.get((y * w + x) as usize).copied().unwrap_or(0);
                if a == 0 {
                    continue;
                }
                let fx = (x as f32 + 0.5) / w as f32;
                let fy = (y as f32 + 0.5) / h as f32;
                let rgb = match picture {
                    Some((p, win)) => {
                        let u = (win[0] + fx * win[2]) * p.width as f32;
                        let v = (win[1] + fy * win[3]) * p.height as f32;
                        let s = bilinear(p, u, v, true);
                        [s[0], s[1], s[2]]
                    }
                    None => [255.0; 3],
                };
                let o = ((y * w + x) * 4) as usize;
                for i in 0..3 {
                    rgba[o + i] = (rgb[i] + 0.5).clamp(0.0, 255.0) as u8;
                }
                rgba[o + 3] = a;
            }
        }
        Tex {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn map_layer_equals_the_per_pixel_reference() {
        // a picture with distinct texels, windows inside, across the edges (wrap) and far outside
        let (pw, ph) = (37u32, 23u32);
        let mut pic = solid(pw, ph, [0, 0, 0, 255]);
        for (i, px) in pic.rgba.chunks_mut(4).enumerate() {
            let i = i as u32;
            px.copy_from_slice(&[
                (i * 7 % 256) as u8,
                (i * 13 % 256) as u8,
                (i * 29 % 256) as u8,
                255,
            ]);
        }
        let disc = disc_alpha(&solid(16, 16, [0, 0, 0, 200]), [16.0, 16.0], 61, 47);
        for win in [
            [0.0, 0.0, 1.0, 1.0],
            [0.31, 0.47, 0.25, 0.25],
            [-0.6, 0.8, 0.5, 0.7],
            [3.3, -7.9, 0.125, 0.33],
            [-1234.5, 987.25, 0.01, 2.5],
        ] {
            let a = map_layer(&disc, Some((&pic, win)), 61, 47);
            let b = map_layer_reference(&disc, Some((&pic, win)), 61, 47);
            assert_eq!(a.rgba, b.rgba, "window {win:?}");
        }
        assert_eq!(
            map_layer(&disc, None, 61, 47).rgba,
            map_layer_reference(&disc, None, 61, 47).rgba
        );
        let empty = Tex {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
        assert_eq!(
            map_layer(&disc, Some((&empty, [0.0, 0.0, 1.0, 1.0])), 61, 47).rgba,
            map_layer_reference(&disc, Some((&empty, [0.0, 0.0, 1.0, 1.0])), 61, 47).rgba
        );
    }

    #[test]
    fn view_layer_is_the_used_region_mirrored() {
        let mut v = solid(4, 2, [0, 0, 0, 0]);
        v.rgba[0..4].copy_from_slice(&[1, 2, 3, 4]); // texel (0, 0)
        let m = view_layer(&v, [3, 2]);
        assert_eq!((m.width, m.height), (3, 2));
        assert_eq!(&m.rgba[8..12], &[1, 2, 3, 4]); // now at (2, 0)
    }

    #[test]
    fn background_composer_hands_back_the_inline_layer() {
        let l = RadarLayout::fallback();
        let mut radar = Radar::new(l.clone());
        if !radar.compose_in_background() {
            return; // no thread on this target: the inline path is what the other tests cover
        }
        let mut assets = HudAssets::new(
            Box::new(super::super::NoTextures),
            super::super::HudFonts::new(None, 18.0, None, 1.0),
        );
        let frame = |own: [f32; 3]| RadarFrame {
            own,
            yaw: 0.0,
            blips: Vec::new(),
        };
        // a picture with distinct texels (put into the cache: the asset loader of the test has none) and an intro
        // block whose window follows the own position
        let mut pic = solid(37, 23, [0, 0, 0, 255]);
        for (i, px) in pic.rgba.chunks_mut(4).enumerate() {
            let i = i as u32;
            px.copy_from_slice(&[
                (i * 7 % 256) as u8,
                (i * 13 % 256) as u8,
                (i * 29 % 256) as u8,
                255,
            ]);
        }
        assets.put("pic.dtx", pic);
        let mut s = HudState {
            life_state: 1,
            radar: Some(frame([0.0; 3])),
            intro_block: Some(IntroBlock {
                radar: "pic.dtx".into(),
                radar_map_scale: [0.25, 0.0, 0.2],
                radar_map_offset: [0.5, 0.0, 0.4],
                radar_half: 0.3,
                radar_w: 1000,
                radar_h: 800,
                ..IntroBlock::default()
            }),
            ..HudState::default()
        };
        let mut out = Vec::new();
        // the first layer is composed inline
        radar.push_items(&mut out, &s, &mut assets, (1024, 768));
        let first = assets.generated(MAP_LAYER).expect("first layer inline");
        assert!(radar.cache.pending.is_none());
        // a moved window goes to the composer; the frame keeps the first layer until the new one is back
        s.radar = Some(frame([500.0, 0.0, 300.0]));
        let t0 = std::time::Instant::now();
        let mut key = None;
        loop {
            out.clear();
            radar.push_items(&mut out, &s, &mut assets, (1024, 768));
            if key.is_none() {
                key = radar.cache.pending.clone();
            }
            if radar.cache.pending.is_none() && radar.cache.map == key && key.is_some() {
                break;
            }
            assert!(t0.elapsed().as_secs() < 10, "composer never answered");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let moved = assets.generated(MAP_LAYER).expect("layer");
        // the composer's layer is the bytes of the inline composition of the same frame
        let mut inline = Radar::new(l.clone());
        let mut a2 = HudAssets::new(
            Box::new(super::super::NoTextures),
            super::super::HudFonts::new(None, 18.0, None, 1.0),
        );
        a2.put("pic.dtx", (*assets.texture("pic.dtx").unwrap()).clone());
        inline.push_items(&mut Vec::new(), &s, &mut a2, (1024, 768));
        assert_eq!(
            a2.generated(MAP_LAYER).expect("inline").rgba,
            moved.rgba,
            "background layer differs from the inline one"
        );
        assert_eq!(first.rgba.len(), moved.rgba.len());
        assert_ne!(first.rgba, moved.rgba, "the window moved");
    }

    #[test]
    fn items_draw_the_radar_under_everything_when_a_frame_is_set() {
        let l = RadarLayout::fallback();
        let mut radar = Radar::new(l.clone());
        let mut assets = HudAssets::new(
            Box::new(super::super::NoTextures),
            super::super::HudFonts::new(None, 18.0, None, 1.0),
        );
        let mut s = HudState {
            life_state: 1,
            ..HudState::default()
        };
        let mut out = vec![DrawItem::Fill {
            rect: [0.0; 4],
            colour: [0; 4],
        }];
        radar.push_items(&mut out, &s, &mut assets, (1024, 768));
        assert_eq!(out.len(), 1, "no frame, no radar");
        s.radar = Some(RadarFrame {
            own: [0.0; 3],
            yaw: 0.0,
            blips: vec![RadarBlip {
                pid: 1,
                at: [900.0, 90.0],
                kind: BlipKind::Mate,
            }],
        });
        radar.push_items(&mut out, &s, &mut assets, (1024, 768));
        assert_eq!(out.len(), 4);
        assert!(matches!(&out[0], DrawItem::Image { tex, .. } if tex == MAP_LAYER));
        assert!(matches!(&out[1], DrawItem::Image { tex, turn: Some(_), .. } if tex == VIEW_LAYER));
        assert!(matches!(&out[2], DrawItem::Fill { colour, .. } if *colour == FALLBACK_MATE));
        let img = super::super::compose(&out, &mut assets, 1024, 768);
        let alpha = |x: u32, y: u32| img.rgba[((y * 1024 + x) * 4 + 3) as usize];
        let c = l.centre();
        assert!(alpha(c[0] as u32 + 20, c[1] as u32 + 20) > 0, "disc drawn");
        assert_eq!(
            alpha(l.place[0] as u32 + 1, l.place[1] as u32 + 1),
            0,
            "corner outside the disc"
        );
        assert_eq!(
            &img.rgba[((94 * 1024 + 904) * 4) as usize..][..3],
            &FALLBACK_MATE[..3]
        );
        // dead (render level 2): the layer-3 radar is not drawn
        s.life_state = 2;
        let mut dead = Vec::new();
        radar.push_items(&mut dead, &s, &mut assets, (1024, 768));
        assert!(dead.is_empty());
    }
}
