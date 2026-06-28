// 极简 SVG 折线图。给详情页时序卡片用,不引图表库。
// 响应式:viewBox 坐标系固定、容器 width:100% 缩放,stroke 用 non-scaling-stroke 保持 1.5px。
// 顶/底基线 + 右缘 min/max 标注给出 Y 轴量程(消除"按序列 min-max 自动缩放"造成的假性平直误导);
// 头部显示 当前/峰值 读数;hover 显示最近点的值。少于 2 个样本显示占位(单点画不出趋势)。
import { useState } from 'react'

interface Props {
  values: number[]
  /** viewBox 坐标宽(随容器缩放,非渲染像素);height 为渲染高度。 */
  width?: number
  height?: number
  /** Tailwind stroke 颜色类,如 'stroke-accent'。默认 accent。 */
  colorClass?: string
  /** Tailwind fill 颜色类(填面积)。默认透明。 */
  fillClass?: string
  /** 数据不足(样本 < 2)时显示的占位文案。 */
  emptyLabel?: string
  /** 数值格式化(如 formatBytes);不传则整数原样、小数保留 1 位。 */
  formatValue?: (n: number) => string
  /** 无障碍标签:图本身的可访问名(配合父卡标题);不传用通用文案。 */
  label?: string
}

export function Sparkline({
  values,
  width = 320,
  height = 56,
  colorClass = 'stroke-accent',
  fillClass,
  emptyLabel = '尚无时序数据',
  formatValue,
  label,
}: Props) {
  const [hover, setHover] = useState<number | null>(null)
  const fmt = formatValue ?? ((n: number) => (Number.isInteger(n) ? String(n) : n.toFixed(1)))

  if (values.length < 2) {
    return (
      <div className="flex items-center justify-center text-xs text-zinc-400" style={{ height }}>
        {values.length === 0 ? emptyLabel : '数据不足(再等一个统计周期)'}
      </div>
    )
  }

  const max = Math.max(...values)
  const min = Math.min(...values)
  const cur = values[values.length - 1]
  const range = Math.max(max - min, 1e-9)
  const step = width / (values.length - 1)
  const points = values.map((v, i) => [i * step, height - ((v - min) / range) * height] as const)
  const path = points
    .map(([x, y], i) => `${i ? 'L' : 'M'}${x.toFixed(1)},${y.toFixed(1)}`)
    .join(' ')
  const areaPath = fillClass ? `${path} L${width},${height} L0,${height} Z` : null
  const hp = hover != null && hover >= 0 && hover < points.length ? points[hover] : null

  return (
    <div className="w-full">
      <div className="mb-1 flex items-center justify-between text-[11px] text-zinc-400">
        <span>
          当前 <span className="text-zinc-200 tabular-nums">{fmt(cur)}</span>
        </span>
        <span className="tabular-nums">峰值 {fmt(max)}</span>
      </div>
      <div
        className="relative w-full"
        style={{ height }}
        onMouseLeave={() => setHover(null)}
        onMouseMove={(e) => {
          const r = e.currentTarget.getBoundingClientRect()
          if (r.width === 0) return
          const frac = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width))
          setHover(Math.round(frac * (values.length - 1)))
        }}
      >
        <svg
          viewBox={`0 0 ${width} ${height}`}
          width="100%"
          height={height}
          preserveAspectRatio="none"
          role="img"
          aria-label={label ?? '数据时序折线图'}
          className="block"
        >
          {/* 顶/底基线:提示 Y 轴量程,避免自动缩放误导 */}
          <line x1="0" y1="0.75" x2={width} y2="0.75" className="stroke-white/10" strokeWidth="1" vectorEffect="non-scaling-stroke" />
          <line x1="0" y1={height - 0.75} x2={width} y2={height - 0.75} className="stroke-white/10" strokeWidth="1" vectorEffect="non-scaling-stroke" />
          {areaPath && <path d={areaPath} className={`${fillClass} stroke-none`} />}
          <path d={path} fill="none" strokeWidth={1.5} vectorEffect="non-scaling-stroke" className={colorClass} />
          {hp && (
            <line x1={hp[0]} y1="0" x2={hp[0]} y2={height} className="stroke-white/25" strokeWidth="1" vectorEffect="non-scaling-stroke" />
          )}
        </svg>
        {/* 右缘 min/max 量程标注:加底色防与折线重叠;留 right-1 内边距防裁切 */}
        <span className="pointer-events-none absolute right-1 top-0 rounded bg-zinc-950/70 px-1 text-xs leading-tight text-zinc-400 tabular-nums">{fmt(max)}</span>
        <span className="pointer-events-none absolute right-1 bottom-0 rounded bg-zinc-950/70 px-1 text-xs leading-tight text-zinc-400 tabular-nums">{fmt(min)}</span>
        {/* hover 数值 */}
        {hover != null && (
          <span
            className="pointer-events-none absolute -top-3.5 rounded bg-zinc-900/95 px-1.5 py-0.5 text-[11px] text-zinc-100 ring-1 ring-white/10 tabular-nums"
            style={{ left: `${(hover / (values.length - 1)) * 100}%`, transform: 'translateX(-50%)' }}
          >
            {fmt(values[hover])}
          </span>
        )}
      </div>
    </div>
  )
}
