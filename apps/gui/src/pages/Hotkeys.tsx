import { useState } from 'react'
import { Check, Plus, Search, X } from 'lucide-react'
import { useConfigPatch, useDaemon } from '../api/hooks'
import type { AnyStateCommand, StateCommand } from '../api/types'
import { shortcutWarning } from '../shared/hotkey'
import { HotkeyRecorder } from '../shared/HotkeyRecorder'
import { useT, type MessageKey, type T, type Vars } from '../i18n'
import { Appear, Badge, Button, Card, ErrorState, Keys, Loading, Notice, PageHeading } from '../shared/ui'

const DIRECTIONS = ['left', 'right', 'up', 'down'] as const
const actions: { label: MessageKey; vars?: Vars; command: StateCommand }[] = [
  { label: 'hotkeys.action.reconcile', command: { command: 'reconcile_windows' } },
  { label: 'hotkeys.action.toggleFloating', command: { command: 'toggle_floating', window: null } },
  { label: 'hotkeys.action.toggleZoom', command: { command: 'toggle_layout_zoom', window: null } },
  { label: 'hotkeys.action.nextDisplay', command: { command: 'move_window_to_next_display', window: null } },
  { label: 'hotkeys.action.previousDisplay', command: { command: 'move_window_to_previous_display', window: null } },
  { label: 'hotkeys.action.nextWorkspace', command: { command: 'focus_next_workspace' } },
  { label: 'hotkeys.action.previousWorkspace', command: { command: 'focus_previous_workspace' } },
  { label: 'hotkeys.action.cycleLayout', command: { command: 'cycle_layout' } },
  { label: 'hotkeys.action.useLayout', vars: { layout: 'BSP' }, command: { command: 'set_layout', layout: 'bsp' } },
  { label: 'hotkeys.action.useLayout', vars: { layout: 'Dwindle' }, command: { command: 'set_layout', layout: 'dwindle' } },
  { label: 'hotkeys.action.useLayout', vars: { layout: 'Scrolling' }, command: { command: 'set_layout', layout: 'scrolling' } },
  { label: 'hotkeys.action.nextWindow', command: { command: 'focus_next_window' } },
  { label: 'hotkeys.action.previousWindow', command: { command: 'focus_previous_window' } },
  { label: 'hotkeys.action.scrollLeft', command: { command: 'scroll_layout_left' } },
  { label: 'hotkeys.action.scrollRight', command: { command: 'scroll_layout_right' } },
  { label: 'hotkeys.action.center', command: { command: 'center_focused_window' } },
  { label: 'hotkeys.action.cycleWidth', command: { command: 'resize_scrolling_column', delta: null } },
  ...DIRECTIONS.flatMap((direction) => [
    { label: 'hotkeys.action.focusWindow' as const, vars: { direction }, command: { command: 'focus_window' as const, direction } },
    { label: 'hotkeys.action.moveWindow' as const, vars: { direction }, command: { command: 'move_window' as const, direction } },
  ]),
]
const SCRATCHPAD_VERBS: Record<string, MessageKey> = { toggle_scratchpad: 'hotkeys.action.toggleScratchpad', show_scratchpad: 'hotkeys.action.showScratchpad', hide_scratchpad: 'hotkeys.action.hideScratchpad' }
const DIRECTION_KEYS: Record<string, MessageKey> = { left: 'direction.left', right: 'direction.right', up: 'direction.up', down: 'direction.down' }
function actionLabel(t: T, entry: { label: MessageKey; vars?: Vars }): string {
  const direction = entry.vars?.direction
  return t(entry.label, direction ? { direction: t(DIRECTION_KEYS[direction]!) } : entry.vars)
}
function label(t: T, command: AnyStateCommand): string {
  if (command.command === 'focus_workspace' && 'id' in command) return t('hotkeys.action.focusWorkspace', { name: String(command.id) })
  if (command.command === 'move_window_to_workspace' && 'workspace' in command) return t('hotkeys.action.moveToWorkspace', { name: String(command.workspace) })
  if (command.command === 'focus_named_workspace' && 'name' in command) return t('hotkeys.action.focusWorkspace', { name: String(command.name) })
  if (command.command === 'move_window_to_named_workspace' && 'name' in command) return t('hotkeys.action.moveToWorkspace', { name: String(command.name) })
  if (SCRATCHPAD_VERBS[command.command] && 'id' in command) return t(SCRATCHPAD_VERBS[command.command]!, { id: String(command.id) })
  const known = actions.find((entry) => JSON.stringify(entry.command) === JSON.stringify(command))
  return known ? actionLabel(t, known) : command.command.replaceAll('_', ' ')
}

export function Hotkeys() {
  const t = useT()
  const status = useDaemon('status')
  const online = status.isSuccess
  const config = useDaemon('config', online)
  const active = useDaemon('hotkeys', online)
  const patch = useConfigPatch()
  const [search, setSearch] = useState('')
  const [index, setIndex] = useState<number | null | undefined>(undefined)
  const [chord, setChord] = useState('')
  const [command, setCommand] = useState<AnyStateCommand>(actions[0]!.command)
  const [enabled, setEnabled] = useState(true)
  const definitions = config.data?.config.hotkeys ?? []
  const actionChoices: { label: string; command: AnyStateCommand }[] = [...actions.map((entry) => ({ label: actionLabel(t, entry), command: entry.command })), ...(config.data?.config.workspaces.flatMap((workspace) => [
    { label: t('hotkeys.action.focusWorkspace', { name: workspace.name }), command: { command: 'focus_workspace' as const, id: workspace.id } },
    { label: t('hotkeys.action.moveToWorkspace', { name: workspace.name }), command: { command: 'move_window_to_workspace' as const, window: null, workspace: workspace.id } },
  ]) ?? [])]
  const open = (at: number | null) => { const binding = at === null ? undefined : definitions[at]; setIndex(at); setChord(binding?.chord ?? ''); setCommand(binding?.command ?? actionChoices[0]!.command); setEnabled(binding?.enabled ?? true); patch.reset() }
  const conflict = chord && (definitions.some((binding, at) => at !== index && binding.enabled && binding.chord.toLowerCase() === chord.toLowerCase())
    || (config.data?.config.scratchpads ?? []).some((pad) => pad.hotkey?.toLowerCase() === chord.toLowerCase()))
  const save = () => { if (!config.data || !chord || conflict) return; patch.mutate({ revision: config.data.revision, patch: { kind: 'set_hotkey', index: index ?? null, chord, command, enabled } }) }
  const rows = definitions.map((binding, at) => ({ binding, at })).filter(({ binding }) => label(t, binding.command).toLowerCase().includes(search.toLowerCase()) || binding.chord.toLowerCase().includes(search.toLowerCase()))
  return <><PageHeading eyebrow={t('focus.eyebrow')} title={t('nav.hotkeys')} intro={t('hotkeys.intro')} actions={<Button variant="primary" onClick={() => open(null)}><Plus size={14} /> {t('hotkeys.new')}</Button>} />
    {!online ? status.isPending ? <Loading /> : <ErrorState message={t('hotkeys.offline')} retry={() => void status.refetch()} /> : config.isPending || active.isPending ? <Loading /> : config.isError || active.isError ? <ErrorState message={config.error?.message ?? active.error?.message ?? t('common.unknownError')} /> : <>
      <div className="search-field" style={{ maxWidth: 300 }}><Search size={14} /><input className="search-input" placeholder={t('hotkeys.search')} aria-label={t('hotkeys.search')} value={search} onChange={(event) => setSearch(event.target.value)} /></div>
      <div className={index !== undefined ? 'two-col' : 'stack'} style={{ gap: 18 }}><div>
      <Card title={t('hotkeys.active')} description={t('hotkeys.configured', { count: definitions.length })}>{rows.map(({ binding, at }) => <div className="list-row" key={at}>
        <div><div className="list-primary">{label(t, binding.command)}</div><div className="list-secondary">{binding.enabled ? (active.data.hotkeys[at]?.active ? t('hotkeys.installed') : t('hotkeys.unavailableHere')) : t('common.disabled')}</div></div>
        <div className="list-actions"><Keys chord={binding.chord} /><Badge tone={!binding.enabled ? 'neutral' : active.data.hotkeys[at]?.active ? 'good' : 'warn'}>{!binding.enabled ? t('common.disabled') : active.data.hotkeys[at]?.active ? t('common.active') : t('common.unavailable')}</Badge><Button variant="ghost" size="sm" onClick={() => open(at)}>{t('common.edit')}</Button></div>
      </div>)}</Card>
      </div>
      {index !== undefined && <Appear style={{ position: 'sticky', top: 72 }}><Card title={index === null ? t('hotkeys.new') : t('hotkeys.edit')} description={t('hotkeys.editHint')} aside={<button className="icon-button" aria-label={t('common.cancel')} onClick={() => setIndex(undefined)}><X size={14} /></button>} className="padded stack">
        <div className="field"><label htmlFor="action-select">{t('hotkeys.action')}</label><select id="action-select" value={JSON.stringify(command)} onChange={(event) => { const selected = actionChoices.find((item) => JSON.stringify(item.command) === event.target.value); if (selected) setCommand(selected.command) }}>
          {!actionChoices.some((item) => JSON.stringify(item.command) === JSON.stringify(command)) && <option value={JSON.stringify(command)}>{label(t, command)}</option>}
          {actionChoices.map((item) => <option key={`${item.label}-${JSON.stringify(item.command)}`} value={JSON.stringify(item.command)}>{item.label}</option>)}</select></div>
        <div className="field"><label htmlFor="hotkey-recorder">{t('hotkeys.shortcut')}</label><HotkeyRecorder id="hotkey-recorder" value={chord || null} onChange={(next) => setChord(next ?? '')} /></div>
        {conflict && <Notice tone="bad">{t('hotkeys.conflict')}</Notice>}
        {shortcutWarning(chord, t) && <Notice tone="warn">{shortcutWarning(chord, t)}</Notice>}
        <div className="form-actions"><label className="row" style={{ marginRight: 'auto', gap: 9 }}><input type="checkbox" checked={enabled} onChange={(event) => setEnabled(event.target.checked)} /> {t('common.enabled')}</label><Button variant="ghost" onClick={() => setIndex(undefined)}>{t('common.cancel')}</Button><Button variant="primary" disabled={patch.isPending || !chord || Boolean(conflict)} onClick={save}><Check size={13} /> {patch.isPending ? t('common.saving') : t('hotkeys.save')}</Button></div>
        {patch.isError && <p className="bad-text">{patch.error.message}</p>}{patch.isSuccess && <p className="good-text">{t('hotkeys.saved')}</p>}
      </Card></Appear>}
      </div>
      <Notice>{t('hotkeys.notice')}</Notice>
    </>}
  </>
}
