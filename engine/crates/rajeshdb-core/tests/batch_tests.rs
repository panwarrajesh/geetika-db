use rajeshdb_core::{BatchOp, Db, Options};
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
        let p = std::env::temp_dir().join(format!("rajeshdb-batch-test-{}-{}-{}", std::process::id(), n, nanos));
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

fn put(k: &str, v: &str) -> BatchOp {
    (k.as_bytes().to_vec(), Some(v.as_bytes().to_vec()))
}
fn del(k: &str) -> BatchOp {
    (k.as_bytes().to_vec(), None)
}
fn get(db: &Db, k: &str) -> Option<Vec<u8>> {
    db.get(k.as_bytes()).unwrap()
}

#[test]
fn batch_applies_and_survives_reopen() {
    let t = TempDir::new();
    {
        let mut db = Db::open(t.path(), Options::default()).unwrap();
        db.put(b"old", b"1").unwrap();
        db.write_batch(&[put("a", "1"), put("b", "2"), del("old")]).unwrap();
        assert_eq!(get(&db, "a"), Some(b"1".to_vec()));
        assert_eq!(get(&db, "old"), None);
    }
    let db = Db::open(t.path(), Options::default()).unwrap();
    assert_eq!(get(&db, "a"), Some(b"1".to_vec()));
    assert_eq!(get(&db, "b"), Some(b"2".to_vec()));
    assert_eq!(get(&db, "old"), None);
}

#[test]
fn torn_batch_is_all_or_nothing() {
    let t = TempDir::new();
    let wal = t.path().join("wal.log");
    {
        let mut db = Db::open(t.path(), Options::default()).unwrap();
        db.put(b"before", b"ok").unwrap();
        db.write_batch(&[put("b1", "x"), put("b2", "y"), put("b3", "z")]).unwrap();
    }
    let len = fs::metadata(&wal).unwrap().len();
    fs::OpenOptions::new().write(true).open(&wal).unwrap().set_len(len - 3).unwrap();
    let db = Db::open(t.path(), Options::default()).unwrap();
    assert!(db.recovery_report().bytes_discarded > 0);
    assert_eq!(get(&db, "before"), Some(b"ok".to_vec()));
    assert_eq!(get(&db, "b1"), None);
    assert_eq!(get(&db, "b2"), None);
    assert_eq!(get(&db, "b3"), None);
}

#[test]
fn later_operation_on_same_key_wins() {
    let t = TempDir::new();
    {
        let mut db = Db::open(t.path(), Options::default()).unwrap();
        db.write_batch(&[put("k", "1"), del("k"), put("k", "3"), put("j", "1"), del("j")]).unwrap();
    }
    let db = Db::open(t.path(), Options::default()).unwrap();
    assert_eq!(get(&db, "k"), Some(b"3".to_vec()));
    assert_eq!(get(&db, "j"), None);
}

#[test]
fn invalid_batch_applies_nothing() {
    let t = TempDir::new();
    let mut db = Db::open(t.path(), Options::default()).unwrap();
    let bad = vec![put("good", "1"), (Vec::new(), Some(b"x".to_vec()))];
    assert_eq!(db.write_batch(&bad).unwrap_err().kind(), ErrorKind::InvalidInput);
    assert_eq!(get(&db, "good"), None);
    assert_eq!(db.stats().wal_bytes, 0);
    db.write_batch(&[]).unwrap(); // empty batch is a no-op
}

#[test]
fn batch_survives_flush_and_scan_range_works() {
    let t = TempDir::new();
    {
        let mut db = Db::open(t.path(), Options::default()).unwrap();
        db.write_batch(&[put("a", "1"), put("b", "2"), put("c", "3"), put("d", "4")]).unwrap();
        db.flush().unwrap();
        db.write_batch(&[put("b", "2b"), del("c")]).unwrap();
        let r = db.scan_range(b"b", Some(b"d")).unwrap();
        assert_eq!(r, vec![(b"b".to_vec(), b"2b".to_vec())]);
        assert_eq!(db.scan_range(b"b", None).unwrap().len(), 2); // b, d
        assert!(db.scan_range(b"d", Some(b"b")).unwrap().is_empty());
        assert!(db.scan_range(b"b", Some(b"b")).unwrap().is_empty());
    }
    let db = Db::open(t.path(), Options::default()).unwrap();
    assert_eq!(get(&db, "b"), Some(b"2b".to_vec()));
    assert_eq!(get(&db, "c"), None);
    assert_eq!(get(&db, "a"), Some(b"1".to_vec()));
}
