import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { animationProblem, Animations, DEFAULT_ANIMATIONS } from './Animations'

const harness = vi.hoisted(() => ({ offline: false, legacy: false, mutate: vi.fn() }))
vi.mock('../api/hooks', () => ({
  useDaemon: (kind: string) => {
    if (kind === 'status') return { isSuccess: !harness.offline, isPending: false, isFetched: true, refetch: vi.fn() }
    if (kind === 'config') return { isPending: false, isError: false, data: { revision: 'rev1', config: harness.legacy ? {} : { animations: structuredClone(DEFAULT_ANIMATIONS) } } }
    if (kind === 'animations') return {
      isError: false,
      data: {
        version: 1, config: DEFAULT_ANIMATIONS, reduce_motion: false,
        backends: [
          { name: 'instant', available: true, reason: null },
          { name: 'accessibility', available: true, reason: null },
          { name: 'native_experimental', available: false, reason: 'WindowServer ignores other apps’ windows' },
        ],
        worker: { policy_enabled: true, backend: 'accessibility', reduce_motion: false, accessibility: true, slow_applications: [], instant_reasons: {} },
        metrics: { started: 3, completed: 3, retargeted: 0, cancelled: 0, failed: 0, instant: 1, steps_written: 20, skipped_slots: 2, ticks: 19, writes: 20, active: 0, final_error_max: 0, finals_measured: 1 },
        summary: { active: 0, mean_write_ms: 7.9, max_write_ms: 29.3, mean_tick_interval_ms: 22.3, mean_final_error_pt: 0 },
      },
    }
    return { data: undefined }
  },
  useConfigPatch: () => ({ mutate: harness.mutate, reset: vi.fn(), isPending: false, isError: false, isSuccess: false }),
}))

describe('Animations page', () => {
  let host: HTMLDivElement
  let root: Root
  beforeEach(() => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
    harness.offline = false
    harness.legacy = false
    harness.mutate.mockReset()
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
  })
  afterEach(() => { act(() => root.unmount()); host.remove() })
  const button = (text: string) => [...host.querySelectorAll('button')].find((node) => node.textContent?.includes(text))!
  const labelled = <T extends Element>(label: string) => host.querySelector<T>(`[aria-label="${label}"]`)!
  const setValue = (element: HTMLInputElement | HTMLSelectElement, value: string) => act(() => {
    const prototype = element instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype
    Object.getOwnPropertyDescriptor(prototype, 'value')!.set!.call(element, value)
    element.dispatchEvent(new Event(element instanceof HTMLSelectElement ? 'change' : 'input', { bubbles: true }))
  })

  it('shows a disconnected state', () => {
    harness.offline = true
    act(() => root.render(<Animations />))
    expect(host.textContent).toContain('Connect to the daemon to edit animation settings')
  })

  it('saves edited settings as one set_animations patch', () => {
    act(() => root.render(<Animations />))
    expect(button('Save animations').disabled).toBe(true)
    setValue(labelled<HTMLSelectElement>('Easing'), 'ease_out_expo')
    act(() => labelled<HTMLInputElement>('Scrolling strip').click())
    act(() => button('Save animations').click())
    expect(harness.mutate).toHaveBeenCalledTimes(1)
    const [{ revision, patch }] = harness.mutate.mock.calls[0] as [{ revision: string; patch: { kind: string; animations: typeof DEFAULT_ANIMATIONS } }]
    expect(revision).toBe('rev1')
    expect(patch.kind).toBe('set_animations')
    expect(patch.animations.easing).toBe('ease_out_expo')
    expect(patch.animations.events.scrolling).toBe(false)
    expect(patch.animations.events.window_open).toBe(true)
  })

  it('blocks invalid durations and explains the unsupported native backend', () => {
    act(() => root.render(<Animations />))
    setValue(labelled<HTMLInputElement>('Duration in milliseconds'), '900')
    expect(host.textContent).toContain('Duration must be a whole number from 0 to 500 ms')
    expect(button('Save animations').disabled).toBe(true)
    const native = [...labelled<HTMLSelectElement>('Backend').options].find((option) => option.value === 'native_experimental')!
    expect(native.disabled).toBe(true)
    expect(host.textContent).toContain('WindowServer ignores other apps’ windows')
    expect(host.textContent).toContain('not how often macOS redrew the windows')
  })

  it('plays the preview without sending anything to the daemon', () => {
    act(() => root.render(<Animations />))
    act(() => button('Play').click())
    expect(harness.mutate).not.toHaveBeenCalled()
    expect(host.textContent).toContain('The preview never moves real windows')
  })

  it('refuses to save to a daemon without animations', () => {
    harness.legacy = true
    act(() => root.render(<Animations />))
    expect(host.textContent).toContain('predates animations')
    act(() => labelled<HTMLInputElement>('Animate windows').click())
    expect(button('Save animations').disabled).toBe(true)
  })

  it('validates like the daemon', () => {
    expect(animationProblem(DEFAULT_ANIMATIONS)).toBe(null)
    expect(animationProblem({ ...DEFAULT_ANIMATIONS, duration_ms: 501 })).toBe('duration')
    expect(animationProblem({ ...DEFAULT_ANIMATIONS, duration_ms: 12.5 })).toBe('duration')
    expect(animationProblem({ ...DEFAULT_ANIMATIONS, preferred_fps: 14 })).toBe('fps')
    expect(animationProblem({ ...DEFAULT_ANIMATIONS, duration_ms: 0 })).toBe(null)
  })
})
