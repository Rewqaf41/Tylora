import { useEffect, useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { RotateCcw, Save } from 'lucide-react'
import { previewLayout } from '../api/bridge'
import { useAction, useConfigPatch, useDaemon } from '../api/hooks'
import { DEFAULT_DWINDLE, DEFAULT_SCROLLING, LAYOUT_KINDS } from '../api/types'
import type { DwindleSettings, LayoutKind, LayoutPreview, ScrollingSettings } from '../api/types'
import { Badge, Button, Card, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

export const LAYOUT_LABELS: Record<LayoutKind, string> = { bsp: 'BSP', dwindle: 'Dwindle', scrolling: 'Scrolling' }
const LAYOUT_HINTS: Record<LayoutKind, string> = {
  bsp: 'Binary space partitioning. New windows split the most recent region.',
  dwindle: 'Hyprland-style. Each new window splits the focused window; splits alternate orientation.',
  scrolling: 'niri-style. Windows form a horizontal strip of columns; the viewport scrolls to the focused one.',
}
const PREVIEW_WIDTH = 640
const PREVIEW_HEIGHT = 360

const percent = (value: number) => `${Math.round(value * 100)}%`
export function validScrolling(s: ScrollingSettings): string | null {
  const fraction = (v: number) => Number.isFinite(v) && v > 0 && v <= 1
  if (![s.min_column_width, s.max_column_width, s.default_column_width].every(fraction) || s.min_column_width < 0.05) return 'Column widths must be between 5% and 100%.'
  if (s.min_column_width > s.max_column_width || s.default_column_width < s.min_column_width || s.default_column_width > s.max_column_width) return 'Use minimum ≤ default ≤ maximum.'
  if (s.column_width_presets.length === 0 || s.column_width_presets.length > 8 || s.column_width_presets.some((p) => !fraction(p) || p < s.min_column_width || p > s.max_column_width)) return 'Presets need 1 to 8 widths inside the minimum and maximum.'
  if (!(s.scroll_sensitivity >= 0.05 && s.scroll_sensitivity <= 1)) return 'Scroll step must be between 5% and 100%.'
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
  const scrollingError = validScrolling(draftScrolling)
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
  if (!status.isSuccess) return <><Heading />{status.isPending ? <Loading /> : <ErrorState message="Connect to the daemon to edit layout settings." retry={() => void status.refetch()} />}</>
  if (config.isPending) return <><Heading /><Loading /></>
  if (config.isError || !saved || !config.data) return <><Heading /><ErrorState message={config.error?.message ?? 'Configuration unavailable'} /></>
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
  return <><Heading aside={<Button onClick={resetAll}><RotateCcw size={13} /> Reset all to defaults</Button>} />
    <SectionHeader title="Current layout" description="Live layout of the workspace on each display. Switching keeps every window and its macOS Space." />
    <Card className="padded stack">
      {current.length === 0 && <span className="row-sub">No active display reported by the daemon.</span>}
      {current.map((entry) => <div className="row row-pad" key={entry.display}>
        <div><span className="row-title">Display {entry.display} · {workspaceName(entry.workspace) ?? (entry.native_space_id ? `Unbound Space ${entry.native_space_id}` : 'Unknown Space')}</span>
          <span className="row-sub">{entry.windows} tiled window{entry.windows === 1 ? '' : 's'}{entry.scrolling ? ` · ${entry.scrolling.mode === 'bounded' ? 'bounded (no safe edge to park columns)' : `${entry.scrolling.columns.filter((c) => c.placement.state === 'parked').length} parked column(s)`}` : ''}</span></div>
        <div className="segmented" role="group" aria-label={`Layout for display ${entry.display}`}>{LAYOUT_KINDS.map((kind) => <button key={kind} className={entry.layout === kind ? 'active' : ''} aria-pressed={entry.layout === kind} disabled={action.isPending} onClick={() => action.mutate({ kind: 'command', command: { command: 'set_layout', layout: kind } })}>{LAYOUT_LABELS[kind]}</button>)}</div>
      </div>)}
      <span className="row-sub">Live switching applies to the display of the focused window, or the display under the pointer. It is not saved; assign layouts below to keep them.</span>
      {action.isError && <p className="bad-text">{action.error.message}</p>}
    </Card>

    <SectionHeader title="Workspace layouts" description="Saved in the daemon configuration. Each workspace keeps its own layout state per native Space." />
    <Card className="padded stack">
      <div className="row row-pad"><div><span className="row-title">Default layout</span><span className="row-sub">Used by workspaces without their own choice and by unbound Spaces.</span></div>
        <select aria-label="Default layout" className="search-input" style={{ width: 160 }} value={saved.default_layout} disabled={patch.isPending} onChange={(event) => patch.mutate({ revision, patch: { kind: 'set_default_layout', layout: event.target.value as LayoutKind } })}>{LAYOUT_KINDS.map((kind) => <option key={kind} value={kind}>{LAYOUT_LABELS[kind]}</option>)}</select></div>
      <div className="table-wrap"><table className="table"><thead><tr><th>Workspace</th><th>Native Space</th><th>Layout</th></tr></thead><tbody>
        {saved.workspaces.map((ws) => <tr key={ws.id}><td>{ws.name}</td><td className="mono">{ws.native_space_id ?? '—'}</td><td>
          <select aria-label={`Layout for ${ws.name}`} className="search-input" value={ws.layout ?? ''} disabled={patch.isPending} onChange={(event) => patch.mutate({ revision, patch: { kind: 'set_workspace_layout', id: ws.id, layout: event.target.value === '' ? null : event.target.value as LayoutKind } })}>
            <option value="">Default ({LAYOUT_LABELS[saved.default_layout]})</option>{LAYOUT_KINDS.map((kind) => <option key={kind} value={kind}>{LAYOUT_LABELS[kind]}</option>)}</select></td></tr>)}
      </tbody></table></div>
    </Card>

    <div className="two-col"><div><SectionHeader title="Live preview" description={`${LAYOUT_HINTS[previewKind]} Frames come from the daemon's Rust engine.`} aside={<div className="segmented" role="group" aria-label="Preview layout">{LAYOUT_KINDS.map((kind) => <button key={kind} className={previewKind === kind ? 'active' : ''} aria-pressed={previewKind === kind} onClick={() => setPreviewKind(kind)}>{LAYOUT_LABELS[kind]}</button>)}</div>} />
      <Card className="padded">
        {!previewValid ? <p className="bad-text">Fix the highlighted settings to update the preview.</p> : preview.isPending ? <Loading /> : preview.isError ? <ErrorState message={preview.error.message} /> : <PreviewCanvas preview={preview.data} />}
        <div className="form-grid" style={{ marginTop: 14 }}>
          <div className="field"><label htmlFor="preview-windows">SAMPLE WINDOWS · {previewWindows}</label><input id="preview-windows" type="range" min="1" max="6" value={previewWindows} onChange={(event) => { const n = Number(event.target.value); setPreviewWindows(n); setPreviewFocus((f) => Math.min(f, n)) }} /></div>
          <div className="field"><label htmlFor="preview-focus">FOCUSED WINDOW</label><select id="preview-focus" value={previewFocus} onChange={(event) => setPreviewFocus(Number(event.target.value))}>{Array.from({ length: previewWindows }, (_, i) => i + 1).map((n) => <option key={n} value={n}>{String.fromCharCode(64 + n)}</option>)}</select></div>
        </div>
        <p className="row-sub" style={{ marginTop: 12 }}>Preview updates while you edit. Real windows change only after Apply.{previewKind === 'scrolling' ? ' Columns outside the viewport are parked by the daemon with a small sliver visible.' : ''}</p>
      </Card></div>
      <div><SectionHeader title="Gaps" description="Shared by all layouts" /><Card className="padded stack">
        <div className="field"><label htmlFor="inner-gap">INNER GAP · {inner} PT</label><div className="range-row"><input id="inner-gap" type="range" min="0" max="64" step="1" value={inner} onChange={(event) => setInner(Number(event.target.value))} /><input aria-label="Inner gap value" type="number" min="0" max="100" value={inner} onChange={(event) => setInner(Number(event.target.value))} /></div></div>
        <div className="field"><label htmlFor="outer-gap">OUTER GAP · {outer} PT</label><div className="range-row"><input id="outer-gap" type="range" min="0" max="64" step="1" value={outer} onChange={(event) => setOuter(Number(event.target.value))} /><input aria-label="Outer gap value" type="number" min="0" max="100" value={outer} onChange={(event) => setOuter(Number(event.target.value))} /></div></div>
        <div className="form-actions"><Button onClick={() => { setInner(8); setOuter(8) }}><RotateCcw size={13} /> Defaults</Button><Button onClick={() => { setInner(saved.gaps.inner); setOuter(saved.gaps.outer) }} disabled={!gapsChanged}>Cancel</Button><Button variant="primary" onClick={() => patch.mutate({ revision, patch: { kind: 'set_gaps', inner, outer } })} disabled={!gapsChanged || !gapsValid || patch.isPending}><Save size={13} /> Apply gaps</Button></div>
        {!gapsValid && <p className="bad-text">Gap values must be between 0 and 100 points.</p>}
      </Card>

      <SectionHeader title="Dwindle" description="Applies to new splits; existing splits keep their ratio" /><Card className="padded stack">
        <div className="field"><label htmlFor="dwindle-orientation">FIRST SPLIT</label><select id="dwindle-orientation" value={dwindle.initial_orientation} onChange={(event) => setDwindle({ ...dwindle, initial_orientation: event.target.value as DwindleSettings['initial_orientation'] })}><option value="vertical">Side by side</option><option value="horizontal">Stacked</option></select></div>
        <div className="field"><label htmlFor="dwindle-ratio">SPLIT RATIO · {percent(dwindle.split_ratio)}</label><input id="dwindle-ratio" type="range" min="0.1" max="0.9" step="0.05" value={dwindle.split_ratio} onChange={(event) => setDwindle({ ...dwindle, split_ratio: Number(event.target.value) })} /></div>
        <div className="form-actions"><Button onClick={() => setDwindle(DEFAULT_DWINDLE)}><RotateCcw size={13} /> Defaults</Button><Button onClick={() => setDwindle(saved.dwindle)} disabled={!dwindleChanged}>Cancel</Button><Button variant="primary" onClick={() => patch.mutate({ revision, patch: { kind: 'set_dwindle', settings: dwindle } })} disabled={!dwindleChanged || !dwindleValid || patch.isPending}><Save size={13} /> Apply Dwindle</Button></div>
      </Card></div></div>

    <SectionHeader title="Scrolling" description="Widths are fractions of the usable display width inside the outer gap" />
    <Card className="padded stack">
      <div className="form-grid">
        <div className="field"><label htmlFor="scroll-default">DEFAULT COLUMN WIDTH · {percent(scrolling.default_column_width)}</label><input id="scroll-default" type="range" min="0.05" max="1" step="0.01" value={scrolling.default_column_width} onChange={(event) => setScrolling({ ...scrolling, default_column_width: Number(event.target.value) })} /></div>
        <div className="field"><label htmlFor="scroll-step">SCROLL STEP · {percent(scrolling.scroll_sensitivity)} OF VIEWPORT</label><input id="scroll-step" type="range" min="0.05" max="1" step="0.05" value={scrolling.scroll_sensitivity} onChange={(event) => setScrolling({ ...scrolling, scroll_sensitivity: Number(event.target.value) })} /></div>
        <div className="field"><label htmlFor="scroll-min">MINIMUM WIDTH · {percent(scrolling.min_column_width)}</label><input id="scroll-min" type="range" min="0.05" max="1" step="0.01" value={scrolling.min_column_width} onChange={(event) => setScrolling({ ...scrolling, min_column_width: Number(event.target.value) })} /></div>
        <div className="field"><label htmlFor="scroll-max">MAXIMUM WIDTH · {percent(scrolling.max_column_width)}</label><input id="scroll-max" type="range" min="0.05" max="1" step="0.01" value={scrolling.max_column_width} onChange={(event) => setScrolling({ ...scrolling, max_column_width: Number(event.target.value) })} /></div>
        <div className="field"><label htmlFor="scroll-presets">WIDTH PRESETS (%) · OPTION+CONTROL+W CYCLES</label><input id="scroll-presets" value={presetText} onChange={(event) => setPresetText(event.target.value)} /></div>
        <div className="field"><label htmlFor="scroll-align">VIEWPORT ALIGNMENT</label><select id="scroll-align" value={scrolling.viewport_alignment} onChange={(event) => setScrolling({ ...scrolling, viewport_alignment: event.target.value as ScrollingSettings['viewport_alignment'] })}><option value="nearest">Nearest — scroll only as far as needed</option><option value="center">Center — keep the focused column centred</option></select></div>
      </div>
      <label className="row row-pad"><span><span className="row-title">Scroll to focused column</span><span className="row-sub">Keyboard focus and new windows move the viewport so the column is visible.</span></span><input type="checkbox" checked={scrolling.focus_auto_scroll} onChange={(event) => setScrolling({ ...scrolling, focus_auto_scroll: event.target.checked })} /></label>
      <label className="row row-pad"><span><span className="row-title">Mouse resize</span><span className="row-sub">Modifier + right-drag changes the column width.</span></span><input type="checkbox" checked={scrolling.mouse_resize} onChange={(event) => setScrolling({ ...scrolling, mouse_resize: event.target.checked })} /></label>
      {scrollingError && <p className="bad-text">{scrollingError}</p>}
      <div className="form-actions"><Button onClick={() => { setScrolling(DEFAULT_SCROLLING); setPresetText(DEFAULT_SCROLLING.column_width_presets.map((p) => Math.round(p * 100)).join(', ')) }}><RotateCcw size={13} /> Defaults</Button><Button onClick={() => { setScrolling(saved.scrolling); setPresetText(saved.scrolling.column_width_presets.map((p) => Math.round(p * 100)).join(', ')) }} disabled={!scrollingChanged}>Cancel</Button><Button variant="primary" onClick={() => patch.mutate({ revision, patch: { kind: 'set_scrolling', settings: draftScrolling } })} disabled={!scrollingChanged || scrollingError !== null || patch.isPending}><Save size={13} /> Apply Scrolling</Button></div>
      <Notice tone="warn">macOS never lets a window leave every display: WindowServer keeps about 40 points of it visible. Tylora parks columns outside the viewport in a bottom corner of their own display. If another display is attached on both sides, the strip falls back to a bounded mode that fits every column on screen.</Notice>
    </Card>
    {patch.isError && <p className="bad-text">{patch.error.message}</p>}{patch.isSuccess && <p className="good-text">Daemon saved the configuration and scheduled retiling.</p>}

    <div style={{ marginTop: 18 }}><SectionHeader title="Layout Zoom" description="Enlarge the focused tiled window within its current macOS Space. Option+Shift+F toggles it by default. Works in every layout and keeps ratios, column widths and the viewport." /><Card className="padded stack">
      <label className="row row-pad"><span><span className="row-title">Respect outer gaps</span><span className="row-sub">Leave the configured outer margin around a zoomed window.</span></span><input type="checkbox" checked={saved.zoom?.respect_outer_gaps ?? false} disabled={patch.isPending} onChange={(event) => patch.mutate({ revision, patch: { kind: 'set_zoom', respect_outer_gaps: event.target.checked } })} /></label>
      <div className="row row-pad"><span className="row-sub">{status.data.layout_zoom?.filter((entry) => status.data.native_spaces.some((space) => space.active && space.display === entry.display && space.id === entry.native_space_id)).map((entry) => entry.confirmed ? `Zoom active on display ${entry.display}` : `Zoom pending on display ${entry.display}`).join(' · ') || 'No active layout Zoom'}</span><Button onClick={() => action.mutate({ kind: 'command', command: { command: 'toggle_layout_zoom', window: null } })} disabled={action.isPending}>Toggle focused window</Button></div>
    </Card></div>
  </>
}

function Heading({ aside }: { aside?: React.ReactNode }) {
  return <div className="page-heading"><div><span className="eyebrow">Tiling</span><h1>Layouts</h1><p>BSP, Dwindle and Scrolling layouts. Settings are applied by the daemon over IPC; previews use the same Rust layout engine.</p></div>{aside && <div className="page-actions">{aside}<Badge tone="good">3 layouts</Badge></div>}</div>
}
