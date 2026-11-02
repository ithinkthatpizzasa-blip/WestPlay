//! The mod mount's pieces in the Mac game (1.57.2.7, Apple's compiler): found in its executable as loaded (a Mach-O).
//!
//! 1.57 has the 1.6x mount, taking the same request at the same offsets (`+0x1AACC70` in 1.57.2.7s): it makes the
//! device (kind 3: HashFS, else zip, `extra` under 3; the game's own archives are kind 4, HashFS alone) and adds it to
//! the pool. Its pools have no index (its mount logs `[fs] device %s mounted.`, as 1.58 on Windows), so there's
//! nothing to rebuild after (docs/MACOS_PORT.md).

use std::ffi::c_void;

use super::Found;
use crate::navigation::Image;

/// The mount's start: its frame, the request kept (`mov rbx, rdi`), the game's path prefix tested
/// (`cmp byte [rcx], 0`), then the request's path read (`mov r8, [rbx+8]`) (`None`: any byte, an address or a jump).
const MOUNT: &[Option<u8>] = &[
    Some(0x55),
    Some(0x48),
    Some(0x89),
    Some(0xe5),
    Some(0x53),
    Some(0x48),
    Some(0x81),
    Some(0xec),
    Some(0x28),
    Some(0x01),
    Some(0x00),
    Some(0x00),
    Some(0x48),
    Some(0x89),
    Some(0xfb),
    Some(0x48),
    Some(0x8b),
    Some(0x05),
    None,
    None,
    None,
    None,
    Some(0x48),
    Some(0x8b),
    Some(0x00),
    Some(0x48),
    Some(0x89),
    Some(0x45),
    Some(0xf0),
    Some(0x48),
    Some(0x8d),
    Some(0x05),
    None,
    None,
    None,
    None,
    Some(0x48),
    Some(0x8b),
    Some(0x08),
    Some(0x80),
    Some(0x39),
    Some(0x00),
    Some(0x74),
    None,
    Some(0x4c),
    Some(0x8b),
    Some(0x43),
    Some(0x08),
];

/// The request's other fields, read soon after (`movzx r8d, byte [rbx+18h]`, `mov r9d, [rbx+1Ch]`,
/// `mov r10d, [rbx+24h]`): the request is laid out as the plugin's.
const FIELD_READS: [&[u8]; 3] = [&[0x44, 0x0f, 0xb6, 0x43, 0x18], &[0x44, 0x8b, 0x4b, 0x1c], &[0x44, 0x8b, 0x53, 0x24]];

/// How far after its start the mount reads the request's fields.
const FIELDS_WITHIN: usize = 0xd0;

/// The environment variable an earlier load of the plugin set in this process, once it mounted (the process's
/// environment outlives the plugin, `sdk reload`).
const MOUNTED: &str = "WESTPLAY_MOD_MOUNTED";

unsafe extern "C" {
    fn _dyld_get_image_header(index: u32) -> *const c_void;
}

/// Whether this is the first time the plugin mounts in this game process.
pub fn first_in_process() -> bool {
    if std::env::var_os(MOUNTED).is_some() {
        return false;
    }
    // SAFETY: set on the game's main thread, at the plugin's init, before the plugin starts threads of its own that
    // read the environment (its memory reader reads none); the game reads its environment at start.
    unsafe { std::env::set_var(MOUNTED, "1") };
    true
}

/// Where the game's executable is loaded, and what the mount needs of it.
///
/// # Safety
/// In the game's process.
pub unsafe fn found() -> Option<(usize, Found)> {
    // SAFETY: image 0 is the process's executable, loaded for its life.
    let image = unsafe { Image::of(_dyld_get_image_header(0) as usize) }?;
    let found = locate(&image)?;
    Some((image.base, found))
}

/// The mount in `image`: found once, and reading the request's fields where the plugin's request has them.
fn locate(image: &Image) -> Option<Found> {
    let mount = image.unique(MOUNT)?;
    let code = image.code_at(mount, FIELDS_WITHIN)?;
    FIELD_READS.iter().all(|read| code.windows(read.len()).any(|w| w == *read)).then_some(Found { mount, index: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mount found in the Mac game's executable named by `WESTPLAY_MAC_GAME` (1.57.2.7s: `+0x1AACC70`, read
    /// 2026-11-02). Its `__TEXT` starts its file, so its bytes there lie as loaded.
    #[test]
    #[ignore = "needs the Mac game's executable: WESTPLAY_MAC_GAME"]
    fn finds_the_mount_in_the_mac_game() {
        let file = std::fs::read(std::env::var("WESTPLAY_MAC_GAME").expect("WESTPLAY_MAC_GAME")).unwrap();
        let be = |at: usize| u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        let image = if be(0) == 0xcafe_babe {
            let arch = (0..be(4)).map(|i| 8 + i * 20).find(|&at| be(at) == 0x0100_0007).unwrap();
            file[be(arch + 8)..be(arch + 8) + be(arch + 12)].to_vec()
        } else {
            file
        };
        // SAFETY: the executable's bytes, laid out as loaded for its `__TEXT`, alive through the test.
        let image = unsafe { Image::of(image.as_ptr() as usize) }.expect("the image isn't read");
        let found = locate(&image).expect("no mount");
        assert_eq!((found.mount - image.base, found.index), (0x1aacc70, None));
    }
}
