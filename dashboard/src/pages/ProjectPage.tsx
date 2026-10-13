import { useState } from 'react';
import { api } from '../api.ts';
import type { NewKey } from '../api.ts';
import { formatBytes } from '../lib.ts';
import { link, navigate } from '../router.ts';
import { Card, CopyButton, ErrorBox, Loading, Meter, Modal, errMsg, fmtTime, useAsync } from '../ui.tsx';

const ENVS = ['dev', 'staging', 'prod'];

export function ProjectPage({ org, pid }: { org: string; pid: string }) {
  const orgs = useAsync(() => api.orgs(), []);
  const projects = useAsync(() => api.projects(org), [org]);
  if ((orgs.loading && !orgs.data) || (projects.loading && !projects.data)) return <Loading />;
  const me = orgs.data?.find((o) => o.org_id === org);
  const project = projects.data?.find((p) => p.project_id === pid);
  if (!me || !project) return <ErrorBox message={orgs.error ?? projects.error ?? 'Project not found.'} />;
  const isAdmin = me.role === 'owner' || me.role === 'admin';

  async function remove() {
    if (!window.confirm(`Delete project "${project?.name}" and ALL its data in every environment? This cannot be undone.`)) return;
    try { await api.deleteProject(pid); navigate(link('org', org)); } catch (e) { window.alert(errMsg(e)); }
  }

  return (
    <div className="stack">
      <p className="muted"><a href={link('org', org)}>← {me.name}</a></p>
      <h2>{project.name}</h2>
      <p className="muted mono">project id: {pid}</p>
      <div className="grid">
        {ENVS.map((env) => <EnvCard key={env} org={org} pid={pid} env={env} />)}
      </div>
      {isAdmin ? <Keys pid={pid} /> : <Card title="API keys"><p className="muted">Only organization admins can manage API keys.</p></Card>}
      {me.role === 'owner' && (
        <Card title="Danger zone">
          <button className="btn danger" onClick={() => void remove()}>Delete project</button>
        </Card>
      )}
    </div>
  );
}

function EnvCard({ org, pid, env }: { org: string; pid: string; env: string }) {
  const u = useAsync(() => api.usage(pid, env), [pid, env]);
  return (
    <Card title={env} actions={<a className="btn primary" href={link('org', org, 'project', pid, 'env', env)}>Open</a>}>
      <ErrorBox message={u.error} />
      {u.data && (
        <>
          <Meter used={u.data.used_bytes} total={u.data.quota_bytes} />
          <p className="muted">{formatBytes(u.data.used_bytes)} of {formatBytes(u.data.quota_bytes)} used</p>
        </>
      )}
    </Card>
  );
}

function Keys({ pid }: { pid: string }) {
  const keys = useAsync(() => api.keys(pid), [pid]);
  const [name, setName] = useState('');
  const [env, setEnv] = useState('dev');
  const [role, setRole] = useState('read');
  const [days, setDays] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [fresh, setFresh] = useState<NewKey | null>(null);

  async function create() {
    setError(null);
    try {
      const body: { name: string; env: string; role: string; expires_in_secs?: number } = { name: name.trim(), env, role };
      if (days.trim()) {
        const d = Number(days);
        if (!(d > 0)) { setError('Expiry must be a positive number of days (or empty for no expiry).'); return; }
        body.expires_in_secs = Math.round(d * 86400);
      }
      setFresh(await api.createKey(pid, body));
      setName('');
      keys.reload();
    } catch (e) { setError(errMsg(e)); }
  }
  async function revoke(kid: string) {
    if (!window.confirm('Revoke this key? Applications using it stop working immediately.')) return;
    try { await api.revokeKey(pid, kid); keys.reload(); } catch (e) { setError(errMsg(e)); }
  }
  async function rotate(kid: string) {
    const g = window.prompt('Keep the OLD key working for how many minutes? (0 = revoke it immediately)', '0');
    if (g === null) return;
    const minutes = Number(g);
    if (!(minutes >= 0) || minutes > 7 * 24 * 60) { setError('Grace period must be between 0 and 10080 minutes.'); return; }
    try { setFresh(await api.rotateKey(pid, kid, Math.round(minutes * 60))); keys.reload(); } catch (e) { setError(errMsg(e)); }
  }

  return (
    <>
      <Card title="API keys">
        <ErrorBox message={keys.error ?? error} />
        <p className="muted">A key works for ONE environment of this project. Use read/write keys in apps; keep admin keys for tooling.</p>
        {keys.loading && !keys.data ? <Loading /> : keys.data && keys.data.length === 0 ? <p className="muted">No keys yet.</p> : (
          <div className="scroll">
            <table>
              <thead><tr><th>Name</th><th>Env</th><th>Role</th><th>Created</th><th>Expires</th><th>Key id</th><th /></tr></thead>
              <tbody>
                {keys.data?.map((k) => (
                  <tr key={k.key_id}>
                    <td>{k.name}</td><td>{k.env}</td><td><span className="badge">{k.role}</span></td>
                    <td>{fmtTime(k.created_ms)}</td><td>{k.expires_ms ? fmtTime(k.expires_ms) : 'never'}</td>
                    <td className="mono">{k.key_id}</td>
                    <td className="right">
                      <button className="btn" onClick={() => void rotate(k.key_id)}>Rotate</button>{' '}
                      <button className="btn danger" onClick={() => void revoke(k.key_id)}>Revoke</button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
      <Card title="Create key">
        <div className="row wrap">
          <input placeholder="name, e.g. backend" value={name} onChange={(e) => setName(e.target.value)} />
          <select value={env} onChange={(e) => setEnv(e.target.value)}>{ENVS.map((e) => <option key={e}>{e}</option>)}</select>
          <select value={role} onChange={(e) => setRole(e.target.value)}>
            <option value="read">read</option><option value="write">write</option><option value="admin">admin</option>
          </select>
          <input className="narrow" placeholder="expires (days)" value={days} onChange={(e) => setDays(e.target.value)} />
          <button className="btn primary" disabled={!name.trim()} onClick={() => void create()}>Create</button>
        </div>
      </Card>
      {fresh && (
        <Modal title="Copy your new API key" onClose={() => setFresh(null)}>
          <p><strong>This is the only time the key is shown.</strong> It is not stored in readable form anywhere.</p>
          <pre className="secret">{fresh.api_key}</pre>
          <p className="muted">env: {fresh.env} · role: {fresh.role} · id: {fresh.key_id}</p>
          <div className="row"><CopyButton text={fresh.api_key} /><button className="btn primary" onClick={() => setFresh(null)}>I have saved it</button></div>
        </Modal>
      )}
    </>
  );
}
