//! The in-match player and bug report form (round 10, CLI-950): the exit box's middle button opens the same report
//! box the lobby bar opens, over the match. The box is the lobby's [`Report`] screen (its layout section, lists,
//! entry check and the 0x312 command), driven here without a front end: the form keeps its own catalogue, the
//! pointer and key glue of the menu's input path, and a detached link that only keeps the commands sent, which the
//! window hands to the match session's lobby connection.
// EVIDENCE: cshell+0x48e80 (the exit box's answer 2: the report box is opened through cshell+0x273b70(1), the
// user list is filled with the match's players except the own one and the empty slots, cshell+0x273700 per player)
// EVIDENCE: cshell+0x273b70 (opening within 0x2710 ms of the last send shows the notice 0x346 instead)
// DC-DEFAULT: CLI-950 (proposed) the form is the lobby report box unchanged (same lists, same refusals); the
// players offered are the match roster; a send goes over the lobby connection the session keeps during the match

use super::catalog::Catalog;
use super::frontfx::FrontLits;
use super::link::{Link, LinkCmd};
use super::popup::{BoxTag, Popup};
use super::report::{self, Report};
use super::{Ctx, Go, MenuData, MenuInput, MenuKey, Screen};
use crate::hud::DrawItem;
use dc_ui_data::WidgetKind;
use std::path::Path;
use std::time::Instant;

/// What an input did to the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormStep {
    /// Nothing changed that shows.
    None,
    /// The form changed (draw it again).
    Redraw,
    /// The form closed (cancel, a sent report, or the notice's OK).
    Closed,
}

pub struct ReportForm {
    cat: Catalog,
    lits: FrontLits,
    data: MenuData,
    link: Link,
    /// The report box; `None` while only the resend notice shows.
    screen: Option<Report>,
    popups: Vec<Popup>,
    pressed: Option<usize>,
    drop_pressed: Option<usize>,
    pointer: [f32; 2],
    started: Instant,
    sent_at: Option<Instant>,
}

impl ReportForm {
    /// The form over a `width` x `height` target. `rom`: the client folder (`None`: our own layout, `font` the
    /// text font); `players`: the roster names offered by the user list; `me`: the own name (left out);
    /// `previous_at`: when the last report went (a new one within [`report::RESEND_MS`] shows the notice only).
    pub fn open(
        rom: Option<&Path>,
        font: Option<(Vec<u8>, usize)>,
        size: (u32, u32),
        players: &[String],
        me: &str,
        previous_at: Option<Instant>,
    ) -> ReportForm {
        let cat = rom
            .and_then(|r| Catalog::from_rom(r, size.0, size.1).ok())
            .unwrap_or_else(|| Catalog::fallback(font, size.0, size.1));
        let lits = rom.map(FrontLits::from_rom).unwrap_or_default();
        let mut data = MenuData::default();
        data.profile.nick = me.to_string();
        data.users = players
            .iter()
            .filter(|n| n.as_str() != me && !n.is_empty())
            .map(|n| super::link::UserRow {
                name: n.clone(),
                ..Default::default()
            })
            .collect();
        data.last_report = previous_at;
        let mut f = ReportForm {
            screen: None,
            popups: Vec::new(),
            link: Link::detached(),
            lits,
            data,
            cat,
            pressed: None,
            drop_pressed: None,
            pointer: [512.0, 384.0],
            started: Instant::now(),
            sent_at: None,
        };
        // EVIDENCE: cshell+0x273b70 (`elapsed < 0x2711` -> the notice instead of the box)
        let recent = previous_at.is_some_and(|t| t.elapsed().as_millis() <= report::RESEND_MS);
        if recent {
            let text = report::resend_notice(&f.cat);
            f.popups.push(Popup::ok(text, BoxTag::None));
        } else {
            let r = Report::new(&f.cat, &f.data);
            f.screen = Some(r);
        }
        f
    }

    /// When a report was sent through this form.
    pub fn sent_at(&self) -> Option<Instant> {
        self.sent_at
    }

    /// The commands the form sent since the last call (reports).
    pub fn take_sent(&mut self) -> Vec<LinkCmd> {
        self.link.take_sent()
    }

    /// The report box's sheet (tests).
    pub fn sheet(&self) -> Option<&super::sheet::Sheet> {
        self.screen.as_ref().map(|s| s.sheet())
    }

    /// The report box itself (tests).
    pub fn report(&self) -> Option<&Report> {
        self.screen.as_ref()
    }

    /// The open list (tests).
    pub fn dropdown(&self) -> Option<&super::dropdown::Dropdown> {
        self.screen.as_ref().and_then(|s| s.dropdown())
    }

    /// The box with a notice or refusal over it is up.
    pub fn popup(&self) -> Option<&Popup> {
        self.popups.first()
    }

    fn secs(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// The form's display list in the virtual 1024 x 768 screen (the blind, the box, its open list, a notice).
    pub fn items(&mut self) -> Vec<DrawItem> {
        let secs = self.secs();
        let mut out = Vec::new();
        if let Some(s) = self.screen.as_ref() {
            s.sheet().draw(&mut self.cat, secs, &mut out);
            // CLI-1810: the name row's shade while the category needs no target
            s.draw_extra(&mut self.cat, &self.data, secs, &mut out);
            if let Some(d) = s.dropdown() {
                d.draw(&mut self.cat, &self.lits.drop_art, &mut out);
            }
        }
        if let Some(p) = self.popups.first() {
            p.draw(&mut self.cat, &mut out);
        }
        out
    }

    /// Runs `f` on the report screen with the form's context; the `Go` the screen asked for.
    fn run(&mut self, screen: &mut Report, f: impl FnOnce(&mut Report, &mut Ctx)) -> Option<Go> {
        let mut go = None;
        let mut quit = false;
        let mut sounds = Vec::new();
        {
            let mut ctx = Ctx {
                cat: &self.cat,
                data: &mut self.data,
                link: &self.link,
                popups: &mut self.popups,
                go: &mut go,
                quit: &mut quit,
                sounds: &mut sounds,
            };
            f(screen, &mut ctx);
        }
        go
    }

    /// One input in virtual 1024 x 768 coordinates (the menu's own input path for the report screen).
    pub fn input(&mut self, ev: MenuInput) -> FormStep {
        if let MenuInput::Move(p) | MenuInput::Press(p) | MenuInput::Release(p) = ev {
            self.pointer = p;
        }
        if let Some(top) = self.popups.first_mut() {
            if top.input(&ev).is_some() {
                self.popups.remove(0);
                // the notice alone: its OK closes the form
                if self.screen.is_none() {
                    return FormStep::Closed;
                }
            }
            return FormStep::Redraw;
        }
        let Some(mut screen) = self.screen.take() else {
            return FormStep::None;
        };
        let before = self.link.sent();
        let step = self.route(&mut screen, ev);
        if self.link.sent() != before {
            self.sent_at = self.data.last_report;
        }
        // CLI-1650: the sent notice stays up alone; its OK closes the form
        if step == FormStep::Closed && !self.popups.is_empty() {
            return FormStep::Redraw;
        }
        self.screen = Some(screen);
        step
    }

    fn route(&mut self, screen: &mut Report, ev: MenuInput) -> FormStep {
        let mut go = None;
        match ev {
            MenuInput::Move(p) => {
                if screen.dropdown_mut().is_some_and(|d| d.drag_to(p)) {
                    return FormStep::Redraw;
                }
                if let Some(d) = screen.dropdown_mut() {
                    d.hover = d.row_at(p);
                }
                screen.sheet_mut().hover(p);
            }
            MenuInput::Press(p) => {
                if let Some(d) = screen.dropdown_mut() {
                    if d.takes(p) {
                        if !d.press_bar(p) {
                            self.drop_pressed = d.row_at(p);
                        }
                        self.pressed = None;
                        return FormStep::Redraw;
                    }
                }
                let sheet = screen.sheet_mut();
                let hit = sheet.hit(p);
                self.pressed = hit;
                if let Some(i) = hit {
                    match sheet.parts[i].kind {
                        WidgetKind::Button => {
                            sheet.parts[i].down = true;
                            sheet.parts[i].hover = true;
                        }
                        WidgetKind::EditBox => sheet.focus(Some(i)),
                        _ => {}
                    }
                }
            }
            MenuInput::Release(p) => {
                let dragged = screen.dropdown().is_some_and(|d| d.dragging());
                if let Some(d) = screen.dropdown_mut() {
                    d.release();
                }
                if dragged {
                    return FormStep::Redraw;
                }
                if let Some(row) = self.drop_pressed.take() {
                    let picked = screen
                        .dropdown()
                        .filter(|d| d.row_at(p) == Some(row))
                        .map(|d| (d.list, d.values.get(row).copied().unwrap_or(0)));
                    if let Some((list, value)) = picked {
                        screen.close_dropdown();
                        self.run(screen, |s, ctx| s.dropdown_pick(list, value, row, ctx));
                    }
                    return FormStep::Redraw;
                }
                let sheet = screen.sheet_mut();
                for part in &mut sheet.parts {
                    part.down = false;
                }
                let hit = sheet.hit(p);
                let pressed = self.pressed.take();
                if let Some(i) = hit.filter(|h| Some(*h) == pressed) {
                    let part = &sheet.parts[i];
                    if part.kind == WidgetKind::Button && part.enabled && part.command != 0 {
                        let cmd = part.command;
                        go = self.run(screen, |s, ctx| s.click(cmd, i, ctx));
                    }
                }
            }
            MenuInput::Key(MenuKey::Backspace) => {
                screen.sheet_mut().backspace();
            }
            MenuInput::Key(k) => {
                go = self.run(screen, |s, ctx| s.key(k, ctx));
            }
            MenuInput::Char(c) => {
                screen.sheet_mut().type_char(c);
            }
            MenuInput::Wheel(_, notches) => {
                if let Some(d) = screen.dropdown_mut() {
                    d.wheel(notches);
                }
            }
            _ => return FormStep::None,
        }
        // Cancel and a sent report both go back: the form is done
        if matches!(go, Some(Go::Back)) {
            return FormStep::Closed;
        }
        FormStep::Redraw
    }
}

/// The text of the form's two entries as typed (tests).
pub fn entry_text(f: &ReportForm, id: i32) -> String {
    f.sheet()
        .and_then(|s| s.part(report::WINDOW, id, WidgetKind::EditBox))
        .map(|p| p.text.clone())
        .unwrap_or_default()
}
