# Performance and load testing (Phase 9)

## Status: NO RESULTS EXIST YET

This project has **no benchmark numbers**, on purpose. The author's build environment had no Rust compiler, so
nothing could be measured, and numbers must never be invented. Do not quote any performance figure for RajeshDB
until you have produced it yourself with the tools below and written it into section 5 together with the hardware.

## 1. Engine benchmarks (`rajeshdb-bench`)

    cd D:\rajeshdb\engine
    cargo run --release -p rajeshdb-bench -- --size small
    cargo run --release -p rajeshdb-bench -- --size medium --dir D:\bench --json results-medium.json

* **Always `--release`.** A debug build is many times slower and the tool prints a warning.
* `--dir` selects the disk to measure; results include the speed of that disk (fsync latency dominates the durable-write rows).
* Presets: small = 20k KV pairs / 300 fsync writes / 2k documents; medium = 200k / 2k / 20k; large = 1M / 5k / 100k.
  "large" needs a few hundred MB of free disk and noticeable RAM (the engine keeps every key of the table files in memory).
* Scenarios: KV put without fsync, KV put with fsync on every write, random get hit / miss, prefix scan, backup copy time,
  full integrity verification, WAL-replay recovery time, document insert with two indexes (each insert one atomic batch),
  query by indexed equality vs. the same query unindexed (full scan), indexed range query.
* Each row reports operations, seconds, ops/sec and p50/p99 latency where individual operations are timed.

## 2. HTTP load test (`loadtest/loadtest.mjs`)

Needs Node 18+. Start the platform server first (release build), then, ideally from a **second machine**:

    node loadtest\loadtest.mjs --base http://SERVER:8081 --scenario mixed --concurrency 16 --duration 30 --json mixed.json
    node loadtest\loadtest.mjs --base http://SERVER:8081 --scenario insert --concurrency 8  --duration 30
    node loadtest\loadtest.mjs --base http://SERVER:8081 --scenario query  --concurrency 32 --duration 30
    node loadtest\loadtest.mjs --base http://SERVER:8081 --scenario abort  --concurrency 16 --duration 10

* The tool registers a throw-away user and project by itself (needs `RAJESHDB_ALLOW_REGISTRATION=true`) and seeds documents.
  For a deployment with registration closed, pass `--key rdb_... --project <id> --env dev --collection load`.
* Registration and login are rate limited per IP by design: a load test from one machine only hits those a few times.
  The default API-key rate limit is 50 requests/s with a burst of 100 **per key**. Raise `RAJESHDB_RATE_PER_SEC` and
  `RAJESHDB_RATE_BURST` on the TEST server, otherwise you are measuring the rate limiter (you will see HTTP 429 in `by_status`).
* `RAJESHDB_MAX_INFLIGHT` (default 256) sheds load with HTTP 503 and `RAJESHDB_REQUEST_TIMEOUT_SECS` (30) returns 504: both show up in `by_status`.
* `abort` scenario = network interruption test: clients cancel requests after 1-5 ms; the tool then checks `/health` and a normal
  request still work (exit code 1 if not).
* The tool prints client hardware (OS, CPU, Node version) next to the results. Also write down the SERVER hardware, disk type and settings.

`loadtest/mock-server.mjs` is a fake server used only to test the tool itself. It is not RajeshDB.

## 3. Crash and durability test (runs inside `cargo test`)

`rajeshdb-core/tests/crash_kill.rs` starts a writer process that fsyncs every write and prints an acknowledgement after each one,
kills it abruptly (no flush, no shutdown) four times at different moments on the same directory, and after every kill checks that
every acknowledged write is present, acknowledged deletes stayed deleted, and every table record passes its checksum.
It proves recovery after a process kill. It does **not** prove survival of power loss on every disk or file system.

## 4. What is NOT measured by these tools

* Behaviour with data sets larger than RAM: the engine keeps the full key index of all table files in memory (known limitation).
* Long-running soak tests (days), memory growth, or fragmentation.
* Multi-node anything: there is only one node.
* Real internet conditions and TLS overhead (run the load test through the real Caddy URL for that).
* Comparisons with other databases: none are made or implied.

## 5. Results log (fill this in yourself)

Copy the printed tables here. A result without hardware is worthless.

| Date | Machine (CPU, RAM) | Disk type | OS | Build | Tool + options | Dataset | Headline numbers | Notes |
|---|---|---|---|---|---|---|---|---|
| (none yet) | | | | | | | | |

## 6. Known performance limits that you should expect to see

* Point reads open the table file on every lookup (no file-handle or block cache yet).
* Prefix/range scans, document queries and exports materialize results in memory.
* Full-scan queries read every document of the collection; only equality, `$in` and range conditions on indexed fields use an index.
* Writes are serialized per database by one lock; a backup or compaction pauses that database's writers.
* Every durable write pays one fsync; expect latency to be set by your disk.
