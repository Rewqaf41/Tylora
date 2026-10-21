import { useState } from 'react'
import { ArrowRight, ChevronDown, ChevronUp, Crosshair, Link2, Unlink } from 'lucide-react'
import { useAction, useConfigPatch, useDaemon } from '../api/hooks'
import { bindSpacePatch, bindsSpace, unbindSpacePatch } from '../api/types'
import type { NativeSpace, WorkspaceDefinition } from '../api/types'
import { useT } from '../i18n'
import { Badge, Button, Card, Empty, ErrorState, Keys, Loading, Notice } from '../shared/ui'

function WorkspaceEditor({ workspace, space, revision, all, spaces, chord }: { workspace: WorkspaceDefinition | undefined; space: NativeSpace; revision: string; all: WorkspaceDefinition[]; spaces: NativeSpace[]; chord?: string }) {
  const t = useT()
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
  return <div className={`ws-row${space.active ? ' active' : ''}`}>
    <div className="ws-line">
      <div><span className={`space-dot ${space.active ? '' : 'inactive'}`} /><span className="ws-title">{t('dashboard.desktop', { n: space.desktop })}</span><Badge tone={space.active ? 'accent' : 'neutral'}>{t('ws.nativeId')} {space.id}</Badge>{space.active && <Badge tone="good">{t('common.active')}</Badge>}</div>
      <div>
        <label className="inline-label" htmlFor={`workspace-${space.id}`}>{t('ws.logical')}</label>
        <select id={`workspace-${space.id}`} value={selectedId} onChange={(event) => { const id = Number(event.target.value); setSelectedId(id); setName(all.find((ws) => ws.id === id)?.name ?? '') }}>
          {all.filter(selectable).map((entry) => <option key={entry.id} value={entry.id}>{entry.id} · {entry.name}{entry.native_space_id !== null && !bindsSpace(entry, space.id) ? t('ws.alsoElsewhere') : ''}</option>)}
        </select>
        <span className="field"><input id={`name-${space.id}`} aria-label={t('common.name')} placeholder={t('common.name')} value={name} maxLength={60} onChange={(event) => setName(event.target.value)} /></span>
        <Button variant="ghost" size="sm" aria-expanded={showWindows} onClick={() => setShowWindows(!showWindows)}>{showWindows ? t('ws.hideWindows') : t('ws.showWindows')}{showWindows ? <ChevronUp size={13} /> : <ChevronDown size={13} />}</Button>
      </div>
    </div>
    <div className="ws-line">
      <div className="row-sub" style={{ marginTop: 0 }}>{chord ? <>{t('ws.focusShortcut')} <Keys chord={chord} /></> : null}{elsewhere.length > 0 && <span>{t('ws.alsoBound', { spaces: elsewhere.join(', ') })}</span>}</div>
      <div>
        {workspace && <Button variant="ghost" size="sm" onClick={() => action.mutate({ kind: 'command', command: { command: 'focus_workspace', id: workspace.id } })} disabled={action.isPending}><Crosshair size={13} /> {t('ws.focus')}</Button>}
        {workspace && <Button variant="ghost" size="sm" onClick={() => action.mutate({ kind: 'command', command: { command: 'move_window_to_workspace', window: null, workspace: workspace.id } })} disabled={action.isPending}><ArrowRight size={13} /> {t('ws.moveFocused')}</Button>}
        {workspace && <Button variant="ghost" size="sm" onClick={unbind} disabled={patch.isPending}><Unlink size={13} /> {t('ws.unbind')}</Button>}
        <Button size="sm" onClick={save} disabled={patch.isPending || !target || !name.trim()}><Link2 size={13} /> {patch.isPending ? t('common.saving') : t('ws.save')}</Button>
      </div>
    </div>
    {patch.isError && <p className="bad-text" style={{ margin: 0 }}>{patch.error.message}</p>}{action.isError && <p className="bad-text" style={{ margin: 0 }}>{action.error.message}</p>}
    {patch.isSuccess && <p className="good-text" style={{ margin: 0 }}>{t('ws.saved')}</p>}
    {showWindows && <div className="ws-windows">{windows.isPending ? <Loading /> : (() => { const here = windows.data?.windows.filter((window) => window.native_space_id === space.id) ?? []; return here.length ? <div className="table-wrap"><table className="table"><tbody>{here.map((window) => <tr key={`${window.pid}-${window.token}`}><td>{window.application_id ?? t('ws.process', { pid: window.pid })}</td><td className="mono faint" style={{ textAlign: 'right' }}>CG {window.cg_window_id ?? '?'}</td></tr>)}</tbody></table></div> : <p className="row-sub" style={{ padding: '12px 20px', margin: 0 }}>{t('ws.noWindows')}</p> })()}</div>}
  </div>
}

export function Workspaces() {
  const t = useT()
  const status = useDaemon('status')
  const online = status.isSuccess
  const spaces = useDaemon('spaces', online)
  const config = useDaemon('config', online)
  const hotkeys = useDaemon('hotkeys', online)
  const displays = [...new Set(spaces.data?.spaces.map((space) => space.display) ?? [])]
  return <><div className="page-heading"><div><span className="eyebrow">{t('ws.eyebrow')}</span><h1>{t('nav.workspaces')}</h1><p>{t('ws.intro')}</p></div></div>
    {!online ? status.isPending ? <Loading /> : <ErrorState message={t('ws.offline')} retry={() => void status.refetch()} /> : spaces.isPending || config.isPending ? <Loading /> : spaces.isError || config.isError ? <ErrorState message={spaces.error?.message ?? config.error?.message ?? t('common.unknownError')} retry={() => { void spaces.refetch(); void config.refetch() }} /> : <>
      {spaces.data.spaces.length === 0 ? <Empty title={t('ws.none')}>{t('ws.noneHint')}</Empty> : displays.map((display) => <Card key={display} title={t('common.display', { id: display })} description={t('ws.desktopCount', { count: spaces.data.spaces.filter((space) => space.display === display).length })}>
        <div>{spaces.data.spaces.filter((space) => space.display === display).map((space) => {
          const workspace = config.data.config.workspaces.find((entry) => bindsSpace(entry, space.id))
          const chord = hotkeys.data?.hotkeys.find((hotkey) => hotkey.command.command === 'focus_workspace' && 'id' in hotkey.command && hotkey.command.id === workspace?.id)?.chord
          return <WorkspaceEditor key={`${space.id}-${workspace?.id}-${config.data.revision}`} space={space} workspace={workspace} all={config.data.config.workspaces} spaces={spaces.data.spaces} revision={config.data.revision} chord={workspace ? chord : undefined} />
        })}</div>
      </Card>)}
      <Notice>{t('ws.notice')}</Notice>
    </>}
  </>
}
