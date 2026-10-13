# Production-readiness review (Phase 10)

## Verdict

**RajeshDB is NOT production-ready. It is an experimental, single-node system whose Rust code has never been compiled or run by its author.**
Every statement of reliability, security or performance in this repository describes intent and design plus tests that are written but not yet executed.
Do not put data you cannot afford to lose into it, and do not offer it to other people as a service, until every gate in section 3 is ticked.

What can honestly be said today: "an experimental document database engine in Rust with a multi-tenant HTTP API, a dashboard, backups and a deployment recipe,
currently being brought up for the first time".

## 1. How this review was done (and its limits)

* Written from the code and the documents; no compiler, no Docker, no real server and no network were available when the code was produced.
* Actually executed in the build environment: unit tests of the dashboard's pure logic (9), load-test statistics (2), the JavaScript SDK (6) and the Python SDK (6)
  against scripted fake servers, the load-test tool and the Node example against a fake server, syntax checks of YAML/shell/TypeScript, a strict TypeScript check of the SDK and
  (with stub React typings) of the dashboard.
* Never executed: all Rust code (engine, document layer, both servers, CLI, benchmark crate), the Rust tests (including the crash-kill and restore tests), Docker builds, Caddy,
  the GitHub workflows, the PowerShell script, any browser rendering, any real HTTP exchange between the dashboard/SDKs and the server.
* Some Rust files were re-read for errors after being written (engine `db.rs`, platform state and middleware); nothing was found, which proves little. The first compile will find mistakes.
* A different reviewer (a second person or a tool such as `cargo clippy`, `cargo audit`, an external penetration test) has not looked at it.

## 2. Final acceptance criteria from the project brief

| Criterion | Where it stands | What is missing |
|---|---|---|
| A genuine, independently implemented database engine | Own code for WAL, memtable, table files, manifest, compaction, checkpoint, document layer, indexes, planner (no wrapped database) | Never compiled or run; far less mature than established engines; known scale limits (keys in RAM) |
| A secure multi-project management platform | Auth, roles, isolation by directory + authorization, keys, quotas, audit, limits, hardening written; 14 known weaknesses listed in `SECURITY.md` | Not audited, not penetration-tested, tests never executed |
| Local and online deployment | Local run documented; Docker/Compose/Caddy/CI written; rehearsal steps in `DEPLOYMENT.md` | Nothing was ever built or deployed |
| Durable data storage | WAL + fsync + atomic publication + crash-kill test | Test never run; no power-loss testing; no file-format versioning |
| Documented APIs | OpenAPI files, SDK docs, examples, runbooks | OpenAPI is hand-written (can drift); SDKs unpublished |
| A professional dashboard | Console with all planned pages written | Never rendered in a browser; no browser tests; typechecked only against stubs |
| Verified backup and recovery | Backup, verification (full test restore) and restore with rollback implemented | **Never verified in practice: run a restore drill (`DEPLOYMENT.md` section 7 and 9)** |

## 3. Gates before any real user data (tick every box, in order)

- [ ] **G1** `scripts/verify-all` is all PASS on your Windows machine; `cargo test --workspace --release` also passes
- [ ] **G2** GitHub CI is green; `Cargo.lock` and `package-lock.json` are committed; `cargo audit` / `npm audit` findings reviewed
- [ ] **G3** `crash_kill` passes 20 times in a row; plus a manual power-cut test (pull the plug during a write loop on a test machine, then verify)
- [ ] **G4** Benchmarks and load test run on the hardware you will use; results recorded in `PERFORMANCE.md`; they meet your needs with headroom; `abort` scenario passes
- [ ] **G5** Restore drill done twice: single environment restore, and full disaster recovery onto a fresh server from the off-site copy
- [ ] **G6** Deployment rehearsal on a throw-away server: HTTPS works, port 8081 and `/api/ops` closed from outside, off-site backup runs from cron, monitoring alerts fire when you stop the server
- [ ] **G7** Upgrade rehearsal (new version over old data) and rollback rehearsal
- [ ] **G8** `SECURITY.md` section 4 executed; cross-tenant tests repeated by hand; someone other than the author tried to break it
- [ ] **G9** Disk encryption on the server and encrypted off-site copies; secrets only in `.env`/a secret store
- [ ] **G10** Operations basics: someone is on call, an uptime monitor and `/ops/health` alerting are configured, a written incident procedure exists
- [ ] **G11** Users are told the limits in `LIMITS.md` and the lack of SLA/replication in writing; backups and data-location/privacy obligations that apply to your users have been checked with qualified advice

## 4. Claims you must not make (from the brief) and what is true

| Claim | Status |
|---|---|
| Full SQL | Not supported at all (no SQL parser) |
| ACID transactions | Not claimed. True: single writes and multi-key batches are atomic and durable (fsync mode) after a crash; writers are serialized per database. Missing: multi-statement transactions, defined isolation levels |
| Distributed consistency | Not applicable: one node, no replication |
| High availability | No. One server down = service down |
| Production reliability | Not until section 3 is complete |
| Unlimited storage, users or traffic | Never: see `LIMITS.md`; quotas and your disk decide |
| Performance figures | None exist yet |

## 5. Supported now / experimental / planned (summary of `FEATURES.md`)

* **Implemented, unproven:** engine, document layer, platform, dashboard, backups, deployment recipe, SDKs.
* **Experimental (use only for non-critical data once G1-G7 pass):** everything above, plus indexes, the planner, restore.
* **Planned:** local-to-cloud sync, replication, SQL subset, transactions, MFA/e-mail flows, streaming compaction and block cache (`ROADMAP.md`).

## 6. Top risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | First compile/test run reveals many errors or logic bugs | Near certain | Blocks everything | R0 in the roadmap; verification script gives one report |
| 2 | Data-loss bug in recovery/compaction/restore (code is new and unrun) | Medium | Severe | crash_kill, restore tests, drills, frequent verified off-site backups, start with disposable data |
| 3 | Cross-tenant data exposure through a bug (one process serves all tenants) | Low-medium | Severe | tests, manual review, separate instances for sensitive tenants |
| 4 | RAM exhaustion by a tenant with many keys (keys of open databases live in RAM) | Medium | Outage | small quotas, monitoring, roadmap R1 |
| 5 | Single server failure | Medium | Outage / data loss since the last off-site copy | off-site backups every few hours, restore drill, say "no HA" |
| 6 | Operator mistakes (deleting a volume, wrong `.env`, exposing `/ops`) | Medium | Severe | runbooks, checklists, host-folder storage, Caddy rule |
| 7 | Unknown vulnerabilities in new auth code | Medium | Severe | external review, dependency audits, rate limits, small user base first |
| 8 | Upgrade without format versioning breaks old data | Medium | Severe | verified backup before every upgrade; add format versioning (R1) |
| 9 | Cost surprises or provider changes (free tiers shrink, prices move) | High | Moderate | review prices on purchase day; no free-tier production |
| 10 | Legal/privacy obligations toward your users unaddressed | Unknown | Severe | take qualified advice before offering the service |

## 7. Suggested launch path

1. **Private alpha:** you only, disposable data, all gates G1-G7.
2. **Friendly beta:** a few known users, non-critical data, written disclaimer, off-site backups every few hours, you read the metrics daily; G8-G11.
3. **Paid or public service:** only after a quiet period with no data-loss incidents, an external security review, tested capacity numbers and a replication/failover answer you are comfortable with (roadmap R6) or a clearly communicated single-node SLA-free offer.
