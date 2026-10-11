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
//! Keys: F1 free camera, F2 tile overlay, F10 settings screen (Display / Audio / Controls /
//! Gameplay; also "Settings" in the pause menus and Extras), Shift+F10 the advanced screen
//! (raw controls / gameplay / level select; Tab switches page), F11 / Alt+Enter toggle
//! windowed and the fullscreen type, Esc quit (Enter/Esc skip videos; any key in the
//! enhanced profile);
//! display A/B (`docs/enhancements.md`): F5 profile, F3 perspective-correct, F4 render
//! scale, F6 MSAA, F7 texture filter, F8 widescreen, F9 fog, F12 draw distance.

mod firstrun;
mod present;

#[cfg(test)]
use frogger::render;
use frogger::{app, data, gameplay, netplay, padscript};

use app::{App, NetRequest, Step};
use f2_audio::AudioEngine;
use f2_engine::{GpuContext, WindowSurface};
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
    /// `--screenshot-page PAGE[:ROW]` (headless): the settings screen in the screenshot.
    screenshot_page: Option<(app::SettingsPage, usize)>,
    /// `--screenshot-keymap normal|capture|conflict` (headless).
    screenshot_keymap: Option<String>,
    /// `--show-fps` (headless screenshots).
    show_fps: bool,
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
         \x20      [--screenshot-settings controls|gameplay|levels] (headless: Shift+F10 page in the screenshot)\n\
         \x20      [--screenshot-page display|audio|controls|gameplay[:ROW]] [--screenshot-keymap normal|capture|conflict] [--show-fps]\n\
         editor: --editor [--level W L] [--editor-cmd CMDS]... (saves to the first --mods/--mod id, else level-W-L, in the mods dir)\n\
         saves: [--save FILE] (default: config dir; headless: none) [--dev-unlock] (start with everything unlocked, not saved)\n\
         keys: F1 free camera, F2 tile overlay, F10 settings, Shift+F10 advanced (level select), F11 / Alt+Enter fullscreen, Esc quit\n\
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
        screenshot_page: None,
        screenshot_keymap: None,
        show_fps: false,
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
            "--screenshot-page" => {
                let v = it.next().unwrap_or_else(|| usage());
                let (p, row) = v.split_once(':').map_or((v.as_str(), 0), |(p, r)| (p, r.parse().unwrap_or_else(|_| usage())));
                a.screenshot_page = Some((app::SettingsPage::parse(p).unwrap_or_else(|| usage()), row));
            }
            "--screenshot-keymap" => a.screenshot_keymap = Some(it.next().unwrap_or_else(|| usage())),
            "--show-fps" => a.show_fps = true,
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
            // First run: offer the import screen (windowed runs); a successful import restarts the game.
            if !args.headless && args.data.is_none() && firstrun::run(&cfg, &e) {
                firstrun::relaunch();
            }
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
    let mut hcfg = cfg.clone();
    if args.show_fps {
        // F2_FPS_CORNER=tl|bl|br|tr picks the corner for headless screenshots.
        use f2_platform::FpsCorner as C;
        hcfg.video.show_fps = match std::env::var("F2_FPS_CORNER").as_deref() {
            Ok("tl") => C::TopLeft,
            Ok("bl") => C::BottomLeft,
            Ok("br") => C::BottomRight,
            _ => C::TopRight,
        };
    }
    app.set_config(hcfg);
    app.win = app::WinInfo { size: (w, h), ..Default::default() };
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
    if let Some((page, row)) = args.screenshot_page {
        app.open_settings_page(page);
        use app::settings_nav as nav;
        for _ in 0..row {
            app.settings_nav(nav::DOWN);
        }
    }
    if let Some(k) = &args.screenshot_keymap {
        // Controls -> Keyboard mapping; `capture`: waiting for the key of Down;
        // `conflict`: Down rebound to Up's key (the conflict prompt).
        use app::settings_nav as nav;
        app.open_settings_page(app::SettingsPage::Controls);
        app.settings_nav(nav::OK);
        app.settings_nav(nav::DOWN);
        if k != "normal" {
            app.settings_nav(nav::OK);
        }
        if k == "conflict" {
            let up = app.bindings.get(0, f2_game::levplay::PAD_UP).iter().find_map(|b| if let f2_platform::Binding::Key(k) = b { Some(*k) } else { None });
            app.settings_key_raw(up.unwrap_or(f2_platform::dik::UP));
        }
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
    if std::env::var_os("F2_PRINT_CAMERA").is_some() {
        // Debug aid (N64 renderer comparisons, tools/n64r-export): the drawn camera.
        if let Some(c) = app.r.view.camera {
            let v = |a: [f32; 3]| format!("{},{},{}", a[0], a[1], a[2]);
            let k = app.clear_color();
            eprintln!("frogger: camera {} {} {} far {} clear {},{},{}", v(c.source), v(c.target), v(c.up), app.r.view.far_clip, k[0], k[1], k[2]);
        }
    }
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
    /// Present pass (brightness / borderless render resolution), created on demand.
    present: Option<present::Present>,
    /// The window transition in flight (the real state follows asynchronously; until it
    /// settles the observed state does not overwrite the config and resizes are not saved).
    mode_req: Option<ModeReq>,
    /// A mode / resolution change requested while a transition was in flight: applied once
    /// it settles (latest settings win). Overlapping OS transitions confuse the platform's
    /// saved windowed placement (a monitor-sized "windowed" window).
    mode_next: bool,
    /// Last logged window state.
    logged: Option<(f2_platform::WindowMode, u32, u32)>,
    /// Window size / position changed: save the config on exit.
    window_dirty: bool,
    /// `F2_WINDOW_TEST=1`: steps through the window modes / resolutions every 3 s, logging
    /// what was requested and what the window became, then exits (platform check).
    window_test: Option<(usize, Instant)>,
    last: Instant,
    error: Option<String>,
}

/// A requested window mode, when it was requested and when the window last changed size.
struct ModeReq {
    mode: f2_platform::WindowMode,
    at: Instant,
    last_resize: Instant,
}

impl ModeReq {
    fn new(mode: f2_platform::WindowMode) -> ModeReq {
        let now = Instant::now();
        ModeReq { mode, at: now, last_resize: now }
    }

    /// The transition is over: the window reached the mode and stopped resizing (or the
    /// platform never got there).
    fn settled(&self, actual: f2_platform::WindowMode) -> bool {
        const QUIET: Duration = Duration::from_millis(250);
        (actual == self.mode && self.last_resize.elapsed() >= QUIET) || self.at.elapsed() >= Duration::from_secs(2)
    }
}

impl Host {
    /// Applies `cfg.video`'s window mode / resolution to the window, or queues it while a
    /// previous transition is still in flight.
    fn apply_window(&mut self, window: &window::Window) {
        let Some(app) = &mut self.app else { return };
        if self.mode_req.is_some() {
            self.mode_next = true;
            return;
        }
        let v = &app.cfg.video;
        let msg = window::apply_mode(window, v.window_mode, v.resolution, v.refresh_mhz, [v.width, v.height]);
        eprintln!("frogger: window: {msg}");
        self.mode_req = Some(ModeReq::new(v.window_mode));
    }

    /// Follows the real window state: mode (the OS can leave fullscreen), size, monitor
    /// modes; logs changes.
    fn observe_window(&mut self, window: &window::Window) {
        let (Some(app), Some(g)) = (&mut self.app, &self.gfx) else { return };
        let actual = window::actual_mode(window);
        let size = (g.surface.config.width, g.surface.config.height);
        app.win.mode = actual;
        app.win.size = size;
        if app.win.modes.is_empty() || app.win.desktop != window::monitor_mode(window) {
            app.win.modes = window::video_modes(window);
            app.win.desktop = window::monitor_mode(window);
        }
        let mut settled = None;
        let pending = match &self.mode_req {
            Some(r) if !r.settled(actual) => true,
            Some(_) => {
                settled = self.mode_req.take();
                false
            }
            None => false,
        };
        if !pending && app.cfg.video.observe_mode(actual) {
            eprintln!("frogger: window mode changed outside the game: {}", actual.name());
            self.window_dirty = true;
        }
        if let Some(r) = settled {
            if r.mode == f2_platform::WindowMode::Windowed && actual == f2_platform::WindowMode::Windowed {
                let v = &app.cfg.video;
                window::fix_windowed(window, v.resolution.unwrap_or([v.width, v.height]));
            }
            if std::mem::take(&mut self.mode_next) {
                self.apply_window(window);
                return;
            }
        }
        if self.logged != Some((actual, size.0, size.1)) {
            self.logged = Some((actual, size.0, size.1));
            let inner = window.inner_size();
            eprintln!("frogger: window now {} (inner {}x{}, surface {}x{})", actual.name(), inner.width, inner.height, size.0, size.1);
        }
    }

    /// Saves the configuration (the app's copy).
    fn save_config(&mut self) {
        let Some(app) = &mut self.app else { return };
        app.sync_cfg();
        self.cfg = app.cfg.clone();
        if let Err(e) = self.cfg.save_default() {
            eprintln!("frogger: saving config: {e}");
        }
    }
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
                if let Some(p) = &mut self.gamepads {
                    // controller: PS1-style pad options; remembered pads go back to their players,
                    // other pads to the lowest keyboard players (connection order).
                    p.stick_mode = self.cfg.input.stick_mode;
                    p.deadzone = self.cfg.input.deadzone;
                    p.face_layout = self.cfg.input.face_layout;
                    p.set_preferred(&self.cfg.input.pad_devices);
                    // Slot order can change between runs: with remembered devices, start from
                    // scratch so each player finds their own pad.
                    let start =
                        if self.cfg.input.pad_devices.iter().any(|d| !d.is_empty()) { [f2_platform::Controller::Keyboard; 4] } else { mapper.controllers };
                    mapper.controllers = p.reassign(&start);
                }
                eprintln!(
                    "frogger: {} {:?} ({:?}), audio {}",
                    g.ctx.adapter.get_info().name,
                    g.ctx.adapter.get_info().backend,
                    g.surface.config.format,
                    if audio.is_realtime() { "on" } else { "muted" }
                );
                let mut app = App::new(self.data.take().expect("data"), &g.ctx.device, &g.ctx.queue, g.surface.config.format, audio, mapper);
                app.set_config(self.cfg.clone());
                app.apply_master_volume();
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

    fn event(&mut self, w: &Arc<window::Window>, event: &winit::event::WindowEvent) -> Flow {
        use winit::event::{ElementState, MouseButton, WindowEvent};
        if let (WindowEvent::Moved(_), Some(app)) = (event, &mut self.app) {
            if let Some(p) = window::windowed_position(w) {
                if app.cfg.video.window_pos != Some(p) {
                    app.cfg.video.window_pos = Some(p);
                    self.window_dirty = true;
                }
            }
        }
        if let Some(ed) = self.app.as_mut().and_then(|a| a.editor.as_mut()) {
            match event {
                WindowEvent::CursorMoved { position, .. } => ed.mouse(Some((position.x as f32, position.y as f32)), false),
                WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => ed.mouse(None, true),
                _ => {}
            }
        }
        Flow::Continue
    }

    fn resized(&mut self, w: &Arc<window::Window>, size: winit::dpi::PhysicalSize<u32>) {
        if let Some(g) = &mut self.gfx {
            g.surface.resize(&g.ctx.device, size.width, size.height);
        }
        if let Some(r) = &mut self.mode_req {
            r.last_resize = Instant::now();
        }
        // Windowed: remember the size; a user resize away from a fixed resolution makes it
        // "Auto" (the setting always shows the real state).
        if let Some(app) = &mut self.app {
            let v = &mut app.cfg.video;
            if w.fullscreen().is_none() && v.window_mode == f2_platform::WindowMode::Windowed && self.mode_req.is_none() && size.width > 0 && size.height > 0 {
                // Config window sizes are logical points (physical / scale_factor), so the
                // window does not grow on every launch on HiDPI displays.
                let scale = w.scale_factor();
                let (lw, lh) = (window::physical_to_logical(size.width, scale), window::physical_to_logical(size.height, scale));
                let same = |a: u32, b: u32| window::logical_to_physical(a, scale) == b;
                if v.resolution.is_some_and(|[rw, rh]| !(same(rw, size.width) && same(rh, size.height))) {
                    v.resolution = None;
                }
                if !(same(v.width, size.width) && same(v.height, size.height)) {
                    (v.width, v.height) = (lw, lh);
                    self.window_dirty = true;
                }
            }
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
                // controller: assignment by device id (players keep their pads when slots shift).
                let c = p.reassign(&app.mapper.controllers);
                app.mapper.controllers = c;
                for i in 0..4 {
                    app.families[i] = p.family_for_player(i);
                }
                if app.cfg.input.controllers != c || app.cfg.input.pad_devices != p.preferred() {
                    // The app's copy is the one the settings screen edits and saves.
                    app.cfg.input.controllers = c;
                    app.cfg.input.pad_devices = p.preferred();
                    app.requests.save_config = true;
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
        let rq = &mut app.requests;
        let save = std::mem::take(&mut rq.save_bindings)
            | std::mem::take(&mut rq.save_gameplay)
            | rq.save_volumes.take().is_some()
            | std::mem::take(&mut rq.save_config);
        if std::mem::take(&mut rq.apply_vsync) {
            g.surface.config.present_mode = if app.cfg.video.vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
            g.surface.surface.configure(&g.ctx.device, &g.surface.config);
        }
        if std::mem::take(&mut rq.apply_input) {
            if let Some(p) = &mut self.gamepads {
                let i = &app.cfg.input;
                p.rumble_scale = if i.rumble { i.rumble_scale } else { 0.0 };
                p.stick_mode = i.stick_mode;
                p.deadzone = i.deadzone;
                p.face_layout = i.face_layout;
                for n in 0..4 {
                    app.families[n] = p.family_for_player(n);
                }
            }
        }
        if app.requests.quit {
            return Flow::Exit;
        }
        if let Some((n, t)) = self.window_test {
            // Steps 10-12 follow each other quickly (rapid switching in the settings screen).
            let wait = if (10..=12).contains(&n) { Duration::from_millis(80) } else { Duration::from_secs(3) };
            if t.elapsed() >= wait {
                use f2_platform::WindowMode as M;
                let v = &mut app.cfg.video;
                let what = match n {
                    0 => {
                        v.select_mode(M::Borderless);
                        "select Borderless"
                    }
                    1 => {
                        v.select_mode(M::Fullscreen);
                        "select Fullscreen (exclusive)"
                    }
                    2 => {
                        app.requests.toggle_fullscreen = true;
                        "Alt+Enter (expect Windowed)"
                    }
                    3 => {
                        app.requests.toggle_fullscreen = true;
                        "Alt+Enter (expect exclusive Fullscreen)"
                    }
                    4 => {
                        v.select_mode(M::Windowed);
                        v.resolution = Some([1024, 768]);
                        "Windowed 1024x768"
                    }
                    5 => {
                        v.resolution = Some([800, 600]);
                        "Windowed 800x600"
                    }
                    6 => {
                        v.select_mode(M::Fullscreen);
                        v.resolution = Some([1280, 720]);
                        "exclusive Fullscreen 1280x720"
                    }
                    7 => {
                        v.select_mode(M::Borderless);
                        v.resolution = Some([640, 480]);
                        "Borderless, render 640x480"
                    }
                    8 => {
                        v.select_mode(M::Windowed);
                        v.resolution = None;
                        "Windowed, auto"
                    }
                    9 | 11 => {
                        v.select_mode(M::Borderless);
                        "rapid: Borderless"
                    }
                    10 | 12 => {
                        v.select_mode(M::Windowed);
                        "rapid: Windowed (expect a normal window at the saved size)"
                    }
                    _ => return Flow::Exit,
                };
                eprintln!("frogger: window test step {n}: {what}");
                if n != 2 && n != 3 {
                    app.requests.apply_window = true;
                }
                self.window_test = Some((n + 1, Instant::now()));
            }
        }
        let toggle = std::mem::take(&mut app.requests.toggle_fullscreen);
        if toggle {
            app.cfg.video.toggle_fullscreen();
            eprintln!("frogger: Alt+Enter / F11: {}", app.cfg.video.window_mode.name());
        }
        let apply = std::mem::take(&mut app.requests.apply_window) | toggle;
        if app.idle() {
            return Flow::Exit;
        }
        if save || toggle {
            self.save_config();
        }
        if apply {
            self.apply_window(frame.window);
        }
        self.observe_window(frame.window);
        let (Some(app), Some(g)) = (&mut self.app, &mut self.gfx) else { return Flow::Continue };
        // A mode switch can end without a final `Resized` (or with a stale one): the surface
        // always follows the real client size.
        let inner = frame.window.inner_size();
        if inner.width > 0 && inner.height > 0 && (inner.width, inner.height) != (g.surface.config.width, g.surface.config.height) {
            g.surface.resize(&g.ctx.device, inner.width, inner.height);
        }

        let (w, h) = (g.surface.config.width, g.surface.config.height);
        let v = &app.cfg.video;
        let (rs, via_present) = present::plan((w, h), app.win.mode == f2_platform::WindowMode::Borderless, v.resolution, v.brightness);
        let brightness = v.brightness;
        app.draw(rs.0, rs.1);
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
        if via_present {
            let p = self.present.get_or_insert_with(|| present::Present::new(&g.ctx.device, g.surface.config.format));
            {
                let off = p.target(&g.ctx.device, rs.0, rs.1);
                app.r.render(off, rs.0, rs.1, clear);
            }
            p.blit(&g.ctx.device, &g.ctx.queue, &view, w, h, brightness);
        } else {
            app.r.render(&view, w, h, clear);
        }
        frame.window.pre_present_notify();
        g.ctx.queue.present(tex);
        // Frame cap (`video.frame_cap`): the simulation is fixed 60 Hz and rendering is
        // interpolated, so any cap/refresh rate is smooth.
        let cap = app.cfg.video.frame_cap;
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

fn run_window(data: data::Data, mut cfg: Config, args: &Args, steps: Vec<Step>, net: Option<NetRequest>) -> Result<(), String> {
    if let Some((w, h)) = args.size {
        (cfg.video.width, cfg.video.height) = (w, h);
        cfg.video.resolution = None;
    }
    if args.fullscreen && cfg.video.window_mode == f2_platform::WindowMode::Windowed {
        cfg.video.window_mode = cfg.video.fullscreen_type.mode();
    }
    let mut wc = WindowConfig::from_config(&cfg);
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
        present: None,
        mode_req: (wc.mode != f2_platform::WindowMode::Windowed).then(|| ModeReq::new(wc.mode)),
        mode_next: false,
        logged: None,
        window_dirty: false,
        window_test: std::env::var_os("F2_WINDOW_TEST").map(|_| (0, Instant::now())),
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
    let mut host = window::run(wc, host)?;
    if host.window_dirty {
        // Remember the window size / position.
        host.save_config();
    }
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
        let mut app = level_app(ctx, root, (0, 0), 120, profile);
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

    /// Level `world`/`level` loaded headless in `profile` and run for `ticks` ticks.
    fn level_app(ctx: &GpuContext, root: &std::path::Path, (world, level): (u32, u32), ticks: u32, profile: Profile) -> App {
        let data = data::Data::locate(&Config::default(), Some(root.to_path_buf())).expect("data");
        let mut app = App::new(data, &ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm, AudioEngine::null(44100), Config::default().input_mapper());
        app.set_display(profile, DisplaySettings::ENHANCED);
        app.push_steps([Step::Level(world, level)]);
        app.update(Duration::ZERO, &f2_platform::KeyboardState::default(), &[], true);
        assert!(app.scene.is_some(), "level {world}-{level} failed to load");
        for _ in 0..ticks {
            app.tick(&[]);
        }
        app
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

    /// P7b: hello-mod's behaviour scripts are collected (mod-wide + level-scoped), mark the
    /// mod gameplay-affecting, compile, and run deterministically on Hello Garden: the moving
    /// hazard turns tiles deadly every 2 seconds. Headless (no GPU).
    #[test]
    fn hello_mod_scripts() {
        let Some(root) = rendition() else {
            eprintln!("skipping: no game data (F2_GAME_DIR)");
            return;
        };
        let data = hello_mod_data(&root);
        let names: Vec<_> = data.scripts.iter().map(|s| (s.name.as_str(), s.scope)).collect();
        assert_eq!(names, [("hello-mod/scripts/messages.f2s", None), ("levels/hello-garden/hazard.f2s", Some((0, 1)))]);
        assert!(data.mods[0].gameplay_affecting);
        let run = || {
            let mut cfg = f2_game::session::SessionConfig::new(std::sync::Arc::new(data.clone()));
            cfg.extra_levels = data.levels.clone();
            cfg.scripts = data.scripts.clone();
            let mut g = f2_game::Game::new_session(cfg);
            assert!(g.modscript_errors().is_empty(), "{:?}", g.modscript_errors());
            g.start_arcade(0, 1);
            g.after_game_loop();
            let initial: Vec<u32> = g.map.tiles.iter().map(|t| t.state).collect();
            let mut deadly = std::collections::BTreeSet::new();
            for t in 0..900u32 {
                let pad = if t % 40 == 0 { f2_game::levplay::PAD_UP } else { 0 };
                g.tick(f2_game::TickInput { pads: [pad, 0, 0, 0] });
                g.take_effects();
                for (i, tile) in g.map.tiles.iter().enumerate() {
                    if tile.state == f2_game::define::tilestate::DEADLY && initial[i] != tile.state {
                        deadly.insert(i);
                    }
                }
            }
            assert!(g.modscript_errors().is_empty(), "{:?}", g.modscript_errors());
            (g.checksum(), deadly)
        };
        let (a, deadly) = run();
        let (b, _) = run();
        assert_eq!(a, b, "two runs identical");
        eprintln!("tiles that were deadly: {deadly:?}");
        assert!((3..=5).contains(&deadly.len()), "the hazard walked over 3-5 tiles: {deadly:?}");
        // On Garden 1 the scripts do nothing (messages.f2s checks the level).
        let mut fp = f2_game::Game::new();
        assert!(fp.install_mod_scripts(&data.scripts).is_empty());
        assert_ne!(fp.modscript_hash(), 0);
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
        let mut app =
            App::new(data.clone(), &ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm, AudioEngine::null(44100), Config::default().input_mapper());
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
            let mut app = App::new(
                hello_mod_data(&root),
                &ctx.device,
                &ctx.queue,
                wgpu::TextureFormat::Rgba8Unorm,
                AudioEngine::null(44100),
                Config::default().input_mapper(),
            );
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

    /// Hor+ wider than 4:3 draws what the simulation's 640×480 culling dropped. In the
    /// Subterranean ant nest (4-1) the sarge ants' bodies are object sprites; one outside
    /// the 4:3 frustum has them killed by `ActorListDraw`, which used to leave only its legs
    /// on a 32:9 screen. The render-side pose must draw them, outside the central 4:3 band,
    /// without touching the simulation.
    #[test]
    fn widescreen_draws_sim_culled_object_sprites() {
        let Some(root) = rendition() else {
            eprintln!("skipping: no game data (F2_GAME_DIR)");
            return;
        };
        let ctx = GpuContext::new_headless().expect("headless GPU");
        let (w, h) = (2000, 563);
        let mut app = level_app(&ctx, &root, (4, 1), 200, Profile::Enhanced);
        app.game.overlays.draw_overlays = false;
        app.game.textover.num_entries = 0;
        let sum = app.game.checksum();
        app.draw(w, h);
        assert!(app.r.view.widened());
        let posed = render::widened_object_sprites(&app.game);
        assert!(!posed.is_empty(), "no sim-culled object sprites in the 32:9 ant nest");
        // Some of them land on the wide screen outside the 4:3 band.
        let band = (w as f32 - h as f32 * 4.0 / 3.0) / 2.0;
        let outside = posed
            .values()
            .filter_map(|s| app.r.view.xfm_point([s.pos.vx as f32 * 0.1, s.pos.vy as f32 * 0.1, s.pos.vz as f32 * 0.1]))
            .filter(|p| (p[0] < band || p[0] > w as f32 - band) && p[0] >= 0.0 && p[0] < w as f32 && p[1] >= 0.0 && p[1] < h as f32)
            .count();
        assert!(outside > 0, "no culled sprite projects outside the 4:3 band");
        let with = app.r.render_to_image(app.clear_color()).expect("render");
        assert_eq!(app.game.checksum(), sum, "drawing changed the simulation");
        // Baseline: the same frame without the render-side pose (the old behaviour).
        let culled: Vec<_> = app.game.actor2.act_list.ids(&app.game.actor2.actors);
        let mut saved = Vec::new();
        for id in culled {
            if app.game.posed_object_sprites(id).iter().any(|(sid, _)| posed.contains_key(sid)) {
                saved.push((id, app.game.actor2.actors[id].obj_sprites.take()));
            }
        }
        app.draw(w, h);
        let without = app.r.render_to_image(app.clear_color()).expect("render");
        for (id, s) in saved {
            app.game.actor2.actors[id].obj_sprites = s;
        }
        let (mut out_band, mut in_band) = (0usize, 0usize);
        for y in 0..h {
            for x in 0..w {
                if with.pixel(x, y) != without.pixel(x, y) {
                    if (x as f32) < band || (x as f32) >= w as f32 - band {
                        out_band += 1;
                    } else {
                        in_band += 1;
                    }
                }
            }
        }
        assert!(out_band > 200, "culled sprites drawn outside the 4:3 band: {out_band} px (inside: {in_band})");
        // In 4:3 nothing changes: the view is not widened and the retail culling stands.
        app.draw(1440, 1080);
        assert!(!app.r.view.widened());
    }
}
