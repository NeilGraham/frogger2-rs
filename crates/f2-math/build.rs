//! Test-only: compiles the ORIGINAL `Common/fixed.c` + `Common/maths.c` (with stub headers
//! from `tests/csrc/`) into a static library that is linked only into integration tests
//! (`rustc-link-arg-tests`), for Rust-vs-C difftests. The library crate itself never links
//! it. If the sources or a C compiler are unavailable, the difftests are skipped
//! (cfg `f2_math_no_c`).
//!
//! The only source transformation: `2147483648L` -> `((unsigned int)2147483648U)` in
//! fixed.c, reproducing MSVC's typing of that literal (`unsigned long`, 32-bit) on an
//! LP64 host, where it would otherwise be a signed 64-bit `long`.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo::rustc-check-cfg=cfg(f2_math_no_c)");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=tests/csrc");
    println!("cargo:rerun-if-env-changed=F2_MATH_NO_C");

    if let Err(e) = build_c() {
        println!("cargo:warning=f2-math C difftest library not built ({e}); difftests skipped");
        println!("cargo:rustc-cfg=f2_math_no_c");
    }
}

fn build_c() -> Result<(), String> {
    if std::env::var_os("F2_MATH_NO_C").is_some() {
        return Err("F2_MATH_NO_C set".into());
    }
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86_64") || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        return Err("difftests need an x86_64 gcc/clang target".into());
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../../frogger2-vss/teamSpirit/FroggerProject");
    let common = root.join("Common");
    let pc = root.join("PC");
    let stubs = manifest.join("tests/csrc");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("csrc");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;

    let copy = |src: &Path, name: &str, xform: &dyn Fn(String) -> String| -> Result<(), String> {
        let p = src.join(name);
        println!("cargo:rerun-if-changed={}", p.display());
        let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        std::fs::write(out.join(name), xform(text)).map_err(|e| e.to_string())
    };
    let same = |s: String| s;
    copy(&common, "fixed.c", &|s: String| s.replace("2147483648L", "((unsigned int)2147483648U)"))?;
    copy(&common, "fixed.h", &same)?;
    // RETAIL: the exe runs on Direct3D's 24-bit x87 control word, so arccos rounds
    // every step to float (see `f2_math::arccos`); emulate that on the SSE host.
    copy(&common, "maths.c", &|s: String| {
        s.replace(
            "return (long)(acos(cos/4096.0F)*(4096/PI2));",
            "{ float _x = cos/4096.0F; float _a = (float)acos(_x); float _r = (float)(_a*(4096/PI2)); return (long)_r; }",
        )
    })?;
    copy(&common, "maths.h", &same)?;
    copy(&common, "types.c", &same)?;
    copy(&common, "newpsx.h", &same)?;
    copy(&pc, "libgte.h", &same)?;
    for e in std::fs::read_dir(&stubs).map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        let name = e.file_name().into_string().unwrap();
        copy(&stubs, &name, &same)?;
    }

    let lib = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("libf2mathc.a");
    cc::Build::new()
        .cargo_metadata(false)
        .include(&out)
        .flag("-include")
        .flag(out.join("prelude.h").to_str().unwrap())
        .flag("-fwrapv")
        .flag("-fno-strict-aliasing")
        .flag("-ffp-contract=off")
        .flag("-fno-builtin")
        .flag("-w")
        .opt_level(1)
        .file(out.join("fixed.c"))
        .file(out.join("maths.c"))
        .file(out.join("types.c"))
        .file(out.join("shim.c"))
        .try_compile("f2mathc")
        .map_err(|e| e.to_string())?;
    if !lib.exists() {
        return Err(format!("{} missing", lib.display()));
    }
    println!("cargo:rustc-link-arg-tests={}", lib.display());
    println!("cargo:rustc-link-arg-tests=-lm");
    Ok(())
}
