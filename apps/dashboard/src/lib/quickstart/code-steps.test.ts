import { describe, expect, it } from 'vitest'
import {
  getOnboardingInterfaces,
  renderOnboardingCodeExample,
  renderOnboardingCodeSteps,
} from '../onboarding-code-examples'

describe('quickstart creation and execution sections', () => {
  const options = {
    restApiUrl: 'https://api.example.test',
    command: { executable: 'echo', args: ['Hello from BoxLite'] },
  }
  for (const item of getOnboardingInterfaces()) {
    it(`keeps ${item.id} setup and execution in one complete example`, () => {
      const parts = renderOnboardingCodeSteps(item.id, options)
      expect(parts.creation).not.toContain('Hello from BoxLite')
      expect(parts.execution).toContain('Hello from BoxLite')
      expect([parts.creation, parts.execution, parts.preview].join('\n\n')).toBe(parts.example)
      expect(parts.example).toBe(renderOnboardingCodeExample(item.id, options))
      if (item.id === 'cli') {
        expect(parts.creation).toContain('boxlite create --name "$box_name"')
        expect(parts.creation).toContain('boxlite start "$box_name"')
        expect(parts.execution).toContain('boxlite exec "$box_name" --')
      }
    })
  }
})

for (const item of getOnboardingInterfaces()) {
  it(`keeps ${item.id} running after creating its preview`, () => {
    const parts = renderOnboardingCodeSteps(item.id, { restApiUrl: 'https://dev.boxlite.ai/api' })
    expect(parts).toHaveProperty('preview')
    expect(parts.example).not.toMatch(/rt\.remove|ForceRemove|boxlite_remove|boxlite rm|DELETE|Press Enter/)
    expect(parts.preview).toContain('3000')
    expect(parts.preview).toContain('Hello from BoxLite')
    expect(parts.preview).toContain('Preview URL:')
    expect(parts.execution).not.toMatch(/rt\.remove|ForceRemove|boxlite_remove|boxlite rm|DELETE/)
    expect([parts.creation, parts.execution, parts.preview].join('\n\n')).toBe(parts.example)
  })
}
