/* Copyright 2026 BoxLite AI · SPDX-License-Identifier: AGPL-3.0 */
import { useState } from 'react'
import { Navigate } from 'react-router-dom'
import { AsciiButton } from '@/components/ascii'
import { Check, ChevronRight, Code2, Terminal, Globe, Bot, Container, SparklesIcon } from '@/components/ui/icon'
import { useApi } from '@/hooks/useApi'
import { useQuery } from '@tanstack/react-query'
import { useAuth } from 'react-oidc-context'
import { useSelectedOrganization } from '@/hooks/useSelectedOrganization'
import { useWelcomeOnboarding } from '@/hooks/useWelcomeOnboarding'
import { RoutePath } from '@/enums/RoutePath'
import { FIRST_RUN_USE_CASES, type FirstRunWelcome, type FirstRunUseCase } from '@/lib/first-run'
import { getLocalStorageItem } from '@/lib/local-storage'
import FirstBoxAtmosphere from './FirstBoxAtmosphere'

const USE_CASE_ICONS = {
  'coding-agent': Code2,
  'code-execution': Terminal,
  'browser-agent': Globe,
  'long-running-agent': Bot,
  'application-deployments': Container,
  'reinforcement-learning': SparklesIcon,
  exploring: Globe,
}

export default function FirstBoxWelcome() {
  const { selectedOrganization } = useSelectedOrganization()
  const { user } = useAuth()
  const api = useApi()
  const orgId = selectedOrganization?.id
  const profile = useQuery({
    queryKey: ['organization-onboarding', orgId],
    enabled: !!orgId,
    queryFn: () => api.getOrganizationOnboarding(orgId!),
  })
  if (profile.isError)
    return (
      <p role="alert" className="p-8">
        Could not load your setup. <button onClick={() => void profile.refetch()}>Try again</button>
      </p>
    )
  if (!selectedOrganization || profile.isPending) return <p className="p-8">Loading your workspace…</p>
  let organizationSaved = false
  let completedAt: number | null = null
  try {
    const progress = JSON.parse(getLocalStorageItem(`boxlite:welcome:${user?.profile.sub}:${orgId}`) ?? 'null')
    if (typeof progress?.completedAt === 'number' && Number.isFinite(progress.completedAt) && progress.completedAt > 0)
      completedAt = progress.completedAt
    organizationSaved = typeof progress?.organizationName === 'string' && !!progress.organizationName.trim()
  } catch {
    /* Invalid browser progress must not block first-time setup. */
  }
  const initial: FirstRunWelcome = {
    organizationName: profile.data || organizationSaved ? selectedOrganization.name : '',
    useCase: profile.data?.useCase ?? null,
    note: profile.data?.note ?? '',
    creditGrantedAt: null,
    completedAt,
  }
  return <WelcomeForm key={`${user?.profile.sub}:${orgId}`} initial={initial} />
}

function WelcomeForm({ initial }: { initial: FirstRunWelcome }) {
  const { welcome, action, billingEnabled } = useWelcomeOnboarding(initial)
  const [step, setStep] = useState<'organization' | 'intent'>(welcome.organizationName ? 'intent' : 'organization')
  const [name, setName] = useState(welcome.organizationName)
  const [useCase, setUseCase] = useState<FirstRunUseCase | null>(welcome.useCase)
  const [note, setNote] = useState(welcome.note)
  const [noteOpen, setNoteOpen] = useState(Boolean(welcome.note))
  const error = action.isError
    ? action.error instanceof Error
      ? action.error.message
      : 'Could not save. Please try again.'
    : null

  if (welcome.completedAt != null || welcome.creditGrantedAt !== null)
    return <Navigate to={RoutePath.QUICKSTART} replace />

  return (
    <FirstBoxAtmosphere>
      <main className="mx-auto w-full max-w-[620px] px-6 pb-12 pt-10 font-display sm:pt-16">
        <div
          className="mb-8 flex items-center gap-3 font-mono text-label text-muted-foreground"
          aria-label="Onboarding progress"
        >
          <span className={step === 'organization' ? 'text-foreground' : ''}>1 Organization</span>
          <span aria-hidden="true">/</span>
          <span className={step === 'intent' ? 'text-foreground' : ''}>2 Use case</span>
        </div>
        {step === 'organization' && (
          <form
            onSubmit={(event) => {
              event.preventDefault()
              action.mutate({ type: 'organization', name }, { onSuccess: () => setStep('intent') })
            }}
          >
            <h1 className="font-display text-page font-semibold">Create your organization</h1>
            <p className="mt-3 text-sm leading-6 text-muted-foreground">
              A home for your Boxes, team, and billing. Working solo? Use your own name.
            </p>
            <label htmlFor="welcome-organization" className="mb-2 mt-8 block text-body">
              Organization name
            </label>
            <input
              id="welcome-organization"
              autoFocus
              value={name}
              onChange={(event) => setName(event.target.value)}
              maxLength={60}
              required
              placeholder="e.g. Acme or Alex’s workspace"
              autoComplete="organization"
              className="h-12 w-full border border-border bg-card px-4 text-body outline-none focus:border-foreground/50"
            />
            <p className="mt-2 font-mono text-xs text-muted-foreground">You can change this later.</p>
            <AsciiButton
              type="submit"
              variant="primary"
              className="mt-8 h-11 w-full font-display"
              disabled={!name.trim() || action.isPending}
            >
              {action.isPending ? 'Saving…' : 'Continue'}
            </AsciiButton>
          </form>
        )}
        {step === 'intent' && (
          <form
            onSubmit={(event) => {
              event.preventDefault()
              if (useCase) action.mutate({ type: 'intent', useCase, note })
            }}
          >
            <h1 className="font-display text-page font-semibold">What are you building?</h1>
            <p className="mt-3 text-sm leading-6 text-muted-foreground">What would you like to use BoxLite for?</p>
            <fieldset className="mt-7" disabled={action.isPending}>
              <legend className="sr-only">Use case</legend>
              <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
                {FIRST_RUN_USE_CASES.filter((option) => option.id !== 'exploring').map((option) => {
                  const Icon = USE_CASE_ICONS[option.id]
                  return (
                    <label key={option.id} className="group relative cursor-pointer">
                      <input
                        type="radio"
                        name="boxlite-use-case"
                        value={option.id}
                        checked={useCase === option.id}
                        onChange={() => setUseCase(option.id)}
                        className="peer sr-only"
                      />
                      <span className="flex min-h-[56px] items-center gap-3 rounded-none bg-foreground/[0.025] px-3.5 py-3 text-sm text-foreground/80 transition-colors duration-200 group-hover:bg-foreground/[0.055] peer-checked:bg-brand/[0.10] peer-checked:text-foreground peer-focus-visible:outline peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-ring peer-disabled:cursor-wait">
                        <Icon
                          aria-hidden
                          className={`size-4 shrink-0 transition-colors ${useCase === option.id ? 'text-brand' : 'text-muted-foreground group-hover:text-foreground'}`}
                        />
                        <span className="flex-1">{option.label}</span>
                        <Check
                          aria-hidden="true"
                          className={`size-3.5 shrink-0 text-brand transition-opacity ${useCase === option.id ? 'opacity-100' : 'opacity-0'}`}
                        />
                      </span>
                    </label>
                  )
                })}
              </div>
              <label className="mt-3 inline-flex cursor-pointer items-center gap-2 py-2">
                <input
                  type="radio"
                  name="boxlite-use-case"
                  value="exploring"
                  checked={useCase === 'exploring'}
                  onChange={() => setUseCase('exploring')}
                  className="peer sr-only"
                />
                <span className="text-sm text-muted-foreground underline-offset-4 hover:text-foreground peer-checked:text-foreground peer-focus-visible:underline">
                  Just exploring
                </span>
                <Check
                  aria-hidden="true"
                  className={`size-3.5 ${useCase === 'exploring' ? 'opacity-100' : 'opacity-0'}`}
                />
              </label>
            </fieldset>
            <details open={noteOpen} onToggle={(event) => setNoteOpen(event.currentTarget.open)} className="group mt-5">
              <summary className="flex w-fit cursor-pointer list-none items-center gap-1.5 text-sm text-muted-foreground outline-offset-4 hover:text-foreground [&::-webkit-details-marker]:hidden">
                <ChevronRight aria-hidden="true" className="size-3.5 transition-transform group-open:rotate-90" />
                Add context <span className="ml-1 text-xs">(optional)</span>
              </summary>
              <label htmlFor="welcome-note" className="sr-only">
                Additional context
              </label>
              <textarea
                id="welcome-note"
                value={note}
                onChange={(event) => setNote(event.target.value)}
                maxLength={600}
                rows={3}
                placeholder="e.g. Give my coding agent a safe place to run tests"
                className="mt-3 w-full resize-y border border-border bg-card px-4 py-3 text-sm outline-none focus:border-foreground/50"
              />
            </details>
            <div className="mt-8 flex items-center justify-between">
              <button
                type="button"
                disabled={action.isPending}
                className="py-2 text-sm text-muted-foreground underline-offset-4 hover:text-foreground focus-visible:underline"
                onClick={() => {
                  action.reset()
                  setStep('organization')
                }}
              >
                Back
              </button>
              <AsciiButton
                type="submit"
                variant="primary"
                className="h-10 min-w-[120px] border border-transparent px-6 font-display text-sm focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:border-border disabled:bg-card disabled:text-muted-foreground disabled:opacity-100"
                disabled={!useCase || action.isPending}
              >
                {action.isPending
                  ? billingEnabled
                    ? 'Waiting for credits…'
                    : 'Saving…'
                  : action.isError
                    ? 'Try again'
                    : 'Start quickstart'}
              </AsciiButton>
            </div>
            {billingEnabled && (
              <p className="mt-3 text-right text-xs text-muted-foreground">
                We’ll check your signup credits before opening Quickstart.
              </p>
            )}
            {billingEnabled && action.isError && useCase && (
              <button
                type="button"
                className="mt-3 text-sm underline"
                onClick={() => action.mutate({ type: 'intent', useCase, note, skipCredit: true })}
              >
                Continue without credits
              </button>
            )}
          </form>
        )}
        {error && (
          <p role="alert" className="mt-4 text-xs text-destructive">
            {error}
          </p>
        )}
      </main>
    </FirstBoxAtmosphere>
  )
}
