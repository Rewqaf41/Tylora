import { ArrowRight, CircleCheck, Play, RotateCcw, ShieldCheck, Square, Wrench } from 'lucide-react'
import { useAction, useDaemon } from '../api/hooks'
import { useT } from '../i18n'
import { Badge, Button, Card, ConfirmButton, ErrorState, Loading, Metric, Notice, SectionHeader } from '../shared/ui'

export function Dashboard() {
  const t = useT()
  const status = useDaemon('status')
  const connected = status.isSuccess
  const config = useDaemon('config', connected)
  const capabilities = useDaemon('capabilities', connected)
  const action = useAction()
  const activeSpace = status.data?.native_spaces.find((space) => space.active)
  const currentWorkspace = config.data?.config.workspaces.find((ws) => ws.native_space_id === activeSpace?.id)
  const run = (value: Parameters<typeof action.mutate>[0]) => action.mutate(value)
  const canMove = capabilities.data?.capabilities.can_move_window_between_spaces

  return <>
    <div className="page-heading"><div><span className="eyebrow">{t('dashboard.eyebrow')}</span><h1>{t('dashboard.title')}</h1><p>{t('dashboard.intro')}</p></div>
      <div className="page-actions"><Button onClick={() => void status.refetch()}><RotateCcw size={13} /> {t('common.refresh')}</Button></div></div>
    {status.isPending ? <Loading /> : status.isError ? <>
      <Notice tone="warn">{t('dashboard.notConnected')}</Notice>
      <SectionHeader title={t('dashboard.control')} description={t('dashboard.controlHint')} />
      <Card className="padded row"><div><strong>{t('dashboard.stopped')}</strong><span className="row-sub">{t('dashboard.noState')}</span></div><Button variant="primary" disabled={action.isPending} onClick={() => run({ kind: 'control', op: 'start' })}><Play size={14} /> {t('dashboard.start')}</Button></Card>
      {action.isError && <p className="bad-text">{action.error.message}</p>}
      <div style={{ marginTop: 18 }}><ErrorState message={status.error.message} retry={() => void status.refetch()} /></div>
    </> : <>
      <div className="metric-grid">
        <Metric label={t('dashboard.daemon')} value={<span className="good-text">{t('dashboard.running')}</span>} detail={t('dashboard.viaIpc')} />
        <Metric label={t('dashboard.currentWorkspace')} value={currentWorkspace?.name ?? t('dashboard.unbound')} detail={activeSpace ? t('dashboard.nativeSpace', { id: activeSpace.id }) : t('dashboard.noActiveSpace')} />
        <Metric label={t('dashboard.managedWindows')} value={status.data.windows} detail={t('dashboard.tiledFloating', { tiled: status.data.tiled, floating: status.data.floating })} />
        <Metric label={t('dashboard.displays')} value={status.data.displays} detail={t('dashboard.primaryDisplay', { id: status.data.primary_display })} />
      </div>
      <div className="two-col">
        <div><SectionHeader title={t('dashboard.systemStatus')} description={t('dashboard.systemStatusHint')} />
          <Card className="padded">
            <div className="row row-pad"><div><span className="row-title">Accessibility</span><span className="row-sub">{t('dashboard.axHint')}</span></div><Badge tone={status.data.accessibility_trusted ? 'good' : 'bad'}>{status.data.accessibility_trusted ? t('common.granted') : t('dashboard.permissionNeeded')}</Badge></div>
            <div className="row row-pad"><div><span className="row-title">{t('dashboard.nativeSpaces')}</span><span className="row-sub">{t('dashboard.spacesDiscovered', { count: status.data.native_spaces.length })}</span></div><Badge tone={status.data.native_spaces.length ? 'good' : 'warn'}>{status.data.native_spaces.length ? t('common.available') : t('common.unavailable')}</Badge></div>
            <div className="row row-pad"><div><span className="row-title">{t('dashboard.spaceMovement')}</span><span className="row-sub">{t('dashboard.spaceMovementHint')}</span></div><Badge tone={canMove ? 'good' : 'warn'}>{canMove ? t('common.available') : t('common.unavailable')}</Badge></div>
            <div className="row row-pad"><div><span className="row-title">{t('dashboard.layoutEngine')}</span><span className="row-sub">{t('dashboard.layoutEngineHint')}</span></div><Badge>{status.data.layout.toUpperCase()}</Badge></div>
          </Card>
        </div>
        <div><SectionHeader title={t('dashboard.quickActions')} description={t('dashboard.quickActionsHint')} />
          <Card className="padded stack">
            <Button onClick={() => run({ kind: 'simple', action: 'reconcile' })} disabled={action.isPending}><RotateCcw size={14} /> {t('dashboard.reconcile')}</Button>
            <Button onClick={() => run({ kind: 'simple', action: 'recover_windows' })} disabled={action.isPending}><Wrench size={14} /> {t('dashboard.recover')}</Button>
            <ConfirmButton variant="secondary" prompt={t('dashboard.restartConfirm')} disabled={action.isPending} onConfirm={() => run({ kind: 'control', op: 'restart' })}><ArrowRight size={14} /> {t('dashboard.restart')}</ConfirmButton>
            <ConfirmButton prompt={t('dashboard.stopConfirm')} disabled={action.isPending} onConfirm={() => run({ kind: 'control', op: 'stop' })}><Square size={13} /> {t('dashboard.stop')}</ConfirmButton>
          </Card>
          {action.isError && <p className="bad-text">{action.error.message}</p>}
          {action.isSuccess && <p className="good-text"><CircleCheck size={13} style={{ display: 'inline' }} /> {t('dashboard.confirmed')}</p>}
        </div>
      </div>
      <SectionHeader title={t('dashboard.session')} description={t('dashboard.sessionHint')} />
      <Card className="padded row"><div><span className="row-title">{activeSpace ? t('dashboard.desktop', { n: activeSpace.desktop }) : t('dashboard.noDesktop')}</span><span className="row-sub">{activeSpace ? t('dashboard.spaceAndDisplay', { id: activeSpace.id, display: activeSpace.display }) : t('dashboard.noObservation')}</span></div><ShieldCheck size={20} className="muted" /></Card>
      {status.data.degraded > 0 && <div style={{ marginTop: 14 }}><Notice tone="warn">{t('dashboard.degraded', { count: status.data.degraded })}</Notice></div>}
    </>}
  </>
}
