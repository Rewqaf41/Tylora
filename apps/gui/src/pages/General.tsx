import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowUpCircle, ExternalLink, RefreshCw, RotateCcw, ShieldCheck, Sparkles } from 'lucide-react'
import { useInstallUpdate, useUpdateCheck } from '../features/Updates'
import { appInfo, guiPreferences, openReleaseNotes, openSystemPane, requestAccessibility, saveGuiPreferences, type GuiPreferences } from '../api/bridge'
import { useUI, type Theme } from '../app/store'
import { useDaemon } from '../api/hooks'
import { useT, type LanguagePref } from '../i18n'
import { Badge, Button, Card, ConfirmButton, ErrorState, Loading, Notice, Segmented } from '../shared/ui'

type Preferences = GuiPreferences
const defaults: Omit<Preferences, 'onboarding_completed' | 'last_seen_version'> = { version: 1, launch_at_login: false, start_daemon_automatically: false, open_settings_at_launch: true, show_menu_bar_icon: true, check_for_updates: true }

export function General() {
  const t = useT()
  const theme = useUI((state) => state.theme)
  const language = useUI((state) => state.language)
  const daemon = useDaemon('status')
  const setTheme = useUI((state) => state.setTheme)
  const setLanguage = useUI((state) => state.setLanguage)
  const client = useQueryClient()
  const prefs = useQuery({ queryKey: ['gui-prefs'], queryFn: guiPreferences })
  const info = useQuery({ queryKey: ['app-info'], queryFn: appInfo, refetchInterval: 5000 })
  const path = useQuery({ queryKey: ['config-path'], queryFn: () => invoke<string>('config_location'), staleTime: Infinity })
  const save = useMutation({ mutationFn: saveGuiPreferences, onSuccess: (next) => { client.setQueryData(['gui-prefs'], next); void info.refetch() } })
  const updates = useUpdateCheck(Boolean(prefs.data?.check_for_updates))
  const { install, percent } = useInstallUpdate()
  const [openError, setOpenError] = useState('')
  const update = (key: 'launch_at_login' | 'start_daemon_automatically' | 'open_settings_at_launch' | 'show_menu_bar_icon' | 'check_for_updates', checked: boolean) => { if (prefs.data) save.mutate({ ...prefs.data, [key]: checked }) }
  const open = async () => { try { await invoke('open_config_file'); setOpenError('') } catch (error) { setOpenError(String(error)) } }
  return <><div className="page-heading"><div><span className="eyebrow">{t('general.eyebrow')}</span><h1>{t('general.title')}</h1><p>{t('general.intro')}</p></div></div>
    {prefs.isPending ? <Loading /> : prefs.isError ? <ErrorState message={String(prefs.error)} retry={() => void prefs.refetch()} /> : <>
      <div className="two-col even"><div>
      <Card title={t('general.appearance')} description={t('general.appearanceHint')} className="padded">
        <div className="row row-pad"><div><span className="row-title">{t('general.theme')}</span><span className="row-sub">{t('general.themeHint')}</span></div><Segmented<Theme> label={t('general.theme')} value={theme} onChange={setTheme} options={[{ value: 'system', label: t('general.themeSystem') }, { value: 'light', label: t('general.themeLight') }, { value: 'dark', label: t('general.themeDark') }]} /></div>
        <div className="row row-pad"><div><span className="row-title">{t('app.language')}</span><span className="row-sub">{t('general.languageHint')}</span></div><Segmented<LanguagePref> label={t('app.language')} value={language} onChange={setLanguage} options={[{ value: 'system', label: t('language.system') }, { value: 'en', label: 'English' }, { value: 'ru', label: 'Русский' }]} /></div>
      </Card>
      <Card title={t('general.startup')} description={t('general.startupHint')} className="padded">
        {([
          ['launch_at_login', t('general.launchAtLogin'), t('general.launchAtLoginHint')],
          ['start_daemon_automatically', t('general.startDaemon'), t('general.startDaemonHint')],
          ['open_settings_at_launch', t('general.openAtLaunch'), t('general.openAtLaunchHint')],
          ['show_menu_bar_icon', t('general.menuBarIcon'), t('general.menuBarIconHint')],
        ] as const).map(([key, title, detail]) => <div className="row row-pad" key={key}><div><span className="row-title">{title}</span><span className="row-sub">{detail}</span></div><input aria-label={title} type="checkbox" checked={prefs.data[key]} disabled={save.isPending} onChange={(event) => update(key, event.target.checked)} /></div>)}
      </Card>
      {info.data?.login_item === 'requires_approval' && <Notice tone="warn">{t('general.loginApproval')} <button className="link-button" onClick={() => void openSystemPane('login_items')}>{t('general.openLoginItems')}</button></Notice>}
      {save.isError && <p className="bad-text">{save.error.message}</p>}{save.isSuccess && <p className="good-text">{t('general.saved')}</p>}
      </div><div>
      <Card title={t('general.configuration')} description={t('general.configurationHint')} className="padded"><div className="row row-pad"><div><span className="row-title">{t('general.configLocation')}</span><span className="row-sub mono">{path.data ?? t('common.loadingShort')}</span></div><Button size="sm" onClick={() => void open()}><ExternalLink size={13} /> {t('general.openFile')}</Button></div><div className="row row-pad"><div><span className="row-title">{t('general.logLevel')}</span><span className="row-sub">{t('general.logLevelHint')}</span></div><Badge tone="info">{t('general.requiresRestart')}</Badge></div></Card>
      {openError && <p className="bad-text">{openError}</p>}
      <Card title={t('general.permissions')} description={t('general.permissionsHint')} className="padded">
        <div className="row row-pad"><div><span className="row-title">Accessibility · {t('general.appTrust')}</span><span className="row-sub">{t('onboarding.axWho')}</span></div><div className="list-actions">{info.isSuccess && <Badge tone={info.data.accessibility_trusted ? 'good' : 'bad'}>{info.data.accessibility_trusted ? t('common.granted') : t('dashboard.permissionNeeded')}</Badge>}{info.isSuccess && !info.data.accessibility_trusted && <><Button size="sm" variant="primary" onClick={() => void requestAccessibility().then(() => info.refetch())}><ShieldCheck size={13} /> {t('general.axRequest')}</Button><Button size="sm" onClick={() => void openSystemPane('accessibility')}><ExternalLink size={13} /> {t('general.axOpen')}</Button></>}</div></div>
        <div className="row row-pad"><div><span className="row-title">Accessibility · {t('dashboard.daemon')}</span><span className="row-sub">{daemon.isSuccess ? (daemon.data.accessibility_trusted ? t('general.axGranted') : t('general.axMissing')) : t('general.axStartDaemon')}</span></div><div className="list-actions">{daemon.isSuccess && <Badge tone={daemon.data.accessibility_trusted ? 'good' : 'bad'}>{daemon.data.accessibility_trusted ? t('common.granted') : t('dashboard.permissionNeeded')}</Badge>}<Button variant="ghost" size="sm" onClick={() => { void daemon.refetch(); void info.refetch() }}><RefreshCw size={13} /> {t('general.checkPermissions')}</Button></div></div>
        <div className="row row-pad"><div><span className="row-title">{t('general.runSetup')}</span><span className="row-sub">{t('general.runSetupHint')}</span></div><Button size="sm" disabled={save.isPending} onClick={() => save.mutate({ ...prefs.data, onboarding_completed: false })}><Sparkles size={13} /> {t('general.runSetupButton')}</Button></div>
      </Card>
      <Card title={t('updates.title')} description={t('updates.hint', { version: __APP_VERSION__ })} className="padded">
        <div className="row row-pad"><div><span className="row-title">{t('updates.auto')}</span><span className="row-sub">{t('updates.autoHint')}</span></div><input aria-label={t('updates.auto')} type="checkbox" checked={prefs.data.check_for_updates} disabled={save.isPending} onChange={(event) => update('check_for_updates', event.target.checked)} /></div>
        <div className="row row-pad"><div><span className="row-title">{updates.data?.available ? t('updates.available', { version: updates.data.available.version }) : t('updates.status')}</span><span className="row-sub">{updates.isFetching ? t('updates.checking') : updates.isError ? updates.error.message : updates.data ? (updates.data.available ? (install.isPending ? (percent === null ? t('updates.installing') : t('updates.downloading', { percent })) : install.isError ? install.error.message : t('updates.availableHint', { current: updates.data.current })) : t('updates.upToDate', { version: updates.data.current })) : t('updates.never')}</span></div>
          <div className="list-actions"><Button size="sm" variant="ghost" onClick={() => void openReleaseNotes(updates.data?.available?.version)}><ExternalLink size={13} /> {t('updates.notes')}</Button>
            {updates.data?.available ? <Button size="sm" variant="primary" disabled={install.isPending} onClick={() => install.mutate()}><ArrowUpCircle size={13} /> {t('updates.install')}</Button>
              : <Button size="sm" disabled={updates.isFetching} onClick={() => void updates.refetch()}><RefreshCw size={13} /> {t('updates.checkNow')}</Button>}</div></div>
      </Card>
      <Card title={t('general.reset')} description={t('general.resetHint')} className="padded"><div className="row row-pad"><div><span className="row-title">{t('general.resetApp')}</span><span className="row-sub">{t('general.resetAppHint')}</span></div><ConfirmButton size="sm" prompt={t('general.resetConfirm')} disabled={save.isPending} onConfirm={() => { setTheme('system'); setLanguage('system'); save.mutate({ ...defaults, onboarding_completed: true, last_seen_version: prefs.data.last_seen_version }) }}><RotateCcw size={13} /> {t('general.resetButton')}</ConfirmButton></div></Card>
      </div></div>
      <Notice>{t('general.closingNotice')}</Notice>
    </>}
  </>
}
