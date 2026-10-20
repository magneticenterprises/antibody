import { useState } from 'react';
import { Link } from 'react-router';
import { api } from '../../api/client';
import { unwrap, unwrapVoid } from '../../api/errors';
import type { JobView } from '../../api/types';
import { Mono, RelativeTime, TableWrap } from '../../components/common';
import { ActionStatus, EmptyState, QueryBoundary } from '../../components/Status';
import { useAction } from '../../hooks/misc';
import { useQuery } from '../../hooks/useQuery';
import { humanize } from '../../lib/format';

/** Dead jobs (exhausted retries) with a manual retry; administrators only. */
export function QueueFailures() {
  const query = useQuery('jobs:dead', () => unwrap(api.GET('/v1/admin/jobs', { params: { query: { state: 'dead', limit: 100 } } })));
  const [done, setDone] = useState<string | undefined>();
  const retry = useAction(async (job: JobView) => {
    await unwrapVoid(api.POST('/v1/admin/jobs/{id}/retry', { params: { path: { id: job.id } } }));
    return job;
  });

  return (
    <section aria-labelledby="queue-title">
      <h2 id="queue-title">Queue failures</h2>
      <p className="hint">Jobs that failed on every attempt. Retrying puts a job back in the queue; the worker picks it up.</p>
      <ActionStatus pending={retry.pending} error={retry.error} success={done} pendingText="Retrying…" />
      <QueryBoundary
        query={query}
        label="failed jobs"
        forbiddenWhat="the job queue"
        isEmpty={(d) => d.length === 0}
        empty={<EmptyState title="No failed jobs. The queue is healthy." />}
      >
        {(rows) => (
          <TableWrap label="Failed jobs">
            <table>
              <thead>
                <tr>
                  <th scope="col">Kind</th>
                  <th scope="col">Attempts</th>
                  <th scope="col">Last error</th>
                  <th scope="col">Session</th>
                  <th scope="col">Updated</th>
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {rows.map((j) => (
                  <tr key={j.id}>
                    <td>{humanize(j.kind)}</td>
                    <td>
                      {j.attempts} / {j.max_attempts}
                    </td>
                    <td className="cell--text">{j.last_error ? <span className="cell-error">{j.last_error}</span> : '—'}</td>
                    <td>
                      {j.session_id ? (
                        <Link to={`/activity/sessions/${j.session_id}`}>
                          <Mono short={8}>{j.session_id}</Mono>
                        </Link>
                      ) : (
                        '—'
                      )}
                    </td>
                    <td>
                      <RelativeTime iso={j.updated_at} />
                    </td>
                    <td>
                      <button
                        type="button"
                        className="button"
                        disabled={retry.pending}
                        aria-label={`Retry ${j.kind} job ${j.id.slice(0, 8)}`}
                        onClick={() => {
                          setDone(undefined);
                          void retry.run(j).then((ok) => {
                            if (ok) {
                              setDone(`Queued ${j.kind} job ${j.id.slice(0, 8)} for retry.`);
                              query.refetch();
                            }
                          });
                        }}
                      >
                        Retry
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </TableWrap>
        )}
      </QueryBoundary>
    </section>
  );
}
