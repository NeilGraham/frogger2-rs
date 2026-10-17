//! The CNT: the package metadata (keys, digests, entry table, `param.json`, artwork, PlayGo
//! files) that follows the outer PFS in the finished package (LibProsperoPkg 1.2.0
//! `ProsperoPkgBuilder.BuildContainer` / `LayOutEntries` / `FinishContainer`, the publisher
//! "nwonly" application profile), plus the publisher `param.json` and PlayGo files.

use super::keys::Keyset;
use super::pfs::{hash_path, sha3};
use crate::ps4::crypto::{hmac_sha256, rsa_pkcs1_deterministic};
use serde_json::{json, Map, Value};

pub(crate) const PASSCODE: &str = "00000000000000000000000000000000";
const BODY_OFFSET: u64 = 0x2000;

/// The publisher form of `sce_sys/param.json` (`BuildPublisherParamJson`, application, build
/// time 0).
pub(crate) fn publisher_param_json(src: &[u8]) -> Result<Vec<u8>, String> {
    let v: Value = serde_json::from_slice(src).map_err(|e| format!("sce_sys/param.json is not valid JSON: {e}"))?;
    let Value::Object(mut o) = v else { return Err("sce_sys/param.json must contain a JSON object".into()) };
    let uri = o.get("versionFileUri").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if uri.len() > 255 {
        return Err("param.json versionFileUri is longer than 255 characters".into());
    }
    o.insert("versionFileUri".into(), format!("{uri:<255}").into());
    let loudness = o.get("pubtools").and_then(|p| p.get("loudnessSnd0")).cloned();
    let mut pubtools = Map::new();
    pubtools.insert("creationDate".into(), "1970-01-01 00:00:00".into());
    pubtools.insert("toolVersion".into(), "2.79".into());
    if let Some(l) = loudness {
        pubtools.insert("loudnessSnd0".into(), l);
    }
    o.insert("pubtools".into(), Value::Object(pubtools));
    let sdk = o
        .get("sdkVersion")
        .and_then(|v| v.as_str())
        .and_then(|s| s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")))
        .and_then(|h| u64::from_str_radix(h, 16).ok())
        .unwrap_or(0);
    o.entry("requiredSystemSoftwareVersion").or_insert_with(|| "0x0000000000000000".into());
    o.insert("sdkVersion".into(), format!("0x{sdk:016x}").into());
    o.entry("applicationDrmType").or_insert_with(|| "standard".into());
    for k in ["attribute", "attribute2", "attribute3"] {
        o.entry(k).or_insert_with(|| 0.into());
    }
    o.entry("conceptId").or_insert_with(|| "10000000".into());
    o.entry("masterVersion").or_insert_with(|| "01.00".into());
    o.entry("contentVersion").or_insert_with(|| "01.000.000".into());
    o.entry("ageLevel").or_insert_with(|| json!({"JP": 0, "US": 0, "default": 0}));
    o.entry("contentBadgeType").or_insert_with(|| 1.into());
    o.remove("originContentVersion");
    o.remove("targetContentVersion");
    o.insert("addcont".into(), json!({ "serviceIdForSharing": vec![" ".repeat(19); 7] }));
    // serde_json's map is ordered by key (ordinal), as `SortJsonNode`; same 2-space layout.
    let text = serde_json::to_string_pretty(&Value::Object(o)).map_err(|e| e.to_string())?;
    Ok((text.replace("\r\n", "\n").replace('\n', "\r\n") + "\r\n").into_bytes())
}

/// (contentVersion, applicationDrmType) of a param.json (`ReadParamJsonInfo`).
pub(crate) fn param_info(param: &[u8]) -> (String, String) {
    let v: Value = serde_json::from_slice(param).unwrap_or(Value::Null);
    let s = |k: &str, d: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or(d).to_string();
    (s("contentVersion", "01.000.000"), s("applicationDrmType", "free"))
}

/// `NormalizePfsContentVersion`: `NN.NNN.NNN`.
pub(crate) fn pfs_content_version(v: &str) -> String {
    let parts: Vec<&str> = v.trim().split('.').collect();
    let num = |s: &str| s.parse::<i64>().ok();
    if !(2..=3).contains(&parts.len()) {
        return "01.000.000".into();
    }
    let (a, b) = (num(parts[0]), num(parts[1]));
    let c = if parts.len() == 3 { num(parts[2]) } else { Some(0) };
    match (a, b, c) {
        (Some(a), Some(b), Some(c)) if (0..=99).contains(&a) && (0..=999).contains(&b) && (0..=999).contains(&c) => format!("{a:02}.{b:03}.{c:03}"),
        _ => "01.000.000".into(),
    }
}

/// `ContentVersionHigh`: the 8 digits of the content version as BCD.
pub(crate) fn content_version_high(v: &str) -> u32 {
    let d: Vec<u8> = v.bytes().filter(u8::is_ascii_digit).collect();
    if d.len() != 8 {
        return 0;
    }
    d.iter().fold(0, |n, c| (n << 4) | (c - b'0') as u32)
}

/// `Crypto.ComputeKeys` with SHA3-256.
pub(crate) fn compute_key(content_id: &str, index: u32) -> [u8; 32] {
    let mut pad = content_id.as_bytes().to_vec();
    pad.resize(48, 0);
    let mut d = sha3(&index.to_be_bytes()).to_vec();
    d.extend(sha3(&pad));
    d.extend(PASSCODE.as_bytes());
    sha3(&d)
}

/// `DeriveDeterministicOuterSeed`.
pub(crate) fn deterministic_seed(content_id: &str) -> [u8; 16] {
    let s = format!("LibProsperoPkg deterministic outer seed\0{content_id}\0{PASSCODE}");
    sha3(s.as_bytes())[..16].try_into().unwrap()
}

/// `DerivePublisherPfsImageKey`: HMAC-SHA256(EKPFS, seed).
pub(crate) fn pfs_image_key(content_id: &str, seed: &[u8; 16]) -> [u8; 32] {
    hmac_sha256(&compute_key(content_id, 1), seed)
}

/// The keystone (`CreateKeystone` version 3).
pub(crate) fn keystone(keys: &Keyset) -> Vec<u8> {
    let mut out = b"keystone\x03\x00\x01".to_vec();
    out.resize(32, 0);
    out.extend(hmac_sha256(&keys.keystone_hmac_key, PASSCODE.as_bytes()));
    let fin = hmac_sha256(&keys.keystone_mac_data, &out);
    out.extend(fin);
    out
}

// ---- PlayGo (one chunk, one scenario, all languages) ----

const LANGUAGES: [&str; 31] = [
    "ja-JP", "en-US", "fr-FR", "es-ES", "de-DE", "it-IT", "nl-NL", "pt-PT", "ru-RU", "ko-KR", "zh-Hant", "zh-Hans", "fi-FI", "sv-SE", "da-DK", "no-NO", "pl-PL", "pt-BR", "en-GB", "tr-TR", "es-419",
    "ar-AE", "fr-CA", "cs-CZ", "hu-HU", "el-GR", "ro-RO", "th-TH", "vi-VN", "id-ID", "uk-UA",
];

/// `BuildScenarioJson(1, all languages, 0)` (compact `Utf8JsonWriter` output).
pub(crate) fn scenario_json() -> Vec<u8> {
    let langs: Vec<String> = LANGUAGES.iter().map(|l| format!("\"{l}\":{{\"title\":\"Scenario #0\",\"description\":\"Scenario #0\"}}")).collect();
    let codes: Vec<String> = LANGUAGES.iter().map(|l| format!("\"{l}\"")).collect();
    format!(
        "{{\"scenarioCount\":1,\"scenarioDefaultId\":0,\"scenarioDefaultLanguage\":\"en-US\",\"scenarios\":[{{\"id\":0,\"type\":\"playmode\",{}}}],\"chunkDefaultLanguage\":\"en-US\",\"chunkSupportedLanguages\":[{}]}}",
        langs.join(","),
        codes.join(",")
    )
    .into_bytes()
}

fn align16(v: usize) -> usize {
    (v + 15) & !15
}

/// `BuildMultiChunkDat` for one chunk of `main_size` bytes and a tail of `tail_size`, labels
/// included (publisher application), all languages, default language en-US.
pub(crate) fn chunk_dat(content_id: &str, main_size: u64, tail_size: u64) -> Vec<u8> {
    let labels = ["Chunk #0"];
    let scen = ["Scenario #0"];
    let count = 1usize;
    let num_mchunks = 1 + 1; // the chunk and the tail
    let labels_size: usize = labels.iter().map(|l| l.len() + 1).sum();
    let chunk_at = 256;
    let chunk_size = count * 32;
    let mchunk_idx_at = chunk_at + chunk_size;
    let mchunk_idx_size = num_mchunks * 4;
    let label_at = align16(mchunk_idx_at + mchunk_idx_size);
    let mchunk_at = align16(label_at + labels_size);
    let mchunk_size = num_mchunks * 16;
    let scen_at = mchunk_at + mchunk_size;
    let scen_size = 32;
    let scen_chunks_at = scen_at + scen_size;
    let scen_chunks_size = count * 2;
    let scen_label_at = align16(scen_chunks_at + scen_chunks_size);
    let scen_label_size: usize = scen.iter().map(|l| l.len() + 1).sum();
    let total = align16(scen_label_at + scen_label_size);
    let mut b = vec![0u8; total];
    let w16 = |b: &mut Vec<u8>, at: usize, v: u16| b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    let w32 = |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let w64 = |b: &mut Vec<u8>, at: usize, v: u64| b[at..at + 8].copy_from_slice(&v.to_le_bytes());
    b[0..4].copy_from_slice(b"plgx");
    w16(&mut b, 4, 4096);
    w16(&mut b, 8, 1);
    w16(&mut b, 10, count as u16);
    w16(&mut b, 14, 1);
    w32(&mut b, 16, total as u32);
    w16(&mut b, 20, 0);
    w16(&mut b, 22, 1);
    b[30] = 133;
    w32(&mut b, 32, num_mchunks as u32);
    b[36] = 1; // en-US
    b[48] = 17;
    w64(&mut b, 56, u64::MAX);
    b[64..100].copy_from_slice(content_id.as_bytes());
    for (i, (at, size)) in [(chunk_at, chunk_size), (mchunk_idx_at, mchunk_idx_size), (label_at, labels_size), (mchunk_at, mchunk_size), (scen_at, scen_size), (scen_chunks_at, scen_chunks_size), (scen_label_at, scen_label_size)]
        .into_iter()
        .enumerate()
    {
        w32(&mut b, 192 + i * 8, at as u32);
        w32(&mut b, 196 + i * 8, size as u32);
    }
    // Chunk 0: one main mchunk (index 0) and the tail (index 1).
    b[chunk_at] = 128;
    b[chunk_at + 2] = 3;
    w16(&mut b, chunk_at + 4, 2);
    b[chunk_at + 8] = 17;
    w64(&mut b, chunk_at + 16, u64::MAX);
    w32(&mut b, chunk_at + 24, 0);
    w32(&mut b, chunk_at + 28, 0);
    w32(&mut b, mchunk_idx_at, 0);
    w32(&mut b, mchunk_idx_at + 4, 1);
    b[label_at..label_at + labels[0].len()].copy_from_slice(labels[0].as_bytes());
    w64(&mut b, mchunk_at, 0);
    w64(&mut b, mchunk_at + 8, main_size);
    w64(&mut b, mchunk_at + 16, main_size);
    w64(&mut b, mchunk_at + 24, tail_size);
    w64(&mut b, scen_at, 33);
    w16(&mut b, scen_at + 20, 1);
    w16(&mut b, scen_at + 22, count as u16);
    w32(&mut b, scen_at + 24, 0);
    w32(&mut b, scen_at + 28, 0);
    w16(&mut b, scen_chunks_at, 0);
    b[scen_label_at..scen_label_at + scen[0].len()].copy_from_slice(scen[0].as_bytes());
    b
}

fn flt_header(count: usize) -> Vec<u8> {
    let mut a = vec![0u8; 56 + count * 8];
    a[0..4].copy_from_slice(&1u32.to_le_bytes());
    a[4..8].copy_from_slice(&134_217_728u32.to_le_bytes());
    a[8..12].copy_from_slice(&56u32.to_le_bytes());
    a[12..16].copy_from_slice(&((count * 8) as u32).to_le_bytes());
    a[24..28].copy_from_slice(b"\x7fFLT");
    a[36..40].copy_from_slice(&(count as u32).to_le_bytes());
    a[40..56].copy_from_slice(&[81, 79, 162, 38, 171, 138, 202, 146, 77, 196, 27, 164, 97, 183, 187, 9]);
    a
}

/// `BuildFileMap` with every file in chunk 0: (playgo-hash-table.dat, playgo-ficm.dat).
pub(crate) fn file_map(paths: &[String]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut h: Vec<u64> = paths.iter().map(|p| hash_path(p)).collect();
    h.sort_unstable();
    if h.windows(2).any(|w| w[0] == w[1]) {
        return Err("PlayGo file paths have duplicate hashes".into());
    }
    let mut table = flt_header(h.len());
    for (i, v) in h.iter().enumerate() {
        table[56 + i * 8..64 + i * 8].copy_from_slice(&v.to_le_bytes());
    }
    let n = h.len() * 2;
    let mut ficm = vec![0u8; 16 + n];
    ficm[0..4].copy_from_slice(&1u32.to_le_bytes());
    ficm[8..12].copy_from_slice(&16u32.to_le_bytes());
    ficm[12..16].copy_from_slice(&(n as u32).to_le_bytes());
    Ok((table, ficm))
}

// ---- The container ----

const ID_DIGESTS: u32 = 0x1;
const ID_ENTRY_KEYS: u32 = 0x10;
const ID_IMAGE_KEY: u32 = 0x20;
const ID_GENERAL_DIGESTS: u32 = 0x80;
const ID_METAS: u32 = 0x100;
const ID_ENTRY_NAMES: u32 = 0x200;
const ID_IMAGEDIGS: u32 = 0x40A;
const ID_PLAYGO_CHUNK: u32 = 0x1001;
const ID_PARAM_JSON: u32 = 0x2000;

/// The artwork entries the shell reads (`MediaFiles`, `DdsMedia`): (name, id).
pub(crate) const MEDIA: [(&str, u32); 10] = [
    ("icon0.png", 0x1200),
    ("pic0.png", 0x1220),
    ("pic1.png", 0x1006),
    ("pic2.png", 0x2040),
    ("snd0.at9", 0x1240),
    ("save_data.png", 0x100D),
    ("icon0.dds", 0x1280),
    ("pic0.dds", 0x12A0),
    ("pic1.dds", 0x12C0),
    ("pic2.dds", 0x2060),
];
const SYSTEM_MEDIA_IDS: [u32; 10] = [0x1006, 0x100D, 0x1200, 0x1220, 0x1240, 0x1280, 0x12A0, 0x12C0, 0x2040, 0x2060];
const PLAYGO_IDS: [u32; 4] = [0x1001, 0x2010, 0x2011, 0x3000];

struct Entry {
    id: u32,
    name: Option<String>,
    data: Vec<u8>,
    offset: u32,
    name_offset: u32,
    flags1: u32,
}

fn rank(id: u32) -> i32 {
    match id {
        0x2000 => 0,
        0x40A | 0x1001 => 2,
        0x2010 | 0x2011 => 4,
        _ if (0x3000..0x3090).contains(&id) => 4,
        _ if (0x1200..0x2000).contains(&id) || id == 0x1006 || id == 0x100D => 3,
        _ => 1,
    }
}

fn flags1(id: u32) -> u32 {
    match id {
        ID_DIGESTS | ID_ENTRY_NAMES => 0x4000_0000,
        ID_ENTRY_KEYS | ID_IMAGE_KEY | ID_GENERAL_DIGESTS | ID_METAS => 0x6000_0000,
        ID_PARAM_JSON => 0,
        _ => 0x0800_0000,
    }
}

/// What the CNT needs from the rest of the package.
pub(crate) struct CntInput<'a> {
    pub content_id: &'a str,
    pub param_json: Vec<u8>,
    /// Artwork entries (name in `sce_sys/`, bytes), from `MEDIA`.
    pub media: Vec<(String, Vec<u8>)>,
    pub pfs_size: u64,
    pub image_digests: &'a [[u8; 32]],
    pub superblock: &'a [u8],
    pub pfs_image_key: [u8; 32],
    /// The inner image length (64 KiB aligned) and its files' paths in AFID order.
    pub inner_len: u64,
    pub file_paths: Vec<String>,
    /// The FIH header the package will carry.
    pub fih: &'a [u8],
}

/// The CNT bytes as embedded in the package (everything before the PFS image offset, the
/// header's PFS image offset already 0x10000), and the `playgo-chunk.dat` entry.
pub(crate) fn build(keys: &Keyset, inp: CntInput) -> Result<(Vec<u8>, Vec<u8>), String> {
    let cid = inp.content_id;
    // Entry keys (publisher profile).
    let mut ek = sha3(&{
        let mut p = cid.as_bytes().to_vec();
        p.resize(48, 0);
        p
    })
    .to_vec();
    let mut wrapped = Vec::new();
    for i in 0..7u32 {
        let k = compute_key(cid, i);
        let d = sha3(&k);
        ek.extend(d.iter().zip(k).map(|(a, b)| a ^ b));
        wrapped.push(if i == 0 {
            rsa_pkcs1_deterministic(&keys.passcode_moduli[0], PASSCODE.as_bytes())
        } else {
            rsa_pkcs1_deterministic(&keys.passcode_moduli[i as usize], &k)
        });
    }
    for w in wrapped {
        ek.extend(w);
    }
    // Image key: the pfs-image-key wrapped for the mount, then SHAKE128 filler.
    let mut image_key = rsa_pkcs1_deterministic(&keys.mount_image_modulus, &inp.pfs_image_key);
    {
        use sha3::digest::{ExtendableOutput, Update, XofReader};
        let mut s = sha3::Shake128::default();
        s.update(&sha3(&inp.pfs_image_key));
        let mut tail = vec![0u8; 2048 - 384];
        s.finalize_xof().read(&mut tail);
        image_key.extend(tail);
    }
    let imagedigs: Vec<u8> = inp.image_digests.iter().flat_map(|d| d.iter().rev().copied()).collect();
    let mchunk_total = 65536 + inp.pfs_size;
    let main = (inp.inner_len.saturating_sub(65536)).min(mchunk_total);
    let mut chunk = chunk_dat(cid, main, mchunk_total - main);
    let (hash_table, ficm) = file_map(&inp.file_paths)?;

    let mut entries: Vec<Entry> = Vec::new();
    let mut push = |id: u32, name: Option<&str>, data: Vec<u8>| entries.push(Entry { id, name: name.map(String::from), data, offset: 0, name_offset: 0, flags1: flags1(id) });
    push(ID_ENTRY_KEYS, None, ek);
    push(ID_IMAGE_KEY, None, image_key);
    push(ID_GENERAL_DIGESTS, None, vec![0; 480]);
    push(ID_METAS, None, vec![]);
    push(ID_DIGESTS, None, vec![]);
    push(ID_ENTRY_NAMES, None, vec![]);
    let mut rest: Vec<(u32, Option<&str>, Vec<u8>)> = vec![(ID_PARAM_JSON, Some("param.json"), inp.param_json.clone())];
    for (name, data) in &inp.media {
        let id = MEDIA.iter().find(|m| m.0 == name).ok_or_else(|| format!("unsupported package artwork {name}"))?.1;
        rest.push((id, Some(name.as_str()), data.clone()));
    }
    rest.push((ID_IMAGEDIGS, None, imagedigs));
    rest.push((ID_PLAYGO_CHUNK, Some("playgo-chunk.dat"), vec![]));
    rest.push((0x2010, Some("playgo-hash-table.dat"), hash_table));
    rest.push((0x2011, Some("playgo-ficm.dat"), ficm));
    rest.push((0x3000, Some("playgo-scenario.json"), scenario_json()));
    rest.sort_by_key(|e| (rank(e.0), e.0));
    for (id, name, data) in rest {
        push(id, name, data);
    }
    let count = entries.len();
    let digests_at = entries.iter().position(|e| e.id == ID_DIGESTS).unwrap();
    entries[digests_at].data = vec![0; count * 32];
    // Name table: in id order.
    let mut names: Vec<u8> = vec![0];
    let mut by_id: Vec<usize> = (0..count).collect();
    by_id.sort_by_key(|&i| entries[i].id);
    for &i in &by_id {
        if let Some(n) = entries[i].name.clone() {
            entries[i].name_offset = names.len() as u32;
            names.extend(n.as_bytes());
            names.push(0);
        }
    }
    let names_at = entries.iter().position(|e| e.id == ID_ENTRY_NAMES).unwrap();
    entries[names_at].data = names;
    let metas_at = entries.iter().position(|e| e.id == ID_METAS).unwrap();
    // Layout (the PlayGo chunk file's size is known: one chunk).
    let chunk_len = chunk.len();
    let mut off = BODY_OFFSET;
    for e in entries.iter_mut() {
        e.offset = off as u32;
        let len = if e.id == ID_METAS {
            count * 32
        } else if e.id == ID_PLAYGO_CHUNK {
            chunk_len
        } else {
            e.data.len()
        };
        off = (off + len as u64 + 15) & !15;
    }
    let body_size = ((off + 0xFFFF) & !0xFFFF) - BODY_OFFSET;
    let pfs_image_offset = BODY_OFFSET + body_size;
    // The tail mchunk of playgo-chunk.dat ends at the PFS end.
    {
        let p = u32::from_le_bytes(chunk[216..220].try_into().unwrap()) as usize + u32::from_le_bytes(chunk[220..224].try_into().unwrap()) as usize - 16;
        let start = u64::from_le_bytes(chunk[p..p + 8].try_into().unwrap());
        chunk[p + 8..p + 16].copy_from_slice(&(inp.pfs_size + 65536 - start).to_le_bytes());
    }
    let chunk_out = chunk.clone();
    entries.iter_mut().find(|e| e.id == ID_PLAYGO_CHUNK).unwrap().data = chunk;
    let meta_bytes = |entries: &[Entry]| -> Vec<u8> {
        let mut v: Vec<&Entry> = entries.iter().collect();
        v.sort_by_key(|e| e.id);
        let mut out = Vec::with_capacity(v.len() * 32);
        for e in v {
            let size = if e.id == ID_METAS { (entries.len() * 32) as u32 } else { e.data.len() as u32 };
            for x in [e.id, e.name_offset, e.flags1, 0, e.offset, size] {
                out.extend(x.to_be_bytes());
            }
            out.extend([0u8; 8]);
        }
        out
    };
    entries[metas_at].data = meta_bytes(&entries);
    let main_ent_data_size: usize = entries[..5].iter().map(|e| e.data.len()).sum();
    let get = |entries: &[Entry], id: u32| entries.iter().position(|e| e.id == id).unwrap();
    let imagedigs_at = get(&entries, ID_IMAGEDIGS);
    let image_key_at = get(&entries, ID_IMAGE_KEY);

    // Header.
    let game = sha3(inp.superblock);
    let mut h = vec![0u8; 0x1000];
    let be32 = |h: &mut Vec<u8>, at: usize, v: u32| h[at..at + 4].copy_from_slice(&v.to_be_bytes());
    let be64 = |h: &mut Vec<u8>, at: usize, v: u64| h[at..at + 8].copy_from_slice(&v.to_be_bytes());
    h[0..4].copy_from_slice(b"\x7fCNT");
    be32(&mut h, 4, 0x20001);
    be32(&mut h, 8, 0x8000_0000);
    be32(&mut h, 12, 12);
    be32(&mut h, 16, count as u32);
    h[20..22].copy_from_slice(&6u16.to_be_bytes());
    h[22..24].copy_from_slice(&(count as u16).to_be_bytes());
    be32(&mut h, 24, entries[metas_at].offset);
    be32(&mut h, 28, main_ent_data_size as u32);
    be64(&mut h, 32, BODY_OFFSET);
    be64(&mut h, 40, body_size);
    be64(&mut h, 48, entries[imagedigs_at].offset as u64);
    h[64..100].copy_from_slice(cid.as_bytes());
    be32(&mut h, 112, 0); // drm: free
    be32(&mut h, 116, 32);
    be32(&mut h, 120, 0x0200_0000 | 0x20000);
    be32(&mut h, 124, pfs_image_offset as u32);
    be32(&mut h, 128, 539_231_496);
    be32(&mut h, 132, 152_027_073);
    h[512..548].copy_from_slice(cid.as_bytes());
    be32(&mut h, 1024, 1);
    be32(&mut h, 1028, 1);
    be64(&mut h, 1032, 11_529_215_046_068_470_540);
    be64(&mut h, 1040, pfs_image_offset);
    be64(&mut h, 1048, inp.pfs_size);
    let package_size = 65536 + inp.pfs_size + pfs_image_offset;
    be64(&mut h, 1064, package_size);
    be64(&mut h, 1072, package_size);
    be32(&mut h, 1080, 65536);
    h[1088..1120].copy_from_slice(&game);
    h[1120..1152].copy_from_slice(&sha3(inp.fih));
    h[1184..1200].copy_from_slice(&inp.superblock[880..896]);
    be64(&mut h, 1200, 65536 + inp.pfs_size);
    be64(&mut h, 1208, pfs_image_offset);
    be32(&mut h, 1296, entries[image_key_at].offset);
    be32(&mut h, 1300, entries[image_key_at].data.len() as u32);
    be32(&mut h, 1304, entries[imagedigs_at].offset);
    be32(&mut h, 1308, entries[imagedigs_at].data.len() as u32);

    // General digests.
    let mut set = 0u32;
    let mut slots: [[u8; 32]; 14] = [[0; 32]; 14];
    let mut put = |flag: u32, d: [u8; 32]| {
        slots[flag.trailing_zeros() as usize - 1] = d;
        set |= flag;
    };
    let mut hd = h[0..64].to_vec();
    hd.extend(&h[1024..1152]);
    hd[64 + 16..64 + 24].copy_from_slice(&65536u64.to_be_bytes());
    put(0x8, sha3(&hd));
    let mut desc = [0u8; 56];
    desc[..36].copy_from_slice(cid.as_bytes());
    desc[52..56].copy_from_slice(&32u32.to_be_bytes());
    let mut cd = desc.to_vec();
    cd.extend(game);
    cd.extend([0u8; 32]);
    put(0x2, sha3(&cd));
    put(0x4, game);
    put(0x1000, game);
    let concat = |ids: &[u32]| -> Option<[u8; 32]> {
        let mut v: Vec<&Entry> = entries.iter().filter(|e| ids.contains(&e.id)).collect();
        v.sort_by_key(|e| e.id);
        (!v.is_empty()).then(|| sha3(&v.iter().flat_map(|e| sha3(&e.data)).collect::<Vec<u8>>()))
    };
    if let Some(d) = concat(&SYSTEM_MEDIA_IDS) {
        put(0x10, d);
    }
    if let Some(d) = concat(&PLAYGO_IDS) {
        put(0x80, d);
    }
    put(0x40, sha3(&inp.param_json));
    let gd = &mut entries.iter_mut().find(|e| e.id == ID_GENERAL_DIGESTS).unwrap().data;
    gd[0..2].copy_from_slice(&53846u16.to_be_bytes());
    gd[2..4].copy_from_slice(&258u16.to_be_bytes());
    gd[28..32].copy_from_slice(&set.to_be_bytes());
    for (i, s) in slots.iter().enumerate() {
        gd[32 + i * 32..64 + i * 32].copy_from_slice(s);
    }

    // Body, then the per-entry digests (in id order, the digest table itself excluded).
    let mut cnt = vec![0u8; pfs_image_offset as usize];
    for e in &entries {
        cnt[e.offset as usize..e.offset as usize + e.data.len()].copy_from_slice(&e.data);
    }
    let digests_off = entries[digests_at].offset as usize;
    by_id.sort_by_key(|&i| entries[i].id);
    for (n, &i) in by_id.iter().enumerate().skip(1) {
        let e = &entries[i];
        let len = if e.id == ID_METAS { count * 32 } else { e.data.len() };
        let d = sha3(&cnt[e.offset as usize..e.offset as usize + len]);
        cnt[digests_off + 32 * n..digests_off + 32 * n + 32].copy_from_slice(&d);
    }
    let body_digest = sha3(&cnt[BODY_OFFSET as usize..(BODY_OFFSET + body_size) as usize]);
    let digest_table_hash = sha3(&cnt[digests_off..digests_off + count * 32]);
    let range = |id: u32, len: Option<usize>| -> &[u8] {
        let e = &entries[get(&entries, id)];
        let l = len.unwrap_or(if id == ID_METAS { count * 32 } else { e.data.len() });
        &cnt[e.offset as usize..e.offset as usize + l]
    };
    let sc1: Vec<u8> = [ID_ENTRY_KEYS, ID_IMAGE_KEY, ID_GENERAL_DIGESTS, ID_METAS, ID_DIGESTS].iter().flat_map(|&id| range(id, None).to_vec()).collect();
    let sc2: Vec<u8> = [ID_ENTRY_KEYS, ID_IMAGE_KEY, ID_GENERAL_DIGESTS].iter().flat_map(|&id| range(id, None).to_vec()).chain(range(ID_METAS, Some(6 * 32)).to_vec()).collect();
    let mut dd = sha3(range(ID_IMAGE_KEY, None)).to_vec();
    dd.extend(sha3(range(ID_IMAGEDIGS, None)));
    h[256..288].copy_from_slice(&sha3(&sc1));
    h[288..320].copy_from_slice(&sha3(&sc2));
    h[320..352].copy_from_slice(&digest_table_hash);
    h[352..384].copy_from_slice(&body_digest);
    h[1312..1376].copy_from_slice(&dd);
    // As embedded in the package: the PFS image right after the FIH header.
    h[1040..1048].copy_from_slice(&65536u64.to_be_bytes());
    let pd = sha3(&h[..4064]);
    h[4064..4096].copy_from_slice(&pd);
    cnt[..4096].copy_from_slice(&h);
    let wrap = rsa_pkcs1_deterministic(&keys.passcode_moduli[3], &sha3(&cnt[..4096]));
    cnt[4096..4480].copy_from_slice(&wrap);
    Ok((cnt, chunk_out))
}
