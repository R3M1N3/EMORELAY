import { describe, expect, it } from 'vitest'
import { expiryWarning, expiryWarningKey } from './expiry-warning'

const NOW = Date.parse('2026-06-13T00:00:00Z')
const inDays = (n: number) => new Date(NOW + n * 86400_000).toISOString()

describe('expiryWarning', () => {
  it('永不过期返回 null', () => {
    expect(expiryWarning(null, NOW)).toBeNull()
  })

  it('无法解析的时间返回 null', () => {
    expect(expiryWarning('not-a-date', NOW)).toBeNull()
  })

  it('距到期超过 7 天不提醒', () => {
    expect(expiryWarning(inDays(8), NOW)).toBeNull()
  })

  it('已过期 → expired', () => {
    const w = expiryWarning(inDays(-1), NOW)
    expect(w?.level).toBe('expired')
    expect(w?.daysLeft).toBe(0)
  })

  it('1 天内 → critical', () => {
    const w = expiryWarning(new Date(NOW + 12 * 3600_000).toISOString(), NOW)
    expect(w?.level).toBe('critical')
    expect(w?.daysLeft).toBe(1)
  })

  it('3 天内 → warning', () => {
    const w = expiryWarning(inDays(3), NOW)
    expect(w?.level).toBe('warning')
    expect(w?.daysLeft).toBe(3)
  })

  it('7 天内 → info', () => {
    const w = expiryWarning(inDays(6), NOW)
    expect(w?.level).toBe('info')
    expect(w?.daysLeft).toBe(6)
  })

  it('边界:恰好 7 天提醒,8 天不提醒', () => {
    expect(expiryWarning(inDays(7), NOW)?.level).toBe('info')
    expect(expiryWarning(inDays(8), NOW)).toBeNull()
  })
})

describe('expiryWarningKey', () => {
  it('同级别同日 key 一致,不同日不同', () => {
    const k1 = expiryWarningKey('warning', NOW)
    const k2 = expiryWarningKey('warning', NOW + 3600_000)
    const k3 = expiryWarningKey('warning', NOW + 2 * 86400_000)
    expect(k1).toBe(k2)
    expect(k1).not.toBe(k3)
  })

  it('不同级别 key 不同', () => {
    expect(expiryWarningKey('warning', NOW)).not.toBe(expiryWarningKey('critical', NOW))
  })
})
