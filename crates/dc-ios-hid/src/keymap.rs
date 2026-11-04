//! USB HID keyboard-page usage ids (what `GCKeyCode` holds) to winit key codes and US-layout characters.

use winit::keyboard::KeyCode;

/// The winit key code of a USB HID keyboard-page usage id (`None`: not a key the client maps).
pub fn key_code(usage: u16) -> Option<KeyCode> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR,
        KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    // 0x1e..0x27: 1..9 then 0
    const DIGITS: [KeyCode; 10] = [
        Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Digit0,
    ];
    const F1_F12: [KeyCode; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    const F13_F24: [KeyCode; 12] = [F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24];
    // 0x59..0x62: keypad 1..9 then 0
    const NUMPAD: [KeyCode; 10] = [
        Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9, Numpad0,
    ];
    const MODIFIERS: [KeyCode; 8] = [
        ControlLeft,
        ShiftLeft,
        AltLeft,
        SuperLeft,
        ControlRight,
        ShiftRight,
        AltRight,
        SuperRight,
    ];
    let u = usage as usize;
    Some(match usage {
        0x04..=0x1d => LETTERS[u - 0x04],
        0x1e..=0x27 => DIGITS[u - 0x1e],
        0x28 => Enter,
        0x29 => Escape,
        0x2a => Backspace,
        0x2b => Tab,
        0x2c => Space,
        0x2d => Minus,
        0x2e => Equal,
        0x2f => BracketLeft,
        0x30 => BracketRight,
        0x31 => Backslash,
        // non-US # ~ (the key left of Enter on ISO boards): the W3C code for it is Backslash too
        0x32 => Backslash,
        0x33 => Semicolon,
        0x34 => Quote,
        0x35 => Backquote,
        0x36 => Comma,
        0x37 => Period,
        0x38 => Slash,
        0x39 => CapsLock,
        0x3a..=0x45 => F1_F12[u - 0x3a],
        0x46 => PrintScreen,
        0x47 => ScrollLock,
        0x48 => Pause,
        0x49 => Insert,
        0x4a => Home,
        0x4b => PageUp,
        0x4c => Delete,
        0x4d => End,
        0x4e => PageDown,
        0x4f => ArrowRight,
        0x50 => ArrowLeft,
        0x51 => ArrowDown,
        0x52 => ArrowUp,
        0x53 => NumLock,
        0x54 => NumpadDivide,
        0x55 => NumpadMultiply,
        0x56 => NumpadSubtract,
        0x57 => NumpadAdd,
        0x58 => NumpadEnter,
        0x59..=0x62 => NUMPAD[u - 0x59],
        0x63 => NumpadDecimal,
        // non-US \ | (the key right of left Shift on ISO boards)
        0x64 => IntlBackslash,
        0x65 => ContextMenu,
        0x67 => NumpadEqual,
        0x68..=0x73 => F13_F24[u - 0x68],
        0x85 => NumpadComma,
        0x87 => IntlRo,
        0x89 => IntlYen,
        0xe0..=0xe7 => MODIFIERS[u - 0xe0],
        _ => return None,
    })
}

/// The character a key types on a US layout (`shift`: with Shift held), for text fields; `None` for keys that type
/// nothing (modifiers, arrows, function keys, Enter, Tab, Backspace). Caps Lock is the caller's business.
pub fn key_text(usage: u16, shift: bool) -> Option<char> {
    const DIGITS: &[u8; 10] = b"1234567890";
    const DIGITS_SHIFTED: &[u8; 10] = b"!@#$%^&*()";
    const NUMPAD: &[u8; 10] = b"1234567890";
    let u = usage as usize;
    let pick = |plain: char, shifted: char| if shift { shifted } else { plain };
    Some(match usage {
        0x04..=0x1d => {
            let c = (b'a' + (u - 0x04) as u8) as char;
            if shift {
                c.to_ascii_uppercase()
            } else {
                c
            }
        }
        0x1e..=0x27 => {
            let table = if shift { DIGITS_SHIFTED } else { DIGITS };
            table[u - 0x1e] as char
        }
        0x2c => ' ',
        0x2d => pick('-', '_'),
        0x2e => pick('=', '+'),
        0x2f => pick('[', '{'),
        0x30 => pick(']', '}'),
        0x31 | 0x32 => pick('\\', '|'),
        0x33 => pick(';', ':'),
        0x34 => pick('\'', '"'),
        0x35 => pick('`', '~'),
        0x36 => pick(',', '<'),
        0x37 => pick('.', '>'),
        0x38 => pick('/', '?'),
        0x54 => '/',
        0x55 => '*',
        0x56 => '-',
        0x57 => '+',
        0x59..=0x62 => NUMPAD[u - 0x59] as char,
        0x63 => '.',
        0x67 => '=',
        0x85 => ',',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_both_ways() {
        let codes = [KeyCode::KeyA, KeyCode::KeyB, KeyCode::KeyM, KeyCode::KeyZ];
        for (usage, code) in [0x04u16, 0x05, 0x10, 0x1d].into_iter().zip(codes) {
            assert_eq!(key_code(usage), Some(code));
        }
        for (i, usage) in (0x04u16..=0x1d).enumerate() {
            let lower = (b'a' + i as u8) as char;
            assert_eq!(key_text(usage, false), Some(lower));
            assert_eq!(key_text(usage, true), Some(lower.to_ascii_uppercase()));
            assert!(key_code(usage).is_some());
        }
    }

    #[test]
    fn digits_both_ways() {
        let table = [
            (0x1e, KeyCode::Digit1, '1', '!'),
            (0x1f, KeyCode::Digit2, '2', '@'),
            (0x20, KeyCode::Digit3, '3', '#'),
            (0x21, KeyCode::Digit4, '4', '$'),
            (0x22, KeyCode::Digit5, '5', '%'),
            (0x23, KeyCode::Digit6, '6', '^'),
            (0x24, KeyCode::Digit7, '7', '&'),
            (0x25, KeyCode::Digit8, '8', '*'),
            (0x26, KeyCode::Digit9, '9', '('),
            (0x27, KeyCode::Digit0, '0', ')'),
        ];
        for (usage, code, plain, shifted) in table {
            assert_eq!(key_code(usage), Some(code));
            assert_eq!(key_text(usage, false), Some(plain));
            assert_eq!(key_text(usage, true), Some(shifted));
        }
    }

    #[test]
    fn punctuation_both_ways() {
        let table = [
            (0x2c, KeyCode::Space, ' ', ' '),
            (0x2d, KeyCode::Minus, '-', '_'),
            (0x2e, KeyCode::Equal, '=', '+'),
            (0x2f, KeyCode::BracketLeft, '[', '{'),
            (0x30, KeyCode::BracketRight, ']', '}'),
            (0x31, KeyCode::Backslash, '\\', '|'),
            (0x33, KeyCode::Semicolon, ';', ':'),
            (0x34, KeyCode::Quote, '\'', '"'),
            (0x35, KeyCode::Backquote, '`', '~'),
            (0x36, KeyCode::Comma, ',', '<'),
            (0x37, KeyCode::Period, '.', '>'),
            (0x38, KeyCode::Slash, '/', '?'),
        ];
        for (usage, code, plain, shifted) in table {
            assert_eq!(key_code(usage), Some(code));
            assert_eq!(key_text(usage, false), Some(plain));
            assert_eq!(key_text(usage, true), Some(shifted));
        }
    }

    #[test]
    fn keypad() {
        assert_eq!(key_code(0x53), Some(KeyCode::NumLock));
        assert_eq!(key_code(0x58), Some(KeyCode::NumpadEnter));
        assert_eq!(key_code(0x59), Some(KeyCode::Numpad1));
        assert_eq!(key_code(0x62), Some(KeyCode::Numpad0));
        assert_eq!(key_code(0x63), Some(KeyCode::NumpadDecimal));
        assert_eq!(key_text(0x59, false), Some('1'));
        assert_eq!(key_text(0x61, true), Some('9'));
        assert_eq!(key_text(0x62, false), Some('0'));
        assert_eq!(key_text(0x55, false), Some('*'));
        assert_eq!(key_text(0x58, false), None);
    }

    #[test]
    fn modifiers() {
        let table = [
            KeyCode::ControlLeft,
            KeyCode::ShiftLeft,
            KeyCode::AltLeft,
            KeyCode::SuperLeft,
            KeyCode::ControlRight,
            KeyCode::ShiftRight,
            KeyCode::AltRight,
            KeyCode::SuperRight,
        ];
        for (usage, code) in (0xe0u16..=0xe7).zip(table) {
            assert_eq!(key_code(usage), Some(code));
            assert_eq!(key_text(usage, false), None);
            assert_eq!(key_text(usage, true), None);
        }
    }

    #[test]
    fn arrows_and_editing() {
        assert_eq!(key_code(0x4f), Some(KeyCode::ArrowRight));
        assert_eq!(key_code(0x50), Some(KeyCode::ArrowLeft));
        assert_eq!(key_code(0x51), Some(KeyCode::ArrowDown));
        assert_eq!(key_code(0x52), Some(KeyCode::ArrowUp));
        assert_eq!(key_code(0x28), Some(KeyCode::Enter));
        assert_eq!(key_code(0x29), Some(KeyCode::Escape));
        assert_eq!(key_code(0x2a), Some(KeyCode::Backspace));
        assert_eq!(key_code(0x2b), Some(KeyCode::Tab));
        assert_eq!(key_code(0x4c), Some(KeyCode::Delete));
        for usage in [0x28u16, 0x29, 0x2a, 0x2b, 0x4f, 0x50, 0x51, 0x52, 0x4c] {
            assert_eq!(key_text(usage, false), None);
        }
    }

    #[test]
    fn function_keys() {
        assert_eq!(key_code(0x3a), Some(KeyCode::F1));
        assert_eq!(key_code(0x45), Some(KeyCode::F12));
        assert_eq!(key_code(0x68), Some(KeyCode::F13));
        assert_eq!(key_code(0x73), Some(KeyCode::F24));
        assert_eq!(key_text(0x3a, false), None);
    }

    #[test]
    fn unknown_is_none() {
        for usage in [0x00u16, 0x01, 0x02, 0x03, 0x66, 0x74, 0xa0, 0xdf, 0xe8, 0xff, 0x1234] {
            assert_eq!(key_code(usage), None, "usage {usage:#x}");
            assert_eq!(key_text(usage, false), None, "usage {usage:#x}");
        }
    }
}
