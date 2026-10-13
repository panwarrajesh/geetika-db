import { useState } from 'react';
import { ApiError, opsRequest } from '../api.ts';
import type { BackupMeta, OpsHealth, OpsMetrics } from '../api.ts';
import { formatBytes, timeAgo } from '../lib.ts';
import { Card, ErrorBox, errMsg, fmtTime } from '../ui.tsx';

interface Snapshot { metrics: OpsMetrics; health: OpsHealth; control: BackupMeta[] }

/**
 * Operator console. The operator token is the server's RAJESHDB_OPERATOR_TOKEN. It is kept only in
 * this component's memory (not in storage) and disappears when you leave the page or reload.
 */
export function OpsPage() {
  const [token, setToken] = useState('');
  const [active, setActive] = useState<string | null>(null); // token that worked
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  async function load(t: string) {
    setBusy(true);
    setError(null);
    try {
      const [metrics, health, control] = await Promise.all([
        opsRequest<OpsMetrics>('GET', '/ops/metrics', t),
        opsRequest<OpsHealth>('GET', '/ops/health', t),
        opsRequest<{ backups: BackupMeta[] }>('GET', '/ops/backups/control', t),
      ]);
      setSnap({ metrics, health, control: control.backups });
      setActive(t);
    } catch (e) {
      setSnap(null);
      setActive(null);
      setError(e instanceof ApiError && e.status === 401 ? 'The operator token was not accepted.' : errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  async function backupNow() {
    if (!active) return;
    setBusy(true);
    setNotice(null);
    try {
      const r = await opsRequest<{ succeeded: number; failed: string[] }>('POST', '/ops/backup/run', active);
      setNotice(`Backup run finished: ${r.succeeded} succeeded, ${r.failed.length} failed${r.failed.length ? ' (' + r.failed.join(', ') + ')' : ''}.`);
      await load(active);
    } catch (e) {
      setError(errMsg(e));
      setBusy(false);
    }
  }

  const m = snap?.metrics;
  return (
    <div className="stack">
      <h2>Operations</h2>
      <Card title="Operator access">
        <p className="muted">
          Server-wide monitoring needs the operator token (environment variable RAJESHDB_OPERATOR_TOKEN on the server). It is not your login.
          The token is kept in memory only. Never expose /ops/* to the public internet.
        </p>
        <ErrorBox message={error} />
        <div className="row">
          <input type="password" autoComplete="off" placeholder="operator token" value={token} onChange={(e) => setToken(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter' && token && !busy) void load(token); }} />
          <button className="btn primary" disabled={busy || !token} onClick={() => void load(token)}>{active ? 'Refresh' : 'Connect'}</button>
          {active && <button className="btn" disabled={busy} onClick={() => void backupNow()}>Run backups now</button>}
        </div>
        {notice && <div className="notice">{notice}</div>}
      </Card>

      {snap && m && (
        <>
          <Card title="Health">
            <p>Status: <span className={`status ${snap.health.status}`}>{snap.health.status.toUpperCase()}</span> · control database: {snap.health.control_database}</p>
            {snap.health.alerts.length === 0 ? <p className="muted">No active alerts.</p> : snap.health.alerts.map((a, i) => (
              <div key={i} className={`alert ${a.level}`}><strong>{a.level}</strong> · {a.message}</div>
            ))}
          </Card>

          <div className="grid">
            <Card title="Traffic since start">
              <table><tbody>
                <tr><td>Uptime</td><td>{Math.floor(m.uptime_seconds / 60)} min</td></tr>
                <tr><td>Requests</td><td>{m.requests_total}</td></tr>
                <tr><td>2xx / 4xx / 5xx</td><td>{m.responses_2xx} / {m.responses_4xx} / {m.responses_5xx}</td></tr>
                <tr><td>Average latency</td><td>{m.latency_ms_avg.toFixed(1)} ms</td></tr>
                <tr><td>Failed logins / bad tokens</td><td>{m.auth_failures}</td></tr>
                <tr><td>Rate-limited requests</td><td>{m.rate_limited}</td></tr>
                <tr><td>Writes refused (quota)</td><td>{m.quota_rejections}</td></tr>
                <tr><td>Open project databases</td><td>{m.open_project_databases}</td></tr>
              </tbody></table>
            </Card>
            <Card title="Storage">
              <table><tbody>
                <tr><td>Data directory</td><td>{formatBytes(m.storage.data_dir_bytes)}</td></tr>
                <tr><td>Backups</td><td>{formatBytes(m.storage.backups_bytes)}</td></tr>
                <tr><td>Configured budget</td><td>{m.storage.budget_bytes > 0 ? formatBytes(m.storage.budget_bytes) : 'not set'}</td></tr>
              </tbody></table>
              <p className="muted">Sizes are cached up to 30 s. The budget is a limit you configure; free space of the physical disk is not measured.</p>
            </Card>
            <Card title="Backups">
              <table><tbody>
                <tr><td>Schedule</td><td>{m.backups.interval_seconds > 0 ? `every ${Math.round(m.backups.interval_seconds / 60)} min` : 'automatic backups are OFF'}</td></tr>
                <tr><td>Retention</td><td>{m.backups.retention} per database</td></tr>
                <tr><td>Last success</td><td>{timeAgo(Date.now(), m.backups.last_success_ms)}</td></tr>
                <tr><td>Succeeded / failed / restores</td><td>{m.backups.ok_total} / {m.backups.failed_total} / {m.backups.restores_total}</td></tr>
                <tr><td>Last error</td><td>{m.backups.last_error ?? 'none'}</td></tr>
              </tbody></table>
            </Card>
          </div>

          <Card title="Control database backups (users, organizations, projects, keys)">
            {snap.control.length === 0 ? <p className="muted">None yet. Click "Run backups now".</p> : (
              <table>
                <thead><tr><th>Created</th><th>Records</th><th>Size</th><th>Id</th></tr></thead>
                <tbody>{snap.control.map((b) => (
                  <tr key={b.id}><td>{fmtTime(b.created_ms)}</td><td>{b.entries}</td><td>{formatBytes(b.bytes)}</td><td className="mono">{b.id}</td></tr>
                ))}</tbody>
              </table>
            )}
            <p className="muted">Restoring the control database is an offline procedure: see docs/OPERATIONS.md.</p>
          </Card>
        </>
      )}

      <Card title="Not available yet">
        <ul>
          <li>Billing and plan management</li>
          <li>SQL editor</li>
          <li>Off-site copies of backups and point-in-time recovery (backups are snapshots; copy the backups folder off the machine yourself, see docs/OPERATIONS.md)</li>
        </ul>
      </Card>
    </div>
  );
}
