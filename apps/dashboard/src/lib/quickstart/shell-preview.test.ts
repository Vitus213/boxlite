import { spawnSync } from 'node:child_process'
import { expect, it } from 'vitest'
import { renderOnboardingCodeSteps } from '../onboarding-code-examples'

it.each(['cli', 'rest'])('accepts a piped API key without terminal controls for %s', (language) => {
  const { creation } = renderOnboardingCodeSteps(language, { restApiUrl: 'https://api.example.test' })
  const commands = `curl() { printf '%s' '{"path_prefix":"org","box_id":"box1"}'; }
boxlite() { printf 'box1\\n'; }
`
  const result = spawnSync('bash', ['-c', commands + creation], {
    input: 'test-key\n',
    env: { ...process.env, BOXLITE_API_KEY: '' },
    encoding: 'utf8',
    timeout: 5000,
  })
  expect(result.status, result.stderr).toBe(0)
  expect(result.stderr).not.toContain('stty')
  expect(result.stdout).toContain('Created Box: box1')
})

it('rejects an empty CLI tunnel response instead of printing a usable preview', () => {
  const { preview } = renderOnboardingCodeSteps('cli', { restApiUrl: 'https://api.example.test' })
  const result = spawnSync('bash', ['-c', 'set -euo pipefail\nbox_name=box1\nboxlite() { :; }\n' + preview], {
    encoding: 'utf8',
    timeout: 5000,
  })
  expect(result.status).not.toBe(0)
  expect(result.stderr).toContain('Cloud did not return a preview URL')
  expect(result.stdout).not.toContain('Preview URL:')
})
