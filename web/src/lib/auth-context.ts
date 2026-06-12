import { createContext } from 'react'
import type { UserView } from './api'

export interface AuthState {
  user: UserView | null
  loading: boolean
  /** 首登强制改密未完成:为 true 时全站受保护路由被挡到改密页。 */
  mustChangePassword: boolean
  /** login 返回是否要求强制改密(供登录页决定跳转)。 */
  login: (username: string, password: string) => Promise<boolean>
  logout: () => Promise<void>
  /** 改密成功后清除强制改密态,放行回正常路由。 */
  markPasswordChanged: () => void
}

// 拆到独立文件：让 auth.tsx 只 export 组件，避免破坏 react-refresh HMR
// (react-refresh/only-export-components rule)。
export const AuthContext = createContext<AuthState | null>(null)
