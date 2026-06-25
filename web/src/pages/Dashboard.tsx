import { useEffect, useState } from 'react'
import {
  actionLabel,
  formatBytes,
  formatCount,
  nodes,
  rules,
  shortTime,
  system,
  type AuditLogEntry,
  type NodeView,
  type RuleView,
  type SystemOverview,
  ApiError,
} from '../lib/api'
import { useAuth } from '../lib/use-auth'
import { useAutoRefresh } from '../lib/use-auto-refresh'
import { RegionBadge } from '../components/RegionBadge'
import UserDashboard from './UserDashboard'
import { ErrorBox, PageLoading } from '../lib/ui'

type Ov = SystemOverview | 'unavailable' | null
type RecentErrors = AuditLogEntry[] | 'loading' | 'unavailable'

interface Overview {
  nodes: NodeView[]
  rules: RuleView[]
  loading: boolean
  error: string | null
}

export default function Dashboard() {
  const { user } = useAuth()
  // 角色分流:普通用户看自助概览(自己的规则/配额),admin 看全局。
  if (user && user.role !== 'admin') return <UserDashboard />
  return <AdminDashboard />
}

function AdminDashboard() {
  const [data, setData] = useState<Overview>({ nodes: [], rules: [], loading: true, error: null })
  const [ov, setOv] = useState<Ov>(null)
  const [recentErrors, setRecentErrors] = useState<RecentErrors>('loading')
  // 30s 静默刷新:节点在线状态/流量卡片不再要求手动 F5。
  const [refreshTick, setRefreshTick] = useState(0)
  useAutoRefresh(() => setRefreshTick((n) => n + 1), 30_000)

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
    // 概览统计卡用 overview 的权威全量(节点/规则/连接/规则转发累计 + 24h),避免
    // nodes/rules 列表 100 行分页封顶导致规模 > 100 时少算;流量统一规则口径,不混用
    // 节点网卡(曾相差数百倍)。失败/加载中回退本页 ≤100 列表聚合(降级不空白)。
    system
      .overview()
      .then((o) => {
        if (!cancelled) setOv(o)
      })
      .catch(() => {
        if (!cancelled) setOv('unavailable')
      })
    Promise.all([nodes.list({ page_size: 100 }), rules.list({ page_size: 100 })])
      .then(([n, r]) => {
        if (cancelled) return
        setData({ nodes: n.items, rules: r.items, loading: false, error: null })
      })
      .catch((e: unknown) => {
        if (cancelled) return
        const msg = e instanceof ApiError ? e.message : '加载失败'
        // 静默刷新失败不打扰:已有数据则保留,等下个周期自愈;仅首载落错误态。
        setData((prev) => (prev.loading ? { nodes: [], rules: [], loading: false, error: msg } : prev))
      })
    return () => {
      cancelled = true
    }
    // refreshTick 驱动周期重拉;首次挂载 tick=0 也执行。
  }, [refreshTick])

  if (data.loading) return <PageLoading />
  if (data.error)
    return (
      <ErrorBox
        message={data.error}
        onRetry={() => {
          setData((d) => ({ ...d, loading: true, error: null }))
          setRefreshTick((n) => n + 1)
        }}
      />
    )

  // overview 拉到则用其权威全量;失败/加载中回退本页 ≤100 列表聚合(降级不空白)。
  const ov0 = ov && typeof ov === 'object' ? ov : null
  const onlineNodes = data.nodes.filter((n) => n.status === 'online').length
  const totalRx = data.rules.reduce((s, r) => s + r.rx_bytes, 0)
  const totalTx = data.rules.reduce((s, r) => s + r.tx_bytes, 0)
  const totalConn = data.rules.reduce((s, r) => s + r.connection_count, 0)
  const enabledRules = data.rules.filter((r) => r.enabled).length
  // overview 有则用规则口径权威值,缺字段则回退本页 ≤100 聚合;提取避免 value/hint 重复同一回退表达式。
  const ruleRx = ov0?.rule_rx_bytes_total ?? totalRx
  const ruleTx = ov0?.rule_tx_bytes_total ?? totalTx

  const today = ov0
    ? `${formatBytes(ov0.rx_bytes_24h + ov0.tx_bytes_24h)}`
    : ov === 'unavailable'
    ? '—'
    : '…'
  const todayHint = ov0
    ? `↓${formatBytes(ov0.rx_bytes_24h)} ↑${formatBytes(ov0.tx_bytes_24h)} · 仅规则转发字节`
    : ov === 'unavailable'
    ? '暂无数据'
    : '聚合中'

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-xl font-semibold tracking-tight">概览</h2>
        <p className="text-sm text-zinc-400 mt-1">节点 / 规则 / 流量实时状态</p>
      </div>

      <div className="grid grid-cols-2 lg:grid-cols-5 gap-4">
        <Stat label="总节点数" value={ov0 ? ov0.total_nodes : data.nodes.length} hint={`${ov0 ? ov0.online_nodes : onlineNodes} 在线`} accent="indigo" />
        <Stat label="转发规则" value={ov0 ? ov0.total_rules : data.rules.length} hint={`${ov0 ? ov0.enabled_rules : enabledRules} 启用`} accent="violet" />
        <Stat label="总连接数" value={formatCount(ov0?.total_connections ?? totalConn)} hint="累计" accent="emerald" />
        <Stat label="总转发流量" value={formatBytes(ruleRx + ruleTx)} hint={`规则转发累计 ↓${formatBytes(ruleRx)} ↑${formatBytes(ruleTx)}`} accent="amber" />
        <Stat label="24h 转发流量" value={today} hint={todayHint} accent="sky" />
      </div>

      <section className="glass-card rise p-5">
        <h3 className="text-sm font-medium text-zinc-200 mb-3">节点状态</h3>
        {data.nodes.length === 0 ? (
          <p className="text-sm text-zinc-400">尚无节点。前往节点页添加。</p>
        ) : (
          <div className="space-y-2">
            {data.nodes.map((n) => (
              <NodeRow key={n.id} node={n} />
            ))}
          </div>
        )}
      </section>

      {/* C3: 宽屏下「流量 Top 规则」与「最近错误」并排两栏,消除右半幅纵向留白;
          节点状态保持全宽(NodeRow 横向信息多)。窄屏(<xl)仍纵向堆叠。 */}
      <div className="grid gap-6 xl:grid-cols-2 items-start">
        {data.rules.length > 0 && (
          <section className="glass-card rise p-5">
            <h3 className="text-sm font-medium text-zinc-200 mb-3">流量 Top 规则</h3>
            <div className="space-y-2">
              {[...data.rules]
                .sort((a, b) => b.rx_bytes + b.tx_bytes - (a.rx_bytes + a.tx_bytes))
                .slice(0, 5)
                .map((r) => (
                  <div
                    key={r.id}
                    className="flex items-center justify-between gap-3 rounded-lg border border-white/5 bg-white/[0.03] px-3 py-2 text-sm"
                  >
                    <span className="truncate font-medium">{r.name}</span>
                    <span className="shrink-0 text-[11px] text-zinc-400 tabular-nums">
                      ↓{formatBytes(r.rx_bytes)} ↑{formatBytes(r.tx_bytes)}
                    </span>
                  </div>
                ))}
            </div>
          </section>
        )}

        <section className="glass-card rise p-5">
          <div className="mb-3">
            <h3 className="text-sm font-medium text-zinc-200">最近错误</h3>
            <p className="mt-0.5 text-[11px] text-zinc-400">来自审计日志的失败操作记录</p>
          </div>
          {recentErrors === 'loading' ? (
            <p className="text-sm text-zinc-400">加载中…</p>
          ) : recentErrors === 'unavailable' ? (
            <p className="text-sm text-zinc-400">暂无数据</p>
          ) : recentErrors.length === 0 ? (
            <p className="text-sm text-zinc-400">最近无错误。</p>
          ) : (
            <div className="space-y-2">
              {recentErrors.map((e) => (
                <ErrorRow key={e.id} entry={e} />
              ))}
            </div>
          )}
        </section>
      </div>
    </div>
  )
}

const ACCENT: Record<string, string> = {
  indigo: 'from-accent/15 ring-accent/30',
  violet: 'from-violet-500/15 ring-violet-500/30',
  emerald: 'from-emerald-500/15 ring-emerald-500/30',
  amber: 'from-amber-500/15 ring-amber-500/30',
  sky: 'from-sky-500/15 ring-sky-500/30',
}

// 导出给 UserDashboard 复用同款统计卡。
export function Stat({ label, value, hint, accent }: { label: string; value: number | string; hint: string; accent: keyof typeof ACCENT }) {
  return (
    <div className={`relative rounded-2xl border border-white/10 bg-gradient-to-br ${ACCENT[accent]} to-zinc-900/40 p-4 ring-1 ring-inset`}>
      <div className="text-xs text-zinc-400">{label}</div>
      <div className="mt-1 text-2xl font-semibold tracking-tight">{value}</div>
      <div className="mt-1 text-[11px] text-zinc-400 leading-tight">{hint}</div>
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
        <div className="text-sm font-medium truncate text-red-200" title={entry.action}>
          {actionLabel(entry.action)}{' '}
          {target && <span className="ml-1 text-[11px] text-red-300/70">{target}</span>}
        </div>
        <div className="text-[11px] text-zinc-400 truncate">
          {entry.error_message ?? '(无消息)'}
        </div>
      </div>
      <div className="text-[11px] text-zinc-400 shrink-0">{shortTime(entry.created_at)}</div>
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
    <div className="flex items-center justify-between rounded-lg border border-white/5 bg-white/[0.03] px-3 py-2">
      <div className="flex items-center gap-3 min-w-0">
        <span className={`inline-block h-2 w-2 rounded-full shadow ${dot}`} aria-hidden />
        <span className="sr-only">{node.status === 'online' ? '在线' : node.status === 'offline' ? '离线' : '状态未知'}</span>
        <div className="min-w-0">
          <div className="text-sm font-medium truncate">{node.name}</div>
          <div className="text-[11px] text-zinc-400 truncate"><RegionBadge region={node.region} /> · {node.public_ip || '未填'}</div>
        </div>
      </div>
      <div className="flex items-center gap-4 text-[11px] text-zinc-400 shrink-0">
        {/* 离线节点资源是掉线前陈旧采样,不当现值展示。 */}
        {node.status === 'online' ? (
          <>
            <span>CPU {node.cpu_usage.toFixed(1)}%</span>
            <span>MEM {node.memory_usage.toFixed(1)}%</span>
            <span>LOAD {node.load_average.toFixed(2)}</span>
          </>
        ) : (
          <span className="text-zinc-500">离线</span>
        )}
      </div>
    </div>
  )
}
