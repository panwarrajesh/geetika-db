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
