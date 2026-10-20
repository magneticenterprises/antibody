-- Antibody initial schema. PostgreSQL is the source of truth.
-- Every row is scoped to an organization; repository access is enforced by
-- the application before search, ranking and source expansion.

CREATE EXTENSION IF NOT EXISTS vector;

-- ---------------------------------------------------------------------------
-- Tenancy and identity

CREATE TABLE organizations (
    id uuid PRIMARY KEY,
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    name text NOT NULL,
    -- Raw (redacted) transcript capture must be explicitly enabled.
    raw_capture_enabled boolean NOT NULL DEFAULT false,
    raw_retention_days integer NOT NULL DEFAULT 30 CHECK (raw_retention_days BETWEEN 0 AND 3650),
    -- Private content is only sent to external model providers after approval.
    external_providers_approved boolean NOT NULL DEFAULT false,
    external_providers_approved_by uuid,
    external_providers_approved_at timestamptz,
    -- CI lookups search only the failing repository unless enabled.
    ci_cross_repository_search boolean NOT NULL DEFAULT false,
    webhook_secret_version integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE principals (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('user', 'service')),
    display_name text NOT NULL,
    role text NOT NULL CHECK (role IN ('admin', 'member')),
    created_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz
);
CREATE INDEX principals_org_idx ON principals (organization_id);

CREATE TABLE users (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    principal_id uuid NOT NULL UNIQUE REFERENCES principals(id) ON DELETE CASCADE,
    username text NOT NULL UNIQUE CHECK (length(username) BETWEEN 1 AND 128),
    password_hash text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_login_at timestamptz
);

CREATE TABLE api_tokens (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    principal_id uuid NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    name text NOT NULL,
    token_prefix text NOT NULL,
    -- SHA-256 of the full token; plaintext is shown once and never stored.
    token_hash bytea NOT NULL UNIQUE,
    scopes text[] NOT NULL CHECK (scopes <@ ARRAY['sessions:write', 'context:read', 'outcomes:write', 'ci:write', 'admin']::text[]),
    -- NULL: the principal's full repository access. Otherwise an allow-list.
    repository_ids uuid[],
    created_by uuid REFERENCES principals(id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz,
    last_used_at timestamptz,
    revoked_at timestamptz
);
CREATE INDEX api_tokens_org_idx ON api_tokens (organization_id);

CREATE TABLE web_sessions (
    id_hash bytea PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    principal_id uuid NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    csrf_token text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);
CREATE INDEX web_sessions_expiry_idx ON web_sessions (expires_at);

-- ---------------------------------------------------------------------------
-- Repositories and components

CREATE TABLE repositories (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    provider text NOT NULL CHECK (provider IN ('github', 'gitlab', 'generic')),
    provider_repo_id text,
    full_name text NOT NULL CHECK (full_name ~ '^[A-Za-z0-9_.-]+(/[A-Za-z0-9_.-]+)+$'),
    url text,
    default_branch text NOT NULL DEFAULT 'main',
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disconnected')),
    last_event_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organization_id, provider, full_name)
);
CREATE INDEX repositories_full_name_idx ON repositories (organization_id, lower(full_name));

CREATE TABLE repository_grants (
    principal_id uuid NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    repository_id uuid NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    permission text NOT NULL CHECK (permission IN ('read', 'write')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (principal_id, repository_id)
);

CREATE TABLE components (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    repository_id uuid NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    name text NOT NULL,
    path_glob text NOT NULL,
    package_name text,
    ecosystem text,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (repository_id, name)
);

-- ---------------------------------------------------------------------------
-- Sessions (Flow A)

CREATE TABLE sessions (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    repository_id uuid NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    principal_id uuid REFERENCES principals(id) ON DELETE SET NULL,
    agent_type text NOT NULL CHECK (agent_type IN ('claude_code', 'codex', 'other')),
    external_id text,
    idempotency_key text NOT NULL,
    request_hash bytea NOT NULL,
    revision text,
    branch text,
    component_path text,
    capture_mode text NOT NULL CHECK (capture_mode IN ('raw', 'structured_only')),
    ingestion_status text NOT NULL DEFAULT 'accepted' CHECK (ingestion_status IN ('accepted')),
    extraction_status text NOT NULL DEFAULT 'pending'
        CHECK (extraction_status IN ('pending', 'running', 'succeeded', 'failed', 'skipped')),
    extraction_error text,
    extractor text,
    message_count integer NOT NULL,
    payload_bytes integer NOT NULL,
    redaction_count integer NOT NULL DEFAULT 0,
    retention_expires_at timestamptz,
    started_at timestamptz,
    ended_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    processed_at timestamptz,
    UNIQUE (organization_id, idempotency_key)
);
CREATE INDEX sessions_repo_idx ON sessions (repository_id, created_at DESC);
CREATE INDEX sessions_org_created_idx ON sessions (organization_id, created_at DESC);
CREATE INDEX sessions_branch_idx ON sessions (repository_id, branch);

-- Redacted transcript payloads, kept apart so they can be purged on schedule.
CREATE TABLE session_payloads (
    session_id uuid PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    payload jsonb NOT NULL,
    purge_after timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX session_payloads_purge_idx ON session_payloads (purge_after);

-- ---------------------------------------------------------------------------
-- Source artifacts and solutions

CREATE TABLE source_artifacts (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    repository_id uuid NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('session', 'commit', 'pull_request', 'issue', 'check_run', 'workflow_run', 'deployment', 'url')),
    external_id text NOT NULL,
    url text,
    revision text,
    title text,
    state text,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    observed_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (repository_id, kind, external_id)
);
CREATE INDEX source_artifacts_revision_idx ON source_artifacts (repository_id, revision);

CREATE TABLE solutions (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    repository_id uuid NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    component_id uuid REFERENCES components(id) ON DELETE SET NULL,
    lineage_key text NOT NULL,
    fingerprint text NOT NULL,
    version integer NOT NULL DEFAULT 1,
    title text NOT NULL,
    symptom text NOT NULL,
    root_cause text,
    remedy text NOT NULL,
    error_signatures text[] NOT NULL DEFAULT '{}',
    signature_hashes text[] NOT NULL DEFAULT '{}',
    symbols text[] NOT NULL DEFAULT '{}',
    paths text[] NOT NULL DEFAULT '{}',
    -- Space-joined signatures/symbols/paths, maintained by the application
    -- (array_to_string is not immutable, so it cannot feed a generated column).
    keywords text NOT NULL DEFAULT '',
    applicability jsonb NOT NULL DEFAULT '{}'::jsonb,
    state text NOT NULL DEFAULT 'candidate'
        CHECK (state IN ('candidate', 'corroborated', 'verified', 'superseded', 'withdrawn')),
    needs_revalidation boolean NOT NULL DEFAULT false,
    revalidation_reason text,
    merge_state text NOT NULL DEFAULT 'unknown' CHECK (merge_state IN ('unknown', 'unmerged', 'merged', 'reverted')),
    deployment_state text NOT NULL DEFAULT 'unknown' CHECK (deployment_state IN ('unknown', 'deployed')),
    fix_revision text,
    fix_branch text,
    owner_principal_id uuid REFERENCES principals(id) ON DELETE SET NULL,
    superseded_by uuid REFERENCES solutions(id) ON DELETE SET NULL,
    confidence real NOT NULL DEFAULT 0.5 CHECK (confidence BETWEEN 0 AND 1),
    extractor text NOT NULL,
    corroboration_count integer NOT NULL DEFAULT 1,
    last_verified_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    search_tsv tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('english', coalesce(title, '')), 'A') ||
        setweight(to_tsvector('english', coalesce(symptom, '')), 'A') ||
        setweight(to_tsvector('simple', coalesce(keywords, '')), 'A') ||
        setweight(to_tsvector('english', coalesce(root_cause, '')), 'B') ||
        setweight(to_tsvector('english', coalesce(remedy, '')), 'C')
    ) STORED,
    UNIQUE (organization_id, repository_id, fingerprint)
);
CREATE INDEX solutions_tsv_idx ON solutions USING gin (search_tsv);
CREATE INDEX solutions_sighash_idx ON solutions USING gin (signature_hashes);
CREATE INDEX solutions_symbols_idx ON solutions USING gin (symbols);
CREATE INDEX solutions_paths_idx ON solutions USING gin (paths);
CREATE INDEX solutions_scope_idx ON solutions (organization_id, repository_id, state);
CREATE INDEX solutions_lineage_idx ON solutions (organization_id, lineage_key);
CREATE INDEX solutions_fix_revision_idx ON solutions (repository_id, fix_revision);

CREATE TABLE solution_sources (
    solution_id uuid NOT NULL REFERENCES solutions(id) ON DELETE CASCADE,
    source_artifact_id uuid NOT NULL REFERENCES source_artifacts(id) ON DELETE CASCADE,
    relation text NOT NULL CHECK (relation IN ('origin_session', 'fix_commit', 'pull_request', 'check_run', 'issue', 'deployment', 'related')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (solution_id, source_artifact_id, relation)
);

-- Which sessions produced (or corroborated) a solution. Deleting a session
-- deletes solutions that no remaining session supports.
CREATE TABLE solution_sessions (
    solution_id uuid NOT NULL REFERENCES solutions(id) ON DELETE CASCADE,
    session_id uuid NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (solution_id, session_id)
);
CREATE INDEX solution_sessions_session_idx ON solution_sessions (session_id);

CREATE TABLE evidence (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    solution_id uuid NOT NULL REFERENCES solutions(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('ci_check', 'merge', 'revert', 'deployment', 'outcome', 'agent_claim', 'manual_verification', 'dependency_change', 'corroboration')),
    result text NOT NULL CHECK (result IN ('pass', 'fail', 'info')),
    trusted boolean NOT NULL,
    tested_revision text,
    check_name text,
    environment text,
    source_artifact_id uuid REFERENCES source_artifacts(id) ON DELETE SET NULL,
    detail text,
    dedupe_key text NOT NULL,
    observed_at timestamptz NOT NULL DEFAULT now(),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (solution_id, dedupe_key)
);

CREATE TABLE embeddings (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    solution_id uuid NOT NULL REFERENCES solutions(id) ON DELETE CASCADE,
    content_hash text NOT NULL,
    provider text NOT NULL,
    model text NOT NULL,
    model_version text NOT NULL DEFAULT '',
    dimensions integer NOT NULL,
    embedding vector NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (solution_id, provider, model, model_version)
);
CREATE INDEX embeddings_scope_idx ON embeddings (organization_id, provider, model, model_version);

CREATE TABLE workflow_improvements (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    repository_id uuid REFERENCES repositories(id) ON DELETE CASCADE,
    source_session_id uuid REFERENCES sessions(id) ON DELETE CASCADE,
    target_scope text NOT NULL,
    title text NOT NULL,
    proposed_change text NOT NULL,
    fingerprint text NOT NULL,
    version integer NOT NULL DEFAULT 1,
    evaluation jsonb,
    owner_principal_id uuid REFERENCES principals(id) ON DELETE SET NULL,
    approved_by uuid REFERENCES principals(id) ON DELETE SET NULL,
    state text NOT NULL DEFAULT 'candidate' CHECK (state IN ('candidate', 'evaluated', 'available', 'promoted', 'retired')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organization_id, fingerprint)
);

-- ---------------------------------------------------------------------------
-- Retrieval analytics

CREATE TABLE retrievals (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    principal_id uuid REFERENCES principals(id) ON DELETE SET NULL,
    channel text NOT NULL CHECK (channel IN ('http', 'mcp', 'hook', 'ci')),
    repository_id uuid REFERENCES repositories(id) ON DELETE SET NULL,
    task_fingerprint text NOT NULL,
    agent_session_id text,
    -- Only ids and scores: no derived content survives solution deletion.
    candidates jsonb NOT NULL DEFAULT '[]'::jsonb,
    result_count integer NOT NULL,
    latency_ms integer NOT NULL,
    context_tokens integer NOT NULL,
    degraded boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX retrievals_org_created_idx ON retrievals (organization_id, created_at DESC);
CREATE INDEX retrievals_agent_session_idx ON retrievals (organization_id, principal_id, agent_session_id);

CREATE TABLE outcomes (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    retrieval_id uuid REFERENCES retrievals(id) ON DELETE CASCADE,
    solution_id uuid NOT NULL REFERENCES solutions(id) ON DELETE CASCADE,
    principal_id uuid REFERENCES principals(id) ON DELETE SET NULL,
    outcome text NOT NULL CHECK (outcome IN ('used', 'rejected', 'inapplicable')),
    verification text NOT NULL DEFAULT 'none' CHECK (verification IN ('none', 'claimed', 'ci_verified')),
    repository_id uuid REFERENCES repositories(id) ON DELETE SET NULL,
    check_name text,
    check_revision text,
    check_url text,
    check_result text,
    notes text,
    created_at timestamptz NOT NULL DEFAULT now(),
    verified_at timestamptz
);
CREATE UNIQUE INDEX outcomes_retrieval_solution_idx ON outcomes (retrieval_id, solution_id) WHERE retrieval_id IS NOT NULL;
CREATE INDEX outcomes_check_idx ON outcomes (repository_id, check_revision) WHERE verification = 'claimed';

-- ---------------------------------------------------------------------------
-- CI events (Flow C) and webhook replay protection

CREATE TABLE webhook_deliveries (
    provider text NOT NULL,
    delivery_id text NOT NULL,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    event text NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, delivery_id)
);

CREATE TABLE ci_events (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    repository_id uuid NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    source text NOT NULL,
    revision text NOT NULL,
    check_name text NOT NULL,
    conclusion text NOT NULL,
    signature text NOT NULL,
    signature_hash text NOT NULL,
    excerpt text,
    url text,
    component_path text,
    retrieval_id uuid REFERENCES retrievals(id) ON DELETE SET NULL,
    match_count integer NOT NULL DEFAULT 0,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'searched', 'failed')),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (repository_id, revision, check_name, signature_hash)
);
CREATE INDEX ci_events_org_created_idx ON ci_events (organization_id, created_at DESC);

-- ---------------------------------------------------------------------------
-- Audit and jobs

CREATE TABLE audit_events (
    id bigserial PRIMARY KEY,
    organization_id uuid REFERENCES organizations(id) ON DELETE CASCADE,
    actor_principal_id uuid,
    actor_label text NOT NULL,
    action text NOT NULL,
    resource_type text NOT NULL,
    resource_id text,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX audit_events_org_created_idx ON audit_events (organization_id, created_at DESC);

CREATE TABLE jobs (
    id uuid PRIMARY KEY,
    organization_id uuid REFERENCES organizations(id) ON DELETE CASCADE,
    session_id uuid REFERENCES sessions(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('extract_session', 'embed_solution', 'process_git_event', 'ci_lookup', 'revalidate', 'retention_sweep')),
    dedupe_key text NOT NULL,
    payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    state text NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'running', 'succeeded', 'dead', 'cancelled')),
    attempts integer NOT NULL DEFAULT 0,
    max_attempts integer NOT NULL DEFAULT 8 CHECK (max_attempts BETWEEN 1 AND 50),
    run_at timestamptz NOT NULL DEFAULT now(),
    lease_owner text,
    lease_expires_at timestamptz,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz
);
CREATE UNIQUE INDEX jobs_active_dedupe_idx ON jobs (kind, dedupe_key) WHERE state IN ('queued', 'running');
CREATE INDEX jobs_ready_idx ON jobs (run_at) WHERE state = 'queued';
CREATE INDEX jobs_lease_idx ON jobs (lease_expires_at) WHERE state = 'running';
CREATE INDEX jobs_org_state_idx ON jobs (organization_id, state, updated_at DESC);
