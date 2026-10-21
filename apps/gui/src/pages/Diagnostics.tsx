import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useQuery } from '@tanstack/react-query'
import { Cable, ClipboardCopy, Clock, Download, Laptop, RefreshCcw, TriangleAlert } from 'lucide-react'
import { useDaemon } from '../api/hooks'
import { useT } from '../i18n'
import { Badge, Button, Card, ErrorState, Loading, Metric, Notice } from '../shared/ui'

export function Diagnostics() {
  const t = useT()
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
  const copy = async () => { try { await navigator.clipboard.writeText(JSON.stringify(report(), null, 2)); setMessage(t('diag.copied')) } catch { setMessage(t('diag.clipboardDenied')) } }
  const exportReport = async () => { try { const path = await invoke<string>('export_diagnostics'); setMessage(t('diag.saved', { path })) } catch (error) { setMessage(t('diag.exportFailed', { error: String(error) })) } }
  return <><div className="page-heading"><div><span className="eyebrow">{t('diag.eyebrow')}</span><h1>{t('nav.diagnostics')}</h1><p>{t('diag.intro')}</p></div><div className="page-actions"><Button onClick={refresh}><RefreshCcw size={13} /> {t('common.refresh')}</Button><Button onClick={() => void copy()} disabled={!connected}><ClipboardCopy size={13} /> {t('diag.copy')}</Button><Button variant="primary" onClick={() => void exportReport()} disabled={!connected}><Download size={13} /> {t('diag.export')}</Button></div></div>
    {message && <Notice>{message}</Notice>}
    {!connected ? status.isPending ? <Loading /> : <ErrorState message={t('diag.offline')} retry={() => void status.refetch()} /> : diag.isPending ? <Loading /> : diag.isError ? <ErrorState message={diag.error.message} retry={() => void diag.refetch()} /> : <>
      <div className="metric-grid"><Metric icon={Cable} label="IPC" value={t('app.connected')} detail={t('diag.socket')} tone="good" /><Metric icon={Laptop} label="macOS" value={macos.data ?? t('diag.unknown')} detail={t('diag.systemVersion')} /><Metric icon={Clock} label={t('diag.stale')} value={diag.data.stale_results} detail={t('diag.staleHint')} /><Metric icon={TriangleAlert} label={t('diag.degraded')} value={diag.data.degraded} detail={t('diag.degradedHint')} /></div>
      <div className="two-col even"><div><Card title={t('diag.capabilities')} description={t('diag.capabilitiesHint')} className="padded">
        {Object.entries(capabilities.data?.capabilities ?? {}).filter(([, value]) => typeof value === 'boolean').map(([key, value]) => <div key={key} className="row row-pad"><span>{key.replaceAll('_', ' ')}</span><Badge tone={value ? 'good' : 'bad'}>{value ? t('common.available') : t('common.unavailable')}</Badge></div>)}
        {!capabilities.data && <span className="muted">{t('diag.noCapabilities')}</span>}
      </Card></div><div><Card title={t('diag.observability')} description={t('diag.counters')} className="padded"><div className="row row-pad"><span>{t('diag.axPermission')}</span><Badge tone={diag.data.accessibility_trusted ? 'good' : 'bad'}>{diag.data.accessibility_trusted ? t('common.granted') : t('diag.denied')}</Badge></div><div className="row row-pad"><span>{t('dashboard.nativeSpaces')}</span><strong>{diag.data.native_spaces}</strong></div><div className="row row-pad"><span>{t('dashboard.managedWindows')}</span><strong>{diag.data.managed_windows}</strong></div><div className="row row-pad"><span>{t('diag.registeredHotkeys')}</span><strong>{hotkeys.data?.hotkeys.filter((hotkey) => hotkey.active).length ?? '—'}</strong></div></Card></div></div>
      {diag.data.focus_follows_mouse && <><Card title={t('focus.pointer')} description={t('diag.sinceStart')} className="padded">{([
        [t('diag.ffmRequests'), diag.data.focus_follows_mouse.requests],
        [t('diag.ffmApplied'), diag.data.focus_follows_mouse.applied ?? '—'],
        [t('diag.ffmConfirmed'), diag.data.focus_follows_mouse.confirmed],
        [t('diag.ffmCancelled'), diag.data.focus_follows_mouse.cancelled_delayed],
        [t('diag.ffmFailures'), diag.data.focus_follows_mouse.failures],
        [t('diag.ffmHitTest'), diag.data.focus_follows_mouse.hit_tests ? `${Math.round(diag.data.focus_follows_mouse.hit_test_total_us / diag.data.focus_follows_mouse.hit_tests)} µs` : '—'],
        [t('diag.ffmMouseRequest'), diag.data.focus_follows_mouse.requests ? `${Math.round(diag.data.focus_follows_mouse.mouse_to_request_total_us / diag.data.focus_follows_mouse.requests)} µs` : '—'],
        [t('diag.ffmRequestApplied'), diag.data.focus_follows_mouse.applied ? `${Math.round((diag.data.focus_follows_mouse.request_to_applied_total_us ?? 0) / diag.data.focus_follows_mouse.applied)} µs` : '—'],
        [t('diag.ffmRequestConfirmed'), diag.data.focus_follows_mouse.confirmed ? `${Math.round(diag.data.focus_follows_mouse.request_to_confirm_total_us / diag.data.focus_follows_mouse.confirmed)} µs` : '—'],
      ] as const).map(([name, value]) => <div className="row row-pad" key={name}><span>{name}</span><strong>{value}</strong></div>)}</Card></>}
      <Notice tone={diag.data.degraded ? 'warn' : 'info'}>{diag.data.degraded ? t('diag.degradedWindows', { count: diag.data.degraded }) : t('diag.noDegraded')}</Notice>
      <Notice>{t('diag.exportNotice')}</Notice>
    </>}
  </>
}
