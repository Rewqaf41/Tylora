import { useConfigPatch, useDaemon } from '../api/hooks'
import type { Config } from '../api/types'
import { Badge, Card, Empty, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

export function Displays() {
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
  return <><div className="page-heading"><div><span className="eyebrow">Hardware</span><h1>Displays</h1><p>Live geometry and workspace assignments for connected monitors.</p></div></div>
    {!online ? status.isPending ? <Loading /> : <ErrorState message="Start the daemon to inspect displays." retry={() => void status.refetch()} /> : displays.isPending || spaces.isPending ? <Loading /> : displays.isError || spaces.isError ? <ErrorState message={displays.error?.message ?? spaces.error?.message ?? 'Unknown error'} /> : all.length === 0 ? <Empty title="No displays found">Reconnect your monitor or refresh the daemon.</Empty> : <>
      <SectionHeader title="Arrangement" description="Positions and sizes reported by Core Graphics" />
      <Card className="padded"><div className="monitor-map" style={{ height: 230 }}>{all.map((display) => <div key={display.id} className="monitor-visual" style={{ position: 'absolute', left: `${8 + (display.full_frame.x - minX) / totalW * 84}%`, top: `${8 + (display.full_frame.y - minY) / totalH * 84}%`, width: `${Math.max(15, display.full_frame.width / totalW * 84)}%`, height: `${Math.max(25, display.full_frame.height / totalH * 84)}%` }}>{display.primary ? 'Primary' : `Display ${display.id}`}</div>)}</div></Card>
      <SectionHeader title="Connected displays" description={`${all.length} currently reported`} />
      <div className="stack">{all.map((display) => { const current = spaces.data?.spaces.find((space) => space.display === display.id && space.active); const count = spaces.data?.spaces.filter((space) => space.display === display.id).length ?? 0; return <Card key={display.id} className="padded"><div className="row"><div><strong>{display.primary ? 'Primary display' : `Display ${display.id}`}</strong><span className="row-sub">CGDirectDisplayID {display.id} · {display.full_frame.width} × {display.full_frame.height} pt</span></div><Badge tone={display.primary ? 'good' : 'neutral'}>{display.primary ? 'Primary' : 'External'}</Badge></div><div className="form-grid" style={{ marginTop: 17 }}><div className="row row-pad"><span className="muted">Scaling</span><strong>{display.scale.toFixed(2)}×</strong></div><div className="row row-pad"><span className="muted">Workspaces</span><strong>{count}</strong></div><div className="row row-pad"><span className="muted">Current Desktop</span><strong>{current ? `${current.desktop} · ID ${current.id}` : 'Unavailable'}</strong></div><div className="row row-pad"><span className="muted">Assigned logical workspaces</span><strong>{config.data?.config.workspaces.filter((ws) => ws.display === display.id || ws.display === null).length ?? '—'}</strong></div></div></Card> })}</div>
      <SectionHeader title="Move windows between displays" description="Native Space transfer follows physical left-to-right display order" /><Card className="padded stack">
        <div className="row row-pad"><div><strong>Move to next display</strong><span className="row-sub">{all.length < 2 ? 'Connect another display to enable window transfer.' : `Shortcut: ${moveHotkey?.chord ?? 'Not assigned'}${moveHotkey && !moveHotkey.active ? ' · unavailable' : ''}. Change it in Hotkeys.`}</span></div></div>
        {(['focus_after_move', 'wrap_around', 'preserve_floating_position'] as const).map((key) => <label key={key} className="row row-pad"><span><strong>{key === 'focus_after_move' ? 'Focus moved window' : key === 'wrap_around' ? 'Cycle through displays' : 'Preserve floating position'}</strong><span className="row-sub">{key === 'focus_after_move' ? 'Activate the window after it appears on the target display.' : key === 'wrap_around' ? 'Continue from the last display to the first.' : 'Keep its relative position inside the target work area.'}</span></span><input type="checkbox" checked={movement[key]} disabled={patch.isPending} onChange={(event) => updateMovement({ ...movement, [key]: event.target.checked })} /></label>)}
        {patch.isError && <p className="bad-text">{patch.error.message}</p>}
      </Card>
      <SectionHeader title="Navigation behavior" description="Current daemon configuration" /><Card className="padded"><div className="row row-pad"><div><strong>Cross-monitor navigation</strong><span className="row-sub">Move focus across display edges</span></div><Badge>{config.data?.config.cross_monitor_navigation ? 'Enabled' : 'Disabled'}</Badge></div><div className="row row-pad"><div><strong>Focus follows mouse</strong><span className="row-sub">The setting exists in TOML but the daemon does not currently enforce it.</span></div><Badge tone="warn">Unsupported</Badge></div><div className="row row-pad"><div><strong>Disconnect behavior</strong><span className="row-sub">Windows are rehomed to the primary display when possible.</span></div><Badge>Move to primary</Badge></div></Card>
      <div style={{ marginTop: 18 }}><Notice>Tylora reads monitor geometry but does not change macOS display arrangement or resolution.</Notice></div>
    </>}
  </>
}
