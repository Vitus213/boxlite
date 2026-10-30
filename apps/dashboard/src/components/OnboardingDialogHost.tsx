/* Copyright 2026 BoxLite AI · SPDX-License-Identifier: AGPL-3.0 */
import { useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { RoutePath } from '@/enums/RoutePath'
import { ONBOARDING_OPEN_EVENT } from '@/lib/onboarding-progress'

/** Preserve the shared Guide event while routing to the dedicated Quickstart. */
export function OnboardingDialogHost() {
  const navigate = useNavigate()
  useEffect(() => {
    const open = (event: Event) => {
      event.preventDefault()
      navigate(RoutePath.QUICKSTART)
    }
    window.addEventListener(ONBOARDING_OPEN_EVENT, open)
    return () => window.removeEventListener(ONBOARDING_OPEN_EVENT, open)
  }, [navigate])
  return null
}
