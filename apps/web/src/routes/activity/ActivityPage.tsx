import { Link, useParams, useSearchParams } from 'react-router';
import { api } from '../../api/client';
import { unwrap } from '../../api/errors';
import { EXTRACTION_STATUSES, type ExtractionStatus } from '../../api/types';
import { useIsAdmin } from '../../auth/AuthContext';
import { Badge, ExtractionBadge } from '../../components/Badge';
import { ExternalLink, Mono, PageHeader, RelativeTime, TabNav, TableWrap, useFieldId } from '../../components/common';
import { EmptyState, Forbidden, QueryBoundary } from '../../components/Status';
import { useDocumentTitle } from '../../hooks/misc';
import { useQuery } from '../../hooks/useQuery';
import { formatNumber, humanize } from '../../lib/format';
import { QueueFailures } from './QueueFailures';

function SessionsTab() {
  const id = useFieldId('sessions');
  const [params, setParams] = useSearchParams();
  const status = EXTRACTION_STATUSES.find((s) => s === params.get('status')) as ExtractionStatus | undefined;
  const query = useQuery(`sessions:${status ?? ''}`, () =>
    unwrap(api.GET('/v1/sessions', { params: { query: { status, limit: 100 } } })),
  );
  return (
    <section aria-labelledby={`${id}-title`}>
      <h2 id={`${id}-title`}>Agent sessions</h2>
      <form className="form form--inline filters" onSubmit={(e) => e.preventDefault()}>
        <div className="field">
          <label htmlFor={`${id}-status`}>Extraction status</label>
          <select
            id={`${id}-status`}
            value={status ?? ''}
            onChange={(e) => setParams(e.target.value ? { status: e.target.value } : {})}
          >
            <option value="">All statuses</option>
            {EXTRACTION_STATUSES.map((s) => (
              <option key={s} value={s}>
                {humanize(s)}
              </option>
            ))}
          </select>
        </div>
      </form>
      <QueryBoundary
        query={query}
        label="sessions"
        isEmpty={(d) => d.items.length === 0}
        empty={
          <EmptyState title={status ? `No sessions with status “${status}”.` : 'No sessions ingested yet.'}>
            <p>Agent integrations upload sessions with an API token; extraction runs in the background.</p>
          </EmptyState>
        }
      >
        {(d) => (
          <TableWrap label="Session list">
            <table>
              <thead>
                <tr>
                  <th scope="col">Received</th>
                  <th scope="col">Repository</th>
                  <th scope="col">Agent</th>
                  <th scope="col">Extraction</th>
                  <th scope="col">Messages</th>
                  <th scope="col">Redactions</th>
                  <th scope="col">Solutions</th>
                  <th scope="col">
                    <span className="visually-hidden">Details</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {d.items.map((s) => (
                  <tr key={s.id}>
                    <td>
                      <RelativeTime iso={s.created_at} />
                    </td>
                    <td>{s.repository}</td>
                    <td>{humanize(s.agent_type)}</td>
                    <td>
                      <ExtractionBadge status={s.extraction_status} />
                      {s.extraction_error ? <p className="cell-error">{s.extraction_error}</p> : null}
                    </td>
                    <td>{formatNumber(s.message_count)}</td>
                    <td>{formatNumber(s.redaction_count)}</td>
                    <td>{formatNumber(s.solution_ids.length)}</td>
                    <td>
                      <Link to={`/activity/sessions/${s.id}`} aria-label={`Session details ${s.id.slice(0, 8)} (${s.repository})`}>
                        Details
                      </Link>
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

function CiEventsTab() {
  const query = useQuery('ci-events', () => unwrap(api.GET('/v1/activity/ci-events', { params: { query: { limit: 100 } } })));
  return (
    <section aria-labelledby="ci-title">
      <h2 id="ci-title">CI failure events</h2>
      <p className="hint">Received through the signed CI webhook. Excerpts are redacted, untrusted log text.</p>
      <QueryBoundary
        query={query}
        label="CI events"
        isEmpty={(d) => d.length === 0}
        empty={
          <EmptyState title="No CI events yet.">
            <p>Configure your CI to post failures to the signed webhook shown in Settings.</p>
          </EmptyState>
        }
      >
        {(rows) => (
          <ul className="card-list">
            {rows.map((e) => (
              <li key={e.id} className="card">
                <header className="card__header">
                  <h3 className="card__title">
                    <Mono>{e.check_name}</Mono> <span className="muted">in {e.repository}</span>
                  </h3>
                  <div className="badges">
                    <Badge tone={e.conclusion === 'success' ? 'ok' : 'bad'}>{humanize(e.conclusion)}</Badge>
                    <Badge tone="neutral">{humanize(e.status)}</Badge>
                  </div>
                </header>
                <dl className="meta">
                  <div>
                    <dt>Received</dt>
                    <dd>
                      <RelativeTime iso={e.created_at} />
                    </dd>
                  </div>
                  <div>
                    <dt>Revision</dt>
                    <dd>
                      <Mono short={12}>{e.revision}</Mono>
                    </dd>
                  </div>
                  <div>
                    <dt>Source</dt>
                    <dd>{e.source}</dd>
                  </div>
                  <div>
                    <dt>Signature</dt>
                    <dd>
                      <Mono>{e.signature}</Mono>
                    </dd>
                  </div>
                  <div>
                    <dt>Matching solutions</dt>
                    <dd>
                      {e.suggested_solution_ids.length === 0 ? (
                        '0'
                      ) : (
                        <ul className="inline-list">
                          {e.suggested_solution_ids.map((sid) => (
                            <li key={sid}>
                              <Link to={`/knowledge/${sid}`}>
                                <Mono short={8}>{sid}</Mono>
                              </Link>
                            </li>
                          ))}
                        </ul>
                      )}
                    </dd>
                  </div>
                  {e.url ? (
                    <div>
                      <dt>Run</dt>
                      <dd>
                        <ExternalLink href={e.url}>Open CI run</ExternalLink>
                      </dd>
                    </div>
                  ) : null}
                </dl>
                {e.excerpt ? (
                  <details className="disclosure">
                    <summary>Log excerpt (untrusted)</summary>
                    <pre className="excerpt">{e.excerpt}</pre>
                  </details>
                ) : null}
              </li>
            ))}
          </ul>
        )}
      </QueryBoundary>
    </section>
  );
}

function RetrievalsTab() {
  const query = useQuery('retrievals', () => unwrap(api.GET('/v1/activity/retrievals', { params: { query: { limit: 100 } } })));
  return (
    <section aria-labelledby="ret-title">
      <h2 id="ret-title">Retrievals and outcomes</h2>
      <p className="hint">Each search an agent ran, with the outcomes agents reported for its suggestions. Dashboard previews are not recorded.</p>
      <QueryBoundary
        query={query}
        label="retrievals"
        isEmpty={(d) => d.length === 0}
        empty={<EmptyState title="No retrievals yet." />}
      >
        {(rows) => (
          <TableWrap label="Retrievals">
            <table>
              <thead>
                <tr>
                  <th scope="col">When</th>
                  <th scope="col">Channel</th>
                  <th scope="col">Repository</th>
                  <th scope="col">Principal</th>
                  <th scope="col">Results</th>
                  <th scope="col">Latency</th>
                  <th scope="col">Context tokens</th>
                  <th scope="col">Quality</th>
                  <th scope="col">Used</th>
                  <th scope="col">Rejected</th>
                  <th scope="col">Inapplicable</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((r) => (
                  <tr key={r.id}>
                    <td>
                      <RelativeTime iso={r.created_at} />
                    </td>
                    <td>{humanize(r.channel)}</td>
                    <td>{r.repository ?? '—'}</td>
                    <td>{r.principal ?? '—'}</td>
                    <td>{formatNumber(r.result_count)}</td>
                    <td>{formatNumber(r.latency_ms)} ms</td>
                    <td>{formatNumber(r.context_tokens)}</td>
                    <td>{r.degraded ? <Badge tone="warn">Degraded</Badge> : <Badge tone="ok">Complete</Badge>}</td>
                    <td>{formatNumber(r.outcomes_used)}</td>
                    <td>{formatNumber(r.outcomes_rejected)}</td>
                    <td>{formatNumber(r.outcomes_inapplicable)}</td>
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

const TABS = ['sessions', 'ci', 'retrievals', 'queue'] as const;
type Tab = (typeof TABS)[number];

export function ActivityPage() {
  useDocumentTitle('Activity');
  const isAdmin = useIsAdmin();
  const { tab: tabParam } = useParams();
  const tab: Tab | undefined = tabParam === undefined ? 'sessions' : TABS.find((t) => t === tabParam);
  const items = [
    { key: 'sessions', to: '/activity/sessions', label: 'Sessions' },
    { key: 'ci', to: '/activity/ci', label: 'CI events' },
    { key: 'retrievals', to: '/activity/retrievals', label: 'Retrievals & outcomes' },
    ...(isAdmin ? [{ key: 'queue', to: '/activity/queue', label: 'Queue failures' }] : []),
  ];
  return (
    <>
      <PageHeader title="Activity" lede="What agents and CI sent recently, and how extraction and retrieval went." />
      <TabNav label="Activity views" active={tab ?? ''} items={items} />
      {tab === 'sessions' ? <SessionsTab /> : null}
      {tab === 'ci' ? <CiEventsTab /> : null}
      {tab === 'retrievals' ? <RetrievalsTab /> : null}
      {tab === 'queue' ? isAdmin ? <QueueFailures /> : <Forbidden what="the job queue" /> : null}
      {tab === undefined ? <EmptyState title="Unknown activity view." /> : null}
    </>
  );
}
