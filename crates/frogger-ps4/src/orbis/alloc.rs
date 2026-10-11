//! `malloc` family and libm maths for the console build.
//!
//! The process heap is our own dlmalloc over libkernel `mmap` instead of the system libc's
//! (`libSceLibcInternal`) heap, whose size and setup depend on SDK parameters homebrew does
//! not control; it also lets emulators that do not ship that library run the eboot.
//! Defining the symbols here overrides the system ones at link time, so Rust's `System`
//! allocator (which calls `malloc`) and everything else share this heap.

use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::{AtomicBool, Ordering};

extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
    fn sched_yield() -> c_int;
    fn getcwd(buf: *mut c_char, size: usize) -> *mut c_char;
    fn __error() -> *mut c_int;
}

const PAGE: usize = 16 * 1024;

struct Mmap;

unsafe impl dlmalloc::Allocator for Mmap {
    fn alloc(&self, size: usize) -> (*mut u8, usize, u32) {
        const PROT_RW: c_int = 3;
        const MAP_PRIVATE_ANON: c_int = 0x0002 | 0x1000;
        let size = size.div_ceil(PAGE) * PAGE;
        let p = unsafe { mmap(core::ptr::null_mut(), size, PROT_RW, MAP_PRIVATE_ANON, -1, 0) };
        if p as isize == -1 || p.is_null() {
            (core::ptr::null_mut(), 0, 0)
        } else {
            (p as *mut u8, size, 0)
        }
    }
    fn remap(&self, _ptr: *mut u8, _old: usize, _new: usize, _can_move: bool) -> *mut u8 {
        core::ptr::null_mut()
    }
    fn free_part(&self, _ptr: *mut u8, _old: usize, _new: usize) -> bool {
        false
    }
    fn free(&self, ptr: *mut u8, size: usize) -> bool {
        unsafe { munmap(ptr as *mut c_void, size) == 0 }
    }
    fn can_release_part(&self, _flags: u32) -> bool {
        false
    }
    fn allocates_zeros(&self) -> bool {
        true
    }
    fn page_size(&self) -> usize {
        PAGE
    }
}

static LOCK: AtomicBool = AtomicBool::new(false);
static mut HEAP: dlmalloc::Dlmalloc<Mmap> = dlmalloc::Dlmalloc::new_with_allocator(Mmap);

fn with_heap<R>(f: impl FnOnce(&mut dlmalloc::Dlmalloc<Mmap>) -> R) -> R {
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        unsafe { sched_yield() };
    }
    #[allow(static_mut_refs)]
    let r = f(unsafe { &mut HEAP });
    LOCK.store(false, Ordering::Release);
    r
}

/// Each block is preceded by a header (at `ptr - HDR`): the dlmalloc block, its size and
/// alignment, so `free`/`realloc` can work without the caller's size.
const HDR: usize = 32;

unsafe fn alloc(align: usize, size: usize) -> *mut u8 {
    let align = align.max(HDR).next_power_of_two();
    let Some(total) = size.checked_add(align) else { return core::ptr::null_mut() };
    let raw = with_heap(|h| h.malloc(total, align));
    if raw.is_null() {
        *__error() = 12; // ENOMEM
        return raw;
    }
    let p = raw.add(align);
    let h = p.sub(HDR) as *mut usize;
    *h = raw as usize;
    *h.add(1) = total;
    *h.add(2) = align;
    p
}

unsafe fn header(p: *mut u8) -> (*mut u8, usize, usize) {
    let h = p.sub(HDR) as *const usize;
    (*h as *mut u8, *h.add(1), *h.add(2))
}

#[no_mangle]
unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
    alloc(16, size) as *mut c_void
}

#[no_mangle]
unsafe extern "C" fn calloc(n: usize, size: usize) -> *mut c_void {
    let Some(total) = n.checked_mul(size) else { return core::ptr::null_mut() };
    let p = alloc(16, total);
    if !p.is_null() {
        core::ptr::write_bytes(p, 0, total);
    }
    p as *mut c_void
}

#[no_mangle]
unsafe extern "C" fn free(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    let (raw, total, align) = header(p as *mut u8);
    with_heap(|h| h.free(raw, total, align));
}

#[no_mangle]
unsafe extern "C" fn realloc(p: *mut c_void, size: usize) -> *mut c_void {
    if p.is_null() {
        return malloc(size);
    }
    let (_, total, align) = header(p as *mut u8);
    let n = alloc(align, size);
    if n.is_null() {
        return core::ptr::null_mut();
    }
    core::ptr::copy_nonoverlapping(p as *const u8, n, (total - align).min(size));
    free(p);
    n as *mut c_void
}

#[no_mangle]
unsafe extern "C" fn posix_memalign(out: *mut *mut c_void, align: usize, size: usize) -> c_int {
    if !align.is_power_of_two() || align < core::mem::size_of::<usize>() {
        return 22; // EINVAL
    }
    let p = alloc(align, size);
    if p.is_null() {
        return 12;
    }
    *out = p as *mut c_void;
    0
}

/// `realpath(path, NULL)` hands back a `malloc`ed string, which must come from this heap:
/// a lexical version (absolute against `getcwd`, `.`/`..` folded; no symlinks on the
/// console filesystems the game reads).
#[no_mangle]
unsafe extern "C" fn realpath(path: *const c_char, resolved: *mut c_char) -> *mut c_char {
    let src = core::ffi::CStr::from_ptr(path).to_bytes();
    let mut full: Vec<u8> = Vec::new();
    if src.first() != Some(&b'/') {
        let mut cwd = [0 as c_char; 1024];
        if getcwd(cwd.as_mut_ptr(), cwd.len()).is_null() {
            return core::ptr::null_mut();
        }
        full.extend_from_slice(core::ffi::CStr::from_ptr(cwd.as_ptr()).to_bytes());
        full.push(b'/');
    }
    full.extend_from_slice(src);
    let mut parts: Vec<&[u8]> = Vec::new();
    for c in full.split(|&b| b == b'/') {
        match c {
            b"" | b"." => {}
            b".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let mut out = Vec::with_capacity(full.len() + 2);
    for c in &parts {
        out.push(b'/');
        out.extend_from_slice(c);
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out.push(0);
    let dst = if resolved.is_null() { malloc(out.len()) as *mut c_char } else { resolved };
    if dst.is_null() {
        return dst;
    }
    let n = if resolved.is_null() { out.len() } else { out.len().min(1024) };
    core::ptr::copy_nonoverlapping(out.as_ptr() as *const c_char, dst, n);
    dst
}

macro_rules! libm_fns {
    ($($name:ident($($a:ident: $t:ty),*) -> $r:ty;)*) => {$(
        #[no_mangle]
        extern "C" fn $name($($a: $t),*) -> $r {
            libm::$name($($a),*)
        }
    )*};
}

libm_fns! {
    sin(x: f64) -> f64;
    cos(x: f64) -> f64;
    acos(x: f64) -> f64;
    sinf(x: f32) -> f32;
    cosf(x: f32) -> f32;
    acosf(x: f32) -> f32;
    asinf(x: f32) -> f32;
    atan2f(y: f32, x: f32) -> f32;
    expf(x: f32) -> f32;
    powf(x: f32, y: f32) -> f32;
}

/// Byte comparison (the emulator's system-library stand-in stubs `bcmp` to "equal").
#[no_mangle]
unsafe extern "C" fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> c_int {
    let (a, b) = (a as *const u8, b as *const u8);
    for i in 0..n {
        let (x, y) = (*a.add(i), *b.add(i));
        if x != y {
            return x as c_int - y as c_int;
        }
    }
    0
}

#[no_mangle]
unsafe extern "C" fn bcmp(a: *const c_void, b: *const c_void, n: usize) -> c_int {
    memcmp(a, b, n)
}
