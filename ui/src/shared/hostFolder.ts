/**
 * Where the host serves one of WestPlay's own folders to the cabin page (the maps, the player's own looks):
 * `https://<name>` as WebView2 serves it, and `westplay://<name>` in WestPlay's WebKit on macOS, which gives
 * `https:` to no app (crates/host/src/webview/macos.rs). WestPlay's WebKit is known by its bridge
 * (`window.__westplayWebview`).
 */
export function hostFolder(name: string, from: object = globalThis): string {
  return '__westplayWebview' in from ? `westplay://${name}` : `https://${name}`
}
