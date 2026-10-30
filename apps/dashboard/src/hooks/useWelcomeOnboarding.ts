/* Copyright 2026 BoxLite AI · SPDX-License-Identifier: AGPL-3.0 */
import { useEffect, useRef, useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useAuth } from 'react-oidc-context'
import { useApi } from './useApi'
import { useOrganizations } from './useOrganizations'
import { useSelectedOrganization } from './useSelectedOrganization'
import { useConfig } from './useConfig'
import { queryKeys } from './queries/queryKeys'
import { type FirstRunWelcome, type FirstRunUseCase } from '@/lib/first-run'
import { LocalStorageKey } from '@/enums/LocalStorageKey'
import { setLocalStorageItem } from '@/lib/local-storage'
import { waitForSignupCredit } from '@/lib/signup-credit'

export function useWelcomeOnboarding(initial: FirstRunWelcome) {
  const api = useApi()
  const { selectedOrganization } = useSelectedOrganization()
  const { refreshOrganizations } = useOrganizations()
  const { user } = useAuth()
  const config = useConfig()
  const client = useQueryClient()
  const [welcome, setWelcome] = useState(initial)
  const controller = useRef<AbortController | null>(null)
  const organizationId = selectedOrganization?.id
  useEffect(() => () => controller.current?.abort(), [organizationId])
  const action = useMutation({
    mutationFn: async (
      payload:
        | { type: 'organization'; name: string }
        | { type: 'intent'; useCase: FirstRunUseCase; note: string; skipCredit?: boolean },
    ) => {
      if (!organizationId) throw new Error('Select an organization and try again.')
      controller.current?.abort()
      const active = new AbortController()
      controller.current = active
      const save = (value: FirstRunWelcome) => {
        active.signal.throwIfAborted()
        setLocalStorageItem(`boxlite:welcome:${user?.profile.sub}:${organizationId}`, JSON.stringify(value))
        setWelcome(value)
      }
      if (payload.type === 'organization') {
        const name = payload.name.trim()
        if (!name || name.length > 60) throw new Error('Enter an organization name of 1–60 characters.')
        await api.organizationsApi.updateOrganizationName(organizationId, { name })
        active.signal.throwIfAborted()
        await refreshOrganizations()
        save({ ...welcome, organizationName: name })
        return
      }
      await api.saveOrganizationOnboarding(organizationId, { useCase: payload.useCase, note: payload.note.trim() })
      active.signal.throwIfAborted()
      save({ ...welcome, useCase: payload.useCase, note: payload.note.trim() })
      if (!config.billingApiUrl || payload.skipCredit) {
        save({ ...welcome, useCase: payload.useCase, note: payload.note.trim(), completedAt: Date.now() })
        setLocalStorageItem(`${LocalStorageKey.SkipOnboardingPrefix}${user?.profile.sub}`, 'true')
        return
      }
      await waitForSignupCredit(
        () => api.billingApi.getSignupCreditDelivery(organizationId, active.signal),
        active.signal,
      )
      active.signal.throwIfAborted()
      await Promise.all([
        client.invalidateQueries({ queryKey: queryKeys.organization.wallet(organizationId) }),
        client.invalidateQueries({ queryKey: queryKeys.billing.transactions(organizationId) }),
      ])
      save({
        ...welcome,
        useCase: payload.useCase,
        note: payload.note.trim(),
        creditGrantedAt: Date.now(),
        completedAt: Date.now(),
      })
      setLocalStorageItem(`${LocalStorageKey.SkipOnboardingPrefix}${user?.profile.sub}`, 'true')
    },
  })
  return { welcome, action, billingEnabled: Boolean(config.billingApiUrl) }
}
