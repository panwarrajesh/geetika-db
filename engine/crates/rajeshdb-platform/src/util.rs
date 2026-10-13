use argon2::{
    password_hash::{PasswordHash, SaltString},
    Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version,
};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn rand_hex(bytes: usize) -> Result<String, String> {
    let mut b = vec![0u8; bytes];
    getrandom::getrandom(&mut b).map_err(|e| format!("no secure randomness available: {}", e))?;
    Ok(b.iter().map(|x| format!("{:02x}", x)).collect())
}

pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{:02x}", b)).collect())
}

/// Constant-time equality for secrets of equal length (different lengths return false at once).
pub fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// True if `s` is exactly `len` lowercase hex characters (our generated ids).
pub fn is_hex_id(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn normalize_email(e: &str) -> Option<String> {
    let e = e.trim().to_lowercase();
    if e.len() > 254 || e.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (local, domain) = e.split_once('@')?;
    if local.is_empty() || domain.contains('@') || !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return None;
    }
    Some(e)
}

pub fn validate_password(p: &str) -> Result<(), String> {
    let n = p.chars().count();
    if n < 10 {
        return Err("password must be at least 10 characters".into());
    }
    if n > 128 {
        return Err("password must be at most 128 characters".into());
    }
    Ok(())
}

/// Argon2id with a random 16-byte salt. `m_cost_kib` defaults to 19456 (19 MiB) in production.
pub fn hash_password(pw: &str, m_cost_kib: u32) -> Result<String, String> {
    let params = Params::new(m_cost_kib, 2, 1, None).map_err(|e| e.to_string())?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut salt_bytes = [0u8; 16];
    getrandom::getrandom(&mut salt_bytes).map_err(|e| e.to_string())?;
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|e| e.to_string())?;
    argon.hash_password(pw.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

pub fn verify_password(pw: &str, stored: &str) -> bool {
    match PasswordHash::new(stored) {
        Ok(h) => Argon2::default().verify_password(pw.as_bytes(), &h).is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hashing() {
        let h1 = hash_password("correct horse battery", 8).unwrap();
        let h2 = hash_password("correct horse battery", 8).unwrap();
        assert_ne!(h1, h2, "salt must be random");
        assert!(!h1.contains("correct horse"));
        assert!(h1.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &h1));
        assert!(!verify_password("wrong password!!", &h1));
        assert!(!verify_password("x", "not a hash"));
    }

    #[test]
    fn validation() {
        assert_eq!(normalize_email("  Ann@Example.COM ").unwrap(), "ann@example.com");
        for bad in ["", "a", "a@b", "@x.com", "a@@x.com", "a b@x.com", "a@x.", "a@.x"] {
            assert!(normalize_email(bad).is_none(), "{}", bad);
        }
        assert!(validate_password("short").is_err());
        assert!(validate_password("long enough pw").is_ok());
        assert!(validate_password(&"x".repeat(129)).is_err());
        assert!(is_hex_id("0123456789abcdef", 16));
        assert!(!is_hex_id("0123456789abcdeg", 16));
        assert!(!is_hex_id("../..", 5));
        assert_eq!(sha256_hex("abc").len(), 64);
        assert!(ct_eq("abc", "abc") && !ct_eq("abc", "abd") && !ct_eq("abc", "ab"));
    }
}
