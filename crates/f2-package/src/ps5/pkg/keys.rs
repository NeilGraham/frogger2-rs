//! Key material of the PS5 fake package. It is not stored in this repository: the first
//! package export downloads five files of LibProsperoPkg (drakmor/LibProsperoPkg, GPL-3.0),
//! pinned to a commit and checked against their SHA-256, into the keys folder:
//! `Util/Keys.cs` (keystone HMAC keys), `PKG/ProsperoNapsMeta.cs` (the `naps_meta_18.dat`
//! AES-XTS keys), `Keys/Data/passcode.bin` (the 7 package RSA moduli),
//! `Keys/Data/mount_image.bin` (the mount-image RSA modulus) and `PlayGo/Data/right.sprx`.
//! `F2_PS5_KEYS_DIR` names a local folder holding those files by base name instead (no
//! download, no hash check); `F2_PS5_KEYS_URL` downloads from a mirror of the repository's
//! `src/LibProsperoPkg/` folder (the hashes still have to match).

use std::path::{Path, PathBuf};

const BASE_URL: &str = "https://raw.githubusercontent.com/drakmor/LibProsperoPkg/be79990357efa6c1a873ae7443dec6e640172914/src/LibProsperoPkg";

/// (path under `src/LibProsperoPkg/`, SHA-256).
const FILES: [(&str, &str); 5] = [
    ("Util/Keys.cs", "b22c0e64bf2269371c0e867145bd0103ca46b36102717c61789b95680991db10"),
    ("PKG/ProsperoNapsMeta.cs", "64d19e6d829598d28470afd8930f86230543044cd02b6f3042306dba0f5e0ac4"),
    ("Keys/Data/passcode.bin", "81e8283d582f522d8da34565ecd6ab39a38b3d3c0cacef078df97d2b285ec24e"),
    ("Keys/Data/mount_image.bin", "4510462812321cea26b4c0155e363955b16e31b89252b92510accad02eca5cd0"),
    ("PlayGo/Data/right.sprx", "acb7d999f31ffc73bbfff703c7866455a89f35f8a7f4feebc152edf40f707d50"),
];

/// What writing a package needs.
pub(crate) struct Keyset {
    pub keystone_hmac_key: [u8; 32],
    pub keystone_mac_data: [u8; 32],
    /// The 7 RSA-3072 moduli (`passcode.bin`): ENTRY_KEYS slot i, slot 3 wraps the header.
    pub passcode_moduli: Vec<[u8; 384]>,
    pub mount_image_modulus: [u8; 384],
    pub meta18_data_key: [u8; 16],
    pub meta18_tweak_key: [u8; 16],
    pub meta18_tweak: [u8; 16],
    pub right_sprx: Vec<u8>,
}

fn base_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap()
}

fn sha256_hex(data: &[u8]) -> String {
    crate::ps4::crypto::sha256(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Loads the key files: `F2_PS5_KEYS_DIR`, else the cached downloads in `dir` (a temp folder
/// without one), downloading what is missing.
pub(crate) fn load(dir: Option<&Path>, log: &dyn Fn(&str)) -> Result<Keyset, String> {
    let mut data = Vec::new();
    match std::env::var_os("F2_PS5_KEYS_DIR") {
        Some(d) => {
            let d = PathBuf::from(d);
            for (p, _) in FILES {
                let f = d.join(base_name(p));
                data.push(std::fs::read(&f).map_err(|e| format!("F2_PS5_KEYS_DIR {}: {e}", f.display()))?);
            }
        }
        None => {
            let dir = dir.map(Path::to_path_buf).unwrap_or_else(|| std::env::temp_dir().join("f2-ps5-keys"));
            for (p, sha) in FILES {
                data.push(cached_or_download(&dir, p, sha, log)?);
            }
        }
    }
    parse(&data)
}

fn cached_or_download(dir: &Path, rel: &str, sha: &str, log: &dyn Fn(&str)) -> Result<Vec<u8>, String> {
    let path = dir.join(format!("LibProsperoPkg-{}", base_name(rel)));
    if let Ok(t) = std::fs::read(&path) {
        if sha256_hex(&t) == sha {
            return Ok(t);
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let base = std::env::var("F2_PS5_KEYS_URL").unwrap_or_else(|_| BASE_URL.to_string());
    let url = format!("{}/{rel}", base.trim_end_matches('/'));
    log(&format!("Downloading PS5 package key material from LibProsperoPkg: {url}"));
    let part = path.with_extension("part");
    let hint = "download LibProsperoPkg's Util/Keys.cs, PKG/ProsperoNapsMeta.cs, Keys/Data/passcode.bin, Keys/Data/mount_image.bin and PlayGo/Data/right.sprx by hand into one folder and set F2_PS5_KEYS_DIR, or set F2_PS5_KEYS_URL to a mirror";
    let mut cmd = std::process::Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let status = cmd.args(["-fsSL", "--retry", "3", "-o"]).arg(&part).arg(&url).status().map_err(|e| format!("running curl: {e} ({hint})"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(format!("downloading {url} failed ({status}); {hint}"));
    }
    let bytes = std::fs::read(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let got = sha256_hex(&bytes);
    if got != sha {
        let _ = std::fs::remove_file(&part);
        return Err(format!("{url} has SHA-256 {got}, expected {sha}; {hint}"));
    }
    std::fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(bytes)
}

/// The bytes of the array initialiser (`{ 0x.., }` or `[0x.., ]`) after `name`'s `=`.
fn array_after(text: &str, name: &str) -> Result<Vec<u8>, String> {
    let mut from = 0;
    while let Some(at) = text[from..].find(name).map(|i| i + from) {
        let rest = &text[at + name.len()..];
        if let Some(r) = rest.trim_start().strip_prefix('=') {
            // `new byte[0x20] { .. }`: skip the size.
            let rest = match r.trim_start().strip_prefix("new") {
                Some(n) => &n[n.find(']').map_or(0, |i| i + 1)..],
                None => r,
            };
            let open = rest.find(['{', '[']).ok_or_else(|| format!("{name}: no initialiser"))?;
            let close_ch = if rest.as_bytes()[open] == b'{' { '}' } else { ']' };
            let close = rest[open..].find(close_ch).ok_or_else(|| format!("{name}: initialiser not closed"))?;
            return rest[open + 1..open + close]
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|t| !t.is_empty())
                .map(|t| {
                    let h = t.trim_start_matches("0x").trim_start_matches("0X");
                    if t.len() != h.len() { u8::from_str_radix(h, 16) } else { t.parse::<u8>() }.map_err(|_| format!("{name}: bad byte {t:?}"))
                })
                .collect();
        }
        from = at + name.len();
    }
    Err(format!("{name} not found"))
}

fn fixed<const N: usize>(v: &[u8], what: &str) -> Result<[u8; N], String> {
    v.try_into().map_err(|_| format!("{what} has {} bytes, expected {N}", v.len()))
}

/// The key set from the five files' bytes (in `FILES` order).
pub(crate) fn parse(data: &[Vec<u8>]) -> Result<Keyset, String> {
    let keys_cs = String::from_utf8_lossy(&data[0]);
    let meta_cs = String::from_utf8_lossy(&data[1]);
    if data[2].len() != 7 * 384 {
        return Err(format!("passcode.bin has {} bytes, expected {}", data[2].len(), 7 * 384));
    }
    Ok(Keyset {
        keystone_hmac_key: fixed(&array_after(&keys_cs, "keystone_hmac_key_ps5")?, "keystone_hmac_key_ps5")?,
        keystone_mac_data: fixed(&array_after(&keys_cs, "keystone_mac_data_ps5")?, "keystone_mac_data_ps5")?,
        passcode_moduli: data[2].chunks(384).map(|c| c.try_into().unwrap()).collect(),
        mount_image_modulus: fixed(&data[3], "mount_image.bin")?,
        meta18_data_key: fixed(&array_after(&meta_cs, "Meta18DataKey")?, "Meta18DataKey")?,
        meta18_tweak_key: fixed(&array_after(&meta_cs, "Meta18TweakKey")?, "Meta18TweakKey")?,
        meta18_tweak: fixed(&array_after(&meta_cs, "Meta18Tweak ")?, "Meta18Tweak")?,
        right_sprx: data[4].clone(),
    })
}

/// A throwaway key set for tests (nothing here is a real key): random-looking odd moduli.
#[cfg(test)]
pub(crate) fn test_keyset() -> Keyset {
    let modulus = |seed: u8| -> [u8; 384] {
        let mut m = [0u8; 384];
        for (i, b) in m.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(131).wrapping_add(seed);
        }
        m[0] |= 0x80;
        m[383] |= 1;
        m
    };
    Keyset {
        keystone_hmac_key: [8; 32],
        keystone_mac_data: [9; 32],
        passcode_moduli: (0..7).map(|i| modulus(10 + i)).collect(),
        mount_image_modulus: modulus(3),
        meta18_data_key: [1; 16],
        meta18_tweak_key: [2; 16],
        meta18_tweak: [3; 16],
        right_sprx: b"right".to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_upstream_layouts() {
        let keys_cs = "public static byte[] keystone_hmac_key_ps5 = new byte[0x20]\n{\n 0x01, 0x02,\n".to_string()
            + &"0x03, ".repeat(30)
            + "\n};\npublic static byte[] keystone_mac_data_ps5 = new byte[0x20]\n{\n"
            + &"0xAB, ".repeat(32)
            + "};";
        let meta_cs = "private static readonly byte[] Meta18DataKey =\n [".to_string()
            + &"0x11, ".repeat(16)
            + "];\nprivate static readonly byte[] Meta18TweakKey =\n ["
            + &"0x22, ".repeat(16)
            + "];\nprivate static readonly byte[] Meta18Tweak =\n [0x3C, 0xBA, 0x10, 0x7D, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];";
        let k = parse(&[keys_cs.into_bytes(), meta_cs.into_bytes(), vec![5; 7 * 384], vec![6; 384], b"sprx".to_vec()]).unwrap();
        assert_eq!(k.keystone_hmac_key[..3], [1, 2, 3]);
        assert_eq!(k.keystone_mac_data, [0xAB; 32]);
        assert_eq!(k.meta18_data_key, [0x11; 16]);
        assert_eq!(k.meta18_tweak_key, [0x22; 16]);
        assert_eq!(k.meta18_tweak[..4], [0x3C, 0xBA, 0x10, 0x7D]);
        assert_eq!(k.passcode_moduli.len(), 7);
        assert!(parse(&[vec![], vec![], vec![], vec![], vec![]]).is_err());
    }
}
