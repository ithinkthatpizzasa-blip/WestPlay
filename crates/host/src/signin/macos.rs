//! The sign-in window on macOS: an ordinary window of WestPlay's, in front, with a WKWebView on the service's
//! profile (the data store of the service's page in WestPlay: `webview::Environment`). It takes the mouse and the
//! keyboard as any window does; WestPlay sees only the address of each page it finishes loading, what a page that
//! says so says (`page_says`), and whether it's still open.

use dispatch2::{MainThreadBound, run_on_main};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly, msg_send};
use objc2_app_kit::{NSApplication, NSBackingStoreType, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest};
use objc2_web_kit::{WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration};

use super::{Progress, Site, progress_at, says_signed_in};
use crate::webview::macos::{BRIDGE_SCRIPT, Messages, Navigation, SharedInbox, lock, user_agent};
use crate::webview::{self, Environment};

/// The window's size, in points.
const SIZE: (f64, f64) = (520.0, 760.0);

pub struct SignInWindow {
    window: MainThreadBound<Retained<NSWindow>>,
    _view: MainThreadBound<(Retained<WKWebView>, Retained<Navigation>, Retained<Messages>)>,
    inbox: SharedInbox,
    site: &'static Site,
    /// Its page said it's signed in (a site that says so: `page_says`).
    said: std::cell::Cell<bool>,
}

impl SignInWindow {
    /// Opens `site`'s sign-in in a window on the desktop, in front, on `environment` (that of the service's page
    /// in WestPlay: its profile, so the sign-in is its).
    pub fn open(environment: &Environment, site: &'static Site) -> Result<Self, webview::Error> {
        let inbox = SharedInbox::default();
        let store = environment.store.clone();
        let made = inbox.clone();
        let (window, view) = run_on_main(move |mtm| {
            let navigation = Navigation::new(made.clone(), mtm);
            let messages = Messages::new(made, mtm);
            // SAFETY: WebKit's configuration, WebView and an AppKit window, made on the main thread; the window is
            // kept (not released on close), the delegates with the WebView.
            unsafe {
                let configuration = WKWebViewConfiguration::new(mtm);
                configuration.setWebsiteDataStore(store.get(mtm));
                let content = configuration.userContentController();
                content.addScriptMessageHandler_name(
                    ProtocolObject::from_ref(&*messages),
                    &NSString::from_str("westplay"),
                );
                for script in [Some(BRIDGE_SCRIPT), site.page_says].into_iter().flatten() {
                    content.addUserScript(&WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                        WKUserScript::alloc(mtm),
                        &NSString::from_str(script),
                        WKUserScriptInjectionTime::AtDocumentEnd,
                        true,
                    ));
                }
                let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(SIZE.0, SIZE.1));
                let view = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &configuration);
                view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*navigation)));
                view.setCustomUserAgent(Some(&NSString::from_str(user_agent())));
                let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    frame,
                    NSWindowStyleMask::Titled
                        | NSWindowStyleMask::Closable
                        | NSWindowStyleMask::Miniaturizable
                        | NSWindowStyleMask::Resizable,
                    NSBackingStoreType::Buffered,
                    false,
                );
                window.setReleasedWhenClosed(false);
                window.setTitle(&NSString::from_str(site.title));
                window.setContentView(Some(&view));
                window.center();
                if let Some(url) = NSURL::URLWithString(&NSString::from_str(site.sign_in)) {
                    view.loadRequest(&NSURLRequest::requestWithURL(&url));
                }
                bring_to_front(&window, mtm);
                (MainThreadBound::new(window, mtm), MainThreadBound::new((view, navigation, messages), mtm))
            }
        });
        Ok(Self { window, _view: view, inbox, site, said: std::cell::Cell::new(false) })
    }

    /// The window in front, its page taking the keyboard (also when it's already open: the sign-in asked for
    /// again).
    pub fn bring_forward(&self) {
        let window = &self.window;
        run_on_main(|mtm| bring_to_front(window.get(mtm), mtm));
    }

    pub fn progress(&self) -> Progress {
        let window = &self.window;
        if !run_on_main(|mtm| window.get(mtm).isVisible()) {
            return Progress::Closed;
        }
        let (messages, loaded) = {
            let mut inbox = lock(&self.inbox);
            (std::mem::take(&mut inbox.messages), inbox.loaded.clone())
        };
        if messages.iter().any(|message| says_signed_in(message)) {
            self.said.set(true);
        }
        if self.said.get() {
            return Progress::SignedIn;
        }
        if self.site.page_says.is_some() {
            return Progress::Signing;
        }
        progress_at(self.site, &loaded)
    }
}

impl Drop for SignInWindow {
    fn drop(&mut self) {
        let window = &self.window;
        run_on_main(|mtm| window.get(mtm).close());
    }
}

/// `window` in front of every other app's, and WestPlay the app in front, for the keyboard to reach it.
fn bring_to_front(window: &NSWindow, mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: AppKit's own calls, on the main thread.
    unsafe {
        let () = msg_send![&*app, activateIgnoringOtherApps: true];
    }
    window.makeKeyAndOrderFront(None);
}
