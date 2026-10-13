# Architecture

## 1. Big picture

    Browser (React dashboard) ----\
    Your backend (SDK + API key) --+--> HTTPS (Caddy: TLS, headers, /ops blocked) --> rajeshdb-platform (HTTP API, one process)
    curl / scripts ---------------/                                                   |
                                                                                      |  control plane: users, orgs, projects, keys, audit
                                                                                      |  data plane:    one DocDb per project environment
                                                                                      v
                                              <data>/control/                     (a DocDb holding the control data)
                                              <data>/projects/<project_id>/<env>/ (a DocDb per environment)
                                              <data>/backups/...                  (checksummed snapshots)

Layers, bottom to top, each in its own crate under `engine/crates/`:

| Crate | Responsibility |
|---|---|
| `rajeshdb-core` | the storage engine: key/value bytes, WAL, memtable, immutable table files, manifest, compaction, atomic batches, checkpoint, integrity verification |
| `rajeshdb-docs` | collections of JSON documents, optional schema, secondary indexes, query language and planner, built ONLY on the public API of `core` |
| `rajeshdb-server` | Phase 4 single-database HTTP server (kept as a simple reference; not used by the platform) |
| `rajeshdb-platform` | multi-tenant control plane + data plane over HTTP: users, organizations, projects/environments, API keys, quotas, audit, backups/restore, operator metrics |
| `rajeshdb-cli` | command line for the engine (key/value mode) and the document layer (`--docs`), local only, no authentication |
| `rajeshdb-bench` | engine benchmarks that print only measured results |
| `dashboard/` | React + TypeScript console for the platform API |
| `sdk/js`, `sdk/python` | official clients; `examples/` show backend and React usage |
| `deploy/` | Dockerfiles, Compose, Caddy, off-site backup script; `.github/workflows` CI and image build |

## 2. Technology choices

**Engine language: Rust** (C++ was evaluated). Memory safety without a garbage collector removes whole classes of corruption bugs
that matter most in storage code; Cargo gives tests, benchmarks and dependency management in one tool; Windows builds are straightforward.
Trade-offs accepted: a steeper learning curve (borrow checker), a smaller pool of database-specific libraries than C++, and compile times.
C++ would offer more mature storage libraries and tooling, but also use-after-free and data-race risks in exactly the code that must never be wrong.
Python/Node are used only outside the engine (SDK, tooling, load test); the dashboard is TypeScript so API types are shared by convention.

**Is the engine original?** The storage engine (WAL, memtable, SSTable-style files, manifest, compaction, checkpoint, checksums, document layer,
indexes, query planner) is code written for this project, using the well-known LSM-tree design. It does not wrap SQLite, RocksDB or any other database.
Libraries are used for everything that is not the storage engine: HTTP (axum/tokio), JSON (serde_json), password hashing (argon2),
SHA-256 (sha2) and OS randomness (getrandom). The design is NOT novel research and it is far less mature than established engines.

## 3. How the engine works

**Write path.** `put` -> encode a frame `[crc32][length][tag, key, value]` -> append to `wal.log` -> (optionally) fsync -> insert into the in-memory
sorted memtable. A batch is ONE frame containing many operations, so after a crash all of them or none are visible (this is what keeps a document and its index
entries consistent). When the memtable reaches its size limit it is written to a new immutable table file (`NNNNNN.sst`): data frames sorted by key,
then an index and a footer with a checksum and magic number. The file is written as `.tmp`, fsynced, renamed, the `MANIFEST` (list of live table ids) is replaced
atomically, and only then is the WAL truncated.

**Read path.** memtable first, then table files newest to oldest; the first hit wins; a tombstone means "deleted". Every record read from a table is
checksum-verified. Scans merge all sources. (Known limitation: each table's full key index lives in RAM, and every lookup opens the file.)

**Recovery.** On open: read `MANIFEST`; delete leftover `*.tmp` files and table files not listed in the manifest; open the listed tables (index checksums
verified); replay the WAL frame by frame until the first incomplete or corrupt frame, truncate the WAL there, and continue. Replaying records that were already
flushed is harmless because the same values are applied again.

**Compaction.** Merges all tables into one, drops overwritten values and tombstones, publishes the new manifest, then deletes the old files (orphans are cleaned on the next open).

**Checkpoint / backup.** Flush, copy the immutable table files, write a fresh manifest: the copy is a normal database directory. Verification re-reads every record.

**Concurrency.** One reader-writer lock per database inside the platform: many readers or one writer. There are no multi-statement transactions or snapshot isolation.

## 4. The document layer

Everything is stored as engine keys: `m/<coll>` (metadata), `d/<coll>/<id>` (JSON document), `i/<coll>/<field>/<encoded value><id>` (index entry).
The index value encoding is order-preserving and self-delimiting (numbers as transformed IEEE-754 bytes, strings with escaped NUL and a terminator), which makes
equality and range scans plain byte-range scans. Writes (document + index diffs) are one atomic batch. The planner picks at most one index among the top-level
AND conditions (equality < `$in` < range) and then ALWAYS re-checks the full filter on every candidate, so an index can only narrow results, never change them.

## 5. The platform

* **Control plane data** (users, sessions, organizations, members, projects, API keys, audit) lives in its own `DocDb` at `<data>/control`.
* **Authentication:** passwords hashed with Argon2id; session tokens (`rdbs_...`) and API keys (`rdb_...`) are random 256-bit values stored only as SHA-256 hashes.
* **Authorization:** organization roles owner > admin > member; data roles read < write < admin. A key is bound to one project and one environment.
* **Tenant isolation:** every project environment is a separate directory and a separate `DocDb`; the path is built only from a validated 32-hex project id and a fixed
  environment name; authorization happens before the database is opened; anything outside your scope answers 404.
* **Operations:** quotas (bytes on disk), backups, restore with automatic rollback, metrics, alerts, rate limits, load shedding, audit log.

## 6. Deployment topology

Docker Compose runs two containers: Caddy (only published ports 80/443; TLS certificates; static dashboard; reverse proxy for `/api`) and the platform
(internal network only, non-root, read-only root filesystem). Data and backups sit in one host folder. See `docs/DEPLOYMENT.md`.

## 7. What this architecture deliberately does not do (yet)

Single node only; no replication, sharding, distributed queries or automatic failover; no SQL; no multi-document transactions; no binary wire protocol
(HTTP + JSON is the client protocol); no encryption at rest. See `docs/FEATURES.md` and `docs/ROADMAP.md`.
