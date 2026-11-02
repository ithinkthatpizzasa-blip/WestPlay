//! Typing on the player's own keyboard into the WestPlay text field selected (the bridge's
//! [`westplay_shared::TypingBlock`]).
//!
//! While the truck stands still, a field selected then takes the keyboard: every key press becomes what
//! the player's keyboard layout types (Turkish Q types ı ğ ü, AltGr, Option and accent keys work), queued for
//! the app, and the game gets none of them but the few that stay its own ([`stays_the_games`]). The keys are
//! the game's again when the field is no longer selected (submitted, a result chosen, the keyboard put away,
//! Esc) or the truck moves; after moving, only a new selection takes them again, so the throttle never types
//! a "w" at a red light ([`Gate`]).
//!
//! What a key types is each OS's: Windows' keyboard layout (`typing/windows.rs`), the Mac's (`typing/macos.rs`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use westplay_shared::{TypedKey, TypingState};

use crate::{bridge, log};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(windows)]
pub use windows::*;

/// Faster than this, the truck is moving, km/h.
const MOVING_KMH: f32 = 1.0;

/// Virtual-key codes of the keys with a meaning of their own, and of those that stay the game's.
const VK_BACK: u32 = 0x08;
const VK_RETURN: u32 = 0x0d;
const VK_ESCAPE: u32 = 0x1b;
const VK_SNAPSHOT: u32 = 0x2c;
const VK_LWIN: u32 = 0x5b;
const VK_RWIN: u32 = 0x5c;
const VK_APPS: u32 = 0x5d;
const VK_F1: u32 = 0x70;
const VK_F24: u32 = 0x87;

/// Whether the keys type into the field right now (read by the keyboard hooks, on any thread).
static TYPING: AtomicBool = AtomicBool::new(false);
static GATE: Mutex<Gate> = Mutex::new(Gate { seen: 0, armed: 0 });

/// Whether the player's keys type into WestPlay's field (and so none reach the game but its own).
pub fn typing() -> bool {
    TYPING.load(Ordering::Acquire)
}

/// Once a frame (the truck's `speed`, m/s, and whether the game is paused): whether the keys type, from
/// the app's field selection; published for the app.
pub fn update(speed: f32, paused: bool) {
    let Some(bridge) = bridge::get() else { return };
    let block = &bridge.typing;
    let selection = if bridge::screen_on() { block.selection.load(Ordering::Acquire) } else { 0 };
    let moving = speed.abs() * 3.6 > MOVING_KMH;
    let state = GATE.lock().unwrap_or_else(|e| e.into_inner()).update(selection, moving);
    // Paused (a menu, the game's map): its keys are its own, without letting go of the selection.
    let typing = state == TypingState::Typing && !paused;
    if TYPING.swap(typing, Ordering::AcqRel) != typing {
        log!("typing on the keyboard {}", if typing { "on" } else { "off" });
    }
    block.state.store(state as u32, Ordering::Release);
}

/// When the keys type: from a selection made while the truck stands still, until it moves; then not until a
/// new selection (the app's number changes with each).
#[derive(Debug, PartialEq)]
struct Gate {
    /// The selection last seen, and the one typing is allowed for (0: none).
    seen: u32,
    armed: u32,
}

impl Gate {
    fn update(&mut self, selection: u32, moving: bool) -> TypingState {
        if selection == 0 {
            (self.seen, self.armed) = (0, 0);
            return TypingState::Off;
        }
        if selection != self.seen {
            self.seen = selection;
            self.armed = if moving { 0 } else { selection };
        }
        if moving {
            self.armed = 0;
        }
        if self.armed == selection { TypingState::Typing } else { TypingState::Paused }
    }
}

/// Keys that stay the game's while typing (virtual-key codes): the F-keys, Print Screen, the Windows keys and
/// the menu key (screenshots, the game's own F-key functions, switching windows).
pub fn stays_the_games(key: u32) -> bool {
    (VK_F1..=VK_F24).contains(&key) || [VK_SNAPSHOT, VK_LWIN, VK_RWIN, VK_APPS].contains(&key)
}

/// Queues what a key press typed for the app.
fn queue(typed: impl IntoIterator<Item = u32>) {
    let Some(bridge) = bridge::get() else { return };
    for code in typed {
        if !bridge.typing.keys.push(code) {
            log!("typing queue full, a key dropped");
        }
    }
}

/// A key with a meaning of its own while typing (`shortcut`: Ctrl on Windows, Command on a Mac; `word`: what
/// takes a whole word back, Ctrl on Windows, Option on a Mac): `Some(Some(key))`; `Some(None)` for a shortcut
/// that types nothing; `None` for one the layout turns into characters.
fn key_meaning(key: u32, shortcut: bool, word: bool) -> Option<Option<TypedKey>> {
    match key {
        VK_BACK => Some(Some(if word { TypedKey::WordBackspace } else { TypedKey::Backspace })),
        VK_RETURN => Some(Some(TypedKey::Enter)),
        VK_ESCAPE => Some(Some(TypedKey::Escape)),
        k if k == u32::from(b'V') && shortcut => Some(Some(TypedKey::Paste)),
        _ if shortcut => Some(None),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_from_a_selection_made_standing_still_until_the_truck_moves() {
        let mut gate = Gate { seen: 0, armed: 0 };
        assert_eq!(gate.update(0, false), TypingState::Off);
        // Selected while stopped: typing.
        assert_eq!(gate.update(1, false), TypingState::Typing);
        assert_eq!(gate.update(1, false), TypingState::Typing);
        // It moves: the keys are the game's, and stay so when it stops again (a red light).
        assert_eq!(gate.update(1, true), TypingState::Paused);
        assert_eq!(gate.update(1, false), TypingState::Paused);
        // Selected anew, standing still: typing again.
        assert_eq!(gate.update(2, false), TypingState::Typing);
        // Let go (submitted, Esc): off.
        assert_eq!(gate.update(0, false), TypingState::Off);
    }

    #[test]
    fn a_selection_made_while_moving_never_types() {
        let mut gate = Gate { seen: 0, armed: 0 };
        assert_eq!(gate.update(5, true), TypingState::Paused);
        assert_eq!(gate.update(5, false), TypingState::Paused);
        assert_eq!(gate.update(6, false), TypingState::Typing);
    }

    #[test]
    fn keys_with_meanings_of_their_own() {
        assert_eq!(key_meaning(VK_BACK, false, false), Some(Some(TypedKey::Backspace)));
        assert_eq!(key_meaning(VK_BACK, true, true), Some(Some(TypedKey::WordBackspace)));
        assert_eq!(key_meaning(VK_RETURN, false, false), Some(Some(TypedKey::Enter)));
        assert_eq!(key_meaning(VK_ESCAPE, false, false), Some(Some(TypedKey::Escape)));
        assert_eq!(key_meaning(u32::from(b'V'), true, true), Some(Some(TypedKey::Paste)));
        // A letter is the layout's; with the shortcut key, nothing.
        assert_eq!(key_meaning(u32::from(b'V'), false, false), None);
        assert_eq!(key_meaning(u32::from(b'C'), true, true), Some(None));
        // On a Mac, Option takes a word back but types characters otherwise.
        assert_eq!(key_meaning(VK_BACK, false, true), Some(Some(TypedKey::WordBackspace)));
        assert_eq!(key_meaning(u32::from(b'E'), false, true), None);
    }

    #[test]
    fn some_keys_stay_the_games() {
        assert!(stays_the_games(VK_F1) && stays_the_games(VK_SNAPSHOT));
        assert!(!stays_the_games(u32::from(b'W')) && !stays_the_games(VK_RETURN));
    }
}
