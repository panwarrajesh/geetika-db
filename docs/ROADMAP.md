# Roadmap (ordered by what should happen first)

Dates are deliberately absent. A step starts only when the previous one is finished and proven.

## R0 - Prove what exists (blocks everything else)
1. `scripts/verify-all` passes on a real Windows machine: all Rust tests (including `crash_kill`), dashboard typecheck/tests/build, SDK tests, `docker compose config`.
2. Fix whatever the first compile and test run reports (expect compile errors: nothing Rust was ever compiled before delivery).
3. Commit `Cargo.lock` and `package-lock.json`; CI green on GitHub.
4. Run the benchmarks and the load test, fill in the results table in `docs/PERFORMANCE.md`.
5. Deployment rehearsal on a throw-away server, including a restore drill and an upgrade.

## R1 - Engine reliability and scale on one node
* Streaming merge for compaction and verification (today they load whole tables in RAM).
* Sparse block index and a block/file-handle cache (today every key is in RAM and every lookup opens a file); Bloom filters.
* Background compaction with size-tiered or leveled policy instead of "merge everything".
* Exclusive directory lock so two processes can never open one database.
* Group commit for the WAL (many writers share one fsync).
* File format version number and an upgrade path (today there is none).
* Power-loss testing on real hardware / fault-injection file system.

## R2 - Data model
* Compound and unique indexes; array-element queries; `$not`, `$regex`-like prefix queries; aggregation (count/sum/group).
* Multi-document transactions with a defined isolation level (needs a clear design first; until then do not say "ACID").
* Bulk import endpoint and CLI import/export.
* Schema: nested fields, formats, defaults.

## R3 - Platform
* E-mail verification, password reset, MFA, account deletion and data-erasure workflow.
* Per-project custom quotas, request metering and usage history; plans and billing readiness (metering first, payments later).
* Key restrictions (IP allow-list, collection scopes); per-session list and revoke; audit log export; encryption at rest option.
* Metrics history, per-project logs in the dashboard, alert delivery (webhook/e-mail).
* Windows service/installer for local mode; CLI backup/restore commands.

## R4 - Developer experience
* Publish the SDKs (npm / PyPI) with versioned releases; generate the OpenAPI file from code; typed clients from it.
* A real connection-pool/retry policy in the SDKs; pagination helpers; examples for more frameworks.
* A SQL subset (SELECT/INSERT/UPDATE/DELETE, WHERE, ORDER BY, LIMIT, single-table first) on top of the document layer, clearly labelled experimental.

## R5 - Local-to-cloud synchronization
Implement the design in `docs/SYNC_DESIGN.md`: change feed, versions, conflict detection, resumable cursors. Start one-directional (cloud pulls from local), then two-way.

## R6 - Scale beyond one node (only after R1-R2 are solid)
1. Read replicas by shipping the WAL to a follower; consistency documented as "eventually consistent reads".
2. Automated failover requires consensus (Raft or an external coordinator) - a large project of its own.
3. Partitioning/sharding of collections; distributed query planning.
Each step needs its own recovery and consistency tests before any marketing claim.
