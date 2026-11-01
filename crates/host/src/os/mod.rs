//! The few small things the host asks of the OS, each OS's own (`windows.rs`, `macos.rs`): the local time, how long
//! the process has run, the clipboard's text, a page in the player's browser, memory, and a key's name.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;

/// A time of day, local.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millisecond: u32,
}
