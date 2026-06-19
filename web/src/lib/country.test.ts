import { describe, expect, it } from 'vitest'
import { COMMON_COUNTRY_CODES, countryName, isCountryCode, normalizeRegion } from './country'

describe('isCountryCode', () => {
  it('两字母(大小写/空白不敏感)为真', () => {
    expect(isCountryCode('JP')).toBe(true)
    expect(isCountryCode('jp')).toBe(true)
    expect(isCountryCode(' hk ')).toBe(true)
  })
  it('非两字母为假', () => {
    expect(isCountryCode('JPN')).toBe(false)
    expect(isCountryCode('香港')).toBe(false)
    expect(isCountryCode('')).toBe(false)
    expect(isCountryCode('J1')).toBe(false)
  })
})

describe('countryName', () => {
  it('合法码 → 中文名(大小写不敏感)', () => {
    expect(countryName('JP')).toBe('日本')
    expect(countryName('jp')).toBe('日本')
  })
  it('非法/历史自由文本原样返回(降级)', () => {
    expect(countryName('香港')).toBe('香港')
    expect(countryName('SGP')).toBe('SGP')
  })
})

describe('normalizeRegion', () => {
  it('两字母码统一大写', () => {
    expect(normalizeRegion('jp')).toBe('JP')
    expect(normalizeRegion(' hk ')).toBe('HK')
  })
  it('非码原样(去首尾空白)', () => {
    expect(normalizeRegion(' 香港 ')).toBe('香港')
    expect(normalizeRegion('SGP')).toBe('SGP')
  })
})

it('COMMON_COUNTRY_CODES 均为合法两字母大写码', () => {
  for (const c of COMMON_COUNTRY_CODES) {
    expect(isCountryCode(c)).toBe(true)
    expect(c).toBe(c.toUpperCase())
  }
})
