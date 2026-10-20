import type { EvidenceView } from '../../api/types';
import { ExternalLink, Mono, RelativeTime, TableWrap } from '../../components/common';
import { ResultBadge, TrustBadge } from '../../components/Badge';
import { humanize } from '../../lib/format';
import { Section } from './Section';

function EvidenceTable({ rows, caption }: { rows: EvidenceView[]; caption: string }) {
  return (
    <TableWrap label={caption}>
      <table>
        <caption className="visually-hidden">{caption}</caption>
        <thead>
          <tr>
            <th scope="col">Source</th>
            <th scope="col">Kind</th>
            <th scope="col">Result</th>
            <th scope="col">Check</th>
            <th scope="col">Revision</th>
            <th scope="col">Environment</th>
            <th scope="col">Detail</th>
            <th scope="col">Observed</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((e) => (
            <tr key={e.id} className={e.trusted ? 'row--trusted' : 'row--untrusted'}>
              <td>
                <TrustBadge trusted={e.trusted} />
              </td>
              <td>{humanize(e.kind)}</td>
              <td>
                <ResultBadge result={e.result} />
              </td>
              <td>{e.check_name ? <Mono>{e.check_name}</Mono> : '—'}</td>
              <td>{e.tested_revision ? <Mono short={12}>{e.tested_revision}</Mono> : '—'}</td>
              <td>{e.environment ?? '—'}</td>
              <td className="cell--text">
                {e.detail ?? '—'}
                {e.source ? (
                  <>
                    {' '}
                    <ExternalLink href={e.source.url}>{`(${humanize(e.source.kind)}: ${e.source.label})`}</ExternalLink>
                  </>
                ) : null}
              </td>
              <td>
                <RelativeTime iso={e.observed_at} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </TableWrap>
  );
}

export function EvidenceSection({ evidence }: { evidence: EvidenceView[] }) {
  const trusted = evidence.filter((e) => e.trusted);
  const untrusted = evidence.filter((e) => !e.trusted);
  return (
    <Section id="sol-evidence" title="Evidence">
      <div className="evidence-group evidence-group--trusted">
        <h3>
          Trusted evidence <span className="count">({trusted.length})</span>
        </h3>
        <p className="hint">From signed CI webhooks or audited administrator actions. Only this kind can verify a solution.</p>
        {trusted.length === 0 ? (
          <p className="muted">No trusted evidence yet.</p>
        ) : (
          <EvidenceTable rows={trusted} caption="Trusted evidence" />
        )}
      </div>
      <div className="evidence-group evidence-group--untrusted">
        <h3>
          Untrusted claims <span className="count">({untrusted.length})</span>
        </h3>
        <p className="hint">Reported by agents or clients (for example "all tests pass"). Shown for context; never counts as verification.</p>
        {untrusted.length === 0 ? (
          <p className="muted">No agent claims recorded.</p>
        ) : (
          <EvidenceTable rows={untrusted} caption="Untrusted claims" />
        )}
      </div>
    </Section>
  );
}
