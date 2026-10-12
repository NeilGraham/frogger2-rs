//! Save game on the cartridge: the PC's 2112-byte `savegame.dat` block (`MakeSaveGameBlock`,
//! too big for a 16 Kbit EEPROM) in 256 Kbit SRAM, which flashcarts (EverDrive, SummerCart64)
//! and emulators support well.
//!
//! SRAM layout (big-endian): `"F2SV"`, u16 version (1), u16 0, u32 length, u32 FNV-1a of the
//! block, then the block. A missing/blank/corrupt record means "no save" (the PC's
//! "Couldn't load savegame": a new player starts with the training level).

use alloc::vec::Vec;

pub const MAGIC: [u8; 4] = *b"F2SV";
pub const HEADER: usize = 16;
/// Largest block accepted (below the settings record, `settings::OFFSET`).
pub const MAX_BLOCK: usize = crate::settings::OFFSET - HEADER;

fn fnv1a(d: &[u8]) -> u32 {
    d.iter().fold(0x811c_9dc5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// The SRAM record of a save block.
pub fn encode(block: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(HEADER + block.len());
    o.extend_from_slice(&MAGIC);
    o.extend_from_slice(&1u16.to_be_bytes());
    o.extend_from_slice(&0u16.to_be_bytes());
    o.extend_from_slice(&(block.len() as u32).to_be_bytes());
    o.extend_from_slice(&fnv1a(block).to_be_bytes());
    o.extend_from_slice(block);
    o
}

/// Length of the block a record header announces (to read the rest), if valid.
pub fn header_len(h: &[u8]) -> Option<usize> {
    if h.len() < HEADER || h[..4] != MAGIC || h[4..6] != [0, 1] {
        return None;
    }
    let n = u32::from_be_bytes([h[8], h[9], h[10], h[11]]) as usize;
    (n <= MAX_BLOCK).then_some(n)
}

/// The block of a full record, if intact.
pub fn decode(r: &[u8]) -> Option<&[u8]> {
    let n = header_len(r)?;
    let b = r.get(HEADER..HEADER + n)?;
    (u32::from_be_bytes([r[12], r[13], r[14], r[15]]) == fnv1a(b)).then_some(b)
}

/// Reads the save block from SRAM.
#[cfg(target_vendor = "nintendo64")]
pub fn load() -> Option<Vec<u8>> {
    let h = crate::rt::sram_read(0, HEADER)?;
    let n = header_len(&h)?;
    let r = crate::rt::sram_read(0, HEADER + n)?;
    decode(&r).map(|b| b.to_vec())
}

/// Writes the save block to SRAM.
#[cfg(target_vendor = "nintendo64")]
pub fn store(block: &[u8]) -> bool {
    block.len() <= MAX_BLOCK && crate::rt::sram_write(0, &encode(block))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_corruption() {
        let block: Vec<u8> = (0..2112u32).map(|i| (i * 7) as u8).collect();
        let r = encode(&block);
        assert_eq!(header_len(&r), Some(2112));
        assert_eq!(decode(&r), Some(&block[..]));
        let mut bad = r.clone();
        bad[100] ^= 1;
        assert!(decode(&bad).is_none());
        assert!(header_len(&[0xFF; 16]).is_none(), "blank SRAM is no save");
        assert!(header_len(&[0; 16]).is_none());
    }
}
