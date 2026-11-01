// WestPlay Phase 2 probe (playback.swift): does a page's video play in a WKWebView whose window is on a display
// but unseen (as the host's stage is), and if not, why? Plays a YouTube video and asks the page's <video>.
// Build: swiftc -O -o playback playback.swift.
// Usage: [VISIBLE=1 | OFFSCREEN=1] [ALPHA=a] [HOST=crates/host/src] [NO_PLAY=1] [STORE=1] [TRANSPARENT=1] playback <watch url> (unseen: on a display, transparent; OFFSCREEN: beyond
// the displays, opaque)
import AppKit
import WebKit

let url = URL(string: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "https://www.youtube.com/watch?v=aMnol74WRUs")!
let visible = ProcessInfo.processInfo.environment["VISIBLE"] != nil
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let config = WKWebViewConfiguration()
config.mediaTypesRequiringUserActionForPlayback = []
// HOST=<crates/host/src>: YouTube's page as the host has it, WestPlay's bridge and scripts in it, opened on its home
// page, and the video asked for as the host asks (`{"action":"load"}`).
final class Said: NSObject, WKScriptMessageHandler {
    func userContentController(_ c: WKUserContentController, didReceive message: WKScriptMessage) {
        if let text = message.body as? String, text.contains("holds") || text.contains("diagnosis") { print("page says:", text.prefix(400)) }
    }
}
let said = Said()
let hostSource = ProcessInfo.processInfo.environment["HOST"]
if let src = hostSource {
    config.userContentController.add(said, name: "westplay")
    let bridge = """
    (() => { const handler = window.webkit.messageHandlers.westplay; const target = new EventTarget();
      const webview = { postMessage: (m) => handler.postMessage(JSON.stringify(m ?? null)),
        addEventListener: (t, l, o) => target.addEventListener(t, l, o), removeEventListener: (t, l, o) => target.removeEventListener(t, l, o) };
      Object.defineProperty(window, '__westplayWebview', { value: webview });
      Object.defineProperty(window, '__westplayReceive', { value: (data) => target.dispatchEvent(new MessageEvent('message', { data })) }); })();
    """
    let scripts = try! ["cab_sound.js", "youtube.js", "consent.js"].map { try String(contentsOfFile: src + "/" + $0, encoding: .utf8) }
    for source in [bridge, scripts.joined(separator: "\n")] {
        config.userContentController.addUserScript(WKUserScript(source: source, injectionTime: .atDocumentStart, forMainFrameOnly: true))
    }
}
// STORE=1: a persistent data store of its own, new (or STORE=<uuid>: that one), as the host's profiles are; TRANSPARENT=1: no background, as the
// host's WebViews draw.
if ProcessInfo.processInfo.environment["STORE"] != nil {
    config.websiteDataStore = WKWebsiteDataStore(forIdentifier: ProcessInfo.processInfo.environment["STORE"].flatMap { UUID(uuidString: $0) } ?? UUID())
}
let web = WKWebView(frame: NSRect(x: 0, y: 0, width: 1280, height: 720), configuration: config)
if ProcessInfo.processInfo.environment["TRANSPARENT"] != nil {
    web.setValue(false, forKey: "drawsBackground")
}
web.customUserAgent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0 Safari/605.1.15"
let occlusion = NSSelectorFromString("_setWindowOcclusionDetectionEnabled:")
typealias SetBool = @convention(c) (AnyObject, Selector, Bool) -> Void
unsafeBitCast(web.method(for: occlusion), to: SetBool.self)(web, occlusion, false)
let env = ProcessInfo.processInfo.environment
let at: CGFloat = env["OFFSCREEN"] != nil ? -20000 : 100
let window = NSWindow(contentRect: NSRect(x: at, y: at, width: 1280, height: 720), styleMask: [.borderless], backing: .buffered, defer: false)
window.contentView = web
if !visible {
    window.alphaValue = env["ALPHA"].flatMap { Double($0) }.map { CGFloat($0) } ?? (env["OFFSCREEN"] != nil ? 1 : 0)
    window.ignoresMouseEvents = true
    window.sharingType = .none
    window.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.desktopWindow)))
}
window.orderBack(nil)
// SOCS=1: YouTube's consent answered before it loads, as the host answers it (`answer_consent`: `SOCS=CAE=`).
func start() {
if hostSource != nil {
    web.load(URLRequest(url: URL(string: "https://www.youtube.com/")!))
    DispatchQueue.main.asyncAfter(deadline: .now() + 5) {
        web.evaluateJavaScript("window.__westplayReceive({action: 'load', video: '\(url.query?.split(separator: "=").last ?? "")'})")
        print("asked for the video, as the host does")
    }
} else {
    web.load(URLRequest(url: url))
}
}
if ProcessInfo.processInfo.environment["SOCS"] != nil {
    let cookie = HTTPCookie(properties: [.name: "SOCS", .value: "CAE=", .domain: ".youtube.com", .path: "/", .secure: "TRUE",
                                         .expires: Date(timeIntervalSinceNow: 365 * 86400)])!
    web.configuration.websiteDataStore.httpCookieStore.setCookie(cookie) {
        web.configuration.websiteDataStore.httpCookieStore.getAllCookies { all in
            print("cookies before loading:", all.map { "\($0.domain) \($0.name)=\($0.value)" })
            start()
        }
    }
} else {
    start()
}
let ask = (ProcessInfo.processInfo.environment["NO_PLAY"] != nil ? "globalThis.NO_PLAY = true;\n" : "") + """
const v = document.querySelector('video');
if (!v) return 'no video';
const state = () => ({paused: v.paused, t: Math.round(v.currentTime * 10) / 10, ready: v.readyState,
  frames: v.getVideoPlaybackQuality ? v.getVideoPlaybackQuality().totalVideoFrames : -1, muted: v.muted});
const before = state();
let played;
if (!(globalThis.NO_PLAY)) { try { await v.play(); played = 'ok' } catch (e) { played = e.name + ': ' + e.message } }
await new Promise(r => setTimeout(r, 3000));
return JSON.stringify({visibility: document.visibilityState, hasFocus: document.hasFocus(), before, played, after: state()});
"""
for at in [12.0, 16.0] {
    DispatchQueue.main.asyncAfter(deadline: .now() + at) {
        web.callAsyncJavaScript(ask, arguments: [:], in: nil, in: .page) { result in
            switch result {
            case .success(let value): print("\(visible ? "visible" : "unseen") alpha \(window.alphaValue) at \(at) s:", value ?? "nil")
            case .failure(let error): print("error:", error)
            }
            fflush(stdout)
        }
    }
}
// DUMP_CONSENT=1: YouTube's consent dialog in the page, its buttons and forms, at 10 s.
if ProcessInfo.processInfo.environment["DUMP_CONSENT"] != nil {
    DispatchQueue.main.asyncAfter(deadline: .now() + 10) {
        let dump = """
        const dialog = document.querySelector('ytd-consent-bump-v2-lightbox, tp-yt-paper-dialog, [aria-modal="true"]');
        if (!dialog) return 'no dialog';
        const buttons = [...dialog.querySelectorAll('button, a, ytd-button-renderer, form')].map(e => ({tag: e.tagName,
          text: (e.innerText || '').trim().slice(0, 30), label: e.getAttribute('aria-label'), action: e.getAttribute('action'),
          href: e.getAttribute('href'), cls: (e.className || '').toString().slice(0, 60),
          inputs: e.tagName === 'FORM' ? [...e.elements].map(i => i.name + '=' + i.value) : undefined }));
        const data = [...dialog.querySelectorAll('ytd-button-renderer')].map(e => JSON.stringify(e.data ?? e.__data?.data ?? null).slice(0, 700));
        return JSON.stringify({tag: dialog.tagName, buttons, data}, null, 0);
        """
        web.callAsyncJavaScript(dump, arguments: [:], in: nil, in: .page) { r in print("consent:", (try? r.get()) ?? "error") }
    }
}
// SNAPSHOT=<file>: what the page shows, at 17 s.
if let file = ProcessInfo.processInfo.environment["SNAPSHOT"] {
    DispatchQueue.main.asyncAfter(deadline: .now() + 17) {
        web.takeSnapshot(with: nil) { image, _ in
            if let tiff = image?.tiffRepresentation { try? NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: file)) }
        }
    }
}
DispatchQueue.main.asyncAfter(deadline: .now() + 22) { app.terminate(nil) }
app.run()
