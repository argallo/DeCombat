//! CLI-1908: the Mods tab of the options over a match (CLI-1907): the loaded mods of the match that declare settings,
//! each with a Settings button that opens the same settings page the Mods screen shows ([`ModPageView`], MOD-24). A
//! change goes into the settings at once; the match loads its mod set again with the new values (the HUD keys, the
//! hooks), and the menu saves them to the profile when it takes the match's settings back.
//!
//! The tab sits right of the four 2009 tabs, drawn with their tab picture in nine parts (it is narrower than they
//! are); its page is the 2009 tab page's window with our own lines. The lobby's options keep their four tabs: this
//! tab exists only over a match.
//!
//! Provenance: ORIGINAL (DeCombat's own tab; the 2009 options have no mods).
// DC-DEFAULT: CLI-1908 (proposed, owner request 2026-11-05 "a tab in the settings popup to change things like the
// custom crosshair while in-game"), see DC_DEFAULTS.md

use std::path::PathBuf;

use dc_mod_lua::{ModHost, ModPolicy, ModSetting};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::WidgetKind;

use super::catalog::Catalog;
use super::modbrowser::{fit, text, wrap, Button, ButtonKind, Buttons, ModPageView, ORANGE};
use super::modsettings::SettingsPage;
use super::sheet::{place_text, Caption, Look, Sheet, ALIGN_CENTRE, BUTTON_LETTER, CAPTION_COLOUR};
use super::usebox::{fill, inside, MESSAGE_COLOUR, TEXT_COLOUR, TITLE_COLOUR};
use crate::hud::items::DrawItem;

/// The tab's index among the options' tabs (after the four 2009 ones) and its command.
pub const TAB_MODS: usize = 4;
pub const CMD_TAB_MODS: i32 = 1004;
/// Mod `i`'s Settings button.
pub const CMD_SETTINGS_BASE: i32 = 120_000;

pub const T_TAB: &str = "Mods";
pub const T_TITLE: &str = "Mod Settings";
pub const T_LIVE: &str = "Changes apply at once.";
pub const T_VANILLA: &str = "Vanilla server: no mods run here.";
pub const T_NO_LOCAL: &str = "This server does not allow your own mods.";
pub const T_NONE: &str = "No mod of this match has settings.";
pub const T_SETTINGS: &str = "Settings";
/// CLI-1907: the Graphic tab's note over a match (what does not change in the running match).
pub const T_GRAPHIC_NOTE: &str = "Resolution applies after the match, Renderer at the next start.";

/// The tab's width (the 2009 tabs are 94 px; the frame leaves this much right of the fourth).
const TAB_W: i32 = 48;
/// The page's lines and rows.
const PAD: i32 = 14;
const ROWS_DY: i32 = 64;
const ROW_H: i32 = 34;
const ROW_STEP: i32 = 40;
const BUTTON: [i32; 2] = [100, 25];
const ROW_BACK: [u8; 4] = [0, 0, 0, 150];

/// One loaded mod of the match with declared settings.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchMod {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    pub settings: Vec<ModSetting>,
}

/// What the tab lists: the match's policy (`None`: a vanilla match, no mod set) and its loaded mods with settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MatchModsInfo {
    pub policy: Option<ModPolicy>,
    pub mods: Vec<MatchMod>,
    /// CLI-1916: the field-of-view range the match's server published (`None`: none, the Field of View row is locked).
    pub fov_range: Option<(u32, u32)>,
}

impl MatchModsInfo {
    /// The mods of the match's `host` (load order) that declare settings, under `policy`.
    pub fn of_host(host: Option<&ModHost>, policy: Option<ModPolicy>) -> MatchModsInfo {
        let mods = host
            .map(|h| h.loaded_mods())
            .unwrap_or_default()
            .into_iter()
            .filter(|(m, _)| !m.settings.is_empty())
            .filter_map(|(m, dir)| {
                Some(MatchMod {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    dir: dir?,
                    settings: m.settings.clone(),
                })
            })
            .collect();
        MatchModsInfo {
            policy,
            mods,
            fov_range: None,
        }
    }

    /// This info with the match's field-of-view range (CLI-1916).
    pub fn with_fov_range(mut self, range: Option<(u32, u32)>) -> MatchModsInfo {
        self.fov_range = range;
        self
    }

    /// The tab's status line and its colour: a vanilla match, a server that refuses the player's own mods, else the
    /// note that changes apply at once.
    pub fn status(&self) -> (&'static str, u32) {
        match self.policy {
            None | Some(ModPolicy::Vanilla) => (T_VANILLA, MESSAGE_COLOUR),
            Some(ModPolicy::Modded {
                client_visual_allowed: false,
            })
            | Some(ModPolicy::Curated {
                client_visual_allowed: false,
            }) => (T_NO_LOCAL, ORANGE),
            _ => (T_LIVE, MESSAGE_COLOUR),
        }
    }
}

/// The tab's state inside the options: what it lists, its buttons, the open settings page.
pub struct ModsTab {
    pub info: MatchModsInfo,
    pub buttons: Buttons,
    /// The open settings page (it covers the options while open).
    pub view: Option<ModPageView>,
    /// The tab button and the page area (the 2009 tab page's window).
    pub tab: [i32; 4],
    pub area: [i32; 4],
    tab_hover: bool,
    tab_down: bool,
    /// The 2009 tabs' picture prefix and caption look.
    tab_prefix: Option<String>,
    tab_caption: Option<Caption>,
    tab_art: [i32; 2],
}

/// What a pointer release on the tab's controls asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabClick {
    /// The tab itself.
    Tab,
    /// Mod `i`'s Settings button.
    Settings(usize),
}

impl ModsTab {
    pub fn new(info: MatchModsInfo) -> ModsTab {
        ModsTab {
            info,
            buttons: Buttons::default(),
            view: None,
            tab: [676, 120, TAB_W, 28],
            area: [300, 148, 425, 498],
            tab_hover: false,
            tab_down: false,
            tab_prefix: None,
            tab_caption: None,
            tab_art: [94, 28],
        }
    }

    /// Takes the tab's place from the options' sheet (right of the fourth 2009 tab, its height) and the page area
    /// (the tab page's window); `open`: whether the Mods tab is the open one (its Settings buttons exist then).
    pub fn place(&mut self, sheet: &Sheet, open: bool) {
        let button = |cmd: i32| {
            sheet
                .parts
                .iter()
                .find(|p| p.kind == WidgetKind::Button && p.command == cmd)
        };
        if let Some(last) = button(super::options::CMD_TABS[3]) {
            self.tab = [last.pos[0] + last.size[0], last.pos[1], TAB_W, last.size[1]];
        }
        if let Some(first) = button(super::options::CMD_TABS[0]) {
            if let Look::Button {
                prefix, caption, ..
            } = &first.look
            {
                self.tab_prefix = (!prefix.is_empty()).then(|| prefix.clone());
                self.tab_caption = caption.clone();
            }
            self.tab_art = first.size;
        }
        if let Some(w) = sheet
            .parts
            .iter()
            .find(|p| p.kind == WidgetKind::Window && p.layer == super::options::PAGE_WINDOW)
        {
            self.area = [w.pos[0], w.pos[1], w.size[0], w.size[1]];
        }
        self.rebuild(open);
    }

    /// The Settings buttons of the open tab (none while another tab is open).
    pub fn rebuild(&mut self, open: bool) {
        let [ax, ay, aw, _] = self.area;
        self.buttons.list = if open {
            (0..self.shown_rows())
                .map(|i| Button {
                    cmd: CMD_SETTINGS_BASE + i as i32,
                    at: [
                        ax + aw - 2 * 10 - BUTTON[0],
                        ay + ROWS_DY + i as i32 * ROW_STEP + (ROW_H - BUTTON[1]) / 2,
                    ],
                    size: BUTTON,
                    label: T_SETTINGS.into(),
                    on: true,
                    kind: ButtonKind::Art,
                })
                .collect()
        } else {
            Vec::new()
        };
    }

    /// The rows that fit in the page area.
    fn shown_rows(&self) -> usize {
        let room = ((self.area[3] - ROWS_DY) / ROW_STEP).max(0) as usize;
        self.info.mods.len().min(room)
    }

    /// The pointer moved: the hover looks; `true` when the hovered control changed to another one (a sound).
    pub fn moved(&mut self, p: [f32; 2]) -> bool {
        let over = inside([self.tab[0], self.tab[1]], [self.tab[2], self.tab[3]], p);
        let changed = over && !self.tab_hover;
        self.tab_hover = over;
        self.buttons.moved(p) || changed
    }

    /// A press: `true` when it is on one of the tab's controls.
    pub fn press(&mut self, p: [f32; 2]) -> bool {
        self.tab_down = inside([self.tab[0], self.tab[1]], [self.tab[2], self.tab[3]], p);
        self.buttons.press(p) || self.tab_down
    }

    /// A release: the control clicked (the release over the pressed one).
    pub fn release(&mut self, p: [f32; 2]) -> Option<TabClick> {
        let tab = std::mem::take(&mut self.tab_down)
            && inside([self.tab[0], self.tab[1]], [self.tab[2], self.tab[3]], p);
        if tab {
            return Some(TabClick::Tab);
        }
        self.buttons
            .release(p)
            .map(|c| TabClick::Settings((c - CMD_SETTINGS_BASE) as usize))
    }

    /// Opens mod `i`'s settings page with the player's `stored` values.
    pub fn open_page(
        &mut self,
        cat: &Catalog,
        i: usize,
        stored: &crate::settings::Settings,
    ) -> bool {
        let Some(m) = self.info.mods.get(i) else {
            return false;
        };
        let page = SettingsPage::of(
            &m.id,
            &m.name,
            &m.dir,
            &m.settings,
            stored.mods_settings.get(&m.id),
        );
        self.view = Some(ModPageView::new(
            cat,
            page,
            super::modsettings::T_APPLIES_NOW,
        ));
        true
    }

    /// The tab button: the 2009 tab picture in nine parts (state as the 2009 tabs: the open one drawn disabled), our
    /// caption with the 2009 tabs' caption look.
    // DC-DEFAULT: CLI-1908 (proposed) as CLI-556: the open tab shows its fourth picture
    pub fn draw_tab(&self, cat: &mut Catalog, open: bool, out: &mut Vec<DrawItem>) {
        let state = if open {
            3
        } else if self.tab_hover && self.tab_down {
            2
        } else if self.tab_hover {
            1
        } else {
            0
        };
        let tex = self.tab_prefix.as_ref().map(|p| {
            let t = format!("{p}{}.dtx", BUTTON_LETTER[state]);
            if cat.tex_size(&t).is_some() {
                t
            } else {
                format!("{p}A.dtx")
            }
        });
        let drawn = match tex.filter(|t| cat.tex_size(t).is_some()) {
            Some(t) => {
                let art = (self.tab_art[0].max(4) as u32, self.tab_art[1].max(4) as u32);
                super::nine::draw_art(&t, art, self.tab, [255; 4], out)
            }
            None => 0,
        };
        if drawn == 0 {
            let c = if open {
                [70, 60, 30, 230]
            } else {
                [40, 40, 40, 230]
            };
            out.push(fill(
                [self.tab[0], self.tab[1]],
                [self.tab[2], self.tab[3]],
                c,
            ));
        }
        let [x, y, w, h] = self.tab;
        let (at, font, size, colour) = match &self.tab_caption {
            Some(c) => (
                [x + w / 2, y + c.at[1]],
                c.font,
                c.size,
                c.colours[CAPTION_COLOUR[state]],
            ),
            None => ([x + w / 2, y + h / 2], 14, 14, TEXT_COLOUR),
        };
        let align = match &self.tab_caption {
            Some(c) => Align {
                first: ALIGN_CENTRE,
                second: c.align.second,
            },
            None => Align {
                first: ALIGN_CENTRE,
                second: ALIGN_SECOND_CENTRE,
            },
        };
        out.push(place_text(T_TAB, at, [0, 0], align, font, size, colour));
    }

    /// The open tab's page: title, status line, one row per mod with its Settings button.
    pub fn draw_body(&self, cat: &mut Catalog, out: &mut Vec<DrawItem>) {
        let [ax, ay, aw, _] = self.area;
        out.push(text(T_TITLE, [ax + PAD, ay + 12], TITLE_COLOUR));
        let (line, colour) = self.info.status();
        out.push(text(
            &fit(cat, line, aw - 2 * PAD),
            [ax + PAD, ay + 36],
            colour,
        ));
        let vanilla = matches!(self.info.policy, None | Some(ModPolicy::Vanilla));
        if self.info.mods.is_empty() && !vanilla {
            out.push(text(T_NONE, [ax + PAD, ay + ROWS_DY + 8], MESSAGE_COLOUR));
        }
        for (i, m) in self.info.mods.iter().take(self.shown_rows()).enumerate() {
            let y = ay + ROWS_DY + i as i32 * ROW_STEP;
            out.push(fill([ax + 10, y], [aw - 20, ROW_H], ROW_BACK));
            let room = aw - 2 * 10 - BUTTON[0] - 2 * PAD;
            out.push(text(
                &fit(cat, &m.name, room),
                [ax + 10 + PAD, y + (ROW_H - 14) / 2],
                TEXT_COLOUR,
            ));
        }
        self.buttons.draw(cat, out);
    }

    /// The Graphic tab's note over a match, under the tab page's rows.
    pub fn draw_graphic_note(&self, cat: &mut Catalog, out: &mut Vec<DrawItem>) {
        let [ax, ay, aw, ah] = self.area;
        let lines = wrap(cat, T_GRAPHIC_NOTE, aw - 2 * PAD);
        for (i, l) in lines.iter().take(2).enumerate() {
            out.push(text(
                l,
                [ax + PAD, ay + ah - 44 + i as i32 * 18],
                MESSAGE_COLOUR,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(policy: Option<ModPolicy>, n: usize) -> MatchModsInfo {
        MatchModsInfo {
            policy,
            mods: (0..n)
                .map(|i| MatchMod {
                    id: format!("m{i}"),
                    name: format!("Mod {i}"),
                    dir: PathBuf::from("x"),
                    settings: Vec::new(),
                })
                .collect(),
            fov_range: None,
        }
    }

    #[test]
    fn the_status_line_follows_the_policy() {
        assert_eq!(info(None, 0).status().0, T_VANILLA);
        assert_eq!(info(Some(ModPolicy::Vanilla), 0).status().0, T_VANILLA);
        let refused = ModPolicy::Curated {
            client_visual_allowed: false,
        };
        assert_eq!(info(Some(refused), 0).status().0, T_NO_LOCAL);
        let allowed = ModPolicy::Modded {
            client_visual_allowed: true,
        };
        assert_eq!(info(Some(allowed), 1).status().0, T_LIVE);
    }

    #[test]
    fn the_settings_buttons_exist_only_on_the_open_tab_and_click_their_mod() {
        let allowed = ModPolicy::Modded {
            client_visual_allowed: true,
        };
        let mut t = ModsTab::new(info(Some(allowed), 2));
        t.rebuild(false);
        assert!(t.buttons.list.is_empty());
        t.rebuild(true);
        assert_eq!(t.buttons.list.len(), 2);
        let b = t.buttons.list[1].clone();
        let p = [b.at[0] as f32 + 2.0, b.at[1] as f32 + 2.0];
        assert!(t.press(p));
        assert_eq!(t.release(p), Some(TabClick::Settings(1)));
        let tab = [t.tab[0] as f32 + 2.0, t.tab[1] as f32 + 2.0];
        assert!(t.press(tab));
        assert_eq!(t.release(tab), Some(TabClick::Tab));
        // a release off the pressed control clicks nothing
        assert!(t.press(tab));
        assert_eq!(t.release([0.0, 0.0]), None);
    }
}
