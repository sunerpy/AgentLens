import { describe, expect, it } from 'vitest'

import { detectPlatform, currentPlatform, TITLEBAR_PLATFORMS } from './platform'

const MAC =
  'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15'
const WINDOWS =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36'
const LINUX =
  'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36'
const WEBKITGTK =
  'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15'
const IOS =
  'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1'
const ANDROID =
  'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36'

describe('detectPlatform', () => {
  it('identifies the three desktop platforms AgentLens ships for', () => {
    expect(detectPlatform(MAC)).toBe('macos')
    expect(detectPlatform(WINDOWS)).toBe('windows')
    expect(detectPlatform(LINUX)).toBe('linux')
  })

  it('identifies the WebKitGTK agent as linux, not macos', () => {
    // WebKitGTK is the Linux Tauri webview and reports `AppleWebKit` + `Safari`, so a
    // naive Safari/WebKit check would classify a Linux window as macOS and then reserve
    // an 80px traffic-light inset that nothing occupies.
    expect(detectPlatform(WEBKITGTK)).toBe('linux')
  })

  it('screens mobile agents before the desktop patterns they overlap with', () => {
    expect(detectPlatform(IOS)).toBe('unknown')
    expect(detectPlatform(ANDROID)).toBe('unknown')
  })

  it('degrades an unrecognised or malformed agent to unknown instead of throwing', () => {
    expect(detectPlatform('')).toBe('unknown')
    expect(detectPlatform('SomeFutureOS/1.0')).toBe('unknown')
    expect(detectPlatform('\u0000\uFFFD not a user agent')).toBe('unknown')
    expect(detectPlatform('null')).toBe('unknown')
  })

  it('only ever returns a member of the declared union', () => {
    for (const agent of [MAC, WINDOWS, LINUX, WEBKITGTK, IOS, ANDROID, '', 'garbage']) {
      expect(TITLEBAR_PLATFORMS).toContain(detectPlatform(agent))
    }
  })
})

describe('currentPlatform', () => {
  it('reads the live navigator user-agent', () => {
    expect(TITLEBAR_PLATFORMS).toContain(currentPlatform())
  })
})
