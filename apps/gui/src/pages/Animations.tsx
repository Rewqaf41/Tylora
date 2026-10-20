import { useEffect, useRef, useState } from 'react'
import { Play, Save } from 'lucide-react'
import { useConfigPatch, useDaemon } from '../api/hooks'
import type { AnimationBackendChoice, AnimationConfig, AnimationEvent, AnimationsStatus, Easing, Rect, SlideDirection } from '../api/types'
import { useT, type MessageKey } from '../i18n'
import { ease, EASINGS, interpolate } from '../shared/easing'
import { Badge, Button, Card, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

/** The daemon's defaults (tylora-core AnimationConfig::default). */
export const DEFAULT_ANIMATIONS: AnimationConfig = {
  enabled: true, backend: 'auto', duration_ms: 180, easing: 'ease_out_cubic', preferred_fps: 60,
  respect_reduce_motion: true, scratchpad_slide: 'down',
  events: { window_open: true, window_close: true, layout_change: true, layout_zoom: true, scrolling: true, scratchpad: true },
}
const EVENTS: AnimationEvent[] = ['window_open', 'window_close', 'layout_change', 'layout_zoom', 'scrolling', 'scratchpad']
const BACKENDS: AnimationBackendChoice[] = ['auto', 'accessibility', 'instant', 'native_experimental']
const SLIDES: SlideDirection[] = ['down', 'up', 'left', 'right']
const FPS = [30, 60, 90, 120]

/** Why a draft cannot be saved, or null. Mirrors AnimationConfig::validate. */
export function animationProblem(config: AnimationConfig): 'duration' | 'fps' | null {
  if (!Number.isInteger(config.duration_ms) || config.duration_ms < 0 || config.duration_ms > 500) return 'duration'
  if (!Number.isInteger(config.preferred_fps) || config.preferred_fps < 15 || config.preferred_fps > 240) return 'fps'
  return null
}

/** Demo tiles in percent of the preview: a window opens beside two others. */
const TWO_TILES: Rect[] = [{ x: 2, y: 3, width: 47, height: 94 }, { x: 51, y: 3, width: 47, height: 94 }, { x: 74, y: 52, width: 24, height: 45 }]
const THREE_TILES: Rect[] = [{ x: 2, y: 3, width: 47, height: 94 }, { x: 51, y: 3, width: 47, height: 45 }, { x: 51, y: 52, width: 47, height: 45 }]

/**
 * Plays the chosen timing on demo rectangles only; real windows never move
 * from here. With animations off or a zero duration the tiles jump, like
 * the daemon would.
 */
export function AnimationPreview({ config }: { config: AnimationConfig }) {
  const t = useT()
  const [opened, setOpened] = useState(false)
  const [tiles, setTiles] = useState<Rect[]>(TWO_TILES)
  const frame = useRef<number | null>(null)
  useEffect(() => () => { if (frame.current !== null) cancelAnimationFrame(frame.current) }, [])
  const play = () => {
    const targets = opened ? TWO_TILES : THREE_TILES
    setOpened(!opened)
    if (frame.current !== null) cancelAnimationFrame(frame.current)
    if (!config.enabled || config.duration_ms === 0) { setTiles(targets); return }
    // Retarget from wherever the tiles are now, like the daemon does.
    const from = tiles
    const started = performance.now()
    const step = (now: number) => {
      const progress = Math.min(1, (now - started) / config.duration_ms)
      const eased = ease(config.easing, progress)
      setTiles(from.map((rect, index) => interpolate(rect, targets[index] ?? rect, eased)))
      frame.current = progress < 1 ? requestAnimationFrame(step) : null
    }
    frame.current = requestAnimationFrame(step)
  }
  return <Card className="padded stack">
    <div className="preview-frame" aria-label={t('anim.previewArea')}>
      {tiles.map((rect, index) => <div key={index} className="preview-tile" style={{ left: `${rect.x}%`, top: `${rect.y}%`, width: `${rect.width}%`, height: `${rect.height}%`, ...(index === 2 && !opened ? { opacity: 0.55 } : {}) }}>{index + 1}</div>)}
    </div>
    <div className="form-actions"><Button onClick={play}><Play size={13} /> {t('anim.play')}</Button></div>
    <span className="row-sub">{t('anim.previewHint')}</span>
  </Card>
}

function BackendPanel({ status }: { status: AnimationsStatus }) {
  const t = useT()
  const ms = (value: number | null) => value === null ? '—' : `${value.toFixed(1)} ms`
  const m = status.metrics
  const rows: [MessageKey, string | number][] = [
    ['anim.inUse', status.worker ? t(`anim.backend.${status.worker.backend}` as MessageKey) : '—'],
    ['anim.reduceMotionSystem', status.reduce_motion ? t('anim.on') : t('anim.off')],
    ['anim.slowApps', status.worker?.slow_applications.length ?? 0],
    ['anim.counts', `${m.started} / ${m.completed} / ${m.retargeted} / ${m.cancelled} / ${m.instant}`],
    ['anim.writeLatency', `${ms(status.summary.mean_write_ms)} · max ${status.summary.max_write_ms.toFixed(1)} ms`],
    ['anim.tickInterval', ms(status.summary.mean_tick_interval_ms)],
    ['anim.finalError', status.summary.mean_final_error_pt === null ? '—' : `${status.summary.mean_final_error_pt.toFixed(1)} pt`],
    ['anim.skipped', m.skipped_slots],
  ]
  return <Card className="padded">
    {status.backends.map((backend) => <div key={backend.name} className="row row-pad"><div><strong>{t(`anim.backend.${backend.name}` as MessageKey)}</strong>{backend.reason && <span className="row-sub">{backend.reason}</span>}</div><Badge tone={backend.available ? 'good' : 'warn'}>{backend.available ? t('common.available') : t('common.unavailable')}</Badge></div>)}
    {rows.map(([key, value]) => <div key={key} className="row row-pad"><span>{t(key)}</span><strong>{value}</strong></div>)}
    <span className="row-sub">{t('anim.fpsNote')}</span>
  </Card>
}

export function Animations() {
  const t = useT()
  const status = useDaemon('status')
  const config = useDaemon('config', status.isSuccess)
  const animations = useDaemon('animations', status.isSuccess)
  const patch = useConfigPatch()
  const saved = config.data?.config.animations
  const [draft, setDraft] = useState<AnimationConfig | null>(null)
  useEffect(() => { if (config.data) setDraft(structuredClone(config.data.config.animations ?? DEFAULT_ANIMATIONS)) }, [config.data?.revision])
  const change = (next: Partial<AnimationConfig>) => { patch.reset(); setDraft((current) => current && { ...current, ...next }) }
  const changed = !!draft && JSON.stringify(draft) !== JSON.stringify(saved ?? DEFAULT_ANIMATIONS)
  const problem = draft ? animationProblem(draft) : null
  const apply = () => { if (draft && config.data && !problem) patch.mutate({ revision: config.data.revision, patch: { kind: 'set_animations', animations: draft } }) }
  const nativeBackend = animations.data?.backends.find((backend) => backend.name === 'native_experimental')
  return <><div className="page-heading"><div><span className="eyebrow">{t('anim.eyebrow')}</span><h1>{t('nav.animations')}</h1><p>{t('anim.intro')}</p></div></div>
    {!status.isSuccess ? status.isPending ? <Loading /> : <ErrorState message={t('anim.offline')} retry={() => void status.refetch()} /> : config.isError ? <ErrorState message={config.error.message} /> : config.isPending || !draft ? <Loading /> : <>
      {!saved && <div style={{ marginBottom: 14 }}><Notice tone="warn">{t('anim.unsupportedDaemon')}</Notice></div>}
      <SectionHeader title={t('anim.motion')} description={t('anim.motionHint')} />
      <Card className="padded stack">
        <div className="row row-pad"><div><strong>{t('anim.enabled')}</strong><span className="row-sub">{t('anim.enabledHint')}</span></div><input aria-label={t('anim.enabled')} type="checkbox" checked={draft.enabled} onChange={(e) => change({ enabled: e.target.checked })} /></div>
        <div className="row row-pad"><div><strong>{t('anim.backendLabel')}</strong><span className="row-sub">{t('anim.backendHint')}</span></div><select className="search-input" aria-label={t('anim.backendLabel')} value={draft.backend} disabled={!draft.enabled} onChange={(e) => change({ backend: e.target.value as AnimationBackendChoice })}>
          {BACKENDS.map((backend) => <option key={backend} value={backend} disabled={backend === 'native_experimental' && !nativeBackend?.available}>{t(`anim.backend.${backend}` as MessageKey)}</option>)}
        </select></div>
        <div className="field"><label htmlFor="anim-duration">{t('anim.duration', { ms: draft.duration_ms })}</label><div className="range-row"><input id="anim-duration" type="range" min="0" max="500" step="10" value={draft.duration_ms} disabled={!draft.enabled} onChange={(e) => change({ duration_ms: Number(e.target.value) })} /><input aria-label={t('anim.durationValue')} type="number" min="0" max="500" value={draft.duration_ms} disabled={!draft.enabled} onChange={(e) => change({ duration_ms: Number(e.target.value) })} /></div></div>
        <div className="row row-pad"><div><strong>{t('anim.easing')}</strong></div><select className="search-input" aria-label={t('anim.easing')} value={draft.easing} disabled={!draft.enabled} onChange={(e) => change({ easing: e.target.value as Easing })}>{EASINGS.map((easing) => <option key={easing} value={easing}>{t(`anim.easing.${easing}` as MessageKey)}</option>)}</select></div>
        <div className="row row-pad"><div><strong>{t('anim.fps')}</strong><span className="row-sub">{t('anim.fpsHint')}</span></div><select className="search-input" aria-label={t('anim.fps')} value={draft.preferred_fps} disabled={!draft.enabled} onChange={(e) => change({ preferred_fps: Number(e.target.value) })}>{(FPS.includes(draft.preferred_fps) ? FPS : [...FPS, draft.preferred_fps]).map((fps) => <option key={fps} value={fps}>{fps}</option>)}</select></div>
        <div className="row row-pad"><div><strong>{t('anim.reduceMotion')}</strong><span className="row-sub">{t('anim.reduceMotionHint')}</span></div><input aria-label={t('anim.reduceMotion')} type="checkbox" checked={draft.respect_reduce_motion} disabled={!draft.enabled} onChange={(e) => change({ respect_reduce_motion: e.target.checked })} /></div>
        <div className="row row-pad"><div><strong>{t('anim.slide')}</strong><span className="row-sub">{t('anim.slideHint')}</span></div><select className="search-input" aria-label={t('anim.slide')} value={draft.scratchpad_slide} disabled={!draft.enabled || !draft.events.scratchpad} onChange={(e) => change({ scratchpad_slide: e.target.value as SlideDirection })}>{SLIDES.map((slide) => <option key={slide} value={slide}>{t(`anim.slide.${slide}` as MessageKey)}</option>)}</select></div>
      </Card>
      <SectionHeader title={t('anim.events')} description={t('anim.eventsHint')} />
      <Card className="padded stack">
        {EVENTS.map((event) => <div key={event} className="row row-pad"><div><strong>{t(`anim.event.${event}` as MessageKey)}</strong></div><input aria-label={t(`anim.event.${event}` as MessageKey)} type="checkbox" checked={draft.events[event]} disabled={!draft.enabled} onChange={(e) => change({ events: { ...draft.events, [event]: e.target.checked } })} /></div>)}
        <div className="form-actions"><Button onClick={() => setDraft(structuredClone(saved ?? DEFAULT_ANIMATIONS))} disabled={!changed}>{t('common.cancel')}</Button><Button variant="primary" onClick={apply} disabled={!changed || !!problem || patch.isPending || !saved}><Save size={13} /> {patch.isPending ? t('common.applying') : t('anim.apply')}</Button></div>
        {problem && <p className="bad-text">{t(problem === 'duration' ? 'anim.invalidDuration' : 'anim.invalidFps')}</p>}
        {patch.isError && <p className="bad-text">{patch.error.message}</p>}{patch.isSuccess && <p className="good-text">{t('anim.confirmed')}</p>}
      </Card>
      <div className="two-col">
        <div><SectionHeader title={t('anim.preview')} description={t('anim.previewDescription')} /><AnimationPreview config={draft} /></div>
        <div><SectionHeader title={t('anim.diagnostics')} description={t('anim.diagnosticsHint')} />{animations.isError ? <Notice tone="warn">{animations.error.message}</Notice> : animations.data ? <BackendPanel status={animations.data} /> : <Loading />}</div>
      </div>
      <div style={{ marginTop: 18 }}><Notice>{t('anim.notice')}</Notice></div>
    </>}
  </>
}
