//! Shared memory on macOS: POSIX shared memory (`shm_open`, `ftruncate`, `mmap`).
//!
//! macOS keeps a POSIX shared-memory object until it's unlinked, through every process that held it going (the
//! game crashing included), where Windows frees a mapping with its last handle. So that a region is new, zeroed,
//! exactly when Windows' would be, each holder keeps a shared `flock` on a lock file beside it for as long as it
//! has the region mapped (the system drops it with the process, however that ends): an opener that can lock it
//! exclusively is alone, and starts the region afresh. A second lock file, held only while opening, keeps two
//! openers from both deciding they're alone.

use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

pub type Error = io::Error;

/// A mapping of a named region.
pub struct Region {
    ptr: *mut u8,
    size: usize,
    /// The shared lock that says this process holds the region, held as long as it's mapped.
    _holder: File,
}

// SAFETY: the mapping may be used from any thread; what's in it is shared through atomics.
unsafe impl Send for Region {}
// SAFETY: as for `Send`.
unsafe impl Sync for Region {}

impl Region {
    /// The region `name` ([`super::shared_name`]), `size` bytes: zeroed if no other process holds it.
    pub fn open(name: &str, size: usize) -> Result<Self, Error> {
        if name.len() > super::MAX_NAME_LEN || !name.starts_with('/') || name[1..].contains('/') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("not a shared-memory name: {name}")));
        }
        let c_name = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let locks = lock_dir();
        let base = &name[1..];
        // Held until this function returns: no other opener decides it's alone meanwhile.
        let gate = lock_file(locks.join(format!("{base}.open")))?;
        flock(&gate, libc::LOCK_EX)?;
        let holder = lock_file(locks.join(format!("{base}.holders")))?;
        if flock(&holder, libc::LOCK_EX | libc::LOCK_NB).is_ok() {
            // Alone: whatever is left is from processes gone. Started afresh, as Windows would.
            // SAFETY: a nul-terminated name; unlinking one that doesn't exist only fails.
            unsafe { libc::shm_unlink(c_name.as_ptr()) };
        }
        // SAFETY: a nul-terminated name; the descriptor is closed below, the mapping keeping the memory.
        let fd = unsafe { libc::shm_open(c_name.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o600 as libc::c_uint) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mapped = size_to(fd, size).and_then(|()| {
            // SAFETY: a new mapping of the object's first `size` bytes, which it has (`size_to`).
            let ptr = unsafe {
                libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0)
            };
            if ptr == libc::MAP_FAILED { Err(io::Error::last_os_error()) } else { Ok(ptr as *mut u8) }
        });
        // SAFETY: the descriptor is ours, closed once; a mapping outlives it.
        unsafe { libc::close(fd) };
        let ptr = mapped?;
        // From exclusive (if it was) to shared: the gate still keeps other openers out while it changes.
        if let Err(error) = flock(&holder, libc::LOCK_SH) {
            // SAFETY: the mapping just made, unmapped once.
            unsafe { libc::munmap(ptr.cast(), size) };
            return Err(error);
        }
        drop(gate);
        Ok(Self { ptr, size, _holder: holder })
    }

    /// The start of the mapping, mapped as long as `self`.
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: the mapping is this region's own, unmapped once, as it goes (its lock goes after it).
        unsafe { libc::munmap(self.ptr.cast(), self.size) };
    }
}

/// Gives the object behind `fd` at least `size` bytes. macOS lets a shared-memory object be sized once only:
/// another process may have sized it first (a second `ftruncate` fails then), and its size stays.
fn size_to(fd: libc::c_int, size: usize) -> io::Result<()> {
    let current = || -> io::Result<usize> {
        // SAFETY: `stat` is plain data, filled in by `fstat` on a descriptor that is open.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: as above.
        if unsafe { libc::fstat(fd, &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(stat.st_size as usize)
    };
    if current()? == 0 {
        // SAFETY: an open descriptor; a new object reads as zeros.
        unsafe { libc::ftruncate(fd, size as libc::off_t) };
    }
    let actual = current()?;
    if actual < size {
        return Err(io::Error::other(format!("shared memory is {actual} bytes, {size} needed")));
    }
    Ok(())
}

fn flock(file: &File, operation: libc::c_int) -> io::Result<()> {
    loop {
        // SAFETY: an open descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn lock_file(path: PathBuf) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(path)
}

/// Where the lock files are: the user's own temporary folder (`confstr(_CS_DARWIN_USER_TEMP_DIR)`, the one
/// `TMPDIR` names), the same for the game and the app whatever their environments, as both run as the user.
fn lock_dir() -> PathBuf {
    let mut buffer = [0u8; 1024];
    // SAFETY: the buffer's length is passed with it; `confstr` writes a nul-terminated string within it.
    let len = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buffer.as_mut_ptr().cast(), buffer.len()) };
    if len > 1 && len <= buffer.len() {
        let path = String::from_utf8_lossy(&buffer[..len - 1]).into_owned();
        return PathBuf::from(path);
    }
    std::env::temp_dir()
}
