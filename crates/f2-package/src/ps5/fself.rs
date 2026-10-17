//! Fake-signed SELFs for the PS5 package: the raw ELF modules of the runtime (`eboot.bin`,
//! `sce_module/libc.prx`) wrapped as the package builder does it (LibProsperoPkg 1.2.0,
//! `Content/ProsperoFself.cs` `MakeFself` with default options), so the loader of a
//! jailbroken console accepts them.

use sha2::{Digest, Sha256};

/// The SCE fake-SELF magic (little-endian `0xEEF51454`).
pub(crate) const MAGIC: u32 = 0xEEF5_1454;

pub(crate) fn is_elf(data: &[u8]) -> bool {
    data.len() >= 64 && data[..4] == *b"\x7fELF"
}

fn u16_at(d: &[u8], o: usize) -> usize {
    u16::from_le_bytes([d[o], d[o + 1]]) as usize
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn u64_at(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}

fn align16(v: usize) -> usize {
    (v + 15) & !15
}

/// The payload of ELF section `name`, if present.
fn find_section<'a>(elf: &'a [u8], name: &str) -> &'a [u8] {
    let shoff = u64_at(elf, 40);
    let shentsize = u16_at(elf, 58);
    let shnum = u16_at(elf, 60);
    let shstrndx = u16_at(elf, 62);
    if shoff == 0 || shnum == 0 || shentsize < 64 || shstrndx >= shnum || shoff > i32::MAX as u64 {
        return &[];
    }
    let shoff = shoff as usize;
    if shoff + shnum * shentsize > elf.len() {
        return &[];
    }
    let payload = |h: usize| -> &'a [u8] {
        let (off, size) = (u64_at(elf, h + 24), u64_at(elf, h + 32));
        if off > i32::MAX as u64 || size > i32::MAX as u64 || off > elf.len() as u64 || size > elf.len() as u64 - off {
            return &[];
        }
        &elf[off as usize..(off + size) as usize]
    };
    let names = payload(shoff + shstrndx * shentsize);
    for i in 0..shnum {
        let h = shoff + i * shentsize;
        let at = u32_at(elf, h) as usize;
        if at >= names.len() {
            continue;
        }
        let rest = &names[at..];
        if let Some(z) = rest.iter().position(|&b| b == 0) {
            if &rest[..z] == name.as_bytes() {
                return payload(h);
            }
        }
    }
    &[]
}

/// Whether an ELF carries its own `.sceversion` section.
pub(crate) fn has_sce_version(elf: &[u8]) -> bool {
    !find_section(elf, ".sceversion").is_empty()
}

/// `MakeFself(elf, new FselfOptions())`.
pub(crate) fn make_fself(elf: &[u8]) -> Result<Vec<u8>, String> {
    if !is_elf(elf) {
        return Err("not an ELF file".into());
    }
    if elf[4] != 2 {
        return Err("only 64-bit ELF modules are supported".into());
    }
    let e_type = u16_at(elf, 16);
    if u64_at(elf, 32) != 64 {
        return Err("the ELF program headers must follow the ELF header".into());
    }
    let (phentsize, phnum) = (u16_at(elf, 54), u16_at(elf, 56));
    if phentsize != 56 || 64 + phnum * 56 > elf.len() {
        return Err("unexpected ELF program headers".into());
    }
    // Loadable content: PT_LOAD, module data, relro, comment.
    let mut segs: Vec<(usize, usize, usize)> = Vec::new();
    for i in 0..phnum {
        let p = 64 + i * 56;
        let ty = u32_at(elf, p);
        let off = u64_at(elf, p + 8) as i64;
        let size = u64_at(elf, p + 32) as i64;
        if size > 0 && off + size <= elf.len() as i64 && matches!(ty, 1 | 0x6100_0000 | 0x6100_0010 | 0x6FFF_FF00) {
            segs.push((i, off as usize, size as usize));
        }
    }
    if segs.is_empty() {
        return Err("the ELF has no loadable segment content".into());
    }
    let nseg = segs.len() * 2;
    let seg_table_end = 32 + nseg * 32;
    let elf_headers = 64 + phnum * 56;
    let ext_info = align16(seg_table_end + elf_headers);
    let header_size = ext_info + 64 + 48;
    let meta_size = nseg * 80 + 80 + 544;
    if header_size > 65535 || meta_size > 65535 {
        return Err("the ELF has too many segments to fake-sign".into());
    }
    let mut offsets = vec![0usize; nseg];
    let mut digests = vec![0usize; segs.len()];
    let mut at = header_size + meta_size;
    for (i, &(_, _, size)) in segs.iter().enumerate() {
        digests[i] = size.div_ceil(16384) * 32;
        offsets[i * 2] = at;
        offsets[i * 2 + 1] = at + digests[i];
        at = offsets[i * 2 + 1] + size;
        if i + 1 < segs.len() {
            at = align16(at);
        }
    }
    let file_size = at;
    let version = find_section(elf, ".sceversion");
    let mut out = vec![0u8; file_size + version.len()];
    out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    out[4..8].copy_from_slice(&[16, 1, 1, 18]);
    out[8..12].copy_from_slice(&0x1000_0101u32.to_le_bytes());
    out[12..14].copy_from_slice(&(header_size as u16).to_le_bytes());
    out[14..16].copy_from_slice(&(meta_size as u16).to_le_bytes());
    out[16..24].copy_from_slice(&(file_size as u64).to_le_bytes());
    out[24..26].copy_from_slice(&(nseg as u16).to_le_bytes());
    out[26..28].copy_from_slice(&50u16.to_le_bytes());
    let mut put_seg = |entry: usize, flags: u64, offset: usize, size: usize| {
        out[entry..entry + 8].copy_from_slice(&flags.to_le_bytes());
        out[entry + 8..entry + 16].copy_from_slice(&(offset as u64).to_le_bytes());
        out[entry + 16..entry + 24].copy_from_slice(&(size as u64).to_le_bytes());
        out[entry + 24..entry + 32].copy_from_slice(&(size as u64).to_le_bytes());
    };
    for (j, &(phdr, _, size)) in segs.iter().enumerate() {
        put_seg(32 + j * 2 * 32, (((j * 2 + 1) as u64) << 20) | 0x10004, offsets[j * 2], digests[j]);
        put_seg(32 + (j * 2 + 1) * 32, ((phdr as u64) << 20) | 0x2804, offsets[j * 2 + 1], size);
    }
    out[seg_table_end..seg_table_end + elf_headers].copy_from_slice(&elf[..elf_headers]);
    // Extended info: authority ID, program type, versions, the ELF's SHA-256.
    let exinfo = if 16128 < elf.len() { elf[16128] } else { 0 };
    let exec = matches!(e_type, 2 | 0xFE00 | 0xFE10);
    let authority: u64 = match (exinfo, exec) {
        (64, true) => 0x3100_0000_0000_1101,
        (64, false) => 0x3100_0000_0000_1102,
        (128, true) => 0x3100_0000_0000_1001,
        (128, false) => 0x3100_0000_0000_1002,
        (_, true) => 0x3100_0000_0000_0001,
        (_, false) => 0x3100_0000_0000_0002,
    };
    out[ext_info..ext_info + 8].copy_from_slice(&authority.to_le_bytes());
    out[ext_info + 8..ext_info + 16].copy_from_slice(&1u64.to_le_bytes());
    // App and firmware versions: 0.
    out[ext_info + 32..ext_info + 64].copy_from_slice(&Sha256::digest(elf));
    out[ext_info + 64..ext_info + 72].copy_from_slice(&3u64.to_le_bytes());
    let footer = header_size + nseg * 80 + 48;
    out[footer..footer + 4].copy_from_slice(&65536u32.to_le_bytes());
    for (k, &(_, off, size)) in segs.iter().enumerate() {
        let dst = offsets[k * 2 + 1];
        out[dst..dst + size].copy_from_slice(&elf[off..off + size]);
    }
    out[file_size..].copy_from_slice(version);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `$F2_FSELF_IN` (an ELF) fake-signed into `$F2_FSELF_OUT`, to compare with the package
    /// builder's output.
    #[test]
    #[ignore]
    fn vector() {
        let elf = std::fs::read(std::env::var("F2_FSELF_IN").unwrap()).unwrap();
        std::fs::write(std::env::var("F2_FSELF_OUT").unwrap(), make_fself(&elf).unwrap()).unwrap();
    }

    #[test]
    fn rejects_non_elf() {
        assert!(make_fself(&[0u8; 100]).is_err());
    }
}
