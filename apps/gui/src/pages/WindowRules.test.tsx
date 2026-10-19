import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { WindowRules } from './WindowRules'

const harness = vi.hoisted(() => ({ offline: false, mutate: vi.fn(), reset: vi.fn() }))
vi.mock('../api/hooks', () => ({
  useDaemon: (kind: string) => {
    if (kind === 'status') return { isSuccess: !harness.offline, isPending: false, refetch: vi.fn() }
    if (kind === 'config') return { isPending: false, isError: false, data: { revision: 'abc', config: { window_rules: [] } } }
    return { isPending: false, isError: false, data: { windows: [{ application_id: 'com.example.Editor' }] } }
  },
  useConfigPatch: () => ({ mutate: harness.mutate, reset: harness.reset, isPending: false, isError: false, isSuccess: false }),
}))

describe('Window Rules form', () => {
  let host: HTMLDivElement
  let root: Root
  beforeEach(() => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
    harness.offline = false
    harness.mutate.mockReset()
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
  })
  afterEach(() => { act(() => root.unmount()); host.remove() })

  it('shows a disconnected state instead of editable controls', () => {
    harness.offline = true
    act(() => root.render(<WindowRules />))
    expect(host.textContent).toContain('Connect to the daemon to edit rules')
    expect((host.querySelector('button') as HTMLButtonElement).disabled).toBe(true)
  })

  it('opens the rule form with a disabled save until a bundle ID is entered', () => {
    act(() => root.render(<WindowRules />))
    const create = [...host.querySelectorAll('button')].find((button) => button.textContent?.includes('New rule'))!
    act(() => create.click())
    expect(host.querySelector('#rule-app')).toBeTruthy()
    expect(host.querySelector('#rule-mode')).toBeTruthy()
    const save = [...host.querySelectorAll('button')].find((button) => button.textContent?.includes('Save rule'))!
    expect(save.disabled).toBe(true)
    expect(harness.mutate).not.toHaveBeenCalled()
  })
})
