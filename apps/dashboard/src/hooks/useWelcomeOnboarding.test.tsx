// @vitest-environment jsdom
import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { useWelcomeOnboarding } from './useWelcomeOnboarding'
const api = vi.hoisted(() => ({
  rename: vi.fn(),
  refresh: vi.fn(),
  delivery: vi.fn(),
  save: vi.fn(),
  billingApiUrl: 'https://billing.example.test',
}))
vi.mock('./useApi', () => ({
  useApi: () => ({
    saveOrganizationOnboarding: api.save,
    organizationsApi: { updateOrganizationName: api.rename },
    billingApi: { getSignupCreditDelivery: api.delivery },
  }),
}))
vi.mock('./useOrganizations', () => ({ useOrganizations: () => ({ refreshOrganizations: api.refresh }) }))
vi.mock('./useSelectedOrganization', () => ({
  useSelectedOrganization: () => ({ selectedOrganization: { id: 'org-1' } }),
}))
vi.mock('./useConfig', () => ({ useConfig: () => ({ billingApiUrl: api.billingApiUrl }) }))
vi.mock('react-oidc-context', () => ({ useAuth: () => ({ user: { profile: { sub: 'user-1' } } }) }))
let current: ReturnType<typeof useWelcomeOnboarding>
let root: Root
let client: QueryClient
function Harness() {
  current = useWelcomeOnboarding({ organizationName: '', useCase: null, note: '', creditGrantedAt: null })
  return null
}
beforeEach(async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  vi.resetAllMocks()
  api.billingApiUrl = 'https://billing.example.test'
  client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  root = createRoot(document.createElement('div'))
  await act(async () =>
    root.render(
      <QueryClientProvider client={client}>
        <Harness />
      </QueryClientProvider>,
    ),
  )
})
afterEach(async () => {
  await act(async () => root.unmount())
  client.clear()
  localStorage.clear()
})
it('renames the selected organization through the existing API before saving the step', async () => {
  await act(async () => {
    await current.action.mutateAsync({ type: 'organization', name: '  New name  ' })
  })
  expect(api.rename).toHaveBeenCalledWith('org-1', { name: 'New name' })
  expect(api.refresh).toHaveBeenCalledTimes(1)
  expect(current.welcome.organizationName).toBe('New name')
})
it('keeps the organization step incomplete if rename fails', async () => {
  api.rename.mockRejectedValue(new Error('permission denied'))
  await act(async () => {
    await expect(current.action.mutateAsync({ type: 'organization', name: 'New name' })).rejects.toThrow(
      'permission denied',
    )
  })
  expect(current.welcome.organizationName).toBe('')
  expect(api.refresh).not.toHaveBeenCalled()
})
it('marks completion and refreshes billing only after the server confirms delivery', async () => {
  let deliver!: (value: { delivered: boolean }) => void
  api.delivery.mockImplementation(
    () =>
      new Promise((resolve) => {
        deliver = resolve
      }),
  )
  const invalidate = vi.spyOn(client, 'invalidateQueries')
  let pending!: Promise<void>
  await act(async () => {
    pending = current.action.mutateAsync({ type: 'intent', useCase: 'coding-agent', note: 'tests' })
  })
  expect(current.welcome.creditGrantedAt).toBeNull()
  expect(invalidate).not.toHaveBeenCalled()
  await act(async () => {
    deliver({ delivered: true })
    await pending
  })
  expect(current.welcome.creditGrantedAt).not.toBeNull()
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ['organization', 'org-1', 'wallet'] })
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ['billing', 'org-1', 'transactions'] })
})
it('retains intent but not completion on a billing error', async () => {
  api.delivery.mockRejectedValue(new Error('billing unavailable'))
  await act(async () => {
    await expect(
      current.action.mutateAsync({ type: 'intent', useCase: 'coding-agent', note: 'saved answer' }),
    ).rejects.toThrow('billing unavailable')
  })
  expect(current.welcome.note).toBe('saved answer')
  expect(current.welcome.creditGrantedAt).toBeNull()
})

it('saves the selected scenario before polling and does not poll when saving fails', async () => {
  api.save.mockRejectedValue(new Error('save failed'))
  await act(async () => {
    await expect(
      current.action.mutateAsync({ type: 'intent', useCase: 'long-running-agent', note: 'always on' }),
    ).rejects.toThrow('save failed')
  })
  expect(api.save).toHaveBeenCalledWith('org-1', { useCase: 'long-running-agent', note: 'always on' })
  expect(api.delivery).not.toHaveBeenCalled()
})

it('finishes without inventing credit delivery when billing is disabled', async () => {
  api.billingApiUrl = ''
  await act(async () =>
    root.render(
      <QueryClientProvider client={client}>
        <Harness />
      </QueryClientProvider>,
    ),
  )
  await act(async () => current.action.mutateAsync({ type: 'intent', useCase: 'exploring', note: '' }))
  expect(api.delivery).not.toHaveBeenCalled()
  expect(current.welcome.creditGrantedAt).toBeNull()
  expect(current.welcome.completedAt).toEqual(expect.any(Number))
  expect(localStorage.getItem('SkipOnboarding_user-1')).toBe('true')
})
it('does not dismiss unfinished setup on the organization step', async () => {
  await act(async () => current.action.mutateAsync({ type: 'organization', name: 'New name' }))
  expect(localStorage.getItem('SkipOnboarding_user-1')).toBeNull()
})

it('allows an explicit exit after undelivered credits without recording a grant', async () => {
  api.delivery.mockRejectedValueOnce(new Error('not delivered'))
  await act(async () => {
    await expect(current.action.mutateAsync({ type: 'intent', useCase: 'exploring', note: '' })).rejects.toThrow(
      'not delivered',
    )
  })
  await act(async () =>
    current.action.mutateAsync({ type: 'intent', useCase: 'exploring', note: '', skipCredit: true }),
  )
  expect(api.delivery).toHaveBeenCalledTimes(1)
  expect(current.welcome.completedAt).toEqual(expect.any(Number))
  expect(current.welcome.creditGrantedAt).toBeNull()
})
