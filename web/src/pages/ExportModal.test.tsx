import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { ToastProvider } from '../lib/toast'

vi.mock('../lib/api', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/api')>()
  return {
    ...mod,
    rules: {
      ...mod.rules,
      exportFetch: vi.fn().mockResolvedValue([
        {
          name: 'r1', protocol: 'tcp', listen_ip: '0.0.0.0', listen_port: 100,
          target_host: '1.1.1.1', target_port: 80, enabled: true, node_name: 'n',
          tunnel_name: null, bandwidth_profile_name: null, extra_targets: [], lb_strategy: 'fifo',
        },
      ]),
    },
  }
})

import { ExportModal } from './Rules'
import { rules } from '../lib/api'

beforeEach(() => vi.clearAllMocks())

function renderExport() {
  return render(
    <ToastProvider>
      <ExportModal nodeList={[] as never} tunnelList={[] as never} onClose={() => {}} />
    </ToastProvider>,
  )
}

describe('ExportModal', () => {
  it('生成后 JSON 美化展示,可切 TXT 客户端重格式化', async () => {
    renderExport()
    fireEvent.click(screen.getByRole('button', { name: '生成' }))
    await waitFor(() => expect(rules.exportFetch).toHaveBeenCalledWith({}))
    const ta = (await screen.findByLabelText('导出内容')) as HTMLTextAreaElement
    await waitFor(() => expect(ta.value).toContain('"name": "r1"'))
    fireEvent.click(screen.getByRole('radio', { name: 'TXT' }))
    await waitFor(() => expect(ta.value).toBe('1.1.1.1:80|r1|100'))
  })
})
