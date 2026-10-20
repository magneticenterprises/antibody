/** Short relative time, e.g. "just now", "5m ago", "3h ago", "2d ago". */
export function formatRelative(iso: string | null | undefined, now: number = Date.now()): string {
  if (!iso) return '—';
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return '—';
  const diff = now - t;
  const future = diff < 0;
  const s = Math.abs(diff) / 1000;
  let text: string;
  // Small differences either way (clock skew, render timing) read as "just now".
  if (s < 45) return 'just now';
  if (s < 3600) text = `${Math.round(s / 60)}m`;
  else if (s < 86_400) text = `${Math.round(s / 3600)}h`;
  else if (s < 86_400 * 60) text = `${Math.round(s / 86_400)}d`;
  else return formatDate(iso);
  return future ? `in ${text}` : `${text} ago`;
}

/** Relative age of a millisecond timestamp. */
export function formatAge(ms: number | undefined, now: number = Date.now()): string {
  if (ms === undefined) return '—';
  return formatRelative(new Date(ms).toISOString(), now);
}

export function formatDate(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '—';
  return d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

export function formatNumber(n: number | null | undefined, digits = 0): string {
  if (n === null || n === undefined || Number.isNaN(n)) return '—';
  return n.toLocaleString(undefined, { maximumFractionDigits: digits });
}

export function formatSeconds(s: number | null | undefined): string {
  if (s === null || s === undefined) return '—';
  if (s < 60) return `${Math.round(s)}s`;
  if (s < 3600) return `${Math.round(s / 60)}m`;
  return `${(s / 3600).toFixed(1)}h`;
}

export function shortId(id: string | null | undefined, n = 8): string {
  if (!id) return '—';
  return id.length > n ? id.slice(0, n) : id;
}

/** Human label for snake_case enum values. */
export function humanize(value: string): string {
  const s = value.replace(/_/g, ' ');
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** Only http(s) URLs may become links; anything else is shown as text. */
export function safeHttpUrl(url: string | null | undefined): string | null {
  if (!url) return null;
  try {
    const u = new URL(url);
    return u.protocol === 'http:' || u.protocol === 'https:' ? u.href : null;
  } catch {
    return null;
  }
}

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export function isUuid(v: string): boolean {
  return UUID_RE.test(v.trim());
}

/** Pretty JSON for untrusted, display-only values (rendered as text). */
export function prettyJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? 'null';
  } catch {
    return String(value);
  }
}
