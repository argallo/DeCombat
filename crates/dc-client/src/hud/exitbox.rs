//! The in-match exit box (round 10, CLI-920): the box one Escape opens over the match, with the return-to-lobby,
//! report and back-to-game buttons. Its place, size and button places come from the ROM layout section the 2009
//! box reads; its window and the button state images are the 2009 build's literals. None of these texts is in this
//! tree: each is read from the user's CShell.dll at the pinned rva and accepted only when its length and SHA-256
//! prefix match.
// EVIDENCE: cshell+0x274dd0 (the box's build: window image + WindowPos / WindowWidth / WindowHeight of its section,
// then ButtonWidth / ButtonHeight and three buttons with four state images each at GotoLobby / GotoReport / GotoGame
// positions relative to the window; DISASM cshell+0x274dff..0x27518c for the literal addresses)

use crate::hud::DrawItem;
use crate::loading::{LeavePage, LeaveView};
use dc_ui_data::names::{read_pinned, Pin, UiSection, CSHELL_REL_PATH};
use dc_ui_data::LayoutTable;
use std::path::Path;

const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
    Pin {
        what,
        rva,
        sha16,
        len,
    }
}

/// Keys of the box's section: window place, width, height, button width, height, the three button places.
// EVIDENCE: cshell+0x274e0f, 0x274e2c, 0x274e51, 0x274ef2, 0x274f09, 0x274f68, 0x27507a, 0x27518c (the pushes)
const KEYS: [Pin; 8] = [
    pin("exit box window place", 0x3f24fc, "7fd8459190304736", 9),
    pin("exit box width", 0x3f5678, "af29812ecc6aa531", 11),
    pin("exit box height", 0x3f5668, "6cffa9fd407edebe", 12),
    pin("exit box button width", 0x3f58e8, "24215a24e2454fdf", 11),
    pin("exit box button height", 0x3f58d8, "caf9885fbe134af7", 12),
    pin("exit box lobby place", 0x3f5818, "7cdad07367942600", 12),
    pin("exit box report place", 0x3f5784, "ea2240554e62476a", 13),
    pin("exit box game place", 0x3f56c8, "c5e34af410f466d8", 11),
];

/// The window image.
// EVIDENCE: cshell+0x274dff
const WINDOW: Pin = pin("exit box window", 0x3f5904, "93b31d4fae3d8037", 39);

/// The four state images of each button in the order the build hands them to the button's texture setter
/// (cshell+0x33d7f0 slots: normal, over, disabled, pressed). The report button gets its third image twice.
// EVIDENCE: cshell+0x274f20..0x274f63, 0x275034..0x275075, 0x275146..0x275187; cshell+0x33e850 (+0x140 passes
// the four to the setter +0x164 = cshell+0x33d7f0)
const BUTTONS: [[Pin; 4]; 3] = [
    [
        pin("exit box lobby normal", 0x3f58ac, "ada0ca5950f9b36a", 40),
        pin("exit box lobby over", 0x3f5880, "b4426c28e81f377b", 40),
        pin("exit box lobby disabled", 0x3f5854, "55f8d98737c19190", 40),
        pin("exit box lobby pressed", 0x3f5828, "e7db52908e33b62f", 40),
    ],
    [
        pin("exit box report normal", 0x3f57ec, "356fc585ca95def6", 41),
        pin("exit box report over", 0x3f57c0, "768a808cf1eac492", 41),
        pin("exit box report third", 0x3f5794, "2bac98a78015d7fd", 41),
        pin("exit box report third", 0x3f5794, "2bac98a78015d7fd", 41),
    ],
    [
        pin("exit box game normal", 0x3f5758, "cf3b07abe31ca524", 41),
        pin("exit box game over", 0x3f572c, "bf52f235636ca134", 41),
        pin("exit box game disabled", 0x3f5700, "38c7d64e3d15a986", 41),
        pin("exit box game pressed", 0x3f56d4, "8adcd9f0604b0c44", 41),
    ],
];

/// Texture slot of a drawn button state.
// EVIDENCE: cshell+0x188f0 (over -> slot 1, pressed -> slot 3, else slot 0)
fn slot(hover: bool, down: bool) -> usize {
    if down {
        3
    } else if hover {
        1
    } else {
        0
    }
}

/// The exit box: window rectangle and button rectangles in the virtual 1024 x 768 screen, and its images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitBox {
    pub window: [i32; 4],
    pub buttons: [[i32; 4]; 3],
    pub window_tex: Option<String>,
    pub button_tex: [[Option<String>; 4]; 3],
    /// The button captions drawn over plain fills when there are no images (no ROM).
    pub captions: [String; 3],
    /// The file of the interface "select" sound a button click plays (`None`: silent).
    // EVIDENCE: cshell+0x1aeb90 (a released control that takes the click plays interface sound 0x14; the exit box's
    // buttons are controls of its window), CLI-1012 (playtest report: "exiting a game does not make the button click noises")
    pub select_file: Option<String>,
}

impl ExitBox {
    /// Our own box without a ROM: centred, three buttons in a column (plain fills and captions).
    // DC-DEFAULT: CLI-921 (proposed) the no-ROM exit box (400 x 200 centred, buttons 200 x 30)
    pub fn fallback() -> ExitBox {
        let window = [312, 284, 400, 200];
        let b = |i: i32| [412, 284 + 40 + i * 45, 200, 30];
        ExitBox {
            window,
            buttons: [b(0), b(1), b(2)],
            window_tex: None,
            button_tex: Default::default(),
            captions: ["Lobby".into(), "Report".into(), "Back".into()],
            select_file: None,
        }
    }

    /// The box of the user's ROM (`table`: its layout table); `None` when a pin or a key does not resolve.
    pub fn from_rom(rom: &Path, table: &LayoutTable) -> Option<ExitBox> {
        let p = rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]);
        let pe = dc_rom::PeFile::from_file(&p).ok()?;
        let read = |p: &Pin| read_pinned(&pe, p).ok();
        let section = read(&UiSection::ExitGame.pin())?;
        let keys: Vec<String> = KEYS.iter().map(read).collect::<Option<_>>()?;
        let s = table.section(&section)?;
        let at = s.pair(&keys[0])?;
        let size = [s.int(&keys[1])?, s.int(&keys[2])?];
        let bsize = [s.int(&keys[3])?, s.int(&keys[4])?];
        let mut buttons = [[0; 4]; 3];
        for (i, b) in buttons.iter_mut().enumerate() {
            // button places are relative to the window
            let r = s.pair(&keys[5 + i])?;
            *b = [at[0] + r[0], at[1] + r[1], bsize[0], bsize[1]];
        }
        Some(ExitBox {
            window: [at[0], at[1], size[0], size[1]],
            buttons,
            window_tex: read(&WINDOW),
            button_tex: BUTTONS.map(|row| row.map(|p| read(&p))),
            captions: Default::default(),
            select_file: crate::menu::frontfx::FrontLits::from_rom(rom)
                .sounds
                .get(&crate::menu::frontfx::SOUND_SELECT)
                .cloned(),
        })
    }

    /// The sound of a button click (the release over a pressed button), as the menu's controls play it.
    pub fn select_cue(&self) -> Option<crate::sound::SoundCue> {
        self.select_file
            .clone()
            .map(|file| crate::sound::SoundCue::Interface {
                file: Some(file),
                what: crate::menu::frontfx::cue_name(crate::menu::frontfx::SOUND_SELECT),
            })
    }

    /// The buttons of `page` (the question uses the message box's Yes / No row).
    pub fn buttons_of(&self, page: LeavePage) -> Vec<[i32; 4]> {
        match page {
            LeavePage::Exit => self.buttons.to_vec(),
            LeavePage::Ask => crate::menu::popup::yes_no_buttons(),
        }
    }

    /// The button of `page` under the virtual point `p`.
    pub fn button_at(&self, page: LeavePage, p: [f32; 2]) -> Option<usize> {
        self.buttons_of(page).iter().position(|r| {
            p[0] >= r[0] as f32
                && p[1] >= r[1] as f32
                && p[0] < (r[0] + r[2]) as f32
                && p[1] < (r[1] + r[3]) as f32
        })
    }

    /// The exit page's draw items: the window (its image's top-left part of the box size) and the buttons in the
    /// state of `view`.
    pub fn push_items(&self, view: &LeaveView, out: &mut Vec<DrawItem>) {
        let w = self.window;
        match &self.window_tex {
            Some(t) => out.push(crate::menu::sheet::image(t, [w[0], w[1]], [w[2], w[3]])),
            None => out.push(DrawItem::Fill {
                rect: [w[0] as f32, w[1] as f32, w[2] as f32, w[3] as f32],
                colour: [24, 24, 24, 240],
            }),
        }
        for (i, r) in self.buttons.iter().enumerate() {
            let s = slot(view.hover == Some(i), view.down == Some(i));
            match &self.button_tex[i][s] {
                Some(t) => out.push(crate::menu::sheet::image(t, [r[0], r[1]], [r[2], r[3]])),
                None => {
                    let c = [
                        [70, 70, 70, 255],
                        [110, 110, 110, 255],
                        [0; 4],
                        [40, 40, 40, 255],
                    ][s];
                    out.push(DrawItem::Fill {
                        rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
                        colour: c,
                    });
                    out.push(crate::menu::sheet::place_text(
                        &self.captions[i],
                        [r[0], r[1]],
                        [r[2], r[3]],
                        dc_ui_data::pages::Align {
                            first: crate::menu::sheet::ALIGN_CENTRE,
                            second: dc_ui_data::pages::ALIGN_SECOND_CENTRE,
                        },
                        -1,
                        0,
                        0xffee_eeee,
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_button_states_pick_the_2009_slots() {
        assert_eq!(slot(false, false), 0);
        assert_eq!(slot(true, false), 1);
        assert_eq!(slot(true, true), 3);
    }

    #[test]
    fn hover_draws_the_over_image() {
        let mut b = ExitBox::fallback();
        b.button_tex = [0, 1, 2].map(|i| [0, 1, 2, 3].map(|s| Some(format!("b{i}s{s}"))));
        let texs = |v: LeaveView| {
            let mut out = Vec::new();
            b.push_items(&v, &mut out);
            out.iter()
                .filter_map(|d| match d {
                    DrawItem::Image { tex, .. } => Some(tex.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(texs(LeaveView::default()), ["b0s0", "b1s0", "b2s0"]);
        let v = LeaveView {
            hover: Some(2),
            ..Default::default()
        };
        assert_eq!(texs(v), ["b0s0", "b1s0", "b2s1"]);
        let v = LeaveView {
            hover: Some(0),
            down: Some(0),
            ..Default::default()
        };
        assert_eq!(texs(v), ["b0s3", "b1s0", "b2s0"]);
    }

    #[test]
    fn the_pointer_finds_the_button() {
        let b = ExitBox::fallback();
        let r = b.buttons[1];
        let p = [r[0] as f32 + 1.0, r[1] as f32 + 1.0];
        assert_eq!(b.button_at(LeavePage::Exit, p), Some(1));
        assert_eq!(b.button_at(LeavePage::Exit, [0.0, 0.0]), None);
    }
}
