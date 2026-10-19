export type Direction = 'left' | 'right' | 'up' | 'down'
export type LayoutKind = 'bsp' | 'dwindle' | 'scrolling'
export const LAYOUT_KINDS: LayoutKind[] = ['bsp', 'dwindle', 'scrolling']
export interface DwindleSettings { initial_orientation: 'vertical' | 'horizontal'; split_ratio: number }
export interface ScrollingSettings {
  default_column_width: number; min_column_width: number; max_column_width: number
  column_width_presets: number[]; viewport_alignment: 'nearest' | 'center'
  focus_auto_scroll: boolean; mouse_resize: boolean; scroll_sensitivity: number
}
export const DEFAULT_DWINDLE: DwindleSettings = { initial_orientation: 'vertical', split_ratio: 0.5 }
export const DEFAULT_SCROLLING: ScrollingSettings = {
  default_column_width: 0.65, min_column_width: 0.2, max_column_width: 1,
  column_width_presets: [1 / 3, 0.5, 0.65, 1], viewport_alignment: 'nearest',
  focus_auto_scroll: true, mouse_resize: true, scroll_sensitivity: 0.25,
}
export type StateCommand =
  | { command: 'focus_workspace'; id: number }
  | { command: 'move_window_to_workspace'; window: null; workspace: number }
  | { command: 'focus_window'; direction: Direction }
  | { command: 'move_window'; direction: Direction }
  | { command: 'toggle_floating'; window: null }
  | { command: 'toggle_layout_zoom'; window: null }
  | { command: 'reconcile_windows' }
  | { command: 'focus_next_workspace' }
  | { command: 'focus_previous_workspace' }
  | { command: 'focus_named_workspace'; name: string }
  | { command: 'move_window_to_named_workspace'; name: string }
  | { command: 'focus_display'; direction: Direction }
  | { command: 'focus_display_next' }
  | { command: 'move_window_to_next_display'; window: null }
  | { command: 'move_window_to_previous_display'; window: null }
  | { command: 'move_window_to_display'; window: null; display: number }
  | { command: 'set_layout'; layout: LayoutKind }
  | { command: 'cycle_layout' }
  | { command: 'focus_next_window' }
  | { command: 'focus_previous_window' }
  | { command: 'scroll_layout_left' }
  | { command: 'scroll_layout_right' }
  | { command: 'center_focused_window' }
  | { command: 'resize_scrolling_column'; delta: number | null }

export interface WorkspaceDefinition { id: number; name: string; display: number | null; native_space_id: number | null; native_space_ids?: number[]; layout?: LayoutKind | null }
export const bindsSpace = (ws: WorkspaceDefinition, space: number) => ws.native_space_id === space || (ws.native_space_ids ?? []).includes(space)
/** Binding change for `space` on `ws`: a workspace already bound elsewhere gains it as an extra instance. */
export function bindSpacePatch(ws: WorkspaceDefinition, space: number, name: string) {
  const extras = ws.native_space_ids ?? []
  if (ws.native_space_id === null || ws.native_space_id === space) return { kind: 'set_workspace' as const, id: ws.id, name, display: ws.display, native_space_id: space, native_space_ids: extras.filter((id) => id !== space) }
  return { kind: 'set_workspace' as const, id: ws.id, name, display: ws.display, native_space_id: ws.native_space_id, native_space_ids: extras.includes(space) ? extras : [...extras, space] }
}
/** Removes only `space` from `ws`; the first extra becomes primary when the primary is removed. */
export function unbindSpacePatch(ws: WorkspaceDefinition, space: number) {
  const extras = (ws.native_space_ids ?? []).filter((id) => id !== space)
  if (ws.native_space_id === space) return { kind: 'set_workspace' as const, id: ws.id, name: ws.name, display: ws.display, native_space_id: extras[0] ?? null, native_space_ids: extras.slice(1) }
  return { kind: 'set_workspace' as const, id: ws.id, name: ws.name, display: ws.display, native_space_id: ws.native_space_id, native_space_ids: extras }
}
export type AnyStateCommand = StateCommand | ({ command: string } & Record<string, unknown>)
export interface Hotkey { chord: string; command: AnyStateCommand; enabled: boolean }
export interface WindowRule { application_id: string; mode: 'floating' | 'tiled'; priority: number; enabled: boolean }
export interface FocusSettings { follows_mouse: boolean; mode: 'disabled' | 'instant' | 'delayed'; delay_ms: number; raise_behavior: 'focus_and_raise' | 'focus_only'; ignore_apps: { bundle_id: string; enabled: boolean }[]; restore_last_window: boolean }
export interface Config {
  version: number
  gaps: { inner: number; outer: number }
  zoom: { respect_outer_gaps: boolean }
  window_movement: { focus_after_move: boolean; wrap_around: boolean; preserve_floating_position: boolean }
  default_layout: LayoutKind
  dwindle: DwindleSettings
  scrolling: ScrollingSettings
  hotkeys: Hotkey[]
  window_rules: WindowRule[]
  workspaces: WorkspaceDefinition[]
  cross_monitor_navigation: boolean
  focus: FocusSettings
  mouse: { enabled: boolean; modifier: string; drag_threshold: number; resize_sensitivity: number; update_interval_ms: number }
}
export interface ConfigSnapshot { version: 1; revision: string; config: Config }
export type ConfigPatch =
  | { kind: 'set_gaps'; inner: number; outer: number }
  | { kind: 'set_zoom'; respect_outer_gaps: boolean }
  | { kind: 'set_window_movement'; settings: Config['window_movement'] }
  | { kind: 'set_workspace'; id: number; name: string; display: number | null; native_space_id: number | null; native_space_ids?: number[] }
  | { kind: 'set_hotkey'; index: number | null; chord: string; command: AnyStateCommand; enabled: boolean }
  | { kind: 'set_window_rule'; index: number | null; rule: WindowRule }
  | { kind: 'remove_window_rule'; index: number }
  | { kind: 'set_focus'; focus: FocusSettings }
  | { kind: 'set_default_layout'; layout: LayoutKind }
  | { kind: 'set_workspace_layout'; id: number; layout: LayoutKind | null }
  | { kind: 'set_dwindle'; settings: DwindleSettings }
  | { kind: 'set_scrolling'; settings: ScrollingSettings }

export interface NativeSpace { id: number; desktop: number; display: number; active: boolean }
export interface Rect { x: number; y: number; width: number; height: number }
export interface DisplayInfo { id: number; uuid: string | null; full_frame: Rect; work_area: Rect; scale: number; primary: boolean }
export interface ManagedWindow { pid: number; token: number; cg_window_id: number | null; application_id: string | null; native_space_id: number | null; display: number; workspace: number | null; floating: boolean }
export interface HotkeyState extends Hotkey { active: boolean }
export interface DaemonStatus {
  version: 1; state: 'running'; displays: number; primary_display: number; windows: number; tiled: number; floating: number
  active_workspaces: { display: number; id: number }[]; layout: LayoutKind; accessibility_trusted: boolean
  layouts?: CurrentLayout[]
  native_spaces: NativeSpace[]; degraded: number; stale_results: number
  layout_zoom: { display: number; native_space_id: number; window: { process: { pid: number; epoch: number }; token: number }; cg_window_id: number; confirmed: boolean }[]
  pending_display_moves?: { cg_window_id: number; source_display: number; target_display: number; target_space_id: number; native_verified: boolean; elapsed_ms: number }[]
}
export type ColumnPlacement = 'visible' | 'partial' | 'parked'
export interface ScrollingSummary {
  mode: 'strip' | 'bounded'; viewport_offset: number; strip_width: number; usable_width: number
  active: { process: { pid: number; epoch: number }; token: number } | null; state_version: number
  columns: { window: { process: { pid: number; epoch: number }; token: number }; width_fraction: number; strip_x: number; width: number; frame: Rect; placement: { state: ColumnPlacement } }[]
}
export interface CurrentLayout { display: number; native_space_id: number | null; workspace: number | null; layout: LayoutKind; windows: number; scrolling: ScrollingSummary | null }
export interface PreviewOptions { layout: LayoutKind; dwindle?: DwindleSettings; scrolling?: ScrollingSettings; windows?: number; focus?: number }
export interface PreviewFrame { slot: number; rect: Rect; placement?: { state: ColumnPlacement } }
export interface LayoutPreview { version: 1; layout?: LayoutKind; frames: PreviewFrame[]; scrolling?: ScrollingSummary | null }
export interface Capabilities { can_list_spaces?: boolean; can_focus_space?: boolean; can_move_window_between_spaces?: boolean; [key: string]: unknown }
export interface Diagnostics { version: 1; daemon: 'running'; accessibility_trusted: boolean; native_spaces: number; managed_windows: number; stale_results: number; degraded: number; focus_follows_mouse?: { requests: number; applied?: number; confirmed: number; cancelled_delayed: number; failures: number; hit_tests: number; hit_test_total_us: number; mouse_to_request_total_us: number; request_to_applied_total_us?: number; request_to_confirm_total_us: number } }
export interface CommandResult { ok: true; message: string }

export interface QueryMap {
  status: DaemonStatus
  displays: { version: 1; displays: DisplayInfo[] }
  spaces: { version: 1; spaces: NativeSpace[] }
  workspaces: { version: 1; workspaces: { id: number; name: string; display: number; active: boolean; windows: number }[] }
  windows: { version: 1; windows: ManagedWindow[] }
  hotkeys: { version: 1; hotkeys: HotkeyState[] }
  config: ConfigSnapshot
  capabilities: { version: 1; capabilities: Capabilities }
  diagnostics: Diagnostics
}
export type QueryKind = keyof QueryMap
