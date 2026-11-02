//! Changing the game's route on Windows (1.6x): its pieces found in its executable as mapped (a PE image), and its
//! navigator's four lists given at once.

use windows::Win32::System::LibraryLoader::GetModuleHandleW;

use super::{List, Planned, Target, targets};
use crate::log;

/// Offsets in the navigator.
pub const MODE: usize = 0x1c8;
pub const ROUTE: usize = 0x1f8;
const AVOID: usize = 0x220;
const WAYPOINTS: usize = 0x248;

type SetTargets = unsafe extern "system" fn(*mut u8, u32, *const List, *const List, *const List, *const List);
type FindNode = unsafe extern "system" fn(*mut u8, u64) -> *mut u8;

pub struct Game {
    /// The global holding the game's core object (the navigator's owner, and the node table's).
    pub core: *const *mut u8,
    pub navigator: usize,
    set_targets: SetTargets,
    find_node: FindNode,
}

// SAFETY: the pointers are into the game's own executable, fixed for the process's life, and only used on
// the game's main thread (`serve`).
unsafe impl Send for Game {}
// SAFETY: as for `Send`: never used from two threads at once.
unsafe impl Sync for Game {}

impl Game {
    /// The map node with this uid, from the game's node table.
    ///
    /// # Safety
    /// On the game's main thread, `core` the game's core object.
    pub unsafe fn find_node(&self, core: *mut u8, uid: u64) -> Option<usize> {
        // SAFETY: the game's own lookup, found by its shape (`locate`), with its core object.
        let node = unsafe { (self.find_node)(core, uid) };
        (!node.is_null()).then_some(node as usize)
    }
}

/// The navigator's waypoints, in the order they were added (a list of its own).
///
/// # Safety
/// `navigator` is the game's navigator.
pub unsafe fn waypoints(navigator: *mut u8, _route: &[Target], _mode: u32) -> Option<Vec<Target>> {
    // SAFETY: the navigator's list (the caller's contract).
    unsafe { targets(navigator.add(WAYPOINTS)) }
}

/// Gives the navigator its new route: the four lists at once (the first given empty, the game fills it as it plans;
/// the places to avoid kept), and it plans again.
///
/// # Safety
/// On the game's main thread, `navigator` the game's, `game` as `locate` found it.
pub unsafe fn give(game: &Game, navigator: *mut u8, planned: &Planned) {
    // SAFETY: the navigator's lists, read as the game's `array_t`; the game's own function, which copies the lists
    // given (alive through the call).
    unsafe {
        let allocator = (navigator.add(WAYPOINTS) as *const List).read().allocator;
        (game.set_targets)(
            navigator,
            planned.mode,
            &List::of(allocator, &[]),
            &List::of(allocator, &planned.route),
            navigator.add(AVOID) as *const List,
            &List::of(allocator, &planned.waypoints),
        );
    }
}

/// Finds the pieces in the running game's code; `None` (logged) if any isn't there exactly once.
///
/// # Safety
/// In the game's process (the plugin is loaded in it).
pub unsafe fn locate() -> Option<Game> {
    let found = (|| {
        // SAFETY: no name: the game's own executable, mapped for the process's life.
        let base = unsafe { GetModuleHandleW(None) }.ok()?.0 as usize;
        // SAFETY: `base` is the game's executable, as mapped.
        let image = unsafe { Image::of(base) }?;
        let (core, navigator) = image.navigator()?;
        let set_targets = image.unique(SET_TARGETS)?;
        let find_node = image.find_node()?;
        log!(
            "waypoint: game code found: core {:#x}, navigator +{navigator:#x}, set targets {:#x}, find node {:#x}",
            core - base,
            set_targets - base,
            find_node - base
        );
        // SAFETY: each address is the start of the function its pattern names, found exactly once in the
        // game's code; the patterns pin their calling conventions (`SetTargets`, `FindNode`).
        let (set_targets, find_node) = unsafe {
            (std::mem::transmute::<usize, SetTargets>(set_targets), std::mem::transmute::<usize, FindNode>(find_node))
        };
        Some(Game { core: core as *const *mut u8, navigator, set_targets, find_node })
    })();
    if found.is_none() {
        log!("waypoint: the game's waypoint code isn't the version WestPlay knows; waypoints can't be added");
    }
    found
}

/// The start of the navigator's "set targets": it saves its registers, keeps the mode
/// (`mov [rcx+1C8h], edx`) and starts on the first list (`add rcx, 1D0h`).
const SET_TARGETS: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x74, 0x24, 0x10, 0x57, 0x48, 0x83, 0xec, 0x20, 0x48, 0x8b, 0xf9, 0x89,
    0x91, 0xc8, 0x01, 0x00, 0x00, 0x48, 0x81, 0xc1, 0xd0, 0x01, 0x00, 0x00,
];

/// The start of a lookup by uid in a table at `+0x38` of its object (`lea rbx, [rcx+38h]`).
const FIND_IN_TABLE_38: &[u8] = &[
    0x48, 0x89, 0x54, 0x24, 0x10, 0x53, 0x48, 0x83, 0xec, 0x40, 0x48, 0x8d, 0x59, 0x38, 0x48, 0x8b, 0xcb, 0x48, 0x8d,
    0x54, 0x24, 0x58, 0xe8,
];

/// The game's executable, as mapped in its process.
pub(crate) struct Image {
    base: usize,
    /// The whole image's size as mapped (its `SizeOfImage`): nothing is read outside it.
    size: usize,
    text: (usize, usize),
    rdata: (usize, usize),
    /// Its unwind table (`.pdata`): where each of its functions starts and ends.
    pdata: Option<(usize, usize)>,
}

impl Image {
    /// The image mapped at `base`: its code, its constants and its unwind table.
    ///
    /// # Safety
    /// `base` is the start of a module mapped in this process (a PE image), for as long as the `Image` lives.
    pub(crate) unsafe fn of(base: usize) -> Option<Self> {
        // SAFETY: the PE headers lie at the start of the mapped image (the caller's contract), read as they lie.
        let u16_at = |at: usize| unsafe { (base as *const u8).add(at).cast::<u16>().read_unaligned() } as usize;
        // SAFETY: as above.
        let u32_at = |at: usize| unsafe { (base as *const u8).add(at).cast::<u32>().read_unaligned() } as usize;
        let header = u32_at(0x3c);
        let (count, optional) = (u16_at(header + 6), u16_at(header + 20));
        let section = |name: &[u8]| {
            (0..count).find_map(|i| {
                let at = header + 24 + optional + i * 40;
                // SAFETY: a section header's name, within the headers the image maps.
                let bytes = unsafe { std::slice::from_raw_parts((base + at) as *const u8, 8) };
                bytes.starts_with(name).then(|| (base + u32_at(at + 12), u32_at(at + 8)))
            })
        };
        Some(Image {
            base,
            // The optional header's SizeOfImage.
            size: u32_at(header + 24 + 56),
            text: section(b".text\0")?,
            rdata: section(b".rdata\0")?,
            pdata: section(b".pdata\0"),
        })
    }

    /// `len` bytes of the image at the address `at`, if they lie within it.
    pub(crate) fn code_at(&self, at: usize, len: usize) -> Option<&[u8]> {
        self.bytes(at.checked_sub(self.base)?, len)
    }

    /// `len` bytes of the image at `rva`, if they lie within it.
    fn bytes(&self, rva: usize, len: usize) -> Option<&[u8]> {
        // SAFETY: within the mapped image (checked against its size), mapped for the process's life.
        (rva.checked_add(len)? <= self.size)
            .then(|| unsafe { std::slice::from_raw_parts((self.base + rva) as *const u8, len) })
    }

    /// Where the function holding the code at `at` starts, from the unwind table: a function in several parts
    /// (its later parts' unwind info chained to the one before) starts where its first part does.
    pub(crate) fn function_start(&self, at: usize) -> Option<usize> {
        let (table, len) = self.pdata?;
        let rva = at.checked_sub(self.base)?;
        let entry = |i: usize| {
            // SAFETY: an entry of the unwind table, within the section (`i` below its count).
            let read = |field: usize| unsafe { ((table + i * 12 + field * 4) as *const u32).read_unaligned() } as usize;
            (read(0), read(1), read(2))
        };
        // The entries are sorted by where their code starts: the last one starting at or before `rva`.
        let (mut low, mut high) = (0, len / 12);
        while low < high {
            let middle = (low + high) / 2;
            if entry(middle).0 <= rva { low = middle + 1 } else { high = middle }
        }
        let (mut start, end, mut unwind) = entry(low.checked_sub(1)?);
        if rva >= end {
            return None;
        }
        for _ in 0..16 {
            // UNWIND_INFO: its flags in the first byte's top five bits, its count of unwind codes in the third;
            // chained (UNW_FLAG_CHAININFO), the part before's entry follows the codes (kept to an even count).
            let info = self.bytes(unwind, 4)?;
            if (info[0] >> 3) & 0x4 == 0 {
                return Some(self.base + start);
            }
            let parent = self.bytes(unwind + 4 + (usize::from(info[2]).next_multiple_of(2)) * 2, 12)?;
            let field = |i: usize| u32::from_le_bytes(parent[i * 4..i * 4 + 4].try_into().unwrap()) as usize;
            (start, unwind) = (field(0), field(2));
        }
        None
    }

    fn code(&self) -> &[u8] {
        // SAFETY: the image's code section, mapped for the process's life (`Image::of`).
        unsafe { std::slice::from_raw_parts(self.text.0 as *const u8, self.text.1) }
    }

    fn constants(&self) -> &[u8] {
        // SAFETY: the image's constants section, mapped for the process's life (`Image::of`).
        unsafe { std::slice::from_raw_parts(self.rdata.0 as *const u8, self.rdata.1) }
    }

    /// Where `pattern` starts in the code, if exactly once (`None` in it: any byte).
    pub(crate) fn unique_masked(&self, pattern: &[Option<u8>]) -> Option<usize> {
        let fits = |window: &[u8]| window.iter().zip(pattern).all(|(byte, wanted)| wanted.is_none_or(|w| *byte == w));
        let mut found = self.code().windows(pattern.len()).enumerate().filter(|(_, w)| fits(w));
        let (at, _) = found.next()?;
        found.next().is_none().then_some(self.text.0 + at)
    }

    /// Where `pattern` starts in the code, if exactly once.
    fn unique(&self, pattern: &[u8]) -> Option<usize> {
        let mut found = self.code().windows(pattern.len()).enumerate().filter(|(_, w)| *w == pattern);
        let (at, _) = found.next()?;
        found.next().is_none().then_some(self.text.0 + at)
    }

    /// Where a RIP-relative lea at `at` (an address in the code) points.
    fn lea_target(&self, at: usize) -> Option<usize> {
        let offset = at.checked_sub(self.text.0)?;
        let b = self.code().get(offset..offset + 7)?;
        let rip_relative = (b[0] == 0x48 || b[0] == 0x4c) && b[1] == 0x8d && b[2] & 0xc7 == 0x05;
        rip_relative.then(|| (at + 7).wrapping_add_signed(i32::from_le_bytes(b[3..7].try_into().unwrap()) as isize))
    }

    pub(crate) fn constant(&self, text: &[u8]) -> Option<usize> {
        self.constants().windows(text.len()).position(|w| w == text).map(|at| self.rdata.0 + at)
    }

    /// The code loading the address `target` (a RIP-relative lea), if exactly one place does.
    pub(crate) fn unique_load(&self, target: usize) -> Option<usize> {
        let mut found = (0..self.code().len().saturating_sub(7))
            .map(|i| self.text.0 + i)
            .filter(|&at| self.lea_target(at) == Some(target));
        let at = found.next()?;
        found.next().is_none().then_some(at)
    }

    /// The game's node lookup: of the lookups shaped like it, the one that names the node table
    /// (`array_t<node_item_t*>`) in its bounds check.
    fn find_node(&self) -> Option<usize> {
        let name = self.constant(b"??A?$array_t@PEAVnode_item_t@prism@@@prism@@QEAAAEAPEAVnode_item_t@1@_K@Z\0")?;
        let code = self.code();
        let mut found =
            code.windows(FIND_IN_TABLE_38.len()).enumerate().filter(|(_, w)| *w == FIND_IN_TABLE_38).filter(
                |&(at, _)| {
                    (at..(at + 0x80).min(code.len() - 7)).any(|i| self.lea_target(self.text.0 + i) == Some(name))
                },
            );
        let (at, _) = found.next()?;
        found.next().is_none().then_some(self.text.0 + at)
    }

    /// The global holding the core object and the navigator's offset in it, from the telemetry getter for
    /// `truck.navigation.distance` (registered right beside its name): `sub rsp, 28h; mov rax, [global]`,
    /// then `lea rcx, [rax + navigator]` (as crates/host/src/route.rs finds them).
    fn navigator(&self) -> Option<(usize, usize)> {
        let name = self.constant(b"truck.navigation.distance\0")?;
        let code = self.code();
        for site in (0..code.len() - 7).map(|i| self.text.0 + i).filter(|&at| self.lea_target(at) == Some(name)) {
            for back in 1..=64 {
                let Some(getter) = self.lea_target(site - back) else { continue };
                let Some(b) = getter.checked_sub(self.text.0).and_then(|start| code.get(start..start + 48)) else {
                    continue;
                };
                if b[..4] != [0x48, 0x83, 0xec, 0x28] || b[4..7] != [0x48, 0x8b, 0x05] {
                    continue;
                }
                let global =
                    (getter + 11).wrapping_add_signed(i32::from_le_bytes(b[7..11].try_into().unwrap()) as isize);
                let at = (11..41).find(|&i| b[i..i + 3] == [0x48, 0x8d, 0x88])?;
                return Some((global, u32::from_le_bytes(b[at + 3..at + 7].try_into().unwrap()) as usize));
            }
        }
        None
    }
}
