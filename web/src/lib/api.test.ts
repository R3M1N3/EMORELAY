// 纯函数单测:不需 DOM,不需要 mock fetch。仅验证 lib/api 内的 formatter。
import { describe, it, expect } from 'vitest'
import { formatBytes, shortTime } from './api'

describe('formatBytes', () => {
  it('returns B for values < 1024', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(1023)).toBe('1023 B')
  })

  it('scales to KB / MB / GB / TB on 1024 boundaries', () => {
    expect(formatBytes(1024)).toBe('1.00 KB')
    expect(formatBytes(1024 * 1024)).toBe('1.00 MB')
    expect(formatBytes(1024 ** 3)).toBe('1.00 GB')
    expect(formatBytes(1024 ** 4)).toBe('1.00 TB')
  })

  it('caps at TB (no PB unit)', () => {
    // 1024 PB = 1024 ** 5 字节 -> 应仍以 TB 显示
    expect(formatBytes(1024 ** 5)).toMatch(/^[\d.]+ TB$/)
  })

  it('formats with 2 decimal places above KB', () => {
    expect(formatBytes(1536)).toBe('1.50 KB') // 1.5 KB
    expect(formatBytes(2_500_000)).toBe('2.38 MB')
  })
})

describe('shortTime', () => {
  it('replaces T with space and slices to YYYY-MM-DD HH:MM', () => {
    expect(shortTime('2026-06-09T14:23:45Z')).toBe('2026-06-09 14:23')
  })

  it('passes through SQLite-style strings without T', () => {
    expect(shortTime('2026-06-09 14:23:45')).toBe('2026-06-09 14:23')
  })

  it('truncates strings shorter than 16 chars to whatever fits', () => {
    expect(shortTime('2026-06-09')).toBe('2026-06-09')
  })
})
