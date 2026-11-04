//! Channel select screen: the server's name, one bar per channel with its load, Enter / double click / the enter
//! button enters the selected channel (0x372); the lobby screen opens on the lobby's answer. Refresh asks for the
//! channel list again (0x376).
// SPEC: PROTOCOL_2009.md §3.4 (0x376 / 0x377, 0x372 / 0x373)
// EVIDENCE: cshell+0xfa130 (build: the server select build, windows 1, 2, 3 hidden, then the channel page)
// EVIDENCE: cshell+0xfb760 (focus: name into control 3 of window 5, list control 4 of window 5 emptied, no row
// selected, not waiting, 0x376 sent)
// EVIDENCE: cshell+0xfb380 (commands 0x2710 enter the selected row, 0x2711 server screen, 0x2712 0x376, 0x55 box 0x1843)
// EVIDENCE: cshell+0xfa860 (channel rows: steps `> 30`, `> 80`, `> 95`, `> 100`), cshell+0xfab60 (enter checks:
// closed -> box 0x1012, full -> command 0x55; both title 0x371, OK), cshell+0xfacd0 (0x373 answer: refused -> box
// 0x1843, title 0x346; accepted -> the lobby screen)
// EVIDENCE: cshell+0xfa420, cshell+0xfa1c0, cshell+0xfa520, cshell+0xfb2e0 (press, release, move and double click
// do nothing while the enter request waits for its answer)

use super::catalog::Catalog;
use super::drawlist::DynNotes;
use super::link::{ChannelRow, LinkCmd, LinkEvent, Wait};
use super::popup::{text_ids, BoxTag, Popup};
use super::servers::{draw_bar, BarRow, BarStyle};
use super::sheet::{list_hit, list_row, Part, Sheet, Tiles};
use super::{build_sheet, pages, Ctx, Go, MenuData, MenuKey, Screen, DOUBLE_CLICK_MS};
use crate::hud::items::DrawItem;
use dc_ui_data::{UiScreen, WidgetKind};
use std::time::Instant;

pub const WINDOW: i32 = 5;
pub const SERVER_NAME: i32 = 3;
pub const LIST: i32 = 4;
pub const CMD_ENTER: i32 = 10000;
pub const CMD_SERVER_LIST: i32 = 10001;
pub const CMD_RELOAD_LIST: i32 = 10002;
pub const CMD_QUIT: i32 = 303;
/// Channel load thresholds (per cent up to which the step applies; above 100 the channel shows "checking").
// EVIDENCE: cshell+0xfa860
pub const CHANNEL_LOAD_STEPS: [u32; 4] = [30, 80, 95, 100];

/// The load step of a channel (`None`: closed, without capacity or full).
pub fn channel_step(c: &ChannelRow) -> Option<usize> {
    if c.capacity == 0 || c.open == 0 {
        return None;
    }
    let pct = (c.current as u64 * 100 / c.capacity as u64) as u32;
    CHANNEL_LOAD_STEPS.iter().position(|t| pct <= *t)
}

pub struct Channels {
    sheet: Sheet,
    style: BarStyle,
    selected: Option<usize>,
    hover: Option<usize>,
    last_click: Option<(usize, Instant)>,
    busy: Wait,
}

impl Channels {
    pub fn new(cat: &Catalog, data: &MenuData) -> Channels {
        let mut sheet = build_sheet(cat, &[pages::COMMON, pages::SERVERS, pages::CHANNELS]);
        if let Some(prefix) = &cat.paths.server_back {
            sheet.tiles = Tiles::new([0, 0], 3, 4, prefix);
        }
        for w in 1..=3 {
            sheet.show_window(w, false);
        }
        let name = data
            .server
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_default();
        sheet.set_text(WINDOW, SERVER_NAME, &name);
        Channels {
            sheet,
            style: BarStyle::load(cat, UiScreen::Screen12),
            selected: None,
            hover: None,
            last_click: None,
            busy: Wait::default(),
        }
    }

    fn list(&self) -> Option<&Part> {
        self.sheet.part(WINDOW, LIST, WidgetKind::List)
    }

    fn enter_channel(&mut self, ctx: &mut Ctx) {
        let Some(c) = self
            .selected
            .and_then(|i| ctx.data.channels.get(i))
            .cloned()
        else {
            return;
        };
        if self.busy.busy() {
            return;
        }
        if c.open == 0 {
            ctx.notice(text_ids::CHANNEL_SHUT);
            return;
        }
        // gm-client (CLI-1304): an operator is held back only by the operator slots
        if super::gm::channel_full_for(&c, ctx.data.gm.operator) {
            ctx.notice(text_ids::CHANNEL_FULL);
            return;
        }
        ctx.send(LinkCmd::EnterChannel {
            id: c.id,
            name: c.name.clone(),
        });
        self.busy.start();
    }
}

impl Screen for Channels {
    fn list_rows(&self, data: &MenuData) -> Vec<(i32, i32, usize, i32)> {
        vec![(WINDOW, LIST, data.channels.len(), self.style.size[1])]
    }
    fn name(&self) -> &'static str {
        "channels"
    }

    /// The channel list is asked for whenever the screen opens.
    // EVIDENCE: cshell+0xfb760 (focus: the list emptied, 0x376 sent)
    fn enter(&mut self, ctx: &mut Ctx) {
        if ctx.data.lobby_up {
            ctx.send(LinkCmd::Channels);
        }
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        let n = ctx.data.channels.len();
        match k {
            MenuKey::Enter => self.enter_channel(ctx),
            MenuKey::Escape => {
                let t = ctx.cat.text(text_ids::ASK_QUIT);
                ctx.popups.push(Popup::ask(t, BoxTag::Quit));
            }
            MenuKey::Down if n > 0 => {
                self.selected = Some(self.selected.map_or(0, |s| (s + 1).min(n - 1)))
            }
            MenuKey::Up if n > 0 => {
                self.selected = Some(self.selected.map_or(0, |s| s.saturating_sub(1)))
            }
            _ => {}
        }
    }

    fn click(&mut self, command: i32, _part: usize, ctx: &mut Ctx) {
        if self.busy.busy() {
            return;
        }
        match command {
            CMD_ENTER => self.enter_channel(ctx),
            // the server list button: back to the server screen (whose focus asks for the list again, 0x3b0)
            // DISASM: cshell+0xfb3f9 (command 0x2711 -> screen manager slot 0x54 with screen 0xb)
            CMD_SERVER_LIST => *ctx.go = Some(Go::Servers),
            CMD_RELOAD_LIST => ctx.send(LinkCmd::Channels),
            CMD_QUIT => self.key(MenuKey::Escape, ctx),
            _ => {}
        }
    }

    fn press(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        if self.busy.busy() {
            return false;
        }
        let Some(list) = self.list().cloned() else {
            return false;
        };
        let n = ctx.data.channels.len();
        let Some(i) = list_hit(&list, self.style.size[0], self.style.size[1], n, 0, p) else {
            return false;
        };
        let double = self
            .last_click
            .is_some_and(|(j, t)| j == i && t.elapsed().as_millis() < DOUBLE_CLICK_MS);
        self.last_click = if double {
            None
        } else {
            Some((i, Instant::now()))
        };
        self.selected = Some(i);
        if double {
            self.enter_channel(ctx);
        }
        true
    }

    fn hover(&mut self, p: [f32; 2], data: &MenuData) {
        self.hover = self.list().and_then(|l| {
            list_hit(
                l,
                self.style.size[0],
                self.style.size[1],
                data.channels.len(),
                0,
                p,
            )
        });
    }

    fn event(&mut self, e: &LinkEvent, ctx: &mut Ctx) {
        match e {
            LinkEvent::Channels(v) => {
                if self.selected.is_some_and(|s| s >= v.len()) {
                    self.selected = None;
                }
            }
            LinkEvent::ChannelEntered { .. } => {
                self.busy.stop();
                *ctx.go = Some(Go::Lobby);
            }
            LinkEvent::ChannelRefused => {
                self.busy.stop();
                ctx.error(text_ids::CHANNEL_FULL, BoxTag::None);
            }
            _ => {}
        }
    }

    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, _secs: f32, out: &mut Vec<DrawItem>) {
        self.draw_rows(cat, data, out, None);
    }

    fn draw_extra_noted(
        &self,
        cat: &mut Catalog,
        data: &MenuData,
        _secs: f32,
        out: &mut Vec<DrawItem>,
        notes: &mut DynNotes,
    ) {
        self.draw_rows(cat, data, out, Some(notes));
    }
}

impl Channels {
    fn draw_rows(
        &self,
        cat: &mut Catalog,
        data: &MenuData,
        out: &mut Vec<DrawItem>,
        mut notes: Option<&mut DynNotes>,
    ) {
        let Some(list) = self.list() else {
            return;
        };
        for (i, c) in data.channels.iter().enumerate() {
            // rows scrolled above the list or below its end are not drawn
            let Some(at) = list_row(list, self.style.size[1], i, 0) else {
                continue;
            };
            let step = channel_step(c);
            let row = BarRow {
                index: i,
                name: &c.name,
                step,
                refused: step.is_none(),
                // EVIDENCE: cshell+0xfa860 (closed or without capacity: the checking picture; over 100 %: none)
                closed: c.open == 0 || c.capacity == 0,
                full: step == Some(3),
                selected: self.selected == Some(i),
                hover: self.hover == Some(i),
                badge: None,
            };
            draw_bar(&self.style, cat, &row, at, out, notes.as_deref_mut());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_load_steps() {
        let c = |cur, cap, open| ChannelRow {
            id: 1,
            name: String::new(),
            open,
            capacity: cap,
            current: cur,
            op_capacity: 0,
            op_current: 0,
        };
        assert_eq!(channel_step(&c(0, 100, 1)), Some(0));
        assert_eq!(channel_step(&c(30, 100, 1)), Some(0));
        assert_eq!(channel_step(&c(81, 100, 1)), Some(2));
        assert_eq!(channel_step(&c(100, 100, 1)), Some(3));
        assert_eq!(channel_step(&c(101, 100, 1)), None);
        assert_eq!(channel_step(&c(1, 100, 0)), None);
    }
}
