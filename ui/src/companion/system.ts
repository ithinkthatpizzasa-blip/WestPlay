/**
 * The system WestPlay runs on, for the words and places that differ between them: the WebView's own (WebKit's
 * on the Mac says Macintosh; WebView2's, Windows). Opened in a plain browser (the preview), `?system=mac` or
 * `?system=windows` shows either; else the browser's.
 */
export type System = 'mac' | 'windows'

/** The system a WebView's `userAgent` says, unless `asked` (the preview's `?system=`) names one. */
export function systemOf(userAgent: string, asked: string | null = null): System {
  if (asked === 'mac' || asked === 'windows') return asked
  return /Macintosh|Mac OS X/.test(userAgent) ? 'mac' : 'windows'
}

export const SYSTEM: System = (() => {
  const inApp = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
  const asked = inApp || typeof location === 'undefined' ? null : new URLSearchParams(location.search).get('system')
  return systemOf(typeof navigator === 'undefined' ? '' : navigator.userAgent, asked)
})()
