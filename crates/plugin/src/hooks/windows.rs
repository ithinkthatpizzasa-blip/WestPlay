//! Windows: MinHook patches function bodies inside system DLLs, so a hook applies to every object of that type in
//! the game, the game's own included.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use minhook::MinHook;

use crate::log;

static HOOK_TARGETS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Hooks the function at `target`; `original` receives the trampoline that calls the original function.
///
/// # Safety
/// `target` is the start of a function in this process, and `detour` a function of the same signature that
/// calls on through `original`; created disabled, on until [`enable_all`].
pub unsafe fn hook(target: usize, detour: *mut c_void, original: &AtomicUsize, name: &str) -> Result<(), String> {
    // SAFETY: a function and its detour of the same signature (the caller's contract).
    let trampoline = unsafe { MinHook::create_hook(target as *mut c_void, detour) }
        .map_err(|e| format!("create_hook {name}: {e:?}"))?;
    original.store(trampoline as usize, Ordering::Release);
    HOOK_TARGETS.lock().unwrap_or_else(|e| e.into_inner()).push(target);
    log!("hooked {name} at {target:#x}");
    Ok(())
}

/// Switches every hook on.
///
/// # Safety
/// Every hook created was made as [`hook`] requires, and its detour is ready to run (on any thread).
pub unsafe fn enable_all() -> Result<(), String> {
    // SAFETY: hooks made as `hook` requires (the caller's contract).
    unsafe { MinHook::enable_all_hooks() }.map_err(|e| format!("enable_all_hooks: {e:?}"))
}

/// Switches every hook off and removes it, the functions as they were.
///
/// # Safety
/// No detour may be running on another thread meanwhile, or run after (the caller waits for them to return
/// before the plugin can be unloaded: `scs_telemetry_shutdown`).
pub unsafe fn remove_all() {
    for target in HOOK_TARGETS.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        // SAFETY: a hook this plugin made, removed once (taken out of the list).
        unsafe {
            let _ = MinHook::disable_hook(target as *mut c_void);
            let _ = MinHook::remove_hook(target as *mut c_void);
        }
    }
}

/// The function a vtable slot points to.
///
/// # Safety
/// `object` is a live COM object whose vtable has more than `slot` entries.
pub unsafe fn vtable_entry(object: *mut c_void, slot: usize) -> usize {
    // SAFETY: a COM object starts with its vtable's address, and the slot is within it (the caller's contract).
    unsafe { *(*(object as *const *const usize)).add(slot) }
}
