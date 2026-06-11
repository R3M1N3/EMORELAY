import { describe, expect, it, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import TunnelDetail from './TunnelDetail'
import { ToastProvider } from '../lib/toast'

vi.mock('../lib/api', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/api')>()
  return {
    ...mod,
    tunnels: {
      ...mod.tunnels,
      get: vi.fn().mockResolvedValue({
        id: 5, name: 'hk-jp-us', transport: 'tls', status: 'up',
        hops: [
          { ordinal: 0, node_id: 11, inter_port: null },
          { ordinal: 1, node_id: 12, inter_port: 30001 },
          { ordinal: 2, node_id: 13, inter_port: 30002 },
        ],
        rules_count: 1,
        rules: [{ id: 77, name: 'r-game', protocol: 'tcp', listen_port: 20000, enabled: true }],
        created_at: '2026-06-11 00:00:00', updated_at: '2026-06-11 00:00:00',
      }),
      restart: vi.fn().mockResolvedValue({ ok: true, dispatched: true }),
    },
    nodes: {
      ...mod.nodes,
      list: vi.fn().mockResolvedValue({
        items: [
          { id: 11, name: 'hk-1' }, { id: 12, name: 'jp-1' }, { id: 13, name: 'us-1' },
        ],
        total: 3, page: 1, page_size: 100,
      }),
    },
  }
})

function renderPage() {
  return render(
    <ToastProvider>
      <MemoryRouter initialEntries={['/tunnels/5']}>
        <Routes>
          <Route path="/tunnels/:id" element={<TunnelDetail />} />
        </Routes>
      </MemoryRouter>
    </ToastProvider>,
  )
}

describe('TunnelDetail page', () => {
  it('renders hop chain with roles and node names', async () => {
    renderPage()
    expect(await screen.findByText('hk-jp-us')).toBeInTheDocument()
    expect(screen.getByText('Entry')).toBeInTheDocument()
    expect(screen.getByText('Mid')).toBeInTheDocument()
    expect(screen.getByText('Exit')).toBeInTheDocument()
    expect(screen.getByText('hk-1')).toBeInTheDocument()
    expect(screen.getByText('jp-1')).toBeInTheDocument()
    expect(screen.getByText('30001')).toBeInTheDocument()
  })

  it('renders associated rules', async () => {
    renderPage()
    expect(await screen.findByText('r-game')).toBeInTheDocument()
    expect(screen.getByText('20000')).toBeInTheDocument()
  })
})
