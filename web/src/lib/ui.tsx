import { useEffect, useId, useRef, useState, type InputHTMLAttributes, type ReactNode } from 'react'

// 弹窗内可聚焦元素选择器:焦点陷阱与挂载聚焦共用。
const FOCUSABLE_SELECTOR =
  'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])'

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
  const dialogRef = useRef<HTMLDivElement>(null)
  const titleId = useId()
  // onClose 存 ref(同 use-auto-refresh 惯例):keydown 调最新值,使主 effect 依赖 [] 仅在
  // 挂载/卸载各跑一次。否则调用点均传内联闭包,父组件在弹窗开启期重渲染(如删除置 busy)会令
  // effect 重挂——cleanup 把焦点甩回触发按钮、setup 再抢回容器,造成开启中夺焦/踢出输入光标。
  const onCloseRef = useRef(onClose)
  useEffect(() => {
    onCloseRef.current = onClose
  })
  useEffect(() => {
    const dialog = dialogRef.current
    // 打开前持有焦点的元素(通常是触发按钮),关闭后归还,避免键盘/读屏用户焦点丢失。
    const prevFocused = document.activeElement as HTMLElement | null
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onCloseRef.current()
        return
      }
      // 焦点陷阱:Tab/Shift+Tab 在弹窗内可聚焦元素间环绕,不泄漏到被遮罩盖住的背景。
      if (e.key === 'Tab' && dialog) {
        const items = dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)
        if (items.length === 0) {
          e.preventDefault()
          return
        }
        const first = items[0]
        const last = items[items.length - 1]
        const active = document.activeElement
        if (e.shiftKey && (active === first || !dialog.contains(active))) {
          e.preventDefault()
          last.focus()
        } else if (!e.shiftKey && (active === last || !dialog.contains(active))) {
          e.preventDefault()
          first.focus()
        }
      }
    }
    document.addEventListener('keydown', onKey)
    const prevOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    // 挂载即聚焦:桌面端(精确指针)落到首个输入框省去一次点击;触屏端只聚焦对话框容器,
    // 避免 autoFocus 弹出软键盘遮挡表单;无输入框的确认弹窗一律聚焦容器(不抢焦到危险按钮)。
    if (dialog) {
      const finePointer = window.matchMedia('(pointer: fine)').matches
      const firstField = finePointer
        ? dialog.querySelector<HTMLElement>(
            'input:not([disabled]), select:not([disabled]), textarea:not([disabled])',
          )
        : null
      const target = firstField ?? dialog
      target.focus()
    }
    return () => {
      document.removeEventListener('keydown', onKey)
      document.body.style.overflow = prevOverflow
      prevFocused?.focus?.()
    }
  }, [])

  const w = size === 'lg' ? 'max-w-2xl' : size === 'sm' ? 'max-w-sm' : 'max-w-md'

  return (
    <div className="fixed inset-0 z-50 grid place-items-center px-4 py-8 overflow-auto">
      <div
        className="fixed inset-0 bg-black/60 backdrop-blur-sm"
        onClick={onClose}
        aria-hidden
      />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        className={`relative w-full ${w} glass-card rise bg-zinc-950/85 shadow-2xl outline-none`}
      >
        <div className="flex items-center justify-between border-b border-white/5 px-5 py-3">
          <h3 id={titleId} className="text-sm font-medium text-zinc-100">{title}</h3>
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

// 眼睛图标(自研内联 SVG,不引图标库):睁眼=当前明文可点击隐藏前的「显示」态提示,带斜杠=隐藏态。
function EyeIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7Z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  )
}
function EyeOffIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7Z" />
      <circle cx="12" cy="12" r="3" />
      <path d="M3 3l18 18" />
    </svg>
  )
}

// 密码输入框 + 显示/隐藏切换:登录/改密/建用户复用。切换按钮是正常 Tab 停留点(键盘可操作),
// type=button 不触发表单提交;沿用 fieldInputCls 并右留图标位(pr-10)。其余 input 属性透传。
export function PasswordInput({
  className,
  ...rest
}: Omit<InputHTMLAttributes<HTMLInputElement>, 'type'>) {
  const [show, setShow] = useState(false)
  return (
    <div className="relative">
      <input {...rest} type={show ? 'text' : 'password'} className={`${className ?? fieldInputCls} pr-10`} />
      <button
        type="button"
        onClick={() => setShow((s) => !s)}
        aria-label={show ? '隐藏密码' : '显示密码'}
        aria-pressed={show}
        title={show ? '隐藏密码' : '显示密码'}
        className="absolute inset-y-0 right-0 grid w-10 place-items-center rounded-r-lg text-zinc-400 hover:text-zinc-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/50"
      >
        {show ? <EyeOffIcon /> : <EyeIcon />}
      </button>
    </div>
  )
}

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

// 页面级错误条:role="alert" 让读屏即时播报;可选「重试」按钮触发重新拉取
// (避免干等 15-30s 的静默自动刷新)。列表/详情/概览的整页错误态统一用它。
export function ErrorBox({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div
      role="alert"
      className="flex items-start justify-between gap-3 rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-200"
    >
      <span>{message}</span>
      {onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="shrink-0 rounded-md bg-red-500/20 hover:bg-red-500/30 ring-1 ring-inset ring-red-400/30 px-2.5 py-1 text-xs font-medium text-red-100"
        >
          重试
        </button>
      )}
    </div>
  )
}

// 骨架屏基元:脉冲占位块,纯装饰对 AT 隐藏(reduced-motion 下脉冲自动近乎静止)。
export function Skeleton({ className = '' }: { className?: string }) {
  return <div className={`animate-pulse rounded bg-white/10 ${className}`} aria-hidden />
}

// 表格首载骨架:列表页表格用,替代「加载中…」纯文字,减少数据到位时的布局跳动。
// 容器 role="status" + sr-only 文案,保留读屏的「加载中」反馈。
export function TableSkeleton({ rows = 6, cols = 4 }: { rows?: number; cols?: number }) {
  return (
    <div role="status" className="p-4 space-y-3">
      <span className="sr-only">加载中…</span>
      {Array.from({ length: rows }).map((_, r) => (
        <div key={r} className="flex gap-4" aria-hidden>
          {Array.from({ length: cols }).map((_, c) => (
            <div key={c} className="h-4 flex-1 animate-pulse rounded bg-white/10" />
          ))}
        </div>
      ))}
    </div>
  )
}

// 整页首载骨架:概览/详情/设置等卡片型页面用。标题条 + 卡片网格 + 大块。
export function PageLoading() {
  return (
    <div role="status" className="space-y-6">
      <span className="sr-only">加载中…</span>
      <Skeleton className="h-7 w-40" />
      <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <Skeleton key={i} className="h-24 rounded-2xl" />
        ))}
      </div>
      <Skeleton className="h-40 rounded-2xl" />
    </div>
  )
}

// 空状态:列表无数据时居中展示图标 + 文案 + 可选就地行动按钮,替代一行细灰字。
export function EmptyState({
  title,
  hint,
  action,
}: {
  title: string
  hint?: string
  action?: ReactNode
}) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 px-6 py-16 text-center">
      <div className="grid h-12 w-12 place-items-center rounded-2xl bg-white/[0.04] text-zinc-400 ring-1 ring-inset ring-white/10">
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
          <path d="M3 7l9-4 9 4-9 4-9-4Z" />
          <path d="M3 7v10l9 4 9-4V7" />
          <path d="M3 7l9 4 9-4" />
        </svg>
      </div>
      <div>
        <div className="text-sm font-medium text-zinc-200">{title}</div>
        {hint && <p className="mx-auto mt-1 max-w-sm text-[12px] text-zinc-400">{hint}</p>}
      </div>
      {action && <div className="mt-1">{action}</div>}
    </div>
  )
}
