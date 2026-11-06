//! A screen's controls built from pages of the ROM's front-end layout table (`screen_pages`), with the state
//! the screens change (shown, text, checked, focus, hover, pressed) and their display list.
//!
//! Build order and parenting follow the 2009 client: the last tiled-background record of the pages wins; windows
//! come first, then the other controls kind by kind (images, buttons, edit boxes, texts, lists, check buttons);
//! a control whose parent number names a window sits in that window (positions relative to it), every other one
//! sits on the screen. Drawing walks the windows in order, each followed by its own controls, then the
//! controls that sit on the screen.
// SPEC: FMT_CRES_UI.md §6.3 (record schemas, value codes, button texture prefixes + state letter)
// EVIDENCE: cshell+0x1d2670 (page parser), cshell+0x1dbe80 (tiler: `<prefix><row><col>.dtx`, 1..8 rows / cols,
// at most 16 tiles)
// EVIDENCE: cshell+0x1ad200 (creation order by kind), cshell+0x1ae120 (parent -1: on the screen, else into the
// window of that number through vtable +0x74), cshell+0x2f1c70 (caret: on / off every record period)
// DISASM: cshell+0x11ce43 (the login screen pushes its tile prefix with 3 rows, 4 columns to cshell+0x1dbe80)

use super::catalog::{argb, Catalog};
use crate::hud::items::DrawItem;
use crate::hud::layout::TextStyle;
use dc_ui_data::pages::{Align, Widget, ALIGN_SECOND_CENTRE};
use dc_ui_data::{PageTag, ScreenPages, WidgetKind};

/// Alignment codes of the layout (first letter: left 1, right 2, centre 4; second: top 0x10, bottom 0x20).
// SPEC: FMT_CRES_UI.md §6.3 (value code `A`, cshell+0x1c3a70)
pub const ALIGN_LEFT: u32 = 1;
pub const ALIGN_RIGHT: u32 = 2;
pub const ALIGN_CENTRE: u32 = 4;
pub const ALIGN_BOTTOM: u32 = 0x20;

/// The record's draw mode that covers the whole rectangle with the texture (the full-screen dimming window of the
/// room creation page, a 1024 x 768 rectangle with a small texture); other modes draw the texture at its size.
// DC-DEFAULT: CLI-160 (proposed) stretch mode 0 = texture stretched over the rectangle (the mode's 2009 meaning
// is not read)
pub const STRETCH_WHOLE: i32 = 0;

/// Which of a text record's four colours (`C` value) a string control shows in its normal state: the second.
// EVIDENCE: cshell+0x18930 (the controls' colour getter, vtable +0xa8, called by the text render cshell+0x2eb910:
// with none of its state flags set it returns the second of the four colour words, control +0x48)
// EVIDENCE: cshell+0x1b0bd0 (the record's four colour words +0x2c, +0x30, +0x34, +0x38 handed in that order to the
// control's colour setter, vtable +0xb0)
// OBSERVED: lobbyfx4/real/R10_lobby.png, R1_create.png, R20_room.png (2009, 1024 x 768: the lobby
// column titles, the room creation labels and the waiting room's game-setting labels, records with colours
// 170 / 238 / 238 / x, are drawn 238; the waiting room's player-list titles, records with 170 in all four, are 170)
pub const TEXT_NORMAL_COLOUR: usize = 1;

/// The record colour a button caption shows per drawn state (normal, over, down, disabled), as the 2009 colour
/// getter picks it: plain = second colour, over = first, over while pressed = fourth, disabled = third.
// EVIDENCE: cshell+0x18930 (colour getter: a first flag test (vtable +0x8c) -> colour 0; else over (vtable +0xb8)
// -> colour 3 when the pressed flag (vtable +0xb4) is also set, else colour 0; else vtable +0xc4 -> colour 2; else
// colour 1), cshell+0x186f0 (vtable +0xc4 = neither over nor active), cshell+0x186a0 (vtable +0xbc, active = the
// enabled field +0x38 that vtable +0x98 writes (cshell+0x18620) and the shown field +0x3c (cshell+0x18720,
// written by vtable +0x9c)): +0xc4 is the disabled state (CLI-725, amends CLI-463)
// DC-DEFAULT: CLI-463 the getter's first flag (vtable +0x8c) is not modelled
pub const CAPTION_COLOUR: [usize; 4] = [TEXT_NORMAL_COLOUR, 0, 3, 2];
/// The texture letter a button shows per drawn state (normal, over, down, disabled).
// EVIDENCE: cshell+0x188f0 (texture slot: over -> 1, disabled (+0xc4) -> 2, pressed (+0xb4) -> 3, else 0),
// cshell+0x1211b0 (a build hands the slots the textures A, B, D, C of one prefix: disabled = D)
pub const BUTTON_LETTER: [&str; 4] = ["A", "B", "C", "D"];

/// The caret period the boxes of the 2009 pages use (their records give 0.333 s).
// OBSERVED: the edit box records of the user's ROM pages (caret period 0.333, read at run time)
pub const CARET_PERIOD: f32 = 0.333;

/// The width an edit box answers clicks on when its screen gives it no frame of its own.
// DC-DEFAULT: CLI-160 (proposed) edit box hit width 240 virtual pixels, height = font size + 4
pub const EDIT_HIT_WIDTH: i32 = 240;

/// A tiled background: `rows` x `cols` textures `<prefix><row><col>.dtx` laid out from `at` by their own sizes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tiles {
    pub at: [i32; 2],
    pub rows: i32,
    pub cols: i32,
    pub prefix: String,
}

impl Tiles {
    /// The texture name of tile (`row`, `col`), in the tiler's three name forms.
    // EVIDENCE: cshell+0x1dbe80 (`%s.dtx`, `%s%d.dtx` for one row or one column, `%s%d%d.dtx`)
    pub fn name(&self, row: i32, col: i32) -> String {
        let p = &self.prefix;
        match (self.rows < 2, self.cols < 2) {
            (true, true) => format!("{p}.dtx"),
            (true, false) => format!("{p}{col}.dtx"),
            (false, true) => format!("{p}{row}.dtx"),
            (false, false) => format!("{p}{row}{col}.dtx"),
        }
    }

    /// Valid tilings only (1..8 rows and columns, at most 16 tiles).
    // EVIDENCE: cshell+0x1dbe80
    pub fn new(at: [i32; 2], rows: i32, cols: i32, prefix: &str) -> Option<Tiles> {
        let ok = (1..=8).contains(&rows) && (1..=8).contains(&cols) && rows * cols <= 16;
        (ok && !prefix.is_empty()).then(|| Tiles {
            at,
            rows,
            cols,
            prefix: prefix.to_string(),
        })
    }

    pub fn draw(&self, cat: &mut Catalog, out: &mut Vec<DrawItem>) {
        let mut y = self.at[1];
        for r in 0..self.rows {
            let mut x = self.at[0];
            let mut row_h = 0;
            for c in 0..self.cols {
                let name = self.name(r, c);
                let (w, h) = cat.tex_size(&name).unwrap_or((256, 256));
                out.push(image(&name, [x, y], [w as i32, h as i32]));
                x += w as i32;
                row_h = h as i32;
            }
            y += row_h;
        }
    }
}

/// A caption a button draws from the string table.
#[derive(Debug, Clone, PartialEq)]
pub struct Caption {
    pub string_id: i32,
    pub at: [i32; 2],
    pub align: Align,
    pub font: i32,
    pub size: i32,
    /// Normal, over, down, disabled.
    pub colours: [u32; 4],
}

/// How a control looks.
#[derive(Debug, Clone, PartialEq)]
pub enum Look {
    /// Window or image: one texture (`stretch`: drawn over the whole rectangle).
    Panel { tex: String, stretch: bool },
    /// Button: `<prefix>A..D.dtx` (normal, over, down, disabled), an optional caption and an optional caption image
    /// (texture, size; drawn at the text position).
    Button {
        prefix: String,
        caption: Option<Caption>,
        frame: Option<(String, [i32; 2])>,
    },
    /// Text: the part's `text`, aligned in its rectangle.
    Text {
        align: Align,
        font: i32,
        size: i32,
        colour: u32,
    },
    /// List: geometry only; the screens draw the rows. `bar`: the scroll bar of a scrolling list once its screen
    /// gave it rows ([`Sheet::set_list_rows`]); its value moves the rows up.
    List {
        height: i32,
        spacing: i32,
        scroll: bool,
        scroll_at: [i32; 2],
        scroll_len: i32,
        bar: Option<super::scrollbar::ScrollBar>,
    },
    /// Edit box: the part's `text`, masked when `masked`. `clip` > 0 (the run-time chat box, [`super::textbox`]):
    /// the field is `clip` pixels wide, the text's tail that fits is drawn in `text_colour` and the caret is a bar in
    /// `caret_colour` after it.
    Edit {
        font: i32,
        size: i32,
        max_len: usize,
        caret_colour: u32,
        caret_period: f32,
        clip: i32,
        text_colour: u32,
    },
    /// Check button: one texture per state ("unchecked" may name no texture).
    Check { on: String, off: String },
}

/// One control.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub page: PageTag,
    pub kind: WidgetKind,
    /// Windows: their own number; other controls: the parent number of their record (-1 = on the screen).
    pub layer: i32,
    pub id: i32,
    pub command: i32,
    /// Absolute top-left (virtual 1024 x 768).
    pub pos: [i32; 2],
    pub size: [i32; 2],
    pub look: Look,
    pub parent: Option<usize>,
    pub shown: bool,
    pub enabled: bool,
    pub text: String,
    pub checked: bool,
    pub masked: bool,
    pub hover: bool,
    pub down: bool,
    /// Buttons: the screen's selected flag (a page's current tab): the caption takes the record's first colour and
    /// the button its pressed texture.
    // EVIDENCE: cshell+0x185f0 / 0x18600 / 0x18610 (vtable +0x84 / +0x88 / +0x8c: set / clear / read the control's
    // +0x40), cshell+0x18930 (the caption colour getter: +0x8c set -> colour 0), cshell+0x13fd20 (the shop sets it on
    // its current tab 0xc .. 0xf, clears the others), cshell+0x1528d0 (the Modification screen on its tab 10 .. 15)
    // DC-DEFAULT: CLI-762 a selected button draws its C (pressed) texture: the texture slot getter cshell+0x188f0
    // does not read +0x40; OBSERVED shop-guns/cpp/shop_bb100_body_c4_p1.png (the current tab
    // shows 2nd_Tab_C's raised outline)
    pub selected: bool,
    pub focus: bool,
    /// The click area when it differs from `pos` / `size` (edit boxes with a frame).
    pub hit: Option<[i32; 4]>,
    /// Buttons: the record's text position (relative to the button).
    pub text_at: [i32; 2],
    /// The record this control was built from: its index in its page's widget list (the draw trace looks the
    /// record up again, [`super::drawlist`]).
    pub rec: usize,
    /// Text controls: the word-wrap width in virtual pixels (0 = one line), one line item per wrapped line.
    pub wrap: i32,
    /// Wrapped lines' pitch in virtual pixels (0 = the font's line height).
    pub wrap_pitch: i32,
    /// Most wrapped lines drawn (0 = all); a cut text ends its last line with "...".
    // DC-DEFAULT: CLI-1626 (proposed) the waiting room's map description under a Hacks row
    pub wrap_max: i32,
    /// Texts: the screen set the colour at run time (the 2009 screen code hands its own colours to the control's
    /// colour setter, vtable +0xb0), so the record's colours no longer apply (the draw-list check skips COLOUR).
    pub colour_set: bool,
    /// The screen moved the control at run time (the 2009 screen code calls the control's position setter, vtable
    /// +0x20): `pos` and `text_at` hold the new place, which the draw-list check then takes instead of the record's.
    pub placed: bool,
    /// Buttons: the blink period in seconds (0 = no blink). While set, an enabled, unselected button shows its C
    /// (pressed) texture in every other period ([`blink_on`]); the caption keeps its colour.
    // EVIDENCE: cshell+0x33de40 (button draw: not selected, blink phase on, enabled (vtable +0xbc) and blink mode
    // +0x1f0 0 or 2 -> texture slot +0x18c, else slot +0x188 = C; the caption takes the blink colour +0x1ec only in
    // modes 1 / 2), cshell+0x33e430 (the constructor clears the period +0x1e8, mode +0x1f0 and slot +0x18c),
    // cshell+0x33d7f0 (the texture setter fills only the slots +0x17c .. +0x188)
    pub blink: f32,
}

/// The blink phase of a button blinking every `period` seconds at clock `secs`: on in the even periods.
// EVIDENCE: cshell+0x33dc20 (period <= 0 -> off; once `period` seconds passed since the last flip the shared phase
// flips: on and off take one period each)
pub fn blink_on(period: f32, secs: f32) -> bool {
    period > 0.0 && ((secs.max(0.0) / period) as u64).is_multiple_of(2)
}

/// The record index of a control the screen builds at run time (it has no record: the draw trace finds none).
pub const RUNTIME_REC: usize = usize::MAX;

impl Part {
    /// A shown, enabled control the screen builds at run time (no layout record) at absolute `pos`.
    #[allow(clippy::too_many_arguments)]
    pub fn runtime(
        page: PageTag,
        kind: WidgetKind,
        layer: i32,
        id: i32,
        command: i32,
        pos: [i32; 2],
        size: [i32; 2],
        look: Look,
    ) -> Part {
        Part {
            page,
            kind,
            layer,
            id,
            command,
            pos,
            size,
            look,
            parent: None,
            shown: true,
            enabled: true,
            text: String::new(),
            checked: false,
            masked: false,
            hover: false,
            down: false,
            selected: false,
            focus: false,
            hit: None,
            text_at: [0, 0],
            rec: RUNTIME_REC,
            wrap: 0,
            wrap_pitch: 0,
            wrap_max: 0,
            colour_set: false,
            placed: true,
            blink: 0.0,
        }
    }

    pub fn rect(&self) -> [i32; 4] {
        if let Some(h) = self.hit {
            return h;
        }
        match self.look {
            Look::Edit { size, .. } => [self.pos[0], self.pos[1], EDIT_HIT_WIDTH, size + 4],
            _ => [self.pos[0], self.pos[1], self.size[0], self.size[1]],
        }
    }

    pub fn contains(&self, p: [f32; 2]) -> bool {
        let r = self.rect();
        p[0] >= r[0] as f32
            && p[1] >= r[1] as f32
            && p[0] < (r[0] + r[2]) as f32
            && p[1] < (r[1] + r[3]) as f32
    }

    fn clickable(&self) -> bool {
        matches!(self.kind, WidgetKind::Button | WidgetKind::CheckButton)
    }

    /// A control a screen builds in code (no page record: [`Part::rec`] is [`NO_RECORD`]), at the absolute `pos`.
    // w3-options: the option rows, the report box and the help box are built by the 2009 code, not by page records
    pub fn synthetic(
        kind: WidgetKind,
        layer: i32,
        id: i32,
        command: i32,
        pos: [i32; 2],
        size: [i32; 2],
        look: Look,
    ) -> Part {
        Part {
            page: PageTag::Page0,
            kind,
            layer,
            id,
            command,
            pos,
            size,
            look,
            parent: None,
            shown: true,
            enabled: true,
            text: String::new(),
            checked: false,
            masked: false,
            hover: false,
            down: false,
            selected: false,
            focus: false,
            hit: None,
            text_at: [0, 0],
            rec: NO_RECORD,
            wrap: 0,
            wrap_pitch: 0,
            wrap_max: 0,
            colour_set: false,
            placed: false,
            blink: 0.0,
        }
    }
}

/// [`Part::rec`] of a control without a page record.
pub const NO_RECORD: usize = usize::MAX;

/// A screen's controls.
#[derive(Debug, Clone, Default)]
pub struct Sheet {
    pub tiles: Option<Tiles>,
    pub parts: Vec<Part>,
    /// clan-modal: the window layer that takes every press while it is up (a modal web window); other controls are
    /// not hit.
    pub modal: Option<i32>,
    /// The page whose controls a modal window does not lock (the clan screen's lobby bar, CLI-1260).
    pub modal_free: Option<PageTag>,
}

fn text_of(cat: &Catalog, id: i32) -> String {
    if id > 0 {
        cat.text(id)
    } else {
        String::new()
    }
}

/// Kinds in the order the client creates them.
// EVIDENCE: cshell+0x1d2670 (one record table per kind), cshell+0x1ad200 (window, image, button, edit box, text,
// list, the +0x80 table, check button; the +0x80 table's records have no kind here)
const CREATION_ORDER: [WidgetKind; 7] = [
    WidgetKind::Window,
    WidgetKind::Image,
    WidgetKind::Button,
    WidgetKind::EditBox,
    WidgetKind::Text,
    WidgetKind::List,
    WidgetKind::CheckButton,
];

impl Sheet {
    /// The controls of `tags` (in this order, later pages on top) from `pages`.
    pub fn build(pages: &ScreenPages, tags: &[PageTag], cat: &Catalog) -> Sheet {
        let mut sheet = Sheet::default();
        let mut records: Vec<(PageTag, usize, &Widget)> = Vec::new();
        for tag in tags {
            if let Some(p) = pages.page(*tag) {
                for (rec, w) in p.widgets.iter().enumerate() {
                    if let Widget::Lattice(l) = &w.widget {
                        if let Some(t) = Tiles::new(l.pos, l.rows, l.cols, &l.texture) {
                            sheet.tiles = Some(t);
                        }
                    }
                    records.push((*tag, rec, &w.widget));
                }
            }
        }
        for kind in CREATION_ORDER {
            for (tag, rec, w) in records.iter().filter(|(_, _, w)| w.kind() == kind) {
                if let Some(mut part) = sheet.part_of(*tag, w, cat) {
                    part.rec = *rec;
                    sheet.parts.push(part);
                }
            }
        }
        sheet
    }

    pub fn window_index(&self, layer: i32) -> Option<usize> {
        self.parts
            .iter()
            .rposition(|p| p.kind == WidgetKind::Window && p.layer == layer)
    }

    fn part_of(&self, page: PageTag, w: &Widget, cat: &Catalog) -> Option<Part> {
        let blank = |kind, layer, id, command, pos: [i32; 2], size, look| Part {
            page,
            kind,
            layer,
            id,
            command,
            pos,
            size,
            look,
            parent: None,
            shown: true,
            enabled: true,
            text: String::new(),
            checked: false,
            masked: false,
            hover: false,
            down: false,
            selected: false,
            focus: false,
            hit: None,
            text_at: [0, 0],
            rec: 0,
            wrap: 0,
            wrap_pitch: 0,
            wrap_max: 0,
            colour_set: false,
            placed: false,
            blink: 0.0,
        };
        let size_of = |r: [i32; 4]| [r[2] - r[0], r[3] - r[1]];
        let mut part = match w {
            Widget::Window(r) => blank(
                WidgetKind::Window,
                r.control_id,
                r.control_id,
                0,
                r.pos,
                size_of(r.source_rect),
                Look::Panel {
                    tex: r.texture.clone(),
                    stretch: r.stretch_mode == STRETCH_WHOLE,
                },
            ),
            Widget::Image(r) => blank(
                WidgetKind::Image,
                r.parent,
                r.control_id,
                0,
                r.pos,
                size_of(r.source_rect),
                Look::Panel {
                    tex: r.texture.clone(),
                    stretch: r.stretch_mode == STRETCH_WHOLE,
                },
            ),
            Widget::Button(r) => {
                let mut b = blank(
                    WidgetKind::Button,
                    r.parent,
                    r.control_id,
                    r.command_id,
                    r.pos,
                    size_of(r.source_rect),
                    Look::Button {
                        prefix: r.texture_prefix.clone(),
                        caption: (r.text_style > 0).then_some(Caption {
                            string_id: r.text_style,
                            at: r.text_pos,
                            align: r.text_align,
                            font: r.font_index,
                            size: r.font_size,
                            colours: r.text_colours,
                        }),
                        frame: r.frame.clone(),
                    },
                );
                b.text_at = r.text_pos;
                b
            }
            Widget::Text(r) => {
                let mut p = blank(
                    WidgetKind::Text,
                    r.parent,
                    r.control_id,
                    r.command_id,
                    r.pos,
                    size_of(r.rect),
                    Look::Text {
                        align: r.align,
                        font: r.font_index,
                        size: r.font_size,
                        colour: r.colours[TEXT_NORMAL_COLOUR],
                    },
                );
                p.text = text_of(cat, r.text_style);
                p
            }
            Widget::List(r) => blank(
                WidgetKind::List,
                r.parent,
                r.control_id,
                0,
                r.pos,
                [0, r.row_height],
                Look::List {
                    height: r.row_height,
                    spacing: r.item_spacing,
                    scroll: r.scroll != 0,
                    scroll_at: r.scroll_offset,
                    scroll_len: if r.scroll_size > 0 {
                        r.scroll_size
                    } else {
                        r.row_height
                    },
                    bar: None,
                },
            ),
            Widget::EditBox(r) => {
                let mut p = blank(
                    WidgetKind::EditBox,
                    r.parent,
                    r.control_id,
                    0,
                    r.pos,
                    [EDIT_HIT_WIDTH, r.font_size + 4],
                    Look::Edit {
                        font: r.font_index,
                        size: r.font_size,
                        max_len: r.max_length.max(0) as usize,
                        caret_colour: r.caret_colour,
                        caret_period: r.caret_period,
                        clip: 0,
                        text_colour: 0,
                    },
                );
                p.shown = r.visible != 0;
                p
            }
            Widget::CheckButton(r) => blank(
                WidgetKind::CheckButton,
                r.parent,
                r.control_id,
                r.command_id,
                r.pos,
                size_of(r.source_rect),
                Look::Check {
                    on: r.texture_checked.clone(),
                    off: r.texture_unchecked.clone(),
                },
            ),
            _ => return None,
        };
        if part.kind != WidgetKind::Window && part.layer >= 0 {
            // the page's own window first (page numbers repeat across the pages of one screen)
            let own = self.parts.iter().rposition(|p| {
                p.kind == WidgetKind::Window && p.layer == part.layer && p.page == page
            });
            if let Some(wi) = own.or_else(|| self.window_index(part.layer)) {
                let wp = self.parts[wi].pos;
                part.pos = [part.pos[0] + wp[0], part.pos[1] + wp[1]];
                part.parent = Some(wi);
            }
        }
        Some(part)
    }

    /// The control `id` of window `layer` (`layer` -1: a control on the screen) of `kind` (the last page's one
    /// when several pages of the screen have it).
    pub fn find(&self, layer: i32, id: i32, kind: WidgetKind) -> Option<usize> {
        self.parts.iter().rposition(|p| {
            p.kind == kind
                && p.id == id
                && (if kind == WidgetKind::Window {
                    true
                } else {
                    p.layer == layer
                })
        })
    }

    pub fn part(&self, layer: i32, id: i32, kind: WidgetKind) -> Option<&Part> {
        self.find(layer, id, kind).map(|i| &self.parts[i])
    }

    pub fn part_mut(&mut self, layer: i32, id: i32, kind: WidgetKind) -> Option<&mut Part> {
        self.find(layer, id, kind).map(move |i| &mut self.parts[i])
    }

    /// The rectangle (x, y, w, h) of control `id` of window `layer` of any kind (images first, then buttons, texts,
    /// the rest), the place a drop-down list opens under.
    // EVIDENCE: cshell+0x1625f0 (the list's control is found by window and control number alone, vtable +0xac / +0x78)
    pub fn anchor_rect(&self, layer: i32, id: i32) -> Option<[i32; 4]> {
        let rank = |k: WidgetKind| match k {
            WidgetKind::Image => 0,
            WidgetKind::Button => 1,
            WidgetKind::Text => 2,
            _ => 3,
        };
        self.parts
            .iter()
            .filter(|p| p.kind != WidgetKind::Window && p.layer == layer && p.id == id)
            .min_by_key(|p| rank(p.kind))
            .map(|p| [p.pos[0], p.pos[1], p.size[0], p.size[1]])
    }

    /// The first button with `command` (the screens address several buttons by their command).
    pub fn button(&self, command: i32) -> Option<&Part> {
        self.parts
            .iter()
            .find(|p| p.kind == WidgetKind::Button && p.command == command && self.visible_part(p))
    }

    pub fn set_text(&mut self, layer: i32, id: i32, text: &str) {
        if let Some(p) = self.part_mut(layer, id, WidgetKind::Text) {
            p.text = text.to_string();
        }
    }

    /// Draws text control `id` of window `layer` in `colour` (`0xAARRGGBB`) instead of the record's colour.
    pub fn set_colour(&mut self, layer: i32, id: i32, c: u32) {
        if let Some(p) = self.part_mut(layer, id, WidgetKind::Text) {
            if let Look::Text { colour, .. } = &mut p.look {
                *colour = c;
                p.colour_set = true;
            }
        }
    }

    /// Word-wraps text control `id` of window `layer` at `width` virtual pixels.
    pub fn set_wrap(&mut self, layer: i32, id: i32, width: i32) {
        if let Some(p) = self.part_mut(layer, id, WidgetKind::Text) {
            p.wrap = width;
        }
    }

    /// The pitch of text control `id`'s wrapped lines (0 = the font's line height).
    pub fn set_wrap_pitch(&mut self, layer: i32, id: i32, pitch: i32) {
        if let Some(p) = self.part_mut(layer, id, WidgetKind::Text) {
            p.wrap_pitch = pitch;
        }
    }

    pub fn show(&mut self, layer: i32, id: i32, kind: WidgetKind, shown: bool) {
        if let Some(p) = self.part_mut(layer, id, kind) {
            p.shown = shown;
        }
    }

    /// At most `lines` wrapped lines of text control `id` of window `layer` (0 = all).
    pub fn set_wrap_max(&mut self, layer: i32, id: i32, lines: i32) {
        if let Some(p) = self.part_mut(layer, id, WidgetKind::Text) {
            p.wrap_max = lines;
        }
    }

    /// Inserts `new` before index `at` (a window's run-time children right after the window draw before its record
    /// children, children draw in index order); the parent indices of the parts after `at` follow.
    pub fn insert_parts(&mut self, at: usize, new: Vec<Part>) {
        let n = new.len();
        for p in self.parts.iter_mut() {
            if let Some(q) = p.parent.as_mut() {
                if *q >= at {
                    *q += n;
                }
            }
        }
        let at = at.min(self.parts.len());
        self.parts.splice(at..at, new);
    }

    /// Shows or hides window `layer` (its controls follow it).
    pub fn show_window(&mut self, layer: i32, shown: bool) {
        if let Some(i) = self.window_index(layer) {
            self.parts[i].shown = shown;
        }
    }

    fn visible_part(&self, p: &Part) -> bool {
        p.shown && p.parent.is_none_or(|w| self.parts[w].shown)
    }

    pub fn visible(&self, i: usize) -> bool {
        self.visible_part(&self.parts[i])
    }

    /// The control under `p`: buttons and check buttons first (top-most first), then edit boxes.
    // EVIDENCE: cshell+0x1ad620, cshell+0x2f86d0 (the screen and a window try their controls last created first)
    // DC-DEFAULT: CLI-160 (proposed) buttons before edit boxes, as an edit box's click area here is wider than its
    // field (the 2009 order has no such step)
    pub fn hit(&self, p: [f32; 2]) -> Option<usize> {
        let pick = |want_click: bool| {
            (0..self.parts.len()).rev().find(|&i| {
                let part = &self.parts[i];
                let kind_ok = if want_click {
                    part.clickable()
                } else {
                    part.kind == WidgetKind::EditBox
                };
                kind_ok
                    && self
                        .modal
                        .is_none_or(|m| part.layer == m || self.modal_free == Some(part.page))
                    && self.visible(i)
                    && part.contains(p)
            })
        };
        pick(true).or_else(|| pick(false))
    }

    /// Updates the hover flags for the pointer at `p`.
    pub fn hover(&mut self, p: [f32; 2]) {
        let hit = self.hit(p);
        for (i, part) in self.parts.iter_mut().enumerate() {
            part.hover = Some(i) == hit && part.kind == WidgetKind::Button && part.enabled;
        }
    }

    /// Keyboard focus to edit box `i` (or none).
    pub fn focus(&mut self, i: Option<usize>) {
        for (k, part) in self.parts.iter_mut().enumerate() {
            part.focus = Some(k) == i && part.kind == WidgetKind::EditBox;
        }
    }

    pub fn focused(&self) -> Option<usize> {
        self.parts.iter().position(|p| p.focus)
    }

    /// A typed character for the focused edit box (printable characters only, up to its length).
    pub fn type_char(&mut self, c: char) -> bool {
        let Some(i) = self.focused() else {
            return false;
        };
        let p = &mut self.parts[i];
        let max = match p.look {
            Look::Edit { max_len, .. } => max_len,
            _ => 0,
        };
        if c.is_control() || (max > 0 && p.text.chars().count() >= max) {
            return true;
        }
        p.text.push(c);
        true
    }

    /// The focused edit box's text when the box is not masked and not empty.
    pub fn focused_plain_text(&self) -> Option<String> {
        let p = &self.parts[self.focused()?];
        (!p.masked && !p.text.is_empty()).then(|| p.text.clone())
    }

    /// Empties the focused plain edit box (a cut).
    pub fn clear_focused(&mut self) {
        if let Some(i) = self.focused() {
            if !self.parts[i].masked {
                self.parts[i].text.clear();
            }
        }
    }

    pub fn backspace(&mut self) -> bool {
        let Some(i) = self.focused() else {
            return false;
        };
        self.parts[i].text.pop();
        true
    }

    /// Gives scrolling list `i` its rows (`rows` of `row_h`): its scroll bar is built (at the list's place plus the
    /// record's bar offset, as high as the list less that offset, page = the record's bar size) and its range set to
    /// the rows' height. Lists without the record's scroll flag stay as they are.
    // EVIDENCE: cshell+0x2f00f0 (bar at list + offset, width 0xe, height = list height - offset y, page = the
    // record's bar size), cshell+0x2f0430 (range = the rows' heights with the spacing between them; step = the
    // first row's height + spacing)
    pub fn set_list_rows(&mut self, i: usize, rows: usize, row_h: i32, art: &[Option<String>; 4]) {
        let Some(p) = self.parts.get_mut(i) else {
            return;
        };
        let pos = p.pos;
        let Look::List {
            height,
            spacing,
            scroll,
            scroll_at,
            scroll_len,
            bar,
        } = &mut p.look
        else {
            return;
        };
        if !*scroll {
            return;
        }
        let rect = [
            pos[0] + scroll_at[0],
            pos[1] + scroll_at[1],
            super::scrollbar::WIDTH,
            *height - scroll_at[1],
        ];
        let b = bar.get_or_insert_with(|| {
            super::scrollbar::ScrollBar::new(rect, *scroll_len, art.clone())
        });
        b.rect = rect;
        b.page = *scroll_len;
        b.art = art.clone();
        b.step = row_h + *spacing;
        let n = rows as i32;
        b.set_content(if n == 0 {
            0
        } else {
            n * row_h + (n - 1) * *spacing
        });
    }

    /// The area of list `i` that takes wheel turns: from the list's corner to its bar's right edge, down to the
    /// list's height (at most the bar's offset + size).
    // EVIDENCE: cshell+0x2f00f0 (list width = bar offset x + 0xe, height at most bar offset y + bar size),
    // cshell+0x1af0a0 (a wheel turn goes to the control under the pointer)
    fn wheel_rect(&self, i: usize) -> Option<[i32; 4]> {
        let p = &self.parts[i];
        match &p.look {
            Look::List {
                height,
                scroll_at,
                scroll_len,
                bar: Some(_),
                ..
            } => Some([
                p.pos[0],
                p.pos[1],
                scroll_at[0] + super::scrollbar::WIDTH,
                (*height).min(scroll_at[1] + scroll_len),
            ]),
            _ => None,
        }
    }

    fn bars(&mut self) -> impl Iterator<Item = (usize, &mut super::scrollbar::ScrollBar)> {
        self.parts
            .iter_mut()
            .enumerate()
            .filter_map(|(i, p)| match &mut p.look {
                Look::List { bar: Some(b), .. } => Some((i, b)),
                _ => None,
            })
    }

    /// A wheel turn at `p` (`notches` > 0: away from the user) on the scrolling list under it: one row per notch,
    /// up for a positive turn. True when a list took it.
    // DISASM: cshell+0x2f09b0 (the list's wheel: |turn| rows, each the row's height + spacing, through
    // cshell+0x3458d0 / cshell+0x345900)
    pub fn list_wheel(&mut self, p: [f32; 2], notches: i32) -> bool {
        let Some(i) = (0..self.parts.len()).rev().find(|&i| {
            self.visible(i)
                && self.wheel_rect(i).is_some_and(|r| {
                    p[0] >= r[0] as f32
                        && p[1] >= r[1] as f32
                        && p[0] < (r[0] + r[2]) as f32
                        && p[1] < (r[1] + r[3]) as f32
                })
        }) else {
            return false;
        };
        if let Look::List { bar: Some(b), .. } = &mut self.parts[i].look {
            b.value = (b.value - notches * b.step).clamp(0, b.top());
        }
        true
    }

    /// A press on the scroll bar of a shown list (arrow step or the start of a thumb drag). True when taken.
    // EVIDENCE: cshell+0x2f07e0 (the list's hit test asks its bar first), cshell+0x2f1030 (a press on the bar goes
    // to the bar, then the value is clamped)
    pub fn list_press(&mut self, p: [f32; 2]) -> bool {
        let shown: Vec<bool> = (0..self.parts.len()).map(|i| self.visible(i)).collect();
        for (i, b) in self.bars() {
            if shown[i] && b.contains(p) {
                b.press(p);
                return true;
            }
        }
        false
    }

    /// The pointer moved: a dragged bar follows it. True when one did.
    // EVIDENCE: cshell+0x2f16d0 (pointer moves go to the bar)
    pub fn list_drag(&mut self, p: [f32; 2]) -> bool {
        self.bars().fold(false, |any, (_, b)| b.drag_to(p) || any)
    }

    /// The button went up: every drag ends. True when one was running.
    pub fn list_release(&mut self) -> bool {
        self.bars().fold(false, |any, (_, b)| {
            let was = b.drag;
            b.release();
            was || any
        })
    }

    /// List `i` loses its scroll bar (a hidden bar: not drawn, no presses or wheel turns, its rows unscrolled).
    pub fn drop_list_bar(&mut self, i: usize) {
        if let Some(Look::List { bar, .. }) = self.parts.get_mut(i).map(|p| &mut p.look) {
            *bar = None;
        }
    }

    /// The bar of list `i` (tests and scripted players).
    pub fn list_bar_mut(&mut self, i: usize) -> Option<&mut super::scrollbar::ScrollBar> {
        match &mut self.parts.get_mut(i)?.look {
            Look::List { bar, .. } => bar.as_mut(),
            _ => None,
        }
    }

    pub fn list_bar(&self, i: usize) -> Option<&super::scrollbar::ScrollBar> {
        match &self.parts.get(i)?.look {
            Look::List { bar, .. } => bar.as_ref(),
            _ => None,
        }
    }

    /// The controls in draw order: each shown window followed by its own shown controls, then the shown controls
    /// that sit on the screen.
    pub fn draw_order(&self) -> Vec<usize> {
        let mut order = Vec::with_capacity(self.parts.len());
        for (wi, w) in self.parts.iter().enumerate() {
            if w.kind != WidgetKind::Window || !w.shown {
                continue;
            }
            order.push(wi);
            order.extend(
                (0..self.parts.len())
                    .filter(|&c| self.parts[c].parent == Some(wi) && self.parts[c].shown),
            );
        }
        order.extend((0..self.parts.len()).filter(|&c| {
            let p = &self.parts[c];
            p.parent.is_none() && p.kind != WidgetKind::Window && p.shown
        }));
        order
    }

    /// The display list of the controls (`secs`: the screen clock for the caret).
    pub fn draw(&self, cat: &mut Catalog, secs: f32, out: &mut Vec<DrawItem>) {
        self.draw_traced(cat, secs, out, &mut |_, _| {});
    }

    /// The controls of window `layer` (the window itself and its children) again, over what was drawn since: a dialog
    /// window that a screen's own drawing must not cover.
    pub fn draw_layer(&self, layer: i32, cat: &mut Catalog, secs: f32, out: &mut Vec<DrawItem>) {
        for i in self.draw_order() {
            if self.parts[i].layer == layer {
                draw_part(&self.parts[i], cat, secs, out);
            }
        }
    }

    /// [`Sheet::draw`], telling `origin` which control (`None`: the tiled background) drew how many items.
    pub fn draw_traced(
        &self,
        cat: &mut Catalog,
        secs: f32,
        out: &mut Vec<DrawItem>,
        origin: &mut dyn FnMut(Option<usize>, usize),
    ) {
        if let Some(t) = &self.tiles {
            let n = out.len();
            t.draw(cat, out);
            origin(None, out.len() - n);
        }
        for i in self.draw_order() {
            let n = out.len();
            draw_part(&self.parts[i], cat, secs, out);
            origin(Some(i), out.len() - n);
        }
    }
}

/// An image at `at` with the layout size, never larger than its texture.
// EVIDENCE: cshell+0x1dbe80 (tiles placed by their own sizes)
pub fn image(tex: &str, at: [i32; 2], size: [i32; 2]) -> DrawItem {
    DrawItem::Image {
        tex: tex.to_string(),
        src: [size[0] as f32, size[1] as f32],
        dst: [at[0] as f32, at[1] as f32, size[0] as f32, size[1] as f32],
        tint: [255; 4],
        turn: None,
    }
}

/// An image clamped to its texture's size (unknown textures keep the layout size).
// EVIDENCE: cshell+0x342d10 (draw mode not 0: a quad of the record's size, one texel per pixel from the corner)
// DC-DEFAULT: CLI-160 (proposed) a rectangle past the texture's edge draws nothing there (the sampler state is not read)
pub fn image_fit(cat: &mut Catalog, tex: &str, at: [i32; 2], size: [i32; 2]) -> DrawItem {
    let (w, h) = match cat.tex_size(tex) {
        Some((tw, th)) => (size[0].min(tw as i32), size[1].min(th as i32)),
        None => (size[0], size[1]),
    };
    image(tex, at, [w, h])
}

/// Text at `at` in rectangle `size` with the layout's alignment pair.
pub fn place_text(
    text: &str,
    at: [i32; 2],
    size: [i32; 2],
    align: Align,
    font: i32,
    font_size: i32,
    colour: u32,
) -> DrawItem {
    let ax = match align.first {
        ALIGN_CENTRE => 0.5,
        ALIGN_RIGHT => 1.0,
        _ => 0.0,
    };
    let ay = if align.second == ALIGN_SECOND_CENTRE {
        0.5
    } else if align.second == ALIGN_BOTTOM {
        1.0
    } else {
        0.0
    };
    DrawItem::Label {
        style: TextStyle {
            font,
            colour: argb(colour),
            size: (font_size > 0).then_some(font_size),
        },
        pos: [
            at[0] as f32 + ax * size[0] as f32,
            at[1] as f32 + ay * size[1] as f32,
        ],
        text: text.to_string(),
        anchor: [ax, ay],
    }
}

fn draw_part(p: &Part, cat: &mut Catalog, secs: f32, out: &mut Vec<DrawItem>) {
    match &p.look {
        Look::Panel { tex, stretch } => {
            if tex.is_empty() {
            } else if *stretch {
                if let Some((w, h)) = cat.tex_size(tex) {
                    out.push(DrawItem::Image {
                        tex: tex.clone(),
                        src: [w as f32, h as f32],
                        dst: [
                            p.pos[0] as f32,
                            p.pos[1] as f32,
                            p.size[0] as f32,
                            p.size[1] as f32,
                        ],
                        tint: [255; 4],
                        turn: None,
                    });
                }
            } else {
                out.push(image_fit(cat, tex, p.pos, p.size));
            }
        }
        Look::Button {
            prefix,
            caption,
            frame,
        } => {
            // states: normal, over, down, disabled
            let state = if !p.enabled {
                3
            } else if (p.hover && p.down) || p.selected {
                2
            } else if p.hover {
                1
            } else {
                0
            };
            // a blinking button's texture only (blink mode 0: the caption keeps the state's colour)
            let tex_state = if state < 2 && blink_on(p.blink, secs) {
                2
            } else {
                state
            };
            if !prefix.is_empty() {
                let letter = BUTTON_LETTER[tex_state];
                let tex = format!("{prefix}{letter}.dtx");
                let tex = if cat.tex_size(&tex).is_some() {
                    tex
                } else {
                    format!("{prefix}A.dtx")
                };
                out.push(image_fit(cat, &tex, p.pos, p.size));
            }
            // a framed button's caption image sits at the text position with the record's last pair as its size
            // EVIDENCE: cshell+0x1c9020 (button record: text position, then the framed form's texture and pair),
            // cshell+0x1b07a0 -> cshell+0x33e210 (caption image at button + text position), cshell+0x33de40 (drawn
            // last), cshell+0x33e590 (the only place that moves it: with the button)
            if let Some((tex, size)) = frame {
                let at = p.text_at;
                out.push(image_fit(
                    cat,
                    tex,
                    [p.pos[0] + at[0], p.pos[1] + at[1]],
                    *size,
                ));
            }
            if let Some(c) = caption {
                let text = cat.text(c.string_id);
                out.push(place_text(
                    &text,
                    [p.pos[0] + c.at[0], p.pos[1] + c.at[1]],
                    [0, 0],
                    c.align,
                    c.font,
                    c.size,
                    c.colours[if p.selected { 0 } else { CAPTION_COLOUR[state] }],
                ));
            }
        }
        Look::Text {
            align,
            font,
            size,
            colour,
        } => {
            // aligned about the record's position, not inside the record's rectangle (the 2009 frames below; many
            // shown text records have a zero rectangle)
            // EVIDENCE: cshell+0x1b0bd0 (string control build from the text record: +0x18 / +0x1c alignment or-ed
            // into one flag word, +0x20 font, +0x24 size; the record itself then goes to the control's +0x20)
            // OBSERVED: lobbyfx4/real/R1_create.png, R10_lobby.png (2009, 1024 x 768: the lobby's
            // column titles centred on the record x, the room creation labels centred on the record y)
            if p.text.is_empty() {
            } else if p.wrap > 0 {
                // one item per wrapped line, each a line height below the one before; the breaks are measured in
                // the font's own virtual units, so every target size breaks the text as 1024 x 768 does
                // DC-DEFAULT: CLI-540 (proposed)
                let (font, size) = (*font, *size);
                let lh = if p.wrap_pitch > 0 {
                    p.wrap_pitch as f32
                } else {
                    cat.measure_text(font, size, "").map_or(0.0, |m| m.1)
                };
                let mut lines = super::roomtext::wrap_lines(&p.text, p.wrap as f32, |t| {
                    cat.measure_text_font_units(font, size, t).unwrap_or(0.0)
                });
                if p.wrap_max > 0 {
                    lines = super::roomtext::cut_lines(lines, p.wrap_max as usize, p.wrap as f32, |t| {
                        cat.measure_text_font_units(font, size, t).unwrap_or(0.0)
                    });
                }
                for (i, line) in lines.iter().enumerate() {
                    let mut item = place_text(line, p.pos, [0, 0], *align, font, size, *colour);
                    if let DrawItem::Label { pos, .. } = &mut item {
                        pos[1] += i as f32 * lh;
                    }
                    out.push(item);
                }
            } else {
                out.push(place_text(
                    &p.text,
                    p.pos,
                    [0, 0],
                    *align,
                    *font,
                    *size,
                    *colour,
                ));
            }
        }
        Look::Edit {
            font,
            size,
            caret_colour,
            caret_period,
            clip,
            text_colour,
            ..
        } if *clip > 0 => draw_clipped_edit(
            p,
            cat,
            secs,
            out,
            (
                *font,
                *size,
                *text_colour,
                *caret_colour,
                *caret_period,
                *clip,
            ),
        ),
        Look::Edit {
            font,
            size,
            caret_colour,
            caret_period,
            ..
        } => {
            let mut s = if p.masked {
                "*".repeat(p.text.chars().count())
            } else {
                p.text.clone()
            };
            let period = if *caret_period > 0.0 {
                *caret_period
            } else {
                0.333
            };
            if p.focus && (secs % (2.0 * period)) < period {
                s.push('|');
            }
            if !s.is_empty() {
                let colour = if p.focus { 0xffff_ffff } else { *caret_colour };
                out.push(place_text(
                    &s,
                    p.pos,
                    [0, 0],
                    Align::default(),
                    *font,
                    *size,
                    colour,
                ));
            }
        }
        Look::Check { on, off } => {
            let tex = if p.checked { on } else { off };
            if cat.tex_size(tex).is_some() {
                out.push(image_fit(cat, tex, p.pos, p.size));
            }
        }
        Look::List { bar: Some(b), .. } if !b.over_rows => b.draw(cat, out),
        Look::List { .. } => {}
    }
}

/// The caret is shown in the first half of each blink cycle of `period` seconds on the screen clock `secs`.
pub fn caret_on(secs: f32, period: f32) -> bool {
    let period = if period > 0.0 { period } else { CARET_PERIOD };
    (secs % (2.0 * period)) < period
}

/// The tail of `text` that fits in `width` pixels as `measure` gives them (the whole text when it fits).
pub fn fitting_tail(text: &str, width: f32, mut measure: impl FnMut(&str) -> f32) -> &str {
    let mut start = 0;
    let mut rest = text;
    while !rest.is_empty() && measure(rest) > width {
        let step = rest.chars().next().map_or(1, char::len_utf8);
        start += step;
        rest = &text[start..];
    }
    rest
}

/// A run-time edit field `clip` pixels wide: the tail of its text that leaves room for the caret, then the caret as a
/// bar of the font size's height while it blinks on.
// EVIDENCE: cshell+0x1e880 (the chat box: clip rectangle at the control's place, 0xff x 0xf), cshell+0x2f1c70 (the
// caret flips each +0x1ac seconds of the engine clock), cshell+0x2f79c0 (+0x1ac = 0.333 from the chat build)
// OBSERVED: lobbyfx4/real/R10_lobby.png (2009, empty chat field: the caret drawn x 185..186, y
// 689..701, (105, 61, 123) over the field's (8, 7, 5): the edit control's default caret colour 0x89bc6ae0 of
// cshell+0x2f74c0 blended)
// DC-DEFAULT: CLI-484 (proposed) the caret sits right after the shown text; a text longer than the field shows its
// tail (the 2009 edit control's scrolling is not read)
fn draw_clipped_edit(
    p: &Part,
    cat: &mut Catalog,
    secs: f32,
    out: &mut Vec<DrawItem>,
    (font, size, colour, caret, period, clip): (i32, i32, u32, u32, f32, i32),
) {
    let width = (clip - CARET_WIDTH) as f32;
    let (shown, text_w) = {
        let mut m = |t: &str| cat.measure_text(font, size, t).map_or(0.0, |m| m.0);
        let tail = fitting_tail(&p.text, width, &mut m).to_string();
        let w = if tail.is_empty() { 0.0 } else { m(&tail) };
        (tail, w)
    };
    if !shown.is_empty() {
        out.push(place_text(
            &shown,
            p.pos,
            [0, 0],
            Align::default(),
            font,
            size,
            colour,
        ));
    }
    if p.focus && caret_on(secs, period) {
        out.push(DrawItem::Fill {
            rect: [
                p.pos[0] as f32 + text_w,
                p.pos[1] as f32,
                CARET_WIDTH as f32,
                size as f32,
            ],
            colour: argb(caret),
        });
    }
}

/// The caret bar's width (virtual pixels).
// OBSERVED: lobbyfx4/real/R10_lobby.png (x 185..186)
pub const CARET_WIDTH: i32 = 2;

/// Row geometry of a list: the top of row `i` (rows of `row_h` with the list's spacing) and whether it fits. A
/// scrolled list moves its rows up by its bar's value; rows above its top or below its bottom are not shown.
// EVIDENCE: cshell+0x2f0430 (the rows from the first whose top is at or under the bar's value, each at its top less
// the value, down to the last one that fits in the list's height)
pub fn list_row(list: &Part, row_h: i32, i: usize, first: usize) -> Option<[i32; 2]> {
    let Look::List {
        height,
        spacing,
        ref bar,
        ..
    } = list.look
    else {
        return None;
    };
    if i < first {
        return None;
    }
    let scrolled = bar.as_ref().map_or(0, |b| b.value);
    let y = (i - first) as i32 * (row_h + spacing) - scrolled;
    (y >= 0 && (height <= 0 || y + row_h <= height)).then_some([list.pos[0], list.pos[1] + y])
}

/// The row of a list under `p` (rows `row_w` x `row_h`, `n` rows from row `first`).
pub fn list_hit(
    list: &Part,
    row_w: i32,
    row_h: i32,
    n: usize,
    first: usize,
    p: [f32; 2],
) -> Option<usize> {
    (first..n).find(|&i| {
        list_row(list, row_h, i, first).is_some_and(|r| {
            p[0] >= r[0] as f32
                && p[0] < (r[0] + row_w) as f32
                && p[1] >= r[1] as f32
                && p[1] < (r[1] + row_h) as f32
        })
    })
}

/// How many rows of `row_h` fit in a list.
pub fn list_capacity(list: &Part, row_h: i32) -> usize {
    match list.look {
        Look::List {
            height, spacing, ..
        } if height > 0 && row_h > 0 => ((height + spacing) / (row_h + spacing)).max(1) as usize,
        _ => 16,
    }
}
