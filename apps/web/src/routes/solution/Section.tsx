import type { ReactNode } from 'react';

export function Section({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <section className="detail-section" aria-labelledby={id}>
      <h2 id={id}>{title}</h2>
      {children}
    </section>
  );
}

export function TagList({ items, mono, empty }: { items: string[]; mono?: boolean; empty: string }) {
  if (items.length === 0) return <p className="muted">{empty}</p>;
  return (
    <ul className="tag-list">
      {items.map((t) => (
        <li key={t}>{mono ? <code className="mono">{t}</code> : t}</li>
      ))}
    </ul>
  );
}
