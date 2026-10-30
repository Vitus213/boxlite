// @vitest-environment jsdom
import React, { act } from 'react'
import { createRoot } from 'react-dom/client'
import { MemoryRouter, Routes, Route } from 'react-router-dom'
import { expect, it } from 'vitest'
import { OnboardingDialogHost } from './OnboardingDialogHost'
import { ONBOARDING_OPEN_EVENT } from '@/lib/onboarding-progress'
it('routes the shared guide event to Quickstart and releases its listener on unmount', async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  const host = document.createElement('div'),
    root = createRoot(host)
  await act(async () =>
    root.render(
      <MemoryRouter initialEntries={['/dashboard/boxes']}>
        <OnboardingDialogHost />
        <Routes>
          <Route path="/dashboard/boxes" element={<p>Boxes</p>} />
          <Route path="/dashboard/quickstart" element={<p>Dedicated Quickstart</p>} />
        </Routes>
      </MemoryRouter>,
    ),
  )
  expect(host.textContent).toBe('Boxes')
  const event = new Event(ONBOARDING_OPEN_EVENT, { cancelable: true })
  await act(async () => window.dispatchEvent(event))
  expect(event.defaultPrevented).toBe(true)
  expect(host.textContent).toBe('Dedicated Quickstart')
  await act(async () => root.unmount())
  expect(() => window.dispatchEvent(new Event(ONBOARDING_OPEN_EVENT))).not.toThrow()
})
