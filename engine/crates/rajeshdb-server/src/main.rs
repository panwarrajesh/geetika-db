use rajeshdb_docs::{DocDb, Options};
use rajeshdb_server::keys::{generate_key, list_keys, load_keys, revoke_key, Role};
use rajeshdb_server::{build_app, AppState, Config};
use std::net::SocketAddr;
use std::process::exit;
use std::sync::Arc;

const HELP: &str = "\
RajeshDB server (Phase 4)

USAGE:
  rajeshdb-server serve                       start the HTTP API (default)
  rajeshdb-server keygen <name> <read|write|admin>
                                              create an API key (shown ONCE)
  rajeshdb-server keys list
  rajeshdb-server keys revoke <id>            (restart the server to apply)

CONFIG (environment variables, all optional):
  RAJESHDB_DATA_DIR        ./rajeshdb-server-data
  RAJESHDB_BIND            127.0.0.1:8080
  RAJESHDB_KEYS_FILE       ./rajeshdb-keys.json
  RAJESHDB_MAX_BODY_BYTES  1048576
  RAJESHDB_RATE_PER_SEC    50
  RAJESHDB_RATE_BURST      100
  RAJESHDB_DEFAULT_LIMIT   100
  RAJESHDB_MAX_LIMIT       1000";

fn fail(msg: &str) -> ! {
    eprintln!("error: {}", msg);
    exit(1);
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    eprintln!("shutting down...");
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = Config::from_env().unwrap_or_else(|e| fail(&e));

    match args.first().map(|s| s.as_str()).unwrap_or("serve") {
        "help" | "--help" | "-h" => println!("{}", HELP),
        "keygen" => {
            if args.len() != 3 {
                fail("usage: keygen <name> <read|write|admin>");
            }
            let role = Role::parse(&args[2]).unwrap_or_else(|| fail("role must be read, write or admin"));
            let (key, id) = generate_key(&cfg.keys_file, &args[1], role).unwrap_or_else(|e| fail(&e));
            println!("key id : {}", id);
            println!("role   : {}", role.as_str());
            println!("API key: {}", key);
            println!("Copy the key now. It cannot be shown again (only its hash is stored).");
        }
        "keys" => match args.get(1).map(|s| s.as_str()) {
            Some("list") => {
                for (id, name, role) in list_keys(&cfg.keys_file).unwrap_or_else(|e| fail(&e)) {
                    println!("{}  {:<6}  {}", id, role, name);
                }
            }
            Some("revoke") if args.len() == 3 => {
                if revoke_key(&cfg.keys_file, &args[2]).unwrap_or_else(|e| fail(&e)) {
                    println!("revoked {} (restart the server to apply)", args[2]);
                } else {
                    fail("no such key id");
                }
            }
            _ => fail("usage: keys list | keys revoke <id>"),
        },
        "serve" => {
            let keys = load_keys(&cfg.keys_file).unwrap_or_else(|e| fail(&e));
            if keys.is_empty() {
                fail("no API keys configured. Create one first:  rajeshdb-server keygen admin-key admin");
            }
            let addr: SocketAddr = cfg.bind.parse().unwrap_or_else(|_| fail("RAJESHDB_BIND must look like 127.0.0.1:8080"));
            if !addr.ip().is_loopback() {
                eprintln!("WARNING: listening on a non-loopback address without TLS. Put a TLS reverse proxy in front.");
            }
            let db = DocDb::open(&cfg.data_dir, Options::default()).unwrap_or_else(|e| fail(&e.to_string()));
            let state = Arc::new(AppState::new(db, keys, &cfg));
            let app = build_app(state.clone());
            let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| fail(&format!("cannot bind {}: {}", addr, e)));
            eprintln!("RajeshDB listening on http://{} (data: {})", addr, cfg.data_dir.display());
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(shutdown_signal())
                .await
                .unwrap_or_else(|e| fail(&e.to_string()));
            if let Err(e) = state.flush() {
                eprintln!("flush on shutdown failed: {}", e);
            }
        }
        other => fail(&format!("unknown command '{}' (try: help)", other)),
    }
}
