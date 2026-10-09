//! Little-endian cursor and error type shared by all parsers.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub what: &'static str,
    pub msg: String,
}

impl Error {
    pub fn new(what: &'static str, msg: impl Into<String>) -> Self {
        Error { what, msg: msg.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.what, self.msg)
    }
}

impl std::error::Error for Error {}

impl From<Error> for std::io::Error {
    fn from(e: Error) -> Self {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Bounds-checked little-endian reader.
#[derive(Clone)]
pub struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    pub what: &'static str,
}

macro_rules! rd {
    ($name:ident, $t:ty) => {
        pub fn $name(&mut self) -> Result<$t> {
            const N: usize = std::mem::size_of::<$t>();
            let b = self.bytes(N)?;
            Ok(<$t>::from_le_bytes(b.try_into().unwrap()))
        }
    };
}

impl<'a> Reader<'a> {
    pub fn new(what: &'static str, data: &'a [u8]) -> Self {
        Reader { data, pos: 0, what }
    }

    pub fn at(what: &'static str, data: &'a [u8], pos: usize) -> Self {
        Reader { data, pos, what }
    }

    pub fn err(&self, msg: impl Into<String>) -> Error {
        Error::new(self.what, format!("{} (at offset {:#x})", msg.into(), self.pos))
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        match end {
            Some(e) => {
                let s = &self.data[self.pos..e];
                self.pos = e;
                Ok(s)
            }
            None => Err(self.err(format!("unexpected end of data reading {n} bytes"))),
        }
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    rd!(u8, u8);
    rd!(i8, i8);
    rd!(u16, u16);
    rd!(i16, i16);
    rd!(u32, u32);
    rd!(i32, i32);
    rd!(f32, f32);

    /// Fixed-size NUL-padded string.
    pub fn fixed_str(&mut self, n: usize) -> Result<String> {
        Ok(cstr(self.bytes(n)?))
    }
}

/// Decodes a NUL-terminated Latin-1 string.
pub fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect()
}
