import { describe, expect, it } from 'vitest'
import { formatHostPort } from './format-addr'

describe('formatHostPort', () => {
  it('拼接 IPv4 与端口', () => {
    expect(formatHostPort('1.2.3.4', 8080)).toBe('1.2.3.4:8080')
  })

  it('拼接域名与端口', () => {
    expect(formatHostPort('example.com', 443)).toBe('example.com:443')
  })

  it('IPv6 字面量加方括号', () => {
    expect(formatHostPort('::1', 8080)).toBe('[::1]:8080')
    expect(formatHostPort('2001:db8::1', 80)).toBe('[2001:db8::1]:80')
  })

  it('已带方括号的 IPv6 不重复包裹', () => {
    expect(formatHostPort('[::1]', 8080)).toBe('[::1]:8080')
  })

  it('去除首尾空白', () => {
    expect(formatHostPort('  1.2.3.4  ', 22)).toBe('1.2.3.4:22')
  })
})
