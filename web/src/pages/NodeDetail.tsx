import { useEffect, useState, type ReactNode } from 'react'
import { Link, useParams } from 'react-router-dom'
import {
  ApiError,
  formatBytes,
  nodes,
  rules,
  shortTime,
  statusLabel,
  type GrantedUser,
  type NodeStatsResponse,
  type NodeView,
} from '../lib/api'
import { Sparkline } from '../components/Sparkline'
import { RegionBadge } from '../components/RegionBadge'
import { ErrorBox, Modal, PageLoading, StatusDot } from '../lib/ui'
import { useToast } from '../lib/use-toast'
import { useAutoRefresh } from '../lib/use-auto-refresh'

interface State {
  node: NodeView | null
  stats: NodeStatsResponse | null
  loading: boolean
  error: string | null
}

// 轮换凭据后一次性返回的 mTLS 三件套。
type RevokedCreds = {
  caPem: string
  clientCertPem: string
  clientKeyPem: string
}

export default function NodeDetail() {
  const { id } = useParams<{ id: string }>()
  const nodeId = id ? Number(id) : NaN
  const toast = useToast()
  const [state, setState] = useState<State>({
    node: null,
    stats: null,
    loading: true,
    error: null,
  })
  const [confirmingRevoke, setConfirmingRevoke] = useState(false)
  const [confirmingUpgrade, setConfirmingUpgrade] = useState(false)
  const [upgrading, setUpgrading] = useState(false)
  const [revoking, setRevoking] = useState(false)
  const [revokedCreds, setRevokedCreds] = useState<RevokedCreds | null>(null)
  // P7:该节点被授权给哪些用户(admin-only 端点;本页路由已 admin-only)。null = 未加载。
  const [grantedUsers, setGrantedUsers] = useState<GrantedUser[] | null>(null)
  // 30s 静默刷新心跳/资源/时序。
  const [refreshTick, setRefreshTick] = useState(0)
  useAutoRefresh(() => setRefreshTick((n) => n + 1), 30_000)

  function copyCred(value: string, label: string) {
    if (!navigator.clipboard) {
      // HTTP 非安全上下文剪贴板不可用:提示手动复制(凭据均以可选文本展示)。
      toast.error('当前环境(非 HTTPS)无法自动复制，请手动选择文本复制')
      return
    }
    navigator.clipboard
      .writeText(value)
      .then(() => toast.success(`已复制${label}`))
      .catch(() => toast.error('复制失败，请手动选择'))
  }

  async function doRevoke() {
    if (!Number.isFinite(nodeId)) return
    setRevoking(true)
    try {
      const r = await nodes.revokeCredentials(nodeId)
      setConfirmingRevoke(false)
      setRevokedCreds({
        caPem: r.ca_pem,
        clientCertPem: r.client_cert_pem,
        clientKeyPem: r.client_key_pem,
      })
      toast.success('凭据已轮换，旧证书已吊销')
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : '轮换失败')
      setConfirmingRevoke(false)
    } finally {
      setRevoking(false)
    }
  }

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
        // 静默刷新失败不打扰(凭据 Modal 可能开着,整页错误态会把一次性私钥顶掉)。
        setState((prev) =>
          prev.node && prev.node.id === nodeId
            ? prev
            : { node: null, stats: null, loading: false, error: msg },
        )
      })
    return () => {
      cancelled = true
    }
  }, [nodeId, refreshTick])

  // 授权用户列表只拉一次(变更入口在用户编辑弹窗,本页只读展示)。
  useEffect(() => {
    if (!Number.isFinite(nodeId)) return
    let cancelled = false
    nodes
      .grants(nodeId)
      .then((g) => {
        if (!cancelled) setGrantedUsers(g)
      })
      .catch(() => {
        // 加载失败不阻塞详情页,该区块显示「—」。
      })
    return () => {
      cancelled = true
    }
  }, [nodeId])

  if (state.loading) return <PageLoading />
  if (state.error)
    return (
      <div className="space-y-4">
        <Link to="/nodes" className="text-xs text-zinc-400 hover:text-zinc-200">← 返回节点列表</Link>
        <ErrorBox message={state.error} onRetry={() => setRefreshTick((n) => n + 1)} />
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
      <div className="flex items-start justify-between gap-3">
        <div>
          <Link to="/nodes" className="text-xs text-zinc-400 hover:text-zinc-200">← 返回节点列表</Link>
          <h2 className="mt-1 text-xl font-semibold tracking-tight">{node.name}</h2>
          <p className="text-sm text-zinc-400">
            <span className="inline-flex items-center gap-1.5 mr-3">
              <StatusDot kind={node.status} />
              {statusLabel(node.status)}
            </span>
            ID #{node.id} · <RegionBadge region={node.region} /> · {node.public_ip || '未填'}
          </p>
        </div>
        <div className="flex gap-2 shrink-0">
          {/* P10b: 一键升级 Agent(下载/校验/原子替换/exec 重启,节点须在线)。 */}
          <button
            type="button"
            onClick={() => setConfirmingUpgrade(true)}
            className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm"
            title={node.agent_version ? `当前 Agent 版本 ${node.agent_version}` : undefined}
          >
            升级 Agent
          </button>
          {/* P9: 导出本节点全部规则(跨实例迁移/备份用)。 */}
          <button
            type="button"
            onClick={async () => {
              try {
                await rules.exportDownload({ node_id: nodeId })
                toast.success('已导出本节点规则')
              } catch (e) {
                toast.error(e instanceof ApiError ? e.message : '导出失败')
              }
            }}
            className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm"
          >
            导出规则
          </button>
          <button
            type="button"
            onClick={() => setConfirmingRevoke(true)}
            className="rounded-lg bg-amber-600/80 hover:bg-amber-500 px-3 py-2 text-sm font-medium"
          >
            轮换凭据
          </button>
        </div>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <section className="glass-card rise p-5">
          <h3 className="text-sm font-medium text-zinc-200 mb-3">基本信息</h3>
          <dl className="text-sm space-y-2">
            <Row k="区域" v={<RegionBadge region={node.region} />} />
            <Row k="接入地址" v={node.public_ip || '—'} mono />
            <Row
              k="展示地址"
              v={node.display_address || '（回落接入地址）'}
              mono={!!node.display_address}
            />
            <Row k="gRPC 端点" v={node.grpc_endpoint || '—'} mono />
            <Row k="端口池" v={`${node.port_pool_min}–${node.port_pool_max}`} />
            <Row k="最后心跳" v={node.last_seen_at ? shortTime(node.last_seen_at) : '从未上线'} />
            <Row k="创建" v={shortTime(node.created_at)} />
          </dl>
          <div className="text-[11px] text-zinc-400 mt-2">
            Agent 接入凭据在创建节点时一次性显示；
            凭据遗失或泄露时，点右上角「轮换凭据」重新签发(旧证书随即吊销)。
          </div>
          {/* P7:已授权使用本节点的用户(在「用户」页编辑授权)。 */}
          <div className="mt-3 pt-3 border-t border-white/5">
            <div className="text-[11px] text-zinc-400 mb-1.5">已授权用户</div>
            {grantedUsers == null ? (
              <span className="text-[12px] text-zinc-400">—</span>
            ) : grantedUsers.length === 0 ? (
              <span className="text-[12px] text-zinc-400">无（普通用户默认不可用本节点）</span>
            ) : (
              <div className="flex flex-wrap gap-1.5">
                {grantedUsers.map((u) => (
                  <span
                    key={u.id}
                    className="inline-flex items-center rounded-md border border-white/10 bg-white/5 px-2 py-0.5 text-[11px] text-zinc-200"
                  >
                    {u.username}
                  </span>
                ))}
              </div>
            )}
          </div>
        </section>

        <ProtocolBlockCard
          node={node}
          onChanged={() => {
            toast.success('协议阻断设置已更新')
            setRefreshTick((n) => n + 1)
          }}
        />

        <section className="glass-card rise p-5">
          <h3 className="text-sm font-medium text-zinc-200 mb-3">
            当前资源
            {node.status !== 'online' && (
              <span className="ml-2 text-[11px] font-normal text-zinc-500">节点离线,实时资源不可用(下方累计为最后值)</span>
            )}
          </h3>
          <div className="grid grid-cols-3 gap-3 text-sm">
            <Stat label="CPU" value={node.status === 'online' ? `${stats.current.cpu_usage.toFixed(1)}%` : '—'} />
            <Stat label="MEM" value={node.status === 'online' ? `${stats.current.memory_usage.toFixed(1)}%` : '—'} />
            <Stat label="LOAD" value={node.status === 'online' ? stats.current.load_average.toFixed(2) : '—'} />
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
        <SeriesCard title="rx 字节 / 分钟" values={rx} color="stroke-accent" fill="fill-accent/10" />
        <SeriesCard title="tx 字节 / 分钟" values={tx} color="stroke-emerald-400" fill="fill-emerald-500/10" />
      </div>

      {confirmingUpgrade && (
        <Modal title="升级 Agent" onClose={() => !upgrading && setConfirmingUpgrade(false)} size="sm">
          <p className="text-sm text-zinc-300">
            将向节点 <span className="font-medium text-white">{node.name}</span> 下发一键升级
            （目标 = 面板当前版本）。Agent 自行下载校验后原子替换并重启。
          </p>
          <p className="mt-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-300">
            重启瞬间该节点全部存量连接会中断（规则随即自动恢复，已建立的连接不会回来）。
            升级结果请稍后观察节点列表的 Agent 版本列。
          </p>
          <div className="mt-5 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setConfirmingUpgrade(false)}
              disabled={upgrading}
              className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm"
            >
              取消
            </button>
            <button
              type="button"
              onClick={async () => {
                setUpgrading(true)
                try {
                  const r = await nodes.upgradeAgent(nodeId)
                  toast.success(`升级命令已下发(目标 v${r.target_version})`)
                  setConfirmingUpgrade(false)
                } catch (e) {
                  toast.error(e instanceof ApiError ? e.message : '下发失败')
                } finally {
                  setUpgrading(false)
                }
              }}
              disabled={upgrading}
              className="btn-accent"
            >
              {upgrading ? '下发中…' : '确认升级'}
            </button>
          </div>
        </Modal>
      )}

      {confirmingRevoke && (
        <Modal title="轮换 Agent 凭据" onClose={() => !revoking && setConfirmingRevoke(false)} size="sm">
          <p className="text-sm text-zinc-300">
            将为节点 <span className="font-medium text-white">{node.name}</span> 重新签发 mTLS 证书。
          </p>
          <p className="mt-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-300">
            旧证书将立即失效，必须用新的四件套重装 Agent，否则该节点将无法再连接主控。
          </p>
          <div className="mt-5 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setConfirmingRevoke(false)}
              disabled={revoking}
              className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm"
            >
              取消
            </button>
            <button
              type="button"
              onClick={doRevoke}
              disabled={revoking}
              className="rounded-lg bg-amber-600 hover:bg-amber-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
            >
              {revoking ? '轮换中…' : '确认轮换'}
            </button>
          </div>
        </Modal>
      )}

      {revokedCreds && (
        <Modal title="新 Agent 凭据" onClose={() => setRevokedCreds(null)} size="md">
          <p className="text-sm text-zinc-300">
            节点 <span className="font-medium text-white">{node.name}</span> 的新 mTLS 凭据。
          </p>
          <p className="mt-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-300">
            旧证书已吊销。私钥仅此一次显示，请立即妥善保存并用以下三件套重装 Agent。
          </p>
          <div className="mt-4 space-y-2">
            <CredBlock label="CA 证书 (ca.pem)" value={revokedCreds.caPem} onCopy={copyCred} />
            <CredBlock
              label="客户端证书 (client.pem)"
              value={revokedCreds.clientCertPem}
              onCopy={copyCred}
            />
            <CredBlock
              label="客户端私钥 (client-key.pem)"
              value={revokedCreds.clientKeyPem}
              onCopy={copyCred}
            />
          </div>
          <div className="mt-5 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setRevokedCreds(null)}
              className="btn-accent"
            >
              我已保存
            </button>
          </div>
        </Modal>
      )}
    </div>
  )
}

// 单条凭据展示块：可折叠 + 一键复制。与 Nodes.tsx 的创建凭据弹窗风格一致。
function CredBlock({
  label,
  value,
  onCopy,
}: {
  label: string
  value: string
  onCopy: (value: string, label: string) => void
}) {
  return (
    <details className="rounded-lg border border-white/10 bg-zinc-950">
      <summary className="flex cursor-pointer items-center justify-between px-3 py-2 text-[11px] text-zinc-400 select-none">
        <span>{label}</span>
        <button
          type="button"
          onClick={(e) => {
            e.preventDefault()
            onCopy(value, label)
          }}
          className="rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-2 py-0.5 text-[11px] text-zinc-200"
        >
          复制
        </button>
      </summary>
      <pre className="max-h-40 overflow-auto border-t border-white/5 px-3 py-2 font-mono text-[11px] text-emerald-200 whitespace-pre-wrap break-all">
        {value}
      </pre>
    </details>
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
    <section className="glass-card rise p-5">
      <h3 className="text-sm font-medium text-zinc-200 mb-3">{title}</h3>
      <Sparkline values={values} colorClass={color} fillClass={fill} label={title} />
    </section>
  )
}

function Row({ k, v, mono }: { k: string; v: ReactNode; mono?: boolean }) {
  return (
    <div className="flex justify-between gap-3">
      <dt className="text-zinc-400">{k}</dt>
      <dd className={`text-zinc-200 ${mono ? 'font-mono text-[12px]' : ''}`}>{v}</dd>
    </div>
  )
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-white/5 bg-white/[0.03] p-3">
      <div className="text-[11px] text-zinc-400">{label}</div>
      <div className="mt-1 text-base font-semibold">{value}</div>
    </div>
  )
}

// 协议嗅探阻断:3 个位的开关(HTTP/TLS/SOCKS),改动即 PATCH 节点并重发规则。
const PROTO_BITS: { bit: number; label: string; hint: string }[] = [
  { bit: 1, label: 'HTTP', hint: '阻断明文 HTTP 请求(防当开放代理)' },
  { bit: 2, label: 'TLS', hint: '阻断 TLS ClientHello(防套 CDN/HTTPS 代理)' },
  { bit: 4, label: 'SOCKS', hint: '阻断 SOCKS4/5 握手' },
]

function ProtocolBlockCard({ node, onChanged }: { node: NodeView; onChanged: () => void }) {
  const toast = useToast()
  const [saving, setSaving] = useState(false)
  const mask = node.block_protocols

  async function toggle(bit: number) {
    if (saving) return
    setSaving(true)
    const next = mask & bit ? mask & ~bit : mask | bit
    try {
      await nodes.update(node.id, { block_protocols: next })
      onChanged()
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : '更新失败')
    } finally {
      setSaving(false)
    }
  }

  return (
    <section className="glass-card rise p-5">
      <h3 className="text-sm font-medium text-zinc-200 mb-1">协议阻断</h3>
      <p className="text-[11px] text-zinc-400 mb-3">
        对普通 TCP 转发的首包做被动指纹识别，命中即断连，防止转发被滥用为开放代理。默认全关。
      </p>
      <div className="space-y-2">
        {PROTO_BITS.map(({ bit, label, hint }) => (
          <label
            key={bit}
            className="flex items-start gap-2.5 rounded-lg border border-white/5 bg-white/[0.02] px-3 py-2 cursor-pointer hover:bg-white/[0.04]"
          >
            <input
              type="checkbox"
              checked={(mask & bit) !== 0}
              disabled={saving}
              onChange={() => toggle(bit)}
              className="mt-0.5 accent-accent"
            />
            <span className="min-w-0">
              <span className="text-sm text-zinc-200">阻断 {label}</span>
              <span className="block text-[11px] text-zinc-400">{hint}</span>
            </span>
          </label>
        ))}
      </div>
    </section>
  )
}
