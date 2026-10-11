//! On-disk record frame, shared by the WAL and SSTable data section.
//!
//! Layout (all integers little-endian):
//!   [crc32: u32][body_len: u32][body]
//! body:
//!   [tag: u8 (1 = put, 0 = delete)][key_len: u32][key][val_len: u32][value]
//! The CRC covers the body only. A delete has val_len = 0.

use crate::crc32::crc32;

pub const MAX_KEY_LEN: usize = 64 * 1024;
pub const MAX_VALUE_LEN: usize = 16 * 1024 * 1024;
pub const FRAME_HEADER_LEN: usize = 8;
/// Upper bound on the encoded size of one atomic batch.
pub const MAX_BATCH_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_BODY_LEN: usize = 40 * 1024 * 1024;

const TAG_DELETE: u8 = 0;
const TAG_PUT: u8 = 1;
const TAG_BATCH: u8 = 2;

pub type Value = Option<Vec<u8>>; // None = tombstone
/// One operation inside an atomic batch: (key, Some(value)) = put, (key, None) = delete.
pub type BatchOp = (Vec<u8>, Value);

pub enum Decoded {
    Entry { key: Vec<u8>, value: Value, consumed: usize },
    /// Several operations that were written (and checksummed) as ONE frame: all or nothing.
    Batch { ops: Vec<BatchOp>, consumed: usize },
    /// Not enough bytes yet (typically a torn write at the end of the WAL).
    Incomplete,
    /// Checksum or structure mismatch.
    Corrupt,
}

pub fn encode_frame(key: &[u8], value: Option<&[u8]>) -> Vec<u8> {
    let vlen = value.map_or(0, |v| v.len());
    let body_len = 1 + 4 + key.len() + 4 + vlen;
    let mut body = Vec::with_capacity(body_len);
    body.push(if value.is_some() { TAG_PUT } else { TAG_DELETE });
    body.extend_from_slice(&(key.len() as u32).to_le_bytes());
    body.extend_from_slice(key);
    body.extend_from_slice(&(vlen as u32).to_le_bytes());
    if let Some(v) = value {
        body.extend_from_slice(v);
    }
    let mut out = Vec::with_capacity(FRAME_HEADER_LEN + body_len);
    out.extend_from_slice(&crc32(&body).to_le_bytes());
    out.extend_from_slice(&(body_len as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

pub fn decode_frame(buf: &[u8]) -> Decoded {
    if buf.len() < FRAME_HEADER_LEN {
        return Decoded::Incomplete;
    }
    let crc = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    let body_len = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
    if body_len < 9 || body_len > MAX_BODY_LEN {
        return Decoded::Corrupt;
    }
    if buf.len() < FRAME_HEADER_LEN + body_len {
        return Decoded::Incomplete;
    }
    let body = &buf[FRAME_HEADER_LEN..FRAME_HEADER_LEN + body_len];
    if crc32(body) != crc {
        return Decoded::Corrupt;
    }
    if body[0] == TAG_BATCH {
        return match parse_batch(body) {
            Some(ops) => Decoded::Batch { ops, consumed: FRAME_HEADER_LEN + body_len },
            None => Decoded::Corrupt,
        };
    }
    match parse_body(body) {
        Some((key, value)) => Decoded::Entry { key, value, consumed: FRAME_HEADER_LEN + body_len },
        None => Decoded::Corrupt,
    }
}

/// Encode many operations as a single frame. Replay applies all of them or none of them,
/// because the CRC covers the whole body.
pub fn encode_batch(ops: &[BatchOp]) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(TAG_BATCH);
    body.extend_from_slice(&(ops.len() as u32).to_le_bytes());
    for (k, v) in ops {
        body.push(if v.is_some() { TAG_PUT } else { TAG_DELETE });
        body.extend_from_slice(&(k.len() as u32).to_le_bytes());
        body.extend_from_slice(k);
        let vlen = v.as_ref().map_or(0, |x| x.len());
        body.extend_from_slice(&(vlen as u32).to_le_bytes());
        if let Some(x) = v {
            body.extend_from_slice(x);
        }
    }
    let mut out = Vec::with_capacity(FRAME_HEADER_LEN + body.len());
    out.extend_from_slice(&crc32(&body).to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn rd_u32(buf: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(pos..pos.checked_add(4)?)?.try_into().ok()?))
}

fn parse_batch(body: &[u8]) -> Option<Vec<BatchOp>> {
    let count = rd_u32(body, 1)? as usize;
    let mut pos = 5usize;
    let mut ops = Vec::new(); // not pre-sized: `count` comes from untrusted bytes
    for _ in 0..count {
        let kind = *body.get(pos)?;
        pos += 1;
        let klen = rd_u32(body, pos)? as usize;
        pos += 4;
        let key = body.get(pos..pos.checked_add(klen)?)?;
        pos += klen;
        let vlen = rd_u32(body, pos)? as usize;
        pos += 4;
        let val = body.get(pos..pos.checked_add(vlen)?)?;
        pos += vlen;
        let value = match kind {
            TAG_PUT => Some(val.to_vec()),
            TAG_DELETE if vlen == 0 => None,
            _ => return None,
        };
        ops.push((key.to_vec(), value));
    }
    if pos != body.len() {
        return None;
    }
    Some(ops)
}

fn parse_body(body: &[u8]) -> Option<(Vec<u8>, Value)> {
    let tag = *body.first()?;
    let klen = u32::from_le_bytes(body.get(1..5)?.try_into().ok()?) as usize;
    let kend = 5usize.checked_add(klen)?;
    let key = body.get(5..kend)?;
    let vstart = kend.checked_add(4)?;
    let vlen = u32::from_le_bytes(body.get(kend..vstart)?.try_into().ok()?) as usize;
    let vend = vstart.checked_add(vlen)?;
    if vend != body.len() {
        return None;
    }
    let value = match tag {
        TAG_PUT => Some(body[vstart..vend].to_vec()),
        TAG_DELETE if vlen == 0 => None,
        _ => return None,
    };
    Some((key.to_vec(), value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_put_and_delete() {
        let f = encode_frame(b"k", Some(b"v"));
        match decode_frame(&f) {
            Decoded::Entry { key, value, consumed } => {
                assert_eq!(key, b"k");
                assert_eq!(value, Some(b"v".to_vec()));
                assert_eq!(consumed, f.len());
            }
            _ => panic!("expected entry"),
        }
        let d = encode_frame(b"k", None);
        match decode_frame(&d) {
            Decoded::Entry { value, .. } => assert_eq!(value, None),
            _ => panic!("expected entry"),
        }
    }

    #[test]
    fn truncated_is_incomplete_and_flipped_is_corrupt() {
        let f = encode_frame(b"key", Some(b"value"));
        assert!(matches!(decode_frame(&f[..f.len() - 1]), Decoded::Incomplete));
        let mut bad = f.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        assert!(matches!(decode_frame(&bad), Decoded::Corrupt));
    }

    #[test]
    fn batch_roundtrip() {
        let ops: Vec<BatchOp> = vec![(b"a".to_vec(), Some(b"1".to_vec())), (b"b".to_vec(), None)];
        let f = encode_batch(&ops);
        match decode_frame(&f) {
            Decoded::Batch { ops: got, consumed } => {
                assert_eq!(got, ops);
                assert_eq!(consumed, f.len());
            }
            _ => panic!("expected batch"),
        }
        assert!(matches!(decode_frame(&f[..f.len() - 2]), Decoded::Incomplete));
    }
}
