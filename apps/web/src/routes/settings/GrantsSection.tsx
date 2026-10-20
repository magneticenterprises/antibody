import { useState, type FormEvent } from 'react';
import { api } from '../../api/client';
import { unwrap, unwrapVoid } from '../../api/errors';
import { PERMISSIONS, type GrantRow, type Permission, type PrincipalListRow, type RepositoryView } from '../../api/types';
import { Badge } from '../../components/Badge';
import { RelativeTime, TableWrap, useFieldId } from '../../components/common';
import { ConfirmAction } from '../../components/ConfirmAction';
import { ActionStatus, EmptyState, QueryBoundary } from '../../components/Status';
import { useAction } from '../../hooks/misc';
import { useQuery } from '../../hooks/useQuery';
import { humanize } from '../../lib/format';

export function GrantsSection({ principals, repositories, canGrant }: {
  principals: PrincipalListRow[];
  repositories: RepositoryView[];
  canGrant: boolean;
}) {
  const id = useFieldId('grant');
  const grants = useQuery('grants', () => unwrap(api.GET('/v1/admin/grants')));
  const members = principals.filter((p) => !p.revoked_at);
  const [principalId, setPrincipalId] = useState('');
  const [repositoryId, setRepositoryId] = useState('');
  const [permission, setPermission] = useState<Permission>('read');
  const [done, setDone] = useState<string | undefined>();
  const grant = useAction(async () => {
    await unwrapVoid(api.PUT('/v1/admin/grants', { body: { principal_id: principalId, repository_id: repositoryId, permission } }));
    return true;
  });
  const revoke = useAction(async (g: GrantRow) => {
    await unwrapVoid(api.DELETE('/v1/admin/grants', { body: { principal_id: g.principal_id, repository_id: g.repository_id } }));
    return g;
  });
  const ready = canGrant && principalId !== '' && repositoryId !== '';

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!ready || grant.pending) return;
    setDone(undefined);
    if (await grant.run()) {
      const p = principals.find((x) => x.id === principalId)?.display_name ?? 'principal';
      const r = repositories.find((x) => x.id === repositoryId)?.full_name ?? 'repository';
      setDone(`Granted ${permission} access on ${r} to ${p}.`);
      grants.refetch();
    }
  };

  return (
    <section className="settings-section" aria-labelledby="grants-title">
      <h2 id="grants-title">Repository grants</h2>
      <p className="hint">
        Members and services can only read or write repositories they are granted. Administrators see everything. Granting
        again changes the permission.
      </p>
      {canGrant && (members.length === 0 || repositories.length === 0) ? (
        <EmptyState title="Grants need at least one principal and one repository." />
      ) : (
        <form className="form" onSubmit={submit}>
          <div className="form-row">
            <div className="field">
              <label htmlFor={`${id}-principal`}>Principal</label>
              <select id={`${id}-principal`} required value={principalId} onChange={(e) => setPrincipalId(e.target.value)}>
                <option value="">Choose…</option>
                {members.map((p) => (
                  <option key={p.id} value={p.id}>
                    {`${p.display_name} (${p.kind}, ${p.role})`}
                  </option>
                ))}
              </select>
            </div>
            <div className="field">
              <label htmlFor={`${id}-repo`}>Repository</label>
              <select id={`${id}-repo`} required value={repositoryId} onChange={(e) => setRepositoryId(e.target.value)}>
                <option value="">Choose…</option>
                {repositories.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.full_name}
                  </option>
                ))}
              </select>
            </div>
            <div className="field">
              <label htmlFor={`${id}-perm`}>Permission</label>
              <select id={`${id}-perm`} value={permission} onChange={(e) => setPermission(e.target.value as Permission)}>
                {PERMISSIONS.map((p) => (
                  <option key={p} value={p}>
                    {p === 'read' ? 'Read (search, view)' : 'Write (also upload sessions)'}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="button-row">
            <button type="submit" className="button button--primary" disabled={grant.pending || !ready}>
              Grant access
            </button>
          </div>
        </form>
      )}
      <ActionStatus
        pending={grant.pending || revoke.pending}
        error={grant.error ?? revoke.error}
        success={done}
      />
      <QueryBoundary
        query={grants}
        label="grants"
        forbiddenWhat="repository grants"
        isEmpty={(d) => d.length === 0}
        empty={<EmptyState title="No grants yet. Only administrators can see repositories." />}
      >
        {(rows) => (
          <TableWrap label="Grant list">
            <table>
              <thead>
                <tr>
                  <th scope="col">Principal</th>
                  <th scope="col">Repository</th>
                  <th scope="col">Permission</th>
                  <th scope="col">Granted</th>
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {rows.map((g) => (
                  <tr key={`${g.principal_id}:${g.repository_id}`}>
                    <td>{g.principal_name}</td>
                    <td>{g.repository}</td>
                    <td>
                      <Badge tone={g.permission === 'write' ? 'info' : 'neutral'}>{humanize(g.permission)}</Badge>
                    </td>
                    <td>
                      <RelativeTime iso={g.created_at} />
                    </td>
                    <td>
                      <ConfirmAction
                        label="Revoke"
                        title={`Revoke ${g.principal_name}'s access to ${g.repository}?`}
                        description="Access ends immediately, including through the principal's API tokens."
                        confirmLabel="Revoke grant"
                        tone="danger"
                        pending={revoke.pending}
                        onConfirm={async () => {
                          setDone(undefined);
                          const ok = await revoke.run(g);
                          if (ok) {
                            setDone(`Revoked ${g.principal_name}'s access to ${g.repository}.`);
                            grants.refetch();
                          }
                          return Boolean(ok);
                        }}
                      />
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
