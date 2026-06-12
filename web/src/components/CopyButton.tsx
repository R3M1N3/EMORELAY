import { useCallback, useState } from 'react'
import { useToast } from '../lib/use-toast'

// 一键复制按钮:点击把 value 写入剪贴板,内联 ✓ 反馈 1.2s + toast。
// 面板最高频操作是把入口地址复制发给用户,故做成轻量可内联复用的组件。
// 剪贴板不可用(http 非安全上下文/旧浏览器)时降级 toast 提示手动复制。
export function CopyButton({
  value,
  label,
  className = '',
}: {
  value: string
  /** 无障碍标签,如「复制监听地址」;默认「复制」。 */
  label?: string
  className?: string
}) {
  const toast = useToast()
  const [copied, setCopied] = useState(false)

  const onCopy = useCallback(async () => {
    try {
      if (!navigator.clipboard) throw new Error('clipboard unavailable')
      await navigator.clipboard.writeText(value)
      setCopied(true)
      toast.success('已复制到剪贴板')
      setTimeout(() => setCopied(false), 1200)
    } catch {
      toast.error('复制失败，请手动选择文本复制')
    }
  }, [value, toast])

  return (
    <button
      type="button"
      onClick={onCopy}
      aria-label={label ?? '复制'}
      title={label ?? '复制'}
      className={`inline-flex items-center justify-center rounded p-0.5 text-zinc-500 hover:text-accent-hi transition-colors ${className}`}
    >
      {copied ? (
        <span aria-hidden className="text-emerald-400 text-[11px] leading-none">
          ✓
        </span>
      ) : (
        // 两叠方框的复制图标,12px,继承 currentColor。
        <svg
          aria-hidden
          viewBox="0 0 24 24"
          className="h-3 w-3"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <rect x="9" y="9" width="13" height="13" rx="2" />
          <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
        </svg>
      )}
    </button>
  )
}
