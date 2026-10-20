import { useState, type FormEvent } from 'react';
import { api } from '../../api/client';
import { unwrap } from '../../api/errors';
import {
  WORKFLOW_STATES,
  type CreateWorkflowImprovement,
  type WorkflowImprovementView,
  type WorkflowState,
  type WorkflowTransition,
} from '../../api/types';
import { Badge } from '../../components/Badge';
import { PlainText, RelativeTime, useFieldId } from '../../components/common';
import { ActionStatus, EmptyState, QueryBoundary } from '../../components/Status';
import { useAction } from '../../hooks/misc';
import { useQuery } from '../../hooks/useQuery';
import { humanize, prettyJson } from '../../lib/format';

function TransitionForm({ item, onDone }: { item: WorkflowImprovementView; onDone: (w: WorkflowImprovementView) => void }) {
  const id = useFieldId('wf');
  const [to, setTo] = useState<WorkflowState>('evaluated');
  const [evaluation, setEvaluation] = useState('');
  const [reproducible, setReproducible] = useState(false);
  const [negative, setNegative] = useState(false);
  const [preserves, setPreserves] = useState(false);
  const [parseError, setParseError] = useState<string | undefined>();
  const action = useAction((body: WorkflowTransition) =>
    unwrap(api.POST('/v1/admin/workflow-improvements/{id}/transition', { params: { path: { id: item.id } }, body })),
  );

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setParseError(undefined);
    let parsed: unknown = undefined;
    if (evaluation.trim()) {
      try {
        parsed = JSON.parse(evaluation);
      } catch {
        setParseError('The evaluation record must be valid JSON.');
        return;
      }
    }
    const result = await action.run({
      to,
      evaluation: parsed,
      evaluation_reproducible: reproducible,
      has_negative_cases: negative,
      preserves_checks: preserves,
    });
    if (result) {
      setEvaluation('');
      onDone(result);
    }
  };

  return (
    <details className="disclosure">
      <summary>Move to another state</summary>
      <form className="form" onSubmit={submit}>
        <div className="field">
          <label htmlFor={`${id}-to`}>Target state</label>
          <select id={`${id}-to`} value={to} onChange={(e) => setTo(e.target.value as WorkflowState)}>
            {WORKFLOW_STATES.map((s) => (
              <option key={s} value={s}>
                {humanize(s)}
              </option>
            ))}
          </select>
        </div>
        <div className="field">
          <label htmlFor={`${id}-eval`}>
            Evaluation record (JSON) <span className="optional">(required to move to “evaluated”)</span>
          </label>
          <textarea
            id={`${id}-eval`}
            className="mono"
            rows={4}
            spellCheck={false}
            aria-invalid={Boolean(parseError)}
            value={evaluation}
            onChange={(e) => setEvaluation(e.target.value)}
          />
          {parseError ? (
            <p className="hint hint--error" role="alert">
              {parseError}
            </p>
          ) : null}
        </div>
        <fieldset className="fieldset">
          <legend>Promotion gate</legend>
          {(
            [
              ['repro', 'The evaluation is reproducible', reproducible, setReproducible],
              ['neg', 'It includes negative cases', negative, setNegative],
              ['checks', 'Existing checks are preserved', preserves, setPreserves],
            ] as const
          ).map(([key, label, value, set]) => (
            <div className="field field--check" key={key}>
              <input id={`${id}-${key}`} type="checkbox" checked={value} onChange={(e) => set(e.target.checked)} />
              <label htmlFor={`${id}-${key}`}>{label}</label>
            </div>
          ))}
        </fieldset>
        <button type="submit" className="button button--primary" disabled={action.pending}>
          Apply transition
        </button>
      </form>
      <ActionStatus
        pending={action.pending}
        error={action.error}
        success={action.result ? `Moved to ${action.result.state}.` : undefined}
      />
    </details>
  );
}

const MIN_CHANGE = 10;

function CreateWorkflowForm({ onCreated }: { onCreated: (w: WorkflowImprovementView) => void }) {
  const id = useFieldId('wf-new');
  const repos = useQuery('repositories', () => unwrap(api.GET('/v1/repositories')));
  const [title, setTitle] = useState('');
  const [change, setChange] = useState('');
  const [scope, setScope] = useState('organization');
  const [repositoryId, setRepositoryId] = useState('');
  const action = useAction((body: CreateWorkflowImprovement) =>
    unwrap(api.POST('/v1/admin/workflow-improvements', { body })),
  );
  const changeTooShort = change.trim().length > 0 && change.trim().length < MIN_CHANGE;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (change.trim().length < MIN_CHANGE) return;
    const created = await action.run({
      title: title.trim(),
      proposed_change: change.trim(),
      target_scope: scope.trim(),
      repository_id: repositoryId || null,
    });
    if (created) {
      setTitle('');
      setChange('');
      onCreated(created);
    }
  };

  return (
    <details className="disclosure panel">
      <summary>Propose an improvement</summary>
      <form className="form" onSubmit={submit}>
        <div className="field">
          <label htmlFor={`${id}-title`}>Title</label>
          <input id={`${id}-title`} required maxLength={200} value={title} onChange={(e) => setTitle(e.target.value)} />
        </div>
        <div className="field">
          <label htmlFor={`${id}-change`}>Proposed change</label>
          <textarea
            id={`${id}-change`}
            required
            rows={4}
            minLength={MIN_CHANGE}
            aria-invalid={changeTooShort}
            aria-describedby={`${id}-change-hint`}
            value={change}
            onChange={(e) => setChange(e.target.value)}
          />
          <p id={`${id}-change-hint`} className={changeTooShort ? 'hint hint--error' : 'hint'}>
            At least {MIN_CHANGE} characters. Describe the workflow change, not a code fix.
          </p>
        </div>
        <div className="form-row">
          <div className="field">
            <label htmlFor={`${id}-scope`}>Target scope</label>
            <input
              id={`${id}-scope`}
              required
              list={`${id}-scopes`}
              spellCheck={false}
              aria-describedby={`${id}-scope-hint`}
              value={scope}
              onChange={(e) => setScope(e.target.value)}
            />
            <datalist id={`${id}-scopes`}>
              <option value="organization" />
              <option value="repository" />
            </datalist>
            <p id={`${id}-scope-hint`} className="hint">
              For example <code>organization</code>, <code>repository</code> or <code>component:services/api</code>.
            </p>
          </div>
          <div className="field">
            <label htmlFor={`${id}-repo`}>
              Repository <span className="optional">(optional)</span>
            </label>
            <select id={`${id}-repo`} value={repositoryId} onChange={(e) => setRepositoryId(e.target.value)}>
              <option value="">None (organization-wide)</option>
              {(repos.data?.items ?? []).map((r) => (
                <option key={r.id} value={r.id}>
                  {r.full_name}
                </option>
              ))}
            </select>
          </div>
        </div>
        <button type="submit" className="button button--primary" disabled={action.pending}>
          Propose improvement
        </button>
      </form>
      <ActionStatus
        pending={action.pending}
        error={action.error}
        success={action.result ? `Proposed “${action.result.title}” as a candidate.` : undefined}
      />
    </details>
  );
}

export function WorkflowSection() {
  const query = useQuery('workflow', () => unwrap(api.GET('/v1/admin/workflow-improvements')));
  return (
    <section className="settings-section" aria-labelledby="wf-title">
      <h2 id="wf-title">Workflow improvements</h2>
      <p className="hint">
        Proposed changes to agent workflows, mined from sessions. The API enforces the lifecycle (candidate → evaluated →
        available → promoted, or retired); promotion needs a reproducible evaluation with negative cases.
      </p>
      <CreateWorkflowForm onCreated={(w) => query.setData([w, ...(query.data ?? []).filter((x) => x.id !== w.id)])} />
      <QueryBoundary
        query={query}
        label="workflow improvements"
        isEmpty={(d) => d.length === 0}
        empty={<EmptyState title="No workflow improvements proposed yet." />}
      >
        {(rows) => (
          <ul className="card-list">
            {rows.map((w) => (
              <li key={w.id} className="card">
                <header className="card__header">
                  <h3 className="card__title">{w.title}</h3>
                  <div className="badges">
                    <Badge tone={w.state === 'promoted' ? 'ok' : w.state === 'retired' ? 'muted' : 'neutral'}>{humanize(w.state)}</Badge>
                    <Badge tone="neutral">{`v${w.version}`}</Badge>
                  </div>
                </header>
                <p className="result__meta">
                  <span>Scope: {w.target_scope}</span>
                  <span>
                    Updated <RelativeTime iso={w.updated_at} />
                  </span>
                </p>
                <h4 className="subhead">Proposed change (untrusted)</h4>
                <PlainText>{w.proposed_change}</PlainText>
                {w.evaluation !== null && w.evaluation !== undefined ? (
                  <details className="disclosure">
                    <summary>Evaluation record</summary>
                    <pre className="excerpt">{prettyJson(w.evaluation)}</pre>
                  </details>
                ) : null}
                <TransitionForm
                  item={w}
                  onDone={(updated) => query.setData(rows.map((r) => (r.id === updated.id ? updated : r)))}
                />
              </li>
            ))}
          </ul>
        )}
      </QueryBoundary>
    </section>
  );
}
