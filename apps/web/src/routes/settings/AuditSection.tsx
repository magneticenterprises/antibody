import { useEffect, useState, type FormEvent } from 'react';
import { useSearchParams } from 'react-router';
import { api } from '../../api/client';
import { unwrap } from '../../api/errors';
import { Mono, RelativeTime, TableWrap, useFieldId } from '../../components/common';
import { EmptyState, QueryBoundary } from '../../components/Status';
import { useQuery } from '../../hooks/useQuery';
import { prettyJson } from '../../lib/format';

const PAGE_SIZE = 50;
const ACTION_RE = /^[A-Za-z0-9._]{0,64}$/;

export function AuditSection() {
  const id = useFieldId('audit');
  const [params, setParams] = useSearchParams();
  const action = params.get('action') ?? '';
  const beforeRaw = params.get('before_id');
  const beforeId = beforeRaw && /^\d+$/.test(beforeRaw) ? Number(beforeRaw) : undefined;
  const [draft, setDraft] = useState(action);
  useEffect(() => setDraft(action), [action]);
  const draftValid = ACTION_RE.test(draft.trim());

  const query = useQuery(`audit:${action}:${beforeId ?? ''}`, () =>
    unwrap(
      api.GET('/v1/admin/audit', {
        params: { query: { action: action || undefined, before_id: beforeId, limit: PAGE_SIZE } },
      }),
    ),
  );

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!draftValid) return;
    const next = new URLSearchParams();
    if (draft.trim()) next.set('action', draft.trim());
    setParams(next);
  };

  const goOlder = (lastId: number) => {
    const next = new URLSearchParams(params);
    next.set('before_id', String(lastId));
    setParams(next);
  };
  const goNewest = () => {
    const next = new URLSearchParams(params);
    next.delete('before_id');
    setParams(next);
  };

  return (
    <section className="settings-section" aria-labelledby={`${id}-title`}>
      <h2 id={`${id}-title`}>Audit log</h2>
      <form className="form form--inline filters" onSubmit={submit}>
        <div className="field field--grow">
          <label htmlFor={`${id}-action`}>Action prefix</label>
          <input
            id={`${id}-action`}
            placeholder="session.  token.  grant.  solution."
            spellCheck={false}
            autoCapitalize="none"
            aria-invalid={!draftValid}
            aria-describedby={`${id}-action-hint`}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          <p id={`${id}-action-hint`} className={draftValid ? 'hint' : 'hint hint--error'}>
            {draftValid ? 'Letters, digits, dots and underscores, e.g. session.' : 'Only letters, digits, “.” and “_” (max 64).'}
          </p>
        </div>
        <div className="field field--action">
          <button type="submit" className="button button--primary" disabled={!draftValid}>
            Filter
          </button>
        </div>
      </form>
      <QueryBoundary
        query={query}
        label="audit events"
        isEmpty={(d) => d.length === 0}
        empty={
          <EmptyState title={beforeId ? 'No older events.' : action ? `No events starting with “${action}”.` : 'No audit events.'}>
            {beforeId ? (
              <button type="button" className="button" onClick={goNewest}>
                Back to newest
              </button>
            ) : null}
          </EmptyState>
        }
      >
        {(rows) => {
          const last = rows[rows.length - 1];
          return (
            <>
              <TableWrap label="Audit events">
                <table>
                  <thead>
                    <tr>
                      <th scope="col">#</th>
                      <th scope="col">When</th>
                      <th scope="col">Actor</th>
                      <th scope="col">Action</th>
                      <th scope="col">Resource</th>
                      <th scope="col">Details</th>
                    </tr>
                  </thead>
                  <tbody>
                    {rows.map((ev) => (
                      <tr key={ev.id}>
                        <td>{ev.id}</td>
                        <td>
                          <RelativeTime iso={ev.created_at} />
                        </td>
                        <td>{ev.actor_label}</td>
                        <td>
                          <Mono>{ev.action}</Mono>
                        </td>
                        <td>
                          {ev.resource_type}
                          {ev.resource_id ? (
                            <>
                              {' '}
                              <Mono short={8}>{ev.resource_id}</Mono>
                            </>
                          ) : null}
                        </td>
                        <td className="cell--text">
                          {ev.metadata && typeof ev.metadata === 'object' && Object.keys(ev.metadata as object).length > 0 ? (
                            <details>
                              <summary>Metadata</summary>
                              <pre className="excerpt">{prettyJson(ev.metadata)}</pre>
                            </details>
                          ) : (
                            '—'
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </TableWrap>
              <nav className="pager" aria-label="Audit log pages">
                <button type="button" className="button" disabled={beforeId === undefined} onClick={goNewest}>
                  Newest
                </button>
                <button
                  type="button"
                  className="button"
                  disabled={!last || rows.length < PAGE_SIZE}
                  onClick={() => last && goOlder(last.id)}
                >
                  Older
                </button>
              </nav>
            </>
          );
        }}
      </QueryBoundary>
    </section>
  );
}
