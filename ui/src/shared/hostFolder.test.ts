import { describe, expect, it } from 'vitest'
import { hostFolder } from './hostFolder'

describe('hostFolder', () => {
  it("is WebView2's https address, or WestPlay's own scheme in its WebKit", () => {
    expect(hostFolder('westplay.maps', {})).toBe('https://westplay.maps')
    expect(hostFolder('westplay.maps', { __westplayWebview: {} })).toBe('westplay://westplay.maps')
  })
})
