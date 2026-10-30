// @vitest-environment jsdom
import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { usePromptDraft } from './usePromptDraft'
const api = vi.hoisted(() => ({ getPromptDraft: vi.fn(), savePromptDraft: vi.fn() }))
vi.mock('./useApi', () => ({ useApi: () => api }))
let root: Root
let client: QueryClient
let current: ReturnType<typeof usePromptDraft>
function Harness({ org }: { org: string }) {
  current = usePromptDraft(org, 'user', true)
  return null
}
async function render(org = 'org-a') {
  await act(async () => {
    root.render(
      <QueryClientProvider client={client}>
        <Harness key={org} org={org} />
      </QueryClientProvider>,
    )
  })
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10))
  })
}
beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  vi.resetAllMocks()
  api.getPromptDraft.mockResolvedValue({ text: 'saved request', revision: 2 })
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  root = createRoot(document.createElement('div'))
})
afterEach(async () => {
  await act(async () => root.unmount())
  client.clear()
})
it('restores a saved draft and uses its revision when flushing', async () => {
  await render()
  expect(current.text).toBe('saved request')
  api.savePromptDraft.mockResolvedValue({ text: 'new request', revision: 3 })
  await act(async () => current.setText('new request'))
  await act(async () => current.flush())
  expect(api.savePromptDraft).toHaveBeenCalledWith('org-a', 'new request', 2)
})
it('serializes concurrent flushes without duplicate revision writes', async () => {
  await render()
  api.savePromptDraft.mockImplementation(async (_org, text, revision) => ({ text, revision: revision + 1 }))
  await act(async () => {
    await Promise.all([current.flush('one'), current.flush('two')])
  })
  expect(api.savePromptDraft.mock.calls).toEqual([
    ['org-a', 'one', 2],
    ['org-a', 'two', 3],
  ])
})
it('keeps typed text after a failed save and retries the same revision', async () => {
  await render()
  api.savePromptDraft
    .mockRejectedValueOnce(new Error('offline'))
    .mockResolvedValueOnce({ text: 'keep me', revision: 3 })
  await act(async () => current.setText('keep me'))
  await act(async () => {
    await expect(current.flush()).rejects.toThrow('offline')
  })
  expect(current.text).toBe('keep me')
  expect(current.error).toContain('Could not save')
  await act(async () => current.retry())
  expect(api.savePromptDraft).toHaveBeenLastCalledWith('org-a', 'keep me', 2)
})
it('loads another organization without carrying the previous text across', async () => {
  await render()
  await act(async () => current.setText('private to A'))
  api.getPromptDraft.mockResolvedValue({ text: 'B request', revision: 1 })
  await render('org-b')
  expect(current.text).toBe('B request')
  expect(api.savePromptDraft).not.toHaveBeenCalled()
})

it('automatically saves after typing pauses and updates the recovery cache', async () => {
  await render()
  api.savePromptDraft.mockResolvedValue({ text: 'autosaved', revision: 3 })
  await act(async () => current.setText('autosaved'))
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 1100))
  })
  expect(api.savePromptDraft).toHaveBeenCalledWith('org-a', 'autosaved', 2)
  expect(client.getQueryData(['quickstart-prompt', 'org-a', 'user'])).toMatchObject({ text: 'autosaved', revision: 3 })
})

it('starts a missing draft at revision zero without writing until the text changes', async () => {
  api.getPromptDraft.mockResolvedValue(null)
  await render()
  expect(current.ready).toBe(true)
  expect(current.text).toBe('')
  await act(async () => current.flush())
  expect(api.savePromptDraft).not.toHaveBeenCalled()
  api.savePromptDraft.mockResolvedValue({ text: 'first', revision: 1 })
  await act(async () => current.flush('first'))
  expect(api.savePromptDraft).toHaveBeenCalledWith('org-a', 'first', 0)
})
it('blocks saving before load and recovers a failed read on retry', async () => {
  api.getPromptDraft.mockRejectedValueOnce(new Error('offline'))
  await render()
  expect(current.ready).toBe(false)
  expect(current.error).toContain('Could not load')
  await expect(current.flush('keep')).rejects.toThrow('has not loaded')
  api.getPromptDraft.mockResolvedValue({ text: 'recovered', revision: 4 })
  await act(async () => {
    await current.retry()
  })
  await render()
  expect(current.ready).toBe(true)
  expect(current.text).toBe('recovered')
  expect(api.savePromptDraft).not.toHaveBeenCalled()
})
