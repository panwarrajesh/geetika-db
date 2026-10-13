# Feature matrix

Status words:
* **Implemented**: code exists. "Tests written / run?" says what evidence there is.
* **Partial**: works for a subset; the gap is stated.
* **Planned**: designed or listed in the roadmap, no code.
* **Not supported**: no plan in the current scope.

**Evidence today:** the JavaScript/Python pieces (dashboard logic, load-test statistics, both SDKs against a scripted fake server) were actually run.
All Rust code has automated tests written but **never executed** by the author (no compiler available). Until `scripts/verify-all` shows PASS on your machine, treat
every Rust feature below as "implemented, unproven". Nothing here is "supported" in the sense of a service level.

## Storage engine (the 13 planned components)

| # | Component | Status | Notes |
|---|---|---|---|
| 1 | Persistent storage engine | Implemented | LSM design: WAL, memtable, table files, manifest |
| 2 | Record and document storage | Implemented | bytes at the core; JSON documents in the document layer |
| 3 | Table / collection management | Implemented | create, drop, list; names 1-64 chars `A-Za-z0-9_-` |
| 4 | Indexing and retrieval | Partial | single-field secondary indexes on dotted paths (scalars only); no compound, unique, text or array-element indexes |
| 5 | Query execution | Partial | filters, sort, skip/limit; sorting and scans materialize results in memory |
| 6 | SQL-compatible queries | Not supported | JSON query language only; no SQL parser exists |
| 7 | JSON document storage and queries | Implemented | `$eq $ne $gt $gte $lt $lte $in $nin $exists $and $or`, dotted paths; no `$not`, regex, aggregation |
| 8 | Transactions and concurrency control | Partial | atomic multi-key batches (all-or-nothing after a crash); per-database reader/writer lock; **no multi-statement transactions, no snapshot isolation. Do not call this ACID** |
| 9 | WAL and crash recovery | Implemented | checksummed frames, torn-tail detection; kill test written (`crash_kill.rs`), not yet run |
| 10 | Data integrity and schema validation | Implemented | CRC32 on every record, SHA-256 on backups; optional schema: top-level field types, required, additionalProperties |
| 11 | Memory management and disk-based storage | Partial | data lives on disk; but all keys of the table files are kept in RAM and lookups open files each time |
| 12 | Query planning and optimization | Partial | picks one index (eq, in, range); no cost model, no joins, no multi-index plans |
| 13 | Import, export, backup, restore | Partial | NDJSON export (capped), full backup/verify/restore implemented; **no bulk import endpoint** (insert via SDK/API) |

## Platform

| Feature | Status | Notes |
|---|---|---|
| User registration and login | Implemented | e-mail + password, Argon2id; no e-mail verification, password reset, MFA or account deletion |
| Organizations, members, roles | Implemented | owner / admin / member |
| Projects and environments | Implemented | dev, staging, prod per project |
| Database instances / namespaces | Implemented | one database directory per project environment |
| Database users and role-based permissions | Partial | API keys with read/write/admin roles bound to one project + environment; no per-collection permissions, no separate "database user" accounts |
| API keys, rotation, revocation | Implemented | rotation with grace period, expiry, instant revoke |
| Storage quotas and usage tracking | Partial | bytes on disk per environment, enforced on writes; no traffic/request metering per project |
| Project isolation and audit logs | Implemented | audit of administrative actions; not tamper-evident |
| Rate limiting, request validation, structured logs | Implemented | see `docs/LIMITS.md` |
| REST API + OpenAPI | Implemented | `docs/openapi-platform.yaml` (+ `docs/openapi.yaml` for the Phase 4 server); the spec files are hand-written, not generated |
| Client protocol for data operations | Partial | HTTP + JSON only; no binary protocol and no explicit connection-pool object (HTTP keep-alive of the client library) |
| SDKs (JavaScript/TypeScript, Python) | Implemented | tested against a scripted fake server only; not published to npm/PyPI |
| React and Node.js examples | Implemented | syntax-checked; the Node example was exercised against a fake server |

## Operation modes

| Feature | Status | Notes |
|---|---|---|
| Local mode on Windows (disk storage, offline) | Implemented | run the platform locally; `rajeshdb` CLI for raw engine access |
| Local CLI | Partial | put/get/delete/scan/flush/compact/stats and document commands; **no backup/restore commands** (use the platform) |
| Local management dashboard | Implemented | the same dashboard against a local platform server |
| Windows service / installer | Not supported | start it from a terminal or Task Scheduler |
| Cloud mode, TLS, persistent disk | Implemented | Docker Compose + Caddy; never run on real infrastructure by the author |
| Health checks, logging, monitoring, automated backups | Implemented | `/health`, `/ops/*`, Prometheus text, JSON logs, scheduler |
| Local-to-cloud synchronization | Planned | design only: `docs/SYNC_DESIGN.md` |
| Replication, partitioning/sharding, distributed queries, high availability | Not supported | single node; see roadmap |

## Dashboard

| Page / feature | Status |
|---|---|
| Overview and system health | Partial (health indicator; Operations page needs the operator token) |
| Projects and environments, keys, members, audit | Implemented |
| Databases, collections, records (browser with search, filter, sort, pagination) | Implemented |
| Query editor | Partial (JSON queries, read-only; no SQL) |
| Storage usage and limits, backup and restore | Implemented |
| Logs and metrics | Partial (server-wide metrics and alerts for the operator; no per-project request logs) |
| Billing-readiness | Not supported (documented only: usage is storage bytes) |
| Documentation page | Implemented (short; points to the docs folder) |
