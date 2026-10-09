//! `cargo run -p f2-engine --example render_model -- <bank.dll> <model-name-or-index> out.png [anim_frame] [--tex bank.fla]...`

mod common;

use f2_engine::{actor_bounds, find_model, Actor, Camera, GpuContext, Renderer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (args, tex) = common::parse_args();
    if args.len() < 3 {
        eprintln!("usage: render_model <bank.dll> <model-name-or-index> out.png [anim_frame] [--tex bank.fla]...");
        std::process::exit(2);
    }
    let bank_path = std::path::PathBuf::from(&args[0]);
    let bank = f2_engine::load::read_object_bank(&bank_path)?;
    let Some(model) = find_model(&bank, &args[1]) else {
        eprintln!("model {:?} not found; models:", args[1]);
        for (i, m) in bank.models.iter().enumerate() {
            eprintln!("  {i}: {} (skinned={}, anims={})", m.name(), m.is_skinned, m.animations.len());
        }
        std::process::exit(1);
    };
    let frame: f32 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(0.0);

    let ctx = GpuContext::new_headless()?;
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    common::load_textures(&mut r, &bank_path, &tex);
    r.update_animating_textures(frame as u32);

    let mut actor = Actor { anim_time: frame, ..Default::default() };
    let (lo, hi) = actor_bounds(model, &mut actor).ok_or("model has no geometry")?;
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let rad = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt() / 2.0;
    let d = (rad * 2.6).max(40.0);
    let cam = Camera { source: [c[0] + d * 0.5, c[1] + d * 0.35, c[2] + d * 0.8], target: c, up: [0.0, 1.0, 0.0] };
    eprintln!("model {} bounds {lo:?}..{hi:?}, camera {cam:?}", model.name());

    r.begin_frame(640, 480);
    r.view.far_clip = d * 4.0;
    r.set_camera(&cam);
    r.draw_actor(model, &mut actor);
    eprintln!("{} triangles queued", r.queued_tris());
    let img = r.render_to_image([0.15, 0.2, 0.3, 1.0])?;
    img.write_png(&args[2])?;
    eprintln!("wrote {}", args[2]);
    Ok(())
}
