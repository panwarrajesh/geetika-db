// Example backend: a tiny to-do API that stores data in RajeshDB. Needs Node 22.18+ (runs the TypeScript SDK directly).
//
//   set RAJESHDB_URL=http://127.0.0.1:8081      (PowerShell:  $env:RAJESHDB_URL = "...")
//   set RAJESHDB_PROJECT=<32-char project id>
//   set RAJESHDB_API_KEY=rdb_...                (a READ+WRITE key for ONE environment; never put it in frontend code)
//   set RAJESHDB_ENV=dev
//   node examples/node/server.mjs               -> http://127.0.0.1:3000/todos
//
// The browser talks to THIS server; only this server knows the API key.
import http from 'node:http';
import { RajeshDB, RajeshDBError } from '../../sdk/js/src/index.ts';

const need = (n) => { const v = process.env[n]; if (!v) { console.error(`missing environment variable ${n}`); process.exit(1); } return v; };
const db = new RajeshDB({
  baseUrl: need('RAJESHDB_URL'), project: need('RAJESHDB_PROJECT'), apiKey: need('RAJESHDB_API_KEY'),
  env: process.env.RAJESHDB_ENV ?? 'dev', timeoutMs: 10000,
});
const todos = db.collection('todos');   // create it once with an admin key: db.createCollection('todos')

const send = (res, status, body) => { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };

const server = http.createServer(async (req, res) => {
  try {
    if (req.method === 'GET' && req.url === '/todos') {
      const r = await todos.find({ sort: { field: 'created', order: 'desc' }, limit: 50 });
      return send(res, 200, r.documents);
    }
    if (req.method === 'POST' && req.url === '/todos') {
      let raw = '';
      for await (const chunk of req) { raw += chunk; if (raw.length > 10_000) return send(res, 413, { error: 'too large' }); }
      const input = JSON.parse(raw || '{}');
      // Validate on YOUR server: never pass raw request bodies straight into the database.
      if (typeof input.title !== 'string' || input.title.trim() === '' || input.title.length > 200) return send(res, 400, { error: 'title must be 1-200 characters' });
      const id = await todos.insert({ title: input.title.trim(), done: false, created: Date.now() });
      return send(res, 201, { _id: id });
    }
    send(res, 404, { error: 'not found' });
  } catch (e) {
    if (e instanceof RajeshDBError) { console.error('database error', e.status, e.code); return send(res, 502, { error: 'database unavailable' }); }
    if (e instanceof SyntaxError) return send(res, 400, { error: 'invalid JSON' });
    console.error(e);
    send(res, 500, { error: 'internal error' });
  }
});
server.listen(Number(process.env.PORT ?? 3000), '127.0.0.1', () => console.log(`todo API on http://127.0.0.1:${process.env.PORT ?? 3000}/todos`));
