import { useState, type FormEvent } from 'react'
import { useNavigate } from 'react-router-dom'
import { useAuth } from '../lib/use-auth'
import { ApiError } from '../lib/api'

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
      await login(username, password)
      navigate('/', { replace: true })
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
    <div className="min-h-svh bg-zinc-950 text-zinc-100 flex items-center justify-center px-4 relative overflow-hidden">
      {/* 背景毛玻璃光晕 */}
      <div className="absolute -top-32 -left-32 w-96 h-96 rounded-full bg-indigo-600/30 blur-3xl" aria-hidden />
      <div className="absolute -bottom-32 -right-32 w-96 h-96 rounded-full bg-violet-600/30 blur-3xl" aria-hidden />

      <form
        onSubmit={onSubmit}
        className="relative w-full max-w-sm rounded-2xl border border-white/10 bg-zinc-900/60 backdrop-blur-xl p-8 shadow-2xl"
      >
        <div className="mb-6 text-center">
          <h1 className="text-2xl font-semibold tracking-tight">EMORELAY</h1>
          <p className="mt-1 text-sm text-zinc-400">流量转发管理面板</p>
        </div>

        <label className="block text-xs font-medium text-zinc-300 mb-1.5">用户名</label>
        <input
          type="text"
          autoComplete="username"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          required
          className="w-full rounded-lg bg-zinc-800/80 border border-white/10 px-3 py-2 text-sm placeholder:text-zinc-500 focus:outline-none focus:ring-2 focus:ring-indigo-500/60"
          placeholder="admin"
        />

        <label className="block text-xs font-medium text-zinc-300 mt-4 mb-1.5">密码</label>
        <input
          type="password"
          autoComplete="current-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          required
          className="w-full rounded-lg bg-zinc-800/80 border border-white/10 px-3 py-2 text-sm placeholder:text-zinc-500 focus:outline-none focus:ring-2 focus:ring-indigo-500/60"
          placeholder="••••••••"
        />

        {error && (
          <div className="mt-4 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200">
            {error}
          </div>
        )}

        <button
          type="submit"
          disabled={submitting}
          className="mt-6 w-full rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium transition-colors"
        >
          {submitting ? '登录中…' : '登录'}
        </button>
      </form>
    </div>
  )
}
