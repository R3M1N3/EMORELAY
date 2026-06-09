import { useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import {
  ApiError,
  formatBytes,
  nodes,
  shortTime,
  type NodeStatsResponse,
  type NodeView,
} from '../lib/api'
import { Sparkline } from '../components/Sparkline'
import { StatusDot } from '../lib/ui'

interface State {
  node: NodeView | null
  stats: NodeStatsResponse | null
  loading: boolean
  error: string | null
}

export default function NodeDetail() {
  const { id } = useParams<{ id: string }>()
  const nodeId = id ? Number(id) : NaN
  const [state, setState] = useState<State>({
    node: null,
    stats: null,
    loading: true,
    error: null,
  })

  useEffect(() => {
    let cancelled = false
    // invalid id 走 Promise.reject 让 setState 落在 .catch 异步路径。
    const work: Promise<[NodeView, NodeStatsResponse]> = Number.isFinite(nodeId)
      ? Promise.all([nodes.get(nodeId), nodes.stats(nodeId)])
      : Promise.reject(new Error('无效的节点 ID'))

    work
      .then(([node, stats]) => {
        if (cancelled) return
        setState({ node, stats, loading: false, error: null })
      })
      .catch((e: unknown) => {
        if (cancelled) return
        const msg =
          e instanceof ApiError ? e.message : e instanceof Error ? e.message : '加载失败'
        setState({ node: null, stats: null, loading: false, error: msg })
      })
    return () => {
      cancelled = true
    }
  }, [nodeId])

  if (state.loading) return <div className="text-zinc-400">加载中…</div>
  if (state.error)
    return (
      <div className="space-y-4">
        <Link to="/nodes" className="text-xs text-zinc-400 hover:text-zinc-200">← 返回节点列表</Link>
        <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-200">
          {state.error}
        </div>
      </div>
    )
  if (!state.node || !state.stats) return null

  const { node, stats } = state
  const seriesAsc = [...stats.series].reverse()
  const cpu = seriesAsc.map((b) => b.cpu_usage)
  const mem = seriesAsc.map((b) => b.memory_usage)
  const load = seriesAsc.map((b) => b.load_average)
  const rx = seriesAsc.map((b) => b.rx_bytes)
  const tx = seriesAsc.map((b) => b.tx_bytes)

  return (
    <div className="space-y-6">
      <div>
        <Link to="/nodes" className="text-xs text-zinc-400 hover:text-zinc-200">← 返回节点列表</Link>
        <h2 className="mt-1 text-xl font-semibold tracking-tight">{node.name}</h2>
        <p className="text-sm text-zinc-400">
          <span className="inline-flex items-center gap-1.5 mr-3">
            <StatusDot kind={node.status} />
            {node.status}
          </span>
          ID #{node.id} · {node.region || '—'} · {node.public_ip || '未填'}
        </p>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <section className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5">
          <h3 className="text-sm font-medium text-zinc-200 mb-3">基本信息</h3>
          <dl className="text-sm space-y-2">
            <Row k="区域" v={node.region || '—'} />
            <Row k="公网 IP" v={node.public_ip || '—'} mono />
            <Row k="gRPC 端点" v={node.grpc_endpoint || '—'} mono />
            <Row k="端口池" v={`${node.port_pool_min}–${node.port_pool_max}`} />
            <Row k="最后心跳" v={node.last_seen_at ? shortTime(node.last_seen_at) : '从未上线'} />
            <Row k="创建" v={shortTime(node.created_at)} />
          </dl>
          <div className="text-[11px] text-zinc-500 mt-2">
            Agent 安装命令需要创建节点时一次性显示的 token；
            如已遗失，后续（P2 阶段）将提供「轮换 Agent 凭据」入口。
          </div>
        </section>

        <section className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5">
          <h3 className="text-sm font-medium text-zinc-200 mb-3">当前资源</h3>
          <div className="grid grid-cols-3 gap-3 text-sm">
            <Stat label="CPU" value={`${stats.current.cpu_usage.toFixed(1)}%`} />
            <Stat label="MEM" value={`${stats.current.memory_usage.toFixed(1)}%`} />
            <Stat label="LOAD" value={stats.current.load_average.toFixed(2)} />
          </div>
          <div className="grid grid-cols-2 gap-3 text-sm mt-3">
            <Stat label="累计 ↓ rx" value={formatBytes(stats.current.rx_bytes_total)} />
            <Stat label="累计 ↑ tx" value={formatBytes(stats.current.tx_bytes_total)} />
          </div>
        </section>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
        <SeriesCard title="CPU (%)" values={cpu} color="stroke-amber-400" fill="fill-amber-500/10" />
        <SeriesCard title="MEM (%)" values={mem} color="stroke-violet-400" fill="fill-violet-500/10" />
        <SeriesCard title="LOAD (1m)" values={load} color="stroke-sky-400" fill="fill-sky-500/10" />
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <SeriesCard title="rx 字节 / 分钟" values={rx} color="stroke-indigo-400" fill="fill-indigo-500/10" />
        <SeriesCard title="tx 字节 / 分钟" values={tx} color="stroke-emerald-400" fill="fill-emerald-500/10" />
      </div>
    </div>
  )
}

function SeriesCard({
  title,
  values,
  color,
  fill,
}: {
  title: string
  values: number[]
  color: string
  fill: string
}) {
  return (
    <section className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5">
      <h3 className="text-sm font-medium text-zinc-200 mb-3">{title}</h3>
      <Sparkline values={values} colorClass={color} fillClass={fill} />
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
