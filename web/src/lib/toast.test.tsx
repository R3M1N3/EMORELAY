import { describe, it, expect, vi } from 'vitest'
import { render, screen, act, fireEvent } from '@testing-library/react'
import { ToastProvider } from './toast'
import { useToast } from './use-toast'

function Trigger() {
  const toast = useToast()
  return (
    <>
      <button onClick={() => toast.success('saved')}>ok</button>
      <button onClick={() => toast.error('boom')}>fail</button>
    </>
  )
}

describe('Toast', () => {
  it('renders success and error toasts in fixed container', () => {
    render(
      <ToastProvider>
        <Trigger />
      </ToastProvider>,
    )
    fireEvent.click(screen.getByText('ok'))
    expect(screen.getByText('saved')).toBeInTheDocument()
    fireEvent.click(screen.getByText('fail'))
    expect(screen.getByText('boom')).toBeInTheDocument()
  })

  it('auto-dismisses after 4 seconds', () => {
    vi.useFakeTimers()
    try {
      render(
        <ToastProvider>
          <Trigger />
        </ToastProvider>,
      )
      fireEvent.click(screen.getByText('ok'))
      expect(screen.getByText('saved')).toBeInTheDocument()
      act(() => {
        vi.advanceTimersByTime(4500)
      })
      expect(screen.queryByText('saved')).toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })

  it('useToast throws when used outside provider', () => {
    expect(() => render(<Trigger />)).toThrow(/ToastProvider/)
  })
})
