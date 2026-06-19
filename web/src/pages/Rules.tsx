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
  type LbStrategy,
  type NodeView,
  type RuleExportItem,
  type RuleView,
  type TargetDto,
  type TunnelView,
  type UpdateRuleRequest,
  type UserDetail,
} from '../lib/api'
import { EmptyState, ErrorBox, Modal, StatusDot, TableSkeleton, fieldInputCls, fieldLabelCls } from '../lib/ui'
import { Pagination } from '../components/Pagination'
import { CopyButton } from '../components/CopyButton'
import { DiagnosePanel } from '../components/DiagnosePanel'
import { formatHostPort, nodeEntryHost } from '../lib/format-addr'
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
  const [diagnosing, setDiagnosing] = useState<RuleView | null>(null)
  const [exportOpen, setExportOpen] = useState(false)
  const [importing, setImporting] = useState<{
    items: RuleExportItem[]
    report: ImportReport
    strategy: 'skip' | 'overwrite'
    /** P9: '' = 按文件内节点名映射;数字字符串 = 全部导入到该节点 */
    targetNodeId: string
    submitting: boolean
  } | null>(null)
  // 策略切换重跑 dry-run 期间为 true,禁用 radio 与确认按钮,防止用旧策略提交。
  const [refreshing, setRefreshing] = useState(false)
  const [actingId, setActingId] = useState<number | null>(null)
  const [busy, setBusy] = useState(false)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  // P7:用户视角的 nodes/tunnels 列表即「被授权集合」,加载成功后才能判定授权撤销。
  const [nodesLoaded, setNodesLoaded] = useState(false)
  const [tunnelsLoaded, setTunnelsLoaded] = useState(false)

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
        if (cancelled) return
        setNodeList(r.items)
        setNodesLoaded(true)
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

  // 隧道列表只加载一次(P7 起对普通用户也放开,只返回被授权的隧道;表单关联下拉用)。
  // grantsLoaded:节点+隧道都拉成功才做「授权已撤销」判定,避免加载失败误标。
  useEffect(() => {
    let cancelled = false
    tunnels
      .list({ page_size: 100 })
      .then((r) => {
        if (cancelled) return
        setTunnelList(r.items)
        setTunnelsLoaded(true)
      })
      .catch(() => {}) // 非关键数据，失败静默（表单退化为无隧道下拉）
    return () => { cancelled = true }
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
      const res = await rules.del(rule.id)
      // 节点离线时软删仍成功,但数据面可能仍在转发——如实告知,由对账后续清理。
      if (res.dispatched) {
        toast.success('规则已删除')
      } else {
        toast.info('规则已删除；目标节点当前离线，将在节点恢复后自动清理')
      }
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
      setImporting({ items, report, strategy: 'skip', targetNodeId: '', submitting: false })
    } catch (err) {
      toast.error(err instanceof ApiError ? err.message : '预检失败')
    }
  }

  // 策略或导入目标变化都重跑 dry-run(预览必须反映最终参数)。
  async function rerunPreview(strategy: 'skip' | 'overwrite', targetNodeId: string) {
    if (!importing || refreshing) return
    setRefreshing(true)
    try {
      const report = await rules.importRules(
        importing.items,
        strategy,
        true,
        targetNodeId ? Number(targetNodeId) : undefined,
      )
      setImporting({ ...importing, strategy, targetNodeId, report })
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
      const report = await rules.importRules(
        importing.items,
        importing.strategy,
        false,
        importing.targetNodeId ? Number(importing.targetNodeId) : undefined,
      )
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
                onClick={() => setExportOpen(true)}
                className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm"
              >
                导出
              </button>
              <label className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm cursor-pointer">
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
            // 只授权了隧道(无节点授权)的用户也能建隧道规则。
            disabled={nodeList.length === 0 && tunnelList.length === 0}
            title={
              nodeList.length === 0 && tunnelList.length === 0
                ? isAdmin
                  ? '请先创建节点'
                  : '暂无可用节点/隧道,请联系管理员授权'
                : ''
            }
            className="btn-accent shrink-0"
          >
            新增规则
          </button>
        </div>
      </div>

      <div className="flex flex-wrap gap-3 items-end">
        <div className="min-w-[160px]">
          <label htmlFor="rules-f-node" className={fieldLabelCls}>节点</label>
          <select
            id="rules-f-node"
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
          <label htmlFor="rules-f-protocol" className={fieldLabelCls}>协议</label>
          <select
            id="rules-f-protocol"
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
            <label htmlFor="rules-f-search" className={fieldLabelCls}>搜索</label>
            <input
              id="rules-f-search"
              value={filters.search}
              onChange={(e) => setFilters((f) => ({ ...f, search: e.target.value }))}
              placeholder="规则名 / 端口 / 目标主机"
              className={fieldInputCls}
            />
          </div>
          <button
            type="submit"
            className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm"
          >
            搜索
          </button>
        </form>
      </div>

      {list.error && <ErrorBox message={list.error} onRetry={() => void reload()} />}

      <section className="glass-card rise overflow-hidden">
        {list.loading ? (
          <TableSkeleton cols={8} />
        ) : list.items.length === 0 ? (
          filters.node_id || filters.protocol || filters.search ? (
            <EmptyState title="当前筛选条件下没有规则" hint="调整或清空筛选查看全部规则。" />
          ) : !isAdmin && nodeList.length === 0 && tunnelList.length === 0 ? (
            <EmptyState title="尚无可用资源" hint="当前没有可用的节点或隧道,请联系管理员授权后再创建规则。" />
          ) : (
            <EmptyState
              title="尚无转发规则"
              hint="创建一条 TCP/UDP 端口转发,把入口流量转到目标地址。"
              action={
                <button
                  type="button"
                  onClick={() => setEditing({ mode: 'create' })}
                  disabled={nodeList.length === 0 && tunnelList.length === 0}
                  className="btn-accent"
                >
                  新增规则
                </button>
              }
            />
          )
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-[11px] uppercase text-zinc-400 bg-white/[0.03]">
                <tr>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">名称</th>
                  {isAdmin && <th scope="col" className="px-4 py-2.5 text-left font-medium">归属</th>}
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">节点 / 协议</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">入口</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">目标</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">状态</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">流量 / 连接</th>
                  <th scope="col" className="px-4 py-2.5 text-right font-medium">操作</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-white/5">
                {list.items.map((r) => (
                  <RuleRow
                    key={r.id}
                    rule={r}
                    node={nodesById.get(r.node_id)}
                    tunnelName={
                      // P7 起用户也能拉到(被授权的)隧道列表;查不到名字时显示裸 id。
                      r.tunnel_id != null
                        ? tunnelList.find((t) => t.id === r.tunnel_id)?.name ?? `#${r.tunnel_id}`
                        : null
                    }
                    // P7 撤销授权标黄:用户视角 nodes/tunnels 列表即授权集合,
                    // 规则挂的节点/隧道不在其中 = 授权已撤销(规则保留运行,仅禁止新建)。
                    grantRevoked={
                      !isAdmin &&
                      (r.tunnel_id != null
                        ? tunnelsLoaded && !tunnelList.some((t) => t.id === r.tunnel_id)
                        : nodesLoaded && !nodesById.has(r.node_id))
                    }
                    showOwner={isAdmin}
                    acting={actingId === r.id}
                    onEdit={() => setEditing({ mode: 'edit', rule: r })}
                    onDelete={() => setConfirming(r)}
                    onToggle={() => doToggle(r)}
                    onRestart={() => doRestart(r)}
                    onDiagnose={() => setDiagnosing(r)}
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
            （入口 {formatHostPort(nodeEntryHost(nodesById.get(confirming.node_id)) || confirming.listen_ip, confirming.listen_port)}）。
            节点在线时对应端口将立即停止监听；若节点离线，规则将在其恢复后自动清理。
          </p>
          <p className="mt-2 text-[12px] text-amber-300/90">
            将一并清除该规则累计统计：↓{formatBytes(confirming.rx_bytes)} ↑{formatBytes(confirming.tx_bytes)} · 连接 {confirming.connection_count}
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

      {diagnosing && (
        <Modal
          title={`链路测延时 · ${diagnosing.name}`}
          onClose={() => setDiagnosing(null)}
          size="md"
        >
          <DiagnosePanel autoRun run={() => rules.diagnose(diagnosing.id)} />
        </Modal>
      )}

      {exportOpen && (
        <ExportModal
          nodeList={nodeList}
          tunnelList={tunnelList}
          onClose={() => setExportOpen(false)}
        />
      )}

      {importing && (
        <Modal
          title={`导入预览 · ${importing.items.length} 项`}
          onClose={() => !importing.submitting && setImporting(null)}
          size="lg"
        >
          <div className="flex items-center gap-3 mb-2 text-sm flex-wrap">
            <span className="text-zinc-400">冲突策略:</span>
            {(['skip', 'overwrite'] as const).map((s) => (
              <label key={s} className="inline-flex items-center gap-1.5 cursor-pointer">
                <input
                  type="radio"
                  name="import-strategy"
                  checked={importing.strategy === s}
                  disabled={refreshing}
                  onChange={() => void rerunPreview(s, importing.targetNodeId)}
                />
                {s === 'skip' ? '跳过 (skip)' : '覆盖 (overwrite)'}
              </label>
            ))}
            {refreshing && <span className="text-zinc-400 text-xs">刷新中…</span>}
          </div>
          {/* P9: 导入目标——按文件内节点名映射,或全部映射到指定节点。 */}
          <div className="flex items-center gap-2 mb-3 text-sm">
            <span className="text-zinc-400 shrink-0">导入目标:</span>
            <select
              aria-label="导入目标节点"
              value={importing.targetNodeId}
              disabled={refreshing}
              onChange={(e) => void rerunPreview(importing.strategy, e.target.value)}
              className={`${fieldInputCls} max-w-xs`}
            >
              <option value="">按文件内节点名映射</option>
              {nodeList.map((n) => (
                <option key={n.id} value={n.id}>
                  全部导入到 {n.name}
                </option>
              ))}
            </select>
          </div>
          {/* 归属按用户名跨实例回填,落空场景必须显式告知。 */}
          <p className="mb-3 text-[11px] text-zinc-400">
            归属按文件内用户名匹配回填（规则计入被回填用户的流量配额）；本实例不存在该用户（或老版本导出文件）时，规则归当前操作者并计入其配额。
          </p>
          <div className="max-h-80 overflow-y-auto rounded-lg border border-white/10">
            <table className="w-full text-sm">
              {/* sticky 表头必须近实底:Modal 底色变透明后,半透明表头滚动时会与行文字叠影。 */}
              <thead className="text-[11px] uppercase text-zinc-400 bg-zinc-950 sticky top-0">
                <tr>
                  <th scope="col" className="px-3 py-2 text-left font-medium">#</th>
                  <th scope="col" className="px-3 py-2 text-left font-medium">规则</th>
                  <th scope="col" className="px-3 py-2 text-left font-medium">动作</th>
                  <th scope="col" className="px-3 py-2 text-left font-medium">说明</th>
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
                      <td className="px-3 py-2 text-zinc-400">{it.index + 1}</td>
                      <td className="px-3 py-2 text-zinc-200">
                        {src?.name ?? '—'}
                        <span className="text-[11px] text-zinc-400 ml-1.5 font-mono">
                          {src
                            ? `${
                                // 选了导入目标时显示实际落点节点,避免与文件内 node_name 误导。
                                importing.targetNodeId
                                  ? nodesById.get(Number(importing.targetNodeId))?.name ??
                                    `#${importing.targetNodeId}`
                                  : src.node_name
                              }:${src.listen_port}/${src.protocol}`
                            : ''}
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
              className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm"
            >
              取消
            </button>
            <button
              type="button"
              onClick={() => void confirmImport()}
              disabled={importing.submitting || refreshing}
              className="btn-accent"
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
  grantRevoked,
  showOwner,
  acting,
  onEdit,
  onDelete,
  onToggle,
  onRestart,
  onDiagnose,
}: {
  rule: RuleView
  node: NodeView | undefined
  /** 关联隧道名(null = 直连);列表页用 tunnelList 映射,无需逐行请求 */
  tunnelName: string | null
  /** P7:规则所挂节点/隧道的授权已被撤销(规则保留运行,标黄提示) */
  grantRevoked: boolean
  /** admin 模式显示归属列 */
  showOwner: boolean
  acting: boolean
  onEdit: () => void
  onDelete: () => void
  onToggle: () => void
  onRestart: () => void
  onDiagnose: () => void
}) {
  const protoLabel = rule.protocol === 'tcp_udp' ? 'TCP+UDP' : rule.protocol.toUpperCase()
  // 入口地址 = 节点展示地址/public_ip + 监听端口;node 缺失(已删/未授权)回落绑定地址。
  const entryHost = nodeEntryHost(node) || rule.listen_ip
  return (
    <tr className={grantRevoked ? 'bg-amber-500/[0.06] hover:bg-amber-500/10' : 'hover:bg-white/[0.02]'}>
      <td className="px-4 py-3 align-top">
        <Link
          to={`/rules/${rule.id}`}
          className="font-medium text-zinc-100 hover:text-accent"
        >
          {rule.name}
        </Link>
        <div className="text-[11px] text-zinc-400 mt-0.5">ID #{rule.id}</div>
        {grantRevoked && (
          <div
            className="mt-1 inline-flex items-center rounded-md border border-amber-500/30 bg-amber-500/10 px-1.5 py-0.5 text-[10px] text-amber-300"
            title="管理员已撤销该节点/隧道的使用授权:此规则保留运行,但不能再新建同类规则。"
          >
            授权已撤销
          </div>
        )}
      </td>
      {showOwner && (
        <td className="px-4 py-3 align-top text-zinc-300 text-[12px]">
          {rule.user_name ?? '—'}
        </td>
      )}
      <td className="px-4 py-3 align-top text-zinc-300">
        <div>{node?.name ?? `节点 #${rule.node_id}`}</div>
        <div className="text-[11px] text-zinc-400 mt-0.5">
          {protoLabel}
          {rule.bandwidth_mbps != null && ` · ${rule.bandwidth_mbps} Mbps`}
        </div>
      </td>
      <td className="px-4 py-3 align-top text-zinc-300 font-mono text-[12px]">
        <span className="inline-flex items-center gap-1">
          {formatHostPort(entryHost, rule.listen_port)}
          <CopyButton
            value={formatHostPort(entryHost, rule.listen_port)}
            label="复制入口地址"
          />
        </span>
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
        <div className="text-[11px] text-zinc-400 mt-0.5">连接 {rule.connection_count}</div>
      </td>
      <td className="px-4 py-3 align-top text-right whitespace-nowrap">
        <button
          type="button"
          onClick={onDiagnose}
          className="rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-2.5 py-1 text-xs"
        >
          测延时
        </button>
        <button
          type="button"
          onClick={onToggle}
          disabled={acting}
          className="ml-1.5 rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 disabled:opacity-60 px-2.5 py-1 text-xs"
        >
          {rule.enabled ? '禁用' : '启用'}
        </button>
        <button
          type="button"
          onClick={onRestart}
          disabled={acting}
          className="ml-1.5 rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 disabled:opacity-60 px-2.5 py-1 text-xs"
        >
          重启
        </button>
        <button
          type="button"
          onClick={onEdit}
          disabled={acting}
          className="ml-1.5 rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 disabled:opacity-60 px-2.5 py-1 text-xs"
        >
          编辑
        </button>
        <button
          type="button"
          onClick={onDelete}
          disabled={acting}
          className="ml-1.5 rounded-md px-2.5 py-1 text-xs text-red-300/90 ring-1 ring-inset ring-red-500/25 hover:bg-red-500/15 hover:text-red-200 disabled:opacity-60"
        >
          删除
        </button>
      </td>
    </tr>
  )
}

// 导出范围 Modal:全部 / 指定节点 / 指定隧道。复用已支持 node_id/tunnel_id 过滤的 exportDownload。
// 显式让用户选维度(对标 flux「导出时选维度」),解决「导出入口分散/不能自选节点隧道」的体验断点。
function ExportModal({
  nodeList,
  tunnelList,
  onClose,
}: {
  nodeList: NodeView[]
  tunnelList: TunnelView[]
  onClose: () => void
}) {
  const toast = useToast()
  const [scope, setScope] = useState<'all' | 'node' | 'tunnel'>('all')
  const [nodeId, setNodeId] = useState('')
  const [tunnelId, setTunnelId] = useState('')
  const [busy, setBusy] = useState(false)

  async function doExport() {
    if (scope === 'node' && !nodeId) {
      toast.error('请选择节点')
      return
    }
    if (scope === 'tunnel' && !tunnelId) {
      toast.error('请选择隧道')
      return
    }
    setBusy(true)
    try {
      const q =
        scope === 'node'
          ? { node_id: Number(nodeId) }
          : scope === 'tunnel'
            ? { tunnel_id: Number(tunnelId) }
            : {}
      await rules.exportDownload(q)
      toast.success(
        scope === 'all'
          ? '已导出全部规则'
          : scope === 'node'
            ? '已导出该节点规则'
            : '已导出该隧道规则',
      )
      onClose()
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : '导出失败')
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal title="导出规则" onClose={() => !busy && onClose()} size="sm">
      <div className="space-y-3">
        <p className="text-[12px] text-zinc-400">选择导出范围，生成 JSON 文件下载。</p>
        <div className="flex flex-col gap-2 text-sm">
          {(
            [
              ['all', '全部规则'],
              ['node', '指定节点'],
              ['tunnel', '指定隧道'],
            ] as const
          ).map(([v, label]) => (
            <label key={v} className="inline-flex items-center gap-2 cursor-pointer">
              <input
                type="radio"
                name="export-scope"
                checked={scope === v}
                disabled={busy}
                onChange={() => setScope(v)}
              />
              {label}
            </label>
          ))}
        </div>
        {scope === 'node' && (
          <div>
            <label htmlFor="export-node" className={fieldLabelCls}>节点</label>
            <select
              id="export-node"
              value={nodeId}
              disabled={busy}
              onChange={(e) => setNodeId(e.target.value)}
              className={fieldInputCls}
            >
              <option value="">请选择节点</option>
              {nodeList.map((n) => (
                <option key={n.id} value={n.id}>{n.name}</option>
              ))}
            </select>
          </div>
        )}
        {scope === 'tunnel' && (
          <div>
            <label htmlFor="export-tunnel" className={fieldLabelCls}>隧道</label>
            <select
              id="export-tunnel"
              value={tunnelId}
              disabled={busy}
              onChange={(e) => setTunnelId(e.target.value)}
              className={fieldInputCls}
            >
              <option value="">请选择隧道</option>
              {tunnelList.map((t) => (
                <option key={t.id} value={t.id}>{t.name}</option>
              ))}
            </select>
            <p className="text-[11px] text-zinc-400 mt-1">
              隧道关联规则导入到其它实例时需手动重建隧道关联。
            </p>
          </div>
        )}
      </div>
      <div className="mt-5 flex justify-end gap-2">
        <button
          type="button"
          onClick={onClose}
          disabled={busy}
          className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm"
        >
          取消
        </button>
        <button type="button" onClick={() => void doExport()} disabled={busy} className="btn-accent">
          {busy ? '导出中…' : '导出'}
        </button>
      </div>
    </Modal>
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
  max_connections: string
  /** 向上游发送 PROXY protocol(admin 管控,仅非隧道 TCP) */
  send_proxy_protocol: boolean
  /** 额外目标,每行一个 host:port */
  extra_targets: string
  lb_strategy: LbStrategy
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
  /** user 模式隐藏 限速/归属 字段且不发送对应 payload(隧道 P7 起按授权对用户开放) */
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
    max_connections: initial?.max_connections != null ? String(initial.max_connections) : '',
    send_proxy_protocol: initial?.send_proxy_protocol ?? false,
    extra_targets: (initial?.extra_targets ?? []).map((t) => `${t.host}:${t.port}`).join('\n'),
    lb_strategy: initial?.lb_strategy ?? 'fifo',
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

  // 与后端 is_valid_target_host 同源的轻校验:合法 IP,或主机名且顶级标签非纯数字
  // (杀掉 1.2.3 / 12345 这类形似 IP 的无效输入)。IPv6 细节交后端兜底。
  function isValidTargetHostShape(host: string): boolean {
    if (!host || host.length > 253) return false
    if (host.includes(':')) return true
    const segs = host.split('.')
    if (segs.every((s) => /^\d+$/.test(s)))
      // 不允许前导零(04),与后端 IpAddr 解析口径一致,避免"前端过后端拒"。
      return segs.length === 4 && segs.every((s) => /^(0|[1-9]\d*)$/.test(s) && Number(s) <= 255)
    const segOk = segs.every(
      (s) => /^[a-zA-Z0-9-]{1,63}$/.test(s) && !s.startsWith('-') && !s.endsWith('-'),
    )
    return segOk && !/^\d+$/.test(segs[segs.length - 1])
  }

  // 解析额外目标 textarea(每行 host:端口,IPv6 用 [::1]:端口)。返回错误字符串或目标数组。
  function parseExtraTargets(text: string): TargetDto[] | string {
    const out: TargetDto[] = []
    for (const raw of text.split('\n')) {
      const line = raw.trim()
      if (!line) continue
      let host: string
      let portStr: string
      if (line.startsWith('[')) {
        const m = line.match(/^\[(.+)\]:(\d+)$/)
        if (!m) return `额外目标格式应为 [IPv6]:端口 — "${line}"`
        host = m[1]
        portStr = m[2]
      } else {
        const idx = line.lastIndexOf(':')
        if (idx <= 0) return `额外目标格式应为 host:端口 — "${line}"`
        host = line.slice(0, idx)
        portStr = line.slice(idx + 1)
      }
      const port = Number(portStr)
      if (!Number.isInteger(port) || port < 1 || port > 65535)
        return `额外目标端口非法 — "${line}"`
      if (!isValidTargetHostShape(host)) return `额外目标地址不合法 — "${host}"`
      out.push({ host, port })
    }
    return out
  }

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)

    if (!form.name.trim()) {
      setError('规则名不能为空')
      return
    }

    let listenPort: number | undefined
    if (form.listen_port.trim() !== '') {
      const parsed = parsePort(form.listen_port, '监听端口')
      if (typeof parsed === 'string') return setError(parsed)
      listenPort = parsed
    }
    const targetPort = parsePort(form.target_port, '目标端口')
    if (typeof targetPort === 'string') return setError(targetPort)
    if (!isValidTargetHostShape(form.target_host.trim()))
      return setError('目标地址不是合法 IP 或域名')

    const extraTargets = parseExtraTargets(form.extra_targets)
    if (typeof extraTargets === 'string') return setError(extraTargets)
    if (extraTargets.length > 0 && form.tunnel_id)
      return setError('隧道规则暂不支持多目标')

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
        // 限速/归属是 admin 管控字段;user 模式不发送(后端也会 400 拦截)。
        // 隧道 P7 起按授权对用户开放,user 也可发送。
        if (isAdmin) {
          payload.bandwidth_profile_id = form.bandwidth_profile_id
            ? Number(form.bandwidth_profile_id)
            : null
          if (form.user_id) payload.user_id = Number(form.user_id)
          if (form.max_connections) payload.max_connections = Number(form.max_connections)
          if (form.send_proxy_protocol) payload.send_proxy_protocol = true
        }
        payload.tunnel_id = form.tunnel_id ? Number(form.tunnel_id) : null
        if (extraTargets.length > 0) {
          payload.extra_targets = extraTargets
          payload.lb_strategy = form.lb_strategy
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
          // 同上;0 = 清除上限。
          max_connections:
            isAdmin &&
            (form.max_connections ? Number(form.max_connections) : 0) !==
              (initial.max_connections ?? 0)
              ? form.max_connections
                ? Number(form.max_connections)
                : 0
              : undefined,
          // admin 管控:PROXY protocol 开关变化才发送。
          send_proxy_protocol:
            isAdmin && form.send_proxy_protocol !== initial.send_proxy_protocol
              ? form.send_proxy_protocol
              : undefined,
        }
        // 多目标/策略变更才发(空数组 = 清空回单目标)。
        const initialExtraStr = (initial.extra_targets ?? [])
          .map((t) => `${t.host}:${t.port}`)
          .join('\n')
        if (form.extra_targets.trim() !== initialExtraStr.trim() || form.lb_strategy !== initial.lb_strategy) {
          payload.extra_targets = extraTargets
          payload.lb_strategy = form.lb_strategy
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
    <form noValidate onSubmit={onSubmit} className="space-y-4">
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
            {/* 节点不在(授权)列表时的占位项:选隧道=入口节点;编辑普通规则=节点授权已撤销。 */}
            {form.node_id && !nodeList.some((n) => String(n.id) === form.node_id) && (
              <option value={form.node_id}>
                节点 #{form.node_id}{form.tunnel_id ? '（隧道入口）' : '（授权已撤销）'}
              </option>
            )}
          </select>
        </div>
        <div>
          <label htmlFor="rule-protocol" className={fieldLabelCls}>协议 *</label>
          <select
            id="rule-protocol"
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

      {tunnelList.length > 0 && (
        <div>
          <label htmlFor="rule-tunnel" className={fieldLabelCls}>关联隧道</label>
          <select
            id="rule-tunnel"
            value={form.tunnel_id}
            onChange={(e) => {
              set('tunnel_id', e.target.value)
              if (!e.target.value) {
                setEntryNodeId(null)
                // 反选隧道时入口节点可能不在(授权)节点列表里,残留会被后端 400,重置回首个可选节点。
                set('node_id', nodeList[0] ? String(nodeList[0].id) : '')
              }
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
          <p className="text-[11px] text-zinc-400 mt-1">
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
          <p className="text-[11px] text-zinc-400 mt-1">
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

      <div>
        <label htmlFor="rule-listen-port" className={fieldLabelCls}>
          监听端口
          {selectedNode && (
            <span className="ml-1 text-zinc-400 font-normal">
              {selectedNode.port_pool_min}-{selectedNode.port_pool_max}
            </span>
          )}
        </label>
        <input
          id="rule-listen-port"
          type="number"
          min={1}
          max={65535}
          value={form.listen_port}
          onChange={(e) => set('listen_port', e.target.value)}
          className={fieldInputCls}
          placeholder={mode === 'create' ? '留空 = 自动分配' : '留空 = 不修改'}
        />
        <p className="text-[11px] text-zinc-400 mt-1">
          监听 IP 固定 0.0.0.0(所有网卡);入口地址按节点展示地址显示。
        </p>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div>
          <label htmlFor="rule-target-host" className={fieldLabelCls}>目标地址 *</label>
          <input
            id="rule-target-host"
            required
            value={form.target_host}
            onChange={(e) => set('target_host', e.target.value)}
            onPaste={(e) => {
              // 粘贴形如 host:port(单冒号,非 IPv6)时自动拆到地址+端口两框,省手动二次编辑。
              // IPv6(含 []/多冒号)与无端口纯地址一律走默认粘贴,不干预。
              const text = e.clipboardData.getData('text').trim()
              if (text.includes('[') || text.includes(']')) return
              if ((text.match(/:/g) || []).length !== 1) return
              const [host, portStr] = text.split(':')
              const port = Number(portStr)
              if (!host || !/^\d{1,5}$/.test(portStr) || port < 1 || port > 65535) return
              e.preventDefault()
              set('target_host', host)
              set('target_port', portStr)
            }}
            className={fieldInputCls}
            placeholder="1.2.3.4 或 backend.example.com"
          />
          {form.target_host.trim() !== '' &&
            !isValidTargetHostShape(form.target_host.trim()) && (
              <p aria-live="polite" className="text-[11px] text-red-300 mt-1">
                不是合法 IP 或域名（如 1.2.3.4 / backend.example.com）
              </p>
            )}
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

      {!form.tunnel_id && (
        <div>
          <label htmlFor="rule-extra-targets" className={fieldLabelCls}>
            额外目标（可选，负载均衡）
          </label>
          <textarea
            id="rule-extra-targets"
            rows={3}
            value={form.extra_targets}
            onChange={(e) => set('extra_targets', e.target.value)}
            className={`${fieldInputCls} font-mono`}
            placeholder={'每行一个 host:端口\n2.2.2.2:443\nbackend2.example.com:8080'}
          />
          <p className="text-[11px] text-zinc-400 mt-1">
            留空 = 单目标。主目标(上方) + 额外目标组成负载池;IPv6 用 [::1]:端口。
          </p>
          {form.extra_targets.trim() !== '' && (
            <div className="mt-2">
              <label htmlFor="rule-lb-strategy" className={fieldLabelCls}>负载策略</label>
              <select
                id="rule-lb-strategy"
                value={form.lb_strategy}
                onChange={(e) => set('lb_strategy', e.target.value as LbStrategy)}
                className={fieldInputCls}
              >
                <option value="fifo">主备故障转移（fifo，主目标优先）</option>
                <option value="round">轮询（round）</option>
                <option value="rand">随机（rand）</option>
                <option value="hash">客户端 IP 哈希（hash，会话粘性）</option>
              </select>
            </div>
          )}
        </div>
      )}

      {isAdmin && (
        <div className="grid grid-cols-2 gap-3">
          <div>
            <label htmlFor="rule-bw" className={fieldLabelCls}>限速配置</label>
            <select
              id="rule-bw"
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
            <p className="text-[11px] text-zinc-400 mt-1">
              在「限速」页维护可复用配置；到期与流量配额已移至用户维度。
            </p>
          </div>
          <div>
            <label htmlFor="rule-maxconn" className={fieldLabelCls}>并发连接上限</label>
            <input
              id="rule-maxconn"
              type="number"
              min={1}
              value={form.max_connections}
              onChange={(e) => set('max_connections', e.target.value)}
              className={fieldInputCls}
              placeholder="留空 = 不限"
            />
            <p className="text-[11px] text-zinc-400 mt-1">
              仅 TCP 生效;达到上限时新连接被直接断开。
            </p>
          </div>
          <div>
            <label className={fieldLabelCls}>PROXY protocol</label>
            <label className="flex items-center gap-2 text-[13px] text-zinc-200 mt-1.5 cursor-pointer">
              <input
                type="checkbox"
                checked={form.send_proxy_protocol}
                onChange={(e) => set('send_proxy_protocol', e.target.checked)}
              />
              向上游发送 PROXY protocol v1
            </label>
            <p className="text-[11px] text-zinc-400 mt-1">
              仅非隧道 TCP;让上游(如 nginx)拿到真实客户端 IP(需上游启用 proxy_protocol)。
            </p>
          </div>
        </div>
      )}

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
