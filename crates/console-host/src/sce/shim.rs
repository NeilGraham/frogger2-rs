//! libc shims between Rust's `x86_64-unknown-freebsd` std and the PS4/PS5 system libraries.
//!
//! std is built for the FreeBSD 12+ ABI (64-bit `ino_t`: 224-byte `struct stat`, the
//! long `struct dirent`); the PS4 (FreeBSD 9) and PS5 (FreeBSD 11) kernels and libkernel keep
//! the older 32-bit `ino_t` layouts (the PS5 payload SDK's `sys/stat.h`, `sys/dirent.h` agree). The calls
//! whose structs changed are defined here (they take precedence over libkernel's at link
//! time) and translate from the native layouts. The rest are symbols std references that
//! the system libraries do not export.

use core::ffi::{c_char, c_int, c_void};

/// FreeBSD 9 `struct stat` (what `sceKernelStat` fills).
#[repr(C)]
#[derive(Default)]
struct Stat9 {
    st_dev: u32,
    st_ino: u32,
    st_mode: u16,
    st_nlink: u16,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u32,
    st_atim: [i64; 2],
    st_mtim: [i64; 2],
    st_ctim: [i64; 2],
    st_size: i64,
    st_blocks: i64,
    st_blksize: u32,
    st_flags: u32,
    st_gen: u32,
    st_lspare: i32,
    st_birthtim: [i64; 2],
}

/// FreeBSD 12+ `struct stat` (what std reads).
#[repr(C)]
struct Stat12 {
    st_dev: u64,
    st_ino: u64,
    st_nlink: u64,
    st_mode: u16,
    st_padding0: i16,
    st_uid: u32,
    st_gid: u32,
    st_padding1: i32,
    st_rdev: u64,
    st_atim: [i64; 2],
    st_mtim: [i64; 2],
    st_ctim: [i64; 2],
    st_birthtim: [i64; 2],
    st_size: i64,
    st_blocks: i64,
    st_blksize: i32,
    st_flags: u32,
    st_gen: u64,
    st_spare: [u64; 10],
}

const _: () = assert!(core::mem::size_of::<Stat9>() == 120);
const _: () = assert!(core::mem::size_of::<Stat12>() == 224);

/// FreeBSD 9 `struct dirent` header (`getdents` records).
#[repr(C)]
struct Dirent9 {
    d_fileno: u32,
    d_reclen: u16,
    d_type: u8,
    d_namlen: u8,
    // d_name follows
}

/// FreeBSD 12+ `struct dirent`.
#[repr(C)]
struct Dirent12 {
    d_fileno: u64,
    d_off: i64,
    d_reclen: u16,
    d_type: u8,
    d_pad0: u8,
    d_namlen: u16,
    d_pad1: u16,
    d_name: [u8; 256],
}

extern "C" {
    fn sceKernelStat(path: *const c_char, st: *mut Stat9) -> c_int;
    fn sceKernelFstat(fd: c_int, st: *mut Stat9) -> c_int;
    fn sceKernelOpen(path: *const c_char, flags: c_int, mode: u16) -> c_int;
    fn sceKernelClose(fd: c_int) -> c_int;
    fn sceKernelGetdents(fd: c_int, buf: *mut u8, nbytes: c_int) -> c_int;
    fn __error() -> *mut c_int;
    fn pthread_rename_np(thread: *mut c_void, name: *const c_char) -> c_int;
    fn malloc(n: usize) -> *mut c_void;
    fn free(p: *mut c_void);
}

/// `sceKernel*` calls return `0x8002_0000 | errno` on failure; the POSIX shims return -1
/// and set `errno`.
unsafe fn sce_ret(r: c_int) -> c_int {
    if r < 0 {
        *__error() = r & 0xffff;
        -1
    } else {
        r
    }
}

fn convert(s: &Stat9, o: &mut Stat12) {
    *o = Stat12 {
        st_dev: s.st_dev as u64,
        st_ino: s.st_ino as u64,
        st_nlink: s.st_nlink as u64,
        st_mode: s.st_mode,
        st_padding0: 0,
        st_uid: s.st_uid,
        st_gid: s.st_gid,
        st_padding1: 0,
        st_rdev: s.st_rdev as u64,
        st_atim: s.st_atim,
        st_mtim: s.st_mtim,
        st_ctim: s.st_ctim,
        st_birthtim: s.st_birthtim,
        st_size: s.st_size,
        st_blocks: s.st_blocks,
        st_blksize: s.st_blksize as i32,
        st_flags: s.st_flags,
        st_gen: s.st_gen as u64,
        st_spare: [0; 10],
    };
}

#[no_mangle]
unsafe extern "C" fn stat(path: *const c_char, out: *mut Stat12) -> c_int {
    let mut s = Stat9::default();
    let r = sce_ret(sceKernelStat(path, &mut s));
    if r == 0 {
        convert(&s, &mut *out);
    }
    r
}

/// No symlinks on the console filesystems the game reads: same as `stat`.
#[no_mangle]
unsafe extern "C" fn lstat(path: *const c_char, out: *mut Stat12) -> c_int {
    stat(path, out)
}

#[no_mangle]
unsafe extern "C" fn fstat(fd: c_int, out: *mut Stat12) -> c_int {
    let mut s = Stat9::default();
    let r = sce_ret(sceKernelFstat(fd, &mut s));
    if r == 0 {
        convert(&s, &mut *out);
    }
    r
}

const DIRBUF: usize = 16 * 1024;

/// Our `DIR`: the descriptor, a `getdents` buffer and the converted current entry.
#[repr(C)]
struct Dir {
    fd: c_int,
    pos: usize,
    len: usize,
    ent: Dirent12,
    buf: [u8; DIRBUF],
}

const O_RDONLY: c_int = 0;
const O_DIRECTORY: c_int = 0x0002_0000;

#[no_mangle]
unsafe extern "C" fn fdopendir(fd: c_int) -> *mut Dir {
    let d = malloc(core::mem::size_of::<Dir>()) as *mut Dir;
    if d.is_null() {
        *__error() = 12; // ENOMEM
        return core::ptr::null_mut();
    }
    (*d).fd = fd;
    (*d).pos = 0;
    (*d).len = 0;
    d
}

#[no_mangle]
unsafe extern "C" fn opendir(path: *const c_char) -> *mut Dir {
    let fd = sce_ret(sceKernelOpen(path, O_RDONLY | O_DIRECTORY, 0));
    if fd < 0 {
        return core::ptr::null_mut();
    }
    let d = fdopendir(fd);
    if d.is_null() {
        sceKernelClose(fd);
    }
    d
}

#[no_mangle]
unsafe extern "C" fn readdir(d: *mut Dir) -> *mut Dirent12 {
    let d = &mut *d;
    loop {
        if d.pos >= d.len {
            let n = sceKernelGetdents(d.fd, d.buf.as_mut_ptr(), DIRBUF as c_int);
            if n <= 0 {
                if n < 0 {
                    sce_ret(n);
                }
                return core::ptr::null_mut();
            }
            d.pos = 0;
            d.len = n as usize;
        }
        let rec = d.buf.as_ptr().add(d.pos) as *const Dirent9;
        let reclen = (*rec).d_reclen as usize;
        if reclen == 0 {
            d.len = 0;
            return core::ptr::null_mut();
        }
        d.pos += reclen;
        if (*rec).d_fileno == 0 {
            continue; // deleted entry
        }
        let namlen = ((*rec).d_namlen as usize).min(255);
        let name = (rec as *const u8).add(core::mem::size_of::<Dirent9>());
        d.ent.d_fileno = (*rec).d_fileno as u64;
        d.ent.d_off = 0;
        d.ent.d_reclen = core::mem::size_of::<Dirent12>() as u16;
        d.ent.d_type = (*rec).d_type;
        d.ent.d_namlen = namlen as u16;
        core::ptr::copy_nonoverlapping(name, d.ent.d_name.as_mut_ptr(), namlen);
        d.ent.d_name[namlen] = 0;
        return &mut d.ent;
    }
}

#[no_mangle]
unsafe extern "C" fn closedir(d: *mut Dir) -> c_int {
    let r = sce_ret(sceKernelClose((*d).fd));
    free(d as *mut c_void);
    r
}

#[no_mangle]
unsafe extern "C" fn dirfd(d: *mut Dir) -> c_int {
    (*d).fd
}

/// HashMap seeds only: TSC-seeded splitmix64 (no kernel entropy API in the stubs).
#[no_mangle]
unsafe extern "C" fn arc4random_buf(buf: *mut u8, n: usize) {
    use core::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
    let mut x = core::arch::x86_64::_rdtsc() ^ CTR.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed);
    for i in 0..n {
        if i % 8 == 0 {
            x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        }
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        *buf.add(i) = (z >> ((i % 8) * 8)) as u8;
    }
}

/// Only used by backtraces (which stay off): report no loaded objects.
#[no_mangle]
unsafe extern "C" fn dl_iterate_phdr(_cb: *mut c_void, _data: *mut c_void) -> c_int {
    0
}

#[no_mangle]
unsafe extern "C" fn pthread_setname_np(thread: *mut c_void, name: *const c_char) -> c_int {
    pthread_rename_np(thread, name)
}

extern "C" {
    fn nanosleep(req: *const [i64; 2], rem: *mut [i64; 2]) -> c_int;
    fn clock_gettime(clock: c_int, ts: *mut [i64; 2]) -> c_int;
}

/// `std::thread::sleep` / `sleep_until` (FreeBSD std uses `clock_nanosleep`; libkernel
/// has only `nanosleep`). Returns an error number like POSIX, not -1.
#[no_mangle]
unsafe extern "C" fn clock_nanosleep(clock: c_int, flags: c_int, req: *const [i64; 2], rem: *mut [i64; 2]) -> c_int {
    const TIMER_ABSTIME: c_int = 1;
    let mut rel = *req;
    if flags & TIMER_ABSTIME != 0 {
        let mut now = [0i64; 2];
        if clock_gettime(clock, &mut now) != 0 {
            return *__error();
        }
        let ns = (rel[0] - now[0]) as i128 * 1_000_000_000 + (rel[1] - now[1]) as i128;
        if ns <= 0 {
            return 0;
        }
        rel = [(ns / 1_000_000_000) as i64, (ns % 1_000_000_000) as i64];
    }
    if nanosleep(&rel, if flags & TIMER_ABSTIME != 0 { core::ptr::null_mut() } else { rem }) != 0 {
        return *__error();
    }
    0
}

/// Process control std links for `std::process::Command` (the level editor's external
/// text editor), which a console app cannot do: fail with `ENOSYS`.
macro_rules! enosys {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        #[no_mangle]
        unsafe extern "C" fn $name($(_: $ty),*) -> c_int {
            *__error() = 78; // ENOSYS
            -1
        }
    )*};
}

enosys! {
    fork();
    chroot(p: *const c_char);
    setgid(g: u32);
    setpgid(p: c_int, g: c_int);
    setsid();
    pipe2(fds: *mut c_int, flags: c_int);
}

/// Weak imports of std's `Command` (`current_dir`); every import must resolve to a
/// system library for `create-fself` / the PS5 converter, so define them (posix_spawn style: return ENOSYS).
#[no_mangle]
unsafe extern "C" fn posix_spawn_file_actions_addchdir_np(_fa: *mut c_void, _path: *const c_char) -> c_int {
    78
}

#[no_mangle]
unsafe extern "C" fn posix_spawn_file_actions_addchdir(_fa: *mut c_void, _path: *const c_char) -> c_int {
    78
}
