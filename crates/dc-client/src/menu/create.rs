//! Room creation screen, a box over the lobby pages (the lobby's room and user rows are not drawn under it, CLI-172): title, password (behind its check box), player count (previous /
//! next by the mode's step between its limits), mode, OK / cancel. OK sends 0x382; the lobby's answer opens the
//! waiting room.
// SPEC: PROTOCOL_2009.md §3.5 (0x311: a mode is offered when a map's mask has its bit), §3.7 (0x382 / 0x383: code
// 0x8d -> res 0x1061; the password needs 4 characters)
// EVIDENCE: cshell+0x1391e0 (commands: 7 cancel -> back, 0x54 create -> cshell+0x1388c0, 0x5d password check shows
// the password box (1, 10), 0x86 next = players + step up to the mode's maximum, 0x87 previous = players - step
// down to its minimum, 10000 opens the mode list), cshell+0x1388c0 (title and password read from the boxes, then
// 0x382), cshell+0x138ad0 (0x383 answer -> screen 9)

use super::catalog::Catalog;
use super::dropdown::{self, Dropdown};
use super::link::{LinkCmd, LinkEvent, Wait};
use super::lobby::{fill_char_panel, lobby_chat, lobby_hides, PanelArt};
use super::popup::{text_ids, BoxTag};
use super::roomtext;
use super::sheet::Sheet;
use super::{build_sheet, pages, Ctx, Go, MenuData, MenuKey, Screen};
use dc_ui_data::WidgetKind;

pub const WINDOW: i32 = 1;
pub const TITLE_BOX: i32 = 9;
pub const PASSWORD_BOX: i32 = 10;
/// The password box's frame image.
pub const PASSWORD_FRAME: i32 = 2;
pub const PLAYER_COUNT_TEXT: i32 = 16;
pub const MODE_LABEL_TEXT: i32 = 17;
pub const PASSWORD_CHECK: i32 = 18;
pub const CMD_CANCEL: i32 = 7;
pub const CMD_OK: i32 = 84;
/// The default room titles: the box starts with string `TITLES[tick % 5]`, the tick being the system's millisecond
/// count when the box opens.
// EVIDENCE: cshell+0x139770 (the list built in this order: 0x414 .. 0x418), cshell+0x1399e0 (title box (1, 9) set to
// the string of list[GetTickCount() % count] on the path the reference stack and a fresh account take)
pub const TITLES: [i32; 5] = [0x414, 0x415, 0x416, 0x417, 0x418];

/// The default title string id for the millisecond count `tick`.
pub fn default_title(tick: u64) -> i32 {
    TITLES[(tick % TITLES.len() as u64) as usize]
}

/// A millisecond count for the title pick (the 2009 tick count is the time since boot; any running count serves).
// DC-DEFAULT: CLI-752 (proposed) the pick's count is the wall clock's milliseconds
fn tick_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
pub const CMD_LOCK_CHECK: i32 = 93;
pub const CMD_MORE_PLAYERS: i32 = 134;
pub const CMD_FEWER_PLAYERS: i32 = 135;
pub const CMD_MODE_LIST: i32 = 10000;
/// The control the mode list opens under.
// EVIDENCE: cshell+0x139590 (the list is placed under control (1, 3))
pub const MODE_LIST_ANCHOR: i32 = 3;
/// The shortest room password the screen sends.
pub const PASSWORD_MIN: usize = 4;
/// Answer code of 0x383 "no more rooms".
pub const CODE_NO_MORE_ROOMS: u16 = 0x8d;

/// The modes offered: those with a bit in any map's mask of 0x311 (every mode row without 0x311).
pub fn offered_modes(masks: &[(u32, u32)], mode_rows: usize) -> Vec<i32> {
    let all: u32 = masks.iter().fold(0, |a, (_, m)| a | m);
    let n = mode_rows.clamp(1, 32);
    (0..n as i32)
        .filter(|i| masks.is_empty() || all & (1u32 << i) != 0)
        .collect()
}

/// The mode row the box starts on when it is offered (else the first of the list).
// EVIDENCE: cshell+0x1399e0 (box open: the list index of mode row 8 through cshell+0x139020, 0 when it is not in the
// list, handed to the mode pick cshell+0x138e90)
pub const START_MODE_ROW: i32 = 8;

/// The offered mode rows in the box's list order: by each row's sorting order (`order`), rows without one at their
/// row number, equal orders by row.
// EVIDENCE: cshell+0x266940 (the list is a map keyed by the rows' sorting order, read in key order)
// DC-DEFAULT: CLI-541 (proposed) rows with an equal order are all kept (by row number) where the 2009 map keeps the
// last; a row without the key sorts at its row number
pub fn sorted_modes(modes: &[i32], order: impl Fn(i32) -> Option<i32>) -> Vec<i32> {
    let mut v: Vec<(i32, i32)> = modes.iter().map(|&m| (order(m).unwrap_or(m), m)).collect();
    v.sort();
    v.into_iter().map(|(_, m)| m).collect()
}

/// The list index the box starts on: [`START_MODE_ROW`]'s place, else 0.
// EVIDENCE: cshell+0x139020 (search of the list for the row, 0 when absent)
pub fn start_index(modes: &[i32]) -> usize {
    modes.iter().position(|&m| m == START_MODE_ROW).unwrap_or(0)
}

/// The player count a mode pick sets: the row's recommended count, else its maximum.
// EVIDENCE: cshell+0x138e90 (mode pick: players = cshell+0x2651b0 of the mode without a map = the row's
// recommended player count)
// DC-DEFAULT: CLI-541 (proposed) a missing or non-positive recommendation falls back to the row's maximum
pub fn pick_players(recommended: Option<i32>, max: i32) -> i32 {
    recommended.filter(|n| *n > 0).unwrap_or(max)
}

/// LG-305: the Gun Game row of the mode lists, not a ROM row: the room is created on the free-for-all row and asks for
/// Gun Game with our request.
pub const GUN_GAME_ROW: i32 = -100;

/// The free-for-all mode row a Gun Game room runs on.
pub fn free_for_all_row(cat: &Catalog, data: &MenuData) -> Option<i32> {
    offered_modes(&data.masks, cat.mode_count())
        .into_iter()
        .find(|m| cat.mode_class(*m) == Some(6))
}

/// A mode list row's text (the Gun Game row's is ours).
pub fn row_name(cat: &Catalog, m: i32) -> String {
    if m == GUN_GAME_ROW {
        dc_wire::gungame::MODE_NAME.to_string()
    } else {
        cat.mode_full_name(m)
    }
}

pub struct Create {
    sheet: Sheet,
    art: Option<PanelArt>,
    modes: Vec<i32>,
    /// LG-305: the free-for-all row under the Gun Game row (`None`: the row is not listed).
    gun_base: Option<i32>,
    mode_at: usize,
    players: i32,
    busy: Wait,
    /// The open mode list.
    drop: Option<Dropdown>,
}

impl Create {
    pub fn new(cat: &Catalog, data: &MenuData) -> Create {
        let mut sheet = build_sheet(
            cat,
            &[
                pages::LOBBY_COMMON,
                pages::CHAT,
                pages::ROOM_LIST,
                pages::ROOM_CREATE,
            ],
        );
        lobby_hides(&mut sheet);
        lobby_chat(&mut sheet, cat);
        let mut modes = sorted_modes(&offered_modes(&data.masks, cat.mode_count()), |m| {
            cat.mode_sorting_order(m)
        });
        // DC-DEFAULT: CLI-1638 (proposed) on a featured entry the list ends with "Gun Game"
        let gun_base = data.gun_offered.then(|| free_for_all_row(cat, data)).flatten();
        if gun_base.is_some() {
            modes.push(GUN_GAME_ROW);
        }
        let mut c = Create {
            sheet,
            art: cat.rom.as_deref().and_then(PanelArt::cached),
            mode_at: start_index(&modes),
            gun_base,
            players: 0,
            modes,
            busy: Wait::default(),
            drop: None,
        };
        // CLI-479 retired by w2-gaterows: the 2009 start row and its recommended count (CLI-541)
        c.players = c.picked_players(cat);
        if let Some(p) = c.sheet.part_mut(WINDOW, TITLE_BOX, WidgetKind::EditBox) {
            // EVIDENCE: cshell+0x1399e0 (one of the default titles; CLI-752 replaces CLI-172's nickname)
            let t = cat.text(default_title(tick_ms()));
            p.text = if t.is_empty() {
                data.profile.nick.clone()
            } else {
                t
            };
        }
        // the password box and its frame stay hidden until the check box is ticked
        // DISASM: cshell+0x13922e (command 0x5d: check (1, 18) state -> frame image (1, 2) and box (1, 10) shown /
        // hidden together)
        // OBSERVED: lobbyfx4/real/R1_create.png (2009: no frame next to the unticked check box)
        if let Some(p) = c.sheet.part_mut(WINDOW, PASSWORD_BOX, WidgetKind::EditBox) {
            p.shown = false;
            p.masked = true;
        }
        c.sheet
            .show(WINDOW, PASSWORD_FRAME, WidgetKind::Image, false);
        c.fill_char(cat, data);
        c.show_values(cat);
        let title = c.sheet.find(WINDOW, TITLE_BOX, WidgetKind::EditBox);
        c.sheet.focus(title);
        c
    }

    /// The list's current row (the Gun Game row included).
    fn row(&self) -> i32 {
        self.modes.get(self.mode_at).copied().unwrap_or(0)
    }

    /// The current ROM mode row (Gun Game: the free-for-all row).
    fn mode(&self) -> i32 {
        match self.row() {
            GUN_GAME_ROW => self.gun_base.unwrap_or(0),
            m => m,
        }
    }

    /// The player count a pick of the current mode sets.
    fn picked_players(&self, cat: &Catalog) -> i32 {
        pick_players(
            cat.mode_recommended_players(self.mode()),
            self.limits(cat).1,
        )
    }

    /// The current mode row (tests).
    pub fn current_mode(&self) -> i32 {
        self.mode()
    }

    /// The current player count (tests).
    pub fn current_players(&self) -> i32 {
        self.players
    }

    /// Player limits of the current mode: (min, max, step).
    // DC-DEFAULT: CLI-172 (proposed) without the mode row: 2..16 players in steps of 2
    fn limits(&self, cat: &Catalog) -> (i32, i32, i32) {
        cat.mode_players(self.mode()).unwrap_or((2, 16, 2))
    }

    /// The lobby's Character Info panel under the box, with the room list's fill.
    // DC-DEFAULT: CLI-476 (proposed) the box is its own screen and fills the panel itself
    fn fill_char(&mut self, cat: &Catalog, data: &MenuData) {
        fill_char_panel(
            &mut self.sheet,
            cat,
            data,
            self.art.as_ref(),
            &data.char_extras,
            data.balance,
        );
    }

    /// Player count and mode name.
    // EVIDENCE: cshell+0x1391e0 / cshell+0x138e90 ((1, 16) players through cshell+0x262510, (1, 17) the mode's
    // full name through cshell+0x2666d0)
    fn show_values(&mut self, cat: &Catalog) {
        let players = roomtext::player_count_label(cat, self.mode(), self.players);
        self.sheet.set_text(WINDOW, PLAYER_COUNT_TEXT, &players);
        let mode = row_name(cat, self.row());
        self.sheet.set_text(WINDOW, MODE_LABEL_TEXT, &mode);
    }

    fn text(&self, id: i32) -> String {
        self.sheet
            .part(WINDOW, id, WidgetKind::EditBox)
            .map(|p| p.text.clone())
            .unwrap_or_default()
    }

    fn create(&mut self, ctx: &mut Ctx) {
        if self.busy.busy() {
            return;
        }
        let locked = self
            .sheet
            .part(WINDOW, PASSWORD_CHECK, WidgetKind::CheckButton)
            .is_some_and(|p| p.checked);
        let password = if locked {
            self.text(PASSWORD_BOX)
        } else {
            String::new()
        };
        if locked && password.chars().count() < PASSWORD_MIN {
            return;
        }
        let title = self.text(TITLE_BOX).trim().to_string();
        let title = if title.is_empty() {
            ctx.data.profile.nick.clone()
        } else {
            title
        };
        ctx.data.mode = self.mode();
        ctx.send(LinkCmd::OpenRoom {
            max_players: self.players,
            title,
            password,
            mode: self.mode(),
        });
        // LG-305: the lobby takes the two requests in order, the room exists by the second
        if self.row() == GUN_GAME_ROW {
            ctx.send(LinkCmd::RoomGunGame(true));
        }
        self.busy.start();
    }
}

impl Screen for Create {
    fn name(&self) -> &'static str {
        "create"
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        match k {
            MenuKey::Enter => self.create(ctx),
            MenuKey::Escape => *ctx.go = Some(Go::Lobby),
            MenuKey::Tab | MenuKey::BackTab => {
                let title = self.sheet.find(WINDOW, TITLE_BOX, WidgetKind::EditBox);
                let pw = self
                    .sheet
                    .find(WINDOW, PASSWORD_BOX, WidgetKind::EditBox)
                    .filter(|&i| self.sheet.parts[i].shown);
                let next = if self.sheet.focused() == title {
                    pw.or(title)
                } else {
                    title
                };
                self.sheet.focus(next);
            }
            _ => {}
        }
    }

    fn click(&mut self, command: i32, _part: usize, ctx: &mut Ctx) {
        match command {
            CMD_CANCEL => *ctx.go = Some(Go::Lobby),
            CMD_OK => self.create(ctx),
            CMD_MORE_PLAYERS | CMD_FEWER_PLAYERS => {
                let (min, max, step) = self.limits(ctx.cat);
                self.players = if command == CMD_MORE_PLAYERS {
                    (self.players + step).min(max)
                } else {
                    (self.players - step).max(min)
                };
                self.show_values(ctx.cat);
            }
            CMD_MODE_LIST => {
                // the mode list under control (1, 3): the command shows it, or hides it when shown
                // EVIDENCE: cshell+0x1391e0 (command 10000: the list's vtable +0xc8 shown state, then +0x9c),
                // cshell+0x139590 (rows: the modes, their full names, size 0xc)
                let was_open = self.drop.take().is_some();
                if !was_open {
                    if let Some(anchor) = self.sheet.anchor_rect(WINDOW, MODE_LIST_ANCHOR) {
                        let rows = self
                            .modes
                            .iter()
                            .map(|m| (row_name(ctx.cat, *m), *m))
                            .collect();
                        self.drop = Some(Dropdown::new(
                            "create_mode",
                            anchor,
                            rows,
                            dropdown::CREATE_ROW_SIZE,
                            false,
                        ));
                    }
                }
            }
            _ => {}
        }
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

    /// A mode row of the list: the box takes the mode and its player count.
    // EVIDENCE: cshell+0x138e90 (command 0xf0 with the row: mode of the row, players from the mode row, texts (1, 16)
    // and (1, 17) again)
    // CLI-479 retired by w2-gaterows: the row's recommended count (CLI-541)
    fn dropdown_pick(&mut self, _list: &'static str, value: i32, _index: usize, ctx: &mut Ctx) {
        if let Some(i) = self.modes.iter().position(|m| *m == value) {
            self.mode_at = i;
        }
        self.players = self.picked_players(ctx.cat);
        self.show_values(ctx.cat);
    }

    fn check(&mut self, command: i32, part: usize, on: bool, _ctx: &mut Ctx) {
        if command == CMD_LOCK_CHECK {
            let _ = part;
            // DISASM: cshell+0x13922e (the frame and the box follow the check together)
            self.sheet
                .show(WINDOW, PASSWORD_FRAME, WidgetKind::Image, on);
            if let Some(i) = self.sheet.find(WINDOW, PASSWORD_BOX, WidgetKind::EditBox) {
                self.sheet.parts[i].shown = on;
                if on {
                    self.sheet.focus(Some(i));
                } else {
                    self.sheet.parts[i].text.clear();
                }
            }
        }
    }

    fn event(&mut self, e: &LinkEvent, ctx: &mut Ctx) {
        match e {
            LinkEvent::RoomCreated { .. } => {
                self.busy.stop();
                *ctx.go = Some(Go::Room);
            }
            LinkEvent::CharInfo(_)
            | LinkEvent::Grade(_)
            | LinkEvent::Balance { .. }
            | LinkEvent::Profile(_) => self.fill_char(ctx.cat, ctx.data),
            LinkEvent::CreateRefused(code) => {
                self.busy.stop();
                let id = if *code == CODE_NO_MORE_ROOMS {
                    text_ids::NO_MORE_ROOMS
                } else {
                    text_ids::CANNOT_ENTER
                };
                ctx.error(id, BoxTag::None);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_starts_on_row_8_in_sorting_order_with_its_recommended_count() {
        // rows 0..4 with orders 30, -, 10, 20: row 2, row 3, row 1 (its row number), row 0
        let order = |m: i32| [Some(30), None, Some(10), Some(20)][m as usize];
        assert_eq!(sorted_modes(&[0, 1, 2, 3], order), vec![1, 2, 3, 0]);
        // equal orders keep both rows, by row number
        assert_eq!(sorted_modes(&[3, 2], |_| Some(5)), vec![2, 3]);
        assert_eq!(start_index(&[2, 8, 5]), 1);
        assert_eq!(start_index(&[2, 5]), 0);
        assert_eq!(pick_players(Some(12), 16), 12);
        assert_eq!(pick_players(Some(0), 16), 16);
        assert_eq!(pick_players(None, 16), 16);
    }

    #[test]
    fn modes_follow_the_map_masks() {
        assert_eq!(offered_modes(&[], 3), vec![0, 1, 2]);
        assert_eq!(offered_modes(&[(0, 0b101), (1, 0b001)], 4), vec![0, 2]);
    }
}
