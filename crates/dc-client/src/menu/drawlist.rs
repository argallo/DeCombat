//! The front end's draw list with its provenance, and the structural check of it against the ROM layout pages.
//!
//! [`Menu::draw_trace`] returns every item the current screen draws, in draw order, each with where it came from:
//! the tiled background, a control built from a `screen_pages` record (page, record index, kind, window, control
//! id), the screen's own dynamic drawing (list rows, panels filled from lobby data) or the open message box. The
//! rows carry what was drawn (kind, rectangle in virtual 1024 x 768 pixels, texture, text hash, font index and size,
//! colour) and, for record-built items, what the record asks for (position, size, font, alignment, colour).
//!
//! [`check`] compares the two and names each mismatch with a class (`POS`, `SIZE`, `FONT`, `FSIZE`, `COLOUR`,
//! `ORDER`, `DUP`, `MISSING`, `FACE`, `COVERED`, `OVERLAP`, `RANGE`); `examples/menu_drawlist.rs` writes the rows and
//! the mismatches of every screen as JSONL and `gate/uidiff.py` turns them into the per-screen counts the
//! deploy gate ratchets.
//!
//! No ROM text is written unless the caller asks for it ([`TraceOptions::with_text`]): rows carry the text's length
//! and a SHA-256 prefix.
// SPEC: FMT_CRES_UI.md §6.3 (record schemas, value code `A`: first letter l 1, r 2, c 4, t 0x10, b 0x20; second
// letter `c` 0x40), §7.1 (system font records: the font index of a record indexes them)
// EVIDENCE: cshell+0x1c9680 (text record: pos, rect, alignment pair, font index, font size, text id, four colours),
// cshell+0x1c9020 (button record: text position, alignment pair, font index, font size, caption, four colours),
// cshell+0x1c07b0 (font lookup by index; past the end gives entry 0 when allowed)

use super::catalog::{argb, Catalog};
use super::sheet::{Look, Part, ALIGN_BOTTOM, ALIGN_CENTRE, ALIGN_RIGHT};
use super::Menu;
use crate::hud::items::DrawItem;
use dc_ui_data::pages::{Align, Widget, ALIGN_SECOND_CENTRE};
use dc_ui_data::{FontResolver, SystemFonts, WidgetKind};
use serde_json::{json, Value};

/// Where a drawn item came from.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    /// The opaque backdrop the menu draws first.
    Backdrop,
    /// The tiled background of the screen's pages.
    Tiles,
    /// Control `part` of the screen's sheet; `piece` counts the items that control drew (0 = its first).
    Part { part: usize, piece: usize },
    /// The screen's own drawing (rows of lists, panels filled from lobby data).
    Extra,
    /// The open drop-down list of the screen (`list`: what it holds; `piece` 0 = its frame).
    Dropdown { list: &'static str, piece: usize },
    /// The open message box.
    Popup,
    /// The menu's own mouse cursor image (drawn last).
    Cursor,
    /// w3-options: the screen under an open box (report, help), drawn before the box; the box covers it on purpose.
    Under,
}

/// What the record of a control asks for (virtual pixels, absolute).
#[derive(Debug, Clone, PartialEq)]
pub struct RecordWant {
    /// Page tag index and record index in that page.
    pub page: usize,
    pub rec: usize,
    pub kind: WidgetKind,
    /// Window number (windows: their own; others: their parent, -1 on the screen).
    pub layer: i32,
    pub id: i32,
    /// The control's absolute top-left and size.
    pub pos: [i32; 2],
    pub size: [i32; 2],
    /// Text items: the anchor point and anchor fractions the record's alignment gives, font index, font size and
    /// colour (`0xAARRGGBB`).
    pub text_at: Option<[f32; 2]>,
    pub text_anchor: Option<[f32; 2]>,
    pub font: Option<i32>,
    pub font_size: Option<i32>,
    pub colour: Option<u32>,
    /// Text records: the record's own rectangle as written (`R` value).
    pub rect: Option<[i32; 4]>,
    pub align: Option<Align>,
    /// Text records: the record names a string of the string table (else the screen fills the control at run
    /// time).
    pub rom_text: bool,
    /// Text and button records: all four colours of the record (`C` value).
    pub colours: Option<[u32; 4]>,
}

/// What a text of the screen's own drawing asks for (dynamic rows: server / channel bars, the lobby's room and user
/// rows, the waiting room's member rows, the result rows), noted by the screen next to the item it draws. The want is computed by the screen's rule function, apart from its drawing code, so
/// a drawing that drifts from the rule is a `POS` / `ALIGN` / `FONT` / `FSIZE` mismatch like a record's.
// DC-DEFAULT: CLI-465 (proposed) dynamic texts carry a layout key `dyn/<what>/<style key sha>` and the anchor the
// screen's 2009 rule gives
#[derive(Debug, Clone, PartialEq)]
pub struct DynWant {
    /// The layout key the text is placed by, e.g. `dyn/bar0/d5698f80e06d3478` (row 0, the style key's sha prefix).
    pub key: String,
    /// Anchor point (virtual pixels) and anchor fractions the rule gives.
    pub text_at: [f32; 2],
    pub text_anchor: [f32; 2],
    pub font: Option<i32>,
    pub font_size: Option<i32>,
}

/// Notes of a screen's dynamic drawing: the index of the item in the draw list and what it asks for.
pub type DynNotes = Vec<(usize, DynWant)>;

/// One drawn item.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceRow {
    pub seq: usize,
    pub origin: Origin,
    pub item: DrawItem,
    /// Drawn rectangle (virtual pixels; text: the line box the compositor fills).
    pub rect: [f32; 4],
    /// Text: the font record behind the index (`None`: no record, drawn with the default font) and whether its
    /// own face was found (`false`: drawn with the client's default font file).
    pub font_record: Option<i32>,
    pub font_own_face: Option<bool>,
    pub want: Option<RecordWant>,
    /// Dynamic texts: what the screen's rule asks for (see [`DynWant`]).
    pub note: Option<DynWant>,
    /// The frame of an open drop-down list: where the 2009 rule puts it (x, y, w, h; [`super::dropdown::rule_frame`]).
    pub frame_want: Option<[f32; 4]>,
}

/// Options of a trace.
#[derive(Debug, Clone, Copy, Default)]
pub struct TraceOptions {
    /// Write the drawn text (ROM strings) into the rows: local diagnosis only, never committed.
    pub with_text: bool,
}

/// One system font record as the trace sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FontSlot {
    /// The record's own file resolves in the user's font archive (else the face is a system face: drawn with the
    /// client's default font file).
    pub own_file: bool,
    pub height: i32,
    pub bold: bool,
    /// Face and file names (ROM text: written only with [`TraceOptions::with_text`]).
    pub face: String,
    pub file: Option<String>,
}

/// How each font index resolves: record present, and its own face available in the user's font archive.
#[derive(Debug, Clone, Default)]
pub struct FontTable {
    pub slots: Vec<Option<FontSlot>>,
    /// The installed face (name, bold) the menu draws records without a file of their own with.
    pub system_face: Option<(String, bool)>,
}

impl FontTable {
    pub fn from_catalog(cat: &Catalog) -> FontTable {
        let (Some(layout), Some(rom)) = (cat.layout.as_ref(), cat.rom.as_ref()) else {
            return FontTable::default();
        };
        let fonts = SystemFonts::from_layout(layout);
        let resolver = FontResolver::open(rom).ok();
        let slots = fonts
            .iter()
            .map(|s| {
                s.map(|spec| FontSlot {
                    own_file: resolver.as_ref().and_then(|r| r.resolve(spec)).is_some(),
                    height: spec.height,
                    bold: spec.bold,
                    face: spec.face.clone(),
                    file: spec.file.clone(),
                })
            })
            .collect();
        FontTable {
            slots,
            system_face: cat.system_face.clone(),
        }
    }

    /// The record slot index `font` resolves to (the client's lookup falls back to slot 0).
    // EVIDENCE: cshell+0x1c07b0
    pub fn slot(&self, font: i32) -> Option<usize> {
        let i = usize::try_from(font).ok()?;
        match self.slots.get(i) {
            Some(Some(_)) => Some(i),
            _ => self.slots.first().and_then(|s| s.as_ref()).map(|_| 0),
        }
    }

    /// The record's own face is what draws it: its file from the ROM, or the installed face of its name and
    /// weight that the menu's default font was loaded from.
    fn own_file(&self, slot: usize) -> bool {
        self.slots
            .get(slot)
            .and_then(|s| s.as_ref())
            .is_some_and(|s| {
                s.own_file
                    || (s.file.is_none()
                        && self
                            .system_face
                            .as_ref()
                            .is_some_and(|(f, b)| f.eq_ignore_ascii_case(&s.face) && *b == s.bold))
            })
    }

    /// The table as JSON (face and file names only `with_text`).
    pub fn json(&self, opts: TraceOptions) -> Value {
        Value::Array(
            self.slots
                .iter()
                .enumerate()
                .map(|(i, s)| match s {
                    None => json!({"index": i, "record": false}),
                    Some(s) => {
                        let mut v = json!({"index": i, "record": true, "own_file": s.own_file,
                            "height": s.height, "bold": s.bold, "has_file": s.file.is_some()});
                        if opts.with_text {
                            v["face"] = json!(s.face);
                            v["file"] = json!(s.file);
                        }
                        v
                    }
                })
                .collect(),
        )
    }
}

/// The record colour a text control or a button caption shows in its normal state (the check's own copy of the
/// rule, independent of the drawing code it checks).
// EVIDENCE: cshell+0x18930 (the controls' colour getter: no state flag set -> the second colour word)
// EVIDENCE: cshell+0x1b0bd0 (four colour words handed in record order to the control's colour setter)
// OBSERVED: lobbyfx4/real/R10_lobby.png, R1_create.png, R20_room.png (2009: records with colours
// 170 / 238 / 238 / x drawn 238, records with 170 in all four drawn 170)
pub const WANT_NORMAL_COLOUR: usize = 1;

/// The anchor fractions of a layout alignment pair: x (left 0, centre 0.5, right 1) and y (top 0, centre 0.5,
/// bottom 1).
// SPEC: FMT_CRES_UI.md §6.3 (value code `A`)
pub fn anchor_of(align: Align) -> [f32; 2] {
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
    [ax, ay]
}

fn sha16(text: &str) -> String {
    dc_rom::sha256_hex(text.as_bytes())[..16].to_string()
}

/// The record a control was built from, with what it asks the draw to be.
pub fn record_want(cat: &Catalog, part: &Part, parent_pos: [i32; 2]) -> Option<RecordWant> {
    let page = cat.pages.page(part.page)?;
    let w = &page.widgets.get(part.rec)?.widget;
    let abs = |p: [i32; 2]| [p[0] + parent_pos[0], p[1] + parent_pos[1]];
    let size_of = |r: [i32; 4]| [r[2] - r[0], r[3] - r[1]];
    let mut want = RecordWant {
        page: part.page.index(),
        rec: part.rec,
        kind: w.kind(),
        layer: part.layer,
        id: part.id,
        pos: [0, 0],
        size: [0, 0],
        text_at: None,
        text_anchor: None,
        font: None,
        font_size: None,
        colour: None,
        rect: None,
        align: None,
        rom_text: false,
        colours: None,
    };
    match w {
        Widget::Window(r) => {
            want.pos = r.pos;
            want.size = size_of(r.source_rect);
        }
        Widget::Image(r) => {
            want.pos = abs(r.pos);
            want.size = size_of(r.source_rect);
        }
        Widget::Button(r) => {
            want.pos = abs(r.pos);
            want.size = size_of(r.source_rect);
            if r.text_style > 0 {
                let at = [want.pos[0] + r.text_pos[0], want.pos[1] + r.text_pos[1]];
                want.text_at = Some([at[0] as f32, at[1] as f32]);
                want.text_anchor = Some(anchor_of(r.text_align));
                want.font = Some(r.font_index);
                want.font_size = Some(r.font_size);
                want.colour = Some(r.text_colours[WANT_NORMAL_COLOUR]);
                want.colours = Some(r.text_colours);
                want.align = Some(r.text_align);
            }
        }
        Widget::Text(r) => {
            // the alignment is about the record's position, not inside its rectangle: the 2009 frames show it, and
            // many shown text records have a zero rectangle (the lobby's character-info values, the result's
            // fields), which an in-rectangle alignment could not place
            // EVIDENCE: cshell+0x1b0bd0 (the string control is created from +0x18 / +0x1c alignment or-ed into one
            // flag word, +0x20 font index through cshell+0x1c07b0, +0x24 size; the whole record then goes to the
            // control's +0x20 / the window's +0x74, so this function alone does not exclude the rectangle)
            // OBSERVED: lobbyfx4/real/R1_create.png, R10_lobby.png (2009 frames, 1024 x 768: the
            // lobby column titles are centred on the record x, the room creation labels centred on the record y)
            want.pos = abs(r.pos);
            want.size = size_of(r.rect);
            want.rom_text = r.text_style > 0;
            let a = anchor_of(r.align);
            want.text_at = Some([want.pos[0] as f32, want.pos[1] as f32]);
            want.text_anchor = Some(a);
            want.font = Some(r.font_index);
            want.font_size = Some(r.font_size);
            want.colour = Some(r.colours[WANT_NORMAL_COLOUR]);
            want.colours = Some(r.colours);
            want.rect = Some(r.rect);
            want.align = Some(r.align);
        }
        Widget::EditBox(r) => {
            want.pos = abs(r.pos);
            want.size = [0, r.font_size];
            want.text_at = Some([want.pos[0] as f32, want.pos[1] as f32]);
            want.text_anchor = Some([0.0, 0.0]);
            want.font = Some(r.font_index);
            want.font_size = Some(r.font_size);
        }
        Widget::List(r) => {
            want.pos = abs(r.pos);
            want.size = [0, r.row_height];
        }
        Widget::CheckButton(r) => {
            want.pos = abs(r.pos);
            want.size = size_of(r.source_rect);
        }
        _ => return None,
    }
    // a control the screen moved at run time is checked at its new place (the text anchor moves with it)
    if part.placed {
        if let Some(at) = want.text_at.as_mut() {
            let rel = if w.kind() == WidgetKind::Button {
                part.text_at
            } else {
                [at[0] as i32 - want.pos[0], at[1] as i32 - want.pos[1]]
            };
            *at = [(part.pos[0] + rel[0]) as f32, (part.pos[1] + rel[1]) as f32];
        }
        want.pos = part.pos;
    }
    Some(want)
}

/// The drawn rectangle of an item (virtual pixels); text by the atlas the compositor would use.
fn item_rect(cat: &mut Catalog, item: &DrawItem) -> [f32; 4] {
    let (w, h) = cat.size();
    let sx = w as f32 / 1024.0;
    let sy = h as f32 / 768.0;
    match item {
        DrawItem::Image { dst, .. } => *dst,
        DrawItem::Fill { rect, .. } => *rect,
        DrawItem::Text {
            style,
            pos,
            text,
            right,
        } => {
            let a = cat.assets.fonts.atlas_sized(style.font, style.size);
            let (tw, lh) = a
                .map(|a| (a.measure(text) / sx, a.line_height / sy))
                .unwrap_or((0.0, 0.0));
            let x = if *right { pos[0] - tw } else { pos[0] };
            [x, pos[1], tw, lh]
        }
        DrawItem::Label {
            style,
            pos,
            text,
            anchor,
        } => {
            let a = cat.assets.fonts.atlas_sized(style.font, style.size);
            let (tw, lh) = a
                .map(|a| (a.measure(text) / sx, a.cell_height / sy))
                .unwrap_or((0.0, 0.0));
            [pos[0] - anchor[0] * tw, pos[1] - anchor[1] * lh, tw, lh]
        }
        _ => [0.0, 0.0, 0.0, 0.0],
    }
}

fn text_font(item: &DrawItem) -> Option<(i32, Option<i32>)> {
    match item {
        DrawItem::Text { style, .. } | DrawItem::Label { style, .. } => {
            Some((style.font, style.size))
        }
        _ => None,
    }
}

impl Menu {
    /// The current screen's draw list with provenance (the backdrop first, as [`Menu::frame_items`] draws it).
    /// Does not mark the screen drawn.
    pub fn draw_trace(&mut self) -> Vec<TraceRow> {
        self.sync_lists();
        let secs = self.secs();
        let fonts = FontTable::from_catalog(&self.cat);
        let mut out = Vec::new();
        let mut origins: Vec<Origin> = vec![Origin::Backdrop];
        out.push(DrawItem::Fill {
            rect: [0.0, 0.0, 1024.0, 768.0],
            colour: [0, 0, 0, 255],
        });
        let n = out.len();
        self.draw_under(secs, &mut out);
        origins.extend(std::iter::repeat_n(Origin::Under, out.len() - n));
        let sheet = self.screen.sheet();
        sheet.draw_traced(&mut self.cat, secs, &mut out, &mut |who, n| {
            for piece in 0..n {
                origins.push(match who {
                    None => Origin::Tiles,
                    Some(part) => Origin::Part { part, piece },
                });
            }
        });
        let n = out.len();
        let mut notes = DynNotes::new();
        self.screen
            .draw_extra_noted(&mut self.cat, &self.data, secs, &mut out, &mut notes);
        // w3-clan: the messenger window is the menu's own drawing over the screen
        self.messenger.draw(&mut self.cat, &self.data, &mut out);
        origins.extend(std::iter::repeat_n(Origin::Extra, out.len() - n));
        // the open drop-down list over the screen: its frame, scroll bar and rows, the rows noted with the rule
        let mut frame_want = None;
        if let Some(d) = self.screen.dropdown() {
            let n = out.len();
            for (seq, k) in d.draw(&mut self.cat, &self.lits.drop_art, &mut out) {
                let mut want = super::dropdown::rule_row(d.list, d.anchor, k, d.font_size);
                if d.left {
                    want.text_at[0] = (d.anchor[0] + super::dropdown::LEFT_PAD) as f32;
                    want.text_anchor = [0.0, 0.0];
                }
                notes.push((seq, want));
            }
            for piece in 0..out.len() - n {
                origins.push(Origin::Dropdown {
                    list: d.list,
                    piece,
                });
            }
            frame_want = Some((
                n,
                super::dropdown::rule_frame(d.anchor, d.shown().len(), d.font_size),
            ));
        }
        let n = out.len();
        if let Some(p) = self.popups.first() {
            p.draw(&mut self.cat, &mut out);
        }
        origins.extend(std::iter::repeat_n(Origin::Popup, out.len() - n));
        // L1: the cursor image, last
        if let Some(c) = self.cursor_item() {
            out.push(c);
            origins.push(Origin::Cursor);
        }
        let sheet = self.screen.sheet();
        out.into_iter()
            .zip(origins)
            .enumerate()
            .map(|(seq, (item, origin))| {
                let note = notes
                    .iter()
                    .find(|(i, _)| {
                        *i == seq && matches!(origin, Origin::Extra | Origin::Dropdown { .. })
                    })
                    .map(|(_, w)| w.clone());
                let frame = frame_want.and_then(|(i, w)| (i == seq).then_some(w));
                let want = match &origin {
                    Origin::Part { part, .. } => {
                        let p = &sheet.parts[*part];
                        let parent_pos = p.parent.map_or([0, 0], |w| sheet.parts[w].pos);
                        record_want(&self.cat, p, parent_pos)
                    }
                    _ => None,
                };
                let rect = item_rect(&mut self.cat, &item);
                let (font_record, font_own_face) = match text_font(&item) {
                    Some((f, _)) => {
                        let slot = fonts.slot(f);
                        (
                            slot.map(|s| s as i32),
                            Some(slot.is_some_and(|s| fonts.own_file(s))),
                        )
                    }
                    None => (None, None),
                };
                TraceRow {
                    seq,
                    origin,
                    item,
                    rect,
                    font_record,
                    font_own_face,
                    want,
                    note,
                    frame_want: frame,
                }
            })
            .collect()
    }

    /// The current screen's controls (for the MISSING check).
    pub fn trace_sheet(&self) -> &super::sheet::Sheet {
        self.screen.sheet()
    }
}

/// A row as JSON (`screen`: the screen's name).
pub fn row_json(screen: &str, r: &TraceRow, opts: TraceOptions) -> Value {
    let (kind, tex, text, font, size, colour, anchor) = match &r.item {
        DrawItem::Image { tex, tint, .. } => {
            ("image", Some(tex.clone()), None, None, None, *tint, None)
        }
        DrawItem::Fill { colour, .. } => ("fill", None, None, None, None, *colour, None),
        DrawItem::Text { style, text, .. } => (
            "text",
            None,
            Some(text.clone()),
            Some(style.font),
            style.size,
            style.colour,
            None,
        ),
        DrawItem::Label {
            style,
            text,
            anchor,
            ..
        } => (
            "label",
            None,
            Some(text.clone()),
            Some(style.font),
            style.size,
            style.colour,
            Some(*anchor),
        ),
        DrawItem::AimMark(_) => ("aim_mark", None, None, None, None, [0; 4], None),
        DrawItem::Crosshair(c) => ("drawn_crosshair", None, None, None, None, c.colour, None),
        DrawItem::AimCross { colour, .. } => ("aim_cross", None, None, None, None, *colour, None),
    };
    let origin = match &r.origin {
        Origin::Backdrop => json!({"from": "backdrop"}),
        Origin::Tiles => json!({"from": "tiles"}),
        Origin::Part { part, piece } => json!({"from": "record", "part": part, "piece": piece}),
        Origin::Extra => json!({"from": "dynamic"}),
        Origin::Dropdown { list, piece } => {
            json!({"from": "dropdown", "list": list, "piece": piece, "frame_want": r.frame_want})
        }
        Origin::Popup => json!({"from": "popup"}),
        Origin::Cursor => json!({"from": "cursor"}),
        Origin::Under => json!({"from": "under"}),
    };
    let mut v = json!({
        "screen": screen,
        "seq": r.seq,
        "origin": origin,
        "kind": kind,
        "rect": r.rect.map(|x| (x * 100.0).round() / 100.0),
        "colour": colour,
    });
    let o = v.as_object_mut().expect("object");
    if let Some(t) = tex {
        o.insert("tex_sha".into(), json!(sha16(&t.to_ascii_lowercase())));
        if opts.with_text {
            o.insert("tex".into(), json!(t));
        }
    }
    if let Some(t) = text {
        o.insert("text_sha".into(), json!(sha16(&t)));
        o.insert("text_chars".into(), json!(t.chars().count()));
        if opts.with_text {
            o.insert("text".into(), json!(t));
        }
    }
    if let Some(f) = font {
        o.insert("font_index".into(), json!(f));
        o.insert("font_size".into(), json!(size));
        o.insert("font_record".into(), json!(r.font_record));
        o.insert("font_own_face".into(), json!(r.font_own_face));
    }
    if let Some(a) = anchor {
        o.insert("anchor".into(), json!(a));
    }
    if let Some(w) = &r.want {
        o.insert(
            "layout_key".into(),
            json!(format!(
                "p{}/r{}/{:?}/w{}/id{}",
                w.page, w.rec, w.kind, w.layer, w.id
            )),
        );
        o.insert(
            "want".into(),
            json!({
                "pos": w.pos, "size": w.size, "text_at": w.text_at, "text_anchor": w.text_anchor,
                "font_index": w.font, "font_size": w.font_size,
                "colour": w.colour.map(argb), "colours": w.colours.map(|c| c.map(argb)), "rect": w.rect,
                "align": w.align.map(|a| [a.first, a.second]),
            }),
        );
    }
    if let Some(n) = &r.note {
        o.insert("layout_key".into(), json!(n.key));
        o.insert(
            "want".into(),
            json!({
                "text_at": n.text_at, "text_anchor": n.text_anchor,
                "font_index": n.font, "font_size": n.font_size,
            }),
        );
    }
    v
}

/// One structural mismatch.
#[derive(Debug, Clone, PartialEq)]
pub struct Mismatch {
    pub class: &'static str,
    /// The layout key (`p<page>/r<record>/<kind>/w<window>/id<id>`) or the origin of an item without a record.
    pub key: String,
    pub seq: usize,
    pub detail: String,
    /// Size of the difference (pixels, or 1 for a categorical mismatch).
    pub delta: f32,
}

/// Position tolerance (virtual pixels).
pub const POS_TOL: f32 = 1.0;

fn key_of(r: &TraceRow) -> String {
    if let Some(n) = &r.note {
        return n.key.clone();
    }
    match &r.want {
        Some(w) => format!(
            "p{}/r{}/{:?}/w{}/id{}",
            w.page, w.rec, w.kind, w.layer, w.id
        ),
        None => match r.origin {
            Origin::Backdrop => "backdrop".into(),
            Origin::Tiles => "tiles".into(),
            Origin::Part { part, .. } => format!("part{part}"),
            Origin::Extra => "dynamic".into(),
            Origin::Dropdown { list, .. } => format!("dropdown/{list}"),
            Origin::Popup => "popup".into(),
            Origin::Cursor => "cursor".into(),
            Origin::Under => "under".into(),
        },
    }
}

fn is_text(item: &DrawItem) -> bool {
    matches!(item, DrawItem::Text { .. } | DrawItem::Label { .. })
}

fn intersect(a: [f32; 4], b: [f32; 4]) -> f32 {
    let w = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let h = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    if w > 0.0 && h > 0.0 {
        w * h
    } else {
        0.0
    }
}

/// The structural check of one screen's trace: every record-built item against its record, the draw order against
/// the record order, items drawn twice, texts covered by a later image, texts drawn over other texts, and font
/// indices without a record or without their own face.
pub fn check(
    rows: &[TraceRow],
    sheet: &super::sheet::Sheet,
    fonts: &FontTable,
    cat: &mut Catalog,
) -> Vec<Mismatch> {
    let mut out = Vec::new();
    let mut push = |class, r: &TraceRow, detail: String, delta: f32| {
        out.push(Mismatch {
            class,
            key: key_of(r),
            seq: r.seq,
            detail,
            delta,
        })
    };
    let mut seen_parts: Vec<(usize, usize)> = Vec::new();
    let mut no_system_face = false;
    let mut last_part_order: Option<usize> = None;
    let order = sheet.draw_order();
    for r in rows {
        // dynamic texts with a note: the screen's rule
        if let (
            Some(n),
            DrawItem::Label {
                style, pos, anchor, ..
            },
        ) = (&r.note, &r.item)
        {
            let dx = pos[0] - n.text_at[0];
            let dy = pos[1] - n.text_at[1];
            if dx.abs() > POS_TOL || dy.abs() > POS_TOL {
                push(
                    "POS",
                    r,
                    format!("text anchor point at +{dx:.1},+{dy:.1} from the rule's"),
                    dx.abs().max(dy.abs()),
                );
            }
            let an = n.text_anchor;
            if (anchor[0] - an[0]).abs() > 0.01 || (anchor[1] - an[1]).abs() > 0.01 {
                push(
                    "ALIGN",
                    r,
                    format!("anchor {anchor:?} for the rule's {an:?}"),
                    1.0,
                );
            }
            if let Some(f) = n.font.filter(|f| *f != style.font) {
                push(
                    "FONT",
                    r,
                    format!("font {} for the rule's {f}", style.font),
                    1.0,
                );
            }
            if let Some(s) = n.font_size.filter(|s| style.size != Some(*s)) {
                push(
                    "FSIZE",
                    r,
                    format!("size {:?} for the rule's {s}", style.size),
                    1.0,
                );
            }
        }
        // record-built items
        if let (Origin::Part { part, piece }, Some(w)) = (&r.origin, &r.want) {
            if seen_parts.contains(&(*part, *piece)) {
                push(
                    "DUP",
                    r,
                    format!("part {part} piece {piece} drawn twice"),
                    1.0,
                );
            }
            seen_parts.push((*part, *piece));
            // draw order: the part's position in the window-then-children walk never goes back
            let pos_in_order = order.iter().position(|o| o == part);
            if let (Some(now), Some(before)) = (pos_in_order, last_part_order) {
                if now < before {
                    push(
                        "ORDER",
                        r,
                        format!("drawn at walk step {now} after {before}"),
                        1.0,
                    );
                }
            }
            last_part_order = pos_in_order.or(last_part_order);
            match &r.item {
                DrawItem::Image { dst, .. } => {
                    let p = sheet.parts[*part].kind;
                    // a button's caption image sits at its text position: compare only the body; a list draws
                    // only its scroll bar, a run-time child the record does not place (the bar's own rule,
                    // menu::scrollbar)
                    // EVIDENCE: cshell+0x2f00f0 (the list's bar built in code at the record's offset)
                    if p != WidgetKind::List && (*piece == 0 || p != WidgetKind::Button) {
                        let dx = dst[0] - w.pos[0] as f32;
                        let dy = dst[1] - w.pos[1] as f32;
                        if dx.abs() > POS_TOL || dy.abs() > POS_TOL {
                            push(
                                "POS",
                                r,
                                format!("image at +{dx:.1},+{dy:.1} from the record"),
                                dx.abs().max(dy.abs()),
                            );
                        }
                        let stretched =
                            matches!(sheet.parts[*part].look, Look::Panel { stretch: true, .. });
                        let over_w = dst[2] - w.size[0] as f32;
                        let over_h = dst[3] - w.size[1] as f32;
                        if !stretched && (over_w > POS_TOL || over_h > POS_TOL) {
                            push(
                                "SIZE",
                                r,
                                format!(
                                    "image {:.0}x{:.0} larger than the record's {}x{}",
                                    dst[2], dst[3], w.size[0], w.size[1]
                                ),
                                over_w.max(over_h),
                            );
                        }
                    }
                }
                DrawItem::Label {
                    style, pos, anchor, ..
                } => {
                    if let (Some(at), Some(an)) = (w.text_at, w.text_anchor) {
                        // a word-wrapped text's later lines sit one line height (the item's height, or the part's
                        // own pitch, CLI-751) per line lower
                        // DC-DEFAULT: CLI-478 (proposed)
                        let pitch = sheet.parts[*part].wrap_pitch;
                        let line = if sheet.parts[*part].wrap > 0 {
                            *piece as f32 * if pitch > 0 { pitch as f32 } else { r.rect[3] }
                        } else {
                            0.0
                        };
                        let dx = pos[0] - at[0];
                        let dy = pos[1] - at[1] - line;
                        if dx.abs() > POS_TOL || dy.abs() > POS_TOL {
                            push(
                                "POS",
                                r,
                                format!("text anchor point at +{dx:.1},+{dy:.1} from the record's"),
                                dx.abs().max(dy.abs()),
                            );
                        }
                        if (anchor[0] - an[0]).abs() > 0.01 || (anchor[1] - an[1]).abs() > 0.01 {
                            push(
                                "ALIGN",
                                r,
                                format!("anchor {:?} for the record's alignment {:?}", anchor, an),
                                1.0,
                            );
                        }
                    }
                    if let Some(f) = w.font {
                        if style.font != f {
                            push(
                                "FONT",
                                r,
                                format!("font {} for the record's {f}", style.font),
                                1.0,
                            );
                        }
                    }
                    if let Some(s) = w.font_size {
                        if style.size != Some(s).filter(|s| *s > 0) {
                            push(
                                "FSIZE",
                                r,
                                format!("size {:?} for the record's {s}", style.size),
                                1.0,
                            );
                        }
                    }
                    if let Some(c) = w.colour {
                        let p = &sheet.parts[*part];
                        // a text whose colour the screen set at run time (`Sheet::set_colour`) is still reported:
                        // the gate exempts only the layout keys its baseline lists with evidence (`run_colour`:
                        // the character info's numbers and grade name), so a new `set_colour` caller shows up
                        // DC-DEFAULT: CLI-478 (proposed), CLI-537 (proposed)
                        let normal = match p.kind {
                            WidgetKind::Text => true,
                            WidgetKind::Button => p.enabled && !p.hover && !p.down,
                            _ => false,
                        };
                        if normal && style.colour != argb(c) {
                            let why = if p.colour_set {
                                " (set at run time by the screen)"
                            } else {
                                ""
                            };
                            push(
                                "COLOUR",
                                r,
                                format!(
                                    "colour {:?} for the record's {:?}{why}",
                                    style.colour,
                                    argb(c)
                                ),
                                1.0,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        // every text: its font index resolves to a record with its own face
        if let Some((f, _)) = text_font(&r.item) {
            match fonts.slot(f) {
                _ if fonts.slots.is_empty() => {}
                None => push("RANGE", r, format!("font index {f} has no record"), 1.0),
                Some(s) if s as i32 != f => push(
                    "RANGE",
                    r,
                    format!("font index {f} has no record (slot 0 used)"),
                    1.0,
                ),
                Some(s) if !fonts.own_file(s) => {
                    let file_less = fonts.slots[s].as_ref().is_some_and(|x| x.file.is_none());
                    if file_less && fonts.system_face.is_none() {
                        // no installed face was found at all: one line per screen below, not one per text
                        no_system_face = true;
                    } else {
                        push("FACE", r, format!("font {f}: neither its file nor its installed face and weight draws it (drawn with the menu's default font)"), 1.0);
                    }
                }
                _ => {}
            }
        }
    }
    // texts drawn over by a later image's opaque texels, and texts drawn over other texts
    for (i, a) in rows.iter().enumerate() {
        // the screen under a box is covered by the box on purpose (w3-options, CLI-560)
        if !is_text(&a.item) || a.rect[2] <= 0.0 || a.origin == Origin::Under {
            continue;
        }
        let area = a.rect[2] * a.rect[3];
        for b in &rows[i + 1..] {
            match &b.item {
                DrawItem::Image {
                    tex,
                    src,
                    dst,
                    tint,
                    ..
                } if tint[3] > 0 => {
                    // a record panel stretched over its rectangle is a blind (the modal dimming window): it dims
                    // what is under it on purpose; the mouse cursor is drawn over everything wherever the pointer
                    // is (the 2009 shell draws it after the screen), so a text under it is not covered by the layout
                    // DISASM: cshell+0x1ef530 (the cursor image drawn after the screen)
                    // DC-DEFAULT: CLI-538 (proposed)
                    let blind = match b.origin {
                        Origin::Part { part, .. } => {
                            matches!(sheet.parts[part].look, Look::Panel { stretch: true, .. })
                        }
                        Origin::Cursor => true,
                        _ => false,
                    };
                    if blind || intersect(a.rect, *dst) <= 0.0 {
                        continue;
                    }
                    let cover = opaque_share(cat, tex, *src, *dst, a.rect);
                    if cover > 0.5 {
                        push(
                            "COVERED",
                            a,
                            format!(
                                "text {:.0}% under opaque texels of the image drawn at seq {} ({})",
                                cover * 100.0,
                                b.seq,
                                key_of(b)
                            ),
                            cover,
                        );
                        break;
                    }
                }
                item if is_text(item) && b.rect[2] > 0.0 => {
                    let both = intersect(a.rect, b.rect);
                    let smaller = area.min(b.rect[2] * b.rect[3]);
                    if both / smaller > 0.3 {
                        push(
                            "OVERLAP",
                            a,
                            format!(
                                "text overlaps the text at seq {} ({}) by {:.0}%",
                                b.seq,
                                key_of(b),
                                both / smaller * 100.0
                            ),
                            both / smaller,
                        );
                    }
                }
                _ => {}
            }
        }
        // a text record without a string of its own that the screen filled at run time
        if let (Origin::Part { part, .. }, Some(w)) = (&a.origin, &a.want) {
            if w.kind == WidgetKind::Text && !w.rom_text && !sheet.parts[*part].text.is_empty() {
                push(
                    "FILL",
                    a,
                    "a text record without a string of its own drew run-time text".into(),
                    1.0,
                );
            }
        }
    }
    // records of shown controls that drew nothing although they have something to draw
    let drawn: Vec<usize> = rows
        .iter()
        .filter_map(|r| match r.origin {
            Origin::Part { part, .. } => Some(part),
            _ => None,
        })
        .collect();
    for i in order {
        let p = &sheet.parts[i];
        let key = format!(
            "p{}/r{}/{:?}/w{}/id{}",
            p.page.index(),
            p.rec,
            p.kind,
            p.layer,
            p.id
        );
        let has = match &p.look {
            Look::Panel { tex, .. } => !tex.is_empty(),
            Look::Button {
                prefix, caption, ..
            } => !prefix.is_empty() || caption.is_some(),
            _ => false,
        };
        if has && !drawn.contains(&i) {
            out.push(Mismatch {
                class: "MISSING",
                key,
                seq: usize::MAX,
                detail: format!(
                    "a shown control at {:?} with a texture or caption drew nothing",
                    p.pos
                ),
                delta: 1.0,
            });
        } else if p.kind == WidgetKind::Text && !drawn.contains(&i) {
            // a shown text control that drew nothing: a string of its own that came out empty, or a run-time
            // text the screen never filled
            let rom = match cat.pages.page(p.page).and_then(|pg| pg.widgets.get(p.rec)) {
                Some(w) => matches!(&w.widget, Widget::Text(r) if r.text_style > 0),
                None => false,
            };
            out.push(Mismatch {
                class: if rom { "MISSING" } else { "EMPTY" },
                key,
                seq: usize::MAX,
                detail: if rom {
                    format!(
                        "a shown text control at {:?} with a string of its own drew nothing",
                        p.pos
                    )
                } else {
                    format!(
                        "a shown text control at {:?} the screen never filled (run-time text)",
                        p.pos
                    )
                },
                delta: 1.0,
            });
        }
    }
    if no_system_face {
        out.push(Mismatch {
            class: "SYSFACE",
            key: "fonts".into(),
            seq: usize::MAX,
            detail: "system face unavailable: the host has no installed face for the file-less font records, their                      texts are drawn with the ROM default font"
                .into(),
            delta: 1.0,
        });
    }
    // the open drop-down list: its frame where the 2009 rule puts it, drawn over the whole screen (only the box and
    // the cursor after it), with its rows
    // EVIDENCE: cshell+0x33c350 / cshell+0x33c620 (frame and rows), cshell+0x15ca90 (the list is added to the screen
    // after its pages: drawn over them)
    // DC-DEFAULT: CLI-505 (proposed) the draw-list class DROP: frame more than 1 px off the rule, a screen item drawn
    // after the list, or an open list without rows
    if let Some(first) = rows
        .iter()
        .position(|r| matches!(r.origin, Origin::Dropdown { .. }))
    {
        let fr = &rows[first];
        if let Some(w) = fr.frame_want {
            // the frame is drawn in nine texture parts: the union of the leading items of its texture
            let base = |r: &TraceRow| match &r.item {
                DrawItem::Image { tex, .. } => {
                    Some(tex.split('#').next().unwrap_or("").to_string())
                }
                _ => None,
            };
            let frame_tex = base(fr);
            let rect = rows[first..]
                .iter()
                .take_while(|r| frame_tex.is_some() && base(r) == frame_tex)
                .fold(None::<[f32; 4]>, |u, r| {
                    Some(match u {
                        None => r.rect,
                        Some(u) => {
                            let (x0, y0) = (u[0].min(r.rect[0]), u[1].min(r.rect[1]));
                            let x1 = (u[0] + u[2]).max(r.rect[0] + r.rect[2]);
                            let y1 = (u[1] + u[3]).max(r.rect[1] + r.rect[3]);
                            [x0, y0, x1 - x0, y1 - y0]
                        }
                    })
                })
                .unwrap_or(fr.rect);
            let d = (0..4)
                .map(|i| (rect[i] - w[i]).abs())
                .fold(0.0f32, f32::max);
            if d > POS_TOL {
                out.push(Mismatch {
                    class: "DROP",
                    key: key_of(fr),
                    seq: fr.seq,
                    detail: format!("list frame {:?} for the rule's {:?}", rect, w),
                    delta: d,
                });
            }
        }
        for r in &rows[first..] {
            if matches!(
                r.origin,
                Origin::Part { .. } | Origin::Extra | Origin::Tiles | Origin::Backdrop
            ) {
                out.push(Mismatch {
                    class: "DROP",
                    key: key_of(r),
                    seq: r.seq,
                    detail: "a screen item drawn over the open list".into(),
                    delta: 1.0,
                });
            }
        }
        let texts = rows
            .iter()
            .filter(|r| matches!(r.origin, Origin::Dropdown { .. }) && is_text(&r.item))
            .count();
        if texts == 0 {
            out.push(Mismatch {
                class: "DROP",
                key: key_of(fr),
                seq: fr.seq,
                detail: "open list without rows".into(),
                delta: 1.0,
            });
        }
    }
    // the 2009 shell draws its own mouse cursor image over the front end (the shell loads it from the logo
    // archive through an engine service); the menu's draw list must end with it
    // EVIDENCE: cshell+0x1bf010, cshell+0x1ef3a0 (SPEC FMT_SPR_LTC_FX.md §9.4: the cursor image, a PCX of the logo
    // archive)
    // DISASM: cshell+0x1ef530 (the image drawn after the screen: nothing covers it)
    let cursors = rows.iter().filter(|r| r.origin == Origin::Cursor).count();
    if cursors == 0 {
        out.push(Mismatch {
            class: "CURSOR",
            key: "cursor".into(),
            seq: usize::MAX,
            detail: "no cursor image in the draw list (the 2009 shell draws its own cursor)".into(),
            delta: 1.0,
        });
    } else if cursors > 1 || rows.last().is_none_or(|r| r.origin != Origin::Cursor) {
        out.push(Mismatch {
            class: "CURSOR",
            key: "cursor".into(),
            seq: usize::MAX,
            detail: "the cursor image is not the single last item of the draw list".into(),
            delta: 1.0,
        });
    }
    out
}

/// The share of `rect` (virtual pixels) under texels of `tex` with alpha of at least one half, the texture's
/// `src` texels stretched over `dst`.
fn opaque_share(cat: &mut Catalog, tex: &str, src: [f32; 2], dst: [f32; 4], rect: [f32; 4]) -> f32 {
    let Some(t) = cat.assets.texture(tex) else {
        return 0.0;
    };
    if dst[2] <= 0.0 || dst[3] <= 0.0 {
        return 0.0;
    }
    let (mut hit, mut all) = (0u32, 0u32);
    let mut y = rect[1] + 0.5;
    while y < rect[1] + rect[3] {
        let mut x = rect[0] + 0.5;
        while x < rect[0] + rect[2] {
            all += 1;
            let u = (x - dst[0]) / dst[2] * src[0];
            let v = (y - dst[1]) / dst[3] * src[1];
            if u >= 0.0 && v >= 0.0 && u < src[0] && v < src[1] {
                let (tx, ty) = (u as u32, v as u32);
                if tx < t.width && ty < t.height {
                    let a = t.rgba[((ty * t.width + tx) * 4 + 3) as usize];
                    if a >= 128 {
                        hit += 1;
                    }
                }
            }
            x += 1.0;
        }
        y += 1.0;
    }
    if all == 0 {
        0.0
    } else {
        hit as f32 / all as f32
    }
}

/// A mismatch as JSON.
pub fn mismatch_json(screen: &str, m: &Mismatch) -> Value {
    json!({
        "screen": screen, "class": m.class, "layout_key": m.key,
        "seq": (m.seq != usize::MAX).then_some(m.seq), "detail": m.detail, "delta": (m.delta * 100.0).round() / 100.0,
    })
}

impl Menu {
    /// Puts `p` on top of the screen (tools that trace a message box over a screen).
    pub fn show_popup(&mut self, p: super::popup::Popup) {
        self.popups.insert(0, p);
        self.dirty = true;
    }
}
