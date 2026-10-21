import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowLeft, ArrowRight, CircleCheck, ExternalLink, Keyboard, LayoutGrid, Play, ShieldCheck, Sparkles } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { appInfo, controlDaemon, openSystemPane, requestAccessibility, saveGuiPreferences, type GuiPreferences } from '../api/bridge'
import { useDaemon } from '../api/hooks'
import { useT, type MessageKey } from '../i18n'
import { Appear, Badge, Button, Card, Keys, Notice } from '../shared/ui'
import { BrandMark } from '../shared/BrandMark'

const STEPS: { title: MessageKey; icon: LucideIcon }[] = [
  { title: 'onboarding.welcome', icon: Sparkles },
  { title: 'onboarding.accessibility', icon: ShieldCheck },
  { title: 'onboarding.hotkeys', icon: Keyboard },
  { title: 'onboarding.workspaces', icon: LayoutGrid },
  { title: 'onboarding.start', icon: Play },
]

/** Defaults installed by the daemon when nothing else binds the chord. */
const DEFAULT_HOTKEYS: [string, MessageKey][] = [
  ['option+h', 'onboarding.keyFocus'],
  ['option+shift+h', 'onboarding.keySwap'],
  ['option+f', 'onboarding.keyFloating'],
  ['option+shift+f', 'onboarding.keyZoom'],
  ['option+r', 'onboarding.keyReconcile'],
  ['option+1', 'onboarding.keyWorkspace'],
  ['option+shift+1', 'onboarding.keyMoveWorkspace'],
]

/**
 * First-run setup: Welcome → Accessibility → hotkeys → Workspaces → Start.
 * Every state shown is read back from macOS or the daemon; nothing starts or
 * registers without an explicit click, and every step can go back.
 */
export function Onboarding({ prefs, onDone }: { prefs: GuiPreferences; onDone: (next: GuiPreferences) => void }) {
  const t = useT()
  const client = useQueryClient()
  const [step, setStep] = useState(0)
  const [autoStart, setAutoStart] = useState(prefs.start_daemon_automatically)
  const [atLogin, setAtLogin] = useState(prefs.launch_at_login)
  const info = useQuery({ queryKey: ['app-info'], queryFn: appInfo, refetchInterval: step === 1 || step === 4 ? 2000 : false })
  const status = useDaemon('status', step === 3 || step === 4)
  const spaces = status.data?.native_spaces ?? []
  const prompt = useMutation({ mutationFn: requestAccessibility, onSettled: () => void info.refetch() })
  const start = useMutation({ mutationFn: () => controlDaemon('start'), onSettled: async () => { await client.invalidateQueries({ queryKey: ['daemon'] }); void info.refetch() } })
  const finish = useMutation({
    mutationFn: () => saveGuiPreferences({ ...prefs, start_daemon_automatically: autoStart, launch_at_login: atLogin, onboarding_completed: true }),
    onSuccess: (next) => { client.setQueryData(['gui-prefs'], next); onDone(next) },
  })
  const trusted = info.data?.accessibility_trusted
  const running = status.isSuccess
  const last = step === STEPS.length - 1
  const skip = useMutation({
    mutationFn: () => saveGuiPreferences({ ...prefs, onboarding_completed: true }),
    onSuccess: (next) => { client.setQueryData(['gui-prefs'], next); onDone(next) },
  })

  return <div className="onboarding" role="dialog" aria-modal="true" aria-label={t('onboarding.label')}>
    <div className="onboarding-panel">
      <header className="onboarding-head">
        <div className="brand"><BrandMark /><div><strong>Tylora</strong><small>{t('onboarding.setup')}</small></div></div>
        <ol className="onboarding-steps" aria-label={t('onboarding.progress')}>{STEPS.map(({ title, icon: Icon }, index) =>
          <li key={title} className={index === step ? 'current' : index < step ? 'done' : ''} aria-current={index === step ? 'step' : undefined}><Icon size={13} /><span>{t(title)}</span></li>)}</ol>
      </header>
      <Appear key={step}><div className="onboarding-body">
        {step === 0 && <>
          <h1>{t('onboarding.welcomeTitle')}</h1>
          <p className="muted">{t('onboarding.welcomeIntro')}</p>
          <Card className="padded"><ul className="onboarding-list">
            <li>{t('onboarding.welcomeLayouts')}</li><li>{t('onboarding.welcomeSpaces')}</li><li>{t('onboarding.welcomeDaemon')}</li>
          </ul></Card>
          <Notice tone="warn">{t('onboarding.experimental')}</Notice>
        </>}
        {step === 1 && <>
          <h1>{t('onboarding.axTitle')}</h1>
          <p className="muted">{t('onboarding.axIntro')}</p>
          <Card className="padded">
            <div className="row row-pad"><div><span className="row-title">Accessibility</span><span className="row-sub">{t('onboarding.axWho')}</span></div>
              {info.isSuccess ? <Badge tone={trusted ? 'good' : 'bad'}>{trusted ? t('common.granted') : t('dashboard.permissionNeeded')}</Badge> : <Badge>{t('common.loadingShort')}</Badge>}</div>
            {!trusted && <div className="row row-pad"><div><span className="row-title">{t('onboarding.axGrant')}</span><span className="row-sub">{t('onboarding.axGrantHint')}</span></div>
              <div className="list-actions"><Button size="sm" variant="primary" disabled={prompt.isPending} onClick={() => prompt.mutate()}><ShieldCheck size={13} /> {t('onboarding.axRequest')}</Button><Button size="sm" onClick={() => void openSystemPane('accessibility')}><ExternalLink size={13} /> {t('onboarding.openSettings')}</Button></div></div>}
          </Card>
          <Notice>{t('onboarding.axSigning')}</Notice>
          <p className="row-sub">{t('onboarding.inputMonitoring')} <button className="link-button" onClick={() => void openSystemPane('input_monitoring')}>{t('onboarding.openInputMonitoring')}</button></p>
        </>}
        {step === 2 && <>
          <h1>{t('onboarding.hotkeysTitle')}</h1>
          <p className="muted">{t('onboarding.hotkeysIntro')}</p>
          <Card className="padded">{DEFAULT_HOTKEYS.map(([chord, label]) =>
            <div className="row row-pad" key={chord}><span className="row-title">{t(label)}</span><Keys chord={chord} /></div>)}</Card>
          <p className="row-sub">{t('onboarding.hotkeysLater')}</p>
        </>}
        {step === 3 && <>
          <h1>{t('onboarding.workspacesTitle')}</h1>
          <p className="muted">{t('onboarding.workspacesIntro')}</p>
          <Card className="padded">
            <div className="row row-pad"><div><span className="row-title">{t('dashboard.nativeSpaces')}</span><span className="row-sub">{running ? t('dashboard.spacesDiscovered', { count: spaces.length }) : t('onboarding.workspacesNeedDaemon')}</span></div>
              <Badge tone={running ? (spaces.length ? 'good' : 'warn') : 'neutral'}>{running ? (spaces.length ? t('common.available') : t('common.unavailable')) : t('dashboard.stopped')}</Badge></div>
          </Card>
          <p className="row-sub">{t('onboarding.workspacesLater')}</p>
        </>}
        {step === 4 && <>
          <h1>{t('onboarding.startTitle')}</h1>
          <p className="muted">{t('onboarding.startIntro')}</p>
          <Card className="padded">
            <div className="row row-pad"><div><span className="row-title">{t('dashboard.daemon')}</span><span className="row-sub">{running ? t('onboarding.running', { version: status.data.daemon_version ?? '?' }) : trusted === false ? t('onboarding.startNeedsAx') : t('dashboard.stopped')}</span></div>
              {running ? <Badge tone="good"><CircleCheck size={12} /> {t('dashboard.running')}</Badge> : <Button size="sm" variant="primary" disabled={start.isPending} onClick={() => start.mutate()}><Play size={13} /> {t('dashboard.start')}</Button>}</div>
            <div className="row row-pad"><div><span className="row-title">{t('general.startDaemon')}</span><span className="row-sub">{t('general.startDaemonHint')}</span></div><input type="checkbox" aria-label={t('general.startDaemon')} checked={autoStart} onChange={(event) => setAutoStart(event.target.checked)} /></div>
            <div className="row row-pad"><div><span className="row-title">{t('general.launchAtLogin')}</span><span className="row-sub">{t('general.launchAtLoginHint')}</span></div><input type="checkbox" aria-label={t('general.launchAtLogin')} checked={atLogin} onChange={(event) => setAtLogin(event.target.checked)} /></div>
          </Card>
          {start.isError && <p className="bad-text">{start.error.message}</p>}
          {finish.isError && <p className="bad-text">{String(finish.error)}</p>}
        </>}
      </div></Appear>
      <footer className="onboarding-foot">
        <Button variant="ghost" disabled={skip.isPending} onClick={() => skip.mutate()}>{t('onboarding.skip')}</Button>
        <div className="list-actions">
          {step > 0 && <Button onClick={() => setStep(step - 1)}><ArrowLeft size={14} /> {t('onboarding.back')}</Button>}
          {last ? <Button variant="primary" disabled={finish.isPending} onClick={() => finish.mutate()}><CircleCheck size={14} /> {t('onboarding.finish')}</Button>
            : <Button variant="primary" onClick={() => setStep(step + 1)}>{t('onboarding.next')} <ArrowRight size={14} /></Button>}
        </div>
      </footer>
    </div>
  </div>
}
