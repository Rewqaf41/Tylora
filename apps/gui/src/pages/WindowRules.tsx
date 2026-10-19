import { useMemo, useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { ArrowDown, ArrowUp, GripVertical, Play, Plus, RefreshCw, Search, Trash2 } from 'lucide-react'
import { applyRule, reloadRules, testRule } from '../api/bridge'
import { useConfigPatch, useDaemon } from '../api/hooks'
import type { ApplyPolicy, ConfigPatch, ListedRule, RuleCandidate, RuleDefinition, RuleModeKind, RuleTest, TitleMatch } from '../api/types'
import { tEn, useT, type MessageKey, type T } from '../i18n'
import { Badge, Button, Card, Empty, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

const MODE_LABELS: Record<RuleModeKind, MessageKey> = { unchanged: 'rules.mode.unchanged', tiled: 'rules.mode.tiled', floating: 'rules.mode.floating', unmanaged: 'rules.mode.unmanaged' }
const APPLY_LABELS: Record<ApplyPolicy, MessageKey> = { on_create: 'rules.apply.onCreate', on_first_seen: 'rules.apply.onFirstSeen', on_title_change: 'rules.apply.onTitleChange', manual: 'rules.apply.manual' }
const TITLE_LABELS: Record<TitleMatch, MessageKey> = { exact: 'rules.title.exact', contains: 'rules.title.contains', regex: 'rules.title.regex' }

export const blankRule: RuleDefinition = { id: '', name: '', enabled: true, bundle_id: '', title: null, title_match: 'exact', workspace: null, display: null, scratchpad: null, mode: 'unchanged', priority: 0, apply: 'on_create', stop: true }

export function slug(text: string): string {
  return text.toLowerCase().normalize('NFKD').replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 64)
}

/** Client-side checks mirroring the daemon's validation. The daemon has the final say. */
export function ruleProblem(rule: RuleDefinition, others: RuleDefinition[], workspaces: string[], scratchpads: string[] = [], t: T = tEn): string | null {
  const id = rule.id.trim()
  if (!id || id.length > 64) return t('rules.err.id')
  if (others.some((other) => other.id.toLowerCase() === id.toLowerCase())) return t('rules.err.duplicate', { id })
  if (!rule.bundle_id.trim()) return t('rules.err.bundle')
  if (!Number.isInteger(rule.priority) || rule.priority < -1000 || rule.priority > 1000) return t('rules.err.priority')
  if (rule.title !== null && rule.title !== undefined && rule.title.length === 0) return t('rules.err.title')
  if (rule.title && rule.title_match === 'regex') { try { new RegExp(rule.title) } catch { return t('rules.err.regex') } }
  if (rule.workspace && !workspaces.some((name) => name.toLowerCase() === rule.workspace!.toLowerCase())) return t('rules.err.workspace', { name: rule.workspace! })
  if (rule.scratchpad) {
    if (!scratchpads.some((id) => id.toLowerCase() === rule.scratchpad!.toLowerCase())) return t('rules.err.scratchpad', { id: rule.scratchpad! })
    if (rule.workspace || rule.display !== null && rule.display !== undefined || rule.mode !== 'unchanged') return t('rules.err.scratchpadExclusive')
    return null
  }
  if (rule.mode === 'unmanaged' && (rule.workspace || rule.display !== null && rule.display !== undefined)) return t('rules.err.unmanaged')
  if (rule.mode === 'unchanged' && !rule.workspace && (rule.display === null || rule.display === undefined)) return t('rules.err.noAction')
  return null
}

/** New priorities for a drag-and-drop order: highest first, spaced by 10. */
export function reorderedPriorities(order: string[]): { id: string; priority: number }[] {
  return order.map((id, index) => ({ id, priority: (order.length - index) * 10 }))
}

export function describeActions(rule: RuleDefinition, t: T = tEn): string {
  const parts: string[] = []
  if (rule.scratchpad) return t('rules.describe.scratchpad', { id: rule.scratchpad })
  if (rule.workspace) parts.push(`→ ${rule.workspace}`)
  if (rule.display !== null && rule.display !== undefined) parts.push(t('rules.describe.display', { id: rule.display }))
  if (rule.mode !== 'unchanged') parts.push(t(MODE_LABELS[rule.mode]))
  return parts.join(' · ') || t('rules.describe.none')
}

function checkText(t: T, candidate: RuleCandidate): string {
  const check = candidate.check
  switch (check.result) {
    case 'applied': return t('rules.check.applied', { actions: check.actions.join(', ') })
    case 'overridden': return t('rules.check.overridden')
    case 'disabled': return t('common.disabled')
    case 'bundle_mismatch': return t('rules.check.bundle')
    case 'title_mismatch': return t('rules.check.title')
    case 'policy_skipped': return t('rules.check.policy', { policy: t(APPLY_LABELS[check.policy]) })
    case 'stopped': return t('rules.check.stopped', { rule: check.by })
  }
}

function RulePreview({ test, onApply, applying, applyError, applied }: { test: RuleTest; onApply: () => void; applying: boolean; applyError: string | null; applied: string | null }) {
  const t = useT()
  const [confirming, setConfirming] = useState(false)
  const target = test.target
  const moves = test.manual.plan.workspace !== null || test.manual.plan.display !== null
  return <Card className="padded stack">
    <div className="row"><div><span className="row-title">{test.window.title || t('common.untitled')} </span><span className="row-sub mono">{test.window.application_id} · CG {test.window.cg_window_id} · {t('rules.describe.display', { id: test.window.display })} · Space {test.window.native_space_id ?? '?'} · {test.window.mode}</span></div>
      {test.manual_override ? <Badge tone="warn">{t('rules.override')}</Badge> : <Badge tone="neutral">{t('rules.noOverride')}</Badge>}</div>
    <div className="table-wrap"><table className="table"><thead><tr><th>{t('rules.colRule')}</th><th>{t('rules.colPriority')}</th><th>{t('rules.colAutomatic', { trigger: test.automatic.trigger.replace('_', ' ') })}</th><th>{t('rules.colManual')}</th></tr></thead><tbody>
      {test.manual.candidates.length === 0 ? <tr><td colSpan={4} className="muted">{t('rules.noCandidates')}</td></tr> : test.manual.candidates.map((candidate, index) => <tr key={candidate.id}>
        <td><strong>{candidate.name}</strong>{candidate.legacy && <span className="muted"> {t('rules.legacyMark')}</span>}{test.manual.winner === candidate.id && <> <Badge tone="good">{t('rules.winner')}</Badge></>}</td><td className="mono">{candidate.priority}</td>
        <td>{test.automatic.candidates[index] ? checkText(t, test.automatic.candidates[index]!) : '—'}</td><td>{checkText(t, candidate)}</td></tr>)}
    </tbody></table></div>
    <div className="row-sub">{t('rules.planned')} {test.manual.plan.workspace ? t('rules.planWorkspace', { name: test.manual.plan.workspace[0] }) : t('rules.planNoMove')}{test.manual.plan.display ? ` · ${t('rules.describe.display', { id: test.manual.plan.display[0] })}` : ''}{test.manual.plan.mode ? ` · ${t(MODE_LABELS[test.manual.plan.mode[0]])}` : ''}
      {target && (typeof target === 'string' ? ` · ${target}` : 'error' in target ? ` · ${t('rules.cannotMove', { error: target.error })}` : ` · ${t('rules.targetSpace', { space: target.native_space_id, display: target.display })}`)}</div>
    {test.tracker?.last_error && <p className="bad-text">{t('rules.lastFailed', { error: test.tracker.last_error })}</p>}
    <div className="form-actions">{!confirming ? <Button variant="primary" disabled={!test.manual.winner || applying} onClick={() => setConfirming(true)}><Play size={13} /> {t('rules.applyNow')}</Button> : <>
      <span className="row-sub" style={{ marginRight: 'auto' }}>{moves ? t('rules.confirmMove') : t('rules.confirmMode')} {t('rules.continue')}</span>
      <Button onClick={() => setConfirming(false)}>{t('common.cancel')}</Button><Button variant="primary" disabled={applying} onClick={() => { setConfirming(false); onApply() }}>{applying ? t('common.applying') : t('common.confirm')}</Button></>}</div>
    {applyError && <p className="bad-text">{applyError}</p>}{applied && <p className="good-text">{applied}</p>}
  </Card>
}

export function WindowRules() {
  const t = useT()
  const status = useDaemon('status')
  const online = status.isSuccess
  const config = useDaemon('config', online)
  const listed = useDaemon('rules', online)
  const windows = useDaemon('windows', online)
  const apps = useDaemon('apps', online)
  const displays = useDaemon('displays', online)
  const diagnostics = useDaemon('rule_diagnostics', online)
  const validation = useDaemon('rule_validation', online)
  const patch = useConfigPatch()
  const client = useQueryClient()
  const [editing, setEditing] = useState<string | null | undefined>(undefined)
  const [draft, setDraft] = useState<RuleDefinition>(blankRule)
  const [titleOn, setTitleOn] = useState(false)
  const [dragged, setDragged] = useState<string | null>(null)
  const [previewWindow, setPreviewWindow] = useState<number | null>(null)
  const [filter, setFilter] = useState('')
  const preview = useMutation({ mutationFn: testRule })
  const apply = useMutation({ mutationFn: applyRule, onSuccess: async () => { if (previewWindow !== null) preview.mutate(previewWindow); await client.invalidateQueries({ queryKey: ['daemon'] }) } })
  const reload = useMutation({ mutationFn: reloadRules, onSuccess: async () => { await client.invalidateQueries({ queryKey: ['daemon'] }) } })
  const rules = useMemo(() => [...(config.data?.config.rules ?? [])].sort((a, b) => b.priority - a.priority), [config.data?.revision])
  const legacy = config.data?.config.window_rules ?? []
  const workspaces = config.data?.config.workspaces ?? []
  const workspaceNames = workspaces.map((ws) => ws.name)
  const others = rules.filter((rule) => rule.id !== editing)
  const scratchpadIds = (config.data?.config.scratchpads ?? []).map((pad) => pad.id)
  const problem = editing !== undefined ? ruleProblem(draft, others, workspaceNames, scratchpadIds, t) : null
  const send = (next: ConfigPatch, done?: () => void) => { if (config.data) patch.mutate({ revision: config.data.revision, patch: next }, { onSuccess: done }) }
  const open = (rule: RuleDefinition | null) => {
    const next = rule ? { ...rule, title: rule.title ?? null, workspace: rule.workspace ?? null, display: rule.display ?? null, scratchpad: rule.scratchpad ?? null } : blankRule
    setDraft(next); setTitleOn(Boolean(next.title)); setEditing(rule ? rule.id : null); patch.reset()
  }
  const save = () => {
    if (problem) return
    const rule: RuleDefinition = { ...draft, id: draft.id.trim(), bundle_id: draft.bundle_id.trim(), title: titleOn ? draft.title ?? '' : null }
    send({ kind: 'set_rule', previous_id: editing ?? null, rule }, () => setEditing(undefined))
  }
  const drop = (target: string) => {
    if (!dragged || dragged === target) return
    const order = rules.map((rule) => rule.id).filter((id) => id !== dragged)
    order.splice(order.indexOf(target), 0, dragged)
    send({ kind: 'set_rule_priorities', priorities: reorderedPriorities(order) })
    setDragged(null)
  }
  const move = (index: number, delta: number) => {
    const order = rules.map((rule) => rule.id)
    const [item] = order.splice(index, 1)
    order.splice(index + delta, 0, item!)
    send({ kind: 'set_rule_priorities', priorities: reorderedPriorities(order) })
  }
  const previewable = (windows.data?.windows ?? []).filter((window) => window.cg_window_id !== null)
    .filter((window) => !filter || `${window.application_id} ${window.title ?? ''}`.toLowerCase().includes(filter.toLowerCase()))
  const listedById = new Map((listed.data?.rules ?? []).map((rule: ListedRule) => [rule.id, rule]))

  return <><div className="page-heading"><div><span className="eyebrow">{t('rules.eyebrow')}</span><h1>{t('nav.rules')}</h1><p>{t('rules.intro')}</p></div>
    <div className="page-actions"><Button disabled={!online || reload.isPending} onClick={() => reload.mutate()}><RefreshCw size={13} /> {t('rules.reload')}</Button><Button variant="primary" disabled={!online || !config.data} onClick={() => open(null)}><Plus size={14} /> {t('rules.new')}</Button></div></div>
    {!online ? status.isPending ? <Loading /> : <ErrorState message={t('rules.offline')} retry={() => void status.refetch()} /> : config.isPending ? <Loading /> : config.isError ? <ErrorState message={config.error.message} retry={() => void config.refetch()} /> : <>
      {validation.data && !validation.data.ok && <Notice tone="warn">{validation.data.problems.join(' · ')}</Notice>}
      <SectionHeader title={t('rules.list')} description={t('rules.listHint')} />
      {rules.length === 0 ? <Empty title={t('rules.none')}>{t('rules.noneHint')}</Empty> : <Card>{rules.map((rule, index) => <div className="list-item" key={rule.id} draggable onDragStart={() => setDragged(rule.id)} onDragOver={(event) => event.preventDefault()} onDrop={() => drop(rule.id)} style={{ opacity: dragged === rule.id ? 0.5 : 1 }}>
        <div className="row" style={{ gap: 10, justifyContent: 'flex-start' }}><GripVertical size={14} className="muted" aria-label={t('rules.drag')} /><div><div className="list-primary">{rule.name || rule.id} <span className="muted mono">#{rule.priority}</span></div>
          <div className="list-secondary"><span className="mono">{rule.bundle_id}</span>{rule.title ? ` · ${t('rules.titleCondition', { match: t(TITLE_LABELS[rule.title_match]), title: rule.title })}` : ''} · {describeActions(rule, t)} · {t(APPLY_LABELS[rule.apply])}{rule.stop ? '' : ` · ${t('rules.continues')}`}</div></div></div>
        <div className="list-actions">
          <label className="row" style={{ gap: 5 }}><input type="checkbox" aria-label={t('rules.enable', { id: rule.id })} checked={rule.enabled} disabled={patch.isPending} onChange={(event) => send({ kind: 'set_rule', previous_id: rule.id, rule: { ...rule, enabled: event.target.checked } })} />{rule.enabled ? t('rules.on') : t('rules.off')}</label>
          <Button variant="ghost" aria-label={t('rules.moveUp')} disabled={index === 0 || patch.isPending} onClick={() => move(index, -1)}><ArrowUp size={13} /></Button>
          <Button variant="ghost" aria-label={t('rules.moveDown')} disabled={index === rules.length - 1 || patch.isPending} onClick={() => move(index, 1)}><ArrowDown size={13} /></Button>
          {!listedById.has(rule.id) && <Badge tone="warn">{t('rules.notLoaded')}</Badge>}
          <Button variant="ghost" onClick={() => open(rule)}>{t('common.edit')}</Button>
          <Button variant="ghost" aria-label={t('common.deleteItem', { id: rule.id })} disabled={patch.isPending} onClick={() => send({ kind: 'remove_rule', id: rule.id })}><Trash2 size={13} /></Button></div>
      </div>)}</Card>}

      {legacy.length > 0 && <><SectionHeader title={t('rules.legacy')} description={t('rules.legacyHint')} />
        <Card>{legacy.map((item, index) => <div className="list-item" key={`${index}-${item.application_id}`}><div><div className="list-primary mono">{item.application_id}</div><div className="list-secondary">{t('rules.legacyLine', { priority: item.priority, mode: item.mode, state: item.enabled ? t('common.enabled') : t('common.disabled') })}</div></div>
          <div className="list-actions"><Button disabled={patch.isPending} onClick={() => send({ kind: 'convert_window_rule', index, id: slug(item.application_id) || `rule-${index + 1}` })}>{t('rules.convert')}</Button></div></div>)}</Card></>}

      {editing !== undefined && <><SectionHeader title={editing === null ? t('rules.new') : t('common.editItem', { id: editing })} description={t('rules.editHint')} /><Card className="padded stack">
        <div className="form-grid">
          <div className="field"><label htmlFor="rule-name">{t('common.name')}</label><input id="rule-name" value={draft.name} maxLength={80} placeholder={t('rules.namePlaceholder')} onChange={(event) => setDraft({ ...draft, name: event.target.value, id: editing === null && (!draft.id || draft.id === slug(draft.name)) ? slug(event.target.value) : draft.id })} /></div>
          <div className="field"><label htmlFor="rule-id">ID</label><input id="rule-id" className="mono" value={draft.id} maxLength={64} onChange={(event) => setDraft({ ...draft, id: event.target.value })} /></div>
          <div className="field"><label htmlFor="rule-app">{t('rules.application')}</label><select id="rule-app" value={apps.data?.apps.some((app) => app.bundle_id === draft.bundle_id) ? draft.bundle_id : ''} onChange={(event) => setDraft({ ...draft, bundle_id: event.target.value })}>
            <option value="">{apps.isPending ? t('rules.loadingApps') : t('rules.chooseApp')}</option>
            {apps.data?.apps.map((app) => <option key={app.bundle_id} value={app.bundle_id}>{app.name ?? app.bundle_id} — {app.bundle_id}</option>)}</select></div>
          <div className="field"><label htmlFor="rule-bundle">{t('rules.bundleId')}</label><input id="rule-bundle" className="mono" value={draft.bundle_id} maxLength={255} placeholder="com.microsoft.VSCode" onChange={(event) => setDraft({ ...draft, bundle_id: event.target.value })} /></div>
          <div className="field"><label htmlFor="rule-workspace">{t('rules.moveToWorkspace')}</label><select id="rule-workspace" value={draft.workspace ?? ''} onChange={(event) => setDraft({ ...draft, workspace: event.target.value || null })}><option value="">{t('rules.dontMove')}</option>{workspaces.map((ws) => <option key={ws.id} value={ws.name}>{ws.name}{ws.native_space_id === null ? ` ${t('rules.unbound')}` : ''}</option>)}</select></div>
          <div className="field"><label htmlFor="rule-display">{t('rules.onDisplay')}</label><select id="rule-display" value={draft.display ?? ''} onChange={(event) => setDraft({ ...draft, display: event.target.value === '' ? null : Number(event.target.value) })}><option value="">{t('rules.anyDisplay')}</option>{displays.data?.displays.map((display) => <option key={display.id} value={display.id}>{t('common.display', { id: display.id })}{display.primary ? ` ${t('common.primaryMark')}` : ''} · {display.full_frame.width}×{display.full_frame.height}</option>)}</select></div>
          <div className="field"><label htmlFor="rule-mode">{t('rules.windowMode')}</label><select id="rule-mode" value={draft.mode} onChange={(event) => setDraft({ ...draft, mode: event.target.value as RuleModeKind })}>{(Object.keys(MODE_LABELS) as RuleModeKind[]).map((mode) => <option key={mode} value={mode}>{t(MODE_LABELS[mode])}</option>)}</select></div>
          <div className="field"><label htmlFor="rule-scratchpad">{t('rules.toScratchpad')}</label><select id="rule-scratchpad" value={draft.scratchpad ?? ''} onChange={(event) => setDraft({ ...draft, scratchpad: event.target.value || null })}><option value="">{t('rules.noScratchpad')}</option>{scratchpadIds.map((id) => <option key={id} value={id}>{id}</option>)}</select></div>
          <div className="field"><label htmlFor="rule-apply">{t('rules.applyTo')}</label><select id="rule-apply" value={draft.apply} onChange={(event) => setDraft({ ...draft, apply: event.target.value as ApplyPolicy })}>{(Object.keys(APPLY_LABELS) as ApplyPolicy[]).map((policy) => <option key={policy} value={policy}>{t(APPLY_LABELS[policy])}</option>)}</select></div>
          <div className="field"><label htmlFor="rule-priority">{t('rules.priority')}</label><input id="rule-priority" type="number" min="-1000" max="1000" step="1" value={draft.priority} onChange={(event) => setDraft({ ...draft, priority: Number(event.target.value) })} /></div>
          <div className="field"><label htmlFor="rule-title-match">{t('rules.windowTitle')}</label><div className="range-row" style={{ gridTemplateColumns: '130px 1fr' }}>
            <select id="rule-title-match" value={titleOn ? draft.title_match : 'any'} onChange={(event) => { const value = event.target.value; if (value === 'any') { setTitleOn(false) } else { setTitleOn(true); setDraft({ ...draft, title_match: value as TitleMatch, title: draft.title ?? '' }) } }}>
              <option value="any">{t('rules.anyTitle')}</option><option value="exact">{t('rules.equals')}</option><option value="contains">{t('rules.contains')}</option><option value="regex">{t('rules.regex')}</option></select>
            <input aria-label={t('rules.titleConditionLabel')} disabled={!titleOn} value={titleOn ? draft.title ?? '' : ''} maxLength={512} placeholder={draft.title_match === 'regex' ? '^Project \\d+' : t('rules.titleText')} onChange={(event) => setDraft({ ...draft, title: event.target.value })} /></div></div>
        </div>
        <div className="form-actions"><label className="row" style={{ marginRight: 12, gap: 7 }}><input type="checkbox" checked={draft.enabled} onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })} /> {t('common.enabled')}</label>
          <label className="row" style={{ marginRight: 'auto', gap: 7 }}><input type="checkbox" checked={draft.stop} onChange={(event) => setDraft({ ...draft, stop: event.target.checked })} /> {t('rules.stop')}</label>
          <Button onClick={() => setEditing(undefined)}>{t('common.cancel')}</Button><Button variant="primary" disabled={problem !== null || patch.isPending} onClick={save}>{patch.isPending ? t('common.saving') : t('rules.save')}</Button></div>
        {problem && <p className="bad-text">{problem}</p>}
      </Card></>}
      {patch.isError && <p className="bad-text">{patch.error.message}</p>}{reload.isError && <p className="bad-text">{reload.error.message}</p>}

      <SectionHeader title={t('rules.preview')} description={t('rules.previewHint')} aside={<div style={{ width: 230, position: 'relative' }}><Search size={14} style={{ position: 'absolute', left: 10, top: 10 }} className="muted" /><input className="search-input" style={{ paddingLeft: 30 }} placeholder={t('rules.filterWindows')} value={filter} onChange={(event) => setFilter(event.target.value)} /></div>} />
      <Card className="padded stack">
        <div className="field"><label htmlFor="preview-window">{t('rules.openWindow')}</label><select id="preview-window" value={previewWindow ?? ''} onChange={(event) => { const id = event.target.value === '' ? null : Number(event.target.value); setPreviewWindow(id); apply.reset(); if (id !== null) preview.mutate(id) }}>
          <option value="">{t('rules.chooseWindow')}</option>
          {previewable.map((window) => <option key={window.cg_window_id!} value={window.cg_window_id!}>{window.application_id ?? `pid ${window.pid}`} — {window.title || t('common.untitled')} · CG {window.cg_window_id}</option>)}</select></div>
        {preview.isPending && <Loading />}{preview.isError && <p className="bad-text">{preview.error.message}</p>}
      </Card>
      {preview.data && previewWindow !== null && <RulePreview test={preview.data} applying={apply.isPending} applyError={apply.isError ? apply.error.message : null} applied={apply.isSuccess ? apply.data.message : null} onApply={() => apply.mutate(previewWindow)} />}

      {diagnostics.data && <><SectionHeader title={t('nav.diagnostics')} description={t('rules.diagHint')} /><Card className="padded stack">
        <div className="row-sub">{t('rules.diagLine', { rules: diagnostics.data.rules, tracked: diagnostics.data.tracked_windows, overrides: diagnostics.data.manual_overrides, evaluations: diagnostics.data.metrics.evaluations ?? 0, avg: Math.round((diagnostics.data.metrics.evaluation_total_us ?? 0) / Math.max(1, diagnostics.data.metrics.evaluations ?? 0)), moves: diagnostics.data.metrics.moves_applied ?? 0, modes: diagnostics.data.metrics.modes_applied ?? 0, errors: diagnostics.data.metrics.errors ?? 0 })}</div>
        {diagnostics.data.recent.slice(-6).reverse().map((entry, index) => <div key={index} className="row row-pad"><span className="mono">{entry.rule}</span><span className="row-sub">{entry.action}: {entry.detail} · pid {entry.pid}{entry.cg_window_id ? ` · CG ${entry.cg_window_id}` : ''}</span></div>)}
      </Card></>}
      <div style={{ marginTop: 18 }}><Notice>{t('rules.notice')} <span className="mono">tylora rules apply --window</span>.</Notice></div>
    </>}
  </>
}
