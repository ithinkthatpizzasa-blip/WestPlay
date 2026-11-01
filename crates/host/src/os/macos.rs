//! The host's small OS calls on macOS.

use std::path::PathBuf;

use dispatch2::run_on_main;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSWorkspace};
use objc2_foundation::{NSString, NSURL};

use super::LocalTime;

/// The local time now.
pub fn local_time() -> LocalTime {
    let mut now = libc::timeval { tv_sec: 0, tv_usec: 0 };
    // SAFETY: `now` is a local the call fills; no time zone wanted.
    unsafe { libc::gettimeofday(&mut now, std::ptr::null_mut()) };
    // SAFETY: plain data, filled in by `localtime_r` from the seconds above.
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both are locals, the result written into `local`.
    unsafe { libc::localtime_r(&now.tv_sec, &mut local) };
    LocalTime {
        hour: local.tm_hour as u32,
        minute: local.tm_min as u32,
        second: local.tm_sec as u32,
        millisecond: (now.tv_usec / 1000) as u32,
    }
}

/// Milliseconds since this process was started.
pub fn uptime_ms() -> Option<u64> {
    // SAFETY: plain data, filled in by `proc_pidinfo` for this process.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a local of the size passed with it.
    let read = unsafe { libc::proc_pidinfo(libc::getpid(), libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if read != size {
        return None;
    }
    let started = info.pbi_start_tvsec * 1000 + info.pbi_start_tvusec / 1000;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    Some(now.saturating_sub(started))
}

/// The text on the Mac's clipboard, if it holds any.
pub fn clipboard_text() -> Option<String> {
    run_on_main(|_| {
        let board = NSPasteboard::generalPasteboard();
        // SAFETY: AppKit's own type name, a static string.
        let kind = unsafe { NSPasteboardTypeString };
        board.stringForType(kind).map(|text| text.to_string())
    })
}

/// Opens `url` in the player's default browser; why not, if it didn't.
pub fn open_url(url: &str) -> Result<(), String> {
    let url = url.to_owned();
    run_on_main(move |_| {
        let Some(address) = NSURL::URLWithString(&NSString::from_str(&url)) else {
            return Err("not an address".to_owned());
        };
        if NSWorkspace::sharedWorkspace().openURL(&address) { Ok(()) } else { Err("NSWorkspace said no".to_owned()) }
    })
}

/// The memory WestPlay has taken: its physical footprint (what macOS counts against it, in RAM or compressed).
pub fn process_memory() -> Option<u64> {
    // SAFETY: plain data, filled in by `proc_pid_rusage` for this process.
    let mut usage: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: `usage` is a local of the flavour asked for.
    let read = unsafe {
        libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, (&raw mut usage).cast::<libc::rusage_info_t>())
    };
    (read == 0).then_some(usage.ri_phys_footprint)
}

/// The Mac's memory: free now (free, inactive and purgeable pages: what's there to take), and in all (bytes).
pub fn memory() -> Option<(u64, u64)> {
    let mut total: u64 = 0;
    let mut size = size_of::<u64>();
    // SAFETY: a named sysctl into a local of its size.
    let read = unsafe {
        libc::sysctlbyname(c"hw.memsize".as_ptr(), (&raw mut total).cast(), &mut size, std::ptr::null_mut(), 0)
    };
    if read != 0 {
        return None;
    }
    // SAFETY: plain data, filled in by `host_statistics64`.
    let mut stats: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = (size_of::<libc::vm_statistics64>() / size_of::<libc::integer_t>()) as libc::mach_msg_type_number_t;
    // SAFETY: this host's port; `stats` is a local of the count passed with it.
    #[allow(deprecated)]
    let result = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            (&raw mut stats).cast::<libc::integer_t>(),
            &mut count,
        )
    };
    if result != 0 {
        return None;
    }
    // SAFETY: a plain query.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    let free =
        (u64::from(stats.free_count) + u64::from(stats.inactive_count) + u64::from(stats.purgeable_count)) * page;
    Some((free, total))
}

/// A key's name on a Mac keyboard ("Left Option", "Num 4", "Q"), by its Windows virtual-key code (the bridge's: the
/// plugin gives the Mac's keys those codes). `None` for a mouse button, or a code with no key on a Mac.
pub fn key_name(code: u32) -> Option<String> {
    let name = match code {
        0x30..=0x39 | 0x41..=0x5a => return char::from_u32(code).map(String::from),
        0x60..=0x69 => return Some(format!("Num {}", code - 0x60)),
        0x70..=0x87 => return Some(format!("F{}", code - 0x6f)),
        0x08 => "Delete",
        0x09 => "Tab",
        0x0d => "Return",
        0x10 => "Shift",
        0x11 => "Control",
        0x12 => "Option",
        0x14 => "Caps Lock",
        0x1b => "Esc",
        0x20 => "Space",
        0x21 => "Page Up",
        0x22 => "Page Down",
        0x23 => "End",
        0x24 => "Home",
        0x25 => "Left",
        0x26 => "Up",
        0x27 => "Right",
        0x28 => "Down",
        0x2e => "Forward Delete",
        0x2f => "Help",
        0x5b => "Left Command",
        0x5c => "Right Command",
        0x5d => "Menu",
        0x6a => "Num *",
        0x6b => "Num +",
        0x6d => "Num -",
        0x6e => "Num .",
        0x6f => "Num /",
        0x90 => "Clear",
        0xa0 => "Left Shift",
        0xa1 => "Right Shift",
        0xa2 => "Left Control",
        0xa3 => "Right Control",
        0xa4 => "Left Option",
        0xa5 => "Right Option",
        0xba => ";",
        0xbb => "=",
        0xbc => ",",
        0xbd => "-",
        0xbe => ".",
        0xbf => "/",
        0xc0 => "`",
        0xdb => "[",
        0xdc => "\\",
        0xdd => "]",
        0xde => "'",
        0xe2 => "§",
        _ => return None,
    };
    Some(name.to_owned())
}

/// WestPlay's own data folder, for its log when its first choice refuses it: the companion's
/// (`~/Library/Application Support/app.westplay`, Tauri's for the app's identifier).
pub fn app_data() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(PathBuf::from(home).join("Library").join("Application Support").join("app.westplay"))
}

/// Being the one WestPlay running for this user: two would both drive the game's bridge (taking turns at its
/// typed-key queue, which has room for one reader, and flipping the game between their frames). An exclusive lock
/// on a file of WestPlay's, held for as long as the app runs; a WestPlay that's killed lets go with its process.
pub struct Instance(#[allow(dead_code)] std::fs::File);

impl Instance {
    /// Claims being the one WestPlay, or `None` if another is running.
    pub fn claim() -> Option<Self> {
        Self::claim_named("WestPlay.Instance")
    }

    pub(crate) fn claim_named(name: &str) -> Option<Self> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        let path = temp_dir().join(format!("{name}.lock"));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .ok()?;
        // SAFETY: an open descriptor; a lock that's not to be had is refused at once.
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        locked.then_some(Self(file))
    }
}

/// The user's own temporary folder (the one `TMPDIR` names), whatever this process's environment says.
fn temp_dir() -> PathBuf {
    let mut buffer = [0u8; 1024];
    // SAFETY: the buffer's length is passed with it; `confstr` writes a nul-terminated string within it.
    let len = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buffer.as_mut_ptr().cast(), buffer.len()) };
    if len > 1 && len <= buffer.len() {
        return PathBuf::from(String::from_utf8_lossy(&buffer[..len - 1]).into_owned());
    }
    std::env::temp_dir()
}

/// Tells the player WestPlay is already running (a second one was started), in an alert.
pub fn tell_already_running() {
    use objc2_app_kit::NSAlert;
    run_on_main(|mtm| {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str("WestPlay is already running."));
        alert.setInformativeText(&NSString::from_str("Find it in the Dock or the menu bar."));
        alert.runModal();
    });
}

/// Whether the process `game` (its pid; 0: no game) has a window showing: on screen, on this Space, not minimised
/// (macOS's list of on-screen windows has only those), in front or not. Without a game, it counts as showing
/// (nothing to be away from).
pub fn is_shown(game: u32) -> bool {
    use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
    use objc2_core_graphics::{
        CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowLayer, kCGWindowOwnerPID,
    };
    if game == 0 {
        return true;
    }
    // Only windows' owners and layers are read: no Screen Recording permission is asked for.
    let Some(list) = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        return false;
    };
    // SAFETY: the list holds dictionaries, as Core Graphics documents.
    let list: CFRetained<CFArray<CFDictionary<CFString, CFType>>> = unsafe { CFRetained::cast_unchecked(list) };
    let number = |window: &CFDictionary<CFString, CFType>, key: &CFString| -> Option<i64> {
        window.get(key)?.downcast::<CFNumber>().ok()?.as_i64()
    };
    // SAFETY: Core Graphics' own constants.
    let (owner, layer) = unsafe { (kCGWindowOwnerPID, kCGWindowLayer) };
    list.iter().any(|window| number(&window, owner) == Some(i64::from(game)) && number(&window, layer) == Some(0))
}

/// Whether the process `game` (its pid) runs as another user than WestPlay (as root, say): then macOS keeps
/// WestPlay out of it (its shared memory among them), as Windows keeps it out of a game run as administrator, and
/// the player is told. A process WestPlay can't look at counts as elevated: it's kept out all the same.
pub fn is_elevated(game: u32) -> bool {
    if game == 0 {
        return false;
    }
    // Its owner, from the kernel's short record of it (readable for any process, root's too).
    // SAFETY: plain data, filled in by `proc_pidinfo`.
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdshortinfo>() as libc::c_int;
    // SAFETY: `info` is a local of the size passed with it.
    let read = unsafe {
        libc::proc_pidinfo(game as libc::c_int, libc::PROC_PIDT_SHORTBSDINFO, 0, (&raw mut info).cast(), size)
    };
    if read != size {
        return is_running(game);
    }
    // SAFETY: a plain query.
    info.pbsi_uid != unsafe { libc::geteuid() }
}

/// Whether the process `game` (its pid; 0: no game) is running: the plugin clears its pid as the game closes,
/// and a game that crashed leaves it behind, its process gone.
pub fn is_running(game: u32) -> bool {
    if game == 0 {
        return false;
    }
    // SAFETY: signal 0 only asks whether the process is there (another user's answers "not permitted").
    let answer = unsafe { libc::kill(game as libc::pid_t, 0) };
    answer == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Whether the process `pid` has one of the modules `names` loaded: TruckersMP's client, which has no macOS
/// version (docs/MACOS_PORT.md), so never.
pub fn has_module(_pid: u32, _names: &[&str]) -> bool {
    false
}

/// Runs what the main thread left for this one: the callbacks of scripts the host ran (`webview::pump`).
pub fn pump_messages() {
    crate::webview::macos::pump();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_the_time_memory_and_the_processs_age() {
        let now = local_time();
        assert!(now.hour < 24 && now.minute < 60 && now.second < 61 && now.millisecond < 1000);
        let (free, total) = memory().unwrap();
        assert!(total > 1 << 30 && free <= total);
        assert!(process_memory().unwrap() > 0);
        assert!(uptime_ms().unwrap() < 24 * 3600 * 1000);
    }

    #[test]
    fn one_westplay_at_a_time() {
        let name = format!("WestPlay.Instance.Test.{}", std::process::id());
        let first = Instance::claim_named(&name).expect("the first claims it");
        assert!(Instance::claim_named(&name).is_none(), "a second must not");
        drop(first);
        assert!(Instance::claim_named(&name).is_some(), "once let go, it can be claimed again");
        let _ = std::fs::remove_file(temp_dir().join(format!("{name}.lock")));
    }

    #[test]
    fn knows_a_process_running_and_by_whom() {
        let me = std::process::id();
        assert!(is_running(me) && !is_running(0));
        assert!(!is_elevated(me) && !is_elevated(0));
        // launchd, root's.
        assert!(is_running(1) && is_elevated(1));
        assert!(is_shown(0));
    }

    #[test]
    fn names_keys_as_a_mac_keyboard_has_them() {
        assert_eq!(key_name(0xa4).as_deref(), Some("Left Option"));
        assert_eq!(key_name(0x51).as_deref(), Some("Q"));
        assert_eq!(key_name(0x64).as_deref(), Some("Num 4"));
        assert_eq!(key_name(0x7b).as_deref(), Some("F12"));
        // Mouse buttons have no key's name.
        assert_eq!(key_name(0x05), None);
    }
}
