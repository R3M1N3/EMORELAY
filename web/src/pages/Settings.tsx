import { useEffect, useState, type FormEvent } from 'react'
import {
  ApiError,
  shortTime,
  system,
  type AuditLogEntry,
  type SecurityInfo,
} from '../lib/api'
import { ErrorBox, PageLoading, fieldInputCls, fieldLabelCls } from '../lib/ui'
import { useToast } from '../lib/use-toast'
import { applyAccent } from '../lib/use-theme'

interface SettingsFormState {
  reserved_ports: string
  stats_retention_days: string
  agent_control_endpoint: string
  notify_webhook_url: string
  ui_accent_color: string
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
  stats_retention_days: '',
  agent_control_endpoint: '',
  notify_webhook_url: '',
  ui_accent_color: '',
}

export default function Settings() {
  const toast = useToast()
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
            stats_retention_days: initial.stats_retention_days ?? '',
            agent_control_endpoint: initial.agent_control_endpoint ?? '',
            // 未配置时 key 不存在于 settings 表,必须 ?? '' 兜底。
            notify_webhook_url: initial.notify_webhook_url ?? '',
            ui_accent_color: initial.ui_accent_color ?? '',
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
        'stats_retention_days',
        'agent_control_endpoint',
        'notify_webhook_url',
        'ui_accent_color',
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
          stats_retention_days: resp.settings.stats_retention_days ?? '',
          agent_control_endpoint: resp.settings.agent_control_endpoint ?? '',
          notify_webhook_url: resp.settings.notify_webhook_url ?? '',
          ui_accent_color: resp.settings.ui_accent_color ?? '',
        },
        savedAt: new Date().toISOString().replace('T', ' ').slice(0, 19),
      }))
      // 本端立即生效,不等 30s 轮询;其余客户端由轮询跟进。
      applyAccent(resp.settings.ui_accent_color ?? null)
      toast.success('设置已保存')
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '保存失败'
      setState((p) => ({ ...p, saving: false, saveError: msg }))
      toast.error(msg)
    }
  }

  function set<K extends keyof SettingsFormState>(k: K, v: SettingsFormState[K]) {
    setState((p) => ({ ...p, form: { ...p.form, [k]: v } }))
  }

  if (state.loading) return <PageLoading />
  if (state.loadError) return <ErrorBox message={state.loadError} />

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-xl font-semibold tracking-tight">系统设置</h2>
        <p className="text-sm text-zinc-400 mt-1">安全状态 / 保留端口黑名单 / 全局默认配置</p>
      </div>

      <SecurityCard data={security} />

      <form onSubmit={onSubmit} className="glass-card rise p-5 space-y-4 max-w-2xl">
        <div>
          <label htmlFor="set-endpoint" className={fieldLabelCls}>Agent 上报端点</label>
          <input
            id="set-endpoint"
            type="text"
            value={state.form.agent_control_endpoint}
            onChange={(e) => set('agent_control_endpoint', e.target.value)}
            className={fieldInputCls}
            placeholder="https://relay.example.com:50051"
          />
          <p className="text-[11px] text-zinc-400 mt-1">
            Agent 默认 gRPC 连入地址。新建节点的「安装命令」会嵌入这个值；
            生产建议用 https。留空表示未配置（节点详情页的安装命令按钮会禁用）。
            <span className="text-amber-400/90">
              必须是 Agent 能直连本机的域名/IP——不能填 CDN/Cloudflare 橙云代理域名
              （CDN 不转发 50051），且需与安装时的 PANEL_PUBLIC_HOST 一致，否则证书校验失败。
            </span>
          </p>
        </div>

        <div>
          <label htmlFor="set-reserved" className={fieldLabelCls}>保留端口 (reserved_ports)</label>
          <textarea
            id="set-reserved"
            value={state.form.reserved_ports}
            onChange={(e) => set('reserved_ports', e.target.value)}
            rows={2}
            className={`${fieldInputCls} font-mono text-xs`}
            placeholder="[22, 80, 443, 3306, 5432]"
          />
          <p className="text-[11px] text-zinc-400 mt-1">
            JSON 整数数组。任何规则的 listen_port 命中将被拒绝创建。
          </p>
        </div>

        <div>
          <label htmlFor="set-retention" className={fieldLabelCls}>统计保留天数 (stats_retention_days)</label>
          <input
            id="set-retention"
            type="number"
            min={1}
            value={state.form.stats_retention_days}
            onChange={(e) => set('stats_retention_days', e.target.value)}
            className={fieldInputCls}
            placeholder="30"
          />
          <p className="text-[11px] text-zinc-400 mt-1">
            node_stats / rule_stats 分钟桶保留天数（默认 30），超期数据由后台每小时自动清理；
            不清理审计日志。
            <span className="text-amber-400/90">
              注意：设为小于 30 天会蚕食「30 天滚动流量配额」的计算窗口，导致用户用量被低估。
            </span>
          </p>
        </div>

        <div>
          <label htmlFor="set-webhook" className={fieldLabelCls}>通知 Webhook URL (notify_webhook_url)</label>
          <input
            id="set-webhook"
            type="text"
            value={state.form.notify_webhook_url}
            onChange={(e) => set('notify_webhook_url', e.target.value)}
            className={fieldInputCls}
            placeholder="https://example.com/hook（留空 = 关闭通知）"
          />
          <p className="text-[11px] text-zinc-400 mt-1">
            节点掉线/恢复、用户超额/到期时 POST JSON{' '}
            <code className="text-zinc-400">{'{event, occurred_at, data}'}</code> 到此地址。
            事件：node.offline / node.online / user.quota_exceeded / user.expired。
            https 端点需公网受信证书；内网接收器可用 http。发送失败重试 1 次后丢弃。
          </p>
        </div>

        <div>
          <label htmlFor="set-accent" className={fieldLabelCls}>全局强调色 (ui_accent_color)</label>
          <div className="flex items-center gap-3">
            <input
              type="color"
              value={/^#[0-9a-fA-F]{6}$/.test(state.form.ui_accent_color) ? state.form.ui_accent_color : '#67e8f9'}
              onChange={(e) => set('ui_accent_color', e.target.value)}
              aria-label="选择强调色"
              className="h-9 w-12 cursor-pointer rounded-lg border border-white/10 bg-white/[0.04] p-1"
            />
            <input
              id="set-accent"
              type="text"
              value={state.form.ui_accent_color}
              onChange={(e) => set('ui_accent_color', e.target.value.trim())}
              className={`${fieldInputCls} font-mono max-w-[10rem]`}
              placeholder="#67e8f9（留空 = 默认）"
            />
            {state.form.ui_accent_color !== '' && (
              <button
                type="button"
                onClick={() => set('ui_accent_color', '')}
                className="text-xs text-zinc-400 hover:text-zinc-200 underline underline-offset-2"
              >
                恢复默认
              </button>
            )}
          </div>
          <p className="text-[11px] text-zinc-400 mt-1">
            #rrggbb 格式。保存后全站配色（按钮/导航/背景极光）随之联动，
            所有已登录客户端最迟 30 秒内自动跟进，无需刷新。
            建议选用较亮的颜色，深色会降低暗底上的文字对比度。
          </p>
        </div>

        {state.saveError && (
          <div role="alert" className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200">
            {state.saveError}
          </div>
        )}
        {state.savedAt && !state.saveError && (
          <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-200">
            已保存于 {state.savedAt}
          </div>
        )}

        <div className="flex justify-end">
          <button type="submit" disabled={state.saving} className="btn-accent">
            {state.saving ? '保存中…' : '保存设置'}
          </button>
        </div>
      </form>

      <section className="glass-card rise overflow-hidden">
        <div className="px-5 py-3 border-b border-white/5">
          <h3 className="text-sm font-medium text-zinc-200">最近审计日志</h3>
          <p className="text-[11px] text-zinc-400">最近 50 条操作记录</p>
        </div>
        {logs.loading ? (
          <div className="p-5 text-sm text-zinc-400">加载中…</div>
        ) : logs.error ? (
          <div className="p-5 text-sm text-red-300">{logs.error}</div>
        ) : logs.items.length === 0 ? (
          <div className="p-5 text-sm text-zinc-400">暂无记录。</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-[11px] uppercase text-zinc-400 bg-zinc-900/80">
                <tr>
                  <th scope="col" className="px-4 py-2 text-left font-medium">时间</th>
                  <th scope="col" className="px-4 py-2 text-left font-medium">操作</th>
                  <th scope="col" className="px-4 py-2 text-left font-medium">对象</th>
                  <th scope="col" className="px-4 py-2 text-left font-medium">结果</th>
                  <th scope="col" className="px-4 py-2 text-left font-medium">详情</th>
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
                    <td className="px-4 py-2 align-top text-[11px] text-zinc-400 max-w-[18rem] truncate">
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
      <section className="glass-card p-5 text-sm text-zinc-400">
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
        hint: '内置 CA 强制双向证书认证 + 加密传输;Agent 须携带「安装命令四件套」里签发的 client cert,否则握手失败导致离线',
      }
    : data.grpc_tls_enabled
    ? {
        text: 'Token + TLS',
        cls: 'text-emerald-300',
        hint: 'TLS 加密传输,token 不裸跑(内置 CA 模式下一般直接强制 mTLS)',
      }
    : {
        text: 'Token (明文)',
        cls: 'text-amber-300',
        hint: '开发模式(PANEL_DEV_DISABLE_MTLS=1);生产移除该 env 即默认启用内置 CA mTLS',
      }
  return (
    <section className="glass-card rise p-5">
      <h3 className="text-sm font-medium text-zinc-200 mb-3">安全状态</h3>
      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        <div className="rounded-lg border border-white/5 bg-white/[0.03] px-3 py-2">
          <div className="text-[11px] text-zinc-400">JWT 密钥</div>
          <div className={`text-sm mt-0.5 ${jwtStatus.cls}`}>{jwtStatus.text}</div>
          <div className="text-[11px] text-zinc-400 mt-0.5">
            长度 {data.jwt_secret_length} 字节 · 过期 {data.jwt_expiry_hours} 小时
          </div>
        </div>
        <div className="rounded-lg border border-white/5 bg-white/[0.03] px-3 py-2">
          <div className="text-[11px] text-zinc-400">Agent 鉴权方式</div>
          <div className={`text-sm mt-0.5 ${tlsStatus.cls}`}>{tlsStatus.text}</div>
          <div className="text-[11px] text-zinc-400 mt-0.5">{tlsStatus.hint}</div>
        </div>
      </div>
    </section>
  )
}
