//! The `render.entity_visibility` hook bridge (CLI-1620): a declared x-ray server's mod decides which players are
//! shown through the world and how they are marked.
//!
//! Only a mod of the server's set can reach this hook: the host refuses `hooks.render` for a local mod at load
//! (`dc_mod_lua::manifest::CLIENT_VIEW_CAPABILITIES`), and the bridge is built only when the loaded set declares
//! both `hooks.render` and `world.full_visibility` (MOD-22: the server then sends every position, so the client
//! shows nothing the server did not choose to share with everyone).
//!
//! [`REFRESH_MS`] apart (CLI-1635; at most one Lua call, none while no mod registered the hook, sooner when the number
//! of players changes) the context lists the players this client knows:
//! `ctx.entities = { {id, kind = "player", team, is_enemy, distance, is_local, is_visible_by_line_of_sight} }`; between
//! two calls the frames draw the last answer at the players' current places.
//! The answer `{ entities = { {id, visible, outline, mode} } }` is drawn in the player's x-ray style
//! (CLI-1624, `settings::XrayStyle`; `mode = "box" | "chams" | "skeleton"` in an entry pins the style for that player).
//! The colours are not the mod's (CLI-1635): an enemy of the viewer is drawn in the player's two picks, a team mate in
//! [`mate_colours`], or not at all with `settings::XrayStyle::teammates` off:
//! - box: screen boxes over the world and under the HUD: `visible = true` on a player the world hides draws a box
//!   filled with the primary colour (alpha / 3); `outline = true` draws its border in the secondary colour whether the
//!   player is hidden or not;
//! - chams: `visible = true` on a hidden player draws its model flat in the two colours through the walls and, over its normal draw, when it is in sight
//!   (user 2026-11-01: the flat colours never switch off);
//! - skeleton: `visible = true` on a hidden player, or `outline = true`, draws its node lines through the walls.
//!
//! Chams and skeleton are [`XrayDraw`]s the frame hands to the renderer (`dc_render::WorldRenderer::encode_xray`). A
//! hook cannot hide a player the world shows. The first error switches the hook off for the rest of the match,
//! logged once.
//!
//! CLI-1631 (amended 2026-11-01): with the player's switches on (`settings::XrayStyle::nametags` / `health`,
//! CLI-1630) every enemy the hook marks `visible` gets the 2009 enemy name tag, as if the aim cross were on him
//! (`marks::frame_marks_forced`: the same caption, point over the head, colour and hit flash; team mates keep the
//! 2009 mate rule), and every other player marked `visible` a [`BAR_W`] x [`BAR_H`] health bar above his tag (above
//! the head without one; [`push_info`]: the primary colour over a dark back, its share = health / maximum; the
//! outline alone while this client does not know that player's health). A player who is not alive (the board's
//! dead flag or a DSV-480 health of 0) or who left gets neither, at once; both come back with the respawn. The
//! health comes from the game server's DSV-480 message, which only a match without culling (a room with wallhacks)
//! sends. Without the bridge (no declared x-ray server, or a "No-recoil only" room, CLI-1627) nothing of it is drawn.
//!
//! Provenance: ORIGINAL (DeCombat's own mod API; the 2009 client has no render mods).
// DC-DEFAULT: CLI-1620 (proposed) the render.entity_visibility context and effects, server-declared mods only
// DC-DEFAULT: CLI-1631 (proposed, USER REQUEST 2026-11-01) the x-ray name tags and health bars

use std::cell::RefCell;
use std::rc::Rc;

use dc_mod_lua::{LuaTable, LuaValue, ModHost};

use super::items::DrawItem;
use super::layout::TextStyle;
use crate::settings::{XrayMode, XrayStyle};

/// The hook name the bridge runs.
pub const ENTITY_VISIBILITY_HOOK: &str = "render.entity_visibility";

/// The capabilities the loaded set must declare for the bridge to exist.
pub const NEEDED_CAPABILITIES: [&str; 2] = ["hooks.render", "world.full_visibility"];

/// Width of an outline bar (virtual pixels).
const OUTLINE_PX: f32 = dc_render::BOX_BORDER_PX;

/// A silhouette's fill alpha is the primary colour's alpha divided by this (the body box stays see-through).
const FILL_ALPHA_DIV: u32 = dc_render::BOX_FILL_ALPHA_DIV;

/// One player of the frame, as the hook sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityCtx {
    /// The player's world object.
    pub id: u16,
    pub team: u8,
    /// CLI-1635: an enemy of the viewer (`marks::is_mate` is false): everybody in a free-for-all, a player of the other
    /// team in a team match.
    pub is_enemy: bool,
    /// From the camera to the body centre (world units).
    pub distance: f32,
    pub is_local: bool,
    /// No world geometry between the camera and the body centre.
    pub los: bool,
}

/// What the hook asked for one player.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EntityLook {
    pub id: u16,
    pub visible: bool,
    pub outline: bool,
    /// CLI-1620 (amended): the style the mod pins for this player (`None`: the player's setting).
    pub mode: Option<XrayMode>,
}

/// CLI-1624: one player the renderer draws in an x-ray style this frame (chams or skeleton; boxes are HUD items).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XrayDraw {
    /// The player's world object.
    pub entity: u16,
    pub mode: XrayMode,
    pub primary: [u8; 4],
    pub secondary: [u8; 4],
}

/// CLI-1631: a health bar's size and the space between the head, the bar and the name (virtual pixels).
pub const BAR_W: f32 = 40.0;
pub const BAR_H: f32 = 4.0;
pub const INFO_GAP: f32 = 2.0;
/// CLI-1631: the bar's dark back.
pub const BAR_BACK: [u8; 4] = [0, 0, 0, 0xa0];

/// CLI-1631: what this client knows of one player for its health bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerInfo {
    /// `(health, maximum)` from the server's DSV-480 message (`None`: not known).
    pub health: Option<(u16, u16)>,
    /// The point of the player's 2009 name tag this frame (the caption's bottom centre; `None`: no tag shown).
    pub tag_at: Option<[f32; 2]>,
    /// Alive as far as this client knows (`marks::not_alive`): a player who is not gets no bar.
    pub alive: bool,
}

/// The context table of one frame.
pub fn context(ents: &[EntityCtx]) -> LuaTable {
    let mut list = LuaTable::new();
    for (i, e) in ents.iter().enumerate() {
        let mut t = LuaTable::new();
        t.set_str("id", LuaValue::Number(e.id as f64));
        t.set_str("kind", LuaValue::String("player".into()));
        t.set_str("team", LuaValue::Number(e.team as f64));
        t.set_str("is_enemy", LuaValue::Bool(e.is_enemy));
        t.set_str("distance", LuaValue::Number(e.distance as f64));
        t.set_str("is_local", LuaValue::Bool(e.is_local));
        t.set_str("is_visible_by_line_of_sight", LuaValue::Bool(e.los));
        list.set_int(i as i64 + 1, LuaValue::Table(Rc::new(RefCell::new(t))));
    }
    let mut ctx = LuaTable::new();
    ctx.set_str("entities", LuaValue::Table(Rc::new(RefCell::new(list))));
    ctx
}

/// CLI-1635: the x-ray refreshes the line of sight and the hook's answer this far apart (session milliseconds).
// DC-DEFAULT: CLI-1635 (proposed, USER REPORT 2026-11-01) about 30 times a second
pub const REFRESH_MS: u32 = 33;

/// CLI-1635: a palette colour closer than this (RGB distance) to a pick is not a team mate's colour.
const MATE_MIN_DISTANCE: i32 = 110;
/// CLI-1635: a palette colour with less than this between its strongest and weakest channel (black, greys, white) reads
/// badly as a mark and is left out of the team mates' colours.
const MATE_MIN_CHROMA: i32 = 64;

fn colour_distance2(a: [u8; 4], b: [u8; 4]) -> i32 {
    (0..3).map(|i| (a[i] as i32 - b[i] as i32).pow(2)).sum()
}

/// CLI-1635: the team mates' two colours: chosen by `seed` from the shop's palette, not close to either of the player's
/// picks nor to each other (when the palette leaves no such colour, the ones farthest from the picks).
// DC-DEFAULT: CLI-1635 (proposed, USER REQUEST 2026-11-01 "what you pick adhere to the enemies and your team is random
// other colors")
pub fn mate_colours(style: &XrayStyle, seed: u32) -> [[u8; 4]; 2] {
    let palette = crate::menu::shopxray::PALETTE;
    let close = MATE_MIN_DISTANCE * MATE_MIN_DISTANCE;
    let to_picks =
        |c: [u8; 4]| colour_distance2(c, style.primary).min(colour_distance2(c, style.secondary));
    let chroma = |c: [u8; 4]| {
        let (a, b) = (c[..3].iter().max(), c[..3].iter().min());
        a.zip(b).map_or(0, |(a, b)| *a as i32 - *b as i32)
    };
    let mut pool: Vec<[u8; 4]> = palette
        .into_iter()
        .filter(|&c| chroma(c) >= MATE_MIN_CHROMA && to_picks(c) >= close)
        .collect();
    if pool.len() < 2 {
        pool = palette.to_vec();
        pool.sort_by_key(|&c| std::cmp::Reverse(to_picks(c)));
        pool.truncate(2);
    }
    let first = pool[seed as usize % pool.len()];
    let far: Vec<[u8; 4]> = pool
        .iter()
        .copied()
        .filter(|&c| colour_distance2(c, first) >= close)
        .collect();
    let rest = if far.is_empty() {
        pool.into_iter().filter(|&c| c != first).collect()
    } else {
        far
    };
    [first, rest[(seed >> 8) as usize % rest.len()]]
}

/// CLI-1635: this run's seed of [`mate_colours`]: one value for the whole session, so the team mates' colours stay put
/// (also across a device loss that rebuilds the HUD).
pub fn session_seed() -> u32 {
    static SEED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *SEED.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        nanos ^ std::process::id().rotate_left(16)
    })
}

/// The hook's answer as looks (ids the frame does not hold and malformed entries are dropped; a later entry for an
/// id replaces an earlier one).
pub fn parse_looks(v: &LuaValue, ents: &[EntityCtx], out: &mut Vec<EntityLook>) {
    out.clear();
    let Some(t) = v.as_table() else { return };
    let list = t.borrow().get_str("entities");
    let Some(list) = list.as_table() else { return };
    for (_, e) in list.borrow().entries() {
        let Some(e) = e.as_table() else { continue };
        let e = e.borrow();
        let Some(id) = e
            .get_str("id")
            .as_f64()
            .filter(|x| x.fract() == 0.0 && (0.0..=65535.0).contains(x))
        else {
            continue;
        };
        let id = id as u16;
        if !ents.iter().any(|x| x.id == id) {
            continue;
        }
        let look = EntityLook {
            id,
            visible: e.get_str("visible").as_bool().unwrap_or(false),
            outline: e.get_str("outline").as_bool().unwrap_or(false),
            mode: e.get_str("mode").as_str().and_then(XrayMode::parse),
        };
        out.retain(|l| l.id != id);
        out.push(look);
    }
}

/// The x-ray of `looks` in `style` (a look's `mode` pins the mode; an enemy takes the style's colours, a team mate
/// `mates`, and nothing at all with the style's `teammates` off): the box mode's screen boxes as HUD items (virtual 1024 x 768; `rect_of(id)` gives a player's projected body rectangle,
/// `None`: off screen or unknown) and the chams / skeleton players as [`XrayDraw`]s. The own player gets nothing.
pub fn push_items(
    looks: &[EntityLook],
    ents: &[EntityCtx],
    style: &XrayStyle,
    mates: [[u8; 4]; 2],
    rect_of: &dyn Fn(u16) -> Option<[f32; 4]>,
    out: &mut Vec<DrawItem>,
    draws: &mut Vec<XrayDraw>,
) {
    for l in looks {
        let Some(e) = ents.iter().find(|e| e.id == l.id) else {
            continue;
        };
        if e.is_local || (!e.is_enemy && !style.teammates) {
            continue;
        }
        let hidden = l.visible && !e.los;
        let mode = l.mode.unwrap_or(style.mode);
        let [primary, secondary] = if e.is_enemy {
            [style.primary, style.secondary]
        } else {
            mates
        };
        let draw = match mode {
            XrayMode::Box => false,
            // USER 2026-11-01: chams stay on a player in sight too (the flat colours over the normal draw)
            XrayMode::Chams => l.visible,
            XrayMode::Skeleton => hidden || l.outline,
        };
        if draw {
            draws.push(XrayDraw {
                entity: l.id,
                mode,
                primary,
                secondary,
            });
        }
        if mode != XrayMode::Box || (!hidden && !l.outline) {
            continue;
        }
        let Some([x, y, w, h]) = rect_of(l.id) else {
            continue;
        };
        if hidden {
            let mut f = primary;
            f[3] = (f[3] as u32 / FILL_ALPHA_DIV) as u8;
            out.push(DrawItem::Fill {
                rect: [x, y, w, h],
                colour: f,
            });
        }
        if l.outline {
            let b = OUTLINE_PX;
            for rect in [
                [x, y, w, b],
                [x, y + h - b, w, b],
                [x, y, b, h],
                [x + w - b, y, b, h],
            ] {
                out.push(DrawItem::Fill {
                    rect,
                    colour: secondary,
                });
            }
        }
    }
}

/// CLI-1631 (amended): the health bars of `looks` (every other player marked `visible`) when `style` switches them
/// on. The name tags are not drawn here: an enemy marked `visible` gets the 2009 enemy tag
/// ([`super::marks::frame_marks_forced`]). A bar stands centred [`INFO_GAP`] above the player's 2009 tag when one is
/// shown (`tag` = the caption's text style, its size the line height), else [`INFO_GAP`] above the body rectangle's
/// top; the bar takes the primary colour of an enemy, `mates[0]` on a team mate (none with `teammates` off). `info_of(id)` describes the player (`None`: he left or is
/// unknown, nothing drawn); a player who is not alive gets no bar.
pub fn push_info(
    looks: &[EntityLook],
    ents: &[EntityCtx],
    style: &XrayStyle,
    mates: [[u8; 4]; 2],
    tag: TextStyle,
    rect_of: &dyn Fn(u16) -> Option<[f32; 4]>,
    info_of: &dyn Fn(u16) -> Option<PlayerInfo>,
    out: &mut Vec<DrawItem>,
) {
    if !style.health {
        return;
    }
    let line = tag.size.unwrap_or(super::layout::MARK_FONT_SIZE) as f32;
    for l in looks.iter().filter(|l| l.visible) {
        let Some(e) = ents.iter().find(|e| e.id == l.id).filter(|e| !e.is_local) else {
            continue;
        };
        if !e.is_enemy && !style.teammates {
            continue;
        }
        let Some(info) = info_of(l.id).filter(|i| i.alive) else {
            continue;
        };
        let place = match info.tag_at {
            Some([x, y]) => Some([x, y - line]),
            None => rect_of(l.id).map(|[x, y, w, _]| [x + w / 2.0, y]),
        };
        let Some([cx, top]) = place else {
            continue;
        };
        let bar = [cx - BAR_W / 2.0, top - INFO_GAP - BAR_H, BAR_W, BAR_H];
        let primary = if e.is_enemy { style.primary } else { mates[0] };
        match info.health {
            Some((hp, max)) if max > 0 => {
                out.push(DrawItem::Fill {
                    rect: bar,
                    colour: BAR_BACK,
                });
                let share = (hp as f32 / max as f32).clamp(0.0, 1.0);
                if share > 0.0 {
                    out.push(DrawItem::Fill {
                        rect: [bar[0], bar[1], BAR_W * share, BAR_H],
                        colour: primary,
                    });
                }
            }
            _ => {
                let [bx, by, bw, bh] = bar;
                for rect in [
                    [bx, by, bw, 1.0],
                    [bx, by + bh - 1.0, bw, 1.0],
                    [bx, by, 1.0, bh],
                    [bx + bw - 1.0, by, 1.0, bh],
                ] {
                    out.push(DrawItem::Fill {
                        rect,
                        colour: primary,
                    });
                }
            }
        }
    }
}

/// The projected screen rectangle of a body box (`centre` +- `half`), clipped to the virtual screen; `None` when a
/// corner is behind the camera or nothing is left on screen (a target at 16:9 or narrower; [`body_rect_on`]).
pub fn body_rect(camera: &dc_render::Camera, centre: [f32; 3], half: [f32; 3]) -> Option<[f32; 4]> {
    body_rect_on(
        camera,
        centre,
        half,
        (dc_ui_data::VIRTUAL_WIDTH, dc_ui_data::VIRTUAL_HEIGHT),
    )
}

/// [`body_rect`] on a `size` target: clipped to the whole target in virtual units (CLI-1915: a target wider than
/// 16:9 reaches left of 0 and right of 1024).
pub fn body_rect_on(
    camera: &dc_render::Camera,
    centre: [f32; 3],
    half: [f32; 3],
    size: (u32, u32),
) -> Option<[f32; 4]> {
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for i in 0..8 {
        let s = |b: usize| if i & (1 << b) == 0 { -1.0 } else { 1.0 };
        let p = [
            centre[0] + s(0) * half[0],
            centre[1] + s(1) * half[1],
            centre[2] + s(2) * half[2],
        ];
        let q = super::project_on(camera, p, size)?;
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let [vl, vt, vr, vb] = dc_ui_data::VirtualScaler::new(size.0.max(1), size.1.max(1)).surface_virtual();
    let (x0, y0, x1, y1) = (lo[0].max(vl), lo[1].max(vt), hi[0].min(vr), hi[1].min(vb));
    (x1 > x0 && y1 > y0 && [x0, y0, x1, y1].iter().all(|v| v.is_finite())).then_some([
        x0,
        y0,
        x1 - x0,
        y1 - y0,
    ])
}

/// The players of a frame for the hook: the own player (`own`: object and team, at the camera) and every remote
/// with a finite body; `world` answers the line of sight from `eye` to each body centre, `is_enemy` the viewer's
/// relation to each remote.
pub fn entities(
    remotes: &[crate::world::RemoteBody],
    eye: [f32; 3],
    own: Option<(u16, u8)>,
    world: Option<&dyn dc_sim::Collider>,
    is_enemy: &dyn Fn(&crate::world::RemoteBody) -> bool,
    out: &mut Vec<EntityCtx>,
) {
    out.clear();
    if let Some((id, team)) = own {
        out.push(EntityCtx {
            id,
            team,
            is_enemy: false,
            distance: 0.0,
            is_local: true,
            los: true,
        });
    }
    for r in remotes {
        if !r.pos.iter().chain(r.half.iter()).all(|v| v.is_finite()) {
            continue;
        }
        let d = [r.pos[0] - eye[0], r.pos[1] - eye[1], r.pos[2] - eye[2]];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let los = match world {
            Some(w) if dist > 1e-3 => {
                let dir = [d[0] / dist, d[1] / dist, d[2] / dist];
                crate::arms::world_hit(w, eye, dir, dist).is_none_or(|h| h >= dist)
            }
            _ => true,
        };
        out.push(EntityCtx {
            id: r.obj,
            team: r.team,
            is_enemy: is_enemy(r),
            distance: dist,
            is_local: false,
            los,
        });
    }
}

/// The hook of the loaded mods: at most one Lua call per [`REFRESH_MS`] (under the host's per-call instruction limit), none
/// while no mod registered it; the first error switches it off for the rest of the match, logged once.
pub struct LuaRenderHook {
    host: Rc<RefCell<ModHost>>,
    disabled: Option<String>,
    /// Lua calls made (tests, the profiler).
    pub calls: u64,
    /// The frame's players and the last answer (buffers kept across frames).
    pub ents: Vec<EntityCtx>,
    pub looks: Vec<EntityLook>,
    /// CLI-1635: the session time and the number of remotes of the last refresh.
    refreshed: Option<(u32, usize)>,
}

impl LuaRenderHook {
    /// The bridge, or `None` unless the loaded set declares `hooks.render` and `world.full_visibility` (a local mod
    /// can declare neither: the host refuses it).
    pub fn new(host: Rc<RefCell<ModHost>>) -> Option<LuaRenderHook> {
        let mods = host.borrow().loaded_mods();
        let declared = NEEDED_CAPABILITIES
            .iter()
            .all(|c| mods.iter().any(|(m, _)| m.has_capability(c)));
        declared.then_some(LuaRenderHook {
            host,
            disabled: None,
            calls: 0,
            ents: Vec::new(),
            looks: Vec::new(),
            refreshed: None,
        })
    }

    /// CLI-1635: whether this frame asks again: none yet, [`REFRESH_MS`] passed or the number of remotes changed.
    pub fn due(&self, now_ms: u32, remotes: usize) -> bool {
        self.refreshed
            .is_none_or(|(t, n)| n != remotes || now_ms.wrapping_sub(t) >= REFRESH_MS)
    }

    /// CLI-1635: this frame asked.
    pub fn note_refresh(&mut self, now_ms: u32, remotes: usize) {
        self.refreshed = Some((now_ms, remotes));
    }

    /// Why the hook is off (`None`: running).
    pub fn disabled(&self) -> Option<&str> {
        self.disabled.as_deref()
    }

    /// Runs the hook over `self.ents` into `self.looks` (empty: no call, the hook is off, or it failed).
    pub fn run(&mut self) {
        self.looks.clear();
        if self.disabled.is_some() || self.ents.is_empty() {
            return;
        }
        let Ok(mut host) = self.host.try_borrow_mut() else {
            return;
        };
        if !host.has_hook(ENTITY_VISIBILITY_HOOK) {
            return;
        }
        self.calls += 1;
        let before = host.error_count();
        let r = host.run_hook(ENTITY_VISIBILITY_HOOK, context(&self.ents), |_| {
            Ok(LuaValue::Table(LuaTable::new_rc()))
        });
        let failed = match r {
            Ok(v) if host.error_count() == before => {
                parse_looks(&v, &self.ents, &mut self.looks);
                return;
            }
            Ok(_) => host
                .all_mod_errors()
                .into_values()
                .flatten()
                .last()
                .unwrap_or_else(|| "a hook raised an error".into()),
            Err(e) => e,
        };
        drop(host);
        eprintln!("mods: {ENTITY_VISIBILITY_HOOK} is off for the rest of the match: {failed}");
        self.disabled = Some(failed);
    }
}

#[cfg(test)]
#[path = "xray_tests.rs"]
mod tests;
