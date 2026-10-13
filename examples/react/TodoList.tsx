// A React component that talks to YOUR backend (examples/node/server.mjs), never to RajeshDB directly.
// Why: a RajeshDB API key is a secret. Anything in browser code is public, so a key placed here would be stolen.
//
// Wiring in a Vite project: add to vite.config.ts
//   server: { proxy: { '/todos': 'http://127.0.0.1:3000' } }
// then render <TodoList /> anywhere.
import { useEffect, useState } from 'react';

interface Todo { _id: string; title: string; done: boolean }

export default function TodoList() {
  const [todos, setTodos] = useState<Todo[]>([]);
  const [title, setTitle] = useState('');
  const [error, setError] = useState<string | null>(null);

  async function load() {
    try {
      const res = await fetch('/todos');
      if (!res.ok) throw new Error(`server answered ${res.status}`);
      setTodos((await res.json()) as Todo[]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  useEffect(() => { void load(); }, []);

  async function add() {
    const res = await fetch('/todos', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ title }) });
    if (!res.ok) { setError(`could not add (${res.status})`); return; }
    setTitle('');
    await load();
  }

  return (
    <div>
      <h2>To-do</h2>
      {error && <p role="alert">{error}</p>}
      <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="What needs doing?" maxLength={200} />
      <button disabled={!title.trim()} onClick={() => void add()}>Add</button>
      <ul>{todos.map((t) => <li key={t._id}>{t.title}</li>)}</ul>
    </div>
  );
}
