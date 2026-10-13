//! Immutable sorted file ("SSTable").
//!
//! File layout:
//!   [data frames, sorted by key][index section][footer]
//! index entry: [key_len: u32][key][frame_offset: u64][is_tombstone: u8]
//! footer (28 bytes): [index_offset: u64][entry_count: u64][index_crc32: u32][magic "RDBSST01"]
//!
//! Files are written to "<id>.tmp", fsynced, then atomically renamed to "<id>.sst".
//! Phase 2 limitation: the full key index is held in memory (a sparse index comes later).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::crc32::crc32;
use crate::frame::{decode_frame, encode_frame, Decoded, Value, FRAME_HEADER_LEN, MAX_BODY_LEN};
use crate::{corrupt, sync_dir};

const MAGIC: &[u8; 8] = b"RDBSST01";
const FOOTER_LEN: u64 = 28;

pub fn table_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{:06}.sst", id))
}

pub struct SsTable {
    pub id: u64,
    path: PathBuf,
    index: BTreeMap<Vec<u8>, (u64, bool)>, // key -> (frame offset, is_tombstone)
}

fn read_u32(buf: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(pos..pos.checked_add(4)?)?.try_into().ok()?))
}

fn read_u64(buf: &[u8], pos: usize) -> Option<u64> {
    Some(u64::from_le_bytes(buf.get(pos..pos.checked_add(8)?)?.try_into().ok()?))
}

impl SsTable {
    pub fn write<'a, I>(dir: &Path, id: u64, entries: I) -> io::Result<SsTable>
    where
        I: Iterator<Item = (&'a Vec<u8>, &'a Option<Vec<u8>>)>,
    {
        let final_path = table_path(dir, id);
        let tmp_path = final_path.with_extension("tmp");
        let mut w = BufWriter::new(File::create(&tmp_path)?);

        let mut offset = 0u64;
        let mut count = 0u64;
        let mut index: BTreeMap<Vec<u8>, (u64, bool)> = BTreeMap::new();
        for (k, v) in entries {
            let frame = encode_frame(k, v.as_deref());
            w.write_all(&frame)?;
            index.insert(k.clone(), (offset, v.is_none()));
            offset += frame.len() as u64;
            count += 1;
        }

        let index_offset = offset;
        let mut ibuf = Vec::new();
        for (k, (off, tomb)) in &index {
            ibuf.extend_from_slice(&(k.len() as u32).to_le_bytes());
            ibuf.extend_from_slice(k);
            ibuf.extend_from_slice(&off.to_le_bytes());
            ibuf.push(*tomb as u8);
        }
        w.write_all(&ibuf)?;
        w.write_all(&index_offset.to_le_bytes())?;
        w.write_all(&count.to_le_bytes())?;
        w.write_all(&crc32(&ibuf).to_le_bytes())?;
        w.write_all(MAGIC)?;
        w.flush()?;
        let file = w.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);

        fs::rename(&tmp_path, &final_path)?;
        sync_dir(dir)?;
        Ok(SsTable { id, path: final_path, index })
    }

    pub fn open(path: PathBuf, id: u64) -> io::Result<SsTable> {
        let mut f = File::open(&path)?;
        let len = f.metadata()?.len();
        if len < FOOTER_LEN {
            return Err(corrupt(format!("{}: file too small", path.display())));
        }
        f.seek(SeekFrom::Start(len - FOOTER_LEN))?;
        let mut foot = [0u8; FOOTER_LEN as usize];
        f.read_exact(&mut foot)?;
        if &foot[20..28] != MAGIC {
            return Err(corrupt(format!("{}: bad magic", path.display())));
        }
        let index_offset = u64::from_le_bytes(foot[0..8].try_into().unwrap());
        let count = u64::from_le_bytes(foot[8..16].try_into().unwrap());
        let icrc = u32::from_le_bytes(foot[16..20].try_into().unwrap());
        if index_offset > len - FOOTER_LEN {
            return Err(corrupt(format!("{}: bad index offset", path.display())));
        }
        let ilen = (len - FOOTER_LEN - index_offset) as usize;
        f.seek(SeekFrom::Start(index_offset))?;
        let mut ibuf = vec![0u8; ilen];
        f.read_exact(&mut ibuf)?;
        if crc32(&ibuf) != icrc {
            return Err(corrupt(format!("{}: index checksum mismatch", path.display())));
        }

        let bad = || corrupt(format!("{}: malformed index", path.display()));
        let mut index = BTreeMap::new();
        let mut pos = 0usize;
        while pos < ibuf.len() {
            let klen = read_u32(&ibuf, pos).ok_or_else(bad)? as usize;
            pos += 4;
            let kend = pos.checked_add(klen).ok_or_else(bad)?;
            let key = ibuf.get(pos..kend).ok_or_else(bad)?.to_vec();
            pos = kend;
            let off = read_u64(&ibuf, pos).ok_or_else(bad)?;
            pos += 8;
            let tomb = *ibuf.get(pos).ok_or_else(bad)?;
            pos += 1;
            index.insert(key, (off, tomb == 1));
        }
        if index.len() as u64 != count {
            return Err(corrupt(format!("{}: entry count mismatch", path.display())));
        }
        Ok(SsTable { id, path, index })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn index(&self) -> &BTreeMap<Vec<u8>, (u64, bool)> {
        &self.index
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Outer None = key not in this table. Some(None) = tombstone. Some(Some(v)) = value.
    pub fn get(&self, key: &[u8]) -> io::Result<Option<Value>> {
        match self.index.get(key) {
            None => Ok(None),
            Some(&(_, true)) => Ok(Some(None)),
            Some(&(off, false)) => {
                let (k, v) = self.read_at(off)?;
                if k != key {
                    return Err(corrupt(format!("{}: key mismatch at offset {}", self.path.display(), off)));
                }
                Ok(Some(v))
            }
        }
    }

    /// Every entry (tombstones as None), in key order. Used by compaction.
    pub fn read_all(&self) -> io::Result<Vec<(Vec<u8>, Value)>> {
        let mut out = Vec::with_capacity(self.index.len());
        for (k, &(off, _)) in &self.index {
            let (rk, v) = self.read_at(off)?;
            if &rk != k {
                return Err(corrupt(format!("{}: key mismatch at offset {}", self.path.display(), off)));
            }
            out.push((rk, v));
        }
        Ok(out)
    }

    fn read_at(&self, offset: u64) -> io::Result<(Vec<u8>, Value)> {
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut header = [0u8; FRAME_HEADER_LEN];
        f.read_exact(&mut header)?;
        let body_len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        if body_len > MAX_BODY_LEN {
            return Err(corrupt(format!("{}: absurd frame length at {}", self.path.display(), offset)));
        }
        let mut buf = vec![0u8; FRAME_HEADER_LEN + body_len];
        buf[..FRAME_HEADER_LEN].copy_from_slice(&header);
        f.read_exact(&mut buf[FRAME_HEADER_LEN..])?;
        match decode_frame(&buf) {
            Decoded::Entry { key, value, .. } => Ok((key, value)),
            _ => Err(corrupt(format!("{}: corrupt entry at offset {}", self.path.display(), offset))),
        }
    }
}
