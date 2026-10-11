//! PS5-only libc overrides and the log sink.
//!
//! Each is correct on the console and also keeps the build running in the KytyPS5
//! emulator, whose system-library stand-ins lack these entry points (unresolved imports
//! there return 0 without doing anything, so e.g. a `rename` would silently "succeed").

use core::ffi::{c_char, c_int, c_ulong, c_void};
use std::ffi::CStr;

extern "C" {
    fn sceKernelRename(from: *const c_char, to: *const c_char) -> c_int;
    fn __error() -> *mut c_int;
    fn printf(fmt: *const c_char, ...) -> c_int;
}

/// No environment on the console: std's `env::var` (and `RUST_BACKTRACE` lookups) see
/// nothing set.
#[no_mangle]
unsafe extern "C" fn getenv(_name: *const c_char) -> *mut c_char {
    core::ptr::null_mut()
}

/// `rename` through libkernel's `sceKernelRename` (config/save writes go through
/// write-to-temporary + rename).
#[no_mangle]
unsafe extern "C" fn rename(from: *const c_char, to: *const c_char) -> c_int {
    let r = sceKernelRename(from, to);
    if r < 0 {
        *__error() = r & 0xffff;
        -1
    } else {
        0
    }
}

extern "C" {
    /// libkernel's `_umtx_op` (the link uses `--wrap=_umtx_op`).
    fn __real__umtx_op(obj: *mut c_void, op: c_int, val: c_ulong, uaddr: *mut c_void, uaddr2: *mut c_void) -> c_int;
}

/// std's futex on FreeBSD is `_umtx_op` with `UMTX_OP_WAIT_UINT_PRIVATE` (15) and
/// `UMTX_OP_WAKE_PRIVATE` (16) (also accepted: the shared forms 11 and 3). These are
/// served by one process-wide pthread mutex + condition variable instead: a wait sleeps
/// while the word still holds the expected value (checked under the mutex, so a wake that
/// follows a store cannot be lost), a wake broadcasts; spurious wake-ups are allowed by the
/// futex contract and std re-checks. KytyPS5 implements none of these `_umtx_op`
/// operations (it stops with "Unsupported _umtx_op operation"), and the console's pthreads
/// work either way; the cost, waking every waiter on any wake, is negligible for the
/// game's handful of threads. Other operations go to libkernel.
mod futex {
    use super::*;
    use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

    extern "C" {
        fn pthread_mutex_init(m: *mut *mut c_void, attr: *const c_void) -> c_int;
        fn pthread_mutex_lock(m: *mut *mut c_void) -> c_int;
        fn pthread_mutex_unlock(m: *mut *mut c_void) -> c_int;
        fn pthread_cond_init(c: *mut *mut c_void, attr: *const c_void) -> c_int;
        fn pthread_cond_wait(c: *mut *mut c_void, m: *mut *mut c_void) -> c_int;
        fn pthread_cond_timedwait(c: *mut *mut c_void, m: *mut *mut c_void, abstime: *const [i64; 2]) -> c_int;
        fn pthread_cond_broadcast(c: *mut *mut c_void) -> c_int;
        fn clock_gettime(clock: c_int, ts: *mut [i64; 2]) -> c_int;
        fn sched_yield() -> c_int;
    }

    const ETIMEDOUT: c_int = 60;
    const CLOCK_REALTIME: c_int = 0;
    const UMTX_ABSTIME: u32 = 1;

    /// `struct _umtx_time`.
    #[repr(C)]
    struct UmtxTime {
        timeout: [i64; 2],
        flags: u32,
        clockid: u32,
    }

    static mut MUTEX: *mut c_void = core::ptr::null_mut();
    static mut COND: *mut c_void = core::ptr::null_mut();
    static INIT: AtomicU8 = AtomicU8::new(0);

    #[allow(static_mut_refs)]
    unsafe fn init() {
        match INIT.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire) {
            Ok(_) => {
                pthread_mutex_init(&raw mut MUTEX, core::ptr::null());
                pthread_cond_init(&raw mut COND, core::ptr::null());
                INIT.store(2, Ordering::Release);
            }
            Err(_) => {
                while INIT.load(Ordering::Acquire) != 2 {
                    sched_yield();
                }
            }
        }
    }

    fn ns(t: [i64; 2]) -> i128 {
        t[0] as i128 * 1_000_000_000 + t[1] as i128
    }

    /// Absolute CLOCK_REALTIME deadline for a futex timeout (`size` = `uaddr`).
    unsafe fn deadline(size: usize, t: *const c_void) -> [i64; 2] {
        let (timeout, abs, clock) = if size > 16 {
            let u = &*(t as *const UmtxTime);
            (u.timeout, u.flags & UMTX_ABSTIME != 0, u.clockid as c_int)
        } else {
            (*(t as *const [i64; 2]), false, CLOCK_REALTIME)
        };
        let mut rel = ns(timeout);
        if abs {
            let mut now = [0i64; 2];
            clock_gettime(clock, &mut now);
            rel -= ns(now);
        }
        let mut now = [0i64; 2];
        clock_gettime(CLOCK_REALTIME, &mut now);
        let end = ns(now) + rel.max(0);
        [(end / 1_000_000_000) as i64, (end % 1_000_000_000) as i64]
    }

    #[allow(static_mut_refs)]
    pub unsafe fn wait(word: *const AtomicU32, expected: u32, size: usize, timeout: *const c_void) -> c_int {
        init();
        let end = (!timeout.is_null()).then(|| deadline(size, timeout));
        pthread_mutex_lock(&raw mut MUTEX);
        let mut r = 0;
        if (*word).load(Ordering::SeqCst) == expected {
            r = match end {
                Some(end) => pthread_cond_timedwait(&raw mut COND, &raw mut MUTEX, &end),
                None => pthread_cond_wait(&raw mut COND, &raw mut MUTEX),
            };
        }
        pthread_mutex_unlock(&raw mut MUTEX);
        if r == ETIMEDOUT {
            *__error() = ETIMEDOUT;
            return -1;
        }
        0
    }

    #[allow(static_mut_refs)]
    pub unsafe fn wake() -> c_int {
        init();
        pthread_mutex_lock(&raw mut MUTEX);
        pthread_cond_broadcast(&raw mut COND);
        pthread_mutex_unlock(&raw mut MUTEX);
        0
    }
}

#[no_mangle]
unsafe extern "C" fn __wrap__umtx_op(obj: *mut c_void, op: c_int, val: c_ulong, uaddr: *mut c_void, uaddr2: *mut c_void) -> c_int {
    match op {
        11 | 15 => futex::wait(obj as *const core::sync::atomic::AtomicU32, val as u32, uaddr as usize, uaddr2),
        3 | 16 => futex::wake(),
        _ => __real__umtx_op(obj, op, val, uaddr, uaddr2),
    }
}

/// The GPU build links without `--gc-sections` (the driver archive is linked whole), which
/// keeps std code (Unix sockets, fifos, process groups) that references these; the console
/// libraries do not export them and the game never calls them: fail with `ENOSYS`.
#[cfg(feature = "gpu")]
mod unused_posix {
    use core::ffi::{c_int, c_void};
    extern "C" {
        fn __error() -> *mut c_int;
    }
    macro_rules! enosys {
        ($($name:ident($($t:ty),*);)*) => {$(
            #[no_mangle]
            unsafe extern "C" fn $name($(_: $t),*) -> c_int {
                *__error() = 78; // ENOSYS
                -1
            }
        )*};
    }
    enosys! {
        mkfifo(*const c_void, u16);
        accept4(c_int, *mut c_void, *mut c_void, c_int);
        getpeereid(c_int, *mut u32, *mut u32);
        killpg(c_int, c_int);
    }
}

/// Log lines also go to the title's stdout (`printf`), which KytyPS5 prints; they reach the
/// console's kernel log through `sceKernelDebugOutText` (console_host::sce::log) in any case,
/// so on hardware a line may show up twice in klog if the title's stdout is routed there too.
pub fn log_sink(line: &CStr) {
    unsafe { printf(c"%s".as_ptr(), line.as_ptr()) };
}
