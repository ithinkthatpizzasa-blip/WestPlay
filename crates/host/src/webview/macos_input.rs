//! Mouse input for a WKWebView of the stage (`macos.rs`), as AppKit would give it from a real mouse: `NSEvent`s
//! made for the stage's window and sent to the WebView, which is never the window's key view (docs/MACOS_PORT.md,
//! Phase 2: they reach the page all the same). The wheel's events carry their point on the screen, where WebKit
//! looks for what to scroll: the stage's window is on a display for that.

use std::cell::Cell;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSScreen};
use objc2_foundation::{MainThreadMarker, NSPoint, NSProcessInfo};
use objc2_web_kit::WKWebView;

use super::{MouseButtons, MouseKind};

/// Clicks this close together, in time and place, count as one double (or triple) click, as macOS counts them.
const MULTI_CLICK: Duration = Duration::from_millis(500);
const MULTI_CLICK_SLOP: i32 = 4;
/// Windows' wheel units per notch (`MouseKind::Wheel`'s), and the lines a notch scrolls on a Mac.
const WHEEL_DELTA: i32 = 120;

thread_local! {
    /// The last press, for counting clicks: when, where, and its count.
    static LAST_PRESS: Cell<Option<(Instant, i32, i32, isize)>> = const { Cell::new(None) };
}

/// Sends `kind` at `x, y` (page pixels from the WebView's top left), `buttons` held, to `view`.
pub(super) fn send(view: &WKWebView, kind: MouseKind, buttons: MouseButtons, data: u32, x: i32, y: i32) {
    let Some(window) = view.window() else { return };
    let mtm = MainThreadMarker::from(view);
    // The WebView's top left is the window's (`macos::place_view`), and window points count from the bottom.
    let height = window.contentLayoutRect().size.height;
    let point = NSPoint::new(x as f64, height - y as f64);
    let number = window.windowNumber();
    let now = NSProcessInfo::processInfo().systemUptime();
    let event = |kind: NSEventType, clicks: isize, pressure: f32| -> Option<Retained<NSEvent>> {
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            point,
            NSEventModifierFlags::empty(),
            now,
            number,
            None,
            0,
            clicks,
            pressure,
        )
    };
    match kind {
        MouseKind::Move => {
            let (kind, pressure) = if buttons.left {
                (NSEventType::LeftMouseDragged, 1.0)
            } else if buttons.right {
                (NSEventType::RightMouseDragged, 1.0)
            } else {
                (NSEventType::MouseMoved, 0.0)
            };
            if let Some(event) = event(kind, 0, pressure) {
                match kind {
                    NSEventType::LeftMouseDragged => view.mouseDragged(&event),
                    NSEventType::RightMouseDragged => view.rightMouseDragged(&event),
                    _ => view.mouseMoved(&event),
                }
            }
        }
        MouseKind::Leave => {
            // Somewhere off the page: what was under the mouse is left, as when it leaves a window.
            let away = NSPoint::new(-10.0, height + 10.0);
            if let Some(event) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                NSEventType::MouseMoved,
                away,
                NSEventModifierFlags::empty(),
                now,
                number,
                None,
                0,
                0,
                0.0,
            ) {
                view.mouseMoved(&event);
            }
        }
        MouseKind::LeftDown | MouseKind::RightDown => {
            let clicks = click_count(x, y);
            let (kind, right) = if kind == MouseKind::LeftDown {
                (NSEventType::LeftMouseDown, false)
            } else {
                (NSEventType::RightMouseDown, true)
            };
            if let Some(event) = event(kind, clicks, 1.0) {
                if right { view.rightMouseDown(&event) } else { view.mouseDown(&event) }
            }
        }
        MouseKind::LeftUp | MouseKind::RightUp => {
            let clicks = LAST_PRESS.get().map_or(1, |(_, _, _, count)| count);
            let (kind, right) = if kind == MouseKind::LeftUp {
                (NSEventType::LeftMouseUp, false)
            } else {
                (NSEventType::RightMouseUp, true)
            };
            if let Some(event) = event(kind, clicks, 0.0) {
                if right { view.rightMouseUp(&event) } else { view.mouseUp(&event) }
            }
        }
        MouseKind::Wheel => scroll(view, &window, mtm, point, data as i32),
    }
}

/// The click count of a press at `x, y` now: one more than the last press's if it was close by, just before.
fn click_count(x: i32, y: i32) -> isize {
    let now = Instant::now();
    let count = match LAST_PRESS.get() {
        Some((at, last_x, last_y, count))
            if now.duration_since(at) <= MULTI_CLICK
                && (x - last_x).abs() <= MULTI_CLICK_SLOP
                && (y - last_y).abs() <= MULTI_CLICK_SLOP =>
        {
            count + 1
        }
        _ => 1,
    };
    LAST_PRESS.set(Some((now, x, y, count)));
    count
}

/// The wheel turned `delta` (Windows' units, positive away from the player) at `point` in the window: a scroll
/// event of as many lines, at that point on the screen.
fn scroll(view: &WKWebView, window: &objc2_app_kit::NSWindow, mtm: MainThreadMarker, point: NSPoint, delta: i32) {
    use objc2_core_graphics::{CGEvent, CGScrollEventUnit};
    // A part of a notch still scrolls, a line.
    let lines = if delta.abs() < WHEEL_DELTA { delta.signum() } else { delta / WHEEL_DELTA };
    if lines == 0 {
        return;
    }
    let on_screen = window.convertPointToScreen(point);
    // CG's points count from the main display's top left, AppKit's from its bottom left.
    let main_height = NSScreen::screens(mtm).firstObject().map_or(0.0, |screen| screen.frame().size.height);
    let Some(cg) = CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Line, 1, lines, 0, 0) else {
        return;
    };
    CGEvent::set_location(Some(&cg), objc2_core_foundation::CGPoint::new(on_screen.x, main_height - on_screen.y));
    if let Some(event) = NSEvent::eventWithCGEvent(&cg) {
        view.scrollWheel(&event);
    }
}
