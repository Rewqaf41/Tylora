import { ArrowRight, CircleCheck, Play, RotateCcw, ShieldCheck, Square, Wrench } from 'lucide-react'
import { useAction, useDaemon } from '../api/hooks'
import { Badge, Button, Card, ConfirmButton, ErrorState, Loading, Metric, Notice, SectionHeader } from '../shared/ui'

export function Dashboard() {
  const status = useDaemon('status')
  const connected = status.isSuccess
  const config = useDaemon('config', connected)
  const capabilities = useDaemon('capabilities', connected)
  const action = useAction()
  const activeSpace = status.data?.native_spaces.find((space) => space.active)
  const currentWorkspace = config.data?.config.workspaces.find((ws) => ws.native_space_id === activeSpace?.id)
  const run = (value: Parameters<typeof action.mutate>[0]) => action.mutate(value)

  return <>
    <div className="page-heading"><div><span className="eyebrow">Overview</span><h1>Good to see you.</h1><p>A live view of your window manager and native macOS Spaces.</p></div>
      <div className="page-actions"><Button onClick={() => void status.refetch()}><RotateCcw size={13} /> Refresh</Button></div></div>
    {status.isPending ? <Loading /> : status.isError ? <>
      <Notice tone="warn">The daemon is not connected. Start it to see live windows and Spaces.</Notice>
      <SectionHeader title="Daemon control" description="The settings app can stay open while the daemon is stopped." />
      <Card className="padded row"><div><strong>Tylora is stopped</strong><span className="row-sub">No window state is available.</span></div><Button variant="primary" disabled={action.isPending} onClick={() => run({ kind: 'control', op: 'start' })}><Play size={14} /> Start daemon</Button></Card>
      {action.isError && <p className="bad-text">{action.error.message}</p>}
      <div style={{ marginTop: 18 }}><ErrorState message={status.error.message} retry={() => void status.refetch()} /></div>
    </> : <>
      <div className="metric-grid">
        <Metric label="Daemon" value={<span className="good-text">Running</span>} detail="Connected via local IPC" />
        <Metric label="Current workspace" value={currentWorkspace?.name ?? 'Unbound'} detail={activeSpace ? `Native Space ${activeSpace.id}` : 'No active Space observed'} />
        <Metric label="Managed windows" value={status.data.windows} detail={`${status.data.tiled} tiled · ${status.data.floating} floating`} />
        <Metric label="Displays" value={status.data.displays} detail={`Primary display ${status.data.primary_display}`} />
      </div>
      <div className="two-col">
        <div><SectionHeader title="System status" description="Capabilities reported by the running daemon." />
          <Card className="padded">
            <div className="row row-pad"><div><span className="row-title">Accessibility</span><span className="row-sub">Required for observing and focusing windows</span></div><Badge tone={status.data.accessibility_trusted ? 'good' : 'bad'}>{status.data.accessibility_trusted ? 'Granted' : 'Permission needed'}</Badge></div>
            <div className="row row-pad"><div><span className="row-title">Native Spaces</span><span className="row-sub">{status.data.native_spaces.length} discovered across displays</span></div><Badge tone={status.data.native_spaces.length ? 'good' : 'warn'}>{status.data.native_spaces.length ? 'Available' : 'Unavailable'}</Badge></div>
            <div className="row row-pad"><div><span className="row-title">Space movement</span><span className="row-sub">Private backend capability reported at runtime</span></div><Badge tone={capabilities.data?.capabilities.can_move_window_between_spaces ? 'good' : 'warn'}>{capabilities.data?.capabilities.can_move_window_between_spaces ? 'Available' : 'Unavailable'}</Badge></div>
            <div className="row row-pad"><div><span className="row-title">Layout engine</span><span className="row-sub">Active layout on this session</span></div><Badge>{status.data.layout.toUpperCase()}</Badge></div>
          </Card>
        </div>
        <div><SectionHeader title="Quick actions" description="Actions are confirmed by the daemon." />
          <Card className="padded stack">
            <Button onClick={() => run({ kind: 'simple', action: 'reconcile' })} disabled={action.isPending}><RotateCcw size={14} /> Reconcile windows</Button>
            <Button onClick={() => run({ kind: 'simple', action: 'recover_windows' })} disabled={action.isPending}><Wrench size={14} /> Recover windows</Button>
            <ConfirmButton variant="secondary" prompt="Restart the Tylora daemon? Windows may briefly stop tiling." disabled={action.isPending} onConfirm={() => run({ kind: 'control', op: 'restart' })}><ArrowRight size={14} /> Restart daemon</ConfirmButton>
            <ConfirmButton prompt="Stop Tylora? Window tiling and hotkeys will stop." disabled={action.isPending} onConfirm={() => run({ kind: 'control', op: 'stop' })}><Square size={13} /> Stop Tylora</ConfirmButton>
          </Card>
          {action.isError && <p className="bad-text">{action.error.message}</p>}
          {action.isSuccess && <p className="good-text"><CircleCheck size={13} style={{ display: 'inline' }} /> Action confirmed.</p>}
        </div>
      </div>
      <SectionHeader title="Current session" description="Current native Desktop and logical binding are separate identities." />
      <Card className="padded row"><div><span className="row-title">{activeSpace ? `Desktop ${activeSpace.desktop}` : 'No active Desktop'}</span><span className="row-sub">{activeSpace ? `Space ID ${activeSpace.id} · Display ${activeSpace.display}` : 'Native Space observation is unavailable'}</span></div><ShieldCheck size={20} className="muted" /></Card>
      {status.data.degraded > 0 && <div style={{ marginTop: 14 }}><Notice tone="warn">{status.data.degraded} window geometry update{status.data.degraded === 1 ? '' : 's'} could not reach the requested frame. See Diagnostics.</Notice></div>}
    </>}
  </>
}
