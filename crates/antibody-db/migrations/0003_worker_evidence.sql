-- Previous workers could correlate dirty session base revisions or mutable
-- PR links with a passing check. Keep the signed historical observations,
-- but require review where committed fix provenance can no longer be proved.
-- Manual verification remains an independent, audited verification source.
UPDATE solutions s
   SET needs_revalidation = true,
       revalidation_reason = 'legacy CI evidence requires review of committed fix provenance',
       updated_at = now()
 WHERE s.state = 'verified' AND NOT s.needs_revalidation
   AND EXISTS (
       SELECT 1 FROM evidence e WHERE e.solution_id = s.id
        AND e.trusted AND e.kind = 'ci_check' AND e.result = 'pass')
   AND NOT EXISTS (
       SELECT 1 FROM evidence e WHERE e.solution_id = s.id
        AND e.trusted AND e.kind = 'manual_verification' AND e.result = 'pass')
   AND (s.fix_revision IS NULL OR EXISTS (
       SELECT 1 FROM solution_sources ss
       JOIN source_artifacts a ON a.id = ss.source_artifact_id
        WHERE ss.solution_id = s.id AND ss.relation = 'pull_request'
          AND a.revision IS DISTINCT FROM s.fix_revision
          AND a.metadata->>'head_sha' IS DISTINCT FROM s.fix_revision));
