import { createContext } from 'react'
import type { UserView } from './api'

export interface AuthState {
  user: UserView | null
  loading: boolean
  login: (username: string, password: string) => Promise<void>
  logout: () => Promise<void>
}

// 拆到独立文件：让 auth.tsx 只 export 组件，避免破坏 react-refresh HMR
// (react-refresh/only-export-components rule)。
export const AuthContext = createContext<AuthState | null>(null)
