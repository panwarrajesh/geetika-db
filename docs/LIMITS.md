# Resource limits and defaults

Nothing in RajeshDB is unlimited. Defaults below come from the code; "env" shows how an operator changes them.

## Storage engine
| Item | Limit |
|---|---|
| Key size | 64 KiB, must not be empty |
| Value size | 16 MiB |
| One atomic batch | 32 MiB in total |
| Memtable before it is flushed to a table file | 4 MiB (platform default) |
| Durability | fsync on every write by default (`RAJESHDB_SYNC_WRITES`); without it the OS may lose the last writes on power loss |
| Memory | all keys of all table files of an open database are held in RAM; compaction and verification read whole table files into memory |

## Documents and queries
| Item | Limit |
|---|---|
| Collection name | 1-64 characters `A-Za-z0-9_-` |
| Document id | 1-512 bytes, string |
| Field path | at most 256 characters, no `/`, no empty segments |
| Indexed values | null, boolean, number, string up to 1024 bytes; arrays and objects are not indexed; numbers are compared as 64-bit floats (integers above 2^53 are not distinguished) |
| Request body (documents, queries) | 1 MiB (`RAJESHDB_MAX_BODY_BYTES`) |
| Filter complexity | nesting depth 8; 500 conditions in total; 1000 values per `$in` / `$nin` |
| Query result size | default 100, hard maximum 1000 per request (`RAJESHDB_DEFAULT_LIMIT`, `RAJESHDB_MAX_LIMIT`); use `skip`/`limit` to page |
| Export | at most 50,000 documents per call (`RAJESHDB_MAX_EXPORT_DOCS`); the response says when it was truncated |
| Update | shallow merge of top-level fields only; schemas validate top-level fields only |

## Platform
| Item | Default | Env |
|---|---|---|
| Storage quota per project environment | 100 MiB (bytes of all files on disk) | `RAJESHDB_PROJECT_QUOTA_BYTES` |
| Organizations per user (as owner) | 10 | `RAJESHDB_MAX_ORGS_PER_USER` |
| Projects per organization | 20 | `RAJESHDB_MAX_PROJECTS_PER_ORG` |
| API keys per project | 20 | `RAJESHDB_MAX_KEYS_PER_PROJECT` |
| Environments per project | 3 (dev, staging, prod), fixed | - |
| Names | organization 1-100 characters; project 1-64 `A-Za-z0-9_-`; key name 1-64 | - |
| Password | 10-128 characters; Argon2id with 19456 KiB memory | `RAJESHDB_ARGON_MEMORY_KIB` |
| Session lifetime | 24 hours | `RAJESHDB_SESSION_TTL_SECS` |
| Rate limit per key or session | 50 requests/s, burst 100 | `RAJESHDB_RATE_PER_SEC`, `RAJESHDB_RATE_BURST` |
| Register/login per client IP | 1/s, burst 10; login per e-mail 1 per 5 s, burst 10; failed credentials per IP 1/s, burst 10 | fixed |
| Concurrent requests | 256, then HTTP 503 | `RAJESHDB_MAX_INFLIGHT` |
| Concurrent password hashes | 4, then HTTP 503 | `RAJESHDB_MAX_CONCURRENT_HASHES` |
| Request timeout | 30 s, then HTTP 504 (storage work is not cancelled) | `RAJESHDB_REQUEST_TIMEOUT_SECS` |
| Key rotation grace period | up to 7 days | - |
| Automatic backups | every 6 hours, newest 7 kept per database | `RAJESHDB_BACKUP_INTERVAL_SECS`, `RAJESHDB_BACKUP_RETENTION` |
| Audit events kept | 365 days | `RAJESHDB_AUDIT_RETENTION_DAYS` |
| Operator token | at least 24 characters | `RAJESHDB_OPERATOR_TOKEN` |

## What decides how far one server goes
Disk size, RAM (keys of open databases), CPU for password hashing and queries, and the single writer per database. There is no automatic scaling.
For cost estimates per usage level see `docs/DEPLOYMENT.md` section 13 (prices change; verify with the provider).
