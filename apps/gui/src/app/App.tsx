import { useEffect, useState } from 'react'
import { NavLink, Route, Routes, useLocation } from 'react-router'
import { Activity, AppWindow, Command, House, LayoutDashboard, Menu, Monitor, Settings2, SlidersHorizontal, Grid2X2, CircleHelp, RefreshCcw, MousePointer2, Layers, Languages } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { useDaemon } from '../api/hooks'
import { useLang, useT, type LanguagePref, type MessageKey } from '../i18n'
import { useUI } from './store'
import { Dashboard } from '../pages/Dashboard'
import { Workspaces } from '../pages/Workspaces'
import { Hotkeys } from '../pages/Hotkeys'
import { Layouts } from '../pages/Layouts'
import { Displays } from '../pages/Displays'
import { WindowRules } from '../pages/WindowRules'
import { Scratchpads } from '../pages/Scratchpads'
import { Diagnostics } from '../pages/Diagnostics'
import { General } from '../pages/General'
import { FocusMouse } from '../pages/FocusMouse'

const navigation: { path: string; title: MessageKey; icon: LucideIcon }[] = [
  { path: '/', title: 'nav.dashboard', icon: House },
  { path: '/workspaces', title: 'nav.workspaces', icon: Grid2X2 },
  { path: '/displays', title: 'nav.displays', icon: Monitor },
  { path: '/layouts', title: 'nav.layouts', icon: LayoutDashboard },
  { path: '/hotkeys', title: 'nav.hotkeys', icon: Command },
  { path: '/rules', title: 'nav.rules', icon: AppWindow },
  { path: '/scratchpads', title: 'nav.scratchpads', icon: Layers },
  { path: '/focus', title: 'nav.focus', icon: MousePointer2 },
  { path: '/diagnostics', title: 'nav.diagnostics', icon: Activity },
  { path: '/general', title: 'nav.general', icon: Settings2 },
]

export function App() {
  const t = useT()
  const lang = useLang()
  const location = useLocation()
  const status = useDaemon('status', true, true)
  const theme = useUI((state) => state.theme)
  const language = useUI((state) => state.language)
  const setLanguage = useUI((state) => state.setLanguage)
  const [menuOpen, setMenuOpen] = useState(false)
  const current = navigation.find((item) => item.path === location.pathname)
  const title = current ? t(current.title) : 'Tylora'

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = () => { document.documentElement.dataset.theme = theme === 'system' ? (media.matches ? 'dark' : 'light') : theme }
    apply(); media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [theme])
  useEffect(() => { document.documentElement.lang = lang }, [lang])

  return <div className="app-shell">
    <aside className="sidebar">
      <div className="brand"><div className="brand-mark"><span /><span /><span /></div><div><strong>Tylora</strong><small>{t('app.controlCenter')}</small></div></div>
      <div className="sidebar-section">{t('app.sectionWorkspace')}</div>
      <nav aria-label={t('app.mainNavigation')}>{navigation.map(({ path, title: name, icon: Icon }) =>
        <NavLink key={path} to={path} end={path === '/'} className={({ isActive }) => `nav-link ${isActive ? 'active' : ''}`}>
          <Icon size={17} strokeWidth={1.9} /><span>{t(name)}</span>
        </NavLink>)}</nav>
      <div className="sidebar-bottom"><div className="sidebar-divider" /><div className="sidebar-foot"><span className={status.isSuccess ? 'live-dot' : 'off-dot'} />{status.isSuccess ? t('app.daemonConnected') : t('app.daemonOffline')}</div><small>Tylora 0.1 · macOS</small></div>
    </aside>
    <div className="main-shell">
      <header className="topbar"><div className="breadcrumb"><SlidersHorizontal size={15} /><span>Tylora</span><span className="slash">/</span><strong>{title}</strong></div>
        <div className="top-actions"><div className={`connection ${status.isSuccess ? 'connected' : 'disconnected'}`}><span />{status.isSuccess ? t('app.connected') : t('app.disconnected')}</div>
          <label className="row" style={{ gap: 6 }} title={t('app.language')}><Languages size={15} className="muted" /><select aria-label={t('app.language')} className="search-input" style={{ width: 'auto', padding: '4px 8px' }} value={language} onChange={(event) => setLanguage(event.target.value as LanguagePref)}>
            <option value="system">{t('language.system')}</option><option value="en">English</option><option value="ru">Русский</option></select></label>
          <button className="icon-button" aria-label={t('app.moreActions')} onClick={() => setMenuOpen(!menuOpen)}><Menu size={18} /></button>
          {menuOpen && <div className="top-menu"><button onClick={() => { void status.refetch(); setMenuOpen(false) }}><RefreshCcw size={15} /> {t('app.refreshStatus')}</button><NavLink to="/diagnostics" onClick={() => setMenuOpen(false)}><CircleHelp size={15} /> {t('nav.diagnostics')}</NavLink></div>}
        </div>
      </header>
      <main className="page"><Routes>
        <Route path="/" element={<Dashboard />} />
        <Route path="/workspaces" element={<Workspaces />} />
        <Route path="/displays" element={<Displays />} />
        <Route path="/layouts" element={<Layouts />} />
        <Route path="/hotkeys" element={<Hotkeys />} />
        <Route path="/rules" element={<WindowRules />} />
        <Route path="/scratchpads" element={<Scratchpads />} />
        <Route path="/focus" element={<FocusMouse />} />
        <Route path="/diagnostics" element={<Diagnostics />} />
        <Route path="/general" element={<General />} />
      </Routes></main>
    </div>
  </div>
}
