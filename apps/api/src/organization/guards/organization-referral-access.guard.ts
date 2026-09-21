/*
 * Copyright 2025 Daytona Platforms Inc.
 * Modified by BoxLite AI, 2025-2026
 * SPDX-License-Identifier: AGPL-3.0
 */

import { ExecutionContext, HttpStatus, Injectable } from '@nestjs/common'
import { isUUID } from 'class-validator'
import { OrganizationReferralCodeException } from '../../exceptions/organization-referral-code.exception'
import { OrganizationAccessGuard } from './organization-access.guard'

@Injectable()
export class OrganizationReferralAccessGuard extends OrganizationAccessGuard {
  async canActivate(context: ExecutionContext): Promise<boolean> {
    const organizationId = context.switchToHttp().getRequest().params.organizationId
    if (typeof organizationId !== 'string' || !isUUID(organizationId, '4')) {
      throw new OrganizationReferralCodeException(HttpStatus.BAD_REQUEST, 'invalid_organization_id')
    }
    if (!(await super.canActivate(context))) {
      throw new OrganizationReferralCodeException(HttpStatus.FORBIDDEN, 'invitation_unavailable')
    }
    return true
  }
}
