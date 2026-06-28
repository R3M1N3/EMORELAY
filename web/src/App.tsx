import { lazy, Suspense, useEffect, useRef, useState, type ReactNode } from 'react'
import { BrowserRouter, Navigate, Outlet, Route, Routes, NavLink, useLocation } from 'react-router-dom'
import Login from './pages/Login'
import Dashboard from './pages/Dashboard'
import { AuthProvider } from './lib/auth'
import { useAuth } from './lib/use-auth'
import { ToastProvider } from './lib/toast'
import { Backdrop, ForbiddenCard } from './lib/ui'
import { useThemeSync } from './lib/use-theme'

// 路由级懒加载:首屏只下载 Login + Dashboard(未登录/登录后的两个落地页),其余按需切 chunk——
// 尤其把 Rules/Users 两个巨型表单与 5 个 admin-only 页移出首屏,普通用户不再下载进不去的管理页。
const ChangePassword = lazy(() => import('./pages/ChangePassword'))
const Nodes = lazy(() => import('./pages/Nodes'))
const Rules = lazy(() => import('./pages/Rules'))
const Users = lazy(() => import('./pages/Users'))
const BandwidthProfiles = lazy(() => import('./pages/BandwidthProfiles'))
const Settings = lazy(() => import('./pages/Settings'))
const RuleDetail = lazy(() => import('./pages/RuleDetail'))
const NodeDetail = lazy(() => import('./pages/NodeDetail'))
const Tunnels = lazy(() => import('./pages/Tunnels'))
const TunnelDetail = lazy(() => import('./pages/TunnelDetail'))

export default function App() {
  // 全局强调色:启动拉取 + 30s 轮询,管理员改色后所有客户端(含登录页)自动跟进。
  useThemeSync()
  return (
    <ToastProvider>
      <AuthProvider>
        <BrowserRouter>
          <Suspense fallback={<RouteFallback />}>
            <Routes>
              <Route path="/login" element={<Login />} />
              <Route path="/change-password" element={<ForcePasswordChangeRoute />} />
              <Route path="/" element={<ProtectedShell />}>
                <Route index element={<Dashboard />} />
                <Route path="nodes" element={<AdminRoute><Nodes /></AdminRoute>} />
                <Route path="nodes/:id" element={<AdminRoute><NodeDetail /></AdminRoute>} />
                <Route path="rules" element={<Rules />} />
                <Route path="rules/:id" element={<RuleDetail />} />
                <Route path="tunnels" element={<AdminRoute><Tunnels /></AdminRoute>} />
                <Route path="tunnels/:id" element={<AdminRoute><TunnelDetail /></AdminRoute>} />
                <Route path="users" element={<AdminRoute><Users /></AdminRoute>} />
                <Route path="bandwidth-profiles" element={<AdminRoute><BandwidthProfiles /></AdminRoute>} />
                <Route path="settings" element={<AdminRoute><Settings /></AdminRoute>} />
              </Route>
              <Route path="*" element={<Navigate to="/" replace />} />
            </Routes>
          </Suspense>
        </BrowserRouter>
      </AuthProvider>
    </ToastProvider>
  )
}

// 懒加载路由切换时的占位:落在受保护页内容区或全屏改密页,中性居中提示,避免白屏。
function RouteFallback() {
  return <div className="grid place-items-center py-24 text-zinc-400 text-sm">加载中…</div>
}

// 强制改密路由:仅当已登录且 mustChangePassword 时展示改密页;
// 否则按状态回落(未登录→/login,已改→/),避免该 URL 被任意访问。
function ForcePasswordChangeRoute() {
  const { user, loading, mustChangePassword } = useAuth()
  if (loading)
    return (
      <div className="min-h-svh grid place-items-center bg-zinc-950 text-zinc-400 text-sm">
        加载会话…
      </div>
    )
  if (!user) return <Navigate to="/login" replace />
  if (!mustChangePassword) return <Navigate to="/" replace />
  return <ChangePassword />
}

// admin-only 路由兜底:导航虽已按角色隐藏,直接输 URL 也不能看到裸错误。
function AdminRoute({ children }: { children: ReactNode }) {
  const { user } = useAuth()
  if (user && user.role !== 'admin') return <ForbiddenCard />
  return children
}

// 导航项:普通用户只保留自助可用页(概览/规则),其余 admin-only。
const NAV: { to: string; label: string; adminOnly?: boolean }[] = [
  { to: '/', label: '概览' },
  { to: '/nodes', label: '节点', adminOnly: true },
  { to: '/rules', label: '规则' },
  { to: '/tunnels', label: '隧道', adminOnly: true },
  { to: '/users', label: '用户', adminOnly: true },
  { to: '/bandwidth-profiles', label: '限速', adminOnly: true },
  { to: '/settings', label: '设置', adminOnly: true },
]

function ProtectedShell() {
  const { user, loading, logout, mustChangePassword } = useAuth()
  const [drawerOpen, setDrawerOpen] = useState(false)
  const mainRef = useRef<HTMLElement>(null)
  const loc = useLocation()
  // 路由切换后内容区滚回顶部(移动端尤其重要,否则停在上一页的滚动位置)。
  useEffect(() => {
    mainRef.current?.scrollTo(0, 0)
  }, [loc.pathname])
  // 移动端 drawer 支持 Escape 关闭,与弹窗行为一致。
  useEffect(() => {
    if (!drawerOpen) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setDrawerOpen(false)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [drawerOpen])
  if (loading)
    return (
      <div className="min-h-svh grid place-items-center bg-zinc-950 text-zinc-400 text-sm">
        加载会话…
      </div>
    )
  if (!user) return <Navigate to="/login" replace />
  // 强制改密未完成:挡在改密页,所有受保护页都进不去。
  if (mustChangePassword) return <Navigate to="/change-password" replace />

  return (
    <div className="min-h-svh text-zinc-100 flex gap-5 p-3 md:p-5 relative">
      <Backdrop />
      {/* Sidebar:大屏为悬浮玻璃板;小屏隐藏(translate-x),由汉堡触发覆盖 drawer。 */}
      <aside
        className={`fixed inset-y-0 left-0 z-30 w-56 shrink-0 p-4 pt-[max(1rem,env(safe-area-inset-top))] flex flex-col transition-transform duration-300 glass-card rounded-none md:rounded-3xl md:translate-x-0 md:sticky md:top-5 md:inset-y-auto md:h-[calc(100svh-2.5rem)] md:self-start ${
          drawerOpen ? 'translate-x-0' : '-translate-x-full'
        }`}
      >
        <div className="px-2 py-1 mb-6 flex items-center justify-between">
          <div>
            <div className="text-sm font-bold tracking-[0.14em] bg-gradient-to-r from-white via-accent-hi to-white bg-clip-text text-transparent">
              EMORELAY
            </div>
            <div className="text-xs text-zinc-400 mt-0.5">流量转发面板</div>
          </div>
          <button
            onClick={() => setDrawerOpen(false)}
            aria-label="关闭导航"
            className="md:hidden text-zinc-400 hover:text-white text-lg leading-none"
          >
            ×
          </button>
        </div>
        <nav className="space-y-1 text-sm">
          {NAV.filter((n) => !n.adminOnly || user.role === 'admin').map((n) => (
            <NavItem key={n.to} to={n.to} label={n.label} onClick={() => setDrawerOpen(false)} />
          ))}
        </nav>
        <div className="mt-auto pt-4 border-t border-white/5 text-xs text-zinc-400 md:hidden">
          <div className="truncate">{user.username} · {user.role}</div>
        </div>
      </aside>

      {/* 小屏遮罩,点击关闭 drawer。 */}
      {drawerOpen && (
        <div
          className="fixed inset-0 z-20 bg-black/50 backdrop-blur-sm md:hidden"
          aria-hidden
          onClick={() => setDrawerOpen(false)}
        />
      )}

      <div className="flex-1 min-w-0 flex flex-col relative">
        {/* 顶部状态栏:小屏左边汉堡;右边当前用户 + 登出。 */}
        <header className="sticky top-3 md:top-5 z-10 glass-card flex items-center gap-3 px-4 py-2.5 md:px-6">
          <button
            onClick={() => setDrawerOpen(true)}
            aria-label="打开导航"
            className="md:hidden rounded-md bg-white/5 hover:bg-white/10 px-2 py-1 text-zinc-200"
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden>
              <path d="M3 6h18M3 12h18M3 18h18" />
            </svg>
          </button>
          <CurrentRoute />
          <div className="ml-auto flex items-center gap-3 text-xs">
            <span className="hidden sm:inline text-zinc-400 truncate max-w-[12rem]">
              {user.username}{' '}
              <span className="ml-1 text-[11px] uppercase text-zinc-400">{user.role}</span>
            </span>
            <button
              onClick={logout}
              className="rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-2.5 py-1 text-xs transition-colors"
            >
              登出
            </button>
          </div>
        </header>

        <main ref={mainRef} className="flex-1 min-w-0 py-6 md:py-8 overflow-auto">
          {/* 受保护壳层的页面级 h1(视觉隐藏):各页可见标题是 h2、分区是 h3,
              这里补一个 h1 使无障碍层级为 h1→h2→h3,读屏「按标题导航」有顶层锚点。 */}
          <h1 className="sr-only">EMORELAY 流量转发管理面板</h1>
          <Outlet />
        </main>
      </div>
    </div>
  )
}

function CurrentRoute() {
  // 顶栏左侧给当前路径一个轻提示,移动端点汉堡前用户能确认当前在哪一页。
  const loc = useLocation()
  const labels: Record<string, string> = {
    '/': '概览',
    '/nodes': '节点',
    '/rules': '规则',
    '/users': '用户',
    '/bandwidth-profiles': '限速',
    '/settings': '设置',
    '/tunnels': '隧道',
  }
  const base = '/' + (loc.pathname.split('/')[1] || '')
  const label = labels[base] ?? '详情'
  return <span className="text-sm font-medium text-zinc-200">{label}</span>
}

function NavItem({ to, label, hint, onClick }: { to: string; label: string; hint?: string; onClick?: () => void }) {
  return (
    <NavLink
      to={to}
      end={to === '/'}
      onClick={onClick}
      className={({ isActive }) =>
        `relative flex items-center justify-between rounded-xl px-3 py-2 transition-all duration-300 ${
          isActive
            ? 'text-white bg-gradient-to-r from-accent/15 to-accent/5 shadow-[inset_0_0_0_1px] shadow-accent/20'
            : 'text-zinc-400 hover:bg-white/5 hover:text-zinc-200 hover:translate-x-1'
        }`
      }
    >
      {({ isActive }) => (
        <>
          {isActive && (
            <span
              className="absolute -left-[5px] top-1/4 bottom-1/4 w-[3px] rounded bg-gradient-to-b from-accent-hi to-accent shadow-[0_0_10px] shadow-accent/70"
              aria-hidden
            />
          )}
          <span>{label}</span>
          {hint && <span className="text-[11px] uppercase text-zinc-400">{hint}</span>}
        </>
      )}
    </NavLink>
  )
}
