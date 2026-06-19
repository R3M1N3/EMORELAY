import { useState, type FormEvent } from 'react'
import { useAuth } from '../lib/use-auth'
import { useToast } from '../lib/use-toast'
import { ApiError, auth as authApi } from '../lib/api'
import { Backdrop, PasswordInput } from '../lib/ui'

// 首登强制改密页(对标 flux change-password):无侧栏布局,改成功前出不去。
// 由 App 路由守卫保证:mustChangePassword 为 true 时全站受保护路由都跳到这里。
export default function ChangePassword() {
  const toast = useToast()
  const { user, logout } = useAuth()
  const [oldPassword, setOldPassword] = useState('')
  const [newPassword, setNewPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)
    if (newPassword.length < 8) {
      setError('新密码长度至少 8 个字符')
      return
    }
    if (newPassword !== confirm) {
      setError('两次输入的新密码不一致')
      return
    }
    setSubmitting(true)
    try {
      await authApi.changePassword(oldPassword, newPassword)
      toast.success('密码已修改，请用新密码重新登录')
      // 旧 token 仍带 mcp 标志(仅可访问 me/改密),必须清掉换新 token,故强制重登。
      logout()
    } catch (e) {
      setError(e instanceof ApiError ? e.message : '修改失败，请重试')
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="min-h-svh text-zinc-100 flex items-center justify-center px-4 relative overflow-hidden">
      <Backdrop />
      <form onSubmit={onSubmit} className="relative w-full max-w-sm glass-card rise p-8 shadow-2xl">
        <div className="mb-6 text-center">
          <h1 className="text-xl font-bold tracking-wide text-zinc-100">设置新密码</h1>
          <p className="mt-1.5 text-sm text-zinc-400">
            {user ? `${user.username}，` : ''}首次登录或密码被重置，请先设置自己的密码
          </p>
        </div>

        <label htmlFor="cp-old" className="block text-xs font-medium text-zinc-300 mb-1.5">当前密码</label>
        <PasswordInput
          id="cp-old"
          autoComplete="current-password"
          value={oldPassword}
          onChange={(e) => setOldPassword(e.target.value)}
          required
          placeholder="••••••••"
        />

        <label htmlFor="cp-new" className="block text-xs font-medium text-zinc-300 mt-4 mb-1.5">新密码</label>
        <PasswordInput
          id="cp-new"
          autoComplete="new-password"
          value={newPassword}
          onChange={(e) => setNewPassword(e.target.value)}
          required
          placeholder="至少 8 个字符"
        />

        <label htmlFor="cp-confirm" className="block text-xs font-medium text-zinc-300 mt-4 mb-1.5">确认新密码</label>
        <PasswordInput
          id="cp-confirm"
          autoComplete="new-password"
          value={confirm}
          onChange={(e) => setConfirm(e.target.value)}
          required
          placeholder="再次输入新密码"
        />
        {confirm !== '' && confirm !== newPassword && (
          <p aria-live="polite" className="mt-1.5 text-[11px] text-red-300">两次输入的新密码不一致</p>
        )}

        {error && (
          <div role="alert" className="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200">
            {error}
          </div>
        )}

        <button type="submit" disabled={submitting} className="mt-6 w-full btn-accent">
          {submitting ? '提交中…' : '确认修改'}
        </button>
        <button
          type="button"
          onClick={logout}
          className="mt-3 w-full text-xs text-zinc-400 hover:text-zinc-300"
        >
          退出登录
        </button>
      </form>
    </div>
  )
}
