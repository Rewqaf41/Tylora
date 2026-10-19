import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { blankRule, describeActions, reorderedPriorities, ruleProblem, slug, WindowRules } from './WindowRules'

const harness = vi.hoisted(() => ({ offline: false, mutate: vi.fn(), reset: vi.fn(), preview: vi.fn(), apply: vi.fn() }))
const rule = (id: string, priority: number, extra: Record<string, unknown> = {}) => ({ ...{ id, name: id, enabled: true, bundle_id: 'com.microsoft.VSCode', title: null, title_match: 'exact', workspace: 'Code', display: null, mode: 'tiled', priority, apply: 'on_create', stop: true }, ...extra })
vi.mock('@tanstack/react-query', () => ({
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
  useMutation: ({ mutationFn }: { mutationFn: unknown }) => {
    const name = String(mutationFn)
    const mutate = name.includes('apply_rule') ? harness.apply : harness.preview
    return { mutate, reset: vi.fn(), isPending: false, isError: false, isSuccess: false, data: undefined }
  },
}))
vi.mock('../api/bridge', () => ({ testRule: 'test_rule', applyRule: 'apply_rule', reloadRules: 'reload_rules' }))
vi.mock('../api/hooks', () => ({
  useDaemon: (kind: string) => {
    if (kind === 'status') return { isSuccess: !harness.offline, isPending: false, refetch: vi.fn() }
    if (kind === 'config') return { isPending: false, isError: false, data: { revision: 'abc', config: { window_rules: [{ application_id: 'com.apple.calculator', mode: 'floating', priority: 0, enabled: true }], workspaces: [{ id: 2, name: 'Code', display: null, native_space_id: 4 }], rules: [rule('low', 10), rule('high', 100)] } } }
    if (kind === 'rules') return { data: { rules: [rule('low', 10), rule('high', 100)] } }
    if (kind === 'apps') return { isPending: false, data: { apps: [{ pid: 1, bundle_id: 'com.microsoft.VSCode', name: 'Code', windows: 2 }] } }
    if (kind === 'windows') return { data: { windows: [{ pid: 1, token: 1, cg_window_id: 501, application_id: 'com.microsoft.VSCode', title: 'main.rs', native_space_id: 7, display: 1, workspace: null, floating: false }] } }
    if (kind === 'displays') return { data: { displays: [{ id: 1, primary: true, full_frame: { width: 1470, height: 956 } }] } }
    return { data: undefined }
  },
  useConfigPatch: () => ({ mutate: harness.mutate, reset: harness.reset, isPending: false, isError: false, isSuccess: false }),
}))

describe('Window Rules page', () => {
  let host: HTMLDivElement
  let root: Root
  beforeEach(() => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
    harness.offline = false
    for (const fn of [harness.mutate, harness.preview, harness.apply]) fn.mockReset()
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
  })
  afterEach(() => { act(() => root.unmount()); host.remove() })

  it('shows a disconnected state instead of editable controls', () => {
    harness.offline = true
    act(() => root.render(<WindowRules />))
    expect(host.textContent).toContain('Connect to the daemon to edit rules')
  })

  it('lists rules by priority and offers legacy conversion', () => {
    act(() => root.render(<WindowRules />))
    const names = [...host.querySelectorAll('.list-primary')].map((node) => node.textContent)
    expect(names[0]).toContain('high')
    expect(names[1]).toContain('low')
    const convert = [...host.querySelectorAll('button')].find((button) => button.textContent === 'Convert')!
    act(() => convert.click())
    expect(harness.mutate).toHaveBeenCalledWith({ revision: 'abc', patch: { kind: 'convert_window_rule', index: 0, id: 'com-apple-calculator' } }, expect.anything())
  })

  it('toggles a rule and reorders priorities', () => {
    act(() => root.render(<WindowRules />))
    const toggle = host.querySelector('[aria-label="Enable low"]') as HTMLInputElement
    act(() => toggle.click())
    expect(harness.mutate.mock.calls[0]![0].patch).toMatchObject({ kind: 'set_rule', previous_id: 'low', rule: { enabled: false } })
    const up = host.querySelectorAll('[aria-label="Move up"]')[1] as HTMLButtonElement
    act(() => up.click())
    expect(harness.mutate.mock.calls[1]![0].patch).toEqual({ kind: 'set_rule_priorities', priorities: [{ id: 'low', priority: 20 }, { id: 'high', priority: 10 }] })
  })

  it('keeps Save disabled until the new rule is valid, picking a running app', () => {
    act(() => root.render(<WindowRules />))
    const create = [...host.querySelectorAll('button')].find((button) => button.textContent?.includes('New rule'))!
    act(() => create.click())
    const save = () => [...host.querySelectorAll('button')].find((button) => button.textContent?.includes('Save rule'))!
    expect(save().disabled).toBe(true)
    const name = host.querySelector('#rule-name') as HTMLInputElement
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!
    act(() => { setter.call(name, 'VS Code to Code'); name.dispatchEvent(new Event('input', { bubbles: true })) })
    expect((host.querySelector('#rule-id') as HTMLInputElement).value).toBe('vs-code-to-code')
    const app = host.querySelector('#rule-app') as HTMLSelectElement
    act(() => { app.value = 'com.microsoft.VSCode'; app.dispatchEvent(new Event('change', { bubbles: true })) })
    const workspace = host.querySelector('#rule-workspace') as HTMLSelectElement
    act(() => { workspace.value = 'Code'; workspace.dispatchEvent(new Event('change', { bubbles: true })) })
    expect(save().disabled).toBe(false)
    act(() => save().click())
    expect(harness.mutate.mock.calls[0]![0].patch).toMatchObject({ kind: 'set_rule', previous_id: null, rule: { id: 'vs-code-to-code', bundle_id: 'com.microsoft.VSCode', workspace: 'Code', title: null } })
  })

  it('previews an open window without moving it', () => {
    act(() => root.render(<WindowRules />))
    const select = host.querySelector('#preview-window') as HTMLSelectElement
    act(() => { select.value = '501'; select.dispatchEvent(new Event('change', { bubbles: true })) })
    expect(harness.preview).toHaveBeenCalledWith(501)
    expect(harness.apply).not.toHaveBeenCalled()
  })
})

describe('rule helpers', () => {
  it('validates like the daemon', () => {
    const ok = { ...blankRule, id: 'vscode', bundle_id: 'com.microsoft.VSCode', workspace: 'Code' }
    expect(ruleProblem(ok, [], ['Code'])).toBeNull()
    expect(ruleProblem({ ...ok, id: '' }, [], ['Code'])).not.toBeNull()
    expect(ruleProblem(ok, [{ ...ok, id: 'VSCODE' }], ['Code'])).toContain('already uses')
    expect(ruleProblem({ ...ok, workspace: 'Nope' }, [], ['Code'])).toContain('not configured')
    expect(ruleProblem({ ...ok, title: '(', title_match: 'regex' }, [], ['Code'])).toContain('regex')
    expect(ruleProblem({ ...ok, mode: 'unmanaged' }, [], ['Code'])).toContain('cannot also be moved')
    expect(ruleProblem({ ...ok, workspace: null }, [], ['Code'])).toContain('at least one action')
    expect(ruleProblem({ ...ok, priority: 1.5 }, [], ['Code'])).not.toBeNull()
  })
  it('spaces reordered priorities and describes actions', () => {
    expect(reorderedPriorities(['a', 'b', 'c'])).toEqual([{ id: 'a', priority: 30 }, { id: 'b', priority: 20 }, { id: 'c', priority: 10 }])
    expect(slug('VS Code → Code!')).toBe('vs-code-code')
    expect(describeActions({ ...blankRule, workspace: 'Code', display: 2, mode: 'floating' })).toBe('→ Code · display 2 · Floating')
  })
})
