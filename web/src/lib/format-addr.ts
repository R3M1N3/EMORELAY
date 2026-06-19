// 把 host + port 拼成可直接复制/连接的地址串。
// IPv6 字面量(含 ':')必须用方括号包裹,否则 host:port 与 IPv6 冒号歧义。
// 已是方括号形式([::1])或域名/IPv4 则原样拼接。
export function formatHostPort(host: string, port: number): string {
  const h = host.trim()
  const isIpv6Literal = h.includes(':') && !h.startsWith('[')
  return isIpv6Literal ? `[${h}]:${port}` : `${h}:${port}`
}

// 规则「入口地址」的主机部分:优先节点展示地址(display_address),回落接入地址(public_ip)。
// 这与规则的 listen_ip 无关——listen_ip 是 agent 绑定哪张网卡(通常 0.0.0.0=所有网卡),
// 而入口地址是用户实际连接的主机。node 缺失(已删/未授权)返回空串,调用方自行回落。
// 兼容两种后端视角:admin 拿到 display_address+public_ip 需此处回落;普通用户视角
// display_address 恒空、public_ip 已被后端替换为有效展示地址,同样命中回落分支。
export function nodeEntryHost(
  node: { display_address?: string | null; public_ip?: string | null } | undefined | null,
): string {
  if (!node) return ''
  const disp = node.display_address?.trim()
  if (disp) return disp
  return node.public_ip?.trim() ?? ''
}
