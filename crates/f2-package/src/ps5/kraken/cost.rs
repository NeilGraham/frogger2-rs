//! The optimal parser's cost model (`KrakenOptimalCost`): symbol costs in 1/32 bit from the
//! histograms of a previous parse.

pub(super) struct CodeCosts {
    pub lit_sub_mask: i32,
    pub lit: [i32; 256],
    pub packet: [i32; 256],
    pub offset_bucket: [i32; 256],
    pub length: [i32; 256],
}

const LOG2_MANTISSA: [u16; 65] = [
    0, 183, 364, 541, 716, 889, 1059, 1227, 1392, 1555, 1716, 1874, 2031, 2186, 2338, 2489, 2637, 2784, 2929, 3072, 3214, 3354, 3492, 3629, 3764, 3897, 4029,
    4160, 4289, 4417, 4543, 4668, 4792, 4914, 5036, 5156, 5274, 5392, 5509, 5624, 5738, 5851, 5963, 6074, 6184, 6293, 6401, 6508, 6614, 6719, 6823, 6926, 7029,
    7130, 7231, 7330, 7429, 7527, 7625, 7721, 7817, 7912, 8006, 8099, 8192,
];

pub(super) fn bit_len(x: u32) -> i32 {
    32 - x.leading_zeros() as i32
}

fn log2_fix(x: u32) -> i32 {
    let num = bit_len(x) - 1;
    let num2 = x.wrapping_shl(((32 - num) & 0x1f) as u32);
    let num3 = (num2 >> 26) as usize;
    let num4 = LOG2_MANTISSA[num3] as i32;
    let num5 = LOG2_MANTISSA[num3 + 1] as i32;
    let num6 = ((num2 & 0x3FF_FFFF) >> 10) as i32;
    let num7 = ((num5 - num4) * num6 + 32768) >> 16;
    (32 - num) * 8192 - num4 - num7
}

fn histo_to_code_cost(histo: &[i32], cost: &mut [i32; 256], bias: i32, threshold: i32) {
    let total: i32 = histo[..256].iter().sum();
    let num2 = total * 4 + 256;
    let num3 = log2_fix(num2 as u32);
    let mut num4: i64 = 0;
    for j in 0..256 {
        let num5 = histo[j] * 4 + 1;
        let num6 = ((log2_fix(num5 as u32) - num3) * 32) >> 13;
        num4 += num6 as i64 * num5 as i64;
        cost[j] = num6 + bias;
    }
    if (threshold as i64) * (num2 as i64) < num4 {
        cost.fill(bias + 256);
    }
}

/// `BuildFromPassinfo`: costs from a 0x601-entry histogram set (literals, sub-literals,
/// packets, lengths, offset buckets).
pub(super) fn build_from_passinfo(passinfo: &[i32], lit_mode: i32) -> CodeCosts {
    let mut cc = CodeCosts { lit_sub_mask: if lit_mode != 1 { 255 } else { 0 }, lit: [0; 256], packet: [0; 256], offset_bucket: [0; 256], length: [0; 256] };
    histo_to_code_cost(&passinfo[1025..1281], &mut cc.offset_bucket, 36, 255);
    // (The alternative offset table, passinfo[1024] > 1, is never used by the parser.)
    histo_to_code_cost(&passinfo[512..768], &mut cc.packet, 18, 255);
    histo_to_code_cost(&passinfo[768..1024], &mut cc.length, 12, 255);
    let lit = if lit_mode == 1 { &passinfo[0..256] } else { &passinfo[256..512] };
    histo_to_code_cost(lit, &mut cc.lit, 0, 255);
    cc
}

pub(super) fn cost_literals(buf: &[u8], start: i32, lrl: i32, lo: i32, cc: &CodeCosts) -> i32 {
    let mut num = 0;
    for i in 0..lrl {
        let p = (start + i) as usize;
        let sub = if cc.lit_sub_mask != 0 { buf[p - lo as usize] as i32 & cc.lit_sub_mask } else { 0 };
        num += cc.lit[((buf[p] as i32 - sub) & 0xff) as usize];
    }
    num
}

pub(super) fn cost_add_literal(buf: &[u8], pos: i32, lo: i32, cc: &CodeCosts) -> i32 {
    let p = pos as usize;
    let sub = if cc.lit_sub_mask != 0 { buf[p - lo as usize] as i32 & cc.lit_sub_mask } else { 0 };
    cc.lit[((buf[p] as i32 - sub) & 0xff) as usize]
}

/// The offset bucket byte of a match distance and its count of extra bits.
pub(super) fn offset_bucket_byte(dist: i32) -> (i32, i32) {
    let num = (dist + 248) as u32;
    let num2 = 31 - num.leading_zeros() as i32 - 8;
    let num3 = ((num as i32).wrapping_sub(1 << (num2 + 8))) & 0xf;
    ((num2 << 4) | num3, num2 + 4)
}

pub(super) fn cost_offset(dist: i32, cc: &CodeCosts) -> i32 {
    let (b, extra) = offset_bucket_byte(dist);
    let mut c = cc.offset_bucket[b as usize] + extra * 32;
    if dist > 8_388_359 {
        c += 12;
    }
    c
}

fn count_bits(val: u32, nbits: i32) -> i32 {
    let v = (val >> nbits) as i32;
    (bit_len((v + 1) as u32) - 1) * 2 + 1 + nbits
}

pub(super) fn cost_len(cc: &CodeCosts, v: i32) -> i32 {
    if v < 255 {
        cc.length[v as usize]
    } else {
        cc.length[255] + count_bits((v - 255) as u32, 6) * 32
    }
}

pub(super) fn cost_lo_match(cc: &CodeCosts, lit_field: i32, ml: i32, loi: i32) -> i32 {
    let num = ml - 17;
    if num < 0 {
        cc.packet[(lit_field + (ml - 2) * 4 + loi * 64) as usize]
    } else {
        cost_len(cc, num) + cc.packet[(lit_field + 60 + loi * 64) as usize]
    }
}

pub(super) fn cost_normal_match(cc: &CodeCosts, lit_field: i32, ml: i32) -> i32 {
    let num = ml - 17;
    if num < 0 {
        cc.packet[(lit_field + (ml - 2) * 4 + 192) as usize]
    } else {
        cost_len(cc, num) + cc.packet[(lit_field + 252) as usize]
    }
}

/// The MSB-first bit writer of the offset and length streams (`KrakenBitWriter`).
#[derive(Default)]
pub(super) struct BitWriter {
    bytes: Vec<u8>,
    cur: u32,
    cur_bits: i32,
}

impl BitWriter {
    fn write_bit(&mut self, bit: u32) {
        self.cur = (self.cur << 1) | (bit & 1);
        self.cur_bits += 1;
        if self.cur_bits == 8 {
            self.bytes.push(self.cur as u8);
            self.cur = 0;
            self.cur_bits = 0;
        }
    }

    fn write_bits(&mut self, value: u32, n: i32) {
        let mut i = n - 1;
        while i >= 0 {
            self.write_bit((value >> i) & 1);
            i -= 1;
        }
    }

    pub fn write_length(&mut self, value: u32) {
        let v = value + 64;
        let n = 32 - v.leading_zeros() as i32;
        for _ in 0..n - 7 {
            self.write_bit(0);
        }
        self.write_bits(v, n);
    }

    /// Writes the extra bits of `distance`; returns its bucket byte.
    pub fn write_distance(&mut self, distance: i32) -> u8 {
        let num = (distance + 248) as u32;
        let num2 = 31 - num.leading_zeros() as i32 - 8;
        let num3 = (num as i32) - (1 << (num2 + 8));
        let low = (num3 & 0xf) as u32;
        self.write_bits((num3 as u32) >> 4, num2 + 4);
        ((num2 << 4) as u32 | low) as u8
    }

    pub fn into_bytes(mut self) -> Vec<u8> {
        if self.cur_bits > 0 {
            self.bytes.push((self.cur << (8 - self.cur_bits)) as u8);
        }
        self.bytes
    }
}
