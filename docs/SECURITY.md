# Security notes (Phase 9)

**Status: not audited, not penetration-tested, and the Rust code has never been compiled by its author.**
Nothing here is a claim that the system is "secure". It lists what is implemented, where it is tested, and what is missing.

## 1. What we protect and from whom

| Asset | Main threats considered |
|---|---|
| Tenant data (documents, backups) | another tenant, a stolen API key, a stolen laptop/backup, a curious operator |
| Credentials (passwords, sessions, API keys, operator token) | database leak, log leak, brute force, timing attacks |
| Availability | request floods, slow clients, expensive queries, password-hash floods, big uploads, disk filling |

Not considered: a malicious operator with shell access on the server, nation-state attackers, hardware attacks.

## 2. Controls that exist, and the tests that exercise them

| Control | Where | Test |
|---|---|---|
| Passwords: Argon2id, random salt, 10-128 chars | platform `util.rs` | `util::tests::password_hashing`, `registration_and_login_rules` |
| Same error for unknown e-mail and wrong password; dummy hash for equal timing | `handlers.rs` login | `registration_and_login_rules` |
| Session tokens and API keys stored only as SHA-256 of 256-bit random values; plaintext shown once | `handlers.rs`, `state.rs` | `key_roles_scope_revoke_and_rotate` (listing never contains secrets) |
| Operator token compared in constant time; endpoints off unless configured | `ops_mw`, `util::ct_eq` | `operator_endpoints_are_off_without_a_token_and_protected_with_one` |
| Tenant isolation: separate database directory per project environment; every request authorized BEFORE a database is opened; cross-tenant answers are 404 | `authorize_data` | `tenants_cannot_see_each_others_data`, `bad_ids_and_environments_are_plain_404s` |
| Directory names come only from validated 32-hex ids and a fixed environment list; backup ids match a strict pattern; manifest file names must be plain names | `state.rs`, `backup.rs` | `bad_ids...`, `backup_access_control`, `backup::tests` |
| Roles: read < write < admin on data; org owner / admin / member; keys can never manage users, orgs, keys or backups | `handlers.rs` | `roles_inside_an_organization`, `backup_access_control`, `tenants_...` |
| Least privilege on destructive actions: restore/delete backup and delete project = owner only; typed confirmation for restore; `confirm_all` for empty-filter update/delete | `handlers.rs` | `restore_brings_back...`, `full_document_lifecycle` |
| Injection: no SQL; queries are JSON with a whitelist of operators; storage keys are built only from validated collection names / field paths | docs `keys.rs`, `query.rs` | `invalid_queries_are_errors_not_panics`, docs `query::tests` |
| Query cost limits: nesting depth 8, 500 conditions, 1000 values per `$in` | docs `query.rs` | `expensive_filters_are_rejected`, `expensive_filters_are_refused_over_the_api` |
| Request limits: body size, result size caps (default 100 / max 1000), export cap, per-environment storage quota | platform | `oversized_body...`, `result_sizes_are_capped...`, `export_returns_ndjson_with_a_cap`, `storage_quota_blocks_writes...` |
| Rate limiting: per principal, per client IP (register/login), per e-mail, failed logins; real client IP behind the proxy only when `RAJESHDB_TRUST_PROXY` is on | `limiter.rs`, middleware | `login_attempts_are_throttled`, `forwarded_for_is_ignored_unless_the_proxy_is_trusted`, `trusted_proxy_...` |
| Load shedding: max concurrent requests (503), request timeout (504), max concurrent password hashes (503) | `log_mw`, `enter_pw` | `busy_server_sheds_load...`, `slow_requests_time_out_with_504`, `password_hashing_is_concurrency_limited` |
| Slow-client protection at the proxy (header/body/write/idle timeouts) | `deploy/Caddyfile` | none automated (needs a real Caddy) |
| TLS: terminated by Caddy with HSTS; the API itself binds to loopback by default and is never published by the compose file | `deploy/` | manual checklist below |
| Headers: `nosniff`, `no-store` on every API response; CSP, frame-ancestors none, no-referrer from the proxy | `log_mw`, `Caddyfile` | `responses_carry_security_headers...` (Phase 4 server) |
| Logs: JSON lines with method, path, status, timing, principal id. Never bodies, query strings or credentials | `log_mw` | review |
| Audit log of administrative actions with retention (default 365 days) | `state.rs` | `audit_log_records_admin_actions`, `old_audit_events_can_be_pruned` |
| Data export and deletion: NDJSON export per collection; delete documents, collections, projects (including files on disk) | `handlers.rs` | `export_returns_ndjson...`, `deleting_a_project_removes_keys_and_storage` |
| Backups: verified restore, tamper detection by SHA-256 | `backup.rs` | `tampered_backup_is_detected_and_restore_refused` |
| Crash safety: WAL with checksums, atomic table/manifest publication, kill test | core | `crash_kill`, engine tests |
| Container: non-root user, read-only filesystem, no capabilities, no-new-privileges, internal-only network port | `deploy/` | manual checklist |
| Supply chain: `cargo audit` and `npm audit` in CI (non-blocking), lock files to be committed, third-party actions pinned to major versions | `.github/workflows/ci.yml` | CI |

## 3. Known weaknesses and risks (read before exposing real data)

1. **One process serves all tenants.** Isolation is by directories and authorization checks, not by operating-system boundaries. A bug in this code could cross tenants. For high-risk customers run a separate instance.
2. **No encryption at rest.** Data files and backups are clear text. Use full-disk encryption on the server and encrypt off-site copies (rclone crypt, encrypted disks).
3. **Bearer tokens in the browser.** The dashboard keeps the session token in `sessionStorage`; any XSS on the dashboard origin could read it. The dashboard has no third-party scripts, React escapes output, and the proxy sends a strict CSP, but this is a real trade-off. Sessions last 24 h by default.
4. **No MFA, no e-mail verification, no password reset, no account deletion.** Sign-ups can be closed with `RAJESHDB_ALLOW_REGISTRATION=false`.
5. **User enumeration is possible** through registration ("already registered") and "add member by e-mail" ("no registered user"). Login does not leak.
6. **Rate limits are per process and in memory**: they reset on restart and are not shared if you ever run several instances. A distributed attacker with many IPs is limited only by the global in-flight cap.
7. **Timeouts do not cancel storage work.** After a 504 the blocking operation still finishes (and holds its lock).
8. **Memory is not bounded by quotas.** Quotas count bytes on disk; the engine keeps every key of a database's table files in RAM, and full scans/exports build results in memory. A tenant with a huge key count can exhaust RAM. Open project databases are never evicted.
9. **`/ops/*` shares the API port**; it is protected by a token and blocked by the proxy for other IPs, but if someone runs the platform without that proxy rule, only the token stands in the way (min. 24 characters, throttled).
10. **Throttling relies on the client IP.** Without `RAJESHDB_TRUST_PROXY` all clients behind a proxy look like one IP; with it enabled and the API reachable directly, clients could spoof their IP. The compose file avoids both (no published API port).
11. **Audit log is not tamper-evident**; an operator can edit it.
12. **Operator token and organization/project data are visible in the process environment and files** to anyone with root on the server.
13. **No WAF / DDoS protection.** Put a CDN or the provider's protection in front if you expect hostile traffic.
14. **Secrets in backups:** the control-database backup contains password hashes and API-key hashes.

## 4. Manual checks before real users (run them and tick them off)

    # dependencies
    cd engine   && cargo install cargo-audit --locked && cargo audit
    cd ../dashboard && npm audit --audit-level=high
    # tests (all of them, in release mode too, because timing-dependent tests can differ)
    cd ../engine && cargo test --workspace && cargo test --workspace --release
    # from ANOTHER machine, against your public domain
    curl -sI https://DOMAIN/api/health                      # 200, has strict-transport-security
    curl -s  -o /dev/null -w "%{http_code}\n" https://DOMAIN/api/ops/metrics   # 404 (not on your allow-list)
    nmap -Pn -p 22,80,443,8081,5432 SERVER_IP                # only 22/80/443 open; 8081 closed
    curl -sI http://DOMAIN/ | head -3                        # redirects to https
    # cross-tenant check by hand: create two accounts/projects; use account A's key on B's project id: expect 404
    # abort/interruption test, then check health:
    node loadtest/loadtest.mjs --base https://DOMAIN/api --scenario abort --key rdb_... --project ID --env dev --collection load

## 5. Reporting a vulnerability

Before you invite outside users, publish a security contact (an e-mail address you read) and a policy for how reports are handled.
This repository intentionally contains no placeholder address.
