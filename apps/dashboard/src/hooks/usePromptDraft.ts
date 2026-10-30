/* Copyright 2026 BoxLite AI · SPDX-License-Identifier: AGPL-3.0 */
import { useEffect, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useApi } from './useApi'

/** Mount once per user/organization; serialize autosaves against the server revision. */
export function usePromptDraft(organizationId: string | undefined, userId: string | undefined, enabled: boolean) {
  const api = useApi()
  const client = useQueryClient()
  const [text, setText] = useState('')
  const [ready, setReady] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const saved = useRef({ text: '', revision: 0 })
  const pending = useRef<Promise<void>>(Promise.resolve())
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  const query = useQuery({
    queryKey: ['quickstart-prompt', organizationId, userId],
    enabled: enabled && !!organizationId && !!userId,
    queryFn: () => api.getPromptDraft(organizationId!),
    retry: false,
  })
  useEffect(() => {
    if (query.isSuccess && !query.isFetching && !ready) {
      saved.current = query.data ?? { text: '', revision: 0 }
      setText(saved.current.text)
      setReady(true)
    }
  }, [query.isSuccess, query.isFetching, query.data, ready])
  const flush = (value = text): Promise<void> => {
    const operation = pending.current
      .catch(() => {})
      .then(async () => {
        if (!organizationId || !ready) throw new Error('Your draft has not loaded yet.')
        if (saved.current.text === value) return
        if (mounted.current) {
          setSaving(true)
          setError(null)
        }
        try {
          saved.current = await api.savePromptDraft(organizationId, value, saved.current.revision)
          client.setQueryData(['quickstart-prompt', organizationId, userId], saved.current)
        } catch (cause) {
          if (mounted.current)
            setError('Could not save. Your text is still here. Retry, or reload if another tab changed the draft.')
          throw cause
        } finally {
          if (mounted.current) setSaving(false)
        }
      })
    pending.current = operation
    return operation
  }
  useEffect(() => {
    if (!enabled || !ready || text === saved.current.text) return
    const timer = setTimeout(() => {
      void flush(text).catch(() => {})
    }, 1000)
    return () => clearTimeout(timer)
  }, [text, ready, enabled])
  return {
    text,
    setText,
    ready,
    saving,
    dirty: text !== saved.current.text,
    error: error ?? (query.isError ? 'Could not load your draft.' : null),
    flush,
    retry: () => (ready ? flush() : query.refetch()),
  }
}
