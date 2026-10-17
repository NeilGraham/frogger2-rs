//! Android binary XML (`AndroidManifest.xml` as compiled by aapt2) and its string pool
//! (`ResStringPool`, the same chunk `resources.arsc` uses).
//!
//! Edits never patch strings in place: [`StringPool`] decodes every string, new strings are
//! appended (so existing indices, the attribute resource map and styles stay valid) and the
//! pool is re-encoded with fresh offsets, padding and chunk sizes. Attribute edits re-point
//! an attribute's raw value and typed value at the new string.

const RES_STRING_POOL_TYPE: u16 = 0x0001;
const RES_XML_TYPE: u16 = 0x0003;
const RES_XML_START_ELEMENT_TYPE: u16 = 0x0102;
const UTF8_FLAG: u32 = 0x100;
const SORTED_FLAG: u32 = 0x1;
const NO_INDEX: u32 = 0xffff_ffff;

/// `Res_value::dataType`.
pub const TYPE_STRING: u8 = 0x03;
pub const TYPE_INT_DEC: u8 = 0x10;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// A decoded `ResStringPool` chunk.
#[derive(Clone, Debug, PartialEq)]
pub struct StringPool {
    pub strings: Vec<String>,
    pub utf8: bool,
    flags: u32,
    /// Style spans, kept as raw bytes (offsets relative to the styles area) and copied
    /// unchanged: they reference string indices, which never move.
    style_offsets: Vec<u32>,
    styles: Vec<u8>,
}

impl StringPool {
    /// Decodes the pool chunk at the start of `b`; returns it and the chunk's size.
    pub fn parse(b: &[u8]) -> Result<(StringPool, usize), String> {
        if b.len() < 28 || u16le(b, 0) != RES_STRING_POOL_TYPE {
            return Err("not a string pool chunk".into());
        }
        let header = u16le(b, 2) as usize;
        let size = u32le(b, 4) as usize;
        if size > b.len() || header < 28 {
            return Err("string pool chunk out of range".into());
        }
        let b = &b[..size];
        let count = u32le(b, 8) as usize;
        let style_count = u32le(b, 12) as usize;
        let flags = u32le(b, 16);
        let strings_start = u32le(b, 20) as usize;
        let styles_start = u32le(b, 24) as usize;
        let utf8 = flags & UTF8_FLAG != 0;
        if header + 4 * (count + style_count) > size || strings_start > size || styles_start > size {
            return Err("string pool offsets out of range".into());
        }
        let mut strings = Vec::with_capacity(count);
        for i in 0..count {
            let off = strings_start + u32le(b, header + 4 * i) as usize;
            let s = if utf8 { read_utf8(b, off) } else { read_utf16(b, off) }.ok_or_else(|| format!("string {i} out of range"))?;
            strings.push(s);
        }
        let style_offsets = (0..style_count).map(|i| u32le(b, header + 4 * (count + i))).collect();
        let styles = if style_count > 0 { b[styles_start..].to_vec() } else { Vec::new() };
        Ok((StringPool { strings, utf8, flags, style_offsets, styles }, size))
    }

    /// Index of `s`, appended if no string at or after `min_index` equals it (indices below
    /// `min_index` may carry meaning, e.g. the XML resource map's attribute names).
    pub fn intern(&mut self, s: &str, min_index: usize) -> u32 {
        if let Some(i) = self.strings.iter().skip(min_index).position(|x| x == s) {
            return (i + min_index) as u32;
        }
        self.strings.push(s.to_string());
        // Appending breaks the sort order a SORTED pool promises.
        self.flags &= !SORTED_FLAG;
        (self.strings.len() - 1) as u32
    }

    /// Encodes the chunk (header, offsets, string data padded to 4 bytes, styles).
    pub fn encode(&self) -> Vec<u8> {
        let mut data = Vec::new();
        let mut offsets = Vec::with_capacity(self.strings.len());
        for s in &self.strings {
            offsets.push(data.len() as u32);
            if self.utf8 {
                write_utf8(&mut data, s);
            } else {
                write_utf16(&mut data, s);
            }
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let header = 28usize;
        let strings_start = header + 4 * (self.strings.len() + self.style_offsets.len());
        let styles_start = if self.style_offsets.is_empty() { 0 } else { strings_start + data.len() };
        let size = strings_start + data.len() + self.styles.len();
        let mut out = Vec::with_capacity(size);
        out.extend_from_slice(&RES_STRING_POOL_TYPE.to_le_bytes());
        out.extend_from_slice(&(header as u16).to_le_bytes());
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&(self.strings.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.style_offsets.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&(strings_start as u32).to_le_bytes());
        out.extend_from_slice(&(styles_start as u32).to_le_bytes());
        for o in offsets.iter().chain(&self.style_offsets) {
            out.extend_from_slice(&o.to_le_bytes());
        }
        out.extend_from_slice(&data);
        out.extend_from_slice(&self.styles);
        debug_assert_eq!(out.len(), size);
        out
    }
}

/// Length prefix of UTF-8 pools: 1 byte, or 2 with the high bit set (15 bits).
fn read_len8(b: &[u8], o: &mut usize) -> Option<usize> {
    let a = *b.get(*o)? as usize;
    *o += 1;
    if a & 0x80 != 0 {
        let c = *b.get(*o)? as usize;
        *o += 1;
        Some(((a & 0x7f) << 8) | c)
    } else {
        Some(a)
    }
}

fn read_utf8(b: &[u8], mut o: usize) -> Option<String> {
    let _utf16_len = read_len8(b, &mut o)?;
    let n = read_len8(b, &mut o)?;
    Some(String::from_utf8_lossy(b.get(o..o + n)?).into_owned())
}

fn read_utf16(b: &[u8], mut o: usize) -> Option<String> {
    let mut n = u16::from_le_bytes([*b.get(o)?, *b.get(o + 1)?]) as usize;
    o += 2;
    if n & 0x8000 != 0 {
        let lo = u16::from_le_bytes([*b.get(o)?, *b.get(o + 1)?]) as usize;
        o += 2;
        n = ((n & 0x7fff) << 16) | lo;
    }
    let units: Vec<u16> = b.get(o..o + 2 * n)?.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    Some(String::from_utf16_lossy(&units))
}

fn write_len8(out: &mut Vec<u8>, n: usize) {
    if n > 0x7f {
        out.push(0x80 | ((n >> 8) & 0x7f) as u8);
    }
    out.push((n & 0xff) as u8);
}

fn write_utf8(out: &mut Vec<u8>, s: &str) {
    write_len8(out, s.encode_utf16().count());
    write_len8(out, s.len());
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn write_utf16(out: &mut Vec<u8>, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    let n = units.len();
    if n > 0x7fff {
        out.extend_from_slice(&((0x8000 | (n >> 16)) as u16).to_le_bytes());
    }
    out.extend_from_slice(&((n & 0xffff) as u16).to_le_bytes());
    for u in units {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out.extend_from_slice(&[0, 0]);
}

/// A compiled XML document: its string pool and the chunks after it (kept as bytes; the
/// attribute edits patch them in place, they only hold string indices).
pub struct Axml {
    pub pool: StringPool,
    /// Number of strings the resource map covers (attribute names; never reused for values).
    res_map_len: usize,
    /// Everything after the string pool chunk.
    rest: Vec<u8>,
}

/// One attribute of a start element: its position in [`Axml::rest`].
struct AttrPos {
    element: String,
    name: String,
    at: usize,
}

impl Axml {
    pub fn parse(b: &[u8]) -> Result<Axml, String> {
        if b.len() < 8 || u16le(b, 0) != RES_XML_TYPE {
            return Err("not a binary XML file".into());
        }
        let header = u16le(b, 2) as usize;
        let size = (u32le(b, 4) as usize).min(b.len());
        let (pool, pool_size) = StringPool::parse(&b[header..size])?;
        let rest = b[header + pool_size..size].to_vec();
        let res_map_len = if rest.len() >= 8 && u16le(&rest, 0) == 0x0180 { (u32le(&rest, 4) as usize - u16le(&rest, 2) as usize) / 4 } else { 0 };
        let a = Axml { pool, res_map_len, rest };
        a.attrs()?;
        Ok(a)
    }

    pub fn encode(&self) -> Vec<u8> {
        let pool = self.pool.encode();
        let size = 8 + pool.len() + self.rest.len();
        let mut out = Vec::with_capacity(size);
        out.extend_from_slice(&RES_XML_TYPE.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&pool);
        out.extend_from_slice(&self.rest);
        out
    }

    fn string(&self, i: u32) -> String {
        if i == NO_INDEX {
            return String::new();
        }
        self.pool.strings.get(i as usize).cloned().unwrap_or_default()
    }

    fn attrs(&self) -> Result<Vec<AttrPos>, String> {
        let b = &self.rest;
        let mut out = Vec::new();
        let mut p = 0;
        while p + 8 <= b.len() {
            let ty = u16le(b, p);
            let header = u16le(b, p + 2) as usize;
            let size = u32le(b, p + 4) as usize;
            if size < 8 || p + size > b.len() {
                return Err("corrupt binary XML chunk".into());
            }
            if ty == RES_XML_START_ELEMENT_TYPE {
                let ext = p + header;
                let element = self.string(u32le(b, ext + 4));
                let start = u16le(b, ext + 8) as usize;
                let asize = u16le(b, ext + 10) as usize;
                let count = u16le(b, ext + 12) as usize;
                for k in 0..count {
                    let at = ext + start + k * asize;
                    if at + 20 > p + size {
                        return Err("corrupt binary XML attribute".into());
                    }
                    out.push(AttrPos { element: element.clone(), name: self.string(u32le(b, at + 4)), at });
                }
            }
            p += size;
        }
        Ok(out)
    }

    fn attr(&self, element: &str, name: &str) -> Result<usize, String> {
        self.attrs()?
            .into_iter()
            .find(|a| a.element == element && a.name == name)
            .map(|a| a.at)
            .ok_or_else(|| format!("<{element} {name}=...> not found in the manifest"))
    }

    /// The typed value (data type, data) of `<element name=...>` (the first such element).
    pub fn value(&self, element: &str, name: &str) -> Result<(u8, u32), String> {
        let at = self.attr(element, name)?;
        Ok((self.rest[at + 15], u32le(&self.rest, at + 16)))
    }

    /// The string value of `<element name=...>`.
    pub fn get_string(&self, element: &str, name: &str) -> Result<String, String> {
        let at = self.attr(element, name)?;
        let raw = u32le(&self.rest, at + 8);
        match self.value(element, name)? {
            (TYPE_STRING, i) => Ok(self.string(i)),
            _ if raw != NO_INDEX => Ok(self.string(raw)),
            _ => Err(format!("<{element} {name}> is not a string")),
        }
    }

    /// Sets `<element name=...>` to the string `value` (raw and typed value).
    pub fn set_string(&mut self, element: &str, name: &str, value: &str) -> Result<(), String> {
        let at = self.attr(element, name)?;
        let i = self.pool.intern(value, self.res_map_len);
        put32(&mut self.rest, at + 8, i);
        self.rest[at + 12..at + 14].copy_from_slice(&8u16.to_le_bytes());
        self.rest[at + 14] = 0;
        self.rest[at + 15] = TYPE_STRING;
        put32(&mut self.rest, at + 16, i);
        Ok(())
    }

    /// Sets `<element name=...>` to the decimal integer `value`.
    pub fn set_int(&mut self, element: &str, name: &str, value: u32) -> Result<(), String> {
        let at = self.attr(element, name)?;
        put32(&mut self.rest, at + 8, NO_INDEX);
        self.rest[at + 15] = TYPE_INT_DEC;
        put32(&mut self.rest, at + 16, value);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn pool_bytes(strings: &[&str], utf8: bool) -> Vec<u8> {
        StringPool {
            strings: strings.iter().map(|s| s.to_string()).collect(),
            utf8,
            flags: if utf8 { UTF8_FLAG } else { 0 },
            style_offsets: vec![],
            styles: vec![],
        }
        .encode()
    }

    #[test]
    fn pools_round_trip_and_grow() {
        let long = "x".repeat(300);
        for utf8 in [false, true] {
            let b = pool_bytes(&["label", "package", "Frogger 2: Swampy's Revenge (Rust)", &long, "é"], utf8);
            assert_eq!(b.len() % 4, 0);
            let (mut p, size) = StringPool::parse(&b).unwrap();
            assert_eq!(size, b.len());
            assert_eq!(p.strings[3], long);
            assert_eq!(p.strings[4], "é");
            assert_eq!(p.encode(), b, "re-encodes identically");
            assert_eq!(p.intern("package", 0), 1);
            assert_eq!(p.intern("package", 2), 5, "names below min_index are not reused");
            assert_eq!(p.intern("Frogger 2", 0), 6);
            let e = p.encode();
            assert_eq!(e.len() % 4, 0);
            let (q, qsize) = StringPool::parse(&e).unwrap();
            assert_eq!(qsize, e.len());
            assert_eq!(q.strings.last().unwrap(), "Frogger 2");
            assert_eq!(q.strings.len(), 7);
        }
    }

    #[test]
    fn styles_survive() {
        let mut p = StringPool {
            strings: vec!["a".into(), "b".into()],
            utf8: true,
            flags: UTF8_FLAG,
            style_offsets: vec![0],
            styles: vec![1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff],
        };
        let (q, _) = StringPool::parse(&p.encode()).unwrap();
        assert_eq!(q, p);
        p.intern("c", 0);
        let (q, _) = StringPool::parse(&p.encode()).unwrap();
        assert_eq!(q.styles, p.styles);
        assert_eq!(q.strings.len(), 3);
    }

    /// An attribute value for [`build_xml`].
    pub(crate) enum Val<'a> {
        Str(&'a str),
        Int(u32),
    }

    /// A flat binary XML document (start elements only, which is all the edits look at):
    /// `(element, [(android namespace?, attribute, value)])`. Android-namespace attribute
    /// names come first in the pool and are covered by the resource map, like aapt2 does.
    pub(crate) type Element<'a> = (&'a str, Vec<(bool, &'a str, Val<'a>)>);

    pub(crate) fn build_xml(elements: &[Element]) -> Vec<u8> {
        let mut strings: Vec<String> = Vec::new();
        let idx = |strings: &mut Vec<String>, s: &str| -> u32 {
            match strings.iter().position(|x| x == s) {
                Some(i) => i as u32,
                None => {
                    strings.push(s.into());
                    (strings.len() - 1) as u32
                }
            }
        };
        for (_, attrs) in elements {
            for (ns, name, _) in attrs {
                if *ns {
                    idx(&mut strings, name);
                }
            }
        }
        let res_map_len = strings.len();
        let android = idx(&mut strings, "http://schemas.android.com/apk/res/android");
        let mut chunks = Vec::new();
        for (el, attrs) in elements {
            let el_i = idx(&mut strings, el);
            let mut e = Vec::new();
            e.extend_from_slice(&RES_XML_START_ELEMENT_TYPE.to_le_bytes());
            e.extend_from_slice(&16u16.to_le_bytes());
            e.extend_from_slice(&((16 + 20 + 20 * attrs.len()) as u32).to_le_bytes());
            e.extend_from_slice(&1u32.to_le_bytes());
            e.extend_from_slice(&NO_INDEX.to_le_bytes());
            e.extend_from_slice(&NO_INDEX.to_le_bytes());
            e.extend_from_slice(&el_i.to_le_bytes());
            for v in [20u16, 20, attrs.len() as u16, 0, 0, 0] {
                e.extend_from_slice(&v.to_le_bytes());
            }
            for (ns, name, val) in attrs {
                let name_i = idx(&mut strings, name);
                let (raw, ty, data) = match val {
                    Val::Str(s) => {
                        let i = idx(&mut strings, s);
                        (i, TYPE_STRING, i)
                    }
                    Val::Int(v) => (NO_INDEX, TYPE_INT_DEC, *v),
                };
                for v in [if *ns { android } else { NO_INDEX }, name_i, raw] {
                    e.extend_from_slice(&v.to_le_bytes());
                }
                e.extend_from_slice(&[8, 0, 0, ty]);
                e.extend_from_slice(&data.to_le_bytes());
            }
            chunks.extend(e);
        }
        let refs: Vec<&str> = strings.iter().map(|s| s.as_str()).collect();
        let pool = pool_bytes(&refs, false);
        let mut resmap = Vec::new();
        resmap.extend_from_slice(&0x0180u16.to_le_bytes());
        resmap.extend_from_slice(&8u16.to_le_bytes());
        resmap.extend_from_slice(&((8 + 4 * res_map_len) as u32).to_le_bytes());
        for i in 0..res_map_len {
            resmap.extend_from_slice(&(0x0101_0000u32 + i as u32).to_le_bytes());
        }
        let body: Vec<u8> = [pool, resmap, chunks].concat();
        let mut out = Vec::new();
        out.extend_from_slice(&RES_XML_TYPE.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// `<manifest versionCode=1 package="a.b"><application label="Old"/>`.
    fn tiny_manifest() -> Vec<u8> {
        build_xml(&[
            ("manifest", vec![(true, "versionCode", Val::Int(1)), (false, "package", Val::Str("a.b"))]),
            ("application", vec![(true, "label", Val::Str("Old"))]),
        ])
    }

    #[test]
    fn manifest_edits() {
        let mut a = Axml::parse(&tiny_manifest()).unwrap();
        assert_eq!(a.get_string("manifest", "package").unwrap(), "a.b");
        assert_eq!(a.get_string("application", "label").unwrap(), "Old");
        a.set_string("manifest", "package", "a.b.game").unwrap();
        a.set_string("application", "label", "Frogger 2").unwrap();
        // "label" is an attribute name (resource map): a value "label" gets its own string.
        a.set_string("application", "label", "label").unwrap();
        a.set_string("application", "label", "Frogger 2").unwrap();
        a.set_int("manifest", "versionCode", 1000).unwrap();
        let bytes = a.encode();
        assert_eq!(u32le(&bytes, 4) as usize, bytes.len());
        let mut b = Axml::parse(&bytes).unwrap();
        assert_eq!(b.get_string("manifest", "package").unwrap(), "a.b.game");
        assert_eq!(b.get_string("application", "label").unwrap(), "Frogger 2");
        assert_eq!(b.value("manifest", "versionCode").unwrap(), (TYPE_INT_DEC, 1000));
        let orig = Axml::parse(&tiny_manifest()).unwrap().pool.strings;
        assert_eq!(b.pool.strings.len(), orig.len() + 3);
        assert_eq!(&b.pool.strings[..orig.len()], &orig[..], "existing indices unchanged");
        assert!(b.set_string("activity", "name", "x").is_err());
    }
}
