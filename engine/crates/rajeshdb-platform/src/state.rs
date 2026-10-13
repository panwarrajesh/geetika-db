use crate::backup;
use crate::error::PErr;
use crate::limiter::Limiter;
use crate::types::{OrgRole, ENVS};
use crate::util::{hash_password, is_hex_id, now_ms, sha256_hex};
use axum::http::StatusCode;
use rajeshdb_docs::{DocDb, DocError, Options, Query, Value};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: String,
    pub max_body: usize,
    pub rate_per_sec: f64,
    pub rate_burst: f64,
    pub default_limit: usize,
    pub max_limit: usize,
    pub session_ttl_secs: u64,
    /// Storage quota for EACH project environment database, in bytes (all its files on disk).
    pub project_quota_bytes: u64,
    /// Argon2 memory cost in KiB (19456 = 19 MiB, the OWASP minimum recommendation).
    pub argon_m_cost: u32,
    pub allow_registration: bool,
    pub max_projects_per_org: usize,
    pub max_keys_per_project: usize,
    pub max_orgs_per_user: usize,
    /// Register/login attempts per client IP (token bucket).
    pub auth_ip_rate: f64,
    pub auth_ip_burst: f64,
    pub sync_writes: bool,
    /// Automatic backups every N seconds (0 = off). Default 6 hours.
    pub backup_interval_secs: u64,
    /// Backups kept per database (oldest are deleted).
    pub backup_retention: usize,
    /// Alert when data + backups use this many bytes (0 = no budget alerts).
    pub disk_budget_bytes: u64,
    /// Take the client IP from the last X-Forwarded-For entry. ONLY enable when the server is reachable
    /// exclusively through your own reverse proxy (otherwise clients could spoof their IP).
    pub trust_proxy: bool,
    /// A request running longer than this gets HTTP 504 (the storage work itself is not cancelled).
    pub request_timeout_secs: u64,
    /// Requests being handled at the same time; more get HTTP 503 "busy".
    pub max_inflight: usize,
    /// Password hashes (Argon2 uses ~19 MiB each) computed at the same time; more get HTTP 503.
    pub max_concurrent_hashes: usize,
    /// Audit events older than this are deleted by the daily maintenance (0 = keep forever).
    pub audit_retention_days: u64,
    /// Most documents one export returns (the response is marked truncated beyond it).
    pub max_export_docs: usize,
    /// Secret for the /ops/* operator endpoints (min. 24 characters). Unset = endpoints disabled.
    pub operator_token: Option<String>,
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match std::env::var(name) {
        Ok(v) => v.trim().parse::<T>().map_err(|_| format!("{} has an invalid value: '{}'", name, v)),
        Err(_) => Ok(default),
    }
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Config {
        Config {
            data_dir: data_dir.into(),
            bind: "127.0.0.1:8081".into(),
            max_body: 1024 * 1024,
            rate_per_sec: 50.0,
            rate_burst: 100.0,
            default_limit: 100,
            max_limit: 1000,
            session_ttl_secs: 24 * 3600,
            project_quota_bytes: 100 * 1024 * 1024,
            argon_m_cost: 19456,
            allow_registration: true,
            max_projects_per_org: 20,
            max_keys_per_project: 20,
            max_orgs_per_user: 10,
            auth_ip_rate: 1.0,
            auth_ip_burst: 10.0,
            sync_writes: true,
            backup_interval_secs: 6 * 3600,
            backup_retention: 7,
            disk_budget_bytes: 0,
            operator_token: None,
            trust_proxy: false,
            request_timeout_secs: 30,
            max_inflight: 256,
            max_concurrent_hashes: 4,
            audit_retention_days: 365,
            max_export_docs: 50_000,
        }
    }

    pub fn from_env() -> Result<Config, String> {
        let d = Config::new("./rajeshdb-platform-data");
        Ok(Config {
            data_dir: PathBuf::from(env_or("RAJESHDB_DATA_DIR", d.data_dir.to_string_lossy().into_owned())?),
            bind: env_or("RAJESHDB_BIND", d.bind.clone())?,
            max_body: env_or("RAJESHDB_MAX_BODY_BYTES", d.max_body)?,
            rate_per_sec: env_or("RAJESHDB_RATE_PER_SEC", d.rate_per_sec)?,
            rate_burst: env_or("RAJESHDB_RATE_BURST", d.rate_burst)?,
            default_limit: env_or("RAJESHDB_DEFAULT_LIMIT", d.default_limit)?,
            max_limit: env_or("RAJESHDB_MAX_LIMIT", d.max_limit)?,
            session_ttl_secs: env_or("RAJESHDB_SESSION_TTL_SECS", d.session_ttl_secs)?,
            project_quota_bytes: env_or("RAJESHDB_PROJECT_QUOTA_BYTES", d.project_quota_bytes)?,
            argon_m_cost: env_or("RAJESHDB_ARGON_MEMORY_KIB", d.argon_m_cost)?,
            allow_registration: env_or("RAJESHDB_ALLOW_REGISTRATION", d.allow_registration)?,
            max_projects_per_org: env_or("RAJESHDB_MAX_PROJECTS_PER_ORG", d.max_projects_per_org)?,
            max_keys_per_project: env_or("RAJESHDB_MAX_KEYS_PER_PROJECT", d.max_keys_per_project)?,
            max_orgs_per_user: env_or("RAJESHDB_MAX_ORGS_PER_USER", d.max_orgs_per_user)?,
            auth_ip_rate: d.auth_ip_rate,
            auth_ip_burst: d.auth_ip_burst,
            sync_writes: env_or("RAJESHDB_SYNC_WRITES", d.sync_writes)?,
            backup_interval_secs: env_or("RAJESHDB_BACKUP_INTERVAL_SECS", d.backup_interval_secs)?,
            backup_retention: env_or("RAJESHDB_BACKUP_RETENTION", d.backup_retention)?,
            disk_budget_bytes: env_or("RAJESHDB_DISK_BUDGET_BYTES", d.disk_budget_bytes)?,
            trust_proxy: env_or("RAJESHDB_TRUST_PROXY", d.trust_proxy)?,
            request_timeout_secs: env_or("RAJESHDB_REQUEST_TIMEOUT_SECS", d.request_timeout_secs)?,
            max_inflight: env_or("RAJESHDB_MAX_INFLIGHT", d.max_inflight)?,
            max_concurrent_hashes: env_or("RAJESHDB_MAX_CONCURRENT_HASHES", d.max_concurrent_hashes)?,
            audit_retention_days: env_or("RAJESHDB_AUDIT_RETENTION_DAYS", d.audit_retention_days)?,
            max_export_docs: env_or("RAJESHDB_MAX_EXPORT_DOCS", d.max_export_docs)?,
            operator_token: std::env::var("RAJESHDB_OPERATOR_TOKEN").ok().filter(|t| !t.is_empty()),
        })
    }
}

pub fn poisoned() -> PErr {
    PErr::internal("database lock poisoned")
}

fn doc_poisoned() -> DocError {
    DocError::Io(std::io::Error::new(std::io::ErrorKind::Other, "database lock poisoned"))
}

pub struct ReadGuard<'a>(RwLockReadGuard<'a, Option<DocDb>>);
pub struct WriteGuard<'a>(RwLockWriteGuard<'a, Option<DocDb>>);

impl Deref for ReadGuard<'_> {
    type Target = DocDb;
    fn deref(&self) -> &DocDb {
        self.0.as_ref().expect("checked when the guard was created")
    }
}
impl Deref for WriteGuard<'_> {
    type Target = DocDb;
    fn deref(&self) -> &DocDb {
        self.0.as_ref().expect("checked when the guard was created")
    }
}
impl DerefMut for WriteGuard<'_> {
    fn deref_mut(&mut self) -> &mut DocDb {
        self.0.as_mut().expect("checked when the guard was created")
    }
}

fn unavailable() -> DocError {
    DocError::Io(std::io::Error::new(std::io::ErrorKind::Other, "database is unavailable (restore in progress?); retry"))
}

/// One project environment database (its own directory, its own DocDb). The `Option` is `None`
/// only for a moment while a restore swaps the directory (the write lock is held then).
pub struct ProjDb {
    db: RwLock<Option<DocDb>>,
    pub dir: PathBuf,
    pub quota: u64,
}

impl ProjDb {
    pub fn rd(&self) -> Result<ReadGuard<'_>, DocError> {
        let g = self.db.read().map_err(|_| doc_poisoned())?;
        if g.is_none() {
            return Err(unavailable());
        }
        Ok(ReadGuard(g))
    }
    pub fn wr(&self) -> Result<WriteGuard<'_>, DocError> {
        let g = self.db.write().map_err(|_| doc_poisoned())?;
        if g.is_none() {
            return Err(unavailable());
        }
        Ok(WriteGuard(g))
    }
    /// Real bytes on disk used by this database's files.
    pub fn used_bytes(&self) -> u64 {
        match fs::read_dir(&self.dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum(),
            Err(_) => 0,
        }
    }
    pub fn check_quota(&self) -> Result<(), PErr> {
        if self.used_bytes() >= self.quota {
            Err(PErr::new(
                StatusCode::INSUFFICIENT_STORAGE,
                "quota_exceeded",
                format!("storage quota of {} bytes reached for this environment", self.quota),
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
pub struct Metrics {
    pub requests: AtomicU64,
    pub s2xx: AtomicU64,
    pub s4xx: AtomicU64,
    pub s5xx: AtomicU64,
    pub auth_failures: AtomicU64,
    pub rate_limited: AtomicU64,
    pub quota_rejections: AtomicU64,
    pub backups_ok: AtomicU64,
    pub backups_failed: AtomicU64,
    pub restores: AtomicU64,
    pub latency_ms_sum: AtomicU64,
    pub latency_count: AtomicU64,
}

#[derive(Default)]
pub struct BackupStatus {
    pub last_ok_ms: u64,
    pub last_error: Option<String>,
    pub last_error_ms: u64,
}

pub struct AppState {
    pub cfg: Config,
    pub metrics: Metrics,
    pub backup_status: Mutex<BackupStatus>,
    size_cache: Mutex<Option<(Instant, u64, u64)>>,
    pub operator_hash: Option<String>,
    pub inflight: AtomicUsize,
    pub pw_inflight: AtomicUsize,
    pub ctl: RwLock<DocDb>,
    pub(crate) projects: Mutex<HashMap<(String, String), Arc<ProjDb>>>,
    pub limiter: Limiter,
    pub ip_limiter: Limiter,
    pub login_limiter: Limiter,
    pub fail_limiter: Limiter,
    pub dummy_hash: String,
    pub started: Instant,
}

fn db_options(cfg: &Config) -> Options {
    Options { memtable_limit_bytes: 4 * 1024 * 1024, sync_writes: cfg.sync_writes }
}

fn init_control(db: &mut DocDb) -> Result<(), DocError> {
    let existing = db.list_collections()?;
    let specs: [(&str, &[&str]); 7] = [
        ("users", &["email"]),
        ("sessions", &["expires_ms"]),
        ("orgs", &[]),
        ("members", &["user_id", "org_id"]),
        ("projects", &["org_id"]),
        ("apikeys", &["project_id"]),
        ("audit", &["org_id", "ts_ms"]),
    ];
    for (name, indexes) in specs {
        if !existing.iter().any(|c| c == name) {
            db.create_collection(name, None)?;
        }
        let have = db.list_indexes(name)?;
        for f in indexes {
            if !have.iter().any(|h| h == f) {
                db.create_index(name, f)?;
            }
        }
    }
    Ok(())
}

impl AppState {
    pub fn open(cfg: Config) -> Result<Arc<AppState>, String> {
        let ctl_dir = cfg.data_dir.join("control");
        fs::create_dir_all(&ctl_dir).map_err(|e| format!("cannot create {}: {}", ctl_dir.display(), e))?;
        let mut db = DocDb::open(&ctl_dir, db_options(&cfg)).map_err(|e| e.to_string())?;
        init_control(&mut db).map_err(|e| e.to_string())?;
        let dummy_hash = hash_password("dummy password used for constant-time logins", cfg.argon_m_cost)?;
        let operator_hash = match &cfg.operator_token {
            Some(t) if t.chars().count() < 24 => return Err("RAJESHDB_OPERATOR_TOKEN must be at least 24 characters".into()),
            Some(t) => Some(sha256_hex(t)),
            None => None,
        };
        Ok(Arc::new(AppState {
            metrics: Metrics::default(),
            backup_status: Mutex::new(BackupStatus::default()),
            size_cache: Mutex::new(None),
            operator_hash,
            inflight: AtomicUsize::new(0),
            pw_inflight: AtomicUsize::new(0),
            ctl: RwLock::new(db),
            projects: Mutex::new(HashMap::new()),
            limiter: Limiter::new(cfg.rate_per_sec, cfg.rate_burst),
            ip_limiter: Limiter::new(cfg.auth_ip_rate, cfg.auth_ip_burst),
            // 1 attempt per 5 s sustained, burst 10, per e-mail address
            login_limiter: Limiter::new(0.2, 10.0),
            fail_limiter: Limiter::new(1.0, 10.0),
            dummy_hash,
            started: Instant::now(),
            cfg,
        }))
    }

    /// Open (or reuse) the database of one project environment. `pid` and `env` must already
    /// be validated by the caller; the directory is derived ONLY from them.
    pub fn open_project(&self, pid: &str, env: &str) -> Result<Arc<ProjDb>, PErr> {
        if !is_hex_id(pid, 32) || !ENVS.contains(&env) {
            return Err(PErr::not_found("project or environment not found"));
        }
        let mut map = self.projects.lock().map_err(|_| poisoned())?;
        let key = (pid.to_string(), env.to_string());
        if let Some(p) = map.get(&key) {
            return Ok(p.clone());
        }
        let dir = self.cfg.data_dir.join("projects").join(pid).join(env);
        fs::create_dir_all(&dir).map_err(|e| {
            println!("{}", json!({"level": "error", "msg": "cannot create project dir", "detail": e.to_string()}));
            PErr::internal("cannot create project storage")
        })?;
        let db = DocDb::open(&dir, db_options(&self.cfg))?;
        let p = Arc::new(ProjDb { db: RwLock::new(Some(db)), dir, quota: self.cfg.project_quota_bytes });
        map.insert(key, p.clone());
        Ok(p)
    }

    /// Forget open handles and delete the project's directory tree.
    pub fn remove_project_storage(&self, pid: &str) -> Result<(), PErr> {
        if !is_hex_id(pid, 32) {
            return Err(PErr::not_found("project not found"));
        }
        {
            let mut map = self.projects.lock().map_err(|_| poisoned())?;
            map.retain(|(p, _), _| p != pid);
        }
        let dir = self.cfg.data_dir.join("projects").join(pid);
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| {
                println!("{}", json!({"level": "error", "msg": "project storage cleanup failed", "detail": e.to_string()}));
                PErr::internal("project was removed but its storage cleanup failed; retry later")
            })?;
        }
        Ok(())
    }

    /// Flush every open database (graceful shutdown).
    pub fn flush_all(&self) {
        if let Ok(mut c) = self.ctl.write() {
            let _ = c.flush();
        }
        if let Ok(map) = self.projects.lock() {
            for p in map.values() {
                if let Ok(mut d) = p.wr() {
                    let _ = d.flush();
                }
            }
        }
    }
}

// ---------------------------------------------------------------- control-store helpers

pub fn find_all(db: &DocDb, coll: &str, filter: Value) -> Result<Vec<Value>, DocError> {
    db.find(coll, &Query::new(filter))
}

pub fn find_one(db: &DocDb, coll: &str, filter: Value) -> Result<Option<Value>, DocError> {
    let mut q = Query::new(filter);
    q.limit = Some(1);
    Ok(db.find(coll, &q)?.into_iter().next())
}

pub fn audit(db: &mut DocDb, org_id: &str, actor: &str, action: &str, target: &str, detail: &str) -> Result<(), DocError> {
    db.insert(
        "audit",
        json!({"ts_ms": now_ms(), "org_id": org_id, "actor": actor, "action": action, "target": target, "detail": detail}),
    )?;
    Ok(())
}

pub fn org_role(db: &DocDb, org: &str, user: &str) -> Result<Option<OrgRole>, PErr> {
    if !is_hex_id(org, 32) {
        return Ok(None);
    }
    Ok(db
        .get("members", &format!("{}:{}", org, user))?
        .and_then(|d| d.get("role").and_then(|r| r.as_str()).and_then(OrgRole::parse)))
}

/// The caller must belong to the organization with at least `min`. Non-members get 404, so the
/// existence of other organizations is not revealed.
pub fn need_org(db: &DocDb, org: &str, user: &str, min: OrgRole) -> Result<OrgRole, PErr> {
    match org_role(db, org, user)? {
        None => Err(PErr::not_found("organization not found")),
        Some(r) if r >= min => Ok(r),
        Some(_) => Err(PErr::forbidden()),
    }
}

pub fn get_project(db: &DocDb, pid: &str) -> Result<Value, PErr> {
    if !is_hex_id(pid, 32) {
        return Err(PErr::not_found("project not found"));
    }
    db.get("projects", pid)?.ok_or_else(|| PErr::not_found("project not found"))
}

pub fn path_exists(p: &Path) -> bool {
    p.exists()
}

// ---------------------------------------------------------------- backup, restore, monitoring

fn sibling(dir: &Path, suffix: &str) -> PathBuf {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dir.with_file_name(format!("{}.{}", name, suffix))
}

fn io_internal(ctx: &'static str) -> impl Fn(std::io::Error) -> PErr {
    move |e| {
        println!("{}", json!({"level": "error", "msg": ctx, "detail": e.to_string()}));
        PErr::internal(ctx)
    }
}

impl AppState {
    pub fn backups_root(&self) -> PathBuf {
        self.cfg.data_dir.join("backups")
    }
    pub fn env_backup_root(&self, pid: &str, env: &str) -> PathBuf {
        self.backups_root().join("projects").join(pid).join(env)
    }
    pub fn control_backup_root(&self) -> PathBuf {
        self.backups_root().join("control")
    }
    fn scratch_dir(&self, tag: &str) -> PathBuf {
        self.cfg.data_dir.join("tmp").join(tag)
    }

    fn check_env_ids(pid: &str, env: &str) -> Result<(), PErr> {
        if !is_hex_id(pid, 32) || !ENVS.contains(&env) {
            return Err(PErr::not_found("project or environment not found"));
        }
        Ok(())
    }

    fn backup_failed(&self, what: &str, detail: String) -> PErr {
        println!("{}", json!({"level": "error", "msg": "backup failed", "target": what, "detail": detail}));
        self.metrics.backups_failed.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut s) = self.backup_status.lock() {
            s.last_error = Some(format!("{}: {}", what, detail));
            s.last_error_ms = now_ms();
        }
        PErr::internal("backup failed; see the server log")
    }

    fn backup_succeeded(&self) {
        self.metrics.backups_ok.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut s) = self.backup_status.lock() {
            s.last_ok_ms = now_ms();
        }
    }

    fn backup_env_inner(&self, pid: &str, env: &str, prune: bool) -> Result<Value, PErr> {
        Self::check_env_ids(pid, env)?;
        let pdb = self.open_project(pid, env)?;
        let root = self.env_backup_root(pid, env);
        backup::cleanup_partials(&root);
        let created = {
            let mut g = pdb.wr()?;
            backup::create_backup(&root, &mut *g, "env")
        };
        let meta = created.map_err(|e| self.backup_failed(&format!("{}/{}", pid, env), e))?;
        if prune {
            backup::prune(&root, self.cfg.backup_retention);
        }
        self.backup_succeeded();
        Ok(meta)
    }

    pub fn backup_env(&self, pid: &str, env: &str) -> Result<Value, PErr> {
        self.backup_env_inner(pid, env, true)
    }

    pub fn backup_control(&self) -> Result<Value, PErr> {
        let root = self.control_backup_root();
        backup::cleanup_partials(&root);
        let created = {
            let mut g = self.ctl.write().map_err(|_| poisoned())?;
            backup::create_backup(&root, &mut *g, "control")
        };
        let meta = created.map_err(|e| self.backup_failed("control", e))?;
        backup::prune(&root, self.cfg.backup_retention);
        self.backup_succeeded();
        Ok(meta)
    }

    /// Back up the control database and every project environment that exists on disk.
    /// Returns (number of successful backups, names of failed ones).
    pub fn backup_all(&self) -> (usize, Vec<String>) {
        let mut ok = 0usize;
        let mut failed: Vec<String> = Vec::new();
        if self.cfg.audit_retention_days > 0 {
            let cutoff = now_ms().saturating_sub(self.cfg.audit_retention_days.saturating_mul(86_400_000));
            if self.prune_audit_before(cutoff).is_err() {
                println!("{}", json!({"level": "warn", "msg": "audit log pruning failed"}));
            }
        }
        match self.backup_control() {
            Ok(_) => ok += 1,
            Err(_) => failed.push("control".to_string()),
        }
        let projects = match self.ctl.read() {
            Ok(c) => find_all(&c, "projects", json!({})).unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        for p in projects {
            let pid = p["_id"].as_str().unwrap_or("").to_string();
            for env in ENVS {
                if !self.cfg.data_dir.join("projects").join(&pid).join(env).is_dir() {
                    continue; // environment never used: nothing to back up
                }
                match self.backup_env(&pid, env) {
                    Ok(_) => ok += 1,
                    Err(_) => failed.push(format!("{}/{}", pid, env)),
                }
            }
        }
        (ok, failed)
    }

    pub fn list_env_backups(&self, pid: &str, env: &str) -> Result<Vec<Value>, PErr> {
        Self::check_env_ids(pid, env)?;
        Ok(backup::list_backups(&self.env_backup_root(pid, env)))
    }

    fn env_backup_dir(&self, pid: &str, env: &str, bid: &str) -> Result<PathBuf, PErr> {
        Self::check_env_ids(pid, env)?;
        if !backup::valid_backup_id(bid) {
            return Err(PErr::not_found("backup not found"));
        }
        let dir = self.env_backup_root(pid, env).join(bid);
        if !dir.is_dir() {
            return Err(PErr::not_found("backup not found"));
        }
        Ok(dir)
    }

    /// A failed verification is a normal result ({"ok": false, ...}), not an HTTP error.
    pub fn verify_env_backup(&self, pid: &str, env: &str, bid: &str) -> Result<Value, PErr> {
        let dir = self.env_backup_dir(pid, env, bid)?;
        let scratch = self.scratch_dir(&format!("verify-{}", bid));
        Ok(match backup::verify_backup(&dir, &scratch, &db_options(&self.cfg)) {
            Ok(report) => report,
            Err(msg) => json!({"ok": false, "id": bid, "error": msg}),
        })
    }

    pub fn delete_env_backup(&self, pid: &str, env: &str, bid: &str) -> Result<(), PErr> {
        let dir = self.env_backup_dir(pid, env, bid)?;
        fs::remove_dir_all(dir).map_err(io_internal("could not delete backup"))
    }

    /// Replace the live environment with the contents of a backup.
    /// 1. the backup is verified completely (checksums + opened + every record read);
    /// 2. the current state is saved as a new "pre-restore" backup (not subject to retention);
    /// 3. under the exclusive lock the verified files are staged, the old directory is moved
    ///    aside, the staged one is moved into place and opened; on any failure the old
    ///    directory is put back. Requests in flight during the swap wait for the lock.
    pub fn restore_env(&self, pid: &str, env: &str, bid: &str) -> Result<Value, PErr> {
        let src = self.env_backup_dir(pid, env, bid)?;
        let opts = db_options(&self.cfg);
        let scratch = self.scratch_dir(&format!("restore-check-{}", bid));
        if let Err(msg) = backup::verify_backup(&src, &scratch, &opts) {
            return Err(PErr::bad(format!("backup failed verification, restore refused: {}", msg)));
        }
        let meta = backup::read_meta(&src).map_err(|e| PErr::bad(e))?;
        let safety = self.backup_env_inner(pid, env, false)?;

        let pdb = self.open_project(pid, env)?;
        let mut g = pdb.db.write().map_err(|_| poisoned())?;
        let live = pdb.dir.clone();
        let staged = sibling(&live, "restore-tmp");
        let _ = fs::remove_dir_all(&staged);
        backup::copy_backup_files(&src, &meta, &staged).map_err(|e| {
            let _ = fs::remove_dir_all(&staged);
            PErr::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("could not stage backup: {}", e))
        })?;
        {
            // Prove the staged copy opens and is intact before touching the live data.
            let check = DocDb::open(&staged, opts.clone());
            let ok = match &check {
                Ok(db) => db.verify_integrity().is_ok(),
                Err(_) => false,
            };
            drop(check);
            if !ok {
                let _ = fs::remove_dir_all(&staged);
                return Err(PErr::bad("staged backup could not be opened; restore refused"));
            }
        }
        drop(g.take()); // closes the live database (and its files)
        let aside = sibling(&live, &format!("old-{}", now_ms()));
        let swapped = fs::rename(&live, &aside).and_then(|_| fs::rename(&staged, &live));
        let reopened = match swapped {
            Ok(()) => DocDb::open(&live, opts.clone()).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        match reopened {
            Ok(db) => {
                *g = Some(db);
                let _ = fs::remove_dir_all(&aside);
                self.metrics.restores.fetch_add(1, Ordering::Relaxed);
                Ok(json!({"restored": bid, "entries": meta["entries"], "safety_backup": safety["id"]}))
            }
            Err(detail) => {
                // Roll back: put the old directory back and reopen it.
                println!("{}", json!({"level": "error", "msg": "restore failed, rolling back", "detail": detail}));
                let _ = fs::remove_dir_all(&live);
                if aside.exists() {
                    let _ = fs::rename(&aside, &live);
                }
                let _ = fs::remove_dir_all(&staged);
                if let Ok(db) = DocDb::open(&live, opts) {
                    *g = Some(db);
                }
                Err(PErr::internal("restore failed; the previous data was put back"))
            }
        }
    }

    // ------------------------------------------------------------ monitoring

    /// (bytes in the whole data directory, bytes in backups). Cached for 30 s.
    pub fn storage_sizes(&self) -> (u64, u64) {
        if let Ok(mut c) = self.size_cache.lock() {
            if let Some((at, data, bk)) = *c {
                if at.elapsed().as_secs() < 30 {
                    return (data, bk);
                }
            }
            let data = backup::dir_size(&self.cfg.data_dir);
            let bk = backup::dir_size(&self.backups_root());
            *c = Some((Instant::now(), data, bk));
            return (data, bk);
        }
        (0, 0)
    }

    pub fn alerts(&self, data_bytes: u64) -> Vec<Value> {
        let mut out = Vec::new();
        let budget = self.cfg.disk_budget_bytes;
        if budget > 0 {
            let pct = (data_bytes as f64 / budget as f64) * 100.0;
            if pct >= 95.0 {
                out.push(json!({"level": "critical", "code": "disk_budget", "message": format!("storage is at {:.0}% of the configured budget", pct)}));
            } else if pct >= 80.0 {
                out.push(json!({"level": "warning", "code": "disk_budget", "message": format!("storage is at {:.0}% of the configured budget", pct)}));
            }
        }
        let interval = self.cfg.backup_interval_secs;
        if let Ok(s) = self.backup_status.lock() {
            if interval > 0 {
                let now = now_ms();
                let overdue_ms = interval.saturating_mul(2000);
                let age_ok = s.last_ok_ms > 0 && now.saturating_sub(s.last_ok_ms) <= overdue_ms;
                let young = (self.started.elapsed().as_millis() as u64) <= overdue_ms;
                if !age_ok && !young {
                    out.push(json!({"level": "critical", "code": "backup_overdue", "message": "no successful automatic backup within twice the configured interval"}));
                }
            }
            if let Some(e) = &s.last_error {
                if s.last_error_ms >= s.last_ok_ms {
                    out.push(json!({"level": "warning", "code": "backup_failed", "message": format!("the latest backup attempt failed: {}", e)}));
                }
            }
        }
        out
    }

    pub fn metrics_json(&self) -> Value {
        let (data, bk) = self.storage_sizes();
        let m = &self.metrics;
        let l = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let (sum, cnt) = (l(&m.latency_ms_sum), l(&m.latency_count));
        let (last_ok, last_err) = match self.backup_status.lock() {
            Ok(s) => (s.last_ok_ms, s.last_error.clone()),
            Err(_) => (0, None),
        };
        let open = self.projects.lock().map(|p| p.len()).unwrap_or(0);
        json!({
            "uptime_seconds": self.started.elapsed().as_secs(),
            "requests_total": l(&m.requests),
            "responses_2xx": l(&m.s2xx),
            "responses_4xx": l(&m.s4xx),
            "responses_5xx": l(&m.s5xx),
            "auth_failures": l(&m.auth_failures),
            "rate_limited": l(&m.rate_limited),
            "quota_rejections": l(&m.quota_rejections),
            "latency_ms_avg": if cnt > 0 { sum as f64 / cnt as f64 } else { 0.0 },
            "open_project_databases": open,
            "storage": {"data_dir_bytes": data, "backups_bytes": bk, "budget_bytes": self.cfg.disk_budget_bytes},
            "backups": {
                "ok_total": l(&m.backups_ok), "failed_total": l(&m.backups_failed), "restores_total": l(&m.restores),
                "last_success_ms": last_ok, "last_error": last_err,
                "interval_seconds": self.cfg.backup_interval_secs, "retention": self.cfg.backup_retention
            },
            "alerts": self.alerts(data),
        })
    }

    /// "ok" | "degraded" | "critical", from a real probe of the control database plus the alerts.
    pub fn deep_health(&self) -> Value {
        let probe = match self.ctl.read() {
            Ok(c) => c.list_collections().map(|v| v.len()).map_err(|e| e.to_string()),
            Err(_) => Err("lock poisoned".to_string()),
        };
        let (data, _) = self.storage_sizes();
        let alerts = self.alerts(data);
        let critical = probe.is_err() || alerts.iter().any(|a| a["level"] == "critical");
        let status = if critical { "critical" } else if alerts.is_empty() { "ok" } else { "degraded" };
        json!({"status": status, "control_database": match &probe { Ok(_) => "ok".to_string(), Err(e) => e.clone() }, "alerts": alerts})
    }

    /// Prometheus text exposition format.
    pub fn prometheus(&self) -> String {
        let v = self.metrics_json();
        let n = |p: &str| v.pointer(p).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let rows: Vec<(&str, &str, &str, f64)> = vec![
            ("rajeshdb_uptime_seconds", "gauge", "Seconds since the server started", n("/uptime_seconds")),
            ("rajeshdb_requests_total", "counter", "HTTP requests handled", n("/requests_total")),
            ("rajeshdb_responses_2xx_total", "counter", "Responses with status 2xx", n("/responses_2xx")),
            ("rajeshdb_responses_4xx_total", "counter", "Responses with status 4xx", n("/responses_4xx")),
            ("rajeshdb_responses_5xx_total", "counter", "Responses with status 5xx", n("/responses_5xx")),
            ("rajeshdb_auth_failures_total", "counter", "Failed authentications", n("/auth_failures")),
            ("rajeshdb_rate_limited_total", "counter", "Requests rejected by rate limiting", n("/rate_limited")),
            ("rajeshdb_quota_rejections_total", "counter", "Writes rejected because a quota was reached", n("/quota_rejections")),
            ("rajeshdb_request_latency_ms_avg", "gauge", "Average request latency in ms since start", n("/latency_ms_avg")),
            ("rajeshdb_open_project_databases", "gauge", "Project databases currently open", n("/open_project_databases")),
            ("rajeshdb_data_dir_bytes", "gauge", "Bytes used by the data directory", n("/storage/data_dir_bytes")),
            ("rajeshdb_backups_bytes", "gauge", "Bytes used by backups", n("/storage/backups_bytes")),
            ("rajeshdb_backups_ok_total", "counter", "Successful backups", n("/backups/ok_total")),
            ("rajeshdb_backups_failed_total", "counter", "Failed backups", n("/backups/failed_total")),
            ("rajeshdb_restores_total", "counter", "Completed restores", n("/backups/restores_total")),
            ("rajeshdb_last_backup_success_timestamp_ms", "gauge", "Unix time (ms) of the last successful backup", n("/backups/last_success_ms")),
            ("rajeshdb_alerts_active", "gauge", "Number of active alerts", v["alerts"].as_array().map_or(0.0, |a| a.len() as f64)),
        ];
        let mut out = String::new();
        for (name, kind, help, val) in rows {
            out.push_str(&format!("# HELP {} {}\n# TYPE {} {}\n{} {}\n", name, help, name, kind, name, val));
        }
        out
    }
}


// ---------------------------------------------------------------- hardening helpers

/// Held while one password hash/verification runs; limits how many run at once.
pub struct PwGuard(Arc<AppState>);

impl Drop for PwGuard {
    fn drop(&mut self) {
        self.0.pw_inflight.fetch_sub(1, Ordering::Relaxed);
    }
}

impl AppState {
    /// None when `max_concurrent_hashes` password operations are already running.
    pub fn enter_pw(self: &Arc<Self>) -> Option<PwGuard> {
        let n = self.pw_inflight.fetch_add(1, Ordering::Relaxed) + 1;
        if n > self.cfg.max_concurrent_hashes {
            self.pw_inflight.fetch_sub(1, Ordering::Relaxed);
            None
        } else {
            Some(PwGuard(self.clone()))
        }
    }

    /// Delete audit events older than `cutoff_ms`; returns how many were removed.
    pub fn prune_audit_before(&self, cutoff_ms: u64) -> Result<u64, PErr> {
        let mut c = self.ctl.write().map_err(|_| poisoned())?;
        Ok(c.delete_where("audit", &json!({"ts_ms": {"$lt": cutoff_ms}}))?)
    }
}
