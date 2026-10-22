import { useState } from 'react'
import { ArrowRight, Check, ChevronDown, ChevronUp, Crosshair, Link2, Pencil, Plus, Trash2, Unlink } from 'lucide-react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { createDesktop } from '../api/bridge'
import { useAction, useConfigPatch, useDaemon } from '../api/hooks'
import { bindSpacePatch, bindsSpace, unbindSpacePatch } from '../api/types'
import type { NativeSpace, WorkspaceDefinition } from '../api/types'
import { useT } from '../i18n'
import { Badge, Button, Card, ConfirmButton, Empty, ErrorState, Keys, Loading, Notice } from '../shared/ui'

function WorkspaceEditor({ workspace, space, revision, all, spaces, chord }: { workspace: WorkspaceDefinition | undefined; space: NativeSpace; revision: string; all: WorkspaceDefinition[]; spaces: NativeSpace[]; chord?: string }) {
  const t = useT()
  // 0 stands for "new workspace": created with a name and bound here.
  const [selectedId, setSelectedId] = useState(workspace?.id ?? all.find((entry) => entry.native_space_id === null)?.id ?? 0)
  // A workspace may also own one Space on each other display.
  const displayOf = (id: number) => spaces.find((entry) => entry.id === id)?.display
  const selectable = (entry: WorkspaceDefinition) => bindsSpace(entry, space.id) || entry.native_space_id === null || [entry.native_space_id, ...(entry.native_space_ids ?? [])].every((id) => id === null || displayOf(id) !== space.display)
  const elsewhere = workspace ? [workspace.native_space_id, ...(workspace.native_space_ids ?? [])].filter((id): id is number => id !== null && id !== space.id) : []
  // The name field shows only while naming a new workspace or renaming.
  const [renaming, setRenaming] = useState(false)
  const [name, setName] = useState('')
  const patch = useConfigPatch()
  const action = useAction()
  const [showWindows, setShowWindows] = useState(false)
  const windows = useDaemon('windows', showWindows)
  const target = all.find((entry) => entry.id === selectedId)
  const naming = selectedId === 0 || renaming
  const bind = () => { if (target) patch.mutate({ revision, patch: bindSpacePatch(target, space.id, target.name) }) }
  const unbind = () => { if (workspace) patch.mutate({ revision, patch: unbindSpacePatch(workspace, space.id) }) }
  const submitName = () => {
    const value = name.trim()
    if (!value) return
    if (renaming && workspace) patch.mutate({ revision, patch: bindSpacePatch(workspace, space.id, value) })
    else patch.mutate({ revision, patch: { kind: 'create_workspace', name: value, display: null, native_space_id: space.id } })
  }
  return <div className={`ws-row${space.active ? ' active' : ''}`}>
    <div className="ws-line">
      <div><span className={`space-dot ${space.active ? '' : 'inactive'}`} /><span className="ws-title">{t('dashboard.desktop', { n: space.desktop })}</span><Badge tone={space.active ? 'accent' : 'neutral'}>{t('ws.nativeId')} {space.id}</Badge>{space.active && <Badge tone="good">{t('common.active')}</Badge>}</div>
      <div>
        <Button variant="ghost" size="sm" aria-expanded={showWindows} onClick={() => setShowWindows(!showWindows)}>{showWindows ? t('ws.hideWindows') : t('ws.showWindows')}{showWindows ? <ChevronUp size={13} /> : <ChevronDown size={13} />}</Button>
      </div>
    </div>
    <div className="ws-line">
      <div>
        <label className="inline-label" htmlFor={`workspace-${space.id}`}>{t('ws.logical')}</label>
        {naming ? <>
          <span className="field"><input id={`workspace-${space.id}`} autoFocus aria-label={t('ws.workspaceName')} placeholder={renaming ? workspace?.name : t('ws.workspaceName')} value={name} maxLength={60} onChange={(event) => setName(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') submitName(); if (event.key === 'Escape') { setRenaming(false); setName('') } }} /></span>
          <Button size="sm" variant="primary" onClick={submitName} disabled={patch.isPending || !name.trim()}><Check size={13} /> {renaming ? t('ws.rename') : t('ws.create')}</Button>
          {renaming ? <Button size="sm" variant="ghost" onClick={() => { setRenaming(false); setName('') }}>{t('common.cancel')}</Button>
            : all.some(selectable) && <Button size="sm" variant="ghost" onClick={() => setSelectedId(workspace?.id ?? all.find(selectable)?.id ?? 0)}>{t('common.cancel')}</Button>}
        </> : <select id={`workspace-${space.id}`} value={selectedId} onChange={(event) => { setSelectedId(Number(event.target.value)); setName('') }}>
          {all.filter(selectable).map((entry) => <option key={entry.id} value={entry.id}>{entry.id} · {entry.name}{entry.native_space_id !== null && !bindsSpace(entry, space.id) ? t('ws.alsoElsewhere') : ''}</option>)}
          <option value={0}>{t('ws.newWorkspace')}</option>
        </select>}
      </div>
    </div>
    {(chord || elsewhere.length > 0) && <div className="row-sub ws-hint">{chord && <span>{t('ws.focusShortcut')} <Keys chord={chord} /></span>}{elsewhere.length > 0 && <span>{t('ws.alsoBound', { spaces: elsewhere.join(', ') })}</span>}</div>}
    <div className="ws-line">
      <div>
        {workspace && <Button variant="ghost" size="sm" onClick={() => action.mutate({ kind: 'command', command: { command: 'focus_workspace', id: workspace.id } })} disabled={action.isPending}><Crosshair size={13} /> {t('ws.focus')}</Button>}
        {workspace && <Button variant="ghost" size="sm" onClick={() => action.mutate({ kind: 'command', command: { command: 'move_window_to_workspace', window: null, workspace: workspace.id } })} disabled={action.isPending}><ArrowRight size={13} /> {t('ws.moveFocused')}</Button>}
        {workspace && !renaming && <Button variant="ghost" size="sm" onClick={() => { setRenaming(true); setName(workspace.name) }}><Pencil size={13} /> {t('ws.rename')}</Button>}
        {workspace && <Button variant="ghost" size="sm" onClick={unbind} disabled={patch.isPending}><Unlink size={13} /> {t('ws.unbind')}</Button>}
        {!naming && target && <ConfirmButton size="sm" variant="secondary" prompt={t('ws.removeConfirm', { name: target.name })} disabled={patch.isPending} onConfirm={() => { patch.mutate({ revision, patch: { kind: 'remove_workspace', id: target.id } }); setSelectedId(0) }}><Trash2 size={13} /> {t('ws.remove')}</ConfirmButton>}
        {!naming && target && target.id !== workspace?.id && <Button size="sm" onClick={bind} disabled={patch.isPending}><Link2 size={13} /> {patch.isPending ? t('common.saving') : t('ws.save')}</Button>}
      </div>
    </div>
    {patch.isError && <p className="bad-text" style={{ margin: 0 }}>{patch.error.message}</p>}{action.isError && <p className="bad-text" style={{ margin: 0 }}>{action.error.message}</p>}
    {patch.isSuccess && <p className="good-text" style={{ margin: 0 }}>{t('ws.saved')}</p>}
    {showWindows && <div className="ws-windows">{windows.isPending ? <Loading /> : (() => { const here = windows.data?.windows.filter((window) => window.native_space_id === space.id) ?? []; return here.length ? <div className="table-wrap"><table className="table"><tbody>{here.map((window) => <tr key={`${window.pid}-${window.token}`}><td>{window.application_id ?? t('ws.process', { pid: window.pid })}</td><td className="mono faint" style={{ textAlign: 'right' }}>CG {window.cg_window_id ?? '?'}</td></tr>)}</tbody></table></div> : <p className="row-sub" style={{ padding: '12px 20px', margin: 0 }}>{t('ws.noWindows')}</p> })()}</div>}
  </div>
}

/**
 * Adds a Desktop on the display and binds the first unbound workspace to it,
 * so its Option+digit works right away.
 */
function NewDesktop({ display, workspaces, revision }: { display: number; workspaces: WorkspaceDefinition[]; revision: string }) {
  const t = useT()
  const client = useQueryClient()
  const patch = useConfigPatch()
  const [note, setNote] = useState('')
  const create = useMutation({
    mutationFn: () => createDesktop(display),
    onSuccess: async ({ space }) => {
      const free = workspaces.filter((ws) => ws.native_space_id === null && !(ws.native_space_ids ?? []).length).sort((a, b) => a.id - b.id)[0]
      if (free) {
        patch.mutate({ revision, patch: bindSpacePatch(free, space.id, free.name) })
        setNote(t('ws.createdBound', { desktop: space.desktop, id: free.id, name: free.name }))
      } else {
        // Every workspace is taken: make a new one for this Desktop.
        const name = t('ws.defaultName', { desktop: space.desktop })
        patch.mutate({ revision, patch: { kind: 'create_workspace', name, display: null, native_space_id: space.id } })
        setNote(t('ws.createdNew', { desktop: space.desktop, name }))
      }
      await client.invalidateQueries({ queryKey: ['daemon'] })
    },
  })
  return <div className="list-actions" style={{ flexWrap: 'wrap', justifyContent: 'flex-end' }}>
    {create.isError && <span className="bad-text" style={{ fontSize: 12 }}>{create.error.message}</span>}
    {patch.isError && <span className="bad-text" style={{ fontSize: 12 }}>{patch.error.message}</span>}
    {note && !create.isPending && <span className="good-text" style={{ fontSize: 12 }}>{note}</span>}
    <Button size="sm" variant="primary" disabled={create.isPending} onClick={() => { setNote(''); create.mutate() }}><Plus size={13} /> {create.isPending ? t('ws.creating') : t('ws.newDesktop')}</Button>
  </div>
}

export function Workspaces() {
  const t = useT()
  const status = useDaemon('status')
  const online = status.isSuccess
  const spaces = useDaemon('spaces', online)
  const config = useDaemon('config', online)
  const hotkeys = useDaemon('hotkeys', online)
  const scratchpads = useDaemon('scratchpads', online)
  const parking = new Set(Object.values(scratchpads.data?.parking ?? {}))
  const displays = [...new Set(spaces.data?.spaces.map((space) => space.display) ?? [])]
  return <><div className="page-heading"><div><span className="eyebrow">{t('ws.eyebrow')}</span><h1>{t('nav.workspaces')}</h1><p>{t('ws.intro')}</p></div></div>
    {!online ? !status.isFetched ? <Loading /> : <ErrorState message={t('ws.offline')} retry={() => void status.refetch()} /> : spaces.isPending || config.isPending ? <Loading /> : spaces.isError || config.isError ? <ErrorState message={spaces.error?.message ?? config.error?.message ?? t('common.unknownError')} retry={() => { void spaces.refetch(); void config.refetch() }} /> : <>
      {spaces.data.spaces.length === 0 ? <Empty title={t('ws.none')}>{t('ws.noneHint')}</Empty> : displays.map((display) => <Card key={display} title={t('common.display', { id: display })} description={t('ws.desktopCount', { count: spaces.data.spaces.filter((space) => space.display === display).length })} aside={<NewDesktop display={display} workspaces={config.data.config.workspaces} revision={config.data.revision} />}>
        <div>{spaces.data.spaces.filter((space) => space.display === display).map((space) => {
          if (parking.has(space.id)) return <div key={space.id} className="ws-row"><div className="ws-line"><div><span className="space-dot inactive" /><span className="ws-title">{t('dashboard.desktop', { n: space.desktop })}</span><Badge>{t('ws.nativeId')} {space.id}</Badge><Badge tone="info">{t('ws.parking')}</Badge></div></div><div className="row-sub" style={{ marginTop: 0 }}>{t('ws.parkingHint')}</div></div>
          const workspace = config.data.config.workspaces.find((entry) => bindsSpace(entry, space.id))
          // Focus hotkeys name a workspace by ID or by name; both count.
          const chord = workspace && hotkeys.data?.hotkeys.find(({ command }) =>
            (command.command === 'focus_workspace' && 'id' in command && command.id === workspace.id)
            || (command.command === 'focus_named_workspace' && 'name' in command && command.name === workspace.name))?.chord
          return <WorkspaceEditor key={`${space.id}-${workspace?.id}-${config.data.revision}`} space={space} workspace={workspace} all={config.data.config.workspaces} spaces={spaces.data.spaces} revision={config.data.revision} chord={workspace ? chord : undefined} />
        })}</div>
      </Card>)}
      <Notice>{t('ws.notice')}</Notice>
    </>}
  </>
}
