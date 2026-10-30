import { ConflictException, InternalServerErrorException, NotFoundException, ValidationPipe } from '@nestjs/common'
import { DataSource } from 'typeorm'
import { OnboardingService } from './onboarding.service'
import { CreatePromptHandoffDto, SaveOnboardingDto, SavePromptDraftDto } from '../dto/onboarding.dto'

// Database semantics (constraints, races and rollback) are covered by the migration integration suite.
describe('onboarding persistence', () => {
  const query = jest.fn()
  const service = new OnboardingService({ query } as unknown as DataSource)
  beforeEach(() => query.mockReset())

  it('returns null for an existing organization without onboarding', async () => {
    query.mockResolvedValue([{ onboarding: null }])
    expect(await service.getOnboarding('org')).toBeNull()
  })

  it('records the server identity and timestamp, only accepting profile fields', async () => {
    query.mockImplementation(async (_sql, args) => [[{ onboarding: JSON.parse(args[1]) }], 1])
    const saved = await service.saveOnboarding('org', 'actor', { useCase: 'long-running-agent', note: ' hi ' })
    expect(saved).toMatchObject({ useCase: 'long-running-agent', note: 'hi', submittedBy: 'actor' })
    expect(Number.isNaN(Date.parse(saved.submittedAt))).toBe(false)
    expect(query.mock.calls[0][1][0]).toBe('org')
  })

  it('scopes draft reads to both organization and authenticated user', async () => {
    query.mockResolvedValue([])
    expect(await service.getDraft('org-a', 'user-a')).toBeNull()
    expect(query.mock.calls[0][1]).toEqual(['org-a', 'user-a'])
  })

  it('rejects a stale autosave instead of overwriting newer work', async () => {
    query.mockResolvedValue([])
    await expect(service.saveDraft('org', 'user', { text: 'old', revision: 2 })).rejects.toBeInstanceOf(
      ConflictException,
    )
    expect(query.mock.calls[0][1]).toEqual(['org', 'user', 'old', 2])
  })

  it('returns the original handoff on retry and rejects reuse with changed content', async () => {
    const row = { requestId: 'request', text: 'build app', createdAt: new Date(), copiedAt: null }
    query.mockResolvedValueOnce([]).mockResolvedValueOnce([row])
    expect(await service.createHandoff('org', 'user', { requestId: 'request', text: 'build app' })).toEqual(row)
    query.mockResolvedValueOnce([]).mockResolvedValueOnce([row])
    await expect(
      service.createHandoff('org', 'user', { requestId: 'request', text: 'changed' }),
    ).rejects.toBeInstanceOf(ConflictException)
  })

  it("does not acknowledge another identity's handoff", async () => {
    query.mockResolvedValue([])
    await expect(service.markCopied('other-org', 'other-user', 'request')).rejects.toBeInstanceOf(NotFoundException)
    expect(query.mock.calls[0][1]).toEqual(['other-org', 'other-user', 'request'])
  })

  it('never passes database parameters to the exception logger', async () => {
    query.mockRejectedValue(new Error('query failed: private prompt and key'))
    await expect(service.getDraft('org', 'user')).rejects.toEqual(
      new InternalServerErrorException('Unable to persist onboarding data. Please retry.'),
    )
  })

  it('rejects profile reads and writes after an organization is removed', async () => {
    query.mockResolvedValue([])
    await expect(service.getOnboarding('missing')).rejects.toBeInstanceOf(NotFoundException)
    await expect(service.saveOnboarding('missing', 'user', { useCase: 'exploring', note: '' })).rejects.toBeInstanceOf(
      NotFoundException,
    )
  })

  it.each([0, 3])('returns the persisted draft for revision %s', async (revision) => {
    const row = { text: 'saved', revision: revision + 1, updatedAt: new Date() }
    query.mockResolvedValueOnce(revision === 0 ? [row] : [[row], 1])
    expect(await service.saveDraft('org', 'user', { text: 'saved', revision })).toEqual(row)
    expect(query.mock.calls[0][1]).toEqual(
      revision === 0 ? ['org', 'user', 'saved'] : ['org', 'user', 'saved', revision],
    )
    query.mockResolvedValueOnce([row])
    expect(await service.getDraft('org', 'user')).toEqual(row)
  })

  it('rejects a handoff removed before it can be read', async () => {
    query.mockResolvedValue([])
    await expect(service.createHandoff('org', 'user', { requestId: 'request', text: 'app' })).rejects.toBeInstanceOf(
      ConflictException,
    )
  })

  it('returns the database copy timestamp for the current identity', async () => {
    const row = { requestId: 'request', text: 'app', copiedAt: new Date() }
    query.mockResolvedValue([[row], 1])
    expect(await service.markCopied('org', 'user', 'request')).toEqual(row)
    expect(query.mock.calls[0][1]).toEqual(['org', 'user', 'request'])
  })

  const pipe = new ValidationPipe({ transform: true, whitelist: true, forbidNonWhitelisted: true })
  it.each([
    [SaveOnboardingDto, { useCase: 'data-jobs', note: '' }],
    [SaveOnboardingDto, { useCase: 'exploring', note: '', submittedBy: 'forged' }],
    [SaveOnboardingDto, { useCase: 'exploring', note: 'x'.repeat(2001) }],
    [SavePromptDraftDto, { text: 'x'.repeat(16001), revision: 0 }],
    [SavePromptDraftDto, { text: 'app', revision: -1 }],
    [SavePromptDraftDto, { text: 'app', revision: 0, apiKey: 'secret' }],
    [CreatePromptHandoffDto, { requestId: 'invalid', text: 'app' }],
    [CreatePromptHandoffDto, { requestId: '12345678-1234-4234-8234-123456789012', text: '   ' }],
  ])('rejects invalid or unexpected input %p', async (metatype, value) => {
    await expect(pipe.transform(value, { type: 'body', metatype })).rejects.toThrow()
  })

  it('allows clearing a draft', async () => {
    await expect(
      pipe.transform({ text: '', revision: 1 }, { type: 'body', metatype: SavePromptDraftDto }),
    ).resolves.toMatchObject({ text: '' })
  })
})
