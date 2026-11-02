//! The keyboard on macOS: the key events the game takes from its own event loop (`input/macos.rs`).
//!
//! A key kept from the game (`keyboard`) has its events held back there, before the game reads them. Its modifiers
//! (Shift, Control, Option, Command: `flagsChanged`) always reach it, but a press of one bound to WestPlay while its
//! binding counts: the game also asks for them itself (`CGEventSourceKeyState`), and a key that types needs them.
//!
//! Keys are named by Windows' virtual-key codes, as the app's settings hold them: as WebKit names a Mac's keys to
//! the companion, where the player chose them (letters, digits and punctuation by what the layout puts on them,
//! the rest by their place on the keyboard).

use std::ffi::{CStr, c_char};

use objc2::msg_send;
use objc2::runtime::AnyObject;

use super::hidden;
use crate::{input, keybinds, typing};

/// NSEventModifierFlagCommand.
const COMMAND: usize = 1 << 20;
/// NSEventModifierFlags' device-dependent bits for each modifier key, by its key code.
const MODIFIER_BITS: [(u16, usize); 8] =
    [(56, 0x2), (60, 0x4), (59, 0x1), (62, 0x2000), (58, 0x20), (61, 0x40), (55, 0x8), (54, 0x10)];

/// Whether a key event (`flags_changed`: a modifier's; else `down`: a press, or a release) goes on to the game.
///
/// # Safety
/// A live key event, on the main thread.
pub unsafe fn for_the_game(event: &AnyObject, flags_changed: bool, down: bool) -> bool {
    // SAFETY: a key event (the caller's contract), asked for its key and, a key's (not a modifier's), its characters.
    unsafe {
        let code: u16 = msg_send![event, keyCode];
        let flags: usize = msg_send![event, modifierFlags];
        if flags_changed {
            let Some(key) = special_key(code) else { return true };
            // A press when its own bit is set.
            let pressed = MODIFIER_BITS.iter().find(|(c, _)| *c == code).is_some_and(|(_, bit)| flags & bit != 0);
            return !(pressed && keybinds::taken(key));
        }
        let characters: *mut AnyObject = msg_send![event, charactersIgnoringModifiers];
        let key = key_of(code, first_character(characters));
        input::set_key_down(key, down);
        // Command's shortcuts stay the game's (and the Mac's: Quit, Hide), but pasting and taking text back.
        let command = flags & COMMAND != 0;
        let games_shortcut = command && key != u32::from(b'V') && key != 0x08;
        let typed = typing::typing() && !games_shortcut && !typing::stays_the_games(key);
        if typed {
            if down {
                typing::press(event, code, flags, key);
            }
            return false;
        }
        // A key bound to WestPlay: its presses aren't the game's (a release it gets is harmless).
        !(down && !games_shortcut && hidden(key))
    }
}

/// The first character of an NSString (none for none, or an empty one).
///
/// # Safety
/// `string` is null or a live NSString.
unsafe fn first_character(string: *mut AnyObject) -> Option<char> {
    // SAFETY: a live NSString (the caller's contract), its UTF-8 alive while it is.
    unsafe {
        let string = string.as_ref()?;
        let utf8: *const c_char = msg_send![string, UTF8String];
        if utf8.is_null() {
            return None;
        }
        CStr::from_ptr(utf8).to_str().ok()?.chars().next()
    }
}

/// A Mac key's virtual-key code: one without characters of its own by its key code; else by the character the
/// layout gives it (`character`, without modifiers), as WebKit does; else by its place on a US keyboard.
pub fn key_of(code: u16, character: Option<char>) -> u32 {
    if let Some(key) = special_key(code) {
        return key;
    }
    if let Some(key) = character.and_then(character_key) {
        return key;
    }
    us_key(code).map_or(0, u32::from)
}

/// The virtual-key code of a character a key types: letters and digits as themselves, punctuation as Windows'
/// `VK_OEM_` keys.
fn character_key(character: char) -> Option<u32> {
    let upper = character.to_ascii_uppercase();
    Some(match upper {
        'A'..='Z' | '0'..='9' => u32::from(upper),
        ';' | ':' => 0xba,
        '=' | '+' => 0xbb,
        ',' | '<' => 0xbc,
        '-' | '_' => 0xbd,
        '.' | '>' => 0xbe,
        '/' | '?' => 0xbf,
        '`' | '~' => 0xc0,
        '[' | '{' => 0xdb,
        '\\' | '|' => 0xdc,
        ']' | '}' => 0xdd,
        '\'' | '"' => 0xde,
        _ => return None,
    })
}

/// Keys without characters of their own (and the keypad's, named by their place), by their Mac key code.
fn special_key(code: u16) -> Option<u32> {
    Some(match code {
        36 | 76 => 0x0d,                        // Return, keypad Enter
        48 => 0x09,                             // Tab
        49 => 0x20,                             // Space
        51 => 0x08,                             // Delete (backspace)
        53 => 0x1b,                             // Escape
        54 => 0x5c,                             // right Command
        55 => 0x5b,                             // left Command
        56 => 0xa0,                             // left Shift
        57 => 0x14,                             // Caps Lock
        58 => 0xa4,                             // left Option
        59 => 0xa2,                             // left Control
        60 => 0xa1,                             // right Shift
        61 => 0xa5,                             // right Option
        62 => 0xa3,                             // right Control
        65 => 0x6e,                             // keypad .
        67 => 0x6a,                             // keypad *
        69 => 0x6b,                             // keypad +
        71 => 0x0c,                             // keypad Clear
        72 => 0xaf,                             // volume up
        73 => 0xae,                             // volume down
        74 => 0xad,                             // mute
        75 => 0x6f,                             // keypad /
        78 => 0x6d,                             // keypad -
        81 => 0xbb,                             // keypad =
        82..=89 => 0x60 + u32::from(code - 82), // keypad 0-7
        91 => 0x68,                             // keypad 8
        92 => 0x69,                             // keypad 9
        114 => 0x2d,                            // Help (Insert)
        115 => 0x24,                            // Home
        116 => 0x21,                            // Page Up
        117 => 0x2e,                            // forward Delete
        119 => 0x23,                            // End
        121 => 0x22,                            // Page Down
        123 => 0x25,                            // left
        124 => 0x27,                            // right
        125 => 0x28,                            // down
        126 => 0x26,                            // up
        _ => 0x6f + function_key(code)?,
    })
}

/// The number of a function key (F1-F20), by its Mac key code.
fn function_key(code: u16) -> Option<u32> {
    const CODES: [u16; 20] =
        [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80, 90];
    CODES.iter().position(|c| *c == code).map(|i| i as u32 + 1)
}

/// A key's character on a US keyboard, by its Mac key code.
fn us_key(code: u16) -> Option<u8> {
    const KEYS: &[u8; 51] = b"ASDFHGZXCV\0BQWERYT123465=97-80]OU[IP\0LJ'K;\\,/NM.\0\0`";
    let key = *KEYS.get(usize::from(code))?;
    match key {
        0 => None,
        b'=' | b'-' | b']' | b'[' | b'\'' | b';' | b'\\' | b',' | b'/' | b'.' | b'`' => {
            character_key(char::from(key)).map(|k| k as u8)
        }
        _ => Some(key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_named_as_the_companion_names_them() {
        // Option, the default interaction key, and its side.
        assert_eq!(key_of(58, None), 0xa4);
        assert_eq!(key_of(61, None), 0xa5);
        // Letters by what the layout types: on a French keyboard the key in Q's place types A.
        assert_eq!(key_of(12, Some('a')), u32::from(b'A'));
        assert_eq!(key_of(12, Some('q')), u32::from(b'Q'));
        // A letter the layout gives no Latin letter for: by its US place.
        assert_eq!(key_of(12, Some('й')), u32::from(b'Q'));
        assert_eq!(key_of(24, None), 0xbb);
        // Function keys, the keypad, arrows.
        assert_eq!(key_of(122, None), 0x70);
        assert_eq!(key_of(111, None), 0x7b);
        assert_eq!(key_of(90, None), 0x83);
        assert_eq!(key_of(82, Some('0')), 0x60);
        assert_eq!(key_of(92, Some('9')), 0x69);
        assert_eq!(key_of(126, None), 0x26);
        assert_eq!(key_of(51, None), 0x08);
    }
}
