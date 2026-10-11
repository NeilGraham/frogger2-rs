//! Texture size histogram of `.fla` banks (sizing the N64 texture budget).
fn main() {
    for p in std::env::args().skip(1) {
        let b = std::fs::read(&p).unwrap();
        let b = f2_formats::fla::maybe_decompress(b).unwrap();
        let bank = f2_formats::texture::TexBank::parse(&b).unwrap();
        let mut hist = std::collections::BTreeMap::new();
        let mut tot = 0;
        for t in &bank.textures {
            *hist.entry((t.width, t.height)).or_insert(0) += 1;
            tot += t.width as usize * t.height as usize * 2;
        }
        println!("{p}: {} textures, {} KiB 16-bit: {:?}", bank.textures.len(), tot / 1024, hist);
    }
}
