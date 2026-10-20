import { useState, type FormEvent } from 'react';
import { Link } from 'react-router';
import { api } from '../api/client';
import { unwrap, unwrapVoid } from '../api/errors';
import { REPOSITORY_PROVIDERS, type CreateRepository, type RepositoryProvider, type RepositoryView } from '../api/types';
import { useIsAdmin } from '../auth/AuthContext';
import { Badge } from '../components/Badge';
import { ExternalLink, Mono, PageHeader, RelativeTime, TableWrap, useFieldId } from '../components/common';
import { ConfirmAction } from '../components/ConfirmAction';
import { ActionStatus, EmptyState, QueryBoundary } from '../components/Status';
import { useAction, useDocumentTitle } from '../hooks/misc';
import { useQuery } from '../hooks/useQuery';
import { formatNumber, humanize } from '../lib/format';

function blankToNull(v: string): string | null {
  const t = v.trim();
  return t ? t : null;
}

function AddRepositoryForm({ onAdded }: { onAdded: () => void }) {
  const id = useFieldId('repo');
  const [provider, setProvider] = useState<RepositoryProvider>('github');
  const [fullName, setFullName] = useState('');
  const [url, setUrl] = useState('');
  const [branch, setBranch] = useState('');
  const action = useAction((body: CreateRepository) => unwrap(api.POST('/v1/repositories', { body })));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const repo = await action.run({
      provider,
      full_name: fullName.trim(),
      url: blankToNull(url),
      default_branch: blankToNull(branch),
    });
    if (repo) {
      setFullName('');
      setUrl('');
      setBranch('');
      onAdded();
    }
  };

  return (
    <section className="panel" aria-labelledby={`${id}-title`}>
      <h2 id={`${id}-title`}>Add a repository</h2>
      <form className="form form--inline" onSubmit={submit}>
        <div className="field">
          <label htmlFor={`${id}-provider`}>Provider</label>
          <select id={`${id}-provider`} value={provider} onChange={(e) => setProvider(e.target.value as RepositoryProvider)}>
            {REPOSITORY_PROVIDERS.map((p) => (
              <option key={p} value={p}>
                {humanize(p)}
              </option>
            ))}
          </select>
        </div>
        <div className="field">
          <label htmlFor={`${id}-name`}>Full name</label>
          <input
            id={`${id}-name`}
            required
            placeholder="owner/name"
            pattern="[^/\s]+/[^\s]+"
            title="owner/name, e.g. acme/api"
            spellCheck={false}
            autoCapitalize="none"
            value={fullName}
            onChange={(e) => setFullName(e.target.value)}
          />
        </div>
        <div className="field">
          <label htmlFor={`${id}-url`}>
            URL <span className="optional">(optional)</span>
          </label>
          <input id={`${id}-url`} type="url" placeholder="https://…" value={url} onChange={(e) => setUrl(e.target.value)} />
        </div>
        <div className="field">
          <label htmlFor={`${id}-branch`}>
            Default branch <span className="optional">(optional)</span>
          </label>
          <input id={`${id}-branch`} placeholder="main" value={branch} onChange={(e) => setBranch(e.target.value)} />
        </div>
        <div className="field field--action">
          <button type="submit" className="button button--primary" disabled={action.pending}>
            Add repository
          </button>
        </div>
      </form>
      <ActionStatus
        pending={action.pending}
        error={action.error}
        success={action.result ? `Saved ${action.result.full_name}.` : undefined}
      />
    </section>
  );
}

function AddComponentForm({ repo, onAdded }: { repo: RepositoryView; onAdded: () => void }) {
  const id = useFieldId('comp');
  const [name, setName] = useState('');
  const [glob, setGlob] = useState('');
  const [pkg, setPkg] = useState('');
  const [ecosystem, setEcosystem] = useState('');
  const action = useAction(() =>
    unwrap(
      api.POST('/v1/repositories/{id}/components', {
        params: { path: { id: repo.id } },
        body: { name: name.trim(), path_glob: glob.trim(), package_name: blankToNull(pkg), ecosystem: blankToNull(ecosystem) },
      }),
    ),
  );
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (await action.run()) {
      setName('');
      setGlob('');
      setPkg('');
      setEcosystem('');
      onAdded();
    }
  };
  return (
    <details className="disclosure">
      <summary>Add a component</summary>
      <form className="form form--inline" onSubmit={submit}>
        <div className="field">
          <label htmlFor={`${id}-name`}>Name</label>
          <input id={`${id}-name`} required maxLength={200} value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="field">
          <label htmlFor={`${id}-glob`}>Path glob</label>
          <input
            id={`${id}-glob`}
            required
            maxLength={500}
            placeholder="services/api/**"
            spellCheck={false}
            value={glob}
            onChange={(e) => setGlob(e.target.value)}
          />
        </div>
        <div className="field">
          <label htmlFor={`${id}-pkg`}>
            Package <span className="optional">(optional)</span>
          </label>
          <input id={`${id}-pkg`} value={pkg} onChange={(e) => setPkg(e.target.value)} />
        </div>
        <div className="field">
          <label htmlFor={`${id}-eco`}>
            Ecosystem <span className="optional">(optional)</span>
          </label>
          <input id={`${id}-eco`} placeholder="cargo, npm, pypi…" value={ecosystem} onChange={(e) => setEcosystem(e.target.value)} />
        </div>
        <div className="field field--action">
          <button type="submit" className="button" disabled={action.pending}>
            Add component
          </button>
        </div>
      </form>
      <ActionStatus
        pending={action.pending}
        error={action.error}
        success={action.result ? `Added component ${action.result.name}.` : undefined}
      />
    </details>
  );
}

function StatusControls({ repo, onChanged }: { repo: RepositoryView; onChanged: () => void }) {
  const action = useAction(async (status: 'active' | 'disconnected') => {
    await unwrapVoid(api.PATCH('/v1/repositories/{id}', { params: { path: { id: repo.id } }, body: { status } }));
    return true;
  });
  const [done, setDone] = useState<string | undefined>();
  const change = async (status: 'active' | 'disconnected') => {
    setDone(undefined);
    if (await action.run(status)) {
      setDone(status === 'active' ? `Reconnected ${repo.full_name}.` : `Disconnected ${repo.full_name}.`);
      onChanged();
    }
  };
  return (
    <div className="repo__admin">
      {repo.status === 'disconnected' ? (
        <button type="button" className="button" disabled={action.pending} onClick={() => void change('active')}>
          Reconnect
        </button>
      ) : (
        <ConfirmAction
          label="Disconnect"
          title={`Disconnect ${repo.full_name}?`}
          description="Disconnected repositories are excluded from every access scope, so their solutions stop appearing in search. You can reconnect later."
          confirmLabel="Disconnect repository"
          tone="danger"
          pending={action.pending}
          onConfirm={async () => {
            await change('disconnected');
          }}
        />
      )}
      <ActionStatus pending={action.pending} error={action.error} success={done} />
    </div>
  );
}

function RepositoryCard({ repo, isAdmin, onChanged }: { repo: RepositoryView; isAdmin: boolean; onChanged: () => void }) {
  const titleId = useFieldId('repo-title');
  return (
    <li>
      <article className="card repo" aria-labelledby={titleId}>
        <header className="card__header">
          <h2 id={titleId} className="card__title">
            {repo.url ? <ExternalLink href={repo.url}>{repo.full_name}</ExternalLink> : repo.full_name}
          </h2>
          <div className="badges">
            <Badge tone={repo.status === 'active' ? 'ok' : 'muted'}>{humanize(repo.status)}</Badge>
            <Badge tone={repo.permission === 'write' ? 'info' : 'neutral'}>{`${humanize(repo.permission)} access`}</Badge>
          </div>
        </header>
        <dl className="meta">
          <div>
            <dt>Provider</dt>
            <dd>{humanize(repo.provider)}</dd>
          </div>
          <div>
            <dt>Default branch</dt>
            <dd>
              <Mono>{repo.default_branch}</Mono>
            </dd>
          </div>
          <div>
            <dt>Last event</dt>
            <dd>{repo.last_event_at ? <RelativeTime iso={repo.last_event_at} /> : <span className="muted">No events</span>}</dd>
          </div>
          <div>
            <dt>Sessions</dt>
            <dd>{formatNumber(repo.session_count)}</dd>
          </div>
          <div>
            <dt>Solutions</dt>
            <dd>
              {repo.solution_count > 0 ? (
                <Link to={`/knowledge?repository_id=${encodeURIComponent(repo.id)}`}>{formatNumber(repo.solution_count)}</Link>
              ) : (
                '0'
              )}
            </dd>
          </div>
        </dl>
        <h3 className="subhead">Components</h3>
        {repo.components.length === 0 ? (
          <p className="muted">No components defined. Monorepos can map path globs to components.</p>
        ) : (
          <TableWrap label={`Components of ${repo.full_name}`}>
            <table>
              <thead>
                <tr>
                  <th scope="col">Name</th>
                  <th scope="col">Path glob</th>
                  <th scope="col">Package</th>
                  <th scope="col">Ecosystem</th>
                </tr>
              </thead>
              <tbody>
                {repo.components.map((c) => (
                  <tr key={c.id}>
                    <td>{c.name}</td>
                    <td>
                      <Mono>{c.path_glob}</Mono>
                    </td>
                    <td>{c.package_name ?? '—'}</td>
                    <td>{c.ecosystem ?? '—'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </TableWrap>
        )}
        {isAdmin ? (
          <div className="card__footer">
            <AddComponentForm repo={repo} onAdded={onChanged} />
            <StatusControls repo={repo} onChanged={onChanged} />
          </div>
        ) : null}
      </article>
    </li>
  );
}

export function RepositoriesPage() {
  useDocumentTitle('Repositories');
  const isAdmin = useIsAdmin();
  const query = useQuery('repositories', () => unwrap(api.GET('/v1/repositories')));
  return (
    <>
      <PageHeader
        title="Repositories"
        lede={isAdmin ? 'Every repository in the organization.' : 'Repositories you have been granted access to.'}
      />
      {isAdmin ? <AddRepositoryForm onAdded={query.refetch} /> : null}
      <QueryBoundary
        query={query}
        label="repositories"
        isEmpty={(d) => d.items.length === 0}
        empty={
          <EmptyState title="No repositories yet.">
            {isAdmin ? (
              <p>Add a repository above, then point agent integrations and webhooks at it.</p>
            ) : (
              <p>Ask an administrator to grant you access to a repository.</p>
            )}
          </EmptyState>
        }
      >
        {(d) => (
          <ul className="card-list" aria-label="Repositories">
            {d.items.map((r) => (
              <RepositoryCard key={r.id} repo={r} isAdmin={isAdmin} onChanged={query.refetch} />
            ))}
          </ul>
        )}
      </QueryBoundary>
    </>
  );
}
