import { useEffect, useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { RotateCcw, Save } from 'lucide-react'
import { previewLayout } from '../api/bridge'
import { useAction, useConfigPatch, useDaemon } from '../api/hooks'
import { DEFAULT_DWINDLE, DEFAULT_SCROLLING, LAYOUT_KINDS } from '../api/types'
import type { DwindleSettings, LayoutKind, LayoutPreview, ScrollingSettings } from '../api/types'
import { tEn, useT, type MessageKey, type T } from '../i18n'
import { Badge, Button, Card, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

export const LAYOUT_LABELS: Record<LayoutKind, string> = { bsp: 'BSP', dwindle: 'Dwindle', scrolling: 'Scrolling' }
const LAYOUT_HINTS: Record<LayoutKind, MessageKey> = { bsp: 'layouts.hint.bsp', dwindle: 'layouts.hint.dwindle', scrolling: 'layouts.hint.scrolling' }
const PREVIEW_WIDTH = 640
const PREVIEW_HEIGHT = 360

const percent = (value: number) => `${Math.round(value * 100)}%`
export function validScrolling(s: ScrollingSettings, t: T = tEn): string | null {
  const fraction = (v: number) => Number.isFinite(v) && v > 0 && v <= 1
  if (![s.min_column_width, s.max_column_width, s.default_column_width].every(fraction) || s.min_column_width < 0.05) return t('layouts.err.widths')
  if (s.min_column_width > s.max_column_width || s.default_column_width < s.min_column_width || s.default_column_width > s.max_column_width) return t('layouts.err.order')
  if (s.column_width_presets.length === 0 || s.column_width_presets.length > 8 || s.column_width_presets.some((p) => !fraction(p) || p < s.min_column_width || p > s.max_column_width)) return t('layouts.err.presets')
  if (!(s.scroll_sensitivity >= 0.05 && s.scroll_sensitivity <= 1)) return t('layouts.err.step')
  return null
}
/** Parses "33, 50%, 65" into fractions. A value shown rounded (33 for 1/3)
 * maps back to the exact known fraction so an untouched field stays equal. */
export function parsePresets(text: string, known: number[] = []): number[] {
  return text.split(',').map((part) => part.trim().replace('%', '')).filter(Boolean).map((part) => {
    const value = Number(part) / 100
    return known.find((exact) => Math.round(exact * 100) === Math.round(value * 100) && Math.abs(exact - value) < 0.005) ?? value
  })
}
const sameJson = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)

export function PreviewCanvas({ preview }: { preview: LayoutPreview }) {
  const strip = preview.scrolling
  return <>
    <div className="preview-frame" aria-label={`${preview.layout ?? 'bsp'} layout preview`}>
      {preview.frames.map(({ slot, rect, placement }) => <div className={`preview-tile${placement?.state === 'parked' ? ' preview-parked' : ''}`} key={slot} data-placement={placement?.state ?? 'visible'} style={{ left: `${rect.x / PREVIEW_WIDTH * 100}%`, top: `${rect.y / PREVIEW_HEIGHT * 100}%`, width: `${rect.width / PREVIEW_WIDTH * 100}%`, height: `${rect.height / PREVIEW_HEIGHT * 100}%` }}>{String.fromCharCode(64 + slot)}</div>)}
    </div>
    {strip && strip.strip_width > 0 && <div className="strip-map" aria-label="Scrolling strip overview">
      {strip.columns.map((column) => <span key={column.window.token} className={`strip-column strip-${column.placement.state}`} style={{ left: `${column.strip_x / Math.max(strip.strip_width, strip.usable_width) * 100}%`, width: `${column.width / Math.max(strip.strip_width, strip.usable_width) * 100}%` }}>{String.fromCharCode(64 + column.window.token)}</span>)}
      <span className="strip-viewport" style={{ left: `${strip.viewport_offset / Math.max(strip.strip_width, strip.usable_width) * 100}%`, width: `${strip.usable_width / Math.max(strip.strip_width, strip.usable_width) * 100}%` }} />
    </div>}
  </>
}

export function Layouts() {
  const t = useT()
  const status = useDaemon('status', true, true)
  const config = useDaemon('config', status.isSuccess)
  const patch = useConfigPatch()
  const action = useAction()
  const saved = config.data?.config
  const [inner, setInner] = useState(8)
  const [outer, setOuter] = useState(8)
  const [dwindle, setDwindle] = useState<DwindleSettings>(DEFAULT_DWINDLE)
  const [scrolling, setScrolling] = useState<ScrollingSettings>(DEFAULT_SCROLLING)
  const [presetText, setPresetText] = useState('33, 50, 65, 100')
  const [previewKind, setPreviewKind] = useState<LayoutKind>('bsp')
  const [previewWindows, setPreviewWindows] = useState(3)
  const [previewFocus, setPreviewFocus] = useState(1)
  useEffect(() => {
    if (!saved) return
    setInner(saved.gaps.inner); setOuter(saved.gaps.outer)
    setDwindle(saved.dwindle ?? DEFAULT_DWINDLE)
    const next = saved.scrolling ?? DEFAULT_SCROLLING
    setScrolling(next); setPresetText(next.column_width_presets.map((p) => Math.round(p * 100)).join(', '))
    setPreviewKind(saved.default_layout ?? 'bsp')
  }, [config.data?.revision])
  const knownPresets = useMemo(() => [...(saved?.scrolling?.column_width_presets ?? []), ...DEFAULT_SCROLLING.column_width_presets], [saved])
  const draftScrolling = useMemo(() => ({ ...scrolling, column_width_presets: parsePresets(presetText, knownPresets) }), [scrolling, presetText, knownPresets])
  const gapsValid = Number.isFinite(inner) && Number.isFinite(outer) && inner >= 0 && outer >= 0 && inner <= 100 && outer <= 100
  const dwindleValid = Number.isFinite(dwindle.split_ratio) && dwindle.split_ratio >= 0.1 && dwindle.split_ratio <= 0.9
  const scrollingError = validScrolling(draftScrolling, t)
  const [previewInput, setPreviewInput] = useState({ inner, outer, previewKind, dwindle, scrolling: draftScrolling, previewWindows, previewFocus })
  useEffect(() => {
    const timer = window.setTimeout(() => setPreviewInput({ inner, outer, previewKind, dwindle, scrolling: draftScrolling, previewWindows, previewFocus }), 140)
    return () => window.clearTimeout(timer)
  }, [inner, outer, previewKind, dwindle, draftScrolling, previewWindows, previewFocus])
  const previewValid = gapsValid && dwindleValid && !validScrolling(previewInput.scrolling)
  const preview = useQuery({
    queryKey: ['preview', previewInput],
    queryFn: ({ signal }) => previewLayout(previewInput.inner, previewInput.outer, signal, { layout: previewInput.previewKind, dwindle: previewInput.dwindle, scrolling: previewInput.scrolling, windows: previewInput.previewWindows, focus: previewInput.previewFocus }),
    enabled: status.isSuccess && previewValid,
    staleTime: 30_000,
  })
  if (!status.isSuccess) return <><Heading />{status.isPending ? <Loading /> : <ErrorState message={t('layouts.offline')} retry={() => void status.refetch()} />}</>
  if (config.isPending) return <><Heading /><Loading /></>
  if (config.isError || !saved || !config.data) return <><Heading /><ErrorState message={config.error?.message ?? t('layouts.noConfig')} /></>
  const revision = config.data.revision
  const gapsChanged = inner !== saved.gaps.inner || outer !== saved.gaps.outer
  const dwindleChanged = !sameJson(dwindle, saved.dwindle)
  const scrollingChanged = !sameJson(draftScrolling, saved.scrolling)
  const current = status.data.layouts ?? []
  const workspaceName = (id: number | null) => saved.workspaces.find((ws) => ws.id === id)?.name
  const resetAll = () => {
    setInner(8); setOuter(8); setDwindle(DEFAULT_DWINDLE); setScrolling(DEFAULT_SCROLLING)
    setPresetText(DEFAULT_SCROLLING.column_width_presets.map((p) => Math.round(p * 100)).join(', '))
  }
  return <><Heading aside={<Button onClick={resetAll}><RotateCcw size={13} /> {t('layouts.resetAll')}</Button>} />
    <SectionHeader title={t('layouts.current')} description={t('layouts.currentHint')} />
    <Card className="padded stack">
      {current.length === 0 && <span className="row-sub">{t('layouts.noDisplay')}</span>}
      {current.map((entry) => <div className="row row-pad" key={entry.display}>
        <div><span className="row-title">{t('common.display', { id: entry.display })} · {workspaceName(entry.workspace) ?? (entry.native_space_id ? t('layouts.unboundSpace', { id: entry.native_space_id }) : t('layouts.unknownSpace'))}</span>
          <span className="row-sub">{t('layouts.tiledWindows', { count: entry.windows })}{entry.scrolling ? ` · ${entry.scrolling.mode === 'bounded' ? t('layouts.bounded') : t('layouts.parkedColumns', { count: entry.scrolling.columns.filter((c) => c.placement.state === 'parked').length })}` : ''}</span></div>
        <div className="segmented" role="group" aria-label={t('layouts.forDisplay', { id: entry.display })}>{LAYOUT_KINDS.map((kind) => <button key={kind} className={entry.layout === kind ? 'active' : ''} aria-pressed={entry.layout === kind} disabled={action.isPending} onClick={() => action.mutate({ kind: 'command', command: { command: 'set_layout', layout: kind } })}>{LAYOUT_LABELS[kind]}</button>)}</div>
      </div>)}
      <span className="row-sub">{t('layouts.liveHint')}</span>
      {action.isError && <p className="bad-text">{action.error.message}</p>}
    </Card>

    <SectionHeader title={t('layouts.workspace')} description={t('layouts.workspaceHint')} />
    <Card className="padded stack">
      <div className="row row-pad"><div><span className="row-title">{t('layouts.default')}</span><span className="row-sub">{t('layouts.defaultHint')}</span></div>
        <select aria-label={t('layouts.default')} className="search-input" style={{ width: 160 }} value={saved.default_layout} disabled={patch.isPending} onChange={(event) => patch.mutate({ revision, patch: { kind: 'set_default_layout', layout: event.target.value as LayoutKind } })}>{LAYOUT_KINDS.map((kind) => <option key={kind} value={kind}>{LAYOUT_LABELS[kind]}</option>)}</select></div>
      <div className="table-wrap"><table className="table"><thead><tr><th>{t('layouts.colWorkspace')}</th><th>{t('layouts.colSpace')}</th><th>{t('layouts.colLayout')}</th></tr></thead><tbody>
        {saved.workspaces.map((ws) => <tr key={ws.id}><td>{ws.name}</td><td className="mono">{ws.native_space_id ?? '—'}</td><td>
          <select aria-label={t('layouts.forWorkspace', { name: ws.name })} className="search-input" value={ws.layout ?? ''} disabled={patch.isPending} onChange={(event) => patch.mutate({ revision, patch: { kind: 'set_workspace_layout', id: ws.id, layout: event.target.value === '' ? null : event.target.value as LayoutKind } })}>
            <option value="">{t('layouts.defaultOption', { layout: LAYOUT_LABELS[saved.default_layout] })}</option>{LAYOUT_KINDS.map((kind) => <option key={kind} value={kind}>{LAYOUT_LABELS[kind]}</option>)}</select></td></tr>)}
      </tbody></table></div>
    </Card>

    <div className="two-col"><div><SectionHeader title={t('layouts.preview')} description={`${t(LAYOUT_HINTS[previewKind])} ${t('layouts.previewEngine')}`} aside={<div className="segmented" role="group" aria-label={t('layouts.previewLayout')}>{LAYOUT_KINDS.map((kind) => <button key={kind} className={previewKind === kind ? 'active' : ''} aria-pressed={previewKind === kind} onClick={() => setPreviewKind(kind)}>{LAYOUT_LABELS[kind]}</button>)}</div>} />
      <Card className="padded">
        {!previewValid ? <p className="bad-text">{t('layouts.fixSettings')}</p> : preview.isPending ? <Loading /> : preview.isError ? <ErrorState message={preview.error.message} /> : <PreviewCanvas preview={preview.data} />}
        <div className="form-grid" style={{ marginTop: 14 }}>
          <div className="field"><label htmlFor="preview-windows">{t('layouts.sampleWindows', { n: previewWindows })}</label><input id="preview-windows" type="range" min="1" max="6" value={previewWindows} onChange={(event) => { const n = Number(event.target.value); setPreviewWindows(n); setPreviewFocus((f) => Math.min(f, n)) }} /></div>
          <div className="field"><label htmlFor="preview-focus">{t('layouts.focusedWindow')}</label><select id="preview-focus" value={previewFocus} onChange={(event) => setPreviewFocus(Number(event.target.value))}>{Array.from({ length: previewWindows }, (_, i) => i + 1).map((n) => <option key={n} value={n}>{String.fromCharCode(64 + n)}</option>)}</select></div>
        </div>
        <p className="row-sub" style={{ marginTop: 12 }}>{t('layouts.previewNote')}{previewKind === 'scrolling' ? ` ${t('layouts.previewParked')}` : ''}</p>
      </Card></div>
      <div><SectionHeader title={t('layouts.gaps')} description={t('layouts.gapsHint')} /><Card className="padded stack">
        <div className="field"><label htmlFor="inner-gap">{t('layouts.innerGap', { n: inner })}</label><div className="range-row"><input id="inner-gap" type="range" min="0" max="64" step="1" value={inner} onChange={(event) => setInner(Number(event.target.value))} /><input aria-label={t('layouts.innerGapValue')} type="number" min="0" max="100" value={inner} onChange={(event) => setInner(Number(event.target.value))} /></div></div>
        <div className="field"><label htmlFor="outer-gap">{t('layouts.outerGap', { n: outer })}</label><div className="range-row"><input id="outer-gap" type="range" min="0" max="64" step="1" value={outer} onChange={(event) => setOuter(Number(event.target.value))} /><input aria-label={t('layouts.outerGapValue')} type="number" min="0" max="100" value={outer} onChange={(event) => setOuter(Number(event.target.value))} /></div></div>
        <div className="form-actions"><Button onClick={() => { setInner(8); setOuter(8) }}><RotateCcw size={13} /> {t('common.defaults')}</Button><Button onClick={() => { setInner(saved.gaps.inner); setOuter(saved.gaps.outer) }} disabled={!gapsChanged}>{t('common.cancel')}</Button><Button variant="primary" onClick={() => patch.mutate({ revision, patch: { kind: 'set_gaps', inner, outer } })} disabled={!gapsChanged || !gapsValid || patch.isPending}><Save size={13} /> {t('layouts.applyGaps')}</Button></div>
        {!gapsValid && <p className="bad-text">{t('layouts.err.gaps')}</p>}
      </Card>

      <SectionHeader title="Dwindle" description={t('layouts.dwindleHint')} /><Card className="padded stack">
        <div className="field"><label htmlFor="dwindle-orientation">{t('layouts.firstSplit')}</label><select id="dwindle-orientation" value={dwindle.initial_orientation} onChange={(event) => setDwindle({ ...dwindle, initial_orientation: event.target.value as DwindleSettings['initial_orientation'] })}><option value="vertical">{t('layouts.sideBySide')}</option><option value="horizontal">{t('layouts.stacked')}</option></select></div>
        <div className="field"><label htmlFor="dwindle-ratio">{t('layouts.splitRatio', { value: percent(dwindle.split_ratio) })}</label><input id="dwindle-ratio" type="range" min="0.1" max="0.9" step="0.05" value={dwindle.split_ratio} onChange={(event) => setDwindle({ ...dwindle, split_ratio: Number(event.target.value) })} /></div>
        <div className="form-actions"><Button onClick={() => setDwindle(DEFAULT_DWINDLE)}><RotateCcw size={13} /> {t('common.defaults')}</Button><Button onClick={() => setDwindle(saved.dwindle)} disabled={!dwindleChanged}>{t('common.cancel')}</Button><Button variant="primary" onClick={() => patch.mutate({ revision, patch: { kind: 'set_dwindle', settings: dwindle } })} disabled={!dwindleChanged || !dwindleValid || patch.isPending}><Save size={13} /> {t('layouts.apply', { layout: 'Dwindle' })}</Button></div>
      </Card></div></div>

    <SectionHeader title="Scrolling" description={t('layouts.scrollingHint')} />
    <Card className="padded stack">
      <div className="form-grid">
        <div className="field"><label htmlFor="scroll-default">{t('layouts.defaultWidth', { value: percent(scrolling.default_column_width) })}</label><input id="scroll-default" type="range" min="0.05" max="1" step="0.01" value={scrolling.default_column_width} onChange={(event) => setScrolling({ ...scrolling, default_column_width: Number(event.target.value) })} /></div>
        <div className="field"><label htmlFor="scroll-step">{t('layouts.scrollStep', { value: percent(scrolling.scroll_sensitivity) })}</label><input id="scroll-step" type="range" min="0.05" max="1" step="0.05" value={scrolling.scroll_sensitivity} onChange={(event) => setScrolling({ ...scrolling, scroll_sensitivity: Number(event.target.value) })} /></div>
        <div className="field"><label htmlFor="scroll-min">{t('layouts.minWidth', { value: percent(scrolling.min_column_width) })}</label><input id="scroll-min" type="range" min="0.05" max="1" step="0.01" value={scrolling.min_column_width} onChange={(event) => setScrolling({ ...scrolling, min_column_width: Number(event.target.value) })} /></div>
        <div className="field"><label htmlFor="scroll-max">{t('layouts.maxWidth', { value: percent(scrolling.max_column_width) })}</label><input id="scroll-max" type="range" min="0.05" max="1" step="0.01" value={scrolling.max_column_width} onChange={(event) => setScrolling({ ...scrolling, max_column_width: Number(event.target.value) })} /></div>
        <div className="field"><label htmlFor="scroll-presets">{t('layouts.presets')}</label><input id="scroll-presets" value={presetText} onChange={(event) => setPresetText(event.target.value)} /></div>
        <div className="field"><label htmlFor="scroll-align">{t('layouts.alignment')}</label><select id="scroll-align" value={scrolling.viewport_alignment} onChange={(event) => setScrolling({ ...scrolling, viewport_alignment: event.target.value as ScrollingSettings['viewport_alignment'] })}><option value="nearest">{t('layouts.nearest')}</option><option value="center">{t('layouts.centerOption')}</option></select></div>
      </div>
      <label className="row row-pad"><span><span className="row-title">{t('layouts.autoScroll')}</span><span className="row-sub">{t('layouts.autoScrollHint')}</span></span><input type="checkbox" checked={scrolling.focus_auto_scroll} onChange={(event) => setScrolling({ ...scrolling, focus_auto_scroll: event.target.checked })} /></label>
      <label className="row row-pad"><span><span className="row-title">{t('layouts.mouseResize')}</span><span className="row-sub">{t('layouts.mouseResizeHint')}</span></span><input type="checkbox" checked={scrolling.mouse_resize} onChange={(event) => setScrolling({ ...scrolling, mouse_resize: event.target.checked })} /></label>
      {scrollingError && <p className="bad-text">{scrollingError}</p>}
      <div className="form-actions"><Button onClick={() => { setScrolling(DEFAULT_SCROLLING); setPresetText(DEFAULT_SCROLLING.column_width_presets.map((p) => Math.round(p * 100)).join(', ')) }}><RotateCcw size={13} /> {t('common.defaults')}</Button><Button onClick={() => { setScrolling(saved.scrolling); setPresetText(saved.scrolling.column_width_presets.map((p) => Math.round(p * 100)).join(', ')) }} disabled={!scrollingChanged}>{t('common.cancel')}</Button><Button variant="primary" onClick={() => patch.mutate({ revision, patch: { kind: 'set_scrolling', settings: draftScrolling } })} disabled={!scrollingChanged || scrollingError !== null || patch.isPending}><Save size={13} /> {t('layouts.apply', { layout: 'Scrolling' })}</Button></div>
      <Notice tone="warn">{t('layouts.parkNotice')}</Notice>
    </Card>
    {patch.isError && <p className="bad-text">{patch.error.message}</p>}{patch.isSuccess && <p className="good-text">{t('layouts.saved')}</p>}

    <div style={{ marginTop: 18 }}><SectionHeader title={t('layouts.zoom')} description={t('layouts.zoomHint')} /><Card className="padded stack">
      <label className="row row-pad"><span><span className="row-title">{t('layouts.respectGaps')}</span><span className="row-sub">{t('layouts.respectGapsHint')}</span></span><input type="checkbox" checked={saved.zoom?.respect_outer_gaps ?? false} disabled={patch.isPending} onChange={(event) => patch.mutate({ revision, patch: { kind: 'set_zoom', respect_outer_gaps: event.target.checked } })} /></label>
      <div className="row row-pad"><span className="row-sub">{status.data.layout_zoom?.filter((entry) => status.data.native_spaces.some((space) => space.active && space.display === entry.display && space.id === entry.native_space_id)).map((entry) => entry.confirmed ? t('layouts.zoomActive', { id: entry.display }) : t('layouts.zoomPending', { id: entry.display })).join(' · ') || t('layouts.noZoom')}</span><Button onClick={() => action.mutate({ kind: 'command', command: { command: 'toggle_layout_zoom', window: null } })} disabled={action.isPending}>{t('layouts.toggleZoom')}</Button></div>
    </Card></div>
  </>
}

function Heading({ aside }: { aside?: React.ReactNode }) {
  const t = useT()
  return <div className="page-heading"><div><span className="eyebrow">{t('layouts.eyebrow')}</span><h1>{t('nav.layouts')}</h1><p>{t('layouts.intro')}</p></div>{aside && <div className="page-actions">{aside}<Badge tone="good">{t('layouts.count')}</Badge></div>}</div>
}
