// @vitest-environment jsdom
import React, { act } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, it, vi } from 'vitest'
import { QuickstartCopyButton } from './QuickstartCopyButton'
it.each([true, false])('shows idle, copied and denied-clipboard feedback (quiet=%s)', async (quiet) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  const host = document.createElement('div'),
    root = createRoot(host),
    copy = vi.fn()
  try {
    for (const state of ['idle', 'copied', 'failed']) {
      await act(async () =>
        root.render(
          <QuickstartCopyButton quiet={quiet} copied={state === 'copied'} failed={state === 'failed'} onClick={copy} />,
        ),
      )
      expect(host.textContent).toBe(state === 'idle' ? 'Copy' : state === 'copied' ? '✓ Copied' : '✕ Blocked')
      await act(async () => host.querySelector('button')!.click())
    }
    expect(copy).toHaveBeenCalledTimes(3)
  } finally {
    await act(async () => root.unmount())
  }
})
