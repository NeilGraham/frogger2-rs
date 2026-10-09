//! Decode frame N of a Bink file and write it as a PNG:
//! `cargo run -p f2-bink --example bink2png -- file.bik N out.png`

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: bink2png <file.bik> <frame> <out.png>");
        std::process::exit(2);
    }
    let data = std::fs::read(&args[1]).expect("read input");
    let n: u32 = args[2].parse().expect("frame number");
    let mut bink = f2_bink::BinkFile::open(data).expect("open bink");
    println!("{}x{} @ {}/{} fps, {} frames, audio {:?}", bink.width(), bink.height(), bink.fps().0, bink.fps().1, bink.frame_count(), bink.audio_tracks());
    if n >= bink.frame_count() {
        eprintln!("frame {n} out of range");
        std::process::exit(1);
    }
    let frame = loop {
        let f = bink.decode_next().expect("frame").expect("decode");
        if f.index == n {
            break f;
        }
    };
    let file = std::fs::File::create(&args[3]).expect("create output");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), bink.width(), bink.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&frame.to_rgba8()).unwrap();
}
