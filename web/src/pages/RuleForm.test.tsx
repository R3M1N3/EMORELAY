import { describe, expect, it, vi, beforeEach } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { RuleForm } from './Rules'

vi.mock('../lib/api', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/api')>()
  return {
    ...mod,
    rules: { ...mod.rules, create: vi.fn().mockResolvedValue({ id: 1 }) },
    tunnels: {
      ...mod.tunnels,
      get: vi.fn().mockResolvedValue({
        id: 5, name: 'hk-jp', transport: 'tls', status: 'up',
        hops: [
          { ordinal: 0, node_id: 12, inter_port: null },
          { ordinal: 1, node_id: 11, inter_port: 30001 },
        ],
        rules_count: 0, rules: [],
        created_at: '', updated_at: '',
      }),
    },
  }
})

import { rules, tunnels } from '../lib/api'

const nodeList = [
  { id: 11, name: 'us-1', port_pool_min: 30000, port_pool_max: 31000 },
  { id: 12, name: 'hk-1', port_pool_min: 30000, port_pool_max: 31000 },
] as never[]
const tunnelList = [
  { id: 5, name: 'hk-jp', transport: 'tls', status: 'up', hops_count: 2, rules_count: 0, created_at: '', updated_at: '' },
] as never[]

beforeEach(() => vi.clearAllMocks())

describe('RuleForm tunnel association', () => {
  it('selecting a tunnel locks node select to the entry node and submits tunnel_id', async () => {
    render(
      <RuleForm
        mode="create"
        nodeList={nodeList}
        profiles={[]}
        tunnelList={tunnelList}
        onCancel={() => {}}
        onSuccess={() => {}}
      />,
    )
    fireEvent.change(screen.getByLabelText('关联隧道'), { target: { value: '5' } })
    await waitFor(() => expect(tunnels.get).toHaveBeenCalledWith(5))

    const nodeSelect = screen.getByLabelText('节点 *') as HTMLSelectElement
    await waitFor(() => expect(nodeSelect.value).toBe('12'))
    expect(nodeSelect).toBeDisabled()

    fireEvent.change(screen.getByLabelText('规则名 *'), { target: { value: 'r1' } })
    fireEvent.change(screen.getByLabelText('目标主机 *'), { target: { value: '10.0.0.1' } })
    fireEvent.change(screen.getByLabelText('目标端口 *'), { target: { value: '80' } })
    fireEvent.click(screen.getByRole('button', { name: '创建' }))

    await waitFor(() =>
      expect(rules.create).toHaveBeenCalledWith(
        expect.objectContaining({ node_id: 12, tunnel_id: 5 }),
      ),
    )
  })

  it('without tunnel the node select stays editable and tunnel_id is null', async () => {
    render(
      <RuleForm
        mode="create"
        nodeList={nodeList}
        profiles={[]}
        tunnelList={tunnelList}
        onCancel={() => {}}
        onSuccess={() => {}}
      />,
    )
    expect(screen.getByLabelText('节点 *')).not.toBeDisabled()
    fireEvent.change(screen.getByLabelText('规则名 *'), { target: { value: 'r2' } })
    fireEvent.change(screen.getByLabelText('目标主机 *'), { target: { value: '10.0.0.1' } })
    fireEvent.change(screen.getByLabelText('目标端口 *'), { target: { value: '80' } })
    fireEvent.click(screen.getByRole('button', { name: '创建' }))
    await waitFor(() =>
      expect(rules.create).toHaveBeenCalledWith(
        expect.objectContaining({ tunnel_id: null }),
      ),
    )
  })
})
