use axum::{body::Body, http::Request, Router};
use http_body_util::BodyExt;
use rajeshdb_platform::{build_app, AppState, Config};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

static COUNTER: AtomicU64 = AtomicU64::new(0);
const PW: &str = "correct horse battery";

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("rajeshdb-plat-test-{}-{}-{}", std::process::id(), n, nanos));
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

fn app_with(dir: &Path, tweak: impl FnOnce(&mut Config)) -> Router {
    let mut cfg = Config::new(dir);
    cfg.argon_m_cost = 8; // cheap hashing for tests only
    cfg.sync_writes = false;
    cfg.rate_per_sec = 1000.0;
    cfg.rate_burst = 1000.0;
    cfg.auth_ip_rate = 1000.0;
    cfg.auth_ip_burst = 1000.0;
    tweak(&mut cfg);
    build_app(AppState::open(cfg).unwrap())
}

fn app(dir: &Path) -> Router {
    app_with(dir, |_| {})
}

async fn call(app: &Router, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {}", t));
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

async fn signup(app: &Router, email: &str) -> String {
    let (s, _) = call(app, "POST", "/v1/auth/register", None, Some(json!({"email": email, "password": PW}))).await;
    assert_eq!(s, 201, "register {}", email);
    login(app, email).await
}

async fn login(app: &Router, email: &str) -> String {
    let (s, v) = call(app, "POST", "/v1/auth/login", None, Some(json!({"email": email, "password": PW}))).await;
    assert_eq!(s, 200, "login {}", email);
    v["token"].as_str().unwrap().to_string()
}

async fn mk_org(app: &Router, tok: &str, name: &str) -> String {
    let (s, v) = call(app, "POST", "/v1/orgs", Some(tok), Some(json!({"name": name}))).await;
    assert_eq!(s, 201);
    v["org_id"].as_str().unwrap().to_string()
}

async fn mk_project(app: &Router, tok: &str, org: &str, name: &str) -> String {
    let (s, v) = call(app, "POST", &format!("/v1/orgs/{}/projects", org), Some(tok), Some(json!({"name": name}))).await;
    assert_eq!(s, 201, "{:?}", v);
    v["project_id"].as_str().unwrap().to_string()
}

async fn mk_key(app: &Router, tok: &str, pid: &str, env: &str, role: &str) -> (String, String) {
    let (s, v) = call(app, "POST", &format!("/v1/projects/{}/keys", pid), Some(tok), Some(json!({"name": "k", "env": env, "role": role}))).await;
    assert_eq!(s, 201, "{:?}", v);
    (v["api_key"].as_str().unwrap().to_string(), v["key_id"].as_str().unwrap().to_string())
}

fn dp(pid: &str, env: &str, rest: &str) -> String {
    format!("/v1/projects/{}/envs/{}/{}", pid, env, rest)
}

#[tokio::test]
async fn registration_and_login_rules() {
    let t = TempDir::new();
    let app = app(t.path());
    let ok = json!({"email": "Ann@Example.com", "password": PW});
    assert_eq!(call(&app, "POST", "/v1/auth/register", None, Some(ok.clone())).await.0, 201);
    // duplicate (case-insensitive), weak password, bad e-mail, bad body
    assert_eq!(call(&app, "POST", "/v1/auth/register", None, Some(json!({"email": "ann@example.com", "password": PW}))).await.0, 409);
    assert_eq!(call(&app, "POST", "/v1/auth/register", None, Some(json!({"email": "b@example.com", "password": "short"}))).await.0, 400);
    assert_eq!(call(&app, "POST", "/v1/auth/register", None, Some(json!({"email": "nope", "password": PW}))).await.0, 400);
    assert_eq!(call(&app, "POST", "/v1/auth/register", None, None).await.0, 400);
    // wrong password and unknown e-mail look identical
    let (s1, v1) = call(&app, "POST", "/v1/auth/login", None, Some(json!({"email": "ann@example.com", "password": "wrong password!!"}))).await;
    let (s2, v2) = call(&app, "POST", "/v1/auth/login", None, Some(json!({"email": "who@example.com", "password": PW}))).await;
    assert_eq!((s1, s2), (401, 401));
    assert_eq!(v1, v2);
    let tok = login(&app, "ann@example.com").await;
    assert!(tok.starts_with("rdbs_"));
    let (s, v) = call(&app, "GET", "/v1/me", Some(&tok), None).await;
    assert_eq!(s, 200);
    assert_eq!(v["email"], "ann@example.com");
    assert!(v.get("pw_hash").is_none());
}

#[tokio::test]
async fn protected_routes_need_credentials_and_logout_works() {
    let t = TempDir::new();
    let app = app(t.path());
    assert_eq!(call(&app, "GET", "/health", None, None).await.0, 200);
    assert_eq!(call(&app, "GET", "/v1/orgs", None, None).await.0, 401);
    assert_eq!(call(&app, "GET", "/v1/orgs", Some("garbage"), None).await.0, 401);
    let tok = signup(&app, "a@example.com").await;
    assert_eq!(call(&app, "GET", "/v1/orgs", Some(&tok), None).await.0, 200);
    assert_eq!(call(&app, "POST", "/v1/auth/logout", Some(&tok), None).await.0, 200);
    assert_eq!(call(&app, "GET", "/v1/orgs", Some(&tok), None).await.0, 401);
}

#[tokio::test]
async fn expired_sessions_are_rejected() {
    let t = TempDir::new();
    let app = app_with(t.path(), |c| c.session_ttl_secs = 0);
    let tok = signup(&app, "a@example.com").await;
    assert_eq!(call(&app, "GET", "/v1/me", Some(&tok), None).await.0, 401);
}

#[tokio::test]
async fn login_attempts_are_throttled() {
    let t = TempDir::new();
    let app = app_with(t.path(), |c| {
        c.auth_ip_rate = 0.0001;
        c.auth_ip_burst = 8.0;
    });
    let mut codes = Vec::new();
    for _ in 0..12 {
        codes.push(call(&app, "POST", "/v1/auth/login", None, Some(json!({"email": "x@example.com", "password": "wrong password!!"}))).await.0);
    }
    assert_eq!(codes[0], 401);
    assert!(codes.contains(&429), "{:?}", codes);
}

#[tokio::test]
async fn org_and_project_basics() {
    let t = TempDir::new();
    let app = app(t.path());
    let tok = signup(&app, "owner@example.com").await;
    let org = mk_org(&app, &tok, "Acme Inc").await;
    let (_, v) = call(&app, "GET", "/v1/orgs", Some(&tok), None).await;
    assert_eq!(v["organizations"][0]["role"], "owner");
    let pid = mk_project(&app, &tok, &org, "shop").await;
    assert_eq!(call(&app, "POST", &format!("/v1/orgs/{}/projects", org), Some(&tok), Some(json!({"name": "shop"}))).await.0, 409);
    assert_eq!(call(&app, "POST", &format!("/v1/orgs/{}/projects", org), Some(&tok), Some(json!({"name": "bad name!"}))).await.0, 400);
    let (_, v) = call(&app, "GET", &format!("/v1/orgs/{}/projects", org), Some(&tok), None).await;
    assert_eq!(v["projects"].as_array().unwrap().len(), 1);
    assert_eq!(v["projects"][0]["environments"], json!(["dev", "staging", "prod"]));
    assert_eq!(v["projects"][0]["project_id"], pid.as_str());
}

#[tokio::test]
async fn project_and_key_limits_are_enforced() {
    let t = TempDir::new();
    let app = app_with(t.path(), |c| {
        c.max_projects_per_org = 2;
        c.max_keys_per_project = 2;
        c.max_orgs_per_user = 1;
    });
    let tok = signup(&app, "a@example.com").await;
    let org = mk_org(&app, &tok, "o").await;
    assert_eq!(call(&app, "POST", "/v1/orgs", Some(&tok), Some(json!({"name": "second"}))).await.0, 400);
    let p1 = mk_project(&app, &tok, &org, "p1").await;
    mk_project(&app, &tok, &org, "p2").await;
    assert_eq!(call(&app, "POST", &format!("/v1/orgs/{}/projects", org), Some(&tok), Some(json!({"name": "p3"}))).await.0, 400);
    mk_key(&app, &tok, &p1, "dev", "read").await;
    mk_key(&app, &tok, &p1, "dev", "read").await;
    let (s, _) = call(&app, "POST", &format!("/v1/projects/{}/keys", p1), Some(&tok), Some(json!({"name": "k", "env": "dev", "role": "read"}))).await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn tenants_cannot_see_each_others_data() {
    let t = TempDir::new();
    let app = app(t.path());
    let alice = signup(&app, "alice@example.com").await;
    let bob = signup(&app, "bob@example.com").await;
    let org_a = mk_org(&app, &alice, "A").await;
    let org_b = mk_org(&app, &bob, "B").await;
    let pa = mk_project(&app, &alice, &org_a, "app").await;
    let pb = mk_project(&app, &bob, &org_b, "app").await;
    let (ka, _) = mk_key(&app, &alice, &pa, "dev", "write").await;
    let (kb, _) = mk_key(&app, &bob, &pb, "dev", "write").await;

    // Same collection name in both tenants; each key writes to its own project.
    for (owner, key, pid, who) in [(&alice, &ka, &pa, "alice-data"), (&bob, &kb, &pb, "bob-data")] {
        assert_eq!(call(&app, "POST", &dp(pid, "dev", "collections"), Some(owner), Some(json!({"name": "users"}))).await.0, 201);
        let (s, _) = call(&app, "POST", &dp(pid, "dev", "collections/users/documents"), Some(key), Some(json!({"_id": "1", "owner": who}))).await;
        assert_eq!(s, 201);
    }
    let (_, v) = call(&app, "POST", &dp(&pa, "dev", "collections/users/query"), Some(&ka), Some(json!({}))).await;
    assert_eq!(v["count"], 1);
    assert_eq!(v["documents"][0]["owner"], "alice-data");
    let (_, v) = call(&app, "POST", &dp(&pb, "dev", "collections/users/query"), Some(&kb), Some(json!({}))).await;
    assert_eq!(v["documents"][0]["owner"], "bob-data");

    // Cross-tenant access, all as 404 (existence is not revealed).
    assert_eq!(call(&app, "POST", &dp(&pb, "dev", "collections/users/query"), Some(&ka), Some(json!({}))).await.0, 404);
    assert_eq!(call(&app, "GET", &dp(&pb, "dev", "collections/users/documents/1"), Some(&ka), None).await.0, 404);
    assert_eq!(call(&app, "POST", &dp(&pa, "dev", "collections/users/query"), Some(&bob), Some(json!({}))).await.0, 404);
    assert_eq!(call(&app, "POST", &dp(&pb, "dev", "collections/users/documents"), Some(&alice), Some(json!({"x": 1}))).await.0, 404);
    // A key is also bound to ONE environment of its project.
    assert_eq!(call(&app, "POST", &dp(&pa, "prod", "collections/users/query"), Some(&ka), Some(json!({}))).await.0, 404);
    assert_eq!(call(&app, "POST", &dp(&pa, "staging", "collections/users/query"), Some(&ka), Some(json!({}))).await.0, 404);
    // Management APIs of other tenants are closed too.
    assert_eq!(call(&app, "GET", &format!("/v1/projects/{}/keys", pb), Some(&alice), None).await.0, 404);
    assert_eq!(call(&app, "GET", &format!("/v1/orgs/{}/projects", org_b), Some(&alice), None).await.0, 404);
    assert_eq!(call(&app, "POST", &format!("/v1/orgs/{}/projects", org_b), Some(&alice), Some(json!({"name": "x"}))).await.0, 404);
    assert_eq!(call(&app, "GET", &format!("/v1/orgs/{}/audit", org_b), Some(&alice), None).await.0, 404);
    assert_eq!(call(&app, "DELETE", &format!("/v1/projects/{}", pb), Some(&alice), None).await.0, 404);
    // API keys can never manage anything.
    assert_eq!(call(&app, "GET", "/v1/orgs", Some(&ka), None).await.0, 403);
    assert_eq!(call(&app, "GET", &format!("/v1/projects/{}/keys", pa), Some(&ka), None).await.0, 403);
    // Physically separate storage.
    let da = t.path().join("projects").join(&pa).join("dev");
    let db = t.path().join("projects").join(&pb).join("dev");
    assert!(da.is_dir() && db.is_dir() && da != db);
}

#[tokio::test]
async fn bad_ids_and_environments_are_plain_404s() {
    let t = TempDir::new();
    let app = app(t.path());
    let tok = signup(&app, "a@example.com").await;
    let org = mk_org(&app, &tok, "o").await;
    let pid = mk_project(&app, &tok, &org, "p").await;
    for uri in [
        "/v1/projects/not-hex/envs/dev/collections".to_string(),
        "/v1/projects/..%2F..%2Fetc/envs/dev/collections".to_string(),
        "/v1/projects/0123456789abcdef0123456789abcdef/envs/dev/collections".to_string(),
        format!("/v1/projects/{}/envs/qa/collections", pid),
        format!("/v1/projects/{}/envs/../collections", pid),
    ] {
        assert_eq!(call(&app, "GET", &uri, Some(&tok), None).await.0, 404, "{}", uri);
    }
    assert_eq!(call(&app, "GET", &format!("/v1/orgs/{}/projects", "zz"), Some(&tok), None).await.0, 404);
}

#[tokio::test]
async fn roles_inside_an_organization() {
    let t = TempDir::new();
    let app = app(t.path());
    let owner = signup(&app, "owner@example.com").await;
    let carol = signup(&app, "carol@example.com").await;
    let org = mk_org(&app, &owner, "o").await;
    let pid = mk_project(&app, &owner, &org, "p").await;
    assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&owner), Some(json!({"name": "c"}))).await.0, 201);
    assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections/c/documents"), Some(&owner), Some(json!({"_id": "1"}))).await.0, 201);

    // carol is not a member yet
    assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections/c/query"), Some(&carol), Some(json!({}))).await.0, 404);
    let (s, _) = call(&app, "POST", &format!("/v1/orgs/{}/members", org), Some(&owner), Some(json!({"email": "carol@example.com", "role": "member"}))).await;
    assert_eq!(s, 201);
    assert_eq!(call(&app, "POST", &format!("/v1/orgs/{}/members", org), Some(&owner), Some(json!({"email": "carol@example.com", "role": "member"}))).await.0, 409);
    assert_eq!(call(&app, "POST", &format!("/v1/orgs/{}/members", org), Some(&owner), Some(json!({"email": "ghost@example.com", "role": "member"}))).await.0, 404);
    assert_eq!(call(&app, "POST", &format!("/v1/orgs/{}/members", org), Some(&owner), Some(json!({"email": "carol@example.com", "role": "owner"}))).await.0, 400);

    // member: read-only on data, no management
    assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections/c/query"), Some(&carol), Some(json!({}))).await.0, 200);
    assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections/c/documents"), Some(&carol), Some(json!({"x": 1}))).await.0, 403);
    assert_eq!(call(&app, "POST", &format!("/v1/orgs/{}/projects", org), Some(&carol), Some(json!({"name": "x"}))).await.0, 403);
    assert_eq!(call(&app, "GET", &format!("/v1/projects/{}/keys", pid), Some(&carol), None).await.0, 403);
    assert_eq!(call(&app, "GET", &format!("/v1/orgs/{}/audit", org), Some(&carol), None).await.0, 403);
    assert_eq!(call(&app, "DELETE", &format!("/v1/projects/{}", pid), Some(&carol), None).await.0, 403);
    assert_eq!(call(&app, "GET", &format!("/v1/orgs/{}/members", org), Some(&carol), None).await.0, 200);

    // the owner cannot be removed; removing carol takes her access away
    let (_, me) = call(&app, "GET", "/v1/me", Some(&owner), None).await;
    let owner_id = me["user_id"].as_str().unwrap().to_string();
    assert_eq!(call(&app, "DELETE", &format!("/v1/orgs/{}/members/{}", org, owner_id), Some(&owner), None).await.0, 400);
    let (_, me) = call(&app, "GET", "/v1/me", Some(&carol), None).await;
    let carol_id = me["user_id"].as_str().unwrap().to_string();
    assert_eq!(call(&app, "DELETE", &format!("/v1/orgs/{}/members/{}", org, carol_id), Some(&owner), None).await.0, 200);
    assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections/c/query"), Some(&carol), Some(json!({}))).await.0, 404);
}

#[tokio::test]
async fn key_roles_scope_revoke_and_rotate() {
    let t = TempDir::new();
    let app = app(t.path());
    let tok = signup(&app, "a@example.com").await;
    let org = mk_org(&app, &tok, "o").await;
    let pid = mk_project(&app, &tok, &org, "p").await;
    call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&tok), Some(json!({"name": "c"}))).await;

    let (rk, _) = mk_key(&app, &tok, &pid, "dev", "read").await;
    let (wk, w_id) = mk_key(&app, &tok, &pid, "dev", "write").await;
    let (ak, _) = mk_key(&app, &tok, &pid, "dev", "admin").await;
    let doc = dp(&pid, "dev", "collections/c/documents");
    assert_eq!(call(&app, "POST", &doc, Some(&rk), Some(json!({"a": 1}))).await.0, 403);
    assert_eq!(call(&app, "POST", &doc, Some(&wk), Some(json!({"_id": "1"}))).await.0, 201);
    assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections/c/query"), Some(&rk), Some(json!({}))).await.0, 200);
    assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections/c/indexes"), Some(&wk), Some(json!({"field": "a"}))).await.0, 403);
    assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections/c/indexes"), Some(&ak), Some(json!({"field": "a"}))).await.0, 201);

    // listing never reveals secrets
    let (s, v) = call(&app, "GET", &format!("/v1/projects/{}/keys", pid), Some(&tok), None).await;
    assert_eq!(s, 200);
    assert_eq!(v["keys"].as_array().unwrap().len(), 3);
    let text = v.to_string();
    for k in [&rk, &wk, &ak] {
        assert!(!text.contains(k.as_str()));
    }
    assert!(!text.contains("hash") && !text.contains("api_key"));

    // rotate with no grace: new key works, old key stops immediately
    let (s, v) = call(&app, "POST", &format!("/v1/projects/{}/keys/{}/rotate", pid, w_id), Some(&tok), None).await;
    assert_eq!(s, 201);
    let new_key = v["api_key"].as_str().unwrap().to_string();
    assert_ne!(new_key, wk);
    assert_eq!(call(&app, "POST", &doc, Some(&wk), Some(json!({"x": 1}))).await.0, 401);
    assert_eq!(call(&app, "POST", &doc, Some(&new_key), Some(json!({"_id": "2"}))).await.0, 201);

    // rotate with grace: both work for a while
    let new_id = v["key_id"].as_str().unwrap().to_string();
    let (s, v2) = call(&app, "POST", &format!("/v1/projects/{}/keys/{}/rotate", pid, new_id), Some(&tok), Some(json!({"grace_seconds": 3600}))).await;
    assert_eq!(s, 201);
    let newer = v2["api_key"].as_str().unwrap().to_string();
    assert_eq!(call(&app, "POST", &doc, Some(&new_key), Some(json!({"_id": "3"}))).await.0, 201);
    assert_eq!(call(&app, "POST", &doc, Some(&newer), Some(json!({"_id": "4"}))).await.0, 201);
    assert_eq!(call(&app, "POST", &format!("/v1/projects/{}/keys/{}/rotate", pid, new_id), Some(&tok), Some(json!({"grace_seconds": -1}))).await.0, 400);

    // revoke is immediate (no restart)
    let newer_id = v2["key_id"].as_str().unwrap().to_string();
    assert_eq!(call(&app, "POST", &doc, Some(&newer), Some(json!({"_id": "5"}))).await.0, 201);
    assert_eq!(call(&app, "DELETE", &format!("/v1/projects/{}/keys/{}", pid, newer_id), Some(&tok), None).await.0, 200);
    assert_eq!(call(&app, "POST", &doc, Some(&newer), Some(json!({"_id": "6"}))).await.0, 401);
    assert_eq!(call(&app, "DELETE", &format!("/v1/projects/{}/keys/{}", pid, newer_id), Some(&tok), None).await.0, 404);
}

#[tokio::test]
async fn keys_can_expire() {
    let t = TempDir::new();
    let app = app(t.path());
    let tok = signup(&app, "a@example.com").await;
    let org = mk_org(&app, &tok, "o").await;
    let pid = mk_project(&app, &tok, &org, "p").await;
    let (s, v) = call(&app, "POST", &format!("/v1/projects/{}/keys", pid), Some(&tok), Some(json!({"name": "k", "env": "dev", "role": "read", "expires_in_secs": 3600}))).await;
    assert_eq!(s, 201);
    let k = v["api_key"].as_str().unwrap();
    assert_eq!(call(&app, "GET", &dp(&pid, "dev", "collections"), Some(k), None).await.0, 200);
    let (s, _) = call(&app, "POST", &format!("/v1/projects/{}/keys", pid), Some(&tok), Some(json!({"name": "k", "env": "dev", "role": "read", "expires_in_secs": 0}))).await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn storage_quota_blocks_writes_but_not_deletes() {
    let t = TempDir::new();
    let app = app_with(t.path(), |c| c.project_quota_bytes = 30_000);
    let tok = signup(&app, "a@example.com").await;
    let org = mk_org(&app, &tok, "o").await;
    let pid = mk_project(&app, &tok, &org, "p").await;
    let (wk, _) = mk_key(&app, &tok, &pid, "dev", "write").await;
    assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&tok), Some(json!({"name": "c"}))).await.0, 201);
    let url = dp(&pid, "dev", "collections/c/documents");
    let mut statuses = Vec::new();
    for i in 0..30 {
        statuses.push(call(&app, "POST", &url, Some(&wk), Some(json!({"_id": format!("d{}", i), "blob": "x".repeat(3000)}))).await.0);
    }
    assert_eq!(statuses[0], 201);
    assert!(statuses.contains(&507), "expected quota errors, got {:?}", statuses);
    let (s, u) = call(&app, "GET", &dp(&pid, "dev", "usage"), Some(&tok), None).await;
    assert_eq!(s, 200);
    assert_eq!(u["quota_bytes"], 30_000);
    assert!(u["used_bytes"].as_u64().unwrap() >= 30_000);
    // deleting is still allowed when over quota
    assert_eq!(call(&app, "DELETE", &format!("{}/d0", url), Some(&wk), None).await.0, 200);
    // other environments have their own quota
    assert_eq!(call(&app, "POST", &dp(&pid, "staging", "collections"), Some(&tok), Some(json!({"name": "c"}))).await.0, 201);
}

#[tokio::test]
async fn audit_log_records_admin_actions() {
    let t = TempDir::new();
    let app = app(t.path());
    let tok = signup(&app, "a@example.com").await;
    let org = mk_org(&app, &tok, "o").await;
    let pid = mk_project(&app, &tok, &org, "p").await;
    let (_, kid) = mk_key(&app, &tok, &pid, "dev", "read").await;
    call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&tok), Some(json!({"name": "c"}))).await;
    call(&app, "DELETE", &format!("/v1/projects/{}/keys/{}", pid, kid), Some(&tok), None).await;
    let (s, v) = call(&app, "GET", &format!("/v1/orgs/{}/audit", org), Some(&tok), None).await;
    assert_eq!(s, 200);
    let actions: Vec<String> = v["events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap().to_string()).collect();
    for want in ["org.create", "project.create", "key.create", "collection.create", "key.revoke"] {
        assert!(actions.contains(&want.to_string()), "missing {} in {:?}", want, actions);
    }
    let text = v.to_string();
    assert!(!text.contains("rdb_") && !text.contains("password"), "audit log must not contain secrets");
}

#[tokio::test]
async fn deleting_a_project_removes_keys_and_storage() {
    let t = TempDir::new();
    let app = app(t.path());
    let tok = signup(&app, "a@example.com").await;
    let org = mk_org(&app, &tok, "o").await;
    let pid = mk_project(&app, &tok, &org, "p").await;
    let (k, _) = mk_key(&app, &tok, &pid, "dev", "admin").await;
    assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&k), Some(json!({"name": "c"}))).await.0, 201);
    let dir = t.path().join("projects").join(&pid);
    assert!(dir.is_dir());
    assert_eq!(call(&app, "DELETE", &format!("/v1/projects/{}", pid), Some(&tok), None).await.0, 200);
    assert!(!dir.exists());
    assert_eq!(call(&app, "GET", &dp(&pid, "dev", "collections"), Some(&k), None).await.0, 401);
    assert_eq!(call(&app, "GET", &dp(&pid, "dev", "collections"), Some(&tok), None).await.0, 404);
    let (_, v) = call(&app, "GET", &format!("/v1/orgs/{}/projects", org), Some(&tok), None).await;
    assert!(v["projects"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn everything_survives_a_restart() {
    let t = TempDir::new();
    let pid;
    let key;
    {
        let app = app(t.path());
        let tok = signup(&app, "a@example.com").await;
        let org = mk_org(&app, &tok, "o").await;
        pid = mk_project(&app, &tok, &org, "p").await;
        key = mk_key(&app, &tok, &pid, "dev", "admin").await.0;
        call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&key), Some(json!({"name": "c"}))).await;
        call(&app, "POST", &dp(&pid, "dev", "collections/c/indexes"), Some(&key), Some(json!({"field": "k"}))).await;
        assert_eq!(call(&app, "POST", &dp(&pid, "dev", "collections/c/documents"), Some(&key), Some(json!({"_id": "1", "k": "v"}))).await.0, 201);
    }
    let app = app(t.path());
    let tok = login(&app, "a@example.com").await;
    let (s, v) = call(&app, "POST", &dp(&pid, "dev", "collections/c/query"), Some(&key), Some(json!({"filter": {"k": "v"}}))).await;
    assert_eq!(s, 200);
    assert_eq!(v["count"], 1);
    assert_eq!(v["plan"], "IndexScan(field=k, op=eq)");
    assert_eq!(call(&app, "GET", "/v1/orgs", Some(&tok), None).await.0, 200);
}

#[tokio::test]
async fn registration_can_be_disabled() {
    let t = TempDir::new();
    let app = app_with(t.path(), |c| c.allow_registration = false);
    assert_eq!(call(&app, "POST", "/v1/auth/register", None, Some(json!({"email": "a@example.com", "password": PW}))).await.0, 403);
}

// ====================================================================== Phase 7: backup, restore, monitoring

const OPERATOR: &str = "operator-secret-token-0123456789";

fn app_ops(dir: &Path, tweak: impl FnOnce(&mut Config)) -> Router {
    app_with(dir, |c| {
        c.operator_token = Some(OPERATOR.to_string());
        c.backup_interval_secs = 0; // no background task in tests; backups are triggered explicitly
        tweak(c);
    })
}

/// Returns (owner token, org id, project id, owner-admin key).
async fn setup_project(app: &Router) -> (String, String, String, String) {
    let tok = signup(app, "owner@example.com").await;
    let org = mk_org(app, &tok, "o").await;
    let pid = mk_project(app, &tok, &org, "p").await;
    let (key, _) = mk_key(app, &tok, &pid, "dev", "admin").await;
    (tok, org, pid, key)
}

fn ids_of(list: &Value) -> Vec<String> {
    list["backups"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn backup_create_list_verify_delete() {
    let t = TempDir::new();
    let app = app_ops(t.path(), |_| {});
    let (tok, _org, pid, key) = setup_project(&app).await;
    call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&key), Some(json!({"name": "c"}))).await;
    for i in 0..20 {
        call(&app, "POST", &dp(&pid, "dev", "collections/c/documents"), Some(&key), Some(json!({"_id": format!("d{}", i)}))).await;
    }
    let url = dp(&pid, "dev", "backups");
    let (s, meta) = call(&app, "POST", &url, Some(&tok), None).await;
    assert_eq!(s, 201, "{:?}", meta);
    let bid = meta["id"].as_str().unwrap().to_string();
    assert!(meta["files"].as_array().unwrap().iter().all(|f| f["sha256"].as_str().unwrap().len() == 64));
    assert!(meta["entries"].as_u64().unwrap() >= 20);

    let (s, list) = call(&app, "GET", &url, Some(&tok), None).await;
    assert_eq!(s, 200);
    assert_eq!(ids_of(&list), vec![bid.clone()]);

    let (s, rep) = call(&app, "POST", &format!("{}/{}/verify", url, bid), Some(&tok), None).await;
    assert_eq!((s, rep["ok"].clone()), (200, json!(true)));

    // only the owner may delete
    let (s, _) = call(&app, "DELETE", &format!("{}/{}", url, bid), Some(&tok), None).await;
    assert_eq!(s, 200);
    assert_eq!(call(&app, "DELETE", &format!("{}/{}", url, bid), Some(&tok), None).await.0, 404);
    let (_, list) = call(&app, "GET", &url, Some(&tok), None).await;
    assert!(ids_of(&list).is_empty());
}

#[tokio::test]
async fn backup_access_control() {
    let t = TempDir::new();
    let app = app_ops(t.path(), |_| {});
    let (owner, org, pid, key) = setup_project(&app).await;
    let url = dp(&pid, "dev", "backups");
    // API keys (even admin keys) can never manage backups
    assert_eq!(call(&app, "POST", &url, Some(&key), None).await.0, 403);
    assert_eq!(call(&app, "GET", &url, Some(&key), None).await.0, 403);
    // a plain member cannot, a stranger gets 404
    let member = signup(&app, "m@example.com").await;
    let stranger = signup(&app, "s@example.com").await;
    call(&app, "POST", &format!("/v1/orgs/{}/members", org), Some(&owner), Some(json!({"email": "m@example.com", "role": "member"}))).await;
    assert_eq!(call(&app, "POST", &url, Some(&member), None).await.0, 403);
    assert_eq!(call(&app, "GET", &url, Some(&member), None).await.0, 403);
    assert_eq!(call(&app, "POST", &url, Some(&stranger), None).await.0, 404);
    assert_eq!(call(&app, "GET", &url, Some(&stranger), None).await.0, 404);
    // an org admin may create/list/verify but not restore or delete
    let admin = signup(&app, "a@example.com").await;
    call(&app, "POST", &format!("/v1/orgs/{}/members", org), Some(&owner), Some(json!({"email": "a@example.com", "role": "admin"}))).await;
    let (s, meta) = call(&app, "POST", &url, Some(&admin), None).await;
    assert_eq!(s, 201);
    let bid = meta["id"].as_str().unwrap();
    assert_eq!(call(&app, "POST", &format!("{}/{}/verify", url, bid), Some(&admin), None).await.0, 200);
    assert_eq!(call(&app, "POST", &format!("{}/{}/restore", url, bid), Some(&admin), Some(json!({"confirm": "dev"}))).await.0, 403);
    assert_eq!(call(&app, "DELETE", &format!("{}/{}", url, bid), Some(&admin), None).await.0, 403);
    // malformed ids never reach the file system
    for bad in ["..", "../x", "1760000000000-ZZZZ", "x"] {
        let (s, _) = call(&app, "POST", &format!("{}/{}/verify", url, bad), Some(&owner), None).await;
        assert_eq!(s, 404, "{}", bad);
    }
    assert_eq!(call(&app, "GET", &dp(&pid, "qa", "backups"), Some(&owner), None).await.0, 404);
}

#[tokio::test]
async fn restore_brings_back_the_old_state_and_keeps_working() {
    let t = TempDir::new();
    let app = app_ops(t.path(), |_| {});
    let (tok, _org, pid, key) = setup_project(&app).await;
    let docs = dp(&pid, "dev", "collections/c/documents");
    call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&key), Some(json!({"name": "c"}))).await;
    call(&app, "POST", &dp(&pid, "dev", "collections/c/indexes"), Some(&key), Some(json!({"field": "k"}))).await;
    call(&app, "POST", &docs, Some(&key), Some(json!({"_id": "A", "k": "old"}))).await;
    let url = dp(&pid, "dev", "backups");
    let (_, meta) = call(&app, "POST", &url, Some(&tok), None).await;
    let bid = meta["id"].as_str().unwrap().to_string();

    // damage after the backup: delete A, add B
    call(&app, "DELETE", &format!("{}/A", docs), Some(&key), None).await;
    call(&app, "POST", &docs, Some(&key), Some(json!({"_id": "B", "k": "new"}))).await;

    // restore needs the typed environment name
    let r = format!("{}/{}/restore", url, bid);
    assert_eq!(call(&app, "POST", &r, Some(&tok), Some(json!({}))).await.0, 400);
    assert_eq!(call(&app, "POST", &r, Some(&tok), Some(json!({"confirm": "prod"}))).await.0, 400);
    assert_eq!(call(&app, "GET", &format!("{}/B", docs), Some(&key), None).await.0, 200, "nothing changed yet");

    let (s, rep) = call(&app, "POST", &r, Some(&tok), Some(json!({"confirm": "dev"}))).await;
    assert_eq!(s, 200, "{:?}", rep);
    assert!(rep["safety_backup"].as_str().is_some());

    assert_eq!(call(&app, "GET", &format!("{}/A", docs), Some(&key), None).await.0, 200);
    assert_eq!(call(&app, "GET", &format!("{}/B", docs), Some(&key), None).await.0, 404);
    let (_, q) = call(&app, "POST", &dp(&pid, "dev", "collections/c/query"), Some(&key), Some(json!({"filter": {"k": "old"}}))).await;
    assert_eq!(q["count"], 1);
    assert_eq!(q["plan"], "IndexScan(field=k, op=eq)");

    // the database accepts writes after the restore
    assert_eq!(call(&app, "POST", &docs, Some(&key), Some(json!({"_id": "C", "k": "after"}))).await.0, 201);
    assert_eq!(call(&app, "GET", &format!("{}/C", docs), Some(&key), None).await.0, 200);

    // a pre-restore safety backup exists and still contains B, so the restore itself is undoable
    let (_, list) = call(&app, "GET", &url, Some(&tok), None).await;
    let all = ids_of(&list);
    assert_eq!(all.len(), 2);
    let safety = rep["safety_backup"].as_str().unwrap().to_string();
    assert!(all.contains(&safety));
    let (s, _) = call(&app, "POST", &format!("{}/{}/restore", url, safety), Some(&tok), Some(json!({"confirm": "dev"}))).await;
    assert_eq!(s, 200);
    assert_eq!(call(&app, "GET", &format!("{}/B", docs), Some(&key), None).await.0, 200);
    assert_eq!(call(&app, "GET", &format!("{}/A", docs), Some(&key), None).await.0, 404);

    // no temporary directories left behind
    let env_dir = t.path().join("projects").join(&pid);
    let names: Vec<String> = fs::read_dir(&env_dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["dev".to_string()], "{:?}", names);
}

#[tokio::test]
async fn tampered_backup_is_detected_and_restore_refused() {
    let t = TempDir::new();
    let app = app_ops(t.path(), |_| {});
    let (tok, _org, pid, key) = setup_project(&app).await;
    let docs = dp(&pid, "dev", "collections/c/documents");
    call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&key), Some(json!({"name": "c"}))).await;
    call(&app, "POST", &docs, Some(&key), Some(json!({"_id": "A", "v": "x".repeat(200)}))).await;
    let url = dp(&pid, "dev", "backups");
    let (_, meta) = call(&app, "POST", &url, Some(&tok), None).await;
    let bid = meta["id"].as_str().unwrap().to_string();

    let dir = t.path().join("backups").join("projects").join(&pid).join("dev").join(&bid);
    let sst = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).find(|p| p.extension().map_or(false, |e| e == "sst")).unwrap();
    let mut bytes = fs::read(&sst).unwrap();
    bytes[30] ^= 0xFF;
    fs::write(&sst, &bytes).unwrap();

    let (s, rep) = call(&app, "POST", &format!("{}/{}/verify", url, bid), Some(&tok), None).await;
    assert_eq!(s, 200);
    assert_eq!(rep["ok"], false);
    assert!(rep["error"].as_str().unwrap().contains("checksum"));
    call(&app, "POST", &docs, Some(&key), Some(json!({"_id": "LIVE"}))).await;
    let (s, v) = call(&app, "POST", &format!("{}/{}/restore", url, bid), Some(&tok), Some(json!({"confirm": "dev"}))).await;
    assert_eq!(s, 400, "{:?}", v);
    // live data untouched and still writable
    assert_eq!(call(&app, "GET", &format!("{}/LIVE", docs), Some(&key), None).await.0, 200);
    assert_eq!(call(&app, "GET", &format!("{}/A", docs), Some(&key), None).await.0, 200);
}

#[tokio::test]
async fn retention_keeps_only_the_newest_backups() {
    let t = TempDir::new();
    let app = app_ops(t.path(), |c| c.backup_retention = 2);
    let (tok, _org, pid, key) = setup_project(&app).await;
    call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&key), Some(json!({"name": "c"}))).await;
    let url = dp(&pid, "dev", "backups");
    let mut made = Vec::new();
    for _ in 0..4 {
        let (s, m) = call(&app, "POST", &url, Some(&tok), None).await;
        assert_eq!(s, 201);
        made.push(m["id"].as_str().unwrap().to_string());
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
    }
    let (_, list) = call(&app, "GET", &url, Some(&tok), None).await;
    let got = ids_of(&list);
    assert_eq!(got.len(), 2);
    assert_eq!(got, vec![made[3].clone(), made[2].clone()]);
}

#[tokio::test]
async fn operator_endpoints_are_off_without_a_token_and_protected_with_one() {
    let t = TempDir::new();
    let off = app_with(t.path(), |_| {});
    assert_eq!(call(&off, "GET", "/ops/metrics", Some(OPERATOR), None).await.0, 404);
    let t2 = TempDir::new();
    let app = app_ops(t2.path(), |_| {});
    assert_eq!(call(&app, "GET", "/ops/metrics", None, None).await.0, 401);
    assert_eq!(call(&app, "GET", "/ops/metrics", Some("wrong"), None).await.0, 401);
    // a user session or an API key is not an operator token
    let tok = signup(&app, "a@example.com").await;
    assert_eq!(call(&app, "GET", "/ops/metrics", Some(&tok), None).await.0, 401);
    assert_eq!(call(&app, "GET", "/ops/metrics", Some(OPERATOR), None).await.0, 200);
    assert_eq!(call(&app, "POST", "/ops/backup/run", Some("wrong"), None).await.0, 401);
    // operator token is not accepted on the tenant API
    assert_eq!(call(&app, "GET", "/v1/orgs", Some(OPERATOR), None).await.0, 401);
}

#[tokio::test]
async fn operator_token_must_be_long_enough() {
    let t = TempDir::new();
    let mut cfg = Config::new(t.path());
    cfg.argon_m_cost = 8;
    cfg.operator_token = Some("too-short".to_string());
    assert!(AppState::open(cfg).is_err());
}

#[tokio::test]
async fn metrics_reflect_real_traffic() {
    let t = TempDir::new();
    let app = app_ops(t.path(), |c| c.project_quota_bytes = 3000);
    let (_tok, _org, pid, key) = setup_project(&app).await;
    call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&key), Some(json!({"name": "c"}))).await;
    call(&app, "GET", "/v1/orgs", Some("bad-token"), None).await;
    for i in 0..10 {
        call(&app, "POST", &dp(&pid, "dev", "collections/c/documents"), Some(&key), Some(json!({"_id": format!("d{}", i), "blob": "x".repeat(800)}))).await;
    }
    let (s, m) = call(&app, "GET", "/ops/metrics", Some(OPERATOR), None).await;
    assert_eq!(s, 200);
    assert!(m["requests_total"].as_u64().unwrap() >= 15);
    assert!(m["responses_2xx"].as_u64().unwrap() >= 5);
    assert!(m["responses_4xx"].as_u64().unwrap() >= 1);
    assert!(m["auth_failures"].as_u64().unwrap() >= 1);
    assert!(m["quota_rejections"].as_u64().unwrap() >= 1, "{:?}", m);
    assert!(m["storage"]["data_dir_bytes"].as_u64().unwrap() > 0);
    assert!(m["open_project_databases"].as_u64().unwrap() >= 1);
    assert_eq!(m["alerts"], json!([]));

    // Prometheus text format
    let req = Request::builder().uri("/ops/metrics/prometheus").header("authorization", format!("Bearer {}", OPERATOR)).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let text = String::from_utf8(resp.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(text.contains("# TYPE rajeshdb_requests_total counter"));
    assert!(text.lines().any(|l| l.starts_with("rajeshdb_requests_total ") && !l.ends_with(" 0")));
    assert!(text.contains("rajeshdb_alerts_active 0"));
}

#[tokio::test]
async fn operator_backup_run_covers_control_db_and_every_used_environment() {
    let t = TempDir::new();
    let app = app_ops(t.path(), |_| {});
    let (_tok, _org, pid, key) = setup_project(&app).await;
    call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&key), Some(json!({"name": "c"}))).await; // only dev is ever used
    let (s, v) = call(&app, "POST", "/ops/backup/run", Some(OPERATOR), None).await;
    assert_eq!(s, 200);
    assert_eq!(v["succeeded"], 2); // control + dev (staging and prod were never opened)
    assert_eq!(v["failed"], json!([]));
    let (_, c) = call(&app, "GET", "/ops/backups/control", Some(OPERATOR), None).await;
    assert_eq!(c["backups"].as_array().unwrap().len(), 1);
    assert_eq!(c["backups"][0]["kind"], "control");
    let (_, m) = call(&app, "GET", "/ops/metrics", Some(OPERATOR), None).await;
    assert_eq!(m["backups"]["ok_total"], 2);
    assert!(m["backups"]["last_success_ms"].as_u64().unwrap() > 0);
    // the control backup really restores: open it as a database and look for our user
    let dir = t.path().join("backups").join("control").join(c["backups"][0]["id"].as_str().unwrap());
    let copy = t.path().join("copy-of-control");
    fs::create_dir_all(&copy).unwrap();
    for e in fs::read_dir(&dir).unwrap() {
        let e = e.unwrap();
        if e.file_name() != "backup.json" {
            fs::copy(e.path(), copy.join(e.file_name())).unwrap();
        }
    }
    let db = rajeshdb_docs::DocDb::open(&copy, rajeshdb_docs::Options { memtable_limit_bytes: 1 << 20, sync_writes: false }).unwrap();
    let users = db.find("users", &rajeshdb_docs::Query::new(json!({"email": "owner@example.com"}))).unwrap();
    assert_eq!(users.len(), 1);
}

#[tokio::test]
async fn disk_budget_raises_alerts_and_deep_health_reports_them() {
    let t = TempDir::new();
    let app = app_ops(t.path(), |c| c.disk_budget_bytes = 1); // absurdly small: always over budget
    signup(&app, "a@example.com").await;
    let (s, h) = call(&app, "GET", "/ops/health", Some(OPERATOR), None).await;
    assert_eq!(s, 503, "{:?}", h);
    assert_eq!(h["status"], "critical");
    assert_eq!(h["control_database"], "ok");
    assert_eq!(h["alerts"][0]["code"], "disk_budget");
    let t2 = TempDir::new();
    let ok = app_ops(t2.path(), |_| {});
    let (s, h) = call(&ok, "GET", "/ops/health", Some(OPERATOR), None).await;
    assert_eq!((s, h["status"].clone()), (200, json!("ok")));
}

// ====================================================================== Phase 8: behind a reverse proxy

async fn login_from(app: &Router, xff: Option<&str>) -> u16 {
    let mut b = Request::builder().method("POST").uri("/v1/auth/login").header("content-type", "application/json");
    if let Some(x) = xff {
        b = b.header("x-forwarded-for", x);
    }
    let body = Body::from(serde_json::to_vec(&json!({"email": "x@example.com", "password": "wrong password!!"})).unwrap());
    app.clone().oneshot(b.body(body).unwrap()).await.unwrap().status().as_u16()
}

#[tokio::test]
async fn forwarded_for_is_ignored_unless_the_proxy_is_trusted() {
    // Not trusted: spoofing a different X-Forwarded-For per request does NOT escape throttling.
    let t = TempDir::new();
    let app = app_with(t.path(), |c| {
        c.auth_ip_rate = 0.0001;
        c.auth_ip_burst = 3.0;
    });
    let mut codes = Vec::new();
    for i in 0..6 {
        codes.push(login_from(&app, Some(&format!("10.0.0.{}", i))).await);
    }
    assert!(codes.contains(&429), "spoofed headers must not help: {:?}", codes);
}

#[tokio::test]
async fn trusted_proxy_gives_each_client_its_own_throttle_bucket() {
    let t = TempDir::new();
    let app = app_with(t.path(), |c| {
        c.trust_proxy = true;
        c.auth_ip_rate = 0.0001;
        c.auth_ip_burst = 3.0;
    });
    // One client hammering from 203.0.113.9 gets throttled...
    let mut mine = Vec::new();
    for _ in 0..6 {
        mine.push(login_from(&app, Some("198.51.100.1, 203.0.113.9")).await);
    }
    assert!(mine.contains(&429), "{:?}", mine);
    // ...while a different client behind the same proxy is unaffected (the LAST entry is the proxy's view).
    assert_eq!(login_from(&app, Some("203.0.113.77")).await, 401);
    // A malformed header falls back to the connection address and never crashes.
    let _ = login_from(&app, Some("not-an-ip")).await;
}

// ====================================================================== Phase 9: hardening

#[tokio::test]
async fn busy_server_sheds_load_instead_of_queueing() {
    let t = TempDir::new();
    let app = app_with(t.path(), |c| c.max_inflight = 0);
    let (s, v) = call(&app, "GET", "/health", None, None).await;
    assert_eq!(s, 503);
    assert_eq!(v["error"]["code"], "busy");
    // with a sane limit everything works
    let t2 = TempDir::new();
    let ok = app_with(t2.path(), |c| c.max_inflight = 8);
    assert_eq!(call(&ok, "GET", "/health", None, None).await.0, 200);
}

#[tokio::test]
async fn slow_requests_time_out_with_504() {
    let t = TempDir::new();
    let app = app_with(t.path(), |c| c.request_timeout_secs = 0);
    // /v1/orgs with a token does asynchronous work, so a zero timeout always fires first
    let (s, v) = call(&app, "GET", "/v1/orgs", Some("rdbs_anything"), None).await;
    assert_eq!(s, 504, "{:?}", v);
    assert_eq!(v["error"]["code"], "timeout");
}

#[tokio::test]
async fn password_hashing_is_concurrency_limited() {
    let t = TempDir::new();
    let app = app_with(t.path(), |c| c.max_concurrent_hashes = 0);
    let (s, v) = call(&app, "POST", "/v1/auth/register", None, Some(json!({"email": "a@example.com", "password": PW}))).await;
    assert_eq!((s, v["error"]["code"].clone()), (503, json!("busy")));
    let (s, _) = call(&app, "POST", "/v1/auth/login", None, Some(json!({"email": "a@example.com", "password": PW}))).await;
    assert_eq!(s, 503);
}

#[tokio::test]
async fn old_audit_events_can_be_pruned() {
    let t = TempDir::new();
    let mut cfg = Config::new(t.path());
    cfg.argon_m_cost = 8;
    cfg.sync_writes = false;
    cfg.auth_ip_burst = 1000.0;
    cfg.auth_ip_rate = 1000.0;
    let st = AppState::open(cfg).ok().expect("state opens");
    let app = build_app(st.clone());
    signup(&app, "a@example.com").await; // writes user.register and user.login events
    let removed = match st.prune_audit_before(u64::MAX) {
        Ok(n) => n,
        Err(_) => panic!("pruning failed"),
    };
    assert!(removed >= 2, "removed {}", removed);
    let again = match st.prune_audit_before(u64::MAX) {
        Ok(n) => n,
        Err(_) => panic!("pruning failed"),
    };
    assert_eq!(again, 0);
}

#[tokio::test]
async fn export_returns_ndjson_with_a_cap() {
    let t = TempDir::new();
    let app = app_with(t.path(), |c| c.max_export_docs = 2);
    let tok = signup(&app, "a@example.com").await;
    let org = mk_org(&app, &tok, "o").await;
    let pid = mk_project(&app, &tok, &org, "p").await;
    let (rk, _) = mk_key(&app, &tok, &pid, "dev", "read").await;
    call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&tok), Some(json!({"name": "c"}))).await;
    for i in 0..3 {
        call(&app, "POST", &dp(&pid, "dev", "collections/c/documents"), Some(&tok), Some(json!({"_id": format!("d{}", i), "n": i}))).await;
    }
    let req = Request::builder().uri(dp(&pid, "dev", "collections/c/export")).header("authorization", format!("Bearer {}", rk)).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("content-type").unwrap(), "application/x-ndjson");
    assert_eq!(resp.headers().get("x-truncated").unwrap(), "true");
    assert!(resp.headers().get("content-disposition").unwrap().to_str().unwrap().contains("c.ndjson"));
    let text = String::from_utf8(resp.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    for l in &lines {
        let v: Value = serde_json::from_str(l).unwrap();
        assert!(v["_id"].is_string());
    }
    // a stranger gets 404, a missing collection 404
    let other = signup(&app, "b@example.com").await;
    let req = Request::builder().uri(dp(&pid, "dev", "collections/c/export")).header("authorization", format!("Bearer {}", other)).body(Body::empty()).unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status().as_u16(), 404);
    assert_eq!(call(&app, "GET", &dp(&pid, "dev", "collections/nope/export"), Some(&rk), None).await.0, 404);
}

#[tokio::test]
async fn expensive_filters_are_refused_over_the_api() {
    let t = TempDir::new();
    let app = app(t.path());
    let tok = signup(&app, "a@example.com").await;
    let org = mk_org(&app, &tok, "o").await;
    let pid = mk_project(&app, &tok, &org, "p").await;
    call(&app, "POST", &dp(&pid, "dev", "collections"), Some(&tok), Some(json!({"name": "c"}))).await;
    let big: Vec<i64> = (0..2000).collect();
    let (s, v) = call(&app, "POST", &dp(&pid, "dev", "collections/c/query"), Some(&tok), Some(json!({"filter": {"a": {"$in": big}}}))).await;
    assert_eq!(s, 400, "{:?}", v);
    let mut f = json!({"a": 1});
    for _ in 0..12 {
        f = json!({"$and": [f]});
    }
    let (s, _) = call(&app, "POST", &dp(&pid, "dev", "collections/c/query"), Some(&tok), Some(json!({"filter": f}))).await;
    assert_eq!(s, 400);
}
