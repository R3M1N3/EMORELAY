// 极简分页器:仅"上一页 / 下一页 / 当前页/总页数 / page_size 切换"。
// 不渲染页码 1 2 3...,避免在 100+ 页时挤爆 toolbar。

interface Props {
  page: number
  pageSize: number
  total: number
  onChangePage: (page: number) => void
  onChangePageSize?: (size: number) => void
  pageSizeOptions?: number[]
}

export function Pagination({
  page,
  pageSize,
  total,
  onChangePage,
  onChangePageSize,
  pageSizeOptions = [20, 50, 100],
}: Props) {
  const totalPages = Math.max(1, Math.ceil(total / pageSize))
  const safePage = Math.min(Math.max(1, page), totalPages)
  const start = total === 0 ? 0 : (safePage - 1) * pageSize + 1
  const end = Math.min(total, safePage * pageSize)

  return (
    <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3 border-t border-white/5 text-[12px] text-zinc-400">
      <div>
        共 {total} 条 · 显示 {start}-{end}
      </div>
      <div className="flex items-center gap-2">
        {onChangePageSize && (
          <label className="flex items-center gap-1.5">
            <span className="text-zinc-500">每页</span>
            <select
              value={pageSize}
              onChange={(e) => onChangePageSize(Number(e.target.value))}
              className="rounded-md bg-zinc-800 border border-white/10 px-1.5 py-0.5 text-xs"
            >
              {pageSizeOptions.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
          </label>
        )}
        <button
          type="button"
          disabled={safePage <= 1}
          onClick={() => onChangePage(safePage - 1)}
          className="rounded-md bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 disabled:cursor-not-allowed px-2 py-0.5"
        >
          ← 上一页
        </button>
        <span className="text-zinc-300 tabular-nums">
          {safePage} / {totalPages}
        </span>
        <button
          type="button"
          disabled={safePage >= totalPages}
          onClick={() => onChangePage(safePage + 1)}
          className="rounded-md bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 disabled:cursor-not-allowed px-2 py-0.5"
        >
          下一页 →
        </button>
      </div>
    </div>
  )
}
