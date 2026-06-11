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
  // P4 起 shortTime 把后端 UTC 时间转为浏览器本地时区显示。
  // 断言以「与 Date 本地字段一致」表达,不绑死测试机时区。
  function expectedLocal(d: Date): string {
    const p = (n: number) => String(n).padStart(2, '0')
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
  }

  it('converts explicit-UTC ISO strings to local time', () => {
    const input = '2026-06-09T14:23:45Z'
    expect(shortTime(input)).toBe(expectedLocal(new Date(input)))
  })

  it('treats SQLite-style strings (no zone marker) as UTC then localizes', () => {
    const input = '2026-06-09 14:23:45'
    expect(shortTime(input)).toBe(expectedLocal(new Date('2026-06-09T14:23:45Z')))
  })

  it('parses date-only strings as UTC midnight', () => {
    expect(shortTime('2026-06-09')).toBe(expectedLocal(new Date('2026-06-09T00:00:00Z')))
  })

  it('falls back to raw truncation for unparseable strings', () => {
    expect(shortTime('not-a-date')).toBe('not-a-date')
  })
})
