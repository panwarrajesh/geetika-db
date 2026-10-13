// A tiny FAKE server that imitates just enough of the platform API to test loadtest.mjs itself.
// It is NOT RajeshDB. Any number measured against it says nothing about RajeshDB's performance.
import http from 'node:http';
const docs = new Map();
let n = 0;
const send = (res, status, body) => { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
http.createServer((req, res) => {
  let data = '';
  req.on('data', (c) => (data += c));
  req.on('end', () => {
    const body = data ? JSON.parse(data) : {};
    const u = req.url;
    if (u === '/health') return send(res, 200, { status: 'ok' });
    if (u === '/v1/auth/register') return send(res, 201, { user_id: 'u' });
    if (u === '/v1/auth/login') return send(res, 200, { token: 'rdbs_x' });
    if (u === '/v1/orgs') return send(res, 201, { org_id: 'o' });
    if (/\/v1\/orgs\/.+\/projects$/.test(u)) return send(res, 201, { project_id: 'p'.repeat(32) });
    if (/\/keys$/.test(u)) return send(res, 201, { api_key: 'rdb_mock', key_id: 'k' });
    if (/\/collections$/.test(u) || /\/indexes$/.test(u)) return send(res, 201, {});
    if (/\/documents$/.test(u)) { const id = body._id ?? `auto${n++}`; if (docs.has(id)) return send(res, 409, { error: { code: 'already_exists', message: 'dup' } }); docs.set(id, body); return send(res, 201, { _id: id }); }
    const m = u.match(/\/documents\/(.+)$/);
    if (m && req.method === 'GET') return docs.has(m[1]) ? send(res, 200, docs.get(m[1])) : send(res, 404, {});
    if (/\/query$/.test(u)) { const out = [...docs.values()].filter((d) => !body.filter?.city || d.city === body.filter.city).slice(0, body.limit ?? 100); return setTimeout(() => send(res, 200, { documents: out, count: out.length }), 2); }
    send(res, 404, { error: { code: 'not_found', message: 'no route' } });
  });
}).listen(Number(process.argv[2] ?? 18081), '127.0.0.1');
