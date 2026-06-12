import { useEffect, useState, type ReactNode } from 'react'
import { BrowserRouter, Navigate, Outlet, Route, Routes, NavLink, useLocation } from 'react-router-dom'
import Login from './pages/Login'
import Dashboard from './pages/Dashboard'
import Nodes from './pages/Nodes'
import Rules from './pages/Rules'
import Users from './pages/Users'
import BandwidthProfiles from './pages/BandwidthProfiles'
import Settings from './pages/Settings'
import RuleDetail from './pages/RuleDetail'
import NodeDetail from './pages/NodeDetail'
import Tunnels from './pages/Tunnels'
import TunnelDetail from './pages/TunnelDetail'
import { AuthProvider } from './lib/auth'
import { useAuth } from './lib/use-auth'
import { ToastProvider } from './lib/toast'
import { Backdrop, ForbiddenCard } from './lib/ui'
import { useThemeSync } from './lib/use-theme'

export default function App() {
  // 全局强调色:启动拉取 + 30s 轮询,管理员改色后所有客户端(含登录页)自动跟进。
  useThemeSync()
  return (
    <ToastProvider>
      <AuthProvider>
        <BrowserRouter>
          <Routes>
            <Route path="/login" element={<Login />} />
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
        </BrowserRouter>
      </AuthProvider>
    </ToastProvider>
  )
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
  const { user, loading, logout } = useAuth()
  const [drawerOpen, setDrawerOpen] = useState(false)
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

  return (
    <div className="min-h-svh text-zinc-100 flex gap-5 p-3 md:p-5 relative">
      <Backdrop />
      {/* Sidebar:大屏为悬浮玻璃板;小屏隐藏(translate-x),由汉堡触发覆盖 drawer。 */}
      <aside
        className={`fixed inset-y-0 left-0 z-30 w-56 shrink-0 p-4 flex flex-col transition-transform duration-300 glass-card rounded-none md:rounded-3xl md:translate-x-0 md:sticky md:top-5 md:inset-y-auto md:h-[calc(100svh-2.5rem)] md:self-start ${
          drawerOpen ? 'translate-x-0' : '-translate-x-full'
        }`}
      >
        <div className="px-2 py-1 mb-6 flex items-center justify-between">
          <div>
            <div className="text-sm font-bold tracking-[0.14em] bg-gradient-to-r from-white via-accent-hi to-white bg-clip-text text-transparent">
              EMORELAY
            </div>
            <div className="text-[11px] text-zinc-500 mt-0.5">流量转发面板</div>
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
        <div className="mt-auto pt-4 border-t border-white/5 text-[11px] text-zinc-500 md:hidden">
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
            ☰
          </button>
          <CurrentRoute />
          <div className="ml-auto flex items-center gap-3 text-[12px]">
            <span className="hidden sm:inline text-zinc-400 truncate max-w-[12rem]">
              {user.username}{' '}
              <span className="ml-1 text-[10px] uppercase text-zinc-500">{user.role}</span>
            </span>
            <button
              onClick={logout}
              className="rounded-md bg-white/5 hover:bg-white/10 ring-1 ring-inset ring-white/10 px-2.5 py-1 text-xs transition-colors"
            >
              登出
            </button>
          </div>
        </header>

        <main className="flex-1 min-w-0 py-6 md:py-8 overflow-auto">
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
          {hint && <span className="text-[10px] uppercase text-zinc-600">{hint}</span>}
        </>
      )}
    </NavLink>
  )
}
