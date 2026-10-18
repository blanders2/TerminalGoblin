//! Translating keyboard and mouse input into the byte sequences terminal programs expect.

use alacritty_terminal::term::TermMode;
use winit::keyboard::{Key, ModifiersState, NamedKey};

fn modifier_param(mods: ModifiersState) -> u8 {
    1 + mods.shift_key() as u8 + 2 * mods.alt_key() as u8 + 4 * mods.control_key() as u8
}

/// Kitty keyboard protocol encoding, used once a program enables it (fish 4, Claude Code, ...).
/// Returns `None` for keys whose legacy encoding is also valid under the protocol.
/// See <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>.
fn encode_kitty(unmodified: &Key, text: Option<&str>, mods: ModifiersState, mode: TermMode) -> Option<Vec<u8>> {
    let all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let m = modifier_param(mods) + 8 * mods.super_key() as u8;
    let code = match unmodified {
        Key::Named(NamedKey::Escape) => 27,
        Key::Named(NamedKey::Enter) => 13,
        Key::Named(NamedKey::Tab) => 9,
        Key::Named(NamedKey::Backspace) => 127,
        Key::Named(NamedKey::Space) => 32,
        Key::Character(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c.to_lowercase().next().unwrap_or(c) as u32,
                _ => return None,
            }
        }
        // Cursor, function and editing keys keep their legacy CSI forms.
        _ => return None,
    };
    if !all {
        match code {
            27 => {}
            // Kept legacy when unmodified so a shell stays usable if a program dies with the mode on.
            13 | 9 | 127 if m == 1 => return None,
            13 | 9 | 127 => {}
            _ if !(mods.control_key() || mods.alt_key() || mods.super_key()) => return None,
            _ => {}
        }
    }
    let associated: Option<String> = (all && mode.contains(TermMode::REPORT_ASSOCIATED_TEXT))
        .then(|| text.filter(|t| t.chars().all(|c| !c.is_control())))
        .flatten()
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().map(|c| (c as u32).to_string()).collect::<Vec<_>>().join(":"));
    let mut out = format!("\x1b[{code}");
    if m > 1 || associated.is_some() {
        out.push_str(&format!(";{m}"));
    }
    if let Some(a) = associated {
        out.push(';');
        out.push_str(&a);
    }
    out.push('u');
    Some(out.into_bytes())
}

/// Encode a key press. `unmodified` is the key without modifiers applied, used for Ctrl combos.
pub fn encode_key(
    key: &Key,
    unmodified: &Key,
    text: Option<&str>,
    mods: ModifiersState,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if mode.intersects(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC)
        && let Some(bytes) = encode_kitty(unmodified, text, mods, mode)
    {
        return Some(bytes);
    }
    let m = modifier_param(mods);
    let has_mods = m > 1;
    let alt = mods.alt_key();
    let with_alt = |mut v: Vec<u8>| {
        if alt {
            v.insert(0, 0x1b);
        }
        v
    };
    let csi_letter = |c: char, ss3_when_app: bool| -> Vec<u8> {
        if has_mods {
            format!("\x1b[1;{m}{c}").into_bytes()
        } else if ss3_when_app {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if has_mods {
            format!("\x1b[{n};{m}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let app_cursor = mode.contains(TermMode::APP_CURSOR);

    if let Key::Named(named) = key {
        let bytes = match named {
            NamedKey::Enter => with_alt(b"\r".to_vec()),
            NamedKey::Tab if mods.shift_key() => b"\x1b[Z".to_vec(),
            NamedKey::Tab => with_alt(b"\t".to_vec()),
            NamedKey::Backspace if mods.control_key() => with_alt(vec![0x08]),
            NamedKey::Backspace => with_alt(vec![0x7f]),
            NamedKey::Escape => with_alt(vec![0x1b]),
            NamedKey::Space if mods.control_key() => vec![0x00],
            NamedKey::Space => with_alt(b" ".to_vec()),
            NamedKey::ArrowUp => csi_letter('A', app_cursor),
            NamedKey::ArrowDown => csi_letter('B', app_cursor),
            NamedKey::ArrowRight => csi_letter('C', app_cursor),
            NamedKey::ArrowLeft => csi_letter('D', app_cursor),
            NamedKey::Home => csi_letter('H', app_cursor),
            NamedKey::End => csi_letter('F', app_cursor),
            NamedKey::Insert => tilde(2),
            NamedKey::Delete => tilde(3),
            NamedKey::PageUp => tilde(5),
            NamedKey::PageDown => tilde(6),
            NamedKey::F1 => csi_letter('P', true),
            NamedKey::F2 => csi_letter('Q', true),
            NamedKey::F3 => csi_letter('R', true),
            NamedKey::F4 => csi_letter('S', true),
            NamedKey::F5 => tilde(15),
            NamedKey::F6 => tilde(17),
            NamedKey::F7 => tilde(18),
            NamedKey::F8 => tilde(19),
            NamedKey::F9 => tilde(20),
            NamedKey::F10 => tilde(21),
            NamedKey::F11 => tilde(23),
            NamedKey::F12 => tilde(24),
            _ => {
                return text.filter(|t| !t.is_empty()).map(|t| with_alt(t.as_bytes().to_vec()));
            }
        };
        return Some(bytes);
    }

    if mods.control_key()
        && let Key::Character(s) = unmodified
    {
        let mut chars = s.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            let code = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
                '@' | '2' | ' ' => Some(0),
                '[' | '3' => Some(0x1b),
                '\\' | '4' => Some(0x1c),
                ']' | '5' => Some(0x1d),
                '^' | '6' => Some(0x1e),
                '_' | '-' | '7' | '/' => Some(0x1f),
                '8' | '?' => Some(0x7f),
                _ => None,
            };
            if let Some(code) = code {
                return Some(with_alt(vec![code]));
            }
        }
    }

    let text = text.filter(|t| !t.is_empty())?;
    // winit already applies Ctrl to `text`; strip control chars we didn't map above.
    if mods.control_key() && text.chars().all(|c| c.is_control()) {
        return None;
    }
    Some(with_alt(text.as_bytes().to_vec()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButtonCode {
    Left = 0,
    Middle = 1,
    Right = 2,
    WheelUp = 64,
    WheelDown = 65,
}

/// Encode a mouse report for the program running in the terminal. `col`/`line` are 0-based.
pub fn mouse_report(
    button: MouseButtonCode,
    pressed: bool,
    motion: bool,
    col: usize,
    line: usize,
    mods: ModifiersState,
    mode: TermMode,
) -> Vec<u8> {
    let mut code = button as u8;
    if motion {
        code += 32;
    }
    if mods.shift_key() {
        code += 4;
    }
    if mods.alt_key() {
        code += 8;
    }
    if mods.control_key() {
        code += 16;
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if pressed || motion { 'M' } else { 'm' };
        return format!("\x1b[<{code};{};{}{suffix}", col + 1, line + 1).into_bytes();
    }
    if !pressed && !motion && !matches!(button, MouseButtonCode::WheelUp | MouseButtonCode::WheelDown) {
        code = 3 + (code & !3);
    }
    let mut out = b"\x1b[M".to_vec();
    out.push(32 + code);
    if mode.contains(TermMode::UTF8_MOUSE) {
        for v in [col, line] {
            let c = char::from_u32((v + 33).min(2047) as u32).unwrap_or(' ');
            out.extend(c.encode_utf8(&mut [0; 4]).as_bytes());
        }
    } else {
        out.push((32 + 1 + col.min(222)) as u8);
        out.push((32 + 1 + line.min(222)) as u8);
    }
    out
}

/// Prepare clipboard text for the terminal, honouring bracketed paste mode.
pub fn paste_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let clean = text.replace(['\x1b', '\x03'], "");
        let mut out = b"\x1b[200~".to_vec();
        out.extend(clean.as_bytes());
        out.extend(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn ch(c: &str) -> Key {
        Key::Character(SmolStr::new(c))
    }

    #[test]
    fn control_and_alt() {
        let ctrl = ModifiersState::CONTROL;
        assert_eq!(
            encode_key(&ch("c"), &ch("c"), Some("\x03"), ctrl, TermMode::empty()),
            Some(vec![3])
        );
        assert_eq!(
            encode_key(&ch("["), &ch("["), None, ctrl, TermMode::empty()),
            Some(vec![0x1b])
        );
        assert_eq!(
            encode_key(&ch("b"), &ch("b"), Some("b"), ModifiersState::ALT, TermMode::empty()),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            encode_key(&ch("A"), &ch("a"), Some("A"), ModifiersState::SHIFT, TermMode::empty()),
            Some(b"A".to_vec())
        );
    }

    #[test]
    fn cursor_keys() {
        let up = Key::Named(NamedKey::ArrowUp);
        assert_eq!(
            encode_key(&up, &up, None, ModifiersState::empty(), TermMode::empty()),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(&up, &up, None, ModifiersState::empty(), TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key(&up, &up, None, ModifiersState::CONTROL, TermMode::empty()),
            Some(b"\x1b[1;5A".to_vec())
        );
        let del = Key::Named(NamedKey::Delete);
        assert_eq!(
            encode_key(&del, &del, None, ModifiersState::SHIFT, TermMode::empty()),
            Some(b"\x1b[3;2~".to_vec())
        );
        let tab = Key::Named(NamedKey::Tab);
        assert_eq!(
            encode_key(&tab, &tab, None, ModifiersState::SHIFT, TermMode::empty()),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn kitty_disambiguate() {
        let k = TermMode::DISAMBIGUATE_ESC_CODES;
        let enter = Key::Named(NamedKey::Enter);
        let esc = Key::Named(NamedKey::Escape);
        let none = ModifiersState::empty();
        let enc = |key: &Key, unmod: &Key, text: Option<&str>, mods| encode_key(key, unmod, text, mods, k);
        assert_eq!(
            enc(&enter, &enter, Some("\r"), none),
            Some(b"\r".to_vec()),
            "plain Enter stays legacy"
        );
        assert_eq!(
            enc(&enter, &enter, Some("\r"), ModifiersState::SHIFT),
            Some(b"\x1b[13;2u".to_vec())
        );
        assert_eq!(enc(&esc, &esc, None, none), Some(b"\x1b[27u".to_vec()));
        assert_eq!(
            enc(&ch("c"), &ch("c"), Some("c"), ModifiersState::CONTROL),
            Some(b"\x1b[99;5u".to_vec())
        );
        let alt_shift = ModifiersState::ALT | ModifiersState::SHIFT;
        assert_eq!(
            enc(&ch("A"), &ch("a"), Some("A"), alt_shift),
            Some(b"\x1b[97;4u".to_vec())
        );
        assert_eq!(
            enc(&ch("A"), &ch("a"), Some("A"), ModifiersState::SHIFT),
            Some(b"A".to_vec()),
            "text stays text"
        );
        let up = Key::Named(NamedKey::ArrowUp);
        assert_eq!(
            enc(&up, &up, None, ModifiersState::CONTROL),
            Some(b"\x1b[1;5A".to_vec())
        );
    }

    #[test]
    fn kitty_report_all_keys() {
        let k = TermMode::REPORT_ALL_KEYS_AS_ESC | TermMode::REPORT_ASSOCIATED_TEXT;
        let none = ModifiersState::empty();
        assert_eq!(
            encode_key(&ch("a"), &ch("a"), Some("a"), none, k),
            Some(b"\x1b[97;1;97u".to_vec())
        );
        let enter = Key::Named(NamedKey::Enter);
        assert_eq!(
            encode_key(&enter, &enter, Some("\r"), none, k),
            Some(b"\x1b[13u".to_vec())
        );
    }

    #[test]
    fn mouse_sgr_and_legacy() {
        let m = ModifiersState::empty();
        assert_eq!(
            mouse_report(MouseButtonCode::Left, true, false, 4, 9, m, TermMode::SGR_MOUSE),
            b"\x1b[<0;5;10M"
        );
        assert_eq!(
            mouse_report(MouseButtonCode::Left, false, false, 4, 9, m, TermMode::SGR_MOUSE),
            b"\x1b[<0;5;10m"
        );
        assert_eq!(
            mouse_report(MouseButtonCode::Left, false, false, 0, 0, m, TermMode::empty()),
            b"\x1b[M#!!"
        );
    }

    #[test]
    fn paste_modes() {
        assert_eq!(paste_bytes("a\nb", TermMode::empty()), b"a\rb");
        assert_eq!(
            paste_bytes("x\x1by", TermMode::BRACKETED_PASTE),
            b"\x1b[200~xy\x1b[201~"
        );
    }
}
