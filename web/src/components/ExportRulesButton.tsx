import { useState } from 'react'
import { ApiError, rules } from '../lib/api'
import { useToast } from '../lib/use-toast'

// P9: 导出规则按钮(节点/隧道详情页共用)。自持 exporting 态 + 统一禁用样式 + 错误兜底,
// 避免「进行中态 / 错误 toast」整段逻辑在各详情页 copy-paste(原 NodeDetail / TunnelDetail 各一份)。
// 导出走一次 fetch+blob,规则量大/网络慢时有可感知延迟,故需进行中态 + disabled 防连点。
export function ExportRulesButton({
  query,
  successText,
}: {
  /** 导出范围:{ node_id } 或 { tunnel_id }。 */
  query: { node_id?: number; tunnel_id?: number }
  /** 导出成功的 toast 文案(节点/隧道语义不同)。 */
  successText: string
}) {
  const toast = useToast()
  const [exporting, setExporting] = useState(false)
  return (
    <button
      type="button"
      disabled={exporting}
      onClick={async () => {
        setExporting(true)
        try {
          await rules.exportDownload(query)
          toast.success(successText)
        } catch (e) {
          toast.error(e instanceof ApiError ? e.message : '导出失败')
        } finally {
          setExporting(false)
        }
      }}
      className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 disabled:opacity-60 disabled:cursor-not-allowed px-3 py-2 text-sm"
    >
      {exporting ? '导出中…' : '导出规则'}
    </button>
  )
}
