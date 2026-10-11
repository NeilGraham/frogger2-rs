//! Diagnostics: per-stream sizes of the packed model banks (all banks concatenated per
//! stream), compressed with F2Z (plain / planes / delta) and xz, to see where the bytes go.
//! `cargo run --release -p f2-n64pack --example model_streams`

use std::io::Write;
use std::process::{Command, Stdio};

use f2_formats::objbank::ObjectBank;
use f2_n64pack::models::{encode, NSTREAMS, STREAM_WIDTH};
use f2_n64z::enc::{compress, Options};
use f2_n64z::Segment;

fn xz(d: &[u8]) -> usize {
    let mut c = Command::new("xz").args(["--format=lzma", "-9e", "-c"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut i = c.stdin.take().unwrap();
    let d2 = d.to_vec();
    let t = std::thread::spawn(move || i.write_all(&d2).unwrap());
    let o = c.wait_with_output().unwrap();
    t.join().unwrap();
    o.stdout.len()
}

fn main() {
    let root = f2_n64pack::ctx::find_rendition(&std::env::current_dir().unwrap()).expect("no game data");
    let mut streams: Vec<Vec<u8>> = vec![Vec::new(); NSTREAMS];
    let mut names: Vec<_> = std::fs::read_dir(root.join("game/objects")).unwrap().map(|e| e.unwrap().path()).collect();
    names.sort();
    for p in names {
        let bank = ObjectBank::parse(&std::fs::read(&p).unwrap()).unwrap();
        let (data, segs) = encode(&bank);
        let mut off = segs[0].len as usize;
        for (i, s) in segs[1..].iter().enumerate() {
            streams[i].extend_from_slice(&data[off..off + s.len as usize]);
            off += s.len as usize;
        }
    }
    let names =
        ["META", "NAMES", "U32", "VERT", "FACE", "UV", "NORMAL", "COLOR", "FFLAG", "TEXIDX", "DECV", "DECQ", "DECM", "RAW", "TIME", "IDX32", "SPRITE", "ANIM"];
    let opt = Options::level(9);
    println!("| stream | raw | f2z | f2z planes | f2z delta | xz |");
    for (i, s) in streams.iter().enumerate() {
        if s.is_empty() {
            continue;
        }
        let w = STREAM_WIDTH[i];
        let n = s.len() as u32;
        let plain = compress(s, &[], None, &opt).len();
        let planes = if w > 1 { compress(s, &[Segment::planes(u16::from(w), n)], None, &opt).len() } else { plain };
        let delta = if w > 1 { compress(s, &[Segment::delta(u16::from(w), w, true, n)], None, &opt).len() } else { plain };
        println!("| {} | {} | {plain} | {planes} | {delta} | {} |", names[i], s.len(), xz(s));
    }
}
