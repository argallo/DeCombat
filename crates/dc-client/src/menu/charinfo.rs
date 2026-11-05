//! The Character Info box: the window a profile reply (0x379) opens over the lobby, with the player's rank, experience,
//! kills / deaths / headshots and clan on its default page and the fireteam statistics on its second page.
//!
//! The 2009 code builds a dimmed screen, a 473 x 548 panel at the layout section's window place, a rank icon, an OK
//! button, a close button and two tab buttons ("Default Info", "Fireteam Info"); the reply fills the text lines
//! (cshell+0x2412c0). Positions of every line come from the layout section of the box.
// EVIDENCE: cshell+0x240040 (CCharacterInfoBox::Init: dim image 1024 x 768, panel 0x1d9 x 0x224 at (0x114, 0x43),
// rank icon 30 x 22 at the section's CharLevelMarkPos, OK button 102 x 25 at (0xba, 0x1f0) with string 0x42, close
// button 15 x 15 at (0x1ab, 0x15), the second panel at WindowPos, tabs 103 x 29 at ButtonNormalPos / ButtonCoopPos,
// every line a text control of the section's place with colour 0xffaaaaaa), cshell+0x2412c0 (BuildCharInfo: the lines),
// cshell+0x35cf0 (0x379 fills the box), cshell+0x23fb70 (the box shows its default page)
// EVIDENCE: cshell+0x241fd0 (CLI-961: the name colour number +0xa6 and the clan colour number +0xaa go through the
// colour table cshell+0x1dcda0 (entry n = `ColorN` of `NameColorTable` in Attributes\ColorTable.txt, 0 or past the
// table = the box's default grey) onto the name (control 0 of both pages) and the clan line), cshell+0x2412c0 DISASM
// 0x2417b5..0x2419a2 (a nation > 0: the flag image of country row `Country<nation>` of Attributes\CountryInfoTable.txt
// (`FlagImg`) at x = the name text's right edge + 15, y 179 (the name's own row), and `FullName(SimpleName)` at the
// right edge + 50, on both pages), cshell+0x35cf0 (+0x9e emblem, +0xa2 background of the clan mark)
// DC-DEFAULT: CLI-961 the clan mark (shown when the clan id and both mark numbers are set) is 26 x 26 at
// CharClanmarkPos, the lobby panel's frame size (the control's own size is not read)

use super::catalog::key;
use super::catalog::{pair, prefix_of_a, Catalog, KeyPin};
use super::link::ProfileView;
use super::lobby::PanelArt;
use super::sheet::{image, place_text, Caption, Look, Part, Sheet, ALIGN_CENTRE, ALIGN_LEFT};
use super::{Ctx, Go, MenuData, MenuKey, Screen};
use crate::hud::items::DrawItem;
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::WidgetKind;

pub const NAME: &str = "charinfo";

/// The box's layout section and its keys (pins by SHA-256 prefix and length).
// OBSERVED: the user's ROM layout table (section CharacterInfoBox)
pub const SECTION: KeyPin = key("c8c3a41e3d4d319f", 16);
mod keys {
    use super::{key, KeyPin};
    pub const WINDOW: KeyPin = key("7fd8459190304736", 9);
    pub const TAB_DEFAULT: KeyPin = key("907330841744d5db", 15);
    pub const TAB_FIRETEAM: KeyPin = key("c08aa255a04c992a", 13);
    pub const MARK: KeyPin = key("a9393ede2ee6f2fd", 16);
    pub const CLAN_MARK: KeyPin = key("6b7b9f192fbd9f88", 15);
    /// Default page lines in control order (name, level name, level exp, clan, total exp, rank, record, kills,
    /// deaths, headshots).
    pub const PAGE1: [KeyPin; 10] = [
        key("a25e3d3b215b3446", 11),
        key("ac92227f2f2dcdf7", 16),
        key("2aed4c31e8ebde96", 15),
        key("5d0b5689e7141b4a", 15),
        key("3b721f65b8e1ce3a", 15),
        key("a0789655f3bca6bf", 11),
        key("198adc63a565c7a0", 13),
        key("07e1b555b11f653c", 11),
        key("82509664ba7a7d28", 12),
        key("b9b543c16cb9cc58", 15),
    ];
    /// Fireteam page lines in control order (name, then record / kills / deaths / headshots / combos twice).
    pub const PAGE2: [KeyPin; 11] = [
        key("a25e3d3b215b3446", 11),
        key("be03f7007d31987b", 13),
        key("481b81bc33366c87", 11),
        key("6b4cad9c88e30b77", 12),
        key("32bf567435cd9b55", 15),
        key("26151b983ee271d1", 12),
        key("2869fa59fbf04cb7", 16),
        key("4687c47224351949", 14),
        key("05cd524dacc71f78", 15),
        key("101b85f3f0c58a0a", 18),
        key("b1312170f6a44774", 15),
    ];
}

/// Window numbers (the dim, the default page, the fireteam page) and control numbers / commands.
const DIM: i32 = 1;
const PAGE1: i32 = 2;
const PAGE2: i32 = 3;
const TABS: i32 = 4;
pub const CMD_DISMISS: i32 = 100_000;
pub const CMD_OK: i32 = 100_001;
pub const CMD_TAB_DEFAULT: i32 = 100_002;
pub const CMD_TAB_FIRETEAM: i32 = 100_003;

/// The text colour of every line.
const GREY: u32 = 0xffaa_aaaa;
const WHITE: u32 = 0xffff_ffff;
/// System font indices of the lines (the name 0xe, the values 0x19) and of the button captions.
const FONT_NAME: i32 = 14;
const FONT_VALUE: i32 = 25;
const SCREEN: [i32; 2] = [1024, 768];

const RANK_ICON: i32 = 20;

pub struct CharInfo {
    sheet: Sheet,
    page: usize,
    tabs: [usize; 2],
    selected: Option<String>,
    normal: Option<String>,
    /// CharClanmarkPos on the screen.
    mark_at: Option<[i32; 2]>,
}

impl CharInfo {
    pub fn new(cat: &Catalog, data: &MenuData) -> CharInfo {
        let ov = &cat.paths.overlay;
        let at = |k: &KeyPin| -> Option<[i32; 2]> { pair(cat.section_value(&SECTION, k)) };
        let tex = |t: &Option<String>| t.clone().unwrap_or_default();
        let window = at(&keys::WINDOW).unwrap_or([276, 67]);
        let mut sheet = Sheet::default();
        // the dimmed screen
        sheet.parts.push(Part::synthetic(
            WidgetKind::Window,
            DIM,
            DIM,
            0,
            [0, 0],
            SCREEN,
            Look::Panel {
                tex: tex(&ov.blind),
                stretch: true,
            },
        ));
        let panel_size = [0x1d9, 0x224];
        for (layer, t) in [(PAGE1, &ov.ci_panel), (PAGE2, &ov.ci_panel2)] {
            sheet.parts.push(Part::synthetic(
                WidgetKind::Window,
                layer,
                layer,
                0,
                window,
                panel_size,
                Look::Panel {
                    tex: tex(t),
                    stretch: false,
                },
            ));
        }
        let rel = |p: [i32; 2]| [window[0] + p[0], window[1] + p[1]];
        let child = |sheet: &mut Sheet, parent: usize, mut p: Part| {
            p.parent = Some(parent);
            sheet.parts.push(p);
        };
        let left = Align {
            first: ALIGN_LEFT,
            second: 0,
        };
        let line = |layer: i32, id: i32, at: [i32; 2], font: i32| {
            Part::synthetic(
                WidgetKind::Text,
                layer,
                id,
                0,
                at,
                [0, 0],
                Look::Text {
                    align: left,
                    font,
                    size: 0,
                    colour: GREY,
                },
            )
        };
        let centre = Align {
            first: ALIGN_CENTRE,
            second: ALIGN_SECOND_CENTRE,
        };
        let button = |layer: i32,
                      id: i32,
                      cmd: i32,
                      at: [i32; 2],
                      size: [i32; 2],
                      prefix: String,
                      string_id: i32| {
            Part::synthetic(
                WidgetKind::Button,
                layer,
                id,
                cmd,
                at,
                size,
                Look::Button {
                    prefix,
                    caption: Some(Caption {
                        string_id,
                        at: [size[0] / 2, size[1] / 2],
                        align: centre,
                        font: FONT_VALUE,
                        size: 0,
                        colours: [WHITE; 4],
                    }),
                    frame: None,
                },
            )
        };
        let ok_prefix = prefix_of_a(cat.paths.popup_buttons[0].as_deref()).unwrap_or_default();
        let close_prefix = prefix_of_a(ov.ci_close.as_deref()).unwrap_or_default();
        for (layer, parent) in [(PAGE1, 1usize), (PAGE2, 2usize)] {
            let lines: &[KeyPin] = if layer == PAGE1 {
                &keys::PAGE1
            } else {
                &keys::PAGE2
            };
            for (id, key) in lines.iter().enumerate() {
                let pos = at(key).unwrap_or([0, 0]);
                let font = if id == 0 { FONT_NAME } else { FONT_VALUE };
                child(&mut sheet, parent, line(layer, id as i32, rel(pos), font));
            }
            child(
                &mut sheet,
                parent,
                button(
                    layer,
                    30,
                    CMD_OK,
                    rel([0xba, 0x1f0]),
                    [0x66, 0x19],
                    ok_prefix.clone(),
                    0x42,
                ),
            );
            let mut close = button(
                layer,
                31,
                CMD_DISMISS,
                rel([0x1ab, 0x15]),
                [15, 15],
                close_prefix.clone(),
                0,
            );
            if let Look::Button { caption, .. } = &mut close.look {
                *caption = None;
            }
            child(&mut sheet, parent, close);
        }
        // the rank icon sits on the default page
        let icon_at = at(&keys::MARK).unwrap_or([175, 177]);
        child(
            &mut sheet,
            1,
            Part::synthetic(
                WidgetKind::Image,
                PAGE1,
                RANK_ICON,
                0,
                rel(icon_at),
                [0x1e, 0x16],
                Look::Panel {
                    tex: tex(&ov.ci_rank),
                    stretch: true,
                },
            ),
        );
        // the tabs sit on the screen
        let mut tabs = [0usize; 2];
        for (k, (cmd, key)) in [
            (CMD_TAB_DEFAULT, &keys::TAB_DEFAULT),
            (CMD_TAB_FIRETEAM, &keys::TAB_FIRETEAM),
        ]
        .into_iter()
        .enumerate()
        {
            let p = Part::synthetic(
                WidgetKind::Button,
                TABS,
                40 + k as i32,
                cmd,
                at(key).unwrap_or([300 + 105 * k as i32, 118]),
                [0x67, 0x1d],
                Look::Panel {
                    tex: String::new(),
                    stretch: false,
                },
            );
            tabs[k] = sheet.parts.len();
            let (tab_at, tab_size) = (p.pos, p.size);
            sheet.parts.push(p);
            // the caption centred on the tab
            let mut t = Part::synthetic(
                WidgetKind::Text,
                TABS,
                50 + k as i32,
                0,
                [tab_at[0] + tab_size[0] / 2, tab_at[1] + tab_size[1] / 2],
                [0, 0],
                Look::Text {
                    align: centre,
                    font: FONT_VALUE,
                    size: 0,
                    colour: WHITE,
                },
            );
            t.text = ov.ci_tab_names[k].clone().unwrap_or_default();
            sheet.parts.push(t);
        }
        let mut s = CharInfo {
            sheet,
            page: 0,
            tabs,
            selected: ov.ci_tab_selected.clone(),
            normal: ov.ci_tab.clone(),
            mark_at: at(&keys::CLAN_MARK).map(rel),
        };
        s.fill(cat, data);
        s.show_page(0);
        s
    }

    /// The text lines from the profile.
    fn fill(&mut self, cat: &Catalog, data: &MenuData) {
        let p = data.char_view.clone().unwrap_or_default();
        let art = cat.rom.as_deref().and_then(PanelArt::cached);
        let lines = lines(cat, &p, art.as_ref());
        let tables = cat.rom.as_deref().map(Tables::cached).unwrap_or_default();
        let name_colour = tables.name_colour(p.name_colour);
        let clan_colour = tables.name_colour(p.clan_colour);
        for (layer, id, c) in [
            (PAGE1, 0, name_colour),
            (PAGE2, 0, name_colour),
            (PAGE1, 3, clan_colour),
        ] {
            if let Some(c) = c {
                self.sheet.set_colour(layer, id, c);
            }
        }
        for (layer, rows) in [(PAGE1, &lines.0[..]), (PAGE2, &lines.1[..])] {
            for (id, text) in rows.iter().enumerate() {
                self.sheet.set_text(layer, id as i32, text);
            }
        }
        if let (Some(art), Some(part)) = (
            art.as_ref(),
            self.sheet.part_mut(PAGE1, RANK_ICON, WidgetKind::Image),
        ) {
            if let (Some(icon), Look::Panel { tex, .. }) =
                (art.levels.icon_of_exp(p.exp, &p.name), &mut part.look)
            {
                *tex = icon.to_string();
            }
        }
    }

    /// Shows page `k` (0 default, 1 fireteam); the tab of the shown page takes the selected look.
    fn show_page(&mut self, k: usize) {
        self.page = k;
        for w in self.sheet.parts.iter_mut() {
            if w.kind == WidgetKind::Window && (w.layer == PAGE1 || w.layer == PAGE2) {
                w.shown = (w.layer == PAGE1) == (k == 0);
            }
        }
        for (n, &i) in self.tabs.iter().enumerate() {
            let tex = if n == k { &self.selected } else { &self.normal };
            if let Look::Panel { tex: t, .. } = &mut self.sheet.parts[i].look {
                *t = tex.clone().unwrap_or_default();
            }
        }
    }
}

/// The name colour table and the country table of the user's ROM.
#[derive(Debug, Clone, Default)]
pub struct Tables {
    /// `Color1`.. of `NameColorTable` as ARGB.
    pub colours: Vec<u32>,
    /// `Country0`.. : full name, short name, flag texture.
    pub countries: Vec<(String, String, String)>,
}

impl Tables {
    pub fn cached(rom: &std::path::Path) -> Tables {
        Tables::with_cached(rom, Tables::clone)
    }

    /// `f` over the tables of `rom`, read once per ROM folder and process.
    fn with_cached<R>(rom: &std::path::Path, f: impl FnOnce(&Tables) -> R) -> R {
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::sync::{Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, Tables>>> = OnceLock::new();
        let mut c = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        f(c.entry(rom.to_path_buf())
            .or_insert_with(|| Tables::read(rom)))
    }

    fn read(rom: &std::path::Path) -> Tables {
        use dc_bute::ButeDoc;
        let vfs = dc_rez::RezVfs::new(rom);
        let mut t = Tables::default();
        if let Some(d) = ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\COLORTABLE.TXT") {
            let max = d.get_int("NameColorTable", "ColorMax", 0).max(0);
            for n in 1..=max {
                let v = d
                    .get_str("NameColorTable", &format!("Color{n}"))
                    .unwrap_or("");
                let c: Vec<f32> = v
                    .trim_matches(|c| c == '<' || c == '>')
                    .split(',')
                    .filter_map(|x| x.trim().parse().ok())
                    .collect();
                let g = |i: usize| c.get(i).copied().unwrap_or(0.0).clamp(0.0, 255.0) as u32;
                t.colours.push(0xff00_0000 | g(0) << 16 | g(1) << 8 | g(2));
            }
        }
        if let Some(d) =
            ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\COUNTRYINFOTABLE.TXT")
        {
            let mut n = 0;
            while d.get(&format!("Country{n}")).is_some() {
                let s = |k: &str| {
                    d.get_str(&format!("Country{n}"), k)
                        .unwrap_or("")
                        .to_string()
                };
                t.countries
                    .push((s("FullName"), s("SimpleName"), s("FlagImg")));
                n += 1;
            }
        }
        t
    }

    /// The colour of colour number `n` (1-based); none for 0 or past the table.
    // EVIDENCE: cshell+0x1dcda0 (0 < n <= count: entry n - 1, else 0 = the caller's default)
    pub fn name_colour(&self, n: u32) -> Option<u32> {
        (n > 0)
            .then(|| self.colours.get(n as usize - 1).copied())
            .flatten()
    }
}

/// The colour of name colour number `n` of the user's ROM (none without a ROM, for 0 or past the table): the one
/// lookup behind every lobby place that paints a name or a clan name in a colour item's colour (CLI-1887).
// EVIDENCE: cshell+0x1dcda0 (0 < n <= count: entry n - 1, else 0 = the caller's default colour)
pub fn table_colour(rom: Option<&std::path::Path>, n: u32) -> Option<u32> {
    if n == 0 {
        return None;
    }
    Tables::with_cached(rom?, |t| t.name_colour(n))
}

/// The ordinal form of a rank: 1st, 2nd, 3rd, 4th, 11th..13th.
// EVIDENCE: cshell+0x23f9b0 (last digit 1 / 2 / 3 give st / nd / rd unless the number ends in 11 / 12 / 13)
pub fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, r) if r != 11 => "st",
        (2, r) if r != 12 => "nd",
        (3, r) if r != 13 => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// The kills-per-death ratio.
// EVIDENCE: cshell+0x1b15c0 (1.0 unless the kills are not 0; without deaths the kills; else kills / deaths)
pub fn ratio(kills: u32, deaths: u32) -> f64 {
    if kills == 0 {
        1.0
    } else if deaths == 0 {
        f64::from(kills)
    } else {
        f64::from(kills) / f64::from(deaths)
    }
}

fn pct(part: u32, whole: u32) -> f64 {
    if whole > 0 {
        f64::from(part) * 100.0 / f64::from(whole)
    } else {
        0.0
    }
}

/// The text lines of the two pages, in the control order of [`LINES1`] / [`LINES2`].
// EVIDENCE: cshell+0x2412c0 (formats: "%d exp", "%d / %d (%3.2f %%)", "Unranked" below level 4 else the ordinal of the
// rank, "-" for the record line, string 0x18e4 kills with the ratio, "%d Deaths", the headshot share; the fireteam
// page: strings 0x18e9 total / cleared, 0x18ea kills, 0x18eb deaths, 0x18ec headshot share, 0x18ed combos)
pub fn lines(cat: &Catalog, p: &ProfileView, art: Option<&PanelArt>) -> (Vec<String>, Vec<String>) {
    let level = art.map_or(0, |a| a.levels.level_of(p.exp));
    let (part, span) = art.map_or((0, 0), |a| a.part_span(p.exp));
    let clan = if p.clan_id == 0 {
        cat.text(0x41a)
    } else {
        p.clan.clone()
    };
    let rank = if level > 3 {
        ordinal(p.rank)
    } else {
        "Unranked".to_string()
    };
    let t = |id: i32, a: &[f64]| -> String { form(&cat.text(id), a) };
    let c = &p.coop;
    let page1 = vec![
        p.name.clone(),
        art.map_or(String::new(), |a| a.short_name(p.exp).to_string()),
        format!("{} / {} ({:3.2} %)", part, span, pct(part, span)),
        clan,
        format!("{} exp", p.exp),
        rank,
        "-".to_string(),
        t(0x18e4, &[f64::from(p.kills), ratio(p.kills, p.deaths)]),
        format!("{} Deaths", p.deaths),
        format!(
            "{} / {} ({:3.2} %)",
            p.headshots,
            p.kills,
            pct(p.headshots, p.kills)
        ),
    ];
    let group = |base: usize| -> Vec<String> {
        // base: 0 fireteam (words 0..5), 6 the second group (words 6..11)
        let (kills, deaths, hs, combos, total, cleared) = if base == 0 {
            (c[0], c[1], c[2], c[3], c[4], c[5])
        } else {
            (c[6], c[7], c[8], c[9], c[10], c[11])
        };
        vec![
            t(0x18e9, &[f64::from(total), f64::from(cleared)]),
            t(0x18ea, &[f64::from(kills)]),
            t(0x18eb, &[f64::from(deaths)]),
            t(0x18ec, &[f64::from(hs), f64::from(kills), pct(hs, kills)]),
            t(0x18ed, &[f64::from(combos)]),
        ]
    };
    let mut page2 = vec![p.name.clone()];
    page2.extend(group(0));
    page2.extend(group(6));
    (page1, page2)
}

/// Fills a `%d` / `%3.2f` format with numbers in order (`%%` is one per cent sign).
fn form(f: &str, args: &[f64]) -> String {
    let mut out = String::new();
    let mut it = args.iter();
    let mut chars = f.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        let mut spec = String::new();
        while let Some(&n) = chars.peek() {
            chars.next();
            if n == 'd' || n == 'f' || n == '%' {
                spec.push(n);
                break;
            }
            spec.push(n);
        }
        match spec.chars().last() {
            Some('%') => out.push('%'),
            Some('d') => out.push_str(&format!("{}", it.next().copied().unwrap_or(0.0) as i64)),
            Some('f') => out.push_str(&format!("{:3.2}", it.next().copied().unwrap_or(0.0))),
            _ => out.push_str(&spec),
        }
    }
    out
}

impl Screen for CharInfo {
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
    /// CLI-961: the nation flag and its name after the player's name, and the clan mark on the default page.
    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, _secs: f32, out: &mut Vec<DrawItem>) {
        let Some(p) = data.char_view.as_ref() else {
            return;
        };
        let Some(rom) = cat.rom.clone() else {
            return;
        };
        let layer = if self.page == 0 { PAGE1 } else { PAGE2 };
        if p.nation > 0 {
            let tables = Tables::cached(&rom);
            let name = self.sheet.part(layer, 0, WidgetKind::Text);
            if let (Some((full, short, flag)), Some(name)) =
                (tables.countries.get(p.nation as usize), name)
            {
                let sx = cat.size().0 as f32 / 1024.0;
                let width = cat
                    .assets
                    .fonts
                    .atlas_sized(FONT_NAME, None)
                    .map_or(p.name.chars().count() as f32 * 7.0, |a| {
                        a.measure(&p.name) / sx.max(f32::EPSILON)
                    });
                let right = name.pos[0] + width.round() as i32;
                if let Some(t) = cat.assets.texture(flag) {
                    out.push(image(
                        flag,
                        [right + 15, name.pos[1]],
                        [t.width as i32, t.height as i32],
                    ));
                }
                out.push(place_text(
                    &format!("{full}({short})"),
                    [right + 50, name.pos[1]],
                    [0, 0],
                    Align {
                        first: ALIGN_LEFT,
                        second: 0,
                    },
                    FONT_VALUE,
                    0,
                    GREY,
                ));
            }
        }
        if self.page == 0 && p.clan_id != 0 && p.emblem > 0 && p.background > 0 {
            let lits = super::clan::ClanLits::cached(&rom);
            if let (Some((e, b)), Some(at)) =
                (lits.marks.textures(p.emblem, p.background), self.mark_at)
            {
                super::clanpage::mark_cropped(Some(b), Some(e), [at[0], at[1], 26, 26], out);
            }
        }
    }
    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        if k == MenuKey::Escape {
            self.click(CMD_DISMISS, usize::MAX, ctx);
        }
    }
    fn click(&mut self, cmd: i32, _part: usize, ctx: &mut Ctx) {
        match cmd {
            CMD_DISMISS | CMD_OK => *ctx.go = Some(Go::Back),
            CMD_TAB_DEFAULT => self.show_page(0),
            CMD_TAB_FIRETEAM => self.show_page(1),
            _ => {}
        }
    }
}
