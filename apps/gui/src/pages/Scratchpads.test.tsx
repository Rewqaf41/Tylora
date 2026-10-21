import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { blankScratchpad, previewFrame, scratchpadProblem, scratchpadSlug, Scratchpads } from './Scratchpads'

const harness = vi.hoisted(() => ({ offline: false, mutate: vi.fn(), action: vi.fn(), assign: vi.fn(), other: vi.fn() }))
const pad = (id: string, extra: Record<string, unknown> = {}) => ({ ...{ id, name: '', hotkey: 'option+grave', scope: 'global', width: 0.7, height: 0.65, alignment: 'center', offset_x: 0, offset_y: 0, target: 'pointer', remember_position: true, remember_size: true, hide: 'park' }, ...extra })
vi.mock('@tanstack/react-query', () => ({
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
  useMutation: ({ mutationFn }: { mutationFn: unknown }) => {
    const name = String(mutationFn)
    const mutate = name.includes('scratchpadAction') ? harness.action : name.includes('assignScratchpad') ? harness.assign : harness.other
    return { mutate, reset: vi.fn(), isPending: false, isError: false, isSuccess: false, data: undefined }
  },
}))
vi.mock('../api/bridge', () => ({
  scratchpadAction: (id: string, verb: string) => ['scratchpad_action', id, verb],
  assignScratchpad: (id: string, window: number | null, force: boolean) => ['assign_scratchpad', id, window, force],
  unassignScratchpad: 'unassign', recoverScratchpads: 'recover',
}))
vi.mock('../api/hooks', () => ({
  useDaemon: (kind: string) => {
    if (kind === 'status') return { isSuccess: !harness.offline, isPending: false, isFetched: true, refetch: vi.fn() }
    if (kind === 'config') return { isPending: false, isError: false, data: { revision: 'abc', config: { hotkeys: [{ chord: 'option+1', command: { command: 'focus_workspace', id: 1 }, enabled: true }], scratchpads: [pad('term'), pad('notes', { hotkey: null, scope: 'local' })] } } }
    if (kind === 'scratchpads') return { data: { version: 1, scratchpads: [{ id: 'term', name: 'term', state: 'hidden', manual: true, window: { process: { pid: 7, epoch: 1 }, cg_window_id: 900, bundle_id: 'com.apple.Terminal', title: 'zsh' }, last_error: null }, { id: 'notes', name: 'notes', state: 'unassigned', manual: false, window: null, last_error: null }], parking: {}, metrics: {} } }
    if (kind === 'scratchpad_capabilities') return { isPending: false, data: { version: 1, park: { available: true, mechanism: '', parking_desktops: {} }, global: { available: true, target: '' }, minimize: { available: true, mechanism: '' }, sticky: { available: false, reason: 'requires SIP changes' }, create_parking_desktop: { mechanism: '' }, reasons: [] } }
    if (kind === 'windows') return { data: { windows: [{ pid: 7, token: 1, cg_window_id: 900, application_id: 'com.apple.Terminal', title: 'zsh', native_space_id: 4, display: 1, workspace: null, floating: false }] } }
    if (kind === 'apps') return { isPending: false, data: { apps: [{ pid: 7, bundle_id: 'com.apple.Terminal', name: 'Terminal', windows: 1 }] } }
    if (kind === 'displays') return { data: { displays: [{ id: 1, primary: true, full_frame: { x: 0, y: 0, width: 1000, height: 800 }, work_area: { x: 0, y: 25, width: 1000, height: 775 } }] } }
    return { data: undefined }
  },
  useConfigPatch: () => ({ mutate: harness.mutate, reset: vi.fn(), isPending: false, isError: false, isSuccess: false }),
}))

describe('Scratchpads page', () => {
  let host: HTMLDivElement
  let root: Root
  beforeEach(() => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
    harness.offline = false
    for (const fn of [harness.mutate, harness.action, harness.assign, harness.other]) fn.mockReset()
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
  })
  afterEach(() => { act(() => root.unmount()); host.remove() })
  const button = (text: string) => [...host.querySelectorAll('button')].filter((node) => node.textContent?.includes(text))

  it('shows a disconnected state', () => {
    harness.offline = true
    act(() => root.render(<Scratchpads />))
    expect(host.textContent).toContain('Connect to the daemon to edit scratchpads')
  })

  it('lists scratchpads with state and toggles an assigned one', () => {
    act(() => root.render(<Scratchpads />))
    expect(host.textContent).toContain('Hidden')
    expect(host.textContent).toContain('No window')
    const [toggleTerm, toggleNotes] = button('Toggle')
    expect(toggleNotes!.disabled).toBe(true)
    act(() => toggleTerm!.click())
    expect(harness.action).toHaveBeenCalledWith({ id: 'term', verb: 'toggle' })
  })

  it('assigns the focused window without naming one', () => {
    act(() => root.render(<Scratchpads />))
    act(() => button('Assign Focused Window')[1]!.click())
    expect(harness.assign).toHaveBeenCalledWith({ id: 'notes', window: null, force: true })
  })

  it('creates a scratchpad through a config patch', () => {
    act(() => root.render(<Scratchpads />))
    act(() => button('New scratchpad')[0]!.click())
    const name = host.querySelector('#pad-name') as HTMLInputElement
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!
    act(() => { setter.call(name, 'Music Player'); name.dispatchEvent(new Event('input', { bubbles: true })) })
    expect((host.querySelector('#pad-id') as HTMLInputElement).value).toBe('music-player')
    expect(host.querySelector('[aria-label="Scratchpad geometry preview"]')).not.toBeNull()
    act(() => button('Save scratchpad')[0]!.click())
    expect(harness.mutate.mock.calls[0]![0].patch).toMatchObject({ kind: 'set_scratchpad', previous_id: null, scratchpad: { id: 'music-player', name: 'Music Player', width: 0.7, height: 0.65, alignment: 'center', hide: 'park' } })
  })

  it('deletes a scratchpad', () => {
    act(() => root.render(<Scratchpads />))
    act(() => (host.querySelector('[aria-label="Delete notes"]') as HTMLButtonElement).click())
    expect(harness.mutate.mock.calls[0]![0].patch).toEqual({ kind: 'remove_scratchpad', id: 'notes' })
  })
})

describe('scratchpad helpers', () => {
  it('validates like the daemon', () => {
    const ok = { ...blankScratchpad, id: 'term' }
    expect(scratchpadProblem(ok, [], [])).toBeNull()
    expect(scratchpadProblem({ ...ok, id: 'has space' }, [], [])).not.toBeNull()
    expect(scratchpadProblem({ ...ok, id: 'x'.repeat(33) }, [], [])).not.toBeNull()
    expect(scratchpadProblem(ok, [{ ...ok, id: 'TERM' }], [])).toContain('already uses')
    expect(scratchpadProblem({ ...ok, width: 0.05 }, [], [])).not.toBeNull()
    expect(scratchpadProblem({ ...ok, offset_x: 5000 }, [], [])).not.toBeNull()
    expect(scratchpadProblem({ ...ok, hotkey: 'option+1' }, [], ['Option+1'])).toContain('already bound')
    expect(scratchpadSlug('My Term!')).toBe('my-term')
  })
  it('places the frame like the daemon', () => {
    const area = { x: 0, y: 25, width: 1000, height: 775 }
    expect(previewFrame(blankScratchpad, area)).toEqual({ x: 150, y: 25 + 775 * 0.175, width: 700, height: 775 * 0.65 })
    const corner = previewFrame({ ...blankScratchpad, alignment: 'bottom_right', offset_x: 500 }, area)
    expect(corner.x + corner.width).toBe(1000)
    expect(corner.y + corner.height).toBeCloseTo(800)
  })
})
