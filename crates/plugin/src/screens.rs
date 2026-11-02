//! In-cabin screen takeover.
//!
//! The WestPlay mod (`westplay_setup::game_mod`, built from the player's own game) patches the game's GPS UI scripts: each draws one full-screen
//! image, the *screen texture* of its layout (B8G8R8A8 sRGB, one mip), as the top layer of the script's
//! "screen on" group. The game renders it like any other UI image, through its own render-target pooling,
//! post-processing, flipping and mip generation, and switches the screen on and off with the truck's
//! electrics.
//!
//! Every layout (`crates/shared/screen_layouts.txt`) has its own texture, [`SCREEN_WIDTH`] wide and
//! [`SCREEN_HEIGHT`] plus the layout's index high, so its size tells which kind of screen the game is
//! drawing; we publish that for the app, which picks its screen profile by it. App frames are
//! [`SCREEN_WIDTH`] x [`SCREEN_HEIGHT`]; a texture's extra rows repeat the frame's bottom row.
//!
//! We never touch the game's render targets; we only keep the screen texture filled: when the game binds it
//! to draw a screen, we copy the newest app frame into it first.
//!
//! WestPlay's tablet (`westplay_shared::TABLET`) is a model whose screen shows its layout's texture itself, drawn
//! in the game's 3D pass rather than into a screen's render target. When WestPlay is switched off, the app is
//! not running, or the truck isn't one WestPlay supports on this screen (most trucks share the stock one),
//! it is made fully transparent, so the game's original screen shows through.
//!
//! Which screen shows WestPlay, in which truck, is decided here ([`drawing`]); finding the textures as the game
//! binds them, and filling them, is each graphics API's (`windows`: Direct3D 11; `macos`: OpenGL).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(test)]
use westplay_shared::SCREEN_HEIGHT;
use westplay_shared::{SCREEN_WIDTH, TRUCK_SUPPORTED, TRUCK_UNSUPPORTED, screen_layouts, screen_supported};

use crate::{bridge, log};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(windows)]
pub use windows::*;

/// Current game frame, advanced by `on_frame`.
static FRAME: AtomicU64 = AtomicU64::new(0);
/// Whether a screen the game draws is WestPlay's in the truck the player is in (`screen_supported`): the stock
/// GPS screen is shared by trucks WestPlay doesn't support. False until a screen is drawn.
static ON_SUPPORTED_TRUCK: AtomicBool = AtomicBool::new(false);
/// The frame a screen of WestPlay's in this truck was last drawn in (0: none since the truck changed). The
/// tablet is drawn beside the truck's own screen, which isn't WestPlay's there: the truck is WestPlay's while
/// either was drawn lately, not by whichever was drawn last.
static SUPPORTED_DRAWN: AtomicU64 = AtomicU64::new(0);
/// Whether the log has said this truck's screen isn't WestPlay's (once a truck: `truck_changed`).
static UNSUPPORTED_SAID: AtomicBool = AtomicBool::new(false);
/// Whether the log has said the cab's mark is drawn in this truck (once a truck: `truck_changed`): so whether the
/// camera system goes by it here, not by the tablet (which, a hookup on a dashboard set's toy spot, is drawn from
/// outside the cab too).
static CAB_MARK_SAID: AtomicBool = AtomicBool::new(false);
/// Whether the tablet stays blank in this truck (`westplay_shared::TABLET_APART_FILE`: owner 2026-10-26, a hookup any
/// vehicle's set takes, kept from showing in those it mustn't), as the truck changes.
static TABLET_BLANK: AtomicBool = AtomicBool::new(false);
/// Whether the truck's own screen (not the tablet) has shown WestPlay since the truck changed: the tablet then stays
/// blank in it (owner 2026-10-27). Both showing, the page was laid out for one and then the other every frame (a
/// player's Scania S, its own screen and the tablet: 85 times a second, the page slowing and freezing).
static OWN_SCREEN_SEEN: AtomicBool = AtomicBool::new(false);
/// The frame any screen of WestPlay's layouts (WestPlay's or not) was last drawn in, in this truck (0: none since the
/// truck changed).
static ANY_SCREEN_DRAWN: AtomicU64 = AtomicU64::new(0);
/// The frame the cab began being drawn with the electrics on and no such screen (0: it isn't).
static CAB_WITHOUT_SCREEN: AtomicU64 = AtomicU64::new(0);
/// How long the cab is drawn with the electrics on and no screen of WestPlay's layouts before its truck counts as
/// not WestPlay's (frames: a screen starts with the electrics in a frame or two).
const NO_SCREEN_FRAMES: u64 = 120;
/// How lately (frames) a screen of WestPlay's was drawn for the truck to still be WestPlay's when another is.
const SUPPORTED_LATELY: u64 = 30;

/// Whether the truck's screen is one WestPlay shows on (see [`ON_SUPPORTED_TRUCK`]).
pub fn on_supported_truck() -> bool {
    ON_SUPPORTED_TRUCK.load(Ordering::Acquire)
}

/// The player is in another truck (or the game told its truck again): whether WestPlay is on its screen is
/// known again only once the game draws a screen of WestPlay's in it. Until then, holding the interaction
/// key takes nothing from the game (a player's log, 2026-10-21: in a Mercedes Actros after a DAF 2021, the
/// key still took the mouse).
pub fn truck_changed(id: &str) {
    // The vehicles the tablet stays blank in, as setup last wrote them beside the mod (none: none). A truck with
    // no id (the game says one now and then while the player stays in theirs, as under TruckersMP, 2026-10-26)
    // leaves it as it was.
    if !id.is_empty() {
        let path = crate::log::log_dir().parent().map(|dir| dir.join(westplay_shared::TABLET_APART_FILE));
        let marks =
            path.and_then(|path| std::fs::read_to_string(path).ok()).map(|text| westplay_shared::read_marks(&text));
        let blank = marks.is_some_and(|marks| westplay_shared::vehicle_marked(id, &marks));
        if blank && !TABLET_BLANK.load(Ordering::Acquire) {
            log!("tablet: kept blank in this truck");
        }
        TABLET_BLANK.store(blank, Ordering::Release);
        // Another truck: its own screen not seen showing WestPlay yet.
        OWN_SCREEN_SEEN.store(false, Ordering::Release);
    }
    SUPPORTED_DRAWN.store(0, Ordering::Release);
    UNSUPPORTED_SAID.store(false, Ordering::Release);
    CAB_MARK_SAID.store(false, Ordering::Release);
    ANY_SCREEN_DRAWN.store(0, Ordering::Release);
    CAB_WITHOUT_SCREEN.store(0, Ordering::Release);
    another_truck();
    if let Some(bridge) = bridge::get() {
        bridge.plugin.truck_supported.store(0, Ordering::Release);
    }
    if ON_SUPPORTED_TRUCK.swap(false, Ordering::AcqRel) {
        log!("screen: another truck, WestPlay's screen not seen in it yet");
    }
}

/// The current game frame.
fn frame() -> u64 {
    FRAME.load(Ordering::Relaxed)
}

/// A screen texture of `layout` is about to be drawn this frame: whether it shows the app's frame (otherwise it's
/// made transparent, the game's own screen showing through). Which truck the player is in, and which screen
/// WestPlay is on, are said for the app on the way.
fn drawing(layout: usize) -> bool {
    let frame = frame();
    // A truck's id can't be read while the telemetry thread writes it: the last answer stands meanwhile.
    let truck = bridge::get().and_then(|b| b.plugin.truck.read());
    let tablet = layout == westplay_shared::tablet_layout();
    let supported = truck.as_ref().map_or_else(on_supported_truck, |truck| screen_supported(layout, truck.id.as_str()));
    if supported && !tablet && !OWN_SCREEN_SEEN.swap(true, Ordering::AcqRel) {
        log!("screen: {} shows WestPlay in this truck: the tablet kept blank in it", screen_layouts()[layout].id);
    }
    let here =
        shows_here(tablet, supported, TABLET_BLANK.load(Ordering::Acquire), OWN_SCREEN_SEEN.load(Ordering::Acquire));
    let lately = drawn_lately(SUPPORTED_DRAWN.load(Ordering::Acquire), frame);
    if here {
        SUPPORTED_DRAWN.store(frame.max(1), Ordering::Release);
    }
    // WestPlay's truck while one of its screens was drawn lately (the tablet, beside the truck's own).
    let supported = here || lately;
    let was = ON_SUPPORTED_TRUCK.swap(supported, Ordering::AcqRel);
    ANY_SCREEN_DRAWN.store(frame.max(1), Ordering::Release);
    // Said for the app (the companion says a truck isn't WestPlay's): WestPlay's once a screen of WestPlay's has
    // been drawn in it at all (the tablet, left out as the player looks away from it, beside a truck's own screen
    // that isn't WestPlay's), until the truck changes.
    if let Some(bridge) = bridge::get() {
        let ever = SUPPORTED_DRAWN.load(Ordering::Acquire) != 0;
        let said = if here || ever { TRUCK_SUPPORTED } else { TRUCK_UNSUPPORTED };
        bridge.plugin.truck_supported.store(said, Ordering::Release);
    }
    // Said once a truck (or as it stops being WestPlay's): a truck first seen unsupported was said nothing
    // (a player's Actros MP4, 2026-10-24: the log never said why WestPlay didn't show).
    if !supported && (was || !UNSUPPORTED_SAID.swap(true, Ordering::AcqRel)) {
        let truck = truck.map(|truck| truck.id.as_str().to_owned()).unwrap_or_default();
        log!("screen: {} isn't WestPlay's in {truck}, the game's own shows", screen_layouts()[layout].id);
    }

    // The screen WestPlay is on, said for the app (not the truck's own beside the tablet).
    if let Some(bridge) = bridge::get()
        && (here || !lately)
    {
        let plugin = &bridge.plugin;
        let published = layout as u32 + 1;
        if plugin.screen_layout.swap(published, Ordering::AcqRel) != published {
            log!("screen layout: {}", screen_layouts()[layout].id);
        }
        // The cab drawn, for the camera system: not by the tablet in a truck whose cab has its mark (the tablet
        // is drawn from outside the cab too, as a hookup on a set's toy spot).
        if !(tablet && CAB_MARK_SAID.load(Ordering::Acquire)) {
            plugin.screen_frame.store(plugin.heartbeat.load(Ordering::Relaxed), Ordering::Release);
        }
    }

    // This screen WestPlay's itself: the truck's own, beside the tablet, keeps the game's.
    here && bridge::get().is_some_and(|b| b.host.screen_enabled.load(Ordering::Acquire) == 1)
}

/// Whether a screen shows WestPlay itself: a layout WestPlay's in the truck (`supported`); the tablet not where it's
/// kept blank (`blank`: the vehicles set apart), nor once the truck's own screen has shown WestPlay (`own_seen`).
fn shows_here(tablet: bool, supported: bool, blank: bool, own_seen: bool) -> bool {
    supported && !(tablet && (blank || own_seen))
}

/// Whether a screen of WestPlay's, last drawn in frame `drawn` (0: none since the truck changed), was drawn
/// lately at `frame`.
fn drawn_lately(drawn: u64, frame: u64) -> bool {
    drawn != 0 && frame.saturating_sub(drawn) <= SUPPORTED_LATELY
}

/// The cab's mark drawn into a dashboard's screen: the camera in the cab, this frame, said for the app.
fn cab_drawn() {
    let Some(bridge) = bridge::get() else { return };
    let plugin = &bridge.plugin;
    plugin.cab_frame.store(plugin.heartbeat.load(Ordering::Relaxed), Ordering::Release);
    if !CAB_MARK_SAID.swap(true, Ordering::AcqRel) {
        log!("cab: the dashboard's mark drawn: the camera in the cab is told by it");
    }
    // A truck with no screen of WestPlay's layouts at all (a mod truck's own GPS screen): its cab drawn a while with
    // the electrics on (with them off, no screen is drawn in any truck) and no such screen, not WestPlay's.
    let frame = frame();
    let electrics = plugin.telemetry.read().is_some_and(|telemetry| telemetry.electric_enabled);
    // Never once a screen of WestPlay's has been drawn in this truck (its tablet, looked away from).
    if !electrics
        || SUPPORTED_DRAWN.load(Ordering::Acquire) != 0
        || drawn_lately(ANY_SCREEN_DRAWN.load(Ordering::Acquire), frame)
    {
        CAB_WITHOUT_SCREEN.store(0, Ordering::Release);
        return;
    }
    let since = CAB_WITHOUT_SCREEN.load(Ordering::Acquire);
    if since == 0 {
        CAB_WITHOUT_SCREEN.store(frame.max(1), Ordering::Release);
    } else if frame.saturating_sub(since) > NO_SCREEN_FRAMES
        && plugin.truck_supported.swap(TRUCK_UNSUPPORTED, Ordering::AcqRel) != TRUCK_UNSUPPORTED
    {
        log!("screen: no screen of WestPlay's layouts in this truck: not WestPlay's");
    }
}

/// Called once per presented game frame.
pub fn on_frame(frame: u64) {
    FRAME.store(frame, Ordering::Relaxed);
    if !frame.is_multiple_of(300) {
        return;
    }
    // None drawn for a while: not on WestPlay's screen any more, whatever truck this is.
    if release_unused(frame) {
        ON_SUPPORTED_TRUCK.store(false, Ordering::Release);
    }
}

/// The rows of a screen texture `height` high to fill from a frame's rows: the frame's, then its bottom row
/// repeated (`(texture's row, frame's row)`).
#[cfg_attr(windows, allow(dead_code))]
fn rows(height: u32) -> impl Iterator<Item = (u32, u32)> {
    let frame_rows = westplay_shared::SCREEN_HEIGHT;
    (0..height).map(move |row| (row, row.min(frame_rows - 1)))
}

/// The bytes of a transparent screen as tall as the tallest screen texture (the last layout's), so it covers any.
fn transparent_pixels() -> Vec<u8> {
    let height = westplay_shared::ScreenLayout::texture_height(screen_layouts().len() - 1);
    vec![0u8; (SCREEN_WIDTH * height * 4) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Held by the tests that change which truck the player is in, which would otherwise race each other.
    static TRUCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn another_truck_is_not_on_westplays_screen_until_one_is_drawn_in_it() {
        let _truck = TRUCK.lock().unwrap_or_else(|e| e.into_inner());
        ON_SUPPORTED_TRUCK.store(true, Ordering::Release);
        truck_changed("scania.s_2016");
        assert!(!on_supported_truck());
        // Told again with nothing to clear: still not.
        truck_changed("scania.s_2016");
        assert!(!on_supported_truck());
    }

    #[test]
    fn the_tablet_keeps_the_truck_westplays_beside_its_own_screen() {
        // The tablet drawn in frame 100; the truck's own screen (not WestPlay's) in the same frame, the next, or
        // half a second on at 60 fps: the truck is still WestPlay's.
        for frame in [100, 101, 130] {
            assert!(drawn_lately(100, frame), "{frame}");
        }
        // No tablet for longer (the camera outside, the tablet taken off), or none since the truck changed.
        assert!(!drawn_lately(100, 131));
        assert!(!drawn_lately(0, 10));
        // Drawn after it in the same frame order: as lately as can be.
        assert!(drawn_lately(101, 100));
    }

    #[test]
    fn the_tablet_is_blank_once_the_trucks_own_screen_shows_westplay() {
        // A truck's own screen of WestPlay's shows it; one that isn't WestPlay's in the truck doesn't.
        assert!(shows_here(false, true, false, true));
        assert!(!shows_here(false, false, false, false));
        // The tablet shows it in a truck whose own screen hasn't (none of WestPlay's, or not seen yet)...
        assert!(shows_here(true, true, false, false));
        // ...but not once it has, nor in the vehicles it's kept blank in.
        assert!(!shows_here(true, true, false, true));
        assert!(!shows_here(true, true, true, false));
    }

    #[test]
    fn a_real_truck_change_forgets_the_own_screen_an_unnamed_one_doesnt() {
        let _truck = TRUCK.lock().unwrap_or_else(|e| e.into_inner());
        OWN_SCREEN_SEEN.store(true, Ordering::Release);
        // TruckersMP's now-and-then truck with no id, the player still in theirs.
        truck_changed("");
        assert!(OWN_SCREEN_SEEN.load(Ordering::Acquire));
        truck_changed("scania.s_2016");
        assert!(!OWN_SCREEN_SEEN.load(Ordering::Acquire));
    }

    #[test]
    fn a_screen_texture_repeats_the_frames_bottom_row_below_it() {
        let rows: Vec<_> = rows(SCREEN_HEIGHT + 2).collect();
        assert_eq!(rows.len() as u32, SCREEN_HEIGHT + 2);
        assert_eq!(rows[0], (0, 0));
        assert_eq!(rows[SCREEN_HEIGHT as usize - 1], (SCREEN_HEIGHT - 1, SCREEN_HEIGHT - 1));
        assert_eq!(rows[SCREEN_HEIGHT as usize + 1], (SCREEN_HEIGHT + 1, SCREEN_HEIGHT - 1));
    }
}
