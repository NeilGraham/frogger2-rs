//! Rust std smoke test for the PS5 build (`tools/ps5/build.sh --smoke`): exercises what
//! the game needs from std on the console's system libraries (heap, threads, thread
//! locals, mutexes, files and directories, time, formatting), logs each result and
//! then shows green (all passed) or red bars with a moving stripe through the tiled
//! video out, so a run can be judged from a screenshot.

use crate::prospero::{notify, Video};
use console_host::sce::{self, Screen};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

thread_local! {
    static TLS_INIT: Cell<u32> = const { Cell::new(0x5eed) };
    static TLS_LAZY: Vec<u32> = (0..4).collect();
}

fn check(name: &str, ok: bool, detail: String, failures: &mut u32) {
    let line = format!("smoke: {} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
    sce::log(&line);
    *failures += !ok as u32;
}

pub fn run() -> i32 {
    let mut fails = 0;
    println!("smoke: println via std stdout");

    // Heap: many sizes, a big block, realloc growth.
    let v: Vec<Vec<u8>> = (0..2000).map(|i| vec![i as u8; (i * 37) % 4096 + 1]).collect();
    let big = vec![7u8; 64 << 20];
    let mut grow = Vec::new();
    for i in 0..1_000_000u32 {
        grow.push(i);
    }
    let heap_ok = v.iter().enumerate().all(|(i, b)| b[0] == i as u8) && big[(64 << 20) - 1] == 7 && grow[999_999] == 999_999;
    check("heap", heap_ok, format!("{} blocks, 64 MiB, 1M-push vec", v.len()), &mut fails);
    drop((v, big, grow));

    // Thread locals: const init (static TLS image), lazy init, per-thread copies.
    let main_tls = TLS_INIT.with(|c| c.get());
    TLS_INIT.with(|c| c.set(1));
    let other = std::thread::spawn(|| (TLS_INIT.with(|c| c.get()), TLS_LAZY.with(|v| v.iter().sum::<u32>()))).join().unwrap();
    check("thread_local", main_tls == 0x5eed && other == (0x5eed, 6), format!("main {main_tls:#x}, other thread {other:?}"), &mut fails);

    // Threads + mutex + atomics.
    let counter = Arc::new(Mutex::new(0u64));
    let atomic = Arc::new(AtomicU32::new(0));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (c, a) = (counter.clone(), atomic.clone());
            std::thread::spawn(move || {
                for _ in 0..10_000 {
                    *c.lock().unwrap() += 1;
                    a.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let (n, a) = (*counter.lock().unwrap(), atomic.load(Ordering::Relaxed));
    check("threads", n == 80_000 && a == 80_000, format!("mutex {n}, atomic {a}"), &mut fails);
    // Informational: KytyPS5 lacks sysconf/cpuset_getaffinity (the game falls back to 4).
    let par = std::thread::available_parallelism().map(|n| n.get());
    sce::log(&format!("smoke: INFO available_parallelism: {par:?}"));

    // Time.
    let t = Instant::now();
    std::thread::sleep(Duration::from_millis(50));
    let slept = t.elapsed();
    check("sleep", slept >= Duration::from_millis(45) && slept < Duration::from_millis(500), format!("{slept:?}"), &mut fails);
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    check("system_time", since > 1_500_000_000, format!("{since} s since 1970"), &mut fails);

    // HashMap (random seeds), formatting of floats.
    let m: HashMap<u32, String> = (0..1000).map(|i| (i, format!("{:.3}", i as f64 / 7.0))).collect();
    check("hashmap+fmt", m[&700] == "100.000" && m.len() == 1000, format!("m[700] = {}", m[&700]), &mut fails);

    // Files: the writable dirs the game uses, metadata, readdir, rename, remove.
    let dir = console_host::host::writable_dir(console_host::host::SAVE_DIRS);
    match &dir {
        Some(d) => {
            let p = d.join("smoke.txt");
            let w = std::fs::write(&p, b"frogger smoke test\n");
            let r = std::fs::read_to_string(&p).unwrap_or_default();
            let md = std::fs::metadata(&p).map(|m| (m.len(), m.is_file())).unwrap_or((0, false));
            let listed = std::fs::read_dir(d).map(|it| it.filter_map(|e| e.ok()).any(|e| e.file_name() == "smoke.txt")).unwrap_or(false);
            let q = d.join("smoke2.txt");
            let renamed = std::fs::rename(&p, &q).is_ok() && q.exists();
            let removed = std::fs::remove_file(&q).is_ok() && !q.exists();
            let ok = w.is_ok() && r == "frogger smoke test\n" && md == (19, true) && listed && renamed && removed;
            check("files", ok, format!("{} write {w:?} meta {md:?} listed {listed} renamed {renamed} removed {removed}", d.display()), &mut fails);
        }
        None => check("files", false, "no writable save dir".into(), &mut fails),
    }
    let app0 = std::fs::read_dir("/app0").map(|it| it.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>());
    check("read_dir /app0", app0.as_ref().is_ok_and(|v| v.iter().any(|n| n == "eboot.bin")), format!("{app0:?}"), &mut fails);
    let sfo = std::fs::read("/app0/sce_sys/param.json").map(|b| b.len());
    check("read /app0/sce_sys/param.json", sfo.as_ref().is_ok_and(|&n| n > 0), format!("{sfo:?}"), &mut fails);

    sce::log(&format!("smoke: done, {fails} failures"));
    notify(&format!("Frogger 2 PS5 smoke test: {fails} failures"));

    let mut video = match Video::open(4) {
        Ok(v) => v,
        Err(e) => {
            sce::log(&format!("smoke: video: {e}"));
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    };
    // 8 bars (green shades on success, red on failure) and a white stripe moving down.
    let (w, h) = (256u32, 144u32);
    let mut px = vec![0u32; (w * h) as usize];
    let base = if fails == 0 { 0xff00_c000u32 } else { 0xff00_00e0 };
    let pool = console_host::host::Pool::new(3);
    let exec = pool.exec();
    let mut frame = 0u32;
    loop {
        for y in 0..h {
            for x in 0..w {
                let bar = x * 8 / w;
                let shade = 0x10 * bar;
                let c = if (y + frame) % h < 6 { 0xffff_ffff } else { base | (shade << 16) | shade };
                px[(y * w + x) as usize] = c;
            }
        }
        video.show(&px, w, h, &exec);
        if frame.is_multiple_of(600) {
            sce::log(&format!("smoke: frame {frame} at {:.2} Hz", video.refresh));
        }
        frame = frame.wrapping_add(1);
    }
}
