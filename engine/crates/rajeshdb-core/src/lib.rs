//! RajeshDB core storage engine (Phase 2).
//!
//! Write path: append to WAL (optionally fsync) -> apply to in-memory memtable.
//! When the memtable is big enough it is flushed to an immutable sorted SSTable,
//! the MANIFEST is atomically updated, and the WAL is reset.
//! Recovery: replay WAL on open; a torn/corrupt tail is detected via CRC and cut off.

mod crc32;
mod db;
pub mod frame;
mod sstable;

pub use db::{Db, Options, RecoveryReport, Stats};
pub use frame::{BatchOp, MAX_BATCH_BYTES, MAX_KEY_LEN, MAX_VALUE_LEN};

use std::io;
use std::path::Path;

pub(crate) fn corrupt(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// fsync a directory so renames/creates are durable. Not possible via std on Windows
/// (NTFS journals metadata itself), so it is a no-op there.
#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}
