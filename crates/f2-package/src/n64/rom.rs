//! N64 ROM assembly: a pure-Rust port of what libdragon's `n64.mk` does after linking,
//! `n64tool --toc --title T --output ROM --align 256 ELF.stripped ELF.sym GAME.dfs` followed by
//! `ed64romconfig` (libdragon `tools/n64tool.c`, `tools/ed64romconfig.c`), byte for byte.
//!
//! Layout written by `n64tool` (offsets in the file):
//! * `0 .. header_size`: the IPL3 boot code with the ROM header in its first 64 bytes
//!   (libdragon's `default_ipl3`, 6444 bytes for the pinned libdragon). n64tool patches the
//!   title (0x20, 20 bytes), category (0x3B), region (0x3E) and the iQue entry point (0x08,
//!   the ELF's first load address); `ed64romconfig` the EverDrive/advanced homebrew header
//!   bytes (0x34-0x37 controllers, 0x3C-0x3D "ED", 0x3F save type / Expansion Pak / region
//!   free). There is **no checksum to compute**: libdragon's IPL3 does not verify the
//!   cartridge CRC (the header's CRC words stay zero; IPL3 itself carries the signature the
//!   console's CIC check needs).
//! * with `--toc`: zeros up to a 16-byte boundary, then the 1024-byte table of contents
//!   ("TOC0", cookie, size 1024, entry size 64, count, then `{offset, size, name[56]}` per file,
//!   all big-endian); the runtime finds its ELF, symbols and DFS through it (`rompak`).
//! * each file aligned (relative to the 4096-byte logical header; the first, the compressed
//!   ELF, to 256 bytes, the others to 16).
//! * zero padding to a multiple of 16 KiB.
//!
//! The TOC cookie identifies the ROM (`~crc32` over every byte after the header, the zero
//! padding included, then over the TOC itself with a zero cookie field).

/// Size of the TOC.
pub const TOC_SIZE: usize = 1024;
const TOC_ALIGN: usize = 16;
const TOC_ENTRY_SIZE: usize = 64;
/// Files the TOC can list.
pub const TOC_MAX_ENTRIES: usize = (TOC_SIZE - 16) / TOC_ENTRY_SIZE;
/// Size of the TOC structure n64tool hashes and writes (`sizeof(toc)`).
const TOC_STRUCT_SIZE: usize = 16 + TOC_MAX_ENTRIES * TOC_ENTRY_SIZE;
/// Offset and size of the title in the header.
pub const TITLE_OFFSET: usize = 0x20;
pub const TITLE_SIZE: usize = 20;
/// n64tool's logical header size: alignment and padding are computed as if the header were
/// exactly this long.
const LOGICAL_HEADER: usize = 4096;
/// n64tool's default final padding (16 KiB: iQue, 64drive, EverDrive transfer sizes).
pub const DEFAULT_PAD_ALIGN: usize = 16384;
/// The largest N64 cartridge (64 MiB), the limit `tools/n64/build.sh` enforces.
pub const MAX_ROM_SIZE: u64 = 64 << 20;

static CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// `crc32_update` of libdragon `tools/common/crc32.c` (reflected CRC-32, no implicit
/// inversion).
pub fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = CRC_TABLE[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc
}

/// One file placed in the ROM.
#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    /// TOC name (n64tool uses the file's base name).
    pub name: &'a str,
    pub data: &'a [u8],
    /// `--align` before this file (n64tool's default: 16).
    pub align: usize,
}

/// A TOC entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TocEntry {
    pub name: String,
    pub offset: u32,
    pub size: u32,
}

struct Writer {
    out: Vec<u8>,
    /// n64tool's `total_bytes_written`: bytes after the logical 4096-byte header.
    total: usize,
    cookie: u32,
}

impl Writer {
    fn zeros(&mut self, n: usize) {
        self.out.resize(self.out.len() + n, 0);
        let z = [0u8; 1024];
        let mut left = n;
        while left > 0 {
            let k = left.min(z.len());
            self.cookie = crc32_update(self.cookie, &z[..k]);
            left -= k;
        }
        self.total += n;
    }

    fn data(&mut self, d: &[u8]) {
        self.out.extend_from_slice(d);
        self.cookie = crc32_update(self.cookie, d);
        self.total += d.len();
    }

    fn align(&mut self, a: usize) {
        if a > 0 && !self.total.is_multiple_of(a) {
            self.zeros(a - self.total % a);
        }
    }
}

/// Assembles a ROM like `n64tool [--toc] ... files` with the default padding (`pad_align`,
/// 0 = none), and returns it with the TOC filled in. `header` is the IPL3 and header block
/// as it should appear (title and the other header fields already set; use [`set_title`]
/// afterwards to change the title, it is outside every checksum).
pub fn assemble(header: &[u8], files: &[Input], toc: bool, pad_align: usize) -> Result<Vec<u8>, String> {
    assemble_with(header, false, files, toc, pad_align)
}

/// [`assemble`]; `header_hashed`: the header counts towards the TOC cookie, as when n64tool
/// copies it from a `--header` file (its built-in IPL3, the one `n64.mk` uses, does not).
pub fn assemble_with(header: &[u8], header_hashed: bool, files: &[Input], toc: bool, pad_align: usize) -> Result<Vec<u8>, String> {
    if header.len() < LOGICAL_HEADER {
        return Err(format!("IPL3 header is {} bytes (minimum {LOGICAL_HEADER})", header.len()));
    }
    if files.is_empty() {
        return Err("no files to put in the ROM".into());
    }
    if toc && files.len() > TOC_MAX_ENTRIES {
        return Err(format!("{} files do not fit the ROM's table of contents ({TOC_MAX_ENTRIES})", files.len()));
    }
    let total_size: usize = header.len() + TOC_SIZE + files.iter().map(|f| f.data.len() + f.align).sum::<usize>() + pad_align;
    let mut w = Writer { out: Vec::with_capacity(total_size), total: header.len() - LOGICAL_HEADER, cookie: 0xFFFF_FFFF };
    w.out.extend_from_slice(header);
    if header_hashed {
        w.cookie = crc32_update(w.cookie, header);
    }
    let mut toc_offset = 0;
    if toc {
        w.align(TOC_ALIGN);
        toc_offset = w.out.len();
        w.zeros(TOC_SIZE);
    }
    let mut entries = Vec::new();
    for f in files {
        w.align(f.align.max(1));
        let offset = w.out.len();
        w.data(f.data);
        let base = f.name.rsplit(['/', '\\']).next().unwrap_or(f.name);
        entries.push(TocEntry { name: base.to_string(), offset: offset as u32, size: f.data.len() as u32 });
    }
    // Final padding: n64tool pads the logical size to at least pad_align - 4096, then the
    // whole ROM to a multiple of pad_align.
    if pad_align > 0 {
        let declared = LOGICAL_HEADER.next_multiple_of(pad_align) - LOGICAL_HEADER;
        if declared > w.total {
            w.zeros(declared - w.total);
        } else if !(w.total + LOGICAL_HEADER).is_multiple_of(pad_align) {
            w.zeros(pad_align - (w.total + LOGICAL_HEADER) % pad_align);
        }
    }
    if toc {
        let mut t = toc_struct(&entries, 0);
        let cookie = !crc32_update(w.cookie, &t);
        t[4..8].copy_from_slice(&cookie.to_be_bytes());
        w.out[toc_offset..toc_offset + TOC_STRUCT_SIZE].copy_from_slice(&t);
    }
    Ok(w.out)
}

/// The TOC structure (`struct toc_s`, big-endian) with the given cookie.
fn toc_struct(entries: &[TocEntry], cookie: u32) -> Vec<u8> {
    let mut t = Vec::with_capacity(TOC_STRUCT_SIZE);
    t.extend_from_slice(b"TOC0");
    t.extend_from_slice(&cookie.to_be_bytes());
    t.extend_from_slice(&(TOC_SIZE as u32).to_be_bytes());
    t.extend_from_slice(&(TOC_ENTRY_SIZE as u16).to_be_bytes());
    t.extend_from_slice(&(entries.len() as u16).to_be_bytes());
    for e in entries {
        t.extend_from_slice(&e.offset.to_be_bytes());
        t.extend_from_slice(&e.size.to_be_bytes());
        // strlcpy into name[56]
        let mut name = [0u8; TOC_ENTRY_SIZE - 8];
        let n = e.name.len().min(name.len() - 1);
        name[..n].copy_from_slice(&e.name.as_bytes()[..n]);
        t.extend_from_slice(&name);
    }
    t.resize(TOC_STRUCT_SIZE, 0);
    t
}

/// Finds and parses the TOC of a libdragon ROM: `(offset, cookie, entries)`.
pub fn read_toc(rom: &[u8]) -> Option<(usize, u32, Vec<TocEntry>)> {
    let be32 = |o: usize| u32::from_be_bytes(rom[o..o + 4].try_into().unwrap());
    let be16 = |o: usize| u16::from_be_bytes(rom[o..o + 2].try_into().unwrap());
    let mut at = LOGICAL_HEADER;
    while at + TOC_SIZE <= rom.len() && at < 0x10000 {
        if &rom[at..at + 4] == b"TOC0" && be32(at + 8) as usize == TOC_SIZE && be16(at + 12) as usize == TOC_ENTRY_SIZE {
            let n = be16(at + 14) as usize;
            if n > TOC_MAX_ENTRIES {
                return None;
            }
            let entries = (0..n)
                .map(|i| {
                    let e = at + 16 + i * TOC_ENTRY_SIZE;
                    let name = &rom[e + 8..e + TOC_ENTRY_SIZE];
                    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
                    TocEntry { name: String::from_utf8_lossy(&name[..len]).into_owned(), offset: be32(e), size: be32(e + 4) }
                })
                .collect();
            return Some((at, be32(at + 4), entries));
        }
        at += TOC_ALIGN;
    }
    None
}

/// The ROM header title for a game name: upper case, printable ASCII only (others become
/// spaces), at most 20 characters, like `N64_ROM_TITLE = "FROGGER 2"` in tools/n64/Makefile.
pub fn rom_title(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_ascii_graphic() || c == ' ' { c.to_ascii_uppercase() } else { ' ' }).collect();
    s.chars().take(TITLE_SIZE).collect::<String>().trim_end().to_string()
}

/// Writes the 20-byte title field (zero padded, truncated like n64tool's `--title`).
pub fn set_title(rom: &mut [u8], title: &str) {
    let f = &mut rom[TITLE_OFFSET..TITLE_OFFSET + TITLE_SIZE];
    f.fill(0);
    let n = title.len().min(TITLE_SIZE);
    f[..n].copy_from_slice(&title.as_bytes()[..n]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_standard_crc32() {
        // `~crc32_update(~0, "123456789")` = the CRC-32 check value
        assert_eq!(!crc32_update(0xFFFF_FFFF, b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn titles() {
        assert_eq!(rom_title("Frogger 2"), "FROGGER 2");
        assert_eq!(rom_title("Frogger 2: Swampy's Revenge"), "FROGGER 2: SWAMPY'S");
        assert_eq!(rom_title("Caf\u{e9}"), "CAF");
        let mut h = vec![0xAAu8; 64];
        set_title(&mut h, "FROGGER 2");
        assert_eq!(&h[0x20..0x34], b"FROGGER 2\0\0\0\0\0\0\0\0\0\0\0");
        assert_eq!(h[0x1F], 0xAA);
        assert_eq!(h[0x34], 0xAA);
    }

    fn header(len: usize) -> Vec<u8> {
        let mut h: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
        set_title(&mut h, "TEST");
        // n64tool always writes the category and region
        h[0x3B] = b'N';
        h[0x3E] = 0;
        h
    }

    /// Golden output of the real tool for a synthetic case: `n64tool --toc --title TEST
    /// --header hdr.bin --output t.z64 --align 256 a.bin --align 16 b.bin --align 16 c.dfs`
    /// with hdr.bin = 6444 bytes `(i*7+3) & 0xFF` (title TEST, category N, region 0), a.bin =
    /// 5000 bytes `i & 0xFF`, b.bin = 300 bytes `0x5A`, c.dfs = 17 bytes `0..17` (`--header`
    /// makes n64tool hash the header into the cookie). Checked: size, TOC entries and the
    /// cookie; byte for byte when n64tool is installed.
    #[test]
    fn golden_synthetic_rom() {
        let a: Vec<u8> = (0..5000).map(|i| i as u8).collect();
        let b = vec![0x5Au8; 300];
        let c: Vec<u8> = (0..17).collect();
        let files = [Input { name: "a.bin", data: &a, align: 256 }, Input { name: "b.bin", data: &b, align: 16 }, Input { name: "c.dfs", data: &c, align: 16 }];
        let rom = assemble_with(&header(6444), true, &files, true, DEFAULT_PAD_ALIGN).unwrap();
        let (at, cookie, toc) = read_toc(&rom).unwrap();
        assert_eq!(at, 6448);
        assert_eq!(rom.len(), GOLDEN_LEN);
        assert_eq!(toc.iter().map(|e| (e.name.as_str(), e.offset, e.size)).collect::<Vec<_>>(), GOLDEN_TOC);
        assert_eq!(cookie, GOLDEN_COOKIE);
        if let Some(ref_rom) = n64tool_reference(&header(6444), &files) {
            assert!(ref_rom == rom, "differs from n64tool");
        }
    }

    const GOLDEN_LEN: usize = 16384;
    const GOLDEN_TOC: [(&str, u32, u32); 3] = [("a.bin", 7680, 5000), ("b.bin", 12688, 300), ("c.dfs", 12992, 17)];
    const GOLDEN_COOKIE: u32 = 0x13A7_0C1D;

    /// Runs the real n64tool on the same inputs when it is installed.
    fn n64tool_reference(header: &[u8], files: &[Input]) -> Option<Vec<u8>> {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
        let tool = std::env::var_os("N64_INST")
            .map(|i| std::path::PathBuf::from(i).join("bin/n64tool"))
            .into_iter()
            .chain(["opt/n64root-3a", "opt/n64root-3p", "opt/n64root"].iter().map(|v| home.join(v).join("opt/libdragon/bin/n64tool")))
            .find(|p| p.is_file())?;
        let dir = std::env::temp_dir().join(format!("f2-rom-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(dir.join("hdr.bin"), header).ok()?;
        let mut cmd = std::process::Command::new(tool);
        cmd.args(["--toc", "--title", "TEST", "--header"]).arg(dir.join("hdr.bin")).arg("--output").arg(dir.join("t.z64"));
        for f in files {
            std::fs::write(dir.join(f.name), f.data).ok()?;
            cmd.arg("--align").arg(f.align.to_string()).arg(dir.join(f.name));
        }
        let ok = cmd.output().ok()?.status.success();
        let rom = std::fs::read(dir.join("t.z64")).ok();
        let _ = std::fs::remove_dir_all(&dir);
        if ok {
            rom
        } else {
            None
        }
    }

    #[test]
    fn padding_and_alignment_rules() {
        // a ROM whose logical size is already a multiple of 16 KiB gets no padding
        let h = header(4096);
        let d = vec![1u8; 16384 - 1024 - 4096];
        let rom = assemble(&h, &[Input { name: "x", data: &d, align: 16 }], true, DEFAULT_PAD_ALIGN).unwrap();
        assert_eq!(rom.len(), 16384);
        let rom = assemble(&h, &[Input { name: "x", data: &d[..10], align: 16 }], true, 0).unwrap();
        assert_eq!(rom.len(), 4096 + 1024 + 10);
        assert!(assemble(&h[..100], &[Input { name: "x", data: &d, align: 16 }], true, 0).is_err());
    }
}
