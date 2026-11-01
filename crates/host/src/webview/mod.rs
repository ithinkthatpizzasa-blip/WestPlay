//! The WebViews the cab is drawn in, offscreen: WebView2 on Windows (`windows.rs`), WKWebView on macOS
//! (`macos.rs`). Both give the same [`Stage`] and [`OffscreenWebView`]; what they share is here.
//!
//! Every WebView of the host renders onto one [`Stage`]: its WebViews, in their order (the cabin page on top,
//! YouTube's player under it while it plays), make up the frame that is captured.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "macos")]
mod macos_input;
#[cfg(target_os = "macos")]
pub use self::macos::*;

use std::cell::Cell;
use std::rc::Rc;

pub type Error = Box<dyn std::error::Error>;

/// A WebView process that failed, as far as it matters to WestPlay (the others, such as its GPU process,
/// WebView2 restarts by itself).
/// Ordered by how much they take to recover from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Failure {
    /// The page's own process exited or stopped responding: the page is gone (blank) until reloaded.
    Page,
    /// The browser process exited: the whole WebView is gone and must be created again. (WebView2's only: on macOS
    /// the app is WebKit's browser.)
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Browser,
}

/// The page's size (width, height), as the stage has it now: the players keep their videos within it.
pub type PageSize = Rc<Cell<(u32, u32)>>;

/// Mouse input for a page ([`OffscreenWebView::send_mouse`]), as the page gets it from a mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Move,
    /// The mouse gone from the page.
    Leave,
    LeftDown,
    LeftUp,
    RightDown,
    RightUp,
    /// The wheel turned (its delta, Windows' units: 120 a notch, positive away from the player).
    Wheel,
}

/// The mouse buttons held along with a [`MouseKind`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseButtons {
    pub left: bool,
    pub right: bool,
}

/// The page for [`OffscreenWebView::fit`]: `width` x `height` grown evenly until it fills `room` one way,
/// and how much to shrink it by to be `width` x `height` again.
fn fitted((room_width, room_height): (u32, u32), width: u32, height: u32) -> (u32, u32, f32) {
    let (width, height) = (width.max(1) as f32, height.max(1) as f32);
    let grow = (room_width as f32 / width).min(room_height as f32 / height).max(1.0);
    ((width * grow).round() as u32, (height * grow).round() as u32, 1.0 / grow)
}

/// A page's address as the log gives it: only its host (`https://www.youtube.com/watch?v=…` → `www.youtube.com`).
fn host_of(address: &str) -> &str {
    let rest = address.split_once("://").map_or(address, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_only_a_pages_host() {
        assert_eq!(host_of("https://www.youtube.com/watch?v=abc&t=1"), "www.youtube.com");
        assert_eq!(host_of("https://open.spotify.com"), "open.spotify.com");
        assert_eq!(host_of("https://music.apple.com?x=1"), "music.apple.com");
        assert_eq!(host_of("about:blank"), "about:blank");
    }

    #[test]
    fn fits_a_small_view_in_its_own_shape() {
        // A 16:9 card on a 16:9 screen: the whole screen, shrunk.
        assert_eq!(fitted((1280, 720), 142, 80), (1278, 720, 80.0 / 720.0));
        // A 16:9 card on a squarer screen: as wide as the screen, in the card's shape.
        let (w, h, scale) = fitted((1024, 768), 160, 90);
        assert_eq!((w, h), (1024, 576));
        assert!((w as f32 * scale - 160.0).abs() < 0.5 && (h as f32 * scale - 90.0).abs() < 0.5);
        // Never grown past the screen, nor shrunk below the view's own size.
        assert_eq!(fitted((1280, 720), 2000, 1000).2, 1.0);
    }
}
