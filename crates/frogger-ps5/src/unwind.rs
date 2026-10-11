//! `_Unwind_*` stand-ins. The build uses `panic=abort`, so nothing ever unwinds, but std's
//! personality routine (`rust_eh_personality`) still references these and the PS5 system
//! libraries do not export them (the payload SDK's static libunwind would drag in its own
//! libc dependencies). They are unreachable without an unwinder raising an exception;
//! should one run anyway it aborts. (`_Unwind_Backtrace`, used for backtraces, comes from
//! `libSceLibcInternal`.)

use core::ffi::c_void;

extern "C" {
    fn abort() -> !;
}

macro_rules! unreachable_unwind {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {$(
        #[no_mangle]
        unsafe extern "C" fn $name($(_: $ty),*) -> $ret {
            abort()
        }
    )*};
}

unreachable_unwind! {
    _Unwind_GetTextRelBase(ctx: *mut c_void) -> usize;
    _Unwind_GetDataRelBase(ctx: *mut c_void) -> usize;
    _Unwind_GetLanguageSpecificData(ctx: *mut c_void) -> *mut c_void;
    _Unwind_GetIPInfo(ctx: *mut c_void, before: *mut i32) -> usize;
    _Unwind_GetRegionStart(ctx: *mut c_void) -> usize;
    _Unwind_SetGR(ctx: *mut c_void, reg: i32, value: usize) -> ();
    _Unwind_SetIP(ctx: *mut c_void, value: usize) -> ();
    _Unwind_Resume(exception: *mut c_void) -> ();
    _Unwind_RaiseException(exception: *mut c_void) -> i32;
    _Unwind_DeleteException(exception: *mut c_void) -> ();
}
