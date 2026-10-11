//! Host check of the CPU renderer: `cargo run --release --manifest-path
//! crates/frogger-ps4/Cargo.toml --example soft_shot -- DATA_DIR OUT.png [W L] [TICKS]` renders a level after TICKS ticks like
//! `frogger --headless --screenshot` does with wgpu, for side-by-side comparison.

use f2_audio::AudioEngine;
use f2_platform::{Config, KeyboardState, Profile};
use frogger::app::{App, Step};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let data = frogger::data::Data::locate(&Config::default(), Some(PathBuf::from(&a[0]))).unwrap();
    let out = PathBuf::from(&a[1]);
    let (w, l) = (a.get(2).map_or(0, |s| s.parse().unwrap()), a.get(3).map_or(0, |s| s.parse().unwrap()));
    let ticks: u32 = a.get(4).map_or(120, |s| s.parse().unwrap());
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
    app.draw(640, 480);
    let mut t = f2_engine::SoftTarget::default();
    let t0 = Instant::now();
    let n = 20;
    for _ in 0..n {
        app.draw(640, 480);
        app.r.render_soft(&mut t, app.clear_color(), 4);
    }
    eprintln!("soft: {:.2} ms/frame (4 threads), {} tris", t0.elapsed().as_secs_f64() * 1000.0 / n as f64, app.r.queued_tris());
    app.draw(640, 480);
    let img = app.r.render_to_image(app.clear_color()).unwrap();
    img.write_png(&out).unwrap();
}
