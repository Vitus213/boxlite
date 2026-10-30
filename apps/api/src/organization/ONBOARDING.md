# Organization onboarding and prompt persistence

Onboarding is optional organization profile data, separate from billing and Quickstart completion.
Existing organizations return null. Owners may replace the answer; authenticated organization
members may read it. Server-generated submitter and timestamps cannot be supplied by clients.
Stable use cases include `long-running-agent`; the old preview `data-jobs` value is not accepted.

All endpoints below are relative to `/organizations/:organizationId`, require a user JWT,
and enforce organization membership through the existing organization guard.

| Method/path                                  | Request                | Result                                                |
| -------------------------------------------- | ---------------------- | ----------------------------------------------------- |
| GET /onboarding                              | —                      | Onboarding profile or null                            |
| PUT /onboarding                              | useCase, note          | Saved profile (owner only)                            |
| GET /quickstart/prompt                       | —                      | Current user's draft or null                          |
| PUT /quickstart/prompt                       | text, revision         | Saved draft; revision 0 creates, otherwise must match |
| POST /quickstart/handoffs                    | requestId (UUID), text | Immutable snapshot; same ID/content returns original  |
| PATCH /quickstart/handoffs/:requestId/copied | —                      | Idempotently mark successful clipboard copy           |

Drafts are keyed by organization and authenticated user. Empty text clears a draft. Revision
conflicts return 409 so delayed autosaves and multiple tabs cannot silently overwrite newer
work. Clients should serialize saves and reload on conflict. Handoff text must not be empty.
A handoff request ID is scoped to organization and user. Reusing it with different text returns
409; concurrent identical submissions return one snapshot. Handoffs do not modify drafts.
A created snapshot means prepared for copying; copiedAt is populated only after the client
reports clipboard success. Neither event indicates a successful deployment.

Only original user text is accepted (maximum 16,000 characters); do not send the assembled
credential-bearing agent prompt. Unknown DTO properties are rejected and these routes do not
log bodies or add text to audit/analytics payloads. This does not detect arbitrary secrets a
user types into their own request. Onboarding notes are limited to 2,000 characters.

Run the additive pre-deploy migration before deploying the API. Both prompt tables reference
the organization with ON DELETE CASCADE; leaving an organization removes API access through
membership checks, but does not erase historical submissions. Rollback drops these new tables
and the nullable onboarding column, destroying only this feature's data.

Frontend integration is separate: save onboarding before polling billing; debounce draft saves;
flush the latest draft before preparing a handoff, copy locally, then acknowledge copiedAt.
The API never generates or stores an API key as part of this workflow.

## Verification

The migration integration suite uses `ONBOARDING_TEST_DATABASE_URL`, accepts only loopback
hosts and creates a random isolated schema. It never reads the application's DB credentials.
Set `NX_SKIP_NX_CACHE=true` when enabling database integration tests so a cached skipped run
cannot substitute for database execution. Run `make test:apps:api FILTER=onboarding`.

Verified locally on PostgreSQL 17: migration up/down/up, existing organization null profile,
persistence across service instances, organization/user draft isolation, concurrent draft
revision conflicts, concurrent handoff deduplication, changed-content conflicts, idempotent
copy acknowledgement and organization deletion cascade. Controller guard and DTO/service
unit tests also cover role restrictions, input limits, unknown fields and sanitized errors.

Deploy the pre-deploy migration and API before the new Console. The Console saves
onboarding before checking credit, restores each user's prompt draft, and acknowledges
copying only after handing the assembled prompt to the clipboard.

CI runs the onboarding migration integration suite against the API job’s existing disposable PostgreSQL service when its standard localhost/boxlite configuration is present. Local runs opt in with `ONBOARDING_TEST_DATABASE_URL`. Each run uses an isolated schema and removes it after testing; without either configuration, the suite is skipped.

Nullable onboarding reads explicitly serialize JSON `null`, including for new users without prompt drafts. Revision schemas use bounded `int32` values (request maximum 2147483646, response maximum 2147483647). Generated TypeScript onboarding timestamp properties are ISO strings, matching Axios response data; their OpenAPI wire format remains `date-time`.
