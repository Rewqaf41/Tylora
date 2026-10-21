import { useEffect, useRef, useState } from 'react'
import { Check, Play } from 'lucide-react'
import { useConfigPatch, useDaemon } from '../api/hooks'
import type { AnimationBackendChoice, AnimationConfig, AnimationEvent, AnimationsStatus, Easing, Rect, SlideDirection } from '../api/types'
import { useT, type MessageKey } from '../i18n'
import { ease, EASINGS, interpolate } from '../shared/easing'
import { Badge, Button, Card, ErrorState, Loading, Notice, PageHeading, Range } from '../shared/ui'

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
  return <Card title={t('anim.preview')} description={t('anim.previewDescription')} aside={<Button size="sm" onClick={play}><Play size={13} /> {t('anim.play')}</Button>} className="padded stack">
    <div className="preview-frame" aria-label={t('anim.previewArea')}>
      {tiles.map((rect, index) => <div key={index} className={`preview-tile${index === 1 ? ' preview-focused' : ''}`} style={{ left: `${rect.x}%`, top: `${rect.y}%`, width: `${rect.width}%`, height: `${rect.height}%`, ...(index === 2 && !opened ? { opacity: 0.55 } : {}) }}>{index + 1}</div>)}
    </div>
    <span className="row-sub" style={{ marginTop: 0 }}>{t('anim.previewHint')}</span>
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
  return <Card title={t('anim.diagnostics')} description={t('anim.diagnosticsHint')} className="padded">
    {status.backends.map((backend) => <div key={backend.name} className="row row-pad"><div><strong>{t(`anim.backend.${backend.name}` as MessageKey)}</strong>{backend.reason && <span className="row-sub">{backend.reason}</span>}</div><Badge tone={backend.available ? 'good' : 'bad'}>{backend.available ? t('common.available') : t('common.unavailable')}</Badge></div>)}
    {rows.map(([key, value]) => <div key={key} className="row row-pad"><span className="row-title">{t(key)}</span><span className="mono">{value}</span></div>)}
    <span className="row-sub" style={{ marginTop: 12 }}>{t('anim.fpsNote')}</span>
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
  const ready = status.isSuccess && !!draft && !config.isPending && !config.isError
  return <><PageHeading eyebrow={t('anim.eyebrow')} title={t('nav.animations')} intro={t('anim.intro')} actions={ready && <><Button variant="ghost" onClick={() => setDraft(structuredClone(saved ?? DEFAULT_ANIMATIONS))} disabled={!changed}>{t('common.cancel')}</Button><Button variant="primary" onClick={apply} disabled={!changed || !!problem || patch.isPending || !saved}><Check size={14} /> {patch.isPending ? t('common.applying') : t('anim.apply')}</Button></>} />
    {!status.isSuccess ? !status.isFetched ? <Loading /> : <ErrorState message={t('anim.offline')} retry={() => void status.refetch()} /> : config.isError ? <ErrorState message={config.error.message} /> : config.isPending || !draft ? <Loading /> : <>
      {!saved && <Notice tone="warn">{t('anim.unsupportedDaemon')}</Notice>}
      {problem && <Notice tone="bad">{t(problem === 'duration' ? 'anim.invalidDuration' : 'anim.invalidFps')}</Notice>}
      {patch.isError && <Notice tone="bad">{patch.error.message}</Notice>}{patch.isSuccess && <p className="good-text">{t('anim.confirmed')}</p>}
      <div className="two-col"><div>
      <Card title={t('anim.motion')} description={t('anim.motionHint')} className="padded stack">
        <div className="row row-pad"><div><strong>{t('anim.enabled')}</strong><span className="row-sub">{t('anim.enabledHint')}</span></div><input aria-label={t('anim.enabled')} type="checkbox" checked={draft.enabled} onChange={(e) => change({ enabled: e.target.checked })} /></div>
        <div className="row row-pad"><div><strong>{t('anim.backendLabel')}</strong><span className="row-sub">{t('anim.backendHint')}</span></div><select className="search-input" aria-label={t('anim.backendLabel')} value={draft.backend} disabled={!draft.enabled} onChange={(e) => change({ backend: e.target.value as AnimationBackendChoice })}>
          {BACKENDS.map((backend) => <option key={backend} value={backend} disabled={backend === 'native_experimental' && !nativeBackend?.available}>{t(`anim.backend.${backend}` as MessageKey)}</option>)}
        </select></div>
        <div className="row row-pad"><div><label htmlFor="anim-duration" className="row-title">{t('anim.duration', { ms: draft.duration_ms })}</label></div><div className="range-row"><Range id="anim-duration" min="0" max="500" step="10" value={draft.duration_ms} disabled={!draft.enabled} onChange={(e) => change({ duration_ms: Number(e.target.value) })} /><input aria-label={t('anim.durationValue')} type="number" min="0" max="500" value={draft.duration_ms} disabled={!draft.enabled} onChange={(e) => change({ duration_ms: Number(e.target.value) })} /></div></div>
        <div className="row row-pad"><div><strong>{t('anim.easing')}</strong></div><select className="search-input" aria-label={t('anim.easing')} value={draft.easing} disabled={!draft.enabled} onChange={(e) => change({ easing: e.target.value as Easing })}>{EASINGS.map((easing) => <option key={easing} value={easing}>{t(`anim.easing.${easing}` as MessageKey)}</option>)}</select></div>
        <div className="row row-pad"><div><strong>{t('anim.fps')}</strong><span className="row-sub">{t('anim.fpsHint')}</span></div><select className="search-input" aria-label={t('anim.fps')} value={draft.preferred_fps} disabled={!draft.enabled} onChange={(e) => change({ preferred_fps: Number(e.target.value) })}>{(FPS.includes(draft.preferred_fps) ? FPS : [...FPS, draft.preferred_fps]).map((fps) => <option key={fps} value={fps}>{fps}</option>)}</select></div>
        <div className="row row-pad"><div><strong>{t('anim.reduceMotion')}</strong><span className="row-sub">{t('anim.reduceMotionHint')}</span></div><input aria-label={t('anim.reduceMotion')} type="checkbox" checked={draft.respect_reduce_motion} disabled={!draft.enabled} onChange={(e) => change({ respect_reduce_motion: e.target.checked })} /></div>
        <div className="row row-pad"><div><strong>{t('anim.slide')}</strong><span className="row-sub">{t('anim.slideHint')}</span></div><select className="search-input" aria-label={t('anim.slide')} value={draft.scratchpad_slide} disabled={!draft.enabled || !draft.events.scratchpad} onChange={(e) => change({ scratchpad_slide: e.target.value as SlideDirection })}>{SLIDES.map((slide) => <option key={slide} value={slide}>{t(`anim.slide.${slide}` as MessageKey)}</option>)}</select></div>
      </Card>
      <Card title={t('anim.events')} description={t('anim.eventsHint')} className="padded stack">
        {EVENTS.map((event) => <div key={event} className="row row-pad"><div><strong>{t(`anim.event.${event}` as MessageKey)}</strong></div><input aria-label={t(`anim.event.${event}` as MessageKey)} type="checkbox" checked={draft.events[event]} disabled={!draft.enabled} onChange={(e) => change({ events: { ...draft.events, [event]: e.target.checked } })} /></div>)}
      </Card>
      </div><div>
        <AnimationPreview config={draft} />
        {animations.isError ? <Notice tone="warn">{animations.error.message}</Notice> : animations.data ? <BackendPanel status={animations.data} /> : <Loading />}
      </div></div>
      <Notice>{t('anim.notice')}</Notice>
    </>}
  </>
}
