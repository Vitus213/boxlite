// @vitest-environment jsdom
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, it, vi } from 'vitest'
import { QuickstartRecipeHeader } from './QuickstartRecipeHeader'
import { quickstartRecipeFor } from '@/lib/quickstart/recipes'

it('shows local coding agents for an inline application and no duplicate heading for inline SDK setup', async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  const host = document.createElement('div')
  const root = createRoot(host)
  try {
    const preview = { inline: true, onRecipeChange: vi.fn(), onConsole: vi.fn(), creditCents: 0 }
    await act(async () =>
      root.render(
        <QuickstartRecipeHeader
          compact={false}
          preview={{ ...preview, recipe: quickstartRecipeFor('application-deployments') }}
        />,
      ),
    )
    expect(host.textContent).toContain('Pi')
    expect(host.querySelectorAll('svg, img')).toHaveLength(8)
    await act(async () =>
      root.render(
        <QuickstartRecipeHeader
          compact={false}
          preview={{ ...preview, recipe: quickstartRecipeFor('code-execution') }}
        />,
      ),
    )
    expect(host.innerHTML).toBe('')
  } finally {
    await act(async () => root.unmount())
  }
})
