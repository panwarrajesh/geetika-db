use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::ops::Bound;
use std::path::{Path, PathBuf};

use crate::frame::{
    decode_frame, encode_batch, encode_frame, BatchOp, Decoded, Value, MAX_BATCH_BYTES, MAX_KEY_LEN,
    MAX_VALUE_LEN,
};
use crate::sstable::{table_path, SsTable};
use crate::{corrupt, sync_dir};

#[derive(Debug, Clone)]
pub struct Options {
    /// Flush the memtable to an SSTable once it holds this many (approximate) bytes.
    pub memtable_limit_bytes: usize,
    /// If true, fsync the WAL on every write (durable against power loss, slower).
    pub sync_writes: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { memtable_limit_bytes: 4 * 1024 * 1024, sync_writes: true }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RecoveryReport {
    pub records_replayed: u64,
    /// Bytes at the end of the WAL that were torn/corrupt and were cut off.
    pub bytes_discarded: u64,
}

#[derive(Debug, Clone)]
pub struct Stats {
    pub memtable_entries: usize,
    pub memtable_bytes: usize,
    pub sstable_count: usize,
    pub sstable_keys: usize,
    pub wal_bytes: u64,
}

pub struct Db {
    dir: PathBuf,
    opts: Options,
    wal: File,
    wal_len: u64,
    mem: BTreeMap<Vec<u8>, Value>,
    mem_bytes: usize,
    tables: Vec<SsTable>, // oldest first
    next_id: u64,
    report: RecoveryReport,
}

fn entry_size(key: &[u8], value: &Value) -> usize {
    key.len() + value.as_ref().map_or(0, |v| v.len()) + 32
}

fn mem_apply(mem: &mut BTreeMap<Vec<u8>, Value>, bytes: &mut usize, key: Vec<u8>, value: Value) {
    if let Some(old) = mem.get(&key) {
        *bytes -= entry_size(&key, old);
    }
    *bytes += entry_size(&key, &value);
    mem.insert(key, value);
}

fn invalid_input(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.to_string())
}

fn validate_key(key: &[u8]) -> io::Result<()> {
    if key.is_empty() {
        return Err(invalid_input("key must not be empty"));
    }
    if key.len() > MAX_KEY_LEN {
        return Err(invalid_input("key too large"));
    }
    Ok(())
}

fn write_manifest(dir: &Path, ids: &[u64]) -> io::Result<()> {
    let tmp = dir.join("MANIFEST.tmp");
    let mut f = File::create(&tmp)?;
    for id in ids {
        writeln!(f, "{}", id)?;
    }
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, dir.join("MANIFEST"))?;
    sync_dir(dir)
}

fn read_manifest(dir: &Path) -> io::Result<Vec<u64>> {
    let p = dir.join("MANIFEST");
    if !p.exists() {
        return Ok(vec![]);
    }
    let s = fs::read_to_string(p)?;
    let mut ids = Vec::new();
    for line in s.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        ids.push(l.parse::<u64>().map_err(|_| corrupt("bad MANIFEST line"))?);
    }
    ids.sort_unstable();
    Ok(ids)
}

impl Db {
    pub fn open(dir: impl AsRef<Path>, opts: Options) -> io::Result<Db> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;

        // 1. MANIFEST is the source of truth for which SSTables are live.
        let manifest_ids = read_manifest(&dir)?;

        // 2. Clean up leftovers from a crash: *.tmp files and SSTables not in MANIFEST.
        let mut max_id = manifest_ids.iter().copied().max().unwrap_or(0);
        let mut to_delete = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".tmp") {
                to_delete.push(entry.path());
            } else if let Some(stem) = name.strip_suffix(".sst") {
                if let Ok(id) = stem.parse::<u64>() {
                    max_id = max_id.max(id);
                    if !manifest_ids.contains(&id) {
                        to_delete.push(entry.path());
                    }
                }
            }
        }
        for p in to_delete {
            fs::remove_file(p)?;
        }

        let mut tables = Vec::new();
        for id in &manifest_ids {
            tables.push(SsTable::open(table_path(&dir, *id), *id)?);
        }

        // 3. Replay the WAL into a fresh memtable.
        let wal_path = dir.join("wal.log");
        let mut mem = BTreeMap::new();
        let mut mem_bytes = 0usize;
        let mut report = RecoveryReport::default();
        let mut wal_len = 0u64;
        if wal_path.exists() {
            let data = fs::read(&wal_path)?;
            let mut pos = 0usize;
            while pos < data.len() {
                match decode_frame(&data[pos..]) {
                    Decoded::Entry { key, value, consumed } => {
                        mem_apply(&mut mem, &mut mem_bytes, key, value);
                        pos += consumed;
                        report.records_replayed += 1;
                    }
                    Decoded::Batch { ops, consumed } => {
                        for (key, value) in ops {
                            mem_apply(&mut mem, &mut mem_bytes, key, value);
                            report.records_replayed += 1;
                        }
                        pos += consumed;
                    }
                    // Torn write or corruption: everything from here on is untrusted.
                    Decoded::Incomplete | Decoded::Corrupt => break,
                }
            }
            report.bytes_discarded = (data.len() - pos) as u64;
            wal_len = pos as u64;
            if report.bytes_discarded > 0 {
                let f = OpenOptions::new().write(true).open(&wal_path)?;
                f.set_len(wal_len)?;
                f.sync_all()?;
            }
        }

        // NOTE: not opened with append(true): on Windows an append-only handle cannot set_len().
        let mut wal = OpenOptions::new().create(true).write(true).open(&wal_path)?;
        wal.seek(SeekFrom::End(0))?;

        let mut db = Db {
            dir,
            opts,
            wal,
            wal_len,
            mem,
            mem_bytes,
            tables,
            next_id: max_id + 1,
            report,
        };
        if db.mem_bytes >= db.opts.memtable_limit_bytes {
            db.flush()?;
        }
        Ok(db)
    }

    pub fn recovery_report(&self) -> &RecoveryReport {
        &self.report
    }

    pub fn put(&mut self, key: &[u8], value: &[u8]) -> io::Result<()> {
        validate_key(key)?;
        if value.len() > MAX_VALUE_LEN {
            return Err(invalid_input("value too large"));
        }
        self.write_record(key, Some(value))
    }

    pub fn delete(&mut self, key: &[u8]) -> io::Result<()> {
        validate_key(key)?;
        self.write_record(key, None)
    }

    /// Append one already-encoded frame to the WAL (and fsync if configured).
    /// On failure the partial record is cut off so later records are not hidden behind garbage.
    fn append_wal(&mut self, frame: &[u8]) -> io::Result<()> {
        let res = self.wal.write_all(frame).and_then(|_| {
            if self.opts.sync_writes {
                self.wal.sync_data()
            } else {
                Ok(())
            }
        });
        if let Err(e) = res {
            let _ = self.wal.set_len(self.wal_len);
            let _ = self.wal.seek(SeekFrom::Start(self.wal_len));
            return Err(e);
        }
        self.wal_len += frame.len() as u64;
        Ok(())
    }

    fn write_record(&mut self, key: &[u8], value: Option<&[u8]>) -> io::Result<()> {
        let frame = encode_frame(key, value);
        self.append_wal(&frame)?;
        mem_apply(&mut self.mem, &mut self.mem_bytes, key.to_vec(), value.map(|v| v.to_vec()));
        if self.mem_bytes >= self.opts.memtable_limit_bytes {
            self.flush()?;
        }
        Ok(())
    }

    /// Apply several puts/deletes ATOMICALLY: after a crash either all of them are visible
    /// or none of them. If a key appears twice, the later operation wins.
    pub fn write_batch(&mut self, ops: &[BatchOp]) -> io::Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut total = 0usize;
        for (k, v) in ops {
            validate_key(k)?;
            if let Some(v) = v {
                if v.len() > MAX_VALUE_LEN {
                    return Err(invalid_input("value too large"));
                }
                total += v.len();
            }
            total += k.len() + 9;
        }
        if total > MAX_BATCH_BYTES {
            return Err(invalid_input("batch too large"));
        }
        let frame = encode_batch(ops);
        self.append_wal(&frame)?;
        for (k, v) in ops {
            mem_apply(&mut self.mem, &mut self.mem_bytes, k.clone(), v.clone());
        }
        if self.mem_bytes >= self.opts.memtable_limit_bytes {
            self.flush()?;
        }
        Ok(())
    }

    pub fn get(&self, key: &[u8]) -> io::Result<Option<Vec<u8>>> {
        if let Some(v) = self.mem.get(key) {
            return Ok(v.clone());
        }
        for t in self.tables.iter().rev() {
            if let Some(v) = t.get(key)? {
                return Ok(v);
            }
        }
        Ok(None)
    }

    /// All live (key, value) pairs whose key starts with `prefix`, sorted by key.
    /// An empty prefix scans everything.
    pub fn scan_prefix(&self, prefix: &[u8]) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut merged: BTreeMap<Vec<u8>, Value> = BTreeMap::new();
        let start = (Bound::Included(prefix), Bound::Unbounded);
        for t in &self.tables {
            for (k, _) in t.index().range::<[u8], _>(start) {
                if !k.starts_with(prefix) {
                    break;
                }
                if let Some(v) = t.get(k)? {
                    merged.insert(k.clone(), v);
                }
            }
        }
        for (k, v) in self.mem.range::<[u8], _>(start) {
            if !k.starts_with(prefix) {
                break;
            }
            merged.insert(k.clone(), v.clone());
        }
        Ok(merged.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect())
    }

    /// All live pairs with `start <= key < end` (or `start <= key` if `end` is None), sorted.
    pub fn scan_range(&self, start: &[u8], end: Option<&[u8]>) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if let Some(e) = end {
            if start >= e {
                return Ok(Vec::new());
            }
        }
        let end_bound = match end {
            Some(e) => Bound::Excluded(e),
            None => Bound::Unbounded,
        };
        let range = (Bound::Included(start), end_bound);
        let mut merged: BTreeMap<Vec<u8>, Value> = BTreeMap::new();
        for t in &self.tables {
            for (k, _) in t.index().range::<[u8], _>(range) {
                if let Some(v) = t.get(k)? {
                    merged.insert(k.clone(), v);
                }
            }
        }
        for (k, v) in self.mem.range::<[u8], _>(range) {
            merged.insert(k.clone(), v.clone());
        }
        Ok(merged.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect())
    }

    /// Write the memtable to a new SSTable, publish it in the MANIFEST, then reset the WAL.
    /// Crash-safety: a crash before the MANIFEST update leaves an orphan file (deleted on open)
    /// and the WAL is still intact; a crash after it just re-applies identical WAL records.
    pub fn flush(&mut self) -> io::Result<()> {
        if self.mem.is_empty() {
            return Ok(());
        }
        let id = self.next_id;
        let table = SsTable::write(&self.dir, id, self.mem.iter())?;
        let mut ids: Vec<u64> = self.tables.iter().map(|t| t.id).collect();
        ids.push(id);
        write_manifest(&self.dir, &ids)?;
        self.next_id += 1;
        self.tables.push(table);

        self.wal.set_len(0)?;
        self.wal.seek(SeekFrom::Start(0))?;
        self.wal.sync_all()?;
        self.wal_len = 0;
        self.mem.clear();
        self.mem_bytes = 0;
        Ok(())
    }

    /// Merge all SSTables into one, dropping overwritten values and tombstones.
    /// Phase 2 limitation: the merge happens in memory, so it needs RAM ~ dataset size.
    pub fn compact(&mut self) -> io::Result<()> {
        self.flush()?;
        if self.tables.len() < 2 {
            return Ok(());
        }
        let mut merged: BTreeMap<Vec<u8>, Value> = BTreeMap::new();
        for t in &self.tables {
            for (k, v) in t.read_all()? {
                merged.insert(k, v); // later (newer) tables overwrite earlier ones
            }
        }
        merged.retain(|_, v| v.is_some());

        let old_paths: Vec<PathBuf> = self.tables.iter().map(|t| t.path().to_path_buf()).collect();
        let id = self.next_id;
        let new_tables = if merged.is_empty() {
            vec![]
        } else {
            vec![SsTable::write(&self.dir, id, merged.iter())?]
        };
        let ids: Vec<u64> = new_tables.iter().map(|t| t.id).collect();
        write_manifest(&self.dir, &ids)?;
        self.next_id += 1;
        self.tables = new_tables;
        for p in old_paths {
            let _ = fs::remove_file(p); // if this fails, the orphan is removed on next open
        }
        Ok(())
    }

    /// Write a consistent, self-contained copy of the database into `dest` (must be empty or
    /// not exist yet): flushes the memtable, copies the immutable table files and writes a
    /// fresh MANIFEST. The copy is a normal database directory: open it with `Db::open`.
    /// Needs `&mut self`, so writers are paused while the copy is made.
    pub fn checkpoint(&mut self, dest: &Path) -> io::Result<Vec<String>> {
        self.flush()?;
        fs::create_dir_all(dest)?;
        if fs::read_dir(dest)?.next().is_some() {
            return Err(invalid_input("checkpoint destination must be empty"));
        }
        let mut names = Vec::new();
        for t in &self.tables {
            let name = t
                .path()
                .file_name()
                .ok_or_else(|| corrupt("table path has no file name"))?
                .to_owned();
            let to = dest.join(&name);
            fs::copy(t.path(), &to)?;
            OpenOptions::new().write(true).open(&to)?.sync_all()?;
            names.push(name.to_string_lossy().into_owned());
        }
        let ids: Vec<u64> = self.tables.iter().map(|t| t.id).collect();
        write_manifest(dest, &ids)?;
        names.push("MANIFEST".to_string());
        names.sort();
        Ok(names)
    }

    /// Read every record of every table file and verify its checksum. Returns the number of
    /// records (tombstones included). Fails with `InvalidData` on any corruption.
    pub fn verify_integrity(&self) -> io::Result<u64> {
        let mut n = 0u64;
        for t in &self.tables {
            n += t.read_all()?.len() as u64;
        }
        Ok(n)
    }

    pub fn stats(&self) -> Stats {
        Stats {
            memtable_entries: self.mem.len(),
            memtable_bytes: self.mem_bytes,
            sstable_count: self.tables.len(),
            sstable_keys: self.tables.iter().map(|t| t.len()).sum(),
            wal_bytes: self.wal_len,
        }
    }
}
