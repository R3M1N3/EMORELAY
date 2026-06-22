import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { ApiError, auth as authApi, clearToken, getToken, setToken, type UserView } from './api'
import { AuthContext } from './auth-context'
import { useToast } from './use-toast'

export function AuthProvider({ children }: { children: ReactNode }) {
  const toast = useToast()
  const [user, setUser] = useState<UserView | null>(null)
  const [mustChangePassword, setMustChangePassword] = useState(false)
  // 初始 loading 与 token 存在性挂钩：无 token 直接 false，
  // 避免 effect 内同步 setState（react-hooks/set-state-in-effect）。
  const [loading, setLoading] = useState(() => !!getToken())

  // user 存 ref 供 emorelay:unauthorized 回调读最新值(免得监听 effect 依赖 user 反复重挂)。
  const userRef = useRef(user)
  useEffect(() => {
    userRef.current = user
  })
  // 会话失效去重:失效瞬间页面常并发多个请求同时 401(各派发一次),只提示 + 登出一次;
  // 重新登录(login 成功)后重置,使下次失效仍能提示。
  const unauthorizedNotified = useRef(false)

  useEffect(() => {
    const token = getToken()
    if (!token) return
    authApi
      .me()
      .then((me) => {
        setUser(me)
        // 刷新/重进时也据 me() 反映强制改密,挡住非登录入口。
        setMustChangePassword(me.must_change_password)
      })
      .catch((e: unknown) => {
        if (e instanceof ApiError && e.status === 401) clearToken()
      })
      .finally(() => setLoading(false))
  }, [])

  // 会话中途失效(token 过期 / 被吊销 / 另端登出):api 层广播 emorelay:unauthorized。
  // 仅当「当前确有会话」时登出 + 提示(排除登录页自身的 401:此时 userRef 为 null,静默忽略),
  // setUser(null) 后 ProtectedShell 路由守卫自动跳登录。toast 由 useMemo 稳定,effect 只挂一次。
  useEffect(() => {
    const onUnauthorized = () => {
      if (!userRef.current || unauthorizedNotified.current) return
      unauthorizedNotified.current = true
      setUser(null)
      setMustChangePassword(false)
      toast.error('登录状态已失效,请重新登录')
    }
    window.addEventListener('emorelay:unauthorized', onUnauthorized)
    return () => window.removeEventListener('emorelay:unauthorized', onUnauthorized)
  }, [toast])

  const login = useCallback(async (username: string, password: string) => {
    const resp = await authApi.login(username, password)
    setToken(resp.token)
    setUser(resp.user)
    setMustChangePassword(resp.must_change_password)
    unauthorizedNotified.current = false
    return resp.must_change_password
  }, [])

  const logout = useCallback(async () => {
    try {
      await authApi.logout()
    } catch {
      // 网络失败也不阻止本地清理。
    }
    clearToken()
    setUser(null)
    setMustChangePassword(false)
  }, [])

  return (
    <AuthContext.Provider
      value={{ user, loading, mustChangePassword, login, logout }}
    >
      {children}
    </AuthContext.Provider>
  )
}
