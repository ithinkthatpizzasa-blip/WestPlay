//! Typing on macOS: what a key types in the Mac's keyboard layout (the input source chosen now), with its dead
//! keys: an accent key types nothing, and its accent goes on the next letter (Option-E then E types é).

use std::cell::Cell;
use std::ffi::c_void;

use objc2::msg_send;
use objc2::runtime::AnyObject;

use super::{key_meaning, queue};

// NSEventModifierFlags.
const CAPS_LOCK: usize = 1 << 16;
const SHIFT: usize = 1 << 17;
const CONTROL: usize = 1 << 18;
const OPTION: usize = 1 << 19;
const COMMAND: usize = 1 << 20;

type CFTypeRef = *const c_void;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    static kTISPropertyUnicodeKeyLayoutData: CFTypeRef;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
    fn TISGetInputSourceProperty(source: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(
        layout: *const c_void,
        key_code: u16,
        action: u16,
        modifier_state: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_length: usize,
        actual_length: *mut usize,
        characters: *mut u16,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFRelease(object: CFTypeRef);
}

thread_local! {
    /// The layout's dead-key state: an accent waiting for the next key.
    static DEAD_KEY: Cell<u32> = const { Cell::new(0) };
}

/// A key pressed while typing (`code`, the Mac's; `flags`, the event's modifiers; `key`, its virtual-key code):
/// queued for the app as what it types.
///
/// # Safety
/// `event` is the live key event, on the main thread (the input source is asked there).
pub unsafe fn press(event: &AnyObject, code: u16, flags: usize, key: u32) {
    // Command (or Control) is the Mac's shortcut key; Option takes a word back, else types characters.
    let shortcut = flags & (COMMAND | CONTROL) != 0;
    if let Some(meaning) = key_meaning(key, shortcut, flags & OPTION != 0) {
        DEAD_KEY.with(|dead| dead.set(0));
        queue(meaning.map(|key| key as u32));
        return;
    }
    // SAFETY: on the main thread with the live event (the caller's contract).
    let typed = unsafe { translated(code, flags) }.or_else(|| unsafe { characters(event) });
    queue(typed.unwrap_or_default().chars().filter(|c| !c.is_control()).map(u32::from));
}

/// What the key types in the current layout, its dead keys kept; `None` without a layout to ask.
///
/// # Safety
/// On the main thread.
unsafe fn translated(code: u16, flags: usize) -> Option<String> {
    // Carbon's modifier bits (shiftKey, alphaLock, optionKey, controlKey), shifted down by 8 as UCKeyTranslate takes them.
    let mut modifiers = 0u32;
    for (flag, carbon) in [(SHIFT, 0x02), (CAPS_LOCK, 0x04), (OPTION, 0x08), (CONTROL, 0x10)] {
        if flags & flag != 0 {
            modifiers |= carbon;
        }
    }
    // SAFETY: the input source copied and let go here; its layout data alive while it is; UCKeyTranslate given
    // a buffer of its length.
    unsafe {
        let source = TISCopyCurrentKeyboardLayoutInputSource();
        if source.is_null() {
            return None;
        }
        let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        let result = if data.is_null() {
            None
        } else {
            let mut buffer = [0u16; 8];
            let mut length = 0usize;
            let mut dead = DEAD_KEY.with(Cell::get);
            let status = UCKeyTranslate(
                CFDataGetBytePtr(data).cast(),
                code,
                0, // kUCKeyActionDown
                modifiers,
                u32::from(LMGetKbdType()),
                0,
                &mut dead,
                buffer.len(),
                &mut length,
                buffer.as_mut_ptr(),
            );
            DEAD_KEY.with(|cell| cell.set(dead));
            (status == 0).then(|| String::from_utf16_lossy(&buffer[..length.min(buffer.len())]))
        };
        CFRelease(source);
        result
    }
}

/// The characters the event says the key types (no dead keys): when there's no layout to ask.
///
/// # Safety
/// A live key event.
unsafe fn characters(event: &AnyObject) -> Option<String> {
    // SAFETY: a key event (the caller's contract); its string's UTF-8 alive while the event is.
    unsafe {
        let string: *mut AnyObject = msg_send![event, characters];
        let string = string.as_ref()?;
        let utf8: *const std::ffi::c_char = msg_send![string, UTF8String];
        (!utf8.is_null()).then(|| std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }
}
