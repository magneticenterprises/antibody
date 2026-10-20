import { useState, type FormEvent } from 'react';
import { api } from '../../api/client';
import { unwrap, unwrapVoid } from '../../api/errors';
import {
  SCOPES,
  type CreateToken,
  type PrincipalListRow,
  type RepositoryView,
  type Scope,
  type TokenCreated,
  type TokenRow,
} from '../../api/types';
import { Badge } from '../../components/Badge';
import { CopyButton, Mono, RelativeTime, TableWrap, useFieldId } from '../../components/common';
import { ConfirmAction } from '../../components/ConfirmAction';
import { ActionStatus, EmptyState, QueryBoundary } from '../../components/Status';
import { useAction } from '../../hooks/misc';
import { useQuery } from '../../hooks/useQuery';

const SCOPE_HELP: Record<Scope, string> = {
  'sessions:write': 'Upload agent sessions',
  'context:read': 'Search knowledge and read solutions',
  'outcomes:write': 'Report whether suggestions were used',
  'ci:write': 'Post CI events',
  admin: 'Administer the organization (admin principals only)',
};

function tokenStatus(t: TokenRow, now: number): { label: string; tone: 'ok' | 'muted' | 'bad' } {
  if (t.revoked_at) return { label: 'Revoked', tone: 'bad' };
  if (t.expires_at && Date.parse(t.expires_at) <= now) return { label: 'Expired', tone: 'muted' };
  return { label: 'Active', tone: 'ok' };
}

/** The plaintext token, shown exactly once and dropped from memory on dismiss. */
export function NewTokenPanel({ created, onDismiss }: { created: TokenCreated; onDismiss: () => void }) {
  return (
    <div className="notice notice--secret" role="region" aria-labelledby="new-token-title">
      <p id="new-token-title" className="notice__title">
        Copy the new token now
      </p>
      <p>
        This is the only time the token is shown. Store it in a secret manager or the agent's environment; Antibody keeps only
        a hash.
      </p>
      <div className="secret-row">
        <label htmlFor="new-token-value" className="visually-hidden">
          New API token
        </label>
        <input
          id="new-token-value"
          className="mono secret"
          readOnly
          value={created.token}
          onFocus={(e) => e.currentTarget.select()}
        />
        <CopyButton value={created.token} label="Copy token" what="new API token" />
      </div>
      <button type="button" className="button" onClick={onDismiss}>
        I have stored it — hide the token
      </button>
    </div>
  );
}

function CreateTokenForm({
  principals,
  repositories,
  canCreate,
  onCreated,
}: {
  principals: PrincipalListRow[];
  repositories: RepositoryView[];
  canCreate: boolean;
  onCreated: () => void;
}) {
  const id = useFieldId('tok');
  const active = principals.filter((p) => !p.revoked_at);
  const [principalId, setPrincipalId] = useState('');
  const [name, setName] = useState('');
  const [scopes, setScopes] = useState<Scope[]>(['context:read']);
  const [repoIds, setRepoIds] = useState<string[]>([]);
  const [expires, setExpires] = useState('');
  const [created, setCreated] = useState<TokenCreated | undefined>();
  const action = useAction((body: CreateToken) => unwrap(api.POST('/v1/admin/tokens', { body })));

  const toggle = <T,>(list: T[], v: T): T[] => (list.includes(v) ? list.filter((x) => x !== v) : [...list, v]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!canCreate || action.pending) return;
    setCreated(undefined);
    const days = expires.trim() ? Number.parseInt(expires, 10) : null;
    const result = await action.run({
      principal_id: principalId,
      name: name.trim(),
      scopes,
      repository_ids: repoIds.length > 0 ? repoIds : null,
      expires_in_days: days,
    });
    if (result) {
      setCreated(result);
      setName('');
      setRepoIds([]);
      setExpires('');
      onCreated();
    }
  };

  return (
    <div className="panel">
      <h3>Create a token</h3>
      {created ? <NewTokenPanel created={created} onDismiss={() => setCreated(undefined)} /> : null}
      <form className="form" onSubmit={submit}>
        <div className="form-row">
          <div className="field">
            <label htmlFor={`${id}-principal`}>Principal</label>
            <select id={`${id}-principal`} required value={principalId} onChange={(e) => setPrincipalId(e.target.value)}>
              <option value="">Choose a principal…</option>
              {active.map((p) => (
                <option key={p.id} value={p.id}>
                  {`${p.display_name} (${p.kind}, ${p.role})`}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor={`${id}-name`}>Token name</label>
            <input
              id={`${id}-name`}
              required
              maxLength={120}
              placeholder="ci-runner, laptop-claude-code…"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>
          <div className="field">
            <label htmlFor={`${id}-expires`}>
              Expires in days <span className="optional">(optional)</span>
            </label>
            <input
              id={`${id}-expires`}
              type="number"
              min={1}
              max={3650}
              inputMode="numeric"
              aria-describedby={`${id}-expires-hint`}
              value={expires}
              onChange={(e) => setExpires(e.target.value)}
            />
            <p id={`${id}-expires-hint`} className="hint">
              1–3650. Leave empty for a token that does not expire.
            </p>
          </div>
        </div>
        <fieldset className="fieldset">
          <legend>Scopes</legend>
          <p className="hint">A token can never exceed its principal's role.</p>
          {SCOPES.map((s) => (
            <div className="field field--check" key={s}>
              <input
                id={`${id}-scope-${s}`}
                type="checkbox"
                checked={scopes.includes(s)}
                onChange={() => setScopes((list) => toggle(list, s))}
              />
              <label htmlFor={`${id}-scope-${s}`}>
                <code>{s}</code> <span className="muted">— {SCOPE_HELP[s]}</span>
              </label>
            </div>
          ))}
        </fieldset>
        <fieldset className="fieldset" disabled={!canCreate}>
          <legend>
            Repository allow-list <span className="optional">(optional)</span>
          </legend>
          <p className="hint">Restricts the token further; it is intersected with the principal's grants.</p>
          <p className="hint">Leave unchecked to allow all repositories granted to the principal.</p>
          {repositories.map((r) => (
            <div className="field field--check" key={r.id}>
              <input
                id={`${id}-repo-${r.id}`}
                type="checkbox"
                checked={repoIds.includes(r.id)}
                onChange={() => setRepoIds((list) => toggle(list, r.id))}
              />
              <label htmlFor={`${id}-repo-${r.id}`}>{r.full_name}</label>
            </div>
          ))}
        </fieldset>
        <button type="submit" className="button button--primary" disabled={!canCreate || action.pending || scopes.length === 0}>
          Create token
        </button>
      </form>
      <ActionStatus pending={action.pending} error={action.error} success={created ? `Created token ${created.prefix}….` : undefined} />
    </div>
  );
}

export function TokensSection({ principals, repositories, canCreate }: {
  principals: PrincipalListRow[];
  repositories: RepositoryView[];
  canCreate: boolean;
}) {
  const query = useQuery('tokens', () => unwrap(api.GET('/v1/admin/tokens')));
  const [done, setDone] = useState<string | undefined>();
  const revoke = useAction(async (t: TokenRow) => {
    await unwrapVoid(api.DELETE('/v1/admin/tokens/{id}', { params: { path: { id: t.id } } }));
    return t;
  });
  const now = Date.now();
  return (
    <section className="settings-section" aria-labelledby="tokens-title">
      <h2 id="tokens-title">API tokens</h2>
      <p className="hint">Tokens let agents, hooks and CI call the API. Browsers never use them.</p>
      <CreateTokenForm principals={principals} repositories={repositories} canCreate={canCreate} onCreated={query.refetch} />
      <ActionStatus pending={revoke.pending} error={revoke.error} success={done} pendingText="Revoking…" />
      <QueryBoundary query={query} label="tokens" isEmpty={(d) => d.length === 0} empty={<EmptyState title="No tokens yet." />}>
        {(rows) => (
          <TableWrap label="Token list">
            <table>
              <thead>
                <tr>
                  <th scope="col">Name</th>
                  <th scope="col">Principal</th>
                  <th scope="col">Prefix</th>
                  <th scope="col">Scopes</th>
                  <th scope="col">Repositories</th>
                  <th scope="col">Created</th>
                  <th scope="col">Expires</th>
                  <th scope="col">Last used</th>
                  <th scope="col">Status</th>
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {rows.map((t) => {
                  const st = tokenStatus(t, now);
                  return (
                    <tr key={t.id}>
                      <td>{t.name}</td>
                      <td>{t.principal_name}</td>
                      <td>
                        <Mono>{t.token_prefix}</Mono>
                      </td>
                      <td>
                        <ul className="inline-list">
                          {t.scopes.map((s) => (
                            <li key={s}>
                              <code>{s}</code>
                            </li>
                          ))}
                        </ul>
                      </td>
                      <td>{t.repository_ids && t.repository_ids.length > 0 ? `${t.repository_ids.length} allowed` : 'All granted'}</td>
                      <td>
                        <RelativeTime iso={t.created_at} />
                      </td>
                      <td>{t.expires_at ? <RelativeTime iso={t.expires_at} /> : 'Never'}</td>
                      <td>
                        <RelativeTime iso={t.last_used_at} fallback="Never" />
                      </td>
                      <td>
                        <Badge tone={st.tone}>{st.label}</Badge>
                      </td>
                      <td>
                        {st.label === 'Active' ? (
                          <ConfirmAction
                            label="Revoke"
                            title={`Revoke “${t.name}”?`}
                            description="Clients using this token stop working immediately. This cannot be undone."
                            confirmLabel="Revoke token"
                            tone="danger"
                            pending={revoke.pending}
                            onConfirm={async () => {
                              setDone(undefined);
                              const ok = await revoke.run(t);
                              if (ok) {
                                setDone(`Revoked token ${t.name}.`);
                                query.refetch();
                              }
                              return Boolean(ok);
                            }}
                          />
                        ) : null}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </TableWrap>
        )}
      </QueryBoundary>
    </section>
  );
}
