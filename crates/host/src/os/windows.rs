//! The host's small OS calls on Windows.

use std::path::PathBuf;

use windows::Win32::Foundation::{FILETIME, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTimeAsFileTime};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage};

use super::LocalTime;

/// The local time now.
pub fn local_time() -> LocalTime {
    // SAFETY: a plain query of the time.
    let now = unsafe { GetLocalTime() };
    LocalTime {
        hour: now.wHour.into(),
        minute: now.wMinute.into(),
        second: now.wSecond.into(),
        millisecond: now.wMilliseconds.into(),
    }
}

/// Milliseconds since this process was created.
pub fn uptime_ms() -> Option<u64> {
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    // SAFETY: this process's own times, into locals.
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
    // SAFETY: a plain query of the time.
    let now = unsafe { GetSystemTimeAsFileTime() };
    // FILETIMEs count 100 ns steps.
    let ticks = |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Some(ticks(now).saturating_sub(ticks(created)) / 10_000)
}

/// The text on the PC's clipboard, if it holds any.
pub fn clipboard_text() -> Option<String> {
    // SAFETY: the clipboard is opened and closed on this thread around its data, which is read only while
    // locked and only within the block's own size (its text ends at a nul, but another program's need not).
    unsafe {
        OpenClipboard(None).ok()?;
        let text = (|| {
            let HANDLE(data) = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let memory = HGLOBAL(data);
            let start = GlobalLock(memory) as *const u16;
            if start.is_null() {
                return None;
            }
            let units = GlobalSize(memory) / size_of::<u16>();
            let length = (0..units).take_while(|&i| *start.add(i) != 0).count();
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(start, length));
            let _ = GlobalUnlock(memory);
            Some(text)
        })();
        let _ = CloseClipboard();
        text
    }
}

/// Opens `url` in the player's default browser; why not, if it didn't.
pub fn open_url(url: &str) -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR, w};
    let address = HSTRING::from(url);
    // SAFETY: the operation and the address are null-terminated wide strings that outlive the call; the
    // parameters and the folder are null (none), and no window owns it.
    let result = unsafe { ShellExecuteW(None, w!("open"), &address, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    // ShellExecute's convention: above 32 is success, the rest an error code.
    let code = result.0 as usize;
    if code <= 32 { Err(format!("ShellExecute {code}")) } else { Ok(()) }
}

/// The memory WestPlay has taken (its private bytes, what it holds whether in RAM or paged out).
pub fn process_memory() -> Option<u64> {
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    let mut counters =
        PROCESS_MEMORY_COUNTERS_EX { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    // SAFETY: `counters` is a local with its size set, read as the larger form the size says it is.
    unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(), counters.cb)
    }
    .ok()?;
    Some(counters.PrivateUsage as u64)
}

/// The PC's memory: free now, and in all (bytes); `None` if Windows doesn't say.
pub fn memory() -> Option<(u64, u64)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `status` is a local with its size set, as the call requires.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some((status.ullAvailPhys, status.ullTotalPhys))
}

/// A keyboard key's name as Windows gives it for the player's keyboard ("Right Ctrl", "Num 4", "Q"), by its
/// virtual-key code. `None` for a mouse button, or a key Windows has no name for.
pub fn key_name(code: u32) -> Option<String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyNameTextW, MAPVK_VK_TO_VSC_EX, MapVirtualKeyW};
    // SAFETY: plain queries; the buffer is a local the call writes within its length.
    unsafe {
        // The scan code, 0xE0xx for an extended key (right Ctrl and Alt, the arrows, Insert...).
        let scancode = MapVirtualKeyW(code, MAPVK_VK_TO_VSC_EX);
        if scancode == 0 {
            return None;
        }
        let extended = i32::from(scancode & 0xff00 == 0xe000);
        let lparam = ((scancode & 0xff) as i32) << 16 | extended << 24;
        let mut name = [0u16; 64];
        let length = usize::try_from(GetKeyNameTextW(lparam, &mut name)).ok().filter(|&n| n > 0)?;
        Some(String::from_utf16_lossy(&name[..length]))
    }
}

/// WestPlay's own data folder, for its log when Documents refuses it (`%LOCALAPPDATA%\app.westplay`).
pub fn app_data() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("app.westplay"))
}

/// Being the one WestPlay running in this Windows session: two would both drive the game's bridge (taking
/// turns at its typed-key queue, which has room for one reader, and flipping the game between their frames).
/// Held for as long as the app runs; a WestPlay that's killed lets go with its process.
pub struct Instance(windows::Win32::Foundation::HANDLE);

// SAFETY: a handle to a named mutex, only ever closed (once, as it goes), from whichever thread.
unsafe impl Send for Instance {}

impl Instance {
    /// Claims being the one WestPlay, or `None` if another is running.
    pub fn claim() -> Option<Self> {
        Self::claim_named(windows::core::w!(r"Local\WestPlay.Instance"))
    }

    pub(crate) fn claim_named(name: windows::core::PCWSTR) -> Option<Self> {
        use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
        // SAFETY: a plain call: the name is a static string, the mutex is the process's own.
        let handle = unsafe { windows::Win32::System::Threading::CreateMutexW(None, false, name) }.ok()?;
        // SAFETY: reads this thread's last error, right after the call it's for.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: `handle` was just opened, and nothing else holds it.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return None;
        }
        Some(Self(handle))
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the mutex is this `Instance`'s own, closed once, as it goes.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// Tells the player WestPlay is already running (a second one was started), in a message box.
pub fn tell_already_running() {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};
    // SAFETY: a message box with static strings, no owner window.
    unsafe {
        MessageBoxW(
            None,
            windows::core::w!("WestPlay is already running. Find it in the taskbar or the notification area."),
            windows::core::w!("WestPlay"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

/// Whether the process `game` (its pid; 0: no game) has a window showing: shown, not minimised, not cloaked
/// (on another virtual desktop). In front or not: Lossless Scaling's window is in front of a game it scales,
/// and a window over the game leaves it showing (owner 2026-10-28). Without a game, it counts as showing
/// (nothing to be away from).
pub fn is_shown(game: u32) -> bool {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible};
    use windows::core::BOOL;

    struct Looking {
        game: u32,
        shown: bool,
    }
    unsafe extern "system" fn each(window: HWND, looking: LPARAM) -> BOOL {
        // SAFETY: `looking` is the `Looking` below, alive through `EnumWindows`; the window is only asked about.
        unsafe {
            let looking = &mut *(looking.0 as *mut Looking);
            let mut owner = 0;
            GetWindowThreadProcessId(window, Some(&mut owner));
            if owner != looking.game || !IsWindowVisible(window).as_bool() || IsIconic(window).as_bool() {
                return BOOL(1);
            }
            let mut cloaked = 0u32;
            let read = DwmGetWindowAttribute(window, DWMWA_CLOAKED, (&raw mut cloaked).cast(), size_of::<u32>() as u32);
            looking.shown = read.is_err() || cloaked == 0;
            // Found one showing: done.
            BOOL(i32::from(!looking.shown))
        }
    }

    if game == 0 {
        return true;
    }
    let mut looking = Looking { game, shown: false };
    // SAFETY: `each` only reads the windows and writes `looking`, which outlives the call. Stopping early makes
    // it return an error: what was found stands.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&raw mut looking as isize)) };
    looking.shown
}

/// Whether the process `game` (its pid) runs as administrator (elevated), as Task Manager's "Elevated" says: then
/// Windows keeps WestPlay out of it, and the game's route, guidance and weather can't be read. WestPlay doesn't
/// work with a game run that way (owner 2026-10-28: nothing that reaches into an elevated game); the player is
/// told. A token WestPlay isn't let read counts as elevated: it's kept out all the same.
pub fn is_elevated(game: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows::Win32::System::Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
    if game == 0 {
        return false;
    }
    // SAFETY: handles opened here and closed once; `elevation` and `length` are locals the call writes, its size
    // given.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, game) else { return false };
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token);
        let _ = CloseHandle(process);
        if let Err(error) = opened {
            return error.code() == E_ACCESSDENIED;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut length = 0;
        let read = GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        );
        let _ = CloseHandle(token);
        read.is_ok() && elevation.TokenIsElevated != 0
    }
}

/// Whether the process `game` (its pid; 0: no game) is running: the plugin clears its pid as the game closes,
/// and a game that crashed leaves it behind, its process gone.
pub fn is_running(game: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    if game == 0 {
        return false;
    }
    // SAFETY: a handle opened here, waited on without waiting (signalled: the process has ended), and closed.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, game) else { return false };
        let running = WaitForSingleObject(process, 0) == WAIT_TIMEOUT;
        let _ = CloseHandle(process);
        running
    }
}

/// Whether the process `pid` has one of the modules `names` loaded (names compared ignoring case). Only the
/// game's list of modules is looked at, nothing in them.
pub fn has_module(pid: u32, names: &[&str]) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE,
    };
    if pid == 0 {
        return false;
    }
    // SAFETY: the snapshot is closed once after use; `entry` is a local with its size set, as the calls require.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) else { return false };
        let mut entry = MODULEENTRY32W { dwSize: size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
        let mut found = false;
        let mut more = Module32FirstW(snapshot, &mut entry).is_ok();
        while more && !found {
            let length = entry.szModule.iter().position(|&c| c == 0).unwrap_or(entry.szModule.len());
            let module = String::from_utf16_lossy(&entry.szModule[..length]);
            found = names.iter().any(|name| module.eq_ignore_ascii_case(name));
            more = Module32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

/// Passes on this thread's window messages (WebView2's events come with them).
pub fn pump_messages() {
    // SAFETY: this thread's own messages, each passed on as Windows hands it; `msg` is a local.
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
