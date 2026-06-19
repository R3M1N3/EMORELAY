import { useEffect, useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'
import {
  ApiError,
  formatBytes,
  getToken,
  nodes,
  renderInstallCommand,
  rules,
  shortTime,
  statusLabel,
  system,
  type CreateNodeRequest,
  type NodeView,
  type UpdateNodeRequest,
} from '../lib/api'
import { EmptyState, ErrorBox, Modal, StatusDot, TableSkeleton, fieldInputCls, fieldLabelCls } from '../lib/ui'
import { Pagination } from '../components/Pagination'
import { RegionBadge } from '../components/RegionBadge'
import { COMMON_COUNTRY_CODES, countryName, normalizeRegion } from '../lib/country'
import { useToast } from '../lib/use-toast'
import { useAutoRefresh } from '../lib/use-auto-refresh'

type Editing = { mode: 'create' } | { mode: 'edit'; node: NodeView } | null

// 创建节点后一次性返回的 Agent 接入凭据(token + mTLS 三件套)。
type CreatedCreds = {
  token: string
  name: string
  id: number
  caPem: string
  clientCertPem: string
  clientKeyPem: string
}

interface ListState {
  items: NodeView[]
  total: number
  loading: boolean
  error: string | null
}

export default function Nodes() {
  const toast = useToast()
  const [list, setList] = useState<ListState>({ items: [], total: 0, loading: true, error: null })
  const [editing, setEditing] = useState<Editing>(null)
  const [confirming, setConfirming] = useState<NodeView | null>(null)
  const [token, setToken] = useState<CreatedCreds | null>(null)
  const [settings, setSettings] = useState<Record<string, string>>({})
  const [busy, setBusy] = useState(false)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const [search, setSearch] = useState('')
  // 删除预检:打开确认框时查询该节点的活跃规则数;null = 预检中/失败(交给后端兜底)。
  const [deleteRefCount, setDeleteRefCount] = useState<number | null>(null)

  useEffect(() => {
    system.getSettings().then((r) => setSettings(r.settings)).catch(() => {})
  }, [])

  // silent=true 用于自动刷新:不置 loading 态,避免表格周期性闪烁。
  async function reload(opts: { silent?: boolean } = {}) {
    if (!opts.silent) setList((s) => ({ ...s, loading: true, error: null }))
    try {
      const r = await nodes.list({ page, page_size: pageSize, search: search.trim() || undefined })
      setList({ items: r.items, total: r.total, loading: false, error: null })
    } catch (e) {
      if (opts.silent) return // 静默刷新失败不打扰,下个周期重试
      const msg = e instanceof ApiError ? e.message : '加载失败'
      setList({ items: [], total: 0, loading: false, error: msg })
    }
  }

  // page / pageSize 变化均触发拉取;请求体带当前 search(翻页保留筛选,
  // 第 2+ 页搜索经 setPage(1) 由本 effect 单请求完成,无并发竞态)。
  useEffect(() => {
    let cancelled = false
    nodes
      .list({ page, page_size: pageSize, search: search.trim() || undefined })
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
    // search 不进 deps:打字不请求,回车/点搜索按钮显式 reload。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, pageSize])

  // 评审 P2-1:节点上线/掉线此前必须手动刷新才能看到。15s 静默轮询兜底。
  useAutoRefresh(() => {
    void reload({ silent: true })
  }, 15_000)

  // SSE 实时推送:节点上线/掉线/指标变更即时合并进当前列表(替代等轮询),
  // 失败/不支持时静默回落轮询。仅合并已在当前页的节点,避免越权/分页错乱。
  useEffect(() => {
    const tok = getToken()
    if (!tok || typeof EventSource === 'undefined') return
    const es = new EventSource(`/api/nodes/stream?token=${encodeURIComponent(tok)}`)
    const onNode = (e: MessageEvent) => {
      try {
        const node = JSON.parse(e.data) as NodeView
        setList((s) => {
          const idx = s.items.findIndex((n) => n.id === node.id)
          if (idx === -1) return s // 不在当前页:留给轮询/翻页处理
          const items = s.items.slice()
          items[idx] = node
          return { ...s, items }
        })
      } catch {
        // 坏帧忽略
      }
    }
    es.addEventListener('node', onNode as EventListener)
    return () => {
      es.removeEventListener('node', onNode as EventListener)
      es.close()
    }
  }, [])

  // 删除预检:用规则列表 total 判断引用数(page_size=1 最小开销)。
  // 状态重置在事件回调(openConfirm/关闭)做,effect 只负责拉取。
  useEffect(() => {
    if (!confirming) return
    let cancelled = false
    rules
      .list({ node_id: confirming.id, page_size: 1 })
      .then((r) => {
        if (!cancelled) setDeleteRefCount(r.total)
      })
      .catch(() => {
        if (!cancelled) setDeleteRefCount(null)
      })
    return () => {
      cancelled = true
    }
  }, [confirming])

  function openDeleteConfirm(n: NodeView) {
    setDeleteRefCount(null)
    setConfirming(n)
  }

  function copyCred(value: string, label: string) {
    navigator.clipboard
      ?.writeText(value)
      .then(() => toast.success(`已复制${label}`))
      .catch(() => toast.error('复制失败，请手动选择'))
  }

  async function doDelete(node: NodeView) {
    setBusy(true)
    try {
      await nodes.del(node.id)
      setConfirming(null)
      toast.success(`节点 ${node.name} 已删除`)
      await reload()
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '删除失败'
      toast.error(msg)
      setConfirming(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">节点</h2>
          <p className="text-sm text-zinc-400 mt-1">转发节点列表与 Agent 心跳状态</p>
        </div>
        <button
          onClick={() => setEditing({ mode: 'create' })}
          className="btn-accent shrink-0"
        >
          新增节点
        </button>
      </div>

      {list.error && <ErrorBox message={list.error} onRetry={() => void reload()} />}

      {/* 服务端搜索:替换原「搜索当前页」本地过滤(数据过百后会搜不到明明存在的节点)。 */}
      <form
        onSubmit={(e) => {
          e.preventDefault()
          // 第 2+ 页搜索:setPage(1) 触发上方 effect(带 search)单请求;
          // 已在第 1 页则显式 reload。两条路径互斥,无并发竞态。
          if (page !== 1) setPage(1)
          else void reload()
        }}
        className="flex items-center gap-2 flex-wrap"
      >
        <input
          type="search"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          aria-label="搜索节点"
          placeholder="搜索名称 / 区域 / IP"
          className={`${fieldInputCls} max-w-sm`}
        />
        <button type="submit" className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm">
          搜索
        </button>
      </form>

      <section className="glass-card rise overflow-hidden">
        {list.loading ? (
          <TableSkeleton cols={8} />
        ) : list.items.length === 0 ? (
          search.trim() ? (
            <EmptyState title="没有匹配的节点" hint="换个关键词,或清空搜索查看全部。" />
          ) : (
            <EmptyState
              title="尚无节点"
              hint="添加第一台转发节点,创建后会给出一键安装命令部署 Agent。"
              action={<button type="button" onClick={() => setEditing({ mode: 'create' })} className="btn-accent">新增节点</button>}
            />
          )
        ) : (
          <>
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead className="text-[11px] uppercase text-zinc-400 bg-white/[0.03]">
                  <tr>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">名称</th>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">区域 / IP</th>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">gRPC</th>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">状态</th>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">资源</th>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium" title="节点网卡总流量(含系统流量),非规则转发流量">
                      网卡流量
                    </th>
                    <th scope="col" className="px-4 py-2.5 text-left font-medium">端口池</th>
                    <th scope="col" className="px-4 py-2.5 text-right font-medium">操作</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-white/5">
                  {list.items.map((n) => (
                    <NodeRow
                      key={n.id}
                      node={n}
                      onEdit={() => setEditing({ mode: 'edit', node: n })}
                      onDelete={() => openDeleteConfirm(n)}
                    />
                  ))}
                </tbody>
              </table>
            </div>
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
          </>
        )}
      </section>

      {editing && (
        <Modal
          title={editing.mode === 'create' ? '新增节点' : `编辑节点 · ${editing.node.name}`}
          onClose={() => setEditing(null)}
        >
          <NodeForm
            mode={editing.mode}
            initial={editing.mode === 'edit' ? editing.node : undefined}
            agentEndpointConfigured={Boolean(settings.agent_control_endpoint)}
            onCancel={() => setEditing(null)}
            onSuccess={async (createdToken) => {
              setEditing(null)
              if (createdToken) setToken(createdToken)
              await reload()
            }}
          />
        </Modal>
      )}

      {confirming && (
        <Modal title="删除节点" onClose={() => !busy && setConfirming(null)} size="sm">
          {/* 评审 P2-4:原文案暗示可删,点确认才被后端拒。预检后直接说清楚。 */}
          {deleteRefCount != null && deleteRefCount > 0 ? (
            <p className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-sm text-red-200">
              节点 <span className="font-medium">{confirming.name}</span> 上仍有{' '}
              {deleteRefCount} 条规则，无法删除。请先在规则页删除或迁移这些规则。
            </p>
          ) : (
            <p className="text-sm text-zinc-300">
              将删除节点 <span className="text-white font-medium">{confirming.name}</span>。
              删除后该节点不再出现在面板中，请确认。
            </p>
          )}
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
              disabled={busy || (deleteRefCount != null && deleteRefCount > 0)}
              className="rounded-lg bg-red-600 hover:bg-red-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
            >
              {busy ? '删除中…' : '确认删除'}
            </button>
          </div>
        </Modal>
      )}

      {token && (
        <Modal title="Agent 接入凭据" onClose={() => setToken(null)} size="md">
          <p className="text-sm text-zinc-300">
            节点 <span className="font-medium text-white">{token.name}</span> 的 Agent 接入凭据。
          </p>
          <p className="mt-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-300">
            私钥仅此一次显示，丢失需轮换凭据。请立即妥善保存以下全部内容。
          </p>
          {(() => {
            const endpoint = settings.agent_control_endpoint || ''
            if (!endpoint) {
              // 评审 P2-3:原文案说「再回到这里」,但本 Modal 关闭即永久消失(凭据一次性),
              // 是条死路。注意:轮换凭据只重签证书不补发 token,不能承诺「重新生成安装命令」。
              return (
                <p className="mt-3 text-[11px] text-amber-300">
                  未配置 Agent 上报端点，无法生成一键安装命令，本节点请用下方凭据手动部署。
                  到「设置」页配置端点后，之后新建的节点会自动附带安装命令。
                </p>
              )
            }
            const cmd = renderInstallCommand({
              nodeId: token.id,
              token: token.token,
              caPem: token.caPem,
              clientCertPem: token.clientCertPem,
              clientKeyPem: token.clientKeyPem,
            })
            return (
              <div className="mt-3">
                <div className="text-[11px] text-zinc-400 mb-1">
                  一键安装命令（已内嵌 mTLS 凭据）
                </div>
                <div className="rounded-lg border border-white/10 bg-zinc-950 px-3 py-2 font-mono text-[11px] text-emerald-100 break-all max-h-32 overflow-auto">
                  {cmd}
                </div>
                <button
                  type="button"
                  onClick={() => copyCred(cmd, '安装命令')}
                  className="mt-2 rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-2.5 py-1 text-xs"
                >
                  复制安装命令
                </button>
              </div>
            )
          })()}

          <div className="mt-4 space-y-2">
            <div className="text-[11px] text-zinc-400">
              或手动分发以下各项（与安装命令二选一）：
            </div>
            <CredBlock label="Agent Token" value={token.token} onCopy={copyCred} defaultOpen />
            <CredBlock label="CA 证书 (ca.pem)" value={token.caPem} onCopy={copyCred} />
            <CredBlock
              label="客户端证书 (client.pem)"
              value={token.clientCertPem}
              onCopy={copyCred}
            />
            <CredBlock
              label="客户端私钥 (client-key.pem)"
              value={token.clientKeyPem}
              onCopy={copyCred}
            />
          </div>

          <div className="mt-5 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setToken(null)}
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

function NodeRow({
  node,
  onEdit,
  onDelete,
}: {
  node: NodeView
  onEdit: () => void
  onDelete: () => void
}) {
  return (
    <tr className="hover:bg-white/[0.02]">
      <td className="px-4 py-3 align-top">
        <Link
          to={`/nodes/${node.id}`}
          className="font-medium text-zinc-100 hover:text-accent"
        >
          {node.name}
        </Link>
        <div className="text-[11px] text-zinc-400 mt-0.5">ID #{node.id}</div>
      </td>
      <td className="px-4 py-3 align-top text-zinc-300">
        <div><RegionBadge region={node.region} /></div>
        <div className="text-[11px] text-zinc-400 mt-0.5">{node.public_ip || '未填'}</div>
      </td>
      {/* 长 URL 截断显示,完整值挂 title;否则挤压名称/状态列(移动端尤甚)。 */}
      <td
        className="px-4 py-3 align-top text-zinc-400 font-mono text-[12px] max-w-[14rem] truncate"
        title={node.grpc_endpoint || undefined}
      >
        {node.grpc_endpoint || '—'}
      </td>
      <td className="px-4 py-3 align-top">
        <span className="inline-flex items-center gap-1.5 text-xs text-zinc-300">
          <StatusDot kind={node.status} />
          {statusLabel(node.status)}
        </span>
        <div className="text-[11px] text-zinc-400 mt-0.5">
          {node.last_seen_at ? `最后心跳 ${shortTime(node.last_seen_at)}` : '从未上线'}
        </div>
        {node.agent_version && (
          <div className="text-[10px] text-zinc-400 mt-0.5">Agent v{node.agent_version}</div>
        )}
      </td>
      <td className="px-4 py-3 align-top text-[12px] text-zinc-300">
        <div>CPU {node.cpu_usage.toFixed(1)}%</div>
        <div>MEM {node.memory_usage.toFixed(1)}%</div>
        <div>LOAD {node.load_average.toFixed(2)}</div>
      </td>
      <td className="px-4 py-3 align-top text-[12px] text-zinc-300">
        <div>↓ {formatBytes(node.rx_bytes_total)}</div>
        <div>↑ {formatBytes(node.tx_bytes_total)}</div>
      </td>
      <td className="px-4 py-3 align-top text-[12px] text-zinc-400">
        {node.port_pool_min}–{node.port_pool_max}
      </td>
      <td className="px-4 py-3 align-top text-right">
        <button
          type="button"
          onClick={onEdit}
          className="rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-2.5 py-1 text-xs"
        >
          编辑
        </button>
        <button
          type="button"
          onClick={onDelete}
          className="ml-1.5 rounded-md px-2.5 py-1 text-xs text-red-300/90 ring-1 ring-inset ring-red-500/25 hover:bg-red-500/15 hover:text-red-200"
        >
          删除
        </button>
      </td>
    </tr>
  )
}

// 单条凭据展示块：可折叠 + 一键复制。私钥默认折叠，token 默认展开。
function CredBlock({
  label,
  value,
  onCopy,
  defaultOpen = false,
}: {
  label: string
  value: string
  onCopy: (value: string, label: string) => void
  defaultOpen?: boolean
}) {
  return (
    <details open={defaultOpen} className="rounded-lg border border-white/10 bg-zinc-950">
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

interface NodeFormState {
  name: string
  region: string
  public_ip: string
  display_address: string
  grpc_endpoint: string
  port_pool_min: string
  port_pool_max: string
}

function NodeForm({
  mode,
  initial,
  agentEndpointConfigured = true,
  onCancel,
  onSuccess,
}: {
  mode: 'create' | 'edit'
  initial?: NodeView
  /** 设置页 agent_control_endpoint 是否已配置;未配则创建前预警(安装命令将不可用)。 */
  agentEndpointConfigured?: boolean
  onCancel: () => void
  onSuccess: (createdCreds: CreatedCreds | null) => void | Promise<void>
}) {
  const [form, setForm] = useState<NodeFormState>({
    name: initial?.name ?? '',
    region: initial?.region ?? '',
    public_ip: initial?.public_ip ?? '',
    display_address: initial?.display_address ?? '',
    grpc_endpoint: initial?.grpc_endpoint ?? '',
    // 新建默认 10000-65535:避开系统/常用端口段,与后端 normalize_port_pool 缺省一致。
    port_pool_min: initial ? String(initial.port_pool_min) : '10000',
    port_pool_max: initial ? String(initial.port_pool_max) : '65535',
  })
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  function set<K extends keyof NodeFormState>(k: K, v: NodeFormState[K]) {
    setForm((f) => ({ ...f, [k]: v }))
  }

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)

    if (!form.name.trim()) {
      setError('名称不能为空')
      return
    }

    const portMin = form.port_pool_min.trim() === '' ? undefined : Number(form.port_pool_min)
    const portMax = form.port_pool_max.trim() === '' ? undefined : Number(form.port_pool_max)
    if (
      (portMin !== undefined && (!Number.isInteger(portMin) || portMin < 1 || portMin > 65535)) ||
      (portMax !== undefined && (!Number.isInteger(portMax) || portMax < 1 || portMax > 65535))
    ) {
      setError('端口池范围必须是 1-65535 的整数')
      return
    }
    if (portMin !== undefined && portMax !== undefined && portMin > portMax) {
      setError('端口池下界不能大于上界')
      return
    }

    setSubmitting(true)
    try {
      if (mode === 'create') {
        const payload: CreateNodeRequest = {
          name: form.name.trim(),
          region: normalizeRegion(form.region),
          public_ip: form.public_ip.trim(),
          display_address: form.display_address.trim(),
          grpc_endpoint: form.grpc_endpoint.trim(),
          port_pool_min: portMin,
          port_pool_max: portMax,
        }
        const r = await nodes.create(payload)
        await onSuccess({
          token: r.agent_token,
          name: r.node.name,
          id: r.node.id,
          caPem: r.ca_pem,
          clientCertPem: r.client_cert_pem,
          clientKeyPem: r.client_key_pem,
        })
      } else if (initial) {
        const payload: UpdateNodeRequest = {
          name: form.name.trim() !== initial.name ? form.name.trim() : undefined,
          region: normalizeRegion(form.region) !== normalizeRegion(initial.region) ? normalizeRegion(form.region) : undefined,
          public_ip: form.public_ip.trim() !== initial.public_ip ? form.public_ip.trim() : undefined,
          // '' 是合法值(清空 = 回落接入地址),有变更就原样发送。
          display_address:
            form.display_address.trim() !== initial.display_address
              ? form.display_address.trim()
              : undefined,
          grpc_endpoint:
            form.grpc_endpoint.trim() !== initial.grpc_endpoint
              ? form.grpc_endpoint.trim()
              : undefined,
          port_pool_min: portMin !== initial.port_pool_min ? portMin : undefined,
          port_pool_max: portMax !== initial.port_pool_max ? portMax : undefined,
        }
        await nodes.update(initial.id, payload)
        await onSuccess(null)
      }
    } catch (e) {
      setError(e instanceof ApiError ? e.message : '提交失败')
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <form noValidate onSubmit={onSubmit} className="space-y-4">
      {mode === 'create' && !agentEndpointConfigured && (
        <p className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-300">
          尚未配置 Agent 上报端点，创建后将无法生成一键安装命令（凭据仍可手动保存）。
          建议先到「设置」页配置后再创建节点。
        </p>
      )}
      <div>
        <label htmlFor="node-name" className={fieldLabelCls}>名称 *</label>
        <input
          id="node-name"
          required
          value={form.name}
          onChange={(e) => set('name', e.target.value)}
          className={fieldInputCls}
          placeholder="例如 hk-relay-01"
        />
      </div>
      <div className="grid grid-cols-2 gap-3">
        <div>
          <label htmlFor="node-region" className={fieldLabelCls}>区域</label>
          <input
            id="node-region"
            value={form.region}
            onChange={(e) => set('region', e.target.value)}
            list="node-country-codes"
            className={fieldInputCls}
            placeholder="国家码如 HK / JP（可下拉选）"
          />
          <datalist id="node-country-codes">
            {COMMON_COUNTRY_CODES.map((c) => (
              <option key={c} value={c}>{countryName(c)}</option>
            ))}
          </datalist>
        </div>
        <div>
          <label htmlFor="node-public-ip" className={fieldLabelCls}>接入地址</label>
          <input
            id="node-public-ip"
            value={form.public_ip}
            onChange={(e) => set('public_ip', e.target.value)}
            className={fieldInputCls}
            placeholder="1.2.3.4 或 node.ddns.example.com"
          />
        </div>
      </div>
      <div>
        <label htmlFor="node-display" className={fieldLabelCls}>展示地址（可选）</label>
        <input
          id="node-display"
          value={form.display_address}
          onChange={(e) => set('display_address', e.target.value)}
          className={fieldInputCls}
          placeholder="留空 = 直接展示接入地址"
        />
        <p className="text-[11px] text-zinc-400 mt-1">
          接入地址是隧道/节点互联实际连接的地址(NAT/DDNS 机器填可达域名);
          展示地址是普通用户看到的入口,留空回落接入地址。
        </p>
      </div>
      <div>
        <label htmlFor="node-grpc" className={fieldLabelCls}>gRPC 端点</label>
        <input
          id="node-grpc"
          value={form.grpc_endpoint}
          onChange={(e) => set('grpc_endpoint', e.target.value)}
          className={fieldInputCls}
          placeholder="https://agent.example.com:7001"
        />
        <p className="text-[11px] text-zinc-400 mt-1">
          仅作展示用途。Agent 会主动用 token 连接主控，不由主控反向拨号。
        </p>
      </div>
      <div className="grid grid-cols-2 gap-3">
        <div>
          <label htmlFor="node-port-min" className={fieldLabelCls}>端口池下界</label>
          <input
            id="node-port-min"
            type="number"
            min={1}
            max={65535}
            value={form.port_pool_min}
            onChange={(e) => set('port_pool_min', e.target.value)}
            className={fieldInputCls}
            placeholder="默认 10000"
          />
        </div>
        <div>
          <label htmlFor="node-port-max" className={fieldLabelCls}>端口池上界</label>
          <input
            id="node-port-max"
            type="number"
            min={1}
            max={65535}
            value={form.port_pool_max}
            onChange={(e) => set('port_pool_max', e.target.value)}
            className={fieldInputCls}
            placeholder="默认 65535"
          />
        </div>
      </div>

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

