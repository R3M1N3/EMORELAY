import { useEffect } from 'react'
import { system } from './api'
import { useAutoRefresh } from './use-auto-refresh'

/**
 * 把管理员配置的全局强调色写入 :root 的 --ui-accent。
 * null/非法值 = 清除覆盖,回退 index.css 里的默认色。
 * 颜色值仅接受 #rrggbb(后端已校验,这里再防一手,杜绝注入 style)。
 */
export function applyAccent(color: string | null) {
  const root = document.documentElement
  if (color && /^#[0-9a-fA-F]{6}$/.test(color)) {
    root.style.setProperty('--ui-accent', color)
  } else {
    root.style.removeProperty('--ui-accent')
  }
}

/**
 * 全站主题同步:挂载时拉一次 /api/ui-config,之后 30s 静默轮询。
 * 端点免鉴权,登录页同样生效;管理员改色后所有客户端最迟 30s 内跟进。
 * 拉取失败保持现状(不清除已生效的颜色),等下个周期自愈。
 */
export function useThemeSync() {
  useEffect(() => {
    system.uiConfig().then((c) => applyAccent(c.accent_color)).catch(() => {})
  }, [])
  useAutoRefresh(() => {
    system.uiConfig().then((c) => applyAccent(c.accent_color)).catch(() => {})
  }, 30_000)
}
