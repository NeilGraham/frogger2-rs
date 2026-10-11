//! Platform-independent parts of the console host: pad conversion, the framebuffer
//! scaler and where data and saves live. Unit-tested on the build machine.

use f2_platform::{PadAxis, PadButton, RawEvent, RawPad};

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

/// Internal render size for a display: 480 lines (the original resolution) at the
/// display's aspect ratio, so the enhanced profile's Hor+ fills a 16:9 screen.
pub fn internal_size(dw: u32, dh: u32) -> (u32, u32) {
    let h = 480;
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
    fn internal_sizes() {
        assert_eq!(internal_size(1920, 1080), (852, 480));
        assert_eq!(internal_size(640, 480), (640, 480));
    }
}
