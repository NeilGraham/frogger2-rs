//! The N64 runtime (docs/EXPORT.md "N64"): everything of the ROM except the game data, built
//! by CI with libdragon (`tools/runtime/build-n64.sh`) and published as
//! `runtime-n64-<version>.zip`:
//!
//! ```text
//! runtime.json            {"target":"n64","version":..,"format":1,"rom":{..},"pack":{..},"build":{..}}
//! ipl3-header.bin         IPL3 boot code + ROM header as n64tool/ed64romconfig wrote them
//! frogger2.elf.stripped   the engine: linked, stripped, n64elfcompress'ed ELF (TOC entry 0)
//! frogger2.elf.sym        n64sym symbol table (backtraces on the crash screen)
//! ```
//!
//! `rom.files` lists the TOC entries in ROM order with their alignment; the one with
//! `"dfs": true` is the game's DragonFS image, which the exporter builds from the player's
//! data. Re-assembling the same files with the DFS of a `tools/n64/build.sh` build gives that
//! build's ROM byte for byte (`Runtime::from_rom` checks exactly this when the runtime is made).

use super::rom::{self, Input};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

/// `runtime.json` `format` this exporter reads.
pub const FORMAT: u32 = 1;
/// Name of the manifest inside the runtime.
pub const MANIFEST: &str = "runtime.json";
/// Name of the IPL3 + header block inside the runtime.
pub const HEADER_FILE: &str = "ipl3-header.bin";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub target: String,
    pub version: String,
    pub format: u32,
    pub rom: RomSpec,
    pub pack: PackSpec,
    /// Provenance (git revision, libdragon prefix, crate features).
    #[serde(default)]
    pub build: BTreeMap<String, String>,
}

/// How the ROM is assembled (n64tool's inputs and flags).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RomSpec {
    /// IPL3 + header file (title etc. already set, the title is replaced by the exporter).
    pub header: String,
    /// `n64tool --toc`.
    pub toc: bool,
    /// Final padding (n64tool's 16 KiB).
    pub pad_align: usize,
    /// Largest ROM allowed (64 MiB, the cartridge size `tools/n64/build.sh` enforces).
    pub max_size: u64,
    /// TOC entries in ROM order.
    pub files: Vec<RomFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RomFile {
    /// TOC name.
    pub name: String,
    /// The file inside the runtime (absent for the DFS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The game data's DragonFS image, built by the exporter.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dfs: bool,
    /// n64tool `--align` before this file.
    pub align: usize,
}

/// How the exporter builds the DFS (f2-n64pack settings; `tools/n64/build.sh --portable` uses
/// the same, so both give the same ROM).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackSpec {
    /// "portable": f2-n64pack `--portable` (pure-Rust encoders).
    pub profile: String,
    /// Cartridge size the pack is budgeted for (f2-n64pack `--cart-mib`).
    pub cart_bytes: u64,
    /// Space kept for the boot code + ELF (f2-n64pack `--reserve-mib`).
    pub code_reserve: u64,
}

impl PackSpec {
    /// f2-n64pack's defaults (what `tools/n64/build.sh` passes).
    pub fn default_portable() -> PackSpec {
        PackSpec { profile: "portable".into(), cart_bytes: 64 << 20, code_reserve: (2.5 * 1_048_576.0) as u64 }
    }
}

/// A loaded runtime: the manifest and its files.
pub struct Runtime {
    pub manifest: Manifest,
    files: BTreeMap<String, Vec<u8>>,
}

fn safe_name(name: &str) -> bool {
    !name.is_empty() && !name.contains("..") && !name.starts_with('/') && !name.contains('\\') && !name.contains(':')
}

impl Runtime {
    /// Loads a runtime from its zip (the release asset) or an extracted directory.
    pub fn load(path: &Path) -> Result<Runtime, String> {
        let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let mut files = BTreeMap::new();
        if path.is_dir() {
            let m = std::fs::read(path.join(MANIFEST)).map_err(|e| err(&e))?;
            files.insert(MANIFEST.to_string(), m);
        } else {
            let f = std::fs::File::open(path).map_err(|e| err(&e))?;
            let mut z = zip::ZipArchive::new(f).map_err(|e| err(&e))?;
            for i in 0..z.len() {
                let mut e = z.by_index(i).map_err(|e| err(&e))?;
                if e.is_dir() {
                    continue;
                }
                let name = e.name().to_string();
                let mut b = Vec::with_capacity(e.size() as usize);
                e.read_to_end(&mut b).map_err(|e| err(&e))?;
                files.insert(name, b);
            }
        }
        let manifest: Manifest = serde_json::from_slice(files.get(MANIFEST).ok_or_else(|| err(&"no runtime.json: not an N64 runtime"))?)
            .map_err(|e| err(&format!("runtime.json: {e}")))?;
        if manifest.target != "n64" {
            return Err(err(&format!("a runtime for {}, not n64", manifest.target)));
        }
        if manifest.format != FORMAT {
            return Err(err(&format!("runtime format {} (this exporter reads {FORMAT}): use the runtime of this app's version", manifest.format)));
        }
        let mut wanted = vec![manifest.rom.header.clone()];
        wanted.extend(manifest.rom.files.iter().filter_map(|f| f.path.clone()));
        for name in wanted {
            if !safe_name(&name) {
                return Err(err(&format!("bad file name {name:?} in runtime.json")));
            }
            if let std::collections::btree_map::Entry::Vacant(v) = files.entry(name.clone()) {
                if !path.is_dir() {
                    return Err(err(&format!("{name} missing")));
                }
                v.insert(std::fs::read(path.join(&name)).map_err(|e| err(&format!("{name}: {e}")))?);
            }
        }
        if manifest.rom.files.iter().filter(|f| f.dfs).count() != 1 {
            return Err(err(&"runtime.json must name exactly one DFS entry"));
        }
        Ok(Runtime { manifest, files })
    }

    /// A file of the runtime.
    pub fn file(&self, name: &str) -> Result<&[u8], String> {
        self.files.get(name).map(Vec::as_slice).ok_or_else(|| format!("runtime file {name} missing"))
    }

    /// The ROM for a DFS image, with `title` in the header (`None`: the runtime's).
    pub fn assemble(&self, dfs: &[u8], title: Option<&str>) -> Result<Vec<u8>, String> {
        let r = &self.manifest.rom;
        let mut inputs = Vec::with_capacity(r.files.len());
        for f in &r.files {
            let data = if f.dfs { dfs } else { self.file(f.path.as_deref().ok_or_else(|| format!("runtime.json: {} has no path", f.name))?)? };
            inputs.push(Input { name: &f.name, data, align: f.align });
        }
        let mut out = rom::assemble(self.file(&r.header)?, &inputs, r.toc, r.pad_align)?;
        if let Some(t) = title {
            rom::set_title(&mut out, t);
        }
        Ok(out)
    }

    /// Makes a runtime from a ROM built by `tools/n64/build.sh` (normally the data-free
    /// `--runtime` build): its header and every TOC file except the DFS. `aligns` are the
    /// n64tool `--align` values per TOC entry (`n64.mk`: 256 for the ELF, then 16). Fails
    /// unless re-assembling the parts with the ROM's own DFS reproduces the ROM byte for byte
    /// (which also determines the exact IPL3 size).
    pub fn from_rom(rom_bytes: &[u8], version: &str, aligns: &[usize], pack: PackSpec, build: BTreeMap<String, String>) -> Result<Runtime, String> {
        let (toc_at, _, entries) = rom::read_toc(rom_bytes).ok_or("no libdragon table of contents (TOC0) in the ROM")?;
        if aligns.len() != entries.len() {
            return Err(format!("{} alignments for {} TOC entries ({:?})", aligns.len(), entries.len(), entries.iter().map(|e| &e.name).collect::<Vec<_>>()));
        }
        let dfs_count = entries.iter().filter(|e| e.name.ends_with(".dfs")).count();
        if dfs_count != 1 {
            return Err(format!("{dfs_count} DFS entries in the ROM's TOC"));
        }
        let slice = |e: &rom::TocEntry| {
            rom_bytes.get(e.offset as usize..e.offset as usize + e.size as usize).ok_or_else(|| format!("TOC entry {} outside the ROM", e.name))
        };
        let inputs: Vec<Input> =
            entries.iter().zip(aligns).map(|(e, &a)| Ok(Input { name: &e.name, data: slice(e)?, align: a })).collect::<Result<_, String>>()?;
        // n64tool zero-pads the header to 16 bytes before the TOC: try each possible IPL3 size.
        let mut header_size = None;
        for k in 0..16 {
            let hs = toc_at - k;
            if hs < 4096 || rom_bytes[hs..toc_at].iter().any(|&b| b != 0) {
                break;
            }
            if rom::assemble(&rom_bytes[..hs], &inputs, true, rom::DEFAULT_PAD_ALIGN)? == rom_bytes {
                header_size = Some(hs);
                break;
            }
        }
        let hs = header_size.ok_or("re-assembling the ROM's parts does not reproduce it (unexpected n64tool flags?)")?;
        let mut files = BTreeMap::new();
        files.insert(HEADER_FILE.to_string(), rom_bytes[..hs].to_vec());
        let mut rfiles = Vec::new();
        for (e, &a) in entries.iter().zip(aligns) {
            if e.name.ends_with(".dfs") {
                rfiles.push(RomFile { name: e.name.clone(), path: None, dfs: true, align: a });
            } else {
                if !safe_name(&e.name) || e.name == MANIFEST || e.name == HEADER_FILE {
                    return Err(format!("unexpected TOC entry name {:?}", e.name));
                }
                files.insert(e.name.clone(), slice(e)?.to_vec());
                rfiles.push(RomFile { name: e.name.clone(), path: Some(e.name.clone()), dfs: false, align: a });
            }
        }
        let manifest = Manifest {
            target: "n64".into(),
            version: version.into(),
            format: FORMAT,
            rom: RomSpec { header: HEADER_FILE.into(), toc: true, pad_align: rom::DEFAULT_PAD_ALIGN, max_size: rom::MAX_ROM_SIZE, files: rfiles },
            pack,
            build,
        };
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
        files.insert(MANIFEST.into(), json);
        Ok(Runtime { manifest, files })
    }

    /// Writes the runtime as a zip (deflated, fixed timestamps: reproducible).
    pub fn write_zip(&self, path: &Path) -> Result<(), String> {
        let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let f = std::fs::File::create(path).map_err(|e| err(&e))?;
        let mut z = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        // the manifest first, then the ROM parts in ROM order
        let mut order = vec![MANIFEST.to_string(), self.manifest.rom.header.clone()];
        order.extend(self.manifest.rom.files.iter().filter_map(|f| f.path.clone()));
        for name in order {
            z.start_file(name.as_str(), o).map_err(|e| err(&e))?;
            z.write_all(self.file(&name)?).map_err(|e| err(&e))?;
        }
        z.finish().map_err(|e| err(&e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rom(dfs: &[u8]) -> Vec<u8> {
        let header: Vec<u8> = (0..6444).map(|i| (i % 251) as u8).collect();
        let elf = vec![7u8; 3000];
        let sym = vec![9u8; 100];
        rom::assemble(
            &header,
            &[
                Input { name: "game.elf.stripped", data: &elf, align: 256 },
                Input { name: "game.elf.sym", data: &sym, align: 16 },
                Input { name: "game.dfs", data: dfs, align: 16 },
            ],
            true,
            rom::DEFAULT_PAD_ALIGN,
        )
        .unwrap()
    }

    #[test]
    fn runtime_round_trip() {
        let rom_a = sample_rom(&[1, 2, 3]);
        let rt = Runtime::from_rom(&rom_a, "0.1.0", &[256, 16, 16], PackSpec::default_portable(), BTreeMap::new()).unwrap();
        assert_eq!(rt.file(HEADER_FILE).unwrap().len(), 6444, "IPL3 size found");
        // another DFS gives the ROM n64tool would have made with it
        let dfs_b: Vec<u8> = (0..5000).map(|i| (i * 13) as u8).collect();
        assert!(rt.assemble(&dfs_b, None).unwrap() == sample_rom(&dfs_b));
        let titled = rt.assemble(&dfs_b, Some("FROGGER 2")).unwrap();
        assert_eq!(&titled[0x20..0x29], b"FROGGER 2");
        // zip round trip
        let dir = std::env::temp_dir().join(format!("f2-n64rt-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zp = dir.join("runtime-n64-0.1.0.zip");
        rt.write_zip(&zp).unwrap();
        let back = Runtime::load(&zp).unwrap();
        assert_eq!(back.manifest.version, "0.1.0");
        assert!(back.assemble(&dfs_b, None).unwrap() == sample_rom(&dfs_b));
        // wrong alignments are caught
        assert!(Runtime::from_rom(&rom_a, "0.1.0", &[16, 16, 16], PackSpec::default_portable(), BTreeMap::new()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
