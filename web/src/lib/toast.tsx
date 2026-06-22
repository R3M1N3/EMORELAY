// Toast Provider + 右上角容器。Context 定义在 ./toast-context.ts;hook 定义在 ./use-toast.ts。
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { ToastContext, type ToastApi, type ToastKind } from './toast-context'

interface ToastItem {
  id: number
  kind: ToastKind
  message: string
}

const AUTO_DISMISS_MS = 4000

// 模块级递增 id 计数器,保证同一会话内 id 严格唯一
// (Date.now()+Math.random() 在极速连点理论上仍可能撞)。
let _nextId = 0
function nextId(): number {
  return ++_nextId
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([])
  const timers = useRef<ReturnType<typeof setTimeout>[]>([])

  const push = useCallback((kind: ToastKind, message: string) => {
    const id = nextId()
    setItems((prev) => [...prev, { id, kind, message }])
    const t = setTimeout(() => {
      setItems((prev) => prev.filter((it) => it.id !== id))
    }, AUTO_DISMISS_MS)
    timers.current.push(t)
  }, [])

  useEffect(() => {
    const timersRef = timers
    return () => {
      timersRef.current.forEach(clearTimeout)
      timersRef.current = []
    }
  }, [])

  const api = useMemo<ToastApi>(
    () => ({
      success: (m) => push('success', m),
      error: (m) => push('error', m),
      info: (m) => push('info', m),
    }),
    [push],
  )

  const renderToast = (it: ToastItem) => (
    <div
      key={it.id}
      className={`rounded-lg border px-3 py-2 text-sm backdrop-blur shadow-lg
        animate-[slide-in_0.18s_ease-out] ${kindCls(it.kind)}`}
    >
      {it.message}
    </div>
  )

  return (
    <ToastContext.Provider value={api}>
      {children}
      <div className="fixed top-3 right-3 z-50 flex flex-col gap-2 max-w-sm">
        {/* 错误用 assertive 抢播：删除/重启等破坏性操作失败需即时知晓,不被 polite 队列延迟或吞掉;
            成功/信息保持 polite 不打扰。拆两个 live region 实现分级(WCAG / 读屏可靠性)。 */}
        <div role="alert" aria-live="assertive" className="flex flex-col gap-2 empty:hidden">
          {items.filter((it) => it.kind === 'error').map(renderToast)}
        </div>
        <div role="status" aria-live="polite" className="flex flex-col gap-2 empty:hidden">
          {items.filter((it) => it.kind !== 'error').map(renderToast)}
        </div>
      </div>
    </ToastContext.Provider>
  )
}

function kindCls(k: ToastKind): string {
  switch (k) {
    case 'success':
      return 'border-emerald-500/40 bg-emerald-500/15 text-emerald-100'
    case 'error':
      return 'border-red-500/40 bg-red-500/15 text-red-100'
    case 'info':
      return 'border-[var(--line)] bg-[var(--glass)] text-zinc-100'
  }
}
