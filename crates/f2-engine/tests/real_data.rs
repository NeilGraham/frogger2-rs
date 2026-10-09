//! Real-data render tests. Need the retail data (`$F2_GAME_DIR`, default
//! `game-data/usa`); skipped when absent. Output PNGs go to `target/`.

use f2_engine::load::{read_object_bank, read_tex_bank};
use f2_engine::{actor_bounds, find_model, Actor, Camera, Font, GpuContext, Renderer, RgbaImage};
use std::path::PathBuf;

fn game() -> Option<PathBuf> {
    let base = std::env::var("F2_GAME_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../game-data/usa").into());
    let g = PathBuf::from(base).join("game");
    if g.join("objects/garden1.dll").is_file() {
        Some(g)
    } else {
        eprintln!("skipping: retail data not found at {}", g.display());
        None
    }
}

fn out(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target").join(name)
}

fn coverage(img: &RgbaImage, bg: [u8; 4]) -> f32 {
    let n = img.pixels.as_chunks::<4>().0.iter().filter(|p| **p != bg).count();
    n as f32 / (img.width * img.height) as f32
}

fn renderer(g: &std::path::Path, banks: &[&str]) -> Renderer {
    let ctx = GpuContext::new_headless().expect("GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    for b in banks {
        r.load_tex_bank(&read_tex_bank(g.join("textures").join(b)).unwrap());
    }
    r.update_animating_textures(0);
    r
}

#[test]
fn garden_world_renders() {
    let Some(g) = game() else { return };
    let bank = read_object_bank(g.join("objects/garden1.dll")).unwrap();
    let world = find_model(&bank, "world").expect("world model");
    let mut r = renderer(&g, &["system.fla", "garden.fla"]);
    let (lo, hi) = actor_bounds(world, &mut Actor { animated: false, ..Default::default() }).unwrap();
    let c = [(lo[0] + hi[0]) / 2.0, lo[1], (lo[2] + hi[2]) / 2.0];
    let ext = (hi[0] - lo[0]).max(hi[2] - lo[2]);
    r.begin_frame(640, 480);
    r.set_camera(&Camera { source: [c[0], c[1] + ext * 0.55, c[2] - ext * 0.75], target: [c[0], c[1], c[2] - ext * 0.05], up: [0.0, 1.0, 0.0] });
    r.view.far_clip = ext * 3.0;
    r.draw_landscape(world);
    let tris = r.queued_tris();
    let img = r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap();
    img.write_png(out("f2-engine-garden.png")).unwrap();
    let cov = coverage(&img, [0, 0, 0, 255]);
    eprintln!("garden: {tris} tris, coverage {cov:.3}");
    assert!(tris > 1000, "{tris} triangles");
    assert!(cov > 0.15, "coverage {cov}");
    // Textured, not flat: many distinct colours.
    let mut cols: Vec<[u8; 4]> = img.pixels.as_chunks::<4>().0.to_vec();
    cols.sort_unstable();
    cols.dedup();
    assert!(cols.len() > 500, "{} distinct colours", cols.len());
}

#[test]
fn frog_renders_animated_with_text() {
    let Some(g) = game() else { return };
    let bank = read_object_bank(g.join("objects/frogger.dll")).unwrap();
    let frog = find_model(&bank, "frog").expect("frog model");
    assert!(frog.is_skinned);
    let mut r = renderer(&g, &["system.fla"]);
    let font = Font::from_bmp(&std::fs::read(g.join("textures/font/bigfont.bmp")).unwrap()).expect("bmp");
    assert!(font.glyphs[b'Z' as usize].is_some() && font.glyphs[b'9' as usize].is_some());
    let font = r.add_font(font);
    let mut actor = Actor { anim_time: 30.0, ..Default::default() };
    let (lo, hi) = actor_bounds(frog, &mut actor).unwrap();
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    r.begin_frame(320, 240);
    r.set_camera(&Camera { source: [c[0] + 60.0, c[1] + 45.0, c[2] + 100.0], target: c, up: [0.0, 1.0, 0.0] });
    r.draw_actor(frog, &mut actor);
    let end = r.draw_text(font, 4.0, 4.0, "FROGGER 2", [255, 255, 255, 255], 0.5, None);
    assert!(end > 20.0);
    let img = r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap();
    img.write_png(out("f2-engine-frog.png")).unwrap();
    // Green frog in the middle of the frame.
    let green = img.pixels.as_chunks::<4>().0.iter().filter(|p| p[1] > 100 && p[1] > p[0] && p[1] > p[2]).count();
    assert!(green > 2000, "{green} green pixels");
    // Text drew something in the top strip.
    let text = (0..20u32).flat_map(|y| (0..(end as u32)).map(move |x| (x, y))).filter(|&(x, y)| img.pixel(x, y) != [0, 0, 0, 255]).count();
    assert!(text > 50, "{text} text pixels");
}

#[test]
fn title_backdrop_fills_screen() {
    let Some(g) = game() else { return };
    let data = f2_formats::fla::maybe_decompress(std::fs::read(g.join("textures/title.fla")).unwrap()).unwrap();
    let t = f2_formats::texture::parse_backdrop(&data).unwrap();
    assert_eq!((t.width, t.height), (640, 480));
    let mut r = renderer(&g, &[]);
    let h = r.add_texture(&t);
    r.begin_frame(640, 480);
    r.draw_backdrop(h);
    let img = r.render_to_image([1.0, 0.0, 1.0, 1.0]).unwrap();
    img.write_png(out("f2-engine-title.png")).unwrap();
    assert!(coverage(&img, [255, 0, 255, 255]) > 0.99);
}
