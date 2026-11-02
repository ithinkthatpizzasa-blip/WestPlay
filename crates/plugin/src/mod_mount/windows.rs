//! The mod mount's pieces in the Windows game (1.58-1.61): found in its executable as mapped (a PE image).

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
use windows::core::PCWSTR;

use super::Found;
use crate::navigation::Image;

/// The game's mount, found by its start (`+0xF17E0` in 1.61.1.9, `+0xECB20` in 1.60.1.7, the same in both but
/// for the bytes masked: the addresses of what it calls and of the game's globals, and a jump's length). Kept:
/// its frame, and the request's fields it reads (`+0x8`, `+0x10`, `+0x0`), so a request laid out otherwise
/// doesn't match. Every build known reads the request's fields at the same offsets throughout (1.61.1.9,
/// 1.60.1.7, the VR branch's 1.61.1.1009 at `+0xF1A30`: checked 2026-10-23).
const MOUNT: &str = "48 89 54 24 10 53 48 81 ec 90 01 00 00 4c 8b 0d ?? ?? ?? ?? 48 8b d9 41 80 39 00 74 ?? 48 8b 41 08 \
                     4c 8d 05 ?? ?? ?? ?? 48 8d 4c 24 50 48 89 44 24 20 33 d2 e8 ?? ?? ?? ?? 48 8b 43 10 48 8d 15 \
                     ?? ?? ?? ?? 8b 0b 4c 8d 8c 24 a8 01 00 00 48 85 c0 c7 44 24 40 ff ff ff ff 4c 8d 44 24 58";
/// The game's pool by index (the mount takes the pool with it: `+0x153780` in 1.61.1.9, `+0x14F2A0` in
/// 1.60.1.7), its bounds check and the assert after it (its line number masked).
const POOL: &str = "48 83 ec 48 48 63 d1 48 3b 15 ?? ?? ?? ?? 73 10 48 8b 05 ?? ?? ?? ?? 48 8b 04 d0 48 83 c4 48 c3 \
                    48 8d 0d ?? ?? ?? ?? e8 ?? ?? ?? ?? 48 89 44 24 30 4c 8d 0d ?? ?? ?? ?? 48 89 54 24 28 48 8d 05 \
                    ?? ?? ?? ?? ba ?? ?? 00 00 48 89 44 24 20";
/// The rebuild of a pool's index (`+0x158BC0` in 1.61.1.9, `+0x1546E0` in 1.60.1.7, compiled differently in
/// each): the function holding the one load of the line it logs, which reads the pool's parts at these offsets
/// as it starts (`lea r, [rcx + E8h / 138h / 160h]`, in both builds).
const INDEX_LOG: &[u8] = b"[ufs] %s pool: %Iu packs, %Iu files, %Iu dirs\0";
const INDEX_READS: [[u8; 4]; 3] = [[0xe8, 0x00, 0x00, 0x00], [0x38, 0x01, 0x00, 0x00], [0x60, 0x01, 0x00, 0x00]];

/// A pattern's bytes, `??` any byte.
fn pattern(text: &str) -> Vec<Option<u8>> {
    text.split_whitespace().map(|byte| u8::from_str_radix(byte, 16).ok()).collect()
}

/// The lines a game with a pool index logs about it. A game logging neither (1.58.1.4) has none: it looks a
/// pool's files up device by device (its mount logs `[fs] device %s mounted.`, no pool), so there's no index to
/// rebuild after the mount.
const INDEX_SIGNS: [&[u8]; 2] = [INDEX_LOG, b"[ufs] disabling pool lookup"];

/// What the mount needs in `image`: `None` unless each is found exactly once (another game whose code differs is
/// left alone, as is one with a pool index whose rebuild isn't found).
fn locate(image: &Image) -> Option<Found> {
    let mount = image.unique_masked(&pattern(MOUNT))?;
    if INDEX_SIGNS.iter().all(|sign| image.constant(sign).is_none()) {
        return Some(Found { mount, index: None });
    }
    let pool = image.unique_masked(&pattern(POOL))?;
    let index = image.function_start(image.unique_load(image.constant(INDEX_LOG)?)?)?;
    let start = image.code_at(index, 0x60)?;
    INDEX_READS
        .iter()
        .all(|read| start.windows(4).any(|w| w == read))
        .then_some(Found { mount, index: Some((pool, index)) })
}

/// Whether this is the first time the plugin mounts in this game process: a mutex named for the process, left open
/// (so it lasts as long as the process), says whether an earlier load of the plugin did.
pub fn first_in_process() -> bool {
    let name: Vec<u16> =
        // SAFETY: a plain query of this process.
        format!("Local\\WestPlay.ModMounted.{}", unsafe { GetCurrentProcessId() }).encode_utf16().chain([0]).collect();
    // SAFETY: a mutex made by name (a nul-terminated string alive through the call), its handle kept open.
    unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())).is_ok() && GetLastError() != ERROR_ALREADY_EXISTS }
}

/// Where the game's executable is mapped, and what the mount needs of it.
///
/// # Safety
/// In the game's process.
pub unsafe fn found() -> Option<(usize, Found)> {
    // SAFETY: no name: the game's own executable, mapped for the process's life.
    let base = unsafe { GetModuleHandleW(None) }.ok()?.0 as usize;
    // SAFETY: `base` is the game's executable, as mapped.
    let found = unsafe { Image::of(base) }.as_ref().and_then(locate)?;
    Some((base, found))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A game executable laid out as Windows maps it: its headers, then each section at its address.
    fn mapped(file: &[u8]) -> Vec<u8> {
        let u16_at = |at: usize| u16::from_le_bytes([file[at], file[at + 1]]) as usize;
        let u32_at = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        let header = u32_at(0x3c);
        let (count, optional) = (u16_at(header + 6), u16_at(header + 20));
        let mut image = vec![0u8; u32_at(header + 24 + 56)];
        let headers = u32_at(header + 24 + 60);
        image[..headers].copy_from_slice(&file[..headers]);
        for i in 0..count {
            let at = header + 24 + optional + i * 40;
            let (address, raw_size, raw) = (u32_at(at + 12), u32_at(at + 16), u32_at(at + 20));
            let size = raw_size.min(image.len() - address).min(file.len() - raw);
            image[address..address + size].copy_from_slice(&file[raw..raw + size]);
        }
        image
    }

    /// The mount, the pools and their index found in each game build named by `WESTPLAY_GAME_EXES` (paths to
    /// `eurotrucks2.exe`, separated by `;`), at the places they were read in the builds known: 1.61.1.9, 1.60.1.7,
    /// the VR branch's 1.61.1.1009 (the same functions, at other addresses) and 1.58.1.4 (its mount alone: its
    /// pools have no index).
    #[test]
    #[ignore = "needs game executables: WESTPLAY_GAME_EXES"]
    fn finds_the_mount_in_each_game_build() {
        const KNOWN: [(usize, Option<(usize, usize)>); 4] = [
            (0xF17E0, Some((0x153780, 0x158BC0))),
            (0xECB20, Some((0x14F2A0, 0x1546E0))),
            (0xF1A30, Some((0x1539D0, 0x158E10))),
            (0x21BB30, None),
        ];
        let exes = std::env::var("WESTPLAY_GAME_EXES").expect("WESTPLAY_GAME_EXES");
        for exe in exes.split(';').filter(|exe| !exe.is_empty()) {
            let image = mapped(&std::fs::read(exe).unwrap());
            let base = image.as_ptr() as usize;
            // SAFETY: `image` is laid out as a mapped PE image, alive through the search.
            let found = unsafe { Image::of(base) }
                .as_ref()
                .and_then(locate)
                .map(|found| (found.mount - base, found.index.map(|(pool, index)| (pool - base, index - base))));
            println!("{exe}: {found:x?}");
            let found = found.expect("the mount isn't found");
            if let Some(known) = KNOWN.iter().find(|known| known.0 == found.0) {
                assert_eq!(found, *known);
            }
        }
    }
}
