import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { controlDaemon, patchConfig, queryDaemon, sendCommand, simpleAction } from './bridge'
import type { ConfigPatch, QueryKind, QueryMap, StateCommand } from './types'

export function useDaemon<K extends QueryKind>(kind: K, enabled = true, poll = false) {
  return useQuery<QueryMap[K]>({
    queryKey: ['daemon', kind],
    queryFn: ({ signal }) => queryDaemon(kind, signal),
    enabled,
    staleTime: 1500,
    refetchInterval: kind === 'status' && poll ? 5000 : false,
    refetchIntervalInBackground: false,
    retry: 1,
  })
}

export function useConfigPatch() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: ({ revision, patch }: { revision: string; patch: ConfigPatch }) => patchConfig(revision, patch),
    onSuccess: async (snapshot) => {
      client.setQueryData(['daemon', 'config'], snapshot)
      await client.invalidateQueries({ queryKey: ['daemon'] })
    },
  })
}

export function useAction() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (action: { kind: 'command'; command: StateCommand } | { kind: 'simple'; action: 'reconcile' | 'recover_windows' } | { kind: 'control'; op: 'start' | 'stop' | 'restart' }) => {
      if (action.kind === 'command') return sendCommand(action.command)
      if (action.kind === 'simple') return simpleAction(action.action)
      return controlDaemon(action.op)
    },
    onSuccess: async () => { await client.invalidateQueries({ queryKey: ['daemon'] }) },
  })
}
