//! APK Signature Scheme v2 (https://source.android.com/docs/security/features/apksigning/v2)
//! with ECDSA P-256 / SHA-256 (signature algorithm 0x0201, supported since API 24; the app's
//! minSdk is 26, so no v1/JAR signature is needed), and the per-machine export key with its
//! self-signed X.509 certificate.

use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding};
use p256::SecretKey;
use sha2::{Digest, Sha256};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

const SIG_ECDSA_SHA256: u32 = 0x0201;
const V2_BLOCK_ID: u32 = 0x7109_871a;
/// apksigner's padding pair (keeps the signing block a multiple of 4096 bytes).
const PADDING_BLOCK_ID: u32 = 0x4272_6577;
const MAGIC: &[u8; 16] = b"APK Sig Block 42";
const CHUNK: usize = 1 << 20;

/// File names in the key directory.
pub const KEY_FILE: &str = "android-export.key";
pub const CERT_FILE: &str = "android-export.cer";

/// The signing key and its certificate (DER).
pub struct Key {
    signing: SigningKey,
    pub cert: Vec<u8>,
}

impl Key {
    /// A new random key.
    pub fn generate() -> Key {
        Key::from_secret(&SecretKey::random(&mut rand_core::OsRng))
    }

    fn from_secret(secret: &SecretKey) -> Key {
        let signing = SigningKey::from(secret);
        let cert = certificate(&signing);
        Key { signing, cert }
    }

    /// The key kept in `dir` (`android-export.key`, PKCS#8 PEM, and its certificate
    /// `android-export.cer`, DER), created on first use. The same key on every export lets
    /// a re-exported APK install as an update.
    pub fn load_or_create(dir: &Path, log: &dyn Fn(&str)) -> Result<Key, String> {
        let kp = dir.join(KEY_FILE);
        let cp = dir.join(CERT_FILE);
        if kp.is_file() {
            let pem = std::fs::read_to_string(&kp).map_err(|e| format!("{}: {e}", kp.display()))?;
            let secret = SecretKey::from_pkcs8_pem(&pem).map_err(|e| format!("{}: not a P-256 PKCS#8 key: {e}", kp.display()))?;
            let mut key = Key::from_secret(&secret);
            match std::fs::read(&cp) {
                Ok(cert) => key.cert = cert,
                Err(_) => std::fs::write(&cp, &key.cert).map_err(|e| format!("{}: {e}", cp.display()))?,
            }
            return Ok(key);
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let secret = SecretKey::random(&mut rand_core::OsRng);
        let pem = secret.to_pkcs8_pem(LineEnding::LF).map_err(|e| format!("encoding the key: {e}"))?;
        write_private(&kp, pem.as_bytes()).map_err(|e| format!("{}: {e}", kp.display()))?;
        let key = Key::from_secret(&secret);
        std::fs::write(&cp, &key.cert).map_err(|e| format!("{}: {e}", cp.display()))?;
        log(&format!("Created the Android signing key {} (keep it: updates of the installed app need the same key)", kp.display()));
        Ok(key)
    }

    fn public_key_der(&self) -> Vec<u8> {
        self.signing.verifying_key().to_public_key_der().expect("P-256 public key").as_bytes().to_vec()
    }

    fn sign(&self, data: &[u8]) -> Vec<u8> {
        let sig: Signature = self.signing.sign(data);
        sig.to_der().as_bytes().to_vec()
    }
}

fn write_private(p: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(p)?.write_all(bytes)
}

// --- DER ------------------------------------------------------------------------------

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = content.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes: Vec<u8> = n.to_be_bytes().iter().copied().skip_while(|&b| b == 0).collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend_from_slice(&bytes);
    }
    out.extend_from_slice(content);
    out
}

fn seq(parts: &[&[u8]]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}

/// A self-signed X.509 v3 certificate for the key: CN "Frogger 2: Swampy's Revenge (Rust) export",
/// valid 2026..2125, serial from the public key. Deterministic in the key (ECDSA signing is
/// RFC 6979), so a lost certificate file can be recreated identically.
fn certificate(signing: &SigningKey) -> Vec<u8> {
    let spki = signing.verifying_key().to_public_key_der().expect("P-256 public key").as_bytes().to_vec();
    let mut serial = Sha256::digest(&spki)[..16].to_vec();
    serial[0] = (serial[0] & 0x7f) | 0x40;
    let alg = seq(&[&tlv(0x06, &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02])]); // ecdsa-with-SHA256
    let cn = "Frogger 2: Swampy's Revenge (Rust) export";
    let name = seq(&[&tlv(0x31, &seq(&[&tlv(0x06, &[0x55, 0x04, 0x03]), &tlv(0x0c, cn.as_bytes())]))]);
    let validity = seq(&[&tlv(0x17, b"260101000000Z"), &tlv(0x18, b"21251231235959Z")]);
    let tbs = seq(&[&tlv(0xa0, &tlv(0x02, &[2])), &tlv(0x02, &serial), &alg, &name, &validity, &name, &spki]);
    let sig: Signature = signing.sign(&tbs);
    let mut bits = vec![0u8];
    bits.extend_from_slice(sig.to_der().as_bytes());
    seq(&[&tbs, &alg, &tlv(0x03, &bits)])
}

// --- v2 -------------------------------------------------------------------------------

fn lp(b: &[u8]) -> Vec<u8> {
    let mut out = (b.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(b);
    out
}

/// The v2 content digest: SHA-256 over 1 MiB chunks of each section, then over the chunk
/// digests.
struct ChunkDigests {
    digests: Vec<u8>,
    count: u32,
}

impl ChunkDigests {
    fn add_chunk(&mut self, chunk: &[u8]) {
        let mut h = Sha256::new();
        h.update([0xa5]);
        h.update((chunk.len() as u32).to_le_bytes());
        h.update(chunk);
        self.digests.extend_from_slice(&h.finalize());
        self.count += 1;
    }

    fn add_section(&mut self, data: &[u8]) {
        for c in data.chunks(CHUNK) {
            self.add_chunk(c);
        }
    }

    fn finish(&self) -> Vec<u8> {
        let mut h = Sha256::new();
        h.update([0x5a]);
        h.update(self.count.to_le_bytes());
        h.update(&self.digests);
        h.finalize().to_vec()
    }
}

/// The APK Signing Block holding one v2 signer.
fn signing_block(digest: &[u8], key: &Key) -> Vec<u8> {
    let digests = lp(&lp(&[&SIG_ECDSA_SHA256.to_le_bytes()[..], &lp(digest)].concat()));
    let certs = lp(&lp(&key.cert));
    let attrs = lp(&[]);
    let signed_data = [digests, certs, attrs].concat();
    let signature = key.sign(&signed_data);
    let signatures = lp(&lp(&[&SIG_ECDSA_SHA256.to_le_bytes()[..], &lp(&signature)].concat()));
    let signer = [lp(&signed_data), signatures, lp(&key.public_key_der())].concat();
    let v2 = lp(&lp(&signer));

    let pair = |id: u32, value: &[u8]| -> Vec<u8> { [&((value.len() + 4) as u64).to_le_bytes()[..], &id.to_le_bytes(), value].concat() };
    let mut pairs = pair(V2_BLOCK_ID, &v2);
    let total = 8 + pairs.len() + 8 + 16;
    if total % 4096 != 0 {
        let mut pad = 4096 - total % 4096;
        if pad < 12 {
            pad += 4096;
        }
        pairs.extend(pair(PADDING_BLOCK_ID, &vec![0; pad - 12]));
    }
    let size = (pairs.len() + 8 + 16) as u64;
    [&size.to_le_bytes()[..], &pairs, &size.to_le_bytes(), MAGIC].concat()
}

/// Signs a zip whose entries occupy `file[..entries_end]` (nothing after them yet): computes
/// the v2 digest over the entries, `cd` and `eocd` (whose central-directory offset must be
/// `entries_end`), then writes the signing block, the central directory and the end record
/// (now pointing past the block).
pub fn sign_v2<F: Read + Write + Seek>(file: &mut F, entries_end: u64, cd: &[u8], eocd: &[u8], key: &Key) -> io::Result<()> {
    let mut d = ChunkDigests { digests: Vec::new(), count: 0 };
    file.seek(SeekFrom::Start(0))?;
    let mut buf = vec![0u8; CHUNK];
    let mut left = entries_end;
    while left > 0 {
        let n = (CHUNK as u64).min(left) as usize;
        file.read_exact(&mut buf[..n])?;
        d.add_chunk(&buf[..n]);
        left -= n as u64;
    }
    d.add_section(cd);
    let mut eocd = eocd.to_vec();
    eocd[16..20].copy_from_slice(&(entries_end as u32).to_le_bytes());
    d.add_section(&eocd);
    let block = signing_block(&d.finish(), key);
    let cd_offset = entries_end + block.len() as u64;
    let cd_offset = u32::try_from(cd_offset).map_err(|_| io::Error::other("APK larger than 4 GiB"))?;
    eocd[16..20].copy_from_slice(&cd_offset.to_le_bytes());
    file.seek(SeekFrom::Start(entries_end))?;
    file.write_all(&block)?;
    file.write_all(cd)?;
    file.write_all(&eocd)?;
    file.flush()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::android::zip::{Archive, Writer};
    use std::io::Cursor;

    fn u32le(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }

    /// Reads a length-prefixed field at `*p`.
    fn take<'a>(b: &'a [u8], p: &mut usize) -> &'a [u8] {
        let n = u32le(b, *p) as usize;
        let v = &b[*p + 4..*p + 4 + n];
        *p += 4 + n;
        v
    }

    pub(crate) fn signed_zip(key: &Key) -> Vec<u8> {
        let mut w = Writer::new(Cursor::new(Vec::new()));
        w.add_stored("AndroidManifest.xml", b"<manifest/>").unwrap();
        w.add_stored("assets/big.bin", &vec![9u8; (CHUNK * 2) + 17]).unwrap();
        let (mut out, end, cd, eocd) = w.finish().unwrap();
        sign_v2(&mut out, end, &cd, &eocd, key).unwrap();
        out.into_inner()
    }

    /// Verifies like apksigner (structure, digest, signature, certificate key) and returns
    /// the signed APK's entries.
    pub(crate) fn verify(apk: &[u8]) -> Archive {
        use p256::ecdsa::signature::Verifier;
        use p256::pkcs8::DecodePublicKey;
        let n = apk.len();
        let eocd = (0..=n - 22).rev().find(|&i| u32le(apk, i) == 0x0605_4b50).unwrap();
        let cd_off = u32le(apk, eocd + 16) as usize;
        assert_eq!(&apk[cd_off - 16..cd_off], MAGIC);
        let size = u64::from_le_bytes(apk[cd_off - 24..cd_off - 16].try_into().unwrap()) as usize;
        let start = cd_off - size - 8;
        assert_eq!(u64::from_le_bytes(apk[start..start + 8].try_into().unwrap()) as usize, size);
        assert_eq!((size + 8) % 4096, 0, "padded to 4096");
        // Pairs.
        let mut p = start + 8;
        let mut v2 = None;
        while p < cd_off - 24 {
            let len = u64::from_le_bytes(apk[p..p + 8].try_into().unwrap()) as usize;
            if u32le(apk, p + 8) == V2_BLOCK_ID {
                v2 = Some(&apk[p + 12..p + 8 + len]);
            }
            p += 8 + len;
        }
        assert_eq!(p, cd_off - 24);
        let v2 = v2.expect("v2 block");
        let mut q = 0;
        let signers = take(v2, &mut q);
        let mut q = 0;
        let signer = take(signers, &mut q);
        let mut q = 0;
        let signed_data = take(signer, &mut q);
        let sigs = take(signer, &mut q);
        let pubkey = take(signer, &mut q);
        let vk = p256::ecdsa::VerifyingKey::from_public_key_der(pubkey).unwrap();
        let mut s = 0;
        let sig = take(sigs, &mut s);
        assert_eq!(u32le(sig, 0), SIG_ECDSA_SHA256);
        let mut s = 4;
        let sig = p256::ecdsa::Signature::from_der(take(sig, &mut s)).unwrap();
        vk.verify(signed_data, &sig).expect("signature");
        let mut r = 0;
        let digests = take(signed_data, &mut r);
        let certs = take(signed_data, &mut r);
        let mut s = 0;
        let dg = take(digests, &mut s);
        assert_eq!(u32le(dg, 0), SIG_ECDSA_SHA256);
        let mut s = 4;
        let digest = take(dg, &mut s);
        // Recompute the content digest.
        let mut d = ChunkDigests { digests: Vec::new(), count: 0 };
        d.add_section(&apk[..start]);
        d.add_section(&apk[cd_off..eocd]);
        let mut e = apk[eocd..].to_vec();
        e[16..20].copy_from_slice(&(start as u32).to_le_bytes());
        d.add_section(&e);
        assert_eq!(digest, &d.finish()[..]);
        // The certificate carries the signer's public key.
        let mut s = 0;
        let cert = take(certs, &mut s);
        assert!(cert.windows(pubkey.len()).any(|w| w == pubkey));
        Archive::parse(apk.to_vec()).unwrap()
    }

    #[test]
    fn v2_signature_verifies() {
        let key = Key::generate();
        let apk = signed_zip(&key);
        let a = verify(&apk);
        assert_eq!(a.entries.len(), 2);
        assert_eq!(a.read(a.find("AndroidManifest.xml").unwrap()).unwrap(), b"<manifest/>");
        // A flipped byte in the entries breaks the digest.
        let mut bad = apk.clone();
        bad[40] ^= 1;
        assert!(std::panic::catch_unwind(|| verify(&bad)).is_err());
    }

    #[test]
    fn key_is_kept() {
        let dir = std::env::temp_dir().join(format!("f2-android-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Key::load_or_create(&dir, &|_| {}).unwrap();
        let b = Key::load_or_create(&dir, &|_| {}).unwrap();
        assert_eq!(a.cert, b.cert);
        assert_eq!(a.public_key_der(), b.public_key_der());
        // A lost certificate is recreated identically.
        std::fs::remove_file(dir.join(CERT_FILE)).unwrap();
        let c = Key::load_or_create(&dir, &|_| {}).unwrap();
        assert_eq!(a.cert, c.cert);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
