//! The options screen: four pages (graphic, sound, control, quick enter) behind tab buttons, opened over the
//! screen the top bar's option button was pressed on, closed by OK (the settings take effect and the profile is
//! saved), Cancel / the close button / Escape (nothing changes).
//!
//! The frame page and each tab's page are ROM layout pages drawn through the sheet. The rows of a tab are built
//! the 2009 way from the tab screen's layout section (element count, then per element its type, height, index,
//! command index, spacer and offset) as controls of the sheet: on / off buttons, sliders with minus / plus, the
//! resolution dropdown and key boxes. The rows whose knob DeCombat has a use for change the settings; the others are
//! drawn and stored (DC-DEFAULT, inert).
// EVIDENCE: cshell+0x1ac4b0 (option button -> screen 0x18), cshell+0x1149a0 (tabs 1000..1003 -> screens 0x18..0x1b,
// 6 OK -> settings applied + back, 7 cancel / close -> pages restored + back), cshell+0x114160 (the open tab's
// button state), cshell+0x1140c0 (every tab loads the frame page first), cshell+0x117ad0 / cshell+0x151510 /
// cshell+0x112200 / cshell+0x12f590 (graphic, sound, control, quick enter pages: rows from the layout section, row
// width 0x1a7), cshell+0xf1e10 / cshell+0xf25f0 (row controls by element type), cshell+0x151af0 (sound rows: 1 music
// on / off, 2 music volume, 3 effects on / off, 4 effects volume), cshell+0x112800 (control rows: 3 invert, 4
// sensitivity), cshell+0x117100 (graphic row 1: the resolution list, each mode drawn with string 0x3d9)
// OBSERVED: the user's ROM pages (tag 13 the frame: blind, window, tab buttons 1000..1003, close 7, OK 6, cancel 7;
// tags 14..17 the graphic, sound, control and quick enter pages: window 2 with a reset button 39 and the rows' list)
// DC-DEFAULT: CLI-556 (proposed) the controls' places inside a row, the slider range and step, the dropdown as a
// cycling box, key capture, list scrolling, the inert rows; see DC_DEFAULTS.md

use super::catalog::{prefix_of_a, Catalog, OverlayPaths};
use super::dropdown::{self, Dropdown};
use super::sheet::{place_text, Look, Part, Sheet};
use super::{build_sheet, Ctx, Go, MenuData, MenuKey, Screen};
use crate::hud::compose::part_of;
use crate::hud::items::DrawItem;
use crate::settings::{
    scan_name, Renderer, Settings, WinStyle, ANTIALIAS_CHOICES, BGM_CHOICES, BGM_CLASSIC, FPS_CAP_CHOICES,
    RENDERER_CHOICES,
    GAMMA_MAX, SENSITIVITY_MAX,
};
use dc_ui_data::pages::Widget;
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::{PageTag, UiScreen, WidgetKind};

pub const NAME: &str = "options";

/// The frame page and the four tab pages, by tag index.
// OBSERVED: the page table of the user's ROM (tag 13 the options frame, 14 graphic, 15 sound, 16 control, 17 quick
// enter)
pub const FRAME_PAGE: PageTag = PageTag::Page13;
pub const TAB_PAGES: [PageTag; 4] = [
    PageTag::Page14,
    PageTag::Page15,
    PageTag::Page16,
    PageTag::Page17,
];
/// The tabs' screens (their layout sections hold the rows).
// EVIDENCE: cshell+0x1149a0 (screens 0x18 graphic, 0x19 sound, 0x1a control, 0x1b quick enter)
pub const TAB_SCREENS: [UiScreen; 4] = [
    UiScreen::Screen24,
    UiScreen::Screen25,
    UiScreen::Screen26,
    UiScreen::Screen27,
];
pub const TAB_NAMES: [&str; 4] = ["graphic", "sound", "control", "quick"];

pub const CMD_OK: i32 = 6;
pub const CMD_CANCEL: i32 = 7;
pub const CMD_RESET: i32 = 39;
/// The quick enter page's check buttons: three on / off pairs from this command (the fourth pair, spectator mode,
/// stays hidden).
// OBSERVED: the user's ROM page tag 17 (check buttons 2000..2007, two per row)
pub const QUICK_FIRST: i32 = 2000;
pub const QUICK_PAIRS: i32 = 3;
pub const CMD_TABS: [i32; 4] = [1000, 1001, 1002, 1003];
/// The frame window and the tab page's window and list.
pub const FRAME_WINDOW: i32 = 1;
pub const PAGE_WINDOW: i32 = 2;
pub const LIST: i32 = 1;
/// A row's width.
// EVIDENCE: cshell+0x151510, cshell+0x112200 (rows built 0x1a7 wide)
pub const ROW_WIDTH: i32 = 0x1a7;

/// Element types of the rows.
// EVIDENCE: cshell+0xf1e10 (1 dropdown, 2 two buttons, 3 three buttons, 4 slider, 9 four buttons), cshell+0xf25f0
// (6 one key box, 7 two key boxes for the command row and the next one, 8 one key box for the next column)
pub const TYPE_DROPDOWN: i32 = 1;
pub const TYPE_SLIDER: i32 = 4;
pub const TYPE_KEYS_ONE: i32 = 6;
pub const TYPE_KEYS_TWO: i32 = 7;
pub const TYPE_KEYS_RIGHT: i32 = 8;

/// Control sizes of a row.
// EVIDENCE: cshell+0xf1e10 (dropdown 0x73 x 0x17, slider line 200 x 8, knob 0x15 x 0x15, minus / plus 0x11 x 0x11),
// cshell+0xf25f0 (key box 0x41 x 0x17)
pub const DROPDOWN_SIZE: [i32; 2] = [0x73, 0x17];
pub const LINE_SIZE: [i32; 2] = [200, 8];
pub const KNOB_SIZE: [i32; 2] = [0x15, 0x15];
pub const STEP_SIZE: [i32; 2] = [0x11, 0x11];
pub const KEY_BOX: [i32; 2] = [0x41, 0x17];

/// Where the controls sit inside a row (x from the row's left, y from the row's top). The rows' labels, section
/// headers and the choices' words are part of the list's back image or the page's window image, so the controls
/// sit where that image leaves room for them.
// OBSERVED: screens-sweep/cpp/options/ (the reference client's graphic, sound and control
// pages: controls from 126 px right of the row's left; two / three choices 72 px apart, four 52 px apart, one pixel
// below the row's top; slider knob 5, line 12, minus / plus 22 px below the row's top, minus at the line's left,
// plus at its right end; value 0xde px right of the line's left (cshell+0xf1e10 stores 0xde, 4 in the row); key
// boxes at 142 and 331)
// DC-DEFAULT: CLI-735 (proposed) the measured places above stand for the row control's own layout (not traced)
pub const CTRL_X: i32 = 126;
pub const CHOICE_Y: i32 = 1;
pub const CHOICE_STEP: i32 = 72;
pub const CHOICE_STEP_FOUR: i32 = 52;
pub const KNOB_Y: i32 = 5;
pub const LINE_Y: i32 = 12;
pub const STEP_Y: i32 = 22;
pub const VALUE_AT: [i32; 2] = [0xde, 16];
pub const DROPDOWN_TEXT_X: i32 = 53;
pub const KEY_X: [i32; 2] = [142, 331];
/// Text look of the rows: the values in the boxes are orange.
// DC-DEFAULT: CLI-556 (proposed) font 14 size 14 (the pages' common text font), selected key box white
// OBSERVED: the reference stack pages' value texts (0xffffc060)
pub const FONT: i32 = 14;
pub const SIZE: i32 = 14;
pub const COLOUR: u32 = 0xffdd_dddd;
pub const VALUE_COLOUR: u32 = 0xffff_c060;
pub const COLOUR_ACTIVE: u32 = 0xffff_ffff;
/// The graphic page's lists: 1 the short list (the quality presets), 2 the full list (user control).
// EVIDENCE: cshell+0x117ad0 (list 1 of window 2 takes the first count's rows, list 2 the second count's),
// cshell+0x116e20 (one of the two shown), cshell+0x1168c0 / cshell+0x1173f0 (the full list for quality 3)
pub const LIST_FULL: i32 = 2;
/// The quality row of the graphic page and its user-control choice (the fourth button).
// EVIDENCE: cshell+0x115100 (row 5: setting 3 = the fourth button), cshell+0x1168c0 (setting 3 shows the full list)
// DC-DEFAULT: CLI-735 (proposed) user control is the quality row's default (the reference stack's page shows it so; the presets
// change nothing here)
pub const QUALITY_KEY: &str = "graphic.5";
pub const QUALITY_USER: i32 = 3;
/// List scrolling by Up / Down (pixels).
// DC-DEFAULT: CLI-556 (proposed)
pub const SCROLL_STEP: i32 = 22;
/// Command numbers of the row controls (they have no 2009 command): row r, control c -> BASE + 16 r + c.
// DC-DEFAULT: CLI-556 (proposed)
pub const ROW_CMD_BASE: i32 = 50_000;
/// The control-page labels of invert and sensitivity, the graphic page's resolution label and form, on / off.
// EVIDENCE: cshell+0x117100 (string 0x3d9 formats each mode with width and height)
// OBSERVED: the user's string table (352 invert Y, 353 mouse sensitivity, 381 resolution, 51 / 52 on / off, 339 /
// 340 / 341 / 342 music, music volume, sound effects, effects volume)
pub mod strings {
    pub const INVERT: i32 = 352;
    pub const SENSITIVITY: i32 = 353;
    pub const RESOLUTION: i32 = 381;
    pub const MODE_FORM: i32 = 0x3d9;
    pub const ON: i32 = 51;
    pub const OFF: i32 = 52;
    pub const MUSIC: i32 = 339;
    pub const MUSIC_VOLUME: i32 = 340;
    pub const EFFECTS: i32 = 341;
    pub const EFFECTS_VOLUME: i32 = 342;
}

/// The resolution list (the windowed look's size, CLI-1280); a saved size not in it (the first launch's monitor
/// size) is one more entry at the end.
// DC-DEFAULT: CLI-1283 (proposed) a fixed list (the 2009 list holds the adapter's modes); the row sets the
// resolution only (the look is Alt+Enter: the Graphic tab's rows come from the ROM layout, which has no look row)
pub const RESOLUTIONS: [(u32, u32); 8] = [
    (1024, 768),
    (1280, 720),
    (1280, 1024),
    (1366, 768),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
];

/// Our rows appended to the Graphic tab (CLI-1595 / CLI-1596): element indices past the ROM's, their geometry
/// (the ROM rows' dropdown and choice rows: 22 and 18 high, 13 blank) and where their labels and words go (the ROM
/// rows' labels sit 24 px into the list in the back image; the choice words a little right of their buttons).
pub const ROW_FRAME_RATE: i32 = 100;
pub const ROW_SHOW_FPS: i32 = 101;
/// CLI-1599: our "BGM Music" row of the Sound tab (after the music switch).
pub const ROW_BGM: i32 = 102;
/// CLI-1845: the gap above our key row, as between the ROM's key rows.
const KEY_ROW_GAP: i32 = 8;
/// CLI-1845: our weapon inspect key on the Control tab.
pub const ROW_INSPECT: i32 = 103;
/// CLI-1852: our "Renderer" row of the Graphic tab (Windows only), after "Show FPS".
pub const ROW_RENDERER: i32 = 104;
pub const OWN_LABEL_X: i32 = 24;
pub const OWN_WORD_DX: i32 = STEP_SIZE[0] + 8;
pub const OWN_ROW_BLANK: i32 = 13;
/// The drop-down lists of the Graphic tab's rows (the draw list's tags).
pub const LIST_RESOLUTION: &str = "opt_resolution";
pub const LIST_ANTIALIAS: &str = "opt_antialias";
pub const LIST_FRAME_RATE: &str = "opt_frame_rate";
pub const LIST_BGM: &str = "opt_bgm";
pub const LIST_RENDERER: &str = "opt_renderer";

/// The anti-aliasing row's word for a sample count (CLI-1594: the 2009 list named the device's multisample types).
pub fn antialias_text(samples: u32) -> String {
    match samples {
        0 | 1 => "Off".to_string(),
        n => format!("{n}x"),
    }
}

/// The frame-rate row's word for a cap (CLI-1595; 0 = unlimited).
/// CLI-1599: the BGM row's words.
pub fn bgm_text(style: u32) -> String {
    if style == BGM_CLASSIC {
        "Classic".into()
    } else {
        "2009".into()
    }
}

/// CLI-1852: the Renderer row's word for a choice.
pub fn renderer_text(r: Renderer) -> &'static str {
    r.label()
}

/// CLI-1852: our rows appended to the Graphic tab, in order; the Renderer row only where `choosable` (Windows).
fn graphic_own_rows(choosable: bool) -> Vec<(i32, i32, i32, Knob)> {
    let mut rows = vec![
        (ROW_FRAME_RATE, TYPE_DROPDOWN, DROPDOWN_SIZE[1], Knob::FrameRate),
        (ROW_SHOW_FPS, 2, STEP_SIZE[1] + 1, Knob::ShowFps),
    ];
    if choosable {
        rows.push((ROW_RENDERER, TYPE_DROPDOWN, DROPDOWN_SIZE[1], Knob::Renderer));
    }
    rows
}

pub fn frame_rate_text(cap: u32) -> String {
    match cap {
        0 => "Unlimited".to_string(),
        crate::settings::FPS_VSYNC => "VSync".to_string(),
        n => n.to_string(),
    }
}

/// The resolution list's entries for the edited setting `cur` (the list, and `cur` when it is not in it).
pub fn resolution_entries(cur: Option<(u32, u32)>) -> Vec<(u32, u32)> {
    let mut v = RESOLUTIONS.to_vec();
    if let Some(c) = cur.filter(|c| !v.contains(c)) {
        v.push(c);
    }
    v
}

/// A knob a row changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Knob {
    MusicOn,
    MusicVolume,
    EffectsOn,
    EffectsVolume,
    Invert,
    Sensitivity,
    /// CLI-1140: the Graphic tab's gamma slider (row 3), live.
    Gamma,
    /// The resolution list (with the bordered window as its last choice).
    Resolution,
    /// CLI-1594: the Graphic tab's anti-aliasing row (row 4): the multisample count, a real list.
    Antialias,
    /// CLI-1595: our frame-rate row (appended to the Graphic tab): the presentation cap, a real list.
    FrameRate,
    /// CLI-1596: our "Show FPS" row (appended to the Graphic tab): an on / off pair.
    ShowFps,
    /// CLI-1852: our "Renderer" row (appended to the Graphic tab, Windows only): Vulkan / DirectX 12, a real list;
    /// the choice applies at the next start of the client.
    Renderer,
    /// CLI-1599: our "BGM Music" row (after the Sound tab's music switch): Classic / 2009, a real list.
    Bgm,
    /// Key boxes: the bindable command table rows of the boxes.
    Keys(Vec<usize>),
    /// CLI-1845: our key box for a command with no row in the table.
    OwnKey(i32),
    /// Drawn and stored, no effect here (`<page>.<element index>`).
    // DC-DEFAULT: CLI-556 (proposed)
    Inert(String),
}

/// One row of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: i32,
    pub index: i32,
    /// Absolute top-left before scrolling, height.
    pub at: [i32; 2],
    pub height: i32,
    /// The blank row after it (the list's range counts it).
    // EVIDENCE: cshell+0x117ad0 (a blank row of the element's blank height added after the element)
    pub blank: i32,
    pub knob: Option<Knob>,
    /// The sheet parts of its controls.
    pub parts: Vec<usize>,
}

/// The rows of tab `tab` from its screen's layout section (empty without a ROM section); `full`: the graphic page's
/// full list (its second count).
pub fn rows_of(cat: &Catalog, tab: usize, origin: [i32; 2], full: bool) -> Vec<Row> {
    let names = &cat.paths.overlay;
    let count = if full {
        &names.element_all_count
    } else {
        &names.element_count
    };
    let (Some(count_key), Some(sec)) = (
        count.as_deref(),
        cat.layout
            .as_ref()
            .and_then(|l| l.screen_section(TAB_SCREENS[tab])),
    ) else {
        return Vec::new();
    };
    let num = |k: usize, n: i32| -> Option<i32> {
        let key = format!("{}{n}", names.element_keys[k].as_deref()?);
        sec.int(&key)
    };
    let offset = |n: i32| -> [i32; 2] {
        names.element_keys[5]
            .as_deref()
            .and_then(|k| sec.pair(&format!("{k}{n}")))
            .unwrap_or([0, 0])
    };
    let mut rows = Vec::new();
    let mut y = origin[1];
    let count = sec.int(count_key).unwrap_or(0).clamp(0, 64);
    for n in 0..count {
        let kind = num(0, n).unwrap_or(0);
        let height = num(1, n).unwrap_or(0).max(0);
        let index = num(2, n).unwrap_or(0);
        let cmd = num(3, n).unwrap_or(-1);
        let off = offset(n);
        let knob = knob_of(tab, kind, index, cmd);
        let blank = num(4, n).unwrap_or(0).max(0);
        rows.push(Row {
            kind,
            index,
            at: [origin[0] + off[0], y + off[1]],
            height,
            blank,
            knob,
            parts: Vec::new(),
        });
        y += height + blank;
    }
    rows
}

/// What row `index` of tab `tab` changes.
pub fn knob_of(tab: usize, kind: i32, index: i32, cmd: i32) -> Option<Knob> {
    if kind == 0 || kind == 5 {
        return None;
    }
    // EVIDENCE: cshell+0x151af0 (sound), cshell+0x112800 (control), cshell+0x117100 (graphic row 1)
    let k = match (tab, index) {
        (1, 1) => Knob::MusicOn,
        (1, 2) => Knob::MusicVolume,
        (1, 3) => Knob::EffectsOn,
        (1, 4) => Knob::EffectsVolume,
        (2, 3) => Knob::Invert,
        (2, 4) => Knob::Sensitivity,
        (0, 1) => Knob::Resolution,
        // OBSERVED: the reference stack's Graphic tab, row 3 = the Gamma slider (value 22 at the 2009 default 1.22)
        (0, 3) => Knob::Gamma,
        // OBSERVED: the Graphic tab's back image (Graph_bg): row 4 is labelled Anti-aliasing (a dropdown)
        // DC-DEFAULT: CLI-1594 (proposed, playtest report "anti-aliasing only shows 0,1,2")
        (0, 4) => Knob::Antialias,
        _ if (TYPE_KEYS_ONE..=TYPE_KEYS_RIGHT).contains(&kind) && cmd >= 0 => {
            let c = cmd as usize;
            Knob::Keys(match kind {
                TYPE_KEYS_TWO => vec![c, c + 1],
                _ => vec![c],
            })
        }
        _ => Knob::Inert(format!("{}.{index}", TAB_NAMES[tab])),
    };
    Some(k)
}

/// A key box's command: through the table's row, or our own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyRef {
    Row(usize),
    Command(i32),
}

/// The key boxes of a row.
fn key_refs(knob: &Option<Knob>) -> Option<Vec<KeyRef>> {
    match knob {
        Some(Knob::Keys(rows)) => Some(rows.iter().map(|r| KeyRef::Row(*r)).collect()),
        Some(Knob::OwnKey(c)) => Some(vec![KeyRef::Command(*c)]),
        _ => None,
    }
}

fn key_boxes(knob: &Option<Knob>) -> Option<usize> {
    key_refs(knob).map(|k| k.len())
}

/// The number of choice buttons of a row type (0: not a choice row).
pub fn choices(kind: i32) -> usize {
    match kind {
        2 => 2,
        3 => 3,
        9 => 4,
        _ => 0,
    }
}

pub struct Options {
    sheet: Sheet,
    tab: usize,
    /// The values being edited (the settings in effect stay until OK).
    edit: Settings,
    rows: Vec<Row>,
    /// Parts of the frame and tab pages (the row controls follow them).
    base_parts: usize,
    /// The shown list of the page window (the graphic page has two) and its back image.
    list_id: i32,
    back: Option<String>,
    list_left: i32,
    list_top: i32,
    list_height: i32,
    /// The back image's width (the list's width up to its scroll bar).
    back_width: i32,
    scroll: i32,
    /// A key box waiting for a key: (row, box).
    capture: Option<(usize, usize)>,
    /// The last pointer position (a click on a slider's line takes the value under it).
    pointer: [f32; 2],
    /// The control textures (the rows are rebuilt after every change).
    art: OverlayPaths,
    /// The open drop-down list of a dropdown row (CLI-1594 / CLI-1595 / CLI-1597: a real list, not a cycling box).
    drop: Option<Dropdown>,
    /// CLI-1595: where our rows cut the list (list y from its top) and the gap they take; the back image is drawn
    /// around it.
    split: Option<(i32, i32)>,
}

impl Options {
    pub fn new(cat: &Catalog, data: &MenuData) -> Options {
        let mut o = Options {
            sheet: Sheet::default(),
            tab: 0,
            edit: data.settings.clone(),
            rows: Vec::new(),
            base_parts: 0,
            list_id: LIST,
            back: None,
            list_left: 0,
            list_top: 0,
            list_height: 0,
            back_width: 0,
            scroll: 0,
            capture: None,
            pointer: [0.0, 0.0],
            art: cat.paths.overlay.clone(),
            drop: None,
            split: None,
        };
        o.open_tab(cat, 0);
        o
    }

    /// The open tab (0 graphic, 1 sound, 2 control, 3 quick enter).
    pub fn tab(&self) -> usize {
        self.tab
    }

    /// The values being edited.
    pub fn edit(&self) -> &Settings {
        &self.edit
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The quick enter page's pairs: the first button checked for value 1, the second for 0 (the default).
    // EVIDENCE: cshell+0x12f6e0 (button 4 takes the value, button 5 its negation; the next pairs alike)
    // OBSERVED: the reference stack's quick enter page (every pair on its second button)
    fn quick_checks(&mut self) {
        for p in self.sheet.parts.iter_mut() {
            if p.kind == WidgetKind::CheckButton
                && (QUICK_FIRST..QUICK_FIRST + 2 * QUICK_PAIRS).contains(&p.command)
            {
                let first = QUICK_FIRST + (p.command - QUICK_FIRST) / 2 * 2;
                let v = self
                    .edit
                    .inert
                    .get(&format!("quick.{first}"))
                    .copied()
                    .unwrap_or(0);
                p.checked = (v == 1) == (p.command == first);
            }
        }
    }

    /// The shown list of the page window.
    pub fn list_id(&self) -> i32 {
        self.list_id
    }

    /// The list's scroll value (pixels).
    pub fn scroll(&self) -> i32 {
        self.scroll
    }

    /// A key box is waiting for a key.
    pub fn capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// The quality row's choice (button index).
    fn quality(&self) -> i32 {
        self.edit
            .inert
            .get(QUALITY_KEY)
            .copied()
            .unwrap_or(QUALITY_USER)
    }

    fn open_tab(&mut self, cat: &Catalog, tab: usize) {
        self.tab = tab;
        self.capture = None;
        self.sheet = build_sheet(cat, &[FRAME_PAGE, TAB_PAGES[tab]]);
        // the open tab's button shows its other state
        // DC-DEFAULT: CLI-556 (proposed) the open tab is drawn disabled (its fourth texture; cshell+0x114160 sets a
        // state whose texture is not traced) and takes no click
        for (i, c) in CMD_TABS.iter().enumerate() {
            if let Some(p) = self
                .sheet
                .parts
                .iter_mut()
                .find(|p| p.kind == WidgetKind::Button && p.command == *c)
            {
                p.enabled = i != tab;
            }
        }
        self.quick_checks();
        // the spectator row (its words and its pair) stays hidden
        // EVIDENCE: cshell+0x12fb10 (window 2 texts 1..3 and buttons 10, 11 shown or hidden together)
        // OBSERVED: the reference stack's quick enter page shows three rows
        // DC-DEFAULT: CLI-738 (proposed) hidden (the condition that shows it is not traced)
        if tab == 3 {
            for p in self.sheet.parts.iter_mut() {
                let row = match p.kind {
                    WidgetKind::Text => (1..=3).contains(&p.id),
                    WidgetKind::CheckButton => p.command >= QUICK_FIRST + 2 * QUICK_PAIRS,
                    _ => false,
                };
                if row && p.layer == PAGE_WINDOW && p.page == TAB_PAGES[3] {
                    p.shown = false;
                }
            }
        }
        // the page window's reset button is hidden when the page opens
        // EVIDENCE: cshell+0x113310 (the control page's focus hides window 2 control 0, the reset button)
        // OBSERVED: screens-sweep/cpp/options/ (no reset button on any of the four pages)
        // DC-DEFAULT: CLI-736 (proposed) hidden on every page (the other pages' focus code is not traced)
        for p in self.sheet.parts.iter_mut() {
            if p.kind == WidgetKind::Button && p.layer == PAGE_WINDOW && p.command == CMD_RESET {
                p.shown = false;
            }
        }
        self.base_parts = self.sheet.parts.len();
        self.lay_list(cat);
    }

    /// Shows the page's list for the current values (the graphic page: the full list for user control, else the
    /// short one), hides the other, and builds its rows from the top.
    fn lay_list(&mut self, cat: &Catalog) {
        self.list_id = if self.tab == 0 && self.quality() == QUALITY_USER {
            LIST_FULL
        } else {
            LIST
        };
        self.scroll = 0;
        let mut geometry = None;
        for p in self.sheet.parts.iter_mut().take(self.base_parts) {
            if p.kind == WidgetKind::List && p.layer == PAGE_WINDOW {
                p.shown = p.id == self.list_id;
                if let Look::List {
                    height,
                    scroll,
                    scroll_at,
                    bar,
                    ..
                } = &mut p.look
                {
                    if let Some(b) = bar {
                        b.value = 0;
                    }
                    if p.id == self.list_id {
                        geometry = Some((p.pos, *height, *scroll, scroll_at[0]));
                    }
                }
            }
        }
        let (origin, height, scrolls, bar_x) = geometry.unwrap_or(([300, 148], 435, false, 0));
        self.list_left = origin[0];
        self.list_top = origin[1];
        self.list_height = height;
        self.back_width = bar_x;
        // a scrolling list draws its record's back image (the rows' labels and headers), moved with the rows
        // EVIDENCE: cshell+0x1c9db0 (the list record's back image flag and texture)
        // OBSERVED: the reference stack's graphic and control pages (the back image's labels move with the rows; the sound
        // page's list, which does not scroll, shows none of its back image)
        // DC-DEFAULT: CLI-735 (proposed) drawn from the list's corner, as wide as the list up to its bar, as high as
        // the list
        let page = TAB_PAGES[self.tab];
        let id = self.list_id;
        self.back = if scrolls {
            cat.pages
                .page(page)
                .and_then(|pg| {
                    pg.widgets.iter().find_map(|w| match &w.widget {
                        Widget::List(l) if l.parent == PAGE_WINDOW && l.control_id == id => {
                            l.back_texture.clone()
                        }
                        _ => None,
                    })
                })
                .filter(|t| !t.is_empty())
        } else {
            None
        };
        self.rows = rows_of(cat, self.tab, origin, self.list_id == LIST_FULL);
        self.drop = None;
        self.split = None;
        // DC-DEFAULT: CLI-1595 / CLI-1596 (proposed, user request 2026-10-31 "put framerate and show fps up near
        // resolution and push the other items down") our two rows right after the anti-aliasing row of the Graphic
        // tab, labelled by us; the ROM rows after them move down by the rows' height and the back image (their
        // labels) is drawn in two pieces around the gap
        // DC-DEFAULT: CLI-1599 (proposed, user request 2026-10-31 "a setting under BGM being on or off to be BGM
        // music with a drop down for Classic or 2009") our BGM row right after the Sound tab's music switch
        let own: Vec<(i32, i32, i32, Knob)> = match self.tab {
            0 => graphic_own_rows(crate::settings::renderer_choosable()),
            1 => vec![(ROW_BGM, TYPE_DROPDOWN, DROPDOWN_SIZE[1], Knob::Bgm)],
            // DC-DEFAULT: CLI-1845 our key row after the last key row of the Control tab
            2 => vec![(
                ROW_INSPECT,
                TYPE_KEYS_ONE,
                KEY_BOX[1],
                Knob::OwnKey(crate::settings::CMD_INSPECT),
            )],
            _ => Vec::new(),
        };
        let anchor = match self.tab {
            0 => Some(Knob::Antialias),
            1 => Some(Knob::MusicOn),
            _ => None,
        };
        if !own.is_empty() {
            let at = if self.tab == 2 {
                self.rows
                    .iter()
                    .rposition(|r| matches!(r.knob, Some(Knob::Keys(_))))
            } else {
                self.rows.iter().position(|r| r.knob == anchor)
            };
            if let Some(at) = at {
                // the key rows' own spacing before our key row
                if self.tab == 2 {
                    self.rows[at].blank += KEY_ROW_GAP;
                }
                let y0 = self.rows[at].at[1] + self.rows[at].height + self.rows[at].blank;
                let mut y = y0;
                let mut ours = Vec::new();
                for (index, kind, height, knob) in own {
                    ours.push(Row {
                        kind,
                        index,
                        at: [origin[0], y],
                        height,
                        blank: OWN_ROW_BLANK,
                        knob: Some(knob),
                        parts: Vec::new(),
                    });
                    y += height + OWN_ROW_BLANK;
                }
                let gap = y - y0;
                for r in self.rows.iter_mut().skip(at + 1) {
                    r.at[1] += gap;
                }
                let tail = self.rows.split_off(at + 1);
                self.rows.extend(ours);
                self.rows.extend(tail);
                self.split = Some((y0 - origin[1], gap));
            }
        }
        self.build_rows();
    }

    /// The pieces of the back image on the screen for the list window `[scroll, scroll + height)`: `(screen y
    /// offset from the list top, image y, piece height)`; with a split the image continues below the gap of our
    /// rows.
    pub fn back_pieces(split: Option<(i32, i32)>, scroll: i32, height: i32) -> Vec<(i32, i32, i32)> {
        let scroll = scroll.max(0);
        let window = (scroll, scroll + height.max(1));
        let clip = |v0: i32, v1: i32, image_shift: i32| -> Option<(i32, i32, i32)> {
            let (a, b) = (v0.max(window.0), v1.min(window.1));
            (b > a).then_some((a - scroll, a - image_shift, b - a))
        };
        match split {
            None => clip(0, i32::MAX / 2, 0).into_iter().collect(),
            Some((at, gap)) => [clip(0, at, 0), clip(at + gap, i32::MAX / 2, gap)]
                .into_iter()
                .flatten()
                .collect(),
        }
    }

    /// The label and the words of one of our rows (the ROM rows' are in the back image).
    fn own_row_label(knob: &Knob) -> Option<(&'static str, &'static [&'static str])> {
        match knob {
            Knob::FrameRate => Some(("Frame Rate", &[])),
            Knob::ShowFps => Some(("Show FPS", &["On", "Off"])),
            Knob::Renderer => Some(("Renderer", &[])),
            Knob::Bgm => Some(("BGM Music", &[])),
            Knob::OwnKey(_) => Some(("Inspect Weapon", &[])),
            _ => None,
        }
    }

    /// After a change: the other list when the quality row moved to or from user control, else the rows again.
    fn relist(&mut self, cat: &Catalog) {
        let full = self.tab == 0 && self.quality() == QUALITY_USER;
        if full != (self.list_id == LIST_FULL) {
            self.lay_list(cat);
        } else {
            self.build_rows();
        }
    }

    /// The rows' height with their blank rows.
    fn content_height(&self) -> i32 {
        self.rows
            .iter()
            .map(|r| r.height + r.blank)
            .sum::<i32>()
            .max(0)
    }

    /// The shown list's part.
    fn list_part(&self) -> Option<usize> {
        self.sheet.find(PAGE_WINDOW, self.list_id, WidgetKind::List)
    }

    /// The row controls as sheet parts (rebuilt after every change).
    fn build_rows(&mut self) {
        self.sheet.parts.truncate(self.base_parts);
        let art = &self.art;
        let tex = |t: &Option<String>| t.clone().unwrap_or_default();
        let parent = self
            .sheet
            .parts
            .iter()
            .rposition(|p| p.kind == WidgetKind::Window && p.layer == PAGE_WINDOW);
        if let Some(back) = &self.back {
            for (i, (dy, image_y, h)) in
                Options::back_pieces(self.split, self.scroll, self.list_height)
                    .into_iter()
                    .enumerate()
            {
                let mut p = Part::synthetic(
                    WidgetKind::Image,
                    PAGE_WINDOW,
                    99 - i as i32,
                    0,
                    [self.list_left, self.list_top + dy],
                    [self.back_width, h],
                    Look::Panel {
                        tex: part_of(
                            back,
                            [
                                0,
                                image_y.max(0) as u32,
                                self.back_width.max(1) as u32,
                                h.max(1) as u32,
                            ],
                        ),
                        stretch: false,
                    },
                );
                p.parent = parent;
                self.sheet.parts.push(p);
            }
        } else if let (Some((at, gap)), Some(w)) = (self.split, parent) {
            // CLI-1599: a page whose list does not scroll paints its rows' labels on the page window's image (the
            // Sound page, CLI-735's observation): the image below our row's gap is drawn again `gap` lower, so the
            // labels keep to their controls, and the gap shows a blank strip from the image's bottom
            let win = self.sheet.parts[w].clone();
            if let Look::Panel { tex, .. } = &win.look {
                let rel = (self.list_top + at - win.pos[1]).clamp(0, win.size[1]);
                let below = (win.size[1] - rel - gap).max(0);
                let pieces = [
                    // the rows below the gap, moved down
                    ([0, rel, win.size[0], below], [win.pos[0], win.pos[1] + rel + gap]),
                    // the gap: the image's blank bottom strip
                    (
                        [0, (win.size[1] - gap).max(0), win.size[0], gap.min(win.size[1])],
                        [win.pos[0], win.pos[1] + rel],
                    ),
                ];
                for (i, (crop, pos)) in pieces.into_iter().enumerate() {
                    if crop[3] <= 0 {
                        continue;
                    }
                    let mut p = Part::synthetic(
                        WidgetKind::Image,
                        PAGE_WINDOW,
                        97 - i as i32,
                        0,
                        pos,
                        [crop[2], crop[3]],
                        Look::Panel {
                            tex: part_of(
                                tex,
                                [0, crop[1] as u32, crop[2].max(1) as u32, crop[3] as u32],
                            ),
                            stretch: false,
                        },
                    );
                    p.parent = parent;
                    self.sheet.parts.push(p);
                }
            }
        }
        for r in 0..self.rows.len() {
            let row = self.rows[r].clone();
            let mut parts = Vec::new();
            let top = row.at[1] - self.scroll;
            let visible = top >= self.list_top
                && top + row.height <= self.list_top + self.list_height.max(row.height);
            let x = row.at[0] + CTRL_X;
            let cmd = |c: i32| ROW_CMD_BASE + 16 * r as i32 + c;
            let mut push = |sheet: &mut Sheet, mut p: Part| {
                p.parent = parent;
                p.shown = visible;
                sheet.parts.push(p);
                parts.push(sheet.parts.len() - 1);
            };
            let n = choices(row.kind);
            if n > 0 {
                let value = self.choice_value(&row);
                let step = if n == 4 {
                    CHOICE_STEP_FOUR
                } else {
                    CHOICE_STEP
                };
                for c in 0..n {
                    let mut p = Part::synthetic(
                        WidgetKind::CheckButton,
                        PAGE_WINDOW,
                        100 + c as i32,
                        cmd(c as i32),
                        [x + step * c as i32, top + CHOICE_Y],
                        [STEP_SIZE[0] + 2, STEP_SIZE[1] + 2],
                        Look::Check {
                            on: tex(&art.radio_on),
                            off: tex(&art.radio_off),
                        },
                    );
                    p.checked = value == c as i32;
                    push(&mut self.sheet, p);
                }
            } else if row.kind == TYPE_SLIDER {
                let (v, max) = self.slider_value(&row);
                let step = |prefix: Option<String>, c: i32, at: i32| {
                    Part::synthetic(
                        WidgetKind::Button,
                        PAGE_WINDOW,
                        100 + c,
                        cmd(c),
                        [x + at, top + STEP_Y],
                        STEP_SIZE,
                        Look::Button {
                            prefix: prefix.unwrap_or_default(),
                            caption: None,
                            frame: None,
                        },
                    )
                };
                push(
                    &mut self.sheet,
                    step(prefix_of_a(art.minus.as_deref()), 0, 0),
                );
                let mut line = Part::synthetic(
                    WidgetKind::Button,
                    PAGE_WINDOW,
                    102,
                    cmd(2),
                    [x, top + LINE_Y],
                    LINE_SIZE,
                    Look::Panel {
                        tex: tex(&art.line),
                        stretch: false,
                    },
                );
                // the line answers clicks over the knob's height
                line.hit = Some([x, top + KNOB_Y, LINE_SIZE[0], KNOB_SIZE[1]]);
                push(&mut self.sheet, line);
                let travel = LINE_SIZE[0] - KNOB_SIZE[0];
                let kx = x + travel * v / max.max(1);
                push(
                    &mut self.sheet,
                    Part::synthetic(
                        WidgetKind::Image,
                        PAGE_WINDOW,
                        103,
                        0,
                        [kx, top + KNOB_Y],
                        KNOB_SIZE,
                        Look::Panel {
                            tex: tex(&art.knob),
                            stretch: false,
                        },
                    ),
                );
                push(
                    &mut self.sheet,
                    step(
                        prefix_of_a(art.plus.as_deref()),
                        1,
                        LINE_SIZE[0] - STEP_SIZE[0],
                    ),
                );
            } else if row.kind == TYPE_DROPDOWN {
                let at = [x, top];
                let mut frame = Part::synthetic(
                    WidgetKind::Button,
                    PAGE_WINDOW,
                    100,
                    cmd(0),
                    at,
                    DROPDOWN_SIZE,
                    Look::Panel {
                        tex: tex(&art.dropdown),
                        stretch: false,
                    },
                );
                frame.hit = Some([at[0], at[1], DROPDOWN_SIZE[0], DROPDOWN_SIZE[1]]);
                push(&mut self.sheet, frame);
                let arrow = prefix_of_a(art.dropdown_arrow.as_deref()).unwrap_or_default();
                push(
                    &mut self.sheet,
                    Part::synthetic(
                        WidgetKind::Button,
                        PAGE_WINDOW,
                        101,
                        cmd(1),
                        [at[0] + DROPDOWN_SIZE[0] - DROPDOWN_SIZE[1], at[1]],
                        [DROPDOWN_SIZE[1], DROPDOWN_SIZE[1]],
                        Look::Button {
                            prefix: arrow,
                            caption: None,
                            frame: None,
                        },
                    ),
                );
            } else if let Some(boxes) = key_boxes(&row.knob) {
                let first_col = usize::from(row.kind == TYPE_KEYS_RIGHT);
                for b in 0..boxes {
                    let col = (first_col + b).min(1);
                    let mut p = Part::synthetic(
                        WidgetKind::Button,
                        PAGE_WINDOW,
                        100 + b as i32,
                        cmd(b as i32),
                        [row.at[0] + KEY_X[col], top],
                        KEY_BOX,
                        Look::Panel {
                            tex: tex(&art.key_box),
                            stretch: false,
                        },
                    );
                    p.hit = Some([p.pos[0], p.pos[1], KEY_BOX[0], KEY_BOX[1]]);
                    push(&mut self.sheet, p);
                }
            }
            self.rows[r].parts = parts;
        }
    }

    fn choice_value(&self, row: &Row) -> i32 {
        // the on / off pair: the first button is "on" (cshell+0x151af0: value 0 sets the switch, 1 clears it)
        match &row.knob {
            Some(Knob::MusicOn) => (!self.edit.music_on) as i32,
            Some(Knob::EffectsOn) => (!self.edit.effects_on) as i32,
            Some(Knob::Invert) => (!self.edit.invert_y) as i32,
            Some(Knob::ShowFps) => (!self.edit.show_fps) as i32,
            Some(Knob::Inert(k)) if k == QUALITY_KEY => self.quality(),
            Some(Knob::Inert(k)) => self.edit.inert.get(k).copied().unwrap_or(0),
            _ => 0,
        }
    }

    fn set_choice(&mut self, row: usize, c: i32) {
        match self.rows[row].knob.clone() {
            Some(Knob::MusicOn) => self.edit.music_on = c == 0,
            Some(Knob::EffectsOn) => self.edit.effects_on = c == 0,
            Some(Knob::Invert) => self.edit.invert_y = c == 0,
            Some(Knob::ShowFps) => self.edit.show_fps = c == 0,
            Some(Knob::Inert(k)) => {
                self.edit.inert.insert(k, c);
            }
            _ => {}
        }
    }

    /// A slider's value and maximum.
    fn slider_value(&self, row: &Row) -> (i32, i32) {
        match &row.knob {
            Some(Knob::MusicVolume) => (self.edit.music_volume, 100),
            Some(Knob::EffectsVolume) => (self.edit.effects_volume, 100),
            Some(Knob::Sensitivity) => (self.edit.sensitivity, SENSITIVITY_MAX),
            Some(Knob::Gamma) => (self.edit.gamma, GAMMA_MAX),
            Some(Knob::Inert(k)) => (self.edit.inert.get(k).copied().unwrap_or(50), 100),
            _ => (0, 100),
        }
    }

    fn set_slider(&mut self, row: usize, v: i32) {
        let (_, max) = self.slider_value(&self.rows[row]);
        let v = v.clamp(0, max);
        match self.rows[row].knob.clone() {
            Some(Knob::MusicVolume) => self.edit.music_volume = v,
            Some(Knob::EffectsVolume) => self.edit.effects_volume = v,
            Some(Knob::Sensitivity) => self.edit.sensitivity = v,
            Some(Knob::Gamma) => {
                self.edit.gamma = v;
                crate::settings::publish_gamma(v);
            }
            Some(Knob::Inert(k)) => {
                self.edit.inert.insert(k, v);
            }
            _ => {}
        }
    }

    /// A click on a dropdown row's frame or arrow: its list opens under the frame (a second click closes it).
    // DC-DEFAULT: CLI-1597 (proposed, playtest report "the dropdown for screen resolution doesn't work"; amends
    // CLI-556's cycling box) the Graphic tab's dropdown rows open a real list, as the room creation box's do
    fn open_list(&mut self, cat: &Catalog, r: usize) {
        let was_open = self.drop.take().is_some();
        if was_open {
            return;
        }
        let row = &self.rows[r];
        let Some(&frame) = row.parts.first() else {
            return;
        };
        let p = &self.sheet.parts[frame];
        let anchor = [p.pos[0], p.pos[1], p.size[0], p.size[1]];
        let (list, rows): (&'static str, Vec<(String, i32)>) = match row.knob {
            Some(Knob::Resolution) => (
                LIST_RESOLUTION,
                resolution_entries(self.edit.resolution)
                    .iter()
                    .enumerate()
                    .map(|(i, &(w, h))| {
                        let mut s = self.edit.clone();
                        s.resolution = Some((w, h));
                        (Options::resolution_text(cat, &s), i as i32)
                    })
                    .collect(),
            ),
            Some(Knob::Antialias) => (
                LIST_ANTIALIAS,
                ANTIALIAS_CHOICES
                    .iter()
                    .map(|&n| (antialias_text(n), n as i32))
                    .collect(),
            ),
            Some(Knob::FrameRate) => (
                LIST_FRAME_RATE,
                FPS_CAP_CHOICES
                    .iter()
                    .map(|&n| (frame_rate_text(n), n as i32))
                    .collect(),
            ),
            Some(Knob::Renderer) => (
                LIST_RENDERER,
                RENDERER_CHOICES
                    .iter()
                    .map(|&r| (renderer_text(r).to_string(), r as i32))
                    .collect(),
            ),
            Some(Knob::Bgm) => (
                LIST_BGM,
                BGM_CHOICES
                    .iter()
                    .map(|&n| (bgm_text(n), n as i32))
                    .collect(),
            ),
            _ => return,
        };
        self.drop = Some(Dropdown::new(
            list,
            anchor,
            rows,
            dropdown::CREATE_ROW_SIZE,
            false,
        ));
    }

    /// The text of the resolution list's entry.
    // DC-DEFAULT: CLI-556 (proposed) our word for a resolution not saved yet
    pub fn resolution_text(cat: &Catalog, s: &Settings) -> String {
        match s.resolution {
            None => "Desktop".into(),
            Some((w, h)) => {
                let form = cat.text(strings::MODE_FORM);
                if form.is_empty() || form.starts_with('#') {
                    format!("{w} x {h}")
                } else {
                    form.replace("%1!d!", &w.to_string())
                        .replace("%2!d!", &h.to_string())
                        .replace("%1", &w.to_string())
                        .replace("%2", &h.to_string())
                }
            }
        }
    }

    fn row_of_command(&self, cmd: i32) -> Option<(usize, i32)> {
        let n = cmd - ROW_CMD_BASE;
        (n >= 0 && (n / 16) < self.rows.len() as i32).then_some(((n / 16) as usize, n % 16))
    }

    fn reset_tab(&mut self) {
        let d = Settings::default();
        match self.tab {
            0 => {
                // CLI-1283: the reset keeps the look and the resolution (the player's own choice, saved apart)
                self.edit.gamma = d.gamma;
                crate::settings::publish_gamma(d.gamma);
                self.edit.inert.retain(|k, _| !k.starts_with("graphic."));
                self.edit.antialias = d.antialias;
                self.edit.fps_cap = d.fps_cap;
                self.edit.show_fps = d.show_fps;
                self.edit.renderer = d.renderer;
            }
            1 => {
                self.edit.effects_volume = d.effects_volume;
                self.edit.music_volume = d.music_volume;
                self.edit.effects_on = d.effects_on;
                self.edit.music_on = d.music_on;
                self.edit.bgm = d.bgm;
            }
            2 => {
                self.edit.sensitivity = d.sensitivity;
                self.edit.invert_y = d.invert_y;
                self.edit.binds = d.binds;
            }
            _ => self.edit.inert.retain(|k, _| !k.starts_with("quick.")),
        }
    }

    /// CLI-1030: a sound row's change is heard at once (the music switch and volume, the effects switch and
    /// volume), before OK; OK keeps it, Cancel restores the values in effect.
    // EVIDENCE: cshell+0x151af0 (rows 1 / 2: the music manager's volume set to the music volume, or 0 with the switch
    // off, on every change; rows 3 / 4: the sound manager's effects volume likewise)
    fn hear(&self, row: &Row, ctx: &mut Ctx) {
        if matches!(
            row.knob,
            Some(Knob::MusicOn | Knob::MusicVolume | Knob::EffectsOn | Knob::EffectsVolume)
        ) {
            ctx.data.sound_live = Some(self.edit.clone());
        }
    }

    /// OK: the edited values become the settings, are saved to the profile and published.
    fn apply(&mut self, ctx: &mut Ctx) {
        let data = &mut *ctx.data;
        data.settings = self.edit.clone();
        data.settings_changed = true;
        crate::settings::publish(&data.settings);
        super::store_settings(data, ctx.cat);
    }

    fn key_text(&self, data: &MenuData, row: usize) -> String {
        data.commands
            .row(row)
            .and_then(|r| self.edit.binds.key_of(r.command))
            .map(scan_name)
            .unwrap_or_default()
    }

    /// The rows' texts: slider values, the dropdowns' entries and the key boxes (the labels and the choices' words
    /// are part of the back or window image).
    fn draw_rows(&self, cat: &mut Catalog, data: &MenuData, out: &mut Vec<DrawItem>) {
        let centre = Align {
            first: super::sheet::ALIGN_CENTRE,
            second: ALIGN_SECOND_CENTRE,
        };
        for (r, row) in self.rows.iter().enumerate() {
            let shown = row
                .parts
                .first()
                .is_some_and(|&i| self.sheet.parts[i].shown);
            if !shown {
                continue;
            }
            let top = row.at[1] - self.scroll;
            let x = row.at[0] + CTRL_X;
            let text = |t: &str, at: [i32; 2], align: Align, colour: u32| {
                place_text(t, at, [0, 0], align, FONT, SIZE, colour)
            };
            // our rows draw their own label and words (CLI-1595 / CLI-1596)
            if let Some((label, words)) = row.knob.as_ref().and_then(Options::own_row_label) {
                let left = Align {
                    first: super::sheet::ALIGN_LEFT,
                    second: ALIGN_SECOND_CENTRE,
                };
                let mid = top + row.height / 2;
                out.push(text(label, [row.at[0] + OWN_LABEL_X, mid], left, COLOUR));
                let step = if choices(row.kind) == 4 {
                    CHOICE_STEP_FOUR
                } else {
                    CHOICE_STEP
                };
                for (c, w) in words.iter().enumerate() {
                    out.push(text(w, [x + step * c as i32 + OWN_WORD_DX, mid], left, COLOUR));
                }
            }
            if row.kind == TYPE_SLIDER {
                let (v, _) = self.slider_value(row);
                out.push(text(
                    &v.to_string(),
                    [x + VALUE_AT[0], top + VALUE_AT[1]],
                    centre,
                    VALUE_COLOUR,
                ));
            } else if row.kind == TYPE_DROPDOWN {
                let t = match row.knob {
                    Some(Knob::Resolution) => Options::resolution_text(cat, &self.edit),
                    Some(Knob::Antialias) => antialias_text(self.edit.antialias),
                    Some(Knob::FrameRate) => frame_rate_text(self.edit.fps_cap),
                    Some(Knob::Renderer) => renderer_text(self.edit.renderer).to_string(),
                    Some(Knob::Bgm) => bgm_text(self.edit.bgm),
                    _ => self
                        .edit
                        .inert
                        .get(&format!("{}.{}", TAB_NAMES[self.tab], row.index))
                        .copied()
                        .unwrap_or(0)
                        .to_string(),
                };
                out.push(text(
                    &t,
                    [x + DROPDOWN_TEXT_X, top + DROPDOWN_SIZE[1] / 2],
                    centre,
                    VALUE_COLOUR,
                ));
            } else if let Some(cmds) = key_refs(&row.knob) {
                let first_col = usize::from(row.kind == TYPE_KEYS_RIGHT);
                let y = top + KEY_BOX[1] / 2;
                for (b, c) in cmds.iter().enumerate() {
                    let col = (first_col + b).min(1);
                    let waiting = self.capture == Some((r, b));
                    let k = if waiting {
                        "?".to_string()
                    } else {
                        match c {
                            KeyRef::Row(c) => self.key_text(data, *c),
                            KeyRef::Command(c) => self
                                .edit
                                .binds
                                .key_of(*c)
                                .map(scan_name)
                                .unwrap_or_default(),
                        }
                    };
                    if !k.is_empty() {
                        out.push(text(
                            &k,
                            [row.at[0] + KEY_X[col] + KEY_BOX[0] / 2, y],
                            centre,
                            if waiting { COLOUR_ACTIVE } else { VALUE_COLOUR },
                        ));
                    }
                }
            }
        }
    }

    /// Scrolls the list by `d` pixels: through its scroll bar when it has one, else bounded by its rows.
    fn scroll_by(&mut self, d: i32) {
        self.drop = None;
        let bar = self
            .list_part()
            .and_then(|i| match &mut self.sheet.parts[i].look {
                Look::List { bar: Some(b), .. } => Some(b),
                _ => None,
            });
        let s = match bar {
            Some(b) => {
                b.value = (b.value + d).clamp(0, b.top());
                b.value
            }
            None => (self.scroll + d).clamp(0, (self.content_height() - self.list_height).max(0)),
        };
        if s != self.scroll {
            self.scroll = s;
            self.build_rows();
        }
    }

    /// A key for the box waiting for one (`scan`: the key's scan code).
    pub fn bind_key(&mut self, _cat: &Catalog, data: &MenuData, scan: u8) {
        let Some((r, b)) = self.capture.take() else {
            return;
        };
        match self.rows.get(r).and_then(|x| x.knob.clone()) {
            Some(Knob::Keys(cmds)) => {
                if let Some(row) = cmds.get(b).and_then(|c| data.commands.row(*c)) {
                    self.edit.binds.bind(row.command, Some(scan));
                }
            }
            Some(Knob::OwnKey(c)) => self.edit.binds.bind(c, Some(scan)),
            _ => {}
        }
        self.build_rows();
    }
}

impl Screen for Options {
    fn name(&self) -> &'static str {
        NAME
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }
    /// The screen the option button was pressed on stays drawn under the box; the frame page's blind dims it.
    // OBSERVED: the user's ROM page tag 13 (the frame's first element is a full-screen blind);
    // screens-sweep/cpp/options/ (the reference client's options box over the dimmed lobby)
    // DC-DEFAULT: CLI-730 (proposed) the options box draws the screen under it as the report and help boxes do
    fn shows_under(&self) -> bool {
        true
    }

    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        match k {
            // Escape: a waiting key box gives up, else the screen closes as Cancel does
            // DC-DEFAULT: CLI-556 (proposed)
            MenuKey::Escape if self.capture.is_some() => {
                self.capture = None;
            }
            MenuKey::Escape => self.click(CMD_CANCEL, usize::MAX, ctx),
            MenuKey::Enter if self.capture.is_none() => self.click(CMD_OK, usize::MAX, ctx),
            MenuKey::Up => self.scroll_by(-SCROLL_STEP),
            MenuKey::Down => self.scroll_by(SCROLL_STEP),
            _ => {}
        }
    }

    /// The wheel over the options list scrolls it as the Up / Down keys do.
    // DC-DEFAULT: CLI-700 (proposed) one wheel notch moves the options list by one key step (the options pages'
    // scroll lists, Page14 / Page16 records with the scroll flag, are laid out by this screen, not by the layout list)
    fn wheel(&mut self, _p: [f32; 2], notches: i32, _ctx: &mut Ctx) -> bool {
        self.scroll_by(-notches * SCROLL_STEP);
        true
    }

    /// The shown list's rows as one block of their height: its scroll bar spans them.
    // EVIDENCE: cshell+0x2f0430 (the bar's range = the rows' heights; step = the first row's height + spacing)
    fn list_rows(&self, _data: &MenuData) -> Vec<(i32, i32, usize, i32)> {
        vec![(PAGE_WINDOW, self.list_id, self.content_height() as usize, 1)]
    }

    /// The bar's step is the first row's height; a moved bar moves the rows.
    fn lists_synced(&mut self) {
        let first = self.rows.first().map_or(1, |r| r.height.max(1));
        let value = self
            .list_part()
            .and_then(|i| match &mut self.sheet.parts[i].look {
                Look::List { bar: Some(b), .. } => {
                    b.step = first;
                    Some(b.value)
                }
                _ => None,
            });
        if let Some(v) = value {
            if v != self.scroll {
                self.scroll = v;
                self.build_rows();
            }
        }
    }

    fn scan(&mut self, scan: u8, ctx: &mut Ctx) -> bool {
        if self.capture.is_none() {
            return false;
        }
        self.bind_key(ctx.cat, ctx.data, scan);
        true
    }

    fn wants_scan(&self) -> bool {
        self.capture.is_some()
    }

    fn click(&mut self, command: i32, part: usize, ctx: &mut Ctx) {
        match command {
            CMD_OK => {
                self.apply(ctx);
                *ctx.go = Some(Go::Back);
            }
            CMD_CANCEL => {
                // the sound page's values heard while editing go back to the ones in effect
                // EVIDENCE: cshell+0x1149a0 (7: the sound page restores its values, cshell+0x151cb0, and applies them)
                ctx.data.sound_live = Some(ctx.data.settings.clone());
                // CLI-1140: the gamma moved while editing goes back too
                crate::settings::publish_gamma(ctx.data.settings.gamma);
                *ctx.go = Some(Go::Back);
            }
            CMD_RESET => {
                self.reset_tab();
                if self.tab == 1 {
                    ctx.data.sound_live = Some(self.edit.clone());
                }
                self.relist(ctx.cat);
            }
            c if CMD_TABS.contains(&c) => {
                let t = CMD_TABS.iter().position(|x| *x == c).unwrap_or(0);
                if t != self.tab {
                    self.open_tab(ctx.cat, t);
                }
            }
            c => {
                let Some((r, ctl)) = self.row_of_command(c) else {
                    return;
                };
                let row = self.rows[r].clone();
                if row.kind == TYPE_SLIDER {
                    let (v, max) = self.slider_value(&row);
                    let step = if max >= 100 { 5 } else { 1 };
                    match ctl {
                        0 => self.set_slider(r, v - step),
                        1 => self.set_slider(r, v + step),
                        // a click on the line: the value under the pointer
                        _ => {
                            if let Some(p) = self.sheet.parts.get(part) {
                                let x =
                                    self.pointer[0] - p.pos[0] as f32 - KNOB_SIZE[0] as f32 / 2.0;
                                let travel = (LINE_SIZE[0] - KNOB_SIZE[0]) as f32;
                                self.set_slider(r, (x / travel * max as f32).round() as i32);
                            }
                        }
                    }
                } else if row.kind == TYPE_DROPDOWN {
                    match row.knob {
                        Some(
                            Knob::Resolution
                            | Knob::Antialias
                            | Knob::FrameRate
                            | Knob::Renderer
                            | Knob::Bgm,
                        ) => {
                            self.open_list(ctx.cat, r);
                            return;
                        }
                        Some(Knob::Inert(k)) => {
                            let v = self.edit.inert.get(&k).copied().unwrap_or(0);
                            self.edit.inert.insert(k, (v + 1) % 4);
                        }
                        _ => {}
                    }
                } else if matches!(row.knob, Some(Knob::Keys(_) | Knob::OwnKey(_))) {
                    self.capture = Some((r, ctl as usize));
                }
                let row = self.rows[r].clone();
                self.hear(&row, ctx);
                self.build_rows();
            }
        }
    }

    fn check(&mut self, cmd: i32, _part: usize, _on: bool, ctx: &mut Ctx) {
        // the quick enter page's check buttons: on / off pairs, the first button "on"
        if (QUICK_FIRST..QUICK_FIRST + 2 * QUICK_PAIRS).contains(&cmd) {
            let first = QUICK_FIRST + (cmd - QUICK_FIRST) / 2 * 2;
            self.edit
                .inert
                .insert(format!("quick.{first}"), (cmd == first) as i32);
            self.quick_checks();
            return;
        }
        if let Some((r, c)) = self.row_of_command(cmd) {
            if choices(self.rows[r].kind) > 0 {
                self.set_choice(r, c);
                let row = self.rows[r].clone();
                self.hear(&row, ctx);
            }
        }
        self.relist(ctx.cat);
    }

    fn hover(&mut self, p: [f32; 2], _data: &MenuData) {
        self.pointer = p;
    }

    fn press(&mut self, p: [f32; 2], _ctx: &mut Ctx) -> bool {
        self.pointer = p;
        false
    }

    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, _secs: f32, out: &mut Vec<DrawItem>) {
        self.draw_rows(cat, data, out);
    }

    fn dropdown(&self) -> Option<&Dropdown> {
        self.drop.as_ref()
    }

    fn dropdown_mut(&mut self) -> Option<&mut Dropdown> {
        self.drop.as_mut()
    }

    fn close_dropdown(&mut self) {
        self.drop = None;
    }

    /// A row of an open list: the edited value takes it (OK keeps it). A resolution picked while the look is full
    /// screen switches the look to the bordered window of that size (CLI-1597: the 2009 row changed the display
    /// mode; the full-screen look here is the monitor's own size, so the window is the only look the size shows).
    fn dropdown_pick(&mut self, list: &'static str, value: i32, _index: usize, ctx: &mut Ctx) {
        match list {
            LIST_RESOLUTION => {
                let entries = resolution_entries(self.edit.resolution);
                if let Some(&e) = usize::try_from(value).ok().and_then(|i| entries.get(i)) {
                    self.edit.resolution = Some(e);
                    if self.edit.window == WinStyle::Borderless {
                        self.edit.window = WinStyle::Bordered;
                    }
                }
            }
            LIST_ANTIALIAS => {
                if let Some(n) = ANTIALIAS_CHOICES.iter().find(|&&n| n as i32 == value) {
                    self.edit.antialias = *n;
                }
            }
            LIST_FRAME_RATE => {
                if let Some(n) = FPS_CAP_CHOICES.iter().find(|&&n| n as i32 == value) {
                    self.edit.fps_cap = *n;
                }
            }
            // CLI-1852: saved with OK, applied at the next start (the device is made once per run)
            LIST_RENDERER => {
                if let Some(r) = RENDERER_CHOICES.iter().find(|&&r| r as i32 == value) {
                    self.edit.renderer = *r;
                }
            }
            // CLI-1599: a sound row: heard at once like the switches (CLI-1030); Cancel restores the style
            LIST_BGM => {
                if let Some(n) = BGM_CHOICES.iter().find(|&&n| n as i32 == value) {
                    self.edit.bgm = *n;
                    ctx.data.sound_live = Some(self.edit.clone());
                }
            }
            _ => {}
        }
        self.build_rows();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI-1595: the back image is drawn in two pieces around our rows' gap, clipped to the list window.
    #[test]
    fn the_back_image_skips_the_gap_of_our_rows() {
        // no split: one piece, the window itself
        assert_eq!(Options::back_pieces(None, 30, 400), vec![(0, 30, 400)]);
        // a gap of 66 at list y 207: the first piece ends there, the second starts 66 lower on screen at image 207
        let p = Options::back_pieces(Some((207, 66)), 0, 435);
        assert_eq!(p, vec![(0, 0, 207), (273, 207, 162)]);
        // scrolled past the gap: only the second piece, image y = screen y - 66
        let p = Options::back_pieces(Some((207, 66)), 300, 435);
        assert_eq!(p, vec![(0, 234, 435)]);
        // scrolled into the gap: the first piece is gone, the second starts under the gap
        let p = Options::back_pieces(Some((207, 66)), 220, 435);
        assert_eq!(p, vec![(53, 207, 382)]);
    }

    /// CLI-1594..1596: the anti-aliasing row names the sample counts, the frame-rate row the caps, and the Graphic
    /// tab's row 4 is the anti-aliasing knob.
    #[test]
    fn graphic_rows_name_their_choices() {
        assert_eq!(antialias_text(1), "Off");
        assert_eq!(antialias_text(4), "4x");
        assert_eq!(frame_rate_text(0), "Unlimited");
        assert_eq!(frame_rate_text(crate::settings::FPS_VSYNC), "VSync");
        assert_eq!(frame_rate_text(240), "240");
        assert_eq!(knob_of(0, TYPE_DROPDOWN, 4, -1), Some(Knob::Antialias));
        assert_eq!(knob_of(0, TYPE_DROPDOWN, 1, -1), Some(Knob::Resolution));
        assert_eq!(
            Options::own_row_label(&Knob::ShowFps).map(|(l, w)| (l, w.len())),
            Some(("Show FPS", 2))
        );
        // CLI-1599: the Sound tab's BGM row
        assert_eq!(bgm_text(BGM_CLASSIC), "Classic");
        assert_eq!(bgm_text(crate::settings::BGM_2009), "2009");
        assert_eq!(
            Options::own_row_label(&Knob::Bgm).map(|(l, w)| (l, w.len())),
            Some(("BGM Music", 0))
        );
        assert_eq!(knob_of(1, 2, 1, -1), Some(Knob::MusicOn), "the row our BGM row follows");
    }

    /// CLI-1852: the Renderer row (Vulkan / DirectX 12 list, label "Renderer") is on the Graphic tab after "Show
    /// FPS" where the build offers the choice, and absent otherwise.
    #[test]
    fn the_renderer_row_is_listed_only_where_the_choice_exists() {
        assert_eq!(renderer_text(Renderer::Vulkan), "Vulkan");
        assert_eq!(renderer_text(Renderer::Dx12), "DirectX 12");
        assert_eq!(RENDERER_CHOICES[0], Renderer::Vulkan, "Vulkan first (the default)");
        assert_eq!(
            Options::own_row_label(&Knob::Renderer).map(|(l, w)| (l, w.len())),
            Some(("Renderer", 0))
        );
        let knobs = |on: bool| -> Vec<Knob> {
            graphic_own_rows(on).into_iter().map(|r| r.3).collect()
        };
        assert_eq!(knobs(false), vec![Knob::FrameRate, Knob::ShowFps], "hidden off Windows");
        assert_eq!(knobs(true), vec![Knob::FrameRate, Knob::ShowFps, Knob::Renderer]);
        let rows = graphic_own_rows(true);
        let last = rows.last().unwrap();
        assert_eq!((last.0, last.1), (ROW_RENDERER, TYPE_DROPDOWN));
        // the build's own list follows the platform flag
        assert_eq!(
            graphic_own_rows(crate::settings::renderer_choosable()).len(),
            if cfg!(windows) { 3 } else { 2 }
        );
    }
}
