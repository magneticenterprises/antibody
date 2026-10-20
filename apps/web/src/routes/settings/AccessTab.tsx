import { api } from '../../api/client';
import { unwrap } from '../../api/errors';
import { EmptyState, QueryBoundary } from '../../components/Status';
import { useQuery } from '../../hooks/useQuery';
import { GrantsSection } from './GrantsSection';
import { PrincipalsSection } from './PrincipalsSection';
import { TokensSection } from './TokensSection';

export { NewTokenPanel } from './TokensSection';

export function AccessTab() {
  const principals = useQuery('principals', () => unwrap(api.GET('/v1/admin/principals')));
  const repositories = useQuery('repositories', () => unwrap(api.GET('/v1/repositories')));
  const p = principals.data ?? [];
  const r = repositories.data?.items ?? [];
  const ready = principals.status === 'success' && !principals.error && !principals.isFetching
    && repositories.status === 'success' && !repositories.error && !repositories.isFetching;
  return (
    <>
      <QueryBoundary
        query={repositories}
        label="repositories"
        isEmpty={(data) => data.items.length === 0}
        empty={<EmptyState title="No repositories registered." />}
      >
        {() => null}
      </QueryBoundary>
      {!ready ? <p className="hint">Token creation and new grants are paused until principals and repositories load successfully.</p> : null}
      <TokensSection principals={p} repositories={r} canCreate={ready} />
      <PrincipalsSection query={principals} />
      <GrantsSection principals={p} repositories={r} canGrant={ready} />
    </>
  );
}
