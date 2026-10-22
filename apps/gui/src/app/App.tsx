import { useEffect, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { NavLink, Route, Routes, useLocation } from 'react-router'
import { ChevronDown, Columns3, Ellipsis, Keyboard, LayoutDashboard, LayoutGrid, Languages, Layers, ListChecks, Monitor, MousePointer2, RefreshCcw, Settings, Sparkles, Stethoscope } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { useDaemon } from '../api/hooks'
import { guiPreferences } from '../api/bridge'
import { Onboarding } from '../features/Onboarding'
import { UpdateBanner } from '../features/Updates'
import { useLang, useT, type LanguagePref, type MessageKey } from '../i18n'
import { useUI } from './store'
import { BrandMark } from '../shared/BrandMark'
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
import { Animations } from '../pages/Animations'

const navigation: { path: string; title: MessageKey; icon: LucideIcon }[] = [
  { path: '/', title: 'nav.dashboard', icon: LayoutDashboard },
  { path: '/workspaces', title: 'nav.workspaces', icon: LayoutGrid },
  { path: '/displays', title: 'nav.displays', icon: Monitor },
  { path: '/layouts', title: 'nav.layouts', icon: Columns3 },
  { path: '/hotkeys', title: 'nav.hotkeys', icon: Keyboard },
  { path: '/rules', title: 'nav.rules', icon: ListChecks },
  { path: '/scratchpads', title: 'nav.scratchpads', icon: Layers },
  { path: '/focus', title: 'nav.focus', icon: MousePointer2 },
  { path: '/animations', title: 'nav.animations', icon: Sparkles },
  { path: '/diagnostics', title: 'nav.diagnostics', icon: Stethoscope },
  { path: '/general', title: 'nav.general', icon: Settings },
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
  const scroller = useRef<HTMLDivElement>(null)
  const client = useQueryClient()
  const prefs = useQuery({ queryKey: ['gui-prefs'], queryFn: guiPreferences })
  const current = navigation.find((item) => item.path === location.pathname)
  const title = current ? t(current.title) : 'Tylora'

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = () => { document.documentElement.dataset.theme = theme === 'system' ? (media.matches ? 'dark' : 'light') : theme }
    apply(); media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [theme])
  useEffect(() => { document.documentElement.lang = lang }, [lang])
  // The content pane scrolls, not the window: start every page at its top.
  useEffect(() => { scroller.current?.scrollTo(0, 0); setMenuOpen(false) }, [location.pathname])

  return <div className="app-shell">
    {prefs.data && !prefs.data.onboarding_completed && <Onboarding prefs={prefs.data} onDone={(next) => client.setQueryData(['gui-prefs'], next)} />}
    <aside className="sidebar">
      <div className="brand"><BrandMark /><div><strong>Tylora</strong><small>{t('app.controlCenter')}</small></div></div>
      <div className="sidebar-section">{t('app.sectionWorkspace')}</div>
      <nav aria-label={t('app.mainNavigation')}>{navigation.map(({ path, title: name, icon: Icon }) =>
        <NavLink key={path} to={path} end={path === '/'} title={t(name)} className={({ isActive }) => `nav-link ${isActive ? 'active' : ''}`}>
          <Icon size={16} strokeWidth={1.8} /><span>{t(name)}</span>
        </NavLink>)}</nav>
      <div className="sidebar-bottom"><div className="sidebar-foot"><span className={status.isSuccess ? 'live-dot' : 'off-dot'} /><span>{status.isSuccess ? t('app.daemonConnected') : t('app.daemonOffline')}</span></div><small>Tylora {__APP_VERSION__} · macOS</small></div>
    </aside>
    <div className="main-shell" ref={scroller}>
      <header className="topbar"><div className="breadcrumb"><span>Tylora</span><span>/</span><strong>{title}</strong></div>
        <div className="top-actions"><div className={`connection ${status.isSuccess ? 'connected' : 'disconnected'}`}><span />{status.isSuccess ? t('app.connected') : t('app.disconnected')}</div>
          <label className="pill-select" title={t('app.language')}><Languages size={14} /><select aria-label={t('app.language')} value={language} onChange={(event) => setLanguage(event.target.value as LanguagePref)}>
            <option value="system">{t('language.system')}</option><option value="en">English</option><option value="ru">Русский</option></select><ChevronDown size={12} className="chev" /></label>
          <button className="icon-button" aria-label={t('app.moreActions')} aria-expanded={menuOpen} onClick={() => setMenuOpen(!menuOpen)}><Ellipsis size={16} /></button>
          {menuOpen && <div className="top-menu"><button onClick={() => { void status.refetch(); setMenuOpen(false) }}><RefreshCcw size={14} /> {t('app.refreshStatus')}</button><NavLink to="/diagnostics" onClick={() => setMenuOpen(false)}><Stethoscope size={14} /> {t('nav.diagnostics')}</NavLink></div>}
        </div>
      </header>
      {prefs.data && <UpdateBanner prefs={prefs.data} />}
      <main className="page"><Routes>
        <Route path="/" element={<Dashboard />} />
        <Route path="/workspaces" element={<Workspaces />} />
        <Route path="/displays" element={<Displays />} />
        <Route path="/layouts" element={<Layouts />} />
        <Route path="/hotkeys" element={<Hotkeys />} />
        <Route path="/rules" element={<WindowRules />} />
        <Route path="/scratchpads" element={<Scratchpads />} />
        <Route path="/focus" element={<FocusMouse />} />
        <Route path="/animations" element={<Animations />} />
        <Route path="/diagnostics" element={<Diagnostics />} />
        <Route path="/general" element={<General />} />
      </Routes></main>
    </div>
  </div>
}
