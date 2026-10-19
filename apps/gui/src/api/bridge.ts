import { invoke } from '@tauri-apps/api/core'
import type { ConfigPatch, ConfigSnapshot, LayoutPreview, PreviewOptions, QueryKind, QueryMap, RuleTest, ScratchpadResult, StateCommand } from './types'

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

export async function testRule(window: number): Promise<RuleTest> {
  try { return await invoke<RuleTest>('daemon_mutate', { action: { kind: 'test_rule', window } }) }
  catch (error) { throw normalize(error) }
}

export async function applyRule(window: number): Promise<{ ok: boolean; message: string }> {
  try { return await invoke('daemon_mutate', { action: { kind: 'apply_rule', window } }) }
  catch (error) { throw normalize(error) }
}

export async function reloadRules(): Promise<{ ok: boolean; rules: number }> {
  try { return await invoke('daemon_mutate', { action: { kind: 'reload_rules' } }) }
  catch (error) { throw normalize(error) }
}

export async function assignScratchpad(id: string, window: number | null, force = false): Promise<ScratchpadResult> {
  try { return await invoke('daemon_mutate', { action: { kind: 'assign_scratchpad', id, window, force } }) }
  catch (error) { throw normalize(error) }
}

export async function unassignScratchpad(id: string): Promise<ScratchpadResult> {
  try { return await invoke('daemon_mutate', { action: { kind: 'unassign_scratchpad', id } }) }
  catch (error) { throw normalize(error) }
}

export async function scratchpadAction(id: string, action: 'toggle' | 'show' | 'hide'): Promise<ScratchpadResult> {
  try { return await invoke('daemon_mutate', { action: { kind: 'scratchpad_action', id, action } }) }
  catch (error) { throw normalize(error) }
}

export async function recoverScratchpads(): Promise<{ ok: boolean; problems: string[] }> {
  try { return await invoke('daemon_mutate', { action: { kind: 'recover_scratchpads' } }) }
  catch (error) { throw normalize(error) }
}

export async function pauseHotkeys(): Promise<unknown> {
  try { return await invoke('daemon_mutate', { action: { kind: 'pause_hotkeys' } }) }
  catch (error) { throw normalize(error) }
}

export async function resumeHotkeys(): Promise<unknown> {
  try { return await invoke('daemon_mutate', { action: { kind: 'resume_hotkeys' } }) }
  catch (error) { throw normalize(error) }
}
