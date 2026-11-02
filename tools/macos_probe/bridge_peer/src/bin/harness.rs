// The game's side of the plugin, without the game: loads WestPlay's plugin (x86_64, as the game is, so this runs under
// Rosetta: build with --target x86_64-apple-darwin) into a process with an OpenGL context current, as the game's main
// thread has at init, and plays the app too (its heartbeat, its frames, the screen switched on). Then makes a screen
// texture the way the game does (libGL's functions looked up by name), binds it as the game does to draw, and reads
// back what the plugin wrote into it.
//   harness <path to westplay.dylib>
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use westplay_shared::frames::{FrameWriter, FramesMapping};
use westplay_shared::{BridgeMapping, FixedStr, SCREEN_HEIGHT, SCREEN_WIDTH, ScreenLayout, TruckInfo, screen_layouts};

#[link(name = "OpenGL", kind = "framework")]
unsafe extern "C" {
    fn CGLChoosePixelFormat(attributes: *const i32, format: *mut *mut c_void, count: *mut i32) -> i32;
    fn CGLCreateContext(format: *mut c_void, share: *mut c_void, context: *mut *mut c_void) -> i32;
    fn CGLSetCurrentContext(context: *mut c_void) -> i32;
}
unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_SRGB8_ALPHA8: u32 = 0x8C43;
const GL_BGRA: u32 = 0x80E1;
const GL_UNSIGNED_INT_8_8_8_8_REV: u32 = 0x8367;
const GL_UNPACK_ALIGNMENT: u32 = 0x0CF5;
const GL_PACK_ALIGNMENT: u32 = 0x0D05;

unsafe extern "C" fn sdk_log(kind: i32, message: *const c_char) {
    println!("game log ({kind}): {}", unsafe { CStr::from_ptr(message) }.to_string_lossy());
}
unsafe extern "C" fn register_ok() -> i32 {
    0
}

#[repr(C)]
struct InitParams {
    game_name: *const c_char,
    game_id: *const c_char,
    game_version: u32,
    log: unsafe extern "C" fn(i32, *const c_char),
    register_for_event: *const c_void,
    unregister_from_event: *const c_void,
    register_for_channel: *const c_void,
    unregister_from_channel: *const c_void,
}

fn sym(lib: *mut c_void, name: &str) -> *mut c_void {
    let c = CString::new(name).unwrap();
    let f = unsafe { dlsym(lib, c.as_ptr()) };
    assert!(!f.is_null(), "no {name}");
    f
}

fn main() {
    let plugin = std::env::args().nth(1).expect("harness <westplay.dylib>");
    let pattern = |seq: u64, pixels: &mut [u8]| {
        for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as u32 % SCREEN_WIDTH, i as u32 / SCREEN_WIDTH);
            px.copy_from_slice(&[(x % 256) as u8, (y % 256) as u8, seq as u8, 255]);
        }
    };

    // The app's side: the bridge, the screen on, a heartbeat, frames; a truck whose screen WestPlay's is.
    let bridge = BridgeMapping::open().unwrap();
    assert!(bridge.matches());
    let b: &'static westplay_shared::Bridge = Box::leak(Box::new(bridge)).bridge();
    b.host.screen_enabled.store(1, Ordering::Release);
    let mut truck = TruckInfo::default();
    truck.id = FixedStr::new("vehicle.daf.2021");
    b.plugin.truck.write(&truck);
    static STOP: AtomicBool = AtomicBool::new(false);
    std::thread::spawn(move || {
        while !STOP.load(Ordering::Relaxed) {
            b.host.heartbeat.fetch_add(1, Ordering::Release);
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let frames: &'static FramesMapping = Box::leak(Box::new(FramesMapping::open().unwrap()));
    let mut writer = FrameWriter::new(frames.block());

    // A GL 4.1 core context, current, as the game's (Metal-backed, as the game's is on this Mac).
    let mut context = std::ptr::null_mut();
    unsafe {
        let attributes = [99 /* profile */, 0x4100 /* 4.1 core */, 73 /* accelerated */, 0];
        let (mut format, mut count) = (std::ptr::null_mut(), 0);
        assert_eq!(CGLChoosePixelFormat(attributes.as_ptr(), &mut format, &mut count), 0);
        assert_eq!(CGLCreateContext(format, std::ptr::null_mut(), &mut context), 0);
        assert_eq!(CGLSetCurrentContext(context), 0);
    }
    let gl = unsafe {
        dlopen(c"/System/Library/Frameworks/OpenGL.framework/Versions/Current/Libraries/libGL.dylib".as_ptr(), 1)
    };
    type GenTextures = unsafe extern "C" fn(i32, *mut u32);
    type BindTexture = unsafe extern "C" fn(u32, u32);
    type TexStorage2D = unsafe extern "C" fn(u32, i32, u32, i32, i32);
    type TexSubImage2D = unsafe extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    type GetTexImage = unsafe extern "C" fn(u32, i32, u32, u32, *mut c_void);
    type PixelStorei = unsafe extern "C" fn(u32, i32);
    type GetIntegerv = unsafe extern "C" fn(u32, *mut i32);
    type DeleteTextures = unsafe extern "C" fn(i32, *const u32);
    type Finish = unsafe extern "C" fn();
    let gen_textures: GenTextures = unsafe { std::mem::transmute(sym(gl, "glGenTextures")) };
    let bind: BindTexture = unsafe { std::mem::transmute(sym(gl, "glBindTexture")) };
    let storage: TexStorage2D = unsafe { std::mem::transmute(sym(gl, "glTexStorage2D")) };
    let sub_image: TexSubImage2D = unsafe { std::mem::transmute(sym(gl, "glTexSubImage2D")) };
    let get_image: GetTexImage = unsafe { std::mem::transmute(sym(gl, "glGetTexImage")) };
    let pixel_store: PixelStorei = unsafe { std::mem::transmute(sym(gl, "glPixelStorei")) };
    let get_integer: GetIntegerv = unsafe { std::mem::transmute(sym(gl, "glGetIntegerv")) };
    let delete: DeleteTextures = unsafe { std::mem::transmute(sym(gl, "glDeleteTextures")) };
    let finish: Finish = unsafe { std::mem::transmute(sym(gl, "glFinish")) };

    let bind_before = unsafe { *(context as *const usize).add(1 + 5) };
    // AppKit, as the game has it (its NSOpenGLContext).
    assert!(!unsafe { dlopen(c"/System/Library/Frameworks/AppKit.framework/AppKit".as_ptr(), 1) }.is_null());
    // The plugin, loaded and started as the game does.
    let path = CString::new(plugin).unwrap();
    let lib = unsafe { dlopen(path.as_ptr(), 2) };
    assert!(!lib.is_null(), "{}", unsafe { CStr::from_ptr(dlerror()) }.to_string_lossy());
    type Init = unsafe extern "C" fn(u32, *const InitParams) -> i32;
    type Shutdown = unsafe extern "C" fn();
    let init: Init = unsafe { std::mem::transmute(sym(lib, "scs_telemetry_init")) };
    let shutdown: Shutdown = unsafe { std::mem::transmute(sym(lib, "scs_telemetry_shutdown")) };
    let params = InitParams {
        game_name: c"Euro Truck Simulator 2 (harness)".as_ptr(),
        game_id: c"eut2".as_ptr(),
        game_version: 1 << 16 | 18,
        log: sdk_log,
        register_for_event: register_ok as *const c_void,
        unregister_from_event: register_ok as *const c_void,
        register_for_channel: register_ok as *const c_void,
        unregister_from_channel: register_ok as *const c_void,
    };
    assert_eq!(unsafe { init(1 << 16 | 1, &params) }, 0, "init");

    // The end of a frame, as the game ends it (glFlushRenderAPPLE, then glSwapAPPLE): the plugin's frame clock.
    unsafe {
        type Swap = unsafe extern "C" fn();
        let flush_render: Swap = std::mem::transmute(sym(gl, "glFlushRenderAPPLE"));
        let swap: Swap = std::mem::transmute(sym(gl, "glSwapAPPLE"));
        flush_render();
        swap();
    }
    println!(
        "after a frame: the plugin's heartbeat {}, its pid ours: {}",
        b.plugin.heartbeat.load(Ordering::Acquire),
        b.plugin.pid.load(Ordering::Acquire) == std::process::id()
    );

    // The DAF 2021's screen texture, as the game makes it from the mod (sRGB, one level, the mod's own picture).
    let layout = screen_layouts().iter().position(|layout| layout.id == "daf-2021").unwrap();
    let height = ScreenLayout::texture_height(layout);
    let mut texture = 0;
    let mut other = 0;
    let read_back = |name: u32| -> Vec<u8> {
        let mut out = vec![0u8; (SCREEN_WIDTH * height * 4) as usize];
        unsafe {
            bind(GL_TEXTURE_2D, name);
            pixel_store(GL_PACK_ALIGNMENT, 4);
            get_image(GL_TEXTURE_2D, 0, GL_BGRA, GL_UNSIGNED_INT_8_8_8_8_REV, out.as_mut_ptr() as *mut c_void);
        }
        out
    };
    unsafe {
        gen_textures(1, &mut texture);
        bind(GL_TEXTURE_2D, texture);
        storage(GL_TEXTURE_2D, 1, GL_SRGB8_ALPHA8, SCREEN_WIDTH as i32, height as i32);
        let grey = vec![0x80u8; (SCREEN_WIDTH * height * 4) as usize];
        pixel_store(GL_UNPACK_ALIGNMENT, 1); // The game's own unpacking: must be as it was after.
        sub_image(GL_TEXTURE_2D, 0, 0, 0, SCREEN_WIDTH as i32, height as i32, GL_BGRA, GL_UNSIGNED_INT_8_8_8_8_REV, grey.as_ptr() as *const c_void);
        // Another texture of the screen's width, but not one (two levels).
        gen_textures(1, &mut other);
        bind(GL_TEXTURE_2D, other);
        storage(GL_TEXTURE_2D, 2, GL_SRGB8_ALPHA8, SCREEN_WIDTH as i32, height as i32);
        bind(GL_TEXTURE_2D, 0);
    }

    // Frames from the app, the texture bound to draw with: each new frame written in, its last row repeated below.
    std::thread::sleep(Duration::from_millis(50)); // The heartbeat seen to change.
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    for seq in 1..=60u64 {
        writer.write(seq, SCREEN_WIDTH, SCREEN_HEIGHT, |pixels| pattern(seq, pixels));
        let start = Instant::now();
        unsafe {
            bind(GL_TEXTURE_2D, texture);
            finish();
        }
        worst = worst.max(start.elapsed());
        total += start.elapsed();
        std::thread::sleep(Duration::from_millis(5));
    }
    let pixels = read_back(texture);
    let mut expected = vec![0u8; (SCREEN_WIDTH * SCREEN_HEIGHT * 4) as usize];
    pattern(60, &mut expected);
    let row = (SCREEN_WIDTH * 4) as usize;
    let frame_ok = pixels[..expected.len()] == expected[..];
    let rows_ok = (SCREEN_HEIGHT..height).all(|y| {
        pixels[y as usize * row..][..row] == expected[(SCREEN_HEIGHT as usize - 1) * row..][..row]
    });
    println!("frame written: {frame_ok}; last row repeated below: {rows_ok}; bind with a new frame (and glFinish): {:?} on average, {worst:?} at most", total / 60);
    // The same, the frame unchanged: the bind alone.
    let start = Instant::now();
    for _ in 0..60 {
        unsafe {
            bind(GL_TEXTURE_2D, texture);
            finish();
        }
    }
    println!("bind with no new frame (and glFinish): {:?} on average", start.elapsed() / 60);
    let unpack = unsafe {
        let mut value = 0;
        get_integer(GL_UNPACK_ALIGNMENT, &mut value);
        value
    };
    println!("the game's unpack alignment after: {unpack} (1 expected)");

    // The screen switched off: transparent.
    b.host.screen_enabled.store(0, Ordering::Release);
    unsafe { bind(GL_TEXTURE_2D, texture) };
    let pixels = read_back(texture);
    println!("switched off, transparent: {}", pixels.iter().all(|&p| p == 0));
    // The other texture never touched.
    b.host.screen_enabled.store(1, Ordering::Release);
    let pixels = read_back(other);
    println!("the two-level texture left alone: {}", pixels.iter().all(|&p| p == 0));

    // A screen texture let go, its name made a small texture: left alone.
    unsafe {
        delete(1, &texture);
        let mut again = 0;
        gen_textures(1, &mut again);
        bind(GL_TEXTURE_2D, again);
        storage(GL_TEXTURE_2D, 1, GL_SRGB8_ALPHA8, 64, 64);
        writer.write(61, SCREEN_WIDTH, SCREEN_HEIGHT, |pixels| pattern(61, pixels));
        bind(GL_TEXTURE_2D, 0);
        bind(GL_TEXTURE_2D, again);
        println!("name {texture} let go, {again} made 64 x 64: no error");
    }

    unsafe { shutdown() };
    // The table as it was: binding no longer looked at.
    let bind_entry = unsafe { *(context as *const usize).add(1 + 5) };
    println!("the context's bind_texture entry as it was after shutdown: {}", bind_entry == bind_before);
    STOP.store(true, Ordering::Relaxed);
}
