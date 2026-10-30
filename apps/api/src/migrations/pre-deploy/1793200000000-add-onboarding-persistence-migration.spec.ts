import { randomUUID } from 'node:crypto'
import { DataSource } from 'typeorm'
import { AddOnboardingPersistence1793200000000 } from './1793200000000-add-onboarding-persistence-migration'
import { OnboardingService } from '../../organization/services/onboarding.service'

// Local runs opt in explicitly. CI already provisions a disposable Postgres service;
// only select its known loopback defaults, never a deployed application's database.
const ciDatabase = process.env.CI && process.env.DB_HOST === 'localhost' && process.env.DB_DATABASE === 'boxlite'
const databaseUrl =
  process.env.ONBOARDING_TEST_DATABASE_URL ??
  (ciDatabase ? 'postgres://postgres:postgres@localhost:5432/boxlite' : undefined)
const integration = databaseUrl ? describe : describe.skip
integration('onboarding persistence (real Postgres)', () => {
  const schema = `onboarding_${randomUUID().replaceAll('-', '')}`
  const org = randomUUID()
  const otherOrg = randomUUID()
  let db: DataSource
  let service: OnboardingService
  const migration = new AddOnboardingPersistence1793200000000()
  beforeAll(async () => {
    const url = new URL(databaseUrl!)
    if (!['localhost', '127.0.0.1', '[::1]'].includes(url.hostname))
      throw new Error('Local disposable database required')
    const setup = await new DataSource({ type: 'postgres', url: databaseUrl, entities: [] }).initialize()
    try {
      await setup.query(`CREATE SCHEMA "${schema}"`)
    } finally {
      await setup.destroy()
    }
    db = await new DataSource({
      type: 'postgres',
      url: databaseUrl,
      entities: [],
      extra: { options: `-c search_path=${schema}` },
    }).initialize()
    await db.query('CREATE TABLE organization (id uuid PRIMARY KEY, "updatedAt" timestamptz DEFAULT now())')
    await db.query('INSERT INTO organization (id) VALUES ($1), ($2)', [org, otherOrg])
    const runner = db.createQueryRunner()
    try {
      await migration.up(runner)
    } finally {
      await runner.release()
    }
    service = new OnboardingService(db)
  })
  afterAll(async () => {
    if (!db?.isInitialized) return
    try {
      await db.query(`DROP SCHEMA "${schema}" CASCADE`)
    } finally {
      await db.destroy()
    }
  })

  it('keeps old organizations valid and persists onboarding across service instances', async () => {
    expect(await service.getOnboarding(org)).toBeNull()
    await service.saveOnboarding(org, 'owner', { useCase: 'long-running-agent', note: 'always running' })
    expect(await new OnboardingService(db).getOnboarding(org)).toMatchObject({
      useCase: 'long-running-agent',
      submittedBy: 'owner',
    })
    expect(await service.getOnboarding(otherOrg)).toBeNull()
  })

  it('isolates drafts by org and user and rejects stale concurrent writes', async () => {
    const first = await service.saveDraft(org, 'alice', { text: 'first', revision: 0 })
    expect(first.revision).toBe(1)
    expect(await service.getDraft(org, 'bob')).toBeNull()
    expect(await service.getDraft(otherOrg, 'alice')).toBeNull()
    const updates = await Promise.allSettled([
      service.saveDraft(org, 'alice', { text: 'one', revision: 1 }),
      service.saveDraft(org, 'alice', { text: 'two', revision: 1 }),
    ])
    expect(updates.filter((r) => r.status === 'fulfilled')).toHaveLength(1)
    expect(updates.filter((r) => r.status === 'rejected')).toHaveLength(1)
    expect((await service.getDraft(org, 'alice'))?.revision).toBe(2)
    await service.saveDraft(org, 'alice', { text: '', revision: 2 })
    expect((await service.getDraft(org, 'alice'))?.text).toBe('')
  })

  it('deduplicates simultaneous handoffs and acknowledges copying independently', async () => {
    await service.saveDraft(org, 'handoff-user', { text: '', revision: 0 })
    const input = { requestId: randomUUID(), text: 'build my app' }
    const results = await Promise.all(
      Array.from({ length: 5 }, () => service.createHandoff(org, 'handoff-user', input)),
    )
    expect(results.every((r) => r.createdAt.getTime() === results[0].createdAt.getTime())).toBe(true)
    expect(results[0].copiedAt).toBeNull()
    const [{ count }] = await db.query('SELECT count(*) FROM quickstart_prompt_handoff')
    expect(Number(count)).toBe(1)
    await expect(service.createHandoff(org, 'handoff-user', { ...input, text: 'different' })).rejects.toThrow(
      'different text',
    )
    await expect(service.markCopied(otherOrg, 'handoff-user', input.requestId)).rejects.toThrow('not found')
    await expect(service.markCopied(org, 'bob', input.requestId)).rejects.toThrow('not found')
    const copied = await service.markCopied(org, 'handoff-user', input.requestId)
    expect(copied.copiedAt).not.toBeNull()
    expect((await service.markCopied(org, 'handoff-user', input.requestId)).copiedAt).toEqual(copied.copiedAt)
    expect((await service.getDraft(org, 'handoff-user'))?.text).toBe('')
  })

  it('cascades organization deletion and rolls the migration back and forward', async () => {
    await db.query('DELETE FROM organization WHERE id = $1', [org])
    expect(await service.getDraft(org, 'alice')).toBeNull()
    expect(await db.query('SELECT * FROM quickstart_prompt_handoff')).toEqual([])
    const runner = db.createQueryRunner()
    try {
      await migration.down(runner)
      const [row] = await db.query("SELECT to_regclass('quickstart_prompt_draft') AS name")
      expect(row.name).toBeNull()
      await migration.up(runner)
      expect(await service.getOnboarding(otherOrg)).toBeNull()
    } finally {
      await runner.release()
    }
  })
})
