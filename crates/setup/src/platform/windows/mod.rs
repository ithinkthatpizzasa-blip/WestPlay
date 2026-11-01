//! Windows: Known Folders, the registry (Steam's folder, the programs installed), the executable's version
//! resource, the process list, and UAC for the plugins folder.

pub mod pick;

use std::io;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, ERROR_SUCCESS, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW,
    RegOpenKeyExW,
};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    FOLDERID_Documents, FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    SHGetKnownFolderPath, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

use crate::game::Version;
use crate::plugin::{EXIT_COPIED, EXIT_FAILED, EXIT_IN_USE, InstallError};

/// The game's executable, in its folder.
pub const EXECUTABLE: &str = "bin/win_x64/eurotrucks2.exe";
/// The game's process, as Windows names it (TruckersMP's client runs the same).
pub const PROCESS: &str = "eurotrucks2.exe";
/// The game's plugins folder, in its folder.
pub const PLUGINS_DIR: &[&str] = &["bin", "win_x64", "plugins"];
/// The plugin's file, in the game's plugins folder.
pub const PLUGIN_FILE: &str = "westplay.dll";

/// The player's Documents folder, wherever it is (it may be moved, to OneDrive say).
pub fn documents() -> Option<PathBuf> {
    // SAFETY: Windows hands back a CoTaskMem string, read before it's freed here.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None).ok()?;
        let result = path.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(path.0 as *const _));
        result
    }
}

/// The player's roaming application data folder (`%APPDATA%`), where TruckersMP installs itself.
pub fn roaming() -> Option<PathBuf> {
    // SAFETY: Windows hands back a CoTaskMem string, read before it's freed here.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, None).ok()?;
        let result = path.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(path.0 as *const _));
        result
    }
}

/// Steam's folder, as Steam records it (for this user, else for the machine).
pub fn steam() -> Option<PathBuf> {
    registry_text(HKEY_CURRENT_USER, w!("Software\\Valve\\Steam"), w!("SteamPath"))
        .or_else(|| registry_text(HKEY_LOCAL_MACHINE, w!("SOFTWARE\\WOW6432Node\\Valve\\Steam"), w!("InstallPath")))
        .map(PathBuf::from)
}

/// Every program Windows lists as installed (for this user and the machine, 64- and 32-bit): its name and
/// where it's installed, when it says.
pub fn installed_programs() -> Vec<(String, PathBuf)> {
    let roots = [
        (HKEY_LOCAL_MACHINE, w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall")),
        (HKEY_LOCAL_MACHINE, w!("SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall")),
        (HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall")),
    ];
    let mut programs = Vec::new();
    for (hive, path) in roots {
        for name in subkeys(hive, path) {
            // SAFETY: `path` is one of the static registry paths below.
            let key = HSTRING::from(format!("{}\\{name}", unsafe { path.to_string() }.unwrap_or_default()));
            let key = PCWSTR(key.as_ptr());
            let (Some(display), Some(location)) =
                (registry_text(hive, key, w!("DisplayName")), registry_text(hive, key, w!("InstallLocation")))
            else {
                continue;
            };
            if !location.trim().is_empty() {
                programs.push((display, PathBuf::from(location.trim().trim_matches('"'))));
            }
        }
    }
    programs
}

/// The names of a registry key's subkeys.
fn subkeys(hive: HKEY, path: PCWSTR) -> Vec<String> {
    let mut key = HKEY::default();
    // SAFETY: the hive is a predefined key and `path` a static string; `key` is a local the call writes.
    if unsafe { RegOpenKeyExW(hive, path, None, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut names = Vec::new();
    for index in 0.. {
        let mut buffer = [0u16; 256];
        let mut length = buffer.len() as u32;
        let status =
            // SAFETY: the key is open; `buffer` is ours and `length` says how much of it the call may write.
            unsafe { RegEnumKeyExW(key, index, Some(PWSTR(buffer.as_mut_ptr())), &mut length, None, None, None, None) };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&buffer[..length as usize]));
    }
    // SAFETY: the key was opened above and is closed once, after its last use.
    unsafe {
        let _ = RegCloseKey(key);
    }
    names
}

/// A text value of the registry, if it's there.
fn registry_text(hive: HKEY, path: PCWSTR, name: PCWSTR) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: the buffer is ours and `size` says how much of it the call may write, in bytes.
    let status = unsafe {
        RegGetValueW(hive, path, name, RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&mut size))
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// The version of the game in `dir`: its executable's file version (its version resource's), if it has one.
pub fn game_version(dir: &Path) -> Option<Version> {
    let file = HSTRING::from(dir.join(EXECUTABLE).as_os_str());
    // SAFETY: `data` is the size Windows asked for and is read only through it; the fixed info is checked to be
    // there and long enough before it's read.
    unsafe {
        let size = GetFileVersionInfoSizeW(&file, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&file, None, size, data.as_mut_ptr().cast()).ok()?;
        let mut info: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut length = 0u32;
        if !VerQueryValueW(data.as_ptr().cast(), w!("\\"), &mut info, &mut length).as_bool()
            || info.is_null()
            || (length as usize) < size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        // Within `data`, a byte buffer: read as it lies, whatever its alignment.
        let info = std::ptr::read_unaligned(info as *const VS_FIXEDFILEINFO);
        let (ms, ls) = (info.dwFileVersionMS, info.dwFileVersionLS);
        Some(Version([(ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16]))
    }
}

/// Whether a process named `name` runs.
pub fn process_running(name: &str) -> bool {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    // SAFETY: a snapshot of the PC's processes, closed below.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else { return false };
    let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut found = false;
    // SAFETY: the snapshot is open; `entry` is a local with its size set, as the call requires.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while more {
        let length = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        if String::from_utf16_lossy(&entry.szExeFile[..length]).eq_ignore_ascii_case(name) {
            found = true;
            break;
        }
        // SAFETY: as above: the open snapshot, and `entry` with its size set.
        more = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    // SAFETY: the snapshot is closed once, after its last use.
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    found
}

/// Readies the plugin just copied to `file` to be loaded by the game: nothing on Windows.
pub fn prepare_plugin(_file: &Path) -> io::Result<()> {
    Ok(())
}

/// What's left of the plugins folder after the plugin's taken out: as it is, on Windows.
pub fn tidy_plugins_dir(_plugins: &Path) {}

/// WestPlay started again with administrator rights and `arguments` (Windows asks the player), waited for.
pub fn as_administrator(arguments: &[&Path], argument: &str) -> Result<(), InstallError> {
    let arguments: Vec<String> = std::iter::once(argument.to_owned())
        .chain(arguments.iter().map(|path| format!("\"{}\"", path.display())))
        .collect();
    let exe = std::env::current_exe()?;
    let (exe, arguments) = (HSTRING::from(exe.as_os_str()), HSTRING::from(arguments.join(" ")));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(arguments.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` is complete and its strings live through the call.
    if let Err(error) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if error.code() == ERROR_CANCELLED.to_hresult() {
            InstallError::Declined
        } else {
            InstallError::Io(io::Error::other(error))
        });
    }
    let mut code = EXIT_FAILED;
    // SAFETY: the process handle came from the call above (`SEE_MASK_NOCLOSEPROCESS`) and is closed once.
    unsafe {
        if WaitForSingleObject(info.hProcess, INFINITE) == WAIT_OBJECT_0 {
            let _ = GetExitCodeProcess(info.hProcess, &mut code);
        }
        let _ = CloseHandle(info.hProcess);
    }
    match code {
        EXIT_COPIED => Ok(()),
        EXIT_IN_USE => Err(InstallError::InUse),
        _ => Err(InstallError::Io(io::Error::other("the plugin couldn't be copied or removed"))),
    }
}
