import { useEffect, useState, type FormEvent } from 'react'
import {
  ApiError,
  bandwidthProfiles,
  shortTime,
  type BandwidthProfileView,
} from '../lib/api'
import { ErrorBox, Modal, TableSkeleton, fieldInputCls, fieldLabelCls } from '../lib/ui'
import { Pagination } from '../components/Pagination'
import { useToast } from '../lib/use-toast'

type Editing = { mode: 'create' } | { mode: 'edit'; profile: BandwidthProfileView } | null

interface ListState {
  items: BandwidthProfileView[]
  total: number
  loading: boolean
  error: string | null
}

export default function BandwidthProfiles() {
  const toast = useToast()
  const [list, setList] = useState<ListState>({ items: [], total: 0, loading: true, error: null })
  const [editing, setEditing] = useState<Editing>(null)
  const [confirming, setConfirming] = useState<BandwidthProfileView | null>(null)
  const [busy, setBusy] = useState(false)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)

  async function reload() {
    setList((s) => ({ ...s, loading: true, error: null }))
    try {
      const r = await bandwidthProfiles.list({ page, page_size: pageSize })
      setList({ items: r.items, total: r.total, loading: false, error: null })
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '加载失败'
      setList({ items: [], total: 0, loading: false, error: msg })
    }
  }

  useEffect(() => {
    let cancelled = false
    bandwidthProfiles
      .list({ page, page_size: pageSize })
      .then((r) => {
        if (!cancelled) setList({ items: r.items, total: r.total, loading: false, error: null })
      })
      .catch((e: unknown) => {
        if (cancelled) return
        const msg = e instanceof ApiError ? e.message : '加载失败'
        setList({ items: [], total: 0, loading: false, error: msg })
      })
    return () => {
      cancelled = true
    }
  }, [page, pageSize])

  async function doDelete(p: BandwidthProfileView) {
    setBusy(true)
    try {
      await bandwidthProfiles.del(p.id)
      toast.success('限速配置已删除')
      setConfirming(null)
      await reload()
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : '删除失败')
      setConfirming(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">限速配置</h2>
          <p className="text-sm text-zinc-400 mt-1">可复用的带宽上限模板，应用于转发规则</p>
        </div>
        <button
          onClick={() => setEditing({ mode: 'create' })}
          className="btn-accent shrink-0"
        >
          新增限速配置
        </button>
      </div>

      {list.error && <ErrorBox message={list.error} onRetry={() => void reload()} />}

      <section className="glass-card rise overflow-hidden">
        {list.loading ? (
          <TableSkeleton cols={5} />
        ) : list.items.length === 0 ? (
          <div className="p-6 text-sm text-zinc-400">尚无限速配置。点击右上角「新增限速配置」。</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-[11px] uppercase text-zinc-400 bg-white/[0.03]">
                <tr>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">名称</th>
                  <th scope="col" className="px-4 py-2.5 text-right font-medium">带宽 (Mbps)</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">描述</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">更新于</th>
                  <th scope="col" className="px-4 py-2.5 text-right font-medium">操作</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-white/5">
                {list.items.map((p) => (
                  <tr key={p.id} className="hover:bg-white/[0.02]">
                    <td className="px-4 py-3 align-top">
                      <div className="font-medium text-zinc-100">{p.name}</div>
                      <div className="text-[11px] text-zinc-400 mt-0.5">ID #{p.id}</div>
                    </td>
                    <td className="px-4 py-3 align-top text-right text-zinc-200 tabular-nums">
                      {p.bandwidth_mbps}
                    </td>
                    <td className="px-4 py-3 align-top text-zinc-400 text-[12px] max-w-[18rem] truncate">
                      {p.description || '—'}
                    </td>
                    <td className="px-4 py-3 align-top text-zinc-400 text-[12px]">
                      {shortTime(p.updated_at)}
                    </td>
                    <td className="px-4 py-3 align-top text-right whitespace-nowrap">
                      <button
                        type="button"
                        onClick={() => setEditing({ mode: 'edit', profile: p })}
                        className="rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-2.5 py-1 text-xs"
                      >
                        编辑
                      </button>
                      <button
                        type="button"
                        onClick={() => setConfirming(p)}
                        className="ml-1.5 rounded-md bg-red-600/80 hover:bg-red-500 px-2.5 py-1 text-xs"
                      >
                        删除
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {!list.loading && list.items.length > 0 && (
          <Pagination
            page={page}
            pageSize={pageSize}
            total={list.total}
            onChangePage={setPage}
            onChangePageSize={(n) => {
              setPageSize(n)
              setPage(1)
            }}
          />
        )}
      </section>

      {editing && (
        <Modal
          title={editing.mode === 'create' ? '新增限速配置' : `编辑 · ${editing.profile.name}`}
          onClose={() => setEditing(null)}
        >
          <ProfileForm
            mode={editing.mode}
            initial={editing.mode === 'edit' ? editing.profile : undefined}
            onCancel={() => setEditing(null)}
            onSuccess={async () => {
              toast.success(editing.mode === 'create' ? '限速配置已创建' : '限速配置已保存')
              setEditing(null)
              await reload()
            }}
          />
        </Modal>
      )}

      {confirming && (
        <Modal title="删除限速配置" onClose={() => !busy && setConfirming(null)} size="sm">
          <p className="text-sm text-zinc-300">
            将删除 <span className="text-white font-medium">{confirming.name}</span>（
            {confirming.bandwidth_mbps} Mbps）。仍被规则引用时会被拒绝。
          </p>
          <div className="mt-5 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setConfirming(null)}
              disabled={busy}
              className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm"
            >
              取消
            </button>
            <button
              type="button"
              onClick={() => doDelete(confirming)}
              disabled={busy}
              className="rounded-lg bg-red-600 hover:bg-red-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
            >
              {busy ? '删除中…' : '确认删除'}
            </button>
          </div>
        </Modal>
      )}
    </div>
  )
}

function ProfileForm({
  mode,
  initial,
  onCancel,
  onSuccess,
}: {
  mode: 'create' | 'edit'
  initial?: BandwidthProfileView
  onCancel: () => void
  onSuccess: () => void | Promise<void>
}) {
  const [form, setForm] = useState({
    name: initial?.name ?? '',
    bandwidth_mbps: initial ? String(initial.bandwidth_mbps) : '',
    description: initial?.description ?? '',
  })
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)
    const mbps = Number(form.bandwidth_mbps)
    if (!Number.isInteger(mbps) || mbps <= 0) {
      setError('带宽必须是正整数 (Mbps)')
      return
    }
    if (!form.name.trim()) {
      setError('名称不能为空')
      return
    }
    setSubmitting(true)
    try {
      if (mode === 'create') {
        await bandwidthProfiles.create({
          name: form.name.trim(),
          bandwidth_mbps: mbps,
          description: form.description.trim(),
        })
      } else if (initial) {
        const payload: { name?: string; bandwidth_mbps?: number; description?: string } = {}
        if (form.name.trim() !== initial.name) payload.name = form.name.trim()
        if (mbps !== initial.bandwidth_mbps) payload.bandwidth_mbps = mbps
        if (form.description.trim() !== initial.description) payload.description = form.description.trim()
        if (Object.keys(payload).length === 0) {
          onCancel()
          return
        }
        await bandwidthProfiles.update(initial.id, payload)
      }
      await onSuccess()
    } catch (e) {
      setError(e instanceof ApiError ? e.message : '提交失败')
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <form onSubmit={onSubmit} className="space-y-4">
      <div>
        <label htmlFor="bw-name" className={fieldLabelCls}>名称 *</label>
        <input
          id="bw-name"
          required
          value={form.name}
          onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
          className={fieldInputCls}
          placeholder="例如 100mbps-shared"
        />
      </div>
      <div>
        <label htmlFor="bw-mbps" className={fieldLabelCls}>带宽 (Mbps) *</label>
        <input
          id="bw-mbps"
          type="number"
          min={1}
          required
          value={form.bandwidth_mbps}
          onChange={(e) => setForm((f) => ({ ...f, bandwidth_mbps: e.target.value }))}
          className={fieldInputCls}
          placeholder="100"
        />
        <p className="text-[11px] text-zinc-400 mt-1">
          上下行合并计；修改后引用此配置的规则即时生效。
        </p>
      </div>
      <div>
        <label htmlFor="bw-desc" className={fieldLabelCls}>描述</label>
        <input
          id="bw-desc"
          value={form.description}
          onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
          className={fieldInputCls}
          placeholder="可选"
        />
      </div>
      {error && (
        <div role="alert" className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200">
          {error}
        </div>
      )}
      <div className="flex justify-end gap-2 pt-1">
        <button
          type="button"
          onClick={onCancel}
          disabled={submitting}
          className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm"
        >
          取消
        </button>
        <button
          type="submit"
          disabled={submitting}
          className="btn-accent"
        >
          {submitting ? '提交中…' : mode === 'create' ? '创建' : '保存'}
        </button>
      </div>
    </form>
  )
}
