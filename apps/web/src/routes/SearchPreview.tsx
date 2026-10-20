import { useState, type FormEvent } from 'react';
import { Link } from 'react-router';
import { api } from '../api/client';
import { unwrap } from '../api/errors';
import type { ApplicabilityStatus, ContextSearchRequest, Suggestion } from '../api/types';
import { Badge, StateBadge, type Tone } from '../components/Badge';
import { ExternalLink, useFieldId } from '../components/common';
import { ActionStatus, EmptyState } from '../components/Status';
import { useAction } from '../hooks/misc';
import { formatNumber, humanize } from '../lib/format';

const APPLICABILITY_TONES: Record<ApplicabilityStatus, Tone> = {
  compatible: 'ok',
  unknown: 'neutral',
  stale: 'warn',
  incompatible: 'bad',
};

function SuggestionCard({ s, rank }: { s: Suggestion; rank: number }) {
  return (
    <li className="card suggestion">
      <header className="card__header">
        <h3 className="card__title">
          <span className="suggestion__rank">{rank}.</span> <Link to={`/knowledge/${s.solution_id}`}>{s.title}</Link>
        </h3>
        <div className="badges">
          <StateBadge state={s.verification.state} />
          <Badge tone={APPLICABILITY_TONES[s.applicability.status] ?? 'neutral'}>
            {`Applicability: ${s.applicability.status}`}
          </Badge>
          {s.cross_repository ? <Badge tone="info">Other repository</Badge> : null}
        </div>
      </header>
      <dl className="pairs">
        <div>
          <dt>Issue</dt>
          <dd className="plain-text">{s.issue}</dd>
        </div>
        <div>
          <dt>What to reuse</dt>
          <dd className="plain-text">{s.reuse_action}</dd>
        </div>
        <div>
          <dt>Verification</dt>
          <dd>{s.verification.summary}</dd>
        </div>
        <div>
          <dt>Repository</dt>
          <dd>{s.repository}</dd>
        </div>
        {s.match_reasons.length > 0 ? (
          <div>
            <dt>Why it matched</dt>
            <dd>
              <ul className="inline-list">
                {s.match_reasons.map((r) => (
                  <li key={r}>{r}</li>
                ))}
              </ul>
            </dd>
          </div>
        ) : null}
        {s.applicability.conditions.length > 0 ? (
          <div>
            <dt>Check before reusing</dt>
            <dd>
              <ul>
                {s.applicability.conditions.map((c) => (
                  <li key={c}>{c}</li>
                ))}
              </ul>
            </dd>
          </div>
        ) : null}
        {s.applicability.notes.length > 0 ? (
          <div>
            <dt>Applicability notes</dt>
            <dd>
              <ul>
                {s.applicability.notes.map((n) => (
                  <li key={n}>{n}</li>
                ))}
              </ul>
            </dd>
          </div>
        ) : null}
        <div>
          <dt>Score</dt>
          <dd>{formatNumber(s.score, 3)}</dd>
        </div>
        {s.source_links.length > 0 ? (
          <div>
            <dt>Sources</dt>
            <dd>
              <ul className="inline-list">
                {s.source_links.map((l, i) => (
                  <li key={`${l.kind}-${i}`}>
                    <ExternalLink href={l.url}>{`${humanize(l.kind)}: ${l.label}`}</ExternalLink>
                  </li>
                ))}
              </ul>
            </dd>
          </div>
        ) : null}
      </dl>
    </li>
  );
}

/** Preview what an agent would receive from POST /v1/context/search. */
export function SearchPreview() {
  const id = useFieldId('preview');
  const [task, setTask] = useState('');
  const [repository, setRepository] = useState('');
  const [error, setError] = useState('');
  const [component, setComponent] = useState('');
  const [platform, setPlatform] = useState('');
  const [includeIncompatible, setIncludeIncompatible] = useState(false);
  // preview=true: the server runs the same search but records no retrieval,
  // so previews never count as usage and cannot receive outcomes.
  const action = useAction((body: ContextSearchRequest) =>
    unwrap(api.POST('/v1/context/search', { params: { query: { preview: true } }, body })),
  );

  const submit = (e: FormEvent) => {
    e.preventDefault();
    void action.run({
      task: task.trim(),
      repository: repository.trim() || null,
      error: error.trim() || null,
      component_path: component.trim() || null,
      platform: platform.trim() || null,
      include_incompatible: includeIncompatible,
    });
  };

  const r = action.result;
  return (
    <section aria-labelledby={`${id}-title`}>
      <h2 id={`${id}-title`} className="visually-hidden">
        Try a search
      </h2>
      <p className="hint">
        Runs the same authorized search an agent gets from <code>find_relevant_context</code>, limited to repositories you
        can read. Previews are not recorded as retrievals: they do not count as usage and cannot receive outcomes.
      </p>
      <form className="form" onSubmit={submit}>
        <div className="field">
          <label htmlFor={`${id}-task`}>Task</label>
          <textarea
            id={`${id}-task`}
            required
            rows={2}
            placeholder="What the agent is about to work on"
            value={task}
            onChange={(e) => setTask(e.target.value)}
          />
        </div>
        <div className="field">
          <label htmlFor={`${id}-error`}>
            Error output <span className="optional">(optional)</span>
          </label>
          <textarea
            id={`${id}-error`}
            rows={3}
            className="mono"
            spellCheck={false}
            placeholder="error[E0308]: mismatched types"
            value={error}
            onChange={(e) => setError(e.target.value)}
          />
        </div>
        <div className="form-row">
          <div className="field">
            <label htmlFor={`${id}-repo`}>
              Repository <span className="optional">(owner/name, recommended)</span>
            </label>
            <input
              id={`${id}-repo`}
              spellCheck={false}
              autoCapitalize="none"
              value={repository}
              onChange={(e) => setRepository(e.target.value)}
            />
          </div>
          <div className="field">
            <label htmlFor={`${id}-component`}>
              Component path <span className="optional">(optional)</span>
            </label>
            <input id={`${id}-component`} spellCheck={false} value={component} onChange={(e) => setComponent(e.target.value)} />
          </div>
          <div className="field">
            <label htmlFor={`${id}-platform`}>
              Platform <span className="optional">(optional)</span>
            </label>
            <input id={`${id}-platform`} placeholder="linux" value={platform} onChange={(e) => setPlatform(e.target.value)} />
          </div>
        </div>
        <div className="field field--check">
          <input
            id={`${id}-incompatible`}
            type="checkbox"
            checked={includeIncompatible}
            onChange={(e) => setIncludeIncompatible(e.target.checked)}
          />
          <label htmlFor={`${id}-incompatible`}>Include suggestions whose known constraints are violated</label>
        </div>
        <button type="submit" className="button button--primary" disabled={action.pending}>
          Search as an agent
        </button>
      </form>
      <ActionStatus
        pending={action.pending}
        pendingText="Searching…"
        error={action.error}
        success={r ? `${r.suggestions.length} suggestion${r.suggestions.length === 1 ? '' : 's'} returned.` : undefined}
      />
      {r ? (
        <section className="preview-results" aria-label="Search preview results">
          <p className="notice notice--info">{r.notice}</p>
          <dl className="meta">
            <div>
              <dt>Latency</dt>
              <dd>{formatNumber(r.latency_ms)} ms</dd>
            </div>
            <div>
              <dt>Withheld as incompatible</dt>
              <dd>{formatNumber(r.suppressed_incompatible)}</dd>
            </div>
            <div>
              <dt>Search quality</dt>
              <dd>
                {r.degraded ? (
                  <Badge tone="warn">Degraded</Badge>
                ) : (
                  <Badge tone="ok">Complete</Badge>
                )}
              </dd>
            </div>
          </dl>
          {r.degraded && r.degraded_reasons && r.degraded_reasons.length > 0 ? (
            <p className="hint">Skipped stages: {r.degraded_reasons.join('; ')}</p>
          ) : null}
          {r.suggestions.length === 0 ? (
            <EmptyState title="The agent would receive no suggestions.">
              <p>Nothing in your readable repositories matched this task.</p>
            </EmptyState>
          ) : (
            <ol className="card-list">
              {r.suggestions.map((s, i) => (
                <SuggestionCard key={s.solution_id} s={s} rank={i + 1} />
              ))}
            </ol>
          )}
        </section>
      ) : null}
    </section>
  );
}
