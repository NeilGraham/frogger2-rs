//! `cargo run -p f2-engine --example render_level -- <bank.dll> out.png [--tex bank.fla]... [--camera sx,sy,sz,tx,ty,tz]`
//!
//! Draws the bank's `world` model as landscape (and `backdrop` sky if present)
//! from a camera framing the level.

mod common;

use f2_engine::{actor_bounds, find_model, Actor, Camera, GpuContext, Renderer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut args, tex) = common::parse_args();
    let mut cam_override = None;
    if let Some(i) = args.iter().position(|a| a == "--camera") {
        let v: Vec<f32> = args[i + 1].split(',').map(|s| s.parse()).collect::<Result<_, _>>()?;
        cam_override = Some(Camera { source: [v[0], v[1], v[2]], target: [v[3], v[4], v[5]], up: [0.0, 1.0, 0.0] });
        args.drain(i..i + 2);
    }
    if args.len() < 2 {
        eprintln!("usage: render_level <bank.dll> out.png [--tex bank.fla]... [--camera sx,sy,sz,tx,ty,tz]");
        std::process::exit(2);
    }
    let bank_path = std::path::PathBuf::from(&args[0]);
    let bank = f2_engine::load::read_object_bank(&bank_path)?;
    let world = find_model(&bank, "world").ok_or("bank has no `world` model")?;

    let ctx = GpuContext::new_headless()?;
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    common::load_textures(&mut r, &bank_path, &tex);
    r.update_animating_textures(0);

    let (lo, hi) = actor_bounds(world, &mut Actor { animated: false, ..Default::default() }).ok_or("world has no geometry")?;
    let c = [(lo[0] + hi[0]) / 2.0, lo[1], (lo[2] + hi[2]) / 2.0];
    let ext = (hi[0] - lo[0]).max(hi[2] - lo[2]);
    let cam =
        cam_override.unwrap_or(Camera { source: [c[0], c[1] + ext * 0.55, c[2] - ext * 0.75], target: [c[0], c[1], c[2] - ext * 0.05], up: [0.0, 1.0, 0.0] });
    eprintln!("world bounds {lo:?}..{hi:?}; camera {cam:?}");

    r.begin_frame(640, 480);
    r.set_camera(&cam);
    r.view.far_clip = ext * 3.0;
    r.options.screen_reject = std::env::var("F2_SCREEN_REJECT").map_or(true, |v| v != "0");
    if let Some(sky) = find_model(&bank, "backdrop") {
        // DrawBackground: sky actor centred on the camera, far plane ignored.
        r.view.no_clip = true;
        r.draw_actor(sky, &mut Actor::at(cam.source));
        r.view.no_clip = false;
    }
    r.draw_landscape(world);
    eprintln!("{} triangles queued", r.queued_tris());
    let img = r.render_to_image([0.0, 0.0, 0.0, 1.0])?;
    img.write_png(&args[1])?;
    eprintln!("wrote {}", args[1]);
    Ok(())
}
