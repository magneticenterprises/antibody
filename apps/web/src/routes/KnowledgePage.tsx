import { useEffect, useState, type FormEvent } from 'react';
import { Link, useLocation, useSearchParams } from 'react-router';
import { api } from '../api/client';
import { unwrap } from '../api/errors';
import { SOLUTION_STATES, type SolutionState, type SolutionSummary } from '../api/types';
import { MergeBadge, RevalidationBadge, StateBadge } from '../components/Badge';
import { PageHeader, RelativeTime, TabNav, useFieldId } from '../components/common';
import { EmptyState, QueryBoundary } from '../components/Status';
import { useDocumentTitle } from '../hooks/misc';
import { useQuery } from '../hooks/useQuery';
import { formatNumber, humanize, isUuid } from '../lib/format';
import { SearchPreview } from './SearchPreview';

const PAGE_SIZE = 25;

function parseState(v: string | null): SolutionState | undefined {
  return SOLUTION_STATES.find((s) => s === v);
}

function parseBool(v: string | null): boolean | undefined {
  return v === 'true' ? true : v === 'false' ? false : undefined;
}

function SolutionRow({ s }: { s: SolutionSummary }) {
  return (
    <li className="result">
      <div className="result__main">
        <h2 className="result__title">
          <Link to={`/knowledge/${s.id}`}>{s.title}</Link>
        </h2>
        <p className="result__summary">{s.symptom}</p>
        <p className="result__meta">
          <span>{s.repository}</span>
          <span>v{s.version}</span>
          <span>
            {formatNumber(s.corroboration_count)} corroboration{s.corroboration_count === 1 ? '' : 's'}
          </span>
          <span>
            Updated <RelativeTime iso={s.updated_at} />
          </span>
        </p>
      </div>
      <div className="badges">
        <StateBadge state={s.state} />
        {s.needs_revalidation ? <RevalidationBadge /> : null}
        <MergeBadge state={s.merge_state} />
      </div>
    </li>
  );
}

function RepositoryFilter({ value, onChange, id }: { value: string; onChange: (v: string) => void; id: string }) {
  // Optional helper: if repositories cannot be loaded, the filter is hidden.
  const repos = useQuery('repositories', () => unwrap(api.GET('/v1/repositories')));
  if (!repos.data || repos.data.items.length === 0) return null;
  return (
    <div className="field">
      <label htmlFor={id}>Repository</label>
      <select id={id} value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">All repositories</option>
        {repos.data.items.map((r) => (
          <option key={r.id} value={r.id}>
            {r.full_name}
          </option>
        ))}
      </select>
    </div>
  );
}

function SolutionBrowser() {
  const id = useFieldId('kn');
  const [params, setParams] = useSearchParams();
  const q = params.get('q') ?? '';
  const state = parseState(params.get('state'));
  const needsRevalidation = parseBool(params.get('needs_revalidation'));
  const repoParam = params.get('repository_id') ?? '';
  const repositoryId = isUuid(repoParam) ? repoParam : undefined;
  const offset = Math.max(0, Number.parseInt(params.get('offset') ?? '0', 10) || 0);

  const [draft, setDraft] = useState(q);
  useEffect(() => setDraft(q), [q]);

  const update = (changes: Record<string, string | undefined>) => {
    const next = new URLSearchParams(params);
    for (const [k, v] of Object.entries(changes)) {
      if (v === undefined || v === '') next.delete(k);
      else next.set(k, v);
    }
    if (!('offset' in changes)) next.delete('offset');
    setParams(next);
  };

  const key = `solutions?q=${q}&state=${state ?? ''}&nr=${String(needsRevalidation)}&repo=${repositoryId ?? ''}&offset=${offset}`;
  const query = useQuery(key, () =>
    unwrap(
      api.GET('/v1/solutions', {
        params: {
          query: {
            q: q || undefined,
            state,
            needs_revalidation: needsRevalidation,
            repository_id: repositoryId,
            limit: PAGE_SIZE,
            offset,
          },
        },
      }),
    ),
  );

  const filtered = Boolean(q || state || needsRevalidation !== undefined || repositoryId);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    update({ q: draft.trim() });
  };

  return (
    <>
      <div role="search">
        <form className="form form--inline filters" onSubmit={submit}>
          <div className="field field--grow">
            <label htmlFor={`${id}-q`}>Search knowledge</label>
            <input
              id={`${id}-q`}
              type="search"
              placeholder="Error text, symbol, file, remedy…"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
            />
          </div>
          <div className="field">
            <label htmlFor={`${id}-state`}>State</label>
            <select id={`${id}-state`} value={state ?? ''} onChange={(e) => update({ state: e.target.value })}>
              <option value="">Any state</option>
              {SOLUTION_STATES.map((s) => (
                <option key={s} value={s}>
                  {humanize(s)}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor={`${id}-reval`}>Revalidation</label>
            <select
              id={`${id}-reval`}
              value={needsRevalidation === undefined ? '' : String(needsRevalidation)}
              onChange={(e) => update({ needs_revalidation: e.target.value })}
            >
              <option value="">Any</option>
              <option value="true">Needs revalidation</option>
              <option value="false">Up to date</option>
            </select>
          </div>
          <RepositoryFilter id={`${id}-repo`} value={repositoryId ?? ''} onChange={(v) => update({ repository_id: v })} />
          <div className="field field--action">
            <button type="submit" className="button button--primary">
              Search
            </button>
          </div>
        </form>
      </div>
      {filtered ? (
        <p className="hint">
          <button type="button" className="link-button" onClick={() => setParams(new URLSearchParams())}>
            Clear filters
          </button>
        </p>
      ) : null}
      <QueryBoundary
        query={query}
        label="solutions"
        isEmpty={(d) => d.items.length === 0}
        empty={
          filtered ? (
            <EmptyState title="No solutions match these filters.">
              <p>Try fewer words or a different state.</p>
            </EmptyState>
          ) : (
            <EmptyState title="No solutions yet.">
              <p>Solutions appear here after agent sessions are ingested and extracted.</p>
            </EmptyState>
          )
        }
      >
        {(d) => (
          <>
            <p className="result-count" role="status" aria-live="polite">
              {d.total === 1 ? '1 solution' : `${formatNumber(d.total)} solutions`}
              {d.total > PAGE_SIZE ? ` · showing ${offset + 1}–${Math.min(offset + d.items.length, d.total)}` : ''}
            </p>
            <ul className="results" aria-label="Solutions">
              {d.items.map((s) => (
                <SolutionRow key={s.id} s={s} />
              ))}
            </ul>
            {d.total > PAGE_SIZE ? (
              <nav className="pager" aria-label="Pagination">
                <button
                  type="button"
                  className="button"
                  disabled={offset === 0}
                  onClick={() => update({ offset: String(Math.max(0, offset - PAGE_SIZE)) })}
                >
                  Previous
                </button>
                <button
                  type="button"
                  className="button"
                  disabled={offset + PAGE_SIZE >= d.total}
                  onClick={() => update({ offset: String(offset + PAGE_SIZE) })}
                >
                  Next
                </button>
              </nav>
            ) : null}
          </>
        )}
      </QueryBoundary>
    </>
  );
}

export function KnowledgePage() {
  useDocumentTitle('Knowledge');
  const location = useLocation();
  const deleted = (location.state as { deleted?: string } | null)?.deleted;
  const [params] = useSearchParams();
  const view = params.get('view') === 'search' ? 'search' : 'browse';
  return (
    <>
      <PageHeader
        title="Knowledge"
        lede="Solutions extracted from agent sessions. Agent-provided text is untrusted until trusted evidence verifies it."
      />
      {deleted ? (
        <p className="notice notice--info" role="status">
          Deleted “{deleted}” permanently.
        </p>
      ) : null}
      <TabNav
        label="Knowledge views"
        active={view}
        items={[
          { key: 'browse', to: '/knowledge', label: 'Browse solutions' },
          { key: 'search', to: '/knowledge?view=search', label: 'Try a search' },
        ]}
      />
      {view === 'search' ? <SearchPreview /> : <SolutionBrowser />}
    </>
  );
}
