//! Keyboard -> PTY byte translation (subset of xterm/alacritty behavior).

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Effect-facing classification of a key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Char,
    Backspace,
    Enter,
    Arrow,
    Other,
}

pub struct DecodedKey {
    pub bytes: Vec<u8>,
    pub kind: KeyKind,
    pub ch: Option<char>,
}

/// Translate a winit key event to bytes for the PTY.
/// `app_cursor` toggles CSI/SS3 arrow sequences (DECCKM).
pub fn decode_key(ev: &KeyEvent, mods: ModifiersState, app_cursor: bool) -> Option<DecodedKey> {
    let ctrl = mods.control_key();
    let shift = mods.shift_key();

    let named = |bytes: &'static [u8], kind: KeyKind, ch: Option<char>| {
        Some(DecodedKey {
            bytes: bytes.to_vec(),
            kind,
            ch,
        })
    };

    let arrows = |n3: &'static [u8], ss3: &'static [u8]| if app_cursor { ss3 } else { n3 };

    match &ev.logical_key {
        Key::Named(named_key) => match named_key {
            NamedKey::Enter => named(b"\r", KeyKind::Enter, Some('\r')),
            NamedKey::Backspace => named(b"\x7f", KeyKind::Backspace, None),
            NamedKey::Tab if shift => named(b"\x1b[Z", KeyKind::Other, None),
            NamedKey::Tab => named(b"\t", KeyKind::Other, None),
            NamedKey::Escape => named(b"\x1b", KeyKind::Other, None),
            NamedKey::Space => named(b" ", KeyKind::Char, Some(' ')),
            NamedKey::Delete => named(b"\x1b[3~", KeyKind::Other, None),
            NamedKey::Insert => named(b"\x1b[2~", KeyKind::Other, None),
            NamedKey::Home => named(arrows(b"\x1b[H", b"\x1bOH"), KeyKind::Other, None),
            NamedKey::End => named(arrows(b"\x1b[F", b"\x1bOF"), KeyKind::Other, None),
            NamedKey::PageUp => named(b"\x1b[5~", KeyKind::Other, None),
            NamedKey::PageDown => named(b"\x1b[6~", KeyKind::Other, None),
            NamedKey::ArrowUp => named(arrows(b"\x1b[A", b"\x1bOA"), KeyKind::Arrow, None),
            NamedKey::ArrowDown => named(arrows(b"\x1b[B", b"\x1bOB"), KeyKind::Arrow, None),
            NamedKey::ArrowRight => named(arrows(b"\x1b[C", b"\x1bOC"), KeyKind::Arrow, None),
            NamedKey::ArrowLeft => named(arrows(b"\x1b[D", b"\x1bOD"), KeyKind::Arrow, None),
            NamedKey::F1 => named(b"\x1bOP", KeyKind::Other, None),
            NamedKey::F2 => named(b"\x1bOQ", KeyKind::Other, None),
            NamedKey::F3 => named(b"\x1bOR", KeyKind::Other, None),
            NamedKey::F4 => named(b"\x1bOS", KeyKind::Other, None),
            NamedKey::F5 => named(b"\x1b[15~", KeyKind::Other, None),
            NamedKey::F6 => named(b"\x1b[17~", KeyKind::Other, None),
            NamedKey::F7 => named(b"\x1b[18~", KeyKind::Other, None),
            NamedKey::F8 => named(b"\x1b[19~", KeyKind::Other, None),
            NamedKey::F9 => named(b"\x1b[20~", KeyKind::Other, None),
            NamedKey::F10 => named(b"\x1b[21~", KeyKind::Other, None),
            NamedKey::F11 => named(b"\x1b[23~", KeyKind::Other, None),
            NamedKey::F12 => named(b"\x1b[24~", KeyKind::Other, None),
            _ => None,
        },
        _ => {
            // Control combos produce control bytes.
            if ctrl {
                if let Key::Character(s) = &ev.logical_key {
                    let mut out = Vec::with_capacity(s.len());
                    for ch in s.chars() {
                        let ch = ch.to_ascii_lowercase();
                        match ch {
                            'a'..='z' => out.push(ch as u8 - b'a' + 1),
                            '[' | '\\' | ']' | '^' | '_' => out.push(ch as u8 - b'[' + 0x1b),
                            ' ' | '2' | '@' => out.push(0),
                            '3'..='7' => out.push(ch as u8 - b'3' + 0x1b),
                            '8' | '?' => out.push(0x7f),
                            _ => {}
                        }
                    }
                    if !out.is_empty() {
                        return Some(DecodedKey {
                            bytes: out,
                            kind: KeyKind::Other,
                            ch: None,
                        });
                    }
                }
                return None;
            }
            ev.text.as_ref().filter(|t| !t.is_empty()).map(|t| {
                let ch = t.chars().next();
                DecodedKey {
                    bytes: t.as_str().as_bytes().to_vec(),
                    kind: KeyKind::Char,
                    ch,
                }
            })
        }
    }
}
