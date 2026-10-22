import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Onboarding } from './Onboarding'
import type { GuiPreferences } from '../api/bridge'

const harness = vi.hoisted(() => ({ trusted: false, running: false, save: vi.fn(), control: vi.fn(), prompt: vi.fn() }))
vi.mock('../api/bridge', () => ({
  appInfo: () => Promise.resolve({ version: '0.1.0', bundled_daemon: '/Applications/Tylora.app/Contents/MacOS/tylora', accessibility_trusted: harness.trusted, login_item: 'not_registered', daemon_version: null, daemon_executable: null }),
  requestAccessibility: () => { harness.prompt(); return Promise.resolve(false) },
  openSystemPane: () => Promise.resolve(),
  controlDaemon: (op: string) => { harness.control(op); return Promise.resolve({}) },
  saveGuiPreferences: (prefs: GuiPreferences) => { harness.save(prefs); return Promise.resolve(prefs) },
}))
vi.mock('../api/hooks', () => ({
  useDaemon: () => harness.running
    ? { isSuccess: true, data: { daemon_version: '0.1.0', native_spaces: [{ id: 4, desktop: 1, display: 1, active: true }] } }
    : { isSuccess: false, data: undefined },
}))

const prefs: GuiPreferences = { version: 1, launch_at_login: false, start_daemon_automatically: false, open_settings_at_launch: true, show_menu_bar_icon: true, onboarding_completed: false, check_for_updates: true, last_seen_version: null }

describe('Onboarding', () => {
  let host: HTMLDivElement
  let root: Root
  const button = (label: string) => [...host.querySelectorAll('button')].find((node) => node.textContent?.trim() === label || node.textContent?.includes(label))
  const click = async (label: string) => { await act(async () => { button(label)?.click(); await Promise.resolve() }) }
  const settle = () => act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
  beforeEach(async () => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
    Object.assign(harness, { trusted: false, running: false })
    for (const fn of [harness.save, harness.control, harness.prompt]) fn.mockReset()
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
  })
  afterEach(() => { act(() => root.unmount()); host.remove() })
  const render = async (onDone = vi.fn()) => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    await act(async () => { root.render(<QueryClientProvider client={client}><Onboarding prefs={prefs} onDone={onDone} /></QueryClientProvider>) })
    await settle()
    return onDone
  }

  it('walks forward and back through every step without starting anything', async () => {
    await render()
    expect(host.textContent).toContain('Welcome to Tylora')
    await click('Next')
    await settle()
    expect(host.textContent).toContain('Allow Accessibility')
    expect(host.textContent).toContain('Permission needed')
    await click('Back')
    expect(host.textContent).toContain('Welcome to Tylora')
    for (let step = 0; step < 4; step++) await click('Next')
    expect(host.textContent).toContain('Start Tylora')
    expect(host.textContent).toContain('starting will fail')
    expect(harness.control).not.toHaveBeenCalled()
    expect(harness.prompt).not.toHaveBeenCalled()
  })

  it('reports granted Accessibility only when macOS says so', async () => {
    harness.trusted = true
    await render()
    await click('Next')
    await settle()
    expect(host.textContent).toContain('Granted')
    expect(button('Request access')).toBeUndefined()
  })

  it('finishes with opt-in choices left off unless chosen', async () => {
    const onDone = await render()
    for (let step = 0; step < 4; step++) await click('Next')
    const login = host.querySelector<HTMLInputElement>('input[aria-label="Open Tylora at login"]')
    expect(login?.checked).toBe(false)
    await click('Finish')
    await settle()
    expect(harness.save).toHaveBeenCalledWith({ ...prefs, onboarding_completed: true })
    expect(onDone).toHaveBeenCalled()
  })

  it('starts the daemon only on an explicit click', async () => {
    harness.trusted = true
    await render()
    for (let step = 0; step < 4; step++) await click('Next')
    await click('Start daemon')
    await settle()
    expect(harness.control).toHaveBeenCalledWith('start')
  })

  it('can be skipped from the first step', async () => {
    const onDone = await render()
    await click('Skip setup')
    await settle()
    expect(harness.save).toHaveBeenCalledWith({ ...prefs, onboarding_completed: true })
    expect(onDone).toHaveBeenCalled()
  })
})
