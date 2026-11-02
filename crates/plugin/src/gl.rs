//! OpenGL hooks (macOS).
//!
//! The Mac game draws with OpenGL 4.1 (Apple's, over Metal), through libGL's functions, which it looks up by name at
//! start-up (docs/MACOS_PORT.md). Each of them calls on through the current context's dispatch table: the `disp` of
//! a `CGLContextObj` (`CGLContext.h`, `gliDispatch.h`: the table `CGLMacro.h` calls through, so a fixed layout). So
//! the plugin points entries of the game's contexts' tables at its own functions, and changes none of the game's
//! code:
//! - `bind_texture`: a texture about to be drawn with, perhaps a screen's (`screens`);
//! - `tex_image2D`, `tex_storage2D`, `tex_sub_image2D`, `delete_textures`: a texture's contents made again, or its
//!   name let go.
//!
//! The frame clock is the end of the game's frame, which it ends one of three ways (1.57's code that does it):
//! `-[NSOpenGLContext flushBuffer]`, `glFlush` (not hooked: called elsewhere too), or `glFlushRenderAPPLE` then
//! `glSwapAPPLE` (the table's `swap_APPLE`, the one seen in the game, 2026-11-02). The context of that frame is the
//! one it draws with, and its table is hooked then if it wasn't already (at init, the context current on the game's
//! main thread is). Each context hooked is held (`CGLRetainContext`) until the hooks are
//! removed, so its table is never freed while it may be put back.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use objc2::runtime::{AnyObject, Imp, Sel};
use objc2::{msg_send, sel};

use crate::hooks::{guarded, hook_method, hook_slot};
use crate::{bridge, log, screens};

/// A GL context as the dispatch table's functions take it (the `rend` of a `CGLContextObj`).
pub type Rend = *mut c_void;
pub type GLenum = u32;
pub type GLuint = u32;
pub type GLint = i32;
pub type GLsizei = i32;

pub const GL_TEXTURE_2D: GLenum = 0x0DE1;
pub const GL_TEXTURE_BINDING_2D: GLenum = 0x8069;
pub const GL_TEXTURE_WIDTH: GLenum = 0x1000;
pub const GL_TEXTURE_HEIGHT: GLenum = 0x1001;
pub const GL_TEXTURE_INTERNAL_FORMAT: GLenum = 0x1003;
pub const GL_SRGB8_ALPHA8: GLenum = 0x8C43;
pub const GL_BGRA: GLenum = 0x80E1;
pub const GL_UNSIGNED_INT_8_8_8_8_REV: GLenum = 0x8367;
pub const GL_PIXEL_UNPACK_BUFFER: GLenum = 0x88EC;
pub const GL_PIXEL_UNPACK_BUFFER_BINDING: GLenum = 0x88EF;
/// The unpacking state an upload of WestPlay's sets, and what it's set to: `GL_UNPACK_SWAP_BYTES`, `_LSB_FIRST`,
/// `_ROW_LENGTH`, `_SKIP_ROWS`, `_SKIP_PIXELS`, `_ALIGNMENT`.
pub const UNPACKING: [(GLenum, GLint); 6] =
    [(0x0CF0, 0), (0x0CF1, 0), (0x0CF2, 0), (0x0CF3, 0), (0x0CF4, 0), (0x0CF5, 4)];

/// Where the dispatch table is in a `CGLContextObj` (in pointers: after `rend`).
const DISPATCH: usize = 1;
/// The entries of the dispatch table used (in pointers: `offsetof(GLIFunctionDispatch, …) / 8`, macOS 27's SDK).
mod entry {
    pub const BIND_TEXTURE: usize = 5;
    pub const DELETE_TEXTURES: usize = 59;
    pub const GET_INTEGERV: usize = 104;
    pub const GET_TEX_LEVEL_PARAMETERIV: usize = 125;
    pub const PIXEL_STOREI: usize = 195;
    pub const SWAP_APPLE: usize = 590;
    pub const TEX_IMAGE_2D: usize = 301;
    pub const TEX_SUB_IMAGE_2D: usize = 307;
    pub const BIND_BUFFER: usize = 642;
    pub const TEX_STORAGE_2D: usize = 970;
}

type BindTextureFn = unsafe extern "C" fn(Rend, GLenum, GLuint);
type DeleteTexturesFn = unsafe extern "C" fn(Rend, GLsizei, *const GLuint);
type GetIntegervFn = unsafe extern "C" fn(Rend, GLenum, *mut GLint);
type GetTexLevelParameterivFn = unsafe extern "C" fn(Rend, GLenum, GLint, GLenum, *mut GLint);
type PixelStoreiFn = unsafe extern "C" fn(Rend, GLenum, GLint);
type TexImage2DFn =
    unsafe extern "C" fn(Rend, GLenum, GLint, GLint, GLsizei, GLsizei, GLint, GLenum, GLenum, *const c_void);
type TexSubImage2DFn =
    unsafe extern "C" fn(Rend, GLenum, GLint, GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, *const c_void);
type BindBufferFn = unsafe extern "C" fn(Rend, GLenum, GLuint);
type TexStorage2DFn = unsafe extern "C" fn(Rend, GLenum, GLsizei, GLenum, GLsizei, GLsizei);
type SwapAppleFn = unsafe extern "C" fn(Rend);
type FlushBufferFn = unsafe extern "C-unwind" fn(*mut AnyObject, Sel);

static ORIG_BIND_TEXTURE: AtomicUsize = AtomicUsize::new(0);
static ORIG_DELETE_TEXTURES: AtomicUsize = AtomicUsize::new(0);
static ORIG_TEX_IMAGE_2D: AtomicUsize = AtomicUsize::new(0);
static ORIG_TEX_SUB_IMAGE_2D: AtomicUsize = AtomicUsize::new(0);
static ORIG_TEX_STORAGE_2D: AtomicUsize = AtomicUsize::new(0);
static ORIG_SWAP_APPLE: AtomicUsize = AtomicUsize::new(0);
static ORIG_FLUSH_BUFFER: AtomicUsize = AtomicUsize::new(0);

static FRAME: AtomicU64 = AtomicU64::new(0);
static FIRST_FLUSH_LOGGED: AtomicBool = AtomicBool::new(false);

#[link(name = "OpenGL", kind = "framework")]
unsafe extern "C" {
    fn CGLGetCurrentContext() -> *mut c_void;
    fn CGLRetainContext(context: *mut c_void) -> *mut c_void;
    fn CGLReleaseContext(context: *mut c_void);
}

/// The contexts whose tables are hooked (each held), by address.
static CONTEXTS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn contexts() -> std::sync::MutexGuard<'static, Vec<usize>> {
    CONTEXTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// # Safety
/// In the game's process, on its main thread, before the hooks are switched on ([`crate::hooks::enable_all`]).
pub unsafe fn install() -> Result<(), String> {
    // SAFETY: `flushBuffer` takes no arguments and returns nothing, as its detour; the context current on the
    // game's main thread at init is the game's (docs/MACOS_PORT.md), alive while it's current.
    unsafe {
        let flush: FlushBufferFn = flush_buffer_hook;
        hook_method(
            c"NSOpenGLContext",
            sel!(flushBuffer),
            std::mem::transmute::<FlushBufferFn, Imp>(flush),
            &ORIG_FLUSH_BUFFER,
            "-[NSOpenGLContext flushBuffer]",
        )?;
        let current = CGLGetCurrentContext();
        if current.is_null() {
            log!("gl: no context current at init: its tables hooked as the game flushes");
        } else {
            hook_context(current);
        }
        Ok(())
    }
}

/// Diagnostics on unload, and the contexts let go (the hooks themselves are removed by `hooks::remove_all`, first).
pub fn on_unload() {
    screens::log_stats();
    for context in contexts().drain(..) {
        // SAFETY: held since hooked (`hook_context`), let go once (taken out of the list).
        unsafe { CGLReleaseContext(context as *mut c_void) };
    }
}

/// Hooks `context`'s dispatch table, unless it is already (or its functions aren't the ones hooked elsewhere: a
/// context of another renderer, left as it is).
///
/// # Safety
/// `context` is a live `CGLContextObj`.
unsafe fn hook_context(context: *mut c_void) {
    let mut held = contexts();
    if held.contains(&(context as usize)) {
        return;
    }
    // SAFETY: a live context (the caller's contract) begins with `rend` and its dispatch table, every entry a
    // function of the signature given here (gliDispatch.h); held from now on, so the table outlives the hooks.
    unsafe {
        let table = (context as *mut usize).add(DISPATCH);
        let hooks: [(usize, usize, &AtomicUsize, &str); 6] = [
            (entry::BIND_TEXTURE, bind_texture_hook as BindTextureFn as usize, &ORIG_BIND_TEXTURE, "bind_texture"),
            (
                entry::DELETE_TEXTURES,
                delete_textures_hook as DeleteTexturesFn as usize,
                &ORIG_DELETE_TEXTURES,
                "delete_textures",
            ),
            (entry::TEX_IMAGE_2D, tex_image_2d_hook as TexImage2DFn as usize, &ORIG_TEX_IMAGE_2D, "tex_image2D"),
            (
                entry::TEX_SUB_IMAGE_2D,
                tex_sub_image_2d_hook as TexSubImage2DFn as usize,
                &ORIG_TEX_SUB_IMAGE_2D,
                "tex_sub_image2D",
            ),
            (
                entry::TEX_STORAGE_2D,
                tex_storage_2d_hook as TexStorage2DFn as usize,
                &ORIG_TEX_STORAGE_2D,
                "tex_storage2D",
            ),
            (entry::SWAP_APPLE, swap_apple_hook as SwapAppleFn as usize, &ORIG_SWAP_APPLE, "swap_APPLE"),
        ];
        // All or none: a table of other functions is another renderer's.
        let other = hooks.iter().find(|(entry, detour, original, _)| {
            let function = *table.add(*entry);
            let had = original.load(Ordering::Acquire);
            function == 0 || (function != *detour && had != 0 && had != function)
        });
        if let Some((_, _, _, name)) = other {
            log!("gl: context {context:#x?} left alone: its {name} isn't the one hooked");
            return;
        }
        for (entry, detour, original, name) in hooks {
            if let Err(e) = hook_slot(table.add(entry), detour, original, name) {
                log!("gl: context {context:#x?}: {e}");
            }
        }
        CGLRetainContext(context);
        held.push(context as usize);
        log!("gl: context {context:#x?} hooked ({} so far)", held.len());
    }
}

/// The current context's own functions, for the plugin's calls into it: the hooked entries' originals, the others
/// its table's.
pub struct Gl {
    rend: Rend,
    table: *const usize,
}

impl Gl {
    /// The calling thread's current context.
    ///
    /// # Safety
    /// On a thread whose current context is a live one (inside one of its functions: a hook).
    pub unsafe fn current() -> Option<Gl> {
        // SAFETY: a live context (the caller's contract) begins with `rend` and its dispatch table.
        unsafe {
            let context = CGLGetCurrentContext() as *const usize;
            if context.is_null() {
                return None;
            }
            Some(Gl { rend: *context as Rend, table: context.add(DISPATCH) })
        }
    }

    /// The function of `entry`: `original`'s if it's hooked, the table's otherwise.
    fn function(&self, entry: usize, original: Option<&AtomicUsize>) -> usize {
        match original.map(|original| original.load(Ordering::Acquire)) {
            Some(function) if function != 0 => function,
            // SAFETY: a live context's table (`current`), `entry` within it.
            _ => unsafe { *self.table.add(entry) },
        }
    }

    /// # Safety
    /// On the context's thread, `name` a value `glGetIntegerv` takes one number for.
    pub unsafe fn get_integer(&self, name: GLenum) -> GLint {
        let mut value = 0;
        // SAFETY: the context's own function, with its context (the caller's contract), and room for one value.
        unsafe {
            let get: GetIntegervFn = std::mem::transmute(self.function(entry::GET_INTEGERV, None));
            get(self.rend, name, &mut value);
        }
        value
    }

    /// One of the 2D texture bound's levels' sizes or format (`GL_TEXTURE_WIDTH`…; 0: no such level).
    ///
    /// # Safety
    /// On the context's thread.
    pub unsafe fn texture_level(&self, level: GLint, name: GLenum) -> GLint {
        let mut value = 0;
        // SAFETY: the context's own function, with its context (the caller's contract), and room for one value.
        unsafe {
            let get: GetTexLevelParameterivFn =
                std::mem::transmute(self.function(entry::GET_TEX_LEVEL_PARAMETERIV, None));
            get(self.rend, GL_TEXTURE_2D, level, name, &mut value);
        }
        value
    }

    /// # Safety
    /// On the context's thread.
    pub unsafe fn pixel_store(&self, name: GLenum, value: GLint) {
        // SAFETY: the context's own function, with its context (the caller's contract).
        unsafe {
            let store: PixelStoreiFn = std::mem::transmute(self.function(entry::PIXEL_STOREI, None));
            store(self.rend, name, value);
        }
    }

    /// # Safety
    /// On the context's thread, `buffer` one of its (or 0).
    pub unsafe fn bind_unpack_buffer(&self, buffer: GLuint) {
        // SAFETY: the context's own function, with its context (the caller's contract).
        unsafe {
            let bind: BindBufferFn = std::mem::transmute(self.function(entry::BIND_BUFFER, None));
            bind(self.rend, GL_PIXEL_UNPACK_BUFFER, buffer);
        }
    }

    /// Writes BGRA rows (`width` × `height`, at `y`) into the 2D texture bound's first level.
    ///
    /// # Safety
    /// On the context's thread, no unpack buffer bound and unpacking as [`UNPACKING`] sets it, `pixels` holding
    /// `width` × `height` × 4 bytes, the rows within the texture.
    pub unsafe fn write_rows(&self, y: GLint, width: GLsizei, height: GLsizei, pixels: *const u8) {
        // SAFETY: the context's own function, with its context (the caller's contract) and the pixels it reads.
        unsafe {
            let write: TexSubImage2DFn =
                std::mem::transmute(self.function(entry::TEX_SUB_IMAGE_2D, Some(&ORIG_TEX_SUB_IMAGE_2D)));
            write(
                self.rend,
                GL_TEXTURE_2D,
                0,
                0,
                y,
                width,
                height,
                GL_BGRA,
                GL_UNSIGNED_INT_8_8_8_8_REV,
                pixels as *const c_void,
            );
        }
    }
}

/// # Safety
/// Called in place of the context's `bind_texture`, with its arguments.
unsafe extern "C" fn bind_texture_hook(rend: Rend, target: GLenum, texture: GLuint) {
    // SAFETY: the original was stored before the hook was switched on, called with the game's arguments as they
    // came; then the texture bound is the game's, on its current context.
    unsafe {
        let original: BindTextureFn = std::mem::transmute(ORIG_BIND_TEXTURE.load(Ordering::Acquire));
        original(rend, target, texture);
        if target == GL_TEXTURE_2D && texture != 0 {
            guarded(|| {
                if let Some(gl) = Gl::current() {
                    screens::on_bind(&gl, texture);
                }
            });
        }
    }
}

/// # Safety
/// Called in place of the context's `delete_textures`, with its arguments.
unsafe extern "C" fn delete_textures_hook(rend: Rend, count: GLsizei, textures: *const GLuint) {
    // SAFETY: the game's `count` names (checked for a sane number), read before they're let go; then the original
    // (stored before the hook was switched on), with the game's arguments as they came.
    unsafe {
        if !textures.is_null() && (1..=1 << 20).contains(&count) {
            let names = std::slice::from_raw_parts(textures, count as usize);
            guarded(|| screens::on_deleted(names));
        }
        let original: DeleteTexturesFn = std::mem::transmute(ORIG_DELETE_TEXTURES.load(Ordering::Acquire));
        original(rend, count, textures)
    }
}

/// The 2D texture bound's contents were made again by the game: whatever it was is to be looked at again.
///
/// # Safety
/// Inside a hook of the current context's, after the original.
unsafe fn contents_changed(target: GLenum) {
    if target != GL_TEXTURE_2D {
        return;
    }
    guarded(|| {
        // SAFETY: inside a hook of the current context's (the caller's contract).
        unsafe {
            if let Some(gl) = Gl::current() {
                screens::on_contents_changed(gl.get_integer(GL_TEXTURE_BINDING_2D) as GLuint);
            }
        }
    });
}

/// # Safety
/// Called in place of the context's `tex_image2D`, with its arguments.
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn tex_image_2d_hook(
    rend: Rend,
    target: GLenum,
    level: GLint,
    internal_format: GLint,
    width: GLsizei,
    height: GLsizei,
    border: GLint,
    format: GLenum,
    kind: GLenum,
    pixels: *const c_void,
) {
    // SAFETY: the original (stored before the hook was switched on), with the game's arguments as they came.
    unsafe {
        let original: TexImage2DFn = std::mem::transmute(ORIG_TEX_IMAGE_2D.load(Ordering::Acquire));
        original(rend, target, level, internal_format, width, height, border, format, kind, pixels);
        contents_changed(target);
    }
}

/// # Safety
/// Called in place of the context's `tex_sub_image2D`, with its arguments.
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn tex_sub_image_2d_hook(
    rend: Rend,
    target: GLenum,
    level: GLint,
    x: GLint,
    y: GLint,
    width: GLsizei,
    height: GLsizei,
    format: GLenum,
    kind: GLenum,
    pixels: *const c_void,
) {
    // SAFETY: the original (stored before the hook was switched on), with the game's arguments as they came.
    unsafe {
        let original: TexSubImage2DFn = std::mem::transmute(ORIG_TEX_SUB_IMAGE_2D.load(Ordering::Acquire));
        original(rend, target, level, x, y, width, height, format, kind, pixels);
        contents_changed(target);
    }
}

/// # Safety
/// Called in place of the context's `tex_storage2D`, with its arguments.
unsafe extern "C" fn tex_storage_2d_hook(
    rend: Rend,
    target: GLenum,
    levels: GLsizei,
    internal_format: GLenum,
    width: GLsizei,
    height: GLsizei,
) {
    // SAFETY: the original (stored before the hook was switched on), with the game's arguments as they came.
    unsafe {
        let original: TexStorage2DFn = std::mem::transmute(ORIG_TEX_STORAGE_2D.load(Ordering::Acquire));
        original(rend, target, levels, internal_format, width, height);
        contents_changed(target);
    }
}

/// # Safety
/// Called in place of the context's `swap_APPLE` (the end of a frame), with its arguments.
unsafe extern "C" fn swap_apple_hook(rend: Rend) {
    // SAFETY: the context current is the one swapped, live through its call; then the original (stored before the
    // hook was switched on), with the game's arguments as they came.
    unsafe {
        guarded(|| on_flush(CGLGetCurrentContext()));
        let original: SwapAppleFn = std::mem::transmute(ORIG_SWAP_APPLE.load(Ordering::Acquire));
        original(rend)
    }
}

/// # Safety
/// Called in place of `-[NSOpenGLContext flushBuffer]`, on a live context.
unsafe extern "C-unwind" fn flush_buffer_hook(this: *mut AnyObject, cmd: Sel) {
    // SAFETY: the game's live context, asked for its CGL context; then the original (stored before the hook was
    // switched on), with the game's arguments as they came.
    unsafe {
        guarded(|| {
            if let Some(this) = this.as_ref() {
                let context: *mut c_void = msg_send![this, CGLContextObj];
                on_flush(context);
            }
        });
        let original: FlushBufferFn = std::mem::transmute(ORIG_FLUSH_BUFFER.load(Ordering::Acquire));
        original(this, cmd)
    }
}

/// The end of a game frame, `context` the one it was drawn with.
///
/// # Safety
/// `context` is a live `CGLContextObj` or null.
unsafe fn on_flush(context: *mut c_void) {
    let frame = FRAME.fetch_add(1, Ordering::Relaxed) + 1;
    screens::on_frame(frame);
    if let Some(bridge) = bridge::get() {
        bridge.plugin.heartbeat.store(frame, Ordering::Release);
    }
    if !FIRST_FLUSH_LOGGED.swap(true, Ordering::Relaxed) {
        log!("first flush: context {context:#x?}, on {} thread", if is_main_thread() { "the main" } else { "another" });
        if let Some(bridge) = bridge::get() {
            bridge.plugin.pid.store(std::process::id(), Ordering::Release);
        }
    }
    if !context.is_null() {
        // SAFETY: a live context (the caller's contract).
        unsafe { hook_context(context) };
    }
}

fn is_main_thread() -> bool {
    // SAFETY: a plain query of the calling thread.
    unsafe { libc_pthread_main_np() == 1 }
}

unsafe extern "C" {
    #[link_name = "pthread_main_np"]
    fn libc_pthread_main_np() -> i32;
}
