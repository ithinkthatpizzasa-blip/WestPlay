//! WKWebView rendered offscreen (docs/MACOS_PORT.md, Phase 2): every WebView of the host in one window that is
//! on a display but never seen (transparent, click-through, out of screenshots), each drawn whole and captured by
//! `takeSnapshot` (`capture.rs`). The same [`Stage`] and [`OffscreenWebView`] as WebView2's (`windows.rs`).
//!
//! WebKit is used on the main thread only, and the host runs on a thread of its own (the companion's main thread
//! is its window's): each call is made on the main thread and waited for ([`dispatch2::run_on_main`]); what WebKit
//! hands back (the page's messages, a process gone, a script's result) is queued for the host's thread, which
//! takes it as it pumps ([`pump`]), as WebView2's events come with its window messages.
//!
//! The pages talk to the host as WebView2 has them do (`window.chrome.webview`: `postMessage`, its `message`
//! event): [`BRIDGE_SCRIPT`] gives them that over WebKit's message handlers. WestPlay's own folders, served at
//! `https://<name>/` by WebView2, are served at `westplay://<name>/` here (WebKit gives `https:` to no app).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use dispatch2::{MainThreadBound, run_on_main};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask};
use objc2_foundation::{
    NSData, NSDate, NSDictionary, NSError, NSHTTPURLResponse, NSNumber, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSSize, NSString, NSURL, NSURLRequest, NSUUID,
};
use objc2_web_kit::{
    WKAudiovisualMediaTypes, WKNavigation, WKNavigationDelegate, WKScriptMessage, WKScriptMessageHandler,
    WKURLSchemeHandler, WKURLSchemeTask, WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebView,
    WKWebViewConfiguration, WKWebsiteDataStore,
};

use super::{Error, Failure, MouseButtons, MouseKind, PageSize, fitted, host_of};

/// Where the WebViews draw: WebKit draws on the GPU macOS gives it, and isn't told (`graphics.rs`).
pub fn draw_on(_argument: Option<String>) {}

/// WebView2's browsers to wait for before starting it again with other arguments: WebKit has none of WestPlay's
/// own (its processes are the system's to start and end).
pub fn wait_for_browsers(_timeout: std::time::Duration) {}

/// WebView2's browser processes the host started: none of WestPlay's own here (see [`wait_for_browsers`]).
#[allow(dead_code)]
pub fn host_browsers() -> Vec<u32> {
    Vec::new()
}

/// What a WebKit call fails with.
#[derive(Debug)]
pub struct WebKitError(pub String);

impl std::fmt::Display for WebKitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WebKitError {}

pub type Result<T, E = WebKitError> = std::result::Result<T, E>;

/// The scheme WestPlay's folders are served at (`westplay://westplay.cabin/…`): WebView2's `https://westplay.cabin/`.
pub const SCHEME: &str = "westplay";

/// The pages' way to the host, as WebView2 gives it (`window.chrome.webview`): `postMessage` (to the host, as
/// JSON) and the `message` event (from it, `post_json`). Under its own name on every page
/// (`window.__westplayWebview`), and as `window.chrome.webview` on WestPlay's own pages only (its scheme, the test
/// page, the UI's development server on this Mac): a `window.chrome` on a service's page would tell it it's in Chrome
/// (and its DRM, Widevine, which WebKit hasn't). WestPlay's scripts in the services' pages take either.
pub const BRIDGE_SCRIPT: &str = r#"(() => {
  const handler = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.westplay;
  if (!handler || window.__westplayWebview) return;
  const target = new EventTarget();
  const webview = {
    postMessage: (message) => handler.postMessage(JSON.stringify(message === undefined ? null : message)),
    addEventListener: (type, listener, options) => target.addEventListener(type, listener, options),
    removeEventListener: (type, listener, options) => target.removeEventListener(type, listener, options),
  };
  Object.defineProperty(window, '__westplayWebview', { value: webview });
  Object.defineProperty(window, '__westplayReceive', {
    value: (data) => target.dispatchEvent(new MessageEvent('message', { data })),
  });
  const own = location.protocol === 'westplay:' || location.protocol === 'about:' ||
    location.hostname === 'localhost' || location.hostname === '127.0.0.1';
  if (own) {
    window.chrome = window.chrome || {};
    window.chrome.webview = webview;
  }
})();"#;

/// What the main thread has for one WebView, until its host takes it.
#[derive(Default)]
pub(crate) struct Inbox {
    /// JSON messages the page posted, oldest first.
    pub(crate) messages: Vec<String>,
    /// The worst failure since last taken.
    failure: Option<Failure>,
    /// The name its page's loads are logged under (`log_as`), if they are.
    logged_as: Option<&'static str>,
    /// The address of the last page that finished loading.
    pub(crate) loaded: String,
}

pub(crate) type SharedInbox = Arc<Mutex<Inbox>>;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Scripts' results come back on the main thread, for callbacks that live on the host's: each result waits here
/// until [`pump`] runs its callback.
static RESULTS: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
static NEXT_SCRIPT: AtomicU64 = AtomicU64::new(1);

/// A script's callback, waiting for its result.
type Callback = Box<dyn FnOnce(String)>;

thread_local! {
    /// The host thread's callbacks waiting for their scripts' results.
    static PENDING: RefCell<HashMap<u64, Callback>> = RefCell::new(HashMap::new());
}

/// Runs, on the calling thread (the host's), the callbacks of the scripts whose results came back: as WebView2's
/// come while the host pumps its window messages.
pub fn pump() {
    let done = std::mem::take(&mut *lock(&RESULTS));
    for (id, json) in done {
        if let Some(callback) = PENDING.with(|pending| pending.borrow_mut().remove(&id)) {
            callback(json);
        }
    }
}

/// What a folder served at `westplay://<name>/` is: where it is, and whether any page may fetch from it (CORS).
struct Served {
    folder: PathBuf,
    any_page: bool,
}

type SharedFolders = Arc<Mutex<HashMap<String, Served>>>;

define_class!(
    // SAFETY: NSObject has no subclassing requirements; `Messages` has no `Drop`.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "WestPlayPageMessages"]
    #[ivars = SharedInbox]
    pub(crate) struct Messages;

    // SAFETY: `NSObjectProtocol` has no requirements.
    unsafe impl NSObjectProtocol for Messages {}

    // SAFETY: the signature is the protocol's.
    unsafe impl WKScriptMessageHandler for Messages {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            // SAFETY: the message is WebKit's, alive through the call.
            let body = unsafe { message.body() };
            if let Ok(text) = body.downcast::<NSString>() {
                lock(self.ivars()).messages.push(text.to_string());
            }
        }
    }
);

impl Messages {
    pub(crate) fn new(inbox: SharedInbox, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(inbox);
        // SAFETY: NSObject's `init`, on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; `Navigation` has no `Drop`.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "WestPlayPageNavigation"]
    #[ivars = SharedInbox]
    pub(crate) struct Navigation;

    // SAFETY: `NSObjectProtocol` has no requirements.
    unsafe impl NSObjectProtocol for Navigation {}

    // SAFETY: each signature is the protocol's.
    unsafe impl WKNavigationDelegate for Navigation {
        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, web_view: &WKWebView, _navigation: Option<&WKNavigation>) {
            let mut inbox = lock(self.ivars());
            // SAFETY: the WebView is WebKit's, alive through the call.
            let url = unsafe { web_view.URL() };
            inbox.loaded = url.and_then(|url| url.absoluteString()).map(|text| text.to_string()).unwrap_or_default();
            if let Some(name) = inbox.logged_as {
                log!("[{name}] page loaded ({})", page_host(web_view));
            }
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, web_view: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.failed_to_load(web_view, error);
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(&self, web_view: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.failed_to_load(web_view, error);
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn process_terminated(&self, _web_view: &WKWebView) {
            let mut inbox = lock(self.ivars());
            // The page's process is gone: the page is blank until reloaded. (WebKit's other processes are the
            // system's to restart; the app is the browser.)
            inbox.failure = inbox.failure.max(Some(Failure::Page));
            if let Some(name) = inbox.logged_as {
                log!("[{name}] browser process failed: the page's process exited");
            }
        }
    }
);

impl Navigation {
    pub(crate) fn new(inbox: SharedInbox, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(inbox);
        // SAFETY: NSObject's `init`, on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }

    fn failed_to_load(&self, web_view: &WKWebView, error: &NSError) {
        if let Some(name) = lock(self.ivars()).logged_as {
            log!("[{name}] page didn't load ({}): {}", page_host(web_view), error.localizedDescription());
        }
    }
}

/// The host of the page a WebView shows, for the log.
fn page_host(web_view: &WKWebView) -> String {
    // SAFETY: the WebView is WebKit's, alive through the call.
    let url = unsafe { web_view.URL() };
    let address = url.and_then(|url| url.absoluteString()).map(|text| text.to_string()).unwrap_or_default();
    host_of(&address).to_owned()
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; `Files` has no `Drop`.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "WestPlayFolders"]
    #[ivars = SharedFolders]
    struct Files;

    // SAFETY: `NSObjectProtocol` has no requirements.
    unsafe impl NSObjectProtocol for Files {}

    // SAFETY: each signature is the protocol's.
    unsafe impl WKURLSchemeHandler for Files {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start(&self, _web_view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            // Answered at once, whole, before this returns: WebKit can't stop a task that's already done.
            // SAFETY: the task is WebKit's, alive through the call; its request and URL are plain objects.
            unsafe {
                let request = task.request();
                let Some(url) = request.URL() else { return };
                let address = url.absoluteString().map(|text| text.to_string()).unwrap_or_default();
                let (status, mime, body, any_page) = answer(&lock(self.ivars()), &address);
                let mut headers = vec![("Content-Type", mime.to_string()), ("Cache-Control", "no-cache".to_string())];
                if any_page {
                    headers.push(("Access-Control-Allow-Origin", "*".to_string()));
                }
                let keys: Vec<Retained<NSString>> = headers.iter().map(|(key, _)| NSString::from_str(key)).collect();
                let values: Vec<Retained<NSString>> =
                    headers.iter().map(|(_, value)| NSString::from_str(value)).collect();
                let key_refs: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
                let fields = NSDictionary::from_retained_objects(&key_refs, &values);
                let response = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
                    NSHTTPURLResponse::alloc(),
                    &url,
                    status,
                    Some(&NSString::from_str("HTTP/1.1")),
                    Some(&fields),
                );
                let Some(response) = response else { return };
                task.didReceiveResponse(&response);
                task.didReceiveData(&NSData::with_bytes(&body));
                task.didFinish();
            }
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop(&self, _web_view: &WKWebView, _task: &ProtocolObject<dyn WKURLSchemeTask>) {}
    }
);

impl Files {
    fn new(folders: SharedFolders, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(folders);
        // SAFETY: NSObject's `init`, on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

/// The answer to `westplay://<name>/<path>`: its status, type, bytes, and whether any page may have it.
fn answer(folders: &HashMap<String, Served>, address: &str) -> (isize, &'static str, Vec<u8>, bool) {
    let not_found = (404, "text/plain", b"not found".to_vec(), false);
    let Some(rest) = address.strip_prefix(&format!("{SCHEME}://")) else { return not_found };
    let (name, path) = rest.split_once('/').unwrap_or((rest, ""));
    let Some(served) = folders.get(&name.to_ascii_lowercase()) else { return not_found };
    let path = path.split(['?', '#']).next().unwrap_or("");
    let Some(file) = file_in(&served.folder, &percent_decoded(path)) else {
        return (404, "text/plain", b"not found".to_vec(), served.any_page);
    };
    match std::fs::read(&file) {
        Ok(bytes) => (200, mime_of(&file), bytes, served.any_page),
        Err(_) => (404, "text/plain", b"not found".to_vec(), served.any_page),
    }
}

/// The file at `path` (as the address has it, `/` between its parts) in `folder`, never outside it.
fn file_in(folder: &Path, path: &str) -> Option<PathBuf> {
    let mut file = folder.to_path_buf();
    for part in path.split('/').filter(|part| !part.is_empty()) {
        if part == ".." || part == "." || part.contains('\\') {
            return None;
        }
        file.push(part);
    }
    if file.is_dir() {
        file.push("index.html");
    }
    file.is_file().then_some(file)
}

/// `%xx` escapes in a path decoded (bytes taken as UTF-8).
fn percent_decoded(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(byte) = path.get(at + 1..at + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A served file's type, by its extension.
fn mime_of(file: &Path) -> &'static str {
    match file.extension().and_then(|extension| extension.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("pbf" | "mvt") => "application/x-protobuf",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// The user agent the WebViews give: Safari's, of the Safari on this Mac. The services know Safari (Spotify's web
/// player plays in it); a bare WKWebView's says no browser at all.
pub(crate) fn user_agent() -> &'static str {
    static AGENT: OnceLock<String> = OnceLock::new();
    AGENT.get_or_init(|| {
        let version = std::process::Command::new("/usr/bin/defaults")
            .args(["read", "/Applications/Safari.app/Contents/Info", "CFBundleShortVersionString"])
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| "18.0".to_owned());
        format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{version} \
             Safari/605.1.15"
        )
    })
}

/// The WebKit data store (cookies, storage) of a profile folder: one of WebKit's own, named by the folder (the same
/// folder, the same store, each run).
fn profile_id(user_data: &Path) -> [u8; 16] {
    let path = user_data.to_string_lossy();
    let high = westplay_shared::cityhash::city_hash64(format!("westplay.profile.a:{path}").as_bytes());
    let low = westplay_shared::cityhash::city_hash64(format!("westplay.profile.b:{path}").as_bytes());
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&high.to_be_bytes());
    id[8..].copy_from_slice(&low.to_be_bytes());
    // A version 4 UUID's marks, as WebKit's identifiers are UUIDs.
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    id
}

fn uuid(id: [u8; 16]) -> Retained<NSUUID> {
    let text = format!(
        "{:08X}-{:04X}-{:04X}-{:04X}-{:012X}",
        u32::from_be_bytes(id[0..4].try_into().unwrap_or_default()),
        u16::from_be_bytes(id[4..6].try_into().unwrap_or_default()),
        u16::from_be_bytes(id[6..8].try_into().unwrap_or_default()),
        u16::from_be_bytes(id[8..10].try_into().unwrap_or_default()),
        u64::from_be_bytes([0, 0, id[10], id[11], id[12], id[13], id[14], id[15]]),
    );
    NSUUID::initWithUUIDString(NSUUID::alloc(), &NSString::from_str(&text)).unwrap_or_default()
}

/// Keeps a WebView drawing in a window that's out of sight (WebKit's private switch: docs/MACOS_PORT.md, Phase 2).
pub(crate) fn keep_drawing(web_view: &WKWebView) {
    let selector = objc2::sel!(_setWindowOcclusionDetectionEnabled:);
    if web_view.respondsToSelector(selector) {
        // SAFETY: WebKit's own method, taking a BOOL, on its WebView.
        let () = unsafe { msg_send![web_view, _setWindowOcclusionDetectionEnabled: false] };
    } else {
        log!("WebKit has no _setWindowOcclusionDetectionEnabled: the cab may stop drawing out of sight");
    }
}

/// One WebView on the stage, as the capture composites it: where it is on the page, at what size and scale.
pub(crate) struct Layer {
    pub id: u64,
    pub view: Arc<MainThreadBound<Retained<WKWebView>>>,
    pub x: f32,
    pub y: f32,
    /// Its page's size (it's laid out at it), and how much it's shrunk on the stage (`fit`).
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub shown: bool,
}

/// What the stage and its WebViews share with the capture.
pub(crate) struct StageShared {
    pub window: MainThreadBound<Retained<NSWindow>>,
    pub root: MainThreadBound<Retained<NSView>>,
    /// The frame's size, and the page's (taller where it's squeezed into the frame: `set_page_height`).
    pub frame: (u32, u32),
    pub page: Mutex<(u32, u32)>,
    /// What's on the frame, bottom first.
    pub layers: Mutex<Vec<Layer>>,
}

impl StageShared {
    /// The window's height, which holds every WebView whole (WebKit draws what's within its window).
    const WINDOW_HEIGHT: f64 = 1200.0;
    const WINDOW_WIDTH: f64 = 1600.0;
}

/// The stage window's opacity: as little as WebKit plays sound in (see `Stage::new`).
const STAGE_OPACITY: f64 = 0.001;

/// The frame the host's WebViews render onto, `width` x `height`.
pub struct Stage {
    pub(crate) shared: Arc<StageShared>,
    page_size: PageSize,
}

impl Stage {
    pub fn new(width: u32, height: u32) -> Result<Self, Error> {
        let (window, root) = run_on_main(|mtm| {
            let frame =
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(StageShared::WINDOW_WIDTH, StageShared::WINDOW_HEIGHT));
            // SAFETY: a borderless window made on the main thread, kept (not released on close); plain values.
            let window = unsafe {
                let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    frame,
                    NSWindowStyleMask::Borderless,
                    NSBackingStoreType::Buffered,
                    false,
                );
                window.setReleasedWhenClosed(false);
                // On a display, for WebKit to take the wheel there (it does nothing with a scroll beyond every
                // display), but never seen: all but transparent, the player's clicks through it, out of screenshots
                // and screen sharing, on every Space, out of window cycling, at the desktop's own level. Not quite
                // transparent: WebKit pauses a video with sound in a window of no opacity at all (it played at
                // 0.001, which rounds to nothing on screen: tools/macos_probe/wkwebview/playback.swift).
                window.setOpaque(false);
                window.setAlphaValue(STAGE_OPACITY);
                window.setIgnoresMouseEvents(true);
                window.setSharingType(objc2_app_kit::NSWindowSharingType::None);
                window.setCollectionBehavior(
                    NSWindowCollectionBehavior::CanJoinAllSpaces
                        | NSWindowCollectionBehavior::Stationary
                        | NSWindowCollectionBehavior::IgnoresCycle
                        | NSWindowCollectionBehavior::Transient,
                );
                window.setLevel(objc2_core_graphics::CGWindowLevelForKey(
                    objc2_core_graphics::CGWindowLevelKey::DesktopWindowLevelKey,
                ) as isize);
                window.setFrameOrigin(NSPoint::new(0.0, 0.0));
                window.orderBack(None);
                window
            };
            let root = NSView::initWithFrame(NSView::alloc(mtm), frame);
            window.setContentView(Some(&root));
            (MainThreadBound::new(window, mtm), MainThreadBound::new(root, mtm))
        });
        let shared = Arc::new(StageShared {
            window,
            root,
            frame: (width, height),
            page: Mutex::new((width, height)),
            layers: Mutex::new(Vec::new()),
        });
        Ok(Self { shared, page_size: Rc::new(Cell::new((width, height))) })
    }

    /// Puts a WebView on the frame: over the rest, or under it.
    pub fn add(&self, view: &OffscreenWebView, on_top: bool) -> Result<()> {
        let mut layers = lock(&self.shared.layers);
        layers.retain(|layer| layer.id != view.id);
        let (width, height) = view.size.get();
        let layer =
            Layer { id: view.id, view: view.view.clone(), x: 0.0, y: 0.0, width, height, scale: 1.0, shown: true };
        if on_top {
            layers.push(layer)
        } else {
            layers.insert(0, layer)
        }
        Ok(())
    }

    /// Takes a WebView off the frame (before it's dropped).
    pub fn remove(&self, view: &OffscreenWebView) -> Result<()> {
        lock(&self.shared.layers).retain(|layer| layer.id != view.id);
        Ok(())
    }

    /// The page's size as it changes, for what keeps within it.
    pub fn page_size(&self) -> PageSize {
        self.page_size.clone()
    }

    /// Lays what's on the frame out `height` tall (the frame's width), squeezed into the frame's height when
    /// it's taller (the capture squeezes it). The WebViews on it are sized by their owners.
    pub fn set_page_height(&self, height: u32) -> Result<()> {
        let width = self.shared.frame.0;
        *lock(&self.shared.page) = (width, height);
        self.page_size.set((width, height));
        Ok(())
    }
}

/// A WebView's profile (its WebKit data store), for another WebView on it: a sign-in window (signin.rs), as
/// WebView2's environment is.
#[derive(Clone)]
pub struct Environment {
    pub(crate) store: Arc<MainThreadBound<Retained<WKWebsiteDataStore>>>,
}

/// A WebView's delegates and handlers, kept alive with it (WebKit holds its delegates weakly).
type Delegates = (Retained<Navigation>, Retained<Messages>, Retained<Files>);

/// The number each WebView is known by on the stage.
static NEXT_VIEW: AtomicU64 = AtomicU64::new(1);

pub struct OffscreenWebView {
    id: u64,
    view: Arc<MainThreadBound<Retained<WKWebView>>>,
    stage: Arc<StageShared>,
    /// Its page's size, as last placed.
    size: Cell<(u32, u32)>,
    inbox: SharedInbox,
    folders: SharedFolders,
    /// Its delegates and handlers, kept alive with it (WebKit holds its delegates weakly).
    _delegates: MainThreadBound<Delegates>,
    environment: Environment,
    pub version: String,
}

impl OffscreenWebView {
    /// A WebView of `width` x `height` for `stage` (not on the frame until [`Stage::add`]), on the profile
    /// `user_data` names.
    pub fn new(stage: &Stage, width: u32, height: u32, user_data: &Path) -> Result<Self, Error> {
        let inbox = SharedInbox::default();
        let folders = SharedFolders::default();
        let id = NEXT_VIEW.fetch_add(1, Ordering::Relaxed);
        let profile = profile_id(user_data);
        let shared = stage.shared.clone();
        let (view, delegates, store) = run_on_main(|mtm| {
            let store = data_store(profile, mtm);
            let (view, delegates) = make_view(mtm, &store, inbox.clone(), folders.clone(), width, height);
            shared.root.get(mtm).addSubview(&view);
            place_view(&view, width, height);
            (
                Arc::new(MainThreadBound::new(view, mtm)),
                MainThreadBound::new(delegates, mtm),
                Arc::new(MainThreadBound::new(store, mtm)),
            )
        });
        Ok(Self {
            id,
            view,
            stage: stage.shared.clone(),
            size: Cell::new((width, height)),
            inbox,
            folders,
            _delegates: delegates,
            environment: Environment { store },
            version: format!("WebKit (Safari {})", safari_version()),
        })
    }

    /// Its profile, for another WebView on it (a window: signin.rs).
    pub fn environment(&self) -> &Environment {
        &self.environment
    }

    /// The worst WebKit process failure since the last call, if any.
    pub fn take_failure(&self) -> Option<Failure> {
        lock(&self.inbox).failure.take()
    }

    /// Logs what happens to this WebView's page as `name`'s (`[spotify] …`): each page load, done or failed
    /// with WebKit's reason, and its process failing. Of the page's address, only its host.
    pub fn log_as(&self, name: &'static str) -> Result<()> {
        lock(&self.inbox).logged_as = Some(name);
        Ok(())
    }

    /// Loads the page again (after its process failed).
    pub fn reload(&self) -> Result<()> {
        self.on_view(|view| {
            // SAFETY: the WebView, on the main thread.
            unsafe { view.reload() };
        });
        Ok(())
    }

    /// Where on the stage the WebView shows, and its size: the page lays itself out at that size.
    pub fn place(&self, x: f32, y: f32, width: u32, height: u32) -> Result<()> {
        self.set_layer(x, y, width, height, 1.0)
    }

    /// The page laid out large, in the shape of the rectangle `x, y, width, height` of the stage and as big
    /// as fits in `room` (width, height), its picture shrunk evenly into the rectangle: a small view of a
    /// page that stays large (a video there keeps its quality, and its shape: it's contained in the page).
    pub fn fit(&self, room: (u32, u32), x: f32, y: f32, width: u32, height: u32) -> Result<()> {
        let (page_width, page_height, scale) = fitted(room, width, height);
        self.set_layer(x, y, page_width, page_height, scale)
    }

    fn set_layer(&self, x: f32, y: f32, width: u32, height: u32, scale: f32) -> Result<()> {
        if self.size.replace((width, height)) != (width, height) {
            self.on_view(move |view| place_view(view, width, height));
        }
        if let Some(layer) = lock(&self.stage.layers).iter_mut().find(|layer| layer.id == self.id) {
            (layer.x, layer.y, layer.width, layer.height, layer.scale) = (x, y, width, height, scale);
        }
        Ok(())
    }

    /// Forgets every cookie of the WebView's profile (signing out of whatever it was signed in to), coming back
    /// once they're gone: a page loaded after this is signed out. Cookies only.
    pub fn delete_cookies(&self) -> Result<()> {
        let (done, wait) = std::sync::mpsc::channel();
        let store = self.environment.store.clone();
        run_on_main(move |mtm| {
            let store = store.get(mtm);
            // SAFETY: the profile's store, on the main thread; the block is kept by WebKit until it calls it.
            unsafe {
                let types = objc2_foundation::NSSet::from_slice(&[objc2_web_kit::WKWebsiteDataTypeCookies]);
                let block = block2::RcBlock::new(move || {
                    let _ = done.send(());
                });
                store.removeDataOfTypes_modifiedSince_completionHandler(&types, &NSDate::distantPast(), &block);
            }
        });
        wait.recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| WebKitError("WebKit didn't say the cookies were gone".into()))
    }

    /// Sets the cookie `name` (`value`) on `domain` for a year, secure, as a site's own page would: unless the
    /// profile has it for `url` already with a value `kept` keeps (a choice made there before). Comes back once
    /// it's set, for a page loaded after it to have it.
    pub fn set_cookie_unless(
        &self,
        url: &str,
        domain: &str,
        name: &str,
        value: &str,
        kept: impl Fn(&str) -> bool,
    ) -> Result<(), Error> {
        let host = host_of(url).to_ascii_lowercase();
        let wanted = name.to_owned();
        let (found, wait) = std::sync::mpsc::channel::<Option<String>>();
        let store = self.environment.store.clone();
        run_on_main(move |mtm| {
            // SAFETY: the profile's cookie store, on the main thread; the block is kept by WebKit until it calls it.
            unsafe {
                let cookies = store.get(mtm).httpCookieStore();
                let block = block2::RcBlock::new(
                    move |list: std::ptr::NonNull<objc2_foundation::NSArray<objc2_foundation::NSHTTPCookie>>| {
                        let list = list.as_ref();
                        let value = list.iter().find_map(|cookie| {
                            let domain = cookie.domain().to_string().trim_start_matches('.').to_ascii_lowercase();
                            let applies = host == domain || host.ends_with(&format!(".{domain}"));
                            (applies && cookie.name().to_string() == wanted).then(|| cookie.value().to_string())
                        });
                        let _ = found.send(value);
                    },
                );
                cookies.getAllCookies(&block);
            }
        });
        let current = wait
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| WebKitError("WebKit didn't give the cookies".into()))?;
        if current.as_deref().is_some_and(kept) {
            return Ok(());
        }
        let (domain, name, value) = (domain.to_owned(), name.to_owned(), value.to_owned());
        let cookie_name = name.clone();
        let (set, wait) = std::sync::mpsc::channel();
        let store = self.environment.store.clone();
        let made = run_on_main(move |mtm| {
            use objc2_foundation::NSHTTPCookie;
            let year = NSDate::dateWithTimeIntervalSinceNow(365.0 * 86_400.0);
            // SAFETY: the cookie's properties are Foundation's own keys with strings, a date and a flag; the store
            // is the profile's, on the main thread; the block is kept by WebKit until it calls it.
            unsafe {
                use objc2_foundation::{
                    NSHTTPCookieDomain, NSHTTPCookieExpires, NSHTTPCookieName, NSHTTPCookiePath, NSHTTPCookieSecure,
                    NSHTTPCookieValue,
                };
                let keys: [&NSString; 6] = [
                    NSHTTPCookieName,
                    NSHTTPCookieValue,
                    NSHTTPCookieDomain,
                    NSHTTPCookiePath,
                    NSHTTPCookieSecure,
                    NSHTTPCookieExpires,
                ];
                let values: [Retained<AnyObject>; 6] = [
                    Retained::into_super(Retained::into_super(NSString::from_str(&name))),
                    Retained::into_super(Retained::into_super(NSString::from_str(&value))),
                    Retained::into_super(Retained::into_super(NSString::from_str(&domain))),
                    Retained::into_super(Retained::into_super(NSString::from_str("/"))),
                    Retained::into_super(Retained::into_super(NSString::from_str("TRUE"))),
                    Retained::into_super(Retained::into_super(year)),
                ];
                let properties = NSDictionary::from_retained_objects(&keys, &values);
                let Some(cookie) = NSHTTPCookie::cookieWithProperties(&properties) else { return false };
                let block = block2::RcBlock::new(move || {
                    let _ = set.send(());
                });
                store.get(mtm).httpCookieStore().setCookie_completionHandler(&cookie, Some(&block));
                true
            }
        });
        if !made {
            return Err(Box::new(WebKitError(format!("cookie {cookie_name}: not one WebKit takes"))));
        }
        wait.recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| Box::new(WebKitError("WebKit didn't say the cookie was set".into())) as Error)
    }

    /// Whether the WebView shows on the stage (hidden, its page runs on: a video still plays its sound).
    pub fn show(&self, shown: bool) -> Result<()> {
        if let Some(layer) = lock(&self.stage.layers).iter_mut().find(|layer| layer.id == self.id) {
            layer.shown = shown;
        }
        Ok(())
    }

    /// Runs `script` in every page the WebView loads, before the page's own scripts (in its main frame).
    pub fn add_startup_script(&self, script: &str) -> Result<(), Error> {
        let script = script.to_owned();
        self.on_view(move |view| {
            // SAFETY: the WebView's own content controller, on the main thread; the script is a plain string.
            unsafe {
                let mtm = MainThreadMarker::from(view);
                let user = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(mtm),
                    &NSString::from_str(&script),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    true,
                );
                view.configuration().userContentController().addUserScript(&user);
            }
        });
        Ok(())
    }

    /// Runs `script` in the page, without waiting: `done` gets its result as JSON once WebKit has it (nothing
    /// if the script couldn't run), on this thread as it pumps ([`pump`]).
    pub fn run_script(&self, script: &str, done: impl FnOnce(String) + 'static) -> Result<()> {
        let id = NEXT_SCRIPT.fetch_add(1, Ordering::Relaxed);
        PENDING.with(|pending| pending.borrow_mut().insert(id, Box::new(done)));
        // As WebView2's `ExecuteScript`: the script's value as JSON (WebKit gives the value, not its JSON).
        let wrapped = format!("JSON.stringify((() => {{ return eval({}); }})()) ?? 'null'", js_string(script));
        self.on_view(move |view| {
            let block = block2::RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
                if !error.is_null() {
                    return;
                }
                // SAFETY: WebKit's result, alive through the call: a string here (the JSON).
                let json = unsafe { result.as_ref() }
                    .and_then(|result| result.downcast_ref::<NSString>())
                    .map(|text| text.to_string())
                    .unwrap_or_else(|| "null".to_owned());
                lock(&RESULTS).push((id, json));
            });
            // SAFETY: the WebView, on the main thread; the block is kept by WebKit until it calls it.
            unsafe { view.evaluateJavaScript_completionHandler(&NSString::from_str(&wrapped), Some(&block)) };
        });
        Ok(())
    }

    /// Tests: runs `script` in the page, and its result as JSON (to measure what a page lays out). For a test with
    /// the main thread's run loop going (not the test harness's: it holds the main thread).
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn evaluate(&self, script: &str) -> Result<String, Error> {
        let result: Rc<RefCell<Option<String>>> = Rc::default();
        let written = result.clone();
        self.run_script(script, move |json| *written.borrow_mut() = Some(json))?;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while result.borrow().is_none() && std::time::Instant::now() < until {
            crate::pump_messages();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        result.take().ok_or_else(|| Box::new(WebKitError("the script gave nothing".into())) as Error)
    }

    /// Serves `folder` at `westplay://{host_name}/`, to the page served from it.
    pub fn map_folder(&self, host_name: &str, folder: &Path) -> Result<()> {
        self.serve(host_name, folder, false)
    }

    /// Serves `folder` at `westplay://{host_name}/` to any page of the WebView's, fetched across origins too
    /// (the map's tiles, fetched by MapLibre's worker from the cabin page).
    pub fn map_folder_to_any_page(&self, host_name: &str, folder: &Path) -> Result<()> {
        self.serve(host_name, folder, true)
    }

    fn serve(&self, host_name: &str, folder: &Path, any_page: bool) -> Result<()> {
        lock(&self.folders).insert(host_name.to_ascii_lowercase(), Served { folder: folder.to_path_buf(), any_page });
        Ok(())
    }

    /// Loads `url`; one of WestPlay's own folders' (`https://westplay.cabin/…`, as the host names them for
    /// WebView2) from where they're served here (`westplay://westplay.cabin/…`).
    pub fn navigate(&self, url: &str) -> Result<()> {
        let url = self.served_address(url);
        let address = url.clone();
        let url_ok = self.on_view(move |view| {
            let Some(url) = NSURL::URLWithString(&NSString::from_str(&address)) else { return false };
            // SAFETY: the WebView, on the main thread; the request is a plain object.
            unsafe { view.loadRequest(&NSURLRequest::requestWithURL(&url)) };
            true
        });
        if url_ok { Ok(()) } else { Err(WebKitError(format!("not an address WebKit takes: {}", host_of(&url)))) }
    }

    /// `url` as it's loaded here: an `https:` address of a folder this WebView serves, at WestPlay's scheme.
    fn served_address(&self, url: &str) -> String {
        if let Some(rest) = url.strip_prefix("https://") {
            let host = rest.split(['/', '?', '#']).next().unwrap_or("").to_ascii_lowercase();
            if lock(&self.folders).contains_key(&host) {
                return format!("{SCHEME}://{rest}");
            }
        }
        url.to_owned()
    }

    pub fn navigate_to_string(&self, html: &str) -> Result<()> {
        let html = html.to_owned();
        self.on_view(move |view| {
            // SAFETY: the WebView, on the main thread; the page is a plain string.
            unsafe { view.loadHTMLString_baseURL(&NSString::from_str(&html), None) };
        });
        Ok(())
    }

    /// Messages the page posted since the last call, oldest first.
    pub fn take_page_messages(&self) -> Vec<String> {
        std::mem::take(&mut lock(&self.inbox).messages)
    }

    /// Posts `json` to the page, as WebView2's `PostWebMessageAsJson`: its `message` event's `data`.
    pub fn post_json(&self, json: &str) -> Result<()> {
        let script = format!("window.__westplayReceive && window.__westplayReceive({json});");
        self.on_view(move |view| {
            // SAFETY: the WebView, on the main thread; no result wanted.
            unsafe { view.evaluateJavaScript_completionHandler(&NSString::from_str(&script), None) };
        });
        Ok(())
    }

    /// Mouse input for the page, at `x, y` in page pixels, `buttons` held (`data`: the wheel's delta).
    pub fn send_mouse(&self, kind: MouseKind, buttons: MouseButtons, data: u32, x: i32, y: i32) -> Result<()> {
        self.on_view(move |view| super::macos_input::send(view, kind, buttons, data, x, y));
        Ok(())
    }

    /// Runs `work` with the WebView, on the main thread, and waits for it.
    fn on_view<R: Send>(&self, work: impl FnOnce(&WKWebView) -> R + Send) -> R {
        let view = &self.view;
        run_on_main(move |mtm| work(view.get(mtm)))
    }
}

impl Drop for OffscreenWebView {
    fn drop(&mut self) {
        lock(&self.stage.layers).retain(|layer| layer.id != self.id);
        self.on_view(|view| {
            // SAFETY: the WebView, on the main thread: taken off the stage's window before it goes.
            unsafe {
                view.stopLoading();
                view.removeFromSuperview();
            }
        });
    }
}

/// The data store of the profile `id`, WebKit's persistent one of that name.
fn data_store(id: [u8; 16], mtm: MainThreadMarker) -> Retained<WKWebsiteDataStore> {
    // SAFETY: WebKit's call, on the main thread, with a UUID.
    unsafe { WKWebsiteDataStore::dataStoreForIdentifier(&uuid(id), mtm) }
}

/// A WebView of WestPlay's on `store`: its message handler, its navigation delegate and WestPlay's scheme, the bridge
/// for the page, and none of a browser's own (no zoom, no swiping back).
fn make_view(
    mtm: MainThreadMarker,
    store: &WKWebsiteDataStore,
    inbox: SharedInbox,
    folders: SharedFolders,
    width: u32,
    height: u32,
) -> (Retained<WKWebView>, Delegates) {
    let navigation = Navigation::new(inbox.clone(), mtm);
    let messages = Messages::new(inbox, mtm);
    let files = Files::new(folders, mtm);
    // SAFETY: WebKit's configuration and WebView, made and set up on the main thread; the delegates outlive the
    // WebView (kept with it by its owner).
    unsafe {
        let configuration = WKWebViewConfiguration::new(mtm);
        configuration.setWebsiteDataStore(store);
        configuration.setMediaTypesRequiringUserActionForPlayback(WKAudiovisualMediaTypes::None);
        configuration
            .setURLSchemeHandler_forURLScheme(Some(ProtocolObject::from_ref(&*files)), &NSString::from_str(SCHEME));
        let content = configuration.userContentController();
        content.addScriptMessageHandler_name(ProtocolObject::from_ref(&*messages), &NSString::from_str("westplay"));
        content.addUserScript(&WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
            WKUserScript::alloc(mtm),
            &NSString::from_str(BRIDGE_SCRIPT),
            WKUserScriptInjectionTime::AtDocumentStart,
            true,
        ));
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width as f64, height as f64));
        let view = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &configuration);
        view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*navigation)));
        view.setCustomUserAgent(Some(&NSString::from_str(user_agent())));
        view.setAllowsMagnification(false);
        view.setAllowsBackForwardNavigationGestures(false);
        // Transparent: what the page leaves transparent stays transparent in the frames, and the game's own
        // screen shows through there (the truck's OEM strip around CarPlay).
        let () =
            msg_send![&*view, setValue: &*NSNumber::new_bool(false), forKey: &*NSString::from_str("drawsBackground")];
        keep_drawing(&view);
        (view, (navigation, messages, files))
    }
}

/// Lays a WebView out `width` x `height`, its top left at the window's (the window's points are bottom-left based).
fn place_view(view: &WKWebView, width: u32, height: u32) {
    let top = StageShared::WINDOW_HEIGHT - height as f64;
    view.setFrame(NSRect::new(NSPoint::new(0.0, top), NSSize::new(width as f64, height as f64)));
}

/// The Safari of this Mac's version (the WebKit it has: Safari's own).
fn safari_version() -> String {
    user_agent().split("Version/").nth(1).and_then(|rest| rest.split(' ').next()).unwrap_or("?").to_owned()
}

/// `text` as a JavaScript string literal.
fn js_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
}
