import { useConfigPatch, useDaemon } from '../api/hooks'
import type { Config } from '../api/types'
import { useT, type MessageKey } from '../i18n'
import { Badge, Card, Empty, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

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
    {!online ? status.isPending ? <Loading /> : <ErrorState message={t('displays.offline')} retry={() => void status.refetch()} /> : displays.isPending || spaces.isPending ? <Loading /> : displays.isError || spaces.isError ? <ErrorState message={displays.error?.message ?? spaces.error?.message ?? t('common.unknownError')} /> : all.length === 0 ? <Empty title={t('displays.none')}>{t('displays.noneHint')}</Empty> : <>
      <SectionHeader title={t('displays.arrangement')} description={t('displays.arrangementHint')} />
      <Card className="padded"><div className="monitor-map" style={{ height: 230 }}>{all.map((display) => <div key={display.id} className="monitor-visual" style={{ position: 'absolute', left: `${8 + (display.full_frame.x - minX) / totalW * 84}%`, top: `${8 + (display.full_frame.y - minY) / totalH * 84}%`, width: `${Math.max(15, display.full_frame.width / totalW * 84)}%`, height: `${Math.max(25, display.full_frame.height / totalH * 84)}%` }}>{display.primary ? t('common.primary') : t('common.display', { id: display.id })}</div>)}</div></Card>
      <SectionHeader title={t('displays.connected')} description={t('displays.reported', { count: all.length })} />
      <div className="stack">{all.map((display) => { const current = spaces.data?.spaces.find((space) => space.display === display.id && space.active); const count = spaces.data?.spaces.filter((space) => space.display === display.id).length ?? 0; return <Card key={display.id} className="padded"><div className="row"><div><strong>{display.primary ? t('displays.primaryDisplay') : t('common.display', { id: display.id })}</strong><span className="row-sub">CGDirectDisplayID {display.id} · {display.full_frame.width} × {display.full_frame.height} pt</span></div><Badge tone={display.primary ? 'good' : 'neutral'}>{display.primary ? t('common.primary') : t('displays.external')}</Badge></div><div className="form-grid" style={{ marginTop: 17 }}><div className="row row-pad"><span className="muted">{t('displays.scaling')}</span><strong>{display.scale.toFixed(2)}×</strong></div><div className="row row-pad"><span className="muted">{t('nav.workspaces')}</span><strong>{count}</strong></div><div className="row row-pad"><span className="muted">{t('displays.currentDesktop')}</span><strong>{current ? `${current.desktop} · ID ${current.id}` : t('common.unavailable')}</strong></div><div className="row row-pad"><span className="muted">{t('displays.assigned')}</span><strong>{config.data?.config.workspaces.filter((ws) => ws.display === display.id || ws.display === null).length ?? '—'}</strong></div></div></Card> })}</div>
      <SectionHeader title={t('displays.moveTitle')} description={t('displays.moveHint')} /><Card className="padded stack">
        <div className="row row-pad"><div><strong>{t('displays.moveNext')}</strong><span className="row-sub">{all.length < 2 ? t('displays.connectAnother') : t('displays.shortcut', { chord: moveHotkey?.chord ?? t('displays.notAssigned'), state: moveHotkey && !moveHotkey.active ? t('displays.unavailableSuffix') : '' })}</span></div></div>
        {(Object.keys(MOVEMENT) as (keyof Config['window_movement'])[]).map((key) => <label key={key} className="row row-pad"><span><strong>{t(MOVEMENT[key][0])}</strong><span className="row-sub">{t(MOVEMENT[key][1])}</span></span><input type="checkbox" checked={movement[key]} disabled={patch.isPending} onChange={(event) => updateMovement({ ...movement, [key]: event.target.checked })} /></label>)}
        {patch.isError && <p className="bad-text">{patch.error.message}</p>}
      </Card>
      <SectionHeader title={t('displays.navigation')} description={t('displays.navigationHint')} /><Card className="padded"><div className="row row-pad"><div><strong>{t('displays.crossMonitor')}</strong><span className="row-sub">{t('displays.crossMonitorHint')}</span></div><Badge>{config.data?.config.cross_monitor_navigation ? t('common.enabled') : t('common.disabled')}</Badge></div><div className="row row-pad"><div><strong>{t('focus.followsMouse')}</strong><span className="row-sub">{t('displays.ffmHint')}</span></div><Badge tone="warn">{t('displays.unsupported')}</Badge></div><div className="row row-pad"><div><strong>{t('displays.disconnect')}</strong><span className="row-sub">{t('displays.disconnectHint')}</span></div><Badge>{t('displays.moveToPrimary')}</Badge></div></Card>
      <div style={{ marginTop: 18 }}><Notice>{t('displays.notice')}</Notice></div>
    </>}
  </>
}
