//! Mods on the HUD: the override keys of the loaded set ([`apply_overrides`], [`scale_items`]; CLI-1613) and the
//! `hud.draw` hook bridge ([`LuaHudHook`]; CLI-1614).
//!
//! The display list is cut into named elements ([`super::items::build_marked`] and [`super::Hud::items`]): a hook
//! sees each element's name and rectangle and may hide, move or tint it. Elements whose place follows other players
//! or world objects ([`PRIVATE_ELEMENTS`]: the names over heads, the radar, the damage arcs, the team-mate panel, the
//! case markers) are neither listed nor changeable, so a hook learns nothing about where others are.
//!
//! Provenance: ORIGINAL (DeCombat's own mod API; the 2009 client has no HUD mods).
// DC-DEFAULT: CLI-1613 (proposed) the HUD override keys, see DC_DEFAULTS.md
// DC-DEFAULT: CLI-1614 (proposed) the hud.draw context holds cosmetics and the own player's state only

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;

use dc_mod_lua::{LuaKey, LuaTable, LuaValue, ModHost};

use super::compose::HudAssets;
use super::items::{flash_colour, DrawItem, HudDrawHook, VIRTUAL_CENTRE};
use super::layout::HudLayout;
use super::state::HudState;
use crate::modassets::{HudOverrides, HITMARKER_MS};
use dc_ui_data::VirtualScaler;

/// The named elements of one frame's list, in list order.
pub type Spans = Vec<(&'static str, Range<usize>)>;

/// Elements a hook never sees or changes: their places follow other players or world objects.
pub const PRIVATE_ELEMENTS: &[&str] = &[
    "damage_arcs",
    "player_names",
    "mate_panel",
    "hisec_cases",
    "radar",
    "xray",
];

/// The hook name the bridge runs.
pub const HUD_DRAW_HOOK: &str = "hud.draw";

/// Element marks (`(name, first index)`) to ranges up to the next mark; empty elements are dropped.
pub fn spans_of(marks: &[(&'static str, usize)], len: usize) -> Spans {
    let mut out = Spans::new();
    for (i, (name, start)) in marks.iter().enumerate() {
        let end = marks.get(i + 1).map_or(len, |m| m.1).min(len);
        if *start < end {
            out.push((name, *start..end));
        }
    }
    out
}

/// Appends `item` to element `owner` of the list (at its end); the later elements move up by one.
fn append_to(items: &mut Vec<DrawItem>, spans: &mut Spans, owner: usize, item: DrawItem) {
    let at = spans[owner].1.end;
    items.insert(at, item);
    for (i, (_, r)) in spans.iter_mut().enumerate() {
        if i == owner {
            r.end += 1;
        } else if r.start >= at {
            *r = r.start + 1..r.end + 1;
        }
    }
}

/// `size` texels centred on the screen. `k` = horizontal / vertical virtual scale of the target (`sx / sy`): the height
/// in virtual units is `size[1] x k`, so a texel is square on any aspect and the picture is as large as the aim mark,
/// which scales by the width on both axes (a ring stays round on a wide screen).
// DC-DEFAULT: CLI-1613 (amended 2026-11-01) override pictures scale like the aim mark
fn centred(tex: &str, texels: [f32; 2], tint: [u8; 4], k: f32) -> DrawItem {
    let k = if k.is_finite() && k > 0.0 { k } else { 1.0 };
    let size = [texels[0], texels[1] * k];
    DrawItem::Image {
        tex: tex.to_string(),
        src: texels,
        dst: [
            VIRTUAL_CENTRE[0] - size[0] / 2.0,
            VIRTUAL_CENTRE[1] - size[1] / 2.0,
            size[0],
            size[1],
        ],
        tint,
        turn: None,
    }
}

/// Applies `crosshair`, `crosshair_colour`, `hitmarker` and the drawn crosshair's keys (`crosshair_style`, ...) to the `crosshair` element of the list. `k`: the target's
/// `sx / sy` ([`centred`]; 1.0 on a 4:3 target).
pub fn apply_overrides(
    ov: &HudOverrides,
    state: &HudState,
    assets: &mut HudAssets,
    items: &mut Vec<DrawItem>,
    spans: &mut Spans,
    k: f32,
) {
    let Some(i) = spans.iter().position(|(n, _)| *n == "crosshair") else {
        return;
    };
    let range = spans[i].1.clone();
    let flash = state.hit_flash();
    let drawn_style = ov.crosshair_style;
    let shape = ov.cross_look();
    let tex = ov.crosshair.as_deref().and_then(|p| {
        assets
            .texture(p)
            .map(|t| (p, [t.width as f32, t.height as f32]))
    });
    let mut drawn = false;
    for item in &mut items[range.clone()] {
        match item {
            DrawItem::AimMark(v) => {
                drawn = true;
                if let Some(c) = ov.crosshair_colour {
                    v.colour = flash_colour(c, flash);
                }
            }
            DrawItem::AimCross { colour, .. } => {
                drawn = true;
                if let Some(c) = ov.crosshair_colour {
                    *colour = flash_colour(c, flash);
                }
            }
            _ => continue,
        }
        // CLI-1633: the drawn crosshair takes the place of the stock mark and of a texture
        if let Some(style) = drawn_style {
            let mark = match item {
                DrawItem::AimMark(v) => Some(*v),
                DrawItem::AimCross {
                    half_gap, colour, ..
                } => Some(crate::spread::AimMarkView {
                    shape: crate::spread::mark::SHAPE_PLAIN,
                    distance: *half_gap,
                    since_shot_s: None,
                    colour: *colour,
                    rest: *half_gap,
                }),
                _ => None,
            };
            if let Some(v) = mark {
                *item = DrawItem::Crosshair(super::crosshair::DrawnCrosshair::of_shape(style, &v, &shape));
            }
            continue;
        }
        if let Some((path, size)) = tex {
            let tint = flash_colour(ov.crosshair_colour.unwrap_or([255; 4]), flash);
            *item = centred(path, size, tint, k);
        }
    }
    if let (true, Some(path), Some(at)) = (drawn, ov.hitmarker.as_deref(), state.own_hit_at) {
        let age = state.now_ms.saturating_sub(at);
        if age < HITMARKER_MS {
            if let Some(t) = assets.texture(path) {
                let a = (255.0 * (1.0 - age as f32 / HITMARKER_MS as f32)).round() as u8;
                let item = centred(
                    path,
                    [t.width as f32, t.height as f32],
                    [255, 255, 255, a],
                    k,
                );
                append_to(items, spans, i, item);
            }
        }
    }
}

/// The crosshair units of a composed list, found by KIND over the whole list (not by the element's place, which
/// another element's items can move): every aim mark, static aim cross and image of the override texture `tex`, in
/// list order. `(kind, line)`: `kind` names the units (and texture) without the per-frame colour, `line` is
/// `mods: hud crosshair drawn as ...` with each unit's colour and pixel rectangle on a `size` target, joined by ` + `.
/// `None` when the list has none (dead, scoped, between rounds).
// DC-DEFAULT: CLI-1613 (amended 2026-11-01) the crosshair log searches the list by kind
pub fn crosshair_drawn(
    items: &[DrawItem],
    tex: Option<&str>,
    size: (u32, u32),
) -> Option<(String, String)> {
    let hex = |c: [u8; 4]| format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]);
    let vs = VirtualScaler::new(size.0, size.1);
    let parts: Vec<(String, String)> = items
        .iter()
        .filter_map(|it| match it {
            DrawItem::Image {
                tex: t, dst, tint, ..
            } if Some(t.as_str()) == tex => Some((
                format!("image {t}"),
                format!(
                    "image {t} tint {} at {:.0},{:.0} {:.0}x{:.0} px",
                    hex(*tint),
                    dst[0] * vs.sx + vs.ox,
                    dst[1] * vs.sy + vs.oy,
                    dst[2] * vs.sx,
                    dst[3] * vs.sy
                ),
            )),
            DrawItem::AimMark(v) => {
                Some(("aim mark".into(), format!("aim mark {}", hex(v.colour))))
            }
            DrawItem::AimCross { colour, .. } => Some((
                "static cross".into(),
                format!("static cross {}", hex(*colour)),
            )),
            DrawItem::Crosshair(c) => Some((
                format!("drawn {}", c.style.name()),
                format!("drawn {} {}", c.style.name(), hex(c.colour)),
            )),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    let kind = parts
        .iter()
        .map(|p| p.0.as_str())
        .collect::<Vec<_>>()
        .join(" + ");
    let line = parts
        .iter()
        .map(|p| p.1.as_str())
        .collect::<Vec<_>>()
        .join(" + ");
    Some((kind, format!("mods: hud crosshair drawn as {line}")))
}

/// The bounding box `(x, y, w, h)` of an element's items in virtual pixels: images and fills by their rectangles,
/// text by its place (a point), the procedural crosshair as the screen centre.
pub fn element_rect(items: &[DrawItem]) -> Option<[f32; 4]> {
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    let mut add = |r: [f32; 4]| {
        lo = [lo[0].min(r[0]), lo[1].min(r[1])];
        hi = [hi[0].max(r[0] + r[2]), hi[1].max(r[1] + r[3])];
    };
    for it in items {
        match it {
            DrawItem::Image { dst, .. } => add(*dst),
            DrawItem::Fill { rect, .. } => add(*rect),
            DrawItem::Text { pos, .. } | DrawItem::Label { pos, .. } => {
                add([pos[0], pos[1], 0.0, 0.0])
            }
            DrawItem::AimMark(_) | DrawItem::AimCross { .. } | DrawItem::Crosshair(_) => {
                add([VIRTUAL_CENTRE[0], VIRTUAL_CENTRE[1], 0.0, 0.0])
            }
        }
    }
    (lo[0] <= hi[0]).then(|| [lo[0], lo[1], hi[0] - lo[0], hi[1] - lo[1]])
}

/// `hud_scale`: each element grows or shrinks by `k` about the nearest of its screen edge or the centre on each axis
/// (an element in the left third keeps its distance ratio to the left edge, one in the middle third to the centre),
/// so corner blocks stay in their corners. Text with no size of its own keeps its font size.
pub fn scale_items(items: &mut [DrawItem], spans: &Spans, k: f32) {
    if !(k.is_finite() && k > 0.0) || (k - 1.0).abs() < 1e-6 {
        return;
    }
    let (vw, vh) = (
        dc_ui_data::VIRTUAL_WIDTH as f32,
        dc_ui_data::VIRTUAL_HEIGHT as f32,
    );
    let anchor = |c: f32, span: f32| {
        if c < span / 3.0 {
            0.0
        } else if c > span * 2.0 / 3.0 {
            span
        } else {
            span / 2.0
        }
    };
    for (_, r) in spans {
        let Some(b) = element_rect(&items[r.clone()]) else {
            continue;
        };
        let a = [anchor(b[0] + b[2] / 2.0, vw), anchor(b[1] + b[3] / 2.0, vh)];
        let pt = |p: [f32; 2]| [a[0] + (p[0] - a[0]) * k, a[1] + (p[1] - a[1]) * k];
        let rect = |q: [f32; 4]| {
            let p = pt([q[0], q[1]]);
            [p[0], p[1], q[2] * k, q[3] * k]
        };
        for it in &mut items[r.clone()] {
            match it {
                DrawItem::Image { dst, turn, .. } => {
                    *dst = rect(*dst);
                    if let Some((pivot, _)) = turn {
                        *pivot = pt(*pivot);
                    }
                }
                DrawItem::Fill { rect: q, .. } => *q = rect(*q),
                DrawItem::Text { style, pos, .. } | DrawItem::Label { style, pos, .. } => {
                    *pos = pt(*pos);
                    if let Some(s) = style.size.as_mut() {
                        *s = ((*s as f32) * k).round().max(1.0) as i32;
                    }
                }
                DrawItem::AimMark(v) => v.distance *= k,
                DrawItem::Crosshair(c) => c.inner *= k,
                DrawItem::AimCross { bar, half_gap, .. } => {
                    *bar *= k;
                    *half_gap *= k;
                }
            }
        }
    }
}

/// The context a `hud.draw` hook receives.
///
/// RULE (CLI-1614, HOSTING_AND_MODS.md §3.1, the wall-see gate): this holds ONLY cosmetics and the OWN player's
/// state. It must never carry world state: no other player (names, health, teams, places, counts), no positions in
/// the world, no object or radar data, no element whose place follows another player. A client_visual mod that saw
/// such data could draw a wallhack. Adding a field here is a review question; the tests pin the field list and the
/// serialised key set.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HudDrawContext {
    /// The render target in pixels.
    pub screen_w: u32,
    pub screen_h: u32,
    /// The own health and armour.
    pub health: Option<f32>,
    pub armor: Option<f32>,
    /// The held weapon's rounds outside the clip and in the clip.
    pub ammo: Option<u32>,
    pub clip: Option<u32>,
    /// The held weapon's row id.
    pub weapon_id: Option<u16>,
    /// Milliseconds left on the round clock while it is shown.
    pub round_time_ms: Option<u32>,
    /// The own kills and deaths of the board.
    pub score_kills: Option<i32>,
    pub score_deaths: Option<i32>,
    /// The HUD elements of this frame by name with their rectangle (virtual 1024 x 768 pixels), [`PRIVATE_ELEMENTS`]
    /// excluded.
    pub elements: Vec<(String, [f32; 4])>,
}

/// Every key [`HudDrawContext::to_lua`] may write, at any depth.
pub const CONTEXT_KEYS: &[&str] = &[
    "screen",
    "w",
    "h",
    "health",
    "armor",
    "ammo",
    "clip",
    "weapon",
    "round_time_ms",
    "score",
    "kills",
    "deaths",
    "elements",
    "x",
    "y",
];

impl HudDrawContext {
    /// The context of one frame: the own state of `state` and the public elements of `items`.
    pub fn from_frame(
        state: &HudState,
        screen: (u32, u32),
        items: &[DrawItem],
        spans: &Spans,
    ) -> Self {
        let own = state.own_pid.and_then(|p| state.roster.get(&p));
        let clock = &state.clock;
        HudDrawContext {
            screen_w: screen.0,
            screen_h: screen.1,
            health: state.health,
            armor: state.armour,
            ammo: state.ammo.as_ref().map(|a| a.reserve),
            clip: state.ammo.as_ref().map(|a| a.clip),
            weapon_id: state.ammo.as_ref().map(|a| a.weapon),
            round_time_ms: clock.shown.then(|| clock.ms_left(state.now_ms)),
            score_kills: own.map(|r| r.kills),
            score_deaths: own.map(|r| r.deaths),
            elements: spans
                .iter()
                .filter(|(n, _)| !PRIVATE_ELEMENTS.contains(n))
                .filter_map(|(n, r)| element_rect(&items[r.clone()]).map(|b| (n.to_string(), b)))
                .collect(),
        }
    }

    /// The Lua table the hook gets: `screen = {w, h}`, `health`, `armor`, `ammo`, `clip`, `weapon`, `round_time_ms`,
    /// `score = {kills, deaths}`, `elements = {<name> = {x, y, w, h}}` (absent values are nil).
    pub fn to_lua(&self) -> LuaTable {
        let opt = |t: &mut LuaTable, k: &str, v: Option<f64>| {
            if let Some(v) = v {
                t.set_str(k, LuaValue::Number(v));
            }
        };
        let table = |pairs: &[(&str, f64)]| {
            let mut t = LuaTable::new();
            for (k, v) in pairs {
                t.set_str(*k, LuaValue::Number(*v));
            }
            LuaValue::Table(Rc::new(RefCell::new(t)))
        };
        let mut t = LuaTable::new();
        t.set_str(
            "screen",
            table(&[("w", self.screen_w as f64), ("h", self.screen_h as f64)]),
        );
        opt(&mut t, "health", self.health.map(f64::from));
        opt(&mut t, "armor", self.armor.map(f64::from));
        opt(&mut t, "ammo", self.ammo.map(f64::from));
        opt(&mut t, "clip", self.clip.map(f64::from));
        opt(&mut t, "weapon", self.weapon_id.map(f64::from));
        opt(&mut t, "round_time_ms", self.round_time_ms.map(f64::from));
        let mut score = LuaTable::new();
        opt(&mut score, "kills", self.score_kills.map(f64::from));
        opt(&mut score, "deaths", self.score_deaths.map(f64::from));
        t.set_str("score", LuaValue::Table(Rc::new(RefCell::new(score))));
        let mut el = LuaTable::new();
        for (name, r) in &self.elements {
            el.set_str(
                name.clone(),
                table(&[
                    ("x", r[0] as f64),
                    ("y", r[1] as f64),
                    ("w", r[2] as f64),
                    ("h", r[3] as f64),
                ]),
            );
        }
        t.set_str("elements", LuaValue::Table(Rc::new(RefCell::new(el))));
        t
    }
}

/// What a hook asks of one element.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ElementChange {
    pub visible: Option<bool>,
    /// Virtual pixels.
    pub offset: Option<[f32; 2]>,
    /// Multiplies the element's colours (255 = unchanged).
    pub tint: Option<[u8; 4]>,
}

fn number_pair(v: &LuaValue, a: &str, b: &str) -> Option<[f32; 2]> {
    let t = v.as_table()?;
    let t = t.borrow();
    let get = |named: &str, idx: i64| {
        let by_name = t.get_str(named);
        let v = if matches!(by_name, LuaValue::Nil) {
            t.get_int(idx)
        } else {
            by_name
        };
        v.as_f64().filter(|x| x.is_finite()).map(|x| x as f32)
    };
    Some([get(a, 1)?, get(b, 2)?])
}

fn tint_of(v: &LuaValue) -> Option<[u8; 4]> {
    if let Some(s) = v.as_str() {
        return dc_mod_lua::parse_rgba_hex(s);
    }
    let t = v.as_table()?;
    let t = t.borrow();
    let c = |i: i64, d: f64| {
        let v = t.get_int(i);
        let x = if matches!(v, LuaValue::Nil) {
            Some(d)
        } else {
            v.as_f64()
        };
        x.filter(|x| x.is_finite())
            .map(|x| x.clamp(0.0, 255.0).round() as u8)
    };
    Some([c(1, 255.0)?, c(2, 255.0)?, c(3, 255.0)?, c(4, 255.0)?])
}

/// The hook's answer: `{ <element> = { visible = bool, offset = {x, y}, tint = {r, g, b, a} | "rrggbbaa" } }`.
/// Unknown and private element names and malformed fields are ignored.
pub fn parse_changes(v: &LuaValue, spans: &Spans) -> BTreeMap<&'static str, ElementChange> {
    let mut out = BTreeMap::new();
    let Some(t) = v.as_table() else {
        return out;
    };
    for (k, val) in t.borrow().entries() {
        let LuaKey::String(name) = k else { continue };
        let Some((n, _)) = spans
            .iter()
            .find(|(n, _)| *n == name.as_str() && !PRIVATE_ELEMENTS.contains(n))
        else {
            continue;
        };
        let Some(c) = val.as_table() else { continue };
        let c = c.borrow();
        out.insert(
            *n,
            ElementChange {
                visible: c.get_str("visible").as_bool(),
                offset: number_pair(&c.get_str("offset"), "x", "y"),
                tint: tint_of(&c.get_str("tint")),
            },
        );
    }
    out
}

fn mul(c: [u8; 4], t: [u8; 4]) -> [u8; 4] {
    std::array::from_fn(|i| ((c[i] as u32 * t[i] as u32 + 127) / 255) as u8)
}

/// Applies a hook's changes: offsets and tints in place, then the hidden elements leave the list.
pub fn apply_changes(
    items: &mut Vec<DrawItem>,
    spans: &Spans,
    changes: &BTreeMap<&'static str, ElementChange>,
) {
    let mut hidden: Vec<Range<usize>> = Vec::new();
    for (name, r) in spans {
        let Some(ch) = changes.get(name) else {
            continue;
        };
        if ch.visible == Some(false) {
            hidden.push(r.clone());
            continue;
        }
        for it in &mut items[r.clone()] {
            if let Some([dx, dy]) = ch.offset {
                match it {
                    DrawItem::Image { dst, turn, .. } => {
                        dst[0] += dx;
                        dst[1] += dy;
                        if let Some((p, _)) = turn {
                            *p = [p[0] + dx, p[1] + dy];
                        }
                    }
                    DrawItem::Fill { rect, .. } => {
                        rect[0] += dx;
                        rect[1] += dy;
                    }
                    DrawItem::Text { pos, .. } | DrawItem::Label { pos, .. } => {
                        *pos = [pos[0] + dx, pos[1] + dy];
                    }
                    // the procedural crosshair is drawn at the target centre
                    DrawItem::AimMark(_) | DrawItem::AimCross { .. } | DrawItem::Crosshair(_) => {}
                }
            }
            if let Some(t) = ch.tint {
                match it {
                    DrawItem::Image { tint, .. } => *tint = mul(*tint, t),
                    DrawItem::Fill { colour, .. } | DrawItem::AimCross { colour, .. } => {
                        *colour = mul(*colour, t)
                    }
                    DrawItem::Text { style, .. } | DrawItem::Label { style, .. } => {
                        style.colour = mul(style.colour, t)
                    }
                    DrawItem::AimMark(v) => v.colour = mul(v.colour, t),
                    DrawItem::Crosshair(c) => c.colour = mul(c.colour, t),
                }
            }
        }
    }
    hidden.sort_by_key(|r| std::cmp::Reverse(r.start));
    for r in hidden {
        items.drain(r);
    }
}

/// The `hud.draw` hook of the loaded mods as a [`HudDrawHook`]: at most one Lua call per composed frame (under the
/// host's per-call instruction limit), none while no mod registered the hook; the first error (a raise, a limit, a
/// malformed answer the host refuses) switches it off for the rest of the match and is logged once.
///
/// Wiring (the runtime owns the host): at match start, `hud.hook = LuaHudHook::new(Rc::clone(&host)).map(|h|
/// Box::new(h) as Box<dyn HudDrawHook>)`; a new match builds a new hook.
pub struct LuaHudHook {
    host: Rc<RefCell<ModHost>>,
    disabled: Option<String>,
    /// Lua calls made (tests, the profiler).
    pub calls: u64,
}

impl LuaHudHook {
    /// The bridge, or `None` when no loaded mod declares the `hooks.hud` capability (no hook object, no per-frame
    /// cost).
    pub fn new(host: Rc<RefCell<ModHost>>) -> Option<LuaHudHook> {
        let declared = host
            .borrow()
            .loaded_mods()
            .iter()
            .any(|(m, _)| m.has_capability("hooks.hud"));
        declared.then_some(LuaHudHook {
            host,
            disabled: None,
            calls: 0,
        })
    }

    /// Why the hook is off (`None`: running).
    pub fn disabled(&self) -> Option<&str> {
        self.disabled.as_deref()
    }

    fn disable(&mut self, why: String) {
        eprintln!("mods: hud.draw is off for the rest of the match: {why}");
        self.disabled = Some(why);
    }

    /// One frame: the changes the hook chain asks for (`None`: no call made or the hook failed).
    fn run(&mut self, ctx: &HudDrawContext) -> Option<LuaValue> {
        if self.disabled.is_some() {
            return None;
        }
        let Ok(mut host) = self.host.try_borrow_mut() else {
            return None;
        };
        if !host.has_hook(HUD_DRAW_HOOK) {
            return None;
        }
        self.calls += 1;
        let before = host.error_count();
        let r = host.run_hook(HUD_DRAW_HOOK, ctx.to_lua(), |_| {
            Ok(LuaValue::Table(LuaTable::new_rc()))
        });
        let failed = match r {
            Ok(v) if host.error_count() == before => return Some(v),
            Ok(_) => host
                .all_mod_errors()
                .into_values()
                .flatten()
                .last()
                .unwrap_or_else(|| "a hook raised an error".into()),
            Err(e) => e,
        };
        drop(host);
        self.disable(failed);
        None
    }
}

impl HudDrawHook for LuaHudHook {
    fn draw(&mut self, state: &HudState, layout: &HudLayout, items: &mut Vec<DrawItem>) {
        self.draw_frame(state, layout, (1024, 768), &Spans::new(), items);
    }

    fn draw_frame(
        &mut self,
        state: &HudState,
        _layout: &HudLayout,
        screen: (u32, u32),
        spans: &[(&'static str, Range<usize>)],
        items: &mut Vec<DrawItem>,
    ) {
        let spans: Spans = spans.to_vec();
        let ctx = HudDrawContext::from_frame(state, screen, items, &spans);
        if let Some(v) = self.run(&ctx) {
            apply_changes(items, &spans, &parse_changes(&v, &spans));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::compose::{HudFonts, Tex, TextureSource};
    use crate::hud::marks::{MarkState, PlayerMark};
    use crate::hud::state::{AmmoView, BoardRow, RoundClock};
    use crate::hud::{Hud, FALLBACK_FONT_CELL};
    use dc_mod_lua::ModManifest;

    /// A frame with the own state and plenty about others (roster, a name over a head).
    fn state() -> HudState {
        let mut s = HudState {
            life_state: 1,
            health: Some(87.0),
            armour: Some(20.0),
            ammo: Some(AmmoView {
                weapon: 145,
                clip: 25,
                reserve: 90,
                clip_size: Some(30),
                single_shot: false,
                order_class: 1,
                infinite: false,
            }),
            own_pid: Some(0),
            clock: RoundClock {
                shown: true,
                secs: 125,
                at: 0,
                running: true,
            },
            now_ms: 5000,
            ..HudState::default()
        };
        for (pid, name, team) in [(0u8, "me", 1u8), (1, "enemy", 2)] {
            s.roster.insert(
                pid,
                BoardRow {
                    pid,
                    name: name.into(),
                    team,
                    kills: 2 + pid as i32,
                    deaths: 1,
                    ..BoardRow::default()
                },
            );
        }
        s.marks = vec![PlayerMark {
            pid: 1,
            obj: 11,
            name: "enemy".into(),
            at: [700.0, 300.0],
            state: MarkState::Enemy,
        }];
        s
    }

    fn manifest(caps: &str) -> ModManifest {
        ModManifest::from_json_str(&format!(
            r#"{{"id": "hudmod", "name": "H", "version": "1.0.0", "kind": "client_visual",
                "description": "A test fixture mod for the HUD tests.", "author": "tests", "screenshots": ["screenshots/main.png"],
                "capabilities": [{caps}]}}"#
        ))
        .unwrap()
    }

    fn host_with(src: &str) -> Rc<RefCell<ModHost>> {
        let mut host = ModHost::new();
        host.load_mod_from_source(manifest(r#""hooks.hud""#), src, None)
            .unwrap();
        Rc::new(RefCell::new(host))
    }

    fn texts(items: &[DrawItem]) -> Vec<String> {
        items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// A hook that records the context the HUD would give.
    struct Probe(Rc<RefCell<Option<HudDrawContext>>>);
    impl HudDrawHook for Probe {
        fn draw(&mut self, _: &HudState, _: &HudLayout, _: &mut Vec<DrawItem>) {}
        fn draw_frame(
            &mut self,
            st: &HudState,
            _: &HudLayout,
            screen: (u32, u32),
            spans: &[(&'static str, Range<usize>)],
            items: &mut Vec<DrawItem>,
        ) {
            *self.0.borrow_mut() = Some(HudDrawContext::from_frame(
                st,
                screen,
                items,
                &spans.to_vec(),
            ));
        }
    }

    fn walk(t: &LuaTable, under_elements: bool, keys: &mut Vec<String>) {
        for (k, v) in t.entries() {
            let LuaKey::String(k) = k else {
                panic!("only named keys")
            };
            if !under_elements {
                keys.push(k.clone());
            }
            if let LuaValue::Table(c) = v {
                walk(&c.borrow(), k == "elements" && !under_elements, keys);
            }
        }
    }

    #[test]
    fn the_hook_context_holds_cosmetics_and_own_state_only() {
        // compile-time shape: a new field breaks this pattern and must pass the CLI-1614 review
        let HudDrawContext {
            screen_w: _,
            screen_h: _,
            health: _,
            armor: _,
            ammo: _,
            clip: _,
            weapon_id: _,
            round_time_ms: _,
            score_kills: _,
            score_deaths: _,
            elements: _,
        } = HudDrawContext::default();

        let mut hud = Hud::fallback(1024, 768, None);
        let s = state();
        let seen = Rc::new(RefCell::new(None));
        hud.hook = Some(Box::new(Probe(Rc::clone(&seen))));
        let _ = hud.items(&s);
        let ctx = seen.borrow().clone().expect("the hook ran");
        assert_eq!(
            (ctx.health, ctx.clip, ctx.ammo, ctx.weapon_id),
            (Some(87.0), Some(25), Some(90), Some(145))
        );
        assert_eq!(
            (ctx.score_kills, ctx.score_deaths),
            (Some(2), Some(1)),
            "the OWN row only"
        );
        let names: Vec<&str> = ctx.elements.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"health") && names.contains(&"crosshair"),
            "{names:?}"
        );
        for p in PRIVATE_ELEMENTS {
            assert!(!names.contains(p), "{p} must not be listed: {names:?}");
        }

        // the serialised key set: the pinned vocabulary at every depth, element names under `elements` only
        let mut keys = Vec::new();
        walk(&ctx.to_lua(), false, &mut keys);
        for k in &keys {
            assert!(
                CONTEXT_KEYS.contains(&k.as_str()),
                "unexpected context key {k}"
            );
            for bad in [
                "player", "pos", "team", "enemy", "roster", "radar", "mark", "origin", "world",
                "name",
            ] {
                assert!(!k.contains(bad), "context key {k} looks like world state");
            }
        }
        let flat = format!("{:?}", ctx);
        assert!(
            !flat.contains("enemy") && !flat.contains("700"),
            "nothing of the other player: {flat}"
        );
    }

    #[test]
    fn a_lua_hook_hides_an_element_of_the_headless_hud() {
        let host = host_with(
            r#"return function(mod)
                mod.hooks:wrap("hud.draw", function(base, ctx)
                    local r = base(ctx)
                    if ctx.health == 87 and ctx.elements.health ~= nil then
                        r.health = { visible = false }
                        r.round_clock = { offset = { 10, 0 } }
                        r.player_names = { visible = false }
                    end
                    return r
                end)
            end"#,
        );
        let mut hud = Hud::fallback(1024, 768, None);
        let s = state();
        let before = texts(&hud.items(&s));
        assert!(
            before.iter().any(|t| t == "HP 87") && before.iter().any(|t| t == "enemy"),
            "{before:?}"
        );
        hud.hook = LuaHudHook::new(Rc::clone(&host)).map(|h| Box::new(h) as Box<dyn HudDrawHook>);
        assert!(hud.hook.is_some());
        let after = texts(&hud.items(&s));
        assert!(
            !after.iter().any(|t| t == "HP 87"),
            "health hidden: {after:?}"
        );
        assert!(
            after.iter().any(|t| t == "enemy"),
            "a private element is not the hook's to hide"
        );
        assert!(host.borrow().all_mod_errors().is_empty());
    }

    #[test]
    fn a_lua_error_turns_the_hook_off_for_the_rest_of_the_match() {
        let host = host_with(
            r#"return function(mod)
                mod.hooks:wrap("hud.draw", function(base, ctx) error("boom") end)
            end"#,
        );
        let mut hook = LuaHudHook::new(host).unwrap();
        let layout = HudLayout::fallback();
        let s = state();
        let mut items = vec![DrawItem::Fill {
            rect: [10.0, 700.0, 50.0, 10.0],
            colour: [255; 4],
        }];
        let spans: Spans = vec![("health", 0..1)];
        hook.draw_frame(&s, &layout, (1024, 768), &spans, &mut items);
        assert_eq!(hook.calls, 1);
        assert!(
            hook.disabled().is_some_and(|w| w.contains("boom")),
            "{:?}",
            hook.disabled()
        );
        hook.draw_frame(&s, &layout, (1024, 768), &spans, &mut items);
        assert_eq!(hook.calls, 1, "no call after the error");
        assert_eq!(items.len(), 1, "the list is untouched");
    }

    #[test]
    fn no_declared_hud_hook_means_no_bridge() {
        let mut host = ModHost::new();
        host.load_mod_from_source(
            manifest(r#""content.hud""#),
            "return function(mod) end",
            None,
        )
        .unwrap();
        assert!(LuaHudHook::new(Rc::new(RefCell::new(host))).is_none());
    }

    struct ModTex;
    impl TextureSource for ModTex {
        fn load(&mut self, path: &str) -> Option<Tex> {
            (path == "@mod:green/assets/crosshair.dtx").then(|| Tex {
                width: 32,
                height: 32,
                rgba: vec![255; 32 * 32 * 4],
            })
        }
    }

    fn mod_assets() -> HudAssets {
        HudAssets::new(
            Box::new(ModTex),
            HudFonts::new(None, FALLBACK_FONT_CELL, None, 1.0),
        )
    }

    #[test]
    fn the_crosshair_keys_replace_and_recolour_the_aim_cross() {
        let mut hud = Hud::fallback(1024, 768, None);
        hud.assets = mod_assets();
        let s = state();
        let cross = |items: &[DrawItem]| {
            items.iter().find_map(|i| match i {
                DrawItem::AimCross { colour, .. } => Some(*colour),
                _ => None,
            })
        };
        assert!(
            cross(&hud.items(&s)).is_some(),
            "the fallback layout draws the static cross"
        );
        hud.overrides.crosshair_colour = Some([0, 255, 0, 255]);
        assert_eq!(cross(&hud.items(&s)), Some([0, 255, 0, 255]));
        hud.overrides.crosshair = Some("@mod:green/assets/crosshair.dtx".into());
        let items = hud.items(&s);
        assert!(cross(&items).is_none());
        let img = items.iter().find_map(|i| match i {
            DrawItem::Image { tex, dst, tint, .. } if tex.starts_with("@mod:") => {
                Some((*dst, *tint))
            }
            _ => None,
        });
        assert_eq!(img, Some(([496.0, 368.0, 32.0, 32.0], [0, 255, 0, 255])));
        // a missing texture keeps the procedural cross
        hud.overrides.crosshair = Some("@mod:green/assets/none.dtx".into());
        assert!(cross(&hud.items(&s)).is_some());
    }

    /// CLI-1613 (playtest report): the live log names the units that draw the crosshair, found by kind over
    /// the whole list (the mod picture with its tint and pixel box, the stock aim mark), wherever they sit.
    #[test]
    fn the_crosshair_log_names_the_drawing_unit() {
        let mark = DrawItem::AimMark(crate::spread::AimMarkView {
            shape: crate::spread::mark::SHAPE_PLAIN,
            distance: 10.0,
            since_shot_s: None,
            colour: [0, 255, 0, 255],
            rest: 5.0,
        });
        let fill = DrawItem::Fill {
            rect: [0.0; 4],
            colour: [0; 4],
        };
        let path = "@mod:test_crosshair/assets/crosshair.dtx";
        let (kind, line) =
            crosshair_drawn(&[fill.clone(), mark.clone()], Some(path), (1024, 768)).unwrap();
        assert_eq!(kind, "aim mark");
        assert_eq!(line, "mods: hud crosshair drawn as aim mark #00ff00ff");
        let ring = centred(path, [32.0, 32.0], [64, 128, 255, 255], 1.0);
        let other = centred("@mod:x/assets/hit.dtx", [8.0, 8.0], [255; 4], 1.0);
        let (kind, line) = crosshair_drawn(
            &[fill.clone(), other, ring.clone()],
            Some(path),
            (2048, 1536),
        )
        .unwrap();
        assert_eq!(kind, "image @mod:test_crosshair/assets/crosshair.dtx");
        assert_eq!(
            line,
            "mods: hud crosshair drawn as image @mod:test_crosshair/assets/crosshair.dtx tint #4080ffff at 992,736 64x64 px"
        );
        // both kinds at once are both named
        let (kind, _) = crosshair_drawn(&[ring, mark], Some(path), (1024, 768)).unwrap();
        assert_eq!(
            kind,
            "image @mod:test_crosshair/assets/crosshair.dtx + aim mark"
        );
        // no crosshair unit (dead, scoped) logs nothing
        assert!(crosshair_drawn(&[fill], Some(path), (1024, 768)).is_none());
    }

    #[test]
    fn hud_scale_keeps_corner_blocks_in_their_corners() {
        let mut items = vec![
            DrawItem::Fill {
                rect: [10.0, 700.0, 100.0, 40.0],
                colour: [255; 4],
            },
            DrawItem::Fill {
                rect: [900.0, 10.0, 100.0, 40.0],
                colour: [255; 4],
            },
        ];
        let spans: Spans = vec![("health", 0..1), ("round_clock", 1..2)];
        scale_items(&mut items, &spans, 2.0);
        let rect = |i: &DrawItem| match i {
            DrawItem::Fill { rect, .. } => *rect,
            _ => unreachable!(),
        };
        assert_eq!(
            rect(&items[0]),
            [20.0, 632.0, 200.0, 80.0],
            "bottom left: about (0, 768)"
        );
        assert_eq!(
            rect(&items[1]),
            [776.0, 20.0, 200.0, 80.0],
            "top right: about (1024, 0)"
        );
    }

    #[test]
    fn a_hitmarker_shows_after_an_own_hit_and_fades() {
        let cross = || DrawItem::AimCross {
            bar: 5.0,
            half_gap: 3.0,
            colour: [255; 4],
        };
        let mut items = vec![
            cross(),
            DrawItem::Fill {
                rect: [0.0; 4],
                colour: [0; 4],
            },
        ];
        let mut spans: Spans = vec![("crosshair", 0..1), ("health", 1..2)];
        let mut assets = mod_assets();
        let ov = HudOverrides {
            hitmarker: Some("@mod:green/assets/crosshair.dtx".into()),
            ..HudOverrides::default()
        };
        let mut s = state();
        s.own_hit_at = Some(s.now_ms - 125);
        apply_overrides(&ov, &s, &mut assets, &mut items, &mut spans, 1.0);
        assert_eq!(spans, vec![("crosshair", 0..2), ("health", 2..3)]);
        assert!(
            matches!(&items[1], DrawItem::Image { tint, .. } if tint[3] == 128),
            "{:?}",
            items[1]
        );
        s.own_hit_at = Some(s.now_ms - HITMARKER_MS);
        let mut items2 = vec![cross()];
        let mut spans2: Spans = vec![("crosshair", 0..1)];
        apply_overrides(&ov, &s, &mut assets, &mut items2, &mut spans2, 1.0);
        assert_eq!(items2.len(), 1, "gone after {HITMARKER_MS} ms");
    }
}
