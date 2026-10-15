//! `param.sfo`: the PS4 title metadata table ("\0PSF", version 1.1): a sorted key table and a
//! data table of fixed-size slots (format as documented on psdevwiki and implemented by
//! LibOrbisPkg's `SFO/ParamSfo.cs`).

/// One value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SfoValue {
    /// `0x0404`: a 32-bit integer.
    Int(i32),
    /// `0x0204`: a NUL-terminated UTF-8 string in a slot of `max` bytes.
    Utf8(String, u32),
    /// `0x0004`: raw UTF-8 bytes (no terminator) in a slot of `max` bytes.
    #[cfg_attr(not(test), allow(dead_code))]
    Utf8Special(String, u32),
}

impl SfoValue {
    fn format(&self) -> u16 {
        match self {
            SfoValue::Int(_) => 0x0404,
            SfoValue::Utf8(..) => 0x0204,
            SfoValue::Utf8Special(..) => 0x0004,
        }
    }

    fn len(&self) -> u32 {
        match self {
            SfoValue::Int(_) => 4,
            SfoValue::Utf8(s, _) => s.len() as u32 + 1,
            SfoValue::Utf8Special(s, _) => s.len() as u32,
        }
    }

    fn max(&self) -> u32 {
        match self {
            SfoValue::Int(_) => 4,
            SfoValue::Utf8(_, m) | SfoValue::Utf8Special(_, m) => *m,
        }
    }

    /// LibOrbisPkg's `ToString()`, used in the package's "major param" digest.
    pub(crate) fn display(&self) -> String {
        match self {
            SfoValue::Int(v) => format!("0x{:08x}", *v as u32),
            SfoValue::Utf8(s, _) | SfoValue::Utf8Special(s, _) => s.clone(),
        }
    }
}

/// A `param.sfo`, kept sorted by key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sfo {
    pub(crate) values: Vec<(String, SfoValue)>,
}

impl Sfo {
    pub(crate) fn set(&mut self, key: &str, v: SfoValue) {
        self.values.retain(|(k, _)| k != key);
        self.values.push((key.to_string(), v));
        self.values.sort_by(|a, b| a.0.cmp(&b.0));
    }

    pub(crate) fn get(&self, key: &str) -> Option<&SfoValue> {
        self.values.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub(crate) fn set_int(&mut self, key: &str, v: i32) {
        self.set(key, SfoValue::Int(v));
    }

    pub(crate) fn set_str(&mut self, key: &str, v: &str, max: u32) {
        self.set(key, SfoValue::Utf8(v.to_string(), max));
    }

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let key_table = 0x14 + 0x10 * self.values.len();
        let key_size: usize = self.values.iter().map(|(k, _)| k.len() + 1).sum();
        let data_table = (key_table + key_size).next_multiple_of(4);
        let data_size: usize = self.values.iter().map(|(_, v)| v.max() as usize).sum();
        let mut out = vec![0u8; data_table + data_size];
        out[0..4].copy_from_slice(b"\0PSF");
        out[4..8].copy_from_slice(&0x101u32.to_le_bytes());
        out[8..12].copy_from_slice(&(key_table as u32).to_le_bytes());
        out[12..16].copy_from_slice(&(data_table as u32).to_le_bytes());
        out[16..20].copy_from_slice(&(self.values.len() as u32).to_le_bytes());
        let (mut ko, mut dof) = (0usize, 0usize);
        for (i, (k, v)) in self.values.iter().enumerate() {
            let e = 0x14 + 0x10 * i;
            out[e..e + 2].copy_from_slice(&(ko as u16).to_le_bytes());
            out[e + 2..e + 4].copy_from_slice(&v.format().to_le_bytes());
            out[e + 4..e + 8].copy_from_slice(&v.len().to_le_bytes());
            out[e + 8..e + 12].copy_from_slice(&v.max().to_le_bytes());
            out[e + 12..e + 16].copy_from_slice(&(dof as u32).to_le_bytes());
            out[key_table + ko..key_table + ko + k.len()].copy_from_slice(k.as_bytes());
            let d = data_table + dof;
            match v {
                SfoValue::Int(x) => out[d..d + 4].copy_from_slice(&x.to_le_bytes()),
                SfoValue::Utf8(s, m) | SfoValue::Utf8Special(s, m) => {
                    let n = s.len().min(*m as usize);
                    out[d..d + n].copy_from_slice(&s.as_bytes()[..n]);
                }
            }
            ko += k.len() + 1;
            dof += v.max() as usize;
        }
        out
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn parse(b: &[u8]) -> Result<Sfo, String> {
        let u32_at = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).ok_or("param.sfo truncated");
        if b.get(0..4) != Some(b"\0PSF") {
            return Err("not a param.sfo".into());
        }
        let (kt, dt, n) = (u32_at(8)? as usize, u32_at(12)? as usize, u32_at(16)? as usize);
        let mut sfo = Sfo::default();
        for i in 0..n {
            let e = 0x14 + 0x10 * i;
            let ko = u16::from_le_bytes(b.get(e..e + 2).ok_or("param.sfo truncated")?.try_into().unwrap()) as usize;
            let fmt = u16::from_le_bytes(b[e + 2..e + 4].try_into().unwrap());
            let (len, max, dof) = (u32_at(e + 4)? as usize, u32_at(e + 8)?, u32_at(e + 12)? as usize);
            let key_bytes = b.get(kt + ko..).ok_or("param.sfo truncated")?;
            let key = String::from_utf8_lossy(&key_bytes[..key_bytes.iter().position(|&c| c == 0).unwrap_or(key_bytes.len())]).into_owned();
            let d = b.get(dt + dof..dt + dof + len).ok_or("param.sfo truncated")?;
            let v = match fmt {
                0x0404 => SfoValue::Int(i32::from_le_bytes(d[..4].try_into().unwrap())),
                0x0204 => SfoValue::Utf8(String::from_utf8_lossy(&d[..len.saturating_sub(1)]).into_owned(), max),
                0x0004 => SfoValue::Utf8Special(String::from_utf8_lossy(d).into_owned(), max),
                f => return Err(format!("param.sfo: unknown value format {f:#06x}")),
            };
            sfo.values.push((key, v));
        }
        Ok(sfo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_layout() {
        let mut s = Sfo::default();
        s.set_str("TITLE", "Frogger 2: Swampy's Revenge", 128);
        s.set_int("APP_TYPE", 1);
        s.set_str("CATEGORY", "gd", 4);
        s.set_str("TITLE_ID", "FROG20000", 12);
        let b = s.to_bytes();
        let keys: Vec<&str> = s.values.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["APP_TYPE", "CATEGORY", "TITLE", "TITLE_ID"]);
        // header 0x14 + 4 entries, keys 9+9+6+9 = 33 -> data table at 0x54 + 33 = 0x75 -> 0x78
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 0x78);
        assert_eq!(b.len(), 0x78 + 4 + 4 + 128 + 12);
        assert_eq!(Sfo::parse(&b).unwrap(), s);
        assert_eq!(s.get("TITLE").unwrap().display(), "Frogger 2: Swampy's Revenge");
        assert_eq!(SfoValue::Int(0).display(), "0x00000000");
    }
}
