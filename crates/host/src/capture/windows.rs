//! Captures the offscreen WebView2 visual with Windows Graphics Capture and publishes frames into the
//! shared texture, at most [`super::MAX_FPS`] per second. WGC only delivers frames when the content changes, so
//! an idle UI costs nothing; and so a frame that arrives before it's due is kept, not dropped, and goes out
//! when it is: dropped, it might be the last of an animation or a cursor move, and nothing would come to
//! replace it.
//!
//! The screen's cursor is drawn here, over each frame as it goes out (`cursor.rs`): the page's frame is kept
//! as captured, and as the cursor moves it goes out again from there with the cursor where it is now, the page
//! and Windows not waited for.
//!
//! Threads: WGC delivers frames on its own threads, which only leave the newest in a [`Mailbox`] (a swap,
//! no GPU work); the host's loop publishes it when due ([`Capture::flush`]), the only place frames are
//! copied. The mailbox's lock is never held across GPU work: WGC may call in while holding the device's
//! lock, and a lock held across a copy (which takes the device's lock) would deadlock against it.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use westplay_shared::BridgeMapping;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::UI::Composition::Visual;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11DeviceContext,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::core::{IInspectable, Interface, Result};

use super::{CURSOR_INTERVAL, Mailbox, flush};
use crate::cursor::{Look, Painter};
use crate::gpu::{Gpu, Lock, SharedFrame};

/// Frames in the capture pool: one kept in the mailbox, one being published, one WGC draws into.
const POOL_FRAMES: i32 = 3;

pub struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    mailbox: Arc<Mailbox<Direct3D11CaptureFrame>>,
    context: ID3D11DeviceContext,
    shared: Arc<SharedFrame>,
    bridge: Arc<BridgeMapping>,
    pub frames: Arc<AtomicU64>,
    /// Frames Windows Graphics Capture delivered (each the page drawn anew; some replaced before they're due).
    pub arrived: Arc<AtomicU64>,
    /// Times a frame couldn't go out because the game was copying the one before (it's tried again).
    pub dropped: Arc<AtomicU64>,
    /// Whether the shared texture's lock was abandoned (the game died holding it): the pipeline must be
    /// built anew.
    abandoned: AtomicBool,
    /// The page's last frame as captured, without the cursor: what the cursor is drawn over, again as it moves.
    clean: ID3D11Texture2D,
    has_clean: Cell<bool>,
    /// Draws the cursor (`None`: Direct2D couldn't be had here, and the page draws its own).
    painter: Option<Painter>,
    /// The cursor as last drawn into the shared frame, and when; whether drawing it failed (said once).
    drawn: Cell<Option<Look>>,
    cursor_at: Cell<Instant>,
    draw_failed: Cell<bool>,
}

impl Capture {
    pub fn start(gpu: &Gpu, visual: &Visual, shared: Arc<SharedFrame>, bridge: Arc<BridgeMapping>) -> Result<Self> {
        let item = GraphicsCaptureItem::CreateFromVisual(visual)?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &gpu.winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            POOL_FRAMES,
            item.Size()?,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(false);

        let mailbox = Arc::new(Mailbox::new(Instant::now()));
        let arrived = Arc::new(AtomicU64::new(0));
        pool.FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new({
            let mailbox = mailbox.clone();
            let arrived = arrived.clone();
            move |pool, _| {
                let Some(pool) = pool.as_ref() else { return Ok(()) };
                let frame = pool.TryGetNextFrame()?;
                arrived.fetch_add(1, Ordering::Relaxed);
                // The frame it replaces goes back to the pool here, after the mailbox is let go.
                drop(mailbox.put(frame));
                Ok(())
            }
        }))?;
        session.StartCapture()?;
        let clean = clean_texture(gpu, &shared)?;
        let painter = match Painter::new(gpu, &shared.texture) {
            Ok(painter) => Some(painter),
            Err(e) => {
                crate::log!("cursor: drawn by the page, Direct2D couldn't be had here: {e}");
                None
            }
        };
        Ok(Self {
            pool,
            session,
            mailbox,
            context: gpu.context.clone(),
            shared,
            bridge,
            frames: Arc::new(AtomicU64::new(0)),
            arrived,
            dropped: Arc::new(AtomicU64::new(0)),
            abandoned: AtomicBool::new(false),
            clean,
            has_clean: Cell::new(false),
            painter,
            drawn: Cell::new(None),
            cursor_at: Cell::new(Instant::now()),
            draw_failed: Cell::new(false),
        })
    }

    /// Whether the cursor is drawn here (else the page draws its own).
    pub fn draws_cursor(&self) -> bool {
        self.painter.is_some()
    }

    /// Whether the shared texture can't be used any more (its lock abandoned): build the pipeline anew.
    pub fn broken(&self) -> bool {
        self.abandoned.load(Ordering::Relaxed)
    }

    /// Publishes the newest frame if it's waiting and due, with the `cursor` drawn over it where it is now (the
    /// page squeezed into the frame `squeeze` times its height); else, the cursor moved or changed, the page's last
    /// frame again with it (called every turn of the host's loop).
    pub fn flush(&self, cursor: Option<Look>, squeeze: f32) {
        if self.broken() {
            return;
        }
        let cursor = cursor.filter(|_| self.painter.is_some());
        let now = Instant::now();
        let sent = flush(&self.mailbox, now, |frame| {
            let texture: Result<ID3D11Texture2D> = frame
                .Surface()
                // SAFETY: the frame's surface is a Direct3D surface (the capture's own frames), alive through the call.
                .and_then(|surface| unsafe { surface.cast::<IDirect3DDxgiInterfaceAccess>()?.GetInterface() });
            // A frame whose texture can't be had is done with all the same.
            let Ok(texture) = texture else { return true };
            match self.publish(&texture, cursor, squeeze) {
                Lock::Held => true,
                Lock::Busy => false,
                // Done with: the pipeline is built anew, with frames to come.
                Lock::Abandoned => {
                    self.abandoned.store(true, Ordering::Relaxed);
                    true
                }
            }
        });
        match sent {
            Some(true) => self.sent(cursor, now),
            Some(false) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            None => {}
        }
        // The cursor moved or changed, and no new frame of the page's to take it: the last one again, with it.
        if sent != Some(true)
            && self.has_clean.get()
            && cursor != self.drawn.get()
            && now.saturating_duration_since(self.cursor_at.get()) >= CURSOR_INTERVAL
        {
            match self.compose(cursor, squeeze) {
                Lock::Held => self.sent(cursor, now),
                // Tried again next turn.
                Lock::Busy => {}
                Lock::Abandoned => self.abandoned.store(true, Ordering::Relaxed),
            }
        }
    }

    /// A frame went out, with `cursor` on it.
    fn sent(&self, cursor: Option<Look>, now: Instant) {
        self.bridge.bridge().host.frame_seq.fetch_add(1, Ordering::Release);
        self.frames.fetch_add(1, Ordering::Relaxed);
        self.drawn.set(cursor);
        self.cursor_at.set(now);
    }

    /// Keeps a frame of the page's as captured, and sends it out with the cursor on it.
    fn publish(&self, frame: &ID3D11Texture2D, cursor: Option<Look>, squeeze: f32) -> Lock {
        // SAFETY: the frame and the clean copy are alive, the clean copy the shared frame's size; the region fits
        // both.
        unsafe {
            let mut desc = Default::default();
            frame.GetDesc(&mut desc);
            let region = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: desc.Width.min(self.shared.width),
                bottom: desc.Height.min(self.shared.height),
                back: 1,
            };
            self.context.CopySubresourceRegion(&self.clean, 0, 0, 0, 0, frame, 0, Some(&region));
        }
        self.has_clean.set(true);
        self.compose(cursor, squeeze)
    }

    /// The page's last frame into the shared texture, the cursor drawn over it, if its lock could be had
    /// ([`Lock::Held`]: done).
    fn compose(&self, cursor: Option<Look>, squeeze: f32) -> Lock {
        let lock = self.shared.try_lock();
        if lock != Lock::Held {
            return lock;
        }
        // SAFETY: both textures are alive and the same size and format; the shared one is locked (above).
        unsafe { self.context.CopyResource(&self.shared.texture, &self.clean) };
        if let (Some(look), Some(painter)) = (cursor, &self.painter)
            && let Err(e) = painter.draw(&look, squeeze)
            && !self.draw_failed.replace(true)
        {
            crate::log!("cursor: couldn't be drawn: {e}");
        }
        self.shared.unlock();
        Lock::Held
    }
}

/// The texture the page's frames are kept in as captured: the shared frame's size and format, ours alone.
fn clean_texture(gpu: &Gpu, shared: &SharedFrame) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: shared.width,
        Height: shared.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture = None;
    // SAFETY: the host's live device; the description is a local.
    unsafe { gpu.device.CreateTexture2D(&desc, None, Some(&mut texture))? };
    texture.ok_or_else(windows::core::Error::empty)
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
