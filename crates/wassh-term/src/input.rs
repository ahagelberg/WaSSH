//! Keyboard encoding: winit/egui key strokes to the bytes a VT expects.
//!
//! The rules follow xterm: cursor keys switch between CSI and SS3 depending on
//! `DECCKM`, modifier parameters use `1 + Shift + 2*Alt + 4*Ctrl`, and Ctrl+letter maps
//! to the corresponding C0 control code.

use crate::emulator::SnapshotModes;

/// A key without text interpretation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backtab,
    Backspace,
    Escape,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Function(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub key: Key,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyStroke {
    pub fn plain(key: Key) -> Self {
        Self {
            key,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    fn modifier_param(&self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl)
    }

    fn has_modifiers(&self) -> bool {
        self.ctrl || self.alt || self.shift
    }
}

/// Encode a stroke for the given terminal modes. Returns an empty vector for keys that
/// produce no bytes (for example a bare modifier press).
pub fn encode(stroke: &KeyStroke, modes: SnapshotModes) -> Vec<u8> {
    let mut out = Vec::new();
    let prefix_alt = stroke.alt && !stroke.ctrl;

    let mut push = |bytes: &[u8], out: &mut Vec<u8>| {
        if prefix_alt && !bytes.starts_with(b"\x1b") {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
    };

    match stroke.key {
        Key::Char(ch) => {
            if stroke.ctrl {
                let code = match ch {
                    'a'..='z' => Some(ch as u8 - b'a' + 1),
                    'A'..='Z' => Some(ch as u8 - b'A' + 1),
                    '@' | ' ' | '2' => Some(0x00),
                    '[' | '3' => Some(0x1b),
                    '\\' | '4' => Some(0x1c),
                    ']' | '5' => Some(0x1d),
                    '^' | '6' => Some(0x1e),
                    '_' | '/' | '7' => Some(0x1f),
                    '8' => Some(0x7f),
                    _ => None,
                };
                if let Some(code) = code {
                    push(&[code], &mut out);
                    return out;
                }
            }
            let mut buf = [0u8; 4];
            push(ch.encode_utf8(&mut buf).as_bytes(), &mut out);
        }
        Key::Enter => push(b"\r", &mut out),
        Key::Tab if stroke.shift => push(b"\x1b[Z", &mut out),
        Key::Tab => push(b"\t", &mut out),
        Key::Backtab => push(b"\x1b[Z", &mut out),
        Key::Backspace => push(b"\x7f", &mut out),
        Key::Escape => push(b"\x1b", &mut out),
        Key::ArrowUp => push_cursor(b'A', stroke, modes, &mut push, &mut out),
        Key::ArrowDown => push_cursor(b'B', stroke, modes, &mut push, &mut out),
        Key::ArrowRight => push_cursor(b'C', stroke, modes, &mut push, &mut out),
        Key::ArrowLeft => push_cursor(b'D', stroke, modes, &mut push, &mut out),
        Key::Home => push_cursor(b'H', stroke, modes, &mut push, &mut out),
        Key::End => push_cursor(b'F', stroke, modes, &mut push, &mut out),
        Key::Insert => push_tilde(2, stroke, &mut push, &mut out),
        Key::Delete => push_tilde(3, stroke, &mut push, &mut out),
        Key::PageUp => push_tilde(5, stroke, &mut push, &mut out),
        Key::PageDown => push_tilde(6, stroke, &mut push, &mut out),
        Key::Function(number) => push_function(number, stroke, &mut push, &mut out),
    }
    out
}

fn push_cursor(
    final_byte: u8,
    stroke: &KeyStroke,
    modes: SnapshotModes,
    push: &mut impl FnMut(&[u8], &mut Vec<u8>),
    out: &mut Vec<u8>,
) {
    if stroke.has_modifiers() {
        let sequence = format!("\x1b[1;{}{}", stroke.modifier_param(), char::from(final_byte));
        push(sequence.as_bytes(), out);
    } else if modes.app_cursor {
        push(&[0x1b, b'O', final_byte], out);
    } else {
        push(&[0x1b, b'[', final_byte], out);
    }
}

fn push_tilde(
    number: u8,
    stroke: &KeyStroke,
    push: &mut impl FnMut(&[u8], &mut Vec<u8>),
    out: &mut Vec<u8>,
) {
    if stroke.has_modifiers() {
        let sequence = format!("\x1b[{number};{}~", stroke.modifier_param());
        push(sequence.as_bytes(), out);
    } else {
        let sequence = format!("\x1b[{number}~");
        push(sequence.as_bytes(), out);
    }
}

fn push_function(
    number: u8,
    stroke: &KeyStroke,
    push: &mut impl FnMut(&[u8], &mut Vec<u8>),
    out: &mut Vec<u8>,
) {
    let (code, tilde) = match number {
        1 => (b'P', false),
        2 => (b'Q', false),
        3 => (b'R', false),
        4 => (b'S', false),
        5 => (15, true),
        6 => (17, true),
        7 => (18, true),
        8 => (19, true),
        9 => (20, true),
        10 => (21, true),
        11 => (23, true),
        12 => (24, true),
        _ => return,
    };

    if tilde {
        push_tilde(code, stroke, push, out);
    } else if stroke.has_modifiers() {
        let sequence = format!("\x1b[1;{}{}", stroke.modifier_param(), char::from(code));
        push(sequence.as_bytes(), out);
    } else {
        push(&[0x1b, b'O', code], out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes(app_cursor: bool) -> SnapshotModes {
        SnapshotModes {
            app_cursor,
            ..Default::default()
        }
    }

    fn simple(key: Key) -> Vec<u8> {
        encode(&KeyStroke::plain(key), modes(false))
    }

    #[test]
    fn printable_text_is_utf8() {
        assert_eq!(simple(Key::Char('a')), b"a");
        assert_eq!(simple(Key::Char('你')), "你".as_bytes());
    }

    #[test]
    fn control_keys() {
        assert_eq!(simple(Key::Enter), b"\r");
        assert_eq!(simple(Key::Tab), b"\t");
        assert_eq!(simple(Key::Backspace), b"\x7f");
        assert_eq!(simple(Key::Escape), b"\x1b");
        assert_eq!(simple(Key::Backtab), b"\x1b[Z");
    }

    #[test]
    fn ctrl_letters_become_control_codes() {
        let stroke = KeyStroke {
            key: Key::Char('c'),
            ctrl: true,
            alt: false,
            shift: false,
        };
        assert_eq!(encode(&stroke, modes(false)), b"\x03");
        let stroke = KeyStroke {
            key: Key::Char('D'),
            ctrl: true,
            alt: false,
            shift: true,
        };
        assert_eq!(encode(&stroke, modes(false)), b"\x04");
    }

    #[test]
    fn arrows_follow_decckm() {
        assert_eq!(simple(Key::ArrowUp), b"\x1b[A");
        let app = encode(&KeyStroke::plain(Key::ArrowUp), modes(true));
        assert_eq!(app, b"\x1bOA");
    }

    #[test]
    fn modified_arrows_use_parameters() {
        let stroke = KeyStroke {
            key: Key::ArrowLeft,
            ctrl: true,
            alt: false,
            shift: false,
        };
        // 1 + 4 = 5 for Ctrl.
        assert_eq!(encode(&stroke, modes(false)), b"\x1b[1;5D");
    }

    #[test]
    fn editing_keys_use_tilde_sequences() {
        assert_eq!(simple(Key::Insert), b"\x1b[2~");
        assert_eq!(simple(Key::Delete), b"\x1b[3~");
        assert_eq!(simple(Key::PageUp), b"\x1b[5~");
        assert_eq!(simple(Key::PageDown), b"\x1b[6~");
    }

    #[test]
    fn function_keys() {
        assert_eq!(simple(Key::Function(1)), b"\x1bOP");
        assert_eq!(simple(Key::Function(5)), b"\x1b[15~");
        assert_eq!(simple(Key::Function(12)), b"\x1b[24~");
        assert_eq!(simple(Key::Function(13)), b"");
    }

    #[test]
    fn alt_prefixes_escape() {
        let stroke = KeyStroke {
            key: Key::Char('b'),
            ctrl: false,
            alt: true,
            shift: false,
        };
        assert_eq!(encode(&stroke, modes(false)), b"\x1bb");
    }

    #[test]
    fn shift_tab_is_backtab() {
        let stroke = KeyStroke {
            key: Key::Tab,
            ctrl: false,
            alt: false,
            shift: true,
        };
        assert_eq!(encode(&stroke, modes(false)), b"\x1b[Z");
    }
}
