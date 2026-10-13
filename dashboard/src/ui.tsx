import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { percent } from './lib.ts';

export function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export interface AsyncState<T> {
  loading: boolean;
  error: string | null;
  data: T | null;
  reload: () => void;
}

/** Runs `fn` on mount and whenever `deps` change; ignores results of outdated calls. */
export function useAsync<T>(fn: () => Promise<T>, deps: ReadonlyArray<unknown>): AsyncState<T> {
  const [state, setState] = useState<{ loading: boolean; error: string | null; data: T | null }>({
    loading: true,
    error: null,
    data: null,
  });
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let alive = true;
    setState((s) => ({ ...s, loading: true, error: null }));
    fn()
      .then((d) => { if (alive) setState({ loading: false, error: null, data: d }); })
      .catch((e: unknown) => { if (alive) setState({ loading: false, error: errMsg(e), data: null }); });
    return () => { alive = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);
  return { ...state, reload: () => setTick((t) => t + 1) };
}

export function ErrorBox({ message }: { message: string | null }) {
  return message ? <div className="error" role="alert">{message}</div> : null;
}

export function Loading() {
  return <p className="muted">Loading…</p>;
}

export function Card({ title, actions, children }: { title?: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <section className="card">
      {(title || actions) && (
        <header className="card-head">
          <h3>{title}</h3>
          <div className="row">{actions}</div>
        </header>
      )}
      {children}
    </section>
  );
}

export function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  return (
    <div className="modal-back" onMouseDown={onClose}>
      <div className="modal" role="dialog" aria-modal="true" aria-label={title} onMouseDown={(e) => e.stopPropagation()}>
        <header className="card-head">
          <h3>{title}</h3>
          <button className="btn ghost" onClick={onClose} aria-label="Close">✕</button>
        </header>
        {children}
      </div>
    </div>
  );
}

export function Tabs<T extends string>({ tabs, value, onChange }: { tabs: { id: T; label: string }[]; value: T; onChange: (t: T) => void }) {
  return (
    <div className="tabs" role="tablist">
      {tabs.map((t) => (
        <button key={t.id} role="tab" aria-selected={t.id === value} className={t.id === value ? 'tab active' : 'tab'} onClick={() => onChange(t.id)}>
          {t.label}
        </button>
      ))}
    </div>
  );
}

export function Meter({ used, total }: { used: number; total: number }) {
  const p = percent(used, total);
  return (
    <div className="meter" role="progressbar" aria-valuenow={Math.round(p)} aria-valuemin={0} aria-valuemax={100}>
      <div className={p >= 90 ? 'meter-fill danger' : 'meter-fill'} style={{ width: `${p}%` }} />
    </div>
  );
}

export function CopyButton({ text }: { text: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      className="btn"
      onClick={() => {
        navigator.clipboard.writeText(text).then(() => { setDone(true); setTimeout(() => setDone(false), 1500); }, () => {});
      }}
    >
      {done ? 'Copied' : 'Copy'}
    </button>
  );
}

export function fmtTime(ms: number | null | undefined): string {
  return ms ? new Date(ms).toLocaleString() : '—';
}
