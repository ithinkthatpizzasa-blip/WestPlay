// WestPlay Phase 2 prototype (capture.swift): a WKWebView kept out of sight, its page's animation frames counted,
// captured with takeSnapshot as fast as it goes, and the media DRM it offers.
// Build: swiftc -O -o capture capture.swift. Usage: capture <mode> [url or .html file]
//   mode: offscreen (window beyond the displays), hidden (ordered out), behind (on screen, alpha 0)
import AppKit
import WebKit

let args = CommandLine.arguments
let mode = args.count > 1 ? args[1] : "offscreen"
let url = args.count > 2 ? args[2] : nil

let page = """
<html><body style="margin:0;background:#123;color:white;font:40px -apple-system">
<div id=t>0</div><canvas id=c width=1280 height=600></canvas>
<script>
let frames = 0, start = performance.now();
const ctx = c.getContext('2d');
function tick(now) { frames++; ctx.fillStyle = `rgb(${frames % 256},0,0)`; ctx.fillRect(0,0,1280,600);
  t.textContent = frames; requestAnimationFrame(tick); }
requestAnimationFrame(tick);
setInterval(() => window.webkit.messageHandlers.probe.postMessage(
  {raf: frames, secs: (performance.now() - start) / 1000, visibility: document.visibilityState}), 1000);
async function drm() {
  const out = {};
  const cfg = [{initDataTypes: ['cenc','sinf','skd'], audioCapabilities: [{contentType: 'audio/mp4;codecs="mp4a.40.2"'}],
    videoCapabilities: [{contentType: 'video/mp4;codecs="avc1.42E01E"'}]}];
  for (const ks of ['com.apple.fps', 'com.apple.fps.1_0', 'com.apple.fps.2_0', 'com.widevine.alpha', 'org.w3.clearkey']) {
    try { await navigator.requestMediaKeySystemAccess(ks, cfg); out[ks] = 'yes'; } catch (e) { out[ks] = 'no: ' + e.name; }
  }
  out.WebKitMediaKeys = typeof WebKitMediaKeys !== 'undefined' ? WebKitMediaKeys.isTypeSupported('com.apple.fps.1_0', 'video/mp4') : 'absent';
  out.mse = typeof MediaSource !== 'undefined' && MediaSource.isTypeSupported('video/mp4;codecs="avc1.42E01E"');
  out.managedMse = typeof ManagedMediaSource !== 'undefined';
  out.aac = document.createElement('audio').canPlayType('audio/mp4;codecs="mp4a.40.2"');
  out.opus = document.createElement('audio').canPlayType('audio/webm;codecs="opus"');
  out.vp9 = document.createElement('video').canPlayType('video/webm;codecs="vp9"');
  out.ua = navigator.userAgent;
  window.webkit.messageHandlers.probe.postMessage({drm: out});
}
drm();
</script></body></html>
"""

final class Probe: NSObject, WKScriptMessageHandler {
    func userContentController(_ c: WKUserContentController, didReceive message: WKScriptMessage) {
        print("page:", message.body)
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let config = WKWebViewConfiguration()
let probe = Probe()
config.userContentController.add(probe, name: "probe")
config.mediaTypesRequiringUserActionForPlayback = []
let web = WKWebView(frame: NSRect(x: 0, y: 0, width: 1280, height: 720), configuration: config)
// WebKit's own switch (private): don't treat a window out of sight as hidden.
let occlusion = NSSelectorFromString("_setWindowOcclusionDetectionEnabled:")
if ProcessInfo.processInfo.environment["KEEP_OCCLUSION"] == nil && web.responds(to: occlusion) {
    typealias SetBool = @convention(c) (AnyObject, Selector, Bool) -> Void
    unsafeBitCast(web.method(for: occlusion), to: SetBool.self)(web, occlusion, false)
    print("occlusion detection disabled")
}
let origin: NSPoint = mode == "offscreen" ? NSPoint(x: -20000, y: -20000) : NSPoint(x: 0, y: 0)
let window = NSWindow(contentRect: NSRect(origin: origin, size: NSSize(width: 1280, height: 720)),
                      styleMask: [.borderless], backing: .buffered, defer: false)
window.contentView = web
window.isReleasedWhenClosed = false
switch mode {
case "hidden": break
case "behind": window.alphaValue = 0; window.ignoresMouseEvents = true; window.orderBack(nil)
default: window.orderBack(nil)
}
print("mode \(mode), window visible: \(window.isVisible), on screen: \(window.occlusionState.contains(.visible))")
if let url, url.hasSuffix(".html") { web.loadFileURL(URL(fileURLWithPath: url), allowingReadAccessTo: URL(fileURLWithPath: "/")) } else if let url, let u = URL(string: url) { web.load(URLRequest(url: u)) } else { web.loadHTMLString(page, baseURL: URL(string: "https://westplay.cabin/")) }

// takeSnapshot as fast as it completes, for 5 s, after the page has had 3 s: how many distinct page frames
// arrive (the canvas's red is the page's frame number), at what size, and what reading the pixels costs.
let after = ProcessInfo.processInfo.environment["AFTER_UPDATES"] != nil
var snaps = 0, fresh = 0, lastRed = -1
var readTime = 0.0
var started = Date()
var size = ""
func snapLoop() {
    let config = WKSnapshotConfiguration()
    config.afterScreenUpdates = after
    config.snapshotWidth = NSNumber(value: Double(1280) / Double(window.backingScaleFactor))
    web.takeSnapshot(with: config) { image, error in
        if let error { print("snapshot error:", error); return }
        snaps += 1
        let t = Date()
        if let image, let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) {
            size = "\(cg.width)x\(cg.height)"
            var pixel = [UInt8](repeating: 0, count: cg.width * cg.height * 4)
            let ctx = CGContext(data: &pixel, width: cg.width, height: cg.height, bitsPerComponent: 8,
                                bytesPerRow: cg.width * 4, space: CGColorSpaceCreateDeviceRGB(),
                                bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)!
            ctx.draw(cg, in: CGRect(x: 0, y: 0, width: cg.width, height: cg.height))
            var hash = 0
            for row in stride(from: 0, to: cg.height, by: 37) { for col in stride(from: 0, to: cg.width * 4, by: 61) { hash = hash &* 31 &+ Int(pixel[row * cg.width * 4 + col]) } }
            if hash != lastRed { fresh += 1; lastRed = hash }
            if snaps == 200 { try? NSBitmapImageRep(cgImage: cg).representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: "/tmp/wkproto-snap.png")) }
        }
        readTime += Date().timeIntervalSince(t)
        if Date().timeIntervalSince(started) < 5 { snapLoop() } else {
            print(String(format: "takeSnapshot (afterScreenUpdates %@): %d done, %d fresh page frames in 5 s = %.1f fresh/s, %@, pixels read in %.2f ms each",
                         after ? "true" : "false", snaps, fresh, Double(fresh) / 5, size, readTime * 1000 / Double(max(snaps, 1))))
        }
    }
}
DispatchQueue.main.asyncAfter(deadline: .now() + 3) { started = Date(); snapLoop() }
DispatchQueue.main.asyncAfter(deadline: .now() + 10) { app.terminate(nil) }
app.run()
