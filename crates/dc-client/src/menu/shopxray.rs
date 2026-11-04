//! CLI-1628: the shop page's x-ray controls next to the player's name: a style cycler (`Wallhack: Box / Chams /
//! Skeleton`, a click cycles) and a colour pair (the two tones of the current style), each swatch opening a small
//! palette of fixed colours. They write the player's x-ray preference (`settings.xray`, CLI-1624), which the shop's
//! character preview (CLI-1629) and the match's x-ray pass of a declared x-ray server draw with. They exist on every
//! lobby: they only set a client preference.
//!
//! CLI-1630: a third row above them holds the two switches `Nametags: On / Off` and `Health: On / Off` (each half
//! of the row is a click target), the x-ray's name tags and health bars (`settings.xray.nametags` / `.health`).
//!
//! CLI-1635: a fourth row above those, `Teammates: On / Off` (`settings.xray.teammates`).
//!
//! Drawn in the ROM's widget style: each row is the drop-down field frame (the buy window's period field, nine parts)
//! with the buy window's arrow on the cycler; the palette popup is the same frame around a 4 x 4 grid.
//!
//! Provenance: ORIGINAL (DeCombat's own control; the 2009 shop has no x-ray settings).
// DC-DEFAULT: CLI-1628 (proposed) the controls' place (right of the nickname box, two rows of its height), the click
// cycle, the 16-colour palette
// DC-DEFAULT: CLI-1630 (proposed, USER REQUEST 2026-11-01) the switch row above the cycler, one click target per half

use super::buybox::BuyArt;
use super::catalog::Catalog;
use super::sheet::{image_fit, place_text, ALIGN_LEFT};
use crate::hud::items::DrawItem;
use crate::settings::{XrayMode, XrayStyle};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};

/// Gap between the nickname box and the controls (virtual pixels).
pub const GAP: i32 = 6;
/// Width of the controls.
pub const WIDTH: i32 = 192;
/// Row height and the space between the two rows.
pub const ROW_H: i32 = 16;
pub const ROW_GAP: i32 = 2;
/// A swatch's size.
pub const SWATCH: [i32; 2] = [24, 12];
/// The cycler's arrow (the buy window's 15 x 15 arrow).
pub const ARROW: [i32; 2] = [15, 15];
/// Palette cells: 4 x 4, each `CELL` square with `CELL_GAP` between, inside a `PAD` border.
pub const CELL: i32 = 16;
pub const CELL_GAP: i32 = 3;
pub const PAD: i32 = 6;
pub const COLS: i32 = 4;

/// The palette's colours (RGBA, opaque; the box fill takes a third of the alpha by itself).
pub const PALETTE: [[u8; 4]; 16] = [
    [0xff, 0x30, 0x30, 0xff],
    [0xff, 0xff, 0xff, 0xff],
    [0x00, 0x00, 0x00, 0xff],
    [0xff, 0xff, 0x00, 0xff],
    [0xff, 0x80, 0x00, 0xff],
    [0x00, 0xff, 0x00, 0xff],
    [0x00, 0x80, 0x00, 0xff],
    [0x00, 0xff, 0xff, 0xff],
    [0x00, 0x40, 0xff, 0xff],
    [0x80, 0xc0, 0xff, 0xff],
    [0xff, 0x00, 0xff, 0xff],
    [0x80, 0x00, 0xff, 0xff],
    [0xff, 0x80, 0xc0, 0xff],
    [0x80, 0x40, 0x00, 0xff],
    [0x80, 0x80, 0x80, 0xff],
    [0xc0, 0xc0, 0xc0, 0xff],
];

const FONT: i32 = 14;
const FONT_SIZE: i32 = 12;
const TEXT_COLOUR: u32 = 0xffdd_dddd;
const HOT_COLOUR: u32 = 0xfffe_ad00;

/// The style's name on the cycler.
pub fn mode_label(m: XrayMode) -> &'static str {
    match m {
        XrayMode::Box => "Box",
        XrayMode::Chams => "Chams",
        XrayMode::Skeleton => "Skeleton",
    }
}

/// The names of the two tones of a style: box fill / border, chams front / rim, skeleton bones / joints.
pub fn tone_labels(m: XrayMode) -> [&'static str; 2] {
    match m {
        XrayMode::Box => ["Fill", "Border"],
        XrayMode::Chams => ["Front", "Rim"],
        XrayMode::Skeleton => ["Bones", "Joints"],
    }
}

/// CLI-1630: the switch row's two labels.
pub fn toggle_labels(style: &XrayStyle) -> [String; 2] {
    let on = |b: bool| if b { "On" } else { "Off" };
    [
        format!("Nametags: {}", on(style.nametags)),
        format!("Health: {}", on(style.health)),
    ]
}

/// What a press on the controls did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrayPress {
    /// Not on the controls (nor on an open palette).
    Miss,
    /// Taken, nothing changed (a palette opened or closed).
    Taken,
    /// The style changed (save and publish it).
    Changed,
}

/// The controls' state: the open palette's tone (0 primary, 1 secondary).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct XrayControls {
    pub open: Option<usize>,
}

/// The rectangles of the controls, from the nickname box's place and size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XrayRects {
    /// CLI-1630: the switch row (above the cycler) and its two halves: name tags, health bars.
    pub toggles: [i32; 4],
    pub toggle: [[i32; 4]; 2],
    /// CLI-1635: the team mates' switch, a row above them.
    pub mates: [i32; 4],
    pub cycler: [i32; 4],
    pub tones: [i32; 4],
    /// The two swatches.
    pub swatch: [[i32; 4]; 2],
}

fn inside(r: [i32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] as f32
        && p[1] >= r[1] as f32
        && p[0] < (r[0] + r[2]) as f32
        && p[1] < (r[1] + r[3]) as f32
}

impl XrayRects {
    /// The controls right of the nickname box (`pos`, `size`), the two rows centred on its height.
    pub fn of_nick(pos: [i32; 2], size: [i32; 2]) -> XrayRects {
        let x = pos[0] + size[0] + GAP;
        let y = pos[1] + (size[1] - 2 * ROW_H - ROW_GAP) / 2;
        let tones = [x, y + ROW_H + ROW_GAP, WIDTH, ROW_H];
        let sy = tones[1] + (ROW_H - SWATCH[1]) / 2;
        let half = WIDTH / 2;
        let swatch = [0, 1].map(|k| [x + half * k + half - SWATCH[0] - 6, sy, SWATCH[0], SWATCH[1]]);
        let toggles = [x, y - ROW_H - ROW_GAP, WIDTH, ROW_H];
        let toggle = [0, 1].map(|k| [x + half * k, toggles[1], half, ROW_H]);
        let mates = [x, toggles[1] - ROW_H - ROW_GAP, WIDTH, ROW_H];
        XrayRects {
            mates,
            toggles,
            toggle,
            cycler: [x, y, WIDTH, ROW_H],
            tones,
            swatch,
        }
    }

    /// The palette of tone `tone`: above the rows, left edge at its swatch's half.
    pub fn palette(&self, tone: usize) -> [i32; 4] {
        let side = 2 * PAD + COLS * CELL + (COLS - 1) * CELL_GAP;
        let x = self.tones[0] + (WIDTH / 2) * tone as i32;
        [x, self.mates[1] - side - 2, side, side]
    }

    /// Cell `i`'s rectangle in the palette `pal`.
    pub fn cell(pal: [i32; 4], i: usize) -> [i32; 4] {
        let (c, r) = (i as i32 % COLS, i as i32 / COLS);
        [
            pal[0] + PAD + c * (CELL + CELL_GAP),
            pal[1] + PAD + r * (CELL + CELL_GAP),
            CELL,
            CELL,
        ]
    }
}

impl XrayControls {
    /// A press at `p` with the controls at `r`: the cycler steps the mode, a swatch opens (or closes) its palette, a
    /// palette cell sets its tone and closes the palette, a press elsewhere closes an open palette (and is taken).
    pub fn press(&mut self, r: &XrayRects, p: [f32; 2], style: &mut XrayStyle) -> XrayPress {
        let was_open = self.open.is_some();
        if let Some(t) = self.open {
            let pal = r.palette(t);
            self.open = None;
            if let Some(i) = (0..PALETTE.len()).find(|&i| inside(XrayRects::cell(pal, i), p)) {
                let tone = if t == 0 {
                    &mut style.primary
                } else {
                    &mut style.secondary
                };
                if *tone == PALETTE[i] {
                    return XrayPress::Taken;
                }
                *tone = PALETTE[i];
                return XrayPress::Changed;
            }
            // a press on the swatch that opened it only closes it
            if inside(r.swatch[t], p) || inside(pal, p) {
                return XrayPress::Taken;
            }
        }
        if inside(r.cycler, p) {
            style.mode = style.mode.next();
            return XrayPress::Changed;
        }
        if inside(r.toggle[0], p) {
            style.nametags = !style.nametags;
            return XrayPress::Changed;
        }
        if inside(r.toggle[1], p) {
            style.health = !style.health;
            return XrayPress::Changed;
        }
        if inside(r.mates, p) {
            style.teammates = !style.teammates;
            return XrayPress::Changed;
        }
        if let Some(t) = (0..2).find(|&t| inside(r.swatch[t], p)) {
            self.open = Some(t);
            return XrayPress::Taken;
        }
        if was_open {
            XrayPress::Taken
        } else {
            XrayPress::Miss
        }
    }

    /// The controls (and the open palette, drawn last over the pane).
    pub fn draw(
        &self,
        cat: &mut Catalog,
        art: &BuyArt,
        r: &XrayRects,
        style: &XrayStyle,
        pointer: Option<[f32; 2]>,
        out: &mut Vec<DrawItem>,
    ) {
        let left = Align {
            first: ALIGN_LEFT,
            second: ALIGN_SECOND_CENTRE,
        };
        let hot = |rect: [i32; 4]| pointer.is_some_and(|p| inside(rect, p));
        // the cycler: field frame, "Wallhack: <style>", the arrow at its right end
        frame(cat, art, r.cycler, out);
        let label = format!("Wallhack: {}", mode_label(style.mode));
        out.push(place_text(
            &label,
            [r.cycler[0] + 5, r.cycler[1]],
            [r.cycler[2] - ARROW[0] - 8, r.cycler[3]],
            left,
            FONT,
            FONT_SIZE,
            if hot(r.cycler) { HOT_COLOUR } else { TEXT_COLOUR },
        ));
        if let Some(t) = art.arrow.as_deref() {
            let at = [
                r.cycler[0] + r.cycler[2] - ARROW[0] - 1,
                r.cycler[1] + (r.cycler[3] - ARROW[1]) / 2,
            ];
            out.push(image_fit(cat, t, at, ARROW));
        }
        // CLI-1630: the switch row: "Nametags: On / Off", "Health: On / Off"
        frame(cat, art, r.toggles, out);
        for (k, label) in toggle_labels(style).iter().enumerate() {
            let t = r.toggle[k];
            out.push(place_text(
                label,
                [t[0] + 5, t[1]],
                [t[2] - 8, t[3]],
                left,
                FONT,
                FONT_SIZE,
                if hot(t) { HOT_COLOUR } else { TEXT_COLOUR },
            ));
        }
        // CLI-1635: the team mates' switch
        frame(cat, art, r.mates, out);
        out.push(place_text(
            &format!("Teammates: {}", if style.teammates { "On" } else { "Off" }),
            [r.mates[0] + 5, r.mates[1]],
            [r.mates[2] - 8, r.mates[3]],
            left,
            FONT,
            FONT_SIZE,
            if hot(r.mates) { HOT_COLOUR } else { TEXT_COLOUR },
        ));
        // the tone row: per tone its name and a framed swatch
        frame(cat, art, r.tones, out);
        let names = tone_labels(style.mode);
        let half = WIDTH / 2;
        for (k, c) in [style.primary, style.secondary].into_iter().enumerate() {
            let x = r.tones[0] + half * k as i32;
            out.push(place_text(
                names[k],
                [x + 5, r.tones[1]],
                [half - SWATCH[0] - 12, r.tones[3]],
                left,
                FONT,
                FONT_SIZE,
                if hot(r.swatch[k]) || self.open == Some(k) {
                    HOT_COLOUR
                } else {
                    TEXT_COLOUR
                },
            ));
            swatch(r.swatch[k], c, out);
        }
        // the open palette: the frame, 16 cells, the current colour's cell outlined
        if let Some(t) = self.open {
            let pal = r.palette(t);
            frame(cat, art, pal, out);
            let current = if t == 0 { style.primary } else { style.secondary };
            for (i, c) in PALETTE.iter().enumerate() {
                let cell = XrayRects::cell(pal, i);
                if *c == current {
                    outline(grow(cell, 2), [0xfe, 0xad, 0x00, 0xff], out);
                }
                swatch(cell, *c, out);
            }
        }
    }
}

fn rect_f(r: [i32; 4]) -> [f32; 4] {
    [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32]
}

fn grow(r: [i32; 4], by: i32) -> [i32; 4] {
    [r[0] - by, r[1] - by, r[2] + 2 * by, r[3] + 2 * by]
}

/// The drop-down field frame in nine parts (an 80 % black fill without the ROM art).
fn frame(cat: &mut Catalog, art: &BuyArt, r: [i32; 4], out: &mut Vec<DrawItem>) {
    let drawn = match art.field.as_deref() {
        Some(t) => super::nine::draw(cat, t, r, [255; 4], out),
        None => 0,
    };
    if drawn == 0 {
        out.push(DrawItem::Fill {
            rect: rect_f(r),
            colour: [0, 0, 0, 204],
        });
    }
}

/// A 1 px outline of `r`.
fn outline(r: [i32; 4], c: [u8; 4], out: &mut Vec<DrawItem>) {
    let [x, y, w, h] = r;
    for b in [[x, y, w, 1], [x, y + h - 1, w, 1], [x, y, 1, h], [x + w - 1, y, 1, h]] {
        out.push(DrawItem::Fill {
            rect: rect_f(b),
            colour: c,
        });
    }
}

/// A colour swatch: a dark 1 px border around the colour (opaque, so black reads on the dark frame too).
pub(crate) fn swatch(r: [i32; 4], c: [u8; 4], out: &mut Vec<DrawItem>) {
    outline(r, [0x90, 0x90, 0x90, 0xff], out);
    out.push(DrawItem::Fill {
        rect: rect_f(grow(r, -1)),
        colour: [c[0], c[1], c[2], 0xff],
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centre(r: [i32; 4]) -> [f32; 2] {
        [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32]
    }

    #[test]
    fn the_cycler_steps_box_chams_skeleton_box() {
        let r = XrayRects::of_nick([22, 585], [223, 33]);
        let mut c = XrayControls::default();
        let mut s = XrayStyle::default();
        let mut seen = vec![s.mode];
        for _ in 0..3 {
            assert_eq!(c.press(&r, centre(r.cycler), &mut s), XrayPress::Changed);
            seen.push(s.mode);
        }
        assert_eq!(
            seen,
            [XrayMode::Box, XrayMode::Chams, XrayMode::Skeleton, XrayMode::Box]
        );
        assert_eq!(c.press(&r, [0.0, 0.0], &mut s), XrayPress::Miss);
    }

    #[test]
    fn a_swatch_opens_its_palette_and_a_cell_sets_that_tone() {
        let r = XrayRects::of_nick([22, 585], [223, 33]);
        let mut c = XrayControls::default();
        let mut s = XrayStyle::default();
        assert_eq!(c.press(&r, centre(r.swatch[1]), &mut s), XrayPress::Taken);
        assert_eq!(c.open, Some(1));
        let cell = XrayRects::cell(r.palette(1), 5);
        assert_eq!(c.press(&r, centre(cell), &mut s), XrayPress::Changed);
        assert_eq!(s.secondary, PALETTE[5]);
        assert_eq!(s.primary, crate::settings::XRAY_PRIMARY_DEFAULT);
        assert_eq!(c.open, None);
        // a press elsewhere closes an open palette without a change
        c.press(&r, centre(r.swatch[0]), &mut s);
        assert_eq!(c.press(&r, [0.0, 0.0], &mut s), XrayPress::Taken);
        assert_eq!(c.open, None);
    }

    #[test]
    fn the_controls_lie_right_of_the_nickname_box() {
        let r = XrayRects::of_nick([22, 585], [223, 33]);
        assert_eq!(r.cycler[0], 22 + 223 + GAP);
        assert!(r.cycler[1] >= 585 && r.tones[1] + ROW_H <= 585 + 33 + 1);
        // the switch row sits right above the cycler, its halves side by side
        assert_eq!(r.toggles[1] + ROW_H + ROW_GAP, r.cycler[1]);
        assert_eq!(r.toggle[0][0] + r.toggle[0][2], r.toggle[1][0]);
        // the palette stays above the rows, inside the left pane's width
        let p = r.palette(1);
        assert!(p[1] + p[3] <= r.toggles[1] && p[0] + p[2] <= 460, "{p:?}");
    }

    #[test]
    fn the_switches_toggle_nametags_and_health() {
        let r = XrayRects::of_nick([22, 585], [223, 33]);
        let mut c = XrayControls::default();
        let mut s = XrayStyle::default();
        assert_eq!(toggle_labels(&s), ["Nametags: Off", "Health: Off"]);
        assert_eq!(c.press(&r, centre(r.toggle[0]), &mut s), XrayPress::Changed);
        assert!(s.nametags && !s.health);
        assert_eq!(c.press(&r, centre(r.toggle[1]), &mut s), XrayPress::Changed);
        assert!(s.nametags && s.health);
        assert_eq!(toggle_labels(&s), ["Nametags: On", "Health: On"]);
        assert_eq!(c.press(&r, centre(r.toggle[0]), &mut s), XrayPress::Changed);
        assert!(!s.nametags && s.health);
        assert_eq!(s.mode, XrayMode::Box, "the style is untouched");
    }
}
