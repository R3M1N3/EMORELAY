import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { act, fireEvent, render, screen } from '@testing-library/react'
import { Link, MemoryRouter, Route, Routes } from 'react-router-dom'
import NodeDetail from './NodeDetail'
import { ToastProvider } from '../lib/toast'

// 协议阻断乐观值在途守卫回归测试:切换后恰在 update 提交前发出的 30s 周期 GET 仍返回旧 mask,
// 整页替换 node 时不得把开关「回弹」(乐观值覆盖旧快照,服务端确认后回归权威)。
//
// vi.mock 工厂被提升到文件顶部,不能引用模块级变量;共享可变状态/工厂函数走 vi.hoisted。
const h = vi.hoisted(() => {
  // 基础 node 工厂:仅 block_protocols 在用例间变化,其余字段填默认值满足 NodeView。
  function makeNode(block: number) {
    return {
      id: 7,
      name: 'hk-relay',
      region: 'hk',
      public_ip: '1.2.3.4',
      display_address: '',
      grpc_endpoint: '',
      agent_version: '0.3.0',
      status: 'online' as const,
      last_seen_at: '2026-06-22 00:00:00',
      cpu_usage: 0,
      memory_usage: 0,
      load_average: 0,
      rx_bytes_total: 0,
      tx_bytes_total: 0,
      port_pool_min: 30000,
      port_pool_max: 31000,
      block_protocols: block,
      created_at: '2026-06-01 00:00:00',
      updated_at: '2026-06-01 00:00:00',
    }
  }
  const STATS = {
    current: {
      status: 'online' as const,
      last_seen_at: '2026-06-22 00:00:00',
      cpu_usage: 0,
      memory_usage: 0,
      load_average: 0,
      rx_bytes_total: 0,
      tx_bytes_total: 0,
    },
    series: [] as never[],
  }
  // 周期刷新(第 2 次起)的 nodes.get 返回值:由用例在「toggle 后、刷新前」设定。
  // staleGet=旧值 0 模拟 GET 早于 update 落库的窄窗口;=2 模拟服务端已确认。
  // nodes.get 的脚本化返回:
  //  - queue 非空时,本次 GET 消费队首 mask(模拟「某一拍周期 GET 返回特定 mask」,与 id 无关);
  //  - 否则按 maskById[id] 返回该节点的权威 mask(默认 0)。
  // 由用例精确编排「首载→toggle→刷新」各拍的返回值。
  const state = { maskById: {} as Record<number, number>, queue: [] as number[] }
  return { makeNode, STATS, state }
})

vi.mock('../lib/api', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/api')>()
  return {
    ...mod,
    nodes: {
      ...mod.nodes,
      get: vi.fn().mockImplementation((id: number) => {
        const mask = h.state.queue.length > 0 ? h.state.queue.shift()! : (h.state.maskById[id] ?? 0)
        return Promise.resolve({ ...h.makeNode(mask), id })
      }),
      stats: vi.fn().mockResolvedValue(h.STATS),
      grants: vi.fn().mockResolvedValue([]),
      update: vi.fn().mockResolvedValue({ ok: true }),
    },
  }
})

import { nodes } from '../lib/api'

function renderPage() {
  return render(
    <ToastProvider>
      <MemoryRouter initialEntries={['/nodes/7']}>
        {/* 节点切换链接:路由复用同一 NodeDetail 实例(无 key),复现跨节点 ref 泄漏场景。 */}
        <Link to="/nodes/9">go-node-9</Link>
        <Routes>
          <Route path="/nodes/:id" element={<NodeDetail />} />
        </Routes>
      </MemoryRouter>
    </ToastProvider>,
  )
}

function tlsBox() {
  return screen.getByLabelText('阻断 TLS', { exact: false }) as HTMLInputElement
}

function socksBox() {
  return screen.getByLabelText('阻断 SOCKS', { exact: false }) as HTMLInputElement
}

beforeEach(() => {
  vi.clearAllMocks()
  h.state.maskById = {}
  h.state.queue = []
  // 在 render 前装 fake timers,使 useAutoRefresh 的 setInterval 受控(render 后再装控不到已建的 interval)。
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

// 推进微任务直到首载完成(TLS 复选框出现);fake timers 下用 advanceTimersByTimeAsync 冲刷 promise。
async function settleInitialLoad() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0)
  })
}

describe('NodeDetail 协议阻断乐观值在途守卫', () => {
  it('周期刷新返回旧 mask 时不回弹乐观开关', async () => {
    renderPage()
    await settleInitialLoad()
    // 首载:TLS 复选框未勾选(mask=0)。
    expect(tlsBox().checked).toBe(false)

    // 切换 TLS(bit=2)→ update 调用 + 乐观勾选。
    await act(async () => {
      fireEvent.click(tlsBox())
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(nodes.update).toHaveBeenCalledWith(7, { block_protocols: 2 })
    expect(tlsBox().checked).toBe(true)

    // 模拟「GET 早于 update 落库」:下一拍周期 GET 仍返回旧 mask=0。
    // 整页替换 node 后,乐观守卫必须用 2 覆盖旧快照 → 开关保持勾选,不回弹。
    h.state.queue = [0]
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000)
    })
    expect(nodes.get).toHaveBeenCalledTimes(2)
    expect(tlsBox().checked).toBe(true)
  })

  it('服务端确认后守卫清空,后续外部权威变更不再被乐观值遮蔽', async () => {
    renderPage()
    await settleInitialLoad()
    await act(async () => {
      fireEvent.click(tlsBox())
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(nodes.update).toHaveBeenCalledWith(7, { block_protocols: 2 })
    expect(tlsBox().checked).toBe(true)

    // 第 1 次刷新返回已落库的 mask=2(服务端确认)→ 守卫必须清空。
    h.state.queue = [2]
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000)
    })
    expect(tlsBox().checked).toBe(true)
    expect(socksBox().checked).toBe(false)

    // 第 2 次刷新返回外部改动后的新权威 mask=4(仅 SOCKS)。
    // 守卫若已清空,UI 必须回归权威:TLS 关、SOCKS 开;若守卫粘住乐观 2,则会错误维持 TLS 勾选。
    h.state.queue = [4]
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000)
    })
    expect(tlsBox().checked).toBe(false)
    expect(socksBox().checked).toBe(true)
  })

  it('切换到另一节点时丢弃上一节点的在途乐观值(不跨节点泄漏)', async () => {
    // 节点 9 的真实权威态 = 不阻断(mask 0)。
    h.state.maskById = { 9: 0 }
    renderPage()
    await settleInitialLoad()

    // 在节点 7 上乐观勾选 TLS(mask=2),但不触发任何 GET(本地 setState)→ ref 仍持有 2。
    await act(async () => {
      fireEvent.click(tlsBox())
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(tlsBox().checked).toBe(true)

    // 立即导航到节点 9(同一 NodeDetail 实例被复用,ref 不随卸载清空)。
    await act(async () => {
      fireEvent.click(screen.getByText('go-node-9'))
      await vi.advanceTimersByTimeAsync(0)
    })

    // 节点 9 GET 返回 mask=0。若上一节点的乐观值 2 泄漏,TLS 会被错误勾选并因 GET≠2 永久套用。
    // 修复后:切节点即清守卫 → 节点 9 的 TLS 必须为未勾选(权威 0)。
    expect(tlsBox().checked).toBe(false)
  })
})
