//! macOS: the screen textures in OpenGL.
//!
//! A texture is known by its name. The first time the game binds a name to draw with, its size and format are asked
//! (and kept: a frame binds thousands, each looked up in a table by name); a screen texture's are [`SCREEN_WIDTH`]
//! by the layout's height, sRGB, one level, as on Windows. A name's contents made again by the game, or the name
//! let go, and it's asked again at its next bind. Each time a screen texture is bound, the newest app frame is written
//! into it first (once a frame), through the game's own context, its unpacking state as it was after.
//!
//! The app's frames come through the frame block (`westplay_shared::frames`), not a GPU texture: written into the
//! texture as they are, so a frame costs one upload of its pixels.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use westplay_shared::frames::{FrameReader, FramesMapping};
use westplay_shared::{
    CAB_MARK_HEIGHT, CAB_MARK_WIDTH, SCREEN_HEIGHT, SCREEN_WIDTH, TABLET, screen_layout_for_texture, screen_layouts,
};

use super::{cab_drawn, drawing, frame, rows, transparent_pixels};
use crate::gl::{
    GL_PIXEL_UNPACK_BUFFER_BINDING, GL_SRGB8_ALPHA8, GL_TEXTURE_HEIGHT, GL_TEXTURE_INTERNAL_FORMAT, GL_TEXTURE_WIDTH,
    GLint, GLuint, Gl, UNPACKING,
};
use crate::{bridge, log};

/// Screen textures the game has not drawn for this many frames are forgotten (it makes them again e.g. when the
/// truck changes).
const STALE_AFTER_FRAMES: u64 = 600;

/// `ScreenTexture::version` when the texture holds transparent pixels.
const TRANSPARENT: u64 = u64::MAX;

/// What a texture name is, as far as it's known ([`Textures::kinds`]).
const UNKNOWN: u8 = 0;
const OTHER: u8 = 1;
const CAB_MARK: u8 = 2;
/// A screen texture: this plus its layout's index.
const SCREEN: u8 = 3;

/// Names past this aren't kept in the table (asked at every bind; the game's are far fewer).
const MOST_NAMES: usize = 1 << 20;

/// A screen texture the game draws.
struct ScreenTexture {
    name: GLuint,
    height: u32,
    /// Version of the app frame last written into it, [`TRANSPARENT`], or 0 for the mod's own picture.
    version: u64,
    last_used: u64,
}

#[derive(Default)]
struct Textures {
    /// What each name is, by name.
    kinds: Vec<u8>,
    screens: Vec<ScreenTexture>,
    frames: FrameSource,
    /// The textures of a screen's width (or the cab mark's size) not taken for one, as said in the log.
    said: HashSet<(GLint, GLint, GLint, bool)>,
    transparent: Vec<u8>,
}

static TEXTURES: Mutex<Option<Textures>> = Mutex::new(None);

fn with_textures<T>(f: impl FnOnce(&mut Textures) -> T) -> T {
    let mut textures = TEXTURES.lock().unwrap_or_else(|e| e.into_inner());
    f(textures.get_or_insert_with(Textures::default))
}

/// The game binds texture `name` to draw with, on `gl`'s context: if it's a screen's, the newest app frame is
/// written into it first.
///
/// # Safety
/// Inside the context's `bind_texture`, `name` bound to its `GL_TEXTURE_2D`.
pub unsafe fn on_bind(gl: &Gl, name: GLuint) {
    with_textures(|textures| {
        let mut kind = textures.kinds.get(name as usize).copied().unwrap_or(UNKNOWN);
        if kind == UNKNOWN {
            // SAFETY: inside the context's bind, `name` bound (the caller's contract).
            kind = unsafe { textures.classify(gl, name) };
        }
        match kind {
            OTHER | UNKNOWN => {}
            CAB_MARK => cab_drawn(),
            // SAFETY: as above.
            _ => unsafe { textures.refresh(gl, name, (kind - SCREEN) as usize) },
        }
    });
}

/// The game made the contents of texture `name` again: what it is is asked at its next bind.
pub fn on_contents_changed(name: GLuint) {
    with_textures(|textures| textures.forget(name));
}

/// The game let texture names go.
pub fn on_deleted(names: &[GLuint]) {
    with_textures(|textures| {
        for &name in names {
            textures.forget(name);
        }
    });
}

impl Textures {
    fn forget(&mut self, name: GLuint) {
        if let Some(kind) = self.kinds.get_mut(name as usize) {
            *kind = UNKNOWN;
        }
        // Its mod picture again (or another texture): written into at its next bind.
        if let Some(screen) = self.screens.iter_mut().find(|screen| screen.name == name) {
            screen.version = 0;
        }
    }

    /// What texture `name` is, asked of the context, and kept (unless it has no contents yet).
    ///
    /// # Safety
    /// As [`on_bind`].
    unsafe fn classify(&mut self, gl: &Gl, name: GLuint) -> u8 {
        // SAFETY: the texture bound to the context's `GL_TEXTURE_2D` (the caller's contract).
        let (width, height, format, more_levels) = unsafe {
            (
                gl.texture_level(0, GL_TEXTURE_WIDTH),
                gl.texture_level(0, GL_TEXTURE_HEIGHT),
                gl.texture_level(0, GL_TEXTURE_INTERNAL_FORMAT),
                gl.texture_level(1, GL_TEXTURE_WIDTH) != 0,
            )
        };
        if width <= 0 || height <= 0 {
            return UNKNOWN;
        }
        let as_made = format == GL_SRGB8_ALPHA8 as GLint && !more_levels;
        let kind = match screen_layout_for_texture(width as u32, height as u32) {
            Some(layout) if as_made => SCREEN + layout as u8,
            _ if as_made && (width as u32, height as u32) == (CAB_MARK_WIDTH, CAB_MARK_HEIGHT) => CAB_MARK,
            _ => OTHER,
        };
        if kind >= SCREEN {
            log!("{}: texture {name}, {width} x {height}", said((kind - SCREEN) as usize));
        } else if kind == OTHER
            && (width as u32 == SCREEN_WIDTH || (width as u32, height as u32) == (CAB_MARK_WIDTH, CAB_MARK_HEIGHT))
            && self.said.insert((width, height, format, more_levels))
        {
            // A screen's width, not taken for one: said once a kind, should the game load the mod's differently.
            log!(
                "texture {name}: {width} x {height}, format {format:#x}{}: not a screen's",
                if more_levels { ", more levels" } else { "" }
            );
        }
        let index = name as usize;
        if index < MOST_NAMES {
            if self.kinds.len() <= index {
                self.kinds.resize(index + 1, UNKNOWN);
            }
            self.kinds[index] = kind;
        }
        kind
    }

    /// Writes the newest app frame (or a transparent picture) into screen texture `name` of `layout`, bound.
    ///
    /// # Safety
    /// As [`on_bind`].
    unsafe fn refresh(&mut self, gl: &Gl, name: GLuint, layout: usize) {
        let frame = frame();
        let index = match self.screens.iter().position(|screen| screen.name == name) {
            Some(index) => index,
            None => {
                log!("{} texture loaded ({} in use)", said(layout), self.screens.len() + 1);
                let height = westplay_shared::ScreenLayout::texture_height(layout);
                self.screens.push(ScreenTexture { name, height, version: 0, last_used: frame });
                self.screens.len() - 1
            }
        };
        self.screens[index].last_used = frame;
        let enabled = drawing(layout);
        let latest = if enabled { self.frames.latest() } else { None };
        let screen = &mut self.screens[index];
        let version = latest.as_ref().map_or(TRANSPARENT, |(version, _)| *version);
        if screen.version == version {
            return;
        }
        // Still the screen texture it was (a name let go and made again on a context not hooked would be another).
        // SAFETY: the texture bound (the caller's contract).
        let size = unsafe { (gl.texture_level(0, GL_TEXTURE_WIDTH), gl.texture_level(0, GL_TEXTURE_HEIGHT)) };
        if size != (SCREEN_WIDTH as GLint, screen.height as GLint) {
            log!("{}: texture {name} is {} x {} now, not a screen's", said(layout), size.0, size.1);
            self.screens.remove(index);
            self.forget(name);
            return;
        }
        let height = screen.height;
        screen.version = version;
        // SAFETY: the texture bound (the caller's contract); the frame's pixels a whole frame's (`FrameSource`), the
        // transparent ones as tall as any screen texture.
        unsafe {
            let unpacking = Unpacking::set(gl);
            match latest {
                Some((_, pixels)) => {
                    let started = Instant::now();
                    gl.write_rows(0, SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32, pixels.as_ptr());
                    let row_bytes = SCREEN_WIDTH as usize * 4;
                    for (row, from) in rows(height).skip(SCREEN_HEIGHT as usize) {
                        gl.write_rows(row as i32, SCREEN_WIDTH as i32, 1, pixels[from as usize * row_bytes..].as_ptr());
                    }
                    with_upload_stats(|stats| stats.wrote(started.elapsed()));
                }
                None => {
                    if self.transparent.is_empty() {
                        self.transparent = transparent_pixels();
                    }
                    gl.write_rows(0, SCREEN_WIDTH as i32, height as i32, self.transparent.as_ptr());
                }
            }
            unpacking.restore(gl);
        }
    }
}

/// The context's unpacking state as the game had it, while WestPlay's upload sets its own.
struct Unpacking {
    buffer: GLint,
    values: [GLint; UNPACKING.len()],
}

impl Unpacking {
    /// # Safety
    /// On the context's thread.
    unsafe fn set(gl: &Gl) -> Self {
        // SAFETY: plain state of the context (the caller's contract).
        unsafe {
            let buffer = gl.get_integer(GL_PIXEL_UNPACK_BUFFER_BINDING);
            let values = UNPACKING.map(|(name, _)| gl.get_integer(name));
            if buffer != 0 {
                gl.bind_unpack_buffer(0);
            }
            for ((name, ours), theirs) in UNPACKING.iter().zip(values) {
                if theirs != *ours {
                    gl.pixel_store(*name, *ours);
                }
            }
            Self { buffer, values }
        }
    }

    /// # Safety
    /// As [`Self::set`], on the same context.
    unsafe fn restore(self, gl: &Gl) {
        // SAFETY: plain state of the context (the caller's contract), as it was.
        unsafe {
            for ((name, ours), theirs) in UNPACKING.iter().zip(self.values) {
                if theirs != *ours {
                    gl.pixel_store(*name, theirs);
                }
            }
            if self.buffer != 0 {
                gl.bind_unpack_buffer(self.buffer as GLuint);
            }
        }
    }
}

/// The frame block, mapped on first use (`None`: it couldn't be).
static FRAMES: OnceLock<Option<FramesMapping>> = OnceLock::new();

/// The app's newest frame, from the frame block.
#[derive(Default)]
struct FrameSource {
    reader: Option<FrameReader<'static>>,
    /// The frame taken last (its number and pixels' address: a whole frame's, the reader's until it takes another),
    /// if it was one the screens take.
    current: Option<(u64, usize)>,
}

// SAFETY: the pixels' address is the frame block's, mapped for good; used only through the mutex it's kept in.
unsafe impl Send for FrameSource {}

impl FrameSource {
    /// The newest frame (its number and pixels, [`SCREEN_WIDTH`] x [`SCREEN_HEIGHT`] BGRA), or `None` while the app
    /// isn't running or hasn't sent one.
    fn latest(&mut self) -> Option<(u64, &[u8])> {
        if !bridge::host_alive() {
            return None;
        }
        if self.reader.is_none() {
            let mapping = FRAMES.get_or_init(|| match FramesMapping::open() {
                Ok(mapping) => Some(mapping),
                Err(e) => {
                    log!("frames: the frame block unavailable: {e}");
                    None
                }
            });
            self.reader = Some(FrameReader::new(mapping.as_ref()?.block()));
        }
        let reader = self.reader.as_mut()?;
        if let Some(frame) = reader.take() {
            let fits = (frame.width, frame.height) == (SCREEN_WIDTH, SCREEN_HEIGHT)
                && frame.pixels.len() == westplay_shared::frames::FRAME_BYTES;
            self.current = fits.then(|| (frame.seq.max(1), frame.pixels.as_ptr() as usize));
            if !fits {
                log!("frames: a frame of {} x {} refused", frame.width, frame.height);
            }
        }
        let (seq, pixels) = self.current?;
        // SAFETY: a whole frame in the block, mapped for good, and the reader's until it takes another (`take`'s
        // contract): not before this borrow of `self` ends.
        Some((seq, unsafe { std::slice::from_raw_parts(pixels as *const u8, westplay_shared::frames::FRAME_BYTES) }))
    }
}

/// Says a screen's id for the log: "tablet" or "screen: <id>".
fn said(layout: usize) -> String {
    let id = screen_layouts()[layout].id;
    if id == TABLET { "tablet".to_owned() } else { format!("screen: {id}") }
}

/// The player is in another truck: nothing to say again on macOS.
pub(super) fn another_truck() {}

/// Forgets the screen textures not drawn for a while; whether that left none.
pub(super) fn release_unused(frame: u64) -> bool {
    with_textures(|textures| {
        let before = textures.screens.len();
        textures.screens.retain(|s| frame.saturating_sub(s.last_used) < STALE_AFTER_FRAMES);
        if textures.screens.len() == before {
            return false;
        }
        log!("{} screen textures unused, {} in use", before - textures.screens.len(), textures.screens.len());
        textures.screens.is_empty()
    })
}

/// How often what writing app frames costs is logged, while they're written.
const STATS_EVERY: Duration = Duration::from_secs(10);

/// What writing app frames into the screen textures costs the game: the time of each write in its frame (the
/// upload as the game's thread pays it; the GPU's copy is its own), and the game's frames, those with a write and
/// those without.
#[derive(Default)]
struct UploadStats {
    since: Option<Instant>,
    /// When the last game frame ended, and whether a frame was written in the one going on.
    frame_ended: Option<Instant>,
    in_frame: bool,
    writes: u32,
    write_time: Duration,
    write_most: Duration,
    frames_with: (u32, Duration),
    frames_without: (u32, Duration),
}

static UPLOAD_STATS: Mutex<Option<UploadStats>> = Mutex::new(None);

fn with_upload_stats(f: impl FnOnce(&mut UploadStats)) {
    f(UPLOAD_STATS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(UploadStats::default));
}

impl UploadStats {
    fn wrote(&mut self, took: Duration) {
        self.in_frame = true;
        self.writes += 1;
        self.write_time += took;
        self.write_most = self.write_most.max(took);
    }

    /// A game frame ended now: counted, and every [`STATS_EVERY`] logged if frames were written meanwhile.
    fn frame_ended(&mut self, now: Instant) {
        if let Some(before) = self.frame_ended {
            let frames = if self.in_frame { &mut self.frames_with } else { &mut self.frames_without };
            frames.0 += 1;
            frames.1 += now - before;
        }
        self.frame_ended = Some(now);
        self.in_frame = false;
        let since = *self.since.get_or_insert(now);
        let over = now - since;
        if over < STATS_EVERY {
            return;
        }
        if self.writes > 0 {
            let ms = |time: Duration, count: u32| time.as_secs_f64() * 1000.0 / f64::from(count.max(1));
            let frames = self.frames_with.0 + self.frames_without.0;
            log!(
                "frame uploads: {} in {:.1} s ({:.1} a second), {:.2} ms each on average, {:.2} ms at most; game frames {} ({:.1} a second): {:.2} ms with an upload, {:.2} ms without",
                self.writes,
                over.as_secs_f64(),
                f64::from(self.writes) / over.as_secs_f64(),
                ms(self.write_time, self.writes),
                self.write_most.as_secs_f64() * 1000.0,
                frames,
                f64::from(frames) / over.as_secs_f64(),
                ms(self.frames_with.1, self.frames_with.0),
                ms(self.frames_without.1, self.frames_without.0),
            );
        }
        *self = UploadStats { frame_ended: self.frame_ended, since: Some(now), ..UploadStats::default() };
    }
}

/// The end of a game frame (the frame clock).
pub fn frame_ended() {
    with_upload_stats(|stats| stats.frame_ended(Instant::now()));
}

pub fn log_stats() {
    let (screens, names) = with_textures(|textures| {
        (textures.screens.len(), textures.kinds.iter().filter(|kind| **kind != UNKNOWN).count())
    });
    log!("screen textures in use: {screens}; texture names known: {names}");
}
