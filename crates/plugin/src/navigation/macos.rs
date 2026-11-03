//! Changing the game's route on macOS (1.57.2.7, Apple's compiler): its pieces found in its executable as loaded (a
//! Mach-O), and the navigator's route given alone.
//!
//! 1.57's navigator is 1.61's 0x18 bytes sooner, with three lists, not four (read in the game, 2026-11-02): at
//! `+0x1B8` one that a stop added doesn't change, at `+0x1D8` the route's targets (its waypoints in route order, a
//! job's destination last), at `+0x1F8` the places to avoid; the mode at `+0x1B0`. There's no list of the waypoints in
//! the order they were added: they're the route's targets. Its "set targets" (`navigator, mode, three lists`) copies
//! a list in unless it's the navigator's own, so the first and the last are given back as they are, and only the route
//! is new.
//!
//! A map node comes from the node table (the `goto` console command's second lookup, by uid: its first is the map's
//! items'): the table's entries are the map nodes themselves, as the route's targets point to them (their position
//! first, as route.rs reads it, their uid at `+0x30`).

use std::ffi::c_void;

use super::{List, Planned, Target};
use crate::log;

/// Offsets in the navigator.
pub const MODE: usize = 0x1b0;
const FIRST: usize = 0x1b8;
pub const ROUTE: usize = 0x1d8;
const AVOID: usize = 0x1f8;

/// A map node's uid.
const NODE_UID: usize = 0x30;

type SetTargets = unsafe extern "C" fn(*mut u8, u32, *const List, *const List, *const List);
type FindNode = unsafe extern "C" fn(*mut u8, u64) -> *mut u8;

pub struct Game {
    /// The global holding the game's core object (the navigator's owner, and the node table's).
    pub core: *const *mut u8,
    pub navigator: usize,
    set_targets: SetTargets,
    find_node: FindNode,
}

// SAFETY: the pointers are into the game's own executable, fixed for the process's life, and only used on the game's
// main thread (`serve`).
unsafe impl Send for Game {}
// SAFETY: as for `Send`: never used from two threads at once.
unsafe impl Sync for Game {}

impl Game {
    /// The map node with this uid, from the game's node table.
    ///
    /// # Safety
    /// On the game's main thread, `core` the game's core object.
    pub unsafe fn find_node(&self, core: *mut u8, uid: u64) -> Option<usize> {
        // SAFETY: the game's own lookup, found by its shape (`locate`), with its core object; the uid read back from the
        // node, a check that it's one (nothing is done with one that isn't).
        unsafe {
            let node = (self.find_node)(core, uid);
            if node.is_null() {
                return None;
            }
            let found = (node.add(NODE_UID) as *const u64).read_unaligned();
            if found != uid {
                log!("route: node {uid:#x}: the table's entry at {node:p} has {found:#x} where its uid should be");
                return None;
            }
            Some(node as usize)
        }
    }
}

/// The waypoints: the route's targets, a job's destination (last) left off.
///
/// # Safety
/// `navigator` is the game's navigator.
pub unsafe fn waypoints(_navigator: *mut u8, route: &[Target], mode: u32) -> Option<Vec<Target>> {
    let job = matches!(mode, 1..=4 | 6 | 7);
    let stops = if job { route.split_last().map(|(_, stops)| stops)? } else { route };
    Some(stops.to_vec())
}

/// Gives the navigator its new route (its other two lists given back as they are), and it plans again.
///
/// # Safety
/// On the game's main thread, `navigator` the game's, `game` as `locate` found it.
pub unsafe fn give(game: &Game, navigator: *mut u8, planned: &Planned) {
    // SAFETY: the navigator's lists, read as the game's `array_t`; the game's own function, which copies a list given
    // (alive through the call) and leaves its own as they are.
    unsafe {
        let allocator = (navigator.add(ROUTE) as *const List).read().allocator;
        (game.set_targets)(
            navigator,
            planned.mode,
            navigator.add(FIRST) as *const List,
            &List::of(allocator, &planned.route),
            navigator.add(AVOID) as *const List,
        );
    }
}

/// Logs what the navigator holds (its mode and three lists, each target's first fields, node and node uid) when
/// it changes: what 1.57 makes of a route without a job.
///
/// # Safety
/// On the game's main thread, `game` as `locate` found it.
pub unsafe fn watch(game: &Game) {
    static SEEN: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    // SAFETY: the navigator as `change` reads it; a list's entries read only when its count is plausible and its
    // data set, a node's uid only from a target that says it's a map node (type 2) with its node set.
    let now = unsafe {
        let core = *game.core;
        if core.is_null() {
            return;
        }
        let navigator = core.add(game.navigator);
        let list = |at: usize| {
            let list = &*(navigator.add(at) as *const List);
            if list.count > super::MAX_TARGETS || (list.count > 0 && list.data.is_null()) {
                return format!("{at:#x}: count {}?", list.count);
            }
            let entries: Vec<String> = std::slice::from_raw_parts(list.data, list.count as usize)
                .iter()
                .map(|target| {
                    let field = |i: usize| u32::from_le_bytes(target[i..i + 4].try_into().unwrap());
                    let node = super::node_of(target);
                    let uid = if field(16) == 2 && node != 0 {
                        format!("{:#x}", ((node + NODE_UID) as *const u64).read_unaligned())
                    } else {
                        "-".into()
                    };
                    format!("{{{:#x} {:#x} {node:#x} {} uid {uid}}}", field(0), field(4), field(16))
                })
                .collect();
            format!("{at:#x}: [{}]", entries.join(", "))
        };
        let mode = (navigator.add(MODE) as *const u32).read();
        format!("mode {mode}, {}, {}, {}", list(FIRST), list(ROUTE), list(AVOID))
    };
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if *seen != now {
        log!("route: navigator now {now}");
        *seen = now;
    }
}

/// The start of the navigator's "set targets": its frame, the third list kept (`mov [rbp-30h], r8`), the mode
/// (`mov [rdi+1B0h], esi`), the first list's place (`add rdi, 1B8h`) and whether it's the navigator's own
/// (`cmp rdi, rdx`).
const SET_TARGETS: &[u8] = &[
    0x55, 0x48, 0x89, 0xe5, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x53, 0x48, 0x83, 0xec, 0x18, 0x4c, 0x89,
    0x45, 0xd0, 0x49, 0x89, 0xcf, 0x48, 0x89, 0xfb, 0x89, 0xb7, 0xb0, 0x01, 0x00, 0x00, 0x48, 0x81, 0xc7, 0xb8, 0x01,
    0x00, 0x00, 0x48, 0x39, 0xd7,
];

/// The node table's lookup by uid: a binary search of the map nodes at `+0x38` of its object (their count at
/// `+0x40`), by the uid at `+0x30` of each (`None`: any byte, a jump's length).
const FIND_NODE: &[Option<u8>] = &[
    Some(0x55),
    Some(0x48),
    Some(0x89),
    Some(0xe5),
    Some(0x48),
    Some(0x83),
    Some(0xec),
    Some(0x10),
    Some(0x48),
    Some(0x8b),
    Some(0x4f),
    Some(0x40),
    Some(0x31),
    Some(0xd2),
    Some(0x49),
    Some(0x89),
    Some(0xc8),
    Some(0xeb),
    None,
    Some(0x66),
    Some(0x2e),
    Some(0x0f),
    Some(0x1f),
    Some(0x84),
    Some(0x00),
    Some(0x00),
    Some(0x00),
    Some(0x00),
    Some(0x00),
    Some(0x0f),
    Some(0x1f),
    Some(0x00),
    Some(0x4d),
    Some(0x89),
    Some(0xd0),
    Some(0x74),
    None,
    Some(0x4c),
    Some(0x39),
    Some(0xc2),
    Some(0x73),
    None,
    Some(0x4e),
    Some(0x8d),
    Some(0x0c),
    Some(0x02),
    Some(0x49),
    Some(0xd1),
    Some(0xe9),
    Some(0x48),
    Some(0x8b),
    Some(0x47),
    Some(0x38),
    Some(0x4a),
    Some(0x8b),
    Some(0x04),
    Some(0xc8),
    Some(0x4d),
    Some(0x8d),
    Some(0x59),
    Some(0x01),
    Some(0x4d),
    Some(0x89),
    Some(0xca),
    Some(0x48),
    Some(0x39),
    Some(0x70),
    Some(0x30),
];

unsafe extern "C" {
    fn _dyld_get_image_header(index: u32) -> *const c_void;
}

/// Finds the pieces in the running game's code; `None` (logged) if any isn't there exactly once.
///
/// # Safety
/// In the game's process (the plugin is loaded in it).
pub unsafe fn locate() -> Option<Game> {
    let found = (|| {
        // SAFETY: image 0 is the process's executable, loaded for its life.
        let image = unsafe { Image::of(_dyld_get_image_header(0) as usize) }?;
        let (core, navigator) = image.navigator()?;
        let set_targets = image.unique(&SET_TARGETS.iter().map(|&b| Some(b)).collect::<Vec<_>>())?;
        let find_node = image.unique(FIND_NODE)?;
        log!(
            "waypoint: game code found: core +{:#x}, navigator +{navigator:#x}, set targets +{:#x}, find node +{:#x}",
            core - image.base,
            set_targets - image.base,
            find_node - image.base
        );
        // SAFETY: each address is the start of the function its pattern names, found exactly once in the game's code;
        // the patterns pin their arguments (`SetTargets`, `FindNode`: the System V convention).
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

/// The game's executable as loaded: its code (`__TEXT,__text`) and C strings (`__TEXT,__cstring`).
pub(crate) struct Image {
    /// Where its Mach-O header is loaded.
    pub(crate) base: usize,
    text: (usize, usize),
    cstrings: (usize, usize),
}

impl Image {
    /// The Mach-O loaded at `base`: where its sections are (their addresses moved by where it's loaded).
    ///
    /// # Safety
    /// `base` is the header of a Mach-O image loaded in this process, for as long as the `Image` lives.
    pub(crate) unsafe fn of(base: usize) -> Option<Self> {
        const LC_SEGMENT_64: u32 = 0x19;
        // SAFETY: the header and its load commands lie at the start of the loaded image (the caller's contract).
        let u32_at = |at: usize| unsafe { ((base + at) as *const u32).read_unaligned() } as usize;
        // SAFETY: as above.
        let u64_at = |at: usize| unsafe { ((base + at) as *const u64).read_unaligned() } as usize;
        // A segment's or section's name: its 16-byte field, up to a nul if there's one (a name of 16 has none: 1.57's
        // `__TEXT` has `__objc_classname`).
        let name = |at: usize| {
            // SAFETY: as above: a 16-byte name field.
            let field = unsafe { std::slice::from_raw_parts((base + at) as *const u8, 16) };
            field.split(|&byte| byte == 0).next().unwrap_or(field)
        };
        if u32_at(0) != 0xfeed_facf {
            return None;
        }
        let (mut text, mut cstrings) = (None, None);
        let mut at = 32;
        for _ in 0..u32_at(16) {
            if u32_at(at) == LC_SEGMENT_64 as usize && name(at + 8) == b"__TEXT" {
                // How far it's loaded from where its segment says: each section's address moved as far.
                let slide = base.wrapping_sub(u64_at(at + 24));
                for i in 0..u32_at(at + 64) {
                    let section = at + 72 + i * 80;
                    let place = (u64_at(section + 32).wrapping_add(slide), u64_at(section + 40));
                    match name(section) {
                        b"__text" => text = Some(place),
                        b"__cstring" => cstrings = Some(place),
                        _ => {}
                    }
                }
            }
            at += u32_at(at + 4);
        }
        Some(Image { base, text: text?, cstrings: cstrings? })
    }

    fn code(&self) -> &[u8] {
        // SAFETY: the image's code section, loaded for the process's life (`Image::of`).
        unsafe { std::slice::from_raw_parts(self.text.0 as *const u8, self.text.1) }
    }

    /// Where `pattern` starts in the code, if exactly once (`None` in it: any byte).
    pub(crate) fn unique(&self, pattern: &[Option<u8>]) -> Option<usize> {
        let fits = |window: &[u8]| window.iter().zip(pattern).all(|(byte, wanted)| wanted.is_none_or(|w| *byte == w));
        let mut found = self.code().windows(pattern.len()).enumerate().filter(|(_, w)| fits(w));
        let (at, _) = found.next()?;
        found.next().is_none().then_some(self.text.0 + at)
    }

    /// `len` bytes of the code at the address `at`, if they lie within it.
    pub(crate) fn code_at(&self, at: usize, len: usize) -> Option<&[u8]> {
        let offset = at.checked_sub(self.text.0)?;
        self.code().get(offset..offset.checked_add(len)?)
    }

    /// Where a RIP-relative lea at `at` (an address in the code) points.
    fn lea_target(&self, at: usize) -> Option<usize> {
        let offset = at.checked_sub(self.text.0)?;
        let b = self.code().get(offset..offset + 7)?;
        let rip_relative = (b[0] == 0x48 || b[0] == 0x4c) && b[1] == 0x8d && b[2] & 0xc7 == 0x05;
        rip_relative.then(|| (at + 7).wrapping_add_signed(i32::from_le_bytes(b[3..7].try_into().unwrap()) as isize))
    }

    /// The global holding the core object and the navigator's offset in it, from the telemetry getter for
    /// `truck.navigation.distance`, registered just after its name: `mov rdi, [global]; test rdi, rdi`, then
    /// `add rdi, navigator` (as crates/host/src/route.rs finds them on a Mac).
    fn navigator(&self) -> Option<(usize, usize)> {
        let wanted = b"truck.navigation.distance\0";
        // SAFETY: the image's C strings, loaded for the process's life (`Image::of`).
        let strings = unsafe { std::slice::from_raw_parts(self.cstrings.0 as *const u8, self.cstrings.1) };
        let name = self.cstrings.0 + strings.windows(wanted.len()).position(|w| w == wanted)?;
        let code = self.code();
        for site in (0..code.len() - 7).map(|i| self.text.0 + i).filter(|&at| self.lea_target(at) == Some(name)) {
            for ahead in 7..=48 {
                let Some(getter) = self.lea_target(site + ahead) else { continue };
                let Some(b) = getter.checked_sub(self.text.0).and_then(|start| code.get(start..start + 40)) else {
                    continue;
                };
                if b[..3] != [0x48, 0x8b, 0x3d] || b[7..10] != [0x48, 0x85, 0xff] {
                    continue;
                }
                let global = (getter + 7).wrapping_add_signed(i32::from_le_bytes(b[3..7].try_into().unwrap()) as isize);
                let at = (10..33).find(|&i| b[i..i + 3] == [0x48, 0x81, 0xc7])?;
                return Some((global, u32::from_le_bytes(b[at + 3..at + 7].try_into().unwrap()) as usize));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A string of this test's own, to be found among its executable's C strings.
    const MARKER: &str = "westplay: a C string the image search finds\0";

    #[test]
    fn finds_the_code_and_strings_of_an_executable_as_loaded() {
        // SAFETY: image 0 is this test's executable, loaded for its life.
        let image = unsafe { Image::of(_dyld_get_image_header(0) as usize) }.expect("the image isn't read");
        assert!(image.text.1 > 0 && image.code().len() == image.text.1);
        // SAFETY: the C strings section as `Image::of` found it, loaded for the test's life.
        let strings = unsafe { std::slice::from_raw_parts(image.cstrings.0 as *const u8, image.cstrings.1) };
        assert!(strings.windows(MARKER.len()).any(|w| w == MARKER.as_bytes()), "{:?}", std::hint::black_box(MARKER));
    }

    /// The Mac game's pieces found in its executable named by `WESTPLAY_MAC_GAME` (1.57.2.7s: the core's global at
    /// `+0x26DB3E0`, the navigator at `+0x3C38`, "set targets" at `+0x7146A0`, the node lookup at `+0x69EE30`, read
    /// 2026-11-02). Its `__TEXT` starts its file, so its bytes there lie as loaded.
    #[test]
    #[ignore = "needs the Mac game's executable: WESTPLAY_MAC_GAME"]
    fn finds_the_pieces_in_the_mac_game() {
        let file = std::fs::read(std::env::var("WESTPLAY_MAC_GAME").expect("WESTPLAY_MAC_GAME")).unwrap();
        let be = |at: usize| u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        // Its x86_64 part, of a universal file.
        let image = if be(0) == 0xcafe_babe {
            let arch = (0..be(4)).map(|i| 8 + i * 20).find(|&at| be(at) == 0x0100_0007).unwrap();
            file[be(arch + 8)..be(arch + 8) + be(arch + 12)].to_vec()
        } else {
            file
        };
        // SAFETY: the executable's bytes, laid out as loaded for its `__TEXT`, alive through the test.
        let image = unsafe { Image::of(image.as_ptr() as usize) }.expect("the image isn't read");
        let (core, navigator) = image.navigator().expect("no navigator");
        let set_targets =
            image.unique(&SET_TARGETS.iter().map(|&b| Some(b)).collect::<Vec<_>>()).expect("no set targets");
        let find_node = image.unique(FIND_NODE).expect("no node lookup");
        let found = (core - image.base, navigator, set_targets - image.base, find_node - image.base);
        assert_eq!(found, (0x26db3e0, 0x3c38, 0x7146a0, 0x69ee30));
    }
}
