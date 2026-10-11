//! Shared per-class dictionaries.
//!
//! ```text
//! "F2ZD" u8 version(1) u8 id u16 0  u32 content_len  content
//! u8 lit_mode   nctx(lit_mode) × (u8 present, [table])    default literal tables per context
//! u8 present, [table]                                     default matched-literal table
//! 3 × (u8 present, [table])                               default LL, ML, OF tables
//! ```
//! At runtime the dictionary file stays resident: `content` is borrowed by every decode
//! and the default tables are pre-built once in the [`crate::Decoder`].

use crate::litctx::{nctx, MAX_CTX, NMODES};
use crate::rans::Freqs;
use crate::{Cursor, Error, Result, DICT_MAGIC};

pub struct Dict<'a> {
    pub id: u8,
    pub content: &'a [u8],
    pub lit_mode: u8,
    pub lit: [Option<Freqs>; MAX_CTX],
    /// Matched-literal (first literal after a match, XOR-coded) table.
    pub am: Option<Freqs>,
    /// LL, ML, OF.
    pub codes: [Option<Freqs>; 3],
}

impl<'a> Dict<'a> {
    pub fn parse(b: &'a [u8]) -> Result<Dict<'a>> {
        let mut c = Cursor::new(b);
        if c.take(4)? != DICT_MAGIC {
            return Err(Error::BadMagic);
        }
        if c.u8()? != 1 {
            return Err(Error::Corrupt("unsupported dictionary version"));
        }
        let id = c.u8()?;
        if id == 0 {
            return Err(Error::Corrupt("dictionary id 0 is reserved"));
        }
        c.u16()?;
        let n = c.u32()? as usize;
        let content = c.take(n)?;
        let lit_mode = c.u8()?;
        if lit_mode as usize >= NMODES {
            return Err(Error::Corrupt("bad literal mode"));
        }
        let mut lit = [None; MAX_CTX];
        for t in lit.iter_mut().take(nctx(lit_mode)) {
            if c.u8()? != 0 {
                *t = Some(Freqs::read(&mut c)?);
            }
        }
        let am = if c.u8()? != 0 { Some(Freqs::read(&mut c)?) } else { None };
        let mut codes = [None; 3];
        for t in codes.iter_mut() {
            if c.u8()? != 0 {
                *t = Some(Freqs::read(&mut c)?);
            }
        }
        Ok(Dict { id, content, lit_mode, lit, am, codes })
    }
}

/// Host-side owned dictionary (encoder).
#[cfg(feature = "encoder")]
#[derive(Clone, Debug)]
pub struct EncDict {
    pub id: u8,
    pub content: Vec<u8>,
    pub lit_mode: u8,
    /// One entry per context of `lit_mode`.
    pub lit: Vec<Option<Freqs>>,
    pub am: Option<Freqs>,
    pub codes: [Option<Freqs>; 3],
}

#[cfg(feature = "encoder")]
impl EncDict {
    /// Content only, no default tables.
    pub fn content_only(id: u8, content: Vec<u8>) -> EncDict {
        EncDict { id, content, lit_mode: 0, lit: vec![None], am: None, codes: [None, None, None] }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(self.content.len() + 4000);
        o.extend_from_slice(&DICT_MAGIC);
        o.push(1);
        o.push(self.id);
        o.extend_from_slice(&[0, 0]);
        o.extend_from_slice(&(self.content.len() as u32).to_le_bytes());
        o.extend_from_slice(&self.content);
        o.push(self.lit_mode);
        assert_eq!(self.lit.len(), nctx(self.lit_mode));
        for t in self.lit.iter().chain(std::iter::once(&self.am)).chain(self.codes.iter()) {
            match t {
                Some(f) => {
                    o.push(1);
                    f.write(&mut o);
                }
                None => o.push(0),
            }
        }
        o
    }

    /// Default table for literal context `c` of mode `m` (only when `m` is the dictionary's mode).
    pub fn lit_table(&self, m: u8, c: usize) -> Option<&Freqs> {
        if m == self.lit_mode {
            self.lit.get(c).and_then(Option::as_ref)
        } else {
            None
        }
    }
}
