//! Shared per-class dictionaries.
//!
//! ```text
//! "F2ZD" u8 version(1) u8 id u16 0  u32 content_len  content
//! 4 × (u8 present, [table if present])     default tables for LIT, LL, ML, OF
//! ```
//! At runtime the dictionary file stays resident: `content` is borrowed by every decode
//! and the default tables are pre-built once in the [`crate::Decoder`].

use crate::rans::Freqs;
use crate::{Cursor, Error, Result, DICT_MAGIC};

pub struct Dict<'a> {
    pub id: u8,
    pub content: &'a [u8],
    pub tables: [Option<Freqs>; 4],
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
        let mut tables = [None; 4];
        for t in tables.iter_mut() {
            if c.u8()? != 0 {
                *t = Some(Freqs::read(&mut c)?);
            }
        }
        Ok(Dict { id, content, tables })
    }
}

/// Host-side owned dictionary (encoder).
#[cfg(feature = "encoder")]
#[derive(Clone, Debug)]
pub struct EncDict {
    pub id: u8,
    pub content: Vec<u8>,
    pub tables: [Option<Freqs>; 4],
}

#[cfg(feature = "encoder")]
impl EncDict {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(self.content.len() + 1200);
        o.extend_from_slice(&DICT_MAGIC);
        o.push(1);
        o.push(self.id);
        o.extend_from_slice(&[0, 0]);
        o.extend_from_slice(&(self.content.len() as u32).to_le_bytes());
        o.extend_from_slice(&self.content);
        for t in &self.tables {
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
}
