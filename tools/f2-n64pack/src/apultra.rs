//! Bit-exact pure-Rust port of the **apultra** aPLib compressor, as vendored by libdragon
//! (`tools/common/apultra`, upstream commit `8f340057d74`) and used by `mkasset -c 2`.
//!
//! The output (bytes *and* `stats.safe_dist`) is identical to the C code for every input, so the
//! N64 ROM exporter can produce libdragon `DCA5` aPLib assets without the C toolchain. The code
//! is kept structurally close to upstream: every function names its C counterpart in its doc
//! comment so it can be diffed against `shrink.c` / `matchfinder.c`.
//!
//! Differences from the C that do not affect the output:
//! - the suffix array comes from our own SA-IS ([`suffix_array`]) instead of libdivsufsort (the
//!   suffix array of a byte string is unique);
//! - `rle_len` / `visited` live in their own buffers instead of aliasing `intervals` /
//!   `pos_data` (every read of them in the C observes data written in the same phase);
//! - C bitfields (`apultra_match`, `apultra_arrival`) are packed into `u32`s with the same
//!   widths, so assignments truncate exactly like the C.
//!
//! ---
//!
//! Original notice (shrink.c / shrink.h / format.h), which must be kept:
//!
//! ```text
//! Copyright (C) 2019 Emmanuel Marty
//!
//! This software is provided 'as-is', without any express or implied
//! warranty.  In no event will the authors be held liable for any damages
//! arising from the use of this software.
//!
//! Permission is granted to anyone to use this software for any purpose,
//! including commercial applications, and to alter it and redistribute it
//! freely, subject to the following restrictions:
//!
//! 1. The origin of this software must not be misrepresented; you must not
//!    claim that you wrote the original software. If you use this software
//!    in a product, an acknowledgment in the product documentation would be
//!    appreciated but is not required.
//! 2. Altered source versions must be plainly marked as such, and must not be
//!    misrepresented as being the original software.
//! 3. This notice may not be removed or altered from any source distribution.
//!
//! Inspired by cap by Sven-Åke Dahl. https://github.com/svendahl/cap
//! Also inspired by Charles Bloom's compression blog. http://cbloomrants.blogspot.com/
//! With ideas from LZ4 by Yann Collet. https://github.com/lz4/lz4
//! With help and support from spke <zxintrospec@gmail.com>
//! ```
//!
//! matchfinder.c: written in 2019 by Emmanuel Marty, portions written in 2014-2015 by Eric
//! Biggers (interval construction from wimlib); dedicated to the public domain (CC0 1.0).
//!
//! **This is an altered source version** (a Rust translation), not the original software.

// The C uses `int` everywhere; keep the arithmetic in i32 so wrap/truncation behaves the same.
// Comparisons and accessor names mirror the C (`from_pos`, `from_slot` are bitfield names).
#![allow(clippy::too_many_arguments, clippy::needless_range_loop, clippy::manual_range_contains, clippy::wrong_self_convention)]

// ---- format.h ----
const MIN_OFFSET: i32 = 1;
const MAX_OFFSET: i32 = 0x1fffff;
const MAX_VARLEN: i32 = 0x1fffff;
const BLOCK_SIZE: usize = 0x100000;
const MIN_MATCH_SIZE: i32 = 1;
const MINMATCH3_OFFSET: i32 = 1280;
const MINMATCH4_OFFSET: i32 = 32000;

// ---- shrink.h ----
const LCP_BITS: u32 = 15;
const TAG_BITS: u32 = 4;
const LCP_MAX: i32 = (1 << (LCP_BITS - TAG_BITS)) - 1;
const LCP_AND_TAG_MAX: usize = (1 << LCP_BITS) - 1;
const LCP_SHIFT: u32 = 63 - LCP_BITS;
const LCP_MASK: u64 = ((1u64 << LCP_BITS) - 1) << LCP_SHIFT;
const POS_MASK: u64 = (1u64 << LCP_SHIFT) - 1;
const VISITED_FLAG: u64 = 0x8000000000000000;
const EXCL_VISITED_MASK: u64 = 0x7fffffffffffffff;

const NARRIVALS_PER_POSITION_MAX: usize = 62;
const NARRIVALS_PER_POSITION_NORMAL: usize = 46;
const NARRIVALS_PER_POSITION_SMALL: usize = 9;

const NMATCHES_PER_INDEX: usize = 64;
const MATCHES_PER_INDEX_SHIFT: u32 = 6;

const LEAVE_ALONE_MATCH_SIZE: i32 = 120;

// ---- shrink.c ----
const TOKEN_CODE_LARGE_MATCH: i32 = 2; // 10
const TOKEN_SIZE_LARGE_MATCH: i32 = 2;
const TOKEN_CODE_7BIT_MATCH: i32 = 6; // 110
const TOKEN_SIZE_7BIT_MATCH: i32 = 3;
const TOKEN_CODE_4BIT_MATCH: i32 = 7; // 111
const TOKEN_SIZE_4BIT_MATCH: i32 = 3;

/// `apultra_match` — `length:11`, `offset:21` packed into one word.
#[derive(Clone, Copy, Default)]
struct Match(u32);

impl Match {
    #[inline]
    fn length(self) -> i32 {
        (self.0 & 0x7ff) as i32
    }
    #[inline]
    fn offset(self) -> i32 {
        (self.0 >> 11) as i32
    }
    #[inline]
    fn set_length(&mut self, v: i32) {
        self.0 = (self.0 & !0x7ff) | (v as u32 & 0x7ff);
    }
    #[inline]
    fn set_offset(&mut self, v: i32) {
        self.0 = (self.0 & 0x7ff) | ((v as u32 & 0x1fffff) << 11);
    }
}

/// `apultra_final_match`
#[derive(Clone, Copy, Default)]
struct FinalMatch {
    length: i32,
    offset: i32,
}

/// `apultra_arrival` with the C bitfield widths:
/// `a` = `from_pos:21 | from_slot:7 (signed) | follows_literal:1`,
/// `b` = `rep_offset:21 | short_offset:4`, `c` = `rep_pos:21 | match_len:11`.
#[derive(Clone, Copy, Default)]
struct Arrival {
    cost: i32,
    a: u32,
    b: u32,
    c: u32,
    score: i32,
}

impl Arrival {
    #[inline]
    fn from_pos(&self) -> u32 {
        self.a & 0x1fffff
    }
    #[inline]
    fn from_slot(&self) -> i32 {
        (((self.a >> 21) & 0x7f) as i32) << 25 >> 25
    }
    #[inline]
    fn follows_literal(&self) -> i32 {
        ((self.a >> 28) & 1) as i32
    }
    #[inline]
    fn rep_offset(&self) -> i32 {
        (self.b & 0x1fffff) as i32
    }
    #[inline]
    fn short_offset(&self) -> i32 {
        ((self.b >> 21) & 0xf) as i32
    }
    #[inline]
    fn rep_pos(&self) -> i32 {
        (self.c & 0x1fffff) as i32
    }
    #[inline]
    fn match_len(&self) -> i32 {
        (self.c >> 21) as i32
    }
    /// Assigns every field, truncating to the C bitfield widths.
    #[inline]
    fn set(
        &mut self,
        cost: i32,
        from_pos: i32,
        from_slot: i32,
        follows_literal: i32,
        rep_offset: i32,
        short_offset: i32,
        rep_pos: i32,
        match_len: i32,
        score: i32,
    ) {
        self.cost = cost;
        self.a = (from_pos as u32 & 0x1fffff) | ((from_slot as u32 & 0x7f) << 21) | ((follows_literal as u32 & 1) << 28);
        self.b = (rep_offset as u32 & 0x1fffff) | ((short_offset as u32 & 0xf) << 21);
        self.c = (rep_pos as u32 & 0x1fffff) | ((match_len as u32 & 0x7ff) << 21);
        self.score = score;
    }
}

/// `apultra_stats` — compression statistics (all fields, as in the C).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub num_literals: i32,
    pub num_4bit_matches: i32,
    pub num_7bit_matches: i32,
    pub num_variable_matches: i32,
    pub num_rep_matches: i32,
    pub num_eod: i32,
    /// Largest (decoder output position − decoder input position) seen while emitting; used by
    /// libdragon to compute the in-place decompression margin.
    pub safe_dist: i32,
    pub min_offset: i32,
    pub max_offset: i32,
    pub total_offsets: i64,
    pub min_match_len: i32,
    pub max_match_len: i32,
    pub total_match_lens: i32,
    pub min_rle1_len: i32,
    pub max_rle1_len: i32,
    pub total_rle1_lens: i32,
    pub min_rle2_len: i32,
    pub max_rle2_len: i32,
    pub total_rle2_lens: i32,
    pub commands_divisor: i32,
    pub match_divisor: i32,
    pub rle1_divisor: i32,
    pub rle2_divisor: i32,
}

/// `apultra_compressor` — compression context.
struct Compressor {
    intervals: Vec<u64>,
    pos_data: Vec<u64>,
    /// Permuted LCP / Phi (in the C, a temporary `int` view of `pos_data`).
    plcp: Vec<i32>,
    open_intervals: Vec<u64>,
    match_: Vec<Match>,
    match_depth: Vec<u16>,
    match1: Vec<u8>,
    best_match: Vec<FinalMatch>,
    arrival: Vec<Arrival>,
    first_offset_for_byte: Vec<i32>,
    next_offset_for_pos: Vec<i32>,
    offset_cache: Vec<i32>,
    /// In the C, an `int` view of `intervals` ("reuse").
    rle_len: Vec<i32>,
    /// In the C, an `int` view of `pos_data` ("reuse"), indexed relative to the block start.
    visited: Vec<i32>,
    block_size: i32,
    max_offset: i32,
    max_arrivals: usize,
    stats: Stats,
}

/// `apultra_get_index_tag` — hash index into TAG_BITS.
#[inline]
fn get_index_tag(index: u32) -> i32 {
    ((index as u64).wrapping_mul(11400714819323198485) >> (64 - TAG_BITS)) as i32
}

/// `apultra_get_gamma2_size` — number of bits of a gamma2-encoded value.
#[inline]
fn get_gamma2_size(value: i32) -> i32 {
    if (0..2048).contains(&value) {
        // `_gamma2_size[]` table: 2 * floor(log2(n)) for n >= 2, 0 for 0 and 1.
        if value < 2 {
            0
        } else {
            2 * (31 - value.leading_zeros() as i32)
        }
    } else {
        let mut v = value;
        let mut n: u32 = 0;
        for bits in [16, 8, 4, 2, 1] {
            // CountShift(N, bits)
            if (v >> bits) != 0 {
                v >>= bits;
                n += bits;
            }
        }
        (n << 1) as i32
    }
}

/// `apultra_get_offset_varlen_size`
#[inline]
fn get_offset_varlen_size(length: i32, match_offset: i32, follows_literal: i32) -> i32 {
    if length <= 3 && match_offset < 128 {
        8 + TOKEN_SIZE_7BIT_MATCH
    } else if follows_literal != 0 {
        8 + TOKEN_SIZE_LARGE_MATCH + get_gamma2_size((match_offset >> 8) + 3)
    } else {
        8 + TOKEN_SIZE_LARGE_MATCH + get_gamma2_size((match_offset >> 8) + 2)
    }
}

/// `apultra_get_match_varlen_size`
#[inline]
fn get_match_varlen_size(length: i32, match_offset: i32) -> i32 {
    if length <= 3 && match_offset < 128 {
        0
    } else if match_offset < 128 || match_offset >= MINMATCH4_OFFSET {
        get_gamma2_size(length - 2)
    } else if match_offset < MINMATCH3_OFFSET {
        get_gamma2_size(length)
    } else {
        get_gamma2_size(length - 1)
    }
}

/// `memcmp(a, b, n) == 0` on window offsets.
#[inline]
fn eq(w: &[u8], a: usize, b: usize, n: usize) -> bool {
    w[a..a + n] == w[b..b + n]
}

/// Bit writer state shared across blocks: the output buffer, the start of the current block in
/// it (`pOutData`), the max size of the current block, and `nCurBitsOffset` / `nCurBitShift`.
/// `bits_offset` is relative to `base` and may be negative (a tag byte from the previous block).
struct BitWriter<'a> {
    out: &'a mut [u8],
    base: usize,
    max: i32,
    bits_offset: i32,
    bit_shift: i32,
}

impl BitWriter<'_> {
    #[inline]
    fn at(&mut self, off: i32) -> &mut u8 {
        &mut self.out[(self.base as isize + off as isize) as usize]
    }

    /// `apultra_write_bits`
    fn write_bits(&mut self, mut out_offset: i32, value: i32, bits: i32) -> i32 {
        if out_offset >= 0 {
            for i in (0..bits).rev() {
                if self.bit_shift == -1 {
                    // Allocate a new byte in the stream to pack bits in
                    if out_offset >= self.max {
                        return -1;
                    }
                    self.bits_offset = out_offset;
                    self.bit_shift = 7;
                    *self.at(out_offset) = 0;
                    out_offset += 1;
                }
                let (bo, bs) = (self.bits_offset, self.bit_shift);
                *self.at(bo) |= (((value >> i) & 1) << bs) as u8;
                self.bit_shift -= 1;
            }
        }
        out_offset
    }

    /// `apultra_write_gamma2_value`
    fn write_gamma2_value(&mut self, mut out_offset: i32, value: i32) -> i32 {
        let mut msb: i32 = 30;
        loop {
            let v = value >> msb;
            msb -= 1;
            if v != 0 {
                break;
            }
        }
        while msb > 0 {
            msb -= 1;
            let bit = (value >> msb) & 2;
            out_offset = self.write_bits(out_offset, bit | 1, 2);
        }
        self.write_bits(out_offset, (value & 1) << 1, 2)
    }

    /// Writes a whole byte at `out_offset` (the `pOutData[nOutOffset++] = x` pattern, including the
    /// bounds check that precedes it). Returns the new offset or -1.
    #[inline]
    fn put_byte(&mut self, out_offset: i32, b: u8) -> i32 {
        if out_offset < 0 || out_offset >= self.max {
            return -1;
        }
        *self.at(out_offset) = b;
        out_offset + 1
    }
}

/// Mutable match-list writer used by `apultra_find_matches_at` (`matchptr`, `depthptr`,
/// `cur_depth`, `nCurDepth`, `nPrevLen`, `nPrevOffset`).
struct MatchList<'a> {
    matches: &'a mut [Match],
    depths: &'a mut [u16],
    count: i32,
    max: i32,
    cur_depth: Option<usize>,
    n_cur_depth: i32,
    prev_len: i32,
    prev_offset: i32,
}

impl MatchList<'_> {
    /// Either extends the depth of the previous match (when this one is the previous one shifted
    /// by one) or appends a new match. `flag` is 0x8000 for the indirect ascents, 0 otherwise.
    #[inline]
    fn push(&mut self, len: i32, offset: i32, flag: u16) {
        let deepen = self.prev_offset != 0 && self.prev_len > 2 && offset == self.prev_offset - 1 && len == self.prev_len - 1 && self.n_cur_depth < LCP_MAX;
        if let (true, Some(cd)) = (deepen, self.cur_depth) {
            self.n_cur_depth += 1;
            self.depths[cd] = (self.n_cur_depth as u16) | flag;
        } else {
            let idx = self.count as usize;
            let mut m = Match(0);
            m.set_length(len);
            m.set_offset(offset);
            self.matches[idx] = m;
            self.count += 1;
            self.n_cur_depth = 0;
            self.depths[idx] = flag;
            self.cur_depth = Some(idx);
        }
        self.prev_len = len;
        self.prev_offset = offset;
    }
}

/// `apultra_find_matches_at` — find matches at the specified offset in the input window.
/// Returns `(number of matches, match1)`.
fn find_matches_at(intervals: &mut [u64], pos_data: &mut [u64], max_offset: i32, offset: i32, list: &mut MatchList, self_contained: bool) -> u8 {
    let mut match1: u8 = 0;
    let off = offset as u64;

    // Get the deepest lcp-interval containing the current suffix.
    let mut ref_ = pos_data[offset as usize];
    pos_data[offset as usize] = 0;

    // Ascend until we reach a visited interval, the root, or a child of the root. Link unvisited
    // intervals to the current suffix as we go.
    let mut super_ref;
    loop {
        super_ref = intervals[(ref_ & POS_MASK) as usize];
        if super_ref & LCP_MASK == 0 {
            break;
        }
        intervals[(ref_ & POS_MASK) as usize] = off | VISITED_FLAG;
        ref_ = super_ref;
    }

    if super_ref == 0 {
        // The current interval may be the root or an unvisited child of the root.
        if ref_ != 0 {
            intervals[(ref_ & POS_MASK) as usize] = off | VISITED_FLAG;
        }
        return 0;
    }

    // Ascend indirectly via pos_data[] links.
    let mut match_pos = super_ref & EXCL_VISITED_MASK;
    let mlen = |r: u64| (r >> (LCP_SHIFT + TAG_BITS)) as i32;
    let moff = |p: u64| off.wrapping_sub(p) as i32;

    if self_contained {
        let match_offset = moff(match_pos);
        if list.count < list.max {
            let match_len = mlen(ref_);
            if match_offset <= max_offset {
                list.push(match_len, match_offset, 0);
            }
        }
    }

    loop {
        super_ref = pos_data[match_pos as usize];
        if super_ref > ref_ {
            match_pos = intervals[(super_ref & POS_MASK) as usize] & EXCL_VISITED_MASK;

            if self_contained {
                let match_offset = moff(match_pos);
                if list.count < list.max {
                    let match_len = mlen(ref_);
                    if match_offset <= max_offset && (list.prev_offset - match_offset) >= 128 {
                        list.push(match_len, match_offset, 0x8000);
                    }
                }
            }
        }

        loop {
            super_ref = pos_data[match_pos as usize];
            if super_ref <= ref_ {
                break;
            }
            match_pos = intervals[(super_ref & POS_MASK) as usize] & EXCL_VISITED_MASK;

            if self_contained {
                let match_offset = moff(match_pos);
                if list.count < list.max {
                    let match_len = mlen(ref_);
                    if match_offset <= max_offset
                        && (match_len >= 3 || (match_len >= 2 && list.count < (list.max - 1)))
                        && match_len < 1280
                        && (list.prev_offset - match_offset) >= 128
                    {
                        list.push(match_len, match_offset, 0x8000);
                    }
                }
            }
        }

        intervals[(ref_ & POS_MASK) as usize] = off | VISITED_FLAG;
        pos_data[match_pos as usize] = ref_;

        let main_match_offset = moff(match_pos);
        let main_match_len = mlen(ref_);

        if list.count < list.max && main_match_offset <= max_offset && main_match_offset != list.prev_offset {
            list.push(main_match_len, main_match_offset, 0);
        }

        if main_match_offset != 0 && main_match_offset < 16 && main_match_len != 0 {
            match1 = main_match_offset as u8;
        }

        if super_ref == 0 {
            break;
        }
        ref_ = super_ref;
        match_pos = intervals[(ref_ & POS_MASK) as usize] & EXCL_VISITED_MASK;

        if self_contained {
            let match_offset = moff(match_pos);
            if list.count < list.max {
                let match_len = mlen(ref_);
                if match_offset <= max_offset && match_len >= 2 && (list.prev_offset - match_offset) >= 128 {
                    list.push(match_len, match_offset, 0x8000);
                }
            }
        }
    }

    match1
}

impl Compressor {
    /// `apultra_compressor_init`
    fn new(block_size: usize, max_window_size: usize, max_arrivals: usize) -> Self {
        Compressor {
            intervals: vec![0; max_window_size],
            pos_data: vec![0; max_window_size],
            plcp: vec![0; max_window_size],
            open_intervals: vec![0; LCP_AND_TAG_MAX + 1],
            match_: vec![Match(0); block_size * NMATCHES_PER_INDEX],
            match_depth: vec![0; block_size * NMATCHES_PER_INDEX],
            match1: vec![0; block_size],
            best_match: vec![FinalMatch::default(); block_size],
            arrival: vec![Arrival::default(); (block_size + 1) * max_arrivals],
            first_offset_for_byte: vec![0; 65536],
            next_offset_for_pos: vec![0; block_size],
            offset_cache: vec![0; 2048],
            rle_len: vec![0; max_window_size],
            visited: vec![0; block_size],
            block_size: block_size as i32,
            max_offset: 0,
            max_arrivals,
            stats: Stats { min_match_len: -1, min_offset: -1, min_rle1_len: -1, min_rle2_len: -1, ..Stats::default() },
        }
    }

    /// `apultra_build_suffix_array` — build the suffix array and the overlaid LCP-interval
    /// structures used for match finding.
    fn build_suffix_array(&mut self, window: &[u8]) {
        let n = window.len();
        let intervals = &mut self.intervals;
        let sa = suffix_array(window);
        for i in (0..n).rev() {
            intervals[i] = sa[i] as u64;
        }
        drop(sa);

        // Compute the permuted LCP first (Kärkkäinen method). Phi and PLCP share storage.
        let plcp = &mut self.plcp;
        plcp[intervals[0] as usize] = -1;
        for i in 1..n {
            plcp[intervals[i] as usize] = intervals[i - 1] as u32 as i32;
        }
        let mut cur_len: i32 = 0;
        for i in 0..n {
            let phi = plcp[i];
            if phi == -1 {
                plcp[i] = 0;
                continue;
            }
            let ii = i as i32;
            let max_len = if ii > phi { n as i32 - ii } else { n as i32 - phi };
            while cur_len < max_len && window[(ii + cur_len) as usize] == window[(phi + cur_len) as usize] {
                cur_len += 1;
            }
            plcp[i] = cur_len;
            if cur_len > 0 {
                cur_len -= 1;
            }
        }

        // Rotate permuted LCP into the LCP.
        intervals[0] &= POS_MASK;
        for i in 1..n {
            let index = (intervals[i] & POS_MASK) as i32;
            let mut len = plcp[index as usize];
            if len < MIN_MATCH_SIZE {
                len = 0;
            }
            if len > LCP_MAX {
                len = LCP_MAX;
            }
            let mut tagged_len = 0;
            if len != 0 {
                tagged_len = (len << TAG_BITS) | (get_index_tag(index as u32) & ((1 << TAG_BITS) - 1));
            }
            intervals[i] = (index as u64) | ((tagged_len as u64) << LCP_SHIFT);
        }

        // Build intervals for finding matches (from wimlib's lcpit_matchfinder.c, CC0).
        let pos_data = &mut self.pos_data;
        let open = &mut self.open_intervals;
        let mut top: usize = 0;
        let mut prev_pos = intervals[0] & POS_MASK;
        open[top] = 0;
        intervals[0] = 0;
        let mut next_interval_idx: u64 = 1;

        for r in 1..n {
            let next_pos = intervals[r] & POS_MASK;
            let next_lcp = intervals[r] & LCP_MASK;
            let top_lcp = open[top] & LCP_MASK;

            if next_lcp == top_lcp {
                // Continuing the deepest open interval
                pos_data[prev_pos as usize] = open[top];
            } else if next_lcp > top_lcp {
                // Opening a new interval
                top += 1;
                open[top] = next_lcp | next_interval_idx;
                next_interval_idx += 1;
                pos_data[prev_pos as usize] = open[top];
            } else {
                // Closing the deepest open interval
                pos_data[prev_pos as usize] = open[top];
                loop {
                    let closed_interval_idx = open[top] & POS_MASK;
                    top -= 1;
                    let superinterval_lcp = open[top] & LCP_MASK;

                    if next_lcp == superinterval_lcp {
                        // Continuing the superinterval
                        intervals[closed_interval_idx as usize] = open[top];
                        break;
                    } else if next_lcp > superinterval_lcp {
                        // Creating a new interval that is a superinterval of the one being
                        // closed, but still a subinterval of its superinterval
                        top += 1;
                        open[top] = next_lcp | next_interval_idx;
                        next_interval_idx += 1;
                        intervals[closed_interval_idx as usize] = open[top];
                        break;
                    } else {
                        // Also closing the superinterval
                        intervals[closed_interval_idx as usize] = open[top];
                    }
                }
            }
            prev_pos = next_pos;
        }

        // Close any still-open intervals.
        pos_data[prev_pos as usize] = open[top];
        while top > 0 {
            intervals[(open[top] & POS_MASK) as usize] = open[top - 1];
            top -= 1;
        }
    }

    /// `apultra_skip_matches` — skip previously compressed bytes (still updates the intervals).
    fn skip_matches(&mut self, start: i32, end: i32) {
        let mut m = [Match(0)];
        let mut d = [0u16];
        for i in start..end {
            let mut list = MatchList { matches: &mut m, depths: &mut d, count: 0, max: 0, cur_depth: None, n_cur_depth: 0, prev_len: 0, prev_offset: 0 };
            find_matches_at(&mut self.intervals, &mut self.pos_data, self.max_offset, i, &mut list, false);
        }
    }

    /// `apultra_find_all_matches` — find all matches for the data to be compressed.
    fn find_all_matches(&mut self, matches_per_offset: usize, start: i32, end: i32, block_flags: i32) {
        let self_contained = (block_flags & 3) == 3;
        for (k, i) in (start..end).enumerate() {
            let base = k * matches_per_offset;
            let mut list = MatchList {
                matches: &mut self.match_[base..base + matches_per_offset],
                depths: &mut self.match_depth[base..base + matches_per_offset],
                count: 0,
                max: matches_per_offset as i32,
                cur_depth: None,
                n_cur_depth: 0,
                prev_len: 0,
                prev_offset: 0,
            };
            let m1 = find_matches_at(&mut self.intervals, &mut self.pos_data, self.max_offset, i, &mut list, self_contained);
            let n = list.count as usize;
            if n < matches_per_offset {
                list.matches[n..].fill(Match(0));
                list.depths[n..].fill(0);
            }
            self.match1[k] = m1;
        }
    }

    /// `apultra_insert_forward_match` — insert forward rep candidate.
    fn insert_forward_match(&mut self, w: &[u8], i: i32, match_offset: i32, start: i32, end: i32, narr: usize, depth: i32) {
        let abase = (i - start) as usize * narr;

        let mut j = 0;
        while j < narr && self.arrival[abase + j].from_slot() != 0 {
            let arr = self.arrival[abase + j];
            j += 1;
            if arr.follows_literal() == 0 {
                continue;
            }
            let rep_offset = arr.rep_offset();
            if match_offset == rep_offset {
                continue;
            }
            let rep_pos = arr.rep_pos();
            if !(rep_pos >= start && (rep_pos + 1) < end && self.visited[(rep_pos - start) as usize] != match_offset) {
                continue;
            }
            self.visited[(rep_pos - start) as usize] = match_offset;

            let fbase = ((rep_pos - start) as usize) << MATCHES_PER_INDEX_SHIFT;
            if self.match_[fbase + NMATCHES_PER_INDEX - 1].length() != 0 {
                continue;
            }
            if rep_pos < match_offset {
                continue;
            }
            let ws = rep_pos as usize;
            let mo = match_offset as usize;
            if !eq(w, ws, ws - mo, 2) || rep_offset == 0 {
                continue;
            }
            let len0 = self.rle_len[ws - mo];
            let len1 = self.rle_len[ws];
            let min_len = len0.min(len1);

            let mut max_rep_len = (end - rep_pos) as u32;
            if max_rep_len > LCP_MAX as u32 {
                max_rep_len = LCP_MAX as u32;
            }
            let wmax = ws + max_rep_len as usize;
            let mut p = ws + min_len as usize;
            if p > wmax {
                p = wmax;
            }
            while p + 8 < wmax && eq(w, p, p - mo, 8) {
                p += 8;
            }
            while p + 4 < wmax && eq(w, p, p - mo, 4) {
                p += 4;
            }
            while p < wmax && w[p] == w[p - mo] {
                p += 1;
            }
            let cur_rep_len = (p - ws) as i32;

            let mut r = 0;
            while self.match_[fbase + r].length() != 0 {
                let fm = self.match_[fbase + r];
                if fm.offset() == match_offset && (self.match_depth[fbase + r] & 0x3fff) == 0 {
                    if fm.length() < cur_rep_len {
                        self.match_[fbase + r].set_length(cur_rep_len);
                        self.match_depth[fbase + r] = 0;
                    }
                    break;
                }
                r += 1;
            }

            if self.match_[fbase + r].length() == 0 {
                self.match_[fbase + r].set_length(cur_rep_len);
                self.match_[fbase + r].set_offset(match_offset);
                self.match_depth[fbase + r] = 0;

                if depth < 9 {
                    self.insert_forward_match(w, rep_pos, match_offset, start, end, narr, depth + 1);
                }
            }
        }
    }

    /// `apultra_optimize_forward` — pick optimal matches with a forward arrivals parse.
    fn optimize_forward(&mut self, w: &[u8], start: i32, end: i32, insert_forward_reps: bool, cur_rep_match_offset: i32, block_flags: i32, narr: usize) {
        if (end - start) > self.block_size {
            return;
        }

        // `arrival` in the C is offset by -start positions; index here is (pos - start) * narr.
        let npos = (end - start) as usize + 1;
        for a in &mut self.arrival[..npos * narr] {
            *a = Arrival { cost: 0x40000000, ..Arrival::default() };
        }
        self.arrival[0].cost = 0;
        self.arrival[0].set(0, 0, -1, 0, cur_rep_match_offset, 0, 0, 0, 0);

        if insert_forward_reps {
            self.visited[..(end - start) as usize].fill(0);
        }

        let mut rep_idx = [0i32; 2 * NARRIVALS_PER_POSITION_MAX + 1];

        for i in start..end {
            let cur = (i - start) as usize * narr;
            let dst = cur + narr;
            let match1_offs = self.match1[(i - start) as usize] as i32;
            let n_short_offset;
            let n_short_len;
            let n_literal_score;
            let n_literal_cost;

            if (w[i as usize] != 0 && match1_offs == 0) || (i == start && (block_flags & 1) != 0) {
                n_short_offset = 0;
                n_short_len = 0;
                n_literal_score = 1;
                n_literal_cost = 9;
            } else {
                n_short_offset = if w[i as usize] != 0 { match1_offs } else { 0 };
                n_short_len = 1;
                n_literal_score = if n_short_offset != 0 { 3 } else { 1 };
                n_literal_cost = 4 + TOKEN_SIZE_4BIT_MATCH;
            }

            let mut j = 0usize;
            if self.arrival[dst].from_slot() != 0 {
                while j < narr && self.arrival[cur + j].from_slot() != 0 {
                    let ca = self.arrival[cur + j];
                    let choice_cost = ca.cost + n_literal_cost;
                    let score = ca.score + n_literal_score;
                    let rep_offset = ca.rep_offset();
                    let last = self.arrival[dst + narr - 1];

                    if choice_cost < last.cost || (choice_cost == last.cost && score < last.score && rep_offset != last.rep_offset()) {
                        let a = &mut self.arrival[dst..];
                        if let Some(n) = find_slot(a, choice_cost, score, rep_offset) {
                            if insert_slot(a, narr, n, choice_cost, rep_offset) {
                                a[n].set(choice_cost, i, j as i32 + 1, 1, rep_offset, n_short_offset, ca.rep_pos(), n_short_len, score);
                            }
                        }
                    }
                    j += 1;
                }
            } else {
                while j < narr && self.arrival[cur + j].from_slot() != 0 {
                    let ca = self.arrival[cur + j];
                    self.arrival[dst + j].set(
                        ca.cost + n_literal_cost,
                        i,
                        j as i32 + 1,
                        1,
                        ca.rep_offset(),
                        n_short_offset,
                        ca.rep_pos(),
                        n_short_len,
                        ca.score + n_literal_score,
                    );
                    j += 1;
                }
            }

            if i == start && (block_flags & 1) != 0 {
                continue;
            }

            let mbase = ((i - start) as usize) << MATCHES_PER_INDEX_SHIFT;
            let num_arrivals_for_this_pos = j;
            let mut overall_min_rep_len = 0;
            let mut overall_max_rep_len = 0;
            let mut num_rep = 0usize;

            if (i + 2) <= end {
                let max_rep_len_for_pos = (end - i).min(LCP_MAX);
                let ws = i as usize;
                let wmax = ws + max_rep_len_for_pos as usize;

                for j in 0..num_arrivals_for_this_pos {
                    let ca = self.arrival[cur + j];
                    if ca.follows_literal() == 0 {
                        continue;
                    }
                    let rep_offset = ca.rep_offset();
                    if i >= rep_offset && eq(w, ws, ws - rep_offset as usize, 2) && rep_offset != 0 {
                        let ro = rep_offset as usize;
                        let len0 = self.rle_len[ws - ro];
                        let len1 = self.rle_len[ws];
                        let min_len = len0.min(len1);
                        let mut p = ws + min_len as usize;
                        if p > wmax {
                            p = wmax;
                        }
                        while p + 8 < wmax && eq(w, p, p - ro, 8) {
                            p += 8;
                        }
                        while p + 4 < wmax && eq(w, p, p - ro, 4) {
                            p += 4;
                        }
                        while p < wmax && w[p] == w[p - ro] {
                            p += 1;
                        }
                        let cur_max_len = (p - ws) as i32;

                        rep_idx[num_rep] = j as i32;
                        rep_idx[num_rep + 1] = cur_max_len;
                        num_rep += 2;

                        if overall_max_rep_len < cur_max_len {
                            overall_max_rep_len = cur_max_len;
                        }
                    }
                }
            }
            rep_idx[num_rep] = -1;

            let mut m = 0;
            while m < NMATCHES_PER_INDEX && self.match_[mbase + m].length() != 0 {
                let mm = self.match_[mbase + m];
                let md = self.match_depth[mbase + m];
                let mut orig_match_len = mm.length();
                let orig_match_offset = mm.offset();
                let orig_match_depth = (md & 0x3fff) as u32;
                let score_penalty = 3 + (md >> 15) as i32;

                if (i + orig_match_len) > end {
                    orig_match_len = end - i;
                }

                let mut d: u32 = 0;
                while d <= orig_match_depth {
                    let match_len = orig_match_len.wrapping_sub(d as i32);
                    let match_offset = orig_match_offset.wrapping_sub(d as i32);

                    if insert_forward_reps {
                        self.insert_forward_match(w, i, match_offset, start, end, narr, 0);
                    }

                    if match_len >= 2 {
                        let no_rep_cost_adjustment = if match_len >= LCP_MAX { 1 } else { 0 };
                        let min_match_len_for_offset = if match_offset < MINMATCH3_OFFSET {
                            2
                        } else if match_offset < MINMATCH4_OFFSET {
                            3
                        } else {
                            4
                        };
                        let starting_match_len = if match_len >= LEAVE_ALONE_MATCH_SIZE && i >= match_len { match_len } else { 2 };
                        let jump_match_len = if (block_flags & 3) == 3 && match_len > 90 && i >= 90 { 90 } else { match_len + 1 };

                        let mut cost_for_lit = [0i32; 2];
                        if starting_match_len <= 3 && match_offset < 128 {
                            cost_for_lit = [8 + TOKEN_SIZE_7BIT_MATCH; 2];
                        } else {
                            cost_for_lit[0] = 8 + TOKEN_SIZE_LARGE_MATCH + get_gamma2_size((match_offset >> 8) + 2);
                            cost_for_lit[1] = 8 + TOKEN_SIZE_LARGE_MATCH + get_gamma2_size((match_offset >> 8) + 3);
                        }
                        let cost_delta = cost_for_lit[1] - cost_for_lit[0];

                        let mut k = starting_match_len;
                        while k <= match_len {
                            let rep_match_len_cost = get_gamma2_size(k);
                            let dbase = cur + k as usize * narr;

                            // Insert non-repmatch candidate
                            if k >= min_match_len_for_offset {
                                let no_rep_len_cost = if k <= 3 && match_offset < 128 {
                                    0
                                } else if match_offset < 128 || match_offset >= MINMATCH4_OFFSET {
                                    get_gamma2_size(k - 2)
                                } else if match_offset < MINMATCH3_OFFSET {
                                    rep_match_len_cost
                                } else {
                                    get_gamma2_size(k - 1)
                                };

                                for j in 0..num_arrivals_for_this_pos {
                                    let ca = self.arrival[cur + j];
                                    let follows_literal = ca.follows_literal();
                                    if match_offset != ca.rep_offset() || follows_literal == 0 {
                                        let cmd_cost = no_rep_len_cost + cost_for_lit[follows_literal as usize];
                                        let choice_cost = ca.cost + cmd_cost;
                                        let a = &mut self.arrival[dbase..];

                                        if choice_cost <= a[narr - 1].cost + 1 {
                                            let score = ca.score + score_penalty;

                                            if choice_cost < a[narr - 2].cost
                                                || (choice_cost == a[narr - 2].cost
                                                    && score < a[narr - 2].score
                                                    && (choice_cost != a[narr - 1].cost || match_offset != a[narr - 1].rep_offset()))
                                            {
                                                let mut exists = false;
                                                let mut n = 0;
                                                while a[n].cost < choice_cost {
                                                    if a[n].rep_offset() == match_offset {
                                                        exists = true;
                                                        break;
                                                    }
                                                    n += 1;
                                                }

                                                if !exists {
                                                    let revised_cost = choice_cost - no_rep_cost_adjustment;
                                                    while n < narr - 1 && a[n].cost == revised_cost && score >= a[n].score {
                                                        if a[n].rep_offset() == match_offset {
                                                            exists = true;
                                                            break;
                                                        }
                                                        n += 1;
                                                    }

                                                    if !exists && n < narr - 1 {
                                                        let mut z = n;
                                                        while z < narr - 1 && a[z].cost == choice_cost {
                                                            if a[z].rep_offset() == match_offset {
                                                                exists = true;
                                                                break;
                                                            }
                                                            z += 1;
                                                        }

                                                        if !exists {
                                                            while z < narr - 1 && a[z].from_slot() != 0 {
                                                                if a[z].rep_offset() == match_offset {
                                                                    break;
                                                                }
                                                                z += 1;
                                                            }
                                                            a.copy_within(n..z, n + 1);
                                                            a[n].set(revised_cost, i, j as i32 + 1, 0, match_offset, 0, i, k, score);
                                                        }
                                                    }
                                                } else if (choice_cost - a[n].cost) >= cost_delta {
                                                    break;
                                                }
                                            }
                                            if ca.follows_literal() == 0 || cost_delta == 0 {
                                                break;
                                            }
                                        } else {
                                            break;
                                        }
                                    }
                                }
                            }

                            if k == 3 && match_offset < 128 {
                                cost_for_lit = [8 + TOKEN_SIZE_LARGE_MATCH + 2; 2];
                            }

                            // Insert repmatch candidate
                            if k > overall_min_rep_len && k <= overall_max_rep_len {
                                let rep_cmd_cost = TOKEN_SIZE_LARGE_MATCH + 2 + rep_match_len_cost;

                                if k <= 90 {
                                    overall_min_rep_len = k;
                                } else if overall_max_rep_len == k {
                                    overall_max_rep_len -= 1;
                                }

                                let mut c = 0;
                                loop {
                                    let jj = rep_idx[c];
                                    if jj < 0 {
                                        break;
                                    }
                                    if rep_idx[c + 1] >= k {
                                        let ca = self.arrival[cur + jj as usize];
                                        let rep_choice_cost = ca.cost + rep_cmd_cost;
                                        let score = ca.score + 2;
                                        let rep_offset = ca.rep_offset();
                                        let a = &mut self.arrival[dbase..];
                                        let last = a[narr - 1];

                                        if rep_choice_cost < last.cost
                                            || (rep_choice_cost == last.cost && score < last.score && rep_offset != last.rep_offset())
                                        {
                                            if let Some(n) = find_slot(a, rep_choice_cost, score, rep_offset) {
                                                if insert_slot(a, narr, n, rep_choice_cost, rep_offset) {
                                                    a[n].set(rep_choice_cost, i, jj + 1, 0, rep_offset, 0, i, k, score);
                                                }
                                            }
                                        } else {
                                            break;
                                        }
                                    }
                                    c += 2;
                                }
                            }

                            if k == jump_match_len {
                                k = match_len - 1;
                            }
                            k += 1;
                        }
                    }

                    // Note: in the C this `break` is inside the depth loop (not the match loop).
                    if orig_match_len >= 512 {
                        break;
                    }
                    d += if orig_match_depth != 0 { orig_match_depth } else { 1 };
                }
                m += 1;
            }
        }

        if !insert_forward_reps {
            let mut e = (end - start) as usize * narr;
            loop {
                let ea = self.arrival[e];
                if !(ea.from_slot() > 0 && ea.from_pos() < end as u32) {
                    break;
                }
                let fp = ea.from_pos() as i32;
                let bm = &mut self.best_match[(fp - start) as usize];
                bm.length = ea.match_len();
                bm.offset = if ea.match_len() >= 2 { ea.rep_offset() } else { ea.short_offset() };
                e = (fp - start) as usize * narr + (ea.from_slot() - 1) as usize;
            }
        }
    }

    /// `apultra_reduce_commands` — replace matches by literals when it makes the bitstream
    /// smaller, and merge large matches. `bm` is indexed by window position (`pBestMatch`).
    fn reduce_commands(&mut self, w: &[u8], start: i32, end: i32, cur_rep_match_offset: i32, block_flags: i32) -> bool {
        let mut rep_match_offset = cur_rep_match_offset;
        let mut follows_literal = 0;
        let mut did_reduce = false;
        let mut last_match_len = 0;
        let s = start as usize;
        let match1 = |p: i32| self.match1[p as usize - s] as i32;
        let bm = &mut self.best_match;
        macro_rules! b {
            ($p:expr) => {
                bm[($p) as usize - s]
            };
        }

        let mut i = start + (block_flags & 1);
        while i < end {
            let cur = b!(i);
            let next = if i + 1 < end { b!(i + 1) } else { FinalMatch::default() };
            if cur.length <= 1
                && (i + 1) < end
                && next.length >= 2
                && next.length < MAX_VARLEN
                && next.offset != 0
                && i >= next.offset
                && (i + next.length + 1) <= end
                && eq(w, (i - next.offset) as usize, i as usize, (next.length + 1) as usize)
                && (next.offset < MINMATCH4_OFFSET || (next.length + 1) >= 4 || (next.offset == rep_match_offset && follows_literal != 0))
            {
                let mut cur_partial = if cur.length == 1 { TOKEN_SIZE_4BIT_MATCH + 4 } else { 1 + 8 };
                if next.offset == rep_match_offset {
                    cur_partial += TOKEN_SIZE_LARGE_MATCH + 2 + get_gamma2_size(next.length);
                } else {
                    cur_partial += get_offset_varlen_size(next.length, next.offset, 1) + get_match_varlen_size(next.length, next.offset);
                }

                let reduced_partial = if next.offset == rep_match_offset && follows_literal != 0 {
                    TOKEN_SIZE_LARGE_MATCH + 2 + get_gamma2_size(next.length + 1)
                } else {
                    get_offset_varlen_size(next.length + 1, next.offset, follows_literal) + get_match_varlen_size(next.length + 1, next.offset)
                };

                if reduced_partial < cur_partial || (follows_literal == 0 && last_match_len >= LCP_MAX) {
                    // Merge
                    b!(i).length = next.length + 1;
                    b!(i).offset = next.offset;
                    b!(i + 1).length = 0;
                    b!(i + 1).offset = 0;
                    did_reduce = true;
                    continue;
                }
            }

            let pm = b!(i);
            if pm.length >= 2 {
                if pm.length < LCP_MAX {
                    let mut next_index = i + pm.length;
                    let mut next_follows_literal = 0;

                    while next_index < end && b!(next_index).length < 2 {
                        next_index += 1;
                        next_follows_literal = 1;
                    }

                    if next_index < end && b!(next_index).length >= 2 {
                        let mut cannot_encode = false;
                        let nm = b!(next_index);

                        if rep_match_offset != 0
                            && rep_match_offset != pm.offset
                            && nm.offset != 0
                            && pm.offset != nm.offset
                            && next_follows_literal != 0
                            && i >= nm.offset
                            && (i + pm.length) <= end
                            && (nm.offset < MINMATCH3_OFFSET || pm.length >= 3)
                            && (nm.offset < MINMATCH4_OFFSET || pm.length >= 4)
                        {
                            // Try to gain a match forward
                            let base = i as usize;
                            let no = nm.offset as usize;
                            let mut max_len: i32 = 0;
                            while (max_len + 8) < pm.length && eq(w, base + max_len as usize - no, base + max_len as usize, 8) {
                                max_len += 8;
                            }
                            while (max_len + 4) < pm.length && eq(w, base + max_len as usize - no, base + max_len as usize, 4) {
                                max_len += 4;
                            }
                            while max_len < pm.length && w[base + max_len as usize - no] == w[base + max_len as usize] {
                                max_len += 1;
                            }

                            if max_len >= pm.length {
                                // Replace
                                b!(i).offset = nm.offset;
                                did_reduce = true;
                            } else if max_len >= 2
                                && ((follows_literal != 0 && rep_match_offset == nm.offset)
                                    || ((nm.offset < MINMATCH3_OFFSET || max_len >= 3) && (nm.offset < MINMATCH4_OFFSET || max_len >= 4)))
                            {
                                let mut before = get_offset_varlen_size(pm.length, pm.offset, follows_literal);
                                before += get_match_varlen_size(pm.length, pm.offset);
                                before += get_offset_varlen_size(nm.length, nm.offset, 1);
                                before += get_match_varlen_size(nm.length, nm.offset);

                                let mut after = get_offset_varlen_size(max_len, nm.offset, follows_literal);
                                if follows_literal != 0 && rep_match_offset == nm.offset {
                                    after += get_gamma2_size(max_len);
                                } else {
                                    after += get_match_varlen_size(max_len, nm.offset);
                                }
                                after += TOKEN_SIZE_LARGE_MATCH + 2;
                                after += get_gamma2_size(nm.length);

                                for j in max_len..pm.length {
                                    if w[(i + j) as usize] == 0 || match1(i + j) != 0 {
                                        after += TOKEN_SIZE_4BIT_MATCH + 4;
                                    } else {
                                        after += 1 + 8;
                                    }
                                }

                                if after < before {
                                    let orig_len = pm.length;
                                    b!(i).offset = nm.offset;
                                    b!(i).length = max_len;
                                    for j in max_len..orig_len {
                                        let m1 = match1(i + j);
                                        b!(i + j).offset = m1;
                                        b!(i + j).length = if w[(i + j) as usize] != 0 && m1 == 0 { 0 } else { 1 };
                                    }
                                    did_reduce = true;
                                    continue;
                                }
                            }
                        }

                        // Note: pMatch may have been updated by "Replace" above.
                        let pm = b!(i);
                        let nm = b!(next_index);

                        // Calculate this command's current cost
                        let cur_cmd = if pm.offset == rep_match_offset && follows_literal != 0 {
                            TOKEN_SIZE_LARGE_MATCH + 2 + get_gamma2_size(pm.length)
                        } else {
                            get_offset_varlen_size(pm.length, pm.offset, follows_literal) + get_match_varlen_size(pm.length, pm.offset)
                        };

                        // Calculate the next command's current cost
                        let next_cmd = if nm.offset == pm.offset && next_follows_literal != 0 && nm.length >= 2 {
                            TOKEN_SIZE_LARGE_MATCH + 2 + get_gamma2_size(nm.length)
                        } else {
                            get_offset_varlen_size(nm.length, nm.offset, next_follows_literal) + get_match_varlen_size(nm.length, nm.offset)
                        };

                        let original_combined = cur_cmd + next_cmd;

                        // Cost of replacing this match command by literals + effect on the next
                        let mut reduced = 0;
                        for j in 0..pm.length {
                            if w[(i + j) as usize] == 0 || match1(i + j) != 0 {
                                reduced += TOKEN_SIZE_4BIT_MATCH + 4;
                            } else {
                                reduced += 1 + 8;
                            }
                        }

                        if nm.offset == rep_match_offset && nm.length >= 2 {
                            reduced += TOKEN_SIZE_LARGE_MATCH + 2 + get_gamma2_size(nm.length);
                        } else if (nm.length < 3 && nm.offset >= MINMATCH3_OFFSET) || (nm.length < 4 && nm.offset >= MINMATCH4_OFFSET) {
                            // This match length can only be encoded with a rep-match
                            cannot_encode = true;
                        } else {
                            reduced += get_offset_varlen_size(nm.length, nm.offset, 1) + get_match_varlen_size(nm.length, nm.offset);
                        }

                        if original_combined > reduced && !cannot_encode {
                            // Reduce
                            let ml = pm.length;
                            for j in 0..ml {
                                let m1 = match1(i + j);
                                b!(i + j).offset = m1;
                                b!(i + j).length = if w[(i + j) as usize] != 0 && m1 == 0 { 0 } else { 1 };
                            }
                            did_reduce = true;
                            continue;
                        }
                    }
                }

                let pm = b!(i);
                let il = i + pm.length;
                if il < end && pm.offset > 0 {
                    let nx = b!(il);
                    if nx.offset > 0
                        && nx.length >= 2
                        && (pm.length + nx.length) <= MAX_VARLEN
                        && il >= pm.offset
                        && il >= nx.offset
                        && (il + nx.length) <= end
                        && eq(w, (il - pm.offset) as usize, (il - nx.offset) as usize, nx.length as usize)
                    {
                        // Join large matches
                        let match_len = pm.length;
                        let mut next_index = i + match_len + nx.length;
                        let mut next_follows_literal = 0;
                        let mut cannot_encode = false;

                        while next_index < end && b!(next_index).length < 2 {
                            next_index += 1;
                            next_follows_literal = 1;
                        }

                        let mut cur_cmd = if pm.offset == rep_match_offset && follows_literal != 0 {
                            TOKEN_SIZE_LARGE_MATCH + 2 + get_gamma2_size(match_len)
                        } else {
                            get_offset_varlen_size(match_len, pm.offset, follows_literal) + get_match_varlen_size(match_len, pm.offset)
                        };

                        cur_cmd += get_offset_varlen_size(nx.length, nx.offset, 0) + get_match_varlen_size(nx.length, nx.offset);

                        let has_next = next_index < end && b!(next_index).length >= 2;
                        let nn = if next_index < end { b!(next_index) } else { FinalMatch::default() };
                        if has_next {
                            if nn.offset == nx.offset && next_follows_literal != 0 {
                                cur_cmd += TOKEN_SIZE_LARGE_MATCH + 2 + get_gamma2_size(nn.length);
                            } else {
                                cur_cmd += get_offset_varlen_size(nn.length, nn.offset, next_follows_literal) + get_match_varlen_size(nn.length, nn.offset);
                            }
                        }

                        let mut reduced_cmd = if pm.offset == rep_match_offset && follows_literal != 0 {
                            TOKEN_SIZE_LARGE_MATCH + 2 + get_gamma2_size(match_len + nx.length)
                        } else {
                            get_offset_varlen_size(match_len + nx.length, pm.offset, follows_literal) + get_match_varlen_size(match_len + nx.length, pm.offset)
                        };

                        if has_next {
                            if nn.offset == pm.offset && next_follows_literal != 0 {
                                reduced_cmd += TOKEN_SIZE_LARGE_MATCH + 2 + get_gamma2_size(nn.length);
                            } else {
                                reduced_cmd += get_offset_varlen_size(nn.length, nn.offset, next_follows_literal) + get_match_varlen_size(nn.length, nn.offset);
                                if (nn.offset >= MINMATCH3_OFFSET && nn.length < 3) || (nn.offset >= MINMATCH4_OFFSET && nn.length < 4) {
                                    cannot_encode = true;
                                }
                            }
                        }

                        if cur_cmd >= reduced_cmd && !cannot_encode {
                            b!(i).length += nx.length;
                            b!(i + match_len).length = 0;
                            b!(i + match_len).offset = 0;
                            did_reduce = true;
                            continue;
                        }
                    }
                }

                let pm = b!(i);
                rep_match_offset = pm.offset;
                follows_literal = 0;
                last_match_len = pm.length;
                i += pm.length;
            } else {
                // 4 bits offset (1 byte match) or literal
                i += 1;
                follows_literal = 1;
                last_match_len = 0;
            }
        }

        did_reduce
    }

    /// `apultra_write_block` — emit a block of compressed data. Returns its size or -1.
    fn write_block(
        &mut self,
        w: &[u8],
        start: i32,
        end: i32,
        bw: &mut BitWriter,
        follows_lit: &mut i32,
        cur_rep: &mut i32,
        block_flags: i32,
        prior_delta: i32,
    ) -> i32 {
        let mut rep_match_offset = *cur_rep;
        let mut cur_follows_literal = *follows_lit;
        let mut out: i32 = 0;
        let max_offset = self.max_offset;
        let s = start as usize;
        let st = &mut self.stats;

        if block_flags & 1 != 0 {
            if bw.max <= 0 {
                return -1;
            }
            out = bw.put_byte(out, w[s]);
            cur_follows_literal = 1;
        }

        let mut i = start + (block_flags & 1);
        while i < end {
            let pm = self.best_match[i as usize - s];

            if pm.length >= 2 {
                let match_len = pm.length;
                let match_offset = pm.offset;

                if !(MIN_OFFSET..=max_offset).contains(&match_offset) {
                    return -1;
                }

                if match_offset == rep_match_offset && cur_follows_literal != 0 {
                    // Rep-match
                    out = bw.write_bits(out, TOKEN_CODE_LARGE_MATCH, TOKEN_SIZE_LARGE_MATCH);
                    out = bw.write_bits(out, 0, 2);
                    out = bw.write_gamma2_value(out, match_len);
                    if out < 0 {
                        return -1;
                    }
                    cur_follows_literal = 0;
                    st.num_rep_matches += 1;
                } else if match_len <= 3 && match_offset < 128 {
                    // 7 bits offset + 1 bit length
                    out = bw.write_bits(out, TOKEN_CODE_7BIT_MATCH, TOKEN_SIZE_7BIT_MATCH);
                    out = bw.put_byte(out, (((match_offset & 0x7f) << 1) | (match_len - 2)) as u8);
                    if out < 0 {
                        return -1;
                    }
                    cur_follows_literal = 0;
                    rep_match_offset = match_offset;
                    st.num_7bit_matches += 1;
                } else {
                    // 8+n bits offset
                    out = bw.write_bits(out, TOKEN_CODE_LARGE_MATCH, TOKEN_SIZE_LARGE_MATCH);
                    out = bw.write_gamma2_value(out, (match_offset >> 8) + 2 + (cur_follows_literal & 1));
                    out = bw.put_byte(out, (match_offset & 0xff) as u8);
                    if out < 0 {
                        return -1;
                    }
                    // The match length isn't encoded in the command, emit elias gamma value
                    out = if match_offset < 128 || match_offset >= MINMATCH4_OFFSET {
                        bw.write_gamma2_value(out, match_len - 2)
                    } else if match_offset < MINMATCH3_OFFSET {
                        bw.write_gamma2_value(out, match_len)
                    } else {
                        bw.write_gamma2_value(out, match_len - 1)
                    };
                    if out < 0 {
                        return -1;
                    }
                    cur_follows_literal = 0;
                    rep_match_offset = match_offset;
                    st.num_variable_matches += 1;
                }

                if match_offset < st.min_offset || st.min_offset == -1 {
                    st.min_offset = match_offset;
                }
                if match_offset > st.max_offset {
                    st.max_offset = match_offset;
                }
                st.total_offsets += match_offset as i64;

                if match_len < st.min_match_len || st.min_match_len == -1 {
                    st.min_match_len = match_len;
                }
                if match_len > st.max_match_len {
                    st.max_match_len = match_len;
                }
                st.total_match_lens = st.total_match_lens.wrapping_add(match_len);
                st.match_divisor += 1;

                if match_offset == 1 {
                    if match_len < st.min_rle1_len || st.min_rle1_len == -1 {
                        st.min_rle1_len = match_len;
                    }
                    if match_len > st.max_rle1_len {
                        st.max_rle1_len = match_len;
                    }
                    st.total_rle1_lens = st.total_rle1_lens.wrapping_add(match_len);
                    st.rle1_divisor += 1;
                } else if match_offset == 2 {
                    if match_len < st.min_rle2_len || st.min_rle2_len == -1 {
                        st.min_rle2_len = match_len;
                    }
                    if match_len > st.max_rle2_len {
                        st.max_rle2_len = match_len;
                    }
                    st.total_rle2_lens = st.total_rle2_lens.wrapping_add(match_len);
                    st.rle2_divisor += 1;
                }

                i += match_len;
                st.commands_divisor += 1;
            } else if pm.length == 1 {
                let match_offset = pm.offset;
                // 4 bits offset
                if !(0..=15).contains(&match_offset) {
                    return -1;
                }
                out = bw.write_bits(out, TOKEN_CODE_4BIT_MATCH, TOKEN_SIZE_4BIT_MATCH);
                out = bw.write_bits(out, match_offset, 4);
                if out < 0 {
                    return -1;
                }
                st.num_4bit_matches += 1;
                st.commands_divisor += 1;
                i += 1;
                cur_follows_literal = 1;
            } else {
                // Literal
                out = bw.write_bits(out, 0, 1);
                out = bw.put_byte(out, w[i as usize]);
                if out < 0 {
                    return -1;
                }
                st.num_literals += 1;
                st.commands_divisor += 1;
                i += 1;
                cur_follows_literal = 1;
            }

            let cur_safe_dist = prior_delta + (i - start) - out;
            if cur_safe_dist >= 0 && st.safe_dist < cur_safe_dist {
                st.safe_dist = cur_safe_dist;
            }
        }

        if block_flags & 2 != 0 {
            // 8 bits offset: EOD
            out = bw.write_bits(out, TOKEN_CODE_7BIT_MATCH, TOKEN_SIZE_7BIT_MATCH);
            out = bw.put_byte(out, 0);
            if out < 0 {
                return -1;
            }
            st.num_eod += 1;
            st.commands_divisor += 1;

            let cur_safe_dist = prior_delta + (i - start) - out;
            if cur_safe_dist >= 0 && st.safe_dist < cur_safe_dist {
                st.safe_dist = cur_safe_dist;
            }
        }

        *cur_rep = rep_match_offset;
        *follows_lit = cur_follows_literal;
        out
    }

    /// `apultra_optimize_and_write_block`
    fn optimize_and_write_block(
        &mut self,
        w: &[u8],
        prev_block_size: i32,
        in_data_size: i32,
        bw: &mut BitWriter,
        follows_lit: &mut i32,
        cur_rep: &mut i32,
        block_flags: i32,
        prior_delta: i32,
    ) -> i32 {
        let end = prev_block_size + in_data_size;
        let narr = self.max_arrivals;
        let start = prev_block_size;

        self.best_match.fill(FinalMatch::default());

        // rle_len
        let mut i = 0usize;
        let e = end as usize;
        while i < e {
            let mut range_start = i;
            let c = w[range_start];
            loop {
                i += 1;
                if !(i < e && w[i] == c) {
                    break;
                }
            }
            while range_start < i {
                self.rle_len[range_start] = (i - range_start) as i32;
                range_start += 1;
            }
        }

        if (block_flags & 3) == 3 {
            // Supplement 2 and 3-byte matches
            self.first_offset_for_byte.fill(-1);
            self.next_offset_for_pos[..in_data_size as usize].fill(-1);

            for pos in start..end - 1 {
                let key = w[pos as usize] as usize | ((w[pos as usize + 1] as usize) << 8);
                self.next_offset_for_pos[(pos - start) as usize] = self.first_offset_for_byte[key];
                self.first_offset_for_byte[key] = pos;
            }

            for pos in start + 1..end - 1 {
                let mbase = ((pos - start) as usize) << MATCHES_PER_INDEX_SHIFT;
                let mut m = 0usize;
                let mut inserted = 0;

                while m < 15 && self.match_[mbase + m].length() != 0 {
                    m += 1;
                }

                let mut match_pos = self.next_offset_for_pos[(pos - start) as usize];
                while m < 15 && match_pos >= 0 {
                    let match_offset = pos - match_pos;
                    if match_offset > self.max_offset {
                        break;
                    }
                    let mut already = false;
                    for x in 0..m {
                        let em = self.match_[mbase + x];
                        if em.offset() == match_offset || (em.offset() - (self.match_depth[mbase + x] & 0x3fff) as i32) == match_offset {
                            already = true;
                            break;
                        }
                    }
                    if !already {
                        let len = if pos < (end - 2) && w[match_pos as usize + 2] == w[pos as usize + 2] { 3 } else { 2 };
                        self.match_[mbase + m].set_length(len);
                        self.match_[mbase + m].set_offset(match_offset);
                        self.match_depth[mbase + m] = 0x4000;
                        m += 1;
                        inserted += 1;
                        if inserted >= 6 {
                            break;
                        }
                    }
                    match_pos = self.next_offset_for_pos[(match_pos - start) as usize];
                }
            }
        }

        self.optimize_forward(w, start, end, true, *cur_rep, block_flags, narr);

        if (block_flags & 3) == 3 && narr == NARRIVALS_PER_POSITION_MAX {
            // Supplement matches further
            self.offset_cache.fill(-1);

            for pos in start + 1..end - 1 {
                let mbase = ((pos - start) as usize) << MATCHES_PER_INDEX_SHIFT;
                if self.match_[mbase].length() >= 8 {
                    continue;
                }
                let mut m = 0usize;
                let mut inserted = 0;
                let max_forward_pos = (pos + 2 + 1 + 5).min(end - 2);

                while m < 46 && self.match_[mbase + m].length() != 0 {
                    let mm = self.match_[mbase + m];
                    self.offset_cache[(mm.offset() & 2047) as usize] = pos;
                    self.offset_cache[((mm.offset() - (self.match_depth[mbase + m] & 0x3fff) as i32) & 2047) as usize] = pos;
                    m += 1;
                }

                let mut match_pos = self.next_offset_for_pos[(pos - start) as usize];
                while m < 46 && match_pos >= 0 {
                    let match_offset = pos - match_pos;
                    if match_offset > self.max_offset {
                        break;
                    }
                    let mut already = false;

                    if self.offset_cache[(match_offset & 2047) as usize] == pos {
                        for x in 0..m {
                            let em = self.match_[mbase + x];
                            let ed = self.match_depth[mbase + x];
                            if em.offset() == match_offset || (em.offset() - (ed & 0x3fff) as i32) == match_offset {
                                already = true;
                                if ed == 0x4000 {
                                    let ml = extend16(w, match_pos, pos, end);
                                    if ml > em.length() {
                                        self.match_[mbase + x].set_length(ml);
                                    }
                                }
                                break;
                            }
                        }
                    }

                    if !already {
                        let mut forward_pos = pos + 2 + 1;
                        if forward_pos >= match_offset {
                            let mut got_match = false;
                            while forward_pos < max_forward_pos {
                                if eq(w, forward_pos as usize, (forward_pos - match_offset) as usize, 2) {
                                    got_match = true;
                                    break;
                                }
                                forward_pos += 1;
                            }

                            if got_match {
                                let ml = extend16(w, match_pos, pos, end);
                                self.match_[mbase + m].set_length(ml);
                                self.match_[mbase + m].set_offset(match_offset);
                                self.match_depth[mbase + m] = 0;
                                m += 1;

                                self.insert_forward_match(w, pos, match_offset, start, end, narr, 8);

                                inserted += 1;
                                if inserted >= 18 || (inserted >= 15 && m >= 38) {
                                    break;
                                }
                            }
                        }
                    }
                    match_pos = self.next_offset_for_pos[(match_pos - start) as usize];
                }
            }
        }

        // Pick optimal matches
        self.optimize_forward(w, start, end, false, *cur_rep, block_flags, narr);

        // Apply reduction and merge pass
        let mut passes = 0;
        loop {
            let did_reduce = self.reduce_commands(w, start, end, *cur_rep, block_flags);
            passes += 1;
            if !(did_reduce && passes < 20) {
                break;
            }
        }

        // Write compressed block
        self.write_block(w, start, end, bw, follows_lit, cur_rep, block_flags, prior_delta)
    }
}

/// Match length extension (up to 16) used by the "supplement matches further" pass: the
/// 8/4/1-byte `memcmp` ladder of the C.
fn extend16(w: &[u8], match_pos: i32, pos: i32, end: i32) -> i32 {
    let mut ml: i32 = 2;
    while (ml + 8) < 16 && (pos + ml + 8) < end && eq(w, (match_pos + ml) as usize, (pos + ml) as usize, 8) {
        ml += 8;
    }
    while (ml + 4) < 16 && (pos + ml + 4) < end && eq(w, (match_pos + ml) as usize, (pos + ml) as usize, 4) {
        ml += 4;
    }
    while ml < 16 && (pos + ml) < end && w[(match_pos + ml) as usize] == w[(pos + ml) as usize] {
        ml += 1;
    }
    ml
}

/// The slot search shared by the literal and rep-match insertions in `apultra_optimize_forward`
/// (the three `exists` loops). Returns the insertion slot `n`, or `None` if an arrival with the
/// same rep offset already exists. The loops are unbounded in the C (they rely on the caller's
/// pre-check against the last slot); `a` extends to the end of the arrival buffer as in the C.
#[inline]
fn find_slot(a: &[Arrival], cost: i32, score: i32, rep_offset: i32) -> Option<usize> {
    let mut n = 0;
    while a[n].cost < cost {
        if a[n].rep_offset() == rep_offset {
            return None;
        }
        n += 1;
    }
    while a[n].cost == cost && score >= a[n].score {
        if a[n].rep_offset() == rep_offset {
            return None;
        }
        n += 1;
    }
    Some(n)
}

/// The `z` scan + `memmove` that opens slot `n` (shared by the literal and rep-match
/// insertions). Returns `false` (and does nothing) if an equal-cost arrival with the same rep
/// offset exists at or after `n`.
#[inline]
fn insert_slot(a: &mut [Arrival], narr: usize, n: usize, cost: i32, rep_offset: i32) -> bool {
    let mut z = n;
    while z < narr - 1 && a[z].cost == cost {
        if a[z].rep_offset() == rep_offset {
            return false;
        }
        z += 1;
    }
    while z < narr - 1 && a[z].from_slot() != 0 {
        if a[z].rep_offset() == rep_offset {
            break;
        }
        z += 1;
    }
    a.copy_within(n..z, n + 1);
    true
}

/// `apultra_get_max_compressed_size` — worst-case compressed size.
pub fn max_compressed_size(input_size: usize) -> usize {
    ((input_size * 9 + 1 + 2 + 8) + 7) >> 3
}

/// `apultra_compress(input, out, len, max_compressed_size(len), 0, max_window, 0, NULL, &stats)`.
///
/// `max_window` is the maximum match offset (`nMaxWindowSize`; 0 selects the format maximum).
/// Returns the compressed stream and the statistics (notably `safe_dist`).
///
/// # Panics
/// If the compressor reports an error (cannot happen for the worst-case output size used here).
pub fn compress(input: &[u8], max_window: usize) -> (Vec<u8>, Stats) {
    let input_size = input.len();
    let max_out_buffer_size = max_compressed_size(input_size);
    let mut out = vec![0u8; max_out_buffer_size];

    // `(nInputSize < BLOCK_SIZE) ? ((nInputSize < 1024) ? 1024 : nInputSize) : BLOCK_SIZE`.
    // Note: the literal if/else form of this was miscompiled by rustc 1.98.1 in release builds
    // (it yielded `input_size` for inputs > BLOCK_SIZE); `clamp` is equivalent and compiles right.
    let block_size: usize = input_size.clamp(1024, BLOCK_SIZE);
    let max_out_block_size = max_compressed_size(block_size) as i32;

    let mut max_arrivals = NARRIVALS_PER_POSITION_SMALL;
    {
        let in_data_size = input_size.min(block_size);
        if in_data_size > 0 && in_data_size >= input_size {
            max_arrivals = if input_size <= 262144 { NARRIVALS_PER_POSITION_MAX } else { NARRIVALS_PER_POSITION_NORMAL };
        }
    }

    let mut c = Compressor::new(block_size, block_size * 2, max_arrivals);
    c.max_offset = if max_window != 0 { max_window as i32 } else { MAX_OFFSET };

    let mut original_size: usize = 0;
    let mut compressed_size: usize = 0;
    let mut prev_block_size: usize = 0;
    let mut bits_offset: i32 = 0;
    let mut bit_shift: i32 = -1;
    let mut follows_literal: i32 = 0;
    let mut block_flags: i32 = 1;
    let mut cur_rep: i32 = 0;

    while original_size < input_size {
        let in_data_size = (input_size - original_size).min(block_size);

        let out_data_end = ((max_out_buffer_size - compressed_size) as i32).min(max_out_block_size);
        if original_size + in_data_size >= input_size {
            block_flags |= 2;
        }
        // Prior cumulative (decoder output − decoder input) at the start of this block.
        let prior_delta = (original_size as i64 - compressed_size as i64) as i32;

        let window = &input[original_size - prev_block_size..original_size + in_data_size];
        let mut bw = BitWriter { out: &mut out, base: compressed_size, max: out_data_end, bits_offset, bit_shift };

        // apultra_compressor_shrink_block
        c.build_suffix_array(window);
        if prev_block_size != 0 {
            c.skip_matches(0, prev_block_size as i32);
        }
        c.find_all_matches(NMATCHES_PER_INDEX, prev_block_size as i32, (prev_block_size + in_data_size) as i32, block_flags);
        let out_data_size = c.optimize_and_write_block(
            window,
            prev_block_size as i32,
            in_data_size as i32,
            &mut bw,
            &mut follows_literal,
            &mut cur_rep,
            block_flags,
            prior_delta,
        );
        bits_offset = bw.bits_offset;
        bit_shift = bw.bit_shift;
        block_flags &= !1;

        assert!(out_data_size >= 0, "apultra: block compression failed");
        original_size += in_data_size;
        compressed_size += out_data_size as usize;
        if bit_shift != -1 {
            bits_offset -= out_data_size;
        }
        prev_block_size = in_data_size;
    }

    out.truncate(compressed_size);
    (out, c.stats)
}

// ---------------------------------------------------------------------------------------------
// Suffix array (SA-IS). Replaces libdivsufsort; the suffix array of a string is unique.
// ---------------------------------------------------------------------------------------------

/// Suffix array of `s` in standard order (unsigned byte comparison; a suffix that is a prefix of
/// another sorts first).
pub fn suffix_array(s: &[u8]) -> Vec<u32> {
    let n = s.len();
    if n == 0 {
        return Vec::new();
    }
    // Shift the alphabet by one and append a unique smallest sentinel.
    let mut t: Vec<u32> = Vec::with_capacity(n + 1);
    t.extend(s.iter().map(|&b| b as u32 + 1));
    t.push(0);
    let mut sa = vec![0u32; n + 1];
    sais(&t, 257, &mut sa);
    sa.remove(0);
    sa
}

const EMPTY: u32 = u32::MAX;

fn get_buckets(s: &[u32], k: usize, end: bool) -> Vec<u32> {
    let mut b = vec![0u32; k];
    for &c in s {
        b[c as usize] += 1;
    }
    let mut sum = 0;
    for x in b.iter_mut() {
        sum += *x;
        *x = if end { sum } else { sum - *x };
    }
    b
}

fn induce(s: &[u32], k: usize, sa: &mut [u32], stype: &[bool]) {
    let n = s.len();
    let mut bkt = get_buckets(s, k, false);
    for i in 0..n {
        let j = sa[i];
        if j != EMPTY && j > 0 && !stype[j as usize - 1] {
            let c = s[j as usize - 1] as usize;
            sa[bkt[c] as usize] = j - 1;
            bkt[c] += 1;
        }
    }
    let mut bkt = get_buckets(s, k, true);
    for i in (0..n).rev() {
        let j = sa[i];
        if j != EMPTY && j > 0 && stype[j as usize - 1] {
            let c = s[j as usize - 1] as usize;
            bkt[c] -= 1;
            sa[bkt[c] as usize] = j - 1;
        }
    }
}

/// SA-IS (Nong, Zhang & Chan). `s` must end with a unique smallest symbol (0).
fn sais(s: &[u32], k: usize, sa: &mut [u32]) {
    let n = s.len();
    if n == 1 {
        sa[0] = 0;
        return;
    }
    // S/L types
    let mut stype = vec![false; n];
    stype[n - 1] = true;
    for i in (0..n - 1).rev() {
        stype[i] = s[i] < s[i + 1] || (s[i] == s[i + 1] && stype[i + 1]);
    }
    let is_lms = |i: usize| i > 0 && stype[i] && !stype[i - 1];

    // Stage 1: sort LMS substrings.
    sa.fill(EMPTY);
    let mut bkt = get_buckets(s, k, true);
    for i in (1..n).rev() {
        if is_lms(i) {
            let c = s[i] as usize;
            bkt[c] -= 1;
            sa[bkt[c] as usize] = i as u32;
        }
    }
    induce(s, k, sa, &stype);

    // Compact sorted LMS positions into the front of sa.
    let mut n1 = 0;
    for i in 0..n {
        if is_lms(sa[i] as usize) {
            sa[n1] = sa[i];
            n1 += 1;
        }
    }

    // Name LMS substrings.
    for x in sa[n1..].iter_mut() {
        *x = EMPTY;
    }
    let mut name: u32 = 0;
    let mut prev: Option<usize> = None;
    for i in 0..n1 {
        let pos = sa[i] as usize;
        let mut diff = false;
        match prev {
            None => diff = true,
            Some(p) => {
                for d in 0..n {
                    if pos + d >= n || p + d >= n || s[pos + d] != s[p + d] || stype[pos + d] != stype[p + d] {
                        diff = true;
                        break;
                    } else if d > 0 && (is_lms(pos + d) || is_lms(p + d)) {
                        break;
                    }
                }
            }
        }
        if diff {
            name += 1;
            prev = Some(pos);
        }
        sa[n1 + pos / 2] = name - 1;
    }
    let mut j = n;
    for i in (n1..n).rev() {
        if sa[i] != EMPTY {
            j -= 1;
            sa[j] = sa[i];
        }
    }

    // Stage 2: solve the reduced problem.
    let s1: Vec<u32> = sa[n - n1..].to_vec();
    let mut sa1 = vec![0u32; n1];
    if (name as usize) < n1 {
        sais(&s1, name as usize, &mut sa1);
    } else {
        for i in 0..n1 {
            sa1[s1[i] as usize] = i as u32;
        }
    }

    // Stage 3: induce the final SA from the sorted LMS suffixes.
    let mut p1 = Vec::with_capacity(n1);
    for i in 1..n {
        if is_lms(i) {
            p1.push(i as u32);
        }
    }
    for x in sa1.iter_mut() {
        *x = p1[*x as usize];
    }
    sa.fill(EMPTY);
    let mut bkt = get_buckets(s, k, true);
    for i in (0..n1).rev() {
        let j = sa1[i] as usize;
        let c = s[j] as usize;
        bkt[c] -= 1;
        sa[bkt[c] as usize] = j as u32;
    }
    induce(s, k, sa, &stype);
}

// ---------------------------------------------------------------------------------------------
// Decoder (used to verify round trips).
// ---------------------------------------------------------------------------------------------

/// Decodes an aPLib stream as produced by [`compress`] (libdragon `aplib_dec.c` semantics).
/// Returns `None` on malformed input.
pub fn decompress(src: &[u8]) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    if src.is_empty() {
        return Some(out);
    }
    struct R<'a> {
        s: &'a [u8],
        p: usize,
        cc: u8,
        shift: i32,
    }
    impl R<'_> {
        fn byte(&mut self) -> Option<u8> {
            let b = *self.s.get(self.p)?;
            self.p += 1;
            Some(b)
        }
        fn bit(&mut self) -> Option<u32> {
            if self.shift < 0 {
                self.shift = 7;
                self.cc = self.byte()?;
            }
            let b = (self.cc >> self.shift) & 1;
            self.shift -= 1;
            Some(b as u32)
        }
        fn gamma2(&mut self) -> Option<u32> {
            let mut v: u32 = 1;
            loop {
                v = (v << 1) | self.bit()?;
                if self.bit()? == 0 {
                    break;
                }
                if v > 1 << 30 {
                    return None;
                }
            }
            Some(v)
        }
    }
    let mut r = R { s: src, p: 0, cc: 0, shift: -1 };
    out.push(r.byte()?);
    let mut nlit: u32 = 3;
    let mut match_off: usize = 0;
    loop {
        let len: usize;
        if r.bit()? == 0 {
            out.push(r.byte()?);
            nlit = 3;
            continue;
        }
        if r.bit()? == 0 {
            let g = r.gamma2()?;
            if g >= nlit {
                let off_hi = (g - nlit) as usize;
                match_off = (off_hi << 8) | r.byte()? as usize;
                let mut l = r.gamma2()? as usize;
                if match_off < 128 || match_off >= MINMATCH4_OFFSET as usize {
                    l += 2;
                } else if match_off >= MINMATCH3_OFFSET as usize {
                    l += 1;
                }
                len = l;
            } else {
                len = r.gamma2()? as usize;
            }
        } else if r.bit()? == 0 {
            let cmd = r.byte()?;
            if cmd == 0 {
                return Some(out);
            }
            match_off = (cmd >> 1) as usize;
            len = (cmd & 1) as usize + 2;
        } else {
            let mut o = 0usize;
            for _ in 0..4 {
                o = (o << 1) | r.bit()? as usize;
            }
            if o == 0 {
                out.push(0);
            } else {
                let b = *out.get(out.len().checked_sub(o)?)?;
                out.push(b);
            }
            nlit = 3;
            continue;
        }
        if match_off == 0 || match_off > out.len() {
            return None;
        }
        for _ in 0..len {
            let b = out[out.len() - match_off];
            out.push(b);
        }
        nlit = 2;
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    /// Deterministic xorshift for test inputs.
    pub(crate) struct Rng(u64);
    impl Rng {
        pub(crate) fn new(seed: u64) -> Self {
            Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1)
        }
        pub(crate) fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        pub(crate) fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// Synthetic inputs: random, small alphabets, runs, text-like, structured.
    pub(crate) fn synth(kind: u32, len: usize, seed: u64) -> Vec<u8> {
        let mut r = Rng::new(seed);
        let mut v = Vec::with_capacity(len);
        match kind % 6 {
            0 => (0..len).for_each(|_| v.push(r.next() as u8)),
            1 => (0..len).for_each(|_| v.push(b"ab"[r.below(2) as usize])),
            2 => {
                while v.len() < len {
                    let b = r.next() as u8 & 0x3;
                    let n = r.below(300) as usize + 1;
                    v.extend(std::iter::repeat_n(b, n));
                }
            }
            3 => {
                let words = ["the ", "frog ", "swampy ", "revenge ", "lily", "pad\n", "hop ", "croak ", "baby ", "frogger "];
                while v.len() < len {
                    v.extend_from_slice(words[r.below(words.len() as u64) as usize].as_bytes());
                }
            }
            4 => {
                // repeated chunks with mutations (long matches at varied offsets)
                let chunk: Vec<u8> = (0..r.below(5000) + 1).map(|_| r.next() as u8).collect();
                while v.len() < len {
                    for &b in &chunk {
                        v.push(if r.below(50) == 0 { r.next() as u8 } else { b });
                    }
                }
            }
            _ => {
                // 16-bit little-endian ramps with noise, like vertex/texture data
                let mut x: u16 = 0;
                while v.len() < len {
                    x = x.wrapping_add((r.below(5)) as u16);
                    v.extend_from_slice(&x.to_le_bytes());
                    if r.below(7) == 0 {
                        v.push(0);
                    }
                }
            }
        }
        v.truncate(len);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;

    fn naive_sa(s: &[u8]) -> Vec<u32> {
        let mut v: Vec<u32> = (0..s.len() as u32).collect();
        v.sort_by(|&a, &b| s[a as usize..].cmp(&s[b as usize..]));
        v
    }

    #[test]
    fn apultra_sa_matches_naive() {
        let mut r = Rng::new(7);
        for t in 0..400u64 {
            let len = r.below(300) as usize;
            let s = synth(t as u32, len, t);
            assert_eq!(suffix_array(&s), naive_sa(&s), "kind {} len {}", t % 6, len);
        }
        for s in [&b""[..], b"a", b"aa", b"aaaaaaaaaa", b"abababab", b"mississippi", b"\xff\x00\xff\x00\xff"] {
            assert_eq!(suffix_array(s), naive_sa(s));
        }
        let big = synth(4, 20000, 99);
        assert_eq!(suffix_array(&big), naive_sa(&big));
    }

    #[test]
    fn apultra_gamma2_table() {
        for v in 2..5000 {
            assert_eq!(get_gamma2_size(v), 2 * (31 - (v as u32).leading_zeros() as i32));
        }
    }

    fn roundtrip(data: &[u8]) {
        let (c, st) = compress(data, 256 * 1024);
        let d = decompress(&c).expect("decode");
        assert_eq!(d.len(), data.len());
        assert!(d == data, "roundtrip mismatch (len {})", data.len());
        assert!(st.safe_dist >= 0);
    }

    #[test]
    fn apultra_roundtrip_small() {
        roundtrip(&[]);
        roundtrip(&[0]);
        roundtrip(&[42]);
        roundtrip(&[0, 0]);
        roundtrip(&[1, 2, 3]);
        for t in 0..120u64 {
            let len = (t as usize * 37) % 3000;
            roundtrip(&synth(t as u32, len, t));
        }
    }

    #[test]
    fn apultra_roundtrip_large() {
        // Single block > 256 KiB (46 arrivals) and multi-block (> 1 MiB).
        roundtrip(&synth(3, 300_000, 1));
        roundtrip(&synth(4, BLOCK_SIZE + 12345, 2));
        roundtrip(&synth(2, 2 * BLOCK_SIZE + 7, 3));
    }

    /// Golden vectors produced by libdragon's `mkasset -c 2 -w 256` (payload after the DCA header).
    #[test]
    fn apultra_golden() {
        let (c, st) = compress(b"a", 2048);
        assert_eq!(c, [0x61, 0xc0, 0x00]);
        assert_eq!(st.safe_dist, 0);
        let (c, _) = compress(b"Hello, Hello, Hello, Hello, Hello! frog frog frog", 2048);
        assert_eq!(c, [0x48, 0x38, 0x65, 0x6c, 0x8a, 0x6f, 0x2c, 0x20, 0x07, 0xd4, 0x77, 0x21, 0x3a, 0x66, 0x72, 0xa9, 0x67, 0x05, 0x4c, 0x00]);
        assert_eq!(decompress(&c).unwrap(), b"Hello, Hello, Hello, Hello, Hello! frog frog frog");
    }
}
