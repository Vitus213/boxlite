import { readFileSync } from 'node:fs'
import { afterEach, expect, it, vi } from 'vitest'

const options = { restApiUrl: 'https://api.example.test' }

afterEach(() => {
  vi.doUnmock('./interfaces.json')
  vi.doUnmock('./templates/python.py?raw')
  vi.resetModules()
})

it('rejects a registry entry that references an unavailable template', async () => {
  vi.resetModules()
  vi.doMock('./interfaces.json', async (importOriginal) => {
    const original = await importOriginal<typeof import('./interfaces.json')>()
    return {
      default: original.default.map((entry) =>
        entry.id === 'python' ? { ...entry, template: 'unavailable-template' } : entry,
      ),
    }
  })
  const { renderOnboardingCodeExample } = await import('../onboarding-code-examples')
  expect(() => renderOnboardingCodeExample('python', options)).toThrow(
    'Missing quickstart template: unavailable-template',
  )
})

it('rejects an incomplete template instead of copying a partial program', async () => {
  vi.resetModules()
  vi.doMock('./templates/python.py?raw', () => {
    const original = readFileSync(new URL('./templates/python.py', import.meta.url), 'utf8')
    return { default: original.replace('{{STEP_PREVIEW}}', '') }
  })
  const { renderOnboardingCodeSteps } = await import('../onboarding-code-examples')
  expect(() => renderOnboardingCodeSteps('python', options)).toThrow('Missing quickstart sections for python')
})
