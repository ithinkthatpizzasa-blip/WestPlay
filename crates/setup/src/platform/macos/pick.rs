//! macOS's own pickers (`NSOpenPanel`, through `rfd`): a folder (the player showing WestPlay where the game is
//! when it isn't found, an icon set's folder), and files (wallpapers, an icon set's .zip). As Windows', each
//! blocks until the player has chosen; `rfd` shows it on the main thread, whichever thread asks.

use std::path::PathBuf;

/// Asks for a folder, titled `title`: the folder chosen, or `None` if the player cancelled. (`owner`, Windows'
/// window handle, isn't used: the panel is the app's own.)
pub fn folder(_owner: isize, title: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().set_title(title).pick_folder()
}

/// Asks for files, titled `title`: those of the kinds `extensions` (`["png", "jpg"]`, shown as `kinds`), one or
/// (`several`) more. None if the player cancelled.
pub fn files(_owner: isize, title: &str, kinds: &str, extensions: &[&str], several: bool) -> Vec<PathBuf> {
    let dialog = rfd::FileDialog::new().set_title(title).add_filter(kinds, extensions);
    if several { dialog.pick_files().unwrap_or_default() } else { dialog.pick_file().into_iter().collect() }
}
