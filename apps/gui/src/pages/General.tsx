import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ExternalLink, RotateCcw } from 'lucide-react'
import { useUI, type Theme } from '../app/store'
import { useDaemon } from '../api/hooks'
import { Badge, Button, Card, ConfirmButton, ErrorState, Loading, Notice, SectionHeader } from '../shared/ui'

interface Preferences { version: 1; launch_at_login: boolean; start_daemon_automatically: boolean; open_settings_at_launch: boolean; show_menu_bar_icon: boolean }
const defaults: Preferences = { version: 1, launch_at_login: false, start_daemon_automatically: false, open_settings_at_launch: true, show_menu_bar_icon: true }

export function General() {
  const theme = useUI((state) => state.theme)
  const daemon = useDaemon('status')
  const setTheme = useUI((state) => state.setTheme)
  const client = useQueryClient()
  const prefs = useQuery({ queryKey: ['gui-prefs'], queryFn: () => invoke<Preferences>('gui_preferences') })
  const path = useQuery({ queryKey: ['config-path'], queryFn: () => invoke<string>('config_location'), staleTime: Infinity })
  const save = useMutation({ mutationFn: (next: Preferences) => invoke<Preferences>('save_gui_preferences', { prefs: next }), onSuccess: (next) => client.setQueryData(['gui-prefs'], next) })
  const [openError, setOpenError] = useState('')
  const update = (key: keyof Omit<Preferences, 'version'>, checked: boolean) => { if (prefs.data) save.mutate({ ...prefs.data, [key]: checked }) }
  const open = async () => { try { await invoke('open_config_file'); setOpenError('') } catch (error) { setOpenError(String(error)) } }
  return <><div className="page-heading"><div><span className="eyebrow">Preferences</span><h1>General Settings</h1><p>Settings for the companion app and its independent daemon.</p></div></div>
    {prefs.isPending ? <Loading /> : prefs.isError ? <ErrorState message={String(prefs.error)} retry={() => void prefs.refetch()} /> : <>
      <SectionHeader title="Appearance" description="Theme is local to this settings app." /><Card className="padded"><div className="row"><div><strong>Theme</strong><span className="row-sub">Follow macOS or choose a fixed appearance.</span></div><select className="search-input" style={{ width: 130 }} value={theme} onChange={(event) => setTheme(event.target.value as Theme)}><option value="system">System</option><option value="light">Light</option><option value="dark">Dark</option></select></div></Card>
      <SectionHeader title="Startup & menu bar" description="Nothing starts automatically unless you enable it." /><Card className="padded">
        {([
          ['launch_at_login', 'Launch settings at login', 'Installs a per-user LaunchAgent for the GUI.'],
          ['start_daemon_automatically', 'Start daemon with settings', 'Only when the GUI launches and the daemon is stopped.'],
          ['open_settings_at_launch', 'Open settings at launch', 'Otherwise stay in the menu bar.'],
          ['show_menu_bar_icon', 'Show menu bar icon', 'The settings app keeps running after its window closes.'],
        ] as const).map(([key, title, detail]) => <div className="row row-pad" key={key}><div><span className="row-title">{title}</span><span className="row-sub">{detail}</span></div><input aria-label={title} type="checkbox" checked={prefs.data[key]} disabled={save.isPending} onChange={(event) => update(key, event.target.checked)} /></div>)}
      </Card>
      {save.isError && <p className="bad-text">{String(save.error)}</p>}{save.isSuccess && <p className="good-text">Preference saved.</p>}
      <SectionHeader title="Configuration" description="TOML remains the source of truth for daemon behavior." /><Card className="padded"><div className="row row-pad"><div><strong>Config location</strong><span className="row-sub mono">{path.data ?? 'Loading…'}</span></div><Button onClick={() => void open()}><ExternalLink size={13} /> Open file</Button></div><div className="row row-pad"><div><strong>Log level</strong><span className="row-sub">The daemon currently reads RUST_LOG only at startup.</span></div><Badge tone="warn">Requires manual restart</Badge></div></Card>
      {openError && <p className="bad-text">{openError}</p>}
      <SectionHeader title="Permissions" description="Accessibility permission is checked in the daemon process." /><Card className="padded row"><div><strong>Accessibility</strong><span className="row-sub">{daemon.isSuccess ? (daemon.data.accessibility_trusted ? 'Granted to the running daemon.' : 'Permission is unavailable to the running daemon.') : 'Start the daemon to check its permission.'}</span></div><Button onClick={() => void daemon.refetch()}>Check permissions</Button></Card>
      <SectionHeader title="Reset" description="Reset only GUI preferences; the daemon TOML remains intact." /><Card className="padded row"><div><strong>Reset settings app</strong><span className="row-sub">Disable login launch and restore default window and menu bar behavior.</span></div><ConfirmButton prompt="Reset GUI preferences? Your Tylora TOML will be preserved." disabled={save.isPending} onConfirm={() => { setTheme('system'); save.mutate(defaults) }}><RotateCcw size={13} /> Reset</ConfirmButton></Card>
      <div style={{ marginTop: 17 }}><Notice>Closing or quitting Settings never stops Tylora. Use the explicit Stop Tylora action when you want the daemon to stop.</Notice></div>
    </>}
  </>
}
