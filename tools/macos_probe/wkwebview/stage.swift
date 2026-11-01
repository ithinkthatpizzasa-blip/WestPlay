// WestPlay Phase 2 prototype (stage.swift): the host's stage on macOS. Two WKWebViews in one window beyond the
// displays: a player (video) under the cabin page, whose background is transparent but for a hole. Checks:
// - the cabin page served by a scheme handler of WestPlay's own: does it load, is it a secure context, does it
//   keep localStorage;
// - each WebView's snapshot, the cabin page's with its transparency, composited into one frame, and the cost;
// - mouse input sent as NSEvents to a window that is never key: moves (hover), clicks, the scroll wheel.
// Build: swiftc -O -o stage stage.swift. Usage: [ONSCREEN=1] [INVISIBLE=1] [PIXELS=1] [VIA_WINDOW=1] stage <video.html>
//   ONSCREEN: the window on a display (otherwise beyond them); INVISIBLE: transparent, click-through, out of
//   screenshots; PIXELS: the wheel in pixels (otherwise lines); VIA_WINDOW: the wheel through the window.
import AppKit
import WebKit

let videoPage = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "video.html")

let cabin = """
<!doctype html><html><head><style>
html, body { margin: 0; background: transparent; width: 1280px; height: 720px; overflow: hidden; font: 28px -apple-system; }
#bar { position: absolute; left: 0; top: 0; width: 320px; height: 720px; background: rgba(20, 30, 60, 0.92); color: white; }
#hole { position: absolute; left: 400px; top: 120px; width: 800px; height: 450px; outline: 4px solid #4c4; }
#button { position: absolute; left: 40px; top: 300px; width: 240px; height: 80px; background: #36c; color: white;
  border-radius: 16px; text-align: center; line-height: 80px; }
#button:hover { background: #c63; }
#list { position: absolute; left: 20px; top: 420px; width: 280px; height: 260px; overflow-y: scroll; color: white; }
</style></head><body>
<div id=bar>WestPlay<div id=button>Press</div><div id=list>\(Array(1...40).map { "<div>row \($0)</div>" }.joined())</div></div>
<div id=hole></div>
<script>
const say = (what) => window.webkit.messageHandlers.probe.postMessage(what);
say({origin: location.origin, secure: window.isSecureContext, storage: (() => { try {
  const n = Number(localStorage.getItem('n') || 0) + 1; localStorage.setItem('n', n); return n; } catch (e) { return 'no: ' + e; } })(),
  crypto: typeof crypto.subtle });
button.addEventListener('mouseenter', () => say({hover: 'in'}));
button.addEventListener('click', (e) => say({click: [e.clientX, e.clientY], button: e.button}));
document.addEventListener('mousedown', (e) => say({down: [e.clientX, e.clientY]}));
list.addEventListener('scroll', () => say({scrollTop: list.scrollTop}));
fetch('westplay://cabin/data.json').then(r => r.json()).then(j => say({fetched: j})).catch(e => say({fetched: 'no: ' + e}));
</script></body></html>
"""

final class Cabin: NSObject, WKURLSchemeHandler {
    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        let path = task.request.url?.path ?? "/"
        let (body, type) = path.hasSuffix(".json") ? (Data("{\"ok\": true}".utf8), "application/json")
                                                    : (Data(cabin.utf8), "text/html")
        task.didReceive(HTTPURLResponse(url: task.request.url!, statusCode: 200, httpVersion: "HTTP/1.1",
                                        headerFields: ["Content-Type": type, "Access-Control-Allow-Origin": "*"])!)
        task.didReceive(body)
        task.didFinish()
    }
    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {}
}

final class Probe: NSObject, WKScriptMessageHandler {
    func userContentController(_ c: WKUserContentController, didReceive message: WKScriptMessage) {
        print("cabin:", message.body); fflush(stdout)
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
typealias SetBool = @convention(c) (AnyObject, Selector, Bool) -> Void
func keepDrawing(_ web: WKWebView) {
    let occlusion = NSSelectorFromString("_setWindowOcclusionDetectionEnabled:")
    unsafeBitCast(web.method(for: occlusion), to: SetBool.self)(web, occlusion, false)
}

let frame = NSRect(x: 0, y: 0, width: 1280, height: 720)
let playerConfig = WKWebViewConfiguration()
playerConfig.mediaTypesRequiringUserActionForPlayback = []
let player = WKWebView(frame: frame, configuration: playerConfig)
keepDrawing(player)

let cabinConfig = WKWebViewConfiguration()
let probe = Probe()
cabinConfig.userContentController.add(probe, name: "probe")
cabinConfig.setURLSchemeHandler(Cabin(), forURLScheme: "westplay")
let page = WKWebView(frame: frame, configuration: cabinConfig)
page.setValue(false, forKey: "drawsBackground")
keepDrawing(page)

let stage = NSView(frame: frame)
stage.addSubview(player)
stage.addSubview(page)  // on top
let at: CGFloat = ProcessInfo.processInfo.environment["ONSCREEN"] != nil ? 100 : -20000
let window = NSWindow(contentRect: NSRect(x: at, y: at, width: 1280, height: 720), styleMask: [.borderless],
                      backing: .buffered, defer: false)
window.contentView = stage
if ProcessInfo.processInfo.environment["INVISIBLE"] != nil {
    // On screen but unseen: fully transparent, clicks pass through it, out of screenshots and window lists.
    window.alphaValue = 0
    window.ignoresMouseEvents = true
    window.sharingType = .none
    window.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .transient]
    window.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.desktopWindow)))
    print("invisible window, on screen: \(window.occlusionState.contains(.visible))")
}
window.orderBack(nil)
player.loadFileURL(videoPage, allowingReadAccessTo: videoPage.deletingLastPathComponent())
page.load(URLRequest(url: URL(string: "westplay://cabin/index.html")!))

// Mouse input, in the page's coordinates (top-left origin), sent as a window would send it.
func mouse(_ type: NSEvent.EventType, _ x: CGFloat, _ y: CGFloat) {
    let point = NSPoint(x: x, y: 720 - y)  // the window's coordinates: bottom-left origin
    guard let event = NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                         windowNumber: window.windowNumber, context: nil, eventNumber: 0,
                                         clickCount: type == .mouseMoved ? 0 : 1, pressure: type == .leftMouseDown ? 1 : 0) else { return }
    switch type {
    case .mouseMoved: page.mouseMoved(with: event)
    case .leftMouseDown: page.mouseDown(with: event)
    case .leftMouseUp: page.mouseUp(with: event)
    default: break
    }
}
func wheel(_ x: CGFloat, _ y: CGFloat, lines: Int32) {
    let env = ProcessInfo.processInfo.environment
    let units: CGScrollEventUnit = env["PIXELS"] != nil ? .pixel : .line
    guard let cg = CGEvent(scrollWheelEvent2Source: nil, units: units, wheelCount: 1, wheel1: env["PIXELS"] != nil ? lines * 40 : lines, wheel2: 0, wheel3: 0) else { return }
    // A scroll event carries a point on the screen (CG's: top-left origin of the main display), which WebKit
    // hit-tests in the window: the window's point taken there, beyond the displays as it is.
    let onScreen = window.convertPoint(toScreen: NSPoint(x: x, y: 720 - y))
    let mainHeight = NSScreen.screens.first?.frame.height ?? 0
    cg.location = CGPoint(x: onScreen.x, y: mainHeight - onScreen.y)
    guard let event = NSEvent(cgEvent: cg) else { return }
    print("wheel event: window \(event.window.map { String($0.windowNumber) } ?? "none"), at \(event.locationInWindow), dy \(event.scrollingDeltaY)"); fflush(stdout)
    if env["VIA_WINDOW"] != nil { window.sendEvent(event) } else { page.scrollWheel(with: event) }
}

DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
    mouse(.mouseMoved, 160, 340)
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { mouse(.leftMouseDown, 160, 340); mouse(.leftMouseUp, 160, 340) }
    for i in 0..<5 { DispatchQueue.main.asyncAfter(deadline: .now() + 0.6 + Double(i) * 0.1) { wheel(160, 550, lines: -5) } }
}

// Composite: the player, then the cabin page over it, as the frame the game gets.
func snapshot(_ web: WKWebView, _ done: @escaping (CGImage?) -> Void) {
    let config = WKSnapshotConfiguration()
    config.snapshotWidth = NSNumber(value: 1280.0 / Double(window.backingScaleFactor))
    web.takeSnapshot(with: config) { image, _ in done(image?.cgImage(forProposedRect: nil, context: nil, hints: nil)) }
}
var frames = 0, compositeTime = 0.0
let context = CGContext(data: nil, width: 1280, height: 720, bitsPerComponent: 8, bytesPerRow: 1280 * 4,
                        space: CGColorSpace(name: CGColorSpace.sRGB)!,
                        bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)!
var started = Date()
func compose() {
    snapshot(player) { under in
        snapshot(page) { over in
            let t = Date()
            let rect = CGRect(x: 0, y: 0, width: 1280, height: 720)
            context.clear(rect)
            if let under { context.draw(under, in: rect) }
            if let over { context.draw(over, in: rect) }
            compositeTime += Date().timeIntervalSince(t)
            frames += 1
            if frames == 1, let over {
                print("cabin snapshot: \(over.width)x\(over.height), alpha \(over.alphaInfo.rawValue)")
                // The hole's middle: transparent?
                let pixels = context.data!.assumingMemoryBound(to: UInt8.self)
                _ = pixels
            }
            if Date().timeIntervalSince(started) < 5 { compose() } else {
                print(String(format: "composited %d frames in 5 s (%.1f/s), %.2f ms each to composite", frames, Double(frames) / 5,
                             compositeTime * 1000 / Double(frames)))
                if let image = context.makeImage() {
                    try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?
                        .write(to: URL(fileURLWithPath: "/tmp/wkproto-stage.png"))
                }
                if let under {
                    try? NSBitmapImageRep(cgImage: under).representation(using: .png, properties: [:])?
                        .write(to: URL(fileURLWithPath: "/tmp/wkproto-player.png"))
                }
                if let over {
                    try? NSBitmapImageRep(cgImage: over).representation(using: .png, properties: [:])?
                        .write(to: URL(fileURLWithPath: "/tmp/wkproto-cabin.png"))
                }
                app.terminate(nil)
            }
        }
    }
}
DispatchQueue.main.asyncAfter(deadline: .now() + 4) { started = Date(); compose() }
app.run()
