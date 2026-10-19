import { useEffect, useState } from 'react'
import { Save, Plus, Trash2 } from 'lucide-react'
import { useConfigPatch, useDaemon } from '../api/hooks'
import type { FocusSettings } from '../api/types'
import { Button, Card, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

export function FocusMouse() {
  const status = useDaemon('status')
  const config = useDaemon('config', status.isSuccess)
  const patch = useConfigPatch()
  const [draft, setDraft] = useState<FocusSettings | null>(null)
  const [newBundle, setNewBundle] = useState('')
  useEffect(() => { if (config.data) setDraft(structuredClone(config.data.config.focus)) }, [config.data?.revision])
  const change = (next: Partial<FocusSettings>) => { patch.reset(); setDraft((current) => current && { ...current, ...next }) }
  const changed = !!draft && !!config.data && JSON.stringify(draft) !== JSON.stringify(config.data.config.focus)
  const valid = !!draft && Number.isInteger(draft.delay_ms) && draft.delay_ms >= 0 && draft.delay_ms <= 500
    && draft.ignore_apps.every((app) => app.bundle_id.trim())
  const apply = () => { if (draft && config.data && valid) patch.mutate({ revision: config.data.revision, patch: { kind: 'set_focus', focus: draft } }) }
  const add = () => {
    const bundle_id = newBundle.trim()
    if (!draft || !bundle_id || draft.ignore_apps.some((app) => app.bundle_id.toLowerCase() === bundle_id.toLowerCase())) return
    change({ ignore_apps: [...draft.ignore_apps, { bundle_id, enabled: true }] })
    setNewBundle('')
  }
  return <><div className="page-heading"><div><span className="eyebrow">Input</span><h1>Focus & Mouse</h1><p>Choose when moving the pointer activates a managed window.</p></div></div>
    {!status.isSuccess ? status.isPending ? <Loading /> : <ErrorState message="Connect to the daemon to edit focus settings." retry={() => void status.refetch()} /> : config.isError ? <ErrorState message={config.error.message} /> : config.isPending || !draft ? <Loading /> : <>
      <SectionHeader title="Pointer focus" description="Changes apply to the running daemon after you save." />
      <Card className="padded stack">
        <div className="row row-pad"><div><strong>Focus follows mouse</strong><span className="row-sub">Activate a managed window when the pointer enters it.</span></div><input aria-label="Focus follows mouse" type="checkbox" checked={draft.follows_mouse} onChange={(e) => change({ follows_mouse: e.target.checked })} /></div>
        <div className="row row-pad"><div><strong>Mode</strong><span className="row-sub">Instant sends a request immediately; Delayed waits for the pointer to settle.</span></div><select className="search-input" aria-label="Focus mode" value={draft.mode} disabled={!draft.follows_mouse} onChange={(e) => change({ mode: e.target.value as FocusSettings['mode'] })}><option value="instant">Instant</option><option value="delayed">Delayed</option><option value="disabled">Disabled</option></select></div>
        {draft.mode === 'delayed' && <div className="field"><label htmlFor="focus-delay">DELAY · {draft.delay_ms} MS</label><div className="range-row"><input id="focus-delay" type="range" min="0" max="500" step="1" value={draft.delay_ms} disabled={!draft.follows_mouse} onChange={(e) => change({ delay_ms: Number(e.target.value) })} /><input aria-label="Focus delay value" type="number" min="0" max="500" value={draft.delay_ms} disabled={!draft.follows_mouse} onChange={(e) => change({ delay_ms: Number(e.target.value) })} /></div></div>}
        <div className="row row-pad"><div><strong>Raise behavior</strong><span className="row-sub">macOS activates and raises the selected window. Focus without raise is unavailable.</span></div><strong>Focus and raise</strong></div>
      </Card>
      <SectionHeader title="Excluded applications" description="Bundle IDs here never receive pointer focus. Floating windows remain eligible unless excluded." />
      <Card className="padded stack">
        {draft.ignore_apps.map((app, index) => <div className="row row-pad" key={app.bundle_id}><div><strong>{app.bundle_id}</strong></div><div className="range-row"><input aria-label={`Enable exclusion ${app.bundle_id}`} type="checkbox" checked={app.enabled} onChange={(e) => change({ ignore_apps: draft.ignore_apps.map((item, i) => i === index ? { ...item, enabled: e.target.checked } : item) })} /><Button aria-label={`Remove ${app.bundle_id}`} onClick={() => change({ ignore_apps: draft.ignore_apps.filter((_, i) => i !== index) })}><Trash2 size={13} /></Button></div></div>)}
        <div className="range-row"><input className="search-input" aria-label="Bundle ID to exclude" placeholder="com.example.Application" value={newBundle} onChange={(e) => setNewBundle(e.target.value)} /><Button onClick={add}><Plus size={13} /> Add</Button></div>
        <div className="form-actions"><Button onClick={() => setDraft(structuredClone(config.data.config.focus))} disabled={!changed}>Cancel</Button><Button variant="primary" onClick={apply} disabled={!changed || !valid || patch.isPending}><Save size={13} /> {patch.isPending ? 'Applying…' : 'Apply focus settings'}</Button></div>
        {!valid && <p className="bad-text">Delay must be 0–500 ms and bundle IDs cannot be empty.</p>}
        {patch.isError && <p className="bad-text">{patch.error.message}</p>}{patch.isSuccess && <p className="good-text">Daemon confirmed focus settings.</p>}
      </Card><div style={{ marginTop: 18 }}><Notice>Focus follows mouse works only for windows currently managed by Tylora and visible in the active macOS Desktop.</Notice></div>
    </>}
  </>
}
