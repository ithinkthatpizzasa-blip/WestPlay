//! Shared memory on Windows: a named file mapping backed by the paging file.

use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
};
use windows::core::HSTRING;

pub type Error = windows::core::Error;

/// A view of a named mapping. Whichever process maps it first creates it (zeroed).
pub struct Region {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

// SAFETY: the mapping's handle and view may be used from any thread; what's in it is shared through atomics.
unsafe impl Send for Region {}
// SAFETY: as for `Send`.
unsafe impl Sync for Region {}

impl Region {
    /// The mapping `name` ([`super::shared_name`]), `size` bytes.
    pub fn open(name: &str, size: usize) -> Result<Self, Error> {
        // SAFETY: a mapping of `size` bytes, viewed whole; it's closed if it can't be viewed.
        unsafe {
            let handle =
                CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, size as u32, &HSTRING::from(name))?;
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size);
            if view.Value.is_null() {
                let error = windows::core::Error::from_thread();
                let _ = CloseHandle(handle);
                return Err(error);
            }
            Ok(Self { handle, view })
        }
    }

    /// The start of the view, mapped as long as `self`.
    pub fn as_ptr(&self) -> *mut u8 {
        self.view.Value as *mut u8
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: the view and the handle are this region's own, unmapped and closed once, as it goes.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}
