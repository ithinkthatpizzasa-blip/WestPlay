//! Screen interaction on macOS: the game's own event loop.
//!
//! The Mac game reads the keyboard and the mouse from Cocoa events alone: each frame it takes them one by one with
//! `-[NSApplication nextEventMatchingMask:untilDate:inMode:dequeue:]`, reads them (the mouse's `deltaX`/`deltaY`,
//! a key's `keyCode` and `characters`, the wheel's `deltaY`), then hands each to `sendEvent:`. (It listens to HID
//! devices too, but only takes joysticks and gamepads, and can't open them without the Input Monitoring
//! permission: docs/MACOS_PORT.md.) So that method's implementation is the hook (data, as the plugin's others),
//! and an event it holds back is never seen by the game, nor by AppKit: no permission is needed, no event tap.
//!
//! While the player uses the screen, mouse movement, the wheel and new presses go to the screen (`input`); keys
//! are the `keyboard` module's. Which keys are down is read from Cocoa too: the modifiers and the mouse buttons
//! as they are now (`+[NSEvent modifierFlags]`, `+[NSEvent pressedMouseButtons]`), any other key from the events
//! the game takes.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{msg_send, sel};

use super::{game_gets_button, interaction_active, press_button, take_mouse};
use crate::hooks::{guarded, hook_method};
use crate::{keyboard, log};

type NextEventFn =
    unsafe extern "C-unwind" fn(*mut AnyObject, Sel, u64, *mut AnyObject, *mut AnyObject, Bool) -> *mut AnyObject;

static ORIG_NEXT_EVENT: AtomicUsize = AtomicUsize::new(0);

// NSEventType.
const LEFT_MOUSE_DOWN: usize = 1;
const LEFT_MOUSE_UP: usize = 2;
const RIGHT_MOUSE_DOWN: usize = 3;
const RIGHT_MOUSE_UP: usize = 4;
const MOUSE_MOVED: usize = 5;
const LEFT_MOUSE_DRAGGED: usize = 6;
const RIGHT_MOUSE_DRAGGED: usize = 7;
const KEY_DOWN: usize = 10;
const KEY_UP: usize = 11;
const FLAGS_CHANGED: usize = 12;
const SCROLL_WHEEL: usize = 22;
const OTHER_MOUSE_DOWN: usize = 25;
const OTHER_MOUSE_UP: usize = 26;
const OTHER_MOUSE_DRAGGED: usize = 27;

/// `kCGEventSourceStateCombinedSessionState`.
const COMBINED_SESSION_STATE: i32 = 0;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
}

/// DirectInput's wheel counts for one line scrolled (a notch), as the pointer block takes them.
const WHEEL_PER_LINE: f64 = 120.0;

/// The mouse buttons the game has seen go down and not yet up (bits by button number).
/// Whether the first event the game took through the hook was logged.
static FIRST_EVENT_LOGGED: AtomicBool = AtomicBool::new(false);
static GAME_HELD: AtomicU8 = AtomicU8::new(0);
/// Keys down that aren't modifiers, by virtual-key code, from the events the game takes (`keyboard`).
static KEYS_DOWN: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];

thread_local! {
    /// What's left of the screen's movement and wheel below a whole count, kept for the next event.
    static REMAINDER: Cell<(f64, f64, f64)> = const { Cell::new((0.0, 0.0, 0.0)) };
}

/// # Safety
/// In the game's process, before the hooks are switched on ([`crate::hooks::enable_all`]).
pub unsafe fn install() -> Result<(), String> {
    // SAFETY: the detour has the method's exact signature (`self`, `_cmd`, the mask, the date, the mode, whether to
    // dequeue), and calls on through the original.
    unsafe {
        let next_event: NextEventFn = next_event_hook;
        hook_method(
            c"NSApplication",
            sel!(nextEventMatchingMask:untilDate:inMode:dequeue:),
            std::mem::transmute::<NextEventFn, Imp>(next_event),
            &ORIG_NEXT_EVENT,
            "-[NSApplication nextEventMatchingMask:untilDate:inMode:dequeue:]",
        )
    }
}

/// # Safety
/// Called in place of `-[NSApplication nextEventMatchingMask:untilDate:inMode:dequeue:]`, with its arguments, on
/// the main thread (AppKit's events are only taken there).
unsafe extern "C-unwind" fn next_event_hook(
    app: *mut AnyObject,
    cmd: Sel,
    mask: u64,
    until: *mut AnyObject,
    mode: *mut AnyObject,
    dequeue: Bool,
) -> *mut AnyObject {
    // SAFETY: the original (stored before the hook was switched on), with the game's arguments as they came; each
    // event it gives is a live NSEvent, autoreleased by it.
    unsafe {
        let original: NextEventFn = std::mem::transmute(ORIG_NEXT_EVENT.load(Ordering::Acquire));
        loop {
            let event = original(app, cmd, mask, until, mode, dequeue);
            // An event only looked at stays in the queue: decided when it's taken.
            if event.is_null() || !dequeue.as_bool() {
                return event;
            }
            let mut game_gets = true;
            guarded(|| game_gets = for_the_game(&*event));
            if game_gets {
                return event;
            }
            // Held back: the next one, as the game asked (by its date, so a wait for one still ends when it would).
        }
    }
}

/// Whether an event taken from the queue goes on to the game, or is the screen's (or held back from the game).
///
/// # Safety
/// A live NSEvent, on the main thread.
unsafe fn for_the_game(event: &AnyObject) -> bool {
    // SAFETY: an NSEvent (the caller's contract), asked what the game asks of it, by its type.
    unsafe {
        let kind: usize = msg_send![event, type];
        if !FIRST_EVENT_LOGGED.swap(true, Ordering::Relaxed) {
            log!("input: the game's event loop runs through the hook (first event: type {kind})");
        }
        match kind {
            LEFT_MOUSE_DOWN | LEFT_MOUSE_UP | RIGHT_MOUSE_DOWN | RIGHT_MOUSE_UP | OTHER_MOUSE_DOWN | OTHER_MOUSE_UP => {
                let number: isize = msg_send![event, buttonNumber];
                let down = matches!(kind, LEFT_MOUSE_DOWN | RIGHT_MOUSE_DOWN | OTHER_MOUSE_DOWN);
                let Ok(index @ 0..8) = u32::try_from(number) else { return true };
                let interacting = interaction_active();
                if interacting && index < 3 {
                    press_button(index, down);
                }
                let mut held = GAME_HELD.load(Ordering::Acquire);
                let gets = game_gets_button(index, down, interacting, &mut held);
                GAME_HELD.store(held, Ordering::Release);
                gets
            }
            MOUSE_MOVED | LEFT_MOUSE_DRAGGED | RIGHT_MOUSE_DRAGGED | OTHER_MOUSE_DRAGGED => {
                if !interaction_active() {
                    return true;
                }
                let dx: f64 = msg_send![event, deltaX];
                let dy: f64 = msg_send![event, deltaY];
                to_screen(dx, dy, 0.0);
                false
            }
            SCROLL_WHEEL => {
                if !interaction_active() {
                    return true;
                }
                // Lines (a notch is one, or more as it's turned faster), up positive: as DirectInput's wheel.
                let lines: f64 = msg_send![event, deltaY];
                to_screen(0.0, 0.0, lines * WHEEL_PER_LINE);
                false
            }
            KEY_DOWN | KEY_UP | FLAGS_CHANGED => keyboard::for_the_game(event, kind == FLAGS_CHANGED, kind == KEY_DOWN),
            _ => true,
        }
    }
}

/// Movement (points, x right, y down) and wheel for the screen, in whole counts, what's left over kept.
fn to_screen(dx: f64, dy: f64, wheel: f64) {
    REMAINDER.with(|remainder| {
        let (rx, ry, rw) = remainder.get();
        let (x, y, w) = (rx + dx, ry + dy, rw + wheel);
        let whole = (x.trunc(), y.trunc(), w.trunc());
        remainder.set((x - whole.0, y - whole.1, w - whole.2));
        take_mouse(whole.0 as i32, whole.1 as i32, whole.2 as i32);
    });
}

/// The player stopped using the screen.
pub(super) fn interaction_ended() {
    REMAINDER.with(|remainder| remainder.set((0.0, 0.0, 0.0)));
}

/// A key that isn't a modifier went down or up (a virtual-key code), as the game takes its events.
pub fn set_key_down(key: u32, down: bool) {
    if let Some(state) = KEYS_DOWN.get(key as usize) {
        state.store(down, Ordering::Release);
    }
}

/// Whether a key or mouse button (a virtual-key code) is down. On the main thread.
pub fn key_down(key: u32) -> bool {
    // Modifiers by their Mac key codes, asked as the game asks for them (`CGEventSourceKeyState`, the session's state).
    let modifier: &[u16] = match key {
        0x10 => &[56, 60],
        0x11 => &[59, 62],
        0x12 => &[58, 61],
        0x14 => &[57],
        0x5b => &[55],
        0x5c => &[54],
        0xa0 => &[56],
        0xa1 => &[60],
        0xa2 => &[59],
        0xa3 => &[62],
        0xa4 => &[58],
        0xa5 => &[61],
        _ => &[],
    };
    if !modifier.is_empty() {
        // SAFETY: a plain query, no pointers.
        return modifier.iter().any(|&code| unsafe { CGEventSourceKeyState(COMBINED_SESSION_STATE, code) });
    }
    // Mouse buttons: left, right, middle, back, forward, as `pressedMouseButtons`' bits.
    let button = match key {
        0x01 => Some(0),
        0x02 => Some(1),
        0x04 => Some(2),
        0x05 => Some(3),
        0x06 => Some(4),
        _ => None,
    };
    let Some(class) = AnyClass::get(c"NSEvent") else { return false };
    if let Some(bit) = button {
        // SAFETY: a class property, the mouse buttons held now.
        let pressed: usize = unsafe { msg_send![class, pressedMouseButtons] };
        return pressed & (1 << bit) != 0;
    }
    // Other keys: as the game's events said, while it's the app in front (one let go elsewhere sends it nothing).
    if !game_shown() {
        KEYS_DOWN.iter().for_each(|state| state.store(false, Ordering::Release));
        return false;
    }
    KEYS_DOWN.get(key as usize).is_some_and(|state| state.load(Ordering::Acquire))
}

/// Whether the game is shown: the app in front, not hidden.
pub fn game_shown() -> bool {
    let Some(class) = AnyClass::get(c"NSApplication") else { return false };
    // SAFETY: the shared application (the game's, made by it), asked whether it's in front and shown: on the main
    // thread.
    unsafe {
        let app: *mut AnyObject = msg_send![class, sharedApplication];
        let Some(app) = app.as_ref() else { return false };
        let active: Bool = msg_send![app, isActive];
        let hidden: Bool = msg_send![app, isHidden];
        active.as_bool() && !hidden.as_bool()
    }
}
