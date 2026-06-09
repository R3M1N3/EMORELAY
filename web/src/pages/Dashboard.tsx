import { useEffect, useState } from 'react'
import {
  formatBytes,
  nodes,
  rules,
  shortTime,
  system,
  type AuditLogEntry,
  type NodeView,
  type RuleView,
  ApiError,
} from '../lib/api'

type Last24h = { rx: number; tx: number } | 'loading' | 'unavailable' | null
type RecentErrors = AuditLogEntry[] | 'loading' | 'unavailable'

interface Overview {
  nodes: NodeView[]
  rules: RuleView[]
  loading: boolean
  error: string | null
}

export default function Dashboard() {
  const [data, setData] = useState<Overview>({ nodes: [], rules: [], loading: true, error: null })
  const [last24h, setLast24h] = useState<Last24h>(null)
  const [recentErrors, setRecentErrors] = useState<RecentErrors>('loading')

  useEffect(() => {
    let cancelled = false
    // 最近错误独立拉取,失败不阻塞主数据(plan §6 Dashboard 第 6 项硬要求)。
    system
      .auditLogs({ result: 'failure', page_size: 10 })
      .then((r) => {
        if (!cancelled) setRecentErrors(r.items)
      })
      .catch(() => {
        if (!cancelled) setRecentErrors('unavailable')
      })
    Promise.all([nodes.list({ page_size: 100 }), rules.list({ page_size: 100 })])
      .then(async ([n, r]) => {
        if (cancelled) return
        setData({ nodes: n.items, rules: r.items, loading: false, error: null })

        // 第二阶段:对 online 节点拉 node_stats,聚合过去 24h 流量。
        // 失败/超时不阻塞主数据;Dashboard 已展示就不要因为时序图拉不到而黑屏。
        const online = n.items.filter((x) => x.status === 'online')
        if (online.length === 0) {
          if (!cancelled) setLast24h({ rx: 0, tx: 0 })
          return
        }
        if (!cancelled) setLast24h('loading')
        try {
          const results = await Promise.allSettled(online.map((x) => nodes.stats(x.id)))
          if (cancelled) return
          const cutoff = Math.floor(Date.now() / 1000) - 24 * 3600
          let rx = 0
          let tx = 0
          for (const res of results) {
            if (res.status !== 'fulfilled') continue
            for (const b of res.value.series) {
              // bucket_at 是 'YYYY-MM-DD HH:MM:SS' UTC 字符串(server 端用 UTC 写库)。
              const ts = Math.floor(
                new Date(b.bucket_at.replace(' ', 'T') + 'Z').getTime() / 1000,
              )
              if (ts < cutoff) continue
              rx += b.rx_bytes
              tx += b.tx_bytes
            }
          }
          if (!cancelled) setLast24h({ rx, tx })
        } catch {
          if (!cancelled) setLast24h('unavailable')
        }
      })
      .catch((e: unknown) => {
        if (cancelled) return
        const msg = e instanceof ApiError ? e.message : '加载失败'
        setData({ nodes: [], rules: [], loading: false, error: msg })
      })
    return () => {
      cancelled = true
    }
  }, [])

  if (data.loading) return <div className="text-zinc-400">加载中…</div>
  if (data.error)
    return (
      <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-200">
        {data.error}
      </div>
    )

  const onlineNodes = data.nodes.filter((n) => n.status === 'online').length
  const totalRx = data.rules.reduce((s, r) => s + r.rx_bytes, 0)
  const totalTx = data.rules.reduce((s, r) => s + r.tx_bytes, 0)
  const totalConn = data.rules.reduce((s, r) => s + r.connection_count, 0)
  const enabledRules = data.rules.filter((r) => r.enabled).length

  const today =
    last24h && typeof last24h === 'object'
      ? `${formatBytes(last24h.rx + last24h.tx)}`
      : last24h === 'loading'
      ? '…'
      : last24h === 'unavailable'
      ? '—'
      : '—'
  const todayHint =
    last24h && typeof last24h === 'object'
      ? `↓${formatBytes(last24h.rx)} ↑${formatBytes(last24h.tx)}`
      : last24h === 'loading'
      ? '聚合中'
      : last24h === 'unavailable'
      ? '暂无数据'
      : `${onlineNodes} 在线节点`

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-xl font-semibold tracking-tight">概览</h2>
        <p className="text-sm text-zinc-400 mt-1">节点 / 规则 / 流量实时状态</p>
      </div>

      <div className="grid grid-cols-2 lg:grid-cols-5 gap-4">
        <Stat label="总节点数" value={data.nodes.length} hint={`${onlineNodes} 在线`} accent="indigo" />
        <Stat label="转发规则" value={data.rules.length} hint={`${enabledRules} 启用`} accent="violet" />
        <Stat label="总连接数" value={totalConn} hint="累计" accent="emerald" />
        <Stat label="总流量" value={formatBytes(totalRx + totalTx)} hint={`↓${formatBytes(totalRx)} ↑${formatBytes(totalTx)}`} accent="amber" />
        <Stat label="过去 24h 流量" value={today} hint={todayHint} accent="sky" />
      </div>

      <section className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5">
        <h3 className="text-sm font-medium text-zinc-200 mb-3">节点状态</h3>
        {data.nodes.length === 0 ? (
          <p className="text-sm text-zinc-500">尚无节点。前往节点页添加。</p>
        ) : (
          <div className="space-y-2">
            {data.nodes.map((n) => (
              <NodeRow key={n.id} node={n} />
            ))}
          </div>
        )}
      </section>

      <section className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5">
        <h3 className="text-sm font-medium text-zinc-200 mb-3">最近错误</h3>
        {recentErrors === 'loading' ? (
          <p className="text-sm text-zinc-500">加载中…</p>
        ) : recentErrors === 'unavailable' ? (
          <p className="text-sm text-zinc-500">暂无数据</p>
        ) : recentErrors.length === 0 ? (
          <p className="text-sm text-zinc-500">最近无错误。</p>
        ) : (
          <div className="space-y-2">
            {recentErrors.map((e) => (
              <ErrorRow key={e.id} entry={e} />
            ))}
          </div>
        )}
      </section>
    </div>
  )
}

const ACCENT: Record<string, string> = {
  indigo: 'from-indigo-500/15 ring-indigo-500/30',
  violet: 'from-violet-500/15 ring-violet-500/30',
  emerald: 'from-emerald-500/15 ring-emerald-500/30',
  amber: 'from-amber-500/15 ring-amber-500/30',
  sky: 'from-sky-500/15 ring-sky-500/30',
}

function Stat({ label, value, hint, accent }: { label: string; value: number | string; hint: string; accent: keyof typeof ACCENT }) {
  return (
    <div className={`relative rounded-2xl border border-white/10 bg-gradient-to-br ${ACCENT[accent]} to-zinc-900/40 p-4 ring-1 ring-inset`}>
      <div className="text-xs text-zinc-400">{label}</div>
      <div className="mt-1 text-2xl font-semibold tracking-tight">{value}</div>
      <div className="mt-1 text-[11px] text-zinc-500">{hint}</div>
    </div>
  )
}

function ErrorRow({ entry }: { entry: AuditLogEntry }) {
  const target =
    entry.target_type && entry.target_id != null
      ? `${entry.target_type}#${entry.target_id}`
      : entry.target_type ?? ''
  return (
    <div className="flex items-start justify-between gap-3 rounded-lg border border-red-500/15 bg-red-500/5 px-3 py-2">
      <div className="min-w-0">
        <div className="text-sm font-medium truncate text-red-200">
          {entry.action}
          {target && <span className="ml-2 text-[11px] text-red-300/70">{target}</span>}
        </div>
        <div className="text-[11px] text-zinc-400 truncate">
          {entry.error_message ?? '(无消息)'}
        </div>
      </div>
      <div className="text-[11px] text-zinc-500 shrink-0">{shortTime(entry.created_at)}</div>
    </div>
  )
}

function NodeRow({ node }: { node: NodeView }) {
  const dot =
    node.status === 'online'
      ? 'bg-emerald-400 shadow-emerald-400/50'
      : node.status === 'offline'
      ? 'bg-zinc-500'
      : 'bg-amber-400'
  return (
    <div className="flex items-center justify-between rounded-lg border border-white/5 bg-zinc-900/60 px-3 py-2">
      <div className="flex items-center gap-3 min-w-0">
        <span className={`inline-block h-2 w-2 rounded-full shadow ${dot}`} aria-hidden />
        <div className="min-w-0">
          <div className="text-sm font-medium truncate">{node.name}</div>
          <div className="text-[11px] text-zinc-500 truncate">{node.region || '—'} · {node.public_ip || '未填'}</div>
        </div>
      </div>
      <div className="flex items-center gap-4 text-[11px] text-zinc-400 shrink-0">
        <span>CPU {node.cpu_usage.toFixed(1)}%</span>
        <span>MEM {node.memory_usage.toFixed(1)}%</span>
        <span>LOAD {node.load_average.toFixed(2)}</span>
      </div>
    </div>
  )
}
