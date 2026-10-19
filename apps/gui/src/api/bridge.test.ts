import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { queryDaemon, patchConfig } from './bridge'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
const mockedInvoke = vi.mocked(invoke)

describe('typed daemon bridge', () => {
  beforeEach(() => mockedInvoke.mockReset())
  it('accepts versioned daemon data', async () => {
    mockedInvoke.mockResolvedValue({ version: 1, state: 'running', windows: 2 })
    const result = await queryDaemon('status')
    expect(result.windows).toBe(2)
    expect(mockedInvoke).toHaveBeenCalledWith('daemon_query', { kind: 'status' })
  })
  it('rejects unsupported protocol versions', async () => {
    mockedInvoke.mockResolvedValue({ version: 2 })
    await expect(queryDaemon('status')).rejects.toMatchObject({ code: 'protocol' })
  })
  it('ignores stale read responses after cancellation', async () => {
    const controller = new AbortController()
    mockedInvoke.mockImplementation(() => new Promise((resolve) => { setTimeout(() => resolve({ version: 1 }), 10) }))
    const pending = queryDaemon('status', controller.signal)
    controller.abort()
    await expect(pending).rejects.toBeTruthy()
  })
  it('sends a revision with a narrow config patch', async () => {
    mockedInvoke.mockResolvedValue({ version: 1, revision: 'next', config: {} })
    await patchConfig('old', { kind: 'set_gaps', inner: 12, outer: 8 })
    expect(mockedInvoke).toHaveBeenCalledWith('daemon_mutate', { action: { kind: 'patch_config', revision: 'old', patch: { kind: 'set_gaps', inner: 12, outer: 8 } } })
  })
})

describe('multi-Space workspace bindings', () => {
  it('adds a second display as an extra instance and removes only that Space', async () => {
    const { bindSpacePatch, unbindSpacePatch } = await import('./types')
    const browser = { id: 1, name: 'Browser', display: null, native_space_id: 7 }
    expect(bindSpacePatch(browser, 4665, 'Browser')).toEqual({ kind: 'set_workspace', id: 1, name: 'Browser', display: null, native_space_id: 7, native_space_ids: [4665] })
    const both = { ...browser, native_space_ids: [4665] }
    expect(unbindSpacePatch(both, 4665)).toEqual({ kind: 'set_workspace', id: 1, name: 'Browser', display: null, native_space_id: 7, native_space_ids: [] })
    expect(unbindSpacePatch(both, 7)).toEqual({ kind: 'set_workspace', id: 1, name: 'Browser', display: null, native_space_id: 4665, native_space_ids: [] })
    expect(bindSpacePatch({ ...browser, native_space_id: null }, 4, 'Code').native_space_id).toBe(4)
  })
})
