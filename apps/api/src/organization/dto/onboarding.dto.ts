/* Copyright 2026 BoxLite AI · SPDX-License-Identifier: AGPL-3.0 */
import { ApiProperty } from '@nestjs/swagger'
import { IsIn, IsInt, IsString, IsUUID, Matches, Max, MaxLength, Min } from 'class-validator'

export const ONBOARDING_USE_CASES = [
  'coding-agent',
  'code-execution',
  'browser-agent',
  'long-running-agent',
  'application-deployments',
  'reinforcement-learning',
  'exploring',
] as const

export class SaveOnboardingDto {
  @ApiProperty({ enum: ONBOARDING_USE_CASES })
  @IsIn(ONBOARDING_USE_CASES)
  useCase: (typeof ONBOARDING_USE_CASES)[number]

  @ApiProperty({ maxLength: 2000 })
  @IsString()
  @MaxLength(2000)
  note: string
}

export class OnboardingDto extends SaveOnboardingDto {
  @ApiProperty()
  submittedBy: string

  @ApiProperty({ format: 'date-time' })
  submittedAt: string
}

export class SavePromptDraftDto {
  @ApiProperty({ maxLength: 16000 })
  @IsString()
  @MaxLength(16000)
  text: string

  @ApiProperty({
    type: 'integer',
    format: 'int32',
    minimum: 0,
    maximum: 2147483646,
    description: 'Last read revision; 0 creates a new draft.',
  })
  @IsInt()
  @Min(0)
  @Max(2147483646)
  revision: number
}

export class CreatePromptHandoffDto {
  @ApiProperty({ format: 'uuid' })
  @IsUUID()
  requestId: string

  @ApiProperty({ maxLength: 16000 })
  @IsString()
  @MaxLength(16000)
  @Matches(/\S/, { message: 'text must not be blank' })
  text: string
}

export class PromptDraftDto extends SavePromptDraftDto {
  @ApiProperty({ type: 'integer', format: 'int32', minimum: 0, maximum: 2147483647 })
  declare revision: number

  @ApiProperty({ format: 'date-time' })
  updatedAt: Date
}

export class PromptHandoffDto extends CreatePromptHandoffDto {
  @ApiProperty({ format: 'date-time' })
  createdAt: Date

  @ApiProperty({ type: String, format: 'date-time', nullable: true })
  copiedAt: Date | null
}
