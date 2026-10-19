import { useEffect, useState } from 'react'
import { NavLink, Route, Routes, useLocation } from 'react-router'
import { Activity, AppWindow, Command, House, LayoutDashboard, Menu, Monitor, Settings2, SlidersHorizontal, Grid2X2, CircleHelp, RefreshCcw, MousePointer2 } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { useDaemon } from '../api/hooks'
import { useUI } from './store'
import { Dashboard } from '../pages/Dashboard'
import { Workspaces } from '../pages/Workspaces'
import { Hotkeys } from '../pages/Hotkeys'
import { Layouts } from '../pages/Layouts'
import { Displays } from '../pages/Displays'
import { WindowRules } from '../pages/WindowRules'
import { Diagnostics } from '../pages/Diagnostics'
import { General } from '../pages/General'
import { FocusMouse } from '../pages/FocusMouse'

const navigation: { path: string; title: string; icon: LucideIcon }[] = [
  { path: '/', title: 'Dashboard', icon: House },
  { path: '/workspaces', title: 'Workspaces', icon: Grid2X2 },
  { path: '/displays', title: 'Displays', icon: Monitor },
  { path: '/layouts', title: 'Layouts', icon: LayoutDashboard },
  { path: '/hotkeys', title: 'Hotkeys', icon: Command },
  { path: '/rules', title: 'Window Rules', icon: AppWindow },
  { path: '/focus', title: 'Focus & Mouse', icon: MousePointer2 },
  { path: '/diagnostics', title: 'Diagnostics', icon: Activity },
  { path: '/general', title: 'General Settings', icon: Settings2 },
]

export function App() {
  const location = useLocation()
  const status = useDaemon('status', true, true)
  const theme = useUI((state) => state.theme)
  const [menuOpen, setMenuOpen] = useState(false)
  const title = navigation.find((item) => item.path === location.pathname)?.title ?? 'Tylora'

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = () => { document.documentElement.dataset.theme = theme === 'system' ? (media.matches ? 'dark' : 'light') : theme }
    apply(); media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [theme])

  return <div className="app-shell">
    <aside className="sidebar">
      <div className="brand"><div className="brand-mark"><span /><span /><span /></div><div><strong>Tylora</strong><small>CONTROL CENTER</small></div></div>
      <div className="sidebar-section">WORKSPACE</div>
      <nav aria-label="Main navigation">{navigation.map(({ path, title: name, icon: Icon }) =>
        <NavLink key={path} to={path} end={path === '/'} className={({ isActive }) => `nav-link ${isActive ? 'active' : ''}`}>
          <Icon size={17} strokeWidth={1.9} /><span>{name}</span>
        </NavLink>)}</nav>
      <div className="sidebar-bottom"><div className="sidebar-divider" /><div className="sidebar-foot"><span className={status.isSuccess ? 'live-dot' : 'off-dot'} />{status.isSuccess ? 'Daemon connected' : 'Daemon offline'}</div><small>Tylora 0.1 · macOS</small></div>
    </aside>
    <div className="main-shell">
      <header className="topbar"><div className="breadcrumb"><SlidersHorizontal size={15} /><span>Tylora</span><span className="slash">/</span><strong>{title}</strong></div>
        <div className="top-actions"><div className={`connection ${status.isSuccess ? 'connected' : 'disconnected'}`}><span />{status.isSuccess ? 'Connected' : 'Disconnected'}</div>
          <button className="icon-button" aria-label="More actions" onClick={() => setMenuOpen(!menuOpen)}><Menu size={18} /></button>
          {menuOpen && <div className="top-menu"><button onClick={() => { void status.refetch(); setMenuOpen(false) }}><RefreshCcw size={15} /> Refresh status</button><NavLink to="/diagnostics" onClick={() => setMenuOpen(false)}><CircleHelp size={15} /> Diagnostics</NavLink></div>}
        </div>
      </header>
      <main className="page"><Routes>
        <Route path="/" element={<Dashboard />} />
        <Route path="/workspaces" element={<Workspaces />} />
        <Route path="/displays" element={<Displays />} />
        <Route path="/layouts" element={<Layouts />} />
        <Route path="/hotkeys" element={<Hotkeys />} />
        <Route path="/rules" element={<WindowRules />} />
        <Route path="/focus" element={<FocusMouse />} />
        <Route path="/diagnostics" element={<Diagnostics />} />
        <Route path="/general" element={<General />} />
      </Routes></main>
    </div>
  </div>
}
