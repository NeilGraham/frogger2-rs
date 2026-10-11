//! `sfx/index.bin`: big-endian index from (sound bank, sample uid) to a deduplicated sample file.
//!
//! ```text
//! header (16 bytes)
//!   [4]  magic "F2SX"
//!   u16  version (1)
//!   u16  nbanks
//!   u16  nsamples      unique sample files; sample id N is the file `sfx/NNNN.wav64` (4 digits)
//!   u16  reserved (0)
//!   u32  nentries
//! banks    nbanks   x 24 bytes: char name[16] (NUL padded, e.g. "garden"), u32 first, u32 count
//!                   (a bank's entries are entries[first .. first+count], in the original bank order,
//!                    so the position inside the bank is the original in-game sound index)
//! entries  nentries x  8 bytes: u32 uid (crc of the lowercase wav name, as in the .wus), u16 sample id, u16 reserved
//! samples  nsamples x 16 bytes: u32 src_rate (Hz of the retail sample), u32 src_len (samples per channel),
//!                               u32 file_bytes (size of the .wav64), u8 codec, u8 flags (bit0 = looped,
//!                               whole-sample loop), u8 channels, u8 reserved
//! ```
//! Codec values: 1 = VADPCM, 3 = Opus (libdragon `--wav-compress` numbering).
//! Opus wav64 files are stored by libdragon at 48 kHz regardless of `src_rate`; the runtime reads the real
//! playback rate from the wav64 header (`wav64_t.wave.frequency`). `src_len / src_rate` is the duration.

use crate::Result;

pub const MAGIC: [u8; 4] = *b"F2SX";
pub const VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Codec {
    Vadpcm = 1,
    Opus = 3,
}

impl Codec {
    fn from_u8(v: u8) -> Option<Codec> {
        match v {
            1 => Some(Codec::Vadpcm),
            3 => Some(Codec::Opus),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Codec::Vadpcm => "vadpcm",
            Codec::Opus => "opus",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BankRec {
    pub name: String,
    pub first: u32,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryRec {
    pub uid: u32,
    pub sample: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleRec {
    pub src_rate: u32,
    pub src_len: u32,
    pub file_bytes: u32,
    pub codec: Codec,
    pub looped: bool,
    pub channels: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SfxIndex {
    pub banks: Vec<BankRec>,
    pub entries: Vec<EntryRec>,
    pub samples: Vec<SampleRec>,
}

impl SfxIndex {
    pub fn write(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(&MAGIC);
        o.extend_from_slice(&VERSION.to_be_bytes());
        o.extend_from_slice(&(self.banks.len() as u16).to_be_bytes());
        o.extend_from_slice(&(self.samples.len() as u16).to_be_bytes());
        o.extend_from_slice(&0u16.to_be_bytes());
        o.extend_from_slice(&(self.entries.len() as u32).to_be_bytes());
        for b in &self.banks {
            let mut n = [0u8; 16];
            let nb = b.name.as_bytes();
            let l = nb.len().min(15);
            n[..l].copy_from_slice(&nb[..l]);
            o.extend_from_slice(&n);
            o.extend_from_slice(&b.first.to_be_bytes());
            o.extend_from_slice(&b.count.to_be_bytes());
        }
        for e in &self.entries {
            o.extend_from_slice(&e.uid.to_be_bytes());
            o.extend_from_slice(&e.sample.to_be_bytes());
            o.extend_from_slice(&0u16.to_be_bytes());
        }
        for s in &self.samples {
            o.extend_from_slice(&s.src_rate.to_be_bytes());
            o.extend_from_slice(&s.src_len.to_be_bytes());
            o.extend_from_slice(&s.file_bytes.to_be_bytes());
            o.push(s.codec as u8);
            o.push(u8::from(s.looped));
            o.push(s.channels);
            o.push(0);
        }
        o
    }

    pub fn read(d: &[u8]) -> Result<SfxIndex> {
        let trunc = || "sfx index truncated".to_string();
        let u16at = |o: usize| -> Result<u16> { d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]])).ok_or_else(trunc) };
        let u32at = |o: usize| -> Result<u32> { d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(trunc) };
        if d.get(..4) != Some(&MAGIC[..]) {
            return Err("sfx index: bad magic".into());
        }
        if u16at(4)? != VERSION {
            return Err("sfx index: unsupported version".into());
        }
        let (nb, ns, ne) = (u16at(6)? as usize, u16at(8)? as usize, u32at(12)? as usize);
        let (boff, eoff) = (16, 16 + nb * 24);
        let soff = eoff + ne * 8;
        if d.len() != soff + ns * 16 {
            return Err(format!("sfx index: size {} != expected {}", d.len(), soff + ns * 16));
        }
        let mut idx = SfxIndex::default();
        for i in 0..nb {
            let o = boff + i * 24;
            let raw = &d[o..o + 16];
            let end = raw.iter().position(|&c| c == 0).unwrap_or(16);
            idx.banks.push(BankRec { name: String::from_utf8_lossy(&raw[..end]).into_owned(), first: u32at(o + 16)?, count: u32at(o + 20)? });
        }
        for i in 0..ne {
            let o = eoff + i * 8;
            idx.entries.push(EntryRec { uid: u32at(o)?, sample: u16at(o + 4)? });
        }
        for i in 0..ns {
            let o = soff + i * 16;
            let codec = Codec::from_u8(d[o + 12]).ok_or("sfx index: bad codec")?;
            idx.samples.push(SampleRec {
                src_rate: u32at(o)?,
                src_len: u32at(o + 4)?,
                file_bytes: u32at(o + 8)?,
                codec,
                looped: d[o + 13] & 1 != 0,
                channels: d[o + 14],
            });
        }
        for b in &idx.banks {
            if (b.first as usize + b.count as usize) > ne {
                return Err("sfx index: bank range out of bounds".into());
            }
        }
        if idx.entries.iter().any(|e| e.sample as usize >= ns) {
            return Err("sfx index: entry references missing sample".into());
        }
        Ok(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_index() -> SfxIndex {
        SfxIndex {
            banks: vec![BankRec { name: "garden".into(), first: 0, count: 2 }, BankRec { name: "a_very_long_bank_name".into(), first: 2, count: 1 }],
            entries: vec![EntryRec { uid: 0xdead_beef, sample: 0 }, EntryRec { uid: 7, sample: 1 }, EntryRec { uid: 0xdead_beef, sample: 0 }],
            samples: vec![
                SampleRec { src_rate: 22050, src_len: 12345, file_bytes: 777, codec: Codec::Vadpcm, looped: true, channels: 1 },
                SampleRec { src_rate: 11025, src_len: 99_999, file_bytes: 1, codec: Codec::Opus, looped: false, channels: 1 },
            ],
        }
    }

    #[test]
    fn round_trip() {
        let mut idx = sample_index();
        let bytes = idx.write();
        assert_eq!(&bytes[..4], b"F2SX");
        assert_eq!(bytes.len(), 16 + 2 * 24 + 3 * 8 + 2 * 16);
        let back = SfxIndex::read(&bytes).unwrap();
        // Long names are truncated to 15 chars on write.
        idx.banks[1].name = "a_very_long_ban".into();
        assert_eq!(back, idx);
    }

    #[test]
    fn rejects_corruption() {
        let bytes = sample_index().write();
        assert!(SfxIndex::read(&bytes[..bytes.len() - 1]).is_err());
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(SfxIndex::read(&bad).is_err());
        let mut bad = bytes.clone();
        let eoff = 16 + 2 * 24;
        bad[eoff + 5] = 9; // sample id 9 does not exist
        assert!(SfxIndex::read(&bad).is_err());
    }

    #[test]
    fn big_endian_layout() {
        let b = sample_index().write();
        assert_eq!(&b[4..6], &[0, 1]);
        assert_eq!(&b[12..16], &[0, 0, 0, 3]);
        assert_eq!(&b[16 + 2 * 24..16 + 2 * 24 + 4], &0xdead_beefu32.to_be_bytes());
    }
}
