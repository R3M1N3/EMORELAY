import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { ToastProvider } from '../lib/toast'
import { ImportModal } from './Rules'

const nodeList = [{ id: 7, name: 'hk-1', port_pool_min: 20000, port_pool_max: 29999 }] as never[]

function renderModal(onSubmit: (items: unknown[]) => void) {
  return render(
    <ToastProvider>
      <ImportModal nodeList={nodeList} onClose={() => {}} onSubmit={onSubmit as never} />
    </ToastProvider>,
  )
}

beforeEach(() => vi.clearAllMocks())

describe('ImportModal', () => {
  it('JSON 数组直接提交解析结果', () => {
    const onSubmit = vi.fn()
    renderModal(onSubmit)
    const json = JSON.stringify([
      { name: 'a', protocol: 'tcp', listen_ip: '0.0.0.0', listen_port: 1, target_host: '1.1.1.1', target_port: 2, enabled: true, node_name: 'hk-1', tunnel_name: null, bandwidth_profile_name: null },
    ])
    fireEvent.change(screen.getByLabelText('导入内容'), { target: { value: json } })
    fireEvent.click(screen.getByRole('button', { name: '预览' }))
    expect(onSubmit).toHaveBeenCalledTimes(1)
    expect(onSubmit.mock.calls[0][0][0].name).toBe('a')
  })

  it('TXT 用所选节点+协议转成 items', () => {
    const onSubmit = vi.fn()
    renderModal(onSubmit)
    fireEvent.change(screen.getByLabelText('导入内容'), { target: { value: '1.1.1.1:80,2.2.2.2:81|r1|20000' } })
    fireEvent.change(screen.getByLabelText('目标节点'), { target: { value: '7' } })
    fireEvent.click(screen.getByRole('button', { name: '预览' }))
    expect(onSubmit).toHaveBeenCalledTimes(1)
    const item = onSubmit.mock.calls[0][0][0]
    expect(item).toMatchObject({
      name: 'r1', protocol: 'tcp_udp', listen_ip: '0.0.0.0', listen_port: 20000,
      target_host: '1.1.1.1', target_port: 80, node_name: 'hk-1',
    })
    expect(item.extra_targets).toEqual([{ host: '2.2.2.2', port: 81 }])
  })

  it('TXT 无节点时报错不提交', () => {
    const onSubmit = vi.fn()
    renderModal(onSubmit)
    fireEvent.change(screen.getByLabelText('导入内容'), { target: { value: '1.1.1.1:80|r1|20000' } })
    fireEvent.click(screen.getByRole('button', { name: '预览' }))
    expect(onSubmit).not.toHaveBeenCalled()
  })

  it('TXT 格式错误显示行级错误', () => {
    const onSubmit = vi.fn()
    renderModal(onSubmit)
    fireEvent.change(screen.getByLabelText('导入内容'), { target: { value: '坏行没有分隔' } })
    fireEvent.change(screen.getByLabelText('目标节点'), { target: { value: '7' } })
    fireEvent.click(screen.getByRole('button', { name: '预览' }))
    expect(onSubmit).not.toHaveBeenCalled()
    // 行级错误框以 role=alert 展示(ToastProvider 另有一个常驻空 alert 容器,故按错误文本定位)。
    const alert = screen.getByText(/第 1 行/).closest('[role="alert"]')
    expect(alert).toBeInTheDocument()
  })
})
