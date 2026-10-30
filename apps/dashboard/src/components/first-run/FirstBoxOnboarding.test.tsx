// @vitest-environment jsdom
import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { FirstBoxQuickstart } from './FirstBoxOnboarding'
const state = vi.hoisted(() => ({ navigate: vi.fn() }))
vi.mock('react-router-dom', async () => ({
  ...(await vi.importActual('react-router-dom')),
  useNavigate: () => state.navigate,
}))
vi.mock('./ConsoleTemplatePicker', () => ({ ConsoleTemplatePicker: () => <p>Image picker</p> }))
vi.mock('@/components/OnboardingGuideDialog', () => ({
  OnboardingGuideDialog: ({ preview, onOpenChange, onProgressChange }: any) => (
    <div data-testid="guide" data-recipe={preview.recipe.id}>
      <button onClick={() => preview.onConsole()}>Choose image</button>
      <button onClick={() => onOpenChange(false)}>Close guide</button>
      <button onClick={() => onProgressChange({})}>Progress</button>
    </div>
  ),
}))
let root: Root, host: HTMLDivElement
beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  vi.clearAllMocks()
})
afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})
async function mount(url = '/') {
  await act(async () =>
    root.render(
      <MemoryRouter initialEntries={[url]}>
        <FirstBoxQuickstart />
      </MemoryRouter>,
    ),
  )
}
async function click(label: string) {
  await act(async () =>
    Array.from(host.querySelectorAll('button'))
      .find((e) => e.getAttribute('aria-label') === label || e.textContent === label)!
      .click(),
  )
}
it('switches between SDK, console and agent prompt without reopening the old modal', async () => {
  await mount()
  expect(host.querySelector('[aria-label="Deploy from SDK / CLI"]')!.getAttribute('aria-pressed')).toBe('true')
  await click('Deploy from console')
  expect(host.textContent).toContain('Image picker')
  expect(host.querySelector('[data-testid="guide"]')!.parentElement!.hidden).toBe(true)
  await click('Deploy from prompt')
  expect(host.querySelector('[data-testid="guide"]')!.parentElement!.hidden).toBe(false)
  await click('Progress')
  await click('Choose image')
  expect(host.textContent).toContain('Image picker')
  await click('Deploy from SDK / CLI')
  await click('Close guide')
  expect(state.navigate).toHaveBeenCalledWith('/dashboard/boxes')
})
it('opens the console directly from an image catalog deep link', async () => {
  await mount('/?method=console&template=python')
  expect(host.textContent).toContain('Image picker')
  expect(host.querySelector('[data-testid="guide"]')!.parentElement!.hidden).toBe(true)
})
