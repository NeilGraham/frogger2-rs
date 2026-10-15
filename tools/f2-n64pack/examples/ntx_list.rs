//! Lists the entries of the NTX1 texture banks the packer makes (debug aid for the N64
//! runtime): name, CRC, image format, flags, stored and PC size.
//!
//! `cargo run --release -p f2-n64pack --example ntx_list -- <rendition> [bank-name-substring]`
use f2_n64pack::textures::NtxFile;

fn main() {
    let rendition = std::path::PathBuf::from(std::env::args().nth(1).expect("rendition dir"));
    let filter = std::env::args().nth(2).unwrap_or_default();
    let ctx = f2_n64pack::Ctx {
        rendition,
        out: std::env::temp_dir().join("ntx_list_out"),
        cache: std::env::temp_dir().join("ntx_list_cache"),
        tools: f2_n64pack::ctx::default_tools(),
        jobs: 8,
        quick: false,
        portable: false,
    };
    let (assets, _) = f2_n64pack::textures::convert(&ctx).expect("convert");
    let files: std::collections::BTreeMap<String, Vec<u8>> = assets.into_iter().map(|a| (a.path, a.data)).collect();
    for (path, data) in &files {
        if !path.contains(&filter) {
            continue;
        }
        let f = NtxFile::parse(data).expect("parse");
        println!("== {path}: {} images, {} entries, deps {:?}", f.images.len(), f.entries.len(), f.deps);
        for e in &f.entries {
            let name = String::from_utf8_lossy(&e.name).trim_end_matches('\0').to_string();
            let d = if e.dep == 0 {
                Some(f.images[e.image as usize].clone())
            } else {
                files.get(&format!("textures/{}", f.deps[e.dep as usize - 1])).and_then(|b| NtxFile::parse(b).ok()).map(|p| p.images[e.image as usize].clone())
            };
            if let Some(d) = d {
                println!(
                    "{name:12} {:08x} dep {} {:?} kind {} flags {:#06x} {}x{} (pc {}x{}) tile {}x{}",
                    e.crc, e.dep, d.fmt, d.kind, d.flags, d.width, d.height, d.orig_width, d.orig_height, d.tile_w, d.tile_h
                );
            }
        }
    }
}
