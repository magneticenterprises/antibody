import { useState, type FormEvent } from 'react';
import { api } from '../../api/client';
import { unwrap } from '../../api/errors';
import {
  PRINCIPAL_KINDS,
  ROLES,
  type CreatePrincipal,
  type PrincipalKind,
  type PrincipalListRow,
  type Role,
} from '../../api/types';
import { Badge } from '../../components/Badge';
import { Mono, RelativeTime, TableWrap, useFieldId } from '../../components/common';
import { ActionStatus, QueryBoundary } from '../../components/Status';
import { useAction } from '../../hooks/misc';
import type { QueryResult } from '../../hooks/useQuery';
import { humanize } from '../../lib/format';

function CreatePrincipalForm({ onCreated }: { onCreated: () => void }) {
  const id = useFieldId('pr');
  const [kind, setKind] = useState<PrincipalKind>('user');
  const [displayName, setDisplayName] = useState('');
  const [role, setRole] = useState<Role>('member');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const action = useAction((body: CreatePrincipal) => unwrap(api.POST('/v1/admin/principals', { body })));
  const [done, setDone] = useState<string | undefined>();

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setDone(undefined);
    const name = displayName.trim();
    const result = await action.run(
      kind === 'user'
        ? { kind, display_name: name, role, username: username.trim(), password }
        : { kind, display_name: name, role, username: null, password: null },
    );
    if (result) {
      setDone(`Created ${kind} “${name}”.`);
      setDisplayName('');
      setUsername('');
      setPassword('');
      onCreated();
    }
  };

  return (
    <div className="panel">
      <h3>Create a principal</h3>
      <form className="form" onSubmit={submit}>
        <div className="form-row">
          <div className="field">
            <label htmlFor={`${id}-kind`}>Kind</label>
            <select id={`${id}-kind`} value={kind} onChange={(e) => setKind(e.target.value as PrincipalKind)}>
              {PRINCIPAL_KINDS.map((k) => (
                <option key={k} value={k}>
                  {k === 'user' ? 'User (signs in to the dashboard)' : 'Service (API tokens only)'}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor={`${id}-display`}>Display name</label>
            <input id={`${id}-display`} required maxLength={120} value={displayName} onChange={(e) => setDisplayName(e.target.value)} />
          </div>
          <div className="field">
            <label htmlFor={`${id}-role`}>Role</label>
            <select id={`${id}-role`} value={role} onChange={(e) => setRole(e.target.value as Role)}>
              {ROLES.map((r) => (
                <option key={r} value={r}>
                  {humanize(r)}
                </option>
              ))}
            </select>
          </div>
        </div>
        {kind === 'user' ? (
          <div className="form-row">
            <div className="field">
              <label htmlFor={`${id}-username`}>Username</label>
              <input
                id={`${id}-username`}
                required
                maxLength={128}
                pattern="[A-Za-z0-9._@\-]+"
                title="Letters, digits and . _ @ -"
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                value={username}
                onChange={(e) => setUsername(e.target.value)}
              />
            </div>
            <div className="field">
              <label htmlFor={`${id}-password`}>Initial password</label>
              <input
                id={`${id}-password`}
                type="password"
                required
                minLength={12}
                maxLength={256}
                autoComplete="new-password"
                aria-describedby={`${id}-password-hint`}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
              <p id={`${id}-password-hint`} className="hint">
                At least 12 characters. Share it with the person through a secure channel.
              </p>
            </div>
          </div>
        ) : null}
        <button type="submit" className="button button--primary" disabled={action.pending}>
          Create principal
        </button>
      </form>
      <ActionStatus pending={action.pending} error={action.error} success={done} />
    </div>
  );
}

export function PrincipalsSection({ query }: { query: QueryResult<PrincipalListRow[]> }) {
  return (
    <section className="settings-section" aria-labelledby="principals-title">
      <h2 id="principals-title">Principals</h2>
      <p className="hint">People (users) and automation (services). Members only see repositories they are granted.</p>
      <CreatePrincipalForm onCreated={query.refetch} />
      <QueryBoundary query={query} label="principals" isEmpty={(d) => d.length === 0}>
        {(rows) => (
          <TableWrap label="Principal list">
            <table>
              <thead>
                <tr>
                  <th scope="col">Name</th>
                  <th scope="col">Kind</th>
                  <th scope="col">Role</th>
                  <th scope="col">Username</th>
                  <th scope="col">Created</th>
                  <th scope="col">Status</th>
                  <th scope="col">ID</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((p) => (
                  <tr key={p.id}>
                    <td>{p.display_name}</td>
                    <td>{humanize(p.kind)}</td>
                    <td>{humanize(p.role)}</td>
                    <td>{p.username ? <Mono>{p.username}</Mono> : '—'}</td>
                    <td>
                      <RelativeTime iso={p.created_at} />
                    </td>
                    <td>{p.revoked_at ? <Badge tone="bad">Revoked</Badge> : <Badge tone="ok">Active</Badge>}</td>
                    <td>
                      <Mono short={8}>{p.id}</Mono>
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
