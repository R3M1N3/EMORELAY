// 极简 SVG 折线图。给详情页时序卡片用,不引图表库。
// 输入一段 values:按时间升序的样本(少于 2 个样本 → 显示 placeholder,
// 单点画不出趋势——评审发现单点被渲染成铺满时间轴的实心三角,误导性强)。

interface Props {
  values: number[]
  width?: number
  height?: number
  /** Tailwind stroke 颜色类,如 'stroke-accent'。默认 accent。 */
  colorClass?: string
  /** Tailwind fill 颜色类(填面积)。默认透明。 */
  fillClass?: string
  /** 数据不足(样本 < 2)时显示的占位文案。 */
  emptyLabel?: string
  /** 峰值标注格式化(如 formatBytes);不传则不显示峰值。 */
  formatValue?: (n: number) => string
  /** 无障碍标签:图本身的可访问名(配合父卡标题);不传用通用文案。 */
  label?: string
}

export function Sparkline({
  values,
  width = 320,
  height = 64,
  colorClass = 'stroke-accent',
  fillClass,
  emptyLabel = '尚无时序数据',
  formatValue,
  label,
}: Props) {
  if (values.length < 2) {
    return (
      <div
        className="flex items-center justify-center text-[11px] text-zinc-400"
        style={{ width, height }}
      >
        {values.length === 0 ? emptyLabel : '数据不足(再等一个统计周期)'}
      </div>
    )
  }

  const max = Math.max(...values, 1)
  const min = Math.min(...values, 0)
  const range = Math.max(max - min, 1)
  const step = width / (values.length - 1)

  const points = values.map((v, i) => {
    const x = i * step
    const y = height - ((v - min) / range) * height
    return [x, y] as const
  })

  const path = points
    .map(([x, y], i) => (i === 0 ? `M${x.toFixed(1)},${y.toFixed(1)}` : `L${x.toFixed(1)},${y.toFixed(1)}`))
    .join(' ')

  const areaPath = fillClass
    ? `${path} L${width.toFixed(1)},${height} L0,${height} Z`
    : null

  return (
    <div className="relative inline-block" style={{ width, height }}>
      {formatValue && (
        <span className="absolute right-0 -top-0.5 text-[10px] text-zinc-400">
          峰值 {formatValue(Math.max(...values))}
        </span>
      )}
      <svg
        viewBox={`0 0 ${width} ${height}`}
        width={width}
        height={height}
        role="img"
        aria-label={label ?? '数据时序折线图'}
        className="overflow-visible"
      >
        {areaPath && <path d={areaPath} className={`${fillClass} stroke-none`} />}
        <path d={path} fill="none" strokeWidth={1.5} className={colorClass} />
      </svg>
    </div>
  )
}
