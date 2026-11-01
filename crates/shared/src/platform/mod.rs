//! Named shared memory, each OS's own: a [`Region`] of a given size, mapped by every process that opens the same
//! name, zeroed when it's new. "New" is the same on both: no process holds it any more (Windows frees a
//! mapping with its last handle; on macOS, which keeps POSIX shared memory until it's unlinked, the last holder's
//! lock going with its process says so).

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{Error, Region};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::{Error, Region};

/// The name of the shared memory `base` (e.g. `WestPlay.Bridge.v24`) as the OS takes it: in the session's own
/// namespace on Windows, a POSIX shared-memory name on macOS (`/` first, at most [`MAX_NAME_LEN`] bytes long).
pub fn shared_name(base: &str) -> String {
    if cfg!(windows) { format!("Local\\{base}") } else { format!("/{base}") }
}

/// The longest POSIX shared-memory name macOS takes (`PSHMNAMLEN`, its `/` included): a longer one is refused
/// with `ENAMETOOLONG`.
pub const MAX_NAME_LEN: usize = 31;

#[cfg(test)]
mod tests {
    use super::*;

    /// A name of the test's own, apart from WestPlay's and from other tests'.
    fn test_name(test: &str) -> String {
        shared_name(&format!("WPT.{test}.{}", std::process::id()))
    }

    fn first_byte(region: &Region) -> u8 {
        // SAFETY: the region has at least one byte, mapped as long as `region`.
        unsafe { region.as_ptr().read_volatile() }
    }

    fn set_first_byte(region: &Region, value: u8) {
        // SAFETY: as in `first_byte`.
        unsafe { region.as_ptr().write_volatile(value) }
    }

    #[test]
    fn a_region_lasts_while_anyone_holds_it_and_starts_afresh_after() {
        let name = test_name("lasts");
        let a = Region::open(&name, 4096).unwrap();
        assert_eq!(first_byte(&a), 0);
        set_first_byte(&a, 7);
        let b = Region::open(&name, 4096).unwrap();
        assert_eq!(first_byte(&b), 7);
        // The first holder going leaves it to the second, as it was.
        drop(a);
        let c = Region::open(&name, 4096).unwrap();
        assert_eq!(first_byte(&c), 7);
        drop(b);
        drop(c);
        // No one holds it: new, zeroed.
        let d = Region::open(&name, 4096).unwrap();
        assert_eq!(first_byte(&d), 0);
        drop(d);
        // macOS keeps it until unlinked: not left behind by the test.
        #[cfg(target_os = "macos")]
        {
            let name = std::ffi::CString::new(name).unwrap();
            // SAFETY: a nul-terminated name.
            unsafe { libc::shm_unlink(name.as_ptr()) };
        }
    }

    #[test]
    fn names_are_the_oss_own() {
        #[cfg(windows)]
        assert_eq!(shared_name("WestPlay.X"), "Local\\WestPlay.X");
        #[cfg(not(windows))]
        assert_eq!(shared_name("WestPlay.X"), "/WestPlay.X");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_name_macos_wouldnt_take_is_refused() {
        let long = shared_name(&"W".repeat(MAX_NAME_LEN));
        assert!(Region::open(&long, 4096).is_err());
    }
}
