//! Key material of the PS4 fake-package (fPKG) format. It is not stored in this repository:
//! it is read from OpenOrbis' LibOrbisPkg `Util/Keys.cs` (LGPL-2.1), which every homebrew
//! package tool uses. The first PS4 export downloads that one file (pinned to a commit and
//! checked against its SHA-256) into the keys folder. `F2_PS4_KEYS_FILE` (or `frogger
//! --ps4-keys`) names a local copy instead (no download, no hash check); `F2_PS4_KEYS_URL`
//! downloads from a mirror (the SHA-256 still has to match).

#[cfg(not(test))]
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The pinned upstream file.
#[cfg(not(test))]
const KEYS_URL: &str = "https://raw.githubusercontent.com/OpenOrbis/LibOrbisPkg/a1324969b77c77fb8c52b12bf19d9e8d9fbc3b49/LibOrbisPkg/Util/Keys.cs";
#[cfg(not(test))]
const KEYS_SHA256: &str = "48b44dd03e653a5c0991b0a2975477802ea2639c4e6ea3270b2ff20a1856f8a7";
#[cfg(not(test))]
const CACHE_NAME: &str = "LibOrbisPkg-Keys.cs";

/// What writing a package needs (names after LibOrbisPkg's `Keys.cs`).
pub(crate) struct Keyset {
    /// Modulus of the "fake" keyset the IMAGE_KEY entry (the EKPFS) is encrypted with.
    pub fake_modulus: [u8; 256],
    /// Modulus and private exponent of the debug RIF keyset (license.dat signature).
    pub debug_rif_modulus: [u8; 256],
    pub debug_rif_private_exponent: [u8; 256],
    /// AES key of the license.dat secret (debug RIF).
    pub rif_debug_key: [u8; 16],
    /// Keystone: key of the passcode fingerprint HMAC, and key of the final HMAC.
    pub keystone_hmac_key: [u8; 32],
    pub keystone_mac_data: [u8; 32],
    /// Moduli of the 7 package keys: ENTRY_KEYS slot i is encrypted with key i; key 3 also
    /// signs the header.
    pub pkg_public_keys: Vec<[u8; 256]>,
}

static KEYS: OnceLock<Keyset> = OnceLock::new();

/// The loaded keys (`ensure` ran first; tests use a throwaway set).
pub(crate) fn get() -> &'static Keyset {
    #[cfg(test)]
    return KEYS.get_or_init(test_keyset);
    #[cfg(not(test))]
    KEYS.get().expect("PS4 keys not loaded (keys::ensure)")
}

/// Loads the keys once: `F2_PS4_KEYS_FILE`, else the cached download in `dir` (a temp folder
/// without one), downloading it first if missing.
pub(crate) fn ensure(dir: Option<&std::path::Path>, log: &dyn Fn(&str)) -> Result<(), String> {
    #[cfg(test)]
    {
        let _ = (dir, log);
        get();
        Ok(())
    }
    #[cfg(not(test))]
    {
        if KEYS.get().is_some() {
            return Ok(());
        }
        let text = match std::env::var_os("F2_PS4_KEYS_FILE") {
            Some(p) => std::fs::read_to_string(&p).map_err(|e| format!("F2_PS4_KEYS_FILE {}: {e}", PathBuf::from(p).display()))?,
            None => cached_or_download(dir.map(Path::to_path_buf).unwrap_or_else(|| std::env::temp_dir().join("f2-ps4-keys")), log)?,
        };
        let _ = KEYS.set(parse(&text)?);
        Ok(())
    }
}

#[cfg(not(test))]
fn cached_or_download(dir: PathBuf, log: &dyn Fn(&str)) -> Result<String, String> {
    let path = dir.join(CACHE_NAME);
    if let Ok(t) = std::fs::read(&path) {
        if sha256_hex(&t) == KEYS_SHA256 {
            return String::from_utf8(t).map_err(|e| e.to_string());
        }
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let url = std::env::var("F2_PS4_KEYS_URL").unwrap_or_else(|_| KEYS_URL.to_string());
    log(&format!("Downloading the PS4 package keys from LibOrbisPkg: {url}"));
    let part = dir.join(format!("{CACHE_NAME}.part"));
    let hint = format!("download {url} by hand (or a copy of LibOrbisPkg's Util/Keys.cs) and pass it with --ps4-keys FILE or F2_PS4_KEYS_FILE, or set F2_PS4_KEYS_URL to a mirror");
    let status = quiet(std::process::Command::new("curl"))
        .args(["-fL", "--retry", "3", "-o"])
        .arg(&part)
        .arg(&url)
        .status()
        .map_err(|e| format!("running curl: {e} ({hint})"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(format!("downloading {url} failed ({status}); {hint}"));
    }
    let bytes = std::fs::read(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let got = sha256_hex(&bytes);
    if got != KEYS_SHA256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the downloaded keys file has SHA-256 {got}, expected {KEYS_SHA256}; {hint}"));
    }
    std::fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

#[cfg(not(test))]
fn sha256_hex(data: &[u8]) -> String {
    super::crypto::sha256(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads the constants out of LibOrbisPkg's `Keys.cs` (C# byte-array initialisers).
pub(crate) fn parse(text: &str) -> Result<Keyset, String> {
    let text: String = text.lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    let section = |name: &str| -> Result<&str, String> {
        let at = text.find(name).ok_or_else(|| format!("Keys.cs: {name} not found"))?;
        let rest = &text[at..];
        Ok(&rest[..rest[name.len()..].find("public static").map_or(rest.len(), |e| e + name.len())])
    };
    let fake = section("FakeKeyset")?;
    let rif = section("DebugRifKeyset")?;
    let pkg = section("PkgPublicKeys")?;
    let mut keys = Vec::new();
    let (mut depth, mut cur) = (0, None::<usize>);
    for (i, c) in pkg.char_indices() {
        match c {
            '{' => {
                depth += 1;
                if depth == 2 {
                    cur = Some(i + 1);
                }
            }
            '}' => {
                if depth == 2 {
                    keys.push(fixed::<256>(&bytes_of(&pkg[cur.take().unwrap()..i])?, "PkgPublicKeys entry")?);
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    if keys.len() != 7 {
        return Err(format!("Keys.cs: {} PkgPublicKeys entries, expected 7", keys.len()));
    }
    Ok(Keyset {
        fake_modulus: fixed(&block(fake, "Modulus")?, "FakeKeyset.Modulus")?,
        debug_rif_modulus: fixed(&block(rif, "Modulus")?, "DebugRifKeyset.Modulus")?,
        debug_rif_private_exponent: fixed(&block(rif, "PrivateExponent")?, "DebugRifKeyset.PrivateExponent")?,
        rif_debug_key: fixed(&block(&text, "rif_debug_key")?, "rif_debug_key")?,
        keystone_hmac_key: fixed(&block(&text, "keystone_hmac_key")?, "keystone_hmac_key")?,
        keystone_mac_data: fixed(&block(&text, "keystone_mac_data")?, "keystone_mac_data")?,
        pkg_public_keys: keys,
    })
}

/// The bytes of the `{ ... }` initialiser following `name = new byte[...]`.
fn block(text: &str, name: &str) -> Result<Vec<u8>, String> {
    let mut from = 0;
    while let Some(at) = text[from..].find(name).map(|i| i + from) {
        let rest = &text[at + name.len()..];
        if rest.trim_start().starts_with("= new byte") {
            let open = rest.find('{').ok_or_else(|| format!("Keys.cs: {name} has no initialiser"))?;
            let close = rest[open..].find('}').ok_or_else(|| format!("Keys.cs: {name} initialiser not closed"))?;
            return bytes_of(&rest[open + 1..open + close]);
        }
        from = at + name.len();
    }
    Err(format!("Keys.cs: {name} not found"))
}

fn bytes_of(list: &str) -> Result<Vec<u8>, String> {
    list.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| u8::from_str_radix(t.trim_start_matches("0x").trim_start_matches("0X"), 16).map_err(|_| format!("Keys.cs: bad byte {t:?}")))
        .collect()
}

fn fixed<const N: usize>(v: &[u8], what: &str) -> Result<[u8; N], String> {
    v.try_into().map_err(|_| format!("Keys.cs: {what} has {} bytes, expected {N}", v.len()))
}

/// A throwaway key set for tests: a random RSA-2048 pair as the debug RIF key, filler
/// elsewhere. Nothing here is a real key.
#[cfg(test)]
fn test_keyset() -> Keyset {
    fn hex<const N: usize>(s: &str) -> [u8; N] {
        let v: Vec<u8> = (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect();
        v.try_into().unwrap()
    }
    let filler = |seed: u8| -> [u8; 256] {
        let mut m = [0u8; 256];
        for (i, b) in m.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(131).wrapping_add(seed);
        }
        m[0] |= 0x80;
        m[255] |= 1;
        m
    };
    Keyset {
        fake_modulus: filler(1),
        debug_rif_modulus: hex("94d25ccd3c93bd91723a274d1aeb0cfaeef20459fdc1b255d0810305dc5a04a2b3014b7090e28e9bacfe3f62e8d9098db7e41fcbb5667dee988db4ae3f50b965f92a8e7169788d3ae45752da6a9b9f95b3947dbf570636f34133d830af24fca19ae24c3450adededa9f269e9920ffec5bbea61d33dfcc1327d8aa3ad929fd3e0ff27e01ade0066e0c8bf60b6b55bc003e0cdb9317de90f47435ffc5e6c231def272125aa0c7c5838ec545a73951cef2f3825be8bdfd484282149451fef56ccf5d1cb654652823699074ab96b421275de158c4039101a476a69c0f14a48ee2bed78ccc66c88ae7493b06b65c6bd422b2c69d9ba1acd218b0dd8d0651bcc4dd259"),
        debug_rif_private_exponent: hex("3bea93cd5799bdc53ca178283d71abd8ad7207d979664e17e9a9bf5f512f76d5504f71a180db8e01d43ccaa5564ce76144cb96178c5196db04556d0c885646466d723a4623ea12a646bf778e93445394506834d65fc193470d95c3711a5ac0518a0b9915a8f3d5aa8d75193a1e01cd97ab87fa59b387bafaca7b92597402602985339f743536ab028b76cba72c2c2d2391a7f1a1653df35977b8f26a872ef66bc704fcb226a157708800ec7cf302787376ea3b0e03650a6d7df119de4414d34d6acb5536de80779aa159b7cf591ae32f87cf8455ad919f0739ec3c52204e2f100f5c1018766b71b49cabe914138a9f256901525506420cedaa1422d5f757288f"),
        rif_debug_key: [7; 16],
        keystone_hmac_key: [8; 32],
        keystone_mac_data: [9; 32],
        pkg_public_keys: (0..7).map(|i| filler(10 + i)).collect(),
    }
}

/// Keeps `curl` from opening a console window on Windows (the launcher and the game have none).
#[cfg(not(test))]
#[cfg_attr(not(windows), allow(unused_mut))]
fn quiet(mut cmd: std::process::Command) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs_array(name: &str, v: &[u8]) -> String {
        let body: Vec<String> = v.iter().map(|b| format!("0x{b:02X}")).collect();
        format!("    {name} = new byte[{}]\n    {{\n      {}\n    }},\n", v.len(), body.join(", "))
    }

    /// The parser against a file laid out like LibOrbisPkg's `Keys.cs`.
    #[test]
    fn parses_keys_cs_layout() {
        let k = test_keyset();
        let mut t = String::from("namespace X {\n  // d\n  public class RSAKeyset {\n  public static RSAKeyset FakeKeyset = new RSAKeyset\n  {\n");
        t += &cs_array("Modulus", &k.fake_modulus);
        t += "  };\n  public static RSAKeyset DebugRifKeyset = new RSAKeyset\n  {\n";
        t += &cs_array("PrivateExponent", &k.debug_rif_private_exponent);
        t += &cs_array("Modulus", &k.debug_rif_modulus);
        t += "  };\n  }\n  public static class Keys\n  {\n";
        t += &cs_array("public static byte[] rif_debug_key", &k.rif_debug_key);
        t += &cs_array("public static byte[] keystone_hmac_key", &k.keystone_hmac_key);
        t += &cs_array("public static byte[] keystone_mac_data", &k.keystone_mac_data);
        t += "    public static byte[][] PkgPublicKeys = new byte[][]\n    {\n      // Used to encrypt Passcode\n";
        for m in &k.pkg_public_keys {
            let body: Vec<String> = m.iter().map(|b| format!("0x{b:02x}")).collect();
            t += &format!("      new byte[]\n      {{\n        {}\n      }},\n", body.join(", "));
        }
        t += "    };\n  }\n}\n";
        let p = parse(&t).unwrap();
        assert_eq!(p.fake_modulus, k.fake_modulus);
        assert_eq!(p.debug_rif_modulus, k.debug_rif_modulus);
        assert_eq!(p.debug_rif_private_exponent, k.debug_rif_private_exponent);
        assert_eq!((p.rif_debug_key, p.keystone_hmac_key, p.keystone_mac_data), (k.rif_debug_key, k.keystone_hmac_key, k.keystone_mac_data));
        assert_eq!(p.pkg_public_keys, k.pkg_public_keys);
        assert!(parse("nothing").is_err());
    }

    /// The real upstream file: `F2_PS4_KEYS_FILE=Keys.cs cargo test -p f2-package real_keys_file -- --ignored`.
    #[test]
    #[ignore]
    fn real_keys_file_parses() {
        let t = std::fs::read_to_string(std::env::var("F2_PS4_KEYS_FILE").unwrap()).unwrap();
        assert_eq!(parse(&t).unwrap().pkg_public_keys.len(), 7);
    }
}
