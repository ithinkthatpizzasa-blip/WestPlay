//! Typing on Windows: what a key types in the player's Windows keyboard layout.

use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardState, ToUnicodeEx, VK_CONTROL, VK_MENU};

use super::{key_meaning, queue};

/// The keys that stay the game's ([`super::stays_the_games`]) as DirectInput names them (scan codes, extended
/// ones with 0x80): F1-F15, Print Screen (SysRq), the Windows and menu keys.
pub fn stays_the_games_scancode(scancode: u32) -> bool {
    matches!(scancode, 0x3b..=0x44 | 0x57 | 0x58 | 0x64..=0x66 | 0xb7 | 0xdb..=0xdd)
}

/// A key pressed while typing (`key`, its scan code): queued for the app as what it types.
///
/// # Safety
/// On the game's window thread (the keyboard state read is that thread's).
pub unsafe fn press(key: u32, scancode: u32) {
    // SAFETY: on the thread whose keyboard state is asked (the caller's contract).
    queue(unsafe { typed(key, scancode) });
}

/// What a key press types: a key with its own meaning ([`key_meaning`]), or the characters the player's
/// keyboard layout gives (none for an accent key, whose accent waits for the next letter).
///
/// # Safety
/// As [`press`].
unsafe fn typed(key: u32, scancode: u32) -> Vec<u32> {
    // SAFETY: plain Windows calls with locals; the keyboard state is this thread's (the caller's contract).
    unsafe {
        let mut state = [0u8; 256];
        if GetKeyboardState(&mut state).is_err() {
            return Vec::new();
        }
        let down = |vk: u16| state[vk as usize] & 0x80 != 0;
        // Ctrl alone (Ctrl with Alt is AltGr, which types characters).
        let ctrl = down(VK_CONTROL.0) && !down(VK_MENU.0);
        if let Some(meaning) = key_meaning(key, ctrl, ctrl) {
            return meaning.map(|key| key as u32).into_iter().collect();
        }
        let mut buffer = [0u16; 8];
        let layout = GetKeyboardLayout(0);
        let count = ToUnicodeEx(key, scancode, &state, &mut buffer, 0, Some(layout));
        if count <= 0 {
            return Vec::new();
        }
        String::from_utf16_lossy(&buffer[..count as usize]).chars().filter(|c| !c.is_control()).map(u32::from).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_games_keys_by_their_scan_codes() {
        // F5 and the left Windows key as DirectInput names them; W (0x11) not.
        assert!(stays_the_games_scancode(0x3f) && stays_the_games_scancode(0xdb));
        assert!(!stays_the_games_scancode(0x11));
    }
}
