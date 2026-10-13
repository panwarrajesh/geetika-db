# RajeshDB Cloud

An experimental document database engine written in Rust, with a multi-tenant HTTP API, a React dashboard, backups and a deployment recipe.

> **Status: NOT production-ready.** All ten planned phases are written, but the Rust code has never been compiled or run by its author,
> and no performance numbers exist. Start with `scripts\verify-all.ps1` (Windows) or `scripts/verify-all.sh` and read
> **docs/PRODUCTION_READINESS.md**. Nothing here is unlimited, and nothing here is a claim of SQL, ACID, high availability or reliability.

## Where to start

| I want to... | Read |
|---|---|
| Install the tools and run everything on Windows | `docs/SETUP_WINDOWS.md` |
| Check that it works on my machine | run `scripts\verify-all.ps1`, then send/read `verification-report.txt` |
| Understand how it is built | `docs/ARCHITECTURE.md` |
| See what exists, what is partial, what is missing | `docs/FEATURES.md`, `docs/STATUS.md` |
| Know every limit | `docs/LIMITS.md` |
| Deploy on a server with HTTPS | `docs/DEPLOYMENT.md` (+ `deploy/`) |
| Back up, restore, monitor | `docs/OPERATIONS.md` |
| Review security | `docs/SECURITY.md` |
| Measure performance | `docs/PERFORMANCE.md` (empty results table: fill it yourself) |
| Decide whether to launch | `docs/PRODUCTION_READINESS.md` |
| Use it from code | `sdk/js`, `sdk/python`, `examples/`, `docs/openapi-platform.yaml` |
| See what comes next | `docs/ROADMAP.md`, `docs/SYNC_DESIGN.md` |

## Layout

    engine/      Rust workspace: core (engine), docs (documents/indexes/queries), platform (multi-tenant server), server (phase 4), cli, bench
    dashboard/   React + TypeScript console          sdk/   JavaScript/TypeScript and Python clients      examples/   Node, React, Python
    deploy/      Docker, Compose, Caddy, off-site backup   loadtest/   HTTP load test      scripts/   verify-all      docs/   all documentation

## Fastest local start (Windows)

    powershell -ExecutionPolicy Bypass -File scripts\verify-all.ps1 -Quick
    cd engine ; cargo run --release -p rajeshdb-platform -- serve          # window 1
    cd dashboard ; npm install ; npm run dev                               # window 2, then open http://localhost:5173

---

# Phase-by-phase notes (history of how it was built)

## Phase 2 — local storage engine (Rust)

### Windows setup (all free)
1. Install **Git**: https://git-scm.com
2. Install **Visual Studio Build Tools** (workload: *Desktop development with C++*): https://visualstudio.microsoft.com/visual-cpp-build-tools/
3. Install **Rust** from https://rustup.rs (choose the default `stable` toolchain, MSVC).
4. Open a **new** PowerShell and verify:
   ```powershell
   rustc --version
   cargo --version
   ```

### Build and test
```powershell
cd rajeshdb\engine
cargo test
cargo build --release
```

### Try the CLI
```powershell
.\target\release\rajeshdb.exe put name Rajesh
.\target\release\rajeshdb.exe get name
.\target\release\rajeshdb.exe scan
.\target\release\rajeshdb.exe stats
.\target\release\rajeshdb.exe shell
```
Data is stored in `.\rajeshdb-data` (change with `--data D:\mydata`).

### Crash test by hand
Run `shell`, `put a 1`, then close the window with the X button (no `exit`). Start again and `get a`.


## Phase 3 — documents, indexes, queries

New crate `rajeshdb-docs` (collections of JSON documents on top of the engine) and an
atomic `write_batch` in `rajeshdb-core`.

```powershell
cd rajeshdb\engine
cargo test            # runs core + docs tests
cargo build --release
```

Try it (document mode uses its own folder, `.\rajeshdb-docs-data`):
```powershell
.\target\release\rajeshdb.exe --docs shell
```
```
create-collection users
create-index users city
insert users {"name":"Asha","city":"Delhi","age":30}
insert users {"name":"Ben","city":"Pune","age":25}
find users {"city":"Delhi"}
find users {"age":{"$gte":20,"$lt":30}}
query users {"filter":{},"sort":{"field":"age","order":"desc"},"limit":1}
explain users {"city":"Delhi"}
```
Query language: equality, `$eq $ne $gt $gte $lt $lte $in $nin $exists`, `$and`, `$or`,
dotted paths (`"address.city"`). See `docs/STATUS.md` for limits.


## Phase 4 — HTTP server and secure API

```powershell
cd rajeshdb\engine
cargo test
cargo build --release

# 1. create an admin key (shown ONCE; only its hash is stored in rajeshdb-keys.json)
.\target\release\rajeshdb-server.exe keygen my-admin admin

# 2. start the server (listens on 127.0.0.1:8080 only)
.\target\release\rajeshdb-server.exe serve
```

In a second PowerShell window (replace the key):
```powershell
$h = @{ Authorization = "Bearer rdb_PASTE_YOUR_KEY_HERE" }
Invoke-RestMethod http://127.0.0.1:8080/health
Invoke-RestMethod -Method Post -Headers $h -ContentType "application/json" `
  -Uri http://127.0.0.1:8080/v1/collections -Body '{"name":"users"}'
Invoke-RestMethod -Method Post -Headers $h -ContentType "application/json" `
  -Uri http://127.0.0.1:8080/v1/collections/users/documents -Body '{"name":"Asha","city":"Delhi"}'
Invoke-RestMethod -Method Post -Headers $h -ContentType "application/json" `
  -Uri http://127.0.0.1:8080/v1/collections/users/query -Body '{"filter":{"city":"Delhi"}}'
Invoke-RestMethod -Headers $h http://127.0.0.1:8080/v1/admin/metrics
```
Create a read-only key for apps that only read: `rajeshdb-server keygen my-app read`.
Revoke: `rajeshdb-server keys revoke <id>` then restart the server.
Config via environment variables: run `rajeshdb-server help`. API reference: `docs/openapi.yaml`.


## Phase 5 — multi-user, multi-project platform (`rajeshdb-platform`)

A separate binary and crate. Phase 4's `rajeshdb-server` is unchanged. If the platform crate has a
compile error, you can still test everything else with:
```powershell
cargo test -p rajeshdb-core -p rajeshdb-docs -p rajeshdb-server
```
Then the platform on its own:
```powershell
cargo test -p rajeshdb-platform
cargo build --release
.\target\release\rajeshdb-platform.exe serve        # http://127.0.0.1:8081
```
Walkthrough (PowerShell; replace the values printed by each step):
```powershell
$u = "http://127.0.0.1:8081"; $j = "application/json"
Invoke-RestMethod -Method Post -Uri $u/v1/auth/register -ContentType $j -Body '{"email":"me@example.com","password":"a long passphrase"}'
$t = (Invoke-RestMethod -Method Post -Uri $u/v1/auth/login -ContentType $j -Body '{"email":"me@example.com","password":"a long passphrase"}').token
$h = @{ Authorization = "Bearer $t" }
$org = (Invoke-RestMethod -Method Post -Headers $h -Uri $u/v1/orgs -ContentType $j -Body '{"name":"My Company"}').org_id
$proj = (Invoke-RestMethod -Method Post -Headers $h -Uri $u/v1/orgs/$org/projects -ContentType $j -Body '{"name":"shop"}').project_id
$key = (Invoke-RestMethod -Method Post -Headers $h -Uri $u/v1/projects/$proj/keys -ContentType $j -Body '{"name":"backend","env":"dev","role":"admin"}').api_key
$kh = @{ Authorization = "Bearer $key" }
Invoke-RestMethod -Method Post -Headers $kh -Uri $u/v1/projects/$proj/envs/dev/collections -ContentType $j -Body '{"name":"users"}'
Invoke-RestMethod -Method Post -Headers $kh -Uri $u/v1/projects/$proj/envs/dev/collections/users/documents -ContentType $j -Body '{"name":"Asha"}'
Invoke-RestMethod -Method Post -Headers $kh -Uri $u/v1/projects/$proj/envs/dev/collections/users/query -ContentType $j -Body '{}'
Invoke-RestMethod -Headers $kh -Uri $u/v1/projects/$proj/envs/dev/usage
```
API reference: `docs/openapi-platform.yaml`. Configuration: `rajeshdb-platform help`.


## Phase 6 - dashboard (React + TypeScript)

See dashboard/README.md. Short version:
```powershell
# terminal 1
cd rajeshdb\engine ; cargo run --release -p rajeshdb-platform -- serve
# terminal 2
cd rajeshdb\dashboard ; npm install ; npm run dev     # http://localhost:5173
npm test                                               # pure-logic unit tests
```


## Phase 7 - backup, restore, monitoring

Full runbook: **docs/OPERATIONS.md**. Quick start (PowerShell):
```powershell
$env:RAJESHDB_OPERATOR_TOKEN = "choose-a-long-random-secret-here-123"   # enables /ops/*
$env:RAJESHDB_BACKUP_INTERVAL_SECS = "3600"                              # automatic backup every hour
cd rajeshdb\engine
cargo test -p rajeshdb-core -p rajeshdb-docs -p rajeshdb-server -p rajeshdb-platform
cargo run --release -p rajeshdb-platform -- serve
```
Then in the dashboard: environment -> **Backups** (create, verify, restore) and **Operations** (paste the operator token).
Command-line checks:
```powershell
$o = @{ Authorization = "Bearer $env:RAJESHDB_OPERATOR_TOKEN" }
Invoke-RestMethod -Headers $o http://127.0.0.1:8081/ops/health
Invoke-RestMethod -Headers $o http://127.0.0.1:8081/ops/metrics
Invoke-RestMethod -Method Post -Headers $o http://127.0.0.1:8081/ops/backup/run
```


## Phase 8 - cloud deployment (Docker + Caddy + HTTPS)

Everything is in `deploy/`; the full guide is **docs/DEPLOYMENT.md**.
```powershell
# local rehearsal (needs Docker Desktop): https://localhost with a self-signed certificate
cd rajeshdb\deploy
copy .env.example .env          # then set DATA_MOUNT=rajeshdb-data and SITE_ADDRESS=localhost in .env
docker compose up -d --build
```
Production on a VPS: see the guide (server prep, GitHub, domain + HTTPS, persistent disk, off-site backups, updates, cost table).


## Phase 9 - load testing and security hardening

* **No benchmark numbers exist yet, on purpose** (nothing could be measured without a compiler). Tools: `rajeshdb-bench` (engine), `loadtest/loadtest.mjs`
  (HTTP, incl. a network-interruption scenario) and the crash-kill test inside `cargo test`. Guide and results log: **docs/PERFORMANCE.md**.
* Hardening added: request timeout, max concurrent requests, max concurrent password hashes, query cost limits, audit retention, collection export,
  slow-client timeouts in Caddy. Controls, tests and an honest list of weaknesses: **docs/SECURITY.md**.
```powershell
cd rajeshdb\engine
cargo test --workspace
cargo run --release -p rajeshdb-bench -- --size small
cd ..\loadtest ; node --test loadtest.test.mjs
```
