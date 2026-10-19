import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Crosshair, Eye, EyeOff, LifeBuoy, Plus, Repeat, Trash2, Unlink } from 'lucide-react'
import { assignScratchpad, recoverScratchpads, scratchpadAction, unassignScratchpad } from '../api/bridge'
import { useConfigPatch, useDaemon } from '../api/hooks'
import type { ConfigPatch, Rect, ScratchpadAlignment, ScratchpadDefinition, ScratchpadState, ScratchpadStatus } from '../api/types'
import { shortcutWarning } from '../shared/hotkey'
import { HotkeyRecorder } from '../shared/HotkeyRecorder'
import { tEn, useT, type MessageKey, type T } from '../i18n'
import { Badge, Button, Card, Empty, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

const ALIGNMENTS: ScratchpadAlignment[] = ['top_left', 'top', 'top_right', 'left', 'center', 'right', 'bottom_left', 'bottom', 'bottom_right']
const ALIGNMENT_LABELS: Record<ScratchpadAlignment, MessageKey> = {
  top_left: 'pads.align.topLeft', top: 'pads.align.top', top_right: 'pads.align.topRight',
  left: 'pads.align.left', center: 'pads.align.center', right: 'pads.align.right',
  bottom_left: 'pads.align.bottomLeft', bottom: 'pads.align.bottom', bottom_right: 'pads.align.bottomRight',
}
const STATE_TONES: Record<ScratchpadState, 'neutral' | 'good' | 'warn' | 'bad'> = { unassigned: 'neutral', hidden: 'neutral', showing: 'warn', visible: 'good', hiding: 'warn', unavailable: 'bad' }
const STATE_LABELS: Record<ScratchpadState, MessageKey> = { unassigned: 'pads.state.unassigned', hidden: 'pads.state.hidden', showing: 'pads.state.showing', visible: 'pads.state.visible', hiding: 'pads.state.hiding', unavailable: 'common.unavailable' }

export const blankScratchpad: ScratchpadDefinition = {
  id: '', name: '', hotkey: null, scope: 'global', width: 0.7, height: 0.65, alignment: 'center', offset_x: 0, offset_y: 0,
  target: 'pointer', display: null, remember_position: true, remember_size: true, hide: 'park', bundle_id: null,
}

export function scratchpadSlug(text: string): string {
  return text.toLowerCase().normalize('NFKD').replace(/[^a-z0-9_]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 32)
}

/** Client-side checks mirroring the daemon's validation. The daemon has the final say. */
export function scratchpadProblem(pad: ScratchpadDefinition, others: ScratchpadDefinition[], takenChords: string[], t: T = tEn): string | null {
  const id = pad.id.trim()
  if (!/^[A-Za-z0-9_-]{1,32}$/.test(id)) return t('pads.err.id')
  if (others.some((other) => other.id.toLowerCase() === id.toLowerCase())) return t('pads.err.duplicate', { id })
  const fraction = (value: number) => Number.isFinite(value) && value >= 0.1 && value <= 1
  if (!fraction(pad.width) || !fraction(pad.height)) return t('pads.err.size')
  if (![pad.offset_x, pad.offset_y].every((value) => Number.isFinite(value) && Math.abs(value) <= 4000)) return t('pads.err.offset')
  if (pad.hotkey && takenChords.some((chord) => chord.toLowerCase() === pad.hotkey!.toLowerCase())) return t('pads.err.hotkey', { chord: pad.hotkey })
  return null
}

/** Same placement as the daemon's `scratchpad_frame` (without a remembered frame). */
export function previewFrame(pad: ScratchpadDefinition, area: Rect): Rect {
  const width = Math.max(1, Math.min(area.width, Math.max(area.width * pad.width, Math.min(160, area.width))))
  const height = Math.max(1, Math.min(area.height, Math.max(area.height * pad.height, Math.min(120, area.height))))
  const freeX = area.width - width
  const freeY = area.height - height
  const fx = pad.alignment.endsWith('left') ? 0 : pad.alignment.endsWith('right') ? 1 : 0.5
  const fy = pad.alignment.startsWith('top') ? 0 : pad.alignment.startsWith('bottom') ? 1 : 0.5
  const clamp = (value: number, low: number, high: number) => Math.min(Math.max(value, low), high)
  return {
    x: clamp(area.x + freeX * fx + pad.offset_x, area.x, area.x + Math.max(0, freeX)),
    y: clamp(area.y + freeY * fy + pad.offset_y, area.y, area.y + Math.max(0, freeY)),
    width, height,
  }
}

function GeometryPreview({ pad, area }: { pad: ScratchpadDefinition; area: Rect }) {
  const t = useT()
  const frame = previewFrame(pad, area)
  const scale = 260 / area.width
  return <svg role="img" aria-label={t('pads.previewLabel')} width={260} height={Math.round(area.height * scale)} style={{ border: '1px solid var(--border)', borderRadius: 6, background: 'var(--surface-2, transparent)' }}>
    <rect x={(frame.x - area.x) * scale} y={(frame.y - area.y) * scale} width={frame.width * scale} height={frame.height * scale} rx={4} fill="var(--accent, #5b8def)" fillOpacity={0.3} stroke="var(--accent, #5b8def)" />
    <text x={6} y={14} fontSize={10} fill="currentColor" opacity={0.6}>{Math.round(frame.width)}×{Math.round(frame.height)} pt</text>
  </svg>
}

function windowLabel(t: T, status: ScratchpadStatus): string {
  const window = status.window
  if (!window) return t('pads.noWindowAssigned')
  return `${window.bundle_id ?? `pid ${window.process.pid}`} — ${window.title || t('common.untitled')} · CG ${window.cg_window_id}${status.manual ? '' : ` · ${t('pads.byRule')}`}`
}

export function Scratchpads() {
  const t = useT()
  const status = useDaemon('status')
  const online = status.isSuccess
  const config = useDaemon('config', online)
  const live = useDaemon('scratchpads', online)
  const capabilities = useDaemon('scratchpad_capabilities', online)
  const windows = useDaemon('windows', online)
  const apps = useDaemon('apps', online)
  const displays = useDaemon('displays', online)
  const patch = useConfigPatch()
  const client = useQueryClient()
  const refresh = async () => { await client.invalidateQueries({ queryKey: ['daemon'] }) }
  const action = useMutation({ mutationFn: ({ id, verb }: { id: string; verb: 'toggle' | 'show' | 'hide' }) => scratchpadAction(id, verb), onSettled: refresh })
  const assign = useMutation({ mutationFn: ({ id, window, force }: { id: string; window: number | null; force: boolean }) => assignScratchpad(id, window, force), onSettled: refresh })
  const unassign = useMutation({ mutationFn: unassignScratchpad, onSettled: refresh })
  const recover = useMutation({ mutationFn: recoverScratchpads, onSettled: refresh })
  const [editing, setEditing] = useState<string | null | undefined>(undefined)
  const [draft, setDraft] = useState<ScratchpadDefinition>(blankScratchpad)
  const [picked, setPicked] = useState<Record<string, number>>({})

  const pads = config.data?.config.scratchpads ?? []
  const states = new Map((live.data?.scratchpads ?? []).map((entry) => [entry.id, entry]))
  const others = pads.filter((pad) => pad.id !== editing)
  const takenChords = [...(config.data?.config.hotkeys ?? []).filter((hotkey) => hotkey.enabled).map((hotkey) => hotkey.chord), ...others.flatMap((pad) => pad.hotkey ? [pad.hotkey] : [])]
  const problem = editing !== undefined ? scratchpadProblem(draft, others, takenChords, t) : null
  const primary = displays.data?.displays.find((display) => display.id === draft.display) ?? displays.data?.displays.find((display) => display.primary) ?? displays.data?.displays[0]
  const assignable = (windows.data?.windows ?? []).filter((window) => window.cg_window_id !== null && window.mode !== 'unmanaged')
  const send = (next: ConfigPatch, done?: () => void) => { if (config.data) patch.mutate({ revision: config.data.revision, patch: next }, { onSuccess: done }) }
  const open = (pad: ScratchpadDefinition | null) => {
    setDraft(pad ? { ...blankScratchpad, ...pad, hotkey: pad.hotkey ?? null, display: pad.display ?? null, bundle_id: pad.bundle_id ?? null } : blankScratchpad)
    setEditing(pad ? pad.id : null); patch.reset()
  }
  const save = () => {
    if (problem) return
    const scratchpad: ScratchpadDefinition = { ...draft, id: draft.id.trim(), name: draft.name.trim() }
    send({ kind: 'set_scratchpad', previous_id: editing ?? null, scratchpad }, () => setEditing(undefined))
  }
  const error = [action, assign, unassign, recover].find((mutation) => mutation.isError)?.error?.message

  return <><div className="page-heading"><div><span className="eyebrow">{t('rules.eyebrow')}</span><h1>{t('nav.scratchpads')}</h1><p>{t('pads.intro')}</p></div>
    <div className="page-actions"><Button disabled={!online || recover.isPending} onClick={() => recover.mutate()}><LifeBuoy size={13} /> {t('dashboard.recover')}</Button><Button variant="primary" disabled={!online || !config.data} onClick={() => open(null)}><Plus size={14} /> {t('pads.new')}</Button></div></div>
    {!online ? status.isPending ? <Loading /> : <ErrorState message={t('pads.offline')} retry={() => void status.refetch()} /> : config.isPending ? <Loading /> : config.isError ? <ErrorState message={config.error.message} retry={() => void config.refetch()} /> : <>
      <SectionHeader title={t('nav.scratchpads')} description={t('pads.listHint')} />
      {pads.length === 0 ? <Empty title={t('pads.none')}>{t('pads.noneHint')}</Empty> : <Card>{pads.map((pad) => {
        const entry = states.get(pad.id)
        const state = entry?.state ?? 'unassigned'
        const busy = state === 'showing' || state === 'hiding' || action.isPending
        return <div className="list-item" key={pad.id} style={{ flexWrap: 'wrap', gap: 8 }}>
          <div style={{ minWidth: 240 }}><div className="list-primary">{pad.name || pad.id} <span className="muted mono">{pad.id}</span> <Badge tone={STATE_TONES[state]}>{t(STATE_LABELS[state])}</Badge></div>
            <div className="list-secondary">{pad.hotkey ? <span className="kbd">{pad.hotkey}</span> : t('pads.noHotkey')} · {pad.scope === 'global' ? t('pads.global') : t('pads.local')} · {Math.round(pad.width * 100)}×{Math.round(pad.height * 100)}% · {pad.hide === 'minimize' ? t('pads.hidesMinimize') : t('pads.hidesPark')}</div>
            <div className="list-secondary">{entry ? (entry.window || !pad.bundle_id ? windowLabel(t, entry) : t('pads.noWindowYet', { app: pad.bundle_id })) : t('pads.notLoaded')}{entry?.last_error ? <span className="bad-text"> · {entry.last_error}</span> : null}</div></div>
          <div className="list-actions">
            <Button disabled={(!entry?.window && !pad.bundle_id) || busy} title={!entry?.window && pad.bundle_id ? t('pads.toggleAdoptHint', { app: pad.bundle_id }) : undefined} onClick={() => action.mutate({ id: pad.id, verb: 'toggle' })}><Repeat size={13} /> {t('pads.toggle')}</Button>
            <Button variant="ghost" aria-label={t('pads.show', { id: pad.id })} disabled={!entry?.window || busy} onClick={() => action.mutate({ id: pad.id, verb: 'show' })}><Eye size={13} /></Button>
            <Button variant="ghost" aria-label={t('pads.hide', { id: pad.id })} disabled={!entry?.window || busy} onClick={() => action.mutate({ id: pad.id, verb: 'hide' })}><EyeOff size={13} /></Button>
            <Button disabled={assign.isPending} title={t('pads.assignFocusedHint')} onClick={() => assign.mutate({ id: pad.id, window: null, force: true })}><Crosshair size={13} /> {t('pads.assignFocused')}</Button>
            <select aria-label={t('pads.windowFor', { id: pad.id })} value={picked[pad.id] ?? ''} onChange={(event) => setPicked({ ...picked, [pad.id]: Number(event.target.value) })} style={{ maxWidth: 220 }}>
              <option value="">{t('pads.chooseWindow')}</option>
              {assignable.filter((window) => !pad.bundle_id || window.application_id === pad.bundle_id).map((window) => <option key={window.cg_window_id!} value={window.cg_window_id!}>{window.application_id ?? `pid ${window.pid}`} — {window.title || t('common.untitled')}</option>)}</select>
            <Button disabled={!picked[pad.id] || assign.isPending} onClick={() => assign.mutate({ id: pad.id, window: picked[pad.id]!, force: true })}>{t('pads.assign')}</Button>
            <Button variant="ghost" aria-label={t('pads.unassign', { id: pad.id })} disabled={!entry?.window || unassign.isPending} onClick={() => unassign.mutate(pad.id)}><Unlink size={13} /></Button>
            <Button variant="ghost" onClick={() => open(pad)}>{t('common.edit')}</Button>
            <Button variant="ghost" aria-label={t('common.deleteItem', { id: pad.id })} disabled={patch.isPending} onClick={() => send({ kind: 'remove_scratchpad', id: pad.id })}><Trash2 size={13} /></Button></div>
        </div>
      })}</Card>}
      {error && <p className="bad-text">{error}</p>}
      {recover.isSuccess && <p className={recover.data.ok ? 'good-text' : 'bad-text'}>{recover.data.ok ? t('pads.recovered') : recover.data.problems.join(' · ')}</p>}

      {editing !== undefined && <><SectionHeader title={editing === null ? t('pads.new') : t('common.editItem', { id: editing })} description={t('pads.editHint')} /><Card className="padded stack">
        <div className="form-grid">
          <div className="field"><label htmlFor="pad-name">{t('common.name')}</label><input id="pad-name" value={draft.name} maxLength={80} placeholder={t('pads.namePlaceholder')} onChange={(event) => setDraft({ ...draft, name: event.target.value, id: editing === null && (!draft.id || draft.id === scratchpadSlug(draft.name)) ? scratchpadSlug(event.target.value) : draft.id })} /></div>
          <div className="field"><label htmlFor="pad-id">ID</label><input id="pad-id" className="mono" value={draft.id} maxLength={32} onChange={(event) => setDraft({ ...draft, id: event.target.value })} /></div>
          <div className="field"><label htmlFor="pad-app">{t('pads.application')}</label><select id="pad-app" value={draft.bundle_id ?? ''} onChange={(event) => setDraft({ ...draft, bundle_id: event.target.value || null })}>
            <option value="">{t('pads.anyApp')}</option>
            {draft.bundle_id && !apps.data?.apps.some((app) => app.bundle_id === draft.bundle_id) && <option value={draft.bundle_id}>{draft.bundle_id}</option>}
            {apps.data?.apps.map((app) => <option key={app.bundle_id} value={app.bundle_id}>{app.name ?? app.bundle_id} — {app.bundle_id}</option>)}</select></div>
          <div className="field"><label htmlFor="pad-hotkey">{t('pads.hotkey')}</label><HotkeyRecorder id="pad-hotkey" clearable value={draft.hotkey ?? null} onChange={(hotkey) => setDraft((current) => ({ ...current, hotkey }))} /></div>
          <div className="field"><label htmlFor="pad-scope">{t('pads.scope')}</label><select id="pad-scope" value={draft.scope} onChange={(event) => setDraft({ ...draft, scope: event.target.value as ScratchpadDefinition['scope'] })}><option value="global">{t('pads.scopeGlobal')}</option><option value="local">{t('pads.scopeLocal')}</option></select></div>
          <div className="field"><label htmlFor="pad-hide">{t('pads.hideBy')}</label><select id="pad-hide" value={draft.hide} onChange={(event) => setDraft({ ...draft, hide: event.target.value as ScratchpadDefinition['hide'] })}><option value="park">{t('pads.hidePark')}</option><option value="minimize">{t('pads.hideMinimize')}</option></select></div>
          <div className="field"><label htmlFor="pad-target">{t('pads.showOn')}</label><select id="pad-target" value={draft.display === null || draft.display === undefined ? draft.target : `display-${draft.display}`} onChange={(event) => { const value = event.target.value; if (value.startsWith('display-')) setDraft({ ...draft, display: Number(value.slice(8)) }); else setDraft({ ...draft, display: null, target: value as ScratchpadDefinition['target'] }) }}>
            <option value="pointer">{t('pads.targetPointer')}</option><option value="focused">{t('pads.targetFocused')}</option>
            {displays.data?.displays.map((display) => <option key={display.id} value={`display-${display.id}`}>{t('pads.alwaysDisplay', { id: display.id })}{display.primary ? ` ${t('common.primaryMark')}` : ''}</option>)}</select></div>
          <div className="field"><label htmlFor="pad-alignment">{t('pads.position')}</label><select id="pad-alignment" value={draft.alignment} onChange={(event) => setDraft({ ...draft, alignment: event.target.value as ScratchpadAlignment })}>{ALIGNMENTS.map((value) => <option key={value} value={value}>{t(ALIGNMENT_LABELS[value])}</option>)}</select></div>
          <div className="field"><label htmlFor="pad-width">{t('pads.width', { value: Math.round(draft.width * 100) })}</label><input id="pad-width" type="range" min={10} max={100} value={Math.round(draft.width * 100)} onChange={(event) => setDraft({ ...draft, width: Number(event.target.value) / 100 })} /></div>
          <div className="field"><label htmlFor="pad-height">{t('pads.height', { value: Math.round(draft.height * 100) })}</label><input id="pad-height" type="range" min={10} max={100} value={Math.round(draft.height * 100)} onChange={(event) => setDraft({ ...draft, height: Number(event.target.value) / 100 })} /></div>
          <div className="field"><label htmlFor="pad-offset-x">{t('pads.offsetX')}</label><input id="pad-offset-x" type="number" min={-4000} max={4000} value={draft.offset_x} onChange={(event) => setDraft({ ...draft, offset_x: Number(event.target.value) })} /></div>
          <div className="field"><label htmlFor="pad-offset-y">{t('pads.offsetY')}</label><input id="pad-offset-y" type="number" min={-4000} max={4000} value={draft.offset_y} onChange={(event) => setDraft({ ...draft, offset_y: Number(event.target.value) })} /></div>
        </div>
        {primary && <div className="row" style={{ gap: 16, justifyContent: 'flex-start', alignItems: 'flex-start' }}><GeometryPreview pad={draft} area={primary.work_area} /><span className="row-sub">{t('pads.previewNote', { id: primary.id, width: primary.work_area.width, height: primary.work_area.height })}</span></div>}
        {draft.hotkey && shortcutWarning(draft.hotkey, t) && <Notice tone="warn">{shortcutWarning(draft.hotkey, t)}</Notice>}
        {draft.hide === 'minimize' && <Notice tone="warn">{t('pads.minimizeNotice')}</Notice>}
        <div className="form-actions"><label className="row" style={{ marginRight: 12, gap: 7 }}><input type="checkbox" checked={draft.remember_position} onChange={(event) => setDraft({ ...draft, remember_position: event.target.checked })} /> {t('pads.rememberPosition')}</label>
          <label className="row" style={{ marginRight: 'auto', gap: 7 }}><input type="checkbox" checked={draft.remember_size} onChange={(event) => setDraft({ ...draft, remember_size: event.target.checked })} /> {t('pads.rememberSize')}</label>
          <Button onClick={() => setEditing(undefined)}>{t('common.cancel')}</Button><Button variant="primary" disabled={problem !== null || patch.isPending} onClick={save}>{patch.isPending ? t('common.saving') : t('pads.save')}</Button></div>
        {problem && <p className="bad-text">{problem}</p>}
      </Card></>}
      {patch.isError && <p className="bad-text">{patch.error.message}</p>}

      <SectionHeader title={t('pads.backend')} description={t('pads.backendHint')} />
      <Card className="padded stack">{capabilities.isPending ? <Loading /> : capabilities.data ? <>
        <div className="row"><span>{t('pads.capPark')}</span><Badge tone={capabilities.data.park.available ? 'good' : 'bad'}>{capabilities.data.park.available ? t('common.available') : t('common.unavailable')}</Badge></div>
        <div className="row"><span>{t('pads.capMinimize')}</span><Badge tone={capabilities.data.minimize.available ? 'good' : 'bad'}>{capabilities.data.minimize.available ? t('common.available') : t('common.unavailable')}</Badge></div>
        <div className="row"><span>{t('pads.capSticky')}</span><Badge tone="neutral">{t('pads.notUsed')}</Badge></div>
        <div className="row-sub">{t('pads.stickyReason')} {t('pads.parkingCount', { value: Object.keys(capabilities.data.park.parking_desktops).length || t('pads.parkingNone') })}{capabilities.data.reasons.length ? ` ${capabilities.data.reasons.join(' · ')}` : ''}</div>
        {live.data && <div className="row-sub">{t('pads.metrics', { toggles: live.data.metrics.toggles ?? 0, avg: Math.round((live.data.metrics.toggle_total_ms ?? 0) / Math.max(1, live.data.metrics.toggles ?? 0)), moves: live.data.metrics.moves ?? 0, errors: live.data.metrics.errors ?? 0 })}</div>}
      </> : <p className="bad-text">{capabilities.error?.message ?? t('common.unknownError')}</p>}</Card>
      <div style={{ marginTop: 18 }}><Notice>{t('pads.notice')}</Notice></div>
    </>}
  </>
}
