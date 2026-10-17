//! The PS5 fake package (`.pkg`), a pure-Rust port of the "nwonly" debug-image path of
//! LibProsperoPkg 1.2.0 as `ffpfsc-pkg-tool build --deterministic` drives it (GPL-3.0;
//! <https://github.com/drakmor/LibProsperoPkg>).

pub(crate) mod inner;
pub(crate) mod naps;
pub(crate) mod outer;
pub(crate) mod pfs;

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds the inner image of the files under `F2_INNER_DIR` (an `extract-inner` of an
    /// oracle package, `source: pfs` files only) into `F2_INNER_OUT`, and its NAPS layout into
    /// `F2_INNER_OUT.naps`.
    #[test]
    #[ignore]
    fn inner_vector() {
        let dir = std::path::PathBuf::from(std::env::var("F2_INNER_DIR").unwrap());
        let out = std::path::PathBuf::from(std::env::var("F2_INNER_OUT").unwrap());
        let mut files = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = format!("/{}", p.strip_prefix(&dir).unwrap().to_string_lossy());
                    if inner::excluded_from_inner(&rel) {
                        continue;
                    }
                    let len = std::fs::metadata(&p).unwrap().len() as i64;
                    files.push(inner::InputFile { path: rel, len, source: inner::Source::Path(p) });
                }
            }
        }
        let img = inner::build(files, &[], &out, &|m| eprintln!("{m}")).unwrap();
        eprintln!("image {} ndblock {} meta_base {} meta_on_disk {}", img.image_length, img.ndblock, img.meta_base_logical, img.metadata_on_disk);
        let naps = naps::generate(&img).unwrap();
        std::fs::write(out.with_extension("naps"), &naps).unwrap();
        let files = vec![
            outer::OuterFile { name: "pfs_image.dat", len: img.image_length as u64, size_compressed: img.ndblock as u64 * 65536, reader: Box::new(std::fs::File::open(&out).unwrap()) },
            outer::OuterFile { name: "naps_pkg_layout.dat", len: naps.len() as u64, size_compressed: naps.len() as u64, reader: Box::new(&naps[..]) },
        ];
        let mut f = std::fs::File::create(out.with_extension("outer")).unwrap();
        let o = outer::build(&mut f, 0, files, &mut |_, _, _| {}).unwrap();
        eprintln!("outer {} blocks", o.pfs_size / 65536);
    }
}
