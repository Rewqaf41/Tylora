import { useState } from 'react'
import { Plus, Trash2 } from 'lucide-react'
import { useConfigPatch, useDaemon } from '../api/hooks'
import type { WindowRule } from '../api/types'
import { Badge, Button, Card, ConfirmButton, Empty, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

const blankRule: WindowRule = { application_id: '', mode: 'floating', priority: 0, enabled: true }

export function WindowRules() {
  const status = useDaemon('status')
  const config = useDaemon('config', status.isSuccess)
  const windows = useDaemon('windows', status.isSuccess)
  const patch = useConfigPatch()
  const [editing, setEditing] = useState<number | null | undefined>(undefined)
  const [rule, setRule] = useState<WindowRule>(blankRule)
  const apps = [...new Set(windows.data?.windows.map((window) => window.application_id).filter((id): id is string => Boolean(id)) ?? [])].sort()
  const rules = config.data?.config.window_rules ?? []
  const valid = rule.application_id.trim().length > 0 && rule.application_id.length <= 255 && Number.isInteger(rule.priority) && rule.priority >= -100 && rule.priority <= 100
  const open = (index: number | null) => { setEditing(index); setRule(index === null ? blankRule : { ...rules[index]! }); patch.reset() }
  const save = () => {
    if (!config.data || !valid) return
    patch.mutate({ revision: config.data.revision, patch: { kind: 'set_window_rule', index: editing ?? null, rule: { ...rule, application_id: rule.application_id.trim() } } }, { onSuccess: () => setEditing(undefined) })
  }
  const remove = (index: number) => {
    if (!config.data) return
    patch.mutate({ revision: config.data.revision, patch: { kind: 'remove_window_rule', index } }, { onSuccess: () => setEditing(undefined) })
  }
  return <><div className="page-heading"><div><span className="eyebrow">Window behavior</span><h1>Window Rules</h1><p>Set an initial floating or tiled mode for windows from an application.</p></div><Button variant="primary" disabled={!status.isSuccess || !config.data} onClick={() => open(null)}><Plus size={14} /> New rule</Button></div>
    {!status.isSuccess ? status.isPending ? <Loading /> : <ErrorState message="Connect to the daemon to edit rules." retry={() => void status.refetch()} /> : config.isPending ? <Loading /> : config.isError ? <ErrorState message={config.error.message} retry={() => void config.refetch()} /> : <>
      <SectionHeader title="Configured rules" description="Exact application bundle IDs · highest priority wins" />
      {rules.length === 0 ? <Empty title="No window rules">Create a rule for an observed app or enter its bundle ID.</Empty> : <Card>{rules.map((item, index) => <div className="list-item" key={`${index}-${item.application_id}`}><div><div className="list-primary mono">{item.application_id}</div><div className="list-secondary">Priority {item.priority} · {item.mode === 'floating' ? 'Start floating' : 'Start tiled'}</div></div><div className="list-actions"><Badge tone={item.enabled ? 'good' : 'neutral'}>{item.enabled ? 'Active' : 'Disabled'}</Badge><Button variant="ghost" onClick={() => open(index)}>Edit</Button><ConfirmButton variant="secondary" prompt={`Remove rule for ${item.application_id}?`} disabled={patch.isPending} onConfirm={() => remove(index)}><Trash2 size={13} /></ConfirmButton></div></div>)}</Card>}
      {editing !== undefined && <><SectionHeader title={editing === null ? 'New rule' : 'Edit rule'} description="The rule applies to new windows and after the next configuration reload." /><Card className="padded stack">
        <div className="form-grid"><div className="field"><label htmlFor="rule-app">APPLICATION BUNDLE ID</label><input id="rule-app" list="observed-apps" value={rule.application_id} maxLength={255} placeholder="com.example.Editor" onChange={(event) => setRule({ ...rule, application_id: event.target.value })} /><datalist id="observed-apps">{apps.map((app) => <option value={app} key={app} />)}</datalist></div>
          <div className="field"><label htmlFor="rule-mode">INITIAL MODE</label><select id="rule-mode" value={rule.mode} onChange={(event) => setRule({ ...rule, mode: event.target.value as WindowRule['mode'] })}><option value="floating">Floating</option><option value="tiled">Tiled</option></select></div>
          <div className="field"><label htmlFor="rule-priority">PRIORITY · −100 TO 100</label><input id="rule-priority" type="number" min="-100" max="100" step="1" value={rule.priority} onChange={(event) => setRule({ ...rule, priority: Number(event.target.value) })} /></div></div>
        <div className="form-actions"><label className="row" style={{ marginRight: 'auto', gap: 7 }}><input type="checkbox" checked={rule.enabled} onChange={(event) => setRule({ ...rule, enabled: event.target.checked })} /> Enabled</label><Button onClick={() => setEditing(undefined)}>Cancel</Button><Button variant="primary" disabled={!valid || patch.isPending} onClick={save}>{patch.isPending ? 'Saving…' : 'Save rule'}</Button></div>
        {!valid && <p className="bad-text">Enter a bundle ID and an integer priority between −100 and 100.</p>}
      </Card></>}
      {patch.isError && <p className="bad-text">{patch.error.message}</p>}{patch.isSuccess && <p className="good-text">Rule saved and applied by daemon.</p>}
      <div style={{ marginTop: 18 }}><Notice>Rules match exact bundle IDs. Title patterns, preferred workspace, and preferred display are unavailable until the daemon can enforce them. A manual floating toggle for a window remains in effect until rules are reloaded.</Notice></div>
    </>}
  </>
}
