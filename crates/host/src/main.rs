//! Standalone host for development: `westplay-host [url] [--size WxH]`.
//! Without a URL it shows the built-in test page. On macOS the host runs on a thread of its own, the main thread
//! running AppKit's event loop, which WebKit needs (as in the companion).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use westplay_host::settings::SharedSettings;
use westplay_host::{Config, Content, SCREEN_HEIGHT, SCREEN_WIDTH, TEST_PAGE, run};

fn main() {
    let mut width = SCREEN_WIDTH;
    let mut height = SCREEN_HEIGHT;
    let mut url = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--size" {
            if let Some((w, h)) = args.next().as_deref().and_then(|s| s.split_once('x')) {
                width = w.parse().unwrap_or(width);
                height = h.parse().unwrap_or(height);
            }
        } else {
            url = Some(arg);
        }
    }

    let data_dir = data_dir();
    let config = Config {
        width,
        height,
        content: url.map(Content::Url).unwrap_or_else(|| Content::Html(TEST_PAGE.to_string())),
        data_dir,
        open_request: Arc::default(),
        accounts: Arc::default(),
        settings: Arc::new(SharedSettings::default()),
        // Nothing kept: the standalone host has no settings file.
        on_settings: Box::new(|_| {}),
        customize_changed: Arc::default(),
        // No map for the standalone host, unless `WESTPLAY_GAME` names the game's folder (developing): the app's
        // setup chooses the game.
        game: Arc::new(westplay_host::maps::SetUpGame::new(std::env::var_os("WESTPLAY_GAME").map(PathBuf::from))),
        on_status: Box::new(|status| {
            println!(
                "frames/s {:>3} (dropped total {}), game {}, screen {}{}",
                status.frames_per_second,
                status.dropped_total,
                if status.game_connected { "connected" } else { "not running" },
                if status.screen_drawing { "drawing" } else { "not visible" },
                status.error.as_deref().map(|e| format!(" — {e}")).unwrap_or_default(),
            )
        }),
    };
    // One WestPlay at a time (the app, or this).
    let Some(_instance) = westplay_host::Instance::claim() else {
        eprintln!("WestPlay is already running; quit it first.");
        std::process::exit(1);
    };
    println!("WestPlay host {width}x{height}; Ctrl+C to quit");
    #[cfg(windows)]
    if let Err(e) = run(config, Arc::new(AtomicBool::new(false))) {
        eprintln!("host failed: {e}");
        std::process::exit(1);
    }
    #[cfg(target_os = "macos")]
    run_with_appkit(config);
}

/// Where the standalone host keeps its WebViews' profiles and maps.
fn data_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library").join("Application Support"));
    base.map(|dir| dir.join("WestPlay")).unwrap_or_else(|| std::env::temp_dir().join("WestPlay"))
}

/// The host on a thread of its own, AppKit's event loop on this one (the main thread) until the host ends.
#[cfg(target_os = "macos")]
fn run_with_appkit(config: Config) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let mtm = MainThreadMarker::new().expect("main runs on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    std::thread::Builder::new()
        .name("westplay-host".into())
        .spawn(move || {
            let code = match run(config, Arc::new(AtomicBool::new(false))) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("host failed: {e}");
                    1
                }
            };
            std::process::exit(code);
        })
        .expect("the host's thread");
    app.run();
}
