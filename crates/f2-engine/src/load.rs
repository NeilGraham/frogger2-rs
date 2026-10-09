//! Small file helpers for examples, tools and tests.

use crate::renderer::EngineError;
use f2_formats::objbank::ObjectBank;
use f2_formats::texture::TexBank;
use std::path::Path;

/// Reads and (if FLA2-compressed) decompresses a texture bank file.
pub fn read_tex_bank(path: impl AsRef<Path>) -> Result<TexBank, EngineError> {
    let p = path.as_ref();
    let data = std::fs::read(p).map_err(|e| EngineError(format!("{}: {e}", p.display())))?;
    let data = f2_formats::fla::maybe_decompress(data).map_err(|e| EngineError(format!("{}: {e}", p.display())))?;
    TexBank::parse(&data).map_err(|e| EngineError(format!("{}: {e}", p.display())))
}

/// Reads and parses an object bank DLL (data only; never executed).
pub fn read_object_bank(path: impl AsRef<Path>) -> Result<ObjectBank, EngineError> {
    let p = path.as_ref();
    let data = std::fs::read(p).map_err(|e| EngineError(format!("{}: {e}", p.display())))?;
    ObjectBank::parse(&data).map_err(|e| EngineError(format!("{}: {e}", p.display())))
}

/// Texture banks a given object bank most likely needs, by name prefix
/// (`garden1.dll` → `garden.fla`), always including `system.fla`.
pub fn default_tex_banks(object_bank: &str) -> Vec<&'static str> {
    let n = object_bank.to_ascii_lowercase();
    let n = n.rsplit(['/', '\\']).next().unwrap_or(&n);
    let world = [
        ("garden", "garden.fla"),
        ("ancient", "ancient.fla"),
        ("city", "city.fla"),
        ("halloween", "halloween.fla"),
        ("lab", "lab.fla"),
        ("space", "space.fla"),
        ("sub", "subter.fla"),
        ("sretro", "super.fla"),
        ("title", "titles.fla"),
    ];
    let mut v = vec!["system.fla"];
    if let Some((_, b)) = world.iter().find(|(p, _)| n.starts_with(p)) {
        v.push(b);
    }
    v
}

/// The game directory: `$F2_GAME_DIR/game` (default `game-data/usa/game`).
pub fn game_dir() -> std::path::PathBuf {
    let base = std::env::var("F2_GAME_DIR").unwrap_or_else(|_| "game-data/usa".into());
    Path::new(&base).join("game")
}
