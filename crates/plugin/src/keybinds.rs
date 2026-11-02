//! The player's keybinds (the app's settings, through the bridge): volume up, volume down, play/pause.
//!
//! Each press of a bound key, in the game with WestPlay on its screen, is counted for the app, which acts on
//! it. A binding with the interaction key counts only while the player uses the screen (the key held, or
//! toggled on); one without it counts whenever, but not while the player types into WestPlay. While its
//! binding counts, a bound key is kept from the game (the `keyboard` module asks [`taken`]), so the game
//! never acts on it too. A mouse button bound is left to the game: it leaves its side buttons unbound.
//!
//! Keys are Windows' virtual-key codes on every OS (the app's settings hold them): on macOS the `keyboard` module
//! names the Mac's keys so.

use std::sync::atomic::{AtomicBool, Ordering};

use westplay_shared::{KEYBIND_COUNT, Keybind, keybind_of};

use crate::{bridge, input, log, typing};

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::*;

/// Virtual-key codes: the generic Shift, Ctrl and Alt, and each one's sides.
pub const VK_SHIFT: u32 = 0x10;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_MENU: u32 = 0x12;
pub const VK_LSHIFT: u32 = 0xa0;
pub const VK_RSHIFT: u32 = 0xa1;
pub const VK_LCONTROL: u32 = 0xa2;
pub const VK_RCONTROL: u32 = 0xa3;
pub const VK_LMENU: u32 = 0xa4;
pub const VK_RMENU: u32 = 0xa5;
/// The mouse's forward button: the last of the mouse buttons' codes.
const VK_XBUTTON2: u32 = 0x06;

/// Whether each binding's key was down at the last frame.
static WAS_DOWN: [AtomicBool; KEYBIND_COUNT] = [const { AtomicBool::new(false) }; KEYBIND_COUNT];

/// Once a frame: counts each new press of a bound key whose binding counts, for the app.
pub fn on_frame() {
    let Some(bridge) = bridge::get() else { return };
    let in_game = input::game_shown();
    for keybind in Keybind::ALL {
        let i = keybind as usize;
        let Some((key, with_interaction_key)) = keybind_of(bridge.host.keybinds[i].load(Ordering::Acquire)) else {
            WAS_DOWN[i].store(false, Ordering::Release);
            continue;
        };
        let down = input::key_down(key);
        // First: every frame's key state is kept, whether the press counts or not.
        if input::newly_down(&WAS_DOWN[i], down) && in_game && counts(with_interaction_key) {
            bridge.plugin.keybind_presses[i].fetch_add(1, Ordering::AcqRel);
            log!("keybind {keybind:?} pressed");
        }
    }
}

/// Whether a binding counts now: with WestPlay on the screen, and either the screen in use (one with the
/// interaction key) or the player not typing into WestPlay (one without it).
fn counts(with_interaction_key: bool) -> bool {
    bridge::screen_on() && if with_interaction_key { input::interaction_active() } else { !typing::typing() }
}

/// Whether the game is kept from `key` (a virtual-key code; the generic Shift, Ctrl and Alt stand for both
/// sides) now: a keyboard key bound whose binding counts.
pub fn taken(key: u32) -> bool {
    let Some(bridge) = bridge::get() else { return false };
    let sides = sides(key);
    bridge.host.keybinds.iter().filter_map(|value| keybind_of(value.load(Ordering::Acquire))).any(
        |(bound, with_interaction_key)| {
            !is_mouse_button(bound) && sides.contains(&bound) && counts(with_interaction_key)
        },
    )
}

/// The keys `key` stands for: both sides for the generic Shift, Ctrl and Alt, else itself.
fn sides(key: u32) -> [u32; 2] {
    match key {
        VK_SHIFT => [VK_LSHIFT, VK_RSHIFT],
        VK_CONTROL => [VK_LCONTROL, VK_RCONTROL],
        VK_MENU => [VK_LMENU, VK_RMENU],
        k => [k, k],
    }
}

/// Mouse buttons' virtual-key codes: left, right, (cancel,) middle, back, forward.
fn is_mouse_button(key: u32) -> bool {
    (1..=VK_XBUTTON2).contains(&key) && key != 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generic_modifiers_stand_for_both_sides() {
        assert_eq!(sides(0x11), [0xa2, 0xa3]);
        assert_eq!(sides(0x31), [0x31, 0x31]);
    }

    #[test]
    fn tells_mouse_buttons_from_keys() {
        assert!(is_mouse_button(0x05) && is_mouse_button(0x06) && is_mouse_button(0x04));
        assert!(!is_mouse_button(0x03) && !is_mouse_button(0x31) && !is_mouse_button(0xa4));
    }
}
