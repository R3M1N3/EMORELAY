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
      const r = await users.list({ page, page_size: pageSize })
      setList({ items: r.items, total: r.total, loading: false, error: null })
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '加载失败'
      setList({ items: [], total: 0, loading: false, error: msg })
    }
  }

  useEffect(() => {
    let cancelled = false
    users
      .list({ page, page_size: pageSize })
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

      {(() => {
        const needle = search.trim().toLowerCase()
        const filtered = needle
          ? list.items.filter((u) => u.username.toLowerCase().includes(needle))
          : list.items
        return (
          <>
            <div className="flex items-center gap-3 flex-wrap">
              <input
                type="search"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="搜索当前页 (用户名)"
                className={`${fieldInputCls} max-w-sm`}
              />
              {needle && (
                <span className="text-xs text-zinc-500">
                  匹配 {filtered.length} / {list.items.length} 条 (仅当前页)
                </span>
              )}
            </div>

            <section className="rounded-2xl border border-white/10 bg-zinc-900/40 overflow-hidden">
              {list.loading ? (
                <div className="p-6 text-sm text-zinc-400">加载中…</div>
              ) : list.items.length === 0 ? (
                <div className="p-6 text-sm text-zinc-500">尚无用户。</div>
              ) : filtered.length === 0 ? (
                <div className="p-6 text-sm text-zinc-500">没有匹配的用户。</div>
              ) : (
                <div className="overflow-x-auto">
                  <table className="w-full text-sm">
                    <thead className="text-[11px] uppercase text-zinc-500 bg-zinc-900/80">
                      <tr>
                        <th className="px-4 py-2.5 text-left font-medium">用户名</th>
                        <th className="px-4 py-2.5 text-left font-medium">角色</th>
                        <th className="px-4 py-2.5 text-right font-medium">规则数</th>
                        <th className="px-4 py-2.5 text-right font-medium">累计流量</th>
                        <th className="px-4 py-2.5 text-left font-medium">创建于</th>
                        <th className="px-4 py-2.5 text-left font-medium">更新于</th>
                        <th className="px-4 py-2.5 text-right font-medium">操作</th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-white/5">
                      {filtered.map((u) => (
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
          </>
        )
      })()}

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
          <p className="text-sm text-zinc-300">
            将删除用户 <span className="text-white font-medium">{confirming.username}</span>。
            该用户名下的规则不会被自动清理,请先在「规则」页处理。
          </p>
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

interface UserFormState {
  username: string
  password: string
  role: 'admin' | 'user'
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
  })
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)
    setSubmitting(true)
    try {
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
