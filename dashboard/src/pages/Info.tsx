import { useState } from 'react';
import { api } from '../api.ts';
import { Card, ErrorBox, Loading, errMsg, fmtTime, useAsync } from '../ui.tsx';

export function DocsPage() {
  return (
    <div className="stack">
      <h2>Documentation</h2>
      <Card title="Connecting an application">
        <p>Create a project, then an API key for one environment (Project → API keys). Use the key from your <strong>server</strong>, never from browser code that you ship to other people.</p>
        <pre className="result">{`curl -X POST http://HOST/v1/projects/<project_id>/envs/dev/collections/users/query \\
  -H "Authorization: Bearer rdb_..." \\
  -H "Content-Type: application/json" \\
  -d '{"filter": {"city": "Delhi"}, "limit": 10}'`}</pre>
      </Card>
      <Card title="Reference">
        <ul>
          <li>Full API: <code>docs/openapi-platform.yaml</code> in the repository.</li>
          <li>What works today and known limits: <code>docs/STATUS.md</code>.</li>
          <li>Roles: <em>read</em> (query/get), <em>write</em> (insert/update/delete documents), <em>admin</em> (collections, indexes).</li>
          <li>Each environment has its own storage quota; at the limit writes return HTTP 507.</li>
        </ul>
      </Card>
    </div>
  );
}

export function SettingsPage({ onLogout }: { onLogout: () => void }) {
  const me = useAsync(() => api.me(), []);
  const [error, setError] = useState<string | null>(null);
  async function out() {
    try { await api.logout(); } catch (e) { setError(errMsg(e)); }
    onLogout();
  }
  return (
    <div className="stack">
      <h2>Settings</h2>
      <Card title="Account">
        <ErrorBox message={me.error ?? error} />
        {me.loading && !me.data ? <Loading /> : me.data && (
          <table><tbody>
            <tr><td>E-mail</td><td>{me.data.email}</td></tr>
            <tr><td>User id</td><td className="mono">{me.data.user_id}</td></tr>
            <tr><td>Registered</td><td>{fmtTime(me.data.created_ms)}</td></tr>
          </tbody></table>
        )}
        <button className="btn danger" onClick={() => void out()}>Sign out</button>
        <p className="muted">Your session token lives in this tab only (sessionStorage) and is deleted on sign-out or when the tab closes.</p>
      </Card>
    </div>
  );
}
