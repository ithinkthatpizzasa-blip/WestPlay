// WestPlay Phase 2 probe (scale.swift): a WKWebView on a Retina display drawn at 1x (WebKit's
// `_setOverrideDeviceScaleFactor:`), as WebView2 draws WestPlay's pages (`SetRasterizationScale(1)`): the page's
// devicePixelRatio, and its snapshot's size. Build: swiftc -O -o scale scale.swift
import AppKit
import WebKit
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let web = WKWebView(frame: NSRect(x: 0, y: 0, width: 1280, height: 720), configuration: WKWebViewConfiguration())
let sel = NSSelectorFromString("_setOverrideDeviceScaleFactor:")
print("has _setOverrideDeviceScaleFactor:", web.responds(to: sel))
typealias SetFloat = @convention(c) (AnyObject, Selector, CGFloat) -> Void
unsafeBitCast(web.method(for: sel), to: SetFloat.self)(web, sel, 1)
let window = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 1280, height: 720), styleMask: [.borderless], backing: .buffered, defer: false)
window.contentView = web
window.alphaValue = 0.001
window.orderBack(nil)
print("window backing scale:", window.backingScaleFactor)
web.loadHTMLString("<body style='margin:0;background:#c00'><div style='width:100px;height:100px;background:#0c0'></div></body>", baseURL: nil)
DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
    web.evaluateJavaScript("window.devicePixelRatio") { r, _ in print("devicePixelRatio:", r ?? "?") }
    let config = WKSnapshotConfiguration()
    config.snapshotWidth = 1280
    let start = Date()
    web.takeSnapshot(with: config) { image, _ in
        let cg = image?.cgImage(forProposedRect: nil, context: nil, hints: nil)
        print("snapshot \(cg?.width ?? 0)x\(cg?.height ?? 0) in \(Int(Date().timeIntervalSince(start) * 1000)) ms")
        app.terminate(nil)
    }
}
app.run()
