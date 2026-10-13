import { useEffect, useState } from 'react';
import { api, getToken, setToken, setUnauthorizedHandler } from './api.ts';
import { link, useRoute } from './router.ts';
import { AuthPage } from './pages/Auth.tsx';
import { OrgsPage } from './pages/Orgs.tsx';
import { OrgPage } from './pages/OrgPage.tsx';
import { ProjectPage } from './pages/ProjectPage.tsx';
import { EnvPage } from './pages/EnvPage.tsx';
import { DocsPage, SettingsPage } from './pages/Info.tsx';
import { OpsPage } from './pages/Ops.tsx';

function Health() {
  const [state, setState] = useState<'checking' | 'up' | 'down'>('checking');
  useEffect(() => {
    let alive = true;
    const check = () => api.health().then(() => alive && setState('up'), () => alive && setState('down'));
    void check();
    const t = setInterval(() => void check(), 15000);
    return () => { alive = false; clearInterval(t); };
  }, []);
  const label = state === 'up' ? 'Server reachable' : state === 'down' ? 'Server unreachable' : 'Checking…';
  return <span className={`health ${state}`} title="Checked every 15 seconds via GET /health"><i /> {label}</span>;
}

export default function App() {
  const [authed, setAuthed] = useState<boolean>(getToken() !== null);
  const route = useRoute();

  useEffect(() => {
    setUnauthorizedHandler(() => { setToken(null); setAuthed(false); });
  }, []);

  if (!authed) return <AuthPage onAuthed={() => setAuthed(true)} />;

  const nav: { href: string; label: string; active: boolean }[] = [
    { href: link('orgs'), label: 'Organizations', active: ['orgs', 'org', 'project', 'env'].includes(route.name) },
    { href: link('docs'), label: 'Documentation', active: route.name === 'docs' },
    { href: link('ops'), label: 'Operations', active: route.name === 'ops' },
    { href: link('settings'), label: 'Settings', active: route.name === 'settings' },
  ];

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">RajeshDB</div>
        <nav>
          {nav.map((n) => <a key={n.href} href={n.href} className={n.active ? 'nav active' : 'nav'}>{n.label}</a>)}
        </nav>
      </aside>
      <div className="main">
        <header className="topbar">
          <Health />
          <button className="btn ghost" onClick={() => { void api.logout().catch(() => {}); setAuthed(false); }}>Sign out</button>
        </header>
        <main className="content">
          {route.name === 'orgs' && <OrgsPage />}
          {route.name === 'org' && <OrgPage key={route.org} org={route.org} />}
          {route.name === 'project' && <ProjectPage key={route.pid} org={route.org} pid={route.pid} />}
          {route.name === 'env' && <EnvPage key={route.pid + route.env} org={route.org} pid={route.pid} env={route.env} />}
          {route.name === 'docs' && <DocsPage />}
          {route.name === 'ops' && <OpsPage />}
          {route.name === 'settings' && <SettingsPage onLogout={() => setAuthed(false)} />}
          {route.name === 'notfound' && <p className="muted">Page not found. <a href={link('orgs')}>Go to organizations</a></p>}
        </main>
      </div>
    </div>
  );
}
