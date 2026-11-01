//! Captures the stage on macOS: WebKit's snapshot of each WebView shown on it, composited into the frame (each where
//! it's placed, the page squeezed into the frame where it's laid out taller), and published into the frame block
//! the plugin reads (`westplay_shared::frames`), at most [`super::MAX_FPS`] a second (docs/MACOS_PORT.md, Phase 2).
//!
//! A frame the same as the last isn't published again (an idle screen costs the game nothing, as WGC sends frames
//! only as the content changes). The screen's cursor is drawn here, over each frame as it goes out
//! (`cursor::paint`), and over the last again as it moves.
//!
//! Threads: snapshots are taken and composited on the main thread (WebKit's), one round at a time, each round's
//! frame left in the [`Mailbox`]; the host's loop starts the rounds and publishes what's due ([`Capture::flush`]).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSImage;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
    CGInterpolationQuality, kCGColorSpaceSRGB,
};
use objc2_foundation::{NSError, NSNumber};
use objc2_web_kit::WKSnapshotConfiguration;
use westplay_shared::BridgeMapping;
use westplay_shared::frames::{FrameWriter, FramesMapping};

use super::{CURSOR_INTERVAL, FRAME_INTERVAL, FRAME_JITTER, Mailbox, flush};
use crate::cursor::{self, Look};
use crate::webview::{self, Stage, macos::StageShared};

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
        if sent != Some(true)
            && self.clean.borrow().is_some()
            && cursor != self.drawn.get()
            && now.saturating_duration_since(self.cursor_at.get()) >= CURSOR_INTERVAL
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

/// Snapshots every WebView shown on `stage`, then gives `done` their composite (on the main thread).
fn snapshot_round(stage: &StageShared, mtm: MainThreadMarker, done: impl FnOnce(Vec<u8>) + 'static) {
    let (width, height) = stage.frame;
    let page_height = stage.page.lock().unwrap_or_else(|e| e.into_inner()).1.max(1);
    let squeeze = f64::from(height) / f64::from(page_height);
    let scale = stage.window.get(mtm).backingScaleFactor();
    let layers: Vec<_> = {
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
                (layer.view.clone(), layer.width, rect)
            })
            .collect()
    };
    let shots: Rc<RefCell<Vec<Shot>>> =
        Rc::new(RefCell::new(layers.iter().map(|(_, _, rect)| Shot { rect: *rect, image: None }).collect()));
    let left = Rc::new(Cell::new(layers.len()));
    let done = Rc::new(RefCell::new(Some(done)));
    let finish = {
        let (shots, done) = (shots.clone(), done.clone());
        move || {
            if let Some(done) = done.borrow_mut().take() {
                done(composite(width, height, &shots.borrow()));
            }
        }
    };
    if layers.is_empty() {
        finish();
        return;
    }
    let finish = Rc::new(finish);
    for (index, (view, page_width, _)) in layers.into_iter().enumerate() {
        let (shots, left, finish) = (shots.clone(), left.clone(), finish.clone());
        let block = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
            // SAFETY: WebKit's image (or none), alive through the call.
            if let Some(image) = unsafe { image.as_ref() } {
                // SAFETY: AppKit's own call on its image; no rectangle, context or hints: the image's whole.
                let cg = unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) };
                shots.borrow_mut()[index].image = cg;
            }
            left.set(left.get() - 1);
            if left.get() == 0 {
                finish();
            }
        });
        // SAFETY: WebKit's own calls on its WebView and configuration, on the main thread; the block is kept by WebKit
        // until it calls it.
        unsafe {
            let config = WKSnapshotConfiguration::new(mtm);
            config.setSnapshotWidth(Some(&NSNumber::new_f64(f64::from(page_width) / scale)));
            view.get(mtm).takeSnapshotWithConfiguration_completionHandler(Some(&config), &block);
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
