import { useState } from 'react';
import { useNavigate } from 'react-router';
import { api } from '../../api/client';
import { unwrap, unwrapVoid } from '../../api/errors';
import type { SolutionAction, SolutionDetail, SolutionStateChange } from '../../api/types';
import { useIsAdmin } from '../../auth/AuthContext';
import { useFieldId } from '../../components/common';
import { ConfirmAction } from '../../components/ConfirmAction';
import { ActionStatus } from '../../components/Status';
import { useAction } from '../../hooks/misc';
import { useQuery } from '../../hooks/useQuery';
import { isUuid } from '../../lib/format';
import { Section } from './Section';

/** Admin-only hard delete for leak response; withdraw is the normal path. */
function DeletePermanently({ solution }: { solution: SolutionDetail }) {
  const navigate = useNavigate();
  const action = useAction(async () => {
    await unwrapVoid(api.DELETE('/v1/solutions/{id}', { params: { path: { id: solution.id } } }));
    return true;
  });
  return (
    <div className="danger-zone">
      <h3>Delete permanently</h3>
      <p className="hint">
        Removes the solution, its evidence and its embeddings. Use only when it contains leaked secrets or data that must not
        be kept. To retire knowledge, withdraw it instead.
      </p>
      <ConfirmAction
        label="Delete permanently…"
        title="Delete this solution permanently?"
        description={
          <>
            <p>
              <strong>This cannot be undone.</strong> The solution, its evidence and its embeddings are erased; the deletion is
              recorded in the audit log.
            </p>
            <p>
              Permanent deletion is meant for leaked secrets or other content that must not be retained. Withdrawing is the
              normal way to retire knowledge.
            </p>
          </>
        }
        confirmLabel="Delete permanently"
        tone="danger"
        pending={action.pending}
        onConfirm={async () => {
          const ok = await action.run();
          if (ok) navigate('/knowledge', { replace: true, state: { deleted: solution.title } });
          return Boolean(ok);
        }}
      />
      <ActionStatus pending={action.pending} error={action.error} pendingText="Deleting…" />
    </div>
  );
}

export function AdminActions({ solution, onChanged }: { solution: SolutionDetail; onChanged: (s: SolutionDetail) => void }) {
  const isAdmin = useIsAdmin();
  const id = useFieldId('sol-action');
  const [reason, setReason] = useState('');
  const [target, setTarget] = useState('');
  const [done, setDone] = useState<string | undefined>();
  const action = useAction((body: SolutionStateChange) =>
    unwrap(api.POST('/v1/solutions/{id}/state', { params: { path: { id: solution.id } }, body })),
  );
  // Candidate replacements: other solutions in the same repository.
  const candidates = useQuery(solution.can_supersede ? `supersede-candidates:${solution.repository_id}` : null, () =>
    unwrap(api.GET('/v1/solutions', { params: { query: { repository_id: solution.repository_id, limit: 100 } } })),
  );

  if (!isAdmin && !solution.can_withdraw && !solution.can_supersede) return null;

  const run = async (act: SolutionAction, extra: Partial<SolutionStateChange> = {}, message: string) => {
    setDone(undefined);
    const updated = await action.run({ action: act, reason: reason.trim() || null, ...extra });
    if (updated) {
      setReason('');
      setTarget('');
      setDone(message);
      onChanged(updated);
      return true;
    }
    return false;
  };

  const reasonField = (suffix: string) => (
    <div className="field">
      <label htmlFor={`${id}-reason-${suffix}`}>
        Reason <span className="optional">(optional, recorded in the audit log)</span>
      </label>
      <input id={`${id}-reason-${suffix}`} maxLength={500} value={reason} onChange={(e) => setReason(e.target.value)} />
    </div>
  );

  const options = (candidates.data?.items ?? []).filter((s) => s.id !== solution.id);
  const targetValid = isUuid(target) && target !== solution.id;

  return (
    <Section id="sol-admin" title="Administrator actions">
      <p className="hint">State changes are applied by the API, which rejects transitions that are not allowed.</p>
      <div className="action-grid">
        {isAdmin ? (
          <ConfirmAction
            label="Verify"
            title="Mark as verified?"
            description="Adds trusted manual-verification evidence attributed to you. Use only after confirming the fix yourself."
            confirmLabel="Verify solution"
            pending={action.pending}
            onConfirm={() => run('verify', {}, 'Solution verified.')}
          >
            {reasonField('verify')}
          </ConfirmAction>
        ) : null}
        {isAdmin && solution.verification.needs_revalidation ? (
          <ConfirmAction
            label="Clear revalidation"
            title="Clear the revalidation flag?"
            description="Confirms the solution still applies after the change that flagged it."
            confirmLabel="Clear revalidation"
            pending={action.pending}
            onConfirm={() => run('clear_revalidation', {}, 'Revalidation flag cleared.')}
          >
            {reasonField('clear')}
          </ConfirmAction>
        ) : null}
        {solution.can_supersede ? (
          <ConfirmAction
            label="Supersede…"
            title="Supersede with another solution"
            description="Agents will be pointed to the replacement instead of this solution."
            confirmLabel="Supersede"
            pending={action.pending}
            onConfirm={() => {
              if (!targetValid) return false;
              return run('supersede', { superseded_by: target.trim() }, 'Solution superseded.');
            }}
          >
            {options.length > 0 ? (
              <div className="field">
                <label htmlFor={`${id}-target-pick`}>Replacement from {solution.repository}</label>
                <select id={`${id}-target-pick`} value={options.some((o) => o.id === target) ? target : ''} onChange={(e) => setTarget(e.target.value)}>
                  <option value="">Choose a solution…</option>
                  {options.map((o) => (
                    <option key={o.id} value={o.id}>
                      {`${o.title} (v${o.version}, ${o.state})`}
                    </option>
                  ))}
                </select>
              </div>
            ) : null}
            <div className="field">
              <label htmlFor={`${id}-target`}>Replacement solution ID</label>
              <input
                id={`${id}-target`}
                className="mono"
                spellCheck={false}
                required
                placeholder="00000000-0000-0000-0000-000000000000"
                value={target}
                aria-invalid={target !== '' && !targetValid}
                aria-describedby={`${id}-target-hint`}
                onChange={(e) => setTarget(e.target.value)}
              />
              <p id={`${id}-target-hint`} className="hint">
                {target !== '' && !targetValid ? 'Enter the UUID of a different solution.' : 'Pick above or paste a solution ID.'}
              </p>
            </div>
            {reasonField('supersede')}
          </ConfirmAction>
        ) : null}
        {solution.can_withdraw ? (
          <ConfirmAction
            label="Withdraw"
            title="Withdraw this solution?"
            description="Withdrawn solutions are no longer suggested to agents. The action is audited."
            confirmLabel="Withdraw solution"
            tone="danger"
            pending={action.pending}
            onConfirm={() => run('withdraw', {}, 'Solution withdrawn.')}
          >
            {reasonField('withdraw')}
          </ConfirmAction>
        ) : null}
      </div>
      <ActionStatus pending={action.pending} error={action.error} success={done} pendingText="Applying…" />
      {isAdmin ? <DeletePermanently solution={solution} /> : null}
    </Section>
  );
}
