//! cargo run --release -p f2-media --example probe -- <image> [extract_dir]
use f2_media::{Disc, IsCab, IsoFs, Names, Vfs};
use std::path::Path;

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let t0 = std::time::Instant::now();
    let disc = Disc::open(Path::new(&args[1]))?;
    println!("container {} tracks {}", disc.container, disc.tracks.len());
    for t in &disc.tracks {
        println!("  track {:2} {:?} sectors {} start {} whole_file {} ({})", t.number, t.kind, t.sectors, t.start, t.whole_file, t.backing.name());
    }
    let mut iso = IsoFs::open(disc.data_source()?, Names::PreferJoliet)?;
    println!("volume {:?} files {} ({:?})", iso.info.volume_id, iso.entries().len(), t0.elapsed());
    let idx = iso.find("FROGGER2/DATA1.CAB").expect("no DATA1.CAB");
    let cab = IsCab::parse(iso.read_file(idx)?)?;
    println!("cab version {:#x} files {} groups {:?}", cab.version, cab.files.len(), cab.groups.iter().map(|g| &g.name).collect::<Vec<_>>());
    let mut ok = 0;
    for f in &cab.files {
        if !IsCab::is_valid(f) {
            continue;
        }
        let data = cab.extract(f)?;
        if let Some(dir) = args.get(2) {
            let p = Path::new(dir).join(f.path());
            std::fs::create_dir_all(p.parent().unwrap())?;
            std::fs::write(p, &data)?;
        }
        ok += 1;
    }
    println!("extracted {ok} files in {:?}", t0.elapsed());
    Ok(())
}
