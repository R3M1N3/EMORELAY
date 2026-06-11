import { useEffect, type ReactNode } from 'react'

// 全站背景：极光流体 + 网格 + 颗粒(样式见 index.css,颜色随 --ui-accent 联动)。
// 登录页与受保护壳层共用;纯装饰,对 AT 隐藏。
export function Backdrop() {
  return (
    <>
      <div className="aurora" aria-hidden>
        <i /><i /><i />
      </div>
      <div className="grid-bg" aria-hidden />
      <div className="grain" aria-hidden />
    </>
  )
}

// 通用模态：暗色毛玻璃壳子，点击遮罩或 Esc 关闭。
// 节点/规则页的所有弹窗都基于它，避免每页各自实现一遍布局。
export function Modal({
  onClose,
  title,
  children,
  size = 'md',
}: {
  onClose: () => void
  title: string
  children: ReactNode
  size?: 'sm' | 'md' | 'lg'
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    document.addEventListener('keydown', onKey)
    const prevOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => {
      document.removeEventListener('keydown', onKey)
      document.body.style.overflow = prevOverflow
    }
  }, [onClose])

  const w = size === 'lg' ? 'max-w-2xl' : size === 'sm' ? 'max-w-sm' : 'max-w-md'

  return (
    <div className="fixed inset-0 z-50 grid place-items-center px-4 py-8 overflow-auto">
      <div
        className="fixed inset-0 bg-black/60 backdrop-blur-sm"
        onClick={onClose}
        aria-hidden
      />
      <div
        role="dialog"
        aria-modal="true"
        className={`relative w-full ${w} glass-card rise bg-zinc-950/85 shadow-2xl`}
      >
        <div className="flex items-center justify-between border-b border-white/5 px-5 py-3">
          <h3 className="text-sm font-medium text-zinc-100">{title}</h3>
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭"
            className="text-zinc-400 hover:text-white text-xl leading-none px-1"
          >
            ×
          </button>
        </div>
        <div className="p-5">{children}</div>
      </div>
    </div>
  )
}

// 表单字段输入框样式，节点/规则表单复用。
export const fieldInputCls =
  'w-full rounded-lg bg-white/[0.04] border border-white/10 px-3 py-2 text-sm placeholder:text-zinc-500 focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent/30 disabled:opacity-60 transition-shadow'

export const fieldLabelCls = 'block text-xs font-medium text-zinc-300 mb-1.5'

// 权限兜底:admin-only 页面对普通用户渲染此卡(直接输 URL 也不再看到裸 forbidden)。
export function ForbiddenCard() {
  return (
    <div className="glass-card rise border-amber-500/20 p-8 text-center">
      <div className="text-lg font-medium text-amber-200">无权限访问</div>
      <p className="mt-2 text-sm text-zinc-400">
        此页面仅管理员可用。如需调整账号权限，请联系管理员。
      </p>
    </div>
  )
}

// 状态徽章：节点 online/offline/unknown，规则 enabled/disabled。
export function StatusDot({ kind }: { kind: 'online' | 'offline' | 'unknown' | 'on' | 'off' }) {
  const color =
    kind === 'online' || kind === 'on'
      ? 'bg-emerald-400 shadow-emerald-400/50'
      : kind === 'offline' || kind === 'off'
      ? 'bg-zinc-500'
      : 'bg-amber-400'
  return <span className={`inline-block h-2 w-2 rounded-full shadow ${color}`} aria-hidden />
}
