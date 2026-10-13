// Pure helpers (no React, no DOM) so they can be unit-tested with plain Node.

export type Doc = Record<string, unknown>;
export type Op = 'eq' | 'ne' | 'gt' | 'gte' | 'lt' | 'lte';
export type SortState = { field: string; order: 'asc' | 'desc' } | null;

export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return '—';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${v} B` : `${v.toFixed(v >= 100 ? 0 : 1)} ${units[i]}`;
}

export type ParsedObject = { ok: true; value: Doc } | { ok: false; error: string };

export function parseJsonObject(text: string): ParsedObject {
  const t = text.trim();
  if (t === '') return { ok: true, value: {} };
  let v: unknown;
  try {
    v = JSON.parse(t);
  } catch (e) {
    return { ok: false, error: `Not valid JSON: ${e instanceof Error ? e.message : String(e)}` };
  }
  if (v === null || typeof v !== 'object' || Array.isArray(v)) {
    return { ok: false, error: 'Must be a JSON object, e.g. {"city": "Delhi"}' };
  }
  return { ok: true, value: v as Doc };
}

/** "42" -> 42, "true" -> true, "null" -> null, "\"x\"" -> "x", "abc" -> "abc". Objects/arrays stay strings. */
export function parseScalar(text: string): unknown {
  const t = text.trim();
  if (t === '') return '';
  try {
    const v: unknown = JSON.parse(t);
    if (v === null || typeof v === 'number' || typeof v === 'boolean' || typeof v === 'string') return v;
  } catch {
    /* fall through: treat as plain text */
  }
  return t;
}

export function buildFilter(field: string, op: Op, valueText: string): Doc {
  const f = field.trim();
  if (f === '') return {};
  const value = parseScalar(valueText);
  if (op === 'eq') return { [f]: value };
  return { [f]: { ['$' + op]: value } };
}

/** `_id` first, then other top-level keys in first-seen order, at most `max` columns. */
export function columnsOf(docs: Doc[], max = 8): string[] {
  const cols: string[] = [];
  const seen = new Set<string>();
  const add = (k: string) => {
    if (!seen.has(k) && cols.length < max) {
      seen.add(k);
      cols.push(k);
    }
  };
  if (docs.some((d) => '_id' in d)) add('_id');
  for (const d of docs) for (const k of Object.keys(d)) add(k);
  return cols;
}

export function cellText(v: unknown, maxLen = 80): string {
  if (v === undefined) return '';
  const s = typeof v === 'string' ? v : JSON.stringify(v);
  return s.length > maxLen ? s.slice(0, maxLen - 1) + '…' : s;
}

/** Click on a column header: asc -> desc -> none. */
export function nextSort(cur: SortState, field: string): SortState {
  if (!cur || cur.field !== field) return { field, order: 'asc' };
  if (cur.order === 'asc') return { field, order: 'desc' };
  return null;
}

export function percent(used: number, total: number): number {
  if (!(total > 0)) return 0;
  return Math.max(0, Math.min(100, (used / total) * 100));
}

export function pretty(v: unknown): string {
  return JSON.stringify(v, null, 2);
}

/** "5 minutes ago", "2 days ago", "just now". `thenMs` of 0/undefined -> "never". */
export function timeAgo(nowMs: number, thenMs: number | null | undefined): string {
  if (!thenMs) return 'never';
  const sec = Math.max(0, Math.floor((nowMs - thenMs) / 1000));
  if (sec < 10) return 'just now';
  if (sec < 60) return `${sec} seconds ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min} minute${min === 1 ? '' : 's'} ago`;
  const hr = Math.floor(min / 60);
  if (hr < 48) return `${hr} hour${hr === 1 ? '' : 's'} ago`;
  const day = Math.floor(hr / 24);
  return `${day} days ago`;
}
