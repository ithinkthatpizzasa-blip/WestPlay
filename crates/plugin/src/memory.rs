//! The game's memory, read for the app (macOS): what the app reads of the game on Windows from outside (its route,
//! its weather), it reads on macOS through the plugin, which is inside it (another program may read a process's
//! memory only as a debugger may). The bridge's [`westplay_shared::MemoryBlock`] carries one read at a time.
//!
//! A thread of the plugin's own answers: it copies the range asked for with `mach_vm_read_overwrite` on its own
//! process, which reads a range not mapped as nothing (an error), never as a fault in the game. It's stopped before
//! the plugin can be unloaded ([`stop`]).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use westplay_shared::MEMORY_READ_MAX;

use crate::{bridge, log};

/// How often the thread looks for a request while none comes.
const IDLE: Duration = Duration::from_millis(2);

static STOP: AtomicBool = AtomicBool::new(false);
static THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

unsafe extern "C" {
    static mach_task_self_: u32;
    fn mach_vm_read_overwrite(task: u32, address: u64, size: u64, data: u64, out_size: *mut u64) -> i32;
    fn _dyld_get_image_header(index: u32) -> *const std::ffi::c_void;
}

/// Says where the game's executable is loaded, and starts answering the app's reads.
pub fn start() {
    let Some(bridge) = bridge::get() else { return };
    // SAFETY: a plain query: image 0 is the process's executable.
    let base = unsafe { _dyld_get_image_header(0) } as u64;
    bridge.memory.image_base.store(base, Ordering::Release);
    // Requests made before this plugin was loaded aren't answered: the app asks again.
    bridge.memory.answered.store(bridge.memory.request.load(Ordering::Acquire), Ordering::Release);
    STOP.store(false, Ordering::Release);
    match std::thread::Builder::new().name("westplay-memory".into()).spawn(serve) {
        Ok(thread) => *THREAD.lock().unwrap_or_else(|e| e.into_inner()) = Some(thread),
        Err(e) => log!("memory: no thread to read the game's memory for the app: {e}"),
    }
    log!("memory: the game's executable at {base:#x}, read for the app");
}

/// Stops answering, the thread ended (before the plugin can be unloaded).
pub fn stop() {
    STOP.store(true, Ordering::Release);
    if let Some(thread) = THREAD.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = thread.join();
    }
    if let Some(bridge) = bridge::get() {
        bridge.memory.image_base.store(0, Ordering::Release);
    }
}

fn serve() {
    let Some(bridge) = bridge::get() else { return };
    let block = &bridge.memory;
    let mut reads = 0u64;
    while !STOP.load(Ordering::Acquire) {
        let request = block.request.load(Ordering::Acquire);
        if request == block.answered.load(Ordering::Acquire) {
            std::thread::sleep(IDLE);
            continue;
        }
        let address = block.address.load(Ordering::Acquire);
        let length = (block.length.load(Ordering::Acquire) as usize).min(MEMORY_READ_MAX);
        let mut read = 0u64;
        // SAFETY: the kernel copies from this process's own memory into the block's data (`length` bytes at most, its
        // size), or fails for a range not mapped; nothing else writes the data until this request is answered.
        let status = unsafe {
            mach_vm_read_overwrite(mach_task_self_, address, length as u64, block.data.as_ptr() as u64, &mut read)
        };
        let ok = status == 0 && read == length as u64;
        block.ok.store(u32::from(ok), Ordering::Release);
        block.answered.store(request, Ordering::Release);
        reads += 1;
        if reads == 1 {
            log!("memory: the app's first read answered ({})", if ok { "read" } else { "not mapped" });
        }
    }
}
