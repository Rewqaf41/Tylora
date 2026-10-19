import { act, useState } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { chordKeys, HotkeyRecorder } from './HotkeyRecorder'
import { rejectionReason } from './hotkey'

const bridge = vi.hoisted(() => ({ pause: vi.fn(async () => ({})), resume: vi.fn(async () => ({})) }))
vi.mock('../api/bridge', () => ({ pauseHotkeys: bridge.pause, resumeHotkeys: bridge.resume }))

describe('HotkeyRecorder', () => {
  let host: HTMLDivElement
  let root: Root
  beforeEach(() => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
    bridge.pause.mockClear(); bridge.resume.mockClear()
    host = document.createElement('div'); document.body.append(host); root = createRoot(host)
  })
  afterEach(() => { act(() => root.unmount()); host.remove() })
  // WebKit leaves a clicked button unfocused, so keys arrive at the window.
  const press = (_button: Element, init: KeyboardEventInit) => act(() => { window.dispatchEvent(new KeyboardEvent('keydown', init)) })
  function Stateful({ onChange }: { onChange: (chord: string | null) => void }) {
    const [value, setValue] = useState<string | null>(null)
    return <HotkeyRecorder value={value} onChange={(chord) => { setValue(chord); onChange(chord) }} />
  }

  it('pauses daemon hotkeys, shows feedback and records a combination', () => {
    const onChange = vi.fn()
    act(() => root.render(<Stateful onChange={onChange} />))
    const button = host.querySelector('button')!
    act(() => button.click())
    expect(bridge.pause).toHaveBeenCalledOnce()
    expect(host.textContent).toContain('paused while recording')
    press(button, { code: 'AltLeft', altKey: true })
    expect(button.textContent).toContain('⌥ …')
    press(button, { code: 'KeyA' })
    expect(host.textContent).toContain('Hold at least one modifier')
    expect(onChange).not.toHaveBeenCalled()
    press(button, { code: 'Digit1', altKey: true, shiftKey: true })
    expect(onChange).toHaveBeenCalledWith('option+shift+1')
    expect(button.textContent).toContain('⌥ ⇧ 1')
    expect(host.textContent).toContain('Shortcut recorded')
    expect(bridge.resume).toHaveBeenCalledOnce()
  })

  it('a click elsewhere cancels and resumes hotkeys', () => {
    act(() => root.render(<Stateful onChange={vi.fn()} />))
    act(() => host.querySelector('button')!.click())
    act(() => { document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true })) })
    expect(bridge.resume).toHaveBeenCalledOnce()
    expect(host.textContent).toContain('Record shortcut')
  })

  it('explains unsupported keys', () => {
    expect(chordKeys('control+grave')).toEqual(['⌃', '`'])
    expect(rejectionReason('ShiftLeft', { meta: false, ctrl: false, alt: false, shift: true })).toBeNull()
    expect(rejectionReason('Space', { meta: false, ctrl: false, alt: true, shift: false })).toContain('not supported')
  })
})
