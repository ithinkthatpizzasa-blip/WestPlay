//! WestPlay's screen mod, mounted by the plugin itself (PLAN.md: the plugin mounts `westplay.scs` in every
//! mode, never shown in the Mod Manager), through the game's own mount: the function that mounts one device
//! from a request, as the game mounts its user folder with it. The archive goes into the mod pool, first. The
//! game's functions are found in its code by what they are (`locate`), not where one version had them, so other
//! builds work too (1.60.1.7, the VR branch's 1.61.1.1009s); a game whose code doesn't match is left alone.
//!
//! Mounted once a game process (an `sdk reload` doesn't mount it again), from `Documents\WestPlay\westplay.scs`
//! (macOS: `~/Library/Application Support/WestPlay/westplay.scs`; written there by WestPlay's setup, from the player's
//! own game), when the plugin loads, before the game
//! mounts the profile's mods; it stays through them and through a profile switch (tested 2026-10-20). Not while
//! WestPlay 1.2's copy is still in the game's `mod` folder (the app takes it out when it installs): with both,
//! the game has the screens' scripts twice and stops when a screen starts.
//!
//! The game finds a pool's files by the pool's index, which its mount doesn't update: the game rebuilds it after
//! its own mounts (the function that logs `[ufs] mod pool: …`). In single player the profile's mods, mounted
//! after the plugin, rebuild it with ours in; under TruckersMP the mod pool is indexed before the plugin loads
//! and nothing rebuilds it after, so a file only ours has (a screen's material) couldn't be opened: a white
//! screen (2026-10-21). So the mod pool's index is rebuilt right after the mount, as the game does.
//!
//! Finding the game's pieces is each OS's (`mod_mount/windows.rs`, `mod_mount/macos.rs`: the Mac game, 1.57, has the
//! same mount and request, and no pool index); mounting with them is shared.

use std::ffi::{CString, c_char, c_void};
use std::path::Path;

use crate::log;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
use macos::{first_in_process, found};
#[cfg(windows)]
use windows::{first_in_process, found};

/// What the mount needs of the game, as found: its mount and, in a game with a pool index (1.60, 1.61), its pool
/// by index and its rebuild of a pool's index.
struct Found {
    mount: usize,
    index: Option<(usize, usize)>,
}

/// A device kind the game opens as an archive (HashFS, else zip), the mod pool, and the highest priority a mod
/// can have there (the game keeps mods' within -100000..=99999).
const KIND_ARCHIVE: u32 = 3;
const POOL_MOD: u32 = 2;
const PRIORITY: i32 = 99_999;

/// The game's mount request, as its mount reads it.
#[repr(C)]
struct Request {
    kind: u32,
    pool: u32,
    path: *const c_char,
    /// The device's name (none: the game's empty one).
    name: *const c_char,
    flag_a: u8,
    priority: i32,
    flag_b: u8,
    /// Must be under 3 for an archive (the game refuses it otherwise).
    extra: u32,
}

type MountFn = unsafe extern "system" fn(*const Request) -> *mut c_void;
type PoolFn = unsafe extern "system" fn(i32) -> *mut c_void;
type IndexFn = unsafe extern "system" fn(*mut c_void);

/// The game's folder in Documents, by the game's id (as the SDK gives it).
fn game_folder(game_id: &str) -> Option<&'static str> {
    match game_id {
        "eut2" => Some("Euro Truck Simulator 2"),
        "ats" => Some("American Truck Simulator"),
        _ => None,
    }
}

/// Mounts WestPlay's mod into the game `game_id` (the SDK's), once a game process.
///
/// # Safety
/// On the game's main thread, from the plugin's init.
pub unsafe fn mount(game_id: &str) {
    let path = crate::log::log_dir().parent().map(|dir| dir.join("westplay.scs"));
    let Some(path) = path.clone().filter(|path| path.is_file()) else {
        // The folder looked in whole, and what's in it: a player's "it's there" is then checked by this log.
        let looked = path.as_deref().map_or("no Documents folder".into(), |path| path.display().to_string());
        let beside: Vec<String> = path
            .as_deref()
            .and_then(Path::parent)
            .and_then(|dir| std::fs::read_dir(dir).ok())
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        log!("mod mount: no {looked}: nothing mounted (beside it: {beside:?})");
        return;
    };
    let old =
        game_folder(game_id).map(|folder| crate::log::documents_dir().join(folder).join("mod").join("westplay.scs"));
    if let Some(old) = old.filter(|old| old.is_file()) {
        log!("mod mount: WestPlay 1.2's mod is still in {} (two would stop the game): nothing mounted", old.display());
        return;
    }
    if !first_in_process() {
        log!("mod mount: already mounted in this game process");
        return;
    }
    // SAFETY: the game's own executable, mapped for the process's life; its code read where the mount was found.
    unsafe {
        let Some((base, found)) = found() else {
            log!("mod mount: the game's mount isn't as read (another game version?): nothing mounted");
            return;
        };
        match found.index {
            Some((pool_at, index_at)) => log!(
                "mod mount: the game's mount at +{:#x}, its pools at +{:#x}, their index at +{:#x}",
                found.mount - base,
                pool_at - base,
                index_at - base
            ),
            None => log!("mod mount: the game's mount at +{:#x} (its pools have no index)", found.mount - base),
        }
        // The game writes paths with forward slashes; the path is kept for the process's life (the device may
        // keep it).
        let text = path.to_string_lossy().replace('\\', "/");
        let Ok(text) = CString::new(text) else { return };
        let text: &'static CString = Box::leak(Box::new(text));
        let request = Request {
            kind: KIND_ARCHIVE,
            pool: POOL_MOD,
            path: text.as_ptr(),
            name: std::ptr::null(),
            flag_a: 0,
            priority: PRIORITY,
            flag_b: 0,
            extra: 0,
        };
        let mount: MountFn = std::mem::transmute(found.mount);
        let device = mount(&request);
        log!(
            "mod mount: {} into the mod pool at priority {PRIORITY}: {}",
            text.to_string_lossy(),
            if device.is_null() { "the game refused it".to_string() } else { format!("device {:#x}", device as usize) }
        );
        if device.is_null() {
            return;
        }
        // The pool the mount just took (so it's there), its index rebuilt as the game does after its own mounts;
        // a game with no index has nothing to rebuild.
        let Some((pool_at, index_at)) = found.index else { return };
        let pool: PoolFn = std::mem::transmute(pool_at);
        let index: IndexFn = std::mem::transmute(index_at);
        let pool = pool(POOL_MOD as i32);
        if pool.is_null() {
            log!("mod mount: no mod pool to index");
            return;
        }
        index(pool);
        log!("mod mount: the mod pool's index rebuilt");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_games_folder_is_known_by_its_id() {
        assert_eq!(game_folder("eut2"), Some("Euro Truck Simulator 2"));
        assert_eq!(game_folder("ats"), Some("American Truck Simulator"));
        assert_eq!(game_folder("other"), None);
    }

    #[test]
    fn the_request_is_laid_out_as_the_games_mount_reads_it() {
        assert_eq!(std::mem::offset_of!(Request, kind), 0x0);
        assert_eq!(std::mem::offset_of!(Request, pool), 0x4);
        assert_eq!(std::mem::offset_of!(Request, path), 0x8);
        assert_eq!(std::mem::offset_of!(Request, name), 0x10);
        assert_eq!(std::mem::offset_of!(Request, flag_a), 0x18);
        assert_eq!(std::mem::offset_of!(Request, priority), 0x1C);
        assert_eq!(std::mem::offset_of!(Request, flag_b), 0x20);
        assert_eq!(std::mem::offset_of!(Request, extra), 0x24);
    }
}
