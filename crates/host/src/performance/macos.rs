//! The processes WestPlay runs on macOS, and how much of the processor each took: the app's own, and WebKit's (its
//! pages', GPU and network processes), which macOS starts for it and counts as WestPlay's responsibility (they're
//! no children of the app). WebKit doesn't say which page a process draws, so they're named by kind.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CStr;
use std::rc::Rc;
use std::sync::OnceLock;

use super::Profile;

/// A profile, for asking its processes: WebKit's processes are told apart only by kind (`Namer`).
pub type Environment = crate::webview::Environment;

/// WebKit doesn't say which page each of its processes draws: nothing to ask.
pub fn ask_parts(
    _parts: &Rc<RefCell<HashMap<u32, &'static str>>>,
    _cab_host: &str,
    _profile: Profile,
    _environment: &Environment,
) {
}

/// macOS's own `responsibility_get_pid_responsible_for_pid` (libquarantine, not in its headers): the process a
/// helper process is run for. Looked up when first needed; without it, only the app is counted.
fn responsible_for(pid: u32) -> Option<u32> {
    type Responsible = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;
    static LOOKUP: OnceLock<Option<Responsible>> = OnceLock::new();
    let lookup = LOOKUP.get_or_init(|| {
        // SAFETY: a symbol looked up by name in what's loaded; its signature is macOS's (a pid in, a pid out).
        unsafe {
            let symbol = libc::dlsym(libc::RTLD_DEFAULT, c"responsibility_get_pid_responsible_for_pid".as_ptr());
            (!symbol.is_null()).then(|| std::mem::transmute::<*mut libc::c_void, Responsible>(symbol))
        }
    });
    // SAFETY: macOS's call, any pid in.
    let responsible = unsafe { (lookup.as_ref()?)(pid as libc::pid_t) };
    (responsible > 0).then_some(responsible as u32)
}

/// WestPlay's processes: its own and the ones macOS runs for it.
pub fn processes() -> Vec<u32> {
    let ours = std::process::id();
    let mut found = vec![ours];
    // SAFETY: with a null buffer, `proc_listallpids` only counts.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return found;
    }
    let mut pids = vec![0 as libc::pid_t; count as usize + 64];
    let size = (pids.len() * size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: the buffer is ours and `size` its length in bytes.
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
    pids.truncate(count.max(0) as usize);
    found.extend(
        pids.into_iter()
            .map(|pid| pid as u32)
            .filter(|&pid| pid != ours && pid != 0 && responsible_for(pid) == Some(ours)),
    );
    found
}

/// Each process's processor time so far (100 ns units, as `cores_by_part` counts); one that can't be read is left
/// out.
pub fn processor_times(pids: &[u32]) -> HashMap<u32, u64> {
    pids.iter()
        .filter_map(|&pid| {
            // SAFETY: plain data, filled in by `proc_pid_rusage`.
            let mut usage: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
            // SAFETY: `usage` is a local of the flavour asked for.
            let read = unsafe {
                libc::proc_pid_rusage(
                    pid as libc::c_int,
                    libc::RUSAGE_INFO_V4,
                    (&raw mut usage).cast::<libc::rusage_info_t>(),
                )
            };
            (read == 0).then(|| (pid, mach_to_100ns(usage.ri_user_time + usage.ri_system_time)))
        })
        .collect()
}

/// Mach time units (what `proc_pid_rusage` counts processor time in) in 100 ns steps.
#[allow(deprecated)]
fn mach_to_100ns(ticks: u64) -> u64 {
    static TIMEBASE: OnceLock<(u64, u64)> = OnceLock::new();
    let (numer, denom) = *TIMEBASE.get_or_init(|| {
        let mut info = libc::mach_timebase_info { numer: 0, denom: 0 };
        // SAFETY: a local the call fills.
        let ok = unsafe { libc::mach_timebase_info(&mut info) } == 0;
        if ok && info.denom != 0 { (u64::from(info.numer), u64::from(info.denom)) } else { (1, 1) }
    });
    (u128::from(ticks) * u128::from(numer) / u128::from(denom) / 100) as u64
}

/// What each process is: the app, or one of WebKit's by its kind.
pub struct Namer<'a> {
    ours: u32,
    _named: &'a HashMap<u32, &'static str>,
}

impl<'a> Namer<'a> {
    pub fn new(named: &'a HashMap<u32, &'static str>) -> Self {
        Self { ours: std::process::id(), _named: named }
    }

    pub fn part(&self, pid: u32) -> &'static str {
        if pid == self.ours {
            return "WestPlay app";
        }
        kind_of(&process_name(pid))
    }
}

/// A WebKit process's part by its name (`com.apple.WebKit.WebContent`…).
fn kind_of(name: &str) -> &'static str {
    if name.contains("WebContent") {
        "pages"
    } else if name.contains("GPU") {
        "GPU"
    } else if name.contains("Networking") {
        "network"
    } else {
        "other"
    }
}

/// A process's name (its executable's), as macOS keeps it.
fn process_name(pid: u32) -> String {
    let mut name = [0u8; 256];
    // SAFETY: the buffer is ours and its length passed with it; the name written in it is nul-terminated.
    let length = unsafe { libc::proc_name(pid as libc::c_int, name.as_mut_ptr().cast(), name.len() as u32) };
    if length <= 0 {
        return String::new();
    }
    CStr::from_bytes_until_nul(&name).map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_its_own_processor_time_and_names_webkits_processes() {
        let ours = std::process::id();
        assert!(processes().contains(&ours));
        let busy = std::time::Instant::now();
        while busy.elapsed().as_millis() < 50 {
            std::hint::black_box(0u64.wrapping_add(1));
        }
        let times = processor_times(&[ours]);
        // At least the 50 ms just spent, in 100 ns steps.
        assert!(times[&ours] >= 400_000, "{times:?}");
        assert_eq!(kind_of("com.apple.WebKit.WebContent"), "pages");
        assert_eq!(kind_of("com.apple.WebKit.GPU"), "GPU");
        assert_eq!(kind_of("com.apple.WebKit.Networking"), "network");
    }
}
