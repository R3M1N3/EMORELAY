// 把 host + port 拼成可直接复制/连接的地址串。
// IPv6 字面量(含 ':')必须用方括号包裹,否则 host:port 与 IPv6 冒号歧义。
// 已是方括号形式([::1])或域名/IPv4 则原样拼接。
export function formatHostPort(host: string, port: number): string {
  const h = host.trim()
  const isIpv6Literal = h.includes(':') && !h.startsWith('[')
  return isIpv6Literal ? `[${h}]:${port}` : `${h}:${port}`
}
