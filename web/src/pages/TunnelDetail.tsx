import { useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import {
  ApiError,
  nodes,
  rules,
  shortTime,
  statusLabel,
  tunnels,
  type GrantedUser,
  type TunnelDetailView,
} from '../lib/api'
import { ErrorBox, Modal, PageLoading, StatusDot } from '../lib/ui'
import { DiagnosePanel } from '../components/DiagnosePanel'
import { useToast } from '../lib/use-toast'
import { useAutoRefresh } from '../lib/use-auto-refresh'

interface State {
  detail: TunnelDetailView | null
  nodeNames: Map<number, string>
  loading: boolean
  error: string | null
}

// ordinal 0 → Entry；最后一个 → Exit；其余 → Mid。
function hopRole(ordinal: number, total: number): string {
  if (ordinal === 0) return 'Entry'
  if (ordinal === total - 1) return 'Exit'
  return 'Mid'
}

function tunnelStatusKind(status: TunnelDetailView['status']): 'on' | 'off' | 'unknown' {
  if (status === 'up') return 'on'
  if (status === 'down') return 'off'
  return 'unknown'
}

export default function TunnelDetail() {
  const { id } = useParams<{ id: string }>()
  const tunnelId = id ? Number(id) : NaN
  const toast = useToast()
  const [state, setState] = useState<State>({
    detail: null,
    nodeNames: new Map(),
    loading: true,
    error: null,
  })
  const [restarting, setRestarting] = useState(false)
  // 重启是破坏性操作(瞬断隧道上所有规则转发):与列表页一致,加二次确认弹窗。
  const [restartConfirm, setRestartConfirm] = useState(false)
  // 导出走一次 fetch+blob,规则量大/网络慢时有可感知延迟:进行中态 + 防连点。
  const [exporting, setExporting] = useState(false)
  // P7:该隧道被授权给哪些用户(admin-only 端点;本页路由已 admin-only)。null = 未加载。
  const [grantedUsers, setGrantedUsers] = useState<GrantedUser[] | null>(null)
  // 15s 静默刷新 hop 心跳聚合状态(隧道 up/degraded/down 变化较快)。
  const [refreshTick, setRefreshTick] = useState(0)
  useAutoRefresh(() => setRefreshTick((n) => n + 1), 15_000)

  // 授权用户列表只拉一次(变更入口在用户编辑弹窗,本页只读展示)。
  useEffect(() => {
    if (!Number.isFinite(tunnelId)) return
    let cancelled = false
    tunnels
      .grants(tunnelId)
      .then((g) => {
        if (!cancelled) setGrantedUsers(g)
      })
      .catch(() => {
        // 加载失败不阻塞详情页,该区块显示「—」。
      })
    return () => {
      cancelled = true
    }
  }, [tunnelId])

  useEffect(() => {
    let cancelled = false
    const work = Number.isFinite(tunnelId)
      ? Promise.all([
          tunnels.get(tunnelId),
          nodes.list({ page_size: 100 }),
        ])
      : Promise.reject(new Error('无效的隧道 ID'))

    work
      .then(([detail, nodeList]) => {
        if (cancelled) return
        const nodeNames = new Map<number, string>(nodeList.items.map((n) => [n.id, n.name]))
        setState({ detail, nodeNames, loading: false, error: null })
      })
      .catch((e: unknown) => {
        if (cancelled) return
        const msg =
          e instanceof ApiError ? e.message : e instanceof Error ? e.message : '加载失败'
        // 静默刷新失败不打扰:同一隧道已有数据则保留。
        setState((prev) =>
          prev.detail && prev.detail.id === tunnelId
            ? prev
            : { detail: null, nodeNames: new Map(), loading: false, error: msg },
        )
      })
    return () => {
      cancelled = true
    }
  }, [tunnelId, refreshTick])

  async function doRestart() {
    if (!Number.isFinite(tunnelId)) return
    setRestarting(true)
    try {
      await tunnels.restart(tunnelId)
      setRestartConfirm(false)
      toast.success('隧道重启指令已下发')
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : '重启失败')
    } finally {
      setRestarting(false)
    }
  }

  if (state.loading) return <PageLoading />
  if (state.error)
    return (
      <div className="space-y-4">
        <Link to="/tunnels" className="text-xs text-zinc-400 hover:text-zinc-200">← 返回隧道列表</Link>
        <ErrorBox message={state.error} onRetry={() => setRefreshTick((n) => n + 1)} />
      </div>
    )
  if (!state.detail) return null

  const { detail, nodeNames } = state

  return (
    <div className="space-y-6">
      {/* 头部 */}
      <div className="flex items-start justify-between gap-3">
        <div>
          <Link to="/tunnels" className="text-xs text-zinc-400 hover:text-zinc-200">← 返回隧道列表</Link>
          <h2 className="mt-1 text-xl font-semibold tracking-tight">{detail.name}</h2>
          <p className="text-sm text-zinc-400">
            <span className="inline-flex items-center gap-1.5 mr-3">
              <StatusDot kind={tunnelStatusKind(detail.status)} />
              {statusLabel(detail.status)}
            </span>
            <span className="uppercase text-xs mr-3">{detail.transport}</span>
            <span className="text-xs mr-3">
              计费 {detail.billing_mode === 1 ? '单向' : '双向'} × {detail.traffic_ratio}
            </span>
            ID #{detail.id} · 创建 {shortTime(detail.created_at)}
          </p>
        </div>
        <div className="flex gap-2 shrink-0">
          {/* P9: 导出本隧道关联规则(隧道规则导入时需手动重建关联,文件中含 tunnel_name 供识别)。 */}
          <button
            type="button"
            disabled={exporting}
            onClick={async () => {
              setExporting(true)
              try {
                await rules.exportDownload({ tunnel_id: tunnelId })
                toast.success('已导出。注意:隧道关联无法随导入自动重建,导入后需手动重新关联隧道')
              } catch (e) {
                toast.error(e instanceof ApiError ? e.message : '导出失败')
              } finally {
                setExporting(false)
              }
            }}
            className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 disabled:opacity-60 disabled:cursor-not-allowed px-3 py-2 text-sm"
          >
            {exporting ? '导出中…' : '导出规则'}
          </button>
          <button
            type="button"
            onClick={() => setRestartConfirm(true)}
            disabled={restarting}
            className="rounded-lg bg-amber-600/80 hover:bg-amber-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
          >
            {restarting ? '重启中…' : '重启隧道'}
          </button>
        </div>
      </div>

      {/* hop 链表 */}
      <section className="glass-card rise overflow-hidden">
        <h3 className="px-5 py-3 text-sm font-medium text-zinc-200 border-b border-white/5">节点链</h3>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead className="text-xs uppercase text-zinc-400 bg-white/[0.03]">
              <tr>
                <th scope="col" className="px-4 py-2.5 text-left font-medium">序号</th>
                <th scope="col" className="px-4 py-2.5 text-left font-medium">角色</th>
                <th scope="col" className="px-4 py-2.5 text-left font-medium">节点</th>
                <th scope="col" className="px-4 py-2.5 text-left font-medium">中继端口</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-white/5">
              {detail.hops.map((hop) => (
                <tr key={hop.ordinal} className="hover:bg-white/[0.02]">
                  <td className="px-4 py-3 text-zinc-400">{hop.ordinal}</td>
                  <td className="px-4 py-3">
                    <HopRoleBadge role={hopRole(hop.ordinal, detail.hops.length)} />
                  </td>
                  <td className="px-4 py-3 text-zinc-200">
                    {nodeNames.get(hop.node_id) ?? `#${hop.node_id}`}
                  </td>
                  <td className="px-4 py-3 text-zinc-400 font-mono text-xs">
                    {hop.inter_port ?? '-'}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      {/* P7:已授权用户(在「用户」页编辑授权;被授权用户可在被授权隧道上自建规则) */}
      <section className="glass-card rise p-5">
        <h3 className="text-sm font-medium text-zinc-200 mb-3">已授权用户</h3>
        {grantedUsers == null ? (
          <span className="text-xs text-zinc-400">—</span>
        ) : grantedUsers.length === 0 ? (
          <span className="text-xs text-zinc-400">无（普通用户默认不可用本隧道）</span>
        ) : (
          <div className="flex flex-wrap gap-1.5">
            {grantedUsers.map((u) => (
              <span
                key={u.id}
                className="inline-flex items-center rounded-md border border-white/10 bg-white/5 px-2 py-0.5 text-xs text-zinc-200"
              >
                {u.username}
              </span>
            ))}
          </div>
        )}
      </section>

      {/* 关联规则表 */}
      <section className="glass-card rise overflow-hidden">
        <h3 className="px-5 py-3 text-sm font-medium text-zinc-200 border-b border-white/5">
          关联规则
          <span className="ml-2 text-xs text-zinc-400">({detail.rules_count})</span>
        </h3>
        {detail.rules.length === 0 ? (
          <div className="px-5 py-4 text-sm text-zinc-400">暂无关联规则</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-xs uppercase text-zinc-400 bg-white/[0.03]">
                <tr>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">名称</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">协议</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">监听端口</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">状态</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-white/5">
                {detail.rules.map((r) => (
                  <tr key={r.id} className="hover:bg-white/[0.02]">
                    <td className="px-4 py-3">
                      <Link
                        to={`/rules/${r.id}`}
                        className="text-[15px] font-semibold text-zinc-100 hover:text-accent"
                      >
                        {r.name}
                      </Link>
                    </td>
                    <td className="px-4 py-3 text-zinc-300 uppercase text-xs">{r.protocol}</td>
                    <td className="px-4 py-3 text-zinc-300 font-mono text-xs">{r.listen_port}</td>
                    <td className="px-4 py-3 text-xs">
                      {r.enabled ? (
                        <span className="text-emerald-400">启用</span>
                      ) : (
                        <span className="text-zinc-400">停用</span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      <DiagnosePanel run={() => tunnels.diagnose(detail.id)} />

      {restartConfirm && (
        <Modal title="重启隧道" onClose={() => !restarting && setRestartConfirm(false)} size="sm">
          <p className="text-sm text-zinc-300">
            重启隧道 <span className="text-white font-medium">{detail.name}</span> 会瞬断其上所有规则的转发，确认继续？
          </p>
          <div className="mt-4 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setRestartConfirm(false)}
              disabled={restarting}
              className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 disabled:opacity-60 px-3 py-1.5 text-sm"
            >
              取消
            </button>
            <button
              type="button"
              onClick={doRestart}
              disabled={restarting}
              className="rounded-lg bg-amber-600/80 hover:bg-amber-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-1.5 text-sm font-medium"
            >
              {restarting ? '重启中…' : '确认重启'}
            </button>
          </div>
        </Modal>
      )}
    </div>
  )
}

// hop 角色徽标
function HopRoleBadge({ role }: { role: string }) {
  const cls =
    role === 'Entry'
      ? 'bg-accent/10 text-accent'
      : role === 'Exit'
        ? 'bg-emerald-500/20 text-emerald-300'
        : 'bg-zinc-700/50 text-zinc-300'
  return (
    <span className={`rounded-md px-2 py-0.5 text-xs font-medium ${cls}`}>{role}</span>
  )
}
