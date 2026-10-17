//! GitHub releases of the project: parsing the API's JSON, picking this platform's asset,
//! and checking downloads against the release's `SHA256SUMS.txt`.

use std::collections::HashMap;

/// The releases API (`F2_RELEASES_URL` / `[launcher] releases_url` override it, e.g. a mirror).
pub const DEFAULT_RELEASES_URL: &str = "https://api.github.com/repos/NeilGraham/frogger2-rs/releases";
/// The human-readable releases page.
pub const RELEASES_PAGE_URL: &str = "https://github.com/NeilGraham/frogger2-rs/releases";
/// The checksum file every release carries.
pub const SUMS_NAME: &str = "SHA256SUMS.txt";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// `v0.1.0`.
    pub tag: String,
    pub name: String,
    /// `2026-10-17` (the date part of `published_at`).
    pub date: String,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

impl Release {
    /// `0.1.0` for the tag `v0.1.0` (the release workflow names assets with it).
    pub fn version(&self) -> &str {
        self.tag.trim_start_matches('v')
    }

    /// The archive for `os` (`windows`, `linux`, `macos`) and `arch` (`x86_64`, `aarch64`).
    pub fn asset_for(&self, os: &str, arch: &str) -> Option<&Asset> {
        let want = platform_asset_name(self.version(), os, arch);
        self.assets.iter().find(|a| a.name == want)
    }

    pub fn sums_asset(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == SUMS_NAME)
    }
}

/// The release workflow's asset names (`.github/workflows/release.yml`).
pub fn platform_asset_name(version: &str, os: &str, arch: &str) -> String {
    match os {
        "windows" => format!("frogger2-rs-{version}-windows-{arch}.zip"),
        "macos" => format!("frogger2-rs-{version}-macos-universal.zip"),
        _ => format!("frogger2-rs-{version}-linux-{arch}.tar.gz"),
    }
}

pub fn host_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

pub fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    }
}

/// The releases API's array (drafts are skipped), newest first as GitHub returns it.
pub fn parse_releases(json: &str) -> Result<Vec<Release>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("releases: {e}"))?;
    let arr = v.as_array().ok_or("releases: expected a JSON array")?;
    let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut out = Vec::new();
    for r in arr {
        if r.get("draft").and_then(|d| d.as_bool()).unwrap_or(false) {
            continue;
        }
        let tag = s(r, "tag_name");
        if tag.is_empty() {
            continue;
        }
        let assets = r
            .get("assets")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .map(|x| Asset { name: s(x, "name"), url: s(x, "browser_download_url"), size: x.get("size").and_then(|n| n.as_u64()).unwrap_or(0) })
                    .filter(|a| !a.name.is_empty() && !a.url.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        out.push(Release {
            name: s(r, "name"),
            date: s(r, "published_at").chars().take(10).collect(),
            prerelease: r.get("prerelease").and_then(|d| d.as_bool()).unwrap_or(false),
            tag,
            assets,
        });
    }
    Ok(out)
}

/// `sha256sum` output (`<hex>  <name>` or `<hex> *<name>`) as name -> lower-case hex.
pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (h, n) = l.trim().split_once(char::is_whitespace)?;
            let n = n.trim().trim_start_matches('*');
            (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()) && !n.is_empty()).then(|| (n.to_string(), h.to_ascii_lowercase()))
        })
        .collect()
}

/// `1.2.3` (an optional leading `v`, anything after `-`/`+` ignored) as comparable numbers.
pub fn parse_version(v: &str) -> Option<[u32; 3]> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u32>().ok());
    Some([it.next()??, it.next().unwrap_or(Some(0))?, it.next().unwrap_or(Some(0))?])
}

/// Whether `candidate` is a newer version than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"[
      {"tag_name":"v0.2.0","name":"0.2.0","published_at":"2026-11-01T10:00:00Z","prerelease":false,"draft":false,"assets":[
        {"name":"frogger2-rs-0.2.0-windows-x86_64.zip","browser_download_url":"https://x/w.zip","size":100},
        {"name":"SHA256SUMS.txt","browser_download_url":"https://x/sums","size":10}]},
      {"tag_name":"v0.3.0","draft":true,"assets":[]},
      {"tag_name":"v0.1.0","name":"first","published_at":"2026-10-20T00:00:00Z","prerelease":true,"assets":[]}
    ]"#;

    #[test]
    fn parses_and_picks_assets() {
        let r = parse_releases(JSON).unwrap();
        assert_eq!(r.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(), ["v0.2.0", "v0.1.0"], "drafts are skipped");
        assert_eq!(r[0].version(), "0.2.0");
        assert_eq!(r[0].date, "2026-11-01");
        assert_eq!(r[0].asset_for("windows", "x86_64").unwrap().url, "https://x/w.zip");
        assert!(r[0].asset_for("windows", "aarch64").is_none());
        assert!(r[0].sums_asset().is_some());
        assert!(r[1].prerelease);
        assert!(parse_releases("{}").is_err());
    }

    #[test]
    fn asset_names_match_the_release_workflow() {
        assert_eq!(platform_asset_name("0.1.0", "windows", "aarch64"), "frogger2-rs-0.1.0-windows-aarch64.zip");
        assert_eq!(platform_asset_name("0.1.0", "macos", "aarch64"), "frogger2-rs-0.1.0-macos-universal.zip");
        assert_eq!(platform_asset_name("0.1.0", "linux", "x86_64"), "frogger2-rs-0.1.0-linux-x86_64.tar.gz");
    }

    #[test]
    fn sums_and_versions() {
        let h = "a".repeat(64);
        let m = parse_sha256sums(&format!("{h}  one.zip\n{}  *two.zip\nnot a line\n", "B".repeat(64)));
        assert_eq!(m["one.zip"], h);
        assert_eq!(m["two.zip"], "b".repeat(64));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("junk", "0.1.0"));
    }
}
