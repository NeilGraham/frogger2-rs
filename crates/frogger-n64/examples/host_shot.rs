//! Host-side check of the N64 front end's drawing: runs the simulation like `src/app.rs`,
//! draws with the shared `f2-draw` loop at 320x240 and renders with `f2-engine`'s CPU
//! rasteriser, optionally after passing every texture through the N64 texel conversion
//! (`--n64`: RGBA16/RGBA32 round trip, TMEM-sized copies for 3D as `render_cpu.rs` uses
//! them). `--cmds` prints the draw list (blend, depth, texture name/size, triangles).
//!
//! ```sh
//! cargo +nightly run --release --manifest-path crates/frogger-n64/Cargo.toml --example host_shot -- \
//!     GAME_DIR out.png garden|book [TICKS] [--n64] [--cmds]
//! ```

use f2_draw::overlay::GameFonts;
use f2_draw::render::{self, DrawCtx, LevelScene};
use f2_engine::{Font, Renderer};
use f2_game::assets::{DataSource, DirData};
use f2_game::levplay::{PAD_RIGHT, PAD_UP};
use f2_game::session::SessionConfig;
use f2_game::{Game, TickInput};
use frogger_n64::texconv;
use std::path::PathBuf;
use std::sync::Arc;

fn decode(t: &texconv::N64Tex) -> Vec<u8> {
    if t.fmt & 0xff == texconv::FMT_RGBA32 {
        return t.data.clone();
    }
    let mut out = Vec::with_capacity(t.data.len() * 2);
    for c in t.data.as_chunks::<2>().0 {
        let v = u16::from_be_bytes(*c);
        let e = |x: u16| ((x << 3) | (x >> 2)) as u8;
        out.extend_from_slice(&[e((v >> 11) & 31), e((v >> 6) & 31), e((v >> 1) & 31), if v & 1 != 0 { 255 } else { 0 }]);
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(&a[0]);
    let out = PathBuf::from(&a[1]);
    let what = a.get(2).map_or("garden", |s| s.as_str()).to_string();
    let ticks: u32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(300);
    let n64 = a.iter().any(|s| s == "--n64");
    let cmds = a.iter().any(|s| s == "--cmds");
    let data = Arc::new(DirData(dir));
    let mut g = Game::new_session(SessionConfig::new(data.clone()));
    let fe = f2_game::layout::worldid::FRONTEND as usize;
    g.layout.world_visual_data[fe].level_visual_data[f2_game::layout::LEVELID_FRONTEND4 as usize].level_completed = 1;
    if what == "book" {
        g.start_frontend();
    } else {
        g.start_arcade(0, 0);
    }
    let book_font = data.read("textures/font/monofont.bmp").and_then(|b| Font::from_bmp(&b));
    let mut r = Renderer::new_soft();
    let mut scene: Option<LevelScene> = None;
    let mut fonts = None;
    let mut converted = 0usize;
    for t in 0..ticks {
        let pad = match (what.as_str(), t) {
            ("book", 60) => PAD_RIGHT,
            ("book", 200) => PAD_UP,
            _ => 0,
        };
        g.tick(TickInput { pads: [pad, 0, 0, 0] });
        g.take_effects();
        if let Some(l) = g.take_level_loads().pop() {
            let banks = l.assets.object_banks.iter().filter_map(|f| data.object_bank(f)).filter_map(|b| b.ok()).collect();
            scene = Some(LevelScene::new(l.world, l.level, banks, l.assets.texture_banks.clone(), &g));
            r = Renderer::new_soft();
            let ld = |p: &str| data.read(p).and_then(|b| Font::from_bmp(&b));
            fonts = match (ld("textures/font/bigfont.bmp"), ld("textures/font/smallfont.bmp")) {
                (Some(b), Some(s)) => Some(GameFonts { normal: r.add_font(b), small: r.add_font(s) }),
                _ => None,
            };
            for f in &l.assets.texture_banks {
                if let Some(Ok(b)) = data.texture_bank(f) {
                    r.load_tex_bank(&b);
                }
            }
            converted = 0;
        }
        f2_draw::book::process_grab(&mut r, &mut g, book_font.as_ref(), 1);
        if let Some(s) = &scene {
            let far = render::far_clip(&g, s);
            g.specfx.view = Some(render::spec_fx_view(&g, far));
        }
    }
    let Some(s) = scene.as_mut() else { panic!("no level loaded") };
    r.begin_frame(320, 240);
    let ctx = DrawCtx {
        prev: &[],
        alpha: 1.0,
        cam: render::game_camera(&g),
        far_clip: render::far_clip(&g, s),
        show_tiles: false,
        world_only: false,
        fonts,
        authentic: true,
        backdrop: None,
        tile_scroll: Default::default(),
    };
    render::draw_loop(&mut r, Some(s), &g, &ctx);
    if n64 {
        // Bank images keep only their TMEM-sized copy; images added/replaced later (fonts,
        // proc textures, book pages) keep the full one (2D) — show what triangles sample.
        let banked: std::collections::BTreeSet<u32> = r.textures.entries.iter().map(|e| e.image.0).collect();
        for (i, img) in r.textures.images.iter_mut().enumerate().skip(converted) {
            if img.rgba.len() != (img.width * img.height * 4) as usize || (img.width == 1 && img.height == 1) {
                continue;
            }
            let (full, small) = texconv::convert(img.width, img.height, &img.rgba);
            let t = if banked.contains(&(i as u32)) { small.unwrap_or(full) } else { full };
            img.width = t.w;
            img.height = t.h;
            img.rgba = decode(&t);
        }
        converted = r.textures.images.len();
    }
    let _ = converted;
    if cmds {
        let names: std::collections::BTreeMap<u32, String> = r.textures.entries.iter().map(|e| (e.image.0, e.name.clone())).collect();
        let (verts, list) = r.build([0.0, 0.0, 0.0, 1.0]);
        for c in &list {
            let img = &r.textures.images[c.tex.0 as usize];
            let col = verts[c.first as usize].color;
            println!(
                "{:?} {:?} scene={} wrap={} tex={} {}x{} tris={} rgba0=[{:.2},{:.2},{:.2},{:.2}]",
                c.blend,
                c.depth,
                c.scene,
                c.wrap,
                names.get(&c.tex.0).map_or("-", |s| s.as_str()),
                img.width,
                img.height,
                c.count / 3,
                col[0],
                col[1],
                col[2],
                col[3]
            );
        }
        r.begin_frame(320, 240);
        render::draw_loop(&mut r, Some(s), &g, &ctx);
    }
    let img = r.render_to_image(s.clear).unwrap();
    img.write_png(&out).unwrap();
    eprintln!("wrote {}", out.display());
}
