//! The GPU on macOS: the one the cab is said to be drawn on (WebKit picks its own), for the companion to show.

use crate::graphics::{self, LUID};

pub struct Gpu {
    pub luid: LUID,
    pub adapter_name: String,
}

impl Gpu {
    /// The GPU `luid` names, or the Mac's first (its high-performance one).
    pub fn new(luid: Option<LUID>) -> Result<Self, crate::webview::Error> {
        let adapters = graphics::adapters();
        let adapter = luid
            .and_then(|luid| adapters.iter().find(|adapter| graphics::same(adapter.luid, luid)))
            .or_else(|| adapters.first())
            .ok_or("Metal lists no GPU")?;
        Ok(Self { luid: adapter.luid, adapter_name: adapter.name.clone() })
    }
}
