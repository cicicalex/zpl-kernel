//! IEEE 802.3 / Ethernet CRC-32 (polynomial 0xEDB88320). Table-driven
//! at runtime so the implementation stays no_std-friendly when the
//! kernel-side wrapper lands with the block-device driver.

const POLY: u32 = 0xEDB8_8320;

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ POLY
            } else {
                crc >> 1
            };
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
}

const TABLE: [u32; 256] = build_table();

pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for byte in bytes {
        let idx = ((crc ^ (*byte as u32)) & 0xFF) as usize;
        crc = (crc >> 8) ^ TABLE[idx];
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bytes() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn known_vector_zero() {
        assert_eq!(crc32(&[0u8]), 0xD202EF8D);
    }

    #[test]
    fn known_vector_ascii() {
        // Standard Ethernet CRC of the ASCII string "123456789".
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
    }
}
