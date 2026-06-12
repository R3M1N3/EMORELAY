export type ExpiryLevel = 'expired' | 'critical' | 'warning' | 'info'

export interface ExpiryWarning {
  level: ExpiryLevel
  message: string
  /** 剩余天数(向上取整);已过期为 0。 */
  daysLeft: number
}

const DAY_MS = 24 * 60 * 60 * 1000

// 账号到期预警(对标 flux dashboard 的分级提醒)。
// 返回 null 表示无需提醒(永不过期 / 距到期 > 7 天 / 时间不可解析)。
// 纯函数:给定 expiresAt 与 now 结果确定,便于测试与在组件里据此 localStorage 去重。
export function expiryWarning(expiresAt: string | null, now: number): ExpiryWarning | null {
  if (!expiresAt) return null
  const exp = Date.parse(expiresAt)
  if (Number.isNaN(exp)) return null

  const diff = exp - now
  if (diff <= 0) {
    return { level: 'expired', message: '账号已到期，请联系管理员续期', daysLeft: 0 }
  }
  const daysLeft = Math.ceil(diff / DAY_MS)
  if (daysLeft > 7) return null

  if (daysLeft <= 1) {
    return { level: 'critical', message: '账号将在 1 天内到期，请尽快联系管理员续期', daysLeft }
  }
  if (daysLeft <= 3) {
    return { level: 'warning', message: `账号将在 ${daysLeft} 天内到期，请尽快续期`, daysLeft }
  }
  return { level: 'info', message: `账号将在 ${daysLeft} 天后到期`, daysLeft }
}

// 去重 key:同一天同一级别只提醒一次(避免每次进页/30s 刷新重复弹)。
// now 用本地日期分桶,跨天后重新提醒。
export function expiryWarningKey(level: ExpiryLevel, now: number): string {
  const d = new Date(now)
  const day = `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`
  return `emorelay-expiry-warn-${level}-${day}`
}
