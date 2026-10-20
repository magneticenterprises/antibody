import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { Link, useNavigationType } from 'react-router';
import { useNow } from '../hooks/misc';
import { formatDate, formatRelative, safeHttpUrl } from '../lib/format';

/** External link restricted to http(s); other URLs render as inert text. */
export function ExternalLink({ href, children }: { href: string | null | undefined; children: ReactNode }) {
  const safe = safeHttpUrl(href);
  if (!safe) return <span>{children}</span>;
  return (
    <a href={safe} target="_blank" rel="noopener noreferrer" className="external">
      {children}
      <span className="visually-hidden"> (opens in a new tab)</span>
    </a>
  );
}

export function RelativeTime({ iso, fallback = '—' }: { iso: string | null | undefined; fallback?: string }) {
  const now = useNow(30_000);
  if (!iso) return <span>{fallback}</span>;
  return (
    <time dateTime={iso} title={formatDate(iso)}>
      {formatRelative(iso, now)}
    </time>
  );
}

/** Monospace identifier (SHA, signature, token prefix), optionally shortened. */
export function Mono({ children, short }: { children: string | null | undefined; short?: number }) {
  if (!children) return <span>—</span>;
  const text = short && children.length > short ? children.slice(0, short) : children;
  return (
    <code className="mono" title={text !== children ? children : undefined}>
      {text}
    </code>
  );
}

/** Page heading; moves focus to the heading after in-app navigation (not on first load). */
export function PageHeader({ title, lede, actions }: { title: string; lede?: ReactNode; actions?: ReactNode }) {
  const ref = useRef<HTMLHeadingElement>(null);
  const navigationType = useNavigationType();
  useEffect(() => {
    if (navigationType !== 'POP') ref.current?.focus();
    // Only on mount: a new page was rendered.
  }, []);
  return (
    <header className="page-header">
      <div>
        <h1 ref={ref} tabIndex={-1}>
          {title}
        </h1>
        {lede ? <p className="lede">{lede}</p> : null}
      </div>
      {actions ? <div className="page-header__actions">{actions}</div> : null}
    </header>
  );
}

export interface TabItem {
  key: string;
  to: string;
  label: string;
}

/** Secondary navigation rendered as tabs (real links, keyboard friendly). */
export function TabNav({ items, label, active }: { items: TabItem[]; label: string; active: string }) {
  return (
    <nav className="tabs" aria-label={label}>
      <ul>
        {items.map((item) => (
          <li key={item.key}>
            <Link
              to={item.to}
              className={item.key === active ? 'tab tab--active' : 'tab'}
              aria-current={item.key === active ? 'page' : undefined}
            >
              {item.label}
            </Link>
          </li>
        ))}
      </ul>
    </nav>
  );
}

/** Copy a value to the clipboard with an announced result. */
export function CopyButton({ value, label = 'Copy', what }: { value: string; label?: string; what?: string }) {
  const [state, setState] = useState<'idle' | 'copied' | 'failed'>('idle');
  useEffect(() => {
    if (state === 'idle') return;
    const t = window.setTimeout(() => setState('idle'), 3000);
    return () => window.clearTimeout(t);
  }, [state]);
  return (
    <>
      <button
        type="button"
        className="button button--quiet"
        aria-label={what ? `${state === 'copied' ? 'Copied' : 'Copy'} ${what}` : undefined}
        onClick={() => {
          const write = navigator.clipboard?.writeText(value);
          if (!write) {
            setState('failed');
            return;
          }
          write.then(
            () => setState('copied'),
            () => setState('failed'),
          );
        }}
      >
        {state === 'copied' ? 'Copied' : label}
      </button>
      <span className="visually-hidden" role="status" aria-live="polite">
        {state === 'copied' ? 'Copied to clipboard' : state === 'failed' ? 'Copy failed; select the text instead' : ''}
      </span>
    </>
  );
}

/** Horizontally scrollable, keyboard-focusable wrapper for wide tables. */
export function TableWrap({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="table-wrap" role="region" aria-label={label} tabIndex={0}>
      {children}
    </div>
  );
}

/** Multi-line untrusted text, rendered as plain text only. */
export function PlainText({ children, className }: { children: string | null | undefined; className?: string }) {
  if (!children) return <p className="muted">—</p>;
  return <div className={`plain-text ${className ?? ''}`}>{children}</div>;
}

export function useFieldId(prefix: string): string {
  return `${prefix}-${useId().replace(/:/g, '')}`;
}
