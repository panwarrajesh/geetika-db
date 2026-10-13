use rajeshdb_core::{Db, Options};
use rajeshdb_docs::{DocDb, Query, Value};
use std::io::{self, BufRead, Write};
use std::process::exit;

const HELP: &str = "\
RajeshDB CLI (Phase 3)

USAGE:
  rajeshdb [--data <dir>] [--docs] <command> [args]

Two separate modes (use a different data dir for each):

KEY/VALUE MODE (default, data dir ./rajeshdb-data)
  put <key> <value>      get <key>      delete <key>
  scan [prefix]          flush          compact        stats

DOCUMENT MODE (--docs, data dir ./rajeshdb-docs-data)
  collections
  create-collection <name> [schema-json]
  drop-collection <name>
  create-index <collection> <field>
  drop-index <collection> <field>
  indexes <collection>
  insert <collection> <json-document>
  get-doc <collection> <id>
  delete-doc <collection> <id>
  find <collection> [filter-json]
  query <collection> <query-json>      {\"filter\":{..},\"sort\":\"age\",\"skip\":0,\"limit\":10}
  explain <collection> [filter-json]
  count <collection> [filter-json]
  flush | compact | stats

Both modes: shell (interactive, one command per line), help
Tip: JSON is easiest to type inside `shell`, because PowerShell mangles quotes.";

enum Backend {
    Kv(Db),
    Docs(DocDb),
}

fn word(s: &str) -> (&str, &str) {
    let s = s.trim();
    match s.split_once(char::is_whitespace) {
        Some((a, b)) => (a, b.trim()),
        None => (s, ""),
    }
}

fn json_or(s: &str, default: &str) -> Result<Value, String> {
    let text = if s.is_empty() { default } else { s };
    serde_json::from_str(text).map_err(|e| format!("bad JSON: {}", e))
}

fn show(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

fn exec_kv(db: &mut Db, line: &str) -> Result<String, String> {
    let (cmd, rest) = word(line);
    let e = |e: io::Error| e.to_string();
    match cmd {
        "put" => {
            let (k, v) = rest.split_once(char::is_whitespace).ok_or("usage: put <key> <value>")?;
            db.put(k.as_bytes(), v.trim_start().as_bytes()).map_err(e)?;
            Ok("OK".into())
        }
        "get" if !rest.is_empty() => match db.get(rest.as_bytes()).map_err(e)? {
            Some(v) => Ok(String::from_utf8_lossy(&v).into_owned()),
            None => Ok("(not found)".into()),
        },
        "delete" if !rest.is_empty() => {
            db.delete(rest.as_bytes()).map_err(e)?;
            Ok("OK".into())
        }
        "scan" => {
            let rows = db.scan_prefix(rest.as_bytes()).map_err(e)?;
            let mut out = String::new();
            for (k, v) in &rows {
                out.push_str(&format!("{} = {}\n", String::from_utf8_lossy(k), String::from_utf8_lossy(v)));
            }
            out.push_str(&format!("({} rows)", rows.len()));
            Ok(out)
        }
        "flush" => db.flush().map(|_| "OK".to_string()).map_err(e),
        "compact" => db.compact().map(|_| "OK".to_string()).map_err(e),
        "stats" => Ok(format!("{:#?}", db.stats())),
        "" => Ok(String::new()),
        "get" | "delete" => Err(format!("usage: {} <key>", cmd)),
        other => Err(format!("unknown command '{}' (try: help)", other)),
    }
}

fn exec_docs(db: &mut DocDb, line: &str) -> Result<String, String> {
    let (cmd, rest) = word(line);
    let e = |e: rajeshdb_docs::DocError| e.to_string();
    match cmd {
        "" => Ok(String::new()),
        "collections" => Ok(db.list_collections().map_err(e)?.join("\n")),
        "create-collection" => {
            let (name, schema) = word(rest);
            let schema = if schema.is_empty() { None } else { Some(json_or(schema, "{}")?) };
            db.create_collection(name, schema).map_err(e)?;
            Ok("OK".into())
        }
        "drop-collection" => db.drop_collection(rest).map(|_| "OK".to_string()).map_err(e),
        "create-index" => {
            let (c, f) = word(rest);
            db.create_index(c, f).map(|_| "OK".to_string()).map_err(e)
        }
        "drop-index" => {
            let (c, f) = word(rest);
            db.drop_index(c, f).map(|_| "OK".to_string()).map_err(e)
        }
        "indexes" => Ok(db.list_indexes(rest).map_err(e)?.join("\n")),
        "insert" => {
            let (c, j) = word(rest);
            let doc = json_or(j, "")?;
            db.insert(c, doc).map_err(e)
        }
        "get-doc" => {
            let (c, id) = word(rest);
            Ok(match db.get(c, id).map_err(e)? {
                Some(d) => show(&d),
                None => "(not found)".into(),
            })
        }
        "delete-doc" => {
            let (c, id) = word(rest);
            Ok(if db.delete(c, id).map_err(e)? { "deleted".into() } else { "(not found)".into() })
        }
        "find" | "query" | "explain" | "count" => {
            let (c, j) = word(rest);
            let parsed = json_or(j, "{}")?;
            let q = if cmd == "query" { Query::from_json(&parsed).map_err(e)? } else { Query::new(parsed) };
            match cmd {
                "explain" => db.explain(c, &q).map_err(e),
                "count" => Ok(db.count(c, &q.filter).map_err(e)?.to_string()),
                _ => {
                    let docs = db.find(c, &q).map_err(e)?;
                    let mut out = String::new();
                    for d in &docs {
                        out.push_str(&show(d));
                        out.push('\n');
                    }
                    out.push_str(&format!("({} documents, plan: {})", docs.len(), db.explain(c, &q).map_err(e)?));
                    Ok(out)
                }
            }
        }
        "flush" => db.flush().map(|_| "OK".to_string()).map_err(e),
        "compact" => db.compact().map(|_| "OK".to_string()).map_err(e),
        "stats" => Ok(format!("{:#?}", db.stats())),
        other => Err(format!("unknown command '{}' (try: help)", other)),
    }
}

fn exec(b: &mut Backend, line: &str) -> Result<String, String> {
    match b {
        Backend::Kv(db) => exec_kv(db, line),
        Backend::Docs(db) => exec_docs(db, line),
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir: Option<String> = None;
    let mut docs_mode = false;
    loop {
        let first = args.first().cloned();
        match first.as_deref() {
            Some("--data") => {
                if args.len() < 2 {
                    eprintln!("--data needs a directory");
                    exit(2);
                }
                dir = Some(args[1].clone());
                args.drain(0..2);
            }
            Some("--docs") => {
                docs_mode = true;
                args.remove(0);
            }
            _ => break,
        }
    }
    if args.is_empty() || args[0] == "help" || args[0] == "--help" {
        println!("{}", HELP);
        return;
    }
    let dir = dir.unwrap_or_else(|| {
        if docs_mode { "./rajeshdb-docs-data".into() } else { "./rajeshdb-data".into() }
    });

    let opened = if docs_mode {
        DocDb::open(&dir, Options::default()).map(Backend::Docs).map_err(|e| e.to_string())
    } else {
        Db::open(&dir, Options::default()).map(Backend::Kv).map_err(|e| e.to_string())
    };
    let mut backend = match opened {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot open database at '{}': {}", dir, e);
            exit(1);
        }
    };

    if args[0] == "shell" {
        let stdin = io::stdin();
        loop {
            print!("rajeshdb> ");
            io::stdout().flush().ok();
            let mut line = String::new();
            if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let t = line.trim();
            if t == "exit" || t == "quit" {
                break;
            }
            if t == "help" {
                println!("{}", HELP);
                continue;
            }
            match exec(&mut backend, t) {
                Ok(s) if !s.is_empty() => println!("{}", s),
                Ok(_) => {}
                Err(e) => println!("error: {}", e),
            }
        }
        return;
    }

    match exec(&mut backend, &args.join(" ")) {
        Ok(s) => println!("{}", s),
        Err(e) => {
            eprintln!("error: {}", e);
            exit(1);
        }
    }
}
