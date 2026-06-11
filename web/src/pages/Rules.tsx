import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'
import { useToast } from '../lib/use-toast'
import { useAuth } from '../lib/use-auth'
import {
  ApiError,
  bandwidthProfiles,
  formatBytes,
  nodes,
  rules,
  tunnels,
  users,
  type BandwidthProfileView,
  type CreateRuleRequest,
  type ImportReport,
  type NodeView,
  type RuleExportItem,
  type RuleView,
  type TunnelView,
  type UpdateRuleRequest,
  type UserDetail,
} from '../lib/api'
import { Modal, StatusDot, fieldInputCls, fieldLabelCls } from '../lib/ui'
import { Pagination } from '../components/Pagination'
import { useAutoRefresh } from '../lib/use-auto-refresh'

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
  const toast = useToast()
  const { user } = useAuth()
  const isAdmin = user?.role === 'admin'
  const [list, setList] = useState<ListState>({ items: [], total: 0, loading: true, error: null })
  const [nodeList, setNodeList] = useState<NodeView[]>([])
  const [profileList, setProfileList] = useState<BandwidthProfileView[]>([])
  const [tunnelList, setTunnelList] = useState<TunnelView[]>([])
  const [userList, setUserList] = useState<UserDetail[]>([])
  const [filters, setFilters] = useState<Filters>({ node_id: '', protocol: '', search: '' })
  const [editing, setEditing] = useState<Editing>(null)
  const [confirming, setConfirming] = useState<RuleView | null>(null)
  const [importing, setImporting] = useState<{
    items: RuleExportItem[]
    report: ImportReport
    strategy: 'skip' | 'overwrite'
    submitting: boolean
  } | null>(null)
  // 策略切换重跑 dry-run 期间为 true,禁用 radio 与确认按钮,防止用旧策略提交。
  const [refreshing, setRefreshing] = useState(false)
  const [actingId, setActingId] = useState<number | null>(null)
  const [busy, setBusy] = useState(false)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)

  const nodesById = useMemo(() => new Map(nodeList.map((n) => [n.id, n])), [nodeList])

  async function reload(opts: { silent?: boolean } = {}) {
    if (!opts.silent) setList((s) => ({ ...s, loading: true, error: null }))
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
      if (opts.silent) return
      const msg = e instanceof ApiError ? e.message : '加载失败'
      setList({ items: [], total: 0, loading: false, error: msg })
    }
  }

  // 流量/连接数列随 Agent 上报变化,30s 静默刷新(保留当前筛选与分页)。
  useAutoRefresh(() => {
    void reload({ silent: true })
  }, 30_000)

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

  // 限速配置列表只加载一次（admin-only 端点;创建/编辑表单下拉用）。
  useEffect(() => {
    if (user?.role !== 'admin') return
    let cancelled = false
    bandwidthProfiles
      .list({ page_size: 100 })
      .then((r) => {
        if (!cancelled) setProfileList(r.items)
      })
      .catch(() => {
        // 拉取失败仅创建表单缺下拉项,不阻塞规则列表。
      })
    return () => {
      cancelled = true
    }
  }, [user?.role])

  // 用户列表(admin 表单「归属用户」下拉;>100 用户时下拉不全,表单内有提示)。
  useEffect(() => {
    if (user?.role !== 'admin') return
    let cancelled = false
    users
      .list({ page_size: 100 })
      .then((r) => {
        if (!cancelled) setUserList(r.items)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [user?.role])

  // 隧道列表只加载一次（admin 权限才有，创建规则时关联隧道用）。
  useEffect(() => {
    if (user?.role !== 'admin') return
    let cancelled = false
    tunnels
      .list({ page_size: 100 })
      .then((r) => { if (!cancelled) setTunnelList(r.items) })
      .catch(() => {}) // 非关键数据，失败静默（表单退化为无隧道下拉）
    return () => { cancelled = true }
  }, [user?.role])

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
      toast.success('规则已删除')
      setConfirming(null)
      await reload()
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '删除失败'
      toast.error(msg)
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
      toast.success(rule.enabled ? '已禁用' : '已启用')
      await reload()
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '操作失败'
      toast.error(msg)
    } finally {
      setActingId(null)
    }
  }

  async function doRestart(rule: RuleView) {
    setActingId(rule.id)
    try {
      await rules.restart(rule.id)
      toast.success('已下发重启')
      await reload()
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '重启失败'
      toast.error(msg)
    } finally {
      setActingId(null)
    }
  }

  async function onImportFile(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0]
    e.target.value = '' // 允许重复选同一文件
    if (!file) return
    let items: RuleExportItem[]
    try {
      items = JSON.parse(await file.text()) as RuleExportItem[]
      if (!Array.isArray(items)) throw new Error('not array')
    } catch {
      toast.error('文件不是合法的规则导出 JSON')
      return
    }
    if (items.length === 0) {
      toast.error('文件为空')
      return
    }
    try {
      const report = await rules.importRules(items, 'skip', true)
      setImporting({ items, report, strategy: 'skip', submitting: false })
    } catch (err) {
      toast.error(err instanceof ApiError ? err.message : '预检失败')
    }
  }

  async function changeStrategy(strategy: 'skip' | 'overwrite') {
    if (!importing || refreshing) return
    setRefreshing(true)
    try {
      const report = await rules.importRules(importing.items, strategy, true)
      setImporting({ ...importing, strategy, report })
    } catch (err) {
      toast.error(err instanceof ApiError ? err.message : '预检失败')
    } finally {
      setRefreshing(false)
    }
  }

  async function confirmImport() {
    if (!importing) return
    setImporting({ ...importing, submitting: true })
    try {
      const report = await rules.importRules(importing.items, importing.strategy, false)
      const errs = report.items.filter((i) => i.action === 'error').length
      if (errs > 0) toast.error(`导入完成，${errs} 项失败`)
      else toast.success('导入完成')
      setImporting(null)
      await reload()
    } catch (err) {
      toast.error(err instanceof ApiError ? err.message : '导入失败')
      setImporting(null)
      // 后端逐项写入,无全局事务;失败也可能已部分落库,刷新列表以反映实况。
      await reload()
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">转发规则</h2>
          <p className="text-sm text-zinc-400 mt-1">TCP / UDP 端口转发配置与运行状态</p>
        </div>
        <div className="flex gap-2 shrink-0">
          {isAdmin && (
            <>
              <button
                onClick={async () => {
                  try {
                    await rules.exportDownload({
                      node_id: filters.node_id ? Number(filters.node_id) : undefined,
                    })
                    toast.success(filters.node_id ? '已导出（按节点筛选）' : '已导出全部规则')
                  } catch (e) {
                    toast.error(e instanceof ApiError ? e.message : '导出失败')
                  }
                }}
                className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm"
              >
                导出
              </button>
              <label className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm cursor-pointer">
                导入
                <input
                  type="file"
                  accept="application/json,.json"
                  className="hidden"
                  onChange={(e) => void onImportFile(e)}
                />
              </label>
            </>
          )}
          <button
            onClick={() => setEditing({ mode: 'create' })}
            disabled={nodeList.length === 0}
            title={nodeList.length === 0 ? '请先创建节点' : ''}
            className="rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium shrink-0"
          >
            新增规则
          </button>
        </div>
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
                  {isAdmin && <th className="px-4 py-2.5 text-left font-medium">归属</th>}
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
                    tunnelName={
                      // user 的 tunnelList 恒空(admin-only 端点),只会显示裸 id,不渲染。
                      isAdmin && r.tunnel_id != null
                        ? tunnelList.find((t) => t.id === r.tunnel_id)?.name ?? `#${r.tunnel_id}`
                        : null
                    }
                    showOwner={isAdmin}
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
            profiles={profileList}
            tunnelList={tunnelList}
            userList={userList}
            isAdmin={isAdmin}
            onCancel={() => setEditing(null)}
            onSuccess={async () => {
              toast.success(editing.mode === 'create' ? '规则已创建' : '规则已保存')
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

      {importing && (
        <Modal
          title={`导入预览 · ${importing.items.length} 项`}
          onClose={() => !importing.submitting && setImporting(null)}
          size="lg"
        >
          <div className="flex items-center gap-3 mb-3 text-sm">
            <span className="text-zinc-400">冲突策略:</span>
            {(['skip', 'overwrite'] as const).map((s) => (
              <label key={s} className="inline-flex items-center gap-1.5 cursor-pointer">
                <input
                  type="radio"
                  name="import-strategy"
                  checked={importing.strategy === s}
                  disabled={refreshing}
                  onChange={() => void changeStrategy(s)}
                />
                {s === 'skip' ? '跳过 (skip)' : '覆盖 (overwrite)'}
              </label>
            ))}
            {refreshing && <span className="text-zinc-500 text-xs">刷新中…</span>}
          </div>
          <div className="max-h-80 overflow-y-auto rounded-lg border border-white/10">
            <table className="w-full text-sm">
              <thead className="text-[11px] uppercase text-zinc-500 bg-zinc-900/80 sticky top-0">
                <tr>
                  <th className="px-3 py-2 text-left font-medium">#</th>
                  <th className="px-3 py-2 text-left font-medium">规则</th>
                  <th className="px-3 py-2 text-left font-medium">动作</th>
                  <th className="px-3 py-2 text-left font-medium">说明</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-white/5">
                {importing.report.items.map((it) => {
                  const src = importing.items[it.index]
                  const tone =
                    it.action === 'error'
                      ? 'text-red-300'
                      : it.action === 'skip'
                        ? 'text-zinc-400'
                        : 'text-emerald-300'
                  return (
                    <tr key={it.index}>
                      <td className="px-3 py-2 text-zinc-500">{it.index + 1}</td>
                      <td className="px-3 py-2 text-zinc-200">
                        {src?.name ?? '—'}
                        <span className="text-[11px] text-zinc-500 ml-1.5 font-mono">
                          {src ? `${src.node_name}:${src.listen_port}/${src.protocol}` : ''}
                        </span>
                      </td>
                      <td className={`px-3 py-2 ${tone}`}>{it.action}</td>
                      <td className="px-3 py-2 text-[12px] text-zinc-400">{it.reason || '—'}</td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
          <div className="mt-4 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setImporting(null)}
              disabled={importing.submitting}
              className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm"
            >
              取消
            </button>
            <button
              type="button"
              onClick={() => void confirmImport()}
              disabled={importing.submitting || refreshing}
              className="rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
            >
              {importing.submitting ? '导入中…' : refreshing ? '刷新中…' : '确认导入'}
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
  tunnelName,
  showOwner,
  acting,
  onEdit,
  onDelete,
  onToggle,
  onRestart,
}: {
  rule: RuleView
  node: NodeView | undefined
  /** 关联隧道名(null = 直连);列表页用 tunnelList 映射,无需逐行请求 */
  tunnelName: string | null
  /** admin 模式显示归属列 */
  showOwner: boolean
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
      {showOwner && (
        <td className="px-4 py-3 align-top text-zinc-300 text-[12px]">
          {rule.user_name ?? '—'}
        </td>
      )}
      <td className="px-4 py-3 align-top text-zinc-300">
        <div>{node?.name ?? `节点 #${rule.node_id}`}</div>
        <div className="text-[11px] text-zinc-500 mt-0.5">
          {protoLabel}
          {rule.bandwidth_mbps != null && ` · ${rule.bandwidth_mbps} Mbps`}
        </div>
      </td>
      <td className="px-4 py-3 align-top text-zinc-300 font-mono text-[12px]">
        {rule.listen_ip}:{rule.listen_port}
        {tunnelName != null && (
          <div className="mt-0.5 text-[10px] text-sky-300/80 font-sans">隧道 {tunnelName}</div>
        )}
      </td>
      <td className="px-4 py-3 align-top text-zinc-300 font-mono text-[12px]">
        {rule.target_host}:{rule.target_port}
      </td>
      <td className="px-4 py-3 align-top">
        <span className="inline-flex items-center gap-1.5 text-xs text-zinc-300">
          <StatusDot kind={rule.enabled ? 'on' : 'off'} />
          {rule.enabled ? '启用' : '禁用'}
        </span>
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
  bandwidth_profile_id: string
  tunnel_id: string
  user_id: string
}

export function RuleForm({
  mode,
  initial,
  nodeList,
  profiles,
  tunnelList,
  userList = [],
  isAdmin = true,
  onCancel,
  onSuccess,
}: {
  mode: 'create' | 'edit'
  initial?: RuleView
  nodeList: NodeView[]
  profiles: BandwidthProfileView[]
  tunnelList: TunnelView[]
  /** admin 归属下拉的候选(仅前 100;user 模式忽略) */
  userList?: UserDetail[]
  /** user 模式隐藏 限速/隧道/归属 字段且不发送对应 payload */
  isAdmin?: boolean
  onCancel: () => void
  onSuccess: () => void | Promise<void>
}) {
  const [form, setForm] = useState<RuleFormState>({
    node_id: initial ? String(initial.node_id) : nodeList[0] ? String(nodeList[0].id) : '',
    name: initial?.name ?? '',
    // 创建模式默认 TCP+UDP; 编辑模式沿用旧值。
    protocol: initial?.protocol ?? 'tcp_udp',
    listen_ip: initial?.listen_ip ?? '0.0.0.0',
    listen_port: initial ? String(initial.listen_port) : '',
    target_host: initial?.target_host ?? '',
    target_port: initial ? String(initial.target_port) : '',
    bandwidth_profile_id:
      initial?.bandwidth_profile_id != null ? String(initial.bandwidth_profile_id) : '',
    tunnel_id: initial?.tunnel_id != null ? String(initial.tunnel_id) : '',
    user_id: initial != null ? String(initial.user_id) : '',
  })
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // 选中隧道时锁定的入口节点 ID。
  const [entryNodeId, setEntryNodeId] = useState<number | null>(null)

  function set<K extends keyof RuleFormState>(k: K, v: RuleFormState[K]) {
    setForm((f) => ({ ...f, [k]: v }))
  }

  // 选隧道时自动填入口节点并锁定节点下拉。
  // 取消选择时的 entryNodeId 重置在 select onChange 回调里做(effect 内同步 setState 触发级联渲染)。
  useEffect(() => {
    if (!form.tunnel_id) return
    let cancelled = false
    tunnels
      .get(Number(form.tunnel_id))
      .then((d) => {
        if (cancelled) return
        const entry = d.hops.find((h) => h.ordinal === 0)
        if (entry) {
          setEntryNodeId(entry.node_id)
          setForm((f) => ({ ...f, node_id: String(entry.node_id) }))
        }
      })
      .catch(() => { if (!cancelled) setError('加载隧道入口节点失败') })
    return () => { cancelled = true }
  }, [form.tunnel_id])

  function parsePort(v: string, label: string): number | string {
    const n = Number(v)
    if (!Number.isInteger(n) || n < 1 || n > 65535) return `${label} 必须是 1-65535 的整数`
    return n
  }

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)

    let listenPort: number | undefined
    if (form.listen_port.trim() !== '') {
      const parsed = parsePort(form.listen_port, '监听端口')
      if (typeof parsed === 'string') return setError(parsed)
      listenPort = parsed
    }
    const targetPort = parsePort(form.target_port, '目标端口')
    if (typeof targetPort === 'string') return setError(targetPort)

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
        }
        // 限速/隧道/归属是 admin 管控字段;user 模式不发送(后端也会 400 拦截)。
        if (isAdmin) {
          payload.bandwidth_profile_id = form.bandwidth_profile_id
            ? Number(form.bandwidth_profile_id)
            : null
          payload.tunnel_id = form.tunnel_id ? Number(form.tunnel_id) : null
          if (form.user_id) payload.user_id = Number(form.user_id)
        }
        await rules.create(payload)
      } else if (initial) {
        // 协议与所属节点不允许编辑（端口绑定语义会变），UI 上禁用了字段。
        const payload: UpdateRuleRequest = {
          name: form.name.trim() !== initial.name ? form.name.trim() : undefined,
          listen_ip:
            form.listen_ip.trim() !== initial.listen_ip ? form.listen_ip.trim() : undefined,
          listen_port:
            listenPort !== undefined && listenPort !== initial.listen_port
              ? listenPort
              : undefined,
          target_host:
            form.target_host.trim() !== initial.target_host
              ? form.target_host.trim()
              : undefined,
          target_port: targetPort !== initial.target_port ? targetPort : undefined,
          // user 模式不发送限速字段(后端 400 拦截普通用户改限速)。
          bandwidth_profile_id:
            isAdmin &&
            (form.bandwidth_profile_id ? Number(form.bandwidth_profile_id) : 0) !==
              (initial.bandwidth_profile_id ?? 0)
              ? form.bandwidth_profile_id
                ? Number(form.bandwidth_profile_id)
                : 0
              : undefined,
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
          <label htmlFor="rule-node" className={fieldLabelCls}>节点 *</label>
          <select
            id="rule-node"
            required
            value={form.node_id}
            onChange={(e) => set('node_id', e.target.value)}
            disabled={mode === 'edit' || entryNodeId != null}
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

      {isAdmin && tunnelList.length > 0 && (
        <div>
          <label htmlFor="rule-tunnel" className={fieldLabelCls}>关联隧道</label>
          <select
            id="rule-tunnel"
            value={form.tunnel_id}
            onChange={(e) => {
              set('tunnel_id', e.target.value)
              if (!e.target.value) setEntryNodeId(null)
            }}
            disabled={mode === 'edit'}
            className={fieldInputCls}
          >
            <option value="">不走隧道</option>
            {tunnelList.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name}（{t.transport.toUpperCase()} · {t.hops_count} 跳）
              </option>
            ))}
          </select>
          <p className="text-[11px] text-zinc-500 mt-1">
            选择隧道后，规则将落在隧道入口节点，流量经隧道链转发至目标。
          </p>
        </div>
      )}

      {isAdmin && (
        <div>
          <label htmlFor="rule-owner" className={fieldLabelCls}>归属用户</label>
          <select
            id="rule-owner"
            value={form.user_id}
            onChange={(e) => set('user_id', e.target.value)}
            disabled={mode === 'edit'}
            className={fieldInputCls}
          >
            {mode === 'create' && <option value="">我自己</option>}
            {userList.map((u) => (
              <option key={u.id} value={u.id}>
                {u.username}（{u.role}）
              </option>
            ))}
            {mode === 'edit' &&
              initial &&
              !userList.some((u) => u.id === initial.user_id) && (
                <option value={String(initial.user_id)}>
                  {initial.user_name ?? `用户 #${initial.user_id}`}
                </option>
              )}
          </select>
          <p className="text-[11px] text-zinc-500 mt-1">
            {mode === 'edit'
              ? '归属创建后不可修改(可删除后以新归属重建)。'
              : userList.length >= 100
              ? '仅列出前 100 个用户;更多用户请用导入或 API 指定。'
              : '规则计入归属用户的流量配额;到期/超额时随该用户一并停用。'}
          </p>
        </div>
      )}

      <div>
        <label htmlFor="rule-name" className={fieldLabelCls}>规则名 *</label>
        <input
          id="rule-name"
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
            监听端口
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
            value={form.listen_port}
            onChange={(e) => set('listen_port', e.target.value)}
            className={fieldInputCls}
            placeholder={mode === 'create' ? '留空 = 自动分配' : '留空 = 不修改'}
          />
        </div>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div>
          <label htmlFor="rule-target-host" className={fieldLabelCls}>目标主机 *</label>
          <input
            id="rule-target-host"
            required
            value={form.target_host}
            onChange={(e) => set('target_host', e.target.value)}
            className={fieldInputCls}
            placeholder="1.2.3.4 或 backend.example.com"
          />
        </div>
        <div>
          <label htmlFor="rule-target-port" className={fieldLabelCls}>目标端口 *</label>
          <input
            id="rule-target-port"
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

      {isAdmin && (
        <div>
          <label className={fieldLabelCls}>限速配置</label>
          <select
            value={form.bandwidth_profile_id}
            onChange={(e) => set('bandwidth_profile_id', e.target.value)}
            className={fieldInputCls}
          >
            <option value="">不限速</option>
            {profiles.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}（{p.bandwidth_mbps} Mbps）
              </option>
            ))}
          </select>
          <p className="text-[11px] text-zinc-500 mt-1">
            在「限速」页维护可复用配置；到期与流量配额已移至用户维度。
          </p>
        </div>
      )}

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
