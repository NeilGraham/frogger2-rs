//! Prints the sfx size distribution (by loop flag and duration) from the pack index. Dev tool.
use f2_n64pack::audio::index::SfxIndex;

fn main() {
    let idx = SfxIndex::read(&std::fs::read("target/n64/pack/sfx/index.bin").unwrap()).unwrap();
    let mut rows: Vec<(String, u64, u64)> = Vec::new();
    for s in &idx.samples {
        let secs = f64::from(s.src_len) / f64::from(s.src_rate);
        let class = format!(
            "{} {} {}",
            if s.looped { "loop" } else { "once" },
            s.codec.name(),
            if secs < 0.5 {
                "<0.5s"
            } else if secs < 1.0 {
                "<1s"
            } else if secs < 2.0 {
                "<2s"
            } else if secs < 5.0 {
                "<5s"
            } else {
                ">=5s"
            }
        );
        match rows.iter_mut().find(|r| r.0 == class) {
            Some(r) => {
                r.1 += 1;
                r.2 += u64::from(s.file_bytes);
            }
            None => rows.push((class, 1, u64::from(s.file_bytes))),
        }
    }
    rows.sort();
    for r in rows {
        println!("{:24} n={:4} bytes={}", r.0, r.1, r.2);
    }
}
