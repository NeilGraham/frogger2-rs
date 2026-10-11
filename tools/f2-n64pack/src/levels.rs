//! Levels, scripts, collision, scenics, demos, text: the **retail bytes unchanged** (the
//! game's own parsers read them), stored under their retail relative paths
//! (`maps/entity-1-0.dat`, `collision/garden1.col`, …). Only `maps/language.fla` is stored
//! FLA2-decompressed (the game's `fileLoad` accepts either form) so the pack codec sees the
//! plain text.
//!
//! Each file gets F2Z structure transforms describing its record arrays (collision tiles,
//! camera planes, scenic records, demo key records, sfx-animation tables); the encoder
//! keeps whichever of {transformed, plain} is smaller per file.

use std::path::Path;

use f2_n64z::enc::{compress, Options};
use f2_n64z::Segment;

use crate::{Class, Ctx, RawAsset, Result};

/// Directories (relative to `game/`) whose files are level data.
pub const DIRS: [&str; 4] = ["maps", "collision", "scenics", "demos"];

fn le32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// `collision/*.col`: header, GAMETILE[68 B], then optional camera planes (20 B) and boxes (8 B).
fn col_segments(d: &[u8], delta_w: u8) -> Option<Vec<Segment>> {
    let tiles = le32(d, 0)? as usize;
    let babies = le32(d, 20)? as usize;
    let h = 24 + 4 * babies;
    let te = h.checked_add(tiles.checked_mul(68)?)?;
    if te > d.len() {
        return None;
    }
    let tile_seg = |len: usize| if delta_w == 0 { Segment::planes(68, len as u32) } else { Segment::delta(68, delta_w, false, len as u32) };
    let mut segs = vec![Segment::raw(h as u32), tile_seg(te - h)];
    let mut o = te;
    if let Some(np) = le32(d, o) {
        let pe = o + 4 + 20 * np as usize;
        if pe <= d.len() {
            segs.push(Segment::raw(4));
            segs.push(Segment::planes(20, (pe - o - 4) as u32));
            o = pe;
            if let Some(nb) = le32(d, o) {
                let be = o + 4 + 8 * nb as usize;
                if be <= d.len() {
                    segs.push(Segment::raw(4));
                    segs.push(Segment::delta(8, 4, false, (be - o - 4) as u32));
                    o = be;
                }
            }
        }
    }
    segs.push(Segment::raw((d.len() - o) as u32));
    Some(segs)
}

/// Candidate transform sets for a level file (by extension).
pub fn candidates(name: &str, d: &[u8]) -> Vec<Vec<Segment>> {
    let n = d.len() as u32;
    let ext = name.rsplit('.').next().unwrap_or("");
    let rec = |h: u32, stride: u16, w: u8| -> Vec<Segment> {
        if n < h {
            return vec![Segment::raw(n)];
        }
        let body = (n - h) / u32::from(stride) * u32::from(stride);
        vec![Segment::raw(h), Segment::delta(stride, w, false, body), Segment::raw(n - h - body)]
    };
    match ext {
        "col" => [0u8, 2, 4].iter().filter_map(|&w| col_segments(d, w)).collect(),
        "sce" if n >= 4 => vec![rec(4, 44, 2), rec(4, 44, 4), vec![Segment::raw(4), Segment::planes(44, (n - 4) / 44 * 44), Segment::raw((n - 4) % 44)]],
        "key" => vec![rec(0, 12, 4)],
        "sam" => vec![vec![Segment::planes(4, n / 4 * 4), Segment::raw(n % 4)]],
        _ => Vec::new(),
    }
}

/// Picks the best candidate (by a quick level-4 compression) for a file.
fn best_segments(name: &str, d: &[u8]) -> Vec<Segment> {
    let opt = Options::level(4);
    let mut best = (compress(d, &[], None, &opt).len(), Vec::new());
    for c in candidates(name, d) {
        let z = compress(d, &c, None, &opt).len();
        if z < best.0 {
            best = (z, c);
        }
    }
    best.1
}

pub fn convert(ctx: &Ctx) -> Result<(Vec<RawAsset>, Vec<String>)> {
    let mut files = Vec::new();
    for dir in DIRS {
        let p = ctx.game().join(dir);
        let mut v: Vec<_> =
            std::fs::read_dir(&p).map_err(|e| format!("{}: {e}", p.display()))?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect();
        v.sort();
        files.extend(v.into_iter().map(|p| (dir, p)));
    }
    let out = crate::tools::par_map(&files, ctx.jobs, |(dir, p): &(&str, std::path::PathBuf)| convert_one(dir, p));
    let out: Vec<RawAsset> = out.into_iter().collect::<Result<_>>()?;
    let src: u64 = out.iter().map(|a| a.source_bytes).sum();
    let raw: u64 = out.iter().map(|a| a.data.len() as u64).sum();
    Ok((out, vec![format!("levels: {} files, {src} B on disk ({raw} B after FLA2 decompression of the text)", files.len())]))
}

fn convert_one(dir: &str, p: &Path) -> Result<RawAsset> {
    let d = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
    let mut notes = Vec::new();
    let data = if f2_formats::fla::is_fla(&d) {
        notes.push("FLA2-decompressed".to_string());
        f2_formats::fla::decompress(&d).map_err(|e| format!("{}: {e}", p.display()))?
    } else {
        d.clone()
    };
    let segments = best_segments(&name, &data);
    Ok(RawAsset { path: format!("{dir}/{name}"), class: Class::Levels, source_bytes: d.len() as u64, data, notes, segments })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn col_segments_cover_file() {
        let mut d = Vec::new();
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&[0; 16]);
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&5u32.to_le_bytes());
        d.extend(std::iter::repeat_n(7u8, 136));
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&[1; 20]);
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&[2; 8]);
        d.extend_from_slice(&[9, 9]);
        for c in candidates("x.col", &d) {
            assert_eq!(c.iter().map(|s| s.len as usize).sum::<usize>(), d.len());
            let back = f2_n64z::transform::forward(&c, &d);
            let mut o = vec![0; d.len()];
            f2_n64z::transform::inverse(&c, &back, &mut o).unwrap();
            assert_eq!(o, d);
        }
        // truncated header → no candidates, never a panic
        assert!(candidates("x.col", &d[..10]).is_empty());
    }

    #[test]
    fn real_level_files_segment_and_round_trip() {
        let Some(root) = crate::ctx::find_rendition(Path::new(env!("CARGO_MANIFEST_DIR"))) else {
            eprintln!("skipped: no game data");
            return;
        };
        for dir in DIRS {
            for e in std::fs::read_dir(root.join("game").join(dir)).unwrap() {
                let p = e.unwrap().path();
                let d = std::fs::read(&p).unwrap();
                let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
                for c in candidates(&name, &d) {
                    assert_eq!(c.iter().map(|s| s.len as usize).sum::<usize>(), d.len(), "{}", p.display());
                    let t = f2_n64z::transform::forward(&c, &d);
                    let mut o = vec![0; d.len()];
                    f2_n64z::transform::inverse(&c, &t, &mut o).unwrap();
                    assert_eq!(o, d, "{}", p.display());
                }
            }
        }
    }
}
