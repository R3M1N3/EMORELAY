import type { RuleExportItem, TargetDto } from './api'

/** 规则数组 → TXT 行(目标地址列表|名称|入口端口)。多目标逗号分隔,主目标在前。 */
export function rulesToTxt(items: RuleExportItem[]): string {
  return items
    .map((it) => {
      const targets = [
        `${it.target_host}:${it.target_port}`,
        ...(it.extra_targets ?? []).map((t) => `${t.host}:${t.port}`),
      ].join(',')
      return `${targets}|${it.name}|${it.listen_port}`
    })
    .join('\n')
}

export interface TxtParseResult {
  items: RuleExportItem[]
  errors: string[]
}

/** 解析单个 `host:port`(按最后一个冒号切,兼容 IPv4/主机名;IPv6 目标请用 JSON 导入)。 */
function parseAddr(a: string): TargetDto | null {
  const idx = a.lastIndexOf(':')
  if (idx <= 0 || idx === a.length - 1) return null
  const host = a.slice(0, idx).trim()
  const port = Number(a.slice(idx + 1).trim())
  if (!host || !Number.isInteger(port) || port < 1 || port > 65535) return null
  return { host, port }
}

/** TXT 文本 → RuleExportItem[]。节点/协议由弹窗提供,监听IP 固定 0.0.0.0。
 *  逐行解析,任一行格式错误收进 errors(不进 items)。 */
export function parseTxtToItems(
  txt: string,
  opts: { nodeName: string; protocol: 'tcp' | 'udp' | 'tcp_udp' },
): TxtParseResult {
  const items: RuleExportItem[] = []
  const errors: string[] = []
  txt.split('\n').forEach((raw, i) => {
    const line = raw.trim()
    if (!line) return
    const n = i + 1
    const parts = line.split('|')
    if (parts.length < 3) {
      errors.push(`第 ${n} 行: 格式应为 目标地址|名称|入口端口`)
      return
    }
    const [addrList, name, portStr] = parts
    if (!name.trim()) {
      errors.push(`第 ${n} 行: 名称不能为空`)
      return
    }
    const port = Number(portStr.trim())
    if (!portStr.trim() || !Number.isInteger(port) || port < 1 || port > 65535) {
      errors.push(`第 ${n} 行: 入口端口必须是 1-65535`)
      return
    }
    const addrs = addrList.split(',').map((a) => a.trim()).filter(Boolean)
    if (addrs.length === 0) {
      errors.push(`第 ${n} 行: 目标地址不能为空`)
      return
    }
    const targets: TargetDto[] = []
    let bad = false
    for (const a of addrs) {
      const t = parseAddr(a)
      if (!t) {
        errors.push(`第 ${n} 行: 目标地址 "${a}" 应为 地址:端口`)
        bad = true
        break
      }
      targets.push(t)
    }
    if (bad) return
    const [primary, ...extra] = targets
    items.push({
      name: name.trim(),
      protocol: opts.protocol,
      listen_ip: '0.0.0.0',
      listen_port: port,
      target_host: primary.host,
      target_port: primary.port,
      enabled: true,
      node_name: opts.nodeName,
      tunnel_name: null,
      bandwidth_profile_name: null,
      owner_username: null,
      extra_targets: extra,
      lb_strategy: 'fifo',
    })
  })
  return { items, errors }
}
