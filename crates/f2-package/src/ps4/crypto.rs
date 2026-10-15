//! The cryptography of PS4 fake packages: key derivation from the content ID and passcode, the
//! deterministic RSA "key encryption" (Mersenne-Twister padding), PKCS#1 v1.5 signing with the
//! debug RIF key, AES-128-CBC for encrypted entries and AES-128-XTS for the outer PFS image.
//!
//! The algorithms follow LibOrbisPkg (OpenOrbis, LGPL-2.1: `Util/Crypto.cs`,
//! `Util/MersenneTwister.cs`, `Util/XtsBlockTransform.cs`), reimplemented here so packages built
//! by this exporter match what `PkgTool.Core` writes for the same inputs.

use super::keys;
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::{Aes128, Block};
use hmac::{Hmac, Mac};
use num_bigint::BigUint;
use sha2::{Digest, Sha256};

pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub(crate) fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("hmac key");
    m.update(data);
    m.finalize().into_bytes().into()
}

/// `ComputeKeys`: the package's derived key `index` (0..7): SHA-256 of SHA-256(index, BE u32) ||
/// SHA-256(content ID padded to 48 bytes) || passcode. Index 1 is the EKPFS (the PFS key).
pub(crate) fn compute_key(content_id: &str, passcode: &str, index: u32) -> [u8; 32] {
    assert_eq!(content_id.len(), 36, "content ID must be 36 characters");
    assert_eq!(passcode.len(), 32, "passcode must be 32 characters");
    let mut data = Vec::with_capacity(96);
    data.extend(sha256(&index.to_be_bytes()));
    data.extend(sha256(&padded(content_id.as_bytes(), 48)));
    data.extend(passcode.as_bytes());
    sha256(&data)
}

/// `bytes` zero-padded (or cut) to `len`.
pub(crate) fn padded(bytes: &[u8], len: usize) -> Vec<u8> {
    let mut v = bytes.to_vec();
    v.resize(len, 0);
    v
}

/// The keystone file of a game package (`sce_sys/keystone`): header, HMAC fingerprint of the
/// passcode, HMAC of both.
pub(crate) fn keystone(passcode: &str) -> Vec<u8> {
    let mut out = b"keystone\x02\x00\x01\x00".to_vec();
    out.resize(32, 0);
    out.extend(hmac_sha256(&keys::KEYSTONE_HMAC_KEY, passcode.as_bytes()));
    let fin = hmac_sha256(&keys::KEYSTONE_MAC_DATA, &out);
    out.extend(fin);
    out
}

/// `PfsGenCryptoKey`: HMAC-SHA256(ekpfs, index (LE u32) || seed).
fn pfs_crypto_key(ekpfs: &[u8], seed: &[u8], index: u32) -> [u8; 32] {
    let mut d = index.to_le_bytes().to_vec();
    d.extend(seed);
    hmac_sha256(ekpfs, &d)
}

/// The XTS (tweak key, data key) of a PFS image.
pub(crate) fn pfs_enc_keys(ekpfs: &[u8], seed: &[u8]) -> ([u8; 16], [u8; 16]) {
    let k = pfs_crypto_key(ekpfs, seed, 1);
    (k[..16].try_into().unwrap(), k[16..].try_into().unwrap())
}

/// The HMAC key that signs a PFS image's blocks.
pub(crate) fn pfs_sign_key(ekpfs: &[u8], seed: &[u8]) -> [u8; 32] {
    pfs_crypto_key(ekpfs, seed, 2)
}

/// AES-128-CBC encryption without padding (`data.len()` a multiple of 16), in place.
pub(crate) fn aes_cbc_encrypt(data: &mut [u8], key: &[u8], iv: &[u8]) {
    assert!(data.len().is_multiple_of(16), "AES-CBC data must be whole blocks");
    let c = Aes128::new_from_slice(key).expect("aes key");
    let mut prev: [u8; 16] = iv.try_into().expect("iv");
    for chunk in data.as_chunks_mut::<16>().0 {
        for (b, p) in chunk.iter_mut().zip(prev) {
            *b ^= p;
        }
        let blk = Block::from_mut_slice(chunk);
        c.encrypt_block(blk);
        prev.copy_from_slice(chunk);
    }
}

/// AES-128-CBC decryption without padding, in place (the package reader).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn aes_cbc_decrypt(data: &mut [u8], key: &[u8], iv: &[u8]) {
    use aes::cipher::BlockDecrypt;
    let c = Aes128::new_from_slice(key).expect("aes key");
    let mut prev: [u8; 16] = iv.try_into().expect("iv");
    for chunk in data.as_chunks_mut::<16>().0 {
        let ct: [u8; 16] = *chunk;
        c.decrypt_block(Block::from_mut_slice(chunk));
        for (b, p) in chunk.iter_mut().zip(prev) {
            *b ^= p;
        }
        prev = ct;
    }
}

/// AES-128-XTS (XEX) over 0x1000-byte sectors, as the PFS uses it: the tweak is the sector
/// number (LE u64) encrypted with the tweak key, multiplied by x in GF(2^128) per 16-byte block.
pub(crate) struct Xts {
    data: Aes128,
    tweak: Aes128,
}

impl Xts {
    pub(crate) fn new(data_key: &[u8; 16], tweak_key: &[u8; 16]) -> Xts {
        Xts { data: Aes128::new_from_slice(data_key).unwrap(), tweak: Aes128::new_from_slice(tweak_key).unwrap() }
    }

    fn crypt(&self, sector: &mut [u8], num: u64, encrypt: bool) {
        let mut t = [0u8; 16];
        t[..8].copy_from_slice(&num.to_le_bytes());
        let mut tb = Block::from(t);
        self.tweak.encrypt_block(&mut tb);
        let mut tw: [u8; 16] = tb.into();
        for chunk in sector.as_chunks_mut::<16>().0 {
            for (b, k) in chunk.iter_mut().zip(tw) {
                *b ^= k;
            }
            let blk = Block::from_mut_slice(chunk);
            if encrypt {
                self.data.encrypt_block(blk);
            } else {
                aes::cipher::BlockDecrypt::decrypt_block(&self.data, blk);
            }
            for (b, k) in chunk.iter_mut().zip(tw) {
                *b ^= k;
            }
            let mut carry = 0u8;
            for b in tw.iter_mut() {
                let c = *b >> 7;
                *b = (*b << 1) | carry;
                carry = c;
            }
            if carry != 0 {
                tw[0] ^= 0x87;
            }
        }
    }

    pub(crate) fn encrypt_sector(&self, sector: &mut [u8], num: u64) {
        self.crypt(sector, num, true)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn decrypt_sector(&self, sector: &mut [u8], num: u64) {
        self.crypt(sector, num, false)
    }
}

/// MT19937 seeded with `init_by_array`, as `RSA2048EncryptKey` uses it for its padding bytes.
struct Mt {
    mt: [u32; 624],
    i: usize,
}

impl Mt {
    fn new(seed: &[u32]) -> Mt {
        const N: usize = 624;
        let mut mt = [0u32; N];
        mt[0] = 0x12BD6AA;
        for i in 1..N {
            mt[i] = (i as u32).wrapping_add(0x6C078965u32.wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30)));
        }
        let (mut s, mut k) = (1usize, 0usize);
        for _ in 0..N.max(seed.len()) {
            mt[s] = (mt[s] ^ (mt[s - 1] ^ (mt[s - 1] >> 30)).wrapping_mul(0x19660D)).wrapping_add(seed[k]).wrapping_add(k as u32);
            s += 1;
            k += 1;
            if s >= N {
                mt[0] = mt[N - 1];
                s = 1;
            }
            if k >= seed.len() {
                k = 0;
            }
        }
        for _ in 0..N - 1 {
            mt[s] = (mt[s] ^ (mt[s - 1] ^ (mt[s - 1] >> 30)).wrapping_mul(0x5D588B65)).wrapping_sub(s as u32);
            s += 1;
            if s >= N {
                mt[0] = mt[N - 1];
                s = 1;
            }
        }
        mt[0] = 0x8000_0000;
        Mt { mt, i: N }
    }

    fn next(&mut self) -> u32 {
        const N: usize = 624;
        const M: usize = 397;
        let mag = |y: u32| if y & 1 != 0 { 0x9908b0df } else { 0 };
        if self.i >= N {
            for k in 0..N {
                let y = (self.mt[k] & 0x8000_0000) | (self.mt[(k + 1) % N] & 0x7fff_ffff);
                self.mt[k] = self.mt[(k + M) % N] ^ (y >> 1) ^ mag(y);
            }
            self.i = 0;
        }
        let mut y = self.mt[self.i];
        self.i += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c5680;
        y ^= (y << 15) & 0xefc60000;
        y ^= y >> 18;
        y
    }
}

/// `value`^`exp` mod `modulus` (big-endian bytes), as 256 big-endian bytes.
fn modpow(value: &[u8], exp: &[u8], modulus: &[u8]) -> [u8; 256] {
    let r = BigUint::from_bytes_be(value).modpow(&BigUint::from_bytes_be(exp), &BigUint::from_bytes_be(modulus));
    let b = r.to_bytes_be();
    let mut out = [0u8; 256];
    out[256 - b.len()..].copy_from_slice(&b);
    out
}

/// `RSA2048EncryptKey`: RSA-2048 (e = 65537) encryption of a 32-byte value with PKCS#1 v1.5
/// type-2 padding whose random bytes come from a Mersenne Twister seeded with
/// SHA-256(SHA-256(modulus || value)), so the result is deterministic.
pub(crate) fn rsa_encrypt_key(modulus: &[u8; 256], value: &[u8; 32]) -> [u8; 256] {
    let mut buf = modulus.to_vec();
    buf.extend(value);
    let h = sha256(&sha256(&buf));
    let seed: Vec<u32> = h.chunks(4).map(|c| u32::from_be_bytes(c.try_into().unwrap())).collect();
    let mut mt = Mt::new(&seed);
    let mut padded = [0u8; 256];
    padded[1] = 2;
    padded[224..].copy_from_slice(value);
    let mut k = 2;
    while k < 223 {
        let mut src = Vec::with_capacity(48);
        for _ in 0..12 {
            src.extend(mt.next().to_be_bytes());
        }
        for r in sha256(&src) {
            if k >= 223 {
                break;
            }
            if r != 0 {
                padded[k] = r;
                k += 1;
            }
        }
    }
    modpow(&padded, &[0x01, 0x00, 0x01], modulus)
}

/// RSA-2048 PKCS#1 v1.5 signature of a SHA-256 hash with the debug RIF key (license.dat).
pub(crate) fn rsa_sign_sha256_debug_rif(hash: &[u8; 32]) -> [u8; 256] {
    const DIGEST_INFO: [u8; 19] = [0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20];
    let mut em = [0xffu8; 256];
    em[0] = 0;
    em[1] = 1;
    let t = 256 - 32 - DIGEST_INFO.len();
    em[t - 1] = 0;
    em[t..t + DIGEST_INFO.len()].copy_from_slice(&DIGEST_INFO);
    em[256 - 32..].copy_from_slice(hash);
    modpow(&em, &keys::DEBUG_RIF_PRIVATE_EXPONENT, &keys::DEBUG_RIF_MODULUS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mt19937_reference() {
        // init_by_array({0x123, 0x234, 0x345, 0x456}): the reference implementation's first outputs.
        let mut mt = Mt::new(&[0x123, 0x234, 0x345, 0x456]);
        let v: Vec<u32> = (0..3).map(|_| mt.next()).collect();
        assert_eq!(v, [1067595299, 955945823, 477289528]);
    }

    #[test]
    fn license_signature_verifies() {
        let h = sha256(b"x");
        let s = rsa_sign_sha256_debug_rif(&h);
        let back = modpow(&s, &[1, 0, 1], &keys::DEBUG_RIF_MODULUS);
        assert_eq!(&back[..2], &[0, 1]);
        assert_eq!(&back[224..], &h);
    }

    #[test]
    fn cbc_and_xts_round_trip() {
        let mut d: Vec<u8> = (0..64u8).collect();
        aes_cbc_encrypt(&mut d, &[1; 16], &[2; 16]);
        aes_cbc_decrypt(&mut d, &[1; 16], &[2; 16]);
        assert_eq!(d, (0..64u8).collect::<Vec<_>>());
        let x = Xts::new(&[3; 16], &[4; 16]);
        let mut s: Vec<u8> = (0..4096).map(|i| i as u8).collect();
        x.encrypt_sector(&mut s, 77);
        assert_ne!(s[..16], (0..16u8).collect::<Vec<_>>()[..]);
        x.decrypt_sector(&mut s, 77);
        assert_eq!(s, (0..4096).map(|i| i as u8).collect::<Vec<_>>());
    }

    #[test]
    fn keystone_layout() {
        let k = keystone("00000000000000000000000000000000");
        assert_eq!(k.len(), 96);
        assert_eq!(&k[..12], b"keystone\x02\x00\x01\x00");
    }
}
