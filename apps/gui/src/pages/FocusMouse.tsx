import { useEffect, useState } from 'react'
import { Save, Plus, Trash2 } from 'lucide-react'
import { useConfigPatch, useDaemon } from '../api/hooks'
import type { FocusSettings } from '../api/types'
import { useT } from '../i18n'
import { Badge, Button, Card, ErrorState, Loading, Notice, Range } from '../shared/ui'

export function FocusMouse() {
  const t = useT()
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
  return <><div className="page-heading"><div><span className="eyebrow">{t('focus.eyebrow')}</span><h1>{t('nav.focus')}</h1><p>{t('focus.intro')}</p></div></div>
    {!status.isSuccess ? !status.isFetched ? <Loading /> : <ErrorState message={t('focus.offline')} retry={() => void status.refetch()} /> : config.isError ? <ErrorState message={config.error.message} /> : config.isPending || !draft ? <Loading /> : <>
      <div className="two-col even"><div><Card title={t('focus.pointer')} description={t('focus.pointerHint')} className="padded stack">
        <div className="row row-pad"><div><strong>{t('focus.followsMouse')}</strong><span className="row-sub">{t('focus.followsMouseHint')}</span></div><input aria-label={t('focus.followsMouse')} type="checkbox" checked={draft.follows_mouse} onChange={(e) => change({ follows_mouse: e.target.checked })} /></div>
        <div className="row row-pad"><div><strong>{t('focus.mode')}</strong><span className="row-sub">{t('focus.modeHint')}</span></div><select className="search-input" aria-label={t('focus.mode')} value={draft.mode} disabled={!draft.follows_mouse} onChange={(e) => change({ mode: e.target.value as FocusSettings['mode'] })}><option value="instant">{t('focus.instant')}</option><option value="delayed">{t('focus.delayed')}</option><option value="disabled">{t('focus.off')}</option></select></div>
        {draft.mode === 'delayed' && <div className="row row-pad"><div><label htmlFor="focus-delay" className="row-title">{t('focus.delay', { ms: draft.delay_ms })}</label></div><div className="range-row"><Range id="focus-delay" min="0" max="500" step="1" value={draft.delay_ms} disabled={!draft.follows_mouse} onChange={(e) => change({ delay_ms: Number(e.target.value) })} /><input aria-label={t('focus.delayValue')} type="number" min="0" max="500" value={draft.delay_ms} disabled={!draft.follows_mouse} onChange={(e) => change({ delay_ms: Number(e.target.value) })} /></div></div>}
        <div className="row row-pad"><div><strong>{t('focus.raise')}</strong><span className="row-sub">{t('focus.raiseHint')}</span></div><Badge>{t('focus.focusAndRaise')}</Badge></div>
      </Card></div><div>
      <Card title={t('focus.excluded')} description={t('focus.excludedHint')} className="padded stack">
        {draft.ignore_apps.map((app, index) => <div className="row row-pad" key={app.bundle_id}><div><strong>{app.bundle_id}</strong></div><div className="list-actions"><input aria-label={t('focus.enableExclusion', { app: app.bundle_id })} type="checkbox" checked={app.enabled} onChange={(e) => change({ ignore_apps: draft.ignore_apps.map((item, i) => i === index ? { ...item, enabled: e.target.checked } : item) })} /><Button variant="ghost" size="icon" aria-label={t('focus.remove', { app: app.bundle_id })} onClick={() => change({ ignore_apps: draft.ignore_apps.filter((_, i) => i !== index) })}><Trash2 size={13} /></Button></div></div>)}
        <div className="range-row" style={{ gridTemplateColumns: '1fr auto' }}><input className="search-input" aria-label={t('focus.bundleToExclude')} placeholder="com.example.Application" value={newBundle} onChange={(e) => setNewBundle(e.target.value)} /><Button onClick={add}><Plus size={13} /> {t('common.add')}</Button></div>
        <div className="form-actions"><Button onClick={() => setDraft(structuredClone(config.data.config.focus))} disabled={!changed}>{t('common.cancel')}</Button><Button variant="primary" onClick={apply} disabled={!changed || !valid || patch.isPending}><Save size={13} /> {patch.isPending ? t('common.applying') : t('focus.apply')}</Button></div>
        {!valid && <p className="bad-text">{t('focus.invalid')}</p>}
        {patch.isError && <p className="bad-text">{patch.error.message}</p>}{patch.isSuccess && <p className="good-text">{t('focus.confirmed')}</p>}
      </Card></div></div><Notice>{t('focus.notice')}</Notice>
    </>}
  </>
}
