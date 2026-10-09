//! Locating the imported data and exposing it to the simulation.

use f2_mods::Vfs;
use f2_platform::Config;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The mod VFS over the selected rendition.
#[derive(Clone)]
pub struct Data {
    pub vfs: Arc<Vfs>,
    pub rendition: PathBuf,
}

impl f2_game::assets::DataSource for Data {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.vfs.read(path).ok()
    }
}

impl Data {
    pub fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        self.vfs.read(path).map_err(|e| format!("{path}: {e}"))
    }

    /// Finds the rendition: `--data <dir>` (a rendition or a data root), else the config's
    /// data root/rendition, else `$F2_GAME_DIR` / `game-data` (developer checkouts).
    pub fn locate(cfg: &Config, arg: Option<PathBuf>) -> Result<Data, String> {
        let mut tried = Vec::new();
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(a) = arg {
            candidates.push(a);
        } else {
            let root = cfg.data_root();
            if let Some(r) = &cfg.rendition {
                candidates.push(root.join(r));
            }
            candidates.push(root);
            if let Some(g) = std::env::var_os("F2_GAME_DIR") {
                candidates.push(PathBuf::from(g));
            }
            candidates.push(PathBuf::from("game-data"));
        }
        for c in candidates {
            if let Some(r) = find_rendition(&c) {
                return Ok(Data { vfs: Arc::new(Vfs::with_rendition(&r)), rendition: r });
            }
            tried.push(c.display().to_string());
        }
        Err(format!(
            "no imported game data found (looked in: {}).\n\
             Import your Frogger 2 CD first:  f2-import <disc image (.cue/.iso/.bin/.zip) or disc/install dir>\n\
             or point the game at existing data with --data <dir> or F2_DATA_ROOT.",
            tried.join(", ")
        ))
    }
}

fn find_rendition(p: &Path) -> Option<PathBuf> {
    if p.join("game").is_dir() {
        return Some(p.to_path_buf());
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(p).ok()?.flatten().map(|e| e.path()).filter(|d| d.join("game").is_dir()).collect();
    subs.sort();
    subs.into_iter().next()
}
