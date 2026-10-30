/* Copyright 2026 BoxLite AI · SPDX-License-Identifier: AGPL-3.0 */
import {
  Body,
  Controller,
  Get,
  HttpCode,
  Param,
  ParseUUIDPipe,
  Patch,
  Post,
  Put,
  Res,
  UseGuards,
  UsePipes,
  ValidationPipe,
} from '@nestjs/common'
import type { Response } from 'express'
import { AuthGuard } from '@nestjs/passport'
import { ApiBearerAuth, ApiExtraModels, ApiOkResponse, ApiOperation, ApiTags, getSchemaPath } from '@nestjs/swagger'
import { AuthContext } from '../../common/decorators/auth-context.decorator'
import { AuthContext as UserContext } from '../../common/interfaces/auth-context.interface'
import { AuthenticatedRateLimitGuard } from '../../common/guards/authenticated-rate-limit.guard'
import { RequiredOrganizationMemberRole } from '../decorators/required-organization-member-role.decorator'
import { OrganizationMemberRole } from '../enums/organization-member-role.enum'
import { OrganizationActionGuard } from '../guards/organization-action.guard'
import {
  CreatePromptHandoffDto,
  OnboardingDto,
  PromptDraftDto,
  PromptHandoffDto,
  SaveOnboardingDto,
  SavePromptDraftDto,
} from '../dto/onboarding.dto'
import { OnboardingService } from '../services/onboarding.service'

@ApiTags('organizations')
@ApiExtraModels(OnboardingDto, PromptDraftDto)
@ApiBearerAuth()
@Controller('organizations/:organizationId')
@UseGuards(AuthGuard('jwt'), AuthenticatedRateLimitGuard, OrganizationActionGuard)
@UsePipes(
  new ValidationPipe({
    transform: true,
    whitelist: true,
    forbidNonWhitelisted: true,
    validationError: { target: false, value: false },
  }),
)
export class OnboardingController {
  constructor(private readonly service: OnboardingService) {}

  @Get('onboarding')
  @ApiOperation({ operationId: 'getOrganizationOnboarding', summary: 'Read organization onboarding answers' })
  @ApiOkResponse({ schema: { nullable: true, allOf: [{ $ref: getSchemaPath(OnboardingDto) }] } })
  async getOnboarding(@Param('organizationId') org: string, @Res() response: Response) {
    response.json(await this.service.getOnboarding(org))
  }

  @Put('onboarding')
  @RequiredOrganizationMemberRole(OrganizationMemberRole.OWNER)
  @ApiOperation({ operationId: 'saveOrganizationOnboarding', summary: 'Save organization onboarding answers' })
  @ApiOkResponse({ type: OnboardingDto })
  saveOnboarding(
    @Param('organizationId') org: string,
    @AuthContext() auth: UserContext,
    @Body() input: SaveOnboardingDto,
  ) {
    return this.service.saveOnboarding(org, auth.userId, input)
  }

  @Get('quickstart/prompt')
  @ApiOperation({ operationId: 'getQuickstartPrompt', summary: 'Read your prompt draft in this organization' })
  @ApiOkResponse({ schema: { nullable: true, allOf: [{ $ref: getSchemaPath(PromptDraftDto) }] } })
  async getDraft(@Param('organizationId') org: string, @AuthContext() auth: UserContext, @Res() response: Response) {
    response.json(await this.service.getDraft(org, auth.userId))
  }

  @Put('quickstart/prompt')
  @ApiOperation({ operationId: 'saveQuickstartPrompt', summary: 'Save your prompt draft with revision checking' })
  @ApiOkResponse({ type: PromptDraftDto })
  saveDraft(@Param('organizationId') org: string, @AuthContext() auth: UserContext, @Body() input: SavePromptDraftDto) {
    return this.service.saveDraft(org, auth.userId, input)
  }

  @Post('quickstart/handoffs')
  @HttpCode(200)
  @ApiOperation({ operationId: 'createQuickstartHandoff', summary: 'Prepare an immutable prompt handoff' })
  @ApiOkResponse({ type: PromptHandoffDto })
  createHandoff(
    @Param('organizationId') org: string,
    @AuthContext() auth: UserContext,
    @Body() input: CreatePromptHandoffDto,
  ) {
    return this.service.createHandoff(org, auth.userId, input)
  }

  @Patch('quickstart/handoffs/:requestId/copied')
  @ApiOperation({ operationId: 'markQuickstartHandoffCopied', summary: 'Record successful clipboard copy' })
  @ApiOkResponse({ type: PromptHandoffDto })
  markCopied(
    @Param('organizationId') org: string,
    @AuthContext() auth: UserContext,
    @Param('requestId', ParseUUIDPipe) id: string,
  ) {
    return this.service.markCopied(org, auth.userId, id)
  }
}
