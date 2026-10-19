import { useState } from 'react'
import { ArrowRight, CornerDownRight, Link2, Unlink } from 'lucide-react'
import { useAction, useConfigPatch, useDaemon } from '../api/hooks'
import { bindSpacePatch, bindsSpace, unbindSpacePatch } from '../api/types'
import type { NativeSpace, WorkspaceDefinition } from '../api/types'
import { Badge, Button, Card, Empty, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

function WorkspaceEditor({ workspace, space, revision, all, spaces }: { workspace: WorkspaceDefinition | undefined; space: NativeSpace; revision: string; all: WorkspaceDefinition[]; spaces: NativeSpace[] }) {
  const [selectedId, setSelectedId] = useState(workspace?.id ?? all.find((entry) => entry.native_space_id === null)?.id ?? 1)
  // A workspace may also own one Space on each other display.
  const displayOf = (id: number) => spaces.find((entry) => entry.id === id)?.display
  const selectable = (entry: WorkspaceDefinition) => bindsSpace(entry, space.id) || entry.native_space_id === null || [entry.native_space_id, ...(entry.native_space_ids ?? [])].every((id) => id === null || displayOf(id) !== space.display)
  const elsewhere = workspace ? [workspace.native_space_id, ...(workspace.native_space_ids ?? [])].filter((id): id is number => id !== null && id !== space.id) : []
  const [name, setName] = useState(workspace?.name ?? '')
  const patch = useConfigPatch()
  const action = useAction()
  const [showWindows, setShowWindows] = useState(false)
  const windows = useDaemon('windows', showWindows)
  const target = all.find((entry) => entry.id === selectedId)
  const save = () => {
    if (!target) return
    patch.mutate({ revision, patch: bindSpacePatch(target, space.id, name.trim() || target.name) })
  }
  const unbind = () => {
    if (!workspace) return
    patch.mutate({ revision, patch: unbindSpacePatch(workspace, space.id) })
  }
  return <Card className="padded">
    <div className="row"><div><span className={`space-dot ${space.active ? '' : 'inactive'}`} /><strong>Desktop {space.desktop}</strong><span className="row-sub">Native Space ID <span className="mono">{space.id}</span></span></div><Badge tone={space.active ? 'good' : 'neutral'}>{space.active ? 'Active' : 'Inactive'}</Badge></div>
    <div style={{ marginTop: 18 }} className="form-grid">
      <div className="field"><label htmlFor={`workspace-${space.id}`}>LOGICAL WORKSPACE</label><select id={`workspace-${space.id}`} value={selectedId} onChange={(event) => { const id = Number(event.target.value); setSelectedId(id); setName(all.find((ws) => ws.id === id)?.name ?? '') }}>
        {all.filter(selectable).map((entry) => <option key={entry.id} value={entry.id}>{entry.id} · {entry.name}{entry.native_space_id !== null && !bindsSpace(entry, space.id) ? ' (also on another display)' : ''}</option>)}
      </select></div>
      <div className="field"><label htmlFor={`name-${space.id}`}>NAME</label><input id={`name-${space.id}`} value={name} maxLength={60} onChange={(event) => setName(event.target.value)} /></div>
    </div>
    <div className="form-actions"><Button variant="ghost" onClick={() => setShowWindows(!showWindows)}>{showWindows ? 'Hide' : 'Show'} windows</Button>
      {workspace && <Button onClick={() => action.mutate({ kind: 'command', command: { command: 'focus_workspace', id: workspace.id } })} disabled={action.isPending}><CornerDownRight size={13} /> Focus</Button>}
      {workspace && <Button onClick={() => action.mutate({ kind: 'command', command: { command: 'move_window_to_workspace', window: null, workspace: workspace.id } })} disabled={action.isPending}><ArrowRight size={13} /> Move focused</Button>}
      {workspace && <Button onClick={unbind} disabled={patch.isPending}><Unlink size={13} /> Unbind</Button>}
      <Button variant="primary" onClick={save} disabled={patch.isPending || !target || !name.trim()}><Link2 size={13} /> {patch.isPending ? 'Saving…' : 'Save binding'}</Button>
    </div>
    {elsewhere.length > 0 && <p className="row-sub" style={{ marginTop: 10 }}>Also bound to Space {elsewhere.join(', ')}. The workspace shortcut opens the instance with the most recent focus; pressing it again switches to the next one.</p>}
    {showWindows && <div style={{ marginTop: 13 }} className="stack">{windows.isPending ? <Loading /> : windows.data?.windows.filter((window) => window.native_space_id === space.id).length ? windows.data.windows.filter((window) => window.native_space_id === space.id).map((window) => <div key={`${window.pid}-${window.token}`} className="row row-pad"><span>{window.application_id ?? `Process ${window.pid}`}</span><span className="mono muted">CG {window.cg_window_id ?? 'unknown'}</span></div>) : <span className="muted">No managed windows observed in this Space.</span>}</div>}
    {patch.isError && <p className="bad-text">{patch.error.message}</p>}{action.isError && <p className="bad-text">{action.error.message}</p>}
    {patch.isSuccess && <p className="good-text">Saved and applied by daemon.</p>}
  </Card>
}

export function Workspaces() {
  const status = useDaemon('status')
  const online = status.isSuccess
  const spaces = useDaemon('spaces', online)
  const config = useDaemon('config', online)
  const hotkeys = useDaemon('hotkeys', online)
  const displays = [...new Set(spaces.data?.spaces.map((space) => space.display) ?? [])]
  return <><div className="page-heading"><div><span className="eyebrow">Spaces & bindings</span><h1>Workspaces</h1><p>Bind logical workspaces to stable native Space IDs. Desktop order can change without breaking bindings.</p></div></div>
    {!online ? status.isPending ? <Loading /> : <ErrorState message="Start the daemon to inspect native Spaces." retry={() => void status.refetch()} /> : spaces.isPending || config.isPending ? <Loading /> : spaces.isError || config.isError ? <ErrorState message={spaces.error?.message ?? config.error?.message ?? 'Unknown error'} retry={() => { void spaces.refetch(); void config.refetch() }} /> : <>
      {spaces.data.spaces.length === 0 ? <Empty title="No native Spaces found">Check the native backend in Diagnostics.</Empty> : displays.map((display) => <div key={display}>
        <SectionHeader title={`Display ${display}`} description={`${spaces.data.spaces.filter((space) => space.display === display).length} native Desktop spaces`} />
        <div className="stack">{spaces.data.spaces.filter((space) => space.display === display).map((space) => {
          const workspace = config.data.config.workspaces.find((entry) => bindsSpace(entry, space.id))
          const chord = hotkeys.data?.hotkeys.find((hotkey) => hotkey.command.command === 'focus_workspace' && 'id' in hotkey.command && hotkey.command.id === workspace?.id)?.chord
          return <div key={space.id}><WorkspaceEditor key={`${space.id}-${workspace?.id}-${config.data.revision}`} space={space} workspace={workspace} all={config.data.config.workspaces} spaces={spaces.data.spaces} revision={config.data.revision} />{workspace && chord && <div className="muted" style={{ padding: '6px 14px 0', fontSize: 11 }}>Focus shortcut: <span className="kbd">{chord}</span></div>}</div>
        })}</div>
      </div>)}
      <div style={{ marginTop: 20 }}><Notice>Native Space IDs are read from WindowServer. Tylora does not create, delete, or renumber macOS Desktops.</Notice></div>
    </>}
  </>
}
