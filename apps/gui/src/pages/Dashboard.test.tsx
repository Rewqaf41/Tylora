import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { MemoryRouter } from 'react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Dashboard } from './Dashboard'

const harness = vi.hoisted(() => ({ fetched: false, fetching: true, openSettings: vi.fn() }))
vi.mock('@tanstack/react-query', () => ({
  useQuery: () => ({ data: { accessibility_trusted: false } }),
  useMutation: () => ({ mutate: vi.fn(), isPending: false, isError: false }),
}))
vi.mock('../api/bridge', () => ({
  appInfo: vi.fn(),
  requestAccessibility: vi.fn(),
  openSystemPane: (pane: string) => { harness.openSettings(pane); return Promise.resolve() },
}))
vi.mock('../api/hooks', () => ({
  useDaemon: (kind: string) => kind === 'status'
    ? { isSuccess: false, isFetched: harness.fetched, isPending: harness.fetching, data: undefined, refetch: vi.fn() }
    : { data: undefined },
  useAction: () => ({ mutate: vi.fn(), isPending: false, isError: false }),
}))

describe('Dashboard while the daemon is offline', () => {
  let host: HTMLDivElement
  let root: Root
  beforeEach(() => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
    harness.fetched = false
    harness.fetching = true
    harness.openSettings.mockReset()
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
  })
  afterEach(() => { act(() => root.unmount()); host.remove() })
  const render = () => act(() => root.render(<MemoryRouter><Dashboard /></MemoryRouter>))

  it('keeps permission guidance visible during background status checks', () => {
    render()
    expect(host.textContent).toContain('Loading live data')

    harness.fetched = true
    harness.fetching = false
    render()
    expect(host.textContent).toContain('Open Accessibility settings')
    expect(host.textContent).not.toContain('Loading live data')

    harness.fetching = true
    render()
    expect(host.textContent).toContain('Open Accessibility settings')
    expect(host.textContent).not.toContain('Loading live data')

    const start = [...host.querySelectorAll('button')].find((button) => button.textContent?.includes('Start daemon'))
    expect(start?.disabled).toBe(true)
    const settings = [...host.querySelectorAll('button')].find((button) => button.textContent?.includes('Open Accessibility settings'))
    act(() => settings?.click())
    expect(harness.openSettings).toHaveBeenCalledWith('accessibility')
  })
})
