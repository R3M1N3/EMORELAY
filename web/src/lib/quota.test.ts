import { describe, it, expect } from 'vitest'
import { quotaTone, quotaPercent, gbToBytes, bytesToGbString } from './quota'

describe('quota helpers', () => {
  it('quotaPercent clamps to 0-100 and handles null limit', () => {
    expect(quotaPercent(50, 100)).toBe(50)
    expect(quotaPercent(150, 100)).toBe(100)
    expect(quotaPercent(0, 100)).toBe(0)
    expect(quotaPercent(10, null)).toBeNull()
    expect(quotaPercent(10, 0)).toBeNull()
  })

  it('quotaTone: green <70, amber 70-90, red >=90', () => {
    expect(quotaTone(69)).toBe('green')
    expect(quotaTone(70)).toBe('amber')
    expect(quotaTone(89.9)).toBe('amber')
    expect(quotaTone(90)).toBe('red')
    expect(quotaTone(100)).toBe('red')
  })

  it('gbToBytes / bytesToGbString roundtrip', () => {
    expect(gbToBytes('1')).toBe(1073741824)
    expect(gbToBytes('0.5')).toBe(536870912)
    expect(gbToBytes('')).toBeNull()
    expect(gbToBytes('abc')).toBeUndefined()
    expect(bytesToGbString(1073741824)).toBe('1')
    expect(bytesToGbString(null)).toBe('')
  })
})
