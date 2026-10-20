// Type aliases derived from the generated OpenAPI schema. Never hand-write
// request/response shapes here: everything comes from `schema.d.ts`, which is
// regenerated with `pnpm run gen:api` and drift-checked by `pnpm run check:api`.
import type { components } from './schema';

type Schemas = components['schemas'];

export type ErrorBody = Schemas['ErrorBody'];
export type ErrorCode = Schemas['ErrorCode'];
export type SessionInfo = Schemas['SessionInfo'];
export type Role = Schemas['Role'];
export type Scope = Schemas['Scope'];
export type OverviewResponse = Schemas['OverviewResponse'];
export type Overview = Schemas['Overview'];
export type QueueStats = Schemas['QueueStats'];
export type RepositoryView = Schemas['RepositoryView'];
export type RepositoryProvider = Schemas['RepositoryProvider'];
export type ComponentView = Schemas['ComponentView'];
export type CreateRepository = Schemas['CreateRepository'];
export type CreateComponent = Schemas['CreateComponent'];
export type Permission = Schemas['Permission'];
export type SolutionSummary = Schemas['SolutionSummary'];
export type SolutionDetail = Schemas['SolutionDetail'];
export type SolutionState = Schemas['SolutionState'];
export type SolutionAction = Schemas['SolutionAction'];
export type SolutionStateChange = Schemas['SolutionStateChange'];
export type SolutionVersionRef = Schemas['SolutionVersionRef'];
export type PropagationList = Schemas['PropagationList'];
export type PropagationCandidate = Schemas['PropagationCandidate'];
export type MergeState = Schemas['MergeState'];
export type DeploymentState = Schemas['DeploymentState'];
export type Applicability = Schemas['Applicability'];
export type ApplicabilityStatus = Schemas['ApplicabilityStatus'];
export type EvidenceView = Schemas['EvidenceView'];
export type EvidenceResult = Schemas['EvidenceResult'];
export type SourceLink = Schemas['SourceLink'];
export type VerificationSummary = Schemas['VerificationSummary'];
export type ContextSearchRequest = Schemas['ContextSearchRequest'];
export type ContextSearchResponse = Schemas['ContextSearchResponse'];
export type Suggestion = Schemas['Suggestion'];
export type SessionStatusResponse = Schemas['SessionStatusResponse'];
export type ExtractionStatus = Schemas['ExtractionStatus'];
export type TranscriptView = Schemas['TranscriptView'];
export type DeletionReport = Schemas['DeletionReport'];
export type CiEventView = Schemas['CiEventView'];
export type RetrievalView = Schemas['RetrievalView'];
export type JobView = Schemas['JobView'];
export type TokenRow = Schemas['TokenRow'];
export type TokenCreated = Schemas['TokenCreated'];
export type CreateToken = Schemas['CreateToken'];
export type PrincipalListRow = Schemas['PrincipalListRow'];
export type CreatePrincipal = Schemas['CreatePrincipal'];
export type PrincipalKind = Schemas['PrincipalKind'];
export type GrantRow = Schemas['GrantRow'];
export type SettingsView = Schemas['SettingsView'];
export type SettingsPatch = Schemas['SettingsPatch'];
export type WebhookSecrets = Schemas['WebhookSecrets'];
export type SweepReport = Schemas['SweepReport'];
export type AuditEventView = Schemas['AuditEventView'];
export type WorkflowImprovementView = Schemas['WorkflowImprovementView'];
export type WorkflowState = Schemas['WorkflowState'];
export type WorkflowTransition = Schemas['WorkflowTransition'];
export type CreateWorkflowImprovement = Schemas['CreateWorkflowImprovement'];

/**
 * Runtime lists of schema enum values (for selects and checkboxes). The
 * helper fails type-checking when the schema gains a value that is missing
 * here, so contract drift surfaces in `tsc` instead of silently in the UI.
 */
function enumValues<T extends string>() {
  return <const A extends readonly T[]>(values: A & ([T] extends [A[number]] ? unknown : 'missing enum values')): A =>
    values;
}

export const SOLUTION_STATES = enumValues<SolutionState>()([
  'candidate',
  'corroborated',
  'verified',
  'superseded',
  'withdrawn',
]);
export const EXTRACTION_STATUSES = enumValues<ExtractionStatus>()([
  'pending',
  'running',
  'succeeded',
  'failed',
  'skipped',
]);
export const SCOPES = enumValues<Scope>()(['sessions:write', 'context:read', 'outcomes:write', 'ci:write', 'admin']);
export const ROLES = enumValues<Role>()(['member', 'admin']);
export const PRINCIPAL_KINDS = enumValues<PrincipalKind>()(['user', 'service']);
export const PERMISSIONS = enumValues<Permission>()(['read', 'write']);
export const REPOSITORY_PROVIDERS = enumValues<RepositoryProvider>()(['github', 'gitlab', 'generic']);
export const WORKFLOW_STATES = enumValues<WorkflowState>()(['candidate', 'evaluated', 'available', 'promoted', 'retired']);
