import { useLayoutEffect, useRef, useState } from 'react'
import type { ButtonHTMLAttributes, CSSProperties, InputHTMLAttributes, PropsWithChildren, ReactNode } from 'react'
import { animate, utils } from 'animejs'
import type { LucideIcon } from 'lucide-react'
import { AlertCircle, AlertTriangle, Inbox, Info, LoaderCircle } from 'lucide-react'
import { useT } from '../i18n'

type ButtonVariant = 'primary' | 'secondary' | 'danger' | 'ghost'
export function Button({ children, variant = 'secondary', size, className = '', ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant; size?: 'sm' | 'icon' | 'block' }) {
  return <button className={`button button-${variant}${size ? ` button-${size}` : ''} ${className}`.trim()} {...props}>{children}</button>
}
/** A surface card. With a title it gets the header strip from the design: title, hint and an aside on the right. */
export function Card({ children, className = '', title, description, aside }: PropsWithChildren<{ className?: string; title?: ReactNode; description?: ReactNode; aside?: ReactNode }>) {
  return <section className={`card ${className}`.trim()}>
    {title !== undefined && <header className="card-header"><div><h2>{title}</h2>{description && <p>{description}</p>}</div>{aside}</header>}
    {children}
  </section>
}
export function SectionHeader({ title, description, aside }: { title: string; description?: string; aside?: ReactNode }) {
  return <div className="section-header"><div><h2>{title}</h2>{description && <p>{description}</p>}</div>{aside}</div>
}
export function PageHeading({ eyebrow, title, intro, actions }: { eyebrow: string; title: string; intro?: ReactNode; actions?: ReactNode }) {
  return <div className="page-heading"><div><span className="eyebrow">{eyebrow}</span><h1>{title}</h1>{intro && <p>{intro}</p>}</div>{actions && <div className="page-actions">{actions}</div>}</div>
}
export type Tone = 'neutral' | 'good' | 'warn' | 'bad' | 'accent' | 'info'
export function Badge({ children, tone = 'neutral' }: PropsWithChildren<{ tone?: Tone }>) {
  return <span className={`badge badge-${tone}`}>{children}</span>
}
export function Notice({ children, tone = 'info' }: PropsWithChildren<{ tone?: 'info' | 'warn' | 'bad' }>) {
  const Icon = tone === 'warn' ? AlertTriangle : tone === 'bad' ? AlertCircle : Info
  return <div className={`notice notice-${tone}`}><Icon size={15} /> <span>{children}</span></div>
}
export function Empty({ title, children }: PropsWithChildren<{ title: string }>) {
  return <div className="empty"><Inbox size={25} /><strong>{title}</strong><span>{children}</span></div>
}
export function Loading() { const t = useT(); return <div className="center-state"><LoaderCircle size={22} className="spin" /><span>{t('common.loading')}</span></div> }
export function ErrorState({ message, retry }: { message: string; retry?: () => void }) {
  const t = useT()
  return <div className="center-state"><AlertCircle size={24} /><strong>{t('common.loadFailed')}</strong><span>{message}</span>{retry && <Button onClick={retry}>{t('common.tryAgain')}</Button>}</div>
}
export function Metric({ label, value, detail, icon: Icon, detailIcon: DetailIcon, tone }: { label: string; value: ReactNode; detail?: string; icon?: LucideIcon; detailIcon?: LucideIcon; tone?: 'good' }) {
  return <Card className="metric"><span className="metric-label">{Icon && <Icon size={15} />}{label}</span><strong>{value}</strong>{detail && <small className={tone === 'good' ? 'good-text' : ''}>{DetailIcon && <DetailIcon size={12} />}{detail}</small>}</Card>
}
/** A slider whose filled part follows the value (WebKit has no native progress track). */
export function Range({ style, ...props }: Omit<InputHTMLAttributes<HTMLInputElement>, 'type'>) {
  const min = Number(props.min ?? 0), max = Number(props.max ?? 100), value = Number(props.value ?? min)
  const fill = max > min ? Math.min(100, Math.max(0, (value - min) / (max - min) * 100)) : 0
  return <input type="range" {...props} style={{ ...style, '--fill': `${fill}%` } as CSSProperties} />
}
/**
 * Pill switch between a few values. The orange thumb slides to the chosen
 * option (anime.js); resizes and Reduce Motion snap it without animating.
 */
export function Segmented<V extends string>({ options, value, onChange, label, disabled }: { options: { value: V; label: ReactNode }[]; value: V; onChange: (value: V) => void; label: string; disabled?: boolean }) {
  const group = useRef<HTMLDivElement>(null)
  const thumb = useRef<HTMLSpanElement>(null)
  const placed = useRef(false)
  useLayoutEffect(() => {
    const root = group.current, knob = thumb.current
    if (!root || !knob) return
    const place = (animated: boolean) => {
      const active = root.querySelector<HTMLElement>('button[aria-pressed="true"]')
      if (!active) { utils.set(knob, { opacity: 0 }); placed.current = false; return }
      const target = { x: active.offsetLeft, width: active.offsetWidth, opacity: 1 }
      const still = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
      if (!animated || !placed.current || still) utils.set(knob, target)
      else animate(knob, { ...target, duration: 340, ease: 'out(4)' })
      placed.current = true
    }
    place(true)
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(() => place(false))
    observer.observe(root)
    return () => observer.disconnect()
  }, [value])
  return <div ref={group} className="segmented" role="group" aria-label={label}>
    <span ref={thumb} className="segmented-thumb" aria-hidden />
    {options.map((option) => <button key={option.value} type="button" className={option.value === value ? 'active' : ''} aria-pressed={option.value === value} disabled={disabled} onClick={() => onChange(option.value)}>{option.label}</button>)}
  </div>
}
/** A wrapper that slides and fades its content in when it mounts (anime.js). */
export function Appear({ children, style }: PropsWithChildren<{ style?: CSSProperties }>) {
  const box = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    const node = box.current
    if (!node || window.matchMedia?.('(prefers-reduced-motion: reduce)').matches) return
    const animation = animate(node, { opacity: [0, 1], x: [28, 0], scale: [0.98, 1], duration: 380, ease: 'out(4)' })
    return () => { animation.revert() }
  }, [])
  return <div ref={box} style={style}>{children}</div>
}
const KEY_GLYPHS: Record<string, string> = { command: '⌘', control: '⌃', option: '⌥', shift: '⇧', left: '←', right: '→', up: '↑', down: '↓', grave: '`', space: 'Space', tab: 'Tab', return: '↩', escape: 'Esc' }
/** A chord such as "option+shift+3" drawn as keycaps. */
export function Keys({ chord }: { chord: string }) {
  return <span className="keys" aria-label={chord}>{chord.split('+').map((key, index) => <kbd key={index} className="kbd">{KEY_GLYPHS[key] ?? (key.length === 1 ? key.toUpperCase() : key)}</kbd>)}</span>
}
/**
 * A button that asks inline before acting: the first click shows the prompt
 * with Confirm and Cancel. (WKWebView in Tauri has no `window.confirm`; it
 * returns false at once, so such buttons used to do nothing.)
 */
export function ConfirmButton({ children, prompt, onConfirm, disabled, variant = 'danger', size }: PropsWithChildren<{ prompt: string; onConfirm: () => void; disabled?: boolean; variant?: 'danger' | 'secondary'; size?: 'sm' | 'icon' | 'block' }>) {
  const t = useT()
  const [asking, setAsking] = useState(false)
  if (!asking) return <Button variant={variant} size={size} disabled={disabled} onClick={() => setAsking(true)}>{children}</Button>
  return <div className={`confirm-inline${size === 'block' ? ' block' : ''}`} role="group" aria-label={prompt}>
    <span>{prompt}</span>
    <div className="list-actions">
      <Button size="sm" variant="ghost" onClick={() => setAsking(false)}>{t('common.cancel')}</Button>
      <Button size="sm" variant={variant === 'danger' ? 'danger' : 'primary'} disabled={disabled} autoFocus onClick={() => { setAsking(false); onConfirm() }}>{t('common.confirm')}</Button>
    </div>
  </div>
}
