use rajeshdb_platform::{build_app, AppState, Config};
use std::net::SocketAddr;
use std::process::exit;

const HELP: &str = "\
RajeshDB platform server (Phase 5)

USAGE:
  rajeshdb-platform serve     start the multi-tenant HTTP API (default)
  rajeshdb-platform healthcheck   exit 0 if the local server answers GET /health (used by Docker)
  rajeshdb-platform help

CONFIG (environment variables, all optional):
  RAJESHDB_DATA_DIR              ./rajeshdb-platform-data
  RAJESHDB_BIND                  127.0.0.1:8081
  RAJESHDB_MAX_BODY_BYTES        1048576
  RAJESHDB_RATE_PER_SEC          50
  RAJESHDB_RATE_BURST            100
  RAJESHDB_DEFAULT_LIMIT         100
  RAJESHDB_MAX_LIMIT             1000
  RAJESHDB_SESSION_TTL_SECS      86400
  RAJESHDB_PROJECT_QUOTA_BYTES   104857600   (per project environment)
  RAJESHDB_ARGON_MEMORY_KIB      19456
  RAJESHDB_ALLOW_REGISTRATION    true
  RAJESHDB_MAX_PROJECTS_PER_ORG  20
  RAJESHDB_MAX_KEYS_PER_PROJECT  20
  RAJESHDB_MAX_ORGS_PER_USER     10
  RAJESHDB_SYNC_WRITES           true
  RAJESHDB_BACKUP_INTERVAL_SECS  21600       (0 = no automatic backups)
  RAJESHDB_BACKUP_RETENTION      7           (backups kept per database)
  RAJESHDB_DISK_BUDGET_BYTES     0           (alert at 80% / 95% of this; 0 = off)
  RAJESHDB_REQUEST_TIMEOUT_SECS  30          (HTTP 504 after this long)
  RAJESHDB_MAX_INFLIGHT          256         (concurrent requests; more get HTTP 503)
  RAJESHDB_MAX_CONCURRENT_HASHES 4           (simultaneous password hashes; more get HTTP 503)
  RAJESHDB_AUDIT_RETENTION_DAYS  365         (0 = keep audit events forever)
  RAJESHDB_MAX_EXPORT_DOCS       50000       (documents per export)
  RAJESHDB_TRUST_PROXY           false       true only behind your own reverse proxy (client IP from X-Forwarded-For)
  RAJESHDB_OPERATOR_TOKEN        (unset)     secret for /ops/* (>= 24 chars); unset disables /ops
                                             never expose /ops publicly: block it in the reverse proxy";

fn fail(msg: &str) -> ! {
    eprintln!("error: {}", msg);
    exit(1);
}

#[tokio::main]
async fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "serve".to_string());
    match cmd.as_str() {
        "help" | "--help" | "-h" => println!("{}", HELP),
        "healthcheck" => {
            use std::io::{Read, Write};
            let bind = std::env::var("RAJESHDB_BIND").unwrap_or_else(|_| "127.0.0.1:8081".to_string());
            let port: u16 = bind.rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(8081);
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            let ok = (|| -> std::io::Result<bool> {
                let timeout = std::time::Duration::from_secs(3);
                let mut s = std::net::TcpStream::connect_timeout(&addr, timeout)?;
                s.set_read_timeout(Some(timeout))?;
                s.write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
                let mut body = String::new();
                s.read_to_string(&mut body)?;
                Ok(body.starts_with("HTTP/1.1 200") || body.starts_with("HTTP/1.0 200"))
            })()
            .unwrap_or(false);
            exit(if ok { 0 } else { 1 });
        }
        "serve" => {
            let cfg = Config::from_env().unwrap_or_else(|e| fail(&e));
            let addr: SocketAddr = cfg.bind.parse().unwrap_or_else(|_| fail("RAJESHDB_BIND must look like 127.0.0.1:8081"));
            if !addr.ip().is_loopback() {
                eprintln!("WARNING: listening on a non-loopback address without TLS. Put a TLS reverse proxy in front.");
            }
            let data_dir = cfg.data_dir.clone();
            let state = AppState::open(cfg).unwrap_or_else(|e| fail(&e));
            let app = build_app(state.clone());
            let interval = state.cfg.backup_interval_secs;
            if interval > 0 {
                let st = state.clone();
                tokio::spawn(async move {
                    let mut tick = tokio::time::interval(std::time::Duration::from_secs(interval));
                    tick.tick().await; // the first tick fires immediately; the first backup runs one interval after start
                    loop {
                        tick.tick().await;
                        let s = st.clone();
                        match tokio::task::spawn_blocking(move || s.backup_all()).await {
                            Ok((ok, failed)) => println!(
                                "{}",
                                format!("{{\"level\":\"info\",\"msg\":\"scheduled backup finished\",\"succeeded\":{},\"failed\":{}}}", ok, failed.len())
                            ),
                            Err(e) => eprintln!("scheduled backup task failed: {}", e),
                        }
                    }
                });
            }
            let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| fail(&format!("cannot bind {}: {}", addr, e)));
            eprintln!("RajeshDB platform listening on http://{} (data: {})", addr, data_dir.display());
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                    eprintln!("shutting down...");
                })
                .await
                .unwrap_or_else(|e| fail(&e.to_string()));
            state.flush_all();
        }
        other => fail(&format!("unknown command '{}' (try: help)", other)),
    }
}
