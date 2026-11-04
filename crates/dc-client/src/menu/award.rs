//! crate-result-window: the supply-case award window, the one the 2009 client opens when an inbox case (a supply case,
//! a supply crate or a HiSec case) has been opened: its own window art, the awarded item's slot picture with its name
//! and class tag over the shop's slot frame, the "<name> / <period>" line, a line of text by kind and one OK button.
//! For the first second the window shows the closed-case art with the title only and the OK button disabled; then the
//! art changes to the opened window and the parts appear.
//!
//! The window art, the slot frame and the six period forms are literals of the user's `CShell.dll`, read by pin; when
//! any of them is missing (no ROM) the shop falls back to the plain message box naming the item.
// EVIDENCE: cshell+0x1412c0 (opener: builds the window once, cshell+0x285b60(1), the HiSec / crate flags +0x3d / +0x3e,
// then cshell+0x287150 (item, type, award unique id +0x98, days +0x40) or cshell+0x287110 for a GP total +0x44),
// cshell+0x286500 (build: window 0x1db x 0x100 at (0x114, 0x100); title font 0xe size 0xe flags 0x11 0xfff3ab01 at
// (0xe, 9); OK button Solid_Bt A / B / D / C with string 0x42 at (0x33, 0xc), at (0xb7, 0xcf); the slot frame
// 0xfe x 0x47 at (0x75, 0x31) and the picture over it at the same place; the class tag at (0x7b, 0x44); the period line
// font 2 size 0x12 flags 0x40004 0xffa0c3c2 at (0xee, 0x88); the message font 6 size 0xe flags 4 width 300 0xffe8e9a5 at
// (0xee, 0xa4); the name font 6 size 0xe flags 0x11 0xffa0c3c2 at (0x7b, 0x37); a full-screen blind), cshell+0x286d40
// (fill: name = the item record's name; period line by the item's product consume type and the days; message 0x58b, 0x18c5
// HiSec, 0x18f6 crate (wins over HiSec), 0x189a GP; picture = record +0x328; class tag by cshell+0x25a5b0 of the item
// type; title 0x18c4 HiSec / 0x18f7 crate / 0x393 with the matching closed-case art), cshell+0x285b60 (show: the parts
// hidden, the OK button disabled, the blind shown, the tick stamped), cshell+0x285a80 (after 1000 ms: the opened-window
// art, the OK button enabled, the frame, picture, period, message, name and tag shown)
// SPEC: HISEC_2009.md §4.3 (0x32f +0x12 award item, +0x16 GP total, +0x1a days, +0x1e key used)

use std::time::{Duration, Instant};

use dc_rom::PeFile;
use dc_ui_data::names::{read_pinned, Pin};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};

use super::catalog::{read_pinned_wide, Catalog};
use super::sheet::{image, image_fit, place_text, ALIGN_CENTRE, ALIGN_LEFT, BUTTON_LETTER};
use super::shop::{class_tag, CASE_AWARD_TITLE, CRATE_AWARD_TITLE, HISEC_AWARD_TITLE};
use super::storage::Wares;
use crate::hud::items::DrawItem;

/// The literals of the window in the 2009 shell (rva, SHA-256 prefix, length).
pub mod pins {
    use super::Pin;

    const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
        Pin {
            what,
            rva,
            sha16,
            len,
        }
    }

    /// The closed-case window art of a plain supply case, a HiSec case and a supply crate, and the opened window.
    // EVIDENCE: cshell+0x286d40 (the three variants by flag, through the pointers at cshell+0x457c48 / 0x457c4c /
    // 0x457c50), cshell+0x285a80 (the opened art through cshell+0x457c54)
    pub const WINDOWS: [Pin; 3] = [
        pin("award window case", 0x3f6a5c, "66b23e64cea6742e", 48),
        pin("award window hisec", 0x3f6a28, "8247bd777cfebdee", 49),
        pin("award window crate", 0x3f69f0, "d1a98355107b3e2d", 52),
    ];
    pub const OPENED: Pin = pin("award window opened", 0x3f69bc, "b9654610e7501a91", 48);
    /// The shop's slot frame under the picture.
    // EVIDENCE: cshell+0x286500 (through the pointer at cshell+0x457c64)
    pub const SLOT: Pin = pin("award slot frame", 0x3f6904, "8f0a902b8f6d8923", 43);
    /// The period forms (UTF-16): one day, days, one item, items, a single use, permanent; each `%s` the item's name.
    // EVIDENCE: cshell+0x286d40 (swprintf with these forms)
    pub const FORMS: [Pin; 6] = [
        pin("award form day", 0x3f6afc, "74b6bac6bc91b995", 11),
        pin("award form days", 0x3f6ae0, "4caeb414de0ee8b3", 12),
        pin("award form item", 0x3f6b14, "34d9b1623e8ee3f6", 12),
        pin("award form items", 0x3f6b30, "175ddcb12046a808", 13),
        pin("award form once", 0x3f6abc, "ce2257ffcecc521b", 17),
        pin("award form permanent", 0x3f6a9c, "d0e757f231122cf1", 14),
    ];
}

/// Indexes into [`AwardLiterals::forms`].
pub const FORM_DAY: usize = 0;
pub const FORM_DAYS: usize = 1;
pub const FORM_ITEM: usize = 2;
pub const FORM_ITEMS: usize = 3;
pub const FORM_ONCE: usize = 4;
pub const FORM_PERMANENT: usize = 5;

/// The window's literals as read from the user's binary (`None`: absent or not matching).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AwardLiterals {
    pub windows: [Option<String>; 3],
    pub opened: Option<String>,
    pub slot: Option<String>,
    pub forms: [Option<String>; 6],
}

impl AwardLiterals {
    pub fn from_cshell(pe: &PeFile) -> AwardLiterals {
        let r = |p: &Pin| read_pinned(pe, p).ok();
        AwardLiterals {
            windows: pins::WINDOWS.map(|p| r(&p)),
            opened: r(&pins::OPENED),
            slot: r(&pins::SLOT),
            forms: pins::FORMS.map(|p| read_pinned_wide(pe, &p)),
        }
    }

    /// Every literal resolved (the window can be drawn as 2009 did).
    pub fn complete(&self) -> bool {
        self.windows.iter().all(Option::is_some)
            && self.opened.is_some()
            && self.slot.is_some()
            && self.forms.iter().all(Option::is_some)
    }
}

/// Which case was opened: the title, the closed-case art and the message follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Case,
    HiSec,
    Crate,
}

/// The window on the virtual 1024 x 768 screen.
// EVIDENCE: cshell+0x286500 (size from the globals cshell+0x457c74 / 0x457c78 = 0x1db x 0x100, place from
// cshell+0x457c7c / 0x457c80 = (0x114, 0x100))
pub const WINDOW: [i32; 4] = [0x114, 0x100, 0x1db, 0x100];
/// The parts' places in the window, fonts, sizes and colours.
// EVIDENCE: cshell+0x286500 (see the module note)
pub const TITLE_AT: [i32; 2] = [0xe, 9];
pub const TITLE_FONT: i32 = 0xe;
pub const TITLE_SIZE: i32 = 0xe;
pub const TITLE_COLOUR: u32 = 0xfff3_ab01;
pub const PICTURE_AT: [i32; 2] = [0x75, 0x31];
pub const PICTURE_SIZE: [i32; 2] = [0xfe, 0x47];
pub const NAME_AT: [i32; 2] = [0x7b, 0x37];
pub const NAME_FONT: i32 = 6;
pub const NAME_SIZE: i32 = 0xe;
pub const NAME_COLOUR: u32 = 0xffa0_c3c2;
pub const TAG_AT: [i32; 2] = [0x7b, 0x44];
pub const PERIOD_AT: [i32; 2] = [0xee, 0x88];
pub const PERIOD_FONT: i32 = 2;
pub const PERIOD_SIZE: i32 = 0x12;
pub const PERIOD_COLOUR: u32 = 0xffa0_c3c2;
pub const MESSAGE_AT: [i32; 2] = [0xee, 0xa4];
pub const MESSAGE_FONT: i32 = 6;
pub const MESSAGE_SIZE: i32 = 0xe;
pub const MESSAGE_WIDTH: f32 = 300.0;
pub const MESSAGE_COLOUR: u32 = 0xffe8_e9a5;
pub const MESSAGE_LINE: i32 = 16;
pub const BUTTON_AT: [i32; 2] = [0xb7, 0xcf];
/// DC-DEFAULT: CLI-1550 (proposed) the OK button's hit and drawn box is the message box's 0x78 x 0x19 (the 2009
/// button takes its size from the Solid_Bt texture, cshell+0x43e430; clamped to it when drawn)
pub const BUTTON_SIZE: [i32; 2] = [0x78, 0x19];
pub const CAPTION_AT: [i32; 2] = [0x33, 0xc];
pub const CAPTION_COLOUR: u32 = 0xffee_eeee;
/// The OK caption.
pub const OK_TEXT: i32 = 0x42;
/// Time from the opening to the reveal.
// EVIDENCE: cshell+0x285a80 (`1000 < now - stamp`)
pub const REVEAL_MS: u64 = 1000;
/// The message under the period line: any case, a HiSec case, a supply crate (wins over HiSec), a GP award.
// DISASM: cshell+0x286df5 (0x58b), cshell+0x286e1a (0x189a with a GP total), cshell+0x286f41 (0x18c5 when +0x3d),
// cshell+0x286f50 (0x18f6 when +0x3e, after the HiSec test)
pub const MESSAGE_CASE: i32 = 0x58b;
pub const MESSAGE_HISEC: i32 = 0x18c5;
pub const MESSAGE_CRATE: i32 = 0x18f6;
pub const MESSAGE_GP: i32 = 0x189a;
/// Days at or above this read as permanent, as 0 does.
// DISASM: cshell+0x286eb2 (`cmp eax, 0x3e7; jge` -> the permanent form)
pub const PERMANENT_DAYS: u32 = 999;
/// The function item type: a 0-day award of it reads "one time use".
// DISASM: cshell+0x286efc (`cmp ebx, 0x15`)
pub const FUNCTION_TYPE: i32 = 0x15;

/// Fills a `%s` / `%d` form: `%s` the name, `%d` the number.
fn fill(form: &str, name: &str, n: u32) -> String {
    form.replacen("%s", name, 1)
        .replacen("%d", &n.to_string(), 1)
}

/// The period line: `<name> / <n> day(s)`, `<name> / <n> item(s)` for an item whose product counts uses (consume type
/// 2, 3 or 6), the single-use form for a 0-day function item, the permanent form for 0 or 999 days and more; a GP
/// award shows the name alone.
// DISASM: cshell+0x286e27..0x286f37 (product by cshell+0x2510e0 of the item; consume type +0x24 in 2..=3 or 6: count
// forms, `> 1` plural; else 0 or >= 999: function type and 0 -> single use, else permanent; 1 -> day; else days),
// cshell+0x286df0..0x286e22 (GP total: "%s")
// DC-DEFAULT: CLI-1551 (proposed) the item's product is the lowest-numbered catalogue product whose first item row is
// the item (cshell+0x2510e0 picks one of the item's products by a field not ported)
pub fn period_text(
    forms: &[Option<String>; 6],
    name: &str,
    days: u32,
    consume: Option<i32>,
    kind: Option<i32>,
    gp: bool,
) -> Option<String> {
    if gp {
        return Some(name.to_string());
    }
    let f = |i: usize| forms[i].as_deref();
    if matches!(consume, Some(2 | 3 | 6)) {
        let form = if days > 1 {
            f(FORM_ITEMS)?
        } else {
            f(FORM_ITEM)?
        };
        return Some(fill(form, name, days));
    }
    Some(if days == 0 || days >= PERMANENT_DAYS {
        let form = if kind == Some(FUNCTION_TYPE) && days == 0 {
            f(FORM_ONCE)?
        } else {
            f(FORM_PERMANENT)?
        };
        fill(form, name, days)
    } else if days == 1 {
        fill(f(FORM_DAY)?, name, days)
    } else {
        fill(f(FORM_DAYS)?, name, days)
    })
}

/// The window of one opened case.
#[derive(Debug, Clone, PartialEq)]
pub struct AwardBox {
    pub variant: Variant,
    pub item: i32,
    pub title: i32,
    pub name: String,
    /// The item's slot picture (`InventoryIcon`), if the item names one.
    pub picture: Option<String>,
    /// The weapon class tag (image, size).
    pub tag: Option<(&'static str, [i32; 2])>,
    pub period: String,
    pub message: i32,
    pub closed_art: String,
    pub opened_art: String,
    pub slot: String,
    pub reveal_at: Instant,
    pub revealed: bool,
}

impl AwardBox {
    /// The window for award `item` (`days` the reply's +0x1a, `gp` its +0x16 GP total), opened at `now`; `None` when a
    /// literal is missing (the caller shows the plain box).
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        lits: &AwardLiterals,
        wares: &Wares,
        item: i32,
        days: u32,
        gp: u32,
        hisec: bool,
        crate_box: bool,
        now: Instant,
    ) -> Option<AwardBox> {
        if !lits.complete() {
            return None;
        }
        let variant = if hisec {
            Variant::HiSec
        } else if crate_box {
            Variant::Crate
        } else {
            Variant::Case
        };
        let (title, art) = match variant {
            Variant::HiSec => (HISEC_AWARD_TITLE, 1),
            Variant::Crate => (CRATE_AWARD_TITLE, 2),
            Variant::Case => (CASE_AWARD_TITLE, 0),
        };
        let message = if gp != 0 {
            MESSAGE_GP
        } else if crate_box {
            MESSAGE_CRATE
        } else if hisec {
            MESSAGE_HISEC
        } else {
            MESSAGE_CASE
        };
        let name = wares.label_of(item);
        let kind = wares.kind_of(item);
        let consume = wares
            .catalog()
            .and_then(|c| c.first_product_of_item(item))
            .map(|p| p.consume);
        let period = period_text(&lits.forms, &name, days, consume, kind, gp != 0)?;
        Some(AwardBox {
            variant,
            item,
            title,
            name,
            picture: wares.icons_of(item).into_iter().next(),
            tag: kind.and_then(class_tag),
            period,
            message,
            closed_art: lits.windows[art].clone()?,
            opened_art: lits.opened.clone()?,
            slot: lits.slot.clone()?,
            reveal_at: now + Duration::from_millis(REVEAL_MS),
            revealed: false,
        })
    }

    /// Reveals the parts once the second has passed; `true` when this call revealed them.
    pub fn tick(&mut self, now: Instant) -> bool {
        if !self.revealed && now >= self.reveal_at {
            self.revealed = true;
            return true;
        }
        false
    }

    /// The OK button (x, y, w, h) on the screen.
    pub fn button(&self) -> [i32; 4] {
        [
            WINDOW[0] + BUTTON_AT[0],
            WINDOW[1] + BUTTON_AT[1],
            BUTTON_SIZE[0],
            BUTTON_SIZE[1],
        ]
    }

    /// The window's draw items: the blind, the art, the title, the parts once revealed and the OK button (`look`: 0
    /// normal, 1 over, 2 down; disabled until revealed).
    pub fn draw(&self, cat: &mut Catalog, look: usize, out: &mut Vec<DrawItem>) {
        let at = |p: [i32; 2]| [WINDOW[0] + p[0], WINDOW[1] + p[1]];
        if let Some(b) = cat.paths.overlay.blind.clone() {
            out.push(image(&b, [0, 0], [0x400, 0x300]));
        }
        let art = if self.revealed {
            &self.opened_art
        } else {
            &self.closed_art
        };
        out.push(image_fit(
            cat,
            art,
            [WINDOW[0], WINDOW[1]],
            [WINDOW[2], WINDOW[3]],
        ));
        let top_left = Align {
            first: ALIGN_LEFT,
            second: 0,
        };
        out.push(place_text(
            &cat.text(self.title),
            at(TITLE_AT),
            [0, 0],
            top_left,
            TITLE_FONT,
            TITLE_SIZE,
            TITLE_COLOUR,
        ));
        if self.revealed {
            out.push(image_fit(cat, &self.slot, at(PICTURE_AT), PICTURE_SIZE));
            if let Some(p) = self
                .picture
                .as_deref()
                .filter(|p| cat.tex_size(p).is_some())
            {
                out.push(image_fit(cat, p, at(PICTURE_AT), PICTURE_SIZE));
            }
            out.push(place_text(
                &self.name,
                at(NAME_AT),
                [0, 0],
                top_left,
                NAME_FONT,
                NAME_SIZE,
                NAME_COLOUR,
            ));
            if let Some((tex, size)) = self.tag {
                out.push(image_fit(cat, tex, at(TAG_AT), size));
            }
            let centred = Align {
                first: ALIGN_CENTRE,
                second: 0,
            };
            out.push(place_text(
                &self.period,
                at(PERIOD_AT),
                [0, 0],
                centred,
                PERIOD_FONT,
                PERIOD_SIZE,
                PERIOD_COLOUR,
            ));
            let sx = cat.size().0 as f32 / 1024.0;
            let text = cat.text(self.message);
            let atlas = cat
                .assets
                .fonts
                .atlas_sized(MESSAGE_FONT, Some(MESSAGE_SIZE));
            let measure = |s: &str| {
                atlas.as_ref().map_or(s.chars().count() as f32 * 7.0, |a| {
                    a.measure(s) / sx.max(f32::EPSILON)
                })
            };
            for (i, line) in super::popup::wrap_px(&text, MESSAGE_WIDTH, measure)
                .iter()
                .enumerate()
            {
                out.push(place_text(
                    line,
                    [
                        WINDOW[0] + MESSAGE_AT[0],
                        WINDOW[1] + MESSAGE_AT[1] + i as i32 * MESSAGE_LINE,
                    ],
                    [0, 0],
                    centred,
                    MESSAGE_FONT,
                    MESSAGE_SIZE,
                    MESSAGE_COLOUR,
                ));
            }
        }
        let r = self.button();
        let state = if self.revealed { look.min(2) } else { 3 };
        match cat.paths.overlay.solid_button.clone() {
            Some(prefix) => {
                let tex = format!("{prefix}{}.dtx", BUTTON_LETTER[state]);
                out.push(image_fit(cat, &tex, [r[0], r[1]], [r[2], r[3]]));
            }
            None => out.push(DrawItem::Fill {
                rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
                colour: [70, 70, 70, 255],
            }),
        }
        out.push(place_text(
            &cat.text(OK_TEXT),
            [r[0] + CAPTION_AT[0], r[1] + CAPTION_AT[1]],
            [0, 0],
            Align {
                first: ALIGN_CENTRE,
                second: ALIGN_SECOND_CENTRE,
            },
            14,
            13,
            CAPTION_COLOUR,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forms() -> [Option<String>; 6] {
        // synthetic forms of the same shape as the shell's (the real ones are read from the ROM)
        [
            Some("%s - %d d".into()),
            Some("%s - %d ds".into()),
            Some("%s - %d i".into()),
            Some("%s - %d is".into()),
            Some("%s - once".into()),
            Some("%s - perm".into()),
        ]
    }

    fn lits() -> AwardLiterals {
        AwardLiterals {
            windows: [Some("w0".into()), Some("w1".into()), Some("w2".into())],
            opened: Some("w3".into()),
            slot: Some("slot".into()),
            forms: forms(),
        }
    }

    // CLI-1551: the period line by days, consume type and item type
    #[test]
    fn period_line_reads_days_items_once_and_permanent() {
        let f = forms();
        let t = |d, c, k| period_text(&f, "Gun", d, c, k, false).unwrap();
        assert_eq!(t(1, Some(1), Some(0)), "Gun - 1 d");
        assert_eq!(t(7, Some(1), Some(0)), "Gun - 7 ds");
        assert_eq!(t(0, Some(1), Some(0)), "Gun - perm");
        assert_eq!(t(999, Some(1), Some(0)), "Gun - perm");
        assert_eq!(t(1000, None, Some(0)), "Gun - perm");
        assert_eq!(t(0, None, Some(FUNCTION_TYPE)), "Gun - once");
        assert_eq!(t(5, Some(3), Some(FUNCTION_TYPE)), "Gun - 5 is");
        assert_eq!(t(1, Some(6), None), "Gun - 1 i");
        assert_eq!(period_text(&f, "GP", 0, None, None, true).unwrap(), "GP");
        let mut part = f.clone();
        part[FORM_DAYS] = None;
        assert_eq!(period_text(&part, "Gun", 7, None, Some(0), false), None);
    }

    #[test]
    fn build_picks_title_art_and_message_by_flags() {
        let w = Wares::default();
        let now = Instant::now();
        let b = |h, c, gp| AwardBox::build(&lits(), &w, 5, 7, gp, h, c, now).unwrap();
        let case = b(false, false, 0);
        assert_eq!(
            (case.title, case.closed_art.as_str(), case.message),
            (CASE_AWARD_TITLE, "w0", MESSAGE_CASE)
        );
        let hi = b(true, false, 0);
        assert_eq!(
            (hi.title, hi.closed_art.as_str(), hi.message),
            (HISEC_AWARD_TITLE, "w1", MESSAGE_HISEC)
        );
        let cr = b(false, true, 0);
        assert_eq!(
            (cr.title, cr.closed_art.as_str(), cr.message),
            (CRATE_AWARD_TITLE, "w2", MESSAGE_CRATE)
        );
        // a HiSec crate: the HiSec title and art, the crate's message (cshell+0x286f50 tests the crate flag last)
        let both = b(true, true, 0);
        assert_eq!(
            (both.title, both.message),
            (HISEC_AWARD_TITLE, MESSAGE_CRATE)
        );
        assert_eq!(b(false, false, 500).message, MESSAGE_GP);
        assert_eq!(case.period, "#5 - 7 ds");
        assert!(
            AwardBox::build(&AwardLiterals::default(), &w, 5, 7, 0, false, false, now).is_none()
        );
    }

    #[test]
    fn the_parts_appear_after_one_second() {
        let now = Instant::now();
        let mut b = AwardBox::build(&lits(), &Wares::default(), 5, 1, 0, false, true, now).unwrap();
        assert!(!b.tick(now + Duration::from_millis(999)));
        assert!(!b.revealed);
        assert!(b.tick(now + Duration::from_millis(REVEAL_MS)));
        assert!(b.revealed);
        assert!(!b.tick(now + Duration::from_millis(5000)));
    }
}
