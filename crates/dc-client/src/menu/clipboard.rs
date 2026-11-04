//! Copy, cut and paste for the menu's text boxes: the chord (platform modifier + C / X / V), a small clipboard
//! trait (tests inject a fake) and, with the on-screen window, the system clipboard through `arboard`.
// DC-DEFAULT: CLI-1510..1512 (paste, copy, cut; Cmd on macOS, Ctrl elsewhere; masked boxes copy nothing)

use super::{Menu, MenuInput};

/// Where text is copied to and pasted from.
pub trait Clipboard {
    fn get(&mut self) -> Option<String>;
    fn set(&mut self, text: &str);
}

/// A clipboard command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chord {
    Paste,
    Copy,
    Cut,
}

/// The command a letter key means with the platform's modifier held: Cmd (super) on macOS, Ctrl elsewhere. Other
/// modifier states (Alt held, the wrong modifier) mean nothing here.
pub fn chord(letter: char, mac: bool, ctrl: bool, sup: bool, alt: bool) -> Option<Chord> {
    let held = if mac { sup && !ctrl } else { ctrl && !sup };
    if !held || alt {
        return None;
    }
    match letter.to_ascii_lowercase() {
        'v' => Some(Chord::Paste),
        'c' => Some(Chord::Copy),
        'x' => Some(Chord::Cut),
        _ => None,
    }
}

/// Runs the command on the menu.
pub fn run(c: Chord, menu: &mut Menu, clip: &mut dyn Clipboard) {
    match c {
        Chord::Paste => {
            if let Some(t) = clip.get() {
                menu.input(MenuInput::Paste(t));
            }
        }
        Chord::Copy => {
            if let Some(t) = menu.copy_text() {
                clip.set(&t);
            }
        }
        Chord::Cut => {
            if let Some(t) = menu.copy_text() {
                clip.set(&t);
                menu.input(MenuInput::Cut);
            }
        }
    }
}

/// The system clipboard (window builds only).
#[cfg(all(feature = "window", not(target_os = "ios")))]
pub struct SystemClipboard(Option<arboard::Clipboard>);

/// iPadOS: `arboard` has no UIKit backend; the boxes copy and paste among themselves through one text of the
/// process (kept across the menu's turns).
// DC-DEFAULT: CLI-1604 (proposed)
#[cfg(all(feature = "window", target_os = "ios"))]
pub struct SystemClipboard(());

#[cfg(all(feature = "window", target_os = "ios"))]
static APP_CLIPBOARD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[cfg(all(feature = "window", not(target_os = "ios")))]
impl SystemClipboard {
    pub fn new() -> Self {
        Self(arboard::Clipboard::new().ok())
    }
}

#[cfg(all(feature = "window", target_os = "ios"))]
impl SystemClipboard {
    pub fn new() -> Self {
        Self(())
    }
}

#[cfg(feature = "window")]
impl Default for SystemClipboard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(feature = "window", not(target_os = "ios")))]
impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        self.0.as_mut()?.get_text().ok()
    }
    fn set(&mut self, text: &str) {
        if let Some(c) = self.0.as_mut() {
            let _ = c.set_text(text.to_string());
        }
    }
}

#[cfg(all(feature = "window", target_os = "ios"))]
impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        APP_CLIPBOARD.lock().ok()?.clone()
    }
    fn set(&mut self, text: &str) {
        if let Ok(mut c) = APP_CLIPBOARD.lock() {
            *c = Some(text.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_per_platform() {
        assert_eq!(chord('v', false, true, false, false), Some(Chord::Paste));
        assert_eq!(chord('v', false, false, true, false), None);
        assert_eq!(chord('V', true, false, true, false), Some(Chord::Paste));
        assert_eq!(chord('c', true, true, false, false), None);
        assert_eq!(chord('x', false, true, false, false), Some(Chord::Cut));
        assert_eq!(chord('c', false, true, false, true), None);
        assert_eq!(chord('w', false, true, false, false), None);
    }

    struct Fake(Option<String>);
    impl Clipboard for Fake {
        fn get(&mut self) -> Option<String> {
            self.0.clone()
        }
        fn set(&mut self, t: &str) {
            self.0 = Some(t.to_string());
        }
    }

    fn login_menu() -> Option<Menu> {
        use crate::menu::link::LinkOptions;
        use crate::menu::MenuOptions;
        let rom = std::env::var_os("CA_ROM")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir());
        let Some(rom) = rom else {
            println!("skipped: CA_ROM unset");
            return None;
        };
        Some(Menu::new(MenuOptions {
            auth: "127.0.0.1:1".parse().unwrap(),
            user: String::new(),
            link: LinkOptions::new("login-clip"),
            rom: Some(rom),
            font: Some((crate::testkit::synth_font(), 0)),
            width: 1024,
            height: 768,
        }))
    }

    fn text(m: &Menu, id: i32) -> String {
        m.sheet()
            .part(0, crate::menu::login::ID_BOX + (id - 9), dc_ui_data::WidgetKind::EditBox)
            .map(|p| p.text.clone())
            .unwrap_or_default()
    }

    #[test]
    fn paste_copy_cut_in_the_login_boxes() {
        let Some(mut m) = login_menu() else { return };
        // paste at the end of the typed text, control characters dropped, capped at the box's length
        m.input(MenuInput::Char('a'));
        let mut clip = Fake(Some("bc\r\nd\te".to_string() + &"z".repeat(200)));
        run(Chord::Paste, &mut m, &mut clip);
        let id = text(&m, 9);
        assert!(id.starts_with("abcde") && !id.contains(['\n', '\t']), "{id}");
        assert!(id.chars().count() < 100, "{id}");
        // copy gives the plain box's text
        let mut out = Fake(None);
        run(Chord::Copy, &mut m, &mut out);
        assert_eq!(out.0.as_deref(), Some(id.as_str()));
        // cut copies and empties
        let mut out = Fake(None);
        run(Chord::Cut, &mut m, &mut out);
        assert_eq!(out.0.as_deref(), Some(id.as_str()));
        assert_eq!(text(&m, 9), "");
        // the password box takes a paste but copies and cuts nothing
        m.input(MenuInput::Key(crate::menu::MenuKey::Tab));
        let mut clip = Fake(Some("secret".into()));
        run(Chord::Paste, &mut m, &mut clip);
        assert_eq!(text(&m, 10), "secret");
        let mut out = Fake(Some("old".into()));
        run(Chord::Copy, &mut m, &mut out);
        run(Chord::Cut, &mut m, &mut out);
        assert_eq!(out.0.as_deref(), Some("old"));
        assert_eq!(text(&m, 10), "secret");
    }
}
