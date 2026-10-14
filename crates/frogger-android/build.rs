// cpal's Android backend (oboe) is C++: link the NDK's C++ runtime statically, so the APK
// needs no libc++_shared.so. Plain `-l` (not `static=`): clang finds them in the NDK sysroot.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-lib=c++_static");
        println!("cargo:rustc-link-lib=c++abi");
    }
}
