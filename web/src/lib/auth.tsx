import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { ApiError, auth as authApi, clearToken, getToken, setToken, type UserView } from './api'
import { AuthContext } from './auth-context'

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<UserView | null>(null)
  // 初始 loading 与 token 存在性挂钩：无 token 直接 false，
  // 避免 effect 内同步 setState（react-hooks/set-state-in-effect）。
  const [loading, setLoading] = useState(() => !!getToken())

  useEffect(() => {
    const token = getToken()
    if (!token) return
    authApi
      .me()
      .then(setUser)
      .catch((e: unknown) => {
        if (e instanceof ApiError && e.status === 401) clearToken()
      })
      .finally(() => setLoading(false))
  }, [])

  const login = useCallback(async (username: string, password: string) => {
    const resp = await authApi.login(username, password)
    setToken(resp.token)
    setUser(resp.user)
  }, [])

  const logout = useCallback(async () => {
    try {
      await authApi.logout()
    } catch {
      // 网络失败也不阻止本地清理。
    }
    clearToken()
    setUser(null)
  }, [])

  return (
    <AuthContext.Provider value={{ user, loading, login, logout }}>{children}</AuthContext.Provider>
  )
}
