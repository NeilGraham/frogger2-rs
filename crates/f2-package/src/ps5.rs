//! Export for PS5: the native app folder of the player's game, from the prebuilt runtime
//! (docs/EXPORT.md "PS5").
//!
//! `<out>/<title> (PS5)/` holds what `tools/ps5/build.sh --data` puts in `target/ps5/PPSA99920/`:
//! `eboot.bin` and `sce_module/libc.prx` from the runtime, `sce_sys/param.json` (the runtime's,
//! with the title from the player's disc), the artwork (`icon0.png`/`.dds` 512x512,
//! `pic0`/`pic1` `.png`/`.dds` 3840x2160, BC7 DX10 as the shell wants) made from the branding
//! art, and the game data in `data/` (`/app0/data`, where the console host looks first). It is
//! installed by copying it to the console (e.g. `/data/homebrew/` for ShadowMountPlus).

mod dds;

use crate::ps4::{art, runtime::Runtime};
use crate::{util, Job};
use std::path::{Path, PathBuf};

pub(crate) fn export(job: &Job) -> Result<PathBuf, String> {
    let log = job.log;
    let rt = Runtime::load(job.runtime, "ps5", log)?;
    let title_id = rt.str("title_id")?.to_string();
    let data_dir = rt.json.get("data_dir").and_then(|v| v.as_str()).unwrap_or("data").trim_matches('/').to_string();
    let mut param = rt.json.get("param_json").cloned().ok_or("runtime.json: missing \"param_json\"")?;
    let set_title = |p: &mut serde_json::Value| -> Option<()> {
        let lp = p.get_mut("localizedParameters")?.as_object_mut()?;
        let lang = lp.get("defaultLanguage").and_then(|v| v.as_str()).unwrap_or("en-US").to_string();
        lp.entry(lang).or_insert_with(|| serde_json::json!({})).as_object_mut()?.insert("titleName".into(), job.branding.title.clone().into());
        Some(())
    };
    set_title(&mut param).ok_or("runtime.json: param_json has no localizedParameters")?;
    if rt.file("eboot.bin").is_none() {
        return Err("the PS5 runtime has no eboot.bin".into());
    }
    let data = util::rendition_files(job.rendition).map_err(|e| format!("{}: {e}", job.rendition.display()))?;
    if !data.iter().any(|(p, _)| p.starts_with("game/")) {
        return Err(format!("{} has no game/ folder (not an imported rendition)", job.rendition.display()));
    }

    let out = job.out_dir.join(format!("{} (PS5)", job.branding.file_stem()));
    let part = job.out_dir.join(format!(".{} (PS5).part", job.branding.file_stem()));
    let _ = std::fs::remove_dir_all(&part);
    let write = |rel: &str, bytes: &[u8]| -> Result<(), String> {
        let p = part.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| format!("{}: {e}", p.display()))?;
        std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))
    };
    let result = (|| -> Result<(), String> {
        for (path, bytes) in rt.ordered_files() {
            write(path, bytes)?;
        }
        let mut json = serde_json::to_string_pretty(&param).map_err(|e| e.to_string())?;
        json.push('\n');
        write("sce_sys/param.json", json.as_bytes())?;

        log("Making the artwork (icon0 512x512, pic0/pic1 3840x2160, PNG + BC7 DDS)...");
        let icon = art::icon(&job.branding.art, 512);
        write("sce_sys/icon0.png", &art::png_rgb(&icon))?;
        write("sce_sys/icon0.dds", &dds::bc7_dds(&icon))?;
        let pic = art::backdrop(&job.branding.art, 3840, 2160);
        let (png, dds) = (art::png_rgb(&pic), dds::bc7_dds(&pic));
        for n in ["pic0", "pic1"] {
            write(&format!("sce_sys/{n}.png"), &png)?;
            write(&format!("sce_sys/{n}.dds"), &dds)?;
        }

        let total: u64 = data.iter().filter_map(|(_, f)| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
        log(&format!("Copying the game data ({} files, {} MB)...", data.len(), total >> 20));
        for (rel, full) in &data {
            let dst = part.join(&data_dir).join(rel);
            std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| format!("{}: {e}", dst.display()))?;
            std::fs::copy(full, &dst).map_err(|e| format!("{} -> {}: {e}", full.display(), dst.display()))?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&part);
        return Err(e);
    }
    replace_dir(&part, &out)?;
    log(&format!("PS5 app folder: {} (title {title_id}); copy it to the console, e.g. /data/homebrew/ for ShadowMountPlus", out.display()));
    Ok(out)
}

/// Moves `from` to `to`, replacing a previous export there (only a folder that looks like one:
/// it has `sce_sys/param.json`).
fn replace_dir(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        if !to.join("sce_sys/param.json").is_file() {
            return Err(format!("{} exists and is not a previous PS5 export; move it away first", to.display()));
        }
        std::fs::remove_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    std::fs::rename(from, to).map_err(|e| format!("{}: {e}", to.display()))
}

#[cfg(test)]
mod tests {
    use crate::ps4::tests::{temp_dir, test_branding, test_rendition};
    use crate::{Job, Target};

    #[test]
    fn export_folder() {
        let root = temp_dir("ps5");
        let rendition = test_rendition(&root);
        let rt = root.join("runtime-ps5");
        std::fs::create_dir_all(rt.join("sce_module")).unwrap();
        std::fs::write(rt.join("eboot.bin"), b"eboot").unwrap();
        std::fs::write(rt.join("sce_module/libc.prx"), b"libc").unwrap();
        std::fs::write(
            rt.join("runtime.json"),
            r#"{"target":"ps5","version":"0.0.0","format":1,"title_id":"PPSA99920","content_id":"UP9000-PPSA99920_00-FROGGER2RUSTPORT",
                "param_json":{"attribute3":524352,"localizedParameters":{"defaultLanguage":"en-US","en-US":{"titleName":""}},"titleId":"PPSA99920"},
                "data_dir":"data","files":["eboot.bin","sce_module/libc.prx"]}"#,
        )
        .unwrap();
        let out = root.join("out");
        let branding = test_branding();
        let job = Job { target: Target::Ps5, rendition: &rendition, runtime: &rt, out_dir: &out, branding: &branding, keys_dir: None, log: &|_| {} };
        for _ in 0..2 {
            // the second export replaces the first
            let dir = crate::export(&job).unwrap();
            assert_eq!(dir.file_name().unwrap(), "Frogger 2 - Swampy's Revenge (PS5)");
            let param: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("sce_sys/param.json")).unwrap()).unwrap();
            assert_eq!(param["localizedParameters"]["en-US"]["titleName"], "Frogger 2: Swampy's Revenge");
            assert_eq!(param["attribute3"], 524352);
            assert_eq!(std::fs::read(dir.join("eboot.bin")).unwrap(), b"eboot");
            assert_eq!(std::fs::read(dir.join("data/game/maps/b.bin")).unwrap(), std::fs::read(rendition.join("game/maps/b.bin")).unwrap());
            assert!(!dir.join("data/oracle").exists());
            let dds = std::fs::read(dir.join("sce_sys/pic1.dds")).unwrap();
            assert_eq!(dds.len(), 148 + 3840 * 2160);
            assert_eq!(&dds[12..20], &[0x70, 0x08, 0, 0, 0, 0x0F, 0, 0]); // 2160 x 3840
            assert_eq!(std::fs::read(dir.join("sce_sys/icon0.dds")).unwrap().len(), 148 + 512 * 512);
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
