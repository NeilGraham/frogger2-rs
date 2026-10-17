//! Console runtimes (PS4 / PS5, docs/EXPORT.md): the release zip `runtime-<target>-<v>.zip`
//! (or the extracted folder) with the app's files and a `runtime.json` describing them.

use serde_json::Value;
use std::io::Read;
use std::path::Path;

pub(crate) struct Runtime {
    pub(crate) json: Value,
    /// Every file but `runtime.json`: (path inside the app, bytes), sorted by path.
    pub(crate) files: Vec<(String, Vec<u8>)>,
}

impl Runtime {
    /// Loads a runtime (zip or folder) and checks it is for `target` with format 1.
    pub(crate) fn load(path: &Path, target: &str, log: &dyn Fn(&str)) -> Result<Runtime, String> {
        Self::load_format(path, target, 1, log)
    }

    /// Loads a runtime (zip or folder) and checks it is for `target` with format `format`.
    pub(crate) fn load_format(path: &Path, target: &str, format: u64, log: &dyn Fn(&str)) -> Result<Runtime, String> {
        let err = |e: &dyn std::fmt::Display| format!("runtime {}: {e}", path.display());
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        if path.is_dir() {
            for (rel, full) in crate::util::walk(path).map_err(|e| err(&e))? {
                files.push((rel, std::fs::read(&full).map_err(|e| err(&e))?));
            }
        } else {
            let f = std::fs::File::open(path).map_err(|e| err(&e))?;
            let mut zip = zip::ZipArchive::new(f).map_err(|e| err(&e))?;
            for i in 0..zip.len() {
                let mut e = zip.by_index(i).map_err(|e| err(&e))?;
                if e.is_dir() {
                    continue;
                }
                let Some(name) = e.enclosed_name() else { return Err(err(&format!("unsafe path {:?}", e.name()))) };
                let rel = name.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                let mut b = Vec::with_capacity(e.size() as usize);
                e.read_to_end(&mut b).map_err(|e| err(&e))?;
                files.push((rel, b));
            }
        }
        // allow one enclosing folder (a zip of the runtime directory)
        if !files.iter().any(|(p, _)| p == "runtime.json") {
            if let Some(prefix) = files
                .iter()
                .find(|(p, _)| p.ends_with("/runtime.json") && p.matches('/').count() == 1)
                .map(|(p, _)| p.trim_end_matches("runtime.json").to_string())
            {
                files.retain(|(p, _)| p.starts_with(&prefix));
                for (p, _) in &mut files {
                    *p = p[prefix.len()..].to_string();
                }
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let pos = files.iter().position(|(p, _)| p == "runtime.json").ok_or_else(|| err(&"no runtime.json (not a console runtime)"))?;
        let (_, json) = files.remove(pos);
        let json: Value = serde_json::from_slice(&json).map_err(|e| err(&format!("runtime.json: {e}")))?;
        let rt = Runtime { json, files };
        if rt.str("target")? != target {
            return Err(err(&format!("is for {}, not {target}", rt.str("target")?)));
        }
        let got = rt.json.get("format").and_then(Value::as_u64).unwrap_or(0);
        if got != format {
            let why = if got < format { "an older runtime" } else { "a newer runtime" };
            return Err(err(&format!(
                "runtime format {got} ({why}); this app reads format {format}: use the {target} runtime of this app's version ({})",
                env!("CARGO_PKG_VERSION")
            )));
        }
        let v = rt.json.get("version").and_then(Value::as_str).unwrap_or("?");
        if v != env!("CARGO_PKG_VERSION") {
            log(&format!("Note: the {target} runtime is version {v}, this app is {}", env!("CARGO_PKG_VERSION")));
        }
        Ok(rt)
    }

    pub(crate) fn str(&self, key: &str) -> Result<&str, String> {
        self.json.get(key).and_then(Value::as_str).ok_or_else(|| format!("runtime.json: missing \"{key}\""))
    }

    pub(crate) fn int(&self, key: &str, default: i64) -> i64 {
        self.json.get(key).and_then(Value::as_i64).unwrap_or(default)
    }

    pub(crate) fn file(&self, path: &str) -> Option<&[u8]> {
        self.files.iter().find(|(p, _)| p == path).map(|(_, b)| b.as_slice())
    }

    /// The app files in package order: runtime.json "files" first (in its order), then the rest.
    pub(crate) fn ordered_files(&self) -> Vec<(&str, &[u8])> {
        let mut out: Vec<(&str, &[u8])> = Vec::new();
        if let Some(list) = self.json.get("files").and_then(Value::as_array) {
            for p in list.iter().filter_map(Value::as_str) {
                if let Some(b) = self.file(p) {
                    out.push((p, b));
                }
            }
        }
        for (p, b) in &self.files {
            if !out.iter().any(|(q, _)| q == p) {
                out.push((p.as_str(), b.as_slice()));
            }
        }
        out
    }
}

/// Seconds since 1970 for the package time stamps: `SOURCE_DATE_EPOCH` if set (reproducible
/// builds), else now.
pub(crate) fn now() -> i64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
}

/// (year, month, day, hour, minute, second) in UTC of `t` seconds since 1970.
pub(crate) fn civil(t: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400) as u32;
    // Howard Hinnant's days -> civil
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, secs / 3600, secs / 60 % 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(1_792_004_116), (2026, 10, 14, 18, 55, 16));
    }

    #[test]
    fn folder_runtime() {
        let dir = std::env::temp_dir().join(format!("f2-rt-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sce_module")).unwrap();
        std::fs::write(dir.join("runtime.json"), r#"{"target":"ps4","version":"0","format":1,"files":["eboot.bin","sce_module/libc.prx"]}"#).unwrap();
        std::fs::write(dir.join("eboot.bin"), b"e").unwrap();
        std::fs::write(dir.join("sce_module/libSceFios2.prx"), b"f").unwrap();
        std::fs::write(dir.join("sce_module/libc.prx"), b"c").unwrap();
        let rt = Runtime::load(&dir, "ps4", &|_| {}).unwrap();
        let names: Vec<&str> = rt.ordered_files().iter().map(|(p, _)| *p).collect();
        assert_eq!(names, ["eboot.bin", "sce_module/libc.prx", "sce_module/libSceFios2.prx"]);
        assert!(Runtime::load(&dir, "ps5", &|_| {}).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
