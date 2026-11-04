//! The help ("?") box: a window over the screen with a close button, an exit button and the rating picture, opened
//! by the top bar's help button (which toggles it) and closed by either button or Escape.
//!
//! The 2009 code builds the box from the layout section of the help box: window texture and position, close button
//! (texture prefix, place, size), exit button (prefix, place, size, caption string and its place) and the picture
//! page. The box stands in the middle of the screen whatever the section's position says.
// EVIDENCE: cshell+0x26b770 (the box build from the help section: window, close button, exit button with its
// caption, picture), cshell+0x26bc30 (shown: the window set to the middle of the screen, (screen - size) / 2),
// cshell+0x1ac4b0 (0x12e: the box toggled)
// OBSERVED: the layout section of the user's ROM (window 673 x 498, close 15 x 15, exit 102 x 25, picture 621 x 354;
// two tab buttons and further rating pictures sit in the section without code reading them under these keys)
// DC-DEFAULT: CLI-558 (proposed) the two tab buttons and the extra rating pictures of the section are not drawn
// (their code is not traced); the exit and close buttons both close the box

use super::catalog::{int, key, pair, Catalog, KeyPin};
use super::sheet::{Caption, Look, Part, Sheet, ALIGN_CENTRE};
use super::{Ctx, Go, MenuData, MenuKey, Screen};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::WidgetKind;

pub const NAME: &str = "help";

/// The help box's layout section and its keys.
// OBSERVED: the user's ROM layout table (sections and keys matched by SHA-256 prefix and length)
pub const SECTION: KeyPin = key("5da2ee9e5d5203da", 7);
pub mod keys {
    use super::{key, KeyPin};
    pub const WND_TEX: KeyPin = key("15be9f59ea299729", 6);
    pub const WND_RECT: KeyPin = key("ea56b73d2f0539de", 7);
    pub const CLOSE_TEX: KeyPin = key("febd9b5913909cc4", 11);
    pub const CLOSE_POS: KeyPin = key("401cb4906700a8dc", 11);
    pub const CLOSE_RECT: KeyPin = key("d847e53f5d4f641d", 12);
    pub const EXIT_TEX: KeyPin = key("f7820eb4c73c2c5b", 10);
    pub const EXIT_POS: KeyPin = key("5ff06510b10acb22", 10);
    pub const EXIT_RECT: KeyPin = key("f0c0c77add1bb7bc", 11);
    pub const EXIT_STRING: KeyPin = key("0bb59ada95e4d916", 15);
    pub const EXIT_STRING_POS: KeyPin = key("914bff83ae7cd602", 16);
    pub const BACK_TEX: KeyPin = key("2c644def23432641", 7);
    pub const BACK_POS: KeyPin = key("1561b9c455140c69", 7);
    pub const BACK_RECT: KeyPin = key("7ce0eec2e9ea5b38", 8);
}

/// The window's and the controls' layer / command numbers.
pub const WINDOW: i32 = 1;
pub const CMD_DISMISS: i32 = 100_000;
pub const CMD_EXIT: i32 = 100_001;
/// The caption font of the exit button (the pages' common font).
// EVIDENCE: cshell+0x26b770 (font index 0xe for the buttons' text)
const FONT: i32 = 14;
/// The virtual screen.
const SCREEN: [i32; 2] = [1024, 768];

pub struct Help {
    sheet: Sheet,
}

impl Help {
    pub fn new(cat: &Catalog, _data: &MenuData) -> Help {
        Help { sheet: build(cat) }
    }
}

/// The box as sheet parts: window, then picture, close button and exit button on it.
pub fn build(cat: &Catalog) -> Sheet {
    let v = |k: &KeyPin| cat.section_value(&SECTION, k);
    let tex = |k: &KeyPin| v(k).unwrap_or_default().to_string();
    let size = pair(v(&keys::WND_RECT)).unwrap_or([673, 498]);
    let at = [(SCREEN[0] - size[0]) / 2, (SCREEN[1] - size[1]) / 2];
    let mut sheet = Sheet::default();
    sheet.parts.push(Part::synthetic(
        WidgetKind::Window,
        WINDOW,
        WINDOW,
        0,
        at,
        size,
        // the window is the top-left part of its texture
        Look::Panel {
            tex: tex(&keys::WND_TEX),
            stretch: false,
        },
    ));
    let on = |sheet: &mut Sheet, mut p: Part| {
        p.parent = Some(0);
        sheet.parts.push(p);
    };
    let rel = |p: [i32; 2]| [at[0] + p[0], at[1] + p[1]];
    let back_size = pair(v(&keys::BACK_RECT)).unwrap_or([621, 354]);
    on(
        &mut sheet,
        Part::synthetic(
            WidgetKind::Image,
            WINDOW,
            10,
            0,
            rel(pair(v(&keys::BACK_POS)).unwrap_or([27, 84])),
            back_size,
            Look::Panel {
                tex: tex(&keys::BACK_TEX),
                stretch: false,
            },
        ),
    );
    on(
        &mut sheet,
        Part::synthetic(
            WidgetKind::Button,
            WINDOW,
            11,
            CMD_DISMISS,
            rel(pair(v(&keys::CLOSE_POS)).unwrap_or([638, 22])),
            pair(v(&keys::CLOSE_RECT)).unwrap_or([15, 15]),
            Look::Button {
                prefix: tex(&keys::CLOSE_TEX),
                caption: None,
                frame: None,
            },
        ),
    );
    let exit_at = pair(v(&keys::EXIT_STRING_POS)).unwrap_or([51, 12]);
    on(
        &mut sheet,
        Part::synthetic(
            WidgetKind::Button,
            WINDOW,
            12,
            CMD_EXIT,
            rel(pair(v(&keys::EXIT_POS)).unwrap_or([548, 450])),
            pair(v(&keys::EXIT_RECT)).unwrap_or([102, 25]),
            Look::Button {
                prefix: tex(&keys::EXIT_TEX),
                caption: Some(Caption {
                    string_id: int(v(&keys::EXIT_STRING)).unwrap_or(0),
                    at: exit_at,
                    align: Align {
                        first: ALIGN_CENTRE,
                        second: ALIGN_SECOND_CENTRE,
                    },
                    font: FONT,
                    size: 0,
                    colours: [0xffff_ffff; 4],
                }),
                frame: None,
            },
        ),
    );
    sheet
}

impl Screen for Help {
    fn name(&self) -> &'static str {
        NAME
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }
    fn shows_under(&self) -> bool {
        true
    }
    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        if k == MenuKey::Escape {
            self.click(CMD_DISMISS, usize::MAX, ctx);
        }
    }
    fn click(&mut self, cmd: i32, _part: usize, ctx: &mut Ctx) {
        if cmd == CMD_DISMISS || cmd == CMD_EXIT {
            *ctx.go = Some(Go::Back);
        }
    }
}
