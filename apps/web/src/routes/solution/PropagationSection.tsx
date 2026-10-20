import { Link } from 'react-router';
import { api } from '../../api/client';
import { unwrap } from '../../api/errors';
import { Mono } from '../../components/common';
import { QueryBoundary } from '../../components/Status';
import { useQuery } from '../../hooks/useQuery';
import { formatNumber } from '../../lib/format';
import { Section } from './Section';

/** Sibling locations that may benefit from this fix (informational only). */
export function PropagationSection({ solutionId }: { solutionId: string }) {
  const query = useQuery(`propagation:${solutionId}`, () =>
    unwrap(api.GET('/v1/solutions/{id}/propagation', { params: { path: { id: solutionId } } })),
  );
  return (
    <Section id="sol-propagation" title="May also apply to">
      <QueryBoundary
        query={query}
        label="sibling locations"
        forbiddenWhat="sibling locations for this solution"
        freshness={false}
      >
        {(list) => (
          <>
            <p className="hint">{list.notice}</p>
            {list.candidates.length === 0 ? (
              <p className="muted">No sibling locations found</p>
            ) : (
              <ul className="propagation-list">
                {list.candidates.map((c, i) => (
                  <li key={`${c.repository_id}-${c.component ?? ''}-${i}`}>
                    <p className="propagation__where">
                      <strong>{c.repository}</strong>
                      {c.component ? (
                        <>
                          {' '}
                          · <Mono>{c.component}</Mono>
                        </>
                      ) : null}
                    </p>
                    <p className="plain-text">{c.reason}</p>
                    <p className="result__meta">
                      <span>
                        {formatNumber(c.matching_ci_failures)} matching CI failure{c.matching_ci_failures === 1 ? '' : 's'}
                      </span>
                      {c.related_solution_id ? (
                        <span>
                          <Link to={`/knowledge/${c.related_solution_id}`}>Related solution</Link>
                        </span>
                      ) : null}
                    </p>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </QueryBoundary>
    </Section>
  );
}
