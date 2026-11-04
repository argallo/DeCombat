//! The settings page of a local mod (MOD-24): opened by the Settings button of its row on the Mods screen. A colour is
//! a palette with red, green, blue and opacity sliders, a choice a cycler, a bool a toggle, a number a slider. Rows
//! stand under the manifest's group headings and scroll when they do not fit. Reset to defaults and a share code
//! (MOD-26) serve every mod. Values go to the profile at once and reach the mod at the next match load. When the mod's Lua with these values asks for a drawn crosshair, the page previews it through the same code
//! as the HUD, firing a simulated burst.
//!
//! Provenance: ORIGINAL (DeCombat's own page; the 2009 client has no mods).
// DC-DEFAULT: MOD-24 (proposed, project decision) the page, see DC_DEFAULTS.md

use std::collections::BTreeMap;
use std::path::PathBuf;

use dc_mod_lua::{
    colour_text, decode_share_code, encode_share_code, parse_rgba_hex, ModHost, ModOrigin, ModPolicy, ModSetting,
    NumberRange, SettingKind,
};

use super::shopxray::{swatch, PALETTE};
use super::usebox::fill;
use crate::hud::crosshair::DrawnCrosshair;
use crate::hud::items::DrawItem;
use crate::modassets::HudOverrides;
use crate::spread::AimMarkView;

/// Commands of the page: Back; row `k` stepped; slider `i` one step down (`+ 2i`) or up (`+ 2i + 1`); Reset to
/// defaults; Copy code; Paste code; colour cell `c` of row `k` picked.
pub const CMD_PAGE_BACK: i32 = 115_000;
pub const CMD_PAGE_STEP_BASE: i32 = 115_100;
pub const CMD_PAGE_NUDGE_BASE: i32 = 115_500;
pub const CMD_PAGE_RESET: i32 = 115_900;
pub const CMD_PAGE_COPY: i32 = 115_901;
pub const CMD_PAGE_PASTE: i32 = 115_902;
pub const CMD_PAGE_COLOUR_BASE: i32 = 116_000;
pub const CELLS_PER_ROW: i32 = 100;

pub const T_APPLIES: &str = "Changes apply at the next match you load.";
pub const T_PREVIEW: &str = "Preview: a simulated burst of fire";
pub const T_RESET: &str = "Reset to defaults";
pub const T_COPY: &str = "Copy code";
pub const T_PASTE: &str = "Paste code";
pub const T_SHARE: &str = "Share code";
pub const T_COPIED: &str = "Code copied to the clipboard.";
pub const T_APPLIED: &str = "Code applied.";
pub const T_WAS_RESET: &str = "Back to the defaults.";
pub const T_NOTHING_CHANGED: &str = "Nothing changed.";
/// The colour sliders' labels.
pub const CHANNELS: [&str; 4] = ["Red", "Green", "Blue", "Opacity"];

/// Palette cells on the page: 8 x 2.
const CELL: i32 = 22;
const CELL_GAP: i32 = 4;
const COLS: i32 = 8;
pub const ROW_H: i32 = 28;
pub const GROUP_H: i32 = 24;
/// A colour row: its palette, then its four sliders.
const PALETTE_H: i32 = 2 * (CELL + CELL_GAP) + 2;
const CHANNEL_H: i32 = 25;
pub const PREVIEW: [i32; 2] = [320, 320];
/// A row's value column, from its label.
pub const VALUE_DX: i32 = 170;
/// A slider in the value column: step button, track, step button, value.
pub const NUDGE: [i32; 2] = [22, 22];
pub const TRACK_W: i32 = 270;
const TRACK_DX: i32 = VALUE_DX + NUDGE[0] + 6;
/// The height of the part of the page that holds the rows.
pub const VIEW_H: i32 = 560;

/// One slider of the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slider {
    /// The number row `k`.
    Number(usize),
    /// Channel `c` (red, green, blue, opacity) of the colour row `k`.
    Channel(usize, usize),
}

/// The page's state.
#[derive(Debug, Clone)]
pub struct SettingsPage {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    pub decl: Vec<ModSetting>,
    /// The resolved values (stored form), every declared key.
    pub values: BTreeMap<String, String>,
    pub preview: Option<HudOverrides>,
    /// How far the rows are scrolled, in virtual pixels.
    pub scroll: i32,
    /// The slider the pointer holds.
    pub drag: Option<Slider>,
    /// The current values as a share code.
    pub code: String,
    /// The last Copy / Paste / Reset's words, and whether it went wrong.
    pub message: Option<(String, bool)>,
}

/// What the mod's Lua does with these values: its HUD keys (`None`: it did not load).
fn preview_of(id: &str, dir: &std::path::Path, values: &BTreeMap<String, String>) -> Option<HudOverrides> {
    let mut host = ModHost::new();
    host.set_policy(ModPolicy::Modded {
        client_visual_allowed: true,
    });
    host.set_setting_values([(id.to_string(), values.clone())].into_iter().collect());
    host.load_mod_dir_as(ModOrigin::Local, dir).ok()?;
    crate::modassets::hud_overrides(&host).ok()
}

impl SettingsPage {
    pub fn open(m: &crate::mods::LocalMod, stored: Option<&BTreeMap<String, String>>) -> SettingsPage {
        let values = dc_mod_lua::resolve_settings(&m.settings, stored);
        let preview = preview_of(&m.id, &m.dir, &values);
        let code = encode_share_code(&m.id, &m.settings, &values);
        SettingsPage {
            scroll: 0,
            drag: None,
            code,
            message: None,
            id: m.id.clone(),
            name: m.name.clone(),
            dir: m.dir.clone(),
            decl: m.settings.clone(),
            values,
            preview,
        }
    }

    /// Row `k` steps to its next value (a choice, a bool); `true` when a value changed.
    pub fn step(&mut self, k: usize) -> bool {
        let Some(d) = self.decl.get(k) else {
            return false;
        };
        let cur = self.values.get(&d.key).cloned().unwrap_or_default();
        let next = match &d.kind {
            SettingKind::Bool => (cur != "true").to_string(),
            SettingKind::Choice(o) => {
                let at = o.iter().position(|x| x.value == cur).unwrap_or(0);
                o[(at + 1) % o.len()].value.clone()
            }
            SettingKind::Colour | SettingKind::Number(_) => return false,
        };
        self.set(k, next)
    }

    /// Colour row `k` takes palette cell `cell`; `true` when a value changed.
    pub fn pick(&mut self, k: usize, cell: usize) -> bool {
        let (Some(d), Some(c)) = (self.decl.get(k), PALETTE.get(cell)) else {
            return false;
        };
        if !matches!(d.kind, SettingKind::Colour) {
            return false;
        }
        // a preset keeps the opacity the player set
        let alpha = self.colour_of(k).map_or(255, |v| v[3]);
        self.set(k, colour_text([c[0], c[1], c[2], alpha]))
    }

    fn colour_of(&self, k: usize) -> Option<[u8; 4]> {
        self.values.get(&self.decl.get(k)?.key).and_then(|v| parse_rgba_hex(v))
    }

    /// Every slider, in the order of its commands.
    pub fn sliders(&self) -> Vec<Slider> {
        let mut out = Vec::new();
        for (k, d) in self.decl.iter().enumerate() {
            match d.kind {
                SettingKind::Number(_) => out.push(Slider::Number(k)),
                SettingKind::Colour => out.extend((0..4).map(|c| Slider::Channel(k, c))),
                _ => {}
            }
        }
        out
    }

    /// The slider's grid: `(index of its value, largest index)`.
    pub fn slider_value(&self, sl: Slider) -> (u32, u32) {
        match sl {
            Slider::Number(k) => {
                let (d, r) = self.number(k);
                let v = self.values.get(&d.key).and_then(|t| t.parse().ok());
                (v.and_then(|x| r.index(x)).unwrap_or(0), r.top())
            }
            Slider::Channel(k, c) => (self.colour_of(k).map_or(0, |v| v[c] as u32), 255),
        }
    }

    fn number(&self, k: usize) -> (&ModSetting, NumberRange) {
        let d = &self.decl[k];
        match d.kind {
            SettingKind::Number(r) => (d, r),
            _ => unreachable!("Slider::Number is built from a number row"),
        }
    }

    /// The slider takes grid index `n`; `true` when a value changed.
    pub fn set_slider(&mut self, sl: Slider, n: u32) -> bool {
        match sl {
            Slider::Number(k) => {
                let (_, r) = self.number(k);
                self.set(k, r.at(n))
            }
            Slider::Channel(k, c) => {
                let Some(mut v) = self.colour_of(k) else {
                    return false;
                };
                v[c] = n.min(255) as u8;
                self.set(k, colour_text(v))
            }
        }
    }

    /// One step down or up.
    pub fn nudge(&mut self, i: usize, up: bool) -> bool {
        let Some(sl) = self.sliders().get(i).copied() else {
            return false;
        };
        let (n, top) = self.slider_value(sl);
        let next = if up { (n + 1).min(top) } else { n.saturating_sub(1) };
        self.set_slider(sl, next)
    }

    /// The slider follows the pointer's x.
    pub fn drag_to(&mut self, x: i32, sl: Slider, px: f32) -> bool {
        let (_, top) = self.slider_value(sl);
        let track = (x + TRACK_DX) as f32;
        let f = ((px - track) / TRACK_W as f32).clamp(0.0, 1.0);
        self.set_slider(sl, (f * top as f32).round() as u32)
    }

    /// The slider whose track holds `p`, on a row in view.
    pub fn slider_at(&self, x: i32, top: i32, p: [f32; 2]) -> Option<Slider> {
        self.sliders().into_iter().find(|sl| {
            let r = self.track_rect(x, top, *sl);
            self.in_view(top, r[1], r[3])
                && p[0] >= (r[0] - 4) as f32
                && p[0] < (r[0] + r[2] + 4) as f32
                && p[1] >= r[1] as f32
                && p[1] < (r[1] + r[3]) as f32
        })
    }

    /// The slider's track, `[x, y, w, h]` (its full height, a row's value box).
    pub fn track_rect(&self, x: i32, top: i32, sl: Slider) -> [i32; 4] {
        let y = match sl {
            Slider::Number(k) => self.row_y(top, k),
            Slider::Channel(k, c) => self.row_y(top, k) + PALETTE_H + c as i32 * CHANNEL_H,
        };
        [x + TRACK_DX, y, TRACK_W, NUDGE[1]]
    }

    /// Everything back to the manifest's defaults; `true` when a value changed.
    pub fn reset(&mut self) -> bool {
        let values = dc_mod_lua::resolve_settings(&self.decl, None);
        let changed = self.replace(values);
        self.message = Some((T_WAS_RESET.to_string(), false));
        changed
    }

    /// Takes a pasted share code. A bad one leaves every value as it was and says so; `true` when a value changed.
    pub fn import(&mut self, code: &str) -> bool {
        match decode_share_code(&self.id, &self.decl, code) {
            Ok(values) => {
                let changed = self.replace(values);
                self.message = Some((T_APPLIED.to_string(), false));
                changed
            }
            Err(e) => {
                self.message = Some((format!("{e} {T_NOTHING_CHANGED}"), true));
                false
            }
        }
    }

    fn replace(&mut self, values: BTreeMap<String, String>) -> bool {
        if self.values == values {
            return false;
        }
        self.values = values;
        self.changed();
        true
    }

    fn changed(&mut self) {
        self.preview = preview_of(&self.id, &self.dir, &self.values);
        self.code = encode_share_code(&self.id, &self.decl, &self.values);
    }

    /// The largest scroll: what the rows need over [`VIEW_H`].
    pub fn max_scroll(&self) -> i32 {
        (self.row_y_at(0, self.decl.len(), 0) - VIEW_H).max(0)
    }

    pub fn scroll_by(&mut self, notches: i32) {
        self.scroll = (self.scroll - notches * ROW_H).clamp(0, self.max_scroll());
    }

    /// A strip at `y` of height `h` lies wholly in the part of the page that holds the rows.
    pub fn in_view(&self, top: i32, y: i32, h: i32) -> bool {
        y >= top && y + h <= top + VIEW_H
    }

    /// Row `k` starts a new group: its heading.
    pub fn group_of(&self, k: usize) -> Option<&str> {
        self.decl[k].group.as_deref().filter(|g| {
            k == 0 || self.decl[k - 1].group.as_deref() != Some(*g)
        })
    }

    fn set(&mut self, k: usize, v: String) -> bool {
        let key = self.decl[k].key.clone();
        if self.values.get(&key) == Some(&v) {
            return false;
        }
        self.values.insert(key, v);
        self.changed();
        true
    }

    /// The y of row `k`, its heading above it, scrolled.
    pub fn row_y(&self, top: i32, k: usize) -> i32 {
        let mut y = self.row_y_at(top, k, self.scroll);
        if self.group_of(k).is_some() {
            y += GROUP_H;
        }
        y
    }

    /// Where row `k`'s strip starts, its heading included (at `k` = the count: where the rows end).
    fn row_y_at(&self, top: i32, k: usize, scroll: i32) -> i32 {
        let mut y = top - scroll;
        for (i, d) in self.decl[..k].iter().enumerate() {
            y += ROW_H;
            if self.group_of(i).is_some() {
                y += GROUP_H;
            }
            if matches!(d.kind, SettingKind::Colour) {
                y += PALETTE_H + 4 * CHANNEL_H - ROW_H;
            }
        }
        y
    }

    /// Cell `c` of the palette of a colour row at `[x, y]`.
    pub fn cell_rect(x: i32, y: i32, c: usize) -> [i32; 4] {
        let c = c as i32;
        [
            x + VALUE_DX + (c % COLS) * (CELL + CELL_GAP),
            y + (c / COLS) * (CELL + CELL_GAP),
            CELL,
            CELL,
        ]
    }

    /// What a press at `p` picked: `(row, cell)`.
    pub fn cell_at(&self, x: i32, top: i32, p: [f32; 2]) -> Option<(usize, usize)> {
        for (k, d) in self.decl.iter().enumerate() {
            if !matches!(d.kind, SettingKind::Colour) {
                continue;
            }
            let y = self.row_y(top, k);
            for c in 0..PALETTE.len() {
                let r = Self::cell_rect(x, y, c);
                if self.in_view(top, r[1], r[3])
                    && p[0] >= r[0] as f32
                    && p[1] >= r[1] as f32
                    && p[0] < (r[0] + r[2]) as f32
                    && p[1] < (r[1] + r[3]) as f32
                {
                    return Some((k, c));
                }
            }
        }
        None
    }

    /// The shown text of row `k`'s value.
    pub fn value_label(&self, k: usize) -> String {
        let d = &self.decl[k];
        let v = self.values.get(&d.key).map(String::as_str).unwrap_or("");
        match &d.kind {
            SettingKind::Bool => if v == "true" { "On" } else { "Off" }.to_string(),
            SettingKind::Choice(o) => o
                .iter()
                .find(|x| x.value == v)
                .map_or_else(|| v.to_string(), |x| x.label.clone()),
            SettingKind::Colour | SettingKind::Number(_) => v.to_string(),
        }
    }

    /// The tracks, fills and thumbs of every slider in view.
    pub fn draw_sliders(&self, x: i32, top: i32, out: &mut Vec<DrawItem>) {
        for sl in self.sliders() {
            let [tx, ty, tw, th] = self.track_rect(x, top, sl);
            if !self.in_view(top, ty, th) {
                continue;
            }
            let (n, max) = self.slider_value(sl);
            let at = (tw as f32 * n as f32 / max.max(1) as f32).round() as i32;
            let mid = ty + th / 2;
            out.push(fill([tx, mid - 2], [tw, 4], [12, 14, 16, 255]));
            out.push(fill([tx, mid - 2], [at, 4], [0xfe, 0xad, 0x00, 0xff]));
            let hot = self.drag == Some(sl);
            let thumb = if hot { [0xff, 0xd8, 0x70, 0xff] } else { [0xd8, 0xd8, 0xd8, 0xff] };
            out.push(fill([tx + at - 3, ty + 3], [6, th - 6], thumb));
        }
    }

    /// The shown text of a slider's value.
    pub fn slider_text(&self, sl: Slider) -> String {
        match sl {
            Slider::Number(k) => self.values.get(&self.decl[k].key).cloned().unwrap_or_default(),
            Slider::Channel(k, c) => {
                let v = self.colour_of(k).map_or(0, |v| v[c] as u32);
                if c == 3 {
                    format!("{}%", (v * 100 + 127) / 255)
                } else {
                    v.to_string()
                }
            }
        }
    }

    /// Palette cells of every colour row, its current value outlined.
    pub fn draw_palettes(&self, x: i32, top: i32, out: &mut Vec<DrawItem>) {
        for (k, d) in self.decl.iter().enumerate() {
            if !matches!(d.kind, SettingKind::Colour) {
                continue;
            }
            let y = self.row_y(top, k);
            let cur = self
                .values
                .get(&d.key)
                .and_then(|v| parse_rgba_hex(v));
            for (c, col) in PALETTE.iter().enumerate() {
                let r = Self::cell_rect(x, y, c);
                if !self.in_view(top, r[1], r[3]) {
                    continue;
                }
                if cur.is_some_and(|v| v[..3] == col[..3]) {
                    out.push(fill([r[0] - 2, r[1] - 2], [r[2] + 4, r[3] + 4], [0xfe, 0xad, 0x00, 0xff]));
                }
                swatch(r, *col, out);
            }
            if let Some(v) = cur.filter(|_| self.in_view(top, y, PALETTE_H)) {
                swatch([x + VALUE_DX + COLS * (CELL + CELL_GAP) + 8, y, 44, 2 * CELL + CELL_GAP], v, out);
            }
        }
    }

    /// The drawn crosshair of the preview at clock `secs`, `None` when the mod's keys ask for no drawn crosshair.
    pub fn preview_item(&self, centre: [f32; 2], secs: f32) -> Option<DrawnCrosshair> {
        let ov = self.preview.as_ref()?;
        let style = ov.crosshair_style?;
        let mut c = DrawnCrosshair::of_shape(
            style,
            &simulated_mark(secs, ov.crosshair_colour.unwrap_or([0, 255, 0, 255])),
            &ov.cross_look(),
        );
        c.centre = Some(centre);
        Some(c)
    }
}

/// A burst every 3 s: a shot each 0.1 s for 1 s while the distance climbs, then its decay. Numbers of our own for the
/// preview, not a weapon's.
pub fn simulated_mark(secs: f32, colour: [u8; 4]) -> AimMarkView {
    const REST: f32 = 5.0;
    const TOP: f32 = 45.0;
    let t = secs.rem_euclid(3.0);
    let (f, since) = if t < 1.0 {
        (t, Some(t.rem_euclid(0.1)))
    } else {
        ((1.0 - (t - 1.0) / 1.5).max(0.0), None)
    };
    AimMarkView {
        distance: REST + (TOP - REST) * f,
        since_shot_s: since,
        colour,
        rest: REST,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(json: &str) -> SettingsPage {
        let decl = dc_mod_lua::parse_settings(Some(&dc_mod_lua::manifest::JsonValue::parse(json).unwrap())).unwrap();
        let values = dc_mod_lua::resolve_settings(&decl, None);
        let code = encode_share_code("m", &decl, &values);
        SettingsPage {
            id: "m".into(),
            name: "M".into(),
            dir: PathBuf::from("no such folder"),
            decl,
            values,
            preview: None,
            scroll: 0,
            drag: None,
            code,
            message: None,
        }
    }

    #[test]
    fn rows_that_do_not_fit_scroll_and_leave_no_hit_outside_the_view() {
        let rows: Vec<String> = (0..16)
            .map(|i| format!(r##"{{"key":"c{i}","label":"C","group":"G{i}","type":"colour","default":"#102030"}}"##))
            .collect();
        let mut p = page(&format!("[{}]", rows.join(",")));
        assert!(p.max_scroll() > 0);
        p.scroll_by(1);
        assert_eq!(p.scroll, 0, "not above the top");
        p.scroll_by(-1000);
        assert_eq!(p.scroll, p.max_scroll(), "not past the end");
        let last = Slider::Channel(15, 3);
        let r = p.track_rect(0, 100, last);
        assert!(p.in_view(100, r[1], r[3]), "the end comes into view: {r:?}");
        assert_eq!(p.slider_at(0, 100, [r[0] as f32 + 5.0, r[1] as f32 + 5.0]), Some(last));
        // what scrolled away takes no press
        let first = p.track_rect(0, 100, Slider::Channel(0, 0));
        assert!(!p.in_view(100, first[1], first[3]));
        assert_eq!(p.slider_at(0, 100, [first[0] as f32 + 5.0, first[1] as f32 + 5.0]), None);
    }

    #[test]
    fn a_short_list_does_not_scroll() {
        let mut p = page(r#"[{"key":"n","label":"N","type":"number","min":0,"max":4,"step":0.5,"default":1}]"#);
        p.scroll_by(-3);
        assert_eq!(p.scroll, 0);
        assert_eq!(p.slider_value(Slider::Number(0)), (2, 8));
        assert!(p.nudge(0, true));
        assert_eq!(p.values["n"], "1.5");
        assert!(p.drag_to(0, Slider::Number(0), (TRACK_DX + TRACK_W) as f32));
        assert_eq!(p.values["n"], "4");
        assert!(!p.nudge(0, true), "at the top");
        assert!(!p.import("bad") && p.message.as_ref().unwrap().1);
        assert_eq!(p.values["n"], "4");
    }
}
