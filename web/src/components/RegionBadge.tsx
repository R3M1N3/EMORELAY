import { useState } from 'react'
import { countryName, isCountryCode } from '../lib/country'

// 节点地区展示:合法 ISO alpha-2 码 → 自托管国旗 SVG(public/flags/) + 中文名;
// 否则原样文本(兼容历史自由文本),空值显示「—」。
// 用图片国旗而非 emoji:Windows Chrome/Edge 不渲染 emoji 国旗。
// SVG 仅常用国(避免引整套 flag-icons CSS 把首屏 CSS 撑到 ~470KB);冷门码 onError 降级为有名无旗。
export function RegionBadge({ region }: { region: string }) {
  const r = (region ?? '').trim()
  if (!r) return <>—</>
  if (!isCountryCode(r)) return <>{r}</>
  const code = r.toLowerCase()
  return (
    <span className="inline-flex items-center gap-1.5">
      {/* key=code:region 变化时重挂载,重置 onError 隐藏状态(避免冷门码失败后污染复用该实例的后续码) */}
      <FlagImg key={code} code={code} />
      {countryName(r)}
    </span>
  )
}

// 国旗 SVG 单独成组件:onError 隐藏状态随 key=code 自动重置,不会因一次加载失败永久隐藏。
function FlagImg({ code }: { code: string }) {
  const [ok, setOk] = useState(true)
  if (!ok) return null
  return (
    <img
      src={`/flags/${code}.svg`}
      alt=""
      aria-hidden
      className="inline-block h-3 w-4 shrink-0 rounded-[2px] object-cover"
      onError={() => setOk(false)}
    />
  )
}
