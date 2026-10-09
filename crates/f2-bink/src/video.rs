//! Bink 1 video decoder (revisions 'c'..'k'; the old 'b' revision is not supported).
//!
//! Close translation of FFmpeg `libavcodec/bink.c` and `binkdsp.c` (LGPL-2.1-or-later,
//! Copyright (C) 2009 Konstantin Shishkov, Copyright (C) 2011 Peter Ross).

use crate::bits::BitReader;
use crate::tables::*;
use crate::Error;

const BINK_FLAG_ALPHA: u32 = 0x0010_0000;

// Bundle sources.
const SRC_BLOCK_TYPES: usize = 0;
const SRC_SUB_BLOCK_TYPES: usize = 1;
const SRC_COLORS: usize = 2;
const SRC_PATTERN: usize = 3;
const SRC_X_OFF: usize = 4;
const SRC_Y_OFF: usize = 5;
const SRC_INTRA_DC: usize = 6;
const SRC_INTER_DC: usize = 7;
const SRC_RUN: usize = 8;
const NB_SRC: usize = 9;

// Block types.
const SKIP_BLOCK: i32 = 0;
const SCALED_BLOCK: i32 = 1;
const MOTION_BLOCK: i32 = 2;
const RUN_BLOCK: i32 = 3;
const RESIDUE_BLOCK: i32 = 4;
const INTRA_BLOCK: i32 = 5;
const FILL_BLOCK: i32 = 6;
const INTER_BLOCK: i32 = 7;
const PATTERN_BLOCK: i32 = 8;
const RAW_BLOCK: i32 = 9;

const DC_START_BITS: u32 = 11;
const BINK_RLELENS: [usize; 4] = [4, 8, 12, 32];

fn bad(msg: &str) -> Error {
    Error::InvalidData(msg.to_string())
}

/// Huffman lookup table for one of the 16 Bink trees: `(leaf index, code length)` indexed by
/// the next `max_bits` bits of the stream.
struct Vlc {
    max_bits: u32,
    table: Vec<(u8, u8)>,
}

fn build_vlcs() -> Vec<Vlc> {
    (0..16)
        .map(|t| {
            let max_bits = BINK_TREE_LENS[t][15] as u32;
            let mut table = vec![(0u8, 0u8); 1 << max_bits];
            for s in 0..16 {
                let len = BINK_TREE_LENS[t][s] as u32;
                let code = BINK_TREE_BITS[t][s] as usize;
                for e in 0..(1usize << (max_bits - len)) {
                    table[code | (e << len)] = (s as u8, len as u8);
                }
            }
            Vlc { max_bits, table }
        })
        .collect()
}

#[derive(Clone, Copy, Default)]
struct Tree {
    vlc_num: usize,
    syms: [u8; 16],
}

struct Bundle {
    len: u32,
    tree: Tree,
    data: Vec<i32>,
    /// Position of the not yet decoded part (`None` = bundle exhausted for this plane).
    cur_dec: Option<usize>,
    /// Position of the next value to hand out.
    cur_ptr: usize,
}

/// Plane buffers for one frame (Y, U, V, A).
#[derive(Clone)]
pub(crate) struct Planes {
    pub(crate) data: [Vec<u8>; 4],
    pub(crate) stride: [usize; 4],
}

pub(crate) struct VideoDecoder {
    width: usize,
    height: usize,
    version: u8,
    has_alpha: bool,
    swap_planes: bool,
    vlcs: Vec<Vlc>,
    bundles: Vec<Bundle>,
    col_high: [Tree; 16],
    col_lastval: usize,
    pub(crate) cur: Planes,
    last: Option<Planes>,
}

impl VideoDecoder {
    pub(crate) fn new(width: u32, height: u32, codec_tag: u32, flags: u32) -> Result<Self, Error> {
        let version = (codec_tag >> 24) as u8;
        if version <= b'b' {
            return Err(Error::Unsupported("Bink revision 'b' video".into()));
        }
        let (width, height) = (width as usize, height as usize);
        let bw = (width + 7) >> 3;
        let bh = (height + 7) >> 3;
        let blocks = bw * bh;
        let bundles = (0..NB_SRC)
            .map(|i| Bundle {
                len: 0,
                tree: Tree::default(),
                // DC bundles hold 16-bit values in FFmpeg's byte buffer: half as many entries.
                data: vec![0; if i == SRC_INTRA_DC || i == SRC_INTER_DC { blocks * 32 } else { blocks * 64 }],
                cur_dec: None,
                cur_ptr: 0,
            })
            .collect();
        let cur = Self::alloc_planes(width, height);
        Ok(VideoDecoder {
            width,
            height,
            version,
            has_alpha: flags & BINK_FLAG_ALPHA != 0,
            swap_planes: version >= b'h',
            vlcs: build_vlcs(),
            bundles,
            col_high: [Tree::default(); 16],
            col_lastval: 0,
            cur,
            last: None,
        })
    }

    fn alloc_planes(width: usize, height: usize) -> Planes {
        let mut data: [Vec<u8>; 4] = Default::default();
        let mut stride = [0usize; 4];
        for p in 0..4 {
            let chroma = p == 1 || p == 2;
            let (bw, bh) = if chroma { ((width + 15) >> 4, (height + 15) >> 4) } else { ((width + 7) >> 3, (height + 7) >> 3) };
            let s = (bw * 8 + 15) & !15;
            stride[p] = s;
            // Extra 8 rows so a 16x16 block in the last block row stays in bounds.
            data[p] = vec![0u8; s * (bh * 8 + 8)];
        }
        Planes { data, stride }
    }

    pub(crate) fn reset(&mut self) {
        self.last = None;
    }

    #[inline]
    fn get_huff(&self, gb: &mut BitReader, tree: &Tree) -> i32 {
        let vlc = &self.vlcs[tree.vlc_num];
        let (sym, len) = vlc.table[gb.peek(vlc.max_bits) as usize];
        gb.skip(len as usize);
        tree.syms[sym as usize] as i32
    }

    fn init_lengths(&mut self, width: usize, bw: usize) {
        let width = (width + 7) & !7;
        let log2 = |v: usize| (usize::BITS - 1 - v.leading_zeros()) + 1;
        self.bundles[SRC_BLOCK_TYPES].len = log2((width >> 3) + 511);
        self.bundles[SRC_SUB_BLOCK_TYPES].len = log2((width >> 4) + 511);
        self.bundles[SRC_COLORS].len = log2(bw * 64 + 511);
        let l = log2((width >> 3) + 511);
        self.bundles[SRC_INTRA_DC].len = l;
        self.bundles[SRC_INTER_DC].len = l;
        self.bundles[SRC_X_OFF].len = l;
        self.bundles[SRC_Y_OFF].len = l;
        self.bundles[SRC_PATTERN].len = log2((bw << 3) + 511);
        self.bundles[SRC_RUN].len = log2(bw * 48 + 511);
    }

    fn read_bundle(&mut self, gb: &mut BitReader, n: usize) -> Result<(), Error> {
        if n == SRC_COLORS {
            for i in 0..16 {
                self.col_high[i] = read_tree(gb)?;
            }
            self.col_lastval = 0;
        }
        if n != SRC_INTRA_DC && n != SRC_INTER_DC {
            self.bundles[n].tree = read_tree(gb)?;
        }
        self.bundles[n].cur_dec = Some(0);
        self.bundles[n].cur_ptr = 0;
        Ok(())
    }

    /// `CHECK_READ_VAL`: returns the number of entries to decode, or `None` to skip.
    fn check_read_val(&mut self, gb: &mut BitReader, n: usize) -> Option<(usize, usize)> {
        let b = &mut self.bundles[n];
        let cur_dec = b.cur_dec?;
        if cur_dec > b.cur_ptr {
            return None;
        }
        let t = gb.get(b.len) as usize;
        if t == 0 {
            b.cur_dec = None;
            return None;
        }
        Some((cur_dec, t))
    }

    fn read_runs(&mut self, gb: &mut BitReader) -> Result<(), Error> {
        let n = SRC_RUN;
        let Some((mut cur, t)) = self.check_read_val(gb, n) else { return Ok(()) };
        let end = cur + t;
        if end > self.bundles[n].data.len() {
            return Err(bad("run value went out of bounds"));
        }
        if gb.left() < 1 {
            return Err(bad("truncated runs"));
        }
        if gb.bit() {
            let v = gb.get(4) as i32;
            self.bundles[n].data[cur..end].fill(v);
            cur = end;
        } else {
            let tree = self.bundles[n].tree;
            while cur < end {
                let v = self.get_huff(gb, &tree);
                self.bundles[n].data[cur] = v;
                cur += 1;
            }
        }
        self.bundles[n].cur_dec = Some(cur);
        Ok(())
    }

    fn read_motion_values(&mut self, gb: &mut BitReader, n: usize) -> Result<(), Error> {
        let Some((mut cur, t)) = self.check_read_val(gb, n) else { return Ok(()) };
        let end = cur + t;
        if end > self.bundles[n].data.len() {
            return Err(bad("too many motion values"));
        }
        if gb.left() < 1 {
            return Err(bad("truncated motion values"));
        }
        if gb.bit() {
            let mut v = gb.get(4) as i32;
            if v != 0 && gb.bit() {
                v = -v;
            }
            self.bundles[n].data[cur..end].fill(v);
            cur = end;
        } else {
            let tree = self.bundles[n].tree;
            while cur < end {
                let mut v = self.get_huff(gb, &tree);
                if v != 0 && gb.bit() {
                    v = -v;
                }
                self.bundles[n].data[cur] = v;
                cur += 1;
            }
        }
        self.bundles[n].cur_dec = Some(cur);
        Ok(())
    }

    fn read_block_types(&mut self, gb: &mut BitReader, n: usize) -> Result<(), Error> {
        let Some((mut cur, mut t)) = self.check_read_val(gb, n) else { return Ok(()) };
        if self.version == b'k' {
            t ^= 0xBB;
            if t == 0 {
                self.bundles[n].cur_dec = None;
                return Ok(());
            }
        }
        let end = cur + t;
        if end > self.bundles[n].data.len() {
            return Err(bad("too many block type values"));
        }
        if gb.left() < 1 {
            return Err(bad("truncated block types"));
        }
        if gb.bit() {
            let v = gb.get(4) as i32;
            self.bundles[n].data[cur..end].fill(v);
            cur = end;
        } else {
            let tree = self.bundles[n].tree;
            let mut last = 0;
            while cur < end {
                let v = self.get_huff(gb, &tree);
                if v < 12 {
                    last = v;
                    self.bundles[n].data[cur] = v;
                    cur += 1;
                } else {
                    let run = BINK_RLELENS[(v - 12) as usize];
                    if end - cur < run {
                        return Err(bad("block type run out of bounds"));
                    }
                    self.bundles[n].data[cur..cur + run].fill(last);
                    cur += run;
                }
            }
        }
        self.bundles[n].cur_dec = Some(cur);
        Ok(())
    }

    fn read_patterns(&mut self, gb: &mut BitReader) -> Result<(), Error> {
        let n = SRC_PATTERN;
        let Some((mut cur, t)) = self.check_read_val(gb, n) else { return Ok(()) };
        let end = cur + t;
        if end > self.bundles[n].data.len() {
            return Err(bad("too many pattern values"));
        }
        let tree = self.bundles[n].tree;
        while cur < end {
            if gb.left() < 2 {
                return Err(bad("truncated patterns"));
            }
            let mut v = self.get_huff(gb, &tree);
            v |= self.get_huff(gb, &tree) << 4;
            self.bundles[n].data[cur] = v;
            cur += 1;
        }
        self.bundles[n].cur_dec = Some(cur);
        Ok(())
    }

    fn read_color(&mut self, gb: &mut BitReader, tree: &Tree) -> i32 {
        let high = self.col_high[self.col_lastval];
        self.col_lastval = self.get_huff(gb, &high) as usize;
        let mut v = self.get_huff(gb, tree);
        v |= (self.col_lastval as i32) << 4;
        if self.version < b'i' {
            let sign = ((v as u8 as i8) >> 7) as i32;
            v = ((v & 0x7F) ^ sign) - sign;
            v += 0x80;
        }
        v & 0xFF
    }

    fn read_colors(&mut self, gb: &mut BitReader) -> Result<(), Error> {
        let n = SRC_COLORS;
        let Some((mut cur, t)) = self.check_read_val(gb, n) else { return Ok(()) };
        let end = cur + t;
        if end > self.bundles[n].data.len() {
            return Err(bad("too many color values"));
        }
        if gb.left() < 1 {
            return Err(bad("truncated colors"));
        }
        let tree = self.bundles[n].tree;
        if gb.bit() {
            let v = self.read_color(gb, &tree);
            self.bundles[n].data[cur..end].fill(v);
            cur = end;
        } else {
            while cur < end {
                if gb.left() < 2 {
                    return Err(bad("truncated colors"));
                }
                let v = self.read_color(gb, &tree);
                self.bundles[n].data[cur] = v;
                cur += 1;
            }
        }
        self.bundles[n].cur_dec = Some(cur);
        Ok(())
    }

    fn read_dcs(&mut self, gb: &mut BitReader, n: usize, start_bits: u32, has_sign: bool) -> Result<(), Error> {
        let Some((mut dst, len)) = self.check_read_val(gb, n) else { return Ok(()) };
        let dst_end = self.bundles[n].data.len();
        let sb = start_bits - has_sign as u32;
        if gb.left() < sb as isize {
            return Err(bad("truncated DC values"));
        }
        let mut v = gb.get(sb) as i32;
        if v != 0 && has_sign && gb.bit() {
            v = -v;
        }
        if dst_end - dst < 1 {
            return Err(bad("too many DC values"));
        }
        let data = &mut self.bundles[n].data;
        data[dst] = v;
        dst += 1;
        let len = len - 1;
        let mut i = 0;
        while i < len {
            let len2 = (len - i).min(8);
            if dst_end - dst < len2 {
                return Err(bad("too many DC values"));
            }
            let bsize = gb.get(4);
            if bsize != 0 {
                for _ in 0..len2 {
                    let mut v2 = gb.get(bsize) as i32;
                    if v2 != 0 && gb.bit() {
                        v2 = -v2;
                    }
                    v += v2;
                    data[dst] = v;
                    dst += 1;
                    if !(-32768..=32767).contains(&v) {
                        return Err(bad("DC value went out of bounds"));
                    }
                }
            } else {
                data[dst..dst + len2].fill(v);
                dst += len2;
            }
            i += 8;
        }
        self.bundles[n].cur_dec = Some(dst);
        Ok(())
    }

    #[inline]
    fn get_value(&mut self, n: usize) -> Result<i32, Error> {
        let b = &mut self.bundles[n];
        let v = *b.data.get(b.cur_ptr).ok_or_else(|| bad("bundle overrun"))?;
        b.cur_ptr += 1;
        Ok(v)
    }

    fn read_dct_coeffs(&self, gb: &mut BitReader, block: &mut [i32; 64], coef_idx: &mut [usize; 64]) -> Result<(usize, usize), Error> {
        let scan = &BINK_SCAN;
        let mut coef_list = [0usize; 128];
        let mut mode_list = [0u8; 128];
        let mut list_start = 64usize;
        let mut list_end = 64usize;
        let mut coef_count = 0usize;
        if gb.left() < 4 {
            return Err(bad("truncated DCT block"));
        }
        for (c, m) in [(4, 0), (24, 0), (44, 0), (1, 3), (2, 3), (3, 3)] {
            coef_list[list_end] = c;
            mode_list[list_end] = m;
            list_end += 1;
        }
        let read_coef = |gb: &mut BitReader, bits: i32| -> i32 {
            if bits == 0 {
                1 - ((gb.get(1) as i32) << 1)
            } else {
                let t = gb.get(bits as u32) as i32 | (1 << bits);
                if gb.bit() {
                    -t
                } else {
                    t
                }
            }
        };
        let mut bits = gb.get(4) as i32 - 1;
        while bits >= 0 {
            let mut list_pos = list_start;
            while list_pos < list_end {
                if (mode_list[list_pos] == 0 && coef_list[list_pos] == 0) || !gb.bit() {
                    list_pos += 1;
                    continue;
                }
                let mut ccoef = coef_list[list_pos];
                let mode = mode_list[list_pos];
                match mode {
                    0 | 2 => {
                        if mode == 0 {
                            coef_list[list_pos] = ccoef + 4;
                            mode_list[list_pos] = 1;
                        } else {
                            coef_list[list_pos] = 0;
                            mode_list[list_pos] = 0;
                            list_pos += 1;
                        }
                        for _ in 0..4 {
                            if gb.bit() {
                                list_start -= 1;
                                coef_list[list_start] = ccoef;
                                mode_list[list_start] = 3;
                            } else {
                                let t = read_coef(gb, bits);
                                block[scan[ccoef] as usize] = t;
                                coef_idx[coef_count] = ccoef;
                                coef_count += 1;
                            }
                            ccoef += 1;
                        }
                    }
                    1 => {
                        mode_list[list_pos] = 2;
                        for _ in 0..3 {
                            ccoef += 4;
                            coef_list[list_end] = ccoef;
                            mode_list[list_end] = 2;
                            list_end += 1;
                        }
                    }
                    _ => {
                        let t = read_coef(gb, bits);
                        block[scan[ccoef] as usize] = t;
                        coef_idx[coef_count] = ccoef;
                        coef_count += 1;
                        coef_list[list_pos] = 0;
                        mode_list[list_pos] = 0;
                        list_pos += 1;
                    }
                }
            }
            bits -= 1;
        }
        let quant_idx = gb.get(4) as usize;
        Ok((quant_idx, coef_count))
    }

    pub(crate) fn decode_frame(&mut self, pkt: &[u8]) -> Result<(), Error> {
        let mut gb = BitReader::new(pkt);
        let bits_count = pkt.len() * 8;
        // The previous output is the motion reference; on the first frame FFmpeg references
        // the frame being decoded, which we approximate with a copy of the (blank) buffer.
        let prev = self.last.take().unwrap_or_else(|| self.cur.clone());
        let res = self.decode_planes(&mut gb, &prev, bits_count);
        let decoded = std::mem::replace(&mut self.cur, prev);
        self.last = Some(decoded);
        res
    }

    /// The most recently decoded frame.
    pub(crate) fn output(&self) -> &Planes {
        self.last.as_ref().unwrap_or(&self.cur)
    }

    fn decode_planes(&mut self, gb: &mut BitReader, prev: &Planes, bits_count: usize) -> Result<(), Error> {
        if self.has_alpha {
            if self.version >= b'i' {
                gb.skip(32);
            }
            self.decode_plane(gb, prev, 3, false)?;
        }
        if self.version >= b'i' {
            gb.skip(32);
        }
        for plane in 0..3 {
            let plane_idx = if plane == 0 || !self.swap_planes { plane } else { plane ^ 3 };
            self.decode_plane(gb, prev, plane_idx, plane != 0)?;
            if gb.count() >= bits_count {
                break;
            }
        }
        Ok(())
    }

    fn decode_plane(&mut self, gb: &mut BitReader, prev_planes: &Planes, plane_idx: usize, is_chroma: bool) -> Result<(), Error> {
        let stride = self.cur.stride[plane_idx];
        let (bw, bh) = if is_chroma { ((self.width + 15) >> 4, (self.height + 15) >> 4) } else { ((self.width + 7) >> 3, (self.height + 7) >> 3) };
        let width = self.width >> is_chroma as u32;
        let height = self.height >> is_chroma as u32;

        if self.version == b'k' && gb.bit() {
            let fill = gb.get(8) as u8;
            let dst = &mut self.cur.data[plane_idx];
            for i in 0..height {
                dst[i * stride..i * stride + width].fill(fill);
            }
        } else {
            self.init_lengths(width.max(8), bw);
            for i in 0..NB_SRC {
                self.read_bundle(gb, i)?;
            }
            let prev = &prev_planes.data[plane_idx];
            let mut dst_plane = std::mem::take(&mut self.cur.data[plane_idx]);
            let r = self.decode_blocks(gb, &mut dst_plane, prev, stride, bw, bh);
            self.cur.data[plane_idx] = dst_plane;
            r?;
        }
        let c = gb.count() & 0x1F;
        if c != 0 {
            gb.skip(32 - c);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn put_pixels(&mut self, dst: &mut [u8], doff: usize, prev: &[u8], poff: usize, stride: usize, ref_end: usize) -> Result<(), Error> {
        let xoff = self.get_value(SRC_X_OFF)? as isize;
        let yoff = self.get_value(SRC_Y_OFF)? as isize;
        let r = poff as isize + xoff + yoff * stride as isize;
        if r < 0 || r as usize > ref_end {
            return Err(bad("motion copy out of bounds"));
        }
        let r = r as usize;
        for j in 0..8 {
            dst[doff + j * stride..doff + j * stride + 8].copy_from_slice(&prev[r + j * stride..r + j * stride + 8]);
        }
        Ok(())
    }

    fn decode_blocks(&mut self, gb: &mut BitReader, dst: &mut [u8], prev: &[u8], stride: usize, bw: usize, bh: usize) -> Result<(), Error> {
        let ref_end = (bw - 1 + stride * (bh - 1)) * 8;
        let mut coordmap = [0usize; 64];
        for (i, c) in coordmap.iter_mut().enumerate() {
            *c = (i & 7) + (i >> 3) * stride;
        }
        let mut ublock = [0u8; 64];
        let mut dctblock: [i32; 64];
        let mut coef_idx = [0usize; 64];
        for by in 0..bh {
            self.read_block_types(gb, SRC_BLOCK_TYPES)?;
            self.read_block_types(gb, SRC_SUB_BLOCK_TYPES)?;
            self.read_colors(gb)?;
            self.read_patterns(gb)?;
            self.read_motion_values(gb, SRC_X_OFF)?;
            self.read_motion_values(gb, SRC_Y_OFF)?;
            self.read_dcs(gb, SRC_INTRA_DC, DC_START_BITS, false)?;
            self.read_dcs(gb, SRC_INTER_DC, DC_START_BITS, true)?;
            self.read_runs(gb)?;

            let row = 8 * by * stride;
            let mut bx = 0;
            while bx < bw {
                let off = row + bx * 8;
                let blk = self.get_value(SRC_BLOCK_TYPES)?;
                if ((by & 1) != 0 || (bx & 1) != 0) && blk == SCALED_BLOCK {
                    bx += 2;
                    continue;
                }
                match blk {
                    SKIP_BLOCK => {
                        for j in 0..8 {
                            let o = off + j * stride;
                            dst[o..o + 8].copy_from_slice(&prev[o..o + 8]);
                        }
                    }
                    SCALED_BLOCK => {
                        let sub = self.get_value(SRC_SUB_BLOCK_TYPES)?;
                        match sub {
                            RUN_BLOCK => {
                                if gb.left() < 4 {
                                    return Err(bad("truncated run block"));
                                }
                                let scan = &BINK_PATTERNS[gb.get(4) as usize];
                                self.decode_run(gb, scan, |idx, v| ublock[idx as usize] = v as u8)?;
                            }
                            INTRA_BLOCK => {
                                dctblock = [0; 64];
                                dctblock[0] = self.get_value(SRC_INTRA_DC)?;
                                let (q, cc) = self.read_dct_coeffs(gb, &mut dctblock, &mut coef_idx)?;
                                unquantize(&mut dctblock, &BINK_INTRA_QUANT[q], cc, &coef_idx);
                                idct_put(&mut ublock, 0, 8, &dctblock);
                            }
                            FILL_BLOCK => {
                                let v = self.get_value(SRC_COLORS)? as u8;
                                for j in 0..16 {
                                    let o = off + j * stride;
                                    dst[o..o + 16].fill(v);
                                }
                            }
                            PATTERN_BLOCK => {
                                let col = [self.get_value(SRC_COLORS)? as u8, self.get_value(SRC_COLORS)? as u8];
                                for j in 0..8 {
                                    let mut v = self.get_value(SRC_PATTERN)?;
                                    for i in 0..8 {
                                        ublock[i + j * 8] = col[(v & 1) as usize];
                                        v >>= 1;
                                    }
                                }
                            }
                            RAW_BLOCK => {
                                for px in ublock.iter_mut() {
                                    *px = self.get_value(SRC_COLORS)? as u8;
                                }
                            }
                            _ => return Err(bad("incorrect 16x16 block type")),
                        }
                        if sub != FILL_BLOCK {
                            // scale_block
                            for j in 0..8 {
                                for i in 0..8 {
                                    let v = ublock[j * 8 + i];
                                    let o1 = off + 2 * j * stride + 2 * i;
                                    let o2 = o1 + stride;
                                    dst[o1] = v;
                                    dst[o1 + 1] = v;
                                    dst[o2] = v;
                                    dst[o2 + 1] = v;
                                }
                            }
                        }
                        bx += 1;
                    }
                    MOTION_BLOCK => self.put_pixels(dst, off, prev, off, stride, ref_end)?,
                    RUN_BLOCK => {
                        let scan = &BINK_PATTERNS[gb.get(4) as usize];
                        self.decode_run(gb, scan, |idx, v| dst[off + coordmap[idx as usize]] = v as u8)?;
                    }
                    RESIDUE_BLOCK => {
                        self.put_pixels(dst, off, prev, off, stride, ref_end)?;
                        let mut block = [0i16; 64];
                        let v = gb.get(7) as i32;
                        read_residue(gb, &mut block, v);
                        for j in 0..8 {
                            for i in 0..8 {
                                let p = &mut dst[off + j * stride + i];
                                *p = (*p as i32 + block[j * 8 + i] as i32) as u8;
                            }
                        }
                    }
                    INTRA_BLOCK => {
                        dctblock = [0; 64];
                        dctblock[0] = self.get_value(SRC_INTRA_DC)?;
                        let (q, cc) = self.read_dct_coeffs(gb, &mut dctblock, &mut coef_idx)?;
                        unquantize(&mut dctblock, &BINK_INTRA_QUANT[q], cc, &coef_idx);
                        idct_put(dst, off, stride, &dctblock);
                    }
                    FILL_BLOCK => {
                        let v = self.get_value(SRC_COLORS)? as u8;
                        for j in 0..8 {
                            let o = off + j * stride;
                            dst[o..o + 8].fill(v);
                        }
                    }
                    INTER_BLOCK => {
                        self.put_pixels(dst, off, prev, off, stride, ref_end)?;
                        dctblock = [0; 64];
                        dctblock[0] = self.get_value(SRC_INTER_DC)?;
                        let (q, cc) = self.read_dct_coeffs(gb, &mut dctblock, &mut coef_idx)?;
                        unquantize(&mut dctblock, &BINK_INTER_QUANT[q], cc, &coef_idx);
                        idct_add(dst, off, stride, &mut dctblock);
                    }
                    PATTERN_BLOCK => {
                        let col = [self.get_value(SRC_COLORS)? as u8, self.get_value(SRC_COLORS)? as u8];
                        for i in 0..8 {
                            let mut v = self.get_value(SRC_PATTERN)?;
                            for j in 0..8 {
                                dst[off + i * stride + j] = col[(v & 1) as usize];
                                v >>= 1;
                            }
                        }
                    }
                    RAW_BLOCK => {
                        for i in 0..8 {
                            for j in 0..8 {
                                dst[off + i * stride + j] = self.get_value(SRC_COLORS)? as u8;
                            }
                        }
                    }
                    _ => return Err(bad("unknown block type")),
                }
                bx += 1;
            }
        }
        Ok(())
    }

    /// Shared body of the RUN block decoders.
    fn decode_run(&mut self, gb: &mut BitReader, scan: &[u8; 64], mut put: impl FnMut(u8, i32)) -> Result<(), Error> {
        let mut si = 0usize;
        let mut i = 0usize;
        loop {
            let run = self.get_value(SRC_RUN)? as usize + 1;
            i += run;
            if i > 64 {
                return Err(bad("run went out of bounds"));
            }
            if gb.bit() {
                let v = self.get_value(SRC_COLORS)?;
                for _ in 0..run {
                    put(scan[si], v);
                    si += 1;
                }
            } else {
                for _ in 0..run {
                    let v = self.get_value(SRC_COLORS)?;
                    put(scan[si], v);
                    si += 1;
                }
            }
            if i >= 63 {
                break;
            }
        }
        if i == 63 {
            let v = self.get_value(SRC_COLORS)?;
            put(scan[si], v);
        }
        Ok(())
    }
}

fn read_tree(gb: &mut BitReader) -> Result<Tree, Error> {
    let mut tree = Tree::default();
    if gb.left() < 4 {
        return Err(bad("truncated tree"));
    }
    tree.vlc_num = gb.get(4) as usize;
    if tree.vlc_num == 0 {
        for i in 0..16 {
            tree.syms[i] = i as u8;
        }
        return Ok(tree);
    }
    if gb.bit() {
        let mut tmp1 = [false; 16];
        let mut len = gb.get(3) as usize;
        for i in 0..=len {
            tree.syms[i] = gb.get(4) as u8;
            tmp1[tree.syms[i] as usize] = true;
        }
        let mut i = 0;
        while i < 16 && len < 15 {
            if !tmp1[i] {
                len += 1;
                tree.syms[len] = i as u8;
            }
            i += 1;
        }
    } else {
        let len = gb.get(2) as usize;
        let mut inp = [0u8; 16];
        for (i, v) in inp.iter_mut().enumerate() {
            *v = i as u8;
        }
        let mut out = [0u8; 16];
        for i in 0..=len {
            let size = 1usize << i;
            let mut t = 0;
            while t < 16 {
                merge(gb, &mut out[t..], &inp[t..], size);
                t += size << 1;
            }
            std::mem::swap(&mut inp, &mut out);
        }
        tree.syms = inp;
    }
    Ok(tree)
}

fn merge(gb: &mut BitReader, dst: &mut [u8], src: &[u8], size: usize) {
    let (mut p1, mut p2) = (0usize, size);
    let (mut s1, mut s2) = (size, size);
    let mut d = 0;
    loop {
        if !gb.bit() {
            dst[d] = src[p1];
            p1 += 1;
            s1 -= 1;
        } else {
            dst[d] = src[p2];
            p2 += 1;
            s2 -= 1;
        }
        d += 1;
        if s1 == 0 || s2 == 0 {
            break;
        }
    }
    while s1 > 0 {
        dst[d] = src[p1];
        d += 1;
        p1 += 1;
        s1 -= 1;
    }
    while s2 > 0 {
        dst[d] = src[p2];
        d += 1;
        p2 += 1;
        s2 -= 1;
    }
}

fn unquantize(block: &mut [i32; 64], quant: &[u32; 64], coef_count: usize, coef_idx: &[usize; 64]) {
    block[0] = block[0].wrapping_mul(quant[0] as i32) >> 11;
    for &idx in &coef_idx[..coef_count] {
        let s = BINK_SCAN[idx] as usize;
        block[s] = block[s].wrapping_mul(quant[idx] as i32) >> 11;
    }
}

fn read_residue(gb: &mut BitReader, block: &mut [i16; 64], mut masks_count: i32) {
    let mut coef_list = [0usize; 128];
    let mut mode_list = [0u8; 128];
    let mut list_start = 64usize;
    let mut list_end = 64usize;
    let mut nz_coeff = [0usize; 64];
    let mut nz_count = 0usize;
    for (c, m) in [(4, 0), (24, 0), (44, 0), (0, 2)] {
        coef_list[list_end] = c;
        mode_list[list_end] = m;
        list_end += 1;
    }
    let mut mask: i32 = 1 << gb.get(3);
    while mask != 0 {
        for &nz in &nz_coeff[..nz_count] {
            if !gb.bit() {
                continue;
            }
            if block[nz] < 0 {
                block[nz] = block[nz].wrapping_sub(mask as i16);
            } else {
                block[nz] = block[nz].wrapping_add(mask as i16);
            }
            masks_count -= 1;
            if masks_count < 0 {
                return;
            }
        }
        let mut list_pos = list_start;
        while list_pos < list_end {
            if (coef_list[list_pos] == 0 && mode_list[list_pos] == 0) || !gb.bit() {
                list_pos += 1;
                continue;
            }
            let mut ccoef = coef_list[list_pos];
            let mode = mode_list[list_pos];
            match mode {
                0 | 2 => {
                    if mode == 0 {
                        coef_list[list_pos] = ccoef + 4;
                        mode_list[list_pos] = 1;
                    } else {
                        coef_list[list_pos] = 0;
                        mode_list[list_pos] = 0;
                        list_pos += 1;
                    }
                    for _ in 0..4 {
                        if gb.bit() {
                            list_start -= 1;
                            coef_list[list_start] = ccoef;
                            mode_list[list_start] = 3;
                        } else {
                            let s = BINK_SCAN[ccoef] as usize;
                            nz_coeff[nz_count] = s;
                            nz_count += 1;
                            block[s] = if gb.bit() { -mask } else { mask } as i16;
                            masks_count -= 1;
                            if masks_count < 0 {
                                return;
                            }
                        }
                        ccoef += 1;
                    }
                }
                1 => {
                    mode_list[list_pos] = 2;
                    for _ in 0..3 {
                        ccoef += 4;
                        coef_list[list_end] = ccoef;
                        mode_list[list_end] = 2;
                        list_end += 1;
                    }
                }
                _ => {
                    let s = BINK_SCAN[ccoef] as usize;
                    nz_coeff[nz_count] = s;
                    nz_count += 1;
                    block[s] = if gb.bit() { -mask } else { mask } as i16;
                    coef_list[list_pos] = 0;
                    mode_list[list_pos] = 0;
                    list_pos += 1;
                    masks_count -= 1;
                    if masks_count < 0 {
                        return;
                    }
                }
            }
        }
        mask >>= 1;
    }
}

// ---- binkdsp ----

const A1: i32 = 2896;
const A2: i32 = 2217;
const A3: i32 = 3784;
const A4: i32 = -5352;

#[inline]
fn mul(x: i32, y: i32) -> i32 {
    x.wrapping_mul(y) >> 11
}

/// One 8-point IDCT pass over `src[s(k)]`, returning the 8 un-munged outputs.
#[inline]
fn idct_transform(s: [i32; 8]) -> [i32; 8] {
    let a0 = s[0].wrapping_add(s[4]);
    let a1 = s[0].wrapping_sub(s[4]);
    let a2 = s[2].wrapping_add(s[6]);
    let a3 = mul(A1, s[2].wrapping_sub(s[6]));
    let a4 = s[5].wrapping_add(s[3]);
    let a5 = s[5].wrapping_sub(s[3]);
    let a6 = s[1].wrapping_add(s[7]);
    let a7 = s[1].wrapping_sub(s[7]);
    let b0 = a4.wrapping_add(a6);
    let b1 = mul(A3, a5.wrapping_add(a7));
    let b2 = mul(A4, a5).wrapping_sub(b0).wrapping_add(b1);
    let b3 = mul(A1, a6.wrapping_sub(a4)).wrapping_sub(b2);
    let b4 = mul(A2, a7).wrapping_add(b3).wrapping_sub(b1);
    [
        a0.wrapping_add(a2).wrapping_add(b0),
        a1.wrapping_add(a3).wrapping_sub(a2).wrapping_add(b2),
        a1.wrapping_sub(a3).wrapping_add(a2).wrapping_add(b3),
        a0.wrapping_sub(a2).wrapping_sub(b4),
        a0.wrapping_sub(a2).wrapping_add(b4),
        a1.wrapping_sub(a3).wrapping_add(a2).wrapping_sub(b3),
        a1.wrapping_add(a3).wrapping_sub(a2).wrapping_sub(b2),
        a0.wrapping_add(a2).wrapping_sub(b0),
    ]
}

/// Column pass, then row pass; returns the 64 row outputs after `MUNGE_ROW`.
fn idct(block: &[i32; 64]) -> [i32; 64] {
    let mut temp = [0i32; 64];
    for i in 0..8 {
        let col: [i32; 8] = std::array::from_fn(|k| block[i + 8 * k]);
        if col[1..].iter().all(|&v| v == 0) {
            for k in 0..8 {
                temp[i + 8 * k] = col[0];
            }
        } else {
            let o = idct_transform(col);
            for k in 0..8 {
                temp[i + 8 * k] = o[k];
            }
        }
    }
    let mut out = [0i32; 64];
    for i in 0..8 {
        let row: [i32; 8] = std::array::from_fn(|k| temp[8 * i + k]);
        let o = idct_transform(row);
        for k in 0..8 {
            out[8 * i + k] = o[k].wrapping_add(0x7F) >> 8;
        }
    }
    out
}

fn idct_put(dst: &mut [u8], off: usize, stride: usize, block: &[i32; 64]) {
    let out = idct(block);
    for i in 0..8 {
        for j in 0..8 {
            dst[off + i * stride + j] = out[i * 8 + j] as u8;
        }
    }
}

fn idct_add(dst: &mut [u8], off: usize, stride: usize, block: &mut [i32; 64]) {
    let out = idct(block);
    for i in 0..8 {
        for j in 0..8 {
            let p = &mut dst[off + i * stride + j];
            *p = (*p as i32).wrapping_add(out[i * 8 + j]) as u8;
        }
    }
}
