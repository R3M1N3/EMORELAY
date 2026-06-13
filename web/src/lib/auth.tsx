import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { ApiError, auth as authApi, clearToken, getToken, setToken, type UserView } from './api'
import { AuthContext } from './auth-context'

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<UserView | null>(null)
  const [mustChangePassword, setMustChangePassword] = useState(false)
  // 初始 loading 与 token 存在性挂钩：无 token 直接 false，
  // 避免 effect 内同步 setState（react-hooks/set-state-in-effect）。
  const [loading, setLoading] = useState(() => !!getToken())

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

  const login = useCallback(async (username: string, password: string) => {
    const resp = await authApi.login(username, password)
    setToken(resp.token)
    setUser(resp.user)
    setMustChangePassword(resp.must_change_password)
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
