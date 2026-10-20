import { useState } from 'react';
import { Link, useParams } from 'react-router';
import { api } from '../../api/client';
import { unwrap } from '../../api/errors';
import type { SessionStatusResponse, TranscriptView } from '../../api/types';
import { useIsAdmin } from '../../auth/AuthContext';
import { Badge, ExtractionBadge } from '../../components/Badge';
import { Mono, PageHeader, RelativeTime } from '../../components/common';
import { ConfirmAction } from '../../components/ConfirmAction';
import { ActionStatus, EmptyState, QueryBoundary } from '../../components/Status';
import { useAction, useDocumentTitle } from '../../hooks/misc';
import { useQuery } from '../../hooks/useQuery';
import { formatDate, formatNumber, humanize, isUuid } from '../../lib/format';
import { DeletionReportView } from '../settings/SessionDeletion';

function Transcript({ t, onHide }: { t: TranscriptView; onHide: () => void }) {
  return (
    <section className="transcript" aria-labelledby="transcript-title">
      <header className="card__header">
        <h2 id="transcript-title">Raw transcript</h2>
        <button type="button" className="button button--quiet" onClick={onHide}>
          Hide transcript
        </button>
      </header>
      <p className="notice notice--warn">{t.notice}</p>
      <p className="hint">
        {formatNumber(t.messages.length)} messages · {formatNumber(t.redaction_count)} redactions. Shown as plain text.
      </p>
      <ol className="transcript__messages">
        {t.messages.map((m, i) => (
          <li key={i} className={`message message--${m.role}`}>
            <p className="message__role">
              {humanize(m.role)}
              {m.tool_name ? (
                <>
                  {' '}
                  · <Mono>{m.tool_name}</Mono>
                </>
              ) : null}
            </p>
            <pre className="message__content">{m.content}</pre>
          </li>
        ))}
      </ol>
      {t.hints ? (
        <details className="disclosure">
          <summary>Structured hints supplied by the agent (untrusted)</summary>
          <pre className="excerpt">{JSON.stringify(t.hints, null, 2)}</pre>
        </details>
      ) : null}
    </section>
  );
}

function PrivilegedActions({ s }: { s: SessionStatusResponse }) {
  const [transcript, setTranscript] = useState<TranscriptView | undefined>();
  const view = useAction(() => unwrap(api.GET('/v1/sessions/{id}/transcript', { params: { path: { id: s.id } } })));
  const del = useAction(() => unwrap(api.DELETE('/v1/sessions/{id}', { params: { path: { id: s.id } } })));

  if (del.result) {
    return (
      <section className="detail-section" aria-labelledby="deleted-title">
        <h2 id="deleted-title">Session deleted</h2>
        <DeletionReportView report={del.result} />
        <p>
          <Link to="/activity/sessions">Back to sessions</Link>
        </p>
      </section>
    );
  }

  return (
    <section className="detail-section" aria-labelledby="priv-title">
      <h2 id="priv-title">Privileged actions</h2>
      <div className="action-grid">
        {s.raw_payload_available ? (
          <ConfirmAction
            label="View raw transcript…"
            title="Open the raw transcript?"
            description={
              <>
                <p>
                  This is privileged access to a retained (best-effort redacted) agent transcript.{' '}
                  <strong>The access is recorded in the audit log</strong> with your name.
                </p>
                <p>Only continue if you need the transcript for an investigation.</p>
              </>
            }
            confirmLabel="Open transcript (audited)"
            pending={view.pending}
            onConfirm={async () => {
              const t = await view.run();
              if (t) setTranscript(t);
              return Boolean(t);
            }}
          />
        ) : (
          <p className="muted">
            No raw transcript is retained for this session (capture mode: {humanize(s.capture_mode)}).
          </p>
        )}
        <ConfirmAction
          label="Delete session…"
          title="Delete this session permanently?"
          description="Deletes the session and its payload. Solutions that came only from this session are deleted; others are detached. This cannot be undone and is audited."
          confirmLabel="Delete session"
          tone="danger"
          pending={del.pending}
          onConfirm={async () => Boolean(await del.run())}
        />
      </div>
      <ActionStatus pending={view.pending} error={view.error} pendingText="Opening transcript…" />
      <ActionStatus pending={del.pending} error={del.error} pendingText="Deleting…" />
      {transcript ? <Transcript t={transcript} onHide={() => setTranscript(undefined)} /> : null}
    </section>
  );
}

export function SessionDetailPage() {
  const { id = '' } = useParams();
  const isAdmin = useIsAdmin();
  const valid = isUuid(id);
  useDocumentTitle('Session');
  const query = useQuery(valid ? `session:${id}` : null, () => unwrap(api.GET('/v1/sessions/{id}', { params: { path: { id } } })));

  return (
    <>
      <p className="breadcrumb">
        <Link to="/activity/sessions">← Sessions</Link>
      </p>
      <PageHeader title="Agent session" lede={valid ? <Mono>{id}</Mono> : undefined} />
      {!valid ? (
        <EmptyState title="Session not found.">
          <p>The address does not contain a valid session ID.</p>
        </EmptyState>
      ) : (
        <QueryBoundary query={query} label="session" forbiddenWhat="this session">
          {(s) => (
            <>
              <div className="badges badges--lead">
                <ExtractionBadge status={s.extraction_status} />
                <Badge tone="neutral">{`Ingestion: ${s.ingestion_status}`}</Badge>
                <Badge tone={s.capture_mode === 'raw' ? 'warn' : 'muted'}>{`Capture: ${humanize(s.capture_mode)}`}</Badge>
              </div>
              {s.extraction_error ? (
                <div className="notice notice--error">
                  <p className="notice__title">Extraction error</p>
                  <p className="plain-text">{s.extraction_error}</p>
                </div>
              ) : null}
              <dl className="pairs pairs--wide">
                <div>
                  <dt>Repository</dt>
                  <dd>{s.repository}</dd>
                </div>
                <div>
                  <dt>Agent</dt>
                  <dd>{humanize(s.agent_type)}</dd>
                </div>
                <div>
                  <dt>Agent session ID</dt>
                  <dd>{s.external_id ? <Mono>{s.external_id}</Mono> : '—'}</dd>
                </div>
                <div>
                  <dt>Revision</dt>
                  <dd>{s.revision ? <Mono>{s.revision}</Mono> : '—'}</dd>
                </div>
                <div>
                  <dt>Branch</dt>
                  <dd>{s.branch ? <Mono>{s.branch}</Mono> : '—'}</dd>
                </div>
                <div>
                  <dt>Received</dt>
                  <dd>
                    <RelativeTime iso={s.created_at} /> <span className="muted">({formatDate(s.created_at)})</span>
                  </dd>
                </div>
                <div>
                  <dt>Processed</dt>
                  <dd>{s.processed_at ? formatDate(s.processed_at) : 'Not yet'}</dd>
                </div>
                <div>
                  <dt>Extractor</dt>
                  <dd>{s.extractor ? <Mono>{s.extractor}</Mono> : '—'}</dd>
                </div>
                <div>
                  <dt>Messages</dt>
                  <dd>{formatNumber(s.message_count)}</dd>
                </div>
                <div>
                  <dt>Payload size</dt>
                  <dd>{formatNumber(s.payload_bytes)} bytes</dd>
                </div>
                <div>
                  <dt>Redactions</dt>
                  <dd>{formatNumber(s.redaction_count)}</dd>
                </div>
                <div>
                  <dt>Raw transcript retained</dt>
                  <dd>
                    {s.raw_payload_available ? 'Yes' : 'No'}
                    {s.retention_expires_at ? ` (until ${formatDate(s.retention_expires_at)})` : ''}
                  </dd>
                </div>
              </dl>
              <section className="detail-section" aria-labelledby="sess-solutions">
                <h2 id="sess-solutions">Extracted solutions</h2>
                {s.solution_ids.length === 0 ? (
                  <p className="muted">
                    {s.extraction_status === 'succeeded' ? 'Nothing reusable was found in this session.' : 'None yet.'}
                  </p>
                ) : (
                  <ul className="tag-list">
                    {s.solution_ids.map((sid) => (
                      <li key={sid}>
                        <Link to={`/knowledge/${sid}`}>
                          Solution <Mono short={8}>{sid}</Mono>
                        </Link>
                      </li>
                    ))}
                  </ul>
                )}
              </section>
              {isAdmin ? <PrivilegedActions s={s} /> : null}
            </>
          )}
        </QueryBoundary>
      )}
    </>
  );
}
