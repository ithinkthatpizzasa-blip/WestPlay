//! macOS: the game reaches the functions WestPlay hooks through tables of pointers (OpenGL's dispatch table in each
//! GL context, Objective-C's methods), so a hook points an entry at the plugin's function, and puts the original back
//! when removed. No code is changed: the game runs with the hardened runtime (docs/MACOS_PORT.md).
//!
//! An entry is pointed at its detour only by [`enable_all`] (or at once, made after it: a GL context the game makes
//! later), and the detour calls on through the original it was given.

use std::ffi::CStr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use objc2::runtime::{AnyClass, Imp, Method, Sel};

use crate::log;

/// Where a hook points the game at its detour.
enum Place {
    /// A function pointer in the game's memory (a GL context's dispatch table).
    Slot(*mut usize),
    /// An Objective-C method's implementation.
    Method(&'static Method),
}

struct Hook {
    place: Place,
    detour: usize,
    original: usize,
    on: bool,
}

struct Hooks(Vec<Hook>);
// SAFETY: the slots are the game's memory, alive while their hooks are (the callers' contract), only touched through
// the mutex the list is kept in.
unsafe impl Send for Hooks {}

static HOOKS: Mutex<Hooks> = Mutex::new(Hooks(Vec::new()));
/// Whether the hooks are switched on: one made then is switched on at once.
static ENABLED: AtomicBool = AtomicBool::new(false);

fn hooks() -> std::sync::MutexGuard<'static, Hooks> {
    HOOKS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Hooks the function pointer at `slot`: `original` receives the function it points to. One `original` serves the
/// same entry in several tables (each GL context's), so a table whose entry is another function is refused.
///
/// # Safety
/// `slot` is an aligned function pointer in the game's memory, alive until [`remove_all`], and `detour` a function of
/// the same signature that calls on through `original`.
pub unsafe fn hook_slot(slot: *mut usize, detour: usize, original: &AtomicUsize, name: &str) -> Result<(), String> {
    // SAFETY: an aligned pointer, alive (the caller's contract); read atomically, as the game may call through it.
    let function = unsafe { AtomicUsize::from_ptr(slot) }.load(Ordering::Acquire);
    if function == 0 {
        return Err(format!("{name}: no function"));
    }
    if function == detour {
        // Already this hook's (a table seen twice).
        return Ok(());
    }
    let had = original.load(Ordering::Acquire);
    if had != 0 && had != function {
        return Err(format!("{name}: {function:#x} here, {had:#x} elsewhere"));
    }
    original.store(function, Ordering::Release);
    let mut hooks = hooks();
    hooks.0.push(Hook { place: Place::Slot(slot), detour, original: function, on: false });
    if ENABLED.load(Ordering::Acquire)
        && let Some(hook) = hooks.0.last_mut()
    {
        // SAFETY: made just now, as this function requires.
        unsafe { switch_on(hook) };
    }
    Ok(())
}

/// Hooks the instance method `selector` of the class `class` (and so its subclasses', unless they have their own):
/// `original` receives its implementation.
///
/// # Safety
/// `detour` is a function of the method's exact signature (`self`, `_cmd`, its arguments) that calls on through
/// `original`.
pub unsafe fn hook_method(
    class: &CStr,
    selector: Sel,
    detour: Imp,
    original: &AtomicUsize,
    name: &str,
) -> Result<(), String> {
    let class = AnyClass::get(class).ok_or_else(|| format!("{name}: no class {class:?}"))?;
    let method = class.instance_method(selector).ok_or_else(|| format!("{name}: no method"))?;
    let function = method.implementation() as usize;
    original.store(function, Ordering::Release);
    let mut hooks = hooks();
    hooks.0.push(Hook { place: Place::Method(method), detour: detour as usize, original: function, on: false });
    if ENABLED.load(Ordering::Acquire)
        && let Some(hook) = hooks.0.last_mut()
    {
        // SAFETY: made just now, as this function requires.
        unsafe { switch_on(hook) };
    }
    log!("hooked {name} at {function:#x}");
    Ok(())
}

/// # Safety
/// A hook made as [`hook_slot`] or [`hook_method`] require.
unsafe fn switch_on(hook: &mut Hook) {
    if hook.on {
        return;
    }
    // SAFETY: the slot is alive and aligned, the detour of the same signature (the hook's maker's contract); an
    // aligned pointer-sized store, so a call through it meanwhile takes either function, whole.
    unsafe {
        match hook.place {
            Place::Slot(slot) => AtomicUsize::from_ptr(slot).store(hook.detour, Ordering::Release),
            Place::Method(method) => {
                method.set_implementation(std::mem::transmute::<usize, Imp>(hook.detour));
            }
        }
    }
    hook.on = true;
}

/// Switches every hook on.
///
/// # Safety
/// Every hook was made as [`hook_slot`] or [`hook_method`] require, and its detour is ready to run (on any thread).
pub unsafe fn enable_all() -> Result<(), String> {
    for hook in &mut hooks().0 {
        // SAFETY: made as required (the caller's contract).
        unsafe { switch_on(hook) };
    }
    ENABLED.store(true, Ordering::Release);
    Ok(())
}

/// Switches every hook off and forgets it, each entry as it was. An entry no longer pointing at its detour (the game
/// changed it since) is left as the game made it.
///
/// # Safety
/// The slots are still alive, and no detour may be running on another thread meanwhile, or run after (the caller
/// waits for them to return before the plugin can be unloaded: `scs_telemetry_shutdown`).
pub unsafe fn remove_all() {
    ENABLED.store(false, Ordering::Release);
    for hook in hooks().0.drain(..).filter(|hook| hook.on) {
        // SAFETY: the slot alive and aligned (the caller's contract), the method the runtime's; the original is what
        // was there.
        unsafe {
            match hook.place {
                Place::Slot(slot) => {
                    let _ = AtomicUsize::from_ptr(slot).compare_exchange(
                        hook.detour,
                        hook.original,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
                Place::Method(method) => {
                    method.set_implementation(std::mem::transmute::<usize, Imp>(hook.original));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn twice(n: u32) -> u32 {
        n * 2
    }

    unsafe extern "C" fn thrice(n: u32) -> u32 {
        n * 3
    }

    fn address(function: unsafe extern "C" fn(u32) -> u32) -> usize {
        function as usize
    }

    #[test]
    fn a_slot_points_at_the_detour_while_on_and_back_after() {
        let mut table = [address(twice)];
        let original = AtomicUsize::new(0);
        // SAFETY: a table of this test's, alive through it; both functions of one signature.
        unsafe {
            hook_slot(&mut table[0], address(thrice), &original, "a test").unwrap();
            assert_eq!(table[0], address(twice), "made switched off");
            enable_all().unwrap();
            assert_eq!(table[0], address(thrice));
            assert_eq!(original.load(Ordering::Acquire), address(twice));
            // Another table whose entry is another function: refused.
            let mut other = [address(thrice) + 1];
            assert!(hook_slot(&mut other[0], address(thrice), &original, "a test").is_err());
            remove_all();
        }
        assert_eq!(table[0], address(twice));
    }
}
