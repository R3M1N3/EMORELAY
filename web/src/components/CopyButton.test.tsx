import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { CopyButton } from './CopyButton'
import { ToastProvider } from '../lib/toast'

function renderCopy(value: string) {
  return render(
    <ToastProvider>
      <CopyButton value={value} label="复制监听地址" />
    </ToastProvider>,
  )
}

describe('CopyButton', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it('点击把 value 写入剪贴板并显示 ✓ 与成功 toast', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('navigator', { clipboard: { writeText } })

    renderCopy('1.2.3.4:8080')
    fireEvent.click(screen.getByRole('button', { name: '复制监听地址' }))

    expect(writeText).toHaveBeenCalledWith('1.2.3.4:8080')
    await waitFor(() => expect(screen.getByText('已复制到剪贴板')).toBeInTheDocument())
    await waitFor(() => expect(screen.getByText('✓')).toBeInTheDocument())
  })

  it('剪贴板不可用时降级为错误 toast', async () => {
    vi.stubGlobal('navigator', {})

    renderCopy('1.2.3.4:8080')
    fireEvent.click(screen.getByRole('button', { name: '复制监听地址' }))

    await waitFor(() =>
      expect(screen.getByText('复制失败，请手动选择文本复制')).toBeInTheDocument(),
    )
  })
})
