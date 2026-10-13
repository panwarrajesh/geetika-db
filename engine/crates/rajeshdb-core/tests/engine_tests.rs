use rajeshdb_core::{Db, Options, MAX_KEY_LEN};
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
        let p = std::env::temp_dir().join(format!("rajeshdb-test-{}-{}-{}", std::process::id(), n, nanos));
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

fn small() -> Options {
    Options { memtable_limit_bytes: 64 * 1024, sync_writes: false }
}

fn get(db: &Db, k: &str) -> Option<Vec<u8>> {
    db.get(k.as_bytes()).unwrap()
}

fn open(t: &TempDir) -> Db {
    Db::open(t.path(), Options::default()).unwrap()
}

#[test]
fn put_get_overwrite_delete() {
    let t = TempDir::new();
    let mut db = open(&t);
    assert_eq!(get(&db, "a"), None);
    db.put(b"a", b"1").unwrap();
    assert_eq!(get(&db, "a"), Some(b"1".to_vec()));
    db.put(b"a", b"2").unwrap();
    assert_eq!(get(&db, "a"), Some(b"2".to_vec()));
    db.delete(b"a").unwrap();
    assert_eq!(get(&db, "a"), None);
}

#[test]
fn empty_value_is_distinct_from_deleted() {
    let t = TempDir::new();
    let mut db = open(&t);
    db.put(b"k", b"").unwrap();
    assert_eq!(get(&db, "k"), Some(vec![]));
    db.flush().unwrap();
    assert_eq!(get(&db, "k"), Some(vec![]));
}

#[test]
fn data_survives_reopen_via_wal_without_flush() {
    let t = TempDir::new();
    {
        let mut db = open(&t);
        db.put(b"x", b"10").unwrap();
        db.put(b"y", b"20").unwrap();
        db.delete(b"x").unwrap();
        // Dropped without flush(): simulates a process crash (WAL writes are unbuffered).
    }
    let db = open(&t);
    assert_eq!(db.recovery_report().records_replayed, 3);
    assert_eq!(get(&db, "x"), None);
    assert_eq!(get(&db, "y"), Some(b"20".to_vec()));
}

#[test]
fn data_survives_reopen_after_flush() {
    let t = TempDir::new();
    {
        let mut db = open(&t);
        db.put(b"k", b"v").unwrap();
        db.flush().unwrap();
        assert_eq!(db.stats().sstable_count, 1);
        assert_eq!(db.stats().wal_bytes, 0);
    }
    let db = open(&t);
    assert_eq!(db.recovery_report().records_replayed, 0);
    assert_eq!(get(&db, "k"), Some(b"v".to_vec()));
}

#[test]
fn tombstone_survives_flush_reopen_and_compaction() {
    let t = TempDir::new();
    {
        let mut db = open(&t);
        db.put(b"k", b"v").unwrap();
        db.flush().unwrap();
        db.delete(b"k").unwrap();
        db.flush().unwrap();
        assert_eq!(get(&db, "k"), None);
    }
    {
        let mut db = open(&t);
        assert_eq!(get(&db, "k"), None);
        db.compact().unwrap();
        assert_eq!(get(&db, "k"), None);
        assert_eq!(db.stats().sstable_count, 0); // everything was deleted
    }
    let db = open(&t);
    assert_eq!(get(&db, "k"), None);
}

#[test]
fn torn_wal_tail_is_discarded_and_log_stays_usable() {
    let t = TempDir::new();
    let wal = t.path().join("wal.log");
    {
        let mut db = open(&t);
        db.put(b"a", b"1").unwrap();
        db.put(b"b", b"2").unwrap();
        db.put(b"c", b"3").unwrap();
    }
    let len = fs::metadata(&wal).unwrap().len();
    {
        let f = fs::OpenOptions::new().write(true).open(&wal).unwrap();
        f.set_len(len - 5).unwrap(); // simulate crash mid-write of the last record
    }
    {
        let mut db = open(&t);
        let r = db.recovery_report().clone();
        assert_eq!(r.records_replayed, 2);
        assert!(r.bytes_discarded > 0);
        assert_eq!(get(&db, "a"), Some(b"1".to_vec()));
        assert_eq!(get(&db, "b"), Some(b"2".to_vec()));
        assert_eq!(get(&db, "c"), None);
        // New writes after recovery must not be lost behind the garbage tail.
        db.put(b"d", b"4").unwrap();
    }
    let db = open(&t);
    assert_eq!(db.recovery_report().bytes_discarded, 0);
    assert_eq!(get(&db, "a"), Some(b"1".to_vec()));
    assert_eq!(get(&db, "c"), None);
    assert_eq!(get(&db, "d"), Some(b"4".to_vec()));
}

#[test]
fn corrupt_last_wal_record_is_rejected_by_checksum() {
    let t = TempDir::new();
    let wal = t.path().join("wal.log");
    {
        let mut db = open(&t);
        db.put(b"a", b"1").unwrap();
        db.put(b"b", b"2").unwrap();
    }
    let mut bytes = fs::read(&wal).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    fs::write(&wal, &bytes).unwrap();

    let db = open(&t);
    assert_eq!(db.recovery_report().records_replayed, 1);
    assert!(db.recovery_report().bytes_discarded > 0);
    assert_eq!(get(&db, "a"), Some(b"1".to_vec()));
    assert_eq!(get(&db, "b"), None);
}

#[test]
fn leftover_files_from_crashed_flush_are_cleaned_up() {
    let t = TempDir::new();
    {
        let mut db = open(&t);
        db.put(b"k", b"v").unwrap();
        db.flush().unwrap();
    }
    let orphan = t.path().join("000099.sst");
    let tmp = t.path().join("000100.tmp");
    fs::write(&orphan, b"garbage from a crashed flush").unwrap();
    fs::write(&tmp, b"partial").unwrap();
    let db = open(&t);
    assert!(!orphan.exists());
    assert!(!tmp.exists());
    assert_eq!(get(&db, "k"), Some(b"v".to_vec()));
}

#[test]
fn many_keys_auto_flush_reopen_and_compact() {
    let t = TempDir::new();
    let n = 20_000;
    {
        let mut db = Db::open(t.path(), small()).unwrap();
        for i in 0..n {
            db.put(format!("key{:06}", i).as_bytes(), format!("value-{}", i).as_bytes()).unwrap();
        }
        for i in (0..n).step_by(2) {
            db.delete(format!("key{:06}", i).as_bytes()).unwrap();
        }
        assert!(db.stats().sstable_count > 1, "expected several automatic flushes");
    }
    let mut db = Db::open(t.path(), small()).unwrap();
    let check = |db: &Db| {
        for i in 0..n {
            let v = get(db, &format!("key{:06}", i));
            if i % 2 == 0 {
                assert_eq!(v, None, "key {} should be deleted", i);
            } else {
                assert_eq!(v, Some(format!("value-{}", i).into_bytes()), "key {}", i);
            }
        }
    };
    check(&db);
    db.compact().unwrap();
    assert_eq!(db.stats().sstable_count, 1);
    assert_eq!(db.stats().sstable_keys, (n / 2) as usize);
    check(&db);
    drop(db);
    let db = Db::open(t.path(), small()).unwrap();
    check(&db);
}

#[test]
fn scan_prefix_is_sorted_and_newest_wins() {
    let t = TempDir::new();
    let mut db = open(&t);
    db.put(b"user:2", b"old").unwrap();
    db.put(b"user:1", b"alice").unwrap();
    db.put(b"order:1", b"x").unwrap();
    db.flush().unwrap();
    db.put(b"user:2", b"bob").unwrap(); // overrides the flushed value
    db.put(b"user:3", b"carol").unwrap();
    db.delete(b"user:1").unwrap(); // hides the flushed value
    let got = db.scan_prefix(b"user:").unwrap();
    assert_eq!(
        got,
        vec![(b"user:2".to_vec(), b"bob".to_vec()), (b"user:3".to_vec(), b"carol".to_vec())]
    );
    assert_eq!(db.scan_prefix(b"").unwrap().len(), 3); // order:1, user:2, user:3
}

#[test]
fn invalid_input_is_rejected() {
    let t = TempDir::new();
    let mut db = open(&t);
    assert_eq!(db.put(b"", b"v").unwrap_err().kind(), ErrorKind::InvalidInput);
    assert_eq!(db.delete(b"").unwrap_err().kind(), ErrorKind::InvalidInput);
    let big_key = vec![b'k'; MAX_KEY_LEN + 1];
    assert_eq!(db.put(&big_key, b"v").unwrap_err().kind(), ErrorKind::InvalidInput);
    // Binary keys/values are fine.
    db.put(&[0, 255, 1], &[0, 0, 0]).unwrap();
    assert_eq!(db.get(&[0, 255, 1]).unwrap(), Some(vec![0, 0, 0]));
}

#[test]
fn corrupted_sstable_data_is_detected_not_returned() {
    let t = TempDir::new();
    {
        let mut db = open(&t);
        db.put(b"k", b"vvvvvvvv").unwrap();
        db.flush().unwrap();
    }
    let sst = fs::read_dir(t.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().map_or(false, |e| e == "sst"))
        .expect("an sst file");
    let mut bytes = fs::read(&sst).unwrap();
    bytes[20] ^= 0xFF; // inside the value bytes of the first frame
    fs::write(&sst, &bytes).unwrap();

    let db = open(&t); // index is intact so open succeeds
    let err = db.get(b"k").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[test]
fn corrupted_sstable_index_is_detected_on_open() {
    let t = TempDir::new();
    {
        let mut db = open(&t);
        db.put(b"k", b"v").unwrap();
        db.flush().unwrap();
    }
    let sst = fs::read_dir(t.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().map_or(false, |e| e == "sst"))
        .unwrap();
    let mut bytes = fs::read(&sst).unwrap();
    let n = bytes.len();
    bytes[n - 29] ^= 0xFF; // last byte of the index section
    fs::write(&sst, &bytes).unwrap();
    assert!(Db::open(t.path(), Options::default()).is_err());
}
