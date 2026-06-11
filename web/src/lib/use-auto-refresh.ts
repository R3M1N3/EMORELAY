import { useEffect, useRef } from 'react'

/**
 * 周期静默刷新:页面不可见(切后台/锁屏)时跳过该次 tick,回前台等下个周期。
 * cb 存 ref —— interval 不随 cb 重建,调用方无需 useCallback。
 * 刷新回调应当只重拉列表数据(silent,不置 loading 态),不得触碰表单 state。
 */
export function useAutoRefresh(cb: () => void, ms: number) {
  const ref = useRef(cb)
  // ref 同步放 effect 里(渲染期写 ref 违反 react-hooks/refs)。
  useEffect(() => {
    ref.current = cb
  })
  useEffect(() => {
    const t = setInterval(() => {
      if (!document.hidden) ref.current()
    }, ms)
    return () => clearInterval(t)
  }, [ms])
}
