// 极简 SVG 折线图。给详情页时序卡片用,不引图表库。
// 输入一段 values:按时间升序的样本(空数组 → 显示 placeholder)。

interface Props {
  values: number[]
  width?: number
  height?: number
  /** Tailwind stroke 颜色类,如 'stroke-indigo-400'。默认 indigo。 */
  colorClass?: string
  /** Tailwind fill 颜色类(填面积)。默认透明。 */
  fillClass?: string
  /** 若不传则在数据为空时显示该占位文案。 */
  emptyLabel?: string
}

export function Sparkline({
  values,
  width = 320,
  height = 64,
  colorClass = 'stroke-indigo-400',
  fillClass,
  emptyLabel = '尚无时序数据',
}: Props) {
  if (values.length === 0) {
    return (
      <div
        className="flex items-center justify-center text-[11px] text-zinc-500"
        style={{ width, height }}
      >
        {emptyLabel}
      </div>
    )
  }

  const max = Math.max(...values, 1)
  const min = Math.min(...values, 0)
  const range = Math.max(max - min, 1)
  const step = values.length > 1 ? width / (values.length - 1) : 0

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
    <svg
      viewBox={`0 0 ${width} ${height}`}
      width={width}
      height={height}
      aria-hidden
      className="overflow-visible"
    >
      {areaPath && <path d={areaPath} className={`${fillClass} stroke-none`} />}
      <path d={path} fill="none" strokeWidth={1.5} className={colorClass} />
    </svg>
  )
}
