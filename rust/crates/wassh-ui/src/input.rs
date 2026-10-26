//! Translation of egui events into terminal input.
//!
//! egui reports printable characters as `Event::Text` and non-text keys as `Event::Key`.
//! We forward text verbatim and only interpret keys that carry no text, which avoids
//! double input for ordinary typing.

use wassh_term::{Key, KeyStroke};

/// One unit of terminal input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalInput {
    /// Text to write as UTF-8 (typing, IME commits).
    Text(String),
    /// A non-text key with modifiers.
    Key(KeyStroke),
    /// Clipboard paste; the caller decides whether to wrap it in bracketed-paste markers.
    Paste(String),
}

/// Collect terminal input from this frame's events.
///
/// `paste_pending` says whether a clipboard paste was asked for: the platform's paste
/// shortcut produces the same [`egui::Event::Paste`], and there the key belongs to the
/// shell (`^V` is readline's "literal next").
pub fn collect(ctx: &egui::Context, paste_pending: bool) -> Vec<TerminalInput> {
    ctx.input(|input| {
        let mut out = Vec::new();
        for event in &input.events {
            match event {
                egui::Event::Text(text) => {
                    if !text.is_empty() {
                        out.push(TerminalInput::Text(text.clone()));
                    }
                }
                // egui turns the platform's Ctrl+C / Ctrl+X / Ctrl+V into clipboard
                // events before the terminal sees them. Put the control codes back: a
                // shell needs `^C` to interrupt, and `^X` / `^V` are its own bindings.
                egui::Event::Copy => out.push(TerminalInput::Key(ctrl_char('c'))),
                egui::Event::Cut => out.push(TerminalInput::Key(ctrl_char('x'))),
                egui::Event::Paste(text) => {
                    if paste_pending {
                        out.push(TerminalInput::Paste(text.clone()));
                    } else {
                        out.push(TerminalInput::Key(ctrl_char('v')));
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if let Some(mapped) = map_key(*key, *modifiers) {
                        out.push(TerminalInput::Key(mapped));
                    }
                }
                _ => {}
            }
        }
        out
    })
}

/// `Ctrl+<ch>` as a key stroke, which [`wassh_term::encode`] turns into a control code.
fn ctrl_char(ch: char) -> KeyStroke {
    KeyStroke {
        key: Key::Char(ch),
        ctrl: true,
        alt: false,
        shift: false,
    }
}

fn map_key(key: egui::Key, modifiers: egui::Modifiers) -> Option<KeyStroke> {
    let ctrl = modifiers.ctrl || modifiers.command;
    // A plain character key is delivered through `Event::Text`; only emit it when a
    // modifier is held (Ctrl+C, Alt+B) so the application sees the escape sequence.
    let ch = char_for(key);
    if let Some(ch) = ch {
        if !ctrl && !modifiers.alt {
            return None;
        }
        return Some(KeyStroke {
            key: Key::Char(ch),
            ctrl,
            alt: modifiers.alt,
            shift: modifiers.shift,
        });
    }

    let key = match key {
        egui::Key::Enter => Key::Enter,
        egui::Key::Tab => Key::Tab,
        egui::Key::Backspace => Key::Backspace,
        egui::Key::Escape => Key::Escape,
        egui::Key::Delete => Key::Delete,
        egui::Key::Insert => Key::Insert,
        egui::Key::Home => Key::Home,
        egui::Key::End => Key::End,
        egui::Key::PageUp => Key::PageUp,
        egui::Key::PageDown => Key::PageDown,
        egui::Key::ArrowUp => Key::ArrowUp,
        egui::Key::ArrowDown => Key::ArrowDown,
        egui::Key::ArrowLeft => Key::ArrowLeft,
        egui::Key::ArrowRight => Key::ArrowRight,
        egui::Key::F1 => Key::Function(1),
        egui::Key::F2 => Key::Function(2),
        egui::Key::F3 => Key::Function(3),
        egui::Key::F4 => Key::Function(4),
        egui::Key::F5 => Key::Function(5),
        egui::Key::F6 => Key::Function(6),
        egui::Key::F7 => Key::Function(7),
        egui::Key::F8 => Key::Function(8),
        egui::Key::F9 => Key::Function(9),
        egui::Key::F10 => Key::Function(10),
        egui::Key::F11 => Key::Function(11),
        egui::Key::F12 => Key::Function(12),
        _ => return None,
    };

    Some(KeyStroke {
        key,
        ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
    })
}

/// Lower-case character for keys represented as characters (letters, digits, space).
fn char_for(key: egui::Key) -> Option<char> {
    Some(match key {
        egui::Key::A => 'a',
        egui::Key::B => 'b',
        egui::Key::C => 'c',
        egui::Key::D => 'd',
        egui::Key::E => 'e',
        egui::Key::F => 'f',
        egui::Key::G => 'g',
        egui::Key::H => 'h',
        egui::Key::I => 'i',
        egui::Key::J => 'j',
        egui::Key::K => 'k',
        egui::Key::L => 'l',
        egui::Key::M => 'm',
        egui::Key::N => 'n',
        egui::Key::O => 'o',
        egui::Key::P => 'p',
        egui::Key::Q => 'q',
        egui::Key::R => 'r',
        egui::Key::S => 's',
        egui::Key::T => 't',
        egui::Key::U => 'u',
        egui::Key::V => 'v',
        egui::Key::W => 'w',
        egui::Key::X => 'x',
        egui::Key::Y => 'y',
        egui::Key::Z => 'z',
        egui::Key::Num0 => '0',
        egui::Key::Num1 => '1',
        egui::Key::Num2 => '2',
        egui::Key::Num3 => '3',
        egui::Key::Num4 => '4',
        egui::Key::Num5 => '5',
        egui::Key::Num6 => '6',
        egui::Key::Num7 => '7',
        egui::Key::Num8 => '8',
        egui::Key::Num9 => '9',
        egui::Key::Space => ' ',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_letters_are_not_double_reported() {
        // Text events carry the character; the key event must be ignored.
        assert!(map_key(egui::Key::A, egui::Modifiers::NONE).is_none());
    }

    #[test]
    fn ctrl_letters_are_reported() {
        let stroke = map_key(
            egui::Key::C,
            egui::Modifiers {
                ctrl: true,
                ..egui::Modifiers::NONE
            },
        )
        .expect("ctrl+c maps");
        assert_eq!(stroke.key, Key::Char('c'));
        assert!(stroke.ctrl);
    }

    #[test]
    fn navigation_keys_are_reported_with_modifiers() {
        let stroke = map_key(
            egui::Key::ArrowUp,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        )
        .expect("arrow maps");
        assert_eq!(stroke.key, Key::ArrowUp);
        assert!(stroke.shift);
    }

    #[test]
    fn function_keys_map() {
        assert_eq!(
            map_key(egui::Key::F5, egui::Modifiers::NONE).unwrap().key,
            Key::Function(5)
        );
    }

    /// Collect from `events`, as one headless frame would.
    fn inputs_for(events: Vec<egui::Event>, paste_pending: bool) -> Vec<TerminalInput> {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(640.0, 480.0),
            )),
            ..Default::default()
        };
        raw.events = events;
        let mut collected = Vec::new();
        let output = ctx.run_ui(raw, |ui| {
            collected = collect(ui.ctx(), paste_pending);
        });
        output.drop_without_applying_deltas();
        collected
    }

    #[test]
    fn the_platform_clipboard_shortcuts_still_reach_the_shell() {
        // egui reports Ctrl+C / Ctrl+X / Ctrl+V as clipboard events, never as keys; a
        // shell needs `^C` to interrupt a command, and `^X` / `^V` for its own bindings.
        let inputs = inputs_for(vec![egui::Event::Copy, egui::Event::Cut], false);
        assert_eq!(
            inputs,
            vec![
                TerminalInput::Key(ctrl_char('c')),
                TerminalInput::Key(ctrl_char('x')),
            ]
        );
        assert_eq!(
            inputs_for(vec![egui::Event::Paste("clip".to_owned())], false),
            vec![TerminalInput::Key(ctrl_char('v'))]
        );
    }

    #[test]
    fn a_requested_paste_is_pasted_not_typed_as_ctrl_v() {
        assert_eq!(
            inputs_for(vec![egui::Event::Paste("clip".to_owned())], true),
            vec![TerminalInput::Paste("clip".to_owned())]
        );
    }

    #[test]
    fn control_codes_encode_as_the_shell_expects() {
        let modes = wassh_term::SnapshotModes::default();
        assert_eq!(wassh_term::encode(&ctrl_char('c'), modes), vec![0x03]);
        assert_eq!(wassh_term::encode(&ctrl_char('z'), modes), vec![0x1a]);
        assert_eq!(wassh_term::encode(&ctrl_char('w'), modes), vec![0x17]);
    }
}
