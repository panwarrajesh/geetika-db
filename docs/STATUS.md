# Status (Phase 3)

Nothing below is considered verified until `cargo test` passes on a real machine.
(Code was written without access to a Rust compiler.)

## Engine (rajeshdb-core)
- Put / get / delete / prefix scan / range scan
- WAL with CRC32, crash recovery, torn-tail truncation
- NEW: atomic multi-key `write_batch` (one WAL frame: all-or-nothing after a crash)
- Memtable -> SSTable flush, MANIFEST, orphan cleanup, compaction, checksummed files

## Document layer (rajeshdb-docs)  NEW
- Collections (names: A-Z a-z 0-9 _ -, max 64 chars), create / drop / list
- JSON documents (objects), string `_id` (auto-generated if absent), insert / get / replace / delete
- Optional per-collection schema: field types, required, additionalProperties (top-level fields only)
- Secondary indexes on dotted field paths, backfilled on creation; maintained atomically with every write
- Query filters: implicit eq, $eq $ne $gt $gte $lt $lte $in $nin $exists, $and, $or, dotted paths
- sort (one field), skip, limit; update_where (shallow top-level merge), delete_where
- Planner: uses one index among top-level AND-ed conditions (eq < in < range); always re-checks the
  full filter on candidates; `explain` shows the choice
- CLI: `--docs` mode

## Semantics worth knowing
- Comparisons only match same kinds (number/number, string/string, bool/bool)
- Missing field != null; `$ne`/`$nin` match documents that lack the field
- Numbers are compared as f64 (integers above 2^53 are not distinguished)
- Indexed values: null, bool, number, string up to 1024 bytes. Arrays/objects are not indexed
- Document keys are stored sorted (serde_json default), not in insertion order

## Not implemented (do NOT advertise)
- Multi-document transactions; update_where/delete_where are atomic per document only
- SQL, joins, aggregation, compound/unique indexes, array-element queries, text search
- Concurrency (single writer, `&mut self`), multi-process locking
- Server / auth / multi-tenant (Phase 4-5)

## Known limitations
- Queries and scans materialise results in memory; full scans read every document of the collection
- SSTable key index and compaction merge are held in RAM (streaming merge still TODO)
- Index range scans read each matching key's id, then fetch documents one by one
- Orphan index entries after a crash during drop_index are harmless (candidates are re-checked)
- No benchmarks run yet; no performance claims


---
# Phase 4 additions: HTTP server (rajeshdb-server)

## Implemented (unverified until `cargo test` passes on a real machine)
- REST API over the document layer (see docs/openapi.yaml): collections, indexes, documents, query, update, delete, admin flush/compact/metrics
- API keys with roles read < write < admin; only SHA-256 hashes stored; key generation/list/revoke via CLI
- Failed-login throttling per client IP; per-key token-bucket rate limit
- Request body size limit, default and maximum result limits, explicit confirmation for empty-filter update/delete
- Uniform JSON errors with correct status codes (400/401/403/404/409/413/422/429/500); no internal details leaked
- One JSON log line per request (method, path, status, ms, key id; never bodies, query strings or credentials)
- Real metrics endpoint (request counts, auth failures, rate limits, engine stats)
- Graceful shutdown (Ctrl+C) with flush; default bind is 127.0.0.1

## Not implemented yet (do NOT advertise)
- TLS inside the server (use a reverse proxy, Phase 8). Never expose port 8080 directly to the internet
- Users, organizations, projects, per-project databases and tenant isolation (Phase 5): in Phase 4 every key sees the same database
- Key expiry, hot reload of keys (revocation needs a restart), audit log
- If a reverse proxy is used, all clients share the proxy IP for the failed-login throttle until proxy headers are supported
- Request timeouts; a very slow storage call is not cancelled
- Writes are serialised by one lock; no concurrency benchmarks have been run


---
# Phase 5 additions: multi-tenant platform (rajeshdb-platform)

## Implemented (unverified until `cargo test` passes on a real machine)
- Users with e-mail + password (Argon2id, random salt); opaque session tokens (only their SHA-256 is stored), expiry, logout
- Login hardening: identical error for unknown e-mail and wrong password, dummy hash verification for equal timing,
  per-IP and per-e-mail throttling
- Organizations with roles owner / admin / member; add and remove members; one owner per organization
- Projects with fixed environments dev / staging / prod; every project environment is a SEPARATE database directory
- Tenant isolation: every data request is authorized (key scope or org membership) before the database is opened;
  other tenants' projects answer 404; directory names come only from validated 32-hex ids and the fixed env list
- Project API keys: role read/write/admin, bound to one project AND one environment, optional expiry,
  immediate revoke, rotation with optional grace period; plaintext shown once, listing never reveals secrets
- API keys cannot manage users, organizations or other keys
- Per-environment storage quota (real bytes on disk; writes get 507, deletes still allowed) and a usage endpoint
- Audit log per organization (register/login, org, member, project, key and collection events); no secrets recorded
- Limits: projects per org, keys per project, orgs per user; request body size; result size caps

## Not implemented yet (do NOT advertise)
- E-mail verification, password reset, MFA, SSO, account deletion
- Billing, per-plan quotas, per-project custom quotas (one global quota per environment)
- Audit log retention/export; sessions are not listed or revoked individually (only logout / expiry)
- Row-level or collection-level permissions (a key sees its whole environment)
- TLS in the server (reverse proxy, Phase 8); behind a proxy all clients share one IP for throttling
- Open project databases stay in memory until shutdown (no idle eviction)
- Backups, monitoring endpoints and the dashboard (Phases 6-7); no benchmarks have been run
- Data-plane code is a copy of Phase 4's handlers adapted for projects; Phase 4's single-database server is kept as-is


---
# Phase 6 additions: dashboard (dashboard/)

## Verified in the build environment (no network, so no npm install)
- src/lib.ts (filter building, sorting, formatting, JSON parsing): 8 unit tests PASS (node --test)
- Every .ts/.tsx file parses without syntax errors
- The whole app type-checks under strict mode against STUB React typings (only the implicit-any of DOM event
  parameters was relaxed, because the stub has no event types). This checks imports, prop names and null-handling
  between our own modules. It is NOT a substitute for npm run typecheck with the real @types/react

## NOT verified
- npm install, vite build, running in a browser, and end-to-end use against the platform server
- Visual layout on real screens (CSS is hand-written and untested in a browser)

## Implemented
- Sign in / register, session handling (auto sign-out on 401), health indicator
- Organizations, members, audit log; projects with per-environment usage meters
- API keys (create/rotate/revoke, shown once); data browser; query editor with plan; indexes; storage and limits panel

## Not implemented (shown as such in the UI)
- Backup and restore, request metrics and logs, billing, SQL editor (need backend work: Phase 7+)
- No chart library and no Monaco editor: plain text areas keep the dependency list tiny
- No automated browser tests


---
# Phase 7 additions: backup, restore, monitoring

## Verified in the build environment
- Dashboard: lib.ts unit tests 9/9 pass (node --test); all files parse; the app type-checks against STUB React typings
  (not the real @types/react)

## NOT verified (no Rust toolchain or network in the build environment)
- Everything in Rust below has never been compiled. Run `cargo test -p rajeshdb-core -p rajeshdb-docs -p rajeshdb-server -p rajeshdb-platform` first
- npm install / vite build / a browser run of the new Backups tab and Operations page
- A real restore drill on your machine (docs/OPERATIONS.md section 3-4)

## Implemented
- Engine: Db::checkpoint (consistent copy of a live database) and Db::verify_integrity (read + checksum every record)
- Backups per project environment and for the control database: checksummed folders (SHA-256 manifest), atomic completion,
  automatic schedule, retention, pruning
- Verify = full test restore into a scratch folder, live data untouched
- Restore of an environment: verify first, automatic safety backup, directory swap under the exclusive lock, automatic rollback on failure,
  typed confirmation, owner only; backups can be managed with a login only (never with API keys)
- Operator endpoints /ops/* (health with 503 on critical, JSON metrics, Prometheus text, run backups, list control backups), off unless a
  24+ character operator token is configured; constant-time token comparison; failed attempts throttled
- Metrics: request counts by class, latency average, auth failures, rate limits, quota rejections, storage sizes, backup counters/status
- Alerts: disk budget (80% / 95%), backup overdue, backup failed
- Dashboard: Backups tab (create / verify / restore / delete) and a real Operations page (operator token kept in memory only)
- docs/OPERATIONS.md runbook including offline control-database restore and off-site copy

## Not implemented (do NOT advertise)
- Incremental backups, point-in-time recovery, built-in off-site/cloud copies, backup encryption
- Measuring free space of the physical disk (alerts use a configured budget instead)
- Persistent metrics history and alert delivery (use Prometheus/Alertmanager or an uptime monitor)
- Online restore of the control database (offline procedure only)
- Backups pause writers of that database while copying; RAM needed ~ largest table file during verify
- Scheduler lives inside the server process


---
# Phase 8 additions: deployment

## Verified in the build environment
- docker-compose.yml, both GitHub workflows and both OpenAPI files parse as YAML; the compose file publishes no port for the database service
- deploy/offsite-backup.sh passes `bash -n`

## NOT verified (no Docker, no Rust toolchain, no network in the build environment)
- docker build of either image, docker compose up, Caddy certificate issuance, the Caddyfile itself, the GitHub workflows running
- The Rust changes of this phase (trusted-proxy client IP, `healthcheck` command) have never been compiled

## Implemented
- Dockerfile (multi-stage, unprivileged user, read-only filesystem, healthcheck via `rajeshdb-platform healthcheck`) and Dockerfile.web (dashboard + Caddy)
- Compose stack: only Caddy publishes 80/443; persistent host folder for data and backups; log rotation; cap_drop ALL; no-new-privileges
- Caddy: automatic HTTPS, security headers incl. CSP and HSTS, `/api` reverse proxy, `/api/ops/*` blocked except for OPS_ALLOW_IPS
- RAJESHDB_TRUST_PROXY: client IP from the LAST X-Forwarded-For entry so login throttling works behind the proxy (off by default; tests cover spoofing)
- .env.example without secrets; off-site backup script (rsync or rclone); CI (tests, build, dependency audits) and image publishing workflows
- docs/DEPLOYMENT.md: server prep, GitHub, deploy, DNS/HTTPS, persistent disk, disaster recovery, updates, monitoring, security checklist, cost table checked against 2026 provider pages

## Not implemented / known gaps
- No Cargo.lock / package-lock.json is included (generate and commit them: see DEPLOYMENT.md section 4)
- Single node only; no replication, failover, point-in-time recovery or format-migration tooling
- No WAF/DDoS protection; no load testing yet (Phase 9)
- Provider-specific automation (Terraform etc.) is not provided


---
# Phase 9 additions: load testing and security hardening

## Verified in the build environment
- loadtest.mjs: its statistics helpers (2 unit tests pass) and a functional run of all four scenarios against a FAKE local server
  (mock-server.mjs). Those runs prove the tool works; the numbers they produced say nothing about RajeshDB and are not reported anywhere
- Dashboard: 9 unit tests pass; the app type-checks against stub React typings (export button added)
- Compose/workflow YAML parse; the Caddyfile was edited but never run through Caddy

## NOT verified
- All Rust code (never compiled): the new crash-kill test, the benchmark crate, the platform hardening, the filter limits, export, audit pruning
- Benchmarks: **there are no performance results at all**; docs/PERFORMANCE.md has an empty results table to fill in on your own hardware

## Implemented
- Docs layer: filter limits (depth 8, 500 conditions, 1000 values per $in/$nin)
- Platform: request timeout (504), max in-flight requests (503), max concurrent password hashes (503), audit retention pruning, NDJSON export with a cap
  (+ dashboard Export button), Caddy slow-client timeouts
- Crash test: a child process with fsync-per-write is killed 4 times at different points; acknowledged writes must survive; tables must verify
- rajeshdb-bench (engine benchmarks, prints measured results with hardware and dataset size) and loadtest/loadtest.mjs (insert/query/get/mixed/abort)
- docs/PERFORMANCE.md, docs/SECURITY.md (controls <-> tests table, 14 known weaknesses, manual pre-launch checks)

## Not implemented
- Any measured performance data, soak tests, memory-growth tests, TLS-inclusive load tests
- MFA, e-mail verification, password reset, account deletion, encryption at rest, tamper-evident audit log, WAF/DDoS protection
- Fuzzing of the JSON parser / HTTP layer and an external penetration test


---
# Phase 10 additions: documentation and production-readiness review

## Verified in the build environment (actually executed)
- scripts/verify-all.sh in --quick mode: dashboard logic tests, load-test statistics tests, JS SDK tests (6) and Python SDK tests (6) PASS;
  Rust steps correctly report SKIPPED (no cargo) and the script exits non-zero (3) so "skipped" is never mistaken for success
- JS SDK strict TypeScript check passes; both SDKs were exercised against scripted fake servers (not against RajeshDB)
- examples/node/server.mjs worked end-to-end through the SDK against a fake server (valid insert, validation error, bad JSON, list)

## NOT verified
- Everything Rust; scripts/verify-all.ps1 (no PowerShell here); Docker; Caddy; GitHub workflows; browser rendering; any real client <-> server exchange

## Delivered
- scripts/verify-all.ps1 and .sh: run every check and write verification-report.txt
- SDKs: sdk/js (TypeScript, no dependencies, refuses to run in a browser by default, hides the key in logs) and sdk/python (standard library only)
- examples/: Node backend, React component (no key in frontend code), Python script
- docs: ARCHITECTURE, FEATURES (honest matrix), LIMITS, ROADMAP, SYNC_DESIGN (design only), SETUP_WINDOWS, PRODUCTION_READINESS (verdict: not production-ready, 11 gates, risk register)

## Known gaps against the original brief
- No bulk import endpoint; CLI has no backup/restore commands; no Windows service/installer
- "Database users" are API keys, there are no per-collection permissions; usage tracking covers storage bytes only (no request metering)
- Client protocol is HTTP/JSON; no explicit connection-pool object
- Local-to-cloud sync is a design only
- OpenAPI files are hand-written and may drift from the code
- No SQL, no multi-statement transactions, no replication/sharding/HA
