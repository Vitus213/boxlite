/* Copyright 2026 BoxLite AI · SPDX-License-Identifier: AGPL-3.0 */
import { Column, Entity, JoinColumn, ManyToOne, PrimaryColumn } from 'typeorm'
import { Organization } from './organization.entity'

abstract class OrganizationPrompt {
  @PrimaryColumn('uuid')
  organizationId: string

  @PrimaryColumn('varchar')
  userId: string

  @Column('text')
  text: string

  @ManyToOne(() => Organization, { onDelete: 'CASCADE' })
  @JoinColumn({ name: 'organizationId' })
  organization: Organization
}

@Entity('quickstart_prompt_draft')
export class QuickstartPromptDraft extends OrganizationPrompt {
  @Column({ type: 'int', default: 1 })
  revision: number

  @Column({ type: 'timestamptz', default: () => 'now()' })
  updatedAt: Date
}

@Entity('quickstart_prompt_handoff')
export class QuickstartPromptHandoff extends OrganizationPrompt {
  @PrimaryColumn('uuid')
  requestId: string

  @Column({ type: 'timestamptz', default: () => 'now()' })
  createdAt: Date

  @Column({ type: 'timestamptz', nullable: true })
  copiedAt: Date | null
}
