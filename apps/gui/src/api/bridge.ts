import { invoke } from '@tauri-apps/api/core'
import type { ConfigPatch, ConfigSnapshot, LayoutPreview, PreviewOptions, QueryKind, QueryMap, StateCommand } from './types'

export class DaemonError extends Error {
  constructor(readonly code: string, message: string) { super(message); this.name = 'DaemonError' }
}

function normalize(error: unknown): DaemonError {
  if (typeof error === 'object' && error !== null && 'code' in error && 'message' in error) {
    const value = error as { code: unknown; message: unknown }
    if (typeof value.code === 'string' && typeof value.message === 'string') return new DaemonError(value.code, value.message)
  }
  return new DaemonError('bridge', error instanceof Error ? error.message : String(error))
}

export async function queryDaemon<K extends QueryKind>(kind: K, signal?: AbortSignal): Promise<QueryMap[K]> {
  signal?.throwIfAborted()
  try {
    const result = await invoke<QueryMap[K]>('daemon_query', { kind })
    signal?.throwIfAborted()
    if (result.version !== 1) throw new DaemonError('protocol', 'Unsupported daemon protocol version')
    return result
  } catch (error) { throw normalize(error) }
}

export async function patchConfig(revision: string, patch: ConfigPatch): Promise<ConfigSnapshot> {
  try { return await invoke<ConfigSnapshot>('daemon_mutate', { action: { kind: 'patch_config', revision, patch } }) }
  catch (error) { throw normalize(error) }
}

export async function sendCommand(command: StateCommand): Promise<{ ok: boolean; message: string }> {
  try { return await invoke('daemon_mutate', { action: { kind: 'state_command', command } }) }
  catch (error) { throw normalize(error) }
}

export async function simpleAction(kind: 'reconcile' | 'recover_windows'): Promise<{ message: string }> {
  try { return await invoke('daemon_mutate', { action: { kind } }) }
  catch (error) { throw normalize(error) }
}

export async function controlDaemon(op: 'start' | 'stop' | 'restart'): Promise<unknown> {
  try { return await invoke('daemon_control', { op }) }
  catch (error) { throw normalize(error) }
}

export async function previewLayout(inner: number, outer: number, signal?: AbortSignal, options?: PreviewOptions): Promise<LayoutPreview> {
  signal?.throwIfAborted()
  try {
    const result = await invoke<LayoutPreview>('layout_preview', options ? { inner, outer, options } : { inner, outer })
    signal?.throwIfAborted()
    return result
  } catch (error) { throw normalize(error) }
}
