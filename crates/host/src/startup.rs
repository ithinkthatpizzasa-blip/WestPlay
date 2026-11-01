//! Startup timing: each stage of a launch logged once (`log.rs`: the console in development, and the app's
//! log), with the local time and how long since the process started, to see where a slow launch goes. Start
//! `npm run dev` with the time printed (PowerShell: `Get-Date -Format HH:mm:ss.fff; npm run dev`), and the
//! gap to the first line is the dev server and the build before the app.

use std::collections::HashSet;
use std::sync::Mutex;

static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Prints `stage` with the time, the first time it's reached (later calls with the same stage do nothing).
pub fn mark(stage: &str) {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !seen.get_or_insert_with(HashSet::new).insert(stage.to_owned()) {
        return;
    }
    let now = crate::os::local_time();
    let uptime = crate::os::uptime_ms().map_or_else(|| "?".to_owned(), |ms| ms.to_string());
    log!(
        "[startup] {:02}:{:02}:{:02}.{:03}  +{uptime:>5} ms  {stage}",
        now.hour,
        now.minute,
        now.second,
        now.millisecond
    );
}
