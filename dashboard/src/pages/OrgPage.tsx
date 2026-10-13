import { useState } from 'react';
import { api } from '../api.ts';
import type { Org } from '../api.ts';
import { link } from '../router.ts';
import { Card, ErrorBox, Loading, Tabs, errMsg, fmtTime, useAsync } from '../ui.tsx';

type Tab = 'projects' | 'members' | 'audit';

export function OrgPage({ org }: { org: string }) {
  const orgs = useAsync(() => api.orgs(), []);
  const [tab, setTab] = useState<Tab>('projects');
  if (orgs.loading && !orgs.data) return <Loading />;
  const me = orgs.data?.find((o) => o.org_id === org);
  if (!me) return <ErrorBox message={orgs.error ?? 'Organization not found, or you are not a member.'} />;
  const isAdmin = me.role === 'owner' || me.role === 'admin';
  const tabs: { id: Tab; label: string }[] = [
    { id: 'projects', label: 'Projects' },
    { id: 'members', label: 'Members' },
    ...(isAdmin ? [{ id: 'audit' as Tab, label: 'Audit log' }] : []),
  ];
  return (
    <div className="stack">
      <h2>{me.name} <span className="badge">{me.role}</span></h2>
      <Tabs tabs={tabs} value={tab} onChange={setTab} />
      {tab === 'projects' && <Projects org={me} isAdmin={isAdmin} />}
      {tab === 'members' && <Members org={me} isAdmin={isAdmin} />}
      {tab === 'audit' && isAdmin && <Audit org={me.org_id} />}
    </div>
  );
}

function Projects({ org, isAdmin }: { org: Org; isAdmin: boolean }) {
  const list = useAsync(() => api.projects(org.org_id), [org.org_id]);
  const [name, setName] = useState('');
  const [error, setError] = useState<string | null>(null);
  async function create() {
    setError(null);
    try { await api.createProject(org.org_id, name.trim()); setName(''); list.reload(); } catch (e) { setError(errMsg(e)); }
  }
  return (
    <>
      <Card title="Projects">
        <ErrorBox message={list.error} />
        {list.loading && !list.data ? <Loading /> : list.data && list.data.length === 0 ? <p className="muted">No projects yet.</p> : (
          <table>
            <thead><tr><th>Name</th><th>Environments</th><th>Created</th><th /></tr></thead>
            <tbody>
              {list.data?.map((p) => (
                <tr key={p.project_id}>
                  <td>{p.name}</td>
                  <td>{p.environments.join(', ')}</td>
                  <td>{fmtTime(p.created_ms)}</td>
                  <td className="right"><a className="btn" href={link('org', org.org_id, 'project', p.project_id)}>Open</a></td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
      {isAdmin && (
        <Card title="New project">
          <ErrorBox message={error} />
          <p className="muted">Each project gets separate dev, staging and prod databases. Name: letters, digits, _ and -.</p>
          <div className="row">
            <input placeholder="my-app" value={name} onChange={(e) => setName(e.target.value)} />
            <button className="btn primary" disabled={!name.trim()} onClick={() => void create()}>Create project</button>
          </div>
        </Card>
      )}
    </>
  );
}

function Members({ org, isAdmin }: { org: Org; isAdmin: boolean }) {
  const list = useAsync(() => api.members(org.org_id), [org.org_id]);
  const [email, setEmail] = useState('');
  const [role, setRole] = useState<'member' | 'admin'>('member');
  const [error, setError] = useState<string | null>(null);
  async function add() {
    setError(null);
    try { await api.addMember(org.org_id, email, role); setEmail(''); list.reload(); } catch (e) { setError(errMsg(e)); }
  }
  async function remove(uid: string) {
    if (!window.confirm('Remove this member? They lose access immediately.')) return;
    setError(null);
    try { await api.removeMember(org.org_id, uid); list.reload(); } catch (e) { setError(errMsg(e)); }
  }
  return (
    <>
      <Card title="Members">
        <ErrorBox message={list.error ?? error} />
        {list.loading && !list.data ? <Loading /> : (
          <table>
            <thead><tr><th>E-mail</th><th>Role</th><th /></tr></thead>
            <tbody>
              {list.data?.map((m) => (
                <tr key={m.user_id}>
                  <td>{m.email ?? m.user_id}</td>
                  <td><span className="badge">{m.role}</span></td>
                  <td className="right">{isAdmin && m.role !== 'owner' && <button className="btn danger" onClick={() => void remove(m.user_id)}>Remove</button>}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
      {isAdmin && (
        <Card title="Add member">
          <p className="muted">The person must already have a RajeshDB account. Only the owner can appoint admins.</p>
          <div className="row">
            <input type="email" placeholder="person@example.com" value={email} onChange={(e) => setEmail(e.target.value)} />
            <select value={role} onChange={(e) => setRole(e.target.value as 'member' | 'admin')}>
              <option value="member">member (read data)</option>
              <option value="admin">admin</option>
            </select>
            <button className="btn primary" disabled={!email} onClick={() => void add()}>Add</button>
          </div>
        </Card>
      )}
    </>
  );
}

function Audit({ org }: { org: string }) {
  const ev = useAsync(() => api.audit(org), [org]);
  return (
    <Card title="Audit log (latest 200)" actions={<button className="btn" onClick={ev.reload}>Refresh</button>}>
      <ErrorBox message={ev.error} />
      {ev.loading && !ev.data ? <Loading /> : ev.data && ev.data.length === 0 ? <p className="muted">No events.</p> : (
        <div className="scroll">
          <table>
            <thead><tr><th>Time</th><th>Actor</th><th>Action</th><th>Target</th><th>Detail</th></tr></thead>
            <tbody>
              {ev.data?.map((e, i) => (
                <tr key={i}>
                  <td>{fmtTime(e.ts_ms)}</td><td className="mono">{e.actor}</td><td>{e.action}</td>
                  <td className="mono">{e.target}</td><td>{e.detail}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}
