//! Paths and settings shared by every step.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Ctx {
    /// Rendition root (contains `game/`, `music/`, `video/`), e.g. `game-data/usa`.
    pub rendition: PathBuf,
    /// Output DFS tree, e.g. `target/n64/pack`.
    pub out: PathBuf,
    /// Cache for slow media encodes (keyed by input + parameters), e.g. `target/n64/cache`.
    pub cache: PathBuf,
    /// libdragon tool directory (`$N64_INST/bin`).
    pub tools: PathBuf,
    /// Parallel jobs for external encoders.
    pub jobs: usize,
    /// Fast mode for development: cheaper encoder settings, may skip FMV/music search.
    pub quick: bool,
}

impl Ctx {
    /// `game/` inside the rendition.
    pub fn game(&self) -> PathBuf {
        self.rendition.join("game")
    }

    /// Absolute path of a file inside the output pack (creating parent dirs).
    pub fn out_path(&self, rel: &str) -> std::io::Result<PathBuf> {
        let p = self.out.join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        Ok(p)
    }

    /// A cache directory for a step (created).
    pub fn cache_dir(&self, step: &str) -> std::io::Result<PathBuf> {
        let p = self.cache.join(step);
        std::fs::create_dir_all(&p)?;
        Ok(p)
    }

    pub fn tool(&self, name: &str) -> PathBuf {
        self.tools.join(name)
    }
}

/// Default libdragon tool directory: `$N64_INST/bin`, else `~/opt/n64root/opt/libdragon/bin`.
pub fn default_tools() -> PathBuf {
    if let Some(i) = std::env::var_os("N64_INST") {
        return PathBuf::from(i).join("bin");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("opt/n64root/opt/libdragon/bin")
}

/// Finds the imported rendition for tests and defaults: `F2_GAME_DIR` (either the rendition
/// root or its `game/`), else the first `game-data/usa` found walking up from `start`
/// (works from git worktrees nested inside the main checkout).
pub fn find_rendition(start: &Path) -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("F2_GAME_DIR") {
        let d = PathBuf::from(d);
        let root = if d.ends_with("game") { d.parent().map(Path::to_path_buf).unwrap_or(d) } else { d };
        return root.join("game").is_dir().then_some(root);
    }
    let mut cur = Some(start);
    while let Some(c) = cur {
        let cand = c.join("game-data/usa");
        if cand.join("game").is_dir() {
            return Some(cand);
        }
        cur = c.parent();
    }
    None
}
