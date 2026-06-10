import { useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import {
  ApiError,
  formatBytes,
  rules,
  shortTime,
  type RuleLogEntry,
  type RuleStatsResponse,
  type RuleView,
} from '../lib/api'
import { Sparkline } from '../components/Sparkline'
import { StatusDot } from '../lib/ui'

interface State {
  rule: RuleView | null
  stats: RuleStatsResponse | null
  logs: RuleLogEntry[]
  loading: boolean
  error: string | null
}

export default function RuleDetail() {
  const { id } = useParams<{ id: string }>()
  const ruleId = id ? Number(id) : NaN
  const [state, setState] = useState<State>({
    rule: null,
    stats: null,
    logs: [],
    loading: true,
    error: null,
  })

  useEffect(() => {
    let cancelled = false
    // invalid id 走 Promise.reject 让 setState 落在 .catch 异步路径,
    // 避免触发 react-hooks/set-state-in-effect (effect body 同步 setState 禁用)。
    const work: Promise<[RuleView, RuleStatsResponse, RuleLogEntry[]]> =
      Number.isFinite(ruleId)
        ? Promise.all([rules.get(ruleId), rules.stats(ruleId), rules.logs(ruleId)])
        : Promise.reject(new Error('无效的规则 ID'))

    work
      .then(([rule, stats, logs]) => {
        if (cancelled) return
        setState({ rule, stats, logs, loading: false, error: null })
      })
      .catch((e: unknown) => {
        if (cancelled) return
        const msg =
          e instanceof ApiError ? e.message : e instanceof Error ? e.message : '加载失败'
        setState({ rule: null, stats: null, logs: [], loading: false, error: msg })
      })
    return () => {
      cancelled = true
    }
  }, [ruleId])

  if (state.loading) return <div className="text-zinc-400">加载中…</div>
  if (state.error)
    return (
      <div className="space-y-4">
        <Link to="/rules" className="text-xs text-zinc-400 hover:text-zinc-200">← 返回规则列表</Link>
        <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-200">
          {state.error}
        </div>
      </div>
    )
  if (!state.rule || !state.stats) return null

  const { rule, stats, logs } = state
  // series 是按 bucket_at DESC 返回(server 端 ORDER BY DESC),时序图要升序。
  const seriesAsc = [...stats.series].reverse()
  const rxValues = seriesAsc.map((b) => b.rx_bytes)
  const txValues = seriesAsc.map((b) => b.tx_bytes)
  const errValues = seriesAsc.map((b) => b.error_count)

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between gap-3">
        <div>
          <Link to="/rules" className="text-xs text-zinc-400 hover:text-zinc-200">← 返回规则列表</Link>
          <h2 className="mt-1 text-xl font-semibold tracking-tight">{rule.name}</h2>
          <p className="text-sm text-zinc-400">
            <span className="inline-flex items-center gap-1.5 mr-3">
              <StatusDot kind={rule.enabled ? 'on' : 'off'} />
              {rule.enabled ? '启用' : '禁用'}
            </span>
            ID #{rule.id} · 节点 #{rule.node_id}
          </p>
        </div>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <ConfigCard rule={rule} />
        <TrafficCard stats={stats} />
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <SeriesCard
          title="过去 ~2.4 小时上下行 (bytes / 分钟)"
          rx={rxValues}
          tx={txValues}
        />
        <SeriesCard
          title="错误次数 (per 分钟)"
          rx={[]}
          tx={errValues}
          rxLabel=""
          txLabel="errors"
          txColor="stroke-rose-400"
          txFill="fill-rose-500/10"
        />
      </div>

      <LogsCard logs={logs} />
    </div>
  )
}

function ConfigCard({ rule }: { rule: RuleView }) {
  const protoLabel = rule.protocol === 'tcp_udp' ? 'TCP+UDP' : rule.protocol.toUpperCase()
  return (
    <section className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5">
      <h3 className="text-sm font-medium text-zinc-200 mb-3">配置</h3>
      <dl className="text-sm space-y-2">
        <Row k="协议" v={protoLabel} />
        <Row k="监听" v={`${rule.listen_ip}:${rule.listen_port}`} mono />
        <Row k="目标" v={`${rule.target_host}:${rule.target_port}`} mono />
        <Row k="限速" v={rule.bandwidth_mbps != null ? `${rule.bandwidth_mbps} Mbps` : '不限'} />
        <Row k="创建" v={shortTime(rule.created_at)} />
        <Row k="更新" v={shortTime(rule.updated_at)} />
      </dl>
    </section>
  )
}

function TrafficCard({ stats }: { stats: RuleStatsResponse }) {
  const { current } = stats
  return (
    <section className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5">
      <h3 className="text-sm font-medium text-zinc-200 mb-3">累计</h3>
      <div className="grid grid-cols-3 gap-3 text-sm">
        <Stat label="下行 (rx)" value={formatBytes(current.rx_bytes)} />
        <Stat label="上行 (tx)" value={formatBytes(current.tx_bytes)} />
        <Stat label="连接" value={current.connection_count.toString()} />
      </div>
    </section>
  )
}

function SeriesCard({
  title,
  rx,
  tx,
  rxLabel = 'rx',
  txLabel = 'tx',
  txColor = 'stroke-emerald-400',
  txFill,
}: {
  title: string
  rx: number[]
  tx: number[]
  rxLabel?: string
  txLabel?: string
  txColor?: string
  txFill?: string
}) {
  return (
    <section className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5">
      <h3 className="text-sm font-medium text-zinc-200 mb-3">{title}</h3>
      <div className="space-y-2">
        {rx.length > 0 && (
          <div>
            {rxLabel && <div className="text-[11px] text-zinc-500 mb-1">↓ {rxLabel}</div>}
            <Sparkline values={rx} colorClass="stroke-indigo-400" fillClass="fill-indigo-500/10" />
          </div>
        )}
        <div>
          {txLabel && <div className="text-[11px] text-zinc-500 mb-1">↑ {txLabel}</div>}
          <Sparkline values={tx} colorClass={txColor} fillClass={txFill ?? 'fill-emerald-500/10'} />
        </div>
      </div>
    </section>
  )
}

function LogsCard({ logs }: { logs: RuleLogEntry[] }) {
  if (logs.length === 0)
    return (
      <section className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5 text-sm text-zinc-500">
        暂无操作历史。
      </section>
    )
  return (
    <section className="rounded-2xl border border-white/10 bg-zinc-900/40 overflow-hidden">
      <div className="px-5 py-3 border-b border-white/5">
        <h3 className="text-sm font-medium text-zinc-200">最近操作</h3>
      </div>
      <div className="overflow-x-auto">
        <table className="w-full text-sm">
          <thead className="text-[11px] uppercase text-zinc-500 bg-zinc-900/80">
            <tr>
              <th className="px-4 py-2 text-left font-medium">时间</th>
              <th className="px-4 py-2 text-left font-medium">操作</th>
              <th className="px-4 py-2 text-left font-medium">结果</th>
              <th className="px-4 py-2 text-left font-medium">错误</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-white/5">
            {logs.map((l) => (
              <tr key={l.id} className="hover:bg-white/[0.02]">
                <td className="px-4 py-2 align-top text-[12px] text-zinc-400 font-mono whitespace-nowrap">
                  {shortTime(l.created_at)}
                </td>
                <td className="px-4 py-2 align-top text-[12px] text-zinc-200 font-mono">{l.action}</td>
                <td className="px-4 py-2 align-top">
                  <span
                    className={`inline-flex items-center rounded-md border px-1.5 py-0.5 text-[10px] ${
                      l.result === 'success'
                        ? 'border-emerald-500/40 bg-emerald-500/10 text-emerald-200'
                        : 'border-red-500/40 bg-red-500/10 text-red-200'
                    }`}
                  >
                    {l.result}
                  </span>
                </td>
                <td className="px-4 py-2 align-top text-[11px] text-zinc-500 max-w-[20rem] truncate">
                  {l.error_message ?? ''}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  )
}

function Row({ k, v, mono }: { k: string; v: string; mono?: boolean }) {
  return (
    <div className="flex justify-between gap-3">
      <dt className="text-zinc-400">{k}</dt>
      <dd className={`text-zinc-200 ${mono ? 'font-mono text-[12px]' : ''}`}>{v}</dd>
    </div>
  )
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-white/5 bg-zinc-950/40 p-3">
      <div className="text-[11px] text-zinc-500">{label}</div>
      <div className="mt-1 text-base font-semibold">{value}</div>
    </div>
  )
}
