import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ExternalLink, RotateCcw } from 'lucide-react'
import { useUI, type Theme } from '../app/store'
import { useDaemon } from '../api/hooks'
import { useT, type LanguagePref } from '../i18n'
import { Badge, Button, Card, ConfirmButton, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

interface Preferences { version: 1; launch_at_login: boolean; start_daemon_automatically: boolean; open_settings_at_launch: boolean; show_menu_bar_icon: boolean }
const defaults: Preferences = { version: 1, launch_at_login: false, start_daemon_automatically: false, open_settings_at_launch: true, show_menu_bar_icon: true }

export function General() {
  const t = useT()
  const theme = useUI((state) => state.theme)
  const language = useUI((state) => state.language)
  const daemon = useDaemon('status')
  const setTheme = useUI((state) => state.setTheme)
  const setLanguage = useUI((state) => state.setLanguage)
  const client = useQueryClient()
  const prefs = useQuery({ queryKey: ['gui-prefs'], queryFn: () => invoke<Preferences>('gui_preferences') })
  const path = useQuery({ queryKey: ['config-path'], queryFn: () => invoke<string>('config_location'), staleTime: Infinity })
  const save = useMutation({ mutationFn: (next: Preferences) => invoke<Preferences>('save_gui_preferences', { prefs: next }), onSuccess: (next) => client.setQueryData(['gui-prefs'], next) })
  const [openError, setOpenError] = useState('')
  const update = (key: keyof Omit<Preferences, 'version'>, checked: boolean) => { if (prefs.data) save.mutate({ ...prefs.data, [key]: checked }) }
  const open = async () => { try { await invoke('open_config_file'); setOpenError('') } catch (error) { setOpenError(String(error)) } }
  return <><div className="page-heading"><div><span className="eyebrow">{t('general.eyebrow')}</span><h1>{t('general.title')}</h1><p>{t('general.intro')}</p></div></div>
    {prefs.isPending ? <Loading /> : prefs.isError ? <ErrorState message={String(prefs.error)} retry={() => void prefs.refetch()} /> : <>
      <SectionHeader title={t('general.appearance')} description={t('general.appearanceHint')} /><Card className="padded">
        <div className="row row-pad"><div><strong>{t('general.theme')}</strong><span className="row-sub">{t('general.themeHint')}</span></div><select aria-label={t('general.theme')} className="search-input" style={{ width: 130 }} value={theme} onChange={(event) => setTheme(event.target.value as Theme)}><option value="system">{t('general.themeSystem')}</option><option value="light">{t('general.themeLight')}</option><option value="dark">{t('general.themeDark')}</option></select></div>
        <div className="row row-pad"><div><strong>{t('app.language')}</strong><span className="row-sub">{t('general.languageHint')}</span></div><select aria-label={t('app.language')} className="search-input" style={{ width: 130 }} value={language} onChange={(event) => setLanguage(event.target.value as LanguagePref)}><option value="system">{t('language.system')}</option><option value="en">English</option><option value="ru">Русский</option></select></div>
      </Card>
      <SectionHeader title={t('general.startup')} description={t('general.startupHint')} /><Card className="padded">
        {([
          ['launch_at_login', t('general.launchAtLogin'), t('general.launchAtLoginHint')],
          ['start_daemon_automatically', t('general.startDaemon'), t('general.startDaemonHint')],
          ['open_settings_at_launch', t('general.openAtLaunch'), t('general.openAtLaunchHint')],
          ['show_menu_bar_icon', t('general.menuBarIcon'), t('general.menuBarIconHint')],
        ] as const).map(([key, title, detail]) => <div className="row row-pad" key={key}><div><span className="row-title">{title}</span><span className="row-sub">{detail}</span></div><input aria-label={title} type="checkbox" checked={prefs.data[key]} disabled={save.isPending} onChange={(event) => update(key, event.target.checked)} /></div>)}
      </Card>
      {save.isError && <p className="bad-text">{String(save.error)}</p>}{save.isSuccess && <p className="good-text">{t('general.saved')}</p>}
      <SectionHeader title={t('general.configuration')} description={t('general.configurationHint')} /><Card className="padded"><div className="row row-pad"><div><strong>{t('general.configLocation')}</strong><span className="row-sub mono">{path.data ?? t('common.loadingShort')}</span></div><Button onClick={() => void open()}><ExternalLink size={13} /> {t('general.openFile')}</Button></div><div className="row row-pad"><div><strong>{t('general.logLevel')}</strong><span className="row-sub">{t('general.logLevelHint')}</span></div><Badge tone="warn">{t('general.requiresRestart')}</Badge></div></Card>
      {openError && <p className="bad-text">{openError}</p>}
      <SectionHeader title={t('general.permissions')} description={t('general.permissionsHint')} /><Card className="padded row"><div><strong>Accessibility</strong><span className="row-sub">{daemon.isSuccess ? (daemon.data.accessibility_trusted ? t('general.axGranted') : t('general.axMissing')) : t('general.axStartDaemon')}</span></div><Button onClick={() => void daemon.refetch()}>{t('general.checkPermissions')}</Button></Card>
      <SectionHeader title={t('general.reset')} description={t('general.resetHint')} /><Card className="padded row"><div><strong>{t('general.resetApp')}</strong><span className="row-sub">{t('general.resetAppHint')}</span></div><ConfirmButton prompt={t('general.resetConfirm')} disabled={save.isPending} onConfirm={() => { setTheme('system'); setLanguage('system'); save.mutate(defaults) }}><RotateCcw size={13} /> {t('general.resetButton')}</ConfirmButton></Card>
      <div style={{ marginTop: 17 }}><Notice>{t('general.closingNotice')}</Notice></div>
    </>}
  </>
}
