import { useEffect, useState, type FormEvent } from 'react';
import { api } from '../../api/client';
import { unwrap } from '../../api/errors';
import type { SettingsPatch, SettingsView, SweepReport, WebhookSecrets } from '../../api/types';
import { useSession } from '../../auth/AuthContext';
import { Badge } from '../../components/Badge';
import { CopyButton, Mono, useFieldId } from '../../components/common';
import { ConfirmAction } from '../../components/ConfirmAction';
import { ActionStatus, QueryBoundary } from '../../components/Status';
import { useAction } from '../../hooks/misc';
import { useQuery } from '../../hooks/useQuery';
import { formatDate, formatNumber, humanize, prettyJson } from '../../lib/format';
import { SessionDeletion } from './SessionDeletion';

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

/** settings.providers is free-form JSON (no secrets); render it as text. */
function ProviderList({ providers }: { providers: unknown }) {
  if (!isRecord(providers)) return <pre className="excerpt">{prettyJson(providers)}</pre>;
  return (
    <dl className="pairs pairs--wide">
      {Object.entries(providers).map(([role, desc]) => (
        <div key={role}>
          <dt>{role === 'llm' ? 'LLM' : humanize(role)}</dt>
          <dd>
            {desc === null || desc === undefined ? (
              <span className="muted">Not configured</span>
            ) : isRecord(desc) ? (
              <ul className="kv-list">
                {Object.entries(desc).map(([k, v]) => (
                  <li key={k}>
                    <span className="muted">{humanize(k)}:</span>{' '}
                    {typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean' ? String(v) : prettyJson(v)}
                  </li>
                ))}
              </ul>
            ) : (
              <span>{prettyJson(desc)}</span>
            )}
          </dd>
        </div>
      ))}
    </dl>
  );
}

function Endpoints({ s }: { s: SettingsView }) {
  const rows: Array<[string, string]> = [
    ['GitHub webhook URL', s.github_webhook_url],
    ['CI webhook URL', s.ci_webhook_url],
    ['MCP server URL', s.mcp_url],
  ];
  return (
    <dl className="pairs pairs--wide">
      {rows.map(([label, value]) => (
        <div key={label}>
          <dt>{label}</dt>
          <dd className="copy-row">
            <Mono>{value}</Mono> <CopyButton value={value} what={label} />
          </dd>
        </div>
      ))}
    </dl>
  );
}

function CaptureSettings({ settings, onSaved }: { settings: SettingsView; onSaved: (s: SettingsView) => void }) {
  const id = useFieldId('cap');
  const org = settings.organization;
  const [raw, setRaw] = useState(org.raw_capture_enabled);
  const [days, setDays] = useState(String(org.raw_retention_days));
  const [external, setExternal] = useState(org.external_providers_approved);
  const [acknowledged, setAcknowledged] = useState(false);
  const [crossRepo, setCrossRepo] = useState(org.ci_cross_repository_search);
  const action = useAction((body: SettingsPatch) => unwrap(api.PATCH('/v1/admin/settings', { body })));

  useEffect(() => {
    setRaw(org.raw_capture_enabled);
    setDays(String(org.raw_retention_days));
    setExternal(org.external_providers_approved);
    setCrossRepo(org.ci_cross_repository_search);
    setAcknowledged(false);
  }, [org.raw_capture_enabled, org.raw_retention_days, org.external_providers_approved, org.ci_cross_repository_search]);

  const daysNum = Number.parseInt(days, 10);
  const daysValid = /^\d+$/.test(days.trim()) && daysNum >= 0 && daysNum <= 3650;
  const approving = external && !org.external_providers_approved;
  const patch: SettingsPatch = {};
  if (raw !== org.raw_capture_enabled) patch.raw_capture_enabled = raw;
  if (daysValid && daysNum !== org.raw_retention_days) patch.raw_retention_days = daysNum;
  if (external !== org.external_providers_approved) patch.external_providers_approved = external;
  if (crossRepo !== org.ci_cross_repository_search) patch.ci_cross_repository_search = crossRepo;
  const dirty = Object.keys(patch).length > 0;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!daysValid || (approving && !acknowledged)) return;
    const saved = await action.run(patch);
    if (saved) onSaved(saved);
  };

  return (
    <section className="settings-section" aria-labelledby={`${id}-title`}>
      <h2 id={`${id}-title`}>Capture, retention and providers</h2>
      <form className="form" onSubmit={submit}>
        <div className="field field--check">
          <input id={`${id}-raw`} type="checkbox" checked={raw} onChange={(e) => setRaw(e.target.checked)} />
          <label htmlFor={`${id}-raw`}>
            Retain raw (redacted) transcripts
            <span className="hint-inline"> Off keeps only structured knowledge; payloads are deleted after extraction.</span>
          </label>
        </div>
        <div className="field">
          <label htmlFor={`${id}-days`}>Raw transcript retention (days)</label>
          <input
            id={`${id}-days`}
            type="number"
            min={0}
            max={3650}
            inputMode="numeric"
            required
            aria-invalid={!daysValid}
            aria-describedby={`${id}-days-hint`}
            value={days}
            onChange={(e) => setDays(e.target.value)}
          />
          <p id={`${id}-days-hint`} className={daysValid ? 'hint' : 'hint hint--error'}>
            {daysValid ? '0–3650. 0 disables raw retention even when capture is on.' : 'Enter a whole number from 0 to 3650.'}
          </p>
        </div>
        <div className="field field--check">
          <input
            id={`${id}-cross`}
            type="checkbox"
            checked={crossRepo}
            onChange={(e) => setCrossRepo(e.target.checked)}
          />
          <label htmlFor={`${id}-cross`}>
            Let CI lookups search across repositories
            <span className="hint-inline"> CI failures may then be matched with solutions from other repositories in this organization.</span>
          </label>
        </div>
        <fieldset className="fieldset fieldset--warn">
          <legend>External model providers</legend>
          <p className="warning-text" id={`${id}-ext-warning`}>
            <strong>Warning:</strong> approving external providers allows Antibody to send private content — redacted
            session transcripts, extracted solutions and search text — to model providers outside your infrastructure.
          </p>
          <div className="field field--check">
            <input
              id={`${id}-ext`}
              type="checkbox"
              checked={external}
              aria-describedby={`${id}-ext-warning`}
              onChange={(e) => {
                setExternal(e.target.checked);
                setAcknowledged(false);
              }}
            />
            <label htmlFor={`${id}-ext`}>Approve sending private content to external providers</label>
          </div>
          {approving ? (
            <div className="field field--check">
              <input
                id={`${id}-ack`}
                type="checkbox"
                required
                checked={acknowledged}
                onChange={(e) => setAcknowledged(e.target.checked)}
              />
              <label htmlFor={`${id}-ack`}>
                I understand that private content will leave our infrastructure and be processed by external providers.
              </label>
            </div>
          ) : null}
          {org.external_providers_approved ? (
            <p className="hint">
              Approved {org.external_providers_approved_at ? formatDate(org.external_providers_approved_at) : ''}
              {org.external_providers_approved_by ? (
                <>
                  {' '}
                  by <Mono short={8}>{org.external_providers_approved_by}</Mono>
                </>
              ) : null}
              .
            </p>
          ) : null}
        </fieldset>
        <button
          type="submit"
          className="button button--primary"
          disabled={!dirty || !daysValid || (approving && !acknowledged) || action.pending}
        >
          Save settings
        </button>
      </form>
      <ActionStatus pending={action.pending} error={action.error} success={action.result ? 'Settings saved.' : undefined} pendingText="Saving…" />
    </section>
  );
}

function WebhookSecretsSection() {
  const [secrets, setSecrets] = useState<WebhookSecrets | undefined>();
  const action = useAction((rotate: boolean) => unwrap(api.POST('/v1/admin/webhook-secrets', { body: { rotate } })));
  const reveal = async (rotate: boolean) => {
    const s = await action.run(rotate);
    if (s) setSecrets(s);
    return Boolean(s);
  };
  return (
    <section className="settings-section" aria-labelledby="secrets-title">
      <h2 id="secrets-title">Webhook secrets</h2>
      <p className="hint">Used to sign GitHub and CI webhooks. Revealing or rotating is recorded in the audit log.</p>
      <div className="action-grid">
        <ConfirmAction
          label="Reveal secrets…"
          title="Reveal the webhook secrets?"
          description="The secrets will be shown on this screen. This access is audited."
          confirmLabel="Reveal (audited)"
          pending={action.pending}
          onConfirm={() => reveal(false)}
        />
        <ConfirmAction
          label="Rotate secrets…"
          title="Rotate and reveal new secrets?"
          description="The old secrets stop working immediately; update GitHub and CI right after. This is audited."
          confirmLabel="Rotate secrets"
          tone="danger"
          pending={action.pending}
          onConfirm={() => reveal(true)}
        />
      </div>
      <ActionStatus pending={action.pending} error={action.error} pendingText="Requesting secrets…" />
      {secrets ? (
        <div className="notice notice--secret" role="region" aria-label="Webhook secrets">
          <p className="notice__title">Secret version {secrets.version}</p>
          <dl className="pairs pairs--wide">
            <div>
              <dt>GitHub webhook secret</dt>
              <dd className="copy-row">
                <Mono>{secrets.github_secret}</Mono> <CopyButton value={secrets.github_secret} what="GitHub webhook secret" />
              </dd>
            </div>
            <div>
              <dt>GitHub webhook URL</dt>
              <dd>
                <Mono>{secrets.github_webhook_url}</Mono>
              </dd>
            </div>
            <div>
              <dt>CI webhook secret</dt>
              <dd className="copy-row">
                <Mono>{secrets.ci_secret}</Mono> <CopyButton value={secrets.ci_secret} what="CI webhook secret" />
              </dd>
            </div>
            <div>
              <dt>CI webhook URL</dt>
              <dd>
                <Mono>{secrets.ci_webhook_url}</Mono>
              </dd>
            </div>
          </dl>
          <button type="button" className="button" onClick={() => setSecrets(undefined)}>
            Hide secrets
          </button>
        </div>
      ) : null}
    </section>
  );
}

function SweepReportView({ r }: { r: SweepReport }) {
  return (
    <dl className="pairs" aria-label="Retention sweep report">
      <div>
        <dt>Payloads purged</dt>
        <dd>{formatNumber(r.payloads_purged)}</dd>
      </div>
      <div>
        <dt>Browser sessions expired</dt>
        <dd>{formatNumber(r.web_sessions_expired)}</dd>
      </div>
      <div>
        <dt>Webhook deliveries pruned</dt>
        <dd>{formatNumber(r.deliveries_pruned)}</dd>
      </div>
      <div>
        <dt>Jobs pruned</dt>
        <dd>{formatNumber(r.jobs_pruned)}</dd>
      </div>
    </dl>
  );
}

function Maintenance() {
  const session = useSession();
  const sweep = useAction(() => unwrap(api.POST('/v1/admin/retention/sweep')));
  const [exported, setExported] = useState<string | undefined>();
  const exportAction = useAction(async () => {
    const data = await unwrap(api.GET('/v1/admin/export'));
    const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const name = `antibody-export-${session.organization_slug}-${new Date().toISOString().slice(0, 10)}.json`;
    const a = document.createElement('a');
    a.href = url;
    a.download = name;
    a.rel = 'noopener';
    document.body.appendChild(a);
    a.click();
    a.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    return name;
  });

  return (
    <>
      <section className="settings-section" aria-labelledby="sweep-title">
        <h2 id="sweep-title">Retention sweep</h2>
        <p className="hint">
          Purges expired transcript payloads, browser sessions, old webhook deliveries and finished jobs now instead of waiting
          for the worker.
        </p>
        <ConfirmAction
          label="Run retention sweep…"
          title="Run the retention sweep now?"
          description="Expired data is permanently deleted. The sweep is audited."
          confirmLabel="Run sweep"
          tone="danger"
          pending={sweep.pending}
          onConfirm={async () => Boolean(await sweep.run())}
        />
        <ActionStatus pending={sweep.pending} error={sweep.error} success={sweep.result ? 'Sweep finished.' : undefined} pendingText="Sweeping…" />
        {sweep.result ? <SweepReportView r={sweep.result} /> : null}
      </section>
      <section className="settings-section" aria-labelledby="export-title">
        <h2 id="export-title">Export</h2>
        <p className="hint">
          Downloads the organization's knowledge and metadata as JSON (no raw transcripts, no credentials). Exports are audited.
        </p>
        <button
          type="button"
          className="button"
          disabled={exportAction.pending}
          onClick={() => {
            setExported(undefined);
            void exportAction.run().then((name) => name && setExported(name));
          }}
        >
          Download export (JSON)
        </button>
        <ActionStatus
          pending={exportAction.pending}
          error={exportAction.error}
          success={exported ? `Saved ${exported}.` : undefined}
          pendingText="Preparing export…"
        />
      </section>
    </>
  );
}

export function OrganizationTab() {
  const query = useQuery('settings', () => unwrap(api.GET('/v1/admin/settings')));
  return (
    <>
      <QueryBoundary query={query} label="settings" forbiddenWhat="organization settings">
        {(s) => (
          <>
            <section className="settings-section" aria-labelledby="org-title">
              <h2 id="org-title">Organization</h2>
              <dl className="pairs pairs--wide">
                <div>
                  <dt>Name</dt>
                  <dd>{s.organization.name}</dd>
                </div>
                <div>
                  <dt>Slug</dt>
                  <dd>
                    <Mono>{s.organization.slug}</Mono>
                  </dd>
                </div>
                <div>
                  <dt>Raw capture</dt>
                  <dd>
                    {s.organization.raw_capture_enabled ? (
                      <Badge tone="warn">{`On, ${s.organization.raw_retention_days} days`}</Badge>
                    ) : (
                      <Badge tone="ok">Off (structured only)</Badge>
                    )}
                  </dd>
                </div>
              </dl>
              <h3>Endpoints</h3>
              <Endpoints s={s} />
              <h3>Model providers</h3>
              <p className="hint">Configured on the server (read-only here). No secrets are shown.</p>
              <ProviderList providers={s.providers} />
            </section>
            <CaptureSettings settings={s} onSaved={query.setData} />
          </>
        )}
      </QueryBoundary>
      <WebhookSecretsSection />
      <Maintenance />
      <SessionDeletion />
    </>
  );
}
