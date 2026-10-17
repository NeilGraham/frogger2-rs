//! Kraken (Oodle) block encoder of the PS5 fake package's inner image, ported from
//! LibProsperoPkg 1.2.0 (drakmor, GPL-3.0-or-later: `PFS/Compression/Oodle/OodleKrakenEncoder.cs`
//! and its helpers) so that packages match what UltraPack's `ffpfsc-pkg-tool` writes byte for
//! byte. Only the path the package builder takes is ported: compression level 7, Huffman
//! arrays, sub-literals and stored halves allowed, the suffix-trie match finder and the
//! optimal parser (windowed for chunks over 4 KiB).

mod cost;
mod huffman;
mod trie;

use cost::{
    build_from_passinfo, cost_add_literal, cost_len, cost_literals, cost_lo_match, cost_normal_match, cost_offset, offset_bucket_byte, BitWriter, CodeCosts,
};

/// One encoded 256 KiB (at most) block: one or two 128 KiB sub-chunks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EncodedBlock {
    pub payload: Vec<u8>,
    pub multi_chunk: bool,
    pub first_chunk_comp_size: usize,
    /// The form/literal-mode nibbles of both sub-chunks (`BoundaryFlags`).
    pub boundary_flags: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Lz,
    BareEntropy,
    Verbatim,
}

fn form_nibble(form: Form, lit_mode: i32, first: bool) -> i32 {
    match form {
        Form::Verbatim => 12,
        Form::BareEntropy => 4,
        Form::Lz => (if first { 6 } else { 2 }) | if lit_mode == 0 { 1 } else { 0 },
    }
}

#[derive(Clone, Copy)]
struct Command {
    lit_start: i32,
    lit_len: i32,
    distance: i32,
    match_len: i32,
    offs_index: i32,
}

#[derive(Clone, Copy)]
struct Cand {
    len: i32,
    dist: i32,
    idx: i32,
}

impl Cand {
    const NONE: Cand = Cand { len: 0, dist: 0, idx: -1 };

    fn valid(&self) -> bool {
        self.idx >= 0
    }

    fn value(&self) -> i32 {
        if self.idx >= 0 {
            self.len * 4 - if self.idx >= 3 { bit_len(self.dist as u32) + 2 } else { 0 }
        } else {
            -1_073_741_824
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Arrival {
    cost: i32,
    r0: i32,
    r1: i32,
    r2: i32,
    ml: i32,
    lrl: i32,
    src: i32,
    idx: i32,
    dist: i32,
    cont_lrl: i32,
    cont_ml: i32,
}

const CHUNK_MAX: usize = 131_072;
const MAX_ARRAY_LENGTH: usize = 262_143;
const NORMAL_MATCH_OFFSET_THRESHOLD: [i32; 6] = [0, 0, 0, 16384, 131_072, 1_048_576];
const ROUND_UP_TINY: [i32; 8] = [0, 8, 8, 9, 8, 10, 12, 14];
const OPTIMAL_MAX_BLOCK: i32 = 4096;
const OPTIMAL_WINDOWED_MAX_BLOCK: i32 = 262_144;

fn bit_len(v: u32) -> i32 {
    if v != 0 {
        32 - v.leading_zeros() as i32
    } else {
        0
    }
}

/// Encodes one block (128 bytes to 256 KiB) at level 7; `None` when it does not get smaller.
pub(crate) fn encode_block(data: &[u8]) -> Option<EncodedBlock> {
    let length = data.len();
    if !(128..=262_144).contains(&length) {
        return None;
    }
    let mut enc = Encoder {
        data,
        match_table: trie::match_table(data),
        carried_passinfo: None,
        carried_lit_mode: -1,
        pending_carry: None,
        greedy_mml: 0,
        use_tiny_offset_remap: false,
        use_dp_mml3: false,
        use_windowed_parse: false,
        dp_max_reached: 0,
    };
    let first_len = length.min(CHUNK_MAX);
    let block_constant = is_constant_run(data);
    let (p0, f0, m0) = enc.encode_sub_chunk(0, first_len, true, block_constant);
    if first_len == length {
        if f0 == Form::Verbatim || p0.len() >= length {
            return None;
        }
        let flags = form_nibble(f0, m0, true);
        return Some(EncodedBlock { first_chunk_comp_size: p0.len(), payload: p0, multi_chunk: false, boundary_flags: flags });
    }
    let (p1, f1, m1) = enc.encode_sub_chunk(first_len, length, false, block_constant);
    if f0 == Form::Verbatim && f1 == Form::Verbatim {
        return None;
    }
    if p0.len() > CHUNK_MAX {
        return None;
    }
    let first = p0.len();
    let mut payload = p0;
    payload.extend_from_slice(&p1);
    if payload.len() >= length {
        return None;
    }
    let flags = form_nibble(f0, m0, true) | (form_nibble(f1, m1, false) << 4);
    Some(EncodedBlock { payload, multi_chunk: true, first_chunk_comp_size: first, boundary_flags: flags })
}

fn is_constant_run(chunk: &[u8]) -> bool {
    chunk.iter().all(|&b| b == chunk[0])
}

fn copy_decode_time(length: i32) -> f32 {
    length as f32 * 0.0625f32
}

fn whole_chunk_deadline(length: i32) -> f32 {
    length as f32 * 0.57f32 - 50f32
}

fn array_decode_time(length: i32, non_zero: i32, runs: i32, average_bits: f32) -> f32 {
    let num = average_bits - 6f32;
    let per = if num <= 0f32 {
        0.25f32
    } else if num >= 1f32 {
        0.375f32
    } else {
        0.25f32 + num * 0.125f32
    };
    per * length as f32 + 20f32 + non_zero as f32 * 1f32 + runs as f32 * 4f32
}

/// `TryWholeChunkArray`: the whole chunk as one Huffman array, when it beats `budget`.
fn try_whole_chunk_array(chunk: &[u8], budget: f32) -> (Option<Vec<u8>>, f32) {
    let length = chunk.len() as i32;
    let Some(prepared) = huffman::prepare(chunk) else { return (None, f32::MAX) };
    let cap = if budget < (length + 3) as f32 { budget } else { (length + 3) as f32 };
    let Some(cost) = huffman::cost_of(&prepared) else {
        let num = 0.04f32 * copy_decode_time(length) + 8f32;
        if num >= cap {
            return (None, f32::MAX);
        }
        return (huffman::finish(chunk, &prepared), num);
    };
    let num2 = array_decode_time(length, cost.non_zero_symbols, cost.alphabet_runs, cost.code_length_bits as f32 / length as f32);
    if num2 > whole_chunk_deadline(length) {
        return (None, f32::MAX);
    }
    let num4 = 0.04f32 * num2 + 5f32;
    if ((((cost.code_length_bits + 7) >> 3) + 13 + cost.header_bytes as i64) as f32) + num4 >= cap {
        return (None, f32::MAX);
    }
    let Some(array) = huffman::finish(chunk, &prepared) else { return (None, f32::MAX) };
    if array.len() >= chunk.len() {
        return (None, f32::MAX);
    }
    let score = array.len() as f32 + num4;
    #[allow(clippy::neg_cmp_op_on_partial_ord)] // as the original: `!(score < limit)`
    if !(score < (length + 3) as f32) {
        return (None, score);
    }
    (Some(array), score)
}

/// `PutArray`: the cheaper of a Huffman array and a raw one, within `budget` (updated).
/// Returns (written, entropy form, written length).
fn put_array(array: &[u8], use_huffman: bool, budget: &mut f32) -> (bool, Option<Vec<u8>>, usize) {
    let count = array.len() as i32;
    let num = (count + 3) as f32;
    let num2 = if *budget < num { *budget } else { num };
    if use_huffman && count > 32 {
        let mut prepared = huffman::prepare(array);
        if let Some(huffman::Prepared::Single(_)) = prepared {
            let num3 = 0.04f32 * copy_decode_time(count) + 8f32;
            if num3 < num2 {
                *budget = num3;
                let p = prepared.as_ref().unwrap();
                return (true, huffman::finish(array, p), 8);
            }
            prepared = None;
        }
        if let Some(p) = &prepared {
            let cost = huffman::cost_of(p).unwrap();
            let num4 = array_decode_time(count, cost.non_zero_symbols, cost.alphabet_runs, cost.code_length_bits as f32 / count as f32);
            let num5 = 0.04f32 * num4 + 5f32;
            if ((((cost.code_length_bits + 7) >> 3) + 13 + cost.header_bytes as i64) as f32) + num5 < num2 {
                if let Some(a) = huffman::finish(array, p) {
                    let num6 = a.len() as f32 + num5;
                    if num6 < num {
                        *budget = num6;
                        let n = a.len();
                        return (true, Some(a), n);
                    }
                }
            }
        }
    }
    if *budget <= num {
        return (false, None, 0);
    }
    *budget = num;
    (true, None, (count + 3) as usize)
}

/// `WriteArray`: appends `array` in its cheapest form; returns its cost.
fn write_array(out: &mut Vec<u8>, array: &[u8], use_huffman: bool) -> f32 {
    let mut budget = f32::MAX;
    let (_, entropy, _) = put_array(array, use_huffman, &mut budget);
    match entropy {
        Some(e) => out.extend_from_slice(&e),
        None => {
            let c = array.len();
            out.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]);
            out.extend_from_slice(array);
        }
    }
    budget
}

fn choose_lit_mode(lit_raw: &[u8], lit_sub: &[u8], use_huffman: bool) -> i32 {
    if lit_sub.len() < 32 {
        return 1;
    }
    let mut budget = f32::MAX;
    let (_, _, written) = put_array(lit_sub, use_huffman, &mut budget);
    if written >= lit_sub.len() {
        return 1;
    }
    if put_array(lit_raw, use_huffman, &mut budget).0 {
        1
    } else {
        0
    }
}

fn build_literal_streams(data: &[u8], commands: &[Command], chunk_end: i32) -> (Vec<u8>, Vec<u8>) {
    let mut raw = Vec::new();
    let mut sub = Vec::new();
    let mut off: i32 = -8;
    for c in commands {
        for i in 0..c.lit_len {
            let p = (c.lit_start + i) as usize;
            raw.push(data[p]);
            sub.push(data[p].wrapping_sub(data[(p as i32 + off) as usize]));
        }
        if c.match_len > 0 {
            off = -c.distance;
        }
    }
    let last = commands[commands.len() - 1];
    for j in last.lit_start + last.lit_len + last.match_len..chunk_end {
        let p = j as usize;
        raw.push(data[p]);
        sub.push(data[p].wrapping_sub(data[(j + off) as usize]));
    }
    (raw, sub)
}

fn update_recent(r0: &mut i32, r1: &mut i32, r2: &mut i32, idx: i32, dist: i32) {
    match idx {
        0 => {}
        1 => std::mem::swap(r0, r1),
        2 => {
            let t = *r2;
            *r2 = *r1;
            *r1 = *r0;
            *r0 = t;
        }
        _ => {
            *r2 = *r1;
            *r1 = *r0;
            *r0 = dist;
        }
    }
}

fn common_prefix_length(data: &[u8], first: i32, second: i32, max_length: i32) -> i32 {
    let (f, s) = (first as usize, second as usize);
    let mut i: i32 = 0;
    while i <= max_length - 8 {
        let a = u64::from_le_bytes(data[f + i as usize..f + i as usize + 8].try_into().unwrap());
        let b = u64::from_le_bytes(data[s + i as usize..s + i as usize + 8].try_into().unwrap());
        let x = a ^ b;
        if x != 0 {
            return i + x.trailing_zeros() as i32 / 8;
        }
        i += 8;
    }
    while i < max_length && data[f + i as usize] == data[s + i as usize] {
        i += 1;
    }
    i
}

fn match_length(data: &[u8], a: i32, b: i32, limit: i32) -> i32 {
    common_prefix_length(data, a, b, limit - b)
}

fn rep_match_length(data: &[u8], pos: i32, dist: i32, limit: i32) -> i32 {
    let num = pos - dist;
    if num < 0 {
        return 0;
    }
    common_prefix_length(data, num, pos, limit - pos)
}

fn match_length_min4(data: &[u8], a: i32, b: i32, limit: i32) -> i32 {
    if a < 0 {
        return 0;
    }
    let n = match_length(data, a, b, limit);
    if n < 4 {
        0
    } else {
        n
    }
}

fn match_length_min3(data: &[u8], a: i32, b: i32, limit: i32) -> i32 {
    if a < 0 {
        return 0;
    }
    let n = match_length(data, a, b, limit);
    if n >= 4 {
        n
    } else if n < 3 {
        0
    } else {
        3
    }
}

fn is_normal_match_better(newml: i32, newoff: i32, bestml: i32, bestoff: i32) -> bool {
    if newml < bestml {
        return false;
    }
    if newml == bestml {
        return newoff < bestoff;
    }
    if newml >= bestml + 2 {
        return true;
    }
    (newoff >> 7) <= bestoff
}

fn take_new_over_rep(rep_len: i32, new_len: i32, new_off: i32) -> bool {
    if rep_len < 2 {
        return true;
    }
    if new_len < rep_len + 2 {
        return false;
    }
    if new_len < rep_len + 3 && new_off >= 1024 {
        return false;
    }
    if new_len < rep_len + 4 && new_off >= 65536 {
        return false;
    }
    true
}

fn is_allowed_normal_match(ml: i32, off: i32) -> bool {
    if ml < 6 {
        off < NORMAL_MATCH_OFFSET_THRESHOLD[ml as usize]
    } else {
        true
    }
}

fn insert_candidate(cml: &mut [i32], cdist: &mut [i32], n: usize, len: i32, dist: i32, cap: usize) -> usize {
    if cdist[..n].contains(&dist) {
        return n;
    }
    let mut j = 0;
    while j < n && cml[j] >= len {
        j += 1;
    }
    if j >= cap {
        return n;
    }
    let mut k = if n < cap { n } else { cap - 1 };
    while k > j {
        cml[k] = cml[k - 1];
        cdist[k] = cdist[k - 1];
        k -= 1;
    }
    cml[j] = len;
    cdist[j] = dist;
    if n >= cap {
        n
    } else {
        n + 1
    }
}

/// The per-block state of the original's thread statics.
struct Encoder<'a> {
    data: &'a [u8],
    match_table: Vec<i32>,
    carried_passinfo: Option<Vec<i32>>,
    carried_lit_mode: i32,
    pending_carry: Option<Vec<i32>>,
    greedy_mml: i32,
    use_tiny_offset_remap: bool,
    use_dp_mml3: bool,
    use_windowed_parse: bool,
    dp_max_reached: i32,
}

impl Encoder<'_> {
    /// `EncodeSubChunk`: (payload, form, literal mode).
    fn encode_sub_chunk(&mut self, chunk_start: usize, chunk_end: usize, is_first: bool, block_constant: bool) -> (Vec<u8>, Form, i32) {
        let num = (chunk_end - chunk_start) as i32;
        let chunk = &self.data[chunk_start..chunk_end];
        if block_constant {
            if num <= 8 {
                return (chunk.to_vec(), Form::Verbatim, 1);
            }
            return (huffman::single_symbol_array(chunk[0], num as usize), Form::BareEntropy, 1);
        }
        if num < 32 {
            return (chunk.to_vec(), Form::Verbatim, 1);
        }
        if is_constant_run(chunk) {
            return (huffman::single_symbol_array(chunk[0], num as usize), Form::BareEntropy, 1);
        }
        let num2 = num as f32 + 3f32 + 0.04f32 * copy_decode_time(num);
        let mut lz: Option<(Vec<u8>, i32)> = None;
        let mut num3 = f32::MAX;
        if num > 128 {
            if let Some((payload, lit_mode, score)) = self.encode_chunk(chunk_start as i32, chunk_end as i32, is_first) {
                if (payload.len() as i32) < num {
                    lz = Some((payload, lit_mode));
                    num3 = score + 3f32;
                }
            }
        }
        let budget = if num3 < num2 { num3 } else { num2 };
        let (array2, score2) = try_whole_chunk_array(chunk, budget);
        if let Some((payload, lit_mode)) = lz {
            if num3 < num2 && num3 <= score2 {
                return (payload, Form::Lz, lit_mode);
            }
        }
        match array2 {
            Some(a) if num2 > score2 => (a, Form::BareEntropy, 1),
            _ => (chunk.to_vec(), Form::Verbatim, 1),
        }
    }

    /// `EncodeChunk`: (payload, literal mode, score).
    fn encode_chunk(&mut self, chunk_start: i32, chunk_end: i32, with_seed: bool) -> Option<(Vec<u8>, i32, f32)> {
        let match_limit = chunk_end - 8;
        let match_start_limit = chunk_end - 16;
        let start = if with_seed { chunk_start + 8 } else { chunk_start };
        let size = chunk_end - chunk_start;
        let windowed = size > OPTIMAL_MAX_BLOCK && size <= OPTIMAL_WINDOWED_MAX_BLOCK;
        let saved = (self.use_dp_mml3, self.use_windowed_parse, self.use_tiny_offset_remap);
        self.use_dp_mml3 = true;
        if windowed {
            self.use_windowed_parse = true;
            self.use_tiny_offset_remap = true;
        }
        let list = self.parse_optimal(start, match_limit, match_start_limit, chunk_start, chunk_end, with_seed);
        (self.use_dp_mml3, self.use_windowed_parse, self.use_tiny_offset_remap) = saved;
        if list.is_empty() {
            return None;
        }
        let emitted = self.emit_chunk(&list, chunk_start, chunk_end, with_seed, -1);
        let pending = self.pending_carry.take();
        let (payload, lit_mode, score) = emitted?;
        if let Some(p) = pending {
            self.carried_passinfo = Some(p);
            self.carried_lit_mode = lit_mode;
        }
        Some((payload, lit_mode, score))
    }

    /// `EmitChunkFromCommands`: (payload, chosen literal mode, score); `lit_mode` -1 chooses.
    fn emit_chunk(&self, commands: &[Command], chunk_start: i32, chunk_end: i32, with_seed: bool, lit_mode: i32) -> Option<(Vec<u8>, i32, f32)> {
        let data = self.data;
        let mut score = 0f32;
        let num = chunk_end - 16;
        if commands.iter().any(|c| c.match_len > 0 && c.lit_start + c.lit_len > num) {
            return None;
        }
        let (lit_raw, lit_sub) = build_literal_streams(data, commands, chunk_end);
        let mut packets: Vec<u8> = Vec::new();
        let mut offsets: Vec<u8> = Vec::new();
        let mut packed_len: Vec<u8> = Vec::new();
        let mut w_even = BitWriter::default();
        let mut w_odd = BitWriter::default();
        let mut escapes: Vec<u32> = Vec::new();
        let emit_len = |v: i32, packed: &mut Vec<u8>, escapes: &mut Vec<u32>| {
            if v < 255 {
                packed.push(v as u8);
            } else {
                packed.push(255);
                escapes.push((v - 255) as u32);
            }
        };
        let mut n_off = 0;
        for c in commands {
            if c.match_len == 0 {
                continue;
            }
            let lit_field = c.lit_len.min(3);
            let len_field = if c.match_len <= 16 { c.match_len - 2 } else { 15 };
            packets.push(((c.offs_index << 6) | (len_field << 2) | lit_field) as u8);
            if lit_field == 3 {
                emit_len(c.lit_len - 3, &mut packed_len, &mut escapes);
            }
            if c.offs_index == 3 {
                let b = if n_off & 1 == 0 { w_even.write_distance(c.distance) } else { w_odd.write_distance(c.distance) };
                offsets.push(b);
                n_off += 1;
            }
            if len_field == 15 {
                emit_len(c.match_len - 17, &mut packed_len, &mut escapes);
            }
        }
        if escapes.len() > 512 {
            return None;
        }
        if lit_raw.len() > MAX_ARRAY_LENGTH || packets.len() > MAX_ARRAY_LENGTH || offsets.len() > MAX_ARRAY_LENGTH || packed_len.len() > MAX_ARRAY_LENGTH {
            return None;
        }
        let chosen = if lit_mode >= 0 { lit_mode } else { choose_lit_mode(&lit_raw, &lit_sub, true) };
        let lits = if chosen == 0 { &lit_sub } else { &lit_raw };
        let dist_even = w_even.into_bytes();
        let dist_odd = w_odd.into_bytes();
        let mut out: Vec<u8> = Vec::with_capacity((chunk_end - chunk_start) as usize);
        if with_seed {
            out.extend_from_slice(&data[chunk_start as usize..chunk_start as usize + 8]);
        }
        let mut l_even = BitWriter::default();
        let mut l_odd = BitWriter::default();
        for (j, &e) in escapes.iter().enumerate() {
            if j & 1 == 0 {
                l_even.write_length(e);
            } else {
                l_odd.write_length(e);
            }
        }
        let len_even = l_even.into_bytes();
        let len_odd = l_odd.into_bytes();
        let num5 = len_even.len() + len_odd.len();
        if num5 > 8191 {
            return None;
        }
        if num5 <= 31 {
            out.push(0x80 | num5 as u8);
        } else {
            out.push(0xA0 | (num5 & 0x1f) as u8);
            out.push(((num5 >> 5) - 1) as u8);
        }
        let count = out.len();
        score += write_array(&mut out, lits, true);
        score += write_array(&mut out, &packets, true);
        score += write_array(&mut out, &offsets, true);
        score += write_array(&mut out, &packed_len, true);
        let num6 = out.len() - count;
        out.extend_from_slice(&dist_even);
        out.extend(dist_odd.iter().rev());
        out.extend_from_slice(&len_even);
        out.extend(len_odd.iter().rev());
        score += (out.len() - num6) as f32;
        Some((out, chosen, score))
    }

    fn real_emit(&self, cand: &[Command], chunk_start: i32, chunk_end: i32, with_seed: bool) -> i32 {
        if cand.is_empty() {
            return i32::MAX;
        }
        self.emit_chunk(cand, chunk_start, chunk_end, with_seed, -1).map_or(i32::MAX, |(p, _, _)| p.len() as i32)
    }

    fn lit_mode_for_parse(&self, commands: &[Command], chunk_end: i32) -> i32 {
        if commands.is_empty() {
            return 1;
        }
        let (raw, sub) = build_literal_streams(self.data, commands, chunk_end);
        choose_lit_mode(&raw, &sub, true)
    }

    fn parse_optimal(&mut self, start: i32, match_limit: i32, match_start_limit: i32, chunk_start: i32, chunk_end: i32, with_seed: bool) -> Vec<Command> {
        let list = self.greedy(4, start, match_limit, match_start_limit);
        if list.is_empty() {
            return list;
        }
        let mut best = list;
        let mut num = self.real_emit(&best, chunk_start, chunk_end, with_seed);
        for mml in [3, 8] {
            let l = self.greedy(mml, start, match_limit, match_start_limit);
            let n = self.real_emit(&l, chunk_start, chunk_end, with_seed);
            if n < num {
                num = n;
                best = l;
            }
        }
        let lit_mode = self.lit_mode_for_parse(&best, chunk_end);
        let mut pi = tally(&best, self.data, start, 1, -1, &mut 8, &mut 8, &mut 8);
        match &self.carried_passinfo {
            None => decay_seed_passinfo(&mut pi),
            Some(c) => blend_seed_passinfo(&mut pi, c, self.carried_lit_mode == lit_mode),
        }
        let cc = build_from_passinfo(&pi, lit_mode);
        let mut adapt = if self.use_windowed_parse { Some(pi.clone()) } else { None };
        let dp = self.forward_dp(start, match_limit, match_start_limit, cc, adapt.as_mut(), lit_mode);
        let n = self.real_emit(&dp, chunk_start, chunk_end, with_seed);
        self.pending_carry = None;
        if n < num {
            best = dp;
            // The windowed parse adds its segments' statistics to the same table.
            self.pending_carry = Some(adapt.unwrap_or(pi));
        }
        best
    }

    /// `Parse` with the exact greedy finder (`Greedy` of `ParseOptimal`).
    fn greedy(&mut self, mml: i32, start: i32, match_limit: i32, match_start_limit: i32) -> Vec<Command> {
        let saved = (self.greedy_mml, self.use_tiny_offset_remap);
        self.greedy_mml = mml;
        self.use_tiny_offset_remap = true;
        let mut list = Vec::new();
        let mut lit_start = start;
        let mut pos = start;
        let (mut r0, mut r1, mut r2) = (8, 8, 8);
        while pos <= match_start_limit {
            let mut cand = self.find_match_exact(pos, match_limit, r0, r1, r2, pos - lit_start);
            if !cand.valid() {
                pos += 1;
                continue;
            }
            while pos < match_start_limit {
                let c2 = self.find_match_exact(pos + 1, match_limit, r0, r1, r2, pos + 1 - lit_start);
                if c2.value() - cand.value() - 4 >= 1 {
                    pos += 1;
                    cand = c2;
                    continue;
                }
                if pos + 2 > match_start_limit {
                    break;
                }
                let c3 = self.find_match_exact(pos + 2, match_limit, r0, r1, r2, pos + 2 - lit_start);
                if c3.value() - cand.value() - 4 < 4 {
                    break;
                }
                pos += 2;
                cand = c3;
            }
            let mut idx = cand.idx;
            if idx == 0 && pos == start && r0 == 8 && r1 == r0 {
                idx = 1;
            }
            list.push(Command { lit_start, lit_len: pos - lit_start, distance: cand.dist, match_len: cand.len, offs_index: idx });
            update_recent(&mut r0, &mut r1, &mut r2, idx, cand.dist);
            pos += cand.len;
            lit_start = pos;
        }
        (self.greedy_mml, self.use_tiny_offset_remap) = saved;
        list
    }

    fn find_match_exact(&self, pos: i32, match_limit: i32, r0: i32, r1: i32, r2: i32, lrl: i32) -> Cand {
        let data = self.data;
        let reps = [r0, r1, r2];
        let (mut best, mut best_i) = (-1, -1);
        for (i, &r) in reps.iter().enumerate() {
            let l = rep_match_length(data, pos, r, match_limit);
            let l = if l >= 2 { l } else { 0 };
            if best < l {
                best = l;
                best_i = i as i32;
            }
        }
        if best >= 4 {
            return Cand { len: best, dist: reps[best_i as usize], idx: best_i };
        }
        let mut mml = if self.greedy_mml > 0 { self.greedy_mml } else { 4 };
        if lrl > 55 {
            if best < 3 {
                best = 0;
            }
            mml += 1;
        }
        let mut out_len = [0i32; 4];
        let mut out_dist = [0i32; 4];
        let n = self.find_pareto_pairs(pos, match_limit, &mut out_len, &mut out_dist);
        let (mut bl, mut bd) = (0, 0);
        for i in 0..n {
            let (len, dist) = (out_len[i], out_dist[i]);
            if len < mml {
                break;
            }
            let (mut l, mut d) = (len, dist);
            if self.use_tiny_offset_remap && dist < 8 {
                let t = ROUND_UP_TINY[dist as usize];
                if t == 0 || t > pos {
                    continue;
                }
                let m = match_length(data, pos - t, pos, match_limit);
                if m < mml {
                    continue;
                }
                l = m;
                d = t;
            }
            if is_allowed_normal_match(l, d) && is_normal_match_better(l, d, bl, bd) {
                bl = l;
                bd = d;
            }
        }
        if !take_new_over_rep(best, bl, bd) {
            return Cand { len: best, dist: reps[best_i as usize], idx: best_i };
        }
        if bl <= 0 {
            return Cand::NONE;
        }
        Cand { len: bl, dist: bd, idx: 3 }
    }

    fn find_pareto_pairs(&self, pos: i32, match_limit: i32, out_len: &mut [i32; 4], out_dist: &mut [i32; 4]) -> usize {
        let t = &self.match_table;
        let mml = if self.greedy_mml > 0 { self.greedy_mml } else { 4 };
        let base = pos as usize * 8;
        let mut last = -1;
        let mut n = 0;
        for i in 0..4 {
            if n >= 4 {
                break;
            }
            let mut len = t[base + i * 2];
            if len == 0 {
                break;
            }
            let dist = t[base + i * 2 + 1];
            if dist >= 8 && len > match_limit - pos {
                len = match_limit - pos;
                if len < mml || len == last {
                    continue;
                }
                last = len;
            }
            out_len[n] = len;
            out_dist[n] = dist;
            n += 1;
        }
        n
    }

    fn find_candidates(&self, pos: i32, match_limit: i32, cc: &CodeCosts, cml: &mut [i32; 8], cdist: &mut [i32; 8], coff: &mut [i32; 8]) -> usize {
        let data = self.data;
        let t = &self.match_table;
        let mml = if self.use_dp_mml3 { 3 } else { 4 };
        let base = pos as usize * 8;
        let mut last = -1;
        let mut n = 0usize;
        for i in 0..4 {
            let mut len = t[base + i * 2];
            if len == 0 {
                break;
            }
            let mut dist = t[base + i * 2 + 1];
            if dist < 8 {
                let tiny = ROUND_UP_TINY[dist as usize];
                if tiny == 0 || tiny > pos {
                    continue;
                }
                let m = match_length_min4(data, pos - tiny, pos, match_limit);
                if m < mml {
                    continue;
                }
                len = m;
                dist = tiny;
            } else if len > match_limit - pos {
                len = match_limit - pos;
            }
            if len >= mml && len != last {
                if pos + len >= match_limit {
                    last = len;
                }
                n = insert_candidate(cml, cdist, n, len, dist, 4);
            }
        }
        let m8 = match_length_min3(data, pos - 8, pos, match_limit);
        if pos >= 8 && m8 >= mml {
            n = insert_candidate(cml, cdist, n, m8, 8, 8);
        }
        for j in 0..n {
            coff[j] = cost_offset(cdist[j], cc);
        }
        n
    }

    #[allow(clippy::too_many_arguments)]
    fn relax(&mut self, arr: &mut [Arrival], target: i32, cost: i64, a: Arrival) {
        if self.use_windowed_parse && target > self.dp_max_reached {
            self.dp_max_reached = target;
        }
        let t = &mut arr[target as usize];
        if cost < t.cost as i64 {
            *t = Arrival { cost: cost as i32, ..a };
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn faef0(
        &mut self,
        arr: &mut [Arrival],
        s: i32,
        cost_at_e: i64,
        primary_ml: i32,
        primary_lrl: i32,
        primary_idx: i32,
        primary_dist: i32,
        src: i32,
        pr: (i32, i32, i32),
        cont_dist: i32,
        match_limit: i32,
        cc: &CodeCosts,
    ) {
        let data = self.data;
        if s + 3 > match_limit || cont_dist < 8 || s + 1 - cont_dist < 0 {
            return;
        }
        let su = s as usize;
        let cd = cont_dist as usize;
        let num = if data[su + 2] == data[su + 2 - cd] {
            if data[su + 1] != data[su + 1 - cd] {
                1
            } else {
                2
            }
        } else {
            0
        };
        let num2 = rep_match_length(data, s + 3, cont_dist, match_limit);
        let lrl = 3 - num;
        let ml = num2 + num;
        if ml < 2 {
            return;
        }
        let mut cost = cost_at_e + cost_literals(data, s, lrl, cont_dist, cc) as i64;
        let mut lit_field = lrl;
        if lrl > 2 {
            lit_field = 3;
            cost += cost_len(cc, 0) as i64;
        }
        cost += cost_lo_match(cc, lit_field, ml, 0) as i64;
        let target = s + lrl + ml;
        self.relax(
            arr,
            target,
            cost,
            Arrival {
                cost: 0,
                r0: pr.0,
                r1: pr.1,
                r2: pr.2,
                ml: primary_ml,
                lrl: primary_lrl,
                src,
                idx: primary_idx,
                dist: primary_dist,
                cont_lrl: lrl,
                cont_ml: ml,
            },
        );
    }

    fn forward_dp(
        &mut self,
        start: i32,
        match_limit: i32,
        match_start_limit: i32,
        mut cc: CodeCosts,
        mut adapt: Option<&mut Vec<i32>>,
        adapt_lit_mode: i32,
    ) -> Vec<Command> {
        let data = self.data;
        let chunk_end = match_limit + 8;
        let mut arr = vec![Arrival { cost: i32::MAX, ..Arrival::default() }; (match_limit + 1) as usize];
        arr[start as usize] = Arrival { cost: 0, r0: 8, r1: 8, r2: 8, src: start, idx: -1, ..Arrival::default() };
        let mut cml = [0i32; 8];
        let mut cdist = [0i32; 8];
        let mut coff = [0i32; 8];
        let windowed = self.use_windowed_parse && adapt.is_some();
        let mut seg_start = start; // num3
        let mut list: Vec<Command> = Vec::new();
        let (mut r0, mut r1, mut r2) = (8, 8, 8);
        let mut anchor = start; // num4
        let mut anchor_lits: i64 = 0; // num5
        let mut clear_to = start; // num6
        loop {
            let mut horizon = match_start_limit; // num7
            if windowed {
                anchor = seg_start;
                anchor_lits = 0;
                let mut commit = seg_start + 256;
                if commit > match_start_limit {
                    commit = match_start_limit;
                }
                if match_start_limit - 16 <= commit {
                    commit = match_start_limit;
                }
                self.dp_max_reached = commit;
                horizon = seg_start + 4096;
                if horizon > match_start_limit {
                    horizon = match_start_limit;
                }
                if match_start_limit - 16 <= horizon {
                    horizon = match_start_limit;
                }
                if seg_start > start {
                    let to = if clear_to < match_limit { clear_to } else { match_limit };
                    for l in seg_start + 1..=to {
                        arr[l as usize].cost = i32::MAX;
                    }
                }
            }
            let mut seg_end = -1; // num10
            let mut m = seg_start;
            while m <= match_start_limit {
                if windowed {
                    if self.dp_max_reached > horizon {
                        seg_end = self.dp_max_reached;
                        break;
                    }
                    if m >= match_start_limit {
                        seg_end = -1;
                        break;
                    }
                }
                if m > seg_start {
                    let a = &arr[anchor as usize];
                    let lo = if a.r0 < 8 { 8 } else { a.r0 };
                    anchor_lits += cost_add_literal(data, m - 1, lo, &cc) as i64;
                    if arr[m as usize].cost != i32::MAX {
                        let run = m - anchor;
                        let via_anchor = arr[anchor as usize].cost as i64 + if run > 2 { cost_len(&cc, run - 3) as i64 } else { 0 } + anchor_lits;
                        if (arr[m as usize].cost as i64) < via_anchor {
                            anchor = m;
                            anchor_lits = 0;
                        }
                    }
                }
                let ncand = self.find_candidates(m, match_limit, &cc, &mut cml, &mut cdist, &mut coff);
                let mut best_base = i64::MAX; // num14
                let mut longest = 0; // num15
                for n in 0..=3 {
                    let mut run = n; // num16
                    if n == 3 && 3 < m - anchor {
                        run = m - anchor;
                    }
                    let from = m - run; // num17
                    if from < seg_start {
                        break;
                    }
                    if arr[from as usize].cost == i32::MAX {
                        continue;
                    }
                    let af = arr[from as usize];
                    let lo2 = if af.r0 < 8 { 8 } else { af.r0 };
                    let lits = if run == m - anchor { anchor_lits } else { cost_literals(data, m - run, run, lo2, &cc) as i64 };
                    let mut base = af.cost as i64 + lits; // num19
                    let lit_field = run.min(3);
                    if run > 2 {
                        base += cost_len(&cc, run - 3) as i64;
                    }
                    let (r4, r5, r6) = (af.r0, af.r1, af.r2);
                    let mut rep_best = 0; // num20
                    for k in 0..3 {
                        let rd = match k {
                            0 => r4,
                            1 => r5,
                            _ => r6,
                        };
                        if rd < 8 {
                            continue;
                        }
                        let rl = rep_match_length(data, m, rd, match_limit);
                        if rl < 2 || rl <= rep_best {
                            continue;
                        }
                        rep_best = rl;
                        let (o1, o2) = match k {
                            0 => (r5, r6),
                            1 => (r4, r6),
                            _ => (r4, r5),
                        };
                        let rep_arrival =
                            |ml: i32| Arrival { cost: 0, r0: rd, r1: o1, r2: o2, ml, lrl: run, src: from, idx: k, dist: rd, cont_lrl: 0, cont_ml: 0 };
                        self.relax(&mut arr, m + rl, base + cost_lo_match(&cc, lit_field, rl, k) as i64, rep_arrival(rl));
                        if ((rl - 3) as u32) < 125 {
                            for sub in 2..rl {
                                self.relax(&mut arr, m + sub, base + cost_lo_match(&cc, lit_field, sub, k) as i64, rep_arrival(sub));
                            }
                        }
                        let cost_at_e = base + cost_lo_match(&cc, lit_field, rl, k) as i64;
                        self.faef0(&mut arr, m + rl, cost_at_e, rl, run, k, rd, from, (rd, o1, o2), rd, match_limit, &cc);
                    }
                    if rep_best > longest {
                        longest = rep_best;
                    }
                    if longest >= 128 {
                        break;
                    }
                    if !(base < best_base) {
                        continue;
                    }
                    best_base = base;
                    for c in 0..ncand {
                        let ml = cml[c];
                        if ml <= rep_best {
                            break;
                        }
                        let dist = cdist[c];
                        if dist == r4 || dist == r5 || dist == r6 {
                            continue;
                        }
                        if ml > longest {
                            longest = ml;
                        }
                        let with_off = base + coff[c] as i64;
                        let mml = if self.use_dp_mml3 { 3 } else { 4 };
                        let fill = mml + 1;
                        let new_arrival =
                            |l: i32| Arrival { cost: 0, r0: dist, r1: r4, r2: r5, ml: l, lrl: run, src: from, idx: 3, dist, cont_lrl: 0, cont_ml: 0 };
                        self.relax(&mut arr, m + ml, with_off + cost_normal_match(&cc, lit_field, ml) as i64, new_arrival(ml));
                        if ((ml - fill) as u32) < (128 - fill) as u32 {
                            for sub in mml..ml {
                                self.relax(&mut arr, m + sub, with_off + cost_normal_match(&cc, lit_field, sub) as i64, new_arrival(sub));
                            }
                        }
                        let cost_at_e = with_off + cost_normal_match(&cc, lit_field, ml) as i64;
                        self.faef0(&mut arr, m + ml, cost_at_e, ml, run, 3, dist, from, (dist, r4, r5), dist, match_limit, &cc);
                    }
                }
                if windowed && longest >= 128 {
                    let mut to = m + longest;
                    if to > match_start_limit {
                        to = match_start_limit;
                    }
                    if to > m && arr[to as usize].cost != i32::MAX {
                        anchor = to;
                        anchor_lits = 0;
                        seg_end = to;
                        break;
                    }
                }
                m += 1;
            }
            if !windowed {
                let e = estar_select(&arr, data, start, match_limit, chunk_end, &cc);
                if e < 0 {
                    return Vec::new();
                }
                return back_trace(&arr, start, e, match_limit).unwrap_or_default();
            }
            if seg_end < 0 || seg_end >= match_start_limit {
                let e = estar_select(&arr, data, seg_start, match_limit, chunk_end, &cc);
                if e > seg_start {
                    if let Some(l) = back_trace(&arr, seg_start, e, match_limit) {
                        list.extend(l);
                    }
                }
                return list;
            }
            clear_to = self.dp_max_reached + 128;
            if clear_to > match_limit {
                clear_to = match_limit;
            }
            let Some(seg) = back_trace(&arr, seg_start, seg_end, match_limit) else { return list };
            let counts = tally(&seg, data, seg_start, 2, -1, &mut r0, &mut r1, &mut r2);
            list.extend(seg);
            let pi = adapt.as_deref_mut().unwrap();
            let n = pi.len().min(counts.len());
            for i in 0..n {
                pi[i] += counts[i];
            }
            cc = build_from_passinfo(pi, adapt_lit_mode);
            seg_start = seg_end;
            if seg_start >= match_start_limit {
                break;
            }
        }
        let e = estar_select(&arr, data, seg_start, match_limit, chunk_end, &cc);
        if e > seg_start {
            if let Some(l) = back_trace(&arr, seg_start, e, match_limit) {
                list.extend(l);
            }
        }
        list
    }
}

fn estar_select(arr: &[Arrival], data: &[u8], from: i32, match_limit: i32, chunk_end: i32, cc: &CodeCosts) -> i32 {
    let mut result = -1;
    let mut best = i64::MAX;
    for i in from + 1..=match_limit {
        let a = &arr[i as usize];
        if a.cost != i32::MAX {
            let lo = if a.r0 < 8 { 8 } else { a.r0 };
            let c = a.cost as i64 + cost_literals(data, i, chunk_end - i, lo, cc) as i64;
            if c < best {
                best = c;
                result = i;
            }
        }
    }
    result
}

fn back_trace(arr: &[Arrival], from: i32, to: i32, max_arr: i32) -> Option<Vec<Command>> {
    let mut list = Vec::new();
    let mut num = to;
    let mut guard = 0;
    while num != from {
        let ok = guard <= max_arr;
        guard += 1;
        if !ok {
            break;
        }
        let a = arr[num as usize];
        if a.ml <= 0 || a.src < from || a.src >= num {
            break;
        }
        if a.cont_ml > 0 {
            list.push(Command { lit_start: a.src + a.lrl + a.ml, lit_len: a.cont_lrl, distance: a.r0, match_len: a.cont_ml, offs_index: 0 });
        }
        list.push(Command { lit_start: a.src, lit_len: a.lrl, distance: a.dist, match_len: a.ml, offs_index: a.idx });
        num = a.src;
    }
    if num != from {
        return None;
    }
    list.reverse();
    Some(list)
}

#[allow(clippy::too_many_arguments)]
fn tally(commands: &[Command], data: &[u8], start: i32, inc: i32, chunk_end: i32, r0: &mut i32, r1: &mut i32, r2: &mut i32) -> Vec<i32> {
    let mut a = vec![0i32; 1537];
    let mut num = start;
    for c in commands {
        for i in 0..c.lit_len {
            let p = c.lit_start + i;
            let b = data[p as usize] as i32;
            a[b as usize] += inc;
            let q = p - *r0;
            let prev = if q >= 0 { data[q as usize] as i32 } else { 0 };
            a[256 + ((b - prev) & 0xff) as usize] += inc;
        }
        if c.lit_len >= 3 {
            a[768 + (c.lit_len - 3).min(255) as usize] += inc;
        }
        let lit_field = c.lit_len.min(3);
        let ml = c.match_len;
        let rep = c.offs_index < 3;
        let len_field = if ml < 17 { ml - 2 } else { 15 };
        let idx_off = if rep { c.offs_index * 64 } else { 192 };
        a[(512 + lit_field + len_field * 4 + idx_off) as usize] += inc;
        if ml >= 17 {
            a[768 + (ml - 17).min(255) as usize] += inc;
        }
        if !rep {
            a[1025 + offset_bucket_byte(c.distance).0 as usize] += inc;
        }
        update_recent(r0, r1, r2, c.offs_index, c.distance);
        num = c.lit_start + c.lit_len + c.match_len;
    }
    if chunk_end > num {
        for j in num..chunk_end {
            let b = data[j as usize] as i32;
            a[b as usize] += inc;
            let q = j - *r0;
            let prev = if q >= 0 { data[q as usize] as i32 } else { 0 };
            a[256 + ((b - prev) & 0xff) as usize] += inc;
        }
    }
    a
}

fn blend_seed_passinfo(pi: &mut [i32], carried: &[i32], lit_mode_unchanged: bool) {
    let blend = |dst: &mut [i32], src: &[i32], start: usize| {
        for i in 0..256 {
            dst[start + i] = ((dst[start + i] + src[start + i]) >> 5) + 1;
        }
    };
    let decay = |p: &mut [i32], start: usize| {
        for i in 0..256 {
            p[start + i] = (p[start + i] >> 4) + 1;
        }
    };
    if !lit_mode_unchanged {
        decay(pi, 0);
        decay(pi, 256);
    } else {
        blend(pi, carried, 0);
        blend(pi, carried, 256);
    }
    blend(pi, carried, 512);
    blend(pi, carried, 768);
    if pi[1024] == carried[1024] {
        blend(pi, carried, 1025);
        if pi[1024] > 1 {
            blend(pi, carried, 1281);
        }
        return;
    }
    pi[1025..1281].copy_from_slice(&carried[1025..1281]);
    pi[1281..1537].copy_from_slice(&carried[1281..1537]);
    pi[1024] = carried[1024];
    decay(pi, 1025);
    if pi[1024] > 1 {
        decay(pi, 1281);
    }
}

fn decay_seed_passinfo(pi: &mut [i32]) {
    let decay = |p: &mut [i32], start: usize| {
        for i in 0..256 {
            p[start + i] = (p[start + i] >> 4) + 1;
        }
    };
    decay(pi, 0);
    decay(pi, 256);
    decay(pi, 1025);
    if pi[1024] > 1 {
        decay(pi, 1281);
    }
    decay(pi, 512);
    decay(pi, 768);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_runs() {
        let b = encode_block(&[b'x'; 70000]).unwrap();
        assert_eq!(b.payload, [0x24, 0x45, 0xbc, 0, 3, 0, 0x5e, 0]);
        assert_eq!((b.multi_chunk, b.first_chunk_comp_size, b.boundary_flags), (false, 8, 4));
        assert!(encode_block(&[1u8; 100]).is_none(), "too small");
    }

    /// Encodes `$F2_KRAKEN_IN` in 256 KiB blocks into `$F2_KRAKEN_OUT` in the format of the
    /// C# harness that runs the original encoder (u32 length or 0xFFFFFFFF, payload, u8 multi,
    /// i32 first, i32 flags), for comparing them on real data.
    #[test]
    #[ignore]
    fn vectors() {
        let input = std::fs::read(std::env::var("F2_KRAKEN_IN").unwrap()).unwrap();
        let mut out = Vec::new();
        for block in input.chunks(262_144) {
            match encode_block(block) {
                None => out.extend_from_slice(&u32::MAX.to_le_bytes()),
                Some(b) => {
                    out.extend_from_slice(&(b.payload.len() as u32).to_le_bytes());
                    out.extend_from_slice(&b.payload);
                    out.push(b.multi_chunk as u8);
                    out.extend_from_slice(&(b.first_chunk_comp_size as i32).to_le_bytes());
                    out.extend_from_slice(&b.boundary_flags.to_le_bytes());
                }
            }
        }
        std::fs::write(std::env::var("F2_KRAKEN_OUT").unwrap(), out).unwrap();
    }
}
