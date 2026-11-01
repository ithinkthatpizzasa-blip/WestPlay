//! What finding and setting up the game asks of the OS, each OS's own: its folders, Steam's, the programs it
//! lists as installed, the game's version and whether it runs, the plugin's file and how it's put in place, and
//! the pickers. Everything else in this crate is the same on both.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;
