//! The game's name hash (`UpdateCRC`, MDX Library `mdxCRC.cpp`).
//!
//! CRC-32 with polynomial 0x04C11DB7, MSB-first, initial value 0, no
//! reflection and no final XOR. Assets are addressed by the CRC of their
//! file name: textures `"name.bmp"`, objects `"name.obe"`, samples `"name"`.

const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u32) << 24;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static TABLE: [u32; 256] = table();

/// CRC of raw bytes.
pub fn crc_bytes(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |c, &b| (c << 8) ^ TABLE[((c >> 24) ^ u32::from(b)) as usize])
}

/// CRC of a name exactly as given (case-sensitive, like the original).
pub fn crc(name: &str) -> u32 {
    crc_bytes(name.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_object_ids() {
        // Verified against descriptor ids in retail object banks.
        assert_eq!(crc("frogger.obe"), 0xf935ac38);
        assert_eq!(crc("world.obe"), 0xce8527f1);
        assert_eq!(crc("jetty.obe"), 0x6ffd5fb6);
    }
}
