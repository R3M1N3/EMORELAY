import { useCallback, useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import {
  auth,
  formatBytes,
  nodes,
  rules,
  shortTime,
  subscription,
  type MeView,
  type NodeView,
  type RuleView,
} from '../lib/api'
import { Stat } from './Dashboard'
import { ErrorBox, PageLoading, StatusDot } from '../lib/ui'
import { CopyButton } from '../components/CopyButton'
import { nodeEntryHost, ruleEntryDisplay } from '../lib/format-addr'
import { useAutoRefresh } from '../lib/use-auto-refresh'
import { useToast } from '../lib/use-toast'
import { expiryWarning, expiryWarningKey } from '../lib/expiry-warning'

// 普通用户自助概览:自己的规则/用量/配额/到期。数据源 = me(扩展) + rules.list(后端已按 owner 过滤)。
export default function UserDashboard() {
  const toast = useToast()
  const [me, setMe] = useState<MeView | null>(null)
  const [myRules, setMyRules] = useState<RuleView[]>([])
  // 入口地址需节点展示地址;拉(授权)节点建 id→node 映射,规则行据此显示真实入口而非裸端口。
  const [nodesById, setNodesById] = useState<Map<number, NodeView>>(new Map())
  const [error, setError] = useState<string | null>(null)
  // 订阅专用 token(scope=sub,仅查用量),进页面获取一次;失败则不展示链接(温和降级)。
  const [subToken, setSubToken] = useState<string | null>(null)

  const load = useCallback(() => {
    Promise.all([auth.me(), rules.list({ page_size: 100 }), nodes.list({ page_size: 100 })])
      .then(([m, r, n]) => {
        setMe(m)
        setMyRules(r.items)
        setNodesById(new Map(n.items.map((node) => [node.id, node])))
        setError(null)
      })
      .catch((e: unknown) =>
        // 静默刷新失败不打扰:已有数据则保留,仅首载落错误态。
        setMe((prev) => {
          if (prev == null) setError(e instanceof Error ? e.message : '加载失败')
          return prev
        }),
      )
  }, [])
  useEffect(() => {
    load()
  }, [load])
  useAutoRefresh(load, 30_000)

  // 订阅链接用的受限 token 进页面取一次即可(到账号到期才失效,无需随用量刷新)。
  useEffect(() => {
    subscription
      .issueToken()
      .then((r) => setSubToken(r.token))
      .catch(() => setSubToken(null))
  }, [])

  // 到期预警:me 拉到后据 expires_at 分级 toast,localStorage 按「级别+日期」去重,
  // 避免每次进页/30s 刷新重复轰炸。expired/critical 用 error,其余用 info。
  const expiresAt = me?.expires_at ?? null
  useEffect(() => {
    const warn = expiryWarning(expiresAt, Date.now())
    if (!warn) return
    const key = expiryWarningKey(warn.level, Date.now())
    if (localStorage.getItem(key)) return
    localStorage.setItem(key, '1')
    if (warn.level === 'expired' || warn.level === 'critical') toast.error(warn.message)
    else toast.info(warn.message)
  }, [expiresAt, toast])

  if (error)
    return <ErrorBox message={error} onRetry={() => { setError(null); load() }} />
  if (!me) return <PageLoading />

  const enabled = myRules.filter((r) => r.enabled).length
  const quota = me.traffic_limit_bytes_30d
  const used = me.period_used_bytes_cached
  const pct = quota ? Math.min(100, Math.round((used / quota) * 100)) : null

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-xl font-semibold tracking-tight">我的概览</h2>
        <p className="text-sm text-zinc-400 mt-1">规则 / 流量 / 配额</p>
      </div>

      <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
        <Stat
          label="我的规则"
          value={
            me.forward_rules_quota != null
              ? `${me.rule_count} / ${me.forward_rules_quota}`
              : me.rule_count
          }
          hint={me.forward_rules_quota != null ? `${enabled} 启用 · 含上限` : `${enabled} 启用`}
          accent="indigo"
        />
        <Stat label="累计流量" value={formatBytes(me.total_traffic_bytes)} hint="规则转发累计" accent="amber" />
        <Stat
          label="30 天用量"
          value={quota ? `${pct}%` : formatBytes(used)}
          hint={
            quota
              ? `${formatBytes(used)} / ${formatBytes(quota)} · 超额后规则自动停用`
              : '不限额'
          }
          accent="sky"
        />
        <Stat
          label="到期时间"
          value={me.expires_at ? shortTime(me.expires_at) : '永不'}
          hint={me.expires_at ? '到期后规则自动停用、登录被拒' : '账号长期有效'}
          accent="violet"
        />
      </div>

      {quota != null && pct != null && (
        <div className="glass-card rise p-5">
          <div className="flex items-center justify-between text-xs text-zinc-400 mb-2">
            <span>30 天滚动用量</span>
            <span>
              {formatBytes(used)} / {formatBytes(quota)}（{pct}%）
            </span>
          </div>
          <div
            className="h-2 rounded-full bg-zinc-800 overflow-hidden"
            role="progressbar"
            aria-valuenow={pct}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-label={`30 天用量 ${pct}%`}
          >
            <div
              className={`h-full rounded-full ${pct >= 90 ? 'bg-red-400' : pct >= 70 ? 'bg-amber-400' : 'bg-emerald-400'}`}
              style={{ width: `${pct}%` }}
            />
          </div>
        </div>
      )}

      <section className="glass-card rise p-5">
        <div className="flex items-center justify-between gap-3 mb-2">
          <h3 className="text-sm font-medium text-zinc-200">订阅用量链接</h3>
          {subToken && (
            <CopyButton
              value={`${window.location.origin}/api/subscription/usage?token=${subToken}`}
              label="复制订阅链接"
            />
          )}
        </div>
        {subToken ? (
          <>
            <p className="text-[12px] text-zinc-400 break-all font-mono">
              {`${window.location.origin}/api/subscription/usage?token=…`}
            </p>
            <p className="text-[11px] text-zinc-400 mt-1.5">
              在 Clash 等客户端里添加此链接，可直接查看套餐余量与到期。此链接为只读用量链接（仅能查看本人流量余额，无法操作其它功能），有效期到账号到期。
            </p>
          </>
        ) : (
          <p className="text-[12px] text-zinc-400">订阅链接获取失败，请刷新页面重试。</p>
        )}
      </section>

      <section className="glass-card rise p-5">
        <h3 className="text-sm font-medium text-zinc-200 mb-3">我的规则</h3>
        {myRules.length === 0 ? (
          <p className="text-sm text-zinc-400">
            尚无规则。前往
            <Link to="/rules" className="text-accent hover:text-accent-hi mx-1">
              规则页
            </Link>
            新建你的第一条转发。
          </p>
        ) : (
          <div className="space-y-2">
            {myRules.map((r) => (
              <div
                key={r.id}
                className="flex items-center justify-between rounded-lg border border-white/5 bg-white/[0.03] px-3 py-2 text-sm"
              >
                <div className="flex items-center gap-3 min-w-0">
                  <StatusDot kind={r.enabled ? 'on' : 'off'} />
                  <div className="min-w-0">
                    <Link
                      to={`/rules/${r.id}`}
                      className="font-medium truncate hover:text-accent"
                    >
                      {r.name}
                    </Link>
                    <div className="text-[11px] text-zinc-400 truncate">
                      {r.protocol.toUpperCase()} ·{' '}
                      {ruleEntryDisplay(nodeEntryHost(nodesById.get(r.node_id)), r.listen_port)}{' '}
                      → {r.target_host}:{r.target_port}
                    </div>
                  </div>
                </div>
                <div className="text-[11px] text-zinc-400 shrink-0">
                  ↓{formatBytes(r.rx_bytes)} ↑{formatBytes(r.tx_bytes)}
                </div>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  )
}
