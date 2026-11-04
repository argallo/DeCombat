//! shop-polish2: the Modification screen's detail panel (the selected shop cell or the picked option of the
//! storage): frame, the item's detail picture, name, class tag, LOCKED / rank badge / PERMANENT, the Function line
//! or the item's modifier rows, and the description under the frame.
// EVIDENCE: cshell+0x15b800 (the screen builds the panel through cshell+0xd5250 and places it at (0x28c, 0x1c2)),
// cshell+0xd5250 (frame Tunning_Detail_Info_Panel 0x15f x 0x7f; picture 0x100 x 0x80 at (0, 0); class tag at
// (5, 0x12); name font 0xe size 0xe at (5, 2); description font 0x17 size 0xe in 0xffaaaaaa at (9, 0x89); the
// Function list at (0xba, -10), the modifier list at (0xba, 0x2b); LOCKED 0x2b x 7, rank badge 0x1e x 0x16 at
// (8, 100), Permenant_Mark 0x41 x 10), cshell+0xd7470 (the fill)

use super::catalog::Catalog;
use super::roomtext::wrap_lines;
use super::sheet::{image_fit, place_text, ALIGN_CENTRE, ALIGN_LEFT};
use super::shop::{
    cell_tag, class_tag, DETAIL_NAME_COLOUR, LOCKED_SIZE, LOCKED_TAG, LOCKED_TINT, PERMANENT_MARK,
};
use super::storage::{ItemArt, STAT_LABELS};
use crate::hud::items::DrawItem;
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};

pub const PANEL_AT: [i32; 2] = [0x28c, 0x1c2];
pub const PANEL_FRAME: (&str, [i32; 2]) = (
    "UI_SCR_Page_Tuning\\Bottom\\Tunning_Detail_Info_Panel.dtx",
    [0x15f, 0x7f],
);
pub const PICTURE_SIZE: [i32; 2] = [0x100, 0x80];
pub const NAME_AT: [i32; 2] = [5, 2];
pub const TAG_AT: [i32; 2] = [5, 0x12];
pub const BADGE_AT: [i32; 2] = [8, 100];
pub const BADGE_SIZE: [i32; 2] = [0x1e, 0x16];
pub const DESC_AT: [i32; 2] = [9, 0x89];
pub const DESC_FONT: i32 = 0x17;
pub const DESC_COLOUR: u32 = 0xffaa_aaaa;
/// The description wraps at the control's width 0x15f in the engine font's extent; DeCombat's measure reads about
/// 7 % narrower (as the shop panel's, CLI-724).
// EVIDENCE: cshell+0xd7470 (the description control's width set to 0x15f before its text)
// DC-DEFAULT: CLI-760 0x15f x 477 / 0x1fe of DeCombat's measure (the shop panel's CLI-724 ratio)
pub const DESC_WRAP: f32 = 0x15f as f32 * 477.0 / 0x1fe as f32;
/// The Function list and the modifier list (panel offsets), their rows 0xaa x 0xf.
pub const FUNCTION_LIST_AT: [i32; 2] = [0xba, -10];
pub const STAT_LIST_AT: [i32; 2] = [0xba, 0x2b];
pub const ROW_SIZE: [i32; 2] = [0xaa, 0xf];
/// The Function row's label.
// EVIDENCE: cshell+0xd7470 (string 0x15fd with the item's FeatureString in 0xff999999, cshell+0x25b560 (record
// +0x520 for types below 0xb, 0x1a and 0x1d))
pub const FUNCTION_TEXT: i32 = 0x15fd;
/// A row's label colour, a raise in blue, a cut in red.
// EVIDENCE: cshell+0xd61d0 (label font 0x17 size 0xe in 0xff999999, the value in the row's colour),
// cshell+0xd7470 (0xff566be9 by default, 0xffc04d4d for a cut)
pub const LABEL_COLOUR: u32 = 0xff99_9999;
pub const RAISE_COLOUR: u32 = 0xff56_6be9;
pub const CUT_COLOUR: u32 = 0xffc0_4d4d;
/// Row pitch and the value's right end in the row.
// EVIDENCE: cshell+0xd4540 (the row's position: label control at the row's x, value control at x + 0x82),
// cshell+0xd61d0 (label align 0x11 = left, value align 0x14 = centred, no width set: the value is centred on x + 0x82),
// cshell+0x2f0430 (the list stacks its rows by their height)
// DC-DEFAULT: CLI-760 rows 18 px apart (OBSERVED shop-guns/cpp/shop_bb171_tuning_c0_p1.png:
// Gunshot Noise / Recoil / Damage at y 496 / 514 / 532; the 3 px gap over the row height 0xf is not traced)
pub const ROW_PITCH: i32 = 18;
/// The value's centre in the row (CLI-831; the reference stack's three equal-width "-50%" values cannot tell centre from right).
pub const VALUE_CENTRE: i32 = 0x82;
/// Description line height.
// DC-DEFAULT: CLI-760 description lines 16 px apart (OBSERVED shop_bb171: glyph rows from y 589 / 605)
pub const DESC_LINE: i32 = 16;

const ALIGN_TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};
const LEFT_MID: Align = Align {
    first: ALIGN_LEFT,
    second: ALIGN_SECOND_CENTRE,
};
const CENTRE_MID: Align = Align {
    first: ALIGN_CENTRE,
    second: ALIGN_SECOND_CENTRE,
};

/// What the panel shows.
pub struct PanelItem<'a> {
    pub name: &'a str,
    pub art: Option<&'a ItemArt>,
    /// The tag of an item without an option class (the cell's type tag).
    pub tag_kind: Option<i32>,
    /// A shop cell: locked for the character (black picture, LOCKED after the tag, the rank badge) and permanent
    /// (its product has no period).
    pub locked: bool,
    pub badge: Option<&'a str>,
    pub permanent: bool,
}

/// Modifier `k`'s value text and colour (`v` in thousandths; `None` for 0).
// EVIDENCE: cshell+0xd7470 (rows for the eighteen values that are not 0; Clip Size and Slot (k 3, 4): a fraction as
// "+%d%%" / "-%d%%" (0x374db5dc / 0x374db5cc), else 0x33c "+%1!d!" / 0x342 "-%1!d!" red; the others 0x494
// "+%1!d!%%" above 0, else 0x495 "-%1!d!%%" red with the value negated)
pub fn stat_value(cat: &Catalog, k: usize, v: i32) -> Option<(String, u32)> {
    if v == 0 {
        return None;
    }
    let form =
        |id: i32, n: i32| super::roomtext::format_message(&cat.text(id), n).replace("%%", "%");
    let whole = v / 1000;
    Some(if k == 3 || k == 4 {
        if v > 0 && v < 1000 {
            (format!("+{}%", v / 10), RAISE_COLOUR)
        } else if v < 0 && v > -1000 {
            (format!("-{}%", -v / 10), CUT_COLOUR)
        } else if v >= 1000 {
            (form(0x33c, whole), RAISE_COLOUR)
        } else {
            (form(0x342, -whole), CUT_COLOUR)
        }
    } else if v > 0 {
        (form(0x494, whole), RAISE_COLOUR)
    } else {
        (form(0x495, -whole), CUT_COLOUR)
    })
}

/// The panel's rows: the Function row when the item has a feature string, else its modifier rows.
// EVIDENCE: cshell+0xd7470 (the Function list is shown when it holds a row; the modifier list only when the Function
// list is empty)
pub fn rows(cat: &Catalog, art: &ItemArt) -> (bool, Vec<(String, String, u32)>) {
    if !art.feature.is_empty() {
        return (
            true,
            vec![(cat.text(FUNCTION_TEXT), art.feature.clone(), LABEL_COLOUR)],
        );
    }
    let v = art
        .stats
        .iter()
        .enumerate()
        .filter_map(|(k, v)| stat_value(cat, k, *v).map(|(t, c)| (cat.text(STAT_LABELS[k]), t, c)))
        .collect();
    (false, v)
}

/// Draws the panel of `item`.
pub fn draw(cat: &mut Catalog, item: &PanelItem, out: &mut Vec<DrawItem>) {
    let at = |p: [i32; 2]| [PANEL_AT[0] + p[0], PANEL_AT[1] + p[1]];
    out.push(image_fit(cat, PANEL_FRAME.0, PANEL_AT, PANEL_FRAME.1));
    if let Some(t) = item
        .art
        .map(|a| a.panel_picture.as_str())
        .filter(|t| !t.is_empty() && cat.tex_size(t).is_some())
    {
        let mut img = image_fit(cat, t, PANEL_AT, PICTURE_SIZE);
        if item.locked {
            if let DrawItem::Image { tint, .. } = &mut img {
                *tint = LOCKED_TINT;
            }
        }
        out.push(img);
    }
    out.push(place_text(
        item.name,
        at(NAME_AT),
        [0, 0],
        ALIGN_TL,
        0xe,
        0xe,
        DETAIL_NAME_COLOUR,
    ));
    // the tag: an option's weapon class, else the item's own tag
    // EVIDENCE: cshell+0xd7470 (the tag image of cshell+0x25a5b0 for the class of cshell+0x25b6c0)
    let tag = item
        .art
        .and_then(|a| a.attach_class)
        .and_then(class_tag)
        .or_else(|| item.tag_kind.and_then(cell_tag));
    let mut below = at([TAG_AT[0], TAG_AT[1]]);
    if let Some((t, size)) = tag {
        out.push(image_fit(cat, t, below, size));
        // LOCKED 2 px after the tag; PERMANENT under it
        // DISASM: cshell+0xd7d31 (LOCKED at the tag's x + its width + 2), cshell+0xd7e2e (PERMANENT at the tag's x,
        // y + its height + 1)
        if item.locked {
            out.push(image_fit(
                cat,
                LOCKED_TAG,
                [below[0] + size[0] + 2, below[1]],
                LOCKED_SIZE,
            ));
        }
        below[1] += size[1] + 1;
    } else if item.locked {
        out.push(image_fit(cat, LOCKED_TAG, below, LOCKED_SIZE));
    }
    if item.locked {
        if let Some(b) = item.badge {
            out.push(image_fit(cat, b, at(BADGE_AT), BADGE_SIZE));
        }
    }
    if item.permanent {
        out.push(image_fit(cat, PERMANENT_MARK.0, below, PERMANENT_MARK.1));
    }
    if let Some(a) = item.art {
        let (function, rows) = rows(cat, a);
        let list = at(if function {
            FUNCTION_LIST_AT
        } else {
            STAT_LIST_AT
        });
        for (k, (label, value, colour)) in rows.iter().enumerate() {
            let y = list[1] + k as i32 * ROW_PITCH;
            out.push(place_text(
                label,
                [list[0], y],
                [0, ROW_SIZE[1]],
                LEFT_MID,
                DESC_FONT,
                0xe,
                LABEL_COLOUR,
            ));
            out.push(place_text(
                value,
                [list[0] + VALUE_CENTRE, y],
                [0, ROW_SIZE[1]],
                CENTRE_MID,
                DESC_FONT,
                0xe,
                *colour,
            ));
        }
        let lines = wrap_lines(&a.description, DESC_WRAP, |s| {
            cat.measure_text(DESC_FONT, 0xe, s)
                .map_or(s.chars().count() as f32 * 7.0, |m| m.0)
        });
        for (k, l) in lines.iter().enumerate() {
            out.push(place_text(
                l,
                at([DESC_AT[0], DESC_AT[1] + k as i32 * DESC_LINE]),
                [0, 0],
                ALIGN_TL,
                DESC_FONT,
                0xe,
                DESC_COLOUR,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrap_width_follows_the_shop_ratio() {
        assert!((DESC_WRAP - 328.3).abs() < 0.1, "{DESC_WRAP}");
    }
}
