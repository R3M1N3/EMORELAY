import { useState } from 'react'
import { ApiError, type DiagnoseResponse, type SegmentResult } from '../lib/api'
import { useToast } from '../lib/use-toast'

// 逐段诊断面板:点击触发 → 对链路每段下发探测 → 渲染可达性/延迟/丢失。
// run 由调用方注入(rules.diagnose / tunnels.diagnose),组件不关心是规则还是隧道。
export function DiagnosePanel({ run }: { run: () => Promise<DiagnoseResponse> }) {
  const toast = useToast()
  const [loading, setLoading] = useState(false)
  const [result, setResult] = useState<DiagnoseResponse | null>(null)

  async function onRun() {
    setLoading(true)
    try {
      setResult(await run())
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : '诊断失败')
    } finally {
      setLoading(false)
    }
  }

  return (
    <section className="glass-card rise p-5">
      <div className="flex items-center justify-between gap-3 mb-3">
        <h3 className="text-sm font-medium text-zinc-200">链路诊断</h3>
        <button
          type="button"
          onClick={onRun}
          disabled={loading}
          className="rounded-lg bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-3 py-1.5 text-xs disabled:opacity-60"
        >
          {loading ? '探测中…' : '开始诊断'}
        </button>
      </div>
      {result == null ? (
        <p className="text-[12px] text-zinc-400">
          逐段探测链路每一跳的 TCP 可达性、延迟与丢失，定位哪一段断了。
        </p>
      ) : result.segments.length === 0 ? (
        <p className="text-[12px] text-zinc-400">无可探测的链路段。</p>
      ) : (
        <div className="space-y-2">
          {result.segments.map((s, i) => (
            <SegmentRow key={i} seg={s} />
          ))}
        </div>
      )}
    </section>
  )
}

function quality(s: SegmentResult): { dot: string; text: string } {
  if (!s.dispatched) return { dot: 'bg-zinc-500', text: '节点离线' }
  if (!s.reachable) return { dot: 'bg-red-400', text: '不可达' }
  if (s.loss_pct > 0) return { dot: 'bg-amber-400', text: `部分丢失 ${s.loss_pct.toFixed(0)}%` }
  return { dot: 'bg-emerald-400', text: '通畅' }
}

function SegmentRow({ seg }: { seg: SegmentResult }) {
  const q = quality(seg)
  return (
    <div className="rounded-lg border border-white/5 bg-white/[0.03] px-3 py-2">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2 min-w-0">
          <span className={`inline-block h-2 w-2 rounded-full shrink-0 ${q.dot}`} aria-hidden />
          <span className="text-sm text-zinc-200 truncate">{seg.label}</span>
        </div>
        <span className="text-[11px] text-zinc-400 shrink-0">{q.text}</span>
      </div>
      <div className="mt-1 flex flex-wrap gap-x-4 gap-y-0.5 text-[11px] text-zinc-400">
        <span className="font-mono">{seg.source_node_name} → {seg.target}</span>
        {seg.dispatched && seg.reachable && (
          <>
            <span>延迟 {seg.avg_latency_ms.toFixed(1)} ms</span>
            <span>丢失 {seg.loss_pct.toFixed(0)}%</span>
          </>
        )}
        {seg.error && <span className="text-red-300/80">{seg.error}</span>}
      </div>
    </div>
  )
}
