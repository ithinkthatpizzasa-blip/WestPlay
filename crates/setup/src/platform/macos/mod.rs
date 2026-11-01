//! macOS: the game's folders in `~/Library/Application Support` (docs/MACOS_PORT.md), Steam's there too, the
//! game's version from its `version.scs`, the process list (`libproc`), and the plugin signed and taken out of
//! quarantine as it's put in place.

pub mod pick;

use std::ffi::{CStr, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::game::Version;
use crate::plugin::{EXIT_COPIED, EXIT_IN_USE, InstallError};

/// The game's executable, in its folder (the one with `base.scs`, beside the game's `.app`).
pub const EXECUTABLE: &str = "Euro Truck Simulator 2.app/Contents/MacOS/eurotrucks2";
/// The game's process, by its executable's name.
pub const PROCESS: &str = "eurotrucks2";
/// The game's plugins folder, in its folder: in its `.app`, beside its executable (proved by loading a plugin
/// from there: docs/MACOS_PORT.md).
pub const PLUGINS_DIR: &[&str] = &["Euro Truck Simulator 2.app", "Contents", "MacOS", "plugins"];
/// The plugin's file, in the game's plugins folder.
pub const PLUGIN_FILE: &str = "westplay.dylib";

/// The user's home folder.
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// The folder that is the game's Documents on Windows: where the game keeps its own folder (its profiles, mods
/// and settings; the game's log: `[ufs] Home directory: '~/Library/Application Support/Euro Truck Simulator 2'`),
/// and WestPlay its own. Not `~/Documents`, which macOS asks the player's leave for each program to read, the
/// game included.
pub fn documents() -> Option<PathBuf> {
    Some(home()?.join("Library").join("Application Support"))
}

/// Where TruckersMP installs itself: nowhere on macOS, as it has no client for the Mac (docs/MACOS_PORT.md).
pub fn roaming() -> Option<PathBuf> {
    None
}

/// Steam's folder: Steam for macOS keeps it in the user's Application Support.
pub fn steam() -> Option<PathBuf> {
    Some(documents()?.join("Steam"))
}

/// The programs installed that aren't Steam's: macOS keeps no list of them, and the game for the Mac comes only
/// through Steam; a copy elsewhere is chosen by its folder.
pub fn installed_programs() -> Vec<(String, PathBuf)> {
    Vec::new()
}

/// The version of the game in `dir`: its pack set's (`version.scs`), as the game logs it (`Loaded pack set
/// version 1.57.2.7`). The executable's own version (`CFBundleShortVersionString`, 1.22.2) isn't the game's.
pub fn game_version(dir: &Path) -> Option<Version> {
    use westplay_mapdata::archive::Archive;
    use westplay_mapdata::files::{GameFiles, Mount};
    let archive = Archive::open(&dir.join("version.scs")).ok()?;
    let files = GameFiles::new(vec![Mount::Archive(archive)]);
    let text = files.read("version.sii").ok()??;
    crate::game::pack_version(&String::from_utf8_lossy(&text))
}

/// Whether a process whose executable is named `name` runs (the user's or another's).
pub fn process_running(name: &str) -> bool {
    // SAFETY: with a null buffer, `proc_listallpids` only counts.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return false;
    }
    // Room for processes started meanwhile.
    let mut pids = vec![0 as libc::pid_t; count as usize + 64];
    let size = (pids.len() * size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: the buffer is ours and `size` its length in bytes.
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
    pids.truncate(count.max(0) as usize);
    pids.into_iter().filter(|&pid| pid > 0).any(|pid| {
        let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer is ours and its length passed with it; the path written in it is nul-terminated.
        let length = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        if length <= 0 {
            return false;
        }
        let path = CStr::from_bytes_until_nul(&path).map(CStr::to_bytes).unwrap_or_default();
        Path::new(OsStr::from_bytes(path)).file_name().is_some_and(|file| file == name)
    })
}

/// Readies the plugin just copied to `file` to be loaded by the game: out of quarantine (a copy keeps a
/// download's), and signed. The game loads libraries signed by anyone (it has `disable-library-validation`),
/// but code for macOS must carry a signature: an ad-hoc one, made on this Mac, unless it has one already.
pub fn prepare_plugin(file: &Path) -> io::Result<()> {
    remove_quarantine(file)?;
    let signed = Command::new("/usr/bin/codesign").arg("--verify").arg(file).output()?.status.success();
    if !signed {
        let output = Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(file).output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!("codesign: {}", String::from_utf8_lossy(&output.stderr).trim())));
        }
    }
    Ok(())
}

/// Takes `file` out of quarantine (the attribute macOS puts on what's downloaded, kept by copies).
fn remove_quarantine(file: &Path) -> io::Result<()> {
    let path = std::ffi::CString::new(file.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // SAFETY: a nul-terminated path and attribute name.
    let result = unsafe { libc::removexattr(path.as_ptr(), c"com.apple.quarantine".as_ptr(), 0) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    // Not in quarantine: nothing to do.
    if error.raw_os_error() == Some(libc::ENOATTR) { Ok(()) } else { Err(error) }
}

/// What's left of the plugins folder after the plugin's taken out: the folder itself, when it's empty, so the
/// game's `.app` is as it was before (it has no such folder until a plugin is put in).
pub fn tidy_plugins_dir(plugins: &Path) {
    // Only an empty folder goes: another plugin there keeps it.
    let _ = std::fs::remove_dir(plugins);
}

/// WestPlay run again with administrator rights and `argument` with `paths` (macOS asks the player for an
/// administrator's password, its own prompt), waited for. The game's folder is the user's own on macOS (in
/// Steam's library, in their Library), so this is only for a library somewhere only an administrator may write.
pub fn as_administrator(paths: &[&Path], argument: &str) -> Result<(), InstallError> {
    let exe = std::env::current_exe()?;
    let mut command = shell_quoted(&exe.to_string_lossy());
    command.push(' ');
    command.push_str(&shell_quoted(argument));
    for path in paths {
        command.push(' ');
        command.push_str(&shell_quoted(&path.to_string_lossy()));
    }
    // The exit code as the script's answer: `do shell script` takes a failing one for an error of its own.
    let script =
        format!("do shell script \"{}; echo $?\" with administrator privileges", applescript_escaped(&command));
    let output = Command::new("/usr/bin/osascript").arg("-e").arg(script).output()?;
    if !output.status.success() {
        // -128: the player cancelled the prompt.
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(if error.contains("-128") {
            InstallError::Declined
        } else {
            InstallError::Io(io::Error::other(error.trim().to_owned()))
        });
    }
    match String::from_utf8_lossy(&output.stdout).trim().parse::<u32>() {
        Ok(EXIT_COPIED) => Ok(()),
        Ok(EXIT_IN_USE) => Err(InstallError::InUse),
        _ => Err(InstallError::Io(io::Error::other("the plugin couldn't be copied or removed"))),
    }
}

/// `text` as one word for the shell, whatever it holds.
fn shell_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `text` inside an AppleScript string.
fn applescript_escaped(text: &str) -> String {
    text.replace('\\', r"\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_for_the_shell_and_applescript() {
        assert_eq!(shell_quoted("/Users/a/Steam Library/it's"), r"'/Users/a/Steam Library/it'\''s'");
        assert_eq!(applescript_escaped(r#"'a "b" \c'"#), r#"'a \"b\" \\c'"#);
    }

    #[test]
    fn finds_a_running_process_by_its_executables_name() {
        let me = std::env::current_exe().unwrap();
        assert!(process_running(me.file_name().unwrap().to_str().unwrap()));
        assert!(!process_running("no-such-process-westplay"));
    }

    #[test]
    fn takes_a_file_out_of_quarantine() {
        let file = std::env::temp_dir().join(format!("westplay-quarantine-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let status = Command::new("/usr/bin/xattr")
            .args(["-w", "com.apple.quarantine", "0083;00000000;Safari;"])
            .arg(&file)
            .status()
            .unwrap();
        assert!(status.success());
        remove_quarantine(&file).unwrap();
        let listed = Command::new("/usr/bin/xattr").arg(&file).output().unwrap();
        assert!(!String::from_utf8_lossy(&listed.stdout).contains("quarantine"));
        // Not in quarantine: fine too.
        remove_quarantine(&file).unwrap();
        let _ = std::fs::remove_file(file);
    }
}

#[cfg(test)]
mod install_tests {
    use std::path::PathBuf;
    use std::process::Command;

    use crate::game::{FoundBy, Game};
    use crate::plugin;

    /// A game folder (temporary) and a plugin as the build makes it for the game: x86_64, unsigned, in quarantine
    /// as a download is.
    fn setup() -> (Game, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("westplay-mac-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("plugin.c");
        std::fs::write(&source, "int westplay_test_plugin = 1;\n").unwrap();
        let ours = root.join("westplay.dylib");
        let built = Command::new("/usr/bin/clang")
            .args(["-arch", "x86_64", "-dynamiclib", "-Wl,-no_adhoc_codesign", "-o"])
            .arg(&ours)
            .arg(&source)
            .status()
            .unwrap();
        assert!(built.success());
        assert!(!Command::new("/usr/bin/codesign").arg("--verify").arg(&ours).status().unwrap().success());
        let quarantined = Command::new("/usr/bin/xattr")
            .args(["-w", "com.apple.quarantine", "0083;00000000;Safari;"])
            .arg(&ours)
            .status()
            .unwrap();
        assert!(quarantined.success());
        let game = Game { dir: root.join("game"), found_by: FoundBy::Chosen, version: None, dlcs: vec![] };
        (game, ours, root)
    }

    #[test]
    fn installs_the_plugin_signed_and_out_of_quarantine_and_leaves_no_trace() {
        let (game, ours, root) = setup();
        let installed = plugin::install(&game, &ours).unwrap();
        assert!(installed.ends_with("Euro Truck Simulator 2.app/Contents/MacOS/plugins/westplay.dylib"));
        assert!(Command::new("/usr/bin/codesign").arg("--verify").arg(&installed).status().unwrap().success());
        let attributes = Command::new("/usr/bin/xattr").arg(&installed).output().unwrap();
        assert!(!String::from_utf8_lossy(&attributes.stdout).contains("quarantine"));
        // Signed on the way in, so not the same bytes as WestPlay's own: told apart as another version. The plugin
        // WestPlay ships is signed when it's built, so its copy is the same, and current.
        assert_eq!(plugin::state(&game, &ours).unwrap(), plugin::State::Different);
        assert!(Command::new("/usr/bin/codesign").args(["--sign", "-"]).arg(&ours).status().unwrap().success());
        plugin::install(&game, &ours).unwrap();
        assert_eq!(plugin::state(&game, &ours).unwrap(), plugin::State::Current);
        plugin::uninstall(&game).unwrap();
        assert!(!game.plugins_dir().exists(), "the plugins folder WestPlay made goes with it");
        // Another plugin there keeps the folder.
        std::fs::create_dir_all(game.plugins_dir()).unwrap();
        std::fs::write(game.plugins_dir().join("other.dylib"), b"theirs").unwrap();
        plugin::install(&game, &ours).unwrap();
        plugin::uninstall(&game).unwrap();
        assert!(game.plugins_dir().join("other.dylib").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
