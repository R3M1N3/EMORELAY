import { useState, type ReactNode } from 'react'
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
import { ForbiddenCard } from './lib/ui'

export default function App() {
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
  if (loading)
    return (
      <div className="min-h-svh grid place-items-center bg-zinc-950 text-zinc-400 text-sm">
        加载会话…
      </div>
    )
  if (!user) return <Navigate to="/login" replace />

  return (
    <div className="min-h-svh bg-zinc-950 text-zinc-100 flex">
      {/* Sidebar:大屏常驻;小屏隐藏(translate-x),由汉堡触发覆盖 drawer。 */}
      <aside
        className={`fixed inset-y-0 left-0 z-30 w-56 shrink-0 border-r border-white/5 bg-zinc-950/95 backdrop-blur p-4 flex flex-col transition-transform md:static md:translate-x-0 md:bg-zinc-950/80 ${
          drawerOpen ? 'translate-x-0' : '-translate-x-full'
        }`}
      >
        <div className="px-2 py-1 mb-6 flex items-center justify-between">
          <div>
            <div className="text-sm font-semibold tracking-tight">EMORELAY</div>
            <div className="text-[11px] text-zinc-500">流量转发面板</div>
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
        <div className="mt-auto text-[11px] text-zinc-500 md:hidden">
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

      <div className="flex-1 min-w-0 flex flex-col">
        {/* 顶部状态栏:小屏左边汉堡;右边当前用户 + 登出。 */}
        <header className="sticky top-0 z-10 flex items-center gap-3 border-b border-white/5 bg-zinc-950/80 backdrop-blur px-4 py-2 md:px-8">
          <button
            onClick={() => setDrawerOpen(true)}
            aria-label="打开导航"
            className="md:hidden rounded-md bg-zinc-800/60 hover:bg-zinc-800 px-2 py-1 text-zinc-200"
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
              className="rounded-md bg-zinc-800/70 hover:bg-zinc-800 px-2.5 py-1 text-xs"
            >
              登出
            </button>
          </div>
        </header>

        <main className="flex-1 min-w-0 p-6 md:p-8 overflow-auto">
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
        `flex items-center justify-between rounded-md px-2 py-1.5 ${
          isActive ? 'bg-zinc-800/80 text-white' : 'text-zinc-400 hover:bg-zinc-800/40 hover:text-zinc-200'
        }`
      }
    >
      <span>{label}</span>
      {hint && <span className="text-[10px] uppercase text-zinc-600">{hint}</span>}
    </NavLink>
  )
}
