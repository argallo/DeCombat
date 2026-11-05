//! clan-functions: the pages of the clan screen's web window (window 5), drawn by the client itself.
//!
//! The 2009 clan screen shows create / find / manage / leave as web pages of the publisher's site in an embedded
//! browser inside window 5 (titles 0x4ff / 0x501 / 0x4fe / 0x500). The page content was never part of the client;
//! DeCombat serves its own pages from the lobby (PROTOCOL_2009.md §5.2, CD-47) and the client draws them inside the
//! same window from the pages' text form (`fmt=text`, CLI-790). Everything below the window's title and above its
//! close button is our design (CLI-791), laid out after the reference stack's picture of the same window.
// SPEC: PROTOCOL_2009.md §5.2 (clan web pages; page content and URLs are ours)
// EVIDENCE: cshell+0x1103d0 (10000 / 10001 / 10004 / 10009 open window 5 with the page's title, 10005 closes it)
// OBSERVED: reference frames of the create window (scratch shots of this branch; layout read off the pixels)

use super::catalog::Catalog;
use super::clan::ClanMarks;
use super::sheet::{place_text, Look, Part, Sheet};
use crate::hud::items::DrawItem;
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::WidgetKind;
use std::net::SocketAddr;

/// The page area of window 5 (below the title, above the close button), filled flat.
// DC-DEFAULT: CLI-791 (proposed) the page geometry and colours of the DeCombat clan pages (OBSERVED in the reference stack's
// frames: page fill 0x1b1b1b at (233, 200) 556 x 365, cells 46 x 46 every 52 x 48 px, 10 per row)
pub const PAGE: [i32; 4] = [233, 200, 556, 365];
pub const PAGE_FILL: [u8; 4] = [27, 27, 27, 255];
pub const LABEL_COLOUR: u32 = 0xfffe_dd00;
pub const HINT_COLOUR: u32 = 0xff99_9999;
pub const TEXT_COLOUR: u32 = 0xffff_ffff;
pub const BOX_EDGE: [u8; 4] = [112, 112, 112, 255];
pub const BOX_FILL: [u8; 4] = [42, 42, 42, 255];
pub const CELL_EDGE: [u8; 4] = [68, 68, 68, 255];
pub const CELL_FILL: [u8; 4] = [16, 16, 16, 255];
pub const PICK_EDGE: [u8; 4] = [254, 173, 0, 255];
pub const BUTTON_TEXT_ON: u32 = 0xffee_eeee;
pub const BUTTON_TEXT_OFF: u32 = 0xff77_7777;

pub const NAME_LABEL_AT: [i32; 2] = [248, 220];
pub const NAME_FIELD: [i32; 4] = [333, 208, 220, 26];
pub const NAME_HINT_AT: [i32; 2] = [564, 220];
pub const PREVIEW: [i32; 4] = [710, 204, 64, 64];
pub const EMBLEM_LABEL_AT: [i32; 2] = [248, 258];
pub const BACKGROUND_LABEL_AT: [i32; 2] = [248, 386];
/// The pager of each grid: previous button, page text centre, next button (y of the emblem pager; the background
/// pager sits 128 px lower, with its grid).
pub const PAGER_PREV: [i32; 4] = [669, 248, 30, 22];
pub const PAGER_NEXT: [i32; 4] = [753, 248, 30, 22];
pub const PAGER_TEXT_AT: [i32; 2] = [726, 258];
pub const GRID_AT: [i32; 2] = [247, 272];
pub const GRID_DY: i32 = 128;
pub const CELL: i32 = 46;
pub const CELL_STEP: [i32; 2] = [52, 48];
pub const PER_ROW: u32 = 10;
pub const PER_PAGE: u32 = 20;
pub const MESSAGE_AT: [i32; 2] = [248, 507];
pub const CREATE_BUTTON: [i32; 4] = [433, 528, 100, 26];

/// The clan name the create page accepts before its button is enabled (the server's rule is CLN-10, 1..16).
// DC-DEFAULT: CLI-791 (proposed) 2..16 characters, as the page's hint says
pub const NAME_MIN: usize = 2;
pub const NAME_MAX: usize = 16;

/// The page's texts (ours; the ROM has none for these pages).
pub const T_NAME: &str = "Clan name";
pub const T_HINT: &str = "2 - 16 characters";
pub const T_EMBLEM: &str = "Emblem";
pub const T_BACKGROUND: &str = "Background";
pub const T_CREATE: &str = "Create";
pub const T_UNREACHABLE: &str = "Cannot reach the clan server.";

/// The web port of the lobby: `DC_CLANWEB` (`host:port`) when set, else the lobby's default, the auth port + 8080.
// DC-DEFAULT: CLI-790 (proposed) the client finds the clan pages at the auth address with port + 8080 (dc-lobby-server's
// `--clanweb -1` default)
pub fn web_addr(auth: Option<SocketAddr>) -> Option<SocketAddr> {
    if let Some(a) = std::env::var("DC_CLANWEB")
        .ok()
        .and_then(|v| v.parse::<SocketAddr>().ok())
    {
        return Some(a);
    }
    let a = auth?;
    Some(SocketAddr::new(a.ip(), a.port().checked_add(8080)?))
}

/// Query escaping (unreserved characters kept, everything else `%XX` of its UTF-8 bytes).
pub fn url_encode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// The lines of a `fmt=text` answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebReply {
    /// The viewer's clan number (0 = none) and whether the viewer leads it.
    pub clan: i64,
    pub leader: bool,
    /// The action's result and message.
    pub msg: Option<(bool, String)>,
    pub clans: Vec<ClanListRow>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClanListRow {
    pub uid: i64,
    pub name: String,
    pub members: u32,
    pub leader: String,
    pub emblem: u32,
    pub background: u32,
}

pub fn parse_reply(body: &str) -> WebReply {
    let mut r = WebReply::default();
    for line in body.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let num = |i: usize| {
            f.get(i)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(0)
        };
        let s = |i: usize| f.get(i).map_or(String::new(), |v| v.to_string());
        match f.first().copied() {
            Some("me") => {
                r.clan = num(1);
                r.leader = num(2) != 0;
            }
            Some("msg") => r.msg = Some((num(1) != 0, s(2))),
            Some("clan") => r.clans.push(ClanListRow {
                uid: num(1),
                name: s(2),
                members: num(3).clamp(0, u32::MAX as i64) as u32,
                leader: s(4),
                emblem: num(5).clamp(0, u32::MAX as i64) as u32,
                background: num(6).clamp(0, u32::MAX as i64) as u32,
            }),
            _ => {}
        }
    }
    r
}

fn fill(r: [i32; 4], colour: [u8; 4], out: &mut Vec<DrawItem>) {
    out.push(DrawItem::Fill {
        rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
        colour,
    });
}

/// A box with a one-pixel edge.
pub fn framed(r: [i32; 4], edge: [u8; 4], inner: [u8; 4], out: &mut Vec<DrawItem>) {
    fill(r, edge, out);
    fill([r[0] + 1, r[1] + 1, r[2] - 2, r[3] - 2], inner, out);
}

fn inside(r: [i32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] as f32
        && p[0] < (r[0] + r[2]) as f32
        && p[1] >= r[1] as f32
        && p[1] < (r[1] + r[3]) as f32
}

fn left_mid() -> Align {
    Align {
        first: super::sheet::ALIGN_LEFT,
        second: ALIGN_SECOND_CENTRE,
    }
}

fn centre() -> Align {
    Align {
        first: super::sheet::ALIGN_CENTRE,
        second: ALIGN_SECOND_CENTRE,
    }
}

fn text(t: &str, at: [i32; 2], align: Align, colour: u32, out: &mut Vec<DrawItem>) {
    out.push(place_text(t, at, [0, 0], align, 14, 14, colour));
}

/// A page button: edge + fill, its caption centred; hovered and enabled: the pick colour edge.
pub fn button(r: [i32; 4], caption: &str, on: bool, hover: bool, out: &mut Vec<DrawItem>) {
    let edge = if on && hover { PICK_EDGE } else { BOX_EDGE };
    framed(r, edge, BOX_FILL, out);
    let c = if on { BUTTON_TEXT_ON } else { BUTTON_TEXT_OFF };
    text(
        caption,
        [r[0] + r[2] / 2, r[1] + r[3] / 2],
        centre(),
        c,
        out,
    );
}

/// The mark layers (background, then the emblem) stretched over `r`.
pub fn mark(
    cat: &mut Catalog,
    bg: Option<&str>,
    emblem: Option<&str>,
    r: [i32; 4],
    out: &mut Vec<DrawItem>,
) {
    for t in [bg, emblem].into_iter().flatten() {
        let src = cat
            .tex_size(t)
            .map_or([r[2] as f32, r[3] as f32], |(w, h)| [w as f32, h as f32]);
        out.push(DrawItem::Image {
            tex: t.to_string(),
            src,
            dst: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
            tint: [255; 4],
            turn: None,
        });
    }
}

/// The mark layers (background, then the emblem) as the 2009 mark control shows them: the texture's top-left
/// [`super::clan::MARK_TEXELS`] rectangle (the art is 51 x 51 in a 64 x 64 texture) over `r`.
// EVIDENCE: cshell+0x343cc0 / 0x343d50 (a layer: texture set, source rectangle (+0x114, +0x118) through the image
// control's slot +0x144, scale (+0x10c, +0x110) through cshell+0x4430a0), DISASM cshell+0x121663..0x121693 (the lobby
// panel's rectangle 51 x 51 at scale 0x3ed2d2d3 = 21 / 51)
// OBSERVED: lobby-r10b/pairs/item3_marks.png (reference client: the clan page's mark at texel scale from
// the frame's corner, art 35..81 x 136..176 for the frame at 33, 133; ours drew the whole padded texture over the
// frame, a smaller mark shifted up and left)
pub fn mark_cropped(bg: Option<&str>, emblem: Option<&str>, r: [i32; 4], out: &mut Vec<DrawItem>) {
    let src = super::clan::MARK_TEXELS.map(|v| v as f32);
    for t in [bg, emblem].into_iter().flatten() {
        out.push(DrawItem::Image {
            tex: t.to_string(),
            src,
            dst: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
            tint: [255; 4],
            turn: None,
        });
    }
}

/// What a press on the create page hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateHit {
    Field,
    Prev(usize),
    Next(usize),
    Cell(usize, u32),
    Create,
    Page,
}

/// The create page: name field (a hidden sheet edit part that keeps the typing and the focus), the emblem and
/// background pickers, the reply line and the create button.
#[derive(Debug, Clone, PartialEq)]
pub struct CreatePage {
    /// The sheet part of the name field.
    pub field: Option<usize>,
    /// Picked emblem and background (1-based indices of the clan mark table) and the shown page of each grid.
    pub pick: [u32; 2],
    pub page: [u32; 2],
    pub message: Option<String>,
    pub busy: bool,
    pub hover: Option<CreateHit>,
}

impl CreatePage {
    pub fn new(field: Option<usize>) -> CreatePage {
        CreatePage {
            field,
            pick: [1, 1],
            page: [0, 0],
            message: None,
            busy: false,
            hover: None,
        }
    }

    pub fn name<'a>(&self, sheet: &'a Sheet) -> &'a str {
        self.field.map_or("", |i| sheet.parts[i].text.as_str())
    }

    pub fn name_ok(&self, sheet: &Sheet) -> bool {
        let n = self.name(sheet).trim().chars().count();
        (NAME_MIN..=NAME_MAX).contains(&n)
    }

    fn grid_rect(g: usize, k: u32) -> [i32; 4] {
        let (col, row) = ((k % PER_ROW) as i32, (k / PER_ROW) as i32);
        [
            GRID_AT[0] + col * CELL_STEP[0],
            GRID_AT[1] + g as i32 * GRID_DY + row * CELL_STEP[1],
            CELL,
            CELL,
        ]
    }

    fn pager(r: [i32; 4], g: usize) -> [i32; 4] {
        [r[0], r[1] + g as i32 * GRID_DY, r[2], r[3]]
    }

    /// Entries of grid `g` (0 emblems, 1 backgrounds): the table's indices 1..
    pub fn count(marks: &ClanMarks, g: usize) -> u32 {
        let n = if g == 0 {
            marks.emblems.len()
        } else {
            marks.backgrounds.len()
        };
        n.saturating_sub(1) as u32
    }

    pub fn pages(marks: &ClanMarks, g: usize) -> u32 {
        Self::count(marks, g).div_ceil(PER_PAGE).max(1)
    }

    pub fn hit(&self, p: [f32; 2]) -> Option<CreateHit> {
        if !inside(PAGE, p) {
            return None;
        }
        if inside(NAME_FIELD, p) {
            return Some(CreateHit::Field);
        }
        if inside(CREATE_BUTTON, p) {
            return Some(CreateHit::Create);
        }
        for g in 0..2 {
            if inside(Self::pager(PAGER_PREV, g), p) {
                return Some(CreateHit::Prev(g));
            }
            if inside(Self::pager(PAGER_NEXT, g), p) {
                return Some(CreateHit::Next(g));
            }
            for k in 0..PER_PAGE {
                if inside(Self::grid_rect(g, k), p) {
                    return Some(CreateHit::Cell(g, k));
                }
            }
        }
        Some(CreateHit::Page)
    }

    /// A press on the page: the grids and pagers change the pick here; the field and the create button are the
    /// caller's (focus, request).
    pub fn press(&mut self, marks: &ClanMarks, h: CreateHit) {
        match h {
            CreateHit::Prev(g) => self.page[g] = self.page[g].saturating_sub(1),
            CreateHit::Next(g) => {
                if self.page[g] + 1 < Self::pages(marks, g) {
                    self.page[g] += 1;
                }
            }
            CreateHit::Cell(g, k) => {
                let i = 1 + self.page[g] * PER_PAGE + k;
                if i <= Self::count(marks, g) {
                    self.pick[g] = i;
                }
            }
            _ => {}
        }
    }

    /// The create request (path and query of the text form).
    pub fn request(&self, sheet: &Sheet, user: &str) -> String {
        format!(
            "/clan/create?p={}&name={}&emblem={}&bg={}&fmt=text",
            url_encode(user),
            url_encode(self.name(sheet).trim()),
            self.pick[0],
            self.pick[1]
        )
    }

    pub fn draw(
        &self,
        cat: &mut Catalog,
        sheet: &Sheet,
        marks: &ClanMarks,
        secs: f32,
        out: &mut Vec<DrawItem>,
    ) {
        fill(PAGE, PAGE_FILL, out);
        let pick = |list: &[Option<String>], i: u32| -> Option<String> {
            list.get(i as usize).cloned().flatten()
        };
        let (e, b) = (
            pick(&marks.emblems, self.pick[0]),
            pick(&marks.backgrounds, self.pick[1]),
        );
        // the name row
        text(T_NAME, NAME_LABEL_AT, left_mid(), LABEL_COLOUR, out);
        framed(NAME_FIELD, BOX_EDGE, [0, 0, 0, 255], out);
        let focused = self.field.is_some_and(|i| sheet.parts[i].focus);
        let mut shown = self.name(sheet).to_string();
        // the caret blinks as the sheet's edit boxes do (on 1/3 s, off 1/3 s)
        if focused && (secs / 0.333) as i64 % 2 == 0 {
            shown.push('|');
        }
        text(
            &shown,
            [NAME_FIELD[0] + 7, NAME_FIELD[1] + NAME_FIELD[3] / 2],
            left_mid(),
            TEXT_COLOUR,
            out,
        );
        text(T_HINT, NAME_HINT_AT, left_mid(), HINT_COLOUR, out);
        mark(cat, b.as_deref(), e.as_deref(), PREVIEW, out);
        // the two grids
        for g in 0..2usize {
            let n = Self::count(marks, g);
            let label = format!(
                "{} {} / {}",
                if g == 0 { T_EMBLEM } else { T_BACKGROUND },
                self.pick[g],
                n
            );
            let at = if g == 0 {
                EMBLEM_LABEL_AT
            } else {
                BACKGROUND_LABEL_AT
            };
            text(&label, at, left_mid(), LABEL_COLOUR, out);
            let prev = Self::pager(PAGER_PREV, g);
            let next = Self::pager(PAGER_NEXT, g);
            button(prev, "<", true, self.hover == Some(CreateHit::Prev(g)), out);
            button(next, ">", true, self.hover == Some(CreateHit::Next(g)), out);
            text(
                &format!("{} / {}", self.page[g] + 1, Self::pages(marks, g)),
                [PAGER_TEXT_AT[0], PAGER_TEXT_AT[1] + g as i32 * GRID_DY],
                centre(),
                HINT_COLOUR,
                out,
            );
            for k in 0..PER_PAGE {
                let i = 1 + self.page[g] * PER_PAGE + k;
                if i > n {
                    break;
                }
                let r = Self::grid_rect(g, k);
                let edge = if self.pick[g] == i {
                    PICK_EDGE
                } else {
                    CELL_EDGE
                };
                framed(r, edge, CELL_FILL, out);
                let inner = [r[0] + 1, r[1] + 1, r[2] - 2, r[3] - 2];
                // CLI-1022: the art is the 51 x 51 texel corner of the 64 x 64 texture (CLI-911): that square fills
                // the cell's inside, so the art sits centred in the selection box (the whole texture put it up and
                // left)
                if g == 0 {
                    mark_cropped(b.as_deref(), pick(&marks.emblems, i).as_deref(), inner, out);
                } else {
                    mark_cropped(
                        pick(&marks.backgrounds, i).as_deref(),
                        e.as_deref(),
                        inner,
                        out,
                    );
                }
            }
        }
        if let Some(m) = &self.message {
            text(m, MESSAGE_AT, left_mid(), LABEL_COLOUR, out);
        }
        let on = self.name_ok(sheet) && !self.busy;
        button(
            CREATE_BUTTON,
            T_CREATE,
            on,
            self.hover == Some(CreateHit::Create),
            out,
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the list pages (find a clan, manage the members) and the leave page
// ---------------------------------------------------------------------------------------------------------------

/// List pages: column headers at y 216, a rule under them at y 230, rows from y 250 every 27 px, the bottom buttons
/// at y 528, a row's action button at its right end.
// DC-DEFAULT: CLI-793 (proposed) the find / manage / leave pages' layout (OBSERVED in the reference stack's frames: headers
// 0xfedd00 at y 216, rule 0x555555 (241, 230) 540 x 1, rows 0xdddddd at y 250 every 27 px, Refresh (245, 528)
// 80 x 24; the Join / Remove row buttons and the reply line are ours: the reference stack's pages have no row buttons)
pub const HEADER_Y: i32 = 216;
pub const RULE: [i32; 4] = [241, 230, 540, 1];
pub const RULE_COLOUR: [u8; 4] = [85, 85, 85, 255];
pub const ROW_Y: i32 = 250;
pub const ROW_STEP: i32 = 27;
pub const ROW_MAX: usize = 10;
pub const ROW_COLOUR: u32 = 0xffdd_dddd;
pub const ROW_BUTTON: [i32; 4] = [705, 0, 70, 20];
pub const REFRESH_BUTTON: [i32; 4] = [245, 528, 80, 24];
pub const SECOND_BUTTON: [i32; 4] = [337, 528, 110, 24];
pub const SEARCH_COLUMNS: [(i32, &str); 3] = [(247, "Clan"), (447, "Members"), (527, "Leader")];
pub const MANAGE_COLUMNS: [(i32, &str); 3] = [(247, "Class"), (337, "Name"), (537, "Experience")];
pub const T_RELOAD: &str = "Refresh";
pub const T_CREATE_A_CLAN: &str = "Create a clan";
pub const T_JOIN: &str = "Join";
pub const T_REMOVE: &str = "Remove";
pub const T_LEADER: &str = "Leader";
pub const T_MEMBER: &str = "Member";

/// The leave page: a question, the clan's name, the rule, two buttons.
pub const LEAVE_QUESTION_AT: [i32; 2] = [511, 247];
pub const LEAVE_NAME_AT: [i32; 2] = [511, 277];
pub const LEAVE_RULE_AT: [i32; 2] = [511, 317];
pub const LEAVE_BUTTON: [i32; 4] = [383, 400, 120, 28];
pub const CANCEL_BUTTON: [i32; 4] = [519, 400, 120, 28];
pub const T_LEAVE_QUESTION: &str = "Leave your clan?";
pub const T_LEAVE_RULE: &str =
    "You will lose your membership. A leader who leaves hands the clan to the next member.";
pub const T_LEAVE: &str = "Leave Clan";
pub const T_CANCEL: &str = "Cancel";

/// One member row of the manage page (`member` lines of the text form).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemberLine {
    pub char_no: u32,
    pub leader: bool,
    pub name: String,
    pub exp: u32,
}

pub fn parse_members(body: &str) -> Vec<MemberLine> {
    body.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.first() == Some(&"member")).then(|| {
                let n = |i: usize| {
                    f.get(i)
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .unwrap_or(0)
                };
                MemberLine {
                    char_no: n(1) as u32,
                    leader: n(2) != 0,
                    name: f.get(3).map_or(String::new(), |v| v.to_string()),
                    exp: n(4).min(u32::MAX as u64) as u32,
                }
            })
        })
        .collect()
}

/// What a press on a list or leave page hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageHit {
    Row(usize),
    Refresh,
    Second,
    Leave,
    Cancel,
    Page,
}

fn row_button(i: usize) -> [i32; 4] {
    let y = ROW_Y + i as i32 * ROW_STEP;
    [
        ROW_BUTTON[0],
        y - ROW_BUTTON[3] / 2,
        ROW_BUTTON[2],
        ROW_BUTTON[3],
    ]
}

/// Which list page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// Find a clan: clan rows, Join for a viewer without a clan, Refresh and Create a clan.
    Search,
    /// The leader's member page: member rows, Remove on the others for the leader, Refresh.
    Manage,
}

/// The find / manage page.
#[derive(Debug, Clone, PartialEq)]
pub struct ListPage {
    pub kind: ListKind,
    pub reply: WebReply,
    pub members: Vec<MemberLine>,
    pub message: Option<String>,
    pub busy: bool,
    pub hover: Option<PageHit>,
}

impl ListPage {
    pub fn new(kind: ListKind) -> ListPage {
        ListPage {
            kind,
            reply: WebReply::default(),
            members: Vec::new(),
            message: None,
            busy: false,
            hover: None,
        }
    }

    /// The page's own request (the list again).
    pub fn request(&self, user: &str) -> String {
        let url = match self.kind {
            ListKind::Search => "guild/list",
            ListKind::Manage => "manager/guildmember",
        };
        format!("/clan?url={url}&p={}&fmt=text", url_encode(user))
    }

    /// The rows' count and whether row `i` has its action button.
    pub fn rows(&self) -> usize {
        match self.kind {
            ListKind::Search => self.reply.clans.len(),
            ListKind::Manage => self.members.len(),
        }
        .min(ROW_MAX)
    }

    pub fn row_action(&self, i: usize, me: u32) -> bool {
        match self.kind {
            ListKind::Search => self.reply.clan == 0 && i < self.reply.clans.len(),
            ListKind::Manage => {
                self.reply.leader && self.members.get(i).is_some_and(|m| m.char_no != me)
            }
        }
    }

    /// The row action's request (join the row's clan, remove the row's member).
    pub fn row_request(&self, i: usize, user: &str) -> Option<String> {
        let p = url_encode(user);
        match self.kind {
            ListKind::Search => self
                .reply
                .clans
                .get(i)
                .map(|k| format!("/clan/join?p={p}&c={}&fmt=text", k.uid)),
            ListKind::Manage => self
                .members
                .get(i)
                .map(|m| format!("/clan/kick?p={p}&m={}&fmt=text", m.char_no)),
        }
    }

    pub fn hit(&self, p: [f32; 2], me: u32) -> Option<PageHit> {
        if !inside(PAGE, p) {
            return None;
        }
        for i in 0..self.rows() {
            if self.row_action(i, me) && inside(row_button(i), p) {
                return Some(PageHit::Row(i));
            }
        }
        if inside(REFRESH_BUTTON, p) {
            return Some(PageHit::Refresh);
        }
        if self.kind == ListKind::Search && inside(SECOND_BUTTON, p) {
            return Some(PageHit::Second);
        }
        Some(PageHit::Page)
    }

    /// A text-form answer for this page (the list, or an action's reply with the list again).
    pub fn answer(&mut self, body: &str) {
        let r = parse_reply(body);
        if let Some((_, m)) = &r.msg {
            self.message = Some(m.clone());
        }
        let members = parse_members(body);
        if self.kind == ListKind::Manage && (!members.is_empty() || r.msg.is_none()) {
            self.members = members;
        }
        if self.kind == ListKind::Search && r.clans.is_empty() && r.msg.is_some() {
            // an action answer without rows keeps the shown list
            self.reply.clan = r.clan;
            self.reply.leader = r.leader;
        } else {
            self.reply = WebReply { msg: None, ..r };
        }
        self.busy = false;
    }

    pub fn draw(&self, me: u32, out: &mut Vec<DrawItem>) {
        fill(PAGE, PAGE_FILL, out);
        let cols = match self.kind {
            ListKind::Search => SEARCH_COLUMNS,
            ListKind::Manage => MANAGE_COLUMNS,
        };
        for (x, t) in cols {
            text(t, [x, HEADER_Y], left_mid(), LABEL_COLOUR, out);
        }
        fill(RULE, RULE_COLOUR, out);
        for i in 0..self.rows() {
            let y = ROW_Y + i as i32 * ROW_STEP;
            let cells: Vec<(String, u32)> = match self.kind {
                ListKind::Search => {
                    let k = &self.reply.clans[i];
                    vec![
                        (k.name.clone(), ROW_COLOUR),
                        (k.members.to_string(), ROW_COLOUR),
                        (k.leader.clone(), ROW_COLOUR),
                    ]
                }
                ListKind::Manage => {
                    let m = &self.members[i];
                    let class = if m.leader { T_LEADER } else { T_MEMBER };
                    vec![
                        (class.to_string(), HINT_COLOUR),
                        (m.name.clone(), ROW_COLOUR),
                        (m.exp.to_string(), ROW_COLOUR),
                    ]
                }
            };
            for ((x, _), (t, c)) in cols.iter().zip(cells) {
                text(&t, [*x, y], left_mid(), c, out);
            }
            if self.row_action(i, me) {
                let cap = match self.kind {
                    ListKind::Search => T_JOIN,
                    ListKind::Manage => T_REMOVE,
                };
                button(
                    row_button(i),
                    cap,
                    !self.busy,
                    self.hover == Some(PageHit::Row(i)),
                    out,
                );
            }
        }
        if let Some(m) = &self.message {
            text(m, MESSAGE_AT, left_mid(), LABEL_COLOUR, out);
        }
        button(
            REFRESH_BUTTON,
            T_RELOAD,
            true,
            self.hover == Some(PageHit::Refresh),
            out,
        );
        if self.kind == ListKind::Search {
            button(
                SECOND_BUTTON,
                T_CREATE_A_CLAN,
                true,
                self.hover == Some(PageHit::Second),
                out,
            );
        }
    }
}

/// The leave page (the 2009 leave button's web page: a confirmation).
#[derive(Debug, Clone, PartialEq)]
pub struct LeavePage {
    pub clan: String,
    pub message: Option<String>,
    pub busy: bool,
    pub hover: Option<PageHit>,
}

impl LeavePage {
    pub fn new(clan: &str) -> LeavePage {
        LeavePage {
            clan: clan.to_string(),
            message: None,
            busy: false,
            hover: None,
        }
    }

    pub fn request(user: &str) -> String {
        format!("/clan/leave?p={}&fmt=text", url_encode(user))
    }

    pub fn hit(&self, p: [f32; 2]) -> Option<PageHit> {
        if !inside(PAGE, p) {
            return None;
        }
        if inside(LEAVE_BUTTON, p) {
            return Some(PageHit::Leave);
        }
        if inside(CANCEL_BUTTON, p) {
            return Some(PageHit::Cancel);
        }
        Some(PageHit::Page)
    }

    pub fn draw(&self, out: &mut Vec<DrawItem>) {
        fill(PAGE, PAGE_FILL, out);
        text(
            T_LEAVE_QUESTION,
            LEAVE_QUESTION_AT,
            centre(),
            ROW_COLOUR,
            out,
        );
        text(&self.clan, LEAVE_NAME_AT, centre(), LABEL_COLOUR, out);
        text(T_LEAVE_RULE, LEAVE_RULE_AT, centre(), HINT_COLOUR, out);
        button(
            LEAVE_BUTTON,
            T_LEAVE,
            !self.busy,
            self.hover == Some(PageHit::Leave),
            out,
        );
        button(
            CANCEL_BUTTON,
            T_CANCEL,
            true,
            self.hover == Some(PageHit::Cancel),
            out,
        );
        if let Some(m) = &self.message {
            text(m, MESSAGE_AT, left_mid(), LABEL_COLOUR, out);
        }
    }
}

/// Adds the name field of the create page to the sheet: an edit part under window 5 that is never drawn by the
/// sheet (the page draws it) but keeps the focus and the typing.
pub fn add_name_field(sheet: &mut Sheet, window: i32, page: dc_ui_data::PageTag) -> Option<usize> {
    let w = sheet
        .parts
        .iter()
        .position(|p| p.kind == WidgetKind::Window && p.id == window && p.page == page)?;
    let r = NAME_FIELD;
    sheet.parts.push(Part {
        page,
        kind: WidgetKind::EditBox,
        layer: window,
        id: -2,
        command: 0,
        pos: [r[0], r[1]],
        size: [r[2], r[3]],
        look: Look::Edit {
            font: 14,
            size: 14,
            max_len: NAME_MAX,
            caret_colour: 0xffff_ffff,
            caret_period: 0.333,
            clip: r[2],
            text_colour: 0,
        },
        parent: Some(w),
        shown: false,
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
        rec: usize::MAX,
        wrap: 0,
        wrap_pitch: 0,
        wrap_max: 0,
        colour_set: false,
        placed: true,
    });
    Some(sheet.parts.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_parse_and_queries_escape() {
        let r = parse_reply("me\t3\t1\nmsg\t0\tCannot x.\nclan\t3\tWolves\t2\tCarol\t4\t7\n");
        assert_eq!((r.clan, r.leader), (3, true));
        assert_eq!(r.msg, Some((false, "Cannot x.".into())));
        assert_eq!(r.clans[0].name, "Wolves");
        assert_eq!((r.clans[0].emblem, r.clans[0].background), (4, 7));
        assert_eq!(url_encode("A b&c"), "A%20b%26c");
        assert_eq!(
            web_addr(Some("127.0.0.1:20000".parse().unwrap())),
            Some("127.0.0.1:28080".parse().unwrap())
        );
    }

    #[test]
    fn grid_pick_and_pages() {
        let marks = ClanMarks {
            emblems: vec![None; 46],
            backgrounds: vec![None; 3],
            ..Default::default()
        };
        assert_eq!(CreatePage::pages(&marks, 0), 3);
        let mut c = CreatePage::new(None);
        assert_eq!(c.hit([248.0, 273.0]), Some(CreateHit::Cell(0, 0)));
        assert_eq!(c.hit([300.0, 401.0]), Some(CreateHit::Cell(1, 1)));
        c.press(&marks, CreateHit::Next(0));
        c.press(&marks, CreateHit::Cell(0, 3));
        assert_eq!(c.pick[0], 24);
        c.press(&marks, CreateHit::Next(0));
        c.press(&marks, CreateHit::Next(0));
        assert_eq!(c.page[0], 2, "stops at the last page");
        c.press(&marks, CreateHit::Cell(0, 9));
        assert_eq!(c.pick[0], 24, "past the table: no pick");
        c.press(&marks, CreateHit::Cell(1, 5));
        assert_eq!(c.pick[1], 1, "background 6 does not exist");
    }
}
