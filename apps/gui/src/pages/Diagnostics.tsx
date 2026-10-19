import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useQuery } from '@tanstack/react-query'
import { ClipboardCopy, Download, RefreshCcw } from 'lucide-react'
import { useDaemon } from '../api/hooks'
import { Badge, Button, Card, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

export function Diagnostics() {
  const status = useDaemon('status')
  const connected = status.isSuccess
  const diag = useDaemon('diagnostics', connected)
  const capabilities = useDaemon('capabilities', connected)
  const hotkeys = useDaemon('hotkeys', connected)
  const macos = useQuery({ queryKey: ['macos-version'], queryFn: () => invoke<string>('macos_version'), staleTime: Infinity })
  const [message, setMessage] = useState('')
  const report = () => ({
    version: 1, macos_version: macos.data ?? null,
    status: status.data && { state: status.data.state, displays: status.data.displays, windows: status.data.windows, tiled: status.data.tiled, floating: status.data.floating, degraded: status.data.degraded },
    diagnostics: diag.data ?? null, capabilities: capabilities.data?.capabilities ?? null,
    hotkeys: hotkeys.data?.hotkeys.map((hotkey) => ({ chord: hotkey.chord, enabled: hotkey.enabled, active: hotkey.active, action: hotkey.command.command })) ?? [],
  })
  const refresh = () => { void status.refetch(); void diag.refetch(); void capabilities.refetch(); void hotkeys.refetch() }
  const copy = async () => { try { await navigator.clipboard.writeText(JSON.stringify(report(), null, 2)); setMessage('Diagnostics copied without window titles or private paths.') } catch { setMessage('Clipboard access was denied.') } }
  const exportReport = async () => { try { const path = await invoke<string>('export_diagnostics'); setMessage(`Saved report to ${path}`) } catch (error) { setMessage(`Export failed: ${String(error)}`) } }
  return <><div className="page-heading"><div><span className="eyebrow">Health</span><h1>Diagnostics</h1><p>Connection and backend details from the running daemon.</p></div><div className="page-actions"><Button onClick={refresh}><RefreshCcw size={13} /> Refresh</Button><Button onClick={() => void copy()} disabled={!connected}><ClipboardCopy size={13} /> Copy</Button><Button variant="primary" onClick={() => void exportReport()} disabled={!connected}><Download size={13} /> Export</Button></div></div>
    {message && <div style={{ marginBottom: 14 }}><Notice>{message}</Notice></div>}
    {!connected ? status.isPending ? <Loading /> : <ErrorState message="Daemon IPC is disconnected. Start the daemon from Dashboard." retry={() => void status.refetch()} /> : diag.isPending ? <Loading /> : diag.isError ? <ErrorState message={diag.error.message} retry={() => void diag.refetch()} /> : <>
      <div className="metric-grid"><Card className="metric"><span className="metric-label">IPC</span><strong className="good-text">Connected</strong><small>Unix socket · protocol v1</small></Card><Card className="metric"><span className="metric-label">macOS</span><strong>{macos.data ?? 'Unknown'}</strong><small>System version</small></Card><Card className="metric"><span className="metric-label">Stale results</span><strong>{diag.data.stale_results}</strong><small>Coordinator rejected</small></Card><Card className="metric"><span className="metric-label">Degraded frames</span><strong>{diag.data.degraded}</strong><small>AX geometry mismatch</small></Card></div>
      <div className="two-col"><div><SectionHeader title="Backend capabilities" description="Reported by the current process and macOS build" /><Card className="padded">
        {Object.entries(capabilities.data?.capabilities ?? {}).filter(([, value]) => typeof value === 'boolean').map(([key, value]) => <div key={key} className="row row-pad"><span>{key.replaceAll('_', ' ')}</span><Badge tone={value ? 'good' : 'warn'}>{value ? 'Available' : 'Unavailable'}</Badge></div>)}
        {!capabilities.data && <span className="muted">Capabilities unavailable.</span>}
      </Card></div><div><SectionHeader title="Observability" description="Current counters" /><Card className="padded"><div className="row row-pad"><span>Accessibility permission</span><Badge tone={diag.data.accessibility_trusted ? 'good' : 'bad'}>{diag.data.accessibility_trusted ? 'Granted' : 'Denied'}</Badge></div><div className="row row-pad"><span>Native Spaces</span><strong>{diag.data.native_spaces}</strong></div><div className="row row-pad"><span>Managed windows</span><strong>{diag.data.managed_windows}</strong></div><div className="row row-pad"><span>Registered hotkeys</span><strong>{hotkeys.data?.hotkeys.filter((hotkey) => hotkey.active).length ?? '—'}</strong></div></Card></div></div>
      {diag.data.focus_follows_mouse && <><SectionHeader title="Pointer focus" description="Measured since daemon start" /><Card className="padded">{([
        ['Focus requests', diag.data.focus_follows_mouse.requests],
        ['Applied', diag.data.focus_follows_mouse.applied ?? '—'],
        ['Confirmed', diag.data.focus_follows_mouse.confirmed],
        ['Cancelled delays', diag.data.focus_follows_mouse.cancelled_delayed],
        ['Failures', diag.data.focus_follows_mouse.failures],
        ['Average hit test', diag.data.focus_follows_mouse.hit_tests ? `${Math.round(diag.data.focus_follows_mouse.hit_test_total_us / diag.data.focus_follows_mouse.hit_tests)} µs` : '—'],
        ['Average mouse → request', diag.data.focus_follows_mouse.requests ? `${Math.round(diag.data.focus_follows_mouse.mouse_to_request_total_us / diag.data.focus_follows_mouse.requests)} µs` : '—'],
        ['Average request → applied', diag.data.focus_follows_mouse.applied ? `${Math.round((diag.data.focus_follows_mouse.request_to_applied_total_us ?? 0) / diag.data.focus_follows_mouse.applied)} µs` : '—'],
        ['Average request → confirmed', diag.data.focus_follows_mouse.confirmed ? `${Math.round(diag.data.focus_follows_mouse.request_to_confirm_total_us / diag.data.focus_follows_mouse.confirmed)} µs` : '—'],
      ] as const).map(([name, value]) => <div className="row row-pad" key={name}><span>{name}</span><strong>{value}</strong></div>)}</Card></>}
      <SectionHeader title="Operation errors" description="The daemon currently exposes counters, not a structured recent-error stream." /><Notice tone={diag.data.degraded ? 'warn' : 'info'}>{diag.data.degraded ? `${diag.data.degraded} windows have a failed geometry target. Inspect the daemon log locally for AX error codes.` : 'No degraded geometry targets were reported. Detailed recent errors remain available in the daemon log.'}</Notice>
      <div style={{ marginTop: 14 }}><Notice>Export excludes window titles, application content, PIDs, and private file paths.</Notice></div>
    </>}
  </>
}
