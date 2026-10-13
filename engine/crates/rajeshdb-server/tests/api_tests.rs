use axum::{body::Body, http::Request, Router};
use http_body_util::BodyExt;
use rajeshdb_docs::{DocDb, Options};
use rajeshdb_server::keys::{hash_key, Principal, Role};
use rajeshdb_server::{build_app, AppState, Config};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

const ADMIN: &str = "rdb_test_admin_key";
const WRITER: &str = "rdb_test_writer_key";
const READER: &str = "rdb_test_reader_key";

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("rajeshdb-api-test-{}-{}-{}", std::process::id(), n, nanos));
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn make_app(dir: &Path, tweak: impl FnOnce(&mut Config)) -> Router {
    let mut cfg = Config::new(dir);
    cfg.rate_per_sec = 1000.0;
    cfg.rate_burst = 1000.0;
    tweak(&mut cfg);
    let mut keys = HashMap::new();
    for (k, id, role) in [(ADMIN, "k-admin", Role::Admin), (WRITER, "k-writer", Role::Write), (READER, "k-reader", Role::Read)] {
        keys.insert(hash_key(k), Principal { id: id.to_string(), name: id.to_string(), role });
    }
    let opts = Options { memtable_limit_bytes: 64 * 1024, sync_writes: false };
    let db = DocDb::open(dir, opts).unwrap();
    build_app(Arc::new(AppState::new(db, keys, &cfg)))
}

async fn call(app: &Router, method: &str, uri: &str, key: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(k) = key {
        b = b.header("authorization", format!("Bearer {}", k));
    }
    let body = match body {
        Some(v) => Body::from(serde_json::to_vec(&v).unwrap()),
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
    (status, v)
}

async fn raw_body(app: &Router, method: &str, uri: &str, key: &str, body: &str) -> (u16, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {}", key))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn health_needs_no_key_and_unknown_route_is_404() {
    let t = TempDir::new();
    let app = make_app(t.path(), |_| {});
    let (s, v) = call(&app, "GET", "/health", None, None).await;
    assert_eq!(s, 200);
    assert_eq!(v["status"], "ok");
    let (s, v) = call(&app, "GET", "/nope", Some(ADMIN), None).await;
    assert_eq!(s, 404);
    assert_eq!(v["error"]["code"], "not_found");
}

#[tokio::test]
async fn missing_or_wrong_key_is_401() {
    let t = TempDir::new();
    let app = make_app(t.path(), |_| {});
    for key in [None, Some("wrong"), Some(""), Some("rdb_test_admin_ke")] {
        let (s, v) = call(&app, "GET", "/v1/collections", key, None).await;
        assert_eq!(s, 401, "key {:?}", key);
        assert_eq!(v["error"]["code"], "unauthorized");
    }
    // Wrong scheme
    let req = Request::builder()
        .uri("/v1/collections")
        .header("authorization", format!("Basic {}", ADMIN))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status().as_u16(), 401);
}

#[tokio::test]
async fn repeated_bad_keys_get_throttled() {
    let t = TempDir::new();
    let app = make_app(t.path(), |_| {});
    let mut codes = Vec::new();
    for _ in 0..15 {
        codes.push(call(&app, "GET", "/v1/collections", Some("bad"), None).await.0);
    }
    assert_eq!(codes[0], 401);
    assert!(codes.contains(&429), "expected throttling, got {:?}", codes);
    // A valid key is still served (throttle applies to failed logins only).
    assert_eq!(call(&app, "GET", "/v1/collections", Some(ADMIN), None).await.0, 200);
}

#[tokio::test]
async fn roles_are_enforced() {
    let t = TempDir::new();
    let app = make_app(t.path(), |_| {});
    assert_eq!(call(&app, "POST", "/v1/collections", Some(WRITER), Some(json!({"name": "c"}))).await.0, 403);
    assert_eq!(call(&app, "POST", "/v1/collections", Some(READER), Some(json!({"name": "c"}))).await.0, 403);
    assert_eq!(call(&app, "POST", "/v1/collections", Some(ADMIN), Some(json!({"name": "c"}))).await.0, 201);

    assert_eq!(call(&app, "POST", "/v1/collections/c/documents", Some(READER), Some(json!({"a": 1}))).await.0, 403);
    assert_eq!(call(&app, "POST", "/v1/collections/c/documents", Some(WRITER), Some(json!({"_id": "1", "a": 1}))).await.0, 201);
    assert_eq!(call(&app, "POST", "/v1/collections/c/query", Some(READER), Some(json!({}))).await.0, 200);
    assert_eq!(call(&app, "GET", "/v1/collections/c/documents/1", Some(READER), None).await.0, 200);

    assert_eq!(call(&app, "DELETE", "/v1/collections/c/documents/1", Some(READER), None).await.0, 403);
    assert_eq!(call(&app, "POST", "/v1/collections/c/indexes", Some(WRITER), Some(json!({"field": "a"}))).await.0, 403);
    assert_eq!(call(&app, "DELETE", "/v1/collections/c", Some(WRITER), None).await.0, 403);
    assert_eq!(call(&app, "POST", "/v1/admin/flush", Some(WRITER), None).await.0, 403);
    assert_eq!(call(&app, "GET", "/v1/admin/metrics", Some(READER), None).await.0, 403);
}

#[tokio::test]
async fn full_document_lifecycle() {
    let t = TempDir::new();
    let app = make_app(t.path(), |_| {});
    assert_eq!(call(&app, "POST", "/v1/collections", Some(ADMIN), Some(json!({"name": "users"}))).await.0, 201);
    assert_eq!(call(&app, "POST", "/v1/collections/users/indexes", Some(ADMIN), Some(json!({"field": "city"}))).await.0, 201);
    let (s, v) = call(&app, "GET", "/v1/collections/users/indexes", Some(READER), None).await;
    assert_eq!((s, v["indexes"].clone()), (200, json!(["city"])));

    let (s, v) = call(&app, "POST", "/v1/collections/users/documents", Some(WRITER), Some(json!({"_id": "1", "name": "A", "city": "Delhi", "age": 30}))).await;
    assert_eq!((s, v["_id"].clone()), (201, json!("1")));
    let (s, v) = call(&app, "POST", "/v1/collections/users/documents", Some(WRITER), Some(json!({"name": "B", "city": "Pune"}))).await;
    assert_eq!(s, 201);
    assert!(v["_id"].as_str().unwrap().len() > 3);

    let (s, v) = call(&app, "GET", "/v1/collections/users/documents/1", Some(READER), None).await;
    assert_eq!(s, 200);
    assert_eq!(v["name"], "A");

    let (s, v) = call(&app, "POST", "/v1/collections/users/query", Some(READER), Some(json!({"filter": {"city": "Delhi"}}))).await;
    assert_eq!(s, 200);
    assert_eq!(v["count"], 1);
    assert_eq!(v["plan"], "IndexScan(field=city, op=eq)");

    let (s, v) = call(&app, "POST", "/v1/collections/users/update", Some(WRITER), Some(json!({"filter": {"city": "Delhi"}, "set": {"age": 31}}))).await;
    assert_eq!((s, v["modified"].clone()), (200, json!(1)));
    let (_, v) = call(&app, "GET", "/v1/collections/users/documents/1", Some(READER), None).await;
    assert_eq!(v["age"], 31);

    let (s, v) = call(&app, "PUT", "/v1/collections/users/documents/1", Some(WRITER), Some(json!({"name": "A2", "city": "Goa"}))).await;
    assert_eq!((s, v["_id"].clone()), (200, json!("1")));
    let (_, v) = call(&app, "POST", "/v1/collections/users/query", Some(READER), Some(json!({"filter": {"city": "Goa"}}))).await;
    assert_eq!(v["count"], 1);
    let (_, v) = call(&app, "POST", "/v1/collections/users/query", Some(READER), Some(json!({"filter": {"city": "Delhi"}}))).await;
    assert_eq!(v["count"], 0);

    // Mass delete needs explicit confirmation.
    let (s, v) = call(&app, "POST", "/v1/collections/users/delete", Some(WRITER), Some(json!({"filter": {}}))).await;
    assert_eq!(s, 400);
    assert_eq!(v["error"]["code"], "invalid_argument");
    let (s, v) = call(&app, "POST", "/v1/collections/users/delete", Some(WRITER), Some(json!({"filter": {"city": "Goa"}}))).await;
    assert_eq!((s, v["deleted"].clone()), (200, json!(1)));

    assert_eq!(call(&app, "DELETE", "/v1/collections/users/documents/1", Some(WRITER), None).await.0, 404);
    let (s, _) = call(&app, "POST", "/v1/collections/users/delete", Some(WRITER), Some(json!({"filter": {}, "confirm_all": true}))).await;
    assert_eq!(s, 200);
    let (_, v) = call(&app, "POST", "/v1/collections/users/query", Some(READER), None).await; // empty body = match all
    assert_eq!(v["count"], 0);

    assert_eq!(call(&app, "DELETE", "/v1/collections/users", Some(ADMIN), None).await.0, 200);
    assert_eq!(call(&app, "POST", "/v1/collections/users/query", Some(READER), Some(json!({}))).await.0, 404);
}

#[tokio::test]
async fn bad_input_gets_clean_errors() {
    let t = TempDir::new();
    let app = make_app(t.path(), |_| {});
    call(&app, "POST", "/v1/collections", Some(ADMIN), Some(json!({"name": "c"}))).await;
    call(&app, "POST", "/v1/collections", Some(ADMIN), Some(json!({"name": "s", "schema": {"fields": {"n": {"type": "string", "required": true}}}}))).await;

    // malformed JSON / missing body / wrong shape
    let (s, v) = raw_body(&app, "POST", "/v1/collections/c/documents", WRITER, "{not json").await;
    assert_eq!((s, v["error"]["code"].clone()), (400, json!("invalid_argument")));
    assert_eq!(call(&app, "POST", "/v1/collections/c/documents", Some(WRITER), None).await.0, 400);
    assert_eq!(call(&app, "POST", "/v1/collections/c/documents", Some(WRITER), Some(json!([1, 2]))).await.0, 400);
    assert_eq!(call(&app, "POST", "/v1/collections", Some(ADMIN), Some(json!({"nombre": "x"}))).await.0, 400);
    assert_eq!(call(&app, "POST", "/v1/collections", Some(ADMIN), Some(json!({"name": "bad/name"}))).await.0, 400);
    // bad queries
    assert_eq!(call(&app, "POST", "/v1/collections/c/query", Some(READER), Some(json!({"filter": {"$bogus": 1}}))).await.0, 400);
    assert_eq!(call(&app, "POST", "/v1/collections/c/query", Some(READER), Some(json!({"limit": -5}))).await.0, 400);
    assert_eq!(call(&app, "POST", "/v1/collections/c/query", Some(READER), Some(json!({"zzz": 1}))).await.0, 400);
    // status mapping
    assert_eq!(call(&app, "POST", "/v1/collections", Some(ADMIN), Some(json!({"name": "c"}))).await.0, 409);
    assert_eq!(call(&app, "POST", "/v1/collections/c/documents", Some(WRITER), Some(json!({"_id": "d"}))).await.0, 201);
    assert_eq!(call(&app, "POST", "/v1/collections/c/documents", Some(WRITER), Some(json!({"_id": "d"}))).await.0, 409);
    assert_eq!(call(&app, "GET", "/v1/collections/nope/documents/1", Some(READER), None).await.0, 404);
    assert_eq!(call(&app, "GET", "/v1/collections/c/documents/missing", Some(READER), None).await.0, 404);
    let (s, v) = call(&app, "POST", "/v1/collections/s/documents", Some(WRITER), Some(json!({"other": 1}))).await;
    assert_eq!((s, v["error"]["code"].clone()), (422, json!("schema_violation")));
}

#[tokio::test]
async fn oversized_body_is_rejected() {
    let t = TempDir::new();
    let app = make_app(t.path(), |c| c.max_body = 1024);
    call(&app, "POST", "/v1/collections", Some(ADMIN), Some(json!({"name": "c"}))).await;
    let big = json!({"blob": "x".repeat(5000)});
    assert_eq!(call(&app, "POST", "/v1/collections/c/documents", Some(WRITER), Some(big)).await.0, 413);
    assert_eq!(call(&app, "POST", "/v1/collections/c/documents", Some(WRITER), Some(json!({"small": 1}))).await.0, 201);
}

#[tokio::test]
async fn per_key_rate_limit() {
    let t = TempDir::new();
    let app = make_app(t.path(), |c| {
        c.rate_per_sec = 0.0001;
        c.rate_burst = 3.0;
    });
    let mut codes = Vec::new();
    for _ in 0..5 {
        codes.push(call(&app, "GET", "/v1/collections", Some(READER), None).await.0);
    }
    assert_eq!(codes, vec![200, 200, 200, 429, 429]);
    // Another key has its own bucket.
    assert_eq!(call(&app, "GET", "/v1/collections", Some(ADMIN), None).await.0, 200);
    // /health is never rate limited.
    assert_eq!(call(&app, "GET", "/health", None, None).await.0, 200);
}

#[tokio::test]
async fn result_sizes_are_capped_and_paginated() {
    let t = TempDir::new();
    let app = make_app(t.path(), |c| {
        c.default_limit = 3;
        c.max_limit = 4;
    });
    call(&app, "POST", "/v1/collections", Some(ADMIN), Some(json!({"name": "c"}))).await;
    for i in 1..=5 {
        call(&app, "POST", "/v1/collections/c/documents", Some(WRITER), Some(json!({"_id": format!("d{}", i)}))).await;
    }
    let (_, v) = call(&app, "POST", "/v1/collections/c/query", Some(READER), Some(json!({}))).await;
    assert_eq!(v["count"], 3); // default limit
    let (_, v) = call(&app, "POST", "/v1/collections/c/query", Some(READER), Some(json!({"limit": 100}))).await;
    assert_eq!(v["count"], 4); // hard cap
    assert_eq!(v["limit"], 4);
    let (_, v) = call(&app, "POST", "/v1/collections/c/query", Some(READER), Some(json!({"sort": "_id", "skip": 1, "limit": 2}))).await;
    let ids: Vec<&str> = v["documents"].as_array().unwrap().iter().map(|d| d["_id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["d2", "d3"]);
}

#[tokio::test]
async fn metrics_report_real_numbers() {
    let t = TempDir::new();
    let app = make_app(t.path(), |_| {});
    call(&app, "POST", "/v1/collections", Some(ADMIN), Some(json!({"name": "c"}))).await;
    call(&app, "POST", "/v1/collections/c/documents", Some(WRITER), Some(json!({"_id": "1"}))).await;
    call(&app, "GET", "/v1/collections", Some("bad-key"), None).await;
    let (s, v) = call(&app, "GET", "/v1/admin/metrics", Some(ADMIN), None).await;
    assert_eq!(s, 200);
    assert!(v["requests_total"].as_u64().unwrap() >= 3);
    assert_eq!(v["auth_failures"], 1);
    assert!(v["responses_2xx"].as_u64().unwrap() >= 2);
    assert!(v["engine"]["wal_bytes"].as_u64().unwrap() > 0);
    assert!(v["engine"]["memtable_entries"].as_u64().unwrap() >= 2);
    call(&app, "POST", "/v1/admin/flush", Some(ADMIN), None).await;
    let (_, v) = call(&app, "GET", "/v1/admin/metrics", Some(ADMIN), None).await;
    assert_eq!(v["engine"]["sstable_count"], 1);
    assert_eq!(v["engine"]["wal_bytes"], 0);
    assert_eq!(call(&app, "POST", "/v1/admin/compact", Some(ADMIN), None).await.0, 200);
}

#[tokio::test]
async fn data_survives_server_restart() {
    let t = TempDir::new();
    {
        let app = make_app(t.path(), |_| {});
        call(&app, "POST", "/v1/collections", Some(ADMIN), Some(json!({"name": "c"}))).await;
        call(&app, "POST", "/v1/collections/c/indexes", Some(ADMIN), Some(json!({"field": "k"}))).await;
        let (s, _) = call(&app, "POST", "/v1/collections/c/documents", Some(WRITER), Some(json!({"_id": "1", "k": "v"}))).await;
        assert_eq!(s, 201);
    }
    let app = make_app(t.path(), |_| {});
    let (s, v) = call(&app, "POST", "/v1/collections/c/query", Some(READER), Some(json!({"filter": {"k": "v"}}))).await;
    assert_eq!(s, 200);
    assert_eq!(v["count"], 1);
    assert_eq!(v["plan"], "IndexScan(field=k, op=eq)");
}

#[tokio::test]
async fn responses_carry_security_headers_and_no_stack_details() {
    let t = TempDir::new();
    let app = make_app(t.path(), |_| {});
    let req = Request::builder().uri("/health").body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.headers().get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(resp.headers().get("cache-control").unwrap(), "no-store");
}
