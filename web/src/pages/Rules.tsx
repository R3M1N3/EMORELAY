import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'
import {
  ApiError,
  formatBytes,
  nodes,
  rules,
  shortTime,
  type CreateRuleRequest,
  type NodeView,
  type RuleView,
  type UpdateRuleRequest,
} from '../lib/api'
import { Modal, StatusDot, fieldInputCls, fieldLabelCls } from '../lib/ui'
import { Pagination } from '../components/Pagination'

type Editing = { mode: 'create' } | { mode: 'edit'; rule: RuleView } | null

interface Filters {
  node_id: string
  protocol: string
  search: string
}

interface ListState {
  items: RuleView[]
  total: number
  loading: boolean
  error: string | null
}

export default function Rules() {
  const [list, setList] = useState<ListState>({ items: [], total: 0, loading: true, error: null })
  const [nodeList, setNodeList] = useState<NodeView[]>([])
  const [filters, setFilters] = useState<Filters>({ node_id: '', protocol: '', search: '' })
  const [editing, setEditing] = useState<Editing>(null)
  const [confirming, setConfirming] = useState<RuleView | null>(null)
  const [actingId, setActingId] = useState<number | null>(null)
  const [busy, setBusy] = useState(false)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)

  const nodesById = useMemo(() => new Map(nodeList.map((n) => [n.id, n])), [nodeList])

  async function reload() {
    setList((s) => ({ ...s, loading: true, error: null }))
    try {
      const r = await rules.list({
        page,
        page_size: pageSize,
        node_id: filters.node_id ? Number(filters.node_id) : undefined,
        protocol: filters.protocol || undefined,
        search: filters.search.trim() || undefined,
      })
      setList({ items: r.items, total: r.total, loading: false, error: null })
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '加载失败'
      setList({ items: [], total: 0, loading: false, error: msg })
    }
  }

  // 节点列表只加载一次（创建/编辑表单与节点筛选下拉都要用）。
  useEffect(() => {
    let cancelled = false
    nodes
      .list({ page_size: 100 })
      .then((r) => {
        if (!cancelled) setNodeList(r.items)
      })
      .catch(() => {
        // 节点拉取失败不阻塞规则列表，仅创建表单会缺下拉项。
      })
    return () => {
      cancelled = true
    }
  }, [])

  // 规则列表：筛选项 / 翻页 / pageSize 变化都重新拉取。
  // 内联 promise chain 避免 react-hooks/set-state-in-effect。
  // 操作后的 reload() 在事件回调里调用，不在 effect 内。
  useEffect(() => {
    let cancelled = false
    rules
      .list({
        page,
        page_size: pageSize,
        node_id: filters.node_id ? Number(filters.node_id) : undefined,
        protocol: filters.protocol || undefined,
        search: filters.search.trim() || undefined,
      })
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
    // search 不进 deps —— 输入框打字不触发请求；用户点「搜索」按钮显式 reload。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filters.node_id, filters.protocol, page, pageSize])

  async function doDelete(rule: RuleView) {
    setBusy(true)
    try {
      await rules.del(rule.id)
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

  async function doToggle(rule: RuleView) {
    setActingId(rule.id)
    try {
      if (rule.enabled) await rules.disable(rule.id)
      else await rules.enable(rule.id)
      await reload()
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '操作失败'
      setList((s) => ({ ...s, error: msg }))
    } finally {
      setActingId(null)
    }
  }

  async function doRestart(rule: RuleView) {
    setActingId(rule.id)
    try {
      await rules.restart(rule.id)
      await reload()
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '重启失败'
      setList((s) => ({ ...s, error: msg }))
    } finally {
      setActingId(null)
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">转发规则</h2>
          <p className="text-sm text-zinc-400 mt-1">TCP / UDP 端口转发配置与运行状态</p>
        </div>
        <button
          onClick={() => setEditing({ mode: 'create' })}
          disabled={nodeList.length === 0}
          title={nodeList.length === 0 ? '请先创建节点' : ''}
          className="rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium shrink-0"
        >
          新增规则
        </button>
      </div>

      <div className="flex flex-wrap gap-3 items-end">
        <div className="min-w-[160px]">
          <label className={fieldLabelCls}>节点</label>
          <select
            value={filters.node_id}
            onChange={(e) => {
              setFilters((f) => ({ ...f, node_id: e.target.value }))
              setPage(1)
            }}
            className={fieldInputCls}
          >
            <option value="">全部</option>
            {nodeList.map((n) => (
              <option key={n.id} value={n.id}>
                {n.name}
              </option>
            ))}
          </select>
        </div>
        <div className="min-w-[140px]">
          <label className={fieldLabelCls}>协议</label>
          <select
            value={filters.protocol}
            onChange={(e) => {
              setFilters((f) => ({ ...f, protocol: e.target.value }))
              setPage(1)
            }}
            className={fieldInputCls}
          >
            <option value="">全部</option>
            <option value="tcp">TCP</option>
            <option value="udp">UDP</option>
            <option value="tcp_udp">TCP+UDP</option>
          </select>
        </div>
        <form
          onSubmit={(e) => {
            e.preventDefault()
            // 搜索时回到第 1 页;page 已是 1 时 reload 也能拿到正确结果。
            if (page !== 1) setPage(1)
            else reload()
          }}
          className="flex-1 min-w-[220px] flex items-end gap-2"
        >
          <div className="flex-1">
            <label className={fieldLabelCls}>搜索</label>
            <input
              value={filters.search}
              onChange={(e) => setFilters((f) => ({ ...f, search: e.target.value }))}
              placeholder="规则名 / 端口 / 目标主机"
              className={fieldInputCls}
            />
          </div>
          <button
            type="submit"
            className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm"
          >
            搜索
          </button>
        </form>
      </div>

      {list.error && (
        <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-200">
          {list.error}
        </div>
      )}

      <section className="rounded-2xl border border-white/10 bg-zinc-900/40 overflow-hidden">
        {list.loading ? (
          <div className="p-6 text-sm text-zinc-400">加载中…</div>
        ) : list.items.length === 0 ? (
          <div className="p-6 text-sm text-zinc-500">
            {filters.node_id || filters.protocol || filters.search
              ? '当前筛选条件下没有规则。'
              : '尚无规则。点击右上角「新增规则」开始。'}
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-[11px] uppercase text-zinc-500 bg-zinc-900/80">
                <tr>
                  <th className="px-4 py-2.5 text-left font-medium">名称</th>
                  <th className="px-4 py-2.5 text-left font-medium">节点 / 协议</th>
                  <th className="px-4 py-2.5 text-left font-medium">监听</th>
                  <th className="px-4 py-2.5 text-left font-medium">目标</th>
                  <th className="px-4 py-2.5 text-left font-medium">状态</th>
                  <th className="px-4 py-2.5 text-left font-medium">流量 / 连接</th>
                  <th className="px-4 py-2.5 text-right font-medium">操作</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-white/5">
                {list.items.map((r) => (
                  <RuleRow
                    key={r.id}
                    rule={r}
                    node={nodesById.get(r.node_id)}
                    acting={actingId === r.id}
                    onEdit={() => setEditing({ mode: 'edit', rule: r })}
                    onDelete={() => setConfirming(r)}
                    onToggle={() => doToggle(r)}
                    onRestart={() => doRestart(r)}
                  />
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
          title={editing.mode === 'create' ? '新增规则' : `编辑规则 · ${editing.rule.name}`}
          onClose={() => setEditing(null)}
          size="lg"
        >
          <RuleForm
            mode={editing.mode}
            initial={editing.mode === 'edit' ? editing.rule : undefined}
            nodeList={nodeList}
            onCancel={() => setEditing(null)}
            onSuccess={async () => {
              setEditing(null)
              await reload()
            }}
          />
        </Modal>
      )}

      {confirming && (
        <Modal title="删除规则" onClose={() => !busy && setConfirming(null)} size="sm">
          <p className="text-sm text-zinc-300">
            将删除规则 <span className="text-white font-medium">{confirming.name}</span>
            （监听 {confirming.listen_ip}:{confirming.listen_port}）。
            Agent 上对应端口将立即停止监听，请确认。
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
    </div>
  )
}

function RuleRow({
  rule,
  node,
  acting,
  onEdit,
  onDelete,
  onToggle,
  onRestart,
}: {
  rule: RuleView
  node: NodeView | undefined
  acting: boolean
  onEdit: () => void
  onDelete: () => void
  onToggle: () => void
  onRestart: () => void
}) {
  const protoLabel = rule.protocol === 'tcp_udp' ? 'TCP+UDP' : rule.protocol.toUpperCase()
  return (
    <tr className="hover:bg-white/[0.02]">
      <td className="px-4 py-3 align-top">
        <Link
          to={`/rules/${rule.id}`}
          className="font-medium text-zinc-100 hover:text-indigo-300"
        >
          {rule.name}
        </Link>
        <div className="text-[11px] text-zinc-500 mt-0.5">ID #{rule.id}</div>
      </td>
      <td className="px-4 py-3 align-top text-zinc-300">
        <div>{node?.name ?? `节点 #${rule.node_id}`}</div>
        <div className="text-[11px] text-zinc-500 mt-0.5">{protoLabel}</div>
      </td>
      <td className="px-4 py-3 align-top text-zinc-300 font-mono text-[12px]">
        {rule.listen_ip}:{rule.listen_port}
      </td>
      <td className="px-4 py-3 align-top text-zinc-300 font-mono text-[12px]">
        {rule.target_host}:{rule.target_port}
      </td>
      <td className="px-4 py-3 align-top">
        <span className="inline-flex items-center gap-1.5 text-xs text-zinc-300">
          <StatusDot kind={rule.enabled ? 'on' : 'off'} />
          {rule.enabled ? '启用' : '禁用'}
        </span>
        {rule.expires_at && (
          <div className="text-[11px] text-zinc-500 mt-0.5">到期 {shortTime(rule.expires_at)}</div>
        )}
      </td>
      <td className="px-4 py-3 align-top text-[12px] text-zinc-300">
        <div>↓ {formatBytes(rule.rx_bytes)}</div>
        <div>↑ {formatBytes(rule.tx_bytes)}</div>
        <div className="text-[11px] text-zinc-500 mt-0.5">连接 {rule.connection_count}</div>
      </td>
      <td className="px-4 py-3 align-top text-right whitespace-nowrap">
        <button
          type="button"
          onClick={onToggle}
          disabled={acting}
          className="rounded-md bg-zinc-800 hover:bg-zinc-700 disabled:opacity-60 px-2.5 py-1 text-xs"
        >
          {rule.enabled ? '禁用' : '启用'}
        </button>
        <button
          type="button"
          onClick={onRestart}
          disabled={acting}
          className="ml-1.5 rounded-md bg-zinc-800 hover:bg-zinc-700 disabled:opacity-60 px-2.5 py-1 text-xs"
        >
          重启
        </button>
        <button
          type="button"
          onClick={onEdit}
          disabled={acting}
          className="ml-1.5 rounded-md bg-zinc-800 hover:bg-zinc-700 disabled:opacity-60 px-2.5 py-1 text-xs"
        >
          编辑
        </button>
        <button
          type="button"
          onClick={onDelete}
          disabled={acting}
          className="ml-1.5 rounded-md bg-red-600/80 hover:bg-red-500 disabled:opacity-60 px-2.5 py-1 text-xs"
        >
          删除
        </button>
      </td>
    </tr>
  )
}

interface RuleFormState {
  node_id: string
  name: string
  protocol: 'tcp' | 'udp' | 'tcp_udp'
  listen_ip: string
  listen_port: string
  target_host: string
  target_port: string
  expires_at: string
  traffic_limit_bytes: string
  bandwidth_limit_mbps: string
}

function RuleForm({
  mode,
  initial,
  nodeList,
  onCancel,
  onSuccess,
}: {
  mode: 'create' | 'edit'
  initial?: RuleView
  nodeList: NodeView[]
  onCancel: () => void
  onSuccess: () => void | Promise<void>
}) {
  const [form, setForm] = useState<RuleFormState>({
    node_id: initial ? String(initial.node_id) : nodeList[0] ? String(nodeList[0].id) : '',
    name: initial?.name ?? '',
    protocol: initial?.protocol ?? 'tcp',
    listen_ip: initial?.listen_ip ?? '0.0.0.0',
    listen_port: initial ? String(initial.listen_port) : '',
    target_host: initial?.target_host ?? '',
    target_port: initial ? String(initial.target_port) : '',
    expires_at: initial?.expires_at ?? '',
    traffic_limit_bytes:
      initial?.traffic_limit_bytes != null ? String(initial.traffic_limit_bytes) : '',
    bandwidth_limit_mbps:
      initial?.bandwidth_limit_mbps != null ? String(initial.bandwidth_limit_mbps) : '',
  })
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  function set<K extends keyof RuleFormState>(k: K, v: RuleFormState[K]) {
    setForm((f) => ({ ...f, [k]: v }))
  }

  function parsePort(v: string, label: string): number | string {
    const n = Number(v)
    if (!Number.isInteger(n) || n < 1 || n > 65535) return `${label} 必须是 1-65535 的整数`
    return n
  }

  function parseOptionalInt(v: string, label: string): number | null | string {
    if (v.trim() === '') return null
    const n = Number(v)
    if (!Number.isInteger(n) || n < 0) return `${label} 必须是非负整数`
    return n
  }

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)

    const listenPort = parsePort(form.listen_port, '监听端口')
    if (typeof listenPort === 'string') return setError(listenPort)
    const targetPort = parsePort(form.target_port, '目标端口')
    if (typeof targetPort === 'string') return setError(targetPort)
    const trafficLimit = parseOptionalInt(form.traffic_limit_bytes, '总流量上限')
    if (typeof trafficLimit === 'string') return setError(trafficLimit)
    const bandwidthLimit = parseOptionalInt(form.bandwidth_limit_mbps, '带宽上限')
    if (typeof bandwidthLimit === 'string') return setError(bandwidthLimit)

    setSubmitting(true)
    try {
      if (mode === 'create') {
        if (!form.node_id) {
          setError('请选择节点')
          setSubmitting(false)
          return
        }
        const payload: CreateRuleRequest = {
          node_id: Number(form.node_id),
          name: form.name.trim(),
          protocol: form.protocol,
          listen_ip: form.listen_ip.trim() || '0.0.0.0',
          listen_port: listenPort,
          target_host: form.target_host.trim(),
          target_port: targetPort,
          expires_at: form.expires_at.trim() || null,
          traffic_limit_bytes: trafficLimit,
          bandwidth_limit_mbps: bandwidthLimit,
        }
        await rules.create(payload)
      } else if (initial) {
        // 协议与所属节点不允许编辑（端口绑定语义会变），UI 上禁用了字段。
        const payload: UpdateRuleRequest = {
          name: form.name.trim() !== initial.name ? form.name.trim() : undefined,
          listen_ip:
            form.listen_ip.trim() !== initial.listen_ip ? form.listen_ip.trim() : undefined,
          listen_port: listenPort !== initial.listen_port ? listenPort : undefined,
          target_host:
            form.target_host.trim() !== initial.target_host
              ? form.target_host.trim()
              : undefined,
          target_port: targetPort !== initial.target_port ? targetPort : undefined,
          expires_at:
            (form.expires_at.trim() || null) !== initial.expires_at
              ? form.expires_at.trim() || null
              : undefined,
          traffic_limit_bytes:
            trafficLimit !== initial.traffic_limit_bytes ? trafficLimit : undefined,
          bandwidth_limit_mbps:
            bandwidthLimit !== initial.bandwidth_limit_mbps ? bandwidthLimit : undefined,
        }
        await rules.update(initial.id, payload)
      }
      await onSuccess()
    } catch (e) {
      setError(e instanceof ApiError ? e.message : '提交失败')
    } finally {
      setSubmitting(false)
    }
  }

  const selectedNode = nodeList.find((n) => String(n.id) === form.node_id)

  return (
    <form onSubmit={onSubmit} className="space-y-4">
      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className={fieldLabelCls}>节点 *</label>
          <select
            required
            value={form.node_id}
            onChange={(e) => set('node_id', e.target.value)}
            disabled={mode === 'edit'}
            className={fieldInputCls}
          >
            {nodeList.length === 0 && <option value="">无可用节点</option>}
            {nodeList.map((n) => (
              <option key={n.id} value={n.id}>
                {n.name} ({n.port_pool_min}-{n.port_pool_max})
              </option>
            ))}
          </select>
        </div>
        <div>
          <label className={fieldLabelCls}>协议 *</label>
          <select
            value={form.protocol}
            onChange={(e) => set('protocol', e.target.value as RuleFormState['protocol'])}
            disabled={mode === 'edit'}
            className={fieldInputCls}
          >
            <option value="tcp">TCP</option>
            <option value="udp">UDP</option>
            <option value="tcp_udp">TCP+UDP</option>
          </select>
        </div>
      </div>

      <div>
        <label className={fieldLabelCls}>规则名 *</label>
        <input
          required
          value={form.name}
          onChange={(e) => set('name', e.target.value)}
          className={fieldInputCls}
          placeholder="例如 game-hk-to-jp"
        />
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className={fieldLabelCls}>监听 IP</label>
          <input
            value={form.listen_ip}
            onChange={(e) => set('listen_ip', e.target.value)}
            className={fieldInputCls}
            placeholder="0.0.0.0"
          />
        </div>
        <div>
          <label className={fieldLabelCls}>
            监听端口 *
            {selectedNode && (
              <span className="ml-1 text-zinc-500 font-normal">
                {selectedNode.port_pool_min}-{selectedNode.port_pool_max}
              </span>
            )}
          </label>
          <input
            type="number"
            min={1}
            max={65535}
            required
            value={form.listen_port}
            onChange={(e) => set('listen_port', e.target.value)}
            className={fieldInputCls}
            placeholder="20000"
          />
        </div>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className={fieldLabelCls}>目标主机 *</label>
          <input
            required
            value={form.target_host}
            onChange={(e) => set('target_host', e.target.value)}
            className={fieldInputCls}
            placeholder="1.2.3.4 或 backend.example.com"
          />
        </div>
        <div>
          <label className={fieldLabelCls}>目标端口 *</label>
          <input
            type="number"
            min={1}
            max={65535}
            required
            value={form.target_port}
            onChange={(e) => set('target_port', e.target.value)}
            className={fieldInputCls}
            placeholder="443"
          />
        </div>
      </div>

      <div className="grid grid-cols-3 gap-3">
        <div>
          <label className={fieldLabelCls}>到期时间</label>
          <input
            type="datetime-local"
            value={toDatetimeLocal(form.expires_at)}
            onChange={(e) => set('expires_at', e.target.value)}
            className={fieldInputCls}
          />
        </div>
        <div>
          <label className={fieldLabelCls}>总流量 (bytes)</label>
          <input
            type="number"
            min={0}
            value={form.traffic_limit_bytes}
            onChange={(e) => set('traffic_limit_bytes', e.target.value)}
            className={fieldInputCls}
            placeholder="留空 = 不限"
          />
        </div>
        <div>
          <label className={fieldLabelCls}>带宽 (Mbps)</label>
          <input
            type="number"
            min={0}
            value={form.bandwidth_limit_mbps}
            onChange={(e) => set('bandwidth_limit_mbps', e.target.value)}
            className={fieldInputCls}
            placeholder="留空 = 不限"
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

// 后端返回 ISO 字符串（含 'T' 或空格），datetime-local input 需要 'YYYY-MM-DDTHH:mm'。
function toDatetimeLocal(s: string): string {
  if (!s) return ''
  return s.replace(' ', 'T').slice(0, 16)
}
