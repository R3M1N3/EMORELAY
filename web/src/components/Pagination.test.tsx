// Pagination 渲染 smoke + 关键交互:上一页按钮 disabled 状态、onChangePage 回调。
import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { Pagination } from './Pagination'

describe('Pagination', () => {
  it('renders total / range / current page summary', () => {
    render(
      <Pagination
        page={2}
        pageSize={20}
        total={75}
        onChangePage={() => {}}
      />,
    )
    expect(screen.getByText(/共 75 条/)).toBeInTheDocument()
    // start=21 end=40
    expect(screen.getByText(/显示 21-40/)).toBeInTheDocument()
    // total_pages = ceil(75/20) = 4
    expect(screen.getByText(/2 \/ 4/)).toBeInTheDocument()
  })

  it('disables prev button on first page', () => {
    render(
      <Pagination
        page={1}
        pageSize={20}
        total={50}
        onChangePage={() => {}}
      />,
    )
    const prev = screen.getByRole('button', { name: /上一页/ })
    expect(prev).toBeDisabled()
  })

  it('disables next button on last page', () => {
    render(
      <Pagination
        page={3}
        pageSize={20}
        total={50}
        onChangePage={() => {}}
      />,
    )
    const next = screen.getByRole('button', { name: /下一页/ })
    expect(next).toBeDisabled()
  })

  it('calls onChangePage with next page on next click', () => {
    const onChange = vi.fn()
    render(
      <Pagination page={2} pageSize={20} total={100} onChangePage={onChange} />,
    )
    fireEvent.click(screen.getByRole('button', { name: /下一页/ }))
    expect(onChange).toHaveBeenCalledWith(3)
  })

  it('treats total=0 as 1 page with 0-0 range', () => {
    render(
      <Pagination page={1} pageSize={20} total={0} onChangePage={() => {}} />,
    )
    expect(screen.getByText(/共 0 条/)).toBeInTheDocument()
    expect(screen.getByText(/显示 0-0/)).toBeInTheDocument()
    expect(screen.getByText(/1 \/ 1/)).toBeInTheDocument()
  })
})
