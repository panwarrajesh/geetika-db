use crate::error::PErr;
use crate::state::*;
use crate::types::*;
use crate::util::*;
use axum::{
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Extension, Json, Router,
};
use rajeshdb_docs::{DocDb, DocError, Query, Value};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

type St = State<Arc<AppState>>;
type Who = Extension<Principal>;

// ---------------------------------------------------------------- plumbing

async fn blocking<T, F>(f: F) -> Result<T, PErr>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, PErr> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(_) => Err(PErr::internal("worker task failed")),
    }
}

async fn ctl_r<T, F>(st: &Arc<AppState>, f: F) -> Result<T, PErr>
where
    T: Send + 'static,
    F: FnOnce(&DocDb) -> Result<T, PErr> + Send + 'static,
{
    let st = st.clone();
    blocking(move || {
        let g = st.ctl.read().map_err(|_| poisoned())?;
        f(&*g)
    })
    .await
}

async fn ctl_w<T, F>(st: &Arc<AppState>, f: F) -> Result<T, PErr>
where
    T: Send + 'static,
    F: FnOnce(&mut DocDb) -> Result<T, PErr> + Send + 'static,
{
    let st = st.clone();
    blocking(move || {
        let mut g = st.ctl.write().map_err(|_| poisoned())?;
        f(&mut *g)
    })
    .await
}

fn rid(bytes: usize) -> Result<String, PErr> {
    rand_hex(bytes).map_err(|e| {
        println!("{}", json!({"level": "error", "msg": e}));
        PErr::internal("could not generate a secure random value")
    })
}

fn parse_json(body: &[u8]) -> Result<Value, PErr> {
    if body.is_empty() {
        return Err(PErr::bad("a JSON request body is required"));
    }
    serde_json::from_slice(body).map_err(|e| PErr::bad(format!("request body is not valid JSON: {}", e)))
}

fn str_field(v: &Value, key: &str) -> Result<String, PErr> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(String::from)
        .ok_or_else(|| PErr::bad(format!("'{}' (string) is required", key)))
}

fn user_id(p: &Principal) -> Result<String, PErr> {
    match p {
        Principal::User { user_id } => Ok(user_id.clone()),
        // API keys are for data access only; they can never manage users, orgs or other keys.
        Principal::Key { .. } => Err(PErr::forbidden()),
    }
}

fn valid_org_name(n: &str) -> bool {
    let c = n.chars().count();
    (1..=100).contains(&c) && !n.chars().any(|ch| ch.is_control())
}

fn valid_project_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn bad_id() -> PErr {
    PErr::not_found("not found")
}

// ---------------------------------------------------------------- middleware

fn too_many() -> Response {
    let mut r = PErr::rate().into_response();
    r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    r
}

fn resolve(st: &AppState, token: &str) -> Option<(Principal, String)> {
    let h = sha256_hex(token);
    let db = st.ctl.read().ok()?;
    if token.starts_with("rdbs_") {
        let s = db.get("sessions", &h).ok()??;
        if s.get("expires_ms")?.as_u64()? <= now_ms() {
            return None;
        }
        let uid = s.get("user_id")?.as_str()?.to_string();
        Some((Principal::User { user_id: uid }, h))
    } else if token.starts_with("rdb_") {
        let k = db.get("apikeys", &h).ok()??;
        if let Some(exp) = k.get("expires_ms").and_then(|e| e.as_u64()) {
            if exp <= now_ms() {
                return None;
            }
        }
        let p = Principal::Key {
            key_id: k.get("key_id")?.as_str()?.to_string(),
            project_id: k.get("project_id")?.as_str()?.to_string(),
            env: k.get("env")?.as_str()?.to_string(),
            role: Role::parse(k.get("role")?.as_str()?)?,
        };
        Some((p, h))
    } else {
        None
    }
}

async fn auth_mw(State(st): St, mut req: Request, next: Next) -> Response {
    let ip = req.extensions().get::<ClientIp>().map(|c| c.0.clone()).unwrap_or_else(|| "unknown".into());
    let token: Option<String> = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string());
    let resolved = match token {
        Some(t) => {
            let st2 = st.clone();
            tokio::task::spawn_blocking(move || resolve(&st2, &t)).await.ok().flatten()
        }
        None => None,
    };
    match resolved {
        None => {
            st.metrics.auth_failures.fetch_add(1, Ordering::Relaxed);
            if !st.fail_limiter.allow(&ip) {
                st.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
                return too_many();
            }
            let mut r = PErr::unauthorized("missing, invalid or expired credentials").into_response();
            r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            r
        }
        Some((p, hash)) => {
            if !st.limiter.allow(p.id()) {
                st.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
                return too_many();
            }
            let kid = p.id().to_string();
            req.extensions_mut().insert(p);
            req.extensions_mut().insert(TokenHash(hash));
            let mut resp = next.run(req).await;
            resp.extensions_mut().insert(KeyId(kid));
            resp
        }
    }
}

/// Last entry of X-Forwarded-For (the one added by OUR proxy), if it is a valid IP address.
fn forwarded_ip(h: &HeaderMap) -> Option<String> {
    let v = h.get("x-forwarded-for")?.to_str().ok()?;
    v.split(',').last()?.trim().parse::<std::net::IpAddr>().ok().map(|ip| ip.to_string())
}

struct InflightGuard(Arc<AppState>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::Relaxed);
    }
}

fn busy() -> PErr {
    PErr::new(StatusCode::SERVICE_UNAVAILABLE, "busy", "the server is busy, retry shortly")
}

async fn log_mw(State(st): St, mut req: Request, next: Next) -> Response {
    let start = Instant::now();
    let conn_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let ip = if st.cfg.trust_proxy { forwarded_ip(req.headers()).unwrap_or(conn_ip) } else { conn_ip };
    req.extensions_mut().insert(ClientIp(ip));
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    st.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let in_flight = st.inflight.fetch_add(1, Ordering::Relaxed) + 1;
    let _inflight = InflightGuard(st.clone());
    let mut resp = if in_flight > st.cfg.max_inflight {
        let mut r = busy().into_response();
        r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        r
    } else {
        match tokio::time::timeout(Duration::from_secs(st.cfg.request_timeout_secs), next.run(req)).await {
            Ok(r) => r,
            Err(_) => PErr::new(StatusCode::GATEWAY_TIMEOUT, "timeout", "the request took too long").into_response(),
        }
    };
    let status = resp.status().as_u16();
    let ms = start.elapsed().as_millis() as u64;
    st.metrics.latency_ms_sum.fetch_add(ms, Ordering::Relaxed);
    st.metrics.latency_count.fetch_add(1, Ordering::Relaxed);
    match status / 100 {
        2 => st.metrics.s2xx.fetch_add(1, Ordering::Relaxed),
        4 => st.metrics.s4xx.fetch_add(1, Ordering::Relaxed),
        5 => st.metrics.s5xx.fetch_add(1, Ordering::Relaxed),
        _ => 0,
    };
    if status == 507 {
        st.metrics.quota_rejections.fetch_add(1, Ordering::Relaxed);
    }
    let who = resp.extensions().get::<KeyId>().map(|k| k.0.clone());
    println!(
        "{}",
        json!({"ts_ms": now_ms(), "method": method, "path": path, "status": status,
               "ms": start.elapsed().as_millis() as u64, "principal": who, "uptime_s": st.started.elapsed().as_secs()})
    );
    let h = resp.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

// ---------------------------------------------------------------- public: health, register, login

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn not_found_route() -> PErr {
    PErr::not_found("no such route")
}

async fn register(State(st): St, Extension(ip): Extension<ClientIp>, body: Bytes) -> Result<(StatusCode, Json<Value>), PErr> {
    if !st.cfg.allow_registration {
        return Err(PErr::forbidden());
    }
    if !st.ip_limiter.allow(&ip.0) {
        return Err(PErr::rate());
    }
    let v = parse_json(&body)?;
    let email = normalize_email(&str_field(&v, "email")?).ok_or_else(|| PErr::bad("invalid e-mail address"))?;
    let pw = str_field(&v, "password")?;
    validate_password(&pw).map_err(PErr::bad)?;
    let _pw = st.enter_pw().ok_or_else(busy)?;
    let m = st.cfg.argon_m_cost;
    let pw_hash = blocking(move || hash_password(&pw, m).map_err(|e| {
        println!("{}", json!({"level": "error", "msg": "password hashing failed", "detail": e}));
        PErr::internal("could not process password")
    }))
    .await?;
    let id = rid(16)?;
    let (uid, em) = (id.clone(), email.clone());
    ctl_w(&st, move |db| {
        // Checked and inserted under the same write lock, so two concurrent sign-ups cannot both win.
        if find_one(db, "users", json!({"email": em}))?.is_some() {
            return Err(PErr::conflict("this e-mail address is already registered"));
        }
        db.insert("users", json!({"_id": uid, "email": em, "pw_hash": pw_hash, "created_ms": now_ms()}))?;
        audit(db, "", &format!("user:{}", uid), "user.register", &uid, "")?;
        Ok(())
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"user_id": id, "email": email}))))
}

async fn login(State(st): St, Extension(ip): Extension<ClientIp>, body: Bytes) -> Result<Json<Value>, PErr> {
    if !st.ip_limiter.allow(&ip.0) {
        return Err(PErr::rate());
    }
    let v = parse_json(&body)?;
    let email = normalize_email(&str_field(&v, "email")?).ok_or_else(|| PErr::unauthorized("invalid e-mail or password"))?;
    let pw = str_field(&v, "password")?;
    if !st.login_limiter.allow(&email) {
        return Err(PErr::rate());
    }
    let em = email.clone();
    let user = ctl_r(&st, move |db| Ok(find_one(db, "users", json!({"email": em}))?)).await?;
    let (stored, uid) = match &user {
        Some(u) => (
            u.get("pw_hash").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            u.get("_id").and_then(|x| x.as_str()).map(String::from),
        ),
        // Unknown e-mail: verify against a dummy hash so timing does not reveal whether it exists.
        None => (st.dummy_hash.clone(), None),
    };
    let _pw = st.enter_pw().ok_or_else(busy)?;
    let ok = blocking(move || Ok(verify_password(&pw, &stored))).await?;
    let uid = match (ok, uid) {
        (true, Some(u)) => u,
        _ => return Err(PErr::unauthorized("invalid e-mail or password")),
    };
    let token = format!("rdbs_{}", rid(32)?);
    let th = sha256_hex(&token);
    let ttl = st.cfg.session_ttl_secs;
    let u2 = uid.clone();
    ctl_w(&st, move |db| {
        // Opportunistic cleanup of expired sessions.
        db.delete_where("sessions", &json!({"expires_ms": {"$lt": now_ms()}}))?;
        db.insert("sessions", json!({"_id": th, "user_id": u2, "expires_ms": now_ms() + ttl * 1000}))?;
        audit(db, "", &format!("user:{}", u2), "user.login", &u2, "")?;
        Ok(())
    })
    .await?;
    Ok(Json(json!({"token": token, "expires_in": ttl, "user_id": uid})))
}

// ---------------------------------------------------------------- session

async fn logout(State(st): St, Extension(p): Who, Extension(th): Extension<TokenHash>) -> Result<Json<Value>, PErr> {
    user_id(&p)?;
    ctl_w(&st, move |db| {
        db.delete("sessions", &th.0)?;
        Ok(())
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

async fn me(State(st): St, Extension(p): Who) -> Result<Json<Value>, PErr> {
    let uid = user_id(&p)?;
    let u = ctl_r(&st, move |db| db.get("users", &uid)?.ok_or_else(|| PErr::not_found("user not found"))).await?;
    Ok(Json(json!({"user_id": u["_id"], "email": u["email"], "created_ms": u["created_ms"]})))
}

// ---------------------------------------------------------------- organizations

async fn create_org(State(st): St, Extension(p): Who, body: Bytes) -> Result<(StatusCode, Json<Value>), PErr> {
    let uid = user_id(&p)?;
    let v = parse_json(&body)?;
    let name = str_field(&v, "name")?;
    if !valid_org_name(&name) {
        return Err(PErr::bad("organization name must be 1-100 printable characters"));
    }
    let id = rid(16)?;
    let (oid, n) = (id.clone(), name.clone());
    let max = st.cfg.max_orgs_per_user;
    ctl_w(&st, move |db| {
        let owned = find_all(db, "members", json!({"user_id": uid, "role": "owner"}))?.len();
        if owned >= max {
            return Err(PErr::bad(format!("limit reached: at most {} organizations per user", max)));
        }
        db.insert("orgs", json!({"_id": oid, "name": n, "created_ms": now_ms()}))?;
        db.insert("members", json!({"_id": format!("{}:{}", oid, uid), "org_id": oid, "user_id": uid, "role": "owner"}))?;
        audit(db, &oid, &format!("user:{}", uid), "org.create", &oid, &n)?;
        Ok(())
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"org_id": id, "name": name}))))
}

async fn list_orgs(State(st): St, Extension(p): Who) -> Result<Json<Value>, PErr> {
    let uid = user_id(&p)?;
    let out = ctl_r(&st, move |db| {
        let mut out = Vec::new();
        for m in find_all(db, "members", json!({"user_id": uid}))? {
            let oid = m["org_id"].as_str().unwrap_or("").to_string();
            if let Some(o) = db.get("orgs", &oid)? {
                out.push(json!({"org_id": oid, "name": o["name"], "role": m["role"]}));
            }
        }
        Ok(out)
    })
    .await?;
    Ok(Json(json!({"organizations": out})))
}

async fn list_members(State(st): St, Extension(p): Who, Path(org): Path<String>) -> Result<Json<Value>, PErr> {
    let uid = user_id(&p)?;
    let out = ctl_r(&st, move |db| {
        need_org(db, &org, &uid, OrgRole::Member)?;
        let mut out = Vec::new();
        for m in find_all(db, "members", json!({"org_id": org}))? {
            let u = m["user_id"].as_str().unwrap_or("").to_string();
            let email = db.get("users", &u)?.map(|d| d["email"].clone()).unwrap_or(Value::Null);
            out.push(json!({"user_id": u, "email": email, "role": m["role"]}));
        }
        Ok(out)
    })
    .await?;
    Ok(Json(json!({"members": out})))
}

async fn add_member(State(st): St, Extension(p): Who, Path(org): Path<String>, body: Bytes) -> Result<(StatusCode, Json<Value>), PErr> {
    let uid = user_id(&p)?;
    let v = parse_json(&body)?;
    let email = normalize_email(&str_field(&v, "email")?).ok_or_else(|| PErr::bad("invalid e-mail address"))?;
    let role = OrgRole::parse(&str_field(&v, "role")?).ok_or_else(|| PErr::bad("role must be 'admin' or 'member'"))?;
    if role == OrgRole::Owner {
        return Err(PErr::bad("role must be 'admin' or 'member'; an organization has exactly one owner"));
    }
    let added = ctl_w(&st, move |db| {
        let mine = need_org(db, &org, &uid, OrgRole::Admin)?;
        if role == OrgRole::Admin && mine < OrgRole::Owner {
            return Err(PErr::forbidden()); // only the owner can appoint admins
        }
        let target = find_one(db, "users", json!({"email": email}))?.ok_or_else(|| PErr::not_found("no registered user with that e-mail"))?;
        let tid = target["_id"].as_str().unwrap_or("").to_string();
        if org_role(db, &org, &tid)?.is_some() {
            return Err(PErr::conflict("that user is already a member"));
        }
        db.insert("members", json!({"_id": format!("{}:{}", org, tid), "org_id": org, "user_id": tid, "role": role.as_str()}))?;
        audit(db, &org, &format!("user:{}", uid), "member.add", &tid, role.as_str())?;
        Ok(tid)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"user_id": added}))))
}

async fn remove_member(State(st): St, Extension(p): Who, Path((org, target)): Path<(String, String)>) -> Result<Json<Value>, PErr> {
    let uid = user_id(&p)?;
    if !is_hex_id(&target, 32) {
        return Err(bad_id());
    }
    ctl_w(&st, move |db| {
        let mine = need_org(db, &org, &uid, OrgRole::Admin)?;
        let theirs = org_role(db, &org, &target)?.ok_or_else(|| PErr::not_found("member not found"))?;
        if theirs == OrgRole::Owner {
            return Err(PErr::bad("the owner cannot be removed"));
        }
        if theirs == OrgRole::Admin && mine < OrgRole::Owner {
            return Err(PErr::forbidden());
        }
        db.delete("members", &format!("{}:{}", org, target))?;
        audit(db, &org, &format!("user:{}", uid), "member.remove", &target, "")?;
        Ok(())
    })
    .await?;
    Ok(Json(json!({"deleted": true})))
}

async fn read_audit(State(st): St, Extension(p): Who, Path(org): Path<String>) -> Result<Json<Value>, PErr> {
    let uid = user_id(&p)?;
    let rows = ctl_r(&st, move |db| {
        need_org(db, &org, &uid, OrgRole::Admin)?;
        let q = Query::from_json(&json!({"filter": {"org_id": org}, "sort": {"field": "ts_ms", "order": "desc"}, "limit": 200}))?;
        Ok(db.find("audit", &q)?)
    })
    .await?;
    Ok(Json(json!({"events": rows})))
}

// ---------------------------------------------------------------- projects

async fn create_project(State(st): St, Extension(p): Who, Path(org): Path<String>, body: Bytes) -> Result<(StatusCode, Json<Value>), PErr> {
    let uid = user_id(&p)?;
    let v = parse_json(&body)?;
    let name = str_field(&v, "name")?;
    if !valid_project_name(&name) {
        return Err(PErr::bad("project name must be 1-64 chars of A-Z a-z 0-9 _ -"));
    }
    let pid = rid(16)?;
    let (pid2, n) = (pid.clone(), name.clone());
    let max = st.cfg.max_projects_per_org;
    ctl_w(&st, move |db| {
        need_org(db, &org, &uid, OrgRole::Admin)?;
        let existing = find_all(db, "projects", json!({"org_id": org}))?;
        if existing.len() >= max {
            return Err(PErr::bad(format!("limit reached: at most {} projects per organization", max)));
        }
        if existing.iter().any(|d| d["name"] == n.as_str()) {
            return Err(PErr::conflict("a project with that name already exists"));
        }
        db.insert("projects", json!({"_id": pid2, "org_id": org, "name": n, "created_ms": now_ms(), "envs": ENVS}))?;
        audit(db, &org, &format!("user:{}", uid), "project.create", &pid2, &n)?;
        Ok(())
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"project_id": pid, "name": name, "environments": ENVS}))))
}

async fn list_projects(State(st): St, Extension(p): Who, Path(org): Path<String>) -> Result<Json<Value>, PErr> {
    let uid = user_id(&p)?;
    let rows = ctl_r(&st, move |db| {
        need_org(db, &org, &uid, OrgRole::Member)?;
        let rows = find_all(db, "projects", json!({"org_id": org}))?;
        Ok(rows.into_iter().map(|d| json!({"project_id": d["_id"], "name": d["name"], "environments": d["envs"], "created_ms": d["created_ms"]})).collect::<Vec<_>>())
    })
    .await?;
    Ok(Json(json!({"projects": rows})))
}

async fn delete_project(State(st): St, Extension(p): Who, Path(pid): Path<String>) -> Result<Json<Value>, PErr> {
    let uid = user_id(&p)?;
    let pid2 = pid.clone();
    ctl_w(&st, move |db| {
        let proj = get_project(db, &pid2)?;
        let org = proj["org_id"].as_str().unwrap_or("").to_string();
        need_org(db, &org, &uid, OrgRole::Owner)?;
        db.delete_where("apikeys", &json!({"project_id": pid2}))?;
        db.delete("projects", &pid2)?;
        audit(db, &org, &format!("user:{}", uid), "project.delete", &pid2, proj["name"].as_str().unwrap_or(""))?;
        Ok(())
    })
    .await?;
    let st2 = st.clone();
    blocking(move || st2.remove_project_storage(&pid)).await?;
    Ok(Json(json!({"deleted": true})))
}

// ---------------------------------------------------------------- project API keys

fn admin_project(db: &DocDb, pid: &str, uid: &str) -> Result<(Value, String), PErr> {
    let proj = get_project(db, pid)?;
    let org = proj["org_id"].as_str().unwrap_or("").to_string();
    need_org(db, &org, uid, OrgRole::Admin)?;
    Ok((proj, org))
}

fn new_key_doc(project_id: &str, name: &str, env: &str, role: Role, expires_ms: Option<u64>) -> Result<(String, String, Value), PErr> {
    let key = format!("rdb_{}", rid(32)?);
    let key_id = rid(8)?;
    let mut doc = json!({
        "_id": sha256_hex(&key), "key_id": key_id, "project_id": project_id, "env": env,
        "role": role.as_str(), "name": name, "created_ms": now_ms()
    });
    if let Some(e) = expires_ms {
        doc["expires_ms"] = json!(e);
    }
    Ok((key, key_id, doc))
}

fn key_summary(d: &Value) -> Value {
    json!({"key_id": d["key_id"], "name": d["name"], "env": d["env"], "role": d["role"],
           "created_ms": d["created_ms"], "expires_ms": d.get("expires_ms").cloned().unwrap_or(Value::Null)})
}

async fn list_keys(State(st): St, Extension(p): Who, Path(pid): Path<String>) -> Result<Json<Value>, PErr> {
    let uid = user_id(&p)?;
    let rows = ctl_r(&st, move |db| {
        admin_project(db, &pid, &uid)?;
        Ok(find_all(db, "apikeys", json!({"project_id": pid}))?.iter().map(key_summary).collect::<Vec<_>>())
    })
    .await?;
    Ok(Json(json!({"keys": rows})))
}

async fn create_key(State(st): St, Extension(p): Who, Path(pid): Path<String>, body: Bytes) -> Result<(StatusCode, Json<Value>), PErr> {
    let uid = user_id(&p)?;
    let v = parse_json(&body)?;
    let name = str_field(&v, "name")?;
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(|c| c.is_control()) {
        return Err(PErr::bad("name must be 1-64 printable characters"));
    }
    let env = str_field(&v, "env")?;
    if !ENVS.contains(&env.as_str()) {
        return Err(PErr::bad("env must be dev, staging or prod"));
    }
    let role = Role::parse(&str_field(&v, "role")?).ok_or_else(|| PErr::bad("role must be read, write or admin"))?;
    let expires = match v.get("expires_in_secs") {
        None | Some(Value::Null) => None,
        Some(x) => Some(now_ms() + x.as_u64().filter(|s| *s > 0).ok_or_else(|| PErr::bad("expires_in_secs must be a positive integer"))? * 1000),
    };
    let (key, key_id, doc) = new_key_doc(&pid, &name, &env, role, expires)?;
    let max = st.cfg.max_keys_per_project;
    let kid = key_id.clone();
    ctl_w(&st, move |db| {
        let (_, org) = admin_project(db, &pid, &uid)?;
        if find_all(db, "apikeys", json!({"project_id": pid}))?.len() >= max {
            return Err(PErr::bad(format!("limit reached: at most {} keys per project", max)));
        }
        db.insert("apikeys", doc)?;
        audit(db, &org, &format!("user:{}", uid), "key.create", &kid, &format!("{} {}", role.as_str(), name))?;
        Ok(())
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"key_id": key_id, "api_key": key, "env": env, "role": role.as_str(),
        "note": "Copy the api_key now; it cannot be shown again."}))))
}

async fn revoke_key(State(st): St, Extension(p): Who, Path((pid, kid)): Path<(String, String)>) -> Result<Json<Value>, PErr> {
    let uid = user_id(&p)?;
    ctl_w(&st, move |db| {
        let (_, org) = admin_project(db, &pid, &uid)?;
        let k = find_one(db, "apikeys", json!({"project_id": pid, "key_id": kid}))?.ok_or_else(|| PErr::not_found("key not found"))?;
        db.delete("apikeys", k["_id"].as_str().unwrap_or(""))?;
        audit(db, &org, &format!("user:{}", uid), "key.revoke", &kid, "")?;
        Ok(())
    })
    .await?;
    Ok(Json(json!({"revoked": true})))
}

/// Issue a replacement key with the same name/env/role. With `grace_seconds` > 0 the old key
/// keeps working until then (so clients can be redeployed); otherwise it is revoked immediately.
async fn rotate_key(State(st): St, Extension(p): Who, Path((pid, kid)): Path<(String, String)>, body: Bytes) -> Result<(StatusCode, Json<Value>), PErr> {
    let uid = user_id(&p)?;
    let grace = if body.is_empty() {
        0
    } else {
        let v = parse_json(&body)?;
        match v.get("grace_seconds") {
            None | Some(Value::Null) => 0,
            Some(x) => x.as_u64().filter(|g| *g <= 7 * 24 * 3600).ok_or_else(|| PErr::bad("grace_seconds must be 0..604800"))?,
        }
    };
    let out = ctl_w(&st, move |db| {
        let (_, org) = admin_project(db, &pid, &uid)?;
        let old = find_one(db, "apikeys", json!({"project_id": pid, "key_id": kid}))?.ok_or_else(|| PErr::not_found("key not found"))?;
        let role = Role::parse(old["role"].as_str().unwrap_or("")).ok_or_else(|| PErr::internal("stored key is damaged"))?;
        let env = old["env"].as_str().unwrap_or("").to_string();
        let name = old["name"].as_str().unwrap_or("").to_string();
        let old_id = old["_id"].as_str().unwrap_or("").to_string();
        let (key, new_id, doc) = new_key_doc(&pid, &name, &env, role, None)?;
        db.insert("apikeys", doc)?;
        if grace == 0 {
            db.delete("apikeys", &old_id)?;
        } else {
            db.update_where("apikeys", &json!({"_id": old_id}), &json!({"expires_ms": now_ms() + grace * 1000}))?;
        }
        audit(db, &org, &format!("user:{}", uid), "key.rotate", &kid, &format!("new={} grace={}s", new_id, grace))?;
        Ok((key, new_id, env, role))
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"key_id": out.1, "api_key": out.0, "env": out.2, "role": out.3.as_str(),
        "note": "Copy the api_key now; it cannot be shown again."}))))
}

// ---------------------------------------------------------------- data plane

/// Decide whether `p` may use project/env with at least `need`, then open that database.
/// Every mismatch (other project, other env, not a member) is a 404, so tenants cannot probe
/// for each other's projects.
fn authorize_data(st: &Arc<AppState>, p: &Principal, pid: &str, env: &str, need: Role, writes: bool) -> Result<Arc<ProjDb>, PErr> {
    if !is_hex_id(pid, 32) || !ENVS.contains(&env) {
        return Err(PErr::not_found("project or environment not found"));
    }
    let granted = match p {
        Principal::Key { project_id, env: kenv, role, .. } => {
            if project_id != pid || kenv != env {
                return Err(PErr::not_found("project or environment not found"));
            }
            *role
        }
        Principal::User { user_id } => {
            let ctl = st.ctl.read().map_err(|_| poisoned())?;
            let proj = ctl.get("projects", pid)?.ok_or_else(|| PErr::not_found("project or environment not found"))?;
            let org = proj["org_id"].as_str().unwrap_or("").to_string();
            match org_role(&ctl, &org, user_id)? {
                None => return Err(PErr::not_found("project or environment not found")),
                Some(r) => r.data_role(),
            }
        }
    };
    if granted < need {
        return Err(PErr::forbidden());
    }
    let pdb = st.open_project(pid, env)?;
    if writes {
        pdb.check_quota()?;
    }
    Ok(pdb)
}

async fn data_op<T, F>(st: &Arc<AppState>, p: Principal, pid: String, env: String, need: Role, writes: bool, f: F) -> Result<T, PErr>
where
    T: Send + 'static,
    F: FnOnce(&ProjDb) -> Result<T, DocError> + Send + 'static,
{
    let st = st.clone();
    blocking(move || {
        let pdb = authorize_data(&st, &p, &pid, &env, need, writes)?;
        f(&*pdb).map_err(PErr::from)
    })
    .await
}

/// Best-effort audit record for administrative data-plane actions.
async fn audit_data(st: &Arc<AppState>, p: &Principal, pid: &str, action: &'static str, target: String) {
    let (actor, pid) = (p.actor(), pid.to_string());
    let _ = ctl_w(st, move |db| {
        if let Some(proj) = db.get("projects", &pid)? {
            let org = proj["org_id"].as_str().unwrap_or("").to_string();
            audit(db, &org, &actor, action, &target, &pid)?;
        }
        Ok(())
    })
    .await;
}

type P2 = Path<(String, String)>;
type P3 = Path<(String, String, String)>;
type P4 = Path<(String, String, String, String)>;

async fn dp_list_collections(State(st): St, Extension(p): Who, Path((pid, env)): P2) -> Result<Json<Value>, PErr> {
    let names = data_op(&st, p, pid, env, Role::Read, false, |d| d.rd()?.list_collections()).await?;
    Ok(Json(json!({"collections": names})))
}

async fn dp_create_collection(State(st): St, Extension(p): Who, Path((pid, env)): P2, body: Bytes) -> Result<(StatusCode, Json<Value>), PErr> {
    let v = parse_json(&body)?;
    let name = str_field(&v, "name")?;
    let schema = v.get("schema").cloned();
    let n = name.clone();
    data_op(&st, p.clone(), pid.clone(), env, Role::Admin, true, move |d| d.wr()?.create_collection(&n, schema)).await?;
    audit_data(&st, &p, &pid, "collection.create", name.clone()).await;
    Ok((StatusCode::CREATED, Json(json!({"name": name}))))
}

async fn dp_drop_collection(State(st): St, Extension(p): Who, Path((pid, env, coll)): P3) -> Result<Json<Value>, PErr> {
    let c = coll.clone();
    data_op(&st, p.clone(), pid.clone(), env, Role::Admin, false, move |d| d.wr()?.drop_collection(&c)).await?;
    audit_data(&st, &p, &pid, "collection.drop", coll).await;
    Ok(Json(json!({"deleted": true})))
}

async fn dp_list_indexes(State(st): St, Extension(p): Who, Path((pid, env, coll)): P3) -> Result<Json<Value>, PErr> {
    let idx = data_op(&st, p, pid, env, Role::Read, false, move |d| d.rd()?.list_indexes(&coll)).await?;
    Ok(Json(json!({"indexes": idx})))
}

async fn dp_create_index(State(st): St, Extension(p): Who, Path((pid, env, coll)): P3, body: Bytes) -> Result<(StatusCode, Json<Value>), PErr> {
    let v = parse_json(&body)?;
    let field = str_field(&v, "field")?;
    let f = field.clone();
    data_op(&st, p, pid, env, Role::Admin, true, move |d| d.wr()?.create_index(&coll, &f)).await?;
    Ok((StatusCode::CREATED, Json(json!({"field": field}))))
}

async fn dp_drop_index(State(st): St, Extension(p): Who, Path((pid, env, coll, field)): P4) -> Result<Json<Value>, PErr> {
    data_op(&st, p, pid, env, Role::Admin, false, move |d| d.wr()?.drop_index(&coll, &field)).await?;
    Ok(Json(json!({"deleted": true})))
}

async fn dp_insert(State(st): St, Extension(p): Who, Path((pid, env, coll)): P3, body: Bytes) -> Result<(StatusCode, Json<Value>), PErr> {
    let doc = parse_json(&body)?;
    let id = data_op(&st, p, pid, env, Role::Write, true, move |d| d.wr()?.insert(&coll, doc)).await?;
    Ok((StatusCode::CREATED, Json(json!({"_id": id}))))
}

async fn dp_get(State(st): St, Extension(p): Who, Path((pid, env, coll, id)): P4) -> Result<Json<Value>, PErr> {
    let found = data_op(&st, p, pid, env, Role::Read, false, move |d| d.rd()?.get(&coll, &id)).await?;
    found.map(Json).ok_or_else(|| PErr::not_found("document does not exist"))
}

async fn dp_replace(State(st): St, Extension(p): Who, Path((pid, env, coll, id)): P4, body: Bytes) -> Result<Json<Value>, PErr> {
    let doc = parse_json(&body)?;
    let id2 = id.clone();
    data_op(&st, p, pid, env, Role::Write, true, move |d| d.wr()?.replace(&coll, &id2, doc)).await?;
    Ok(Json(json!({"_id": id})))
}

async fn dp_delete(State(st): St, Extension(p): Who, Path((pid, env, coll, id)): P4) -> Result<Json<Value>, PErr> {
    let existed = data_op(&st, p, pid, env, Role::Write, false, move |d| d.wr()?.delete(&coll, &id)).await?;
    if existed {
        Ok(Json(json!({"deleted": true})))
    } else {
        Err(PErr::not_found("document does not exist"))
    }
}

async fn dp_query(State(st): St, Extension(p): Who, Path((pid, env, coll)): P3, body: Bytes) -> Result<Json<Value>, PErr> {
    let v = if body.is_empty() { json!({}) } else { parse_json(&body)? };
    let mut q = Query::from_json(&v)?;
    let limit = q.limit.unwrap_or(st.cfg.default_limit).min(st.cfg.max_limit);
    q.limit = Some(limit);
    let skip = q.skip;
    let (docs, plan) = data_op(&st, p, pid, env, Role::Read, false, move |d| {
        let db = d.rd()?;
        let docs = db.find(&coll, &q)?;
        let plan = db.explain(&coll, &q)?;
        Ok((docs, plan))
    })
    .await?;
    let count = docs.len();
    Ok(Json(json!({"documents": docs, "count": count, "skip": skip, "limit": limit, "plan": plan})))
}

fn filter_of(v: &Value) -> Result<Value, PErr> {
    let f = v.get("filter").cloned().ok_or_else(|| PErr::bad("'filter' (object) is required"))?;
    let o = f.as_object().ok_or_else(|| PErr::bad("'filter' must be an object"))?;
    if o.is_empty() && v.get("confirm_all").and_then(|c| c.as_bool()) != Some(true) {
        return Err(PErr::bad("an empty filter matches every document; add \"confirm_all\": true to do that on purpose"));
    }
    Ok(f)
}

async fn dp_update(State(st): St, Extension(p): Who, Path((pid, env, coll)): P3, body: Bytes) -> Result<Json<Value>, PErr> {
    let v = parse_json(&body)?;
    let filter = filter_of(&v)?;
    let set = v.get("set").cloned().ok_or_else(|| PErr::bad("'set' (object) is required"))?;
    let n = data_op(&st, p, pid, env, Role::Write, true, move |d| d.wr()?.update_where(&coll, &filter, &set)).await?;
    Ok(Json(json!({"modified": n})))
}

async fn dp_delete_where(State(st): St, Extension(p): Who, Path((pid, env, coll)): P3, body: Bytes) -> Result<Json<Value>, PErr> {
    let v = parse_json(&body)?;
    let filter = filter_of(&v)?;
    let n = data_op(&st, p, pid, env, Role::Write, false, move |d| d.wr()?.delete_where(&coll, &filter)).await?;
    Ok(Json(json!({"deleted": n})))
}

/// All documents of a collection as newline-delimited JSON (read role). Capped by max_export_docs;
/// a capped response carries the header `x-truncated: true`.
async fn dp_export(State(st): St, Extension(p): Who, Path((pid, env, coll)): P3) -> Result<Response, PErr> {
    let cap = st.cfg.max_export_docs;
    let safe_name = !coll.is_empty() && coll.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    let name = coll.clone();
    let (docs, truncated) = data_op(&st, p, pid, env, Role::Read, false, move |d| {
        let db = d.rd()?;
        let mut q = Query::new(json!({}));
        q.limit = Some(cap + 1);
        let mut docs = db.find(&coll, &q)?;
        let truncated = docs.len() > cap;
        docs.truncate(cap);
        Ok((docs, truncated))
    })
    .await?;
    let mut body = String::new();
    for d in &docs {
        body.push_str(&d.to_string());
        body.push('\n');
    }
    let mut resp = ([(header::CONTENT_TYPE, "application/x-ndjson")], body).into_response();
    if truncated {
        resp.headers_mut().insert("x-truncated", HeaderValue::from_static("true"));
    }
    if safe_name {
        if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{}.ndjson\"", name)) {
            resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
        }
    }
    Ok(resp)
}

async fn dp_usage(State(st): St, Extension(p): Who, Path((pid, env)): P2) -> Result<Json<Value>, PErr> {
    let (used, quota, stats) = data_op(&st, p, pid, env, Role::Read, false, |d| Ok((d.used_bytes(), d.quota, d.rd()?.stats()))).await?;
    Ok(Json(json!({
        "used_bytes": used, "quota_bytes": quota,
        "memtable_entries": stats.memtable_entries, "sstable_count": stats.sstable_count,
        "sstable_keys": stats.sstable_keys, "wal_bytes": stats.wal_bytes
    })))
}

// ---------------------------------------------------------------- backups (project owners/admins)

/// Backups are managed with a login (never with an API key) by organization admins/owners.
fn authorize_backup(st: &Arc<AppState>, p: &Principal, pid: &str, env: &str, min: OrgRole) -> Result<(), PErr> {
    let uid = user_id(p)?;
    if !is_hex_id(pid, 32) || !ENVS.contains(&env) {
        return Err(PErr::not_found("project or environment not found"));
    }
    let ctl = st.ctl.read().map_err(|_| poisoned())?;
    let proj = get_project(&ctl, pid)?;
    let org = proj["org_id"].as_str().unwrap_or("").to_string();
    need_org(&ctl, &org, &uid, min)?;
    Ok(())
}

async fn bk_list(State(st): St, Extension(p): Who, Path((pid, env)): P2) -> Result<Json<Value>, PErr> {
    let st2 = st.clone();
    let list = blocking(move || {
        authorize_backup(&st2, &p, &pid, &env, OrgRole::Admin)?;
        st2.list_env_backups(&pid, &env)
    })
    .await?;
    Ok(Json(json!({"backups": list})))
}

async fn bk_create(State(st): St, Extension(p): Who, Path((pid, env)): P2) -> Result<(StatusCode, Json<Value>), PErr> {
    let st2 = st.clone();
    let (p2, pid2) = (p.clone(), pid.clone());
    let meta = blocking(move || {
        authorize_backup(&st2, &p2, &pid2, &env, OrgRole::Admin)?;
        st2.backup_env(&pid2, &env)
    })
    .await?;
    audit_data(&st, &p, &pid, "backup.create", meta["id"].as_str().unwrap_or("").to_string()).await;
    Ok((StatusCode::CREATED, Json(meta)))
}

async fn bk_verify(State(st): St, Extension(p): Who, Path((pid, env, bid)): P3) -> Result<Json<Value>, PErr> {
    let st2 = st.clone();
    let report = blocking(move || {
        authorize_backup(&st2, &p, &pid, &env, OrgRole::Admin)?;
        st2.verify_env_backup(&pid, &env, &bid)
    })
    .await?;
    Ok(Json(report))
}

async fn bk_delete(State(st): St, Extension(p): Who, Path((pid, env, bid)): P3) -> Result<Json<Value>, PErr> {
    let st2 = st.clone();
    let (p2, pid2, bid2) = (p.clone(), pid.clone(), bid.clone());
    blocking(move || {
        authorize_backup(&st2, &p2, &pid2, &env, OrgRole::Owner)?;
        st2.delete_env_backup(&pid2, &env, &bid2)
    })
    .await?;
    audit_data(&st, &p, &pid, "backup.delete", bid).await;
    Ok(Json(json!({"deleted": true})))
}

/// Body: {"confirm": "<environment name>"}. Owner only. Replaces the live data of the environment.
async fn bk_restore(State(st): St, Extension(p): Who, Path((pid, env, bid)): P3, body: Bytes) -> Result<Json<Value>, PErr> {
    let v = parse_json(&body)?;
    if v.get("confirm").and_then(|c| c.as_str()) != Some(env.as_str()) {
        return Err(PErr::bad(format!("restoring replaces all current data; send {{\"confirm\": \"{}\"}} to proceed", env)));
    }
    let st2 = st.clone();
    let (p2, pid2, bid2) = (p.clone(), pid.clone(), bid.clone());
    let report = blocking(move || {
        authorize_backup(&st2, &p2, &pid2, &env, OrgRole::Owner)?;
        st2.restore_env(&pid2, &env, &bid2)
    })
    .await?;
    audit_data(&st, &p, &pid, "backup.restore", bid).await;
    Ok(Json(report))
}

// ---------------------------------------------------------------- operator endpoints (/ops/*)

/// Server-wide monitoring for whoever runs the service. Disabled (404) unless
/// RAJESHDB_OPERATOR_TOKEN is set. Do NOT expose /ops/* publicly: block it in the reverse proxy.
async fn ops_mw(State(st): St, req: Request, next: Next) -> Response {
    let expected = match &st.operator_hash {
        Some(h) => h.clone(),
        None => return PErr::not_found("no such route").into_response(),
    };
    let ip = req.extensions().get::<ClientIp>().map(|c| c.0.clone()).unwrap_or_else(|| "unknown".into());
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| sha256_hex(t.trim()))
        .unwrap_or_default();
    if !ct_eq(&given, &expected) {
        st.metrics.auth_failures.fetch_add(1, Ordering::Relaxed);
        if !st.fail_limiter.allow(&ip) {
            st.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
            return too_many();
        }
        return PErr::unauthorized("invalid operator token").into_response();
    }
    next.run(req).await
}

async fn ops_metrics(State(st): St) -> Result<Json<Value>, PErr> {
    let st2 = st.clone();
    Ok(Json(blocking(move || Ok(st2.metrics_json())).await?))
}

async fn ops_prometheus(State(st): St) -> Result<Response, PErr> {
    let st2 = st.clone();
    let text = blocking(move || Ok(st2.prometheus())).await?;
    Ok(([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], text).into_response())
}

async fn ops_health(State(st): St) -> Result<Response, PErr> {
    let st2 = st.clone();
    let v = blocking(move || Ok(st2.deep_health())).await?;
    let code = if v["status"] == "critical" { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::OK };
    Ok((code, Json(v)).into_response())
}

async fn ops_backup_run(State(st): St) -> Result<Json<Value>, PErr> {
    let st2 = st.clone();
    let (ok, failed) = blocking(move || Ok(st2.backup_all())).await?;
    Ok(Json(json!({"succeeded": ok, "failed": failed})))
}

async fn ops_control_backups(State(st): St) -> Result<Json<Value>, PErr> {
    let root = st.control_backup_root();
    let list = blocking(move || Ok(crate::backup::list_backups(&root))).await?;
    Ok(Json(json!({"backups": list})))
}

// ---------------------------------------------------------------- router

pub fn build_app(st: Arc<AppState>) -> Router {
    let protected: Router<Arc<AppState>> = Router::new()
        .route("/v1/me", get(me))
        .route("/v1/auth/logout", post(logout))
        .route("/v1/orgs", get(list_orgs).post(create_org))
        .route("/v1/orgs/:org/members", get(list_members).post(add_member))
        .route("/v1/orgs/:org/members/:uid", delete(remove_member))
        .route("/v1/orgs/:org/projects", get(list_projects).post(create_project))
        .route("/v1/orgs/:org/audit", get(read_audit))
        .route("/v1/projects/:pid", delete(delete_project))
        .route("/v1/projects/:pid/keys", get(list_keys).post(create_key))
        .route("/v1/projects/:pid/keys/:kid", delete(revoke_key))
        .route("/v1/projects/:pid/keys/:kid/rotate", post(rotate_key))
        .route("/v1/projects/:pid/envs/:env/usage", get(dp_usage))
        .route("/v1/projects/:pid/envs/:env/backups", get(bk_list).post(bk_create))
        .route("/v1/projects/:pid/envs/:env/backups/:bid", delete(bk_delete))
        .route("/v1/projects/:pid/envs/:env/backups/:bid/verify", post(bk_verify))
        .route("/v1/projects/:pid/envs/:env/backups/:bid/restore", post(bk_restore))
        .route("/v1/projects/:pid/envs/:env/collections", get(dp_list_collections).post(dp_create_collection))
        .route("/v1/projects/:pid/envs/:env/collections/:coll", delete(dp_drop_collection))
        .route("/v1/projects/:pid/envs/:env/collections/:coll/indexes", get(dp_list_indexes).post(dp_create_index))
        .route("/v1/projects/:pid/envs/:env/collections/:coll/indexes/:field", delete(dp_drop_index))
        .route("/v1/projects/:pid/envs/:env/collections/:coll/documents", post(dp_insert))
        .route("/v1/projects/:pid/envs/:env/collections/:coll/documents/:id", get(dp_get).put(dp_replace).delete(dp_delete))
        .route("/v1/projects/:pid/envs/:env/collections/:coll/query", post(dp_query))
        .route("/v1/projects/:pid/envs/:env/collections/:coll/update", post(dp_update))
        .route("/v1/projects/:pid/envs/:env/collections/:coll/delete", post(dp_delete_where))
        .route("/v1/projects/:pid/envs/:env/collections/:coll/export", get(dp_export))
        .route_layer(middleware::from_fn_with_state(st.clone(), auth_mw));

    let ops: Router<Arc<AppState>> = Router::new()
        .route("/ops/metrics", get(ops_metrics))
        .route("/ops/metrics/prometheus", get(ops_prometheus))
        .route("/ops/health", get(ops_health))
        .route("/ops/backup/run", post(ops_backup_run))
        .route("/ops/backups/control", get(ops_control_backups))
        .route_layer(middleware::from_fn_with_state(st.clone(), ops_mw));

    Router::new()
        .route("/health", get(health))
        .merge(ops)
        .route("/v1/auth/register", post(register))
        .route("/v1/auth/login", post(login))
        .merge(protected)
        .fallback(not_found_route)
        .layer(DefaultBodyLimit::max(st.cfg.max_body))
        .layer(middleware::from_fn_with_state(st.clone(), log_mw))
        .with_state(st)
}
