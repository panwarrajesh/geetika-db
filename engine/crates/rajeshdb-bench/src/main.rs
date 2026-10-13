//! Engine benchmarks. Everything printed here is MEASURED on the machine that runs it.
//!   cargo run --release -p rajeshdb-bench -- --size small
//!   cargo run --release -p rajeshdb-bench -- --size medium --dir D:\bench --json results.json
//! Always use --release: debug builds are 10-50x slower and meaningless for performance.

use rajeshdb_core::{Db, Options};
use rajeshdb_docs::{DocDb, Query};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Sizes {
    kv: u64,
    sync: u64,
    docs: u64,
    queries: u64,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn key(i: u64) -> String {
    format!("k{:08}", i)
}
fn value(i: u64) -> String {
    format!("{:0>100}", i) // 100-byte values
}

struct Row {
    name: String,
    ops: u64,
    secs: f64,
    p50_us: Option<f64>,
    p99_us: Option<f64>,
    note: String,
}

fn percentile_us(sorted: &[Duration], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx].as_secs_f64() * 1e6
}

fn row(name: &str, ops: u64, secs: f64, lat: Option<Vec<Duration>>, note: &str) -> Row {
    let (p50, p99) = match lat {
        Some(mut l) if !l.is_empty() => {
            l.sort();
            (Some(percentile_us(&l, 0.50)), Some(percentile_us(&l, 0.99)))
        }
        _ => (None, None),
    };
    Row { name: name.to_string(), ops, secs, p50_us: p50, p99_us: p99, note: note.to_string() }
}

fn dir_bytes(p: &Path) -> u64 {
    fs::read_dir(p)
        .map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

fn fresh(root: &Path, name: &str) -> PathBuf {
    let d = root.join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).expect("create bench dir");
    d
}

fn kv_insert(root: &Path, name: &str, n: u64, sync: bool) -> (Row, PathBuf) {
    let dir = fresh(root, name);
    let mut db = Db::open(&dir, Options { memtable_limit_bytes: 4 * 1024 * 1024, sync_writes: sync }).expect("open");
    let mut lat = Vec::with_capacity(n as usize);
    let t = Instant::now();
    for i in 0..n {
        let s = Instant::now();
        db.put(key(i).as_bytes(), value(i).as_bytes()).expect("put");
        lat.push(s.elapsed());
    }
    let secs = t.elapsed().as_secs_f64();
    let note = if sync { "fsync on every write (durable)" } else { "no fsync (OS may lose last writes on power cut)" };
    (row(name, n, secs, Some(lat), note), dir)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut size, mut base, mut json_out) = ("small".to_string(), None::<String>, None::<String>);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--size" => { size = args.get(i + 1).cloned().unwrap_or_default(); i += 2; }
            "--dir" => { base = args.get(i + 1).cloned(); i += 2; }
            "--json" => { json_out = args.get(i + 1).cloned(); i += 2; }
            "--help" | "-h" => {
                println!("rajeshdb-bench [--size small|medium|large] [--dir <folder on the disk you want to measure>] [--json <file>]");
                return;
            }
            other => { eprintln!("unknown argument: {}", other); std::process::exit(2); }
        }
    }
    let sz = match size.as_str() {
        "small" => Sizes { kv: 20_000, sync: 300, docs: 2_000, queries: 200 },
        "medium" => Sizes { kv: 200_000, sync: 2_000, docs: 20_000, queries: 200 },
        "large" => Sizes { kv: 1_000_000, sync: 5_000, docs: 100_000, queries: 200 },
        _ => { eprintln!("--size must be small, medium or large"); std::process::exit(2); }
    };
    if cfg!(debug_assertions) {
        eprintln!("WARNING: this is a debug build. Run with --release or the numbers are meaningless.");
    }
    let root = match base {
        Some(b) => PathBuf::from(b),
        None => std::env::temp_dir().join(format!("rajeshdb-bench-{}", std::process::id())),
    };
    fs::create_dir_all(&root).expect("create root");
    let mut rows: Vec<Row> = Vec::new();
    let mut rng = Rng(0x9E3779B97F4A7C15);

    // 1. KV writes
    let (r, kv_dir) = kv_insert(&root, "kv_put_nosync", sz.kv, false);
    rows.push(r);
    let (r, _) = kv_insert(&root, "kv_put_fsync", sz.sync, true);
    rows.push(r);

    // 2. KV reads (after flushing everything to table files)
    {
        let mut db = Db::open(&kv_dir, Options { memtable_limit_bytes: 4 * 1024 * 1024, sync_writes: false }).expect("open");
        db.flush().expect("flush");
        let reads = sz.kv.min(50_000);
        for (name, hit) in [("kv_get_hit_random", true), ("kv_get_miss", false)] {
            let mut lat = Vec::with_capacity(reads as usize);
            let t = Instant::now();
            for _ in 0..reads {
                let k = if hit { key(rng.next() % sz.kv) } else { format!("zz{:08}", rng.next() % sz.kv) };
                let s = Instant::now();
                let v = db.get(k.as_bytes()).expect("get");
                lat.push(s.elapsed());
                assert_eq!(v.is_some(), hit, "unexpected result for {}", k);
            }
            rows.push(row(name, reads, t.elapsed().as_secs_f64(), Some(lat), "each lookup opens the table file (no cache yet)"));
        }
        let scans = 50u64;
        let t = Instant::now();
        let mut found = 0usize;
        for _ in 0..scans {
            found = db.scan_prefix(b"k0000").expect("scan").len();
        }
        rows.push(row("kv_scan_prefix", scans, t.elapsed().as_secs_f64(), None, &format!("each scan returned {} rows", found)));

        // 3. checkpoint (backup copy)
        let snap = root.join("kv_snapshot");
        let _ = fs::remove_dir_all(&snap);
        let t = Instant::now();
        db.checkpoint(&snap).expect("checkpoint");
        rows.push(row("checkpoint_copy", 1, t.elapsed().as_secs_f64(), None, &format!("{} bytes copied", dir_bytes(&snap))));
        let t = Instant::now();
        let n = db.verify_integrity().expect("verify");
        rows.push(row("verify_integrity", 1, t.elapsed().as_secs_f64(), None, &format!("{} records checked", n)));
    }

    // 4. crash-recovery time: everything only in the WAL
    {
        let dir = fresh(&root, "recovery");
        let n = sz.kv.min(200_000);
        {
            let mut db = Db::open(&dir, Options { memtable_limit_bytes: 1 << 30, sync_writes: false }).expect("open");
            for i in 0..n {
                db.put(key(i).as_bytes(), value(i).as_bytes()).expect("put");
            }
        } // dropped without flush: like a crash
        let wal = fs::metadata(dir.join("wal.log")).map(|m| m.len()).unwrap_or(0);
        let t = Instant::now();
        let db = Db::open(&dir, Options { memtable_limit_bytes: 1 << 30, sync_writes: false }).expect("reopen");
        let secs = t.elapsed().as_secs_f64();
        let replayed = db.recovery_report().records_replayed;
        rows.push(row("recovery_wal_replay", replayed, secs, None, &format!("WAL of {} bytes replayed at open", wal)));
    }

    // 5. documents: inserts with two indexes, then indexed vs unindexed queries
    {
        let dir = fresh(&root, "docs");
        let mut db = DocDb::open(&dir, Options { memtable_limit_bytes: 4 * 1024 * 1024, sync_writes: false }).expect("open docs");
        db.create_collection("people", None).expect("collection");
        db.create_index("people", "city").expect("index");
        db.create_index("people", "age").expect("index");
        let mut lat = Vec::with_capacity(sz.docs as usize);
        let t = Instant::now();
        for i in 0..sz.docs {
            let city = format!("city{}", rng.next() % 100);
            let doc = json!({"_id": format!("p{}", i), "name": format!("person {}", i), "city": city, "city_plain": city, "age": rng.next() % 100});
            let s = Instant::now();
            db.insert("people", doc).expect("insert");
            lat.push(s.elapsed());
        }
        rows.push(row("doc_insert_2_indexes", sz.docs, t.elapsed().as_secs_f64(), Some(lat), "each insert = one atomic batch (document + 2 index entries)"));

        let mut run = |name: &str, filter: Value, note: &str| {
            let q = Query::new(filter);
            let mut lat = Vec::with_capacity(sz.queries as usize);
            let mut total = 0usize;
            let t = Instant::now();
            for _ in 0..sz.queries {
                let s = Instant::now();
                total += db.find("people", &q).expect("find").len();
                lat.push(s.elapsed());
            }
            rows.push(row(name, sz.queries, t.elapsed().as_secs_f64(), Some(lat), &format!("{} (avg {} docs returned)", note, total / sz.queries as usize)));
        };
        run("doc_query_indexed_eq", json!({"city": "city7"}), "IndexScan");
        run("doc_query_unindexed_eq", json!({"city_plain": "city7"}), "FullScan, same selectivity");
        run("doc_query_indexed_range", json!({"age": {"$gte": 40, "$lt": 42}}), "IndexScan range");
    }

    // ---- report
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    let when = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    println!("# RajeshDB engine benchmark");
    println!("- measured at unix time {} | os: {} | arch: {} | logical CPUs: {}", when, std::env::consts::OS, std::env::consts::ARCH, cpus);
    println!("- data folder: {} (results include the speed of THIS disk) | size preset: {}", root.display(), size);
    println!("- dataset: {} KV pairs of ~100-byte values, {} fsync writes, {} documents with 2 indexes", sz.kv, sz.sync, sz.docs);
    println!("- build: {}", if cfg!(debug_assertions) { "DEBUG (not valid for performance claims)" } else { "release" });
    println!();
    println!("| scenario | operations | seconds | ops/sec | p50 us | p99 us | note |");
    println!("|---|---:|---:|---:|---:|---:|---|");
    let fmt = |v: Option<f64>| v.map(|x| format!("{:.1}", x)).unwrap_or_else(|| "-".to_string());
    for r in &rows {
        println!("| {} | {} | {:.3} | {:.0} | {} | {} | {} |", r.name, r.ops, r.secs, r.ops as f64 / r.secs.max(1e-9), fmt(r.p50_us), fmt(r.p99_us), r.note);
    }
    if let Some(path) = json_out {
        let rows_json: Vec<Value> = rows.iter().map(|r| json!({
            "scenario": r.name, "operations": r.ops, "seconds": r.secs, "ops_per_sec": r.ops as f64 / r.secs.max(1e-9),
            "p50_us": r.p50_us, "p99_us": r.p99_us, "note": r.note
        })).collect();
        let doc = json!({"measured_at_unix": when, "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "logical_cpus": cpus,
                         "data_folder": root.display().to_string(), "size": size, "release_build": !cfg!(debug_assertions), "results": rows_json});
        fs::write(&path, serde_json::to_vec_pretty(&doc).unwrap()).expect("write json");
        println!("\nresults written to {}", path);
    }
    let _ = fs::remove_dir_all(&root);
}
