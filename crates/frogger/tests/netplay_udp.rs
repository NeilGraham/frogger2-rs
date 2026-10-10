//! UDP smoke run: two real `frogger --headless` processes play 600 ticks of a race level over
//! 127.0.0.1 (host prints a join code, the joiner uses it) and must end with the same state
//! checksum. Needs game data (`F2_GAME_DIR`, default `game-data/usa`); skips when absent.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn rendition() -> Option<PathBuf> {
    let base = std::env::var("F2_GAME_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../game-data/usa").into());
    let p = PathBuf::from(base);
    let root = if p.file_name().is_some_and(|n| n == "game") { p.parent()?.to_path_buf() } else { p };
    root.join("game").is_dir().then_some(root)
}

/// The first race level (`multiGameTypes`).
fn race_level() -> (u32, u32) {
    use f2_game::bank_tables::collision_file_for_coll_bank;
    use f2_game::layout::ORIG_WORLD_VISUAL_DATA;
    for (w, wvd) in ORIG_WORLD_VISUAL_DATA.iter().enumerate() {
        if f2_game::multi::MULTI_GAME_TYPES.get(w) != Some(&f2_game::multi::MULTIMODE_RACE) {
            continue;
        }
        for l in 0..wvd.num_levels as usize {
            if collision_file_for_coll_bank(wvd.level_visual_data[l].coll_bank).is_some_and(|f| f.contains("MULTI")) {
                return (w as u32, l as u32);
            }
        }
    }
    panic!("no race level")
}

fn final_checksum(out: &str) -> Option<String> {
    out.lines().find_map(|l| l.strip_prefix("NETPLAY_FINAL ").map(|s| s.to_string()))
}

#[test]
fn udp_two_processes_600_ticks() {
    let Some(root) = rendition() else {
        eprintln!("skipping: no game data (F2_GAME_DIR)");
        return;
    };
    let exe = env!("CARGO_BIN_EXE_frogger");
    let (w, l) = race_level();
    let (ws, ls) = (w.to_string(), l.to_string());
    let data = root.to_str().unwrap();
    let mut host = Command::new(exe)
        .args(["--headless", "--data", data, "--host", "--port", "0", "--host-addr", "127.0.0.1", "--multi", &ws, &ls, "2", "--frames", "600", "--pad-seed", "11"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn host");
    let mut out = BufReader::new(host.stdout.take().unwrap());
    let mut host_text = String::new();
    let code = loop {
        let mut line = String::new();
        if out.read_line(&mut line).unwrap() == 0 {
            panic!("host exited before printing a join code: {host_text}");
        }
        host_text += &line;
        if let Some(c) = line.trim().strip_prefix("JOIN_CODE ") {
            break c.to_string();
        }
    };
    let joiner = Command::new(exe).args(["--headless", "--data", data, "--join", &code, "--frames", "600", "--pad-seed", "22"]).output().expect("run joiner");
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    host_text += &rest;
    let hs = host.wait().unwrap();
    let joiner_text = String::from_utf8_lossy(&joiner.stdout).to_string();
    eprintln!("host:\n{host_text}\njoiner:\n{joiner_text}\njoiner stderr:\n{}", String::from_utf8_lossy(&joiner.stderr));
    assert!(hs.success() && joiner.status.success(), "a peer failed");
    let (a, b) = (final_checksum(&host_text).expect("host checksum"), final_checksum(&joiner_text).expect("joiner checksum"));
    assert_eq!(a, b, "final checksums differ");
}
