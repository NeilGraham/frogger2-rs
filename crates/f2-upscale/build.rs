//! Builds `native/f2_upscale.cpp` for Windows MSVC targets (the Streamline and FidelityFX
//! headers are in `native/`; their libraries are loaded at run time, nothing is linked).

fn main() {
    println!("cargo:rerun-if-changed=native");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os != "windows" || env != "msvc" {
        return;
    }
    cc::Build::new()
        .cpp(true)
        .file("native/f2_upscale.cpp")
        .include("native")
        .include("native/streamline")
        .flag("/std:c++17")
        .flag("/EHsc")
        .define("WIN32_LEAN_AND_MEAN", None)
        .define("NOMINMAX", None)
        // ffx_api_loader.h only defines its loader under MSBuild's `_WINDOWS`.
        .define("_WINDOWS", None)
        .compile("f2_upscale");
    for lib in ["d3d12", "dxgi"] {
        println!("cargo:rustc-link-lib={lib}");
    }
}
