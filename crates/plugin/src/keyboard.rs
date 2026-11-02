//! Screen interaction: the keyboard, for typing into WestPlay.
//!
//! While the player types into a WestPlay field (`typing`), every key but the few that stay the game's own is
//! kept from it, and each press goes to the app as what it types. Otherwise every key reaches the game, the
//! interaction key held or not, so keyboard drivers can steer and brake while using the screen; but the keys
//! bound to WestPlay (`keybinds`), while their bindings count.
//!
//! How keys are kept from the game is each OS's: its window messages on Windows (`keyboard/windows.rs`), its
//! own event loop on macOS (`keyboard/macos.rs`).

use crate::{keybinds, typing};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(windows)]
pub use windows::*;

/// Whether the game sees `key` (a virtual-key code) up whatever it is: any key but its own while the player
/// types, and a key bound to WestPlay while its binding counts (`keybinds`).
fn hidden(key: u32) -> bool {
    (typing::typing() && !typing::stays_the_games(key)) || keybinds::taken(key)
}
