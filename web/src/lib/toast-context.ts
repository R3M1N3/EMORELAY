// Toast Context 与共享类型定义。从 toast.tsx 拆出以满足 react-refresh/only-export-components
// (provider 与 hook 必须分别 export 自不同文件)。
import { createContext } from 'react'

export type ToastKind = 'success' | 'error' | 'info'

export interface ToastApi {
  success: (msg: string) => void
  error: (msg: string) => void
  info: (msg: string) => void
}

export const ToastContext = createContext<ToastApi | null>(null)
