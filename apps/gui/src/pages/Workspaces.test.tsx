import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Workspaces } from './Workspaces'

const harness = vi.hoisted(() => ({ patch: vi.fn(), workspaces: [] as unknown[] }))
vi.mock('@tanstack/react-query', () => ({
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
  useMutation: () => ({ mutate: vi.fn(), isPending: false, isError: false }),
}))
vi.mock('../api/bridge', () => ({ createDesktop: vi.fn() }))
vi.mock('../api/hooks', () => ({
  useAction: () => ({ mutate: vi.fn(), isPending: false, isError: false }),
  useConfigPatch: () => ({ mutate: harness.patch, isPending: false, isError: false, isSuccess: false }),
  useDaemon: (kind: string) => {
    if (kind === 'status') return { isSuccess: true, isFetched: true }
    if (kind === 'spaces') return { isPending: false, isError: false, data: { spaces: [{ id: 7, desktop: 1, display: 1, active: true }, { id: 9, desktop: 2, display: 1, active: false }] } }
    if (kind === 'config') return { isPending: false, isError: false, data: { revision: 'r1', config: { workspaces: harness.workspaces } } }
    if (kind === 'scratchpads') return { data: { parking: {} } }
    return { data: undefined }
  },
}))

describe('Workspaces page', () => {
  let host: HTMLDivElement
  let root: Root
  const rows = () => [...host.querySelectorAll('.ws-row')]
  const button = (row: Element, label: string) => [...row.querySelectorAll('button')].find((node) => node.textContent?.includes(label))
  const type = (input: HTMLInputElement, value: string) => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!
    setter.call(input, value)
    input.dispatchEvent(new Event('input', { bubbles: true }))
  }
  beforeEach(() => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
    harness.patch.mockReset()
    harness.workspaces = [{ id: 1, name: 'Browser', display: null, native_space_id: 7, native_space_ids: [] }]
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
  })
  afterEach(() => { act(() => root.unmount()); host.remove() })

  it('offers a new workspace on a free Desktop and creates it by name', () => {
    act(() => root.render(<Workspaces />))
    const free = rows()[1]!
    // No name field until a name is needed; here every workspace is taken.
    const input = free.querySelector<HTMLInputElement>('input')!
    expect(input.placeholder).toBe('Workspace name')
    act(() => type(input, 'Docs'))
    act(() => button(free, 'Create workspace')?.click())
    expect(harness.patch).toHaveBeenCalledWith({ revision: 'r1', patch: { kind: 'create_workspace', name: 'Docs', display: null, native_space_id: 9 } })
  })

  it('renames through the Rename button only', () => {
    act(() => root.render(<Workspaces />))
    const bound = rows()[0]!
    expect(bound.querySelector('input')).toBeNull()
    act(() => button(bound, 'Rename')?.click())
    const input = bound.querySelector<HTMLInputElement>('input')!
    act(() => type(input, 'Web'))
    act(() => button(bound, 'Rename')?.click())
    expect(harness.patch).toHaveBeenCalledWith({ revision: 'r1', patch: expect.objectContaining({ kind: 'set_workspace', id: 1, name: 'Web', native_space_id: 7 }) })
  })
})
