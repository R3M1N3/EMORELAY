import { useState, type FormEvent } from 'react'
import { useNavigate } from 'react-router-dom'
import { useAuth } from '../lib/use-auth'
import { ApiError } from '../lib/api'
import { Backdrop, fieldInputCls, PasswordInput } from '../lib/ui'

export default function Login() {
  const navigate = useNavigate()
  const { login } = useAuth()
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)
    setSubmitting(true)
    try {
      const mustChange = await login(username, password)
      navigate(mustChange ? '/change-password' : '/', { replace: true })
    } catch (e) {
      if (e instanceof ApiError) {
        if (e.status === 401 && e.message === 'account_expired') {
          setError('账号已到期，请联系管理员')
        } else if (e.status === 401) {
          setError('用户名或密码错误')
        } else {
          setError(e.message)
        }
      } else {
        setError('登录失败，请检查网络')
      }
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="min-h-svh text-zinc-100 flex items-center justify-center px-4 relative overflow-hidden">
      <Backdrop />

      <form
        onSubmit={onSubmit}
        className="relative w-full max-w-sm glass-card rise p-8 shadow-2xl"
      >
        <div className="mb-6 text-center">
          <h1 className="text-2xl font-bold tracking-[0.14em] bg-gradient-to-r from-white via-accent-hi to-white bg-clip-text text-transparent">
            EMORELAY
          </h1>
          <p className="mt-1.5 text-sm text-zinc-400">流量转发管理面板</p>
        </div>

        <label htmlFor="login-username" className="block text-xs font-medium text-zinc-300 mb-1.5">用户名</label>
        <input
          id="login-username"
          type="text"
          autoComplete="username"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          required
          className={fieldInputCls}
          placeholder="admin"
        />

        <label htmlFor="login-password" className="block text-xs font-medium text-zinc-300 mt-4 mb-1.5">密码</label>
        <PasswordInput
          id="login-password"
          autoComplete="current-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          required
          placeholder="••••••••"
        />

        {error && (
          <div role="alert" className="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200">
            {error}
          </div>
        )}

        <button type="submit" disabled={submitting} className="mt-6 w-full btn-accent">
          {submitting ? '登录中…' : '登录'}
        </button>
      </form>
    </div>
  )
}
