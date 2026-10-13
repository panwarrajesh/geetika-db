//! Real crash test: a child process writes with fsync, is KILLED at an arbitrary moment, and the
//! database must reopen with every acknowledged operation intact and no corruption. Repeated
//! several times on the same directory (so recovery after recovery is exercised too).

use rajeshdb_core::{Db, Options};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("rajeshdb-crash-test-{}-{}-{}", std::process::id(), n, nanos));
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

fn key(i: u64) -> String {
    format!("key{:010}", i)
}
fn value(i: u64) -> String {
    format!("value-{}-{}", i, "x".repeat((i % 50) as usize))
}

/// Start the writer at `start`, wait for `acks` acknowledgements, kill it. Returns the last ack seen.
fn run_round(dir: &Path, start: u64, acks: u64) -> u64 {
    let mut child = Command::new(env!("CARGO_BIN_EXE_crash_writer"))
        .arg(dir)
        .arg(start.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn crash_writer");
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut last: Option<u64> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 {
            break; // writer died on its own: report what we have
        }
        if let Some(n) = line.trim().strip_prefix("ACK ") {
            let n: u64 = n.parse().unwrap();
            last = Some(n);
            if n >= start + acks - 1 {
                break;
            }
        }
    }
    let _ = child.kill(); // abrupt kill: no flush, no destructor, no graceful shutdown
    let _ = child.wait();
    last.expect("the writer produced no acknowledgements")
}

/// Every acknowledged operation must be visible; deleted keys must stay deleted.
fn check(dir: &Path, last_ack: u64) {
    let db = Db::open(dir, Options { memtable_limit_bytes: 8 * 1024, sync_writes: true }).expect("database must reopen after a kill");
    for i in 0..=last_ack {
        let deleted = (i + 1) % 7 == 3 && i + 1 <= last_ack;
        let got = db.get(key(i).as_bytes()).unwrap();
        if deleted {
            assert_eq!(got, None, "key {} was deleted by an acknowledged operation", i);
        } else {
            assert_eq!(got, Some(value(i).into_bytes()), "acknowledged key {} is missing or wrong", i);
        }
    }
    db.verify_integrity().expect("every table record must still pass its checksum");
}

#[test]
fn acknowledged_writes_survive_repeated_kills() {
    let t = TempDir::new();
    let mut next = 0u64;
    for round in 0..4 {
        let acks = 150 + 37 * round; // different kill points each round
        let last = run_round(t.path(), next, acks);
        check(t.path(), last);
        next = last + 1;
    }
}
