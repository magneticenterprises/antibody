import type { ReactNode } from 'react';
import { Link } from 'react-router';
import { api } from '../api/client';
import { unwrap } from '../api/errors';
import type { Overview, QueueStats } from '../api/types';
import { useSession } from '../auth/AuthContext';
import { PageHeader } from '../components/common';
import { EmptyState, QueryBoundary } from '../components/Status';
import { useDocumentTitle } from '../hooks/misc';
import { useQuery } from '../hooks/useQuery';
import { formatNumber, formatSeconds } from '../lib/format';

function Stat({ label, value, hint, to }: { label: string; value: ReactNode; hint?: string; to?: string }) {
  return (
    <div className="stat">
      <dt>{label}</dt>
      <dd>
        <span className="stat__value">{to ? <Link to={to}>{value}</Link> : value}</span>
        {hint ? <span className="stat__hint">{hint}</span> : null}
      </dd>
    </div>
  );
}

function StatGroup({ id, title, note, children }: { id: string; title: string; note?: string; children: ReactNode }) {
  return (
    <section className="stat-group" aria-labelledby={id}>
      <h2 id={id}>{title}</h2>
      {note ? <p className="hint">{note}</p> : null}
      <dl className="stats">{children}</dl>
    </section>
  );
}

function isEmptyOverview(s: Overview): boolean {
  return s.repositories === 0 && s.sessions_total === 0 && s.solutions_total === 0 && s.retrievals_7d === 0;
}

function QueueGroup({ queue }: { queue: QueueStats }) {
  return (
    <StatGroup id="ov-queue" title="Queue health" note="Background jobs (extraction, correlation, embeddings). Visible to administrators.">
      <Stat label="Queued" value={formatNumber(queue.queued)} />
      <Stat label="Running" value={formatNumber(queue.running)} />
      <Stat
        label="Dead (failed permanently)"
        value={formatNumber(queue.dead)}
        to={queue.dead > 0 ? '/activity/queue' : undefined}
        hint={queue.dead > 0 ? 'Review and retry' : undefined}
      />
      <Stat label="Oldest queued job" value={formatSeconds(queue.oldest_queued_age_seconds)} />
    </StatGroup>
  );
}

export function OverviewPage() {
  useDocumentTitle('Overview');
  const session = useSession();
  const query = useQuery('overview', () => unwrap(api.GET('/v1/overview')));

  return (
    <>
      <PageHeader title="Overview" lede={`What ${session.organization_name}'s agents have captured, reused and confirmed.`} />
      <QueryBoundary query={query} label="overview" forbiddenWhat="the overview">
        {({ stats: s, queue }) => (
          <>
            {isEmptyOverview(s) ? (
              <EmptyState title="Nothing captured yet.">
                <p>
                  Connect a repository, then let an agent integration upload sessions. Extracted solutions will appear in{' '}
                  <Link to="/knowledge">Knowledge</Link>.
                  {session.role === 'admin' ? (
                    <>
                      {' '}
                      Start on the <Link to="/repositories">Repositories</Link> page.
                    </>
                  ) : null}
                </p>
              </EmptyState>
            ) : null}
            <div className="stat-grid">
              <StatGroup id="ov-capture" title="Capture">
                <Stat label="Repositories" value={formatNumber(s.repositories)} to="/repositories" />
                <Stat label="Sessions (all time)" value={formatNumber(s.sessions_total)} />
                <Stat label="Sessions (last 7 days)" value={formatNumber(s.sessions_7d)} />
                <Stat label="Extraction pending" value={formatNumber(s.extraction_pending)} />
                <Stat
                  label="Extraction failed"
                  value={formatNumber(s.extraction_failed)}
                  to={s.extraction_failed > 0 ? '/activity/sessions?status=failed' : undefined}
                />
              </StatGroup>
              <StatGroup id="ov-knowledge" title="Solutions by state">
                <Stat label="Total" value={formatNumber(s.solutions_total)} to="/knowledge" />
                <Stat label="Candidate" value={formatNumber(s.solutions_candidate)} to="/knowledge?state=candidate" />
                <Stat label="Corroborated" value={formatNumber(s.solutions_corroborated)} to="/knowledge?state=corroborated" />
                <Stat label="Verified" value={formatNumber(s.solutions_verified)} to="/knowledge?state=verified" />
                <Stat
                  label="Needing revalidation"
                  value={formatNumber(s.solutions_needing_revalidation)}
                  to="/knowledge?needs_revalidation=true"
                />
              </StatGroup>
              <StatGroup id="ov-retrieval" title="Retrieval (last 7 days)">
                <Stat label="Retrievals" value={formatNumber(s.retrievals_7d)} to="/activity/retrievals" />
                <Stat label="Retrievals with results" value={formatNumber(s.retrievals_with_results_7d)} />
                <Stat label="Context tokens served" value={formatNumber(s.retrieval_context_tokens_7d)} />
                <Stat
                  label="p95 latency"
                  value={
                    s.retrieval_p95_latency_ms_7d === null || s.retrieval_p95_latency_ms_7d === undefined
                      ? '—'
                      : `${formatNumber(s.retrieval_p95_latency_ms_7d)} ms`
                  }
                />
              </StatGroup>
              <StatGroup
                id="ov-outcomes"
                title="Outcomes reported by agents"
                note="All time. Agent reports are claims; only the last figure is backed by signed CI."
              >
                <Stat label="Used" value={formatNumber(s.outcomes_used)} />
                <Stat label="Rejected" value={formatNumber(s.outcomes_rejected)} />
                <Stat label="Inapplicable" value={formatNumber(s.outcomes_inapplicable)} />
                <Stat
                  label="Used outcomes confirmed by signed CI"
                  value={formatNumber(s.confirmed_reuse)}
                  hint="A used outcome whose claimed check a signed CI event confirmed on the same revision"
                />
              </StatGroup>
              <StatGroup id="ov-ci" title="CI events (last 7 days)">
                <Stat label="CI failure events" value={formatNumber(s.ci_events_7d)} to="/activity/ci" />
                <Stat label="Events with matching solutions" value={formatNumber(s.ci_events_with_matches_7d)} />
              </StatGroup>
              {queue ? <QueueGroup queue={queue} /> : null}
            </div>
          </>
        )}
      </QueryBoundary>
    </>
  );
}
