//! `naps_pkg_layout.dat`: the table that maps the inner image's logical (uncompressed)
//! offsets to its physical blocks (LibProsperoPkg 1.2.0 `ProsperoNwonlyNapsGenerator`,
//! `ProsperoNapsLayoutBuilder`, `ProsperoNapsLayout`).

use super::inner::{BlockChunk, InnerImage, MetaChunk};

const UBLOCK: i64 = 262_144;

/// One compressed block of the plan (`NapsCblockPlanEntry`).
#[derive(Clone, Copy, Default)]
pub(crate) struct PlanEntry {
    pub start_run: bool,
    pub on_disk_offset: i64,
    pub logical_offset: i64,
    pub even_chunk_compressed_length: i64,
    pub stream_length: i64,
    pub even: u8,
    pub odd: u8,
    pub terminator: bool,
}

/// A file's placement for the plan (`NapsFilePlacement`).
pub(crate) struct Placement<'a> {
    pub on_disk_offset: i64,
    pub logical_offset: i64,
    pub on_disk_size: i64,
    pub uncompressed_size: i64,
    pub store_raw: bool,
    pub blocks: &'a [BlockChunk],
}

fn half_mode(boundary_flags: i32, first_half: bool) -> u8 {
    let (a, b) = if first_half { (2, 1) } else { (32, 16) };
    let mut m = if boundary_flags & a != 0 {
        4
    } else if !first_half {
        1
    } else {
        0
    };
    if boundary_flags & b != 0 {
        m |= 2;
    }
    if first_half {
        m |= 1;
    }
    m as u8
}

/// `DeriveDataRegionBlocks` (no run overrides, no shuffle patterns).
pub(crate) fn data_region_blocks(files: &[Placement]) -> Vec<PlanEntry> {
    let mut list = Vec::new();
    let mut expected: Option<i64> = None;
    let mut starts_run = |off: i64, expected: &Option<i64>| -> bool { !matches!(expected, Some(e) if *e == off) };
    for f in files {
        if f.uncompressed_size == 0 && f.on_disk_size == 0 {
            continue;
        }
        if !f.blocks.is_empty() {
            let mut phys = f.on_disk_offset;
            let mut logical = f.logical_offset;
            for b in f.blocks {
                let off = if b.on_disk_offset >= 0 { b.on_disk_offset } else { phys };
                let even_len = if b.is_stored() {
                    b.compressed_size.min(131_072)
                } else if b.multi_chunk {
                    b.first_chunk_compressed_size
                } else {
                    b.compressed_size
                };
                list.push(PlanEntry {
                    start_run: starts_run(off, &expected),
                    on_disk_offset: off,
                    logical_offset: logical,
                    even_chunk_compressed_length: even_len as i64,
                    stream_length: b.compressed_size as i64,
                    even: if b.is_stored() { 1 } else { half_mode(b.boundary_flags, true) },
                    odd: if b.is_stored() {
                        (b.compressed_size > 131_072) as u8
                    } else if b.multi_chunk {
                        half_mode(b.boundary_flags, false)
                    } else {
                        0
                    },
                    terminator: false,
                });
                expected = Some(off + b.compressed_size as i64);
                phys = off + b.compressed_size as i64;
                logical += b.uncompressed_size as i64;
            }
        } else if f.store_raw {
            let full = f.uncompressed_size / UBLOCK;
            let rest = f.uncompressed_size - full * UBLOCK;
            for k in 0..full {
                let off = f.on_disk_offset + k * UBLOCK;
                list.push(PlanEntry {
                    start_run: starts_run(off, &expected),
                    on_disk_offset: off,
                    logical_offset: f.logical_offset + k * UBLOCK,
                    even_chunk_compressed_length: 131_072,
                    stream_length: UBLOCK,
                    even: 1,
                    odd: 1,
                    terminator: false,
                });
                expected = Some(off + UBLOCK);
            }
            if rest > 0 {
                let off = f.on_disk_offset + full * UBLOCK;
                list.push(PlanEntry {
                    start_run: starts_run(off, &expected),
                    on_disk_offset: off,
                    logical_offset: f.logical_offset + full * UBLOCK,
                    even_chunk_compressed_length: rest.min(131_072),
                    stream_length: rest,
                    even: 1,
                    odd: (rest > 131_072) as u8,
                    terminator: false,
                });
                expected = Some(off + rest);
            }
        } else {
            list.push(PlanEntry {
                start_run: starts_run(f.on_disk_offset, &expected),
                on_disk_offset: f.on_disk_offset,
                logical_offset: f.logical_offset,
                even_chunk_compressed_length: f.on_disk_size,
                stream_length: f.on_disk_size,
                even: 5,
                odd: 0,
                terminator: false,
            });
            expected = Some(f.on_disk_offset + f.on_disk_size);
        }
    }
    let _ = &mut starts_run;
    list
}

/// A CblockInfo record: a run base or a block.
#[derive(Clone, Copy)]
enum Info {
    Run { coffset_end_mod: u32, tweak_idx_start: u32, coffset_start_256k: u32 },
    Block { coffset_start_mod: u32, terminal: bool, uoffset_start: u32, clen_even_minus1: u32, even: u8, odd: u8 },
}

impl Info {
    fn is_run(&self) -> bool {
        matches!(self, Info::Run { .. })
    }

    fn encode(&self) -> [u8; 9] {
        let v: u128 = match *self {
            Info::Run { coffset_end_mod, tweak_idx_start, coffset_start_256k } => {
                (coffset_end_mod as u128 & 0x3FFFF) | 1 << 18 | (tweak_idx_start as u128) << 20 | (coffset_start_256k as u128) << 50
            }
            Info::Block { coffset_start_mod, terminal, uoffset_start, clen_even_minus1, even, odd } => {
                (coffset_start_mod as u128 & 0x3FFFF)
                    | (terminal as u128) << 19
                    | (uoffset_start as u128) << 20
                    | (clen_even_minus1 as u128) << 38
                    | (even as u128) << 55
                    | (odd as u128) << 58
            }
        };
        let mut out = [0u8; 9];
        for (i, b) in out.iter_mut().enumerate() {
            *b = (v >> (8 * i)) as u8;
        }
        out
    }
}

/// Whether a run record must be inserted so that every 16-record window starts with one.
fn needs_cursor_preserving_run(blocks: &[PlanEntry], from: usize, entry_count: usize) -> bool {
    let mut n = entry_count;
    for b in &blocks[from..] {
        if n & 0xF == 0 {
            return false;
        }
        if b.start_run {
            n += 1;
        }
        if n & 0xF == 0 {
            return true;
        }
        n += 1;
    }
    false
}

/// `WalkBlocks`: the records, and (record index, logical offset) of every block.
fn walk_blocks(blocks: &[PlanEntry]) -> (Vec<Info>, Vec<(usize, i64)>) {
    let mut list: Vec<Info> = Vec::with_capacity(blocks.len() * 2);
    let mut std_logical = Vec::with_capacity(blocks.len());
    let mut cursor: i64 = 0;
    for (i, b) in blocks.iter().enumerate() {
        if !b.start_run && needs_cursor_preserving_run(blocks, i, list.len()) {
            list.push(Info::Run {
                coffset_end_mod: (cursor & 0x3FFFF) as u32,
                tweak_idx_start: (cursor >> 16) as u32,
                coffset_start_256k: (cursor / UBLOCK) as u32,
            });
        }
        if b.start_run || list.len().is_multiple_of(16) {
            let end_mod = (cursor & 0x3FFFF) as u32;
            let start = if b.start_run { b.on_disk_offset } else { cursor };
            cursor = start;
            list.push(Info::Run {
                coffset_end_mod: end_mod,
                tweak_idx_start: if b.terminator { 0 } else { (start >> 16) as u32 },
                coffset_start_256k: (start / UBLOCK) as u32,
            });
        }
        let idx = list.len();
        list.push(Info::Block {
            coffset_start_mod: (cursor & 0x3FFFF) as u32,
            terminal: b.terminator,
            uoffset_start: (b.logical_offset & 0x3FFFF) as u32,
            clen_even_minus1: if b.terminator { 0 } else { (b.even_chunk_compressed_length - 1).min(131_071) as u32 },
            even: b.even,
            odd: b.odd,
        });
        std_logical.push((idx, b.logical_offset));
        cursor += b.stream_length;
    }
    (list, std_logical)
}

/// `MeasureU2cCapacity`: the largest record delta of every group of 8 u-blocks.
pub(crate) fn measure_u2c_capacity(blocks: &[PlanEntry], num_ublocks: usize) -> Vec<i32> {
    let (entries, std_logical) = walk_blocks(blocks);
    let terminal = if blocks.last().is_some_and(|b| b.terminator) { entries.len() - 1 } else { entries.len() };
    let mut sorted: Vec<(usize, i64)> = std_logical.into_iter().filter(|t| t.0 != terminal).collect();
    sorted.sort_by_key(|t| t.1);
    let mut first = vec![0usize; num_ublocks];
    let mut k = 0;
    for (u, f) in first.iter_mut().enumerate() {
        let lim = u as i64 * UBLOCK;
        while k < sorted.len() && sorted[k].1 < lim {
            k += 1;
        }
        *f = if k < sorted.len() { sorted[k].0 } else { terminal };
    }
    let groups = num_ublocks.div_ceil(8);
    (0..groups)
        .map(|g| {
            let base = first[g * 8] as i32;
            (1..8)
                .map(|j| {
                    let u = g * 8 + j;
                    if u < num_ublocks {
                        first[u] as i32 - base
                    } else {
                        terminal as i32 - base
                    }
                })
                .fold(0, i32::max)
        })
        .collect()
}

/// `AppendDedupCapacityTail`: the metadata stand-in the capacity check appends; returns the
/// u-block count.
pub(crate) fn append_capacity_tail(blocks: &mut Vec<PlanEntry>, logical_end: i64, physical_end: i64) -> usize {
    for i in 0..16 {
        blocks.push(PlanEntry {
            start_run: i == 0,
            on_disk_offset: physical_end,
            logical_offset: logical_end + i * UBLOCK,
            even_chunk_compressed_length: 8,
            stream_length: 16,
            even: 1,
            odd: 1,
            terminator: false,
        });
    }
    let end = logical_end + 4_194_304;
    blocks.push(PlanEntry {
        start_run: true,
        on_disk_offset: physical_end,
        logical_offset: end,
        even_chunk_compressed_length: 1,
        stream_length: 0,
        terminator: true,
        ..Default::default()
    });
    ((end + UBLOCK - 1) / UBLOCK + 1).max(1) as usize
}

fn half_from_meta(m: &MetaChunk) -> (i64, u8, u8) {
    let stored = m.compressed_size == m.uncompressed_size;
    let even_len = if stored {
        m.compressed_size.min(131_072)
    } else if m.multi_chunk {
        m.first_chunk_compressed_size
    } else {
        m.compressed_size
    };
    let even = if stored { 1 } else { half_mode(m.boundary_flags, true) };
    let odd = if stored {
        (m.compressed_size > 131_072) as u8
    } else if m.multi_chunk {
        half_mode(m.boundary_flags, false)
    } else {
        0
    };
    (even_len as i64, even, odd)
}

/// `ProsperoNwonlyNapsGenerator.Generate` for an inner image without shuffle patterns or
/// CMAC tags.
pub(crate) fn generate(inner: &InnerImage) -> Result<Vec<u8>, String> {
    let files: Vec<Placement> = inner
        .placements
        .iter()
        .map(|p| Placement {
            on_disk_offset: p.on_disk_offset,
            logical_offset: p.logical_offset,
            on_disk_size: p.on_disk_size,
            uncompressed_size: p.uncompressed_size,
            store_raw: p.store_raw,
            blocks: &p.blocks,
        })
        .collect();
    let mount_end = inner.ndblock * 65536;
    let meta_base = inner.meta_base_logical;
    let data_end = inner.data_end_logical;
    let mut tail = Vec::new();
    // The block-info table stands in for the gap between the data and the metadata.
    let gap = meta_base - data_end;
    let gap_blocks = if gap > 0 { (gap + UBLOCK - 1) / UBLOCK } else { 0 };
    for i in 0..gap_blocks {
        let large = (UBLOCK).min(gap - i * UBLOCK) > 131_072;
        tail.push(PlanEntry {
            start_run: i == 0,
            on_disk_offset: inner.block_info_on_disk,
            logical_offset: data_end + i * UBLOCK,
            even_chunk_compressed_length: 8,
            stream_length: if large { 16 } else { 8 },
            even: 1,
            odd: large as u8,
            terminator: false,
        });
    }
    let mut phys = inner.metadata_on_disk;
    let mut logical = meta_base;
    for (i, m) in inner.metadata_blocks.iter().enumerate() {
        let (even_len, even, odd) = half_from_meta(m);
        tail.push(PlanEntry {
            start_run: i == 0,
            on_disk_offset: phys,
            logical_offset: logical,
            even_chunk_compressed_length: even_len,
            stream_length: m.compressed_size as i64,
            even,
            odd,
            terminator: false,
        });
        phys += m.compressed_size as i64;
        logical += m.uncompressed_size as i64;
    }
    tail.push(PlanEntry { start_run: true, on_disk_offset: phys, logical_offset: mount_end, terminator: true, ..Default::default() });

    let mut offsets: Vec<i64> = inner.afid_logical_offsets.clone();
    if offsets.last() != Some(&data_end) {
        offsets.push(data_end);
    }
    offsets.push(meta_base);
    offsets.push(mount_end);
    let mut types = vec![0u8; offsets.len()];
    for h in &inner.sparse_afid_holes {
        types[*h] = 64;
    }
    *types.last_mut().unwrap() = 64;
    let num_ublocks = ((mount_end + UBLOCK - 1) / UBLOCK + 1) as usize;
    let num_outer = ((inner.image_length + 65535) / 65536) as usize;

    let mut blocks = data_region_blocks(&files);
    blocks.sort_by_key(|b| b.logical_offset);
    blocks.extend(tail);
    build_layout(&blocks, &offsets, &types, num_ublocks, num_outer)
}

/// `BuildDocument` + `BuildLayout` (compression type 2, one key, no shuffle patterns, zero
/// outer-block digests, an 8-byte zero footer).
fn build_layout(blocks: &[PlanEntry], file_offsets: &[i64], types: &[u8], num_ublocks: usize, num_outer: usize) -> Result<Vec<u8>, String> {
    let (infos, std_logical) = walk_blocks(blocks);
    let num_files = file_offsets.len();
    let ublock_count = num_ublocks; // counts.UBlockCount = (NumUBlocks - 1) + 1
    let num_u2c = (num_ublocks - 1 + 8) >> 3;
    let num_cblock = infos.len();
    // u2c: the first record of every u-block.
    let mut sorted = std_logical.clone();
    sorted.sort_by_key(|t| t.1);
    let terminal = num_cblock - 1;
    let mut first = vec![0usize; ublock_count];
    let mut k = 0;
    for (u, f) in first.iter_mut().enumerate() {
        let lim = u as i64 * UBLOCK;
        while k < sorted.len() && sorted[k].0 != terminal && sorted[k].1 < lim {
            k += 1;
        }
        *f = if k < sorted.len() && sorted[k].0 != terminal && sorted[k].1 >= lim { sorted[k].0 } else { terminal };
    }
    let mut u2c: Vec<[u8; 10]> = Vec::with_capacity(num_u2c);
    for g in 0..num_u2c {
        let base = first[8 * g];
        let mut e = [0u8; 10];
        e[0] = base as u8;
        e[1] = (base >> 8) as u8;
        e[2] = (base >> 16) as u8;
        for j in 0..7 {
            let u = 8 * g + 1 + j;
            let d = if u < ublock_count { first[u] as i64 - base as i64 } else { terminal as i64 - base as i64 };
            if !(0..=255).contains(&d) {
                return Err(format!("NAPS u2c delta {d} exceeds one byte: this layout size is not supported"));
            }
            e[3 + j] = d as u8;
        }
        u2c.push(e);
    }
    // Validation as the original (every window starts with a run base, one terminal).
    for w in (0..infos.len()).step_by(16) {
        if !infos[w].is_run() {
            return Err(format!("NAPS CblockInfo window 0x{w:X} does not begin with a RUN base"));
        }
    }
    // Header and sections.
    let mut out = Vec::new();
    let h0: u64 = ((num_files as u64 - 1) & 0xFFFFFF) | (2u64 << 24) | (((num_ublocks as u64 - 1) & 0x7FFFFF) << 32);
    let h1: u64 = (num_outer as u64 & 0xFFFFFF) | (((num_cblock as u64 - 2) & 0xFFFFFF) << 24);
    out.extend_from_slice(&h0.to_le_bytes());
    out.extend_from_slice(&h1.to_le_bytes());
    out.resize(out.len() + num_outer * 8, 0);
    for (i, &o) in file_offsets.iter().enumerate() {
        let v = o as u64;
        if v > 0xFF_FFFF_FFFF {
            return Err("NAPS file offset exceeds 40 bits".into());
        }
        out.extend_from_slice(&v.to_le_bytes()[..5]);
        out.push(types[i]);
    }
    for e in &u2c {
        out.extend_from_slice(e);
    }
    let aligned = out.len().div_ceil(8) * 8;
    out.resize(aligned, 0);
    for info in &infos {
        out.extend_from_slice(&info.encode());
    }
    let total = out.len().div_ceil(8) * 8;
    out.resize(total + 8, 0);
    Ok(out)
}
