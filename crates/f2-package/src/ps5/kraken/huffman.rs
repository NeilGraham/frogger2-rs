//! Kraken's Huffman-coded byte arrays (`KrakenHuffmanArrayEncoder`): length-limited (11 bit)
//! canonical codes, the simple or "complex" code-length header, three interleaved LSB-first
//! bit streams.

pub(super) struct CostInputs {
    pub code_length_bits: i64,
    pub non_zero_symbols: i32,
    pub alphabet_runs: i32,
    pub header_bytes: i32,
}

#[allow(clippy::large_enum_variant)] // short-lived, one per chunk
pub(super) enum Prepared {
    Single(u8),
    Coded { cost: CostInputs, len_of_sym: [u8; 256], reversed_code: [u16; 256], header: Vec<u8> },
}

const MAX_CODE_LEN: i32 = 11;
const MAX_ARRAY_PAYLOAD: usize = 262_143;

/// An array of `length` copies of `symbol` (8 bytes).
pub(super) fn single_symbol_array(symbol: u8, length: usize) -> Vec<u8> {
    let (mut a, off) = entropy_array(2, 3, length);
    a[off] = 0;
    a[off + 1] = (symbol >> 2) | 0x40;
    a[off + 2] = symbol << 6;
    a
}

fn entropy_array(chunk_type: u32, src_size: usize, dst_size: usize) -> (Vec<u8>, usize) {
    let mut a = vec![0u8; 5 + src_size];
    let num = dst_size as i32 - 1;
    a[0] = ((chunk_type << 4) as i32 | ((num >> 14) & 0xf)) as u8;
    let num2 = (src_size as u32) | (((num & 0x3fff) as u32) << 18);
    a[1..5].copy_from_slice(&num2.to_be_bytes());
    (a, 5)
}

pub(super) fn prepare(data: &[u8]) -> Option<Prepared> {
    let length = data.len();
    if !(2..=262_143).contains(&length) {
        return None;
    }
    let mut freq = [0i32; 256];
    for &b in data {
        freq[b as usize] += 1;
    }
    let distinct = freq.iter().filter(|&&f| f != 0).count();
    if distinct == 1 {
        let sym = freq.iter().position(|&f| f != 0).unwrap_or(0);
        return Some(Prepared::Single(sym as u8));
    }
    let lens = build_code_lengths(&freq)?;
    let codes = canonical_codes(&lens);
    let mut reversed = [0u16; 256];
    for s in 0..256 {
        if lens[s] != 0 {
            reversed[s] = reverse_bits(codes[s] as i32, lens[s] as i32) as u16;
        }
    }
    let header = if distinct < 5 { simple_header(&lens)? } else { complex_header(&lens)? };
    let (mut bits, mut nz, mut runs, mut in_run) = (0i64, 0i32, 0i32, false);
    for s in 0..256 {
        if lens[s] == 0 {
            in_run = false;
            continue;
        }
        nz += 1;
        if !in_run {
            runs += 1;
            in_run = true;
        }
        bits += freq[s] as i64 * lens[s] as i64;
    }
    let cost = CostInputs { code_length_bits: bits, non_zero_symbols: nz, alphabet_runs: runs, header_bytes: header.len() as i32 };
    Some(Prepared::Coded { cost, len_of_sym: lens, reversed_code: reversed, header })
}

/// LSB-first bit writer.
#[derive(Default)]
struct LsbWriter {
    bytes: Vec<u8>,
    acc: u32,
    nbits: i32,
}

impl LsbWriter {
    fn write(&mut self, v: u32, len: i32) {
        self.acc |= (v & ((1u32 << len) - 1)) << self.nbits;
        self.nbits += len;
        while self.nbits >= 8 {
            self.bytes.push(self.acc as u8);
            self.acc >>= 8;
            self.nbits -= 8;
        }
    }

    fn into_bytes(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.bytes.push(self.acc as u8);
        }
        self.bytes
    }
}

/// MSB-first bit writer of the code-length headers.
#[derive(Default)]
struct MsbWriter {
    bytes: Vec<u8>,
    acc: u32,
    nbits: i32,
}

impl MsbWriter {
    fn write(&mut self, v: u32, len: i32) {
        let mut i = len - 1;
        while i >= 0 {
            self.acc = (self.acc << 1) | ((v >> i) & 1);
            self.nbits += 1;
            if self.nbits == 8 {
                self.bytes.push(self.acc as u8);
                self.acc = 0;
                self.nbits = 0;
            }
            i -= 1;
        }
    }

    fn bit_length(&self) -> usize {
        self.bytes.len() * 8 + self.nbits as usize
    }

    fn into_bytes(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.bytes.push((self.acc << (8 - self.nbits)) as u8);
        }
        self.bytes
    }
}

pub(super) fn finish(data: &[u8], prepared: &Prepared) -> Option<Vec<u8>> {
    let (lens, codes, header) = match prepared {
        Prepared::Single(sym) => return Some(single_symbol_array(*sym, data.len())),
        Prepared::Coded { len_of_sym, reversed_code, header, .. } => (len_of_sym, reversed_code, header),
    };
    let mut w: [LsbWriter; 3] = Default::default();
    for (i, &b) in data.iter().enumerate() {
        w[i % 3].write(codes[b as usize] as u32, lens[b as usize] as i32);
    }
    let [w0, w1, w2] = w;
    let (a0, a1, a2) = (w0.into_bytes(), w1.into_bytes(), w2.into_bytes());
    if a0.len() > 65535 {
        return None;
    }
    let total = header.len() + 2 + a0.len() + a2.len() + a1.len();
    if total > MAX_ARRAY_PAYLOAD {
        return None;
    }
    let (mut out, mut p) = entropy_array(2, total, data.len());
    out[p..p + header.len()].copy_from_slice(header);
    p += header.len();
    out[p] = a0.len() as u8;
    out[p + 1] = (a0.len() >> 8) as u8;
    p += 2;
    out[p..p + a0.len()].copy_from_slice(&a0);
    p += a0.len();
    out[p..p + a2.len()].copy_from_slice(&a2);
    p += a2.len();
    for (j, &b) in a1.iter().rev().enumerate() {
        out[p + j] = b;
    }
    Some(out)
}

pub(super) fn cost_of(prepared: &Prepared) -> Option<&CostInputs> {
    match prepared {
        Prepared::Coded { cost, .. } => Some(cost),
        Prepared::Single(_) => None,
    }
}

fn simple_header(lens: &[u8; 256]) -> Option<Vec<u8>> {
    let mut n = 0;
    let mut max = 0;
    for &l in lens {
        if l != 0 {
            n += 1;
            max = max.max(l as i32);
        }
    }
    if !(2..256).contains(&n) {
        return None;
    }
    let width = bit_width(max - 1);
    if width > 4 {
        return None;
    }
    let mut w = MsbWriter::default();
    w.write(0, 1);
    w.write(0, 1);
    w.write(n as u32, 8);
    w.write(width as u32, 3);
    for (j, &l) in lens.iter().enumerate() {
        if l != 0 {
            w.write(j as u32, 8);
            if width > 0 {
                w.write((l - 1) as u32, width);
            }
        }
    }
    Some(w.into_bytes())
}

fn complex_header(lens: &[u8; 256]) -> Option<Vec<u8>> {
    let first = lens.iter().position(|&l| l != 0)?;
    let mut best: Option<Vec<u8>> = None;
    let mut best_bits = usize::MAX;
    for fb in 0..=3 {
        if let Some(w) = complex_header_for(lens, fb, first == 0) {
            let bits = w.bit_length();
            if bits < best_bits {
                best_bits = bits;
                best = Some(w.into_bytes());
            }
        }
    }
    best
}

fn complex_header_for(lens: &[u8; 256], fb: i32, skip: bool) -> Option<MsbWriter> {
    let limit = 20 >> fb;
    let mut w = MsbWriter::default();
    w.write(0, 1);
    w.write(1, 1);
    w.write(fb as u32, 2);
    w.write(skip as u32, 1);
    let mut avg: i32 = 32;
    let mut last: i32 = 0;
    let mut first = true;
    let mut i = 0usize;
    while i < 256 {
        if lens[i] == 0 {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while end + 1 < 256 && lens[end + 1] != 0 {
            end += 1;
        }
        let run = (end - start + 1) as i32;
        if !(first && skip) {
            write_gamma(&mut w, start as i32 - last + 1);
        }
        first = false;
        write_gamma(&mut w, run + 1);
        for &l in &lens[start..=end] {
            let pred = (avg + 2) >> 2;
            let d = l as i32 - pred;
            let z = ((d << 1) ^ (d >> 31)) as u32;
            let zeros = (z >> fb) as i32;
            if zeros > limit {
                return None;
            }
            for _ in 0..zeros {
                w.write(0, 1);
            }
            w.write(1, 1);
            if fb > 0 {
                w.write(z & ((1u32 << fb) - 1), fb);
            }
            avg = l as i32 + ((3 * avg + 2) >> 2);
        }
        last = end as i32 + 1;
        i = end + 1;
    }
    if last < 256 {
        write_gamma(&mut w, 256 - last + 1);
    }
    Some(w)
}

fn write_gamma(w: &mut MsbWriter, f: i32) {
    let mut n = 0;
    let mut v = f;
    while v > 0 {
        n += 1;
        v >>= 1;
    }
    w.write(f as u32, 2 * n - 2);
}

fn bit_width(mut v: i32) -> i32 {
    let mut n = 0;
    while v > 0 {
        n += 1;
        v >>= 1;
    }
    n
}

fn reverse_bits(mut v: i32, len: i32) -> i32 {
    let mut r = 0;
    for _ in 0..len {
        r = (r << 1) | (v & 1);
        v >>= 1;
    }
    r
}

fn canonical_codes(lens: &[u8; 256]) -> [u16; 256] {
    let mut codes = [0u16; 256];
    let mut code: u32 = 0;
    for i in 1..=MAX_CODE_LEN {
        let step = 1u32 << (11 - i);
        for s in 0..256 {
            if lens[s] as i32 == i {
                codes[s] = (code >> (11 - i)) as u16;
                code += step;
            }
        }
    }
    codes
}

fn build_code_lengths(freq: &[i32; 256]) -> Option<[u8; 256]> {
    let mut out = [0u8; 256];
    let present: Vec<usize> = (0..256).filter(|&i| freq[i] != 0).collect();
    let total: i64 = present.iter().map(|&i| freq[i] as i64).sum();
    let count = present.len();
    match count {
        0 => return None,
        1 => {
            out[present[0]] = 1;
            return Some(out);
        }
        _ => {}
    }
    let scaled = scale_counts(freq, total);
    let mut sym: Vec<i32> = vec![0; count + 1];
    let mut cnt: Vec<i32> = vec![0; count + 1];
    for j in 0..count {
        sym[j] = present[j] as i32;
        cnt[j] = scaled[present[j]] as i32;
    }
    if count <= 32 {
        sort_by_count_small(&mut sym, &mut cnt, count);
    } else {
        stable_sort_by_count(&mut sym, &mut cnt, count);
    }
    let mut depths: Vec<i32> = cnt[..count].to_vec();
    in_place_huffman(&mut depths, count);
    if depths[0] <= MAX_CODE_LEN {
        for k in 0..count {
            out[sym[k] as usize] = depths[k] as u8;
        }
        return validate_kraft(&out, &present).then_some(out);
    }
    // Package-merge, limited to 11 bits.
    let l = MAX_CODE_LEN as usize;
    cnt[count] = i32::MAX;
    let m = 2 * count - 2;
    let mut nodes: Vec<Vec<i32>> = vec![Vec::new(); l + 1];
    let mut weights: Vec<Vec<i64>> = vec![Vec::new(); l + 1];
    let mut lens_used = vec![0usize; l + 1];
    for lvl in 1..=l {
        nodes[lvl] = vec![0; m];
        weights[lvl] = vec![0; m];
    }
    for lvl in 1..=l {
        let (mut si, mut pi, mut n) = (0usize, 0usize, 0usize);
        let prev_len = if lvl >= 2 { lens_used[lvl - 1] } else { 0 };
        while n < m {
            let w = cnt[si] as i64;
            if pi + 1 < prev_len && weights[lvl - 1][pi] + weights[lvl - 1][pi + 1] <= w {
                weights[lvl][n] = weights[lvl - 1][pi] + weights[lvl - 1][pi + 1];
                nodes[lvl][n] = pi as i32 | 0x4000_0000;
                pi += 2;
                n += 1;
                continue;
            }
            if si >= count {
                break;
            }
            weights[lvl][n] = w;
            nodes[lvl][n] = sym[si];
            si += 1;
            n += 1;
        }
        lens_used[lvl] = n;
    }
    let mut take = lens_used[l];
    for lvl in (1..=l).rev() {
        let mut next = 0usize;
        for &node in &nodes[lvl][..take] {
            if node & 0x4000_0000 == 0 {
                out[node as usize] += 1;
            } else {
                next = (node & !0x4000_0000) as usize + 2;
            }
        }
        take = next;
    }
    validate_kraft(&out, &present).then_some(out)
}

fn validate_kraft(lens: &[u8; 256], present: &[usize]) -> bool {
    let mut sum: i64 = 0;
    for &s in present {
        let l = lens[s] as i32;
        if !(1..=MAX_CODE_LEN).contains(&l) {
            return false;
        }
        sum += 1i64 << (11 - l);
    }
    sum == 2048
}

/// `ScaleCounts`: counts scaled into 16 bits when they do not fit (single-precision floats, as
/// the original).
fn scale_counts(freq: &[i32; 256], total: i64) -> [i64; 256] {
    let mut dest = [0i64; 256];
    let (mut max, mut max_i) = (0i64, 0usize);
    for (i, &f) in freq.iter().enumerate() {
        if max < f as i64 {
            max = f as i64;
            max_i = i;
        }
    }
    if max <= 65535 && total <= 65535 {
        for i in 0..256 {
            dest[i] = freq[i] as i64;
        }
        return dest;
    }
    let a = 65535f32 / total as f32;
    let b = 65535f32 / max as f32;
    let scale = if a <= b { a } else { b };
    let mut sum: i64 = 0;
    for k in 0..256 {
        if freq[k] == 0 {
            continue;
        }
        let v = (freq[k] as f32 * scale + 0.5f32) as u32;
        let v = if (v as i64) < 65535 { v } else { 65535 };
        dest[k] = if v < 2 { 1 } else { v as i64 };
        sum += dest[k];
    }
    if 65535 < sum {
        dest[max_i] += 65535 - sum;
    }
    dest
}

fn stable_sort_by_count(sym: &mut [i32], cnt: &mut [i32], n: usize) {
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&x, &y| cnt[x].cmp(&cnt[y]).then(x.cmp(&y)));
    let s: Vec<i32> = idx.iter().map(|&i| sym[i]).collect();
    let c: Vec<i32> = idx.iter().map(|&i| cnt[i]).collect();
    sym[..n].copy_from_slice(&s);
    cnt[..n].copy_from_slice(&c);
}

/// The original's introsort-like small sort (unstable: the exact swap order matters).
fn sort_by_count_small(sym: &mut [i32], cnt: &mut [i32], n: usize) {
    if n < 2 {
        return;
    }
    let swap = |sym: &mut [i32], cnt: &mut [i32], i: usize, j: usize| {
        sym.swap(i, j);
        cnt.swap(i, j);
    };
    let mut depth_limit = 0usize;
    let mut v = n as u32;
    loop {
        v = (v >> 1) + (v >> 2);
        depth_limit += 1;
        if v == 0 {
            break;
        }
    }
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let (mut lo, mut hi, mut len) = (0usize, n - 1, n);
    loop {
        if len > 1 {
            if len == 2 {
                if cnt[hi] < cnt[lo] {
                    swap(sym, cnt, lo, hi);
                }
            } else {
                let mid = lo + (len >> 1);
                median(sym, cnt, lo, mid, hi);
                if len < 5 {
                    if len == 4 {
                        insert3(sym, cnt, lo, mid, hi);
                    }
                } else if stack.len() != depth_limit {
                    let mut i = lo;
                    let mut j = hi;
                    swap(sym, cnt, mid, lo);
                    loop {
                        j -= 1;
                        if cnt[lo] >= cnt[j] {
                            if j <= i {
                                break;
                            }
                            loop {
                                i += 1;
                                if cnt[i] >= cnt[lo] {
                                    break;
                                }
                            }
                            if j <= i {
                                i -= 1;
                                break;
                            }
                            swap(sym, cnt, i, j);
                        }
                    }
                    swap(sym, cnt, lo, i);
                    let pivot = i;
                    let mut right = i;
                    loop {
                        right += 1;
                        if !(right < hi && cnt[pivot] >= cnt[right]) {
                            break;
                        }
                    }
                    let mut left = i as isize;
                    loop {
                        left -= 1;
                        if !((lo as isize) < left && cnt[left as usize] >= cnt[pivot]) {
                            break;
                        }
                    }
                    let left_len = (left - lo as isize + 1) as usize;
                    let right_len = hi - right + 1;
                    if right_len < left_len {
                        stack.push((right, hi));
                        hi = left as usize;
                        len = left_len;
                    } else {
                        stack.push((lo, left as usize));
                        lo = right;
                        len = right_len;
                    }
                    continue;
                } else {
                    heap_sort(sym, cnt, lo, len);
                }
            }
        }
        match stack.pop() {
            Some((a, b)) => {
                lo = a;
                hi = b;
                len = b.wrapping_sub(a).wrapping_add(1);
            }
            None => break,
        }
    }
}

fn median(sym: &mut [i32], cnt: &mut [i32], a: usize, b: usize, c: usize) {
    let swap = |sym: &mut [i32], cnt: &mut [i32], i: usize, j: usize| {
        sym.swap(i, j);
        cnt.swap(i, j);
    };
    if cnt[b] < cnt[a] {
        if cnt[b] < cnt[c] {
            if cnt[a] < cnt[c] {
                swap(sym, cnt, a, b);
            } else {
                rot(sym, cnt, b, a, c);
            }
        } else {
            swap(sym, cnt, a, c);
        }
    } else if cnt[c] < cnt[b] {
        if cnt[a] < cnt[c] {
            swap(sym, cnt, b, c);
        } else {
            rot(sym, cnt, a, b, c);
        }
    }
}

fn rot(sym: &mut [i32], cnt: &mut [i32], x: usize, y: usize, z: usize) {
    sym.swap(x, y);
    cnt.swap(x, y);
    sym.swap(x, z);
    cnt.swap(x, z);
}

fn insert3(sym: &mut [i32], cnt: &mut [i32], f: usize, m: usize, l: usize) {
    let n = f + 1;
    if cnt[m] < cnt[n] {
        if cnt[n] < cnt[l] {
            sym.swap(n, m);
            cnt.swap(n, m);
        } else {
            rot(sym, cnt, m, n, l);
        }
    } else if cnt[n] < cnt[f] {
        sym.swap(f, n);
        cnt.swap(f, n);
    }
}

fn heap_sort(sym: &mut [i32], cnt: &mut [i32], lo: usize, count: usize) {
    let mut i = count as isize / 2 - 1;
    while i >= 0 {
        sift_down(sym, cnt, lo, i as usize, count);
        i -= 1;
    }
    let mut k = count - 1;
    while k >= 1 {
        sym.swap(lo, lo + k);
        cnt.swap(lo, lo + k);
        sift_down(sym, cnt, lo, 0, k);
        k -= 1;
    }
}

fn sift_down(sym: &mut [i32], cnt: &mut [i32], lo: usize, mut node: usize, heap: usize) {
    loop {
        let mut child = node * 2 + 1;
        if child >= heap {
            break;
        }
        let mut abs = lo + child;
        if child + 1 < heap && cnt[abs] < cnt[abs + 1] {
            child += 1;
            abs += 1;
        }
        if cnt[lo + node] >= cnt[abs] {
            break;
        }
        sym.swap(lo + node, abs);
        cnt.swap(lo + node, abs);
        node = child;
    }
}

/// Code lengths in place (the classic in-place Huffman of sorted counts).
fn in_place_huffman(cnt: &mut [i32], n: usize) {
    cnt[0] += cnt[1];
    let (mut root, mut leaf) = (0usize, 2usize);
    for i in 1..n - 1 {
        if leaf < n && cnt[leaf] <= cnt[root] {
            cnt[i] = cnt[leaf];
            leaf += 1;
        } else {
            cnt[i] = cnt[root];
            cnt[root] = i as i32;
            root += 1;
        }
        if leaf < n && (i <= root || cnt[leaf] <= cnt[root]) {
            cnt[i] += cnt[leaf];
            leaf += 1;
        } else {
            cnt[i] += cnt[root];
            cnt[root] = i as i32;
            root += 1;
        }
    }
    cnt[n - 2] = 0;
    let mut j = n as isize - 3;
    while j >= 0 {
        cnt[j as usize] = cnt[cnt[j as usize] as usize] + 1;
        j -= 1;
    }
    let (mut depth, mut avail_i, mut next) = (0i32, n as isize - 2, n as isize - 1);
    let mut avail = 1i32;
    while avail > 0 {
        let mut used = 0;
        while avail_i >= 0 && cnt[avail_i as usize] == depth {
            used += 1;
            avail_i -= 1;
        }
        while used < avail {
            cnt[next as usize] = depth;
            next -= 1;
            avail -= 1;
        }
        depth += 1;
        avail = used << 1;
    }
}
