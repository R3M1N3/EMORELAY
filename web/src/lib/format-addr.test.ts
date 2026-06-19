import { describe, expect, it } from 'vitest'
import { formatHostPort, nodeEntryHost } from './format-addr'

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

describe('nodeEntryHost', () => {
  it('优先返回 display_address(去空白)', () => {
    expect(nodeEntryHost({ display_address: 'relay.example.com', public_ip: '1.2.3.4' })).toBe(
      'relay.example.com',
    )
    expect(nodeEntryHost({ display_address: '  relay.example.com  ', public_ip: '1.2.3.4' })).toBe(
      'relay.example.com',
    )
  })

  it('display_address 为空/空白/null 时回落 public_ip', () => {
    expect(nodeEntryHost({ display_address: '', public_ip: '1.2.3.4' })).toBe('1.2.3.4')
    expect(nodeEntryHost({ display_address: '   ', public_ip: '1.2.3.4' })).toBe('1.2.3.4')
    expect(nodeEntryHost({ display_address: null, public_ip: '1.2.3.4' })).toBe('1.2.3.4')
  })

  it('node 缺失(已删/未授权)返回空串,由调用方回落', () => {
    expect(nodeEntryHost(undefined)).toBe('')
    expect(nodeEntryHost(null)).toBe('')
  })

  it('与 formatHostPort 组合得到入口地址', () => {
    const host = nodeEntryHost({ display_address: 'hk.example.com', public_ip: '1.2.3.4' })
    expect(formatHostPort(host, 20001)).toBe('hk.example.com:20001')
  })
})
