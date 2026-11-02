//! Screen interaction: the mouse.
//!
//! While the player holds the interaction key, the game's mouse is taken away from it: it sees no mouse movement
//! or buttons, so the head view stays on the screen (but a button it saw go down still comes up for it: none
//! stays held). The mouse goes to the app through the bridge's pointer block instead, which drives the WestPlay
//! screen with it. (The keyboard is the `keyboard` module's.)
//!
//! How the mouse is taken from the game is each OS's: DirectInput's hooks on Windows (`input/windows.rs`), the
//! game's own event loop on macOS (`input/macos.rs`). When the screen is used, and the interaction key, are
//! shared.

use std::sync::atomic::{AtomicBool, Ordering};

use westplay_shared::{DEFAULT_INTERACTION_KEY, PointerBlock};

use crate::{bridge, log};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(windows)]
pub use windows::*;

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether the player is using the screen: WestPlay is on it and the interaction key (the player's choice,
/// through the bridge) is held, or toggled on. Publishes changes to the app. Without WestPlay on the screen,
/// the key takes nothing from the game.
pub fn interaction_active() -> bool {
    // Never while the game is paused: its menu and its map are used with the mouse (owner 2026-10-21: toggled
    // on, the menu couldn't be used).
    let active = bridge::screen_on() && !crate::telemetry::paused() && interaction_key_held();
    if ACTIVE.swap(active, Ordering::AcqRel) != active {
        log!("screen interaction {}", if active { "on" } else { "off" });
        if !active {
            interaction_ended();
        }
        if let Some(pointer) = pointer_block() {
            if !active {
                pointer.buttons.store(0, Ordering::Release);
            }
            pointer.active.store(active as u32, Ordering::Release);
        }
    }
    active
}

/// Whether the interaction key says the screen is used, whether or not WestPlay is on it: held, or (the
/// player's choice) toggled on.
fn interaction_key_held() -> bool {
    if toggle_mode() { TOGGLED.load(Ordering::Acquire) } else { interaction_key_down() }
}

/// Whether the interaction key is down.
fn interaction_key_down() -> bool {
    let key = match bridge::get().map(|bridge| bridge.host.interaction_key.load(Ordering::Acquire)) {
        Some(key) if key != 0 => key,
        _ => DEFAULT_INTERACTION_KEY,
    };
    key_down(key)
}

/// Whether the interaction key is pressed once to use the screen and again to stop, rather than held.
fn toggle_mode() -> bool {
    bridge::get().is_some_and(|bridge| bridge.host.interaction_toggle.load(Ordering::Acquire) == 1)
}

/// In toggle mode: whether the player has turned using the screen on.
static TOGGLED: AtomicBool = AtomicBool::new(false);
/// Whether the interaction key was down at the last frame.
static KEY_WAS_DOWN: AtomicBool = AtomicBool::new(false);

/// Once a frame: in toggle mode, each press of the interaction key turns using the screen on or off. It's
/// off whenever WestPlay isn't on the screen, and as the game pauses.
pub fn on_frame() {
    let pressed = newly_down(&KEY_WAS_DOWN, interaction_key_down());
    let off = if !toggle_mode() {
        Some("held mode")
    } else if !bridge::screen_on() {
        Some("WestPlay not on the screen")
    } else if crate::telemetry::paused() {
        // Its menu wants the mouse; back in the game, a press uses the screen again.
        Some("the game paused")
    } else {
        None
    };
    if let Some(why) = off {
        if TOGGLED.swap(false, Ordering::AcqRel) {
            log!("screen use toggled off: {why}");
        }
        return;
    }
    if pressed {
        TOGGLED.fetch_xor(true, Ordering::AcqRel);
    }
}

/// Whether a key is down now and wasn't at the last look (`was_down`, which takes `down` for the next). The
/// last look is kept whatever it was: kept only while the key was down (`down && !was_down.swap(down)`, the
/// swap skipped while it's up), the first release was never seen, and no press counted after the first
/// (owner 2026-10-21: toggled on, the key couldn't toggle off).
pub fn newly_down(was_down: &AtomicBool, down: bool) -> bool {
    let before = was_down.swap(down, Ordering::AcqRel);
    down && !before
}

/// Whether a mouse button's press or release (`index`: 0 left, 1 right, 2 middle…) goes to the game, keeping
/// track in `held` of the buttons it has seen go down: all of them while the player isn't using the screen;
/// while they are, only the release of a button it holds.
fn game_gets_button(index: u32, pressed: bool, interacting: bool, held: &mut u8) -> bool {
    let bit = 1 << index;
    if !interacting {
        if pressed {
            *held |= bit
        } else {
            *held &= !bit
        }
        true
    } else if !pressed && *held & bit != 0 {
        *held &= !bit;
        true
    } else {
        false
    }
}

fn take_mouse(dx: i32, dy: i32, wheel: i32) {
    if let Some(pointer) = pointer_block() {
        pointer.dx.fetch_add(dx, Ordering::AcqRel);
        pointer.dy.fetch_add(dy, Ordering::AcqRel);
        pointer.wheel.fetch_add(wheel, Ordering::AcqRel);
    }
}

/// A screen's mouse button (`index`: 0 left, 1 right, 2 middle) going down or up.
fn press_button(index: u32, down: bool) {
    if let Some(pointer) = pointer_block() {
        if down {
            pointer.buttons.fetch_or(1 << index, Ordering::AcqRel);
        } else {
            pointer.buttons.fetch_and(!(1 << index), Ordering::AcqRel);
        }
    }
}

fn pointer_block() -> Option<&'static PointerBlock> {
    bridge::get().map(|bridge| &bridge.plugin.pointer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_press_counts_once_however_many_frames_it_lasts() {
        let was_down = AtomicBool::new(false);
        let frames = [false, true, true, false, false, true, false, true, true, true, false];
        let presses: Vec<bool> = frames.iter().map(|&down| newly_down(&was_down, down)).collect();
        assert_eq!(presses.iter().filter(|&&pressed| pressed).count(), 3);
        assert_eq!(presses, [false, true, false, false, false, true, false, true, false, false, false]);
    }

    #[test]
    fn a_button_held_before_using_the_screen_still_comes_up_for_the_game() {
        let mut held = 0;
        // Driving: the game gets everything, and sees the left button go down.
        assert!(game_gets_button(0, true, false, &mut held));
        // Then the interaction key: new presses are the screen's...
        assert!(!game_gets_button(1, true, true, &mut held));
        // ...but the left button's release still reaches the game, once.
        assert!(game_gets_button(0, false, true, &mut held));
        assert!(!game_gets_button(0, true, true, &mut held));
        assert!(!game_gets_button(0, false, true, &mut held));
        // The right button went down on the screen: its release is the screen's too.
        assert!(!game_gets_button(1, false, true, &mut held));
        assert_eq!(held, 0);
    }
}
