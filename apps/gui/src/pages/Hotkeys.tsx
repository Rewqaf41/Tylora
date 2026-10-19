import { useState } from 'react'
import { Keyboard, Plus, Search } from 'lucide-react'
import { useConfigPatch, useDaemon } from '../api/hooks'
import type { AnyStateCommand, StateCommand } from '../api/types'
import { shortcutFromCode, shortcutWarning } from '../shared/hotkey'
import { Badge, Button, Card, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

const actions: { label: string; command: StateCommand }[] = [
  { label: 'Reconcile windows', command: { command: 'reconcile_windows' } },
  { label: 'Toggle floating', command: { command: 'toggle_floating', window: null } },
  { label: 'Toggle layout Zoom', command: { command: 'toggle_layout_zoom', window: null } },
  { label: 'Move window to next display', command: { command: 'move_window_to_next_display', window: null } },
  { label: 'Move window to previous display', command: { command: 'move_window_to_previous_display', window: null } },
  { label: 'Focus next workspace', command: { command: 'focus_next_workspace' } },
  { label: 'Focus previous workspace', command: { command: 'focus_previous_workspace' } },
  { label: 'Cycle layout (BSP → Dwindle → Scrolling)', command: { command: 'cycle_layout' } },
  { label: 'Use BSP layout', command: { command: 'set_layout', layout: 'bsp' } },
  { label: 'Use Dwindle layout', command: { command: 'set_layout', layout: 'dwindle' } },
  { label: 'Use Scrolling layout', command: { command: 'set_layout', layout: 'scrolling' } },
  { label: 'Focus next window', command: { command: 'focus_next_window' } },
  { label: 'Focus previous window', command: { command: 'focus_previous_window' } },
  { label: 'Scroll layout left', command: { command: 'scroll_layout_left' } },
  { label: 'Scroll layout right', command: { command: 'scroll_layout_right' } },
  { label: 'Center focused column', command: { command: 'center_focused_window' } },
  { label: 'Cycle column width preset', command: { command: 'resize_scrolling_column', delta: null } },
  ...(['left', 'right', 'up', 'down'] as const).flatMap((direction) => [
    { label: `Focus window ${direction}`, command: { command: 'focus_window' as const, direction } },
    { label: `Move window ${direction}`, command: { command: 'move_window' as const, direction } },
  ]),
]
function label(command: AnyStateCommand): string {
  if (command.command === 'focus_workspace' && 'id' in command) return `Focus workspace ${command.id}`
  if (command.command === 'move_window_to_workspace' && 'workspace' in command) return `Move window to workspace ${command.workspace}`
  if (command.command === 'focus_named_workspace' && 'name' in command) return `Focus ${command.name}`
  if (command.command === 'move_window_to_named_workspace' && 'name' in command) return `Move window to ${command.name}`
  return actions.find((entry) => JSON.stringify(entry.command) === JSON.stringify(command))?.label ?? command.command.replaceAll('_', ' ')
}

export function Hotkeys() {
  const status = useDaemon('status')
  const online = status.isSuccess
  const config = useDaemon('config', online)
  const active = useDaemon('hotkeys', online)
  const patch = useConfigPatch()
  const [search, setSearch] = useState('')
  const [index, setIndex] = useState<number | null | undefined>(undefined)
  const [chord, setChord] = useState('')
  const [command, setCommand] = useState<AnyStateCommand>(actions[0]!.command)
  const [recording, setRecording] = useState(false)
  const [enabled, setEnabled] = useState(true)
  const definitions = config.data?.config.hotkeys ?? []
  const actionChoices = [...actions, ...(config.data?.config.workspaces.flatMap((workspace) => [
    { label: `Focus workspace ${workspace.name}`, command: { command: 'focus_workspace' as const, id: workspace.id } },
    { label: `Move window to ${workspace.name}`, command: { command: 'move_window_to_workspace' as const, window: null, workspace: workspace.id } },
  ]) ?? [])]
  const open = (at: number | null) => { const binding = at === null ? undefined : definitions[at]; setIndex(at); setChord(binding?.chord ?? ''); setCommand(binding?.command ?? actionChoices[0]!.command); setEnabled(binding?.enabled ?? true); setRecording(false); patch.reset() }
  const conflict = chord && definitions.some((binding, at) => at !== index && binding.enabled && binding.chord.toLowerCase() === chord.toLowerCase())
  const save = () => { if (!config.data || !chord || conflict) return; patch.mutate({ revision: config.data.revision, patch: { kind: 'set_hotkey', index: index ?? null, chord, command, enabled } }) }
  const keyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!recording) return
    event.preventDefault(); event.stopPropagation()
    if (event.code === 'Escape') { setRecording(false); return }
    const captured = shortcutFromCode(event.code, { meta: event.metaKey, ctrl: event.ctrlKey, alt: event.altKey, shift: event.shiftKey })
    if (captured) { setChord(captured); setRecording(false) }
  }
  return <><div className="page-heading"><div><span className="eyebrow">Input</span><h1>Hotkeys</h1><p>Record a physical key combination and let the daemon validate it before applying.</p></div><Button variant="primary" onClick={() => open(null)}><Plus size={14} /> New shortcut</Button></div>
    {!online ? status.isPending ? <Loading /> : <ErrorState message="Connect to the daemon to edit hotkeys." retry={() => void status.refetch()} /> : config.isPending || active.isPending ? <Loading /> : config.isError || active.isError ? <ErrorState message={config.error?.message ?? active.error?.message ?? 'Unknown error'} /> : <>
      <SectionHeader title="Active bindings" description={`${definitions.length} configured shortcuts`} aside={<div style={{ width: 230, position: 'relative' }}><Search size={14} style={{ position: 'absolute', left: 10, top: 10 }} className="muted" /><input className="search-input" style={{ paddingLeft: 30 }} placeholder="Search actions…" value={search} onChange={(event) => setSearch(event.target.value)} /></div>} />
      <Card>{definitions.map((binding, at) => ({ binding, at })).filter(({ binding }) => label(binding.command).toLowerCase().includes(search.toLowerCase()) || binding.chord.toLowerCase().includes(search.toLowerCase())).map(({ binding, at }) => <div className="list-item" key={at}><div><div className="list-primary">{label(binding.command)}</div><div className="list-secondary">{binding.enabled ? (active.data.hotkeys[at]?.active ? 'Installed in global event tap' : 'Unavailable on this daemon') : 'Disabled'}</div></div><div className="list-actions"><span className="kbd">{binding.chord}</span><Badge tone={!binding.enabled ? 'neutral' : active.data.hotkeys[at]?.active ? 'good' : 'warn'}>{!binding.enabled ? 'Disabled' : active.data.hotkeys[at]?.active ? 'Active' : 'Unavailable'}</Badge><Button variant="ghost" onClick={() => open(at)}>Edit</Button></div></div>)}</Card>
      {index !== undefined && <><SectionHeader title={index === null ? 'New shortcut' : 'Edit shortcut'} description="The key position is used, even when Option produces a symbol." /><Card className="padded"><div className="form-grid"><div className="field"><label htmlFor="action-select">ACTION</label><select id="action-select" value={JSON.stringify(command)} onChange={(event) => { const selected = actionChoices.find((item) => JSON.stringify(item.command) === event.target.value); if (selected) setCommand(selected.command) }}>
        {!actionChoices.some((item) => JSON.stringify(item.command) === JSON.stringify(command)) && <option value={JSON.stringify(command)}>{label(command)}</option>}
        {actionChoices.map((item) => <option key={`${item.label}-${JSON.stringify(item.command)}`} value={JSON.stringify(item.command)}>{item.label}</option>)}</select></div>
        <div className="field"><label>SHORTCUT</label><button className={`button ${recording ? 'button-primary' : ''}`} onClick={() => setRecording(true)} onKeyDown={keyDown} onBlur={() => setRecording(false)}><Keyboard size={14} />{recording ? 'Press a combination…' : chord || 'Record shortcut'}</button></div></div>
        <div className="form-actions"><label className="row" style={{ marginRight: 'auto', gap: 7 }}><input type="checkbox" checked={enabled} onChange={(event) => setEnabled(event.target.checked)} /> Enabled</label><Button onClick={() => setIndex(undefined)}>Cancel</Button><Button variant="primary" disabled={patch.isPending || !chord || Boolean(conflict)} onClick={save}>{patch.isPending ? 'Saving…' : 'Save shortcut'}</Button></div>
        {conflict && <p className="bad-text">This combination is already assigned in Tylora.</p>}
        {shortcutWarning(chord) && <div style={{ marginTop: 10 }}><Notice tone="warn">{shortcutWarning(chord)}</Notice></div>}
        {patch.isError && <p className="bad-text">{patch.error.message}</p>}{patch.isSuccess && <p className="good-text">Saved and reinstalled by daemon.</p>}
      </Card></>}
      <div style={{ marginTop: 18 }}><Notice>Tylora can detect duplicates in its own bindings. macOS and other apps may reserve additional combinations that cannot be enumerated reliably.</Notice></div>
    </>}
  </>
}
