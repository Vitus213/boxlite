/* Copyright 2026 BoxLite AI · SPDX-License-Identifier: AGPL-3.0 */
import { MigrationInterface, QueryRunner } from 'typeorm'

export class AddOnboardingPersistence1793200000000 implements MigrationInterface {
  async up(runner: QueryRunner): Promise<void> {
    await runner.query('ALTER TABLE organization ADD COLUMN onboarding jsonb NULL')
    await runner.query(`CREATE TABLE quickstart_prompt_draft (
      "organizationId" uuid NOT NULL REFERENCES organization(id) ON DELETE CASCADE,
      "userId" varchar NOT NULL, text text NOT NULL CHECK (char_length(text) <= 16000),
      revision integer NOT NULL DEFAULT 1 CHECK (revision > 0),
      "updatedAt" timestamptz NOT NULL DEFAULT now(), PRIMARY KEY ("organizationId", "userId"))`)
    await runner.query(`CREATE TABLE quickstart_prompt_handoff (
      "organizationId" uuid NOT NULL REFERENCES organization(id) ON DELETE CASCADE,
      "userId" varchar NOT NULL, "requestId" uuid NOT NULL,
      text text NOT NULL CHECK (char_length(text) BETWEEN 1 AND 16000),
      "createdAt" timestamptz NOT NULL DEFAULT now(), "copiedAt" timestamptz NULL,
      PRIMARY KEY ("organizationId", "userId", "requestId"))`)
  }

  async down(runner: QueryRunner): Promise<void> {
    await runner.query('DROP TABLE quickstart_prompt_handoff')
    await runner.query('DROP TABLE quickstart_prompt_draft')
    await runner.query('ALTER TABLE organization DROP COLUMN onboarding')
  }
}
