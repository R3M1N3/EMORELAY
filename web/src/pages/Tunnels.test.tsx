import { describe, expect, it, vi, beforeEach } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import Tunnels from './Tunnels'
import { ToastProvider } from '../lib/toast'

vi.mock('../lib/api', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/api')>()
  return {
    ...mod,
    tunnels: {
      list: vi.fn().mockResolvedValue({
        items: [
          { id: 1, name: 'hk-jp', transport: 'tls', status: 'up', hops_count: 2, rules_count: 3, created_at: '2026-06-11 00:00:00', updated_at: '2026-06-11 00:00:00' },
          { id: 2, name: 'hk-jp-us', transport: 'tcp', status: 'degraded', hops_count: 3, rules_count: 0, created_at: '2026-06-11 00:00:00', updated_at: '2026-06-11 00:00:00' },
        ],
        total: 2, page: 1, page_size: 20,
      }),
      create: vi.fn().mockResolvedValue({ id: 9 }),
      del: vi.fn().mockResolvedValue({ ok: true }),
      restart: vi.fn().mockResolvedValue({ ok: true, dispatched: true }),
      get: vi.fn(), update: vi.fn(), status: vi.fn(),
    },
    nodes: {
      ...mod.nodes,
      list: vi.fn().mockResolvedValue({
        items: [
          { id: 11, name: 'hk-1', status: 'online', public_ip: '1.1.1.1' },
          { id: 12, name: 'jp-1', status: 'online', public_ip: '2.2.2.2' },
          { id: 13, name: 'us-1', status: 'online', public_ip: '3.3.3.3' },
        ],
        total: 3, page: 1, page_size: 100,
      }),
    },
  }
})

import { tunnels } from '../lib/api'

function renderPage() {
  return render(
    <ToastProvider>
      <MemoryRouter>
        <Tunnels />
      </MemoryRouter>
    </ToastProvider>,
  )
}

beforeEach(() => vi.clearAllMocks())

describe('Tunnels page', () => {
  it('renders tunnel list with transport, hops and rules count', async () => {
    renderPage()
    expect(await screen.findByText('hk-jp')).toBeInTheDocument()
    expect(screen.getByText('hk-jp-us')).toBeInTheDocument()
  })

  it('creates a tunnel from the chain builder', async () => {
    renderPage()
    await screen.findByText('hk-jp')
    fireEvent.click(screen.getByRole('button', { name: '创建隧道' }))
    // 默认两行节点下拉。
    const selects = await screen.findAllByLabelText(/节点 #/)
    expect(selects).toHaveLength(2)

    fireEvent.change(screen.getByLabelText('隧道名 *'), { target: { value: 't-new' } })
    fireEvent.change(selects[0], { target: { value: '11' } })
    fireEvent.change(selects[1], { target: { value: '12' } })
    fireEvent.click(screen.getByRole('button', { name: '创建' }))

    await waitFor(() =>
      expect(tunnels.create).toHaveBeenCalledWith({
        name: 't-new',
        transport: 'tls',
        node_ids: [11, 12],
      }),
    )
  })

  it('rejects duplicate nodes in the chain before submitting', async () => {
    renderPage()
    await screen.findByText('hk-jp')
    fireEvent.click(screen.getByRole('button', { name: '创建隧道' }))
    const selects = await screen.findAllByLabelText(/节点 #/)
    fireEvent.change(screen.getByLabelText('隧道名 *'), { target: { value: 't-dup' } })
    fireEvent.change(selects[0], { target: { value: '11' } })
    fireEvent.change(selects[1], { target: { value: '11' } })
    fireEvent.click(screen.getByRole('button', { name: '创建' }))
    expect(await screen.findByText(/节点不可重复/)).toBeInTheDocument()
    expect(tunnels.create).not.toHaveBeenCalled()
  })
})
