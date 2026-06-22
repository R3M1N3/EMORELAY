import { useEffect, useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'
import {
  ApiError,
  nodes,
  shortTime,
  statusLabel,
  tunnels,
  type CreateTunnelRequest,
  type NodeView,
  type TunnelView,
} from '../lib/api'
import { EmptyState, ErrorBox, Modal, StatusDot, TableSkeleton, fieldInputCls, fieldLabelCls } from '../lib/ui'
import { Pagination } from '../components/Pagination'
import { useToast } from '../lib/use-toast'
import { useAutoRefresh } from '../lib/use-auto-refresh'

interface ListState {
  items: TunnelView[]
  total: number
  loading: boolean
  error: string | null
}

export default function Tunnels() {
  const toast = useToast()
  const [list, setList] = useState<ListState>({ items: [], total: 0, loading: true, error: null })
  const [showCreate, setShowCreate] = useState(false)
  const [confirming, setConfirming] = useState<TunnelView | null>(null)
  const [busy, setBusy] = useState(false)
  // 重启是破坏性操作(瞬断隧道上所有规则转发):加确认弹窗 + 在途防连点。
  const [restartConfirm, setRestartConfirm] = useState<TunnelView | null>(null)
  const [restarting, setRestarting] = useState(false)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const [nodeList, setNodeList] = useState<NodeView[]>([])

  useEffect(() => {
    nodes.list({ page_size: 100 }).then((r) => setNodeList(r.items)).catch(() => {})
  }, [])

  // 事件回调里的 reload() 走最新 closure 值,与 Nodes.tsx 模式一致。
  async function reload(opts: { silent?: boolean } = {}) {
    if (!opts.silent) setList((s) => ({ ...s, loading: true, error: null }))
    try {
      const r = await tunnels.list({ page, page_size: pageSize })
      setList({ items: r.items, total: r.total, loading: false, error: null })
    } catch (e: unknown) {
      if (opts.silent) return
      const msg = e instanceof ApiError ? e.message : '加载失败'
      setList({ items: [], total: 0, loading: false, error: msg })
    }
  }

  // 隧道 status 随 hop 心跳聚合变化,30s 静默刷新。
  useAutoRefresh(() => {
    void reload({ silent: true })
  }, 30_000)

  useEffect(() => {
    let cancelled = false
    tunnels
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

  async function doRestart(t: TunnelView) {
    setRestarting(true)
    try {
      await tunnels.restart(t.id)
      setRestartConfirm(null)
      toast.success(`隧道 ${t.name} 已下发重启`)
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : '重启失败')
    } finally {
      setRestarting(false)
    }
  }

  async function doDelete(t: TunnelView) {
    setBusy(true)
    try {
      await tunnels.del(t.id)
      setConfirming(null)
      toast.success(`隧道 ${t.name} 已删除`)
      await reload()
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '删除失败'
      toast.error(msg)
      setConfirming(null)
    } finally {
      setBusy(false)
    }
  }

  function tunnelStatusKind(status: TunnelView['status']): 'on' | 'off' | 'unknown' {
    if (status === 'up') return 'on'
    if (status === 'down') return 'off'
    return 'unknown'
  }

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">隧道</h2>
          <p className="text-sm text-zinc-400 mt-1">多跳转发隧道列表与状态</p>
        </div>
        <button
          onClick={() => setShowCreate(true)}
          className="btn-accent shrink-0"
        >
          创建隧道
        </button>
      </div>

      {list.error && <ErrorBox message={list.error} onRetry={() => void reload()} />}

      <section className="glass-card rise overflow-hidden">
        {list.loading ? (
          <TableSkeleton cols={7} />
        ) : list.items.length === 0 ? (
          <EmptyState
            title="尚无隧道"
            hint="多跳隧道把流量经多个节点中继转发;至少选 2 个在线节点组链。"
            action={<button type="button" onClick={() => setShowCreate(true)} className="btn-accent">创建隧道</button>}
          />
        ) : (
          <>
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead className="text-[11px] uppercase text-zinc-400 bg-white/[0.03]">
                  <tr>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">名称</th>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">传输</th>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">状态</th>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">跳数</th>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">规则数</th>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">创建时间</th>
                    <th scope="col" className="px-4 py-2.5 text-right font-medium">操作</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-white/5">
                  {list.items.map((t) => (
                    <tr key={t.id} className="hover:bg-white/[0.02]">
                      <td className="px-4 py-3 align-top">
                        <Link
                          to={`/tunnels/${t.id}`}
                          className="font-medium text-zinc-100 hover:text-accent"
                        >
                          {t.name}
                        </Link>
                        <div className="text-[11px] text-zinc-400 mt-0.5">ID #{t.id}</div>
                      </td>
                      <td className="px-4 py-3 align-top text-zinc-300 uppercase text-xs">
                        {t.transport}
                      </td>
                      <td className="px-4 py-3 align-top whitespace-nowrap">
                        <span className="inline-flex items-center gap-1.5 text-xs text-zinc-300">
                          <StatusDot kind={tunnelStatusKind(t.status)} />
                          {statusLabel(t.status)}
                        </span>
                      </td>
                      <td className="px-4 py-3 align-top text-zinc-300">{t.hops_count}</td>
                      <td className="px-4 py-3 align-top text-zinc-300">{t.rules_count}</td>
                      <td className="px-4 py-3 align-top text-[12px] text-zinc-400">
                        {shortTime(t.created_at)}
                      </td>
                      <td className="px-4 py-3 align-top text-right">
                        <button
                          type="button"
                          onClick={() => setRestartConfirm(t)}
                          className="rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-2.5 py-1 text-xs"
                        >
                          重启
                        </button>
                        <button
                          type="button"
                          onClick={() => setConfirming(t)}
                          className="ml-1.5 rounded-md px-2.5 py-1 text-xs text-red-300/90 ring-1 ring-inset ring-red-500/25 hover:bg-red-500/15 hover:text-red-200"
                        >
                          删除
                        </button>
                      </td>
                    </tr>
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

      {showCreate && (
        <Modal title="创建隧道" onClose={() => setShowCreate(false)} size="lg">
          <TunnelForm
            onlineNodes={nodeList.filter((n) => n.status === 'online')}
            onCancel={() => setShowCreate(false)}
            onSuccess={async () => {
              setShowCreate(false)
              await reload()
            }}
          />
        </Modal>
      )}

      {confirming && (
        <Modal title="删除隧道" onClose={() => !busy && setConfirming(null)} size="sm">
          {/* 评审 P2-4:原文案「关联规则将失去隧道绑定」与后端行为(直接拒绝)相反。
              rules_count 是列表自带数据,直接预检,有引用时禁用确认按钮。 */}
          {confirming.rules_count > 0 ? (
            <p className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-sm text-red-200">
              隧道 <span className="font-medium">{confirming.name}</span> 仍被{' '}
              {confirming.rules_count} 条规则关联，无法删除。请先在规则页删除关联规则。
            </p>
          ) : (
            <p className="text-sm text-zinc-300">
              将删除隧道 <span className="text-white font-medium">{confirming.name}</span>。
              链上各节点的中继任务将被撤销，请确认。
            </p>
          )}
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
              disabled={busy || confirming.rules_count > 0}
              className="rounded-lg bg-red-600 hover:bg-red-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
            >
              {busy ? '删除中…' : '确认删除'}
            </button>
          </div>
        </Modal>
      )}

      {restartConfirm && (
        <Modal title="重启隧道" onClose={() => !restarting && setRestartConfirm(null)} size="sm">
          <p className="text-sm text-zinc-300">
            重启隧道 <span className="text-white font-medium">{restartConfirm.name}</span> 会瞬断其上所有规则的转发，确认继续？
          </p>
          <div className="mt-5 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setRestartConfirm(null)}
              disabled={restarting}
              className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm disabled:opacity-50"
            >
              取消
            </button>
            <button
              type="button"
              onClick={() => doRestart(restartConfirm)}
              disabled={restarting}
              className="btn-accent disabled:opacity-50"
            >
              {restarting ? '重启中…' : '确认重启'}
            </button>
          </div>
        </Modal>
      )}
    </div>
  )
}

function TunnelForm({
  onlineNodes,
  onCancel,
  onSuccess,
}: {
  onlineNodes: NodeView[]
  onCancel: () => void
  onSuccess: () => void | Promise<void>
}) {
  const toast = useToast()
  const [name, setName] = useState('')
  const [transport, setTransport] = useState<CreateTunnelRequest['transport']>('tls')
  const [chain, setChain] = useState<string[]>(['', ''])
  const [trafficRatio, setTrafficRatio] = useState('1')
  const [billingMode, setBillingMode] = useState<1 | 2>(2)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  function setChainItem(i: number, val: string) {
    setChain((c) => c.map((v, idx) => (idx === i ? val : v)))
  }

  function moveUp(i: number) {
    if (i === 0) return
    setChain((c) => {
      const next = [...c]
      ;[next[i - 1], next[i]] = [next[i], next[i - 1]]
      return next
    })
  }

  function moveDown(i: number) {
    if (i === chain.length - 1) return
    setChain((c) => {
      const next = [...c]
      ;[next[i], next[i + 1]] = [next[i + 1], next[i]]
      return next
    })
  }

  function removeHop(i: number) {
    if (chain.length <= 2) return
    setChain((c) => c.filter((_, idx) => idx !== i))
  }

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)

    if (chain.some((v) => !v)) {
      setError('请为每个 hop 选择节点')
      return
    }
    const ids = chain.map(Number)
    if (new Set(ids).size !== ids.length) {
      setError('节点不可重复，请为每跳选择不同节点')
      return
    }

    if (!name.trim()) {
      setError('隧道名不能为空')
      return
    }

    const ratio = Number(trafficRatio)
    if (!Number.isFinite(ratio) || ratio < 0 || ratio > 100) {
      setError('流量倍率必须在 0 到 100 之间')
      return
    }

    setSubmitting(true)
    try {
      await tunnels.create({
        name: name.trim(),
        transport,
        node_ids: ids,
        traffic_ratio: ratio,
        billing_mode: billingMode,
      })
      toast.success(`隧道 ${name.trim()} 已创建`)
      await onSuccess()
    } catch (e) {
      setError(e instanceof ApiError ? e.message : '提交失败')
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <form noValidate onSubmit={onSubmit} className="space-y-4">
      <div>
        <label htmlFor="tunnel-name" className={fieldLabelCls}>
          隧道名 *
        </label>
        <input
          id="tunnel-name"
          required
          value={name}
          onChange={(e) => setName(e.target.value)}
          className={fieldInputCls}
          placeholder="例如 hk-jp"
        />
      </div>

      <div>
        <label htmlFor="tunnel-transport" className={fieldLabelCls}>
          传输协议
        </label>
        <select
          id="tunnel-transport"
          value={transport}
          onChange={(e) => setTransport(e.target.value as CreateTunnelRequest['transport'])}
          className={fieldInputCls}
        >
          <option value="tls">TLS（推荐）</option>
          <option value="tcp">TCP</option>
          <option value="wss">WSS</option>
        </select>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div>
          <label htmlFor="tunnel-ratio" className={fieldLabelCls}>
            流量倍率
          </label>
          <input
            id="tunnel-ratio"
            type="number"
            min={0}
            max={100}
            step="0.1"
            value={trafficRatio}
            onChange={(e) => setTrafficRatio(e.target.value)}
            className={fieldInputCls}
          />
          <p className="mt-1 text-[11px] text-zinc-400">计费乘数，1 = 原样，2 = 双倍</p>
        </div>
        <div>
          <label htmlFor="tunnel-billing" className={fieldLabelCls}>
            计费方向
          </label>
          <select
            id="tunnel-billing"
            value={billingMode}
            onChange={(e) => setBillingMode(Number(e.target.value) as 1 | 2)}
            className={fieldInputCls}
          >
            <option value={2}>双向（上行+下行）</option>
            <option value={1}>单向（较大方向）</option>
          </select>
        </div>
      </div>

      <div>
        <div className={fieldLabelCls}>节点链</div>
        <div className="space-y-2">
          {chain.map((val, i) => (
            <div key={i} className="flex items-center gap-2">
              <select
                aria-label={`节点 #${i + 1}`}
                value={val}
                onChange={(e) => setChainItem(i, e.target.value)}
                className={`${fieldInputCls} flex-1`}
              >
                <option value="">请选择节点</option>
                {onlineNodes.map((n) => (
                  <option key={n.id} value={String(n.id)}>
                    {n.name} ({n.public_ip || '无 IP'})
                  </option>
                ))}
              </select>
              <button
                type="button"
                onClick={() => moveUp(i)}
                disabled={i === 0}
                aria-label={`上移第 ${i + 1} 跳`}
                className="rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 disabled:opacity-40 px-2 py-1.5 text-xs"
              >
                ↑
              </button>
              <button
                type="button"
                onClick={() => moveDown(i)}
                disabled={i === chain.length - 1}
                aria-label={`下移第 ${i + 1} 跳`}
                className="rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 disabled:opacity-40 px-2 py-1.5 text-xs"
              >
                ↓
              </button>
              {chain.length > 2 && (
                <button
                  type="button"
                  onClick={() => removeHop(i)}
                  className="rounded-md bg-red-600/70 hover:bg-red-500 px-2 py-1.5 text-xs"
                >
                  移除
                </button>
              )}
            </div>
          ))}
        </div>
        <button
          type="button"
          onClick={() => setChain((c) => [...c, ''])}
          className="mt-2 rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-2.5 py-1 text-xs"
        >
          + 添加节点
        </button>
        <p className="mt-1.5 text-[11px] text-zinc-400">
          第 2 跳起的节点必须配置公网 IP；所有节点须在线
        </p>
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
          {submitting ? '提交中…' : '创建'}
        </button>
      </div>
    </form>
  )
}
