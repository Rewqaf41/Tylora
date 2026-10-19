import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { DEFAULT_DWINDLE, DEFAULT_SCROLLING } from '../api/types'
import type { LayoutPreview } from '../api/types'
import { Layouts, PreviewCanvas, parsePresets, validScrolling } from './Layouts'

const harness = vi.hoisted(() => ({ patch: vi.fn(), action: vi.fn() }))
const window = (token: number) => ({ process: { pid: 1, epoch: 1 }, token })
vi.mock('@tanstack/react-query', () => ({
  useQuery: () => ({ isPending: false, isError: false, data: { version: 1, layout: 'bsp', frames: [{ slot: 1, rect: { x: 0, y: 0, width: 640, height: 360 } }] } }),
}))
vi.mock('../api/hooks', () => ({
  useDaemon: (kind: string) => {
    if (kind === 'status') return {
      isSuccess: true, isPending: false, refetch: vi.fn(),
      data: {
        native_spaces: [], layout_zoom: [],
        layouts: [{ display: 1, native_space_id: 7, workspace: 1, layout: 'scrolling', windows: 3, scrolling: { mode: 'strip', viewport_offset: 0, strip_width: 2000, usable_width: 1454, active: window(1), state_version: 3, columns: [{ window: window(1), width_fraction: 0.65, strip_x: 0, width: 945, frame: { x: 8, y: 41, width: 945, height: 907 }, placement: { state: 'visible' } }, { window: window(2), width_fraction: 0.65, strip_x: 953, width: 945, frame: { x: 961, y: 41, width: 945, height: 907 }, placement: { state: 'partial' } }, { window: window(3), width_fraction: 0.65, strip_x: 1906, width: 945, frame: { x: 1430, y: 924, width: 945, height: 907 }, placement: { state: 'parked' } }] } }],
      },
    }
    return {
      isPending: false, isError: false,
      data: { revision: 'rev', config: { gaps: { inner: 8, outer: 8 }, zoom: { respect_outer_gaps: false }, default_layout: 'bsp', dwindle: DEFAULT_DWINDLE, scrolling: DEFAULT_SCROLLING, workspaces: [{ id: 1, name: 'Code', display: null, native_space_id: 7, layout: 'scrolling' }, { id: 2, name: 'Web', display: null, native_space_id: 4 }] } },
    }
  },
  useConfigPatch: () => ({ mutate: harness.patch, isPending: false, isError: false, isSuccess: false }),
  useAction: () => ({ mutate: harness.action, isPending: false, isError: false }),
}))

describe('Layouts page', () => {
  let host: HTMLDivElement
  let root: Root
  beforeEach(() => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
    harness.patch.mockReset(); harness.action.mockReset()
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
  })
  afterEach(() => { act(() => root.unmount()); host.remove() })

  it('shows the live layout and switches the current workspace through the daemon', () => {
    act(() => root.render(<Layouts />))
    expect(host.textContent).toContain('Display 1 · Code')
    expect(host.textContent).toContain('1 parked column')
    const group = host.querySelector('[aria-label="Layout for display 1"]')!
    const active = group.querySelector('[aria-pressed="true"]')!
    expect(active.textContent).toBe('Scrolling')
    const dwindle = [...group.querySelectorAll('button')].find((button) => button.textContent === 'Dwindle')!
    act(() => dwindle.click())
    expect(harness.action).toHaveBeenCalledWith({ kind: 'command', command: { command: 'set_layout', layout: 'dwindle' } })
  })

  it('assigns a layout to a workspace with a revisioned config patch', () => {
    act(() => root.render(<Layouts />))
    const select = host.querySelector('[aria-label="Layout for Web"]') as HTMLSelectElement
    expect(select.value).toBe('')
    act(() => { select.value = 'dwindle'; select.dispatchEvent(new Event('change', { bubbles: true })) })
    expect(harness.patch).toHaveBeenCalledWith({ revision: 'rev', patch: { kind: 'set_workspace_layout', id: 2, layout: 'dwindle' } })
    const code = host.querySelector('[aria-label="Layout for Code"]') as HTMLSelectElement
    act(() => { code.value = ''; code.dispatchEvent(new Event('change', { bubbles: true })) })
    expect(harness.patch).toHaveBeenLastCalledWith({ revision: 'rev', patch: { kind: 'set_workspace_layout', id: 1, layout: null } })
  })

  it('keeps Apply disabled until a setting changes', () => {
    act(() => root.render(<Layouts />))
    const apply = [...host.querySelectorAll('button')].find((button) => button.textContent?.includes('Apply Scrolling'))!
    expect(apply.disabled).toBe(true)
  })
})

describe('layout helpers', () => {
  it('validates scrolling widths and presets like the daemon', () => {
    expect(validScrolling(DEFAULT_SCROLLING)).toBeNull()
    expect(validScrolling({ ...DEFAULT_SCROLLING, min_column_width: 0.9 })).not.toBeNull()
    expect(validScrolling({ ...DEFAULT_SCROLLING, column_width_presets: [] })).not.toBeNull()
    expect(validScrolling({ ...DEFAULT_SCROLLING, column_width_presets: [1.5] })).not.toBeNull()
    expect(validScrolling({ ...DEFAULT_SCROLLING, scroll_sensitivity: 0 })).not.toBeNull()
  })
  it('parses percentage presets', () => {
    expect(parsePresets('33, 50%, 65,100')).toEqual([0.33, 0.5, 0.65, 1])
    expect(parsePresets('33, 50', [1 / 3])).toEqual([1 / 3, 0.5])
  })
  it('marks parked columns and draws the viewport over the strip', () => {
    const host = document.createElement('div')
    const root = createRoot(host)
    const preview: LayoutPreview = {
      version: 1, layout: 'scrolling',
      frames: [{ slot: 1, rect: { x: 8, y: 8, width: 400, height: 344 }, placement: { state: 'visible' } }, { slot: 2, rect: { x: 416, y: 8, width: 400, height: 344 }, placement: { state: 'parked' } }],
      scrolling: { mode: 'strip', viewport_offset: 0, strip_width: 808, usable_width: 624, active: window(1), state_version: 1, columns: [{ window: window(1), width_fraction: 0.65, strip_x: 0, width: 400, frame: { x: 8, y: 8, width: 400, height: 344 }, placement: { state: 'visible' } }, { window: window(2), width_fraction: 0.65, strip_x: 408, width: 400, frame: { x: 600, y: 328, width: 400, height: 344 }, placement: { state: 'parked' } }] },
    }
    act(() => root.render(<PreviewCanvas preview={preview} />))
    expect(host.querySelectorAll('.preview-parked')).toHaveLength(1)
    expect(host.querySelector('.strip-viewport')).toBeTruthy()
    expect(host.querySelectorAll('.strip-column')).toHaveLength(2)
    act(() => root.unmount())
  })
})
