//! Keybinds on Windows: a key message's side of Shift, Ctrl and Alt.

use super::{VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_SHIFT};

/// The side of a key message's Shift, Ctrl or Alt (its `wParam` is the generic one), from its `lParam`:
/// right Ctrl and Alt are extended keys, right Shift has its own scan code. Any other key as it is.
pub fn sided(key: u32, lparam: isize) -> u32 {
    let extended = (lparam >> 24) & 1 != 0;
    let scancode = (lparam >> 16) & 0xff;
    match key {
        VK_SHIFT => if scancode == 0x36 { VK_RSHIFT } else { VK_LSHIFT },
        VK_CONTROL => if extended { VK_RCONTROL } else { VK_LCONTROL },
        VK_MENU => if extended { VK_RMENU } else { VK_LMENU },
        k => k,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_messages_shift_ctrl_and_alt_get_their_side() {
        // Right Alt (AltGr) and right Ctrl are extended keys.
        assert_eq!(sided(0x12, 1 << 24 | 0x38 << 16), 0xa5);
        assert_eq!(sided(0x12, 0x38 << 16), 0xa4);
        assert_eq!(sided(0x11, 1 << 24 | 0x1d << 16), 0xa3);
        assert_eq!(sided(0x10, 0x36 << 16), 0xa1);
        assert_eq!(sided(0x10, 0x2a << 16), 0xa0);
        // Any other key as it is.
        assert_eq!(sided(0x31, 0x02 << 16), 0x31);
    }
}
