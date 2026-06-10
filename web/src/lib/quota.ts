// 30 天用量进度条的纯函数(配色阈值 / GB↔bytes 转换)。

export type QuotaTone = 'green' | 'amber' | 'red'

/** used/limit 百分比,clamp 0-100;limit null/0 = 不限 → null。 */
export function quotaPercent(used: number, limit: number | null): number | null {
  if (limit == null || limit <= 0) return null
  return Math.min(100, Math.max(0, (used / limit) * 100))
}

/** 绿 <70 / 橙 70-90 / 红 ≥90。 */
export function quotaTone(percent: number): QuotaTone {
  if (percent >= 90) return 'red'
  if (percent >= 70) return 'amber'
  return 'green'
}

/** 表单 GB 字符串 → bytes。'' → null(不限);非法 → undefined(校验失败)。 */
export function gbToBytes(v: string): number | null | undefined {
  const s = v.trim()
  if (s === '') return null
  const n = Number(s)
  if (!Number.isFinite(n) || n < 0) return undefined
  return Math.round(n * 1024 ** 3)
}

/** bytes → 表单 GB 字符串(去尾零);null → ''。 */
export function bytesToGbString(bytes: number | null): string {
  if (bytes == null) return ''
  return String(parseFloat((bytes / 1024 ** 3).toFixed(2)))
}
