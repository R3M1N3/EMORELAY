// EMORELAY 类型安全 API client。
// 所有 fetch 都过这里，统一处理：Authorization 头、JSON 解析、错误规范化。

const TOKEN_KEY = 'emorelay-token'

export function getToken(): string | null {
  return localStorage.getItem(TOKEN_KEY)
}
export function setToken(token: string): void {
  localStorage.setItem(TOKEN_KEY, token)
}
export function clearToken(): void {
  localStorage.removeItem(TOKEN_KEY)
}

export class ApiError extends Error {
  status: number
  code: string
  constructor(status: number, code: string, message: string) {
    super(message)
    this.status = status
    this.code = code
  }
}

interface ErrorBody {
  error: string
  message: string
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {}
  if (body !== undefined) headers['Content-Type'] = 'application/json'
  const token = getToken()
  if (token) headers['Authorization'] = `Bearer ${token}`

  const res = await fetch(path, {
    method,
    headers,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })

  if (res.status === 204) return undefined as T

  const text = await res.text()
  const json = text ? (JSON.parse(text) as unknown) : null

  if (!res.ok) {
    const err = (json as ErrorBody | null) ?? { error: 'unknown', message: res.statusText }
    if (res.status === 401) clearToken()
    throw new ApiError(res.status, err.error, err.message)
  }
  return json as T
}

export const api = {
  get: <T>(path: string) => request<T>('GET', path),
  post: <T>(path: string, body?: unknown) => request<T>('POST', path, body),
  patch: <T>(path: string, body?: unknown) => request<T>('PATCH', path, body),
  del: <T>(path: string) => request<T>('DELETE', path),
}

// ============ 数据类型 ============

export interface UserView {
  id: number
  username: string
  role: 'admin' | 'user'
}

/** /api/auth/me 扩展视图:UserView + 配额/用量/规则聚合(用户自助概览数据源)。 */
export interface MeView extends UserView {
  expires_at: string | null
  traffic_limit_bytes_30d: number | null
  period_used_bytes_cached: number
  period_used_calculated_at: string | null
  rule_count: number
  total_traffic_bytes: number
  /** true = 强制改密未完成,前端把用户挡在改密页 */
  must_change_password: boolean
}

export interface LoginResponse {
  token: string
  user: UserView
  /** true = 首登强制改密(admin 新建/重置后),前端跳改密页 */
  must_change_password: boolean
}

export interface NodeView {
  id: number
  name: string
  region: string
  /** 接入地址(互联实际使用);普通用户视角已被替换为有效展示地址 */
  public_ip: string
  /** 展示地址(可选,空=回落接入地址);普通用户视角恒为空串 */
  display_address: string
  grpc_endpoint: string
  /** Agent 上报版本(register 落库);普通用户视角恒为空串 */
  agent_version: string
  status: 'online' | 'offline' | 'unknown'
  last_seen_at: string | null
  cpu_usage: number
  memory_usage: number
  load_average: number
  rx_bytes_total: number
  tx_bytes_total: number
  port_pool_min: number
  port_pool_max: number
  /** 协议嗅探阻断位掩码:bit0=http(1) bit1=tls(2) bit2=socks(4);0=不阻断。用户视角恒 0 */
  block_protocols: number
  created_at: string
  updated_at: string
}

export interface NodeListResponse {
  items: NodeView[]
  total: number
  page: number
  page_size: number
}

export interface RuleView {
  id: number
  user_id: number
  /** 归属用户名(admin 列表归属列;用户软删时可能为 null) */
  user_name: string | null
  node_id: number
  name: string
  protocol: 'tcp' | 'udp' | 'tcp_udp'
  listen_ip: string
  listen_port: number
  target_host: string
  target_port: number
  enabled: boolean
  bandwidth_profile_id: number | null
  bandwidth_mbps: number | null
  tunnel_id: number | null
  /** 并发连接上限(仅 TCP);null = 不限 */
  max_connections: number | null
  /** P2 多目标额外目标 + 负载策略 */
  extra_targets: TargetDto[]
  lb_strategy: LbStrategy
  rx_bytes: number
  tx_bytes: number
  connection_count: number
  created_at: string
  updated_at: string
}

export interface TargetDto {
  host: string
  port: number
}

export type LbStrategy = 'fifo' | 'round' | 'rand' | 'hash'

export interface RuleListResponse {
  items: RuleView[]
  total: number
  page: number
  page_size: number
}

/** 逐段诊断:一段链路(源节点 → 目标)的探测结果。 */
export interface SegmentResult {
  label: string
  source_node_id: number
  source_node_name: string
  target: string
  /** 命令是否送达源节点(节点在线) */
  dispatched: boolean
  reachable: boolean
  avg_latency_ms: number
  loss_pct: number
  error: string
}

export interface DiagnoseResponse {
  segments: SegmentResult[]
}

export interface CreateNodeRequest {
  name: string
  region?: string
  public_ip?: string
  /** 展示地址(可选);update 时传 '' 表示清空(回落接入地址) */
  display_address?: string
  grpc_endpoint?: string
  port_pool_min?: number
  port_pool_max?: number
}

export interface CreateNodeResponse {
  node: NodeView
  agent_token: string
  ca_pem: string
  client_cert_pem: string
  client_key_pem: string
}

export type UpdateNodeRequest = Partial<CreateNodeRequest> & {
  /** 协议嗅探阻断位掩码 0-7(bit0=http bit1=tls bit2=socks) */
  block_protocols?: number
}

export interface NodeStatsBucket {
  bucket_at: string
  cpu_usage: number
  memory_usage: number
  load_average: number
  rx_bytes: number
  tx_bytes: number
}

export interface NodeStatsResponse {
  current: {
    status: NodeView['status']
    last_seen_at: string | null
    cpu_usage: number
    memory_usage: number
    load_average: number
    rx_bytes_total: number
    tx_bytes_total: number
  }
  series: NodeStatsBucket[]
}

export interface CreateRuleRequest {
  node_id: number
  name: string
  protocol: RuleView['protocol']
  listen_ip?: string
  listen_port?: number
  target_host: string
  target_port: number
  bandwidth_profile_id?: number | null
  tunnel_id?: number | null
  /** 归属用户:仅 admin 可指定 */
  user_id?: number
  /** 并发连接上限(仅 TCP,admin 管控);不传 = 不限 */
  max_connections?: number
  /** P2 多目标额外目标(空数组 = 单目标)+ 负载策略 */
  extra_targets?: TargetDto[]
  lb_strategy?: LbStrategy
}

export interface UpdateRuleRequest {
  name?: string
  listen_ip?: string
  listen_port?: number
  target_host?: string
  target_port?: number
  /** 0 = 解除关联 */
  bandwidth_profile_id?: number
  /** 0 = 清除上限(admin 管控) */
  max_connections?: number
  /** 给定则全量替换额外目标(空 = 清空);不传 = 不改 */
  extra_targets?: TargetDto[]
  lb_strategy?: LbStrategy
}

export interface RuleStatsBucket {
  bucket_at: string
  rx_bytes: number
  tx_bytes: number
  connection_count: number
  error_count: number
}

export interface RuleStatsResponse {
  current: {
    enabled: boolean
    rx_bytes: number
    tx_bytes: number
    connection_count: number
  }
  series: RuleStatsBucket[]
}

export interface RuleLogEntry {
  id: number
  actor_user_id: number | null
  action: string
  result: string
  error_message: string | null
  created_at: string
}

export interface RuleExportItem {
  name: string
  protocol: 'tcp' | 'udp' | 'tcp_udp'
  listen_ip: string
  listen_port: number
  target_host: string
  target_port: number
  enabled: boolean
  node_name: string
  tunnel_name: string | null
  bandwidth_profile_name: string | null
  /** 归属用户名:导入按用户名匹配回填,匹配不到归导入者(老文件无此字段) */
  owner_username?: string | null
}

export interface ImportItemReport {
  index: number
  action: 'create' | 'skip' | 'overwrite' | 'error'
  reason: string
}

export interface ImportReport {
  dry_run: boolean
  strategy: string
  items: ImportItemReport[]
}

export interface UserDetail {
  id: number
  username: string
  role: 'admin' | 'user'
  created_at: string
  updated_at: string
  rule_count: number
  total_traffic_bytes: number
  expires_at: string | null
  traffic_limit_bytes_30d: number | null
  period_used_bytes_cached: number
  period_used_calculated_at: string | null
  period_remaining_bytes: number | null
  /** 月度重置日 1-31;null = 滚动 30 天 */
  quota_reset_day: number | null
}

export interface UserListResponse {
  items: UserDetail[]
  total: number
  page: number
  page_size: number
}

export interface CreateUserRequest {
  username: string
  password: string
  role: 'admin' | 'user'
  expires_at?: string | null
  traffic_limit_bytes_30d?: number | null
  /** 月度重置日 1-31;0/不传 = 滚动 30 天 */
  quota_reset_day?: number | null
  /** P7 授权(默认拒绝):未传 = 不授权任何节点/隧道 */
  granted_node_ids?: number[]
  granted_tunnel_ids?: number[]
}

export interface UpdateUserRequest {
  password?: string
  role?: 'admin' | 'user'
  /** '' = 清除到期 */
  expires_at?: string
  /** 0 = 清除限额 */
  traffic_limit_bytes_30d?: number
  /** None=不改;0=清除(回滚动);1-31=月度重置日 */
  quota_reset_day?: number
  /** 给定则全量替换该用户授权;不传 = 不改动 */
  granted_node_ids?: number[]
  granted_tunnel_ids?: number[]
}

/** P7: 用户当前授权(编辑回显)。 */
export interface UserGrants {
  granted_node_ids: number[]
  granted_tunnel_ids: number[]
}

/** P7: 节点/隧道详情页反向显示「已授权用户」。 */
export interface GrantedUser {
  id: number
  username: string
}

export interface SystemOverview {
  total_nodes: number
  online_nodes: number
  total_rules: number
  enabled_rules: number
  rx_bytes_total: number
  tx_bytes_total: number
  /** 过去 24h 规则转发流量(rule_stats 口径,区别于节点网卡流量) */
  rx_bytes_24h: number
  tx_bytes_24h: number
}

export interface AuditLogEntry {
  id: number
  actor_user_id: number | null
  actor_ip: string | null
  action: string
  target_type: string | null
  target_id: number | null
  payload: string | null
  result: 'success' | 'failure'
  error_message: string | null
  created_at: string
}

export interface AuditLogListResponse {
  items: AuditLogEntry[]
  total: number
  page: number
  page_size: number
}

export interface SettingsResponse {
  settings: Record<string, string>
}

export interface SecurityInfo {
  jwt_secret_configured: boolean
  jwt_secret_length: number
  jwt_expiry_hours: number
  grpc_tls_enabled: boolean
  grpc_mtls_enabled: boolean
}

// ============ 端点 ============

export const auth = {
  login: (username: string, password: string) =>
    api.post<LoginResponse>('/api/auth/login', { username, password }),
  me: () => api.get<MeView>('/api/auth/me'),
  logout: () => api.post<{ ok: boolean }>('/api/auth/logout'),
  changePassword: (oldPassword: string, newPassword: string) =>
    api.post<{ ok: boolean }>('/api/auth/change-password', {
      old_password: oldPassword,
      new_password: newPassword,
    }),
}

export const nodes = {
  list: (
    q: {
      page?: number
      page_size?: number
      sort?: string
      order?: 'asc' | 'desc'
      search?: string
    } = {},
  ) => {
    const sp = new URLSearchParams()
    if (q.page) sp.set('page', String(q.page))
    if (q.page_size) sp.set('page_size', String(q.page_size))
    if (q.sort) sp.set('sort', q.sort)
    if (q.order) sp.set('order', q.order)
    if (q.search) sp.set('search', q.search)
    return api.get<NodeListResponse>(`/api/nodes?${sp.toString()}`)
  },
  get: (id: number) => api.get<NodeView>(`/api/nodes/${id}`),
  create: (req: CreateNodeRequest) => api.post<CreateNodeResponse>('/api/nodes', req),
  update: (id: number, req: UpdateNodeRequest) => api.patch<NodeView>(`/api/nodes/${id}`, req),
  del: (id: number) => api.del<{ ok: boolean }>(`/api/nodes/${id}`),
  stats: (id: number) => api.get<NodeStatsResponse>(`/api/nodes/${id}/stats`),
  /** admin-only:该节点被授权给哪些用户 */
  grants: (id: number) => api.get<GrantedUser[]>(`/api/nodes/${id}/grants`),
  /** admin-only:向在线节点下发 Agent 一键升级(下载/校验/原子替换/exec 重启) */
  upgradeAgent: (id: number) =>
    api.post<{ ok: boolean; dispatched: boolean; target_version: string }>(
      `/api/nodes/${id}/upgrade-agent`,
    ),
  revokeCredentials: (id: number) =>
    api.post<{ ca_pem: string; client_cert_pem: string; client_key_pem: string }>(
      `/api/nodes/${id}/revoke-credentials`,
    ),
}

export const users = {
  list: (q: { page?: number; page_size?: number; search?: string } = {}) => {
    const sp = new URLSearchParams()
    if (q.page) sp.set('page', String(q.page))
    if (q.page_size) sp.set('page_size', String(q.page_size))
    if (q.search) sp.set('search', q.search)
    return api.get<UserListResponse>(`/api/users?${sp.toString()}`)
  },
  get: (id: number) => api.get<UserDetail>(`/api/users/${id}`),
  create: (req: CreateUserRequest) => api.post<UserDetail>('/api/users', req),
  update: (id: number, req: UpdateUserRequest) => api.patch<UserDetail>(`/api/users/${id}`, req),
  del: (id: number) => api.del<{ ok: boolean }>(`/api/users/${id}`),
  /** admin-only:该用户当前的节点/隧道授权(编辑回显) */
  grants: (id: number) => api.get<UserGrants>(`/api/users/${id}/grants`),
}

export interface BandwidthProfileView {
  id: number
  name: string
  bandwidth_mbps: number
  description: string
  created_at: string
  updated_at: string
}

export interface BandwidthProfileListResponse {
  items: BandwidthProfileView[]
  total: number
  page: number
  page_size: number
}

export const bandwidthProfiles = {
  list: (q: { page?: number; page_size?: number } = {}) => {
    const sp = new URLSearchParams()
    if (q.page) sp.set('page', String(q.page))
    if (q.page_size) sp.set('page_size', String(q.page_size))
    return api.get<BandwidthProfileListResponse>(`/api/bandwidth-profiles?${sp.toString()}`)
  },
  create: (req: { name: string; bandwidth_mbps: number; description?: string }) =>
    api.post<BandwidthProfileView>('/api/bandwidth-profiles', req),
  update: (id: number, req: { name?: string; bandwidth_mbps?: number; description?: string }) =>
    api.patch<BandwidthProfileView>(`/api/bandwidth-profiles/${id}`, req),
  del: (id: number) => api.del<{ ok: boolean }>(`/api/bandwidth-profiles/${id}`),
}

export const system = {
  overview: () => api.get<SystemOverview>('/api/system/overview'),
  security: () => api.get<SecurityInfo>('/api/system/security'),
  auditLogs: (
    q: {
      page?: number
      page_size?: number
      action?: string
      target_type?: string
      result?: 'success' | 'failure'
    } = {},
  ) => {
    const sp = new URLSearchParams()
    if (q.page) sp.set('page', String(q.page))
    if (q.page_size) sp.set('page_size', String(q.page_size))
    if (q.action) sp.set('action', q.action)
    if (q.target_type) sp.set('target_type', q.target_type)
    if (q.result) sp.set('result', q.result)
    return api.get<AuditLogListResponse>(`/api/system/audit-logs?${sp.toString()}`)
  },
  getSettings: () => api.get<SettingsResponse>('/api/system/settings'),
  updateSettings: (settings: Record<string, string>) =>
    api.patch<SettingsResponse>('/api/system/settings', { settings }),
  // 免鉴权:全局 UI 主题(强调色),登录页与普通用户共用。
  uiConfig: () => api.get<{ accent_color: string | null }>('/api/ui-config'),
}

/**
 * 生成节点安装命令字符串(用户复制走)。
 * base URL 取自 window.location.origin —— 生产期需用反代将面板对外 origin 指向 panel-server,
 * 否则脚本里 curl 不到 /install.sh 与 /dist/*。
 * token 一次性,UI 仅在创建节点 / 后续轮换凭据 Modal 内可调用。
 */
export function renderInstallCommand(opts: {
  nodeId: number
  token: string
  caPem?: string
  clientCertPem?: string
  clientKeyPem?: string
}): string {
  const base = window.location.origin
  let cmd = `curl -fsSL ${base}/install.sh?node=${opts.nodeId} | sudo bash -s -- --token=${opts.token}`
  if (opts.caPem && opts.clientCertPem && opts.clientKeyPem) {
    cmd += ` --ca-pem-b64=${btoa(opts.caPem)}`
    cmd += ` --client-cert-pem-b64=${btoa(opts.clientCertPem)}`
    cmd += ` --client-key-pem-b64=${btoa(opts.clientKeyPem)}`
  }
  return cmd
}

export const rules = {
  list: (q: { page?: number; page_size?: number; node_id?: number; protocol?: string; search?: string } = {}) => {
    const sp = new URLSearchParams()
    if (q.page) sp.set('page', String(q.page))
    if (q.page_size) sp.set('page_size', String(q.page_size))
    if (q.node_id) sp.set('node_id', String(q.node_id))
    if (q.protocol) sp.set('protocol', q.protocol)
    if (q.search) sp.set('search', q.search)
    return api.get<RuleListResponse>(`/api/rules?${sp.toString()}`)
  },
  get: (id: number) => api.get<RuleView>(`/api/rules/${id}`),
  create: (req: CreateRuleRequest) => api.post<RuleView>('/api/rules', req),
  update: (id: number, req: UpdateRuleRequest) => api.patch<RuleView>(`/api/rules/${id}`, req),
  /** dispatched=false 表示目标节点离线,规则将由对账在节点恢复后清理 */
  del: (id: number) => api.del<{ ok: boolean; dispatched: boolean }>(`/api/rules/${id}`),
  enable: (id: number) => api.post<{ ok: boolean; enabled: boolean }>(`/api/rules/${id}/enable`),
  disable: (id: number) => api.post<{ ok: boolean; enabled: boolean }>(`/api/rules/${id}/disable`),
  restart: (id: number) => api.post<{ ok: boolean; dispatched: boolean }>(`/api/rules/${id}/restart`),
  stats: (id: number) => api.get<RuleStatsResponse>(`/api/rules/${id}/stats`),
  logs: (id: number) => api.get<RuleLogEntry[]>(`/api/rules/${id}/logs`),
  diagnose: (id: number) => api.post<DiagnoseResponse>(`/api/rules/${id}/diagnose`),
  /** 按当前筛选导出并触发浏览器下载(需带 Authorization,不能用 <a href>)。 */
  exportDownload: async (q: { node_id?: number; tunnel_id?: number } = {}) => {
    const sp = new URLSearchParams()
    if (q.node_id) sp.set('node_id', String(q.node_id))
    if (q.tunnel_id) sp.set('tunnel_id', String(q.tunnel_id))
    const token = getToken()
    const res = await fetch(`/api/rules/export?${sp.toString()}`, {
      headers: token ? { Authorization: `Bearer ${token}` } : {},
    })
    if (!res.ok) {
      const err = (await res.json().catch(() => null)) as { error?: string; message?: string } | null
      if (res.status === 401) clearToken()
      throw new ApiError(res.status, err?.error ?? 'unknown', err?.message ?? res.statusText)
    }
    const blob = await res.blob()
    // 空导出不下载文件,抛给调用方 toast(下载一个 [] 只会困惑)。
    if (blob.size <= 2) {
      throw new ApiError(200, 'empty', '当前范围内没有可导出的规则')
    }
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = 'emorelay-rules-export.json'
    a.click()
    URL.revokeObjectURL(url)
  },
  /** targetNodeId 给定时全部映射到该节点,忽略文件内 node_name(P9)。 */
  importRules: (
    items: RuleExportItem[],
    strategy: 'skip' | 'overwrite',
    dryRun: boolean,
    targetNodeId?: number,
  ) =>
    api.post<ImportReport>(
      `/api/rules/import?strategy=${strategy}&dry_run=${dryRun ? 1 : 0}` +
        (targetNodeId ? `&target_node_id=${targetNodeId}` : ''),
      items,
    ),
}

export interface TunnelView {
  id: number
  name: string
  transport: 'tcp' | 'tls' | 'wss'
  status: 'up' | 'degraded' | 'down' | 'unknown'
  /** 计费倍率(默认 1.0) */
  traffic_ratio: number
  /** 1=单向(计较大方向) 2=双向(rx+tx,默认) */
  billing_mode: 1 | 2
  hops_count: number
  rules_count: number
  created_at: string
  updated_at: string
}

export interface TunnelHopView {
  ordinal: number
  node_id: number
  inter_port: number | null
}

export interface TunnelRuleRef {
  id: number
  name: string
  protocol: string
  listen_port: number
  enabled: boolean
}

export interface TunnelDetailView {
  id: number
  name: string
  transport: TunnelView['transport']
  status: TunnelView['status']
  traffic_ratio: number
  billing_mode: 1 | 2
  hops: TunnelHopView[]
  rules_count: number
  rules: TunnelRuleRef[]
  created_at: string
  updated_at: string
}

export interface TunnelListResponse {
  items: TunnelView[]
  total: number
  page: number
  page_size: number
}

export interface CreateTunnelRequest {
  name: string
  transport: TunnelView['transport']
  node_ids: number[]
  traffic_ratio?: number
  billing_mode?: 1 | 2
}

export const tunnels = {
  list: (q: { page?: number; page_size?: number } = {}) => {
    const sp = new URLSearchParams()
    if (q.page) sp.set('page', String(q.page))
    if (q.page_size) sp.set('page_size', String(q.page_size))
    return api.get<TunnelListResponse>(`/api/tunnels?${sp.toString()}`)
  },
  get: (id: number) => api.get<TunnelDetailView>(`/api/tunnels/${id}`),
  create: (req: CreateTunnelRequest) => api.post<{ id: number }>('/api/tunnels', req),
  update: (
    id: number,
    req: { name?: string; traffic_ratio?: number; billing_mode?: 1 | 2 },
  ) => api.patch<TunnelView>(`/api/tunnels/${id}`, req),
  del: (id: number) => api.del<{ ok: boolean }>(`/api/tunnels/${id}`),
  restart: (id: number) => api.post<{ ok: boolean; dispatched: boolean }>(`/api/tunnels/${id}/restart`),
  diagnose: (id: number) => api.post<DiagnoseResponse>(`/api/tunnels/${id}/diagnose`),
  status: (id: number) => api.get<{ id: number; status: TunnelView['status'] }>(`/api/tunnels/${id}/status`),
  /** admin-only:该隧道被授权给哪些用户 */
  grants: (id: number) => api.get<GrantedUser[]>(`/api/tunnels/${id}/grants`),
}

// ============ 工具 ============

// 后端时间均为 UTC('YYYY-MM-DD HH:MM:SS' 或 ISO,无时区标记)。
// 解析为 UTC 后按浏览器本地时区显示到分钟——评审发现 UI 直显 UTC 导致
// 「最后心跳 03:48」实际是本地 11:48 的困惑。解析失败回退原样截断。
export function shortTime(iso: string): string {
  const normalized = iso.includes('T') ? iso : iso.replace(' ', 'T')
  const withZone = /Z|[+-]\d{2}:?\d{2}$/.test(normalized) ? normalized : normalized + 'Z'
  const d = new Date(withZone)
  if (Number.isNaN(d.getTime())) return iso.replace('T', ' ').slice(0, 16)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
}

// 状态文案统一中文(此前 online/down 等英文裸串与中文页面混排)。
// 放 api.ts 工具区:ui.tsx 受 react-refresh 限制只能导出组件。
export function statusLabel(s: string): string {
  const map: Record<string, string> = {
    online: '在线',
    offline: '离线',
    unknown: '未知',
    up: '正常',
    degraded: '降级',
    down: '中断',
  }
  return map[s] ?? s
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(2)} ${units[i]}`
}
