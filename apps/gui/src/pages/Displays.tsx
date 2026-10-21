import { useConfigPatch, useDaemon } from '../api/hooks'
import type { Config } from '../api/types'
import { useT, type MessageKey } from '../i18n'
import { Monitor } from 'lucide-react'
import { Badge, Card, Empty, ErrorState, Keys, Loading, Notice } from '../shared/ui'

const MOVEMENT: Record<keyof Config['window_movement'], [MessageKey, MessageKey]> = {
  focus_after_move: ['displays.focusMoved', 'displays.focusMovedHint'],
  wrap_around: ['displays.wrap', 'displays.wrapHint'],
  preserve_floating_position: ['displays.preserveFloating', 'displays.preserveFloatingHint'],
}

export function Displays() {
  const t = useT()
  const status = useDaemon('status')
  const online = status.isSuccess
  const displays = useDaemon('displays', online)
  const spaces = useDaemon('spaces', online)
  const config = useDaemon('config', online)
  const hotkeys = useDaemon('hotkeys', online)
  const patch = useConfigPatch()
  const movement: Config['window_movement'] = config.data?.config.window_movement ?? { focus_after_move: false, wrap_around: true, preserve_floating_position: true }
  const updateMovement = (settings: Config['window_movement']) => { if (config.data) patch.mutate({ revision: config.data.revision, patch: { kind: 'set_window_movement', settings } }) }
  const moveHotkey = hotkeys.data?.hotkeys.find((entry) => entry.command.command === 'move_window_to_next_display' && entry.enabled)
  const all = displays.data?.displays ?? []
  const minX = Math.min(0, ...all.map((display) => display.full_frame.x))
  const minY = Math.min(0, ...all.map((display) => display.full_frame.y))
  const maxX = Math.max(1, ...all.map((display) => display.full_frame.x + display.full_frame.width))
  const maxY = Math.max(1, ...all.map((display) => display.full_frame.y + display.full_frame.height))
  const totalW = maxX - minX, totalH = maxY - minY
  return <><div className="page-heading"><div><span className="eyebrow">{t('displays.eyebrow')}</span><h1>{t('nav.displays')}</h1><p>{t('displays.intro')}</p></div></div>
    {!online ? !status.isFetched ? <Loading /> : <ErrorState message={t('displays.offline')} retry={() => void status.refetch()} /> : displays.isPending || spaces.isPending ? <Loading /> : displays.isError || spaces.isError ? <ErrorState message={displays.error?.message ?? spaces.error?.message ?? t('common.unknownError')} /> : all.length === 0 ? <Empty title={t('displays.none')}>{t('displays.noneHint')}</Empty> : <>
      <Card title={t('displays.arrangement')} description={t('displays.arrangementHint')} aside={<Badge>{t('displays.reported', { count: all.length })}</Badge>} className="padded">
        <div className="monitor-map" style={{ height: 210 }}>{all.map((display) => { const current = spaces.data?.spaces.find((space) => space.display === display.id && space.active); return <div key={display.id} className={`monitor-visual${display.primary ? ' primary' : ''}`} style={{ position: 'absolute', left: `${6 + (display.full_frame.x - minX) / totalW * 88}%`, top: `${8 + (display.full_frame.y - minY) / totalH * 84}%`, width: `${Math.max(15, display.full_frame.width / totalW * 88)}%`, height: `${Math.max(25, display.full_frame.height / totalH * 84)}%` }}>
          <div className="row" style={{ gap: 8 }}><span>{t('common.display', { id: display.id })}</span>{display.primary && <Badge tone="accent">{t('common.primary')}</Badge>}</div>
          <small>{current ? t('dashboard.desktop', { n: current.desktop }) : `${display.full_frame.width} × ${display.full_frame.height} pt`}</small>
        </div> })}</div>
      </Card>
      <div className="two-col even">
        <div>
          <Card title={t('displays.connected')} description={t('displays.reported', { count: all.length })} className="padded">
            {all.map((display) => { const current = spaces.data?.spaces.find((space) => space.display === display.id && space.active); const count = spaces.data?.spaces.filter((space) => space.display === display.id).length ?? 0; return <div key={display.id} className="row-pad stack" style={{ gap: 10 }}>
              <div className="row"><span className="row-title" style={{ display: 'flex', gap: 9, alignItems: 'center' }}><Monitor size={15} className={display.primary ? '' : 'faint'} style={display.primary ? { color: 'var(--accent)' } : undefined} />{display.primary ? t('displays.primaryDisplay') : t('common.display', { id: display.id })}</span><Badge tone={display.primary ? 'accent' : 'neutral'}>{display.primary ? t('common.primary') : t('displays.external')}</Badge></div>
              <div className="spec-grid">
                <div><span>{t('displays.scaling')}</span>{display.scale.toFixed(2)}×</div>
                <div><span>{t('nav.workspaces')}</span>{count}</div>
                <div><span>{t('displays.currentDesktop')}</span>{current ? `${current.desktop} · ID ${current.id}` : t('common.unavailable')}</div>
                <div><span>{t('displays.assigned')}</span>{config.data?.config.workspaces.filter((ws) => ws.display === display.id || ws.display === null).length ?? '—'}</div>
              </div>
              <span className="row-sub mono" style={{ marginTop: 0 }}>CGDirectDisplayID {display.id} · {display.full_frame.width} × {display.full_frame.height} pt</span>
            </div> })}
          </Card>
          <Card title={t('displays.disconnect')} className="padded">
            <div className="row row-pad"><div><span className="row-title">{t('displays.disconnect')}</span><span className="row-sub">{t('displays.disconnectHint')}</span></div><Badge>{t('displays.moveToPrimary')}</Badge></div>
          </Card>
        </div>
        <div>
          <Card title={t('displays.moveTitle')} description={t('displays.moveHint')} className="padded">
            <div className="row row-pad"><div><span className="row-title">{t('displays.moveNext')}</span><span className="row-sub">{all.length < 2 ? t('displays.connectAnother') : t('displays.shortcut', { chord: moveHotkey?.chord ?? t('displays.notAssigned'), state: moveHotkey && !moveHotkey.active ? t('displays.unavailableSuffix') : '' })}</span></div>{moveHotkey && <Keys chord={moveHotkey.chord} />}</div>
            {(Object.keys(MOVEMENT) as (keyof Config['window_movement'])[]).map((key) => <label key={key} className="row row-pad"><span><span className="row-title">{t(MOVEMENT[key][0])}</span><span className="row-sub">{t(MOVEMENT[key][1])}</span></span><input type="checkbox" checked={movement[key]} disabled={patch.isPending} onChange={(event) => updateMovement({ ...movement, [key]: event.target.checked })} /></label>)}
            {patch.isError && <p className="bad-text">{patch.error.message}</p>}
          </Card>
          <Card title={t('displays.navigation')} description={t('displays.navigationHint')} className="padded">
            <div className="row row-pad"><div><span className="row-title">{t('displays.crossMonitor')}</span><span className="row-sub">{t('displays.crossMonitorHint')}</span></div><Badge tone={config.data?.config.cross_monitor_navigation ? 'good' : 'neutral'}>{config.data?.config.cross_monitor_navigation ? t('common.enabled') : t('common.disabled')}</Badge></div>
            <div className="row row-pad"><div><span className="row-title">{t('focus.followsMouse')}</span><span className="row-sub">{t('displays.ffmHint')}</span></div><Badge tone="bad">{t('displays.unsupported')}</Badge></div>
          </Card>
        </div>
      </div>
      <Notice>{t('displays.notice')}</Notice>
    </>}
  </>
}
