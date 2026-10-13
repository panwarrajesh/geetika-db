#!/usr/bin/env node
// HTTP load test for the RajeshDB platform API. Needs Node 18+ (built-in fetch). No packages.
//
//   node loadtest.mjs --base http://127.0.0.1:8081 --scenario mixed --concurrency 16 --duration 20
//
// Scenarios: insert | query | get | mixed | abort
//   abort   = clients cancel requests mid-flight (network interruption); afterwards the server must still
//             answer /health and a normal request.
// Setup: by default the tool registers a throw-away user, creates an org/project/key/collection (+index on "city").
//        To test an existing project instead:  --key rdb_... --project <id> --env dev --collection load
// Everything printed is MEASURED by this tool against the server you point it at. Run it on a DIFFERENT
// machine than the server when you want realistic numbers, and write down both machines' specs.

import os from 'node:os';
import { writeFileSync } from 'node:fs';
import { performance } from 'node:perf_hooks';

function args(argv) {
  const o = {};
  for (let i = 0; i < argv.length; i++) {
    if (argv[i].startsWith('--')) {
      const k = argv[i].slice(2);
      const next = argv[i + 1];
      if (next === undefined || next.startsWith('--')) o[k] = true;
      else { o[k] = next; i++; }
    }
  }
  return o;
}

export function percentile(sortedMs, p) {
  if (sortedMs.length === 0) return NaN;
  const idx = Math.min(sortedMs.length - 1, Math.max(0, Math.round((sortedMs.length - 1) * p)));
  return sortedMs[idx];
}

export function summarize(samples, elapsedSec) {
  const lat = samples.map((s) => s.ms).sort((a, b) => a - b);
  const byStatus = {};
  for (const s of samples) byStatus[s.status] = (byStatus[s.status] ?? 0) + 1;
  return {
    requests: samples.length,
    seconds: elapsedSec,
    requests_per_sec: samples.length / elapsedSec,
    p50_ms: percentile(lat, 0.5), p90_ms: percentile(lat, 0.9), p99_ms: percentile(lat, 0.99),
    max_ms: lat.length ? lat[lat.length - 1] : NaN,
    by_status: byStatus,
  };
}

async function api(base, method, path, token, body) {
  const res = await fetch(base + path, {
    method,
    headers: { ...(body !== undefined ? { 'content-type': 'application/json' } : {}), ...(token ? { authorization: `Bearer ${token}` } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json = null;
  try { json = text ? JSON.parse(text) : null; } catch { /* not json */ }
  return { status: res.status, json };
}

async function setup(base, a) {
  if (a.key && a.project) {
    return { key: a.key, project: a.project, env: a.env ?? 'dev', coll: a.collection ?? 'load' };
  }
  const email = `loadtest-${Date.now()}-${Math.floor(Math.random() * 1e6)}@example.com`;
  const password = 'load-test-password-123';
  const must = (r, what) => { if (r.status >= 300) throw new Error(`setup failed at ${what}: HTTP ${r.status} ${JSON.stringify(r.json)}`); return r.json; };
  must(await api(base, 'POST', '/v1/auth/register', null, { email, password }), 'register');
  const { token } = must(await api(base, 'POST', '/v1/auth/login', null, { email, password }), 'login');
  const { org_id } = must(await api(base, 'POST', '/v1/orgs', token, { name: 'loadtest' }), 'org');
  const { project_id } = must(await api(base, 'POST', `/v1/orgs/${org_id}/projects`, token, { name: 'loadtest' }), 'project');
  const { api_key } = must(await api(base, 'POST', `/v1/projects/${project_id}/keys`, token, { name: 'lt', env: 'dev', role: 'admin' }), 'key');
  const root = `/v1/projects/${project_id}/envs/dev/collections`;
  must(await api(base, 'POST', root, api_key, { name: 'load' }), 'collection');
  must(await api(base, 'POST', `${root}/load/indexes`, api_key, { field: 'city' }), 'index');
  return { key: api_key, project: project_id, env: 'dev', coll: 'load' };
}

async function main() {
  const a = args(process.argv.slice(2));
  const base = (a.base ?? 'http://127.0.0.1:8081').replace(/\/$/, '');
  const scenario = a.scenario ?? 'mixed';
  const concurrency = Number(a.concurrency ?? 16);
  const duration = Number(a.duration ?? 20);
  const seed = Number(a.seed ?? 1000);
  if (!['insert', 'query', 'get', 'mixed', 'abort'].includes(scenario)) throw new Error('unknown --scenario');

  const h = await api(base, 'GET', '/health').catch(() => null);
  if (!h || h.status !== 200) throw new Error(`server not reachable at ${base}`);
  const ctx = await setup(base, a);
  const dataRoot = `/v1/projects/${ctx.project}/envs/${ctx.env}/collections/${ctx.coll}`;

  // seed documents so reads have something to find
  for (let i = 0; i < seed; i++) {
    const r = await api(base, 'POST', `${dataRoot}/documents`, ctx.key, { _id: `seed${i}`, city: `city${i % 50}`, n: i });
    if (r.status >= 300 && r.status !== 409) throw new Error(`seeding failed: HTTP ${r.status} ${JSON.stringify(r.json)}`);
  }

  const samples = [];
  let counter = 0;
  const deadline = performance.now() + duration * 1000;
  const op = (kind) => {
    if (kind === 'insert') return () => api(base, 'POST', `${dataRoot}/documents`, ctx.key, { city: `city${counter % 50}`, n: counter++, blob: 'x'.repeat(100) });
    if (kind === 'get') return () => api(base, 'GET', `${dataRoot}/documents/seed${Math.floor(Math.random() * seed)}`, ctx.key);
    return () => api(base, 'POST', `${dataRoot}/query`, ctx.key, { filter: { city: `city${Math.floor(Math.random() * 50)}` }, limit: 20 });
  };

  let aborted = 0;
  async function worker() {
    while (performance.now() < deadline) {
      let kind = scenario;
      if (scenario === 'mixed') { const r = Math.random(); kind = r < 0.7 ? 'query' : r < 0.9 ? 'insert' : 'get'; }
      if (scenario === 'abort') {
        const ac = new AbortController();
        const t = setTimeout(() => ac.abort(), 1 + Math.floor(Math.random() * 4));
        try {
          await fetch(base + `${dataRoot}/query`, { method: 'POST', signal: ac.signal, headers: { 'content-type': 'application/json', authorization: `Bearer ${ctx.key}` }, body: JSON.stringify({ filter: {} }) });
        } catch { aborted++; }
        clearTimeout(t);
        continue;
      }
      const t0 = performance.now();
      let status;
      try { status = (await op(kind)()).status; } catch { status = 'network_error'; }
      samples.push({ ms: performance.now() - t0, status });
    }
  }
  const t0 = performance.now();
  await Promise.all(Array.from({ length: concurrency }, worker));
  const elapsed = (performance.now() - t0) / 1000;

  const out = {
    tool: 'rajeshdb loadtest.mjs', base, scenario, concurrency, duration_requested_s: duration, seeded_docs: seed,
    client: { os: `${os.platform()} ${os.release()}`, cpus: os.cpus().length, cpu_model: os.cpus()[0]?.model, node: process.version },
  };
  if (scenario === 'abort') {
    const after = await api(base, 'GET', '/health').catch(() => null);
    const normal = await api(base, 'POST', `${dataRoot}/query`, ctx.key, { limit: 1 }).catch(() => null);
    out.aborted_requests = aborted;
    out.server_healthy_after = after?.status === 200;
    out.normal_request_after = normal?.status;
    out.pass = after?.status === 200 && normal?.status === 200;
  } else {
    out.results = summarize(samples, elapsed);
  }

  console.log(JSON.stringify(out, null, 2));
  if (a.json && typeof a.json === 'string') writeFileSync(a.json, JSON.stringify(out, null, 2));
  if (scenario === 'abort' && !out.pass) process.exitCode = 1;
}

if (import.meta.url === `file://${process.argv[1]}` || process.argv[1]?.endsWith('loadtest.mjs')) {
  main().catch((e) => { console.error('ERROR:', e.message); process.exit(1); });
}
