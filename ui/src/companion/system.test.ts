import { describe, expect, it } from 'vitest'
import { systemOf } from './system'

describe('the system', () => {
  it('is the WebView’s', () => {
    // WebKit's on macOS (its WKWebView's), WebView2's on Windows.
    expect(systemOf('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)')).toBe(
      'mac',
    )
    expect(
      systemOf(
        'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0',
      ),
    ).toBe('windows')
  })

  it('is the one the preview asks for', () => {
    expect(systemOf('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', 'windows')).toBe('windows')
    expect(systemOf('Mozilla/5.0 (Windows NT 10.0; Win64; x64)', 'mac')).toBe('mac')
    expect(systemOf('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', 'linux')).toBe('mac')
  })
})
