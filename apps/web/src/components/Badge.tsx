import type { ReactNode } from 'react';
import type { EvidenceResult, ExtractionStatus, MergeState, SolutionState } from '../api/types';
import { humanize } from '../lib/format';

export type Tone = 'neutral' | 'info' | 'ok' | 'warn' | 'bad' | 'muted';

/** A text badge. The text always carries the meaning; color only reinforces it. */
export function Badge({ tone = 'neutral', children, title }: { tone?: Tone; children: ReactNode; title?: string }) {
  return (
    <span className={`badge badge--${tone}`} title={title}>
      {children}
    </span>
  );
}

const SOLUTION_TONES: Record<SolutionState, Tone> = {
  candidate: 'neutral',
  corroborated: 'info',
  verified: 'ok',
  superseded: 'muted',
  withdrawn: 'bad',
};

const SOLUTION_HINTS: Record<SolutionState, string> = {
  candidate: 'Extracted from an agent session; not yet confirmed',
  corroborated: 'Seen independently more than once; not verified',
  verified: 'Backed by trusted evidence (signed CI or an audited admin action)',
  superseded: 'Replaced by a newer solution',
  withdrawn: 'Withdrawn by an administrator; not suggested to agents',
};

export function StateBadge({ state }: { state: SolutionState }) {
  return (
    <Badge tone={SOLUTION_TONES[state] ?? 'neutral'} title={SOLUTION_HINTS[state]}>
      {humanize(state)}
    </Badge>
  );
}

export function RevalidationBadge() {
  return (
    <Badge tone="warn" title="Something changed since this was verified; it should be checked again">
      Needs revalidation
    </Badge>
  );
}

const EXTRACTION_TONES: Record<ExtractionStatus, Tone> = {
  pending: 'neutral',
  running: 'info',
  succeeded: 'ok',
  failed: 'bad',
  skipped: 'muted',
};

export function ExtractionBadge({ status }: { status: ExtractionStatus }) {
  return <Badge tone={EXTRACTION_TONES[status] ?? 'neutral'}>{humanize(status)}</Badge>;
}

const MERGE_TONES: Record<MergeState, Tone> = { unknown: 'muted', unmerged: 'neutral', merged: 'info', reverted: 'bad' };

export function MergeBadge({ state }: { state: MergeState }) {
  return <Badge tone={MERGE_TONES[state] ?? 'neutral'}>{`Merge: ${state}`}</Badge>;
}

const RESULT_TONES: Record<EvidenceResult, Tone> = { pass: 'ok', fail: 'bad', info: 'neutral' };

export function ResultBadge({ result }: { result: EvidenceResult }) {
  return <Badge tone={RESULT_TONES[result] ?? 'neutral'}>{humanize(result)}</Badge>;
}

export function TrustBadge({ trusted }: { trusted: boolean }) {
  return trusted ? (
    <Badge tone="ok" title="From a signed webhook or an audited administrator action">
      Trusted
    </Badge>
  ) : (
    <Badge tone="warn" title="Reported by an agent or client; never verifies a solution">
      Untrusted claim
    </Badge>
  );
}
