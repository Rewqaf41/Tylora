import { useEffect, useRef, useState } from 'react'
import { Check, Keyboard } from 'lucide-react'
import { pauseHotkeys, resumeHotkeys } from '../api/bridge'
import { useT } from '../i18n'
import { heldModifiers, rejectionReason, shortcutFromCode } from './hotkey'

const SYMBOLS: Record<string, string> = { command: '⌘', control: '⌃', option: '⌥', shift: '⇧', left: '←', right: '→', up: '↑', down: '↓', tab: '⇥', grave: '`' }

/** `option+shift+1` → `⌥ ⇧ 1` for display. */
export function chordKeys(chord: string): string[] {
  return chord.split('+').filter(Boolean).map((part) => SYMBOLS[part] ?? part.toUpperCase())
}

/**
 * Records a physical key combination. While recording, the daemon releases
 * its global hotkeys so combinations already bound by Tylora reach this field.
 * Keys are read on the window: WebKit does not focus a button on click, so a
 * button-level key handler would never fire.
 */
export function HotkeyRecorder({ id, value, onChange, clearable = false }: { id?: string; value: string | null; onChange: (chord: string | null) => void; clearable?: boolean }) {
  const t = useT()
  const [recording, setRecording] = useState(false)
  const [held, setHeld] = useState('')
  const [message, setMessage] = useState<{ tone: 'good' | 'bad'; text: string } | null>(null)
  const active = useRef(false)
  const button = useRef<HTMLButtonElement>(null)
  const change = useRef(onChange)
  change.current = onChange

  const stop = () => {
    if (!active.current) return
    active.current = false
    setRecording(false); setHeld('')
    void resumeHotkeys().catch(() => undefined)
  }
  const start = () => {
    if (active.current) return
    active.current = true
    setRecording(true); setHeld(''); setMessage(null)
    button.current?.focus()
    void pauseHotkeys().catch(() => undefined)
  }

  useEffect(() => {
    if (!recording) return
    const modifiers = (event: KeyboardEvent) => ({ meta: event.metaKey, ctrl: event.ctrlKey, alt: event.altKey, shift: event.shiftKey })
    const down = (event: KeyboardEvent) => {
      event.preventDefault(); event.stopPropagation()
      if (event.code === 'Escape') { stop(); setMessage(null); return }
      if (clearable && event.code === 'Backspace' && !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey) { change.current(null); stop(); setMessage({ tone: 'good', text: t('recorder.cleared') }); return }
      const captured = shortcutFromCode(event.code, modifiers(event))
      if (captured) { change.current(captured); stop(); setMessage({ tone: 'good', text: t('recorder.recorded') }); return }
      setHeld(heldModifiers(modifiers(event)))
      const reason = rejectionReason(event.code, modifiers(event), t)
      setMessage(reason ? { tone: 'bad', text: reason } : null)
    }
    const up = (event: KeyboardEvent) => { event.preventDefault(); setHeld(heldModifiers(modifiers(event))) }
    // A click anywhere else cancels, like losing focus would.
    const outside = (event: MouseEvent) => { if (!button.current?.contains(event.target as Node)) stop() }
    window.addEventListener('keydown', down, true)
    window.addEventListener('keyup', up, true)
    window.addEventListener('mousedown', outside, true)
    return () => {
      window.removeEventListener('keydown', down, true)
      window.removeEventListener('keyup', up, true)
      window.removeEventListener('mousedown', outside, true)
    }
  }, [recording, clearable, t])
  useEffect(() => () => { if (active.current) void resumeHotkeys().catch(() => undefined) }, [])

  const shown = recording ? held : value
  return <div className="stack" style={{ gap: 6 }}>
    <button ref={button} id={id} type="button" aria-label={t('recorder.record')} className={`button ${recording ? 'button-primary' : ''}`} onClick={() => (recording ? stop() : start())}>
      {!recording && message?.tone === 'good' ? <Check size={14} /> : <Keyboard size={14} />}
      {shown ? <span className="mono" data-chord={shown}>{chordKeys(shown).join(' ')}{recording ? ' …' : ''}</span>
        : <span>{recording ? `${t('recorder.press')}${clearable ? t('recorder.clears') : ''}` : t('recorder.record')}</span>}
    </button>
    {recording ? <span className="row-sub">{t('recorder.paused')}</span>
      : value && <span className="row-sub mono">{value}</span>}
    {message && <span className={message.tone === 'good' ? 'good-text' : 'bad-text'} role="status">{message.text}</span>}
  </div>
}
