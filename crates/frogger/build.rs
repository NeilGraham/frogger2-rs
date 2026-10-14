// Windows: the exe's icon and version info (Explorer, Start menu, shortcuts until the game
// switches them to the player's own artwork, `src/appicon.rs`).
fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icon/swampys-revenge-recomp.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/icon/swampys-revenge-recomp.ico");
        res.set("ProductName", "Swampy's Revenge Recomp");
        res.set("FileDescription", "Swampy's Revenge Recomp");
        if let Err(e) = res.compile() {
            println!("cargo:warning=exe icon not embedded: {e}");
        }
    }
}
