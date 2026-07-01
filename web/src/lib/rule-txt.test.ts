import { describe, it, expect } from 'vitest'
import { rulesToTxt, parseTxtToItems } from './rule-txt'
import type { RuleExportItem } from './api'

function baseItem(over: Partial<RuleExportItem>): RuleExportItem {
  return {
    name: 'r', protocol: 'tcp', listen_ip: '0.0.0.0', listen_port: 100,
    target_host: '1.1.1.1', target_port: 80, enabled: true, node_name: 'n',
    tunnel_name: null, bandwidth_profile_name: null, owner_username: null,
    extra_targets: [], lb_strategy: 'fifo', ...over,
  }
}

describe('rulesToTxt', () => {
  it('单目标 → 地址:端口|名称|入口端口', () => {
    expect(rulesToTxt([baseItem({ name: 'a', listen_port: 10001, target_host: '1.1.1.1', target_port: 80 })]))
      .toBe('1.1.1.1:80|a|10001')
  })
  it('多目标逗号拼接,主目标在前', () => {
    expect(rulesToTxt([baseItem({
      name: 'a', listen_port: 10001, target_host: '1.1.1.1', target_port: 80,
      extra_targets: [{ host: '2.2.2.2', port: 81 }],
    })])).toBe('1.1.1.1:80,2.2.2.2:81|a|10001')
  })
})

describe('parseTxtToItems', () => {
  const opts = { nodeName: 'hk-1', protocol: 'tcp_udp' as const }

  it('合法单行 → item(协议/节点/监听IP 来自 opts)', () => {
    const { items, errors } = parseTxtToItems('1.1.1.1:80|r1|20000', opts)
    expect(errors).toEqual([])
    expect(items).toHaveLength(1)
    expect(items[0]).toMatchObject({
      name: 'r1', protocol: 'tcp_udp', listen_ip: '0.0.0.0', listen_port: 20000,
      target_host: '1.1.1.1', target_port: 80, node_name: 'hk-1', lb_strategy: 'fifo',
    })
    expect(items[0].extra_targets).toEqual([])
  })

  it('多目标 → 首个为主,其余进 extra_targets', () => {
    const { items } = parseTxtToItems('1.1.1.1:80,2.2.2.2:81|r1|20000', opts)
    expect(items[0].target_host).toBe('1.1.1.1')
    expect(items[0].extra_targets).toEqual([{ host: '2.2.2.2', port: 81 }])
  })

  it('空行跳过', () => {
    const { items, errors } = parseTxtToItems('\n  \n1.1.1.1:80|r1|20000\n', opts)
    expect(errors).toEqual([])
    expect(items).toHaveLength(1)
  })

  it('缺字段/空端口/坏地址各自报行级错误', () => {
    expect(parseTxtToItems('只有一段', opts).errors).toHaveLength(1)
    expect(parseTxtToItems('1.1.1.1:80|r1|', opts).errors).toHaveLength(1)
    expect(parseTxtToItems('1.1.1.1:80|r1|70000', opts).errors).toHaveLength(1)
    expect(parseTxtToItems('1.1.1.1|r1|20000', opts).errors).toHaveLength(1)
    expect(parseTxtToItems('1.1.1.1:80||20000', opts).errors).toHaveLength(1)
  })

  it('rulesToTxt → parseTxtToItems 往返目标一致', () => {
    const txt = rulesToTxt([baseItem({
      name: 'a', listen_port: 20000, target_host: '1.1.1.1', target_port: 80,
      extra_targets: [{ host: '2.2.2.2', port: 81 }],
    })])
    const { items } = parseTxtToItems(txt, opts)
    expect(items[0].target_host).toBe('1.1.1.1')
    expect(items[0].target_port).toBe(80)
    expect(items[0].extra_targets).toEqual([{ host: '2.2.2.2', port: 81 }])
  })
})
