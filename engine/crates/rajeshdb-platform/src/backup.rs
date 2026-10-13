//! Backups are plain directories:  <root>/<id>/  containing a consistent copy of the database
//! files plus `backup.json` (file names, sizes, SHA-256 of every file, record count).
//! A backup is built in `<id>.partial` and renamed to `<id>` only when complete, so a crash
//! never leaves something that looks like a valid backup but is not.
//! Ids look like `1760000000000-ab12` (13-digit millisecond timestamp + 4 hex), so they sort
//! chronologically and can never contain path separators.

use crate::util::{now_ms, rand_hex, sha256_file};
use rajeshdb_docs::{DocDb, Options};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub const META_FILE: &str = "backup.json";

pub fn valid_backup_id(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 18
        && b[..13].iter().all(|c| c.is_ascii_digit())
        && b[13] == b'-'
        && b[14..].iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

pub fn new_backup_id() -> Result<String, String> {
    Ok(format!("{:013}-{}", now_ms(), rand_hex(2)?))
}

/// Dir size in bytes, recursive; unreadable entries count as 0.
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(rd) = fs::read_dir(path) {
        for e in rd.flatten() {
            if let Ok(m) = fs::symlink_metadata(e.path()) {
                if m.is_dir() {
                    total += dir_size(&e.path());
                } else {
                    total += m.len();
                }
            }
        }
    }
    total
}

/// Create a backup of `db` under `root`. Pauses writers for the duration (needs &mut).
pub fn create_backup(root: &Path, db: &mut DocDb, kind: &str) -> Result<Value, String> {
    fs::create_dir_all(root).map_err(|e| format!("cannot create {}: {}", root.display(), e))?;
    let id = new_backup_id()?;
    let partial = root.join(format!("{}.partial", id));
    let result = (|| -> Result<Value, String> {
        let names = db.checkpoint(&partial).map_err(|e| e.to_string())?;
        // Reading everything back also proves the live data is not corrupt.
        let entries = db.verify_integrity().map_err(|e| format!("integrity check failed: {}", e))?;
        let mut files = Vec::new();
        let mut bytes = 0u64;
        for n in &names {
            let p = partial.join(n);
            let size = fs::metadata(&p).map_err(|e| e.to_string())?.len();
            bytes += size;
            let digest = sha256_file(&p).map_err(|e| e.to_string())?;
            files.push(json!({"name": n, "size": size, "sha256": digest}));
        }
        let meta = json!({"id": id, "kind": kind, "created_ms": now_ms(), "entries": entries, "bytes": bytes, "files": files});
        fs::write(partial.join(META_FILE), serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        fs::rename(&partial, root.join(&id)).map_err(|e| e.to_string())?;
        Ok(meta)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&partial);
    }
    result
}

pub fn read_meta(dir: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(dir.join(META_FILE)).map_err(|e| format!("cannot read {}: {}", META_FILE, e))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is damaged: {}", META_FILE, e))
}

/// Newest first. Incomplete (`.partial`) and unreadable entries are skipped.
pub fn list_backups(root: &Path) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    if let Ok(rd) = fs::read_dir(root) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if valid_backup_id(&name) {
                if let Ok(m) = read_meta(&e.path()) {
                    out.push(m);
                }
            }
        }
    }
    out.sort_by(|a, b| b["id"].as_str().unwrap_or("").cmp(a["id"].as_str().unwrap_or("")));
    out
}

/// Delete the oldest backups beyond `keep` (at least 1 is always kept). Returns how many were deleted.
pub fn prune(root: &Path, keep: usize) -> usize {
    let keep = keep.max(1);
    let all = list_backups(root);
    let mut deleted = 0;
    for m in all.iter().skip(keep) {
        if let Some(id) = m["id"].as_str() {
            if valid_backup_id(id) && fs::remove_dir_all(root.join(id)).is_ok() {
                deleted += 1;
            }
        }
    }
    deleted
}

/// Remove `.partial` directories left by a crash.
pub fn cleanup_partials(root: &Path) {
    if let Ok(rd) = fs::read_dir(root) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().ends_with(".partial") {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Copy the database files named in the manifest (not backup.json) into `dst`.
pub fn copy_backup_files(src: &Path, meta: &Value, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let files = meta["files"].as_array().ok_or("manifest has no file list")?;
    for f in files {
        let name = f["name"].as_str().ok_or("manifest entry has no name")?;
        // Defence in depth: manifest names must be plain file names.
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
            return Err(format!("unsafe file name in manifest: {}", name));
        }
        fs::copy(src.join(name), dst.join(name)).map_err(|e| format!("cannot copy {}: {}", name, e))?;
    }
    Ok(())
}

/// Full restore test without touching anything live: check every file's size and SHA-256,
/// copy to `scratch`, open it as a database and verify every record. Returns a report.
pub fn verify_backup(dir: &Path, scratch: &Path, opts: &Options) -> Result<Value, String> {
    let meta = read_meta(dir)?;
    let files = meta["files"].as_array().ok_or("manifest has no file list")?;
    for f in files {
        let name = f["name"].as_str().ok_or("manifest entry has no name")?;
        if name.contains('/') || name.contains('\\') || name.starts_with('.') {
            return Err(format!("unsafe file name in manifest: {}", name));
        }
        let p = dir.join(name);
        let md = fs::metadata(&p).map_err(|_| format!("file missing: {}", name))?;
        if Some(md.len()) != f["size"].as_u64() {
            return Err(format!("file size changed: {}", name));
        }
        let h = sha256_file(&p).map_err(|e| e.to_string())?;
        if Some(h.as_str()) != f["sha256"].as_str() {
            return Err(format!("checksum mismatch: {}", name));
        }
    }
    let _ = fs::remove_dir_all(scratch);
    let outcome = (|| -> Result<u64, String> {
        copy_backup_files(dir, &meta, scratch)?;
        let db = DocDb::open(scratch, opts.clone()).map_err(|e| format!("backup does not open: {}", e))?;
        let n = db.verify_integrity().map_err(|e| format!("integrity check failed: {}", e))?;
        if Some(n) != meta["entries"].as_u64() {
            return Err(format!("record count differs from manifest ({} vs {:?})", n, meta["entries"]));
        }
        Ok(n)
    })();
    let _ = fs::remove_dir_all(scratch);
    let entries = outcome?;
    Ok(json!({"ok": true, "id": meta["id"], "entries": entries, "files": files.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("rajeshdb-backup-unit-{}-{}", std::process::id(), now_ms()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn ids() {
        assert!(valid_backup_id("1760000000000-ab12"));
        for bad in ["", "../etc", "1760000000000-AB12", "1760000000000_ab12", "176000000000-ab12", "1760000000000-ab1", "1760000000000-ab12/"] {
            assert!(!valid_backup_id(bad), "{}", bad);
        }
        let id = new_backup_id().unwrap();
        assert!(valid_backup_id(&id));
    }

    #[test]
    fn create_list_verify_tamper_prune() {
        let base = temp();
        let mut db = DocDb::open(base.join("db"), Options { memtable_limit_bytes: 1 << 20, sync_writes: false }).unwrap();
        db.create_collection("c", None).unwrap();
        for i in 0..50 {
            db.insert("c", json!({"_id": format!("d{}", i), "n": i})).unwrap();
        }
        let root = base.join("backups");
        let opts = Options { memtable_limit_bytes: 1 << 20, sync_writes: false };
        let mut ids = Vec::new();
        for _ in 0..3 {
            let m = create_backup(&root, &mut db, "env").unwrap();
            ids.push(m["id"].as_str().unwrap().to_string());
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        let listed = list_backups(&root);
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0]["id"], ids[2].as_str()); // newest first
        assert!(!root.read_dir().unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".partial")));

        let ok = verify_backup(&root.join(&ids[0]), &base.join("scratch"), &opts).unwrap();
        assert_eq!(ok["ok"], true);
        assert!(!base.join("scratch").exists(), "scratch must be cleaned up");

        // flip one byte in a data file -> checksum mismatch is reported
        let victim = fs::read_dir(root.join(&ids[1])).unwrap().map(|e| e.unwrap().path())
            .find(|p| p.extension().map_or(false, |e| e == "sst")).unwrap();
        let mut bytes = fs::read(&victim).unwrap();
        bytes[10] ^= 0xFF;
        fs::write(&victim, &bytes).unwrap();
        let err = verify_backup(&root.join(&ids[1]), &base.join("scratch"), &opts).unwrap_err();
        assert!(err.contains("checksum mismatch"), "{}", err);

        // a missing manifest is reported too
        fs::remove_file(root.join(&ids[2]).join(META_FILE)).unwrap();
        assert!(verify_backup(&root.join(&ids[2]), &base.join("scratch"), &opts).is_err());
        assert_eq!(list_backups(&root).len(), 2); // the one without a manifest is not listed

        assert_eq!(prune(&root, 1), 1);
        assert_eq!(list_backups(&root).len(), 1);
        assert_eq!(prune(&root, 0), 0, "at least one backup is always kept");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn partials_are_cleaned_and_unsafe_manifest_names_rejected() {
        let base = temp();
        fs::create_dir_all(base.join("1760000000000-ab12.partial")).unwrap();
        cleanup_partials(&base);
        assert!(!base.join("1760000000000-ab12.partial").exists());
        let meta = json!({"files": [{"name": "../evil", "size": 1, "sha256": "x"}]});
        assert!(copy_backup_files(&base, &meta, &base.join("out")).is_err());
        let _ = fs::remove_dir_all(&base);
    }
}
