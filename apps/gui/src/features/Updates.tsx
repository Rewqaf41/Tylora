import { useEffect, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { listen } from '@tauri-apps/api/event'
import { ArrowUpCircle, CircleCheck, ExternalLink, LoaderCircle, X } from 'lucide-react'
import { checkUpdate, installUpdate, openReleaseNotes, saveGuiPreferences, type GuiPreferences } from '../api/bridge'
import { useT } from '../i18n'
import { Button } from '../shared/ui'

const SIX_HOURS = 6 * 60 * 60 * 1000

/** Shared query: the banner and General Settings read the same result. */
export function useUpdateCheck(enabled: boolean) {
  return useQuery({ queryKey: ['update-check'], queryFn: checkUpdate, enabled, staleTime: SIX_HOURS, refetchInterval: enabled ? SIX_HOURS : false, retry: false })
}

export function useInstallUpdate() {
  const [progress, setProgress] = useState<{ downloaded: number; total: number | null } | null>(null)
  const install = useMutation({ mutationFn: installUpdate, onMutate: () => setProgress({ downloaded: 0, total: null }), onError: () => setProgress(null) })
  useEffect(() => {
    if (!install.isPending) return
    const stop = listen<{ downloaded: number; total: number | null }>('update://progress', (event) => setProgress(event.payload))
    return () => { void stop.then((unlisten) => unlisten()) }
  }, [install.isPending])
  const percent = progress?.total ? Math.min(100, Math.round(progress.downloaded / progress.total * 100)) : null
  return { install, percent }
}

/**
 * Top-of-window notices: a newer release is available (with Install and
 * restart), or the app was just updated (shown once per version).
 */
export function UpdateBanner({ prefs }: { prefs: GuiPreferences }) {
  const t = useT()
  const client = useQueryClient()
  const check = useUpdateCheck(prefs.check_for_updates && prefs.onboarding_completed)
  const { install, percent } = useInstallUpdate()
  const [hidden, setHidden] = useState<string | null>(null)
  const seen = useMutation({
    mutationFn: () => saveGuiPreferences({ ...prefs, last_seen_version: __APP_VERSION__ }),
    onSuccess: (next) => client.setQueryData(['gui-prefs'], next),
  })
  // A fresh install (or a build before this field) records its version silently.
  const firstSeen = prefs.last_seen_version === null
  useEffect(() => { if (firstSeen && !seen.isPending && !seen.isSuccess) seen.mutate() }, [firstSeen, seen])

  const available = check.data?.available
  if (available && hidden !== available.version) {
    return <div className="update-banner" role="status">
      <ArrowUpCircle size={16} />
      <span className="update-text"><strong>{t('updates.available', { version: available.version })}</strong>
        <span>{install.isPending ? (percent === null ? t('updates.installing') : t('updates.downloading', { percent })) : install.isError ? install.error.message : t('updates.availableHint', { current: __APP_VERSION__ })}</span></span>
      <Button size="sm" variant="ghost" onClick={() => void openReleaseNotes(available.version)}><ExternalLink size={13} /> {t('updates.notes')}</Button>
      <Button size="sm" variant="primary" disabled={install.isPending} onClick={() => install.mutate()}>{install.isPending ? <LoaderCircle size={13} className="spin" /> : <ArrowUpCircle size={13} />} {t('updates.install')}</Button>
      {!install.isPending && <button className="icon-button" aria-label={t('updates.later')} onClick={() => setHidden(available.version)}><X size={14} /></button>}
    </div>
  }
  if (!firstSeen && prefs.last_seen_version !== __APP_VERSION__) {
    return <div className="update-banner done" role="status">
      <CircleCheck size={16} />
      <span className="update-text"><strong>{t('updates.updated', { version: __APP_VERSION__ })}</strong><span>{t('updates.updatedHint')}</span></span>
      <Button size="sm" variant="ghost" onClick={() => void openReleaseNotes(__APP_VERSION__)}><ExternalLink size={13} /> {t('updates.whatsNew')}</Button>
      <button className="icon-button" aria-label={t('updates.dismiss')} disabled={seen.isPending} onClick={() => seen.mutate()}><X size={14} /></button>
    </div>
  }
  return null
}
