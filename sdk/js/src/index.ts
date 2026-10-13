// RajeshDB client for Node 18+/22+ (and other server runtimes). No dependencies.
// SECURITY: an API key is a server-side secret. This client refuses to run in a browser unless you
// explicitly pass allowBrowser: true. Put your own backend between browsers and the database.

export type Doc = Record<string, unknown>;

export class RajeshDBError extends Error {
  status: number;
  code: string;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = 'RajeshDBError';
    this.status = status;
    this.code = code;
  }
}

export interface ClientOptions {
  baseUrl: string;
  apiKey: string;
  project: string;
  env?: 'dev' | 'staging' | 'prod';
  timeoutMs?: number;
  allowBrowser?: boolean;
  fetch?: typeof fetch;
}

export interface QueryOptions {
  filter?: Doc;
  sort?: string | { field: string; order?: 'asc' | 'desc' };
  skip?: number;
  limit?: number;
}

export interface QueryResult {
  documents: Doc[];
  count: number;
  skip: number;
  limit: number;
  plan: string;
}

const enc = encodeURIComponent;

export class RajeshDB {
  baseUrl: string;
  project: string;
  env: string;
  timeoutMs: number;
  apiKey: string;
  fetchImpl: typeof fetch;

  constructor(opts: ClientOptions) {
    if (typeof (globalThis as { window?: unknown }).window !== 'undefined' && !opts.allowBrowser) {
      throw new Error('RajeshDB API keys must not be used in browser code. Call your own backend instead (or pass allowBrowser: true if the key is read-only and you accept the risk).');
    }
    if (!opts.apiKey || !opts.apiKey.startsWith('rdb_')) throw new Error('apiKey must be a project API key starting with "rdb_"');
    if (!/^[0-9a-f]{32}$/.test(opts.project)) throw new Error('project must be the 32-character project id');
    this.baseUrl = opts.baseUrl.replace(/\/+$/, '');
    this.project = opts.project;
    this.env = opts.env ?? 'dev';
    this.timeoutMs = opts.timeoutMs ?? 30000;
    this.apiKey = opts.apiKey;
    this.fetchImpl = opts.fetch ?? fetch;
  }

  /** Never print the key by accident (console.log(client)). */
  toJSON(): Doc {
    return { baseUrl: this.baseUrl, project: this.project, env: this.env, apiKey: '[hidden]' };
  }

  envPath(): string {
    return `/v1/projects/${enc(this.project)}/envs/${enc(this.env)}`;
  }

  async request(method: string, path: string, body?: unknown, allow404 = false): Promise<unknown> {
    const ac = new AbortController();
    const timer = setTimeout(() => ac.abort(), this.timeoutMs);
    let res: Response;
    try {
      res = await this.fetchImpl(this.baseUrl + path, {
        method,
        signal: ac.signal,
        headers: {
          authorization: `Bearer ${this.apiKey}`,
          ...(body !== undefined ? { 'content-type': 'application/json' } : {}),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (e) {
      const aborted = e instanceof Error && e.name === 'AbortError';
      throw new RajeshDBError(0, aborted ? 'timeout' : 'network', aborted ? `request timed out after ${this.timeoutMs} ms` : 'cannot reach the server');
    } finally {
      clearTimeout(timer);
    }
    const text = await res.text();
    let data: unknown = null;
    try { data = text ? JSON.parse(text) : null; } catch { /* non-JSON body */ }
    if (res.status === 404 && allow404) return null;
    if (!res.ok) {
      const err = (data as { error?: { code?: string; message?: string } } | null)?.error;
      throw new RajeshDBError(res.status, err?.code ?? 'http_error', err?.message ?? `HTTP ${res.status}`);
    }
    return data;
  }

  async listCollections(): Promise<string[]> {
    const r = (await this.request('GET', `${this.envPath()}/collections`)) as { collections: string[] };
    return r.collections;
  }

  /** Needs an admin key. */
  async createCollection(name: string, schema?: Doc): Promise<void> {
    await this.request('POST', `${this.envPath()}/collections`, schema ? { name, schema } : { name });
  }

  collection(name: string): Collection {
    return new Collection(this, name);
  }
}

export class Collection {
  db: RajeshDB;
  name: string;
  constructor(db: RajeshDB, name: string) {
    this.db = db;
    this.name = name;
  }

  base(): string {
    return `${this.db.envPath()}/collections/${enc(this.name)}`;
  }

  /** Returns the _id (generated when the document has none). */
  async insert(doc: Doc): Promise<string> {
    const r = (await this.db.request('POST', `${this.base()}/documents`, doc)) as { _id: string };
    return r._id;
  }

  async get(id: string): Promise<Doc | null> {
    return (await this.db.request('GET', `${this.base()}/documents/${enc(id)}`, undefined, true)) as Doc | null;
  }

  async replace(id: string, doc: Doc): Promise<void> {
    await this.db.request('PUT', `${this.base()}/documents/${enc(id)}`, doc);
  }

  /** true if a document was deleted, false if it did not exist. */
  async delete(id: string): Promise<boolean> {
    const r = await this.db.request('DELETE', `${this.base()}/documents/${enc(id)}`, undefined, true);
    return r !== null;
  }

  async find(options: QueryOptions = {}): Promise<QueryResult> {
    return (await this.db.request('POST', `${this.base()}/query`, options)) as QueryResult;
  }

  async findOne(filter: Doc): Promise<Doc | null> {
    const r = await this.find({ filter, limit: 1 });
    return r.documents[0] ?? null;
  }

  /** Shallow-merges `set` into every matching document. An empty filter needs confirmAll: true. */
  async update(filter: Doc, set: Doc, opts: { confirmAll?: boolean } = {}): Promise<number> {
    const r = (await this.db.request('POST', `${this.base()}/update`, { filter, set, ...(opts.confirmAll ? { confirm_all: true } : {}) })) as { modified: number };
    return r.modified;
  }

  async deleteWhere(filter: Doc, opts: { confirmAll?: boolean } = {}): Promise<number> {
    const r = (await this.db.request('POST', `${this.base()}/delete`, { filter, ...(opts.confirmAll ? { confirm_all: true } : {}) })) as { deleted: number };
    return r.deleted;
  }

  /** Needs an admin key. */
  async createIndex(field: string): Promise<void> {
    await this.db.request('POST', `${this.base()}/indexes`, { field });
  }

  async indexes(): Promise<string[]> {
    const r = (await this.db.request('GET', `${this.base()}/indexes`)) as { indexes: string[] };
    return r.indexes;
  }

  /** All documents as newline-delimited JSON text (capped by the server). */
  async exportNdjson(): Promise<string> {
    const res = await this.db.fetchImpl(this.db.baseUrl + `${this.base()}/export`, { headers: { authorization: `Bearer ${this.db.apiKey}` } });
    if (!res.ok) throw new RajeshDBError(res.status, 'http_error', `HTTP ${res.status}`);
    return res.text();
  }
}
