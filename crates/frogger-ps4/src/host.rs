//! Platform-independent parts of the console host: pad conversion, the framebuffer
//! scaler and where data and saves live. Unit-tested on the build machine.

use f2_platform::{PadAxis, PadButton, RawEvent, RawPad};
use frogger::app::Step;

/// `scePadReadState` button bits (`ORBIS_PAD_BUTTON_*`).
pub mod btn {
    pub const L3: u32 = 0x0002;
    pub const R3: u32 = 0x0004;
    pub const OPTIONS: u32 = 0x0008;
    pub const UP: u32 = 0x0010;
    pub const RIGHT: u32 = 0x0020;
    pub const DOWN: u32 = 0x0040;
    pub const LEFT: u32 = 0x0080;
    pub const L2: u32 = 0x0100;
    pub const R2: u32 = 0x0200;
    pub const L1: u32 = 0x0400;
    pub const R1: u32 = 0x0800;
    pub const TRIANGLE: u32 = 0x1000;
    pub const CIRCLE: u32 = 0x2000;
    pub const CROSS: u32 = 0x4000;
    pub const SQUARE: u32 = 0x8000;
    pub const TOUCH_PAD: u32 = 0x10_0000;
}

/// Converts a DualShock 4 / DualSense state (buttons, sticks 0..255 with 128 = centre,
/// y down) to the port's generic pad: face buttons by position (Cross = South), touch
/// pad click = Select (the PC game's "back"), sticks -1..1 with up positive.
pub fn raw_pad(buttons: u32, left: [u8; 2], right: [u8; 2]) -> RawPad {
    let mut p = RawPad::default();
    let map = [
        (btn::CROSS, PadButton::South),
        (btn::CIRCLE, PadButton::East),
        (btn::SQUARE, PadButton::West),
        (btn::TRIANGLE, PadButton::North),
        (btn::L1, PadButton::L1),
        (btn::R1, PadButton::R1),
        (btn::L2, PadButton::L2),
        (btn::R2, PadButton::R2),
        (btn::TOUCH_PAD, PadButton::Select),
        (btn::OPTIONS, PadButton::Start),
        (btn::L3, PadButton::L3),
        (btn::R3, PadButton::R3),
        (btn::UP, PadButton::DPadUp),
        (btn::DOWN, PadButton::DPadDown),
        (btn::LEFT, PadButton::DPadLeft),
        (btn::RIGHT, PadButton::DPadRight),
    ];
    for (bit, b) in map {
        p.set_button(b, buttons & bit != 0);
    }
    let axis = |v: u8| ((v as f32 - 128.0) / 127.0).clamp(-1.0, 1.0);
    p.set_axis(PadAxis::LeftX, axis(left[0]));
    p.set_axis(PadAxis::LeftY, -axis(left[1]));
    p.set_axis(PadAxis::RightX, axis(right[0]));
    p.set_axis(PadAxis::RightY, -axis(right[1]));
    p
}

/// Turns successive pad states into the raw events the desktop host gets from gilrs
/// (`App::pad_event`: FMV skipping, settings navigation, rebinding): a button event on each
/// press, an axis event when a stick crosses 0.7 (the gilrs host's threshold).
#[derive(Default)]
pub struct RawEdges {
    prev: RawPad,
}

impl RawEdges {
    pub fn events(&mut self, pad: &RawPad) -> Vec<RawEvent> {
        const PUSH: f32 = 0.7;
        let mut out: Vec<RawEvent> = PadButton::ALL.iter().filter(|&&b| pad.button(b) && !self.prev.button(b)).map(|&b| RawEvent::PadButton(b)).collect();
        for a in PadAxis::ALL {
            let (v, p) = (pad.axis(a), self.prev.axis(a));
            if (v >= PUSH && p < PUSH) || (v <= -PUSH && p > -PUSH) {
                out.push(RawEvent::PadAxis { axis: a, value: v });
            }
        }
        self.prev = *pad;
        out
    }
}

/// Parses `autostart.txt` (testing without pad input): `level W L`, `demo W L`,
/// `frontend` or `boot`. Anything else: `None` (normal start-up).
pub fn autostart(text: &str) -> Option<Step> {
    let mut it = text.split_whitespace();
    let mut num = || it.next().and_then(|n| n.parse::<u32>().ok());
    match text.split_whitespace().next()? {
        "level" => {
            num();
            Some(Step::Level(num()?, num()?))
        }
        "demo" => {
            num();
            Some(Step::Demo(num()?, num()?))
        }
        "frontend" => Some(Step::Frontend),
        "boot" => Some(Step::Boot),
        _ => None,
    }
}

/// Nearest-neighbour scaler from the internal render size to the display, keeping the
/// aspect ratio (letter/pillarboxed, centred). Index tables are rebuilt on size changes.
#[derive(Default)]
pub struct Scaler {
    key: (u32, u32, u32, u32),
    /// Destination rectangle (x, y, w, h).
    pub rect: [u32; 4],
    xs: Vec<u32>,
    ys: Vec<u32>,
}

impl Scaler {
    fn setup(&mut self, sw: u32, sh: u32, dw: u32, dh: u32) {
        if self.key == (sw, sh, dw, dh) {
            return;
        }
        self.key = (sw, sh, dw, dh);
        // Largest size with the source aspect that fits.
        let (w, h) = if (dw as u64) * (sh as u64) > (dh as u64) * (sw as u64) {
            (((dh as u64 * sw as u64) / sh as u64) as u32, dh)
        } else {
            (dw, ((dw as u64 * sh as u64) / sw as u64) as u32)
        };
        self.rect = [(dw - w) / 2, (dh - h) / 2, w, h];
        self.xs = (0..w).map(|x| ((x as u64 * sw as u64) / w as u64) as u32).collect();
        self.ys = (0..h).map(|y| ((y as u64 * sh as u64) / h as u64) as u32).collect();
    }

    /// Scales `src` (`sw`×`sh`) into `dst` (`dw`×`dh`, row pitch `pitch` pixels). Pixels
    /// outside the image rectangle are left alone (the caller clears borders once per
    /// buffer). `dst` may be write-combined memory: written strictly in order.
    ///
    /// # Safety
    /// `dst` must be valid for `pitch * dh` u32 writes.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn blit(&mut self, src: &[u32], sw: u32, sh: u32, dst: *mut u32, dw: u32, dh: u32, pitch: u32) {
        self.setup(sw, sh, dw, dh);
        let [rx, ry, _, _] = self.rect;
        let mut prev: Option<(u32, *const u32)> = None;
        for (y, &sy) in self.ys.iter().enumerate() {
            let row = dst.add(((ry + y as u32) * pitch + rx) as usize);
            match prev {
                // Same source row as the line above: copy that line.
                Some((py, pline)) if py == sy => core::ptr::copy_nonoverlapping(pline, row, self.xs.len()),
                _ => {
                    let s = &src[(sy * sw) as usize..((sy + 1) * sw) as usize];
                    for (x, &sx) in self.xs.iter().enumerate() {
                        *row.add(x) = s[sx as usize];
                    }
                }
            }
            prev = Some((sy, row));
        }
    }
}

impl Scaler {
    /// [`blit`](Self::blit) split over `threads` horizontal bands of the destination.
    ///
    /// # Safety
    /// As [`blit`](Self::blit).
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn blit_par(&mut self, src: &[u32], sw: u32, sh: u32, dst: *mut u32, dw: u32, dh: u32, pitch: u32, threads: usize) {
        self.setup(sw, sh, dw, dh);
        let threads = threads.clamp(1, 16);
        if threads == 1 {
            return self.blit(src, sw, sh, dst, dw, dh, pitch);
        }
        let (xs, ys, [rx, ry, _, _]) = (&self.xs, &self.ys, self.rect);
        let band = ys.len().div_ceil(threads);
        let dst = dst as usize; // raw pointers are not Send; bands are disjoint
        std::thread::scope(|s| {
            for (i, rows) in ys.chunks(band).enumerate() {
                s.spawn(move || {
                    for (j, &sy) in rows.iter().enumerate() {
                        let y = (i * band + j) as u32;
                        let row = (dst as *mut u32).add(((ry + y) * pitch + rx) as usize);
                        let line = &src[(sy * sw) as usize..((sy + 1) * sw) as usize];
                        for (x, &sx) in xs.iter().enumerate() {
                            *row.add(x) = line[sx as usize];
                        }
                    }
                });
            }
        });
    }
}

/// The framebuffer size for the display mode VideoOut reports: 3840x2160 when the console
/// outputs 4K (PS4 Pro / PS5 with a 4K TV; needs the package's Pro-mode attribute),
/// 2560x1440 for a 1440p mode, else 1920x1080.
pub fn display_size(reported: (u32, u32)) -> (u32, u32) {
    match reported {
        (w, h) if w >= 3840 && h >= 2160 => (3840, 2160),
        (w, h) if w >= 2560 && h >= 1440 => (2560, 1440),
        _ => (1920, 1080),
    }
}

/// Dynamic internal resolution: steps the render height through [`DynRes::LEVELS`]
/// (capped by the display) to keep the CPU renderer's frame time inside budget. Starts at
/// the original 480 lines; moves up after a run of fast frames, down after slow ones.
pub struct DynRes {
    level: usize,
    max: usize,
    fast: u32,
    slow: u32,
}

impl DynRes {
    /// Render heights; 540 / 720 / 1080 scale to 1080p and 2160p by whole numbers.
    pub const LEVELS: [u32; 6] = [480, 540, 720, 900, 1080, 1440];
    /// Up when rendering takes less than this many ms for `RUN` frames in a row.
    const UP_MS: f32 = 6.0;
    /// Down when it takes more than this.
    const DOWN_MS: f32 = 12.0;
    const RUN: u32 = 120;

    pub fn new(display_h: u32) -> DynRes {
        let max = Self::LEVELS.iter().rposition(|&h| h <= display_h).unwrap_or(0);
        DynRes { level: 0, max, fast: 0, slow: 0 }
    }

    pub fn height(&self) -> u32 {
        Self::LEVELS[self.level]
    }

    /// Feeds one frame's render time; returns true when the level changed.
    pub fn update(&mut self, render_ms: f32) -> bool {
        if render_ms < Self::UP_MS {
            self.fast += 1;
            self.slow = 0;
        } else if render_ms > Self::DOWN_MS {
            self.slow += 1;
            self.fast = 0;
        } else {
            self.fast = 0;
            self.slow = 0;
        }
        if self.fast >= Self::RUN && self.level < self.max {
            self.level += 1;
            self.fast = 0;
            return true;
        }
        // Drop quickly (a quarter of the run) so a heavy scene does not stutter for long.
        if self.slow >= Self::RUN / 4 && self.level > 0 {
            self.level -= 1;
            self.slow = 0;
            return true;
        }
        false
    }
}

/// Internal render size for a display: 480 lines (the original resolution) at the
/// display's aspect ratio, so the enhanced profile's Hor+ fills a 16:9 screen.
pub fn internal_size(dw: u32, dh: u32) -> (u32, u32) {
    internal_size_at(dw, dh, 480)
}

/// [`internal_size`] at render height `h`.
pub fn internal_size_at(dw: u32, dh: u32, h: u32) -> (u32, u32) {
    let w = ((h as u64 * dw as u64) / dh.max(1) as u64) as u32;
    (w & !1, h)
}

/// Where imported game data is looked for, in order: bundled in the package, then the
/// console's internal storage, then USB drives.
pub const DATA_DIRS: &[&str] =
    &["/app0/data", "/data/frogger2", "/data/frogger2-rs", "/mnt/usb0/frogger2", "/mnt/usb1/frogger2", "/mnt/usb0/frogger2-rs", "/mnt/usb1/frogger2-rs"];

/// Writable places for the config, saves and the log, in order of preference.
pub const SAVE_DIRS: &[&str] = &["/data/frogger2", "/download0/frogger2", "/user/temp/frogger2"];

/// The first of `dirs` that can be created and written to.
pub fn writable_dir(dirs: &[&str]) -> Option<std::path::PathBuf> {
    dirs.iter().map(std::path::PathBuf::from).find(|d| {
        std::fs::create_dir_all(d).is_ok() && {
            let probe = d.join(".probe");
            let ok = std::fs::write(&probe, b"ok").is_ok();
            let _ = std::fs::remove_file(&probe);
            ok
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_mapping() {
        let p = raw_pad(btn::CROSS | btn::UP | btn::OPTIONS, [128, 0], [255, 128]);
        assert!(p.button(PadButton::South) && p.button(PadButton::DPadUp) && p.button(PadButton::Start));
        assert!(!p.button(PadButton::East));
        assert_eq!(p.axis(PadAxis::LeftX), 0.0);
        assert!(p.axis(PadAxis::LeftY) > 0.99, "stick up is positive");
        assert_eq!(p.axis(PadAxis::RightX), 1.0);
    }

    #[test]
    fn raw_edges() {
        let mut e = RawEdges::default();
        let down = raw_pad(btn::CROSS, [128, 128], [128, 128]);
        assert_eq!(e.events(&down), vec![RawEvent::PadButton(PadButton::South)]);
        assert!(e.events(&down).is_empty(), "held: no repeat");
        let up = raw_pad(0, [128, 0], [128, 128]);
        assert_eq!(e.events(&up), vec![RawEvent::PadAxis { axis: PadAxis::LeftY, value: 1.0 }]);
        assert!(e.events(&up).is_empty());
    }

    #[test]
    fn scaler_fits_and_centres() {
        let mut s = Scaler::default();
        let src: Vec<u32> = (0..640 * 480).map(|i| i as u32).collect();
        let (dw, dh) = (1920u32, 1080u32);
        let mut dst = vec![0xdead_beefu32; (dw * dh) as usize];
        unsafe { s.blit(&src, 640, 480, dst.as_mut_ptr(), dw, dh, dw) };
        assert_eq!(s.rect, [240, 0, 1440, 1080]);
        assert_eq!(dst[0], 0xdead_beef, "pillarbox untouched");
        assert_eq!(dst[240], 0, "top-left source pixel");
        assert_eq!(dst[(1079 * dw + 240 + 1439) as usize], 640 * 480 - 1, "bottom-right source pixel");
    }

    #[test]
    fn parallel_blit_matches() {
        let src: Vec<u32> = (0..852 * 480).map(|i| i as u32 * 7).collect();
        let (dw, dh) = (3840u32, 2160u32);
        let mut a = vec![0u32; (dw * dh) as usize];
        let mut b = vec![0u32; (dw * dh) as usize];
        unsafe {
            Scaler::default().blit(&src, 852, 480, a.as_mut_ptr(), dw, dh, dw);
            Scaler::default().blit_par(&src, 852, 480, b.as_mut_ptr(), dw, dh, dw, 5);
        }
        assert!(a == b);
    }

    #[test]
    fn autostart_parse() {
        assert!(matches!(autostart("level 2 1\n"), Some(Step::Level(2, 1))));
        assert!(matches!(autostart("frontend"), Some(Step::Frontend)));
        assert!(autostart("level x").is_none());
        assert!(autostart("").is_none());
    }

    #[test]
    fn display_sizes() {
        assert_eq!(display_size((3840, 2160)), (3840, 2160));
        assert_eq!(display_size((2560, 1440)), (2560, 1440));
        assert_eq!(display_size((1920, 1080)), (1920, 1080));
        assert_eq!(display_size((1280, 720)), (1920, 1080));
    }

    #[test]
    fn dynres_steps() {
        let mut d = DynRes::new(1080);
        assert_eq!(d.height(), 480);
        for _ in 0..119 {
            assert!(!d.update(3.0));
        }
        assert!(d.update(3.0));
        assert_eq!(d.height(), 540);
        for _ in 0..1000 {
            d.update(3.0);
        }
        assert_eq!(d.height(), 1080, "capped by the display");
        for _ in 0..30 {
            d.update(20.0);
        }
        assert_eq!(d.height(), 900);
        assert_eq!(DynRes::new(2160).max, 5);
    }

    #[test]
    fn internal_sizes() {
        assert_eq!(internal_size(1920, 1080), (852, 480));
        assert_eq!(internal_size(640, 480), (640, 480));
    }
}
