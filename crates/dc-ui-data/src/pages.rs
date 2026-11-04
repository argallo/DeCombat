//! `screen_pages`: the front-end widget layout of the 2009 client (20 pages of positional widget records).
//!
//! Parsing is **positional**, as in the client: every record line is split into whitespace tokens, the
//! first two (key and `=`) are ignored and the third is the value, so key spelling never matters. Values
//! cannot contain spaces. Each widget kind has a fixed line schema (one struct per kind below).
// SPEC: FMT_CRES_UI.md §6.3 (parser cshell+0x1d2670, page body cshell+0x1d2260, record parsers
// cshell+0x1c9b10, +0x1c8ce0, +0x1c9020, +0x1c9680, +0x1c9db0, +0x1ca250, +0x1ca5c0, +0x1caa60, +0x1cadd0,
// +0x1d09f0)

use crate::error::UiDataError;
use crate::names::UiNames;
use crate::screens::PageTag;
use dc_rez::RezVfs;

/// The client's line buffer: `fgets` with 0x104 bytes reads at most 0x103 characters per call.
/// EVIDENCE: cshell+0x1d2670
const LINE_MAX: usize = 0x103;

/// Widget kinds in the client's tag-test order. SPEC: FMT_CRES_UI.md §6.3
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WidgetKind {
    Window = 0,
    Image = 1,
    Button = 2,
    Text = 3,
    List = 4,
    Inventory = 5,
    EditBox = 6,
    CheckButton = 7,
    Lattice = 8,
    RemoteXml = 9,
}

impl WidgetKind {
    pub const ALL: [WidgetKind; 10] = [
        WidgetKind::Window,
        WidgetKind::Image,
        WidgetKind::Button,
        WidgetKind::Text,
        WidgetKind::List,
        WidgetKind::Inventory,
        WidgetKind::EditBox,
        WidgetKind::CheckButton,
        WidgetKind::Lattice,
        WidgetKind::RemoteXml,
    ];
}

/// An alignment pair (value code `A`): raw flag codes `l`=1, `r`=2, `c`=4, `t`=0x10, `b`=0x20, other 0;
/// the second letter `c`/`C` gives 0x40. EVIDENCE: cshell+0x1c6a20, +0x1c3a70
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Align {
    pub first: u32,
    pub second: u32,
}

/// Flag value of the vertical-centre code of [`Align::second`].
pub const ALIGN_SECOND_CENTRE: u32 = 0x40;

/// `d control_id, P pos, R source_rect, S texture, d stretch_mode`
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WindowRec {
    pub control_id: i32,
    pub pos: [i32; 2],
    pub source_rect: [i32; 4],
    pub texture: String,
    pub stretch_mode: i32,
}

/// `d control_id, d parent, d parent_kind, P pos, R source_rect, S texture, d stretch_mode`
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ImageRec {
    pub control_id: i32,
    pub parent: i32,
    pub parent_kind: i32,
    pub pos: [i32; 2],
    pub source_rect: [i32; 4],
    pub texture: String,
    pub stretch_mode: i32,
}

/// `d control_id, d parent, d parent_kind, d command_id, P pos, R source_rect, S texture_prefix, P text_pos,
/// A text_align, d font_index, d font_size, d-or-frame text_style, C text_colours` and, when framed,
/// `S frame_texture, P frame_pos`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ButtonRec {
    pub control_id: i32,
    pub parent: i32,
    pub parent_kind: i32,
    pub command_id: i32,
    pub pos: [i32; 2],
    pub source_rect: [i32; 4],
    /// State textures are `prefix + {A,B,C,D} + .dtx` (which letter is which state: UNPROVEN).
    pub texture_prefix: String,
    pub text_pos: [i32; 2],
    pub text_align: Align,
    pub font_index: i32,
    pub font_size: i32,
    /// Text style number (0 when the record is framed).
    pub text_style: i32,
    pub text_colours: [u32; 4],
    /// `(frame_texture, frame_pos)` of a framed button.
    pub frame: Option<(String, [i32; 2])>,
}

/// `d control_id, d parent, d parent_kind, d command_id, P pos, R rect, A align, d font_index, d font_size,
/// d text_style, C colours, d is_control`
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TextRec {
    pub control_id: i32,
    pub parent: i32,
    pub parent_kind: i32,
    pub command_id: i32,
    pub pos: [i32; 2],
    pub rect: [i32; 4],
    pub align: Align,
    pub font_index: i32,
    pub font_size: i32,
    pub text_style: i32,
    pub colours: [u32; 4],
    pub is_control: i32,
}

/// `d control_id, d parent, d parent_kind, P pos, d row_height, d item_spacing, d scroll, P scroll_offset,
/// d scroll_size, d auto_scroll, d has_back_image` and, when `has_back_image != 0`, `S back_texture`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ListRec {
    pub control_id: i32,
    pub parent: i32,
    pub parent_kind: i32,
    pub pos: [i32; 2],
    pub row_height: i32,
    pub item_spacing: i32,
    pub scroll: i32,
    pub scroll_offset: [i32; 2],
    pub scroll_size: i32,
    pub auto_scroll: i32,
    pub has_back_image: i32,
    pub back_texture: Option<String>,
}

/// `d control_id, d parent, d parent_kind, P pos, P rows_cols, P pos2, P cell_size, P cell_spacing`
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct InventoryRec {
    pub control_id: i32,
    pub parent: i32,
    pub parent_kind: i32,
    pub pos: [i32; 2],
    pub rows_cols: [i32; 2],
    pub pos2: [i32; 2],
    pub cell_size: [i32; 2],
    pub cell_spacing: [i32; 2],
}

/// `d control_id, d parent, d parent_kind, P pos, d font_index, d font_size, d max_length, d input_mode,
/// H caret_colour, d text_style, d caret_enabled, F caret_period, d visible`
#[derive(Clone, Debug, PartialEq, Default)]
pub struct EditBoxRec {
    pub control_id: i32,
    pub parent: i32,
    pub parent_kind: i32,
    pub pos: [i32; 2],
    pub font_index: i32,
    pub font_size: i32,
    pub max_length: i32,
    pub input_mode: i32,
    pub caret_colour: u32,
    pub text_style: i32,
    pub caret_enabled: i32,
    pub caret_period: f32,
    pub visible: i32,
}

/// `d control_id, d parent, d parent_kind, d command_id, P pos, R source_rect, S texture_checked,
/// S texture_unchecked`
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CheckButtonRec {
    pub control_id: i32,
    pub parent: i32,
    pub parent_kind: i32,
    pub command_id: i32,
    pub pos: [i32; 2],
    pub source_rect: [i32; 4],
    pub texture_checked: String,
    pub texture_unchecked: String,
}

/// `P pos, d rows, d cols, S texture`
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LatticeRec {
    pub pos: [i32; 2],
    pub rows: i32,
    pub cols: i32,
    pub texture: String,
}

/// `d parent, d parent_kind, P pos, R rect, S url_region_1, S url_region_2, S url_region_3`. Only the second
/// URL is used by the client, which downloads content from it; a DeCombat client must replace this block
/// with local content and never contact the URL (SPEC §10.10).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RemoteXmlRec {
    pub parent: i32,
    pub parent_kind: i32,
    pub pos: [i32; 2],
    pub rect: [i32; 4],
    pub url_regions: [String; 3],
}

/// One widget record.
#[derive(Clone, Debug, PartialEq)]
pub enum Widget {
    Window(WindowRec),
    Image(ImageRec),
    Button(ButtonRec),
    Text(TextRec),
    List(ListRec),
    Inventory(InventoryRec),
    EditBox(EditBoxRec),
    CheckButton(CheckButtonRec),
    Lattice(LatticeRec),
    RemoteXml(RemoteXmlRec),
}

impl Widget {
    pub fn kind(&self) -> WidgetKind {
        match self {
            Widget::Window(_) => WidgetKind::Window,
            Widget::Image(_) => WidgetKind::Image,
            Widget::Button(_) => WidgetKind::Button,
            Widget::Text(_) => WidgetKind::Text,
            Widget::List(_) => WidgetKind::List,
            Widget::Inventory(_) => WidgetKind::Inventory,
            Widget::EditBox(_) => WidgetKind::EditBox,
            Widget::CheckButton(_) => WidgetKind::CheckButton,
            Widget::Lattice(_) => WidgetKind::Lattice,
            Widget::RemoteXml(_) => WidgetKind::RemoteXml,
        }
    }
}

/// What went wrong on one record line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueKind {
    /// The value is not strictly of its schema form (or the line is missing): the record is not
    /// schema-conformant.
    Schema,
    /// The line is `key = value` shaped, but not as three whitespace tokens (for example a missing space
    /// after `=`): the 2009 tokeniser does not see the value and the field keeps its default. The record is
    /// still schema-conformant.
    Tokens,
}

/// A deviation inside one record: the 0-based line index after the header, the value code expected there
/// (`d P R S A C H F`, `~` for end of input) and its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineIssue {
    pub line: usize,
    pub code: char,
    pub kind: IssueKind,
}

/// A widget plus its conformance to the schema of its kind.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetRecord {
    pub widget: Widget,
    /// Every line is `key = value` with a value strictly of the schema's form.
    pub conformant: bool,
    pub issues: Vec<LineIssue>,
}

/// One page of the table.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub tag: PageTag,
    /// The page's own name (`name = "<name>"` line), when present.
    pub screen_name: Option<String>,
    pub widgets: Vec<WidgetRecord>,
}

/// The parsed table.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ScreenPages {
    pub pages: Vec<Page>,
}

impl ScreenPages {
    /// Loads and parses the table named by `names.pages_path` (decrypted by the archive layer).
    pub fn load(vfs: &RezVfs, names: &UiNames) -> Result<Self, UiDataError> {
        let bytes = crate::read_table_bytes(vfs, &names.pages_path, "screen_pages")?;
        Ok(Self::parse(&bytes, names))
    }

    /// Parses decrypted table bytes. Never fails: unmatched lines are ignored as in the client.
    pub fn parse(bytes: &[u8], names: &UiNames) -> Self {
        let mut r = Lines::new(bytes);
        let mut pages = Vec::new();
        while let Some(line) = r.next_line() {
            if starts_with_ci(line, names.page_list_end.as_bytes()) {
                break;
            }
            if line == b"\n" {
                continue;
            }
            let hit = names
                .page_tags
                .iter()
                .position(|t| starts_with_ci(line, t.as_bytes()));
            if let Some(i) = hit {
                let tag = PageTag::ALL[i];
                if let Some(end) = PageTag::end_tag_of(&names.page_tags[i]) {
                    pages.push(parse_page(&mut r, tag, &end, names));
                }
            }
        }
        ScreenPages { pages }
    }

    /// The first page with `tag`.
    pub fn page(&self, tag: PageTag) -> Option<&Page> {
        self.pages.iter().find(|p| p.tag == tag)
    }

    /// All widget records over all pages.
    pub fn widgets(&self) -> impl Iterator<Item = &WidgetRecord> {
        self.pages.iter().flat_map(|p| p.widgets.iter())
    }

    /// Record count per [`WidgetKind`] (index = kind).
    pub fn kind_counts(&self) -> [usize; 10] {
        let mut c = [0usize; 10];
        for w in self.widgets() {
            c[w.widget.kind() as usize] += 1;
        }
        c
    }
}

fn parse_page(r: &mut Lines, tag: PageTag, end: &str, names: &UiNames) -> Page {
    let mut page = Page {
        tag,
        screen_name: None,
        widgets: Vec::new(),
    };
    while let Some(line) = r.next_line() {
        if starts_with_ci(line, end.as_bytes()) {
            break;
        }
        if line == b"\n" {
            continue;
        }
        if starts_with_ci(line, names.page_name_key.as_bytes()) {
            let v = tokens(line).get(2).copied().unwrap_or(b"");
            page.screen_name = Some(latin1(quoted(v)));
        }
        let kind = names
            .widget_tags
            .iter()
            .position(|(open, _)| starts_with_ci(line, open.as_bytes()));
        if let Some(k) = kind {
            let end_tag = names.widget_tags[k].1.clone();
            parse_block(r, WidgetKind::ALL[k], &end_tag, names, &mut page.widgets);
        }
    }
    page
}

fn parse_block(
    r: &mut Lines,
    kind: WidgetKind,
    end: &str,
    names: &UiNames,
    out: &mut Vec<WidgetRecord>,
) {
    while let Some(line) = r.next_line() {
        if starts_with_ci(line, end.as_bytes()) {
            break;
        }
        if line == b"\n" {
            continue;
        }
        // Any other line is a record header the client does not inspect; the record lines follow.
        let mut rec = Rec {
            r,
            line: 0,
            issues: Vec::new(),
        };
        let widget = rec.widget(kind, names);
        out.push(WidgetRecord {
            widget,
            conformant: !rec.issues.iter().any(|i| i.kind == IssueKind::Schema),
            issues: rec.issues,
        });
    }
}

// ---- line reading ------------------------------------------------------------------------------------------

/// Lines as the client reads them: every CR directly followed by LF is dropped first, then lines are cut
/// like `fgets` with a 0x104-byte buffer (the `\n` stays part of the line).
struct Lines {
    data: Vec<u8>,
    pos: usize,
}

impl Lines {
    fn new(bytes: &[u8]) -> Self {
        let mut data = Vec::with_capacity(bytes.len());
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                continue;
            }
            data.push(b);
        }
        Lines { data, pos: 0 }
    }

    fn next_line(&mut self) -> Option<&[u8]> {
        if self.pos >= self.data.len() {
            return None;
        }
        let start = self.pos;
        let rest = &self.data[start..];
        let n = match rest.iter().take(LINE_MAX).position(|&b| b == b'\n') {
            Some(i) => i + 1,
            None => rest.len().min(LINE_MAX),
        };
        self.pos += n;
        Some(&self.data[start..start + n])
    }
}

fn starts_with_ci(line: &[u8], tag: &[u8]) -> bool {
    !tag.is_empty() && line.len() >= tag.len() && line[..tag.len()].eq_ignore_ascii_case(tag)
}

/// `sscanf("%s")` tokens: runs of non-whitespace (C `isspace`).
fn tokens(line: &[u8]) -> Vec<&[u8]> {
    line.split(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
        .filter(|t| !t.is_empty())
        .collect()
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// `strtok(token, "\"")`: the first run of non-quote characters (empty when none).
fn quoted(v: &[u8]) -> &[u8] {
    v.split(|&b| b == b'"')
        .find(|t| !t.is_empty())
        .unwrap_or(b"")
}

/// C `atoi`: optional whitespace and sign, then decimal digits; wrapping like the 32-bit client.
fn atoi(t: &[u8]) -> i32 {
    let mut i = 0;
    while i < t.len() && matches!(t[i], b' ' | b'\t') {
        i += 1;
    }
    let neg = match t.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let mut v: i32 = 0;
    while let Some(d) = t.get(i).filter(|c| c.is_ascii_digit()) {
        v = v.wrapping_mul(10).wrapping_add((d - b'0') as i32);
        i += 1;
    }
    if neg {
        v.wrapping_neg()
    } else {
        v
    }
}

/// `sscanf("%d")` on a token: optional sign and at least one digit at the start, else no value.
fn scan_int(t: &[u8]) -> Option<i32> {
    let d = t
        .strip_prefix(b"-")
        .or_else(|| t.strip_prefix(b"+"))
        .unwrap_or(t);
    d.first().filter(|c| c.is_ascii_digit()).map(|_| atoi(t))
}

fn is_int(t: &[u8]) -> bool {
    let d = t
        .strip_prefix(b"-")
        .or_else(|| t.strip_prefix(b"+"))
        .unwrap_or(t);
    !d.is_empty() && d.iter().all(|c| c.is_ascii_digit())
}

/// Lower-case hex word, stops at the first other character. EVIDENCE: cshell+0x1c3b10
fn hex_word(t: &[u8]) -> u32 {
    let mut v: u32 = 0;
    for &c in t {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => break,
        };
        v = v.wrapping_mul(16).wrapping_add(d as u32);
    }
    v
}

fn is_hex(t: &[u8]) -> bool {
    !t.is_empty() && t.iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// `strtok(.., "(,)")` pieces.
fn pieces(t: &[u8]) -> Vec<&[u8]> {
    t.split(|b| matches!(b, b'(' | b',' | b')'))
        .filter(|p| !p.is_empty())
        .collect()
}

/// The client loops over groups of `n` pieces and keeps the last group; a missing piece reads as 0/none.
fn last_group(t: &[u8], n: usize) -> Vec<Option<&[u8]>> {
    let p = pieces(t);
    let mut out = vec![None; n];
    let mut i = 0;
    while i < p.len() {
        for (k, slot) in out.iter_mut().enumerate() {
            *slot = p.get(i + k).copied();
        }
        i += n;
    }
    out
}

/// Strict form `(v,v,...)` with exactly `n` pieces each accepted by `ok`.
fn is_tuple(t: &[u8], n: usize, ok: impl Fn(&[u8]) -> bool) -> bool {
    let Some(inner) = t.strip_prefix(b"(").and_then(|r| r.strip_suffix(b")")) else {
        return false;
    };
    let parts: Vec<&[u8]> = inner.split(|&b| b == b',').collect();
    parts.len() == n && parts.iter().all(|p| ok(p))
}

fn align_code(c: Option<&u8>) -> u32 {
    match c {
        Some(b'l') => 1,
        Some(b'r') => 2,
        Some(b'c') => 4,
        Some(b't') => 0x10,
        Some(b'b') => 0x20,
        _ => 0,
    }
}

/// One record line as the client sees it (`client`: the third whitespace token) and as a tolerant reader
/// sees it (`tolerant`: the first token after the first `=`, used only to judge schema conformance).
struct LineVal {
    client: Vec<u8>,
    tolerant: Option<Vec<u8>>,
}

/// Positional record reader over the following lines.
struct Rec<'a> {
    r: &'a mut Lines,
    line: usize,
    issues: Vec<LineIssue>,
}

impl Rec<'_> {
    fn flag(&mut self, code: char, kind: IssueKind) {
        self.issues.push(LineIssue {
            line: self.line.saturating_sub(1),
            code,
            kind,
        });
    }

    fn value(&mut self, code: char) -> LineVal {
        self.line += 1;
        let Some(line) = self.r.next_line() else {
            self.flag('~', IssueKind::Schema);
            return LineVal {
                client: Vec::new(),
                tolerant: None,
            };
        };
        let t = tokens(line);
        // EVIDENCE: cshell+0x1c9b10: sscanf("%s %s ..."): the third token is the value whatever the second is.
        let client = t.get(2).map(|v| v.to_vec()).unwrap_or_default();
        let shaped = t.len() >= 3 && t[1] == b"=";
        let tolerant = if shaped {
            Some(client.clone())
        } else {
            let after = line.iter().position(|&b| b == b'=').map(|i| &line[i + 1..]);
            let tol = after.and_then(|a| tokens(a).first().map(|v| v.to_vec()));
            if tol.is_some() {
                self.flag(code, IssueKind::Tokens);
            }
            tol
        };
        LineVal { client, tolerant }
    }

    /// Checks the tolerant value with `strict`; flags a schema issue when absent or not strict.
    fn check(&mut self, v: &LineVal, code: char, strict: impl Fn(&[u8]) -> bool) {
        if !v.tolerant.as_deref().is_some_and(&strict) {
            self.flag(code, IssueKind::Schema);
        }
    }

    /// `%d` with the field's initial value when the client's scan fails. EVIDENCE: cshell+0x1c8ce0
    fn d_or(&mut self, default: i32) -> i32 {
        let v = self.value('d');
        self.check(&v, 'd', is_int);
        scan_int(&v.client).unwrap_or(default)
    }
    fn d(&mut self) -> i32 {
        self.d_or(0)
    }
    fn p(&mut self) -> [i32; 2] {
        let v = self.value('P');
        self.check(&v, 'P', |t| is_tuple(t, 2, is_int));
        let g = last_group(&v.client, 2);
        [atoi(g[0].unwrap_or(b"")), atoi(g[1].unwrap_or(b""))]
    }
    fn r4(&mut self) -> [i32; 4] {
        let v = self.value('R');
        self.check(&v, 'R', |t| is_tuple(t, 4, is_int));
        let g = last_group(&v.client, 4);
        std::array::from_fn(|k| atoi(g[k].unwrap_or(b"")))
    }
    fn s(&mut self) -> String {
        let v = self.value('S');
        self.check(&v, 'S', |t| {
            t.len() >= 2
                && t.starts_with(b"\"")
                && t.ends_with(b"\"")
                && !t[1..t.len() - 1].contains(&b'"')
        });
        latin1(quoted(&v.client))
    }
    fn a(&mut self) -> Align {
        let v = self.value('A');
        self.check(&v, 'A', |t| {
            is_tuple(t, 2, |p| {
                !p.is_empty() && p.iter().all(|c| c.is_ascii_alphabetic())
            })
        });
        let g = last_group(&v.client, 2);
        let first = align_code(g[0].and_then(|p| p.first()));
        let second = match g[1].and_then(|p| p.first()) {
            Some(b'c') | Some(b'C') => ALIGN_SECOND_CENTRE,
            other => align_code(other),
        };
        Align { first, second }
    }
    fn c(&mut self) -> [u32; 4] {
        let v = self.value('C');
        self.check(&v, 'C', |t| is_tuple(t, 4, is_hex));
        let g = last_group(&v.client, 4);
        std::array::from_fn(|k| hex_word(g[k].unwrap_or(b"")))
    }
    fn h(&mut self) -> u32 {
        let v = self.value('H');
        self.check(&v, 'H', is_hex);
        hex_word(&v.client)
    }
    fn f(&mut self) -> f32 {
        let v = self.value('F');
        let parse = |t: &[u8]| {
            std::str::from_utf8(t)
                .ok()
                .and_then(|s| s.parse::<f32>().ok())
                .filter(|x| x.is_finite())
        };
        self.check(&v, 'F', |t| parse(t).is_some());
        parse(&v.client).unwrap_or(0.0)
    }

    /// One record by the schema of `kind`. Integer fields that the client initialises to -1 before its scan
    /// keep -1 when the scan fails (EVIDENCE: cshell+0x1c9b10, +0x1c8ce0, +0x1c9db0, +0x1ca250, +0x1ca5c0,
    /// +0x1d09f0); all other integer fields keep 0.
    fn widget(&mut self, kind: WidgetKind, names: &UiNames) -> Widget {
        match kind {
            WidgetKind::Window => Widget::Window(WindowRec {
                control_id: self.d_or(-1),
                pos: self.p(),
                source_rect: self.r4(),
                texture: self.s(),
                stretch_mode: self.d(),
            }),
            WidgetKind::Image => Widget::Image(ImageRec {
                control_id: self.d_or(-1),
                parent: self.d_or(-1),
                parent_kind: self.d_or(-1),
                pos: self.p(),
                source_rect: self.r4(),
                texture: self.s(),
                stretch_mode: self.d_or(-1),
            }),
            WidgetKind::Button => {
                let mut b = ButtonRec {
                    control_id: self.d(),
                    parent: self.d(),
                    parent_kind: self.d(),
                    command_id: self.d(),
                    pos: self.p(),
                    source_rect: self.r4(),
                    texture_prefix: self.s(),
                    text_pos: self.p(),
                    text_align: self.a(),
                    font_index: self.d(),
                    font_size: self.d(),
                    ..Default::default()
                };
                // EVIDENCE: cshell+0x1c9020: exact (case-sensitive) compare with the frame word, else atoi.
                let style = self.value('d');
                let framed = style.client == names.frame_word.as_bytes();
                if !framed {
                    self.check(&style, 'd', is_int);
                    b.text_style = atoi(&style.client);
                }
                b.text_colours = self.c();
                if framed {
                    let tex = self.s();
                    let pos = self.p();
                    b.frame = Some((tex, pos));
                }
                Widget::Button(b)
            }
            WidgetKind::Text => Widget::Text(TextRec {
                control_id: self.d(),
                parent: self.d(),
                parent_kind: self.d(),
                command_id: self.d(),
                pos: self.p(),
                rect: self.r4(),
                align: self.a(),
                font_index: self.d(),
                font_size: self.d(),
                text_style: self.d(),
                colours: self.c(),
                is_control: self.d(),
            }),
            WidgetKind::List => {
                let mut l = ListRec {
                    control_id: self.d_or(-1),
                    parent: self.d_or(-1),
                    parent_kind: self.d_or(-1),
                    pos: self.p(),
                    row_height: self.d_or(-1),
                    item_spacing: self.d_or(-1),
                    scroll: self.d_or(-1),
                    scroll_offset: self.p(),
                    scroll_size: self.d_or(-1),
                    auto_scroll: self.d(),
                    has_back_image: self.d(),
                    back_texture: None,
                };
                if l.has_back_image != 0 {
                    l.back_texture = Some(self.s());
                }
                Widget::List(l)
            }
            WidgetKind::Inventory => Widget::Inventory(InventoryRec {
                control_id: self.d_or(-1),
                parent: self.d_or(-1),
                parent_kind: self.d_or(-1),
                pos: self.p(),
                rows_cols: self.p(),
                pos2: self.p(),
                cell_size: self.p(),
                cell_spacing: self.p(),
            }),
            WidgetKind::EditBox => Widget::EditBox(EditBoxRec {
                control_id: self.d_or(-1),
                parent: self.d_or(-1),
                parent_kind: self.d_or(-1),
                pos: self.p(),
                font_index: self.d(),
                font_size: self.d(),
                max_length: self.d(),
                input_mode: self.d(),
                caret_colour: self.h(),
                text_style: self.d(),
                caret_enabled: self.d(),
                caret_period: self.f(),
                visible: self.d(),
            }),
            WidgetKind::CheckButton => Widget::CheckButton(CheckButtonRec {
                control_id: self.d(),
                parent: self.d(),
                parent_kind: self.d(),
                command_id: self.d(),
                pos: self.p(),
                source_rect: self.r4(),
                texture_checked: self.s(),
                texture_unchecked: self.s(),
            }),
            WidgetKind::Lattice => Widget::Lattice(LatticeRec {
                pos: self.p(),
                rows: self.d(),
                cols: self.d(),
                texture: self.s(),
            }),
            WidgetKind::RemoteXml => Widget::RemoteXml(RemoteXmlRec {
                parent: self.d_or(-1),
                parent_kind: self.d_or(-1),
                pos: self.p(),
                rect: self.r4(),
                url_regions: [self.s(), self.s(), self.s()],
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        // Placeholder names (UiNames::placeholder): page tags [[pgN]], widget tags <kind>/<kind_end>.
        "junk line before\r\n\
[[pg2]]\r\n\
page_title = \"lobby\"\r\n\
\r\n\
<win>\r\n\
==w==\r\n\
k = 7\r\n\
k = (10,20)\r\n\
k = (0,0,64,32)\r\n\
k = \"ui\\bg.dtx\"\r\n\
k = 1\r\n\
<win_end>\r\n\
<BTN>\r\n\
==b1==\r\n\
id = 1\r\n\
p = 7\r\n\
pk = 0\r\n\
cmd = 42\r\n\
pos = (5,6)\r\n\
src = (0,0,10,10)\r\n\
tex = \"ui\\btn_\"\r\n\
tp = (1,2)\r\n\
al = (c,C)\r\n\
fi = 3\r\n\
fs = 12\r\n\
st = 2\r\n\
col = (ff00ff00,0,a,ffffffff)\r\n\
\r\n\
==b2==\r\n\
ID = 2\r\n\
P = 7\r\n\
PK = 0\r\n\
CMD = 43\r\n\
POS = (5,40)\r\n\
SRC = (0,0,10,10)\r\n\
TEX = \"ui\\btn_\"\r\n\
TP = (1,2)\r\n\
AL = (l,t)\r\n\
FI = 3\r\n\
FS = 12\r\n\
ST = FRM\r\n\
COL = (1,2,3,4)\r\n\
FT = \"ui\\frame.dtx\"\r\n\
FP = (9,9)\r\n\
<btn_end>\r\n\
<lst>\r\n\
==l==\r\n\
a = 1\r\n\
b = 2\r\n\
c = 3\r\n\
d = (4,5)\r\n\
e = 6\r\n\
f = 7\r\n\
g = 8\r\n\
h = (9,10)\r\n\
i = 11\r\n\
j = 0\r\n\
k = 1\r\n\
l = \"ui\\back.dtx\"\r\n\
<lst_end>\r\n\
<edt>\r\n\
==e==\r\n\
a = 1\r\n\
b = 2\r\n\
c = 3\r\n\
d = (4,5)\r\n\
e = 6\r\n\
f = 7\r\n\
g = 16\r\n\
h = 0\r\n\
i = ff00aa\r\n\
j = 1\r\n\
k = 1\r\n\
l = 0.5\r\n\
m = 1\r\n\
<edt_end>\r\n\
[[pg2_end]]\r\n\
[[PG0]]\r\n\
<txt>\r\n\
==t==\r\n\
a = 1\r\n\
b = 2\r\n\
c = 3\r\n\
d = 4\r\n\
e = (5,6)\r\n\
f = (1,2,3,4)\r\n\
g = (r,b)\r\n\
h = 1\r\n\
i = 14\r\n\
j = 0\r\n\
k = (a,b,c,d)\r\n\
l = 1\r\n\
<txt_end>\r\n\
<img>\r\n\
==i==\r\n\
a = 1\r\n\
b = 2\r\n\
c = 3\r\n\
d = (4,5)\r\n\
e = (1,2,3,4)\r\n\
f = \"x.dtx\"\r\n\
g = 0\r\n\
<img_end>\r\n\
<inv>\r\n\
==v==\r\n\
a = 1\r\n\
b = 2\r\n\
c = 3\r\n\
d = (4,5)\r\n\
e = (2,3)\r\n\
f = (6,7)\r\n\
g = (32,32)\r\n\
h = (1,1)\r\n\
<inv_end>\r\n\
<chk>\r\n\
==c==\r\n\
a = 1\r\n\
b = 2\r\n\
c = 3\r\n\
d = 99\r\n\
e = (4,5)\r\n\
f = (1,2,3,4)\r\n\
g = \"on.dtx\"\r\n\
h = \"off.dtx\"\r\n\
<chk_end>\r\n\
<lat>\r\n\
==p==\r\n\
a = (1,2)\r\n\
b = 3\r\n\
c = 4\r\n\
d = \"grid.dtx\"\r\n\
<lat_end>\r\n\
<xml>\r\n\
==x==\r\n\
a = 1\r\n\
b = 2\r\n\
c = (3,4)\r\n\
d = (5,6,7,8)\r\n\
e = \"u1\"\r\n\
f = \"u2\"\r\n\
g = \"u3\"\r\n\
<xml_end>\r\n\
[[pg0_end]]\r\n\
[[pg9]]\r\n\
<win>\r\n\
==bad==\r\n\
k=7\r\n\
k = 10,20\r\n\
k = (0,0,64)\r\n\
k = ui\\bg.dtx\r\n\
k = x\r\n\
<win_end>\r\n\
[[pg9_end]]\r\n\
[[all_pages_done]]\r\n\
[[pg5]]\r\n\
[[pg5_end]]\r\n"
            .to_string()
    }

    #[test]
    fn parses_every_widget_kind() {
        let n = UiNames::placeholder();
        let sp = ScreenPages::parse(fixture().as_bytes(), &n);
        assert_eq!(
            sp.pages.len(),
            3,
            "pages after the list end tag are not read"
        );
        let p = sp.page(PageTag::Page2).unwrap();
        assert_eq!(p.screen_name.as_deref(), Some("lobby"));
        assert_eq!(sp.kind_counts(), [2, 1, 2, 1, 1, 1, 1, 1, 1, 1]);
        let conformant = sp.widgets().filter(|w| w.conformant).count();
        assert_eq!(conformant, 11);

        let Widget::Window(w) = &p.widgets[0].widget else {
            panic!()
        };
        assert_eq!(
            w,
            &WindowRec {
                control_id: 7,
                pos: [10, 20],
                source_rect: [0, 0, 64, 32],
                texture: "ui\\bg.dtx".into(),
                stretch_mode: 1
            }
        );
        let Widget::Button(b1) = &p.widgets[1].widget else {
            panic!()
        };
        assert_eq!(b1.command_id, 42);
        assert_eq!(
            b1.text_align,
            Align {
                first: 4,
                second: 0x40
            }
        );
        assert_eq!(b1.text_style, 2);
        assert_eq!(b1.text_colours, [0xff00ff00, 0, 10, 0xffff_ffff]);
        assert!(b1.frame.is_none());
        let Widget::Button(b2) = &p.widgets[2].widget else {
            panic!()
        };
        assert_eq!(
            b2.text_align,
            Align {
                first: 1,
                second: 0x10
            }
        );
        assert_eq!(b2.frame, Some(("ui\\frame.dtx".to_string(), [9, 9])));
        assert!(
            p.widgets[2].conformant,
            "key spelling and case never matter"
        );
        let Widget::List(l) = &p.widgets[3].widget else {
            panic!()
        };
        assert_eq!(l.scroll_offset, [9, 10]);
        assert_eq!(l.back_texture.as_deref(), Some("ui\\back.dtx"));
        let Widget::EditBox(e) = &p.widgets[4].widget else {
            panic!()
        };
        assert_eq!(e.caret_colour, 0xff00aa);
        assert_eq!(e.caret_period, 0.5);
        assert_eq!(e.max_length, 16);

        let p0 = sp.page(PageTag::Page0).unwrap();
        assert!(p0.screen_name.is_none());
        let Widget::Text(t) = &p0.widgets[0].widget else {
            panic!()
        };
        assert_eq!(
            t.align,
            Align {
                first: 2,
                second: 0x20
            }
        );
        assert_eq!(t.colours, [10, 11, 12, 13]);
        let Widget::Inventory(v) = &p0.widgets[2].widget else {
            panic!()
        };
        assert_eq!(v.cell_size, [32, 32]);
        let Widget::CheckButton(c) = &p0.widgets[3].widget else {
            panic!()
        };
        assert_eq!(c.texture_unchecked, "off.dtx");
        let Widget::Lattice(lt) = &p0.widgets[4].widget else {
            panic!()
        };
        assert_eq!((lt.rows, lt.cols), (3, 4));
        let Widget::RemoteXml(x) = &p0.widgets[5].widget else {
            panic!()
        };
        assert_eq!(x.url_regions[1], "u2");
        assert_eq!(x.rect, [5, 6, 7, 8]);

        // Malformed record: values read leniently like the client, but flagged.
        let bad = &sp.page(PageTag::Page9).unwrap().widgets[0];
        assert!(!bad.conformant);
        let Widget::Window(w) = &bad.widget else {
            panic!()
        };
        assert_eq!(w.pos, [10, 20]);
        assert_eq!(w.source_rect, [0, 0, 64, 0]);
        assert_eq!(w.texture, "ui\\bg.dtx");
        assert_eq!(
            w.control_id, -1,
            "the client's initial value survives a failed scan"
        );
        assert!(bad
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::Tokens && i.line == 0));
    }

    #[test]
    fn missing_space_after_equals_is_conformant_but_keeps_the_default() {
        let n = UiNames::placeholder();
        let text = "[[pg1]]
<img>
==i==
id =9
b = 2
c = 3
d = (4,5)
e = (1,2,3,4)
f = \"x.dtx\"
g = 0
<img_end>
[[pg1_end]]
";
        let sp = ScreenPages::parse(text.as_bytes(), &n);
        let w = &sp.pages[0].widgets[0];
        assert!(w.conformant);
        assert_eq!(
            w.issues,
            vec![LineIssue {
                line: 0,
                code: 'd',
                kind: IssueKind::Tokens
            }]
        );
        let Widget::Image(i) = &w.widget else {
            panic!()
        };
        assert_eq!(i.control_id, -1);
        assert_eq!(i.parent, 2);
    }

    #[test]
    fn primitive_parsers_follow_the_client() {
        assert_eq!(atoi(b"-12x"), -12);
        assert_eq!(atoi(b""), 0);
        assert_eq!(atoi(b"99999999999"), 99999999999i64 as i32);
        assert_eq!(hex_word(b"ffZ1"), 0xff);
        assert_eq!(hex_word(b"FF"), 0, "upper-case hex is not accepted");
        assert_eq!(
            last_group(b"(1,2)(3,4)", 2),
            vec![Some(&b"3"[..]), Some(&b"4"[..])]
        );
        assert_eq!(quoted(b"\"\""), b"");
        assert_eq!(quoted(b"\"a\"b"), b"a");
        let mut l = Lines::new(&[b'x'; 600]);
        assert_eq!(l.next_line().unwrap().len(), LINE_MAX);
        assert_eq!(l.next_line().unwrap().len(), LINE_MAX);
        assert_eq!(l.next_line().unwrap().len(), 600 - 2 * LINE_MAX);
        assert!(l.next_line().is_none());
        let mut l = Lines::new(b"a\r\r\nb\rc\n");
        assert_eq!(l.next_line().unwrap(), b"a\r\n");
        assert_eq!(l.next_line().unwrap(), b"b\rc\n");
    }

    #[test]
    fn truncated_and_hostile_input_never_panics() {
        let n = UiNames::placeholder();
        let f = fixture();
        let b = f.as_bytes();
        for cut in (0..b.len()).step_by(7) {
            let _ = ScreenPages::parse(&b[..cut], &n);
        }
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut buf = b.to_vec();
        for _ in 0..3000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let p = (x as usize) % buf.len();
            buf[p] = (x >> 40) as u8;
            let _ = ScreenPages::parse(&buf, &n);
        }
    }
}
