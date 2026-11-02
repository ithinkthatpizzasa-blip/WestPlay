//! Shared plumbing for the plugin's function hooks (`d3d`, `input` on Windows; `gl` on macOS), and the guards that
//! keep a panic in the plugin's code from ever reaching the game (`guarded` for hooks, `contained` for what the game
//! calls).
//!
//! Hooks are made switched off and switched on together by [`enable_all`]; [`remove_all`] puts everything back.
//! How a hook is made is each OS's (`windows`, `macos`).

use std::cell::Cell;
use std::panic::AssertUnwindSafe;

use crate::log;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(windows)]
pub use windows::*;

thread_local! {
    static IN_WESTPLAY: Cell<bool> = const { Cell::new(false) };
}

/// Runs plugin code from a hook. Re-entrant calls are skipped (our own calls can land in hooked functions),
/// and panics are contained so they can never unwind into or abort the game.
pub fn guarded(f: impl FnOnce()) {
    if IN_WESTPLAY.with(|flag| flag.replace(true)) {
        return;
    }
    if std::panic::catch_unwind(AssertUnwindSafe(f)).is_err() {
        log!("panic inside a hook was contained");
    }
    IN_WESTPLAY.with(|flag| flag.set(false));
}

/// Runs plugin code the game calls directly (the SDK's init and shutdown, its telemetry callbacks): a panic is
/// contained and `failed` returned instead, so it can never unwind into or abort the game.
pub fn contained<T>(what: &str, failed: T, f: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        log!("panic in {what} was contained");
        failed
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_the_game_called_into_comes_back_as_failed() {
        assert_eq!(contained("a test", -1, || 0), 0);
        assert_eq!(contained("a test", -1, || panic!("a bug in the plugin")), -1);
    }

    #[test]
    fn a_panic_in_a_hook_stops_there_and_the_next_call_still_runs() {
        guarded(|| panic!("a bug in a hook"));
        let mut ran = false;
        guarded(|| ran = true);
        assert!(ran);
    }
}
