use super::*;
use crate::define::mode::INGAME_MODE;
use crate::map::GameTile;

fn src(name: &str, s: &str) -> ScriptSource {
    ScriptSource { name: name.into(), source: s.into(), scope: None }
}

/// A game without data: 16 tiles, in-game mode.
fn game_with(scripts: &[ScriptSource]) -> Game {
    let mut g = Game::new();
    g.game_state.mode = INGAME_MODE;
    g.map.tiles = vec![GameTile::default(); 16];
    g.textover.init_text_overlay_linked_list(40);
    let errs = g.install_mod_scripts(scripts);
    assert!(errs.is_empty(), "{errs:?}");
    g
}

fn step(g: &mut Game) {
    g.act_frame_count = g.act_frame_count.wrapping_add(1);
    g.modscript_tick();
}

fn global(g: &Game, unit: usize, name: &str) -> Value {
    let u = &g.modscript.units[unit];
    let i = u.program.globals.iter().position(|n| n == name).expect("global");
    u.globals[i].clone()
}

#[test]
fn language_basics() {
    let s = r#"
        const N = 10;
        const GREETING = "hi";
        let fib10 = fib(N);
        let sum = 0;
        let text = GREETING + " " + 42;
        let flags = (0x0f & ~3) | (1 << 8) ^ 1;
        let logic = (1 && 0) || (!0 && 2 > 1);
        let neg = -7 / 2 + -7 % 2;
        fn fib(n) { if n < 2 { return n; } return fib(n - 1) + fib(n - 2); }
        fn on_tick() {
            let i = 0;
            while true {
                i += 1;
                if i % 2 == 0 { continue; }
                if i > 9 { break; }
                sum += i;
            }
        }
    "#;
    let mut g = game_with(&[src("t", s)]);
    assert_eq!(global(&g, 0, "fib10"), Value::Int(55));
    assert_eq!(global(&g, 0, "text"), Value::Str("hi 42".into()));
    assert_eq!(global(&g, 0, "flags"), Value::Int((0x0c | 0x100) ^ 1));
    assert_eq!(global(&g, 0, "logic"), Value::Int(1));
    assert_eq!(global(&g, 0, "neg"), Value::Int(-3 + -1));
    step(&mut g);
    assert_eq!(global(&g, 0, "sum"), Value::Int(1 + 3 + 5 + 7 + 9));
    step(&mut g);
    assert_eq!(global(&g, 0, "sum"), Value::Int(50));
    assert!(g.modscript_errors().is_empty());
}

#[test]
fn compile_errors() {
    let bad = [
        ("fn on_tick() { x = 1; }", "unknown variable x"),
        ("fn on_tick() { nope(); }", "unknown function nope"),
        ("fn on_tick(a) { }", "hook on_tick takes 0 parameters"),
        ("fn f(a) {} fn on_tick() { f(); }", "f takes 1 arguments"),
        ("fn on_tick() { set_tile_state(1); }", "set_tile_state takes 2 arguments"),
        ("fn log(x) {}", "shadows a built-in"),
        ("let x = 1", "expected `;`"),
        ("fn on_tick() { let s = \"abc; }", "unterminated string"),
        ("fn on_tick() { break; }", "outside a loop"),
    ];
    for (s, want) in bad {
        let e = compile(s).expect_err(s);
        assert!(e.msg.contains(want), "{s}: {e}");
    }
    let mut g = Game::new();
    let errs = g.install_mod_scripts(&[src("bad", "fn on_tick( {"), src("ok", "fn on_tick() {}")]);
    assert_eq!(errs.len(), 1);
    assert!(errs[0].starts_with("bad: line 1"), "{errs:?}");
    assert_eq!(g.modscript.units.len(), 1);
}

#[test]
fn runtime_error_disables_only_that_script() {
    let mut g = game_with(&[src("div", "let n = 0; fn on_tick() { n += 1; if n == 3 { n = n / 0; } }"), src("ok", "let n = 0; fn on_tick() { n += 1; }")]);
    for _ in 0..5 {
        step(&mut g);
    }
    let errs = g.modscript_errors();
    assert_eq!(errs.len(), 1);
    assert!(errs[0].1.contains("division by zero"), "{errs:?}");
    assert_eq!(global(&g, 0, "n"), Value::Int(3));
    assert_eq!(global(&g, 1, "n"), Value::Int(5));
    assert!(g.take_effects().iter().any(|e| matches!(e, crate::Effect::Log { text } if text.contains("disabled"))));
}

#[test]
fn budget_exhaustion_disables_script() {
    let mut g = game_with(&[src("spin", "let ticks = 0; fn on_tick() { ticks += 1; if ticks == 2 { while true { } } }")]);
    step(&mut g);
    assert!(g.modscript_errors().is_empty());
    step(&mut g);
    let errs = g.modscript_errors();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].1.contains("instruction budget exceeded"), "{errs:?}");
    step(&mut g);
    assert_eq!(global(&g, 0, "ticks"), Value::Int(2), "a disabled script never runs again");

    // Runaway recursion and string growth are bounded too.
    let mut g = game_with(&[
        src("rec", "fn f(n) { return f(n + 1); } fn on_tick() { f(0); }"),
        src("str", "let s = \"x\"; fn on_tick() { while true { s = s + s; } }"),
    ]);
    step(&mut g);
    let errs = g.modscript_errors();
    assert!(errs[0].1.contains("call depth"), "{errs:?}");
    assert!(errs[1].1.contains("string longer"), "{errs:?}");
}

#[test]
fn budget_is_per_tick() {
    // ~6 instructions per iteration: 20 000 iterations fit in one tick's budget, and the
    // budget refreshes every tick.
    let mut g = game_with(&[src("busy", "fn on_tick() { let i = 0; while i < 20000 { i += 1; } }")]);
    for _ in 0..10 {
        step(&mut g);
    }
    assert!(g.modscript_errors().is_empty(), "{:?}", g.modscript_errors());
}

const HAZARD: &str = r#"
    let next = 0;
    let lit = -1;
    let landed = 0;
    let deaths = 0;
    fn on_tick() {
        if frame() >= next {
            next = frame() + 30 + random(30);
            if lit >= 0 { set_tile_state(lit, TILE_NORMAL); }
            lit = random(tile_count());
            set_tile_state(lit, TILE_DEADLY);
        }
    }
    fn on_frog_land(p, t, s) { landed = landed * 31 + t * 7 + s; }
    fn on_frog_death(p, cause) { deaths += 1; }
"#;

#[test]
fn determinism_and_snapshot_restore() {
    let run = |ticks: u32| {
        let mut g = game_with(&[src("hazard", HAZARD)]);
        let mut sums = Vec::new();
        for t in 0..ticks {
            if t % 7 == 0 {
                g.modscript_frog_landed(0, Some(TileId(t % 16)));
            }
            step(&mut g);
            sums.push(g.checksum());
        }
        (g, sums)
    };
    let (a, sa) = run(400);
    let (b, sb) = run(400);
    assert_eq!(sa, sb, "two runs give identical checksums");
    assert_eq!(a.modscript.units[0].globals, b.modscript.units[0].globals);
    assert!(a.map.tiles.iter().any(|t| t.state == tilestate_deadly()));

    // Snapshot mid-script, run on, restore, rerun: identical.
    let (mut g, _) = run(200);
    let snap = g.snapshot();
    let mut after = Vec::new();
    for t in 200..400u32 {
        if t % 7 == 0 {
            g.modscript_frog_landed(0, Some(TileId(t % 16)));
        }
        step(&mut g);
        after.push(g.checksum());
    }
    g.restore(&snap);
    for (k, t) in (200..400u32).enumerate() {
        if t % 7 == 0 {
            g.modscript_frog_landed(0, Some(TileId(t % 16)));
        }
        step(&mut g);
        assert_eq!(g.checksum(), after[k], "tick {t}");
    }
    assert_eq!(after.last(), sa.last());

    // Script state is part of the checksum.
    let mut h = g.clone();
    h.modscript.units[0].globals[0] = Value::Int(12345);
    assert_ne!(h.checksum(), g.checksum());
}

fn tilestate_deadly() -> u32 {
    crate::define::tilestate::DEADLY
}

#[test]
fn scope_and_demo_mode() {
    let mut s = src("scoped", "let n = 0; fn on_tick() { n += 1; }");
    s.scope = Some((0, 1));
    let mut g = game_with(&[s]);
    step(&mut g);
    assert_eq!(global(&g, 0, "n"), Value::Int(0), "level 0-0 is out of scope");
    g.frogger.player[0].level_num = 1;
    step(&mut g);
    assert_eq!(global(&g, 0, "n"), Value::Int(1));
    g.game_state.mode = DEMO_MODE;
    step(&mut g);
    assert_eq!(global(&g, 0, "n"), Value::Int(1), "no hooks in attract demos");
}

#[test]
fn events_and_text() {
    let s = r#"
        let got = "";
        let text = -1;
        fn on_pickup(p, kind) { got = got + p + ":" + kind + " "; text = show_text("picked up " + kind, 3); }
        fn on_frog_death(p, cause) { got = got + "dead" + cause + " "; }
    "#;
    let mut g = game_with(&[src("ev", s)]);
    g.modscript_pickup(0, crate::collect::GOLDCOIN_GARIB);
    step(&mut g);
    let id = match global(&g, 0, "text") {
        Value::Int(i) => i,
        v => panic!("{v:?}"),
    };
    assert!(id >= 0);
    assert_eq!(g.textover.block[id as usize].text, "picked up 6");
    assert_eq!(g.textover.block[id as usize].used, 1);
    for _ in 0..3 {
        step(&mut g);
    }
    assert_eq!(g.textover.block[id as usize].used, 0, "text expired");
    g.frogger.num_frogs = 1;
    g.frogger.player[0].frog_state |= crate::frogmove::FROGSTATUS_ISDEAD;
    g.frogger.player[0].death_by = 2;
    step(&mut g);
    step(&mut g);
    assert_eq!(global(&g, 0, "got"), Value::Str("0:6 dead2 ".into()));
}

#[test]
fn no_scripts_no_state() {
    let mut g = Game::new();
    g.map.tiles = vec![GameTile::default(); 4];
    let before = g.checksum();
    g.modscript_frog_landed(0, Some(TileId(1)));
    g.modscript_pickup(0, 1);
    g.modscript_tick();
    g.modscript_level_start(0, 0);
    assert_eq!(g.checksum(), before);
    assert!(g.take_effects().is_empty());
    assert_eq!(g.modscript_hash(), 0);
}

#[test]
fn natives_documented_consistently() {
    for (name, arity) in api::Native::all() {
        let n = api::Native::by_name(name).expect(name);
        assert_eq!(n.name(), name);
        assert_eq!(n.arity(), arity);
    }
}
