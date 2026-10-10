//! P7b mod behaviour scripts on real data (`src/modscript`): hooks fire in a real level,
//! two runs are identical, a snapshot mid-script resimulates exactly, and a script whose
//! hooks do nothing leaves every other sub-state bit-identical to an unscripted run.
//! Reads `F2_GAME_DIR` (default `game-data/usa/game`); skips when absent.

use std::path::PathBuf;

use f2_game::assets::DirData;
use f2_game::define::mode::INGAME_MODE;
use f2_game::define::{ARCADE_MODE, SINGLEPLAYER};
use f2_game::levplay::*;
use f2_game::modscript::{ScriptSource, Value};
use f2_game::{Game, TickInput};

fn game_dir() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let p = std::env::var("F2_GAME_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("game-data/usa/game"));
    let p = if p.is_absolute() { p } else { root.join(p) };
    let p = if p.join("game").join("collision").is_dir() { p.join("game") } else { p };
    if p.join("collision").is_dir() {
        Some(p)
    } else {
        eprintln!("skipping: no game data at {}", p.display());
        None
    }
}

fn pattern(t: u32) -> u16 {
    const SEQ: [u16; 8] = [PAD_UP, PAD_UP, PAD_RIGHT, PAD_UP, PAD_LEFT, PAD_UP, PAD_CROSS, PAD_DOWN];
    if t % 15 < 2 {
        SEQ[(t / 15) as usize % SEQ.len()]
    } else {
        0
    }
}

const SCRIPT: &str = r#"
    let starts = 0;
    let lands = 0;
    let deaths = 0;
    let hazard = -1;
    let claimed = -1;
    let moved = 0;
    fn on_level_start(w, l) {
        starts += 1;
        hazard = -1;
        claimed = -1;
        let e = 0;
        while e < enemy_count() {
            if enemy_active(e) && enemy_num_nodes(e) > 1 { claimed = e; claim_enemy(e); }
            e += 1;
        }
        set_fog(10, 20, 30);
    }
    fn on_tick() {
        if frame() % 120 == 0 {
            if hazard >= 0 { set_tile_state(hazard, TILE_NORMAL); }
            hazard = random(tile_count());
            if tile_state(hazard) == TILE_NORMAL { set_tile_state(hazard, TILE_DEADLY); } else { hazard = -1; }
            spawn_fx(FX_SMOKEBURST, max(hazard, 0));
        }
    }
    fn on_frog_land(p, t, s) { lands += 1; }
    fn on_frog_death(p, cause) { deaths += 1; show_text("ouch " + cause, 60); }
    fn on_enemy_update(e) {
        // half speed: the default movement every other frame
        if frame() % 2 == 0 { enemy_run_default(e); moved += 1; }
    }
"#;

fn level(data: &DirData, scripts: &[ScriptSource]) -> Game {
    let mut g = Game::new();
    assert!(g.install_mod_scripts(scripts).is_empty());
    g.game_state.multi = SINGLEPLAYER;
    g.game_state.single = ARCADE_MODE;
    g.game_state.mode = INGAME_MODE;
    g.load_level(0, 0, data).expect("garden 1");
    g.frogger.player[0].lives = 100;
    g.take_effects();
    g
}

fn global(g: &Game, name: &str) -> i32 {
    let u = &g.modscript.units[0];
    let i = u.program.globals.iter().position(|n| n == name).expect("global");
    match &u.globals[i] {
        Value::Int(v) => *v,
        v => panic!("{v:?}"),
    }
}

fn src() -> Vec<ScriptSource> {
    vec![ScriptSource { name: "test/hazard.f2s".into(), source: SCRIPT.into(), scope: Some((0, 0)) }]
}

#[test]
fn hooks_fire_and_runs_are_deterministic() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let run = |ticks: u32| {
        let mut g = level(&data, &src());
        let mut sums = Vec::new();
        for t in 0..ticks {
            g.tick(TickInput { pads: [pattern(t), 0, 0, 0] });
            g.take_effects();
            sums.push(g.checksum());
        }
        (g, sums)
    };
    let (a, sa) = run(1200);
    let (b, sb) = run(1200);
    assert_eq!(sa, sb);
    assert_eq!(a.modscript.units[0].globals, b.modscript.units[0].globals);
    assert!(a.modscript_errors().is_empty(), "{:?}", a.modscript_errors());
    eprintln!(
        "starts {} lands {} deaths {} claimed {} moved {}",
        global(&a, "starts"),
        global(&a, "lands"),
        global(&a, "deaths"),
        global(&a, "claimed"),
        global(&a, "moved")
    );
    assert!(global(&a, "starts") >= 1);
    assert!(global(&a, "lands") > 0);
    assert!(global(&a, "claimed") >= 0, "garden 1 has path enemies");
    assert!(global(&a, "moved") > 0);
    assert_eq!((a.frogger.fog.r, a.frogger.fog.g, a.frogger.fog.b), (10, 20, 30));

    // Snapshot mid-run, restore and resimulate.
    let (mut g, _) = run(600);
    let snap = g.snapshot();
    g.restore(&snap);
    for t in 600..1200u32 {
        g.tick_resimulate(TickInput { pads: [pattern(t), 0, 0, 0] });
        assert_eq!(g.checksum(), sa[t as usize], "tick {t}");
    }
}

#[test]
fn inert_script_changes_nothing_else() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let inert = vec![ScriptSource {
        name: "inert.f2s".into(),
        source: "let n = 0; fn on_tick() { n += 1; } fn on_frog_land(p, t, s) { n += t; } fn on_pickup(p, k) {}".into(),
        scope: None,
    }];
    let mut a = level(&data, &[]);
    let mut b = level(&data, &inert);
    for t in 0..900u32 {
        let pads = TickInput { pads: [pattern(t), 0, 0, 0] };
        a.tick(pads);
        b.tick(pads);
        assert_eq!(a.take_effects(), b.take_effects(), "tick {t}");
        let pa: Vec<_> = a.checksum_parts().into_iter().filter(|p| p.0 != "modscript").collect();
        let pb: Vec<_> = b.checksum_parts().into_iter().filter(|p| p.0 != "modscript").collect();
        assert_eq!(pa, pb, "tick {t}");
    }
}
