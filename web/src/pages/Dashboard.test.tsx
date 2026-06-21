import { describe, expect, it, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import Dashboard from './Dashboard'
import { formatBytes } from '../lib/api'

// Dashboard 按角色分流(admin → AdminDashboard);提供 admin user 走全局概览。
vi.mock('../lib/use-auth', () => ({
  useAuth: () => ({ user: { id: 1, username: 'admin', role: 'admin' } }),
}))

vi.mock('../lib/api', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/api')>()
  return {
    ...mod,
    system: {
      ...mod.system,
      // 权威全局总量:节点 150 / 在线 120 / 规则 300 / 启用 280 / 连接 4242 /
      // 规则转发累计 5e9+3e9(规则口径,非节点网卡 9e9+8e9)。
      overview: vi.fn().mockResolvedValue({
        total_nodes: 150, online_nodes: 120, total_rules: 300, enabled_rules: 280,
        total_connections: 4242,
        rule_rx_bytes_total: 5_000_000_000, rule_tx_bytes_total: 3_000_000_000,
        rx_bytes_total: 9_000_000_000, tx_bytes_total: 8_000_000_000,
        rx_bytes_24h: 1000, tx_bytes_24h: 2000,
      }),
      auditLogs: vi.fn().mockResolvedValue({ items: [], total: 0, page: 1, page_size: 10 }),
    },
    nodes: {
      ...mod.nodes,
      // 列表受 page_size:100 封顶,只返回 1 条 —— 不能作为「总节点数」来源。
      list: vi.fn().mockResolvedValue({
        items: [{ id: 1, name: 'n1', status: 'online', public_ip: '1.1.1.1', region: '', cpu_usage: 0, memory_usage: 0, load_average: 0 }],
        total: 150, page: 1, page_size: 100,
      }),
    },
    rules: {
      ...mod.rules,
      list: vi.fn().mockResolvedValue({
        items: [{ id: 1, name: 'r1', enabled: true, rx_bytes: 1, tx_bytes: 1, connection_count: 1 }],
        total: 300, page: 1, page_size: 100,
      }),
    },
  }
})

function renderPage() {
  return render(
    <MemoryRouter>
      <Dashboard />
    </MemoryRouter>,
  )
}

beforeEach(() => vi.clearAllMocks())

describe('AdminDashboard 概览统计卡', () => {
  it('用 system.overview 权威总量,而非受 100 行分页封顶的客户端聚合', async () => {
    renderPage()
    // 列表只返回 1 条 node/rule,但 overview 报权威总量。统计卡必须显示权威值,
    // 否则规模 > 100 时严重少算。
    expect(await screen.findByText('150')).toBeInTheDocument() // 总节点数
    expect(screen.getByText('300')).toBeInTheDocument() // 转发规则
    expect(screen.getByText('4242')).toBeInTheDocument() // 总连接数
    expect(screen.getByText('120 在线')).toBeInTheDocument() // 在线数 hint
    expect(screen.getByText('280 启用')).toBeInTheDocument() // 启用数 hint
    // 总转发流量用规则口径权威累计(5e9+3e9),而非 list reduce 的 2 B。
    expect(screen.getByText(formatBytes(8_000_000_000))).toBeInTheDocument()
  })
})
