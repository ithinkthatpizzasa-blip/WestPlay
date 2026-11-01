//! Web pages opened in the player's own browser, outside WestPlay.

/// Opens `url` in the player's default browser. Only for addresses WestPlay itself names (each of the app's
/// commands opens one fixed page), never one a page asks for.
pub fn open(url: &str) {
    if let Err(why) = crate::os::open_url(url) {
        log!("[browser] {url} didn't open ({why})");
    }
}
