//! Key layout inside the underlying key/value engine.
//!
//!   m/<collection>                          -> collection metadata (JSON)
//!   d/<collection>/<id>                     -> the document (JSON)
//!   i/<collection>/<field>/<enc(value)><id> -> empty (secondary index entry)
//!
//! Collection names may not contain '/', field paths may not contain '/', so prefixes
//! never overlap. enc(value) is order-preserving and self-delimiting, so the id can be
//! appended directly:
//!   null   'z'
//!   bool   'b' + 0/1
//!   number 'n' + 8 bytes (f64, transformed so byte order == numeric order)
//!   string 's' + bytes (0x00 escaped as 0x00 0xFF) + 0x00 0x01

use crate::error::{invalid, Result};
use serde_json::Value;

pub const MAX_INDEXED_STRING: usize = 1024;
pub const MAX_ID_LEN: usize = 512;

pub fn validate_collection_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(invalid("collection name must be 1-64 chars of A-Z a-z 0-9 _ -"))
    }
}

pub fn validate_field_path(p: &str) -> Result<()> {
    let ok = !p.is_empty()
        && p.len() <= 256
        && !p.contains('/')
        && !p.contains('\0')
        && p.split('.').all(|s| !s.is_empty());
    if ok {
        Ok(())
    } else {
        Err(invalid("field path must be non-empty dotted names without '/' (e.g. address.city)"))
    }
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return Err(invalid("_id must be 1-512 bytes"));
    }
    Ok(())
}

pub fn catalog_key(coll: &str) -> Vec<u8> {
    format!("m/{}", coll).into_bytes()
}

pub fn doc_prefix(coll: &str) -> Vec<u8> {
    format!("d/{}/", coll).into_bytes()
}

pub fn doc_key(coll: &str, id: &str) -> Vec<u8> {
    let mut k = doc_prefix(coll);
    k.extend_from_slice(id.as_bytes());
    k
}

pub fn index_coll_prefix(coll: &str) -> Vec<u8> {
    format!("i/{}/", coll).into_bytes()
}

pub fn index_field_prefix(coll: &str, field: &str) -> Vec<u8> {
    format!("i/{}/{}/", coll, field).into_bytes()
}

pub fn index_key(coll: &str, field: &str, enc: &[u8], id: &str) -> Vec<u8> {
    let mut k = index_field_prefix(coll, field);
    k.extend_from_slice(enc);
    k.extend_from_slice(id.as_bytes());
    k
}

/// Ok(None) = value is not indexable (array/object). Err = string too long to index.
pub fn encode_scalar(v: &Value) -> Result<Option<Vec<u8>>> {
    Ok(match v {
        Value::Null => Some(vec![b'z']),
        Value::Bool(b) => Some(vec![b'b', *b as u8]),
        Value::Number(n) => {
            let mut f = n.as_f64().ok_or_else(|| invalid("number out of range"))?;
            if f == 0.0 {
                f = 0.0; // normalise -0.0
            }
            let bits = f.to_bits();
            let ordered = if bits >> 63 == 1 { !bits } else { bits | (1u64 << 63) };
            let mut out = vec![b'n'];
            out.extend_from_slice(&ordered.to_be_bytes());
            Some(out)
        }
        Value::String(s) => {
            if s.len() > MAX_INDEXED_STRING {
                return Err(invalid(format!(
                    "indexed string value too long ({} bytes, max {})",
                    s.len(),
                    MAX_INDEXED_STRING
                )));
            }
            let mut out = vec![b's'];
            for &b in s.as_bytes() {
                if b == 0 {
                    out.push(0);
                    out.push(0xFF);
                } else {
                    out.push(b);
                }
            }
            out.push(0);
            out.push(1);
            Some(out)
        }
        Value::Array(_) | Value::Object(_) => None,
    })
}

/// Given the bytes after `i/<coll>/<field>/`, skip the encoded value and return the id bytes.
pub fn decode_id(rest: &[u8]) -> Option<&[u8]> {
    match *rest.first()? {
        b'z' => rest.get(1..),
        b'b' => rest.get(2..),
        b'n' => rest.get(9..),
        b's' => {
            let mut i = 1;
            loop {
                let b = *rest.get(i)?;
                if b == 0 {
                    match *rest.get(i + 1)? {
                        1 => return rest.get(i + 2..),
                        0xFF => i += 2,
                        _ => return None,
                    }
                } else {
                    i += 1;
                }
            }
        }
        _ => None,
    }
}

/// Smallest byte string greater than every string that starts with `p`.
pub fn prefix_successor(p: &[u8]) -> Option<Vec<u8>> {
    let mut v = p.to_vec();
    while let Some(last) = v.pop() {
        if last < 0xFF {
            v.push(last + 1);
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn enc(v: Value) -> Vec<u8> {
        encode_scalar(&v).unwrap().unwrap()
    }

    #[test]
    fn numbers_sort_numerically() {
        let vals = [-1e9, -10.5, -1.0, -0.0001, 0.0, 0.0001, 0.5, 1.0, 2.0, 1e9];
        let encs: Vec<Vec<u8>> = vals.iter().map(|v| enc(json!(*v))).collect();
        for w in encs.windows(2) {
            assert!(w[0] < w[1]);
        }
        assert_eq!(enc(json!(0.0)), enc(json!(-0.0)));
    }

    #[test]
    fn strings_sort_and_delimit() {
        assert!(enc(json!("a")) < enc(json!("ab")));
        assert!(enc(json!("ab")) < enc(json!("b")));
        assert!(enc(json!("")) < enc(json!("a")));
        let with_nul = enc(json!("a\u{0}b"));
        assert!(enc(json!("a")) < with_nul);
    }

    #[test]
    fn decode_id_skips_value() {
        for v in [json!(null), json!(true), json!(12.5), json!("hi\u{0}there"), json!("")] {
            let mut k = enc(v);
            k.extend_from_slice(b"doc-1");
            assert_eq!(decode_id(&k), Some(&b"doc-1"[..]));
        }
    }

    #[test]
    fn successor() {
        assert_eq!(prefix_successor(b"ab"), Some(b"ac".to_vec()));
        assert_eq!(prefix_successor(&[1, 0xFF]), Some(vec![2]));
        assert_eq!(prefix_successor(&[0xFF, 0xFF]), None);
    }

    #[test]
    fn long_strings_rejected_and_containers_skipped() {
        assert!(encode_scalar(&json!("x".repeat(MAX_INDEXED_STRING + 1))).is_err());
        assert!(encode_scalar(&json!([1, 2])).unwrap().is_none());
        assert!(encode_scalar(&json!({"a": 1})).unwrap().is_none());
    }
}
