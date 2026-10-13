# Operations runbook (Phase 7)

Everything here describes what the code does today. Nothing in this phase has been compiled or run
on a real machine yet by the author: treat the first backup AND the first restore as a drill.

## 1. What is backed up

| What | How | Where |
|---|---|---|
| Every project environment database (dev / staging / prod) that has been used | automatic schedule, "Back up now" in the dashboard, or the API | `<data>/backups/projects/<project_id>/<env>/<backup_id>/` |
| The control database (users, organizations, projects, API key hashes, audit log) | automatic schedule, or "Run backups now" in Operations | `<data>/backups/control/<backup_id>/` |

A backup is a plain folder: copies of the database files plus `backup.json` (file names, sizes, SHA-256 of
every file, record count). It is written to `<id>.partial` and renamed when complete, so an interrupted backup
never looks valid. Before it is declared finished, every record of the live database is read back and
checksummed (so a backup run also detects bit rot in live data).

**Backups contain your data in clear text** (and the control backup contains password hashes and API key
hashes). Protect the folder like the data itself.

## 2. Automatic backups and retention

Environment variables of `rajeshdb-platform`:

| Variable | Default | Meaning |
|---|---|---|
| RAJESHDB_BACKUP_INTERVAL_SECS | 21600 (6 h) | 0 turns automatic backups off. The first run happens one interval after start. |
| RAJESHDB_BACKUP_RETENTION | 7 | newest N backups kept per database; at least 1 is always kept |
| RAJESHDB_DISK_BUDGET_BYTES | 0 | alert at 80% (warning) and 95% (critical) of this size for data + backups; 0 = off |
| RAJESHDB_OPERATOR_TOKEN | unset | secret (24+ chars) for /ops/*; unset disables /ops completely |

While a database is being copied its writers wait (the copy needs the write lock). The pause grows with the
size of that one database. A full backup is a full copy: disk use is roughly data size x retention.

## 3. Taking and checking a backup by hand

* Dashboard: project -> environment -> **Backups** -> "Back up now"; **Verify** on any row.
* **Verify** is a real restore test: it re-checks every file's SHA-256, copies the files to a scratch folder,
  opens them as a database, reads every record and compares the record count with the manifest. Live data is not touched.
* API: `POST /v1/projects/{pid}/envs/{env}/backups`, `GET .../backups`, `POST .../backups/{id}/verify`.
  Allowed for organization owners and admins using their LOGIN. API keys can never manage backups.

Make verification a habit: a backup you have never restored is a hope, not a backup.

## 4. Restoring one environment (owner only)

Dashboard: environment -> Backups -> **Restore** (you must type the environment name), or
`POST /v1/projects/{pid}/envs/{env}/backups/{id}/restore` with body `{"confirm": "<env>"}`.

What happens, in order:
1. The chosen backup is fully verified. If anything is wrong the restore is refused and nothing changes.
2. The CURRENT data is saved as a new "safety" backup (it is not removed by retention). The response names it.
   You can undo a restore by restoring that safety backup.
3. With the environment locked, the verified files are staged, the old directory is moved aside, the staged one
   is moved into place and opened. If opening fails, the old directory is put back automatically.

Requests that arrive during the swap wait for the lock; a request that was already running may fail once: retry it.
Everything written after the backup's time is gone after a restore (that is what restoring means).

## 5. Restoring the control database (offline)

This is deliberately not an API call. Do it with the server stopped (Ctrl+C), in PowerShell
(adjust paths; pick an id from `<data>\backups\control`):

    cd D:\rajeshdb\engine
    Rename-Item rajeshdb-platform-data\control control.broken
    New-Item -ItemType Directory rajeshdb-platform-data\control | Out-Null
    Copy-Item rajeshdb-platform-data\backups\control\<backup_id>\* rajeshdb-platform-data\control\ -Exclude backup.json
    .\target\release\rajeshdb-platform.exe serve

Users who registered, and projects/keys created after that backup, will be missing; sessions of removed users stop working.

## 6. Copy backups off the machine

Backups on the same disk do not survive a dead disk. Copy the `backups` folder elsewhere regularly, e.g. with
Windows Task Scheduler running:

    robocopy D:\rajeshdb\engine\rajeshdb-platform-data\backups E:\offsite\rajeshdb-backups /E /XO

(`/E` copies subfolders, `/XO` skips older files. It does not delete anything at the destination.)
Encrypt the destination if it is not under your control. Phase 8 adds guidance for cloud object storage.

## 7. Monitoring

All of these need the operator token (`Authorization: Bearer <token>`) and must NOT be reachable from the
public internet (block `/ops/` in the reverse proxy; Phase 8 shows how).

| Endpoint | Purpose |
|---|---|
| GET /ops/health | deep check: control database readable + alerts. HTTP 200 for ok/degraded, **503 for critical**, so an uptime monitor can alert on the status code |
| GET /ops/metrics | JSON: requests, status classes, latency average, failed logins, rate limits, quota rejections, storage sizes, backup status, alerts |
| GET /ops/metrics/prometheus | the same numbers in Prometheus text format |
| POST /ops/backup/run | run backups of the control database and every used environment now |
| GET /ops/backups/control | list control-database backups |

Prometheus scrape example:

    scrape_configs:
      - job_name: rajeshdb
        metrics_path: /ops/metrics/prometheus
        authorization: { type: Bearer, credentials: "<operator token>" }
        static_configs: [ { targets: ["127.0.0.1:8081"] } ]

Alerts the server raises by itself:
* `disk_budget` warning/critical: data + backups reached 80% / 95% of RAJESHDB_DISK_BUDGET_BYTES
* `backup_overdue` critical: automatic backups are on but none succeeded within twice the interval
* `backup_failed` warning: the latest backup attempt failed (details in the server log and in /ops/metrics)

Per-request logs are JSON lines on stdout (method, path, status, milliseconds, principal id; never bodies,
query strings or credentials). Redirect them to a file or a log collector when running as a service.

## 8. Known limits (honest list)

* Disk alerts are based on a budget YOU configure. The free space of the physical disk is not measured.
* Snapshots only: no incremental backups and no point-in-time recovery. Data written after the last backup is lost in a disaster.
* Verification and backup read each table file fully into memory: RAM needed is about the size of the largest table file.
* Metrics are in-memory counters since process start (they reset on restart); there is no built-in history or alert delivery
  (use Prometheus/Alertmanager or an uptime monitor on /ops/health).
* The scheduler runs inside the server process: if the server is down, no backups run (that is what `backup_overdue` and external monitoring are for).
* Quota accounting counts files in the environment directory only, not its backups.
