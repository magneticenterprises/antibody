import { useState, type FormEvent } from 'react';
import { Link } from 'react-router';
import { api } from '../../api/client';
import { unwrap } from '../../api/errors';
import type { DeletionReport } from '../../api/types';
import { useFieldId } from '../../components/common';
import { ConfirmAction } from '../../components/ConfirmAction';
import { ActionStatus } from '../../components/Status';
import { useAction } from '../../hooks/misc';
import { formatNumber, isUuid } from '../../lib/format';

export function DeletionReportView({ report }: { report: DeletionReport }) {
  return (
    <dl className="pairs" aria-label="Deletion report">
      <div>
        <dt>Sessions deleted</dt>
        <dd>{formatNumber(report.sessions_deleted)}</dd>
      </div>
      <div>
        <dt>Solutions deleted</dt>
        <dd>{formatNumber(report.solutions_deleted)}</dd>
      </div>
      <div>
        <dt>Solutions detached</dt>
        <dd>{formatNumber(report.solutions_detached)}</dd>
      </div>
      <div>
        <dt>Jobs cancelled</dt>
        <dd>{formatNumber(report.jobs_cancelled)}</dd>
      </div>
    </dl>
  );
}

/** Entry point for deleting one agent session by ID (e.g. a data request). */
export function SessionDeletion() {
  const id = useFieldId('del');
  const [sessionId, setSessionId] = useState('');
  const [deleted, setDeleted] = useState<{ id: string; report: DeletionReport } | undefined>();
  const action = useAction((sid: string) => unwrap(api.DELETE('/v1/sessions/{id}', { params: { path: { id: sid } } })));
  const valid = isUuid(sessionId);

  return (
    <section className="settings-section" aria-labelledby={`${id}-title`}>
      <h2 id={`${id}-title`}>Delete a session</h2>
      <p className="hint">
        Removes one agent session and its payload, deleting solutions that came only from it. Find session IDs under{' '}
        <Link to="/activity/sessions">Activity → Sessions</Link>, where each session page also offers deletion.
      </p>
      <form className="form form--inline" onSubmit={(e: FormEvent) => e.preventDefault()}>
        <div className="field field--grow">
          <label htmlFor={`${id}-sid`}>Session ID</label>
          <input
            id={`${id}-sid`}
            className="mono"
            spellCheck={false}
            placeholder="00000000-0000-0000-0000-000000000000"
            aria-invalid={sessionId !== '' && !valid}
            value={sessionId}
            onChange={(e) => setSessionId(e.target.value)}
          />
        </div>
      </form>
      <ConfirmAction
        label="Delete session…"
        title="Delete this session permanently?"
        description="This cannot be undone. The deletion is recorded in the audit log."
        confirmLabel="Delete session"
        tone="danger"
        disabled={!valid}
        pending={action.pending}
        onConfirm={async () => {
          const sid = sessionId.trim();
          const report = await action.run(sid);
          if (report) {
            setDeleted({ id: sid, report });
            setSessionId('');
          }
          return Boolean(report);
        }}
      />
      <ActionStatus
        pending={action.pending}
        error={action.error}
        success={deleted ? `Deleted session ${deleted.id.slice(0, 8)}.` : undefined}
        pendingText="Deleting…"
      />
      {deleted ? <DeletionReportView report={deleted.report} /> : null}
    </section>
  );
}
