//! Compiler/decompiler tests on small hand-written scripts.

use f2_etc::decompile::float_text;
use f2_etc::{compile, decompile, roundtrip, Compiler, Options, Roundtrip};

fn ok(src: &str) -> Vec<u8> {
    match compile("test.etc", src.as_bytes()) {
        Ok(c) => c.data,
        Err(d) => panic!("compile failed: {d:?}"),
    }
}

fn err(src: &str) -> String {
    match compile("test.etc", src.as_bytes()) {
        Ok(c) => panic!("expected failure, got {:02x?}", c.data),
        Err(d) => d.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("\n"),
    }
}

/// `u8 version, i32 size, i16 count` + body.
fn file(count: u16, body: &[u8]) -> Vec<u8> {
    let mut v = vec![3];
    v.extend_from_slice(&(body.len() as i32 + 2).to_le_bytes());
    v.extend_from_slice(&count.to_le_bytes());
    v.extend_from_slice(body);
    v
}

#[test]
fn single_command() {
    assert_eq!(ok("SetFlag(3)"), file(1, &[24, 3]));
}

#[test]
fn empty_script() {
    assert_eq!(ok("; nothing here\n"), file(0, &[]));
}

#[test]
fn no_param_command_needs_parens() {
    assert_eq!(ok("NoFog() EndLevel()"), file(2, &[21, 47]));
    assert!(err("NoFog").contains("Expecting '('"));
}

#[test]
fn on_block_layout() {
    let src = "On Wait(1.5) 1 {\n  SetFlag(1)\n  ResetFlag(2)\n}\n";
    let mut body = vec![64, 10];
    body.extend_from_slice(&98304i32.to_le_bytes()); // 1.5 * 65536
    body.push(1); // flags
    body.extend_from_slice(&6i32.to_le_bytes()); // count + 2 commands
    body.extend_from_slice(&[2, 0, 24, 1, 25, 2]);
    assert_eq!(ok(src), file(1, &body));
}

#[test]
fn single_command_block_without_braces() {
    let a = ok("On FlagSet(4) 0 SetFlag(5)");
    let b = ok("On FlagSet(4) 0 { SetFlag(5) }");
    assert_eq!(a, b);
}

#[test]
fn if_takes_trigger_and_block_params() {
    let d = ok("If(Not(FrogIsDead(0)), { IncCounter(2) })");
    let mut body = vec![65, 66, 13, 0];
    body.extend_from_slice(&4i32.to_le_bytes());
    body.extend_from_slice(&[1, 0, 32, 2]);
    assert_eq!(d, file(1, &body));
}

#[test]
fn nested_logic_triggers() {
    let d = ok("On And(FrogOnTile(1, 300), Or(FlagSet(2), CounterEquals(3, -2))) 16 HurtFrog()");
    let mut body = vec![64, 64, 1, 1];
    body.extend_from_slice(&300i16.to_le_bytes());
    body.extend_from_slice(&[65, 11, 2, 12, 3]);
    body.extend_from_slice(&(-2i16).to_le_bytes());
    body.push(16);
    body.extend_from_slice(&3i32.to_le_bytes());
    body.extend_from_slice(&[1, 0, 29]);
    assert_eq!(d, file(1, &body));
}

#[test]
fn operand_types() {
    // word, int, byte, float, string
    let d = ok("SetEnemyFlag(-1, 123456789) PlaySound(\"boom.wav\", 7) ChangeVolume(255, -0.25, 9)");
    let mut body = vec![2, 0xFF, 0xFF];
    body.extend_from_slice(&123456789i32.to_le_bytes());
    body.extend_from_slice(&[42, 8]);
    body.extend_from_slice(b"boom.wav");
    body.extend_from_slice(&7i16.to_le_bytes());
    body.extend_from_slice(&[13, 255]);
    body.extend_from_slice(&(-16384i32).to_le_bytes());
    body.extend_from_slice(&9i16.to_le_bytes());
    assert_eq!(d, file(3, &body));
}

#[test]
fn float_truncates_like_original() {
    // (int)((float)0.1 * 65536.0f) = 6553
    let d = ok("ShakeCamera(0.1, -0.1)");
    let mut body = vec![37];
    body.extend_from_slice(&6553i32.to_le_bytes());
    body.extend_from_slice(&(-6553i32).to_le_bytes());
    assert_eq!(d, file(1, &body));
}

#[test]
fn integer_tokens_use_atoi() {
    // A number token with a fraction is accepted where an integer is wanted.
    assert_eq!(ok("SetFlag(7.9)"), file(1, &[24, 7]));
}

#[test]
fn names_are_case_insensitive() {
    assert_eq!(ok("setflag(1) SETFLAG(1)"), ok("SetFlag(1) SetFlag(1)"));
}

#[test]
fn comments_and_whitespace() {
    let src = "; header comment\r\n\tSetFlag( 1 ) ; trailing\n\n  ResetFlag(\n2\n)\n";
    assert_eq!(ok(src), file(2, &[24, 1, 25, 2]));
}

#[test]
fn set_variables() {
    let src = "Set %Frog 1\nSet %Tile 120\nset %speed 2.5\nSet %snd \"pop.wav\"\n\
               On FrogOnTile(%frog, %TILE) 1 { ShakeCamera(%speed, %speed) PlaySound(%snd, %tile) }";
    let lit = "On FrogOnTile(1, 120) 1 { ShakeCamera(2.5, 2.5) PlaySound(\"pop.wav\", 120) }";
    assert_eq!(ok(src), ok(lit));
}

#[test]
fn variable_type_checks() {
    assert!(err("Set %x 1.5\nSetFlag(%x)").contains("Expecting integer"));
    assert!(err("SetFlag(%undefined)").contains("Expecting integer"));
    assert_eq!(ok("Set %x 1\nStopSound(%x)"), ok("StopSound(\"1\")"));
}

#[test]
fn set_inside_block_is_harmless_but_noted() {
    let c = compile("t.etc", b"On FlagSet(1) 0 { Set %a 4 SetFlag(%a) }").unwrap();
    assert_eq!(c.data, ok("On FlagSet(1) 0 { SetFlag(4) }"));
    assert!(c.warnings.iter().any(|w| w.message == "fuzzy"));
}

#[test]
fn empty_block_warns() {
    let c = compile("t.etc", b"On FlagSet(1) 0 { }").unwrap();
    assert!(c.warnings.iter().any(|w| w.message.contains("no commands")));
}

#[test]
fn any_symbol_opens_a_block() {
    // BUG(original): `token[0] = '{'` — kept.
    assert_eq!(ok("On FlagSet(1) 0 ( SetFlag(2) }"), ok("On FlagSet(1) 0 { SetFlag(2) }"));
}

#[test]
fn include_files() {
    let mut c = Compiler::new(Options::default()).unwrap();
    c.lx.virtual_files.insert("common.etc".into(), b"Set %door 5\nSetFlag(1)\nSetFlag(2)\n".to_vec());
    let d = c.compile_source("main.etc", b"Include \"common.etc\"\nSetFlag(%door)\n").unwrap().data;
    assert_eq!(d, file(3, &[24, 1, 24, 2, 24, 5]));
}

#[test]
fn errors() {
    assert!(err("Frobnicate(1)").contains("'Frobnicate' is not a valid event"));
    assert!(err("SetFlag(256)").contains("Integer out of range (0-255)"));
    assert!(err("Hint(70000)").contains("Integer out of range (-65535 to 65535)"));
    assert!(err("SetFlag(1").contains("Expecting ')'"));
    assert!(err("SetCounter(1 2)").contains("Expecting ','"));
    assert!(err("On Banana(1) 1 { }").contains("'Banana' is not a valid trigger"));
    assert!(err("On 5 1 { }").contains("Found number when expecting a trigger name"));
    assert!(err("On FlagSet(1) 300 { }").contains("Expecting trigger flags"));
    assert!(err("PlaySound(\"abc, 1)").contains("No closing quote"));
    assert!(err("SetFlag(1) $").contains("Invalid character"));
    assert!(err("Set 5 5").contains("Expected variable name"));
    let e = err("SetFlag(1)\n\nSetFlag(x)");
    assert!(e.starts_with("test.etc(3) :"), "{e}");
}

#[test]
fn debug_records() {
    let mut c = Compiler::new(Options { write_debug: true, ..Default::default() }).unwrap();
    let d = c.compile_source("t.etc", b"\nSetFlag(1)").unwrap().data;
    assert_eq!(d, file(2, &[0xFF, 0, 0, 2, 0, 24, 1]));
}

#[test]
fn float_text_is_shortest_and_exact() {
    assert_eq!(float_text(65536).unwrap(), "1.0");
    assert_eq!(float_text(6553).unwrap(), "0.1");
    assert_eq!(float_text(-16384).unwrap(), "-0.25");
    assert_eq!(float_text(0).unwrap(), "0.0");
    for v in [1, -1, 12345, i32::MIN, 1 << 30, 6554, 98303] {
        let t = float_text(v).unwrap();
        let back = f2_etc::compiler::float_to_fixed(f2_etc::lexer::c_atof(t.as_bytes()));
        assert_eq!(back, v, "{v} -> {t}");
    }
    // Not reachable from any float: the decompiler reports it instead of guessing.
    assert_eq!(float_text(0x7FFF_FFFF), None);
}

#[test]
fn decompile_roundtrip_handwritten() {
    let src = "On Or(Wait(2.25), RandomWait(0.5, 3)) 1 {\n\
                 If(LevelIsOpen(2, 3), { ChangeLevel(2, 3) Debug(\"hello world\") })\n\
                 SfxEnemy(4, 2, 1.0, 0.1, 0, 10)\n\
                 On EnemyAtFlag(4, 7) 16 { MoveEnemy(4, 0) Fog(255, 128, 0, 10, -20) }\n\
               }\n\
               KillFrog(0, 3) SpringP(1, 400, 2.5, 0.75) AnimateFrog(0, 1, 2, 0.3)";
    let data = ok(src);
    let (dec, r) = roundtrip(&data).unwrap();
    assert_eq!(r, Roundtrip::Identical, "{}", String::from_utf8_lossy(&dec.source));
    let text = String::from_utf8(decompile(&data, "x").unwrap().source).unwrap();
    assert!(text.contains("%TRIGGER_ONCE"));
    assert!(text.contains("If(LevelIsOpen(2, 3), {"));
}

#[test]
fn decompile_rejects_garbage() {
    assert!(decompile(&[3, 3, 0, 0, 0, 1, 0, 200], "x").is_err()); // unknown opcode
    assert!(decompile(&[3, 9, 0, 0, 0, 0, 0], "x").is_err()); // bad size
    assert!(decompile(&[2, 2, 0, 0, 0, 0, 0], "x").is_err()); // version
    assert!(decompile(&[3, 2, 0, 0, 0, 0, 0, 0], "x").is_err()); // trailing byte
}

#[test]
fn compiled_output_loads_in_f2_formats() {
    let data = ok("On FrogOnPlatform(0, 12) 1 { SetEnemy(3, 1) TeleportP(0, 5, 1.0) }");
    let s = f2_formats::fev::Script::parse(&data).unwrap();
    assert_eq!(s.commands.len(), 1);
}

#[test]
fn codes_h_matches_reference() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frogger2-vss/teamSpirit/FroggerProject/Common/codes.h");
    let Ok(reference) = std::fs::read_to_string(&root) else {
        eprintln!("skipping: {} not found", root.display());
        return;
    };
    let reference = reference.replace("\r\n", "\n");
    let c = Compiler::new(Options::default()).unwrap();
    let ours = c.output_header("Thu May 11 19:09:59 2000\n");
    assert_eq!(ours, reference);
}

#[test]
fn asctime_format() {
    assert_eq!(f2_etc::compiler::asctime_utc(958072199), "Thu May 11 19:09:59 2000\n");
    assert_eq!(f2_etc::compiler::asctime_utc(0), "Thu Jan  1 00:00:00 1970\n");
}
