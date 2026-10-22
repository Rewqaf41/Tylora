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

export type LoginItemStatus = 'not_registered' | 'enabled' | 'requires_approval' | 'not_found' | 'unavailable'
export interface AppInfo { version: string; bundled_daemon: string | null; accessibility_trusted: boolean; login_item: LoginItemStatus; daemon_version: string | null; daemon_executable: string | null }
export interface GuiPreferences { version: 1; launch_at_login: boolean; start_daemon_automatically: boolean; open_settings_at_launch: boolean; show_menu_bar_icon: boolean; onboarding_completed: boolean; check_for_updates: boolean; last_seen_version: string | null }

export async function appInfo(): Promise<AppInfo> {
  try { return await invoke<AppInfo>('app_info') }
  catch (error) { throw normalize(error) }
}

/** Shows the macOS Accessibility prompt; resolves to the current trust state. */
export async function requestAccessibility(): Promise<boolean> {
  try { return await invoke<boolean>('request_accessibility') }
  catch (error) { throw normalize(error) }
}

export async function openSystemPane(pane: 'accessibility' | 'input_monitoring' | 'login_items'): Promise<void> {
  try { await invoke('open_system_pane', { pane }) }
  catch (error) { throw normalize(error) }
}

export async function guiPreferences(): Promise<GuiPreferences> {
  try { return await invoke<GuiPreferences>('gui_preferences') }
  catch (error) { throw normalize(error) }
}

export async function saveGuiPreferences(prefs: GuiPreferences): Promise<GuiPreferences> {
  try { return await invoke<GuiPreferences>('save_gui_preferences', { prefs }) }
  catch (error) { throw normalize(error) }
}

export interface UpdateCheck { current: string; available: { version: string; notes: string | null; date: string | null } | null }

/** Asks GitHub Releases for a newer signed build; nothing is downloaded yet. */
export async function checkUpdate(): Promise<UpdateCheck> {
  try { return await invoke<UpdateCheck>('check_update') }
  catch (error) { throw normalize(error) }
}

/** Downloads, verifies and installs the update found by the last check, restarts a running daemon and relaunches the app. */
export async function installUpdate(): Promise<void> {
  try { await invoke('install_update') }
  catch (error) { throw normalize(error) }
}

export async function openReleaseNotes(version?: string): Promise<void> {
  try { await invoke('open_release_notes', { version: version ?? null }) }
  catch (error) { throw normalize(error) }
}
