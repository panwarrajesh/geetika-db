//! RajeshDB HTTP API (Phase 4).
//!
//! Security model of this phase:
//!  * every /v1 route needs `Authorization: Bearer <api key>`; keys have a role (read/write/admin)
//!  * only SHA-256 hashes of keys are stored; failed logins are throttled per client IP
//!  * per-key token-bucket rate limit, request body size limit, capped result sizes
//!  * errors never leak internal details; logs are one JSON object per line and never contain
//!    request bodies or credentials
//!  * TLS is NOT done here: run behind a TLS-terminating reverse proxy (Phase 8) and keep the
//!    default bind address 127.0.0.1
//! Multi-tenant isolation (separate databases per project) arrives in Phase 5; in this phase
//! all keys see the same single database.

pub mod keys;
pub mod limiter;

use axum::{
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Extension, Json, Router,
};
use keys::{hash_key, Principal, Role};
use limiter::Limiter;
use rajeshdb_docs::{DocDb, DocError, Query, Value};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------- config

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: String,
    pub keys_file: PathBuf,
    pub max_body: usize,
    pub rate_per_sec: f64,
    pub rate_burst: f64,
    pub default_limit: usize,
    pub max_limit: usize,
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
            bind: "127.0.0.1:8080".into(),
            keys_file: PathBuf::from("rajeshdb-keys.json"),
            max_body: 1024 * 1024,
            rate_per_sec: 50.0,
            rate_burst: 100.0,
            default_limit: 100,
            max_limit: 1000,
        }
    }

    /// RAJESHDB_DATA_DIR, RAJESHDB_BIND, RAJESHDB_KEYS_FILE, RAJESHDB_MAX_BODY_BYTES,
    /// RAJESHDB_RATE_PER_SEC, RAJESHDB_RATE_BURST, RAJESHDB_DEFAULT_LIMIT, RAJESHDB_MAX_LIMIT
    pub fn from_env() -> Result<Config, String> {
        let d = Config::new("./rajeshdb-server-data");
        Ok(Config {
            data_dir: PathBuf::from(env_or("RAJESHDB_DATA_DIR", d.data_dir.to_string_lossy().into_owned())?),
            bind: env_or("RAJESHDB_BIND", d.bind.clone())?,
            keys_file: PathBuf::from(env_or("RAJESHDB_KEYS_FILE", d.keys_file.to_string_lossy().into_owned())?),
            max_body: env_or("RAJESHDB_MAX_BODY_BYTES", d.max_body)?,
            rate_per_sec: env_or("RAJESHDB_RATE_PER_SEC", d.rate_per_sec)?,
            rate_burst: env_or("RAJESHDB_RATE_BURST", d.rate_burst)?,
            default_limit: env_or("RAJESHDB_DEFAULT_LIMIT", d.default_limit)?,
            max_limit: env_or("RAJESHDB_MAX_LIMIT", d.max_limit)?,
        })
    }
}

// ---------------------------------------------------------------- state

#[derive(Default)]
pub struct Metrics {
    pub requests: AtomicU64,
    pub s2xx: AtomicU64,
    pub s4xx: AtomicU64,
    pub s5xx: AtomicU64,
    pub auth_failures: AtomicU64,
    pub rate_limited: AtomicU64,
}

pub struct AppState {
    db: RwLock<DocDb>,
    keys: HashMap<String, Principal>,
    limiter: Limiter,
    fail_limiter: Limiter,
    metrics: Metrics,
    started: Instant,
    max_body: usize,
    default_limit: usize,
    max_limit: usize,
}

impl AppState {
    pub fn new(db: DocDb, keys: HashMap<String, Principal>, cfg: &Config) -> AppState {
        AppState {
            db: RwLock::new(db),
            keys,
            limiter: Limiter::new(cfg.rate_per_sec, cfg.rate_burst),
            // Failed logins: 1 per second sustained, bursts of 10, per client IP.
            fail_limiter: Limiter::new(1.0, 10.0),
            metrics: Metrics::default(),
            started: Instant::now(),
            max_body: cfg.max_body,
            default_limit: cfg.default_limit.max(1),
            max_limit: cfg.max_limit.max(1),
        }
    }

    fn rd(&self) -> Result<RwLockReadGuard<'_, DocDb>, DocError> {
        self.db.read().map_err(|_| poisoned())
    }

    fn wr(&self) -> Result<RwLockWriteGuard<'_, DocDb>, DocError> {
        self.db.write().map_err(|_| poisoned())
    }

    /// Flush memtable to disk (used on graceful shutdown).
    pub fn flush(&self) -> Result<(), String> {
        self.wr().map_err(|e| e.to_string())?.flush().map_err(|e| e.to_string())
    }
}

fn poisoned() -> DocError {
    DocError::Io(std::io::Error::new(std::io::ErrorKind::Other, "database lock poisoned"))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn log_event(v: Value) {
    println!("{}", v);
}

// ---------------------------------------------------------------- errors

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiError {
        ApiError { status, code, message: message.into() }
    }
    fn bad(msg: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::BAD_REQUEST, "invalid_argument", msg)
    }
    fn forbidden() -> ApiError {
        ApiError::new(StatusCode::FORBIDDEN, "forbidden", "this API key's role does not allow that operation")
    }
    fn internal(msg: &str) -> ApiError {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", msg)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error": {"code": self.code, "message": self.message}}))).into_response()
    }
}

impl From<DocError> for ApiError {
    fn from(e: DocError) -> ApiError {
        match e {
            DocError::InvalidArgument(m) => ApiError::new(StatusCode::BAD_REQUEST, "invalid_argument", m),
            DocError::NotFound(m) => ApiError::new(StatusCode::NOT_FOUND, "not_found", m),
            DocError::AlreadyExists(m) => ApiError::new(StatusCode::CONFLICT, "already_exists", m),
            DocError::SchemaViolation(m) => ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "schema_violation", m),
            DocError::Io(err) => {
                log_event(json!({"level": "error", "msg": "storage error", "detail": err.to_string()}));
                ApiError::internal("internal storage error")
            }
        }
    }
}

impl Principal {
    fn require(&self, min: Role) -> Result<(), ApiError> {
        if self.role >= min {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }
}

// ---------------------------------------------------------------- middleware

#[derive(Clone)]
struct KeyId(String);

async fn auth_mw(State(st): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let principal = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(|t| st.keys.get(&hash_key(t.trim())).cloned());

    match principal {
        None => {
            st.metrics.auth_failures.fetch_add(1, Ordering::Relaxed);
            if !st.fail_limiter.allow(&ip) {
                st.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
                return too_many();
            }
            let mut r = ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid API key").into_response();
            r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            r
        }
        Some(p) => {
            if !st.limiter.allow(&p.id) {
                st.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
                return too_many();
            }
            let id = p.id.clone();
            req.extensions_mut().insert(p);
            let mut resp = next.run(req).await;
            resp.extensions_mut().insert(KeyId(id));
            resp
        }
    }
}

fn too_many() -> Response {
    let mut r = ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "too many requests, slow down").into_response();
    r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    r
}

async fn log_mw(State(st): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().to_string();
    let path = req.uri().path().to_string(); // query string deliberately not logged
    st.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let mut resp = next.run(req).await;
    let status = resp.status().as_u16();
    match status / 100 {
        2 => st.metrics.s2xx.fetch_add(1, Ordering::Relaxed),
        4 => st.metrics.s4xx.fetch_add(1, Ordering::Relaxed),
        5 => st.metrics.s5xx.fetch_add(1, Ordering::Relaxed),
        _ => 0,
    };
    let key_id = resp.extensions().get::<KeyId>().map(|k| k.0.clone());
    log_event(json!({
        "ts_ms": now_ms(),
        "method": method,
        "path": path,
        "status": status,
        "ms": start.elapsed().as_millis() as u64,
        "key_id": key_id,
    }));
    let h = resp.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

// ---------------------------------------------------------------- helpers

/// Run blocking storage work off the async threads.
async fn run<T, F>(st: &Arc<AppState>, f: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&AppState) -> Result<T, DocError> + Send + 'static,
{
    let st = st.clone();
    match tokio::task::spawn_blocking(move || f(&*st)).await {
        Ok(r) => r.map_err(ApiError::from),
        Err(_) => Err(ApiError::internal("worker task failed")),
    }
}

fn parse_json(body: &[u8]) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Err(ApiError::bad("a JSON request body is required"));
    }
    serde_json::from_slice(body).map_err(|e| ApiError::bad(format!("request body is not valid JSON: {}", e)))
}

fn str_field(v: &Value, key: &str) -> Result<String, ApiError> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(String::from)
        .ok_or_else(|| ApiError::bad(format!("'{}' (string) is required", key)))
}

fn filter_of(v: &Value) -> Result<Value, ApiError> {
    let f = v.get("filter").cloned().ok_or_else(|| ApiError::bad("'filter' (object) is required"))?;
    let f_obj = f.as_object().ok_or_else(|| ApiError::bad("'filter' must be an object"))?;
    if f_obj.is_empty() && v.get("confirm_all").and_then(|c| c.as_bool()) != Some(true) {
        return Err(ApiError::bad("an empty filter matches every document; add \"confirm_all\": true to do that on purpose"));
    }
    Ok(f)
}

type St = State<Arc<AppState>>;
type Who = Extension<Principal>;

// ---------------------------------------------------------------- handlers

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such route")
}

async fn list_collections(State(st): St, Extension(p): Who) -> Result<Json<Value>, ApiError> {
    p.require(Role::Read)?;
    let names = run(&st, |s| s.rd()?.list_collections()).await?;
    Ok(Json(json!({"collections": names})))
}

async fn create_collection(State(st): St, Extension(p): Who, body: Bytes) -> Result<(StatusCode, Json<Value>), ApiError> {
    p.require(Role::Admin)?;
    let v = parse_json(&body)?;
    let name = str_field(&v, "name")?;
    let schema = v.get("schema").cloned();
    let n = name.clone();
    run(&st, move |s| s.wr()?.create_collection(&n, schema)).await?;
    Ok((StatusCode::CREATED, Json(json!({"name": name}))))
}

async fn drop_collection(State(st): St, Extension(p): Who, Path(coll): Path<String>) -> Result<Json<Value>, ApiError> {
    p.require(Role::Admin)?;
    run(&st, move |s| s.wr()?.drop_collection(&coll)).await?;
    Ok(Json(json!({"deleted": true})))
}

async fn list_indexes(State(st): St, Extension(p): Who, Path(coll): Path<String>) -> Result<Json<Value>, ApiError> {
    p.require(Role::Read)?;
    let idx = run(&st, move |s| s.rd()?.list_indexes(&coll)).await?;
    Ok(Json(json!({"indexes": idx})))
}

async fn create_index(State(st): St, Extension(p): Who, Path(coll): Path<String>, body: Bytes) -> Result<(StatusCode, Json<Value>), ApiError> {
    p.require(Role::Admin)?;
    let v = parse_json(&body)?;
    let field = str_field(&v, "field")?;
    let f = field.clone();
    run(&st, move |s| s.wr()?.create_index(&coll, &f)).await?;
    Ok((StatusCode::CREATED, Json(json!({"field": field}))))
}

async fn drop_index(State(st): St, Extension(p): Who, Path((coll, field)): Path<(String, String)>) -> Result<Json<Value>, ApiError> {
    p.require(Role::Admin)?;
    run(&st, move |s| s.wr()?.drop_index(&coll, &field)).await?;
    Ok(Json(json!({"deleted": true})))
}

async fn insert_doc(State(st): St, Extension(p): Who, Path(coll): Path<String>, body: Bytes) -> Result<(StatusCode, Json<Value>), ApiError> {
    p.require(Role::Write)?;
    let doc = parse_json(&body)?;
    let id = run(&st, move |s| s.wr()?.insert(&coll, doc)).await?;
    Ok((StatusCode::CREATED, Json(json!({"_id": id}))))
}

async fn get_doc(State(st): St, Extension(p): Who, Path((coll, id)): Path<(String, String)>) -> Result<Json<Value>, ApiError> {
    p.require(Role::Read)?;
    let found = run(&st, move |s| s.rd()?.get(&coll, &id)).await?;
    match found {
        Some(d) => Ok(Json(d)),
        None => Err(ApiError::new(StatusCode::NOT_FOUND, "not_found", "document does not exist")),
    }
}

async fn replace_doc(State(st): St, Extension(p): Who, Path((coll, id)): Path<(String, String)>, body: Bytes) -> Result<Json<Value>, ApiError> {
    p.require(Role::Write)?;
    let doc = parse_json(&body)?;
    let id2 = id.clone();
    run(&st, move |s| s.wr()?.replace(&coll, &id2, doc)).await?;
    Ok(Json(json!({"_id": id})))
}

async fn delete_doc(State(st): St, Extension(p): Who, Path((coll, id)): Path<(String, String)>) -> Result<Json<Value>, ApiError> {
    p.require(Role::Write)?;
    let existed = run(&st, move |s| s.wr()?.delete(&coll, &id)).await?;
    if existed {
        Ok(Json(json!({"deleted": true})))
    } else {
        Err(ApiError::new(StatusCode::NOT_FOUND, "not_found", "document does not exist"))
    }
}

/// Body: {"filter":{..},"sort":"field"|{"field":..,"order":"asc|desc"},"skip":0,"limit":10}
/// The server applies a default limit and a hard maximum limit.
async fn query(State(st): St, Extension(p): Who, Path(coll): Path<String>, body: Bytes) -> Result<Json<Value>, ApiError> {
    p.require(Role::Read)?;
    let v = if body.is_empty() { json!({}) } else { parse_json(&body)? };
    let mut q = Query::from_json(&v)?;
    let limit = q.limit.unwrap_or(st.default_limit).min(st.max_limit);
    q.limit = Some(limit);
    let skip = q.skip;
    let (docs, plan) = run(&st, move |s| {
        let db = s.rd()?;
        let docs = db.find(&coll, &q)?;
        let plan = db.explain(&coll, &q)?;
        Ok((docs, plan))
    })
    .await?;
    let count = docs.len();
    Ok(Json(json!({"documents": docs, "count": count, "skip": skip, "limit": limit, "plan": plan})))
}

/// Body: {"filter":{..},"set":{..}}  (empty filter needs "confirm_all": true)
async fn update_docs(State(st): St, Extension(p): Who, Path(coll): Path<String>, body: Bytes) -> Result<Json<Value>, ApiError> {
    p.require(Role::Write)?;
    let v = parse_json(&body)?;
    let filter = filter_of(&v)?;
    let set = v.get("set").cloned().ok_or_else(|| ApiError::bad("'set' (object) is required"))?;
    let n = run(&st, move |s| s.wr()?.update_where(&coll, &filter, &set)).await?;
    Ok(Json(json!({"modified": n})))
}

/// Body: {"filter":{..}}  (empty filter needs "confirm_all": true)
async fn delete_docs(State(st): St, Extension(p): Who, Path(coll): Path<String>, body: Bytes) -> Result<Json<Value>, ApiError> {
    p.require(Role::Write)?;
    let v = parse_json(&body)?;
    let filter = filter_of(&v)?;
    let n = run(&st, move |s| s.wr()?.delete_where(&coll, &filter)).await?;
    Ok(Json(json!({"deleted": n})))
}

async fn admin_flush(State(st): St, Extension(p): Who) -> Result<Json<Value>, ApiError> {
    p.require(Role::Admin)?;
    run(&st, |s| s.wr()?.flush()).await?;
    Ok(Json(json!({"ok": true})))
}

async fn admin_compact(State(st): St, Extension(p): Who) -> Result<Json<Value>, ApiError> {
    p.require(Role::Admin)?;
    run(&st, |s| s.wr()?.compact()).await?;
    Ok(Json(json!({"ok": true})))
}

async fn metrics(State(st): St, Extension(p): Who) -> Result<Json<Value>, ApiError> {
    p.require(Role::Admin)?;
    let stats = run(&st, |s| Ok(s.rd()?.stats())).await?;
    let m = &st.metrics;
    Ok(Json(json!({
        "uptime_seconds": st.started.elapsed().as_secs(),
        "requests_total": m.requests.load(Ordering::Relaxed),
        "responses_2xx": m.s2xx.load(Ordering::Relaxed),
        "responses_4xx": m.s4xx.load(Ordering::Relaxed),
        "responses_5xx": m.s5xx.load(Ordering::Relaxed),
        "auth_failures": m.auth_failures.load(Ordering::Relaxed),
        "rate_limited": m.rate_limited.load(Ordering::Relaxed),
        "engine": {
            "memtable_entries": stats.memtable_entries,
            "memtable_bytes": stats.memtable_bytes,
            "sstable_count": stats.sstable_count,
            "sstable_keys": stats.sstable_keys,
            "wal_bytes": stats.wal_bytes,
        }
    })))
}

// ---------------------------------------------------------------- router

pub fn build_app(st: Arc<AppState>) -> Router {
    let protected: Router<Arc<AppState>> = Router::new()
        .route("/v1/collections", get(list_collections).post(create_collection))
        .route("/v1/collections/:coll", delete(drop_collection))
        .route("/v1/collections/:coll/indexes", get(list_indexes).post(create_index))
        .route("/v1/collections/:coll/indexes/:field", delete(drop_index))
        .route("/v1/collections/:coll/documents", post(insert_doc))
        .route("/v1/collections/:coll/documents/:id", get(get_doc).put(replace_doc).delete(delete_doc))
        .route("/v1/collections/:coll/query", post(query))
        .route("/v1/collections/:coll/update", post(update_docs))
        .route("/v1/collections/:coll/delete", post(delete_docs))
        .route("/v1/admin/flush", post(admin_flush))
        .route("/v1/admin/compact", post(admin_compact))
        .route("/v1/admin/metrics", get(metrics))
        .route_layer(middleware::from_fn_with_state(st.clone(), auth_mw));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(st.max_body))
        .layer(middleware::from_fn_with_state(st.clone(), log_mw))
        .with_state(st)
}
