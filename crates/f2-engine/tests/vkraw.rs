//! The PS5 GPU path (`vkraw`, feature `ps5-vulkan`) on the host: a wgpu context built from
//! a raw `vkGetInstanceProcAddr` (the system loader's here, the statically linked RADV's on
//! the console) must render exactly what the normal loader path renders.
//!
//! `cargo test -p f2-engine --features ps5-vulkan --test vkraw`
#![cfg(feature = "ps5-vulkan")]

use f2_engine::vkraw::{ash, entry_from_gipa};
use f2_engine::{Blend, GpuContext, Quad2D, Renderer, RgbaImage};
use f2_formats::texture::TexType;

fn scene(ctx: &GpuContext) -> RgbaImage {
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let tex = f2_formats::crc("checker.bmp");
    let mut px = Vec::new();
    for i in 0..64u32 {
        let on = ((i % 8) / 4 + (i / 8) / 4) % 2 == 0;
        px.extend_from_slice(&if on { [250, 200, 40, 255] } else { [30, 60, 200, 255] });
    }
    let h = r.textures.add_rgba(tex, "checker.bmp", TexType::Normal, 8, 8, px);
    r.begin_frame(320, 240);
    r.draw_quad(Quad2D::rect(200.0, 300.0, 2000.0, 1800.0, [0.0, 0.0, 1.0, 1.0], Some(h), [255, 255, 255, 255], Blend::Semi));
    r.draw_sprite_overlay(1500.0, 1200.0, 1600.0, 1600.0, [0.0, 0.0, 1.0, 1.0], None, [255, 0, 0, 128], Blend::Semi, 0.4);
    r.draw_sprite_overlay(2600.0, 400.0, 900.0, 900.0, [0.0, 0.0, 1.0, 1.0], Some(h), [0, 255, 0, 255], Blend::Additive, 0.0);
    r.render_to_image([0.1, 0.2, 0.3, 1.0]).unwrap()
}

fn loader_entry() -> Option<ash::Entry> {
    // SAFETY: loads the system Vulkan loader.
    unsafe { ash::Entry::load() }.ok()
}

#[test]
fn raw_entry_renders_like_loader_path() {
    let Some(entry) = loader_entry() else {
        eprintln!("skipping: no Vulkan loader");
        return;
    };
    let raw = match GpuContext::from_vulkan_entry(entry, &[]) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping: no Vulkan device ({e})");
            return;
        }
    };
    assert_eq!(raw.adapter.get_info().backend, wgpu::Backend::Vulkan);
    let normal = GpuContext::new_headless().expect("headless GPU");
    if normal.adapter.get_info().backend != wgpu::Backend::Vulkan || normal.adapter.get_info().name != raw.adapter.get_info().name {
        eprintln!("skipping comparison: default adapter is {:?}", normal.adapter.get_info());
        return;
    }
    let (a, b) = (scene(&normal), scene(&raw));
    assert!(a.pixels == b.pixels, "raw-entry render differs from the loader path");
    // Not a blank frame.
    let first = &a.pixels[..4];
    let distinct = a.pixels.chunks(4).filter(|p| *p != first).count();
    assert!(distinct > 1000, "frame is (nearly) blank: {distinct} pixels differ from the clear colour");
}

/// The static-driver entry point type works with a function pointer (on the PS5 it is the
/// RADV archive's `vk_icdGetInstanceProcAddr`); here the loader's own is passed through it.
#[test]
fn entry_from_gipa_function_pointer() {
    let Some(entry) = loader_entry() else {
        eprintln!("skipping: no Vulkan loader");
        return;
    };
    let gipa = entry.static_fn().get_instance_proc_addr;
    // SAFETY: the loader stays loaded (`entry` is alive) for the rest of the test.
    let static_entry = unsafe { entry_from_gipa(gipa) };
    let v = unsafe { static_entry.try_enumerate_instance_version() }.unwrap();
    assert!(v.unwrap_or(ash::vk::API_VERSION_1_0) >= ash::vk::API_VERSION_1_0);
    if let Ok(ctx) = GpuContext::from_vulkan_entry(static_entry, &[]) {
        let img = scene(&ctx);
        assert_eq!((img.width, img.height), (320, 240));
    }
    drop(entry);
}
