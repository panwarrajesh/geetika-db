import type { Doc } from './lib.ts';

export class ApiError extends Error {
  status: number;
  code: string;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

export interface Org { org_id: string; name: string; role: 'owner' | 'admin' | 'member' }
export interface Project { project_id: string; name: string; environments: string[]; created_ms: number }
export interface Member { user_id: string; email: string | null; role: string }
export interface KeyInfo { key_id: string; name: string; env: string; role: string; created_ms: number; expires_ms: number | null }
export interface NewKey { key_id: string; api_key: string; env: string; role: string }
export interface AuditEvent { ts_ms: number; actor: string; action: string; target: string; detail: string }
export interface Usage {
  used_bytes: number; quota_bytes: number; memtable_entries: number;
  sstable_count: number; sstable_keys: number; wal_bytes: number;
}
export interface QueryBody { filter?: Doc; sort?: { field: string; order: 'asc' | 'desc' }; skip?: number; limit?: number }
export interface QueryResult { documents: Doc[]; count: number; skip: number; limit: number; plan: string }
export interface BackupFile { name: string; size: number; sha256: string }
export interface BackupMeta { id: string; kind: string; created_ms: number; entries: number; bytes: number; files: BackupFile[] }
export interface VerifyReport { ok: boolean; id?: string; entries?: number; files?: number; error?: string }
export interface OpsAlert { level: 'warning' | 'critical'; code: string; message: string }
export interface OpsMetrics {
  uptime_seconds: number; requests_total: number; responses_2xx: number; responses_4xx: number; responses_5xx: number;
  auth_failures: number; rate_limited: number; quota_rejections: number; latency_ms_avg: number; open_project_databases: number;
  storage: { data_dir_bytes: number; backups_bytes: number; budget_bytes: number };
  backups: { ok_total: number; failed_total: number; restores_total: number; last_success_ms: number; last_error: string | null; interval_seconds: number; retention: number };
  alerts: OpsAlert[];
}
export interface OpsHealth { status: 'ok' | 'degraded' | 'critical'; control_database: string; alerts: OpsAlert[] }
export interface Me { user_id: string; email: string; created_ms: number }

const TOKEN_KEY = 'rajeshdb_token';
// sessionStorage: cleared when the tab closes. Tradeoff: readable by any script on this origin,
// so keep the dashboard free of third-party scripts (it has none).
let token: string | null = sessionStorage.getItem(TOKEN_KEY);
let onUnauthorized: () => void = () => {};

export function getToken(): string | null { return token; }
export function setToken(t: string | null): void {
  token = t;
  if (t) sessionStorage.setItem(TOKEN_KEY, t);
  else sessionStorage.removeItem(TOKEN_KEY);
}
export function setUnauthorizedHandler(fn: () => void): void { onUnauthorized = fn; }

function errorParts(data: unknown, status: number): { code: string; message: string } {
  if (data && typeof data === 'object' && 'error' in data) {
    const e = (data as { error?: { code?: unknown; message?: unknown } }).error;
    if (e && typeof e.message === 'string') {
      return { code: typeof e.code === 'string' ? e.code : 'error', message: e.message };
    }
  }
  return { code: 'http_error', message: `Request failed (HTTP ${status})` };
}

async function request<T>(method: string, path: string, body?: unknown, auth = true): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  if (auth && token) headers['Authorization'] = `Bearer ${token}`;
  let res: Response;
  try {
    res = await fetch('/api' + path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  } catch {
    throw new ApiError(0, 'network', 'Cannot reach the RajeshDB server. Is it running?');
  }
  const text = await res.text();
  let data: unknown = null;
  try { data = text ? JSON.parse(text) : null; } catch { /* non-JSON body */ }
  if (!res.ok) {
    const { code, message } = errorParts(data, res.status);
    if (res.status === 401 && auth && token) onUnauthorized();
    throw new ApiError(res.status, code, message);
  }
  return data as T;
}

const enc = encodeURIComponent;
const envBase = (pid: string, env: string) => `/v1/projects/${enc(pid)}/envs/${enc(env)}`;
const collBase = (pid: string, env: string, c: string) => `${envBase(pid, env)}/collections/${enc(c)}`;

export const api = {
  health: () => request<{ status: string }>('GET', '/health', undefined, false),
  register: (email: string, password: string) => request<{ user_id: string }>('POST', '/v1/auth/register', { email, password }, false),
  login: async (email: string, password: string): Promise<void> => {
    const r = await request<{ token: string }>('POST', '/v1/auth/login', { email, password }, false);
    setToken(r.token);
  },
  logout: async (): Promise<void> => {
    try { await request('POST', '/v1/auth/logout'); } finally { setToken(null); }
  },
  me: () => request<Me>('GET', '/v1/me'),

  orgs: async () => (await request<{ organizations: Org[] }>('GET', '/v1/orgs')).organizations,
  createOrg: (name: string) => request<{ org_id: string; name: string }>('POST', '/v1/orgs', { name }),
  members: async (org: string) => (await request<{ members: Member[] }>('GET', `/v1/orgs/${enc(org)}/members`)).members,
  addMember: (org: string, email: string, role: 'admin' | 'member') => request('POST', `/v1/orgs/${enc(org)}/members`, { email, role }),
  removeMember: (org: string, uid: string) => request('DELETE', `/v1/orgs/${enc(org)}/members/${enc(uid)}`),
  audit: async (org: string) => (await request<{ events: AuditEvent[] }>('GET', `/v1/orgs/${enc(org)}/audit`)).events,

  projects: async (org: string) => (await request<{ projects: Project[] }>('GET', `/v1/orgs/${enc(org)}/projects`)).projects,
  createProject: (org: string, name: string) => request<{ project_id: string }>('POST', `/v1/orgs/${enc(org)}/projects`, { name }),
  deleteProject: (pid: string) => request('DELETE', `/v1/projects/${enc(pid)}`),

  keys: async (pid: string) => (await request<{ keys: KeyInfo[] }>('GET', `/v1/projects/${enc(pid)}/keys`)).keys,
  createKey: (pid: string, body: { name: string; env: string; role: string; expires_in_secs?: number }) =>
    request<NewKey>('POST', `/v1/projects/${enc(pid)}/keys`, body),
  revokeKey: (pid: string, kid: string) => request('DELETE', `/v1/projects/${enc(pid)}/keys/${enc(kid)}`),
  rotateKey: (pid: string, kid: string, graceSeconds: number) =>
    request<NewKey>('POST', `/v1/projects/${enc(pid)}/keys/${enc(kid)}/rotate`, graceSeconds > 0 ? { grace_seconds: graceSeconds } : undefined),

  backups: async (pid: string, env: string) => (await request<{ backups: BackupMeta[] }>('GET', `${envBase(pid, env)}/backups`)).backups,
  createBackup: (pid: string, env: string) => request<BackupMeta>('POST', `${envBase(pid, env)}/backups`),
  verifyBackup: (pid: string, env: string, bid: string) => request<VerifyReport>('POST', `${envBase(pid, env)}/backups/${enc(bid)}/verify`),
  restoreBackup: (pid: string, env: string, bid: string) =>
    request<{ restored: string; safety_backup: string }>('POST', `${envBase(pid, env)}/backups/${enc(bid)}/restore`, { confirm: env }),
  deleteBackup: (pid: string, env: string, bid: string) => request('DELETE', `${envBase(pid, env)}/backups/${enc(bid)}`),

  exportCollection: async (pid: string, env: string, c: string): Promise<{ text: string; truncated: boolean }> => {
    let res: Response;
    try {
      res = await fetch('/api' + `${collBase(pid, env, c)}/export`, { headers: token ? { Authorization: `Bearer ${token}` } : {} });
    } catch {
      throw new ApiError(0, 'network', 'Cannot reach the RajeshDB server. Is it running?');
    }
    const text = await res.text();
    if (!res.ok) {
      let data: unknown = null;
      try { data = JSON.parse(text); } catch { /* not JSON */ }
      const { code, message } = errorParts(data, res.status);
      if (res.status === 401 && token) onUnauthorized();
      throw new ApiError(res.status, code, message);
    }
    return { text, truncated: res.headers.get('x-truncated') === 'true' };
  },

  usage: (pid: string, env: string) => request<Usage>('GET', `${envBase(pid, env)}/usage`),
  collections: async (pid: string, env: string) =>
    (await request<{ collections: string[] }>('GET', `${envBase(pid, env)}/collections`)).collections,
  createCollection: (pid: string, env: string, name: string, schema?: Doc) =>
    request('POST', `${envBase(pid, env)}/collections`, schema ? { name, schema } : { name }),
  dropCollection: (pid: string, env: string, c: string) => request('DELETE', collBase(pid, env, c)),
  indexes: async (pid: string, env: string, c: string) =>
    (await request<{ indexes: string[] }>('GET', `${collBase(pid, env, c)}/indexes`)).indexes,
  createIndex: (pid: string, env: string, c: string, field: string) => request('POST', `${collBase(pid, env, c)}/indexes`, { field }),
  dropIndex: (pid: string, env: string, c: string, field: string) => request('DELETE', `${collBase(pid, env, c)}/indexes/${enc(field)}`),
  query: (pid: string, env: string, c: string, body: QueryBody) => request<QueryResult>('POST', `${collBase(pid, env, c)}/query`, body),
  insertDoc: (pid: string, env: string, c: string, doc: Doc) => request<{ _id: string }>('POST', `${collBase(pid, env, c)}/documents`, doc),
  replaceDoc: (pid: string, env: string, c: string, id: string, doc: Doc) => request('PUT', `${collBase(pid, env, c)}/documents/${enc(id)}`, doc),
  deleteDoc: (pid: string, env: string, c: string, id: string) => request('DELETE', `${collBase(pid, env, c)}/documents/${enc(id)}`),
};

/**
 * Operator endpoints (/ops/*) use a separate secret that is NOT a login session. The token is passed
 * in by the caller for each request and is never stored by this module.
 */
export async function opsRequest<T>(method: string, path: string, operatorToken: string): Promise<T> {
  let res: Response;
  try {
    res = await fetch('/api' + path, { method, headers: { Authorization: `Bearer ${operatorToken}` } });
  } catch {
    throw new ApiError(0, 'network', 'Cannot reach the RajeshDB server. Is it running?');
  }
  const text = await res.text();
  let data: unknown = null;
  try { data = text ? JSON.parse(text) : null; } catch { /* non-JSON body */ }
  // /ops/health answers 503 WITH a JSON report when the status is "critical": that is data, not an error.
  if (!res.ok && !(res.status === 503 && data && typeof data === 'object' && 'status' in data)) {
    if (res.status === 404) throw new ApiError(404, 'disabled', 'Operator endpoints are disabled (RAJESHDB_OPERATOR_TOKEN is not set on the server).');
    const { code, message } = errorParts(data, res.status);
    throw new ApiError(res.status, code, message);
  }
  return data as T;
}
