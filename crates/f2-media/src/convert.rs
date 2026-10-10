//! Re-encodes a disc image into the other supported container formats (developer and
//! test tool: `f2-import --convert`). Every output lays the tracks out back to back
//! from LBA 0 (pregaps included, like the pressed disc), so all formats describe the same
//! sectors and import to identical files.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::ccd::{write_ccd, CcdOut};
use crate::disc::{bad, Disc, TrackKind, RAW};
use crate::mds::{write_mds, MdsOut};
use crate::nrg::{write_nrg_tail, NrgOut};
use crate::toc::{write_toc, TocOut};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutFormat {
    /// Single `.bin` + `.cue` (INDEX 00/01 per track).
    Cue,
    /// CloneCD `.ccd` + `.img` + `.sub` (zeroed subchannel).
    Ccd,
    /// Alcohol `.mds` + `.mdf`, 2352-byte sectors.
    Mds,
    /// Alcohol `.mds` + `.mdf`, 2448-byte RAW+SUB sectors.
    MdsSub,
    /// Nero v2 (`NER5`, DAOX).
    Nrg,
    /// Nero v1 (`NERO`, DAOI).
    Nrg1,
    /// cdrdao `.toc` + `.bin` (big-endian audio).
    Toc,
    /// Data track only, cooked 2048-byte `.iso`.
    Iso,
}

impl OutFormat {
    pub const ALL: &'static [(&'static str, OutFormat)] = &[
        ("cue", OutFormat::Cue),
        ("ccd", OutFormat::Ccd),
        ("mds", OutFormat::Mds),
        ("mds-sub", OutFormat::MdsSub),
        ("nrg", OutFormat::Nrg),
        ("nrg1", OutFormat::Nrg1),
        ("toc", OutFormat::Toc),
        ("iso", OutFormat::Iso),
    ];

    pub fn parse(s: &str) -> Option<OutFormat> {
        Self::ALL.iter().find(|f| f.0 == s).map(|f| f.1)
    }
}

/// Writes `disc` as `<dir>/<stem>.<ext>` (+ data files); returns the path to open.
pub fn write_image(disc: &Disc, fmt: OutFormat, dir: &Path, stem: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    if disc.tracks.iter().any(|t| !t.is_raw()) && fmt != OutFormat::Iso {
        return Err(bad("conversion needs raw 2352-byte tracks"));
    }
    if fmt == OutFormat::Iso {
        let t = disc.first_data_track().ok_or_else(|| bad("no data track"))?;
        let mut src = disc.data_source()?;
        let p = dir.join(format!("{stem}.iso"));
        let mut w = BufWriter::new(File::create(&p)?);
        let mut buf = Vec::new();
        let mut lba = 0;
        while lba < t.sectors {
            let n = 256.min(t.sectors - lba);
            buf.clear();
            src.read_sectors(lba, n, &mut buf)?;
            w.write_all(&buf)?;
            lba += n;
        }
        w.flush()?;
        return Ok(p);
    }
    let sub = fmt == OutFormat::MdsSub;
    let stride = RAW + if sub { 96 } else { 0 };
    let data_ext = match fmt {
        OutFormat::Ccd => "img",
        OutFormat::Mds | OutFormat::MdsSub => "mdf",
        OutFormat::Nrg | OutFormat::Nrg1 => "nrg",
        _ => "bin",
    };
    let data_path = dir.join(format!("{stem}.{data_ext}"));
    let mut w = BufWriter::with_capacity(1 << 20, File::create(&data_path)?);
    // (number, audio, pregap-start LBA, pregap, sectors)
    let mut layout = Vec::new();
    let mut lba = 0u32;
    for t in &disc.tracks {
        let audio = t.kind == TrackKind::Audio;
        layout.push((t.number, audio, lba, t.pregap, t.sectors));
        let big_endian = fmt == OutFormat::Toc && audio;
        disc.stream_redump(t, &mut |b| {
            if !sub && !big_endian {
                return w.write_all(b);
            }
            for s in b.chunks(RAW as usize) {
                if big_endian {
                    let mut v = s.to_vec();
                    for p in v.as_chunks_mut::<2>().0 {
                        p.swap(0, 1);
                    }
                    w.write_all(&v)?;
                } else {
                    w.write_all(s)?;
                }
                if sub {
                    w.write_all(&[0u8; 96])?;
                }
            }
            Ok(())
        })?;
        lba += t.pregap + t.sectors;
    }
    let leadout = lba;
    let data_len = u64::from(leadout) * stride;
    let msf = |n: u32| format!("{:02}:{:02}:{:02}", n / 4500, (n / 75) % 60, n % 75);
    let data_name = data_path.file_name().unwrap().to_string_lossy().into_owned();
    let desc = match fmt {
        OutFormat::Cue => {
            let mut s = format!("FILE \"{data_name}\" BINARY\r\n");
            for &(n, audio, at, pregap, _) in &layout {
                s += &format!("  TRACK {n:02} {}\r\n", if audio { "AUDIO" } else { "MODE1/2352" });
                if pregap > 0 {
                    s += &format!("    INDEX 00 {}\r\n", msf(at));
                }
                s += &format!("    INDEX 01 {}\r\n", msf(at + pregap));
            }
            Some(("cue", s.into_bytes()))
        }
        OutFormat::Ccd => {
            std::fs::write(dir.join(format!("{stem}.sub")), vec![0u8; leadout as usize * 96])?;
            let t: Vec<CcdOut> = layout.iter().map(|&(n, audio, at, pregap, _)| CcdOut { number: n, audio, index0: (pregap > 0).then_some(at), index1: at + pregap }).collect();
            Some(("ccd", write_ccd(&t, leadout).into_bytes()))
        }
        OutFormat::Mds | OutFormat::MdsSub => {
            let t: Vec<MdsOut> = layout
                .iter()
                .map(|&(n, audio, at, pregap, sectors)| MdsOut { number: n as u8, audio, lba: at + pregap, pregap, length: sectors, offset: u64::from(at) * stride })
                .collect();
            Some(("mds", write_mds(&t, stride as u16, leadout)))
        }
        OutFormat::Nrg | OutFormat::Nrg1 => {
            let t: Vec<NrgOut> = layout
                .iter()
                .map(|&(_, audio, at, pregap, sectors)| NrgOut {
                    mode: if audio { 0x07 } else { 0x05 },
                    sector_size: 2352,
                    index0: u64::from(at) * RAW,
                    index1: u64::from(at + pregap) * RAW,
                    end: u64::from(at + pregap + sectors) * RAW,
                })
                .collect();
            w.write_all(&write_nrg_tail(&t, data_len, fmt == OutFormat::Nrg))?;
            None
        }
        OutFormat::Toc => {
            let t: Vec<TocOut> = layout.iter().map(|&(_, audio, at, pregap, sectors)| TocOut { audio, offset: u64::from(at) * RAW, pregap, sectors: pregap + sectors }).collect();
            Some(("toc", write_toc(&data_name, &t).into_bytes()))
        }
        OutFormat::Iso => unreachable!(),
    };
    w.flush()?;
    drop(w);
    Ok(match desc {
        Some((ext, bytes)) => {
            let p = dir.join(format!("{stem}.{ext}"));
            std::fs::write(&p, bytes)?;
            p
        }
        None => data_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every output format re-opens to the same per-track Redump hashes and audio.
    #[test]
    fn all_formats_round_trip() {
        let dir = std::env::temp_dir().join(format!("f2conv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 4 data sectors (+ a second data track of 3), then two audio tracks with pregaps.
        let mut bin = Vec::new();
        for k in 0..7u8 {
            let mut s = vec![0u8; 2352];
            s[..12].copy_from_slice(&crate::sector::SYNC);
            s[15] = 1;
            s[16..2064].fill(k);
            bin.extend_from_slice(&s);
        }
        for i in 0..(20 * 2352 / 2) {
            bin.extend_from_slice(&((i as u16).wrapping_mul(40503)).to_le_bytes());
        }
        std::fs::write(dir.join("src.bin"), &bin).unwrap();
        std::fs::write(
            dir.join("src.cue"),
            "FILE \"src.bin\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\n TRACK 02 MODE1/2352\n  INDEX 01 00:00:04\n TRACK 03 AUDIO\n  INDEX 00 00:00:07\n  INDEX 01 00:00:09\n TRACK 04 AUDIO\n  INDEX 00 00:00:17\n  INDEX 01 00:00:20\n",
        )
        .unwrap();
        let src = Disc::open(&dir.join("src.cue")).unwrap();
        assert_eq!(src.tracks.len(), 4);
        let hashes = |d: &Disc| d.tracks.iter().map(|t| d.redump_sha1(t).unwrap()).collect::<Vec<_>>();
        let want = hashes(&src);
        for &(name, f) in OutFormat::ALL {
            if f == OutFormat::Iso {
                continue;
            }
            let out = dir.join(name);
            let p = write_image(&src, f, &out, "game").unwrap();
            let d = Disc::open(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(hashes(&d), want, "{name} ({})", d.container);
            let a: Vec<_> = d.tracks.iter().map(|t| (t.number, t.pregap, t.sectors)).collect();
            assert_eq!(a, vec![(1, 0, 4), (2, 0, 3), (3, 2, 8), (4, 3, 7)], "{name}");
            std::fs::remove_dir_all(&out).ok();
        }
        let iso = write_image(&src, OutFormat::Iso, &dir, "data").unwrap();
        assert_eq!(std::fs::metadata(&iso).unwrap().len(), 4 * 2048);
        std::fs::remove_dir_all(&dir).ok();
    }
}
