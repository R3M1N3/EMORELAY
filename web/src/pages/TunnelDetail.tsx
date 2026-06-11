import { useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import {
  ApiError,
  nodes,
  shortTime,
  tunnels,
  type TunnelDetailView,
} from '../lib/api'
import { StatusDot } from '../lib/ui'
import { useToast } from '../lib/use-toast'

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
  if (status === 'down' || status === 'degraded') return 'off'
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
        setState({ detail: null, nodeNames: new Map(), loading: false, error: msg })
      })
    return () => {
      cancelled = true
    }
  }, [tunnelId])

  async function doRestart() {
    if (!Number.isFinite(tunnelId)) return
    setRestarting(true)
    try {
      await tunnels.restart(tunnelId)
      toast.success('隧道重启指令已下发')
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : '重启失败')
    } finally {
      setRestarting(false)
    }
  }

  if (state.loading) return <div className="text-zinc-400">加载中…</div>
  if (state.error)
    return (
      <div className="space-y-4">
        <Link to="/tunnels" className="text-xs text-zinc-400 hover:text-zinc-200">← 返回隧道列表</Link>
        <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-200">
          {state.error}
        </div>
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
              {detail.status}
            </span>
            <span className="uppercase text-xs mr-3">{detail.transport}</span>
            ID #{detail.id} · 创建 {shortTime(detail.created_at)}
          </p>
        </div>
        <button
          type="button"
          onClick={doRestart}
          disabled={restarting}
          className="shrink-0 rounded-lg bg-amber-600/80 hover:bg-amber-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
        >
          {restarting ? '重启中…' : '重启隧道'}
        </button>
      </div>

      {/* hop 链表 */}
      <section className="rounded-2xl border border-white/10 bg-zinc-900/40 overflow-hidden">
        <h3 className="px-5 py-3 text-sm font-medium text-zinc-200 border-b border-white/5">节点链</h3>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead className="text-[11px] uppercase text-zinc-500 bg-zinc-900/80">
              <tr>
                <th className="px-4 py-2.5 text-left font-medium">序号</th>
                <th className="px-4 py-2.5 text-left font-medium">角色</th>
                <th className="px-4 py-2.5 text-left font-medium">节点</th>
                <th className="px-4 py-2.5 text-left font-medium">中继端口</th>
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

      {/* 关联规则表 */}
      <section className="rounded-2xl border border-white/10 bg-zinc-900/40 overflow-hidden">
        <h3 className="px-5 py-3 text-sm font-medium text-zinc-200 border-b border-white/5">
          关联规则
          <span className="ml-2 text-xs text-zinc-500">({detail.rules_count})</span>
        </h3>
        {detail.rules.length === 0 ? (
          <div className="px-5 py-4 text-sm text-zinc-500">暂无关联规则</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-[11px] uppercase text-zinc-500 bg-zinc-900/80">
                <tr>
                  <th className="px-4 py-2.5 text-left font-medium">名称</th>
                  <th className="px-4 py-2.5 text-left font-medium">协议</th>
                  <th className="px-4 py-2.5 text-left font-medium">监听端口</th>
                  <th className="px-4 py-2.5 text-left font-medium">状态</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-white/5">
                {detail.rules.map((r) => (
                  <tr key={r.id} className="hover:bg-white/[0.02]">
                    <td className="px-4 py-3">
                      <Link
                        to={`/rules/${r.id}`}
                        className="font-medium text-zinc-100 hover:text-indigo-300"
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
                        <span className="text-zinc-500">停用</span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  )
}

// hop 角色徽标
function HopRoleBadge({ role }: { role: string }) {
  const cls =
    role === 'Entry'
      ? 'bg-indigo-500/20 text-indigo-300'
      : role === 'Exit'
        ? 'bg-emerald-500/20 text-emerald-300'
        : 'bg-zinc-700/50 text-zinc-300'
  return (
    <span className={`rounded-md px-2 py-0.5 text-xs font-medium ${cls}`}>{role}</span>
  )
}
