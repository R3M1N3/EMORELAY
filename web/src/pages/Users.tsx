import { useEffect, useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'
import {
  ApiError,
  formatBytes,
  nodes,
  shortTime,
  tunnels,
  users,
  type CreateUserRequest,
  type NodeView,
  type TunnelView,
  type UpdateUserRequest,
  type UserDetail,
} from '../lib/api'
import { EmptyState, ErrorBox, Modal, TableSkeleton, fieldInputCls, fieldLabelCls, PasswordInput } from '../lib/ui'
import { Pagination } from '../components/Pagination'
import { bytesToGbString, gbToBytes, quotaPercent, quotaTone } from '../lib/quota'
import { useToast } from '../lib/use-toast'

type Editing = { mode: 'create' } | { mode: 'edit'; user: UserDetail } | null

interface ListState {
  items: UserDetail[]
  total: number
  loading: boolean
  error: string | null
}

export default function Users() {
  const toast = useToast()
  const [list, setList] = useState<ListState>({ items: [], total: 0, loading: true, error: null })
  const [editing, setEditing] = useState<Editing>(null)
  const [confirming, setConfirming] = useState<UserDetail | null>(null)
  const [busy, setBusy] = useState(false)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const [search, setSearch] = useState('')

  async function reload() {
    setList((s) => ({ ...s, loading: true, error: null }))
    try {
      const r = await users.list({ page, page_size: pageSize, search: search.trim() || undefined })
      setList({ items: r.items, total: r.total, loading: false, error: null })
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '加载失败'
      setList({ items: [], total: 0, loading: false, error: msg })
    }
  }

  // 请求体带当前 search:翻页保留筛选;第 2+ 页搜索经 setPage(1) 由本 effect 单请求完成。
  useEffect(() => {
    let cancelled = false
    users
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
    // search 不进 deps:打字不请求,提交时显式触发。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, pageSize])

  async function doDelete(user: UserDetail) {
    setBusy(true)
    try {
      await users.del(user.id)
      toast.success('用户已删除')
      setConfirming(null)
      await reload()
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '删除失败'
      setList((s) => ({ ...s, error: msg }))
      setConfirming(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">用户</h2>
          <p className="text-sm text-zinc-400 mt-1">管理员账号与普通用户</p>
        </div>
        <button
          onClick={() => setEditing({ mode: 'create' })}
          className="btn-accent shrink-0"
        >
          新增用户
        </button>
      </div>

      {list.error && <ErrorBox message={list.error} onRetry={() => void reload()} />}

      {/* 服务端搜索:替换原「搜索当前页」本地过滤。 */}
      <form
        onSubmit={(e) => {
          e.preventDefault()
          // 第 2+ 页搜索由 setPage(1) 触发 effect 单请求;第 1 页直接 reload。
          if (page !== 1) setPage(1)
          else void reload()
        }}
        className="flex items-center gap-2 flex-wrap"
      >
        <input
          type="search"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          aria-label="搜索用户名"
          placeholder="搜索用户名"
          className={`${fieldInputCls} max-w-sm`}
        />
        <button type="submit" className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-2 text-sm">
          搜索
        </button>
      </form>

      <section className="glass-card rise overflow-hidden">
        {list.loading ? (
          <TableSkeleton cols={9} />
        ) : list.items.length === 0 ? (
          search.trim() ? (
            <EmptyState title="没有匹配的用户" hint="换个用户名关键词试试。" />
          ) : (
            <EmptyState
              title="尚无用户"
              hint="创建普通用户并授权节点/隧道,他们即可自助管理自己的转发规则。"
              action={<button type="button" onClick={() => setEditing({ mode: 'create' })} className="btn-accent">新增用户</button>}
            />
          )
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-[11px] uppercase text-zinc-400 bg-white/[0.03]">
                <tr>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">用户名</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">角色</th>
                  <th scope="col" className="px-4 py-2.5 text-right font-medium">规则数</th>
                  <th scope="col" className="px-4 py-2.5 text-right font-medium">累计流量</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">到期</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium" title="约每 5 分钟由后台刷新">
                    30d 用量
                  </th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">创建于</th>
                  <th scope="col" className="px-4 py-2.5 text-left font-medium">更新于</th>
                  <th scope="col" className="px-4 py-2.5 text-right font-medium">操作</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-white/5">
                {list.items.map((u) => (
                  <UserRow
                    key={u.id}
                    user={u}
                    onEdit={() => setEditing({ mode: 'edit', user: u })}
                    onDelete={() => setConfirming(u)}
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
          title={editing.mode === 'create' ? '新增用户' : `编辑用户 · ${editing.user.username}`}
          onClose={() => setEditing(null)}
        >
          <UserForm
            mode={editing.mode}
            initial={editing.mode === 'edit' ? editing.user : undefined}
            onCancel={() => setEditing(null)}
            onSuccess={async () => {
              toast.success(editing.mode === 'create' ? '用户已创建' : '用户已保存')
              setEditing(null)
              await reload()
            }}
          />
        </Modal>
      )}

      {confirming && (
        <Modal title="删除用户" onClose={() => !busy && setConfirming(null)} size="sm">
          {confirming.rule_count > 0 ? (
            <p className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-200">
              用户 <span className="font-medium">{confirming.username}</span> 名下仍有{' '}
              {confirming.rule_count} 条规则。删除用户不会清理这些规则，建议先在「规则」页处理。
            </p>
          ) : (
            <p className="text-sm text-zinc-300">
              将删除用户 <span className="text-white font-medium">{confirming.username}</span>
              ，该账号将无法再登录，请确认。
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
              disabled={busy}
              className="rounded-lg bg-red-600 hover:bg-red-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
            >
              {busy ? '删除中…' : '确认删除'}
            </button>
          </div>
        </Modal>
      )}
    </div>
  )
}

function UserRow({
  user,
  onEdit,
  onDelete,
}: {
  user: UserDetail
  onEdit: () => void
  onDelete: () => void
}) {
  const roleClass =
    user.role === 'admin'
      ? 'bg-accent/10 border-accent/30 text-accent'
      : 'bg-zinc-500/15 border-zinc-500/40 text-zinc-300'
  return (
    <tr className="hover:bg-white/[0.02]">
      <td className="px-4 py-3 align-top">
        <div className="font-medium text-zinc-100">{user.username}</div>
        <div className="text-[11px] text-zinc-400 mt-0.5">ID #{user.id}</div>
      </td>
      <td className="px-4 py-3 align-top">
        <span
          className={`inline-flex items-center rounded-md border px-2 py-0.5 text-[11px] ${roleClass}`}
        >
          {user.role}
        </span>
      </td>
      <td className="px-4 py-3 align-top text-right text-zinc-200 tabular-nums text-[12px]">
        {user.rule_count}
        {user.forward_rules_quota != null ? ` / ${user.forward_rules_quota}` : ''}
      </td>
      <td className="px-4 py-3 align-top text-right text-zinc-200 tabular-nums text-[12px]">
        {formatBytes(user.total_traffic_bytes)}
      </td>
      <td className="px-4 py-3 align-top text-[12px] text-zinc-300 whitespace-nowrap">
        {user.expires_at ? shortTime(user.expires_at) : '不限'}
      </td>
      <td
        className="px-4 py-3 align-top min-w-[10rem]"
        title={
          user.period_used_calculated_at
            ? `计算于 ${shortTime(user.period_used_calculated_at)},约每 5 分钟更新`
            : '尚未计算(后台每 5 分钟刷新一次)'
        }
      >
        <QuotaBar used={user.period_used_bytes_cached} limit={user.traffic_limit_bytes_30d} />
      </td>
      <td className="px-4 py-3 align-top text-zinc-400 text-[12px]">{shortTime(user.created_at)}</td>
      <td className="px-4 py-3 align-top text-zinc-400 text-[12px]">{shortTime(user.updated_at)}</td>
      <td className="px-4 py-3 align-top text-right whitespace-nowrap">
        <button
          type="button"
          onClick={onEdit}
          className="rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-2.5 py-1 text-xs"
        >
          编辑
        </button>
        <Link
          to={`/rules?user_id=${user.id}`}
          className="ml-1.5 rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-2.5 py-1 text-xs"
        >
          规则
        </Link>
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

const TONE_CLS = {
  green: 'bg-emerald-500',
  amber: 'bg-amber-500',
  red: 'bg-red-500',
} as const

function QuotaBar({ used, limit }: { used: number; limit: number | null }) {
  const percent = quotaPercent(used, limit)
  if (percent == null) {
    return <span className="text-[12px] text-zinc-400">{formatBytes(used)} / 不限</span>
  }
  return (
    <div>
      <div
        className="h-1.5 w-full rounded-full bg-zinc-800 overflow-hidden"
        role="progressbar"
        aria-valuenow={Math.round(percent)}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={`30 天用量 ${percent.toFixed(0)}%`}
      >
        <div
          className={`h-full rounded-full ${TONE_CLS[quotaTone(percent)]}`}
          style={{ width: `${percent}%` }}
        />
      </div>
      <div className="text-[11px] text-zinc-400 mt-1">
        {formatBytes(used)} / {formatBytes(limit as number)}（{percent.toFixed(0)}%）
      </div>
    </div>
  )
}

// 评审 P2-11:后端存 UTC,datetime-local 是本地时区——此前 UTC 字符串直塞输入框,
// 用户得自己心算时区。两个 helper 负责双向转换。
/** UTC 'YYYY-MM-DD HH:MM:SS' → datetime-local 本地值 'YYYY-MM-DDTHH:MM' */
function utcToLocalInput(utc: string): string {
  const d = new Date(utc.replace(' ', 'T') + 'Z')
  if (Number.isNaN(d.getTime())) return ''
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`
}

/** datetime-local 本地值 → 后端要求的 UTC 'YYYY-MM-DDTHH:MM' */
function localInputToUtc(local: string): string {
  const d = new Date(local)
  if (Number.isNaN(d.getTime())) return local
  return d.toISOString().slice(0, 16)
}

/** P7 授权复选列表(节点/隧道共用):滚动容器内逐项 checkbox。 */
function GrantPicker({
  label,
  empty,
  options,
  chosen,
  onToggle,
}: {
  label: string
  empty: string
  options: { id: number; name: string }[]
  chosen: Set<number>
  onToggle: (id: number) => void
}) {
  return (
    <div>
      <label className={fieldLabelCls}>
        {label}
        <span className="ml-1 text-zinc-400 font-normal">({chosen.size})</span>
      </label>
      <div className="max-h-32 overflow-y-auto rounded-lg border border-white/10 bg-white/[0.03] px-2 py-1.5 space-y-0.5">
        {options.length === 0 ? (
          <div className="px-1 py-1 text-[12px] text-zinc-400">{empty}</div>
        ) : (
          options.map((o) => (
            <label
              key={o.id}
              className="flex items-center gap-2 rounded-md px-1.5 py-1 text-[12px] text-zinc-200 hover:bg-white/5 cursor-pointer"
            >
              <input
                type="checkbox"
                checked={chosen.has(o.id)}
                onChange={() => onToggle(o.id)}
              />
              <span className="truncate">{o.name}</span>
            </label>
          ))
        )}
      </div>
    </div>
  )
}

interface UserFormState {
  username: string
  password: string
  role: 'admin' | 'user'
  expires_at: string
  traffic_limit_gb: string
  /** '' = 滚动 30 天;'1'..'31' = 月度重置日 */
  quota_reset_day: string
  /** '' = 不限;'>0' = 转发规则数上限 */
  forward_rules_quota: string
}

function UserForm({
  mode,
  initial,
  onCancel,
  onSuccess,
}: {
  mode: 'create' | 'edit'
  initial?: UserDetail
  onCancel: () => void
  onSuccess: () => void | Promise<void>
}) {
  const [form, setForm] = useState<UserFormState>({
    username: initial?.username ?? '',
    password: '',
    role: initial?.role ?? 'user',
    // 回填:后端 UTC → 本地时区输入值。
    expires_at: initial?.expires_at ? utcToLocalInput(initial.expires_at) : '',
    traffic_limit_gb: bytesToGbString(initial?.traffic_limit_bytes_30d ?? null),
    quota_reset_day: initial?.quota_reset_day != null ? String(initial.quota_reset_day) : '',
    forward_rules_quota:
      initial?.forward_rules_quota != null ? String(initial.forward_rules_quota) : '',
  })
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // P7 授权多选:候选节点/隧道 + 当前勾选;编辑模式回显已授权再 diff,避免无变更也全量替换。
  const [nodeOptions, setNodeOptions] = useState<NodeView[]>([])
  const [tunnelOptions, setTunnelOptions] = useState<TunnelView[]>([])
  const [grantedNodes, setGrantedNodes] = useState<Set<number>>(new Set())
  const [grantedTunnels, setGrantedTunnels] = useState<Set<number>>(new Set())
  // per-隧道转发条数上限(tunnel_id → 字符串输入;'' = 不限)。
  const [tunnelLimits, setTunnelLimits] = useState<Record<number, string>>({})
  const [initialGrants, setInitialGrants] = useState<{
    nodes: number[]
    tunnels: number[]
    tunnelLimits: Record<number, string>
  } | null>(mode === 'create' ? { nodes: [], tunnels: [], tunnelLimits: {} } : null)

  useEffect(() => {
    let cancelled = false
    const work: [
      ReturnType<typeof nodes.list>,
      ReturnType<typeof tunnels.list>,
      Promise<Awaited<ReturnType<typeof users.grants>> | null>,
    ] = [
      nodes.list({ page_size: 100 }),
      tunnels.list({ page_size: 100 }),
      mode === 'edit' && initial ? users.grants(initial.id) : Promise.resolve(null),
    ]
    Promise.all(work)
      .then(([n, t, g]) => {
        if (cancelled) return
        setNodeOptions(n.items)
        setTunnelOptions(t.items)
        if (g) {
          setGrantedNodes(new Set(g.granted_node_ids))
          setGrantedTunnels(new Set(g.granted_tunnel_ids))
          const tl: Record<number, string> = {}
          for (const x of g.tunnel_forward_limits) {
            if (x.limit != null) tl[x.tunnel_id] = String(x.limit)
          }
          setTunnelLimits(tl)
          setInitialGrants({
            nodes: g.granted_node_ids,
            tunnels: g.granted_tunnel_ids,
            tunnelLimits: tl,
          })
        }
      })
      .catch(() => {
        // 加载失败:授权区选项为空(显示「暂无」);编辑模式 initialGrants 保持 null,
        // 提交时跳过授权字段,不会误发空数组清掉既有授权。
      })
    return () => {
      cancelled = true
    }
    // 仅挂载时拉一次。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  function sameIds(a: number[], chosen: Set<number>): boolean {
    return a.length === chosen.size && a.every((id) => chosen.has(id))
  }

  /** 当前已选隧道的有效上限(>0 整数);留空/非正视为不限,不下发。 */
  function buildTunnelLimits(): { tunnel_id: number; limit: number }[] {
    const out: { tunnel_id: number; limit: number }[] = []
    for (const tid of grantedTunnels) {
      const raw = (tunnelLimits[tid] ?? '').trim()
      if (raw === '') continue
      const n = Number(raw)
      if (Number.isInteger(n) && n > 0) out.push({ tunnel_id: tid, limit: n })
    }
    return out
  }

  /** 隧道授权 + 上限的可比较签名(集合或任一上限变化即不同)。 */
  function tunnelGrantKey(tunnels: Set<number>, limits: Record<number, string>): string {
    return [...tunnels]
      .sort((a, b) => a - b)
      .map((t) => `${t}:${(limits[t] ?? '').trim()}`)
      .join(',')
  }

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)
    setSubmitting(true)
    try {
      const limitBytes = gbToBytes(form.traffic_limit_gb)
      if (limitBytes === undefined) {
        setError('30 天用量上限必须是非负数字')
        setSubmitting(false)
        return
      }
      // 月度重置日:'' = 滚动;否则必须 1-31。
      let resetDay: number | undefined
      if (form.quota_reset_day.trim() !== '') {
        const d = Number(form.quota_reset_day)
        if (!Number.isInteger(d) || d < 1 || d > 31) {
          setError('月度重置日必须是 1-31，或留空表示滚动 30 天')
          setSubmitting(false)
          return
        }
        resetDay = d
      }
      // 转发规则数上限:'' = 不限;否则非负整数。
      let fwdQuota: number | undefined
      if (form.forward_rules_quota.trim() !== '') {
        const q = Number(form.forward_rules_quota)
        if (!Number.isInteger(q) || q < 0) {
          setError('转发规则数上限必须是非负整数,或留空表示不限')
          setSubmitting(false)
          return
        }
        fwdQuota = q
      }
      // 每隧道上限:留空 = 不限;否则须 ≥1 整数。
      for (const tid of grantedTunnels) {
        const raw = (tunnelLimits[tid] ?? '').trim()
        if (raw !== '') {
          const n = Number(raw)
          if (!Number.isInteger(n) || n < 1) {
            setError('隧道转发条数上限必须是 ≥1 的整数,或留空表示不限')
            setSubmitting(false)
            return
          }
        }
      }
      if (mode === 'create') {
        if (form.username.trim().length < 3) {
          setError('用户名长度需 3-32')
          setSubmitting(false)
          return
        }
        if (form.password.length < 8) {
          setError('密码长度需 ≥ 8')
          setSubmitting(false)
          return
        }
        const payload: CreateUserRequest = {
          username: form.username.trim(),
          password: form.password,
          role: form.role,
          expires_at: form.expires_at ? localInputToUtc(form.expires_at) : null,
          traffic_limit_bytes_30d: limitBytes,
        }
        if (resetDay !== undefined) payload.quota_reset_day = resetDay
        if (fwdQuota !== undefined && fwdQuota > 0) payload.forward_rules_quota = fwdQuota
        // 默认拒绝:不勾选即不发送(admin 角色不受授权限制,不发送)。
        if (form.role === 'user') {
          if (grantedNodes.size > 0) payload.granted_node_ids = [...grantedNodes]
          if (grantedTunnels.size > 0) {
            payload.granted_tunnel_ids = [...grantedTunnels]
            const lims = buildTunnelLimits()
            if (lims.length > 0) payload.tunnel_forward_limits = lims
          }
        }
        await users.create(payload)
      } else if (initial) {
        // 编辑时密码为空表示不改;角色变了才发送。
        const payload: UpdateUserRequest = {}
        if (form.password) {
          if (form.password.length < 8) {
            setError('新密码长度需 ≥ 8')
            setSubmitting(false)
            return
          }
          payload.password = form.password
        }
        if (form.role !== initial.role) payload.role = form.role
        const initialExpiresLocal = initial.expires_at
          ? utcToLocalInput(initial.expires_at)
          : ''
        if (form.expires_at !== initialExpiresLocal) {
          // '' = 清除;非空转回 UTC 提交。
          payload.expires_at = form.expires_at ? localInputToUtc(form.expires_at) : ''
        }
        const initialLimit = initial.traffic_limit_bytes_30d
        if ((limitBytes ?? 0) !== (initialLimit ?? 0)) {
          payload.traffic_limit_bytes_30d = limitBytes ?? 0 // 0 = 清除
        }
        // 月度重置日变更:0 清除(回滚动),1-31 设置。
        if ((resetDay ?? 0) !== (initial.quota_reset_day ?? 0)) {
          payload.quota_reset_day = resetDay ?? 0
        }
        // 转发规则数上限变更:0 清除(回不限),>0 设置。
        if ((fwdQuota ?? 0) !== (initial.forward_rules_quota ?? 0)) {
          payload.forward_rules_quota = fwdQuota ?? 0
        }
        // 授权变更检测:回显成功(initialGrants 非 null)且勾选有变化才发送全量替换。
        if (initialGrants && form.role === 'user') {
          if (!sameIds(initialGrants.nodes, grantedNodes)) {
            payload.granted_node_ids = [...grantedNodes]
          }
          // 隧道集合或任一上限变化即全量替换(set_tunnel_grants 一并写入 num)。
          if (
            tunnelGrantKey(grantedTunnels, tunnelLimits) !==
            tunnelGrantKey(new Set(initialGrants.tunnels), initialGrants.tunnelLimits)
          ) {
            payload.granted_tunnel_ids = [...grantedTunnels]
            payload.tunnel_forward_limits = buildTunnelLimits()
          }
        }
        if (Object.keys(payload).length === 0) {
          onCancel()
          return
        }
        await users.update(initial.id, payload)
      }
      await onSuccess()
    } catch (e) {
      setError(e instanceof ApiError ? e.message : '提交失败')
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <form noValidate onSubmit={onSubmit} className="space-y-4">
      <div>
        <label htmlFor="user-username" className={fieldLabelCls}>用户名 *</label>
        <input
          id="user-username"
          value={form.username}
          onChange={(e) => setForm((f) => ({ ...f, username: e.target.value }))}
          required
          disabled={mode === 'edit'}
          minLength={3}
          maxLength={32}
          className={fieldInputCls}
          placeholder="3-32 字符,无空白"
        />
        {mode === 'edit' && (
          <p className="text-[11px] text-zinc-400 mt-1">用户名不可改;新增用户即可指定。</p>
        )}
      </div>

      <div>
        <label htmlFor="user-password" className={fieldLabelCls}>
          密码 {mode === 'create' ? '*' : <span className="text-zinc-400">(留空即不改)</span>}
        </label>
        <PasswordInput
          id="user-password"
          value={form.password}
          onChange={(e) => setForm((f) => ({ ...f, password: e.target.value }))}
          required={mode === 'create'}
          minLength={mode === 'create' ? 8 : undefined}
          placeholder={mode === 'create' ? '≥ 8 字符' : '留空不改'}
          autoComplete="new-password"
        />
      </div>

      <div>
        <label htmlFor="user-role" className={fieldLabelCls}>角色 *</label>
        <select
          id="user-role"
          value={form.role}
          onChange={(e) => setForm((f) => ({ ...f, role: e.target.value as 'admin' | 'user' }))}
          className={fieldInputCls}
        >
          <option value="user">user (普通用户)</option>
          <option value="admin">admin (管理员)</option>
        </select>
        <p className="text-[11px] text-zinc-400 mt-1">
          系统至少保留一个 admin;删除最后一个 admin 或将其降级会被拒绝。
        </p>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div>
          <label htmlFor="user-expires" className={fieldLabelCls}>到期时间（本地时区）</label>
          <input
            id="user-expires"
            type="datetime-local"
            value={form.expires_at}
            onChange={(e) => setForm((f) => ({ ...f, expires_at: e.target.value }))}
            className={fieldInputCls}
          />
          <p className="text-[11px] text-zinc-400 mt-1">留空 = 永不到期。到期后规则自动停用、登录被拒。</p>
        </div>
        <div>
          <label htmlFor="user-traffic" className={fieldLabelCls}>30 天用量上限 (GB)</label>
          <input
            id="user-traffic"
            type="number"
            min={0}
            step="0.5"
            value={form.traffic_limit_gb}
            onChange={(e) => setForm((f) => ({ ...f, traffic_limit_gb: e.target.value }))}
            className={fieldInputCls}
            placeholder="留空 = 不限"
          />
          <p className="text-[11px] text-zinc-400 mt-1">超限后该用户全部规则自动停用。重置方式见下。</p>
        </div>
        <div>
          <label htmlFor="user-reset" className={fieldLabelCls}>用量重置方式</label>
          <select
            id="user-reset"
            value={form.quota_reset_day}
            onChange={(e) => setForm((f) => ({ ...f, quota_reset_day: e.target.value }))}
            className={fieldInputCls}
          >
            <option value="">滚动 30 天窗口</option>
            {Array.from({ length: 31 }, (_, i) => i + 1).map((d) => (
              <option key={d} value={String(d)}>
                每月 {d} 日重置
              </option>
            ))}
          </select>
          <p className="text-[11px] text-zinc-400 mt-1">
            滚动窗口按最近 30 天统计；月度则每月固定日 0 点清零（月末容错）。
          </p>
        </div>
        <div>
          <label htmlFor="user-fwdquota" className={fieldLabelCls}>转发规则数上限</label>
          <input
            id="user-fwdquota"
            type="number"
            min={0}
            step={1}
            value={form.forward_rules_quota}
            onChange={(e) => setForm((f) => ({ ...f, forward_rules_quota: e.target.value }))}
            className={fieldInputCls}
            placeholder="留空 = 不限"
          />
          <p className="text-[11px] text-zinc-400 mt-1">该用户最多可创建的转发规则数;留空/0 = 不限。</p>
        </div>
      </div>

      {/* P7 授权:仅 user 角色;默认拒绝,勾选即授权。撤销不影响存量规则,仅禁止新建。 */}
      {form.role === 'user' && (
        <div className="grid grid-cols-2 gap-3">
          <GrantPicker
            label="可用节点"
            empty="暂无节点"
            options={nodeOptions.map((n) => ({ id: n.id, name: n.name }))}
            chosen={grantedNodes}
            onToggle={(id) =>
              setGrantedNodes((s) => {
                const next = new Set(s)
                if (next.has(id)) next.delete(id)
                else next.add(id)
                return next
              })
            }
          />
          <GrantPicker
            label="可用隧道"
            empty="暂无隧道"
            options={tunnelOptions.map((t) => ({
              id: t.id,
              name: `${t.name}（${t.transport.toUpperCase()}）`,
            }))}
            chosen={grantedTunnels}
            onToggle={(id) =>
              setGrantedTunnels((s) => {
                const next = new Set(s)
                if (next.has(id)) next.delete(id)
                else next.add(id)
                return next
              })
            }
          />
          <p className="col-span-2 text-[11px] text-zinc-400 -mt-1">
            默认拒绝:未勾选的节点/隧道该用户不可见、不可建规则。撤销授权不影响已建规则(保留运行,仅禁止新建)。
          </p>
          {grantedTunnels.size > 0 && (
            <div className="col-span-2">
              <label className={fieldLabelCls}>各隧道转发条数上限（可选,留空 = 不限）</label>
              <div className="rounded-lg border border-white/10 bg-white/[0.03] px-2 py-1.5 space-y-1">
                {tunnelOptions
                  .filter((t) => grantedTunnels.has(t.id))
                  .map((t) => (
                    <div key={t.id} className="flex items-center gap-2 text-[12px]">
                      <span className="truncate flex-1 text-zinc-300">{t.name}</span>
                      <input
                        type="number"
                        min={1}
                        step={1}
                        value={tunnelLimits[t.id] ?? ''}
                        onChange={(e) =>
                          setTunnelLimits((m) => ({ ...m, [t.id]: e.target.value }))
                        }
                        className={`${fieldInputCls} w-24`}
                        placeholder="不限"
                      />
                    </div>
                  ))}
              </div>
            </div>
          )}
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
