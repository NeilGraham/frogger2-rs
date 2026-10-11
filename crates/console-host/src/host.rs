//! Platform-independent parts of the console host: pad conversion, the framebuffer
//! scalers (linear for the PS4's VideoOut buffers, tiled for the PS5's) and where data
//! and saves live. Unit-tested on the build machine.

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

/// [`internal_size`] with `lines` lines.
pub fn internal_size_at(dw: u32, dh: u32, lines: u32) -> (u32, u32) {
    let w = ((lines as u64 * dw as u64) / dh.max(1) as u64) as u32;
    (w & !1, lines)
}

/// Lines to render for a display of height `dh` on a faster CPU (PS5): 540 when that
/// scales by a whole number (2160 = 4x, 1080 = 2x), else 480 (1440 = 3x).
pub fn render_lines(dh: u32) -> u32 {
    if dh.is_multiple_of(540) {
        540
    } else {
        480
    }
}

// ------------------------------------------------------------------ tiled framebuffers

/// Bytes of one tile block of the PS5's scan-out layout (VideoOut tiling mode 0, 32-bit
/// pixels): 128x128 pixels in 64 KiB, blocks stored row-major.
pub const TILE_BLOCK_BYTES: usize = 0x1_0000;
const TILE: u32 = 128;

/// Byte offset of pixel (`x`, `y`) in a tiled buffer `width` pixels wide. The swizzle
/// inside a block is the one the PS5 Vulkan project's CPU VideoOut demo
/// (ps5-native-app-boilerplate, `src/demo_renderer.cpp`) writes on hardware; verified in
/// the KytyPS5 emulator here.
pub fn tiled_offset(x: u32, y: u32, width: u32) -> usize {
    let o = ((y << 4) & 0x70)
        ^ ((y << 5) & 0xf00)
        ^ ((y << 9) & 0x1000)
        ^ ((y << 8) & 0x4000)
        ^ ((x << 2) & 0xc)
        ^ ((x << 5) & 0x380)
        ^ ((x << 4) & 0x400)
        ^ ((x << 6) & 0x800)
        ^ ((x << 9) & 0xa000);
    let block = (y / TILE) * width.div_ceil(TILE) + x / TILE;
    ((block as usize) << 16) + o as usize
}

/// Size in bytes of a tiled buffer of `w`x`h` pixels (whole blocks).
pub fn tiled_size(w: u32, h: u32) -> usize {
    (w.div_ceil(TILE) * h.div_ceil(TILE)) as usize * TILE_BLOCK_BYTES
}

/// Nearest-neighbour scaler from the internal render size into a tiled framebuffer, keeping
/// the aspect ratio (borders black). Writes each 64 KiB block in address order, 16 bytes
/// (4 pixels) at a time, so write-combined memory sees a sequential stream; block rows are
/// split across threads.
pub struct TiledScaler {
    key: (u32, u32, u32, u32),
    /// Destination rectangle (x, y, w, h).
    pub rect: [u32; 4],
    /// Source column of each destination column (`u32::MAX`: border).
    xs: Vec<u32>,
    /// Source row of each destination row (`u32::MAX`: border).
    ys: Vec<u32>,
    /// (x, y) inside a block of each 16-byte chunk, in address order.
    chunks: Vec<(u8, u8)>,
}

impl Default for TiledScaler {
    fn default() -> Self {
        let mut chunks = vec![(0u8, 0u8); TILE_BLOCK_BYTES / 16];
        for y in 0..TILE {
            for x in (0..TILE).step_by(4) {
                chunks[tiled_offset(x, y, TILE) / 16] = (x as u8, y as u8);
            }
        }
        TiledScaler { key: (0, 0, 0, 0), rect: [0; 4], xs: Vec::new(), ys: Vec::new(), chunks }
    }
}

impl TiledScaler {
    fn setup(&mut self, sw: u32, sh: u32, dw: u32, dh: u32) {
        if self.key == (sw, sh, dw, dh) {
            return;
        }
        self.key = (sw, sh, dw, dh);
        let (w, h) = if (dw as u64) * (sh as u64) > (dh as u64) * (sw as u64) {
            (((dh as u64 * sw as u64) / sh as u64) as u32, dh)
        } else {
            (dw, ((dw as u64 * sh as u64) / sw as u64) as u32)
        };
        let (rx, ry) = ((dw - w) / 2, (dh - h) / 2);
        self.rect = [rx, ry, w, h];
        let map = |d: u32, r: u32, len: u32, s: u32| {
            if d >= r && d < r + len {
                (((d - r) as u64 * s as u64) / len as u64) as u32
            } else {
                u32::MAX
            }
        };
        // Padded to whole blocks; the padding is border.
        self.xs = (0..dw.div_ceil(TILE) * TILE).map(|x| if x < dw { map(x, rx, w, sw) } else { u32::MAX }).collect();
        self.ys = (0..dh.div_ceil(TILE) * TILE).map(|y| if y < dh { map(y, ry, h, sh) } else { u32::MAX }).collect();
    }

    /// Scales `src` (`sw`x`sh`, RGBA as u32) into the tiled buffer `dst` of `dw`x`dh`
    /// pixels in up to `parts` jobs run by `exec`; every pixel is written (borders opaque
    /// black).
    ///
    /// # Safety
    /// `dst` must be valid for [`tiled_size`]`(dw, dh)` bytes of writes and 16-byte aligned.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn blit(&mut self, src: &[u32], sw: u32, sh: u32, dst: *mut u8, dw: u32, dh: u32, parts: usize, exec: Exec) {
        assert!(src.len() >= (sw * sh) as usize);
        self.setup(sw, sh, dw, dh);
        let (bw, bh) = (dw.div_ceil(TILE) as usize, dh.div_ceil(TILE) as usize);
        let this = &*self;
        let dst = dst as usize;
        let job = move |r0: usize, r1: usize| {
            for by in r0..r1 {
                for bx in 0..bw {
                    let block = (dst + ((by * bw + bx) << 16)) as *mut [u32; 4];
                    let (x0, y0) = (bx * TILE as usize, by * TILE as usize);
                    for (c, &(cx, cy)) in this.chunks.iter().enumerate() {
                        let sy = this.ys[y0 + cy as usize];
                        let x = x0 + cx as usize;
                        let mut px = [0xff00_0000u32; 4];
                        if sy != u32::MAX {
                            let row = &src[(sy * sw) as usize..];
                            for (i, p) in px.iter_mut().enumerate() {
                                let sx = this.xs[x + i];
                                if sx != u32::MAX {
                                    *p = row[sx as usize];
                                }
                            }
                        }
                        // SAFETY: inside the buffer the caller vouches for.
                        unsafe { block.add(c).write(px) };
                    }
                }
            }
        };
        let rows_per = bh.div_ceil(parts.clamp(1, bh.max(1)));
        if rows_per >= bh {
            job(0, bh);
        } else {
            let job = &job;
            exec((0..bh).step_by(rows_per).map(|r0| Box::new(move || job(r0, (r0 + rows_per).min(bh))) as Box<dyn FnOnce() + Send>).collect());
        }
    }
}

// ------------------------------------------------------------------ worker pool

/// Runs a batch of jobs in parallel, returning once all have finished (the shape of
/// `f2_engine::soft::SoftExec`, so one executor serves the renderer and the scaler).
pub type Exec<'e> = &'e (dyn for<'j> Fn(Vec<Box<dyn FnOnce() + Send + 'j>>) + Sync);

/// [`Exec`] with one scoped thread per job (the first job runs on the caller).
pub fn scoped_exec(jobs: Vec<Box<dyn FnOnce() + Send + '_>>) {
    let mut jobs = jobs.into_iter();
    let first = jobs.next();
    std::thread::scope(|s| {
        for j in jobs {
            s.spawn(j);
        }
        if let Some(f) = first {
            f();
        }
    });
}

type Job = Box<dyn FnOnce() + Send + 'static>;

#[derive(Default)]
struct PoolState {
    jobs: std::collections::VecDeque<Job>,
    pending: usize,
    quit: bool,
}

#[derive(Default)]
struct PoolShared {
    state: std::sync::Mutex<PoolState>,
    work: std::sync::Condvar,
    done: std::sync::Condvar,
}

/// A fixed set of worker threads for the per-frame parallel work (rasteriser bands, the
/// framebuffer scaler): spawning threads every frame costs more than the work on the
/// consoles and in emulators.
pub struct Pool {
    shared: std::sync::Arc<PoolShared>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl Pool {
    /// `threads` workers (the caller's thread also runs one job of each batch).
    pub fn new(threads: usize) -> Pool {
        let shared = std::sync::Arc::new(PoolShared::default());
        let workers = (0..threads)
            .map(|i| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("pool{i}"))
                    .spawn(move || loop {
                        let job = {
                            let mut st = shared.state.lock().unwrap();
                            loop {
                                if st.quit {
                                    return;
                                }
                                if let Some(j) = st.jobs.pop_front() {
                                    break j;
                                }
                                st = shared.work.wait(st).unwrap();
                            }
                        };
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                        let mut st = shared.state.lock().unwrap();
                        st.pending -= 1;
                        if st.pending == 0 {
                            shared.done.notify_all();
                        }
                    })
                    .expect("spawning a pool thread")
            })
            .collect();
        Pool { shared, workers }
    }

    /// This pool as an [`Exec`] (`&pool.exec()`).
    pub fn exec(&self) -> impl for<'j> Fn(Vec<Box<dyn FnOnce() + Send + 'j>>) + Sync + '_ {
        move |jobs| self.run(jobs)
    }

    /// Runs `jobs` (the first on the calling thread) and returns when all are done.
    pub fn run<'j>(&self, jobs: Vec<Box<dyn FnOnce() + Send + 'j>>) {
        /// Waits for the batch even if the caller's own job unwinds, so no job outlives
        /// what it borrows.
        struct Wait<'a>(&'a PoolShared);
        impl Drop for Wait<'_> {
            fn drop(&mut self) {
                let mut st = self.0.state.lock().unwrap();
                while st.pending > 0 {
                    st = self.0.done.wait(st).unwrap();
                }
            }
        }
        let mut jobs = jobs.into_iter();
        let first = jobs.next();
        let wait = Wait(&self.shared);
        {
            let mut st = self.shared.state.lock().unwrap();
            for j in jobs {
                // SAFETY: `wait` blocks until every queued job has run, before 'j ends.
                let j: Job = unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + 'j>, Job>(j) };
                st.jobs.push_back(j);
                st.pending += 1;
            }
        }
        self.shared.work.notify_all();
        if let Some(f) = first {
            f();
        }
        drop(wait);
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().quit = true;
        self.shared.work.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

// ------------------------------------------------------------------ output modes

pub use f2_platform::config::VideoMode;

/// 59.94 Hz and 119.88 Hz, the two rates PS5 VideoOut offers a title
/// (`sceVideoOutConfigureOutput` selectors 1 and 15) in mHz.
pub const REFRESH_60_MHZ: u32 = 59_940;
pub const REFRESH_120_MHZ: u32 = 119_880;
/// Rates from here up count as high frame rate in [`pick_output_mode`].
pub const HIGH_REFRESH_MHZ: u32 = 100_000;

/// Scan-out buffer sizes the PS5 host offers, largest first (VideoOut scales each to the
/// console's video output resolution; PS5_Vulkan measured all three filling the screen).
pub const PS5_SIZES: [(u32, u32); 3] = [(3840, 2160), (2560, 1440), (1920, 1080)];

/// The output modes the PS5 host can switch between: every size of `sizes` at 59.94 Hz,
/// and at 119.88 Hz too when `high_rate` (the console said the current output supports
/// it). Largest first, faster rate first.
pub fn ps5_output_modes(sizes: &[(u32, u32)], high_rate: bool) -> Vec<VideoMode> {
    let rates: &[u32] = if high_rate { &[REFRESH_120_MHZ, REFRESH_60_MHZ] } else { &[REFRESH_60_MHZ] };
    sizes.iter().flat_map(|&(width, height)| rates.iter().map(move |&refresh_mhz| VideoMode { width, height, refresh_mhz })).collect()
}

/// Chooses the output mode for the Display settings' Resolution (`res`, `None` = Auto) and
/// Refresh rate (`refresh_mhz`, `None` = Auto) among the `modes` the display takes.
///
/// - A fixed resolution keeps that size and takes its highest refresh (4K -> 60 Hz on a
///   display without 4K at 120 Hz, 1440p -> 120 Hz where 1440p120 exists).
/// - Auto prefers high frame rate over resolution: the highest resolution that still runs
///   at 100 Hz or more, else the highest resolution at whatever rate (so 1440p120 wins
///   over 4K60: the game's interpolated frames are its main visual upgrade, and the CPU
///   renderer's internal resolution is far below either).
/// - A fixed refresh rate keeps only modes at that rate.
/// - A resolution or rate no mode has is treated as Auto (the display changed since it
///   was saved).
pub fn pick_output_mode(modes: &[VideoMode], res: Option<[u32; 2]>, refresh_mhz: Option<u32>) -> Option<VideoMode> {
    let mut cand: Vec<VideoMode> = modes.to_vec();
    if let Some([w, h]) = res {
        let same: Vec<VideoMode> = cand.iter().copied().filter(|m| (m.width, m.height) == (w, h)).collect();
        if !same.is_empty() {
            cand = same;
        }
    }
    if let Some(r) = refresh_mhz {
        let same: Vec<VideoMode> = cand.iter().copied().filter(|m| m.refresh_mhz == r).collect();
        if !same.is_empty() {
            cand = same;
        }
    }
    cand.into_iter().max_by_key(|m| (m.refresh_mhz >= HIGH_REFRESH_MHZ, m.width as u64 * m.height as u64, m.refresh_mhz))
}

// ------------------------------------------------------------------ data and saves

/// Where imported game data is looked for, in order: bundled in the package, then the
/// console's internal storage, then USB drives (and the PS5's extended storage).
pub const DATA_DIRS: &[&str] = &[
    "/app0/data",
    "/data/frogger2",
    "/data/frogger2-rs",
    "/user/data/frogger2",
    "/mnt/usb0/frogger2",
    "/mnt/usb1/frogger2",
    "/mnt/usb0/frogger2-rs",
    "/mnt/usb1/frogger2-rs",
    "/mnt/ext0/frogger2",
    "/mnt/ext1/frogger2",
];

/// Writable places for the config and saves, in order of preference. `/data` is the
/// console's user data partition (`/user/data` is the same place seen from a sandbox that
/// lacks the `/data` alias, e.g. some PS4 titles on a PS5); `/download0` is app-private.
pub const SAVE_DIRS: &[&str] = &["/data/frogger2", "/user/data/frogger2", "/download0/frogger2", "/user/temp/frogger2"];

/// Where `frogger.log` goes: the first writable place the user can reach over FTP
/// (internal storage, then a USB drive), the app-private ones last.
pub const LOG_DIRS: &[&str] =
    &["/data/frogger2", "/user/data/frogger2", "/mnt/usb0/frogger2", "/mnt/usb1/frogger2", "/download0/frogger2", "/user/temp/frogger2"];

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

    fn vm(width: u32, height: u32, refresh_mhz: u32) -> VideoMode {
        VideoMode { width, height, refresh_mhz }
    }

    /// The owner's monitor: 4K up to 60 Hz, 1440p and 1080p up to 120 Hz.
    fn owner_display() -> Vec<VideoMode> {
        vec![vm(3840, 2160, 59_940), vm(2560, 1440, 119_880), vm(2560, 1440, 59_940), vm(1920, 1080, 119_880), vm(1920, 1080, 59_940)]
    }

    #[test]
    fn auto_prefers_high_refresh_then_resolution() {
        let m = owner_display();
        assert_eq!(pick_output_mode(&m, None, None), Some(vm(2560, 1440, 119_880)));
        // Without any 120 Hz mode: the highest resolution.
        let sixty: Vec<VideoMode> = m.iter().copied().filter(|m| m.refresh_mhz < HIGH_REFRESH_MHZ).collect();
        assert_eq!(pick_output_mode(&sixty, None, None), Some(vm(3840, 2160, 59_940)));
        // A 4K120 display: 4K at 120 Hz.
        let mut tv = m.clone();
        tv.push(vm(3840, 2160, 119_880));
        assert_eq!(pick_output_mode(&tv, None, None), Some(vm(3840, 2160, 119_880)));
        assert_eq!(pick_output_mode(&[], None, None), None);
    }

    #[test]
    fn fixed_resolution_takes_its_highest_refresh() {
        let m = owner_display();
        assert_eq!(pick_output_mode(&m, Some([3840, 2160]), None), Some(vm(3840, 2160, 59_940)));
        assert_eq!(pick_output_mode(&m, Some([2560, 1440]), None), Some(vm(2560, 1440, 119_880)));
        assert_eq!(pick_output_mode(&m, Some([1920, 1080]), None), Some(vm(1920, 1080, 119_880)));
        // Fixed refresh rate.
        assert_eq!(pick_output_mode(&m, Some([1920, 1080]), Some(59_940)), Some(vm(1920, 1080, 59_940)));
        assert_eq!(pick_output_mode(&m, None, Some(59_940)), Some(vm(3840, 2160, 59_940)));
        // Unknown size / rate: Auto.
        assert_eq!(pick_output_mode(&m, Some([1280, 720]), None), Some(vm(2560, 1440, 119_880)));
        assert_eq!(pick_output_mode(&m, Some([3840, 2160]), Some(119_880)), Some(vm(3840, 2160, 59_940)));
    }

    #[test]
    fn ps5_modes() {
        let hfr = ps5_output_modes(&PS5_SIZES, true);
        assert_eq!(hfr.len(), 6);
        assert_eq!(hfr[0], vm(3840, 2160, REFRESH_120_MHZ));
        assert_eq!(pick_output_mode(&hfr, None, None), Some(vm(3840, 2160, REFRESH_120_MHZ)));
        assert_eq!(pick_output_mode(&hfr, Some([2560, 1440]), None), Some(vm(2560, 1440, REFRESH_120_MHZ)));
        assert_eq!(pick_output_mode(&hfr, None, Some(REFRESH_60_MHZ)), Some(vm(3840, 2160, REFRESH_60_MHZ)));
        let sdr = ps5_output_modes(&PS5_SIZES[1..], false);
        assert_eq!(sdr, vec![vm(2560, 1440, REFRESH_60_MHZ), vm(1920, 1080, REFRESH_60_MHZ)]);
        assert_eq!(pick_output_mode(&sdr, Some([3840, 2160]), Some(REFRESH_120_MHZ)), Some(vm(2560, 1440, REFRESH_60_MHZ)));
    }
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
        assert_eq!(internal_size_at(3840, 2160, render_lines(2160)), (960, 540));
        assert_eq!(internal_size_at(2560, 1440, render_lines(1440)), (852, 480));
        assert_eq!(render_lines(1080), 540);
    }

    #[test]
    fn pool_runs_borrowing_jobs() {
        let pool = Pool::new(4);
        for round in 0..50usize {
            let mut out = [0usize; 16];
            let jobs: Vec<Box<dyn FnOnce() + Send>> =
                out.chunks_mut(2).enumerate().map(|(i, c)| Box::new(move || c.fill(i + round)) as Box<dyn FnOnce() + Send>).collect();
            pool.run(jobs);
            assert!(out.chunks(2).enumerate().all(|(i, c)| c == [i + round, i + round]));
        }
        pool.run(Vec::new());
    }

    #[test]
    fn tiled_layout() {
        // Every pixel of a block maps to its own 4-byte slot inside the block.
        let mut seen = vec![false; TILE_BLOCK_BYTES / 4];
        for y in 0..128 {
            for x in 0..128 {
                let o = tiled_offset(x, y, 1920);
                assert_eq!(o % 4, 0);
                assert!(!std::mem::replace(&mut seen[o / 4], true));
            }
        }
        assert_eq!(tiled_offset(128, 0, 1920), TILE_BLOCK_BYTES);
        assert_eq!(tiled_offset(0, 128, 1920), 15 * TILE_BLOCK_BYTES);
        assert_eq!(tiled_size(3840, 2160), 30 * 17 * TILE_BLOCK_BYTES);
    }

    #[test]
    fn tiled_scaler_matches_reference() {
        let (sw, sh) = (96u32, 54u32);
        let src: Vec<u32> = (0..sw * sh).map(|i| i * 7 + 1).collect();
        let pool = Pool::new(3);
        let pooled = pool.exec();
        for (dw, dh, parts) in [(384u32, 216u32, 1usize), (384, 216, 3), (256, 256, 2), (200, 150, 4)] {
            let mut buf = vec![[0u32; 4]; tiled_size(dw, dh) / 16];
            let mut s = TiledScaler::default();
            let exec: Exec = if parts == 3 { &pooled } else { &scoped_exec };
            unsafe { s.blit(&src, sw, sh, buf.as_mut_ptr() as *mut u8, dw, dh, parts, exec) };
            let px: &[u32] = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u32, buf.len() * 4) };
            let [rx, ry, w, h] = s.rect;
            for y in 0..dh {
                for x in 0..dw {
                    let got = px[tiled_offset(x, y, dw) / 4];
                    let inside = x >= rx && x < rx + w && y >= ry && y < ry + h;
                    let want = if inside { src[(((y - ry) * sh / h) * sw + (x - rx) * sw / w) as usize] } else { 0xff00_0000 };
                    assert_eq!(got, want, "{dw}x{dh} at {x},{y}");
                }
            }
        }
    }
}
