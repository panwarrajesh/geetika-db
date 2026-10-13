import { useState } from 'react';
import { api } from '../api.ts';
import type { BackupMeta, QueryBody, VerifyReport } from '../api.ts';
import { buildFilter, cellText, columnsOf, formatBytes, nextSort, parseJsonObject, pretty, timeAgo } from '../lib.ts';
import type { Doc, Op, SortState } from '../lib.ts';
import { link } from '../router.ts';
import { Card, ErrorBox, Loading, Meter, Modal, Tabs, errMsg, useAsync } from '../ui.tsx';

type Tab = 'data' | 'query' | 'usage' | 'backups';

export function EnvPage({ org, pid, env }: { org: string; pid: string; env: string }) {
  const orgs = useAsync(() => api.orgs(), []);
  const [tab, setTab] = useState<Tab>('data');
  if (orgs.loading && !orgs.data) return <Loading />;
  const me = orgs.data?.find((o) => o.org_id === org);
  if (!me) return <ErrorBox message={orgs.error ?? 'Organization not found.'} />;
  const canWrite = me.role === 'owner' || me.role === 'admin';
  const isOwner = me.role === 'owner';
  const tabs: { id: Tab; label: string }[] = [
    { id: 'data', label: 'Data browser' },
    { id: 'query', label: 'Query editor' },
    { id: 'usage', label: 'Storage & limits' },
    ...(canWrite ? [{ id: 'backups' as Tab, label: 'Backups' }] : []),
  ];
  return (
    <div className="stack">
      <p className="muted"><a href={link('org', org, 'project', pid)}>← project</a></p>
      <h2>Environment: {env}</h2>
      <Tabs tabs={tabs} value={tab} onChange={setTab} />
      {tab === 'data' && <DataBrowser pid={pid} env={env} canWrite={canWrite} />}
      {tab === 'query' && <QueryEditor pid={pid} env={env} />}
      {tab === 'usage' && <UsagePanel pid={pid} env={env} />}
      {tab === 'backups' && canWrite && <BackupsPanel pid={pid} env={env} isOwner={isOwner} />}
    </div>
  );
}

// ------------------------------------------------------------------ usage

function UsagePanel({ pid, env }: { pid: string; env: string }) {
  const u = useAsync(() => api.usage(pid, env), [pid, env]);
  return (
    <Card title="Storage (measured on disk)" actions={<button className="btn" onClick={u.reload}>Refresh</button>}>
      <ErrorBox message={u.error} />
      {u.loading && !u.data ? <Loading /> : u.data && (
        <>
          <Meter used={u.data.used_bytes} total={u.data.quota_bytes} />
          <p>{formatBytes(u.data.used_bytes)} of {formatBytes(u.data.quota_bytes)} quota used. When the quota is reached, writes are refused (HTTP 507); reads and deletes keep working.</p>
          <table>
            <tbody>
              <tr><td>Documents waiting in memory (memtable entries)</td><td>{u.data.memtable_entries}</td></tr>
              <tr><td>On-disk table files</td><td>{u.data.sstable_count}</td></tr>
              <tr><td>Keys in on-disk tables</td><td>{u.data.sstable_keys}</td></tr>
              <tr><td>Write-ahead log size</td><td>{formatBytes(u.data.wal_bytes)}</td></tr>
            </tbody>
          </table>
        </>
      )}
    </Card>
  );
}

// ------------------------------------------------------------------ data browser

function DataBrowser({ pid, env, canWrite }: { pid: string; env: string; canWrite: boolean }) {
  const cols = useAsync(() => api.collections(pid, env), [pid, env]);
  const [sel, setSel] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [schema, setSchema] = useState('');
  const [error, setError] = useState<string | null>(null);

  async function create() {
    setError(null);
    let sch: Doc | undefined;
    if (schema.trim()) {
      const p = parseJsonObject(schema);
      if (!p.ok) { setError('Schema: ' + p.error); return; }
      sch = p.value;
    }
    try {
      await api.createCollection(pid, env, name.trim(), sch);
      setSel(name.trim());
      setName('');
      setSchema('');
      cols.reload();
    } catch (e) { setError(errMsg(e)); }
  }

  return (
    <div className="split">
      <aside className="card">
        <h3>Collections</h3>
        <ErrorBox message={cols.error ?? error} />
        {cols.loading && !cols.data ? <Loading /> : (
          <ul className="list">
            {cols.data?.length === 0 && <li className="muted">None yet.</li>}
            {cols.data?.map((c) => (
              <li key={c}><button className={c === sel ? 'list-btn active' : 'list-btn'} onClick={() => setSel(c)}>{c}</button></li>
            ))}
          </ul>
        )}
        {canWrite && (
          <div className="stack small">
            <input placeholder="new collection" value={name} onChange={(e) => setName(e.target.value)} />
            <textarea rows={3} placeholder='optional schema, e.g. {"fields":{"name":{"type":"string","required":true}}}' value={schema} onChange={(e) => setSchema(e.target.value)} />
            <button className="btn primary" disabled={!name.trim()} onClick={() => void create()}>Create collection</button>
          </div>
        )}
      </aside>
      <div>
        {sel ? (
          <CollectionView
            key={sel}
            pid={pid}
            env={env}
            coll={sel}
            canWrite={canWrite}
            onDropped={() => { setSel(null); cols.reload(); }}
          />
        ) : <Card><p className="muted">Select or create a collection.</p></Card>}
      </div>
    </div>
  );
}

interface CollProps { pid: string; env: string; coll: string; canWrite: boolean; onDropped: () => void }

function CollectionView({ pid, env, coll, canWrite, onDropped }: CollProps) {
  const [mode, setMode] = useState<'quick' | 'json'>('quick');
  const [qField, setQField] = useState('');
  const [qOp, setQOp] = useState<Op>('eq');
  const [qValue, setQValue] = useState('');
  const [jsonText, setJsonText] = useState('{}');
  const [applied, setApplied] = useState<Doc>({});
  const [filterError, setFilterError] = useState<string | null>(null);
  const [sort, setSort] = useState<SortState>(null);
  const [page, setPage] = useState(0);
  const [size, setSize] = useState(25);
  const [editing, setEditing] = useState<{ doc: Doc | null } | null>(null);

  const appliedKey = JSON.stringify(applied);
  const data = useAsync(() => {
    const body: QueryBody = { filter: applied, skip: page * size, limit: size };
    if (sort) body.sort = { field: sort.field, order: sort.order };
    return api.query(pid, env, coll, body);
  }, [pid, env, coll, appliedKey, sort?.field, sort?.order, page, size]);
  const idx = useAsync(() => api.indexes(pid, env, coll), [pid, env, coll]);
  const [newIndex, setNewIndex] = useState('');
  const [actionError, setActionError] = useState<string | null>(null);

  function apply() {
    setFilterError(null);
    if (mode === 'quick') {
      setApplied(buildFilter(qField, qOp, qValue));
    } else {
      const p = parseJsonObject(jsonText);
      if (!p.ok) { setFilterError(p.error); return; }
      setApplied(p.value);
    }
    setPage(0);
  }
  function clear() {
    setApplied({}); setQField(''); setQValue(''); setJsonText('{}'); setFilterError(null); setPage(0);
  }

  async function addIndex() {
    setActionError(null);
    try { await api.createIndex(pid, env, coll, newIndex.trim()); setNewIndex(''); idx.reload(); data.reload(); } catch (e) { setActionError(errMsg(e)); }
  }
  async function dropIndex(f: string) {
    if (!window.confirm(`Drop index on "${f}"?`)) return;
    try { await api.dropIndex(pid, env, coll, f); idx.reload(); data.reload(); } catch (e) { setActionError(errMsg(e)); }
  }
  async function dropCollection() {
    if (!window.confirm(`Delete collection "${coll}" and every document in it?`)) return;
    try { await api.dropCollection(pid, env, coll); onDropped(); } catch (e) { setActionError(errMsg(e)); }
  }

  async function exportData() {
    setActionError(null);
    try {
      const r = await api.exportCollection(pid, env, coll);
      const url = URL.createObjectURL(new Blob([r.text], { type: 'application/x-ndjson' }));
      const a = document.createElement('a');
      a.href = url;
      a.download = `${coll}.ndjson`;
      a.click();
      URL.revokeObjectURL(url);
      if (r.truncated) setActionError('The export was cut off at the server limit. Export a filtered subset or ask the operator to raise the limit.');
    } catch (e) { setActionError(errMsg(e)); }
  }

  const docs = data.data?.documents ?? [];
  const columns = columnsOf(docs);
  const hasNext = data.data ? data.data.count === data.data.limit : false;

  return (
    <div className="stack">
      <Card
        title={coll}
        actions={
          <>
            <button className="btn" onClick={() => void exportData()}>Export</button>
            {canWrite && (
              <>
                <button className="btn primary" onClick={() => setEditing({ doc: null })}>New document</button>
                <button className="btn danger" onClick={() => void dropCollection()}>Drop</button>
              </>
            )}
          </>
        }
      >
        <ErrorBox message={actionError} />
        <div className="row wrap">
          <select value={mode} onChange={(e) => setMode(e.target.value as 'quick' | 'json')}>
            <option value="quick">Quick filter</option>
            <option value="json">JSON filter</option>
          </select>
          {mode === 'quick' ? (
            <>
              <input placeholder="field (e.g. city or address.city)" value={qField} onChange={(e) => setQField(e.target.value)} />
              <select value={qOp} onChange={(e) => setQOp(e.target.value as Op)}>
                <option value="eq">=</option><option value="ne">≠</option><option value="gt">&gt;</option>
                <option value="gte">≥</option><option value="lt">&lt;</option><option value="lte">≤</option>
              </select>
              <input placeholder="value" value={qValue} onChange={(e) => setQValue(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') apply(); }} />
            </>
          ) : (
            <textarea className="grow" rows={2} value={jsonText} onChange={(e) => setJsonText(e.target.value)} />
          )}
          <button className="btn primary" onClick={apply}>Apply</button>
          <button className="btn" onClick={clear}>Clear</button>
        </div>
        <ErrorBox message={filterError} />
      </Card>

      <Card>
        <ErrorBox message={data.error} />
        {data.loading && !data.data ? <Loading /> : docs.length === 0 ? <p className="muted">No documents match.</p> : (
          <div className="scroll">
            <table className="data">
              <thead>
                <tr>
                  {columns.map((c) => (
                    <th key={c}>
                      <button className="th-btn" onClick={() => { setSort(nextSort(sort, c)); setPage(0); }}>
                        {c}{sort?.field === c ? (sort.order === 'asc' ? ' ▲' : ' ▼') : ''}
                      </button>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {docs.map((d, i) => (
                  <tr key={String(d['_id'] ?? i)} className="clickable" onClick={() => setEditing({ doc: d })}>
                    {columns.map((c) => <td key={c}>{cellText(d[c])}</td>)}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        <div className="row between">
          <span className="muted">
            {data.data ? `Rows ${page * size + (docs.length ? 1 : 0)}–${page * size + docs.length} · plan: ${data.data.plan}` : ''}
          </span>
          <span className="row">
            <select value={size} onChange={(e) => { setSize(Number(e.target.value)); setPage(0); }}>
              {[10, 25, 50, 100].map((n) => <option key={n} value={n}>{n} / page</option>)}
            </select>
            <button className="btn" disabled={page === 0} onClick={() => setPage(page - 1)}>Previous</button>
            <button className="btn" disabled={!hasNext} onClick={() => setPage(page + 1)}>Next</button>
          </span>
        </div>
      </Card>

      <Card title="Indexes">
        <ErrorBox message={idx.error} />
        <div className="row wrap">
          {idx.data?.length === 0 && <span className="muted">No indexes. Queries scan the whole collection.</span>}
          {idx.data?.map((f) => (
            <span key={f} className="badge">{f}{canWrite && <button className="x" aria-label={`drop index ${f}`} onClick={() => void dropIndex(f)}>✕</button>}</span>
          ))}
        </div>
        {canWrite && (
          <div className="row">
            <input placeholder="field to index" value={newIndex} onChange={(e) => setNewIndex(e.target.value)} />
            <button className="btn" disabled={!newIndex.trim()} onClick={() => void addIndex()}>Create index</button>
          </div>
        )}
      </Card>

      {editing && (
        <DocEditor
          pid={pid}
          env={env}
          coll={coll}
          initial={editing.doc}
          canWrite={canWrite}
          onClose={() => setEditing(null)}
          onChanged={() => { setEditing(null); data.reload(); }}
        />
      )}
    </div>
  );
}

function DocEditor(props: { pid: string; env: string; coll: string; initial: Doc | null; canWrite: boolean; onClose: () => void; onChanged: () => void }) {
  const { pid, env, coll, initial, canWrite } = props;
  const [text, setText] = useState(initial ? pretty(initial) : '{\n  \n}');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function save() {
    const p = parseJsonObject(text);
    if (!p.ok) { setError(p.error); return; }
    setBusy(true);
    setError(null);
    try {
      if (initial) {
        const id = initial['_id'];
        if (typeof id !== 'string') throw new Error('This document has no string _id.');
        await api.replaceDoc(pid, env, coll, id, p.value);
      } else {
        await api.insertDoc(pid, env, coll, p.value);
      }
      props.onChanged();
    } catch (e) { setError(errMsg(e)); } finally { setBusy(false); }
  }
  async function remove() {
    const id = initial?.['_id'];
    if (typeof id !== 'string' || !window.confirm('Delete this document?')) return;
    setBusy(true);
    try { await api.deleteDoc(pid, env, coll, id); props.onChanged(); } catch (e) { setError(errMsg(e)); setBusy(false); }
  }

  return (
    <Modal title={initial ? 'Document' : 'New document'} onClose={props.onClose}>
      <ErrorBox message={error} />
      <textarea className="code" rows={16} value={text} readOnly={!canWrite} onChange={(e) => setText(e.target.value)} spellCheck={false} />
      <div className="row between">
        <span>{canWrite && initial && <button className="btn danger" disabled={busy} onClick={() => void remove()}>Delete</button>}</span>
        <span className="row">
          <button className="btn" onClick={props.onClose}>Close</button>
          {canWrite && <button className="btn primary" disabled={busy} onClick={() => void save()}>{initial ? 'Save changes' : 'Insert'}</button>}
        </span>
      </div>
    </Modal>
  );
}

// ------------------------------------------------------------------ query editor

function QueryEditor({ pid, env }: { pid: string; env: string }) {
  const cols = useAsync(() => api.collections(pid, env), [pid, env]);
  const [coll, setColl] = useState('');
  const [text, setText] = useState('{\n  "filter": {},\n  "sort": "_id",\n  "limit": 20\n}');
  const [result, setResult] = useState<{ plan: string; count: number; body: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const chosen = coll || cols.data?.[0] || '';

  async function run() {
    const p = parseJsonObject(text);
    if (!p.ok) { setError(p.error); return; }
    if (!chosen) { setError('Create a collection first.'); return; }
    setBusy(true);
    setError(null);
    try {
      const r = await api.query(pid, env, chosen, p.value as unknown as QueryBody);
      setResult({ plan: r.plan, count: r.count, body: pretty(r.documents) });
    } catch (e) { setError(errMsg(e)); setResult(null); } finally { setBusy(false); }
  }

  return (
    <Card title="Query editor (read-only)">
      <p className="muted">
        JSON query: <code>filter</code> (equality, $eq $ne $gt $gte $lt $lte $in $nin $exists, $and, $or, dotted paths),
        <code> sort</code>, <code>skip</code>, <code>limit</code>. SQL is not supported yet. Changes are made in the data browser.
      </p>
      <ErrorBox message={cols.error ?? error} />
      <div className="row">
        <select value={chosen} onChange={(e) => setColl(e.target.value)}>
          {cols.data?.map((c) => <option key={c}>{c}</option>)}
        </select>
        <button className="btn primary" disabled={busy} onClick={() => void run()}>Run</button>
      </div>
      <textarea className="code" rows={8} value={text} onChange={(e) => setText(e.target.value)} spellCheck={false} />
      {result && (
        <>
          <p><strong>{result.count}</strong> document(s) · plan: <code>{result.plan}</code></p>
          <pre className="result">{result.body}</pre>
        </>
      )}
    </Card>
  );
}


// ------------------------------------------------------------------ backups

function BackupsPanel({ pid, env, isOwner }: { pid: string; env: string; isOwner: boolean }) {
  const list = useAsync(() => api.backups(pid, env), [pid, env]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [reports, setReports] = useState<Record<string, VerifyReport>>({});

  async function run(label: string, fn: () => Promise<void>) {
    setBusy(label);
    setError(null);
    setNotice(null);
    try { await fn(); } catch (e) { setError(errMsg(e)); } finally { setBusy(null); }
  }

  const create = () => run('create', async () => {
    const m = await api.createBackup(pid, env);
    setNotice(`Backup ${m.id} created (${m.entries} records, ${formatBytes(m.bytes)}).`);
    list.reload();
  });
  const verify = (b: BackupMeta) => run('verify-' + b.id, async () => {
    const r = await api.verifyBackup(pid, env, b.id);
    setReports((prev) => ({ ...prev, [b.id]: r }));
  });
  const restore = (b: BackupMeta) => {
    const typed = window.prompt(
      `Restoring REPLACES all current data in "${env}" with this backup (${new Date(b.created_ms).toLocaleString()}).\n` +
      `A safety backup of the current data is made first.\nType the environment name (${env}) to confirm:`,
    );
    if (typed !== env) { if (typed !== null) setError('Confirmation did not match; nothing was restored.'); return; }
    void run('restore-' + b.id, async () => {
      const r = await api.restoreBackup(pid, env, b.id);
      setNotice(`Restored ${r.restored}. The previous data was saved as backup ${r.safety_backup}.`);
      list.reload();
    });
  };
  const remove = (b: BackupMeta) => {
    if (!window.confirm(`Permanently delete backup ${b.id}?`)) return;
    void run('delete-' + b.id, async () => { await api.deleteBackup(pid, env, b.id); list.reload(); });
  };

  return (
    <Card title="Backups" actions={<button className="btn primary" disabled={busy !== null} onClick={() => void create()}>{busy === 'create' ? 'Working…' : 'Back up now'}</button>}>
      <p className="muted">
        Each backup is a checksummed copy of this environment ({env}). Automatic backups run on the server schedule; the newest ones are kept.
        Verify performs a full test restore into a scratch folder without touching live data.
        {isOwner ? '' : ' Only the organization owner can restore or delete backups.'}
      </p>
      <ErrorBox message={list.error ?? error} />
      {notice && <div className="notice">{notice}</div>}
      {list.loading && !list.data ? <Loading /> : list.data && list.data.length === 0 ? <p className="muted">No backups yet.</p> : (
        <div className="scroll">
          <table>
            <thead><tr><th>Created</th><th>Records</th><th>Size</th><th>Id</th><th>Check</th><th /></tr></thead>
            <tbody>
              {list.data?.map((b) => {
                const r = reports[b.id];
                return (
                  <tr key={b.id}>
                    <td>{timeAgo(Date.now(), b.created_ms)}<br /><span className="muted">{new Date(b.created_ms).toLocaleString()}</span></td>
                    <td>{b.entries}</td>
                    <td>{formatBytes(b.bytes)}</td>
                    <td className="mono">{b.id}</td>
                    <td>{r ? (r.ok ? <span className="badge ok">verified</span> : <span className="badge bad" title={r.error}>FAILED: {r.error}</span>) : <span className="muted">not checked</span>}</td>
                    <td className="right">
                      <button className="btn" disabled={busy !== null} onClick={() => void verify(b)}>{busy === 'verify-' + b.id ? 'Checking…' : 'Verify'}</button>{' '}
                      {isOwner && <button className="btn danger" disabled={busy !== null} onClick={() => restore(b)}>Restore</button>}{' '}
                      {isOwner && <button className="btn ghost" disabled={busy !== null} onClick={() => remove(b)}>Delete</button>}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}
