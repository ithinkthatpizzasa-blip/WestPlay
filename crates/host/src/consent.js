// WestPlay, on YouTube's cookie page ("Before you continue to YouTube", consent.youtube.com: in the EU, the UK and
// Switzerland), in YouTube's and YouTube Music's hidden players, where nobody is there to answer it: answered as its
// Reject all answers it. The cookie set before they load (youtube.rs `answer_consent`) usually keeps it away;
// YouTube Music still showed it now and then, the cookie there (2026-10-27). Its Reject all is the form that saves
// no YouTube cookies (no `set_ytc`) and says `set_eom` (whatever the page's language). Once a visit: should YouTube
// ask again, it's left to the log (the page's own address says so).
;(() => {
  if (location.hostname !== 'consent.youtube.com' || window.top !== window) return
  const reject = () => {
    const form = [...document.forms].find(
      (each) => each.elements.namedItem('set_eom')?.value === 'true' && !each.elements.namedItem('set_ytc'),
    )
    if (!form) return
    try {
      if (sessionStorage.getItem('westplay-consent')) return
      sessionStorage.setItem('westplay-consent', '1')
    } catch {}
    form.submit()
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', reject, { once: true })
  else reject()
})()

// The same question asked in YouTube's own page (a dialog over www.youtube.com or music.youtube.com, not a page of
// its own), as YouTube asks Safari, and so WestPlay's WebKit on macOS: the cookie set before doesn't keep it away
// there (2026-11-01: the video waited behind it). Answered as its Reject all answers: the button whose action is
// YouTube's `disablePersonalizationAction` (Accept all's is `saveConsentAction`), whatever the page's language. Only in
// WestPlay's WebKit (its bridge, `window.__westplayWebview`): WebView2 is answered as above.
;(() => {
  if (!window.__westplayWebview || window.top !== window || !location.hostname.endsWith('youtube.com')) return
  const until = Date.now() + 60_000
  const answer = () => {
    const dialog = document.querySelector('ytd-consent-bump-v2-lightbox')
    const reject = [...(dialog?.querySelectorAll('ytd-button-renderer') ?? [])].find(
      (each) => (each.data ?? each.__data?.data)?.command?.disablePersonalizationAction,
    )
    const button = reject?.querySelector('button')
    if (button) {
      button.click()
      return
    }
    if (Date.now() < until) setTimeout(answer, 500)
  }
  answer()
})()
