//! The 2009 loading screen (interface state 3) and the boxes around a match: the page the client shows from the
//! game start until the world is in (map picture tiles and the loading bar of the HUD layout set), the load stage
//! the server's messages advance, the waiting room's disconnect box after a match that ended badly, and the
//! leave-the-match box.
//!
//! 2009 flow: the lobby's game start switches the interface to the loading state before the engine connects
//! (`match_connect` cs+0x1630a0 -> mission `enter_loading` cs+0x22fa50 -> state change cs+0x1bb0b0(3), whose enter
//! step cs+0x1b56e0 shows the loading screen cs+0x28d8a0); every frame in that state `loading_tick` cs+0x1bb910
//! draws the page again (cs+0x28d640) until the in-world test passes, then hides it and enters the playing state.
//! DeCombat draws the page in the menu's window (the same window the match then plays in: no window swap).
// SPEC: UDP_SESSION_2009_A.md §2.2 step 4 (interface state 3 = loading screen), §6.3 (timeout box), §8.3 (in-world test)
// EVIDENCE: cshell+0x1630a0, cshell+0x1bb0b0, cshell+0x1b56e0, cshell+0x28d8a0, cshell+0x1bb910
// DISASM: cshell+0x22fa50 (`mission_enter_loading`, UDP_SESSION_2009_A.md §2.2 step 4 and §8.3 c)
// EVIDENCE: cshell+0x28d640 (the page: tiles at (0, 0) and (0, 0x200), then the bar)

use crate::hud::compose::Tex;
use crate::hud::items::DrawItem;
use crate::menu::catalog::{find_key, key, Catalog, KeyPin};
use crate::menu::sheet::Sheet;
use crate::menu::{MenuData, Screen};

/// Values of the client shell's load stage (`client_shell`+0xc3b0): the loading bar's base part is
/// `bar width / (9 - stage)`.
// EVIDENCE: cshell+0x28cc90 (base = width / (9 - stage))
pub mod stage {
    /// The game start (mission `start_client`). EVIDENCE: cshell+0x22f8a0
    pub const JOIN: u8 = 1;
    /// Player status 0x66 kind 0x13 (status reset). SPEC: GAME_MESSAGES_2009_S2C.md row 0x66 (cshell+0x5cb80)
    pub const STATUS_RESET: u8 = 5;
    /// Weapon give 0xb2. SPEC: GAME_MESSAGES_2009_S2C.md row 0xb2 (cshell+0x60f20..0x6118e)
    pub const WEAPON_GIVE: u8 = 6;
    /// Player status 0x66 kind 1 (ammunition). SPEC: GAME_MESSAGES_2009_S2C.md row 0x66 (cshell+0x60dda)
    pub const AMMO: u8 = 7;
    /// The in-world test passed. EVIDENCE: cshell+0x1bb910
    pub const IN_WORLD: u8 = 8;
}

/// The load stage after a message writes `to`: the 2009 writes are unconditional (a respawn's weapon give writes 6
/// again), but the page is gone once in world, so DeCombat keeps [`stage::IN_WORLD`] once reached and later writes
/// cannot make the stage read as loading again.
// SPEC: GAME_MESSAGES_2009_S2C.md rows 0x66 / 0xb2 (`client_shell`+0xc3b0 := n)
// DC-DEFAULT: CLI-507 (proposed) the in-world stage is kept once reached
pub fn next_stage(now: u8, to: u8) -> u8 {
    if now >= stage::IN_WORLD {
        now
    } else {
        to
    }
}

/// The 0x66 status kind that sets [`stage::STATUS_RESET`].
// SPEC: GAME_MESSAGES_2009_S2C.md row 0x66 (kind 0x13 -> cshell+0x63160)
pub const STATUS_RESET_KIND: u8 = 0x13;

/// The 2009 page's counter grows by this many units per page update (one per `loading_tick`).
// EVIDENCE: cshell+0x28cc90 (`+0x1518 += 0x16`; reset to 0 by cshell+0x28cae0 when the page is shown)
pub const UNITS_PER_UPDATE: f32 = 22.0;

/// Page updates per second of the clock. The 2009 client updates the page once per frame of its loading loop, a
/// rate DeCombat does not copy (frame-rate independence): the counter follows the clock at this rate.
// DC-DEFAULT: CLI-507 (proposed) the loading bar's counter grows 22 units per 1/8 s of the clock
pub const UPDATES_PER_SECOND: f32 = 8.0;

/// The page's tile grid: rows x columns of the top picture (three rows without a bottom picture, two with one)
/// and of the bottom picture (one row), and where the bottom picture starts.
// EVIDENCE: cshell+0x28d640 (top: `(rows, 4)` with rows 3, or 2 when the bottom name is not empty; bottom: `(1, 4)`
// at y 0x200), cshell+0x1dbe80 (tile files `<name><row><col>.dtx`, `<name><col>.dtx` for one row, `<name>.dtx` for
// one tile), cshell+0x1dbaf0 (row heights and column widths: the largest tile of each)
pub const TOP_ROWS_ALONE: usize = 3;
pub const TOP_ROWS_WITH_BOTTOM: usize = 2;
pub const COLUMNS: usize = 4;
pub const BOTTOM_Y: f32 = 512.0;

/// Keys of the room table's map rows (the loading pictures by team) and of the HUD layout set 0 (the bar).
// EVIDENCE: cshell+0x265780 (map row: alpha picture +0x514, bravo +0x618, alpha bottom +0x71c, bravo bottom +0x820)
// EVIDENCE: cshell+0x263510, cshell+0x2636b0 (team 2 takes the bravo field, any other the alpha field; a map section
// entry `<prefix><mode key>` replaces both when present)
// EVIDENCE: cshell+0x28cae0 (bar position, size and texture of the HUD layout set 0)
pub mod keys {
    use super::{key, KeyPin};
    pub const TOP_ALPHA: KeyPin = key("41ee5d9550866510", 16);
    pub const TOP_BRAVO: KeyPin = key("91f8e1954f70f78b", 16);
    pub const BOTTOM_ALPHA: KeyPin = key("a19c3c529ad6f8ca", 22);
    pub const BOTTOM_BRAVO: KeyPin = key("c443895eb67c1660", 22);
    /// The prefix of the per-mode picture entry (`<prefix><mode key>`).
    pub const MODE_PICTURE_PREFIX: KeyPin = key("25dcba20d54da4aa", 11);
    /// The mode row's key (as `Catalog::map_description` reads it).
    pub const MODE_KEY: KeyPin = key("99a52df3ff3d4994", 3);
    pub const BAR_POS: KeyPin = key("51ac47b9e4815e17", 13);
    pub const BAR_SIZE: KeyPin = key("66708efe32a868bf", 14);
    pub const BAR_TEX: KeyPin = key("11d2af0a99874365", 13);
}

/// The bar's place, size (virtual pixels) and texture.
#[derive(Debug, Clone, PartialEq)]
pub struct BarLayout {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub tex: Option<String>,
}

impl BarLayout {
    /// Our bar without a ROM (no layout set): centred low on the screen.
    // DC-DEFAULT: CLI-508 (proposed) without the ROM's bar keys: a 500 x 12 bar at (262, 730), drawn as fills
    pub fn fallback() -> BarLayout {
        BarLayout {
            pos: [262.0, 730.0],
            size: [500.0, 12.0],
            tex: None,
        }
    }
}

/// What the page draws: the picture prefixes (top, bottom) and the bar; for a community map (MAP-32) one whole
/// picture (the record's `loading`, a `@mod:` path) stretched over the page instead of the tiles.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadingPage {
    pub top: Option<String>,
    pub bottom: Option<String>,
    pub whole: Option<String>,
    pub bar: BarLayout,
}

/// The page a whole picture is stretched over (virtual pixels).
// DC-DEFAULT: MAP-32 (proposed) a community map's loading picture fills the 1024 x 768 page
pub const WHOLE_PAGE: [f32; 4] = [0.0, 0.0, 1024.0, 768.0];

/// The draw item of a whole loading picture (`None`: the picture does not load; the page stays bare).
pub fn whole_item(cat: &mut Catalog, path: &str) -> Option<DrawItem> {
    let (w, h) = cat.tex_size(path)?;
    Some(DrawItem::Image {
        tex: path.to_string(),
        src: [w as f32, h as f32],
        dst: WHOLE_PAGE,
        tint: [255; 4],
        turn: None,
    })
}

fn clean(v: &str) -> Option<String> {
    let v = v.trim().trim_matches('"').trim();
    (!v.is_empty()).then(|| v.to_string())
}

impl LoadingPage {
    /// The page of map row `map` for mode row `mode` and the player's team (`2` = bravo; any other alpha).
    pub fn from_catalog(cat: &Catalog, map: i32, mode: i32, team: i32) -> LoadingPage {
        // MAP-32: a community map's page is its record's loading picture (none: the bare page with the bar)
        if let Some(m) = cat.community_map(map) {
            return LoadingPage {
                top: None,
                bottom: None,
                whole: m.loading.clone(),
                bar: bar_layout(cat).unwrap_or_else(BarLayout::fallback),
            };
        }
        let bravo = team == 2;
        let (mut top, mut bottom) = (None, None);
        if let Some(row) = cat.map_row(map) {
            let (t, b) = if bravo {
                (&keys::TOP_BRAVO, &keys::BOTTOM_BRAVO)
            } else {
                (&keys::TOP_ALPHA, &keys::BOTTOM_ALPHA)
            };
            top = find_key(row.iter(), t).and_then(clean);
            bottom = find_key(row.iter(), b).and_then(clean);
            // the map section's per-mode entry replaces both pictures
            // EVIDENCE: cshell+0x263510 / cshell+0x2636b0 (`<prefix><mode key>`, read when the entry exists; the
            // entry name is formatted `<prefix>%s` with the 11-character prefix, whose last character is the `_`
            // separator, so no separator is added here: same `<name>_<key>` form as `Catalog::map_description`; the
            // `%s` is the entry of the shell's mode-name list at +0x15e0 for the mode)
            let mode_key = cat
                .mode_row(mode)
                .and_then(|m| find_key(m.iter(), &keys::MODE_KEY))
                .and_then(clean);
            if let Some(mk) = mode_key {
                let special = row.iter().find(|(k, _)| {
                    let n = keys::MODE_PICTURE_PREFIX.len;
                    k.len() == n + mk.len()
                        && k.is_char_boundary(n)
                        && k[n..].eq_ignore_ascii_case(&mk)
                        && dc_ui_data::names::matches_pin(
                            &k[..n],
                            keys::MODE_PICTURE_PREFIX.sha16,
                            n,
                        )
                });
                if let Some(v) = special.and_then(|(_, v)| clean(v)) {
                    top = Some(v.clone());
                    bottom = Some(v);
                }
            }
        }
        LoadingPage {
            top,
            bottom,
            whole: None,
            bar: bar_layout(cat).unwrap_or_else(BarLayout::fallback),
        }
    }
}

/// The bar of the HUD layout set 0 (`None` without one).
pub fn bar_layout(cat: &Catalog) -> Option<BarLayout> {
    let set = cat.layout.as_ref()?.hud_set(0)?;
    let pair = |k: &KeyPin| crate::menu::catalog::pair(find_key(set.iter(), k));
    let pos = pair(&keys::BAR_POS)?;
    let size = pair(&keys::BAR_SIZE)?;
    Some(BarLayout {
        pos: [pos[0] as f32, pos[1] as f32],
        size: [size[0] as f32, size[1] as f32],
        tex: find_key(set.iter(), &keys::BAR_TEX).and_then(clean),
    })
}

/// The page updates the bar has had `elapsed_s` seconds after the page opened (one at the opening, then
/// [`UPDATES_PER_SECOND`]): the bar only changes when this number or the stage does, so the menu redraws the page
/// only then.
// DC-DEFAULT: CLI-507 (proposed)
pub fn bar_updates(elapsed_s: f32) -> u32 {
    1 + (elapsed_s.max(0.0) * UPDATES_PER_SECOND).floor() as u32
}

/// The 2009 bar's filled width (virtual pixels) of a bar `width` wide, `elapsed_s` seconds after the page opened,
/// at load stage `stage`: the counter (22 units per update) plus `width / (9 - stage)`, at most `width`. The page
/// updates once when it opens.
// EVIDENCE: cshell+0x28cc90 (`fill = counter + width / (9 - stage)`, capped at the width; counter += 0x16 before it),
// cshell+0x28d8a0 (the page is updated once when shown)
// DC-DEFAULT: CLI-507 (proposed) see [`UPDATES_PER_SECOND`]
pub fn bar_fill(width: f32, stage: u8, elapsed_s: f32) -> f32 {
    let updates = bar_updates(elapsed_s) as f32;
    let base = width / (9.0 - f32::from(stage.min(8)));
    (updates * UNITS_PER_UPDATE + base).min(width)
}

/// One piece of the bar: the texture slice (column 0 left cap, 1 middle, 2 right cap; row 0 filled, 1 empty), its
/// destination rectangle (virtual pixels) and the share of the slice's texels it shows (from its left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarPiece {
    pub col: usize,
    pub row: usize,
    pub dst: [f32; 4],
    pub share: f32,
}

/// Texture columns of the bar texture (u) and rows (v).
// EVIDENCE: cshell+0x28cae0 (u 0, 0.25, 0.75, 1 and v 0, 0.5, 1: six slices)
pub const BAR_U: [f32; 4] = [0.0, 0.25, 0.75, 1.0];
pub const BAR_V: [f32; 3] = [0.0, 0.5, 1.0];

/// The bar as pieces: the ends are half the bar's height wide, the middle stretches; the filled row covers
/// `[0, fill)` and the empty row the rest.
// EVIDENCE: cshell+0x28cc90 (cap width = height * 0.5, middle = width - 2 caps; filled pieces up to the fill, the
// empty row's pieces after it)
pub fn bar_pieces(size: [f32; 2], fill: f32) -> Vec<BarPiece> {
    let (w, h) = (size[0], size[1]);
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let cap = (h * 0.5).min(w * 0.5);
    let spans = [(0.0, cap), (cap, w - cap), (w - cap, w)];
    let fill = fill.clamp(0.0, w);
    let mut out = Vec::new();
    for (row, (from, to)) in [(0, (0.0, fill)), (1, (fill, w))] {
        for (col, (a, b)) in spans.iter().copied().enumerate() {
            let (x0, x1) = (a.max(from), b.min(to));
            if x1 - x0 <= 0.0 || b - a <= 0.0 {
                continue;
            }
            out.push(BarPiece {
                col,
                row,
                dst: [x0, 0.0, x1 - x0, h],
                share: (x1 - x0) / (b - a),
            });
        }
    }
    out
}

/// The cached name of slice (`col`, `row`) of bar texture `tex`.
fn slice_name(tex: &str, col: usize, row: usize) -> String {
    format!("{tex}#slice{col}{row}")
}

/// Cuts the bar texture into its six slices once (cached in the assets under [`slice_name`]); false without it.
fn cut_bar(cat: &mut Catalog, tex: &str) -> bool {
    if cat.assets.generated(&slice_name(tex, 0, 0)).is_some() {
        return true;
    }
    let Some(t) = cat.assets.texture(tex) else {
        return false;
    };
    for row in 0..2 {
        for col in 0..3 {
            let x0 = (BAR_U[col] * t.width as f32).round() as u32;
            let x1 = (BAR_U[col + 1] * t.width as f32).round() as u32;
            let y0 = (BAR_V[row] * t.height as f32).round() as u32;
            let y1 = (BAR_V[row + 1] * t.height as f32).round() as u32;
            let (w, h) = (x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for y in y0..y0 + h {
                for x in x0..x0 + w {
                    let (cx, cy) = (x.min(t.width - 1), y.min(t.height - 1));
                    let o = ((cy * t.width + cx) * 4) as usize;
                    rgba.extend_from_slice(&t.rgba[o..o + 4]);
                }
            }
            cat.assets.put(
                &slice_name(tex, col, row),
                Tex {
                    width: w,
                    height: h,
                    rgba,
                },
            );
        }
    }
    true
}

/// The bar's draw items at fill `fill`.
pub fn bar_items(cat: &mut Catalog, bar: &BarLayout, fill: f32, out: &mut Vec<DrawItem>) {
    let pieces = bar_pieces(bar.size, fill);
    let textured = bar.tex.as_deref().filter(|t| cut_bar(cat, t));
    for p in pieces {
        let dst = [
            bar.pos[0] + p.dst[0],
            bar.pos[1] + p.dst[1],
            p.dst[2],
            p.dst[3],
        ];
        match textured {
            Some(t) => {
                let name = slice_name(t, p.col, p.row);
                let (w, h) = cat.tex_size(&name).unwrap_or((1, 1));
                out.push(DrawItem::Image {
                    tex: name,
                    src: [w as f32 * p.share, h as f32],
                    dst,
                    tint: [255; 4],
                    turn: None,
                });
            }
            // DC-DEFAULT: CLI-508 (proposed) the fallback bar's colours (filled light grey, empty dark grey)
            None => out.push(DrawItem::Fill {
                rect: dst,
                colour: if p.row == 0 {
                    [200, 200, 200, 255]
                } else {
                    [60, 60, 60, 255]
                },
            }),
        }
    }
}

/// Tile file of row `r`, column `c` of a `rows` x `cols` picture named `name`.
// EVIDENCE: cshell+0x1dbe80 (`%s.dtx`, `%s%d.dtx` with the column for one row, `%s%d.dtx` with the row for one
// column, `%s%d%d.dtx` with row and column)
pub fn tile_file(name: &str, rows: usize, cols: usize, r: usize, c: usize) -> String {
    match (rows > 1, cols > 1) {
        (false, false) => format!("{name}.dtx"),
        (false, true) => format!("{name}{c}.dtx"),
        (true, false) => format!("{name}{r}.dtx"),
        (true, true) => format!("{name}{r}{c}.dtx"),
    }
}

/// The tiles of a picture at `origin`: each row as high as its tallest tile, each column as wide as its widest,
/// one texel per virtual pixel; missing tiles leave their cell empty.
// EVIDENCE: cshell+0x1dbaf0 (largest height per row, largest width per column), cshell+0x1dbbc0 (cells placed by
// those sums from the origin)
pub fn tile_items(
    cat: &mut Catalog,
    name: &str,
    rows: usize,
    cols: usize,
    origin: [f32; 2],
    out: &mut Vec<DrawItem>,
) {
    let mut sizes = vec![vec![None; cols]; rows];
    for (r, row) in sizes.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = cat.tex_size(&tile_file(name, rows, cols, r, c));
        }
    }
    let heights: Vec<f32> = sizes
        .iter()
        .map(|row| row.iter().flatten().map(|s| s.1).max().unwrap_or(0) as f32)
        .collect();
    let widths: Vec<f32> = (0..cols)
        .map(|c| {
            sizes
                .iter()
                .filter_map(|row| row[c])
                .map(|s| s.0)
                .max()
                .unwrap_or(0) as f32
        })
        .collect();
    let mut y = origin[1];
    for (r, row) in sizes.iter().enumerate() {
        let mut x = origin[0];
        for (c, cell) in row.iter().enumerate() {
            if let Some((w, h)) = cell {
                out.push(DrawItem::Image {
                    tex: tile_file(name, rows, cols, r, c),
                    src: [*w as f32, *h as f32],
                    dst: [x, y, *w as f32, *h as f32],
                    tint: [255; 4],
                    turn: None,
                });
            }
            x += widths[c];
        }
        y += heights[r];
    }
}

/// The loading screen as a menu screen ([`crate::menu::Go::Loading`]): no controls and no cursor, the page drawn by
/// [`Screen::draw_extra`] from the menu clock.
pub struct LoadingScreen {
    sheet: Sheet,
    pub page: LoadingPage,
}

impl LoadingScreen {
    pub fn new(cat: &Catalog, data: &MenuData) -> LoadingScreen {
        let team = data.members.get(&data.profile.my_id).map_or(1, |m| m.team);
        let mode = data.room.mode.max(data.mode);
        LoadingScreen {
            sheet: Sheet::default(),
            page: LoadingPage::from_catalog(cat, data.room.map, mode, team),
        }
    }
}

impl Screen for LoadingScreen {
    fn name(&self) -> &'static str {
        "loading"
    }

    fn sheet(&self) -> &Sheet {
        &self.sheet
    }

    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, secs: f32, out: &mut Vec<DrawItem>) {
        let p = &self.page;
        let rows = if p.bottom.is_some() {
            TOP_ROWS_WITH_BOTTOM
        } else {
            TOP_ROWS_ALONE
        };
        if let Some(t) = &p.top {
            tile_items(cat, t, rows, COLUMNS, [0.0, 0.0], out);
        }
        if let Some(b) = &p.bottom {
            tile_items(cat, b, 1, COLUMNS, [0.0, BOTTOM_Y], out);
        }
        if let Some(w) = p.whole.as_deref().and_then(|w| whole_item(cat, w)) {
            out.push(w);
        }
        let since = data.loading.map_or(0.0, |l| secs - l.since);
        let stage = data.loading.map_or(stage::JOIN, |l| l.stage);
        let fill = bar_fill(p.bar.size[0], stage, since);
        bar_items(cat, &p.bar, fill, out);
    }
}

/// The loading page's state in the menu data: when it opened (menu clock) and the load stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadingState {
    pub since: f32,
    pub stage: u8,
}

/// How a match session ended, for the box the waiting room opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// Played to its end or left by the player (no box).
    Played,
    /// The join failed or timed out (the engine start result was not zero).
    JoinFailed,
    /// The link was lost (idle time-out, a fatal message, a transport drop).
    Dropped,
    /// The server closed the session with this transport bye code.
    ServerBye(u32),
}

/// The disconnect box's string id for disconnect reason `reason` (the shell's `Disconnected` cases).
// EVIDENCE: cshell+0x1bb2f0 (1 -> 0x3ef, 2 -> 0x3f2, 3 -> 0x178c, 6 -> 0x5bb, 7 -> 0x432, 8 -> 0x462, 9 -> 0xbbe,
// 10 / 11 -> 0x58c, any other -> 0x482)
// SPEC: UDP_SESSION_2009_A.md §6.2 (table)
pub fn disconnect_text(reason: u32) -> i32 {
    match reason {
        1 => 0x3ef,
        2 => 0x3f2,
        3 => 0x178c,
        6 => 0x5bb,
        7 => 0x432,
        // the 2009 text is formatted with two values of a shell global; DeCombat shows it unformatted
        8 => 0x462,
        9 => 0xbbe,
        10 | 11 => 0x58c,
        _ => 0x482,
    }
}

/// The box after a match: `None` for a played match, else the string id.
// SPEC: UDP_SESSION_2009_A.md §6.3 (any non-zero start result: the time-out box 0xbd0, cs+0x1bb910 PROVEN)
// DC-DEFAULT: CLI-509 (proposed) a lost link shows the "any other" box 0x482; a server bye shows the box of the reason
// equal to its bye code (whether the bye code reaches the 2009 box unchanged is UNPROVEN, spec §6.2 V-A6)
pub fn end_box(end: SessionEnd) -> Option<i32> {
    match end {
        SessionEnd::Played => None,
        SessionEnd::JoinFailed => Some(crate::menu::popup::text_ids::TIMEOUT),
        SessionEnd::Dropped => Some(disconnect_text(0)),
        SessionEnd::ServerBye(code) => Some(disconnect_text(code)),
    }
}

/// String ids of the leave-the-match box: notice title, the question, Yes, No.
// EVIDENCE: cshell+0x48e80 (box built at cshell+0x48fe5..0x4900a: title 0x371, text 0xbbf, buttons 0x35 / 0x36,
// Yes callback cshell+0x48450 sends 0xbd)
// SPEC: GAME_MESSAGES_2009_C2S.md §5.7
pub const LEAVE_TITLE: i32 = 0x371;
pub const LEAVE_TEXT: i32 = 0xbbf;
pub const LEAVE_YES: i32 = 0x35;
pub const LEAVE_NO: i32 = 0x36;

/// What a key or click does to the in-match leave box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveStep {
    /// Nothing changes.
    None,
    /// The exit box opened.
    Opened,
    /// The box closed without leaving (back to the game).
    Cancelled,
    /// The exit box's lobby button: the Yes / No question replaced it.
    Asked,
    /// The exit box's report button: the window opens the report form (CLI-950, [`crate::menu::reportdlg`]).
    Report,
    /// The hovered or pressed button changed (redraw only).
    Look,
    /// Yes: leave the match.
    Leave,
}

/// Keys and clicks the leave box understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveKey {
    /// The Escape key.
    Escape,
    /// Enter / Y: Yes of the question.
    Confirm,
    /// N: No of the question.
    Deny,
    /// A click on button `n` of the page shown (exit box: lobby, report, back to the game; question: Yes, No).
    Click(usize),
}

/// The page the leave box shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LeavePage {
    /// The 2009 exit box: return to the lobby, report, back to the game.
    #[default]
    Exit,
    /// The Yes / No question of the lobby button.
    Ask,
}

/// Buttons of the exit box in the 2009 build order (command ids 0x30d41, 0x30d42, 0x30d40).
// EVIDENCE: cshell+0x274dd0 (lobby, report, back-to-game buttons created in this order)
pub const EXIT_LOBBY: usize = 0;
pub const EXIT_REPORT: usize = 1;
pub const EXIT_CANCEL: usize = 2;

/// What the HUD draws of the box: page, hovered button, pressed button.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LeaveView {
    pub page: LeavePage,
    pub hover: Option<usize>,
    pub down: Option<usize>,
}

/// The in-match leave box. One Escape opens the exit box (the 2009 key handler opens it on the key itself, no
/// cursor step before it); its lobby button asks the Yes / No question (Yes leaves), its back-to-game button or
/// Escape closes it.
// EVIDENCE: cshell+0x49670 case 0x1b (DISASM cshell+0x49910..0x4994c: the exit box at interface +0x4b8 is shown
// through cshell+0x275290(1), which also refreshes the cursor (cshell+0x1b6650: cursor shown while a box is up),
// and its answer callback is set to cshell+0x48e80), cshell+0x48e80 (answer 1 = the lobby button: the Yes / No
// box 0x371 / 0xbbf / 0x35 / 0x36; answer 2 = the report form)
// DC-DEFAULT: CLI-920 (proposed) Escape closes the open box (either page); Enter / Y = Yes and N = No on the
// question only; the report button opens the in-match report form (CLI-950)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LeaveBox {
    pub open: bool,
    pub view: LeaveView,
}

impl LeaveBox {
    pub fn key(&mut self, k: LeaveKey) -> LeaveStep {
        if !self.open {
            if k == LeaveKey::Escape {
                *self = LeaveBox {
                    open: true,
                    view: LeaveView::default(),
                };
                return LeaveStep::Opened;
            }
            return LeaveStep::None;
        }
        let close = |b: &mut LeaveBox| {
            *b = LeaveBox::default();
            LeaveStep::Cancelled
        };
        match (self.view.page, k) {
            (_, LeaveKey::Escape) => close(self),
            (LeavePage::Exit, LeaveKey::Click(EXIT_LOBBY)) => {
                self.view = LeaveView {
                    page: LeavePage::Ask,
                    hover: None,
                    down: None,
                };
                LeaveStep::Asked
            }
            (LeavePage::Exit, LeaveKey::Click(EXIT_REPORT)) => {
                self.view.down = None;
                LeaveStep::Report
            }
            (LeavePage::Exit, LeaveKey::Click(EXIT_CANCEL)) => close(self),
            (LeavePage::Exit, _) => LeaveStep::None,
            (LeavePage::Ask, LeaveKey::Confirm | LeaveKey::Click(0)) => {
                *self = LeaveBox::default();
                LeaveStep::Leave
            }
            (LeavePage::Ask, LeaveKey::Deny | LeaveKey::Click(1)) => close(self),
            (LeavePage::Ask, _) => LeaveStep::None,
        }
    }

    /// The pointer is over button `hover` (or none) and the left button is `held`: `Look` when the drawn state
    /// changed.
    pub fn pointer(&mut self, hover: Option<usize>, held: bool) -> LeaveStep {
        if !self.open {
            return LeaveStep::None;
        }
        let down = if held { hover } else { None };
        if self.view.hover == hover && self.view.down == down {
            return LeaveStep::None;
        }
        self.view.hover = hover;
        self.view.down = down;
        LeaveStep::Look
    }

    /// What the HUD draws (`None` while closed).
    pub fn shown(&self) -> Option<LeaveView> {
        self.open.then_some(self.view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_grows_with_the_clock_and_the_stage_and_stops_at_its_width() {
        // one update when the page opens: 22 + 400 / 8
        assert_eq!(bar_fill(400.0, stage::JOIN, 0.0), 22.0 + 50.0);
        // the counter follows the clock, not the frames: same value at any frame split
        assert_eq!(bar_fill(400.0, stage::JOIN, 0.5), 5.0 * 22.0 + 50.0);
        // a later stage gives a larger base (width / (9 - stage))
        assert_eq!(bar_fill(400.0, stage::AMMO, 0.0), 22.0 + 200.0);
        assert!(
            bar_fill(400.0, stage::WEAPON_GIVE, 0.0) > bar_fill(400.0, stage::STATUS_RESET, 0.0)
        );
        assert_eq!(bar_fill(400.0, stage::IN_WORLD, 0.0), 400.0);
        assert_eq!(bar_fill(400.0, stage::JOIN, 60.0), 400.0);
    }

    #[test]
    fn bar_pieces_split_the_caps_and_the_middle_at_the_fill() {
        let p = bar_pieces([100.0, 10.0], 0.0);
        // empty: three pieces of the empty row covering the bar
        assert_eq!(p.len(), 3);
        assert!(p.iter().all(|x| x.row == 1 && x.share == 1.0));
        assert_eq!(p.iter().map(|x| x.dst[2]).sum::<f32>(), 100.0);
        let p = bar_pieces([100.0, 10.0], 50.0);
        let filled: f32 = p.iter().filter(|x| x.row == 0).map(|x| x.dst[2]).sum();
        let empty: f32 = p.iter().filter(|x| x.row == 1).map(|x| x.dst[2]).sum();
        assert_eq!((filled, empty), (50.0, 50.0));
        // the middle is cut: the filled part shows the left share of its slice
        let mid = p.iter().find(|x| x.row == 0 && x.col == 1).unwrap();
        assert_eq!(mid.dst, [5.0, 0.0, 45.0, 10.0]);
        assert!((mid.share - 0.5).abs() < 1e-6);
        let full = bar_pieces([100.0, 10.0], 100.0);
        assert!(full.iter().all(|x| x.row == 0));
        assert_eq!(full[0].dst[2], 5.0, "cap = half the height");
    }

    #[test]
    fn tile_files_follow_the_grid_form() {
        assert_eq!(tile_file("a", 2, 4, 1, 3), "a13.dtx");
        assert_eq!(tile_file("a", 1, 4, 0, 2), "a2.dtx");
        assert_eq!(tile_file("a", 3, 1, 2, 0), "a2.dtx");
        assert_eq!(tile_file("a", 1, 1, 0, 0), "a.dtx");
    }

    #[test]
    fn the_in_world_stage_is_kept_once_reached() {
        assert_eq!(
            next_stage(stage::JOIN, stage::WEAPON_GIVE),
            stage::WEAPON_GIVE
        );
        assert_eq!(
            next_stage(stage::AMMO, stage::STATUS_RESET),
            stage::STATUS_RESET
        );
        // a respawn's weapon give or a later ammo status after the in-world mark
        assert_eq!(
            next_stage(stage::IN_WORLD, stage::WEAPON_GIVE),
            stage::IN_WORLD
        );
        assert_eq!(next_stage(stage::IN_WORLD, stage::AMMO), stage::IN_WORLD);
        assert_eq!(
            next_stage(stage::IN_WORLD, stage::STATUS_RESET),
            stage::IN_WORLD
        );
    }

    #[test]
    fn end_boxes_by_reason() {
        assert_eq!(end_box(SessionEnd::Played), None);
        assert_eq!(end_box(SessionEnd::JoinFailed), Some(0xbd0));
        assert_eq!(end_box(SessionEnd::Dropped), Some(0x482));
        assert_eq!(end_box(SessionEnd::ServerBye(6)), Some(0x5bb));
        assert_eq!(end_box(SessionEnd::ServerBye(3)), Some(0x178c));
        assert_eq!(end_box(SessionEnd::ServerBye(42)), Some(0x482));
    }

    #[test]
    fn leave_box_opens_on_one_escape_and_asks_before_leaving() {
        let mut b = LeaveBox::default();
        assert_eq!(
            b.key(LeaveKey::Confirm),
            LeaveStep::None,
            "closed: Enter does nothing"
        );
        // one Escape opens the exit box, the next closes it
        assert_eq!(b.key(LeaveKey::Escape), LeaveStep::Opened);
        assert_eq!(b.shown().map(|v| v.page), Some(LeavePage::Exit));
        assert_eq!(b.key(LeaveKey::Escape), LeaveStep::Cancelled);
        assert!(!b.open);
        // the back-to-game button closes it
        b.key(LeaveKey::Escape);
        assert_eq!(b.key(LeaveKey::Click(EXIT_CANCEL)), LeaveStep::Cancelled);
        // Enter on the exit box does not leave
        b.key(LeaveKey::Escape);
        assert_eq!(b.key(LeaveKey::Confirm), LeaveStep::None);
        // lobby -> the question; No closes, Yes leaves
        assert_eq!(b.key(LeaveKey::Click(EXIT_LOBBY)), LeaveStep::Asked);
        assert_eq!(b.shown().map(|v| v.page), Some(LeavePage::Ask));
        assert_eq!(b.key(LeaveKey::Deny), LeaveStep::Cancelled);
        b.key(LeaveKey::Escape);
        b.key(LeaveKey::Click(EXIT_LOBBY));
        assert_eq!(b.key(LeaveKey::Click(0)), LeaveStep::Leave);
        assert!(!b.open);
        b.key(LeaveKey::Escape);
        b.key(LeaveKey::Click(EXIT_LOBBY));
        assert_eq!(b.key(LeaveKey::Confirm), LeaveStep::Leave);
    }

    #[test]
    fn the_pointer_sets_hover_and_pressed() {
        let mut b = LeaveBox::default();
        assert_eq!(b.pointer(Some(0), false), LeaveStep::None, "closed");
        b.key(LeaveKey::Escape);
        assert_eq!(b.pointer(Some(1), false), LeaveStep::Look);
        assert_eq!(b.pointer(Some(1), false), LeaveStep::None);
        assert_eq!(b.view.hover, Some(1));
        assert_eq!(b.pointer(Some(1), true), LeaveStep::Look);
        assert_eq!(b.view.down, Some(1));
        assert_eq!(b.pointer(None, true), LeaveStep::Look);
        assert_eq!((b.view.hover, b.view.down), (None, None));
    }
}
