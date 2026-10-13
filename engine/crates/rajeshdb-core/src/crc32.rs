//! CRC-32 (IEEE 802.3, polynomial 0xEDB88320). Used to detect torn/corrupt records.

const fn make_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut j = 0;
        while j < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            j += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

const TABLE: [u32; 256] = make_table();

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test]
    fn known_vector() {
        // Standard CRC-32 check value.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn empty() {
        assert_eq!(crc32(b""), 0);
    }
}
