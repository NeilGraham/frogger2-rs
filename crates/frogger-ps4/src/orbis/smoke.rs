//! Rust std smoke test for the PS4 build (`tools/ps4/build-pkg.sh --smoke`, title
//! FROG20001): the smallest Rust title that proves the toolchain, start-up and loader path
//! on a console before the game is tried. Each step is logged to the kernel log
//! (`[frogger2] smoke: ...`) and `frogger.log`; then the screen shows a solid colour that
//! pulses once a second: green when every check passed, red otherwise.

use super::{klog, log, notify, open_log_file, Video};
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

thread_local! {
    static TLS_INIT: Cell<u32> = const { Cell::new(0x5eed) };
    static TLS_LAZY: Vec<u32> = (0..4).collect();
    static TLS_DROP: DropCount = const { DropCount(Cell::new(0)) };
}

/// Thread-local with a destructor: adds the value it held to [`DROPS`] when its thread exits.
struct DropCount(Cell<u32>);
static DROPS: AtomicU32 = AtomicU32::new(0);
impl Drop for DropCount {
    fn drop(&mut self) {
        DROPS.fetch_add(self.0.get(), Ordering::SeqCst);
    }
}

fn check(name: &str, ok: bool, detail: String, failures: &mut u32) {
    log(&format!("smoke: {} {name}: {detail}", if ok { "PASS" } else { "FAIL" }));
    *failures += !ok as u32;
}

pub fn run() -> i32 {
    klog(c"[frogger2] smoke: std runtime up\n");
    log("smoke: Frogger 2 PS4 Rust smoke test");
    open_log_file();
    let mut fails = 0;

    let v: Vec<Vec<u8>> = (0..2000).map(|i| vec![i as u8; (i * 37) % 4096 + 1]).collect();
    let big = vec![7u8; 32 << 20];
    let heap_ok = v.iter().enumerate().all(|(i, b)| b[0] == i as u8) && big[(32 << 20) - 1] == 7;
    check("heap", heap_ok, format!("{} blocks + 32 MiB", v.len()), &mut fails);
    drop((v, big));

    let main_tls = TLS_INIT.with(|c| c.get());
    TLS_INIT.with(|c| c.set(1));
    let other = std::thread::spawn(|| (TLS_INIT.with(|c| c.get()), TLS_LAZY.with(|v| v.iter().sum::<u32>()))).join();
    let ok = main_tls == 0x5eed && matches!(other, Ok((0x5eed, 6)));
    check("thread_local", ok, format!("main {main_tls:#x}, other thread {other:?}"), &mut fails);
    let handles: Vec<_> = (0..4).map(|i| std::thread::spawn(move || TLS_DROP.with(|d| d.0.set(10 + i)))).collect();
    for h in handles {
        let _ = h.join();
    }
    let drops = DROPS.load(Ordering::SeqCst);
    check("thread_local destructors", drops == 46, format!("sum of dropped values {drops} (expected 46)"), &mut fails);

    let counter = Arc::new(Mutex::new(0u64));
    let handles: Vec<_> = (0..6)
        .map(|_| {
            let c = counter.clone();
            std::thread::spawn(move || {
                for _ in 0..10_000 {
                    *c.lock().unwrap() += 1;
                }
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
    let n = *counter.lock().unwrap();
    check("threads+mutex", n == 60_000, format!("{n}"), &mut fails);

    let t = Instant::now();
    std::thread::sleep(Duration::from_millis(50));
    let slept = t.elapsed();
    check("sleep", slept >= Duration::from_millis(45) && slept < Duration::from_millis(500), format!("{slept:?}"), &mut fails);

    match console_host::host::writable_dir(console_host::host::SAVE_DIRS) {
        Some(d) => {
            let p = d.join("smoke.txt");
            let w = std::fs::write(&p, b"frogger smoke test\n");
            let r = std::fs::read_to_string(&p).unwrap_or_default();
            let listed = std::fs::read_dir(&d).map(|it| it.filter_map(|e| e.ok()).any(|e| e.file_name() == "smoke.txt")).unwrap_or(false);
            let ok = w.is_ok() && r == "frogger smoke test\n" && listed;
            check("files", ok, format!("{} write {w:?} listed {listed}", d.display()), &mut fails);
        }
        None => check("files", false, "no writable save dir".into(), &mut fails),
    }
    let app0 = std::fs::read_dir("/app0").map(|it| it.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>());
    check("read_dir /app0", app0.as_ref().is_ok_and(|v| v.iter().any(|n| n == "eboot.bin")), format!("{app0:?}"), &mut fails);

    log(&format!("smoke: done, {fails} failures"));
    notify(&format!("Frogger 2 PS4 smoke test: {fails} failures"));
    let mut video = match Video::open() {
        Ok(v) => v,
        Err(e) => {
            log(&format!("smoke: video: {e}"));
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    };
    log(&format!("smoke: video out {}x{}", video.width, video.height));
    // ABGR: green / red, pulsing between two shades every second.
    let (bright, dim) = if fails == 0 { (0xff00_d040u32, 0xff00_6020) } else { (0xff20_20e0, 0xff10_1070) };
    let mut frame = 0u64;
    loop {
        video.fill(if (frame / 60).is_multiple_of(2) { bright } else { dim });
        if frame.is_multiple_of(600) {
            log(&format!("smoke: frame {frame}"));
        }
        frame += 1;
    }
}
