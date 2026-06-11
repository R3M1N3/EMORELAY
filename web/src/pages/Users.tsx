import { useEffect, useState, type FormEvent } from 'react'
import {
  ApiError,
  formatBytes,
  shortTime,
  users,
  type CreateUserRequest,
  type UpdateUserRequest,
  type UserDetail,
} from '../lib/api'
import { Modal, fieldInputCls, fieldLabelCls } from '../lib/ui'
import { Pagination } from '../components/Pagination'
import { bytesToGbString, gbToBytes, quotaPercent, quotaTone } from '../lib/quota'

type Editing = { mode: 'create' } | { mode: 'edit'; user: UserDetail } | null

interface ListState {
  items: UserDetail[]
  total: number
  loading: boolean
  error: string | null
}

export default function Users() {
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
          className="rounded-lg bg-indigo-600 hover:bg-indigo-500 px-3 py-2 text-sm font-medium shrink-0"
        >
          新增用户
        </button>
      </div>

      {list.error && (
        <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-200">
          {list.error}
        </div>
      )}

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
          placeholder="搜索用户名"
          className={`${fieldInputCls} max-w-sm`}
        />
        <button type="submit" className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm">
          搜索
        </button>
      </form>

      <section className="rounded-2xl border border-white/10 bg-zinc-900/40 overflow-hidden">
        {list.loading ? (
          <div className="p-6 text-sm text-zinc-400">加载中…</div>
        ) : list.items.length === 0 ? (
          <div className="p-6 text-sm text-zinc-500">
            {search.trim() ? '没有匹配的用户。' : '尚无用户。'}
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-[11px] uppercase text-zinc-500 bg-zinc-900/80">
                <tr>
                  <th className="px-4 py-2.5 text-left font-medium">用户名</th>
                  <th className="px-4 py-2.5 text-left font-medium">角色</th>
                  <th className="px-4 py-2.5 text-right font-medium">规则数</th>
                  <th className="px-4 py-2.5 text-right font-medium">累计流量</th>
                  <th className="px-4 py-2.5 text-left font-medium">到期</th>
                  <th className="px-4 py-2.5 text-left font-medium" title="约每 5 分钟由后台刷新">
                    30d 用量
                  </th>
                  <th className="px-4 py-2.5 text-left font-medium">创建于</th>
                  <th className="px-4 py-2.5 text-left font-medium">更新于</th>
                  <th className="px-4 py-2.5 text-right font-medium">操作</th>
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
              className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm"
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
      ? 'bg-indigo-500/15 border-indigo-500/40 text-indigo-200'
      : 'bg-zinc-500/15 border-zinc-500/40 text-zinc-300'
  return (
    <tr className="hover:bg-white/[0.02]">
      <td className="px-4 py-3 align-top">
        <div className="font-medium text-zinc-100">{user.username}</div>
        <div className="text-[11px] text-zinc-500 mt-0.5">ID #{user.id}</div>
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
          className="rounded-md bg-zinc-800 hover:bg-zinc-700 px-2.5 py-1 text-xs"
        >
          编辑
        </button>
        <button
          type="button"
          onClick={onDelete}
          className="ml-1.5 rounded-md bg-red-600/80 hover:bg-red-500 px-2.5 py-1 text-xs"
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
    return <span className="text-[12px] text-zinc-500">{formatBytes(used)} / 不限</span>
  }
  return (
    <div>
      <div className="h-1.5 w-full rounded-full bg-zinc-800 overflow-hidden">
        <div
          className={`h-full rounded-full ${TONE_CLS[quotaTone(percent)]}`}
          style={{ width: `${percent}%` }}
        />
      </div>
      <div className="text-[11px] text-zinc-500 mt-1">
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

interface UserFormState {
  username: string
  password: string
  role: 'admin' | 'user'
  expires_at: string
  traffic_limit_gb: string
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
  })
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

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
    <form onSubmit={onSubmit} className="space-y-4">
      <div>
        <label className={fieldLabelCls}>用户名 *</label>
        <input
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
          <p className="text-[11px] text-zinc-500 mt-1">用户名不可改;新增用户即可指定。</p>
        )}
      </div>

      <div>
        <label className={fieldLabelCls}>
          密码 {mode === 'create' ? '*' : <span className="text-zinc-500">(留空即不改)</span>}
        </label>
        <input
          type="password"
          value={form.password}
          onChange={(e) => setForm((f) => ({ ...f, password: e.target.value }))}
          required={mode === 'create'}
          minLength={mode === 'create' ? 8 : undefined}
          className={fieldInputCls}
          placeholder={mode === 'create' ? '≥ 8 字符' : '留空不改'}
          autoComplete="new-password"
        />
      </div>

      <div>
        <label className={fieldLabelCls}>角色 *</label>
        <select
          value={form.role}
          onChange={(e) => setForm((f) => ({ ...f, role: e.target.value as 'admin' | 'user' }))}
          className={fieldInputCls}
        >
          <option value="user">user (普通用户)</option>
          <option value="admin">admin (管理员)</option>
        </select>
        <p className="text-[11px] text-zinc-500 mt-1">
          系统至少保留一个 admin;删除最后一个 admin 或将其降级会被拒绝。
        </p>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className={fieldLabelCls}>到期时间（本地时区）</label>
          <input
            type="datetime-local"
            value={form.expires_at}
            onChange={(e) => setForm((f) => ({ ...f, expires_at: e.target.value }))}
            className={fieldInputCls}
          />
          <p className="text-[11px] text-zinc-500 mt-1">留空 = 永不到期。到期后规则自动停用、登录被拒。</p>
        </div>
        <div>
          <label className={fieldLabelCls}>30 天用量上限 (GB)</label>
          <input
            type="number"
            min={0}
            step="0.5"
            value={form.traffic_limit_gb}
            onChange={(e) => setForm((f) => ({ ...f, traffic_limit_gb: e.target.value }))}
            className={fieldInputCls}
            placeholder="留空 = 不限"
          />
          <p className="text-[11px] text-zinc-500 mt-1">滚动 30 天窗口;超限后该用户全部规则自动停用。</p>
        </div>
      </div>

      {error && (
        <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200">
          {error}
        </div>
      )}

      <div className="flex justify-end gap-2 pt-1">
        <button
          type="button"
          onClick={onCancel}
          disabled={submitting}
          className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm"
        >
          取消
        </button>
        <button
          type="submit"
          disabled={submitting}
          className="rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
        >
          {submitting ? '提交中…' : mode === 'create' ? '创建' : '保存'}
        </button>
      </div>
    </form>
  )
}
