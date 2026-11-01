//! The GPU the cab's frames are handed to the game on: on Windows a D3D11 device on the game's GPU and the shared
//! frame texture the plugin reads (`windows.rs`); on macOS the frames go through memory (`westplay_shared::frames`),
//! and there's only the GPU to name (`macos.rs`).

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;
