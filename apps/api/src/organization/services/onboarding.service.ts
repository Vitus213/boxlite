/* Copyright 2026 BoxLite AI · SPDX-License-Identifier: AGPL-3.0 */
import { ConflictException, Injectable, InternalServerErrorException, NotFoundException } from '@nestjs/common'
import { DataSource } from 'typeorm'
import {
  CreatePromptHandoffDto,
  OnboardingDto,
  PromptDraftDto,
  PromptHandoffDto,
  SaveOnboardingDto,
  SavePromptDraftDto,
} from '../dto/onboarding.dto'

@Injectable()
export class OnboardingService {
  constructor(private readonly db: DataSource) {}

  private async query(sql: string, parameters: unknown[]): Promise<any[]> {
    try {
      const result = await this.db.query(sql, parameters)
      // Postgres returns [records, affected] for UPDATE, but records directly for SELECT/INSERT.
      return Array.isArray(result[0]) && typeof result[1] === 'number' ? result[0] : result
    } catch {
      // TypeORM errors contain SQL parameters, including users' private text.
      throw new InternalServerErrorException('Unable to persist onboarding data. Please retry.')
    }
  }

  async getOnboarding(organizationId: string): Promise<OnboardingDto | null> {
    const [row] = await this.query('SELECT onboarding FROM organization WHERE id = $1', [organizationId])
    if (!row) throw new NotFoundException('Organization not found')
    return row.onboarding
  }

  async saveOnboarding(organizationId: string, userId: string, input: SaveOnboardingDto): Promise<OnboardingDto> {
    const value = {
      useCase: input.useCase,
      note: input.note.trim(),
      submittedBy: userId,
      submittedAt: new Date().toISOString(),
    }
    const [row] = await this.query(
      'UPDATE organization SET onboarding = $2::jsonb, "updatedAt" = now() WHERE id = $1 RETURNING onboarding',
      [organizationId, JSON.stringify(value)],
    )
    if (!row) throw new NotFoundException('Organization not found')
    return row.onboarding
  }

  async getDraft(organizationId: string, userId: string): Promise<PromptDraftDto | null> {
    const [row] = await this.query(
      'SELECT text, revision, "updatedAt" FROM quickstart_prompt_draft WHERE "organizationId" = $1 AND "userId" = $2',
      [organizationId, userId],
    )
    return row ?? null
  }

  async saveDraft(organizationId: string, userId: string, input: SavePromptDraftDto): Promise<PromptDraftDto> {
    const rows =
      input.revision === 0
        ? await this.query(
            `INSERT INTO quickstart_prompt_draft ("organizationId", "userId", text)
          VALUES ($1, $2, $3) ON CONFLICT DO NOTHING RETURNING text, revision, "updatedAt"`,
            [organizationId, userId, input.text],
          )
        : await this.query(
            `UPDATE quickstart_prompt_draft SET text = $3, revision = revision + 1, "updatedAt" = now()
          WHERE "organizationId" = $1 AND "userId" = $2 AND revision = $4 RETURNING text, revision, "updatedAt"`,
            [organizationId, userId, input.text, input.revision],
          )
    if (!rows[0]) throw new ConflictException('Draft changed; reload before saving again.')
    return rows[0]
  }

  async createHandoff(
    organizationId: string,
    userId: string,
    input: CreatePromptHandoffDto,
  ): Promise<PromptHandoffDto> {
    // Unique key arbitrates concurrent retries; a separate SELECT sees the committed winning insert.
    await this.query(
      `INSERT INTO quickstart_prompt_handoff ("organizationId", "userId", "requestId", text)
      VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING`,
      [organizationId, userId, input.requestId, input.text],
    )
    const [row] = await this.query(
      `SELECT "requestId", text, "createdAt", "copiedAt" FROM quickstart_prompt_handoff
      WHERE "organizationId" = $1 AND "userId" = $2 AND "requestId" = $3`,
      [organizationId, userId, input.requestId],
    )
    if (!row || row.text !== input.text) throw new ConflictException('Request ID already used with different text.')
    return row
  }

  async markCopied(organizationId: string, userId: string, requestId: string): Promise<PromptHandoffDto> {
    const [row] = await this.query(
      `UPDATE quickstart_prompt_handoff SET "copiedAt" = COALESCE("copiedAt", now())
      WHERE "organizationId" = $1 AND "userId" = $2 AND "requestId" = $3
      RETURNING "requestId", text, "createdAt", "copiedAt"`,
      [organizationId, userId, requestId],
    )
    if (!row) throw new NotFoundException('Handoff not found')
    return row
  }
}
