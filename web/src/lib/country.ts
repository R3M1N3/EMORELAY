// 节点地区:ISO 3166-1 alpha-2 国家码工具。
// 设计:region 字段沿用自由文本(无迁移),前端收敛为「下拉建议 + 手填」;
// 中文名由浏览器原生 Intl.DisplayNames 动态生成(零依赖);国旗用 flag-icons 图片渲染。
// 历史/非法值一律降级原样显示,不破坏存量数据。

// 常见转发节点国家/地区(下拉建议用,非强制枚举——用户可手填任意码)。
export const COMMON_COUNTRY_CODES: string[] = [
  'HK', 'MO', 'TW', 'CN', 'JP', 'KR', 'SG', 'MY', 'TH', 'VN',
  'PH', 'ID', 'IN', 'US', 'CA', 'MX', 'GB', 'DE', 'FR', 'NL',
  'IT', 'ES', 'SE', 'CH', 'RU', 'TR', 'AE', 'AU', 'NZ', 'BR',
  'AR', 'ZA', 'UA', 'PL',
]

const CODE_RE = /^[A-Za-z]{2}$/

/** 是否形如 ISO alpha-2 码(两个字母,大小写不敏感)。 */
export function isCountryCode(s: string): boolean {
  return CODE_RE.test(s.trim())
}

// Intl.DisplayNames 实例懒加载并缓存;不支持的环境降级为 null。
let displayNames: Intl.DisplayNames | null | undefined
function getDisplayNames(): Intl.DisplayNames | null {
  if (displayNames === undefined) {
    try {
      displayNames = new Intl.DisplayNames(['zh-Hans'], { type: 'region' })
    } catch {
      displayNames = null
    }
  }
  return displayNames
}

/** ISO 码 → 中文国名;非法码/不识别/环境不支持时返回原值(降级)。 */
export function countryName(code: string): string {
  const cc = code.trim().toUpperCase()
  if (!CODE_RE.test(cc)) return code
  const dn = getDisplayNames()
  if (!dn) return cc
  try {
    return dn.of(cc) ?? cc
  } catch {
    return cc
  }
}

/** 表单提交规范化:2 字母码统一大写;其余原样(兼容历史自由文本)。 */
export function normalizeRegion(s: string): string {
  const t = s.trim()
  return CODE_RE.test(t) ? t.toUpperCase() : t
}
