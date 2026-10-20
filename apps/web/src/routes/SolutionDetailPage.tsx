import { Link, useParams } from 'react-router';
import { api } from '../api/client';
import { unwrap } from '../api/errors';
import type { SolutionDetail } from '../api/types';
import { Badge, MergeBadge, RevalidationBadge, StateBadge } from '../components/Badge';
import { ExternalLink, Mono, PageHeader, PlainText, RelativeTime, TableWrap } from '../components/common';
import { EmptyState, QueryBoundary } from '../components/Status';
import { useDocumentTitle } from '../hooks/misc';
import { useQuery } from '../hooks/useQuery';
import { formatDate, formatNumber, humanize, isUuid } from '../lib/format';
import { AdminActions } from './solution/AdminActions';
import { EvidenceSection } from './solution/EvidenceSection';
import { PropagationSection } from './solution/PropagationSection';
import { Section, TagList } from './solution/Section';

export { EvidenceSection, PropagationSection };

function SolutionView({ s, onChanged }: { s: SolutionDetail; onChanged: (s: SolutionDetail) => void }) {
  const v = s.verification;
  const checks = Array.from(new Set(s.evidence.map((e) => e.check_name).filter((n): n is string => Boolean(n))));
  const a = s.applicability;
  return (
    <article className="detail">
      <div className="badges badges--lead">
        <StateBadge state={v.state} />
        {v.needs_revalidation ? <RevalidationBadge /> : null}
        <MergeBadge state={v.merge_state} />
        <Badge tone={v.deployment_state === 'deployed' ? 'info' : 'muted'}>{`Deployment: ${v.deployment_state}`}</Badge>
      </div>
      <p className="notice notice--info">{s.notice}</p>

      <dl className="meta">
        <div>
          <dt>Repository</dt>
          <dd>{s.repository}</dd>
        </div>
        <div>
          <dt>Component</dt>
          <dd>{s.component ?? '—'}</dd>
        </div>
        <div>
          <dt>Version</dt>
          <dd>v{s.version}</dd>
        </div>
        <div>
          <dt>Confidence</dt>
          <dd>{formatNumber(s.confidence * 100)}%</dd>
        </div>
        <div>
          <dt>Corroborations</dt>
          <dd>{formatNumber(s.corroboration_count)}</dd>
        </div>
        <div>
          <dt>Extractor</dt>
          <dd>
            <Mono>{s.extractor}</Mono>
          </dd>
        </div>
        <div>
          <dt>Updated</dt>
          <dd>
            <RelativeTime iso={s.updated_at} />
          </dd>
        </div>
      </dl>

      <Section id="sol-verification" title="Verification">
        <p className="lead-text">{v.summary}</p>
        <dl className="pairs">
          <div>
            <dt>Verification state</dt>
            <dd>
              <StateBadge state={v.state} />
            </dd>
          </div>
          <div>
            <dt>Trusted evidence</dt>
            <dd>{formatNumber(v.trusted_evidence_count)}</dd>
          </div>
          <div>
            <dt>Last verified</dt>
            <dd>{v.last_verified_at ? formatDate(v.last_verified_at) : 'Never'}</dd>
          </div>
          <div>
            <dt>Merge state</dt>
            <dd>{humanize(v.merge_state)}</dd>
          </div>
          <div>
            <dt>Deployment state</dt>
            <dd>{humanize(v.deployment_state)}</dd>
          </div>
          {v.needs_revalidation ? (
            <div>
              <dt>Needs revalidation</dt>
              <dd className="plain-text">{v.revalidation_reason ?? 'Yes (no reason recorded)'}</dd>
            </div>
          ) : null}
        </dl>
        <p className="hint">Merge and deployment are tracked separately and never imply verification.</p>
      </Section>

      <Section id="sol-symptom" title="Symptom">
        <PlainText>{s.symptom}</PlainText>
      </Section>
      <Section id="sol-root" title="Root cause">
        <PlainText>{s.root_cause}</PlainText>
      </Section>
      <Section id="sol-remedy" title="Remedy">
        <PlainText>{s.remedy}</PlainText>
      </Section>

      <Section id="sol-signals" title="Signals">
        <h3>Error signatures</h3>
        <TagList items={s.error_signatures} mono empty="No error signatures." />
        <h3>Symbols</h3>
        <TagList items={s.symbols} mono empty="No symbols." />
        <h3>Paths</h3>
        <TagList items={s.paths} mono empty="No paths." />
        <h3>Tests and checks</h3>
        <TagList items={checks} mono empty="No test or check names in the evidence." />
      </Section>

      <Section id="sol-applicability" title="Applicability">
        <h3>Dependencies</h3>
        {(a.dependencies ?? []).length === 0 ? (
          <p className="muted">No dependency constraints.</p>
        ) : (
          <TableWrap label="Dependency constraints">
            <table>
              <thead>
                <tr>
                  <th scope="col">Package</th>
                  <th scope="col">Ecosystem</th>
                  <th scope="col">Version requirement</th>
                </tr>
              </thead>
              <tbody>
                {(a.dependencies ?? []).map((d) => (
                  <tr key={`${d.ecosystem ?? ''}:${d.name}`}>
                    <td>
                      <Mono>{d.name}</Mono>
                    </td>
                    <td>{d.ecosystem ?? '—'}</td>
                    <td>
                      <Mono>{d.version_req}</Mono>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </TableWrap>
        )}
        <h3>Components</h3>
        <TagList items={a.components ?? []} mono empty="Any component." />
        <h3>Platforms</h3>
        <TagList items={a.platforms ?? []} empty="Any platform." />
        <h3>Conditions</h3>
        <p className="hint">Free text from extraction (untrusted).</p>
        <PlainText>{a.conditions}</PlainText>
      </Section>

      <Section id="sol-fix" title="Fix">
        <dl className="pairs">
          <div>
            <dt>Fix revision</dt>
            <dd>{s.fix_revision ? <Mono>{s.fix_revision}</Mono> : '—'}</dd>
          </div>
          <div>
            <dt>Fix branch</dt>
            <dd>{s.fix_branch ? <Mono>{s.fix_branch}</Mono> : '—'}</dd>
          </div>
          {s.superseded_by ? (
            <div>
              <dt>Superseded by</dt>
              <dd>
                <Link to={`/knowledge/${s.superseded_by}`}>
                  <Mono>{s.superseded_by}</Mono>
                </Link>
              </dd>
            </div>
          ) : null}
        </dl>
      </Section>

      <Section id="sol-sources" title="Sources">
        {s.sources.length === 0 ? (
          <p className="muted">No source links.</p>
        ) : (
          <ul className="source-list">
            {s.sources.map((l, i) => (
              <li key={`${l.kind}-${l.label}-${i}`}>
                <Badge tone="neutral">{humanize(l.kind)}</Badge> <ExternalLink href={l.url}>{l.label}</ExternalLink>
                {l.revision ? (
                  <>
                    {' '}
                    <Mono short={12}>{l.revision}</Mono>
                  </>
                ) : null}
                {l.state ? <span className="muted"> · {l.state}</span> : null}
              </li>
            ))}
          </ul>
        )}
        {s.session_ids.length > 0 ? (
          <>
            <h3>Agent sessions</h3>
            <ul className="tag-list">
              {s.session_ids.map((sid) => (
                <li key={sid}>
                  <Link to={`/activity/sessions/${sid}`}>
                    Session <Mono short={8}>{sid}</Mono>
                  </Link>
                </li>
              ))}
            </ul>
          </>
        ) : null}
      </Section>

      <EvidenceSection evidence={s.evidence} />

      <PropagationSection solutionId={s.id} />

      <Section id="sol-versions" title="Other versions">
        {s.other_versions.length === 0 ? (
          <p className="muted">This is the only version.</p>
        ) : (
          <ul className="tag-list">
            {s.other_versions.map((o) => (
              <li key={o.id}>
                <Link to={`/knowledge/${o.id}`}>Version {o.version}</Link> <StateBadge state={o.state} />
              </li>
            ))}
          </ul>
        )}
      </Section>

      <AdminActions solution={s} onChanged={onChanged} />
    </article>
  );
}

export function SolutionDetailPage() {
  const { id = '' } = useParams();
  const valid = isUuid(id);
  const query = useQuery(valid ? `solution:${id}` : null, () =>
    unwrap(api.GET('/v1/solutions/{id}', { params: { path: { id } } })),
  );
  useDocumentTitle(query.data?.title ?? 'Solution');

  return (
    <>
      <p className="breadcrumb">
        <Link to="/knowledge">← Knowledge</Link>
      </p>
      <PageHeader title={query.data?.title ?? 'Solution'} />
      {!valid ? (
        <EmptyState title="Solution not found.">
          <p>The address does not contain a valid solution ID.</p>
        </EmptyState>
      ) : (
        <QueryBoundary query={query} label="solution" forbiddenWhat="this solution">
          {(s) => <SolutionView s={s} onChanged={query.setData} />}
        </QueryBoundary>
      )}
    </>
  );
}
