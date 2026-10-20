-- Serialize the repair with existing writers before enforcing version identity.
LOCK TABLE solutions IN SHARE ROW EXCLUSIVE MODE;

-- Older concurrent extractions could allocate the same version for distinct
-- applicability variants. Keep the oldest member's version and assign only
-- duplicates new versions above the lineage's existing maximum. Preserve IDs,
-- fingerprints, links, and every already-unique version.
WITH ranked AS (
    SELECT id, organization_id, repository_id, lineage_key, version, created_at,
           row_number() OVER (
               PARTITION BY organization_id, repository_id, lineage_key, version
               ORDER BY created_at, id
           ) AS duplicate_position,
           max(version) OVER (
               PARTITION BY organization_id, repository_id, lineage_key
           ) AS maximum_version
    FROM solutions
), repaired AS (
    SELECT id, maximum_version + row_number() OVER (
               PARTITION BY organization_id, repository_id, lineage_key
               ORDER BY version, created_at, id
           ) AS repaired_version
    FROM ranked
    WHERE duplicate_position > 1
)
UPDATE solutions s
SET version = r.repaired_version
FROM repaired r
WHERE s.id = r.id;

ALTER TABLE solutions
    ADD CONSTRAINT solutions_lineage_version_key
    UNIQUE (organization_id, repository_id, lineage_key, version);
