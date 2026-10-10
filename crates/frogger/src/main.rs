//! `frogger`: the Frogger 2 port executable.
//!
//! ```text
//! frogger [--data DIR] [--mute] [--level W L | --frontend | --multi W L N] [--size WxH] [--fullscreen] [display options]
//! frogger --headless --frames N [--level W L | --frontend | --multi W L N] [--size WxH] --screenshot out.png [display options]
//! mods: [--mods ID,ID...] [--mods-dir DIR] (default: config.toml [mods]; see docs/modding.md)
//! display options: --profile authentic|enhanced, --msaa N, --render-scale S, --hud-probe
//! ```
//! No arguments (windowed) runs the game's own start-up (`--boot`: title, logo FMVs,
//! frontend); dev shortcuts: `--level W L` starts `INGAME_MODE` directly on that level,
//! `--frontend` the frontend level (start screen), `--multi W L N` local multiplayer with
//! N frogs, `--demo W L` a recorded demo, `--video NAME` one FMV. Headless without a level option loads Garden 1.
//! `--pad-script FILE` drives player 1's pad: lines `TICK BUTTONS` hold the buttons
//! (`cross+up`, `none`, or a hex mask) from that tick on; `TICK shot FILE.png` writes a
//! screenshot before that tick (headless).
//!
//! Online play (`docs/NETPLAY.md`): `frogger --host [--port N] [--host-addr IP] --multi W L N`
//! hosts an N-player match and prints a join code; `frogger --join CODE` joins it. With
//! `--headless --frames F` both run F ticks of scripted pads (`--pad-seed S`) without a window
//! and print the final state checksum (`NETPLAY_FINAL ...`).
//! `--editor [--level W L]` opens the level editor (`src/editor/`; Ctrl+E toggles it in
//! dev builds); `--mod ID` / `--mods-dir DIR` stack mods over the data.
//! Keys: F1 free camera, F2 tile overlay, F10 controls (rebinding), F11 fullscreen, Esc quit (Enter/Esc skip videos);
//! Keys: F1 free camera, F2 tile overlay, F10 settings (controls / gameplay / level select;
//! Tab switches page), F11 fullscreen, Esc quit (Enter/Esc skip videos; any key in the
//! enhanced profile);
//! display A/B (`docs/enhancements.md`): F5 profile, F3 perspective-correct, F4 render
//! scale, F6 MSAA, F7 texture filter, F8 widescreen, F9 fog, F12 draw distance.

mod app;
mod book;
mod controls;
mod data;
mod mods;
mod editor;
mod fx;
mod netplay;
mod gameplay;
mod overlay;
mod padscript;
mod render;
mod video;

use app::{App, NetRequest, Step};
use f2_audio::AudioEngine;
use f2_engine::{GpuContext, WindowSurface};
use f2_platform::bindings::auto_assign;
use f2_platform::gamepad::{Gamepads, PadEvent};
use f2_platform::window::{self, winit, Flow, WindowApp, WindowConfig};
use f2_platform::{Config, Profile};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Args {
    data: Option<PathBuf>,
    mute: bool,
    level: Option<(u32, u32)>,
    frontend: bool,
    boot: bool,
    multi: Option<(u32, u32, i32)>,
    demo: Option<(u32, u32)>,
    choice: Option<gameplay::LevelChoice>,
    screenshot_settings: Option<gameplay::Tab>,
    headless: bool,
    frames: u32,
    screenshot: Option<PathBuf>,
    size: Option<(u32, u32)>,
    fullscreen: bool,
    profile: Option<Profile>,
    msaa: Option<u32>,
    render_scale: Option<f32>,
    hud_probe: bool,
    screenshot_rebind: bool,
    pad_script: Option<PathBuf>,
    video: Option<String>,
    save: Option<PathBuf>,
    dev_unlock: bool,
    host: bool,
    port: u16,
    host_addr: Option<std::net::IpAddr>,
    join: Option<String>,
    pad_seed: u32,
    /// `--mods a,b` (replaces `[mods] enabled`; `--mods ""` = none).
    mods: Option<Vec<String>>,
    mods_dir: Option<PathBuf>,
    editor: bool,
    editor_cmds: Vec<String>,
}

fn usage() -> ! {
    eprintln!(
        "usage: frogger [--data DIR] [--mute] [--level WORLD LEVEL | --frontend | --boot | --multi WORLD LEVEL FROGS | --demo WORLD LEVEL | --video NAME] [--size WxH] [--fullscreen]\n\
         \x20      frogger --headless --frames N [level options] [--screenshot out.png] [--size WxH]\n\
         display: [--profile authentic|enhanced] [--msaa N] [--render-scale S] [--hud-probe] [--screenshot-rebind]\n\
         \x20      [--pad-script FILE]  (lines `TICK BUTTONS` / `TICK shot FILE.png`)\n\
         online: --host [--port N] [--host-addr IP] --multi WORLD LEVEL PLAYERS | --join CODE  [--pad-seed S (headless)]\n\
         mods: [--mods ID,ID...] [--mod ID]... (default: config [mods] enabled; \"\" = none) [--mods-dir DIR]\n\
         practice: [--practice WORLD LEVEL | --story CHAPTER] (level select through a game session)\n\
         \x20      [--screenshot-settings controls|gameplay|levels] (headless: F10 page in the screenshot)\n\
         editor: --editor [--level W L] [--editor-cmd CMDS]... (saves to the first --mods/--mod id, else level-W-L, in the mods dir)\n\
         saves: [--save FILE] (default: config dir; headless: none) [--dev-unlock] (start with everything unlocked, not saved)\n\
         keys: F1 free camera, F2 tile overlay, F10 settings (Tab: controls/gameplay/level select), F11 fullscreen, Esc quit\n\
         \x20     F5 profile, F3 persp. textures, F4 render scale, F6 MSAA, F7 filter, F8 widescreen,\n\
         \x20     F9 fog, F12 draw distance"
    );
    std::process::exit(2);
}

fn parse_args() -> Args {
    let mut a = Args {
        data: None,
        mute: false,
        level: None,
        frontend: false,
        boot: false,
        multi: None,
        demo: None,
        choice: None,
        screenshot_settings: None,
        headless: false,
        frames: 120,
        screenshot: None,
        size: None,
        fullscreen: false,
        profile: None,
        msaa: None,
        render_scale: None,
        hud_probe: false,
        screenshot_rebind: false,
        pad_script: None,
        video: None,
        save: None,
        dev_unlock: false,
        host: false,
        port: netplay::DEFAULT_PORT,
        host_addr: None,
        join: None,
        pad_seed: 1,
        mods: None,
        mods_dir: None,
        editor: false,
        editor_cmds: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    let num = |s: Option<String>| -> u32 { s.and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()) };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data" => a.data = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--mute" => a.mute = true,
            "--level" => {
                let w = num(it.next());
                let l = num(it.next());
                a.level = Some((w, l));
            }
            "--frontend" => a.frontend = true,
            "--save" => a.save = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--dev-unlock" => a.dev_unlock = true,
            "--host" => a.host = true,
            "--port" => a.port = num(it.next()).try_into().unwrap_or_else(|_| usage()),
            "--host-addr" => a.host_addr = Some(it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage())),
            "--join" => a.join = Some(it.next().unwrap_or_else(|| usage())),
            "--pad-seed" => a.pad_seed = num(it.next()),
            "--mods" => a.mods = Some(it.next().unwrap_or_else(|| usage()).split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()),
            "--mods-dir" => a.mods_dir = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--mod" => a.mods.get_or_insert_with(Vec::new).push(it.next().unwrap_or_else(|| usage())), // alias of --mods (stacks)
            "--editor" => a.editor = true,
            "--editor-cmd" => a.editor_cmds.push(it.next().unwrap_or_else(|| usage())),
            "--video" => a.video = Some(it.next().unwrap_or_else(|| usage())),
            "--boot" => a.boot = true,
            "--multi" => {
                let w = num(it.next());
                let l = num(it.next());
                let n = num(it.next());
                a.multi = Some((w, l, n as i32));
            }
            "--demo" => {
                let w = num(it.next());
                let l = num(it.next());
                a.demo = Some((w, l));
            }
            "--practice" => {
                let w = num(it.next());
                let l = num(it.next());
                a.choice = Some(gameplay::LevelChoice::Arcade(w, l));
            }
            "--story" => a.choice = Some(gameplay::LevelChoice::Story(num(it.next()))),
            "--screenshot-settings" => {
                a.screenshot_settings = Some(match it.next().as_deref() {
                    Some("controls") => gameplay::Tab::Controls,
                    Some("gameplay") => gameplay::Tab::Gameplay,
                    Some("levels") => gameplay::Tab::Levels,
                    _ => usage(),
                })
            }
            "--headless" => a.headless = true,
            "--frames" => a.frames = num(it.next()),
            "--screenshot" => a.screenshot = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--size" => {
                let v = it.next().unwrap_or_else(|| usage());
                let (w, h) = v.split_once('x').unwrap_or_else(|| usage());
                a.size = Some((w.parse().unwrap_or_else(|_| usage()), h.parse().unwrap_or_else(|_| usage())));
            }
            "--fullscreen" => a.fullscreen = true,
            "--profile" => {
                a.profile = Some(match it.next().as_deref() {
                    Some("authentic") => Profile::Authentic,
                    Some("enhanced") => Profile::Enhanced,
                    _ => usage(),
                })
            }
            "--msaa" => a.msaa = Some(num(it.next())),
            "--render-scale" => a.render_scale = Some(it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage())),
            "--hud-probe" => a.hud_probe = true,
            "--screenshot-rebind" => a.screenshot_rebind = true,
            "--pad-script" => a.pad_script = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    a
}

fn main() {
    let args = parse_args();
    let mut cfg = Config::load_default().unwrap_or_else(|e| {
        eprintln!("frogger: config: {e} (using defaults)");
        Config::default()
    });
    // Command-line display overrides.
    if let Some(p) = args.profile {
        cfg.profile = p;
    }
    if let Some(m) = args.msaa {
        cfg.display.msaa = Some(m);
    }
    if let Some(s) = args.render_scale {
        cfg.display.render_scale = Some(s);
    }
    let mut data = match data::Data::locate(&cfg, args.data.clone()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("frogger: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("frogger: data {}", data.rendition.display());
    let mod_ids = args.mods.clone().unwrap_or_else(|| cfg.mods.enabled.clone());
    let mods_dir = args.mods_dir.clone().unwrap_or_else(|| cfg.mods_dir());
    if let Err(e) = data.enable_mods(&mods_dir, &mod_ids) {
        eprintln!("frogger: {e}");
        std::process::exit(1);
    }

    let net = match (&args.join, args.host) {
        (Some(_), true) => usage(),
        (Some(code), false) => Some(NetRequest::Join { code: code.clone() }),
        (None, true) => {
            let Some((world, level, players)) = args.multi else {
                eprintln!("frogger: --host needs --multi WORLD LEVEL PLAYERS");
                std::process::exit(2);
            };
            Some(NetRequest::Host { world, level, players: players.clamp(2, 4) as u8, port: args.port, ip: args.host_addr })
        }
        (None, false) => None,
    };
    if let (Some(req), true) = (&net, args.headless) {
        if let Err(e) = headless_net(&data, req.clone(), &args) {
            eprintln!("frogger: netplay: {e}");
            std::process::exit(1);
        }
        return;
    }

    let steps: Vec<Step> = match args.level {
        _ if net.is_some() => Vec::new(),
        l if args.editor => {
            let (w, l) = l.unwrap_or((0, 0));
            vec![Step::Editor(w, l)]
        }
        _ if args.video.is_some() => vec![Step::Video(args.video.clone().unwrap())],
        _ if args.frontend => vec![Step::Frontend],
        _ if args.choice.is_some() => vec![Step::Choice(args.choice.unwrap())],
        _ if args.boot => vec![Step::Boot],
        _ if args.demo.is_some() => {
            let (w, l) = args.demo.unwrap();
            vec![Step::Demo(w, l)]
        }
        _ if args.multi.is_some() => {
            let (w, l, n) = args.multi.unwrap();
            vec![Step::Multi(w, l, n)]
        }
        Some((w, l)) => vec![Step::Level(w, l)],
        None if args.headless => vec![Step::Level(0, 0)],
        // The game's own start-up: title, logo FMVs, frontend.
        None => vec![Step::Boot],
    };
    let res = if args.headless { headless(data, &cfg, &args, steps) } else { run_window(data, cfg, &args, steps, net) };
    if let Err(e) = res {
        eprintln!("frogger: {e}");
        std::process::exit(1);
    }
}

/// `--dev-unlock`: a save block (`MakeSaveGameBlock`) with every level open and
/// completed and the training done (arcade machine, multiplayer, book pages).
fn dev_unlock_save() -> Vec<u8> {
    let mut g = f2_game::Game::new();
    g.savegame.global_music_vol = f2_game::menus::MAX_SOUND_VOL;
    g.savegame.global_sound_vol = f2_game::menus::MAX_SOUND_VOL;
    g.frogger.player[0].lives = 3;
    for w in g.layout.world_visual_data.iter_mut() {
        for l in w.level_visual_data.iter_mut() {
            l.level_open = f2_game::layout::LEVEL_OPEN;
            l.level_completed = 1;
        }
    }
    g.make_save_game_block()
}

fn make_audio(cfg: &Config, mute: bool) -> AudioEngine {
    let mut a = if mute || cfg.audio.muted { AudioEngine::null(44100) } else { AudioEngine::with_default_output() };
    a.set_master_volume(cfg.audio.master as f32 / 100.0);
    a.set_music_volume(cfg.audio.music as i32);
    a.set_sfx_volume(cfg.audio.sfx as i32);
    a
}

/// Where the editor saves: `--mods-dir` (else config `[mods] dir`), and the first `--mods`/`--mod` id
/// (else `level-W-L`).
fn editor_target(args: &Args, cfg: &Config) -> (PathBuf, String) {
    let (w, l) = args.level.unwrap_or((0, 0));
    let dir = args.mods_dir.clone().unwrap_or_else(|| cfg.mods_dir());
    (dir, args.mods.as_ref().and_then(|m| m.first().cloned()).unwrap_or_else(|| format!("level-{w}-{l}")))
}

// ------------------------------------------------------------------ headless

/// `--headless` with `--host`/`--join`: an online match without window or GPU.
fn headless_net(data: &data::Data, req: NetRequest, args: &Args) -> Result<(), String> {
    let net = match req {
        NetRequest::Host { world, level, players, port, ip } => {
            let g = netplay::new_match_game(data, world, level, players)?;
            netplay::udp_host(g, world, level, players, port, ip)?
        }
        NetRequest::Join { code } => netplay::udp_join(&code)?,
    };
    netplay::run_headless(data, net, args.frames, args.pad_seed).map(|_| ())
}

/// CI smoke mode: runs N ticks without a window (muted) and renders the last frame offscreen.
fn headless(data: data::Data, cfg: &Config, args: &Args, steps: Vec<Step>) -> Result<(), String> {
    let (w, h) = args.size.unwrap_or((640, 480));
    let ctx = GpuContext::new_headless().map_err(|e| e.to_string())?;
    let mut app = App::new(data, &ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm, AudioEngine::null(44100), cfg.input_mapper());
    app.set_display(cfg.profile, cfg.display_settings(Profile::Enhanced));
    app.set_input(cfg.bindings(), cfg.input.deadzone, cfg.hop_buffer_frames(Profile::Enhanced));
    app.hud_probe = args.hud_probe;
    app.set_gameplay(cfg.gameplay.clone(), None);
    app.editor_mods = Some(editor_target(args, cfg));
    app.editor_cmds = args.editor_cmds.clone();
    // Headless runs only read a save given explicitly and never write one.
    app.save_data = match (&args.save, args.dev_unlock) {
        (_, true) => Some(dev_unlock_save()),
        (Some(p), _) => Some(std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?),
        _ => None,
    };
    app.pad_script = match &args.pad_script {
        Some(p) => Some(padscript::PadScript::load(p)?),
        None => None,
    };
    let frames = app.pad_script.as_ref().map_or(args.frames as u64, |s| (args.frames as u64).max(s.last_tick() + 1));
    app.push_steps(steps);
    let kb = f2_platform::KeyboardState::default();
    // First update loads the level (no time passes), then exactly N ticks.
    app.update(Duration::ZERO, &kb, &[], true);
    if app.scene.is_none() && !app.has_session() {
        return Err("level failed to load".into());
    }
    // Debug: `F2_SHOT_UNTIL=tongue|ring|decal|lightning` stops at the first tick showing it.
    let until = std::env::var("F2_SHOT_UNTIL").ok();
    for _ in 0..frames {
        if app.editing() {
            // The editor pauses the simulation.
            break;
        }
        let shots: Vec<PathBuf> = app.pad_script.as_ref().map_or(Vec::new(), |s| s.shots_at(app.ticks).cloned().collect());
        for out in shots {
            app.draw(w, h);
            let img = app.r.render_to_image(app.clear_color()).map_err(|e| e.to_string())?;
            img.write_png(&out).map_err(|e| e.to_string())?;
            eprintln!("frogger: tick {}: mode {}, wrote {}", app.ticks, app.game.game_state.mode, out.display());
        }
        if let Some(u) = &until {
            use f2_game::specfx::FxDraw;
            let g = &app.game;
            let fx = |d: FxDraw| g.specfx.iter_list().any(|(_, e)| e.draw == Some(d));
            let hit = match u.as_str() {
                "tongue" => g.tongue.tongue.iter().any(|t| t.flags & f2_game::tongue::TONGUE_BEINGUSED != 0 && t.progress > 2048),
                "ring" => fx(FxDraw::Ring),
                "decal" => fx(FxDraw::Decal),
                "lightning" => fx(FxDraw::Lightning),
                _ => false,
            };
            if hit {
                break;
            }
        }
        app.tick(&[]);
        // Videos are skipped headless (the simulation would wait for them).
        while app.video.is_some() {
            app.end_video();
        }
    }
    if let Some(tab) = args.screenshot_settings {
        app.open_settings(tab);
    }
    if args.screenshot_rebind {
        // Demo of the F10 screen: rebind Down to Up's key (conflict message), then start
        // capturing for the Cross row.
        use f2_platform::dik;
        app.toggle_controls();
        for k in [dik::DOWN, dik::RETURN, dik::UP, dik::DOWN, dik::DOWN, dik::DOWN, dik::RETURN] {
            app.controls_key(k);
        }
    }
    app.draw(w, h);
    let tris = app.r.queued_tris();
    eprintln!("frogger: {} ticks, act_frame_count {}, {tris} triangles, profile {}", app.ticks, app.game.act_frame_count, app.profile.name());
    let g = &app.game;
    let mut fx = std::collections::BTreeMap::new();
    for (_, e) in g.specfx.iter_list() {
        *fx.entry(format!("{:?}/{}", e.draw, e.ty)).or_insert(0) += 1;
    }
    eprintln!(
        "frogger: mode {}, {} sprites, {} overlays, {} text overlays, {} proc textures, fade {:?}, effects (draw/type) {fx:?}",
        g.game_state.mode,
        g.sprite.count,
        g.overlays.block.iter().filter(|o| o.used != 0 && o.draw != 0).count(),
        g.textover.block.iter().filter(|o| o.used != 0 && o.draw != 0).count(),
        g.ptexture.list.iter().filter(|p| p.surface.is_some()).count(),
        g.screen_fade_colour()
    );
    if let Some(out) = &args.screenshot {
        let img = app.r.render_to_image(app.clear_color()).map_err(|e| e.to_string())?;
        img.write_png(out).map_err(|e| e.to_string())?;
        eprintln!("frogger: wrote {} ({}x{})", out.display(), img.width, img.height);
    }
    Ok(())
}

// ------------------------------------------------------------------ window

struct Gfx {
    ctx: GpuContext,
    surface: WindowSurface,
}

struct Host {
    data: Option<data::Data>,
    cfg: Config,
    mute: bool,
    hud_probe: bool,
    pad_script: Option<padscript::PadScript>,
    save_path: PathBuf,
    dev_unlock: bool,
    steps: Vec<Step>,
    net: Option<NetRequest>,
    /// Editor save target and start-up commands.
    editor_setup: ((PathBuf, String), Vec<String>),
    app: Option<App>,
    gfx: Option<Gfx>,
    gamepads: Option<Gamepads>,
    fullscreen: bool,
    last: Instant,
    error: Option<String>,
}

impl WindowApp for Host {
    fn init(&mut self, window: &Arc<window::Window>) {
        if self.gfx.is_some() {
            return;
        }
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())));
        let vsync = self.cfg.video.vsync;
        let res = (|| -> Result<Gfx, String> {
            let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
            let ctx = GpuContext::with_instance(instance, Some(&surface)).map_err(|e| e.to_string())?;
            let mut surface = WindowSurface::new(&ctx, surface, size.width, size.height).map_err(|e| e.to_string())?;
            // Present at display rate (vsync); the simulation is decoupled from it.
            surface.config.present_mode = if vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
            surface.surface.configure(&ctx.device, &surface.config);
            Ok(Gfx { ctx, surface })
        })();
        match res {
            Ok(g) => {
                let audio = make_audio(&self.cfg, self.mute);
                let mut mapper = self.cfg.input_mapper();
                self.gamepads = Gamepads::new().map_err(|e| eprintln!("frogger: gamepads unavailable: {e}")).ok();
                if let Some(p) = &self.gamepads {
                    if p.count() > 0 && self.cfg.input.controllers.iter().all(|c| *c == f2_platform::Controller::Keyboard) {
                        mapper.reset_controllers(p.count());
                    }
                }
                eprintln!(
                    "frogger: {} {:?} ({:?}), audio {}",
                    g.ctx.adapter.get_info().name,
                    g.ctx.adapter.get_info().backend,
                    g.surface.config.format,
                    if audio.is_realtime() { "on" } else { "muted" }
                );
                let mut app = App::new(self.data.take().expect("data"), &g.ctx.device, &g.ctx.queue, g.surface.config.format, audio, mapper);
                app.set_display(self.cfg.profile, self.cfg.display_settings(Profile::Enhanced));
                app.set_input(self.cfg.bindings(), self.cfg.input.deadzone, self.cfg.hop_buffer_frames(Profile::Enhanced));
                if let Some(p) = &mut self.gamepads {
                    p.rumble_scale = if self.cfg.input.rumble { self.cfg.input.rumble_scale } else { 0.0 };
                    p.set_controllers(app.mapper.controllers);
                    for i in 0..4 {
                        app.families[i] = p.family_for_player(i);
                    }
                }
                app.hud_probe = self.hud_probe;
                app.editor_mods = Some(self.editor_setup.0.clone());
                app.editor_cmds = std::mem::take(&mut self.editor_setup.1);
                app.pad_script = self.pad_script.take();
                app.save_path = Some(self.save_path.clone());
                if self.dev_unlock {
                    app.save_data = Some(dev_unlock_save());
                    app.save_path = None;
                }
                // P6-1: gameplay options, save slots (next to slot 1's file), splits.
                let slot1 = (!self.dev_unlock).then(|| self.save_path.clone());
                app.set_gameplay(self.cfg.gameplay.clone(), slot1);
                app.speedrun = gameplay::Speedrun::new(Some(f2_platform::paths::save_dir().join("splits.csv")));
                app.push_steps(std::mem::take(&mut self.steps));
                if let Some(req) = self.net.take() {
                    if let Err(e) = app.start_net(req) {
                        self.error = Some(format!("online: {e}"));
                    }
                }
                self.app = Some(app);
                self.gfx = Some(g);
            }
            Err(e) => self.error = Some(e),
        }
        self.last = Instant::now();
    }

    fn event(&mut self, _w: &Arc<window::Window>, event: &winit::event::WindowEvent) -> Flow {
        use winit::event::{ElementState, MouseButton, WindowEvent};
        if let Some(ed) = self.app.as_mut().and_then(|a| a.editor.as_mut()) {
            match event {
                WindowEvent::CursorMoved { position, .. } => ed.mouse(Some((position.x as f32, position.y as f32)), false),
                WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => ed.mouse(None, true),
                _ => {}
            }
        }
        Flow::Continue
    }

    fn resized(&mut self, _w: &Arc<window::Window>, size: winit::dpi::PhysicalSize<u32>) {
        if let Some(g) = &mut self.gfx {
            g.surface.resize(&g.ctx.device, size.width, size.height);
        }
    }

    fn frame(&mut self, frame: window::Frame<'_>) -> Flow {
        if self.error.is_some() {
            return Flow::Exit;
        }
        let (Some(app), Some(g)) = (&mut self.app, &mut self.gfx) else { return Flow::Continue };
        let now = Instant::now();
        let dt = now - self.last;
        self.last = now;

        let mut raw: [Option<f2_platform::RawPad>; 4] = Default::default();
        if let Some(p) = &mut self.gamepads {
            raw = p.poll_raw();
            let mut replug = false;
            for ev in p.take_events() {
                match ev {
                    PadEvent::Connected { name, .. } => {
                        eprintln!("frogger: gamepad connected: {name}");
                        replug = true;
                    }
                    PadEvent::Disconnected { slot } => {
                        eprintln!("frogger: gamepad {slot} disconnected");
                        replug = true;
                    }
                    PadEvent::Raw { event, .. } => app.pad_event(event),
                }
            }
            if replug {
                let c = auto_assign(&app.mapper.controllers, p.count());
                app.mapper.controllers = c;
                p.set_controllers(c);
                for i in 0..4 {
                    app.families[i] = p.family_for_player(i);
                }
            }
        }
        app.update(dt, frame.keyboard, &raw, frame.focused);
        if let Some(p) = &mut self.gamepads {
            for (player, strength, frames) in app.rumble.drain(..) {
                p.rumble(player, strength, frames);
            }
        } else {
            app.rumble.clear();
        }
        if std::mem::take(&mut app.requests.save_bindings) {
            self.cfg.set_bindings(&app.bindings);
            if let Err(e) = self.cfg.save_default() {
                eprintln!("frogger: saving config: {e}");
            }
        }
        if std::mem::take(&mut app.requests.save_gameplay) {
            self.cfg.gameplay = app.gameplay.clone();
            self.cfg.profile = app.profile;
            if let Err(e) = self.cfg.save_default() {
                eprintln!("frogger: saving config: {e}");
            }
        }
        if let Some((music, sfx)) = app.requests.save_volumes.take() {
            self.cfg.audio.music = music.clamp(0, 100) as u8;
            self.cfg.audio.sfx = sfx.clamp(0, 100) as u8;
            if let Err(e) = self.cfg.save_default() {
                eprintln!("frogger: saving config: {e}");
            }
        }
        if app.requests.quit {
            return Flow::Exit;
        }
        if std::mem::take(&mut app.requests.toggle_fullscreen) {
            self.fullscreen = !self.fullscreen;
            window::set_fullscreen(frame.window, self.fullscreen);
        }
        if app.idle() {
            return Flow::Exit;
        }

        let (w, h) = (g.surface.config.width, g.surface.config.height);
        app.draw(w, h);
        let tex = match g.surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                let s = frame.window.inner_size();
                g.surface.resize(&g.ctx.device, s.width, s.height);
                return Flow::Continue;
            }
            _ => {
                std::thread::sleep(Duration::from_millis(5));
                return Flow::Continue;
            }
        };
        let view = tex.texture.create_view(&Default::default());
        let clear = app.clear_color();
        app.r.render(&view, w, h, clear);
        frame.window.pre_present_notify();
        g.ctx.queue.present(tex);
        // Frame cap (`video.frame_cap`): the simulation is fixed 60 Hz and rendering is
        // interpolated, so any cap/refresh rate is smooth.
        let cap = self.cfg.video.frame_cap;
        if cap > 0 {
            let period = Duration::from_secs_f64(1.0 / cap as f64);
            let spent = now.elapsed();
            if spent < period {
                std::thread::sleep(period - spent);
            }
        }
        Flow::Continue
    }
}

fn run_window(data: data::Data, cfg: Config, args: &Args, steps: Vec<Step>, net: Option<NetRequest>) -> Result<(), String> {
    let mut wc = WindowConfig::from_config(&cfg);
    if let Some((w, h)) = args.size {
        wc.width = w;
        wc.height = h;
    }
    wc.fullscreen |= args.fullscreen;
    wc.title = "Frogger 2: Swampy's Revenge".into();
    let editor_setup = (editor_target(args, &cfg), args.editor_cmds.clone());
    let host = Host {
        data: Some(data),
        mute: args.mute,
        hud_probe: args.hud_probe,
        pad_script: match &args.pad_script {
            Some(p) => Some(padscript::PadScript::load(p)?),
            None => None,
        },
        save_path: args.save.clone().unwrap_or_else(|| f2_platform::paths::save_dir().join("savegame.dat")),
        dev_unlock: args.dev_unlock,
        fullscreen: wc.fullscreen,
        cfg,
        steps,
        net,
        editor_setup,
        app: None,
        gfx: None,
        gamepads: None,
        last: Instant::now(),
        error: None,
    };
    let host = window::run(wc, host)?;
    match host.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use f2_platform::DisplaySettings;

    fn rendition() -> Option<PathBuf> {
        let base = std::env::var("F2_GAME_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../game-data/usa").into());
        let p = PathBuf::from(base);
        // Accept either the rendition root or its `game/` directory.
        let root = if p.file_name().is_some_and(|n| n == "game") { p.parent()?.to_path_buf() } else { p };
        root.join("game").is_dir().then_some(root)
    }

    /// Garden 1 after 120 ticks, rendered headless at `w`×`h`. `options = None` applies
    /// `profile` through the host; `Some` sets the renderer options directly. `hud = false`
    /// hides the sprite/text overlays (world only).
    fn garden1(
        ctx: &GpuContext,
        root: &std::path::Path,
        w: u32,
        h: u32,
        profile: Profile,
        options: Option<f2_engine::RenderOptions>,
        hud: bool,
    ) -> f2_engine::RgbaImage {
        let data = data::Data::locate(&Config::default(), Some(root.to_path_buf())).expect("data");
        let mut app = App::new(data, &ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm, AudioEngine::null(44100), Config::default().input_mapper());
        app.set_display(profile, DisplaySettings::ENHANCED);
        app.push_steps([Step::Level(0, 0)]);
        app.update(Duration::ZERO, &f2_platform::KeyboardState::default(), &[], true);
        assert!(app.scene.is_some(), "Garden 1 failed to load");
        for _ in 0..120 {
            app.tick(&[]);
        }
        if let Some(o) = options {
            app.r.options = o;
        }
        if !hud {
            app.game.overlays.draw_overlays = false;
            app.game.textover.num_entries = 0;
        }
        app.draw(w, h);
        app.r.render_to_image(app.clear_color()).expect("render")
    }

    /// The authentic profile must stay pixel-identical to the pre-enhancement renderer:
    /// it must equal a render with the retail-default `RenderOptions` (the original
    /// single-pass path). With `F2_AUTHENTIC_BASELINE=<png>` (a 640×480 screenshot from a
    /// pre-change build: `frogger --headless --frames 120 --level 0 0 --screenshot ...`)
    /// the PNG bytes are compared too.
    #[test]
    fn authentic_profile_is_pixel_identical() {
        let Some(root) = rendition() else {
            eprintln!("skipping: no game data (F2_GAME_DIR)");
            return;
        };
        let ctx = GpuContext::new_headless().expect("headless GPU");
        let auth = garden1(&ctx, &root, 640, 480, Profile::Authentic, None, true);
        let legacy = garden1(&ctx, &root, 640, 480, Profile::Authentic, Some(f2_engine::RenderOptions::default()), true);
        assert!(auth.pixels == legacy.pixels, "authentic profile differs from the retail-default renderer");
        let enh = garden1(&ctx, &root, 640, 480, Profile::Enhanced, None, true);
        assert!(enh.pixels != auth.pixels, "enhanced profile should differ (MSAA, filtering)");
        if let Some(base) = std::env::var_os("F2_AUTHENTIC_BASELINE") {
            let out = std::env::temp_dir().join(format!("f2-authentic-{}.png", std::process::id()));
            auth.write_png(&out).unwrap();
            let (a, b) = (std::fs::read(&out).unwrap(), std::fs::read(&base).unwrap());
            let _ = std::fs::remove_file(&out);
            assert!(a == b, "authentic render differs from baseline {}", PathBuf::from(base).display());
        }
    }

    fn hello_mod_data(root: &std::path::Path) -> data::Data {
        let mut d = data::Data::locate(&Config::default(), Some(root.to_path_buf())).expect("data");
        let mods = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/mods"));
        d.enable_mods(&mods, &["hello-mod".to_string()]).expect("hello-mod");
        d
    }

    /// The example mod (`examples/mods/hello-mod`): its level 0-1 loads through the level
    /// registry with the ETC script compiled at load, its text override applies, and
    /// Garden 1 (untouched by the mod) renders exactly as without mods, so the effective
    /// banks from `f2-modkit` equal the retail ones.
    #[test]
    fn hello_mod_pipeline() {
        let Some(root) = rendition() else {
            eprintln!("skipping: no game data (F2_GAME_DIR)");
            return;
        };
        let ctx = GpuContext::new_headless().expect("headless GPU");
        let data = hello_mod_data(&root);
        assert_eq!(data.levels.levels.len(), 1);
        let mut app = App::new(data.clone(), &ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm, AudioEngine::null(44100), Config::default().input_mapper());
        app.push_steps([Step::Level(0, 1)]);
        app.update(Duration::ZERO, &f2_platform::KeyboardState::default(), &[], true);
        assert!(app.scene.is_some(), "hello-garden failed to load");
        for _ in 0..30 {
            app.tick(&[]);
        }
        let g = &app.game;
        let lvd = g.layout.world_visual_data[0].level_visual_data[1];
        assert_eq!(lvd.fog, [96, 64, 160u8 as i8]);
        assert_eq!(g.gamestring(lvd.description_str), "Hello Garden");
        assert_eq!(g.language.gamestring_by_id("STR_START_GAME"), "Hello, Start!");
        // `Fog(96, 64, 160, 5, 60)` at the top of script.etc ran.
        assert_eq!((g.frogger.fog.r, g.frogger.fog.g, g.frogger.fog.b, g.frogger.fog.max), (96, 64, 160, 6000));
        assert!(g.level_files.script.is_some());
        assert!(g.map.tiles.len() > 100);

        let plain = garden1(&ctx, &root, 320, 240, Profile::Authentic, None, true);
        let modded = {
            let mut app = App::new(hello_mod_data(&root), &ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm, AudioEngine::null(44100), Config::default().input_mapper());
            app.set_display(Profile::Authentic, DisplaySettings::ENHANCED);
            app.push_steps([Step::Level(0, 0)]);
            app.update(Duration::ZERO, &f2_platform::KeyboardState::default(), &[], true);
            for _ in 0..120 {
                app.tick(&[]);
            }
            app.draw(320, 240);
            app.r.render_to_image(app.clear_color()).expect("render")
        };
        assert!(plain.pixels == modded.pixels, "Garden 1 differs with hello-mod enabled");
    }

    /// Enhanced widescreen keeps the image unstretched: with Hor+ the centred 1440×1080
    /// of a 2560×1080 frame is the 1440×1080 (4:3) frame — same vertical FOV, same scale.
    #[test]
    fn enhanced_widescreen_is_hor_plus() {
        let Some(root) = rendition() else {
            eprintln!("skipping: no game data (F2_GAME_DIR)");
            return;
        };
        let ctx = GpuContext::new_headless().expect("headless GPU");
        // World only: the HUD is re-anchored to the screen edges in the wide frame, so it
        // is outside the compared centre crop there but inside it in the 4:3 frame.
        let wide = garden1(&ctx, &root, 2560, 1080, Profile::Enhanced, None, false);
        let narrow = garden1(&ctx, &root, 1440, 1080, Profile::Enhanced, None, false);
        assert_eq!((wide.width, wide.height), (2560, 1080));
        let (mut same, mut total) = (0usize, 0usize);
        for y in (0..1080).step_by(3) {
            for x in (0..1440).step_by(3) {
                total += 1;
                // Small tolerance: edge antialiasing and faces rejected at the 4:3 border.
                let (a, b) = (wide.pixel(x + 560, y), narrow.pixel(x, y));
                if a.iter().zip(b).all(|(p, q)| p.abs_diff(q) <= 8) {
                    same += 1;
                }
            }
        }
        assert!(same * 100 >= total * 97, "centre crop matches 4:3 render: {same}/{total}");
        let auth = garden1(&ctx, &root, 2560, 1080, Profile::Authentic, None, false);
        assert!(auth.pixels != wide.pixels);
    }
}
