import { useState } from 'react';
import { api } from '../api.ts';
import { ErrorBox, errMsg } from '../ui.tsx';

export function AuthPage({ onAuthed }: { onAuthed: () => void }) {
  const [mode, setMode] = useState<'login' | 'register'>('login');
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      if (mode === 'register') await api.register(email, password);
      await api.login(email, password);
      onAuthed();
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="auth-wrap">
      <div className="card auth-card">
        <h1>RajeshDB Console</h1>
        <p className="muted">{mode === 'login' ? 'Sign in to manage your projects.' : 'Create an account (password: 10+ characters).'}</p>
        <ErrorBox message={error} />
        <label>E-mail
          <input type="email" autoComplete="email" value={email} onChange={(e) => setEmail(e.target.value)} />
        </label>
        <label>Password
          <input
            type="password"
            autoComplete={mode === 'login' ? 'current-password' : 'new-password'}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter' && !busy) void submit(); }}
          />
        </label>
        <button className="btn primary" disabled={busy || !email || !password} onClick={() => void submit()}>
          {busy ? 'Please wait…' : mode === 'login' ? 'Sign in' : 'Create account'}
        </button>
        <button className="btn ghost" onClick={() => { setMode(mode === 'login' ? 'register' : 'login'); setError(null); }}>
          {mode === 'login' ? 'Need an account? Register' : 'Have an account? Sign in'}
        </button>
      </div>
    </div>
  );
}
