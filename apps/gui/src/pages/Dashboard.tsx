import { Activity, AppWindow, CircleCheck, CircleDot, Link2, LayoutGrid, LifeBuoy, Monitor, Play, Power, RotateCcw, RotateCw, TrendingUp, Wand2 } from 'lucide-react'
import { Link } from 'react-router'
import { useAction, useDaemon } from '../api/hooks'
import { useT } from '../i18n'
import { Badge, Button, Card, ConfirmButton, ErrorState, Loading, Metric, Notice, PageHeading } from '../shared/ui'

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
    <PageHeading eyebrow={t('dashboard.eyebrow')} title={t('dashboard.title')} intro={t('dashboard.intro')} actions={<>
      <Button onClick={() => void status.refetch()}><RotateCcw size={14} /> {t('common.refresh')}</Button>
      {connected && <Button variant="primary" onClick={() => run({ kind: 'simple', action: 'reconcile' })} disabled={action.isPending}><Wand2 size={14} /> {t('dashboard.reconcile')}</Button>}
    </>} />
    {status.isPending ? <Loading /> : status.isError ? <>
      <Notice tone="warn">{t('dashboard.notConnected')}</Notice>
      <Card title={t('dashboard.control')} description={t('dashboard.controlHint')} className="padded">
        <div className="row row-pad"><div><span className="row-title">{t('dashboard.stopped')}</span><span className="row-sub">{t('dashboard.noState')}</span></div><Button variant="primary" disabled={action.isPending} onClick={() => run({ kind: 'control', op: 'start' })}><Play size={14} /> {t('dashboard.start')}</Button></div>
      </Card>
      {action.isError && <p className="bad-text">{action.error.message}</p>}
      <ErrorState message={status.error.message} retry={() => void status.refetch()} />
    </> : <>
      <div className="metric-grid">
        <Metric icon={Activity} label={t('dashboard.daemon')} value={t('dashboard.running')} detail={t('dashboard.viaIpc')} detailIcon={CircleCheck} tone="good" />
        <Metric icon={LayoutGrid} label={t('dashboard.currentWorkspace')} value={currentWorkspace?.name ?? t('dashboard.unbound')} detail={activeSpace ? t('dashboard.nativeSpace', { id: activeSpace.id }) : t('dashboard.noActiveSpace')} detailIcon={Link2} />
        <Metric icon={AppWindow} label={t('dashboard.managedWindows')} value={status.data.windows} detail={t('dashboard.tiledFloating', { tiled: status.data.tiled, floating: status.data.floating })} detailIcon={TrendingUp} tone="good" />
        <Metric icon={Monitor} label={t('dashboard.displays')} value={status.data.displays} detail={t('dashboard.primaryDisplay', { id: status.data.primary_display })} detailIcon={CircleDot} />
      </div>
      <div className="two-col">
        <div>
          <Card title={t('dashboard.systemStatus')} description={t('dashboard.systemStatusHint')} className="padded">
            <div className="row row-pad"><div><span className="row-title">Accessibility</span><span className="row-sub">{t('dashboard.axHint')}</span></div><Badge tone={status.data.accessibility_trusted ? 'good' : 'bad'}>{status.data.accessibility_trusted ? t('common.granted') : t('dashboard.permissionNeeded')}</Badge></div>
            <div className="row row-pad"><div><span className="row-title">{t('dashboard.nativeSpaces')}</span><span className="row-sub">{t('dashboard.spacesDiscovered', { count: status.data.native_spaces.length })}</span></div><Badge tone={status.data.native_spaces.length ? 'good' : 'warn'}>{status.data.native_spaces.length ? t('common.available') : t('common.unavailable')}</Badge></div>
            <div className="row row-pad"><div><span className="row-title">{t('dashboard.spaceMovement')}</span><span className="row-sub">{t('dashboard.spaceMovementHint')}</span></div><Badge tone={canMove ? 'good' : 'bad'}>{canMove ? t('common.available') : t('common.unavailable')}</Badge></div>
            <div className="row row-pad"><div><span className="row-title">{t('dashboard.layoutEngine')}</span><span className="row-sub">{t('dashboard.layoutEngineHint')}</span></div><Badge tone="accent">{status.data.layout.toUpperCase()}</Badge></div>
          </Card>
          {status.data.degraded > 0 && <div className="notice notice-warn"><Activity size={15} /><span className="warn-text" style={{ flex: 1 }}>{t('dashboard.degraded', { count: status.data.degraded })}</span><Link className="button button-sm notice-action" to="/diagnostics">{t('dashboard.openDiagnostics')}</Link></div>}
        </div>
        <div>
          <Card title={t('dashboard.quickActions')} description={t('dashboard.quickActionsHint')}>
            <div className="card-body stack" style={{ gap: 10 }}>
              <Button size="block" onClick={() => run({ kind: 'simple', action: 'reconcile' })} disabled={action.isPending}><Wand2 size={14} /> {t('dashboard.reconcile')}</Button>
              <Button size="block" onClick={() => run({ kind: 'simple', action: 'recover_windows' })} disabled={action.isPending}><LifeBuoy size={14} /> {t('dashboard.recover')}</Button>
              <ConfirmButton size="block" variant="secondary" prompt={t('dashboard.restartConfirm')} disabled={action.isPending} onConfirm={() => run({ kind: 'control', op: 'restart' })}><RotateCw size={14} /> {t('dashboard.restart')}</ConfirmButton>
              <ConfirmButton size="block" prompt={t('dashboard.stopConfirm')} disabled={action.isPending} onConfirm={() => run({ kind: 'control', op: 'stop' })}><Power size={14} /> {t('dashboard.stop')}</ConfirmButton>
              {action.isError && <p className="bad-text" style={{ margin: 0 }}>{action.error.message}</p>}
              {action.isSuccess && <p className="good-text" style={{ margin: 0 }}><CircleCheck size={13} style={{ display: 'inline', verticalAlign: -2 }} /> {t('dashboard.confirmed')}</p>}
            </div>
          </Card>
          <Card title={t('dashboard.session')} description={t('dashboard.sessionHint')}>
            <div className="card-body">{activeSpace ? <div className="facts">
              <div className="fact"><span>{t('dashboard.factDesktop')}</span><span>{t('dashboard.desktop', { n: activeSpace.desktop })}</span></div>
              <div className="fact"><span>{t('dashboard.factSpace')}</span><span className="mono">{activeSpace.id}</span></div>
              <div className="fact"><span>{t('dashboard.factDisplay')}</span><span>{activeSpace.display}</span></div>
            </div> : <><span className="row-title">{t('dashboard.noDesktop')}</span><span className="row-sub">{t('dashboard.noObservation')}</span></>}</div>
          </Card>
        </div>
      </div>
    </>}
  </>
}
