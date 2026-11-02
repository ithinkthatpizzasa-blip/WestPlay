import { describe, expect, it } from 'vitest'
import cabSound from '../../crates/host/src/cab_sound.js?raw'

/**
 * The truck speakers' script (crates/host/src/cab_sound.js) run against a stand-in for a player's page: whether
 * it takes the element playing into its sound's way (Web Audio). Anything of the page not stood in for here is a
 * harmless nothing.
 */

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Loose = any

const nothing: Loose = new Proxy(function () {}, {
  get: (_, key) => (key === 'then' ? undefined : key === Symbol.toPrimitive ? () => 0 : nothing),
  apply: () => nothing,
  construct: () => nothing,
})

/** The script run in WestPlay's WebKit (macOS) or in WebView2 (Windows); `contexts`: Web Audio contexts made. */
function runPage(engine: 'webkit' | 'webview2') {
  const made = { contexts: 0 }
  class HTMLMediaElement {
    mediaKeys: Loose = null
  }
  Object.assign(HTMLMediaElement.prototype, {
    // As the browser does: the keys set some time after.
    setMediaKeys() {
      return Promise.resolve()
    },
  })
  class AudioContext {
    state = 'running'
    constructor() {
      made.contexts++
    }
  }
  const bridge = { postMessage() {}, addEventListener() {} }
  const own: Record<PropertyKey, Loose> = {
    HTMLMediaElement,
    AudioContext,
    ...(engine === 'webkit'
      ? { __westplayWebview: bridge }
      : { __westplayWebview: undefined, chrome: { webview: bridge } }),
  }
  const page: Loose = new Proxy(own, {
    has: () => true,
    get: (_, key) =>
      key === Symbol.unscopables
        ? undefined
        : key === 'window'
          ? page
          : key in own
            ? own[key]
            : key in globalThis
              ? (globalThis as Loose)[key]
              : nothing,
    set: (_, key, value) => ((own[key] = value), true),
  })
  new Function('page', `with (page) {\n${cabSound}\n}`)(page)
  const element: Loose = new HTMLMediaElement()
  const cab = page.westplayCabSound(() => element)
  const tellTheCab = () => {
    cab.speakers([{ band: [0, 0], level: 0, channel: 'left', at: [-1, 1, 0] }])
    cab.head({ x: -0.7, y: 1.6, z: -0.4, heading: 0, pitch: 0 })
  }
  return { element, cab, made, tellTheCab }
}

describe('the truck speakers', () => {
  it('leave a protected element alone in WebKit, from the moment it is given its keys', () => {
    const { element, cab, made, tellTheCab } = runPage('webkit')
    element.setMediaKeys({})
    tellTheCab()
    expect(made.contexts).toBe(0)
    expect(cab.protected()).toBe(true)
  })

  it('take an element without keys in WebKit', () => {
    const { cab, made, tellTheCab } = runPage('webkit')
    tellTheCab()
    expect(made.contexts).toBe(1)
    expect(cab.protected()).toBe(false)
  })

  it('take a protected element in WebView2, as ever', () => {
    const { element, cab, made, tellTheCab } = runPage('webview2')
    element.setMediaKeys({})
    tellTheCab()
    expect(made.contexts).toBe(1)
    expect(cab.protected()).toBe(false)
  })
})
