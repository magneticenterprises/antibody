import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { SOLUTION_STATES } from '../api/types';
import { ExtractionBadge, RevalidationBadge, StateBadge, TrustBadge } from './Badge';

describe('state badges', () => {
  it.each(SOLUTION_STATES)('renders a text label for %s (color is not the only signal)', (state) => {
    render(<StateBadge state={state} />);
    const label = state.charAt(0).toUpperCase() + state.slice(1);
    const badge = screen.getByText(label);
    expect(badge).toHaveClass('badge');
    expect(badge).toHaveAttribute('title');
  });

  it('uses distinct tones for verified and withdrawn', () => {
    render(
      <>
        <StateBadge state="verified" />
        <StateBadge state="withdrawn" />
      </>,
    );
    expect(screen.getByText('Verified')).toHaveClass('badge--ok');
    expect(screen.getByText('Withdrawn')).toHaveClass('badge--bad');
  });

  it('labels trust explicitly', () => {
    render(
      <>
        <TrustBadge trusted />
        <TrustBadge trusted={false} />
      </>,
    );
    expect(screen.getByText('Trusted')).toBeInTheDocument();
    expect(screen.getByText('Untrusted claim')).toBeInTheDocument();
  });

  it('labels extraction status and revalidation', () => {
    render(
      <>
        <ExtractionBadge status="failed" />
        <RevalidationBadge />
      </>,
    );
    expect(screen.getByText('Failed')).toHaveClass('badge--bad');
    expect(screen.getByText('Needs revalidation')).toBeInTheDocument();
  });
});
