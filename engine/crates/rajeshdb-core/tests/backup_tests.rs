use rajeshdb_core::{Db, Options};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("rajeshdb-bk-test-{}-{}-{}", std::process::id(), n, nanos));
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

fn opts() -> Options {
    Options { memtable_limit_bytes: 64 * 1024, sync_writes: false }
}

#[test]
fn checkpoint_is_an_independent_openable_copy() {
    let src = TempDir::new();
    let dst = TempDir::new();
    let snap = dst.path().join("snap");
    let mut db = Db::open(src.path(), opts()).unwrap();
    for i in 0..3000 {
        db.put(format!("k{:05}", i).as_bytes(), format!("v{}", i).as_bytes()).unwrap();
    }
    db.delete(b"k00007").unwrap();
    db.put(b"unflushed", b"still in the memtable").unwrap();
    let files = db.checkpoint(&snap).unwrap();
    assert!(files.contains(&"MANIFEST".to_string()));
    assert!(files.iter().any(|f| f.ends_with(".sst")));
    assert!(!snap.join("wal.log").exists());

    // Later changes to the original do not affect the copy.
    db.put(b"after", b"snapshot").unwrap();
    db.delete(b"k00001").unwrap();

    let copy = Db::open(&snap, opts()).unwrap();
    assert_eq!(copy.get(b"k00000").unwrap(), Some(b"v0".to_vec()));
    assert_eq!(copy.get(b"k00001").unwrap(), Some(b"v1".to_vec()));
    assert_eq!(copy.get(b"k00007").unwrap(), None);
    assert_eq!(copy.get(b"unflushed").unwrap(), Some(b"still in the memtable".to_vec()));
    assert_eq!(copy.get(b"after").unwrap(), None);
    assert!(copy.verify_integrity().unwrap() >= 3000);
    // The original is untouched and keeps working.
    assert_eq!(db.get(b"after").unwrap(), Some(b"snapshot".to_vec()));
    assert_eq!(db.get(b"k00001").unwrap(), None);
}

#[test]
fn checkpoint_of_an_empty_database_restores_to_an_empty_database() {
    let src = TempDir::new();
    let dst = TempDir::new();
    let snap = dst.path().join("snap");
    let mut db = Db::open(src.path(), opts()).unwrap();
    let files = db.checkpoint(&snap).unwrap();
    assert_eq!(files, vec!["MANIFEST".to_string()]);
    let copy = Db::open(&snap, opts()).unwrap();
    assert_eq!(copy.verify_integrity().unwrap(), 0);
    assert!(copy.scan_prefix(b"").unwrap().is_empty());
}

#[test]
fn checkpoint_refuses_a_non_empty_destination() {
    let src = TempDir::new();
    let dst = TempDir::new();
    fs::write(dst.path().join("junk"), b"x").unwrap();
    let mut db = Db::open(src.path(), opts()).unwrap();
    db.put(b"a", b"1").unwrap();
    let err = db.checkpoint(dst.path()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
}

#[test]
fn verify_integrity_detects_bit_rot() {
    let src = TempDir::new();
    {
        let mut db = Db::open(src.path(), opts()).unwrap();
        db.put(b"key", b"vvvvvvvvvvvv").unwrap();
        db.flush().unwrap();
        assert_eq!(db.verify_integrity().unwrap(), 1);
    }
    let sst = fs::read_dir(src.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().map_or(false, |e| e == "sst"))
        .unwrap();
    let mut bytes = fs::read(&sst).unwrap();
    bytes[22] ^= 0xFF; // inside the value of the first record
    fs::write(&sst, &bytes).unwrap();
    let db = Db::open(src.path(), opts()).unwrap();
    assert_eq!(db.verify_integrity().unwrap_err().kind(), ErrorKind::InvalidData);
}
