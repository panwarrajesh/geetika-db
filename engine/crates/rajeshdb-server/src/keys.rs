//! API keys. Only SHA-256 hashes are stored (in a JSON file); the plaintext key is shown
//! exactly once, when it is generated. Keys have a role: read < write < admin.
//! Keys are 256-bit random values, so a plain (unsalted, fast) hash is appropriate here;
//! this is NOT how user passwords should be stored (Phase 5 uses a password hash for those).

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Role {
    Read = 1,
    Write = 2,
    Admin = 3,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "read" => Some(Role::Read),
            "write" => Some(Role::Write),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Read => "read",
            Role::Write => "write",
            Role::Admin => "admin",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Principal {
    pub id: String,
    pub name: String,
    pub role: Role,
}

pub fn hash_key(key: &str) -> String {
    Sha256::digest(key.as_bytes()).iter().map(|b| format!("{:02x}", b)).collect()
}

fn rand_hex(n: usize) -> Result<String, String> {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).map_err(|e| format!("no secure randomness available: {}", e))?;
    Ok(b.iter().map(|x| format!("{:02x}", x)).collect())
}

fn read_records(path: &Path) -> Result<Vec<Value>, String> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {}", path.display(), e))?;
    match v {
        Value::Array(a) => Ok(a),
        _ => Err(format!("{} must contain a JSON array", path.display())),
    }
}

fn write_records(path: &Path, recs: &[Value]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let text = serde_json::to_string_pretty(recs).map_err(|e| e.to_string())?;
    fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
    fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {}", path.display(), e))
}

/// hash -> principal
pub fn load_keys(path: &Path) -> Result<HashMap<String, Principal>, String> {
    let mut map = HashMap::new();
    for r in read_records(path)? {
        let get = |k: &str| r.get(k).and_then(|v| v.as_str()).map(String::from);
        let (id, name, role, hash) = match (get("id"), get("name"), get("role"), get("hash")) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => return Err("malformed record in keys file".into()),
        };
        let role = Role::parse(&role).ok_or_else(|| format!("unknown role '{}' in keys file", role))?;
        map.insert(hash, Principal { id, name, role });
    }
    Ok(map)
}

/// Create a key, store its hash, and return (plaintext key, key id). The plaintext is not stored.
pub fn generate_key(path: &Path, name: &str, role: Role) -> Result<(String, String), String> {
    if name.is_empty() || name.len() > 64 || name.chars().any(|c| c.is_control()) {
        return Err("name must be 1-64 printable characters".into());
    }
    let key = format!("rdb_{}", rand_hex(32)?);
    let id = rand_hex(4)?;
    let mut recs = read_records(path)?;
    let created_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    recs.push(json!({"id": id, "name": name, "role": role.as_str(), "hash": hash_key(&key), "created_ms": created_ms}));
    write_records(path, &recs)?;
    Ok((key, id))
}

pub fn list_keys(path: &Path) -> Result<Vec<(String, String, String)>, String> {
    let mut out = Vec::new();
    for r in read_records(path)? {
        let g = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        out.push((g("id"), g("name"), g("role")));
    }
    Ok(out)
}

pub fn revoke_key(path: &Path, id: &str) -> Result<bool, String> {
    let mut recs = read_records(path)?;
    let before = recs.len();
    recs.retain(|r| r.get("id").and_then(|v| v.as_str()) != Some(id));
    if recs.len() == before {
        return Ok(false);
    }
    write_records(path, &recs)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file() -> std::path::PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("rajeshdb-keys-{}-{}.json", std::process::id(), nanos))
    }

    #[test]
    fn generate_load_revoke() {
        let p = temp_file();
        let (key, id) = generate_key(&p, "ci", Role::Write).unwrap();
        assert!(key.starts_with("rdb_") && key.len() == 4 + 64);
        let text = fs::read_to_string(&p).unwrap();
        assert!(!text.contains(&key), "plaintext key must never be stored");
        let map = load_keys(&p).unwrap();
        let who = map.get(&hash_key(&key)).expect("key resolves");
        assert_eq!(who.role, Role::Write);
        assert_eq!(who.id, id);
        let (key2, _) = generate_key(&p, "other", Role::Read).unwrap();
        assert_ne!(key, key2);
        assert_eq!(list_keys(&p).unwrap().len(), 2);
        assert!(revoke_key(&p, &id).unwrap());
        assert!(!revoke_key(&p, &id).unwrap());
        assert!(!load_keys(&p).unwrap().contains_key(&hash_key(&key)));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn bad_names_and_roles() {
        let p = temp_file();
        assert!(generate_key(&p, "", Role::Read).is_err());
        assert!(generate_key(&p, "a\nb", Role::Read).is_err());
        assert!(Role::parse("root").is_none());
        assert!(Role::Admin > Role::Write && Role::Write > Role::Read);
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let p = temp_file();
        fs::write(&p, "not json").unwrap();
        assert!(load_keys(&p).is_err());
        fs::write(&p, "{}").unwrap();
        assert!(load_keys(&p).is_err());
        let _ = fs::remove_file(&p);
    }
}
