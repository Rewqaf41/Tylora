import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { UpdateBanner } from './Updates'
import type { GuiPreferences, UpdateCheck } from '../api/bridge'

const harness = vi.hoisted(() => ({ check: null as UpdateCheck | null, checks: vi.fn(), install: vi.fn(), save: vi.fn(), notes: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: () => Promise.resolve(() => {}) }))
vi.mock('../api/bridge', () => ({
  checkUpdate: () => { harness.checks(); return Promise.resolve(harness.check) },
  installUpdate: () => { harness.install(); return new Promise(() => {}) },
  openReleaseNotes: (version?: string) => { harness.notes(version); return Promise.resolve() },
  saveGuiPreferences: (prefs: GuiPreferences) => { harness.save(prefs); return Promise.resolve(prefs) },
}))

const base: GuiPreferences = { version: 1, launch_at_login: false, start_daemon_automatically: false, open_settings_at_launch: true, show_menu_bar_icon: true, onboarding_completed: true, check_for_updates: true, last_seen_version: __APP_VERSION__ }

describe('UpdateBanner', () => {
  let host: HTMLDivElement
  let root: Root
  const button = (label: string) => [...host.querySelectorAll('button')].find((node) => node.textContent?.includes(label) || node.getAttribute('aria-label') === label)
  const settle = () => act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
  beforeEach(() => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
    harness.check = { current: __APP_VERSION__, available: null }
    for (const fn of [harness.checks, harness.install, harness.save, harness.notes]) fn.mockReset()
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
  })
  afterEach(() => { act(() => root.unmount()); host.remove() })
  const render = async (prefs: GuiPreferences) => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    await act(async () => { root.render(<QueryClientProvider client={client}><UpdateBanner prefs={prefs} /></QueryClientProvider>) })
    await settle()
  }

  it('stays silent when the app is current', async () => {
    await render(base)
    expect(harness.checks).toHaveBeenCalledTimes(1)
    expect(host.textContent).toBe('')
  })

  it('offers a newer release and installs only on click', async () => {
    harness.check = { current: __APP_VERSION__, available: { version: '9.9.9', notes: 'Fixes', date: null } }
    await render(base)
    expect(host.textContent).toContain('Tylora 9.9.9 is available')
    expect(harness.install).not.toHaveBeenCalled()
    await act(async () => { button('Release notes')?.click() })
    expect(harness.notes).toHaveBeenCalledWith('9.9.9')
    await act(async () => { button('Install and restart')?.click() })
    await settle()
    expect(harness.install).toHaveBeenCalledTimes(1)
    expect(host.textContent).toContain('Installing')
  })

  it('does not contact GitHub when checks are off', async () => {
    await render({ ...base, check_for_updates: false })
    expect(harness.checks).not.toHaveBeenCalled()
  })

  it('announces an update once and records the version on dismiss', async () => {
    await render({ ...base, last_seen_version: '0.0.1' })
    expect(host.textContent).toContain(`Tylora was updated to ${__APP_VERSION__}`)
    await act(async () => { button('Dismiss')?.click() })
    await settle()
    expect(harness.save).toHaveBeenCalledWith({ ...base, last_seen_version: __APP_VERSION__ })
  })

  it('records the version silently on a fresh install', async () => {
    await render({ ...base, last_seen_version: null })
    expect(host.textContent).not.toContain('was updated')
    expect(harness.save).toHaveBeenCalledWith({ ...base, last_seen_version: __APP_VERSION__ })
  })
})
