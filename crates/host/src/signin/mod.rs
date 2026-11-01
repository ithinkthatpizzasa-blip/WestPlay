//! Signing in to YouTube, Spotify or Apple Music: the service's own sign-in page, in a window of WestPlay's on
//! the PC's desktop (a real keyboard, out of the game), on the profile the service's page in WestPlay uses, so the
//! sign-in is its too. Nothing of the account passes through WestPlay: the page is the service's, and what it
//! keeps (its cookies) is the browser's (WebView2's; WebKit's on macOS), as a browser's would be. WestPlay only sees
//! where the window is: still signing in, back on the service (signed in), or closed.
//!
//! Apple Music's sign-in is a dialog in its own web player (music.apple.com), whose address doesn't change as
//! it signs in: a script in the window's page opens that dialog and says when the page is signed in (only
//! that).
//!
//! The window is each OS's own: `windows.rs`, `macos.rs`.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::SignInWindow;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::SignInWindow;

use std::cell::Cell;
use std::time::{Duration, Instant};

/// A service to sign in to: its sign-in page, the window's title, and its own pages, where the sign-in comes
/// back to once done (but for `not_yet`, a step of its own on the way, if any). A service whose sign-in stays
/// on one page has a script in it instead (`page_says`), which posts `{"signedIn":true}` once it is.
pub struct Site {
    pub sign_in: &'static str,
    pub title: &'static str,
    pub home: &'static str,
    pub not_yet: Option<&'static str>,
    pub page_says: Option<&'static str>,
}

/// Google's sign-in for YouTube, coming back to YouTube once signed in (through YouTube's own `/signin`, which
/// sets its cookies on the way).
pub const YOUTUBE: Site = Site {
    sign_in: "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fwww.youtube.com%2F",
    title: "Sign in to YouTube - WestPlay",
    home: "https://www.youtube.com/",
    not_yet: Some("signin"),
    page_says: None,
};

/// Google's sign-in for YouTube Music: YouTube's own (YouTube Music shares YouTube's sign-in: its cookies are
/// YouTube's), coming back to YouTube, which sets them.
pub const YOUTUBE_MUSIC: Site = Site { title: "Sign in to YouTube Music - WestPlay", ..YOUTUBE };

/// Spotify's sign-in, coming back to its web player.
pub const SPOTIFY: Site = Site {
    sign_in: "https://accounts.spotify.com/login?continue=https%3A%2F%2Fopen.spotify.com%2F",
    title: "Sign in to Spotify - WestPlay",
    home: "https://open.spotify.com/",
    not_yet: None,
    page_says: None,
};

/// Spotify's sign-up, coming back to its web player signed in to the new account.
pub const SPOTIFY_SIGN_UP: Site = Site {
    sign_in: "https://www.spotify.com/signup?forward_url=https%3A%2F%2Fopen.spotify.com%2F",
    title: "Sign up for Spotify - WestPlay",
    home: "https://open.spotify.com/",
    not_yet: None,
    page_says: None,
};

/// Apple Music's sign-in: the dialog of its own web player, opened at once (`apple_music_sign_in.js`).
pub const APPLE_MUSIC: Site = Site {
    sign_in: "https://music.apple.com/",
    title: "Sign in to Apple Music - WestPlay",
    home: "https://music.apple.com/",
    not_yet: None,
    page_says: Some(include_str!("../apple_music_sign_in.js")),
};
/// Where the sign-in is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Still on Google's pages.
    Signing,
    /// Back on the service: signed in.
    SignedIn,
    /// The player closed the window.
    Closed,
}

/// Where a sign-in is at, by the address of the last page its window finished loading: a page of the
/// service's own is signed in, but for its own step on the way (YouTube's `/signin`, which sets its cookies).
fn progress_at(site: &Site, loaded: &str) -> Progress {
    let home = loaded.strip_prefix(site.home);
    let done = home.is_some_and(|path| site.not_yet.is_none_or(|step| !path.starts_with(step)));
    if done { Progress::SignedIn } else { Progress::Signing }
}

/// Whether a message of a sign-in window's page says it's signed in (`{"signedIn":true}`).
fn says_signed_in(message: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(message)
        .is_ok_and(|value| value.get("signedIn").and_then(serde_json::Value::as_bool) == Some(true))
}

/// How long the service's page has to say it's signed in, once its sign-in window has closed signed in.
pub const CONFIRM_WAIT: Duration = Duration::from_secs(20);

/// A sign-in still under way after its window closed signed in: the service's own page loads again, and only
/// once it says it's signed in is the sign-in done (or after `CONFIRM_WAIT`, should it never say). The
/// companion shows it as signing in until then, not as signed out for a moment.
#[derive(Debug, Default)]
pub struct Confirming(Cell<Option<Instant>>);

impl Confirming {
    /// The window closed signed in: confirming from now.
    pub fn start(&self) {
        self.0.set(Some(Instant::now()));
    }

    /// Whether confirming just ended: the page says it's signed in (`signed_in`, as it last said, if it said),
    /// or the wait is over.
    pub fn ended(&self, signed_in: Option<bool>) -> bool {
        let Some(since) = self.0.get() else { return false };
        if signed_in == Some(true) || since.elapsed() >= CONFIRM_WAIT {
            self.0.set(None);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signed in in its window, the sign-in goes on until the page says it's signed in: not as the page
    /// starts loading again (nothing said yet) nor while it still says signed out, and only once.
    #[test]
    fn confirms_a_sign_in_once_the_page_says_so() {
        let confirming = Confirming::default();
        assert!(!confirming.ended(Some(true)), "nothing to confirm before the window closed");
        confirming.start();
        assert!(!confirming.ended(None));
        assert!(!confirming.ended(Some(false)));
        assert!(confirming.ended(Some(true)));
        assert!(!confirming.ended(Some(true)), "ended once");
    }

    #[test]
    fn is_signed_in_once_back_on_youtube() {
        assert_eq!(progress_at(&YOUTUBE, YOUTUBE.sign_in), Progress::Signing);
        assert_eq!(
            progress_at(&YOUTUBE, "https://accounts.google.com/v3/signin/challenge/pwd?flowName=x"),
            Progress::Signing
        );
        assert_eq!(progress_at(&YOUTUBE, "https://www.youtube.com/"), Progress::SignedIn);
        // YouTube's own step, setting its cookies: not yet.
        assert_eq!(
            progress_at(&YOUTUBE, "https://www.youtube.com/signin?action_handle_signin=true&app=desktop"),
            Progress::Signing
        );
        // Only YouTube itself, not an address mentioning it.
        assert_eq!(progress_at(&YOUTUBE, "https://evil.example/?https://www.youtube.com/"), Progress::Signing);
        assert_eq!(progress_at(&YOUTUBE, ""), Progress::Signing);
    }

    #[test]
    fn is_signed_in_to_spotify_once_back_on_its_web_player() {
        assert_eq!(progress_at(&SPOTIFY, SPOTIFY.sign_in), Progress::Signing);
        assert_eq!(progress_at(&SPOTIFY, "https://accounts.spotify.com/en/status"), Progress::Signing);
        assert_eq!(progress_at(&SPOTIFY, "https://open.spotify.com/"), Progress::SignedIn);
        assert_eq!(progress_at(&SPOTIFY, "https://open.spotify.com/collection/tracks"), Progress::SignedIn);
        assert_eq!(progress_at(&SPOTIFY, "https://evil.example/?https://open.spotify.com/"), Progress::Signing);
        // Signing up: Spotify's own pages until the new account comes back to the web player.
        assert_eq!(progress_at(&SPOTIFY_SIGN_UP, SPOTIFY_SIGN_UP.sign_in), Progress::Signing);
        assert_eq!(progress_at(&SPOTIFY_SIGN_UP, "https://open.spotify.com/"), Progress::SignedIn);
    }

    /// Apple Music's sign-in stays on its web player: signed in only once its page says so.
    #[test]
    fn hears_a_page_say_its_signed_in() {
        assert!(says_signed_in(r#"{"signedIn":true}"#));
        assert!(!says_signed_in(r#"{"signedIn":false}"#));
        assert!(!says_signed_in(r#"{"signedIn":"yes"}"#));
        assert!(!says_signed_in("not json"));
        assert!(APPLE_MUSIC.page_says.is_some(), "its address says nothing");
    }
}
