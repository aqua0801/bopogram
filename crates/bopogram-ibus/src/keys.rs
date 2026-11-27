//! X key symbols and modifier masks (what IBus delivers) as the controller's
//! keys, which are described by Windows virtual-key codes. Taiwanese
//! keyboards are US keyboards, so a symbol tells which key was pressed.

use bopogram_controller::{vk, Key};

pub const SHIFT_MASK: u32 = 1 << 0;
pub const LOCK_MASK: u32 = 1 << 1;
pub const CONTROL_MASK: u32 = 1 << 2;
pub const MOD1_MASK: u32 = 1 << 3;
pub const RELEASE_MASK: u32 = 1 << 30;

pub const RETURN: u32 = 0xff0d;

/// A key the controller has no rule for (Tab, F1, ...): it is the
/// application's, or swallowed in the middle of a composition.
const OTHER: u32 = 0x07;

pub fn key(keyval: u32, state: u32) -> Key {
    Key {
        vk: vk_of(keyval),
        shift: state & SHIFT_MASK != 0,
        ctrl: state & CONTROL_MASK != 0,
        alt: state & MOD1_MASK != 0,
        caps: state & LOCK_MASK != 0,
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
pub fn is_release(state: u32) -> bool {
    state & RELEASE_MASK != 0
}

/// The virtual-key code of the key that types this symbol.
fn vk_of(keyval: u32) -> u32 {
    match keyval {
        0x20 => vk::SPACE,
        0x30..=0x39 => keyval,
        0x41..=0x5a => keyval,
        0x61..=0x7a => keyval - 0x20,
        0x21..=0x7e => punctuation(keyval as u8 as char).unwrap_or(OTHER),
        0xff08 => vk::BACK,
        0xff0d | 0xff8d => vk::RETURN,
        0xff1b => vk::ESCAPE,
        0xffff | 0xff9f => vk::DELETE,
        0xff50 | 0xff95 => vk::HOME,
        0xff51 | 0xff96 => vk::LEFT,
        0xff52 | 0xff97 => vk::UP,
        0xff53 | 0xff98 => vk::RIGHT,
        0xff54 | 0xff99 => vk::DOWN,
        0xff55 | 0xff9a => vk::PRIOR,
        0xff56 | 0xff9b => vk::NEXT,
        0xff57 | 0xff9c => vk::END,
        // The keypad with Num Lock on.
        0xffb0..=0xffb9 => vk::NUMPAD0 + (keyval - 0xffb0),
        0xffaa => vk::MULTIPLY,
        0xffab => vk::ADD,
        0xffad => vk::SUBTRACT,
        0xffae => vk::DECIMAL,
        0xffaf => vk::DIVIDE,
        // Modifiers.
        0xffe1 => vk::LSHIFT,
        0xffe2 => vk::LSHIFT + 1,
        0xffe3 | 0xffe4 => vk::CONTROL,
        0xffe5 => vk::CAPITAL,
        0xffe7 | 0xffeb => vk::LWIN,
        0xffe8 | 0xffec => vk::RWIN,
        0xffe9 | 0xffea => vk::MENU,
        0xfe03 => vk::RMENU,
        _ => OTHER,
    }
}

/// The key of an ASCII punctuation character on a US keyboard.
fn punctuation(c: char) -> Option<u32> {
    if let Some(i) = ")!@#$%^&*(".find(c) {
        return Some(0x30 + i as u32);
    }
    const KEYS: [(&str, u32); 11] = [
        (";:", vk::OEM_1),
        ("=+", vk::OEM_PLUS),
        (",<", vk::OEM_COMMA),
        ("-_", vk::OEM_MINUS),
        (".>", vk::OEM_PERIOD),
        ("/?", vk::OEM_2),
        ("`~", vk::OEM_3),
        ("[{", vk::OEM_4),
        ("\\|", vk::OEM_5),
        ("]}", vk::OEM_6),
        ("'\"", vk::OEM_7),
    ];
    KEYS.iter().find(|(chars, _)| chars.contains(c)).map(|&(_, code)| code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_name_their_keys() {
        assert_eq!(key('j' as u32, 0).vk, 'J' as u32);
        assert_eq!(key('J' as u32, SHIFT_MASK).vk, 'J' as u32);
        assert_eq!(key('!' as u32, SHIFT_MASK).vk, '1' as u32);
        assert_eq!(key(')' as u32, SHIFT_MASK).vk, '0' as u32);
        assert_eq!(key(';' as u32, 0).vk, vk::OEM_1);
        assert_eq!(key(':' as u32, SHIFT_MASK).vk, vk::OEM_1);
        assert_eq!(key('"' as u32, SHIFT_MASK).vk, vk::OEM_7);
        assert_eq!(key(0xffb5, 0).vk, vk::NUMPAD0 + 5);
        assert_eq!(key(0xffab, 0).vk, vk::ADD);
        assert_eq!(key(0xff96, 0).vk, vk::LEFT, "keypad 4 without Num Lock");
        assert_eq!(key(0xffbe, 0).vk, OTHER, "F1");
    }

    #[test]
    fn modifiers() {
        let k = key('a' as u32, SHIFT_MASK | CONTROL_MASK | MOD1_MASK | LOCK_MASK);
        assert!(k.shift && k.ctrl && k.alt && k.caps);
        assert!(is_release(RELEASE_MASK | SHIFT_MASK));
        assert_eq!(key(0xffe1, 0).vk, vk::LSHIFT);
    }
}
