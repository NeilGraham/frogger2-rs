//! Android host for the Frogger 2 port (`docs/ANDROID.md`).
//!
//! `android_main` (NativeActivity, through winit's `android-activity`) runs the same
//! [`frogger::app::App`] as the desktop executable: wgpu on Vulkan or GLES, cpal on AAudio,
//! on-screen touch controls ([`touch`]) and Android gamepads as player 1's joypad. Game
//! data, settings and saves live in the app's external files directory
//! (`Android/data/<package>/files/`); with no data there, an import screen imports a disc
//! image copied into that directory.

pub mod touch;

#[cfg(target_os = "android")]
mod host;
pub mod import;
#[cfg(target_os = "android")]
mod logcat;

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    host::main(app);
}
