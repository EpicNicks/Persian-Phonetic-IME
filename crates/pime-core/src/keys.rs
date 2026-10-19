//! Windows virtual-key codes (as plain numbers, so this crate needs no Windows
//! types) and the fixed US-QWERTY table used to insert printable keys ourselves
//! (PLAN.md 2.4: under fa-IR the underlying layout is Persian, so pass-through of
//! printable keys would produce Persian-layout characters).

#![allow(missing_docs)]

pub const VK_BACK: u32 = 0x08;
pub const VK_TAB: u32 = 0x09;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_SHIFT: u32 = 0x10;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_MENU: u32 = 0x12;
pub const VK_PAUSE: u32 = 0x13;
pub const VK_CAPITAL: u32 = 0x14;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_SPACE: u32 = 0x20;
pub const VK_PRIOR: u32 = 0x21;
pub const VK_NEXT: u32 = 0x22;
pub const VK_END: u32 = 0x23;
pub const VK_HOME: u32 = 0x24;
pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_DOWN: u32 = 0x28;
pub const VK_INSERT: u32 = 0x2D;
pub const VK_DELETE: u32 = 0x2E;
pub const VK_0: u32 = 0x30;
pub const VK_9: u32 = 0x39;
pub const VK_A: u32 = 0x41;
pub const VK_Z: u32 = 0x5A;
pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;
pub const VK_APPS: u32 = 0x5D;
pub const VK_NUMPAD0: u32 = 0x60;
pub const VK_NUMPAD9: u32 = 0x69;
pub const VK_MULTIPLY: u32 = 0x6A;
pub const VK_ADD: u32 = 0x6B;
pub const VK_SUBTRACT: u32 = 0x6D;
pub const VK_DECIMAL: u32 = 0x6E;
pub const VK_DIVIDE: u32 = 0x6F;
pub const VK_LSHIFT: u32 = 0xA0;
pub const VK_RSHIFT: u32 = 0xA1;
pub const VK_LCONTROL: u32 = 0xA2;
pub const VK_RCONTROL: u32 = 0xA3;
pub const VK_LMENU: u32 = 0xA4;
pub const VK_RMENU: u32 = 0xA5;
pub const VK_OEM_1: u32 = 0xBA;
pub const VK_OEM_PLUS: u32 = 0xBB;
pub const VK_OEM_COMMA: u32 = 0xBC;
pub const VK_OEM_MINUS: u32 = 0xBD;
pub const VK_OEM_PERIOD: u32 = 0xBE;
pub const VK_OEM_2: u32 = 0xBF;
pub const VK_OEM_3: u32 = 0xC0;
pub const VK_OEM_4: u32 = 0xDB;
pub const VK_OEM_5: u32 = 0xDC;
pub const VK_OEM_6: u32 = 0xDD;
pub const VK_OEM_7: u32 = 0xDE;
pub const VK_OEM_102: u32 = 0xE2;

/// True for keys that are only modifiers (pressing them alone never commits).
pub fn is_modifier(vk: u32) -> bool {
    matches!(
        vk,
        VK_SHIFT
            | VK_CONTROL
            | VK_MENU
            | VK_CAPITAL
            | VK_LSHIFT
            | VK_RSHIFT
            | VK_LCONTROL
            | VK_RCONTROL
            | VK_LMENU
            | VK_RMENU
    )
}

/// The character a key produces on a US-QWERTY layout, or `None` for
/// non-printable keys. Letters come out lowercase (uppercase with `shift`).
pub fn us_char(vk: u32, shift: bool) -> Option<char> {
    let pair = |a: char, b: char| Some(if shift { b } else { a });
    match vk {
        VK_A..=VK_Z => {
            let c = (b'a' + (vk - VK_A) as u8) as char;
            Some(if shift { c.to_ascii_uppercase() } else { c })
        }
        VK_0..=VK_9 => {
            let d = (vk - VK_0) as usize;
            if shift {
                Some(b")!@#$%^&*("[d] as char)
            } else {
                Some((b'0' + d as u8) as char)
            }
        }
        VK_SPACE => Some(' '),
        VK_NUMPAD0..=VK_NUMPAD9 => Some((b'0' + (vk - VK_NUMPAD0) as u8) as char),
        VK_MULTIPLY => Some('*'),
        VK_ADD => Some('+'),
        VK_SUBTRACT => Some('-'),
        VK_DECIMAL => Some('.'),
        VK_DIVIDE => Some('/'),
        VK_OEM_1 => pair(';', ':'),
        VK_OEM_PLUS => pair('=', '+'),
        VK_OEM_COMMA => pair(',', '<'),
        VK_OEM_MINUS => pair('-', '_'),
        VK_OEM_PERIOD => pair('.', '>'),
        VK_OEM_2 => pair('/', '?'),
        VK_OEM_3 => pair('`', '~'),
        VK_OEM_4 => pair('[', '{'),
        VK_OEM_5 | VK_OEM_102 => pair('\\', '|'),
        VK_OEM_6 => pair(']', '}'),
        VK_OEM_7 => pair('\'', '"'),
        _ => None,
    }
}

/// Inverse of [`us_char`] for main-block keys: the (vk, shift) that types `c`
/// on US-QWERTY. Used by tests and harnesses to script key sequences.
pub fn vk_for_char(c: char) -> Option<(u32, bool)> {
    const CANDIDATES: &[u32] = &[
        VK_OEM_1,
        VK_OEM_PLUS,
        VK_OEM_COMMA,
        VK_OEM_MINUS,
        VK_OEM_PERIOD,
        VK_OEM_2,
        VK_OEM_3,
        VK_OEM_4,
        VK_OEM_5,
        VK_OEM_6,
        VK_OEM_7,
        VK_SPACE,
    ];
    let main = (VK_A..=VK_Z)
        .chain(VK_0..=VK_9)
        .chain(CANDIDATES.iter().copied());
    for vk in main {
        for shift in [false, true] {
            if us_char(vk, shift) == Some(c) {
                return Some((vk, shift));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_symbols() {
        assert_eq!(us_char(VK_A, false), Some('a'));
        assert_eq!(us_char(VK_Z, true), Some('Z'));
        assert_eq!(us_char(VK_0 + 1, true), Some('!'));
        assert_eq!(us_char(VK_0, true), Some(')'));
        assert_eq!(us_char(VK_OEM_7, false), Some('\''));
        assert_eq!(us_char(VK_OEM_2, true), Some('?'));
        assert_eq!(us_char(VK_LEFT, false), None);
    }

    #[test]
    fn inverse_round_trips_every_printable_ascii() {
        for b in 0x20u8..0x7F {
            let c = b as char;
            let (vk, shift) = vk_for_char(c).unwrap_or_else(|| panic!("no key for {c:?}"));
            assert_eq!(us_char(vk, shift), Some(c));
        }
    }
}
