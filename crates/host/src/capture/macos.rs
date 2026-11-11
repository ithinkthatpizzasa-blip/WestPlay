//! Captures the stage on macOS: WebKit's snapshot of each WebView shown on it, composited into the frame (each where
//! it's placed, the page squeezed into the frame where it's laid out taller), and published into the frame block
//! the plugin reads (`westplay_shared::frames`), at most [`super::MAX_FPS`] a second (docs/MACOS_PORT.md, Phase 2).
//!
//! A frame the same as the last isn't published again (an idle screen costs the game little, as WGC sends frames
//! only as the content changes), but the last goes out again every [`RESEND_IDLE`]: Windows' shared texture always
//! holds the last frame for a game that starts, and the frame block holds it only until it's taken. The cursor is drawn here, over each frame as it goes out
//! (`cursor::paint`), and over the last again as it moves.
//!
//! A WebView whose page was the same in its last two snapshots isn't snapshotted again until the host does something
//! to it, or [`QUIET_RECHECK`] has passed: the cabin page at rest costs next to nothing beside a video player that
//! plays. A round with no snapshot due leaves the last frame as it was.
//!
//! Threads: snapshots are taken and composited on the main thread (WebKit's), one round at a time, each round's
//! frame left in the [`Mailbox`]; the host's loop starts the rounds and publishes what's due ([`Capture::flush`]).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSImage;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
    CGInterpolationQuality, kCGColorSpaceSRGB,
};
use objc2_foundation::{NSError, NSNumber};
use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};
use westplay_shared::BridgeMapping;
use westplay_shared::frames::{FrameWriter, FramesMapping};

use super::{CURSOR_INTERVAL, FRAME_INTERVAL, FRAME_JITTER, Mailbox, flush};
use crate::cursor::{self, Look};
use crate::webview::{self, Stage, macos::StageShared};

/// How often the last frame goes out again while none new comes: a game started meanwhile gets it.
const RESEND_IDLE: Duration = Duration::from_millis(500);

/// A composited frame: `width` x `height` BGRA, premultiplied, rows top first.
type Pixels = Arc<Vec<u8>>;

pub struct Capture {
    stage: Arc<StageShared>,
    bridge: Arc<BridgeMapping>,
    block: FramesMapping,
    mailbox: Arc<Mailbox<Pixels>>,
    /// A round of snapshots under way on the main thread, and when the last started.
    in_round: Arc<AtomicBool>,
    round_at: Cell<Instant>,
    /// The last frame composited, to tell a new one apart (the main thread's).
    last: Arc<Mutex<Option<Pixels>>>,
    /// The page's last frame as composited, without the cursor: what the cursor is drawn over, again as it moves.
    clean: RefCell<Option<Pixels>>,
    pub frames: Arc<AtomicU64>,
    /// Frames composited that differed from the one before (each the page drawn anew).
    pub arrived: Arc<AtomicU64>,
    /// Frames that couldn't go out (none here: the frame block never makes either side wait).
    pub dropped: Arc<AtomicU64>,
    /// The cursor as last drawn into the frame, and when.
    drawn: Cell<Option<Look>>,
    cursor_at: Cell<Instant>,
}

impl Capture {
    pub fn start(stage: &Stage, bridge: Arc<BridgeMapping>) -> Result<Self, webview::Error> {
        let block = FramesMapping::open()?;
        let host = &bridge.bridge().host;
        let (width, height) = stage.shared.frame;
        host.frame_width.store(width, Ordering::Relaxed);
        host.frame_height.store(height, Ordering::Relaxed);
        let now = Instant::now();
        Ok(Self {
            stage: stage.shared.clone(),
            bridge,
            block,
            mailbox: Arc::new(Mailbox::new(now)),
            in_round: Arc::default(),
            round_at: Cell::new(now - FRAME_INTERVAL),
            last: Arc::default(),
            clean: RefCell::new(None),
            frames: Arc::default(),
            arrived: Arc::default(),
            dropped: Arc::default(),
            drawn: Cell::new(None),
            cursor_at: Cell::new(now),
        })
    }

    /// Whether the cursor is drawn here (else the page draws its own): always, on macOS.
    pub fn draws_cursor(&self) -> bool {
        true
    }

    /// Whether the capture can't go on (it always can: the frame block has no lock to abandon).
    pub fn broken(&self) -> bool {
        false
    }

    /// Starts the next round of snapshots when it's time; publishes the newest frame if it's waiting and due, with
    /// the `cursor` drawn over it (the page squeezed into the frame `squeeze` times its height); else, the cursor
    /// moved or changed, the page's last frame again with it (called every turn of the host's loop).
    pub fn flush(&self, cursor: Option<Look>, squeeze: f32) {
        let now = Instant::now();
        if !self.in_round.load(Ordering::Acquire) && now + FRAME_JITTER >= self.round_at.get() + FRAME_INTERVAL {
            self.round_at.set(now);
            self.start_round();
        }
        let sent = flush(&self.mailbox, now, |frame| {
            *self.clean.borrow_mut() = Some(frame.clone());
            self.compose(cursor, squeeze)
        });
        if sent == Some(true) {
            self.sent(cursor, now);
        }
        // The cursor moved or changed, and no new frame of the page's to take it, or none for a while: the last one
        // again, with it.
        let since = now.saturating_duration_since(self.cursor_at.get());
        if sent != Some(true)
            && self.clean.borrow().is_some()
            && ((cursor != self.drawn.get() && since >= CURSOR_INTERVAL) || since >= RESEND_IDLE)
            && self.compose(cursor, squeeze)
        {
            self.sent(cursor, now);
        }
    }

    /// A frame went out, with `cursor` on it.
    fn sent(&self, cursor: Option<Look>, now: Instant) {
        self.bridge.bridge().host.frame_seq.fetch_add(1, Ordering::Release);
        self.frames.fetch_add(1, Ordering::Relaxed);
        self.drawn.set(cursor);
        self.cursor_at.set(now);
    }

    /// The page's last frame into the frame block, the cursor drawn over it.
    fn compose(&self, cursor: Option<Look>, squeeze: f32) -> bool {
        let clean = self.clean.borrow();
        let Some(clean) = clean.as_ref() else { return false };
        let (width, height) = self.stage.frame;
        let seq = self.frames.load(Ordering::Relaxed) + 1;
        // The writer takes the slot the plugin isn't reading, each time (it holds nothing between frames).
        let mut writer = FrameWriter::new(self.block.block());
        writer.write(seq, width, height, |pixels| {
            pixels.copy_from_slice(&clean[..pixels.len()]);
            if let Some(look) = cursor {
                cursor::paint(pixels, width, height, &look, squeeze);
            }
        })
    }

    /// Snapshots each WebView shown, on the main thread, and leaves their composite in the mailbox once they're
    /// all in (if it's new).
    fn start_round(&self) {
        self.in_round.store(true, Ordering::Release);
        let (stage, mailbox, in_round, last, arrived) =
            (self.stage.clone(), self.mailbox.clone(), self.in_round.clone(), self.last.clone(), self.arrived.clone());
        DispatchQueue::main().exec_async(move || {
            // SAFETY: the main queue's work runs on the main thread.
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            snapshot_round(&stage, mtm, move |frame| {
                let Some(frame) = frame else {
                    in_round.store(false, Ordering::Release);
                    return;
                };
                let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                if last.as_ref().is_none_or(|before| **before != *frame) {
                    let frame = Arc::new(frame);
                    *last = Some(frame.clone());
                    arrived.fetch_add(1, Ordering::Relaxed);
                    drop(mailbox.put(frame));
                }
                in_round.store(false, Ordering::Release);
            });
        });
    }
}

/// Where a WebView's snapshot goes on the frame, and the snapshot once taken.
struct Shot {
    rect: CGRect,
    image: Option<Retained<CGImage>>,
}

/// A WebView's last snapshot, kept for the rounds in which its page looks to be at rest.
struct Cached {
    image: Option<Retained<CGImage>>,
    /// A digest of the snapshot's pixels, to tell whether the next differs.
    hash: u64,
    /// When it was taken.
    at: Instant,
    /// Whether the page changed from the snapshot before: such a page is snapshotted every round.
    active: bool,
    /// Where it was drawn on the frame.
    rect: CGRect,
}

/// How long a WebView whose page was at rest goes without a new snapshot (a page that starts moving of itself,
/// a CSS animation, is seen within this).
const QUIET_RECHECK: Duration = Duration::from_millis(250);
/// How long after the host last did something to a WebView (input, a message, a load) it's snapshotted every round.
const POKE_HOLD: Duration = Duration::from_millis(1000);

thread_local! {
    /// The main thread's snapshots, by WebView.
    static CACHE: RefCell<HashMap<u64, Cached>> = RefCell::new(HashMap::new());
}

/// A digest of an image's pixels (`None` if they can't be read).
fn hash_image(image: &CGImage) -> Option<u64> {
    let provider = CGImage::data_provider(Some(image))?;
    let data = CGDataProvider::data(Some(&provider))?;
    // SAFETY: the data's bytes, alive as long as `data` is.
    let bytes = unsafe { data.as_bytes_unchecked() };
    let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ bytes.len() as u64;
    let (words, rest) = bytes.as_chunks::<8>();
    for word in words {
        hash = (hash.rotate_left(5) ^ u64::from_le_bytes(*word)).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    for &byte in rest {
        hash = (hash.rotate_left(5) ^ u64::from(byte)).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    Some(hash)
}

/// One WebView shown on the stage, as a round sees it.
struct Layer {
    id: u64,
    view: Arc<MainThreadBound<Retained<WKWebView>>>,
    page_width: u32,
    rect: CGRect,
    poked: Instant,
}

/// Snapshots the WebViews shown on `stage` whose pages may have changed (the rest are taken from the last round),
/// then gives `done` their composite (on the main thread), or `None` if nothing in the frame can have changed.
fn snapshot_round(stage: &StageShared, mtm: MainThreadMarker, done: impl FnOnce(Option<Vec<u8>>) + 'static) {
    let (width, height) = stage.frame;
    let page_height = stage.page.lock().unwrap_or_else(|e| e.into_inner()).1.max(1);
    let squeeze = f64::from(height) / f64::from(page_height);
    let layers: Vec<Layer> = {
        let layers = stage.layers.lock().unwrap_or_else(|e| e.into_inner());
        layers
            .iter()
            .filter(|layer| layer.shown)
            .map(|layer| {
                let (w, h) =
                    (f64::from(layer.width) * f64::from(layer.scale), f64::from(layer.height) * f64::from(layer.scale));
                let y = f64::from(layer.y) * squeeze;
                // CG's rectangles count from the bottom.
                let rect = CGRect::new(
                    CGPoint::new(f64::from(layer.x), f64::from(height) - y - h * squeeze),
                    CGSize::new(w, h * squeeze),
                );
                Layer { id: layer.id, view: layer.view.clone(), page_width: layer.width, rect, poked: layer.poked }
            })
            .collect()
    };
    let now = Instant::now();
    // Which need a new snapshot; and whether the frame needs drawing again anyway (a WebView gone or moved).
    let mut redraw = false;
    let mut shots = Vec::with_capacity(layers.len());
    let stale: Vec<bool> = CACHE.with_borrow_mut(|cache| {
        let before = cache.len();
        cache.retain(|id, _| layers.iter().any(|layer| layer.id == *id));
        redraw |= cache.len() != before;
        layers
            .iter()
            .map(|layer| {
                let Some(cached) = cache.get_mut(&layer.id) else {
                    shots.push(Shot { rect: layer.rect, image: None });
                    return true;
                };
                if cached.rect != layer.rect {
                    cached.rect = layer.rect;
                    redraw = true;
                }
                shots.push(Shot { rect: layer.rect, image: cached.image.clone() });
                cached.image.is_none()
                    || cached.active
                    || layer.poked >= cached.at
                    || now.saturating_duration_since(layer.poked) < POKE_HOLD
                    || now.saturating_duration_since(cached.at) >= QUIET_RECHECK
            })
            .collect()
    });
    let left = stale.iter().filter(|&&stale| stale).count();
    if left == 0 {
        if redraw {
            done(Some(composite(width, height, &shots)));
        } else {
            done(None);
        }
        return;
    }
    let shots = Rc::new(RefCell::new(shots));
    let left = Rc::new(Cell::new(left));
    let done = Rc::new(RefCell::new(Some(done)));
    let finish = {
        let (shots, done) = (shots.clone(), done.clone());
        move || {
            if let Some(done) = done.borrow_mut().take() {
                done(Some(composite(width, height, &shots.borrow())));
            }
        }
    };
    let finish = Rc::new(finish);
    for (index, layer) in layers.into_iter().enumerate().filter(|(index, _)| stale[*index]) {
        let (shots, left, finish) = (shots.clone(), left.clone(), finish.clone());
        let (id, rect, page_width) = (layer.id, layer.rect, layer.page_width);
        let block = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
            // SAFETY: WebKit's image (or none), alive through the call.
            let cg = unsafe { image.as_ref() }.and_then(|image| {
                // SAFETY: AppKit's own call on its image; no rectangle, context or hints: the image's whole.
                unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }
            });
            let hash = cg.as_deref().and_then(hash_image);
            let kept = CACHE.with_borrow_mut(|cache| {
                let before = cache.remove(&id);
                // A page that differs from its last snapshot (or that can't be told) stays in the rounds.
                let active = match (&before, hash) {
                    (Some(before), Some(hash)) => before.hash != hash,
                    _ => true,
                };
                let image = cg.or(before.and_then(|before| before.image));
                cache.insert(
                    id,
                    Cached { image: image.clone(), hash: hash.unwrap_or(0), at: Instant::now(), active, rect },
                );
                image
            });
            shots.borrow_mut()[index].image = kept;
            left.set(left.get() - 1);
            if left.get() == 0 {
                finish();
            }
        });
        // SAFETY: WebKit's own calls on its WebView and configuration, on the main thread; the block is kept by WebKit
        // until it calls it.
        unsafe {
            let config = WKSnapshotConfiguration::new(mtm);
            // In points, which are the page's pixels: the WebViews draw at 1x (`webview::macos::draw_at_1x`).
            config.setSnapshotWidth(Some(&NSNumber::new_f64(f64::from(page_width))));
            layer.view.get(mtm).takeSnapshotWithConfiguration_completionHandler(Some(&config), &block);
        }
    }
}

/// The frame, `width` x `height`: each snapshot drawn where it goes, over the ones before, on nothing.
fn composite(width: u32, height: u32, shots: &[Shot]) -> Vec<u8> {
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let space = CGColorSpace::with_name(Some(
        // SAFETY: Core Graphics' own constant.
        unsafe { kCGColorSpaceSRGB },
    ));
    // SAFETY: a bitmap context over `pixels`, which outlives it (it's dropped at the end of this function), of the
    // buffer's size and layout: BGRA (32 bits, little-endian), alpha premultiplied.
    let context = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width as usize,
            height as usize,
            8,
            width as usize * 4,
            space.as_deref(),
            CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0,
        )
    };
    let Some(context) = context else { return pixels };
    CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::High);
    for shot in shots {
        if let Some(image) = &shot.image {
            CGContext::draw_image(Some(&context), shot.rect, Some(image));
        }
    }
    drop(context);
    pixels
}
