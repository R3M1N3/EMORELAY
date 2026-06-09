import { useEffect, useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'
import {
  ApiError,
  formatBytes,
  nodes,
  shortTime,
  type CreateNodeRequest,
  type NodeView,
  type UpdateNodeRequest,
} from '../lib/api'
import { Modal, StatusDot, fieldInputCls, fieldLabelCls } from '../lib/ui'
import { Pagination } from '../components/Pagination'

type Editing = { mode: 'create' } | { mode: 'edit'; node: NodeView } | null

interface ListState {
  items: NodeView[]
  total: number
  loading: boolean
  error: string | null
}

export default function Nodes() {
  const [list, setList] = useState<ListState>({ items: [], total: 0, loading: true, error: null })
  const [editing, setEditing] = useState<Editing>(null)
  const [confirming, setConfirming] = useState<NodeView | null>(null)
  const [token, setToken] = useState<{ token: string; name: string } | null>(null)
  const [busy, setBusy] = useState(false)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const [search, setSearch] = useState('')

  async function reload() {
    setList((s) => ({ ...s, loading: true, error: null }))
    try {
      const r = await nodes.list({ page, page_size: pageSize })
      setList({ items: r.items, total: r.total, loading: false, error: null })
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '加载失败'
      setList({ items: [], total: 0, loading: false, error: msg })
    }
  }

  // page / pageSize 变化均触发拉取;事件回调里的 reload() 走最新 closure 值,不在 effect 里。
  useEffect(() => {
    let cancelled = false
    nodes
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

  async function doDelete(node: NodeView) {
    setBusy(true)
    try {
      await nodes.del(node.id)
      setConfirming(null)
      await reload()
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '删除失败'
      setList((s) => ({ ...s, error: msg }))
      setConfirming(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">节点</h2>
          <p className="text-sm text-zinc-400 mt-1">转发节点列表与 Agent 心跳状态</p>
        </div>
        <button
          onClick={() => setEditing({ mode: 'create' })}
          className="rounded-lg bg-indigo-600 hover:bg-indigo-500 px-3 py-2 text-sm font-medium shrink-0"
        >
          新增节点
        </button>
      </div>

      {list.error && (
        <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-200">
          {list.error}
        </div>
      )}

      {(() => {
        const needle = search.trim().toLowerCase()
        const filtered = needle
          ? list.items.filter((n) =>
              [n.name, n.region, n.public_ip, n.grpc_endpoint]
                .some((s) => s.toLowerCase().includes(needle)),
            )
          : list.items
        return (
          <>
            <div className="flex items-center gap-3 flex-wrap">
              <input
                type="search"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="搜索当前页 (名称 / 区域 / IP / gRPC)"
                className={`${fieldInputCls} max-w-sm`}
              />
              {needle && (
                <span className="text-xs text-zinc-500">
                  匹配 {filtered.length} / {list.items.length} 条 (仅当前页)
                </span>
              )}
            </div>

            <section className="rounded-2xl border border-white/10 bg-zinc-900/40 overflow-hidden">
              {list.loading ? (
                <div className="p-6 text-sm text-zinc-400">加载中…</div>
              ) : list.items.length === 0 ? (
                <div className="p-6 text-sm text-zinc-500">
                  尚无节点。点击右上角「新增节点」开始。
                </div>
              ) : filtered.length === 0 ? (
                <div className="p-6 text-sm text-zinc-500">没有匹配的节点。</div>
              ) : (
                <>
                  <div className="overflow-x-auto">
                    <table className="w-full text-sm">
                      <thead className="text-[11px] uppercase text-zinc-500 bg-zinc-900/80">
                        <tr>
                          <th className="px-4 py-2.5 text-left font-medium">名称</th>
                          <th className="px-4 py-2.5 text-left font-medium">区域 / IP</th>
                          <th className="px-4 py-2.5 text-left font-medium">gRPC</th>
                          <th className="px-4 py-2.5 text-left font-medium">状态</th>
                          <th className="px-4 py-2.5 text-left font-medium">资源</th>
                          <th className="px-4 py-2.5 text-left font-medium">流量</th>
                          <th className="px-4 py-2.5 text-left font-medium">端口池</th>
                          <th className="px-4 py-2.5 text-right font-medium">操作</th>
                        </tr>
                      </thead>
                      <tbody className="divide-y divide-white/5">
                        {filtered.map((n) => (
                          <NodeRow
                            key={n.id}
                            node={n}
                            onEdit={() => setEditing({ mode: 'edit', node: n })}
                            onDelete={() => setConfirming(n)}
                          />
                        ))}
                      </tbody>
                    </table>
                  </div>
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
                </>
              )}
            </section>
          </>
        )
      })()}

      {editing && (
        <Modal
          title={editing.mode === 'create' ? '新增节点' : `编辑节点 · ${editing.node.name}`}
          onClose={() => setEditing(null)}
        >
          <NodeForm
            mode={editing.mode}
            initial={editing.mode === 'edit' ? editing.node : undefined}
            onCancel={() => setEditing(null)}
            onSuccess={async (createdToken) => {
              setEditing(null)
              if (createdToken) setToken(createdToken)
              await reload()
            }}
          />
        </Modal>
      )}

      {confirming && (
        <Modal title="删除节点" onClose={() => !busy && setConfirming(null)} size="sm">
          <p className="text-sm text-zinc-300">
            将删除节点 <span className="text-white font-medium">{confirming.name}</span>。
            该节点上的规则将无法继续下发，请确认。
          </p>
          <div className="mt-5 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setConfirming(null)}
              disabled={busy}
              className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm"
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

      {token && (
        <Modal title="Agent 接入凭据" onClose={() => setToken(null)} size="sm">
          <p className="text-sm text-zinc-300">
            节点 <span className="font-medium text-white">{token.name}</span> 的 Agent token，
            <span className="text-amber-300">仅此一次显示</span>，请立即妥善保存：
          </p>
          <div className="mt-3 rounded-lg border border-white/10 bg-zinc-950 px-3 py-2 font-mono text-xs text-emerald-300 break-all">
            {token.token}
          </div>
          <div className="mt-4 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => {
                navigator.clipboard?.writeText(token.token).catch(() => {})
              }}
              className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-xs"
            >
              复制
            </button>
            <button
              type="button"
              onClick={() => setToken(null)}
              className="rounded-lg bg-indigo-600 hover:bg-indigo-500 px-3 py-2 text-xs font-medium"
            >
              我已保存
            </button>
          </div>
        </Modal>
      )}
    </div>
  )
}

function NodeRow({
  node,
  onEdit,
  onDelete,
}: {
  node: NodeView
  onEdit: () => void
  onDelete: () => void
}) {
  return (
    <tr className="hover:bg-white/[0.02]">
      <td className="px-4 py-3 align-top">
        <Link
          to={`/nodes/${node.id}`}
          className="font-medium text-zinc-100 hover:text-indigo-300"
        >
          {node.name}
        </Link>
        <div className="text-[11px] text-zinc-500 mt-0.5">ID #{node.id}</div>
      </td>
      <td className="px-4 py-3 align-top text-zinc-300">
        <div>{node.region || '—'}</div>
        <div className="text-[11px] text-zinc-500 mt-0.5">{node.public_ip || '未填'}</div>
      </td>
      <td className="px-4 py-3 align-top text-zinc-400 font-mono text-[12px]">
        {node.grpc_endpoint || '—'}
      </td>
      <td className="px-4 py-3 align-top">
        <span className="inline-flex items-center gap-1.5 text-xs text-zinc-300">
          <StatusDot kind={node.status} />
          {node.status}
        </span>
        <div className="text-[11px] text-zinc-500 mt-0.5">
          {node.last_seen_at ? `最后心跳 ${shortTime(node.last_seen_at)}` : '从未上线'}
        </div>
      </td>
      <td className="px-4 py-3 align-top text-[12px] text-zinc-300">
        <div>CPU {node.cpu_usage.toFixed(1)}%</div>
        <div>MEM {node.memory_usage.toFixed(1)}%</div>
        <div>LOAD {node.load_average.toFixed(2)}</div>
      </td>
      <td className="px-4 py-3 align-top text-[12px] text-zinc-300">
        <div>↓ {formatBytes(node.rx_bytes_total)}</div>
        <div>↑ {formatBytes(node.tx_bytes_total)}</div>
      </td>
      <td className="px-4 py-3 align-top text-[12px] text-zinc-400">
        {node.port_pool_min}–{node.port_pool_max}
      </td>
      <td className="px-4 py-3 align-top text-right">
        <button
          type="button"
          onClick={onEdit}
          className="rounded-md bg-zinc-800 hover:bg-zinc-700 px-2.5 py-1 text-xs"
        >
          编辑
        </button>
        <button
          type="button"
          onClick={onDelete}
          className="ml-1.5 rounded-md bg-red-600/80 hover:bg-red-500 px-2.5 py-1 text-xs"
        >
          删除
        </button>
      </td>
    </tr>
  )
}

interface NodeFormState {
  name: string
  region: string
  public_ip: string
  grpc_endpoint: string
  port_pool_min: string
  port_pool_max: string
}

function NodeForm({
  mode,
  initial,
  onCancel,
  onSuccess,
}: {
  mode: 'create' | 'edit'
  initial?: NodeView
  onCancel: () => void
  onSuccess: (createdToken: { token: string; name: string } | null) => void | Promise<void>
}) {
  const [form, setForm] = useState<NodeFormState>({
    name: initial?.name ?? '',
    region: initial?.region ?? '',
    public_ip: initial?.public_ip ?? '',
    grpc_endpoint: initial?.grpc_endpoint ?? '',
    port_pool_min: initial ? String(initial.port_pool_min) : '',
    port_pool_max: initial ? String(initial.port_pool_max) : '',
  })
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  function set<K extends keyof NodeFormState>(k: K, v: NodeFormState[K]) {
    setForm((f) => ({ ...f, [k]: v }))
  }

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)

    const portMin = form.port_pool_min.trim() === '' ? undefined : Number(form.port_pool_min)
    const portMax = form.port_pool_max.trim() === '' ? undefined : Number(form.port_pool_max)
    if (
      (portMin !== undefined && (!Number.isInteger(portMin) || portMin < 1 || portMin > 65535)) ||
      (portMax !== undefined && (!Number.isInteger(portMax) || portMax < 1 || portMax > 65535))
    ) {
      setError('端口池范围必须是 1-65535 的整数')
      return
    }
    if (portMin !== undefined && portMax !== undefined && portMin > portMax) {
      setError('端口池下界不能大于上界')
      return
    }

    setSubmitting(true)
    try {
      if (mode === 'create') {
        const payload: CreateNodeRequest = {
          name: form.name.trim(),
          region: form.region.trim(),
          public_ip: form.public_ip.trim(),
          grpc_endpoint: form.grpc_endpoint.trim(),
          port_pool_min: portMin,
          port_pool_max: portMax,
        }
        const r = await nodes.create(payload)
        await onSuccess({ token: r.agent_token, name: r.node.name })
      } else if (initial) {
        const payload: UpdateNodeRequest = {
          name: form.name.trim() !== initial.name ? form.name.trim() : undefined,
          region: form.region.trim() !== initial.region ? form.region.trim() : undefined,
          public_ip: form.public_ip.trim() !== initial.public_ip ? form.public_ip.trim() : undefined,
          grpc_endpoint:
            form.grpc_endpoint.trim() !== initial.grpc_endpoint
              ? form.grpc_endpoint.trim()
              : undefined,
          port_pool_min: portMin !== initial.port_pool_min ? portMin : undefined,
          port_pool_max: portMax !== initial.port_pool_max ? portMax : undefined,
        }
        await nodes.update(initial.id, payload)
        await onSuccess(null)
      }
    } catch (e) {
      setError(e instanceof ApiError ? e.message : '提交失败')
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <form onSubmit={onSubmit} className="space-y-4">
      <div>
        <label className={fieldLabelCls}>名称 *</label>
        <input
          required
          value={form.name}
          onChange={(e) => set('name', e.target.value)}
          className={fieldInputCls}
          placeholder="例如 hk-relay-01"
        />
      </div>
      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className={fieldLabelCls}>区域</label>
          <input
            value={form.region}
            onChange={(e) => set('region', e.target.value)}
            className={fieldInputCls}
            placeholder="HK / SG / JP …"
          />
        </div>
        <div>
          <label className={fieldLabelCls}>公网 IP</label>
          <input
            value={form.public_ip}
            onChange={(e) => set('public_ip', e.target.value)}
            className={fieldInputCls}
            placeholder="1.2.3.4"
          />
        </div>
      </div>
      <div>
        <label className={fieldLabelCls}>gRPC 端点</label>
        <input
          value={form.grpc_endpoint}
          onChange={(e) => set('grpc_endpoint', e.target.value)}
          className={fieldInputCls}
          placeholder="https://agent.example.com:7001"
        />
        <p className="text-[11px] text-zinc-500 mt-1">
          仅作展示用途。Agent 会主动用 token 连接主控，不由主控反向拨号。
        </p>
      </div>
      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className={fieldLabelCls}>端口池下界</label>
          <input
            type="number"
            min={1}
            max={65535}
            value={form.port_pool_min}
            onChange={(e) => set('port_pool_min', e.target.value)}
            className={fieldInputCls}
            placeholder="默认 1"
          />
        </div>
        <div>
          <label className={fieldLabelCls}>端口池上界</label>
          <input
            type="number"
            min={1}
            max={65535}
            value={form.port_pool_max}
            onChange={(e) => set('port_pool_max', e.target.value)}
            className={fieldInputCls}
            placeholder="默认 65535"
          />
        </div>
      </div>

      {error && (
        <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200">
          {error}
        </div>
      )}

      <div className="flex justify-end gap-2 pt-1">
        <button
          type="button"
          onClick={onCancel}
          disabled={submitting}
          className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm"
        >
          取消
        </button>
        <button
          type="submit"
          disabled={submitting}
          className="rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
        >
          {submitting ? '提交中…' : mode === 'create' ? '创建' : '保存'}
        </button>
      </div>
    </form>
  )
}

