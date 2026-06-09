import { useEffect, useState, type FormEvent } from 'react'
import {
  ApiError,
  shortTime,
  system,
  type AuditLogEntry,
  type SecurityInfo,
} from '../lib/api'
import { fieldInputCls, fieldLabelCls } from '../lib/ui'

interface SettingsFormState {
  reserved_ports: string
  default_traffic_limit_bytes: string
  default_bandwidth_limit_mbps: string
  stats_retention_days: string
}

interface SettingsState {
  initial: Record<string, string>
  form: SettingsFormState
  loading: boolean
  saving: boolean
  loadError: string | null
  saveError: string | null
  savedAt: string | null
}

const EMPTY_FORM: SettingsFormState = {
  reserved_ports: '',
  default_traffic_limit_bytes: '',
  default_bandwidth_limit_mbps: '',
  stats_retention_days: '',
}

export default function Settings() {
  const [state, setState] = useState<SettingsState>({
    initial: {},
    form: EMPTY_FORM,
    loading: true,
    saving: false,
    loadError: null,
    saveError: null,
    savedAt: null,
  })
  const [logs, setLogs] = useState<{ items: AuditLogEntry[]; loading: boolean; error: string | null }>(
    { items: [], loading: true, error: null },
  )
  const [security, setSecurity] = useState<SecurityInfo | 'loading' | 'error'>('loading')

  useEffect(() => {
    let cancelled = false
    // SecurityInfo 单独拉,失败不阻塞设置表单/审计日志。
    system
      .security()
      .then((info) => {
        if (!cancelled) setSecurity(info)
      })
      .catch(() => {
        if (!cancelled) setSecurity('error')
      })
    Promise.all([system.getSettings(), system.auditLogs({ page_size: 50 })])
      .then(([s, l]) => {
        if (cancelled) return
        const initial = s.settings
        setState((p) => ({
          ...p,
          initial,
          form: {
            reserved_ports: initial.reserved_ports ?? '',
            default_traffic_limit_bytes: initial.default_traffic_limit_bytes ?? '',
            default_bandwidth_limit_mbps: initial.default_bandwidth_limit_mbps ?? '',
            stats_retention_days: initial.stats_retention_days ?? '',
          },
          loading: false,
        }))
        setLogs({ items: l.items, loading: false, error: null })
      })
      .catch((e: unknown) => {
        if (cancelled) return
        const msg = e instanceof ApiError ? e.message : '加载失败'
        setState((p) => ({ ...p, loading: false, loadError: msg }))
        setLogs({ items: [], loading: false, error: msg })
      })
    return () => {
      cancelled = true
    }
  }, [])

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setState((p) => ({ ...p, saving: true, saveError: null }))
    try {
      // 只发送实际变化的 key,避免无谓的 audit 噪声。
      const diff: Record<string, string> = {}
      const f = state.form
      const init = state.initial
      const keys: (keyof SettingsFormState)[] = [
        'reserved_ports',
        'default_traffic_limit_bytes',
        'default_bandwidth_limit_mbps',
        'stats_retention_days',
      ]
      for (const k of keys) {
        if (f[k] !== (init[k] ?? '')) diff[k] = f[k]
      }
      if (Object.keys(diff).length === 0) {
        setState((p) => ({
          ...p,
          saving: false,
          saveError: '没有需要保存的改动',
        }))
        return
      }
      const resp = await system.updateSettings(diff)
      setState((p) => ({
        ...p,
        saving: false,
        initial: resp.settings,
        form: {
          reserved_ports: resp.settings.reserved_ports ?? '',
          default_traffic_limit_bytes: resp.settings.default_traffic_limit_bytes ?? '',
          default_bandwidth_limit_mbps: resp.settings.default_bandwidth_limit_mbps ?? '',
          stats_retention_days: resp.settings.stats_retention_days ?? '',
        },
        savedAt: new Date().toISOString().replace('T', ' ').slice(0, 19),
      }))
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '保存失败'
      setState((p) => ({ ...p, saving: false, saveError: msg }))
    }
  }

  function set<K extends keyof SettingsFormState>(k: K, v: SettingsFormState[K]) {
    setState((p) => ({ ...p, form: { ...p.form, [k]: v } }))
  }

  if (state.loading) return <div className="text-zinc-400">加载中…</div>
  if (state.loadError)
    return (
      <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-200">
        {state.loadError}
      </div>
    )

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-xl font-semibold tracking-tight">系统设置</h2>
        <p className="text-sm text-zinc-400 mt-1">安全状态 / 保留端口黑名单 / 全局默认配置</p>
      </div>

      <SecurityCard data={security} />

      <form
        onSubmit={onSubmit}
        className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5 space-y-4 max-w-2xl"
      >
        <div>
          <label className={fieldLabelCls}>保留端口 (reserved_ports)</label>
          <textarea
            value={state.form.reserved_ports}
            onChange={(e) => set('reserved_ports', e.target.value)}
            rows={2}
            className={`${fieldInputCls} font-mono text-xs`}
            placeholder="[22, 80, 443, 3306, 5432]"
          />
          <p className="text-[11px] text-zinc-500 mt-1">
            JSON 整数数组。任何规则的 listen_port 命中将被拒绝创建。
          </p>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className={fieldLabelCls}>默认总流量 (bytes)</label>
            <input
              type="text"
              inputMode="numeric"
              value={state.form.default_traffic_limit_bytes}
              onChange={(e) => set('default_traffic_limit_bytes', e.target.value)}
              className={fieldInputCls}
              placeholder="留空 = 不限"
            />
          </div>
          <div>
            <label className={fieldLabelCls}>默认带宽 (Mbps)</label>
            <input
              type="text"
              inputMode="numeric"
              value={state.form.default_bandwidth_limit_mbps}
              onChange={(e) => set('default_bandwidth_limit_mbps', e.target.value)}
              className={fieldInputCls}
              placeholder="留空 = 不限"
            />
          </div>
        </div>

        <div>
          <label className={fieldLabelCls}>统计保留天数 (stats_retention_days)</label>
          <input
            type="number"
            min={1}
            value={state.form.stats_retention_days}
            onChange={(e) => set('stats_retention_days', e.target.value)}
            className={fieldInputCls}
            placeholder="30"
          />
          <p className="text-[11px] text-zinc-500 mt-1">
            node_stats / rule_stats 表保留天数。默认 30。后续可加清理任务。
          </p>
        </div>

        {state.saveError && (
          <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200">
            {state.saveError}
          </div>
        )}
        {state.savedAt && !state.saveError && (
          <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-200">
            已保存于 {state.savedAt}
          </div>
        )}

        <div className="flex justify-end">
          <button
            type="submit"
            disabled={state.saving}
            className="rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-4 py-2 text-sm font-medium"
          >
            {state.saving ? '保存中…' : '保存设置'}
          </button>
        </div>
      </form>

      <section className="rounded-2xl border border-white/10 bg-zinc-900/40 overflow-hidden">
        <div className="px-5 py-3 border-b border-white/5">
          <h3 className="text-sm font-medium text-zinc-200">最近审计日志</h3>
          <p className="text-[11px] text-zinc-500">最近 50 条操作记录</p>
        </div>
        {logs.loading ? (
          <div className="p-5 text-sm text-zinc-400">加载中…</div>
        ) : logs.error ? (
          <div className="p-5 text-sm text-red-300">{logs.error}</div>
        ) : logs.items.length === 0 ? (
          <div className="p-5 text-sm text-zinc-500">暂无记录。</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-[11px] uppercase text-zinc-500 bg-zinc-900/80">
                <tr>
                  <th className="px-4 py-2 text-left font-medium">时间</th>
                  <th className="px-4 py-2 text-left font-medium">操作</th>
                  <th className="px-4 py-2 text-left font-medium">对象</th>
                  <th className="px-4 py-2 text-left font-medium">结果</th>
                  <th className="px-4 py-2 text-left font-medium">详情</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-white/5">
                {logs.items.map((l) => (
                  <tr key={l.id} className="hover:bg-white/[0.02]">
                    <td className="px-4 py-2 align-top text-[12px] text-zinc-400 font-mono whitespace-nowrap">
                      {shortTime(l.created_at)}
                    </td>
                    <td className="px-4 py-2 align-top text-[12px] text-zinc-200 font-mono">
                      {l.action}
                    </td>
                    <td className="px-4 py-2 align-top text-[12px] text-zinc-400">
                      {l.target_type ?? '—'}
                      {l.target_id != null ? ` #${l.target_id}` : ''}
                    </td>
                    <td className="px-4 py-2 align-top">
                      <span
                        className={`inline-flex items-center rounded-md border px-1.5 py-0.5 text-[10px] ${
                          l.result === 'success'
                            ? 'border-emerald-500/40 bg-emerald-500/10 text-emerald-200'
                            : 'border-red-500/40 bg-red-500/10 text-red-200'
                        }`}
                      >
                        {l.result}
                      </span>
                    </td>
                    <td className="px-4 py-2 align-top text-[11px] text-zinc-500 max-w-[18rem] truncate">
                      {l.error_message ?? l.payload ?? ''}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  )
}

// SecurityCard 展示 JWT 配置状态 + Agent 鉴权通道,plan §6 第 7 项前两条。
// 不展示 secret 内容,仅展示长度供肉眼判断强度。
function SecurityCard({ data }: { data: SecurityInfo | 'loading' | 'error' }) {
  if (data === 'loading') {
    return (
      <section className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5 text-sm text-zinc-400">
        安全状态加载中…
      </section>
    )
  }
  if (data === 'error') {
    return (
      <section className="rounded-2xl border border-red-500/30 bg-red-500/10 p-5 text-sm text-red-200">
        安全状态加载失败
      </section>
    )
  }
  const jwtOk = data.jwt_secret_configured && data.jwt_secret_length >= 32
  const jwtStatus = !data.jwt_secret_configured
    ? { text: '未配置', cls: 'text-red-300' }
    : jwtOk
    ? { text: '已配置 (强度足够)', cls: 'text-emerald-300' }
    : { text: '已配置 (强度偏弱)', cls: 'text-amber-300' }
  const tlsStatus = data.grpc_mtls_enabled
    ? {
        text: 'Token + mTLS',
        cls: 'text-emerald-300',
        hint: '双向证书认证 + 加密传输;请同时确认 Agent 端配置 AGENT_GRPC_CLIENT_CERT/KEY,否则 TLS 握手会失败导致 Agent 离线',
      }
    : data.grpc_tls_enabled
    ? {
        text: 'Token + TLS',
        cls: 'text-emerald-300',
        hint: 'TLS 加密传输,token 不裸跑;配 PANEL_GRPC_TLS_CLIENT_CA 可升级 mTLS',
      }
    : {
        text: 'Token (明文)',
        cls: 'text-amber-300',
        hint: '生产建议配置 PANEL_GRPC_TLS_CERT/KEY 启用 TLS',
      }
  return (
    <section className="rounded-2xl border border-white/10 bg-zinc-900/40 p-5">
      <h3 className="text-sm font-medium text-zinc-200 mb-3">安全状态</h3>
      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        <div className="rounded-lg border border-white/5 bg-zinc-900/60 px-3 py-2">
          <div className="text-[11px] text-zinc-500">JWT 密钥</div>
          <div className={`text-sm mt-0.5 ${jwtStatus.cls}`}>{jwtStatus.text}</div>
          <div className="text-[11px] text-zinc-500 mt-0.5">
            长度 {data.jwt_secret_length} 字节 · 过期 {data.jwt_expiry_hours} 小时
          </div>
        </div>
        <div className="rounded-lg border border-white/5 bg-zinc-900/60 px-3 py-2">
          <div className="text-[11px] text-zinc-500">Agent 鉴权方式</div>
          <div className={`text-sm mt-0.5 ${tlsStatus.cls}`}>{tlsStatus.text}</div>
          <div className="text-[11px] text-zinc-500 mt-0.5">{tlsStatus.hint}</div>
        </div>
      </div>
    </section>
  )
}
