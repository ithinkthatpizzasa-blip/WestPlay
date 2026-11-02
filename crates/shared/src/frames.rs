//! Frames through shared memory, for where there's no shared GPU texture (macOS: the plugin draws with the
//! game's OpenGL, the app with its own): the app writes each frame's pixels, BGRA, into a block mapped by both,
//! and the plugin uploads the newest into a texture of its own.
//!
//! The block holds three frames (triple buffering), so neither side ever waits, as with the shared texture:
//! the app writes into one the plugin can't be reading, and swaps it in as the newest; the plugin swaps the
//! newest for the one it's done with. A frame the plugin didn't take in time is written over.
//!
//! Which frame is whose is three slots shared out: one the writer holds, one the reader holds, the newest
//! between them (`latest`, with [`NEW`] while the reader hasn't taken it). Each side says which it holds
//! (`writer`, `reader`), so one that starts again (the game started again, the app) takes the slot that is
//! neither of the others'. While both start at once they may pick the same: a torn frame then, never more
//! (the pixels are plain bytes).

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::{BRIDGE_VERSION, SCREEN_HEIGHT, SCREEN_WIDTH, platform};

/// The most a frame holds: the app's frames are the screen's size, 4 bytes a pixel.
pub const FRAME_BYTES: usize = SCREEN_WIDTH as usize * SCREEN_HEIGHT as usize * 4;

/// [`FrameBlock::latest`]: the newest slot hasn't been taken by the reader yet.
pub const NEW: u32 = 4;
const SLOTS: u32 = 3;

/// The frame block's name: its version in it, as the bridge's.
pub fn frames_name() -> String {
    platform::shared_name(&format!("WestPlay.Frames.v{BRIDGE_VERSION}"))
}

#[repr(C)]
pub struct FrameBlock {
    /// The newest frame's slot, with [`NEW`] until the reader takes it.
    latest: AtomicU32,
    /// The slot each side holds, plus one (0: not known, that side hasn't started).
    writer: AtomicU32,
    reader: AtomicU32,
    slots: [FrameSlot; SLOTS as usize],
}

#[repr(C)]
struct FrameSlot {
    /// The frame's number (the app's count), and its size; its pixels rows of `width` × 4 bytes, BGRA.
    seq: AtomicU64,
    width: AtomicU32,
    height: AtomicU32,
    pixels: UnsafeCell<[u8; FRAME_BYTES]>,
}

// SAFETY: a slot's pixels are written only by the side that holds it (the protocol above); its other fields
// are atomics.
unsafe impl Sync for FrameBlock {}

impl FrameBlock {
    fn latest_slot(&self) -> u32 {
        (self.latest.load(Ordering::Acquire) & !NEW).min(SLOTS - 1)
    }

    /// A slot that is neither the newest nor `other`'s (the other side's, plus one; 0 for none).
    fn free_slot(&self, other: &AtomicU32) -> u32 {
        let latest = self.latest_slot();
        let other = other.load(Ordering::Acquire).checked_sub(1);
        (0..SLOTS).find(|&slot| slot != latest && Some(slot) != other).unwrap_or((latest + 1) % SLOTS)
    }
}

/// A frame the reader took: its number, size and pixels.
pub struct Frame<'a> {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    /// `width` × `height` × 4 bytes, BGRA, rows top first.
    pub pixels: &'a [u8],
}

/// The app's side: writes frames.
pub struct FrameWriter<'a> {
    block: &'a FrameBlock,
    back: u32,
}

impl<'a> FrameWriter<'a> {
    /// The one writer of `block`.
    pub fn new(block: &'a FrameBlock) -> Self {
        let back = block.free_slot(&block.reader);
        block.writer.store(back + 1, Ordering::Release);
        Self { block, back }
    }

    /// Writes a frame of `width` × `height` (`fill` writes its pixels, BGRA rows of `width` × 4 bytes) and makes
    /// it the newest. `false`, and nothing written, if it's larger than [`FRAME_BYTES`].
    pub fn write(&mut self, seq: u64, width: u32, height: u32, fill: impl FnOnce(&mut [u8])) -> bool {
        let len = width as usize * height as usize * 4;
        if len > FRAME_BYTES {
            return false;
        }
        let slot = &self.block.slots[self.back as usize];
        // SAFETY: the back slot is the writer's alone (the protocol above), the writer is `&mut self`.
        let pixels = unsafe { &mut (&mut *slot.pixels.get())[..len] };
        fill(pixels);
        slot.seq.store(seq, Ordering::Relaxed);
        slot.width.store(width, Ordering::Relaxed);
        slot.height.store(height, Ordering::Relaxed);
        // The frame, written, goes to the reader (release); the one it replaces comes back to be written.
        let old = self.block.latest.swap(self.back | NEW, Ordering::AcqRel);
        self.back = (old & !NEW).min(SLOTS - 1);
        self.block.writer.store(self.back + 1, Ordering::Release);
        true
    }
}

/// The plugin's side: takes the newest frame.
pub struct FrameReader<'a> {
    block: &'a FrameBlock,
    front: u32,
}

impl<'a> FrameReader<'a> {
    /// The one reader of `block`.
    pub fn new(block: &'a FrameBlock) -> Self {
        let front = block.free_slot(&block.writer);
        block.reader.store(front + 1, Ordering::Release);
        Self { block, front }
    }

    /// The newest frame, if one came since the last; it stays the reader's until the next call. Once one has come,
    /// the frame taken before is the app's again, so `Some` always (a frame of no size, or of more than a frame
    /// holds: its pixels empty).
    pub fn take(&mut self) -> Option<Frame<'_>> {
        if self.block.latest.load(Ordering::Acquire) & NEW == 0 {
            return None;
        }
        let old = self.block.latest.swap(self.front, Ordering::AcqRel);
        self.front = (old & !NEW).min(SLOTS - 1);
        self.block.reader.store(self.front + 1, Ordering::Release);
        let slot = &self.block.slots[self.front as usize];
        let width = slot.width.load(Ordering::Relaxed);
        let height = slot.height.load(Ordering::Relaxed);
        let len = width as usize * height as usize * 4;
        let len = if len > FRAME_BYTES { 0 } else { len };
        // SAFETY: the front slot is the reader's (the protocol above) until the next `take`, which the borrow
        // of `self` keeps from happening while the frame is used.
        let pixels = unsafe { &(&*slot.pixels.get())[..len] };
        Some(Frame { seq: slot.seq.load(Ordering::Relaxed), width, height, pixels })
    }
}

/// A mapping of the frame block, shared as the bridge is: whichever process maps it first creates it.
pub struct FramesMapping {
    region: platform::Region,
}

impl FramesMapping {
    pub fn open() -> Result<Self, platform::Error> {
        Ok(Self { region: platform::Region::open(&frames_name(), size_of::<FrameBlock>())? })
    }

    pub fn block(&self) -> &FrameBlock {
        // SAFETY: the region is a `FrameBlock`'s size, zeroed when new (a valid block: atomics and bytes),
        // mapped as long as `self`.
        unsafe { &*(self.region.as_ptr() as *const FrameBlock) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zeroed block, as a new mapping is.
    fn new_block() -> Box<FrameBlock> {
        let layout = std::alloc::Layout::new::<FrameBlock>();
        // SAFETY: a block is atomics and bytes: all zeros is a valid one; the box owns the allocation, of its
        // layout.
        unsafe {
            let ptr = std::alloc::alloc_zeroed(layout) as *mut FrameBlock;
            assert!(!ptr.is_null());
            Box::from_raw(ptr)
        }
    }

    #[test]
    fn the_reader_gets_the_newest_frame_once() {
        let block = new_block();
        let mut writer = FrameWriter::new(&block);
        let mut reader = FrameReader::new(&block);
        assert!(reader.take().is_none(), "nothing written yet");
        assert!(writer.write(1, 2, 1, |pixels| pixels.fill(1)));
        assert!(writer.write(2, 2, 1, |pixels| pixels.fill(2)));
        let frame = reader.take().unwrap();
        assert_eq!((frame.seq, frame.width, frame.height), (2, 2, 1));
        assert_eq!(frame.pixels, [2; 8]);
        assert!(reader.take().is_none(), "taken already");
        assert!(writer.write(3, 1, 1, |pixels| pixels.fill(3)));
        assert_eq!(reader.take().unwrap().pixels, [3; 4]);
        // A frame of no size still comes: the one taken before is the app's again.
        assert!(writer.write(4, 0, 0, |_| {}));
        let frame = reader.take().unwrap();
        assert_eq!((frame.seq, frame.pixels.len()), (4, 0));
    }

    #[test]
    fn a_frame_too_large_isnt_written() {
        let block = new_block();
        let mut writer = FrameWriter::new(&block);
        assert!(!writer.write(1, SCREEN_WIDTH, SCREEN_HEIGHT + 1, |_| panic!("not written")));
        assert!(FrameReader::new(&block).take().is_none());
    }

    #[test]
    fn the_three_slots_are_always_shared_out() {
        let block = new_block();
        let mut writer = FrameWriter::new(&block);
        let mut reader = FrameReader::new(&block);
        for seq in 0..20 {
            writer.write(seq, 1, 1, |_| {});
            if seq % 3 == 0 {
                reader.take();
            }
            let latest = block.latest_slot();
            assert!(writer.back != reader.front && writer.back != latest && reader.front != latest, "{seq}");
        }
    }

    #[test]
    fn a_side_starting_again_takes_the_free_slot() {
        let block = new_block();
        let mut writer = FrameWriter::new(&block);
        let reader = FrameReader::new(&block);
        writer.write(1, 1, 1, |_| {});
        // The app started again.
        let writer = FrameWriter::new(&block);
        assert!(writer.back != reader.front && writer.back != block.latest_slot());
        // The game started again.
        let reader = FrameReader::new(&block);
        assert!(writer.back != reader.front && reader.front != block.latest_slot());
    }

    #[test]
    fn frames_written_while_read_are_never_torn() {
        let block: &'static FrameBlock = Box::leak(new_block());
        let mut reader = FrameReader::new(block);
        let writer = std::thread::spawn(move || {
            let mut writer = FrameWriter::new(block);
            for seq in 1..=2000u64 {
                writer.write(seq, 64, 64, |pixels| pixels.fill(seq as u8));
            }
        });
        let mut last = 0;
        let mut taken = 0;
        while !writer.is_finished() {
            if let Some(frame) = reader.take() {
                assert!(frame.seq > last, "frames in order");
                assert!(frame.pixels.iter().all(|&byte| byte == frame.seq as u8), "torn frame {}", frame.seq);
                last = frame.seq;
                taken += 1;
            }
        }
        writer.join().unwrap();
        assert!(taken > 0);
    }

    #[test]
    fn the_frames_name_fits_macos() {
        assert!(frames_name().len() + 6 <= platform::MAX_NAME_LEN);
    }
}
