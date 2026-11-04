//! The iPad's keyboard and mouse as the window events the desktop gets from winit (CLI-1604). winit's iOS backend
//! delivers touches only; dc-ios-hid reads the Bluetooth keyboard and mouse through GameController. The bridge turns
//! both into what the menu and the match already read: key presses with their physical code, logical key and text
//! ([`KeyIn`], the path of `KeyboardInput`), the modifier state before the key that changes it (`ModifiersChanged`),
//! the pointer in surface pixels (`CursorMoved`), the buttons (`MouseInput`), the wheel (`MouseWheel`) and the raw
//! mouse motion (`DeviceEvent::MouseMotion`).
//!
//! The pointer: a touch (iPadOS hands an unlocked mouse's or trackpad's presses over as touches with a place) puts
//! it where the touch is and presses the left button; the mouse's own motion moves it from there (clamped to the
//! surface) while the cursor is not taken, so the menu's hover follows a mouse that never touches. A button pressed
//! through both sources (a click is a touch and a GameController press at once) is one press: a press of a held
//! button and a release of a free one are dropped.
// DC-DEFAULT: CLI-1604 (proposed)
// PUBLIC: winit 0.30 docs (WindowEvent::Touch locations are physical pixels of the window; KeyEvent::text is None
// for a release; ModifiersChanged precedes the key event); USB HID Usage Tables 1.5 §10 (keyboard page: 0x04..0x1d
// letters, 0x39 Caps Lock, 0xe0..0xe7 the modifiers); Apple docs (GCMouseInput deltas are points)

use crate::window::KeyIn;
use dc_ios_hid::HidEvent;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, Modifiers, MouseButton, MouseScrollDelta, TouchPhase,
    WindowEvent,
};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, NativeKey, PhysicalKey, SmolStr};

/// One input for the current stage (the menu or the match), in the order the stage gets it.
#[derive(Debug, Clone, PartialEq)]
pub enum Synth {
    /// The modifier state changed (`ModifiersChanged`).
    Modifiers(ModifiersState),
    /// A key press or release (`KeyboardInput`).
    Key(KeyIn),
    /// The pointer in surface pixels (`CursorMoved`).
    Cursor(f64, f64),
    /// A mouse button went down (`true`) or up (`MouseInput`).
    Button(MouseButton, bool),
    /// The wheel (`MouseWheel`).
    Wheel(MouseScrollDelta),
    /// Raw mouse motion (`DeviceEvent::MouseMotion`, what mouse look reads).
    Motion(f64, f64),
}

impl Synth {
    /// The winit window event of this input (`None`: a key, which goes through [`KeyIn`], or raw motion).
    pub fn window_event(&self) -> Option<WindowEvent> {
        let device_id = DeviceId::dummy();
        Some(match self {
            Synth::Modifiers(m) => WindowEvent::ModifiersChanged(Modifiers::from(*m)),
            Synth::Cursor(x, y) => WindowEvent::CursorMoved {
                device_id,
                position: (*x, *y).into(),
            },
            Synth::Button(button, down) => WindowEvent::MouseInput {
                device_id,
                state: if *down {
                    ElementState::Pressed
                } else {
                    ElementState::Released
                },
                button: *button,
            },
            Synth::Wheel(delta) => WindowEvent::MouseWheel {
                device_id,
                delta: *delta,
                phase: TouchPhase::Moved,
            },
            Synth::Key(_) | Synth::Motion(..) => return None,
        })
    }

    /// The winit device event of this input (raw motion only).
    pub fn device_event(&self) -> Option<DeviceEvent> {
        match self {
            Synth::Motion(dx, dy) => Some(DeviceEvent::MouseMotion { delta: (*dx, *dy) }),
            _ => None,
        }
    }
}

/// The window the inputs are for: its surface (winit's inner size, physical pixels), the whole view (outer size)
/// the surface is shown over, the points-to-pixels scale, and whether the stage holds the cursor for mouse look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub surface: (u32, u32),
    pub outer: (u32, u32),
    pub scale: f64,
    pub captured: bool,
}

impl View {
    /// Surface pixels per window pixel on each axis (the surface is stretched over the whole view).
    fn per_pixel(&self) -> (f64, f64) {
        let r = |s: u32, o: u32| {
            if o == 0 {
                1.0
            } else {
                f64::from(s.max(1)) / f64::from(o)
            }
        };
        (
            r(self.surface.0, self.outer.0),
            r(self.surface.1, self.outer.1),
        )
    }

    fn clamp(&self, p: [f64; 2]) -> [f64; 2] {
        let max = |n: u32| f64::from(n.max(1) - 1);
        [
            p[0].clamp(0.0, max(self.surface.0)),
            p[1].clamp(0.0, max(self.surface.1)),
        ]
    }
}

/// The modifier of a keyboard-page usage (0xe0..0xe7: left Ctrl, Shift, Alt, GUI, then the right ones).
pub fn modifier_of_usage(usage: u16) -> Option<ModifiersState> {
    Some(match usage {
        0xe0 | 0xe4 => ModifiersState::CONTROL,
        0xe1 | 0xe5 => ModifiersState::SHIFT,
        0xe2 | 0xe6 => ModifiersState::ALT,
        0xe3 | 0xe7 => ModifiersState::SUPER,
        _ => return None,
    })
}

/// The logical key winit reports for `code` (named keys by name, typing keys by their text).
pub fn logical_of(code: KeyCode, text: Option<char>) -> Key {
    use NamedKey as N;
    let named = match code {
        KeyCode::Enter | KeyCode::NumpadEnter => N::Enter,
        KeyCode::Escape => N::Escape,
        KeyCode::Tab => N::Tab,
        KeyCode::Backspace => N::Backspace,
        KeyCode::Delete => N::Delete,
        KeyCode::Insert => N::Insert,
        KeyCode::Home => N::Home,
        KeyCode::End => N::End,
        KeyCode::PageUp => N::PageUp,
        KeyCode::PageDown => N::PageDown,
        KeyCode::Space => N::Space,
        KeyCode::ArrowUp => N::ArrowUp,
        KeyCode::ArrowDown => N::ArrowDown,
        KeyCode::ArrowLeft => N::ArrowLeft,
        KeyCode::ArrowRight => N::ArrowRight,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => N::Shift,
        KeyCode::ControlLeft | KeyCode::ControlRight => N::Control,
        KeyCode::AltLeft | KeyCode::AltRight => N::Alt,
        KeyCode::SuperLeft | KeyCode::SuperRight => N::Super,
        KeyCode::CapsLock => N::CapsLock,
        KeyCode::F1 => N::F1,
        KeyCode::F2 => N::F2,
        KeyCode::F3 => N::F3,
        KeyCode::F4 => N::F4,
        KeyCode::F5 => N::F5,
        KeyCode::F6 => N::F6,
        KeyCode::F7 => N::F7,
        KeyCode::F8 => N::F8,
        KeyCode::F9 => N::F9,
        KeyCode::F10 => N::F10,
        KeyCode::F11 => N::F11,
        KeyCode::F12 => N::F12,
        _ => {
            return match text {
                Some(c) => Key::Character(SmolStr::new(c.encode_utf8(&mut [0; 4]))),
                None => Key::Unidentified(NativeKey::Unidentified),
            }
        }
    };
    Key::Named(named)
}

/// The bridge's state: the held modifiers, Caps Lock, the pointer and the held buttons.
#[derive(Debug, Default)]
pub struct Bridge {
    /// The held modifier usages (bit `usage - 0xe0`).
    held: u8,
    caps: bool,
    /// The pointer in surface pixels (`None`: the middle of the surface until something places it).
    pointer: Option<[f64; 2]>,
    /// The held buttons (bit per button number, both sources).
    buttons: u32,
    /// The touch that holds the left button.
    touch: Option<u64>,
}

/// The winit button of a GameController button number (0 left, 1 right, 2 middle, 3.. auxiliary).
pub fn mouse_button(n: u8) -> MouseButton {
    match n {
        0 => MouseButton::Left,
        1 => MouseButton::Right,
        2 => MouseButton::Middle,
        3 => MouseButton::Back,
        4 => MouseButton::Forward,
        n => MouseButton::Other(u16::from(n)),
    }
}

fn button_bit(b: MouseButton) -> u32 {
    let n = match b {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
        MouseButton::Other(n) => u32::from(n).min(31),
    };
    1 << n
}

impl Bridge {
    /// The modifier state of the held modifier keys.
    pub fn modifiers(&self) -> ModifiersState {
        let mut m = ModifiersState::empty();
        for bit in 0..8u16 {
            if self.held & (1 << bit) != 0 {
                if let Some(s) = modifier_of_usage(0xe0 + bit) {
                    m |= s;
                }
            }
        }
        m
    }

    /// The pointer now (surface pixels), the middle of `view` before anything placed it.
    pub fn pointer(&self, view: &View) -> [f64; 2] {
        self.pointer.unwrap_or([
            f64::from(view.surface.0) / 2.0,
            f64::from(view.surface.1) / 2.0,
        ])
    }

    /// One event of the keyboard or the mouse.
    pub fn hid(&mut self, e: HidEvent, view: &View) -> Vec<Synth> {
        match e {
            HidEvent::Key { usage, down } => self.key(usage, down),
            HidEvent::MouseMove { dx, dy } => {
                let mut out = vec![Synth::Motion(f64::from(dx), f64::from(dy))];
                // the cursor taken for mouse look stays where it is (the desktop's locked cursor)
                if !view.captured {
                    let (kx, ky) = view.per_pixel();
                    let p = self.pointer(view);
                    let p = view.clamp([
                        p[0] + f64::from(dx) * view.scale * kx,
                        p[1] + f64::from(dy) * view.scale * ky,
                    ]);
                    self.pointer = Some(p);
                    out.push(Synth::Cursor(p[0], p[1]));
                }
                out
            }
            HidEvent::MouseButton { button, down } => self
                .button(mouse_button(button), down)
                .into_iter()
                .collect(),
            HidEvent::Scroll { dx, dy } => {
                vec![Synth::Wheel(MouseScrollDelta::LineDelta(dx, dy))]
            }
            HidEvent::Keyboard(_) | HidEvent::Mouse(_) => Vec::new(),
        }
    }

    /// One touch of winit (`x`, `y`: window pixels). The first finger (or the pointer's press) moves the pointer and
    /// holds the left button until it lifts; other touches meanwhile are ignored.
    pub fn touch(&mut self, id: u64, phase: TouchPhase, x: f64, y: f64, view: &View) -> Vec<Synth> {
        if self.touch.is_some_and(|t| t != id) {
            return Vec::new();
        }
        let (kx, ky) = view.per_pixel();
        let p = view.clamp([x * kx, y * ky]);
        self.pointer = Some(p);
        let mut out = vec![Synth::Cursor(p[0], p[1])];
        match phase {
            TouchPhase::Started => {
                self.touch = Some(id);
                out.extend(self.button(MouseButton::Left, true));
            }
            TouchPhase::Moved => {}
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.touch = None;
                out.extend(self.button(MouseButton::Left, false));
            }
        }
        out
    }

    /// A button from either source: a change of the held state, or nothing.
    fn button(&mut self, b: MouseButton, down: bool) -> Option<Synth> {
        let bit = button_bit(b);
        if (self.buttons & bit != 0) == down {
            return None;
        }
        self.buttons ^= bit;
        Some(Synth::Button(b, down))
    }

    /// A key: the modifier state first when the key is a modifier, then the key itself (keys without a code are
    /// dropped). No key repeats (GameController reports none).
    fn key(&mut self, usage: u16, down: bool) -> Vec<Synth> {
        let mut out = Vec::new();
        if modifier_of_usage(usage).is_some() {
            let before = self.modifiers();
            let bit = 1u8 << (usage - 0xe0);
            if down {
                self.held |= bit;
            } else {
                self.held &= !bit;
            }
            let now = self.modifiers();
            if now != before {
                out.push(Synth::Modifiers(now));
            }
        }
        // Caps Lock toggles on its press and turns the letters to capitals (Shift turns them back)
        if usage == 0x39 && down {
            self.caps = !self.caps;
        }
        let Some(code) = dc_ios_hid::key_code(usage) else {
            return out;
        };
        let m = self.modifiers();
        let letter = (0x04..=0x1d).contains(&usage);
        let shift = m.shift_key() ^ (self.caps && letter);
        let ch = dc_ios_hid::key_text(usage, shift);
        // a Ctrl / Cmd chord types nothing (the desktop's text is a control character the boxes drop)
        let text = ch
            .filter(|_| down && !m.control_key() && !m.super_key())
            .map(|c| SmolStr::new(c.encode_utf8(&mut [0; 4])));
        out.push(Synth::Key(KeyIn {
            physical: PhysicalKey::Code(code),
            logical: logical_of(code, ch),
            text,
            down,
            repeat: false,
        }));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: View = View {
        surface: (2048, 1536),
        outer: (2048, 1536),
        scale: 2.0,
        captured: false,
    };

    fn key(out: &[Synth]) -> &KeyIn {
        out.iter()
            .find_map(|s| match s {
                Synth::Key(k) => Some(k),
                _ => None,
            })
            .expect("a key")
    }

    #[test]
    fn a_letter_is_its_code_its_text_and_a_character_key() {
        let mut b = Bridge::default();
        let out = b.hid(
            HidEvent::Key {
                usage: 0x04,
                down: true,
            },
            &VIEW,
        );
        let k = key(&out);
        assert_eq!(k.physical, PhysicalKey::Code(KeyCode::KeyA));
        assert_eq!(k.logical, Key::Character("a".into()));
        assert_eq!(k.text.as_deref(), Some("a"));
        assert!(k.down && !k.repeat);
        // the release types nothing (winit's text is None for a release)
        let out = b.hid(
            HidEvent::Key {
                usage: 0x04,
                down: false,
            },
            &VIEW,
        );
        assert_eq!(key(&out).text, None);
        assert!(!key(&out).down);
    }

    #[test]
    fn shift_sends_the_modifier_state_first_and_types_capitals() {
        let mut b = Bridge::default();
        let out = b.hid(
            HidEvent::Key {
                usage: 0xe1,
                down: true,
            },
            &VIEW,
        );
        assert_eq!(out[0], Synth::Modifiers(ModifiersState::SHIFT));
        let k = key(&out);
        assert_eq!(k.physical, PhysicalKey::Code(KeyCode::ShiftLeft));
        assert_eq!(k.logical, Key::Named(NamedKey::Shift));
        let out = b.hid(
            HidEvent::Key {
                usage: 0x05,
                down: true,
            },
            &VIEW,
        );
        assert_eq!(key(&out).text.as_deref(), Some("B"));
        // the right Shift held too: no change of the state, then both up
        let out = b.hid(
            HidEvent::Key {
                usage: 0xe5,
                down: true,
            },
            &VIEW,
        );
        assert!(!out.iter().any(|s| matches!(s, Synth::Modifiers(_))));
        b.hid(
            HidEvent::Key {
                usage: 0xe1,
                down: false,
            },
            &VIEW,
        );
        let out = b.hid(
            HidEvent::Key {
                usage: 0xe5,
                down: false,
            },
            &VIEW,
        );
        assert_eq!(out[0], Synth::Modifiers(ModifiersState::empty()));
    }

    #[test]
    fn caps_lock_capitalises_letters_only_and_ctrl_chords_type_nothing() {
        let mut b = Bridge::default();
        b.hid(
            HidEvent::Key {
                usage: 0x39,
                down: true,
            },
            &VIEW,
        );
        b.hid(
            HidEvent::Key {
                usage: 0x39,
                down: false,
            },
            &VIEW,
        );
        let out = b.hid(
            HidEvent::Key {
                usage: 0x06,
                down: true,
            },
            &VIEW,
        );
        assert_eq!(key(&out).text.as_deref(), Some("C"));
        let out = b.hid(
            HidEvent::Key {
                usage: 0x1e,
                down: true,
            },
            &VIEW,
        );
        assert_eq!(key(&out).text.as_deref(), Some("1"));
        // Cmd+V: the key with its code, no text (the menu's clipboard chord reads the code and the state)
        let out = b.hid(
            HidEvent::Key {
                usage: 0xe3,
                down: true,
            },
            &VIEW,
        );
        assert_eq!(out[0], Synth::Modifiers(ModifiersState::SUPER));
        let out = b.hid(
            HidEvent::Key {
                usage: 0x19,
                down: true,
            },
            &VIEW,
        );
        let k = key(&out);
        assert_eq!(k.physical, PhysicalKey::Code(KeyCode::KeyV));
        assert_eq!(k.text, None);
    }

    #[test]
    fn named_keys_are_named_and_space_types_a_space() {
        let mut b = Bridge::default();
        let out = b.hid(
            HidEvent::Key {
                usage: 0x28,
                down: true,
            },
            &VIEW,
        );
        assert_eq!(key(&out).logical, Key::Named(NamedKey::Enter));
        assert_eq!(key(&out).text, None);
        let out = b.hid(
            HidEvent::Key {
                usage: 0x29,
                down: true,
            },
            &VIEW,
        );
        assert_eq!(key(&out).physical, PhysicalKey::Code(KeyCode::Escape));
        let out = b.hid(
            HidEvent::Key {
                usage: 0x2c,
                down: true,
            },
            &VIEW,
        );
        assert_eq!(key(&out).logical, Key::Named(NamedKey::Space));
        assert_eq!(key(&out).text.as_deref(), Some(" "));
        let out = b.hid(
            HidEvent::Key {
                usage: 0xe0,
                down: true,
            },
            &VIEW,
        );
        assert_eq!(key(&out).logical, Key::Named(NamedKey::Control));
    }

    #[test]
    fn mouse_motion_is_raw_motion_and_moves_the_free_pointer_in_pixels() {
        let mut b = Bridge::default();
        let out = b.hid(HidEvent::MouseMove { dx: 10.0, dy: -5.0 }, &VIEW);
        assert_eq!(out[0], Synth::Motion(10.0, -5.0));
        // from the middle, 2 pixels per point
        assert_eq!(out[1], Synth::Cursor(1024.0 + 20.0, 768.0 - 10.0));
        // clamped to the surface
        let out = b.hid(
            HidEvent::MouseMove {
                dx: -5000.0,
                dy: 5000.0,
            },
            &VIEW,
        );
        assert_eq!(out[1], Synth::Cursor(0.0, 1535.0));
        // the match holds the cursor: motion only
        let held = View {
            captured: true,
            ..VIEW
        };
        let out = b.hid(HidEvent::MouseMove { dx: 3.0, dy: 4.0 }, &held);
        assert_eq!(out, vec![Synth::Motion(3.0, 4.0)]);
        assert_eq!(b.pointer(&VIEW), [0.0, 1535.0]);
    }

    #[test]
    fn a_touch_places_the_pointer_and_holds_the_left_button() {
        let mut b = Bridge::default();
        // a surface smaller than the view it is shown over (the safe area): window pixels scale to surface pixels
        let v = View {
            surface: (1024, 768),
            outer: (2048, 1536),
            ..VIEW
        };
        let out = b.touch(7, TouchPhase::Started, 400.0, 300.0, &v);
        assert_eq!(
            out,
            vec![
                Synth::Cursor(200.0, 150.0),
                Synth::Button(MouseButton::Left, true)
            ]
        );
        // a second finger meanwhile does nothing
        assert!(b.touch(8, TouchPhase::Started, 0.0, 0.0, &v).is_empty());
        assert_eq!(
            b.touch(7, TouchPhase::Moved, 410.0, 300.0, &v),
            vec![Synth::Cursor(205.0, 150.0)]
        );
        let out = b.touch(7, TouchPhase::Ended, 410.0, 300.0, &v);
        assert_eq!(out[1], Synth::Button(MouseButton::Left, false));
        // the mouse goes on from the touch's place
        let out = b.hid(HidEvent::MouseMove { dx: 1.0, dy: 0.0 }, &v);
        assert_eq!(out[1], Synth::Cursor(206.0, 150.0));
    }

    #[test]
    fn a_click_through_both_sources_is_one_press() {
        let mut b = Bridge::default();
        let touch = b.touch(1, TouchPhase::Started, 10.0, 10.0, &VIEW);
        assert!(touch.contains(&Synth::Button(MouseButton::Left, true)));
        assert!(b
            .hid(
                HidEvent::MouseButton {
                    button: 0,
                    down: true
                },
                &VIEW
            )
            .is_empty());
        assert_eq!(
            b.hid(
                HidEvent::MouseButton {
                    button: 0,
                    down: false
                },
                &VIEW
            ),
            vec![Synth::Button(MouseButton::Left, false)]
        );
        let lift = b.touch(1, TouchPhase::Ended, 10.0, 10.0, &VIEW);
        assert_eq!(lift, vec![Synth::Cursor(10.0, 10.0)]);
        // the right button and the wheel come from the mouse alone
        assert_eq!(
            b.hid(
                HidEvent::MouseButton {
                    button: 1,
                    down: true
                },
                &VIEW
            ),
            vec![Synth::Button(MouseButton::Right, true)]
        );
        assert_eq!(
            b.hid(HidEvent::Scroll { dx: 0.0, dy: 1.0 }, &VIEW),
            vec![Synth::Wheel(MouseScrollDelta::LineDelta(0.0, 1.0))]
        );
    }

    #[test]
    fn the_inputs_become_the_winit_events_the_stages_read() {
        assert!(matches!(
            Synth::Button(MouseButton::Left, true).window_event(),
            Some(WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            })
        ));
        match Synth::Cursor(3.0, 4.0).window_event() {
            Some(WindowEvent::CursorMoved { position, .. }) => {
                assert_eq!((position.x, position.y), (3.0, 4.0))
            }
            e => panic!("{e:?}"),
        }
        match Synth::Modifiers(ModifiersState::ALT).window_event() {
            Some(WindowEvent::ModifiersChanged(m)) => assert!(m.state().alt_key()),
            e => panic!("{e:?}"),
        }
        assert!(matches!(
            Synth::Motion(1.0, 2.0).device_event(),
            Some(DeviceEvent::MouseMotion { delta: (1.0, 2.0) })
        ));
        assert!(Synth::Motion(1.0, 2.0).window_event().is_none());
    }
}
