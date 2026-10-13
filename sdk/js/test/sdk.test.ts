import { test } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { RajeshDB, RajeshDBError } from '../src/index.ts';

const PID = 'a'.repeat(32);
const KEY = 'rdb_testkey';

interface Seen { method: string; url: string; auth: string | undefined; body: unknown }

/** A scripted fake server: returns the queued responses in order and records every request. */
async function fake(script: Array<{ status: number; body?: unknown; delayMs?: number }>) {
  const seen: Seen[] = [];
  const server = http.createServer((req, res) => {
    let data = '';
    req.on('data', (c) => (data += c));
    req.on('end', () => {
      seen.push({ method: req.method ?? '', url: req.url ?? '', auth: req.headers.authorization, body: data ? JSON.parse(data) : undefined });
      const next = script.shift() ?? { status: 500, body: { error: { code: 'script_exhausted', message: 'no more scripted responses' } } };
      setTimeout(() => {
        res.writeHead(next.status, { 'content-type': 'application/json' });
        res.end(next.body === undefined ? '' : JSON.stringify(next.body));
      }, next.delayMs ?? 0);
    });
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', () => r()));
  const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  return { url, seen, close: () => new Promise<void>((r) => { server.closeAllConnections(); server.close(() => r()); }) };
}

test('insert/get/replace/delete use the right method, path, auth header and body', async () => {
  const f = await fake([
    { status: 201, body: { _id: 'x1' } },
    { status: 200, body: { _id: 'x1', a: 1 } },
    { status: 200, body: { _id: 'x1' } },
    { status: 200, body: { deleted: true } },
  ]);
  try {
    const db = new RajeshDB({ baseUrl: f.url + '/', apiKey: KEY, project: PID, env: 'prod' });
    const users = db.collection('users');
    assert.equal(await users.insert({ a: 1 }), 'x1');
    assert.deepEqual(await users.get('x1'), { _id: 'x1', a: 1 });
    await users.replace('x1', { a: 2 });
    assert.equal(await users.delete('x1'), true);
    const base = `/v1/projects/${PID}/envs/prod/collections/users`;
    assert.deepEqual(f.seen.map((s) => [s.method, s.url]), [
      ['POST', `${base}/documents`], ['GET', `${base}/documents/x1`], ['PUT', `${base}/documents/x1`], ['DELETE', `${base}/documents/x1`],
    ]);
    assert.ok(f.seen.every((s) => s.auth === `Bearer ${KEY}`));
    assert.deepEqual(f.seen[0].body, { a: 1 });
    assert.deepEqual(f.seen[2].body, { a: 2 });
  } finally { await f.close(); }
});

test('404 on get/delete means null/false; other errors become RajeshDBError with the server code', async () => {
  const f = await fake([
    { status: 404, body: { error: { code: 'not_found', message: 'document does not exist' } } },
    { status: 404, body: { error: { code: 'not_found', message: 'document does not exist' } } },
    { status: 409, body: { error: { code: 'already_exists', message: 'dup' } } },
    { status: 403, body: { error: { code: 'forbidden', message: 'no' } } },
  ]);
  try {
    const c = new RajeshDB({ baseUrl: f.url, apiKey: KEY, project: PID }).collection('c');
    assert.equal(await c.get('nope'), null);
    assert.equal(await c.delete('nope'), false);
    await assert.rejects(c.insert({ _id: 'd' }), (e: unknown) => e instanceof RajeshDBError && e.status === 409 && e.code === 'already_exists' && e.message === 'dup');
    await assert.rejects(c.createIndex('f'), (e: unknown) => e instanceof RajeshDBError && e.status === 403);
  } finally { await f.close(); }
});

test('find, findOne, update, deleteWhere send the documented bodies', async () => {
  const f = await fake([
    { status: 200, body: { documents: [{ _id: '1' }], count: 1, skip: 0, limit: 100, plan: 'FullScan' } },
    { status: 200, body: { documents: [], count: 0, skip: 0, limit: 1, plan: 'FullScan' } },
    { status: 200, body: { modified: 3 } },
    { status: 400, body: { error: { code: 'invalid_argument', message: 'empty filter' } } },
    { status: 200, body: { deleted: 2 } },
  ]);
  try {
    const c = new RajeshDB({ baseUrl: f.url, apiKey: KEY, project: PID }).collection('c');
    const r = await c.find({ filter: { city: 'Delhi' }, sort: { field: 'age', order: 'desc' }, limit: 5 });
    assert.equal(r.count, 1);
    assert.equal(await c.findOne({ x: 1 }), null);
    assert.equal(await c.update({ a: 1 }, { b: 2 }), 3);
    await assert.rejects(c.deleteWhere({}), (e: unknown) => e instanceof RajeshDBError && e.status === 400);
    assert.equal(await c.deleteWhere({}, { confirmAll: true }), 2);
    assert.deepEqual(f.seen[0].body, { filter: { city: 'Delhi' }, sort: { field: 'age', order: 'desc' }, limit: 5 });
    assert.deepEqual(f.seen[1].body, { filter: { x: 1 }, limit: 1 });
    assert.deepEqual(f.seen[2].body, { filter: { a: 1 }, set: { b: 2 } });
    assert.deepEqual(f.seen[3].body, { filter: {} });
    assert.deepEqual(f.seen[4].body, { filter: {}, confirm_all: true });
  } finally { await f.close(); }
});

test('ids and names are URL-encoded', async () => {
  const f = await fake([{ status: 404 }]);
  try {
    const c = new RajeshDB({ baseUrl: f.url, apiKey: KEY, project: PID }).collection('c');
    await c.get('a/b c?d');
    assert.ok(f.seen[0].url.endsWith('/documents/a%2Fb%20c%3Fd'), f.seen[0].url);
  } finally { await f.close(); }
});

test('timeouts and network failures are reported as RajeshDBError', async () => {
  const f = await fake([{ status: 200, body: {}, delayMs: 500 }]);
  try {
    const slow = new RajeshDB({ baseUrl: f.url, apiKey: KEY, project: PID, timeoutMs: 50 }).collection('c');
    await assert.rejects(slow.find(), (e: unknown) => e instanceof RajeshDBError && e.code === 'timeout');
  } finally { await f.close(); }
  const dead = new RajeshDB({ baseUrl: 'http://127.0.0.1:1', apiKey: KEY, project: PID, timeoutMs: 2000 }).collection('c');
  await assert.rejects(dead.find(), (e: unknown) => e instanceof RajeshDBError && e.code === 'network');
});

test('constructor validation, secret hiding and the browser guard', () => {
  assert.throws(() => new RajeshDB({ baseUrl: 'http://x', apiKey: 'nope', project: PID }));
  assert.throws(() => new RajeshDB({ baseUrl: 'http://x', apiKey: KEY, project: 'short' }));
  const db = new RajeshDB({ baseUrl: 'http://x', apiKey: KEY, project: PID });
  assert.ok(!JSON.stringify(db).includes(KEY), 'the key must not appear when the client is serialized');
  const g = globalThis as { window?: unknown };
  g.window = {};
  try {
    assert.throws(() => new RajeshDB({ baseUrl: 'http://x', apiKey: KEY, project: PID }), /browser/);
    assert.doesNotThrow(() => new RajeshDB({ baseUrl: 'http://x', apiKey: KEY, project: PID, allowBrowser: true }));
  } finally { delete g.window; }
});
