// Windows: the exe's icon and version info (Explorer, Start menu, shortcuts until the game
// switches them to the player's own artwork, `src/appicon.rs`).
fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icon/frogger2-rs.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/icon/frogger2-rs.ico");
        res.set("ProductName", "Frogger 2: Swampy\u{2019}s Revenge (Rust)");
        res.set("FileDescription", "Frogger 2: Swampy\u{2019}s Revenge (Rust)");
        if let Err(e) = res.compile() {
            println!("cargo:warning=exe icon not embedded: {e}");
        }
    }
}
