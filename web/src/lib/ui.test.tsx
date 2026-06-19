import { describe, it, expect, beforeEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { useState } from 'react'
import { Modal, ErrorBox } from './ui'

// 覆写 matchMedia 模拟桌面(精确指针 matches=true)或触屏(matches=false)。
// setup.ts 默认桩为 matches=false;Modal 挂载聚焦目标依赖它,故各用例显式设定。
function setFinePointer(fine: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: fine,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia
}

describe('Modal 焦点管理', () => {
  beforeEach(() => setFinePointer(false))

  it('桌面端挂载后自动聚焦首个输入框', () => {
    setFinePointer(true)
    render(
      <Modal onClose={() => {}} title="编辑规则">
        <input aria-label="名称" />
        <input aria-label="端口" />
      </Modal>,
    )
    expect(screen.getByLabelText('名称')).toHaveFocus()
  })

  it('无输入框的确认弹窗聚焦对话框容器,不抢焦危险按钮', () => {
    setFinePointer(true)
    render(
      <Modal onClose={() => {}} title="删除确认">
        <button>确认删除</button>
      </Modal>,
    )
    expect(screen.getByRole('dialog')).toHaveFocus()
    expect(screen.getByText('确认删除')).not.toHaveFocus()
  })

  it('Esc 关闭并把焦点归还触发元素', () => {
    setFinePointer(true)
    function Harness() {
      const [open, setOpen] = useState(false)
      return (
        <>
          <button onClick={() => setOpen(true)}>打开</button>
          {open && (
            <Modal onClose={() => setOpen(false)} title="编辑">
              <input aria-label="字段" />
            </Modal>
          )}
        </>
      )
    }
    render(<Harness />)
    const trigger = screen.getByText('打开')
    trigger.focus()
    fireEvent.click(trigger)
    expect(screen.getByLabelText('字段')).toHaveFocus()
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByLabelText('字段')).toBeNull()
    expect(trigger).toHaveFocus()
  })

  it('开启期父组件重渲染不夺走当前输入焦点(B1 回归)', () => {
    setFinePointer(true)
    function Harness() {
      const [n, setN] = useState(0)
      return (
        <Modal onClose={() => {}} title="编辑">
          <input aria-label="第一" />
          <input aria-label="第二" />
          <button onClick={() => setN(n + 1)}>bump {n}</button>
        </Modal>
      )
    }
    render(<Harness />)
    const second = screen.getByLabelText('第二')
    second.focus()
    expect(second).toHaveFocus()
    // 触发父重渲染(模拟删除置 busy 等开启期状态变更)。
    fireEvent.click(screen.getByText(/bump/))
    // 修复前:effect 因 onClose 引用变更重挂,setup 把焦点拉回「第一」;修复后焦点稳定。
    expect(second).toHaveFocus()
  })
})

describe('ErrorBox', () => {
  it('以 role="alert" 渲染错误信息,无 onRetry 时不显示重试', () => {
    render(<ErrorBox message="加载失败" />)
    expect(screen.getByRole('alert')).toHaveTextContent('加载失败')
    expect(screen.queryByText('重试')).toBeNull()
  })

  it('提供 onRetry 时显示重试按钮并在点击时调用一次', () => {
    let called = 0
    render(<ErrorBox message="加载失败" onRetry={() => { called++ }} />)
    fireEvent.click(screen.getByText('重试'))
    expect(called).toBe(1)
  })
})
