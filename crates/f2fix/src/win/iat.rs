//! Import-table and vtable slot patching.
//! Adapted from theme-park-world-recomp `crates/tpwfix/src/win/iat.rs`
//! (MIT, same owner); typed hook functions instead of naked stubs.

use super::ffi;
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

/// One hooked import of a module.
pub struct Import {
    /// DLL name as in the import descriptor (case-insensitive).
    pub dll: &'static [u8],
    /// Function name (ignored when `ord` is nonzero).
    pub name: &'static [u8],
    /// Import by ordinal when nonzero.
    pub ord: u16,
    /// Address of our hook.
    pub hook: usize,
    /// Where the original is saved.
    pub orig: &'static AtomicUsize,
}

unsafe fn rd32(a: usize) -> u32 {
    (a as *const u32).read_unaligned()
}

unsafe fn cbytes<'a>(p: usize) -> &'a [u8] {
    let mut n = 0;
    while n < 256 && *((p + n) as *const u8) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(p as *const u8, n)
}

/// Overwrite one pointer slot, saving the current value in `orig` first
/// (unless it already holds `hook`).
///
/// # Safety
/// `slot` is a valid, aligned pointer slot.
pub unsafe fn patch_slot(slot: usize, hook: usize, orig: &AtomicUsize) -> bool {
    let cur = rd32(slot) as usize;
    if cur == hook {
        return true;
    }
    orig.store(cur, Ordering::Release);
    let mut old = 0u32;
    if ffi::VirtualProtect(slot as *mut c_void, 4, ffi::PAGE_READWRITE, &mut old) == 0 {
        return false;
    }
    (slot as *mut usize).write_volatile(hook);
    let mut tmp = 0u32;
    ffi::VirtualProtect(slot as *mut c_void, 4, old, &mut tmp);
    true
}

/// Patch vtable slot `i` of COM object `obj`.
///
/// # Safety
/// `obj` is a live COM object whose vtable has more than `i` slots.
pub unsafe fn patch_vtbl(obj: usize, i: usize, hook: usize, orig: &AtomicUsize) -> bool {
    let vt = *(obj as *const usize);
    patch_slot(vt + 4 * i, hook, orig)
}

/// COM vtable slot `i` of interface pointer `p`.
///
/// # Safety
/// As for [`patch_vtbl`].
pub unsafe fn vslot(p: usize, i: usize) -> usize {
    let vt = *(p as *const *const usize);
    *vt.add(i)
}

/// Patch the import table of the loaded module at `base` for every entry of
/// `table`. Returns (patched or already ours, not yet bound).
///
/// # Safety
/// `base` is a loaded PE image.
pub unsafe fn patch_imports(base: usize, table: &[Import]) -> (u32, u32) {
    let nt = base + rd32(base + 0x3C) as usize;
    let opt = nt + 24;
    let size_of_image = rd32(opt + 56) as usize;
    let imp_rva = rd32(opt + 96 + 8) as usize;
    if imp_rva == 0 {
        return (0, 0);
    }
    let (mut patched, mut pending) = (0u32, 0u32);
    let mut d = base + imp_rva;
    loop {
        let oft = rd32(d) as usize;
        let name = rd32(d + 12) as usize;
        let ft = rd32(d + 16) as usize;
        if name == 0 && ft == 0 {
            break;
        }
        let dll = cbytes(base + name);
        let lookup = if oft != 0 { oft } else { ft };
        if table.iter().any(|h| h.dll.eq_ignore_ascii_case(dll)) {
            let mut i = 0usize;
            loop {
                let t = rd32(base + lookup + 4 * i) as usize;
                if t == 0 {
                    break;
                }
                let ord = if t & 0x8000_0000 != 0 { Some((t & 0xffff) as u16) } else { None };
                let fname: &[u8] = if ord.is_none() && t < size_of_image {
                    cbytes(base + t + 2)
                } else {
                    b""
                };
                if let Some(h) = table.iter().find(|h| {
                    h.dll.eq_ignore_ascii_case(dll)
                        && match ord {
                            Some(o) => h.ord == o,
                            None => h.ord == 0 && h.name == fname,
                        }
                }) {
                    let slot = base + ft + 4 * i;
                    let cur = rd32(slot) as usize;
                    if cur == h.hook {
                        patched += 1;
                    } else if cur < size_of_image || cur == t {
                        pending += 1;
                    } else if patch_slot(slot, h.hook, h.orig) {
                        patched += 1;
                    }
                }
                i += 1;
            }
        }
        d += 20;
    }
    (patched, pending)
}

/// Load an original as its exact fn-pointer type.
macro_rules! orig {
    ($o:expr, $t:ty) => {{
        let p = $o.load(core::sync::atomic::Ordering::Acquire);
        // SAFETY: the address the loader bound for this import (the hook is
        // only reachable after it was stored), and `$t` is its prototype.
        unsafe { core::mem::transmute::<usize, $t>(p) }
    }};
}
pub(crate) use orig;
