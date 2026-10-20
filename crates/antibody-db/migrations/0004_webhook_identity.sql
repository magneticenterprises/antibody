-- Retain only authenticated-body fingerprints permanently; delivery diagnostics
-- may still expire. GitHub signatures do not include an expiry timestamp.
CREATE TABLE webhook_payloads (
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    provider text NOT NULL,
    payload_hash text NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, provider, payload_hash)
);

-- Delivery ids are provider metadata, scoped to the receiving organization.
ALTER TABLE webhook_deliveries DROP CONSTRAINT webhook_deliveries_pkey;
ALTER TABLE webhook_deliveries ADD PRIMARY KEY (organization_id, provider, delivery_id);

ALTER TABLE repositories ADD COLUMN disconnected_reason text;
ALTER TABLE repositories ADD COLUMN provider_updated_at timestamptz;
ALTER TABLE repositories ADD COLUMN lifecycle_received_at timestamptz;
-- Existing disconnected rows must require an administrator to reconnect them.
UPDATE repositories SET disconnected_reason = 'admin' WHERE status = 'disconnected';

-- Old deployments did not enforce provider identity uniqueness. Leave ambiguous
-- identities unbound so an administrator or a signed name match can resolve them.
UPDATE repositories SET provider_repo_id = NULL
WHERE (organization_id, provider, provider_repo_id) IN (
    SELECT organization_id, provider, provider_repo_id FROM repositories
    WHERE provider_repo_id IS NOT NULL
    GROUP BY organization_id, provider, provider_repo_id HAVING count(*) > 1
);
CREATE UNIQUE INDEX repositories_provider_identity_idx
    ON repositories (organization_id, provider, provider_repo_id)
    WHERE provider_repo_id IS NOT NULL;
