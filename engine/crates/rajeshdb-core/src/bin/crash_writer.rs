//! Test helper for tests/crash_kill.rs. NOT a user tool.
//! Writes a deterministic sequence of puts/deletes with fsync on every write and prints
//! "ACK <i>" AFTER operation <i> has returned success. The test kills this process at a random
//! moment (SIGKILL / TerminateProcess) and then checks that everything acknowledged survived.
//!
//! usage: crash_writer <dir> <start_index>
//! operation i:  put key(i) = value(i);  if i % 7 == 3 then delete key(i-1)

use rajeshdb_core::{Db, Options};
use std::io::Write;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("usage: crash_writer <dir> <start_index>");
    let start: u64 = args.next().expect("start index").parse().expect("start index must be a number");
    // Tiny memtable: flushes (and the manifest/WAL hand-over) happen many times during the test.
    let mut db = Db::open(&dir, Options { memtable_limit_bytes: 8 * 1024, sync_writes: true }).expect("open");
    let out = std::io::stdout();
    let mut out = out.lock();
    let mut i = start;
    loop {
        db.put(key(i).as_bytes(), value(i).as_bytes()).expect("put");
        if i % 7 == 3 {
            db.delete(key(i - 1).as_bytes()).expect("delete");
        }
        writeln!(out, "ACK {}", i).expect("stdout");
        out.flush().expect("flush stdout");
        i += 1;
    }
}

pub fn key(i: u64) -> String {
    format!("key{:010}", i)
}

pub fn value(i: u64) -> String {
    format!("value-{}-{}", i, "x".repeat((i % 50) as usize))
}
