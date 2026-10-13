import { useState } from 'react';
import { api } from '../api.ts';
import { link, navigate } from '../router.ts';
import { Card, ErrorBox, Loading, errMsg, useAsync } from '../ui.tsx';

export function OrgsPage() {
  const orgs = useAsync(() => api.orgs(), []);
  const [name, setName] = useState('');
  const [error, setError] = useState<string | null>(null);

  async function create() {
    setError(null);
    try {
      const o = await api.createOrg(name.trim());
      setName('');
      navigate(link('org', o.org_id));
    } catch (e) {
      setError(errMsg(e));
    }
  }

  return (
    <div className="stack">
      <h2>Organizations</h2>
      <ErrorBox message={orgs.error} />
      {orgs.loading && !orgs.data ? <Loading /> : (
        <Card>
          {orgs.data && orgs.data.length === 0 && <p className="muted">You are not in any organization yet. Create one below.</p>}
          {orgs.data && orgs.data.length > 0 && (
            <table>
              <thead><tr><th>Name</th><th>Your role</th><th /></tr></thead>
              <tbody>
                {orgs.data.map((o) => (
                  <tr key={o.org_id}>
                    <td>{o.name}</td>
                    <td><span className="badge">{o.role}</span></td>
                    <td className="right"><a className="btn" href={link('org', o.org_id)}>Open</a></td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </Card>
      )}
      <Card title="New organization">
        <ErrorBox message={error} />
        <div className="row">
          <input placeholder="Company or team name" value={name} onChange={(e) => setName(e.target.value)} />
          <button className="btn primary" disabled={!name.trim()} onClick={() => void create()}>Create</button>
        </div>
      </Card>
    </div>
  );
}
