//! iPadOS input for dc-client: a Bluetooth keyboard and mouse through Apple's GameController framework (GCKeyboard
//! and GCMouse give key usages, raw mouse deltas, buttons and scroll; winit's iOS backend delivers touches only),
//! the pointer lock of the view controller (`prefersPointerLocked`: the system pointer hides and the mouse reports
//! deltas), and the app container's Documents folder (where the 2009 client folder and our config / logs live).
//!
//! The Objective-C bridge (`src/hid.m`) is compiled on iOS targets only; on every other target the same API exists
//! and reports nothing, so the client's integration code compiles everywhere and the key maps are unit-tested on
//! the Mac.
//!
//! Interface (fixed; the client integrates against it):
//! - [`Hid::start`] once on the main thread after the window exists; [`Hid::poll`] once per frame.
//! - [`HidEvent`]: `Key { usage, down }` (USB HID keyboard-page usage ids, which is what `GCKeyCode` holds),
//!   `MouseMove { dx, dy }` (points, raw deltas, +x right, +y down), `MouseButton { button, down }` (0 left, 1 right,
//!   2 middle, 3.. auxiliary), `Scroll { dx, dy }` (lines / points as GameController reports them), and the
//!   presence notes `Keyboard(bool)` / `Mouse(bool)`.
//! - [`set_pointer_locked`]: the match locks the pointer; the menu releases it.
//! - [`documents_dir`]: `$HOME/Documents` of the app container.
//! - [`key_code`] / [`key_text`]: usage id -> `winit::keyboard::KeyCode`, and the character a key types on a US
//!   layout (with or without shift), for text fields.

use std::path::PathBuf;

/// One input event of the keyboard or the mouse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HidEvent {
    /// A key went down / up; `usage` is the USB HID keyboard-page usage id (`GCKeyCode`).
    Key { usage: u16, down: bool },
    /// Raw mouse motion in points (+x right, +y down).
    MouseMove { dx: f32, dy: f32 },
    /// A mouse button: 0 left, 1 right, 2 middle, 3.. auxiliary.
    MouseButton { button: u8, down: bool },
    /// Scroll wheel / surface motion.
    Scroll { dx: f32, dy: f32 },
    /// A keyboard was connected (true) / disconnected (false).
    Keyboard(bool),
    /// A mouse was connected (true) / disconnected (false).
    Mouse(bool),
}

/// The keyboard and mouse source. Create one with [`Hid::start`]; drain it with [`Hid::poll`].
pub struct Hid {
    #[cfg(target_os = "ios")]
    inner: ios::Inner,
    keyboard: bool,
    mouse: bool,
}

impl Hid {
    /// Registers the GameController handlers (iOS: main thread, after the window exists). Elsewhere: a source that
    /// never reports anything.
    pub fn start() -> Hid {
        Hid {
            #[cfg(target_os = "ios")]
            inner: ios::Inner::start(),
            keyboard: false,
            mouse: false,
        }
    }

    /// The events since the last call, in order.
    pub fn poll(&mut self) -> Vec<HidEvent> {
        #[cfg(target_os = "ios")]
        let events = self.inner.drain();
        #[cfg(not(target_os = "ios"))]
        let events: Vec<HidEvent> = Vec::new();
        for e in &events {
            match e {
                HidEvent::Keyboard(on) => self.keyboard = *on,
                HidEvent::Mouse(on) => self.mouse = *on,
                _ => {}
            }
        }
        events
    }

    /// Whether a keyboard is connected (as of the last [`Hid::poll`]).
    pub fn keyboard_present(&self) -> bool {
        self.keyboard
    }

    /// Whether a mouse is connected (as of the last [`Hid::poll`]).
    pub fn mouse_present(&self) -> bool {
        self.mouse
    }
}

/// Locks (`true`) or releases the pointer: the key window's root view controller's `prefersPointerLocked`. With the
/// lock the system pointer hides and GCMouse reports deltas while the app is in the foreground. No-op off iOS.
pub fn set_pointer_locked(on: bool) {
    #[cfg(target_os = "ios")]
    ios::set_pointer_locked(on);
    #[cfg(not(target_os = "ios"))]
    let _ = on;
}

/// Asks the system for a display rate up to `max_hz` (a ProMotion iPad stays at 60 Hz unless an app asks: a
/// CADisplayLink with that preferred range is the ask; 0 withdraws it). No-op off iOS.
pub fn set_frame_rate(max_hz: u32) {
    #[cfg(target_os = "ios")]
    ios::set_frame_rate(max_hz);
    #[cfg(not(target_os = "ios"))]
    let _ = max_hz;
}

/// The display's maximum frame rate (`UIScreen.maximumFramesPerSecond`: 120 on a ProMotion iPad); 0 off iOS.
pub fn display_max_hz() -> u32 {
    #[cfg(target_os = "ios")]
    {
        ios::display_max_hz()
    }
    #[cfg(not(target_os = "ios"))]
    {
        0
    }
}

/// The number of firings of the frame-rate display link so far (0 off iOS): divided by the seconds passed it is
/// the rate the system granted.
pub fn link_ticks() -> u64 {
    #[cfg(target_os = "ios")]
    {
        ios::link_ticks()
    }
    #[cfg(not(target_os = "ios"))]
    {
        0
    }
}

/// Whether UIKit reports the pointer locked right now (false off iOS).
pub fn pointer_is_locked() -> bool {
    #[cfg(target_os = "ios")]
    {
        ios::pointer_is_locked()
    }
    #[cfg(not(target_os = "ios"))]
    {
        false
    }
}

/// Asks for the pointer lock again when the system let it go (the flag is toggled: UIKit re-reads it on a change).
pub fn relock() {
    #[cfg(target_os = "ios")]
    ios::relock();
}

/// Hides (`true`) or shows the system pointer over the app's view through a pointer interaction with the hidden
/// style (independent of the pointer lock, which iPadOS does not always grant). No-op off iOS.
pub fn hide_pointer(hide: bool) {
    #[cfg(target_os = "ios")]
    ios::hide_pointer(hide);
    #[cfg(not(target_os = "ios"))]
    let _ = hide;
}

/// The app container's Documents folder (`$HOME/Documents`; visible in the Files app with file sharing on).
pub fn documents_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Documents")
}

/// The winit key code of a USB HID keyboard usage id (`None`: not a key the client maps).
pub fn key_code(usage: u16) -> Option<winit::keyboard::KeyCode> {
    keymap::key_code(usage)
}

/// The character a key types on a US layout (`shift`: with Shift held), for text fields; `None` for keys that type
/// nothing (modifiers, arrows, function keys).
pub fn key_text(usage: u16, shift: bool) -> Option<char> {
    keymap::key_text(usage, shift)
}

pub mod keymap;

#[cfg(target_os = "ios")]
mod ios;
