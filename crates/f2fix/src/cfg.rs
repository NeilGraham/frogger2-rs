//! `f2fix.ini`: run settings of the oracle shim (portable, host-tested).
//!
//! Same `key=value` syntax as TPW's `tpwfix-run.cfg` (theme-park-world-recomp
//! `crates/tpwfix/src/hlcfg.rs`, MIT, same owner): one key per line, `#` or
//! `;` starts a comment, unknown keys are reported and ignored.
//!
//! ```text
//! window=offscreen        # off | offscreen (default) | hidden
//! audio.mute=1            # mute this process's audio session (default 1)
//! input.virtual=1         # DirectInput keyboard reads nothing, no joysticks (default 1)
//! reg.install_dir=        # InstallDir answer; empty = the exe's directory
//! reg.language=US         # Language answer (English | Français | Deutsch | Italiano | US)
//! reg.resolution=640x480  # Resolution answer ((w << 16) | h)
//! reg.fullscreen=0        # Fullscreen answer (headless needs 0)
//! reg.video_device=auto   # VideoDevice answer; auto = primary display device description
//! reg.control=Keyboard    # Control 1..4 answer
//! cd.drive=Q              # virtual CD drive letter (label FROGGER2)
//! cd.root=                # directory the virtual drive maps to (holds Video\); empty = install dir
//! dump.first=1            # dump the first presented frame
//! dump.every=0            # dump every N presented frames (0 = off)
//! dump.interval_ms=0      # dump the first present at least N ms after the previous dump (0 = off)
//! dump.max=100            # cap on dumps per run
//! exit.frames=0           # terminate after N presented frames (0 = never)
//! exit.seconds=0          # terminate N seconds after load (0 = never; a final dump is attempted)
//! log.presents=0          # log every Nth present (0 = off)
//! time.fixed=0            # exactly one 60 Hz frame per main-loop iteration, uncapped (UpdateTiming hook)
//! fmv.skip=0              # BinkOpen fails, so StartVideoPlayback skips every FMV
//! start.demo=             # W-L: go straight from the frontend into the attract demo of world W level L
//! start.level=            # W-L: go straight into an arcade level (scenario mode)
//! start.keys=             # scenario: .key recording replayed as held pad state (path; relative = run dir)
//! start.after=2           # frontend frames (frameCount) before start.demo / start.level fires
//! trace.enable=0          # write one JSON line of game state per GameLoop tick
//! trace.file=f2-trace.jsonl
//! trace.from=level        # level (from the started demo/level on) | all (every GameLoop)
//! trace.version=1         # 1 = base schema; 2 = + script/triggers/camera/specfx/sprites/tongue/babies/hud sections
//! snap.ticks=             # breakpoint snapshots: act_frame_count values (comma/space list) after whose GameLoop
//! snap.regions=           #   the regions are dumped to f2-snap-<tick>.json; NAME@0xADDR:LEN (LEN bytes at ADDR)
//!                         #   or NAME@*0xADDR:LEN (LEN bytes at the pointer stored at ADDR), comma/space separated
//! exit.ticks=0            # terminate after N traced ticks (0 = never)
//! exit.level_end=1        # terminate when the started demo/level is left
//! ```

/// How the game's window is kept off the desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowMode {
    /// Original behaviour (visible, activates).
    Off,
    /// Shown without activation, as a tool window, left of every monitor.
    Offscreen,
    /// Never shown at all.
    Hidden,
}

/// Parsed settings. `Default` is what an absent/empty ini means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cfg {
    pub window: WindowMode,
    pub audio_mute: bool,
    pub input_virtual: bool,
    pub install_dir: String,
    pub language: String,
    pub res_w: u32,
    pub res_h: u32,
    pub fullscreen: u32,
    pub video_device: String,
    pub control: String,
    pub cd_drive: u8,
    pub cd_root: String,
    pub dump_first: bool,
    pub dump_every: u32,
    pub dump_interval_ms: u32,
    pub dump_max: u32,
    pub exit_frames: u32,
    pub exit_seconds: u32,
    pub log_presents: u32,
    pub time_fixed: bool,
    pub fmv_skip: bool,
    pub start_demo: Option<(u32, u32)>,
    pub start_level: Option<(u32, u32)>,
    pub start_keys: String,
    pub start_after: u32,
    /// `start.multi=N` (2..=4): start `start.level` as a local multiplayer match the way
    /// the frontend's `GoMulti` does, with `N` frogs (0 = arcade).
    pub start_multi: u32,
    /// `start.chars=a,b,c,d`: `options.playerChar[i]` (frog characters, frogger.h).
    pub start_chars: [u8; 4],
    /// `start.mp_mode=M` (1 race, 2 battle, 3 collect): override `multiGameTypes[world]`
    /// before `GoMulti` (0 = the level's own mode).
    pub start_mp_mode: u32,
    pub trace_enable: bool,
    pub trace_file: String,
    pub trace_from_level: bool,
    pub trace_version: u32,
    pub snap_ticks: Vec<u32>,
    pub snap_regions: Vec<SnapRegion>,
    pub exit_ticks: u32,
    pub exit_level_end: bool,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            window: WindowMode::Offscreen,
            audio_mute: true,
            input_virtual: true,
            install_dir: String::new(),
            language: "US".into(),
            res_w: 640,
            res_h: 480,
            fullscreen: 0,
            video_device: "auto".into(),
            control: "Keyboard".into(),
            cd_drive: b'Q',
            cd_root: String::new(),
            dump_first: true,
            dump_every: 0,
            dump_interval_ms: 0,
            dump_max: 100,
            exit_frames: 0,
            exit_seconds: 0,
            log_presents: 0,
            time_fixed: false,
            fmv_skip: false,
            start_demo: None,
            start_level: None,
            start_keys: String::new(),
            start_after: 2,
            start_multi: 0,
            start_chars: [0, 1, 2, 3],
            start_mp_mode: 0,
            trace_enable: false,
            trace_file: "f2-trace.jsonl".into(),
            trace_from_level: true,
            trace_version: 1,
            snap_ticks: Vec::new(),
            snap_regions: Vec::new(),
            exit_ticks: 0,
            exit_level_end: true,
        }
    }
}

impl Cfg {
    /// The registry `Resolution` DWORD: `(w << 16) | h` (Main.cpp `resolution`).
    pub fn resolution_dword(&self) -> u32 {
        (self.res_w << 16) | (self.res_h & 0xffff)
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "on" | "yes" | "true" => Some(true),
        "0" | "off" | "no" | "false" => Some(false),
        _ => None,
    }
}

fn parse_window(v: &str) -> Option<WindowMode> {
    match v.to_ascii_lowercase().as_str() {
        "off" | "0" | "visible" => Some(WindowMode::Off),
        "offscreen" | "1" => Some(WindowMode::Offscreen),
        "hidden" => Some(WindowMode::Hidden),
        _ => None,
    }
}

/// `W-L` (world, level), or empty for none.
fn parse_wl(v: &str) -> Option<Option<(u32, u32)>> {
    if v.is_empty() {
        return Some(None);
    }
    let (w, l) = v.split_once(['-', ','])?;
    let (w, l) = (w.trim().parse::<u32>().ok()?, l.trim().parse::<u32>().ok()?);
    (w < 16 && l < 16).then_some(Some((w, l)))
}

/// One region of a breakpoint snapshot (`snap.regions`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapRegion {
    pub name: String,
    pub addr: u32,
    pub len: u32,
    /// `addr` holds a pointer; dump `len` bytes where it points.
    pub deref: bool,
}

fn parse_u32(v: &str) -> Option<u32> {
    match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => v.parse().ok(),
    }
}

fn list(v: &str) -> impl Iterator<Item = &str> {
    v.split([',', ' ', '\t']).filter(|s| !s.is_empty())
}

/// `NAME@0xADDR:LEN` / `NAME@*0xADDR:LEN`, comma/space separated (at most 1 MiB each).
pub fn parse_regions(v: &str) -> Option<Vec<SnapRegion>> {
    list(v)
        .map(|t| {
            let (name, rest) = t.split_once('@')?;
            let (deref, rest) = match rest.strip_prefix('*') {
                Some(r) => (true, r),
                None => (false, rest),
            };
            let (a, l) = rest.split_once(':')?;
            let (addr, len) = (parse_u32(a)?, parse_u32(l)?);
            (!name.is_empty() && len > 0 && len <= 1 << 20).then(|| SnapRegion { name: name.into(), addr, len, deref })
        })
        .collect()
}

fn parse_res(v: &str) -> Option<(u32, u32)> {
    let (w, h) = v.split_once(['x', 'X'])?;
    let (w, h) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    if (1..=0xffff).contains(&w) && (1..=0xffff).contains(&h) {
        Some((w, h))
    } else {
        None
    }
}

/// Parse ini text (UTF-8, or Latin-1 bytes mapped 1:1 by the caller).
/// Returns the settings and one message per rejected line.
pub fn parse(text: &str) -> (Cfg, Vec<String>) {
    let mut c = Cfg::default();
    let mut errs = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split(['#', ';']).next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            errs.push(format!("line {}: no '=': {}", n + 1, line));
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        let num = || v.parse::<u32>().ok();
        let ok = match k {
            "window" => parse_window(v).map(|x| c.window = x).is_some(),
            "audio.mute" => parse_bool(v).map(|x| c.audio_mute = x).is_some(),
            "input.virtual" => parse_bool(v).map(|x| c.input_virtual = x).is_some(),
            "reg.install_dir" => {
                c.install_dir = v.into();
                true
            }
            "reg.language" => {
                c.language = v.into();
                true
            }
            "reg.resolution" => parse_res(v).map(|(w, h)| (c.res_w, c.res_h) = (w, h)).is_some(),
            "reg.fullscreen" => num().map(|x| c.fullscreen = x).is_some(),
            "reg.video_device" => {
                c.video_device = v.into();
                true
            }
            "reg.control" => {
                c.control = v.into();
                true
            }
            "cd.drive" => match v.as_bytes() {
                [d] | [d, b':'] if d.is_ascii_alphabetic() => {
                    c.cd_drive = d.to_ascii_uppercase();
                    true
                }
                _ => false,
            },
            "cd.root" => {
                c.cd_root = v.into();
                true
            }
            "dump.first" => parse_bool(v).map(|x| c.dump_first = x).is_some(),
            "dump.every" => num().map(|x| c.dump_every = x).is_some(),
            "dump.interval_ms" => num().map(|x| c.dump_interval_ms = x).is_some(),
            "dump.max" => num().map(|x| c.dump_max = x).is_some(),
            "exit.frames" => num().map(|x| c.exit_frames = x).is_some(),
            "exit.seconds" => num().map(|x| c.exit_seconds = x).is_some(),
            "log.presents" => num().map(|x| c.log_presents = x).is_some(),
            "time.fixed" => parse_bool(v).map(|x| c.time_fixed = x).is_some(),
            "fmv.skip" => parse_bool(v).map(|x| c.fmv_skip = x).is_some(),
            "start.demo" => parse_wl(v).map(|x| c.start_demo = x).is_some(),
            "start.level" => parse_wl(v).map(|x| c.start_level = x).is_some(),
            "start.keys" => {
                c.start_keys = v.into();
                true
            }
            "start.after" => num().map(|x| c.start_after = x).is_some(),
            "start.multi" => num().filter(|x| *x == 0 || (2..=4).contains(x)).map(|x| c.start_multi = x).is_some(),
            "start.chars" => {
                let v: Option<Vec<u8>> = list(v).map(|x| parse_u32(x).filter(|n| *n < 9).map(|n| n as u8)).collect();
                match v {
                    Some(v) if !v.is_empty() && v.len() <= 4 => {
                        c.start_chars[..v.len()].copy_from_slice(&v);
                        true
                    }
                    _ => false,
                }
            }
            "start.mp_mode" => num().filter(|x| *x <= 3).map(|x| c.start_mp_mode = x).is_some(),
            "trace.enable" => parse_bool(v).map(|x| c.trace_enable = x).is_some(),
            "trace.file" => {
                c.trace_file = v.into();
                !v.is_empty()
            }
            "trace.from" => match v {
                "level" => {
                    c.trace_from_level = true;
                    true
                }
                "all" => {
                    c.trace_from_level = false;
                    true
                }
                _ => false,
            },
            "trace.version" => num().filter(|x| (1..=2).contains(x)).map(|x| c.trace_version = x).is_some(),
            "snap.ticks" => list(v).map(parse_u32).collect::<Option<Vec<_>>>().map(|x| c.snap_ticks = x).is_some(),
            "snap.regions" => parse_regions(v).map(|x| c.snap_regions = x).is_some(),
            "exit.ticks" => num().map(|x| c.exit_ticks = x).is_some(),
            "exit.level_end" => parse_bool(v).map(|x| c.exit_level_end = x).is_some(),
            _ => {
                errs.push(format!("line {}: unknown key {}", n + 1, k));
                continue;
            }
        };
        if !ok {
            errs.push(format!("line {}: bad value for {}: {}", n + 1, k, v));
        }
    }
    (c, errs)
}

/// Should present number `frame` (1-based) be dumped, given `done` dumps so
/// far and `since_ms` milliseconds since the previous dump (or load)?
pub fn want_dump(c: &Cfg, frame: u32, done: u32, since_ms: u32) -> bool {
    if done >= c.dump_max {
        return false;
    }
    (c.dump_first && frame == 1) || (c.dump_every > 0 && frame.is_multiple_of(c.dump_every)) || (c.dump_interval_ms > 0 && since_ms >= c.dump_interval_ms)
}

/// Decode an ini file's bytes: UTF-8 if valid, else Latin-1 (so `Français`
/// survives either way).
pub fn decode(bytes: &[u8]) -> String {
    match core::str::from_utf8(bytes) {
        Ok(s) => s.trim_start_matches('\u{feff}').to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Encode a setting as the game's ANSI (code page 1252 ≈ Latin-1) bytes.
/// Characters outside Latin-1 become `?`.
pub fn to_ansi(s: &str) -> Vec<u8> {
    s.chars().map(|ch| if (ch as u32) < 256 { ch as u32 as u8 } else { b'?' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let (c, e) = parse("");
        assert!(e.is_empty());
        assert_eq!(c, Cfg::default());
        assert_eq!(c.resolution_dword(), (640 << 16) + 480);
        assert_eq!(c.window, WindowMode::Offscreen);
        assert!(c.audio_mute);
    }

    #[test]
    fn parses_keys_and_comments() {
        let t = "# c\nwindow=hidden\naudio.mute=0 ; no\nreg.resolution=800x600\ncd.drive=r:\n\
                 dump.every=30\nexit.frames=500\nreg.install_dir=C:\\f2-runs\\a\\\nreg.language=Français\n";
        let (c, e) = parse(t);
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(c.window, WindowMode::Hidden);
        assert!(!c.audio_mute);
        assert_eq!((c.res_w, c.res_h), (800, 600));
        assert_eq!(c.cd_drive, b'R');
        assert_eq!(c.dump_every, 30);
        assert_eq!(c.exit_frames, 500);
        assert_eq!(c.install_dir, "C:\\f2-runs\\a\\");
        assert_eq!(to_ansi(&c.language), b"Fran\xe7ais");
    }

    #[test]
    fn rejects_bad_lines() {
        let (c, e) = parse("window=maybe\nnope\nfoo=1\nreg.resolution=640\ncd.drive=12\n");
        assert_eq!(e.len(), 5, "{e:?}");
        assert_eq!(c, Cfg::default());
    }

    #[test]
    fn oracle_keys() {
        let (c, e) = parse(
            "time.fixed=1\nfmv.skip=1\nstart.demo=0-0\nstart.level=2,3\nstart.keys=a.key\n\
                            trace.enable=1\ntrace.from=all\nexit.ticks=900\nexit.level_end=0\n",
        );
        assert!(e.is_empty(), "{e:?}");
        assert!(c.time_fixed && c.fmv_skip && c.trace_enable && !c.trace_from_level && !c.exit_level_end);
        assert_eq!(c.start_demo, Some((0, 0)));
        assert_eq!(c.start_level, Some((2, 3)));
        assert_eq!(c.start_keys, "a.key");
        assert_eq!(c.exit_ticks, 900);
        let (c, e) = parse("start.level=1-3\nstart.multi=4\nstart.chars=7, 6 5\nstart.mp_mode=2\n");
        assert!(e.is_empty(), "{e:?}");
        assert_eq!((c.start_multi, c.start_chars, c.start_mp_mode), (4, [7, 6, 5, 3], 2));
        let (_, e) = parse("start.multi=1\nstart.chars=9\nstart.mp_mode=4\n");
        assert_eq!(e.len(), 3, "{e:?}");
        let (c, e) = parse("start.demo=\nstart.level=x-1\ntrace.from=sometimes\n");
        assert_eq!(e.len(), 2, "{e:?}");
        assert_eq!(c.start_demo, None);
    }

    #[test]
    fn trace_v2_and_snapshots() {
        let (c, e) = parse("trace.version=2\nsnap.ticks=1, 200 0x10\nsnap.regions=rseed@0x479d70:4, frog0@*0x489698:64\n");
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(c.trace_version, 2);
        assert_eq!(c.snap_ticks, vec![1, 200, 16]);
        assert_eq!(
            c.snap_regions,
            vec![
                SnapRegion { name: "rseed".into(), addr: 0x479d70, len: 4, deref: false },
                SnapRegion { name: "frog0".into(), addr: 0x489698, len: 64, deref: true },
            ]
        );
        let (c, e) = parse("trace.version=3\nsnap.regions=x@0x10\nsnap.ticks=a\n");
        assert_eq!(e.len(), 3, "{e:?}");
        assert_eq!(c, Cfg::default());
    }

    #[test]
    fn dump_schedule() {
        let (mut c, _) = parse("dump.every=10\ndump.max=3");
        assert!(want_dump(&c, 1, 0, 0));
        assert!(!want_dump(&c, 2, 1, 0));
        assert!(want_dump(&c, 10, 1, 0));
        assert!(!want_dump(&c, 20, 3, 0));
        c.dump_first = false;
        assert!(!want_dump(&c, 1, 0, 0));
        c.dump_interval_ms = 1000;
        assert!(!want_dump(&c, 3, 1, 999));
        assert!(want_dump(&c, 3, 1, 1000));
    }

    #[test]
    fn decode_latin1_and_utf8() {
        assert_eq!(decode(b"a=Fran\xe7ais"), "a=Français");
        assert_eq!(decode("\u{feff}a=Français".as_bytes()), "a=Français");
    }
}
