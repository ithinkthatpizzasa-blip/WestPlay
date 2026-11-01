// WestPlay Phase 2 prototype (service.swift): a real service in a WKWebView, as WestPlay would host it. The window
// starts on screen, for the player to sign in and press play; on SIGUSR1 it goes beyond the displays (as the
// host's would be) and its frames are captured; SIGUSR2 brings it back. Every 2 s it logs what the page's
// media elements are doing (playing, time, encrypted), and the capture's fresh frames and a snapshot.
// Build: swiftc -O -o service service.swift. Usage: service <url> <profile name>
import AppKit
import WebKit
import CryptoKit

let args = CommandLine.arguments
let url = URL(string: args[1])!
let profile = args.count > 2 ? args[2] : "test"

// A profile of its own (cookies, storage), kept between runs, as WestPlay keeps one per service.
let digest = Array(SHA256.hash(data: Data("westplay.\(profile)".utf8)))
let uuid = UUID(uuid: (digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
                       digest[8], digest[9], digest[10], digest[11], digest[12], digest[13], digest[14], digest[15]))

final class Probe: NSObject, WKScriptMessageHandler {
    func userContentController(_ c: WKUserContentController, didReceive message: WKScriptMessage) {
        print(Date().formatted(date: .omitted, time: .standard), "media:", message.body)
        fflush(stdout)
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let config = WKWebViewConfiguration()
config.websiteDataStore = WKWebsiteDataStore(forIdentifier: uuid)
let probe = Probe()
config.userContentController.add(probe, name: "probe")
config.mediaTypesRequiringUserActionForPlayback = []
config.preferences.isElementFullscreenEnabled = true
// What the page's media elements are doing, every 2 s (in every frame of it).
let watch = """
setInterval(() => {
  const media = [...document.querySelectorAll('video,audio')].map(m => ({
    tag: m.tagName, paused: m.paused, t: Math.round(m.currentTime * 10) / 10, muted: m.muted, volume: m.volume,
    encrypted: !!m.mediaKeys, keySystem: m.mediaKeys ? 'yes' : (m.webkitKeys ? 'webkitKeys' : 'none'),
    size: m.videoWidth ? m.videoWidth + 'x' + m.videoHeight : '', error: m.error ? m.error.code : 0 }));
  if (media.length) window.webkit.messageHandlers.probe.postMessage({page: location.host, visibility: document.visibilityState, media});
}, 2000);
"""
config.userContentController.addUserScript(WKUserScript(source: watch, injectionTime: .atDocumentEnd, forMainFrameOnly: false))

let web = WKWebView(frame: NSRect(x: 0, y: 0, width: 1280, height: 720), configuration: config)
// As Safari, whose version this Mac has: the services know it.
let safari = (NSDictionary(contentsOfFile: "/Applications/Safari.app/Contents/Info.plist")?["CFBundleShortVersionString"] as? String) ?? "18.0"
web.customUserAgent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/\(safari) Safari/605.1.15"
let occlusion = NSSelectorFromString("_setWindowOcclusionDetectionEnabled:")
typealias SetBool = @convention(c) (AnyObject, Selector, Bool) -> Void
unsafeBitCast(web.method(for: occlusion), to: SetBool.self)(web, occlusion, false)

let window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 1280, height: 720),
                      styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
window.title = "WestPlay prototype — \(profile): sign in and press play"
window.contentView = web
window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
web.load(URLRequest(url: url))
print("profile \(profile) (\(uuid)), Safari \(safari)"); fflush(stdout)

// Capture: fresh frames per 2 s, and a snapshot saved each time.
var fresh = 0, last = 0, shot = 0
var capturing = false
func capture() {
    guard capturing else { return }
    let snap = WKSnapshotConfiguration()
    snap.snapshotWidth = NSNumber(value: Double(1280) / Double(window.backingScaleFactor))
    web.takeSnapshot(with: snap) { image, _ in
        if let image, let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil),
           let data = cg.dataProvider?.data, let bytes = CFDataGetBytePtr(data) {
            var hash = 0
            var lit = 0
            let length = CFDataGetLength(data)
            for at in stride(from: 0, to: length, by: 4099) { hash = hash &* 31 &+ Int(bytes[at]); if bytes[at] > 16 { lit += 1 } }
            if hash != last { fresh += 1; last = hash }
            shot += 1
            if shot % 60 == 0 {
                try? NSBitmapImageRep(cgImage: cg).representation(using: .png, properties: [:])?
                    .write(to: URL(fileURLWithPath: "/tmp/wkproto-\(profile).png"))
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0 / 60) { capture() }
    }
}
Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in
    if capturing { print("capture: \(fresh / 2) fresh frames/s, window on screen: \(window.occlusionState.contains(.visible))"); fflush(stdout); fresh = 0 }
}

let away = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
signal(SIGUSR1, SIG_IGN)
away.setEventHandler {
    window.setFrameOrigin(NSPoint(x: -20000, y: -20000))
    window.orderBack(nil)
    capturing = true
    print("moved beyond the displays; capturing"); fflush(stdout)
    capture()
}
away.resume()
let back = DispatchSource.makeSignalSource(signal: SIGUSR2, queue: .main)
signal(SIGUSR2, SIG_IGN)
back.setEventHandler {
    capturing = false
    window.setFrameOrigin(NSPoint(x: 200, y: 200))
    window.makeKeyAndOrderFront(nil)
    print("back on screen"); fflush(stdout)
}
back.resume()
app.run()
