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

export interface LoginResponse {
  token: string
  user: UserView
}

export interface NodeView {
  id: number
  name: string
  region: string
  public_ip: string
  grpc_endpoint: string
  status: 'online' | 'offline' | 'unknown'
  last_seen_at: string | null
  cpu_usage: number
  memory_usage: number
  load_average: number
  rx_bytes_total: number
  tx_bytes_total: number
  port_pool_min: number
  port_pool_max: number
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
  node_id: number
  name: string
  protocol: 'tcp' | 'udp' | 'tcp_udp'
  listen_ip: string
  listen_port: number
  target_host: string
  target_port: number
  enabled: boolean
  expires_at: string | null
  traffic_limit_bytes: number | null
  bandwidth_limit_mbps: number | null
  rx_bytes: number
  tx_bytes: number
  connection_count: number
  created_at: string
  updated_at: string
}

export interface RuleListResponse {
  items: RuleView[]
  total: number
  page: number
  page_size: number
}

export interface CreateNodeRequest {
  name: string
  region?: string
  public_ip?: string
  grpc_endpoint?: string
  port_pool_min?: number
  port_pool_max?: number
}

export interface CreateNodeResponse {
  node: NodeView
  agent_token: string
}

export type UpdateNodeRequest = Partial<CreateNodeRequest>

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
  listen_port: number
  target_host: string
  target_port: number
  expires_at?: string | null
  traffic_limit_bytes?: number | null
  bandwidth_limit_mbps?: number | null
}

export interface UpdateRuleRequest {
  name?: string
  listen_ip?: string
  listen_port?: number
  target_host?: string
  target_port?: number
  expires_at?: string | null
  traffic_limit_bytes?: number | null
  bandwidth_limit_mbps?: number | null
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

export interface UserDetail {
  id: number
  username: string
  role: 'admin' | 'user'
  created_at: string
  updated_at: string
  rule_count: number
  total_traffic_bytes: number
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
}

export interface UpdateUserRequest {
  password?: string
  role?: 'admin' | 'user'
}

export interface SystemOverview {
  total_nodes: number
  online_nodes: number
  total_rules: number
  enabled_rules: number
  rx_bytes_total: number
  tx_bytes_total: number
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
  me: () => api.get<UserView>('/api/auth/me'),
  logout: () => api.post<{ ok: boolean }>('/api/auth/logout'),
}

export const nodes = {
  list: (q: { page?: number; page_size?: number; sort?: string; order?: 'asc' | 'desc' } = {}) => {
    const sp = new URLSearchParams()
    if (q.page) sp.set('page', String(q.page))
    if (q.page_size) sp.set('page_size', String(q.page_size))
    if (q.sort) sp.set('sort', q.sort)
    if (q.order) sp.set('order', q.order)
    return api.get<NodeListResponse>(`/api/nodes?${sp.toString()}`)
  },
  get: (id: number) => api.get<NodeView>(`/api/nodes/${id}`),
  create: (req: CreateNodeRequest) => api.post<CreateNodeResponse>('/api/nodes', req),
  update: (id: number, req: UpdateNodeRequest) => api.patch<NodeView>(`/api/nodes/${id}`, req),
  del: (id: number) => api.del<{ ok: boolean }>(`/api/nodes/${id}`),
  stats: (id: number) => api.get<NodeStatsResponse>(`/api/nodes/${id}/stats`),
}

export const users = {
  list: (q: { page?: number; page_size?: number } = {}) => {
    const sp = new URLSearchParams()
    if (q.page) sp.set('page', String(q.page))
    if (q.page_size) sp.set('page_size', String(q.page_size))
    return api.get<UserListResponse>(`/api/users?${sp.toString()}`)
  },
  get: (id: number) => api.get<UserDetail>(`/api/users/${id}`),
  create: (req: CreateUserRequest) => api.post<UserDetail>('/api/users', req),
  update: (id: number, req: UpdateUserRequest) => api.patch<UserDetail>(`/api/users/${id}`, req),
  del: (id: number) => api.del<{ ok: boolean }>(`/api/users/${id}`),
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
  del: (id: number) => api.del<{ ok: boolean }>(`/api/rules/${id}`),
  enable: (id: number) => api.post<{ ok: boolean; enabled: boolean }>(`/api/rules/${id}/enable`),
  disable: (id: number) => api.post<{ ok: boolean; enabled: boolean }>(`/api/rules/${id}/disable`),
  restart: (id: number) => api.post<{ ok: boolean; dispatched: boolean }>(`/api/rules/${id}/restart`),
  stats: (id: number) => api.get<RuleStatsResponse>(`/api/rules/${id}/stats`),
  logs: (id: number) => api.get<RuleLogEntry[]>(`/api/rules/${id}/logs`),
}

// ============ 工具 ============

// 后端 SQLite 返回 'YYYY-MM-DD HH:MM:SS' 或 ISO，UI 上截到分钟即可。
export function shortTime(iso: string): string {
  return iso.replace('T', ' ').slice(0, 16)
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
