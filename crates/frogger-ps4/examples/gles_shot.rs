//! Host check of the GLES2 back end (the PS4 Piglet path) against the CPU renderer:
//! `cargo run --release --manifest-path crates/frogger-ps4/Cargo.toml --example gles_shot --
//! DATA_DIR OUT_PREFIX [W L] [TICKS] [WIDTHxHEIGHT] [--compare OTHER.png]...`
//!
//! Renders one frame of a level after TICKS ticks (as `soft_shot`) with both back ends
//! from the same draw list, writes `OUT_PREFIX-gles.png` and `OUT_PREFIX-soft.png` and
//! prints difference statistics (also against each `--compare` image, e.g. a wgpu
//! screenshot from `frogger --headless --screenshot`). Needs Mesa's `libEGL.so.1`
//! (surfaceless, llvmpipe is fine). `--npot-emulation` forces the shader wrap path.

#[path = "common/egl.rs"]
mod egl;

use f2_audio::AudioEngine;
use f2_engine::gles::{self, GlesBackend};
use f2_platform::{Config, KeyboardState, Profile};
use frogger::app::{App, Step};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn read_png(path: &str) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let px = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()].as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        c => panic!("unsupported PNG colour type {c:?}"),
    };
    (info.width, info.height, px)
}

fn report(name: &str, a: &[u8], b: &[u8]) {
    let (mean, share, max) = egl::diff(a, b, 8);
    let (_, share2, _) = egl::diff(a, b, 32);
    println!("{name}: mean |d| {mean:.3}/255, pixels > 8: {:.3}%, > 32: {:.3}%, max {max}", share * 100.0, share2 * 100.0);
}

fn main() {
    let mut a: Vec<String> = std::env::args().skip(1).collect();
    let mut compare = Vec::new();
    while let Some(i) = a.iter().position(|s| s == "--compare") {
        compare.push(a.remove(i + 1));
        a.remove(i);
    }
    let npot_emulation = a.iter().position(|s| s == "--npot-emulation").map(|i| a.remove(i)).is_some();
    let data = frogger::data::Data::locate(&Config::default(), Some(PathBuf::from(&a[0]))).unwrap();
    let prefix = a[1].clone();
    let (w, l) = (a.get(2).map_or(0, |s| s.parse().unwrap()), a.get(3).map_or(0, |s| s.parse().unwrap()));
    let ticks: u32 = a.get(4).map_or(120, |s| s.parse().unwrap());
    let (width, height) = a.get(5).map_or((640, 480), |s| {
        let (x, y) = s.split_once('x').unwrap();
        (x.parse().unwrap(), y.parse().unwrap())
    });

    let host = egl::HostGl::new(width, height).unwrap_or_else(|e| panic!("no host GLES2 context: {e}"));
    let gl = host.fns();
    let program = gles::program_from_source(&gl).unwrap();
    let mut be = GlesBackend::new(gl, program).unwrap();
    be.force_npot_emulation = npot_emulation;
    eprintln!("GL: {}", be.describe());
    if let Some(bin) = gles::program_binary(&be.gl, program) {
        let p = gles::program_from_binary(&be.gl, &bin);
        eprintln!("program binary round trip: {} bytes, reload {}", bin.len(), if p.is_ok() { "ok" } else { "failed" });
    }

    let cfg = Config::default();
    let mut app = App::new_soft(data, AudioEngine::null(44100), cfg.input_mapper());
    app.set_display(cfg.profile, cfg.display_settings(Profile::Enhanced));
    app.set_config(cfg.clone());
    app.push_steps([Step::Level(w, l)]);
    app.update(Duration::ZERO, &KeyboardState::default(), &[], true);
    for _ in 0..ticks {
        app.tick(&[]);
        while app.video.is_some() {
            app.end_video();
        }
    }

    let n = 10;
    let t0 = Instant::now();
    for _ in 0..n {
        app.draw(width, height);
        app.r.render_gles(&mut be, width, height, app.clear_color());
        unsafe { (be.gl.glFinish)() };
    }
    eprintln!("gles: {:.2} ms/frame (llvmpipe), {} tris", t0.elapsed().as_secs_f64() * 1000.0 / n as f64, app.r.queued_tris());
    // One queued frame drawn by both back ends (drawing can advance animations).
    app.draw(width, height);
    app.r.render_gles(&mut be, width, height, app.clear_color());
    let gles_px = be.read_pixels(width, height);
    let soft = app.r.render_to_image(app.clear_color()).unwrap();

    let out = |suffix: &str, px: &[u8]| {
        let img = f2_engine::RgbaImage { width, height, pixels: px.to_vec() };
        let p = format!("{prefix}-{suffix}.png");
        img.write_png(&p).unwrap();
        eprintln!("wrote {p}");
    };
    out("gles", &gles_px);
    out("soft", &soft.pixels);
    report("gles vs soft", &gles_px, &soft.pixels);
    for c in compare {
        let (cw, ch, px) = read_png(&c);
        if (cw, ch) != (width, height) {
            println!("{c}: size {cw}x{ch} differs, skipped");
            continue;
        }
        report(&format!("gles vs {c}"), &gles_px, &px);
        report(&format!("soft vs {c}"), &soft.pixels, &px);
    }
}
